// The wheel's geometry: which slice a pointer position selects and where the
// wheel stands. Pure arithmetic, no Windows: `examples/radial-probe.rs` proves
// it. Ported in spirit from RadialMenuGeometry of the upstream utility
// (RadialMenuSupport.swift): slice 0 is centred at 12 o'clock and the slices
// run clockwise; a dead zone in the middle selects nothing.
//
// Lengths are logical pixels from the wheel's centre unless a name says
// "physical". The page (src/features/radial-menu/ring.ts) repeats the
// constants for drawing; change both together.

use std::f64::consts::TAU;

/// The window, a square. The disc is smaller: the rest is room for the shadow.
pub const SIZE: f64 = 400.0;
/// The outer edge of the slices.
pub const RADIUS: f64 = 150.0;
/// The hub and the dead zone: nothing is selected inside it.
pub const HUB: f64 = 40.0;
/// The pointer has to travel this far from where the wheel opened before anything highlights.
pub const WAKE: f64 = 8.0;
/// Kept between the disc and the edge of the screen.
const MARGIN: f64 = 12.0;

/// A screen area in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// What a click at an offset from the centre hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Hub,
    Slice(usize),
    Outside,
}

/// The slice an offset from the centre points at, at any distance beyond the
/// dead zone. `dy` grows downwards, as on the screen.
pub fn slice_at(dx: f64, dy: f64, n: usize) -> Option<usize> {
    if n == 0 || dx.hypot(dy) < HUB {
        return None;
    }
    // Clockwise from straight up.
    let angle = dx.atan2(-dy).rem_euclid(TAU);
    let step = TAU / n as f64;
    Some((angle / step).round() as usize % n)
}

/// Where a click lands: the hub, a slice of the disc, or outside it.
pub fn zone(dx: f64, dy: f64, n: usize) -> Zone {
    let d = dx.hypot(dy);
    if d < HUB || n == 0 {
        if d <= RADIUS { Zone::Hub } else { Zone::Outside }
    } else if d <= RADIUS {
        slice_at(dx, dy, n).map_or(Zone::Outside, Zone::Slice)
    } else {
        Zone::Outside
    }
}

/// The centre of the wheel in physical pixels: the pointer, moved just far
/// enough that the whole disc stays inside `area`. An area too small for the
/// disc gets the wheel in its middle.
pub fn centre(pointer: (i32, i32), area: Rect, scale: f64) -> (i32, i32) {
    let r = ((RADIUS + MARGIN) * scale).ceil() as i32;
    let fit = |p: i32, lo: i32, hi: i32| if hi - lo < 2 * r { (lo + hi) / 2 } else { p.clamp(lo + r, hi - r) };
    (fit(pointer.0, area.left, area.right), fit(pointer.1, area.top, area.bottom))
}

/// The middle of an area (the wheel when it does not follow the pointer).
pub fn middle(area: Rect) -> (i32, i32) {
    ((area.left + area.right) / 2, (area.top + area.bottom) / 2)
}

/// Top left and side of the window, physical, around a centre.
pub fn window(centre: (i32, i32), scale: f64) -> ((i32, i32), u32) {
    let side = (SIZE * scale).round() as i32;
    ((centre.0 - side / 2, centre.1 - side / 2), side as u32)
}
