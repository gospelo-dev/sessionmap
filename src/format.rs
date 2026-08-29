pub fn bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let m = b as f64 / MB;
    if m >= 1024.0 { format!("{:.2}G", m / 1024.0) } else { format!("{:.0}M", m) }
}

pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    if d > 0 { format!("{d}d{h:02}h") }
    else if h > 0 { format!("{h}h{m:02}m") }
    else if m > 0 { format!("{m}m{s:02}s") }
    else { format!("{s}s") }
}

pub fn tokens(t: Option<u64>) -> String {
    match t {
        None => "-".into(),
        Some(t) if t >= 1000 => format!("{:.0}k", t as f64 / 1000.0),
        Some(t) => t.to_string(),
    }
}

/// truncate by display width (roughly: CJK counts 2)
pub fn truncate(s: &str, max: usize) -> String {
    let mut w = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = if is_wide(c) { 2 } else { 1 };
        if w + cw > max.saturating_sub(1) {
            out.push('…');
            return out;
        }
        w += cw;
        out.push(c);
    }
    out
}

fn is_wide(c: char) -> bool {
    let u = c as u32;
    (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE4F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1FAFF).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u)
}
