// Scrolling capture: how far a page moved between two grabs of the same region,
// and the long picture made of the rows that were new each time.
//
// Ported from reference/vorssaint-utils/Sources/Vorssaint/Services/QuickTools/ScreenshotSupport.swift
// (scrollingTransition, scrollingFixedBottomRows, scrollingNewContentRows,
// scrollingPixelRange and their helpers) and
// …/Services/QuickTools/ScreenshotScrollingCapture.swift (the sample and the
// loop that keeps the strips).
//
// Everything here is pure: grabs come in, a picture goes out. The session that
// takes the grabs is in lib.rs.

use std::ops::Range;

use svc_capture::{Image, Rect};

/// The whole picture is never larger than this…
pub const MAX_PIXELS: usize = 60_000_000;
/// …nor taller: a canvas of the editor is at most 32 767 pixels high.
pub const MAX_ROWS: usize = 32_000;
pub const MAX_STRIPS: usize = 512;

/// A grab in grey, at most 32 columns wide. Rows stay one for one with the
/// picture: a scroll is a whole number of rows, and has to stay one.
pub struct Sample {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

pub fn sample(image: &Image) -> Sample {
    let (iw, h) = (image.w as usize, image.h as usize);
    let w = iw.min(32);
    let mut px = vec![0u8; w * h];
    for y in 0..h {
        let row = &image.rgba[y * iw * 4..(y + 1) * iw * 4];
        for c in 0..w {
            let (from, to) = (c * iw / w, (c + 1) * iw / w);
            let sum: u32 = row[from * 4..to * 4].chunks_exact(4).map(|p| (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000).sum();
            px[y * w + c] = (sum / (to - from) as u32) as u8;
        }
    }
    Sample { w, h, px }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    /// Nothing moved.
    End,
    /// The page moved: `overlap` rows are in both grabs, in `columns` of the sample.
    Advanced { overlap: usize, forward: bool, columns: Range<usize> },
    Unmatched,
}

/// The mean difference of two samples in `columns`, away from the top and bottom edges.
fn difference(a: &Sample, b: &Sample, columns: &Range<usize>) -> f64 {
    let inset = a.h / 24;
    let (mut sum, mut count) = (0u64, 0u64);
    for row in inset..a.h - inset {
        let at = row * a.w;
        for c in columns.clone() {
            sum += a.px[at + c].abs_diff(b.px[at + c]) as u64;
        }
        count += columns.len() as u64;
    }
    if count > 0 { sum as f64 / count as f64 } else { f64::INFINITY }
}

pub fn stable(a: &Sample, b: &Sample, columns: Option<&Range<usize>>) -> bool {
    a.w == b.w && a.h == b.h && a.w > 0 && a.h > 0 && difference(a, b, columns.unwrap_or(&(0..a.w))) <= 1.5
}

/// The columns in groups of an eighth of the sample: a sidebar or a scrollbar
/// that stays put is found by its group not moving.
fn tiles(columns: &Range<usize>, sample_w: usize) -> Vec<Range<usize>> {
    let tile = (sample_w / 8).max(2);
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut lower = columns.start;
    while lower < columns.end {
        let upper = columns.end.min(lower + tile);
        if upper - lower >= 2 || out.is_empty() {
            out.push(lower..upper);
        }
        lower = upper;
    }
    out
}

struct RowMatch {
    longest: usize,
    matching: usize,
    compared: usize,
    difference: f64,
}

/// `prefix[r * (w + 1) + c]` is the summed difference of compared row `r` in the columns before `c`.
fn row_match(prefix: &[u32], w: usize, rows: usize, columns: &Range<usize>) -> RowMatch {
    let threshold = (columns.len() * 8) as u32;
    let (mut longest, mut run, mut matching, mut total) = (0, 0, 0, 0u64);
    for r in 0..rows {
        let d = prefix[r * (w + 1) + columns.end] - prefix[r * (w + 1) + columns.start];
        if d <= threshold {
            run += 1;
            matching += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
        total += d as u64;
    }
    RowMatch { longest, matching, compared: rows, difference: total as f64 / (rows * columns.len()) as f64 }
}

struct Match {
    advance: usize,
    reversed: bool,
    columns: Range<usize>,
    supporting: usize,
    longest: usize,
    matching: usize,
    difference: f64,
}

/// The widest run of neighbouring tiles that all agree the page moved by this much.
fn candidate(moving: &[Range<usize>], prefix: &[u32], w: usize, rows: usize, required: usize) -> Option<(Range<usize>, usize, RowMatch)> {
    let good = |m: &RowMatch| m.longest >= required && m.matching >= required.max(m.compared / 3);
    let minimum = if moving.len() >= 3 { 2 } else { 1 };
    // Runs of tiles that match, each forgiving one tile that does not.
    let mut runs: Vec<Vec<&Range<usize>>> = Vec::new();
    let mut run: Vec<&Range<usize>> = Vec::new();
    let mut skipped = false;
    for tile in moving {
        if good(&row_match(prefix, w, rows, tile)) {
            run.push(tile);
        } else if !run.is_empty() && !skipped {
            skipped = true;
        } else {
            if !run.is_empty() {
                runs.push(std::mem::take(&mut run));
            }
            skipped = false;
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs.into_iter()
        .filter(|run| run.len() >= minimum)
        .filter_map(|run| {
            let columns = run[0].start..run[run.len() - 1].end;
            let all = row_match(prefix, w, rows, &columns);
            good(&all).then_some((columns, run.len(), all))
        })
        .max_by(|a, b| {
            (a.0.len(), a.2.matching).cmp(&(b.0.len(), b.2.matching)).then(b.2.difference.total_cmp(&a.2.difference))
        })
}

/// How many rows two successive grabs share, and in which columns the page moves.
pub fn transition(previous: &Sample, current: &Sample, content: Option<&Range<usize>>) -> Transition {
    let (w, h) = (previous.w, previous.h);
    if w == 0 || current.w != w || current.h != h || h < 24 {
        return Transition::Unmatched;
    }
    let columns = content.cloned().unwrap_or(0..w);
    if columns.is_empty() || columns.end > w {
        return Transition::Unmatched;
    }
    if stable(previous, current, Some(&columns)) {
        return Transition::End;
    }
    // The last scroll at the bottom of a page is often only a few rows.
    let least = ((h as f64 * 0.01).round() as usize).max(2);
    let most = (h - 8).min((h as f64 * 0.88).round() as usize);
    let moving: Vec<Range<usize>> = tiles(&columns, w).into_iter().filter(|tile| difference(previous, current, tile) > 1.5).collect();
    if least > most || moving.is_empty() {
        return Transition::Unmatched;
    }
    let required = (h / 12).clamp(8, 28);
    let edge = (h / 10).max(2);
    let mut prefix: Vec<u32> = Vec::new();
    let mut matches: Vec<Match> = Vec::new();
    for advance in least..=most {
        // Compared rows are `edge` away from both ends of what the two grabs share.
        let Some(rows) = (h - advance).checked_sub(2 * edge).filter(|rows| *rows > 0) else { break };
        for reversed in [false, true] {
            prefix.clear();
            prefix.resize(rows * (w + 1), 0);
            for r in 0..rows {
                let row = edge + r;
                let (a, b) = if reversed { (row * w, (row + advance) * w) } else { ((row + advance) * w, row * w) };
                let (old, new) = (&previous.px[a..a + w], &current.px[b..b + w]);
                let out = &mut prefix[r * (w + 1)..(r + 1) * (w + 1)];
                for c in 0..w {
                    out[c + 1] = out[c] + old[c].abs_diff(new[c]) as u32;
                }
            }
            if let Some((columns, supporting, m)) = candidate(&moving, &prefix, w, rows, required) {
                matches.push(Match { advance, reversed, columns, supporting, longest: m.longest, matching: m.matching, difference: m.difference });
            }
        }
    }
    matches.sort_by(|a, b| {
        (b.columns.len(), b.supporting, b.longest, b.matching)
            .cmp(&(a.columns.len(), a.supporting, a.longest, a.matching))
            .then(a.difference.total_cmp(&b.difference))
    });
    let Some(best) = matches.first().filter(|best| best.longest >= required) else { return Transition::Unmatched };
    // Repeated blank bands look as good at several offsets: a match has to be
    // the only one, or a seam would be guessed.
    let rival = matches[1..].iter().find(|m| m.reversed != best.reversed || m.advance.abs_diff(best.advance) > 2);
    if let Some(rival) = rival {
        if rival.columns.len() + 1 >= best.columns.len()
            && rival.supporting + 1 >= best.supporting
            && rival.longest + 2 >= best.longest
            && rival.matching + (best.supporting * 3).max(3) >= best.matching
            && rival.difference <= best.difference + 0.75
        {
            return Transition::Unmatched;
        }
    }
    Transition::Advanced { overlap: h - best.advance, forward: !best.reversed, columns: best.columns.clone() }
}

/// A footer that stays at the bottom of the region while the page moves behind
/// it: kept in every strip it would repeat all the way down the picture.
pub fn fixed_bottom_rows(previous: &Sample, current: &Sample, overlap: usize, columns: &Range<usize>) -> usize {
    let (w, h) = (previous.w, previous.h);
    if current.w != w || current.h != h || overlap == 0 || overlap >= h || columns.is_empty() || columns.end > w {
        return 0;
    }
    let same = |row: &usize| {
        let at = row * w;
        let sum: u32 = columns.clone().map(|c| previous.px[at + c].abs_diff(current.px[at + c]) as u32).sum();
        sum as f64 / columns.len() as f64 <= 2.0
    };
    let rows = (0..h).rev().take_while(same).count();
    if rows >= (h / 100).clamp(4, 12) && rows < overlap { rows } else { 0 }
}

/// The rows of a grab that a forward scroll revealed, above the fixed footer.
pub fn new_rows(height: usize, overlap: usize, fixed: usize) -> Option<Range<usize>> {
    (height > 0 && overlap > 0 && overlap < height && fixed < overlap).then(|| overlap - fixed..height - fixed)
}

/// Columns of the sample as columns of the picture.
pub fn pixel_range(columns: &Range<usize>, sample_w: usize, image_w: usize) -> Option<Range<usize>> {
    if sample_w == 0 || image_w == 0 || columns.is_empty() || columns.end > sample_w {
        return None;
    }
    let (lower, upper) = (columns.start * image_w / sample_w, (columns.end * image_w + sample_w - 1) / sample_w);
    (upper <= image_w && lower < upper).then_some(lower..upper)
}

fn strip(image: &Image, columns: &Range<usize>, rows: Range<usize>) -> Image {
    image.crop(Rect { x: columns.start as i32, y: rows.start as i32, w: columns.len() as i32, h: rows.len() as i32 })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    /// The page changed in a way that could not be followed: what was complete is kept.
    Partial,
    /// A limit was reached.
    Limited,
}

/// Takes the grabs of one region in order and keeps the rows that are new.
pub struct Stitcher {
    /// The first grab, whole, until the columns that move are known.
    first: Option<Image>,
    strips: Vec<Image>,
    footer: Option<Image>,
    /// The last grab that added rows.
    previous: Sample,
    /// The columns that move: of the sample, and of the picture.
    columns: Option<(Range<usize>, Range<usize>)>,
    fixed: usize,
    rows: usize,
    unmatched: bool,
    limited: bool,
}

impl Stitcher {
    pub fn new(first: Image) -> Self {
        Self {
            previous: sample(&first),
            rows: first.h as usize,
            first: Some(first),
            strips: Vec::new(),
            footer: None,
            columns: None,
            fixed: 0,
            unmatched: false,
            limited: false,
        }
    }

    /// How tall the picture is so far.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The session ran out of time.
    pub fn limit(&mut self) {
        self.limited = true;
    }

    /// The next grab. False when a limit was reached: there is no point in more.
    pub fn feed(&mut self, frame: &Image) -> bool {
        let current = sample(frame);
        let h = frame.h as usize;
        match transition(&self.previous, &current, self.columns.as_ref().map(|c| &c.0)) {
            Transition::Advanced { overlap, forward: true, columns } => {
                let found = self.columns.clone().or_else(|| Some((columns.clone(), pixel_range(&columns, current.w, frame.w as usize)?)));
                let Some((_, pixels)) = found.clone() else {
                    self.unmatched = true;
                    return true;
                };
                let fixed = if self.columns.is_some() { self.fixed } else { fixed_bottom_rows(&self.previous, &current, overlap, &columns) };
                let Some(new) = new_rows(h, overlap, fixed) else {
                    self.unmatched = true;
                    return true;
                };
                let rows = self.rows + new.len();
                if rows > MAX_ROWS || rows * pixels.len() > MAX_PIXELS || self.strips.len() >= MAX_STRIPS {
                    self.limited = true;
                    return false;
                }
                // The first time: the first grab is cut to the same columns, without the footer.
                if let Some(first) = self.first.take() {
                    self.strips.push(strip(&first, &pixels, 0..h - fixed));
                    (self.columns, self.fixed) = (found, fixed);
                }
                if fixed > 0 {
                    self.footer = Some(strip(frame, &pixels, h - fixed..h));
                }
                self.strips.push(strip(frame, &pixels, new));
                self.rows = rows;
                self.previous = current;
                self.unmatched = false;
            }
            // Scrolling back adds nothing; a later forward scroll goes on from the furthest grab.
            Transition::End | Transition::Advanced { .. } => self.unmatched = false,
            Transition::Unmatched => self.unmatched = true,
        }
        true
    }

    pub fn finish(mut self) -> (Image, Outcome) {
        let mut parts = match self.first.take() {
            Some(first) => vec![first],
            None => self.strips,
        };
        parts.extend(self.footer);
        let w = parts[0].w;
        let mut rgba = Vec::with_capacity(parts.iter().map(|p| p.rgba.len()).sum());
        for part in &parts {
            rgba.extend_from_slice(&part.rgba);
        }
        let h = (rgba.len() / 4 / w.max(1) as usize) as u32;
        let outcome = if self.limited { Outcome::Limited } else if self.unmatched { Outcome::Partial } else { Outcome::Complete };
        (Image { w, h, rgba }, outcome)
    }
}
