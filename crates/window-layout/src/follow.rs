// Focus follows the pointer: the window the pointer rests on comes to the front.
//
// Windows has a setting of its own for this (active window tracking). The app
// leaves it alone and does the work itself on the shared mouse hook, so nothing
// of the system is changed and there is nothing to put back.
//
// The hook only notes where the pointer is and when it moved. A thread, asleep
// until the pointer moves, counts the rest down and brings the window forward;
// it ends with the subscription.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use svc_input::{MouseKind, Verdict};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, IsWindowEnabled};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetGUIThreadInfo, GetWindowLongW, WindowFromPoint, GA_ROOT, GA_ROOTOWNER, GUITHREADINFO,
    GUI_INMENUMODE, GUI_INMOVESIZE, GUI_POPUPMENUMODE, GUI_SYSTEMMENUMODE, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use crate::layout::{class_of, is_shell};

/// The mouse buttons, Shift, Ctrl, Alt and the two Windows keys.
const HELD: [i32; 8] = [0x01, 0x02, 0x04, 0x10, 0x11, 0x12, 0x5B, 0x5C];

/// Windows of the shell that close when another window comes to the front
/// (Start, search, the notification centre, the tray's overflow, a menu).
const FLYOUTS: [&str; 5] =
    ["Windows.UI.Core.CoreWindow", "XamlExplorerHostIslandWindow", "TopLevelWindowForOverflowXamlIsland", "NotifyIconOverflowWindow", "#32768"];

/// The window to bring forward for a pointer resting at this point, if any.
/// None while it is the one in front already, while a button or a modifier is
/// held, while the window in front has a menu open or is being moved, over the
/// desktop and the taskbar, over a panel that never takes the keyboard (the
/// island), and over a window that waits for a dialog of its own.
pub fn under(x: i32, y: i32) -> Option<isize> {
    unsafe {
        let hit = WindowFromPoint(POINT { x, y });
        let root = GetAncestor(hit, GA_ROOT);
        let front = GetForegroundWindow();
        // A list or a popup that belongs to the window in front is that window's business.
        if root.is_invalid() || root == front || GetAncestor(hit, GA_ROOTOWNER) == front {
            return None;
        }
        if HELD.iter().any(|key| GetAsyncKeyState(*key) < 0) {
            return None;
        }
        let mut gui = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        let busy = GUI_INMENUMODE | GUI_INMOVESIZE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE;
        if GetGUIThreadInfo(0, &mut gui).is_ok() && (gui.flags & busy).0 != 0 {
            return None;
        }
        // debt: the shell's flyouts are known by their class names.
        if FLYOUTS.contains(&class_of(front.0 as isize).as_str()) {
            return None;
        }
        let ex = GetWindowLongW(root, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) != 0 || !IsWindowEnabled(root).as_bool() || is_shell(root.0 as isize) {
            return None;
        }
        // On another desktop, or a store app that is asleep: there, but not to be seen.
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(root, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        (cloaked == 0).then_some(root.0 as isize)
    }
}

struct Shared {
    x: AtomicI32,
    y: AtomicI32,
    /// When the pointer last moved, on the hub's clock (microseconds).
    moved: AtomicU64,
    /// The thread is counting a rest down: it needs no second wake.
    counting: AtomicBool,
    stopped: AtomicBool,
}

/// Focus follows the pointer, from `start` until this is dropped.
pub struct Follow {
    shared: Arc<Shared>,
    _sub: svc_input::Sub,
}

impl Drop for Follow {
    fn drop(&mut self) {
        // A rest being counted down brings nothing forward any more.
        self.shared.stopped.store(true, Relaxed);
    }
}

/// `delay_ms`: how long the pointer rests before its window comes forward.
/// `allow` has the last word on a window (in the probe: its own windows only).
pub fn start(delay_ms: u64, allow: impl Fn(isize) -> bool + Send + 'static) -> Follow {
    let shared = Arc::new(Shared {
        x: AtomicI32::new(0),
        y: AtomicI32::new(0),
        moved: AtomicU64::new(0),
        counting: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
    });
    let (wake, woken) = mpsc::channel::<()>();
    let hook = shared.clone();
    // Last of the mouse stages: it only watches, and only what nobody swallowed.
    let sub = svc_input::mouse(50, false, move |m| {
        if m.kind == MouseKind::Move {
            hook.x.store(m.x, Relaxed);
            hook.y.store(m.y, Relaxed);
            hook.moved.store(m.t, Relaxed);
            // One wake for a whole movement.
            if !hook.counting.swap(true, Relaxed) {
                let _ = wake.send(());
            }
        }
        Verdict::Pass
    });
    let state = shared.clone();
    std::thread::spawn(move || {
        let delay = delay_ms * 1000;
        // Asleep until the pointer moves; over when the subscription, which holds the other end, is gone.
        while woken.recv().is_ok() {
            loop {
                let rested = svc_input::now().saturating_sub(state.moved.load(Relaxed));
                if rested >= delay {
                    break;
                }
                std::thread::sleep(Duration::from_micros(delay - rested));
            }
            state.counting.store(false, Relaxed);
            if state.stopped.load(Relaxed) {
                break;
            }
            if let Some(hwnd) = under(state.x.load(Relaxed), state.y.load(Relaxed)).filter(|hwnd| allow(*hwnd)) {
                // A window that runs as administrator says no: nothing to do about it.
                svc_windows::activate(hwnd);
            }
        }
    });
    Follow { shared, _sub: sub }
}
