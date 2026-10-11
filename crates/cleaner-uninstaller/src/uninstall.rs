// Removing a program: the command that does it, and what it left behind.
//
// The command is the one the program registered with Windows, cut into a
// program and its arguments and never handed to a shell; or winget's own
// `uninstall`. The page shows it before anything runs. This file starts
// nothing itself.
//
// What is left behind is found by name alone, so only the strongest evidence
// counts: a folder called exactly what the program was called, in the places
// programs keep their data, and not when another installed program could own
// it. `token`, `without_version`, the ignored names and the exact tier follow
// the upstream file Sources/Vorssaint/Services/Uninstall/UninstallerSupport.swift
// (`normalizedToken`, `nameWithoutTrailingVersion`, `ignoredLastComponents`, `leftoverMatch`).

use std::path::{Path, PathBuf};

use svc_apps::InstalledApp;
use svc_fs::{Base, Found, Rule};

// ── The command ───────────────────────────────────────────────────────────────

/// A program and its arguments: what is shown is what is started.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub exe: PathBuf,
    pub args: Vec<String>,
}

impl Command {
    /// As one line, for the eye.
    pub fn shown(&self) -> String {
        let quote = |text: &str| if text.contains(' ') { format!("\"{text}\"") } else { text.to_string() };
        std::iter::once(quote(&self.exe.to_string_lossy())).chain(self.args.iter().map(|a| quote(a))).collect::<Vec<_>>().join(" ")
    }
}

/// A registered command line as a program and its arguments. Quoted, the
/// program is what the quotes hold; unquoted, a path may hold spaces, so it is
/// the shortest beginning that ends in ".exe" and is a file (or, when none is,
/// the shortest that ends in ".exe").
pub fn split(line: &str) -> Option<(PathBuf, Vec<String>)> {
    let line = line.trim();
    let (exe, rest) = match line.strip_prefix('"') {
        Some(quoted) => {
            let end = quoted.find('"')?;
            (&quoted[..end], &quoted[end + 1..])
        }
        None => {
            // Same length as `line`, so a place in one is the same place in the other.
            let lower = line.to_ascii_lowercase();
            let ends: Vec<usize> = lower.match_indices(".exe").map(|(at, _)| at + 4).filter(|&end| line[end..].chars().next().is_none_or(char::is_whitespace)).collect();
            match ends.iter().find(|&&end| Path::new(&line[..end]).is_file()).or(ends.first()) {
                Some(&end) => (&line[..end], &line[end..]),
                // "MsiExec /X{…}": a bare word.
                None => line.split_once(char::is_whitespace).unwrap_or((line, "")),
            }
        }
    };
    if exe.trim().is_empty() {
        return None;
    }
    let (mut args, mut word, mut quoted) = (Vec::new(), String::new(), false);
    for c in rest.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    args.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        args.push(word);
    }
    Some((PathBuf::from(exe.trim()), args))
}

/// Why a program's own uninstaller is not started from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// A Store app: it has no command of its own.
    Store,
    /// Nothing registered, or a line that does not name a program.
    Unreadable,
    /// The line runs a shell or a script host, which is never started from here.
    Shell,
    /// The uninstaller it names is gone.
    Missing,
}

impl Why {
    pub fn code(self) -> &'static str {
        match self {
            Why::Store => "store",
            Why::Unreadable => "unreadable",
            Why::Shell => "shell",
            Why::Missing => "missing",
        }
    }
}

/// The program's own registered uninstaller.
pub fn own_command(app: &InstalledApp) -> Result<Command, Why> {
    if app.store {
        return Err(Why::Store);
    }
    let (mut exe, mut args) = split(&app.uninstall).ok_or(Why::Unreadable)?;
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if matches!(stem.as_str(), "cmd" | "powershell" | "pwsh" | "wscript" | "cscript" | "mshta" | "rundll32" | "regsvr32") {
        return Err(Why::Shell);
    }
    if !exe.is_absolute() {
        // Windows Installer is the one program registered by its bare name.
        if stem != "msiexec" {
            return Err(Why::Unreadable);
        }
        exe = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())).join(r"System32\msiexec.exe");
    }
    if stem == "msiexec" {
        // Registered as "change" (/I): removing is /X of the same product.
        for arg in &mut args {
            if arg.get(..2).is_some_and(|flag| flag.eq_ignore_ascii_case("/i")) && arg[2..].starts_with('{') {
                *arg = format!("/X{}", &arg[2..]);
            }
        }
    }
    if !exe.is_file() {
        return Err(Why::Missing);
    }
    Ok(Command { exe, args })
}

/// winget's `uninstall` for the program: it starts the same uninstaller, and
/// lets Windows ask for administrator rights when that one needs them.
pub fn winget_command(app: &InstalledApp, winget: Option<&Path>) -> Option<Command> {
    // One word on a command line that winget cannot take for an option.
    let plain = |text: &str| !text.is_empty() && text.len() <= 200 && !text.starts_with(['-', '/']) && !text.chars().any(|c| c.is_control() || c == '"');
    let select = if app.store {
        // winget knows a Store app by its full name, which is the name of the folder it lives in.
        let full = Path::new(&app.location).file_name()?.to_str()?;
        ["--id".to_string(), format!("MSIX\\{full}")]
    } else {
        ["--product-code".to_string(), app.key.strip_prefix("arp:")?.to_string()]
    };
    if !plain(&select[1]) {
        return None;
    }
    let mut args = vec!["uninstall".to_string()];
    args.extend(select);
    args.extend(["--exact", "--accept-source-agreements", "--disable-interactivity"].map(String::from));
    Some(Command { exe: winget?.to_path_buf(), args })
}

// ── What it left behind ───────────────────────────────────────────────────────

/// Where programs keep what an uninstaller forgets. Text of this file: the
/// program's name only ever picks among what is found there.
pub static PLACES: &[Rule] = &[
    Rule { base: Base::Roaming, root: "", pattern: "*" },
    Rule { base: Base::Roaming, root: "", pattern: "*/*" },
    Rule { base: Base::Local, root: "", pattern: "*" },
    Rule { base: Base::Local, root: "", pattern: "*/*" },
    Rule { base: Base::LocalLow, root: "", pattern: "*" },
    Rule { base: Base::LocalLow, root: "", pattern: "*/*" },
    Rule { base: Base::Local, root: "Programs", pattern: "*" },
    Rule { base: Base::StartMenu, root: "", pattern: "*" },
    Rule { base: Base::Desktop, root: "", pattern: "*.lnk" },
];

/// Lowercase letters and digits only: "Example App 2" and "exampleapp2" are the same name.
pub fn token(raw: &str) -> String {
    raw.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// "Example 4.2 (x64)" is "Example": a version, a channel or a bracket at the end is not part of the name.
pub fn without_version(raw: &str) -> &str {
    const CHANNELS: [&str; 11] = ["nightly", "beta", "alpha", "dev", "canary", "preview", "insider", "stable", "release", "rc", "lts"];
    let mut name = raw.trim();
    loop {
        let cut = if name.ends_with(')') {
            name.rfind('(')
        } else {
            name.rfind(char::is_whitespace).filter(|&at| {
                let last = name[at..].trim().to_lowercase();
                let digits = last.trim_start_matches('v');
                CHANNELS.contains(&last.as_str()) || (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '.'))
            })
        };
        match cut {
            Some(at) if at > 0 => name = name[..at].trim(),
            _ => return name,
        }
    }
}

/// Names too common to belong to one program.
const NOBODY: [&str; 62] = [
    "app", "helper", "agent", "daemon", "service", "desktop", "client", "launcher", "plugin", "extension", "web", "free", "pro", "lite", "plus", "ui", "login", "updater",
    "installer", "renderer", "gpu", "worker", "crashpad", "broker", "utility", "alert", "network", "audio", "server", "shared", "core", "common", "framework", "bundle",
    "process", "handler", "tool", "runtime", "electron", "java", "python", "node", "mono", "wine", "microsoft", "windows", "google", "adobe", "apple", "intel", "amd",
    "nvidia", "temp", "cache", "caches", "logs", "packages", "programs", "data", "update", "setup", "system",
];

/// The names a program goes by: its whole name, and its name without the version.
fn names(app: &InstalledApp) -> Vec<String> {
    let mut out: Vec<String> = [app.name.as_str(), without_version(&app.name)].iter().map(|n| token(n)).filter(|t| t.chars().count() >= 3 && !NOBODY.contains(&t.as_str())).collect();
    out.dedup();
    out
}

/// The folders and shortcuts under `places` called exactly what `app` was
/// called. `still` is what Windows lists as installed now: a name another
/// program shares (the same name, a longer one beginning with it, or its
/// publisher) is nobody's to give away, and nothing is offered for it.
/// Reads only: the finds are for the user to tick.
pub fn leftovers(app: &InstalledApp, still: &[InstalledApp], places: &[Rule]) -> Vec<Found> {
    let mine: Vec<String> = names(app)
        .into_iter()
        .filter(|name| !still.iter().filter(|other| other.key != app.key).any(|other| names(other).iter().any(|n| n.starts_with(name.as_str())) || token(&other.publisher) == *name))
        .collect();
    if mine.is_empty() {
        return Vec::new();
    }
    let publisher = token(&app.publisher);
    // A folder's whole name ("Example 2.0" has no extension); a shortcut's without ".lnk".
    let named = |path: &Path| {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        token(name.strip_suffix(".lnk").unwrap_or(&name))
    };
    let mut found: Vec<Found> = Vec::new();
    for rule in places {
        let nested = rule.pattern.contains('/');
        let matches = svc_fs::find(rule, |path| {
            let name = named(path);
            if !nested {
                // "Example", or the folder its updater keeps: "example-updater".
                return mine.iter().any(|m| name == *m || name.strip_suffix("updater") == Some(m.as_str()));
            }
            // "Vendor\Example": under its publisher, or the two together are its name ("Mozilla\Firefox").
            let parent = path.parent().map(named).unwrap_or_default();
            mine.iter().any(|m| (name == *m && !publisher.is_empty() && parent == publisher) || format!("{parent}{name}") == *m)
        });
        for item in matches {
            // A shortcut is a file; anything else a program leaves is a folder.
            let shortcut = item.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
            if (item.is_dir() || shortcut) && !found.iter().any(|f| item.path().starts_with(f.path())) {
                found.push(item);
            }
        }
    }
    found
}
