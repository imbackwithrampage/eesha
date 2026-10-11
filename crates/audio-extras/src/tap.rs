// The tap: the one thread that hears what the computer plays (svc-audio's
// loopback), for the bars and for the beat. It exists only while somebody
// looks: `watch` says whether the playing bars and the mascot are on screen,
// the thread starts with the first of them and ends a moment after the last.
// While only the mascot looks and the beat's follower rests between two looks,
// the loopback is closed and the thread sleeps. The beat is also followed while
// the loopback is open for the bars, so that it is known when the mascot comes;
// it never opens the loopback by being wanted alone. Nothing of the sound is
// kept: each tenth of a second becomes seven bar heights and a few onset
// values, and is dropped.
//
// The beat is taktus (Apache-2.0, by the owner of this project): `Ear` turns
// sound into onsets, `Follower` reads tempo and beat from them and says when it
// wants to listen. What the mascot is told follows the owner's upstream pull
// request 412: a tempo folded into 80 to 160 and the time of one beat.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use taktus::{Ear, Follower};

use crate::bands::{Analyzer, BANDS};

/// What the tap says, on its own thread.
pub enum Told {
    /// The seven bar heights (0 to 1), thirty times a second while the bars
    /// are looked at. None: back to their own motion.
    Levels(Option<[f32; BANDS]>),
    /// The beat was found, moved or said more exactly: the tempo to dance at
    /// and the Unix time in milliseconds of its last beat. None: lost.
    Beat(Option<(f64, u64)>),
    /// The tap gave up until the next play, and why.
    Quiet(String),
}

/// A pause or a skip does not close the loopback at once.
const RELEASE: Duration = Duration::from_millis(1500);
/// A tap that hears nothing for this long is given up on until the next play:
/// a player in exclusive mode, sound on another output.
const SILENCE: Duration = Duration::from_secs(8);
/// How long one read waits for sound.
const WAIT: Duration = Duration::from_millis(100);

/// The tempo the mascot moves at: between 80 and 160, a beat read outside
/// that at half or double.
pub fn dance_tempo(bpm: f64) -> f64 {
    if !bpm.is_finite() || bpm <= 0.0 {
        return 0.0;
    }
    let mut bpm = bpm;
    while bpm < 80.0 {
        bpm *= 2.0;
    }
    while bpm > 160.0 {
        bpm /= 2.0;
    }
    bpm
}

/// A beat `period` seconds long with one beat at `at`, both on a clock that
/// reads `now`: the tempo to dance at and the wall-clock time (Unix ms, which
/// reads `wall` now) of the last beat.
pub fn beat_on_wall(period: f64, at: f64, now: f64, wall: u64) -> (f64, u64) {
    // ponytail: a tempo danced at half keeps whichever beat the engine named, so on a song it counts in halves the mascot may land between two drums; `taktus::beats_at` over the last seconds of onsets, at the doubled period, would say which.
    let ago = (now - at).rem_euclid(period);
    (dance_tempo(60.0 / period), wall.saturating_sub((ago * 1000.0).round() as u64))
}

/// What is made of the sound as it comes: one sample per frame at about
/// 48 kHz, then bar heights and onsets. The tap feeds it the loopback, the
/// probe a click track.
pub struct Hearing {
    /// Samples of all channels that become one: the channels, times the frames
    /// thinned into one on an output that runs faster than 48 kHz.
    group: usize,
    rate: u32,
    mono: Vec<f32>,
    onsets: Vec<f32>,
    analyzer: Analyzer,
    ear: Option<Ear>,
}

impl Hearing {
    pub fn new(rate: u32, channels: u16) -> Self {
        // ponytail: whole frames are averaged, so 96 and 192 kHz come down to 48 and 44.1 stays; a real resampler if an odd rate ever matters.
        let thin = ((rate + 24_000) / 48_000).max(1);
        let rate = rate / thin;
        Hearing {
            group: usize::from(channels.max(1)) * thin as usize,
            rate,
            mono: Vec::new(),
            onsets: Vec::new(),
            analyzer: Analyzer::new(rate),
            ear: None,
        }
    }

    /// Interleaved frames in; `seconds` of silence when there are none (Windows
    /// delivers nothing while nothing plays). True when something was heard.
    pub fn take(&mut self, frames: &[f32], seconds: f64) -> bool {
        self.mono.clear();
        self.mono.extend(frames.chunks_exact(self.group).map(|g| g.iter().sum::<f32>() / g.len() as f32));
        if self.mono.is_empty() {
            self.mono.resize((f64::from(self.rate) * seconds) as usize, 0.0);
        }
        self.mono.iter().any(|s| s.abs() > 1e-4)
    }

    /// The bar heights of what was just taken.
    pub fn bars(&mut self, out: impl FnMut([f32; BANDS])) {
        self.analyzer.feed(&self.mono, out);
    }

    /// The onsets of what was just taken.
    pub fn onsets(&mut self) -> &[f32] {
        let ear = self.ear.get_or_insert_with(|| Ear::new(self.rate));
        self.onsets.clear();
        self.onsets.extend(self.mono.iter().filter_map(|s| ear.hear(*s)));
        &self.onsets
    }

    /// The beat stopped listening: its next spell starts with a new ear.
    pub fn rest(&mut self) {
        self.ear = None;
    }
}

struct State {
    /// The playing bars are on screen.
    bars: bool,
    /// The bounce is on screen while music plays: the tap opens for it.
    beat: bool,
    /// Music plays and the bounce is wanted: the beat is followed whenever the
    /// tap is open, the bars' doing included, so it is known when the mascot comes.
    follow: bool,
    /// Nothing was heard, or the loopback did not open: no tap until the next play.
    gave_up: bool,
    stop: bool,
    /// The tap's thread, while there is one.
    worker: Option<Thread>,
    follower: Follower,
}

impl State {
    fn wanted(&self) -> bool {
        !self.stop && !self.gave_up && (self.bars || self.beat)
    }
}

struct Shared {
    sink: Box<dyn Fn(Told) + Send + Sync>,
    state: Mutex<State>,
    /// Where the follower's clock starts.
    epoch: Instant,
    open: AtomicBool,
    /// The outputs changed: the loopback is opened again on the default one.
    reopen: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }
}

pub struct Tap {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Tap {
    /// Nothing runs yet. `sink` is called on the tap's thread.
    pub fn new(sink: impl Fn(Told) + Send + Sync + 'static) -> Self {
        let state =
            State { bars: false, beat: false, follow: false, gave_up: false, stop: false, worker: None, follower: Follower::default() };
        Tap {
            shared: Arc::new(Shared {
                sink: Box::new(sink),
                state: Mutex::new(state),
                epoch: Instant::now(),
                open: AtomicBool::new(false),
                reopen: AtomicBool::new(false),
            }),
            thread: Mutex::new(None),
        }
    }

    /// Who looks right now: the playing bars, the mascot. `follow`: music plays
    /// and its beat is wanted. The loopback is open while the bars are looked
    /// at, or the mascot with a beat to follow; never for `follow` alone.
    pub fn watch(&self, bars: bool, mascot: bool, follow: bool) {
        let mut s = self.shared.lock();
        let beat = mascot && follow;
        // Looked at anew: a tap that was given up on gets another try.
        if (bars && !s.bars) || (beat && !s.beat) {
            s.gave_up = false;
        }
        (s.bars, s.beat, s.follow) = (bars, beat, follow);
        self.wake(s);
    }

    /// Another song: the beat known is forgotten, the new one looked for from scratch.
    pub fn new_song(&self) {
        let mut s = self.shared.lock();
        s.follower = Follower::default();
        s.gave_up = false;
        self.wake(s);
    }

    /// The song went on from somewhere else (a pause): the tempo holds, where
    /// its beats fall is found again.
    pub fn moved(&self) {
        let mut s = self.shared.lock();
        s.follower.moved();
        s.gave_up = false;
        self.wake(s);
    }

    /// The loopback is open right now.
    pub fn open(&self) -> bool {
        self.shared.open.load(Relaxed)
    }

    /// Starts the thread if it is wanted and not there, wakes it if it sleeps.
    fn wake(&self, mut s: MutexGuard<'_, State>) {
        if let Some(worker) = &s.worker {
            return worker.unpark();
        }
        if !s.wanted() {
            return;
        }
        let shared = self.shared.clone();
        let Ok(handle) = std::thread::Builder::new().name("audio-extras-tap".into()).spawn(move || run(shared)) else {
            return;
        };
        s.worker = Some(handle.thread().clone());
        drop(s);
        // The one before, if any, has said it is ending.
        *self.thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        {
            let mut s = self.shared.lock();
            s.stop = true;
            if let Some(worker) = &s.worker {
                worker.unpark();
            }
        }
        if let Some(handle) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = handle.join();
        }
    }
}

fn run(shared: Arc<Shared>) {
    let tell = |told: Told| (shared.sink)(told);
    // Headphones plugged in, another default output: the stream of the old one would go silent.
    let _outputs = {
        let shared = shared.clone();
        svc_audio::on_endpoint_change(move |change| {
            if matches!(change, svc_audio::Change::Devices) {
                shared.reopen.store(true, Relaxed);
            }
        })
    };
    let mut stream: Option<(svc_audio::Loopback, Hearing)> = None;
    let mut frames = Vec::new();
    // Since when nobody has wanted the sound.
    let mut idle: Option<Instant> = None;
    let mut heard = Instant::now();
    let (mut live, mut feeding, mut errors) = (false, false, 0);
    loop {
        let (bars, beat, rest) = {
            let mut s = shared.lock();
            if s.stop {
                s.worker = None;
                break;
            }
            let now = shared.now();
            let on = !s.gave_up;
            let beat = on && (s.beat || (s.bars && s.follow)) && s.follower.listens(now);
            // Left in the middle of a spell (nobody looks any more): what it
            // heard so far does not join up with what comes next.
            if feeding && !beat && s.follower.sleeps(now).is_none() {
                s.follower.moved();
            }
            (on && s.bars, beat, s.follower.sleeps(now))
        };
        feeding = beat;
        if bars || beat {
            idle = None;
        } else if idle.get_or_insert_with(Instant::now).elapsed() >= RELEASE {
            stream = None;
            shared.open.store(false, Relaxed);
            if std::mem::take(&mut live) {
                tell(Told::Levels(None));
            }
            let mut s = shared.lock();
            if !s.wanted() {
                s.worker = None;
                break;
            }
            drop(s);
            // Only the bounce looks and its follower rests: until it wants
            // sound again, or `watch` says something changed.
            std::thread::park_timeout(Duration::from_secs_f64(rest.unwrap_or(1.0).clamp(0.05, 60.0)));
            continue;
        }

        let give_up = |why: String, idle: &mut Option<Instant>| {
            shared.lock().gave_up = true;
            *idle = Instant::now().checked_sub(RELEASE);
            tell(Told::Quiet(why));
        };
        if shared.reopen.swap(false, Relaxed) {
            stream = None;
        }
        if stream.is_none() {
            match svc_audio::loopback(None) {
                Ok(open) => {
                    let hearing = Hearing::new(open.rate(), open.channels());
                    stream = Some((open, hearing));
                    heard = Instant::now();
                    shared.open.store(true, Relaxed);
                }
                Err(err) => {
                    give_up(format!("the loopback did not open: {err}"), &mut idle);
                    continue;
                }
            }
        }
        let (open, hearing) = stream.as_mut().expect("opened above");
        frames.clear();
        if let Err(err) = open.read(&mut frames, WAIT) {
            // The output went away: once more on whichever is the default now.
            stream = None;
            shared.open.store(false, Relaxed);
            errors += 1;
            if errors > 3 {
                give_up(format!("the loopback stopped: {err}"), &mut idle);
            }
            continue;
        }
        errors = 0;
        if hearing.take(&frames, WAIT.as_secs_f64()) {
            heard = Instant::now();
        } else if heard.elapsed() >= SILENCE {
            give_up("nothing heard on the default output".into(), &mut idle);
            continue;
        }
        if bars {
            hearing.bars(|levels| tell(Told::Levels(Some(levels))));
            live = true;
        } else if std::mem::take(&mut live) {
            tell(Told::Levels(None));
        }
        if beat {
            let onsets = hearing.onsets();
            let now = shared.now();
            let changed = {
                let mut s = shared.lock();
                s.follower.hear(onsets, now).then(|| s.follower.beat())
            };
            if let Some(beat) = changed {
                let wall = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                tell(Told::Beat(beat.map(|(period, at)| beat_on_wall(period, at, now, wall))));
            }
        } else {
            hearing.rest();
        }
    }
    drop(stream);
    shared.open.store(false, Relaxed);
}
