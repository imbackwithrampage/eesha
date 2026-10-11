// The Win32 side: displays, the pointer, capture exclusion, opening a target.

use windows::core::{BOOL, HSTRING};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowDisplayAffinity, SetWindowDisplayAffinity, SW_SHOWNORMAL, WDA_NONE,
    WINDOW_DISPLAY_AFFINITY,
};

const WDA_EXCLUDEFROMCAPTURE: WINDOW_DISPLAY_AFFINITY = WINDOW_DISPLAY_AFFINITY(0x11);

#[derive(Debug, Clone)]
pub struct Disp {
    /// Opaque monitor handle, equal for the same display.
    pub id: isize,
    /// Physical pixels: left, top, right, bottom.
    pub rect: (i32, i32, i32, i32),
    pub scale: f64,
    pub primary: bool,
}

unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
    let v = &mut *(out.0 as *mut Vec<Disp>);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(m, &mut mi).as_bool() {
        let (mut dx, mut dy) = (96, 96);
        let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let r = mi.rcMonitor;
        v.push(Disp { id: m.0 as isize, rect: (r.left, r.top, r.right, r.bottom), scale: dx as f64 / 96.0, primary: mi.dwFlags & 1 != 0 });
    }
    BOOL(1)
}

pub fn displays() -> Vec<Disp> {
    let mut v: Vec<Disp> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

/// The pointer in physical pixels; read only.
pub fn pointer() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

pub fn display_at(x: i32, y: i32) -> isize {
    unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST).0 as isize }
}

pub fn display_of_window(hwnd: isize) -> isize {
    unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST).0 as isize }
}

/// True keeps the window out of screenshots and recordings (Windows 10 2004+).
pub fn set_capture_excluded(hwnd: isize, on: bool) -> Result<(), String> {
    let a = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut _), a).map_err(|e| e.to_string()) }
}

pub fn capture_excluded(hwnd: isize) -> Option<bool> {
    let mut a = 0u32;
    unsafe { GetWindowDisplayAffinity(HWND(hwnd as *mut _), &mut a).ok()? };
    Some(a == WDA_EXCLUDEFROMCAPTURE.0)
}

/// Opens a file, folder, link or program the way Explorer would.
pub fn open(target: &str) -> Result<(), String> {
    let r = unsafe { ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(target), None, None, SW_SHOWNORMAL) };
    if r.0 as isize > 32 { Ok(()) } else { Err(format!("could not open {target}")) }
}
