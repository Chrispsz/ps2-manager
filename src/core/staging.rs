//! Staging em SSD local (Mega Prompt 7) — a conversão ISO→ZSO acontece no
//! disco do PC e a mídia externa só é tocada quando o arquivo final está
//! pronto e validado.
//!
//! * Sessão isolada por diretório `<label>-<pid>-<nanos>`.
//! * Cleanup via `Drop` (RAII): erros/early-return limpam o staging.
//! * Encerramento abrupto (SIGKILL/energia): sessões com mais de 24 h são
//!   purgadas na próxima criação — a mídia externa permanece 100% intacta.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

pub struct StagingBuffer {
    root: PathBuf,
}

impl StagingBuffer {
    /// Cria a sessão de staging no disco local:
    /// `$PS2_MANAGER_STAGING` → `$XDG_CACHE_HOME/ps2-manager` →
    /// `$HOME/.cache/ps2-manager` → `/tmp/ps2-manager`.
    pub fn create(label: &str) -> std::io::Result<Self> {
        let base = staging_base();
        fs::create_dir_all(&base)?;
        purge_stale(&base);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = base.join(format!(
            "{}-{}-{}",
            sanitize_label(label),
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Caminho completo para um arquivo dentro da sessão.
    pub fn path(&self, file: &str) -> PathBuf {
        self.root.join(file)
    }

    /// Remove a sessão inteira (chamado pelo `Drop`).
    pub fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Drop for StagingBuffer {
    fn drop(&mut self) {
        self.cleanup();
    }
}

pub fn staging_base() -> PathBuf {
    if let Ok(p) = std::env::var("PS2_MANAGER_STAGING") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(p) = std::env::var("XDG_CACHE_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p).join("ps2-manager");
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h).join(".cache").join("ps2-manager");
        }
    }
    PathBuf::from("/tmp/ps2-manager")
}

fn sanitize_label(label: &str) -> String {
    let s: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if s.is_empty() {
        "ps2".to_string()
    } else {
        s
    }
}

fn purge_stale(base: &std::path::Path) {
    let Ok(rd) = fs::read_dir(base) else {
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|age| age > STALE_AFTER)
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_dir_all(&path);
        }
    }
}
