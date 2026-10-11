// Mouth: the sentence queue between the page and the player. Filled by package voice-mouth-win
// (VOICE §10; read §7.1, §7.3, §7.4).
//
// `mouth.say {gen, seq, text}` queues a chunk; synthesis runs at most `timing::SYNTH_AHEAD` chunks ahead
// of the player; the voice is picked by `VoiceDesc::split` of the setting `voice` (Kokoro when ready,
// else Windows; ElevenLabs when chosen), and a chunk its voice fails on goes to the Windows voice. Sets
// speaking while the player plays, emits `voice:spoke` as a chunk starts and `voice:level {out}`.

use crate::{eleven, player, sapi, speech, timing, AudioChunk, Emit, Engine, Result, SynthRequest, VoiceDesc, VoiceError};
use notch_core::Ctx;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The arguments of `mouth.say`. A `gen` older than `crate::generation()` is dropped without an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Say {
    pub gen: u64,
    /// Order within the generation, from 0.
    pub seq: u32,
    pub text: String,
    /// A `VoiceDesc::id` for this chunk only (the picker's Test); else the setting.
    #[serde(default)]
    pub voice: Option<String>,
}

#[derive(Clone)]
struct Settings {
    voice: String,
    kokoro_voice: String,
    eleven_voice: String,
    rate: f32,
    win_rate: f32,
    win_pitch: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { voice: String::new(), kokoro_voice: "af_heart".into(), eleven_voice: String::new(), rate: 1.0, win_rate: 1.1, win_pitch: 1.0 }
    }
}

#[derive(Default)]
struct Shared {
    jobs: VecDeque<Say>,
    /// The worker has a job in hand.
    busy: bool,
    set: Settings,
    emit: Option<Emit>,
    /// Short fixed replies kept as PCM per voice (§7.3 item 4), oldest out first.
    cache: VecDeque<((String, String, u32), Vec<AudioChunk>)>,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();
/// The worker of a stopped mouth sees a different epoch and leaves.
static EPOCH: AtomicU64 = AtomicU64::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);
static TICKER: AtomicBool = AtomicBool::new(false);

fn with<T>(f: impl FnOnce(&mut Shared) -> T) -> T {
    f(SHARED.lock().unwrap().get_or_insert_with(Shared::default))
}

pub fn start(cx: &Ctx) -> Result<()> {
    begin(crate::emitter(cx));
    Ok(())
}

/// What `start` does, with the way out for events given (an example cannot make a `Ctx`).
pub fn begin(emit: Emit) {
    with(|s| s.emit = Some(emit));
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    player::on_play(Some(Arc::new(on_play)));
    let epoch = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::Builder::new().name("voice-mouth".into()).spawn(move || worker(epoch)).ok();
}

/// Silences at once and drops the queue. `crate::pause` calls it too.
pub fn stop(_cx: &Ctx) {
    end();
}

pub fn end() {
    halt();
    with(|s| s.jobs.clear());
    RUNNING.store(false, Ordering::SeqCst);
    EPOCH.fetch_add(1, Ordering::SeqCst);
    player::on_play(None);
    WAKE.notify_all();
}

pub fn settings(_cx: &Ctx, settings: &Value) {
    apply(settings);
}

/// A turn began: the natural voice loads while the user talks, if it is the one that will answer
/// (§3.3: "start of the first turn"). Returns at once.
pub fn warm() {
    let voice = with(|s| s.set.voice.clone());
    let natural = VoiceDesc::split(&voice).map_or(true, |(engine, _)| engine == Engine::Kokoro);
    if natural && SPEAK.load(Ordering::Relaxed) {
        std::thread::spawn(|| speech::ready() && speech::load().is_ok());
    }
}

/// The setting `speak`: answers are spoken, not only shown.
static SPEAK: AtomicBool = AtomicBool::new(true);

pub fn apply(v: &Value) {
    SPEAK.store(v["speak"] != false, Ordering::Relaxed);
    with(|s| {
        let text = |k: &str, d: &str| v[k].as_str().unwrap_or(d).to_string();
        let num = |k: &str, d: f32| v[k].as_f64().map(|x| x as f32).unwrap_or(d);
        s.set = Settings {
            voice: text("voice", ""),
            kokoro_voice: text("kokoroVoice", "af_heart"),
            eleven_voice: text("elevenVoice", ""),
            rate: num("rate", 1.0),
            win_rate: num("winRate", 1.1),
            win_pitch: num("winPitch", 1.0),
        };
    });
}

/// `say {gen, seq, text, voice?}` → `{}` · `stop` → `{gen}` (the new generation) · `voices` → `[VoiceDesc]`
/// (Kokoro, Windows, ElevenLabs: what is ready) · `speaking` → `{speaking: bool}`.
pub fn call(cx: &Ctx, method: &str, args: Value) -> Result<Value> {
    match method {
        "say" => {
            let s: Say = serde_json::from_value(args).map_err(|e| VoiceError::Failed(format!("say: {e}")))?;
            say(cx, s)?;
            Ok(json!({}))
        }
        "stop" => Ok(json!({ "gen": halt_all() })),
        "voices" => Ok(json!(voices())),
        "speaking" => Ok(json!({ "speaking": speaking() })),
        _ => Err(VoiceError::Failed("unknown method".into())),
    }
}

/// What `mouth.say` does. Returns once the chunk is queued, not once it is said.
pub fn say(_cx: &Ctx, say: Say) -> Result<()> {
    enqueue(say)
}

pub fn enqueue(say: Say) -> Result<()> {
    if !RUNNING.load(Ordering::SeqCst) {
        return Err(VoiceError::Refused("voice is off".into()));
    }
    if say.gen < crate::generation() || say.text.trim().is_empty() {
        return Ok(());
    }
    with(|s| s.jobs.push_back(say));
    WAKE.notify_one();
    Ok(())
}

/// An interruption (§7.4): `crate::interrupt()`, the player fades in `timing::PLAYER_FADE` and empties,
/// late synthesis is dropped. Returns the new generation. Safe from any thread, with nothing playing too.
pub fn halt() -> u64 {
    let gen = crate::interrupt();
    player::stop();
    gen
}

fn halt_all() -> u64 {
    let gen = halt();
    with(|s| s.jobs.clear());
    gen
}

/// Every voice that can speak now, the three engines together.
pub fn voices() -> Vec<VoiceDesc> {
    let mut all = speech::voices();
    all.extend(sapi::voices());
    all.extend(eleven::voices());
    all
}

/// A chunk is playing or queued.
pub fn speaking() -> bool {
    player::playing() || player::queued() > 0 || with(|s| s.busy || !s.jobs.is_empty())
}

// ── The player's events ───────────────────────────────────────────────────────

fn emit(event: &str, payload: Value) {
    if let Some(e) = with(|s| s.emit.clone()) {
        e(event, payload);
    }
}

fn on_play(ev: Option<(u64, u32)>) {
    match ev {
        Some((gen, seq)) => {
            if let Some(e) = with(|s| s.emit.clone()) {
                crate::set_state(&e, crate::State::Speaking);
            }
            crate::ears::while_speaking();
            emit(crate::event::SPOKE, json!(crate::Spoke { gen, seq }));
            if !TICKER.swap(true, Ordering::SeqCst) {
                std::thread::Builder::new().name("voice-level".into()).spawn(ticker).ok();
            }
        }
        None => emit(crate::event::LEVEL, json!({ "out": 0.0 })),
    }
}

/// `voice:level {out}` `LEVEL_HZ` times a second while the player runs, and only then.
fn ticker() {
    while player::running() {
        emit(crate::event::LEVEL, json!({ "out": player::level() }));
        std::thread::sleep(Duration::from_millis(1000 / timing::LEVEL_HZ as u64));
    }
    TICKER.store(false, Ordering::SeqCst);
}

// ── The synthesis worker ──────────────────────────────────────────────────────

fn worker(epoch: u64) {
    loop {
        let job = {
            let mut g = SHARED.lock().unwrap();
            loop {
                if EPOCH.load(Ordering::SeqCst) != epoch {
                    return;
                }
                let s = g.get_or_insert_with(Shared::default);
                if let Some(j) = s.jobs.pop_front() {
                    s.busy = true;
                    break j;
                }
                g = WAKE.wait(g).unwrap();
            }
        };
        process(&job, epoch);
        with(|s| s.busy = false);
    }
}

fn current(gen: u64, epoch: u64) -> bool {
    gen >= crate::generation() && EPOCH.load(Ordering::SeqCst) == epoch
}

fn process(job: &Say, epoch: u64) {
    // At most SYNTH_AHEAD chunks ahead of the player.
    while current(job.gen, epoch) && player::queued() >= timing::SYNTH_AHEAD {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !current(job.gen, epoch) {
        return;
    }
    let started = std::time::Instant::now();
    let mut tried: Vec<(Engine, String)> = Vec::new();
    for (engine, req) in plan(job) {
        if tried.contains(&(engine, req.voice.clone())) {
            continue;
        }
        tried.push((engine, req.voice.clone()));
        match speak(job, engine, &req, epoch) {
            Ok(()) => {
                notch_core::log::line(format!("voice: chunk {} said by {:?} after {} ms", job.seq, engine, started.elapsed().as_millis()));
                return;
            }
            Err(VoiceError::Cancelled) => return,
            Err(e) => notch_core::log::line(format!("voice: {engine:?} failed on a chunk: {e}")),
        }
    }
    // No voice could make it: the player skips this seq instead of waiting for it.
    let _ = player::play(job.gen, job.seq, AudioChunk { rate: 1, samples: Vec::new() }, true);
}

/// The voices to try for a chunk, in order: the chosen one, then the Windows voice.
fn plan(job: &Say) -> Vec<(Engine, SynthRequest)> {
    let set = with(|s| s.set.clone());
    let id = job.voice.clone().unwrap_or(set.voice.clone());
    let want = VoiceDesc::split(&id).map(|(e, own)| (e, own.to_string())).or_else(|| {
        if speech::ready() {
            Some((Engine::Kokoro, String::new()))
        } else {
            None
        }
    });
    let req = |engine: Engine, own: String| match engine {
        Engine::Kokoro => SynthRequest {
            text: job.text.clone(),
            voice: if own.is_empty() { set.kokoro_voice.clone() } else { own },
            rate: set.rate,
            pitch: 1.0,
        },
        Engine::Eleven => SynthRequest {
            text: job.text.clone(),
            voice: if own.is_empty() { set.eleven_voice.clone() } else { own },
            rate: set.rate,
            pitch: 1.0,
        },
        Engine::Windows => SynthRequest { text: job.text.clone(), voice: own, rate: set.win_rate, pitch: set.win_pitch },
    };
    let mut plan = Vec::new();
    if let Some((engine, own)) = want {
        plan.push((engine, req(engine, own)));
    }
    plan.push((Engine::Windows, req(Engine::Windows, String::new())));
    plan
}

fn speak(job: &Say, engine: Engine, req: &SynthRequest, epoch: u64) -> Result<()> {
    let key = (format!("{engine:?}:{}", req.voice), job.text.clone(), (req.rate * 100.0) as u32 + (req.pitch * 100.0) as u32 * 10_000);
    // A short fixed reply ("Done.") is kept as PCM: instant the second time.
    if job.text.len() <= 48 {
        let hit = with(|s| s.cache.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()));
        if let Some(pieces) = hit {
            let n = pieces.len();
            for (i, p) in pieces.into_iter().enumerate() {
                if !current(job.gen, epoch) {
                    return Err(VoiceError::Cancelled);
                }
                player::play(job.gen, job.seq, p, i + 1 == n)?;
            }
            return Ok(());
        }
    }
    // One piece is held back so that the last can be sent with `last`.
    let mut held: Option<AudioChunk> = None;
    let mut all: Vec<AudioChunk> = Vec::new();
    let mut sink = |piece: AudioChunk| -> bool {
        if !current(job.gen, epoch) {
            return false;
        }
        let keep = (job.text.len() <= 48).then(|| piece.clone());
        if let Some(prev) = held.replace(piece) {
            if player::play(job.gen, job.seq, prev, false).is_err() {
                return false;
            }
        }
        all.extend(keep);
        true
    };
    let done = match engine {
        Engine::Kokoro => speech::synth(req, &mut sink),
        Engine::Windows => sapi::synth(req, &mut sink),
        Engine::Eleven => eleven::synth(req, &mut sink),
    };
    done?;
    let last = held.ok_or_else(|| VoiceError::Failed("no audio".into()))?;
    if !current(job.gen, epoch) {
        return Err(VoiceError::Cancelled);
    }
    player::play(job.gen, job.seq, last, true)?;
    if job.text.len() <= 48 {
        with(|s| {
            s.cache.push_back((key, all));
            while s.cache.len() > timing::PHRASE_CACHE {
                s.cache.pop_front();
            }
        });
    }
    Ok(())
}
