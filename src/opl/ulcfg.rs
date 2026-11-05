//! ul.cfg — formato binário USBExtreme/USBUtil, 100% compatível com o OPL.
//!
//! Fonte da verdade: `include/supportbase.h:28-38` e `src/supportbase.c`
//! (Open-PS2-Loader). Registro de **64 bytes**:
//!
//! ```text
//! +0   name[32]     nome de exibição (não precisa terminar em NUL)
//! +32  magic[3]     "ul."  ← o magic fica NO MEIO do registro!
//! +35  startup[12]  disc ID (ex.: SLUS_200.02)
//! +47  parts        quantidade de partes
//! +48  media        0x12 = CD, 0x14 = DVD
//! +49  unknown[4]   zeros
//! +53  Byte08       sempre 0x08 (compat. USBAdvance)
//! +54  unknown2[10] zeros
//! ```
//!
//! Não existe campo de tamanho nem CRC no registro: o tamanho vem dos
//! arquivos de parte e o CRC32 vai no NOME dos arquivos:
//! `ul.<CRC32>.<ID>.<parte:02x>` — calculado por `USBA_crc32`
//! (`src/system.c:299-325`), que tem 3 quirks replicados aqui byte a byte:
//!   1. tabela MSB-first com poly 0x04C11DB7, gravada INVERTIDA (crctab[255-t]);
//!   2. quando o MSB está setado, NÃO faz XOR (o oposto do CRC-32 padrão);
//!   3. inclui o byte NUL no cálculo e lê até 33 bytes (32 + 1).
//! Em C, a leitura avança além do campo name[32] e cai no magic "ul." —
//! simulado aqui anexando "ul." ao buffer de 32 bytes.

use crate::error::Result;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;

pub const RECORD_SIZE: usize = 64;
pub const MEDIA_CD: u8 = 0x12;
pub const MEDIA_DVD: u8 = 0x14;
pub const NAME_MAX: usize = 32;
pub const STARTUP_MAX: usize = 12;

#[derive(Debug, Clone)]
pub struct UlRecord {
    pub name: String,
    pub startup: String,
    pub parts: u8,
    pub media: u8,
}

impl UlRecord {
    pub fn to_bytes(&self) -> [u8; RECORD_SIZE] {
        let mut b = [0u8; RECORD_SIZE];
        let name = self.name.as_bytes();
        let n = name.len().min(NAME_MAX);
        b[..n].copy_from_slice(&name[..n]);
        b[32..35].copy_from_slice(b"ul.");
        let st = self.startup.as_bytes();
        let m = st.len().min(STARTUP_MAX);
        b[35..35 + m].copy_from_slice(&st[..m]);
        b[47] = self.parts;
        b[48] = self.media;
        b[53] = 0x08; // Byte08 — suportebase.c:700 ("just to be compatible")
        b
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < RECORD_SIZE || &b[32..35] != b"ul." {
            return None;
        }
        let name = clean_str(&b[0..32]);
        let startup = clean_str(&b[35..47]);
        Some(Self {
            name,
            startup,
            parts: b[47],
            media: b[48],
        })
    }
}

fn clean_str(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// Campo name de 32 bytes (NUL-padded) pronto para CRC e gravação.
pub fn name_field(name: &str) -> [u8; NAME_MAX] {
    let mut f = [0u8; NAME_MAX];
    let b = name.as_bytes();
    let n = b.len().min(NAME_MAX);
    f[..n].copy_from_slice(&b[..n]);
    f
}

/// Réplica EXATA de `USBA_crc32` (OPL src/system.c:299-325).
pub fn usba_crc32(name_field: &[u8]) -> u32 {
    let mut crctab = [0u32; 256];
    for table in 0..256usize {
        let mut crc = ((table as u32) << 24) as i32;
        for _ in 0..8 {
            crc = if crc < 0 {
                crc.wrapping_shl(1)
            } else {
                crc.wrapping_shl(1) ^ 0x04C11DB7u32 as i32
            };
        }
        crctab[255 - table] = crc as u32;
    }
    let mut buf = [0u8; 64];
    let n = name_field.len().min(NAME_MAX);
    buf[..n].copy_from_slice(&name_field[..n]);
    buf[32..35].copy_from_slice(b"ul."); // leitura avança no registro em C

    let mut crc: i32 = 0;
    let mut count: usize = 0;
    loop {
        let byte = buf[count] as i32;
        count += 1;
        let idx = ((byte ^ ((crc >> 24) & 0xFF)) & 0xFF) as usize;
        crc = crctab[idx] as i32 ^ (crc.wrapping_shl(8) & 0xFFFFFF00u32 as i32);
        if buf[count - 1] == 0 || count > 32 {
            break;
        }
    }
    crc as u32
}

/// Nome do arquivo de parte: `ul.%08X.%s.%02x` (supportbase.c:726).
pub fn part_filename(crc: u32, startup: &str, part: u8) -> String {
    format!("ul.{crc:08X}.{startup}.{part:02x}")
}

/// Lê o ul.cfg do dispositivo (raiz do cartão/partição).
pub fn read_ulcfg(device_root: &Path) -> Result<Vec<UlRecord>> {
    let path = device_root.join("ul.cfg");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut data = Vec::new();
    std::fs::File::open(&path)
        .map_err(crate::error::Ps2Error::Io)?
        .read_to_end(&mut data)
        .map_err(crate::error::Ps2Error::Io)?;
    Ok(data
        .chunks_exact(RECORD_SIZE)
        .filter_map(UlRecord::from_bytes)
        .collect())
}

/// Grava (substitui) o ul.cfg do dispositivo.
pub fn write_ulcfg(device_root: &Path, records: &[UlRecord]) -> Result<()> {
    let path = device_root.join("ul.cfg");
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .map_err(crate::error::Ps2Error::Io)?;
    for r in records {
        f.write_all(&r.to_bytes())
            .map_err(crate::error::Ps2Error::Io)?;
    }
    Ok(())
}

/// Constrói ul.cfg + partes `ul.*` a partir das ISOs de CD/ e DVD/
/// (formato USBExtreme — ideal para FAT32). Partes sequenciais evitam
/// fragmentação no barramento USB/SIO2.
pub fn build_from_library(
    device_root: &Path,
    part_size_mb: u64,
    progress: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
) -> Result<Vec<String>> {
    use std::io::Read;

    let mut records = read_ulcfg(device_root)?;
    let mut log = Vec::new();
    let existing: Vec<String> = records.iter().map(|r| r.name.clone()).collect();
    let part_size = (part_size_mb.max(1)) * 1024 * 1024;

    for sub in ["CD", "DVD"] {
        let dir = device_root.join(sub);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut isos: Vec<std::path::PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("iso"))
                    .unwrap_or(false)
            })
            .collect();
        isos.sort();
        for iso in isos {
            let Ok(info) = crate::iso::parser::analyze_path(&iso) else {
                log.push(format!("⚠ sem Game ID: {}", iso.display()));
                continue;
            };
            let title = crate::opl::library::sanitize_title(&info.volume_id);
            let name: String = if title.is_empty() {
                "Unknown".to_string()
            } else {
                title.chars().take(NAME_MAX).collect()
            };
            if existing.contains(&name) {
                log.push(format!("• pulado (nome duplicado): {name}"));
                continue;
            }
            let field = name_field(&name);
            let crc = usba_crc32(&field);
            let mut reader =
                std::io::BufReader::with_capacity(1024 * 1024, std::fs::File::open(&iso).map_err(crate::error::Ps2Error::Io)?);
            let mut buf = vec![0u8; part_size as usize];
            let mut parts: u8 = 0;
            loop {
                let mut filled = 0usize;
                while filled < buf.len() {
                    let n = reader
                        .read(&mut buf[filled..])
                        .map_err(crate::error::Ps2Error::Io)?;
                    if n == 0 {
                        break;
                    }
                    filled += n;
                    if let Some(p) = &progress {
                        use std::sync::atomic::Ordering;
                        p.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
                if filled == 0 {
                    break;
                }
                let fname = part_filename(crc, &info.game_id, parts);
                let mut w =
                    std::fs::File::create(dir.join(&fname)).map_err(crate::error::Ps2Error::Io)?;
                w.write_all(&buf[..filled])
                    .map_err(crate::error::Ps2Error::Io)?;
                parts = parts.wrapping_add(1);
                if parts == 0 {
                    return Err(crate::error::Ps2Error::Other(format!(
                        "{name}: mais de 255 partes"
                    )));
                }
                if filled < buf.len() {
                    break;
                }
            }
            records.push(UlRecord {
                name: name.clone(),
                startup: info.game_id.clone(),
                parts,
                media: info.media.ul_byte(),
            });
            log.push(format!("✓ UL: {name} → {parts} parte(s), CRC {crc:08X}"));
        }
    }
    write_ulcfg(device_root, &records)?;
    Ok(log)
}
