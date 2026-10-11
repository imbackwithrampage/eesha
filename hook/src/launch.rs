//! `--launch`: starting the app when it is closed and an agent begins a turn.
//!
//! Opt-in, and off unless the user turned it on in Settings › Agents: the flag
//! is part of the hook command written into the agent's settings (with the
//! usual diff, backup and click), so it is the user's own configuration that
//! asks for it, never a payload.
//!
//! What is started is fixed: the app's own executable, whose path the app wrote
//! next to this relay (`app-path`, in the folder only the user can write, the
//! same one the relay itself is trusted from). It must be an absolute path to an
//! existing file with the app's own name. Nothing in an event can change it.
//!
//! Never blocking the agent: the app is spawned detached, with none of our
//! handles, and not waited for.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Written by the app beside the relay: where the app itself lives.
const APP_PATH_FILE: &str = "app-path";
/// Tells the app the relay started it, so it also leaves by itself afterwards.
const STARTED_BY_RELAY: &str = "--auto-launched";

/// Only the start of a turn starts the app: a session opening, a prompt sent.
pub fn starts(event: &str) -> bool {
    matches!(event, "SessionStart" | "UserPromptSubmit")
}

/// The app's executable as the app recorded it, if it can be trusted to be it.
fn target() -> Option<PathBuf> {
    let file = std::env::current_exe().ok()?.with_file_name(APP_PATH_FILE);
    let text = std::fs::read_to_string(file).ok()?;
    let path = PathBuf::from(text.trim());
    valid_target(&path, |p| p.is_file()).then_some(path)
}

/// An absolute path to an existing file named like the app: nothing else is run.
fn valid_target(path: &Path, is_file: impl Fn(&Path) -> bool) -> bool {
    let named = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(&format!("{}.exe", notch_brand::SLUG)));
    path.is_absolute() && named && is_file(path)
}

/// Starts the app and returns at once. Any failure is silence: the agent
/// carries on exactly as it would with the app closed.
pub fn app() {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS};

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    let Some(exe) = target() else { return };
    // The agent reads our stdout until it closes. A child holding a copy of it
    // would keep the agent waiting for as long as the app runs: our own three
    // handles are made non-inheritable before anything is spawned.
    unsafe {
        for handle in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            let _ = SetHandleInformation(HANDLE(handle), HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
        }
    }
    let _ = Command::new(exe)
        .arg(STARTED_BY_RELAY)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_apps_own_executable_is_ever_a_target() {
        let there = |_: &Path| true;
        let app = format!(r"C:\Users\me\AppData\Local\{0}\{0}.exe", notch_brand::SLUG);
        assert!(valid_target(Path::new(&app), there));
        assert!(valid_target(Path::new(&app.to_uppercase()), there));
        // Another program, a script, a relative path, a missing file: never run.
        for other in [
            r"C:\Windows\System32\cmd.exe",
            r"C:\Users\me\evil.bat",
            &format!(r"{}.exe", notch_brand::SLUG),
            &format!(r"..\{}.exe", notch_brand::SLUG),
            &format!(r"C:\Users\me\{}.exe.bat", notch_brand::SLUG),
            "",
        ] {
            assert!(!valid_target(Path::new(other), there), "{other}");
        }
        assert!(!valid_target(Path::new(&app), |_| false));
    }

    #[test]
    fn only_the_start_of_a_turn_starts_the_app() {
        assert!(starts("SessionStart"));
        assert!(starts("UserPromptSubmit"));
        for other in ["PermissionRequest", "PreToolUse", "Stop", "SessionEnd", "StatusLine", "sessionStart", ""] {
            assert!(!starts(other), "{other}");
        }
    }
}
