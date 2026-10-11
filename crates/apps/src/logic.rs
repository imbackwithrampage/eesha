// The parts of Apps that touch nothing: versions, rules, the schedule and what
// winget's output means. Version comparison, rules and the schedule are ported
// from reference/vorssaint-utils/Sources/Vorssaint/Services/AppUpdates/AppUpdatesSupport.swift;
// the progress reader follows HomebrewProgressParser in …/Services/Homebrew/HomebrewSupport.swift
// and the app's own version check SemanticVersion in …/Services/Update/UpdateServiceSupport.swift.

use std::cmp::Ordering::{self, *};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use svc_apps::winget;

// ── Versions ──────────────────────────────────────────────────────────────────

/// The part of a version that lines up between sources: no leading `v`, no revision after a comma.
pub fn version_core(version: &str) -> &str {
    let v = version.trim();
    let v = match v.strip_prefix(['v', 'V']) {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => v,
    };
    v.split(',').next().unwrap_or(v)
}

/// A version that says nothing ("Unknown", "latest", "< 3.2", a cut-off "1.2…") is never
/// reported as an update: it could not be reported honestly.
pub fn comparable(version: &str) -> bool {
    let v = version_core(version).to_lowercase();
    !(v.is_empty() || v == "latest" || v == "unknown" || v.starts_with(['<', '>']) || v.contains('…'))
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// As text once the leading zeros are gone, so a number longer than an integer still sorts.
fn cmp_digits(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn cmp_part(l: &str, r: &str) -> Ordering {
    let (ld, rd) = (digits(l), digits(r));
    if ld && rd {
        return cmp_digits(l, r);
    }
    // A missing part counts as zero: "1.2" is "1.2.0" and loses to "1.2.1".
    if l.is_empty() {
        return if rd && r.trim_start_matches('0').is_empty() { Equal } else { Less };
    }
    if r.is_empty() {
        return if ld && l.trim_start_matches('0').is_empty() { Equal } else { Greater };
    }
    // "9a" against "10": the number in front decides first; with the numbers
    // tied, the bare part is the release and the suffixed one came before it.
    let num = |s: &str| s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let (ln, rn) = (&l[..num(l)], &r[..num(r)]);
    if ln.is_empty() != rn.is_empty() {
        return if ln.is_empty() { Less } else { Greater };
    }
    if !ln.is_empty() {
        let n = cmp_digits(ln, rn);
        if n != Equal {
            return n;
        }
        if (ln.len() == l.len()) != (rn.len() == r.len()) {
            return if ln.len() == l.len() { Greater } else { Less };
        }
    }
    l.to_lowercase().cmp(&r.to_lowercase())
}

/// Numeric-aware: each part between dots is compared as a number when both sides are digits.
pub fn compare(a: &str, b: &str) -> Ordering {
    let parts = |v: &str| v.split(|c: char| !c.is_alphanumeric()).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (l, r) = (parts(a), parts(b));
    let at = |v: &Vec<String>, i: usize| v.get(i).cloned().unwrap_or_default();
    (0..l.len().max(r.len())).map(|i| cmp_part(&at(&l, i), &at(&r, i))).find(|o| *o != Equal).unwrap_or(Equal)
}

pub fn is_newer(candidate: &str, installed: &str) -> bool {
    compare(version_core(candidate), version_core(installed)) == Greater
}

/// `major.minor.patch` of a release tag and whether it is a pre-release.
fn release(raw: &str) -> Option<([u64; 3], bool)> {
    let v = raw.trim().trim_start_matches(['v', 'V']).split('+').next()?;
    let (core, pre) = v.split_once('-').map_or((v, false), |(c, _)| (c, true));
    let mut n = [0u64; 3];
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for (i, p) in parts.iter().enumerate() {
        n[i] = p.parse().ok()?;
    }
    Some((n, pre))
}

/// Whether the published release is one to offer over the running one. Stable releases only.
pub fn release_is_newer(published: &str, running: &str) -> bool {
    match (release(published), release(running)) {
        (Some((_, true)), _) | (None, _) => false,
        (Some(_), None) => true,
        (Some((p, _)), Some((r, pre))) => p > r || (p == r && pre),
    }
}

// ── Updates and the rules that hide them ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Update {
    /// The winget package id; also what a rule names.
    pub id: String,
    pub name: String,
    pub version: String,
    pub available: String,
    /// "winget" or "msstore".
    pub source: String,
}

/// winget codes that mean "nothing to show", not "it failed".
const NOTHING: [i32; 2] = [0x8A15_0014_u32 as i32, 0x8A15_002B_u32 as i32];

/// The last lines a program said, for an error message.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.split(['\r', '\n']).map(str::trim).filter(|l| !l.is_empty() && !is_bar(l)).collect();
    lines[lines.len().saturating_sub(3)..].join(" ")
}

/// What `winget upgrade` listed. Err when the answer cannot be trusted: a list
/// that failed is "could not check", never "no updates".
pub fn parse_updates(code: Option<i32>, text: &str) -> Result<Vec<Update>, String> {
    let Some(rows) = winget::table(text) else {
        return if code.is_some_and(|c| c == 0 || NOTHING.contains(&c)) { Ok(vec![]) } else { Err(tail(text)) };
    };
    let mut out: Vec<Update> = rows
        .into_iter()
        .filter(|r| r.len() >= 4)
        .map(|r| Update {
            source: if r.len() >= 5 { r[r.len() - 1].to_lowercase() } else { "winget".into() },
            id: r[1].clone(),
            name: r[0].clone(),
            version: r[2].clone(),
            available: r[3].clone(),
        })
        .filter(|u| winget::valid_id(&u.id) && comparable(&u.version) && comparable(&u.available) && is_newer(&u.available, &u.version))
        .collect();
    out.sort_by_cached_key(|u| (u.source != "winget", u.name.to_lowercase()));
    out.dedup_by(|b, a| a.id == b.id);
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub version: String,
}

/// What `winget search` found, in winget's own order (best match first).
pub fn parse_search(code: Option<i32>, text: &str) -> Result<Vec<Found>, String> {
    let Some(rows) = winget::table(text) else {
        return if code.is_some_and(|c| c == 0 || NOTHING.contains(&c)) { Ok(vec![]) } else { Err(tail(text)) };
    };
    Ok(rows.into_iter().filter(|r| winget::valid_id(&r[1])).map(|r| Found { name: r[0].clone(), id: r[1].clone(), version: r[2].clone() }).collect())
}

/// What is typed in the search field, made safe to stand after `--query`: one
/// argument, never an option, no control characters.
pub fn clean_query(raw: &str) -> String {
    let q: String = raw.chars().filter(|c| !c.is_control()).take(80).collect();
    q.trim().trim_start_matches(['-', '/']).trim().to_string()
}

/// One choice per app: skip exactly this release (`version`), or stop checking it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Rule {
    pub fn hides(&self, update: &Update) -> bool {
        self.id.eq_ignore_ascii_case(&update.id) && self.version.as_deref().is_none_or(|v| version_core(v) == version_core(&update.available))
    }
}

/// The stored rules, without what cannot be one: an id winget would not write, a second rule for one app.
pub fn rules(stored: &Value) -> Vec<Rule> {
    let mut out: Vec<Rule> = Vec::new();
    for rule in serde_json::from_value::<Vec<Rule>>(stored.clone()).unwrap_or_default() {
        if winget::valid_id(&rule.id) && rule.version.as_deref().is_none_or(comparable) && !out.iter().any(|r| r.id.eq_ignore_ascii_case(&rule.id)) {
            out.push(rule);
        }
    }
    out
}

pub fn visible(updates: &[Update], rules: &[Rule]) -> Vec<Update> {
    updates.iter().filter(|u| !rules.iter().any(|r| r.hides(u))).cloned().collect()
}

// ── The schedule ──────────────────────────────────────────────────────────────

/// A check that came due while the PC was off or asleep runs this long after
/// the app is up, so starting the app never waits on winget.
pub const CATCH_UP: u64 = 180;

/// When the next automatic check is due, in seconds since 1970; None when the schedule is off.
pub fn next_check(last: Option<u64>, frequency: &str, now: u64) -> Option<u64> {
    let every = match frequency {
        "daily" => 24 * 3600,
        "weekly" => 7 * 24 * 3600,
        _ => return None,
    };
    Some(match last {
        Some(last) if last + every > now => last + every,
        _ => now + CATCH_UP,
    })
}

// ── Progress ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Preparing,
    Downloading,
    Verifying,
    Installing,
    Done,
}

/// Where an install stands, read from what winget prints as it goes.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Progress {
    pub phase: Phase,
    /// 0…1 of the current phase, when winget says.
    pub fraction: Option<f64>,
    /// The last thing it said in words.
    pub line: String,
    /// The line the last piece ended in the middle of.
    #[serde(skip)]
    rest: String,
}

/// A line that is only a bar or a spinner frame.
fn is_bar(line: &str) -> bool {
    line.chars().any(|c| ('\u{2580}'..='\u{259F}').contains(&c)) || matches!(line, "-" | "\\" | "|" | "/")
}

/// "45%" or "1.2 MB / 4.5 MB" anywhere in the line.
fn fraction(line: &str) -> Option<f64> {
    let words: Vec<&str> = line.split_whitespace().collect();
    if let Some(p) = words.iter().rev().find_map(|w| w.strip_suffix('%')?.parse::<f64>().ok()) {
        return Some((p / 100.0).clamp(0.0, 1.0));
    }
    let bytes = |n: &str, unit: &str| {
        let scale = match unit.to_ascii_uppercase().as_str() {
            "B" => 1.0,
            "KB" => 1024.0,
            "MB" => 1024.0 * 1024.0,
            "GB" => 1024.0 * 1024.0 * 1024.0,
            _ => return None,
        };
        Some(n.parse::<f64>().ok()? * scale)
    };
    let slash = words.iter().rposition(|w| *w == "/")?;
    let (done, all) = (bytes(words.get(slash.checked_sub(2)?)?, words[slash - 1])?, bytes(words.get(slash + 1)?, words.get(slash + 2)?)?);
    (all > 0.0).then(|| (done / all).clamp(0.0, 1.0))
}

impl Progress {
    /// Takes the next piece of output. Bars are redrawn in place, so the piece
    /// is read frame by frame; the words are winget's English ones, and in
    /// another language the phase stays put while the numbers still move.
    pub fn feed(&mut self, text: &str) {
        // A piece can end in the middle of a line: that end is read now, and again once it is whole.
        let text = std::mem::take(&mut self.rest) + text;
        let whole = text.rfind(['\r', '\n']).map_or(0, |i| i + 1);
        if text.len() - whole <= 4096 {
            self.rest = text[whole..].to_string();
        }
        let (done, end) = text.split_at(whole);
        for (line, complete) in done.split(['\r', '\n']).map(|l| (l.trim(), true)).chain([(end.trim(), false)]) {
            if line.is_empty() {
                continue;
            }
            let low = line.to_lowercase();
            let phase = if low.contains("successfully installed") {
                Some(Phase::Done)
            } else if low.contains("downloading") {
                Some(Phase::Downloading)
            } else if low.contains("verified") || low.contains("verifying") {
                Some(Phase::Verifying)
            } else if low.contains("starting package") || low.contains("installing") {
                Some(Phase::Installing)
            } else {
                None
            };
            if let Some(phase) = phase.filter(|p| *p != self.phase) {
                self.phase = phase;
                self.fraction = None;
            }
            if let Some(f) = fraction(line) {
                self.fraction = Some(f);
            }
            if complete && !is_bar(line) {
                self.line = line.chars().take(160).collect();
            }
        }
    }
}
