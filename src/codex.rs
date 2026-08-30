//! OpenAI Codex (CLI `codex`, and `codex app-server` spawned by the VS Code extension).
//! Threads live in `~/.codex/state_5.sqlite` (`threads` table); currently open threads
//! hold an empty `~/.codex/thread-writer-locks/<thread_id>.lock`. Locks carry no pid,
//! so threads are attached to processes by source (cli vs vscode) and cwd.

use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use sysinfo::{Process, System};

use crate::collector::{ChildProc, CwdCache, SessionInfo, cmdline, fill_tree};

pub fn codex_home() -> PathBuf {
    if let Some(p) = std::env::var_os("CODEX_HOME") {
        return PathBuf::from(p);
    }
    crate::collector::home_dir().join(".codex")
}

pub fn is_codex(p: &Process) -> bool {
    crate::collector::proc_named(p, "codex")
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub id: String,
    pub cwd: String,
    pub title: String,
    pub source: String,
    pub model: Option<String>,
    pub branch: Option<String>,
    pub updated_at: u64,
    pub tokens_used: u64,
}

fn open_thread_ids(home: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(home.join("thread-writer-locks")) else { return v };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".lock") {
            if !id.starts_with('.') {
                v.push(id.to_string());
            }
        }
    }
    v
}

fn load_threads(home: &Path, ids: &[String]) -> Vec<Thread> {
    let mut out = Vec::new();
    if ids.is_empty() {
        return out;
    }
    let db = home.join("state_5.sqlite");
    let Ok(conn) = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else {
        return out;
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(200));
    let Ok(mut stmt) = conn.prepare(
        "select id, cwd, coalesce(nullif(name,''), nullif(title,''), nullif(first_user_message,''), ''), \
                source, model, git_branch, updated_at, tokens_used from threads where id = ?1",
    ) else {
        return out;
    };
    for id in ids {
        if let Ok(t) = stmt.query_row([id], |r| {
            Ok(Thread {
                id: r.get(0)?,
                cwd: crate::collector::strip_verbatim_prefix(&r.get::<_, String>(1)?),
                title: r.get(2)?,
                source: r.get(3)?,
                model: r.get(4)?,
                branch: r.get(5)?,
                updated_at: r.get::<_, i64>(6)? as u64,
                tokens_used: r.get::<_, i64>(7)? as u64,
            })
        }) {
            out.push(t);
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

pub fn collect(
    sys: &System,
    cwds: &mut CwdCache,
    children: &HashMap<u32, Vec<u32>>,
    windows: &HashMap<u32, Vec<String>>, // VS Code extension-host pid -> workspace folders (from Copilot ide locks, may be empty)
    now_secs: u64,
) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    let procs: Vec<&Process> = sys
        .processes()
        .values()
        .filter(|p| is_codex(p))
        .filter(|p| !p.parent().and_then(|pp| sys.process(pp)).map(is_codex).unwrap_or(false))
        .collect();
    if procs.is_empty() {
        return out;
    }
    let home = codex_home();
    let threads = load_threads(&home, &open_thread_ids(&home));
    let mut used: HashSet<String> = HashSet::new();

    for p in procs {
        let cmd = cmdline(p);
        let is_app_server = cmd.contains("app-server");
        let mode = if is_app_server { "vscode" } else if cmd.contains(" exec") { "exec" } else { "cli" };
        let cwd = cwds.get(p);
        let window_folders: Vec<String> = p
            .parent()
            .and_then(|pp| windows.get(&pp.as_u32()))
            .cloned()
            .unwrap_or_default();

        // threads that plausibly belong to this process
        let mine: Vec<&Thread> = threads
            .iter()
            .filter(|t| !used.contains(&t.id))
            .filter(|t| {
                if is_app_server {
                    t.source != "cli"
                        && (window_folders.is_empty()
                            || window_folders.iter().any(|f| t.cwd == *f || Path::new(&t.cwd).starts_with(f)))
                } else if cwd.is_empty() {
                    // cwd unknown (sysinfo has no cwd on Windows): any open CLI thread
                    t.source == "cli"
                } else {
                    t.cwd == cwd || Path::new(&t.cwd).starts_with(&cwd)
                }
            })
            .collect();
        let mut mine = mine;
        mine.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        for t in &mine {
            used.insert(t.id.clone());
        }
        let head = mine.first().copied();
        let idle = head.map(|t| now_secs.saturating_sub(t.updated_at));
        let mut title = head.map(|t| t.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| {
            if is_app_server { "(codex app-server, no open thread)".into() } else { "(no open codex thread)".into() }
        });
        if mine.len() > 1 {
            title.push_str(&format!("  [+{} threads]", mine.len() - 1));
        }
        let shown_cwd = head.map(|t| t.cwd.clone()).or_else(|| window_folders.first().cloned()).unwrap_or(cwd.clone());
        let project = Path::new(&shown_cwd).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| shown_cwd.clone());

        let mut info = SessionInfo {
            agent: "codex",
            pid: p.pid().as_u32(),
            alive: true,
            session_id: head.map(|t| t.id.clone()).unwrap_or_default(),
            title,
            title_source: if head.is_some() { "db" } else { "none" },
            cwd: shown_cwd,
            project,
            entrypoint: mode.to_string(),
            status: idle.filter(|i| *i < 15).map(|_| "busy".to_string()),
            version: None,
            rss_self: p.memory(),
            rss_tree: 0,
            cpu: p.cpu_usage(),
            uptime_secs: now_secs.saturating_sub(p.start_time()),
            idle_secs: idle,
            context_tokens: head.map(|t| t.tokens_used).filter(|t| *t > 0),
            model: head.and_then(|t| t.model.clone()),
            first_prompt: None,
            git_branch: head.and_then(|t| t.branch.clone()),
            cmdline: cmd,
            children: Vec::<ChildProc>::new(),
            unregistered: false,
        };
        fill_tree(sys, &mut info, children);
        out.push(info);
    }
    out
}
