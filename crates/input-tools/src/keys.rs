// Keys and combinations, by the names Settings stores and shows: "CapsLock",
// "Alt+F4", "Ctrl+Shift+Left", "Win". Modifiers first, then one key; a
// combination may also end on a modifier ("Ctrl", "Ctrl+Shift").

use svc_input::Key;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

pub const CTRL: u8 = 1;
pub const ALT: u8 = 2;
pub const SHIFT: u8 = 4;
pub const WIN: u8 = 8;

/// The modifiers in the order they are written, with the key each is sent as (the left one).
const MODIFIERS: [(u8, &str, u16); 4] = [(CTRL, "Ctrl", 0xA2), (ALT, "Alt", 0xA4), (SHIFT, "Shift", 0xA0), (WIN, "Win", 0x5B)];

const NAMED: &[(&str, u16)] = &[
    ("Esc", 0x1B),
    ("Tab", 0x09),
    ("CapsLock", 0x14),
    ("Space", 0x20),
    ("Enter", 0x0D),
    ("Backspace", 0x08),
    ("Delete", 0x2E),
    ("Insert", 0x2D),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("PrintScreen", 0x2C),
    ("ScrollLock", 0x91),
    ("Pause", 0x13),
    ("NumLock", 0x90),
    ("Menu", 0x5D),
    ("Semicolon", 0xBA),
    ("Equal", 0xBB),
    ("Comma", 0xBC),
    ("Minus", 0xBD),
    ("Period", 0xBE),
    ("Slash", 0xBF),
    ("Backquote", 0xC0),
    ("BracketLeft", 0xDB),
    ("Backslash", 0xDC),
    ("BracketRight", 0xDD),
    ("Quote", 0xDE),
    ("VolumeMute", 0xAD),
    ("VolumeDown", 0xAE),
    ("VolumeUp", 0xAF),
    ("MediaNext", 0xB0),
    ("MediaPrevious", 0xB1),
    ("MediaStop", 0xB2),
    ("MediaPlay", 0xB3),
];

/// A combination: the modifiers held and the key, left and right not told
/// apart. For "Ctrl" alone the key is the modifier itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Chord {
    pub mods: u8,
    pub vk: u16,
}

/// The modifier a virtual key is (0 for any other key).
pub fn modifier_of(vk: u16) -> u8 {
    match vk {
        0x11 | 0xA2 | 0xA3 => CTRL,
        0x12 | 0xA4 | 0xA5 => ALT,
        0x10 | 0xA0 | 0xA1 => SHIFT,
        0x5B | 0x5C => WIN,
        _ => 0,
    }
}

/// Left and right folded into one: the key a combination names.
pub fn generic(vk: u16) -> u16 {
    match modifier_of(vk) {
        CTRL => 0x11,
        ALT => 0x12,
        SHIFT => 0x10,
        WIN => 0x5B,
        _ => vk,
    }
}

fn modifier_named(name: &str) -> Option<u8> {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(CTRL),
        "alt" => Some(ALT),
        "shift" => Some(SHIFT),
        "win" | "super" | "meta" => Some(WIN),
        _ => None,
    }
}

fn key_named(name: &str) -> Option<u16> {
    if let Some(bit) = modifier_named(name) {
        return Some(generic(MODIFIERS.iter().find(|m| m.0 == bit)?.2));
    }
    let upper = name.to_ascii_uppercase();
    match upper.as_bytes() {
        [c] if c.is_ascii_alphanumeric() => return Some(*c as u16),
        [b'F', digits @ ..] if !digits.is_empty() && digits.iter().all(u8::is_ascii_digit) => {
            return match upper[1..].parse::<u16>().ok()? {
                n @ 1..=24 => Some(0x70 + n - 1),
                _ => None,
            };
        }
        _ => {}
    }
    let name = if upper == "ESCAPE" { "Esc" } else { name };
    NAMED.iter().find(|k| k.0.eq_ignore_ascii_case(name)).map(|k| k.1)
}

/// The key has a name here (asked on the hook thread: nothing is allocated).
pub fn known(vk: u16) -> bool {
    matches!(vk, 0x30..=0x39 | 0x41..=0x5A | 0x70..=0x87) || modifier_of(vk) != 0 || NAMED.iter().any(|k| k.1 == vk)
}

fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some((vk as u8 as char).to_string()),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        _ => match modifier_of(vk) {
            0 => NAMED.iter().find(|k| k.1 == vk).map(|k| k.0.to_string()),
            bit => MODIFIERS.iter().find(|m| m.0 == bit).map(|m| m.1.to_string()),
        },
    }
}

/// "Ctrl+Shift+Left" → the combination. None for anything that is not one.
pub fn parse(text: &str) -> Option<Chord> {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let vk = key_named(parts.pop()?)?;
    let mut mods = 0;
    for part in parts {
        mods |= modifier_named(part)?;
    }
    Some(Chord { mods: mods & !modifier_of(vk), vk })
}

/// The combination as it is stored and shown. None for a key that has no name here.
pub fn name(chord: Chord) -> Option<String> {
    let mut parts: Vec<String> = MODIFIERS.iter().filter(|m| chord.mods & m.0 != 0).map(|m| m.1.to_string()).collect();
    parts.push(key_name(chord.vk)?);
    Some(parts.join("+"))
}

/// The keys to press for a combination, in order (released in reverse):
/// modifiers as their left key, then the key. Empty for "" (nothing).
pub fn strokes(text: &str) -> Option<Vec<u16>> {
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    let chord = parse(text)?;
    let side = |vk: u16| MODIFIERS.iter().find(|m| m.0 == modifier_of(vk)).map_or(vk, |m| m.2);
    let mut keys: Vec<u16> = MODIFIERS.iter().filter(|m| chord.mods & m.0 != 0).map(|m| m.2).collect();
    keys.push(side(chord.vk));
    Some(keys)
}

/// The modifiers that are down right now (the app's own injected ones included).
pub fn held() -> u8 {
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
    let mut mods = 0;
    if down(0x11) {
        mods |= CTRL;
    }
    if down(0x12) {
        mods |= ALT;
    }
    if down(0x10) {
        mods |= SHIFT;
    }
    if down(0x5B) || down(0x5C) {
        mods |= WIN;
    }
    mods
}

/// The combination a key event completes: the key, with the other modifiers held.
pub fn of_event(key: &Key) -> Chord {
    Chord { mods: held() & !modifier_of(key.vk), vk: generic(key.vk) }
}

/// The left and right keys of the modifiers in `mods` that are down right now.
pub fn held_keys(mods: u8) -> impl Iterator<Item = u16> {
    [(CTRL, 0xA2u16), (CTRL, 0xA3), (ALT, 0xA4), (ALT, 0xA5), (SHIFT, 0xA0), (SHIFT, 0xA1), (WIN, 0x5B), (WIN, 0x5C)]
        .into_iter()
        .filter(move |(bit, vk)| mods & bit != 0 && unsafe { GetAsyncKeyState(*vk as i32) } < 0)
        .map(|k| k.1)
}

/// Tiny self-check, run by the probe: the names round-trip.
pub fn self_check() -> bool {
    let cases = ["CapsLock", "Alt+F4", "Ctrl+Shift+Left", "Win", "Ctrl+Alt+Shift+Win+A", "Ctrl+Home", "F13", "Shift+Semicolon"];
    cases.iter().all(|c| parse(c).and_then(name).as_deref() == Some(*c))
        && parse("Ctrl+Nope").is_none()
        && parse("escape") == parse("Esc")
        && parse("Ctrl+Shift") == Some(Chord { mods: CTRL, vk: 0x10 })
        && strokes("Ctrl+Home") == Some(vec![0xA2, 0x24])
        && strokes("") == Some(Vec::new())
}
