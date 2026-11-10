//! diag — Diagnóstico completo do ecossistema ("botão de teste").
//!
//! Exercita o fluxo inteiro do PS2 Manager numa sandbox temporária:
//!
//!   1. Ambiente        — versão, SO, arquitetura, diretórios de dados
//!   2. Mídias externas — scan de dispositivos (só lista; nunca grava nelas)
//!   3. Mídia de teste  — ISO/ZSO real na pasta do AppImage/executável;
//!      sem uma, gera um disco sintético interno
//!   4. Parser ISO9660  — Game ID, título, mídia (mountless)
//!   5. Instalação      — pipeline completo staging SSD → destino (ZSO)
//!   6. Integração OPL  — scan da biblioteca, ul.cfg, CFG/<ID>.cfg
//!   7. Capas           — download da capa do jogo (rede)
//!   8. GitHub          — releases do OPL-Mod-PTBR alcançáveis (rede)
//!   9. Relatório       — TXT em ~/.local/share/ps2-manager/reports/
//!
//! Tudo é gravado APENAS em diretório temporário — mídias reais nunca são
//! tocadas. Cada passo é cronometrado e vira linha no relatório + log.

use crate::deploy::github::GitHubClient;
use crate::iso::parser;
use crate::opl::{cfg, library, ulcfg};
use crate::scrape::artwork;
use crate::{core, logging};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    pub fn icon(self) -> &'static str {
        match self {
            Status::Pass => "✓",
            Status::Warn => "⚠",
            Status::Fail => "✗",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASSA",
            Status::Warn => "AVISO",
            Status::Fail => "FALHA",
        }
    }

    fn worst(a: Status, b: Status) -> Status {
        match (a, b) {
            (Status::Fail, _) | (_, Status::Fail) => Status::Fail,
            (Status::Warn, _) | (_, Status::Warn) => Status::Warn,
            _ => Status::Pass,
        }
    }
}

#[derive(Debug)]
pub struct Step {
    pub name: String,
    pub status: Status,
    pub detail: String,
    pub ms: u128,
}

#[derive(Debug)]
pub struct Report {
    pub steps: Vec<Step>,
    pub overall: Status,
    pub total_ms: u128,
    pub report_path: Option<PathBuf>,
}

/// Diretório onde o diagnóstico procura uma ISO/ZSO real:
/// 1) pasta do AppImage (env APPIMAGE), 2) pasta do executável, 3) cwd.
pub fn default_scan_dir() -> Option<PathBuf> {
    if let Ok(ai) = std::env::var("APPIMAGE") {
        if let Some(p) = Path::new(&ai).parent() {
            return Some(p.to_path_buf());
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            return Some(p.to_path_buf());
        }
    }
    std::env::current_dir().ok()
}

// ─────────────────────────── ISO sintética ─────────────────────────────

/// Gera uma mini-ISO9660 válida (40 setores, ~80 KB) com SYSTEM.CNF apontando
/// para o Game ID `SCUS_973.99` — permite testar parser/ZSO/instalação mesmo
/// sem nenhuma ISO real à mão.
pub fn build_synthetic_iso(path: &Path) -> std::io::Result<()> {
    const SECTOR: usize = 2048;
    const SECTORS: usize = 40;
    let mut img = vec![0u8; SECTORS * SECTOR];

    fn put(buf: &mut [u8], sec: usize, off: usize, bytes: &[u8]) {
        let start = sec * SECTOR + off;
        buf[start..start + bytes.len()].copy_from_slice(bytes);
    }
    let le = |v: u32| v.to_le_bytes();
    let be = |v: u32| v.to_be_bytes();

    // PVD (setor 16) — volume id, path tables (falsas, nunca lidas) e root record
    let pvd = 16;
    put(&mut img, pvd, 0, &[1]);
    put(&mut img, pvd, 1, b"CD001");
    put(&mut img, pvd, 40, format!("{:<32}", "PS2M DIAG TEST").as_bytes());
    put(&mut img, pvd, 100, &le(10));
    put(&mut img, pvd, 108, &le(17));
    put(&mut img, pvd, 116, &be(17));
    put(&mut img, pvd, 124, &[34]); // comprimento do record do root
    put(&mut img, pvd, 126, &le(18)); // LBA do extent do dir raiz
    put(&mut img, pvd, 134, &le(2048));
    put(&mut img, pvd, 149, &[0x02]); // flags: diretório
    put(&mut img, pvd, 156, &[1]);
    put(&mut img, pvd, 157, &[0]); // nome "."

    // extent do dir raiz (setor 18): ".", ".." e SYSTEM.CNF
    const ROOT: usize = 18;
    put(&mut img, ROOT, 0, &[34]);
    put(&mut img, ROOT, 2, &le(18));
    put(&mut img, ROOT, 10, &le(2048));
    put(&mut img, ROOT, 25, &[0x02]);
    put(&mut img, ROOT, 32, &[1]);
    put(&mut img, ROOT, 33, &[0]);
    put(&mut img, ROOT, 34, &[34]);
    put(&mut img, ROOT, 36, &le(18));
    put(&mut img, ROOT, 44, &le(2048));
    put(&mut img, ROOT, 59, &[0x02]);
    put(&mut img, ROOT, 66, &[1]);
    put(&mut img, ROOT, 67, &[1]);
    let cnf = b"BOOT2 = cdrom0:\\SCUS_973.99;1\r\nVER = 1.00\r\nVMODE = NTSC\r\n";
    put(&mut img, ROOT, 68, &[43]); // 33 + 10
    put(&mut img, ROOT, 70, &le(19));
    put(&mut img, ROOT, 78, &le(cnf.len() as u32));
    put(&mut img, ROOT, 93, &[0]); // flags: arquivo
    put(&mut img, ROOT, 100, &[10]);
    put(&mut img, ROOT, 101, b"SYSTEM.CNF");

    // conteúdo do SYSTEM.CNF (setor 19)
    put(&mut img, 19, 0, cnf);

    // "dados de jogo" (setores 20..40) — padrão repetido comprime bem no LZ4
    let mut data = [0u8; SECTOR];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i * 31 + 7) % 251) as u8;
    }
    for s in 20..SECTORS {
        put(&mut img, s, 0, &data);
    }

    std::fs::write(path, &img)
}

// ────────────────────────────── execução ───────────────────────────────

fn find_test_media(dirs: &[PathBuf]) -> Option<PathBuf> {
    const EXTS: [&str; 4] = ["iso", "zso", "bin", "img"];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut hits: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.path().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|x| x.to_string_lossy().to_ascii_lowercase())
                    .is_some_and(|x| EXTS.contains(&x.as_str()))
            })
            .map(|e| e.path())
            .collect();
        hits.sort();
        if let Some(p) = hits.into_iter().next() {
            return Some(p);
        }
    }
    None
}

/// Roda o diagnóstico completo. `on_log` recebe linhas em tempo real
/// (painel Log da TUI / stdout do CLI).
pub async fn run_full(
    scan_dir: Option<&Path>,
    offline: bool,
    on_log: Arc<dyn Fn(&str) + Send + Sync>,
) -> Report {
    let t0 = Instant::now();
    let mut steps: Vec<Step> = Vec::new();
    let tmp = std::env::temp_dir().join(format!("ps2m-diag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);

    macro_rules! push {
        ($name:expr, $status:expr, $detail:expr, $t:expr) => {{
            let ms = $t.elapsed().as_millis();
            let st = $status;
            let detail: String = $detail.to_string();
            logging::write(
                match st {
                    Status::Pass => "INFO",
                    Status::Warn => "WARN",
                    Status::Fail => "ERRO",
                },
                &format!("diag {}: {} — {}", $name, st.label(), detail),
            );
            on_log(&format!("{} {}: {} ({}ms)", st.icon(), $name, detail, ms));
            steps.push(Step { name: $name.to_string(), status: st, detail, ms });
        }};
    }

    // ── 1. Ambiente ─────────────────────────────────────────────────────
    let t = Instant::now();
    on_log("▸ Ambiente…");
    {
        let os = std::fs::read_to_string("/proc/sys/kernel/ostype")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "desconhecido".into());
        let rel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let logs_ok = std::fs::create_dir_all(logging::logs_dir()).is_ok();
        let rep_ok = std::fs::create_dir_all(logging::reports_dir()).is_ok();
        let probe = logging::data_dir().join(".diag-probe");
        let write_ok = std::fs::write(&probe, b"ok").is_ok();
        let _ = std::fs::remove_file(&probe);
        if logs_ok && rep_ok && write_ok {
            push!(
                "Ambiente",
                Status::Pass,
                format!("v{} · {os} {rel} · {} · dados escritíveis ✓", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH),
                t
            );
        } else {
            push!(
                "Ambiente",
                Status::Fail,
                "diretórios de dados/logs não são escritíveis",
                t
            );
        }
    }

    // ── 2. Mídias externas ──────────────────────────────────────────────
    let t = Instant::now();
    on_log("▸ Mídias externas…");
    {
        let devs = core::storage::scan_target_devices();
        if devs.is_empty() {
            push!(
                "Mídias externas",
                Status::Warn,
                "nenhuma mídia externa conectada — testando com diretório temporário",
                t
            );
        } else {
            // exFAT é o padrão moderno (OPL 1.2.0+ lê exFAT em todo BDM);
            // a classe do FS entra no detalhe para dar visibilidade no
            // relatório — FAT32 só é relevante como legado (ul.cfg).
            let names: Vec<String> = devs
                .iter()
                .map(|d| {
                    let fs = d.mounts.first().map(|m| {
                        match core::storage::FsClass::from_fs(&m.fs_type) {
                            core::storage::FsClass::Exfat => format!(
                                ", exFAT — {} livres",
                                crate::cli::fmt_size(m.free_bytes)
                            ),
                            core::storage::FsClass::Fat32 => {
                                ", FAT32 (legacy · máx 4 GiB)".to_string()
                            }
                            other => format!(", {}", other.label()),
                        }
                    });
                    format!("{}{}", d.display_name(), fs.unwrap_or_default())
                })
                .collect();
            push!(
                "Mídias externas",
                Status::Pass,
                format!("{} detectada(s): {}", devs.len(), names.join(", ")),
                t
            );
        }
    }

    // ── 3. Mídia de teste (real ou sintética) ───────────────────────────
    let t = Instant::now();
    on_log("▸ Mídia de teste…");
    let media = {
        let mut cands: Vec<PathBuf> = Vec::new();
        if let Some(d) = scan_dir {
            cands.push(d.to_path_buf());
        }
        if let Some(d) = default_scan_dir() {
            cands.push(d);
        }
        match find_test_media(&cands) {
            Some(p) => {
                let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                push!(
                    "Mídia de teste",
                    Status::Pass,
                    format!("ISO real encontrada: {} ({})", p.display(), crate::cli::fmt_size(sz)),
                    t
                );
                p
            }
            None => {
                let p = tmp.join("sintetico.iso");
                match build_synthetic_iso(&p) {
                    Ok(()) => {
                        push!(
                            "Mídia de teste",
                            Status::Warn,
                            "nenhuma ISO/ZSO na pasta do executável — gerado disco sintético interno (80 KB)",
                            t
                        );
                        p
                    }
                    Err(e) => {
                        push!("Mídia de teste", Status::Fail, format!("falha ao gerar sintética: {e}"), t);
                        return finish(steps, t0, on_log.clone());
                    }
                }
            }
        }
    };

    // ── 4. Parser ISO9660 ───────────────────────────────────────────────
    let t = Instant::now();
    on_log("▸ Parser ISO9660…");
    let info = match parser::analyze_path(&media) {
        Ok(i) => {
            push!(
                "Parser ISO9660",
                Status::Pass,
                format!(
                    "{} · \"{}\" · {} · {} setores",
                    i.game_id,
                    i.volume_id,
                    i.media.as_str(),
                    i.total_sectors
                ),
                t
            );
            i
        }
        Err(e) => {
            push!("Parser ISO9660", Status::Fail, e.to_string(), t);
            return finish(steps, t0, on_log.clone());
        }
    };

    // ── 5. Instalação (staging SSD → destino temporário, com ZSO) ───────
    let t = Instant::now();
    on_log("▸ Instalação (staging → destino, ZSO)…");
    let inst = {
        let job = core::pipeline::InstallJob {
            iso: media.clone(),
            target_root: tmp.clone(),
            target_fs: core::storage::fs_class_for_root(&tmp),
            as_zso: true,
            pre_shrink: false,
            fallocate: true,
            overwrite: false,
            split_ul: false,
            free_bytes: core::storage::free_bytes(&tmp),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let log2 = on_log.clone();
        let res = tokio::task::spawn_blocking(move || {
            core::pipeline::staged_install(&job, &|p| {
                if p.phase == core::pipeline::PhaseKind::Compress && p.total > 0 {
                    let pct = p.done.min(p.total) as f64 / p.total as f64 * 100.0;
                    if pct > 0.0 && ((pct * 10.0) as u64).is_multiple_of(25) {
                        log2(&format!("  ZSO {pct:.0}%"));
                    }
                }
            }, &cancel)
        })
        .await;
        match res {
            Ok(Ok(rep)) => {
                let ganho = if rep.original_bytes > 0 {
                    format!(" · {:.1}% do original", rep.written_bytes as f64 / rep.original_bytes as f64 * 100.0)
                } else {
                    String::new()
                };
                push!(
                    "Instalação",
                    Status::Pass,
                    format!(
                        "{} → {}{} · fallocate {} · {:.1}s",
                        crate::cli::fmt_size(rep.original_bytes),
                        crate::cli::fmt_size(rep.written_bytes),
                        ganho,
                        if rep.fallocate_used { "✓" } else { "n/d" },
                        rep.elapsed.as_secs_f64()
                    ),
                    t
                );
                Some(rep)
            }
            Ok(Err(e)) => {
                push!("Instalação", Status::Fail, e.to_string(), t);
                None
            }
            Err(e) => {
                push!("Instalação", Status::Fail, format!("task panica: {e}"), t);
                None
            }
        }
    };

    // ── 6. Integração OPL (scan, ul.cfg USBExtreme, CFG) ────────────────
    let t = Instant::now();
    on_log("▸ Integração OPL (scan/ul.cfg/CFG)…");
    if inst.is_some() {
        // 6a. o jogo instalado (ZSO) aparece no scan da biblioteca
        let scan = library::scan_library(&tmp);
        let found = scan
            .as_ref()
            .map(|g| g.iter().any(|e| e.game_id == info.game_id))
            .unwrap_or(false);
        // 6b. maquinaria ul.cfg/USBExtreme: ISO sintética dedicada em pasta
        //     própria — valida CRC32 USBAdvance, partes ul.* e o próprio ul.cfg
        let ul_dir = tmp.join("ul-test");
        let _ = std::fs::create_dir_all(ul_dir.join("CD"));
        let syn_iso = ul_dir
            .join("CD")
            .join(format!("{}.iso", info.game_id));
        let ul_built = build_synthetic_iso(&syn_iso).is_ok() && ulcfg::build_from_library(&ul_dir, 1024, None).is_ok();
        let ulcfg_ok = ul_dir.join("ul.cfg").metadata().map(|m| m.len() > 0).unwrap_or(false);
        let parts_ok = std::fs::read_dir(ul_dir.join("CD"))
            .map(|rd| {
                rd.flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with("ul.") && n != "ul.cfg"
                })
            })
            .unwrap_or(false);
        // 6c. CFG/<ID>.cfg para o jogo instalado
        let cfgp = cfg::write_game_cfg(&tmp, &info, None);
        let cfg_ok = cfgp.is_ok();
        if found && ul_built && ulcfg_ok && parts_ok && cfg_ok {
            let ulsz = ul_dir.join("ul.cfg").metadata().map(|m| m.len()).unwrap_or(0);
            push!(
                "Integração OPL",
                Status::Pass,
                format!(
                    "scan ✓ ({}) · ul.cfg {} B + parte ul.* ✓ (CRC32 USBAdvance) · CFG/{}.cfg ✓",
                    scan.map(|g| format!("{} jogo(s)", g.len())).unwrap_or_default(),
                    ulsz,
                    info.game_id
                ),
                t
            );
        } else {
            push!(
                "Integração OPL",
                Status::Fail,
                format!(
                    "scan: {} · ul.cfg: {} · partes: {} · cfg: {}",
                    if found { "✓" } else { "✗" },
                    if ulcfg_ok { "✓" } else { "✗" },
                    if parts_ok { "✓" } else { "✗" },
                    if cfg_ok { "✓" } else { "✗" }
                ),
                t
            );
        }
    } else {
        push!("Integração OPL", Status::Fail, "pulado — instalação falhou", t);
    }

    // ── 7. Capas (rede) ─────────────────────────────────────────────────
    let t = Instant::now();
    on_log("▸ Capas (rede)…");
    if offline {
        push!("Capas", Status::Warn, "pulado (--offline)", t);
    } else {
        let entries = library::scan_library(&tmp).unwrap_or_default();
        match artwork::fetch_covers(&tmp, &entries, 1, &[], std::env::var("GITHUB_TOKEN").ok()).await {
            Ok((ok, _)) if ok > 0 => {
                let art = tmp.join("ART");
                let cov = std::fs::read_dir(&art)
                    .ok()
                    .and_then(|rd| rd.flatten().next())
                    .map(|e| e.path());
                let sz = cov.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0);
                push!(
                    "Capas",
                    Status::Pass,
                    format!("capa baixada ({}) para {}", crate::cli::fmt_size(sz), info.game_id),
                    t
                );
            }
            Ok((ok, fail)) => {
                push!(
                    "Capas",
                    Status::Warn,
                    format!("base sem capa para {} ou rede limitada (ok {ok}, falhas {fail})", info.game_id),
                    t
                );
            }
            Err(e) => {
                push!("Capas", Status::Warn, format!("rede indisponível: {e}"), t);
            }
        }
    }

    // ── 8. GitHub (releases do OPL-Mod-PTBR) ────────────────────────────
    let t = Instant::now();
    on_log("▸ GitHub (OPL-Mod-PTBR)…");
    if offline {
        push!("GitHub", Status::Warn, "pulado (--offline)", t);
    } else {
        match GitHubClient::new(std::env::var("GITHUB_TOKEN").ok()) {
            Ok(client) => match client.list_releases("Chrispsz/OPL-Mod-PTBR", 1).await {
                Ok(rels) if !rels.is_empty() => {
                    let r = &rels[0];
                    push!(
                        "GitHub",
                        Status::Pass,
                        format!("release mais recente do Mod-Kit: {} ({} assets)", r.tag_name, r.assets.len()),
                        t
                    );
                }
                Ok(_) => push!("GitHub", Status::Warn, "repositório sem releases", t),
                Err(e) => push!("GitHub", Status::Warn, format!("API indisponível (rate-limit?): {e}"), t),
            },
            Err(e) => push!("GitHub", Status::Warn, format!("cliente: {e}"), t),
        }
    }

    // ── 9. Relatório + limpeza ──────────────────────────────────────────
    let t = Instant::now();
    on_log("▸ Relatório…");
    let overall = steps.iter().fold(Status::Pass, |acc, s| Status::worst(acc, s.status));
    let path = write_report(&steps, overall, t0.elapsed().as_millis());
    match &path {
        Some(p) => push!("Relatório", Status::Pass, p.display().to_string(), t),
        None => push!("Relatório", Status::Fail, "não foi possível gravar o relatório", t),
    }
    let _ = std::fs::remove_dir_all(&tmp);
    on_log("✓ limpeza: temporários removidos (nenhuma mídia real foi tocada)");

    finish(steps, t0, on_log)
}

fn finish(steps: Vec<Step>, t0: Instant, on_log: Arc<dyn Fn(&str) + Send + Sync>) -> Report {
    let overall = steps.iter().fold(Status::Pass, |acc, s| Status::worst(acc, s.status));
    let total = t0.elapsed().as_millis();
    on_log(&format!(
        "diagnóstico concluído: {} ({:.1}s)",
        overall.label(),
        total as f64 / 1000.0
    ));
    Report { steps, overall, total_ms: total, report_path: None }
}

fn write_report(steps: &[Step], overall: Status, total_ms: u128) -> Option<PathBuf> {
    let dir = logging::reports_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = logging::now_string().replace([' ', ':'], "-");
    let path = dir.join(format!("diagnostico-{stamp}.txt"));
    let mut out = String::new();
    out.push_str("PS2 MANAGER — RELATÓRIO DE DIAGNÓSTICO\n");
    out.push_str(&format!(
        "versão {} · {} {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    out.push_str(&format!(
        "veredito: {} {} · {:.1}s\n\n",
        overall.icon(),
        overall.label(),
        total_ms as f64 / 1000.0
    ));
    for (i, s) in steps.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} {} ({}ms)\n     {}\n",
            i + 1,
            s.status.icon(),
            s.name,
            s.ms,
            s.detail
        ));
    }
    out.push_str("\nmídias reais nunca são tocadas — tudo em diretório temporário.\n");
    std::fs::write(&path, out).ok()?;
    Some(path)
}

/// Resumo em texto para o modal da TUI / stdout do CLI.
pub fn summary_text(rep: &Report) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Diagnóstico completo — veredito: {} {}\nduração: {:.1}s\n\n",
        rep.overall.icon(),
        rep.overall.label(),
        rep.total_ms as f64 / 1000.0
    ));
    for st in &rep.steps {
        s.push_str(&format!("{} {} — {}\n", st.status.icon(), st.name, st.detail));
    }
    if let Some(p) = &rep.report_path {
        s.push_str(&format!("\nrelatório gravado em:\n{}", p.display()));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_iso_is_valid() {
        let dir = std::env::temp_dir().join(format!("ps2m-diag-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let iso = dir.join("sintetico.iso");
        build_synthetic_iso(&iso).unwrap();
        let info = parser::analyze_path(&iso).expect("parser deve aceitar a ISO sintética");
        assert_eq!(info.game_id, "SCUS_973.99");
        assert_eq!(info.volume_id, "PS2M DIAG TEST");
        assert_eq!(info.media.as_str(), "CD");
        assert_eq!(info.total_sectors, 40);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worst_ordering() {
        assert_eq!(Status::worst(Status::Pass, Status::Pass), Status::Pass);
        assert_eq!(Status::worst(Status::Pass, Status::Warn), Status::Warn);
        assert_eq!(Status::worst(Status::Warn, Status::Fail), Status::Fail);
    }
}
