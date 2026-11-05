//! VMC Manager — leitura de cartões de memória virtual `.bin` do OPL.
//!
//! Layout do superblock réplica de `MCDevInfo`
//! (`modules/vmc/genvmc/genvmc.c:82-118`):
//! ```text
//! +0   magic[28]  "Sony PS2 Memory Card Format "
//! +28  version[12] "1.2.0.0"
//! +40  pagesize s16          +42 pages_per_cluster u16   +44 blocksize u16
//! +48  clusters_per_card u32 +52 alloc_offset u32        +56 alloc_end u32
//! +60  rootdir_cluster u32   +336 cardtype u8 (2 = PS2)  +340 cluster_size u32
//! +344 FATentries_per_cluster u32 +352 cardform s32  +368 max_alloc_clusters
//! ```
//! Clusters e entradas de diretório: entradas de 512 bytes
//! (mode/length/created/modified/cluster/parent/name[32]).
//! Exportação PSU marcada como EXPERIMENTAL.

use crate::error::{Ps2Error, Result};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: &[u8; 28] = b"Sony PS2 Memory Card Format ";
const ENTRY_SIZE: usize = 512;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VmcInfo {
    pub pagesize: u16,
    pub pages_per_cluster: u16,
    pub clusters_per_card: u32,
    pub alloc_offset: u32,
    pub rootdir_cluster: u32,
    pub cluster_size: u32,
    pub max_allocatable_clusters: u32,
}

pub struct Vmc {
    file: std::fs::File,
    pub info: VmcInfo,
    fat: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct VmcEntry {
    pub name: String,
    pub mode: u32,
    pub length: u32,
    pub cluster: u32,
    pub created: [u8; 8],
    pub modified: [u8; 8],
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VmcSave {
    pub name: String,
    pub size_bytes: u64,
    pub files: usize,
    pub cluster: u32,
}

fn parse_entry(buf: &[u8], name_off: usize) -> Option<VmcEntry> {
    if buf.len() < ENTRY_SIZE || name_off + 32 > ENTRY_SIZE {
        return None;
    }
    let mode = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    let length = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    let cluster = u32::from_le_bytes(buf[24..28].try_into().unwrap());
    let mut created = [0u8; 8];
    created.copy_from_slice(&buf[8..16]);
    let mut modified = [0u8; 8];
    modified.copy_from_slice(&buf[16..24]);
    let name_end = buf[name_off..name_off + 32]
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(32);
    let name = String::from_utf8_lossy(&buf[name_off..name_off + name_end]).to_string();
    if mode == 0 {
        return None;
    }
    Some(VmcEntry {
        name,
        mode,
        length,
        cluster,
        created,
        modified,
    })
}

impl Vmc {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = std::fs::File::open(path).map_err(Ps2Error::Io)?;
        let mut sb = vec![0u8; 1024];
        file.read_exact(&mut sb).map_err(Ps2Error::Io)?;
        if &sb[0..28] != MAGIC {
            return Err(Ps2Error::InvalidVmc(
                "superblock ausente — não é uma VMC PS2 formatada".into(),
            ));
        }
        let pagesize = i16::from_le_bytes([sb[40], sb[41]]) as u16;
        let pages_per_cluster = u16::from_le_bytes([sb[42], sb[43]]);
        let clusters_per_card = u32::from_le_bytes(sb[48..52].try_into().unwrap());
        let alloc_offset = u32::from_le_bytes(sb[52..56].try_into().unwrap());
        let rootdir_cluster = u32::from_le_bytes(sb[60..64].try_into().unwrap());
        let cardtype = sb[336];
        let cluster_size = u32::from_le_bytes(sb[340..344].try_into().unwrap());
        let max_alloc = u32::from_le_bytes(sb[368..372].try_into().unwrap());
        if cardtype != 2 {
            return Err(Ps2Error::InvalidVmc(format!(
                "cardtype {cardtype} ≠ 2 (não é memory card PS2)"
            )));
        }
        if pagesize == 0 || cluster_size == 0 || pages_per_cluster == 0 {
            return Err(Ps2Error::InvalidVmc("geometria inválida no superblock".into()));
        }
        // FAT: começa no alloc_offset, max_allocatable_clusters entradas u32
        let fat_bytes = (max_alloc as usize).saturating_mul(4);
        let mut fat = Vec::new();
        if fat_bytes > 0 {
            file.seek(SeekFrom::Start(alloc_offset as u64 * cluster_size as u64))
                .map_err(Ps2Error::Io)?;
            let mut fb = vec![0u8; fat_bytes.min(32 * 1024 * 1024)];
            file.read_exact(&mut fb).map_err(Ps2Error::Io)?;
            fat = fb
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
        }
        Ok(Self {
            file,
            info: VmcInfo {
                pagesize,
                pages_per_cluster,
                clusters_per_card,
                alloc_offset,
                rootdir_cluster,
                cluster_size,
                max_allocatable_clusters: max_alloc,
            },
            fat,
        })
    }

    fn read_cluster(&mut self, rel: u32) -> Result<Vec<u8>> {
        let off = (self.info.alloc_offset + rel) as u64 * self.info.cluster_size as u64;
        self.file.seek(SeekFrom::Start(off)).map_err(Ps2Error::Io)?;
        let mut buf = vec![0u8; self.info.cluster_size as usize];
        self.file.read_exact(&mut buf).map_err(Ps2Error::Io)?;
        Ok(buf)
    }

    fn read_chain(&mut self, first: u32) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut c = first;
        let mut hops = 0usize;
        loop {
            out.extend_from_slice(&self.read_cluster(c)?);
            let next = *self.fat.get(c as usize).unwrap_or(&0xFFFF_FFFF);
            if next == 0xFFFF_FFFF || next == 0xFFFF_FFFD || next == 0 {
                break;
            }
            c = next;
            hops += 1;
            if hops > 200_000 {
                return Err(Ps2Error::InvalidVmc("cadeia de clusters cíclica".into()));
            }
        }
        Ok(out)
    }

    /// Entradas de um diretório; tenta name@36 (padrão genvmc) e name@32.
    fn read_dir_entries(&mut self, cluster: u32) -> Result<Vec<VmcEntry>> {
        let data = self.read_chain(cluster)?;
        let mut out = Vec::new();
        for name_off in [36usize, 32usize] {
            out.clear();
            for chunk in data.chunks(ENTRY_SIZE) {
                if let Some(e) = parse_entry(chunk, name_off) {
                    out.push(e);
                }
            }
            let dots = out.iter().filter(|e| e.name == "." || e.name == "..").count();
            if dots >= 2 {
                return Ok(out);
            }
        }
        Ok(out)
    }

    /// Lista os saves (diretórios de 2º nível do root).
    pub fn list_saves(&mut self) -> Result<Vec<VmcSave>> {
        let root_entries = self.read_dir_entries(self.info.rootdir_cluster)?;
        let mut saves = Vec::new();
        for e in root_entries {
            let is_dir = e.mode & 0x8027 == 0x8027 || e.mode & 0x0027 == 0x0027;
            if !is_dir || e.name == "." || e.name == ".." {
                continue;
            }
            let files = self.read_dir_entries(e.cluster)?;
            let mut size = 0u64;
            let mut nfiles = 0usize;
            for f in &files {
                let is_file = f.mode & 0x0025 != 0 && f.mode & 0x0002 == 0;
                if f.name != "." && f.name != ".." && is_file {
                    size += f.length as u64;
                    nfiles += 1;
                }
            }
            saves.push(VmcSave {
                name: e.name,
                size_bytes: size,
                files: nfiles,
                cluster: e.cluster,
            });
        }
        saves.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(saves)
    }

    /// Exporta um save para .psu (EXPERIMENTAL — convenção Sjoerd/mcio:
    /// headers de 512B, dados de arquivo alinhados a 1024B).
    pub fn export_psu(&mut self, save_name: &str, out_path: &Path) -> Result<u64> {
        let root_entries = self.read_dir_entries(self.info.rootdir_cluster)?;
        let dir = root_entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(save_name))
            .ok_or_else(|| {
                Ps2Error::InvalidVmc(format!("save '{save_name}' não encontrado"))
            })?
            .clone();
        let entries = self.read_dir_entries(dir.cluster)?;
        let real: Vec<&VmcEntry> = entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .collect();

        let mut out: Vec<u8> = Vec::new();
        let hdr = |mode: u32, length: u32, cluster: u32, name: &str, created: &[u8; 8], modified: &[u8; 8], out: &mut Vec<u8>| {
            let mut h = vec![0u8; ENTRY_SIZE];
            h[0..4].copy_from_slice(&mode.to_le_bytes());
            h[4..8].copy_from_slice(&length.to_le_bytes());
            h[8..16].copy_from_slice(created);
            h[16..24].copy_from_slice(modified);
            h[24..28].copy_from_slice(&cluster.to_le_bytes());
            let nb = name.as_bytes();
            let n = nb.len().min(436);
            h[36..36 + n].copy_from_slice(&nb[..n]);
            out.extend_from_slice(&h);
        };

        // entrada raiz do save
        hdr(
            0x8427,
            (real.len() + 2) as u32,
            dir.cluster,
            &dir.name,
            &dir.created,
            &dir.modified,
            &mut out,
        );
        hdr(0x8427, 2, dir.cluster, ".", &dir.created, &dir.modified, &mut out);
        hdr(0x8427, 2, 0, "..", &dir.created, &dir.modified, &mut out);
        for f in &real {
            let is_file = f.mode & 0x0025 != 0 && f.mode & 0x0002 == 0;
            hdr(
                if is_file { 0x8425 } else { 0x8427 },
                f.length,
                f.cluster,
                &f.name,
                &f.created,
                &f.modified,
                &mut out,
            );
            if is_file {
                let data = self.read_chain(f.cluster)?;
                let n = (f.length as usize).min(data.len());
                out.extend_from_slice(&data[..n]);
                // alinha stream a 1024 bytes
                let pad = (1024 - (out.len() % 1024)) % 1024;
                out.extend(std::iter::repeat(0u8).take(pad));
            }
        }
        std::fs::write(out_path, &out).map_err(Ps2Error::Io)?;
        Ok(out.len() as u64)
    }
}
