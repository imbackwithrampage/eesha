// Sysmon — a shared service (docs/ARCHITECTURE.md §8). Not a feature: the
// features that need it list this crate in their own Cargo.toml. Refcounted:
// nothing here runs without a live need, and the guard `need` returns gives it
// up on drop.
//
// One thread reads the metrics somebody needs every two seconds and tells the
// subscribers. It starts with the first need and ends with the last: the
// counters it holds open (PDH queries, disk handles) close with it. Everything
// else in this crate is a plain call on the caller's thread: the process list
// (`procs`), the listening ports, the USB devices, the addresses, an eject.
//
// `SustainedGate` is ported from the upstream file
// Sources/Vorssaint/Services/Metrics/SustainedAlertGate.swift.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Serialize;

pub mod extra;
pub mod procs;
mod sample;

pub use sample::{core_classes, Disk, Gpu, Memory, Net, Power, Temp, Volume};

/// Between two readings.
pub const TICK: Duration = Duration::from_secs(2);
/// After a metric was first asked for: its rates need a second reading, soon.
const PRIME: Duration = Duration::from_millis(400);
/// Points kept per history.
pub const HISTORY: usize = 120;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metric {
    Cpu,
    Gpu,
    Memory,
    Disk,
    Network,
    Power,
    Temps,
}
const METRICS: usize = 7;

/// One reading. A metric nobody needs, or that could not be read, is `None`.
#[derive(Clone, Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// When it was read, in milliseconds of the clock that stops while the
    /// machine sleeps: two snapshots with the same stamp are the same reading.
    pub at: u64,
    /// 0…1 over all cores; `None` on the first reading (it only sets the baseline).
    pub cpu: Option<f32>,
    pub cores: Vec<f32>,
    pub gpu: Option<Gpu>,
    pub mem: Option<Memory>,
    pub net: Option<Net>,
    pub disk: Option<Disk>,
    pub power: Option<Power>,
    /// Empty when no temperature can be read without elevation.
    pub temps: Vec<Temp>,
    /// GPU share by process, 0…1 (with the GPU metric).
    #[serde(skip)]
    pub gpu_pids: Vec<(u32, f32)>,
}

/// The last `HISTORY` readings of each series, oldest first. Fractions 0…1
/// for cpu, gpu, mem and battery; bytes per second for the rest.
#[derive(Clone, Default, Debug, Serialize)]
pub struct History {
    pub cpu: Vec<f32>,
    pub gpu: Vec<f32>,
    pub mem: Vec<f32>,
    pub down: Vec<f32>,
    pub up: Vec<f32>,
    pub read: Vec<f32>,
    pub write: Vec<f32>,
    pub battery: Vec<f32>,
}

type Callback = Arc<dyn Fn(&Snapshot) + Send + Sync>;

#[derive(Default)]
struct Svc {
    needs: [u32; METRICS],
    /// Changes with every need taken or given up: the thread reads again at once.
    gen: u64,
    running: bool,
    subs: Vec<(u64, Callback)>,
    next_sub: u64,
    snap: Snapshot,
    hist: History,
}

static WAKE: Condvar = Condvar::new();

fn svc() -> MutexGuard<'static, Svc> {
    static SVC: OnceLock<Mutex<Svc>> = OnceLock::new();
    SVC.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Holds metrics sampled until it is dropped.
pub struct Guard(Vec<Metric>);

/// These metrics are read from now on, the first time at once.
pub fn need(metrics: &[Metric]) -> Guard {
    let mut svc = svc();
    for m in metrics {
        svc.needs[*m as usize] += 1;
    }
    svc.gen += 1;
    if !svc.running {
        svc.running = std::thread::Builder::new().name("sysmon".into()).spawn(run).is_ok();
    }
    WAKE.notify_all();
    Guard(metrics.to_vec())
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut svc = svc();
        for m in &self.0 {
            svc.needs[*m as usize] -= 1;
        }
        svc.gen += 1;
        WAKE.notify_all();
    }
}

/// Ends the calls of `on_tick` when dropped.
pub struct Sub(u64);

/// Called on the sampler's thread after every reading. It keeps nothing
/// running by itself: hold a `need` as well.
pub fn on_tick(f: impl Fn(&Snapshot) + Send + Sync + 'static) -> Sub {
    let mut svc = svc();
    svc.next_sub += 1;
    let id = svc.next_sub;
    svc.subs.push((id, Arc::new(f)));
    Sub(id)
}

impl Drop for Sub {
    fn drop(&mut self) {
        svc().subs.retain(|(id, _)| *id != self.0);
    }
}

/// The last reading (empty while nothing is needed).
pub fn snapshot() -> Snapshot {
    svc().snap.clone()
}

pub fn history() -> History {
    svc().hist.clone()
}

/// Whether the sampler's thread exists.
pub fn running() -> bool {
    svc().running
}

fn push(ring: &mut Vec<f32>, value: Option<f32>, needed: bool) {
    if !needed {
        ring.clear();
    } else if let Some(v) = value {
        if ring.len() >= HISTORY {
            ring.remove(0);
        }
        ring.push(v);
    }
}

fn run() {
    let mut samplers = sample::Samplers::default();
    loop {
        let (needs, gen) = {
            let mut svc = svc();
            if svc.needs.iter().all(|n| *n == 0) {
                svc.running = false;
                svc.snap = Snapshot::default();
                svc.hist = History::default();
                // `samplers` goes with the thread: every query and handle closes.
                return;
            }
            (svc.needs.map(|n| n > 0), svc.gen)
        };
        let began = std::time::Instant::now();
        let (snap, priming) = samplers.read(needs);
        let subs: Vec<Callback> = {
            let mut svc = svc();
            let h = &mut svc.hist;
            let on = |m: Metric| needs[m as usize];
            push(&mut h.cpu, snap.cpu, on(Metric::Cpu));
            push(&mut h.gpu, snap.gpu.as_ref().and_then(|g| g.usage), on(Metric::Gpu));
            push(&mut h.mem, snap.mem.as_ref().map(|m| m.used as f32 / m.total.max(1) as f32), on(Metric::Memory));
            push(&mut h.down, snap.net.as_ref().and_then(|n| n.down).map(|v| v as f32), on(Metric::Network));
            push(&mut h.up, snap.net.as_ref().and_then(|n| n.up).map(|v| v as f32), on(Metric::Network));
            push(&mut h.read, snap.disk.as_ref().and_then(|d| d.read).map(|v| v as f32), on(Metric::Disk));
            push(&mut h.write, snap.disk.as_ref().and_then(|d| d.write).map(|v| v as f32), on(Metric::Disk));
            let level = snap.power.as_ref().and_then(|p| p.percent).map(|p| p as f32 / 100.0);
            push(&mut h.battery, level, on(Metric::Power));
            svc.snap = snap.clone();
            svc.subs.iter().map(|(_, f)| f.clone()).collect()
        };
        for f in &subs {
            f(&snap);
        }
        // What is slow to open opens now, with the reading already out.
        samplers.open(needs);
        let wait = if priming { PRIME } else { TICK }.saturating_sub(began.elapsed());
        drop(WAKE.wait_timeout_while(svc(), wait, |svc| svc.gen == gen));
    }
}

/// Decides when a reading over its limit deserves a notice: it has to hold on
/// separate readings for twelve seconds. One gate per metric.
#[derive(Default)]
pub struct SustainedGate {
    held_since: Option<u64>,
    last: Option<u64>,
}

impl SustainedGate {
    pub const HOLD_MS: u64 = 12_000;

    /// `read_at` is the stamp of the snapshot the reading came from
    /// (`Snapshot::at`): a reading that is served twice counts once, and time
    /// asleep does not count at all.
    pub fn should_alert(&mut self, reading: Option<f32>, threshold: f32, read_at: u64) -> bool {
        if reading.is_none_or(|r| r < threshold) {
            *self = Self::default();
            return false;
        }
        if self.last == Some(read_at) {
            return false;
        }
        self.last = Some(read_at);
        match self.held_since {
            None => {
                self.held_since = Some(read_at);
                false
            }
            Some(since) => read_at.saturating_sub(since) >= Self::HOLD_MS,
        }
    }
}
