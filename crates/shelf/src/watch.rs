// Opens the shelf while something is dragged toward the island (shelf.drag-detect).
//
// Windows has no drag pasteboard to look at, so "a content drag is on its way"
// is inferred (shelf.drag-detect):
// the left button is held, the pointer left the system's
// drag box, no window is being moved or sized, and either the shell's drag
// image is up (`SysDragImage`, an undocumented class name) or the press landed
// in an Explorer file view. A wrong guess only opens the shelf.
//
// One low-level mouse hook on a thread of its own, there only while the
// feature runs with "dragDetect" on. The hook reads the buttons and where the
// pointer is, keeps nothing, and does no work at all unless a button is held.
// debt: its own WH_MOUSE_LL until svc-input exists (batch 4), then a subscriber of that.
// ponytail: left button only; a mouse with swapped buttons (SM_SWAPBUTTON) is not followed.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread::JoinHandle;

use notch_core::Ctx;
use tauri::{AppHandle, Manager};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, FindWindowW, GetAncestor, GetClassNameW, GetGUIThreadInfo, GetMessageW, GetSystemMetrics,
    GetWindowRect, GetWindowThreadProcessId, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WindowFromPoint, GA_ROOT, GUITHREADINFO, GUI_INMOVESIZE, MSG, MSLLHOOKSTRUCT, SM_CXDRAG, SM_CYDRAG, WH_MOUSE_LL,
    WM_APP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT,
};

use crate::ID;

/// The band under the top edge, centred on the island, that opens the shelf (logical px).
const ZONE_W: i32 = 640;
const ZONE_H: i32 = 120;

const NEAR: u32 = WM_APP + 1;
const END: u32 = WM_APP + 2;

/// The press being followed. Lives on the hook's thread.
struct Gesture {
    app: AppHandle,
    press: Option<POINT>,
    /// Where the shelf opens for this press, once it turned into a drag. Empty: never.
    zone: Option<RECT>,
    /// The press landed in an Explorer file view.
    explorer: bool,
    /// When the drag was last tested (ms, the hook's clock).
    tested: u32,
    /// The shelf was told to open.
    fired: bool,
}

thread_local! {
    static GESTURE: RefCell<Option<Gesture>> = const { RefCell::new(None) };
}

pub struct Watch {
    thread: u32,
    join: Option<JoinHandle<()>>,
}

/// Installs the hook. None when Windows refuses it.
pub fn start(cx: Ctx) -> Option<Watch> {
    let (ready, started) = mpsc::channel();
    let join = std::thread::spawn(move || unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).ok().map(Into::into);
        let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), module, 0) {
            Ok(hook) => hook,
            Err(err) => {
                notch_core::log::line(format!("shelf: no drag watcher: {err}"));
                let _ = ready.send(0);
                return;
            }
        };
        GESTURE.set(Some(Gesture {
            app: cx.app().clone(),
            press: None,
            zone: None,
            explorer: false,
            tested: 0,
            fired: false,
        }));
        let _ = ready.send(GetCurrentThreadId());
        // The hook is called from inside this loop; what it found is told from here, not from the hook.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            match msg.message {
                NEAR if !cx.paused() => cx.emit(ID, "drag-near", ()),
                END => cx.emit(ID, "drag-end", ()),
                _ => {}
            }
        }
        let _ = UnhookWindowsHookEx(hook);
        GESTURE.set(None);
    });
    match started.recv() {
        Ok(thread) if thread != 0 => Some(Watch { thread, join: Some(join) }),
        _ => None,
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

unsafe extern "system" fn on_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        GESTURE.with_borrow_mut(|gesture| {
            if let Some(gesture) = gesture {
                gesture.on(wparam.0 as u32, info);
            }
        });
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn tell(message: u32) {
    unsafe {
        let _ = PostThreadMessageW(GetCurrentThreadId(), message, WPARAM(0), LPARAM(0));
    }
}

fn class_of(window: HWND) -> String {
    let mut name = [0u16; 64];
    let len = unsafe { GetClassNameW(window, &mut name) };
    String::from_utf16_lossy(&name[..len.max(0) as usize])
}

impl Gesture {
    fn on(&mut self, message: u32, info: &MSLLHOOKSTRUCT) {
        match message {
            WM_LBUTTONDOWN => {
                self.press = Some(info.pt);
                self.zone = None;
                self.fired = false;
            }
            WM_LBUTTONUP => {
                if self.fired {
                    tell(END);
                }
                self.press = None;
                self.fired = false;
            }
            WM_MOUSEMOVE => {
                // Every move of the mouse passes here: nothing held, nothing done.
                let Some(press) = self.press else { return };
                if self.fired {
                    return;
                }
                let at = info.pt;
                let zone = match self.zone {
                    Some(zone) => zone,
                    None => {
                        let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
                        if (at.x - press.x).abs() <= dx && (at.y - press.y).abs() <= dy {
                            return;
                        }
                        let zone = self.zone_for(press);
                        self.zone = Some(zone);
                        zone
                    }
                };
                if at.x < zone.left || at.x >= zone.right || at.y < zone.top || at.y >= zone.bottom {
                    return;
                }
                if info.time.wrapping_sub(self.tested) < 50 {
                    return;
                }
                self.tested = info.time;
                if self.content_drag() {
                    self.fired = true;
                    tell(NEAR);
                }
            }
            _ => {}
        }
    }

    /// The band this press may open the shelf in, in screen pixels; empty when
    /// the press is not one to follow (our own windows, a taskbar button).
    fn zone_for(&mut self, press: POINT) -> RECT {
        let none = RECT::default();
        unsafe {
            let source = WindowFromPoint(press);
            let root = GetAncestor(source, GA_ROOT);
            let mut pid = 0u32;
            GetWindowThreadProcessId(root, Some(&mut pid));
            if pid == GetCurrentProcessId() || class_of(root) == "Shell_TrayWnd" {
                return none;
            }
            self.explorer = matches!(class_of(source).as_str(), "DirectUIHWND" | "SysListView32" | "SysTreeView32");

            // Through the raw handle: Tauri's HWND is another version of the `windows` crate's.
            let Some(island) = self.app.get_webview_window("island").and_then(|w| w.hwnd().ok()) else { return none };
            let island = HWND(island.0 as *mut _);
            let mut at = RECT::default();
            if GetWindowRect(island, &mut at).is_err() {
                return none;
            }
            // The island window sits centred on the top edge of its display.
            let scale = GetDpiForWindow(island).max(96) as i32;
            let middle = (at.left + at.right) / 2;
            let half = ZONE_W * scale / 96 / 2;
            RECT { left: middle - half, right: middle + half, top: at.top, bottom: at.top + ZONE_H * scale / 96 }
        }
    }

    fn content_drag(&self) -> bool {
        unsafe {
            let mut gui = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            if GetGUIThreadInfo(0, &mut gui).is_ok() && gui.flags.contains(GUI_INMOVESIZE) {
                return false;
            }
            self.explorer || FindWindowW(w!("SysDragImage"), PCWSTR::null()).is_ok()
        }
    }
}
