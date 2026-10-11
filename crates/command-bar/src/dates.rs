// The questions about dates people put into a search field: what day is it
// three weeks from now, tomorrow plus three days, how many days until a date.
// Answered by the calendar, never by the network. Narrow on purpose: an answer
// needs a full pattern (a number, a unit and a direction); anything short of
// that is left to the search, or "3 days" would be answered when nobody asked.
//
// Ported from reference/vorssaint-utils/Sources/Vorssaint/Services/CommandBar/CommandBarDates.swift
// (without "time in <city>": Windows has no table of cities by time zone).

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{
    GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, DATE_LONGDATE, ENUM_DATE_FORMATS_FLAGS, LOCALE_IDATE, LOCALE_SDECIMAL,
    LOCALE_STHOUSAND, TIME_NOSECONDS,
};

use crate::math::Separators;
use crate::search::fold;

// ── How this PC writes things ─────────────────────────────────────────────────

fn locale_text(what: u32) -> String {
    let mut text = [0u16; 16];
    let len = unsafe { GetLocaleInfoEx(PCWSTR::null(), what, Some(&mut text)) };
    String::from_utf16_lossy(&text[..(len - 1).max(0) as usize])
}

/// The decimal and grouping characters of the regional format (not of the display language).
pub fn separators() -> Separators {
    let first = |what, or| locale_text(what).chars().next().map_or(or, |c| if c == '\u{a0}' || c == '\u{202f}' { ' ' } else { c });
    Separators { decimal: first(LOCALE_SDECIMAL, '.'), grouping: first(LOCALE_STHOUSAND, ',') }
}

/// Whether this PC writes the day before the month.
pub fn day_first() -> bool {
    locale_text(LOCALE_IDATE) == "1"
}

// ── A day of the calendar ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

fn month_length(year: i32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    }
}

impl Day {
    pub fn today() -> Self {
        let now = notch_core::win::local_time();
        Self { year: now.year as i32, month: now.month, day: now.day }
    }

    /// Days since 1 January 1970 (the civil-calendar arithmetic everyone uses).
    fn number(self) -> i64 {
        let y = (self.year - i32::from(self.month <= 2)) as i64;
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let doy = (153 * ((self.month as i64 + 9) % 12) + 2) / 5 + self.day as i64 - 1;
        era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
    }

    fn from_number(days: i64) -> Self {
        let z = days + 719468;
        let era = z.div_euclid(146097);
        let doe = z.rem_euclid(146097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        Self { year: (yoe + era * 400 + i64::from(month <= 2)) as i32, month, day: (doy - (153 * mp + 2) / 5 + 1) as u32 }
    }

    pub fn plus_days(self, days: i64) -> Self {
        Self::from_number(self.number() + days)
    }

    /// The same day of another month; the 31st of a month without one is its last day.
    pub fn plus_months(self, months: i64) -> Self {
        let total = self.year as i64 * 12 + self.month as i64 - 1 + months;
        let (year, month) = (total.div_euclid(12) as i32, total.rem_euclid(12) as u32 + 1);
        Self { year, month, day: self.day.min(month_length(year, month)) }
    }

    fn valid(self) -> bool {
        (1..=12).contains(&self.month) && (1..=month_length(self.year, self.month)).contains(&self.day)
    }

    fn system_time(self) -> SYSTEMTIME {
        SYSTEMTIME { wYear: self.year as u16, wMonth: self.month as u16, wDay: self.day as u16, ..Default::default() }
    }

    fn written(self, flags: ENUM_DATE_FORMATS_FLAGS, picture: PCWSTR) -> String {
        let mut text = [0u16; 128];
        let len = unsafe { GetDateFormatEx(PCWSTR::null(), flags, Some(&self.system_time()), picture, Some(&mut text), PCWSTR::null()) };
        String::from_utf16_lossy(&text[..(len - 1).max(0) as usize])
    }

    /// The long date of the regional format: "Wednesday, October 14, 2026".
    pub fn long(self) -> String {
        self.written(DATE_LONGDATE, PCWSTR::null())
    }

    pub fn weekday(self) -> String {
        self.written(ENUM_DATE_FORMATS_FLAGS(0), w!("dddd"))
    }
}

/// The time it is now, as this PC writes it, without seconds.
pub fn time_now() -> String {
    let mut text = [0u16; 64];
    let len = unsafe { GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, None, PCWSTR::null(), Some(&mut text)) };
    String::from_utf16_lossy(&text[..(len - 1).max(0) as usize])
}

// ── Parser vocabulary ─────────────────────────────────────────────────────────
//
// Not visible text: people type in their own words whatever the language of the
// app, so each list carries several languages, already folded.

const TODAY: [&str; 10] = ["today", "hoje", "heute", "hoy", "oggi", "aujourdhui", "aujourd'hui", "сегодня", "今天", "आज"];
const TOMORROW: [&str; 9] = ["tomorrow", "amanha", "morgen", "manana", "domani", "demain", "завтра", "besok", "明天"];
const YESTERDAY: [&str; 9] = ["yesterday", "ontem", "gestern", "ayer", "ieri", "hier", "вчера", "kemarin", "昨天"];
const FUTURE: [&str; 12] = ["in", "em", "daqui", "dentro", "nach", "tra", "fra", "dans", "через", "sonra", "后", "from"];
const PAST: [&str; 9] = ["ago", "atras", "ha", "hace", "vor", "fa", "назад", "lalu", "前"];
const UNTIL: [&str; 9] = ["until", "till", "to", "ate", "hasta", "bis", "fino", "jusqu", "до"];

#[derive(Clone, Copy)]
enum Span {
    Days(i64),
    Months(i64),
}

fn span(word: &str) -> Option<Span> {
    Some(match word {
        "day" | "days" | "dia" | "dias" | "tag" | "tage" | "giorno" | "giorni" | "jour" | "jours" | "дня" | "дней" | "день" | "hari" | "天" | "日" | "दिन" => Span::Days(1),
        "week" | "weeks" | "semana" | "semanas" | "woche" | "wochen" | "settimana" | "settimane" | "semaine" | "semaines" | "недели" | "недель"
        | "неделя" | "minggu" | "周" | "星期" | "सप्ताह" => Span::Days(7),
        "month" | "months" | "mes" | "meses" | "monat" | "monate" | "mese" | "mesi" | "mois" | "месяц" | "месяца" | "месяцев" | "bulan" | "月" | "个月" => {
            Span::Months(1)
        }
        "year" | "years" | "ano" | "anos" | "jahr" | "jahre" | "anno" | "anni" | "an" | "ans" | "annee" | "annees" | "года" | "лет" | "год" | "tahun"
        | "年" => Span::Months(12),
        _ => return None,
    })
}

/// A date question and its answer: the answer, and one line that gives it sense.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub detail: String,
}

/// "in 3 weeks", "3 days ago", "today + 10 days", "tomorrow + 3 days",
/// "yesterday - 2 weeks", "days until 25/12". `count` writes "74 days".
pub fn evaluate(input: &str, today: Day, day_first: bool, count: impl Fn(u64) -> String) -> Option<Answer> {
    if input.chars().count() > 80 {
        return None;
    }
    // "tomorrow+3 days" is the same question as "tomorrow + 3 days".
    let folded = fold(&input.replace('+', " + "));
    let tokens: Vec<&str> = folded.split(' ').collect();
    if tokens.len() < 2 {
        return None;
    }
    relative(&tokens, today).or_else(|| until(&tokens, today, day_first, count))
}

fn relative(tokens: &[&str], today: Day) -> Option<Answer> {
    // The number and its unit sit together; the rest of the phrase only says
    // where to count from and which way.
    let (amount, unit) = tokens.windows(2).find_map(|pair| Some((pair[0].parse::<i64>().ok().filter(|n| n.abs() <= 10_000)?, span(pair[1])?)))?;
    let has = |words: &[&str]| tokens.iter().any(|t| words.contains(t));
    let from = if has(&TOMORROW) {
        today.plus_days(1)
    } else if has(&YESTERDAY) {
        today.plus_days(-1)
    } else if has(&TODAY) || has(&FUTURE) || has(&PAST) {
        today
    } else {
        // Without a direction "3 days" is not a question.
        return None;
    };
    let amount = if has(&PAST) || tokens.contains(&"-") { -amount } else { amount };
    let day = match unit {
        Span::Days(each) => from.plus_days(amount * each),
        Span::Months(each) => from.plus_months(amount * each),
    };
    Some(Answer { text: day.long(), detail: day.weekday() })
}

/// "days until 25/12": how far away a written date is. A year left out means
/// the next time that day comes around.
fn until(tokens: &[&str], today: Day, day_first: bool, count: impl Fn(u64) -> String) -> Option<Answer> {
    let at = tokens.iter().position(|t| UNTIL.contains(t))?;
    let numbers: Vec<i32> = tokens[at + 1..].join(" ").split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let (first, second, year) = match numbers[..] {
        [a, b] => (a, b, None),
        [a, b, y] => (a, b, Some(if y < 100 { 2000 + y } else { y })),
        _ => return None,
    };
    let (day, month) = if day_first { (first, second) } else { (second, first) };
    let on = |year| Day { year, month: month as u32, day: day as u32 };
    let target = match year {
        Some(year) => on(year),
        None if on(today.year) < today => on(today.year + 1),
        None => on(today.year),
    };
    if !target.valid() {
        return None;
    }
    Some(Answer { text: count((target.number() - today.number()).unsigned_abs()), detail: target.long() })
}
