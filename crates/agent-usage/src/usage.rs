// Incremental reader of the local Claude Code and Codex logs. Read-only; it keeps counters, model names,
// folder names and turn boundaries, never message text. No network.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const DAY: i64 = 86_400_000;
/// How far back the logs are read.
const WINDOW: i64 = 8 * DAY;
const IDLE: i64 = 600_000; // a turn silent this long is not "working"

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prov {
    Claude,
    Codex,
}
impl Prov {
    pub fn id(self) -> &'static str {
        match self { Prov::Claude => "claude", Prov::Codex => "codex" }
    }
    pub fn name(self) -> &'static str {
        match self { Prov::Claude => "Claude Code", Prov::Codex => "Codex" }
    }
}

struct Rec {
    ts: i64,
    prov: Prov,
    model: String,
    project: String,
    inp: u64,
    out: u64,
    cr: u64,
    cw: u64,
    cw1h: u64,
}

#[derive(Default)]
struct FileSt {
    off: u64,
    session: String,
    cwd: String,
    model: String,
    last_total: u64,
}

#[derive(Default, Clone)]
struct Sess {
    project: String,
    model: String,
    start: Option<i64>,
    last: i64,
}

pub struct Ended {
    pub prov: Prov,
    pub project: String,
    pub duration_ms: i64,
    pub at: i64,
    pub session: String,
}

#[derive(Default)]
pub struct Store {
    files: HashMap<PathBuf, FileSt>,
    recs: HashMap<String, Rec>,
    sess: HashMap<(Prov, String), Sess>,
    seen: HashSet<Prov>,
}

// ── time ─────────────────────────────────────────────────────────────────────

fn days_from_civil(mut y: i64, m: i64, d: i64) -> i64 {
    if m <= 2 {
        y -= 1;
    }
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

/// "2026-10-10T12:53:13.152Z" -> epoch ms.
pub fn parse_iso(s: &str) -> Option<i64> {
    let n = |a: usize, l: usize| s.get(a..a + l)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (n(0, 4)?, n(5, 2)?, n(8, 2)?, n(11, 2)?, n(14, 2)?, n(17, 2)?);
    let ms = if s.as_bytes().get(19) == Some(&b'.') { n(20, 3).unwrap_or(0) } else { 0 };
    Some(((days_from_civil(y, mo, d) * 24 + h) * 60 + mi) * 60_000 + se * 1000 + ms)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Minutes to add to local time to get UTC (Windows `Bias`, daylight included).
pub fn bias_ms() -> i64 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz = TIME_ZONE_INFORMATION::default();
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    (tz.Bias + if r == 2 { tz.DaylightBias } else { tz.StandardBias }) as i64 * 60_000
}

/// "claude-opus-5-5-20260101" -> "Opus 5.5"; other ids as they are.
fn display(model: &str) -> String {
    let Some(rest) = model.strip_prefix("claude-") else { return model.to_string() };
    let mut parts = rest.split('-').filter(|p| !(p.len() >= 8 && p.chars().all(|c| c.is_ascii_digit())));
    let fam = parts.next().unwrap_or("");
    let ver: Vec<&str> = parts.collect();
    let fam = fam[..1].to_uppercase() + &fam[1..];
    if ver.is_empty() { fam } else { format!("{fam} {}", ver.join(".")) }
}

fn project_of(cwd: &str) -> String {
    let c = cwd.replace('\\', "/");
    let c = c.split("/.claude/worktrees/").next().unwrap_or(&c).trim_end_matches('/');
    match c.rsplit('/').next() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => "Unknown".into(),
    }
}

// ── roots ────────────────────────────────────────────────────────────────────

pub fn roots() -> Vec<(Prov, PathBuf)> {
    let home = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    let claude = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| home.join(".claude")).join("projects");
    let codex = std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".codex"));
    [(Prov::Claude, claude), (Prov::Codex, codex.join("sessions")), (Prov::Codex, codex.join("archived_sessions"))]
        .into_iter()
        .filter(|(_, p)| p.is_dir())
        .collect()
}

fn walk(dir: &Path, cutoff_s: u64, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&p, cutoff_s, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") && md.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).is_some_and(|d| d.as_secs() >= cutoff_s) {
            out.push(p);
        }
    }
}

fn has(line: &[u8], pat: &str) -> bool {
    let p = pat.as_bytes();
    let mut i = 0;
    while let Some(k) = line[i..].iter().position(|&b| b == p[0]) {
        i += k;
        if line[i..].starts_with(p) {
            return true;
        }
        i += 1;
    }
    false
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

// ── store ────────────────────────────────────────────────────────────────────

/// Every log touched in the last 8 days. Touches no counters, so it runs outside the store's lock.
pub fn logs(roots: &[(Prov, PathBuf)], now: i64) -> Vec<(Prov, PathBuf)> {
    let mut out = vec![];
    for (prov, root) in roots {
        let mut files = vec![];
        walk(root, ((now - WINDOW) / 1000) as u64, &mut files);
        out.extend(files.into_iter().map(|f| (*prov, f)));
    }
    out
}

impl Store {
    /// Reads what is new in every log touched in the last 8 days. Returns the turns that ended while
    /// reading when `report` is set (never on the first full read).
    pub fn scan(&mut self, roots: &[(Prov, PathBuf)], now: i64, report: bool) -> Vec<Ended> {
        let mut ended = vec![];
        self.found(roots);
        for (prov, f) in logs(roots, now) {
            self.read(prov, &f, now, report, &mut ended);
        }
        ended
    }

    /// A provider whose log folder exists is shown, with or without recent logs.
    pub fn found(&mut self, roots: &[(Prov, PathBuf)]) {
        self.seen.extend(roots.iter().map(|(p, _)| *p));
    }

    /// What is new in one log. The unit of work the feature holds its lock for.
    pub fn read(&mut self, prov: Prov, path: &Path, now: i64, report: bool, ended: &mut Vec<Ended>) {
        let cutoff = now - WINDOW;
        let Ok(file) = File::open(path) else { return };
        let len = file.metadata().map_or(0, |m| m.len());
        let mut st = self.files.remove(path).unwrap_or_default();
        if len < st.off {
            st = FileSt::default();
        }
        if len > st.off {
            let mut rd = BufReader::with_capacity(1 << 20, file);
            if rd.seek(SeekFrom::Start(st.off)).is_ok() {
                let mut buf = vec![];
                loop {
                    buf.clear();
                    match rd.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if buf.last() != Some(&b'\n') {
                                break; // partial line: wait for the rest
                            }
                            st.off += n as u64;
                            let line = buf.trim_ascii_end(); // drops CRLF too
                            if line.is_empty() {
                                continue;
                            }
                            match prov {
                                Prov::Claude => self.claude(line, &mut st, cutoff, report, ended),
                                Prov::Codex => self.codex(line, path, &mut st, cutoff, report, ended),
                            }
                        }
                    }
                }
            }
        }
        self.files.insert(path.to_path_buf(), st);
    }

    fn end_turn(&mut self, prov: Prov, session: &str, ts: i64, notify: bool, report: bool, dur: Option<i64>, ended: &mut Vec<Ended>) {
        if let Some(s) = self.sess.get_mut(&(prov, session.to_string())) {
            if let Some(start) = s.start.take() {
                if notify && report {
                    ended.push(Ended { prov, project: s.project.clone(), duration_ms: dur.unwrap_or(ts - start), at: ts, session: session.to_string() });
                }
            }
        }
    }

    fn claude(&mut self, line: &[u8], st: &mut FileSt, cutoff: i64, report: bool, ended: &mut Vec<Ended>) {
        let assistant = has(line, "\"type\":\"assistant\"");
        let user = !assistant && has(line, "\"type\":\"user\"") && !has(line, "\"type\":\"tool_result\"");
        if !assistant && !user {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        let Some(ts) = v["timestamp"].as_str().and_then(parse_iso) else { return };
        if ts < cutoff {
            return;
        }
        let side = v["isSidechain"].as_bool() == Some(true);
        let session = v["sessionId"].as_str().unwrap_or("").to_string();
        if let Some(c) = v["cwd"].as_str() {
            if st.cwd != c {
                st.cwd = c.to_string();
            }
        }
        let project = project_of(&st.cwd);
        let msg = &v["message"];
        if assistant {
            let model = msg["model"].as_str().unwrap_or("");
            let us = &msg["usage"];
            if us.is_object() && !model.starts_with('<') && !model.is_empty() {
                let key = format!("c:{}:{}", msg["id"].as_str().unwrap_or(""), v["requestId"].as_str().unwrap_or(""));
                let cw = u(&us["cache_creation_input_tokens"]);
                self.recs.insert(key, Rec {
                    ts, prov: Prov::Claude, model: model.to_string(), project: project.clone(),
                    inp: u(&us["input_tokens"]), out: u(&us["output_tokens"]), cr: u(&us["cache_read_input_tokens"]),
                    cw, cw1h: u(&us["cache_creation"]["ephemeral_1h_input_tokens"]).min(cw),
                });
            }
            if side {
                return;
            }
            let s = self.sess.entry((Prov::Claude, session.clone())).or_default();
            s.project = project;
            s.model = model.to_string();
            s.last = ts;
            if s.start.is_none() {
                s.start = Some(ts);
            }
            if matches!(msg["stop_reason"].as_str(), Some("end_turn" | "stop_sequence" | "max_tokens" | "refusal")) {
                let ok = v["isApiErrorMessage"].as_bool() != Some(true);
                self.end_turn(Prov::Claude, &session, ts, ok, report, None, ended);
            }
        } else if !side && v["isMeta"].as_bool() != Some(true) {
            let c = &msg["content"];
            let text = c.as_str().or_else(|| c.get(0).and_then(|b| b["text"].as_str())).unwrap_or("");
            let silent = text.starts_with("[Request interrupted by user") || text.starts_with("<local-command-std");
            let real = c.is_string() || c.get(0).is_some_and(|b| b["type"] == "text");
            if silent {
                self.end_turn(Prov::Claude, &session, ts, false, report, None, ended);
            } else if real {
                let s = self.sess.entry((Prov::Claude, session)).or_default();
                s.project = project;
                s.start = Some(ts);
                s.last = ts;
            }
        }
    }

    fn codex(&mut self, line: &[u8], path: &Path, st: &mut FileSt, cutoff: i64, report: bool, ended: &mut Vec<Ended>) {
        let hit = ["token_count", "task_started", "task_complete", "turn_aborted", "session_meta", "turn_context"].iter().any(|k| has(line, k));
        if !hit {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else { return };
        let p = &v["payload"];
        let ts = v["timestamp"].as_str().and_then(parse_iso).unwrap_or(0);
        match v["type"].as_str() {
            Some("session_meta") => {
                st.session = p["id"].as_str().unwrap_or("").to_string();
                if let Some(c) = p["cwd"].as_str() {
                    st.cwd = c.to_string();
                }
            }
            Some("turn_context") => {
                if let Some(m) = p["model"].as_str() {
                    st.model = m.to_string();
                }
                if let Some(c) = p["cwd"].as_str() {
                    st.cwd = c.to_string();
                }
            }
            Some("event_msg") => {
                if st.session.is_empty() {
                    st.session = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                }
                let side = path.file_name().is_some_and(|n| n.to_string_lossy().contains('_'));
                let project = project_of(&st.cwd);
                match p["type"].as_str() {
                    Some("token_count") if ts >= cutoff => {
                        let info = &p["info"];
                        let total = u(&info["total_token_usage"]["total_tokens"]);
                        if total != 0 && total != st.last_total {
                            st.last_total = total;
                            let l = &info["last_token_usage"];
                            let cached = u(&l["cached_input_tokens"]);
                            self.recs.insert(format!("x:{}:{}", path.display(), total), Rec {
                                ts, prov: Prov::Codex, model: st.model.clone(), project,
                                inp: u(&l["input_tokens"]).saturating_sub(cached), out: u(&l["output_tokens"]), cr: cached, cw: 0, cw1h: 0,
                            });
                        }
                    }
                    Some("task_started") if !side => {
                        let s = self.sess.entry((Prov::Codex, st.session.clone())).or_default();
                        s.project = project;
                        s.model = st.model.clone();
                        s.start = Some(ts);
                        s.last = ts;
                    }
                    Some("task_complete") if !side => {
                        let d = p["duration_ms"].as_i64();
                        self.end_turn(Prov::Codex, &st.session.clone(), ts, true, report, d, ended);
                    }
                    Some("turn_aborted") if !side => self.end_turn(Prov::Codex, &st.session.clone(), ts, false, report, None, ended),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Everything the page draws: per provider today, this week (7 local days) and the open turns.
    pub fn summary(&self, now: i64) -> Value {
        let bias = bias_ms();
        let today = (now - bias).div_euclid(DAY) * DAY + bias;
        let week = today - 6 * DAY;
        let mut provs = vec![];
        for prov in [Prov::Claude, Prov::Codex] {
            if !self.seen.contains(&prov) {
                continue;
            }
            let period = |from: i64| {
                let (mut t, mut n) = ([0u64; 4], 0u64);
                let mut models: HashMap<String, u64> = HashMap::new();
                let mut projects: HashMap<String, u64> = HashMap::new();
                let mut days = [0u64; 7];
                for r in self.recs.values().filter(|r| r.prov == prov && r.ts >= week) {
                    let tok = r.inp + r.out + r.cr + r.cw;
                    days[(((r.ts - week) / DAY) as usize).min(6)] += tok;
                    if r.ts < from {
                        continue;
                    }
                    t[0] += r.inp; t[1] += r.out; t[2] += r.cr; t[3] += r.cw;
                    n += 1;
                    for (m, k) in [(&mut models, display(&r.model)), (&mut projects, r.project.clone())] {
                        *m.entry(k).or_default() += tok;
                    }
                }
                let rank = |m: HashMap<String, u64>| {
                    let mut v: Vec<_> = m.into_iter().collect();
                    v.sort_by(|a, b| b.1.cmp(&a.1));
                    v.into_iter().map(|(name, tokens)| json!({ "name": name, "tokens": tokens })).collect::<Vec<_>>()
                };
                json!({
                    "input": t[0], "output": t[1], "cacheRead": t[2], "cacheWrite": t[3], "total": t.iter().sum::<u64>(),
                    "requests": n,
                    "models": rank(models), "projects": rank(projects), "days": days,
                })
            };
            let live: Vec<Value> = self.sess.iter()
                .filter(|((p, _), s)| *p == prov && s.start.is_some() && now - s.last < IDLE)
                .map(|((_, id), s)| json!({ "session": id, "project": s.project, "model": display(&s.model), "startedAt": s.start, "last": s.last }))
                .collect();
            provs.push(json!({ "id": prov.id(), "name": prov.name(), "today": period(today), "week": period(week), "live": live }));
        }
        json!({ "providers": provs, "at": now })
    }
}
