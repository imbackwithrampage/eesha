// What the probe needs to put windows of its own on the screen and act on
// them as a person would: a window, the message loop, one click, and a
// description of whatever window is in front.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{w, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{GetStockObject, DKGRAY_BRUSH, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GetAncestor, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    MsgWaitForMultipleObjects, PeekMessageW, RegisterClassW, SetCursorPos, TranslateMessage, WindowFromPoint, GA_ROOT,
    MSG, PM_REMOVE, QS_ALLINPUT, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

pub fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

pub fn ours(hwnd: HWND) -> bool {
    pid_of(hwnd) == unsafe { GetCurrentProcessId() }
}

pub fn class_of(hwnd: HWND) -> String {
    let mut name = [0u16; 96];
    let len = unsafe { GetClassNameW(hwnd, &mut name) };
    String::from_utf16_lossy(&name[..len.max(0) as usize])
}

pub fn title_of(hwnd: HWND) -> String {
    let mut name = [0u16; 128];
    let len = unsafe { GetWindowTextW(hwnd, &mut name) };
    String::from_utf16_lossy(&name[..len.max(0) as usize])
}

fn exe_of(hwnd: HWND) -> String {
    unsafe {
        let pid = pid_of(hwnd);
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return format!("pid {pid}") };
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(name.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        if ok.is_err() {
            return format!("pid {pid}");
        }
        String::from_utf16_lossy(&name[..len as usize]).rsplit('\\').next().unwrap_or_default().to_string()
    }
}

pub fn rect_of(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    r
}

/// Class, program, size and place. The title only of a window of the probe's
/// own: what another program has in its title bar is none of the probe's business.
pub fn describe(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return "(none)".into();
    }
    let r = rect_of(hwnd);
    let place = format!("{}x{} at {},{}", r.right - r.left, r.bottom - r.top, r.left, r.top);
    if ours(hwnd) {
        format!("[ours: \"{}\" {place}]", title_of(hwnd))
    } else {
        format!("[{} of {} {place}]", class_of(hwnd), exe_of(hwnd))
    }
}

unsafe extern "system" fn list(hwnd: HWND, out: LPARAM) -> BOOL {
    unsafe {
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut c_void, 4);
        if IsWindowVisible(hwnd).as_bool() && cloaked == 0 {
            (*(out.0 as *mut Vec<isize>)).push(hwnd.0 as isize);
        }
        true.into()
    }
}

/// Every top-level window that is on screen now.
pub fn visible() -> Vec<isize> {
    let mut out: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(list), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A plain grey window of the probe's own.
pub fn window(title: PCWSTR, ex: WINDOW_EX_STYLE, style: WINDOW_STYLE, at: (i32, i32), size: (i32, i32)) -> HWND {
    unsafe {
        let module = GetModuleHandleW(None).expect("module");
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: module.into(),
            lpszClassName: w!("shell-probe.window"),
            hbrBackground: HBRUSH(GetStockObject(DKGRAY_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(ex, w!("shell-probe.window"), title, style, at.0, at.1, size.0, size.1, None, None, Some(module.into()), None)
            .expect("window")
    }
}

/// Runs this thread's messages for `ms`, saying each time another window comes to the front.
pub fn pump(ms: u64) {
    unsafe {
        let until = Instant::now() + Duration::from_millis(ms);
        let mut front = GetForegroundWindow();
        loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let now = GetForegroundWindow();
            if now != front {
                front = now;
                println!("    in front now: {}", describe(now));
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            MsgWaitForMultipleObjects(None, false, left.as_millis().min(30) as u32, QS_ALLINPUT);
        }
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } } }
}

/// A real click in the middle of a window of the probe's own, the pointer put
/// back afterwards. Nothing is clicked when anything else is at that point.
pub fn click(hwnd: HWND) -> bool {
    unsafe {
        let r = rect_of(hwnd);
        let at = POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
        if GetAncestor(WindowFromPoint(at), GA_ROOT) != hwnd {
            println!("    (another window is at that point: not clicked)");
            return false;
        }
        let mut old = POINT::default();
        let _ = GetCursorPos(&mut old);
        let _ = SetCursorPos(at.x, at.y);
        SendInput(&[mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)], size_of::<INPUT>() as i32);
        pump(250);
        let _ = SetCursorPos(old.x, old.y);
        true
    }
}
