//! GitHub Copilot Chat inside VS Code.
//! There is no per-session process: chats run in the VS Code extension host.
//! `~/.copilot/ide/<uuid>.lock` (written by the Copilot extension) gives the
//! extension-host pid and the workspace folders of each window; chat sessions
//! live in `~/Library/Application Support/Code/User/workspaceStorage/<hash>/chatSessions/*.jsonl`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use sysinfo::{Pid, System};

use crate::collector::{ChildProc, SessionInfo, cmdline, fill_tree_excluding};

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

fn ide_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("COPILOT_HOME") {
        return PathBuf::from(p).join("ide");
    }
    home().join(".copilot/ide")
}

fn workspace_storage_dirs() -> Vec<PathBuf> {
    let h = home();
    let mut v = vec![
        h.join("Library/Application Support/Code/User/workspaceStorage"),
        h.join("Library/Application Support/Code - Insiders/User/workspaceStorage"),
        h.join(".config/Code/User/workspaceStorage"),
        h.join(".config/Code - Insiders/User/workspaceStorage"),
    ];
    if let Some(p) = std::env::var_os("VSCODE_WORKSPACE_STORAGE") {
        v.insert(0, PathBuf::from(p));
    }
    v
}

#[derive(serde::Deserialize)]
struct IdeLock {
    pid: u32,
    #[serde(default, rename = "ideName")]
    ide_name: Option<String>,
    #[serde(default, rename = "workspaceFolders")]
    workspace_folders: Vec<String>,
    #[serde(default)]
    timestamp: Option<u64>,
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// folder path -> chatSessions dirs (the same folder can appear under Code and Code - Insiders)
fn chat_dirs_by_folder() -> HashMap<String, Vec<PathBuf>> {
    let mut m: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for base in workspace_storage_dirs() {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        for e in rd.flatten() {
            let ws = e.path().join("workspace.json");
            let Ok(txt) = std::fs::read_to_string(&ws) else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { continue };
            let Some(uri) = v.get("folder").and_then(|f| f.as_str()) else { continue };
            let Some(path) = uri.strip_prefix("file://") else { continue };
            let path = percent_decode(path).trim_end_matches('/').to_string();
            m.entry(path).or_default().push(e.path().join("chatSessions"));
        }
    }
    m
}

#[derive(Default, Clone)]
struct Chat {
    title: Option<String>,
    first_prompt: Option<String>,
    model: Option<String>,
    prompt_tokens: Option<u64>,
}

#[derive(Default)]
pub struct ChatCache {
    map: HashMap<PathBuf, (u64, Chat)>,
}

impl ChatCache {
    fn get(&mut self, path: &Path) -> Chat {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if let Some((l, c)) = self.map.get(path) {
            if *l == len {
                return c.clone();
            }
        }
        let mut c = Chat::default();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                let kind = v.get("kind").and_then(|k| k.as_u64()).unwrap_or(99);
                let val = v.get("v");
                if kind == 0 {
                    // full snapshot
                    if let Some(s) = val {
                        if let Some(t) = s.get("customTitle").and_then(|t| t.as_str()) {
                            c.title = Some(t.to_string());
                        }
                        if let Some(m) = s.pointer("/inputState/selectedModel/metadata/id").and_then(|m| m.as_str()) {
                            c.model = Some(m.to_string());
                        }
                        if let Some(reqs) = s.get("requests").and_then(|r| r.as_array()) {
                            for r in reqs {
                                take_request(&mut c, r);
                            }
                        }
                    }
                } else {
                    // patch: {"k":[...path...],"v":...}
                    let keys: Vec<String> = v
                        .get("k")
                        .and_then(|k| k.as_array())
                        .map(|a| a.iter().map(|x| x.as_str().map(|s| s.to_string()).unwrap_or_else(|| x.to_string())).collect())
                        .unwrap_or_default();
                    match keys.first().map(|s| s.as_str()) {
                        Some("customTitle") => {
                            if let Some(t) = val.and_then(|t| t.as_str()) {
                                c.title = Some(t.to_string());
                            }
                        }
                        Some("requests") => {
                            if keys.len() == 1 {
                                if let Some(arr) = val.and_then(|a| a.as_array()) {
                                    for r in arr {
                                        take_request(&mut c, r);
                                    }
                                } else if let Some(r) = val {
                                    take_request(&mut c, r);
                                }
                            } else if keys.len() == 2 {
                                if let Some(r) = val {
                                    take_request(&mut c, r);
                                }
                            } else if keys.get(2).map(|s| s.as_str()) == Some("promptTokens") {
                                if let Some(n) = val.and_then(|n| n.as_u64()) {
                                    c.prompt_tokens = Some(n);
                                }
                            }
                        }
                        Some("inputState") => {
                            if keys.get(1).map(|s| s.as_str()) == Some("selectedModel") {
                                if let Some(m) = val.and_then(|m| m.pointer("/metadata/id")).and_then(|m| m.as_str()) {
                                    c.model = Some(m.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        self.map.insert(path.to_path_buf(), (len, c.clone()));
        c
    }
}

fn take_request(c: &mut Chat, r: &serde_json::Value) {
    if c.first_prompt.is_none() {
        if let Some(t) = r.pointer("/message/text").and_then(|t| t.as_str()) {
            let one: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
            if !one.is_empty() {
                c.first_prompt = Some(one.chars().take(120).collect());
            }
        }
    }
    if let Some(n) = r.get("promptTokens").and_then(|n| n.as_u64()) {
        c.prompt_tokens = Some(n);
    }
    if let Some(m) = r.pointer("/modelState/metadata/id").or_else(|| r.get("modelId")).and_then(|m| m.as_str()) {
        c.model = Some(m.trim_start_matches("copilot/").to_string());
    }
}

fn mtime_secs(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

pub fn collect(
    sys: &System,
    cache: &mut ChatCache,
    children: &HashMap<u32, Vec<u32>>,
    exclude: &HashSet<u32>,
    now_secs: u64,
) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(ide_dir()) else { return out };
    let chat_dirs = chat_dirs_by_folder();
    let mut seen: HashSet<u32> = HashSet::new();

    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("lock") {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&p) else { continue };
        let Ok(lock) = serde_json::from_str::<IdeLock>(&txt) else { continue };
        if !seen.insert(lock.pid) {
            continue;
        }
        let proc_ = sys.process(Pid::from_u32(lock.pid));
        let alive = proc_.is_some();
        let folder = lock.workspace_folders.first().cloned().unwrap_or_default();
        let folder = folder.trim_end_matches('/').to_string();
        let project = Path::new(&folder).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| folder.clone());

        // chat session files in this workspace, newest first
        let mut files: Vec<(u64, PathBuf)> = Vec::new();
        for dir in chat_dirs.get(&folder).map(|v| v.as_slice()).unwrap_or(&[]) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for f in rd.flatten() {
                    let fp = f.path();
                    if fp.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                        if let Some(m) = mtime_secs(&fp) {
                            files.push((m, fp));
                        }
                    }
                }
            }
        }
        files.sort_by(|a, b| b.0.cmp(&a.0));
        let recent = files.iter().filter(|(m, _)| now_secs.saturating_sub(*m) < 86400).count();
        let idle = files.first().map(|(m, _)| now_secs.saturating_sub(*m));
        // newest chat that actually has content (a freshly opened empty chat has neither title nor prompt)
        let mut picked: Option<(&PathBuf, Chat)> = None;
        for (_, fp) in files.iter().take(8) {
            let c = cache.get(fp);
            if c.title.is_some() || c.first_prompt.is_some() {
                picked = Some((fp, c));
                break;
            }
        }
        let chat = picked.as_ref().map(|(_, c)| c.clone()).unwrap_or_default();
        let (mut title, title_source) = match (&chat.title, &chat.first_prompt) {
            (Some(t), _) => (t.clone(), "custom"),
            (None, Some(p)) => (p.clone(), "prompt"),
            _ if files.is_empty() => ("(no copilot chat in this window)".to_string(), "none"),
            _ => ("(empty chat)".to_string(), "none"),
        };
        if recent > 1 {
            title.push_str(&format!("  [+{} chats/24h]", recent - 1));
        }
        let session_id = picked
            .as_ref()
            .map(|(fp, _)| *fp)
            .or_else(|| files.first().map(|(_, fp)| fp))
            .and_then(|fp| fp.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default();

        let mut info = SessionInfo {
            agent: "copilot",
            pid: lock.pid,
            alive,
            session_id,
            title,
            title_source,
            cwd: folder,
            project,
            entrypoint: "vscode".to_string(),
            status: idle.filter(|i| *i < 15).map(|_| "busy".to_string()),
            version: lock.ide_name.clone(),
            rss_self: proc_.map(|p| p.memory()).unwrap_or(0),
            rss_tree: 0,
            cpu: proc_.map(|p| p.cpu_usage()).unwrap_or(0.0),
            uptime_secs: proc_
                .map(|p| now_secs.saturating_sub(p.start_time()))
                .or_else(|| lock.timestamp.map(|t| now_secs.saturating_sub(t / 1000)))
                .unwrap_or(0),
            idle_secs: idle,
            context_tokens: chat.prompt_tokens,
            model: chat.model.clone(),
            first_prompt: chat.first_prompt.clone(),
            git_branch: None,
            cmdline: proc_.map(cmdline).unwrap_or_default(),
            children: Vec::<ChildProc>::new(),
            unregistered: false,
        };
        if alive {
            fill_tree_excluding(sys, &mut info, children, exclude);
        }
        out.push(info);
    }
    out
}
