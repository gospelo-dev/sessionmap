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
    claude_dir: PathBuf,
}

impl Collector {
    pub fn new() -> Self {
        let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"));
        let claude_dir = std::env::var("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".claude"));
        Self { sys: System::new(), cache: JsonlCache::default(), claude_dir }
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
                Self::fill_tree(&self.sys, &mut info, &children);
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
            let cwd = p.cwd().map(|c| c.display().to_string()).unwrap_or_default();
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
            Self::fill_tree(&self.sys, &mut info, &children);
            out.push(info);
        }

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

    fn fill_tree(sys: &System, info: &mut SessionInfo, children: &HashMap<u32, Vec<u32>>) {
        let mut total = info.rss_self;
        let mut stack: Vec<u32> = children.get(&info.pid).cloned().unwrap_or_default();
        let mut guard = 0;
        while let Some(c) = stack.pop() {
            guard += 1;
            if guard > 2000 {
                break;
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
    let name = p.name().to_string_lossy();
    if name == "claude" {
        return true;
    }
    // exe path may be a symlink target; be tolerant
    p.exe()
        .and_then(|e| e.file_name())
        .map(|n| n.to_string_lossy() == "claude")
        .unwrap_or(false)
}

fn cmdline(p: &sysinfo::Process) -> String {
    p.cmd().iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ")
}

fn short_cmd(p: &sysinfo::Process) -> String {
    let cmd = p.cmd();
    if cmd.is_empty() {
        return p.name().to_string_lossy().to_string();
    }
    let mut parts: Vec<String> = cmd
        .iter()
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
