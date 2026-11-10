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
//! ```
//!
//! Cancelamento: checado a cada 1 MiB transferido — a mídia nunca fica com
//! arquivos parciais no nome final (o `.part` é removido no rollback).

use crate::core::staging::StagingBuffer;
use crate::core::storage::{self, FsClass};
use crate::error::{Ps2Error, Result};
use crate::iso::{parser, shrink, zso};
use crate::opl::library;
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
    pub elapsed: Duration,
}

/// Resumo de uma linha para CLI/TUI.
pub fn summary_line(rep: &InstallReport) -> String {
    format!(
        "{} → {} ({}{}) · fallocate {} · {:.1}s",
        crate::cli::fmt_size(rep.original_bytes),
        crate::cli::fmt_size(rep.written_bytes),
        if rep.is_zso { "ZSO " } else { "ISO" },
        if rep.shrink_saved > 0 {
            format!(" · shrink poupou {}", crate::cli::fmt_size(rep.shrink_saved))
        } else {
            String::new()
        },
        if rep.fallocate_used { "✓" } else { "n/d" },
        rep.elapsed.as_secs_f64()
    )
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

    // Pré-checagem FAT32: ISO pura acima de 4 GiB é impossível sem ZSO
    if !job.as_zso && job.target_fs.max_file() < total_src {
        return Err(Ps2Error::Other(format!(
            "FAT32 aceita no máximo {} por arquivo; a imagem tem {}. Ative a conversão ZSO \
             ou formate a mídia como exFAT.",
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
        let staged_len = fs::metadata(&src).map_err(Ps2Error::Io)?.len();
        if job.target_fs.max_file() < staged_len {
            return Err(Ps2Error::Other(
                "mesmo comprimido o arquivo excede 4 GiB (FAT32) — use uma mídia exFAT".into(),
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
