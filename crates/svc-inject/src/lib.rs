// Inject — a shared service (docs/ARCHITECTURE.md §8). Not a feature: the
// features that need it list this crate in their own Cargo.toml.
//
// Typing and pasting into the window in front, for the clipboard history,
// snippets and the command bar. Nothing here runs on its own: every call works
// on the caller's thread (call it from a blocking thread, it waits for keys to
// be released) and leaves nothing behind.
//
// Windows does not let a program send keys to one that runs as administrator
// while it does not itself: every call that would refuses first with
// `Err(ELEVATED)`, so the caller can say why nothing happened.

pub mod clip;

use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CONTROL, VK_LWIN,
    VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow, SetForegroundWindow,
    ShowWindow, SW_RESTORE,
};

/// The error of every call here when the window in front runs as administrator and the app does not.
pub const ELEVATED: &str = "target is elevated";

/// `dwExtraInfo` of every key this crate sends, so a keyboard hook of the app can tell them from the user's.
pub const INJECTED: usize = 0x6E74_6368;

// ── Who gets the keys ─────────────────────────────────────────────────────────

fn token_elevated(process: HANDLE) -> Option<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        read.ok().map(|_| elevation.TokenIsElevated != 0)
    }
}

/// The window belongs to a program that runs as administrator while the app
/// does not: keys sent to it would be dropped by Windows (UIPI).
pub fn is_elevated(hwnd: isize) -> bool {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() || token_elevated(GetCurrentProcess()) == Some(true) {
            return false;
        }
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // Not even its token can be looked at: it is above us.
            return true;
        };
        let elevated = token_elevated(process).unwrap_or(true);
        let _ = CloseHandle(process);
        elevated
    }
}

/// The window in front, refused when keys cannot or must not reach it.
fn target() -> Result<isize, String> {
    let hwnd = unsafe { GetForegroundWindow() }.0 as isize;
    if hwnd == 0 {
        return Err("There is no window in front to type into.".into());
    }
    if is_elevated(hwnd) {
        return Err(ELEVATED.into());
    }
    Ok(hwnd)
}

/// A window to come back to: the one that was in front before the app took the
/// keyboard (a search field, the command bar). `restore` puts it in front again
/// before anything is typed or pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForegroundGuard {
    hwnd: isize,
}

impl ForegroundGuard {
    /// The window in front right now.
    pub fn capture() -> Self {
        Self { hwnd: unsafe { GetForegroundWindow() }.0 as isize }
    }

    /// A window the caller remembered itself.
    pub fn of(hwnd: isize) -> Self {
        Self { hwnd }
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    pub fn elevated(&self) -> bool {
        is_elevated(self.hwnd)
    }

    /// Puts the window in front again and waits until it is. `Err(ELEVATED)`
    /// when it runs as administrator: nothing could be sent to it anyway.
    pub fn restore(&self) -> Result<(), String> {
        let hwnd = HWND(self.hwnd as *mut _);
        unsafe {
            if self.hwnd == 0 || !IsWindow(Some(hwnd)).as_bool() {
                return Err("The window to go back to is closed.".into());
            }
            if self.elevated() {
                return Err(ELEVATED.into());
            }
            if GetForegroundWindow() != hwnd {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                if !SetForegroundWindow(hwnd).as_bool() {
                    // Windows only lets the program in front hand the front on:
                    // joining its input queue for a moment makes us part of it.
                    let front = GetWindowThreadProcessId(GetForegroundWindow(), None);
                    let me = GetCurrentThreadId();
                    let _ = AttachThreadInput(me, front, true);
                    let _ = BringWindowToTop(hwnd);
                    let _ = SetForegroundWindow(hwnd);
                    let _ = AttachThreadInput(me, front, false);
                }
            }
            for _ in 0..40 {
                if GetForegroundWindow() == hwnd {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Err("That window could not be brought to the front.".into())
    }
}

// ── Keys ──────────────────────────────────────────────────────────────────────

/// Waits until Ctrl, Alt, Shift and Win are all up (the keys of the shortcut
/// that started this would otherwise join what is sent). False after 1.5 s.
pub fn wait_modifiers_up() -> bool {
    let held = || [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN].iter().any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } < 0);
    for _ in 0..100 {
        if !held() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    !held()
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    // The navigation block (arrows, Home, Delete…) and the Win keys are "extended" keys.
    if matches!(vk.0, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: INJECTED } },
    }
}

fn unit(code: u16, up: bool) -> INPUT {
    let flags = if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: code, dwFlags: flags, time: 0, dwExtraInfo: INJECTED },
        },
    }
}

/// Sends the keys to `into`, the window `ready` saw in front, and to no other:
/// if another window has come to the front, nothing (more) is sent.
fn send(into: isize, inputs: &[INPUT]) -> Result<(), String> {
    // In pieces: a long text in one call can overrun the receiver's queue.
    for piece in inputs.chunks(64) {
        if unsafe { GetForegroundWindow() }.0 as isize != into {
            return Err(MOVED.into());
        }
        if unsafe { SendInput(piece, std::mem::size_of::<INPUT>() as i32) } as usize != piece.len() {
            return Err("Windows did not take the keys (another program is blocking input).".into());
        }
        if inputs.len() > 64 {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

const MOVED: &str = "Another window came to the front. Nothing was sent.";

/// Released keys and a moment for the receiver to see them released. Answers
/// with the window that was in front when the call began: the keys are for it.
fn ready() -> Result<isize, String> {
    let into = target()?;
    if !wait_modifiers_up() {
        return Err("Release the keys you are holding and try again.".into());
    }
    std::thread::sleep(Duration::from_millis(60));
    if target()? != into {
        return Err(MOVED.into());
    }
    Ok(into)
}

/// Types `text` at the caret of the window in front, character by character
/// (any script, no layout involved). A line break is Enter.
pub fn type_text(text: &str) -> Result<(), String> {
    let into = ready()?;
    let mut inputs = Vec::with_capacity(text.len() * 2);
    for code in text.replace("\r\n", "\n").encode_utf16() {
        match code {
            0x0A => inputs.extend([key(VK_RETURN, false), key(VK_RETURN, true)]),
            0x09 => inputs.extend([key(VK_TAB, false), key(VK_TAB, true)]),
            _ => inputs.extend([unit(code, false), unit(code, true)]),
        }
    }
    send(into, &inputs)
}

/// One key of a chord, by the name Settings shows for it.
fn named(name: &str) -> Option<VIRTUAL_KEY> {
    let upper = name.to_ascii_uppercase();
    let code = match upper.as_str() {
        "CTRL" | "CONTROL" => 0x11,
        "SHIFT" => 0x10,
        "ALT" => 0x12,
        "WIN" | "SUPER" | "META" => 0x5B,
        "ENTER" | "RETURN" => 0x0D,
        "TAB" => 0x09,
        "ESC" | "ESCAPE" => 0x1B,
        "SPACE" => 0x20,
        "BACKSPACE" => 0x08,
        "DELETE" | "DEL" => 0x2E,
        "INSERT" => 0x2D,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        _ => {
            let bytes = upper.as_bytes();
            match bytes {
                [c] if c.is_ascii_alphanumeric() => *c as u16,
                [b'F', digits @ ..] => match std::str::from_utf8(digits).ok()?.parse::<u16>().ok()? {
                    n @ 1..=24 => 0x70 + n - 1,
                    _ => return None,
                },
                _ => return None,
            }
        }
    };
    Some(VIRTUAL_KEY(code))
}

/// Presses a chord in the window in front: `"Ctrl+V"`, `"Ctrl+Shift+Left"`,
/// `"Enter"`, `"F2"`. Keys go down in the order written and come up in reverse.
pub fn send_keys(chord: &str) -> Result<(), String> {
    let keys: Vec<VIRTUAL_KEY> =
        chord.split('+').map(|name| named(name.trim()).ok_or_else(|| format!("Unknown key in \"{chord}\"."))).collect::<Result<_, _>>()?;
    let into = ready()?;
    let presses: Vec<INPUT> =
        keys.iter().map(|k| key(*k, false)).chain(keys.iter().rev().map(|k| key(*k, true))).collect();
    send(into, &presses)
}

/// Ctrl+V into the window in front, for a clipboard the caller has just
/// filled. Nothing is pasted when the clipboard changed while the keys of the
/// shortcut were still held: it would be somebody else's content.
pub fn paste_clipboard() -> Result<(), String> {
    let filled = clip::sequence();
    let into = ready()?;
    if clip::sequence() != filled {
        return Err("The clipboard changed before the paste. Nothing was pasted.".into());
    }
    send(into, &[key(VK_CONTROL, false), key(VIRTUAL_KEY(0x56), false), key(VIRTUAL_KEY(0x56), true), key(VK_CONTROL, true)])
}

/// Pastes `text` into the window in front without leaving it on the clipboard:
/// what was there is put back half a second later. The transient copy is
/// marked so that no clipboard history records it (Windows' own included).
pub fn paste_text(text: &str) -> Result<(), String> {
    target()?;
    let before = {
        let clipboard = clip::open()?;
        let before = clipboard.snapshot();
        clipboard.empty();
        clipboard.set_text(text)?;
        clipboard.set_excluded()?;
        before
    };
    clip::mark_own();
    let ours = clip::sequence();
    let pasted = paste_clipboard();
    if pasted.is_ok() {
        // The receiver reads the clipboard when it gets the keys, not when we send them.
        std::thread::sleep(Duration::from_millis(500));
    }
    // Somebody copied meanwhile: theirs stays.
    if clip::sequence() == ours {
        if let Ok(clipboard) = clip::open() {
            clipboard.restore(&before);
        }
        clip::mark_own();
    }
    pasted
}
