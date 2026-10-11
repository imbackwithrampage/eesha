// Proves the Windows-facing parts without the app:
// `cargo run -p notch-feat-audio-extras --example audio-extras-probe`.
//
// It plays nothing and sets nothing. The loopback is opened and read (silent by
// nature); the bars and the beat are fed a click track made in memory; the tap
// is opened and left, to see it close; the volume keys are made-up events given
// to the same code the hook calls, and the real hook is installed for a moment
// with a listener that only counts. The output's level, mute and identity are
// read at both ends and compared. Ends by itself; a watchdog ends it after 30 s
// whatever happens, and the hook goes with the process.
//
// With music playing it also shows the live side: frames, bars that move, and
// (run with `--listen 25`) the tempo of what plays.

use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notch_feat_audio_extras::bands::BANDS;
use notch_feat_audio_extras::keys::{self, next_level, Act, Dir, Keys, Mode, Mods, COARSE, FINE};
use notch_feat_audio_extras::tap::{beat_on_wall, dance_tempo, Hearing, Tap, Told};
use svc_audio::{Flow, Role};
use svc_input::{Key, Verdict};
use taktus::Follower;

fn check(what: &str, ok: bool) {
    println!("  {} {what}", if ok { "ok  " } else { "FAIL" });
}

fn setup() -> String {
    let v = svc_audio::endpoint_volume(Flow::Render).map(|v| format!("{:.4} mute={}", v.scalar, v.mute));
    let id = svc_audio::devices(Flow::Render)
        .ok()
        .and_then(|d| d.into_iter().find(|d| d.default_for.contains(&Role::Console)))
        .map(|d| format!("{} [{}]", d.name, d.id));
    format!("{} | {}", id.unwrap_or_else(|| "no output".into()), v.unwrap_or_else(|e| e))
}

fn meter(levels: [f32; BANDS]) -> String {
    levels.iter().map(|v| format!("{:>4.0}", v * 100.0)).collect::<String>()
}

// ── 1. The loopback ───────────────────────────────────────────────────────────

fn loopback(seconds: f64) {
    println!("\n1. loopback");
    let playing = svc_audio::sessions(None).unwrap_or_default().iter().filter(|s| s.active).count();
    println!("  sessions playing right now: {playing}");
    let mut open = match svc_audio::loopback(None) {
        Ok(open) => open,
        Err(err) => return println!("  FAIL the default output did not open: {err}"),
    };
    let (rate, channels) = (open.rate(), open.channels());
    let (out_rate, out_channels) = open.output_format();
    println!("  output runs at {out_rate} Hz, {out_channels} ch; frames come as float32 {rate} Hz, {channels} ch");
    check("opened on the default output", rate > 0 && channels > 0);

    let mut hearing = Hearing::new(rate, channels);
    let mut follower = Follower::default();
    let (mut buf, mut frames, mut reads, mut peak) = (Vec::new(), 0usize, 0usize, 0f32);
    let (mut sets, mut last) = (0usize, [0f32; BANDS]);
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        buf.clear();
        match open.read(&mut buf, Duration::from_millis(100)) {
            Ok(n) => {
                frames += n;
                reads += usize::from(n > 0);
                peak = buf.iter().fold(peak, |p, s| p.max(s.abs()));
                check_len(&buf, n, channels);
                hearing.take(&buf, 0.1);
                hearing.bars(|l| {
                    sets += 1;
                    last = l;
                });
                let now = start.elapsed().as_secs_f64();
                if follower.hear(hearing.onsets(), now) {
                    if let Some((period, at)) = follower.beat() {
                        println!("  {now:5.1} s: {:.1} beats a minute, a beat at {at:.2} s", 60.0 / period);
                    }
                }
            }
            Err(err) => return println!("  FAIL read: {err}"),
        }
    }
    let took = start.elapsed().as_secs_f64();
    println!("  {frames} frames in {took:.1} s ({reads} reads with sound, {:.0} frames/s), peak {peak:.4}", frames as f64 / took);
    if frames > 0 {
        check("frames delivered at the stream's rate", (frames as f64 / took - f64::from(rate)).abs() < f64::from(rate) * 0.15);
        println!("  bars now:{} ({sets} sets)", meter(last));
    } else {
        println!("  nothing plays: Windows delivers no loopback frames then (not silence). Run again with music on.");
    }
    drop(open);

    // One process and its children: this one, which plays nothing. Windows
    // delivers a process's silence as frames, so the frame path shows even now.
    match svc_audio::loopback(Some(std::process::id())) {
        Ok(mut own) => {
            let (start, mut frames, mut peak) = (Instant::now(), 0usize, 0f32);
            while start.elapsed().as_secs_f64() < 1.0 {
                buf.clear();
                match own.read(&mut buf, Duration::from_millis(100)) {
                    Ok(n) => {
                        check_len(&buf, n, own.channels());
                        frames += n;
                        peak = buf.iter().fold(peak, |p, s| p.max(s.abs()));
                    }
                    Err(err) => return println!("  FAIL process read: {err}"),
                }
            }
            let took = start.elapsed().as_secs_f64();
            println!(
                "  process loopback on this process ({} Hz, {} ch): {frames} frames in {took:.1} s ({:.0} frames/s), peak {peak:.4}",
                own.rate(),
                own.channels(),
                frames as f64 / took
            );
            check("the process's frames arrive at the stream's rate", (frames as f64 / took - f64::from(own.rate())).abs() < f64::from(own.rate()) * 0.15);
        }
        Err(err) => println!("  FAIL process loopback: {err}"),
    }
}

fn check_len(buf: &[f32], frames: usize, channels: u16) {
    if buf.len() != frames * usize::from(channels) {
        println!("  FAIL {} samples for {frames} frames of {channels}", buf.len());
    }
}

// ── 2. The bars and the beat, on a click track ────────────────────────────────

/// A drum on every beat (70 Hz, dying away) and a hiss half way between two,
/// 48 kHz stereo, never played.
fn click_track(bpm: f64, seconds: f64) -> Vec<f32> {
    const RATE: f64 = 48_000.0;
    let period = 60.0 / bpm;
    let mut noise = 0x2545_F491u32;
    let mut out = Vec::with_capacity((seconds * RATE) as usize * 2);
    for i in 0..(seconds * RATE) as usize {
        let t = i as f64 / RATE;
        let since = t % period;
        let drum = 0.8 * (std::f64::consts::TAU * 70.0 * since).sin() * (-since / 0.06).exp();
        let off = (t + period / 2.0) % period;
        noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let hiss = 0.15 * (f64::from(noise >> 8) / f64::from(1u32 << 23) - 1.0) * (-off / 0.02).exp();
        let sample = (drum + hiss) as f32;
        out.extend([sample, sample]);
    }
    out
}

fn click(bpm: f64) {
    let seconds = 24.0;
    let track = click_track(bpm, seconds);
    let mut hearing = Hearing::new(48_000, 2);
    let mut follower = Follower::default();
    let (mut sets, mut loudest, mut on_drum, mut found) = (0usize, [0f32; BANDS], [0f32; BANDS], None);
    let period = 60.0 / bpm;
    for (i, chunk) in track.chunks(4800 * 2).enumerate() {
        let now = (i + 1) as f64 * 0.1;
        hearing.take(chunk, 0.1);
        hearing.bars(|levels| {
            sets += 1;
            for b in 0..BANDS {
                loudest[b] = loudest[b].max(levels[b]);
            }
            // The sets that follow a drum closely, late in the track.
            let t = sets as f64 / 30.0;
            if t > 20.0 && (t % period) > 0.03 && (t % period) < 0.10 {
                for b in 0..BANDS {
                    on_drum[b] = on_drum[b].max(levels[b]);
                }
            }
        });
        if follower.listens(now) {
            if follower.hear(hearing.onsets(), now) && found.is_none() {
                found = follower.beat().map(|b| (now, b));
            }
        } else {
            hearing.rest();
        }
    }
    println!("  click track at {bpm} beats a minute, {seconds} s, fed a tenth of a second at a time:");
    println!("    bars, loudest each:{}   just after a drum:{}", meter(loudest), meter(on_drum));
    check("thirty sets of bars a second", sets == (seconds * 30.0) as usize);
    check("the bass bar leads on the drum", on_drum[0] > 0.6 && on_drum[0] > on_drum[4]);
    check("every bar moved, none over 1", loudest.iter().all(|v| *v > 0.05 && *v <= 1.0));
    match (found, follower.beat()) {
        (Some((when, _)), Some((p, at))) => {
            // The engine may count a slow track in halves: its beats are then the drums and the hisses.
            let near = |grid: f64| (at % grid).min(grid - at % grid) * 1000.0;
            let (dance, wall) = beat_on_wall(p, at, seconds, 1_000_000);
            println!(
                "    beat first told {when:.1} s in; read at {:.2} a minute, a beat {:.0} ms from a click; listened {:.1} of {seconds} s",
                60.0 / p,
                near(p),
                follower.listened
            );
            check("the tempo to dance at is the track's, within 1 %", (dance - bpm).abs() < bpm * 0.01);
            check("a beat within 40 ms of a click", near(p) < 40.0);
            println!("    told the page: {dance:.2} a minute, last beat {} ms ago, {:.0} ms from a drum", 1_000_000 - wall, near(period));
            check("the page's beat is the last one before now", (1_000_000 - wall) as f64 <= p * 1000.0 + 1.0);
        }
        _ => check("a beat was found", false),
    }
}

fn beat() {
    println!("\n2. bars and beat, on a click track made in memory (not played)");
    click(126.0);
    click(92.0);
    println!("  dance tempo: 63 -> {}, 190 -> {}, 120 -> {}, 81 -> {}", dance_tempo(63.0), dance_tempo(190.0), dance_tempo(120.0), dance_tempo(81.0));
    check("folded into 80 to 160", dance_tempo(63.0) == 126.0 && dance_tempo(190.0) == 95.0 && dance_tempo(120.0) == 120.0);
}

// ── 3. The tap opens for a watcher and closes without one ─────────────────────

fn tap() {
    println!("\n3. the tap");
    let levels = Arc::new(AtomicUsize::new(0));
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let tap = {
        let (levels, said) = (levels.clone(), said.clone());
        Tap::new(move |told| match told {
            Told::Levels(Some(_)) => {
                levels.fetch_add(1, SeqCst);
            }
            Told::Levels(None) => said.lock().unwrap().push("bars back to their own motion".into()),
            Told::Beat(beat) => said.lock().unwrap().push(format!("beat {beat:?}")),
            Told::Quiet(why) => said.lock().unwrap().push(format!("gave up: {why}")),
        })
    };
    let wait = |open: bool, most: f64| {
        let start = Instant::now();
        while tap.open() != open && start.elapsed().as_secs_f64() < most {
            std::thread::sleep(Duration::from_millis(10));
        }
        start.elapsed().as_secs_f64()
    };
    check("closed before anyone looks", !tap.open());
    tap.watch(false, false, true);
    std::thread::sleep(Duration::from_millis(300));
    check("music plays but nobody looks: still closed", !tap.open());
    tap.watch(true, false, false);
    let took = wait(true, 2.0);
    check(&format!("the bars are looked at: open after {took:.2} s"), tap.open());
    std::thread::sleep(Duration::from_millis(1000));
    let told = levels.load(SeqCst);
    check(&format!("{told} sets of bars in the second that followed"), (20..=40).contains(&told));
    tap.watch(false, false, false);
    let took = wait(false, 4.0);
    check(&format!("nobody looks: closed after {took:.2} s (it lingers 1.5 s for a skip)"), !tap.open() && took > 1.0);
    let after = levels.load(SeqCst);
    std::thread::sleep(Duration::from_millis(300));
    check("and tells nothing more", levels.load(SeqCst) == after);

    // Looked at while nothing sounds: given up after 8 s, until the next play.
    if svc_audio::sessions(None).unwrap_or_default().iter().any(|s| s.active) {
        println!("  something plays: the silence fallback is not shown this time");
    } else {
        tap.watch(false, true, true);
        wait(true, 2.0);
        check("the mascot is looked at while music plays: open", tap.open());
        let took = wait(false, 11.0);
        check(&format!("silence: given up and closed after {took:.1} s"), !tap.open() && (7.0..10.5).contains(&took));
        tap.watch(false, false, false);
    }
    for line in said.lock().unwrap().iter() {
        println!("  told: {line}");
    }
}

// ── 4. The volume keys ────────────────────────────────────────────────────────

fn key(vk: u16, down: bool, ms: u64) -> Key {
    Key { vk, scan: 0, extended: true, down, injected: false, t: ms * 1000 }
}

fn volume_keys() {
    println!("\n4. volume keys (made-up events; no key is sent, no level is written)");
    let (plain, shift, ctrl) = (Mods::default(), Mods { shift: true, other: false }, Mods { shift: false, other: true });
    let fine = Mode { fine: true, replace: false };
    let replace = Mode { fine: false, replace: true };
    let both = Mode { fine: true, replace: true };

    let mut k = Keys::new(fine);
    check("fine steps: Volume Up alone is left to Windows", k.key(&key(keys::UP, true, 0), plain) == (Verdict::Pass, None));
    check("  and so is its release", k.key(&key(keys::UP, false, 40), plain) == (Verdict::Pass, None));
    check(
        "fine steps: Shift+Volume Up is taken and moves 1/64",
        k.key(&key(keys::UP, true, 500), shift) == (Verdict::Swallow, Some(Act::Step(Dir::Up, FINE))),
    );
    check(
        "  its repeat 33 ms later too, even with Shift let go",
        k.key(&key(keys::UP, true, 533), plain) == (Verdict::Swallow, Some(Act::Step(Dir::Up, FINE))),
    );
    check("  a repeat 10 ms later is taken but moves nothing (the gate)", k.key(&key(keys::UP, true, 543), shift) == (Verdict::Swallow, None));
    check("  its release is swallowed", k.key(&key(keys::UP, false, 600), plain) == (Verdict::Swallow, None));
    check("fine steps: Shift+Mute is left to Windows", k.key(&key(keys::MUTE, true, 900), shift) == (Verdict::Pass, None));
    k.key(&key(keys::MUTE, false, 950), shift);
    check("Ctrl+Shift+Volume Down is somebody's shortcut: left alone", {
        let both_mods = Mods { shift: true, other: true };
        k.key(&key(keys::DOWN, true, 1500), both_mods) == (Verdict::Pass, None) && k.key(&key(keys::DOWN, false, 1550), plain) == (Verdict::Pass, None)
    });
    check("a volume key another program sends is passed on", {
        let mut sent = key(keys::UP, true, 2000);
        sent.injected = true;
        k.key(&sent, shift) == (Verdict::Pass, None)
    });
    check("any other key is passed on", k.key(&key(0x41, true, 2100), shift) == (Verdict::Pass, None));

    let mut k = Keys::new(replace);
    check(
        "replace mode: Volume Down is taken and moves 2 %",
        k.key(&key(keys::DOWN, true, 0), plain) == (Verdict::Swallow, Some(Act::Step(Dir::Down, COARSE))),
    );
    k.key(&key(keys::DOWN, false, 50), plain);
    check("replace mode: Shift changes nothing while fine steps are off", {
        let got = k.key(&key(keys::DOWN, true, 400), shift);
        k.key(&key(keys::DOWN, false, 450), shift);
        got == (Verdict::Swallow, Some(Act::Step(Dir::Down, COARSE)))
    });
    check("replace mode: Mute is taken, once per press", {
        let first = k.key(&key(keys::MUTE, true, 1000), plain);
        let held = k.key(&key(keys::MUTE, true, 1500), plain);
        let up = k.key(&key(keys::MUTE, false, 1600), plain);
        first == (Verdict::Swallow, Some(Act::Mute)) && held == (Verdict::Swallow, None) && up == (Verdict::Swallow, None)
    });
    check("replace mode: Ctrl+Volume Up is left to Windows", k.key(&key(keys::UP, true, 2000), ctrl) == (Verdict::Pass, None));
    k.key(&key(keys::UP, false, 2050), ctrl);
    check("a wheel that jitters back within 0.3 s is ignored twice, then believed", {
        let mut steps = Vec::new();
        for (i, vk) in [keys::UP, keys::DOWN, keys::DOWN, keys::DOWN].into_iter().enumerate() {
            let at = 3000 + i as u64 * 50;
            steps.push(k.key(&key(vk, true, at), plain).1.is_some());
            k.key(&key(vk, false, at + 20), plain);
        }
        steps == [true, false, false, true]
    });
    let mut k = Keys::new(both);
    check(
        "both on: Shift+Volume Up moves 1/64, alone 2 %",
        k.key(&key(keys::UP, true, 0), shift).1 == Some(Act::Step(Dir::Up, FINE)) && {
            k.key(&key(keys::UP, false, 30), shift);
            k.key(&key(keys::UP, true, 400), plain).1 == Some(Act::Step(Dir::Up, COARSE))
        },
    );

    // The arithmetic, from where the output stands now (read only).
    let now = svc_audio::endpoint_volume(Flow::Render).map(|v| v.scalar).unwrap_or(0.5);
    let walk = |from: f32, dir: Dir, step: f32, n: usize| {
        (0..n).scan(from, |level, _| {
            *level = next_level(*level, dir, step);
            Some(*level)
        })
        .collect::<Vec<f32>>()
    };
    let show = |v: &[f32]| v.iter().map(|l| format!("{:.4}", l)).collect::<Vec<_>>().join(" ");
    let up = walk(now, Dir::Up, FINE, 4);
    let back = walk(*up.last().unwrap(), Dir::Down, FINE, 4);
    println!("  the output is at {now:.4} ({:.2}/64); fine up x4: {}; fine down x4: {}", now * 64.0, show(&up), show(&back));
    check("every fine step lands on a 64th", up.iter().chain(&back).all(|l| (l * 64.0 - (l * 64.0).round()).abs() < 1e-4));
    check("no step moves more than 1/64, none backwards", std::iter::once(&now).chain(&up).zip(&up).all(|(a, b)| b >= a && b - a <= FINE + 1e-6));
    check("from a 64th: up then down comes back", next_level(next_level(0.5, Dir::Up, FINE), Dir::Down, FINE) == 0.5);
    check("0.5 up is 33/64 = 0.515625", next_level(0.5, Dir::Up, FINE) == 33.0 / 64.0);
    check("0.51 up is 0.515625, down is 0.5", next_level(0.51, Dir::Up, FINE) == 33.0 / 64.0 && next_level(0.51, Dir::Down, FINE) == 0.5);
    check("the ends hold: 1 up is 1, 0 down is 0", next_level(1.0, Dir::Up, FINE) == 1.0 && next_level(0.0, Dir::Down, FINE) == 0.0);
    check("2 % steps: 0.50 up is 0.52, down is 0.48", (next_level(0.5, Dir::Up, COARSE) - 0.52).abs() < 1e-6 && (next_level(0.5, Dir::Down, COARSE) - 0.48).abs() < 1e-6);

    // The real stage on the real hook, with a listener that only counts.
    let before = svc_input::status();
    let pressed = Arc::new(AtomicUsize::new(0));
    let sub = {
        let pressed = pressed.clone();
        keys::hook(fine, move |_| {
            pressed.fetch_add(1, SeqCst);
        })
    };
    let on = svc_input::status();
    std::thread::sleep(Duration::from_millis(300));
    drop(sub);
    let off = svc_input::status();
    println!(
        "  keyboard hook: before {} ({} subscribers), with the stage {} ({}), after {} ({}); real volume presses seen meanwhile: {}",
        before.keyboard, before.keyboard_subscribers, on.keyboard, on.keyboard_subscribers, off.keyboard, off.keyboard_subscribers, pressed.load(SeqCst)
    );
    check("the hook is there with the stage and gone without it", !before.keyboard && on.keyboard && on.keyboard_subscribers == 1 && !off.keyboard);
}

fn main() {
    // Whatever happens, this ends: the process takes its hook and its stream with it.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30));
        println!("watchdog: 30 s, ending");
        std::process::exit(3);
    });
    let args: Vec<String> = std::env::args().collect();
    let listen = args.iter().position(|a| a == "--listen").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok());
    let before = setup();
    println!("audio setup before: {before}");
    loopback(listen.unwrap_or(2.0f64).min(25.0));
    if listen.is_none() {
        beat();
        tap();
        volume_keys();
    }
    let after = setup();
    println!("\naudio setup after:  {after}");
    check("the audio setup is as it was", before == after);
}
