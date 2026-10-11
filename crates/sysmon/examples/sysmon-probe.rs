// Proves the Windows-facing code of svc-sysmon and of the speed test, without
// the app:  cargo run -p notch-feat-sysmon --example sysmon-probe [-- --speed]
//
// It reads and observes. The one process it ends is a child it starts itself;
// it ejects nothing; and it talks to the network only with `--speed`, once.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use svc_sysmon::procs::{self, Refused};
use svc_sysmon::{Metric, SustainedGate};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::System::WindowsProgramming::QueryProcessCycleTime;

const ALL: [Metric; 7] =
    [Metric::Cpu, Metric::Gpu, Metric::Memory, Metric::Disk, Metric::Network, Metric::Power, Metric::Temps];

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1048576.0)
}

/// Processor cycles this process has used, on every thread. (The kernel's
/// own "CPU time" is counted in 15.6 ms steps and misses work this short.)
fn own_cycles() -> u64 {
    let mut cycles = 0u64;
    unsafe { QueryProcessCycleTime(GetCurrentProcess(), &mut cycles) }.expect("QueryProcessCycleTime");
    cycles
}

/// Cycles per second of the counter those are counted in.
fn cycle_rate() -> f64 {
    let (t0, c0) = (Instant::now(), unsafe { core::arch::x86_64::_rdtsc() });
    std::thread::sleep(Duration::from_millis(250));
    (unsafe { core::arch::x86_64::_rdtsc() } - c0) as f64 / t0.elapsed().as_secs_f64()
}

/// Samples `metrics` for three seconds (the counters open), then for `secs`
/// more, and says what each part cost this process.
fn cost(label: &str, metrics: &[Metric], secs: u64, rate: f64) {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counted = ticks.clone();
    let _sub = svc_sysmon::on_tick(move |_| {
        counted.fetch_add(1, Ordering::Relaxed);
    });
    let start = own_cycles();
    let guard = svc_sysmon::need(metrics);
    std::thread::sleep(Duration::from_secs(3));
    let (before, began) = (own_cycles(), Instant::now());
    ticks.store(0, Ordering::Relaxed);
    std::thread::sleep(Duration::from_secs(secs));
    let (used, wall) = ((own_cycles() - before) as f64 / rate, began.elapsed().as_secs_f64());
    drop(guard);
    let n = ticks.load(Ordering::Relaxed).max(1);
    println!(
        "  {label}: {:.1} ms of CPU over {wall:.1} s ({n} readings, {:.2} ms each) = {:.3} % of one core; starting up cost {:.1} ms",
        used * 1000.0,
        used * 1000.0 / n as f64,
        used / wall * 100.0,
        (before - start) as f64 / rate * 1000.0,
    );
    while svc_sysmon::running() {
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn main() {
    let speed = std::env::args().any(|a| a == "--speed");

    println!("== 1. every card, and five seconds of history ==");
    println!("core classes: {:?}", svc_sysmon::core_classes());
    assert!(!svc_sysmon::running(), "nothing runs before a need");
    let began = Instant::now();
    let _sub = svc_sysmon::on_tick(move |s| {
        println!("  tick at {:>5} ms: cpu {:?}  net down {:?}", began.elapsed().as_millis(), s.cpu, s.net.as_ref().and_then(|n| n.down))
    });
    let guard = svc_sysmon::need(&ALL);
    std::thread::sleep(Duration::from_millis(5300));
    let s = svc_sysmon::snapshot();
    println!("cpu     {:?}  cores {:.2?}", s.cpu, s.cores);
    match &s.gpu {
        Some(g) => println!("gpu     {:?} usage {:?} dedicated {} shared {} ({} processes use it)", g.name, g.usage, mb(g.dedicated), mb(g.shared), s.gpu_pids.len()),
        None => println!("gpu     not readable here: the card hides"),
    }
    println!("memory  {:?}", s.mem);
    println!("network {:?}", s.net);
    if let Some(d) = &s.disk {
        println!("disk    read {:?} write {:?} B/s", d.read, d.write);
        for v in &d.volumes {
            println!("        {} {:?} {} total {} free {} system {} ejectable {}", v.letter, v.label, v.fs, mb(v.total), mb(v.free), v.system, v.ejectable);
        }
    }
    println!("power   {:?}", s.power);
    println!("temps   {:?}{}", s.temps, if s.temps.is_empty() { "  (none readable unelevated: the card hides)" } else { "" });
    let h = svc_sysmon::history();
    println!("history cpu {:.2?}", h.cpu);
    println!("history mem {:.2?}", h.mem);
    println!("history gpu {:.2?}", h.gpu);
    println!("history down {:.0?} up {:.0?}", h.down, h.up);
    println!("history read {:.0?} write {:.0?}", h.read, h.write);
    println!("history battery {:.2?}", h.battery);

    println!("== 2. sampling stops when nobody needs it ==");
    let dropped = Instant::now();
    drop(guard);
    while svc_sysmon::running() {
        std::thread::sleep(Duration::from_millis(1));
    }
    println!("the sampler's thread ended {} ms after the last need was dropped", dropped.elapsed().as_millis());
    std::thread::sleep(Duration::from_millis(2500));
    println!("(no tick line above this one for 2.5 s; running = {}, history kept = {})", svc_sysmon::running(), svc_sysmon::history().cpu.len());
    drop(_sub);

    println!("== 3. top processes ==");
    procs::processes(Duration::ZERO);
    std::thread::sleep(Duration::from_secs(1));
    let mut list = procs::processes(Duration::ZERO);
    println!("{} processes, {} protected", list.len(), list.iter().filter(|p| p.protected).count());
    list.sort_by(|a, b| b.cpu.unwrap_or(0.0).total_cmp(&a.cpu.unwrap_or(0.0)));
    for p in list.iter().take(6) {
        println!("  cpu {:>5.1} %  {:>10}  pid {:>6}  {}", p.cpu.unwrap_or(0.0) * 100.0, mb(p.mem), p.pid, p.name);
    }
    list.sort_by(|a, b| b.mem.cmp(&a.mem));
    for p in list.iter().take(6) {
        println!("  mem {:>10}  pid {:>6}  {}{}", mb(p.mem), p.pid, p.name, if p.protected { "  (protected)" } else { "" });
    }

    println!("== 4. listening ports ==");
    match procs::ports() {
        Ok(ports) => {
            println!("{} open", ports.len());
            for p in &ports {
                println!("  {:>5}  {:<28} pid {:>6}  {}{}{}", p.port, p.address, p.pid, p.name, if p.all { "  [all interfaces]" } else { "" }, if p.protected { "  (protected)" } else { "" });
            }
        }
        Err(e) => println!("the listing failed: {e}"),
    }

    println!("== 5. devices and addresses ==");
    println!("usb devices from outside: {:?}", svc_sysmon::extra::usb_devices());
    for a in svc_sysmon::extra::addresses() {
        println!("  {} ({})", a.ip, a.name);
    }

    println!("== 6. ending a process: only a child of this probe ==");
    // CREATE_NO_WINDOW: nothing appears on screen.
    let mut child = Command::new("ping").args(["-n", "120", "127.0.0.1"]).stdout(Stdio::null()).creation_flags(0x0800_0000).spawn().expect("ping");
    std::thread::sleep(Duration::from_millis(300));
    let me = procs::processes(Duration::ZERO);
    let own = me.iter().find(|p| p.pid == std::process::id()).expect("this probe is in the list");
    let target = me.iter().find(|p| p.pid == child.id()).expect("the child is in the list");
    println!("child: pid {} {} started {} parent {} (this probe is {})", target.pid, target.name, target.started, target.parent, own.pid);
    assert_eq!(target.parent, own.pid);
    println!("  this probe itself         -> {:?}", procs::kill(own.pid, own.started));
    assert_eq!(procs::kill(own.pid, own.started), Err(Refused::Protected));
    println!("  the child, wrong start    -> {:?}", procs::kill(target.pid, target.started + 1));
    assert!(child.try_wait().unwrap().is_none(), "a wrong start time must end nothing");
    println!("  the child, asked to close -> {:?}", procs::ask_to_close(target.pid, target.started));
    println!("  the child                 -> {:?}", procs::kill(target.pid, target.started));
    println!("  it exited with {:?}", child.wait().unwrap().code());
    println!("  once more                 -> {:?}", procs::kill(target.pid, target.started));

    println!("== 7. the sustained-alert gate, threshold 90, a reading every 2 s ==");
    let series: [(u64, Option<f32>); 14] = [
        (0, Some(95.0)), (2, Some(97.0)), (4, Some(40.0)), // a burst: gone before it counts
        (6, Some(92.0)), (8, Some(93.0)), (8, Some(93.0)), // the same reading served twice
        (10, Some(96.0)), (12, Some(91.0)), (14, Some(99.0)), (16, Some(94.0)), (18, Some(95.0)), (20, Some(98.0)),
        (22, None), (24, Some(99.0)), // a missed reading starts it over
    ];
    let mut gate = SustainedGate::default();
    let mut fired = Vec::new();
    for (at, reading) in series {
        let alert = gate.should_alert(reading, 90.0, at * 1000);
        println!("  t={at:>2} s  reading {reading:?}  -> {}", if alert { "ALERT" } else { "-" });
        if alert {
            fired.push(at);
        }
    }
    // Held since t=6: twelve seconds are up at t=18. The feature's cooldown makes the first of these the only notice.
    assert_eq!(fired, [18, 20]);
    println!("  over the limit from t=6: first alert at t={} (12 s held), not before", fired[0]);

    println!("== 8. what the sampler costs this process ==");
    let rate = cycle_rate();
    println!("  (cycle counter: {:.0} MHz)", rate / 1e6);
    cost("every metric", &ALL, 10, rate);
    cost("every metric but the GPU", &[Metric::Cpu, Metric::Memory, Metric::Disk, Metric::Network, Metric::Power, Metric::Temps], 6, rate);
    cost("CPU only (a CPU alert armed)", &[Metric::Cpu], 6, rate);
    let before = own_cycles();
    std::thread::sleep(Duration::from_secs(4));
    println!("  nothing needed: {:.3} ms of CPU over 4 s", (own_cycles() - before) as f64 / rate * 1000.0);

    if speed {
        println!("== 9. speed test: contacts {} only ==", notch_feat_sysmon::speed::HOST);
        tauri::async_runtime::block_on(notch_feat_sysmon::speed::run(|p| println!("  {p:?}")));
    } else {
        println!("== 9. speed test: skipped (pass --speed to run it once; it contacts {}) ==", notch_feat_sysmon::speed::HOST);
    }
    println!("done; sampler running = {}", svc_sysmon::running());
}
