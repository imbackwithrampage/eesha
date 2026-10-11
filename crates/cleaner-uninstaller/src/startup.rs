// What starts with Windows, to read: the Run keys and the Startup folder, and
// which entries name a program that is gone. Nothing here changes an entry: a
// registry value does not come back from the Recycle Bin, so turning one off
// stays with Windows' own Startup apps page.
//
// And the mark of a start that did not finish (`begin`, `settled`), for the
// app to come up with its features off after two of them in a row. The rule
// follows the upstream `AppDelegate.beginStartupWatch` (Sources/Vorssaint/App/AppDelegate.swift).

use std::path::Path;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::*;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Entry {
    pub name: String,
    /// The command as it is registered, or the file in the Startup folder.
    pub command: String,
    /// "user" | "machine" | "folder"
    pub place: &'static str,
    /// It names a program by its whole path, and no file is there.
    pub missing: bool,
}

fn run_key(root: HKEY, path: PCWSTR, place: &'static str, out: &mut Vec<Entry>) {
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(root, path, None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return;
    }
    let mut name = [0u16; 512];
    for i in 0.. {
        let mut len = name.len() as u32;
        match unsafe { RegEnumValueW(key, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) } {
            ERROR_SUCCESS => {}
            ERROR_MORE_DATA => continue,
            _ => break,
        }
        let mut text = [0u16; 2048];
        let mut bytes = (text.len() * 2) as u32;
        // REG_EXPAND_SZ comes back with its %variables% filled in.
        if unsafe { RegGetValueW(key, PCWSTR::null(), PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, Some(text.as_mut_ptr() as *mut _), Some(&mut bytes)) } != ERROR_SUCCESS {
            continue;
        }
        let command = String::from_utf16_lossy(&text[..(bytes as usize / 2).saturating_sub(1)]).trim().to_string();
        // A bare name is found by Windows somewhere on its path: not for this file to judge.
        let missing = crate::uninstall::split(&command).is_some_and(|(exe, _)| exe.is_absolute() && !exe.exists());
        out.push(Entry { name: String::from_utf16_lossy(&name[..len as usize]), command, place, missing });
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
}

/// Every entry of the Run keys and of this user's Startup folder. Reads only.
pub fn list() -> Vec<Entry> {
    let mut out = Vec::new();
    const RUN: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
    run_key(HKEY_CURRENT_USER, RUN, "user", &mut out);
    run_key(HKEY_LOCAL_MACHINE, RUN, "machine", &mut out);
    run_key(HKEY_LOCAL_MACHINE, w!(r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run"), "machine", &mut out);
    if let Some(folder) = svc_fs::Base::StartMenu.path().map(|programs| programs.join("Startup")) {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.eq_ignore_ascii_case("desktop.ini") {
                // debt: a shortcut's target is not read, so one that points nowhere is not marked.
                out.push(Entry { name, command: entry.path().to_string_lossy().into_owned(), place: "folder", missing: false });
            }
        }
    }
    out
}

// ── A start that did not finish ───────────────────────────────────────────────

const MARK: &str = "starting";

/// The first thing at launch, with the app's config folder: counts this start
/// and says how many before it never reached `settled`. Two or more is a crash
/// loop: come up with the features off, so that the tray and Settings can be reached.
pub fn begin(dir: &Path) -> u32 {
    let mark = dir.join(MARK);
    let before = std::fs::read_to_string(&mark).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&mark, (before + 1u32).to_string());
    before
}

/// After twenty seconds of healthy running, and on a clean quit: this start finished.
pub fn settled(dir: &Path) {
    let _ = std::fs::write(dir.join(MARK), "0");
}
