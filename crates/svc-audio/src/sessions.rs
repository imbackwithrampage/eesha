// Audio sessions (docs/ARCHITECTURE.md §8): what each app plays on an output,
// with a level and a mute of its own — the rows of the Windows volume mixer.
// Reads and writes are plain calls on the caller's thread. `on_sessions` is the
// part that runs: one MTA thread of its own, from the first `SessionSub` to the
// drop of the last, holding a session manager per active output and a listener
// per session. COM calls back on its own threads; those only push a message to
// that thread, which is where subscribers are called.

use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{implement, Interface, Ref, BOOL, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY, S_OK};
use windows::Win32::Media::Audio::{
    eRender, AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateActive, AudioSessionStateExpired,
    EDataFlow, ERole, IAudioSessionControl, IAudioSessionControl2, IAudioSessionEnumerator, IAudioSessionEvents,
    IAudioSessionEvents_Impl, IAudioSessionManager2, IAudioSessionNotification, IAudioSessionNotification_Impl,
    IMMDevice, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, ISimpleAudioVolume,
    DEVICE_STATE, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::{device_id, enumerator, text, Com, WinResult, OWN};

// ── The model ─────────────────────────────────────────────────────────────────

/// One stream of sound from a process to an output. An app may hold several:
/// one per output it plays on, and one per helper process.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// The process that plays; 0 for the system sounds.
    pub pid: u32,
    /// Its executable, as a full path. Empty for the system sounds and for a
    /// process that may not be asked.
    pub exe: String,
    /// The name the app gave its session. Most give none.
    pub name: String,
    /// The icon the app gave its session (a path, or `file,-resource`). Most give none.
    pub icon: String,
    /// 0…1, the app's own level under the output's.
    pub volume: f32,
    pub mute: bool,
    /// Sound is coming out of it right now.
    pub active: bool,
    /// The session of Windows' own sounds.
    pub system: bool,
    /// The output it plays on (`Device::id`).
    pub device: String,
}

// ── Reads and writes (on the caller's thread) ─────────────────────────────────

/// The sessions on the output `device` (a `Device::id`), or with `None` on
/// every active output: an app can be sent to another output than the default.
/// Sessions whose process has ended are left out.
pub fn sessions(device: Option<&str>) -> Result<Vec<Session>, String> {
    let _com = Com::enter();
    (|| {
        let mut out = Vec::new();
        for (id, device) in outputs(&enumerator()?, device)? {
            // An output on its way out answers nothing: it has no sessions.
            let Ok((_, controls)) = manager(&device).and_then(|m| controls(&m)) else { continue };
            out.extend(controls.iter().filter_map(|control| read(control, &id)));
        }
        Ok(out)
    })()
    .map_err(text)
}

/// Sets the level (0…1) of every session of the processes `pids`, on every
/// output. Process 0 is the system sounds.
pub fn set_session_volume(pids: &[u32], scalar: f32) -> Result<(), String> {
    write(pids, |level| unsafe { level.SetMasterVolume(scalar.clamp(0.0, 1.0), &OWN) })
}

/// Mutes or unmutes every session of the processes `pids`; the level is kept.
pub fn set_session_mute(pids: &[u32], mute: bool) -> Result<(), String> {
    write(pids, |level| unsafe { level.SetMute(mute, &OWN) })
}

/// The executable of a running process as a full path. None for a process
/// that has ended or may not be asked (a protected one).
pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let read = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        read.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn write(pids: &[u32], set: impl Fn(&ISimpleAudioVolume) -> WinResult<()>) -> Result<(), String> {
    let _com = Com::enter();
    (|| {
        let mut done = Ok(());
        for (_, device) in outputs(&enumerator()?, None)? {
            let Ok((_, controls)) = manager(&device).and_then(|m| controls(&m)) else { continue };
            for control in &controls {
                if !whose(control).is_some_and(|(pid, _)| pids.contains(&pid)) {
                    continue;
                }
                // A session that refuses does not keep the others from being set.
                if let Err(err) = control.cast::<ISimpleAudioVolume>().and_then(|level| set(&level)) {
                    done = Err(err);
                }
            }
        }
        done
    })()
    .map_err(text)
}

/// The active outputs with their ids; `only` keeps the one with that id.
fn outputs(enumerator: &IMMDeviceEnumerator, only: Option<&str>) -> WinResult<Vec<(String, IMMDevice)>> {
    unsafe {
        let list = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..list.GetCount()? {
            let device = list.Item(i)?;
            let id = device_id(&device)?;
            if only.is_none_or(|want| want == id) {
                out.push((id, device));
            }
        }
        Ok(out)
    }
}

fn manager(device: &IMMDevice) -> WinResult<IAudioSessionManager2> {
    unsafe { device.Activate(CLSCTX_ALL, None) }
}

/// Every session of an output. Counting them is also what makes Windows start
/// telling the manager's listener about new ones.
fn controls(manager: &IAudioSessionManager2) -> WinResult<(IAudioSessionEnumerator, Vec<IAudioSessionControl>)> {
    unsafe {
        let list = manager.GetSessionEnumerator()?;
        let controls = (0..list.GetCount()?).filter_map(|i| list.GetSession(i).ok()).collect();
        Ok((list, controls))
    }
}

/// The process of a session, and whether it is the system sounds (process 0).
fn whose(control: &IAudioSessionControl) -> Option<(u32, bool)> {
    unsafe {
        let more: IAudioSessionControl2 = control.cast().ok()?;
        if more.IsSystemSoundsSession() == S_OK {
            return Some((0, true));
        }
        more.GetProcessId().ok().filter(|pid| *pid != 0).map(|pid| (pid, false))
    }
}

fn read(control: &IAudioSessionControl, device: &str) -> Option<Session> {
    unsafe {
        let state = control.GetState().ok()?;
        if state == AudioSessionStateExpired {
            return None;
        }
        let (pid, system) = whose(control)?;
        let level: ISimpleAudioVolume = control.cast().ok()?;
        Some(Session {
            pid,
            exe: if system { String::new() } else { process_path(pid).unwrap_or_default() },
            name: taken(control.GetDisplayName()),
            icon: taken(control.GetIconPath()),
            volume: level.GetMasterVolume().ok()?,
            mute: level.GetMute().ok()?.as_bool(),
            active: state == AudioSessionStateActive,
            system,
            device: device.to_owned(),
        })
    }
}

/// A string COM allocated for us: copied, then freed.
unsafe fn taken(raw: WinResult<PWSTR>) -> String {
    match raw {
        Ok(raw) if !raw.is_null() => unsafe {
            let text = raw.to_string().unwrap_or_default();
            CoTaskMemFree(Some(raw.0 as *const _));
            text
        },
        _ => String::new(),
    }
}

// ── Subscribing ───────────────────────────────────────────────────────────────

/// A live subscription to the sessions. Dropping it unsubscribes; dropping the
/// last one ends the sessions thread and unregisters every listener.
#[must_use = "dropping the handle unsubscribes"]
pub struct SessionSub(u64);

/// Called on the sessions thread when a session came, went, started or stopped
/// playing, or had its level or mute changed by someone else (our own writes
/// are not told): read `sessions` again. At most once per 200 ms. Keep it short.
pub fn on_sessions(f: impl Fn() + Send + Sync + 'static) -> SessionSub {
    let mut hub = HUB.lock().unwrap();
    hub.next += 1;
    let id = hub.next;
    hub.subs.push((id, Arc::new(f)));
    if hub.thread.is_none() {
        let (tx, rx) = mpsc::channel();
        let callbacks = tx.clone();
        hub.thread = Some((tx, std::thread::spawn(move || run(rx, callbacks))));
    }
    SessionSub(id)
}

impl Drop for SessionSub {
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

type Handler = Arc<dyn Fn() + Send + Sync>;

struct Hub {
    next: u64,
    subs: Vec<(u64, Handler)>,
    thread: Option<(mpsc::Sender<Msg>, JoinHandle<()>)>,
}

static HUB: Mutex<Hub> = Mutex::new(Hub { next: 0, subs: Vec::new(), thread: None });

fn emit() {
    // Copied out first: a callback may subscribe or drop a `SessionSub`.
    let subs: Vec<Handler> = HUB.lock().unwrap().subs.iter().map(|s| s.1.clone()).collect();
    for f in subs {
        f();
    }
}

// ── The sessions thread ───────────────────────────────────────────────────────

enum Msg {
    /// What a session says changed: read again.
    Changed,
    /// The set of sessions or of outputs changed: listen again, then read again.
    Rebind,
    Stop,
}

/// A fader dragged in another mixer, or an app that starts, calls back many
/// times: subscribers are told once per window.
const SETTLE: Duration = Duration::from_millis(200);

/// What listens on one output. Kept alive for as long as it listens.
struct Bound {
    manager: IAudioSessionManager2,
    created: IAudioSessionNotification,
    _list: IAudioSessionEnumerator,
    sessions: Vec<(IAudioSessionControl, IAudioSessionEvents)>,
}

fn run(rx: mpsc::Receiver<Msg>, tx: mpsc::Sender<Msg>) {
    let _com = Com::enter();
    let Ok(enumerator) = enumerator() else { return };
    let devices: IMMNotificationClient = Events(tx.clone()).into();
    unsafe {
        let _ = enumerator.RegisterEndpointNotificationCallback(&devices);
    }
    let mut bound = bind(&enumerator, &tx);
    'life: while let Ok(first) = rx.recv() {
        let until = Instant::now() + SETTLE;
        let mut rebind = false;
        let mut msg = first;
        loop {
            match msg {
                Msg::Stop => break 'life,
                Msg::Rebind => rebind = true,
                Msg::Changed => {}
            }
            match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(next) => msg = next,
                Err(_) => break,
            }
        }
        if rebind {
            unbind(std::mem::take(&mut bound));
            bound = bind(&enumerator, &tx);
        }
        emit();
    }
    unbind(bound);
    unsafe {
        let _ = enumerator.UnregisterEndpointNotificationCallback(&devices);
    }
}

/// Listens to every active output for new sessions, and to every session.
fn bind(enumerator: &IMMDeviceEnumerator, tx: &mpsc::Sender<Msg>) -> Vec<Bound> {
    let mut out = Vec::new();
    for (_, device) in outputs(enumerator, None).unwrap_or_default() {
        let Ok(manager) = manager(&device) else { continue };
        let created: IAudioSessionNotification = Events(tx.clone()).into();
        unsafe {
            // Registered before the sessions are counted, as Windows wants it.
            let _ = manager.RegisterSessionNotification(&created);
            let Ok((list, controls)) = controls(&manager) else {
                let _ = manager.UnregisterSessionNotification(&created);
                continue;
            };
            let sessions = controls
                .into_iter()
                .filter_map(|control| {
                    let events: IAudioSessionEvents = Events(tx.clone()).into();
                    control.RegisterAudioSessionNotification(&events).ok()?;
                    Some((control, events))
                })
                .collect();
            out.push(Bound { manager, created, _list: list, sessions });
        }
    }
    out
}

fn unbind(bound: Vec<Bound>) {
    for output in bound {
        unsafe {
            for (control, events) in &output.sessions {
                let _ = control.UnregisterAudioSessionNotification(events);
            }
            let _ = output.manager.UnregisterSessionNotification(&output.created);
        }
    }
}

/// Every callback of the sessions thread: each only says that something changed.
#[implement(IAudioSessionNotification, IAudioSessionEvents, IMMNotificationClient)]
struct Events(mpsc::Sender<Msg>);

impl Events_Impl {
    fn say(&self, msg: Msg) -> WinResult<()> {
        let _ = self.0.send(msg);
        Ok(())
    }
}

impl IAudioSessionNotification_Impl for Events_Impl {
    fn OnSessionCreated(&self, _session: Ref<'_, IAudioSessionControl>) -> WinResult<()> {
        self.say(Msg::Rebind)
    }
}

impl IAudioSessionEvents_Impl for Events_Impl {
    fn OnDisplayNameChanged(&self, _name: &PCWSTR, _context: *const GUID) -> WinResult<()> {
        self.say(Msg::Changed)
    }
    fn OnIconPathChanged(&self, _path: &PCWSTR, _context: *const GUID) -> WinResult<()> {
        self.say(Msg::Changed)
    }
    fn OnSimpleVolumeChanged(&self, _volume: f32, _mute: BOOL, context: *const GUID) -> WinResult<()> {
        // Our own write: whoever wrote it knows.
        if unsafe { context.as_ref() } == Some(&OWN) {
            return Ok(());
        }
        self.say(Msg::Changed)
    }
    fn OnChannelVolumeChanged(&self, _count: u32, _volumes: *const f32, _changed: u32, _context: *const GUID) -> WinResult<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _grouping: *const GUID, _context: *const GUID) -> WinResult<()> {
        Ok(())
    }
    fn OnStateChanged(&self, _state: AudioSessionState) -> WinResult<()> {
        self.say(Msg::Changed)
    }
    fn OnSessionDisconnected(&self, _reason: AudioSessionDisconnectReason) -> WinResult<()> {
        self.say(Msg::Rebind)
    }
}

impl IMMNotificationClient_Impl for Events_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> WinResult<()> {
        self.say(Msg::Rebind)
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> WinResult<()> {
        self.say(Msg::Rebind)
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> WinResult<()> {
        self.say(Msg::Rebind)
    }
    // A new default moves no session and adds no output.
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, _role: ERole, _id: &PCWSTR) -> WinResult<()> {
        Ok(())
    }
    // Fires for every property of every device, many times a second while one is set up.
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> WinResult<()> {
        Ok(())
    }
}
