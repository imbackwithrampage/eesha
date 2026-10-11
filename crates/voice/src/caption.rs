// The capsule's window: frameless, transparent, topmost, never activated, opened through the window
// factory (`cx.window().open`, page `capsule` of the feature `voice`) and placed under the island.
// Package voice-ui (VOICE §10; §7.5; ARCHITECTURE §6). What it shows and where it goes are the page's
// (src/features/voice/ui/): this module only opens, places and hides the window, tells the page where
// the island hangs, and passes two messages between the island's page, which knows the turn, and the
// capsule's own page. It never takes the focus, a key or the island's clicks: the window cannot be
// focused, it is moved and shown with `SWP_NOACTIVATE`, and the page puts it beside the island, never on it.

use crate::{Result, VoiceError};
use notch_core::{Ctx, WindowSpec};
use serde_json::{json, Value};
use std::ffi::c_void;
use tauri::{Manager, WebviewWindow};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE};

/// The window's label.
pub const LABEL: &str = "voice-capsule";
/// The island's window (src-tauri/src/island.rs, `WINDOW_LABEL`).
const ISLAND: &str = "island";

pub fn start(_cx: &Ctx) -> Result<()> {
    Ok(())
}

/// Closes the window.
pub fn stop(cx: &Ctx) {
    cx.window().close(LABEL);
}

pub fn settings(_cx: &Ctx, _settings: &Value) {}

fn failed(e: impl std::fmt::Display) -> VoiceError {
    VoiceError::Failed(e.to_string())
}

fn hwnd(w: &WebviewWindow) -> Result<HWND> {
    Ok(HWND(w.hwnd().map_err(failed)?.0 as *mut c_void))
}

/// `place {x, y, w, h}` → `{}` (physical pixels of the screen; opens the window if it is closed, shows it
/// without activating) · `hide` → `{}` · `origin` → `{x, y, scale, left, top, right, bottom}` (the top
/// centre of the island's window, its scale and the bounds of its display, physical pixels: what the
/// page places the capsule from) · `show {…}` → `{}` (the island's page to the capsule's, as
/// `voice:caption`) · `act {what}` → `{}` (a click in the capsule to the island's page, as
/// `voice:caption-act`). The words of a caption pass through and are never logged.
pub fn call(cx: &Ctx, method: &str, args: Value) -> Result<Value> {
    match method {
        "place" => place(cx, &args)?,
        "hide" => {
            if let Some(w) = cx.app().get_webview_window(LABEL) {
                let _ = unsafe { ShowWindow(hwnd(&w)?, SW_HIDE) };
            }
        }
        "origin" => return origin(cx),
        "show" => cx.emit(crate::ID, "caption", args),
        "act" => cx.emit(crate::ID, "caption-act", args),
        _ => return Err(VoiceError::Failed("unknown method".into())),
    }
    Ok(json!({}))
}

fn place(cx: &Ctx, args: &Value) -> Result<()> {
    let n = |key: &str| args[key].as_f64().filter(|v| v.is_finite()).map(|v| v.round() as i32);
    let (Some(x), Some(y), Some(w), Some(h)) = (n("x"), n("y"), n("w"), n("h")) else {
        return Err(VoiceError::Failed("place: x, y, w, h".into()));
    };
    if w <= 0 || h <= 0 {
        return Err(VoiceError::Failed("place: an empty window".into()));
    }
    let win = match cx.app().get_webview_window(LABEL) {
        Some(win) => win,
        None => {
            // The factory shows it once in the middle of the display: its page draws nothing until it is told to.
            let win = cx
                .window()
                .open(WindowSpec {
                    label: LABEL.into(),
                    feature: crate::ID.into(),
                    page: "capsule".into(),
                    w: 380.0,
                    h: 32.0,
                    frameless: true,
                    topmost: true,
                    no_activate: true,
                    transparent: true,
                    ..Default::default()
                })
                .map_err(VoiceError::Failed)?;
            // No sizing frame: Windows holds a window that has one to a least width, wider than the capsule without captions.
            let _ = win.set_resizable(false);
            win
        }
    };
    // Moved, sized and shown in one step, above the island, and never made the active window.
    unsafe { SetWindowPos(hwnd(&win)?, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW) }.map_err(failed)
}

fn origin(cx: &Ctx) -> Result<Value> {
    let island = cx.app().get_webview_window(ISLAND).ok_or_else(|| failed("no island window"))?;
    let (at, size) = (island.inner_position().map_err(failed)?, island.inner_size().map_err(failed)?);
    let display = island
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| cx.app().primary_monitor().ok().flatten())
        .ok_or_else(|| failed("no display"))?;
    let (corner, area) = (display.position(), display.size());
    Ok(json!({
        "x": at.x + size.width as i32 / 2,
        "y": at.y,
        "scale": island.scale_factor().map_err(failed)?,
        "left": corner.x,
        "top": corner.y,
        "right": corner.x + area.width as i32,
        "bottom": corner.y + area.height as i32,
    }))
}
