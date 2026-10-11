// Proves crates/voice/src/{player,mouth,sapi}.rs without a loudspeaker:
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-mouth-probe
//
// The player is given a file output (`player::set_output`) that paces itself in real time like a device
// and keeps what it was handed; everything is checked on that, written to target/voice-dev/mouth/ as
// WAV files and read back. The default output device is opened once, never started, to see that its
// format is accepted: nothing is heard.

use notch_feat_voice::mouth::{self, Say};
use notch_feat_voice::player::{self, Out, BLOCK_MS};
use notch_feat_voice::{sapi, AudioChunk, SynthRequest};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Tape {
    rates: Vec<u32>,
    samples: Vec<f32>,
}

struct FileOut {
    rate: u32,
    tape: Arc<Mutex<Tape>>,
}

impl Out for FileOut {
    fn wait(&mut self) -> usize {
        std::thread::sleep(Duration::from_millis(BLOCK_MS as u64));
        self.rate as usize * BLOCK_MS / 1000
    }
    fn write(&mut self, s: &[f32]) {
        self.tape.lock().unwrap().samples.extend_from_slice(s);
    }
}

fn tape() -> Arc<Mutex<Tape>> {
    wait_idle(); // a stream still lingering from before keeps its old output
    let t = Arc::new(Mutex::new(Tape::default()));
    let t2 = t.clone();
    player::set_output(Some(Arc::new(move |rate| {
        t2.lock().unwrap().rates.push(rate);
        Ok(Box::new(FileOut { rate, tape: t2.clone() }) as Box<dyn Out>)
    })));
    t
}

fn wait_idle() {
    let from = Instant::now();
    while player::running() && from.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wav(path: &str, rate: u32, x: &[f32]) {
    let mut b = Vec::new();
    let data = (x.len() * 2) as u32;
    b.extend(b"RIFF");
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((rate * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    for s in x {
        b.extend(((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

/// Reads our own WAV back: (rate, samples).
fn read_wav(path: &str) -> (u32, Vec<f32>) {
    let b = std::fs::read(path).unwrap();
    let rate = u32::from_le_bytes(b[24..28].try_into().unwrap());
    (rate, b[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect())
}

fn tone(rate: u32, hz: f32, secs: f32) -> AudioChunk {
    let n = (rate as f32 * secs) as usize;
    AudioChunk { rate, samples: (0..n).map(|i| 0.4 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin()).collect() }
}

/// Zero crossings per second of a stretch: the tone's frequency twice.
fn hz(x: &[f32], rate: u32) -> f32 {
    let z = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    z as f32 / 2.0 * rate as f32 / x.len() as f32
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Runs of silence (at most 0.003) longer than `min` samples inside `x`: (start, len).
fn silences(x: &[f32], min: usize) -> Vec<(usize, usize)> {
    let (mut out, mut at) = (Vec::new(), None);
    for (i, s) in x.iter().enumerate() {
        match (s.abs() <= 0.003, at) {
            (true, None) => at = Some(i),
            (false, Some(a)) => {
                if i - a >= min {
                    out.push((a, i - a));
                }
                at = None;
            }
            _ => {}
        }
    }
    if let Some(a) = at {
        if x.len() - a >= min {
            out.push((a, x.len() - a));
        }
    }
    out
}

fn check(name: &str, ok: bool, detail: String) {
    println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
}

fn main() {
    std::fs::create_dir_all("target/voice-dev/mouth").unwrap();
    let events: Arc<Mutex<Vec<Option<(u64, u32)>>>> = Arc::default();
    let e2 = events.clone();
    player::on_play(Some(Arc::new(move |e| e2.lock().unwrap().push(e))));

    // ── 1. Order, two rates, a gap, the level ──────────────────────────────────
    let t = tape();
    let gen = notch_feat_voice::generation();
    let levels = Arc::new(Mutex::new(0f32));
    let l2 = levels.clone();
    let poll = std::thread::spawn(move || {
        while !player::running() {
            std::thread::sleep(Duration::from_millis(2));
        }
        while player::running() {
            let l = player::level();
            let mut m = l2.lock().unwrap();
            *m = m.max(l);
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    // Arrive out of order: 1 (22.05 kHz, 600 Hz) before 0 (24 kHz, 300 Hz); 2 comes in two pieces (16 kHz, 900 Hz).
    player::play(gen, 1, tone(22_050, 600.0, 0.5), true).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    check("nothing before seq 0", t.lock().unwrap().samples.iter().all(|s| s.abs() < 1e-6) && events.lock().unwrap().is_empty(), "silent, no spoke event".into());
    player::play(gen, 0, tone(24_000, 300.0, 0.5), true).unwrap();
    player::play(gen, 2, tone(16_000, 900.0, 0.2), false).unwrap();
    player::play(gen, 2, tone(16_000, 900.0, 0.2), true).unwrap();
    wait_idle();
    poll.join().unwrap();
    let (rates, x) = {
        let t = t.lock().unwrap();
        (t.rates.clone(), t.samples.clone())
    };
    wav("target/voice-dev/mouth/order.wav", t.lock().unwrap().rates[0], &x);
    let (r, back) = read_wav("target/voice-dev/mouth/order.wav");
    check("stream rate", rates == vec![22_050] && r == 22_050, format!("opened {rates:?} (first chunk to arrive), file rate {r}"));
    let rate = rates[0];
    // The stream opened at the first chunk to arrive (seq 1 here); everything after was converted to it.
    let sil = silences(&back, rate as usize / 20);
    let lead = sil.iter().find(|(s, _)| *s == 0).map(|s| s.1).unwrap_or(0);
    let bodies: Vec<(usize, usize)> = {
        let mut at = lead;
        let mut v = Vec::new();
        for (s, l) in sil.iter().filter(|(s, _)| *s > 0) {
            v.push((at, s - at));
            at = s + l;
        }
        if at < back.len() {
            v.push((at, back.len() - at));
        }
        v
    };
    let freqs: Vec<f32> = bodies.iter().filter(|b| b.1 > 1000).map(|b| hz(&back[b.0 + 200..b.0 + b.1 - 200], rate)).collect();
    let near = |a: f32, b: f32| (a - b).abs() < b * 0.03;
    check("order", freqs.len() == 3 && near(freqs[0], 300.0) && near(freqs[1], 600.0) && near(freqs[2], 900.0), format!("pitches in order {freqs:?} Hz (queued 1, 0, 2)"));
    let gaps: Vec<f32> = sil.iter().filter(|(s, l)| *s > 0 && s + l < back.len()).map(|(_, l)| *l as f32 * 1000.0 / rate as f32).collect();
    check("gap", gaps.len() == 2 && gaps.iter().all(|g| (140.0..175.0).contains(g)), format!("gaps {gaps:?} ms between chunks"));
    let total = (back.iter().rposition(|s| s.abs() > 0.003).unwrap() - back.iter().position(|s| s.abs() > 0.003).unwrap()) as f32 / rate as f32;
    check("length", (total - (0.5 + 0.5 + 0.4 + 0.3)).abs() < 0.05, format!("{total:.3} s first to last sound for 1.4 s of audio and two 0.15 s gaps (expected 1.7); the pieces of seq 2 joined"));
    let ev = events.lock().unwrap().clone();
    check("spoke", ev == vec![Some((gen, 0)), Some((gen, 1)), Some((gen, 2)), None], format!("{ev:?}"));
    let lv = *levels.lock().unwrap();
    check("level", lv > 0.3 && player::level() == 0.0 && !player::playing(), format!("peak {lv:.2} while playing, {} after", player::level()));

    // ── 2. A late chunk of an older generation is dropped ─────────────────────
    events.lock().unwrap().clear();
    let old = notch_feat_voice::generation();
    let new = mouth::halt();
    player::play(old, 0, tone(24_000, 440.0, 0.3), true).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    check("stale generation", !player::running() && new > old && events.lock().unwrap().is_empty(), format!("gen {old} dropped after {new}"));

    // ── 3. Stop: the sound is gone within PLAYER_FADE ─────────────────────────
    let t = tape();
    let gen = notch_feat_voice::generation();
    player::play(gen, 0, tone(24_000, 440.0, 3.0), true).unwrap();
    player::play(gen, 1, tone(24_000, 660.0, 1.0), true).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let before = t.lock().unwrap().samples.len();
    let asked = Instant::now();
    player::stop();
    let called = asked.elapsed();
    let at_stop = t.lock().unwrap().samples.len().max(before);
    wait_idle();
    let x = t.lock().unwrap().samples.clone();
    wav("target/voice-dev/mouth/stop.wav", 24_000, &x);
    let (_, back) = read_wav("target/voice-dev/mouth/stop.wav");
    let last_loud = back.iter().rposition(|s| s.abs() > 0.003).unwrap_or(0);
    let lat_samples = last_loud.saturating_sub(before);
    let lat_ms = lat_samples as f32 * 1000.0 / 24_000.0;
    check(
        "stop latency",
        lat_ms <= 30.0 && back[last_loud + 1..].iter().all(|s| s.abs() <= 0.003),
        format!("{lat_samples} samples ({lat_ms:.1} ms at 24 kHz) from the count before stop() to the last sound; stop() itself returned in {called:?}; {} samples were already out", at_stop - before),
    );
    let ramp = &back[before..=last_loud];
    check("fade, no click", ramp.windows(2).all(|w| (w[1] - w[0]).abs() < 0.08), format!("largest step {:.3} over {} samples", ramp.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max), ramp.len()));
    check("queue emptied", player::queued() == 0 && !player::playing() && events.lock().unwrap().last() == Some(&None), format!("queued {}, tail event {:?}", player::queued(), events.lock().unwrap().last()));
    check("tape length", back.len() < 24_000, format!("{} samples, not the 4 s that were queued", back.len()));

    // ── 4. The Windows voices and the mouth ───────────────────────────────────
    let list = sapi::voices();
    println!("INFO installed English Windows voices: {:?}", list.iter().map(|v| format!("{} ({})", v.id, v.lang)).collect::<Vec<_>>());
    check("voices", sapi::ready() && !list.is_empty() && mouth::voices().len() == list.len(), format!("{} voices", list.len()));

    // sapi alone, one voice, to see its own rate and pieces.
    let mut got = Vec::new();
    let req = SynthRequest { text: "Hello there. This is a test.".into(), voice: list[0].name.clone(), rate: 1.1, pitch: 1.0 };
    sapi::synth(&req, &mut |c| {
        got.push(c);
        true
    })
    .unwrap();
    println!("INFO sapi {}: {} piece(s), rate {} Hz, {:.2} s, rms {:.3}", list[0].name, got.len(), got[0].rate, got.iter().map(|c| c.duration().as_secs_f32()).sum::<f32>(), rms(&got[0].samples));

    let t = tape();
    let events_seen: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
    let es = events_seen.clone();
    mouth::begin(Arc::new(move |e, v| es.lock().unwrap().push((e.to_string(), v))));
    events.lock().unwrap().clear();
    let gen = notch_feat_voice::generation();
    let second = list.get(1).map(|v| v.id.clone());
    let started = Instant::now();
    mouth::enqueue(Say { gen, seq: 0, text: "The build finished without any errors.".into(), voice: Some(list[0].id.clone()) }).unwrap();
    mouth::enqueue(Say { gen, seq: 1, text: "Two files changed, and one test was added.".into(), voice: second.clone().or(Some(list[0].id.clone())) }).unwrap();
    mouth::enqueue(Say { gen, seq: 2, text: "Done.".into(), voice: Some(list[0].id.clone()) }).unwrap();
    while !mouth::speaking() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    while mouth::speaking() && started.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    let x = t.lock().unwrap().samples.clone();
    let rates = t.lock().unwrap().rates.clone();
    wav("target/voice-dev/mouth/speech.wav", rates[0], &x);
    let (r, back) = read_wav("target/voice-dev/mouth/speech.wav");
    let sil = silences(&back, rates[0] as usize / 10);
    let ev: Vec<(u64, u64)> = events_seen.lock().unwrap().iter().filter(|(e, _)| e == "spoke").map(|(_, v)| (v["gen"].as_u64().unwrap(), v["seq"].as_u64().unwrap())).collect();
    check(
        "windows voice through the mouth",
        r == rates[0] && rms(&back) > 0.02 && back.len() as f32 / r as f32 > 2.0 && ev == vec![(gen, 0), (gen, 1), (gen, 2)],
        format!("stream {rates:?} Hz, {:.2} s, rms {:.3}, spoke {ev:?}, silent runs over 100 ms: {}", back.len() as f32 / r as f32, rms(&back), sil.len()),
    );
    let seen = events_seen.lock().unwrap();
    let levels_out = seen.iter().filter(|(e, _)| e == "level").count();
    let spoke = seen.iter().filter(|(e, _)| e == "spoke").count();
    check("events", spoke == 3 && levels_out > 3 && seen.iter().any(|(e, v)| e == "state" && v["state"] == "speaking"), format!("{spoke} spoke, {levels_out} level, state speaking set; first level {:?}", seen.iter().find(|(e, _)| e == "level").map(|x| &x.1)));
    drop(seen);
    let first_audio = {
        let first = back.iter().position(|s| s.abs() > 0.003).unwrap_or(0);
        first as f32 * 1000.0 / r as f32
    };
    println!("INFO first sound at {first_audio:.0} ms into the tape (includes the real-time tape pacing)");

    // The phrase cache: "Done." again is instant.
    let t = tape();
    let gen = notch_feat_voice::generation();
    events_seen.lock().unwrap().clear();
    let again = Instant::now();
    mouth::enqueue(Say { gen, seq: 0, text: "Done.".into(), voice: Some(list[0].id.clone()) }).unwrap();
    while !events_seen.lock().unwrap().iter().any(|(e, _)| e == "spoke") && again.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(2));
    }
    println!("INFO cached \"Done.\" started after {} ms", again.elapsed().as_millis());
    while mouth::speaking() {
        std::thread::sleep(Duration::from_millis(20));
    }
    check("cache", t.lock().unwrap().samples.iter().any(|s| s.abs() > 0.02), "same audio again".into());

    // Interruption through the mouth: a long text, halt at once, nothing late arrives.
    let t = tape();
    let gen = notch_feat_voice::generation();
    mouth::enqueue(Say { gen, seq: 0, text: "This sentence is being cut off before it is ever heard by anybody at all.".into(), voice: Some(list[0].id.clone()) }).unwrap();
    let new = mouth::halt();
    std::thread::sleep(Duration::from_millis(800));
    check("halt drops late synthesis", t.lock().unwrap().samples.is_empty() && !player::running() && new > gen && !mouth::speaking(), format!("tape {} samples, gen {gen} -> {new}", t.lock().unwrap().samples.len()));
    mouth::end();

    // ── 5. The real device: opened, never started ─────────────────────────────
    player::set_output(None);
    for rate in [24_000, 22_050] {
        match player::probe_device(rate) {
            Ok(frames) => println!("INFO default output accepted mono float at {rate} Hz, buffer {frames} frames ({:.1} ms); not started", frames as f32 * 1000.0 / rate as f32),
            Err(e) => println!("INFO default output refused {rate} Hz: {e}"),
        }
    }
    println!("DONE");
}
