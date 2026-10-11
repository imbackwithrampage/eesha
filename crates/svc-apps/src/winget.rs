// winget, as far as every caller needs it: where the program is, what may stand
// on its command line, and how to read the tables it prints. winget has no
// structured output for `upgrade`, `list` or `search`, so the table is cut by
// the offsets of its header; the header's words are never read (they follow the
// system language).

use std::path::PathBuf;

/// The full path of winget.exe, or None when App Installer is not on this PC.
/// The per-user alias is asked first, so nothing planted earlier on PATH is run.
pub fn exe() -> Option<PathBuf> {
    let alias = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\WindowsApps\winget.exe"));
    alias
        .filter(|p| p.symlink_metadata().is_ok())
        .or_else(|| notch_core::win::find_on_path("winget").filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))))
}

/// A package id as winget writes one (`Git.Git`, `9NBLGGH4NNS1`). Anything else
/// never reaches a command line: no leading dash, no space, no quote, no `..`.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(c))
        && !id.contains("..")
}

/// Screen cells a character takes: winget pads its columns in cells, and a
/// Chinese, Japanese or Korean name is two cells a character.
// ponytail: combining marks count as one cell; a real width table if a row is ever cut wrong.
fn cells(c: char) -> usize {
    let wide = matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

/// The rows of the first table in winget's output, each with one cell per
/// header column (Name, Id, Version, then what the command adds). None when no
/// table was printed, which the caller must not read as "nothing found" unless
/// winget also ended well. Rows of a later table (pinned packages) are left out.
pub fn table(text: &str) -> Option<Vec<Vec<String>>> {
    // What is left after the last carriage return: the spinner is redrawn in place.
    let lines: Vec<&str> = text.lines().map(|l| l.rsplit('\r').next().unwrap_or("").trim_end()).collect();
    let rule = lines.iter().position(|l| l.len() >= 10 && l.chars().all(|c| c == '-'))?;
    let header = lines.get(rule.checked_sub(1)?)?;

    let mut starts = Vec::new();
    let (mut at, mut gap) = (0, true);
    for c in header.chars() {
        if !c.is_whitespace() && gap {
            starts.push(at);
        }
        gap = c.is_whitespace();
        at += cells(c);
    }
    if starts.len() < 3 {
        return None;
    }

    let mut rows = Vec::new();
    for line in &lines[rule + 1..] {
        let mut row = vec![String::new(); starts.len()];
        let mut at = 0;
        for c in line.chars() {
            row[starts.iter().rposition(|s| *s <= at).unwrap_or(0)].push(c);
            at += cells(c);
        }
        let row: Vec<String> = row.iter().map(|s| s.trim().to_string()).collect();
        // The table ends at the first line that is not a row: a blank one, or the count under it.
        if row[1].is_empty() || row[1].contains(char::is_whitespace) || row[2].is_empty() {
            break;
        }
        rows.push(row);
    }
    Some(rows)
}
