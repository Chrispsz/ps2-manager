//! Biblioteca OPL — estrutura de pastas, naming `GAMEID.Título.iso`,
//! instalação (cópia sequencial ou ZSO) e escaneamento.

use crate::error::{Ps2Error, Result};
use crate::iso::parser::{analyze_path, GameInfo, Media};
use crate::iso::zso::compress_iso;
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Pastas criadas na raiz do dispositivo — idênticas a
/// `sbCreateFolders` (supportbase.c:842-851): CFG, THM, LNG, ART, VMC,
/// CHT, APPS (+ CD, DVD para as imagens).
pub const OPL_FOLDERS: [&str; 9] =
    ["CD", "DVD", "ART", "CFG", "VMC", "CHT", "APPS", "THM", "LNG"];

pub fn ensure_folders(device_root: &Path) -> Result<()> {
    for d in OPL_FOLDERS {
        fs::create_dir_all(device_root.join(d)).map_err(Ps2Error::Io)?;
    }
    Ok(())
}

/// Remove caracteres proibidos em exFAT/FAT32 e colapsa espaços.
pub fn sanitize_title(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('-');
        } else if c.is_control() {
            continue;
        } else {
            out.push(c);
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(60).collect::<String>().trim().to_string()
}

/// `SLUS_214.44.Gran Turismo 4.iso` — convenção oficial de nomes OPL ISO.
pub fn iso_target_name(info: &GameInfo, ext: &str) -> String {
    let title = sanitize_title(&info.volume_id);
    let base = if title.is_empty() { "Unknown" } else { &title };
    format!("{}.{}.{}", info.game_id, base, ext)
}

fn copy_sequential(
    input: &Path,
    output: &Path,
    progress: Option<Arc<AtomicU64>>,
) -> Result<u64> {
    let mut r = BufReader::with_capacity(1024 * 1024, fs::File::open(input).map_err(Ps2Error::Io)?);
    let mut w = BufWriter::with_capacity(1024 * 1024, fs::File::create(output).map_err(Ps2Error::Io)?);
    let mut buf = vec![0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).map_err(Ps2Error::Io)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(Ps2Error::Io)?;
        total += n as u64;
        if let Some(p) = &progress {
            p.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
    w.flush().map_err(Ps2Error::Io)?;
    Ok(total)
}

pub struct InstallOptions {
    pub as_zso: bool,
}

/// Instala uma ISO na biblioteca OPL: detecta Game ID, classifica CD/DVD,
/// nomeia `GAMEID.Título.iso` e copia (ou converte p/ ZSO) sequencialmente.
pub async fn install(
    iso: &Path,
    device_root: &Path,
    opts: &InstallOptions,
    progress: Option<Arc<AtomicU64>>,
) -> Result<PathBuf> {
    ensure_folders(device_root)?;
    let info = analyze_path(iso)?;
    let dir = device_root.join(info.media.as_str());
    let target = dir.join(iso_target_name(&info, if opts.as_zso { "zso" } else { "iso" }));
    if opts.as_zso {
        // progresso do ZSO é em blocos → converte para bytes p/ ticker
        compress_iso(iso, &target, progress)?;
    } else {
        copy_sequential(iso, &target, progress)?;
    }
    Ok(target)
}

#[derive(Debug, Clone)]
pub struct GameEntry {
    pub path: PathBuf,
    pub game_id: String,
    pub title: String,
    pub size: u64,
    pub media: Media,
    pub format: String, // "ISO" | "ZSO" | "UL"
    pub fragmentation: String,
}

fn frag_status(meta: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let blocks = meta.blocks() * 512;
        if meta.len() == 0 || blocks >= meta.len() * 99 / 100 {
            "Sequencial ✓".into()
        } else {
            "Verificar ⚠".into()
        }
    }
    #[cfg(not(unix))]
    {
        "N/A".into()
    }
}

/// Escaneia CD/ e DVD/ + ul.cfg do dispositivo.
pub fn scan_library(device_root: &Path) -> Result<Vec<GameEntry>> {
    let mut out = Vec::new();
    for sub in ["CD", "DVD"] {
        let dir = device_root.join(sub);
        if !dir.is_dir() {
            continue;
        }
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(Ps2Error::Io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e.to_ascii_lowercase().as_str(), "iso" | "zso"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        for path in files {
            let meta = fs::metadata(&path).map_err(Ps2Error::Io)?;
            let frag = frag_status(&meta);
            match analyze_path(&path) {
                Ok(info) => {
                    let title = sanitize_title(&info.volume_id);
                    let fmt = if path.extension().and_then(|e| e.to_str())
                        == Some("zso")
                    {
                        "ZSO"
                    } else {
                        "ISO"
                    };
                    out.push(GameEntry {
                        path: path.clone(),
                        game_id: info.game_id,
                        title: if title.is_empty() { "Unknown".into() } else { title },
                        size: meta.len(),
                        media: info.media,
                        format: fmt.into(),
                        fragmentation: frag,
                    });
                }
                Err(e) => {
                    eprintln!("⚠ {} : {}", path.display(), e);
                }
            }
        }
    }
    // jogos no formato USBExtreme (ul.cfg + partes ul.*)
    for rec in crate::opl::ulcfg::read_ulcfg(device_root)? {
        let media = if rec.media == crate::opl::ulcfg::MEDIA_DVD {
            Media::Dvd
        } else {
            Media::Cd
        };
        let dir = device_root.join(media.as_str());
        let crc = crate::opl::ulcfg::usba_crc32(&crate::opl::ulcfg::name_field(&rec.name));
        let mut size = 0u64;
        for part in 0..rec.parts {
            let name = crate::opl::ulcfg::part_filename(crc, &rec.startup, part);
            if let Ok(m) = fs::metadata(dir.join(&name)) {
                size += m.len();
            }
        }
        out.push(GameEntry {
            path: dir,
            game_id: rec.startup.clone(),
            title: if rec.name.is_empty() { rec.startup } else { rec.name },
            size,
            media,
            format: "UL".into(),
            fragmentation: format!("{} partes", rec.parts),
        });
    }
    out.sort_by(|a, b| a.game_id.cmp(&b.game_id));
    Ok(out)
}
