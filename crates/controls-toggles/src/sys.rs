//! The Windows side of the quick toggles. Every read asks Windows; nothing is remembered.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const PERSONALIZE: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
const ADVANCED: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced");

fn dword(sub: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut n = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, sub, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut n))
    };
    r.is_ok().then_some(v)
}

fn set_dword(sub: PCWSTR, name: PCWSTR, v: u32) -> Result<(), String> {
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, sub, name, REG_DWORD.0, Some(&v as *const u32 as *const _), 4)
            .ok()
            .map_err(|e| e.to_string())
    }
}

fn broadcast(what: PCWSTR) {
    unsafe {
        let mut out = 0usize;
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(what.0 as isize), SMTO_ABORTIFHUNG, 200, Some(&mut out));
    }
}

/// (apps are dark, system is dark). A missing value means light.
pub fn theme() -> (bool, bool) {
    let dark = |n| dword(PERSONALIZE, n).map(|v| v == 0).unwrap_or(false);
    (dark(w!("AppsUseLightTheme")), dark(w!("SystemUsesLightTheme")))
}

pub fn set_dark(on: bool) -> Result<(), String> {
    let v = if on { 0 } else { 1 };
    set_dword(PERSONALIZE, w!("AppsUseLightTheme"), v)?;
    set_dword(PERSONALIZE, w!("SystemUsesLightTheme"), v)?;
    broadcast(w!("ImmersiveColorSet"));
    Ok(())
}

pub fn hidden_shown() -> bool {
    dword(ADVANCED, w!("Hidden")) == Some(1)
}

pub fn set_hidden_shown(on: bool) -> Result<(), String> {
    set_dword(ADVANCED, w!("Hidden"), if on { 1 } else { 2 })?;
    broadcast(w!("ShellState"));
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

fn defview() -> Option<HWND> {
    unsafe {
        let view = |parent: HWND| FindWindowExW(Some(parent), None, w!("SHELLDLL_DefView"), PCWSTR::null()).ok();
        if let Some(v) = FindWindowW(w!("Progman"), PCWSTR::null()).ok().and_then(view) {
            return Some(v);
        }
        let mut prev: Option<HWND> = None;
        while let Ok(wk) = FindWindowExW(None, prev, w!("WorkerW"), PCWSTR::null()) {
            if let Some(v) = view(wk) {
                return Some(v);
            }
            prev = Some(wk);
        }
        None
    }
}

/// Whether the desktop's icon list is visible; None when the desktop window cannot be found.
pub fn icons_shown() -> Option<bool> {
    unsafe {
        let list = FindWindowExW(Some(defview()?), None, w!("SysListView32"), PCWSTR::null()).ok()?;
        Some(IsWindowVisible(list).as_bool())
    }
}

/// The undocumented "Show desktop icons" command of the desktop view (0x7402).
pub fn toggle_icons() -> Result<(), String> {
    let v = defview().ok_or("desktop window not found")?;
    unsafe { SendMessageW(v, WM_COMMAND, Some(WPARAM(0x7402)), None) };
    Ok(())
}

/// (items, bytes) in the Recycle Bin of every drive.
pub fn bin() -> Option<(i64, i64)> {
    let mut i = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
    unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut i).ok()? };
    Some((i.i64NumItems, i.i64Size))
}

/// Deletes for good. The caller has asked the user first.
pub fn bin_empty() -> Result<(), String> {
    unsafe {
        SHEmptyRecycleBinW(None, PCWSTR::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND).map_err(|e| e.to_string())
    }
}

pub fn lock() -> Result<(), String> {
    unsafe { windows::Win32::System::Shutdown::LockWorkStation().map_err(|e| e.to_string()) }
}

pub fn display_off() {
    unsafe {
        let _ = PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2));
    }
}

// ── Brightness: the laptop panel over WMI, by starting PowerShell. Only the fallback: while the
// displays-keepawake feature is on, the Controls slider calls that feature instead (index.ts). ──

fn ps(script: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let o = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// 0…1; None on a screen without WMI brightness (a desktop monitor).
pub fn brightness() -> Option<f32> {
    let s = ps("(Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightness -ErrorAction Stop | Select-Object -First 1).CurrentBrightness")?;
    Some(s.parse::<f32>().ok()? / 100.0)
}

pub fn set_brightness(f: f32) -> Result<(), String> {
    let n = (f.clamp(0.0, 1.0) * 100.0).round() as u32;
    ps(&format!("Invoke-CimMethod -InputObject (Get-CimInstance -Namespace root/wmi -ClassName WmiMonitorBrightnessMethods -ErrorAction Stop | Select-Object -First 1) -MethodName WmiSetBrightness -Arguments @{{Timeout=0;Brightness={n}}} -ErrorAction Stop | Out-Null"))
        .map(|_| ())
        .ok_or_else(|| "brightness could not be set".into())
}
