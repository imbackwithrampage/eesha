// An entry of the app's in Explorer's own menus: the right-click menu of files
// and folders, and "Send to". Both only start a program with the paths; what
// the program does with them is its business.
//
// The right-click entry is a verb under the user's own classes
// (HKEY_CURRENT_USER, no administrator needed): on Windows 11 it shows under
// "Show more options". The "Send to" entry is a shortcut in the user's SendTo
// folder. Nothing here runs unless it is called, and `remove` leaves nothing.

use std::path::{Path, PathBuf};

use windows::core::{Interface, Result as WinResult, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::{FOLDERID_SendTo, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT};

use crate::{said, sta, taken};

/// Where the entries live. `Places::user()` is the real thing; a probe names a
/// key and a folder of its own.
#[derive(Debug, Clone)]
pub struct Places {
    /// The classes key, under HKEY_CURRENT_USER: `Software\Classes`.
    pub classes: String,
    /// The folder "Send to" lists.
    pub send_to: PathBuf,
}

impl Places {
    /// The signed-in user's own menus.
    pub fn user() -> Option<Places> {
        let send_to = unsafe { SHGetKnownFolderPath(&FOLDERID_SendTo, KF_FLAG_DEFAULT, None).ok().map(|path| taken(path))? };
        Some(Places { classes: r"Software\Classes".into(), send_to: PathBuf::from(send_to) })
    }
}

/// One entry: "Add to Shelf", say.
#[derive(Debug, Clone)]
pub struct MenuEntry<'a> {
    /// The verb's key name: letters, digits, `.`, `-` and `_`.
    pub id: &'a str,
    /// What the menus show.
    pub label: &'a str,
    /// The program started…
    pub exe: &'a Path,
    /// …with this before the paths: `"<exe>" <arg> "<path>"`.
    pub arg: &'a str,
}

/// The two kinds of thing the entry is offered on: any file, any folder.
const ON: [&str; 2] = ["*", "Directory"];

fn verb_key(places: &Places, on: &str, id: &str) -> String {
    format!(r"{}\{on}\shell\{id}", places.classes)
}

/// An id that cannot name anything but one key of ours (an empty one would name `shell` itself).
fn checked(id: &str) -> Result<(), String> {
    let fine = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if fine { Ok(()) } else { Err(format!("\"{id}\" cannot be the name of a menu entry.")) }
}

fn win32(code: WIN32_ERROR) -> Result<(), String> {
    if code == ERROR_SUCCESS { Ok(()) } else { Err(said(windows::core::Error::from_hresult(code.to_hresult()))) }
}

unsafe fn set(key: &str, name: Option<&str>, value: &str) -> Result<(), String> {
    unsafe {
        let mut open = HKEY::default();
        win32(RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut open,
            None,
        ))?;
        let bytes: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        let name = name.map(HSTRING::from).unwrap_or_default();
        let done = RegSetValueExW(open, &name, None, REG_SZ, Some(&bytes));
        let _ = RegCloseKey(open);
        win32(done)
    }
}

unsafe fn read(key: &str) -> Option<String> {
    unsafe {
        let mut text = [0u16; 1024];
        let mut size = (text.len() * 2) as u32;
        let got = RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&mut size),
        );
        (got == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&text[..text.iter().position(|c| *c == 0).unwrap_or(0)]))
    }
}

fn shortcut(places: &Places, label: &str) -> PathBuf {
    places.send_to.join(format!("{}.lnk", crate::safe_name(label)))
}

/// Puts the entry in both menus. Calling it again writes the same thing again.
pub fn add_to_menus(entry: &MenuEntry, places: &Places) -> Result<(), String> {
    checked(entry.id)?;
    // A label that changed (another language) leaves its old shortcut otherwise.
    remove_from_menus(entry.id, places)?;
    let exe = entry.exe.to_string_lossy();
    unsafe {
        for on in ON {
            let key = verb_key(places, on, entry.id);
            set(&key, None, entry.label)?;
            set(&key, Some("Icon"), &format!("\"{exe}\",0"))?;
            // Up to a hundred selected items rather than fifteen; the program is still started once for each.
            set(&key, Some("MultiSelectModel"), "Player")?;
            set(&format!(r"{key}\command"), None, &format!("\"{exe}\" {} \"%1\"", entry.arg))?;
        }
    }
    let link = shortcut(places, entry.label);
    let (target, arg) = (HSTRING::from(entry.exe.as_os_str()), HSTRING::from(entry.arg));
    sta(|| unsafe {
        // "Send to" starts the target once, with every selected path after its arguments.
        let made: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        made.SetPath(&target)?;
        made.SetArguments(&arg)?;
        made.SetIconLocation(&target, 0)?;
        made.cast::<IPersistFile>()?.Save(&HSTRING::from(link.as_os_str()), true)
    })
    .map_err(said)
}

/// Takes the entry out of both menus. Fine when it was not there.
pub fn remove_from_menus(id: &str, places: &Places) -> Result<(), String> {
    checked(id)?;
    unsafe {
        // The shortcut is named after the label the entry was added with.
        if let Some(label) = read(&verb_key(places, ON[0], id)) {
            let _ = std::fs::remove_file(shortcut(places, &label));
        }
        for on in ON {
            match RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(verb_key(places, on, id))) {
                ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => {}
                other => win32(other)?,
            }
        }
    }
    Ok(())
}

/// The command line the right-click entry runs, when it is there.
pub fn menu_command(id: &str, places: &Places) -> Option<String> {
    checked(id).ok()?;
    unsafe { read(&format!(r"{}\command", verb_key(places, ON[0], id))) }
}

/// Where a "Send to" shortcut leads: the program and its arguments. For a
/// caller that wants to see what `add_to_menus` wrote.
pub fn shortcut_target(link: &Path) -> Result<(PathBuf, String), String> {
    let link = HSTRING::from(link.as_os_str());
    sta(|| unsafe {
        let made: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        made.cast::<IPersistFile>()?.Load(&link, Default::default())?;
        let (mut path, mut args) = ([0u16; 1024], [0u16; 1024]);
        made.GetPath(&mut path, std::ptr::null_mut(), 0)?;
        made.GetArguments(&mut args)?;
        let text = |buffer: &[u16]| String::from_utf16_lossy(&buffer[..buffer.iter().position(|c| *c == 0).unwrap_or(0)]);
        WinResult::Ok((PathBuf::from(text(&path)), text(&args)))
    })
    .map_err(said)
}
