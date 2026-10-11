use notch_feat_island_extras::disp;
use windows::core::{w, PCWSTR};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP, WINDOW_EX_STYLE};

fn main() {
    unsafe { let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2); }
    let ds = disp::displays();
    for (i, d) in ds.iter().enumerate() {
        println!("display {i}: rect={:?} scale={} primary={}", d.rect, d.scale, d.primary);
    }
    match disp::pointer() {
        Some((x, y)) => {
            let id = disp::display_at(x, y);
            let i = ds.iter().position(|d| d.id == id);
            println!("pointer ({x},{y}) is on display {:?}", i);
        }
        None => println!("pointer unreadable"),
    }
    let hwnd = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), PCWSTR::null(), WS_POPUP, -32000, -32000, 10, 10, None, None, None, None)
    }
    .expect("window");
    let h = hwnd.0 as isize;
    println!("probe window on display id {} (off-screen)", disp::display_of_window(h));
    println!("initial excluded = {:?}", disp::capture_excluded(h));
    println!("set true  -> {:?}, read back {:?}", disp::set_capture_excluded(h, true), disp::capture_excluded(h));
    println!("set false -> {:?}, read back {:?}", disp::set_capture_excluded(h, false), disp::capture_excluded(h));
    unsafe { let _ = DestroyWindow(hwnd); }
}
