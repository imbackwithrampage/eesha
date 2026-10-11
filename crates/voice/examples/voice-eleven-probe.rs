// Proves crates/voice/src/eleven.rs against a loopback stand-in of its own (no real service, no real key):
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-eleven-probe
//
// With the switch off no connection is made; with it on the key goes in the header; a failed, a slow and
// an empty sentence each end in Err before any audio (so `mouth` speaks it with the Windows voice); the
// key is in none of the error texts.

use notch_feat_voice::{eleven, AudioChunk, SynthRequest};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY: &str = "test-key-not-real-0123";

fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let heads = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let (hits, heads) = (hits.clone(), heads.clone());
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                let (hits, heads) = (hits.clone(), heads.clone());
                std::thread::spawn(move || serve(s, &hits, &heads));
            }
        });
    }
    eleven::use_stand_in(&base, KEY, Duration::from_millis(700));
    let say = |text: &str| SynthRequest { text: text.into(), voice: "eleven:abc123".into(), rate: 1.0, pitch: 1.0 };
    let run = |text: &str| {
        let mut pieces: Vec<AudioChunk> = Vec::new();
        let mut sink = |c: AudioChunk| {
            pieces.push(c);
            true
        };
        let r = eleven::synth(&say(text), &mut sink).map_err(|e| e.to_string());
        (r, pieces)
    };

    // 1. Off: nothing is sent although a key exists.
    eleven::set_on(false);
    assert!(!eleven::ready());
    let (r, _) = run("hello there");
    assert!(r.is_err() && hits.load(Ordering::SeqCst) == 0, "off must not connect");
    println!("off: refused, connections={}", hits.load(Ordering::SeqCst));

    // 2. On: audio arrives as 24 kHz pieces, the key and voice id in the request.
    eleven::set_on(true);
    assert!(eleven::ready());
    let (r, pieces) = run("hello there");
    r.expect("good sentence");
    let n: usize = pieces.iter().map(|p| p.samples.len()).sum();
    assert!(pieces.iter().all(|p| p.rate == 24_000) && n == 12_001, "samples {n}");
    let head = heads.lock().unwrap()[0].to_lowercase();
    assert!(head.contains("post /v1/text-to-speech/abc123?output_format=pcm_24000"), "{head}");
    assert!(head.contains(&format!("xi-api-key: {KEY}")), "key header");
    println!("on: {} pieces, {n} samples at 24000 Hz, key in header only", pieces.len());

    // 3. A failing, a slow and an empty sentence: Err, no audio, no key in the text.
    for (what, text) in [("fail", "please fail now"), ("slow", "please be slow"), ("empty", "please be empty")] {
        let t = std::time::Instant::now();
        let (r, pieces) = run(text);
        let err = r.expect_err(what);
        assert!(pieces.is_empty() && !err.contains(KEY), "{what}: {err}");
        println!("{what}: Err after {} ms, no audio, key not in text ({err})", t.elapsed().as_millis());
    }

    // 4. Voices come from the account only while on.
    eleven::refresh().expect("voices");
    let v: Vec<String> = eleven::voices().into_iter().map(|d| d.id).collect();
    assert_eq!(v, vec!["eleven:abc123".to_string()]);
    eleven::set_on(false);
    assert!(eleven::voices().is_empty());
    println!("voices: {v:?}; off again: none");
    println!("OK");
}

fn serve(mut s: std::net::TcpStream, hits: &AtomicUsize, heads: &Mutex<Vec<String>>) {
    hits.fetch_add(1, Ordering::SeqCst);
    let mut data = Vec::new();
    let mut b = [0u8; 1024];
    let (head, mut body) = loop {
        let n = s.read(&mut b).unwrap_or(0);
        if n == 0 {
            return;
        }
        data.extend_from_slice(&b[..n]);
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break (String::from_utf8_lossy(&data[..i]).to_string(), data[i + 4..].to_vec());
        }
    };
    let want = head.to_lowercase().split("content-length: ").nth(1).and_then(|r| r.split("\r\n").next()?.trim().parse::<usize>().ok()).unwrap_or(0);
    while body.len() < want {
        let n = s.read(&mut b).unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&b[..n]);
    }
    heads.lock().unwrap().push(head.clone());
    let text = String::from_utf8_lossy(&body).to_string();
    let reply = |s: &mut std::net::TcpStream, code: &str, ct: &str, payload: &[u8]| {
        let _ = write!(s, "HTTP/1.1 {code}\r\ncontent-type: {ct}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", payload.len());
        let _ = s.write_all(payload);
    };
    if head.contains("/v1/voices") {
        reply(&mut s, "200 OK", "application/json", br#"{"voices":[{"voice_id":"abc123","name":"Test"}]}"#);
    } else if text.contains("fail") {
        reply(&mut s, "500 Internal Server Error", "application/json", b"{}");
    } else if text.contains("slow") {
        std::thread::sleep(Duration::from_millis(2000));
        reply(&mut s, "200 OK", "audio/pcm", &[0u8; 100]);
    } else if text.contains("empty") {
        reply(&mut s, "200 OK", "audio/pcm", b"");
    } else {
        // 12,001 samples: an odd number of bytes would split a sample, so 24,002 bytes.
        reply(&mut s, "200 OK", "audio/pcm", &vec![0x10u8; 24_002]);
    }
}
