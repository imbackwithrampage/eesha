// Explorer extras. Two things, both off until the user turns them on:
//
//  - the Explorer selection: the files that are selected in the Explorer window
//    in front, read through the shell's own windows (IShellWindows → the active
//    view → its selection). Nothing is copied, nothing is clicked.
//  - paste an image as a file: Ctrl+V in an Explorer folder (or on the desktop),
//    when the clipboard holds a picture and no files, writes
//    "Pasted_Image_<date>_<time>.png" into that folder. Anywhere else, and when
//    the folder cannot be found, the Ctrl+V goes on as the user pressed it.
//
// The key hook decides with cheap checks only (window class, focused control,
// clipboard formats) and hands the work to a thread. It swallows Ctrl+V only
// when all of them say "a picture, into a folder list"; a rename box, the
// address bar and the search box are not a folder list, so they keep Ctrl+V.
// The Windows Terminal, browsers and editors are never looked at.
//
// debt: a bitmap without a PNG on the clipboard is written as .bmp (svc-shell).
// debt: Explorer windows showing This PC or the Recycle Bin have no folder: Ctrl+V goes on.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex};

use svc_input::{Key, Sub, Verdict};
use std::mem::ManuallyDrop;

use windows::core::Interface;
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED, IServiceProvider,
};
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::Shell::{
    IFolderView2, IPersistFolder2, IShellBrowser, IShellItem, IShellItemArray, IShellWindows, IWebBrowserApp,
    SHGetNameFromIDList, ShellWindows, SIGDN_FILESYSPATH, SID_STopLevelBrowser,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindowVisible, GUITHREADINFO,
};

const VK_V: u16 = 0x56;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

// ── The shell's view of an Explorer window ───────────────────────────────────

/// Runs `f` on a thread of its own with COM up, so the caller's thread is never touched.
fn sta<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    std::thread::spawn(move || {
        let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let out = f();
        if init.is_ok() {
            unsafe { CoUninitialize() };
        }
        out
    })
    .join()
    .ok()
}

/// The folder view of the Explorer window `top` (a top-level window). With
/// tabs, several shell windows share one `top`: the one whose view is showing.
unsafe fn view_of(top: isize) -> Option<IFolderView2> {
    unsafe {
        let windows: IShellWindows = windows::Win32::System::Com::CoCreateInstance(&ShellWindows, None, windows::Win32::System::Com::CLSCTX_ALL).ok()?;
        for i in 0..windows.Count().ok()? {
            let Ok(item) = windows.Item(&variant(i)) else { continue };
            let Ok(app) = item.cast::<IWebBrowserApp>() else { continue };
            if app.HWND().map(|h| h.0 as isize).unwrap_or(0) != top {
                continue;
            }
            let Ok(provider) = item.cast::<IServiceProvider>() else { continue };
            let Ok(browser) = provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
            let Ok(view) = browser.QueryActiveShellView() else { continue };
            let shown = view.GetWindow().map(|w| IsWindowVisible(w).as_bool()).unwrap_or(false);
            if let (true, Ok(folder_view)) = (shown, view.cast::<IFolderView2>()) {
                return Some(folder_view);
            }
        }
        None
    }
}

/// An integer VARIANT, which is how IShellWindows::Item takes its index.
fn variant(n: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 { vt: VT_I4, wReserved1: 0, wReserved2: 0, wReserved3: 0, Anonymous: VARIANT_0_0_0 { lVal: n } }),
        },
    }
}

fn path_of(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        path
    }
}

/// The files selected in the Explorer window in front. `Err` when it is not an Explorer window.
pub fn selection() -> Result<Vec<PathBuf>, String> {
    let front = unsafe { GetForegroundWindow() };
    if class_of(front) != "CabinetWClass" {
        return Err("Explorer is not in front.".into());
    }
    let top = front.0 as isize;
    sta(move || unsafe {
        let view = view_of(top).ok_or_else(|| "Could not read that Explorer window.".to_string())?;
        let items: IShellItemArray = view.GetSelection(false).map_err(|_| "Nothing is selected.".to_string())?;
        let mut out = Vec::new();
        for i in 0..items.GetCount().map_err(|e| e.to_string())? {
            if let Some(path) = items.GetItemAt(i).ok().and_then(|it| path_of(&it)) {
                out.push(path);
            }
        }
        Ok(out)
    })
    .unwrap_or_else(|| Err("Could not read that Explorer window.".into()))
}

/// The folder Explorer window `top` shows, or the desktop's.
fn folder_of(top: isize, desktop: bool) -> Option<PathBuf> {
    if desktop {
        return std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Desktop")).filter(|p| p.is_dir());
    }
    sta(move || unsafe {
        let view = view_of(top)?;
        let folder: IPersistFolder2 = view.GetFolder().ok()?;
        let pidl = folder.GetCurFolder().ok()?;
        let name = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH).ok();
        CoTaskMemFree(Some(pidl as *const _));
        let name = name?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        path.filter(|p| p.is_dir())
    })
    .flatten()
}

// ── Paste an image as a file ─────────────────────────────────────────────────

/// Window class of the thing that has the keyboard in `front`'s thread.
fn focus_class(front: HWND) -> String {
    unsafe {
        let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        let thread = GetWindowThreadProcessId(front, None);
        if GetGUIThreadInfo(thread, &mut info).is_ok() { class_of(info.hwndFocus) } else { String::new() }
    }
}

fn pressed(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) < 0 }
}

/// The clipboard holds a picture and no files. Formats only: nothing is read here.
fn picture_on_clipboard() -> bool {
    unsafe {
        let png = RegisterClipboardFormatW(windows::core::w!("PNG"));
        IsClipboardFormatAvailable(CF_HDROP).is_err()
            && (IsClipboardFormatAvailable(png).is_ok()
                || IsClipboardFormatAvailable(CF_DIBV5).is_ok()
                || IsClipboardFormatAvailable(CF_DIB).is_ok())
    }
}

/// What the hook knows about a Ctrl+V it wants: the Explorer window and whether it is the desktop.
fn wanted() -> Option<(isize, bool)> {
    if !pressed(0x11) || pressed(0x10) || pressed(0x12) {
        return None;
    }
    let front = unsafe { GetForegroundWindow() };
    let class = class_of(front);
    let desktop = matches!(class.as_str(), "Progman" | "WorkerW");
    if class != "CabinetWClass" && !desktop {
        return None;
    }
    if !matches!(focus_class(front).as_str(), "DirectUIHWND" | "SysListView32") || !picture_on_clipboard() {
        return None;
    }
    Some((front.0 as isize, desktop))
}

pub struct Paste {
    _sub: Sub,
    alive: Arc<AtomicBool>,
}

impl Drop for Paste {
    fn drop(&mut self) {
        self.alive.store(false, Relaxed);
    }
}

/// Starts watching for Ctrl+V. `done` gets the path of every file written.
pub fn watch_paste(done: impl Fn(PathBuf) + Send + 'static) -> Paste {
    let (jobs, work) = mpsc::channel::<(isize, bool)>();
    let alive = Arc::new(AtomicBool::new(true));
    let flag = alive.clone();
    std::thread::spawn(move || {
        while let Ok((top, desktop)) = work.recv() {
            if !flag.load(Relaxed) {
                break;
            }
            let t = notch_core::win::local_time();
            let name = format!("Pasted_Image_{:04}{:02}{:02}_{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second);
            let written = folder_of(top, desktop).and_then(|dir| {
                svc_shell::clipboard(&dir, &name).into_iter().find_map(|i| match i {
                    svc_shell::Item::File(p) => Some(p),
                    _ => None,
                })
            });
            match written {
                Some(path) => done(path),
                // Not ours after all: Ctrl+V goes on as it was pressed (Ctrl is still down).
                None => {
                    svc_input::inject_keys(&[(VK_V, true), (VK_V, false)]);
                }
            }
        }
    });
    let swallowed_press = Mutex::new(false);
    let jobs = Mutex::new(jobs);
    let sub = svc_input::keyboard(25, false, move |k: &Key| {
        if k.vk != VK_V || k.injected {
            return Verdict::Pass;
        }
        let mut held = swallowed_press.lock().unwrap_or_else(|e| e.into_inner());
        if !k.down {
            return if std::mem::take(&mut *held) { Verdict::Swallow } else { Verdict::Pass };
        }
        // Auto-repeat of a press that was taken.
        if *held {
            return Verdict::Swallow;
        }
        match wanted() {
            Some(job) => {
                *held = true;
                let _ = jobs.lock().unwrap_or_else(|e| e.into_inner()).send(job);
                Verdict::Swallow
            }
            None => Verdict::Pass,
        }
    });
    Paste { _sub: sub, alive }
}
