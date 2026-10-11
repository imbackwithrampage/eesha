// Two small live requests (geocoding, forecast) for "Dhaka", a bad city name, and the quick-access
// address check. No sound, no browser. Run: cargo run -p notch-feat-weather --example weather-probe

use notch_core::net;
use notch_feat_weather::{forecast_url, geocode_url, parse_forecast, parse_places};
use std::time::Duration;

fn get(url: &str) -> Result<String, String> {
    let b = tauri::async_runtime::block_on(net::bounded_get(url, Duration::from_secs(15), 512 * 1024))?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

fn main() {
    let places = parse_places(&get(&geocode_url("Dhaka")).expect("geocoding")).expect("parse places");
    println!("places for Dhaka: {}", places.len());
    println!("first: {}", places[0]);
    let f = parse_forecast(&get(&forecast_url(&places[0]["lat"], &places[0]["lon"])).expect("forecast")).expect("parse forecast");
    println!("forecast: {f}");
    println!("hours: {}", f["hours"].as_array().map_or(0, |h| h.len()));
    let bad = parse_places(&get(&geocode_url("Zzqxjkwv Nowhereville")).expect("bad geocoding")).unwrap();
    println!("bad city: {} results", bad.len());
    println!("encode(\"Sao Paulo, BR\") = {}", notch_feat_weather::encode("Sao Paulo, BR"));
    println!("garbage forecast: {:?}", parse_forecast("{}").err());

    for a in ["javascript:alert(1)", "file:///C:/Windows/notepad.exe", "example.com", "https://example.com/a?b=1", "http://user:pw@example.com", "  http://example.org  "] {
        println!("web_url({a:?}) = {:?}", notch_feat_island_extras::web_url(a));
    }
}
