// Hold a key combination to dictate into whatever field has the cursor; release to insert.
//
// The keys are heard through svc-input (an ordered subscriber on the hook thread, so it only
// updates a small state and posts a message). A worker thread does the rest: opens the engine
// (`Hooks::open`), waits for the release, asks the engine for the text and pastes it through
// svc-inject. The engine is a parameter so the probe can feed it a made-up transcript.
//
// Which keys are swallowed: the key that completes the combination (and its repeats and release),
// and Esc while holding. The modifiers pressed before it already reached the window.
// debt: a combination of Alt with other keys can still make the window's menu bar flash on release.

use crate::engine::Emit;
use serde_json::json;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use svc_input::{Key, Sub, Verdict};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// A hold shorter than this does nothing.
pub const MIN_HOLD: Duration = Duration::from_millis(300);
/// A hook that lost a key release must not leave the microphone open for ever.
const MAX_HOLD: Duration = Duration::from_secs(300);

/// A listening engine. Either way the microphone is closed when this returns.
pub trait Take: Send {
    /// Stops listening and answers with everything that was said.
    fn finish(self: Box<Self>) -> String;
    fn cancel(self: Box<Self>);
}

pub struct Hooks {
    /// Opens the microphone and the engine behind it. `Err` is shown to the user.
    pub open: Box<dyn Fn() -> Result<Box<dyn Take>, String> + Send + Sync>,
    /// "hold" with `{ state: listening | working | idle | blocked }`, and "hold-error".
    pub emit: Emit,
}

// ── The combination ───────────────────────────────────────────────────────────

fn vk_of(name: &str) -> Option<u16> {
    Some(match name {
        "Ctrl" => 0x11,
        "Alt" => 0x12,
        "Shift" => 0x10,
        "Win" => 0x5B,
        "Space" => 0x20,
        n if n.len() == 1 && n.as_bytes()[0].is_ascii_alphanumeric() => n.to_ascii_uppercase().as_bytes()[0] as u16,
        n if n.starts_with('F') => 0x6F + n[1..].parse::<u16>().ok().filter(|f| (1..=24).contains(f))?,
        _ => return None,
    })
}

/// "Ctrl+Alt+Space" to its virtual keys (sides of the modifiers not told apart).
pub fn parse(chord: &str) -> Option<Vec<u16>> {
    let keys: Vec<u16> = chord.split('+').map(vk_of).collect::<Option<_>>()?;
    (!keys.is_empty()).then_some(keys)
}

fn norm(vk: u16) -> u16 {
    match vk {
        0xA0 | 0xA1 => 0x10,
        0xA2 | 0xA3 => 0x11,
        0xA4 | 0xA5 => 0x12,
        0x5C => 0x5B,
        v => v,
    }
}

// ── The hook side ─────────────────────────────────────────────────────────────

enum Msg {
    Begin(isize),
    End,
    Cancel,
    Quit,
}

struct Det {
    combo: Vec<u16>,
    down: Vec<u16>,
    trigger: Option<u16>,
    cancelled: bool,
    esc: bool,
}

impl Det {
    fn key(&mut self, k: &Key, tx: &mpsc::Sender<Msg>) -> Verdict {
        let v = norm(k.vk);
        if k.down {
            if let Some(trigger) = self.trigger {
                if k.vk == 0x1B {
                    self.esc = true;
                    if !self.cancelled {
                        self.cancelled = true;
                        let _ = tx.send(Msg::Cancel);
                    }
                    return Verdict::Swallow;
                }
                if self.combo.contains(&v) && !self.down.contains(&v) {
                    self.down.push(v);
                }
                return if v == trigger { Verdict::Swallow } else { Verdict::Pass };
            }
            if self.combo.contains(&v) {
                if !self.down.contains(&v) {
                    self.down.push(v);
                }
                if self.combo.iter().all(|c| self.down.contains(c)) {
                    self.trigger = Some(v);
                    let _ = tx.send(Msg::Begin(unsafe { GetForegroundWindow() }.0 as isize));
                    return Verdict::Swallow;
                }
            }
            return Verdict::Pass;
        }
        if k.vk == 0x1B && self.esc {
            self.esc = false;
            return Verdict::Swallow;
        }
        self.down.retain(|d| *d != v);
        if self.trigger.is_some() && self.combo.contains(&v) {
            let swallow = self.trigger == Some(v);
            self.trigger = None;
            if !std::mem::take(&mut self.cancelled) {
                let _ = tx.send(Msg::End);
            }
            return if swallow { Verdict::Swallow } else { Verdict::Pass };
        }
        Verdict::Pass
    }
}

/// A live subscription and its worker. Dropping it unsubscribes and ends the worker; an engine
/// still working on a phrase finishes it and the text is dropped.
pub struct Hold {
    _sub: Sub,
    tx: mpsc::Sender<Msg>,
}

impl Hold {
    pub fn new(combo: Vec<u16>, hooks: Arc<Hooks>) -> Hold {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || worker(rx, hooks));
        let det = Mutex::new(Det { combo, down: vec![], trigger: None, cancelled: false, esc: false });
        let txk = tx.clone();
        // Order 25: after the blocked shortcuts (20), before the remaps (30).
        let sub = svc_input::keyboard(25, false, move |k| match det.lock() {
            Ok(mut d) => d.key(k, &txk),
            Err(_) => Verdict::Pass,
        });
        Hold { _sub: sub, tx }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
    }
}

// ── The worker ────────────────────────────────────────────────────────────────

fn front() -> isize {
    unsafe { GetForegroundWindow() }.0 as isize
}

/// The field with the cursor holds a password (UI Automation). Unknown counts as no.
fn password_focused() -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(ui) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else { return false };
        ui.GetFocusedElement().and_then(|e| e.CurrentIsPassword()).map(|b| b.as_bool()).unwrap_or(false)
    }
}

/// Why nothing may be inserted into `into` now, if so.
fn refuse(into: isize) -> Option<&'static str> {
    if front() != into || into == 0 {
        Some("window")
    } else if svc_inject::is_elevated(into) {
        Some("elevated")
    } else if password_focused() {
        Some("password")
    } else {
        None
    }
}

fn state(hooks: &Hooks, s: &str) {
    (hooks.emit)("hold", json!({ "state": s }));
}

/// Waits for the end of the hold in progress. True when it ended with a release.
fn wait(rx: &mpsc::Receiver<Msg>, timeout: Duration) -> Option<bool> {
    match rx.recv_timeout(timeout) {
        Ok(Msg::End) => Some(true),
        Ok(Msg::Cancel | Msg::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => Some(false),
        Ok(Msg::Begin(_)) | Err(mpsc::RecvTimeoutError::Timeout) => None,
    }
}

fn worker(rx: mpsc::Receiver<Msg>, hooks: Arc<Hooks>) {
    loop {
        match rx.recv() {
            Ok(Msg::Begin(into)) => {
                if !session(&rx, &hooks, into) {
                    return;
                }
            }
            Ok(Msg::Quit) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// One hold, press to release. False when the worker is to end.
fn session(rx: &mpsc::Receiver<Msg>, hooks: &Hooks, into: isize) -> bool {
    if let Some(why) = refuse(into) {
        (hooks.emit)("hold", json!({ "state": "blocked", "why": why }));
        while wait(rx, Duration::from_secs(1)).is_none() {}
        return true;
    }
    let take = match (hooks.open)() {
        Ok(t) => t,
        Err(message) => {
            (hooks.emit)("hold-error", json!({ "message": message }));
            while wait(rx, Duration::from_secs(1)).is_none() {}
            return true;
        }
    };
    let t0 = Instant::now();
    let mut shown = false;
    let released = loop {
        if let Some(r) = wait(rx, Duration::from_millis(50)) {
            break r;
        }
        if !shown && t0.elapsed() >= MIN_HOLD {
            shown = true;
            state(hooks, "listening");
        }
        if t0.elapsed() > MAX_HOLD {
            break true;
        }
    };
    if !released || t0.elapsed() < MIN_HOLD {
        take.cancel();
        state(hooks, "idle");
        return true;
    }
    if let Some(why) = refuse(into) {
        take.cancel();
        (hooks.emit)("hold", json!({ "state": "blocked", "why": why }));
        state(hooks, "idle");
        return true;
    }
    state(hooks, "working");
    let text = take.finish();
    let text = text.trim();
    // The engine took a while: the window in front may have changed meanwhile.
    match (text.is_empty(), refuse(into)) {
        (true, _) => {}
        (false, Some(why)) => (hooks.emit)("hold", json!({ "state": "blocked", "why": why })),
        (false, None) => {
            if let Err(message) = svc_inject::paste_text(text) {
                (hooks.emit)("hold-error", json!({ "message": message }));
            }
        }
    }
    state(hooks, "idle");
    true
}
