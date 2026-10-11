// Capture — a shared service (docs/ARCHITECTURE.md §8): the screen as pixels,
// for any feature. Displays, a grab of a rectangle or of one window, PNG, the
// clipboard as a picture, text recognition and QR codes.
//
// Everything is in physical pixels of the virtual desktop (top-left origin),
// whatever the DPI awareness of the calling thread: a rectangle that is asked
// for comes back as exactly that many pixels.
//
// Nothing here runs on its own: every function works on the caller's time and
// leaves no thread behind.

use std::ffi::c_void;
use std::mem::size_of;

use svc_inject::clip;
use windows::core::BOOL;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GdiFlush, GetDC,
    GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, DIB_RGB_COLORS, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowDisplayAffinity, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SetWindowDisplayAffinity, GWL_EXSTYLE, WDA_NONE, WINDOW_DISPLAY_AFFINITY, WS_EX_TRANSPARENT,
};

/// A display or a window as a stream of frames (Windows Graphics Capture), for the recorder.
pub mod wgc;

/// Left out of every capture, shown on screen as usual (Windows 10 2004 and later).
const WDA_EXCLUDEFROMCAPTURE: WINDOW_DISPLAY_AFFINITY = WINDOW_DISPLAY_AFFINITY(0x11);
const PW_RENDERFULLCONTENT: u32 = 2;
/// MONITORINFOF_PRIMARY.
const PRIMARY: u32 = 1;

// ── What travels ──────────────────────────────────────────────────────────────

/// Physical pixels; `x`/`y` from the top-left corner of the primary display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    /// The part of this rectangle that lies inside `other` (empty when they do not meet).
    pub fn clip(&self, other: Rect) -> Rect {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let (right, bottom) = ((self.x + self.w).min(other.x + other.w), (self.y + self.h).min(other.y + other.h));
        Rect { x, y, w: (right - x).max(0), h: (bottom - y).max(0) }
    }
}

fn rect_of(r: RECT) -> Rect {
    Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

/// A picture: opaque RGBA, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Red, green, blue of one pixel.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        (x < self.w && y < self.h).then(|| {
            let at = (y as usize * self.w as usize + x as usize) * 4;
            [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2]]
        })
    }

    /// The part inside `area` (pixels of this picture); what lies outside the picture is left out.
    pub fn crop(&self, area: Rect) -> Image {
        let r = area.clip(Rect { x: 0, y: 0, w: self.w as i32, h: self.h as i32 });
        let (x, y, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let at = (row * self.w as usize + x) * 4;
            rgba.extend_from_slice(&self.rgba[at..at + w * 4]);
        }
        Image { w: w as u32, h: h as u32, rgba }
    }
}

#[derive(Debug, Clone)]
pub struct Monitor {
    /// The display's device name (`\\.\DISPLAY1`).
    pub id: String,
    pub rect: Rect,
    /// 1.25 at 125 %.
    pub scale: f64,
    pub primary: bool,
}

/// A window that can be captured, by its visible frame.
#[derive(Debug, Clone, Copy)]
pub struct Win {
    pub hwnd: isize,
    pub rect: Rect,
}

/// One line of recognised text and where it is in the picture.
#[derive(Debug, Clone)]
pub struct Line {
    pub text: String,
    pub rect: Rect,
}

// ── Physical pixels ───────────────────────────────────────────────────────────

/// The calling thread sees the desktop in physical pixels while this lives.
struct Aware(DPI_AWARENESS_CONTEXT);

impl Aware {
    fn new() -> Self {
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for Aware {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

/// Every display, the primary one first.
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
        unsafe {
            let list = &mut *(out.0 as *mut Vec<Monitor>);
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
                let (mut dpi, mut dpi_y) = (96u32, 96u32);
                let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
                let name = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
                list.push(Monitor {
                    id: String::from_utf16_lossy(&info.szDevice[..name]),
                    rect: rect_of(info.monitorInfo.rcMonitor),
                    scale: dpi as f64 / 96.0,
                    primary: info.monitorInfo.dwFlags & PRIMARY != 0,
                });
            }
            true.into()
        }
    }
    let _aware = Aware::new();
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by_key(|m| !m.primary);
    list
}

// ── Grabs ─────────────────────────────────────────────────────────────────────

/// A `w`×`h` picture of whatever `draw` puts into the memory DC it is given (the screen's DC is the second).
fn render(w: i32, h: i32, draw: impl FnOnce(HDC, HDC) -> bool) -> Result<Image, String> {
    if w <= 0 || h <= 0 || w > 65_536 || h > 65_536 {
        return Err("There is nothing to capture there.".into());
    }
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: top row first.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let out = match CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) if !bits.is_null() => {
                let before = SelectObject(memory, bitmap.into());
                let drawn = draw(memory, screen);
                let _ = GdiFlush();
                let image = drawn.then(|| {
                    // BGRA with an alpha GDI leaves at anything: RGBA, opaque.
                    let mut rgba = std::slice::from_raw_parts(bits as *const u8, w as usize * h as usize * 4).to_vec();
                    for px in rgba.chunks_exact_mut(4) {
                        px.swap(0, 2);
                        px[3] = 255;
                    }
                    Image { w: w as u32, h: h as u32, rgba }
                });
                SelectObject(memory, before);
                let _ = DeleteObject(bitmap.into());
                image.ok_or_else(|| "Windows did not hand over the picture.".to_string())
            }
            _ => Err("There is not enough memory for the picture.".to_string()),
        };
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        out
    }
}

/// What is on screen inside `rect` right now, as exactly `rect.w`×`rect.h`
/// pixels. Windows that are excluded from capture are not in it.
pub fn grab(rect: Rect) -> Result<Image, String> {
    let _aware = Aware::new();
    render(rect.w, rect.h, |memory, screen| unsafe {
        BitBlt(memory, 0, 0, rect.w, rect.h, Some(screen), rect.x, rect.y, SRCCOPY | CAPTUREBLT).is_ok()
    })
}

/// The window's visible frame (without the invisible resize borders).
fn frame_of(hwnd: HWND) -> Option<Rect> {
    let mut frame = RECT::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut _ as *mut c_void, size_of::<RECT>() as u32)
            .ok()?;
    }
    Some(rect_of(frame))
}

/// One window from its own buffer, whatever covers it on screen. Some programs
/// draw nothing this way and give a blank picture: fall back on `grab` then.
pub fn grab_window(hwnd: isize) -> Result<Image, String> {
    let _aware = Aware::new();
    let hwnd = HWND(hwnd as *mut c_void);
    let mut outer = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut outer) }.map_err(|e| e.message())?;
    let frame = frame_of(hwnd).unwrap_or(rect_of(outer));
    let whole = render(outer.right - outer.left, outer.bottom - outer.top, |memory, _| unsafe {
        PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool()
    })?;
    Ok(whole.crop(Rect { x: frame.x - outer.left, y: frame.y - outer.top, w: frame.w, h: frame.h }))
}

/// The windows a click could mean, front to back: visible, not minimised, not
/// hidden by Windows, not click-through, and not this program's own.
pub fn windows() -> Vec<Win> {
    unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> BOOL {
        unsafe {
            let list = &mut *(out.0 as *mut Vec<Win>);
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut c_void, 4);
            let through = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0;
            if IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() && cloaked == 0 && !through && pid != GetCurrentProcessId() {
                if let Some(rect) = frame_of(hwnd).filter(|r| r.w > 0 && r.h > 0) {
                    list.push(Win { hwnd: hwnd.0 as isize, rect });
                }
            }
            true.into()
        }
    }
    let _aware = Aware::new();
    let mut list: Vec<Win> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

/// Leaves a window of this program out of every capture (or puts it back). It
/// stays on screen as it is.
pub fn exclude_from_capture(hwnd: isize, on: bool) -> bool {
    let affinity = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut c_void), affinity) }.is_ok()
}

pub fn excluded_from_capture(hwnd: isize) -> bool {
    let mut affinity = 0u32;
    unsafe { GetWindowDisplayAffinity(HWND(hwnd as *mut c_void), &mut affinity) }.is_ok() && affinity == WDA_EXCLUDEFROMCAPTURE.0
}

// ── PNG and the clipboard ─────────────────────────────────────────────────────

// debt: the png crate is slow in a debug build (a second for a whole 4K screen); WIC would not be.
pub fn encode_png(image: &Image) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, image.w, image.h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&image.rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
    pixels.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels,
        png::ColorType::Rgb => pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => pixels.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|p| [*p, *p, *p, 255]).collect(),
        png::ColorType::Indexed => return Err("That PNG cannot be read.".into()),
    };
    Ok(Image { w: info.width, h: info.height, rgba })
}

/// The picture as a device-independent bitmap (CF_DIB): a BITMAPINFOHEADER,
/// then BGRA rows, bottom row first. With a 14-byte file header in front it is
/// a .bmp file.
pub fn dib(image: &Image) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: image.w as i32,
        biHeight: image.h as i32,
        biPlanes: 1,
        biBitCount: 32,
        biSizeImage: image.w * image.h * 4,
        ..Default::default()
    };
    let head = unsafe { std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<BITMAPINFOHEADER>()) };
    let mut out = Vec::with_capacity(head.len() + image.rgba.len());
    out.extend_from_slice(head);
    for row in image.rgba.chunks_exact(image.w.max(1) as usize * 4).rev() {
        out.extend_from_slice(row);
    }
    for px in out[head.len()..].chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}

/// Puts the picture on the clipboard, as a bitmap every program pastes and as
/// PNG for those that prefer it. `png` is the same picture from `encode_png`.
pub fn clipboard_image(image: &Image, png: &[u8]) -> Result<(), String> {
    clip::open()?.empty_then(|clipboard| {
        clipboard.set(clip::CF_DIB, &dib(image))?;
        clipboard.set(clip::format("PNG"), png)
    })
}

// ── Text and codes ────────────────────────────────────────────────────────────

/// WinRT wants a multithreaded apartment, and callers come from thread pools
/// in any state: the work gets a thread of its own, which is in that apartment
/// by not choosing one. The apartment is kept for good from the first use
/// (a count, not a thread): the WinRT factories windows-rs remembers would
/// otherwise die with it when its last thread left.
fn mta<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    static KEEP: std::sync::Once = std::sync::Once::new();
    KEEP.call_once(|| unsafe {
        let _ = CoIncrementMTAUsage();
    });
    std::thread::scope(|scope| scope.spawn(work).join().expect("capture worker"))
}

/// Whether Windows can read text here: its recogniser needs a language pack
/// of one of the user's languages. Hide the tool when it cannot.
pub fn ocr_available() -> bool {
    mta(|| OcrEngine::TryCreateFromUserProfileLanguages().is_ok())
}

/// Twice the size, smoothed: the recogniser misses text as small as a screen draws it.
fn doubled(image: &Image) -> Image {
    let (w, h) = (image.w as usize, image.h as usize);
    // The two source pixels an output pixel lies between, and the share of the second, in quarters.
    let taps = |at: usize, n: usize| if at % 2 == 0 { ((at / 2).saturating_sub(1), at / 2, 3) } else { (at / 2, (at / 2 + 1).min(n - 1), 1) };
    let mut rgba = vec![255u8; w * h * 16];
    for y in 0..h * 2 {
        let (y0, y1, fy) = taps(y, h);
        for x in 0..w * 2 {
            let (x0, x1, fx) = taps(x, w);
            for c in 0..3 {
                let at = |xx: usize, yy: usize| image.rgba[(yy * w + xx) * 4 + c] as u32;
                let top = at(x0, y0) * (4 - fx) + at(x1, y0) * fx;
                let bottom = at(x0, y1) * (4 - fx) + at(x1, y1) * fx;
                rgba[(y * w * 2 + x) * 4 + c] = ((top * (4 - fy) + bottom * fy + 8) / 16) as u8;
            }
        }
    }
    Image { w: image.w * 2, h: image.h * 2, rgba }
}

fn read(image: &Image) -> windows::core::Result<Vec<Line>> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    let most = OcrEngine::MaxImageDimension()?;
    let grow = image.w * 2 <= most && image.h * 2 <= most;
    let big;
    let source = if grow {
        big = doubled(image);
        &big
    } else {
        image
    };
    let mut bgra = source.rgba.clone();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &writer.DetachBuffer()?,
        BitmapPixelFormat::Bgra8,
        source.w as i32,
        source.h as i32,
        BitmapAlphaMode::Ignore,
    )?;
    let lines = engine.RecognizeAsync(&bitmap)?.get()?.Lines()?;
    let k = if grow { 2.0 } else { 1.0 };
    let mut out = Vec::new();
    for i in 0..lines.Size()? {
        let line = lines.GetAt(i)?;
        let words = line.Words()?;
        // The line's box is the box around its words.
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, 0f32, 0f32);
        for j in 0..words.Size()? {
            let b = words.GetAt(j)?.BoundingRect()?;
            (left, top) = (left.min(b.X), top.min(b.Y));
            (right, bottom) = (right.max(b.X + b.Width), bottom.max(b.Y + b.Height));
        }
        let rect = Rect {
            x: (left / k) as i32,
            y: (top / k) as i32,
            w: ((right - left) / k).max(0.0) as i32,
            h: ((bottom - top) / k).max(0.0) as i32,
        };
        out.push(Line { text: line.Text()?.to_string(), rect });
    }
    Ok(out)
}

/// The text in the picture, line by line from the top, read on this computer by
/// Windows' own recogniser in the user's languages. `Err` when there is none
/// (`ocr_available`) or the picture is larger than it takes.
pub fn ocr(image: &Image) -> Result<Vec<Line>, String> {
    if image.w == 0 || image.h == 0 {
        return Ok(Vec::new());
    }
    mta(|| read(image)).map_err(|e| {
        let text = e.message();
        if text.is_empty() { "Windows has no text recognition for your languages installed.".to_string() } else { text }
    })
}

/// What the QR codes in the picture say, top to bottom.
pub fn qr(image: &Image) -> Vec<String> {
    let (w, h) = (image.w as usize, image.h as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let px = &image.rgba[(y * w + x) * 4..];
        ((px[0] as u32 * 299 + px[1] as u32 * 587 + px[2] as u32 * 114) / 1000) as u8
    });
    let mut found: Vec<((i32, i32), String)> = prepared
        .detect_grids()
        .into_iter()
        .filter_map(|grid| {
            let at = (grid.bounds[0].y, grid.bounds[0].x);
            grid.decode().ok().map(|(_, text)| (at, text))
        })
        .collect();
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, text)| text).collect()
}
