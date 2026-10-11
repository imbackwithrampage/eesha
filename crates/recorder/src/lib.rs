// Recorder — the screen as a film: a display or the window in front, with the
// computer's sound and the microphone as two separate tracks, into an MP4 in
// the user's Videos folder.
//
// The contract is docs/ARCHITECTURE.md §2: nothing runs before `start`,
// everything is gone after `stop`, every command arrives through `call`. At
// rest there is nothing at all: no thread, hook, timer or window exists until
// the shortcut starts a recording. A recording is one thread of `rec`, which
// owns the capture, the sound and the file; it ends by the shortcut or the
// Stop button, and by itself when the disk runs low, the recorded window
// closes, the computer is locked or goes to sleep. Whatever ends it, and
// whatever happens to the page meanwhile, that thread finishes the file.
//
// The island shows the recording and is left out of it for as long as it runs.
// The microphone is opened only when its setting is on, only for the length of
// a recording.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use notch_core::{log, win, Ctx, Feature, Group, Manifest, ShortcutDef, Use};
use serde_json::{json, Value};
use svc_audio::Flow;
use svc_capture::Rect;
use tauri::Manager;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetShellWindow, IsIconic, IsWindowVisible};

pub mod mic;
pub mod rec;

use rec::{Event, Options, Quality, Recording, Sound, Stop, Target, Track};

const ID: &str = "recorder";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Screen recorder",
    group: Group::Capture,
    default_on: true,
    uses: &[Use::Capture, Use::Microphone],
    shortcuts: &[
        ShortcutDef { id: "toggle", label: "Start or stop a screen recording", default: Some("Ctrl+Alt+Shift+5") },
        ShortcutDef { id: "window", label: "Record the window in front", default: None },
        ShortcutDef { id: "pause", label: "Pause or resume the recording", default: None },
    ],
    tray: &[],
    defaults: r#"{ "fps": 60, "quality": "balanced", "systemAudio": true, "microphone": false, "mic": "", "pointer": true, "border": false, "folder": "" }"#,
};

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The recording that runs.
struct Running {
    take: Recording,
    /// "display", "window" or "region".
    what: &'static str,
    /// Lock and sleep, heard only while it runs.
    _session: svc_sysevents::Sub,
    /// The island's window, when this recording took it out of the captures.
    island: Option<isize>,
}

/// One recording at a time; empty at rest.
static NOW: Mutex<Option<Running>> = Mutex::new(None);
/// The file of the last recording, for Open and Show in folder: the page never names a path.
static LAST: Mutex<Option<PathBuf>> = Mutex::new(None);

/// What the pages are told. Said while `NOW` is held wherever a recording
/// begins or changes, so that it can never arrive after the news that it ended.
fn state_of(now: &Option<Running>) -> Value {
    match now {
        Some(r) => json!({ "recording": true, "paused": r.take.paused(), "elapsed": r.take.elapsed_ms(), "what": r.what }),
        None => json!({ "recording": false, "paused": false, "elapsed": 0 }),
    }
}

fn state() -> Value {
    state_of(&locked(&NOW))
}

/// The user's Videos folder, wherever it was moved to.
fn videos() -> PathBuf {
    unsafe {
        match SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None) {
            Ok(path) => {
                let folder = PathBuf::from(path.to_string().unwrap_or_default());
                CoTaskMemFree(Some(path.0 as *const _));
                folder
            }
            Err(_) => win::home_dir().join("Videos"),
        }
    }
}

/// Where recordings go: the folder of the settings, or a folder of the app's in Videos.
fn folder(cx: &Ctx) -> PathBuf {
    let chosen = PathBuf::from(cx.settings(ID)["folder"].as_str().unwrap_or_default().trim());
    if chosen.is_absolute() { chosen } else { videos().join(notch_brand::NAME) }
}

/// "Recording 2026-10-11 at 14.03.22.mp4", or "… (2).mp4" when that is taken.
fn free_name(dir: &Path) -> PathBuf {
    let t = win::local_time();
    let name = format!("Recording {:04}-{:02}-{:02} at {:02}.{:02}.{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second);
    (1u32..)
        .map(|n| dir.join(if n == 1 { format!("{name}.mp4") } else { format!("{name} ({n}).mp4") }))
        .find(|path| !path.exists())
        .expect("a free name")
}

fn island(cx: &Ctx) -> Option<isize> {
    cx.app().get_webview_window("island").and_then(|window| window.hwnd().ok()).map(|hwnd| hwnd.0 as isize)
}

/// The display the pointer is on.
fn display() -> Result<Rect, String> {
    let mut at = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut at);
    }
    let monitors = svc_capture::monitors();
    monitors.iter().find(|m| m.rect.contains(at.x, at.y)).or(monitors.first()).map(|m| m.rect).ok_or_else(|| "There is no display to record.".to_string())
}

/// The window to record: the one that was picked (`hwnd`), or the one the user
/// works in. Not the desktop, and not the island.
fn window(cx: &Ctx, picked: Option<isize>) -> Result<isize, String> {
    unsafe {
        let hwnd = picked.map_or_else(|| GetForegroundWindow(), |hwnd| HWND(hwnd as *mut _));
        let usable = !hwnd.0.is_null() && hwnd != GetShellWindow() && Some(hwnd.0 as isize) != island(cx) && IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool();
        if usable { Ok(hwnd.0 as isize) } else { Err("No window is in front to record.".into()) }
    }
}

fn stop(why: Stop) {
    if let Some(running) = locked(&NOW).as_ref() {
        running.take.stop(why);
    }
}

/// The recording's thread closed the file (or could not): the island is back
/// in the captures and the pages are told.
fn ended(cx: &Ctx, result: Result<rec::Done, String>) {
    let running = locked(&NOW).take();
    if let Some(hwnd) = running.as_ref().and_then(|r| r.island) {
        svc_capture::exclude_from_capture(hwnd, false);
    }
    drop(running);
    match result {
        Ok(done) => {
            log::line(format!("recorder: saved {:.1} s, {} pictures, {} bytes ({})", done.seconds, done.frames, done.bytes, done.why.name()));
            let name = done.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            *locked(&LAST) = Some(done.path);
            cx.emit(ID, "saved", json!({ "name": name, "seconds": done.seconds, "bytes": done.bytes, "why": done.why.name() }));
        }
        Err(why) => {
            log::line(format!("recorder: {why}"));
            cx.emit(ID, "failed", json!({ "message": why }));
        }
    }
    cx.emit(ID, "state", state());
}

/// Starts a recording of `what`: "display" (the one the pointer is on),
/// "window" (`hwnd`, or the one in front) or "region" (`rect`, in pixels of the
/// desktop): the last two are what a selector would hand over.
fn begin(cx: &Ctx, what: &str, args: &Value) -> Result<Value, String> {
    let mut now = locked(&NOW);
    if now.is_some() {
        return Err("A recording is already running.".into());
    }
    let (what, target) = match what {
        "window" => ("window", Target::Window(window(cx, args["hwnd"].as_i64().map(|hwnd| hwnd as isize))?)),
        "region" => {
            let n = |key: &str| args["rect"][key].as_f64().map(|v| v.round() as i32);
            let rect = (|| Some(Rect { x: n("x")?, y: n("y")?, w: n("w")?, h: n("h")? }))().ok_or("That area cannot be recorded.")?;
            ("region", Target::Display(rect))
        }
        _ => ("display", Target::Display(display()?)),
    };
    let settings = cx.settings(ID);
    let dir = folder(cx);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} cannot be used: {e}", dir.display()))?;

    let mut tracks = Vec::new();
    if settings["systemAudio"].as_bool() != Some(false) {
        tracks.push(rec::system_sound());
    }
    if settings["microphone"].as_bool() == Some(true) {
        let id = settings["mic"].as_str().unwrap_or_default().to_string();
        tracks.push(Track { name: "microphone", open: Box::new(move || Ok(Box::new(mic::open(&id)?) as Box<dyn Sound>)) });
    }
    let options = Options {
        target,
        path: free_name(&dir),
        fps: settings["fps"].as_u64().unwrap_or(60) as u32,
        quality: Quality::named(settings["quality"].as_str().unwrap_or_default()),
        pointer: settings["pointer"].as_bool() != Some(false),
        border: settings["border"].as_bool() == Some(true),
        software: false,
        tracks,
    };
    // The island holds the recording's controls: it is not in the film. One that
    // is already left out of the captures (a screenshot in progress) is not ours to put back.
    let hidden = island(cx).filter(|hwnd| !svc_capture::excluded_from_capture(*hwnd) && svc_capture::exclude_from_capture(*hwnd, true));
    let told = cx.clone();
    let take = Recording::start(options, move |event| match event {
        Event::Silent { track, why } => {
            log::line(format!("recorder: the {track} track is silent: {why}"));
            told.emit(ID, "silent", json!({ "track": track }));
        }
        Event::Done(result) => ended(&told, result),
    })
    .inspect_err(|_| {
        if let Some(hwnd) = hidden {
            svc_capture::exclude_from_capture(hwnd, false);
        }
    })?;
    let info = &take.info;
    let encoder = if info.hardware { format!("the graphics card's encoder ({})", info.vendor) } else { "Windows' own encoder".into() };
    log::line(format!("recorder: recording a {what} at {}x{}, {} kbit/s at most, {encoder}", info.w, info.h, info.bit_rate / 1000));
    let session = svc_sysevents::on_session(|event| match event {
        svc_sysevents::Session::Lock => stop(Stop::Lock),
        svc_sysevents::Session::Sleep => stop(Stop::Sleep),
        _ => {}
    });
    *now = Some(Running { take, what, _session: session, island: hidden });
    let said = state_of(&now);
    cx.emit(ID, "state", said.clone());
    Ok(said)
}

#[derive(Default)]
pub struct Recorder;

impl Feature for Recorder {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn start(&self, _cx: &Ctx) -> Result<(), String> {
        Ok(())
    }

    /// A recording that runs is finished before this returns: the file is whole.
    fn stop(&self, _cx: &Ctx) {
        let thread = locked(&NOW).as_mut().and_then(|running| {
            running.take.stop(Stop::Off);
            running.take.thread()
        });
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    fn call(&self, cx: &Ctx, method: &str, args: Value) -> Result<Value, String> {
        let recording = || locked(&NOW).is_some();
        // A key was pressed: nobody waits for the answer, so a refusal goes to the island.
        let start = |what: &str| begin(cx, what, &args).inspect_err(|why| cx.emit(ID, "failed", json!({ "message": why })));
        match method {
            "shortcut" => match args["id"].as_str().unwrap_or_default() {
                "pause" => self.call(cx, "pause", Value::Null),
                _ if recording() => self.call(cx, "stop", Value::Null),
                "window" => start("window"),
                _ => start("display"),
            },
            "start" => start(args["what"].as_str().unwrap_or("display")),
            // The file is finished on the recording's thread; `saved` tells of it.
            "stop" => {
                stop(Stop::User);
                Ok(Value::Null)
            }
            "pause" => {
                let now = locked(&NOW);
                if let Some(running) = now.as_ref() {
                    running.take.pause(!running.take.paused());
                }
                let said = state_of(&now);
                cx.emit(ID, "state", said.clone());
                Ok(said)
            }
            "state" => Ok(state()),
            // The last recording, in the program that plays films, or selected in its folder.
            "open" | "reveal" => {
                let path = locked(&LAST).clone().filter(|path| path.is_file()).ok_or("That recording is not there any more.")?;
                if method == "open" { svc_shell::open(&path.to_string_lossy())? } else { svc_shell::reveal(&path)? }
                Ok(Value::Null)
            }
            "folder" => {
                let dir = folder(cx);
                std::fs::create_dir_all(&dir).map_err(|e| format!("{} cannot be used: {e}", dir.display()))?;
                win::reveal_folder(&dir.to_string_lossy());
                Ok(Value::Null)
            }
            // For the settings page: where files go when no folder is chosen, and the microphones.
            "caps" => {
                let mics: Vec<Value> = svc_audio::devices(Flow::Capture).unwrap_or_default().into_iter().map(|d| json!({ "id": d.id, "name": d.name })).collect();
                Ok(json!({ "folder": videos().join(notch_brand::NAME).to_string_lossy(), "mics": mics }))
            }
            _ => Err("unknown method".into()),
        }
    }

    fn strings(&self) -> &'static str {
        include_str!("../../../src/features/recorder/strings.json")
    }
}

pub fn feature() -> std::sync::Arc<dyn Feature> {
    std::sync::Arc::new(Recorder)
}
