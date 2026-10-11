// What a copied text is and what may be done with it: a web link and its
// tracking parameters, and the look of a secret. Text in, answer out; nothing
// here touches the clipboard.

use percent_encoding::percent_decode_str;

/// Tracking parameters removed on every site; `utm_` is matched as a prefix.
const EVERYWHERE: &[&str] = &[
    "fbclid", "gclid", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "mc_cid", "mc_eid", "igshid", "twclid",
    "ttclid", "li_fat_id", "mkt_tok", "_hsenc", "_hsmi", "__twitter_impression", "fb_action_ids", "fb_action_types",
    "fb_source", "mibextid",
];

/// Trackers only one site uses, for that host and its subdomains: `si` is a
/// share token on YouTube and a real parameter elsewhere, `t` a tracker on X
/// and the playback position on YouTube.
const SITES: &[(&str, &[&str])] = &[
    ("youtube.com", &["si", "pp", "feature", "kw"]),
    ("youtu.be", &["si", "pp", "feature", "kw"]),
    ("twitter.com", &["s", "t", "cn", "src", "refsrc", "ref_src", "ref_url"]),
    ("x.com", &["s", "t", "cn", "src", "refsrc", "ref_src", "ref_url"]),
    ("instagram.com", &["igsh"]),
    ("spotify.com", &["si"]),
    (
        "reddit.com",
        &["correlation_id", "ref_campaign", "ref_source", "rdt", "share_id", "_branch_match_id", "$deep_link", "$3p", "$original_url"],
    ),
    ("tiktok.com", &["u_code", "preview_pb", "_d", "_t", "_r", "timestamp", "user_id", "share_app_name", "share_iid"]),
    (
        "bilibili.com",
        &[
            "spm_id_from", "from_spmid", "from_source", "share_source", "share_from", "share_medium", "share_plat",
            "share_tag", "share_session_id", "msource", "refer_from", "seid", "unique_k", "vd_source", "plat_id",
            "buvid", "bbid", "up_id", "is_story_h5", "timestamp", "ts", "visit_id", "session_id", "broadcast_type",
            "is_room_feed",
        ],
    ),
    (
        "xiaohongshu.com",
        &[
            "xhsshare", "author_share", "xsec_source", "share_from_user_hidden", "shareredid", "share_id", "exsource",
            "app_version", "app_platform", "apptime", "appuid",
        ],
    ),
];

/// The host of a text that is one http(s) link and nothing else.
fn web_host(text: &str) -> Option<&str> {
    if text.contains(char::is_whitespace) {
        return None;
    }
    let (scheme, rest) = text.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    // Without the port; an IPv6 address keeps its brackets.
    let host = if host.starts_with('[') { host.split_inclusive(']').next()? } else { host.split(':').next()? };
    (!host.is_empty()).then_some(host)
}

/// One web address and nothing else.
pub fn is_web(text: &str) -> bool {
    web_host(text.trim()).is_some()
}

/// A link without its tracking parameters, and the names that were taken out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub url: String,
    pub removed: Vec<String>,
}

fn tracked(host: &str, name: &str, extra: &[String]) -> bool {
    name.starts_with("utm_")
        || EVERYWHERE.contains(&name)
        || extra.iter().any(|e| e == name)
        || SITES
            .iter()
            .any(|(site, names)| (host == *site || host.ends_with(&format!(".{site}"))) && names.contains(&name))
}

/// Removes the tracking parameters of a link. `None` when `text` is not one
/// web link; `removed` is empty when there was nothing to take out. The rest of
/// the link is kept exactly as written: nothing is re-encoded or re-ordered,
/// the `?` goes with the last parameter and the `#fragment` stays.
pub fn clean(text: &str, extra: &[String]) -> Option<Cleaned> {
    let link = text.trim();
    let host = web_host(link)?.to_lowercase();
    let fragment = link.find('#').unwrap_or(link.len());
    let Some(question) = link[..fragment].find('?') else {
        return Some(Cleaned { url: link.to_string(), removed: Vec::new() });
    };
    let mut removed: Vec<String> = Vec::new();
    let kept: Vec<&str> = link[question + 1..fragment]
        .split('&')
        .filter(|pair| {
            // The name half, decoded: `%75tm_source` is still utm_source. A pair without `=` is a bare flag.
            let raw = pair.split('=').next().unwrap_or_default();
            let name = percent_decode_str(raw).decode_utf8().map_or_else(|_| raw.to_string(), |n| n.into_owned());
            if !tracked(&host, &name.to_lowercase(), extra) {
                return true;
            }
            if !removed.contains(&name) {
                removed.push(name);
            }
            false
        })
        .collect();
    if removed.is_empty() {
        return Some(Cleaned { url: link.to_string(), removed });
    }
    let query = if kept.is_empty() { String::new() } else { format!("?{}", kept.join("&")) };
    Some(Cleaned { url: format!("{}{query}{}", &link[..question], &link[fragment..]), removed })
}

/// 8-4-4-4-12 hex digits, with or without braces: an identifier, nobody's secret.
fn is_identifier(text: &str) -> bool {
    let value = text.trim();
    let value = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')).unwrap_or(value);
    let groups: Vec<&str> = value.split('-').collect();
    groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12]) && groups.iter().all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The text looks like a real key: a known key prefix, a JWT, a PEM private-key
/// header, or one long unbroken high-entropy string. The words "password" or
/// "token" alone mean nothing. A guess; the marks password managers put on the
/// clipboard are what really keeps a secret out (history.rs).
pub fn looks_sensitive(text: &str) -> bool {
    let t = text.trim();
    if t.contains("-----BEGIN") && t.contains("PRIVATE KEY") {
        return true;
    }
    if is_web(t) || is_identifier(t) || t.contains(char::is_whitespace) {
        return false;
    }
    let known = ["sk-", "ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "AKIA", "ASIA", "xoxb-", "xoxp-", "xoxa-", "xoxs-", "xapp-", "AIza"];
    if t.len() >= 16 && known.iter().any(|p| t.starts_with(p)) {
        return true;
    }
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() == 3 && t.starts_with("eyJ") && parts.iter().all(|p| p.len() >= 4 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        return true;
    }
    // Long, mixing letters and digits, and random enough (a word or a path is not).
    let n = t.chars().count();
    if n < 32 || n > 512 || !t.chars().any(|c| c.is_ascii_digit()) || !t.chars().any(|c| c.is_ascii_alphabetic()) || t.contains('/') && t.contains('.') {
        return false;
    }
    let mut counts = std::collections::HashMap::new();
    for c in t.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let entropy: f64 = counts.values().map(|&k| { let p = k as f64 / n as f64; -p * p.log2() }).sum();
    entropy > 4.2
}

/// The checks the probe runs before it touches the clipboard.
pub fn self_check() {
    let none: [String; 0] = [];
    let c = clean("https://example.com/a?utm_source=x&id=7&fbclid=abc#top", &none).unwrap();
    assert_eq!((c.url.as_str(), c.removed.len()), ("https://example.com/a?id=7#top", 2));
    assert_eq!(clean("https://example.com/a?%75tm_medium=x", &none).unwrap().url, "https://example.com/a");
    // `si` goes on YouTube only, `t` stays there and goes on X.
    assert_eq!(clean("https://www.youtube.com/watch?v=1&t=42&si=abc", &none).unwrap().url, "https://www.youtube.com/watch?v=1&t=42");
    assert_eq!(clean("https://x.com/a/status/1?t=abc&s=20", &none).unwrap().url, "https://x.com/a/status/1");
    assert_eq!(clean("https://example.com/?si=abc", &none).unwrap().removed.len(), 0);
    assert_eq!(clean("https://example.com/?ref=a&b=1", &["ref".to_string()]).unwrap().url, "https://example.com/?b=1");
    assert!(clean("see https://example.com/?utm_source=x", &none).is_none());
    assert!(clean("ftp://example.com/?utm_source=x", &none).is_none());
    assert!(!looks_sensitive("my password is hunter2"));
    assert!(!looks_sensitive("let token = next_token();"));
    assert!(looks_sensitive("-----BEGIN RSA PRIVATE KEY-----"));
    assert!(looks_sensitive("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTYifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV"));
    assert!(looks_sensitive("wJalrXUtnFEMI8K7MDENGbPxRfiCYzR3v9sQ0Lp4aZ"));
    assert!(looks_sensitive("ghp_A1b2C3d4E5f6G7h8I9j0-KlMn"));
    assert!(!looks_sensitive("a plain sentence with 3 words more"));
    assert!(!looks_sensitive("{123e4567-e89b-12d3-a456-426614174000}"));
    assert!(!looks_sensitive("https://example.com/a1b2c3?d=4&e=5"));
}
