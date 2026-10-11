// Proves the service against the real system, without the app:
//
//   cargo run -p svc-media --example media-probe
//
// Prints what plays now, then every change for about 20 seconds. It only
// listens: nothing is sent to any player.
//
//   cargo run -p svc-media --example media-probe -- --demo
//
// For a machine where nothing plays: the probe first registers a silent
// session of its own with Windows (no sound, no window), then reads it back
// through the service and sends it every command. Commands only ever go to
// that session, never to a player of the user's.
//
//   cargo run -p svc-media --example media-probe -- --demo --hold 300 --track "Title" "Artist" 240
//
// For the app's own checks: the silent session stays for 300 seconds and is
// left alone (the app drives it), described as the given track of 240 seconds.
//
//   cargo run -p svc-media --example media-probe -- --demo --flip
//
// A change of song as a browser with two tabs shows it: the silent session
// plays a song, says for a second that it is another, paused, one (the paused
// tab), then plays the next song. The service must never report the paused one.
// Then the paused one stays, and must be reported once the wait is over.
//
//   cargo run -p svc-media --example media-probe -- --demo --players
//
// Which player is followed and who gets the commands, with two silent sessions:
// "one" in this process and "two" in a child of it, a poorer player without a
// timeline and without "previous". One is preferred while two speaks last; a
// track changes; a paused player starts again without reporting its position;
// seeks land outside the track; two is preferred, then closed; and two stands
// in for a paused one. Each session says what it was sent, so every line of the
// result is PASS or FAIL. Commands go out only while a session of the probe's
// own is the preferred player and nobody may stand in for it: a player of the
// user's is never sent anything, whatever plays on this computer meanwhile.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use svc_media::Playback;
use windows::core::{w, Result, HSTRING};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::Playback::MediaPlayer;
use windows::Media::{
    MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs, SystemMediaTransportControls,
    SystemMediaTransportControlsButton as Button, SystemMediaTransportControlsButtonPressedEventArgs,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

fn line(p: Option<&Playback>) -> String {
    match p {
        None => "nothing is playing".into(),
        Some(p) => format!(
            "source={:?} ({}){} title={:?} artist={:?} album={:?} playing={} position={:.1}s duration={:.1}s art={} \
             can[play_pause={} next={} prev={} seek={} shuffle={} repeat={}] shuffle={} repeat={:?}",
            p.source_name,
            p.source_aumid,
            if p.fallback { " standing in" } else { "" },
            p.title,
            p.artist,
            p.album,
            p.playing,
            p.position_now(),
            p.duration,
            match &p.art {
                Some(bytes) => format!("read ({} bytes, {}, rev {})", bytes.len(), p.art_mime(), p.art_rev),
                None => "none".into(),
            },
            p.can_play_pause,
            p.can_next,
            p.can_prev,
            p.can_seek,
            p.can_shuffle,
            p.can_repeat,
            p.shuffle,
            p.repeat,
        ),
    }
}

/// Prints every change that arrives within `secs`; returns how many.
fn watch(rx: &Receiver<Option<Playback>>, secs: f32, started: Instant) -> usize {
    let end = Instant::now() + Duration::from_secs_f32(secs);
    let mut changes = 0;
    while let Ok(p) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
        changes += 1;
        println!("[{:>5.1}s] {}", started.elapsed().as_secs_f32(), line(p.as_ref()));
    }
    changes
}

/// `--track <title> <artist> <seconds>`: what the demo session says it plays.
static TRACK: std::sync::OnceLock<(String, String, i64)> = std::sync::OnceLock::new();

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--child") {
        return second_session();
    }
    let after = |flag: &str, n: usize| args.iter().position(|a| a == flag).and_then(|i| args.get(i + n)).cloned();
    if let (Some(title), Some(artist), Some(secs)) = (after("--track", 1), after("--track", 2), after("--track", 3)) {
        let _ = TRACK.set((title, artist, secs.parse().unwrap_or(180)));
    }
    let demo = args.iter().any(|a| a == "--demo");
    let _player = demo.then(|| demo_player(true).expect("the demo session"));
    if let Some(secs) = after("--hold", 1).and_then(|s| s.parse::<u64>().ok()).filter(|_| demo) {
        println!("holding the demo session for {secs} s");
        std::thread::sleep(Duration::from_secs(secs));
        return;
    }

    let (tx, rx) = mpsc::channel();
    let _sub = svc_media::subscribe(move |p| {
        let _ = tx.send(p.cloned());
    });

    // The first reading arrives as a change, unless there is nothing to report.
    let first = rx.recv_timeout(Duration::from_secs(3)).ok().flatten().or_else(svc_media::state);
    println!("now: {}", line(first.as_ref()));
    for s in svc_media::sources() {
        println!("  player: {:?} ({}) playing={}", s.name, s.aumid, s.playing);
    }

    let started = Instant::now();
    if demo && args.iter().any(|a| a == "--flip") {
        flip(_player.as_ref().expect("the demo session"), &rx, started);
        return;
    }
    if demo && args.iter().any(|a| a == "--players") {
        players(_player.as_ref().expect("the demo session"), &rx, started);
        return;
    }
    if !demo {
        println!("watching for 20 s…");
        let changes = watch(&rx, 20.0, started);
        println!("done: {changes} change(s) in 20 s");
        return;
    }

    let Some(own) = svc_media::sources().into_iter().find(|s| s.aumid.to_lowercase().contains("probe")) else {
        println!("the demo session did not show up among the players");
        return;
    };
    let steps: [(&str, fn()); 5] = [
        ("play_pause (expect playing=false)", svc_media::play_pause),
        ("play_pause (expect playing=true)", svc_media::play_pause),
        ("seek to 42 s (expect position≈42)", || svc_media::seek(42.0)),
        ("next (expect \"Probe song two\")", svc_media::next),
        ("activate_player (no window: nothing to see)", svc_media::activate_player),
    ];
    println!("→ pick_source({})", own.aumid);
    svc_media::pick_source(Some(own.aumid.clone()));
    watch(&rx, 1.0, started);
    for (what, act) in steps {
        if svc_media::state().map(|p| p.source_aumid) != Some(own.aumid.clone()) {
            println!("another player is followed: not sending it anything");
            return;
        }
        println!("→ {what}");
        act();
        watch(&rx, 1.5, started);
    }
    println!("done");
}

/// `--flip`: see the top of the file. Only the probe's own session is touched.
fn flip(player: &MediaPlayer, rx: &Receiver<Option<Playback>>, started: Instant) {
    const PAUSED_TAB: &str = "The paused tab";
    let smtc = player.SystemMediaTransportControls().expect("the controls of the demo session");
    let own = |p: &Option<Playback>| p.as_ref().is_some_and(|p| p.source_aumid.to_lowercase().contains("probe"));
    if !own(&svc_media::state()) {
        println!("another player is followed: the change of song cannot be shown with it playing");
        return;
    }
    let say = |title: &str, status: MediaPlaybackStatus| {
        describe(&smtc, title).and_then(|()| smtc.SetPlaybackStatus(status)).expect("the demo session");
    };
    // What the service reports from now on, with when.
    let seen = |secs: f32| {
        let end = Instant::now() + Duration::from_secs_f32(secs);
        let mut titles = Vec::new();
        while let Ok(p) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            println!("[{:>5.1}s] {}", started.elapsed().as_secs_f32(), line(p.as_ref()));
            titles.push(p.map(|p| p.title).unwrap_or_default());
        }
        titles
    };
    seen(0.6);
    println!("→ the session says \"{PAUSED_TAB}\", paused, for 0.9 s, then plays \"{SECOND_SONG}\"");
    say(PAUSED_TAB, MediaPlaybackStatus::Paused);
    let mut titles = seen(0.9);
    say(SECOND_SONG, MediaPlaybackStatus::Playing);
    titles.extend(seen(2.0));
    let flicker = titles.iter().any(|t| t == PAUSED_TAB);
    let arrived = titles.last().is_some_and(|t| t == SECOND_SONG);
    println!("{}: reported {titles:?}", if !flicker && arrived { "PASS (no paused tab between the two songs)" } else { "FAIL" });

    println!("→ the session says \"{PAUSED_TAB}\", paused, and stays so");
    say(PAUSED_TAB, MediaPlaybackStatus::Paused);
    let at = Instant::now();
    let titles = seen(4.0);
    let told = titles.iter().any(|t| t == PAUSED_TAB);
    println!("{}: reported {titles:?} within {:.1} s", if told { "PASS (a real stop is told once the wait is over)" } else { "FAIL" }, at.elapsed().as_secs_f32());
}

// ── --players: two sessions, a preferred one ──────────────────────────────────

/// The id the child gives itself, so that Windows lists its session apart from this process's.
const SECOND_ID: &str = "media-probe.two";

/// The second demo session, in a child of this process, for as long as this lives.
struct Second(Child);

impl Second {
    fn start() -> Second {
        let mut child = Command::new(std::env::current_exe().expect("the probe's own path"))
            .arg("--child")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the second demo session");
        let mut said = BufReader::new(child.stdout.take().expect("the child's output")).lines();
        // Its first line says the session is up; the rest, what the session was sent.
        let _ = said.next();
        std::thread::spawn(move || {
            for line in said.map_while(|l| l.ok()) {
                if let Some(what) = line.strip_prefix("sent ") {
                    SENT.lock().unwrap().push(("two", what.to_string()));
                }
            }
        });
        Second(child)
    }

    /// `title <text>`, `play` or `pause`: said by the session itself, not sent to it as a command.
    fn tell(&mut self, line: &str) {
        let _ = writeln!(self.0.stdin.as_mut().expect("the child's input"), "{line}");
    }
}

impl Drop for Second {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `--child`: the second session. It lives while the parent holds the other end
/// of its input, so none is left behind if the parent dies.
fn second_session() {
    unsafe { SetCurrentProcessExplicitAppUserModelID(w!("media-probe.two")).expect("an id for the second session") };
    let _ = WHO.set("two");
    let player = demo_player(false).expect("the second demo session");
    let smtc = player.SystemMediaTransportControls().expect("the controls of the second demo session");
    println!("ready");
    for line in std::io::stdin().lines().map_while(|l| l.ok()) {
        let done = match line.split_once(' ').unwrap_or((line.as_str(), "")) {
            ("title", text) => describe(&smtc, text),
            ("play", _) => smtc.SetPlaybackStatus(MediaPlaybackStatus::Playing),
            ("pause", _) => smtc.SetPlaybackStatus(MediaPlaybackStatus::Paused),
            _ => return,
        };
        done.expect("the second demo session");
    }
}

/// What each demo session was sent since the last look.
fn sent_since() -> (Vec<String>, Vec<String>) {
    let all: Vec<_> = SENT.lock().unwrap().drain(..).collect();
    let to = |who: &str| all.iter().filter(|(w, _)| *w == who).map(|(_, what)| what.clone()).collect::<Vec<_>>();
    let (one, two) = (to("one"), to("two"));
    println!("      session one was sent {one:?}, session two {two:?}");
    (one, two)
}

/// `--players`: see the top of the file.
fn players(player: &MediaPlayer, rx: &Receiver<Option<Playback>>, started: Instant) {
    let smtc = player.SystemMediaTransportControls().expect("the controls of the demo session");
    let mut second = Second::start();
    watch(rx, 1.5, started);
    let sources = svc_media::sources();
    for s in &sources {
        println!("  player: {:?} ({}) playing={}", s.name, s.aumid, s.playing);
    }
    let find = |end: &str| sources.iter().find(|s| s.aumid.to_lowercase().ends_with(end)).map(|s| s.aumid.clone());
    let (Some(one), Some(two)) = (find("media-probe.exe"), find(SECOND_ID)) else {
        println!("the two demo sessions did not both show up among the players");
        return;
    };
    let shown = svc_media::state;
    let on = |aumid: &str| shown().is_some_and(|p| p.source_aumid == aumid);
    let (mut passed, mut failed) = (0, 0);
    let mut check = |ok: bool, what: &str| {
        *(if ok { &mut passed } else { &mut failed }) += 1;
        println!("  {}: {what}", if ok { "PASS" } else { "FAIL" });
    };
    let only = |sent: &[String], what: &str| sent.len() == 1 && sent[0] == what;
    let length = TRACK.get().map_or(180, |t| t.2) as f64;

    // Every command below is sent while one of the two sessions is the preferred
    // player and nobody may stand in for it: the service then sends to that
    // session or to nobody.
    println!("→ both play; session two speaks last; session one is preferred");
    second.tell("title Probe two, speaking last");
    svc_media::pick_source(Some(one.clone()));
    watch(rx, 1.2, started);
    sent_since();
    check(on(&one), "the preferred player is the one followed, whoever spoke last");

    println!("→ next");
    let before = shown().map_or(0, |p| p.art_rev);
    svc_media::next();
    watch(rx, 1.5, started);
    let (a, b) = sent_since();
    check(only(&a, "Next") && b.is_empty(), "next went to the preferred player and to nobody else");
    let rev = shown().map_or(0, |p| p.art_rev);
    check(shown().is_some_and(|p| p.title == SECOND_SONG) && rev != before && rev != 0, "the track changed, and its cover with it");
    check(
        svc_media::art(before).is_none() && svc_media::art(rev).is_some(),
        "the cover of the song before is no longer handed out; the new one's is",
    );

    println!("→ previous");
    svc_media::prev();
    watch(rx, 1.5, started);
    let (a, b) = sent_since();
    check(only(&a, "Previous") && b.is_empty() && shown().is_some_and(|p| p.title == FIRST), "previous went to the preferred player");

    println!("→ pause, twice");
    svc_media::pause();
    watch(rx, 0.8, started);
    svc_media::pause();
    watch(rx, 0.8, started);
    let (a, b) = sent_since();
    check(
        shown().is_some_and(|p| !p.playing) && !a.iter().any(|what| what == "Play") && b.is_empty(),
        "paused, and the second pause did not start it again",
    );
    let stood = shown().map_or(0.0, |p| p.position_now());
    println!("      paused at {stood:.1} s; 3 s pass");
    watch(rx, 3.0, started);

    println!("→ play (the session says that it plays and nothing of where it is: a player that reports late)");
    svc_media::play();
    watch(rx, 1.5, started);
    let (a, b) = sent_since();
    let at = shown().map_or(0.0, |p| p.position_now());
    check(only(&a, "Play") && b.is_empty() && shown().is_some_and(|p| p.playing), "play went to the preferred player");
    check(
        (at - stood - 1.5).abs() < 0.8,
        &format!("position from timestamps: {at:.1} s, 1.5 s after starting again at {stood:.1} s (the 3 s of the pause are not counted)"),
    );

    // The same with a player that says nothing of its position at either end. Said
    // by the session itself: this is no command, and no report of where it is.
    let say = |playing: bool| {
        let status = if playing { MediaPlaybackStatus::Playing } else { MediaPlaybackStatus::Paused };
        set_position(&smtc, position(), playing, false).and_then(|()| smtc.SetPlaybackStatus(status)).expect("the demo session");
    };
    println!("→ the session pauses, and plays again 3 s later, without a word about where it is");
    let before = shown().map_or(0.0, |p| p.position_now());
    say(false);
    watch(rx, 3.0, started);
    let held = shown().map_or(0.0, |p| p.position_now());
    say(true);
    watch(rx, 1.0, started);
    let at = shown().map_or(0.0, |p| p.position_now());
    check(
        (held - before).abs() < 0.6 && (at - held - 1.0).abs() < 0.6,
        &format!("it stood at {held:.1} s through the pause (it was at {before:.1} s) and is at {at:.1} s a second after: neither back to its last report nor ahead by the pause"),
    );

    for (to, lands) in [(1e9, Some(length)), (-5.0, Some(0.0)), (f64::NAN, None), (42.0, Some(42.0))] {
        println!("→ seek to {to} s");
        svc_media::seek(to);
        watch(rx, 1.0, started);
        let (a, b) = sent_since();
        let ok = match lands {
            Some(secs) => only(&a, &format!("Seek {secs:.1}")),
            None => a.is_empty(),
        };
        let what = match lands {
            Some(secs) => format!("the seek reached the player as {secs:.1} s, inside the track of {length:.0} s"),
            None => "a seek to no number is not sent".into(),
        };
        check(ok && b.is_empty(), &what);
    }
    check(shown().is_some_and(|p| (p.position_now() - 42.5).abs() < 1.5), "and the position reported is where the last seek landed");

    println!("→ session two is preferred: it publishes no timeline");
    svc_media::pick_source(Some(two.clone()));
    watch(rx, 1.2, started);
    sent_since();
    check(
        shown().is_some_and(|p| p.source_aumid == two && p.duration == 0.0 && !p.can_seek && !p.can_prev),
        "a track without a duration: 0 s, no seeking (so no slider), and no previous",
    );
    println!("→ seek to 30 s");
    svc_media::seek(30.0);
    watch(rx, 0.8, started);
    let (a, b) = sent_since();
    check(a.is_empty() && b.is_empty(), "a seek in a track without an end is not sent");
    println!("→ next");
    svc_media::next();
    watch(rx, 1.5, started);
    let (a, b) = sent_since();
    check(a.is_empty() && only(&b, "Next"), "next followed the preference to session two");

    println!("→ session one is preferred again, and paused; session two plays");
    svc_media::pick_source(Some(one.clone()));
    watch(rx, 0.8, started);
    svc_media::pause();
    watch(rx, 1.0, started);
    sent_since();
    check(shown().is_some_and(|p| p.source_aumid == one && !p.playing && !p.fallback), "without the fallback the preferred player stays, paused");

    println!("→ the fallback is turned on (until it is off again the probe sends no command at all)");
    svc_media::set_fallback(true);
    watch(rx, 1.0, started);
    match shown() {
        Some(p) if p.source_aumid == two => check(
            p.fallback && p.playing && p.can_next && !p.can_prev && !p.can_seek,
            "session two stands in, marked as such, with the controls it has (next) and without those it lacks (previous, seeking)",
        ),
        Some(p) if p.fallback => println!("      {} plays on this computer and stands in; nothing is sent to it", p.source_name),
        _ => check(false, "a player that plays stands in for the paused one"),
    }
    println!("→ session one plays again (it says so itself)");
    say(true);
    watch(rx, 1.0, started);
    check(shown().is_some_and(|p| p.source_aumid == one && !p.fallback), "the preferred player is back as soon as it plays");
    svc_media::set_fallback(false);
    watch(rx, 0.5, started);

    println!("→ session two is preferred, and closes");
    svc_media::pick_source(Some(two.clone()));
    watch(rx, 1.0, started);
    drop(second);
    watch(rx, 3.0, started);
    sent_since();
    check(
        shown().is_none() && svc_media::sources().iter().any(|s| s.aumid == one),
        "the preferred player is closed: nothing is reported, though another player runs",
    );
    println!("→ next");
    svc_media::next();
    watch(rx, 1.0, started);
    let (a, _) = sent_since();
    check(a.is_empty(), "with the preferred player closed a command goes to nobody");

    println!("done: {passed} passed, {failed} failed");
}

// ── The demo session: a player that plays nothing ─────────────────────────────

const SECOND: i64 = 10_000_000;
const FIRST: &str = "Probe song one";
const SECOND_SONG: &str = "Probe song two";

/// Which demo session this process holds: "one", or "two" in the child.
static WHO: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
/// What the demo sessions were sent, oldest first: (session, what).
static SENT: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());
/// Whether this process's session publishes a timeline.
static TIMELINE: AtomicBool = AtomicBool::new(true);
/// The session's own clock: ticks into the track, and since when they run on (not while paused).
static CLOCK: Mutex<(i64, Option<Instant>)> = Mutex::new((0, None));

/// The session was sent something: the child says so on its output, where the parent reads it.
fn sent(what: String) {
    match WHO.get() {
        Some(_) => println!("sent {what}"),
        None => SENT.lock().unwrap().push(("one", what)),
    }
}

fn position() -> i64 {
    let (at, since) = *CLOCK.lock().unwrap();
    at + since.map_or(0, |s| s.elapsed().as_nanos() as i64 / 100)
}

fn running() -> bool {
    CLOCK.lock().unwrap().1.is_some()
}

/// The session's clock from here on; `report` also tells Windows, as a player does now and then.
fn set_position(smtc: &SystemMediaTransportControls, ticks: i64, running: bool, report: bool) -> Result<()> {
    *CLOCK.lock().unwrap() = (ticks, running.then(Instant::now));
    if report && TIMELINE.load(Ordering::Relaxed) {
        timeline(smtc, ticks)?;
    }
    Ok(())
}

/// Two 1×1 PNGs, for covers: the first song's and the second's.
const PIXEL: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49,
    0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f, 0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba,
    0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
const PIXEL_TWO: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xba, 0x54, 0x05, 0x00, 0x04, 0x51, 0x01, 0xbf, 0xcc, 0xb1, 0x76, 0x93,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn describe(smtc: &SystemMediaTransportControls, title: &str) -> Result<()> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::new()?;
    writer.WriteBytes(if title == SECOND_SONG { &PIXEL_TWO } else { &PIXEL })?;
    stream.WriteAsync(&writer.DetachBuffer()?)?.get()?;
    stream.Seek(0)?;

    let display = smtc.DisplayUpdater()?;
    display.SetType(MediaPlaybackType::Music)?;
    let music = display.MusicProperties()?;
    // The first song is the one asked for with --track; "next" still moves on to the probe's second.
    let (title, artist) = TRACK.get().filter(|_| title == FIRST).map_or((title, "svc-media probe"), |t| (t.0.as_str(), t.1.as_str()));
    music.SetTitle(&HSTRING::from(title))?;
    music.SetArtist(&HSTRING::from(artist))?;
    music.SetAlbumTitle(&HSTRING::from("Demo session"))?;
    display.SetThumbnail(&RandomAccessStreamReference::CreateFromStream(&stream)?)?;
    display.Update()
}

fn timeline(smtc: &SystemMediaTransportControls, position: i64) -> Result<()> {
    let t = SystemMediaTransportControlsTimelineProperties::new()?;
    t.SetStartTime(TimeSpan { Duration: 0 })?;
    t.SetMinSeekTime(TimeSpan { Duration: 0 })?;
    let length = TRACK.get().map_or(180, |t| t.2) * SECOND;
    t.SetEndTime(TimeSpan { Duration: length })?;
    t.SetMaxSeekTime(TimeSpan { Duration: length })?;
    t.SetPosition(TimeSpan { Duration: position })?;
    smtc.UpdateTimelineProperties(&t)
}

/// Keep the returned player alive: the session lasts as long as it does.
/// `full`: with a timeline and "previous"; without, the poorer player of `--players`.
fn demo_player(full: bool) -> Result<MediaPlayer> {
    let player = MediaPlayer::new()?;
    player.CommandManager()?.SetIsEnabled(false)?;
    let smtc = player.SystemMediaTransportControls()?;
    smtc.SetIsEnabled(true)?;
    smtc.SetIsPlayEnabled(true)?;
    smtc.SetIsPauseEnabled(true)?;
    smtc.SetIsNextEnabled(true)?;
    smtc.SetIsPreviousEnabled(full)?;
    TIMELINE.store(full, Ordering::Relaxed);
    describe(&smtc, FIRST)?;
    set_position(&smtc, 30 * SECOND, true, true)?;
    smtc.SetPlaybackStatus(MediaPlaybackStatus::Playing)?;
    smtc.ButtonPressed(&TypedEventHandler::<SystemMediaTransportControls, SystemMediaTransportControlsButtonPressedEventArgs>::new(
        |smtc, args| {
            let (Some(smtc), Some(args)) = (smtc.as_ref(), args.as_ref()) else { return Ok(()) };
            let button = args.Button()?;
            sent(
                match button {
                    Button::Play => "Play",
                    Button::Pause => "Pause",
                    Button::Next => "Next",
                    Button::Previous => "Previous",
                    _ => "another button",
                }
                .into(),
            );
            match button {
                // Starting again it says that it plays, and where it is only later: as players do.
                Button::Play => {
                    set_position(smtc, position(), true, false)?;
                    smtc.SetPlaybackStatus(MediaPlaybackStatus::Playing)
                }
                Button::Pause => {
                    set_position(smtc, position(), false, true)?;
                    smtc.SetPlaybackStatus(MediaPlaybackStatus::Paused)
                }
                Button::Next | Button::Previous => {
                    describe(smtc, if button == Button::Next { SECOND_SONG } else { FIRST })?;
                    set_position(smtc, 0, running(), true)
                }
                _ => Ok(()),
            }
        },
    ))?;
    smtc.PlaybackPositionChangeRequested(
        &TypedEventHandler::<SystemMediaTransportControls, PlaybackPositionChangeRequestedEventArgs>::new(|smtc, args| {
            let (Some(smtc), Some(args)) = (smtc.as_ref(), args.as_ref()) else { return Ok(()) };
            let to = args.RequestedPlaybackPosition()?.Duration;
            sent(format!("Seek {:.1}", to as f64 / SECOND as f64));
            set_position(smtc, to, running(), true)
        }),
    )?;
    Ok(player)
}
