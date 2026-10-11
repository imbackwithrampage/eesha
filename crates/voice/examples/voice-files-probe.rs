// Proves crates/voice/src/files.rs on files of its own, under target/voice-dev/files/ (nothing of the
// owner's is read, nothing is started):
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-files-probe

use notch_feat_voice::files::{find_in, list_in};
use std::fs;
use std::path::PathBuf;

fn touch(p: &PathBuf) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"x").unwrap();
}

fn main() {
    let base = PathBuf::from("target/voice-dev/files");
    let _ = fs::remove_dir_all(&base);
    let (a, b, outside) = (base.join("all"), base.join("user"), base.join("elsewhere"));
    for f in [
        a.join("Notepad.lnk"),
        a.join("Tools").join("Paint.LNK"),
        a.join("Tools").join("Uninstall Paint.lnk"),
        a.join("readme.txt"),
        a.join("run.exe"),
        b.join("Notepad.lnk"),
        b.join("Deep").join("Chat.lnk"),
        outside.join("Evil.lnk"),
    ] {
        touch(&f);
    }
    let roots = [a.clone(), b.clone()];
    let names: Vec<String> = list_in(&roots).into_iter().map(|(n, _)| n).collect();
    println!("apps: {names:?}");
    assert_eq!(names, ["Chat", "Notepad", "Paint"], "shortcuts only, no uninstaller, a name once");

    let listed = |p: &PathBuf| find_in(&roots, &p.to_string_lossy());
    assert!(listed(&a.join("Tools").join("Paint.LNK")).is_some(), "a listed shortcut opens");
    assert!(listed(&b.join("Notepad.lnk")).is_some(), "the twin of a name listed once was offered too");
    let mut refused = 0;
    for bad in [
        outside.join("Evil.lnk").to_string_lossy().into_owned(),
        a.join("run.exe").to_string_lossy().into_owned(),
        a.join("readme.txt").to_string_lossy().into_owned(),
        a.join("Tools").join("Uninstall Paint.lnk").to_string_lossy().into_owned(),
        a.join("..").join("elsewhere").join("Evil.lnk").to_string_lossy().into_owned(),
        a.to_string_lossy().into_owned(),
        "cmd.exe".to_string(),
        "notepad".to_string(),
        r"C:\Windows\System32\cmd.exe /c calc".to_string(),
        format!("{} /c calc", a.join("Notepad.lnk").display()),
        "https://example.com".to_string(),
        String::new(),
    ] {
        assert!(find_in(&roots, &bad).is_none(), "must refuse: {bad}");
        refused += 1;
    }
    println!("refused {refused} of {refused} paths that are not a listed Start Menu shortcut");
    println!("real Start Menu: {} apps listed (names not printed)", list_in(&notch_feat_voice::files::roots()).len());
}
