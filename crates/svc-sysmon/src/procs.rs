// The process list, ending a process, and the listening ports. Plain calls on
// the caller's thread: nothing here runs on its own. The list is one
// `NtQuerySystemInformation` shared by whoever asks within the freshness
// window; a process's CPU share is the time it used since the reading before.
//
// A process is named by its ID *and* the moment it started: an ID is handed
// out again once its process has ended, and nothing is ever ended on the
// strength of an ID alone.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Serialize, Serializer};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows::Win32::System::Threading::{
    GetActiveProcessorCount, GetProcessTimes, OpenProcess, TerminateProcess, ALL_PROCESSOR_GROUPS, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

/// A 64-bit number as text: JavaScript cannot hold one exactly.
fn as_text<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

#[derive(Clone, Debug, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    /// The image name (`chrome.exe`).
    pub name: String,
    /// When it started (a FILETIME): with `pid`, who it is.
    #[serde(serialize_with = "as_text")]
    pub started: u64,
    /// 0…1 of all cores since the reading before; `None` when there was none.
    pub cpu: Option<f32>,
    /// Private working set, bytes: Task Manager's "Memory".
    pub mem: u64,
    /// Never ended from here: Windows itself, and this app.
    pub protected: bool,
}

/// What Windows runs on, by image name in lower case.
const PROTECTED: [&str; 11] = [
    "registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "dwm.exe", "fontdrvhost.exe",
    "memory compression", "secure system",
];

fn own_exe() -> String {
    std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// The head of an entry of `SystemProcessInformation` on 64-bit Windows.
#[repr(C)]
struct Entry {
    next: u32,
    threads: u32,
    private_working_set: i64,
    hard_faults: u32,
    threads_high: u32,
    cycles: u64,
    create: i64,
    user: i64,
    kernel: i64,
    name_len: u16,
    name_max: u16,
    name: *const u16,
    priority: i32,
    pid: usize,
    parent: usize,
}

#[derive(Default)]
struct Cache {
    at: Option<Instant>,
    list: Vec<Proc>,
    /// CPU time used (100 ns units) at the last reading, by who.
    used: HashMap<(u32, u64), i64>,
    buf: Vec<u64>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// Every process. A reading younger than `max_age` is served again.
pub fn processes(max_age: Duration) -> Vec<Proc> {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(Cache::default);
    if cache.at.is_some_and(|at| at.elapsed() < max_age) {
        return cache.list.clone();
    }
    let now = Instant::now();
    // Older than this, the last reading is no baseline for a rate.
    let span = cache.at.map(|at| now.duration_since(at)).filter(|d| *d < Duration::from_secs(30));
    let budget = span.map(|d| d.as_secs_f64() * 1e7 * unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1) as f64);
    if cache.buf.is_empty() {
        cache.buf.resize(64 * 1024, 0);
    }
    let mut len = 0u32;
    // The list grows between the two calls: until it fits.
    for _ in 0..8 {
        let size = (cache.buf.len() * 8) as u32;
        let status = unsafe { NtQuerySystemInformation(SystemProcessInformation, cache.buf.as_mut_ptr() as *mut _, size, &mut len) };
        if status.is_ok() {
            break;
        }
        len = 0;
        cache.buf.resize(cache.buf.len() * 2, 0);
    }
    let (me, mine) = (std::process::id(), own_exe());
    let mut list = Vec::new();
    let mut used = HashMap::new();
    let mut at = 0usize;
    while len > 0 {
        let e = unsafe { &*((cache.buf.as_ptr() as *const u8).add(at) as *const Entry) };
        let pid = e.pid as u32;
        let name = match pid {
            0 => String::new(),
            _ if e.name.is_null() => "System".into(),
            _ => String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(e.name, e.name_len as usize / 2) }),
        };
        // Process 0 is the idle time itself.
        if pid != 0 {
            let started = e.create as u64;
            let time = e.user + e.kernel;
            let before = cache.used.get(&(pid, started));
            let cpu = budget.zip(before).map(|(budget, before)| ((time - before) as f64 / budget).clamp(0.0, 1.0) as f32);
            used.insert((pid, started), time);
            let lower = name.to_lowercase();
            list.push(Proc {
                pid,
                parent: e.parent as u32,
                protected: pid == 4 || pid == me || lower == mine || PROTECTED.contains(&lower.as_str()),
                name,
                started,
                cpu,
                mem: e.private_working_set.max(0) as u64,
            });
        }
        if e.next == 0 {
            break;
        }
        at += e.next as usize;
    }
    // A failed reading never wipes a good list.
    if !list.is_empty() {
        cache.list = list;
        cache.used = used;
        cache.at = Some(now);
    }
    cache.list.clone()
}

/// Why a process was not ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Refused {
    /// Windows itself, or this app.
    Protected,
    /// It ended by itself, or the ID now belongs to another process.
    Gone,
    /// It runs as administrator or as another user.
    Denied,
    /// It has no window to ask.
    Windowless,
    Failed,
}

/// The process `pid` that started at `started`, checked against the list.
fn checked(pid: u32, started: u64) -> Result<(), Refused> {
    match processes(Duration::from_secs(1)).iter().find(|p| p.pid == pid && p.started == started) {
        None => Err(Refused::Gone),
        Some(p) if p.protected => Err(Refused::Protected),
        Some(_) => Ok(()),
    }
}

/// Ends the process at once (it gets no chance to save anything). The start
/// time is checked again on the handle that ends it, which also keeps the ID
/// from being handed to another process in between.
pub fn kill(pid: u32, started: u64) -> Result<(), Refused> {
    checked(pid, started)?;
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|e| if e.code() == ERROR_ACCESS_DENIED.to_hresult() { Refused::Denied } else { Refused::Gone })?;
        let mut created = FILETIME::default();
        let (mut exit, mut kernel, mut user) = (created, created, created);
        let same = GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user).is_ok()
            && ((created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64) == started;
        // An exit time: it ended a moment ago and only its record is left.
        let alive = exit.dwHighDateTime == 0 && exit.dwLowDateTime == 0;
        let done = if same && alive { TerminateProcess(handle, 1).map_err(|_| Refused::Denied) } else { Err(Refused::Gone) };
        let _ = CloseHandle(handle);
        done
    }
}

/// Asks the process to close, as a click on its windows' close buttons would:
/// it may ask the user about unsaved work, or not close at all. Returns how
/// many windows were asked.
pub fn ask_to_close(pid: u32, started: u64) -> Result<usize, Refused> {
    checked(pid, started)?;
    let windows: Vec<isize> = notch_core::win::top_windows().into_iter().filter(|w| w.1 == pid).map(|w| w.0).collect();
    if windows.is_empty() {
        return Err(Refused::Windowless);
    }
    let asked = windows.iter().filter(|w| unsafe { PostMessageW(Some(HWND(**w as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_ok());
    match asked.count() {
        0 => Err(Refused::Denied),
        n => Ok(n),
    }
}

// ── Listening ports ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
pub struct Port {
    pub port: u16,
    /// The address it is bound to, as text.
    pub address: String,
    pub v6: bool,
    /// Bound to every interface: other machines can reach it unless a firewall stops them.
    pub all: bool,
    pub pid: u32,
    /// Empty when the process is not in the list (it ended meanwhile).
    pub name: String,
    #[serde(serialize_with = "as_text")]
    pub started: u64,
    pub protected: bool,
}

/// The owner-PID listener table of one address family, as raw bytes.
fn tcp_table(family: u32) -> Result<Vec<u32>, String> {
    let mut size = 0u32;
    let mut buf: Vec<u32> = Vec::new();
    // The table changes between the calls: until it fits.
    for _ in 0..8 {
        let ptr = (!buf.is_empty()).then(|| buf.as_mut_ptr() as *mut _);
        match unsafe { GetExtendedTcpTable(ptr, &mut size, false, family, TCP_TABLE_OWNER_PID_LISTENER, 0) } {
            0 => return Ok(buf),
            // ERROR_INSUFFICIENT_BUFFER
            122 => buf.resize(size as usize / 4 + 16, 0),
            code => return Err(format!("GetExtendedTcpTable: error {code}")),
        }
        size = (buf.len() * 4) as u32;
    }
    Err("GetExtendedTcpTable: the table kept growing".into())
}

/// The first u32 is the count, the rows follow.
unsafe fn rows<T>(table: &[u32]) -> &[T] {
    match table.first() {
        Some(count) => unsafe { std::slice::from_raw_parts(table.as_ptr().add(1) as *const T, *count as usize) },
        None => &[],
    }
}

/// Every TCP socket that listens, with its process: by port, then by name.
/// `Err` is a failed listing, not an empty one.
pub fn ports() -> Result<Vec<Port>, String> {
    let (v4, v6) = (tcp_table(2)?, tcp_table(23)?); // AF_INET, AF_INET6
    let port_of = |raw: u32| u16::from_be(raw as u16);
    let mut found: Vec<(u16, String, bool, bool, u32)> = Vec::new();
    for r in unsafe { rows::<MIB_TCPROW_OWNER_PID>(&v4) } {
        let ip = Ipv4Addr::from(r.dwLocalAddr.to_ne_bytes());
        found.push((port_of(r.dwLocalPort), ip.to_string(), false, ip.is_unspecified(), r.dwOwningPid));
    }
    for r in unsafe { rows::<MIB_TCP6ROW_OWNER_PID>(&v6) } {
        let ip = Ipv6Addr::from(r.ucLocalAddr);
        found.push((port_of(r.dwLocalPort), ip.to_string(), true, ip.is_unspecified(), r.dwOwningPid));
    }
    // Read after the table: a process found here was there for all of it.
    let procs: HashMap<u32, Proc> = processes(Duration::from_secs(1)).into_iter().map(|p| (p.pid, p)).collect();
    let mut out: Vec<Port> = found
        .into_iter()
        .map(|(port, address, v6, all, pid)| {
            let p = procs.get(&pid);
            Port {
                port,
                address,
                v6,
                all,
                pid,
                name: p.map(|p| p.name.clone()).unwrap_or_default(),
                started: p.map_or(0, |p| p.started),
                protected: p.is_none_or(|p| p.protected),
            }
        })
        .collect();
    out.sort_by(|a, b| (a.port, &a.name, a.pid, &a.address).cmp(&(b.port, &b.name, b.pid, &b.address)));
    out.dedup_by(|a, b| (a.port, a.pid, &a.address) == (b.port, b.pid, &b.address));
    Ok(out)
}
