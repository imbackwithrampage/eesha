// The readers behind a tick, one per metric. Everything here lives on the
// sampler's thread and is dropped with it; a metric nobody needs gives up its
// baseline and its handles at once, so the next reading of it starts afresh
// and never reports an average over the gap.
//
// All of it works unelevated. What does not (CPU package temperature, fans,
// SMART) belongs to the hardware helper.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::core::{HSTRING, PCWSTR};
use windows::Devices::Power::Battery;
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessorPerformanceInformation};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, SEM_FAILCRITICALERRORS, THREAD_ERROR_MODE};
use windows::Win32::System::Ioctl::{
    DISK_PERFORMANCE, IOCTL_DISK_PERFORMANCE, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, GlobalMemoryStatusEx, RelationProcessorCore, MEMORYSTATUSEX,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::Win32::System::WindowsProgramming::{QueryUnbiasedInterruptTime, SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION};
use windows::Win32::System::IO::DeviceIoControl;

use crate::{Metric, Snapshot, METRICS};

#[derive(Clone, Default, Debug, Serialize)]
pub struct Gpu {
    /// The adapter that drives the main display.
    pub name: String,
    /// 0…1: the busiest engine. `None` on the first reading, before the counters are open.
    pub usage: Option<f32>,
    /// Bytes of video memory in use.
    pub dedicated: u64,
    pub shared: u64,
}

#[derive(Clone, Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub total: u64,
    /// Total less what Windows could hand out now.
    pub used: u64,
    pub available: u64,
    /// The commit charge and its limit (memory promised to programs, RAM and page file).
    pub committed: u64,
    pub commit_limit: u64,
    pub cached: u64,
    /// 0 normal, 1 warning (85 % in use, or 90 % committed), 2 critical (95 % committed).
    pub pressure: u8,
}

#[derive(Clone, Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Net {
    /// Bytes per second over the physical adapters; `None` on the first reading.
    pub down: Option<f64>,
    pub up: Option<f64>,
    /// Since this metric was last asked for.
    pub total_down: u64,
    pub total_up: u64,
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct Disk {
    /// Bytes per second over every physical drive; `None` on the first reading.
    pub read: Option<f64>,
    pub write: Option<f64>,
    pub volumes: Vec<Volume>,
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct Volume {
    /// "C:"
    pub letter: String,
    pub label: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    /// The one Windows runs from.
    pub system: bool,
    /// On a USB or card bus, or with media that comes out.
    pub ejectable: bool,
}

#[derive(Clone, Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Power {
    pub battery: bool,
    pub ac: bool,
    pub charging: bool,
    pub percent: Option<u8>,
    /// Only on battery, and only once Windows has an estimate.
    pub seconds_left: Option<u32>,
    /// Milliwatts into (+) or out of (−) the battery.
    pub rate_mw: Option<i32>,
    pub full_mwh: Option<i32>,
    pub design_mwh: Option<i32>,
    pub saver: bool,
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct Temp {
    /// The ACPI thermal zone's name: a place in the chassis, not a chip.
    pub name: String,
    pub celsius: f32,
}

/// The clock that stops while the machine sleeps, in milliseconds.
fn unbiased_ms() -> u64 {
    let mut t = 0u64;
    unsafe {
        let _ = QueryUnbiasedInterruptTime(&mut t);
    }
    t / 10_000
}

fn wide_str(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())])
}

/// The efficiency class of each logical processor (higher = a performance
/// core); all equal on a processor with one kind of core, empty when unknown.
pub fn core_classes() -> Vec<u8> {
    static CLASSES: OnceLock<Vec<u8>> = OnceLock::new();
    CLASSES
        .get_or_init(|| unsafe {
            let mut len = 0u32;
            let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len);
            let mut buf = vec![0u64; len as usize / 8 + 1];
            let first = buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
            if len == 0 || GetLogicalProcessorInformationEx(RelationProcessorCore, Some(first), &mut len).is_err() {
                return Vec::new();
            }
            let mut out = Vec::new();
            let mut at = 0usize;
            while at < len as usize {
                let info = &*((first as *const u8).add(at) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX);
                let core = &info.Anonymous.Processor;
                // debt: the first processor group only (64 logical processors).
                if core.GroupMask[0].Group == 0 {
                    for bit in 0..usize::BITS as usize {
                        if core.GroupMask[0].Mask & (1 << bit) != 0 {
                            if out.len() <= bit {
                                out.resize(bit + 1, 0);
                            }
                            out[bit] = core.EfficiencyClass;
                        }
                    }
                }
                at += info.Size.max(1) as usize;
            }
            out
        })
        .clone()
}

// ── PDH: the counters Windows only publishes there ────────────────────────────

/// A query with wildcard counters, read as (instance, value) lists. Closed on drop.
struct Pdh {
    query: PDH_HQUERY,
    counters: Vec<PDH_HCOUNTER>,
    buf: Vec<u64>,
}

impl Pdh {
    /// `None` when a counter is not there (an old driver, a virtual machine).
    fn open(paths: &[&str]) -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return None;
        }
        let mut me = Self { query, counters: Vec::new(), buf: Vec::new() };
        for path in paths {
            let mut counter = PDH_HCOUNTER::default();
            if unsafe { PdhAddEnglishCounterW(query, &HSTRING::from(*path), 0, &mut counter) } != 0 {
                return None;
            }
            me.counters.push(counter);
        }
        // The baseline of the counters that are rates.
        me.collect();
        Some(me)
    }

    fn collect(&self) {
        unsafe {
            PdhCollectQueryData(self.query);
        }
    }

    /// The instances of counter `i` that have a value.
    fn values(&mut self, i: usize) -> Vec<(String, f64)> {
        let (mut size, mut count) = (0u32, 0u32);
        unsafe {
            if PdhGetFormattedCounterArrayW(self.counters[i], PDH_FMT_DOUBLE, &mut size, &mut count, None) != PDH_MORE_DATA {
                return Vec::new();
            }
            // Instances come and go between the two calls: some room to spare.
            self.buf.resize(size as usize / 8 + 64, 0);
            size = (self.buf.len() * 8) as u32;
            let items = self.buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counters[i], PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(items, count as usize)
                .iter()
                // 0 = valid, 1 = new data.
                .filter(|it| it.FmtValue.CStatus <= 1 && !it.szName.is_null())
                .map(|it| (it.szName.to_string().unwrap_or_default(), it.FmtValue.Anonymous.doubleValue))
                .collect()
        }
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

struct GpuQuery {
    pdh: Pdh,
    name: String,
}

impl GpuQuery {
    fn open() -> Option<Self> {
        let pdh = Pdh::open(&[
            r"\GPU Engine(*)\Utilization Percentage",
            r"\GPU Adapter Memory(*)\Dedicated Usage",
            r"\GPU Adapter Memory(*)\Shared Usage",
        ])?;
        Some(Self { pdh, name: adapter_name() })
    }

    /// `None`: the counters exist but list no engine (hide the card).
    fn read(&mut self) -> Option<(Gpu, Vec<(u32, f32)>)> {
        let memory = |pdh: &mut Pdh, i| pdh.values(i).iter().map(|(_, v)| *v).sum::<f64>().max(0.0) as u64;
        // The query was opened, and its baseline read, a moment after the tick before.
        self.pdh.collect();
        // An instance is one process on one engine:
        // pid_1234_luid_0x00000000_0x0000D4B8_phys_0_eng_0_engtype_3D.
        // An engine's load is the sum over its processes; the headline is the
        // busiest engine, as Task Manager has it; a process counts with its
        // busiest engine.
        let mut engines: HashMap<String, f64> = HashMap::new();
        let mut pids: HashMap<u32, f32> = HashMap::new();
        for (name, value) in self.pdh.values(0) {
            let Some((pid, engine)) = name.strip_prefix("pid_").and_then(|rest| rest.split_once('_')) else { continue };
            *engines.entry(engine.to_owned()).or_default() += value;
            if let (Ok(pid), true) = (pid.parse::<u32>(), value >= 0.05) {
                let share = pids.entry(pid).or_default();
                *share = share.max((value / 100.0).min(1.0) as f32);
            }
        }
        if engines.is_empty() {
            return None;
        }
        let busiest = engines.values().fold(0f64, |a, b| a.max(*b));
        let gpu = Gpu {
            name: self.name.clone(),
            usage: Some((busiest / 100.0).clamp(0.0, 1.0) as f32),
            dedicated: memory(&mut self.pdh, 1),
            shared: memory(&mut self.pdh, 2),
        };
        Some((gpu, pids.into_iter().collect()))
    }
}

/// The adapter of the primary display, as Windows names it.
fn adapter_name() -> String {
    const PRIMARY: u32 = 4; // DISPLAY_DEVICE_PRIMARY_DEVICE
    let mut first = String::new();
    for i in 0..16 {
        let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0) }.as_bool() {
            break;
        }
        let name = wide_str(&dd.DeviceString);
        if dd.StateFlags.0 & PRIMARY != 0 {
            return name;
        }
        if first.is_empty() {
            first = name;
        }
    }
    first
}

fn read_temps(pdh: &mut Pdh) -> Vec<Temp> {
    pdh.collect();
    let mut temps: Vec<Temp> = pdh
        .values(0)
        .into_iter()
        // Kelvin. A zone that reports nonsense is left out.
        .map(|(name, kelvin)| Temp { name: name.rsplit(['.', '\\']).next().unwrap_or_default().to_owned(), celsius: (kelvin - 273.15) as f32 })
        .filter(|t| (1.0..=125.0).contains(&t.celsius))
        .collect();
    temps.sort_by(|a, b| a.name.cmp(&b.name));
    temps
}

// ── Disks ─────────────────────────────────────────────────────────────────────

/// The physical drives, held open for their transfer counters.
struct DiskIo {
    handles: Vec<HANDLE>,
    prev: Option<(Instant, i64, i64)>,
    opened: Instant,
}

/// Opened for control codes only: no read or write access, so no elevation.
fn open_device(path: &str) -> Option<HANDLE> {
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE;
    unsafe { CreateFileW(&HSTRING::from(path), 0, share, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.ok()
}

impl DiskIo {
    fn open() -> Self {
        let handles = (0..16).filter_map(|n| open_device(&format!(r"\\.\PhysicalDrive{n}"))).collect();
        Self { handles, prev: None, opened: Instant::now() }
    }

    /// Bytes read and written since boot, over every drive.
    fn totals(&self) -> (i64, i64) {
        let mut sum = (0i64, 0i64);
        for h in &self.handles {
            let mut perf = DISK_PERFORMANCE::default();
            let mut got = 0u32;
            let size = std::mem::size_of::<DISK_PERFORMANCE>() as u32;
            let out = Some(&mut perf as *mut _ as *mut _);
            if unsafe { DeviceIoControl(*h, IOCTL_DISK_PERFORMANCE, None, 0, out, size, Some(&mut got), None) }.is_ok() {
                sum.0 += perf.BytesRead;
                sum.1 += perf.BytesWritten;
            }
        }
        sum
    }

    fn rates(&mut self) -> (Option<f64>, Option<f64>) {
        // A drive that came or went: counted from the next look, on a new baseline.
        if self.opened.elapsed() > Duration::from_secs(30) {
            let (count, prev) = (self.handles.len(), self.prev.take());
            *self = Self::open();
            self.prev = prev.filter(|_| self.handles.len() == count);
        }
        let now = Instant::now();
        let (read, write) = self.totals();
        let rates = self.prev.map(|(at, r, w)| {
            let secs = now.duration_since(at).as_secs_f64().max(0.001);
            ((read - r).max(0) as f64 / secs, (write - w).max(0) as f64 / secs)
        });
        self.prev = Some((now, read, write));
        (rates.map(|r| r.0), rates.map(|r| r.1))
    }
}

impl Drop for DiskIo {
    fn drop(&mut self) {
        for h in self.handles.drain(..) {
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

/// Whether the volume sits on a bus that is unplugged (USB, 1394, SD, MMC) or
/// has media that comes out.
fn ejectable(letter: &str) -> bool {
    let Some(h) = open_device(&format!(r"\\.\{letter}")) else { return false };
    let query = STORAGE_PROPERTY_QUERY::default(); // StorageDeviceProperty, PropertyStandardQuery
    let mut out = [0u64; 128];
    let mut got = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query as *const _ as *const _),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(out.as_mut_ptr() as *mut _),
            std::mem::size_of_val(&out) as u32,
            Some(&mut got),
            None,
        )
    }
    .is_ok();
    unsafe {
        let _ = CloseHandle(h);
    }
    if !ok || (got as usize) < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        return false;
    }
    let d = unsafe { &*(out.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
    d.RemovableMedia || matches!(d.BusType.0, 4 | 7 | 12 | 13)
}

/// The fixed and removable volumes that hold something.
pub(crate) fn volumes() -> Vec<Volume> {
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_uppercase();
    let mut out = Vec::new();
    unsafe {
        // An empty card reader must not put up a "no disk" box.
        let mut before = THREAD_ERROR_MODE(0);
        let _ = SetThreadErrorMode(SEM_FAILCRITICALERRORS, Some(&mut before));
        let mask = GetLogicalDrives();
        for i in 0..26u8 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = format!("{}:", (b'A' + i) as char);
            let root = HSTRING::from(format!("{letter}\\"));
            let kind = GetDriveTypeW(&root);
            let (mut free, mut total) = (0u64, 0u64);
            if (kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE)
                || GetDiskFreeSpaceExW(&root, Some(&mut free), Some(&mut total), None).is_err()
                || total == 0
            {
                continue;
            }
            let (mut label, mut fs) = ([0u16; 64], [0u16; 16]);
            let _ = GetVolumeInformationW(&root, Some(&mut label), None, None, None, Some(&mut fs));
            let is_system = letter == system;
            out.push(Volume {
                ejectable: !is_system && (kind == DRIVE_REMOVABLE || ejectable(&letter)),
                letter,
                label: wide_str(&label),
                fs: wide_str(&fs),
                total,
                free,
                system: is_system,
            });
        }
        let _ = SetThreadErrorMode(before, None);
    }
    out
}

// ── The plain readers ─────────────────────────────────────────────────────────

fn read_memory() -> Option<Memory> {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    let mut p = PERFORMANCE_INFORMATION { cb: std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32, ..Default::default() };
    unsafe {
        GlobalMemoryStatusEx(&mut m).ok()?;
        GetPerformanceInfo(&mut p, p.cb).ok()?;
    }
    let page = p.PageSize as u64;
    let (committed, commit_limit) = (p.CommitTotal as u64 * page, p.CommitLimit as u64 * page);
    let used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
    let load = used as f64 / m.ullTotalPhys.max(1) as f64;
    let commit = committed as f64 / commit_limit.max(1) as f64;
    // Windows has no pressure level of its own: these are ours.
    // debt: the low-memory notification object would say "critical" later but for certain.
    let pressure = if commit > 0.95 { 2 } else if load >= 0.85 || commit > 0.90 { 1 } else { 0 };
    Some(Memory {
        total: m.ullTotalPhys,
        used,
        available: m.ullAvailPhys,
        committed,
        commit_limit,
        cached: p.SystemCache as u64 * page,
        pressure,
    })
}

/// Bytes received and sent since boot over the physical adapters that are up:
/// Ethernet, Wi-Fi and mobile broadband. Loopback, tunnels and the virtual
/// switches are left out, so traffic through a VPN is not counted twice.
fn net_totals() -> Option<(u64, u64)> {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return None;
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let mut sum = (0u64, 0u64);
        for r in rows {
            let hardware = r.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && r.OperStatus == IfOperStatusUp && matches!(r.Type, 6 | 71 | 243 | 244) {
                sum.0 += r.InOctets;
                sum.1 += r.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        Some(sum)
    }
}

fn read_power() -> Option<Power> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    // 128 = no system battery, 255 = unknown.
    let battery = s.BatteryFlag != 255 && s.BatteryFlag & 128 == 0;
    let ac = s.ACLineStatus == 1;
    let mut power = Power {
        battery,
        ac,
        charging: battery && s.BatteryFlag & 8 != 0,
        percent: (battery && s.BatteryLifePercent <= 100).then_some(s.BatteryLifePercent),
        seconds_left: (battery && !ac && s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime),
        saver: s.SystemStatusFlag == 1,
        ..Default::default()
    };
    if battery {
        if let Ok(report) = Battery::AggregateBattery().and_then(|b| b.GetReport()) {
            power.rate_mw = report.ChargeRateInMilliwatts().and_then(|v| v.Value()).ok();
            // Some firmware reports 0 for a capacity it does not know.
            power.full_mwh = report.FullChargeCapacityInMilliwattHours().and_then(|v| v.Value()).ok().filter(|v| *v > 0);
            power.design_mwh = report.DesignCapacityInMilliwattHours().and_then(|v| v.Value()).ok().filter(|v| *v > 0);
        }
    }
    Some(power)
}

// ── A tick ────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub(crate) struct Samplers {
    tick: u64,
    last: Option<Instant>,
    /// Per core: idle and total time at the last reading.
    cpu: Option<Vec<(i64, i64)>>,
    net: Option<(Instant, u64, u64)>,
    net_sum: (u64, u64),
    disk: Option<DiskIo>,
    volumes: Vec<Volume>,
    /// Outer `None`: not tried since it was needed. Inner `None`: not there.
    gpu: Option<Option<GpuQuery>>,
    temps: Option<Option<Pdh>>,
}

impl Samplers {
    /// One reading of what is needed, and whether a rate still waits for its
    /// second reading (then the next tick comes early).
    pub(crate) fn read(&mut self, needs: [bool; METRICS]) -> (Snapshot, bool) {
        let on = |m: Metric| needs[m as usize];
        // After a gap (sleep) the old counters are a baseline at most.
        if self.last.is_some_and(|at| at.elapsed() > Duration::from_secs(10)) {
            self.cpu = None;
            self.net = None;
            if let Some(disk) = &mut self.disk {
                disk.prev = None;
            }
        }
        self.last = Some(Instant::now());
        let mut snap = Snapshot { at: unbiased_ms(), ..Default::default() };
        let mut priming = false;

        if on(Metric::Cpu) {
            (snap.cpu, snap.cores) = self.read_cpu();
            priming |= snap.cpu.is_none();
        } else {
            self.cpu = None;
        }

        if on(Metric::Memory) {
            snap.mem = read_memory();
        }

        if on(Metric::Network) {
            if let Some((down, up)) = net_totals() {
                let now = Instant::now();
                let mut net = Net::default();
                if let Some((at, d, u)) = self.net {
                    let secs = now.duration_since(at).as_secs_f64().max(0.001);
                    // A counter that went backwards (an adapter left) counts as nothing.
                    let (dd, du) = (down.saturating_sub(d), up.saturating_sub(u));
                    self.net_sum = (self.net_sum.0 + dd, self.net_sum.1 + du);
                    net.down = Some(dd as f64 / secs);
                    net.up = Some(du as f64 / secs);
                }
                (net.total_down, net.total_up) = self.net_sum;
                priming |= net.down.is_none();
                self.net = Some((now, down, up));
                snap.net = Some(net);
            }
        } else {
            self.net = None;
            self.net_sum = (0, 0);
        }

        if on(Metric::Disk) {
            let io = self.disk.get_or_insert_with(DiskIo::open);
            let (read, write) = io.rates();
            priming |= read.is_none();
            // Free space moves slowly: every fifth tick.
            if self.volumes.is_empty() || self.tick % 5 == 0 {
                self.volumes = volumes();
            }
            snap.disk = Some(Disk { read, write, volumes: self.volumes.clone() });
        } else {
            self.disk = None;
            self.volumes.clear();
        }

        if on(Metric::Power) {
            snap.power = read_power();
        }

        // The two PDH queries take a moment to open: that is done after this
        // reading has gone out (`open`), and until then the GPU is "not yet known".
        if on(Metric::Gpu) {
            match &mut self.gpu {
                None => {
                    snap.gpu = Some(Gpu::default());
                    priming = true;
                }
                Some(query) => {
                    if let Some((gpu, pids)) = query.as_mut().and_then(GpuQuery::read) {
                        priming |= gpu.usage.is_none();
                        snap.gpu = Some(gpu);
                        snap.gpu_pids = pids;
                    }
                }
            }
        } else {
            self.gpu = None;
        }

        if on(Metric::Temps) {
            match &mut self.temps {
                None => priming = true,
                Some(query) => snap.temps = query.as_mut().map(read_temps).unwrap_or_default(),
            }
        } else {
            self.temps = None;
        }

        self.tick += 1;
        (snap, priming)
    }

    /// Opens the counters that were asked for and are not open yet. `None`
    /// inside means they are not there: the GPU card and the temperatures hide.
    pub(crate) fn open(&mut self, needs: [bool; METRICS]) {
        if needs[Metric::Gpu as usize] && self.gpu.is_none() {
            self.gpu = Some(GpuQuery::open());
        }
        if needs[Metric::Temps as usize] && self.temps.is_none() {
            self.temps = Some(Pdh::open(&[r"\Thermal Zone Information(*)\Temperature"]));
        }
    }

    /// Busy share of the time since the last reading, overall and per core.
    fn read_cpu(&mut self) -> (Option<f32>, Vec<f32>) {
        // debt: the thread's own processor group only (64 logical processors).
        let mut buf = [SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION::default(); 64];
        let mut len = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessorPerformanceInformation,
                buf.as_mut_ptr() as *mut _,
                std::mem::size_of_val(&buf) as u32,
                &mut len,
            )
        };
        if status.is_err() {
            self.cpu = None;
            return (None, Vec::new());
        }
        let count = len as usize / std::mem::size_of::<SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION>();
        // Kernel time includes idle time.
        let now: Vec<(i64, i64)> = buf[..count].iter().map(|c| (c.IdleTime, c.KernelTime + c.UserTime)).collect();
        let busy = |idle: i64, total: i64| if total <= 0 { 0.0 } else { (1.0 - idle as f32 / total as f32).clamp(0.0, 1.0) };
        let out = match self.cpu.as_ref().filter(|prev| prev.len() == count) {
            Some(prev) => {
                let deltas: Vec<(i64, i64)> = now.iter().zip(prev).map(|(n, p)| (n.0 - p.0, n.1 - p.1)).collect();
                let (idle, total) = deltas.iter().fold((0, 0), |a, d| (a.0 + d.0, a.1 + d.1));
                (Some(busy(idle, total)), deltas.iter().map(|d| busy(d.0, d.1)).collect())
            }
            None => (None, Vec::new()),
        };
        self.cpu = Some(now);
        out
    }
}
