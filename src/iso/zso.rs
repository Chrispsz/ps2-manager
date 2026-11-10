//! Codec ZSO/ZISO — binário-compatível com o driver ZSO do OPL
//! (`modules/isofs/zso.c` + `pc/ziso.py`).
//!
//! Layout verificado no código-fonte do OPL:
//! ```text
//! ZISO_header (24 bytes):
//!   +0   magic  u32 LE 0x4F53495A ("ZISO")
//!   +4   header_size u32 = 24
//!   +8   total_bytes u64
//!   +16  block_size  u32 (2048 = setor DVD)
//!   +20  ver u8 = 1
//!   +21  align u8            → unidades do índice = 2^align bytes
//!   +22  rsv [2]
//! Índice (u32 cada, a partir do offset 24):
//!   valor & 0x7FFFFFFF  = offset >> align
//!   bit 31 (0x80000000) = bloco "plain" (raw, sem compressão)
//!   último entry = fim dos dados
//! Cada bloco expande para EXATAMENTE block_size (2048) via LZ4 raw block.
//! align do writer de referência = total_bytes >> 31 (0 p/ <2GB, 1 p/ 2–4GB…)
//! ```
//!
//! Compressão multithread via rayon, 100% Rust (lz4_flex).

use crate::error::{Ps2Error, Result};
use crate::iso::parser::SectorSource;
use rayon::prelude::*;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub const ZSO_MAGIC: u32 = 0x4F53_495A; // "ZISO" little-endian
const HEADER_SIZE: u64 = 24;
const DEFAULT_BLOCK: u32 = 2048;
/// ziso.py marca "plain" quando a compressão alcança ≥95% do original.
const PLAIN_THRESHOLD: usize = 1945;
const CHUNK_BLOCKS: u64 = 4096; // 8 MiB por chunk em memória

#[derive(Debug, Clone, Copy)]
pub struct ZsoHeader {
    pub total_bytes: u64,
    pub block_size: u32,
    pub align: u8,
}

impl ZsoHeader {
    pub fn to_bytes(self) -> [u8; 24] {
        let mut b = [0u8; 24];
        b[0..4].copy_from_slice(&ZSO_MAGIC.to_le_bytes());
        b[4..8].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        b[8..16].copy_from_slice(&self.total_bytes.to_le_bytes());
        b[16..20].copy_from_slice(&self.block_size.to_le_bytes());
        b[20] = 1; // ver
        b[21] = self.align;
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() < 24 {
            return Err(Ps2Error::InvalidZso("cabeçalho truncado (<24 bytes)".into()));
        }
        let magic = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if magic != ZSO_MAGIC {
            return Err(Ps2Error::InvalidZso(format!(
                "magic 0x{magic:08X} ≠ 0x{ZSO_MAGIC:08X} (não é ZSO)"
            )));
        }
        Ok(Self {
            total_bytes: u64::from_le_bytes(b[8..16].try_into().unwrap()),
            block_size: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            align: b[21],
        })
    }
}

/// LZ4 raw block com tolerância a padding de alinhamento.
fn lz4_decode(src: &[u8], dst: &mut [u8]) -> std::result::Result<(), String> {
    match lz4_flex::decompress_into(src, dst) {
        Ok(_) => Ok(()),
        Err(_) => {
            // blocos gravados com align ≥1 carregam padding zero no fim
            let mut end = src.len();
            while end > 0 && src[end - 1] == 0 {
                end -= 1;
            }
            lz4_flex::decompress_into(&src[..end], dst)
                .map(|_| ())
                .map_err(|e| format!("LZ4: {e}"))
        }
    }
}

/// Leitor de setores de um .zso — decomprime blocos on-demand.
pub struct ZsoSource {
    file: File,
    hdr: ZsoHeader,
}

impl ZsoSource {
    pub fn open(path: &Path) -> Result<Self> {
        let mut f = File::open(path).map_err(Ps2Error::Io)?;
        let mut hb = [0u8; 24];
        f.read_exact(&mut hb).map_err(Ps2Error::Io)?;
        let hdr = ZsoHeader::from_bytes(&hb)?;
        if hdr.block_size != DEFAULT_BLOCK {
            return Err(Ps2Error::InvalidZso(format!(
                "block_size {} não suportado (apenas 2048)",
                hdr.block_size
            )));
        }
        Ok(Self { file: f, hdr })
    }

    #[allow(dead_code)]
    pub fn header(&self) -> ZsoHeader {
        self.hdr
    }

    fn index_entry(&mut self, i: u64) -> std::io::Result<u32> {
        self.file
            .seek(SeekFrom::Start(HEADER_SIZE + i * 4))?;
        let mut b = [0u8; 4];
        self.file.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
}

impl SectorSource for ZsoSource {
    fn read_at(&mut self, lba: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let bs = self.hdr.block_size as u64;
        let unit = 1u64 << self.hdr.align;
        let mut done = 0usize;
        while done < buf.len() {
            let sec = lba + (done as u64) / bs;
            let in_off = ((done as u64) % bs) as usize;
            let chunk = (buf.len() - done).min(bs as usize - in_off);
            let idx0 = self.index_entry(sec)?;
            let idx1 = self.index_entry(sec + 1)?;
            let o0 = ((idx0 & 0x7FFF_FFFF) as u64) * unit;
            let o1 = ((idx1 & 0x7FFF_FFFF) as u64) * unit;
            let clen = (o1 - o0) as usize;
            if clen == 0 || clen > 64 * 1024 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("bloco {sec} com tamanho inválido ({clen})"),
                ));
            }
            self.file.seek(SeekFrom::Start(o0))?;
            let mut cbuf = vec![0u8; clen];
            self.file.read_exact(&mut cbuf)?;
            let mut block = vec![0u8; bs as usize];
            if idx0 & 0x8000_0000 != 0 {
                let n = clen.min(block.len());
                block[..n].copy_from_slice(&cbuf[..n]);
            } else {
                lz4_decode(&cbuf, &mut block).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                })?;
            }
            buf[done..done + chunk]
                .copy_from_slice(&block[in_off..in_off + chunk]);
            done += chunk;
        }
        Ok(())
    }
}

/// Converte ISO → ZSO com compressão LZ4 paralela (rayon, todos os cores).
/// Retorna (bytes_comprimidos, bytes_originais).
pub fn compress_iso(
    input: &Path,
    output: &Path,
    progress: Option<Arc<AtomicU64>>,
) -> Result<(u64, u64)> {
    let fin = File::open(input).map_err(Ps2Error::Io)?;
    let total = fin.metadata().map_err(Ps2Error::Io)?.len();
    let mut fin = BufReader::new(fin);
    let block = DEFAULT_BLOCK as u64;
    let nblocks = total.div_ceil(block);
    // idêntico ao writer de referência: align = total_bytes // 2**31
    let align: u8 = (total >> 31) as u8;
    let unit: u64 = 1 << align;
    let index_bytes = HEADER_SIZE + (nblocks + 1) * 4;
    let data_start = index_bytes.div_ceil(unit) * unit;

    let hdr = ZsoHeader { total_bytes: total, block_size: DEFAULT_BLOCK, align };
    let mut fout = BufWriter::new(File::create(output).map_err(Ps2Error::Io)?);
    fout.write_all(&hdr.to_bytes()).map_err(Ps2Error::Io)?;
    fout.write_all(&vec![0u8; (nblocks + 1) as usize * 4])
        .map_err(Ps2Error::Io)?; // placeholder do índice
    let cur = HEADER_SIZE + (nblocks + 1) * 4;
    if cur < data_start {
        fout.write_all(&vec![0u8; (data_start - cur) as usize])
            .map_err(Ps2Error::Io)?;
    }

    let mut index = vec![0u32; (nblocks + 1) as usize];
    let mut pos = data_start;
    let mut read_buf = vec![0u8; (CHUNK_BLOCKS * block) as usize];
    let mut sector: u64 = 0;

    while sector < nblocks {
        let count = (CHUNK_BLOCKS.min(nblocks - sector)) as usize;
        let want = count * DEFAULT_BLOCK as usize;
        let valid = ((total - sector * block) as usize).min(want);
        fin.read_exact(&mut read_buf[..valid]).map_err(Ps2Error::Io)?;
        read_buf[valid..want].fill(0);

        let results: Vec<(bool, Vec<u8>)> = read_buf[..want]
            .par_chunks(DEFAULT_BLOCK as usize)
            .map(|blk| {
                let c = lz4_flex::compress(blk);
                if c.len() >= PLAIN_THRESHOLD {
                    (true, blk.to_vec())
                } else {
                    (false, c)
                }
            })
            .collect();

        for (i, (plain, data)) in results.iter().enumerate() {
            let pad = (unit - (pos % unit)) % unit;
            if pad > 0 {
                fout.write_all(&vec![0u8; pad as usize]).map_err(Ps2Error::Io)?;
                pos += pad;
            }
            index[sector as usize + i] =
                ((pos / unit) as u32) | if *plain { 0x8000_0000 } else { 0 };
            fout.write_all(data).map_err(Ps2Error::Io)?;
            pos += data.len() as u64;
        }
        sector += count as u64;
        if let Some(p) = &progress {
            p.store(sector * block, Ordering::Relaxed);
        }
    }
    index[nblocks as usize] = ((pos / unit) as u32) & 0x7FFF_FFFF;

    fout.flush().map_err(Ps2Error::Io)?;
    let mut f = fout.into_inner().map_err(|e| Ps2Error::Other(e.to_string()))?;
    f.seek(SeekFrom::Start(0)).map_err(Ps2Error::Io)?;
    f.write_all(&hdr.to_bytes()).map_err(Ps2Error::Io)?;
    let idx_bytes: Vec<u8> = index.iter().flat_map(|v| v.to_le_bytes()).collect();
    f.write_all(&idx_bytes).map_err(Ps2Error::Io)?;
    f.sync_all().ok();
    Ok((pos, total))
}

/// Converte ZSO → ISO (decompressão).
/// Retorna bytes escritos.
pub fn decompress_zso(
    input: &Path,
    output: &Path,
    progress: Option<Arc<AtomicU64>>,
) -> Result<u64> {
    let mut file = File::open(input).map_err(Ps2Error::Io)?;
    let mut hb = [0u8; 24];
    file.read_exact(&mut hb).map_err(Ps2Error::Io)?;
    let hdr = ZsoHeader::from_bytes(&hb)?;
    if hdr.block_size != DEFAULT_BLOCK {
        return Err(Ps2Error::InvalidZso("block_size ≠ 2048".into()));
    }
    let bs = hdr.block_size as u64;
    let nblocks = hdr.total_bytes.div_ceil(bs);
    let mut idx_bytes = vec![0u8; ((nblocks + 1) * 4) as usize];
    file.seek(SeekFrom::Start(HEADER_SIZE)).map_err(Ps2Error::Io)?;
    file.read_exact(&mut idx_bytes).map_err(Ps2Error::Io)?;
    let (idx_chunks, _rest) = idx_bytes.as_chunks::<4>();
    let idx: Vec<u32> = idx_chunks
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();

    let unit = 1u64 << hdr.align;
    let mut fout = BufWriter::new(File::create(output).map_err(Ps2Error::Io)?);
    let mut buf_out = vec![0u8; bs as usize];
    for sec in 0..nblocks {
        let i0 = (idx[sec as usize] & 0x7FFF_FFFF) as u64 * unit;
        let i1 = (idx[(sec + 1) as usize] & 0x7FFF_FFFF) as u64 * unit;
        let clen = (i1 - i0) as usize;
        if clen == 0 || clen > 64 * 1024 * 1024 {
            return Err(Ps2Error::InvalidZso(format!(
                "bloco {sec} com tamanho inválido ({clen})"
            )));
        }
        file.seek(SeekFrom::Start(i0)).map_err(Ps2Error::Io)?;
        let mut cbuf = vec![0u8; clen];
        file.read_exact(&mut cbuf).map_err(Ps2Error::Io)?;
        let out_len = (hdr.total_bytes - sec * bs).min(bs) as usize;
        if idx[sec as usize] & 0x8000_0000 != 0 {
            fout.write_all(&cbuf[..out_len]).map_err(Ps2Error::Io)?;
        } else {
            buf_out[..out_len].fill(0);
            lz4_decode(&cbuf, &mut buf_out).map_err(Ps2Error::InvalidZso)?;
            fout.write_all(&buf_out[..out_len]).map_err(Ps2Error::Io)?;
        }
        if let Some(p) = &progress {
            p.store((sec + 1) * bs, Ordering::Relaxed);
        }
    }
    fout.flush().map_err(Ps2Error::Io)?;
    Ok(hdr.total_bytes)
}
