// Close protection: Alt+F4 and Ctrl+W ask twice.
//
// The rules follow the upstream QuitProtectionSupport.swift and the service
// around it: the shortcut is held back and counts only when it is held for a
// while or pressed again in time; the key repeating while it is held is never
// a second press; the release of a press that was held back is held back too;
// Esc, or letting go early, cancels.
//
// One keyboard subscription on the shared hub while a guard is on. A plain key
// leaves it after one comparison. What the confirmed shortcut does is the
// listener's business (the app closes the window in front; the probe closes
// its own).

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use svc_input::{Key, Verdict};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    AltF4,
    CtrlW,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The shortcut was held back: keep holding, or press again.
    Asked(Which),
    Cancelled(Which),
    /// Held long enough, or pressed twice: now it counts.
    Confirmed(Which),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The guarded keys by virtual key: F4 (0x73) and W (0x57) in the app.
    pub keys: Vec<(u16, Which)>,
    /// Hold for `ms`; otherwise press again within `ms`.
    pub hold: bool,
    pub ms: u64,
    /// Keys another program sends are guarded as well (the probe's are).
    pub injected: bool,
}

/// Alt, Ctrl, Shift and the Windows key, as they are held now.
pub type Mods = fn() -> [bool; 4];

pub fn held() -> [bool; 4] {
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
    [down(0x12), down(0x11), down(0x10), down(0x5B) || down(0x5C)]
}

/// The shortcut exactly: with Shift or another modifier it is another shortcut.
fn wanted(which: Which, mods: [bool; 4]) -> bool {
    mods == match which {
        Which::AltF4 => [true, false, false, false],
        Which::CtrlW => [false, true, false, false],
    }
}

#[derive(Default)]
struct State {
    /// The guarded key that is down, its press held back.
    down: Option<u16>,
    /// What waits to be confirmed, and when it was pressed (microseconds).
    asked: Option<(Which, u64)>,
    /// Counts the waits: a hold that was let go is not confirmed by its own timer.
    turn: u64,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Close protection, from `start` until this is dropped.
pub struct Guard {
    state: Arc<Mutex<State>>,
    _sub: svc_input::Sub,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut st = lock(&self.state);
        st.turn += 1;
        st.asked = None;
    }
}

/// `on` is never called on the hook's thread: it may close windows and talk to the page.
pub fn start(config: Config, mods: Mods, on: impl Fn(Event) + Send + Sync + 'static) -> Guard {
    let on: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(on);
    let state = Arc::new(Mutex::new(State::default()));
    let shared = state.clone();
    let tell = {
        let on = on.clone();
        move |event: Event| {
            let on = on.clone();
            std::thread::spawn(move || on(event));
        }
    };
    // After the shortcuts blocked per app (20), before the remaps (30). It
    // sends keys of its own: skipped while a window that runs as administrator
    // is in front, which it could not close anyway.
    let sub = svc_input::keyboard(25, true, move |key: &Key| {
        let guarded = config.keys.iter().find(|k| k.0 == key.vk).map(|k| k.1);
        if guarded.is_none() && key.vk != 0x1B {
            // Every other key: one look, no lock.
            return Verdict::Pass;
        }
        let mut st = lock(&shared);
        if !key.down {
            if st.down != Some(key.vk) {
                return Verdict::Pass;
            }
            st.down = None;
            if config.hold {
                // Let go before the time was up.
                if let Some((which, _)) = st.asked.take() {
                    st.turn += 1;
                    tell(Event::Cancelled(which));
                }
            }
            return Verdict::Swallow;
        }
        if st.down == Some(key.vk) {
            // The key repeating while it is held.
            return Verdict::Swallow;
        }
        if key.vk == 0x1B {
            if let Some((which, _)) = st.asked.take() {
                st.turn += 1;
                tell(Event::Cancelled(which));
            }
            return Verdict::Pass;
        }
        let Some(which) = guarded else { return Verdict::Pass };
        if (key.injected && !config.injected) || !wanted(which, mods()) {
            return Verdict::Pass;
        }
        st.down = Some(key.vk);
        if which == Which::AltF4 {
            // Alt let go with no key after it opens the window's menu: an unassigned key in between.
            svc_input::inject_keys(&[(0xE8, true), (0xE8, false)]);
        }
        if config.hold {
            st.turn += 1;
            let turn = st.turn;
            st.asked = Some((which, key.t));
            let (state, on, ms) = (shared.clone(), on.clone(), config.ms);
            std::thread::spawn(move || {
                on(Event::Asked(which));
                std::thread::sleep(Duration::from_millis(ms));
                {
                    let mut st = lock(&state);
                    if st.turn != turn || st.asked.is_none() {
                        return;
                    }
                    st.asked = None;
                }
                // Alt or Ctrl let go meanwhile is not a hold.
                on(if wanted(which, mods()) { Event::Confirmed(which) } else { Event::Cancelled(which) });
            });
        } else {
            match st.asked.take() {
                Some((first, at)) if first == which && key.t.saturating_sub(at) <= config.ms * 1000 => tell(Event::Confirmed(which)),
                _ => {
                    st.asked = Some((which, key.t));
                    tell(Event::Asked(which));
                }
            }
        }
        Verdict::Swallow
    });
    Guard { state, _sub: sub }
}
