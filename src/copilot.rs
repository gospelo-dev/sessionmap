//! GitHub Copilot CLI sessions.
//! Each session lives in `~/.copilot/session-state/<id>/`; a running one holds an
//! `inuse.<pid>.lock` file there, which is our PID registry. `workspace.yaml`
//! has cwd / name / branch / client, `events.jsonl` (if any) has prompts and models.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use sysinfo::{Pid, Process, System};

use crate::collector::{ChildProc, SessionInfo, cmdline, fill_tree};

pub fn state_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("COPILOT_HOME") {
        return PathBuf::from(p).join("session-state");
    }
    crate::collector::home_dir().join(".copilot/session-state")
}

pub fn is_copilot(p: &Process) -> bool {
    p.name().to_string_lossy() == "copilot"
        || p.exe().and_then(|e| e.file_name()).map(|n| n.to_string_lossy() == "copilot").unwrap_or(false)
}

#[derive(Default, Clone)]
struct Workspace {
    cwd: String,
    name: Option<String>,
    branch: Option<String>,
    client: Option<String>,
    created_at: Option<String>,
}

/// Minimal YAML reader for the flat `key: value` file copilot writes.
/// Block scalars (`name: |-` + indented lines) are collapsed to their first line.
fn read_workspace(path: &Path) -> Workspace {
    let mut w = Workspace::default();
    let Ok(text) = std::fs::read_to_string(path) else { return w };
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with(' ') || line.starts_with('\t') || line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let mut v = v.trim().to_string();
        if v.is_empty() || v == "|" || v == "|-" || v == ">" || v == ">-" {
            // block scalar: take first non-empty indented line
            v.clear();
            while let Some(next) = lines.peek() {
                if next.starts_with(' ') || next.starts_with('\t') {
                    let t = next.trim();
                    if v.is_empty() && !t.is_empty() {
                        v = t.to_string();
                    }
                    lines.next();
                } else {
                    break;
                }
            }
        }
        let v = v.trim_matches(|c| c == '"' || c == '\'').to_string();
        match k.trim() {
            "cwd" => w.cwd = v,
            "name" => w.name = Some(v).filter(|s| !s.is_empty()),
            "branch" => w.branch = Some(v).filter(|s| !s.is_empty()),
            "client_name" => w.client = Some(v).filter(|s| !s.is_empty()),
            "created_at" => w.created_at = Some(v),
            _ => {}
        }
    }
    w
}

#[derive(Default, Clone)]
struct Events {
    first_prompt: Option<String>,
    model: Option<String>,
}

#[derive(Default)]
pub struct EventsCache {
    map: HashMap<PathBuf, (u64, Events)>,
}

impl EventsCache {
    fn get(&mut self, path: &Path) -> Events {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if let Some((l, e)) = self.map.get(path) {
            if *l == len {
                return e.clone();
            }
        }
        let mut ev = Events::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                if ev.first_prompt.is_none() && line.contains("\"type\":\"user.message\"") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(c) = v.pointer("/data/content").and_then(|c| c.as_str()) {
                            let one: String = c.split_whitespace().collect::<Vec<_>>().join(" ");
                            if !one.is_empty() {
                                ev.first_prompt = Some(one.chars().take(120).collect());
                            }
                        }
                    }
                } else if line.contains("\"type\":\"assistant.message\"") || line.contains("\"type\":\"session.model_change\"") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(m) = v.pointer("/data/model").or_else(|| v.pointer("/data/newModel")).and_then(|m| m.as_str()) {
                            ev.model = Some(m.to_string());
                        }
                    }
                }
            }
        }
        self.map.insert(path.to_path_buf(), (len, ev.clone()));
        ev
    }
}

fn mtime_secs(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

pub fn collect(sys: &System, cache: &mut EventsCache, children: &HashMap<u32, Vec<u32>>, now_secs: u64) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(state_dir()) else { return out };
    let mut seen: std::collections::HashSet<u32> = Default::default();

    for entry in rd.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        for f in files.flatten() {
            let fname = f.file_name().to_string_lossy().to_string();
            let Some(pid) = fname.strip_prefix("inuse.").and_then(|s| s.strip_suffix(".lock")).and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let sid = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ws = read_workspace(&dir.join("workspace.yaml"));
            let events_path = dir.join("events.jsonl");
            let ev = if events_path.exists() { cache.get(&events_path) } else { Events::default() };
            let idle = mtime_secs(&events_path).or_else(|| mtime_secs(&dir.join("workspace.yaml"))).map(|m| now_secs.saturating_sub(m));

            let proc_ = sys.process(Pid::from_u32(pid)).filter(|p| is_copilot(p));
            // the lock holder is usually a child of a thin `copilot` wrapper; root the tree at the wrapper
            let root = proc_.and_then(|p| p.parent()).and_then(|pp| sys.process(pp)).filter(|pp| is_copilot(pp)).or(proc_);
            let alive = proc_.is_some();
            let root_pid = root.map(|p| p.pid().as_u32()).unwrap_or(pid);
            if !seen.insert(root_pid) {
                continue;
            }
            let (title, title_source) = match (&ws.name, &ev.first_prompt) {
                (Some(n), _) => (n.clone(), "name"),
                (None, Some(p)) => (p.clone(), "prompt"),
                _ => ("(untitled copilot session)".to_string(), "none"),
            };
            let project = Path::new(&ws.cwd).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| ws.cwd.clone());
            let entrypoint = match ws.client.as_deref() {
                Some("github/cli") | Some("cli") => "cli".to_string(),
                Some(c) if c.contains("vscode") => "vscode".to_string(),
                Some(c) => c.rsplit('/').next().unwrap_or(c).to_string(),
                None => "?".to_string(),
            };
            let mut info = SessionInfo {
                agent: "copilot",
                pid: root_pid,
                alive,
                session_id: sid,
                title,
                title_source,
                cwd: ws.cwd.clone(),
                project,
                entrypoint,
                status: idle.filter(|i| *i < 15).map(|_| "busy".to_string()),
                version: None,
                rss_self: proc_.map(|p| p.memory()).unwrap_or(0),
                rss_tree: 0,
                cpu: proc_.map(|p| p.cpu_usage()).unwrap_or(0.0),
                uptime_secs: root.map(|p| now_secs.saturating_sub(p.start_time())).unwrap_or(0),
                idle_secs: idle,
                context_tokens: None,
                model: ev.model.clone(),
                first_prompt: ev.first_prompt.clone(),
                git_branch: ws.branch.clone(),
                cmdline: proc_.map(cmdline).unwrap_or_default(),
                children: Vec::<ChildProc>::new(),
                unregistered: false,
            };
            if alive {
                // count the wrapper's own memory too when we rooted at it
                if let (Some(r), Some(p)) = (root, proc_) {
                    if r.pid() != p.pid() {
                        info.rss_self += r.memory();
                    }
                }
                fill_tree(sys, &mut info, children);
                // fill_tree adds descendants of root_pid, which include the lock holder itself: avoid double count
                if let (Some(r), Some(p)) = (root, proc_) {
                    if r.pid() != p.pid() {
                        info.rss_tree = info.rss_tree.saturating_sub(p.memory());
                        info.children.retain(|c| c.pid != p.pid().as_u32());
                    }
                }
            }
            out.push(info);
        }
    }
    out
}
