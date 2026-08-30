use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};

use crate::collector::{Collector, SessionInfo};
use crate::format::{bytes, duration, tokens, truncate};

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Memory,
    Idle,
    Uptime,
    Project,
}

impl Sort {
    fn label(self) -> &'static str {
        match self {
            Sort::Memory => "mem",
            Sort::Idle => "idle",
            Sort::Uptime => "uptime",
            Sort::Project => "project",
        }
    }
}

struct App {
    collector: Collector,
    sessions: Vec<SessionInfo>,
    table: TableState,
    sort: Sort,
    show_all: bool,
    idle_warn: u64,
    confirm_kill: Option<u32>,
    message: Option<(String, Instant)>,
    last_refresh: Instant,
    sys_total: u64,
}

impl App {
    fn refresh(&mut self) {
        let mut s = self.collector.collect();
        if !self.show_all {
            s.retain(|x| x.alive);
        }
        match self.sort {
            Sort::Memory => s.sort_by(|a, b| b.rss_tree.cmp(&a.rss_tree)),
            Sort::Idle => s.sort_by(|a, b| b.idle_secs.unwrap_or(u64::MAX).cmp(&a.idle_secs.unwrap_or(u64::MAX))),
            Sort::Uptime => s.sort_by(|a, b| b.uptime_secs.cmp(&a.uptime_secs)),
            Sort::Project => s.sort_by(|a, b| a.project.cmp(&b.project).then(b.rss_tree.cmp(&a.rss_tree))),
        }
        // keep selection on the same pid if possible
        let selected_pid = self.table.selected().and_then(|i| self.sessions.get(i)).map(|x| x.pid);
        self.sessions = s;
        let idx = selected_pid
            .and_then(|pid| self.sessions.iter().position(|x| x.pid == pid))
            .unwrap_or(0);
        if self.sessions.is_empty() {
            self.table.select(None);
        } else {
            self.table.select(Some(idx.min(self.sessions.len() - 1)));
        }
        self.last_refresh = Instant::now();
    }

    fn move_sel(&mut self, delta: i32) {
        if self.sessions.is_empty() {
            return;
        }
        let cur = self.table.selected().unwrap_or(0) as i32;
        let n = self.sessions.len() as i32;
        let next = (cur + delta).rem_euclid(n);
        self.table.select(Some(next as usize));
    }

    fn selected(&self) -> Option<&SessionInfo> {
        self.table.selected().and_then(|i| self.sessions.get(i))
    }

    fn set_message(&mut self, m: impl Into<String>) {
        self.message = Some((m.into(), Instant::now()));
    }
}

pub fn run(collector: Collector, interval: u64, idle_warn: u64, show_all: bool) -> io::Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App {
        collector,
        sessions: Vec::new(),
        table: TableState::default(),
        sort: Sort::Memory,
        show_all,
        idle_warn,
        confirm_kill: None,
        message: None,
        last_refresh: Instant::now(),
        sys_total: sysinfo::System::new_with_specifics(sysinfo::RefreshKind::nothing().with_memory(sysinfo::MemoryRefreshKind::everything())).total_memory(),
    };
    app.refresh();

    let res = event_loop(&mut terminal, &mut app, Duration::from_secs(interval.max(1)));

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App, interval: Duration) -> io::Result<()> {
    loop {
        terminal.draw(|f| draw(f, app))?;
        let timeout = interval.saturating_sub(app.last_refresh.elapsed());
        if event::poll(timeout.max(Duration::from_millis(50)))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if let Some(pid) = app.confirm_kill {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            app.confirm_kill = None;
                            match kill(pid) {
                                Ok(()) => app.set_message(format!("sent SIGTERM to {pid}")),
                                Err(e) => app.set_message(format!("kill {pid} failed: {e}")),
                            }
                            app.refresh();
                        }
                        _ => app.confirm_kill = None,
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                    KeyCode::Down | KeyCode::Char('j') => app.move_sel(1),
                    KeyCode::Up | KeyCode::Char('k') => app.move_sel(-1),
                    KeyCode::Home | KeyCode::Char('g') => app.table.select(if app.sessions.is_empty() { None } else { Some(0) }),
                    KeyCode::End | KeyCode::Char('G') => {
                        let n = app.sessions.len();
                        app.table.select(if n == 0 { None } else { Some(n - 1) })
                    }
                    KeyCode::Char('r') => app.refresh(),
                    KeyCode::Char('m') => { app.sort = Sort::Memory; app.refresh() }
                    KeyCode::Char('i') => { app.sort = Sort::Idle; app.refresh() }
                    KeyCode::Char('u') => { app.sort = Sort::Uptime; app.refresh() }
                    KeyCode::Char('p') => { app.sort = Sort::Project; app.refresh() }
                    KeyCode::Char('s') => {
                        app.sort = match app.sort {
                            Sort::Memory => Sort::Idle,
                            Sort::Idle => Sort::Uptime,
                            Sort::Uptime => Sort::Project,
                            Sort::Project => Sort::Memory,
                        };
                        app.refresh()
                    }
                    KeyCode::Char('a') => { app.show_all = !app.show_all; app.refresh() }
                    KeyCode::Char('x') | KeyCode::Delete => {
                        if let Some(s) = app.selected() {
                            if s.alive {
                                app.confirm_kill = Some(s.pid);
                            } else {
                                app.set_message("not running");
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if app.last_refresh.elapsed() >= interval {
            app.refresh();
        }
    }
}

fn kill(pid: u32) -> io::Result<()> {
    let status = std::process::Command::new("kill").arg("-TERM").arg(pid.to_string()).status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("exit {status}"))) }
}

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header / totals
            Constraint::Min(5),     // table
            Constraint::Length(9),  // detail
            Constraint::Length(1),  // help
        ])
        .split(area);

    draw_header(f, app, chunks[0]);
    draw_table(f, app, chunks[1]);
    draw_detail(f, app, chunks[2]);
    draw_help(f, app, chunks[3]);

    if let Some(pid) = app.confirm_kill {
        let title = app.sessions.iter().find(|s| s.pid == pid).map(|s| s.title.clone()).unwrap_or_default();
        let text = vec![
            Line::from(format!("Send SIGTERM to PID {pid}?")),
            Line::from(Span::styled(truncate(&title, 50), Style::default().fg(Color::Yellow))),
            Line::from(""),
            Line::from(vec![Span::styled("y", Style::default().bold()), Span::raw(" = yes   any other key = cancel")]),
        ];
        let w = 60.min(area.width.saturating_sub(2));
        let h = 6;
        let popup = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height.saturating_sub(h)) / 2, w, h);
        f.render_widget(Clear, popup);
        f.render_widget(
            Paragraph::new(text).alignment(ratatui::layout::Alignment::Center).block(
                Block::default().borders(Borders::ALL).title(" confirm ").border_style(Style::default().fg(Color::Red)),
            ),
            popup,
        );
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let alive = app.sessions.iter().filter(|s| s.alive).count();
    let total: u64 = app.sessions.iter().map(|s| s.rss_tree).sum();
    let idle_n = app.sessions.iter().filter(|s| s.alive && s.idle_secs.map(|i| i > app.idle_warn * 60).unwrap_or(false)).count();
    let busy_n = app.sessions.iter().filter(|s| s.alive && s.status.as_deref() == Some("busy")).count();
    let stale_n = app.sessions.len() - alive;

    let mut spans = vec![
        Span::styled(" claude-monitor ", Style::default().bold().fg(Color::Black).bg(Color::Cyan)),
        Span::raw("  "),
        Span::styled(format!("{alive}"), Style::default().bold().fg(Color::Green)),
        Span::raw(" running  "),
        Span::styled(format!("{}", bytes(total)), Style::default().bold().fg(Color::Magenta)),
        Span::raw(" total RSS (incl. children)  "),
    ];
    let cc = app.sessions.iter().filter(|s| s.alive && s.agent == "claude").count();
    let oc = app.sessions.iter().filter(|s| s.alive && s.agent == "opencode").count();
    let cp = app.sessions.iter().filter(|s| s.alive && s.agent == "copilot").count();
    if oc > 0 || cp > 0 {
        spans.push(Span::styled(format!("(claude {cc} / opencode {oc} / copilot {cp})  "), Style::default().fg(Color::DarkGray)));
    }
    if busy_n > 0 {
        spans.push(Span::styled(format!("{busy_n} busy  "), Style::default().fg(Color::Green)));
    }
    if idle_n > 0 {
        spans.push(Span::styled(format!("{idle_n} idle >{}m  ", app.idle_warn), Style::default().fg(Color::Yellow).bold()));
    }
    if stale_n > 0 {
        spans.push(Span::styled(format!("{stale_n} stale  "), Style::default().fg(Color::DarkGray)));
    }
    spans.push(Span::styled(format!("sort:{}", app.sort.label()), Style::default().fg(Color::DarkGray)));

    let sys_total = app.sys_total;
    let ratio = if sys_total > 0 { (total as f64 / sys_total as f64).min(1.0) } else { 0.0 };
    let gauge = Gauge::default()
        .ratio(ratio)
        .label(format!("{} / {} system memory", bytes(total), bytes(sys_total)))
        .gauge_style(Style::default().fg(Color::Magenta).bg(Color::Black));

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    f.render_widget(Paragraph::new(Line::from(spans)), inner[0]);
    f.render_widget(gauge, inner[1]);
}

fn draw_table(f: &mut Frame, app: &mut App, area: Rect) {
    let max_mem = app.sessions.iter().map(|s| s.rss_tree).max().unwrap_or(1).max(1);
    let bar_w: usize = 12;
    let width = area.width as usize;
    // fixed columns: pid(6) mem(7) bar(12) up(7) idle(7) ctx(6) via(6) + gaps
    let fixed = 8 + 6 + 7 + bar_w + 7 + 7 + 6 + 6 + 8 * 2 + 2;
    let rest = width.saturating_sub(fixed).max(20);
    let proj_w = (rest / 3).clamp(10, 28);
    let title_w = rest.saturating_sub(proj_w).max(10);

    let rows: Vec<Row> = app
        .sessions
        .iter()
        .map(|s| {
            let filled = ((s.rss_tree as f64 / max_mem as f64) * bar_w as f64).round() as usize;
            let bar = format!("{}{}", "█".repeat(filled.min(bar_w)), "░".repeat(bar_w - filled.min(bar_w)));
            let idle_over = s.idle_secs.map(|i| i > app.idle_warn * 60).unwrap_or(false);
            let base = if !s.alive {
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
            } else {
                Style::default()
            };
            let status_dot = match (s.alive, s.status.as_deref()) {
                (false, _) => Span::styled("✗", Style::default().fg(Color::DarkGray)),
                (true, Some("busy")) => Span::styled("●", Style::default().fg(Color::Green)),
                (true, _) if idle_over => Span::styled("●", Style::default().fg(Color::Yellow)),
                (true, _) => Span::styled("●", Style::default().fg(Color::Blue)),
            };
            let idle_style = if idle_over { Style::default().fg(Color::Yellow).bold() } else { base };
            let via = match s.entrypoint.as_str() {
                "claude-vscode" => "vscode",
                "cli" => "cli",
                e => e,
            };
            let via = if s.unregistered { "?" } else { via };
            let mem_color = mem_color(s.rss_tree);
            let agent_style = match s.agent {
                "opencode" => Style::default().fg(Color::Magenta),
                "copilot" => Style::default().fg(Color::Rgb(95, 175, 255)),
                _ => Style::default().fg(Color::Rgb(255, 135, 0)),
            };
            Row::new(vec![
                Cell::from(status_dot),
                Cell::from(s.agent).style(agent_style),
                Cell::from(s.pid.to_string()).style(base),
                Cell::from(bytes(s.rss_tree)).style(base.fg(mem_color)),
                Cell::from(bar).style(Style::default().fg(mem_color)),
                Cell::from(duration(s.uptime_secs)).style(base),
                Cell::from(s.idle_secs.map(duration).unwrap_or_else(|| "-".into())).style(idle_style),
                Cell::from(tokens(s.context_tokens)).style(base),
                Cell::from(via).style(base.fg(Color::DarkGray)),
                Cell::from(truncate(&s.project, proj_w)).style(base.fg(Color::Cyan)),
                Cell::from(truncate(&s.title, title_w)).style(base),
            ])
        })
        .collect();

    let header = Row::new(vec!["", "AGENT", "PID", "MEM", "", "UP", "IDLE", "CTX", "VIA", "PROJECT", "TITLE"])
        .style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD | Modifier::UNDERLINED));
    let widths = [
        Constraint::Length(1),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(bar_w as u16),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(proj_w as u16),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .row_highlight_style(Style::default().bg(Color::Rgb(40, 50, 70)).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶")
        .block(Block::default().borders(Borders::TOP).title(format!(" sessions ({}) ", app.sessions.len())));
    if app.sessions.is_empty() {
        f.render_widget(
            Paragraph::new("no running Claude Code sessions").fg(Color::DarkGray).block(Block::default().borders(Borders::TOP).title(" sessions ")),
            area,
        );
    } else {
        f.render_stateful_widget(table, area, &mut app.table);
    }
}

fn mem_color(b: u64) -> Color {
    let mb = b / (1024 * 1024);
    if mb >= 1024 { Color::Red } else if mb >= 400 { Color::Yellow } else { Color::Green }
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::TOP).title(" detail ");
    let Some(s) = app.selected() else {
        f.render_widget(Paragraph::new("").block(block), area);
        return;
    };
    let kv = |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:>9} "), Style::default().fg(Color::DarkGray)), Span::raw(v)]);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{:>9} ", "title"), Style::default().fg(Color::DarkGray)),
            Span::styled(s.title.clone(), Style::default().bold().fg(Color::White)),
            Span::styled(format!("  [{}]", s.title_source), Style::default().fg(Color::DarkGray)),
        ]),
        kv("cwd", format!("{}{}", s.cwd, s.git_branch.as_ref().map(|b| format!("  ({b})")).unwrap_or_default())),
        kv(
            "session",
            format!(
                "{}  v{}  {}{}",
                if s.session_id.is_empty() { "?" } else { &s.session_id },
                s.version.as_deref().unwrap_or("?"),
                s.entrypoint,
                s.status.as_ref().map(|x| format!("  status={x}")).unwrap_or_default()
            ),
        ),
        kv(
            "proc",
            format!(
                "pid {}  rss {} (tree {})  cpu {:.1}%  up {}  idle {}  ctx {} tokens{}",
                s.pid,
                bytes(s.rss_self),
                bytes(s.rss_tree),
                s.cpu,
                duration(s.uptime_secs),
                s.idle_secs.map(duration).unwrap_or_else(|| "-".into()),
                tokens(s.context_tokens),
                s.model.as_ref().map(|m| format!("  model {m}")).unwrap_or_default()
            ),
        ),
    ];
    if let Some(p) = &s.first_prompt {
        lines.push(kv("prompt", p.clone()));
    }
    if !s.children.is_empty() {
        let mut kids: Vec<&crate::collector::ChildProc> = s.children.iter().collect();
        kids.sort_by(|a, b| b.rss.cmp(&a.rss));
        let shown: Vec<String> = kids.iter().take(6).map(|c| format!("{} {}({})", bytes(c.rss), truncate(&c.name, 40), c.pid)).collect();
        let more = if kids.len() > 6 { format!("  +{} more", kids.len() - 6) } else { String::new() };
        lines.push(kv(&format!("child×{}", kids.len()), format!("{}{}", shown.join("  |  "), more)));
    }
    if s.agent == "copilot" && s.entrypoint == "vscode" {
        lines.push(Line::from(Span::styled(
            "  PID is the VS Code extension host: MEM covers all extensions of that window (other agents' sessions excluded); x restarts every extension in it",
            Style::default().fg(Color::DarkGray),
        )));
    }
    if !s.alive {
        lines.push(Line::from(Span::styled("  (process not running — stale registry file ~/.claude/sessions/<pid>.json)", Style::default().fg(Color::Yellow))));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let msg = app.message.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(m, _)| m.clone());
    let line = if let Some(m) = msg {
        Line::from(Span::styled(format!(" {m}"), Style::default().fg(Color::Yellow)))
    } else {
        let k = |s: &str| Span::styled(s.to_string(), Style::default().fg(Color::Black).bg(Color::DarkGray));
        Line::from(vec![
            k(" q "), Span::raw(" quit  "),
            k(" j/k "), Span::raw(" move  "),
            k(" m/i/u/p "), Span::raw(" sort mem/idle/uptime/project  "),
            k(" a "), Span::raw(if app.show_all { " hide stale  " } else { " show stale  " }),
            k(" x "), Span::raw(" kill  "),
            k(" r "), Span::raw(" refresh"),
        ])
    };
    f.render_widget(Paragraph::new(line), area);
}
