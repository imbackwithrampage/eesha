// What the actions need of the PC's files: the Start Menu's programs for "open <app>". Filled by
// package voice-actions-b (VOICE §10; read §5, §8). Opening goes through `svc-shell`; nothing here
// deletes, moves or writes a file of the user's.
//
// The safety: `open` takes a path and starts it only when that path is one of the shortcuts `apps`
// lists right now (a `.lnk` in a Start Menu folder). A command line, a folder, an `.exe` anywhere
// else, `..` and a shortcut that uninstalls are all refused, whatever was heard.

use crate::{Result, VoiceError};
use notch_core::Ctx;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const DEPTH: usize = 5;

pub fn start(_cx: &Ctx) -> Result<()> {
    Ok(())
}

pub fn stop(_cx: &Ctx) {}

pub fn settings(_cx: &Ctx, _settings: &Value) {}

/// The two folders Windows reads the Start Menu's programs from: all users', then this user's.
pub fn roots() -> Vec<PathBuf> {
    let tail = Path::new("Microsoft").join("Windows").join("Start Menu").join("Programs");
    ["ProgramData", "APPDATA"].iter().filter_map(|v| std::env::var_os(v)).map(|b| Path::new(&b).join(&tail)).collect()
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        // `file_type` does not follow a link: a junction cannot lead out of the folder.
        let Ok(kind) = e.file_type() else { continue };
        let path = e.path();
        if kind.is_dir() {
            if depth < DEPTH {
                walk(&path, depth + 1, out);
            }
        } else if kind.is_file() && path.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
            let name = path.file_stem().map(|s| s.to_string_lossy().trim().to_string()).unwrap_or_default();
            // A shortcut that removes a program is not "an app to open".
            if !name.is_empty() && !name.to_lowercase().contains("uninstall") {
                out.push((name, path));
            }
        }
    }
}

/// `[(name, path)]` of the shortcuts under `roots`, by name; a name in both folders is one row.
pub fn list_in(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for r in roots {
        walk(r, 0, &mut out);
    }
    out.sort_by_cached_key(|(n, p)| (n.to_lowercase(), p.clone()));
    out.dedup_by(|b, a| a.0.eq_ignore_ascii_case(&b.0));
    out
}

/// The listed shortcut that `path` names, or nothing: the only way to a program from heard words.
pub fn find_in(roots: &[PathBuf], path: &str) -> Option<PathBuf> {
    let want = path.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    // Not `list_in`: a name in both folders is one row there, but either shortcut was offered.
    let mut all = Vec::new();
    for r in roots {
        walk(r, 0, &mut all);
    }
    all.into_iter().map(|(_, p)| p).find(|p| p.to_string_lossy().to_lowercase() == want)
}

/// `apps` → `[{name, path}]` (the shortcuts of both Start Menu folders, for the parser's `choice` list;
/// read when asked, not watched) · `open {path}` → `{}` (a path `apps` gave, nothing else).
pub fn call(_cx: &Ctx, method: &str, args: Value) -> Result<Value> {
    match method {
        "apps" => Ok(Value::Array(list_in(&roots()).into_iter().map(|(name, path)| json!({ "name": name, "path": path.to_string_lossy() })).collect())),
        "open" => {
            let path = find_in(&roots(), args["path"].as_str().unwrap_or_default()).ok_or_else(|| VoiceError::Refused("That is not a program of the Start Menu.".into()))?;
            svc_shell::open(&path.to_string_lossy()).map_err(VoiceError::Failed)?;
            Ok(json!({}))
        }
        _ => Err(VoiceError::Failed(format!("files.{method}: unknown method"))),
    }
}
