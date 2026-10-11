// GIF: frames taken from the video (video.rs), written by a small encoder of our own.
// Sizing is ported from MediaSupport.swift (`gifSizePlan`, `safeGIFFrameCount`).
// debt: one fixed 256-colour palette (3-3-2 bits) with ordered dithering; a palette fitted to the clip
// (median cut) would look better.

use crate::video::{self, Frames, Progress};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_FRAMES: usize = 300;
const MIN_FPS: f64 = 6.0;
const MIN_WIDTH: u32 = 160;
const HEADROOM: f64 = 0.94;

pub struct Opts {
    pub fps: f64,
    pub width: u32,
    pub loops: bool,
    pub target_mb: Option<u32>,
    pub start: f64,
    pub end: f64,
}

/// Palette index of an RGBA pixel at (x, y), with a 4x4 ordered dither.
fn quantise(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    const BAYER: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 4;
            let d = BAYER[y % 4][x % 4] * 2 - 15; // about -15..15 of 255
            let q = |v: u8, bits: u32| {
                let step = 256 >> bits;
                (((v as i32 + d * step / 32).clamp(0, 255)) >> (8 - bits)) as u8
            };
            out.push((q(rgba[at], 3) << 5) | (q(rgba[at + 1], 3) << 2) | q(rgba[at + 2], 2));
        }
    }
    out
}

struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u16, size: u32) {
        self.acc |= (code as u32) << self.n;
        self.n += size;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
}

fn lzw(data: &[u8]) -> Vec<u8> {
    const CLEAR: u16 = 256;
    let mut bits = Bits { out: vec![], acc: 0, n: 0 };
    let mut dict: HashMap<(u16, u8), u16> = HashMap::new();
    let (mut next, mut size) = (258u16, 9u32);
    bits.put(CLEAR, size);
    let mut cur = data[0] as u16;
    for &b in &data[1..] {
        if let Some(&c) = dict.get(&(cur, b)) {
            cur = c;
            continue;
        }
        bits.put(cur, size);
        dict.insert((cur, b), next);
        if next as u32 >= 1 << size {
            if size == 12 {
                bits.put(CLEAR, size);
                dict.clear();
                (next, size) = (258, 9);
                cur = b as u16;
                continue;
            }
            size += 1;
        }
        next += 1;
        cur = b as u16;
    }
    bits.put(cur, size);
    bits.put(257, size);
    if bits.n > 0 {
        bits.out.push(bits.acc as u8);
    }
    bits.out
}

/// The GIF file for frames already reduced to palette indices.
fn encode(frames: &[Vec<u8>], w: u16, h: u16, delay_cs: u16, loops: bool) -> Vec<u8> {
    let mut o = b"GIF89a".to_vec();
    o.extend(w.to_le_bytes());
    o.extend(h.to_le_bytes());
    o.extend([0xF7, 0, 0]); // global table of 256 colours
    for i in 0..256u32 {
        o.extend([(((i >> 5) & 7) * 255 / 7) as u8, (((i >> 2) & 7) * 255 / 7) as u8, ((i & 3) * 255 / 3) as u8]);
    }
    if loops {
        o.extend(b"\x21\xFF\x0BNETSCAPE2.0\x03\x01\x00\x00\x00");
    }
    for f in frames {
        o.extend([0x21, 0xF9, 4, 0, delay_cs as u8, (delay_cs >> 8) as u8, 0, 0]);
        o.push(0x2C);
        o.extend([0, 0, 0, 0]);
        o.extend(w.to_le_bytes());
        o.extend(h.to_le_bytes());
        o.extend([0, 8]); // no local table, minimum code size 8
        for chunk in lzw(f).chunks(255) {
            o.push(chunk.len() as u8);
            o.extend(chunk);
        }
        o.push(0);
    }
    o.push(0x3B);
    o
}

/// Makes the GIF beside the input and answers its path. A size target takes up to four passes,
/// lowering the frame rate first and then the width.
pub fn make(input: &Path, o: &Opts, cancel: &AtomicBool, progress: Progress) -> Result<PathBuf, String> {
    let src = video::info(input)?;
    let end = if o.end > 0.0 { o.end.min(src.secs) } else { src.secs };
    let start = o.start.clamp(0.0, end);
    let secs = end - start;
    let mut fps = if o.target_mb.is_some() { 15f64.min((MAX_FRAMES as f64 / secs).floor()).max(MIN_FPS) } else { o.fps.clamp(1.0, 30.0) };
    let mut width = if o.target_mb.is_some() { src.w.clamp(MIN_WIDTH, 1600) } else { o.width.clamp(MIN_WIDTH, 1600) };
    let width_cap = width;
    if secs <= 0.0 || (secs * fps).ceil() > MAX_FRAMES as f64 {
        return Err(format!("A GIF takes at most {} frames: {} seconds at {} frames a second.", MAX_FRAMES, (MAX_FRAMES as f64 / fps).floor(), fps));
    }
    let stem = input.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let dir = video::absolute(input.parent().unwrap_or(Path::new(".")))?;
    let out = svc_shell::free_name(&dir, &format!("{stem}.gif"));
    let frames = Frames::open(input)?;
    let target = o.target_mb.map(|mb| mb.clamp(1, 512) as i64 * 1_000_000);
    for pass in 0..4 {
        let (w, h) = {
            let w = width.min(width_cap).min(src.w.max(MIN_WIDTH)) & !1;
            (w.max(2), (((w as f64 * src.h as f64 / src.w as f64).round() as u32).max(2)) & !1)
        };
        let n = ((secs * fps).ceil() as usize).max(1);
        let mut indexed = Vec::with_capacity(n);
        for i in 0..n {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled.".into());
            }
            let at = (start + i as f64 / fps).min(end - 0.001).max(0.0);
            indexed.push(quantise(&frames.at(at, w, h)?, w as usize, h as usize));
            progress((pass as f64 + (i + 1) as f64 / n as f64) / 4.0);
        }
        let bytes = encode(&indexed, w as u16, h as u16, ((100.0 / fps).round() as u16).max(2), o.loops);
        let over = target.filter(|t| bytes.len() as i64 > *t);
        if let (Some(t), true) = (over, pass < 3) {
            let ratio = (t as f64 * HEADROOM / bytes.len() as f64).min(0.9);
            let next_fps = (fps * ratio).round().max(MIN_FPS);
            let remaining = (ratio / (next_fps / fps)).min(1.0);
            let next_w = ((width as f64 * remaining.sqrt()).round() as u32).max(MIN_WIDTH);
            if next_w < width || next_fps < fps {
                (fps, width) = (next_fps, next_w);
                continue;
            }
        }
        let part = dir.join(format!("{}.part", out.file_name().unwrap_or_default().to_string_lossy()));
        std::fs::write(&part, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
        progress(1.0);
        return Ok(out);
    }
    Err("The GIF would not get under the target size.".into())
}
