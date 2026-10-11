// What the feature reads from Windows: how long the person has been idle, whether an app has the
// microphone open, and which program a window belongs to. Reads only, each on the caller's time.

use std::collections::HashMap;
use std::sync::Mutex;

use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId};

/// Seconds since the last key or pointer input of this session (`GetLastInputInfo`).
pub fn idle_secs() -> u64 {
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return 0;
    }
    (unsafe { GetTickCount() }.wrapping_sub(info.dwTime) / 1000) as u64
}

// ── The microphone ────────────────────────────────────────────────────────────
//
// Windows keeps, for its privacy page, one key per app that ever used the microphone: packaged
// apps directly under the store, desktop programs under `NonPackaged` (the path with `#` for `\`).
// While an app has the microphone open its `LastUsedTimeStop` is 0.

const STORE: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone");

fn open(parent: HKEY, path: PCWSTR) -> Option<HKEY> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(parent, path, None, KEY_READ, &mut key) }.is_ok().then_some(key)
}

fn subkeys(key: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        if unsafe { RegEnumKeyExW(key, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..len as usize]));
    }
    out
}

fn open_now(key: HKEY, app: &str) -> bool {
    let mut stop = u64::MAX;
    let mut size = 8u32;
    let read = unsafe {
        RegGetValueW(
            key,
            &HSTRING::from(app),
            w!("LastUsedTimeStop"),
            RRF_RT_REG_QWORD,
            None,
            Some(&mut stop as *mut u64 as *mut _),
            Some(&mut size),
        )
    };
    read.is_ok() && stop == 0
}

/// The apps that have the microphone open now ("Teams.exe"), this app left out: its own dictation
/// is not somebody's call.
pub fn mic_in_use() -> Vec<String> {
    let me = std::env::current_exe().ok().map(|p| p.to_string_lossy().replace('\\', "#").to_lowercase());
    let mut out = Vec::new();
    let Some(store) = open(HKEY_CURRENT_USER, STORE) else { return out };
    let mut scan = |key: HKEY| {
        for app in subkeys(key) {
            if open_now(key, &app) && me.as_deref() != Some(app.to_lowercase().as_str()) {
                out.push(app.rsplit('#').next().unwrap_or(&app).to_string());
            }
        }
    };
    scan(store);
    if let Some(loose) = open(store, w!("NonPackaged")) {
        scan(loose);
        unsafe { _ = RegCloseKey(loose) };
    }
    unsafe { _ = RegCloseKey(store) };
    out
}

// ── Which program ─────────────────────────────────────────────────────────────

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let read = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        read.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// `FileDescription` of the program's version information: "Google Chrome", "Spotify".
/// debt: the same reading as crates/audio-mixer/src/apps.rs; it belongs in notch_core::win.
fn description(exe: &str) -> Option<String> {
    let path = HSTRING::from(exe);
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut at = std::ptr::null_mut();
        let mut len = 0u32;
        // The languages the file has strings in; the first is the one it was written in.
        if !VerQueryValueW(data.as_ptr().cast(), w!("\\VarFileInfo\\Translation"), &mut at, &mut len).as_bool() || len < 4 {
            return None;
        }
        let [language, page] = (at as *const [u16; 2]).read_unaligned();
        let key = HSTRING::from(format!("\\StringFileInfo\\{language:04x}{page:04x}\\FileDescription"));
        if !VerQueryValueW(data.as_ptr().cast(), &key, &mut at, &mut len).as_bool() || at.is_null() {
            return None;
        }
        // Up to its end: some files give a length that runs on into the next entry.
        let text: Vec<u16> =
            (0..len as usize).map(|i| (at as *const u16).add(i).read_unaligned()).take_while(|c| *c != 0).collect();
        Some(String::from_utf16_lossy(&text).trim().to_owned())
    }
}

fn stem(file: &str) -> &str {
    let file = file.rsplit(['\\', '/']).next().unwrap_or(file);
    match file.rsplit_once('.') {
        Some((name, ext)) if ext.eq_ignore_ascii_case("exe") => name,
        _ => file,
    }
}

/// What a program calls itself, by its path: read from the file once.
static NAMES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// The program a window in front belongs to, as (key, name): ("chrome", "Google Chrome"). The key
/// is the file name, lower case, without ".exe". Never the window's title. `None` when Windows
/// will not say (a window of another account, the lock screen).
pub fn app_of(hwnd: isize, pid: u32, exe: &str) -> Option<(String, String)> {
    let mut pid = pid;
    // A Store app's frame belongs to a host every such app shares; the app is the window inside.
    if exe.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
        unsafe {
            if let Ok(core) = FindWindowExW(Some(HWND(hwnd as *mut _)), None, w!("Windows.UI.Core.CoreWindow"), None) {
                let mut inner = 0u32;
                GetWindowThreadProcessId(core, Some(&mut inner));
                if inner != 0 {
                    pid = inner;
                }
            }
        }
    }
    let path = process_path(pid).unwrap_or_else(|| exe.to_string());
    let key = stem(&path).to_lowercase();
    if key.is_empty() || key == "lockapp" {
        return None;
    }
    let mut names = NAMES.lock().unwrap();
    let name = names.get_or_insert_with(HashMap::new).entry(path.clone()).or_insert_with(|| {
        description(&path)
            .map(|d| stem(&d).to_string())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| stem(&path).to_string())
    });
    Some((key, name.clone()))
}
