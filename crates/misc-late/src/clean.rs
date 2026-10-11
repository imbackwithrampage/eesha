// Cleaning mode: the keyboard is locked so it can be wiped.
//
// It must never trap the user. Four ways out, none of them depending on the
// page: five Esc presses in a row (each within six seconds of the one before;
// any other key starts the count again, auto-repeat counts for nothing), the
// time limit (the same thread that holds the hooks ends them, so a page that
// hung changes nothing), `Running::stop` (the Unlock button, the feature being
// switched off, the app quitting), and the hooks dying with the thread or the
// process. Ctrl+Alt+Del and Win+L are Windows' own and never reach a hook.
//
// Keys: one subscriber ahead of all the others (order -100), so no remap or
// snippet acts on a key while it is locked. It swallows every press; a release
// is swallowed only when its press was (a key held down when the lock began
// would otherwise stay down for Windows). The wheel is swallowed, the pointer
// and its buttons are not.
//
// Ported from CleaningUnlockCounter.swift (unlock rule).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use svc_input::{Key, Mouse, MouseKind, Verdict};

/// Esc presses that unlock.
pub const NEED: u32 = 5;
/// The longest pause between two of them.
pub const GAP_MS: u64 = 6000;
pub const VK_ESCAPE: u16 = 0x1B;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing for the count (a release, a repeat, a key while the count is 0).
    Nothing,
    Progress(u32),
    /// A key that is not Esc, or a pause that was too long, put the count back to 0.
    Reset,
    Unlocked,
}

/// The unlock rule, on key events and a clock in milliseconds.
#[derive(Default)]
pub struct Counter {
    count: u32,
    last: u64,
    held: HashSet<u16>,
}

impl Counter {
    pub fn key(&mut self, vk: u16, down: bool, t_ms: u64) -> Step {
        if !down {
            self.held.remove(&vk);
            return Step::Nothing;
        }
        // Still held from the press before: auto-repeat.
        if !self.held.insert(vk) {
            return Step::Nothing;
        }
        if vk != VK_ESCAPE {
            return if std::mem::take(&mut self.count) > 0 { Step::Reset } else { Step::Nothing };
        }
        if self.count > 0 && t_ms.saturating_sub(self.last) > GAP_MS {
            self.count = 0;
        }
        self.count += 1;
        self.last = t_ms;
        if self.count >= NEED {
            self.count = 0;
            Step::Unlocked
        } else {
            Step::Progress(self.count)
        }
    }
}

/// The time limit a user sets, kept within what is safe: long enough to wipe a
/// keyboard, short enough that a forgotten lock ends.
pub fn clamp_secs(secs: u64) -> u64 {
    secs.clamp(10, 600)
}

/// Whole seconds left of `limit_secs` after `elapsed_ms`.
pub fn remaining(limit_secs: u64, elapsed_ms: u64) -> u64 {
    (limit_secs * 1000).saturating_sub(elapsed_ms).div_ceil(1000)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Unlocked,
    TimedOut,
    Stopped,
}

#[derive(Debug, Clone, Copy)]
pub struct Update {
    pub count: u32,
    pub need: u32,
    pub remaining: u64,
    pub limit: u64,
}

pub struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    /// Ends the lock and waits for the hooks to be gone.
    pub fn stop(mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

/// Locks the keyboard for at most `limit_secs`. `on_update` runs at every
/// change of the count and once a second; `on_end` once, after the hooks are
/// gone. Both run on the watchdog thread, never in a hook.
pub fn start(
    limit_secs: u64,
    on_update: impl Fn(Update) + Send + 'static,
    on_end: impl FnOnce(Ended) + Send + 'static,
) -> Running {
    start_ms(clamp_secs(limit_secs) * 1000, on_update, on_end)
}

/// `start` with the limit in milliseconds and no floor: for the probe, which
/// has to see a limit end without waiting ten seconds with the keyboard locked.
pub fn start_ms(
    limit_ms: u64,
    on_update: impl Fn(Update) + Send + 'static,
    on_end: impl FnOnce(Ended) + Send + 'static,
) -> Running {
    let limit = limit_ms.div_ceil(1000);
    let (tx, rx) = mpsc::channel::<Step>();
    let counter = Mutex::new(Counter::default());
    let swallowed = Mutex::new(HashSet::<u16>::new());
    let tx_keys = tx.clone();
    let keys = svc_input::keyboard(-100, false, move |k: &Key| {
        if k.down {
            swallowed.lock().unwrap_or_else(|e| e.into_inner()).insert(k.vk);
        } else if !swallowed.lock().unwrap_or_else(|e| e.into_inner()).remove(&k.vk) {
            return Verdict::Pass;
        }
        let step = counter.lock().unwrap_or_else(|e| e.into_inner()).key(k.vk, k.down, k.t / 1000);
        if step != Step::Nothing {
            let _ = tx_keys.send(step);
        }
        Verdict::Swallow
    });
    let wheel = svc_input::mouse(-100, false, |m: &Mouse| {
        if matches!(m.kind, MouseKind::Wheel | MouseKind::HWheel) { Verdict::Swallow } else { Verdict::Pass }
    });
    drop(tx);

    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        // The hooks live on this stack: when it ends, however it ends, they are gone.
        let _hooks = (keys, wheel);
        let began = Instant::now();
        let mut count = 0;
        let mut last_tick = Instant::now();
        let update = |count: u32| {
            on_update(Update { count, need: NEED, remaining: remaining(limit, began.elapsed().as_millis() as u64), limit })
        };
        update(0);
        let why = loop {
            if flag.load(Relaxed) {
                break Ended::Stopped;
            }
            if began.elapsed() >= Duration::from_millis(limit_ms) {
                break Ended::TimedOut;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Step::Unlocked) => break Ended::Unlocked,
                Ok(Step::Progress(n)) => {
                    count = n;
                    update(count);
                    last_tick = Instant::now();
                }
                Ok(Step::Reset) => {
                    count = 0;
                    update(count);
                    last_tick = Instant::now();
                }
                Ok(Step::Nothing) | Err(_) => {}
            }
            if last_tick.elapsed() >= Duration::from_secs(1) {
                update(count);
                last_tick = Instant::now();
            }
        };
        drop(_hooks);
        on_end(why);
    });
    Running { stop, thread: Some(thread) }
}
