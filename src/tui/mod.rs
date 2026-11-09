//! Dashboard TUI (Ratatui) — dispositivos, biblioteca, ações rápidas,
//! barra de progresso e log. Teclas:
//!   q/Esc sair · ↑/k ↓/j navegar · r rescan · c ZSO · a capas · d deploy

use crate::deploy::github::GitHubClient;
use crate::error::Result;
use crate::iso::zso::compress_iso;
use crate::opl::library::{self, GameEntry};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

enum UiMsg {
    Log(String),
    Games(Vec<GameEntry>),
    Progress(Option<(String, u64, u64)>), // (label, done, total; total=0 → indeterminado)
    Busy(bool),
}

/// Toggles do painel [F3] — Deploy de Boot Essencial.
#[derive(Clone, Copy)]
struct BkOpts {
    include_opl: bool,
    include_wle: bool,
    include_configs: bool,
    overwrite: bool,
}

impl Default for BkOpts {
    fn default() -> Self {
        Self {
            include_opl: true,
            include_wle: true,
            include_configs: false,
            overwrite: false,
        }
    }
}

struct App {
    root: PathBuf,
    games: Vec<GameEntry>,
    selected: usize,
    logs: VecDeque<String>,
    progress: Option<(String, u64, u64)>,
    busy: bool,
    table_state: ratatui::widgets::TableState,
    bk_open: bool,
    bk_sel: usize,
    bk_opts: BkOpts,
    bk_kit: (String, String),
}

impl App {
    fn new(root: PathBuf) -> Self {
        let (bk_name, bk_spec) = crate::deploy::bootkit::default_manifest()
            .map(|m| (m.meta.kit_name, m.meta.spec))
            .unwrap_or_else(|_| ("Boot Kit Essencial PS2".into(), "?".into()));
        Self {
            root,
            games: Vec::new(),
            selected: 0,
            logs: VecDeque::new(),
            progress: None,
            busy: false,
            table_state: ratatui::widgets::TableState::default(),
            bk_open: false,
            bk_sel: 0,
            bk_opts: BkOpts::default(),
            bk_kit: (bk_name, bk_spec),
        }
    }

    fn log(&mut self, s: impl Into<String>) {
        self.logs.push_back(s.into());
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

    fn spawn_ticker(&self, tx: mpsc::UnboundedSender<UiMsg>, counter: Arc<AtomicU64>, total: u64, label: &'static str) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let v = counter.load(Ordering::Relaxed);
                if tx.send(UiMsg::Progress(Some((label.to_string(), v, total)))).is_err() {
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
            let _ = tx2.send(UiMsg::Progress(None));
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

    fn deploy(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        self.busy = true;
        let token = std::env::var("GITHUB_TOKEN").ok();
        let tx2 = tx.clone();
        let root = self.root.clone();
        tokio::spawn(async move {
            let _ = tx2.send(UiMsg::Log("deploy: consultando GitHub…".into()));
            match GitHubClient::new(token) {
                Ok(client) => {
                    let counter = Arc::new(AtomicU64::new(0));
                    let txc = tx2.clone();
                    let c2 = counter.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            let v = c2.load(Ordering::Relaxed);
                            if txc.send(UiMsg::Progress(Some(("Download".into(), v, 0)))).is_err() {
                                break;
                            }
                            if SHUTDOWN.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    });
                    match crate::deploy::fmcb::deploy_boot_mc(
                        &root,
                        crate::deploy::github::DEFAULT_REPO,
                        &client,
                        counter,
                    )
                    .await
                    {
                        Ok(rep) => {
                            let _ = tx2.send(UiMsg::Log(format!("✓ deploy {} ({} arquivos) em BOOT_MC/", rep.tag, rep.files.len())));
                        }
                        Err(e) => {
                            let _ = tx2.send(UiMsg::Log(format!("✗ deploy falhou: {e}")));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx2.send(UiMsg::Log(format!("✗ cliente GitHub: {e}")));
                }
            }
            let _ = tx2.send(UiMsg::Progress(None));
            let _ = tx2.send(UiMsg::Busy(false));
        });
        self.log("deploy BOOT_MC iniciado…");
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
            match GitHubClient::new(token) {
                Ok(client) => {
                    let counter = Arc::new(AtomicU64::new(0));
                    let txc = tx2.clone();
                    let c2 = counter.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            let v = c2.load(Ordering::Relaxed);
                            if txc.send(UiMsg::Progress(Some(("Boot Kit".into(), v, 0)))).is_err() {
                                break;
                            }
                            if SHUTDOWN.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    });
                    match crate::deploy::bootkit::assemble(&root, &manifest, &client, counter, opts.overwrite).await {
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
            let _ = tx2.send(UiMsg::Progress(None));
            let _ = tx2.send(UiMsg::Busy(false));
        });
        self.log("boot kit: montagem de /BOOT_MC/ iniciada…");
    }

    fn covers(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        let tx2 = tx.clone();
        let root = self.root.clone();
        tokio::spawn(async move {
            let _ = tx2.send(UiMsg::Log("capas: consultando GameTDB…".into()));
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

    async fn on_key(&mut self, code: KeyCode, tx: mpsc::UnboundedSender<UiMsg>) -> bool {
        if self.bk_open {
            match code {
                KeyCode::Esc | KeyCode::F(3) => {
                    self.bk_open = false;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.bk_sel = (self.bk_sel + 1).min(3);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.bk_sel = self.bk_sel.saturating_sub(1);
                }
                KeyCode::Char(' ') => match self.bk_sel {
                    0 => self.bk_opts.include_opl = !self.bk_opts.include_opl,
                    1 => self.bk_opts.include_wle = !self.bk_opts.include_wle,
                    2 => self.bk_opts.include_configs = !self.bk_opts.include_configs,
                    _ => self.bk_opts.overwrite = !self.bk_opts.overwrite,
                },
                KeyCode::Enter => self.run_bootkit(tx),
                _ => {}
            }
            return false;
        }
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.games.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Char('r') => {
                if !self.busy {
                    self.rescan();
                }
            }
            KeyCode::Char('c') => self.compress_selected(tx).await,
            KeyCode::Char('d') => self.deploy(tx),
            KeyCode::Char('a') => self.covers(tx),
            KeyCode::F(3) if !self.busy => {
                self.bk_open = true;
            }
            _ => {}
        }
        false
    }
}

fn trunc(s: &str, n: usize) -> String {
    crate::cli::trunc(s, n)
}

fn centered_rect(pct_x: u16, pct_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let v = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Percentage((100 - pct_y) / 2),
            ratatui::layout::Constraint::Percentage(pct_y),
            ratatui::layout::Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(r);
    ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            ratatui::layout::Constraint::Percentage((100 - pct_x) / 2),
            ratatui::layout::Constraint::Percentage(pct_x),
            ratatui::layout::Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}

fn bk_popup(app: &App) -> ratatui::widgets::Paragraph<'static> {
    use ratatui::style::Color;
    use ratatui::style::Modifier;
    use ratatui::style::Style;
    use ratatui::text::Line;
    use ratatui::text::Span;

    let o = app.bk_opts;
    let opts: [(usize, bool, &str); 4] = [
        (0, o.include_opl, "Incluir OPL-Mod-PTBR (compilação recente)"),
        (1, o.include_wle, "Incluir wLaunchELF_ISR (exFAT/BDM)"),
        (2, o.include_configs, "Incluir atalhos no FREMCB.CNF / CONFIG.INI"),
        (3, o.overwrite, "Sobrescrever configurações existentes na mídia"),
    ];
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(format!("Kit    : {}", app.bk_kit.0)));
    lines.push(Line::from(format!("Spec   : {}", app.bk_kit.1)));
    lines.push(Line::from("Origem : Chrispsz/OPL-Mod-PTBR (release oficial)"));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Opções de injeção:",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for (idx, on, label) in opts {
        let sel = idx == app.bk_sel;
        let mark = if on { "[x]" } else { "[ ]" };
        let mark_style = if on {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let spans = vec![
            Span::from(if sel { "▶ " } else { "  " }),
            Span::styled(mark, mark_style),
            Span::from(format!(" {label}")),
        ];
        let line = if sel {
            Line::from(spans).style(Style::default().add_modifier(Modifier::BOLD))
        } else {
            Line::from(spans)
        };
        lines.push(line);
    }
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "Destino: {}/BOOT_MC",
        app.root.display()
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓ seleciona · [ESPAÇO] alterna · [ENTER] gerar · [F3] fecha",
        Style::default().fg(Color::DarkGray),
    )));
    ratatui::widgets::Paragraph::new(lines).block(
        ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title(" DEPLOY DE BOOT ESSENCIAL ")
            .border_style(Style::default().fg(Color::Green)),
    )
}

fn ui(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();
    let mut vchunks = vec![
        ratatui::layout::Constraint::Length(3),
        ratatui::layout::Constraint::Min(4),
    ];
    if app.progress.is_some() {
        vchunks.push(ratatui::layout::Constraint::Length(3));
    }
    vchunks.push(ratatui::layout::Constraint::Length(7));
    let v = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints(vchunks)
        .split(area);

    let status = if app.busy { "ocupado" } else { "pronto" };
    let header = ratatui::widgets::Paragraph::new(format!(
        " PS2 Manager — {} — {status}",
        app.root.display()
    ))
    .style(ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::BOLD))
    .block(ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL));
    f.render_widget(header, v[0]);

    let h = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            ratatui::layout::Constraint::Min(40),
            ratatui::layout::Constraint::Length(30),
        ])
        .split(v[1]);

    let header_row = ratatui::widgets::Row::new([
        ratatui::widgets::Cell::from("GAME ID"),
        ratatui::widgets::Cell::from("Título"),
        ratatui::widgets::Cell::from("Tam"),
        ratatui::widgets::Cell::from("Fmt"),
        ratatui::widgets::Cell::from("Mídia"),
        ratatui::widgets::Cell::from("Fragmentação"),
    ])
    .style(ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::BOLD));

    let rows = app.games.iter().map(|g| {
        ratatui::widgets::Row::new([
            ratatui::widgets::Cell::from(g.game_id.clone()),
            ratatui::widgets::Cell::from(trunc(&g.title, 42)),
            ratatui::widgets::Cell::from(crate::cli::fmt_size(g.size)),
            ratatui::widgets::Cell::from(g.format.clone()),
            ratatui::widgets::Cell::from(g.media.as_str().to_string()),
            ratatui::widgets::Cell::from(g.fragmentation.clone()),
        ])
    });
    let table = ratatui::widgets::Table::new(
        rows,
        [
            ratatui::layout::Constraint::Length(12),
            ratatui::layout::Constraint::Min(18),
            ratatui::layout::Constraint::Length(9),
            ratatui::layout::Constraint::Length(4),
            ratatui::layout::Constraint::Length(6),
            ratatui::layout::Constraint::Length(14),
        ],
    )
    .header(header_row)
    .block(
        ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title("Biblioteca OPL"),
    );
    app.table_state.select(Some(app.selected));
    f.render_stateful_widget(table, h[0], &mut app.table_state);

    let actions: Vec<ratatui::widgets::ListItem<'_>> = vec![
        ratatui::widgets::ListItem::new("↑/↓  navegar"),
        ratatui::widgets::ListItem::new("[C] comprimir ZSO"),
        ratatui::widgets::ListItem::new("[A] baixar capas"),
        ratatui::widgets::ListItem::new("[D] deploy FMCB PT-BR"),
        ratatui::widgets::ListItem::new("[F3] boot kit essencial"),
        ratatui::widgets::ListItem::new("[R] rescan"),
        ratatui::widgets::ListItem::new("[Q] sair"),
    ];
    let list = ratatui::widgets::List::new(actions).block(
        ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title("Ações rápidas"),
    );
    f.render_widget(list, h[1]);

    let mut next_chunk = 2;
    if let Some((label, done, total)) = app.progress.clone() {
        let ratio = if total > 0 { (done as f64 / total as f64).clamp(0.0, 1.0) } else { 0.0 };
        let lbl = if total > 0 {
            format!("{label} {:.1}% ({} / {} MB)", ratio * 100.0, done / 1048576, total / 1048576)
        } else {
            format!("{label} {} MB", done / 1048576)
        };
        let gauge = ratatui::widgets::Gauge::default()
            .ratio(ratio)
            .label(lbl)
            .block(ratatui::widgets::Block::default().borders(ratatui::widgets::Borders::ALL));
        f.render_widget(gauge, v[next_chunk]);
        next_chunk += 1;
    }

    let logs: Vec<String> = app.logs.iter().rev().take(5).rev().cloned().collect();
    let log_widget = ratatui::widgets::Paragraph::new(logs.join("\n")).block(
        ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .title("Log"),
    );
    f.render_widget(log_widget, v[next_chunk]);

    if app.bk_open {
        let popup = centered_rect(76, 58, area);
        f.render_widget(ratatui::widgets::Clear, popup);
        f.render_widget(bk_popup(app), popup);
    }
}

pub async fn run_tui(root: PathBuf) -> Result<()> {
    use crossterm::tty::IsTty;
    if !std::io::stdout().is_tty() {
        return Err(crate::error::Ps2Error::Other(
            "TUI requer terminal interativo (TTY) — use os subcomandos CLI (info/scan/install/…) \
             ou rode dentro de um terminal real."
                .into(),
        ));
    }
    let mut terminal = ratatui::init();
    let res = tui_loop(&mut terminal, root).await;
    ratatui::restore();
    SHUTDOWN.store(true, Ordering::Relaxed);
    res
}

async fn tui_loop(
    terminal: &mut ratatui::DefaultTerminal,
    root: PathBuf,
) -> Result<()> {
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<Event>();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<UiMsg>();

    tokio::task::spawn_blocking(move || {
        loop {
            if SHUTDOWN.load(Ordering::Relaxed) {
                break;
            }
            if let Ok(true) = crossterm::event::poll(Duration::from_millis(150)) {
                match crossterm::event::read() {
                    Ok(ev) => {
                        if key_tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    });

    let mut app = App::new(root);
    app.log("q sair · ↑↓ navegar · r rescan · c ZSO · a capas · d deploy PT-BR · F3 boot kit");
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
                    Some(_) => {}
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
                    Some(UiMsg::Progress(p)) => app.progress = p,
                    Some(UiMsg::Busy(b)) => app.busy = b,
                    None => break,
                }
            }
        }
    }
    Ok(())
}
