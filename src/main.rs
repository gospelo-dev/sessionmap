mod collector;
mod format;
mod jsonl;
mod ui;

use clap::Parser;
use collector::{Collector, SessionInfo};

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
            print_table(&sessions, cli.idle_warn);
        }
        return;
    }
    if let Err(e) = ui::run(collector, cli.interval, cli.idle_warn, cli.all) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn print_table(sessions: &[SessionInfo], idle_warn: u64) {
    use format::*;
    println!(
        "{:>6} {:>7} {:>7} {:>7} {:>7} {:>6} {:<22} {:<8} {}",
        "PID", "MEM", "TREE", "UP", "IDLE", "CTX", "PROJECT", "VIA", "TITLE"
    );
    let mut total = 0u64;
    for s in sessions {
        total += s.rss_tree;
        let idle = s.idle_secs.map(duration).unwrap_or_else(|| "-".into());
        let mark = match s.idle_secs {
            Some(i) if i > idle_warn * 60 => "!",
            _ => " ",
        };
        let via = match s.entrypoint.as_str() {
            "claude-vscode" => "vscode",
            e => e,
        };
        println!(
            "{:>6} {:>7} {:>7} {:>7} {:>6}{} {:>6} {:<22} {:<8} {}",
            s.pid,
            bytes(s.rss_self),
            bytes(s.rss_tree),
            duration(s.uptime_secs),
            idle,
            mark,
            tokens(s.context_tokens),
            truncate(&s.project, 22),
            via,
            s.title
        );
    }
    println!("{} session(s), total tree memory {}", sessions.len(), bytes(total));
}
