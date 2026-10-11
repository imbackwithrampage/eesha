// The action model: `needle.exe --model needle3.cact --tools … --serve` on loopback, through
// `crate::proc` (free port, listener checked, job). Package voice-needle (VOICE §10; §4.2).
//
// Started by `warm` at the first word of a command, ready in about 1.6 s, gone `timing::NEEDLE_IDLE`
// after its last use. Each request is `/reset`, then `POST /complete {"input"}`, within
// `timing::NEEDLE_TIMEOUT`. This module only asks the model: grounding, the trigger rule and the
// decision are the page's (`brain/needle.ts`). The model is downloaded on a click and checked by
// SHA-256 before it is kept and before each start. The engine is the dictation feature's `needle.exe`.
//
// The engine reads every tool when it starts (measured: 5 tools 2.0 s, 9 3.2 s, 24 6.3 s, 60 18 s on a
// busy machine; 9 in 1.6 s on a quiet one), so the tools are few: a short registry whole, a long one as
// the page's list for one sentence. New tools mean a new server.
//
// Nothing of a sentence is logged: starts, ends and their timings only.

use crate::{event, generator, proc, timing, Download, Result, VoiceError};
use notch_core::Ctx;
use notch_feat_dictation::whistle;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::System::SystemInformation::GetLocalTime;

const WHAT: &str = "the action model";

/// 35 MB, from the repository revision the engine is pinned to (`whistle::FILES`).
const MODEL: whistle::File = whistle::File {
    name: "needle3.cact",
    url: "https://huggingface.co/Cactus-Compute/needle3/resolve/93e88ede867e4311c39151285e74df513e4b835b/needle3.cact",
    sha256: "c9d915eca282ed42d1a09b143b592adb4cc6744ffe2d294adf5cfc5548170c38",
    max: 35_335_380,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub tool: String,
    /// An object.
    pub args: Value,
}

/// What the model chose for a sentence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// The tool's name; `no_action` is a tool too. Empty when the model called nothing.
    pub tool: String,
    /// The call's arguments, an object.
    pub args: Value,
    pub confidence: f32,
    /// The engine itself held the call back (`tool` and `args` are what it held).
    pub suppressed: bool,
    /// The further calls of the same sentence, in the order said ("pause the music and set the volume to 20").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<Call>,
}

// ── Where things are, and the switch ──────────────────────────────────────────

#[derive(Clone)]
struct Files {
    engine: PathBuf,
    model: PathBuf,
    /// The tools and the facts are written here for the engine to read.
    work: PathBuf,
}

static FILES: Mutex<Option<Files>> = Mutex::new(None);
/// The setting `needle`, and false while voice is stopped: nothing starts then.
static ON: AtomicBool = AtomicBool::new(false);
/// The tools as the page last gave them, and how many times they changed.
static TOOLS: Mutex<(u64, Vec<Value>)> = Mutex::new((0, Vec::new()));
/// They are the whole registry, not one sentence's: worth starting the server at a command's first word.
static WHOLE: AtomicBool = AtomicBool::new(false);
static DOWNLOADING: AtomicBool = AtomicBool::new(false);
/// A probe's idle time and timeout in ms; 0 is the row of `timing`.
static TUNE: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

fn tuned(i: usize, row: Duration) -> Duration {
    match TUNE[i].load(SeqCst) {
        0 => row,
        ms => Duration::from_millis(ms),
    }
}

fn idle() -> Duration {
    tuned(0, timing::NEEDLE_IDLE)
}

fn timeout() -> Duration {
    tuned(1, timing::NEEDLE_TIMEOUT)
}

/// A probe's way to shorten the idle time and the timeout; zero gives the row of `timing` back.
#[doc(hidden)]
pub fn tune(idle: Duration, timeout: Duration) {
    TUNE[0].store(idle.as_millis() as u64, SeqCst);
    TUNE[1].store(timeout.as_millis() as u64, SeqCst);
}

/// Where the engine and the model are, and the folder the tools and facts files go to. `start` does it in the app.
pub fn locate(engine: PathBuf, model: PathBuf, work: PathBuf) {
    *FILES.lock().unwrap() = Some(Files { engine, model, work });
}

/// The setting `needle`. Off ends the server and keeps it from starting.
pub fn switch(on: bool) {
    ON.store(on, SeqCst);
    if !on {
        unload();
    }
}

pub fn start(cx: &Ctx) -> Result<()> {
    let dir = cx.data_dir(crate::ID);
    locate(cx.data_dir("dictation").join("needle.exe"), dir.join(MODEL.name), dir);
    switch(cx.settings(crate::ID)["needle"] != false);
    Ok(())
}

/// Ends the server. (`crate::pause` ends it through `proc::kill_all`; the next request starts it again.)
pub fn stop(_cx: &Ctx) {
    switch(false);
}

pub fn settings(_cx: &Ctx, settings: &Value) {
    switch(settings["needle"] != false);
}

/// `tools {tools: [...], sentence?, warm?}` → `{}` (the registry's tools, or with `sentence` those for
/// one sentence; a change restarts the server at its next use, or now with `warm`) · `choose {text,
/// system?}` → `Choice` · `status` → `{installed, downloading, running}` · `download` → `{}` (progress as
/// `voice:download {what: "needle"}`).
pub fn call(cx: &Ctx, method: &str, args: Value) -> Result<Value> {
    match method {
        "tools" => {
            set_tools(&args["tools"])?;
            WHOLE.store(args["sentence"] != true, SeqCst);
            if args["warm"] == true {
                kick(&mut SLOT.lock().unwrap(), "");
            }
            Ok(json!({}))
        }
        "choose" => choose(args["text"].as_str().unwrap_or(""), args["system"].as_str().unwrap_or("")).map(|c| json!(c)),
        "status" => Ok(json!({ "installed": ready(), "downloading": DOWNLOADING.load(SeqCst), "running": running().is_some() })),
        "download" => {
            let files = files()?;
            if !DOWNLOADING.swap(true, SeqCst) {
                let emit = crate::emitter(cx);
                std::thread::spawn(move || {
                    let total = MODEL.max as u64;
                    let tell = |received, done, error| emit(event::DOWNLOAD, json!(Download { what: "needle".into(), received, total, done, error }));
                    let result = download(&files, &mut |received, _| tell(received, false, None));
                    DOWNLOADING.store(false, SeqCst);
                    match result {
                        Ok(()) => tell(total, true, None),
                        Err(e) => tell(0, true, Some(e.to_string())),
                    }
                });
            }
            Ok(json!({}))
        }
        _ => Err(VoiceError::Failed("unknown method".into())),
    }
}

fn files() -> Result<Files> {
    FILES.lock().unwrap().clone().ok_or(VoiceError::Missing(WHAT.into()))
}

/// The engine (with dictation's speech model, which voice needs as well), then the model.
fn download(files: &Files, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    if let Some(dir) = files.engine.parent() {
        tauri::async_runtime::block_on(whistle::download(dir, |_| {})).map_err(VoiceError::Failed)?;
    }
    if !is(&files.model, MODEL.sha256) {
        // ponytail: the text model's `fetch` (part file, hash, rename) and so its cancel flag: cancelling that
        // download ends this one too, and the next click resumes it.
        generator::fetch(MODEL.url, &files.model, MODEL.sha256, progress)?;
    }
    Ok(())
}

fn is(path: &Path, sha256: &str) -> bool {
    generator::hash_file(path).is_ok_and(|hash| hash == sha256)
}

/// Engine and model are there and are the pinned ones.
fn intact(files: &Files) -> bool {
    whistle::FILES.iter().any(|engine| engine.name == "needle.exe" && is(&files.engine, engine.sha256)) && is(&files.model, MODEL.sha256)
}

/// The model is downloaded and matches its hash.
pub fn ready() -> bool {
    files().is_ok_and(|f| intact(&f))
}

/// The tools the model chooses among: `[{name, description, parameters}]`, `no_action` among them
/// (`needleTools` of the page). Answers how many; a change restarts the server at its next use.
pub fn set_tools(tools: &Value) -> Result<usize> {
    WHOLE.store(true, SeqCst);
    let list = tools.as_array().filter(|l| l.iter().all(|t| t["name"].is_string())).ok_or_else(|| VoiceError::Failed("tools: a list of named tools".into()))?;
    let mut now = TOOLS.lock().unwrap();
    if now.1 != *list {
        *now = (now.0 + 1, list.clone());
    }
    Ok(list.len())
}

// ── The server ────────────────────────────────────────────────────────────────

struct Server {
    child: proc::Child,
    port: u16,
    /// Which tools it was started with (`TOOLS.0`).
    tools: u64,
    last: Instant,
    /// Dropped with the server: its `watch` thread ends at once.
    _bye: mpsc::Sender<()>,
}

enum Slot {
    Off,
    /// With which tools (`TOOLS.0`).
    Starting(u64),
    Up(Server),
    Failed(VoiceError),
}

static SLOT: Mutex<Slot> = Mutex::new(Slot::Off);
/// A start ended, one way or the other.
static CHANGED: Condvar = Condvar::new();
/// One sentence at a time: the engine holds one conversation.
static REQUEST: Mutex<()> = Mutex::new(());

/// The process id of the server while it runs.
pub fn running() -> Option<u32> {
    match &mut *SLOT.lock().unwrap() {
        Slot::Up(server) => server.child.alive().then(|| server.child.pid()),
        _ => None,
    }
}

/// Ends the server now (dropping it ends the process); the next `warm` or `choose` starts a new one.
pub fn unload() {
    *SLOT.lock().unwrap() = Slot::Off;
    CHANGED.notify_all();
}

/// `date: 2026-10-11 Sun; locale: en-US; device: pc` (§4.2). Without the time of day: measured, the
/// clock changes answers ("I have to leave in 25 minutes, can you time that" was held back at 05:50 and
/// at 21:17, right at 21:41 and without a time), and no argument needs it.
// ponytail: the date is the server's start; it lives three idle minutes and no argument kind carries a
// date, so only a session across midnight is a day behind. Restart on a new day if one ever does.
fn facts() -> String {
    let t = unsafe { GetLocalTime() };
    let day = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][t.wDayOfWeek as usize % 7];
    format!("date: {:04}-{:02}-{:02} {day}; locale: en-US; device: pc", t.wYear, t.wMonth, t.wDay)
}

fn launch(system: &str, tools: u64, list: Vec<Value>) -> Result<(Server, mpsc::Receiver<()>)> {
    let from = Instant::now();
    let files = files()?;
    if !intact(&files) {
        return Err(VoiceError::Missing(WHAT.into()));
    }
    if list.is_empty() {
        return Err(VoiceError::Failed(format!("{WHAT} has no tools yet")));
    }
    let io = |e: std::io::Error| VoiceError::Failed(format!("{WHAT}: {e}"));
    let (tools_file, facts_file) = (files.work.join("needle-tools.json"), files.work.join("needle-facts.txt"));
    std::fs::create_dir_all(&files.work).map_err(io)?;
    std::fs::write(&tools_file, Value::Array(list).to_string()).map_err(io)?;
    std::fs::write(&facts_file, if system.is_empty() { facts() } else { system.to_string() }).map_err(io)?;
    let port = proc::free_port()?;
    let mut child = proc::spawn(
        Command::new(&files.engine)
            .arg("--model")
            .arg(&files.model)
            .arg("--tools")
            .arg(&tools_file)
            .arg("--system")
            .arg(&facts_file)
            .args(["--serve", "--port", &port.to_string()])
            .current_dir(&files.work)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?;
    proc::wait_listening(&mut child, port, timing::CHILD_START)?;
    notch_core::log::line(format!("voice: needle up in {} ms", from.elapsed().as_millis()));
    let (bye, gone) = mpsc::channel();
    Ok((Server { child, port, tools, last: Instant::now(), _bye: bye }, gone))
}

/// Sleeps until the server has been idle long enough, then ends it. One per server; it ends with it.
fn watch(gone: mpsc::Receiver<()>) {
    loop {
        let wait = match &*SLOT.lock().unwrap() {
            Slot::Up(server) => (server.last + idle()).saturating_duration_since(Instant::now()),
            _ => return,
        };
        if gone.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
            return;
        }
        let mut slot = SLOT.lock().unwrap();
        if matches!(&*slot, Slot::Up(server) if server.last.elapsed() >= idle()) {
            *slot = Slot::Off;
            notch_core::log::line("voice: needle gone (idle)");
            return;
        }
    }
}

/// The server runs with the tools of now, or is being started with them.
fn current(slot: &mut Slot) -> bool {
    let tools = TOOLS.lock().unwrap().0;
    match slot {
        Slot::Starting(with) => *with == tools,
        Slot::Up(server) => server.tools == tools && server.child.alive(),
        _ => false,
    }
}

/// Starts the server unless it is `current` (it is then counted as used). A server, or a start, with
/// other tools is dropped, and that ends it.
fn kick(slot: &mut Slot, system: &str) {
    if !ON.load(SeqCst) {
        return;
    }
    if current(slot) {
        if let Slot::Up(server) = slot {
            server.last = Instant::now();
        }
        return;
    }
    let (tools, list) = TOOLS.lock().unwrap().clone();
    *slot = Slot::Starting(tools);
    let system = system.to_string();
    std::thread::spawn(move || {
        let started = launch(&system, tools, list);
        let mut slot = SLOT.lock().unwrap();
        // Anything else: voice went off or the tools changed meanwhile, and dropping the new server ends it.
        if matches!(*slot, Slot::Starting(with) if with == tools) {
            *slot = match started {
                Ok((server, gone)) => {
                    std::thread::spawn(move || watch(gone));
                    Slot::Up(server)
                }
                Err(e) => {
                    notch_core::log::line(format!("voice: needle did not start: {e}"));
                    Slot::Failed(e)
                }
            };
        }
        CHANGED.notify_all();
    });
}

/// The port of the running server, started first if it must be; waits for that until `end`.
fn up(system: &str, end: Instant) -> Result<u16> {
    let mut slot = SLOT.lock().unwrap();
    let mut asked = false;
    loop {
        if !ON.load(SeqCst) {
            return Err(VoiceError::Refused(format!("{WHAT} is switched off")));
        }
        if let (Slot::Failed(e), true) = (&*slot, asked) {
            return Err(e.clone());
        }
        kick(&mut slot, system);
        asked = true;
        if let Slot::Up(server) = &*slot {
            return Ok(server.port);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            // The start goes on: the next sentence finds the server up.
            return Err(VoiceError::Timeout(WHAT));
        }
        slot = CHANGED.wait_timeout(slot, left).unwrap().0;
    }
}

/// Starts the server if it is not running and returns at once: `ears` calls it at the first word of a
/// command (not at the wake). Does nothing when the model is not there, when the tools are one
/// sentence's or none (the next sentence brings its own: the page's `prepare`) or when the setting
/// `needle` is off. (The hashes are checked by the start itself, off this thread.)
pub fn warm() {
    if WHOLE.load(SeqCst) && files().is_ok_and(|f| f.model.is_file()) && !TOOLS.lock().unwrap().1.is_empty() {
        kick(&mut SLOT.lock().unwrap(), "");
    }
}

/// One sentence in, the model's call out. Blocks up to `timing::NEEDLE_TIMEOUT` when the server runs, and
/// `timing::NEEDLE_READY` longer when it has to be started or is starting. `system` is the line of §4.2
/// (`date: …; locale: en-US; device: pc`) for a server this call starts; empty is the line of today, and
/// a running server keeps the one it has.
pub fn choose(text: &str, system: &str) -> Result<Choice> {
    let (text, system) = (text.trim().to_string(), system.to_string());
    if text.is_empty() {
        return Ok(Choice { tool: String::new(), args: json!({}), confidence: 0.0, suppressed: false, more: Vec::new() });
    }
    let running = {
        let mut slot = SLOT.lock().unwrap();
        matches!(*slot, Slot::Up(_)) && current(&mut slot)
    };
    let wait = timeout() + if running { Duration::ZERO } else { timing::NEEDLE_READY };
    let end = Instant::now() + wait;
    let (answer, heard) = mpsc::channel();
    // On a thread of our own: `reqwest::blocking` does not run on an async runtime's thread, and the
    // caller leaves at `end` whatever the engine does.
    std::thread::spawn(move || {
        let _ = answer.send(ask(&text, &system, end));
    });
    heard.recv_timeout(wait).unwrap_or(Err(VoiceError::Timeout(WHAT)))
}

fn ask(text: &str, system: &str, end: Instant) -> Result<Choice> {
    let port = up(system, end)?;
    // Held whether or not an earlier request panicked.
    let _one = REQUEST.lock();
    // The request gets the whole timeout even after a slow start: an engine that answers late is slow,
    // one that does not answer within it is stuck.
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .timeout(timeout())
        .build()
        .map_err(|e| VoiceError::Failed(format!("{WHAT}: {e}")))?;
    let post = |path: &str, body: String| {
        client
            .post(format!("http://127.0.0.1:{port}{path}"))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .and_then(|reply| reply.error_for_status())
            .and_then(|reply| reply.text())
    };
    let answer = post("/reset", String::new()).and_then(|_| post("/complete", json!({ "input": text }).to_string()));
    let mut slot = SLOT.lock().unwrap();
    let ours = matches!(&*slot, Slot::Up(server) if server.port == port);
    match answer {
        Ok(body) => {
            if let (Slot::Up(server), true) = (&mut *slot, ours) {
                server.last = Instant::now();
            }
            parse(&body)
        }
        Err(e) => {
            // An engine that does not answer is ended; the next sentence starts a new one.
            if ours {
                *slot = Slot::Off;
            }
            Err(if e.is_timeout() { VoiceError::Timeout(WHAT) } else { VoiceError::Failed(format!("{WHAT} did not answer")) })
        }
    }
}

/// `{function_calls: [{name, arguments}], suppressed_calls: [...], confidence}` → `Choice`.
fn parse(body: &str) -> Result<Choice> {
    let v: Value = serde_json::from_str(body).map_err(|_| VoiceError::Failed(format!("{WHAT} gave no answer")))?;
    let calls = |key: &str| -> Vec<Call> {
        let list = v[key].as_array().into_iter().flatten();
        list.filter_map(|c| Some(Call { tool: c["name"].as_str()?.to_string(), args: if c["arguments"].is_object() { c["arguments"].clone() } else { json!({}) } }))
            .collect()
    };
    let (said, held) = (calls("function_calls"), calls("suppressed_calls"));
    let suppressed = said.is_empty() && !held.is_empty();
    let mut all = if suppressed { held } else { said }.into_iter();
    let first = all.next().unwrap_or(Call { tool: String::new(), args: json!({}) });
    Ok(Choice { tool: first.tool, args: first.args, confidence: v["confidence"].as_f64().unwrap_or(0.0) as f32, suppressed, more: all.collect() })
}
