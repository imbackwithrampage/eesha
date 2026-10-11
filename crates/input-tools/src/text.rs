// What a snippet's text becomes when it is inserted. The variables and their
// rules are ported from TextSnippetSupport.swift (`expand`): {{date}},
// {{time}}, {{datetime}}, {{clipboard}} and {{date:yyyy-MM-dd}}, {{time:HH:mm}},
// {{datetime:…}} with a pattern. A tag that is not one of these stays as typed.
// The clipboard's text is put in as it is: nothing inside it is expanded.

use windows::core::PCWSTR;
use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, DATE_SHORTDATE, ENUM_DATE_FORMATS_FLAGS, TIME_FORMAT_FLAGS, TIME_NOSECONDS};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Today in the user's own format, or by a Windows date picture ("yyyy-MM-dd").
fn date(picture: Option<&str>) -> String {
    let picture = picture.map(wide);
    let mut out = [0u16; 128];
    let n = unsafe {
        GetDateFormatEx(
            PCWSTR::null(),
            if picture.is_some() { ENUM_DATE_FORMATS_FLAGS(0) } else { DATE_SHORTDATE },
            None,
            picture.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
            Some(&mut out),
            PCWSTR::null(),
        )
    };
    String::from_utf16_lossy(&out[..(n.max(1) - 1) as usize])
}

/// Now, without seconds, or by a Windows time picture ("HH:mm").
fn time(picture: Option<&str>) -> String {
    let picture = picture.map(wide);
    let mut out = [0u16; 128];
    let n = unsafe {
        GetTimeFormatEx(
            PCWSTR::null(),
            if picture.is_some() { TIME_FORMAT_FLAGS(0) } else { TIME_NOSECONDS },
            None,
            picture.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
            Some(&mut out),
        )
    };
    String::from_utf16_lossy(&out[..(n.max(1) - 1) as usize])
}

/// A pattern in the usual letters (yyyy MM dd HH mm ss, MMM, EEEE, a): each
/// run of one letter is formatted on its own, the rest is copied; 'text' is literal.
// ponytail: years, months, days, weekdays, hours, minutes, seconds and AM/PM only; other pattern letters are copied as typed. A time zone (date-tz) stays literal.
fn pattern(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            let end = chars[i + 1..].iter().position(|x| *x == '\'').map_or(chars.len(), |p| i + 1 + p);
            out.extend(&chars[i + 1..end]);
            i = end + 1;
            continue;
        }
        let n = chars[i..].iter().take_while(|x| **x == c).count();
        let run: String = chars[i..i + n].iter().collect();
        match c {
            'y' if n == 1 => out.push_str(&date(Some("yyyy"))),
            'y' | 'M' | 'd' => out.push_str(&date(Some(&run))),
            'E' => out.push_str(&date(Some(if n >= 4 { "dddd" } else { "ddd" }))),
            'H' | 'h' | 'm' | 's' => out.push_str(&time(Some(&run))),
            'a' => out.push_str(&time(Some("tt"))),
            _ => out.push_str(&run),
        }
        i += n;
    }
    out
}

fn resolve(tag: &str, clipboard: &dyn Fn() -> String) -> Option<String> {
    match tag.split_once(':') {
        None => match tag {
            "date" => Some(date(None)),
            "time" => Some(time(None)),
            "datetime" => Some(format!("{} {}", date(None), time(None))),
            "clipboard" => Some(clipboard()),
            _ => None,
        },
        Some(("date" | "time" | "datetime", picture)) if !picture.is_empty() => Some(pattern(picture)),
        _ => None,
    }
}

/// The text with its variables filled in. `clipboard` is only called when the text asks for it.
pub fn expand(text: &str, clipboard: &dyn Fn() -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start..].find("}}") else { break };
        out.push_str(&rest[..start]);
        let whole = &rest[start..start + len + 2];
        match resolve(&whole[2..whole.len() - 2], clipboard) {
            Some(value) => out.push_str(&value),
            None => out.push_str(whole),
        }
        rest = &rest[start + len + 2..];
    }
    out.push_str(rest);
    out
}

/// Tiny self-check, run by the probe.
pub fn self_check() -> bool {
    let clip = || "{{date}}".to_string();
    let year = date(Some("yyyy"));
    expand("a {{nope}} b {{", &clip) == "a {{nope}} b {{"
        && expand("[{{clipboard}}]", &clip) == "[{{date}}]"
        && year.len() == 4
        && expand("{{date:yyyy}}-{{date:'yy'y}}", &clip) == format!("{year}-yy{year}")
        && expand("{{time:HH:mm}}", &clip).len() == 5
        && !expand("{{datetime}}", &clip).contains('{')
}
