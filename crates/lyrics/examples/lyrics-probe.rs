// The lookup, end to end, without the app: `cargo run -p notch-feat-lyrics --example lyrics-probe`.
// Eight songs are asked of lrclib.net (one request each, a second one where the answer is full);
// everything about a busy service is played by a stand-in on this machine. Nothing is played.
// A song's text is other people's work: of a text that was found only facts are printed (how
// many lines, from when to when, how long, a hash), never a line of it. The fixtures are made up.

use notch_feat_lyrics::{clean, get, line_at, lookup, parse_lrc, Lyric, Track, Tracker, BASE, SOURCE};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn track(title: &str, artist: &str, album: &str, duration: f64) -> Track {
    Track { title: title.into(), artist: artist.into(), album: album.into(), duration }
}

fn block<T>(f: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(f)
}

/// A stand-in for the service on a free port of this machine: the nth request gets the nth
/// answer (status line, extra header lines, body). Returns its address.
fn stand_in(answers: Vec<(&'static str, &'static str, String)>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for (status, headers, body) in answers {
            let Ok((mut conn, _)) = listener.accept() else { return };
            let _ = conn.read(&mut [0u8; 8192]);
            let head = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = conn.write_all(head.as_bytes());
            let _ = conn.write_all(body.as_bytes());
        }
    });
    base
}

/// One version as the service lists it.
fn version(t: &Track, synced: &str) -> String {
    serde_json::json!([{ "trackName": t.title, "artistName": t.artist, "albumName": t.album, "duration": t.duration,
                         "plainLyrics": "plain", "syncedLyrics": synced }])
    .to_string()
}

/// A text in facts: nothing of what it says.
fn facts(l: &Lyric) -> String {
    let stamp = |ms: u64| format!("{}:{:05.2}", ms / 60_000, (ms % 60_000) as f64 / 1000.0);
    let mut hash = DefaultHasher::new();
    l.lines.iter().for_each(|line| line.text.hash(&mut hash));
    format!(
        "{}, {} lines, {} to {}, {} characters, hash {:08x}, matched on {}",
        if l.synced { "synced" } else { "plain" },
        l.lines.len(),
        stamp(l.lines.first().map_or(0, |l| l.ms)),
        stamp(l.lines.last().map_or(0, |l| l.ms)),
        l.lines.iter().map(|l| l.text.chars().count()).sum::<usize>(),
        hash.finish() as u32,
        l.matched
    )
}

fn said(result: &Result<Option<Lyric>, String>) -> String {
    match result {
        Ok(Some(l)) => format!("found at {SOURCE}: {}", facts(l)),
        Ok(None) => "the service has no such song".into(),
        Err(e) => format!("no answer ({e})"),
    }
}

fn main() {
    println!("== LRC ==");
    let sample = "[ar:Someone]\n[ti:Song]\n[00:12.50]line one\n[00:17.00]line two\n[00:20.00][01:05.25]line three\n[01:30.00]\n";
    let lines = parse_lrc(sample);
    println!("parsed {} lines", lines.len());
    for ms in [0, 12_499, 12_500, 18_000, 21_000, 65_300, 95_000] {
        println!("{ms:>6} ms -> {:?}", line_at(&lines, ms));
    }

    println!("\n== Titles without their notes ==");
    for title in [
        "Bohemian Rhapsody - Remastered 2011",
        "Levitating (feat. DaBaby)",
        "Hotel California - 2013 Remaster",
        "Song [Deluxe Edition] (Live at Wembley)",
        "One More Time feat. Romanthony",
        "Sorry (Latino Remix)",
        "Stay With Me",
    ] {
        println!("{title:?} -> {:?}", clean(title));
    }

    // Every real request goes through here: counted, and the same question is not asked twice in a
    // run (a remaster-style title asks what its plain title asked: one request in the app, none here).
    let memo: RefCell<HashMap<String, Vec<u8>>> = RefCell::default();
    let sent = Cell::new(0u32);
    let ask = |query: String| {
        let (memo, sent) = (&memo, &sent);
        async move {
            let known = memo.borrow().get(&query).cloned();
            if let Some(body) = known {
                return Ok(body);
            }
            sent.set(sent.get() + 1);
            std::thread::sleep(Duration::from_millis(700));
            let body = get(&format!("{BASE}/api/search?{query}"), &|| true).await?;
            memo.borrow_mut().insert(query, body.clone());
            Ok(body)
        }
    };

    let mut tracker = Tracker::default();
    tracker.set_fetch(true);
    // One report through the tracker and, when it hands out a ticket, the lookup and its answer.
    let play = |tracker: &mut Tracker, label: &str, t: Track| {
        let before = sent.get();
        match tracker.report(Some(t)) {
            None => println!("  {label:<17} no lookup; status {:?}", tracker.status()),
            Some(ticket) => {
                let result = block(lookup(&ticket.track, &ask));
                let text = said(&result);
                tracker.answer(&ticket, result);
                println!("  {label:<17} {} new request(s); status {:?}; {text}", sent.get() - before, tracker.status());
            }
        }
    };

    println!("\n== Eight songs: an incomplete report, the full one, a remaster-style title, a wrong length ==");
    let songs = [
        ("Bohemian Rhapsody", "Queen", "A Night at the Opera", 354.0, " - Remastered 2011"),
        ("Smells Like Teen Spirit", "Nirvana", "Nevermind", 301.0, " (Remastered 2011)"),
        ("Billie Jean", "Michael Jackson", "Thriller", 294.0, " - 2008 Remaster"),
        ("Rolling in the Deep", "Adele", "21", 228.0, " [Deluxe Edition]"),
        ("Viva la Vida", "Coldplay", "Viva la Vida or Death and All His Friends", 242.0, " - Single Version"),
        ("Blinding Lights", "The Weeknd", "After Hours", 200.0, " (feat. Someone Else)"),
        ("Take On Me", "a-ha", "Hunting High and Low", 225.0, " (2015 Remastered Version)"),
        ("Hotel California", "Eagles", "Hotel California", 391.0, " - 2013 Remaster"),
    ];
    for (i, (title, artist, album, duration, note)) in songs.into_iter().enumerate() {
        println!("{artist} - {title} ({duration} s)");
        // What a player says first: the title alone, the title before its length, or no artist yet.
        let partial = match i % 3 {
            0 => track(title, "", "", 0.0),
            1 => track(title, artist, album, 0.0),
            _ => track(title, "", album, duration),
        };
        play(&mut tracker, "incomplete report", partial);
        play(&mut tracker, "full report", track(title, artist, album, duration));
        play(&mut tracker, "remaster title", track(&format!("{title}{note}"), artist, album, duration));
        play(&mut tracker, "wrong length", track(title, artist, album, duration + 1000.0));
        play(&mut tracker, "full report again", track(title, artist, album, duration));
    }
    println!("requests to lrclib.net in this run: {}", sent.get());

    println!("\n== The cache ==");
    let before = sent.get();
    let (title, artist, album, duration, _) = songs[0];
    play(&mut tracker, "first song again", track(title, artist, album, duration));
    println!("  requests made for it: {}", sent.get() - before);

    println!("\n== A busy service (a stand-in on this machine) ==");
    let song = track("Probe Song", "Probe Artist", "Probe Album", 180.0);
    let good = version(&song, "[00:01.00]line one\n[00:05.00]line two");
    let through = |base: String| {
        let started = Instant::now();
        let result = block(lookup(&song, |query| {
            let url = format!("{base}/api/search?{query}");
            async move { get(&url, &|| true).await }
        }));
        (result, started.elapsed().as_secs_f32())
    };

    let (result, took) = through(stand_in(vec![("429 Too Many Requests", "Retry-After: 2\r\n", String::new()), ("200 OK", "", good.clone())]));
    println!("  429 with Retry-After: 2, then 200 -> {} after {took:.1} s", said(&result));

    let (result, took) = through(stand_in(vec![("429 Too Many Requests", "Retry-After: 600\r\n", String::new())]));
    println!("  429 with Retry-After: 600        -> {} after {took:.1} s", said(&result));

    let busy = || ("503 Service Unavailable", "", String::new());
    let (result, took) = through(stand_in(vec![busy(), busy(), busy()]));
    println!("  503 three times                  -> {} after {took:.1} s", said(&result));
    let ticket = tracker.report(Some(song.clone())).expect("a new song is looked up");
    tracker.answer(&ticket, result);
    println!("  the tracker after that failure: status {:?}", tracker.status());
    tracker.report(None);
    let again = tracker.report(Some(song.clone()));
    println!("  the same song played again: {}", if again.is_some() { "looked up again (the failure was not kept)" } else { "NOT looked up: the failure was kept" });
    println!("  asked again by hand while it shows the error: {}", {
        tracker.answer(&again.unwrap(), Err("still busy".into()));
        if tracker.retry().is_some() { "a new lookup" } else { "nothing" }
    });

    println!("\n== An answer that comes too late ==");
    let (first, second) = (track("Late Song", "Probe Artist", "Probe Album", 200.0), track("Next Song", "Probe Artist", "Probe Album", 210.0));
    let ticket_first = tracker.report(Some(first.clone())).expect("looked up");
    let ticket_second = tracker.report(Some(second.clone())).expect("looked up");
    let late = Lyric { lines: parse_lrc("[00:01.00]line one\n[00:04.00]line two"), synced: true, matched: "title, artist, album" };
    let taken = tracker.answer(&ticket_first, Ok(Some(late)));
    println!("  the first song's answer arrives while the second plays: {}", if taken { "SHOWN" } else { "rejected" });
    println!("  the second song meanwhile: status {:?}, lines shown {}", tracker.status(), tracker.lyric().map_or(0, |l| l.lines.len()));
    tracker.answer(&ticket_second, Ok(None));
    println!("  the second song's own answer: status {:?}", tracker.status());
    let back = tracker.report(Some(first));
    println!(
        "  back to the first song: {}, status {:?}, {}",
        if back.is_none() { "no lookup" } else { "looked up again" },
        tracker.status(),
        tracker.lyric().map_or("nothing shown".into(), facts)
    );
}
