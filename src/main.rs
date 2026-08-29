mod collector;
mod format;
mod jsonl;
mod ui;

use clap::Parser;
use collector::{Collector, SessionInfo};
use std::io::IsTerminal;

/// Monitor running Claude Code sessions: memory, uptime, idle time and what each one is about.
#[derive(Parser, Debug)]
#[command(name = "claude-monitor", version, about)]
struct Cli {
    /// Print a one-shot table and exit (no TUI)
    #[arg(short = '1', long)]
    once: bool,
    /// Print JSON (implies --once)
    #[arg(short, long)]
    json: bool,
    /// Refresh interval in seconds for the TUI
    #[arg(short, long, default_value_t = 2)]
    interval: u64,
    /// Highlight sessions idle for longer than this many minutes
    #[arg(long, default_value_t = 30)]
    idle_warn: u64,
    /// Include dead registry entries (stale session files)
    #[arg(short, long)]
    all: bool,
    /// Force ANSI colors in --once output even when piped (e.g. for `watch --color`)
    #[arg(long, conflicts_with = "no_color")]
    color: bool,
    /// Disable ANSI colors in --once output (auto-disabled when not a TTY or NO_COLOR is set)
    #[arg(long)]
    no_color: bool,
}

fn main() {
    let cli = Cli::parse();
    let mut collector = Collector::new();
    if cli.json || cli.once {
        let mut sessions = collector.collect();
        if !cli.all {
            sessions.retain(|s| s.alive);
        }
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&sessions).unwrap());
        } else {
            let color = cli.color || (!cli.no_color && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal());
            print_table(&sessions, cli.idle_warn, color);
        }
        return;
    }
    if let Err(e) = ui::run(collector, cli.interval, cli.idle_warn, cli.all) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// tiny ANSI helper
struct Paint(bool);
impl Paint {
    fn c(&self, code: &str, s: impl AsRef<str>) -> String {
        if self.0 { format!("\x1b[{code}m{}\x1b[0m", s.as_ref()) } else { s.as_ref().to_string() }
    }
    fn dim(&self, s: impl AsRef<str>) -> String { self.c("2", s) }
    fn bold(&self, s: impl AsRef<str>) -> String { self.c("1", s) }
    fn red(&self, s: impl AsRef<str>) -> String { self.c("31", s) }
    fn green(&self, s: impl AsRef<str>) -> String { self.c("32", s) }
    fn yellow(&self, s: impl AsRef<str>) -> String { self.c("33", s) }
    fn blue(&self, s: impl AsRef<str>) -> String { self.c("34", s) }
    fn magenta(&self, s: impl AsRef<str>) -> String { self.c("35", s) }
    fn cyan(&self, s: impl AsRef<str>) -> String { self.c("36", s) }
    fn white_b(&self, s: impl AsRef<str>) -> String { self.c("1;97", s) }
    fn header(&self, s: impl AsRef<str>) -> String { self.c("1;4;90", s) }
    fn banner(&self, s: impl AsRef<str>) -> String { self.c("1;30;46", s) }
}

fn mem_paint(p: &Paint, b: u64, s: String) -> String {
    let mb = b / (1024 * 1024);
    if mb >= 1024 { p.red(s) } else if mb >= 400 { p.yellow(s) } else { p.green(s) }
}

fn print_table(sessions: &[SessionInfo], idle_warn: u64, color: bool) {
    use format::*;
    let p = Paint(color);
    let bar_w = 12usize;
    let max_mem = sessions.iter().map(|s| s.rss_tree).max().unwrap_or(1).max(1);
    let total: u64 = sessions.iter().map(|s| s.rss_tree).sum();
    let idle_n = sessions.iter().filter(|s| s.alive && s.idle_secs.map(|i| i > idle_warn * 60).unwrap_or(false)).count();
    let busy_n = sessions.iter().filter(|s| s.alive && s.status.as_deref() == Some("busy")).count();
    let alive_n = sessions.iter().filter(|s| s.alive).count();

    // banner
    let mut banner = format!(
        "{} {} running  {} total RSS (incl. children)",
        p.banner(" claude-monitor "),
        p.bold(p.green(alive_n.to_string())),
        p.bold(p.magenta(bytes(total)))
    );
    if busy_n > 0 { banner.push_str(&format!("  {}", p.green(format!("{busy_n} busy")))); }
    if idle_n > 0 { banner.push_str(&format!("  {}", p.bold(p.yellow(format!("{idle_n} idle >{idle_warn}m"))))); }
    if sessions.len() > alive_n { banner.push_str(&format!("  {}", p.dim(format!("{} stale", sessions.len() - alive_n)))); }
    println!("{banner}");

    println!(
        "{}",
        p.header(format!(
            "{:>1} {:>6} {:>6} {:<bw$} {:>6} {:>7} {:>6} {:<6} {:<22} {}",
            "", "PID", "MEM", "", "UP", "IDLE", "CTX", "VIA", "PROJECT", "TITLE", bw = bar_w
        ))
    );
    for s in sessions {
        let idle_over = s.idle_secs.map(|i| i > idle_warn * 60).unwrap_or(false);
        let dot = match (s.alive, s.status.as_deref()) {
            (false, _) => p.dim("✗"),
            (true, Some("busy")) => p.green("●"),
            (true, _) if idle_over => p.yellow("●"),
            (true, _) => p.blue("●"),
        };
        let filled = ((s.rss_tree as f64 / max_mem as f64) * bar_w as f64).round() as usize;
        let filled = filled.min(bar_w);
        let bar = format!("{}{}", mem_paint(&p, s.rss_tree, "█".repeat(filled)), p.dim("░".repeat(bar_w - filled)));
        let idle_raw = format!("{:>7}", s.idle_secs.map(duration).unwrap_or_else(|| "-".into()));
        let idle = if idle_over { p.bold(p.yellow(format!("{idle_raw}!"))) } else { format!("{idle_raw} ") };
        let via = match s.entrypoint.as_str() { "claude-vscode" => "vscode", e => e };
        let via = if s.unregistered { "?" } else { via };
        let line = format!(
            "{} {:>6} {} {} {:>6} {} {:>6} {:<6} {:<22} {}",
            dot,
            s.pid,
            mem_paint(&p, s.rss_tree, format!("{:>6}", bytes(s.rss_tree))),
            bar,
            duration(s.uptime_secs),
            idle,
            tokens(s.context_tokens),
            p.dim(format!("{via:<6}")),
            p.cyan(format!("{:<22}", truncate(&s.project, 22))),
            p.white_b(&s.title),
        );
        println!("{}", if s.alive { line } else { p.dim(line) });
    }
    if sessions.is_empty() {
        println!("{}", p.dim("no running Claude Code sessions"));
    }
}
