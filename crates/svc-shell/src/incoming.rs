// What arrives: the things a data object carries (the clipboard, a drop from
// another program), as `Item`s.
//
// Files that exist come as their paths. Files that do not exist yet (an
// attachment dragged or copied out of a mail client, a picture out of a
// browser) are offered as a list of names, `FileGroupDescriptorW`, and their
// bytes one by one, `FileContents`: those are written into a folder the caller
// names, under their own names.

use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};

use windows::core::w;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::Com::{
    IDataObject, IStream, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STREAM_SEEK_SET, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{OleGetClipboard, ReleaseStgMedium, CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, FILEDESCRIPTORW, HDROP};

use crate::{sta, Item};

/// `FILEDESCRIPTORW.dwFlags`: the attributes and the size are filled in.
const FD_ATTRIBUTES: u32 = 0x4;
const FD_FILESIZE: u32 = 0x40;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// "name.ext" in `dir`, or "name (2).ext"… when that is taken.
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let name = Path::new(name);
    let stem = name.file_stem().unwrap_or_default().to_string_lossy();
    let ext = name.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1u32..)
        .map(|n| dir.join(if n == 1 { format!("{stem}{ext}") } else { format!("{stem} ({n}){ext}") }))
        .find(|path| !path.exists())
        .expect("a free name")
}

/// A name another program gave, as a file name of ours: its last part, without
/// what Windows does not take in a name.
pub fn safe_name(name: &str) -> String {
    let last = name.rsplit(['\\', '/']).next().unwrap_or_default();
    let clean: String = last.chars().map(|c| if c.is_control() || r#"<>:"|?*"#.contains(c) { '_' } else { c }).collect();
    let clean = clean.trim().trim_end_matches(['.', ' ']);
    if clean.is_empty() || clean == "." || clean == ".." { "file".to_string() } else { clean.to_string() }
}

unsafe fn get(object: &IDataObject, format: u16, index: i32, tymed: u32) -> Option<STGMEDIUM> {
    let wanted = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: index, tymed };
    unsafe { object.GetData(&wanted).ok() }
}

unsafe fn global_bytes(memory: HGLOBAL) -> Vec<u8> {
    unsafe {
        let from = GlobalLock(memory) as *const u8;
        if from.is_null() {
            return Vec::new();
        }
        let out = std::slice::from_raw_parts(from, GlobalSize(memory)).to_vec();
        let _ = GlobalUnlock(memory);
        out
    }
}

/// A format that travels as one block of memory.
unsafe fn bytes(object: &IDataObject, format: u16) -> Option<Vec<u8>> {
    unsafe {
        let mut medium = get(object, format, -1, TYMED_HGLOBAL.0 as u32)?;
        let out = (medium.tymed == TYMED_HGLOBAL.0 as u32).then(|| global_bytes(medium.u.hGlobal));
        ReleaseStgMedium(&mut medium);
        out
    }
}

fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|u| *u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// CF_HDROP: files that exist, by their paths.
unsafe fn real_files(object: &IDataObject) -> Vec<PathBuf> {
    unsafe {
        let Some(mut medium) = get(object, CF_HDROP.0, -1, TYMED_HGLOBAL.0 as u32) else { return Vec::new() };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let files = (0..count)
            .map(|i| {
                let mut name = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                let len = DragQueryFileW(drop, i, Some(&mut name)) as usize;
                PathBuf::from(String::from_utf16_lossy(&name[..len]))
            })
            .collect();
        ReleaseStgMedium(&mut medium);
        files
    }
}

unsafe fn save_stream(stream: &IStream, to: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::create(to)?;
    let mut chunk = vec![0u8; 64 * 1024];
    unsafe {
        // Some sources hand the stream over at its end.
        let _ = stream.Seek(0, STREAM_SEEK_SET, None);
        loop {
            let mut got = 0u32;
            let read = stream.Read(chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, Some(&mut got));
            if read.is_err() {
                return Err(std::io::Error::other(read.message()));
            }
            if got == 0 {
                return Ok(());
            }
            file.write_all(&chunk[..got as usize])?;
        }
    }
}

/// `FileGroupDescriptorW` + `FileContents`: files that exist only as an offer
/// (a mail's attachments, a picture dragged out of a browser). Each is written
/// into `dir` under its own name.
// ponytail: a whole message dragged out of Outlook travels as a storage, not a
// stream, and is passed over; its attachments are taken. Add TYMED_ISTORAGE if asked for.
unsafe fn offered_files(object: &IDataObject, dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    unsafe {
        let Some(list) = bytes(object, RegisterClipboardFormatW(w!("FileGroupDescriptorW")) as u16) else { return out };
        let contents = RegisterClipboardFormatW(w!("FileContents")) as u16;
        let each = size_of::<FILEDESCRIPTORW>();
        let count = list.get(..4).map_or(0, |n| u32::from_le_bytes([n[0], n[1], n[2], n[3]]) as usize);
        for i in 0..count.min(list.len().saturating_sub(4) / each) {
            let about: FILEDESCRIPTORW = std::ptr::read_unaligned(list.as_ptr().add(4 + i * each).cast());
            // A folder of the offer is not a file; what is in it comes by its own entry.
            if about.dwFlags & FD_ATTRIBUTES != 0 && about.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                continue;
            }
            let name = about.cFileName;
            let name = String::from_utf16_lossy(&name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())]);
            let Some(mut medium) = get(object, contents, i as i32, (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) as u32) else { continue };
            if std::fs::create_dir_all(dir).is_err() {
                ReleaseStgMedium(&mut medium);
                break;
            }
            let to = free_name(dir, &safe_name(&name));
            let written = if medium.tymed == TYMED_ISTREAM.0 as u32 {
                match medium.u.pstm.as_ref() {
                    Some(stream) => save_stream(stream, &to),
                    None => Err(std::io::Error::other("no stream")),
                }
            } else if medium.tymed == TYMED_HGLOBAL.0 as u32 {
                let mut bytes = global_bytes(medium.u.hGlobal);
                // The block may be larger than the file.
                if about.dwFlags & FD_FILESIZE != 0 && about.nFileSizeHigh == 0 {
                    bytes.truncate(about.nFileSizeLow as usize);
                }
                std::fs::write(&to, bytes)
            } else {
                Err(std::io::Error::other("neither a stream nor memory"))
            };
            ReleaseStgMedium(&mut medium);
            match written {
                Ok(()) => out.push(to),
                Err(_) => {
                    let _ = std::fs::remove_file(&to);
                }
            }
        }
    }
    out
}

/// A picture that is nobody's file (a screenshot, "Copy image"), written into
/// `dir` as `name`.png, or .bmp when the source gave no PNG.
unsafe fn picture(object: &IDataObject, dir: &Path, name: &str) -> Option<PathBuf> {
    unsafe {
        let (bytes, ext) = match bytes(object, RegisterClipboardFormatW(w!("PNG")) as u16) {
            Some(png) => (png, "png"),
            None => {
                // ponytail: a bare bitmap is kept as the .bmp it is (a file header in
                // front of it); encode it as PNG if the size ever matters.
                let dib = bytes(object, CF_DIB.0)?;
                let field = |at: usize| dib.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) as usize;
                let (header, depth, packing, colours) = (field(0), field(14) & 0xffff, field(16), field(32));
                let masks = if header == 40 && packing == 3 { 12 } else { 0 };
                let palette = if colours != 0 { colours } else if depth <= 8 { 1 << depth } else { 0 };
                let pixels = 14 + header + masks + palette * 4;
                let mut bmp = Vec::with_capacity(14 + dib.len());
                bmp.extend_from_slice(b"BM");
                bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
                bmp.extend_from_slice(&[0; 4]);
                bmp.extend_from_slice(&(pixels as u32).to_le_bytes());
                bmp.extend_from_slice(&dib);
                (bmp, "bmp")
            }
        };
        std::fs::create_dir_all(dir).ok()?;
        let to = free_name(dir, &format!("{}.{ext}", safe_name(name)));
        std::fs::write(&to, bytes).ok()?;
        Some(to)
    }
}

/// What a data object carries, the first of: files that exist; files that are
/// only offered (written into `dir`); a web address; text; a picture (written
/// into `dir` as `picture_name`). On a thread where OLE is up.
///
/// Text comes before a picture: Word and Excel put a picture of the text
/// beside the text they copy.
pub fn incoming(object: &IDataObject, dir: &Path, picture_name: &str) -> Vec<Item> {
    unsafe {
        let files = real_files(object);
        if !files.is_empty() {
            return files.into_iter().map(Item::File).collect();
        }
        let offered = offered_files(object, dir);
        if !offered.is_empty() {
            return offered.into_iter().map(Item::File).collect();
        }
        if let Some(url) = bytes(object, RegisterClipboardFormatW(w!("UniformResourceLocatorW")) as u16) {
            let url = utf16(&url);
            if !url.trim().is_empty() {
                return vec![Item::Link(url.trim().to_string())];
            }
        }
        if let Some(text) = bytes(object, CF_UNICODETEXT.0) {
            let text = utf16(&text);
            if !text.trim().is_empty() {
                return vec![Item::Text(text)];
            }
        }
        picture(object, dir, picture_name).map(Item::File).into_iter().collect()
    }
}

/// What is on the clipboard now, as `incoming` reads it. The clipboard itself is not changed.
pub fn clipboard(dir: &Path, picture_name: &str) -> Vec<Item> {
    sta(|| unsafe { OleGetClipboard().map(|object| incoming(&object, dir, picture_name)).unwrap_or_default() })
}
