//! Parser ISO9660 sem montagem (mountless).
//!
//! Lê o PVD (Primary Volume Descriptor) no setor 16, caminha pela árvore de
//! diretórios e extrai o Game ID do `SYSTEM.CNF` — tudo via leitura bruta de
//! setores, sem depender do SO. Réplica fiel dos offsets da ECMA-119:
//!
//! ```text
//! PVD @ setor 16:
//!   +0    tipo (1 = PVD)          +1    "CD001"
//!   +40   Volume Identifier (32B) +100  Path Table Size (LE/BE)
//!   +108  L Path Table (LE)       +116  M Path Table (BE)
//!   +124  Root Directory Record (34B)
//!
//! Directory Record:
//!   +0   len  +2  extent LE  +6  extent BE  +10 size LE  +14 size BE
//!   +25  flags (0x02 dir, 0x80 multi-extent)  +32 name_len  +33 name
//! ```

use crate::error::{Ps2Error, Result};
use crate::iso::zso::ZsoSource;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const SECTOR: u64 = 2048;
pub const PVD_LBA: u64 = 16;

/// Fonte de setores abstrata: permite parsear ISO crua (File) ou ZSO
/// (blocos LZ4 decomprimidos on-demand) com o mesmo código.
pub trait SectorSource {
    fn read_at(&mut self, lba: u64, buf: &mut [u8]) -> std::io::Result<()>;
}

pub struct FileSource {
    file: File,
}

impl FileSource {
    pub fn open(p: &Path) -> std::io::Result<Self> {
        Ok(Self { file: File::open(p)? })
    }
}

impl SectorSource for FileSource {
    fn read_at(&mut self, lba: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(lba * SECTOR))?;
        self.file.read_exact(buf)
    }
}

#[inline]
pub fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
pub fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[derive(Debug, Clone)]
pub struct DirRecord {
    pub name: String,
    pub lba: u32,
    pub size: u32,
    pub is_dir: bool,
    pub multi_extent: bool,
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub lba: u32,
    pub size: u32,
    pub is_dir: bool,
    pub multi_extent: bool,
    pub children: Vec<TreeNode>,
}

pub struct IsoInfo {
    pub volume_id: String,
    pub root: TreeNode,
    pub l_path_table: (u32, u32), // (lba, size)
    pub m_path_table: (u32, u32), // (lba, size)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Cd,
    Dvd,
}

impl Media {
    pub fn as_str(self) -> &'static str {
        match self {
            Media::Cd => "CD",
            Media::Dvd => "DVD",
        }
    }
    pub fn ul_byte(self) -> u8 {
        match self {
            Media::Cd => crate::opl::ulcfg::MEDIA_CD,
            Media::Dvd => crate::opl::ulcfg::MEDIA_DVD,
        }
    }
}

fn parse_record(buf: &[u8], off: usize) -> Option<DirRecord> {
    let len = *buf.get(off)? as usize;
    if len < 34 || off + len > buf.len() {
        return None;
    }
    let lba = le32(buf, off + 2);
    let size = le32(buf, off + 10);
    let flags = buf[off + 25];
    let name_len = buf[off + 32] as usize;
    if name_len > len - 33 {
        return None;
    }
    let raw = &buf[off + 33..off + 33 + name_len];
    let name = match raw {
        [0] => ".".to_string(),
        [1] => "..".to_string(),
        r => {
            let s = String::from_utf8_lossy(r).to_string();
            if flags & 0x02 == 0 {
                // arquivos carregam sufixo de versão ";1"
                s.strip_suffix(";1").unwrap_or(&s).to_string()
            } else {
                s
            }
        }
    };
    Some(DirRecord {
        name,
        lba,
        size,
        is_dir: flags & 0x02 != 0,
        multi_extent: flags & 0x80 != 0,
    })
}

fn read_dir_extent(src: &mut dyn SectorSource, lba: u32, size: u32) -> Result<Vec<u8>> {
    if size as u64 > 16 * 1024 * 1024 {
        return Err(Ps2Error::InvalidIso(format!(
            "diretório no LBA {lba} grande demais ({size} bytes)"
        )));
    }
    let nsec = (size as u64).div_ceil(SECTOR);
    let mut buf = vec![0u8; (nsec * SECTOR) as usize];
    src.read_at(lba as u64, &mut buf)
        .map_err(|e| Ps2Error::InvalidIso(format!("falha ao ler diretório LBA {lba}: {e}")))?;
    Ok(buf)
}

fn walk_dir(
    src: &mut dyn SectorSource,
    lba: u32,
    size: u32,
    depth: u8,
) -> Result<Vec<TreeNode>> {
    if depth > 8 {
        return Err(Ps2Error::InvalidIso(
            "profundidade de diretórios > 8 (ISO suspeita)".into(),
        ));
    }
    if lba == 0 || size == 0 {
        return Ok(Vec::new());
    }
    let data = read_dir_extent(src, lba, size)?;
    let mut out = Vec::new();
    let mut pos: usize = 0;
    while pos < data.len() {
        let sec = pos / SECTOR as usize;
        let off = pos % SECTOR as usize;
        let rec_len = data[pos] as usize;
        if rec_len == 0 {
            // padding de fim de setor: salta para o próximo
            pos = (sec + 1) * SECTOR as usize;
            continue;
        }
        if let Some(rec) = parse_record(&data, off) {
            if rec.name != "." && rec.name != ".." {
                let children = if rec.is_dir {
                    walk_dir(src, rec.lba, rec.size, depth + 1)?
                } else {
                    Vec::new()
                };
                out.push(TreeNode {
                    name: rec.name,
                    lba: rec.lba,
                    size: rec.size,
                    is_dir: rec.is_dir,
                    multi_extent: rec.multi_extent,
                    children,
                });
            }
        }
        pos += rec_len;
    }
    Ok(out)
}

pub fn parse_image(src: &mut dyn SectorSource) -> Result<IsoInfo> {
    let mut pvd = vec![0u8; SECTOR as usize];
    src.read_at(PVD_LBA, &mut pvd)
        .map_err(|e| Ps2Error::InvalidIso(format!("falha ao ler PVD (setor 16): {e}")))?;
    if pvd[0] != 1 || &pvd[1..6] != b"CD001" {
        return Err(Ps2Error::InvalidIso(
            "PVD ausente no setor 16 — não é uma ISO9660 válida".into(),
        ));
    }
    let volume_id = String::from_utf8_lossy(&pvd[40..72])
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let pt_size = le32(&pvd, 100);
    let l_lba = le32(&pvd, 108);
    let m_lba = be32(&pvd, 116);
    let root_rec = parse_record(&pvd, 124)
        .ok_or_else(|| Ps2Error::InvalidIso("root directory record inválido no PVD".into()))?;
    let children = walk_dir(src, root_rec.lba, root_rec.size, 0)?;
    Ok(IsoInfo {
        volume_id,
        root: TreeNode {
            name: root_rec.name,
            lba: root_rec.lba,
            size: root_rec.size,
            is_dir: true,
            multi_extent: false,
            children,
        },
        l_path_table: (l_lba, pt_size),
        m_path_table: (m_lba, pt_size),
    })
}

pub fn find_entry<'a>(node: &'a TreeNode, name: &str) -> Option<&'a TreeNode> {
    for c in &node.children {
        if !c.is_dir && c.name.eq_ignore_ascii_case(name) {
            return Some(c);
        }
        if let Some(f) = find_entry(c, name) {
            return Some(f);
        }
    }
    None
}

pub fn read_system_cnf(src: &mut dyn SectorSource, info: &IsoInfo) -> Option<String> {
    let node = find_entry(&info.root, "SYSTEM.CNF")?;
    let mut buf = vec![0u8; node.size.min(4096) as usize];
    src.read_at(node.lba as u64, &mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).to_string())
}

/// Valida `XXXX_YYY.ZZ` (ex.: `SLUS_214.44`, `SLES_541.04`, `SLPM_654.05`).
pub fn looks_like_game_id(s: &str) -> bool {
    let b: Vec<char> = s.chars().collect();
    b.len() == 11
        && b[4] == '_'
        && b[8] == '.'
        && b[..4].iter().all(|c| c.is_ascii_alphabetic())
        && b[5..8].iter().all(|c| c.is_ascii_digit())
        && b[9..11].iter().all(|c| c.is_ascii_digit())
}

/// Extrai o Game ID do conteúdo do SYSTEM.CNF:
/// linha `BOOT2 = cdrom0:\SLUS_214.44;1` → `SLUS_214.44`.
pub fn extract_game_id(cnf: &str) -> Option<String> {
    for line in cnf.lines() {
        let up = line.to_ascii_uppercase();
        if up.trim_start().starts_with("BOOT2") {
            if let Some(p) = up.find("CDROM0:") {
                let after = &line[p + 7..];
                let after = after.trim_start_matches(['\\', '/']);
                let token: String = after
                    .chars()
                    .take_while(|c| !matches!(c, ';' | ' ' | '\t' | '\r' | '\n'))
                    .collect();
                let token = token.trim().to_ascii_uppercase();
                if looks_like_game_id(&token) {
                    return Some(token);
                }
            }
        }
    }
    // fallback: procura qualquer token com o padrão de disc ID
    for tok in cnf.split(|c: char| {
        c.is_whitespace() || matches!(c, ';' | '\\' | '/' | '=' | ',' | '"')
    }) {
        let t = tok.trim().to_ascii_uppercase();
        if looks_like_game_id(&t) {
            return Some(t);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct GameInfo {
    pub game_id: String,
    pub volume_id: String,
    pub media: Media,
    pub size: u64,
    pub total_sectors: u64,
}

pub fn analyze(src: &mut dyn SectorSource, size: u64) -> Result<GameInfo> {
    let info = parse_image(src)?;
    let cnf = read_system_cnf(src, &info).ok_or_else(|| {
        Ps2Error::GameIdNotFound("SYSTEM.CNF ausente na raiz da ISO".into())
    })?;
    let game_id = extract_game_id(&cnf)
        .ok_or_else(|| Ps2Error::GameIdNotFound("SYSTEM.CNF sem BOOT2 válido".into()))?;
    // Heurística de mídia: CDs ≤ ~700–800 MB; DVDs > 750 MB.
    let media = if size > 750 * 1024 * 1024 { Media::Dvd } else { Media::Cd };
    Ok(GameInfo {
        game_id,
        volume_id: info.volume_id,
        media,
        size,
        total_sectors: size.div_ceil(SECTOR),
    })
}

/// Detecta formato pela extensão/magic e analisa (ISO crua ou ZSO).
pub fn analyze_path(path: &Path) -> Result<GameInfo> {
    let size = std::fs::metadata(path)
        .map_err(Ps2Error::Io)?
        .len();
    let mut head = [0u8; 4];
    let mut probe = File::open(path).map_err(Ps2Error::Io)?;
    probe.read_exact(&mut head).map_err(Ps2Error::Io)?;
    if u32::from_le_bytes(head) == crate::iso::zso::ZSO_MAGIC {
        let mut src = ZsoSource::open(path)?;
        // relata o tamanho DESCOMPRIMIDO (metadado do header ZSO)
        let total = src.header().total_bytes;
        analyze(&mut src, total)
    } else {
        let mut src =
            FileSource::open(path).map_err(|e| Ps2Error::InvalidIso(format!("{e}")))?;
        analyze(&mut src, size)
    }
}

/// Abre a fonte de setores correta para o arquivo (ISO ou ZSO).
#[allow(dead_code)]
pub fn open_source(path: &Path) -> Result<Box<dyn SectorSource>> {
    let mut head = [0u8; 4];
    let mut probe = File::open(path).map_err(Ps2Error::Io)?;
    probe.read_exact(&mut head).map_err(Ps2Error::Io)?;
    if u32::from_le_bytes(head) == crate::iso::zso::ZSO_MAGIC {
        Ok(Box::new(ZsoSource::open(path)?))
    } else {
        Ok(Box::new(FileSource::open(path).map_err(Ps2Error::Io)?))
    }
}
