// The break rule: when a break is due, held off, on screen and over. No clock and no Windows in
// here: time comes in as whole seconds, so the probe runs it over a made-up timeline.
//
// Each kind counts the seconds the person was at the PC. Away (idle, locked, asleep) nothing
// counts, and an absence as long as the break was the break. A break that is due waits while the
// person is busy (a call, something full screen) and is asked about again every `RECHECK` seconds.

use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Eye,
    Move,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Eye => "eye",
            Kind::Move => "move",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "eye" => Some(Kind::Eye),
            "move" => Some(Kind::Move),
            _ => None,
        }
    }
}

/// One kind of break: after `every` seconds at the PC, `secs` seconds of break.
#[derive(Clone, Copy, Default, Debug)]
pub struct Plan {
    pub on: bool,
    pub every: u64,
    pub secs: u64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Config {
    pub eye: Plan,
    pub mov: Plan,
    /// A snoozed break is due again after this many seconds at the PC.
    pub snooze: u64,
    /// Says `Soon` this many seconds before a break is due; 0 = never (only the fade warns).
    pub warn: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ev {
    /// The break is `Config::warn` seconds away.
    Soon(Kind),
    /// Due, but the person is busy: it waits.
    Held(Kind),
    /// On screen for this many seconds.
    Start(Kind, u64),
    /// Over: "done", "skip", "snooze", "away" (locked or asleep meanwhile) or "off".
    End(Kind, &'static str),
}

/// A held break asks again this often, so it starts within this long of the call ending.
pub const RECHECK: u64 = 10;

#[derive(Default)]
pub struct Rule {
    pub cfg: Config,
    /// Seconds at the PC since the last break, by kind.
    active: [u64; 2],
    /// Seconds of the absence that is going on.
    away: u64,
    /// The break on screen and the seconds it has left.
    now: Option<(Kind, u64)>,
    held: bool,
    wait: u64,
}

impl Rule {
    fn plan(&self, kind: Kind) -> Plan {
        match kind {
            Kind::Eye => self.cfg.eye,
            Kind::Move => self.cfg.mov,
        }
    }

    pub fn on_break(&self) -> Option<Kind> {
        self.now.map(|n| n.0)
    }

    /// `dt` seconds went by. `away`: idle, locked or asleep for all of them. `busy` is asked only
    /// when a break is due: is the person on a call, or is something full screen?
    pub fn tick(&mut self, dt: u64, away: bool, busy: impl FnOnce() -> bool) -> Vec<Ev> {
        let mut out = Vec::new();
        if let Some((kind, left)) = self.now {
            // A break runs on the clock, whatever the person does meanwhile.
            if dt >= left {
                out.push(self.end(kind, "done"));
            } else {
                self.now = Some((kind, left - dt));
            }
            return out;
        }
        if away {
            self.away += dt;
            for kind in [Kind::Eye, Kind::Move] {
                if self.away >= self.plan(kind).secs {
                    self.active[kind as usize] = 0;
                }
            }
            (self.held, self.wait) = (false, 0);
            return out;
        }
        self.away = 0;
        for kind in [Kind::Eye, Kind::Move] {
            let plan = self.plan(kind);
            if !plan.on {
                self.active[kind as usize] = 0;
                continue;
            }
            let before = self.active[kind as usize];
            let after = before + dt;
            self.active[kind as usize] = after;
            let warn_at = plan.every.saturating_sub(self.cfg.warn);
            if self.cfg.warn > 0 && before < warn_at && after >= warn_at && after < plan.every {
                out.push(Ev::Soon(kind));
            }
        }
        // Movement first: it covers an eye break due at the same time.
        let due = |kind: Kind| self.plan(kind).on && self.active[kind as usize] >= self.plan(kind).every;
        let Some(kind) = [Kind::Move, Kind::Eye].into_iter().find(|k| due(*k)) else {
            (self.held, self.wait) = (false, 0);
            return out;
        };
        if self.wait > dt {
            self.wait -= dt;
            return out;
        }
        if busy() {
            if !self.held {
                self.held = true;
                out.push(Ev::Held(kind));
            }
            self.wait = RECHECK;
            return out;
        }
        out.push(self.start(kind));
        out
    }

    fn start(&mut self, kind: Kind) -> Ev {
        let secs = self.plan(kind).secs.max(1);
        self.now = Some((kind, secs));
        (self.held, self.wait) = (false, 0);
        Ev::Start(kind, secs)
    }

    fn end(&mut self, kind: Kind, why: &'static str) -> Ev {
        self.now = None;
        let plan = self.plan(kind);
        if why == "snooze" {
            self.active[kind as usize] = plan.every.saturating_sub(self.cfg.snooze.max(1));
        } else {
            self.active[kind as usize] = 0;
            // Standing up rests the eyes as well.
            if kind == Kind::Move {
                self.active[Kind::Eye as usize] = 0;
            }
        }
        Ev::End(kind, why)
    }

    /// Ends the break on screen, if there is one.
    pub fn stop(&mut self, why: &'static str) -> Option<Ev> {
        self.now.map(|(kind, _)| self.end(kind, why))
    }

    /// Skip: ends the break on screen; with none on screen, calls off the one that was announced.
    pub fn skip(&mut self) -> Option<Ev> {
        if self.now.is_some() {
            return self.stop("skip");
        }
        for kind in [Kind::Eye, Kind::Move] {
            let plan = self.plan(kind);
            if plan.on && self.active[kind as usize] + self.cfg.warn >= plan.every {
                self.active[kind as usize] = 0;
            }
        }
        None
    }

    /// A break now, asked for by hand (it does not wait for a call to end).
    pub fn take(&mut self, kind: Kind) -> Option<Ev> {
        self.now.is_none().then(|| self.start(kind))
    }

    /// For the page: the break on screen, and the seconds at the PC until the next one of each kind.
    pub fn status(&self) -> Value {
        let next = |kind: Kind| {
            let plan = self.plan(kind);
            plan.on.then(|| plan.every.saturating_sub(self.active[kind as usize]))
        };
        json!({
            "break": self.now.map(|(kind, left)| json!({ "kind": kind.name(), "left": left, "total": self.plan(kind).secs.max(1) })),
            "eye": next(Kind::Eye),
            "move": next(Kind::Move),
            "held": self.held,
        })
    }
}
