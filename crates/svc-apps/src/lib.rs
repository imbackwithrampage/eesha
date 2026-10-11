// Apps — a shared service (docs/ARCHITECTURE.md §8). Not a feature: the
// features that need it list this crate in their own Cargo.toml. Nothing here
// runs on its own: every call works on the caller's thread and ends with it.
//
//   installed()            the programs on this PC (the three uninstall hives and the Store packages)
//   run(exe, args, ..)     one hidden program, its output as text while it runs, stoppable
//   strip_vt(text)         that text without terminal escapes
//   winget::exe()          where winget.exe is
//   winget::valid_id(id)   whether an id may stand on a command line
//   winget::table(text)    the rows of the table winget printed

pub mod winget;

use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::ApplicationModel::PackageSignatureKind;
use windows::Management::Deployment::PackageManager;
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE};
use windows::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject};
use windows::Win32::System::Registry::*;

// ── The installed programs ────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledApp {
    /// Stays the same across launches: `arp:<uninstall key>` or `msix:<package family name>`.
    pub key: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    /// The folder it lives in, when it says so.
    pub location: String,
    /// The uninstall command as the program registered it (empty for a Store package).
    pub uninstall: String,
    /// Installed for this user only: removing it needs no administrator.
    pub per_user: bool,
    /// A Store (MSIX) package.
    pub store: bool,
}

/// Every program Windows lists under "Installed apps", by name. Reads the
/// registry and asks the package manager each time (a few hundred ms): keep
/// the answer for as long as a page shows it.
pub fn installed() -> Vec<InstalledApp> {
    let mut out = Vec::new();
    const UNINSTALL: PCWSTR = w!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall");
    hive(HKEY_LOCAL_MACHINE, UNINSTALL, false, &mut out);
    hive(HKEY_LOCAL_MACHINE, w!(r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"), false, &mut out);
    hive(HKEY_CURRENT_USER, UNINSTALL, true, &mut out);
    let _ = msix(&mut out);
    out.sort_by_cached_key(|a| a.name.to_lowercase());
    // One program listed in two hives (32 and 64 bit, or twice by its installer) is one row.
    out.dedup_by(|b, a| a.name.eq_ignore_ascii_case(&b.name) && a.version == b.version && a.store == b.store);
    out
}

fn reg_text(key: HKEY, sub: PCWSTR, value: PCWSTR) -> String {
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // REG_EXPAND_SZ comes back expanded; anything that is not text, or longer than this, is "not said".
    let r = unsafe { RegGetValueW(key, sub, value, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut bytes)) };
    if r != ERROR_SUCCESS {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(bytes as usize / 2).saturating_sub(1)]).trim().to_string()
}

fn reg_flag(key: HKEY, sub: PCWSTR, value: PCWSTR) -> bool {
    let (mut n, mut bytes) = (0u32, 4u32);
    let r = unsafe { RegGetValueW(key, sub, value, RRF_RT_REG_DWORD, None, Some(&mut n as *mut u32 as *mut _), Some(&mut bytes)) };
    r == ERROR_SUCCESS && n != 0
}

fn hive(root: HKEY, path: PCWSTR, per_user: bool, out: &mut Vec<InstalledApp>) {
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(root, path, None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return;
    }
    let mut name = [0u16; 256];
    for i in 0.. {
        let mut len = name.len() as u32;
        match unsafe { RegEnumKeyExW(key, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) } {
            ERROR_SUCCESS => {}
            ERROR_NO_MORE_ITEMS => break,
            _ => continue,
        }
        let sub = PCWSTR(name.as_ptr());
        let text = |value| reg_text(key, sub, value);
        let display = text(w!("DisplayName"));
        // Not programs: parts of Windows, patches of another entry, updates.
        let patch = matches!(text(w!("ReleaseType")).as_str(), "Update" | "Hotfix" | "Security Update");
        if display.is_empty() || reg_flag(key, sub, w!("SystemComponent")) || !text(w!("ParentKeyName")).is_empty() || patch {
            continue;
        }
        out.push(InstalledApp {
            key: format!("arp:{}", String::from_utf16_lossy(&name[..len as usize])),
            name: display,
            version: text(w!("DisplayVersion")),
            publisher: text(w!("Publisher")),
            location: text(w!("InstallLocation")),
            uninstall: text(w!("UninstallString")),
            per_user,
            store: false,
        });
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
}

fn msix(out: &mut Vec<InstalledApp>) -> windows::core::Result<()> {
    for p in PackageManager::new()?.FindPackagesByUserSecurityId(&HSTRING::new())? {
        // Frameworks, language packs and what Windows itself is made of are not programs.
        let part = p.IsFramework().unwrap_or(true) || p.IsResourcePackage().unwrap_or(true) || p.IsBundle().unwrap_or(true);
        if part || p.SignatureKind().is_ok_and(|k| k == PackageSignatureKind::System) {
            continue;
        }
        let (Ok(id), Ok(name)) = (p.Id(), p.DisplayName()) else { continue };
        let name = name.to_string();
        if name.is_empty() || name.starts_with("ms-resource:") {
            continue;
        }
        let v = id.Version().unwrap_or_default();
        out.push(InstalledApp {
            key: format!("msix:{}", id.FamilyName().unwrap_or_default()),
            name,
            version: format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision),
            publisher: p.PublisherDisplayName().unwrap_or_default().to_string(),
            location: p.InstalledPath().unwrap_or_default().to_string(),
            uninstall: String::new(),
            per_user: true,
            store: true,
        });
    }
    Ok(())
}

// ── Running a program ─────────────────────────────────────────────────────────

pub struct Output {
    /// The exit code; None when the program was stopped.
    pub code: Option<i32>,
    /// Everything it printed, without terminal escapes (the first 4 MiB).
    pub text: String,
    pub cancelled: bool,
    /// Stopped for having said nothing for `silence`.
    pub timed_out: bool,
}

/// Holds the program and everything it starts, so that stopping it stops them all.
struct Job(HANDLE);

impl Job {
    fn of(child: &Child) -> Option<Job> {
        let job = Job(unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ok()?);
        unsafe { AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle() as _)) }.ok()?;
        Some(job)
    }
}

impl Drop for Job {
    // Closing the handle stops nothing: a program an installer opened at its end stays open.
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Runs `exe` with `args` and waits for it. No window, no shell (`args` is an
/// array and reaches the program as it is), no keyboard; what it prints on
/// either stream arrives in `on_text` as it comes, already without escapes.
/// It is stopped, with everything it started, when `cancel` turns true or when
/// it has printed nothing for `silence`.
pub fn run(exe: &Path, args: &[&str], silence: Duration, cancel: &AtomicBool, mut on_text: impl FnMut(&str)) -> Result<Output, String> {
    let (mut reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(writer.try_clone().map_err(|e| e.to_string())?).stderr(writer);
    let mut child = notch_core::win::no_console(&mut cmd).spawn().map_err(|e| e.to_string())?;
    drop(cmd); // our ends of the pipe: reading ends when the last program holding it does
    let job = Job::of(&child);

    let (tx, rx) = channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut out = Output { code: None, text: String::new(), cancelled: false, timed_out: false };
    let mut pending: Vec<u8> = Vec::new();
    let mut quiet = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(bytes) => {
                quiet = Instant::now();
                pending.extend(bytes);
                // A character cut in two by the pipe waits for its other half.
                let whole = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => pending.len(),
                };
                // ponytail: an escape cut in two by the pipe leaves a few of its characters behind.
                let text = strip_vt(&String::from_utf8_lossy(&pending[..whole]));
                pending.drain(..whole);
                on_text(&text);
                if out.text.len() < 4 << 20 {
                    out.text.push_str(&text);
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                out.cancelled = cancel.load(Ordering::SeqCst);
                out.timed_out = !out.cancelled && quiet.elapsed() >= silence;
                if out.cancelled || out.timed_out {
                    match &job {
                        Some(job) => unsafe {
                            let _ = TerminateJobObject(job.0, 1);
                        },
                        None => {
                            let _ = child.kill();
                        }
                    }
                    break;
                }
                // It ended, but something it left running still holds the pipe: quiet is the end.
                if quiet.elapsed() >= Duration::from_millis(600) && matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !out.cancelled && !out.timed_out {
        out.code = status.code();
    }
    Ok(out)
}

/// `text` without terminal escape sequences (colours, cursor moves, titles).
pub fn strip_vt(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final letter.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
