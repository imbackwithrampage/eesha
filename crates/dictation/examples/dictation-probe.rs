// End to end without a microphone: downloads the Whistle model and the Needle engine into target/dictation-probe,
// verifies their hashes, has Windows' own text-to-speech speak a sentence to a 16 kHz mono WAV, and transcribes it
// through the engine code. Also creates the Windows recognizer (no session). Opens no microphone.
use notch_feat_dictation::whistle;
use std::time::Instant;

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/dictation-probe");
    let t = Instant::now();
    if let Err(e) = tauri::async_runtime::block_on(whistle::download(&dir, |n| println!("downloading {n}"))) {
        return println!("DOWNLOAD FAILED: {e}");
    }
    for f in &whistle::FILES {
        println!("verified {} sha256 {}", f.name, f.sha256);
    }
    println!("files ready: {} ({:.1}s)", whistle::ready(&dir), t.elapsed().as_secs_f32());

    let sentence = "The quick brown fox jumps over the lazy dog near the river bank.";
    let wav = dir.join("speech.wav");
    let script = format!(
        "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono); \
         $s.SetOutputToWaveFile('{}', $f); $s.Speak('{sentence}'); $s.Dispose()",
        wav.display()
    );
    let ps = std::path::Path::new(&std::env::var("SystemRoot").unwrap_or(r"C:\Windows".into())).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let ok = std::process::Command::new(ps).args(["-NoProfile", "-NonInteractive", "-Command", &script]).status().map(|s| s.success()).unwrap_or(false);
    let bytes = std::fs::read(&wav).unwrap_or_default();
    if !ok || bytes.len() < 44 {
        return println!("TEXT-TO-SPEECH FAILED");
    }
    let pcm: Vec<f32> = bytes[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
    println!("spoke: \"{sentence}\" ({:.1}s of audio)", pcm.len() as f32 / 16000.0);

    let t = Instant::now();
    match whistle::transcribe(&dir, &pcm) {
        Ok(r) => println!("transcript: \"{}\"\nlanguage: {}\ntime: {} ms", r.text, r.language, t.elapsed().as_millis()),
        Err(e) => println!("TRANSCRIBE FAILED: {e}"),
    }
    let _ = std::fs::remove_file(&wav);

    match notch_feat_dictation::engine::probe() {
        Ok(v) => println!("windows engine: recognizer created, compile {}, language {}", v["compile"], v["language"]),
        Err(e) => println!("windows engine unavailable: {}", e.msg),
    }
}
