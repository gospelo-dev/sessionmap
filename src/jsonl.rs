//! Incremental reader for session transcripts (`<sessionId>.jsonl`).
//! Only appended bytes are scanned on refresh, so huge transcripts stay cheap.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct JsonlInfo {
    pub custom_title: Option<String>,
    pub ai_title: Option<String>,
    pub first_prompt: Option<String>,
    pub context_tokens: Option<u64>,
    pub model: Option<String>,
    pub git_branch: Option<String>,
}

#[derive(Default)]
struct Entry {
    offset: u64,
    info: JsonlInfo,
}

#[derive(Default)]
pub struct JsonlCache {
    entries: HashMap<PathBuf, Entry>,
}

impl JsonlCache {
    pub fn info(&mut self, path: &Path) -> JsonlInfo {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let entry = self.entries.entry(path.to_path_buf()).or_default();
        if len < entry.offset {
            // truncated / rewritten: start over
            *entry = Entry::default();
        }
        if len > entry.offset {
            if let Ok(mut f) = File::open(path) {
                if f.seek(SeekFrom::Start(entry.offset)).is_ok() {
                    let mut reader = BufReader::with_capacity(1 << 16, f);
                    let mut line = String::new();
                    let mut consumed = entry.offset;
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                // only accept complete lines; a partial tail will be re-read next time
                                if !line.ends_with('\n') {
                                    break;
                                }
                                consumed += n as u64;
                                apply_line(&mut entry.info, &line);
                            }
                        }
                    }
                    entry.offset = consumed;
                }
            }
        }
        entry.info.clone()
    }
}

fn apply_line(info: &mut JsonlInfo, line: &str) {
    // cheap pre-filter before JSON parsing
    let interesting = line.contains("\"custom-title\"")
        || line.contains("\"ai-title\"")
        || line.contains("\"usage\"")
        || (info.first_prompt.is_none() && line.contains("\"type\":\"user\""))
        || (info.git_branch.is_none() && line.contains("\"gitBranch\""));
    if !interesting {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "custom-title" => {
            info.custom_title = v.get("customTitle").and_then(|t| t.as_str()).map(|s| s.to_string());
        }
        "ai-title" => {
            info.ai_title = v.get("aiTitle").and_then(|t| t.as_str()).map(|s| s.to_string());
        }
        "user" => {
            if info.git_branch.is_none() {
                info.git_branch = v.get("gitBranch").and_then(|t| t.as_str()).map(|s| s.to_string());
            }
            if info.first_prompt.is_none() && !v.get("isSidechain").and_then(|b| b.as_bool()).unwrap_or(false) {
                if let Some(text) = user_text(&v) {
                    info.first_prompt = Some(text);
                }
            }
        }
        "assistant" => {
            if let Some(msg) = v.get("message") {
                if let Some(m) = msg.get("model").and_then(|m| m.as_str()) {
                    info.model = Some(m.to_string());
                }
                if let Some(u) = msg.get("usage") {
                    let g = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                    let total = g("input_tokens") + g("cache_read_input_tokens") + g("cache_creation_input_tokens");
                    if total > 0 {
                        info.context_tokens = Some(total);
                    }
                }
            }
        }
        _ => {}
    }
}

fn user_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    let raw = if let Some(s) = content.as_str() {
        s.to_string()
    } else {
        let arr = content.as_array()?;
        let mut s = String::new();
        for c in arr {
            if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                    s.push_str(t);
                    s.push(' ');
                }
            }
        }
        s
    };
    let raw = raw.trim();
    // skip system-ish injected content
    if raw.is_empty() || raw.starts_with('<') || raw.starts_with("[Request interrupted") {
        return None;
    }
    let one_line: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(one_line.chars().take(120).collect())
}
