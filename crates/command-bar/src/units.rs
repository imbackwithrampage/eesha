// Unit conversion for the bar: "10 km in mi", "20c to f", "5 gb to mb".
// Strict on purpose: a number, a unit it knows, one of the little words that
// mean "to", and another unit of the same family. Anything else is a search.
//
// Ported from reference/vorssaint-utils/Sources/Vorssaint/Services/CommandBar/CommandBarUnits.swift
// (a table of factors stands in for Foundation's Measurement).

use crate::math::{grouped, Separators};
use crate::search::fold;

/// The words that mean "convert into", in the languages people type. "in" is
/// also inches: it is only the verb when a unit follows it and a number with a
/// unit comes before.
const TO_WORDS: [&str; 16] = ["to", "in", "into", "as", "em", "para", "pra", "en", "a", "nach", "zu", "à", "su", "->", ">", "→"];

#[derive(Clone, Copy, PartialEq)]
enum Family {
    Temperature,
    Length,
    Mass,
    Data,
    Duration,
    Volume,
}

#[derive(Clone, Copy)]
struct Unit {
    family: Family,
    /// As it is written after the number.
    symbol: &'static str,
    /// How many of the family's base unit one of these is (metres, grams, bytes, seconds, litres).
    factor: f64,
    /// Added after scaling: temperatures are readings, not amounts (base: kelvin).
    offset: f64,
}

const fn unit(family: Family, symbol: &'static str, factor: f64) -> Unit {
    Unit { family, symbol, factor, offset: 0.0 }
}

/// Names are matched exactly once folded, never loosely: "mi" is miles and
/// nothing else, and a near miss is left to the search.
fn lookup(name: &str) -> Option<Unit> {
    use Family::*;
    Some(match name {
        "c" | "celsius" | "°c" | "centigrade" => Unit { family: Temperature, symbol: "°C", factor: 1.0, offset: 273.15 },
        "f" | "fahrenheit" | "°f" => Unit { family: Temperature, symbol: "°F", factor: 5.0 / 9.0, offset: 459.67 * 5.0 / 9.0 },
        "k" | "kelvin" => unit(Temperature, "K", 1.0),

        "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" | "milimetro" | "milimetros" => unit(Length, "mm", 0.001),
        "cm" | "centimeter" | "centimeters" | "centimetre" | "centimetres" | "centimetro" | "centimetros" => unit(Length, "cm", 0.01),
        "m" | "meter" | "meters" | "metro" | "metros" | "metre" | "metres" => unit(Length, "m", 1.0),
        "km" | "kilometer" | "kilometers" | "kilometre" | "kilometres" | "quilometro" | "quilometros" => unit(Length, "km", 1000.0),
        "in" | "inch" | "inches" | "polegada" | "polegadas" | "\"" => unit(Length, "in", 0.0254),
        "ft" | "foot" | "feet" | "pe" | "pes" => unit(Length, "ft", 0.3048),
        "yd" | "yard" | "yards" | "jarda" | "jardas" => unit(Length, "yd", 0.9144),
        "mi" | "mile" | "miles" | "milha" | "milhas" => unit(Length, "mi", 1609.344),

        "mg" | "milligram" | "milligrams" => unit(Mass, "mg", 0.001),
        "g" | "gram" | "grams" | "grama" | "gramas" => unit(Mass, "g", 1.0),
        "kg" | "kilogram" | "kilograms" | "quilo" | "quilos" => unit(Mass, "kg", 1000.0),
        "t" | "ton" | "tons" | "tonne" | "tonelada" | "toneladas" => unit(Mass, "t", 1e6),
        "oz" | "ounce" | "ounces" | "onca" | "oncas" => unit(Mass, "oz", 28.349523125),
        "lb" | "lbs" | "pound" | "pounds" | "libra" | "libras" => unit(Mass, "lb", 453.59237),

        // Decimal and binary are separate units: settling which was meant is why someone asks.
        "b" | "byte" | "bytes" => unit(Data, "B", 1.0),
        "kb" | "kilobyte" | "kilobytes" => unit(Data, "kB", 1e3),
        "mb" | "megabyte" | "megabytes" => unit(Data, "MB", 1e6),
        "gb" | "gigabyte" | "gigabytes" => unit(Data, "GB", 1e9),
        "tb" | "terabyte" | "terabytes" => unit(Data, "TB", 1e12),
        "kib" | "kibibyte" | "kibibytes" => unit(Data, "KiB", 1024.0),
        "mib" | "mebibyte" | "mebibytes" => unit(Data, "MiB", 1048576.0),
        "gib" | "gibibyte" | "gibibytes" => unit(Data, "GiB", 1073741824.0),
        "tib" | "tebibyte" | "tebibytes" => unit(Data, "TiB", 1099511627776.0),
        "bit" | "bits" => unit(Data, "bit", 0.125),

        "ms" | "millisecond" | "milliseconds" => unit(Duration, "ms", 0.001),
        "s" | "sec" | "secs" | "second" | "seconds" | "segundo" | "segundos" => unit(Duration, "s", 1.0),
        "min" | "mins" | "minute" | "minutes" | "minuto" | "minutos" => unit(Duration, "min", 60.0),
        "h" | "hr" | "hrs" | "hour" | "hours" | "hora" | "horas" => unit(Duration, "h", 3600.0),

        "ml" | "milliliter" | "milliliters" | "millilitre" | "millilitres" | "mililitro" | "mililitros" => unit(Volume, "mL", 0.001),
        "l" | "liter" | "liters" | "litre" | "litres" | "litro" | "litros" => unit(Volume, "L", 1.0),
        "gal" | "gallon" | "gallons" | "galao" | "galoes" => unit(Volume, "gal", 3.785411784),
        "qt" | "quart" | "quarts" => unit(Volume, "qt", 0.946352946),
        "floz" | "fluidounce" | "fluidounces" => unit(Volume, "fl oz", 0.0295735295625),
        "cup" | "cups" | "xicara" | "xicaras" => unit(Volume, "cup", 0.24),
        _ => return None,
    })
}

/// Folded words, with "100km" pulled apart into its number and its unit.
fn tokenize(input: &str) -> Vec<String> {
    let numeric = |c: char| c.is_ascii_digit() || matches!(c, '.' | ',' | '-');
    let mut tokens = Vec::new();
    for raw in fold(input).split(' ') {
        let digits = raw.chars().take_while(|c| numeric(*c)).count();
        // "->" is a word, not a number.
        if digits == 0 || !raw.chars().any(|c| c.is_ascii_digit()) {
            tokens.push(raw.to_string());
            continue;
        }
        tokens.push(raw[..digits].to_string());
        if digits < raw.len() {
            tokens.push(raw[digits..].to_string());
        }
    }
    tokens
}

/// As in the calculator: the other separator is a decimal point unless it
/// forms grouped thousands, and with both in a number the last is the decimal.
fn number(token: &str, sep: Separators) -> Option<f64> {
    let alternate = if matches!(sep.grouping, '.' | ',') && sep.grouping != sep.decimal { sep.grouping } else if sep.decimal == ',' { '.' } else { ',' };
    let (has_decimal, has_alternate) = (token.contains(sep.decimal), token.contains(alternate));
    let plain = if has_decimal && has_alternate {
        let (point, group) = if token.rfind(alternate) > token.rfind(sep.decimal) { (alternate, sep.decimal) } else { (sep.decimal, alternate) };
        token.replace(group, "").replace(point, ".")
    } else if has_alternate {
        let groups: Vec<&str> = token.trim_start_matches('-').split(alternate).collect();
        let grouping = groups.len() >= 2 && (1..=3).contains(&groups[0].len()) && !groups[0].starts_with('0') && groups[1..].iter().all(|g| g.len() == 3);
        token.replace(alternate, if grouping { "" } else { "." })
    } else {
        token.replace(sep.decimal, ".")
    };
    if plain.matches('.').count() > 1 || !plain.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    plain.parse().ok()
}

/// Small numbers keep their decimals, big ones lose the noise.
fn digits_for(value: f64) -> usize {
    match value.abs() {
        v if v == 0.0 => 0,
        v if v < 1.0 => 4,
        v if v < 100.0 => 2,
        _ => 1,
    }
}

fn written(value: f64, unit: Unit, sep: Separators) -> String {
    let gap = if unit.symbol.starts_with('°') { "" } else { " " };
    format!("{}{gap}{}", grouped(value, digits_for(value), sep), unit.symbol)
}

/// The conversion `input` asks for, in the unit it names, or None when it is not one.
pub fn convert(input: &str, sep: Separators) -> Option<String> {
    let tokens = tokenize(input);
    if !(3..=8).contains(&tokens.len()) {
        return None;
    }
    // Each word that could be the verb, from the right: in "5 in to cm" the
    // last one is the verb and the first a unit.
    for at in (1..tokens.len() - 1).rev() {
        if !TO_WORDS.contains(&tokens[at].as_str()) {
            continue;
        }
        let ([amount, from], [to]) = (&tokens[..at], &tokens[at + 1..]) else { continue };
        let (Some(from), Some(to), Some(amount)) = (lookup(from), lookup(to), number(amount, sep)) else { continue };
        if from.family != to.family {
            continue;
        }
        let value = ((amount * from.factor + from.offset) - to.offset) / to.factor;
        if !value.is_finite() {
            continue;
        }
        // A whole foot and more reads better with its inches.
        if to.symbol == "ft" && value >= 1.0 {
            let mut feet = value.floor();
            let scale = 10f64.powi(digits_for((value - feet) * 12.0) as i32);
            let mut inches = ((value - feet) * 12.0 * scale).round() / scale;
            if inches >= 12.0 {
                feet += 1.0;
                inches = 0.0;
            }
            let whole = written(feet, to, sep);
            return Some(if inches == 0.0 { whole } else { format!("{whole} {}", written(inches, lookup("in")?, sep)) });
        }
        return Some(written(value, to, sep));
    }
    None
}
