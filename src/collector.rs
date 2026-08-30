//! Collects live Claude Code sessions by joining three sources:
//!  1. `~/.claude/sessions/<pid>.json`  – registry written by each session
//!  2. the process table (sysinfo)       – RSS / CPU / uptime, incl. child processes (MCP servers, hooks)
//!  3. `~/.claude/projects/*/<sessionId>.jsonl` – title, last activity, context tokens

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::jsonl::{JsonlCache, JsonlInfo};

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct Registry {
    pub pid: u32,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChildProc {
    pub pid: u32,
    pub rss: u64,
    pub name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub agent: &'static str,
    pub pid: u32,
    pub alive: bool,
    pub session_id: String,
    pub title: String,
    pub title_source: &'static str,
    pub cwd: String,
    pub project: String,
    pub entrypoint: String,
    pub status: Option<String>,
    pub version: Option<String>,
    /// RSS of the claude process itself (bytes)
    pub rss_self: u64,
    /// RSS of the whole process tree (bytes)
    pub rss_tree: u64,
    pub cpu: f32,
    /// seconds since process start
    pub uptime_secs: u64,
    /// seconds since the session transcript was last written (None = no transcript)
    pub idle_secs: Option<u64>,
    pub context_tokens: Option<u64>,
    pub model: Option<String>,
    pub first_prompt: Option<String>,
    pub git_branch: Option<String>,
    pub cmdline: String,
    #[serde(skip)]
    pub children: Vec<ChildProc>,
    /// true when found only in the process table (no registry entry)
    pub unregistered: bool,
}

pub struct Collector {
    sys: System,
    cache: JsonlCache,
    pub cwds: CwdCache,
    copilot_events: crate::copilot::EventsCache,
    vscode_chats: crate::copilot_vscode::ChatCache,
    claude_dir: PathBuf,
}

impl Collector {
    pub fn new() -> Self {
        let home = home_dir();
        let claude_dir = std::env::var("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".claude"));
        Self { sys: System::new(), cache: JsonlCache::default(), cwds: CwdCache::default(), copilot_events: Default::default(), vscode_chats: Default::default(), claude_dir }
    }

    pub fn collect(&mut self) -> Vec<SessionInfo> {
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_cpu()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );

        // parent -> children map
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, p) in self.sys.processes() {
            if let Some(pp) = p.parent() {
                children.entry(pp.as_u32()).or_default().push(pid.as_u32());
            }
        }

        let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let registry = self.read_registry();
        let jsonl_index = self.index_transcripts();

        let mut out = Vec::new();
        let mut seen_pids = std::collections::HashSet::new();

        for reg in registry {
            seen_pids.insert(reg.pid);
            let proc_ = self.sys.process(Pid::from_u32(reg.pid));
            // PID-reuse guard: the registry's startedAt (ms) must be close to the process start time.
            let alive = match (proc_, reg.started_at) {
                (Some(p), Some(started_ms)) => {
                    let diff = (p.start_time() as i64 - (started_ms / 1000) as i64).abs();
                    diff < 120 && is_claude(p)
                }
                (Some(p), None) => is_claude(p),
                (None, _) => false,
            };
            let mut info = Self::base_info(&mut self.cache, &reg, now_secs, &jsonl_index);
            if alive {
                let p = proc_.unwrap();
                info.alive = true;
                info.rss_self = p.memory();
                info.cpu = p.cpu_usage();
                info.uptime_secs = now_secs.saturating_sub(p.start_time());
                info.cmdline = cmdline(p);
                fill_tree(&self.sys, &mut info, &children);
            }
            out.push(info);
        }

        // Claude processes that have no registry entry (headless / older versions / crashed registry)
        for (pid, p) in self.sys.processes() {
            let pid = pid.as_u32();
            if seen_pids.contains(&pid) || !is_claude(p) {
                continue;
            }
            // skip processes whose parent is itself a claude process (forked helpers)
            if let Some(pp) = p.parent() {
                if self.sys.process(pp).map(is_claude).unwrap_or(false) {
                    continue;
                }
            }
            let cmd = cmdline(p);
            let session_id = extract_session_id(&cmd);
            let cwd = self.cwds.get(p);
            let reg = Registry { pid, session_id: session_id.clone().unwrap_or_default(), cwd, ..Default::default() };
            let mut info = Self::base_info(&mut self.cache, &reg, now_secs, &jsonl_index);
            info.alive = true;
            info.unregistered = true;
            info.rss_self = p.memory();
            info.cpu = p.cpu_usage();
            info.uptime_secs = now_secs.saturating_sub(p.start_time());
            info.cmdline = cmd;
            if info.title_source == "none" {
                info.title = "(unregistered claude process)".into();
            }
            fill_tree(&self.sys, &mut info, &children);
            out.push(info);
        }

        out.extend(crate::opencode::collect(&self.sys, &mut self.cwds, &children, now_secs));
        out.extend(crate::copilot::collect(&self.sys, &mut self.copilot_events, &children, now_secs));
        let windows = crate::copilot_vscode::windows();
        out.extend(crate::codex::collect(&self.sys, &mut self.cwds, &children, &windows, now_secs));
        let claimed: std::collections::HashSet<u32> = out.iter().filter(|s| s.alive).map(|s| s.pid).collect();
        out.extend(crate::copilot_vscode::collect(&self.sys, &mut self.vscode_chats, &children, &claimed, now_secs));
        out.sort_by(|a, b| b.rss_tree.cmp(&a.rss_tree));
        out
    }

    fn base_info(cache: &mut JsonlCache, reg: &Registry, now_secs: u64, index: &HashMap<String, PathBuf>) -> SessionInfo {
        let transcript = index.get(&reg.session_id).cloned();
        let jinfo: Option<JsonlInfo> = transcript.as_ref().map(|p| cache.info(p));
        let idle_secs = transcript.as_ref().and_then(|p| {
            std::fs::metadata(p).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| now_secs.saturating_sub(d.as_secs()))
        });

        let (title, title_source) = choose_title(reg, jinfo.as_ref());
        let project = Path::new(&reg.cwd)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| reg.cwd.clone());

        SessionInfo {
            agent: "claude",
            pid: reg.pid,
            alive: false,
            session_id: reg.session_id.clone(),
            title,
            title_source,
            cwd: reg.cwd.clone(),
            project,
            entrypoint: reg.entrypoint.clone().unwrap_or_else(|| "?".into()),
            status: reg.status.clone(),
            version: reg.version.clone(),
            rss_self: 0,
            rss_tree: 0,
            cpu: 0.0,
            uptime_secs: reg
                .started_at
                .map(|ms| now_secs.saturating_sub(ms / 1000))
                .unwrap_or(0),
            idle_secs,
            context_tokens: jinfo.as_ref().and_then(|j| j.context_tokens),
            model: jinfo.as_ref().and_then(|j| j.model.clone()),
            first_prompt: jinfo.as_ref().and_then(|j| j.first_prompt.clone()),
            git_branch: jinfo.as_ref().and_then(|j| j.git_branch.clone()),
            cmdline: String::new(),
            children: Vec::new(),
            unregistered: false,
        }
    }
}

pub fn fill_tree(sys: &System, info: &mut SessionInfo, children: &HashMap<u32, Vec<u32>>) {
    fill_tree_excluding(sys, info, children, &std::collections::HashSet::new())
}

/// Like `fill_tree`, but subtrees rooted at any pid in `exclude` are skipped
/// (used so a VS Code extension host does not re-count the claude/copilot sessions it spawned).
pub fn fill_tree_excluding(sys: &System, info: &mut SessionInfo, children: &HashMap<u32, Vec<u32>>, exclude: &std::collections::HashSet<u32>) {
        let mut total = info.rss_self;
        let mut stack: Vec<u32> = children.get(&info.pid).cloned().unwrap_or_default();
        let mut guard = 0;
        while let Some(c) = stack.pop() {
            guard += 1;
            if guard > 2000 {
                break;
            }
            if exclude.contains(&c) {
                continue;
            }
            if let Some(p) = sys.process(Pid::from_u32(c)) {
                total += p.memory();
                info.children.push(ChildProc {
                    pid: c,
                    rss: p.memory(),
                    name: short_cmd(p),
                });
                if let Some(cc) = children.get(&c) {
                    stack.extend(cc.iter().copied());
                }
            }
        }
        info.rss_tree = total;
}

impl Collector {
    fn read_registry(&self) -> Vec<Registry> {
        let dir = self.claude_dir.join("sessions");
        let mut v = Vec::new();
        let Ok(rd) = std::fs::read_dir(&dir) else { return v };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok(mut r) = serde_json::from_str::<Registry>(&s) {
                    if r.pid == 0 {
                        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                            r.pid = stem.parse().unwrap_or(0);
                        }
                    }
                    v.push(r);
                }
            }
        }
        v
    }

    /// sessionId -> transcript path
    fn index_transcripts(&self) -> HashMap<String, PathBuf> {
        let mut m = HashMap::new();
        let Ok(rd) = std::fs::read_dir(self.claude_dir.join("projects")) else { return m };
        for proj in rd.flatten() {
            let Ok(files) = std::fs::read_dir(proj.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        m.insert(stem.to_string(), p.clone());
                    }
                }
            }
        }
        m
    }
}

fn choose_title(reg: &Registry, j: Option<&JsonlInfo>) -> (String, &'static str) {
    if let Some(t) = j.and_then(|j| j.custom_title.clone()) {
        return (t, "custom");
    }
    if let Some(t) = j.and_then(|j| j.ai_title.clone()) {
        return (t, "ai");
    }
    if let Some(t) = j.and_then(|j| j.first_prompt.clone()) {
        return (t, "prompt");
    }
    if let Some(n) = &reg.name {
        return (n.clone(), "name");
    }
    ("(no title)".to_string(), "none")
}

fn is_claude(p: &sysinfo::Process) -> bool {
    proc_named(p, "claude") && !is_claude_desktop(p)
}

/// The Claude Desktop app is also `claude.exe` on Windows (and `Claude` on macOS);
/// it is not a Claude Code session.
fn is_claude_desktop(p: &sysinfo::Process) -> bool {
    p.exe()
        .map(|e| {
            let s = e.to_string_lossy();
            s.contains("AnthropicClaude") || s.contains("Claude.app")
        })
        .unwrap_or(false)
}

/// True if the process name or exe basename equals `want` after removing a
/// trailing `.exe` (Windows reports `codex.exe`, macOS/Linux report `codex`).
/// The suffix check is case-insensitive; the name itself is compared exactly.
pub fn proc_named(p: &sysinfo::Process, want: &str) -> bool {
    exe_stem(&p.name().to_string_lossy()) == want
        || p.exe()
            .and_then(|e| e.file_name())
            .map(|n| exe_stem(&n.to_string_lossy()) == want)
            .unwrap_or(false)
}

/// Drop the Windows extended-length prefix (`\\?\C:\x` -> `C:\x`,
/// `\\?\UNC\srv\share` -> `\\srv\share`) that some tools store in their state.
pub fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

fn exe_stem(name: &str) -> &str {
    match name.len().checked_sub(4) {
        Some(i) if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".exe") => &name[..i],
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::{exe_stem, strip_verbatim_prefix};

    #[test]
    fn strips_windows_verbatim_prefix() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\Users\yurik"), r"C:\Users\yurik");
        assert_eq!(strip_verbatim_prefix(r"\\?\UNC\srv\share\dir"), r"\\srv\share\dir");
        assert_eq!(strip_verbatim_prefix("/Users/gorosun"), "/Users/gorosun");
        assert_eq!(strip_verbatim_prefix(r"C:\plain"), r"C:\plain");
    }

    #[test]
    fn exe_stem_strips_windows_suffix() {
        assert_eq!(exe_stem("codex"), "codex");
        assert_eq!(exe_stem("codex.exe"), "codex");
        assert_eq!(exe_stem("codex.EXE"), "codex");
        assert_eq!(exe_stem("codex.exe.bak"), "codex.exe.bak");
        // macOS Claude Desktop is `Claude`; it must not equal `claude`
        assert_ne!(exe_stem("Claude"), "claude");
    }
}

pub fn cmdline(p: &sysinfo::Process) -> String {
    p.cmd().iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ")
}

pub fn short_cmd(p: &sysinfo::Process) -> String {
    let cmd = p.cmd();
    if cmd.is_empty() {
        return p.name().to_string_lossy().to_string();
    }
    let mut parts: Vec<String> = cmd
        .iter()
        .filter(|s| !s.to_string_lossy().contains('='))
        .take(4)
        .map(|s| {
            let s = s.to_string_lossy();
            // shorten long paths to basename
            if s.contains('/') && !s.starts_with('-') {
                Path::new(s.as_ref()).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(s.to_string())
            } else {
                s.to_string()
            }
        })
        .collect();
    if cmd.len() > 4 {
        parts.push("…".into());
    }
    parts.join(" ")
}

fn extract_session_id(cmd: &str) -> Option<String> {
    for key in ["--session-id", "--resume", "-r"] {
        if let Some(pos) = cmd.find(key) {
            let rest = &cmd[pos + key.len()..];
            let rest = rest.trim_start_matches('=').trim_start();
            let id: String = rest.chars().take_while(|c| c.is_ascii_hexdigit() || *c == '-').collect();
            if id.len() == 36 {
                return Some(id);
            }
        }
    }
    None
}

/// Home directory: `$HOME` on Unix, `%USERPROFILE%` on Windows.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// cwd lookup with fallback to `lsof` (sysinfo returns nothing for some processes on macOS).
/// Results are cached per (pid, start_time) so lsof runs only for newly seen processes.
#[derive(Default)]
pub struct CwdCache {
    map: HashMap<(u32, u64), String>,
}

impl CwdCache {
    pub fn get(&mut self, p: &sysinfo::Process) -> String {
        let key = (p.pid().as_u32(), p.start_time());
        if let Some(c) = self.map.get(&key) {
            return c.clone();
        }
        let mut cwd = p.cwd().map(|c| strip_verbatim_prefix(&c.display().to_string())).unwrap_or_default();
        if cwd.is_empty() {
            cwd = lsof_cwd(key.0).unwrap_or_default();
        }
        self.map.insert(key, cwd.clone());
        cwd
    }
}

#[cfg(unix)]
fn lsof_cwd(pid: u32) -> Option<String> {
    let out = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().find_map(|l| l.strip_prefix('n')).map(|s| s.to_string())
}

#[cfg(not(unix))]
fn lsof_cwd(_pid: u32) -> Option<String> {
    None
}
