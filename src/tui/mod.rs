//! Dashboard TUI v2 (Ratatui + mouse) — Mega Prompts 6/7.
//!
//! * Abas: `[F1] Biblioteca · [F2] Instalar · [F3] Boot Kit · [F4] Capas/Cheats · [F5] Ajuda`
//! * Modais: seletor de mídia (`F6`/`s`), primeiro uso (wizard), confirmação
//!   de instalação, progresso por fases e avisos.
//! * Mouse: clique em abas, toggles, botões e linhas; roda faz scroll.
//! * Segurança: o seletor de mídia só lista dispositivos externos — os
//!   discos do PC nunca aparecem (ver `core::storage`).

mod modals;

use crate::core::pipeline::{self, InstallJob, PhaseKind};
use crate::core::storage::{self, FsClass, TargetDevice};
use crate::error::Result;
use crate::iso::zso::compress_iso;
use crate::opl::cfg::GsmConfig;
use crate::opl::library::{self, GameEntry};
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Row, Table, Wrap};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

use modals::Browser;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

const TAB_LABELS: [&str; 5] = [
    "F1 Biblioteca",
    "F2 Instalar",
    "F3 Boot Kit",
    "F4 Capas/Cheats",
    "F5 Ajuda",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Library,
    Install,
    BootKit,
    Extras,
    Help,
}

impl Tab {
    fn from_index(i: usize) -> Option<Tab> {
        match i {
            0 => Some(Tab::Library),
            1 => Some(Tab::Install),
            2 => Some(Tab::BootKit),
            3 => Some(Tab::Extras),
            4 => Some(Tab::Help),
            _ => None,
        }
    }
    fn idx(self) -> usize {
        match self {
            Tab::Library => 0,
            Tab::Install => 1,
            Tab::BootKit => 2,
            Tab::Extras => 3,
            Tab::Help => 4,
        }
    }
}

#[derive(Clone, PartialEq)]
enum Modal {
    None,
    Picker,
    Wizard,
    ConfirmInstall,
    Progress,
    Notify(String),
    PickPnach,
}

#[derive(Clone)]
enum Hit {
    Tab(usize),
    DeviceChip,
    GameRow(usize),
    QuickAction(usize),
    PickerRow(usize),
    PickerConfirm,
    BrowserRow(usize),
    BrowserQuick(usize),
    InstToggle(usize),
    InstGo,
    BkRow(usize),
    BkGo,
    ExtraAction(usize),
    ExtraToggle,
    WizardOk,
    NotifyOk,
    ConfirmYes,
    ConfirmNo,
    ProgressCancel,
    BrowserDirRow(usize),
    BrowserDirGo,
}

enum UiMsg {
    Log(String),
    Games(Vec<GameEntry>),
    NetProgress(Option<(String, u64, u64)>),
    PipeProgress(pipeline::Progress),
    Busy(bool),
    InstallDone(std::result::Result<String, String>),
    Notify(String),
}

/// Toggles do painel [F3] — Deploy de Boot Essencial.
#[derive(Clone, Copy)]
struct BkOpts {
    include_opl: bool,
    include_wle: bool,
    include_configs: bool,
    include_library: bool,
    overwrite: bool,
}

impl Default for BkOpts {
    fn default() -> Self {
        Self {
            include_opl: true,
            include_wle: true,
            include_configs: false,
            include_library: true,
            overwrite: false,
        }
    }
}

/// Opções do pipeline de instalação (aba [F2]).
#[derive(Clone, Copy)]
struct InstallOpts {
    as_zso: bool,
    pre_shrink: bool,
    fallocate: bool,
    /// formato USBExtreme (ul.cfg + partes ul.*) — legado perfeito;
    /// em FAT32 também é automático para imagens >4 GiB
    split_ul: bool,
}

impl Default for InstallOpts {
    fn default() -> Self {
        Self { as_zso: true, pre_shrink: false, fallocate: true, split_ul: false }
    }
}

#[derive(Clone)]
struct SelectedMedia {
    device: String,
    dev: String,
    mount: PathBuf,
    fs_type: String,
    fs: FsClass,
    free: u64,
    total: u64,
}

impl SelectedMedia {
    fn from_device(d: &TargetDevice) -> Option<Self> {
        let m = d.primary_mount()?;
        Some(Self {
            device: d.display_name(),
            dev: m.source.clone(),
            mount: m.mount_point.clone(),
            fs_type: m.fs_type.clone(),
            fs: FsClass::from_fs(&m.fs_type),
            free: m.free_bytes,
            total: m.total_bytes,
        })
    }
}

struct ProgressUi {
    phase: PhaseKind,
    done: u64,
    total: u64,
    speed_mbps: f64,
    last_done: u64,
    last_t: Instant,
    finished: Option<std::result::Result<String, String>>,
    phase_log: Vec<String>,
}

impl ProgressUi {
    fn new() -> Self {
        Self {
            phase: PhaseKind::Analyze,
            done: 0,
            total: 0,
            speed_mbps: 0.0,
            last_done: 0,
            last_t: Instant::now(),
            finished: None,
            phase_log: Vec::new(),
        }
    }

    fn update(&mut self, p: &pipeline::Progress) {
        if p.phase != self.phase {
            self.phase_log.push(format!("✓ {}", self.phase.label()));
            self.phase = p.phase;
            self.last_done = 0;
            self.last_t = Instant::now();
        }
        if p.phase == PhaseKind::Transfer {
            let dt = self.last_t.elapsed().as_secs_f64();
            if dt >= 0.25 && p.done >= self.last_done {
                let inst = (p.done - self.last_done) as f64 / dt / 1048576.0;
                self.speed_mbps = if self.speed_mbps <= 0.0 {
                    inst
                } else {
                    self.speed_mbps * 0.7 + inst * 0.3
                };
                self.last_done = p.done;
                self.last_t = Instant::now();
            }
        }
        self.done = p.done;
        self.total = p.total;
    }

    fn eta_secs(&self) -> Option<u64> {
        if self.speed_mbps <= 0.01 || self.total <= self.done {
            return None;
        }
        Some((((self.total - self.done) as f64 / 1048576.0) / self.speed_mbps) as u64)
    }
}

struct App {
    manual_root: Option<PathBuf>,
    root: PathBuf,
    media: Option<SelectedMedia>,
    devices: Vec<TargetDevice>,
    picker_sel: usize,
    tab: Tab,
    games: Vec<GameEntry>,
    selected: usize,
    logs: VecDeque<String>,
    progress: Option<(String, u64, u64)>,
    busy: bool,
    modal: Modal,
    table_state: ratatui::widgets::TableState,
    bk_sel: usize,
    bk_opts: BkOpts,
    bk_kit: (String, String),
    browser: Browser,
    dir_browser: Browser,
    inst_opts: InstallOpts,
    inst_gsm: i32,
    extras_sel: usize,
    prog: Option<ProgressUi>,
    cancel: Arc<AtomicBool>,
    hits: Vec<(Rect, Hit)>,
}

fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

fn config_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config").join("ps2-manager"))
}

fn wizard_pending() -> bool {
    match config_dir() {
        Some(d) => !d.join("wizard.done").exists(),
        None => false,
    }
}

fn wizard_mark_done() {
    if let Some(d) = config_dir() {
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join("wizard.done"), b"ok");
    }
}

impl App {
    fn new(manual_root: Option<PathBuf>) -> Self {
        let (bk_name, bk_spec) = crate::deploy::bootkit::default_manifest()
            .map(|m| (m.meta.kit_name, m.meta.spec))
            .unwrap_or_else(|_| ("Boot Kit Essencial PS2".into(), "?".into()));
        let root = manual_root.clone().unwrap_or_else(|| PathBuf::from("."));
        let home = home_dir();
        Self {
            browser: Browser::open(&home),
            dir_browser: Browser::open(&home),
            root,
            manual_root,
            media: None,
            devices: Vec::new(),
            picker_sel: 0,
            tab: Tab::Library,
            games: Vec::new(),
            selected: 0,
            logs: VecDeque::new(),
            progress: None,
            busy: false,
            modal: Modal::None,
            table_state: ratatui::widgets::TableState::default(),
            bk_sel: 0,
            bk_opts: BkOpts::default(),
            bk_kit: (bk_name, bk_spec),
            inst_opts: InstallOpts::default(),
            inst_gsm: 0,
            extras_sel: 0,
            prog: None,
            cancel: Arc::new(AtomicBool::new(false)),
            hits: Vec::new(),
        }
    }

    fn log(&mut self, s: impl Into<String>) {
        let s = s.into();
        crate::logging::info(&s); // painel Log da TUI → também vai para o arquivo
        self.logs.push_back(s);
        while self.logs.len() > 200 {
            self.logs.pop_front();
        }
    }

    fn rescan(&mut self) {
        match library::scan_library(&self.root) {
            Ok(games) => {
                let n = games.len();
                self.games = games;
                if self.selected >= self.games.len() {
                    self.selected = self.games.len().saturating_sub(1);
                }
                self.log(format!("Biblioteca: {n} jogos em {}", self.root.display()));
            }
            Err(e) => self.log(format!("scan falhou: {e}")),
        }
    }

    fn rescan_devices(&mut self) {
        let devs = storage::scan_target_devices();
        let n = devs.len();
        self.devices = devs;
        if self.picker_sel >= self.devices.len() {
            self.picker_sel = self.devices.len().saturating_sub(1);
        }
        // atualiza espaço livre da mídia em uso
        let upd = self
            .media
            .as_ref()
            .and_then(|m| storage::free_bytes(&m.mount));
        if let (Some(free), Some(m)) = (upd, self.media.as_mut()) {
            m.free = free;
        }
        // auto-seleção passiva quando há exatamente uma mídia
        if self.media.is_none() && self.manual_root.is_none() && n == 1 {
            if let Some(m) = SelectedMedia::from_device(&self.devices[0]) {
                self.root = m.mount.clone();
                self.log(format!(
                    "mídia detectada: {} ({} — {} livres)",
                    m.device,
                    m.fs.label(),
                    crate::cli::fmt_size(m.free)
                ));
                if m.fs == FsClass::Fat32 {
                    self.log("⚠ FAT32: máx 4 GiB por arquivo — ZSO obrigatório para DVDs grandes");
                }
                self.media = Some(m);
            }
        }
        self.log(format!("scan de mídias: {n} externa(s) visível(is)"));
    }

    fn confirm_picker(&mut self) {
        let Some(d) = self.devices.get(self.picker_sel) else { return; };
        let Some(m) = SelectedMedia::from_device(d) else {
            self.log("dispositivo sem ponto de montagem utilizável");
            return;
        };
        self.root = m.mount.clone();
        let has_opl = m.mount.join("DVD").is_dir() || m.mount.join("CD").is_dir();
        self.log(format!(
            "mídia: {} — {} ({} livres)",
            m.device,
            m.fs.label(),
            crate::cli::fmt_size(m.free)
        ));
        if m.fs == FsClass::Fat32 {
            self.log("⚠ FAT32: máx 4 GiB por arquivo — ZSO obrigatório para DVDs grandes");
        }
        if !has_opl {
            self.log("estrutura OPL ausente — será criada na primeira instalação");
        } else {
            self.log("estrutura OPL detectada ✓");
        }
        self.media = Some(m);
        self.modal = Modal::None;
        self.rescan();
    }

    fn spawn_ticker(&self, tx: mpsc::UnboundedSender<UiMsg>, counter: Arc<AtomicU64>, total: u64, label: &'static str) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let v = counter.load(Ordering::Relaxed);
                if tx.send(UiMsg::NetProgress(Some((label.to_string(), v, total)))).is_err() {
                    break;
                }
                if SHUTDOWN.load(Ordering::Relaxed) {
                    break;
                }
            }
        });
    }

    async fn compress_selected(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        let Some(g) = self.games.get(self.selected).cloned() else {
            self.log("nada selecionado");
            return;
        };
        if g.format != "ISO" {
            self.log("selecione um jogo no formato ISO");
            return;
        }
        self.busy = true;
        let out = g.path.with_extension("zso");
        let counter = Arc::new(AtomicU64::new(0));
        self.spawn_ticker(tx.clone(), counter.clone(), g.size, "ZSO");
        let tx2 = tx.clone();
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let res = compress_iso(&g.path, &out, Some(counter));
            let _ = tx2.send(UiMsg::NetProgress(None));
            let _ = tx2.send(UiMsg::Busy(false));
            match res {
                Ok((comp, tot)) => {
                    let _ = tx2.send(UiMsg::Log(format!(
                        "✓ ZSO: {} → {} MB ({:.1}% do original)",
                        tot / 1048576,
                        comp / 1048576,
                        comp as f64 / tot as f64 * 100.0
                    )));
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ ZSO falhou: {e}")));
                }
            }
            let _ = tx2.send(UiMsg::Games(
                library::scan_library(&root).unwrap_or_default(),
            ));
        });
        self.log(format!("convertendo {} …", g.game_id));
    }

    /// Painel [F3] — monta o Boot Kit Essencial em root/BOOT_MC.
    fn run_bootkit(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        if self.busy {
            self.log("ocupado — aguarde a operação atual");
            return;
        }
        self.busy = true;
        let opts = self.bk_opts;
        let tx2 = tx.clone();
        let root = self.root.clone();
        let token = std::env::var("GITHUB_TOKEN").ok();
        tokio::spawn(async move {
            let _ = tx2.send(UiMsg::Log("boot kit: resolvendo releases no GitHub…".into()));
            let manifest = match crate::deploy::bootkit::default_manifest() {
                Ok(mut m) => {
                    crate::deploy::bootkit::apply_flags(
                        &mut m,
                        opts.include_opl,
                        opts.include_wle,
                        opts.include_configs,
                    );
                    m
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ boot kit: manifest: {e}")));
                    let _ = tx2.send(UiMsg::Busy(false));
                    return;
                }
            };
            match crate::deploy::github::GitHubClient::new(token) {
                Ok(client) => {
                    let counter = Arc::new(AtomicU64::new(0));
                    let txc = tx2.clone();
                    let c2 = counter.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            let v = c2.load(Ordering::Relaxed);
                            if txc.send(UiMsg::NetProgress(Some(("Boot Kit".into(), v, 0)))).is_err() {
                                break;
                            }
                            if SHUTDOWN.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    });
                    match crate::deploy::bootkit::assemble(&root, &manifest, &client, counter.clone(), opts.overwrite).await {
                        Ok(rep) => {
                            for a in &rep.assets {
                                let line = match &a.status {
                                    crate::deploy::bootkit::AssetStatus::Ok { bytes, sha256, .. } => {
                                        format!("✓ {} ({} sha256:{}…)", a.target, crate::cli::fmt_size(*bytes), &sha256[..8])
                                    }
                                    crate::deploy::bootkit::AssetStatus::Skipped(w) => {
                                        format!("⊘ {} ({w})", a.target)
                                    }
                                    crate::deploy::bootkit::AssetStatus::Failed(m) => {
                                        format!("✗ {}: {m}", a.target)
                                    }
                                };
                                let _ = tx2.send(UiMsg::Log(line));
                            }
                            let ok = rep.assets.iter().filter(|a| matches!(a.status, crate::deploy::bootkit::AssetStatus::Ok { .. })).count();
                            let _ = tx2.send(UiMsg::Log(format!("✓ {}: {ok} arquivos em {}", rep.kit_name, rep.dest.display())));

                            // Biblioteca da Cena (acervo opcional em /BIBLIOTECA/)
                            if opts.include_library {
                                let _ = tx2.send(UiMsg::Log("biblioteca: resolvendo releases do acervo…".into()));
                                match crate::deploy::bootkit::default_library() {
                                    Ok(lib) => {
                                        match crate::deploy::bootkit::assemble_kind(&root, crate::deploy::bootkit::KitKind::Library, &lib, &client, counter, opts.overwrite).await {
                                            Ok(lrep) => {
                                                for a in &lrep.assets {
                                                    let line = match &a.status {
                                                        crate::deploy::bootkit::AssetStatus::Ok { bytes, .. } => {
                                                            format!("✓ {} ({})", a.target, crate::cli::fmt_size(*bytes))
                                                        }
                                                        crate::deploy::bootkit::AssetStatus::Skipped(w) => {
                                                            format!("⊘ {} ({w})", a.target)
                                                        }
                                                        crate::deploy::bootkit::AssetStatus::Failed(m) => {
                                                            format!("✗ {}: {m}", a.target)
                                                        }
                                                    };
                                                    let _ = tx2.send(UiMsg::Log(line));
                                                }
                                                let okl = lrep.assets.iter().filter(|a| matches!(a.status, crate::deploy::bootkit::AssetStatus::Ok { .. })).count();
                                                let _ = tx2.send(UiMsg::Log(format!("✓ Biblioteca: {okl} arquivos em {} — guia em LEIA-ME.txt", lrep.dest.display())));
                                            }
                                            Err(e) => {
                                                let _ = tx2.send(UiMsg::Log(format!("✗ biblioteca falhou: {e}")));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = tx2.send(UiMsg::Log(format!("✗ biblioteca: manifest: {e}")));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx2.send(UiMsg::Log(format!("✗ boot kit falhou: {e}")));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ cliente GitHub: {e}")));
                }
            }
            let _ = tx2.send(UiMsg::NetProgress(None));
            let _ = tx2.send(UiMsg::Busy(false));
        });
        self.log("boot kit: montagem de /BOOT_MC/ iniciada…");
    }

    fn covers(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        let tx2 = tx.clone();
        let root = self.root.clone();
        tokio::spawn(async move {
            let _ = tx2.send(UiMsg::Log("capas: consultando base de capas…".into()));
            let entries = library::scan_library(&root).unwrap_or_default();
            let token = std::env::var("GITHUB_TOKEN").ok();
            match crate::scrape::artwork::fetch_covers(&root, &entries, 8, &[], token).await {
                Ok((ok, fail)) => {
                    let _ = tx2.send(UiMsg::Log(format!("✓ capas: {ok} baixadas, {fail} falhas")));
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ capas falhou: {e}")));
                }
            }
            let _ = tx2.send(UiMsg::Games(library::scan_library(&root).unwrap_or_default()));
        });
    }

    fn run_cfgs(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        if self.busy {
            self.log("ocupado — aguarde a operação atual");
            return;
        }
        self.busy = true;
        let root = self.root.clone();
        let gsm_on = self.inst_gsm > 0;
        let vmode = self.inst_gsm;
        let tx2 = tx.clone();
        tokio::task::spawn_blocking(move || {
            let gsm = if gsm_on {
                Some(GsmConfig { vmode, ..Default::default() })
            } else {
                None
            };
            let res = (|| -> crate::error::Result<usize> {
                let games = library::scan_library(&root)?;
                let mut n = 0;
                for g in &games {
                    if g.format == "UL" {
                        continue;
                    }
                    let info = crate::iso::parser::analyze_path(&g.path)?;
                    crate::opl::cfg::write_game_cfg(&root, &info, gsm)?;
                    n += 1;
                }
                Ok(n)
            })();
            let _ = tx2.send(UiMsg::Busy(false));
            match res {
                Ok(n) => {
                    let _ = tx2.send(UiMsg::Log(format!(
                        "✓ {n} CFG gerados (GSM {})",
                        if gsm_on { format!("vmode {vmode}") } else { "desligado".into() }
                    )));
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ CFG falhou: {e}")));
                }
            }
        });
        self.log("gerando CFG/<ID>.cfg …");
    }

    fn run_ulcfg(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        if self.busy {
            self.log("ocupado — aguarde a operação atual");
            return;
        }
        self.busy = true;
        let root = self.root.clone();
        let tx2 = tx.clone();
        tokio::task::spawn_blocking(move || {
            let res = crate::opl::ulcfg::build_from_library(&root, 1024, None);
            let _ = tx2.send(UiMsg::Busy(false));
            match res {
                Ok(log) => {
                    let n = log.len();
                    let _ = tx2.send(UiMsg::Log(format!("✓ ul.cfg reconstruído ({n} registros)")));
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ ul.cfg falhou: {e}")));
                }
            }
        });
        self.log("reconstruindo ul.cfg …");
    }

    fn run_cheats_current(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        let dir = self.dir_browser.path.clone();
        let root = self.root.clone();
        self.modal = Modal::None;
        self.busy = true;
        self.log(format!("cheats: convertendo PNACH de {}…", dir.display()));
        let tx2 = tx.clone();
        tokio::task::spawn_blocking(move || {
            let res = crate::scrape::cheats::convert_dir(&root, &dir);
            let _ = tx2.send(UiMsg::Busy(false));
            match res {
                Ok((ok, skip)) => {
                    let _ = tx2.send(UiMsg::Log(format!(
                        "✓ CHT: {ok} convertidos, {skip} ignorados (fonte {})",
                        dir.display()
                    )));
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ cheats falharam: {e}")));
                }
            }
        });
    }

    fn start_install(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        if self.busy {
            self.log("ocupado — aguarde a operação atual");
            return;
        }
        let (Some(media), Some(iso)) = (self.media.clone(), self.browser.selected_file()) else {
            self.modal = Modal::Notify(
                "Selecione uma ISO no navegador (à esquerda) e uma mídia externa ([F6]).".into(),
            );
            return;
        };
        if !iso.exists() {
            self.modal = Modal::Notify("Arquivo ISO não encontrado.".into());
            return;
        }
        // o melhor legado possível: FAT32 + ISO crua >4 GiB = USBExtreme automático
        if media.fs == crate::core::storage::FsClass::Fat32
            && !self.inst_opts.as_zso
            && !self.inst_opts.split_ul
            && std::fs::metadata(&iso)
                .is_ok_and(|m| m.len() > crate::core::storage::FsClass::Fat32.max_file())
        {
            self.log("ISO > 4 GiB em FAT32 — fatiamento USBExtreme (ul.*) automático");
        }
        self.busy = true;
        self.cancel.store(false, Ordering::Relaxed);
        self.prog = Some(ProgressUi::new());
        self.modal = Modal::Progress;
        let opts = self.inst_opts;
        let cancel = self.cancel.clone();
        let tx2 = tx.clone();
        let (mount, fs, free) = (media.mount.clone(), media.fs, media.free);
        tokio::task::spawn_blocking(move || {
            let job = InstallJob {
                iso,
                target_root: mount,
                target_fs: fs,
                as_zso: opts.as_zso,
                pre_shrink: opts.pre_shrink,
                fallocate: opts.fallocate,
                overwrite: false,
                split_ul: opts.split_ul,
                free_bytes: if free > 0 { Some(free) } else { None },
            };
            let send = |p: pipeline::Progress| {
                let _ = tx2.send(UiMsg::PipeProgress(p));
            };
            let res = pipeline::staged_install(&job, &send, &cancel);
            let msg = match res {
                Ok(rep) => Ok(format!(
                    "✓ {} {} — {}\n  {}",
                    rep.game_id,
                    trunc(&rep.title, 40),
                    rep.final_path.display(),
                    pipeline::summary_line(&rep)
                )),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx2.send(UiMsg::InstallDone(msg));
        });
        self.log(format!("instalando {} …", self.browser.selected_file().map(|p| p.display().to_string()).unwrap_or_default()));
    }

    async fn run_extras(&mut self, idx: usize, tx: mpsc::UnboundedSender<UiMsg>) {
        match idx {
            0 => self.covers(tx),
            1 => {
                let home = home_dir();
                self.dir_browser = Browser::open(&home);
                self.modal = Modal::PickPnach;
            }
            2 => self.run_cfgs(tx),
            3 => self.run_ulcfg(tx),
            4 => self.run_diag(tx),
            _ => {}
        }
    }

    /// Diagnóstico completo — exercita parser, instalação staging→ZSO,
    /// ul.cfg/CFG, capas e GitHub numa sandbox temporária e grava relatório.
    fn run_diag(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        if self.busy {
            self.log("ocupado — aguarde a operação atual");
            return;
        }
        self.busy = true;
        self.log("diagnóstico: iniciando teste completo (sandbox temporária)…");
        let tx2 = tx.clone();
        let tx3 = tx2.clone();
        let on_log: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |s: &str| {
            let _ = tx3.send(UiMsg::Log(s.to_string()));
        });
        tokio::spawn(async move {
            let rep = crate::diag::run_full(crate::diag::default_scan_dir().as_deref(), false, on_log).await;
            let summary = crate::diag::summary_text(&rep);
            if let Some(p) = &rep.report_path {
                crate::logging::info(&format!("diag: relatório em {}", p.display()));
            }
            let _ = tx2.send(UiMsg::Busy(false));
            let _ = tx2.send(UiMsg::Notify(summary));
        });
    }

    fn game_info(&mut self) {
        let Some(g) = self.games.get(self.selected) else { return; };
        self.modal = Modal::Notify(format!(
            "{} — {}\n\n{:>10}  {}\n{:>10}  {}\n{:>10}  {}\n{:>10}  {}",
            g.game_id,
            g.title,
            "Tamanho",
            crate::cli::fmt_size(g.size),
            "Formato",
            g.format,
            "Mídia",
            g.media.as_str(),
            "Caminho",
            g.path.display()
        ));
    }

    fn toggle_bk(&mut self) {
        match self.bk_sel {
            0 => self.bk_opts.include_opl = !self.bk_opts.include_opl,
            1 => self.bk_opts.include_wle = !self.bk_opts.include_wle,
            2 => self.bk_opts.include_configs = !self.bk_opts.include_configs,
            3 => self.bk_opts.include_library = !self.bk_opts.include_library,
            _ => self.bk_opts.overwrite = !self.bk_opts.overwrite,
        }
    }

    fn toggle_install_opt(&mut self, i: usize) {
        match i {
            0 => self.inst_opts.as_zso = !self.inst_opts.as_zso,
            1 => self.inst_opts.pre_shrink = !self.inst_opts.pre_shrink,
            2 => self.inst_opts.fallocate = !self.inst_opts.fallocate,
            _ => self.inst_opts.split_ul = !self.inst_opts.split_ul,
        }
    }

    async fn on_key(&mut self, code: KeyCode, tx: mpsc::UnboundedSender<UiMsg>) -> bool {
        match self.modal.clone() {
            Modal::Wizard => {
                wizard_mark_done();
                self.modal = if self.media.is_none() && self.manual_root.is_none() {
                    Modal::Picker
                } else {
                    Modal::None
                };
            }
            Modal::Picker => match code {
                KeyCode::Esc | KeyCode::F(6) | KeyCode::Char('s') => self.modal = Modal::None,
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.picker_sel + 1 < self.devices.len() {
                        self.picker_sel += 1;
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.picker_sel = self.picker_sel.saturating_sub(1);
                }
                KeyCode::Char('r') => self.rescan_devices(),
                KeyCode::Enter => self.confirm_picker(),
                _ => {}
            },
            Modal::ConfirmInstall => match code {
                KeyCode::Enter => self.start_install(tx),
                KeyCode::Esc => self.modal = Modal::None,
                _ => {}
            },
            Modal::Progress => match code {
                KeyCode::Esc | KeyCode::Enter => {
                    let finished = self.prog.as_ref().map(|p| p.finished.is_some()).unwrap_or(false);
                    if finished {
                        self.modal = Modal::None;
                        self.prog = None;
                    } else {
                        self.cancel.store(true, Ordering::Relaxed);
                        self.log("cancelando — a mídia não será tocada além do rollback");
                    }
                }
                _ => {}
            },
            Modal::Notify(_) => self.modal = Modal::None,
            Modal::PickPnach => match code {
                KeyCode::Esc => self.modal = Modal::None,
                KeyCode::Down | KeyCode::Char('j') => {
                    let b = &mut self.dir_browser;
                    move_sel(&mut b.selected, b.entries.len(), 1);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let b = &mut self.dir_browser;
                    move_sel(&mut b.selected, b.entries.len(), -1);
                }
                KeyCode::Enter => {
                    self.dir_browser.enter();
                }
                KeyCode::Backspace => self.dir_browser.parent(),
                KeyCode::Char('c') => self.run_cheats_current(tx),
                _ => {}
            },
            Modal::None => return self.on_key_main(code, tx).await,
        }
        false
    }

    async fn on_key_main(&mut self, code: KeyCode, tx: mpsc::UnboundedSender<UiMsg>) -> bool {
        match code {
            KeyCode::Char('q') => {
                if self.busy {
                    self.log("operação em andamento — cancele com ESC na janela de progresso");
                    return false;
                }
                return true;
            }
            KeyCode::Esc => {
                if self.busy {
                    self.log("operação em andamento — cancele com ESC na janela de progresso");
                    return false;
                }
                return true;
            }
            KeyCode::F(6) | KeyCode::Char('s') => self.modal = Modal::Picker,
            KeyCode::Char('d') => self.run_diag(tx),
            KeyCode::Char('r') => {
                if !self.busy {
                    self.rescan_devices();
                    self.rescan();
                }
            }
            KeyCode::F(n) => {
                if let Some(t) = Tab::from_index(n.saturating_sub(1) as usize) {
                    self.tab = t;
                }
            }
            _ => match self.tab {
                Tab::Library => match code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.selected + 1 < self.games.len() {
                            self.selected += 1;
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.selected = self.selected.saturating_sub(1);
                    }
                    KeyCode::Char('c') => self.compress_selected(tx).await,
                    KeyCode::Char('a') => self.covers(tx),
                    KeyCode::Enter => self.game_info(),
                    _ => {}
                },
                Tab::Install => match code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        let b = &mut self.browser;
                        move_sel(&mut b.selected, b.entries.len(), 1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        let b = &mut self.browser;
                        move_sel(&mut b.selected, b.entries.len(), -1);
                    }
                    KeyCode::Enter | KeyCode::Char('l') => {
                        if !self.browser.enter() && self.browser.selected_file().is_some() {
                            self.modal = Modal::ConfirmInstall;
                        }
                    }
                    KeyCode::Backspace | KeyCode::Char('h') => self.browser.parent(),
                    KeyCode::Char('z') => self.toggle_install_opt(0),
                    KeyCode::Char('x') => self.toggle_install_opt(1),
                    KeyCode::Char('f') => self.toggle_install_opt(2),
                    KeyCode::Char('u') => self.toggle_install_opt(3),
                    KeyCode::Char('g') if self.browser.selected_file().is_some() => {
                        self.modal = Modal::ConfirmInstall;
                    }
                    _ => {}
                },
                Tab::BootKit => match code {
                    KeyCode::Down | KeyCode::Char('j') => self.bk_sel = (self.bk_sel + 1).min(4),
                    KeyCode::Up | KeyCode::Char('k') => self.bk_sel = self.bk_sel.saturating_sub(1),
                    KeyCode::Char(' ') => self.toggle_bk(),
                    KeyCode::Enter => self.run_bootkit(tx),
                    _ => {}
                },
                Tab::Extras => match code {
                    KeyCode::Down | KeyCode::Char('j') => self.extras_sel = (self.extras_sel + 1).min(4),
                    KeyCode::Up | KeyCode::Char('k') => self.extras_sel = self.extras_sel.saturating_sub(1),
                    KeyCode::Enter => self.run_extras(self.extras_sel, tx).await,
                    KeyCode::Char('g') => self.inst_gsm = if self.inst_gsm == 0 { 2 } else { 0 },
                    KeyCode::Char('1') => self.run_extras(0, tx).await,
                    KeyCode::Char('2') => self.run_extras(1, tx).await,
                    KeyCode::Char('3') => self.run_extras(2, tx).await,
                    KeyCode::Char('4') => self.run_extras(3, tx).await,
                    KeyCode::Char('5') => self.run_extras(4, tx).await,
                    _ => {}
                },
                Tab::Help => {}
            },
        }
        false
    }

    async fn on_mouse(&mut self, m: MouseEvent, tx: mpsc::UnboundedSender<UiMsg>) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let pos = Position { x: m.column, y: m.row };
                let hits = std::mem::take(&mut self.hits);
                for (r, hit) in hits {
                    if r.contains(pos) {
                        self.on_hit(hit, tx).await;
                        break;
                    }
                }
            }
            MouseEventKind::ScrollUp => self.scroll(-3),
            MouseEventKind::ScrollDown => self.scroll(3),
            _ => {}
        }
    }

    async fn on_hit(&mut self, hit: Hit, tx: mpsc::UnboundedSender<UiMsg>) {
        match hit {
            Hit::Tab(i) => {
                if let Some(t) = Tab::from_index(i) {
                    self.tab = t;
                }
            }
            Hit::DeviceChip => self.modal = Modal::Picker,
            Hit::GameRow(i) => {
                if i < self.games.len() {
                    self.selected = i;
                }
            }
            Hit::QuickAction(i) => match i {
                0 => self.compress_selected(tx).await,
                1 => self.covers(tx),
                2 => self.run_cfgs(tx),
                3 => self.run_ulcfg(tx),
                4 => {
                    if !self.busy {
                        self.rescan_devices();
                        self.rescan();
                    }
                }
                5 => self.modal = Modal::Picker,
                6 => self.run_diag(tx),
                _ => {}
            },
            Hit::PickerRow(i) => {
                if i < self.devices.len() {
                    self.picker_sel = i;
                }
            }
            Hit::PickerConfirm => self.confirm_picker(),
            Hit::BrowserRow(i) => {
                if i < self.browser.entries.len() {
                    self.browser.selected = i;
                }
            }
            Hit::BrowserQuick(i) => {
                let target = self.browser.quick.get(i).map(|(_, p)| p.clone());
                if let Some(p) = target {
                    self.browser.cd(p);
                }
            }
            Hit::InstToggle(i) => self.toggle_install_opt(i),
            Hit::InstGo => {
                if self.browser.selected_file().is_some() {
                    self.modal = Modal::ConfirmInstall;
                }
            }
            Hit::BkRow(i) => {
                self.bk_sel = i.min(4);
                self.toggle_bk();
            }
            Hit::BkGo => self.run_bootkit(tx),
            Hit::ExtraAction(i) => self.run_extras(i, tx).await,
            Hit::ExtraToggle => self.inst_gsm = if self.inst_gsm == 0 { 2 } else { 0 },
            Hit::WizardOk => {
                wizard_mark_done();
                self.modal = if self.media.is_none() && self.manual_root.is_none() {
                    Modal::Picker
                } else {
                    Modal::None
                };
            }
            Hit::NotifyOk => self.modal = Modal::None,
            Hit::ConfirmYes => self.start_install(tx),
            Hit::ConfirmNo => self.modal = Modal::None,
            Hit::ProgressCancel => {
                let finished = self.prog.as_ref().map(|p| p.finished.is_some()).unwrap_or(false);
                if finished {
                    self.modal = Modal::None;
                    self.prog = None;
                } else {
                    self.cancel.store(true, Ordering::Relaxed);
                }
            }
            Hit::BrowserDirRow(i) => {
                if i < self.dir_browser.entries.len() {
                    self.dir_browser.selected = i;
                }
            }
            Hit::BrowserDirGo => self.run_cheats_current(tx),
        }
    }

    fn scroll(&mut self, delta: i32) {
        match self.modal {
            Modal::Picker => move_sel(&mut self.picker_sel, self.devices.len(), delta),
            Modal::PickPnach => {
                let b = &mut self.dir_browser;
                move_sel(&mut b.selected, b.entries.len(), delta);
            }
            _ => match self.tab {
                Tab::Library => move_sel(&mut self.selected, self.games.len(), delta),
                Tab::Install => {
                    let b = &mut self.browser;
                    move_sel(&mut b.selected, b.entries.len(), delta);
                }
                Tab::BootKit => move_sel(&mut self.bk_sel, 5, delta),
                Tab::Extras => move_sel(&mut self.extras_sel, 5, delta),
                Tab::Help => {}
            },
        }
    }
}

fn move_sel(sel: &mut usize, len: usize, delta: i32) {
    if len == 0 {
        *sel = 0;
        return;
    }
    let mut v = *sel as i32 + delta;
    let max = len as i32 - 1;
    if v < 0 {
        v = 0;
    }
    if v > max {
        v = max;
    }
    *sel = v as usize;
}

fn trunc(s: &str, n: usize) -> String {
    crate::cli::trunc(s, n)
}

// ─────────────────────────────── UI ───────────────────────────────────

fn ui(f: &mut ratatui::Frame, app: &mut App) {
    app.hits.clear();
    let area = f.area();
    if area.width == 0 || area.height == 0 {
        // terminal sem dimensões (minimizado/pty 0×0) — nada a desenhar
        return;
    }

    let mut constraints = vec![
        Constraint::Length(3), // header
        Constraint::Length(1), // tab bar
        Constraint::Min(6),    // main
    ];
    if app.progress.is_some() {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Length(7)); // logs
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_header(f, v[0], app);
    render_tabbar(f, v[1], app);

    match app.tab {
        Tab::Library => render_library(f, v[2], app),
        Tab::Install => render_install(f, v[2], app),
        Tab::BootKit => render_bootkit(f, v[2], app),
        Tab::Extras => render_extras(f, v[2], app),
        Tab::Help => render_help(f, v[2], app),
    }

    let mut next = 3;
    if let Some((label, done, total)) = app.progress.clone() {
        let ratio = if total > 0 { (done as f64 / total as f64).clamp(0.0, 1.0) } else { 0.0 };
        let lbl = if total > 0 {
            format!("{label} {:.1}% ({} / {} MB)", ratio * 100.0, done / 1048576, total / 1048576)
        } else {
            format!("{label} {} MB", done / 1048576)
        };
        let gauge = Gauge::default()
            .ratio(ratio)
            .label(lbl)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(gauge, v[next]);
        next += 1;
    }

    let logs: Vec<String> = app.logs.iter().rev().take(5).rev().cloned().collect();
    let log_widget = Paragraph::new(logs.join("\n")).block(
        Block::default().borders(Borders::ALL).title("Log"),
    );
    f.render_widget(log_widget, v[next]);

    // modais por cima de tudo
    match app.modal {
        Modal::None => {}
        Modal::Picker => modals::picker(f, app),
        Modal::Wizard => modals::wizard(f, app),
        Modal::ConfirmInstall => modals::confirm_install(f, app),
        Modal::Progress => modals::progress(f, app),
        Modal::Notify(_) => modals::notify(f, app),
        Modal::PickPnach => modals::pick_pnach(f, app),
    }
}

fn render_header(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(24), Constraint::Length(44)])
        .split(area);

    let status = if app.busy { "ocupado" } else { "pronto" };
    let title = Paragraph::new(format!(
        " PS2 Manager v{} — {status}",
        env!("CARGO_PKG_VERSION")
    ))
    .style(Style::default().add_modifier(Modifier::BOLD))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, h[0]);

    // chip da mídia selecionada (clicável → abre o picker)
    let (chip_text, chip_style) = match &app.media {
        Some(m) => (
            format!(
                " {} · {} ",
                trunc(&m.device, 24),
                m.fs.label()
            ),
            if m.fs.bdm_recommended() {
                Style::default().fg(Color::Green)
            } else if m.fs == FsClass::Fat32 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            },
        ),
        None => (
            match &app.manual_root {
                Some(p) => format!(" manual: {} ", trunc(&p.display().to_string(), 26)),
                None => " [F6] selecionar mídia ".to_string(),
            },
            Style::default().fg(Color::DarkGray),
        ),
    };
    let chip = Paragraph::new(chip_text).style(chip_style).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Mídia")
            .border_style(chip_style),
    );
    f.render_widget(chip, h[1]);
    app.hits.push((h[1], Hit::DeviceChip));
}

fn render_tabbar(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let mut spans = Vec::new();
    let mut x = area.x;
    for (i, label) in TAB_LABELS.iter().enumerate() {
        let sel = i == app.tab.idx();
        let style = if sel {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let text = format!(" {label} ");
        let w = text.chars().count() as u16;
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
        app.hits.push((
            Rect { x, y: area.y, width: w, height: 1 },
            Hit::Tab(i),
        ));
        x += w + 1;
    }
    let hint = Span::styled(
        "  F6 mídia · d diagnóstico · r rescan · q sair",
        Style::default().fg(Color::DarkGray),
    );
    spans.push(hint);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_library(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(36)])
        .split(area);

    let header_row = Row::new([
        "GAME ID", "Título", "Tam", "Fmt", "Mídia", "Fragmentação",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = app.games.iter().map(|g| {
        Row::new([
            g.game_id.clone(),
            trunc(&g.title, 42),
            crate::cli::fmt_size(g.size),
            g.format.clone(),
            g.media.as_str().to_string(),
            g.fragmentation.clone(),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Min(18),
            Constraint::Length(9),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(14),
        ],
    )
    .header(header_row)
    .block(Block::default().borders(Borders::ALL).title("Biblioteca OPL"));
    app.table_state.select(Some(app.selected));
    f.render_stateful_widget(table, h[0], &mut app.table_state);

    // hits das linhas visíveis (offset pós-render)
    let inner = modals::inner_rect(h[0]);
    let off = *app.table_state.offset_mut();
    for i in 0..app.games.len() {
        let vy = inner.y + 1 + (i as u16).saturating_sub(off as u16);
        if vy >= inner.y + inner.height {
            break;
        }
        app.hits.push((
            Rect { x: inner.x, y: vy, width: inner.width, height: 1 },
            Hit::GameRow(i),
        ));
    }

    // painel direito: mídia + ações rápidas
    let r = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(0)])
        .split(h[1]);

    let media_lines: Vec<Line> = match &app.media {
        Some(m) => vec![
            Line::from(format!(" {} ", trunc(&m.device, 26))),
            Line::from(format!(" /dev/{} · {}", m.dev, m.fs_type)),
            Line::from(format!(" {}", trunc(&m.mount.display().to_string(), 30))),
            Line::from(format!(
                " livre {} de {}",
                crate::cli::fmt_size(m.free),
                crate::cli::fmt_size(m.total)
            )),
        ],
        None => vec![Line::from(" nenhuma mídia selecionada"), Line::from(" [F6] escolher")],
    };
    let media_panel = Paragraph::new(media_lines)
        .style(Style::default().add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title("Mídia ativa"));
    f.render_widget(media_panel, r[0]);

    let actions = [
        "[C] comprimir p/ ZSO",
        "[A] baixar capas",
        "[G] gerar CFGs",
        "[U] reconstruir ul.cfg",
        "[R] rescan",
        "[F6] trocar de mídia",
        "[D] diagnóstico completo",
    ];
    let items: Vec<ListItem> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let sel = i == app.selected.min(actions.len() - 1);
            ListItem::new(Line::from(vec![
                Span::from(if sel { "▶ " } else { "  " }),
                Span::raw(*a),
            ]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default().borders(Borders::ALL).title("Ações rápidas (mouse ok)"),
    );
    f.render_widget(list, r[1]);
    let ainner = modals::inner_rect(r[1]);
    for (i, _) in actions.iter().enumerate() {
        let vy = ainner.y + i as u16;
        if vy >= ainner.y + ainner.height {
            break;
        }
        app.hits.push((
            Rect { x: ainner.x, y: vy, width: ainner.width, height: 1 },
            Hit::QuickAction(i),
        ));
    }
}

fn render_install(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(38)])
        .split(area);

    // navegador de ISOs (PC) — atalhos + lista
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(h[0]);

    let mut quick_spans = Vec::new();
    let mut x = modals::inner_rect(left[0]).x;
    for (i, (label, _)) in app.browser.quick.iter().enumerate() {
        let text = format!(" ⌂ {label} ");
        let w = text.chars().count() as u16;
        quick_spans.push(Span::styled(text, Style::default().fg(Color::Cyan)));
        quick_spans.push(Span::raw("  "));
        app.hits.push((
            Rect { x, y: modals::inner_rect(left[0]).y, width: w, height: 1 },
            Hit::BrowserQuick(i),
        ));
        x += w + 2;
    }
    let quick = Paragraph::new(Line::from(quick_spans)).block(
        Block::default().borders(Borders::ALL).title("Atalhos"),
    );
    f.render_widget(quick, left[0]);

    modals::render_browser_in(f, left[1], app, false);

    // painel de opções
    let o = app.inst_opts;
    let file = app.browser.selected_file();
    let (fname, fsize) = match &file {
        Some(p) => (
            trunc(
                &p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                24,
            ),
            std::fs::metadata(p).map(|m| crate::cli::fmt_size(m.len())).unwrap_or_default(),
        ),
        None => ("—".to_string(), String::new()),
    };
    let toggle = |on: bool| if on { "[x]" } else { "[ ]" };
    let ready = app.media.is_some() && file.is_some();

    let lines = vec![
        Line::from(format!(" Arquivo : {fname} {fsize}")),
        Line::from(""),
        Line::from(format!(" {} Converter para ZSO (LZ4)  [Z]", toggle(o.as_zso))),
        Line::from(format!(" {} Smart Shrink antes        [X]", toggle(o.pre_shrink))),
        Line::from(format!(" {} fallocate (contíguo)      [F]", toggle(o.fallocate))),
        Line::from(format!(" {} USBExtreme ul.* (legado)  [U]", toggle(o.split_ul))),
        Line::from(""),
        match &app.media {
            Some(m) => Line::from(format!(
                " Destino : {} — {} — {} livres",
                trunc(&m.device, 18),
                m.fs.label(),
                crate::cli::fmt_size(m.free)
            )),
            None => Line::from(" Destino : nenhuma mídia — [F6]"),
        },
        Line::from(""),
        Line::from(if ready {
            " ▸ INSTALAR AGORA (ENTER) ◂"
        } else {
            " ▸ selecione arquivo e mídia ◂"
        }),
        Line::from(""),
        Line::from(Span::styled(
            " staging no SSD local (~/.cache/ps2-manager):",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            " a mídia só é tocada com o arquivo validado ✓",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let panel = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Instalação em staging (SSD → mídia)")
            .border_style(Style::default().fg(Color::Green)),
    );
    f.render_widget(panel, h[1]);

    let inner = modals::inner_rect(h[1]);
    for i in 0..4 {
        app.hits.push((
            Rect { x: inner.x, y: inner.y + 2 + i as u16, width: inner.width, height: 1 },
            Hit::InstToggle(i),
        ));
    }
    if ready {
        app.hits.push((
            Rect { x: inner.x, y: inner.y + 9, width: inner.width, height: 1 },
            Hit::InstGo,
        ));
    }
}

fn render_bootkit(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let inner_area = area;
    let o = app.bk_opts;
    let opts: [(usize, bool, &str); 5] = [
        (0, o.include_opl, "Incluir OPL-Mod-PTBR (compilação recente)"),
        (1, o.include_wle, "Incluir wLaunchELF_ISR (exFAT/BDM/MX4SIO)"),
        (2, o.include_configs, "Incluir atalhos no FREMCB.CNF / CONFIG.INI"),
        (3, o.include_library, "Baixar Biblioteca da Cena (EXTRAS → /BIBLIOTECA)"),
        (4, o.overwrite, "Sobrescrever configurações existentes na mídia"),
    ];
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(format!(" Kit    : {}", app.bk_kit.0)));
    lines.push(Line::from(format!(" Spec   : {}", app.bk_kit.1)));
    lines.push(Line::from(" Origem : GitHub releases (SHA256 verificado)"));
    lines.push(Line::from(""));
    for (idx, on, label) in opts {
        let sel = idx == app.bk_sel;
        let mark = if on { "[x]" } else { "[ ]" };
        let mark_style = if on {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut spans = vec![
            Span::from(if sel { "▶ " } else { "  " }),
            Span::styled(mark, mark_style),
            Span::from(format!(" {label}")),
        ];
        if sel {
            spans.push(Span::styled("  ←", Style::default().fg(Color::Yellow)));
        }
        let line = if sel {
            Line::from(spans).style(Style::default().add_modifier(Modifier::BOLD))
        } else {
            Line::from(spans)
        };
        lines.push(line);
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        " Destino: {}/BOOT_MC",
        trunc(&app.root.display().to_string(), 40)
    )));
    if o.include_library {
        lines.push(Line::from(format!(
            "          + {}/BIBLIOTECA (acervo opcional)",
            trunc(&app.root.display().to_string(), 40)
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " [ESPAÇO] alterna · [ENTER] gerar kit · progresso no rodapé",
        Style::default().fg(Color::DarkGray),
    )));
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" DEPLOY DE BOOT ESSENCIAL ")
            .border_style(Style::default().fg(Color::Green)),
    );
    let inner = modals::inner_rect(inner_area);
    f.render_widget(p, inner_area);
    for i in 0..5 {
        app.hits.push((
            Rect { x: inner.x, y: inner.y + 4 + i as u16, width: inner.width, height: 1 },
            Hit::BkRow(i),
        ));
    }
    app.hits.push((
        Rect { x: inner.x, y: inner.y + 12, width: inner.width, height: 1 },
        Hit::BkGo,
    ));
}

fn render_extras(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let items: [(&str, bool); 5] = [
        ("Baixar capas (GameTDB + xlenore)", false),
        ("Converter PNACH → CHT (escolher pasta)", false),
        ("Gerar CFG/<ID>.cfg para todos os jogos", false),
        ("Reconstruir ul.cfg (USBExtreme/FAT32)", false),
        ("Diagnóstico completo — testa tudo e grava relatório", false),
    ];
    let mut lines: Vec<Line> = Vec::new();
    for (i, (label, _)) in items.iter().enumerate() {
        let sel = i == app.extras_sel;
        lines.push(Line::from(vec![
            Span::from(if sel { "▶ " } else { "  " }),
            Span::from(format!("[{}] {label}", i + 1)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        " GSM nos CFGs gerados: [{}]   [G] alterna",
        if app.inst_gsm == 0 { "desligado" } else { "480p (vmode 2)" }
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ + ENTER · clique com o mouse · [G] alterna GSM",
        Style::default().fg(Color::DarkGray),
    )));
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" CAPAS, CHEATS & CFG ")
            .border_style(Style::default().fg(Color::Green)),
    );
    f.render_widget(p, area);
    let inner = modals::inner_rect(area);
    for i in 0..5 {
        app.hits.push((
            Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 },
            Hit::ExtraAction(i),
        ));
    }
    app.hits.push((
        Rect { x: inner.x, y: inner.y + 5, width: inner.width, height: 1 },
        Hit::ExtraToggle,
    ));
}

fn render_help(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let _ = app;
    let lines = vec![
        Line::from(Span::styled("INÍCIO RÁPIDO", Style::default().add_modifier(Modifier::BOLD))),
        Line::from("  Dois cliques no AppImage (ou ./ps2-manager) abrem a UI direto —"),
        Line::from("  sem terminal aberto, o app lança um automaticamente. Nada de comandos."),
        Line::from("  F1 jogos · F2 instalar ISO · F3 boot kit · F4 extras · F5 ajuda"),
        Line::from(""),
        Line::from(Span::styled("TECLAS", Style::default().add_modifier(Modifier::BOLD))),
        Line::from("  F1–F5   abas · F6/s seletor de mídia · r rescan · q sair"),
        Line::from("  ↑↓/jk   navegar · ENTER confirmar · ESC cancelar/voltar"),
        Line::from("  Backspace  pasta anterior (navegador) · ESPAÇO alterna toggles"),
        Line::from("  Z/X/F    toggles de instalação · G GSM · C ZSO · A capas · D diagnóstico"),
        Line::from(""),
        Line::from(Span::styled("DIAGNÓSTICO (tecla D)", Style::default().add_modifier(Modifier::BOLD))),
        Line::from("  Testa TODOS os subsistemas numa sandbox temporária: parser ISO,"),
        Line::from("  instalação staging→ZSO, ul.cfg/CFG, capas e GitHub. Coloque uma ISO"),
        Line::from("  na mesma pasta do AppImage para testá-la; sem ISO, um disco sintético"),
        Line::from("  interno é usado. Relatório TXT em ~/.local/share/ps2-manager/reports/"),
        Line::from(""),
        Line::from(Span::styled("LOGS", Style::default().add_modifier(Modifier::BOLD))),
        Line::from("  Toda operação é registrada em ~/.local/share/ps2-manager/logs/"),
        Line::from("  (rotaciona a 2 MB). Envie o arquivo ao reportar qualquer problema."),
        Line::from(""),
        Line::from(Span::styled("MOUSE", Style::default().add_modifier(Modifier::BOLD))),
        Line::from("  Clique em abas, toggles, botões, chip da mídia e linhas."),
        Line::from("  A roda faz scroll nas listas."),
        Line::from(""),
        Line::from(Span::styled("SEGURANÇA (Mega Prompt 7)", Style::default().add_modifier(Modifier::BOLD))),
        Line::from("  • Discos internos do PC NUNCA aparecem como destino."),
        Line::from("  • A conversão ZSO acontece no seu SSD (staging)."),
        Line::from("  • A mídia só é gravada com o arquivo final validado."),
        Line::from("  • Cancelamento faz rollback — nada parcial fica na mídia."),
        Line::from(""),
        Line::from(Span::styled("SOBRE", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!(
            "  ps2-manager v{} — Rust + Ratatui, binário estático (musl).",
            env!("CARGO_PKG_VERSION")
        )),
        Line::from("  github.com/Chrispsz/ps2-manager · docs/BOOT_KIT.md"),
    ];
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Ajuda "))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

// ────────────────────────────── event loop ────────────────────────────

fn useful_mouse(m: &MouseEvent) -> bool {
    matches!(
        m.kind,
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
    )
}

/// Lança a TUI num emulador de terminal quando o app foi aberto sem um
/// (dois cliques no binário cru / gerenciador de arquivos). Procura os
/// terminais mais comuns; se nenhum existir, tenta aviso gráfico (zenity).
fn spawn_terminal_self() -> Option<std::process::Child> {
    #[derive(Clone, Copy)]
    enum Style {
        /// `term -e cmd`
        E,
        /// `term -- cmd`
        Dashes,
        /// `term -x cmd`
        X,
        /// `term cmd`
        Bare,
    }
    const CANDIDATES: &[(&str, Style)] = &[
        ("x-terminal-emulator", Style::E),
        ("gnome-terminal", Style::Dashes),
        ("konsole", Style::E),
        ("alacritty", Style::E),
        ("kitty", Style::Bare),
        ("foot", Style::Bare),
        ("xfce4-terminal", Style::E),
        ("terminator", Style::E),
        ("lxterminal", Style::E),
        ("mate-terminal", Style::X),
        ("tilix", Style::E),
        ("xterm", Style::E),
    ];
    let exe = std::env::current_exe().ok()?;
    for (name, style) in CANDIDATES {
        let mut cmd = std::process::Command::new(name);
        match style {
            Style::E => cmd.arg("-e"),
            Style::Dashes => cmd.arg("--"),
            Style::X => cmd.arg("-x"),
            Style::Bare => &mut cmd,
        };
        if let Ok(child) = cmd.arg(&exe).spawn() {
            crate::logging::info(&format!("sem TTY — TUI lançada em {name}"));
            return Some(child);
        }
    }
    let msg = "PS2 Manager precisa de um terminal para abrir a interface. \
               Instale um emulador (gnome-terminal, konsole, alacritty, xterm…).";
    crate::logging::error("sem TTY e nenhum emulador de terminal encontrado");
    for (dlg, pre) in [
        ("zenity", &["--error", "--text"][..]),
        ("kdialog", &["--error"][..]),
    ] {
        let mut c = std::process::Command::new(dlg);
        for a in pre {
            c.arg(a);
        }
        if c.arg(msg).spawn().is_ok() {
            break;
        }
    }
    None
}

pub async fn run_tui(manual_root: Option<PathBuf>) -> Result<()> {
    use crossterm::tty::IsTty;
    if !std::io::stdout().is_tty() {
        // aberto direto do sistema (2 cliques) — lança um terminal com a UI
        if spawn_terminal_self().is_some() {
            return Ok(());
        }
        return Err(crate::error::Ps2Error::Other(
            "TUI requer terminal interativo — nenhum emulador encontrado para abrir \
             automaticamente. Rode dentro de um terminal real (ex.: gnome-terminal)."
                .into(),
        ));
    }
    // panics são registrados no log e o terminal é restaurado antes de sair
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        crate::logging::error(&format!("PANIC: {info}"));
        ratatui::restore();
        default_hook(info);
    }));
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    let res = tui_loop(&mut terminal, manual_root).await;
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    SHUTDOWN.store(true, Ordering::Relaxed);
    res
}

async fn tui_loop(terminal: &mut ratatui::DefaultTerminal, manual_root: Option<PathBuf>) -> Result<()> {
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<Event>();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<UiMsg>();

    tokio::task::spawn_blocking(move || loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }
        if let Ok(true) = crossterm::event::poll(std::time::Duration::from_millis(150)) {
            match crossterm::event::read() {
                Ok(ev) => {
                    if key_tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut app = App::new(manual_root);
    if wizard_pending() {
        app.modal = Modal::Wizard;
    }
    app.rescan_devices(); // auto-scan passivo (seleção automática de mídia única)
    if app.modal == Modal::None && app.media.is_none() && app.manual_root.is_none() {
        app.modal = Modal::Picker;
    }
    app.log("F1–F5 abas · F6 mídia · mouse: clique/scroll · d diagnóstico · q sair");
    if let Some(lp) = crate::logging::path() {
        app.log(format!("log em {}", lp.display()));
    }
    app.rescan();

    loop {
        terminal
            .draw(|f| ui(f, &mut app))
            .map_err(|e| crate::error::Ps2Error::Other(e.to_string()))?;
        tokio::select! {
            ev = key_rx.recv() => {
                match ev {
                    Some(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                        if app.on_key(k.code, msg_tx.clone()).await {
                            break;
                        }
                    }
                    Some(Event::Mouse(m)) if useful_mouse(&m) => {
                        app.on_mouse(m, msg_tx.clone()).await;
                    }
                    Some(Event::Resize(_, _)) => {}
                    Some(_) => continue, // não redesenha para eventos irrelevantes
                    None => break,
                }
            }
            msg = msg_rx.recv() => {
                match msg {
                    Some(UiMsg::Log(s)) => app.log(s),
                    Some(UiMsg::Games(g)) => {
                        app.games = g;
                        if app.selected >= app.games.len() {
                            app.selected = app.games.len().saturating_sub(1);
                        }
                    }
                    Some(UiMsg::NetProgress(p)) => app.progress = p,
                    Some(UiMsg::PipeProgress(p)) => {
                        if let Some(pr) = &mut app.prog {
                            pr.update(&p);
                        }
                    }
                    Some(UiMsg::Busy(b)) => app.busy = b,
                    Some(UiMsg::Notify(s)) => {
                        app.busy = false;
                        app.modal = Modal::Notify(s);
                    }
                    Some(UiMsg::InstallDone(r)) => {
                        app.busy = false;
                        if let Some(pr) = &mut app.prog {
                            pr.finished = Some(r.clone());
                        }
                        match &r {
                            Ok(s) => app.log(s.clone()),
                            Err(e) => app.log(format!("✗ instalação: {e}")),
                        }
                        app.rescan();
                    }
                    None => break,
                }
            }
        }
    }
    Ok(())
}
