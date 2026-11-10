//! VMC Manager — leitura E escrita de cartões de memória virtuais `.bin` do OPL.
//!
//! Layout validado contra três fontes autoritativas (regra do projeto: nunca
//! de memória):
//! 1. ps2sdk `common/include/libmc-common.h` — `McFsEntry` (512 B), a entrada
//!    de diretório on-card:
//!    ```text
//!    mode u16@0  unused u16@2  length u32@4  created[8]@8  cluster u32@16
//!    dir_entry u32@20  modified[8]@24  attr u32@32  unused2[7]@36
//!    name[32]@64  unused3[416]@96 → 512 total
//!    ```
//! 2. OPL `modules/vmc/genvmc/genvmc.c` — superblock `MCDevInfo` (magic +0/28,
//!    pagesize +40, clusters_per_card +48, alloc_offset +52, rootdir_cluster
//!    +60, cardtype +336 = 2, cluster_size +340, FATentries_per_cluster +344,
//!    max_allocatable_clusters +368), FAT com **livre = 0x7FFFFFFF** e
//!    **EOF = 0xFFFFFFFF**, root dir em `rootdir_cluster = 0` com entradas
//!    "." e ".." de `length = 2`.
//! 3. uLE_ISR `vmc_fs/vmc.h` + `vmc_misc.c` + `filer.c` — attrs `DF_*`
//!    (EXISTS 0x8000, HIDDEN 0x2000, 0400 0x0400, DIRECTORY 0x0020, FILE
//!    0x0010, R/W/X = 0x0007), pseudo-entradas de diretório (".": cluster =
//!    próprio, dir_entry = slot no pai; "..": cluster = 0) e o formato
//!    **PSU** (`psu_header` == `mcTable`, 512 B, dados alinhados a 1024 B,
//!    header raiz com size = nº de headers seguintes).
//!
//! A FAT vive em clusters indiretos: `ifc_list` no superblock (+408, u32[32])
//! aponta clusters cujo conteúdo é u32 = índice ABSOLUTO de um cluster da
//! FAT. Entradas da FAT e de diretório são relativas a `alloc_offset`.
//!
//! Escrita (v0.3): alocação de clusters, extensão de cadeias de diretório,
//! `import_psu` (restauro) e `export_psu` (backup) byte-compatível com o
//! writer/reader do uLE — pseudo-entradas com size 0, dados de arquivo
//! alinhados a 1024 B, attrs padrão (arquivo 0x8497, diretório 0x8427).
//! Import é all-or-nothing: qualquer erro devolve os clusters alocados
//! (rollback em memória) e a FAT em disco não é alterada (`flush_fat` só
//! roda no fim, com tudo íntegro).

use crate::error::{Ps2Error, Result};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 28] = b"Sony PS2 Memory Card Format ";
const ENTRY_SIZE: usize = 512;
const NAME_OFF: usize = 64;

// ── attrs (uLE vmc_fs/vmc.h) ────────────────────────────────────────────────
const DF_EXISTS: u16 = 0x8000;
const DF_HIDDEN: u16 = 0x2000;
const DF_0400: u16 = 0x0400;
const DF_DIRECTORY: u16 = 0x0020;
const DF_FILE: u16 = 0x0010;
const DF_CLOSED: u16 = 0x0080;
const RWX: u16 = 0x0004 | 0x0002 | 0x0001;
/// modo de arquivo padrão (uLE filer.c: "8497 for normal file")
const MODE_FILE: u16 = DF_EXISTS | DF_0400 | DF_CLOSED | DF_FILE | RWX;
/// modo de diretório padrão ("8427 = normal folder")
const MODE_DIR: u16 = DF_EXISTS | DF_0400 | DF_DIRECTORY | RWX;
/// modo ".." (genvmc: Exists | Hidden | 0400 | Subdir | Writeable | Executable)
const MODE_DOTDOT: u16 = DF_EXISTS | DF_HIDDEN | DF_0400 | DF_DIRECTORY | RWX;

// ── FAT (genvmc.c / uLE vmc.h) ──────────────────────────────────────────────
const FAT_FREE: u32 = 0x7FFF_FFFF;
const FAT_EOF: u32 = 0xFFFF_FFFF;
/// fim de cadeia aceitável além do EOF canônico (marcadores mcman)
const FAT_END_ALT: u32 = 0xFFFF_FFFD;

#[derive(Debug, Clone)]
#[allow(dead_code)] // pagesize/pages_per_cluster: geometria exposta para API/diag
pub struct VmcInfo {
    pub pagesize: u16,
    pub pages_per_cluster: u16,
    pub clusters_per_card: u32,
    pub alloc_offset: u32,
    pub rootdir_cluster: u32,
    pub cluster_size: u32,
    pub max_allocatable_clusters: u32,
}

/// Entrada de diretório (`McFsEntry`) decodificada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmcEntry {
    pub name: String,
    pub mode: u32,
    pub length: u32,
    pub cluster: u32,
    pub created: [u8; 8],
    pub modified: [u8; 8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmcSave {
    pub name: String,
    pub size_bytes: u64,
    pub files: usize,
    pub cluster: u32,
}

/// Resultado de um import PSU.
#[derive(Debug, Clone)]
pub struct ImportReport {
    pub name: String,
    pub files: usize,
    pub bytes: u64,
    pub clusters_used: u32,
}

pub struct Vmc {
    file: std::fs::File,
    pub info: VmcInfo,
    /// índices ABSOLUTOS dos clusters da FAT (via ifc_list do superblock)
    ifc_ptrs: Vec<u32>,
    /// FAT em memória — índice = cluster relativo a alloc_offset
    fat: Vec<u32>,
}

fn clean_str(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

fn is_dir(mode: u32) -> bool {
    mode as u16 & DF_DIRECTORY != 0
}

fn is_file(mode: u32) -> bool {
    mode as u16 & DF_FILE != 0 && mode as u16 & DF_DIRECTORY == 0
}

/// Decodifica uma `McFsEntry` de 512 B. `None` se a entrada estiver livre
/// (bit EXISTS limpo).
fn parse_entry(buf: &[u8]) -> Option<VmcEntry> {
    if buf.len() < ENTRY_SIZE {
        return None;
    }
    let mode = u16::from_le_bytes([buf[0], buf[1]]);
    if mode & DF_EXISTS == 0 {
        return None;
    }
    Some(VmcEntry {
        name: clean_str(&buf[NAME_OFF..NAME_OFF + 32]),
        mode: mode as u32,
        length: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        cluster: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
        created: buf[8..16].try_into().unwrap(),
        modified: buf[24..32].try_into().unwrap(),
    })
}

/// Serializa uma `McFsEntry` de 512 B (campos desconhecidos = 0, como o
/// genvmc faz com `memset(0)`). `dir_entry` = posição da entrada no pai.
fn entry_bytes(e: &VmcEntry, dir_entry: u32) -> Vec<u8> {
    let mut b = vec![0u8; ENTRY_SIZE];
    b[0..2].copy_from_slice(&(e.mode as u16).to_le_bytes());
    b[4..8].copy_from_slice(&e.length.to_le_bytes());
    b[8..16].copy_from_slice(&e.created);
    b[16..20].copy_from_slice(&e.cluster.to_le_bytes());
    b[20..24].copy_from_slice(&dir_entry.to_le_bytes());
    b[24..32].copy_from_slice(&e.modified);
    let nb = e.name.as_bytes();
    let n = nb.len().min(32);
    b[NAME_OFF..NAME_OFF + n].copy_from_slice(&nb[..n]);
    b
}

/// Cabeçalho PSU (== `mcTable` — uLE filer.c `psu_header`, 512 B):
/// attr u16@0, size u32@4, cTime[8]@8, mTime[8]@24, name[32]@64.
#[derive(Debug, Clone)]
struct PsuHeader {
    attr: u16,
    size: u32,
    created: [u8; 8],
    modified: [u8; 8],
    name: String,
}

fn parse_psu_header(buf: &[u8]) -> Result<PsuHeader> {
    if buf.len() < ENTRY_SIZE {
        return Err(Ps2Error::InvalidPsu("header truncado (< 512 B)".into()));
    }
    Ok(PsuHeader {
        attr: u16::from_le_bytes([buf[0], buf[1]]),
        size: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        created: buf[8..16].try_into().unwrap(),
        modified: buf[24..32].try_into().unwrap(),
        name: clean_str(&buf[NAME_OFF..NAME_OFF + 32]),
    })
}

/// Serializa um cabeçalho PSU (campos `unknown` zerados — os leitores do
/// uLE/OPL os repassam ao cartão sem validar conteúdo).
fn psu_header_bytes(
    attr: u16,
    size: u32,
    created: &[u8; 8],
    modified: &[u8; 8],
    name: &[u8],
) -> Vec<u8> {
    let mut b = vec![0u8; ENTRY_SIZE];
    b[0..2].copy_from_slice(&attr.to_le_bytes());
    b[4..8].copy_from_slice(&size.to_le_bytes());
    b[8..16].copy_from_slice(created);
    b[24..32].copy_from_slice(modified);
    let n = name.len().min(32);
    b[NAME_OFF..NAME_OFF + n].copy_from_slice(&name[..n]);
    b
}

/// Preenchimento de dados de arquivo no PSU (uLE filer.c, restore:
/// `psu_pad_size = 0x400 - (size & 0x3FF)` quando não alinhado).
fn psu_data_pad(size: u32) -> usize {
    if size & 0x3FF != 0 {
        (0x400 - (size & 0x3FF)) as usize
    } else {
        0
    }
}

/// Lê um cluster ABSOLUTO (índice de cartão — usado pela FAT/ifc).
fn read_abs_cluster(file: &mut std::fs::File, abs: u32, cluster_size: u32) -> Result<Vec<u8>> {
    let off = abs as u64 * cluster_size as u64;
    file.seek(SeekFrom::Start(off)).map_err(Ps2Error::Io)?;
    let mut buf = vec![0u8; cluster_size as usize];
    file.read_exact(&mut buf).map_err(Ps2Error::Io)?;
    Ok(buf)
}

/// Grava um cluster ABSOLUTO (zero-padded até cluster_size).
fn write_abs_cluster(file: &mut std::fs::File, abs: u32, data: &[u8], cluster_size: u32) -> Result<()> {
    let off = abs as u64 * cluster_size as u64;
    file.seek(SeekFrom::Start(off)).map_err(Ps2Error::Io)?;
    let mut buf = vec![0u8; cluster_size as usize];
    let n = data.len().min(buf.len());
    buf[..n].copy_from_slice(&data[..n]);
    file.write_all(&buf).map_err(Ps2Error::Io)
}

impl Vmc {
    pub fn open(path: &Path) -> Result<Self> {
        // leitura + escrita: import/overwrite gravam clusters e FAT no cartão
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(Ps2Error::Io)?;
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
        // ifc_list em +408 (genvmc MCDevInfo: unused2[8] termina em +400)
        let mut ifc_ptrs = Vec::new();
        for i in 0..32 {
            let p = u32::from_le_bytes(sb[(408 + i * 4)..(408 + i * 4 + 4)].try_into().unwrap());
            if p == 0 {
                break;
            }
            ifc_ptrs.push(p);
        }
        if ifc_ptrs.is_empty() {
            return Err(Ps2Error::InvalidVmc("ifc_list vazio no superblock".into()));
        }
        // FAT: cada cluster do ifc contém u32 = índice absoluto de um cluster
        // da FAT; cada cluster da FAT contém cluster_size/4 entradas u32.
        let wanted = (clusters_per_card as usize).max(max_alloc as usize);
        let mut fat = Vec::with_capacity(wanted);
        'outer: for ifc_ptr in &ifc_ptrs {
            let ifc = read_abs_cluster(&mut file, *ifc_ptr, cluster_size)?;
            for c in ifc.as_chunks::<4>().0 {
                let fc = u32::from_le_bytes(*c);
                if fc == 0 {
                    break 'outer;
                }
                let data = read_abs_cluster(&mut file, fc, cluster_size)?;
                for c in data.as_chunks::<4>().0 {
                    fat.push(u32::from_le_bytes(*c));
                }
                if fat.len() >= wanted {
                    break 'outer;
                }
            }
        }
        if fat.len() < max_alloc as usize {
            return Err(Ps2Error::InvalidVmc(format!(
                "FAT incompleta: {} entradas lidas, superblock pede {}",
                fat.len(),
                max_alloc
            )));
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
            ifc_ptrs,
            fat,
        })
    }

    // ── clusters ────────────────────────────────────────────────────────────
    fn read_cluster(&mut self, rel: u32) -> Result<Vec<u8>> {
        read_abs_cluster(
            &mut self.file,
            self.info.alloc_offset + rel,
            self.info.cluster_size,
        )
    }

    fn write_cluster(&mut self, rel: u32, data: &[u8]) -> Result<()> {
        write_abs_cluster(
            &mut self.file,
            self.info.alloc_offset + rel,
            data,
            self.info.cluster_size,
        )
    }

    fn read_chain(&mut self, first: u32) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut c = first;
        let mut hops = 0usize;
        loop {
            out.extend_from_slice(&self.read_cluster(c)?);
            let next = *self.fat.get(c as usize).unwrap_or(&FAT_EOF);
            if next == FAT_EOF || next == FAT_END_ALT || next == 0 {
                break;
            }
            if next == FAT_FREE || next as usize >= self.fat.len() {
                return Err(Ps2Error::InvalidVmc(format!(
                    "cadeia de clusters corrompida em {c} → {next:#x}"
                )));
            }
            c = next;
            hops += 1;
            if hops > 200_000 {
                return Err(Ps2Error::InvalidVmc("cadeia de clusters cíclica".into()));
            }
        }
        Ok(out)
    }

    /// Clusters livres (FAT_FREE) na área alocável.
    pub fn free_clusters(&self) -> u32 {
        self.fat[..self.info.max_allocatable_clusters as usize]
            .iter()
            .filter(|&&e| e == FAT_FREE)
            .count() as u32
    }

    /// Aloca `n` clusters encadeados (último = EOF) — só na FAT em memória.
    fn alloc_chain(&mut self, n: u32) -> Result<Vec<u32>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut chain = Vec::with_capacity(n as usize);
        for i in 0..self.info.max_allocatable_clusters as usize {
            if self.fat[i] == FAT_FREE {
                chain.push(i as u32);
                if chain.len() == n as usize {
                    break;
                }
            }
        }
        if chain.len() < n as usize {
            return Err(Ps2Error::InvalidVmc(format!(
                "espaço insuficiente na VMC: faltam {} cluster(s)",
                n as usize - chain.len()
            )));
        }
        for w in 0..chain.len() {
            let next = if w + 1 < chain.len() {
                chain[w + 1]
            } else {
                FAT_EOF
            };
            self.fat[chain[w] as usize] = next;
        }
        Ok(chain)
    }

    /// Libera uma cadeia (rollback em memória ou remoção).
    fn free_chain(&mut self, first: u32) {
        let mut c = first;
        let mut hops = 0usize;
        loop {
            let next = *self.fat.get(c as usize).unwrap_or(&FAT_EOF);
            self.fat[c as usize] = FAT_FREE;
            if next == FAT_EOF || next == FAT_END_ALT || next == 0 || next == FAT_FREE {
                break;
            }
            if next as usize >= self.fat.len() || hops > 200_000 {
                break;
            }
            c = next;
            hops += 1;
        }
    }

    /// n-ésimo cluster (relativo) de uma cadeia.
    fn chain_nth_rel(&self, first: u32, n: usize) -> Result<u32> {
        let mut c = first;
        for i in 0..=n {
            if i == n {
                return Ok(c);
            }
            let next = *self.fat.get(c as usize).unwrap_or(&FAT_EOF);
            if next == FAT_EOF || next == FAT_END_ALT || next == 0 || next as usize >= self.fat.len() {
                return Err(Ps2Error::InvalidVmc("cadeia mais curta que o esperado".into()));
            }
            c = next;
        }
        Ok(c)
    }

    /// Último cluster (relativo) de uma cadeia.
    fn chain_last_rel(&self, first: u32) -> Result<u32> {
        let mut c = first;
        let mut hops = 0usize;
        loop {
            let next = *self.fat.get(c as usize).unwrap_or(&FAT_EOF);
            if next == FAT_EOF || next == FAT_END_ALT || next == 0 {
                return Ok(c);
            }
            if next as usize >= self.fat.len() || hops > 200_000 {
                return Err(Ps2Error::InvalidVmc("cadeia corrompida/cíclica".into()));
            }
            c = next;
            hops += 1;
        }
    }

    /// Persiste a FAT em disco (clusters apontados pelo ifc_list) + sync.
    fn flush_fat(&mut self) -> Result<()> {
        let per = (self.info.cluster_size / 4) as usize;
        for (i, ptr) in self.ifc_ptrs.iter().enumerate() {
            let start = i * per;
            if start >= self.fat.len() {
                break;
            }
            let end = (start + per).min(self.fat.len());
            let mut buf = vec![0u8; self.info.cluster_size as usize];
            for (j, e) in self.fat[start..end].iter().enumerate() {
                buf[j * 4..j * 4 + 4].copy_from_slice(&e.to_le_bytes());
            }
            write_abs_cluster(&mut self.file, *ptr, &buf, self.info.cluster_size)?;
        }
        self.file.sync_all().map_err(Ps2Error::Io)?;
        Ok(())
    }

    // ── diretórios ──────────────────────────────────────────────────────────
    /// Entradas vivas de um diretório (scaneia a cadeia inteira).
    fn read_dir_entries(&mut self, cluster: u32) -> Result<Vec<VmcEntry>> {
        let data = self.read_chain(cluster)?;
        let mut out = Vec::new();
        for chunk in data.chunks(ENTRY_SIZE) {
            if let Some(e) = parse_entry(chunk) {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// Busca exata por nome; retorna (slot global na cadeia, entrada).
    fn find_entry(&mut self, dir_cluster: u32, name: &str) -> Result<Option<(u32, VmcEntry)>> {
        for (slot, e) in self
            .read_dir_entries(dir_cluster)?
            .into_iter()
            .enumerate()
        {
            if e.name == name {
                return Ok(Some((slot as u32, e)));
            }
        }
        Ok(None)
    }

    /// Número de entradas vivas num diretório (contando "." e "..").
    fn dir_count(&mut self, dir_cluster: u32) -> Result<u32> {
        Ok(self.read_dir_entries(dir_cluster)?.len() as u32)
    }

    /// Grava uma entrada no primeiro slot morto do diretório (slot com bit
    /// EXISTS limpo — convenção mcman) ou estende a cadeia com um cluster
    /// novo zerado. Retorna o slot usado.
    fn append_entry(&mut self, dir_cluster: u32, entry: &VmcEntry) -> Result<u32> {
        let per = (self.info.cluster_size as usize / ENTRY_SIZE).max(1);
        let data = self.read_chain(dir_cluster)?;
        let total_slots = data.len() / ENTRY_SIZE;
        // 1) slot morto dentro da cadeia existente
        for (idx, chunk) in data.chunks(ENTRY_SIZE).enumerate() {
            if chunk[0..2] == [0u8, 0u8] {
                let rel = self.chain_nth_rel(dir_cluster, idx / per)?;
                let off = (idx % per) * ENTRY_SIZE;
                let mut page = self.read_cluster(rel)?;
                page[off..off + ENTRY_SIZE].copy_from_slice(&entry_bytes(entry, idx as u32));
                self.write_cluster(rel, &page)?;
                return Ok(idx as u32);
            }
        }
        // 2) estende a cadeia com um cluster zerado
        let newc = self.alloc_chain(1)?[0];
        self.write_cluster(newc, &[])?;
        let last = self.chain_last_rel(dir_cluster)?;
        self.fat[last as usize] = newc;
        self.fat[newc as usize] = FAT_EOF;
        let off = (total_slots % per) * ENTRY_SIZE;
        let mut page = self.read_cluster(newc)?;
        page[off..off + ENTRY_SIZE].copy_from_slice(&entry_bytes(entry, total_slots as u32));
        self.write_cluster(newc, &page)?;
        Ok(total_slots as u32)
    }

    /// Sobrescreve o campo `length` da entrada "." de um diretório
    /// (contagem canônica de entradas vivas — genvmc/mymc).
    fn set_dot_length(&mut self, dir_cluster: u32, count: u32) -> Result<()> {
        let per = (self.info.cluster_size as usize / ENTRY_SIZE).max(1);
        let data = self.read_chain(dir_cluster)?;
        for (idx, chunk) in data.chunks(ENTRY_SIZE).enumerate() {
            if let Some(e) = parse_entry(chunk) {
                if e.name == "." {
                    let rel = self.chain_nth_rel(dir_cluster, idx / per)?;
                    let off = (idx % per) * ENTRY_SIZE;
                    let mut page = self.read_cluster(rel)?;
                    page[off + 4..off + 8].copy_from_slice(&count.to_le_bytes());
                    self.write_cluster(rel, &page)?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    // ── listagem (API pública) ──────────────────────────────────────────────
    /// Lista os saves (diretórios de 2º nível do root).
    pub fn list_saves(&mut self) -> Result<Vec<VmcSave>> {
        let root_entries = self.read_dir_entries(self.info.rootdir_cluster)?;
        let mut saves = Vec::new();
        for e in root_entries {
            if !is_dir(e.mode) || e.name == "." || e.name == ".." {
                continue;
            }
            let files = self.read_dir_entries(e.cluster)?;
            let mut size = 0u64;
            let mut nfiles = 0usize;
            for f in &files {
                if f.name != "." && f.name != ".." && is_file(f.mode) {
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

    // ── import PSU (restauro — v0.3) ────────────────────────────────────────
    /// Restaura um `.psu` (uLE/EMS) dentro da VMC — o save vira um diretório
    /// de 2º nível do root, como o psuPaste do uLE. Estrutura esperada:
    /// header raiz (dir, size = nº de headers seguintes) → "." e ".."
    /// (pseudo, size 0) → arquivos (size + dados alinhados a 1024 B).
    ///
    /// All-or-nothing: os dados novos são gravados ANTES de qualquer
    /// alteração no save existente; qualquer erro devolve os clusters
    /// alocados e deixa a FAT em disco intacta (`flush_fat` só no fim).
    pub fn import_psu(&mut self, psu_path: &Path, overwrite: bool) -> Result<ImportReport> {
        let mut raw = Vec::new();
        std::fs::File::open(psu_path)
            .map_err(Ps2Error::Io)?
            .read_to_end(&mut raw)
            .map_err(Ps2Error::Io)?;
        if raw.len() > 128 * 1024 * 1024 {
            return Err(Ps2Error::InvalidPsu("arquivo PSU gigantesco (> 128 MiB)".into()));
        }
        if raw.len() < ENTRY_SIZE {
            return Err(Ps2Error::InvalidPsu("arquivo menor que um header (512 B)".into()));
        }

        // header raiz
        let main = parse_psu_header(&raw[..ENTRY_SIZE])?;
        if main.attr & DF_DIRECTORY == 0 {
            return Err(Ps2Error::InvalidPsu(format!(
                "header raiz não é diretório (attr {:#06x})",
                main.attr
            )));
        }
        if main.name.is_empty() {
            return Err(Ps2Error::InvalidPsu("nome do save vazio no header raiz".into()));
        }
        let name: String = main.name.chars().take(32).collect();
        let count = main.size as usize;
        if count < 2 {
            return Err(Ps2Error::InvalidPsu(format!(
                "header raiz anuncia {count} entradas (mínimo 2: \".\" e \"..\")"
            )));
        }

        // percorre o conteúdo (uLE filer.c restore: pseudo = dir com size 0)
        let mut files: Vec<(VmcEntry, Vec<u8>)> = Vec::new();
        let mut pos = ENTRY_SIZE;
        for i in 0..count {
            let end = (pos + ENTRY_SIZE).min(raw.len());
            let h = parse_psu_header(&raw[pos..end])?;
            pos += ENTRY_SIZE;
            if h.attr & DF_DIRECTORY != 0 {
                if h.size == 0 {
                    continue; // "." / ".." (uLE: "Dummy/Pseudo folder entry")
                }
                return Err(Ps2Error::InvalidPsu(format!(
                    "subpasta '{}' no PSU — uLE não gera saves com subpastas",
                    h.name
                )));
            }
            if h.name.is_empty() || h.name == "." || h.name == ".." {
                return Err(Ps2Error::InvalidPsu(format!(
                    "arquivo {i} com nome inválido '{}'",
                    h.name
                )));
            }
            let size = h.size as usize;
            if pos + size > raw.len() {
                return Err(Ps2Error::InvalidPsu(format!(
                    "arquivo '{}' truncado no PSU (declara {} B)",
                    h.name,
                    size
                )));
            }
            let mut data = raw[pos..pos + size].to_vec();
            data.resize(size, 0);
            pos += size + psu_data_pad(h.size);
            files.push((
                VmcEntry {
                    name: h.name.chars().take(32).collect(),
                    mode: MODE_FILE as u32,
                    length: h.size,
                    cluster: 0,
                    created: h.created,
                    modified: h.modified,
                },
                data,
            ));
        }
        if files.is_empty() {
            return Err(Ps2Error::InvalidPsu("PSU sem arquivos de dados".into()));
        }

        // colisão de nome no root
        let existing = self.find_entry(self.info.rootdir_cluster, &name)?;
        if existing.is_some() && !overwrite {
            return Err(Ps2Error::InvalidVmc(format!(
                "save '{name}' já existe na VMC — ative sobrescrever para substituir"
            )));
        }

        // sobrescrever: zera a entrada antiga AGORA (o append_entry vai
        // reusar o slot morto); as cadeias antigas só são liberadas no fim
        let mut old_to_free: Vec<u32> = Vec::new();
        if let Some((slot, old)) = existing {
            let per = (self.info.cluster_size as usize / ENTRY_SIZE).max(1);
            let rel = self.chain_nth_rel(self.info.rootdir_cluster, slot as usize / per)?;
            let off = (slot as usize % per) * ENTRY_SIZE;
            let mut page = self.read_cluster(rel)?;
            page[off..off + ENTRY_SIZE].fill(0);
            self.write_cluster(rel, &page)?;
            for f in self.read_dir_entries(old.cluster)? {
                if is_file(f.mode) {
                    old_to_free.push(f.cluster);
                }
            }
            old_to_free.push(old.cluster);
        }

        // plano de espaço (após zerar, o slot antigo já é reutilizável)
        let cs = self.info.cluster_size as u64;
        let nentries = (files.len() + 2) as u64;
        let dir_clusters = (nentries * ENTRY_SIZE as u64).div_ceil(cs) as u32;
        let file_clusters: u32 = files
            .iter()
            .map(|(_, d)| ((d.len() as u64).max(1)).div_ceil(cs) as u32)
            .sum();
        let root_slots = self.read_chain(self.info.rootdir_cluster)?.len() / ENTRY_SIZE;
        let used_slots = self.dir_count(self.info.rootdir_cluster)? as usize;
        let needs_root_cluster = used_slots >= root_slots;
        let needed = dir_clusters + file_clusters + u32::from(needs_root_cluster);
        if self.free_clusters() < needed {
            return Err(Ps2Error::InvalidVmc(format!(
                "espaço insuficiente na VMC: precisamos de {needed} cluster(s), livre(s) {}",
                self.free_clusters()
            )));
        }

        // ── execução (rollback de FAT em memória em qualquer erro) ──────────
        let mut allocated: Vec<u32> = Vec::new();
        let result = (|| -> Result<ImportReport> {
            // 1) cadeias de dados dos arquivos
            for (fe, data) in &mut files {
                let n = ((data.len() as u64).max(1)).div_ceil(cs) as u32;
                let chain = self.alloc_chain(n)?;
                allocated.extend_from_slice(&chain);
                fe.cluster = chain[0];
                let csz = self.info.cluster_size as usize;
                for (i, crel) in chain.iter().enumerate() {
                    let start = i * csz;
                    let stop = ((i + 1) * csz).min(data.len());
                    let mut page = vec![0u8; csz];
                    if start < data.len() {
                        page[..stop - start].copy_from_slice(&data[start..stop]);
                    }
                    self.write_cluster(*crel, &page)?;
                }
            }

            // 2) clusters do diretório do save
            let dir_chain = self.alloc_chain(dir_clusters)?;
            allocated.extend_from_slice(&dir_chain);

            // 3) entrada do save no root (sabe o slot → dir_entry de ".")
            let root_count = self.dir_count(self.info.rootdir_cluster)?;
            let root_entry = VmcEntry {
                name: name.clone(),
                mode: MODE_DIR as u32,
                length: nentries as u32,
                cluster: dir_chain[0],
                created: main.created,
                modified: main.modified,
            };
            let slot = self.append_entry(self.info.rootdir_cluster, &root_entry)?;

            // 4) conteúdo do diretório do save: "." + ".." + arquivos
            let per = (self.info.cluster_size as usize / ENTRY_SIZE).max(1);
            let dot = VmcEntry {
                name: ".".into(),
                mode: MODE_DIR as u32,
                length: nentries as u32,
                cluster: dir_chain[0],
                created: main.created,
                modified: main.modified,
            };
            let dotdot = VmcEntry {
                name: "..".into(),
                mode: MODE_DOTDOT as u32,
                length: 0,
                cluster: 0,
                created: main.created,
                modified: main.modified,
            };
            let mut all = vec![dot, dotdot];
            all.extend(files.iter().map(|(fe, _)| fe.clone()));
            for (i, e) in all.iter().enumerate() {
                let dir_entry = if i == 0 { slot } else { i as u32 };
                let rel = dir_chain[i / per];
                let off = (i % per) * ENTRY_SIZE;
                let mut page = self.read_cluster(rel)?;
                page[off..off + ENTRY_SIZE].copy_from_slice(&entry_bytes(e, dir_entry));
                self.write_cluster(rel, &page)?;
            }

            // 5) contagem do root ("." entry) e liberação do save antigo
            self.set_dot_length(self.info.rootdir_cluster, root_count)?;
            for c in old_to_free.drain(..) {
                self.free_chain(c);
            }

            // 6) FAT íntegra em disco — ponto de não-retorno
            self.flush_fat()?;

            let bytes: u64 = files.iter().map(|(_, d)| d.len() as u64).sum();
            Ok(ImportReport {
                name,
                files: files.len(),
                bytes,
                clusters_used: needed,
            })
        })();

        match result {
            Ok(rep) => Ok(rep),
            Err(e) => {
                for c in allocated.drain(..) {
                    self.fat[c as usize] = FAT_FREE;
                }
                Err(e)
            }
        }
    }

    // ── export PSU (backup — réplica do writer uLE filer.c PM_PSU_BACKUP) ───
    /// Exporta um save para `.psu`. Compatível com o reader do uLE:
    /// header raiz (size = nº de headers seguintes), pseudo-entradas "." e
    /// ".." com size 0, arquivos com dados alinhados a 1024 B. Subpastas
    /// internas (raro — uLE não copia) viram headers neutros size 0, que o
    /// restore do uLE ignora como "Dummy/Pseudo folder entry".
    pub fn export_psu(&mut self, save_name: &str, out_path: &Path) -> Result<u64> {
        let root_entries = self.read_dir_entries(self.info.rootdir_cluster)?;
        let dir = root_entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(save_name))
            .ok_or_else(|| Ps2Error::InvalidVmc(format!("save '{save_name}' não encontrado")))?
            .clone();
        let entries = self.read_dir_entries(dir.cluster)?;
        let count = entries.len() as u32;

        let mut out: Vec<u8> = Vec::with_capacity(64 * 1024);
        out.extend_from_slice(&psu_header_bytes(
            dir.mode as u16,
            count,
            &dir.created,
            &dir.modified,
            dir.name.as_bytes(),
        ));
        for e in &entries {
            if is_dir(e.mode) {
                // "." e ".." (size 0) e subpastas internas neutralizadas — o
                // restore do uLE ignora header dir com size 0 como "Dummy"
                out.extend_from_slice(&psu_header_bytes(e.mode as u16, 0, &e.created, &e.modified, e.name.as_bytes()));
                continue;
            }
            if !is_file(e.mode) {
                continue;
            }
            out.extend_from_slice(&psu_header_bytes(
                e.mode as u16,
                e.length,
                &e.created,
                &e.modified,
                e.name.as_bytes(),
            ));
            let data = self.read_chain(e.cluster)?;
            let n = (e.length as usize).min(data.len());
            out.extend_from_slice(&data[..n]);
            out.extend(std::iter::repeat_n(0u8, psu_data_pad(e.length)));
        }
        std::fs::write(out_path, &out).map_err(Ps2Error::Io)?;
        Ok(out.len() as u64)
    }
}

// ── testes ──────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    /// Cartão sintético no layout genvmc 8 MiB: pagesize 512, cluster 1024 B,
    /// 8192 clusters, 1 ifc, 32 clusters de FAT, alloc_offset 34, root em 0.
    fn fixture_card(path: &std::path::Path) {
        use std::io::Write as _;
        const CLUSTER: usize = 1024;
        const CLUSTERS: u32 = 8192;
        const FAT_CLUSTERS: u32 = 32;
        const ALLOC: u32 = 34; // superblock(1) + ifc(1) + fat(32)
        let mut f = std::fs::File::create(path).unwrap();
        let mut card = vec![0u8; CLUSTERS as usize * CLUSTER];

        card[0..28].copy_from_slice(MAGIC);
        card[28..36].copy_from_slice(b"1.2.0.0\0");
        card[40..42].copy_from_slice(&512i16.to_le_bytes());
        card[42..44].copy_from_slice(&2u16.to_le_bytes());
        card[44..46].copy_from_slice(&16u16.to_le_bytes());
        card[48..52].copy_from_slice(&CLUSTERS.to_le_bytes());
        card[52..56].copy_from_slice(&ALLOC.to_le_bytes());
        card[60..64].copy_from_slice(&0u32.to_le_bytes());
        card[336] = 2;
        card[340..344].copy_from_slice(&(CLUSTER as u32).to_le_bytes());
        card[344..348].copy_from_slice(&256u32.to_le_bytes());
        card[352..356].copy_from_slice(&1i32.to_le_bytes());
        card[368..372].copy_from_slice(&(CLUSTERS - ALLOC - 64).to_le_bytes());
        card[408..412].copy_from_slice(&1u32.to_le_bytes()); // ifc_list[0]

        // cluster 1: ifc → ponteiros para os 32 clusters da FAT (2..34)
        for i in 0..FAT_CLUSTERS {
            let idx = CLUSTER + i as usize * 4;
            card[idx..idx + 4].copy_from_slice(&(2 + i).to_le_bytes());
        }
        // FAT: entrada 0 (root dir) = EOF; resto livre
        let fat_off = 2 * CLUSTER;
        card[fat_off..fat_off + 4].copy_from_slice(&FAT_EOF.to_le_bytes());
        let fat_entries = (CLUSTERS - ALLOC - 64) as usize;
        for i in 1..fat_entries {
            card[fat_off + i * 4..fat_off + i * 4 + 4].copy_from_slice(&FAT_FREE.to_le_bytes());
        }
        // root dir: "." e ".." (genvmc)
        let t: [u8; 8] = [0x25, 0x06, 0x0e, 0x14, 0x0b, 0x11, 0xe9, 0x07];
        let root = ALLOC as usize * CLUSTER;
        let dot = VmcEntry {
            name: ".".into(),
            mode: MODE_DIR as u32,
            length: 2,
            cluster: 0,
            created: t,
            modified: t,
        };
        card[root..root + ENTRY_SIZE].copy_from_slice(&entry_bytes(&dot, 0));
        let dotdot = VmcEntry {
            name: "..".into(),
            mode: MODE_DOTDOT as u32,
            length: 2,
            cluster: 0,
            created: t,
            modified: t,
        };
        card[root + ENTRY_SIZE..root + 2 * ENTRY_SIZE].copy_from_slice(&entry_bytes(&dotdot, 0));

        f.write_all(&card).unwrap();
        f.sync_all().unwrap();
    }

    /// PSU sintético no layout uLE filer.c (raiz + "." + ".." + arquivos).
    fn fixture_psu(path: &std::path::Path, save: &str) -> Vec<(String, usize)> {
        let t: [u8; 8] = [0x25, 0x06, 0x0e, 0x14, 0x0b, 0x11, 0xe9, 0x07];
        let files: Vec<(String, Vec<u8>)> = vec![
            ("icon.sys".into(), vec![b'S'; 77]),
            ("icon.icn".into(), vec![b'I'; 1172]),
            ("SCE_SYSTEM_CONF".into(), vec![b'C'; 512]),
        ];
        let mut out: Vec<u8> = Vec::new();
        let count = (2 + files.len()) as u32;
        out.extend_from_slice(&psu_header_bytes(MODE_DIR, count, &t, &t, save.as_bytes()));
        out.extend_from_slice(&psu_header_bytes(MODE_DIR, 0, &t, &t, b"."));
        out.extend_from_slice(&psu_header_bytes(MODE_DIR, 0, &t, &t, b".."));
        for (n, d) in &files {
            out.extend_from_slice(&psu_header_bytes(
                MODE_FILE,
                d.len() as u32,
                &t,
                &t,
                n.as_bytes(),
            ));
            out.extend_from_slice(d);
            out.extend(std::iter::repeat_n(0u8, psu_data_pad(d.len() as u32)));
        }
        std::fs::write(path, &out).unwrap();
        files.into_iter().map(|(n, d)| (n, d.len())).collect()
    }

    #[test]
    fn import_e_roundtrip_psu() {
        let tmp = std::env::temp_dir().join(format!("ps2m-vmc-t{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let card_path = tmp.join("card.bin");
        fixture_card(&card_path);
        let psu_path = tmp.join("save.psu");
        let expect = fixture_psu(&psu_path, "BESLES-52546TEST");

        // 1) cartão vazio → 0 saves
        let mut card = Vmc::open(&card_path).unwrap();
        assert_eq!(card.list_saves().unwrap().len(), 0);
        let free0 = card.free_clusters();

        // 2) import → 1 save, arquivos e tamanhos corretos
        let rep = card.import_psu(&psu_path, false).unwrap();
        assert_eq!(rep.name, "BESLES-52546TEST");
        assert_eq!(rep.files, expect.len());
        let total: u64 = expect.iter().map(|(_, s)| *s as u64).sum();
        assert_eq!(rep.bytes, total);
        let saves = card.list_saves().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].name, "BESLES-52546TEST");
        assert_eq!(saves[0].files, expect.len());
        assert_eq!(saves[0].size_bytes, total);
        assert!(card.free_clusters() < free0);

        // 3) sem sobrescrever → erro claro; com sobrescrever → 1 save só
        let err = card.import_psu(&psu_path, false).unwrap_err().to_string();
        assert!(err.contains("já existe"));
        card.import_psu(&psu_path, true).unwrap();
        let saves = card.list_saves().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].files, expect.len());

        // 4) roundtrip: export → cartão novo → import → listagem idêntica
        let out_psu = tmp.join("out.psu");
        let n = card.export_psu("BESLES-52546TEST", &out_psu).unwrap();
        assert!(n > 0);
        let card2_path = tmp.join("card2.bin");
        fixture_card(&card2_path);
        let mut card2 = Vmc::open(&card2_path).unwrap();
        card2.import_psu(&out_psu, false).unwrap();
        let a = card.list_saves().unwrap();
        let b = card2.list_saves().unwrap();
        // cluster difere entre cartões (alocação própria) — conteúdo é igual
        let sig = |v: &[VmcSave]| {
            v.iter()
                .map(|s| (s.name.clone(), s.files, s.size_bytes))
                .collect::<Vec<_>>()
        };
        assert_eq!(sig(&a), sig(&b));

        // 5) dois saves convivem (slots distintos)
        let psu_b = tmp.join("save2.psu");
        fixture_psu(&psu_b, "SLUS_200.02XXX");
        card2.import_psu(&psu_b, false).unwrap();
        let saves = card2.list_saves().unwrap();
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[0].name, "BESLES-52546TEST");
        assert_eq!(saves[1].name, "SLUS_200.02XXX");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn psu_ruim_da_erro_claro() {
        let tmp = std::env::temp_dir().join(format!("ps2m-vmc-e{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let card_path = tmp.join("card.bin");
        fixture_card(&card_path);
        let mut card = Vmc::open(&card_path).unwrap();

        // header raiz não-diretório
        let bad = tmp.join("bad.psu");
        std::fs::write(&bad, psu_header_bytes(MODE_FILE, 2, &[0; 8], &[0; 8], b"X")).unwrap();
        let err = card.import_psu(&bad, false).unwrap_err().to_string();
        assert!(err.contains("não é diretório"));

        // subpasta com dados → erro (uLE também aborta)
        let t = [0u8; 8];
        let mut sub = psu_header_bytes(MODE_DIR, 3, &t, &t, b"S");
        sub.extend_from_slice(&psu_header_bytes(MODE_DIR, 0, &t, &t, b"."));
        sub.extend_from_slice(&psu_header_bytes(MODE_DIR, 0, &t, &t, b".."));
        sub.extend_from_slice(&psu_header_bytes(MODE_DIR, 99, &t, &t, b"pasta"));
        let subp = tmp.join("sub.psu");
        std::fs::write(&subp, &sub).unwrap();
        let err = card.import_psu(&subp, false).unwrap_err().to_string();
        assert!(err.contains("subpasta"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
