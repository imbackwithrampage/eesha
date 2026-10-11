// The feature registry host (docs/ARCHITECTURE.md §2).
//
// Every feature is one `notch_core::Feature` object: the base features wrapped
// in features/ (hand-listed) plus one per `notch-feat-*` dependency of this
// crate, which build.rs collects into features_gen.rs. A feature has two
// states: enabled (the user's switch, `Settings.enabled`, absent = the
// manifest's default) and running (its `start` succeeded). Nothing of a
// disabled feature runs.
//
// Feature code never runs on the main thread inside a Tauri command: the
// commands below are async and hop to a blocking thread, as do shortcut and
// tray clicks. That is what lets a feature open a window from `start` or
// `call` (see winfactory.rs).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use notch_core::{Ctx, Feature, Group, Host, Manifest, ShortcutDef, Use, WindowSpec};
use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::settings::Settings;
use crate::{log, settings, shortcuts, tray, winfactory, Shared};

mod features_gen {
    include!(concat!(env!("OUT_DIR"), "/features_gen.rs"));
}

pub struct Registry {
    features: Vec<Arc<dyn Feature>>,
    running: Mutex<BTreeSet<&'static str>>,
}

impl Registry {
    /// Built before the app: no feature runs yet, but their strings and secret
    /// names are known from here on.
    pub fn new() -> Self {
        let mut features: Vec<Arc<dyn Feature>> = Vec::new();
        for feature in crate::features::builtin().into_iter().chain(features_gen::all()) {
            let id = feature.manifest().id;
            if features.iter().any(|f| f.manifest().id == id) {
                log::line(format!("feature {id} is registered twice — the second one is ignored"));
                continue;
            }
            notch_core::i18n::merge(feature.strings());
            features.push(feature);
        }
        notch_core::secrets::allow_features(features.iter().map(|f| f.manifest().id).collect());
        Self { features, running: Mutex::new(BTreeSet::new()) }
    }
}

// ── What `Ctx` asks of the app ────────────────────────────────────────────────

struct AppHost;
static HOST: AppHost = AppHost;

impl Host for AppHost {
    fn settings(&self, app: &AppHandle, id: &str) -> Value {
        feature_settings_of(app, id)
    }
    fn patch_settings(&self, app: &AppHandle, id: &str, patch: Value) {
        if let Err(err) = patch_settings(app, id, patch) {
            log::line(format!("feature {id}: settings not saved: {err}"));
        }
    }
    fn open_window(&self, app: &AppHandle, spec: WindowSpec) -> Result<WebviewWindow, String> {
        winfactory::open(app, spec)
    }
    fn close_window(&self, app: &AppHandle, label: &str) {
        winfactory::close(app, label);
    }
}

pub fn ctx(app: &AppHandle) -> Ctx {
    Ctx::new(app.clone(), &HOST)
}

// ── Lookups ───────────────────────────────────────────────────────────────────

fn find(app: &AppHandle, id: &str) -> Result<Arc<dyn Feature>, String> {
    app.state::<Registry>()
        .features
        .iter()
        .find(|f| f.manifest().id == id)
        .cloned()
        .ok_or_else(|| format!("unknown feature {id}"))
}

fn switched_on(settings: &Settings, manifest: &Manifest) -> bool {
    settings.enabled.get(manifest.id).copied().unwrap_or(manifest.default_on)
}

pub fn is_running(app: &AppHandle, id: &str) -> bool {
    app.try_state::<Registry>().is_some_and(|r| r.running.lock().unwrap().contains(id))
}

/// For the base commands of a built-in feature: refused while it is off.
pub fn require(app: &AppHandle, id: &str) -> Result<(), String> {
    if is_running(app, id) {
        return Ok(());
    }
    let name = find(app, id).map(|f| f.manifest().name).unwrap_or(id);
    Err(crate::i18n::tf("{name} is turned off.", &[("name", &crate::i18n::t(name))]))
}

/// The global shortcuts of the running features, for shortcuts.rs.
pub fn shortcut_defs(app: &AppHandle) -> Vec<(&'static str, ShortcutDef)> {
    running_manifests(app).into_iter().flat_map(|m| m.shortcuts.iter().map(move |s| (m.id, *s))).collect()
}

/// The tray items of the running features, for tray.rs: (feature, item, label).
pub fn tray_items(app: &AppHandle) -> Vec<(&'static str, &'static str, &'static str)> {
    running_manifests(app).into_iter().flat_map(|m| m.tray.iter().map(move |(id, label)| (m.id, *id, *label))).collect()
}

fn running_manifests(app: &AppHandle) -> Vec<&'static Manifest> {
    let Some(registry) = app.try_state::<Registry>() else { return Vec::new() };
    let running = registry.running.lock().unwrap();
    registry.features.iter().map(|f| f.manifest()).filter(|m| running.contains(m.id)).collect()
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

fn start(app: &AppHandle, feature: &Arc<dyn Feature>) -> Result<(), String> {
    let id = feature.manifest().id;
    match feature.start(&ctx(app)) {
        Ok(()) => {
            app.state::<Registry>().running.lock().unwrap().insert(id);
            log::line(format!("feature {id} started"));
            Ok(())
        }
        Err(err) => {
            log::line(format!("feature {id} failed to start: {err}"));
            Err(err)
        }
    }
}

fn stop(app: &AppHandle, feature: &Arc<dyn Feature>) {
    let id = feature.manifest().id;
    if !app.state::<Registry>().running.lock().unwrap().remove(id) {
        return;
    }
    feature.stop(&ctx(app));
    // Whatever window it left open goes with it.
    winfactory::close_all_of(app, id);
    log::line(format!("feature {id} stopped"));
}

/// At launch, from `setup`: starts every feature that is switched on.
pub fn start_enabled(app: &AppHandle) {
    let settings = app.state::<Shared>().settings.lock().unwrap().clone();
    let features = app.state::<Registry>().features.clone();
    for feature in features.iter().filter(|f| switched_on(&settings, f.manifest())) {
        let _ = start(app, feature);
    }
}

/// At quit.
pub fn stop_all(app: &AppHandle) {
    let features = app.state::<Registry>().features.clone();
    for feature in &features {
        stop(app, feature);
    }
}

/// The user's switch: start or stop, remember it, tell every window, and
/// rebuild what is made from manifests (tray menu, global shortcuts).
pub fn set_enabled(app: &AppHandle, id: &str, on: bool) -> Result<(), String> {
    let feature = find(app, id)?;
    let started = if on && !is_running(app, id) { start(app, &feature) } else { Ok(()) };
    if !on {
        stop(app, &feature);
    }
    let stored = {
        let shared = app.state::<Shared>();
        let mut current = shared.settings.lock().unwrap();
        current.enabled.insert(id.to_string(), on);
        if let Err(err) = settings::save(&current) {
            log::line(format!("could not save settings: {err}"));
        }
        current.shortcuts.clone()
    };
    let _ = app.emit("features-changed", json!({ "id": id, "on": on }));
    tray::rebuild(app);
    shortcuts::apply(app, &stored);
    started
}

// ── Settings ──────────────────────────────────────────────────────────────────

/// defaults ⊕ stored: the manifest's defaults with what the user changed on top.
pub fn feature_settings_of(app: &AppHandle, id: &str) -> Value {
    let mut merged = find(app, id)
        .ok()
        .and_then(|f| serde_json::from_str::<Map<String, Value>>(f.manifest().defaults).ok())
        .unwrap_or_default();
    let stored = app.state::<Shared>().settings.lock().unwrap().features.get(id).cloned();
    if let Some(Value::Object(stored)) = stored {
        merged.extend(stored);
    }
    Value::Object(merged)
}

/// Shallow merge into what is stored, saved atomically with the rest of the
/// settings, then `settings-changed:<id>` with the merged value.
pub fn patch_settings(app: &AppHandle, id: &str, patch: Value) -> Result<(), String> {
    let feature = find(app, id)?;
    let Value::Object(patch) = patch else { return Err("a settings patch is an object".into()) };
    {
        let shared = app.state::<Shared>();
        let mut current = shared.settings.lock().unwrap();
        let entry = current.features.entry(id.to_string()).or_insert_with(|| json!({}));
        if !entry.is_object() {
            *entry = json!({});
        }
        if let Value::Object(stored) = entry {
            stored.extend(patch);
        }
        settings::save(&current).map_err(|e| e.to_string())?;
    }
    let merged = feature_settings_of(app, id);
    let _ = app.emit(&format!("settings-changed:{id}"), &merged);
    if is_running(app, id) {
        feature.settings_changed(&ctx(app), &merged);
    }
    Ok(())
}

// ── Calls ─────────────────────────────────────────────────────────────────────

pub fn call(app: &AppHandle, id: &str, method: &str, args: Value) -> Result<Value, String> {
    let feature = find(app, id)?;
    // The headless completion lives in the chat adapter and is the voice assistant's: voice does not
    // depend on the Chat feature being on. Its own gate is chat/headless.rs `permit`.
    if !(id == "chat" && matches!(method, "ask" | "cancel" | "reach")) {
        require(app, id)?;
    }
    feature.call(&ctx(app), method, args)
}

/// A global shortcut or a tray item of a feature: `<feature>:<what> { id }` to
/// the windows, and `call(what, { id })` to the feature, off the main thread.
pub fn notify(app: &AppHandle, feature: &str, what: &'static str, id: &str) {
    let (app, feature, id) = (app.clone(), feature.to_string(), id.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let _ = app.emit(&format!("{feature}:{what}"), json!({ "id": id }));
        match call(&app, &feature, what, json!({ "id": id })) {
            Ok(_) => {}
            // A feature that handles it in its page only.
            Err(err) if err == "unknown method" => {}
            Err(err) => log::line(format!("feature {feature} {what} {id}: {err}")),
        }
    });
}

// ── Commands (the only ones a feature uses) ───────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureInfo {
    id: &'static str,
    /// English; the page translates it.
    name: &'static str,
    group: Group,
    default_on: bool,
    on: bool,
    running: bool,
    uses: &'static [Use],
    shortcuts: &'static [ShortcutDef],
    tray: Vec<Value>,
}

#[tauri::command]
pub fn features_list(app: AppHandle) -> Vec<FeatureInfo> {
    let settings = app.state::<Shared>().settings.lock().unwrap().clone();
    let registry = app.state::<Registry>();
    let running = registry.running.lock().unwrap();
    registry
        .features
        .iter()
        .map(|f| f.manifest())
        .map(|m| FeatureInfo {
            id: m.id,
            name: m.name,
            group: m.group,
            default_on: m.default_on,
            on: switched_on(&settings, m),
            running: running.contains(m.id),
            uses: m.uses,
            shortcuts: m.shortcuts,
            tray: m.tray.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect(),
        })
        .collect()
}

#[tauri::command]
pub async fn feature_call(app: AppHandle, feature: String, method: String, args: Option<Value>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || call(&app, &feature, &method, args.unwrap_or(Value::Null)))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn feature_settings(app: AppHandle, feature: String) -> Value {
    feature_settings_of(&app, &feature)
}

#[tauri::command]
pub async fn feature_patch_settings(app: AppHandle, feature: String, patch: Value) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || patch_settings(&app, &feature, patch))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn feature_set_enabled(app: AppHandle, feature: String, on: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || set_enabled(&app, &feature, on))
        .await
        .map_err(|e| e.to_string())?
}
