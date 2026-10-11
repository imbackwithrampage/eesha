// The island's own keys while the pointer is over it (docs/ARCHITECTURE.md §4).
//
// The island is then not the active window: the user's own window keeps the
// keyboard. A low-level keyboard hook reads the keys instead. It swallows
// exactly the combinations the page asked for (Esc, Ctrl+K, Alt+letter, what
// the section on screen uses) and hands them to the page; every other key goes
// on, untouched, to the window the user types in.
//
// The hook exists only while there is something for it to do: the page gave a
// set of keys (the island is open) and the pointer is over the island. It
// lives on a thread of its own that does nothing else, so a busy main thread
// never delays anyone's typing, and Windows removes it with that thread
// however the app ends. Each key is checked again as it arrives: nothing is
// swallowed unless the pointer is over the island at that moment and the
// island's window is up.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{mpsc, Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter};

use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use ::windows::Win32::System::Threading::GetCurrentThreadId;
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowRect, IsWindowVisible,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    WM_QUIT,
};

use crate::island::WINDOW_LABEL;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut core::ffi::c_void;
}

/// A combination as the page and the hook name it: modifiers above, the
/// virtual key below.
pub const CTRL: u32 = 1 << 16;
pub const ALT: u32 = 2 << 16;
pub const SHIFT: u32 = 4 << 16;
const WIN: u32 = 8 << 16;

/// A key no keyboard has and no program reads. Sent after a swallowed
/// combination: the window in front has seen the modifier go down, and a
/// modifier released with nothing in between is a command of its own (Alt
/// opens the menu bar, Ctrl shows where the pointer is, Shift switches the
/// input language). With this key in between it is none of them.
const MASK_KEY: u16 = 0xE8;
const LLKHF_INJECTED: u32 = 0x10;
const LLKHF_UP: u32 = 0x80;

/// What the page wants swallowed right now. Empty: nothing, and no hook.
static KEYS: RwLock<Vec<u32>> = RwLock::new(Vec::new());
/// The pointer is over the island, as the cursor poll last saw it.
static OVER: AtomicBool = AtomicBool::new(false);
/// The island's window.
static ISLAND: AtomicIsize = AtomicIsize::new(0);
/// The hook thread's id while it runs.
static THREAD: Mutex<Option<u32>> = Mutex::new(None);
/// Keys whose press was swallowed: their release is swallowed too.
static HELD: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Once, at launch: where the keys go and which window is the island.
pub fn install(app: &AppHandle, island: HWND) {
    let _ = APP.set(app.clone());
    ISLAND.store(island.0 as isize, Ordering::Relaxed);
}

/// The page says which combinations are the island's at this moment.
pub fn set_keys(keys: Vec<u32>) {
    *KEYS.write().unwrap() = keys;
    sync();
}

/// The cursor poll says whether the pointer is over the island. Called on
/// every tick; only a change does anything.
pub fn pointer(over: bool) {
    if OVER.swap(over, Ordering::Relaxed) != over {
        sync();
    }
}

/// The app is quitting: nothing more is swallowed.
pub fn remove() {
    set_keys(Vec::new());
}

/// Starts the hook when it is wanted and not there, ends it when it is there
/// and not wanted.
fn sync() {
    let wanted = OVER.load(Ordering::Relaxed) && !KEYS.read().unwrap().is_empty();
    let mut thread = THREAD.lock().unwrap();
    match (*thread, wanted) {
        (None, true) => {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || run(tx));
            // The thread answers once its hook is in (or could not be).
            *thread = rx.recv().ok().flatten();
        }
        (Some(id), false) => {
            *thread = None;
            // Its message loop ends, it takes the hook out and exits.
            unsafe {
                let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        _ => {}
    }
}

/// The hook thread: the hook, a message loop for Windows to call it through,
/// and nothing else.
fn run(ready: mpsc::Sender<Option<u32>>) {
    unsafe {
        let module = HINSTANCE(GetModuleHandleW(std::ptr::null()));
        let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), Some(module), 0) else {
            crate::log::line("island keys: the keyboard hook could not be installed");
            let _ = ready.send(None);
            return;
        };
        let _ = ready.send(Some(GetCurrentThreadId()));
        let mut msg = MSG::default();
        // False on WM_QUIT, -1 on an error: either way the hook goes.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
        let _ = UnhookWindowsHookEx(hook);
    }
    HELD.lock().unwrap().clear();
}

unsafe extern "system" fn on_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if swallow(key.vkCode, key.flags.0) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// True when this key event is the island's: it goes to the page and no further.
fn swallow(vk: u32, flags: u32) -> bool {
    // Our own mask key goes through, to the window it is meant for.
    if vk == MASK_KEY as u32 && flags & LLKHF_INJECTED != 0 {
        return false;
    }
    if flags & LLKHF_UP != 0 {
        // The release of a key whose press never reached the window in front.
        let mut held = HELD.lock().unwrap();
        let was = held.len();
        held.retain(|k| *k != vk);
        return held.len() != was;
    }
    if !live() {
        return false;
    }
    let down = |key: VIRTUAL_KEY| unsafe { GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 != 0 };
    let mut combo = vk;
    if down(VK_CONTROL) {
        combo |= CTRL;
    }
    if down(VK_MENU) {
        combo |= ALT;
    }
    if down(VK_SHIFT) {
        combo |= SHIFT;
    }
    if down(VK_LWIN) || down(VK_RWIN) {
        combo |= WIN;
    }
    if !wanted(&KEYS.read().unwrap(), combo) {
        return false;
    }
    let repeat = {
        let mut held = HELD.lock().unwrap();
        let repeat = held.contains(&vk);
        if !repeat {
            held.push(vk);
        }
        repeat
    };
    if combo != vk && !repeat {
        mask();
    }
    if let Some(app) = APP.get() {
        let _ = app.emit_to(WINDOW_LABEL, "island-key", combo);
    }
    true
}

/// A combination is the island's only when it is in the set as it stands:
/// the same key with one modifier more or less is somebody else's.
fn wanted(keys: &[u32], combo: u32) -> bool {
    keys.contains(&combo)
}

/// The island's window is up, the pointer is over the island, and the island
/// is not itself the window in front (its keys then reach it the usual way).
fn live() -> bool {
    if !OVER.load(Ordering::Relaxed) {
        return false;
    }
    let island = HWND(ISLAND.load(Ordering::Relaxed) as *mut _);
    unsafe {
        if island.is_invalid() || !IsWindowVisible(island).as_bool() || GetForegroundWindow() == island {
            return false;
        }
        // Should the cursor poll ever stop telling: the pointer must be inside
        // the island's window right now, whatever was last said.
        let (mut at, mut window) = (POINT::default(), RECT::default());
        if GetCursorPos(&mut at).is_err() || GetWindowRect(island, &mut window).is_err() {
            return false;
        }
        at.x >= window.left && at.x < window.right && at.y >= window.top && at.y < window.bottom
    }
}

/// Presses and releases the mask key (see MASK_KEY).
fn mask() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(MASK_KEY), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    };
    unsafe {
        SendInput(&[key(Default::default()), key(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_combination_is_the_islands() {
        let (esc, k, m) = (0x1B, 0x4B, 0x4D);
        let keys = [esc, CTRL | k, ALT | m];
        assert!(wanted(&keys, esc));
        assert!(wanted(&keys, CTRL | k));
        assert!(wanted(&keys, ALT | m));
        // A plain letter, the same letter with another modifier, AltGr (Ctrl+Alt) and the Windows key.
        assert!(!wanted(&keys, k));
        assert!(!wanted(&keys, CTRL | SHIFT | k));
        assert!(!wanted(&keys, CTRL | ALT | m));
        assert!(!wanted(&keys, WIN | esc));
        assert!(!wanted(&[], esc));
    }

    #[test]
    fn nothing_is_swallowed_while_the_pointer_is_elsewhere() {
        // No pointer over the island, whatever the page asked for.
        OVER.store(false, Ordering::Relaxed);
        *KEYS.write().unwrap() = vec![0x1B];
        assert!(!swallow(0x1B, 0));
        // A release is swallowed only when its press was.
        assert!(!swallow(0x1B, LLKHF_UP));
        HELD.lock().unwrap().push(0x1B);
        assert!(swallow(0x1B, LLKHF_UP));
        assert!(!swallow(0x1B, LLKHF_UP));
        // The mask key we send ourselves always goes through.
        assert!(!swallow(MASK_KEY as u32, LLKHF_INJECTED));
        KEYS.write().unwrap().clear();
    }
}
