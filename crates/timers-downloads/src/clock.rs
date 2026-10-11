// Timer, Pomodoro and stopwatch state. Anchored on an end date (wall clock, so a sleep cannot
// stretch it), one parked thread per run (a Condvar wait to the end date, no ticking). The page asks for a snapshot and counts locally.

use serde_json::{json, Value};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

#[derive(Clone, Copy, PartialEq)]
enum St {
    Idle,
    Running,
    Paused,
    Waiting, // pomodoro: the next phase is ready and needs an explicit Start
    Finished,
}

impl St {
    fn name(self) -> &'static str {
        match self {
            St::Idle => "idle",
            St::Running => "running",
            St::Paused => "paused",
            St::Waiting => "waiting",
            St::Finished => "finished",
        }
    }
}

struct S {
    mode: String,
    st: St,
    phase: &'static str,
    total: u64,
    remaining: u64,
    deadline: Option<SystemTime>,
    base: u64,
    since: Option<Instant>,
    laps: Vec<u64>,
    focus: u64,
    short: u64,
    long: u64,
    every: u64,
    sessions: u64,
    auto: bool,
    done_focus: u64,
    gen: u64,
}

impl S {
    fn elapsed(&self) -> u64 {
        self.base + self.since.map_or(0, |t| t.elapsed().as_millis() as u64)
    }
    fn left(&self) -> u64 {
        match self.deadline {
            Some(d) => d.duration_since(SystemTime::now()).map_or(0, |d| d.as_millis() as u64),
            None => self.remaining,
        }
    }
    fn snap(&self) -> Value {
        let sw = self.mode == "stopwatch";
        json!({
            "mode": self.mode, "status": self.st.name(), "phase": self.phase,
            "totalMs": self.total,
            "remainingMs": if sw { 0 } else { self.left() },
            "elapsedMs": if sw { self.elapsed() } else { self.total.saturating_sub(self.left()) },
            "laps": self.laps,
            "session": self.done_focus + (self.phase == "focus" && self.st != St::Finished) as u64,
            "sessions": self.sessions,
        })
    }
    fn arm(&mut self) {
        self.deadline = Some(SystemTime::now() + Duration::from_millis(self.remaining));
        self.st = St::Running;
    }
    /// The deadline passed: returns the "done" event payload.
    fn finish(&mut self) -> Value {
        self.deadline = None;
        self.remaining = 0;
        let (mode, phase) = (self.mode.clone(), self.phase);
        if self.mode != "pomodoro" {
            self.st = St::Finished;
            return json!({ "mode": mode, "phase": phase, "final": true });
        }
        if phase == "focus" {
            self.done_focus += 1;
            if self.done_focus >= self.sessions {
                self.st = St::Finished;
                return json!({ "mode": mode, "phase": phase, "final": true });
            }
            let long = self.done_focus % self.every == 0;
            self.phase = if long { "long" } else { "short" };
            self.total = if long { self.long } else { self.short };
        } else {
            self.phase = "focus";
            self.total = self.focus;
        }
        self.remaining = self.total;
        if self.auto {
            self.arm();
        } else {
            self.st = St::Waiting;
        }
        json!({ "mode": mode, "phase": phase, "next": self.phase, "final": false })
    }
}

#[derive(Clone)]
pub struct Clock {
    s: Arc<(Mutex<S>, Condvar)>,
    emit: Emit,
}

fn ms(v: &Value, key: &str, default: f64) -> u64 {
    (v.get(key).and_then(Value::as_f64).unwrap_or(default).clamp(0.0, 180.0) * 60_000.0) as u64
}

impl Clock {
    pub fn new(emit: Emit) -> Self {
        let s = S {
            mode: "timer".into(), st: St::Idle, phase: "focus", total: 0, remaining: 0, deadline: None,
            base: 0, since: None, laps: vec![], focus: 25 * 60_000, short: 5 * 60_000, long: 15 * 60_000,
            every: 4, sessions: 4, auto: false, done_focus: 0, gen: 0,
        };
        Clock { s: Arc::new((Mutex::new(s), Condvar::new())), emit }
    }

    pub fn snapshot(&self) -> Value {
        self.s.0.lock().unwrap().snap()
    }

    /// Runs `f` under the lock; a `true` result means a deadline was armed and needs its sleeper.
    fn apply(&self, f: impl FnOnce(&mut S) -> bool) -> Value {
        let (snap, gen) = {
            let mut g = self.s.0.lock().unwrap();
            g.gen += 1; // retires any sleeper
            f(&mut g);
            self.s.1.notify_all();
            // a deadline still armed (the command left it running) gets a fresh sleeper
            (g.snap(), g.deadline.map(|_| g.gen))
        };
        if let Some(gen) = gen {
            self.sleeper(gen);
        }
        (self.emit)("state", snap.clone());
        snap
    }

    fn sleeper(&self, gen: u64) {
        let me = self.clone();
        std::thread::spawn(move || {
            let (m, cv) = &*me.s;
            let mut g = m.lock().unwrap();
            loop {
                if g.gen != gen {
                    return;
                }
                let Some(d) = g.deadline else { return };
                match d.duration_since(SystemTime::now()) {
                    Ok(w) if !w.is_zero() => g = cv.wait_timeout(g, w).unwrap().0,
                    _ => {
                        let done = g.finish();
                        let snap = g.snap();
                        drop(g);
                        (me.emit)("done", done);
                        (me.emit)("state", snap);
                        // auto-start armed the next phase: keep sleeping for it
                        g = m.lock().unwrap();
                    }
                }
            }
        });
    }

    /// args: mode ("timer" | "pomodoro" | "stopwatch"), minutes, focus, short, long (minutes), every, sessions, auto.
    /// While a pomodoro waits for its next phase, start begins that phase.
    pub fn start(&self, a: &Value) -> Value {
        self.apply(|s| {
            if s.st == St::Waiting {
                s.arm();
                return true;
            }
            s.mode = a.get("mode").and_then(Value::as_str).unwrap_or("timer").to_string();
            s.laps.clear();
            s.base = 0;
            s.since = None;
            s.done_focus = 0;
            s.phase = "focus";
            match s.mode.as_str() {
                "stopwatch" => {
                    s.since = Some(Instant::now());
                    s.st = St::Running;
                    false
                }
                m => {
                    if m == "pomodoro" {
                        s.focus = ms(a, "focus", 25.0).max(1);
                        s.short = ms(a, "short", 5.0).max(1);
                        s.long = ms(a, "long", 15.0).max(1);
                        s.every = a.get("every").and_then(Value::as_u64).unwrap_or(4).clamp(1, 24);
                        s.sessions = a.get("sessions").and_then(Value::as_u64).unwrap_or(4).clamp(1, 24);
                        s.auto = a.get("auto").and_then(Value::as_bool).unwrap_or(false);
                        s.total = s.focus;
                    } else {
                        s.mode = "timer".into();
                        s.total = ms(a, "minutes", 25.0).max(1);
                    }
                    s.remaining = s.total;
                    s.arm();
                    true
                }
            }
        })
    }

    pub fn pause(&self) -> Value {
        self.apply(|s| {
            if s.st == St::Running {
                if s.mode == "stopwatch" {
                    s.base = s.elapsed();
                    s.since = None;
                } else {
                    s.remaining = s.left();
                    s.deadline = None;
                }
                s.st = St::Paused;
            }
            false
        })
    }

    pub fn resume(&self) -> Value {
        self.apply(|s| {
            if s.st != St::Paused {
                return false;
            }
            if s.mode == "stopwatch" {
                s.since = Some(Instant::now());
                s.st = St::Running;
                false
            } else {
                s.arm();
                true
            }
        })
    }

    /// Pomodoro: leave this phase now. A skipped focus does not count toward the long break.
    pub fn skip(&self) -> Value {
        let mut ev = None;
        let snap = self.apply(|s| {
            if s.mode != "pomodoro" || !matches!(s.st, St::Running | St::Paused | St::Waiting) {
                return false;
            }
            let from = s.phase;
            (s.phase, s.total) = if from == "focus" { ("short", s.short) } else { ("focus", s.focus) };
            s.remaining = s.total;
            s.deadline = None;
            ev = Some(json!({ "mode": "pomodoro", "phase": from, "next": s.phase, "final": false, "skipped": true }));
            if s.auto {
                s.arm();
                return true;
            }
            s.st = St::Waiting;
            false
        });
        if let Some(e) = ev {
            (self.emit)("done", e);
        }
        snap
    }

    /// The end date moved earlier by `ms`, as when the machine slept through part of a run.
    pub fn shift(&self, ms: u64) {
        let mut g = self.s.0.lock().unwrap();
        if let Some(d) = g.deadline.as_mut() {
            *d -= Duration::from_millis(ms);
        }
        self.s.1.notify_all();
    }

    pub fn lap(&self) -> Value {
        self.apply(|s| {
            if s.mode == "stopwatch" && s.st == St::Running {
                let e = s.elapsed();
                s.laps.push(e);
            }
            false
        })
    }

    /// Cancel, or dismiss a finished one.
    pub fn cancel(&self) -> Value {
        self.apply(|s| {
            s.st = St::Idle;
            s.deadline = None;
            s.since = None;
            s.base = 0;
            s.remaining = 0;
            s.total = 0;
            s.laps.clear();
            false
        })
    }

    /// Feature stop: retire the sleeper without emitting.
    pub fn stop(&self) {
        let mut g = self.s.0.lock().unwrap();
        g.gen += 1;
        g.st = St::Idle;
        g.deadline = None;
        g.since = None;
        self.s.1.notify_all();
    }
}
