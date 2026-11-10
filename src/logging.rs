//! logging — log persistente em disco para diagnóstico pós-uso.
//!
//! * Arquivo: `~/.local/share/ps2-manager/logs/ps2-manager.log`
//!   (respeita `XDG_DATA_HOME`; rotaciona a 2 MB, mantém até 5 rotações).
//! * Zero dependências externas — timestamp civil calculado localmente.
//! * O log NUNCA impede o app de abrir: qualquer falha de I/O é silenciosa.
//!
//! Toda linha que aparece no painel Log da TUI também é escrita aqui,
//! além de eventos de CLI, downloads e panics — é a fonte para investigar
//! "o que deu errado" depois de usar o programa.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ROTATIONS: u32 = 5;

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Raiz de dados do app (`XDG_DATA_HOME` ou `~/.local/share`).
pub fn data_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|p| Path::new(p).is_absolute())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("/tmp/ps2-manager-data"));
    base.join("ps2-manager")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn reports_dir() -> PathBuf {
    data_dir().join("reports")
}

/// Caminho do log atual (para mostrar na UI).
#[must_use]
pub fn path() -> Option<&'static PathBuf> {
    PATH.get()
}

/// Inicializa o logger (idempotente). Silencioso por design.
pub fn init() {
    let _ = PATH.set(logs_dir().join("ps2-manager.log"));
    let cell = LOG.get_or_init(|| Mutex::new(None));
    {
        // escopo: o guard PRECISA morrer antes de write_raw/info (Mutex não
        // é reentrante — lock duplo no mesmo thread = deadlock no futex)
        let mut guard = match cell.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.is_some() {
            return;
        }
        let dir = logs_dir();
        let file = PATH.get().map(|p| p.as_path()).unwrap_or(Path::new("/dev/null"));
        let _ = std::fs::create_dir_all(&dir);
        rotate_if_needed(file);
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(file) {
            *guard = Some(f);
        }
    }
    write_raw("─".repeat(60));
    info(&format!(
        "ps2-manager v{} iniciado (pid {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));
}

fn rotate_if_needed(file: &Path) {
    let Ok(meta) = std::fs::metadata(file) else { return };
    if meta.len() < MAX_LOG_BYTES {
        return;
    }
    // empurra .4→.5, .3→.4 … e o atual → .1
    for i in (1..MAX_ROTATIONS).rev() {
        let from = file.with_extension(format!("log.{i}"));
        let to = file.with_extension(format!("log.{}", i + 1));
        if from.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
    let _ = std::fs::rename(file, file.with_extension("log.1"));
    // limpa rotações excedentes antigas
    let old = file.with_extension(format!("log.{MAX_ROTATIONS}"));
    let _ = std::fs::remove_file(old);
}

fn write_raw(line: String) {
    let Some(cell) = LOG.get() else { return };
    let Ok(mut guard) = cell.lock() else { return };
    if let Some(f) = guard.as_mut() {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

/// Escreve uma linha com nível no arquivo de log.
pub fn write(level: &str, msg: &str) {
    write_raw(format!("[{}] [{level}] {msg}", now_string()));
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn error(msg: &str) {
    write("ERRO", msg);
}

/// `YYYY-MM-DD HH:MM:SS` em UTC (sem dependências; suficiente p/ diagnóstico).
pub fn now_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Algoritmo civil (Howard Hinnant) — dias desde epoch → (ano, mês, dia).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_epoch_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
    }

    #[test]
    fn now_is_sane_length() {
        let s = now_string();
        assert_eq!(s.len(), 19);
        assert_eq!(&s[4..5], "-");
    }
}
