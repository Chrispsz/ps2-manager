//! Pipeline de instalação em 3 fases (Mega Prompt 7):
//!
//! ```text
//! [ 1. ISO original no PC ]
//!            │
//!            ▼
//! [ 2. Processamento em SSD local (staging) ]
//!    - Smart ISO Shrink (remove setores fantasma)
//!    - Compressão LZ4 multithread → ZSO
//!    - Validação de integridade (header + Game ID via decompressão)
//!            │  (somente após 100% de sucesso)
//!            ▼
//! [ 3. Gravação sequencial na mídia externa ]
//!    - fallocate (alocação contígua, best-effort)
//!    - streaming I/O 1 MiB + sync_all
//!    - arquivo `.part` renomeado só no final (rollback automático)
//!
//! Variante USBExtreme (o melhor legado possível): `split_ul` explícito ou
//! FAT32 com imagem >4 GiB → a imagem é fatiada em partes `ul.<crc>.<ID>.NN`
//! sequenciais (CD/ ou DVD/) + registro no `ul.cfg`. Rollback remove todas
//! as partes geradas; o `ul.cfg` só é escrito no final, com tudo íntegro.
//! ```
//!
//! Cancelamento: checado a cada 1 MiB transferido — a mídia nunca fica com
//! arquivos parciais no nome final (o `.part` é removido no rollback).

use crate::core::staging::StagingBuffer;
use crate::core::storage::{self, FsClass};
use crate::error::{Ps2Error, Result};
use crate::iso::{parser, shrink, zso};
use crate::opl::{library, ulcfg};
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseKind {
    Analyze,
    Shrink,
    Compress,
    Verify,
    Transfer,
    Done,
}

impl PhaseKind {
    pub fn label(self) -> &'static str {
        match self {
            PhaseKind::Analyze => "Analisando imagem",
            PhaseKind::Shrink => "Smart Shrink (SSD local)",
            PhaseKind::Compress => "Comprimindo ZSO (SSD local)",
            PhaseKind::Verify => "Validando integridade",
            PhaseKind::Transfer => "Gravando na mídia",
            PhaseKind::Done => "Concluído",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub phase: PhaseKind,
    pub done: u64,
    pub total: u64,
}

pub type ProgressFn<'a> = &'a (dyn Fn(Progress) + Send + Sync);

pub struct InstallJob {
    pub iso: PathBuf,
    /// Ponto de montagem da mídia externa.
    pub target_root: PathBuf,
    pub target_fs: FsClass,
    pub as_zso: bool,
    pub pre_shrink: bool,
    pub fallocate: bool,
    pub overwrite: bool,
    /// Formato USBExtreme explícito (ul.cfg + partes ul.*) — o melhor
    /// legado possível: compatibilidade com loaders antigos em qualquer
    /// filesystem. Em FAT32 também é escolhido automaticamente quando a
    /// imagem excede o limite de 4 GiB.
    pub split_ul: bool,
    /// Espaço livre conhecido no destino (statvfs), se disponível.
    pub free_bytes: Option<u64>,
}

#[derive(Debug)]
pub struct InstallReport {
    pub game_id: String,
    pub title: String,
    pub final_path: PathBuf,
    pub original_bytes: u64,
    pub written_bytes: u64,
    pub is_zso: bool,
    pub fallocate_used: bool,
    pub shrink_saved: u64,
    /// Número de partes ul.* geradas (0 = arquivo único ISO/ZSO).
    pub ul_parts: u8,
    pub elapsed: Duration,
}

/// Resumo de uma linha para CLI/TUI.
pub fn summary_line(rep: &InstallReport) -> String {
    let kind = if rep.ul_parts > 0 {
        format!("USBExtreme · {} parte(s)", rep.ul_parts)
    } else if rep.is_zso {
        "ZSO".to_string()
    } else {
        "ISO".to_string()
    };
    format!(
        "{} → {} ({}){} · fallocate {} · {:.1}s",
        crate::cli::fmt_size(rep.original_bytes),
        crate::cli::fmt_size(rep.written_bytes),
        kind,
        if rep.shrink_saved > 0 {
            format!(" · shrink poupou {}", crate::cli::fmt_size(rep.shrink_saved))
        } else {
            String::new()
        },
        if rep.fallocate_used { "✓" } else { "n/d" },
        rep.elapsed.as_secs_f64()
    )
}

/// Decide o formato de gravação: partes USBExtreme (o melhor legado
/// possível) ou arquivo único ISO/ZSO (o melhor moderno possível).
///
/// Regras:
/// - `split_ul` explícito → sempre partes (compatibilidade com loaders
///   USBExtreme antigos em qualquer filesystem);
/// - FAT32 + imagem crua acima do limite de 4 GiB → partes automáticas
///   (única forma de caber um DVD grande no FAT32);
/// - exFAT (padrão moderno) → arquivo único ISO/ZSO, sem fatiamento.
fn wants_usbextreme(job: &InstallJob, total_src: u64) -> bool {
    job.split_ul
        || (job.target_fs == FsClass::Fat32
            && !job.as_zso
            && job.target_fs.max_file() < total_src)
}

/// Tamanho de parte USBExtreme (1 GiB — mesmo padrão do rebuild `ulcfg`).
const UL_PART_SIZE: u64 = 1024 * 1024 * 1024;
const UL_CHUNK: usize = 1024 * 1024;

/// Fase 3 variante USBExtreme: fatia a imagem em `ul.<crc>.<ID>.NN`
/// sequenciais (CD/ ou DVD/ conforme a mídia) e registra no `ul.cfg`.
///
/// - Partes sequenciais evitam fragmentação dispendiosa no barramento
///   USB/SIO2 e permitem jogos >4 GiB em FAT32;
/// - rollback total: erro/cancelamento remove as partes geradas — o
///   `ul.cfg` só é (re)escrito no final, com todas as partes íntegras;
/// - sobrescrever remove sobras de partes antigas além do novo total.
#[allow(clippy::too_many_arguments)]
fn install_usbextreme(
    job: &InstallJob,
    src: &Path,
    info: &parser::GameInfo,
    progress: ProgressFn<'_>,
    cancel: &AtomicBool,
    total_src: u64,
    shrink_saved: u64,
    t0: Instant,
) -> Result<InstallReport> {
    library::ensure_folders(&job.target_root)?;
    let dir = job.target_root.join(info.media.as_str());

    // mesmo naming do `ulcfg::build_from_library` (dedupe por nome visível)
    let title = library::sanitize_title(&info.volume_id);
    let name = if title.is_empty() {
        "Unknown".to_string()
    } else {
        title.chars().take(ulcfg::NAME_MAX).collect()
    };

    let mut records = ulcfg::read_ulcfg(&job.target_root)?;
    let replacing = records.iter().any(|r| r.startup == info.game_id);
    if replacing && !job.overwrite {
        return Err(Ps2Error::Other(format!(
            "jogo já registrado em ul.cfg: {name} ({}) — ative sobrescrever para substituir",
            info.game_id
        )));
    }

    let crc = ulcfg::usba_crc32(&ulcfg::name_field(&name));
    let mut reader =
        BufReader::with_capacity(UL_CHUNK, fs::File::open(src).map_err(Ps2Error::Io)?);
    let mut chunk = vec![0u8; UL_CHUNK];
    let mut created: Vec<PathBuf> = Vec::new();
    let mut written = 0u64;
    let mut parts: u8 = 0;
    let mut fallocate_used = false;

    let written_parts = (|| -> Result<u8> {
        loop {
            check_cancel(cancel)?;
            let path = dir.join(ulcfg::part_filename(crc, &info.game_id, parts));
            let mut wf = fs::File::create(&path).map_err(Ps2Error::Io)?;
            created.push(path);
            if storage::fallocate_space(&wf, UL_PART_SIZE).unwrap_or(false) {
                fallocate_used = true;
            }
            let mut filled = 0u64;
            while filled < UL_PART_SIZE {
                let want = ((UL_PART_SIZE - filled) as usize).min(chunk.len());
                let n = reader.read(&mut chunk[..want]).map_err(Ps2Error::Io)?;
                if n == 0 {
                    break;
                }
                wf.write_all(&chunk[..n]).map_err(Ps2Error::Io)?;
                filled += n as u64;
                written += n as u64;
                progress(Progress {
                    phase: PhaseKind::Transfer,
                    done: written,
                    total: total_src,
                });
            }
            wf.set_len(filled).map_err(Ps2Error::Io)?;
            wf.sync_all().map_err(Ps2Error::Io)?;
            drop(wf);
            if filled == 0 {
                // imagem com tamanho múltiplo exato do tamanho de parte:
                // descarta a parte vazia e conclui
                if let Some(p) = created.pop() {
                    let _ = fs::remove_file(&p);
                }
                break;
            }
            parts = parts.wrapping_add(1);
            if parts == 0 {
                return Err(Ps2Error::Other(
                    "imagem excede 255 partes USBExtreme".into(),
                ));
            }
            if filled < UL_PART_SIZE {
                break;
            }
        }
        Ok(parts)
    })();

    let parts = match written_parts {
        Ok(p) => p,
        Err(e) => {
            // rollback: nenhuma parte parcial fica na mídia
            for p in &created {
                let _ = fs::remove_file(p);
            }
            return Err(e);
        }
    };

    // sobrescrever: remove sobras de partes antigas além do novo total
    for nn in (parts as u16)..=(u8::MAX as u16) {
        let leftover = dir.join(ulcfg::part_filename(crc, &info.game_id, nn as u8));
        if leftover.exists() {
            let _ = fs::remove_file(&leftover);
        }
    }

    records.retain(|r| r.startup != info.game_id);
    records.push(ulcfg::UlRecord {
        name,
        startup: info.game_id.clone(),
        parts,
        media: info.media.ul_byte(),
    });
    ulcfg::write_ulcfg(&job.target_root, &records)?;

    let final_path = dir.join(ulcfg::part_filename(crc, &info.game_id, 0));
    progress(Progress { phase: PhaseKind::Done, done: 1, total: 1 });

    Ok(InstallReport {
        game_id: info.game_id.clone(),
        title: info.volume_id.clone(),
        final_path,
        original_bytes: total_src,
        written_bytes: written,
        is_zso: false,
        fallocate_used,
        shrink_saved,
        ul_parts: parts,
        elapsed: t0.elapsed(),
    })
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Ps2Error::Other("cancelado pelo usuário".into()));
    }
    Ok(())
}

/// Instala uma ISO na mídia via staging em SSD local. Ver `staged_install`.
pub fn staged_install(job: &InstallJob, progress: ProgressFn<'_>, cancel: &AtomicBool) -> Result<InstallReport> {
    let t0 = Instant::now();
    check_cancel(cancel)?;

    // ── Fase 1: análise ────────────────────────────────────────────────
    let info = parser::analyze_path(&job.iso)?;
    let total_src = fs::metadata(&job.iso).map_err(Ps2Error::Io)?.len();
    progress(Progress { phase: PhaseKind::Analyze, done: 1, total: 1 });

    // Decisão de formato: USBExtreme (o melhor legado possível) ou arquivo
    // único ISO/ZSO (o melhor moderno possível).
    let split = wants_usbextreme(job, total_src);
    if split && job.as_zso {
        return Err(Ps2Error::Other(
            "formato USBExtreme armazena ISO crua em partes ul.* — não combina com ZSO; \
             desative a conversão ZSO ou use uma mídia exFAT (arquivo inteiro)"
                .into(),
        ));
    }
    // Pré-checagem FAT32: ISO pura acima de 4 GiB é impossível sem ZSO
    // (a menos que a imagem seja fatiada em partes ul.*)
    if !split && !job.as_zso && job.target_fs.max_file() < total_src {
        return Err(Ps2Error::Other(format!(
            "FAT32 aceita no máximo {} por arquivo; a imagem tem {}. Ative a conversão ZSO, \
             o formato USBExtreme (partes ul.*) ou formate a mídia como exFAT.",
            crate::cli::fmt_size(job.target_fs.max_file()),
            crate::cli::fmt_size(total_src)
        )));
    }

    library::ensure_folders(&job.target_root)?;
    let dir = job.target_root.join(info.media.as_str());
    let final_ext = if job.as_zso { "zso" } else { "iso" };
    let name = library::iso_target_name(&info, final_ext);
    let dest = dir.join(&name);
    if dest.exists() && !job.overwrite {
        return Err(Ps2Error::Other(format!(
            "destino já existe: {} — remova-o ou ative sobrescrever",
            dest.display()
        )));
    }

    // ── Fase 2: processamento no SSD local (staging) ───────────────────
    let mut shrink_saved = 0u64;
    let (staged_file, _staging) = if job.as_zso || job.pre_shrink {
        let sb = StagingBuffer::create("install").map_err(Ps2Error::Io)?;
        let mut src: PathBuf = job.iso.clone();

        if job.pre_shrink {
            progress(Progress { phase: PhaseKind::Shrink, done: 0, total: 0 });
            let shrunk = sb.path("shrunk.iso");
            let rep = shrink::shrink_iso(&job.iso, &shrunk)?;
            shrink_saved = rep.saved_bytes();
            src = shrunk;
            progress(Progress { phase: PhaseKind::Shrink, done: 1, total: 1 });
            check_cancel(cancel)?;
        }

        if job.as_zso {
            progress(Progress { phase: PhaseKind::Compress, done: 0, total: total_src });
            let out = sb.path("staged.zso");
            let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let stop = std::sync::Arc::new(AtomicBool::new(false));
            let res = std::thread::scope(|s| {
                let c2 = counter.clone();
                let st2 = stop.clone();
                s.spawn(move || {
                    while !st2.load(Ordering::Relaxed) {
                        progress(Progress {
                            phase: PhaseKind::Compress,
                            done: c2.load(Ordering::Relaxed),
                            total: total_src,
                        });
                        std::thread::sleep(Duration::from_millis(200));
                    }
                });
                let r = zso::compress_iso(&src, &out, Some(counter));
                stop.store(true, Ordering::Relaxed);
                r
            });
            let (_comp, _orig) = res?;
            src = out;
            progress(Progress { phase: PhaseKind::Compress, done: total_src, total: total_src });
            check_cancel(cancel)?;
        }

        // Validação: header ZSO/ISO + releitura do SYSTEM.CNF (decomprime
        // blocos reais) — o Game ID precisa bater com a imagem original.
        progress(Progress { phase: PhaseKind::Verify, done: 0, total: 0 });
        let vinfo = parser::analyze_path(&src)?;
        if vinfo.game_id != info.game_id {
            return Err(Ps2Error::Other(
                "verificação falhou: Game ID divergiu após a conversão".into(),
            ));
        }
        progress(Progress { phase: PhaseKind::Verify, done: 1, total: 1 });

        // Pós-checagem FAT32: até o ZSO comprimido precisa caber em 4 GiB
        // (a variante USBExtreme grava em partes — não se aplica)
        let staged_len = fs::metadata(&src).map_err(Ps2Error::Io)?.len();
        if !split && job.target_fs.max_file() < staged_len {
            return Err(Ps2Error::Other(
                "mesmo comprimido o arquivo excede 4 GiB (FAT32) — instale como USBExtreme \
                 (partes ul.*, dados crus) ou use uma mídia exFAT"
                    .into(),
            ));
        }
        if let Some(free) = job.free_bytes {
            storage::ensure_space(free, staged_len)?;
        }
        check_cancel(cancel)?;
        (src, Some(sb))
    } else {
        if let Some(free) = job.free_bytes {
            storage::ensure_space(free, total_src)?;
        }
        (job.iso.clone(), None)
    };

    // ── Fase 3-UL (USBExtreme): fatiamento em partes + ul.cfg ──────────
    if split {
        return install_usbextreme(
            job,
            &staged_file,
            &info,
            progress,
            cancel,
            total_src,
            shrink_saved,
            t0,
        );
    }

    // ── Fase 3: gravação validada na mídia ─────────────────────────────
    let dest_part = dir.join(format!("{name}.part"));
    let xfer = transfer_file(&staged_file, &dest_part, job.fallocate, progress, cancel);
    let (written, fallocate_used) = match xfer {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&dest_part); // rollback: nada parcial fica
            return Err(e);
        }
    };
    fs::rename(&dest_part, &dest).map_err(Ps2Error::Io)?;
    progress(Progress { phase: PhaseKind::Done, done: 1, total: 1 });

    Ok(InstallReport {
        game_id: info.game_id,
        title: info.volume_id,
        final_path: dest,
        original_bytes: total_src,
        written_bytes: written,
        is_zso: job.as_zso,
        fallocate_used,
        shrink_saved,
        ul_parts: 0,
        elapsed: t0.elapsed(),
    })
}

/// Streaming I/O com fallocate best-effort, progresso por chunk,
/// cancelamento a cada 1 MiB e `sync_all` antes de retornar.
fn transfer_file(
    src: &Path,
    dest_part: &Path,
    fallocate: bool,
    progress: ProgressFn<'_>,
    cancel: &AtomicBool,
) -> Result<(u64, bool)> {
    let src_len = fs::metadata(src).map_err(Ps2Error::Io)?.len();
    let rf = fs::File::open(src).map_err(Ps2Error::Io)?;
    let wf = fs::File::create(dest_part).map_err(Ps2Error::Io)?;

    let mut fallocate_used = false;
    if fallocate && src_len > 0 && storage::fallocate_space(&wf, src_len).unwrap_or(false) {
        fallocate_used = true;
    }

    let mut r = BufReader::with_capacity(1024 * 1024, rf);
    let mut w = BufWriter::with_capacity(1024 * 1024, wf);
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Ps2Error::Other("cancelado pelo usuário".into()));
        }
        let n = r.read(&mut buf).map_err(Ps2Error::Io)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(Ps2Error::Io)?;
        done += n as u64;
        progress(Progress { phase: PhaseKind::Transfer, done, total: src_len });
    }
    w.flush().map_err(Ps2Error::Io)?;
    let f = w
        .into_inner()
        .map_err(|e| Ps2Error::Other(e.to_string()))?;
    f.sync_all().map_err(Ps2Error::Io)?; // durabilidade p/ remoção segura

    if done != src_len {
        return Err(Ps2Error::Other("cópia incompleta — verifique a mídia".into()));
    }
    Ok((done, fallocate_used))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(fs: FsClass, as_zso: bool, split_ul: bool, size: u64) -> (InstallJob, u64) {
        (
            InstallJob {
                iso: PathBuf::new(),
                target_root: PathBuf::new(),
                target_fs: fs,
                as_zso,
                pre_shrink: false,
                fallocate: true,
                overwrite: false,
                split_ul,
                free_bytes: None,
            },
            size,
        )
    }

    #[test]
    fn decisao_usbextreme() {
        let big = 5 * 1024 * 1024 * 1024u64; // 5 GiB
        // exFAT (moderno): arquivo único, sempre
        let (j, s) = job(FsClass::Exfat, false, false, big);
        assert!(!wants_usbextreme(&j, s));
        // FAT32 + ISO crua >4 GiB: fatiamento automático (o melhor legado)
        let (j, s) = job(FsClass::Fat32, false, false, big);
        assert!(wants_usbextreme(&j, s));
        // FAT32 + ZSO: segue o caminho de compressão (pós-checagem decide)
        let (j, s) = job(FsClass::Fat32, true, false, big);
        assert!(!wants_usbextreme(&j, s));
        // seleção explícita vence em qualquer FS, com qualquer tamanho
        let (j, s) = job(FsClass::Exfat, false, true, 1024);
        assert!(wants_usbextreme(&j, s));
        // FAT32 com imagem pequena: arquivo único
        let (j, s) = job(FsClass::Fat32, false, false, 1024);
        assert!(!wants_usbextreme(&j, s));
    }

    #[test]
    fn usbextreme_end_to_end() {
        let tmp = std::env::temp_dir().join(format!("ps2m-ul-t{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let iso = tmp.join("teste.iso");
        crate::diag::build_synthetic_iso(&iso).unwrap();
        let total = fs::metadata(&iso).unwrap().len();

        let media = tmp.join("midia");
        fs::create_dir_all(&media).unwrap();
        let j = InstallJob {
            iso: iso.clone(),
            target_root: media.clone(),
            target_fs: FsClass::Fat32,
            as_zso: false,
            pre_shrink: false,
            fallocate: true,
            overwrite: false,
            split_ul: true,
            free_bytes: None,
        };
        let cancel = AtomicBool::new(false);
        let rep = staged_install(&j, &|_| {}, &cancel).unwrap();

        assert_eq!(rep.ul_parts, 1);
        assert!(!rep.is_zso);
        assert_eq!(rep.written_bytes, total);

        let info = parser::analyze_path(&iso).unwrap();
        let recs = crate::opl::ulcfg::read_ulcfg(&media).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].startup, info.game_id);
        assert_eq!(recs[0].parts, 1);
        assert_eq!(
            recs[0].media,
            crate::opl::ulcfg::MEDIA_CD
        );

        let dir = media.join(info.media.as_str());
        let crc = crate::opl::ulcfg::usba_crc32(&crate::opl::ulcfg::name_field(
            &crate::opl::library::sanitize_title(&info.volume_id),
        ));
        let part = dir.join(crate::opl::ulcfg::part_filename(crc, &info.game_id, 0));
        assert!(part.exists());
        assert_eq!(fs::metadata(&part).unwrap().len(), total);

        // reinstalar sem sobrescrever falha com mensagem clara
        let err = staged_install(&j, &|_| {}, &cancel).unwrap_err().to_string();
        assert!(err.contains("ul.cfg"));
        // com sobrescrever: substitui sem duplicar registro
        let j2 = InstallJob { overwrite: true, ..j };
        let rep2 = staged_install(&j2, &|_| {}, &cancel).unwrap();
        assert_eq!(rep2.ul_parts, 1);
        assert_eq!(crate::opl::ulcfg::read_ulcfg(&media).unwrap().len(), 1);

        let _ = fs::remove_dir_all(&tmp);
    }
}
