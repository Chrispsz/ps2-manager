//! CLI — definição dos subcomandos e execução.

use crate::core::storage;
use crate::deploy::bootkit;
use crate::deploy::fmcb;
use crate::deploy::github::GitHubClient;
use crate::iso::{parser, shrink, zso};
use crate::opl::{cfg, library, ulcfg};
use crate::scrape::{artwork, cheats};
use crate::tui;
use crate::vmc;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "ps2-manager",
    version,
    about = "Gerenciador nativo Linux de coleções PS2/OPL — ISO9660, ZSO, VMC, ul.cfg, deploy OPL-Mod-PTBR",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    /// Token GitHub (alternativa à env GITHUB_TOKEN; nunca é gravado em disco)
    #[arg(long, global = true)]
    pub token: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Analisa ISO/ZSO sem montar: Game ID (SYSTEM.CNF), título, mídia
    Info { path: PathBuf },
    /// Escaneia a biblioteca OPL (CD/, DVD/ e ul.cfg)
    Scan {
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Instala ISO na estrutura OPL (staging em SSD → fallocate na mídia)
    Install {
        path: PathBuf,
        device_root: PathBuf,
        /// converte para ZSO durante a instalação
        #[arg(long)]
        zso: bool,
        /// gravação direta na mídia, sem staging (legado)
        #[arg(long)]
        direct: bool,
    },
    /// Converte ISO → ZSO (LZ4 multithread)
    Zso {
        input: PathBuf,
        output: PathBuf,
    },
    /// Converte ZSO → ISO
    Unzso {
        input: PathBuf,
        output: PathBuf,
    },
    /// Smart ISO Shrink — remove setores vazios preservando LBA lógico
    Shrink {
        input: PathBuf,
        output: PathBuf,
    },
    /// Constrói ul.cfg + partes ul.* (formato USBExtreme/FAT32)
    Ulcfg {
        device_root: PathBuf,
        #[arg(long, default_value = "1024")]
        part_size_mb: u64,
    },
    /// Gera CFG/<ID>.cfg para todos os jogos (+ GSM opcional)
    Cfg {
        device_root: PathBuf,
        /// grava $GSMVMode (ex.: 2 = 480p) nas cfg geradas
        #[arg(long)]
        gsm_vmode: Option<i32>,
    },
    /// Baixa capas GameTDB → ART/<ID>_COV.png (140×200)
    Covers {
        device_root: PathBuf,
        #[arg(long, default_value = "8")]
        concurrency: usize,
        #[arg(long, default_value = "US,EU,JP")]
        regions: String,
    },
    /// Converte PNACH (PCSX2) → CHT/<ID>.cht nativo do OPL
    Cheats {
        device_root: PathBuf,
        #[arg(long)]
        pnach_dir: PathBuf,
    },
    /// VMC: list | export
    Vmc {
        #[command(subcommand)]
        cmd: VmcCmd,
    },
    /// Deploy: baixa release OPL-Mod-PTBR e gera /BOOT_MC/ para uLaunchELF
    Deploy {
        device_root: PathBuf,
        #[arg(long, default_value = "Chrispsz/OPL-Mod-PTBR")]
        repo: String,
        /// apenas lista releases disponíveis
        #[arg(long)]
        list: bool,
    },
    /// Boot Kit Essencial: OPL-Mod-PTBR + wLaunchELF_ISR + PS2BBL + MCA em /BOOT_MC/
    Bootkit {
        /// raiz onde criar BOOT_MC/
        #[arg(default_value = ".")]
        dest: PathBuf,
        /// manifest TOML alternativo (padrão: embutido no binário)
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// sobrescreve arquivos existentes no destino
        #[arg(long)]
        overwrite: bool,
        /// não incluir OPL-Mod-PTBR
        #[arg(long)]
        no_opl: bool,
        /// não incluir wLaunchELF_ISR
        #[arg(long)]
        no_wlaunchelf: bool,
        /// não incluir FREMCB.CNF / CONFIG.INI
        #[arg(long)]
        no_configs: bool,
        /// lista os assets do manifest e sai
        #[arg(long)]
        list: bool,
    },
    /// Biblioteca da Cena: acervo opcional em /BIBLIOTECA/ (copie ao MC só o que usar)
    Biblioteca {
        /// raiz da mídia onde criar BIBLIOTECA/
        #[arg(default_value = ".")]
        dest: PathBuf,
        /// manifest TOML alternativo (padrão: embutido no binário)
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// sobrescreve arquivos existentes no destino
        #[arg(long)]
        overwrite: bool,
        /// lista os assets do acervo e sai
        #[arg(long)]
        list: bool,
    },
    /// Lista mídias externas SEGURAS (discos internos do PC nunca aparecem)
    Devices {},
    /// Dashboard interativo (Ratatui TUI, mouse + abas)
    Tui {
        /// raiz manual (padrão: seletor automático de mídia)
        dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum VmcCmd {
    /// Lista os saves de uma VMC .bin
    List { bin: PathBuf },
    /// Exporta um save para .psu (experimental)
    Export { bin: PathBuf, save: String, out: PathBuf },
}

pub fn fmt_size(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{} MB", b / (1 << 20))
    } else if b >= 1 << 10 {
        format!("{} KB", b / (1 << 10))
    } else {
        format!("{b} B")
    }
}

pub fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

fn attach_ticker(
    counter: Arc<AtomicU64>,
    total: u64,
    label: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = u64::MAX;
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let v = counter.load(Ordering::Relaxed);
            if v != last {
                if total > 0 {
                    eprint!(
                        "\r  {label}: {:>5.1}% ({} / {} MB)   ",
                        v.min(total) as f64 / total as f64 * 100.0,
                        v / 1048576,
                        total / 1048576
                    );
                } else {
                    eprint!("\r  {label}: {} MB   ", v / 1048576);
                }
                last = v;
            }
        }
    })
}

pub async fn run(cmd: Commands, token: Option<String>) -> anyhow::Result<()> {
    match cmd {
        Commands::Info { path } => {
            let info = parser::analyze_path(&path)?;
            println!("Arquivo   : {}", path.display());
            println!("Game ID   : {}", info.game_id);
            println!("Título    : {}", info.volume_id);
            println!("Mídia     : {}", info.media.as_str());
            println!(
                "Tamanho   : {} ({} setores de 2048)",
                fmt_size(info.size),
                info.total_sectors
            );
        }
        Commands::Scan { dir } => {
            let games = library::scan_library(&dir)?;
            println!(
                "{:<12} {:<36} {:>9}  {:<4} {:<5} FRAG",
                "GAME ID", "TÍTULO", "TAMANHO", "FMT", "MÍDIA"
            );
            for g in &games {
                println!(
                    "{:<12} {:<36} {:>9}  {:<4} {:<5} {}",
                    g.game_id,
                    trunc(&g.title, 34),
                    fmt_size(g.size),
                    g.format,
                    g.media.as_str(),
                    g.fragmentation
                );
            }
            println!("\n{} jogos", games.len());
        }
        Commands::Install { path, device_root, zso, direct } => {
            if direct {
                // caminho legado: conversão/cópia direta na mídia
                let total = std::fs::metadata(&path)?.len();
                let counter = Arc::new(AtomicU64::new(0));
                let t = attach_ticker(
                    counter.clone(),
                    total,
                    if zso { "ZSO" } else { "Copiando" },
                );
                let target = library::install(
                    &path,
                    &device_root,
                    &library::InstallOptions { as_zso: zso },
                    Some(counter),
                )
                .await?;
                t.abort();
                eprintln!();
                println!("✓ Instalado: {}", target.display());
            } else {
                // Mega Prompt 7: staging em SSD local → gravação validada
                if !storage::is_removable_root(&device_root) {
                    eprintln!(
                        "⚠ atenção: '{}' não é uma mídia externa detectada — \
                         prosseguindo por invocação explícita de CLI",
                        device_root.display()
                    );
                }
                let job = crate::core::pipeline::InstallJob {
                    target_fs: storage::fs_class_for_root(&device_root),
                    free_bytes: storage::free_bytes(&device_root),
                    iso: path.clone(),
                    target_root: device_root.clone(),
                    as_zso: zso,
                    pre_shrink: false,
                    fallocate: true,
                    overwrite: false,
                };
                let cancel = Arc::new(AtomicBool::new(false));
                let st = Arc::new(Mutex::new((
                    crate::core::pipeline::PhaseKind::Analyze,
                    0u64,
                    0u64,
                )));
                let stop = Arc::new(AtomicBool::new(false));
                let (s2, stop2) = (st.clone(), stop.clone());
                let th = std::thread::spawn(move || loop {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(g) = s2.lock() {
                        let (ph, done, total) = *g;
                        let pct = if total > 0 {
                            format!(
                                "{:>5.1}%",
                                done.min(total) as f64 / total as f64 * 100.0
                            )
                        } else {
                            "…".to_string()
                        };
                        eprint!("\r  {:<34} {:>7}", ph.label(), pct);
                    }
                    std::thread::sleep(Duration::from_millis(400));
                });
                let res = crate::core::pipeline::staged_install(&job, &|p| {
                    if let Ok(mut g) = st.lock() {
                        *g = (p.phase, p.done, p.total);
                    }
                }, &cancel);
                stop.store(true, Ordering::Relaxed);
                let _ = th.join();
                eprintln!();
                let rep = res?;
                println!("✓ Instalado: {}", rep.final_path.display());
                println!("  {} · {}", rep.game_id, crate::core::pipeline::summary_line(&rep));
            }
        }
        Commands::Zso { input, output } => {
            let total = std::fs::metadata(&input)?.len();
            let counter = Arc::new(AtomicU64::new(0));
            let t = attach_ticker(counter.clone(), total, "ZSO");
            let (comp, orig) = zso::compress_iso(&input, &output, Some(counter))?;
            t.abort();
            eprintln!();
            println!(
                "✓ ZSO: {} ({} MB, {:.1}% do original)",
                output.display(),
                comp / 1048576,
                comp as f64 / orig as f64 * 100.0
            );
        }
        Commands::Unzso { input, output } => {
            let total = zso::decompress_zso(&input, &output, None)?;
            println!("✓ ISO restaurada: {} ({})", output.display(), fmt_size(total));
        }
        Commands::Shrink { input, output } => {
            let rep = shrink::shrink_iso(&input, &output)?;
            println!(
                "✓ Shrink: {} → {} (economia {} | {} extents movidos | {} arquivos aparados)",
                fmt_size(rep.original_bytes),
                fmt_size(rep.new_bytes),
                fmt_size(rep.saved_bytes()),
                rep.extents_moved,
                rep.files_trimmed
            );
        }
        Commands::Ulcfg { device_root, part_size_mb } => {
            let log = ulcfg::build_from_library(&device_root, part_size_mb, None)?;
            for l in &log {
                println!("{l}");
            }
            println!("✓ ul.cfg atualizado em {}", device_root.display());
        }
        Commands::Cfg { device_root, gsm_vmode } => {
            let games = library::scan_library(&device_root)?;
            let mut n = 0;
            for g in &games {
                if g.format == "UL" {
                    continue;
                }
                let info = parser::analyze_path(&g.path)?;
                let gsm = gsm_vmode.map(|v| cfg::GsmConfig {
                    vmode: v,
                    ..Default::default()
                });
                let p = cfg::write_game_cfg(&device_root, &info, gsm)?;
                println!("✓ {}", p.display());
                n += 1;
            }
            println!("{n} cfg gerados");
        }
        Commands::Covers { device_root, concurrency, regions } => {
            let games = library::scan_library(&device_root)?;
            let extra: Vec<String> = regions.split(',').map(str::trim).map(String::from).collect();
            println!("baixando capas para {} jogos…", games.len());
            let (ok, fail) = artwork::fetch_covers(&device_root, &games, concurrency, &extra, token).await?;
            println!("✓ Capas: {ok} baixadas, {fail} falhas");
        }
        Commands::Cheats { device_root, pnach_dir } => {
            let (ok, skip) = cheats::convert_dir(&device_root, &pnach_dir)?;
            println!("✓ CHT: {ok} convertidos, {skip} ignorados");
        }
        Commands::Vmc { cmd } => match cmd {
            VmcCmd::List { bin } => {
                let mut card = vmc::Vmc::open(&bin)?;
                let saves = card.list_saves()?;
                println!(
                    "VMC: {} clusters × {} B — {} saves",
                    card.info.clusters_per_card,
                    card.info.cluster_size,
                    saves.len()
                );
                for s in &saves {
                    println!(
                        "  {:<32} {:>9}  {} arquivos",
                        trunc(&s.name, 30),
                        fmt_size(s.size_bytes),
                        s.files
                    );
                }
            }
            VmcCmd::Export { bin, save, out } => {
                let mut card = vmc::Vmc::open(&bin)?;
                let n = card.export_psu(&save, &out)?;
                println!("✓ PSU exportado: {} ({})", out.display(), fmt_size(n));
            }
        },
        Commands::Deploy { device_root, repo, list } => {
            let client = GitHubClient::new(token)?;
            if list {
                for r in client.list_releases(&repo, 5).await? {
                    println!("• {} {} — {} assets", r.tag_name, if r.prerelease { "(pre)" } else { "" }, r.assets.len());
                    for a in &r.assets {
                        println!("    - {} ({})", a.name, fmt_size(a.size));
                    }
                }
            } else {
                let counter = Arc::new(AtomicU64::new(0));
                println!("deploy: consultando {repo}…");
                let rep = fmcb::deploy_boot_mc(&device_root, &repo, &client, counter).await?;
                println!("✓ Deploy {} concluído:", rep.tag);
                for (f, s) in &rep.files {
                    println!("    {:<44} {:>8}", f, fmt_size(*s));
                }
                println!("\nCopie via uLaunchELF — instruções em BOOT_MC/README-INSTALACAO.txt");
            }
        }
        Commands::Bootkit { dest, manifest, overwrite, no_opl, no_wlaunchelf, no_configs, list } => {
            let mut m = match &manifest {
                Some(p) => bootkit::load_manifest(p)?,
                None => bootkit::default_manifest()?,
            };
            bootkit::apply_flags(&mut m, !no_opl, !no_wlaunchelf, !no_configs);
            if list {
                println!("Kit : {} (spec {})", m.meta.kit_name, m.meta.spec);
                for a in &m.assets {
                    println!(
                        "• {:<20} {:<30} {:<5} {}",
                        a.id,
                        a.target,
                        if a.enabled { "[ON]" } else { "[off]" },
                        bootkit::source_desc(&a.source)
                    );
                    if !a.description.is_empty() {
                        println!("  {:<58} {}", "", a.description);
                    }
                }
                return Ok(());
            }
            let client = GitHubClient::new(token)?;
            let counter = Arc::new(AtomicU64::new(0));
            let ticker = attach_ticker(counter.clone(), 0, "Boot Kit");
            println!("boot kit: montando /BOOT_MC/ em {}…", dest.display());
            let rep = bootkit::assemble(&dest, &m, &client, counter, overwrite).await?;
            ticker.abort();
            eprintln!();
            for a in &rep.assets {
                match &a.status {
                    bootkit::AssetStatus::Ok { bytes, sha256, source } => {
                        println!(
                            "✓ {:<30} {:>9}  sha256:{}…  [{}]",
                            a.target,
                            fmt_size(*bytes),
                            &sha256[..8],
                            source
                        );
                    }
                    bootkit::AssetStatus::Skipped(w) => {
                        println!("⊘ {:<30} ({w})", a.target);
                    }
                    bootkit::AssetStatus::Failed(err) => {
                        println!("✗ {:<30}: {err}", a.target);
                    }
                }
            }
            let ok = rep
                .assets
                .iter()
                .filter(|a| matches!(a.status, bootkit::AssetStatus::Ok { .. }))
                .count();
            println!(
                "\n✓ Boot Kit: {ok} arquivos, {} em {}",
                fmt_size(rep.total_bytes),
                rep.dest.display()
            );
        }
        Commands::Biblioteca { dest, manifest, overwrite, list } => {
            let m = match &manifest {
                Some(p) => bootkit::load_manifest(p)?,
                None => bootkit::default_library()?,
            };
            if list {
                println!("Acervo: {} (spec {})", m.meta.kit_name, m.meta.spec);
                for a in &m.assets {
                    println!(
                        "• {:<20} {:<42} {:<5} {}",
                        a.id,
                        a.target,
                        if a.enabled { "[ON]" } else { "[off]" },
                        bootkit::source_desc(&a.source)
                    );
                    if !a.description.is_empty() {
                        println!("  {:<58} {}", "", a.description);
                    }
                }
                return Ok(());
            }
            let client = GitHubClient::new(token)?;
            let counter = Arc::new(AtomicU64::new(0));
            let ticker = attach_ticker(counter.clone(), 0, "Biblioteca");
            println!("biblioteca: montando /BIBLIOTECA/ em {}…", dest.display());
            let rep =
                bootkit::assemble_kind(&dest, bootkit::KitKind::Library, &m, &client, counter, overwrite).await?;
            ticker.abort();
            eprintln!();
            for a in &rep.assets {
                match &a.status {
                    bootkit::AssetStatus::Ok { bytes, sha256, source } => {
                        println!(
                            "✓ {:<42} {:>9}  sha256:{}…  [{}]",
                            a.target,
                            fmt_size(*bytes),
                            &sha256[..8],
                            source
                        );
                    }
                    bootkit::AssetStatus::Skipped(w) => {
                        println!("⊘ {:<42} ({w})", a.target);
                    }
                    bootkit::AssetStatus::Failed(err) => {
                        println!("✗ {:<42}: {err}", a.target);
                    }
                }
            }
            let ok = rep
                .assets
                .iter()
                .filter(|a| matches!(a.status, bootkit::AssetStatus::Ok { .. }))
                .count();
            println!(
                "\n✓ Biblioteca: {ok} arquivos, {} em {}",
                fmt_size(rep.total_bytes),
                rep.dest.display()
            );
            println!("  guia completo em BIBLIOTECA/LEIA-ME.txt");
        }
        Commands::Devices {} => {
            let devs = storage::scan_target_devices();
            if devs.is_empty() {
                println!("nenhuma mídia externa montada detectada");
                println!(
                    "discos do sistema são bloqueados por padrão \
                     (PS2_MANAGER_ALLOW_INTERNAL=1 libera, por conta e risco)"
                );
            }
            for d in &devs {
                println!(
                    "• {} — /dev/{} [{}]{}",
                    d.display_name(),
                    d.name,
                    d.bus.label(),
                    if d.removable { " removível" } else { "" }
                );
                for m in &d.mounts {
                    let cls = storage::FsClass::from_fs(&m.fs_type);
                    let note = if cls.bdm_recommended() {
                        " ← BDM recomendado ✓"
                    } else if cls == storage::FsClass::Fat32 {
                        " ← legacy, máx 4 GiB/arquivo"
                    } else {
                        ""
                    };
                    println!(
                        "    /dev/{} em {} — {} livre de {} — {}{}",
                        m.source,
                        m.mount_point.display(),
                        fmt_size(m.free_bytes),
                        fmt_size(m.total_bytes),
                        m.fs_type,
                        note
                    );
                }
            }
        }
        Commands::Tui { dir } => {
            tui::run_tui(dir).await?;
        }
    }
    Ok(())
}
