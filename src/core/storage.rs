//! Seletor de dispositivos seguro (Anti-Desastre Drive Picker) — Mega Prompt 7.
//!
//! Lê o sysfs (`/sys/block`) e `/proc/self/mounts` — **zero libudev**.
//! Discos internos do sistema (raiz `/`, `/boot`, `/home`, NVMe, virtio,
//! md/dm, zram…) NUNCA aparecem como destino de gravação, a menos que a
//! chave de desenvolvedor `PS2_MANAGER_ALLOW_INTERNAL=1` esteja ativa.
//!
//! Categorias:
//!  * **Destinos válidos** — removíveis (`removable == 1`) ou barramento
//!    USB/SD (`/dev/sdb`, `/dev/mmcblk0`) com ponto de montagem visível.
//!  * **Bloqueados por padrão** — qualquer dispositivo que hospede `/`,
//!    `/boot`, `/home`…, NVMe interno, discos virtio/loop/dm/md/zram.

use crate::error::{Ps2Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Pontos de montagem do sistema — quem hospede um deles é disco interno.
const PROTECTED_MOUNTS: [&str; 12] = [
    "/", "/boot", "/boot/efi", "/efi", "/home", "/root", "/etc", "/usr", "/var", "/opt",
    "/srv", "/swap",
];

/// Prefixos de dispositivos sempre internos/virtuais (bloqueados).
const INTERNAL_PREFIXES: [&str; 8] = ["nvme", "zram", "ram", "loop", "dm-", "md", "vd", "sr"];

/// Filesystems que não fazem sentido como destino (ignorados na listagem).
const SKIP_FS: [&str; 2] = ["swap", "squashfs"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusKind {
    Usb,
    Mmc,
    Virtual,
    Other,
}

impl BusKind {
    pub fn label(self) -> &'static str {
        match self {
            BusKind::Usb => "USB",
            BusKind::Mmc => "SD/MMC",
            BusKind::Virtual => "virtual",
            BusKind::Other => "outro",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsClass {
    Exfat,
    Fat32,
    Ntfs,
    Ext,
    Other,
}

impl FsClass {
    pub fn from_fs(fs_type: &str) -> Self {
        match fs_type.to_ascii_lowercase().as_str() {
            "exfat" => FsClass::Exfat,
            "vfat" | "fat32" | "msdos" => FsClass::Fat32,
            "ntfs" | "ntfs3" | "fuseblk" => FsClass::Ntfs,
            "ext2" | "ext3" | "ext4" => FsClass::Ext,
            _ => FsClass::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FsClass::Exfat => "exFAT",
            FsClass::Fat32 => "FAT32",
            FsClass::Ntfs => "NTFS",
            FsClass::Ext => "ext",
            FsClass::Other => "outro",
        }
    }

    /// exFAT é o formato recomendado para BDM (MX4SIO/USB) no OPL.
    pub fn bdm_recommended(self) -> bool {
        matches!(self, FsClass::Exfat)
    }

    /// Tamanho máximo de arquivo suportado (FAT32 = 4 GiB − 1).
    pub fn max_file(self) -> u64 {
        match self {
            FsClass::Fat32 => 4_294_967_295,
            _ => u64::MAX,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MountInfo {
    pub source: String,       // "sdb1"
    pub mount_point: PathBuf, // /run/media/usuario/PS2_SD
    pub fs_type: String,      // "exfat"
    pub free_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct TargetDevice {
    pub name: String,      // "sdb"
    #[allow(dead_code)] // inspecionável em futuras UIs (fdisk-like)
    pub dev_node: PathBuf, // /dev/sdb
    pub model: String,
    pub vendor: String,
    pub bus: BusKind,
    pub removable: bool,
    pub size_bytes: u64,
    pub mounts: Vec<MountInfo>,
}

impl TargetDevice {
    /// Nome amigável exibido no picker: "SanDisk Ultra (64.0 GB)".
    pub fn display_name(&self) -> String {
        let m = if !self.model.is_empty() {
            self.model.clone()
        } else {
            match self.bus {
                BusKind::Usb => "Dispositivo USB".to_string(),
                BusKind::Mmc => "Cartão SD".to_string(),
                _ => "Mídia externa".to_string(),
            }
        };
        let extra = if !self.vendor.is_empty() {
            format!(" {}", self.vendor)
        } else {
            String::new()
        };
        format!("{}{} ({})", m, extra, crate::cli::fmt_size(self.size_bytes))
    }

    /// Partição montada "principal" (a com mais espaço livre).
    pub fn primary_mount(&self) -> Option<&MountInfo> {
        self.mounts.iter().max_by_key(|m| m.free_bytes)
    }
}

/// Chave de desenvolvedor: libera discos internos como destino (uso por
/// conta e risco — nunca necessário para o fluxo normal de mídias PS2).
pub fn allow_internal() -> bool {
    std::env::var("PS2_MANAGER_ALLOW_INTERNAL")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Um ponto de montagem é "protegido" (faz parte do sistema)?
pub fn is_protected_mount(mp: &Path) -> bool {
    let s = mp.to_string_lossy();
    for p in PROTECTED_MOUNTS {
        if p == "/" {
            if s == "/" {
                return true;
            }
        } else if s == p || s.starts_with(&format!("{p}/")) {
            return true;
        }
    }
    false
}

fn basename(dev: &str) -> &str {
    dev.rsplit('/').next().unwrap_or(dev)
}

/// Decodifica escapes octais do /proc/self/mounts (ex.: `\040` = espaço).
fn decode_mount_escapes(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut code = String::new();
        for _ in 0..3 {
            match chars.peek() {
                Some(d) if d.is_digit(8) => code.push(chars.next().unwrap_or('0')),
                _ => break,
            }
        }
        match u8::from_str_radix(&code, 8) {
            Ok(v) => out.push(v as char),
            Err(_) => {
                out.push('\\');
                out.push_str(&code);
            }
        }
    }
    out
}

fn read_mounts() -> Vec<(String, PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(text) = fs::read_to_string("/proc/self/mounts") else {
        return out;
    };
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(source), Some(mp), Some(fs_type)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if SKIP_FS.contains(&fs_type) {
            continue;
        }
        out.push((
            basename(source).to_string(),
            PathBuf::from(decode_mount_escapes(mp)),
            fs_type.to_string(),
        ));
    }
    out
}

/// Espaço livre/total via `statvfs(3)` — sem invocar processos externos.
pub fn statvfs_bytes(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs apenas lê o caminho NUL-terminado e preenche `st`.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc == 0 {
        let free = st.f_bavail as u64 * st.f_bsize as u64;
        let total = st.f_blocks as u64 * st.f_bsize as u64;
        Some((free, total))
    } else {
        None
    }
}

pub fn free_bytes(path: &Path) -> Option<u64> {
    statvfs_bytes(path).map(|(f, _)| f)
}

fn detect_bus(name: &str) -> BusKind {
    if name.starts_with("mmcblk") {
        return BusKind::Mmc;
    }
    if INTERNAL_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return BusKind::Virtual;
    }
    // /sys/block/<dev> é symlink para a árvore física do barramento
    if let Ok(p) = fs::read_link(format!("/sys/block/{name}")) {
        let s = p.to_string_lossy().to_lowercase();
        if s.contains("/usb") {
            return BusKind::Usb;
        }
        if s.contains("/mmc") {
            return BusKind::Mmc;
        }
    }
    BusKind::Other
}

fn read_u64(path: &str) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn read_trimmed(path: &str) -> String {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Escaneia mídias externas SEGURAS (removíveis/USB/SD com ponto de
/// montagem). Discos internos são filtrados categoricamente.
pub fn scan_target_devices() -> Vec<TargetDevice> {
    let mut out = Vec::new();
    let entries = match fs::read_dir("/sys/block") {
        Ok(e) => e,
        Err(_) => return out, // ambiente sem sysfs (não-Linux)
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mounts = read_mounts();
    let allow = allow_internal();

    for name in names {
        let bus = detect_bus(&name);
        let removable = read_u64(&format!("/sys/block/{name}/removable")) == 1;
        if !allow {
            if matches!(bus, BusKind::Virtual) {
                continue;
            }
            if !removable && !matches!(bus, BusKind::Usb | BusKind::Mmc) {
                continue;
            }
        }

        // partições do dispositivo (sdb1, sdb2…) + o próprio device
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(rd) = fs::read_dir(format!("/sys/block/{name}")) {
            let mut parts: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.len() > name.len() && n.starts_with(&name))
                .collect();
            parts.sort();
            candidates.extend(parts);
        }
        candidates.push(name.clone());

        // dispositivo que hospeda montagem protegida → disco interno → descarta
        if !allow
            && mounts
                .iter()
                .any(|m| candidates.contains(&m.0) && is_protected_mount(&m.1))
        {
            continue;
        }

        let mut dev_mounts = Vec::new();
        for m in &mounts {
            if !candidates.contains(&m.0) {
                continue;
            }
            let (free, total) = statvfs_bytes(&m.1).unwrap_or((0, 0));
            dev_mounts.push(MountInfo {
                source: m.0.clone(),
                mount_point: m.1.clone(),
                fs_type: m.2.clone(),
                free_bytes: free,
                total_bytes: total,
            });
        }
        if dev_mounts.is_empty() {
            continue; // sem montagem visível → não é destino prático
        }

        out.push(TargetDevice {
            size_bytes: read_u64(&format!("/sys/block/{name}/size")) * 512,
            model: read_trimmed(&format!("/sys/block/{name}/device/model")),
            vendor: read_trimmed(&format!("/sys/block/{name}/device/vendor")),
            dev_node: PathBuf::from(format!("/dev/{name}")),
            name,
            bus,
            removable,
            mounts: dev_mounts,
        });
    }
    out
}

/// O caminho pertence a alguma mídia externa detectada?
pub fn is_removable_root(path: &Path) -> bool {
    scan_target_devices()
        .iter()
        .any(|d| d.mounts.iter().any(|m| path.starts_with(&m.mount_point)))
}

/// Classifica o filesystem do ponto de montagem que contém `root`.
/// Retorna `Other` quando o caminho não está em nenhuma mídia externa.
pub fn fs_class_for_root(root: &Path) -> FsClass {
    for d in scan_target_devices() {
        for m in &d.mounts {
            if root.starts_with(&m.mount_point) {
                return FsClass::from_fs(&m.fs_type);
            }
        }
    }
    FsClass::Other
}

/// Alocação contígua via `fallocate(2)` — reduz fragmentação e ciclos de
/// escrita na flash. Best-effort: FAT32 não suporta e retorna `Ok(false)`
/// (a escrita sequencial pura segue como fallback).
pub fn fallocate_space(file: &fs::File, len: u64) -> std::result::Result<bool, std::io::Error> {
    use std::os::unix::io::AsRawFd;
    if len == 0 {
        return Ok(false);
    }
    // SAFETY: fallocate(2) apenas atua no fd fornecido.
    let rc = unsafe { libc::fallocate(file.as_raw_fd(), 0, 0, len as libc::off_t) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.kind() {
        std::io::ErrorKind::Unsupported
        | std::io::ErrorKind::InvalidInput
        | std::io::ErrorKind::PermissionDenied => Ok(false),
        _ => Err(err),
    }
}

/// Helper: mensagem de erro padronizada para mídia cheia.
pub fn ensure_space(free: u64, needed: u64) -> Result<()> {
    if needed + 8_000_000 > free {
        return Err(Ps2Error::Other(format!(
            "espaço insuficiente: livre {} · necessário {}",
            crate::cli::fmt_size(free),
            crate::cli::fmt_size(needed)
        )));
    }
    Ok(())
}

// ── FIEMAP — validação real de fragmentação (v0.3) ──────────────────────────
/// `struct fiemap` do kernel (header de 32 B + array de extents).
#[repr(C)]
struct FiemapHdr {
    start: u64,
    length: u64,
    flags: u32,
    mapped: u32,
    count: u32,
    reserved: u32,
}

/// `struct fiemap_extent` do kernel (48 B).
#[allow(dead_code)]
#[repr(C)]
struct FiemapExtent {
    logical: u64,
    physical: u64,
    length: u64,
    reserved: u64,
    flags: u32,
    r2: [u32; 3],
}

/// `FS_IOC_FIEMAP` = `_IOWR('f', 11, struct fiemap)` = 0xC020660B.
/// Tipo do request difere por libc: glibc usa `c_ulong`, musl usa
/// `c_int` (o kernel trunca para `unsigned int` no syscall, então o
/// bit-pattern chega correto nos dois ambientes).
#[cfg(target_env = "musl")]
const FS_IOC_FIEMAP: libc::c_int = 0xC020_660Bu32 as libc::c_int;
#[cfg(not(target_env = "musl"))]
const FS_IOC_FIEMAP: libc::c_ulong = 0xC020_660B;

/// Resultado best-effort da análise de fragmentação de um arquivo.
#[derive(Debug, Clone, Copy)]
pub struct FragInfo {
    /// false = o filesystem não implementa FIEMAP (FAT32, tmpfs) — n/d
    pub supported: bool,
    /// nº de extents mapeados (teto de 256 por chamada)
    pub extents: u32,
}

impl FragInfo {
    /// `Some(true)` = fragmentado (>1 extent) · `Some(false)` = contíguo ·
    /// `None` = filesystem sem FIEMAP.
    pub fn fragmented(&self) -> Option<bool> {
        if self.supported {
            Some(self.extents > 1)
        } else {
            None
        }
    }
}

/// Conta extents de um arquivo via `ioctl(FS_IOC_FIEMAP)` (Linux ≥ 2.6.28;
/// ext4/xfs/btrfs e exFAT moderno implementam; FAT32 e tmpfs não →
/// `supported: false`). Best-effort: nunca é condição de falha da
/// instalação — o valor entra no relatório como diagnóstico do transporte
/// USB/SIO2 (extents extras custam seeks no barramento do PS2).
pub fn fragmentation(file: &fs::File) -> std::io::Result<FragInfo> {
    use std::os::unix::io::AsRawFd;
    if file.metadata()?.len() == 0 {
        return Ok(FragInfo { supported: true, extents: 0 });
    }
    const MAX: u32 = 256;
    let mut buf = vec![0u8; 32 + MAX as usize * std::mem::size_of::<FiemapExtent>()];
    {
        // SAFETY: buf tem exatamente FiemapHdr + MAX extents alinhados.
        let hdr = unsafe { &mut *(buf.as_mut_ptr() as *mut FiemapHdr) };
        hdr.start = 0;
        hdr.length = u64::MAX;
        hdr.flags = 0;
        hdr.count = MAX;
        hdr.reserved = 0;
    }
    // SAFETY: o kernel escreve apenas dentro de buf (tamanho declarado no
    // próprio número do ioctl, que codifica o tamanho de `struct fiemap`).
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_FIEMAP, buf.as_mut_ptr()) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        return match e.raw_os_error() {
            // FS sem suporte (ENOTTY = ioctl desconhecido; EINVAL/EOPNOTSUPP
            // = implementação ausente/flags não suportadas)
            Some(libc::ENOTTY) | Some(libc::EOPNOTSUPP) | Some(libc::EINVAL) => {
                Ok(FragInfo { supported: false, extents: 0 })
            }
            _ => Err(e),
        };
    }
    // SAFETY: leitura do header preenchido pelo kernel.
    let hdr = unsafe { &*(buf.as_ptr() as *const FiemapHdr) };
    Ok(FragInfo { supported: true, extents: hdr.mapped })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fiemap_best_effort() {
        let tmp = std::env::temp_dir().join(format!("ps2m-frag-t{}.bin", std::process::id()));
        let _ = fs::remove_file(&tmp);
        fs::write(&tmp, vec![0xABu8; 128 * 1024]).unwrap();
        let f = fs::File::open(&tmp).unwrap();
        let fi = fragmentation(&f).unwrap();
        // em qualquer filesystem: nunca erro — supported ou n/d
        if fi.supported {
            assert!(fi.extents >= 1);
            assert_eq!(fi.fragmented(), Some(fi.extents > 1));
        } else {
            assert_eq!(fi.fragmented(), None);
        }
        let _ = fs::remove_file(&tmp);
    }
}
