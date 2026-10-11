// Reset and uninstall: built to show exactly what will go, and to go only after
// a click. `plan` lists the parts with what is there to remove right now;
// `apply` removes the ones it is given and says what it did for each.
//
// Nothing is a hard-coded path: `Targets` names the places, so the probe can
// run the whole thing on made-up ones.
//
// Reset: the startup entry, the keys in the Credential Manager, what the
// features keep (their data folders, the inbox, the relay program), and the
// agents' hook entries (the app's own `--uninstall-hooks`, dated backups, only
// its entries). Uninstall adds the settings folder, removed a few seconds after
// the app has quit, and the installer removes the program itself.
//
// debt: Credential Manager entries of features (`<id>/<name>`) are not listed, only the core keys.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

pub struct Targets {
    /// The Run value of the startup entry.
    pub run_value: String,
    /// Credential Manager keys to look for.
    pub secrets: Vec<String>,
    /// Folders emptied by a reset: (label, path).
    pub data: Vec<(&'static str, PathBuf)>,
    /// The relay program the agents' hooks run; goes only after their entries did.
    pub relay: PathBuf,
    /// The settings folder, removed only by an uninstall.
    pub config: PathBuf,
}

impl Targets {
    pub fn real() -> Self {
        let local = notch_core::win::local_dir();
        Targets {
            run_value: notch_brand::SLUG.to_string(),
            secrets: notch_core::secrets::KNOWN_KEYS.iter().map(|k| k.to_string()).collect(),
            data: vec![
                ("features", local.join("features")),
                ("inbox", local.join("inbox")),
            ],
            relay: local.join("bin"),
            config: notch_core::win::config_dir(),
        }
    }
}

fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if meta.is_file() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|d| d.flatten().map(|e| size_of(&e.path())).sum())
        .unwrap_or(0)
}

fn run_value_present(name: &str) -> bool {
    let mut cmd = std::process::Command::new("reg");
    notch_core::win::no_console(&mut cmd);
    cmd.args(["query", RUN_KEY, "/v", name]).output().is_ok_and(|o| o.status.success())
}

/// What is there to remove. `uninstall` adds the settings folder.
pub fn plan(t: &Targets, uninstall: bool, secret_present: impl Fn(&str) -> bool) -> Value {
    let mut items = Vec::new();
    items.push(json!({ "id": "startup", "present": run_value_present(&t.run_value), "detail": t.run_value }));
    let keys: Vec<&String> = t.secrets.iter().filter(|k| secret_present(k)).collect();
    items.push(json!({ "id": "secrets", "present": !keys.is_empty(), "count": keys.len(), "names": keys }));
    for (id, path) in &t.data {
        items.push(json!({ "id": id, "present": path.exists(), "detail": path.display().to_string(), "bytes": size_of(path) }));
    }
    items.push(json!({ "id": "hooks", "present": true, "detail": t.relay.display().to_string() }));
    if uninstall {
        items.push(json!({ "id": "config", "present": t.config.exists(), "detail": t.config.display().to_string(), "bytes": size_of(&t.config) }));
    }
    json!({ "items": items })
}

/// Removes `ids` and answers {id: "removed" | "nothing" | an error}.
pub fn apply(
    t: &Targets,
    ids: &[String],
    secret_clear: impl Fn(&str) -> Result<(), String>,
    run_hooks_removal: impl Fn() -> Result<(), String>,
) -> Value {
    let mut out = serde_json::Map::new();
    for id in ids {
        let result = match id.as_str() {
            "startup" => {
                if !run_value_present(&t.run_value) {
                    Ok("nothing")
                } else {
                    let mut cmd = std::process::Command::new("reg");
                    notch_core::win::no_console(&mut cmd);
                    match cmd.args(["delete", RUN_KEY, "/v", &t.run_value, "/f"]).output() {
                        Ok(o) if o.status.success() => Ok("removed"),
                        _ => Err("Could not remove the startup entry.".to_string()),
                    }
                }
            }
            "secrets" => {
                let failed: Vec<String> = t.secrets.iter().filter(|k| secret_clear(k).is_err()).cloned().collect();
                if failed.is_empty() { Ok("removed") } else { Err(format!("Could not remove: {}", failed.join(", "))) }
            }
            // The relay goes only when the agents' entries that run it are gone.
            "hooks" => run_hooks_removal().map(|_| {
                let _ = std::fs::remove_dir_all(&t.relay);
                "removed"
            }),
            "config" => Ok("after the app quits"),
            other => match t.data.iter().find(|(name, _)| name == &other) {
                Some((_, path)) if path.exists() => std::fs::remove_dir_all(path).map(|_| "removed").map_err(|e| e.to_string()),
                Some(_) => Ok("nothing"),
                None => Err("unknown part".to_string()),
            },
        };
        out.insert(id.clone(), match result {
            Ok(s) => json!(s),
            Err(e) => json!({ "error": e }),
        });
    }
    Value::Object(out)
}

/// Removes the settings folder a few seconds from now, when the app has quit
/// and no longer writes to it. Only the uninstall asks for it.
pub fn remove_config_later(config: &Path) {
    let mut cmd = std::process::Command::new("cmd");
    notch_core::win::no_console(&mut cmd);
    let _ = cmd
        .args(["/C", &format!("ping -n 4 127.0.0.1 >nul & rmdir /s /q \"{}\"", config.display())])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
