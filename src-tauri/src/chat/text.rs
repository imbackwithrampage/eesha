// What a provider or a command-line tool says when it fails, cut down to what
// the chat can show: one line.
//
// `brief` is ported from upstream `windows/src-tauri/src/net.rs` (pull request
// 365), `strip_ansi` from upstream `windows/src-tauri/src/cursor_chat.rs`
// (pull request 151).

/// The most of someone else's words that goes in a message.
const MAX_DETAIL: usize = 200;

/// A message cut down to its first sentence, or its first line. It is shown in
/// a note made for one short line, after words of ours that already say what
/// to do; what a provider adds after that is advice for whoever writes code
/// against its API (Gemini's 404 goes on for two more sentences and a link).
pub fn brief(message: &str) -> String {
    let message = message.trim();
    let end = message
        .char_indices()
        .find(|&(i, c)| {
            let next = message[i + c.len_utf8()..].chars().next();
            c == '\n' || (matches!(c, '.' | '!' | '?') && next.is_some_and(char::is_whitespace))
        })
        .map_or(message.len(), |(i, c)| if c == '\n' { i } else { i + c.len_utf8() });
    message[..end].trim_end().chars().take(MAX_DETAIL).collect()
}

/// Text without the colour and cursor codes a terminal program prints.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // ESC [ … letter, or ESC and one more character.
        for c in chars.by_ref() {
            if c.is_ascii_alphabetic() {
                break;
            }
        }
    }
    out
}

/// What a command-line tool printed when it failed, as one line: the first
/// line that says something (not a stack frame, a brace or a blank).
pub fn first_words(output: &str) -> String {
    let clean = strip_ansi(output);
    let line = clean
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("at ") && !l.starts_with(['{', '}', '[', ']']))
        .unwrap_or("");
    brief(line.trim_start_matches("error:").trim_start_matches("Error:").trim())
}
