// Proves the wheel's geometry without the app or any window:
//
//   cargo run -p notch-feat-radial-menu --example radial-probe
//
// Which slice a pointer position selects for 4, 6 and 8 slices (at each slice's
// middle and either side of each boundary), the dead zone in the middle, and
// the wheel kept on screen near an edge and in a corner at 125 % scaling.
// Pure arithmetic: nothing is opened, sent or listened to.

use notch_feat_radial_menu::geometry::{centre, slice_at, window, zone, Rect, Zone, HUB, RADIUS, SIZE};

fn main() {
    let mut fails = 0;
    let mut check = |name: String, ok: bool| {
        println!("{} {name}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };

    // Offset at `deg` clockwise from straight up, `r` from the centre.
    let at = |deg: f64, r: f64| (r * deg.to_radians().sin(), -r * deg.to_radians().cos());

    for n in [4usize, 6, 8] {
        let step = 360.0 / n as f64;
        let middles = (0..n).all(|i| {
            let (dx, dy) = at(i as f64 * step, 100.0);
            slice_at(dx, dy, n) == Some(i)
        });
        check(format!("{n} slices: the middle of each slice selects it, slice 0 at 12 o'clock"), middles);
        let far = (0..n).all(|i| {
            let (dx, dy) = at(i as f64 * step, 2000.0);
            slice_at(dx, dy, n) == Some(i)
        });
        check(format!("{n} slices: selection works at any distance, far beyond the disc"), far);
        // Just inside the clockwise side of slice i is still i; just past it is i + 1.
        let edges = (0..n).all(|i| {
            let (a, b) = at(i as f64 * step + step / 2.0 - 0.5, 100.0);
            let (c, d) = at(i as f64 * step + step / 2.0 + 0.5, 100.0);
            slice_at(a, b, n) == Some(i) && slice_at(c, d, n) == Some((i + 1) % n)
        });
        check(format!("{n} slices: a boundary changes the slice within a degree"), edges);
        // Counter-clockwise from 12 o'clock is the last slice.
        let (dx, dy) = at(-1.0, 100.0);
        check(format!("{n} slices: just left of 12 o'clock is slice 0, a slice back is {}", n - 1), {
            let (px, py) = at(-step, 100.0);
            slice_at(dx, dy, n) == Some(0) && slice_at(px, py, n) == Some(n - 1)
        });
    }

    check("dead zone: nothing is selected inside the hub".into(), {
        slice_at(0.0, 0.0, 6).is_none()
            && slice_at(HUB - 0.5, 0.0, 6).is_none()
            && slice_at(0.0, -(HUB - 0.5), 6).is_none()
            && slice_at(HUB + 0.5, 0.0, 6).is_some()
    });
    check("an empty wheel selects nothing".into(), slice_at(100.0, 0.0, 0).is_none());
    check("click zones: hub, slice, outside".into(), {
        zone(0.0, 0.0, 6) == Zone::Hub
            && zone(0.0, -100.0, 6) == Zone::Slice(0)
            && zone(RADIUS + 1.0, 0.0, 6) == Zone::Outside
            && zone(RADIUS - 1.0, 10.0, 6) == Zone::Slice(2)
    });
    check("the full turn is covered: 360 positions give every slice of 8".into(), {
        let mut seen = [false; 8];
        for d in 0..360 {
            let (dx, dy) = at(d as f64, 90.0);
            seen[slice_at(dx, dy, 8).unwrap()] = true;
        }
        seen.iter().all(|s| *s)
    });

    // 1920x1080 physical at 125 % scaling (a 1536x864 desktop), taskbar 40 px at the bottom: 1920x1040 of work area.
    let scale = 1.25;
    let area = Rect { left: 0, top: 0, right: 1920, bottom: 1040 };
    let disc = (RADIUS * scale).ceil() as i32;
    let inside = |c: (i32, i32)| c.0 - disc >= area.left && c.0 + disc <= area.right && c.1 - disc >= area.top && c.1 + disc <= area.bottom;

    let mid = centre((960, 520), area, scale);
    check(format!("125 %: in the middle of the screen the wheel is exactly at the pointer {mid:?}"), mid == (960, 520));
    let left = centre((5, 520), area, scale);
    check(format!("125 %: pointer at the left edge, the wheel moves right, y kept {left:?}"), inside(left) && left.1 == 520 && left.0 > 5);
    let top = centre((960, 3), area, scale);
    check(format!("125 %: pointer at the top edge, the wheel moves down, x kept {top:?}"), inside(top) && top.0 == 960 && top.1 > 3);
    let corner = centre((1919, 1039), area, scale);
    check(format!("125 %: pointer in the bottom right corner {corner:?}"), inside(corner) && corner.0 < 1919 && corner.1 < 1039);
    let corner = centre((0, 0), area, scale);
    check(format!("125 %: pointer in the top left corner {corner:?}"), inside(corner));
    // A second display to the left (negative coordinates) with its own area.
    let left_screen = Rect { left: -1920, top: 0, right: 0, bottom: 1080 };
    let c = centre((-3, 1077), left_screen, scale);
    check(format!("125 %: corner of a display with negative coordinates {c:?}"), c.0 + disc <= 0 && c.1 + disc <= 1080 && c.0 - disc >= -1920);
    let tiny = Rect { left: 0, top: 0, right: 200, bottom: 200 };
    check("an area smaller than the disc gets the wheel in its middle".into(), centre((10, 10), tiny, scale) == (100, 100));

    let (origin, side) = window((960, 520), scale);
    check(format!("125 %: the window is {SIZE} logical = {side} physical, centred on the wheel, origin {origin:?}"), side == 500 && origin == (710, 270));

    println!("{}", if fails == 0 { "all passed" } else { "FAILED" });
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
