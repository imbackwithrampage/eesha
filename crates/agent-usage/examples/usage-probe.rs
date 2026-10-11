// Reads the real local logs and prints totals only: no text, no file contents, no paths.
use notch_feat_agent_usage::usage::{now_ms, roots, Store};
use std::time::Instant;

fn main() {
    let roots = roots();
    let mut s = Store::default();
    let t0 = Instant::now();
    s.scan(&roots, now_ms(), false);
    println!("first full parse: {} ms ({} log folders)", t0.elapsed().as_millis(), roots.len());
    let t1 = Instant::now();
    s.scan(&roots, now_ms(), true);
    println!("second pass (nothing new): {} ms", t1.elapsed().as_millis());
    let v = s.summary(now_ms());
    for p in v["providers"].as_array().unwrap() {
        println!("== {}  (open turns: {})", p["name"].as_str().unwrap(), p["live"].as_array().unwrap().len());
        for k in ["today", "week"] {
            let x = &p[k];
            let names: Vec<&str> = x["models"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
            println!(
                "  {k:5} input {} output {} cacheRead {} cacheWrite {} | requests {} | projects {} | models {:?}",
                x["input"], x["output"], x["cacheRead"], x["cacheWrite"], x["requests"],
                x["projects"].as_array().unwrap().len(), names
            );
        }
    }
}
