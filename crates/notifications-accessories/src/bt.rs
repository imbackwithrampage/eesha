// Bluetooth accessories: which are paired, which are connected, and the battery
// Windows has for them. One DeviceWatcher over the paired devices (classic and
// LE) tells of every link that comes up or goes down; nothing is polled, and a
// battery is read only when there is something to say.
//
// Which devices are accessories and the rule of who is announced follow
// reference/vorssaint-utils/Sources/Vorssaint/Services/Notch/NotchAccessorySupport.swift
// (`announcesConnection`, `symbol(name:majorClass:minorClass:)`, `NotchAccessoryConnectionState`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use windows::core::{IInspectable, Interface, Ref, Result, HSTRING};
use windows::Devices::Bluetooth::GenericAttributeProfile::{GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::{IPropertyValue, PropertyType, TypedEventHandler};
use windows::Storage::Streams::DataReader;
use windows_collections::{IIterable, IMapView};

/// A headset says its battery a moment after its link is up, and a link that
/// drops and comes back within this time is not news.
const SETTLE: Duration = Duration::from_secs(2);
/// The longest a connect waits for its battery after that: a device that is
/// slow to answer (an uncached GATT read can take half a minute) is announced
/// without one rather than late or never.
pub const BATTERY_WAIT: Duration = Duration::from_millis(1500);

const ADDRESS: &str = "System.Devices.Aep.DeviceAddress";
const CONNECTED: &str = "System.Devices.Aep.IsConnected";
/// Asked for only so that Windows tells when it changes: a reason to look at the link again.
const PRESENT: &str = "System.Devices.Aep.IsPresent";
const MAJOR: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
const MINOR: &str = "System.Devices.Aep.Bluetooth.Cod.Minor";
const APPEARANCE: &str = "System.Devices.Aep.Bluetooth.Le.Appearance";
/// DEVPKEY_Bluetooth_Battery, on a device node: the percentage Windows Settings shows.
const BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
const INSTANCE: &str = "System.Devices.DeviceInstanceId";

// ── What a device is ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Audio,
    Keyboard,
    Mouse,
    Gamepad,
    Other,
}

impl Kind {
    /// The headphone battery is for these only: a mouse that connected last is not a headset.
    pub fn is_audio(self) -> bool {
        self == Kind::Audio
    }
}

/// From the class of device a classic link announces, or the appearance of an
/// LE one (Bluetooth assigned numbers). `None`: a computer, a phone or a
/// network point, whose links come and go without anyone reaching for them.
/// A device that says nothing about itself is still an accessory.
pub fn kind(major: Option<u32>, minor: u32, appearance: Option<u32>) -> Option<Kind> {
    if let Some(major) = major {
        return match major {
            1..=3 => None,
            4 => Some(Kind::Audio),
            5 if matches!(minor & 0x0F, 1 | 2) => Some(Kind::Gamepad),
            5 if minor & 0x30 == 0x20 => Some(Kind::Mouse),
            5 if minor & 0x10 != 0 => Some(Kind::Keyboard),
            _ => Some(Kind::Other),
        };
    }
    let Some(appearance) = appearance else { return Some(Kind::Other) };
    // The category is the top ten bits: 1 phone, 2 computer, 15 HID, 33 and 34 audio sink and source, 37 wearable audio.
    match (appearance >> 6, appearance & 0x3F) {
        (1 | 2, _) => None,
        (0x0F, 1) => Some(Kind::Keyboard),
        (0x0F, 2) => Some(Kind::Mouse),
        (0x0F, 3 | 4) => Some(Kind::Gamepad),
        (0x21 | 0x22 | 0x25, _) => Some(Kind::Audio),
        _ => Some(Kind::Other),
    }
}

// ── The battery ───────────────────────────────────────────────────────────────

/// One level, or one per earbud where the device says which is which.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize)]
pub struct Battery {
    pub percent: Option<u8>,
    pub left: Option<u8>,
    pub right: Option<u8>,
}

impl Battery {
    /// What the colour goes by: the emptier earbud when there are two.
    pub fn lowest(&self) -> Option<u8> {
        [self.percent, self.left, self.right].into_iter().flatten().min()
    }

    pub fn tone(&self) -> Option<&'static str> {
        self.lowest().map(tone)
    }
}

/// The colour of a level: red at 20 % and under (the low mark of the laptop's own battery), amber to half.
pub fn tone(percent: u8) -> &'static str {
    match percent {
        0..=20 => "bad",
        21..=50 => "warn",
        _ => "ok",
    }
}

// ── Who is announced ──────────────────────────────────────────────────────────

/// Who is connected and what the user was told. A device is its address; a
/// dual-mode one has two links (classic and LE) and is connected while either
/// is up.
#[derive(Default)]
pub struct Connections {
    primed: bool,
    links: HashMap<String, HashSet<String>>,
    told: HashMap<String, bool>,
}

impl Connections {
    /// A link came up or went down. True when that leaves something to tell: ask `settle` a moment later.
    pub fn feed(&mut self, device: &str, link: &str, up: bool) -> bool {
        let links = self.links.entry(device.to_owned()).or_default();
        if up {
            links.insert(link.to_owned());
        } else {
            links.remove(link);
        }
        self.primed && self.connected(device) != self.was_told(device)
    }

    /// The first listing is over: what is connected now is where we start, not news.
    pub fn prime(&mut self) {
        self.told = self.links.iter().map(|(device, links)| (device.clone(), !links.is_empty())).collect();
        self.primed = true;
    }

    pub fn connected(&self, device: &str) -> bool {
        self.links.get(device).is_some_and(|links| !links.is_empty())
    }

    fn was_told(&self, device: &str) -> bool {
        self.told.get(device).copied().unwrap_or(false)
    }

    /// The settle time is over: `Some(connected)` when that is still news. A
    /// link that came and went meanwhile tells nothing.
    pub fn settle(&mut self, device: &str) -> Option<bool> {
        let now = self.connected(device);
        if !self.primed || self.was_told(device) == now {
            return None;
        }
        self.told.insert(device.to_owned(), now);
        Some(now)
    }
}

// ── Reading Windows ───────────────────────────────────────────────────────────

/// A paired accessory, its links merged.
#[derive(Clone, Debug)]
pub struct Device {
    /// Twelve hex digits, in capitals.
    pub address: String,
    pub name: String,
    pub kind: Kind,
    pub connected: bool,
    pub battery: Option<Battery>,
    /// Its LE link while that is up: where a battery service can be read.
    le: Option<String>,
}

/// One link of a device, as the watcher lists it.
#[derive(Clone)]
struct Link {
    id: String,
    address: String,
    name: String,
    kind: Kind,
    connected: bool,
}

type Props = IMapView<HSTRING, IInspectable>;

fn value(props: &Props, key: &str) -> Option<IPropertyValue> {
    props.Lookup(&HSTRING::from(key)).ok()?.cast().ok()
}

fn num(props: &Props, key: &str) -> Option<u32> {
    let v = value(props, key)?;
    match v.Type().ok()? {
        PropertyType::UInt8 => v.GetUInt8().ok().map(u32::from),
        PropertyType::UInt16 => v.GetUInt16().ok().map(u32::from),
        PropertyType::UInt32 => v.GetUInt32().ok(),
        PropertyType::Int32 => v.GetInt32().ok().and_then(|n| u32::try_from(n).ok()),
        _ => None,
    }
}

fn flag(props: &Props, key: &str) -> Option<bool> {
    value(props, key)?.GetBoolean().ok()
}

fn text(props: &Props, key: &str) -> Option<String> {
    value(props, key)?.GetString().ok().map(|s| s.to_string())
}

fn strings(keys: &[&str]) -> IIterable<HSTRING> {
    keys.iter().map(|k| HSTRING::from(*k)).collect::<Vec<_>>().into()
}

/// The paired devices of both radios. The selectors are Windows' own, which
/// list what is paired without searching the air for anything.
fn selector() -> Result<HSTRING> {
    let classic = BluetoothDevice::GetDeviceSelectorFromPairingState(true)?;
    let le = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?;
    Ok(HSTRING::from(format!("({classic}) OR ({le})")))
}

fn wanted() -> IIterable<HSTRING> {
    strings(&[ADDRESS, CONNECTED, PRESENT, MAJOR, MINOR, APPEARANCE])
}

/// Windows goes on listing a link as connected when the radio was switched off
/// under it, so the device itself is asked. Asking connects nothing.
fn is_up(id: &str) -> bool {
    let name = HSTRING::from(id);
    let status = if id.starts_with("BluetoothLE") {
        BluetoothLEDevice::FromIdAsync(&name).and_then(|op| op.get()).and_then(|d| d.ConnectionStatus())
    } else {
        BluetoothDevice::FromIdAsync(&name).and_then(|op| op.get()).and_then(|d| d.ConnectionStatus())
    };
    matches!(status, Ok(BluetoothConnectionStatus::Connected))
}

/// `None` for what is not an accessory.
fn link(info: &DeviceInformation) -> Option<Link> {
    let props = info.Properties().ok()?;
    let id = info.Id().ok()?.to_string();
    // The id ends with the address too ("Bluetooth#Bluetooth<radio>-<device>").
    let address = text(&props, ADDRESS).or_else(|| id.rsplit('-').next().map(str::to_owned))?;
    Some(Link {
        address: address.replace(':', "").to_uppercase(),
        name: info.Name().map(|n| n.to_string()).unwrap_or_default(),
        kind: kind(num(&props, MAJOR), num(&props, MINOR).unwrap_or(0), num(&props, APPEARANCE))?,
        connected: flag(&props, CONNECTED).unwrap_or(false) && is_up(&id),
        id,
    })
}

fn merged<'a>(links: impl IntoIterator<Item = &'a Link>) -> Vec<Device> {
    let mut out: Vec<Device> = Vec::new();
    for l in links {
        let le = (l.connected && l.id.starts_with("BluetoothLE")).then(|| l.id.clone());
        match out.iter_mut().find(|d| d.address == l.address) {
            Some(d) => {
                d.connected |= l.connected;
                d.le = d.le.take().or(le);
                if d.kind == Kind::Other {
                    d.kind = l.kind;
                }
                if d.name.is_empty() {
                    d.name = l.name.clone();
                }
            }
            None => out.push(Device { address: l.address.clone(), name: l.name.clone(), kind: l.kind, connected: l.connected, battery: None, le }),
        }
    }
    out
}

/// The battery level of every Bluetooth device node that carries one, by
/// instance id in capitals (the id holds the device's address). Windows keeps
/// the last reading while the device is away.
fn node_batteries() -> Vec<(String, u8)> {
    let read = || -> Result<Vec<(String, u8)>> {
        let filter = HSTRING::from(format!(r#"{INSTANCE}:~<"BTH""#));
        let nodes = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&filter, &strings(&[INSTANCE, BATTERY]), DeviceInformationKind::Device)?.get()?;
        Ok(nodes
            .into_iter()
            .filter_map(|node| {
                let props = node.Properties().ok()?;
                let percent = num(&props, BATTERY).filter(|p| *p <= 100)? as u8;
                Some((text(&props, INSTANCE)?.to_uppercase(), percent))
            })
            .collect())
    };
    read().unwrap_or_default()
}

/// The battery services of an LE device that is connected: one level, or one
/// per earbud when the device says which side a level is for. The services are
/// the ones Windows already knows; only the levels are asked of the device.
fn gatt_battery(id: &str) -> Result<Battery> {
    let mut battery = Battery::default();
    let device = BluetoothLEDevice::FromIdAsync(&HSTRING::from(id))?.get()?;
    if device.ConnectionStatus()? != BluetoothConnectionStatus::Connected {
        return Ok(battery);
    }
    for service in device.GetGattServicesForUuidWithCacheModeAsync(GattServiceUuids::Battery()?, BluetoothCacheMode::Cached)?.get()?.Services()? {
        let levels = service.GetCharacteristicsForUuidWithCacheModeAsync(GattCharacteristicUuids::BatteryLevel()?, BluetoothCacheMode::Cached)?.get()?;
        for level in levels.Characteristics()? {
            let read = level.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?.get()?;
            if read.Status()? != GattCommunicationStatus::Success {
                continue;
            }
            let percent = DataReader::FromBuffer(&read.Value()?)?.ReadByte()?.min(100);
            // The presentation format says the side: 0x010D left, 0x010E right (GATT namespace descriptions).
            match level.PresentationFormats()?.into_iter().find_map(|f| f.Description().ok()) {
                Some(0x010D) => battery.left = Some(percent),
                Some(0x010E) => battery.right = Some(percent),
                _ => battery.percent = Some(percent),
            }
        }
    }
    Ok(battery)
}

fn with_battery(mut device: Device, nodes: &[(String, u8)]) -> Device {
    let mut battery = device.le.as_deref().and_then(|id| gatt_battery(id).ok()).unwrap_or_default();
    if battery == Battery::default() {
        battery.percent = nodes.iter().find(|(id, _)| id.contains(&device.address)).map(|(_, percent)| *percent);
    }
    device.battery = (battery != Battery::default()).then_some(battery);
    device
}

/// The device with its battery when Windows says it within `wait`, and as it is otherwise.
pub fn battery_within(device: Device, wait: Duration) -> Device {
    let (tx, rx) = mpsc::channel();
    let asked = device.clone();
    std::thread::spawn(move || tx.send(with_battery(asked, &node_batteries())));
    rx.recv_timeout(wait).unwrap_or(device)
}

/// Every paired accessory with the battery Windows has for it. Read once, on the caller's thread.
pub fn devices() -> Result<Vec<Device>> {
    let found = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&selector()?, &wanted(), DeviceInformationKind::AssociationEndpoint)?.get()?;
    let links: Vec<Link> = found.into_iter().filter_map(|info| link(&info)).collect();
    let nodes = node_batteries();
    Ok(merged(&links).into_iter().map(|d| with_battery(d, &nodes)).collect())
}

// ── The watcher ───────────────────────────────────────────────────────────────

#[derive(Default)]
struct State {
    links: HashMap<String, Link>,
    rule: Connections,
}

struct Shared {
    state: Mutex<State>,
    alive: AtomicBool,
    tell: Box<dyn Fn(Device) + Send + Sync>,
}

impl Shared {
    /// A link is up or down (or new): note it and, if that is news, say so once it has settled.
    fn note(self: &Arc<Self>, link: Link) {
        let address = link.address.clone();
        let news = {
            let mut state = self.state.lock().unwrap();
            let news = state.rule.feed(&address, &link.id, link.connected);
            state.links.insert(link.id.clone(), link);
            news
        };
        if !news {
            return;
        }
        let shared = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SETTLE);
            let device = {
                let mut state = shared.state.lock().unwrap();
                state.rule.settle(&address).and_then(|_| merged(state.links.values().filter(|l| l.address == address)).pop())
            };
            let Some(device) = device else { return };
            let device = if device.connected { battery_within(device, BATTERY_WAIT) } else { device };
            if shared.alive.load(Relaxed) {
                (shared.tell)(device);
            }
        });
    }
}

/// Dropping it ends the watcher; what was about to be told is not told.
pub struct Watch {
    watcher: DeviceWatcher,
    shared: Arc<Shared>,
}

impl Watch {
    /// How many links Windows listed, once the first listing is over (the probe asks).
    pub fn listed(&self) -> Option<usize> {
        let state = self.shared.state.lock().unwrap();
        state.rule.primed.then(|| state.links.len())
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.shared.alive.store(false, Relaxed);
        let _ = self.watcher.Stop();
    }
}

/// Tells of every accessory that connects or disconnects from now on, with its
/// battery when it connected. What is connected already is not told.
pub fn watch(tell: impl Fn(Device) + Send + Sync + 'static) -> Result<Watch> {
    let watcher = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(&selector()?, &wanted(), DeviceInformationKind::AssociationEndpoint)?;
    let shared = Arc::new(Shared { state: Mutex::default(), alive: AtomicBool::new(true), tell: Box::new(tell) });

    let on = shared.clone();
    watcher.Added(&TypedEventHandler::new(move |_, info: Ref<DeviceInformation>| {
        if let Some(link) = info.as_ref().and_then(link) {
            on.note(link);
        }
        Ok(())
    }))?;
    // An update names the link and carries only what changed.
    let changed = |shared: &Arc<Shared>, update: Ref<DeviceInformationUpdate>, removed: bool| {
        let Some(update) = update.as_ref() else { return };
        let Ok(id) = update.Id().map(|id| id.to_string()) else { return };
        let Some(link) = shared.state.lock().unwrap().links.get(&id).cloned() else { return };
        // What Windows says now is believed. Anything else that changed (the radio
        // went off under the link, or came back) is a reason to ask the device.
        let says = update.Properties().ok().and_then(|props| flag(&props, CONNECTED));
        let up = !removed && says.unwrap_or_else(|| is_up(&id));
        if up != link.connected {
            shared.note(Link { connected: up, ..link });
        }
    };
    let on = shared.clone();
    watcher.Updated(&TypedEventHandler::new(move |_, update| {
        changed(&on, update, false);
        Ok(())
    }))?;
    let on = shared.clone();
    watcher.Removed(&TypedEventHandler::new(move |_, update| {
        changed(&on, update, true);
        Ok(())
    }))?;
    let on = shared.clone();
    watcher.EnumerationCompleted(&TypedEventHandler::new(move |_, _| {
        on.state.lock().unwrap().rule.prime();
        Ok(())
    }))?;
    watcher.Start()?;
    Ok(Watch { watcher, shared })
}
