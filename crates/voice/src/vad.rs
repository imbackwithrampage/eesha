// Voice detection by energy: a port of upstream's `Voice/EnergyVAD.swift`, with the "voiced" test and
// the microphone level of `Voice/VoiceAudio.swift`, and the numbers of `crate::timing` (VOICE §3.4).
// Package voice-ears. Pure: frames in, events out, no clock and no device, so a probe replays a WAV file
// through it.
//
// Not upstream's: every duration and every smoothing goes by the frame's own length (upstream counts
// frames and rescales the counts); the floor never sinks under where it starts and does not follow
// digital silence (a muted input would make the next sample a voice); and `hear`, for a turn.
//
// `hear` has a floor of its own. Upstream's microphone is always open, so a turn finds a floor learned
// long before, and keeps it. Ours is opened by the key, and the user may be talking from the first
// frame: a floor smoothed over that start is the voice itself, and nothing after it is "voiced" (the
// spike's clips, whose first word comes 0.4 s in, lost their first word or the whole turn). So a turn
// measures against the quietest moment of the last `CALIBRATION`: the gaps between words bring it down
// at once, a louder room raises it half a second later, and it needs no time to itself at the start.

use crate::timing::*;
use std::collections::VecDeque;

/// The level for the page: power smoothed over about this many seconds, times the gain, 1 at most
/// (upstream: 0.4 of each frame at 43 frames a second, × 2000).
const LEVEL_SMOOTH: f32 = 0.045;
const LEVEL_GAIN: f32 = 2000.0;

/// What one frame did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum VadEvent {
    #[default]
    None,
    /// A segment began (power over noise × `SEGMENT_START_RATIO`).
    Start,
    /// It ended: quiet for `SEGMENT_END` (`SEGMENT_END_WAITING` while waiting), or `forced` at `FORCED_END`.
    End { forced: bool },
}

pub struct Vad {
    rate: f32,
    waiting: bool,
    noise: f32,
    power: f32,
    smooth: f32,
    voiced: bool,
    /// Samples fed, and when each frame of the last `CALIBRATION` came, with its power.
    clock: u64,
    recent: VecDeque<(u64, f32)>,
    /// Seconds: of calibration done, of quiet in the segment, of the segment.
    calibrated: f32,
    quiet: f32,
    age: f32,
    active: bool,
    /// The segment's power, for the floor after a forced end.
    sum: f64,
    frames: u32,
}

impl Vad {
    /// `waiting`: the shorter segment end, used while the wake phrase is awaited.
    pub fn new(rate: u32, waiting: bool) -> Vad {
        Vad {
            rate: rate.max(1) as f32,
            waiting,
            noise: NOISE_START,
            power: 0.0,
            smooth: 0.0,
            voiced: false,
            clock: 0,
            recent: VecDeque::new(),
            calibrated: 0.0,
            quiet: 0.0,
            age: 0.0,
            active: false,
            sum: 0.0,
            frames: 0,
        }
    }

    pub fn set_waiting(&mut self, waiting: bool) {
        self.waiting = waiting;
    }

    /// The quiet that ends a segment now.
    pub fn segment_end(&self) -> std::time::Duration {
        if self.waiting { SEGMENT_END_WAITING } else { SEGMENT_END }
    }

    /// One capture poll of samples (any length).
    pub fn feed(&mut self, frame: &[f32]) -> VadEvent {
        let dt = self.measure(frame);
        self.voiced = false;
        if dt == 0.0 || self.calibrating(dt) {
            return VadEvent::None;
        }
        self.voiced = self.power > self.noise * VOICED_RATIO;
        if !self.active {
            self.follow(NOISE_ADAPT_IDLE, dt);
            if self.power > self.noise * SEGMENT_START_RATIO {
                (self.active, self.quiet, self.age, self.sum, self.frames) = (true, 0.0, 0.0, self.power as f64, 1);
                return VadEvent::Start;
            }
            return VadEvent::None;
        }
        self.age += dt;
        self.sum += self.power as f64;
        self.frames += 1;
        if self.age >= FORCED_END.as_secs_f32() {
            // Steady music or noise: it becomes the room.
            self.noise = ((self.sum / self.frames as f64) as f32).max(NOISE_START);
            self.active = false;
            return VadEvent::End { forced: true };
        }
        if self.power < self.noise * QUIET_RATIO {
            self.quiet += dt;
            self.follow(NOISE_ADAPT_QUIET, dt);
            if self.quiet >= self.segment_end().as_secs_f32() {
                self.active = false;
                return VadEvent::End { forced: false };
            }
        } else {
            self.quiet = 0.0;
        }
        VadEvent::None
    }

    /// One poll of a turn, where detection is bypassed: the frame is measured and no segment is cut.
    /// "Voiced" is power over `VOICED_RATIO` times the quietest of the last `CALIBRATION` (see the head
    /// of the file); the floor of the wait is left as it was, as upstream leaves it.
    pub fn hear(&mut self, frame: &[f32]) {
        self.measure(frame);
        let floor = self.recent.iter().map(|(_, power)| *power).fold(f32::INFINITY, f32::min).max(NOISE_START);
        self.voiced = self.power > floor * VOICED_RATIO;
    }

    /// The last frame was voiced: power over the floor × `VOICED_RATIO`. Never while calibrating.
    pub fn voiced(&self) -> bool {
        self.voiced
    }

    /// Mean power of the last frame, and the noise floor.
    pub fn power(&self) -> f32 {
        self.power
    }

    pub fn noise(&self) -> f32 {
        self.noise
    }

    /// 0 to 1, for `voice:level`.
    pub fn level(&self) -> f32 {
        (self.smooth * LEVEL_GAIN).min(1.0)
    }

    /// Back to calibration (a rebuilt capture).
    pub fn reset(&mut self) {
        *self = Vad::new(self.rate as u32, self.waiting);
    }

    /// No segment is open any more; the floor is kept (a turn began or ended). Upstream's `reset`.
    pub fn rest(&mut self) {
        (self.active, self.quiet, self.age, self.sum, self.frames) = (false, 0.0, 0.0, 0.0, 0);
    }

    /// The frame's length in seconds, 0 for an empty one.
    fn measure(&mut self, frame: &[f32]) -> f32 {
        if frame.is_empty() {
            return 0.0;
        }
        let dt = frame.len() as f32 / self.rate;
        self.power = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
        self.smooth += (self.power - self.smooth) * (1.0 - (-dt / LEVEL_SMOOTH).exp());
        // Digital silence is no measurement of the room: it is not what a turn's floor is made of.
        self.clock += frame.len() as u64;
        if self.power > 0.0 {
            self.recent.push_back((self.clock, self.power));
        }
        let window = (CALIBRATION.as_secs_f32() * self.rate) as u64;
        while self.recent.front().is_some_and(|(at, _)| at + window <= self.clock) {
            self.recent.pop_front();
        }
        dt
    }

    /// The first `CALIBRATION` of a capture: the floor is learned fast and nothing triggers.
    fn calibrating(&mut self, dt: f32) -> bool {
        if self.calibrated >= CALIBRATION.as_secs_f32() {
            return false;
        }
        self.calibrated += dt;
        self.follow(NOISE_ADAPT_CALIBRATING, dt);
        true
    }

    /// The floor closes `share` per second of its gap to the frame's power. Digital silence (a muted
    /// microphone, a packet the device marked silent) is no measurement of the room.
    fn follow(&mut self, share: f32, dt: f32) {
        if self.power > 0.0 {
            let k = 1.0 - (1.0 - share).powf(dt);
            self.noise = (self.noise + (self.power - self.noise) * k).max(NOISE_START);
        }
    }
}
