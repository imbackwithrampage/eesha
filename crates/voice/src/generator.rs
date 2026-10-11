// The local text model's process: llama.cpp's `llama-server` with Qwen3.5-4B, on loopback through
// `crate::proc` (free port, listener checked, job), below-normal priority, the setting `threads` on the
// efficiency cores. Package voice-generator (VOICE §10 and §4.3; the files, hashes
// and flags are below).
//
// This module owns the process and its files only. The words go through the chat's headless completion
// (`chat.ask {provider: "builtin"}`), which is given the address `endpoint` returns. Started by the
// first turn that needs written text, ended after the setting `keepModel` minutes (default
// `timing::GENERATOR_IDLE`) without a request, and at once on voice off, lock, sleep, energy saver
// (`proc::kill_all`; the next `endpoint` starts it again). Files are fetched by the Settings button
// only, streamed to `.part`, hashed, renamed; the model is hashed again before the first start of each
// run (and again whenever its size or date changes).
//
// Nothing here needs a `Ctx` except `start`/`settings`: `init` and the `use_files`/`set_idle` overrides
// let a probe run the whole module without the app.

use crate::{event, proc, state, timing, Download, Emit, PauseReason, Result, State, VoiceError};
use notch_core::Ctx;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

// ── What is downloaded ────────────────────────────────────────────────────────

const ENGINE_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b11552/llama-b11552-bin-win-cpu-x64.zip";
const ENGINE_SHA: &str = "c00741179811ff903b515eae28bdeef1f47af6e81c49920c4e68b62954e056d6";
const ENGINE_BYTES: u64 = 19_528_406;
const MODEL_FILE: &str = "Qwen3.5-4B-Q4_K_M.gguf";
const MODEL_URL: &str = "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf";
const MODEL_SHA: &str = "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4";
const MODEL_BYTES: u64 = 2_740_937_888;
const MODEL_NAME: &str = "Qwen3.5-4B";

const BELOW_NORMAL: u32 = 0x4000;
/// Sent with the warm-up request (the server itself runs with `--reasoning off`): no hidden chain of thought (local-brain.md §6).
const NO_THINKING: &str = r#"{"enable_thinking":false}"#;
const DEFAULT_SYSTEM: &str = "You are a voice assistant on a Windows PC. Answer in one or two short spoken sentences.";

// ── Where things are ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct Cfg {
    /// `<data>/voice/generator`: the zip, the marker, the model.
    dir: PathBuf,
    engine: PathBuf,
    model: PathBuf,
    emit: Emit,
    threads: u32,
    idle: Duration,
}

static CFG: Mutex<Option<Cfg>> = Mutex::new(None);

/// Points the module at `dir` and the events at `emit`. `start` does it in the app.
pub fn init(dir: PathBuf, emit: Emit) {
    let _ = std::fs::create_dir_all(&dir);
    let mut cfg = CFG.lock().unwrap();
    let (threads, idle) = cfg.as_ref().map_or((2, timing::GENERATOR_IDLE), |c| (c.threads, c.idle));
    *cfg = Some(Cfg { engine: dir.join("llama"), model: dir.join(MODEL_FILE), dir, emit, threads, idle });
}

/// A probe's way to use files that are already on the machine (the engine folder holds `llama-server.exe`;
/// `engine.ok` in the data folder must still carry the zip's hash).
pub fn use_files(engine: PathBuf, model: PathBuf) {
    if let Some(c) = CFG.lock().unwrap().as_mut() {
        c.engine = engine;
        c.model = model;
    }
}

/// A probe's way to shorten the idle time.
pub fn set_idle(idle: Duration) {
    if let Some(c) = CFG.lock().unwrap().as_mut() {
        c.idle = idle;
    }
}

/// The marker a verified, extracted engine leaves.
pub fn mark_engine(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join("engine.ok"), ENGINE_SHA)
}

fn cfg() -> Result<Cfg> {
    CFG.lock().unwrap().clone().ok_or_else(|| VoiceError::Failed("the text model is not started".into()))
}

pub fn start(cx: &Ctx) -> Result<()> {
    init(cx.data_dir(crate::ID).join("generator"), crate::emitter(cx));
    settings(cx, &cx.settings(crate::ID));
    Ok(())
}

/// Ends the server and any download.
pub fn stop(_cx: &Ctx) {
    CANCEL.store(true, Ordering::SeqCst);
    unload();
}

/// `threads` (restarts the server if it runs with another count) and `keepModel` (minutes).
pub fn settings(_cx: &Ctx, settings: &Value) {
    let threads = settings["threads"].as_u64().map_or(2, |n| n.clamp(1, 8) as u32);
    let idle = settings["keepModel"].as_f64().filter(|m| *m > 0.0).map_or(timing::GENERATOR_IDLE, |m| Duration::from_secs_f64(m * 60.0));
    let changed = {
        let mut cfg = CFG.lock().unwrap();
        let Some(c) = cfg.as_mut() else { return };
        let changed = c.threads != threads;
        c.threads = threads;
        c.idle = idle;
        changed
    };
    if changed {
        unload();
    }
}

/// `status` → `{installed, downloading, running, model}` · `download` → `{}` (progress as
/// `voice:download {what: "generator"}`) · `remove` → `{}` · `endpoint` → `{url, model}` (starts the
/// server if it must; what the page hands to `chat.ask`) · `touch` → `{}` (a token arrived: the idle
/// clock starts again) · `unload` → `{}` · `prefix {messages, tools?}` → `{}` (the system prompt and
/// tools the first real request will begin with, which the warm-up caches).
pub fn call(_cx: &Ctx, method: &str, args: Value) -> Result<Value> {
    match method {
        "status" => Ok(json!({ "installed": ready(), "downloading": DOWNLOADING.load(Ordering::SeqCst), "running": running(), "model": MODEL_NAME })),
        "download" => download().map(|_| json!({})),
        "remove" => remove().map(|_| json!({})),
        "endpoint" => Ok(json!({ "url": endpoint()?, "model": MODEL_NAME })),
        "touch" => {
            touch();
            Ok(json!({}))
        }
        "unload" => {
            unload();
            Ok(json!({}))
        }
        "prefix" => {
            *PREFIX.lock().unwrap() = Some(args);
            Ok(json!({}))
        }
        _ => Err(VoiceError::Failed("unknown method".into())),
    }
}

/// Engine and model are downloaded (and the model is the size it should be; its hash is checked at the
/// first start).
pub fn ready() -> bool {
    cfg().is_ok_and(|c| engine_ok(&c) && std::fs::metadata(&c.model).is_ok_and(|m| m.len() == MODEL_BYTES))
}

fn engine_ok(c: &Cfg) -> bool {
    c.engine.join("llama-server.exe").is_file() && std::fs::read_to_string(c.dir.join("engine.ok")).is_ok_and(|s| s == ENGINE_SHA)
}

// ── Files: fetch, hash, unzip ─────────────────────────────────────────────────

static DOWNLOADING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Streams `url` to `<dest>.part` (resuming a part that is there), checks `sha256`, and only then renames
/// it to `dest`. A wrong hash deletes the part and is an error. `progress(received, total)` runs about
/// four times a second. https only (plain http only from this machine); redirects are followed.
pub fn fetch(url: &str, dest: &Path, sha256: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let fail = |e: String| VoiceError::Failed(e);
    let url = notch_core::net::fetchable(url).map_err(fail)?;
    tauri::async_runtime::block_on(async {
        let client = notch_core::net::client(&url, Duration::from_secs(3 * 3600)).map_err(fail)?;
        let have = std::fs::metadata(&part).map_or(0, |m| m.len());
        let mut request = client.get(url.clone());
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request.send().await.map_err(|e| fail(format!("download: {e}")))?;
        let status = response.status().as_u16();
        if status == 416 {
            // The part is longer than the file or the server will not resume: start again next click.
            let _ = std::fs::remove_file(&part);
            return Err(fail("the partial download was not usable and was deleted; try again".into()));
        }
        if !response.status().is_success() {
            return Err(fail(format!("download: HTTP {status}")));
        }
        if response.url().scheme() != "https" && !notch_core::net::is_loopback_url(response.url()) {
            return Err(fail("the download was redirected away from https".into()));
        }
        let resumed = status == 206;
        let mut received = if resumed { have } else { 0 };
        let total = received + response.content_length().unwrap_or(0);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)
            .map_err(|e| fail(format!("download: {e}")))?;
        let mut last = Instant::now();
        while let Some(chunk) = response.chunk().await.map_err(|e| fail(format!("download: {e}")))? {
            if CANCEL.load(Ordering::SeqCst) {
                return Err(VoiceError::Cancelled);
            }
            file.write_all(&chunk).map_err(|e| fail(format!("download: {e}")))?;
            received += chunk.len() as u64;
            if last.elapsed() >= Duration::from_millis(250) {
                last = Instant::now();
                progress(received, total);
            }
        }
        progress(received, total);
        Ok(())
    })?;
    let got = hash_file(&part).map_err(|e| fail(e.to_string()))?;
    if got != sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(fail("checksum mismatch, the download was deleted".into()));
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&part, dest).map_err(|e| fail(e.to_string()))
}

/// Windows' own `tar.exe` (bsdtar, in System32 since Windows 10 1803) reads zip files.
pub fn unzip(zip: &Path, to: &Path) -> Result<()> {
    let tar = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("tar.exe");
    let mut cmd = std::process::Command::new(tar);
    cmd.arg("-xf").arg(zip).arg("-C").arg(to);
    notch_core::win::no_console(&mut cmd);
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        _ => Err(VoiceError::Failed("could not unpack the engine".into())),
    }
}

fn model_ok(c: &Cfg) -> bool {
    verify_model(c).is_ok()
}

static VERIFIED: Mutex<Option<(u64, SystemTime)>> = Mutex::new(None);

/// The model is the right size and hashes to `MODEL_SHA`; a wrong file is deleted. Hashing 2.7 GB takes a
/// few seconds, so it is done once per size-and-date, not at every start.
fn verify_model(c: &Cfg) -> Result<()> {
    let meta = std::fs::metadata(&c.model).map_err(|_| VoiceError::Missing("the text model".into()))?;
    let stamp = (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    if *VERIFIED.lock().unwrap() == Some(stamp) {
        return Ok(());
    }
    if meta.len() != MODEL_BYTES || hash_file(&c.model).map_err(|e| VoiceError::Failed(e.to_string()))? != MODEL_SHA {
        let _ = std::fs::remove_file(&c.model);
        return Err(VoiceError::Missing("the text model (checksum)".into()));
    }
    *VERIFIED.lock().unwrap() = Some(stamp);
    Ok(())
}

/// Starts the download on a thread of its own and returns; progress and the end are `voice:download`.
fn download() -> Result<()> {
    let c = cfg()?;
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    CANCEL.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        let total = ENGINE_BYTES + MODEL_BYTES;
        let send = |received: u64, done: bool, error: Option<String>| {
            (c.emit)(event::DOWNLOAD, json!(Download { what: "generator".into(), received, total, done, error }))
        };
        let result = (|| -> Result<()> {
            if !engine_ok(&c) {
                let zip = c.dir.join("llama.zip");
                fetch(ENGINE_URL, &zip, ENGINE_SHA, &mut |r, _| send(r, false, None))?;
                let fresh = c.dir.join("llama.new");
                let _ = std::fs::remove_dir_all(&fresh);
                std::fs::create_dir_all(&fresh).map_err(|e| VoiceError::Failed(e.to_string()))?;
                unzip(&zip, &fresh)?;
                if !fresh.join("llama-server.exe").is_file() {
                    return Err(VoiceError::Failed("the engine archive has no llama-server.exe".into()));
                }
                let _ = std::fs::remove_dir_all(&c.engine);
                std::fs::rename(&fresh, &c.engine).map_err(|e| VoiceError::Failed(e.to_string()))?;
                let _ = std::fs::remove_file(&zip);
                mark_engine(&c.dir).map_err(|e| VoiceError::Failed(e.to_string()))?;
            }
            if !model_ok(&c) {
                fetch(MODEL_URL, &c.model, MODEL_SHA, &mut |r, _| send(ENGINE_BYTES + r, false, None))?;
                // Just hashed by `fetch`: the first start need not hash it again.
                if let Ok(m) = std::fs::metadata(&c.model) {
                    *VERIFIED.lock().unwrap() = Some((m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
                }
            }
            Ok(())
        })();
        DOWNLOADING.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => send(total, true, None),
            Err(VoiceError::Cancelled) => send(0, true, Some("cancelled".into())),
            Err(e) => send(0, true, Some(e.to_string())),
        }
    });
    Ok(())
}

fn remove() -> Result<()> {
    unload();
    let c = cfg()?;
    let _ = std::fs::remove_file(&c.model);
    let _ = std::fs::remove_dir_all(&c.engine);
    let _ = std::fs::remove_file(c.dir.join("engine.ok"));
    *VERIFIED.lock().unwrap() = None;
    Ok(())
}

// ── The process ───────────────────────────────────────────────────────────────

struct Server {
    child: proc::Child,
    port: u16,
    id: u64,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
/// One start at a time; callers of `endpoint` wait for it.
static START: Mutex<()> = Mutex::new(());
static LAST: Mutex<Option<Instant>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// The idle watcher sleeps on this; `unload` wakes it.
static WAKE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());
static PREFIX: Mutex<Option<Value>> = Mutex::new(None);

pub fn running() -> bool {
    SERVER.lock().unwrap().as_mut().is_some_and(|s| s.child.alive())
}

/// A request is made or a token arrived: the idle clock starts again.
pub fn touch() {
    *LAST.lock().unwrap() = Some(Instant::now());
}

/// Ends the server now.
pub fn unload() {
    let server = SERVER.lock().unwrap().take();
    if server.is_some() {
        notch_core::log::line("voice: generator unloaded");
    }
    drop(server);
    WAKE.1.notify_all();
}

/// `http://127.0.0.1:<port>` of the running server (OpenAI-compatible), starting it and waiting until
/// it answers its health check (`timing::CHILD_START` to listen, three times that to load the model).
pub fn endpoint() -> Result<String> {
    if matches!(state(), State::Paused(PauseReason::Lock | PauseReason::Sleep | PauseReason::EnergySaver | PauseReason::Tray)) {
        return Err(VoiceError::Refused("voice is paused".into()));
    }
    let _starting = START.lock().unwrap();
    touch();
    {
        let mut server = SERVER.lock().unwrap();
        if let Some(s) = server.as_mut() {
            if s.child.alive() {
                return Ok(format!("http://127.0.0.1:{}", s.port));
            }
        }
        *server = None;
    }
    let c = cfg()?;
    if !engine_ok(&c) {
        return Err(VoiceError::Missing("the text model's engine".into()));
    }
    verify_model(&c)?;
    let port = proc::free_port()?;
    // Absolute, because the server runs in its own folder (it finds its DLLs there).
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut cmd = std::process::Command::new(abs(&c.engine).join("llama-server.exe"));
    cmd.current_dir(abs(&c.engine))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .arg("-m")
        .arg(abs(&c.model))
        .args(["-t", &c.threads.to_string(), "-c", "2048", "-np", "1", "--jinja", "--no-webui", "--reasoning", "off"])
        .args(["--host", "127.0.0.1", "--port", &port.to_string()]);
    let mut child = proc::spawn(&mut cmd)?;
    limit(&child);
    proc::wait_listening(&mut child, port, timing::CHILD_START)?;
    let end = Instant::now() + timing::CHILD_START * 3;
    while !healthy(port) {
        if !child.alive() {
            return Err(VoiceError::Failed("the text model ended while loading".into()));
        }
        if Instant::now() >= end {
            return Err(VoiceError::Timeout("the text model"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    *SERVER.lock().unwrap() = Some(Server { child, port, id });
    touch();
    notch_core::log::line(format!("voice: generator up, port {port}"));
    std::thread::spawn(move || watch(id));
    std::thread::spawn(move || warm(id, port));
    Ok(format!("http://127.0.0.1:{port}"))
}

fn healthy(port: u16) -> bool {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .and_then(|c| c.get(format!("http://127.0.0.1:{port}/health")).send())
        .is_ok_and(|r| r.status().as_u16() == 200)
}

/// Kills server `id` once nothing asked for `idle`; ends early if it is gone or replaced.
fn watch(id: u64) {
    loop {
        let left = {
            let mut server = SERVER.lock().unwrap();
            match server.as_mut().map(|s| (s.id == id, s.child.alive())) {
                Some((true, true)) => {}
                Some((true, false)) => {
                    *server = None;
                    return;
                }
                _ => return,
            }
            let idle = cfg().map_or(timing::GENERATOR_IDLE, |c| c.idle);
            let last = LAST.lock().unwrap().unwrap_or_else(Instant::now);
            (last + idle).saturating_duration_since(Instant::now())
        };
        if left.is_zero() {
            let mut server = SERVER.lock().unwrap();
            if server.as_ref().is_some_and(|s| s.id == id) {
                *server = None;
                notch_core::log::line("voice: generator unloaded after idle");
            }
            return;
        }
        let guard = WAKE.0.lock().unwrap();
        let _ = WAKE.1.wait_timeout(guard, left);
    }
}

/// One request that only fills the prompt cache with the system prompt and tools, so the first real
/// request skips them. It queues in front of a real one (one slot) and does not move the idle clock.
fn warm(id: u64, port: u16) {
    let prefix = PREFIX.lock().unwrap().clone().unwrap_or_else(|| json!({ "messages": [{ "role": "system", "content": DEFAULT_SYSTEM }] }));
    let mut messages = prefix["messages"].as_array().cloned().unwrap_or_default();
    messages.push(json!({ "role": "user", "content": "Hi." }));
    let mut body = json!({
        "messages": messages, "max_tokens": 1, "temperature": 0, "cache_prompt": true,
        "chat_template_kwargs": serde_json::from_str::<Value>(NO_THINKING).unwrap_or_default(),
    });
    if let Some(tools) = prefix.get("tools") {
        body["tools"] = tools.clone();
    }
    let started = Instant::now();
    let ok = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .build()
        .and_then(|c| c.post(format!("http://127.0.0.1:{port}/v1/chat/completions")).json(&body).send())
        .is_ok_and(|r| r.status().is_success());
    if SERVER.lock().unwrap().as_ref().is_some_and(|s| s.id == id) {
        notch_core::log::line(format!("voice: generator warm-up {} in {} ms", if ok { "done" } else { "failed" }, started.elapsed().as_millis()));
    }
}

// ── Priority and cores ────────────────────────────────────────────────────────

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::SystemInformation::{GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION};
use windows::Win32::System::Threading::{
    GetPriorityClass, GetProcessAffinityMask, SetPriorityClass, SetProcessAffinityMask, PROCESS_CREATION_FLAGS,
};

/// Logical processors (group 0) whose efficiency class is below the highest: the efficiency cores. `None`
/// when every core has the same class (no hybrid CPU) or the query fails.
fn efficiency_mask() -> Option<usize> {
    let mut len = 0u32;
    unsafe {
        let _ = GetSystemCpuSetInformation(None, 0, &mut len, None, Some(0));
    }
    if len == 0 {
        return None;
    }
    // u64 elements: the records need 8-byte alignment.
    let mut buf = vec![0u64; len as usize / 8 + 1];
    let base = buf.as_mut_ptr() as *mut SYSTEM_CPU_SET_INFORMATION;
    if !unsafe { GetSystemCpuSetInformation(Some(base), len, &mut len, None, Some(0)) }.as_bool() {
        return None;
    }
    let mut cpus: Vec<(u32, u8)> = Vec::new();
    let mut at = 0usize;
    while at + 8 <= len as usize {
        let rec = unsafe { &*((base as *const u8).add(at) as *const SYSTEM_CPU_SET_INFORMATION) };
        if rec.Size == 0 {
            break;
        }
        // Type 0 is CpuSetInformation, the only kind there is.
        if rec.Type.0 == 0 {
            let cpu = unsafe { rec.Anonymous.CpuSet };
            if cpu.Group == 0 && (cpu.LogicalProcessorIndex as usize) < usize::BITS as usize {
                cpus.push((cpu.LogicalProcessorIndex as u32, cpu.EfficiencyClass));
            }
        }
        at += rec.Size as usize;
    }
    let top = cpus.iter().map(|c| c.1).max()?;
    let mask = cpus.iter().filter(|c| c.1 < top).fold(0usize, |m, c| m | 1 << c.0);
    (mask != 0).then_some(mask)
}

/// Below-normal priority, and the efficiency cores when the CPU has them. Failures are not fatal: the
/// model then simply runs at normal priority.
fn limit(child: &proc::Child) {
    let h = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&child.0) as _);
    unsafe {
        let _ = SetPriorityClass(h, PROCESS_CREATION_FLAGS(BELOW_NORMAL));
        if let Some(mask) = efficiency_mask() {
            let _ = SetProcessAffinityMask(h, mask);
        }
    }
}

/// What is set on the running server, read back from the process.
#[derive(Debug, Clone, Copy)]
pub struct Diag {
    pub pid: u32,
    pub port: u16,
    pub priority: u32,
    pub affinity: usize,
    /// The efficiency cores the CPU reports, if it has any.
    pub efficiency: Option<usize>,
}

pub fn diagnostics() -> Option<Diag> {
    let server = SERVER.lock().unwrap();
    let s = server.as_ref()?;
    let h = HANDLE(std::os::windows::io::AsRawHandle::as_raw_handle(&s.child.0) as _);
    let (mut mine, mut system) = (0usize, 0usize);
    unsafe {
        GetProcessAffinityMask(h, &mut mine, &mut system).ok()?;
        Some(Diag { pid: s.child.pid(), port: s.port, priority: GetPriorityClass(h), affinity: mine, efficiency: efficiency_mask() })
    }
}
