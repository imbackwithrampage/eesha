// Windows Graphics Capture: a display or one window as frames, for whoever
// films the screen (the recorder). `session` opens it and `Session::frame`
// hands over the next picture, on the caller's thread and on the caller's
// time; dropping the `Session` ends it. Windows sends a frame only when the
// picture changed, so a still screen costs nothing.
//
// A frame is a texture on the session's own Direct3D device (BGRA, the size the
// source had when the session began: a window that grows is cut, one that
// shrinks leaves the rest as it was). It belongs to Windows again when the
// `Frame` is dropped: copy what is to be kept. Times are on the performance
// counter's clock, in 100 ns.
//
// Windows that are excluded from capture (`exclude_from_capture`) are not in
// the frames. Windows 10 2004 and later; the border can only be taken away on
// Windows 11.

use std::ffi::c_void;
use std::sync::mpsc::{sync_channel, Receiver};
use std::time::Duration;

use windows::core::{factory, IInspectable, Interface, Result as WinResult};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem,
    GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

use crate::Rect;

/// What is filmed.
#[derive(Debug, Clone, Copy)]
pub enum Source {
    /// The display that holds this rectangle (physical pixels of the desktop), whole.
    Display(Rect),
    /// One window, from its own buffer: what covers it is not in the frames.
    Window(isize),
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// The pointer is in the frames.
    pub pointer: bool,
    /// Windows draws its border around what is filmed (it always does on Windows 10).
    pub border: bool,
}

pub struct Session {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    arrived: Receiver<()>,
    /// The window that is filmed, when it is one.
    window: Option<isize>,
    area: Rect,
}

/// One picture. Windows takes the texture back when this is dropped.
pub struct Frame {
    pub texture: ID3D11Texture2D,
    /// When Windows drew it: the performance counter, in 100 ns.
    pub time: i64,
    held: Direct3D11CaptureFrame,
}

impl Drop for Frame {
    fn drop(&mut self) {
        let _ = self.held.Close();
    }
}

/// Starts filming. The first frame is on its way at once. The session may be
/// used and dropped on any thread.
pub fn session(source: Source, options: Options) -> Result<Session, String> {
    crate::mta(|| open(source, options)).map_err(|e| e.message())
}

/// A device that draws on the graphics card, or in software where there is
/// none. Media Foundation encodes from threads of its own on the same device.
fn device() -> WinResult<(ID3D11Device, ID3D11DeviceContext)> {
    let make = |kind: D3D_DRIVER_TYPE, flags: D3D11_CREATE_DEVICE_FLAG| unsafe {
        let (mut device, mut context) = (None, None);
        D3D11CreateDevice(None, kind, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        device.zip(context).ok_or_else(windows::core::Error::empty)
    };
    let (device, context) = make(D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT)
        .or_else(|_| make(D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT))?;
    unsafe {
        let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
    }
    Ok((device, context))
}

fn open(source: Source, options: Options) -> WinResult<Session> {
    let _aware = crate::Aware::new();
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let (item, at): (GraphicsCaptureItem, (i32, i32)) = unsafe {
        match source {
            Source::Display(rect) => {
                let wanted = RECT { left: rect.x, top: rect.y, right: rect.x + rect.w, bottom: rect.y + rect.h };
                let monitor = MonitorFromRect(&wanted, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(monitor, &mut info);
                (interop.CreateForMonitor(monitor)?, (info.rcMonitor.left, info.rcMonitor.top))
            }
            Source::Window(hwnd) => (interop.CreateForWindow(HWND(hwnd as *mut c_void))?, (0, 0)),
        }
    };
    let size = item.Size()?;
    let (device, context) = device()?;
    let shared: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)? }.cast()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&shared, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;

    // Windows says on a thread of its own that a frame is there; one mark is enough.
    let (mark, arrived) = sync_channel(1);
    pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |_, _| {
        let _ = mark.try_send(());
        Ok(())
    }))?;

    let session = pool.CreateCaptureSession(&item)?;
    // Both are refused on a Windows that does not know them: it films as it can.
    let _ = session.SetIsCursorCaptureEnabled(options.pointer);
    if !options.border {
        let _ = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless).and_then(|asked| asked.get());
        let _ = session.SetIsBorderRequired(false);
    }
    session.StartCapture()?;
    let window = match source {
        Source::Window(hwnd) => Some(hwnd),
        Source::Display(_) => None,
    };
    Ok(Session { device, context, pool, session, arrived, window, area: Rect { x: at.0, y: at.1, w: size.Width, h: size.Height } })
}

impl Session {
    /// The device the frames' textures live on.
    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    /// The size of every frame, and for a display where its top-left corner is
    /// on the desktop (a window's is at 0, 0).
    pub fn area(&self) -> Rect {
        self.area
    }

    /// The window that is filmed was closed: no frame will come any more.
    /// Asked of the window itself: the item's own `Closed` is only said to a
    /// thread with a dispatcher queue, which a caller's thread is not.
    pub fn closed(&self) -> bool {
        self.window.is_some_and(|hwnd| !unsafe { IsWindow(Some(HWND(hwnd as *mut c_void))) }.as_bool())
    }

    /// The next frame, waiting `wait` at most for one. None while the picture stands still.
    pub fn frame(&self, wait: Duration) -> Option<Frame> {
        let next = || self.pool.TryGetNextFrame().ok();
        let held = next().or_else(|| {
            let _ = self.arrived.recv_timeout(wait);
            next()
        })?;
        let time = held.SystemRelativeTime().ok()?.Duration;
        let texture = unsafe { held.Surface().ok()?.cast::<IDirect3DDxgiInterfaceAccess>().ok()?.GetInterface::<ID3D11Texture2D>().ok()? };
        Some(Frame { texture, time, held })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
