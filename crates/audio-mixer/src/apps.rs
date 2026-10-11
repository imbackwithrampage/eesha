// The rows of the mixer: the audio sessions svc-audio reads, gathered by app.
// A browser plays from a helper process and an app with a web view from that
// web view's: each is billed to the program the user knows, the nearest one up
// the process tree that has a window.

use std::collections::{HashMap, HashSet};

use notch_core::win::Proc;
use serde::Serialize;
use svc_audio::Session;
use windows::core::{w, HSTRING};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};

/// The row of the system sounds.
pub const SYSTEM: &str = "system";

#[derive(Debug, Clone, Serialize)]
pub struct App {
    /// What a pin, the order and the hide list go by: the program's file name
    /// in lower case (not its folder: apps that update move to a new one).
    pub id: String,
    /// The program's own description. Empty for the system sounds: the page names them.
    pub name: String,
    pub volume: f32,
    pub mute: bool,
    pub playing: bool,
    pub system: bool,
    /// The program the icon is read from.
    #[serde(skip)]
    pub exe: String,
    /// The processes whose sessions the row sets.
    #[serde(skip)]
    pub pids: Vec<u32>,
}

/// What one reading leaves for the next: who a playing process belongs to
/// (None = this app itself, never listed) and what each program is called.
#[derive(Default)]
pub struct Known {
    owners: HashMap<u32, Option<String>>,
    names: HashMap<String, String>,
}

/// One row per app, by name; the system sounds last.
pub fn group(sessions: &[Session], known: &mut Known) -> Vec<App> {
    // The process list and the windows are read only when someone new plays.
    if sessions.iter().any(|s| !s.system && !known.owners.contains_key(&s.pid)) {
        let procs = notch_core::win::process_table();
        let windowed: HashSet<u32> = notch_core::win::top_windows().into_iter().map(|w| w.1).collect();
        for s in sessions.iter().filter(|s| !s.system) {
            known.owners.entry(s.pid).or_insert_with(|| owner(s, &procs, &windowed));
        }
    }
    known.owners.retain(|pid, _| sessions.iter().any(|s| s.pid == *pid));

    let mut apps: Vec<App> = Vec::new();
    for s in sessions {
        let exe = match (s.system, known.owners.get(&s.pid)) {
            (true, _) => String::new(),
            (false, Some(Some(exe))) => exe.clone(),
            _ => continue,
        };
        let id = if s.system { SYSTEM.to_owned() } else { file_name(&exe).to_lowercase() };
        match apps.iter_mut().find(|a| a.id == id) {
            Some(app) => {
                if !app.pids.contains(&s.pid) {
                    app.pids.push(s.pid);
                }
                app.volume = app.volume.max(s.volume);
                app.mute &= s.mute;
                app.playing |= s.active;
            }
            None => apps.push(App {
                name: if s.system { String::new() } else { known.name(&exe, &s.name) },
                id,
                volume: s.volume,
                mute: s.mute,
                playing: s.active,
                system: s.system,
                exe,
                pids: vec![s.pid],
            }),
        }
    }
    apps.sort_by_cached_key(|a| (a.system, a.name.to_lowercase(), a.id.clone()));
    apps
}

/// The program a playing process belongs to: itself when it has a window, else
/// the nearest of its parents (six at most) that has one. None = this app.
// ponytail: a parent that has ended may have left its process ID to a stranger,
// who is then billed; compare start times if that ever shows.
fn owner(s: &Session, procs: &HashMap<u32, Proc>, windowed: &HashSet<u32>) -> Option<String> {
    let mut found = s.pid;
    let mut at = s.pid;
    for _ in 0..=6 {
        if at == std::process::id() {
            return None;
        }
        if windowed.contains(&at) {
            found = at;
            break;
        }
        match procs.get(&at).map(|p| p.parent) {
            // The shell starts everything and is nobody's app.
            Some(parent) if procs.get(&parent).is_some_and(|p| !p.exe.eq_ignore_ascii_case("explorer.exe")) => at = parent,
            _ => break,
        }
    }
    let path = if found == s.pid { s.exe.clone() } else { svc_audio::process_path(found).unwrap_or_default() };
    // A process that will not say where it lives still has the name the process list gives it.
    Some(if path.is_empty() { procs.get(&found).map_or_else(|| format!("process {found}"), |p| p.exe.clone()) } else { path })
}

impl Known {
    /// What the program calls itself, else what it called its session, else its file name.
    fn name(&mut self, exe: &str, said: &str) -> String {
        if let Some(name) = self.names.get(exe) {
            return name.clone();
        }
        let name = description(exe)
            .filter(|d| !d.is_empty())
            // "@file,-id" is a resource the app never resolved.
            .or_else(|| (!said.is_empty() && !said.starts_with('@')).then(|| said.to_owned()))
            .unwrap_or_else(|| file_name(exe).trim_end_matches(".exe").trim_end_matches(".EXE").to_owned());
        self.names.insert(exe.to_owned(), name.clone());
        name
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// `FileDescription` of the program's version information: "Google Chrome", "Spotify".
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
