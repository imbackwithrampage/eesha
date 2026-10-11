// Watch: a window is read now and then (its picture, then the text in it) and
// the island shows a headline from it: a percentage, a clock, a number with its
// unit or the first words. A condition ends the watch with a notice.
//
// The thread exists only while a watch does. A picture that has not changed
// (a 16×16 grey fingerprint) is not read again for ten seconds.
//
// debt: a window only; a region of a window or of a display needs a selection overlay.
// debt: no "settles" condition; text is matched case-insensitively, accents as they are.
//
// Ported from NotchWatchSupport.swift (tracker, headline, number parsing).

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use svc_capture::Image;

// ── Reading a headline ────────────────────────────────────────────────────────

fn is_num(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == ','
}

/// The digits (with separators) that end at `end` (exclusive) in `chars`.
fn number_before(chars: &[char], end: usize) -> Option<String> {
    let mut at = end;
    while at > 0 && chars[at - 1] == ' ' {
        at -= 1;
    }
    let to = at;
    while at > 0 && is_num(chars[at - 1]) {
        at -= 1;
    }
    let text: String = chars[at..to].iter().collect();
    let text = text.trim_matches(|c| c == '.' || c == ',').to_string();
    text.chars().any(|c| c.is_ascii_digit()).then_some(text)
}

fn digits_at(chars: &[char], from: usize) -> usize {
    chars[from..].iter().take_while(|c| c.is_ascii_digit()).count()
}

/// What to show for the text that was read: a percentage first, else a clock,
/// else a number with its unit, else the first line cut to 22 characters.
pub fn headline(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if let Some(at) = chars.iter().position(|c| *c == '%') {
        if let Some(n) = number_before(&chars, at) {
            return Some(format!("{n}%"));
        }
    }
    for i in 1..chars.len().saturating_sub(1) {
        if chars[i] == ':' && chars[i - 1].is_ascii_digit() && digits_at(&chars, i + 1) == 2 {
            let mut from = i;
            while from > 0 && chars[from - 1].is_ascii_digit() && i - from < 2 {
                from -= 1;
            }
            let mut to = i + 3;
            if chars.get(to) == Some(&':') && digits_at(&chars, to + 1) == 2 {
                to += 3;
            }
            return Some(chars[from..to].iter().collect());
        }
    }
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut end = i;
            while end < chars.len() && is_num(chars[end]) {
                end += 1;
            }
            let num: String = chars[i..end].iter().collect::<String>().trim_end_matches(|c| c == '.' || c == ',').to_string();
            let mut unit_from = end;
            if chars.get(unit_from) == Some(&' ') {
                unit_from += 1;
            }
            let unit: String = chars[unit_from..].iter().take_while(|c| c.is_alphabetic() || **c == '/').take(4).collect();
            return Some(if unit.is_empty() { num } else { format!("{num} {unit}") });
        }
        i += 1;
    }
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(22).collect())
}

/// The number in a headline, either separator style: "1,234.5", "1.234,5", "12,5".
pub fn parse_number(text: &str) -> Option<f64> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let raw: String = text[start..].chars().take_while(|c| is_num(*c)).collect();
    let raw = raw.trim_end_matches(|c| c == '.' || c == ',');
    let (last_dot, last_comma) = (raw.rfind('.'), raw.rfind(','));
    let decimal = match (last_dot, last_comma) {
        (Some(d), Some(c)) => Some(if d > c { '.' } else { ',' }),
        (Some(p), None) | (None, Some(p)) => {
            let sep = raw.as_bytes()[p] as char;
            let groups = raw.matches(sep).count();
            // "1,234" and "1.234.567" are thousands; "12,5" and "3.14" are decimals.
            if groups == 1 && raw.len() - p - 1 != 3 { Some(sep) } else { None }
        }
        _ => None,
    };
    let clean: String = raw
        .chars()
        .filter_map(|c| match c {
            c if c.is_ascii_digit() => Some(c),
            c if Some(c) == decimal => Some('.'),
            _ => None,
        })
        .collect();
    clean.parse().ok()
}

// ── The condition ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Cond {
    /// The headline differs from the first one for three seconds.
    Changes,
    /// The text has these words.
    Contains(String),
    /// The number reaches this, from either side.
    Reaches(f64),
}

pub struct Tracker {
    cond: Cond,
    base: Option<String>,
    /// Reaches: +1 when the number began below the goal, -1 above.
    from_below: Option<bool>,
    since: Option<(String, u64)>,
}

impl Tracker {
    pub fn new(cond: Cond) -> Self {
        Tracker { cond, base: None, from_below: None, since: None }
    }

    /// One reading: the headline, all the text, and a clock in milliseconds. True when the condition holds.
    pub fn feed(&mut self, headline: &str, text: &str, now_ms: u64) -> bool {
        match &self.cond {
            Cond::Contains(words) => text.to_lowercase().contains(&words.to_lowercase()),
            Cond::Reaches(goal) => {
                let Some(n) = parse_number(headline) else { return false };
                let below = *self.from_below.get_or_insert(n < *goal);
                if below { n >= *goal } else { n <= *goal }
            }
            Cond::Changes => {
                let base = self.base.get_or_insert_with(|| headline.to_string());
                if headline == base {
                    self.since = None;
                    return false;
                }
                match &self.since {
                    Some((h, at)) if h == headline => now_ms.saturating_sub(*at) >= 3000,
                    _ => {
                        self.since = Some((headline.to_string(), now_ms));
                        false
                    }
                }
            }
        }
    }
}

// ── The picture ───────────────────────────────────────────────────────────────

const GRID: u32 = 16;

/// 16×16 grey samples.
pub fn fingerprint(image: &Image) -> Vec<u8> {
    let mut out = Vec::with_capacity((GRID * GRID) as usize);
    for gy in 0..GRID {
        for gx in 0..GRID {
            let p = image.pixel(gx * image.w / GRID, gy * image.h / GRID).unwrap_or([0; 3]);
            out.push(((p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10) as u8);
        }
    }
    out
}

fn same_picture(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u32).sum::<u32>() <= 2 * a.len() as u32
}

/// The text recogniser takes up to ~2600 px: a larger picture is thinned out.
fn fit(image: Image) -> Image {
    const MAX: u32 = 2400;
    let long = image.w.max(image.h);
    if long <= MAX {
        return image;
    }
    let k = long.div_ceil(MAX);
    let (w, h) = (image.w / k, image.h / k);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let at = (((y * k) * image.w + x * k) * 4) as usize;
            rgba.extend_from_slice(&image.rgba[at..at + 4]);
        }
    }
    Image { w, h, rgba }
}

/// The text in the picture, one line per line.
pub fn read_text(image: &Image) -> Result<String, String> {
    let lines = svc_capture::ocr(&fit(image.clone()))?;
    Ok(lines.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n"))
}

// ── The watch ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct Reading {
    pub headline: Option<String>,
    pub matched: bool,
    pub error: Option<String>,
}

pub struct Running {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    pub fn stop(mut self) {
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
}

/// Reads window `hwnd` every `every` until the condition holds or it is stopped.
pub fn start(hwnd: isize, cond: Cond, every: Duration, on_reading: impl Fn(&Reading) + Send + 'static) -> Running {
    let (stop, rx) = mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        let began = Instant::now();
        let mut tracker = Tracker::new(cond);
        let mut last: Option<(Vec<u8>, Instant, String, Option<String>)> = None;
        loop {
            let reading = (|| -> Result<Reading, String> {
                let image = svc_capture::grab_window(hwnd)?;
                let print = fingerprint(&image);
                let (text, head) = match &last {
                    Some((p, at, text, head)) if same_picture(p, &print) && at.elapsed() < Duration::from_secs(10) => {
                        (text.clone(), head.clone())
                    }
                    _ => {
                        let text = read_text(&image)?;
                        let head = headline(&text);
                        last = Some((print, Instant::now(), text.clone(), head.clone()));
                        (text, head)
                    }
                };
                let matched = tracker.feed(head.as_deref().unwrap_or(""), &text, began.elapsed().as_millis() as u64);
                Ok(Reading { headline: head, matched, error: None })
            })()
            .unwrap_or_else(|e| Reading { headline: None, matched: false, error: Some(e) });
            let done = reading.matched || reading.error.is_some();
            on_reading(&reading);
            if done {
                break;
            }
            // A message or a closed channel is the end; a timeout is the next look.
            if !matches!(rx.recv_timeout(every), Err(mpsc::RecvTimeoutError::Timeout)) {
                break;
            }
        }
    });
    Running { stop: Some(stop), thread: Some(thread) }
}
