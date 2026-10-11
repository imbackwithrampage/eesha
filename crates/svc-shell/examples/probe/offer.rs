// A data object as a mail client makes one for its attachments: the names in
// `FileGroupDescriptorW`, and each file's bytes under `FileContents` at the
// file's own index (the shell's ready-made data object does not tell the
// indexes apart, so the probe brings its own). The first file comes as a
// stream, the others as a block of memory larger than the file, the two ways
// a source may answer.

use std::mem::ManuallyDrop;

use windows::core::{implement, w, Ref, Result as WinResult, BOOL, HRESULT};
use windows::Win32::Foundation::{DATA_S_SAMEFORMATETC, DV_E_FORMATETC, E_NOTIMPL, OLE_E_ADVISENOTSUPPORTED, S_OK};
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DATADIR_GET, DVASPECT_CONTENT, FORMATETC,
    STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Shell::{SHCreateMemStream, SHCreateStdEnumFmtEtc, FILEDESCRIPTORW};

/// A block of memory holding `bytes`, as a medium the receiver frees.
pub unsafe fn global(bytes: &[u8]) -> STGMEDIUM {
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).expect("memory");
        let to = GlobalLock(memory) as *mut u8;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), to, bytes.len());
        let _ = GlobalUnlock(memory);
        STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: memory }, pUnkForRelease: ManuallyDrop::new(None) }
    }
}

#[implement(IDataObject)]
struct Offer {
    files: Vec<(String, Vec<u8>)>,
    descriptor: u16,
    contents: u16,
}

impl Offer {
    fn names(&self) -> Vec<u8> {
        let mut list = (self.files.len() as u32).to_le_bytes().to_vec();
        for (name, bytes) in &self.files {
            let mut written = [0u16; 260];
            for (to, from) in written.iter_mut().zip(name.encode_utf16()) {
                *to = from;
            }
            // FD_FILESIZE: the size is filled in.
            let about = FILEDESCRIPTORW { dwFlags: 0x40, nFileSizeLow: bytes.len() as u32, cFileName: written, ..Default::default() };
            list.extend_from_slice(unsafe { std::slice::from_raw_parts(&about as *const _ as *const u8, size_of::<FILEDESCRIPTORW>()) });
        }
        list
    }
}

#[allow(non_snake_case)]
impl IDataObject_Impl for Offer_Impl {
    fn GetData(&self, wanted: *const FORMATETC) -> WinResult<STGMEDIUM> {
        let wanted = unsafe { &*wanted };
        if wanted.cfFormat == self.descriptor && wanted.tymed & TYMED_HGLOBAL.0 as u32 != 0 {
            return Ok(unsafe { global(&self.names()) });
        }
        if wanted.cfFormat == self.contents {
            let Some((_, bytes)) = usize::try_from(wanted.lindex).ok().and_then(|i| self.files.get(i)) else { return Err(DV_E_FORMATETC.into()) };
            if wanted.lindex == 0 && wanted.tymed & TYMED_ISTREAM.0 as u32 != 0 {
                let stream = unsafe { SHCreateMemStream(Some(bytes)) };
                return Ok(STGMEDIUM { tymed: TYMED_ISTREAM.0 as u32, u: STGMEDIUM_0 { pstm: ManuallyDrop::new(stream) }, pUnkForRelease: ManuallyDrop::new(None) });
            }
            if wanted.tymed & TYMED_HGLOBAL.0 as u32 != 0 {
                let mut padded = bytes.clone();
                padded.extend_from_slice(&[0xEE; 300]);
                return Ok(unsafe { global(&padded) });
            }
        }
        Err(DV_E_FORMATETC.into())
    }
    fn QueryGetData(&self, wanted: *const FORMATETC) -> HRESULT {
        let format = unsafe { (*wanted).cfFormat };
        if format == self.descriptor || format == self.contents { S_OK } else { DV_E_FORMATETC }
    }
    fn EnumFormatEtc(&self, direction: u32) -> WinResult<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let one = |format: u16, tymed: i32| FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: tymed as u32 };
        unsafe { SHCreateStdEnumFmtEtc(&[one(self.descriptor, TYMED_HGLOBAL.0), one(self.contents, TYMED_ISTREAM.0 | TYMED_HGLOBAL.0)]) }
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, out: *mut FORMATETC) -> HRESULT {
        unsafe { (*out).ptd = std::ptr::null_mut() };
        DATA_S_SAMEFORMATETC
    }
    fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> WinResult<()> {
        Err(E_NOTIMPL.into())
    }
    fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<'_, IAdviseSink>) -> WinResult<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn DUnadvise(&self, _: u32) -> WinResult<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
    fn EnumDAdvise(&self) -> WinResult<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

/// The offer of `files`: (name, bytes).
pub fn offer(files: &[(&str, &[u8])]) -> IDataObject {
    let (descriptor, contents) = unsafe { (RegisterClipboardFormatW(w!("FileGroupDescriptorW")) as u16, RegisterClipboardFormatW(w!("FileContents")) as u16) };
    Offer { files: files.iter().map(|(name, bytes)| (name.to_string(), bytes.to_vec())).collect(), descriptor, contents }.into()
}
