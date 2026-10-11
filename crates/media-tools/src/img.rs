// Pictures: read and write through Windows' imaging component (WIC), resize with its Fant scaler,
// a text watermark drawn with GDI, and the one-page PDF written by hand around a JPEG stream.
// Metadata is never copied to the output: the pixels are written to a fresh file.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use svc_capture::Image;
use windows::core::{w, Interface, HSTRING, PWSTR};
use windows::Win32::Foundation::{COLORREF, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{IPropertyBag2, PROPBAG2};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::VARIANT;

/// What WIC and the rest of this crate need on every thread that works.
pub fn com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Jpeg,
    Png,
    Pdf,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Pdf => "pdf",
        }
    }
}

fn e(err: windows::core::Error) -> String {
    err.message()
}

fn factory() -> Result<IWICImagingFactory, String> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(e) }
}

fn open(f: &IWICImagingFactory, path: &Path) -> Result<IWICBitmapDecoder, String> {
    unsafe {
        f.CreateDecoderFromFilename(&HSTRING::from(path.to_string_lossy().as_ref()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .map_err(|_| "This file is not a picture Windows can read.".to_string())
    }
}

pub fn size_of_file(path: &Path) -> Result<(u32, u32), String> {
    unsafe {
        let f = factory()?;
        let (mut w, mut h) = (0, 0);
        open(&f, path)?.GetFrame(0).map_err(e)?.GetSize(&mut w, &mut h).map_err(e)?;
        Ok((w, h))
    }
}

/// Frames in a file (a GIF has several).
pub fn frame_count(path: &Path) -> Result<u32, String> {
    unsafe { open(&factory()?, path)?.GetFrameCount().map_err(e) }
}

/// Reads frame `n` as RGBA; `plan` is given its size and answers the size to decode it at (the scaler does the work).
/// 20 000 px a side and 8192 x 8192 pixels are the limits, as upstream.
pub fn load_frame(path: &Path, n: u32, plan: &dyn Fn(u32, u32) -> (u32, u32)) -> Result<Image, String> {
    unsafe {
        let f = factory()?;
        let frame = open(&f, path)?.GetFrame(n).map_err(e)?;
        let (mut w, mut h) = (0, 0);
        frame.GetSize(&mut w, &mut h).map_err(e)?;
        let (tw, th) = plan(w, h);
        if w.max(h) > 20_000 || (tw as u64) * (th as u64) > 8192 * 8192 || tw == 0 || th == 0 {
            return Err("The picture is too large.".into());
        }
        let conv = f.CreateFormatConverter().map_err(e)?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).map_err(e)?;
        let src: IWICBitmapSource = if (tw, th) != (w, h) {
            let s = f.CreateBitmapScaler().map_err(e)?;
            s.Initialize(&conv, tw, th, WICBitmapInterpolationModeFant).map_err(e)?;
            s.cast().map_err(e)?
        } else {
            conv.cast().map_err(e)?
        };
        let mut rgba = vec![0u8; tw as usize * th as usize * 4];
        src.CopyPixels(std::ptr::null(), tw * 4, &mut rgba).map_err(e)?;
        Ok(Image { w: tw, h: th, rgba })
    }
}

pub fn load(path: &Path, plan: &dyn Fn(u32, u32) -> (u32, u32)) -> Result<Image, String> {
    load_frame(path, 0, plan)
}

/// Lays transparent pixels over a flat colour (white, black).
pub fn flatten(img: &mut Image, bg: [u8; 3]) {
    for p in img.rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + bg[c] as u32 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
}

/// Writes the picture to `path` (a new file; the caller picks a free name).
pub fn save(img: &Image, fmt: Format, quality: f32, path: &Path) -> Result<(), String> {
    if fmt != Format::Pdf {
        return encode(img, fmt, quality, path);
    }
    let tmp = path.with_extension("jpg.part");
    encode(img, Format::Jpeg, quality, &tmp)?;
    let jpeg = std::fs::read(&tmp).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(path, pdf(&jpeg?, img.w, img.h)).map_err(|e| e.to_string())
}

fn encode(img: &Image, fmt: Format, quality: f32, path: &Path) -> Result<(), String> {
    unsafe {
        let f = factory()?;
        let stream = f.CreateStream().map_err(e)?;
        stream.InitializeFromFilename(&HSTRING::from(path.to_string_lossy().as_ref()), GENERIC_WRITE.0).map_err(e)?;
        let jpeg = fmt == Format::Jpeg;
        let enc = f.CreateEncoder(if jpeg { &GUID_ContainerFormatJpeg } else { &GUID_ContainerFormatPng }, std::ptr::null()).map_err(e)?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).map_err(e)?;
        let (mut frame, mut bag): (Option<IWICBitmapFrameEncode>, Option<IPropertyBag2>) = (None, None);
        enc.CreateNewFrame(&mut frame, &mut bag).map_err(e)?;
        let frame = frame.ok_or("no frame")?;
        if let (true, Some(bag)) = (jpeg, bag.as_ref()) {
            let mut name: Vec<u16> = "ImageQuality\0".encode_utf16().collect();
            let prop = PROPBAG2 { pstrName: PWSTR(name.as_mut_ptr()), ..Default::default() };
            bag.Write(1, &prop, &VARIANT::from(quality.clamp(0.05, 1.0))).map_err(e)?;
        }
        frame.Initialize(bag.as_ref()).map_err(e)?;
        frame.SetSize(img.w, img.h).map_err(e)?;
        let mut pf = if jpeg { GUID_WICPixelFormat24bppBGR } else { GUID_WICPixelFormat32bppBGRA };
        frame.SetPixelFormat(&mut pf).map_err(e)?;
        let (buf, stride) = if jpeg {
            (img.rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect::<Vec<u8>>(), img.w * 3)
        } else {
            (img.rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect::<Vec<u8>>(), img.w * 4)
        };
        frame.WritePixels(img.h, stride, &buf).map_err(e)?;
        frame.Commit().map_err(e)?;
        enc.Commit().map_err(e)?;
        Ok(())
    }
}

/// One page the size of the picture (96 dpi to points), the JPEG stream embedded as it is.
fn pdf(jpeg: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (pw, ph) = (w as f32 * 0.75, h as f32 * 0.75);
    let content = format!("q {pw} 0 0 {ph} 0 0 cm /Im0 Do Q");
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut at = vec![];
    let mut obj = |out: &mut Vec<u8>, body: Vec<u8>| {
        at.push(out.len());
        out.extend(format!("{} 0 obj\n", at.len()).as_bytes());
        out.extend(body);
        out.extend(b"\nendobj\n");
    };
    obj(&mut out, b"<</Type/Catalog/Pages 2 0 R>>".to_vec());
    obj(&mut out, b"<</Type/Pages/Kids[3 0 R]/Count 1>>".to_vec());
    obj(&mut out, format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 {pw} {ph}]/Resources<</XObject<</Im0 4 0 R>>>>/Contents 5 0 R>>").into_bytes());
    let mut image = format!("<</Type/XObject/Subtype/Image/Width {w}/Height {h}/ColorSpace/DeviceRGB/BitsPerComponent 8/Filter/DCTDecode/Length {}>>\nstream\n", jpeg.len()).into_bytes();
    image.extend(jpeg);
    image.extend(b"\nendstream");
    obj(&mut out, image);
    obj(&mut out, format!("<</Length {}>>\nstream\n{content}\nendstream", content.len()).into_bytes());
    let xref = out.len();
    out.extend(b"xref\n0 6\n0000000000 65535 f \n");
    for a in &at {
        out.extend(format!("{a:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<</Size 6/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

/// Text in the corner `pos` (0 top left, 1 top right, 2 bottom left, 3 bottom right, 4 centre): white over a soft
/// shadow, drawn by GDI as a coverage mask and blended with `opacity`. `scale` is a share of the picture's width.
pub fn watermark(img: &mut Image, text: &str, pos: u8, opacity: f32, margin: i32, scale: f32) {
    if text.trim().is_empty() {
        return;
    }
    unsafe {
        let dc = CreateCompatibleDC(None);
        let px = ((img.w as f32 * scale * 0.15) as i32).clamp(12, 400);
        let font = CreateFontW(-px, 0, 0, 0, 600, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY, DEFAULT_PITCH.0 as u32, w!("Segoe UI"));
        let old = SelectObject(dc, font.into());
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut sz = windows::Win32::Foundation::SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &wide, &mut sz);
        let (tw, th) = (sz.cx + 4, sz.cy + 4);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: tw, biHeight: -th, biPlanes: 1, biBitCount: 32, ..Default::default() },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        if let Ok(dib) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            if !bits.is_null() {
                let prev = SelectObject(dc, dib.into());
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00FF_FFFF));
                let _ = TextOutW(dc, 2, 2, &wide);
                let _ = GdiFlush();
                let mask = std::slice::from_raw_parts(bits as *const u8, tw as usize * th as usize * 4);
                let (iw, ih) = (img.w as i32, img.h as i32);
                let (x0, y0) = match pos {
                    0 => (margin, margin),
                    1 => (iw - tw - margin, margin),
                    2 => (margin, ih - th - margin),
                    3 => (iw - tw - margin, ih - th - margin),
                    _ => ((iw - tw) / 2, (ih - th) / 2),
                };
                // The shadow first (two pixels down and right), then the text.
                for (d, color, k) in [(2, 0f32, 0.6f32), (0, 255.0, 1.0)] {
                    for y in 0..th {
                        for x in 0..tw {
                            let cov = mask[(y * tw + x) as usize * 4 + 1] as f32 / 255.0 * opacity * k;
                            let (px, py) = (x0 + x + d, y0 + y + d);
                            if cov <= 0.0 || px < 0 || py < 0 || px >= iw || py >= ih {
                                continue;
                            }
                            let at = (py * iw + px) as usize * 4;
                            for c in 0..3 {
                                let was = img.rgba[at + c] as f32;
                                img.rgba[at + c] = (was + (color - was) * cov) as u8;
                            }
                        }
                    }
                }
                SelectObject(dc, prev);
                let _ = DeleteObject(dib.into());
            }
        }
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);
    }
}
