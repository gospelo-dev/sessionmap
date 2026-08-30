//! OpenCode (sst/opencode) sessions.
//! Processes are named `opencode`; session metadata lives in
//! `~/.local/share/opencode/opencode.db` (SQLite). There is no PID registry,
//! so a process is matched to the most recently updated session whose
//! `directory` equals the process cwd.

use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use sysinfo::{Process, System};

use crate::collector::{ChildProc, CwdCache, SessionInfo, cmdline, fill_tree};

pub struct DbSession {
    pub id: String,
    pub directory: String,
    pub title: String,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub time_updated: u64,
    pub context_tokens: Option<u64>,
    pub first_prompt: Option<String>,
}

pub fn db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("OPENCODE_DB") {
        return PathBuf::from(p);
    }
    let home = crate::collector::home_dir();
    let base = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local/share"));
    base.join("opencode/opencode.db")
}

/// `model` is stored as JSON like {"id":"...","providerID":"..."}; keep the id only.
fn model_id(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| raw.to_string())
}

pub fn is_opencode(p: &Process) -> bool {
    p.name().to_string_lossy() == "opencode"
        || p.exe().and_then(|e| e.file_name()).map(|n| n.to_string_lossy() == "opencode").unwrap_or(false)
}

/// Recent top-level sessions, newest first. Cheap: indexed query, small limit.
pub fn recent_sessions(path: &Path, limit: usize) -> Vec<DbSession> {
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else {
        return Vec::new();
    };
    let _ = conn.busy_timeout(std::time::Duration::from_millis(200));
    let Ok(mut stmt) = conn.prepare(
        "select id, directory, title, model, agent, time_updated from session \
         where parent_id is null and time_archived is null order by time_updated desc limit ?1",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(DbSession {
            id: r.get(0)?,
            directory: r.get(1)?,
            title: r.get(2)?,
            model: r.get::<_, Option<String>>(3)?.map(|m| model_id(&m)),
            agent: r.get::<_, Option<String>>(4)?,
            time_updated: r.get::<_, i64>(5)? as u64,
            context_tokens: None,
            first_prompt: None,
        })
    });
    let mut out: Vec<DbSession> = rows.map(|it| it.flatten().collect()).unwrap_or_default();
    for s in &mut out {
        s.context_tokens = last_context_tokens(&conn, &s.id);
        s.first_prompt = first_prompt(&conn, &s.id);
    }
    out
}

fn last_context_tokens(conn: &Connection, sid: &str) -> Option<u64> {
    // walk back until an assistant message with tokens is found
    let mut stmt = conn
        .prepare("select data from message where session_id = ?1 order by time_created desc, id desc limit 8")
        .ok()?;
    let rows = stmt.query_map([sid], |r| r.get::<_, String>(0)).ok()?;
    for d in rows.flatten() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&d) else { continue };
        if v.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        if let Some(t) = v.get("tokens") {
            let g = |k: &str| t.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            let cache = t.get("cache").map(|c| {
                c.get("read").and_then(|x| x.as_u64()).unwrap_or(0) + c.get("write").and_then(|x| x.as_u64()).unwrap_or(0)
            }).unwrap_or(0);
            let total = g("input") + cache;
            let total = if total > 0 { total } else { g("total") };
            if total > 0 {
                return Some(total);
            }
        }
    }
    None
}

fn first_prompt(conn: &Connection, sid: &str) -> Option<String> {
    let mut stmt = conn
        .prepare(
            "select p.data from part p join message m on m.id = p.message_id \
             where m.session_id = ?1 order by m.time_created asc, p.time_created asc limit 5",
        )
        .ok()?;
    let rows = stmt.query_map([sid], |r| r.get::<_, String>(0)).ok()?;
    for d in rows.flatten() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&d) else { continue };
        if v.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
                let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
                if !one.is_empty() {
                    return Some(one.chars().take(120).collect());
                }
            }
        }
    }
    None
}

pub fn collect(sys: &System, cwds: &mut CwdCache, children: &HashMap<u32, Vec<u32>>, now_secs: u64) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    let procs: Vec<(u32, &Process)> = sys
        .processes()
        .iter()
        .filter(|(_, p)| is_opencode(p))
        .filter(|(_, p)| !p.parent().and_then(|pp| sys.process(pp)).map(is_opencode).unwrap_or(false))
        .map(|(pid, p)| (pid.as_u32(), p))
        .collect();
    if procs.is_empty() {
        return out;
    }
    let sessions = recent_sessions(&db_path(), 200);
    let mut used: std::collections::HashSet<String> = Default::default();

    for (pid, p) in procs {
        let cwd = cwds.get(p);
        let cmd = cmdline(p);
        let mode = cmd.split_whitespace().nth(1).filter(|a| !a.starts_with('-')).unwrap_or("tui").to_string();
        let matched = sessions
            .iter()
            .filter(|s| !used.contains(&s.id))
            .find(|s| !cwd.is_empty() && (s.directory == cwd || Path::new(&s.directory).starts_with(&cwd)));
        if let Some(s) = matched {
            used.insert(s.id.clone());
        }
        let idle = matched.map(|s| now_secs.saturating_sub(s.time_updated / 1000));
        let uptime = now_secs.saturating_sub(p.start_time());
        // session last touched before this process started: it is the previous session in that dir
        let previous = matched.map(|s| s.time_updated / 1000 + 5 < p.start_time()).unwrap_or(false);
        let project = Path::new(&cwd).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| cwd.clone());
        let mut info = SessionInfo {
            agent: "opencode",
            pid,
            alive: true,
            session_id: matched.map(|s| s.id.clone()).unwrap_or_default(),
            title: match matched {
                Some(s) if previous => format!("(last in dir) {}", s.title),
                Some(s) => s.title.clone(),
                None => format!("(opencode {mode}, no session in cwd)"),
            },
            title_source: if previous { "db:previous" } else if matched.is_some() { "db" } else { "none" },
            cwd,
            project,
            entrypoint: mode,
            status: idle.filter(|i| *i < 15).map(|_| "busy".to_string()),
            version: None,
            rss_self: p.memory(),
            rss_tree: 0,
            cpu: p.cpu_usage(),
            uptime_secs: uptime,
            idle_secs: idle,
            context_tokens: matched.and_then(|s| s.context_tokens),
            model: matched.and_then(|s| s.model.clone().or_else(|| s.agent.clone())),
            first_prompt: matched.and_then(|s| s.first_prompt.clone()),
            git_branch: None,
            cmdline: cmd,
            children: Vec::<ChildProc>::new(),
            unregistered: matched.is_none(),
        };
            fill_tree(sys, &mut info, children);
        out.push(info);
    }
    out
}
