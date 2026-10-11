// Proves crates/voice/src/{ears,vad,turn,stt}.rs without a microphone and without a sound:
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-ears-probe [-- <part of a clip's name>]
//   … --example voice-ears-probe -- download    (the engine and the model, 19 MB, into target/voice-dev/ears/whistle)
//
// 1. The talk key, as made-up key events through `ears::TalkKey`: tap, hold, and with another key.
// 2. The end of a turn, as made-up polls and transcripts through `turn::Turn`: every row of VOICE §3.4.
// 3. The spike's clips (target/voice-spike/audio) through the capture path's entry, `ears::feed`: a file
//    stands in for the microphone, 30 ms a poll, and the engine of target/dictation-probe transcribes.
//    Per clip: where the voice was, how the turn ended and by which rule, the partial and the final
//    text, and the text the spike got from the same file.
//
// The microphone is never opened. The app's log of this run goes to target/voice-dev/ears/.

use notch_feat_voice::ears::{self, Capture, Talk, TalkKey};
use notch_feat_voice::stt::{self, SttOptions};
use notch_feat_voice::timing::*;
use notch_feat_voice::turn::{Turn, TurnEnd, TurnStep};
use notch_feat_voice::vad::VadEvent;
use notch_feat_voice::{wakephrase, AudioChunk, Emit, ListenKind, Segment};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static FAILED: AtomicU32 = AtomicU32::new(0);

fn check(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        FAILED.fetch_add(1, SeqCst);
        "FAIL"
    }
}

// ── 1. The talk key ───────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Ev {
    /// The talk key goes down; `true`: nothing else was held.
    Down(bool),
    /// Another key, a click or the wheel.
    Other,
    Up,
}

/// Made-up events at their milliseconds, with the 0.4 s clock run as the key's thread runs it.
fn play(hold: bool, events: &[(u64, Ev)]) -> Vec<Talk> {
    let mut key = TalkKey::new(hold);
    let mut did = Vec::new();
    for &(ms, ev) in events {
        if let Some(due) = key.due().filter(|due| *due <= ms * 1000) {
            did.push(key.tick(due));
        }
        did.push(match ev {
            Ev::Down(alone) => key.press(ms * 1000, alone),
            Ev::Other => key.other(),
            Ev::Up => key.release(),
        });
    }
    did.retain(|talk| *talk != Talk::Nothing);
    did
}

fn keys() {
    use Ev::*;
    use Talk::*;
    println!("── the talk key (Right Ctrl), made-up events, hold after {} ms ──", HOLD_AFTER.as_millis());
    let cases: &[(&str, bool, &[(u64, Ev)], &[Talk])] = &[
        ("down 0, up 120", true, &[(0, Down(true)), (120, Up)], &[Tap]),
        ("down 0, up 390", true, &[(0, Down(true)), (390, Up)], &[Tap]),
        ("down 0, up 410", true, &[(0, Down(true)), (410, Up)], &[Hold, Release]),
        ("down 0, up 1500", true, &[(0, Down(true)), (1500, Up)], &[Hold, Release]),
        ("down 0, its own repeats, up 300", true, &[(0, Down(true)), (250, Down(true)), (280, Down(true)), (300, Up)], &[Tap]),
        ("down 0, C at 80, up 200 (Ctrl+C)", true, &[(0, Down(true)), (80, Other), (200, Up)], &[]),
        ("down 0, C at 100, up 900 (Ctrl+C, slowly)", true, &[(0, Down(true)), (100, Other), (900, Up)], &[]),
        ("down 0, a click at 150, up 300 (Ctrl+click)", true, &[(0, Down(true)), (150, Other), (300, Up)], &[]),
        ("Shift already down, down 0, up 100", true, &[(0, Down(false)), (100, Up)], &[]),
        ("Shift already down, down 0, up 900", true, &[(0, Down(false)), (900, Up)], &[]),
        ("down 0, a key at 700 while talking, up 1500", true, &[(0, Down(true)), (700, Other), (1500, Up)], &[Hold, Release]),
        ("two taps", true, &[(0, Down(true)), (100, Up), (600, Down(true)), (700, Up)], &[Tap, Tap]),
        ("an up without its down", true, &[(0, Up)], &[]),
        ("hold switched off: down 0, up 1500", false, &[(0, Down(true)), (1500, Up)], &[Tap]),
    ];
    for (name, hold, events, want) in cases {
        let did = play(*hold, events);
        println!("  {name:<46} → {:<16} {}", format!("{did:?}"), check(did == *want));
    }
}

// ── 2. The end of a turn ──────────────────────────────────────────────────────

const POLL: f32 = 0.03;

/// A turn through made-up polls: `voiced` are the spans of voice in seconds, `heard` the transcripts
/// handed to it at each `Provisional` (the last one again after that), `up` when the key is released.
/// Answers the end, when it came, and when the provisional transcripts were asked for.
fn drive(mut turn: Turn, voiced: &[(f32, f32)], heard: &[&str], up: Option<f32>) -> (TurnEnd, f32, Vec<f32>) {
    let (mut asked, mut n) = (Vec::new(), 0usize);
    loop {
        let t = n as f32 * POLL;
        n += 1;
        if up.is_some_and(|up| t >= up) {
            turn.release();
        }
        match turn.step(voiced.iter().any(|(from, to)| t >= *from && t < *to), Duration::from_secs_f32(POLL)) {
            TurnStep::Wait => {}
            TurnStep::Provisional => {
                turn.extend(heard.get(asked.len()).or(heard.last()).copied().unwrap_or(""));
                asked.push(t + POLL);
            }
            TurnStep::End(end) => return (end, t + POLL, asked),
        }
    }
}

fn turns() {
    use ListenKind::*;
    use TurnEnd::*;
    println!("── the end of a turn, made-up polls of 30 ms (provisional after {} ms quiet) ──", PROVISIONAL_AFTER.as_millis());
    let turn = |kind| Turn::new(kind, false);
    let cases: Vec<(&str, Turn, &[(f32, f32)], &[&str], Option<f32>, TurnEnd, f32)> = vec![
        ("command, voice 0.5 to 2.0: \"pause the music\"", turn(Command), &[(0.5, 2.0)], &["pause the music"], None, Silence, 3.6),
        ("… ending on \"and\"", turn(Command), &[(0.5, 2.0)], &["pause the music and"], None, Silence, 4.6),
        ("… ending on \"to\" with a full stop", turn(Command), &[(0.5, 2.0)], &["Remind me to."], None, Silence, 4.6),
        ("free text, voice 0.5 to 2.0", turn(FreeText), &[(0.5, 2.0)], &["tell her I am late"], None, Silence, 5.2),
        ("voice again at 3.0 to 4.0, new words", turn(Command), &[(0.5, 2.0), (3.0, 4.0)], &["set a timer", "set a timer for ten minutes"], None, Silence, 5.6),
        ("a click at 3.0, no new word", turn(Command), &[(0.5, 2.0), (3.0, 3.03)], &["pause the music"], None, Silence, 3.66),
        ("an \"uhh\" at 3.3 to 3.55, no new word", turn(Command), &[(0.5, 2.0), (3.3, 3.55)], &["pause the music"], None, Silence, 4.2),
        ("command, no voice", turn(Command), &[], &[], None, NoSpeech, 5.0),
        ("reply, no voice", turn(Reply), &[], &[], None, NoSpeech, 8.0),
        ("yes or no, no voice", turn(YesNo), &[], &[], None, NoSpeech, 5.0),
        ("a dropped file awaited, no voice", turn(File), &[], &[], None, NoSpeech, 20.0),
        ("sound and never a word", turn(Command), &[(1.0, 1.5)], &[""], None, NoSpeech, 5.0),
        ("voice that never stops", turn(Command), &[(0.5, 60.0)], &[], None, Limit, 20.0),
        ("key held, voice 0.3 to 5.0 with a 3 s pause, up at 5.2", Turn::new(Command, true), &[(0.3, 1.0), (4.0, 5.0)], &[], Some(5.2), Released, 5.4),
        ("key held and never let go", Turn::new(Command, true), &[(0.3, 2.0)], &[], None, Limit, 20.0),
        ("tapped again at 2.5 while it listens", turn(Command), &[(0.5, 2.0)], &["pause the music"], Some(2.5), Released, 2.7),
        ("after the wake phrase and nothing more", turn(Command).heard(SEGMENT_END_WAITING), &[], &[""], None, NoSpeech, 5.0),
        ("after the wake phrase and a command in one breath", turn(Command).heard(SEGMENT_END_WAITING), &[], &["set a timer for twelve minutes"], None, Silence, 1.1),
    ];
    for (name, turn, voiced, heard, up, want, at) in cases {
        let (end, ended, asked) = drive(turn, voiced, heard, up);
        let asked: Vec<String> = asked.iter().map(|t| format!("{t:.2}")).collect();
        let ok = end == want && (ended - at).abs() < 0.05;
        println!("  {name:<52} → {end:?} at {ended:.2} s (wanted {at:.2}), transcripts asked at [{}] {}", asked.join(", "), check(ok));
    }
}

// ── 3. The clips ──────────────────────────────────────────────────────────────

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")
}

/// 16-bit mono PCM at the capture rate, as the spike wrote its clips.
fn read_wav(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if &bytes[at..at + 4] == b"fmt " {
            let (channels, rate, bits) = (body[2], u32::from_le_bytes([body[4], body[5], body[6], body[7]]), body[14]);
            assert!(channels == 1 && rate == CAPTURE_RATE && bits == 16, "{}: not 16 kHz mono 16-bit", path.display());
        }
        if &bytes[at..at + 4] == b"data" {
            return body.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        }
        at += 8 + size + size % 2;
    }
    Vec::new()
}

/// What the path sent out, kept for the probe to read.
type Events = Arc<Mutex<Vec<(String, Value)>>>;

/// The capture path with a file for a microphone.
struct Line {
    core: Arc<Mutex<Capture>>,
    events: Events,
    dir: PathBuf,
    /// As the microphone's thread runs it: a poll every 30 ms of the clock, the engine on a thread of its own.
    live: bool,
    /// Samples fed: the line's clock.
    fed: usize,
    /// Partial texts, the final one, the `voice:turn` numbers, and the detection's segments in seconds.
    partials: Vec<String>,
    last: Option<String>,
    turn: Value,
    segments: Vec<(f32, f32)>,
    begun: Option<f32>,
}

impl Line {
    fn new(dir: &Path) -> Line {
        let events: Events = Arc::default();
        let sent = events.clone();
        let emit: Emit = Arc::new(move |event, payload| sent.lock().unwrap().push((event.to_string(), payload)));
        Line { core: Arc::new(Mutex::new(Capture::new(emit))), events, dir: dir.to_path_buf(), live: false, fed: 0, partials: Vec::new(), last: None, turn: Value::Null, segments: Vec::new(), begun: None }
    }

    fn now(&self) -> f32 {
        self.fed as f32 / CAPTURE_RATE as f32
    }

    /// One poll through `ears::feed`, the engine run at once where the microphone would hand it to a thread.
    fn poll(&mut self, frame: &[f32]) {
        let step = ears::feed(&self.core, frame);
        self.fed += frame.len();
        match step.vad {
            VadEvent::Start => self.begun = Some(self.now()),
            VadEvent::End { .. } => self.segments.push((self.begun.take().unwrap_or(0.0), self.now())),
            VadEvent::None => {}
        }
        if step.woke == Some(true) {
            self.segments.push((self.begun.take().unwrap_or(0.0), self.now()));
        }
        match step.job {
            Some(job) if self.live => {
                let (core, dir) = (self.core.clone(), self.dir.clone());
                std::thread::spawn(move || ears::transcribe(&core, &dir, job));
            }
            Some(job) => ears::transcribe(&self.core, &self.dir, job),
            None => {}
        }
        if self.live {
            std::thread::sleep(CAPTURE_POLL);
        }
        for (event, payload) in self.events.lock().unwrap().drain(..) {
            match event.as_str() {
                "heard" if payload["final"] == true => self.last = payload["text"].as_str().map(str::to_string),
                "heard" => self.partials.push(payload["text"].as_str().unwrap_or("").to_string()),
                "turn" => self.turn = payload,
                _ => {}
            }
        }
    }

    /// A clip in polls of 30 ms, then the room until the turn is over (or `limit` seconds of it).
    fn play(&mut self, clip: &[f32], room: &[f32], limit: f32, mut each: impl FnMut(&mut Line)) {
        let poll = (CAPTURE_POLL.as_secs_f32() * CAPTURE_RATE as f32) as usize;
        let mut source: Vec<f32> = clip.to_vec();
        let (mut at, until) = (0, self.fed + clip.len() + (limit * CAPTURE_RATE as f32) as usize);
        while self.last.is_none() && self.fed < until {
            while source.len() < at + poll {
                if room.is_empty() {
                    source.resize(at + poll, 0.0);
                } else {
                    source.extend_from_slice(room);
                }
            }
            self.poll(&source[at..at + poll]);
            at += poll;
            each(self);
        }
    }

    /// "end silence by the 1600 ms rule, voice 420 to 2220 ms of 3840, 3 runs 512 ms"
    fn ended(&self) -> String {
        let t = &self.turn;
        format!(
            "end {} by the {} ms rule | voice {} to {} ms of {} | engine {} ms in {} runs",
            t["end"].as_str().unwrap_or("?"),
            t["rule"],
            t["voice"][0],
            t["voice"][1],
            t["audio"],
            t["stt"],
            t["runs"]
        )
    }
}

/// The room after a clip: its own noise for the white-noise clips (their first 0.3 s holds nothing
/// else), silence for the others. The competing voices of the "babble" clips stop with the file.
fn room(name: &str, clip: &[f32]) -> Vec<f32> {
    if name.contains("white") {
        clip[..clip.len().min(4800)].to_vec()
    } else {
        Vec::new()
    }
}

fn words(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// The spellings the spike found the engine to waver between on one and the same sentence
/// (the spike's notes, §13.4): set, said, sent · 12, twelve · 20%, twenty per cent · tomorrow, to morrow.
fn settled(text: &str) -> String {
    let spaced: String = text.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect();
    spaced
        .split_whitespace()
        .filter_map(|word| match word {
            "said" | "sent" => Some("set"),
            "twelve" => Some("12"),
            "twenty" => Some("20"),
            "per" | "cent" | "percent" => None,
            word => Some(word),
        })
        .collect()
}

/// "=" the same text, "~" the same but for case, spaces and punctuation, "≈" the same but for the
/// engine's own wavering (`settled`), "≠" other words.
fn compare(ours: &str, theirs: &str) -> &'static str {
    if ours == theirs {
        "="
    } else if words(ours) == words(theirs) {
        "~"
    } else if settled(ours) == settled(theirs) {
        "≈"
    } else {
        "≠"
    }
}

/// The spike's text for a clip: `whistle_results.json`, without (`nokw`) or with (`kw`) keywords.
fn spike(results: &Value, name: &str, mode: &str) -> String {
    let parts: Vec<&str> = name.split("__").collect();
    results
        .as_array()
        .and_then(|all| all.iter().find(|r| r["key"] == parts[0] && r["voice"] == parts[1] && r["rate"] == parts[2] && r["cond"] == parts[3] && r["mode"] == mode))
        .and_then(|r| r["text"].as_str())
        .unwrap_or("")
        .to_string()
}

fn english() -> SttOptions {
    SttOptions { language: Some("en".into()), keywords: Vec::new() }
}

fn clips(filter: &str) {
    let dir = root().join("dictation-probe");
    let audio = root().join("voice-spike/audio");
    let from = Instant::now();
    let ready = stt::ready(&dir);
    let first = from.elapsed();
    let from = Instant::now();
    let again = stt::ready(&dir);
    println!("── the spike's clips through the capture path ──");
    println!("  engine and model in {}: ready {ready} (hashed in {} ms; asked again: {again}, {} µs)", dir.display(), first.as_millis(), from.elapsed().as_micros());
    let results: Value = std::fs::read(root().join("voice-spike/whistle_results.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let mut names: Vec<String> = std::fs::read_dir(&audio).map(|d| d.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".wav")).map(str::to_string)).collect()).unwrap_or_default();
    names.sort();
    names.retain(|n| n.contains(filter));
    if !ready || names.is_empty() {
        return println!("  SKIPPED: no engine or no clips ({} clips)", names.len());
    }

    // 3a. A tap, then a command: the microphone opens cold with the clip (its first word comes 0.4 s in,
    // while the floor is still being learned).
    println!("  a tap, then the clip (commands; the engine is asked without keywords, as the spike's \"nokw\"):");
    let (mut exact, mut same, mut whole, mut count, mut ends) = (0, 0, 0, 0, std::collections::BTreeMap::<String, u32>::new());
    let mut engine: Vec<u64> = Vec::new();
    for name in names.iter().filter(|n| n.starts_with("c_")) {
        let clip = read_wav(&audio.join(format!("{name}.wav")));
        let theirs = spike(&results, name, "nokw");
        // The engine on the whole file, as the spike ran it: the options and the WAV are the same.
        let file = stt::transcribe(&dir, &AudioChunk { rate: CAPTURE_RATE, samples: clip.clone() }, &english()).unwrap_or_else(|e| format!("({e})"));
        let mut line = Line::new(&dir);
        line.core.lock().unwrap().opened();
        line.core.lock().unwrap().listen(ListenKind::Command, false, 1);
        line.play(&clip, &room(name, &clip), 25.0, |_| {});
        let ours = line.last.clone().unwrap_or_else(|| "(no end)".into());
        count += 1;
        whole += (file == theirs) as u32;
        exact += (ours == theirs) as u32;
        same += (settled(&ours) == settled(&theirs)) as u32;
        *ends.entry(format!("{} {} ms", line.turn["end"].as_str().unwrap_or("?"), line.turn["rule"])).or_default() += 1;
        engine.push(line.turn["stt"].as_u64().unwrap_or(0) / line.turn["runs"].as_u64().unwrap_or(1).max(1));
        println!("    {name:<32} {} | at {:.2} s", line.ended(), line.now());
        println!("      partials {:?}", line.partials);
        println!("      final \"{ours}\" {} spike \"{theirs}\" (whole file here: {})", compare(&ours, &theirs), compare(&file, &theirs));
    }
    if count > 0 {
        engine.sort();
        println!("  commands: {count} clips | engine on the whole file = the spike's text: {whole} | the turn's final = the spike's text: {exact}, the same but for the engine's wavering (set/said, 12/twelve, 20%/twenty per cent, to morrow): {same} | ends: {ends:?} | median engine run about {} ms", engine[engine.len() / 2]);
        FAILED.fetch_add((whole != count || same * 10 < count * 6) as u32, SeqCst);
    }

    // 3b. The key held from before the first word, let go after the last; and a second tap.
    println!("  the key held through the clip, let go 0.3 s after it; and a second tap 0.3 s after the last word:");
    for name in names.iter().filter(|n| n.starts_with("c_") && n.contains("david__r10")) {
        let clip = read_wav(&audio.join(format!("{name}.wav")));
        for (what, held) in [("held", true), ("tapped again", false)] {
            let mut line = Line::new(&dir);
            line.core.lock().unwrap().opened();
            line.core.lock().unwrap().listen(ListenKind::Command, held, 1);
            let up = clip.len() as f32 / CAPTURE_RATE as f32 + if held { 0.3 } else { -0.7 };
            let mut released = false;
            line.play(&clip, &room(name, &clip), 25.0, |line| {
                if !released && line.now() >= up {
                    released = true;
                    line.core.lock().unwrap().release();
                }
            });
            let ours = line.last.clone().unwrap_or_else(|| "(no end)".into());
            let ok = line.turn["end"] == "released" && (line.now() - up - HOLD_RELEASE_TAIL.as_secs_f32()).abs() < 0.07 && !ours.is_empty();
            println!("    {name:<32} {what}, key up at {up:.2} s: {} | at {:.2} s | final \"{ours}\" {}", line.ended(), line.now(), check(ok));
        }
    }

    // 3b'. The same turn as the microphone's thread runs it: the clock's pace, the engine beside it, its
    // answers arriving a few polls later.
    println!("  at the clock's pace, the engine on its own thread (as the capture thread does it):");
    for name in names.iter().filter(|n| n.starts_with("c_") && n.contains("zira__r10") && !n.contains("babble")) {
        let clip = read_wav(&audio.join(format!("{name}.wav")));
        let mut line = Line::new(&dir);
        line.live = true;
        line.core.lock().unwrap().opened();
        line.core.lock().unwrap().listen(ListenKind::Command, false, 1);
        let from = Instant::now();
        line.play(&clip, &room(name, &clip), 25.0, |_| {});
        let ours = line.last.clone().unwrap_or_else(|| "(no end)".into());
        let ok = line.turn["end"] == "silence" && !ours.is_empty();
        println!("    {name:<32} {} | at {:.2} s of audio, {:.2} s of the clock | partials {:?} | final \"{ours}\" {}", line.ended(), line.now(), from.elapsed().as_secs_f32(), line.partials, check(ok));
    }

    // 3c. Waiting for the wake phrase: the room for a second, then the clip. The sink is the probe's
    // stand-in for the wake package: the engine with the phrase as keywords, then `wakephrase::find`
    // or, while that is a stub, "ok" or "okay" followed by the name.
    println!("  waiting for \"{}\": detection, the wake's sink, then the same buffer as a command:", wakephrase::phrase());
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let (sink_dir, sink_seen) = (dir.clone(), seen.clone());
    ears::on_segment(Some(Arc::new(move |segment: &Segment| {
        let options = SttOptions { language: Some("en".into()), keywords: wakephrase::keywords() };
        let text = stt::transcribe(&sink_dir, &segment.audio, &options).unwrap_or_default();
        let said: Vec<String> = text.split_whitespace().map(words).collect();
        let woke = wakephrase::find(&text).is_some() || said.windows(2).any(|w| (w[0] == "ok" || w[0] == "okay") && w[1] == words(notch_brand::NAME));
        sink_seen.lock().unwrap().push(format!(
            "{} of {:.2} s, {:.2} s voiced: \"{text}\"{}",
            if segment.window { "window" } else { "segment" },
            segment.audio.duration().as_secs_f32(),
            segment.voiced.as_secs_f32(),
            if woke { " → the wake phrase" } else { "" }
        ));
        woke
    })));
    let mut woke = std::collections::BTreeMap::<String, (u32, u32)>::new();
    for name in names.iter().filter(|n| n.starts_with("w_oknotch") || n.starts_with("wc_")) {
        let clip = read_wav(&audio.join(format!("{name}.wav")));
        let room = room(name, &clip);
        let mut line = Line::new(&dir);
        line.core.lock().unwrap().waiting(true);
        line.core.lock().unwrap().opened();
        line.core.lock().unwrap().rest();
        // The microphone has been open for a while: a second of the room before the clip.
        let mut source = vec![0.0; CAPTURE_RATE as usize];
        if !room.is_empty() {
            source = room.iter().cycle().take(CAPTURE_RATE as usize).copied().collect();
        }
        source.extend_from_slice(&clip);
        line.play(&source, &room, 9.0, |_| {});
        let heard = std::mem::take(&mut *seen.lock().unwrap());
        let yes = heard.iter().any(|h| h.ends_with("the wake phrase"));
        let set = format!("{} {}", name.split("__").next().unwrap_or(""), name.rsplit("__").next().unwrap_or(""));
        let tally = woke.entry(set).or_default();
        *tally = (tally.0 + yes as u32, tally.1 + 1);
        println!("    {name:<40} segments {:?}", line.segments);
        for h in &heard {
            println!("      {h}");
        }
        if yes {
            println!("      turn: {} | at {:.2} s | partials {:?} | final \"{}\" | spike, whole file with keywords: \"{}\"", line.ended(), line.now(), line.partials, line.last.clone().unwrap_or_else(|| "(no end)".into()), spike(&results, name, "kw"));
        }
        line.core.lock().unwrap().waiting(false);
    }
    ears::on_segment(None);
    if !woke.is_empty() {
        println!("  woke, by clip set and condition: {woke:?}");
    }

    // 3d. A tap and nobody speaks: steady room noise, no word, no engine run.
    let mut seed = 7u32;
    let noise: Vec<f32> = (0..CAPTURE_RATE as usize)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.01
        })
        .collect();
    let mut line = Line::new(&dir);
    line.core.lock().unwrap().opened();
    line.core.lock().unwrap().listen(ListenKind::Command, false, 1);
    line.play(&[], &noise, 25.0, |_| {});
    let ok = line.turn["end"] == "noSpeech" && line.turn["stt"] == 0 && line.last.as_deref() == Some("") && (line.now() - FIRST_WORD.as_secs_f32()).abs() < 0.05;
    println!("  a tap into room noise: {} | at {:.2} s | final {:?} {}", line.ended(), line.now(), line.last, check(ok));
}

/// `-- download`: the engine and the model from the addresses the product uses, into a folder of the
/// probe's own: streamed to a `.part` file, hashed, renamed.
fn download() {
    let dir = root().join("voice-dev/ears/whistle");
    let _ = std::fs::remove_dir_all(&dir);
    let from = Instant::now();
    let (mut pieces, mut bytes, mut seen_part) = (0u32, std::collections::BTreeMap::<String, (u64, u64)>::new(), false);
    let fetched = tauri::async_runtime::block_on(notch_feat_dictation::whistle::download_with(&dir, |name, received, total| {
        pieces += 1;
        seen_part |= received > 0 && dir.join(format!("{name}.part")).exists() && !dir.join(name).exists();
        bytes.insert(name.to_string(), (received, total));
    }));
    println!("── the download, to {} ──", dir.display());
    println!("  {fetched:?} in {:.1} s, {pieces} pieces; while it came it was a .part file and nothing else: {seen_part}", from.elapsed().as_secs_f32());
    for f in &notch_feat_dictation::whistle::FILES {
        let (received, total) = bytes.get(f.name).copied().unwrap_or_default();
        let on_disk = std::fs::metadata(dir.join(f.name)).map_or(0, |m| m.len());
        println!("  {}: {on_disk} bytes on disk ({received} received, the server said {total}), sha256 {}, .part left: {}", f.name, f.sha256, dir.join(format!("{}.part", f.name)).exists());
    }
    let left: Vec<String> = std::fs::read_dir(&dir).map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
    let ok = fetched.is_ok() && stt::ready(&dir) && left.len() == 2 && seen_part;
    println!("  the folder holds {left:?}; engine and model match their hashes: {} {}", stt::ready(&dir), check(ok));
    // A second call finds them right and fetches nothing.
    let mut again = 0;
    let _ = tauri::async_runtime::block_on(notch_feat_dictation::whistle::download_with(&dir, |_, _, _| again += 1));
    println!("  asked again: {again} pieces fetched {}", check(again == 0));
}

fn main() {
    // `notch_core::log` writes under %LOCALAPPDATA%: this run's lines stay out of the app's own log.
    let out = root().join("voice-dev/ears");
    let _ = std::fs::create_dir_all(&out);
    std::env::set_var("LOCALAPPDATA", &out);
    let filter = std::env::args().nth(1).unwrap_or_default();
    if filter == "download" {
        download();
        std::process::exit(FAILED.load(SeqCst).min(1) as i32);
    }
    keys();
    turns();
    clips(&filter);
    let failed = FAILED.load(SeqCst);
    println!("{}", if failed == 0 { "ok".to_string() } else { format!("{failed} FAILED") });
    std::process::exit(failed.min(1) as i32);
}
