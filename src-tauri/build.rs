// Besides Tauri's own build step: writes $OUT_DIR/features_gen.rs, the list of
// feature crates, from this crate's Cargo.toml. Every `[dependencies]` key that
// starts with `notch-feat-` is one feature (scripts/new-feature.mjs adds them),
// so no Rust file lists features by hand. See docs/ARCHITECTURE.md §3.

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    let manifest = std::fs::read_to_string("Cargo.toml").expect("src-tauri/Cargo.toml");
    let mut calls = String::new();
    let mut in_dependencies = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]";
        } else if in_dependencies && line.starts_with("notch-feat-") {
            let name = line.split('=').next().unwrap_or_default().trim();
            calls.push_str(&format!("{}::feature(), ", name.replace('-', "_")));
        }
    }
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("features_gen.rs");
    let source = format!("pub fn all() -> Vec<std::sync::Arc<dyn notch_core::Feature>> {{ vec![{calls}] }}\n");
    std::fs::write(out, source).expect("features_gen.rs");

    tauri_build::build()
}
