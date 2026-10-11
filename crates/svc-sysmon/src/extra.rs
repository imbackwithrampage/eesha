// What a detail page asks for once: the USB devices plugged in, this
// machine's addresses, and ejecting a removable volume. Plain calls on the
// caller's thread.

use std::net::Ipv4Addr;
use std::time::Duration;

use serde::Serialize;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDevicePropertyW,
    SetupDiGetDeviceRegistryPropertyW, CM_DEVCAP_REMOVABLE, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SETUP_DI_REGISTRY_PROPERTY,
    SPDRP_CAPABILITIES, SPDRP_DEVICEDESC, SPDRP_SERVICE, SP_DEVINFO_DATA,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_BusReportedDeviceDesc, DEVPROPTYPE};
use windows::Win32::Devices::Usb::GUID_DEVINTERFACE_USB_DEVICE;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::SOCKADDR_IN;
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_STORAGE_EJECT_MEDIA, IOCTL_STORAGE_MEDIA_REMOVAL, PREVENT_MEDIA_REMOVAL,
};
use windows::Win32::System::IO::DeviceIoControl;

fn text(bytes: &[u8]) -> String {
    let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
    String::from_utf16_lossy(&wide)
}

unsafe fn registry(set: HDEVINFO, data: &SP_DEVINFO_DATA, property: SETUP_DI_REGISTRY_PROPERTY) -> Vec<u8> {
    let mut buf = vec![0u8; 512];
    let mut len = 0u32;
    let ok = unsafe { SetupDiGetDeviceRegistryPropertyW(set, data, property, None, Some(&mut buf), Some(&mut len)) }.is_ok();
    buf.truncate(if ok { len as usize } else { 0 });
    buf
}

/// The USB devices plugged into this machine from outside, by the name each
/// gives itself. Hubs and what is built in (camera, fingerprint reader,
/// Bluetooth radio: not removable) are left out.
pub fn usb_devices() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let flags = DIGCF_PRESENT | DIGCF_DEVICEINTERFACE;
        let Ok(set) = SetupDiGetClassDevsW(Some(&GUID_DEVINTERFACE_USB_DEVICE), PCWSTR::null(), None, flags) else { return out };
        for i in 0.. {
            let mut data = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, i, &mut data).is_err() {
                break;
            }
            let caps = registry(set, &data, SPDRP_CAPABILITIES);
            let removable = caps.len() >= 4 && u32::from_le_bytes([caps[0], caps[1], caps[2], caps[3]]) & CM_DEVCAP_REMOVABLE.0 != 0;
            if !removable || text(&registry(set, &data, SPDRP_SERVICE)).to_lowercase().starts_with("usbhub") {
                continue;
            }
            // What the device says it is ("USB Receiver"), else what Windows calls its kind.
            let mut buf = vec![0u8; 512];
            let (mut kind, mut len) = (DEVPROPTYPE(0), 0u32);
            let said = SetupDiGetDevicePropertyW(set, &data, &DEVPKEY_Device_BusReportedDeviceDesc, &mut kind, Some(&mut buf), Some(&mut len), 0);
            let mut name = if said.is_ok() { text(&buf[..len as usize]) } else { String::new() };
            if name.trim().is_empty() {
                name = text(&registry(set, &data, SPDRP_DEVICEDESC));
            }
            out.push(name.trim().to_owned());
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    out.sort();
    out
}

#[derive(Clone, Debug, Serialize)]
pub struct Address {
    pub ip: String,
    /// The connection's name ("Wi-Fi").
    pub name: String,
}

/// This machine's IPv4 addresses on the connections that are up. Nothing is
/// asked of the network.
pub fn addresses() -> Vec<Address> {
    let mut out = Vec::new();
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut buf = vec![0u64; 4096];
    unsafe {
        let mut size = (buf.len() * 8) as u32;
        let first = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        // 2 = AF_INET. debt: a machine with more adapters than fit in 32 KB lists none.
        if GetAdaptersAddresses(2, flags, None, Some(first), &mut size) != 0 {
            return out;
        }
        let mut adapter = first as *const IP_ADAPTER_ADDRESSES_LH;
        while let Some(a) = adapter.as_ref() {
            // 24 = loopback.
            if a.OperStatus == IfOperStatusUp && a.IfType != 24 {
                let mut unicast = a.FirstUnicastAddress;
                while let Some(u) = unicast.as_ref() {
                    if let Some(sa) = (u.Address.lpSockaddr as *const SOCKADDR_IN).as_ref() {
                        let ip = Ipv4Addr::from(sa.sin_addr.S_un.S_addr.to_ne_bytes());
                        if sa.sin_family.0 == 2 && !ip.is_link_local() {
                            out.push(Address { ip: ip.to_string(), name: a.FriendlyName.to_string().unwrap_or_default() });
                        }
                    }
                    unicast = u.Next;
                }
            }
            adapter = a.Next;
        }
    }
    out
}

/// Why a volume was not ejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EjectError {
    /// Not a volume that comes out (or no longer there).
    Unknown,
    /// A program still has a file open on it.
    Busy,
    Failed,
}

/// Ejects a removable volume ("E:"): lock it (which fails while any file on
/// it is open, and then nothing is done), dismount it, let the media go.
/// Only ever on a click.
// debt: written to the documented recipe, never run against a real drive; a
// USB hard disk (fixed media) needs CM_Request_Device_Eject instead and is refused here.
pub fn eject(letter: &str) -> Result<(), EjectError> {
    let known = crate::sample::volumes().into_iter().any(|v| v.ejectable && v.letter.eq_ignore_ascii_case(letter));
    if !known {
        return Err(EjectError::Unknown);
    }
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE;
    let access = (GENERIC_READ | GENERIC_WRITE).0;
    let path = HSTRING::from(format!(r"\\.\{letter}"));
    unsafe {
        let handle = CreateFileW(&path, access, share, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None).map_err(|_| EjectError::Failed)?;
        let control = |code: u32, input: Option<&PREVENT_MEDIA_REMOVAL>| {
            let mut got = 0u32;
            let (ptr, len) = input.map_or((None, 0), |i| (Some(i as *const _ as *const _), std::mem::size_of::<PREVENT_MEDIA_REMOVAL>() as u32));
            DeviceIoControl(handle, code, ptr, len, None, 0, Some(&mut got), None).is_ok()
        };
        // A scanner or the indexer may hold it for a moment.
        let locked = (0..5).any(|i| {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(200));
            }
            control(FSCTL_LOCK_VOLUME, None)
        });
        let done = if !locked {
            Err(EjectError::Busy)
        } else if control(FSCTL_DISMOUNT_VOLUME, None)
            && control(IOCTL_STORAGE_MEDIA_REMOVAL, Some(&PREVENT_MEDIA_REMOVAL { PreventMediaRemoval: false }))
            && control(IOCTL_STORAGE_EJECT_MEDIA, None)
        {
            Ok(())
        } else {
            Err(EjectError::Failed)
        };
        let _ = CloseHandle(handle);
        done
    }
}
