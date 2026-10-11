// Editor — marks on a picture, and on the screen itself.
//
// Three things, each a window of its own that exists only while it is used:
// the editor (one window per picture: arrows, shapes, text, highlight, blur,
// crop, a backdrop, a watermark, Copy and Save), drawing on the live screen (a
// see-through window over one display) and the scrolling capture (a region is
// grabbed while the user scrolls it, and the new rows are joined into one long
// picture, scroll.rs).
//
// The contract is docs/ARCHITECTURE.md §2: nothing runs before `start`,
// everything is gone after `stop`, every command arrives through `call`. At
// rest there is nothing: no hook, timer, thread or window. The drawing itself
// is the page's (src/features/editor); a picture comes here as the PNG the
// page made, pixel for pixel, and leaves for the clipboard or a file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::Engine;
use notch_core::{log, win, Ctx, Feature, Group, Manifest, Monitor as Where, ShortcutDef, Use, WindowSpec};
use serde_json::{json, Value};
use svc_capture::{Image, Monitor, Rect};
use tauri::{LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

pub mod scroll;

const ID: &str = "editor";
const DRAW: &str = "editor-draw";
const SCROLL: &str = "editor-scroll";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Editor",
    group: Group::Capture,
    // Part of taking a capture, and nothing of it exists until it is asked for.
    default_on: true,
    uses: &[Use::Capture, Use::Clipboard],
    shortcuts: &[
        ShortcutDef { id: "draw", label: "Draw on the screen", default: None },
        ShortcutDef { id: "scroll", label: "Capture a scrolling page", default: None },
    ],
    tray: &[],
    // `backdrop` and `watermark` keep the shape the recorder will share.
    defaults: r#"{
        "color": 0, "size": 4,
        "backdrop": { "kind": "none", "presetID": "indigo", "padding": 0.5, "cornerRadius": 0.3 },
        "watermark": { "kind": "none", "text": "", "color": 5, "anchor": 8, "size": 0.3, "opacity": 0.4 }
    }"#,
};

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn cursor() -> (i32, i32) {
    let mut at = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut at);
    }
    (at.x, at.y)
}

/// The display under the pointer.
fn display() -> Option<Monitor> {
    let (x, y) = cursor();
    let monitors = svc_capture::monitors();
    monitors.iter().find(|m| m.rect.contains(x, y)).or(monitors.first()).cloned()
}

fn rect_arg(value: &Value) -> Option<Rect> {
    let n = |key: &str| value[key].as_f64().map(|v| v.round() as i32);
    Some(Rect { x: n("x")?, y: n("y")?, w: n("w")?, h: n("h")? })
}

// ── Pictures in and out ───────────────────────────────────────────────────────

/// What the editor opens: the kinds of picture its page can decode.
const KINDS: [(&str, &str); 6] =
    [("png", "image/png"), ("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("bmp", "image/bmp"), ("gif", "image/gif"), ("webp", "image/webp")];
const LARGEST: u64 = 96 * 1024 * 1024;

pub struct Loaded {
    /// The file's name.
    pub name: String,
    pub mime: &'static str,
    /// The file as it is: the page decodes it.
    pub bytes: Vec<u8>,
    /// Pixels, when the file says so in its first bytes (a PNG).
    pub size: Option<(u32, u32)>,
}

/// Width and height from a PNG's header.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(12..16) == Some(b"IHDR") { Some((be(16)?, be(20)?)) } else { None }
}

pub fn load(path: &Path) -> Result<Loaded, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let mime = KINDS.iter().find(|(e, _)| *e == ext).map(|(_, mime)| *mime).ok_or("That file is not a picture the editor opens.")?;
    let meta = std::fs::metadata(path).map_err(|_| "That picture is not there any more.".to_string())?;
    if !meta.is_file() || meta.len() > LARGEST {
        return Err("That picture is too large to edit.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("The picture could not be read: {e}"))?;
    Ok(Loaded { name: path.file_name().unwrap_or_default().to_string_lossy().into_owned(), mime, size: png_size(&bytes), bytes })
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The PNG the page made, as text: its bytes and, decoded, the proof that it is one.
pub fn decoded(png: &str) -> Result<(Vec<u8>, Image), String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(png).map_err(|_| "The picture did not arrive whole.".to_string())?;
    let image = svc_capture::decode_png(&bytes)?;
    Ok((bytes, image))
}

/// Writes the PNG as "stem.png" in `dir`, or "stem (2).png"… when that is taken.
pub fn save_png(dir: &Path, stem: &str, png: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{} cannot be used: {e}", dir.display()))?;
    let out = (1u32..)
        .map(|n| dir.join(if n == 1 { format!("{stem}.png") } else { format!("{stem} ({n}).png") }))
        .find(|path| !path.exists())
        .expect("a free name");
    std::fs::write(&out, png).map_err(|e| format!("The picture could not be saved: {e}"))?;
    Ok(out)
}

/// Where "Save" puts a picture: where the Capture feature saves, so there is one folder.
fn folder(cx: &Ctx) -> PathBuf {
    let chosen = PathBuf::from(cx.settings("capture")["folder"].as_str().unwrap_or_default().trim());
    if chosen.is_absolute() {
        return chosen;
    }
    win::picture_dirs().into_iter().find(|dir| dir.is_dir()).unwrap_or_else(win::home_dir).join("Screenshots")
}

// ── The editor's windows ──────────────────────────────────────────────────────

struct Doc {
    label: String,
    path: PathBuf,
    /// Where "Save" put it.
    saved: Option<PathBuf>,
}

/// The pictures that are open, one window each. Empty at rest and after `stop`.
static DOCS: Mutex<Vec<Doc>> = Mutex::new(Vec::new());

/// The smallest the editor's window gets, in logical pixels: the tools still fit.
const LEAST: (f64, f64) = (880.0, 580.0);

/// Opens the editor on a picture file; a file that is open already comes to the front.
pub fn open(cx: &Ctx, path: &Path) -> Result<(), String> {
    let picture = load(path)?;
    let known = locked(&DOCS).iter().find(|d| d.path == path).map(|d| d.label.clone());
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let label = known.clone().unwrap_or_else(|| format!("editor-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
    // Large enough to show the picture pixel for pixel, when the display has the room.
    let (scale, room) = display().map_or((1.0, LEAST), |m| (m.scale, (m.rect.w as f64 / m.scale * 0.86, m.rect.h as f64 / m.scale * 0.86)));
    let (iw, ih) = picture.size.unwrap_or((1280, 800));
    let w = (iw as f64 / scale + 132.0).clamp(LEAST.0, room.0.max(LEAST.0));
    let h = (ih as f64 / scale + 152.0).clamp(LEAST.1, room.1.max(LEAST.1));
    if known.is_none() {
        // Before the window: its page asks for the picture as soon as it is up.
        locked(&DOCS).push(Doc { label: label.clone(), path: path.to_path_buf(), saved: None });
    }
    let spec = WindowSpec { label: label.clone(), feature: ID.into(), page: "editor".into(), w, h, frameless: true, monitor: Where::Cursor, ..Default::default() };
    match cx.window().open(spec) {
        Ok(window) if known.is_none() => {
            let _ = window.set_min_size(Some(LogicalSize::new(LEAST.0, LEAST.1)));
            window.on_window_event(move |event| {
                if matches!(event, WindowEvent::Destroyed) {
                    locked(&DOCS).retain(|d| d.label != label);
                }
            });
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(err) => {
            locked(&DOCS).retain(|d| d.label != label);
            Err(err)
        }
    }
}

/// The picture of the window a page named: only windows this feature opened.
fn doc(args: &Value) -> Result<(String, PathBuf, Option<PathBuf>), String> {
    let label = args["label"].as_str().unwrap_or_default();
    let docs = locked(&DOCS);
    let doc = docs.iter().find(|d| d.label == label).ok_or("That picture is closed.")?;
    Ok((doc.label.clone(), doc.path.clone(), doc.saved.clone()))
}

// ── Windows over a whole display ──────────────────────────────────────────────

/// A see-through window over the display under the pointer, with the keyboard.
/// `short` leaves the last row of the display free, so Windows does not take a
/// window that stays up for a full-screen program.
fn cover(cx: &Ctx, label: &str, page: &str, exclude_from_capture: bool, short: i32) -> Result<(WebviewWindow, Monitor), String> {
    let monitor = display().ok_or("There is no display to draw on.")?;
    let spec = WindowSpec {
        label: label.into(),
        feature: ID.into(),
        page: page.into(),
        w: 320.0,
        h: 200.0,
        frameless: true,
        topmost: true,
        exclude_from_capture,
        transparent: true,
        monitor: Where::Primary,
        ..Default::default()
    };
    let window = cx.window().open(spec)?;
    let _ = window.set_skip_taskbar(true);
    let _ = window.set_resizable(false);
    // Exactly the display, in physical pixels: one pixel of the page is one of the screen.
    let _ = window.set_position(PhysicalPosition::new(monitor.rect.x, monitor.rect.y));
    let _ = window.set_size(PhysicalSize::new(monitor.rect.w as u32, (monitor.rect.h - short).max(1) as u32));
    let _ = window.set_focus();
    Ok((window, monitor))
}

/// Numbers the windows that cover a display, across both kinds.
fn next_cover() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A window over a whole display takes every click: should its page never come
/// up, nobody is left behind it. One short-lived thread per window.
fn watch(cx: &Ctx, id: u32) {
    let cx = cx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(6));
        // Each list is let go of before its window is closed.
        let drawing = locked(&DRAWING).as_ref().is_some_and(|d| d.id == id && !d.ready);
        let scrolling = locked(&SCROLLING).as_ref().is_some_and(|s| s.turn == id && !s.ready);
        if drawing {
            log::line("editor: the drawing did not come up; it is closed");
            draw_end(&cx);
        }
        if scrolling {
            log::line("editor: the scrolling capture did not come up; it is ended");
            scroll_end(&cx);
        }
    });
}

// ── Drawing on the screen ─────────────────────────────────────────────────────

struct Drawing {
    id: u32,
    /// Its page is up.
    ready: bool,
    /// The display's top-left corner.
    origin: (i32, i32),
    /// Clicks go through to what is under the drawing.
    through: bool,
    /// The tool bar on the desktop: the one place that takes clicks while they go through.
    bar: Rect,
    /// Counts the changes of `through`, for the thread that watches the pointer.
    turn: u32,
}

static DRAWING: Mutex<Option<Drawing>> = Mutex::new(None);

fn draw_end(cx: &Ctx) {
    let was = locked(&DRAWING).take();
    if was.is_some() {
        cx.window().close(DRAW);
    }
}

fn draw(cx: &Ctx) -> Result<Value, String> {
    // The shortcut again: between drawing and using what is under the drawing.
    if locked(&DRAWING).is_some() {
        cx.emit(ID, "draw-toggle", ());
        return Ok(Value::Null);
    }
    let (window, monitor) = cover(cx, DRAW, "draw", false, 1)?;
    let id = next_cover();
    *locked(&DRAWING) = Some(Drawing { id, ready: false, origin: (monitor.rect.x, monitor.rect.y), through: false, bar: Rect::default(), turn: 0 });
    watch(cx, id);
    // Closed some other way (Alt+F4): the next shortcut starts afresh.
    window.on_window_event(|event| {
        if matches!(event, WindowEvent::Destroyed) {
            locked(&DRAWING).take();
        }
    });
    Ok(Value::Null)
}

/// Clicks go through the drawing (`on`) or draw on it. While they go through,
/// a thread watches the pointer so the tool bar still takes its clicks; it ends
/// with the mode.
fn draw_through(cx: &Ctx, on: bool, bar: Option<Rect>) -> Result<Value, String> {
    let window = cx.app().get_webview_window(DRAW).ok_or("The drawing is closed.")?;
    let turn = {
        let mut drawing = locked(&DRAWING);
        let drawing = drawing.as_mut().ok_or("The drawing is closed.")?;
        drawing.through = on;
        drawing.turn += 1;
        if let Some(bar) = bar {
            drawing.bar = Rect { x: drawing.origin.0 + bar.x, y: drawing.origin.1 + bar.y, ..bar };
        }
        drawing.turn
    };
    let _ = window.set_ignore_cursor_events(on);
    if !on {
        // Esc and the text tool need the keyboard.
        let _ = window.set_focus();
        return Ok(Value::Null);
    }
    std::thread::spawn(move || {
        let mut over = false;
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let bar = match locked(&DRAWING).as_ref() {
                Some(drawing) if drawing.through && drawing.turn == turn => drawing.bar,
                _ => return,
            };
            let (x, y) = cursor();
            if bar.contains(x, y) != over {
                over = !over;
                let _ = window.set_ignore_cursor_events(!over);
            }
        }
    });
    Ok(Value::Null)
}

// ── Scrolling capture ─────────────────────────────────────────────────────────

struct Scrolling {
    /// Which session this is, for the thread that began with it.
    turn: u32,
    monitor: Monitor,
    /// Its page is up.
    ready: bool,
    /// "Done" was pressed: one more grab, then the picture.
    done: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    /// The grabs have begun.
    running: bool,
}

static SCROLLING: Mutex<Option<Scrolling>> = Mutex::new(None);

/// How long a session may run.
const LONGEST: Duration = Duration::from_secs(120);

/// Ends the session that is up, without a picture. False when there was none.
fn scroll_end(cx: &Ctx) -> bool {
    let Some(session) = locked(&SCROLLING).take() else { return false };
    session.cancel.store(true, Ordering::Relaxed);
    cx.window().close(SCROLL);
    true
}

/// Puts the selector over the display: the user drags the part of the page that moves.
fn scroll_begin(cx: &Ctx) -> Result<Value, String> {
    // The shortcut again: the session that is up ends.
    if scroll_end(cx) {
        return Ok(Value::Null);
    }
    let turn = next_cover();
    // Left out of every grab: nothing of the selector or its bar is in the picture.
    let (window, monitor) = cover(cx, SCROLL, "scroll", true, 0)?;
    let session = Scrolling { turn, monitor, ready: false, done: Arc::default(), cancel: Arc::default(), running: false };
    *locked(&SCROLLING) = Some(session);
    watch(cx, turn);
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            let mut session = locked(&SCROLLING);
            if session.as_ref().is_some_and(|s| s.turn == turn) {
                if let Some(s) = session.take() {
                    s.cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    Ok(Value::Null)
}

/// The region is chosen (pixels of the selector's window): the window becomes
/// a small bar beside it and the grabs begin.
fn scroll_start(cx: &Ctx, args: &Value) -> Result<Value, String> {
    let window = cx.app().get_webview_window(SCROLL).ok_or("No scrolling capture is in progress.")?;
    let (turn, monitor, done, cancel) = {
        let mut session = locked(&SCROLLING);
        let session = session.as_mut().filter(|s| !s.running).ok_or("No scrolling capture is in progress.")?;
        session.running = true;
        (session.turn, session.monitor.clone(), session.done.clone(), session.cancel.clone())
    };
    let on = monitor.rect;
    let picked = rect_arg(args).map(|r| Rect { x: on.x + r.x, y: on.y + r.y, ..r }.clip(on)).filter(|r| r.w >= 40 && r.h >= 40);
    let Some(region) = picked else {
        scroll_end(cx);
        return Err("That is too small to scroll.".into());
    };
    // The bar: under the region, above it, or over its bottom edge when the display has no room.
    let px = |v: f64| (v * monitor.scale).round() as i32;
    let (w, h, gap) = (px(380.0).min(on.w), px(44.0), px(10.0));
    let x = (region.x + (region.w - w) / 2).clamp(on.x, on.x + on.w - w);
    let y = if region.y + region.h + gap + h <= on.y + on.h {
        region.y + region.h + gap
    } else if region.y - gap - h >= on.y {
        region.y - gap - h
    } else {
        region.y + region.h - gap - h
    };
    let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
    let _ = window.set_position(PhysicalPosition::new(x, y));

    let cx = cx.clone();
    std::thread::spawn(move || {
        let grabs = || -> Result<Option<(Image, scroll::Outcome)>, String> {
            let mut stitcher = scroll::Stitcher::new(svc_capture::grab(region)?);
            let began = Instant::now();
            let mut shown = 0;
            loop {
                std::thread::sleep(Duration::from_millis(90));
                if cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                // Read before the grab: "Done" still gets one grab of where the page stopped.
                let last = done.load(Ordering::Relaxed);
                let more = stitcher.feed(&svc_capture::grab(region)?);
                if stitcher.rows() != shown {
                    shown = stitcher.rows();
                    cx.emit(ID, "scroll", json!({ "rows": shown }));
                }
                if !last && began.elapsed() > LONGEST {
                    stitcher.limit();
                }
                if last || !more || began.elapsed() > LONGEST {
                    return Ok(Some(stitcher.finish()));
                }
            }
        };
        let result = grabs();
        // Only for the session that began this: a cancelled one leaves nothing.
        let ours = {
            let mut session = locked(&SCROLLING);
            let ours = session.as_ref().is_some_and(|s| s.turn == turn);
            if ours {
                session.take();
            }
            ours
        };
        if !ours {
            return;
        }
        cx.window().close(SCROLL);
        let kept = result.and_then(|found| match found {
            Some((image, outcome)) => {
                let t = win::local_time();
                let name = format!("Scrolling capture {:04}-{:02}-{:02} at {:02}.{:02}.{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second);
                let file = save_png(&cx.data_dir(ID).join("scrolls"), &name, &svc_capture::encode_png(&image)?)?;
                open(&cx, &file)?;
                Ok(Some((image.w, image.h, outcome)))
            }
            None => Ok(None),
        });
        match kept {
            Ok(Some((w, h, outcome))) => {
                let outcome = match outcome {
                    scroll::Outcome::Complete => "complete",
                    scroll::Outcome::Partial => "partial",
                    scroll::Outcome::Limited => "limited",
                };
                cx.emit(ID, "scrolled", json!({ "outcome": outcome, "w": w, "h": h }));
            }
            Ok(None) => {}
            Err(err) => {
                log::line(format!("editor: scrolling capture: {err}"));
                cx.emit(ID, "scrolled", json!({ "outcome": "failed" }));
            }
        }
    });
    // Not null: the page tells "began" from a call that failed.
    Ok(json!(true))
}

// ── The feature ───────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct Editor;

impl Feature for Editor {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn start(&self, cx: &Ctx) -> Result<(), String> {
        // The long pictures of the last run were the app's own copies: Save made the user's.
        let _ = std::fs::remove_dir_all(cx.data_dir(ID).join("scrolls"));
        Ok(())
    }

    fn stop(&self, cx: &Ctx) {
        scroll_end(cx);
        draw_end(cx);
        // The list is let go of first: closing a window comes back to it.
        let docs = std::mem::take(&mut *locked(&DOCS));
        for doc in docs {
            cx.window().close(&doc.label);
        }
    }

    fn call(&self, cx: &Ctx, method: &str, args: Value) -> Result<Value, String> {
        match method {
            "shortcut" => match args["id"].as_str().unwrap_or_default() {
                "draw" => draw(cx),
                "scroll" => scroll_begin(cx),
                _ => Err("unknown method".into()),
            },

            // The page of a window that covers a display is up.
            "ready" => {
                if let Some(drawing) = locked(&DRAWING).as_mut().filter(|_| args["page"] == "draw") {
                    drawing.ready = true;
                }
                if let Some(session) = locked(&SCROLLING).as_mut().filter(|_| args["page"] == "scroll") {
                    session.ready = true;
                }
                Ok(Value::Null)
            }

            // ── The editor ────────────────────────────────────────────────────
            // A picture file by its path, or a kept capture by its file's name
            // (what the Captures page knows of it).
            "open" => {
                let path = match (args["path"].as_str(), args["capture"].as_str()) {
                    (Some(path), _) if Path::new(path).is_absolute() => PathBuf::from(path),
                    (None, Some(name)) if Path::new(name).file_name().is_some_and(|n| n == name) => cx.data_dir("capture").join("captures").join(name),
                    _ => return Err("No picture was named.".into()),
                };
                open(cx, &path)?;
                Ok(Value::Null)
            }
            // The editor's page asks for its picture: the file as it is.
            "doc" => {
                let (_, path, _) = doc(&args)?;
                let picture = load(&path)?;
                Ok(json!({ "name": picture.name, "url": format!("data:{};base64,{}", picture.mime, base64(&picture.bytes)) }))
            }
            // The page's PNG, to the clipboard or into the user's folder. Only ever after a click.
            "export" => {
                let (label, path, _) = doc(&args)?;
                let (png, image) = decoded(args["png"].as_str().unwrap_or_default())?;
                match args["action"].as_str().unwrap_or_default() {
                    "copy" => {
                        svc_capture::clipboard_image(&image, &png)?;
                        Ok(json!({ "w": image.w, "h": image.h }))
                    }
                    "save" => {
                        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                        let out = save_png(&folder(cx), &format!("{stem} edited"), &png)?;
                        if let Some(doc) = locked(&DOCS).iter_mut().find(|d| d.label == label) {
                            doc.saved = Some(out.clone());
                        }
                        Ok(json!({ "path": out.to_string_lossy(), "w": image.w, "h": image.h }))
                    }
                    _ => Err("unknown method".into()),
                }
            }
            // The folder "Save" wrote to.
            "reveal" => {
                let saved = doc(&args)?.2.ok_or("It is not saved yet.")?;
                win::reveal_folder(&saved.parent().unwrap_or(&saved).to_string_lossy());
                Ok(Value::Null)
            }
            "close" => {
                let (label, ..) = doc(&args)?;
                cx.window().close(&label);
                Ok(Value::Null)
            }

            // ── Drawing on the screen ─────────────────────────────────────────
            "draw" => draw(cx),
            "draw_through" => draw_through(cx, args["on"].as_bool().unwrap_or(false), rect_arg(&args["bar"])),
            "draw_close" => {
                draw_end(cx);
                Ok(Value::Null)
            }

            // ── Scrolling capture ─────────────────────────────────────────────
            "scroll" => scroll_begin(cx),
            "scroll_start" => scroll_start(cx, &args),
            "scroll_done" => {
                if let Some(session) = locked(&SCROLLING).as_ref() {
                    session.done.store(true, Ordering::Relaxed);
                }
                Ok(Value::Null)
            }
            "scroll_cancel" => {
                scroll_end(cx);
                Ok(Value::Null)
            }
            _ => Err("unknown method".into()),
        }
    }

    fn strings(&self) -> &'static str {
        include_str!("../../../src/features/editor/strings.json")
    }
}

pub fn feature() -> std::sync::Arc<dyn Feature> {
    std::sync::Arc::new(Editor)
}
