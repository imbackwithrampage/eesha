// Proves crates/voice/src/needle.rs without the app, on the spike's material (the engine in
// target/dictation-probe/, the model, tools and sentences of target/voice-spike/):
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-needle-probe
//
// The tools reach the model as a live list through `needle::set_tools`: first the spike's own nine,
// then, sentence by sentence, the tools the page builds from a registry of 59 actions
// (target/voice-dev/voice-needle/tools.json, written by `npm test`: tests/voice-needle.test.mjs). What
// the model answered is written to target/voice-dev/voice-needle/answers.json, where the same test puts
// it through the page's rules (grounding, trigger, decision). Then: the timeout, the idle exit, the switch.

use notch_feat_voice::{needle, timing, VoiceError};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The spike's 23 (the voice inventory, §13.3) with the calls it expected.
const SENTENCES: [(&str, &str); 23] = [
    ("set a timer for 12 minutes", r#"[["start_timer",{"minutes":12}]]"#),
    ("start a 5 minute timer", r#"[["start_timer",{"minutes":5}]]"#),
    ("I have to leave in 25 minutes, can you time that for me", r#"[["start_timer",{"minutes":25}]]"#),
    ("pause the music", r#"[["control_media",{"action":"pause"}]]"#),
    ("skip this song", r#"[["control_media",{"action":"next"}]]"#),
    ("it's too quiet in here, put the volume at 70 percent", r#"[["set_volume",{"percent":70}]]"#),
    ("volume 30", r#"[["set_volume",{"percent":30}]]"#),
    ("open the clipboard", r#"[["open_section",{"name":"clipboard"}]]"#),
    ("show me my calendar", r#"[["open_section",{"name":"calendar"}]]"#),
    ("remind me to call the dentist", r#"[["add_reminder",{"text":"call the dentist"}]]"#),
    ("add a reminder to buy milk", r#"[["add_reminder",{"text":"buy milk"}]]"#),
    ("what are my agents doing right now", r#"[["get_agent_status",{}]]"#),
    ("is Claude done yet", r#"[["get_agent_status",{}]]"#),
    ("don't let the computer go to sleep for two hours", r#"[["keep_awake",{"minutes":120}]]"#),
    ("keep awake for 45 minutes", r#"[["keep_awake",{"minutes":45}]]"#),
    ("mute my microphones", r#"[["mute_microphones",{}]]"#),
    ("pause the music and set the volume to 20", r#"[["control_media",{"action":"pause"}],["set_volume",{"percent":20}]]"#),
    ("mets un minuteur de dix minutes", r#"[["start_timer",{"minutes":10}]]"#),
    ("mets la musique en pause", r#"[["control_media",{"action":"pause"}]]"#),
    ("what's the weather in Paris", "[]"),
    ("tell me a joke", "[]"),
    ("who won the football match yesterday", "[]"),
    ("send an email to Tana", "[]"),
];

/// Nothing may run on these: the spike's wrong calls (§13.3) and more small talk.
const SMALL_TALK: [&str; 14] = [
    "tell me a joke",
    "I love this song",
    "uh huh",
    "Thank you.",
    "Both the music and turn the volume down to 20%.",
    "I am going to bed",
    "good morning",
    "that was a long meeting",
    "how are you",
    "never mind",
    "okay",
    "I think it is going to rain",
    "the timer went off twenty minutes ago",
    "my microphone sounds strange",
];

fn fail(what: &str) -> ! {
    println!("FAIL {what}");
    needle::unload();
    std::process::exit(1)
}

/// What would run: nothing for a held-back call or `no_action`. `[[tool, args], …]`.
fn calls(c: &needle::Choice) -> Value {
    if c.suppressed || c.tool.is_empty() || c.tool == "no_action" {
        return json!([]);
    }
    let mut all = vec![json!([c.tool, c.args])];
    all.extend(c.more.iter().map(|m| json!([m.tool, m.args])));
    Value::Array(all)
}

/// What the model said, held back or not.
fn said(c: &needle::Choice) -> String {
    let held = if c.suppressed { " (held back by the engine)" } else { "" };
    let more: String = c.more.iter().map(|m| format!(" + {} {}", m.tool, m.args)).collect();
    if c.tool.is_empty() {
        "nothing".into()
    } else {
        format!("{} {}{more}{held}", c.tool, c.args)
    }
}

fn up(limit: Duration) -> Option<(u32, Duration)> {
    let from = Instant::now();
    while from.elapsed() < limit {
        if let Some(pid) = needle::running() {
            return Some((pid, from.elapsed()));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn alive(pid: u32) -> bool {
    notch_core::win::process_table().contains_key(&pid)
}

/// The 23 sentences against one list of tools; answers go to `out[set]`.
fn score(set: &str, tools: &Value, out: &mut serde_json::Map<String, Value>) {
    let count = needle::set_tools(tools).unwrap_or_else(|e| fail(&e.to_string()));
    needle::warm();
    let (pid, took) = up(timing::CHILD_START).unwrap_or_else(|| fail("the server did not start"));
    println!("\n[{set}] {count} tools as a live list; server {pid} up in {} ms", took.as_millis());
    let (mut right, mut ms, mut rows) = (0, Vec::new(), Vec::new());
    for (text, expected) in SENTENCES {
        let expected: Value = serde_json::from_str(expected).unwrap();
        let from = Instant::now();
        let choice = needle::choose(text, "").unwrap_or_else(|e| fail(&format!("{text}: {e}")));
        ms.push(from.elapsed().as_millis());
        let ok = calls(&choice) == expected;
        right += ok as usize;
        println!("  {} {:4} ms  {:.2}  {text} -> {}", if ok { "ok " } else { "BAD" }, ms.last().unwrap(), choice.confidence, said(&choice));
        rows.push(json!({ "text": text, "expected": expected, "choice": choice }));
    }
    ms.sort();
    println!("[{set}] the model alone: {right} of {} right; a request takes {} / {} / {} ms (min / median / max)", SENTENCES.len(), ms[0], ms[ms.len() / 2], ms[ms.len() - 1]);
    out.insert(set.into(), Value::Array(rows));
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let (engine, model) = (root.join("dictation-probe/needle.exe"), root.join("voice-spike/needle3.cact"));
    let work = root.join("voice-dev/voice-needle");
    let read = |path: PathBuf| -> Option<Value> { serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok() };
    let spike = read(root.join("voice-spike/tools_c.json")).unwrap_or_else(|| fail("target/voice-spike/tools_c.json is missing"));

    // 1. The files: the pinned ones pass, anything else is refused before it is run.
    needle::switch(true);
    needle::set_tools(&spike).unwrap();
    needle::locate(engine.clone(), root.join("voice-spike/tools_c.json"), work.clone());
    let refused = needle::choose("set a timer for 12 minutes", "");
    println!("a file that is not the model: ready {}, choose -> {:?}", needle::ready(), refused);
    if needle::ready() || !matches!(refused, Err(VoiceError::Missing(_))) {
        fail("a wrong model was not refused");
    }
    needle::locate(engine, model, work.clone());
    let from = Instant::now();
    if !needle::ready() {
        fail("engine or model is missing or does not match its hash");
    }
    println!("engine and model match their SHA-256 (checked in {} ms, again before each start)", from.elapsed().as_millis());

    // 2. The sentences, on the spike's own nine tools.
    let mut answers = serde_json::Map::new();
    score("spike", &spike, &mut answers);

    // 3. The same sentences and the small talk with the page's tools: a list per sentence, so a start
    // per sentence. The timeout is lifted here to see how long that takes.
    match read(work.join("tools.json")) {
        None => println!("\n[page] no target/voice-dev/voice-needle/tools.json: run `npm test` first"),
        Some(page) => {
            needle::tune(Duration::ZERO, Duration::from_secs(30));
            let (mut rows, mut right, mut cold, mut warm) = (Vec::new(), 0, Vec::new(), Vec::new());
            println!("\n[page] the tools the page builds from 59 actions, sentence by sentence");
            for row in page.as_array().into_iter().flatten() {
                let (text, expected) = (row["text"].as_str().unwrap_or(""), &row["expected"]);
                if row["tools"].is_null() {
                    println!("  --            {text} -> not asked: no action has a word of it");
                    right += (rows.len() < SENTENCES.len() && *expected == json!([])) as usize;
                    rows.push(json!({ "text": text, "expected": expected, "choice": null }));
                    continue;
                }
                let count = needle::set_tools(&row["tools"]).unwrap_or_else(|e| fail(&e.to_string()));
                let (before, from) = (needle::running(), Instant::now());
                let choice = needle::choose(text, "").unwrap_or_else(|e| fail(&format!("{text}: {e}")));
                let ms = from.elapsed().as_millis();
                let started = before != needle::running();
                if started && before.is_some_and(alive) {
                    fail("the tools changed and the old server is still there");
                }
                if started { &mut cold } else { &mut warm }.push(ms);
                let ok = rows.len() < SENTENCES.len() && calls(&choice) == *expected;
                right += ok as usize;
                let mark = if rows.len() >= SENTENCES.len() { "   " } else if ok { "ok " } else { "BAD" };
                println!("  {mark} {ms:5} ms {} {count:2} tools  {:.2}  {text} -> {}", if started { "start" } else { "     " }, choice.confidence, said(&choice));
                rows.push(json!({ "text": text, "expected": expected, "choice": choice, "ms": ms, "started": started }));
            }
            cold.sort();
            warm.sort();
            let mid = |v: &Vec<u128>| v.get(v.len() / 2).copied().unwrap_or(0);
            println!("[page] the model alone: {right} of {} right", SENTENCES.len());
            let most = (timing::NEEDLE_TIMEOUT + timing::NEEDLE_READY).as_millis();
            println!(
                "[page] a sentence whose tools differ from the last one's starts a server: {} of them, {} / {} / {} ms with the request (min / median / max), {} within the {most} ms such a call is given; {} with the same tools, median {} ms",
                cold.len(),
                cold.first().copied().unwrap_or(0),
                mid(&cold),
                cold.last().copied().unwrap_or(0),
                cold.iter().filter(|ms| **ms <= most).count(),
                warm.len(),
                mid(&warm)
            );
            if cold.len() < 2 {
                fail("a change of tools did not start a new server");
            }
            answers.insert("page".into(), Value::Array(rows));
            needle::tune(Duration::ZERO, Duration::ZERO);
        }
    }

    // 4. Small talk and the spike's wrong calls, on the spike's nine: what the model says by itself.
    needle::set_tools(&spike).unwrap();
    needle::warm();
    up(timing::CHILD_START).unwrap_or_else(|| fail("the server did not start"));
    let mut rows = Vec::new();
    println!("\n[small talk] what the model says by itself (the page's grounding decides: see `npm test`)");
    for text in SMALL_TALK {
        let choice = needle::choose(text, "").unwrap_or_else(|e| fail(&format!("{text}: {e}")));
        println!("  {:.2}  {text} -> {}", choice.confidence, said(&choice));
        rows.push(json!({ "text": text, "expected": [], "choice": choice }));
    }
    answers.insert("small talk".into(), Value::Array(rows));
    std::fs::write(work.join("answers.json"), serde_json::to_string_pretty(&answers).unwrap()).unwrap();
    println!("wrote target/voice-dev/voice-needle/answers.json");

    // 5. The timeout. A probe's 30 ms first (the engine needs 180 ms at least): the caller is let go on
    // time and the engine that did not answer is ended. Then the real row on a cold start.
    let pid = up(timing::CHILD_START).map(|(pid, _)| pid).unwrap_or_else(|| fail("no server"));
    needle::tune(Duration::ZERO, Duration::from_millis(30));
    let from = Instant::now();
    let timed = needle::choose("set a timer for 12 minutes", "");
    let took = from.elapsed();
    std::thread::sleep(Duration::from_millis(400));
    println!("\ntimeout 30 ms (probe): {:?} after {} ms; the server that did not answer is {}", timed, took.as_millis(), if alive(pid) { "STILL THERE" } else { "ended" });
    if timed != Err(VoiceError::Timeout("the action model")) || took > Duration::from_millis(80) || alive(pid) {
        fail("the timeout did not hold");
    }
    needle::tune(Duration::ZERO, Duration::ZERO);
    let from = Instant::now();
    let cold = needle::choose("set a timer for 12 minutes", "");
    let took = from.elapsed();
    let most = timing::NEEDLE_TIMEOUT + timing::NEEDLE_READY;
    println!("server cold, {} ms at most (the rows: timeout + ready): {} after {} ms", most.as_millis(), cold.as_ref().map(said).unwrap_or_else(|e| format!("{e:?}")), took.as_millis());
    if took > most + Duration::from_millis(80) || !matches!(cold, Ok(_) | Err(VoiceError::Timeout(_))) {
        fail("a cold start kept the caller past the timeout");
    }
    up(timing::CHILD_START).unwrap_or_else(|| fail("the cold start did not finish"));
    let from = Instant::now();
    let warm = needle::choose("set a timer for 12 minutes", "").unwrap_or_else(|e| fail(&format!("after the cold start: {e}")));
    println!("the next sentence: {} after {} ms", said(&warm), from.elapsed().as_millis());

    // 6. Idle. 3 s here instead of the row's 180 s; a `warm` counts as a use.
    needle::unload();
    needle::tune(Duration::from_secs(3), Duration::ZERO);
    needle::warm();
    let (pid, _) = up(timing::CHILD_START).unwrap_or_else(|| fail("no server"));
    needle::choose("pause the music", "").unwrap_or_else(|e| fail(&e.to_string()));
    let last = Instant::now();
    std::thread::sleep(Duration::from_secs(2));
    needle::warm();
    while alive(pid) && last.elapsed() < Duration::from_secs(12) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let gone = last.elapsed();
    println!("\nidle 3 s (probe; the row is {} s): a request at 0 s, a warm at 2.0 s, server {pid} gone at {:.2} s; running: {:?}", timing::NEEDLE_IDLE.as_secs(), gone.as_secs_f32(), needle::running());
    if alive(pid) || needle::running().is_some() || !(4.9..5.6).contains(&gone.as_secs_f32()) {
        fail("the server did not leave 3 s after its last use");
    }
    needle::tune(Duration::ZERO, Duration::ZERO);

    // 7. The switch: off, nothing starts.
    needle::switch(false);
    needle::warm();
    std::thread::sleep(Duration::from_millis(300));
    let off = needle::choose("pause the music", "");
    println!("switched off: warm starts nothing (running: {:?}), choose -> {:?}", needle::running(), off);
    if needle::running().is_some() || !matches!(off, Err(VoiceError::Refused(_))) {
        fail("the switch did not hold");
    }
    println!("\nPASS");
}
