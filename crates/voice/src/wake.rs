// The wake phrase and its guards. Package voice-wake (VOICE §3.4, §6, §7.4, §8).
//
// It never opens the microphone: `ears` does, when `wanted` says so, and hands every voice segment to
// the sink registered here. Three parts:
// 1. `Guards`: what keeps voice from listening, as pure logic. Tray Pause, lock, sleep and energy saver
//    stop everything (`crate::pause`); display off, a full-screen app, a muted microphone, battery power
//    under "only while plugged in", its own speech (unless interruption by voice is on) and a page that
//    cannot show the indicator close the wake's microphone only.
// 2. `Listener`: what is made of a segment. The energy gate, the engine with the keywords, the stock
//    phrases, `wakephrase::find`, a second look on a tighter cut, the second pass, and the two back-offs. It has no device: the probe
//    feeds it files. Its spacing of runs in long speech is a port of upstream's
//    `Voice/WakeWindowBackoff.swift`.
// 3. The module: the subscriptions (`svc-sysevents`, `svc-audio`) that live from `start` to `stop` and
//    are not touched by a pause, and the methods.
//
// The numbers of §6 that only this module uses are constants of this file; the shared ones are in
// `crate::timing`. No word that was heard is logged.

use crate::stt::{self, SttOptions};
use crate::timing::*;
use crate::{ears, event, wakephrase, AudioChunk, Emit, PauseReason, Result, Segment, State, Transcript, VoiceError};
use notch_core::Ctx;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// `voice:wake {wanted, woke}`: the microphone is to be open for the wake phrase or not, sent before
/// `ears` acts on it (the page lights the indicator first); `woke`: the phrase was just heard.
pub use crate::event::WAKE;

/// Energy gate: voiced frames are at least this share of the span they lie in (clicks and hum fail),
/// and there is `WORTH_TRANSCRIBING` of them.
const GATE_SHARE: f32 = 0.4;
/// A second look gets the voice with only this much around it, as the spike's clips had.
const AROUND: Duration = Duration::from_millis(300);
/// A stock phrase from under this much voice is dropped.
const STOCK_UNDER: Duration = Duration::from_millis(600);
/// Second pass: voiced power under the noise floor times this.
const SECOND_PASS_RATIO: f32 = 10.0;
/// This many wakes without a command within `EMPTY_WAKES_IN` double the gate for `STRICT_FOR`.
const EMPTY_WAKES: usize = 3;
const EMPTY_WAKES_IN: Duration = Duration::from_secs(600);
const STRICT_FOR: Duration = Duration::from_secs(600);
/// Long speech: after this many runs without the phrase the next are spaced, from … doubling to ….
const SPACED_AFTER: u32 = 3;
const SPACED_FROM: Duration = Duration::from_millis(500);
const SPACED_MOST: Duration = Duration::from_secs(5);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ── 1. The guards ─────────────────────────────────────────────────────────────

/// What is known of the PC and the app right now. All false: nothing listens (the page has not said it
/// can show the indicator).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Guards {
    /// The tray's Pause.
    pub tray: bool,
    pub locked: bool,
    pub asleep: bool,
    /// Windows' energy saver.
    pub saver: bool,
    pub display_off: bool,
    /// A full-screen app is in front.
    pub fullscreen: bool,
    /// The default microphone is muted.
    pub muted: bool,
    /// On the charger, or a PC without a battery.
    pub mains: bool,
    /// The island's page runs and can show the indicator (§8: "if the indicator cannot be shown the
    /// microphone is not opened").
    pub page: bool,
}

impl Guards {
    /// Everything of voice stops (§3.3): the reason, the first that holds.
    pub fn stops(&self) -> Option<PauseReason> {
        use PauseReason::*;
        [(self.tray, Tray), (self.locked, Lock), (self.asleep, Sleep), (self.saver, EnergySaver)].into_iter().find(|guard| guard.0).map(|guard| guard.1)
    }

    /// The microphone is to be open for the wake phrase (§8, rule 9 b). `settings`: the feature's;
    /// `wake` is "off", "on" or "plugged" (only on mains power), `bargeIn` lets it listen while it talks.
    pub fn wanted(&self, settings: &Value, state: State) -> bool {
        let mode = settings["wake"].as_str().unwrap_or("off");
        (mode == "on" || mode == "plugged" && self.mains)
            && self.page
            && self.stops().is_none()
            && !(self.display_off || self.fullscreen || self.muted)
            && !matches!(state, State::Off | State::Paused(_))
            && (state != State::Speaking || settings["bargeIn"] == true)
    }
}

// ── 2. The listener ───────────────────────────────────────────────────────────

/// What was made of a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// A guard holds: nothing was listened to.
    Guard,
    /// Long speech: this window was left out.
    Spaced,
    /// Too little voice, or too thin: the engine was not run.
    Gate,
    /// The engine failed or is missing.
    Engine,
    /// Only a stock phrase, from a moment of sound.
    Stock,
    NoPhrase,
    /// The phrase with the keywords, and no OK word without them.
    SecondPass,
    Woke,
}

/// A segment's verdict. The words stay with whoever asked: the app drops them, the probe prints them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heard {
    pub why: Why,
    /// What the engine wrote with the keywords; on the tighter cut, if that was tried; and without the
    /// keywords, if the second pass ran.
    pub text: String,
    pub tight: Option<String>,
    pub again: Option<String>,
    /// After the phrase: the start of the command.
    pub rest: String,
}

impl Heard {
    fn not(why: Why) -> Heard {
        Heard { why, text: String::new(), tight: None, again: None, rest: String::new() }
    }

    pub fn woke(&self) -> bool {
        self.why == Why::Woke
    }
}

/// Where the voice of a segment lies, first voiced frame to last, in samples.
fn voice(segment: &Segment) -> Option<(usize, usize)> {
    let frame = ((CAPTURE_POLL.as_secs_f32() * segment.audio.rate as f32) as usize).max(1);
    let floor = segment.noise.max(NOISE_START) * VOICED_RATIO;
    let voiced: Vec<bool> = segment.audio.samples.chunks(frame).map(|f| f.iter().map(|s| s * s).sum::<f32>() / f.len() as f32 > floor).collect();
    let (first, last) = (voiced.iter().position(|v| *v)?, voiced.iter().rposition(|v| *v)?);
    Some((first * frame, ((last + 1) * frame).min(segment.audio.samples.len())))
}

/// Tray Pause, or voice stopped while a segment was on its way.
fn stopped() -> bool {
    notch_core::paused() || matches!(crate::state(), State::Off | State::Paused(_))
}

pub struct Listener {
    /// The dictation feature's folder: the engine and the speech model.
    dir: PathBuf,
    emit: Emit,
    /// It may be interrupted by voice (the setting `bargeIn`): it listens while it talks.
    pub barge_in: bool,
    /// Something plays (the second pass). `None`: `svc-media` is asked; the probe says it.
    pub playing: Option<bool>,
    /// The wake phrase is listened for. `None`: what `wanted` last answered; the probe says it.
    pub wanted: Option<bool>,
    /// Wakes that no command followed, and until when the gate is doubled for them.
    empties: VecDeque<Instant>,
    strict: Option<Instant>,
    /// Long speech: runs without the phrase in a row, and how long the next one waits.
    streak: u32,
    hold: Duration,
}

impl Listener {
    pub fn new(dir: PathBuf, emit: Emit) -> Listener {
        Listener { dir, emit, barge_in: false, playing: None, wanted: None, empties: VecDeque::new(), strict: None, streak: 0, hold: Duration::ZERO }
    }

    /// A wake that no command followed.
    pub fn empty_wake(&mut self, now: Instant) {
        self.empties.push_back(now);
        self.empties.retain(|at| now.duration_since(*at) < EMPTY_WAKES_IN);
        if self.empties.len() >= EMPTY_WAKES {
            self.strict = Some(now + STRICT_FOR);
            self.empties.clear();
        }
    }

    pub fn empty_wakes(&self) -> usize {
        self.empties.len()
    }

    /// The gate is doubled.
    pub fn strict(&self, now: Instant) -> bool {
        self.strict.is_some_and(|until| now < until)
    }

    /// A run without the phrase. Within one long segment the third and later ones space the next run.
    fn missed(&mut self, window: bool) {
        if !window {
            (self.streak, self.hold) = (0, Duration::ZERO);
            return;
        }
        self.streak += 1;
        if self.streak >= SPACED_AFTER {
            self.hold = (SPACED_FROM * 2u32.pow((self.streak - SPACED_AFTER).min(8))).min(SPACED_MOST);
        }
    }

    /// One segment from `ears::on_segment`, or one sliding window of long speech: was it the wake phrase.
    /// Blocks for the engine's run, and for one more with the second look and with the second pass.
    pub fn hear(&mut self, segment: &Segment, now: Instant) -> Heard {
        let speaking = crate::state() == State::Speaking;
        let wanted = self.wanted.unwrap_or_else(|| WANTED.load(SeqCst));
        // Outside its own speech the phrase counts only while the wake is wanted (a segment cut as the speech ended).
        if stopped() || !speaking && !wanted {
            return Heard::not(Why::Guard);
        }
        // Windows come `WAKE_WINDOW_EVERY` apart: the wait is counted in them, not on a clock.
        if segment.window && self.hold > WAKE_WINDOW_EVERY {
            self.hold -= WAKE_WINDOW_EVERY;
            return Heard::not(Why::Spaced);
        }
        let Some((from, to)) = voice(segment) else { return Heard::not(Why::Gate) };
        let rate = segment.audio.rate as f32;
        // Doubled after wakes that nothing followed; never against a "stop" said while it talks.
        let need = WORTH_TRANSCRIBING * if self.strict(now) && !speaking { 2 } else { 1 };
        if segment.voiced < need || segment.voiced.as_secs_f32() < GATE_SHARE * (to - from) as f32 / rate {
            return Heard::not(Why::Gate);
        }
        let mut options = SttOptions { language: Some("en".into()), keywords: wakephrase::keywords() };
        let Ok(text) = stt::transcribe(&self.dir, &segment.audio, &options) else { return Heard::not(Why::Engine) };
        if stopped() {
            return Heard::not(Why::Guard);
        }
        if segment.voiced < STOCK_UNDER && wakephrase::stock(&text) {
            return Heard { text, ..Heard::not(Why::Stock) };
        }
        // Its own speech, and it may not be interrupted by voice: one word is listened for, "stop" (the
        // owner's decision), which the page knows. Anything longer is dropped here, unread.
        // ponytail: no echo cancelling: over the speakers its own voice is in the segment too, and a "stop"
        // said over it may not come out as three words or fewer. Headphones hear it; the owner's session measures the rest.
        if speaking && !(self.barge_in && wanted) {
            if !text.is_empty() && text.split_whitespace().count() <= 3 {
                (self.emit)(event::HEARD, json!(Transcript { text: text.clone(), is_final: false }));
            }
            return Heard { text, ..Heard::not(Why::NoPhrase) };
        }
        // The OK word and not the name: once more, on the voice with little around it. The engine writes
        // the same sound differently with other silence beside it (the spike's clips: each cut hears
        // phrases the other misses).
        let (mut wake, mut audio, mut tight) = (wakephrase::find(&text), &segment.audio, None);
        let cut;
        if wake.is_none() && wakephrase::ok_word(&text) {
            let around = (AROUND.as_secs_f32() * rate) as usize;
            cut = AudioChunk { rate: segment.audio.rate, samples: segment.audio.samples[from.saturating_sub(around)..(to + around).min(segment.audio.samples.len())].to_vec() };
            let closer = stt::transcribe(&self.dir, &cut, &options).unwrap_or_default();
            (wake, audio) = (wakephrase::find(&closer), &cut);
            tight = Some(closer);
        }
        let Some(wake) = wake else {
            // While it talks, what else was said goes to the page, which knows "stop" when it hears it.
            if speaking && !text.is_empty() {
                (self.emit)(event::HEARD, json!(Transcript { text: text.clone(), is_final: false }));
            }
            self.missed(segment.window);
            return Heard { text, tight, ..Heard::not(Why::NoPhrase) };
        };
        // Faint against the room, or something plays: the keywords may have written the phrase themselves.
        let mut again = None;
        if segment.power < segment.noise * SECOND_PASS_RATIO || self.playing.unwrap_or_else(|| svc_media::state().is_some_and(|media| media.playing)) {
            options.keywords.clear();
            let plain = stt::transcribe(&self.dir, audio, &options).unwrap_or_default();
            if !wakephrase::ok_word(&plain) {
                self.missed(segment.window);
                return Heard { text, tight, again: Some(plain), ..Heard::not(Why::SecondPass) };
            }
            again = Some(plain);
        }
        (self.streak, self.hold) = (0, Duration::ZERO);
        notch_core::log::line(format!("voice: wake voiced={} second={} window={}", segment.voiced.as_millis(), again.is_some(), segment.window));
        (self.emit)(WAKE, json!({ "wanted": true, "woke": true }));
        Heard { why: Why::Woke, text, tight, again, rest: wake.rest }
    }
}

// ── 3. The module ─────────────────────────────────────────────────────────────

struct Running {
    cx: Ctx,
    listener: Arc<Mutex<Listener>>,
    /// Session, power, the window in front, the microphone's mute: from `start` to `stop`.
    _subs: (svc_sysevents::Sub, svc_sysevents::Sub, svc_sysevents::Sub, svc_audio::Sub),
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// What the services last said; the tray's Pause is read when asked (`guards`).
static GUARDS: Mutex<Guards> = Mutex::new(Guards { tray: false, locked: false, asleep: false, saver: false, display_off: false, fullscreen: false, muted: false, mains: false, page: false });
/// What `wanted` last answered.
static WANTED: AtomicBool = AtomicBool::new(false);
/// One `apply` at a time.
static APPLY: Mutex<()> = Mutex::new(());

fn guards() -> Guards {
    Guards { tray: notch_core::paused(), ..*lock(&GUARDS) }
}

fn muted() -> bool {
    svc_audio::endpoint_volume(svc_audio::Flow::Capture).is_ok_and(|volume| volume.mute)
}

/// A service said something: acted on only if it changes a guard, and off the service's own thread.
fn change(set: impl FnOnce(&mut Guards)) {
    let changed = {
        let mut guards = lock(&GUARDS);
        let before = *guards;
        set(&mut guards);
        *guards != before
    };
    if changed {
        std::thread::spawn(apply);
    }
}

/// What the guards ask for now: everything stops, everything is back, or `ears` asks `wanted` again.
fn apply() {
    let _one = lock(&APPLY);
    let Some(cx) = lock(&RUNNING).as_ref().map(|running| running.cx.clone()) else { return };
    match (guards().stops(), crate::state()) {
        (Some(reason), now) if now != State::Paused(reason) => crate::pause(&cx, reason),
        (Some(_), _) => {}
        (None, State::Paused(_)) => crate::resume(&cx),
        (None, _) => {
            let _ = ears::start(&cx);
        }
    }
}

pub fn start(cx: &Ctx) -> Result<()> {
    {
        let mut running = lock(&RUNNING);
        if running.is_some() {
            return Ok(());
        }
        let mut listener = Listener::new(cx.data_dir("dictation"), crate::emitter(cx));
        listener.barge_in = cx.settings(crate::ID)["bargeIn"] == true;
        let listener = Arc::new(Mutex::new(listener));
        let power = svc_sysevents::power_status();
        // The page says when it is there (`sync {page}`, maybe a moment before this); until then nothing listens.
        let page = lock(&GUARDS).page;
        *lock(&GUARDS) = Guards { saver: power.saver, mains: power.ac, fullscreen: svc_sysevents::foreground().fullscreen, muted: muted(), page, ..Guards::default() };
        let sink = listener.clone();
        ears::on_segment(Some(Arc::new(move |segment: &Segment| lock(&sink).hear(segment, Instant::now()).woke())));
        use svc_sysevents::Session;
        let subs = (
            svc_sysevents::on_session(|session| {
                change(|g| match session {
                    Session::Lock | Session::Unlock => g.locked = session == Session::Lock,
                    Session::Sleep | Session::Resume => g.asleep = session == Session::Sleep,
                    Session::DisplayOff | Session::DisplayOn => g.display_off = session == Session::DisplayOff,
                    _ => {}
                })
            }),
            svc_sysevents::on_power(|power| change(|g| (g.saver, g.mains) = (power.saver, power.ac))),
            svc_sysevents::on_foreground(|front| {
                let full = front.fullscreen;
                change(|g| g.fullscreen = full)
            }),
            svc_audio::on_endpoint_change(|what| match what {
                svc_audio::Change::Volume { flow: svc_audio::Flow::Capture, volume, .. } => {
                    let mute = volume.mute;
                    change(|g| g.muted = mute)
                }
                svc_audio::Change::Volume { .. } => {}
                // Another default microphone: its mute is read off the audio thread.
                svc_audio::Change::Devices => drop(std::thread::spawn(|| {
                    let mute = muted();
                    change(|g| g.muted = mute)
                })),
            }),
        );
        *running = Some(Running { cx: cx.clone(), listener, _subs: subs });
    }
    apply();
    Ok(())
}

pub fn stop(_cx: &Ctx) {
    ears::on_segment(None);
    // An `apply` under way ends first: none may start `ears` again behind this.
    let _one = lock(&APPLY);
    // The subscriptions end outside the lock.
    let running = lock(&RUNNING).take();
    drop(running);
    lock(&GUARDS).page = false;
    WANTED.store(false, SeqCst);
}

pub fn settings(_cx: &Ctx, settings: &Value) {
    let listener = lock(&RUNNING).as_ref().map(|running| running.listener.clone());
    if let Some(listener) = listener {
        lock(&listener).barge_in = settings["bargeIn"] == true;
    }
}

/// `status` → `{wanted: bool, phrase, emptyWakes}` · `tray {id}` (the host's tray item `listen`: switches
/// the setting `wake` between "off" and "on") · `sync {page?: bool}` → `{wanted}` (the page: it can show
/// the indicator or no longer can; and whenever the tray's Pause or its own speech may have changed
/// what is wanted) · `empty` (a wake that no command followed).
pub fn call(cx: &Ctx, method: &str, args: Value) -> Result<Value> {
    let listener = lock(&RUNNING).as_ref().map(|running| running.listener.clone());
    match method {
        "status" => Ok(json!({ "wanted": WANTED.load(SeqCst), "phrase": wakephrase::phrase(), "emptyWakes": listener.map_or(0, |l| lock(&l).empty_wakes()) })),
        "tray" => {
            let on = cx.settings(crate::ID)["wake"] != "off";
            cx.patch_settings(crate::ID, json!({ "wake": if on { "off" } else { "on" } }));
            Ok(json!({}))
        }
        "sync" => {
            if let Some(page) = args["page"].as_bool() {
                lock(&GUARDS).page = page;
                // A page that has just come knows nothing: it is told again.
                WANTED.store(false, SeqCst);
            }
            apply();
            Ok(json!({ "wanted": WANTED.load(SeqCst) }))
        }
        "empty" => {
            if let Some(listener) = listener {
                lock(&listener).empty_wake(Instant::now());
            }
            Ok(json!({}))
        }
        _ => Err(VoiceError::Failed("unknown method".into())),
    }
}

/// Should the microphone be open for the wake phrase now: the setting `wake` is "on", or "plugged" and
/// the PC is on mains power; no guard holds; and the speech model is there. `ears::start` asks. The page
/// is told first when the answer changes: the indicator is lit before the microphone opens.
pub fn wanted(cx: &Ctx) -> bool {
    let on = lock(&RUNNING).is_some() && guards().wanted(&cx.settings(crate::ID), crate::state()) && stt::ready(&cx.data_dir("dictation"));
    if WANTED.swap(on, SeqCst) != on {
        cx.emit(crate::ID, WAKE, json!({ "wanted": on, "woke": false }));
    }
    on
}
