//! Modais e navegador de arquivos da TUI v2 (Mega Prompts 6/7).

use super::{App, Hit, Modal};
use crate::cli::{fmt_size, trunc};
use crate::core::storage::FsClass;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::widgets::ListState;
use std::fs;
use std::path::{Path, PathBuf};

// ───────────────────────── navegador de ISOs ──────────────────────────

pub enum BrowserEntry {
    Dir(String),
    File(String, u64),
}

pub struct Browser {
    pub path: PathBuf,
    pub entries: Vec<BrowserEntry>,
    pub selected: usize,
    pub quick: Vec<(String, PathBuf)>,
    pub state: ListState,
}

impl Browser {
    pub fn open(start: &Path) -> Self {
        let mut b = Self {
            path: start.to_path_buf(),
            entries: Vec::new(),
            selected: 0,
            quick: Vec::new(),
            state: ListState::default(),
        };
        b.build_quick();
        b.reload();
        b
    }

    pub fn cd(&mut self, p: PathBuf) {
        self.path = p;
        self.selected = 0;
        self.reload();
    }

    pub fn parent(&mut self) {
        if let Some(p) = self.path.parent() {
            if p != self.path {
                self.cd(p.to_path_buf());
            }
        }
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        if let Ok(rd) = fs::read_dir(&self.path) {
            let mut dirs: Vec<String> = Vec::new();
            let mut files: Vec<(String, u64)> = Vec::new();
            for e in rd.filter_map(|e| e.ok()) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                match e.file_type() {
                    Ok(ft) if ft.is_dir() => dirs.push(name),
                    Ok(ft) if ft.is_file() => {
                        let lower = name.to_lowercase();
                        if lower.ends_with(".iso") || lower.ends_with(".zso") {
                            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                            files.push((name, size));
                        }
                    }
                    _ => {}
                }
            }
            dirs.sort_by_key(|a| a.to_lowercase());
            files.sort_by_key(|a| a.0.to_lowercase());
            self.entries.push(BrowserEntry::Dir("..".into()));
            self.entries
                .extend(dirs.into_iter().map(BrowserEntry::Dir));
            self.entries
                .extend(files.into_iter().map(|(n, s)| BrowserEntry::File(n, s)));
        }
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    /// ENTER no item selecionado: entra em diretório (true) ou é arquivo (false).
    pub fn enter(&mut self) -> bool {
        match self.entries.get(self.selected) {
            Some(BrowserEntry::Dir(n)) if n == ".." => {
                self.parent();
                true
            }
            Some(BrowserEntry::Dir(n)) => {
                let mut p = self.path.clone();
                p.push(n);
                self.cd(p);
                true
            }
            _ => false,
        }
    }

    pub fn selected_file(&self) -> Option<PathBuf> {
        match self.entries.get(self.selected) {
            Some(BrowserEntry::File(n, _)) => {
                let mut p = self.path.clone();
                p.push(n);
                Some(p)
            }
            _ => None,
        }
    }

    fn build_quick(&mut self) {
        self.quick.clear();
        if let Ok(h) = std::env::var("HOME") {
            if h.is_empty() {
                return;
            }
            let home = PathBuf::from(h);
            self.quick.push(("Início".into(), home.clone()));
            for (label, rel) in [("Downloads", "Downloads"), ("Documentos", "Documents")] {
                let p = home.join(rel);
                if p.is_dir() {
                    self.quick.push((label.into(), p));
                }
            }
        }
    }
}

fn entry_item(e: &BrowserEntry) -> ListItem<'static> {
    match e {
        BrowserEntry::Dir(n) => ListItem::new(Line::from(Span::styled(
            format!("▸ {n}/"),
            Style::default().fg(Color::Cyan),
        ))),
        BrowserEntry::File(n, s) => ListItem::new(Line::from(vec![
            Span::raw("  "),
            Span::raw(n.clone()),
            Span::styled(
                format!("  {}", fmt_size(*s)),
                Style::default().fg(Color::DarkGray),
            ),
        ])),
    }
}

/// Renderiza `app.browser` (ou `app.dir_browser` em modo pasta) com hits.
pub fn render_browser_in(f: &mut ratatui::Frame, area: Rect, app: &mut App, dir_mode: bool) {
    let title = if dir_mode {
        format!(
            " Pasta de PNACH — {} ",
            trunc(&app.dir_browser.path.display().to_string(), 40)
        )
    } else {
        format!(
            " Navegador ISO — {} ",
            trunc(&app.browser.path.display().to_string(), 40)
        )
    };
    if !dir_mode {
        let items: Vec<ListItem> = app
            .browser
            .entries
            .iter()
            .map(entry_item)
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            )
            .block(Block::default().borders(Borders::ALL).title(title));
        app.browser.state.select(Some(app.browser.selected));
        f.render_stateful_widget(list, area, &mut app.browser.state);
        let inner = inner_rect(area);
        let off = *app.browser.state.offset_mut();
        for i in 0..app.browser.entries.len() {
            let vy = inner.y + (i as u16).saturating_sub(off as u16);
            if vy >= inner.y + inner.height {
                break;
            }
            app.hits.push((
                Rect { x: inner.x, y: vy, width: inner.width, height: 1 },
                Hit::BrowserRow(i),
            ));
        }
    } else {
        let items: Vec<ListItem> = app
            .dir_browser
            .entries
            .iter()
            .map(|e| match e {
                BrowserEntry::Dir(n) => ListItem::new(Line::from(Span::styled(
                    format!("▸ {n}/"),
                    Style::default().fg(Color::Cyan),
                ))),
                BrowserEntry::File(n, s) => ListItem::new(Line::from(vec![
                    Span::raw("  "),
                    Span::raw(n.clone()),
                    Span::styled(
                        format!("  {}", fmt_size(*s)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])),
            })
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            )
            .block(Block::default().borders(Borders::ALL).title(title));
        app.dir_browser.state.select(Some(app.dir_browser.selected));
        f.render_stateful_widget(list, area, &mut app.dir_browser.state);
        let inner = inner_rect(area);
        let off = *app.dir_browser.state.offset_mut();
        for i in 0..app.dir_browser.entries.len() {
            let vy = inner.y + (i as u16).saturating_sub(off as u16);
            if vy >= inner.y + inner.height {
                break;
            }
            app.hits.push((
                Rect { x: inner.x, y: vy, width: inner.width, height: 1 },
                Hit::BrowserDirRow(i),
            ));
        }
    }
}

// ───────────────────────────── helpers ────────────────────────────────

pub fn inner_rect(r: Rect) -> Rect {
    Rect {
        x: r.x.saturating_add(1),
        y: r.y.saturating_add(1),
        width: r.width.saturating_sub(2),
        height: r.height.saturating_sub(2),
    }
}

pub fn centered_rect(pct_x: u16, pct_y: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}

fn fs_badge(cls: FsClass) -> Span<'static> {
    match cls {
        FsClass::Exfat => Span::styled(
            "exFAT (BDM válido ✓)",
            Style::default().fg(Color::Green),
        ),
        FsClass::Fat32 => Span::styled(
            "FAT32 (legacy · máx 4 GiB)",
            Style::default().fg(Color::Yellow),
        ),
        FsClass::Ntfs => Span::styled(
            "NTFS (requer driver)",
            Style::default().fg(Color::Yellow),
        ),
        FsClass::Ext => Span::styled(
            "ext (não lido pelo OPL)",
            Style::default().fg(Color::Red),
        ),
        FsClass::Other => Span::raw("—"),
    }
}

fn fmt_eta(s: Option<u64>) -> String {
    match s {
        Some(t) => format!("{:02}:{:02}", t / 60, t % 60),
        None => "—".to_string(),
    }
}

fn hint_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(Color::DarkGray),
    ))
}

/// Botão de uma linha (clicável): registra o hit e desenha destacado.
fn line_button(f: &mut ratatui::Frame, area: Rect, app: &mut App, label: &str, hit: Hit, ok: bool) {
    let style = if ok {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let p = Paragraph::new(Line::from(Span::styled(format!(" ▸ {label} ◂ "), style)))
        .block(Block::default().borders(Borders::ALL).border_style(style));
    f.render_widget(p, area);
    app.hits.push((area, hit));
}

// ──────────────────────────── modais ──────────────────────────────────

/// Seletor de mídia anti-desastre (F6).
pub fn picker(f: &mut ratatui::Frame, app: &mut App) {
    let area = centered_rect(74, 62, f.area());
    f.render_widget(Clear, area);
    let inner = inner_rect(area);

    let max_dev = ((inner.height.saturating_sub(4)) / 4).max(1) as usize;
    let shown = app.devices.len().min(max_dev);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Mídias externas montadas (clique ou ↑↓ + ENTER):",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    if shown == 0 {
        lines.push(Line::from("Nenhuma mídia externa montada."));
        lines.push(Line::from("Conecte o pendrive/cartão SD e pressione [R]."));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Os discos internos do seu PC nunca aparecem aqui.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for i in 0..shown {
            let d = &app.devices[i];
            let sel = i == app.picker_sel;
            let name = format!("[{}] {}", i + 1, d.display_name());
            lines.push(Line::from(vec![
                Span::from(if sel { "▶ " } else { "  " }),
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled(
                    name,
                    if sel {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
            ]));
            if let Some(m) = d.primary_mount() {
                lines.push(Line::from(format!(
                    "      /dev/{} → {}",
                    m.source,
                    trunc(&m.mount_point.display().to_string(), (inner.width.saturating_sub(12)) as usize)
                )));
                lines.push(Line::from(vec![
                    Span::from(format!(
                        "      Livre {} de {} · ",
                        fmt_size(m.free_bytes),
                        fmt_size(m.total_bytes)
                    )),
                    fs_badge(FsClass::from_fs(&m.fs_type)),
                ]));
            }
            lines.push(Line::from(""));
        }
    }
    lines.push(hint_line("[ENTER] selecionar · [R] rescan · [ESC] fechar"));

    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" SELEÇÃO DE MÍDIA PS2 ")
            .border_style(Style::default().fg(Color::Green)),
    );
    f.render_widget(p, area);

    // hits: bloco de 3 linhas por dispositivo + botão confirmar
    for i in 0..shown {
        app.hits.push((
            Rect {
                x: inner.x,
                y: inner.y + 2 + (i as u16) * 4,
                width: inner.width,
                height: 3,
            },
            Hit::PickerRow(i),
        ));
    }
    if shown > 0 {
        let by = inner.y + inner.height.saturating_sub(3);
        line_button(
            f,
            Rect { x: inner.x, y: by, width: 24, height: 3 },
            app,
            "SELECIONAR (ENTER)",
            Hit::PickerConfirm,
            true,
        );
    }
}

/// Wizard de primeiro uso.
pub fn wizard(f: &mut ratatui::Frame, app: &mut App) {
    let area = centered_rect(70, 64, f.area());
    f.render_widget(Clear, area);
    let inner = inner_rect(area);
    let lines = vec![
        Line::from(Span::styled(
            "BEM-VINDO AO PS2 MANAGER",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(" 1. Selecione a mídia ([F6]): só aparecem pendrives e"),
        Line::from("    cartões externos — os discos do seu PC nunca são"),
        Line::from("    listados como destino (anti-desastre)."),
        Line::from(""),
        Line::from(" 2. [F2] Instalar: navegue pelas ISOs do seu computador;"),
        Line::from("    a conversão ZSO acontece no seu SSD (staging) e a"),
        Line::from("    mídia só é gravada com o arquivo final validado."),
        Line::from(""),
        Line::from(" 3. [F1] Biblioteca, [F3] Boot Kit, [F4] Capas/Cheats."),
        Line::from(""),
        Line::from(" Teclas: ↑↓ navegar · ENTER confirmar · ESC cancelar"),
        Line::from(" Mouse: clique em abas, itens e botões; roda faz scroll."),
    ];
    let p = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" PRIMEIRO USO ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
    let by = inner.y + inner.height.saturating_sub(3);
    line_button(
        f,
        Rect { x: inner.x, y: by, width: 22, height: 3 },
        app,
        "COMEÇAR (ENTER)",
        Hit::WizardOk,
        true,
    );
}

/// Confirmação antes do pipeline de instalação.
pub fn confirm_install(f: &mut ratatui::Frame, app: &mut App) {
    let area = centered_rect(70, 52, f.area());
    f.render_widget(Clear, area);
    let inner = inner_rect(area);

    let file = app.browser.selected_file();
    let (fname, fsize) = match &file {
        Some(p) => (
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::fs::metadata(p).map(|m| fmt_size(m.len())).unwrap_or_default(),
        ),
        None => ("—".to_string(), String::new()),
    };
    let o = app.inst_opts;
    let yn = |b: bool| if b { "✓" } else { "✗" };
    let media_line = match &app.media {
        Some(m) => format!(
            "{} — {} — {} livres",
            trunc(&m.device, 22),
            m.fs.label(),
            fmt_size(m.free)
        ),
        None => "nenhuma".to_string(),
    };

    let lines = vec![
        Line::from(Span::styled(
            "CONFIRMAR INSTALAÇÃO",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(" Arquivo : {fname} ({fsize})")),
        Line::from(format!(" Destino : {media_line}")),
        Line::from(format!(
            " Opções  : ZSO {} · Shrink {} · fallocate {}",
            yn(o.as_zso),
            yn(o.pre_shrink),
            yn(o.fallocate)
        )),
        Line::from(""),
        Line::from(Span::styled(
            " A conversão acontece no SSD local; a mídia só é",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            " gravada quando o arquivo estiver 100% validado.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" INSTALAR JOGO ")
            .border_style(Style::default().fg(Color::Green)),
    );
    f.render_widget(p, area);

    let by = inner.y + inner.height.saturating_sub(3);
    let w1 = 30.min(inner.width / 2);
    line_button(
        f,
        Rect { x: inner.x, y: by, width: w1, height: 3 },
        app,
        "CONFIRMAR E ALOCAR (ENTER)",
        Hit::ConfirmYes,
        true,
    );
    line_button(
        f,
        Rect { x: inner.x + w1 + 2, y: by, width: 18, height: 3 },
        app,
        "CANCELAR (ESC)",
        Hit::ConfirmNo,
        false,
    );
}

/// Progresso do pipeline por fases (com ETA e velocidade).
pub fn progress(f: &mut ratatui::Frame, app: &mut App) {
    let area = centered_rect(66, 56, f.area());
    f.render_widget(Clear, area);
    let inner = inner_rect(area);

    let (phase, done, total, speed, eta, log, finished, is_transfer) = match &app.prog {
        Some(p) => (
            p.phase,
            p.done,
            p.total,
            p.speed_mbps,
            p.eta_secs(),
            p.phase_log.clone(),
            p.finished.clone(),
            p.phase == crate::core::pipeline::PhaseKind::Transfer,
        ),
        None => return,
    };

    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(inner);

    let mut lines: Vec<Line> = Vec::new();
    for l in &log {
        lines.push(Line::from(Span::styled(l.clone(), Style::default().fg(Color::Green))));
    }
    if let Some(res) = &finished {
        match res {
            Ok(s) => {
                for l in s.lines() {
                    lines.push(Line::from(Span::styled(
                        l.to_string(),
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    )));
                }
            }
            Err(e) => {
                lines.push(Line::from(Span::styled(
                    format!("✗ {e}"),
                    Style::default().fg(Color::Red),
                )));
                lines.push(Line::from(Span::styled(
                    "a mídia permanece intacta (rollback)",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    } else {
        let mark = if is_transfer { "▶" } else { "…" };
        lines.push(Line::from(format!(
            " {mark} {}",
            phase.label()
        )));
    }
    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(p, v[0]);

    let ratio = if total > 0 {
        (done as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let lbl = if is_transfer && !finished.is_some() {
        format!(
            "{:.1}% · {:.1} MB/s · ETA {}",
            ratio * 100.0,
            speed,
            fmt_eta(eta)
        )
    } else if total > 0 {
        format!("{:.1}%", ratio * 100.0)
    } else if finished.is_some() {
        "concluído".to_string()
    } else {
        "…".to_string()
    };
    let gauge = Gauge::default()
        .ratio(ratio)
        .label(lbl)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(gauge, v[1]);

    if finished.is_some() {
        line_button(
            f,
            Rect { x: v[2].x, y: v[2].y, width: 12, height: 3 },
            app,
            "OK (ENTER)",
            Hit::ProgressCancel,
            true,
        );
    } else {
        line_button(
            f,
            Rect { x: v[2].x, y: v[2].y, width: 16, height: 3 },
            app,
            "CANCELAR (ESC)",
            Hit::ProgressCancel,
            false,
        );
    }
}

/// Aviso simples com botão OK.
pub fn notify(f: &mut ratatui::Frame, app: &mut App) {
    let area = centered_rect(60, 42, f.area());
    f.render_widget(Clear, area);
    let inner = inner_rect(area);
    let msg = match &app.modal {
        Modal::Notify(m) => m.clone(),
        _ => String::new(),
    };
    let p = Paragraph::new(msg)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" AVISO ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
    let by = inner.y + inner.height.saturating_sub(3);
    line_button(
        f,
        Rect { x: inner.x, y: by, width: 12, height: 3 },
        app,
        "OK (ENTER)",
        Hit::NotifyOk,
        true,
    );
}

/// Seleção de pasta com PNACH (cheats PCSX2 → CHT do OPL).
pub fn pick_pnach(f: &mut ratatui::Frame, app: &mut App) {
    let area = centered_rect(70, 66, f.area());
    f.render_widget(Clear, area);
    let inner = inner_rect(area);

    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(inner);

    render_browser_in(f, v[0], app, true);

    let hint = Paragraph::new(hint_line(
        "[ENTER] entra na pasta · [BACKSPACE] volta · [C] converte a pasta atual · [ESC] cancela",
    ));
    f.render_widget(hint, v[1]);
    line_button(
        f,
        Rect { x: v[2].x, y: v[2].y, width: 26, height: 3 },
        app,
        "CONVERTER ESTA PASTA (C)",
        Hit::BrowserDirGo,
        true,
    );
}
