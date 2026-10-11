// Tools and setup — one feature holding the small things that do not each
// deserve a package: the demo, settings backup, presets and the first-run page
// (page side), the privacy page (page side), reset and uninstall, cleaning mode,
// the watch, the Terminal tab, and two Explorer extras. The contract is
// docs/ARCHITECTURE.md §2: nothing runs before `start`, and `start` itself only
// begins what the user switched on (the Explorer paste); everything else begins
// with a click and ends by itself or in `stop`.

pub mod backup;
pub mod clean;
pub mod demo;
pub mod explorer;
pub mod reset;
pub mod tabs;
pub mod watch;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use notch_core::{Ctx, Feature, Group, Manifest, Monitor, ShortcutDef, Use, WindowSpec};
use serde_json::{json, Value};

const ID: &str = "misc-late";
const CLEANING: &str = "cleaning";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Tools and setup",
    group: Group::Tools,
    // Backup, presets, the first-run page and the privacy list are part of the product.
    // Cleaning mode, the watch, the demo and the Explorer extras only begin on a click or a switch.
    default_on: true,
    uses: &[Use::InputHook, Use::Capture, Use::Clipboard],
    shortcuts: &[
        ShortcutDef { id: "demo", label: "Play or stop the demo", default: None },
        ShortcutDef { id: "cleaning", label: "Lock the keyboard for cleaning", default: None },
        ShortcutDef { id: "shelf-selection", label: "Add the Explorer selection to the shelf", default: None },
    ],
    tray: &[],
    defaults: r#"{ "onboarded": false, "cleaningSeconds": 120, "pasteImage": false, "shelfSelection": false }"#,
};

#[derive(Default)]
struct Slots {
    clean: Mutex<Option<clean::Running>>,
    watch: Mutex<Option<watch::Running>>,
    demo: Mutex<Option<demo::Running>>,
    paste: Mutex<Option<explorer::Paste>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
pub struct MiscLate {
    s: Arc<Slots>,
}

fn settings_file() -> PathBuf {
    notch_core::win::config_dir().join("settings.json")
}

fn documents() -> PathBuf {
    let home = notch_core::win::home_dir();
    let docs = home.join("Documents");
    if docs.is_dir() { docs } else { home }
}

fn path_arg(args: &Value) -> Result<PathBuf, String> {
    args["path"].as_str().map(PathBuf::from).ok_or_else(|| "No file was chosen.".to_string())
}

impl MiscLate {
    // ── Cleaning mode ────────────────────────────────────────────────────────

    fn clean_start(&self, cx: &Ctx) -> Result<Value, String> {
        if lock(&self.s.clean).as_ref().is_some_and(|r| !r.finished()) {
            return Ok(json!({ "on": true }));
        }
        let secs = cx.settings(ID)["cleaningSeconds"].as_u64().unwrap_or(120);
        // The window with the way out comes up first: no lock without it.
        cx.window()
            .open(WindowSpec {
                label: CLEANING.into(),
                feature: ID.into(),
                page: CLEANING.into(),
                w: 540.0,
                h: 340.0,
                frameless: true,
                topmost: true,
                monitor: Monitor::Cursor,
                ..Default::default()
            })
            .map_err(|e| format!("Could not open the unlock window: {e}"))?;
        let (on_update, on_end) = (cx.clone(), cx.clone());
        let slots = self.s.clone();
        let running = clean::start(
            secs,
            move |u| {
                on_update.emit(ID, "cleaning", json!({ "on": true, "count": u.count, "need": u.need, "remaining": u.remaining, "limit": u.limit }));
            },
            move |why| {
                // Our own handle is dropped without joining: we are the thread it holds.
                lock(&slots.clean).take();
                on_end.window().close(CLEANING);
                on_end.emit(ID, "cleaning", json!({ "on": false, "why": format!("{why:?}").to_lowercase() }));
            },
        );
        *lock(&self.s.clean) = Some(running);
        Ok(json!({ "on": true }))
    }

    fn clean_stop(&self) {
        let running = lock(&self.s.clean).take();
        if let Some(r) = running {
            r.stop();
        }
    }

    // ── Watch ────────────────────────────────────────────────────────────────

    fn watch_start(&self, cx: &Ctx, args: &Value) -> Result<Value, String> {
        self.watch_stop();
        let hwnd = args["hwnd"].as_i64().ok_or("Pick a window to watch.")? as isize;
        let cond = match args["cond"].as_str().unwrap_or("changes") {
            "contains" => {
                let words = args["text"].as_str().unwrap_or("").trim().to_string();
                if words.is_empty() {
                    return Err("Type the words to wait for.".into());
                }
                watch::Cond::Contains(words)
            }
            "reaches" => watch::Cond::Reaches(
                args["number"].as_f64().or_else(|| args["number"].as_str().and_then(watch::parse_number)).ok_or("Type the number to wait for.")?,
            ),
            _ => watch::Cond::Changes,
        };
        if !svc_capture::ocr_available() {
            return Err("Windows has no text recognition for your languages installed.".into());
        }
        let title = svc_windows::windows().into_iter().find(|w| w.hwnd == hwnd).map(|w| w.title).unwrap_or_default();
        let (cx2, slots) = (cx.clone(), self.s.clone());
        let running = watch::start(hwnd, cond, Duration::from_secs(2), move |r| {
            let ended = r.matched || r.error.is_some();
            cx2.emit(ID, "watch", json!({ "on": !ended, "headline": r.headline, "matched": r.matched, "error": r.error, "title": title }));
            if ended {
                lock(&slots.watch).take();
            }
        });
        *lock(&self.s.watch) = Some(running);
        Ok(json!({ "on": true }))
    }

    fn watch_stop(&self) {
        let running = lock(&self.s.watch).take();
        if let Some(r) = running {
            r.stop();
        }
    }

    // ── Demo ─────────────────────────────────────────────────────────────────

    fn demo_toggle(&self, cx: &Ctx, on: Option<bool>) -> Result<Value, String> {
        let running = lock(&self.s.demo).as_ref().is_some_and(|r| !r.finished());
        if on.unwrap_or(!running) == running {
            return Ok(json!({ "on": running }));
        }
        if running {
            let r = lock(&self.s.demo).take();
            if let Some(r) = r {
                r.stop();
            }
            cx.emit(ID, "demo", json!({ "on": false }));
            return Ok(json!({ "on": false }));
        }
        let (cx_step, cx_end, slots) = (cx.clone(), cx.clone(), self.s.clone());
        let r = demo::start(
            move |i, total| cx_step.emit(ID, "demo", json!({ "on": true, "step": i + 1, "total": total })),
            move || {
                lock(&slots.demo).take();
                cx_end.emit(ID, "demo", json!({ "on": false }));
            },
        )?;
        *lock(&self.s.demo) = Some(r);
        cx.emit(ID, "demo", json!({ "on": true, "step": 0, "total": 0 }));
        Ok(json!({ "on": true }))
    }

    // ── Explorer ─────────────────────────────────────────────────────────────

    fn sync_paste(&self, cx: &Ctx) {
        let want = cx.settings(ID)["pasteImage"].as_bool() == Some(true);
        let mut slot = lock(&self.s.paste);
        if want && slot.is_none() {
            let cx = cx.clone();
            *slot = Some(explorer::watch_paste(move |path| {
                cx.emit(ID, "pasted", json!({ "path": path.display().to_string(), "name": path.file_name().map(|n| n.to_string_lossy().to_string()) }));
            }));
        } else if !want {
            slot.take();
        }
    }

    fn explorer_selection(&self, cx: &Ctx) -> Result<Value, String> {
        if cx.settings(ID)["shelfSelection"].as_bool() != Some(true) {
            return Err("Turn on \"Explorer selection to the shelf\" in Settings first.".into());
        }
        let paths: Vec<String> = explorer::selection()?.into_iter().map(|p| p.display().to_string()).collect();
        if paths.is_empty() {
            return Err("Nothing is selected.".into());
        }
        cx.emit(ID, "selection", json!({ "paths": paths }));
        Ok(json!({ "count": paths.len() }))
    }

    // ── Backup ───────────────────────────────────────────────────────────────

    fn backup_plan(&self, file: &Path) -> Result<backup::Plan, String> {
        let incoming = backup::read_json(file)?;
        let current = if settings_file().exists() { backup::read_json(&settings_file())? } else { json!({}) };
        backup::plan(&current, &incoming)
    }
}

impl Feature for MiscLate {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn start(&self, cx: &Ctx) -> Result<(), String> {
        self.sync_paste(cx);
        Ok(())
    }

    fn stop(&self, cx: &Ctx) {
        self.clean_stop();
        self.watch_stop();
        let demo = lock(&self.s.demo).take();
        if let Some(d) = demo {
            d.stop();
        }
        lock(&self.s.paste).take();
        cx.window().close(CLEANING);
    }

    fn settings_changed(&self, cx: &Ctx, _settings: &Value) {
        self.sync_paste(cx);
    }

    // The page cannot read an `Err` (it gets null and a log line), so what a person has to be told
    // comes back as { error }.
    fn call(&self, cx: &Ctx, method: &str, args: Value) -> Result<Value, String> {
        match self.dispatch(cx, method, args) {
            Err(e) if method != "shortcut" && e != "unknown method" => Ok(json!({ "error": e })),
            other => other,
        }
    }

    fn strings(&self) -> &'static str {
        include_str!("../../../src/features/misc-late/strings.json")
    }
}

impl MiscLate {
    fn dispatch(&self, cx: &Ctx, method: &str, args: Value) -> Result<Value, String> {
        match method {
            "shortcut" => match args["id"].as_str() {
                Some("demo") => self.demo_toggle(cx, None),
                Some("cleaning") => self.clean_start(cx),
                Some("shelf-selection") => self.explorer_selection(cx).map(|_| Value::Null).or_else(|e| {
                    // A shortcut has no page to show the reason on.
                    notch_core::log::line(format!("misc-late: shelf selection: {e}"));
                    Ok(Value::Null)
                }),
                _ => Err("unknown method".into()),
            },

            "demo" => self.demo_toggle(cx, args["on"].as_bool()),
            "demo_state" => Ok(json!({ "on": lock(&self.s.demo).as_ref().is_some_and(|r| !r.finished()) })),

            "cleaning_start" => self.clean_start(cx),
            "cleaning_stop" => {
                self.clean_stop();
                Ok(json!({ "on": false }))
            }
            "cleaning_state" => Ok(json!({ "on": lock(&self.s.clean).as_ref().is_some_and(|r| !r.finished()) })),

            "watch_windows" => Ok(Value::Array(
                svc_windows::windows()
                    .into_iter()
                    .filter(|w| !w.title.trim().is_empty() && !w.minimized)
                    .map(|w| json!({ "hwnd": w.hwnd, "title": w.title, "app": w.app() }))
                    .collect(),
            )),
            "watch_start" => self.watch_start(cx, &args),
            "watch_stop" => {
                self.watch_stop();
                cx.emit(ID, "watch", json!({ "on": false }));
                Ok(json!({ "on": false }))
            }

            "terminal_tab" => {
                let hints = tabs::Hints {
                    cwd: args["cwd"].as_str().unwrap_or("").to_string(),
                    title: args["title"].as_str().unwrap_or("").to_string(),
                };
                let terminals = tabs::terminal_windows();
                let wanted = args["hwnd"].as_i64().map(|h| h as isize);
                for hwnd in terminals.into_iter().filter(|h| wanted.is_none_or(|w| w == *h)) {
                    if let Some(tab) = tabs::select(hwnd, &hints)? {
                        svc_windows::activate(hwnd);
                        return Ok(json!({ "selected": tab }));
                    }
                }
                Ok(json!({ "selected": null }))
            }

            "explorer_selection" => self.explorer_selection(cx),

            "backup_export" => {
                let stamp = notch_core::win::stamp();
                let to = documents().join(format!("{}-settings-{stamp}.json", notch_brand::SLUG));
                backup::export_file(&settings_file(), &to, env!("CARGO_PKG_VERSION"), &stamp)?;
                Ok(json!({ "path": to.display().to_string() }))
            }
            "backup_pick" => {
                let picked = svc_shell::pick_files("Choose a settings backup");
                Ok(json!({ "path": picked.first().map(|p| p.display().to_string()) }))
            }
            "backup_preview" => Ok(backup::plan_json(&self.backup_plan(&path_arg(&args)?)?)),
            "backup_apply" => {
                let plan = self.backup_plan(&path_arg(&args)?)?;
                // The copy first: nothing is replaced before it exists.
                let copy = backup::keep_copy(&settings_file(), &notch_core::win::config_dir().join("backups"), &notch_core::win::stamp())?;
                for (id, keys) in &plan.features {
                    cx.patch_settings(id, keys.clone());
                }
                Ok(json!({
                    "core": plan.core, "enabled": plan.enabled,
                    "copy": copy.display().to_string(), "changes": plan.changes.len(),
                }))
            }
            "reveal" => svc_shell::reveal(&path_arg(&args)?).map(|_| Value::Null),

            "reset_plan" => Ok(reset::plan(&reset::Targets::real(), args["uninstall"] == true, notch_core::secrets::present)),
            "reset_apply" => {
                let uninstall = args["uninstall"] == true;
                let ids: Vec<String> = args["ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
                let targets = reset::Targets::real();
                let done = reset::apply(&targets, &ids, notch_core::secrets::clear, || {
                    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                    let mut cmd = std::process::Command::new(exe);
                    notch_core::win::no_console(&mut cmd);
                    match cmd.arg("--uninstall-hooks").status() {
                        Ok(s) if s.success() => Ok(()),
                        _ => Err("Could not take the entries out of the agents' settings.".to_string()),
                    }
                });
                if uninstall && ids.iter().any(|i| i == "config") {
                    reset::remove_config_later(&targets.config);
                    let app = cx.app().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(800));
                        app.exit(0);
                    });
                }
                Ok(done)
            }

            "open_settings" => {
                // Windows' own settings pages the privacy list points to; nothing else is opened from here.
                let page = args["page"].as_str().unwrap_or("");
                if !matches!(page, "privacy-webcam" | "privacy-microphone" | "appsfeatures") {
                    return Err("unknown page".into());
                }
                svc_shell::open(&format!("ms-settings:{page}")).map(|_| Value::Null)
            }

            _ => Err("unknown method".into()),
        }
    }
}

pub fn feature() -> std::sync::Arc<dyn Feature> {
    std::sync::Arc::new(MiscLate::default())
}
