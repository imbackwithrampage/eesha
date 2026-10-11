// Settings backup: export and import of the settings file, nothing else.
//
// What travels is settings.json in an envelope {formatVersion, app, appVersion,
// exportedAt, settings}. Secrets never do: the keys live in the Credential
// Manager, and anything in the file that is named or shaped like one is
// dropped on the way out and again on the way in. The install state of this
// machine (hooks, relay, startup entry) stays here. An import is a plan first
// (`plan`: what would change, what was left out), and applying it keeps a dated
// copy of the file it replaces (`keep_copy`).
//
// Only pure functions on `serde_json::Value` and plain files, so the probe can
// run the round trip on a made-up file.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

pub const FORMAT: u64 = 1;

/// Core settings that describe this machine, not the user's choices.
const MACHINE: &[&str] = &["hooksInstalled", "planRelayInstalled", "autostart"];
/// Parts of a name that make a value a secret.
const SECRET_WORDS: &[&str] = &["secret", "token", "password", "apikey", "api_key", "api-key", "credential"];
/// Starts of the strings that API providers hand out.
const SECRET_SHAPES: &[&str] = &["sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "AIza"];

fn secret_name(name: &str) -> bool {
    let n = name.to_lowercase();
    SECRET_WORDS.iter().any(|w| n.contains(w)) || (n.len() > 3 && n.ends_with("key") && n != "hotkey" && n != "shortkey")
}

fn secret_value(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.len() > 12 && SECRET_SHAPES.iter().any(|p| s.starts_with(p)))
}

/// Takes every secret-looking entry out, at any depth. Returns how many.
pub fn scrub(v: &mut Value) -> usize {
    let mut n = 0;
    match v {
        Value::Object(map) => {
            let bad: Vec<String> = map.iter().filter(|(k, v)| secret_name(k) || secret_value(v)).map(|(k, _)| k.clone()).collect();
            for k in &bad {
                map.remove(k);
            }
            n += bad.len();
            for child in map.values_mut() {
                n += scrub(child);
            }
        }
        Value::Array(items) => {
            items.retain(|i| !secret_value(i));
            for child in items {
                n += scrub(child);
            }
        }
        _ => {}
    }
    n
}

/// The envelope for `settings` (the content of settings.json).
pub fn export_value(settings: &Value, app_version: &str, exported_at: &str) -> Value {
    let mut body = settings.clone();
    if let Value::Object(map) = &mut body {
        for key in MACHINE {
            map.remove(*key);
        }
    }
    scrub(&mut body);
    json!({
        "formatVersion": FORMAT,
        "app": notch_brand::NAME,
        "appVersion": app_version,
        "exportedAt": exported_at,
        "settings": body,
    })
}

pub fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let text = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    serde_json::from_slice(text).map_err(|_| "This file is not a valid backup.".to_string())
}

/// Writes the export of the settings file `from` to `to`.
pub fn export_file(from: &Path, to: &Path, app_version: &str, exported_at: &str) -> Result<(), String> {
    let settings = if from.exists() { read_json(from)? } else { json!({}) };
    let envelope = export_value(&settings, app_version, exported_at);
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())?;
    std::fs::write(to, text).map_err(|e| format!("Could not write {}: {e}", to.display()))
}

// ── Import ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// `language`, `features.clipboard.max`, `enabled.clipboard`.
    pub path: String,
    pub from: Value,
    pub to: Value,
}

#[derive(Debug, Default)]
pub struct Plan {
    /// Core settings to set.
    pub core: Map<String, Value>,
    /// Per feature id: the keys to set.
    pub features: Map<String, Value>,
    pub enabled: Map<String, Value>,
    pub changes: Vec<Change>,
    /// Entries of the file that were left out: secrets, other machines' state, unknown or mistyped values.
    pub skipped: usize,
}

fn same_kind(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// What importing `file` (an envelope) into `current` (the content of settings.json) would change.
/// Anything not in `current` or of another type is dropped, as are secrets;
/// a feature's own keys, which `current` only holds when they were changed, are taken as they come.
pub fn plan(current: &Value, file: &Value) -> Result<Plan, String> {
    let bad = || "This file is not a valid backup.".to_string();
    let version = file.get("formatVersion").and_then(Value::as_u64).ok_or_else(bad)?;
    if version == 0 || version > FORMAT {
        return Err("This backup comes from a newer version of the app.".into());
    }
    let incoming = file.get("settings").and_then(Value::as_object).ok_or_else(bad)?;
    let empty = Map::new();
    let have = current.as_object().unwrap_or(&empty);
    let mut plan = Plan::default();
    let note = |plan: &mut Plan, path: String, from: Value, to: Value| {
        if from != to {
            plan.changes.push(Change { path, from, to });
        }
    };

    for (key, value) in incoming {
        match key.as_str() {
            "features" | "enabled" => {}
            k if MACHINE.contains(&k) || secret_name(k) || secret_value(value) => plan.skipped += 1,
            k => match have.get(k) {
                Some(now) if same_kind(now, value) => {
                    note(&mut plan, k.to_string(), now.clone(), value.clone());
                    if now != value {
                        plan.core.insert(k.to_string(), value.clone());
                    }
                }
                _ => plan.skipped += 1,
            },
        }
    }

    let now_features = have.get("features").and_then(Value::as_object).unwrap_or(&empty);
    if let Some(features) = incoming.get("features").and_then(Value::as_object) {
        for (id, keys) in features {
            let Some(keys) = keys.as_object() else {
                plan.skipped += 1;
                continue;
            };
            let before = now_features.get(id).and_then(Value::as_object).unwrap_or(&empty);
            let mut take = Map::new();
            for (k, v) in keys {
                let mut v = v.clone();
                // A key or token inside the value goes, whatever the value is called.
                let hidden = secret_name(k) || secret_value(&v) || scrub(&mut v) > 0;
                if hidden || before.get(k).is_some_and(|old| !same_kind(old, &v)) {
                    plan.skipped += 1;
                } else if before.get(k) != Some(&v) {
                    note(&mut plan, format!("features.{id}.{k}"), before.get(k).cloned().unwrap_or(Value::Null), v.clone());
                    take.insert(k.clone(), v);
                }
            }
            if !take.is_empty() {
                plan.features.insert(id.clone(), Value::Object(take));
            }
        }
    }

    let now_enabled = have.get("enabled").and_then(Value::as_object).unwrap_or(&empty);
    if let Some(enabled) = incoming.get("enabled").and_then(Value::as_object) {
        for (id, on) in enabled {
            if !on.is_boolean() {
                plan.skipped += 1;
            } else if now_enabled.get(id) != Some(on) {
                note(&mut plan, format!("enabled.{id}"), now_enabled.get(id).cloned().unwrap_or(Value::Null), on.clone());
                plan.enabled.insert(id.clone(), on.clone());
            }
        }
    }
    Ok(plan)
}

/// `current` with the plan applied: what the host ends up with once it has
/// taken the three parts. The real import goes through the host's own
/// commands; this is what the probe compares.
pub fn apply_to(current: &mut Value, plan: &Plan) {
    let Value::Object(map) = current else { return };
    for (k, v) in &plan.core {
        map.insert(k.clone(), v.clone());
    }
    for (part, entries) in [("features", &plan.features), ("enabled", &plan.enabled)] {
        let target = map.entry(part).or_insert_with(|| json!({}));
        if let Value::Object(target) = target {
            for (id, v) in entries {
                match (target.get_mut(id), v) {
                    (Some(Value::Object(old)), Value::Object(new)) if part == "features" => old.extend(new.clone()),
                    _ => {
                        target.insert(id.clone(), v.clone());
                    }
                }
            }
        }
    }
}

/// A dated copy of the settings file in `dir`, made before an import replaces anything.
pub fn keep_copy(settings: &Path, dir: &Path, stamp: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let to = dir.join(format!("settings-{stamp}.json"));
    if settings.exists() {
        std::fs::copy(settings, &to).map_err(|e| format!("Could not keep a copy of the settings: {e}"))?;
    } else {
        std::fs::write(&to, "{}").map_err(|e| e.to_string())?;
    }
    Ok(to)
}

pub fn plan_json(plan: &Plan) -> Value {
    json!({
        "changes": plan.changes.iter().map(|c| json!({ "path": c.path, "from": c.from, "to": c.to })).collect::<Vec<_>>(),
        "skipped": plan.skipped,
    })
}
