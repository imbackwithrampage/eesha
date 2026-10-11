// The volume keys, taken from Windows: finer steps with Shift, and replace
// mode, where every press is applied here so the Windows flyout never shows.
// `Keys` decides and `next_level` counts, both without touching anything, so
// the probe can feed them made-up keys; `hook` is the part that listens
// (svc-input, stage 35: after the remaps, before the snippets) and `apply` the
// part that writes. The gate and the rule of who owns a press are ported from
// reference/vorssaint-utils/Sources/Vorssaint/Services/Audio/
// PreciseVolumeRollerSupport.swift.

use std::sync::Mutex;

use svc_audio::Flow;
use svc_input::{Key, Verdict};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};

pub const MUTE: u16 = VK_VOLUME_MUTE.0;
pub const DOWN: u16 = VK_VOLUME_DOWN.0;
pub const UP: u16 = VK_VOLUME_UP.0;

/// A fine step: a quarter of the sixteenth other systems move by.
pub const FINE: f32 = 1.0 / 64.0;
/// What one press moves on Windows.
pub const COARSE: f32 = 0.02;
/// Where this stage stands among the keyboard subscribers of svc-input.
const ORDER: i32 = 35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
}

/// Tames a wheel that jitters: presses closer than 30 ms are dropped, and a
/// turn the other way within 0.3 s of the last step counts only once it has
/// come more than twice.
#[derive(Default)]
pub struct Gate {
    last: Option<(f64, Dir)>,
    reversal: Option<(Dir, u32)>,
}

impl Gate {
    const SPACING: f64 = 0.03;
    const REVERSAL_WINDOW: f64 = 0.30;
    const REVERSAL_CONFIRMATIONS: u32 = 2;

    /// `at` in seconds, on any clock that only goes forward.
    pub fn accepts(&mut self, dir: Dir, at: f64) -> bool {
        if let Some((last, was)) = self.last {
            if at - last <= Self::SPACING {
                return false;
            }
            if dir != was && at - last < Self::REVERSAL_WINDOW {
                let count = match self.reversal {
                    Some((d, n)) if d == dir => n + 1,
                    _ => 1,
                };
                self.reversal = Some((dir, count));
                if count <= Self::REVERSAL_CONFIRMATIONS {
                    return false;
                }
            }
        }
        self.last = Some((at, dir));
        self.reversal = None;
        true
    }
}

/// The keys held beside a volume key.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    /// Ctrl, Alt or Win: the press is somebody's shortcut and is left alone.
    pub other: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    /// Shift and a volume key move 1/64.
    pub fine: bool,
    /// Every volume key is applied here: Windows never sees it, so its flyout stays away.
    pub replace: bool,
}

impl Mode {
    pub fn any(self) -> bool {
        self.fine || self.replace
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    Step(Dir, f32),
    Mute,
}

/// What becomes of each key event. Who owns a press is decided when the key
/// goes down and holds for its repeats and its release.
pub struct Keys {
    mode: Mode,
    gate: Gate,
    /// Per key (mute, down, up) while it is held: ours, and with a fine step.
    held: [Option<(bool, bool)>; 3],
}

impl Keys {
    pub fn new(mode: Mode) -> Self {
        Keys { mode, gate: Gate::default(), held: [None; 3] }
    }

    pub fn key(&mut self, key: &Key, mods: Mods) -> (Verdict, Option<Act>) {
        let i = match key.vk {
            MUTE => 0,
            DOWN => 1,
            UP => 2,
            _ => return (Verdict::Pass, None),
        };
        // A key another program sends is that program's business.
        if key.injected {
            return (Verdict::Pass, None);
        }
        let swallow = |ours: bool| if ours { Verdict::Swallow } else { Verdict::Pass };
        if !key.down {
            return (swallow(self.held[i].take().is_some_and(|h| h.0)), None);
        }
        let first = self.held[i].is_none();
        let (ours, fine) = *self.held[i].get_or_insert_with(|| {
            let fine = self.mode.fine && mods.shift;
            (!mods.other && (self.mode.replace || (fine && i != 0)), fine)
        });
        let act = match (ours, i) {
            (false, _) => None,
            // Holding the mute key does not flicker.
            (true, 0) => first.then_some(Act::Mute),
            (true, _) => {
                let dir = if i == 2 { Dir::Up } else { Dir::Down };
                self.gate.accepts(dir, key.t as f64 / 1e6).then_some(Act::Step(dir, if fine { FINE } else { COARSE }))
            }
        };
        (swallow(ours), act)
    }
}

/// The level one step from `level`, on the grid of that step: a level between
/// two marks goes to the next mark, so no press moves more than a step and up
/// then down comes back to where it was.
pub fn next_level(level: f32, dir: Dir, step: f32) -> f32 {
    let marks = level / step;
    let next = match dir {
        Dir::Up => (marks + 1e-3).floor() + 1.0,
        Dir::Down => (marks - 1e-3).ceil() - 1.0,
    };
    (next * step).clamp(0.0, 1.0)
}

/// Applies one press to the default output and says what the level and the
/// mute are after it. `asked` is the level of our last step: an output rounds
/// what it is given to steps of its own, and counting on from what it reads
/// back could stand still.
pub fn apply(act: Act, asked: &mut Option<f32>) -> Result<(f32, bool), String> {
    let now = svc_audio::endpoint_volume(Flow::Render)?;
    match act {
        Act::Mute => {
            svc_audio::set_mute(Flow::Render, !now.mute)?;
            Ok((now.scalar, !now.mute))
        }
        Act::Step(dir, step) => {
            let from = asked.filter(|a| (a - now.scalar).abs() < step).unwrap_or(now.scalar);
            let next = next_level(from, dir, step);
            // Level 0 mutes and any other opens: svc-audio's rule for an output.
            svc_audio::set_volume(Flow::Render, next)?;
            *asked = Some(next);
            Ok((next, next < 0.005))
        }
    }
}

fn mods() -> Mods {
    let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetAsyncKeyState(i32::from(vk.0)) } < 0;
    Mods { shift: down(VK_SHIFT), other: down(VK_CONTROL) || down(VK_MENU) || down(VK_LWIN) || down(VK_RWIN) }
}

/// Listens for the volume keys while the handle lives. `on` is called on the
/// hook thread: it must only hand the press on.
pub fn hook(mode: Mode, on: impl Fn(Act) + Send + Sync + 'static) -> svc_input::Sub {
    let keys = Mutex::new(Keys::new(mode));
    svc_input::keyboard(ORDER, false, move |key| {
        // Every other key costs one comparison.
        if !(MUTE..=UP).contains(&key.vk) {
            return Verdict::Pass;
        }
        let (verdict, act) = keys.lock().unwrap_or_else(|e| e.into_inner()).key(key, mods());
        if let Some(act) = act {
            on(act);
        }
        verdict
    })
}
