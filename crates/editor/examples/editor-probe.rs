// editor-probe: proves the Rust side of the editor on this machine.
//
//   cargo run -p notch-feat-editor --example editor-probe [-- <folder to work in>]
//
// 1. Load, export and save: a made-up picture the size a 1280×800 window has at
//    125 % (1600×1000) and an odd-sized one go to a file, come back through
//    `load`, travel as the text the page sends (`decoded`) and are saved with
//    `save_png`; every size and every pixel has to be what went in, and a line
//    one pixel thick has to be one pixel thick in the saved file.
// 2. Scrolling capture: a made-up page (a sidebar and a footer that stay put, a
//    scrollbar that moves) is "scrolled" as grabs from memory, and the long
//    picture has to be the page, pixel for pixel.
//
// Nothing of the screen is read and nothing is shown. The files go to
// target/editor-probe/ (or the folder given) and are deleted before it exits.
// The clipboard is not touched: svc-capture's own probe covers `clipboard_image`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use base64::Engine;
use notch_feat_editor::scroll::{Outcome, Stitcher};
use notch_feat_editor::{decoded, load, png_size, save_png};
use svc_capture::Image;

static FAILED: AtomicU32 = AtomicU32::new(0);

fn check(ok: bool, what: &str) -> bool {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        FAILED.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// The red of the design language, as the colour of the line.
const LINE: [u8; 3] = [0xF4, 0x50, 0x5E];

/// A gradient with one line a pixel thick across the middle row.
fn picture(w: u32, h: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let line = y == h / 2 && x >= 100 && x < w - 100;
            let px = if line { LINE } else { [(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8] };
            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    Image { w, h, rgba }
}

fn files(dir: &PathBuf, w: u32, h: u32) -> Result<(), String> {
    let made = picture(w, h);
    let png = svc_capture::encode_png(&made)?;
    let file = dir.join(format!("probe {w}x{h}.png"));
    std::fs::write(&file, &png).map_err(|e| e.to_string())?;

    let loaded = load(&file)?;
    check(loaded.size == Some((w, h)) && loaded.mime == "image/png", &format!("load: {} is {:?} ({})", loaded.name, loaded.size, loaded.mime));
    check(loaded.bytes == png, "load: the file's bytes, as they are");

    // What the page sends back: its PNG as text.
    let text = base64::engine::general_purpose::STANDARD.encode(&loaded.bytes);
    let (bytes, image) = decoded(&text)?;
    check((image.w, image.h) == (w, h) && image == made, &format!("export: {}×{} and every pixel the same", image.w, image.h));

    let saved = save_png(dir, "probe edited", &bytes)?;
    let again = save_png(dir, "probe edited", &bytes)?;
    check(saved != again && again.to_string_lossy().ends_with("(2).png"), &format!("save: a second save takes a new name ({})", again.file_name().unwrap_or_default().to_string_lossy()));
    let back = std::fs::read(&saved).map_err(|e| e.to_string())?;
    let read = svc_capture::decode_png(&back)?;
    check(png_size(&back) == Some((w, h)) && read == made, &format!("save: the file is {}×{} and every pixel the same", read.w, read.h));
    let (row, x) = (h / 2, w / 2);
    let thin = read.pixel(x, row) == Some(LINE) && read.pixel(x, row - 1) != Some(LINE) && read.pixel(x, row + 1) != Some(LINE);
    check(thin, "save: the one-pixel line is one pixel thick");
    for path in [file, saved, again] {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

// ── A page to scroll ──────────────────────────────────────────────────────────

const VIEW: (u32, u32) = (960, 700);
const PAGE_H: u32 = 4200;
const SIDEBAR: u32 = 120;
const SCROLLBAR: u32 = 30;
const FOOTER: u32 = 40;

struct Page {
    /// The band each row is in, and each band's greys in blocks of 60 pixels.
    band: Vec<usize>,
    greys: Vec<[u8; 16]>,
}

impl Page {
    fn new() -> Self {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let (mut band, mut greys) = (Vec::new(), Vec::new());
        while band.len() < PAGE_H as usize {
            let rows = 4 + (next() % 17) as usize;
            band.extend(std::iter::repeat(greys.len()).take(rows));
            greys.push(std::array::from_fn(|_| 20 + (next() % 216) as u8));
        }
        Self { band, greys }
    }

    /// The page itself, at a column of the view and a row of the page.
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        let g = self.greys[self.band[y as usize]][(x / 60) as usize];
        [g, g.wrapping_add((x % 7) as u8), 255 - g]
    }

    fn footer(x: u32, row: u32) -> [u8; 3] {
        [30 + (row * 3) as u8, 40, (x % 200) as u8]
    }

    /// What the region shows when the page is scrolled by `offset` rows.
    fn view(&self, offset: u32) -> Image {
        let (w, h) = VIEW;
        let thumb = offset * (h - 80) / (PAGE_H - h);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let px = if x < SIDEBAR {
                    [(y % 97) as u8 + 60, 70, (x * 2) as u8]
                } else if y >= h - FOOTER {
                    Self::footer(x, y - (h - FOOTER))
                } else if x >= w - SCROLLBAR {
                    if y >= thumb && y < thumb + 80 { [200, 200, 200] } else { [45, 45, 45] }
                } else {
                    self.at(x, offset + y)
                };
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        Image { w, h, rgba }
    }
}

fn scrolling() {
    let page = Page::new();
    let (w, h) = VIEW;
    // Still, a scroll, a small one, back up, on down the page, and the last few rows.
    let offsets = [0, 137, 149, 420, 300, 900, 1380, 1392, 1900, 2400, 2900, 3400, PAGE_H - h];
    let mut stitcher = Stitcher::new(page.view(0));
    let t = Instant::now();
    let all = offsets.iter().all(|offset| stitcher.feed(&page.view(*offset)));
    let each = t.elapsed() / offsets.len() as u32;
    let rows = stitcher.rows();
    let (long, outcome) = stitcher.finish();
    println!("     {} grabs of {w}×{h}, {each:.0?} each (a debug build is the slow case)", offsets.len());
    check(all && outcome == Outcome::Complete, &format!("scrolling: every grab was followed ({outcome:?})"));
    check(long.w == w - SIDEBAR, &format!("scrolling: the sidebar that stays put is left out ({} of {w} columns)", long.w));
    check(long.h == PAGE_H && rows == PAGE_H as usize, &format!("scrolling: {} rows for a page of {PAGE_H}", long.h));
    if long.w != w - SIDEBAR || long.h != PAGE_H {
        return;
    }
    let content = (0..PAGE_H - FOOTER).all(|y| (SIDEBAR..w - SCROLLBAR).all(|x| long.pixel(x - SIDEBAR, y) == Some(page.at(x, y))));
    check(content, "scrolling: every pixel of the page is where it is on the page");
    let footer = (0..FOOTER).all(|row| (SIDEBAR..w).all(|x| long.pixel(x - SIDEBAR, PAGE_H - FOOTER + row) == Some(Page::footer(x, row))));
    check(footer, "scrolling: the footer that stays put is there once, at the end");

    // A page that does not move is the first grab, whole.
    let mut still = Stitcher::new(page.view(500));
    still.feed(&page.view(500));
    let (one, outcome) = still.finish();
    check((one.w, one.h) == VIEW && outcome == Outcome::Complete, "scrolling: a page that did not move is the region as it is");

    // Something else entirely in the region: the part that was complete is kept.
    let mut lost = Stitcher::new(page.view(0));
    lost.feed(&page.view(300));
    lost.feed(&Page { band: page.band.iter().rev().copied().collect(), greys: page.greys.iter().rev().copied().collect() }.view(2000));
    let (kept, outcome) = lost.finish();
    check(outcome == Outcome::Partial && kept.h == h + 300, &format!("scrolling: a page that changed keeps what was complete ({outcome:?}, {} rows)", kept.h));
}

fn main() {
    let given = std::env::args().nth(1).map(PathBuf::from);
    let dir = given.clone().unwrap_or_else(|| PathBuf::from("target").join("editor-probe"));
    let _ = std::fs::create_dir_all(&dir);

    // 1280×800 at 125 %, and a size no scale divides.
    for (w, h) in [(1600, 1000), (1281, 803)] {
        if let Err(err) = files(&dir, w, h) {
            check(false, &format!("{w}×{h}: {err}"));
        }
    }
    let wrong = dir.join("probe.txt");
    let _ = std::fs::write(&wrong, "not a picture");
    check(load(&wrong).is_err() && load(&dir.join("gone.png")).is_err(), "load: a text file and a missing file are refused");
    check(decoded("not a png").is_err(), "export: text that is not a PNG is refused");
    let _ = std::fs::remove_file(wrong);
    if given.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }

    scrolling();

    let failed = FAILED.load(Ordering::Relaxed);
    println!("{}", if failed == 0 { "editor-probe: all good".to_string() } else { format!("editor-probe: {failed} failed") });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
