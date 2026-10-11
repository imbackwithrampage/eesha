// Parses an embedded ICS and prints the occurrences of one fixed week in local time, then the
// countdown for a fixed "now". No network. Run: cargo run -p notch-feat-calendar --example calendar-probe

use jiff::{civil::date, tz::TimeZone};
use notch_feat_calendar::ics;

const ICS: &str = "BEGIN:VCALENDAR\r
BEGIN:VEVENT\r
UID:single\r
SUMMARY:Dentist\r
DTSTART:20261014T100000\r
DTEND:20261014T110000\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:weekly\r
SUMMARY:Stand-up (Mon+Wed, one Wed skipped)\r
DTSTART:20260928T093000\r
DTEND:20260928T094500\r
RRULE:FREQ=WEEKLY;BYDAY=MO,WE\r
EXDATE:20261014T093000\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:monthly\r
SUMMARY:Board (3rd Thursday, 4 times)\r
DTSTART:20260820T090000\r
DTEND:20260820T100000\r
RRULE:FREQ=MONTHLY;BYDAY=3TH;COUNT=4\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:tz\r
SUMMARY:Call with New York\r
DTSTART;TZID=America/New_York:20261016T090000\r
DTEND;TZID=America/New_York:20261016T100000\r
BEGIN:VALARM\r
TRIGGER:-PT15M\r
DESCRIPTION:not an event\r
END:VALARM\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:utc\r
SUMMARY:Weekly sync in UTC until Friday\r
DTSTART:20261012T130000Z\r
DURATION:PT30M\r
RRULE:FREQ=DAILY;UNTIL=20261014T130000Z\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:yearly\r
SUMMARY:Anniversary\r
DTSTART;VALUE=DATE:20261017\r
RRULE:FREQ=YEARLY\r
END:VEVENT\r
END:VCALENDAR\r
";

fn main() {
    let local = TimeZone::system();
    let at = |d: i8, h: i8, m: i8| date(2026, 10, d).at(h, m, 0, 0).to_zoned(local.clone()).unwrap().timestamp().as_second();
    let (from, to) = (at(12, 0, 0), at(19, 0, 0));
    let events = ics::parse(ICS);
    println!("{} events parsed; week 2026-10-12..2026-10-18, zone {}", events.len(), local.iana_name().unwrap_or("?"));
    let mut occs: Vec<ics::Occ> = events.iter().flat_map(|e| e.expand(&local, from, to)).collect();
    occs.sort_by_key(|o| o.start);
    let show = |s: i64| jiff::Timestamp::from_second(s).unwrap().to_zoned(local.clone()).strftime("%a %m-%d %H:%M").to_string();
    for o in &occs {
        let when = if o.all_day { format!("{} all day", show(o.start).split(' ').take(2).collect::<Vec<_>>().join(" ")) } else { format!("{} -> {}", show(o.start), &show(o.end)[10..]) };
        match &o.unsupported {
            Some(why) => println!("  {when}  {}  [unsupported rule: {why}]", o.title),
            None => println!("  {when}  {}", o.title),
        }
    }
    let now = at(15, 8, 20);
    match ics::countdown(&occs, now) {
        Some(o) => println!("now {}: next event in {} min -> {} at {}", show(now), (o.start - now) / 60, o.title, show(o.start)),
        None => println!("now {}: no countdown", show(now)),
    }
    let later = at(15, 7, 0);
    println!("now {}: countdown {}", show(later), ics::countdown(&occs, later).map_or("none (nothing within the hour)", |_| "SHOWN"));

    // ── meeting links ──
    let cases: [(&str, &[&str]); 10] = [
        ("meet", &["", "Notes
https://meet.google.com/abc-defg-hij
bye"]),
        ("zoom", &["https://us02web.zoom.us/j/123456?pwd=x", ""]),
        ("teams", &["", "", "https://teams.microsoft.com/l/meetup-join/19%3a"]),
        ("webex", &["https://acme.webex.com/meet/jo"]),
        ("lookalike suffix", &["https://meet.google.com.evil.example/abc"]),
        ("lookalike prefix", &["https://notzoom.us/j/1", "https://zoom.us.evil.example/j/1"]),
        ("userinfo trick", &["https://meet.google.com@evil.example/abc"]),
        ("http refused", &["http://zoom.us/j/1"]),
        ("bad first, good later", &["https://evil.example/x https://acme.webex.com/meet/jo"]),
        ("no path", &["https://zoom.us"]),
    ];
    for (name, t) in cases {
        println!("link {name}: {}", ics::meeting_link(t).unwrap_or_else(|| "refused/none".into()));
    }
    let long = format!("{} https://teams.microsoft.com/l/meetup-join/x {}", "lorem ".repeat(2000), "ipsum ".repeat(2000));
    println!("link long description: {}", ics::meeting_link(&[&long]).unwrap_or_else(|| "none".into()));
    let ev = ics::parse(&["BEGIN:VEVENT", "UID:1", "SUMMARY:Sync", "DTSTART:20261015T090000Z", "DESCRIPTION:Join: https://acme.webex.com/meet/jo\nthanks", "END:VEVENT"].join("
"));
    println!("parsed DESCRIPTION link: {:?}", ev[0].expand(&local, 0, i64::MAX / 4)[0].link);

    // ── strip cards: in progress beats upcoming; same start = one card; lead window ──
    let it = |t: &str, s: i64, e: i64, l: bool| ics::Item { title: t.into(), start: s, end: e, link: l.then(|| "https://zoom.us/j/1".into()) };
    let m = 60;
    let show = |label: &str, c: Vec<ics::Card>| {
        let c: Vec<String> = c.iter().map(|c| format!("[{}{} @{}min: {}]", if c.in_progress { "LIVE " } else { "" }, c.items.len(), c.start / m, c.items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>().join("+"))).collect();
        println!("cards {label}: {}", c.join(" "));
    };
    let now = 1000 * m;
    let upcoming = [it("A", now + 10 * m, now + 40 * m, true), it("B", now + 10 * m, now + 50 * m, false), it("C", now + 25 * m, now + 50 * m, false), it("far", now + 90 * m, now + 99 * m, false)];
    show("lead 30 min (A+B together, C next, far hidden)", ics::cards(&upcoming, now, 30 * m));
    show("lead 15 min (C out of range)", ics::cards(&upcoming, now, 15 * m));
    let mut with_live = upcoming.to_vec();
    with_live.push(it("Live", now - 5 * m, now + 20 * m, true));
    with_live.push(it("Long no link", now - 60 * m, now + 600 * m, false));
    show("with a meeting in progress (it wins, upcoming hidden)", ics::cards(&with_live, now, 30 * m));
}
