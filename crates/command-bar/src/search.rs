// Matching, ranking and what the bar learns from use. Pure text work: no window, no file.
//
// Ported from reference/vorssaint-utils/Sources/Vorssaint/Services/CommandBar/CommandBarSupport.swift
// (`CommandBarSearch`, `CommandBarUsage`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use windows::Win32::Globalization::{FoldStringW, MAP_COMPOSITE, MAP_FOLDCZONE};

/// Accents, as separate characters once the text is taken apart.
fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// What takes no room and nobody can type: soft hyphens, direction marks, joiners.
fn is_invisible(c: char) -> bool {
    matches!(c as u32, 0x00AD | 0x061C | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0xFEFF)
}

/// Case, accents and width never matter, and any run of white space is one
/// space. No locale is involved: a Turkish one would lower a capital I to a
/// dotless one and stop "insta" from finding a title that begins with that letter.
pub fn fold(value: &str) -> String {
    let lowered = if value.is_ascii() {
        value.to_ascii_lowercase()
    } else {
        // Full-width forms to plain ones and each accent split from its letter (Unicode form KD).
        let wide: Vec<u16> = value.encode_utf16().collect();
        let mut out = vec![0u16; wide.len() * 4 + 8];
        let len = unsafe { FoldStringW(MAP_COMPOSITE | MAP_FOLDCZONE, &wide, Some(&mut out)) };
        let apart = if len > 0 { String::from_utf16_lossy(&out[..len as usize]) } else { value.to_string() };
        apart.chars().flat_map(char::to_lowercase).filter(|c| !is_mark(*c) && !is_invisible(*c)).collect()
    };
    lowered.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn numeric(token: &str) -> bool {
    token.chars().all(|c| c.is_numeric())
}

fn is_subsequence(needle: &str, word: &str) -> bool {
    let mut rest = word.chars();
    needle.chars().all(|c| rest.any(|w| w == c))
}

/// One neighbouring pair typed in reverse order.
fn is_transposition(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len() != b.len() {
        return false;
    }
    let differ: Vec<usize> = (0..a.len()).filter(|i| a[*i] != b[*i]).collect();
    matches!(differ[..], [i, j] if j == i + 1 && a[i] == b[j] && a[j] == b[i])
}

/// At most one substitution, insertion, deletion or neighbouring swap apart.
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edited) = (0, 0, false);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        if edited {
            return false;
        }
        edited = true;
        if a.len() == b.len() {
            let swapped = i + 1 < a.len() && a[i] == b[j + 1] && a[i + 1] == b[j];
            i += if swapped { 2 } else { 1 };
            j += if swapped { 2 } else { 1 };
        } else if a.len() > b.len() {
            i += 1;
        } else {
            j += 1;
        }
    }
    true
}

/// Whole word 140, start of a word 80, inside a word 44; then what only rescues
/// a typing mistake, which needs some length to bite on and never applies to digits.
fn token_score(token: &str, words: &[&str], haystack: &str) -> Option<i32> {
    if words.contains(&token) {
        return Some(140);
    }
    if words.iter().any(|w| w.starts_with(token)) {
        return Some(80);
    }
    if haystack.contains(token) {
        return Some(44);
    }
    let len = token.chars().count();
    if len < 3 || numeric(token) {
        return None;
    }
    if words.iter().any(|w| is_subsequence(token, w)) {
        return Some(24);
    }
    if words.iter().any(|w| is_transposition(token, w)) || (len >= 4 && words.iter().any(|w| within_one_edit(token, w))) {
        return Some(16);
    }
    None
}

/// None when the query does not match. Every word of the query has to be found;
/// a hit of the whole query on the title then weighs more than any single word.
/// All three texts are already folded.
pub fn score(title: &str, keywords: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return None;
    }
    let haystack = if keywords.is_empty() { title.to_string() } else { format!("{title} {keywords}") };
    let words: Vec<&str> = haystack.split(' ').collect();
    let mut total = 0;
    for token in query.split(' ') {
        total += token_score(token, &words, &haystack)?;
    }
    total += match tier(title, keywords, query) {
        5 => 1200,
        4 => 900,
        3 => 700,
        2 => 350,
        _ => 0,
    };
    Some(total)
}

/// How well the whole query sits in the row: exactly its title, the start of it,
/// inside it, inside its keywords, or found word by word only.
fn tier(title: &str, keywords: &str, query: &str) -> i32 {
    if title == query {
        5
    } else if title.starts_with(query) {
        4
    } else if title.contains(query) {
        3
    } else if !keywords.is_empty() && keywords.contains(query) {
        2
    } else {
        1
    }
}

/// One row offered to the ranking.
pub struct Candidate<'a> {
    pub title: &'a str,
    /// Extra words the row is found by.
    pub keywords: &'a str,
    /// A deliberate preference (a pinned row): it leads whatever the text scores.
    pub priority: i32,
    /// Passive signals: how often it is used, where it comes from, whether it runs.
    pub boost: i32,
}

/// The candidates that match, best first, by their position in `candidates`.
/// Preference, then how well the query sits in the row, then score and boost;
/// equal rows keep the order they came in.
pub fn rank(candidates: &[Candidate], query: &str) -> Vec<usize> {
    let query = fold(query);
    let mut found: Vec<(usize, i32, i32, i32)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(at, c)| {
            let (title, keywords) = (fold(c.title), fold(c.keywords));
            let points = score(&title, &keywords, &query)?;
            Some((at, c.priority, tier(&title, &keywords, &query), points + c.boost))
        })
        .collect();
    found.sort_by(|a, b| (b.1, b.2, b.3).cmp(&(a.1, a.2, a.3)).then(a.0.cmp(&b.0)));
    found.into_iter().map(|f| f.0).collect()
}

// ── What is used most ─────────────────────────────────────────────────────────

/// How often and how recently one row ran.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Use {
    pub count: u32,
    /// Seconds since 1970.
    pub last: f64,
}

/// What the bar keeps between runs: counts by row id and the pinned rows. The
/// typed text is never part of it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Learned {
    #[serde(default)]
    pub usage: HashMap<String, Use>,
    #[serde(default)]
    pub pins: Vec<String>,
}

const KEPT: usize = 200;

impl Learned {
    /// One more run of `id`. The rows used longest ago fall off past the cap.
    pub fn record(&mut self, id: &str, now: f64) {
        let entry = self.usage.entry(id.to_string()).or_default();
        entry.count = (entry.count + 1).min(999);
        entry.last = now;
        while self.usage.len() > KEPT {
            let Some(oldest) = self.usage.iter().min_by(|a, b| a.1.last.total_cmp(&b.1.last)).map(|(id, _)| id.clone()) else { break };
            self.usage.remove(&oldest);
        }
    }

    /// What habit adds to a row's score: well under a literal hit, so what was
    /// typed always beats what is usually run.
    pub fn boost(&self, id: &str, now: f64) -> i32 {
        let Some(used) = self.usage.get(id).filter(|u| u.count > 0) else { return 0 };
        let age = (now - used.last).max(0.0);
        let weight = if age < 2.0 * 3600.0 {
            12
        } else if age < 48.0 * 3600.0 {
            8
        } else if age < 14.0 * 86400.0 {
            5
        } else {
            2
        };
        used.count.min(40) as i32 * weight
    }

    /// The rows of `available` that were used, most used first, then most recent.
    pub fn most_used<'a>(&self, available: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
        let mut used: Vec<(&str, &Use)> = available.filter_map(|id| self.usage.get(id).filter(|u| u.count > 0).map(|u| (id, u))).collect();
        used.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(b.1.last.total_cmp(&a.1.last)));
        used.into_iter().map(|(id, _)| id).collect()
    }
}
