// Proves crates/voice/src/weather.rs against a stand-in on 127.0.0.1 of its own (no real request):
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-weather-probe

use notch_feat_voice::weather::{find_with, gate_lookup, gate_weather, lookup_with, now_with, Urls};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (h, sn) = (hits.clone(), seen.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
            h.fetch_add(1, Ordering::SeqCst);
            sn.lock().unwrap().push(line.clone());
            let body = if line.contains("/geo") {
                r#"{"results":[{"name":"Paris","country":"France","latitude":48.85,"longitude":2.35}]}"#.to_string()
            } else if line.contains("/forecast") {
                r#"{"current":{"temperature_2m":18.4,"apparent_temperature":16.2,"weather_code":2},
                    "daily":{"weather_code":[2,61],"temperature_2m_max":[21.0,17.0],"temperature_2m_min":[12.0,9.0],"precipitation_probability_max":[30,80]}}"#
                    .to_string()
            } else if line.contains("gsrsearch=Mars") {
                r#"{"query":{"pages":[{"title":"Mars","extract":"Mars is the fourth planet from the Sun (after Mercury, Venus and Earth). It is a cold desert world. A third sentence."}]}}"#.to_string()
            } else {
                r#"{"query":{"pages":[{"title":"Mercury","extract":"Mercury may refer to:\n- a planet"}]}}"#.to_string()
            };
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    let urls = Urls { geocode: format!("{base}/geo"), forecast: format!("{base}/forecast"), wiki: format!("{base}/wiki") };

    // Switches off: refused before any request, also with a city set.
    let off = json!({"weather": false, "city": "Paris", "lookup": false});
    assert!(gate_weather(&off).is_err() && gate_lookup(&off).is_err());
    assert!(gate_weather(&json!({"weather": true, "city": "  "})).is_err(), "a switch with no city");
    assert_eq!(gate_weather(&json!({"weather": true, "city": " Paris "})).unwrap(), "Paris");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "with the switches off nothing was requested");
    println!("switches off: refused, 0 requests");

    let today = now_with(&urls, "Paris", 0, false).unwrap();
    println!("today: {today}");
    assert_eq!(today["city"], "Paris");
    assert_eq!(today["text"], "partly cloudy");
    assert_eq!(today["high"], 21.0);
    assert_eq!(today["rainChance"], 30.0);
    let tomorrow = now_with(&urls, "Paris", 1, true).unwrap();
    println!("tomorrow: {tomorrow}");
    assert_eq!(tomorrow["text"], "rainy");
    assert_eq!(tomorrow["unit"], "F");
    let lines = seen.lock().unwrap().clone();
    assert_eq!(lines.iter().filter(|l| l.contains("/geo")).count(), 1, "the city is geocoded once");
    let forecast = lines.iter().find(|l| l.contains("/forecast")).unwrap();
    assert!(forecast.contains("latitude=48.85") && !forecast.contains("Paris"), "the forecast carries coordinates only");
    println!("forecast request: {}", forecast.split(' ').nth(1).unwrap());

    let found = find_with(&urls, "Par").unwrap();
    assert_eq!(found[0]["country"], "France");

    let long = lookup_with(&urls, "Mars").unwrap();
    println!("lookup: {long}");
    assert_eq!(long["source"], "Wikipedia");
    let text = long["text"].as_str().unwrap();
    assert!(!text.contains('(') && text.starts_with("Mars is the fourth planet from the Sun."), "{text}");
    assert!(lookup_with(&urls, "Mercury").is_err(), "a disambiguation page is no answer");
    assert!(lookup_with(&urls, "   ").is_err());
    println!("ok: {} requests, all to 127.0.0.1", hits.load(Ordering::SeqCst));
}
