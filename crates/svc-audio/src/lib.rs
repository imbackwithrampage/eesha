// Audio — a shared service (docs/ARCHITECTURE.md §8). Not a feature: the
// features that need it list this crate in their own Cargo.toml. Refcounted:
// nothing here runs without a live subscriber, and the handle `subscribe`
// returns unsubscribes on drop.
//
// Core Audio endpoints: the level and mute of the default output and input,
// the device list, the default device. Reads and writes are plain calls on the
// caller's thread. `on_endpoint_change` is the part that runs: one MTA thread
// that owns the device enumerator and the volume callbacks, from the first
// `Sub` to the drop of the last. COM calls back on its own threads; those only
// push a message to that thread, which is where subscribers are called.

#![allow(non_snake_case)] // COM method names

use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{implement, interface, IUnknown, IUnknown_Vtbl, GUID, HRESULT, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, PKEY_AudioEndpoint_FormFactor,
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

type WinResult<T> = windows::core::Result<T>;

mod sessions;
pub use sessions::{on_sessions, process_path, sessions, set_session_mute, set_session_volume, Session, SessionSub};

mod loopback;
pub use loopback::{loopback, Loopback};

// ── The model ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Output: speakers, headphones.
    Render,
    /// Input: microphones.
    Capture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Console,
    Multimedia,
    Communications,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume {
    /// 0…1, what the Windows slider shows.
    pub scalar: f32,
    pub mute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// `IMMDevice::GetId`: the stable key for anything stored about a device.
    pub id: String,
    pub name: String,
    /// `EndpointFormFactor`: 1 speakers, 3 headphones, 4 microphone, 5 headset,
    /// 8 S/PDIF, 9 display audio, 10 unknown.
    pub form_factor: u32,
    /// The roles it is the default device for; empty for most.
    pub default_for: Vec<Role>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The level or mute of a default endpoint moved: the volume keys, a
    /// headset button, the Windows slider, another app. `own` = this process
    /// did it through `set_volume` or `set_mute`.
    Volume { flow: Flow, volume: Volume, own: bool },
    /// A device came or went, or a default changed (one call per burst). Read
    /// `devices` and `endpoint_volume` again.
    Devices,
}

impl Flow {
    fn raw(self) -> EDataFlow {
        if self == Flow::Render { eRender } else { eCapture }
    }
}

impl Role {
    fn raw(self) -> ERole {
        match self {
            Role::Console => eConsole,
            Role::Multimedia => eMultimedia,
            Role::Communications => eCommunications,
        }
    }
}

// ── Reads and writes (on the caller's thread) ─────────────────────────────────

/// Level and mute of the default device of `flow`. An error when there is none.
pub fn endpoint_volume(flow: Flow) -> Result<Volume, String> {
    let _com = Com::enter();
    (|| unsafe {
        let endpoint = default_endpoint(&enumerator()?, flow)?;
        Ok(Volume { scalar: endpoint.GetMasterVolumeLevelScalar()?, mute: endpoint.GetMute()?.as_bool() })
    })()
    .map_err(text)
}

/// Below this a level reads 0 % on the slider: an output is muted with it.
const SILENT: f32 = 0.005;

/// Output writes asked for and not yet applied, newest per device.
struct Writes {
    busy: bool,
    queue: Vec<(String, f32)>,
}

static WRITES: Mutex<Writes> = Mutex::new(Writes { busy: false, queue: Vec::new() });

/// Sets the level (0…1) of the default device of `flow`. For an output, a level
/// that reads 0 % also mutes it and a higher one opens it; the write goes to the
/// device that is the default now, even if another is by the time it is applied;
/// and writes that come faster than they are applied collapse to the newest.
pub fn set_volume(flow: Flow, scalar: f32) -> Result<(), String> {
    let _com = Com::enter();
    let scalar = scalar.clamp(0.0, 1.0);
    if flow == Flow::Capture {
        return (|| unsafe { default_endpoint(&enumerator()?, flow)?.SetMasterVolumeLevelScalar(scalar, &OWN) })().map_err(text);
    }
    let id = (|| unsafe { device_id(&enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole)?) })().map_err(text)?;
    {
        let mut w = WRITES.lock().unwrap();
        w.queue.retain(|q| q.0 != id);
        w.queue.push((id, scalar));
        // Another call is applying writes: it will take this one too.
        if std::mem::replace(&mut w.busy, true) {
            return Ok(());
        }
    }
    let mut result = Ok(());
    loop {
        let next = {
            let mut w = WRITES.lock().unwrap();
            if w.queue.is_empty() {
                w.busy = false;
                return result;
            }
            w.queue.remove(0)
        };
        result = apply_level(&next.0, next.1);
    }
}

/// Mute first when going silent, open last when coming back: never a blip.
fn apply_level(id: &str, scalar: f32) -> Result<(), String> {
    (|| unsafe {
        let endpoint = endpoint_of(&enumerator()?, id)?;
        let (silent, muted) = (scalar < SILENT, endpoint.GetMute()?.as_bool());
        if silent && !muted {
            endpoint.SetMute(true, &OWN)?;
        }
        endpoint.SetMasterVolumeLevelScalar(scalar, &OWN)?;
        if !silent && muted {
            endpoint.SetMute(false, &OWN)?;
        }
        Ok(())
    })()
    .map_err(text)
}

pub fn set_mute(flow: Flow, mute: bool) -> Result<(), String> {
    let _com = Com::enter();
    (|| unsafe { default_endpoint(&enumerator()?, flow)?.SetMute(mute, &OWN) })().map_err(text)
}

/// Level and mute of the device `id` (from `devices`), default or not.
pub fn device_volume(id: &str) -> Result<Volume, String> {
    let _com = Com::enter();
    (|| unsafe {
        let endpoint = endpoint_of(&enumerator()?, id)?;
        Ok(Volume { scalar: endpoint.GetMasterVolumeLevelScalar()?, mute: endpoint.GetMute()?.as_bool() })
    })()
    .map_err(text)
}

/// Sets the level (0…1) of the device `id`.
pub fn set_device_volume(id: &str, scalar: f32) -> Result<(), String> {
    let _com = Com::enter();
    (|| unsafe { endpoint_of(&enumerator()?, id)?.SetMasterVolumeLevelScalar(scalar.clamp(0.0, 1.0), &OWN) })()
        .map_err(text)
}

/// Mutes or unmutes the device `id`.
pub fn set_device_mute(id: &str, mute: bool) -> Result<(), String> {
    let _com = Com::enter();
    (|| unsafe { endpoint_of(&enumerator()?, id)?.SetMute(mute, &OWN) })().map_err(text)
}

/// The active devices of `flow`: the defaults first, then by name, then by id.
pub fn devices(flow: Flow) -> Result<Vec<Device>, String> {
    let _com = Com::enter();
    (|| unsafe {
        let enumerator = enumerator()?;
        let defaults = [Role::Console, Role::Multimedia, Role::Communications].map(|role| {
            let id = enumerator.GetDefaultAudioEndpoint(flow.raw(), role.raw()).and_then(|d| device_id(&d));
            (role, id.unwrap_or_default())
        });
        let list = enumerator.EnumAudioEndpoints(flow.raw(), DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..list.GetCount()? {
            let device = list.Item(i)?;
            let id = device_id(&device)?;
            let props = device.OpenPropertyStore(STGM_READ)?;
            let name = props.GetValue(&PKEY_Device_FriendlyName).map(|v| v.to_string()).unwrap_or_default();
            let form_factor = props.GetValue(&PKEY_AudioEndpoint_FormFactor).ok().and_then(|v| u32::try_from(&v).ok());
            out.push(Device {
                default_for: defaults.iter().filter(|d| d.1 == id).map(|d| d.0).collect(),
                id,
                name,
                form_factor: form_factor.unwrap_or(10),
            });
        }
        out.sort_by(|a, b| (a.default_for.is_empty(), &a.name, &a.id).cmp(&(b.default_for.is_empty(), &b.name, &b.id)));
        Ok(out)
    })()
    .map_err(text)
}

/// Whether `set_default` can work on this build of Windows. The interface
/// behind it is undocumented: hide the picker when this is false.
pub fn can_set_default() -> bool {
    let _com = Com::enter();
    policy_config().is_ok()
}

/// Makes the device `id` (from `devices`) the default for `role`.
pub fn set_default(id: &str, role: Role) -> Result<(), String> {
    let _com = Com::enter();
    let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
    (|| unsafe { policy_config()?.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role.raw()).ok() })().map_err(text)
}

/// The event context of our own writes, so that `Change::Volume` can say `own`.
const OWN: GUID = GUID::from_u128(0x6e6f7463_682d_4a75_8d69_6f2d766f6c00);

/// COM on this thread for the length of one call. Declared first, so that it
/// is dropped after every interface the call made.
struct Com(bool);

impl Com {
    fn enter() -> Self {
        // S_FALSE (already entered) has to be left too; a refusal (the thread
        // is an STA) does not, and these interfaces work from either.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn text(err: windows::core::Error) -> String {
    err.message()
}

fn enumerator() -> WinResult<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

/// The console default: the device the volume keys move.
fn default_endpoint(enumerator: &IMMDeviceEnumerator, flow: Flow) -> WinResult<IAudioEndpointVolume> {
    unsafe { enumerator.GetDefaultAudioEndpoint(flow.raw(), eConsole)?.Activate(CLSCTX_ALL, None) }
}

fn endpoint_of(enumerator: &IMMDeviceEnumerator, id: &str) -> WinResult<IAudioEndpointVolume> {
    let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
    unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr()))?.Activate(CLSCTX_ALL, None) }
}

fn device_id(device: &IMMDevice) -> WinResult<String> {
    unsafe {
        let raw = device.GetId()?;
        let id = raw.to_string().unwrap_or_default();
        CoTaskMemFree(Some(raw.0 as *const _));
        Ok(id)
    }
}

// `IPolicyConfig`, the interface the Sound control panel sets the default
// device with. Undocumented but unchanged since Windows 7; `can_set_default`
// is the probe. Ten methods come before the one used: they are never called,
// so only their slots are declared.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn GetMixFormat(&self) -> HRESULT;
    fn GetDeviceFormat(&self) -> HRESULT;
    fn ResetDeviceFormat(&self) -> HRESULT;
    fn SetDeviceFormat(&self) -> HRESULT;
    fn GetProcessingPeriod(&self) -> HRESULT;
    fn SetProcessingPeriod(&self) -> HRESULT;
    fn GetShareMode(&self) -> HRESULT;
    fn SetShareMode(&self) -> HRESULT;
    fn GetPropertyValue(&self) -> HRESULT;
    fn SetPropertyValue(&self) -> HRESULT;
    fn SetDefaultEndpoint(&self, device_id: PCWSTR, role: ERole) -> HRESULT;
}

const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

fn policy_config() -> WinResult<IPolicyConfig> {
    unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }
}

// ── Subscribing ───────────────────────────────────────────────────────────────

/// A live subscription. Dropping it unsubscribes; dropping the last one ends
/// the audio thread and unregisters every callback.
#[must_use = "dropping the handle unsubscribes"]
pub struct Sub(u64);

/// Called on the audio thread for every change of the default endpoints and of
/// the device list. Keep it short.
pub fn on_endpoint_change(f: impl Fn(&Change) + Send + Sync + 'static) -> Sub {
    let mut hub = HUB.lock().unwrap();
    hub.next += 1;
    let id = hub.next;
    hub.subs.push((id, Arc::new(f)));
    if hub.thread.is_none() {
        let (tx, rx) = mpsc::channel();
        let callbacks = tx.clone();
        hub.thread = Some((tx, std::thread::spawn(move || run(rx, callbacks))));
    }
    Sub(id)
}

impl Drop for Sub {
    fn drop(&mut self) {
        let ended = {
            let mut hub = HUB.lock().unwrap();
            hub.subs.retain(|s| s.0 != self.0);
            if hub.subs.is_empty() { hub.thread.take() } else { None }
        };
        let Some((tx, handle)) = ended else { return };
        let _ = tx.send(Msg::Stop);
        // Dropped from inside a callback: the thread is this one and ends on its own.
        if handle.thread().id() != std::thread::current().id() {
            let _ = handle.join();
        }
    }
}

type Handler = Arc<dyn Fn(&Change) + Send + Sync>;

struct Hub {
    next: u64,
    subs: Vec<(u64, Handler)>,
    thread: Option<(mpsc::Sender<Msg>, JoinHandle<()>)>,
}

static HUB: Mutex<Hub> = Mutex::new(Hub { next: 0, subs: Vec::new(), thread: None });

fn emit(change: Change) {
    // Copied out first: a callback may subscribe or drop a `Sub`.
    let subs: Vec<Handler> = HUB.lock().unwrap().subs.iter().map(|s| s.1.clone()).collect();
    for f in subs {
        f(&change);
    }
}

// ── The audio thread ──────────────────────────────────────────────────────────

enum Msg {
    Volume(Flow, Volume, bool),
    Devices,
    Stop,
}

/// A device switch calls back once per role and per flow: one pass after the burst.
const SETTLE: Duration = Duration::from_millis(200);

type Bound = Option<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)>;

fn run(rx: mpsc::Receiver<Msg>, tx: mpsc::Sender<Msg>) {
    let _com = Com::enter();
    let Ok(enumerator) = enumerator() else { return };
    let devices: IMMNotificationClient = DeviceEvents(tx.clone()).into();
    unsafe {
        let _ = enumerator.RegisterEndpointNotificationCallback(&devices);
    }
    let bind_all = || [Flow::Render, Flow::Capture].map(|flow| bind(&enumerator, flow, &tx));
    let mut bound = bind_all();
    let mut rebind_at: Option<Instant> = None;
    loop {
        let msg = match rebind_at {
            None => rx.recv().ok(),
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(msg) => Some(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    rebind_at = None;
                    unbind(bound);
                    bound = bind_all();
                    emit(Change::Devices);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => None,
            },
        };
        match msg {
            Some(Msg::Volume(flow, volume, own)) => emit(Change::Volume { flow, volume, own }),
            Some(Msg::Devices) => {
                rebind_at.get_or_insert(Instant::now() + SETTLE);
            }
            Some(Msg::Stop) | None => break,
        }
    }
    unbind(bound);
    unsafe {
        let _ = enumerator.UnregisterEndpointNotificationCallback(&devices);
    }
}

/// Listens to the default device of `flow`. None when there is no such device.
fn bind(enumerator: &IMMDeviceEnumerator, flow: Flow, tx: &mpsc::Sender<Msg>) -> Bound {
    let endpoint = default_endpoint(enumerator, flow).ok()?;
    let callback: IAudioEndpointVolumeCallback = VolumeEvents { flow, tx: tx.clone() }.into();
    unsafe { endpoint.RegisterControlChangeNotify(&callback).ok()? };
    Some((endpoint, callback))
}

fn unbind(bound: [Bound; 2]) {
    for (endpoint, callback) in bound.into_iter().flatten() {
        unsafe {
            let _ = endpoint.UnregisterControlChangeNotify(&callback);
        }
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeEvents {
    flow: Flow,
    tx: mpsc::Sender<Msg>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeEvents_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> WinResult<()> {
        if let Some(data) = unsafe { data.as_ref() } {
            let volume = Volume { scalar: data.fMasterVolume, mute: data.bMuted.as_bool() };
            let _ = self.tx.send(Msg::Volume(self.flow, volume, data.guidEventContext == OWN));
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceEvents(mpsc::Sender<Msg>);

impl DeviceEvents_Impl {
    fn changed(&self) -> WinResult<()> {
        let _ = self.0.send(Msg::Devices);
        Ok(())
    }
}

impl IMMNotificationClient_Impl for DeviceEvents_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> WinResult<()> {
        self.changed()
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> WinResult<()> {
        self.changed()
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> WinResult<()> {
        self.changed()
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> WinResult<()> {
        self.changed()
    }
    // Fires for every property of every device, many times a second while one is set up.
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> WinResult<()> {
        Ok(())
    }
}
