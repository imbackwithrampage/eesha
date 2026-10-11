// What VOICE §3.3 demands of every child process the voice assistant starts (the action model's server,
// the speech server, the text model's server, one speech-to-text run): no console window, the
// no-telemetry environment, one kill-on-close job, 127.0.0.1 on a free port whose listener's process is
// checked, and an end for all of them at once when voice is switched off or paused.
//
// Real code, written with the skeleton: four packages start children and none writes this again.
// Needs only std and `windows`, so a probe uses it without the app (examples/voice-proc-probe.rs).

use crate::{Result, VoiceError};
use std::net::{Ipv4Addr, TcpListener};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Set on every child, whatever it is. An engine with a switch of its own (a flag, another variable)
/// gets that one from its module as well.
pub const NO_TELEMETRY: [(&str, &str); 2] = [("NEEDLE_TELEMETRY", "0"), ("DO_NOT_TRACK", "1")];

/// The one job every child is put in. Its handle is never closed and never inherited, so Windows
/// closes it when this process ends, however it ends, and the job then ends what is in it.
static JOB: OnceLock<std::result::Result<isize, String>> = OnceLock::new();

fn job() -> Result<HANDLE> {
    JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null()).map_err(|e| e.message())?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32)
            .map_err(|e| e.message())?;
        Ok(job.0 as isize)
    })
    .clone()
    .map(|raw| HANDLE(raw as *mut _))
    .map_err(|e| VoiceError::Failed(format!("job object: {e}")))
}

/// A child in the job. Dropping it ends it; `.0` is the std handle (its pipes, `as_raw_handle` for
/// priority and affinity).
pub struct Child(pub std::process::Child);

impl Child {
    pub fn pid(&self) -> u32 {
        self.0.id()
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Starts `cmd` as §3.3 wants it: no console, `NO_TELEMETRY`, in the job. The caller sets the program,
/// its arguments, its folder and its pipes (an unset stdin, stdout or stderr is the app's own: null).
/// A child that cannot be put in the job is not left running.
pub fn spawn(cmd: &mut Command) -> Result<Child> {
    let job = job()?;
    cmd.creation_flags(CREATE_NO_WINDOW).envs(NO_TELEMETRY);
    let mut child = Child(cmd.spawn().map_err(|e| VoiceError::Failed(format!("could not start the engine: {e}")))?);
    // ponytail: the child runs for a moment before it is in the job, so a grandchild started in that
    // moment would escape; none of our engines starts one. CREATE_SUSPENDED needs CreateProcessW by hand.
    if let Err(e) = unsafe { AssignProcessToJobObject(job, HANDLE(child.0.as_raw_handle() as _)) } {
        child.kill();
        return Err(VoiceError::Failed(format!("job object: {}", e.message())));
    }
    Ok(child)
}

/// Ends every child at once: voice off, tray Pause, lock, sleep, energy saver, quit. The job stays for
/// the next ones; a module finds its child gone through `Child::alive`.
pub fn kill_all() {
    if let Some(Ok(raw)) = JOB.get() {
        unsafe {
            let _ = TerminateJobObject(HANDLE(*raw as *mut _), 1);
        }
    }
}

/// A port nobody listens on, on 127.0.0.1. It is free now, not promised: give it to the child, then
/// `wait_listening`.
pub fn free_port() -> Result<u16> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| VoiceError::Failed(format!("no free port: {e}")))
}

/// Who listens on a TCP port over IPv4: the address bound and the process.
pub fn listeners(port: u16) -> Result<Vec<(Ipv4Addr, u32)>> {
    let mut size = 0u32;
    let mut buf: Vec<u32> = Vec::new();
    // The table changes between the calls: until it fits (as svc-sysmon reads it).
    for _ in 0..8 {
        let ptr = (!buf.is_empty()).then(|| buf.as_mut_ptr() as *mut _);
        match unsafe { GetExtendedTcpTable(ptr, &mut size, false, 2, TCP_TABLE_OWNER_PID_LISTENER, 0) } {
            0 => {
                // The first u32 is the count, the rows follow.
                let Some(count) = buf.first() else { return Ok(Vec::new()) };
                let rows = unsafe { std::slice::from_raw_parts(buf.as_ptr().add(1) as *const MIB_TCPROW_OWNER_PID, *count as usize) };
                return Ok(rows
                    .iter()
                    .filter(|r| u16::from_be(r.dwLocalPort as u16) == port)
                    .map(|r| (Ipv4Addr::from(r.dwLocalAddr.to_ne_bytes()), r.dwOwningPid))
                    .collect());
            }
            // ERROR_INSUFFICIENT_BUFFER
            122 => buf.resize(size as usize / 4 + 16, 0),
            code => return Err(VoiceError::Failed(format!("GetExtendedTcpTable: error {code}"))),
        }
        size = (buf.len() * 4) as u32;
    }
    Err(VoiceError::Failed("GetExtendedTcpTable: the table kept growing".into()))
}

/// Waits until `child`, and nobody else, listens on 127.0.0.1:`port`. Another process on the port (the
/// spike saw two servers share one), an address beyond this computer, a child that ended or `timeout`
/// is an error, and the caller drops the child.
pub fn wait_listening(child: &mut Child, port: u16, timeout: Duration) -> Result<()> {
    let end = Instant::now() + timeout;
    loop {
        let on = listeners(port)?;
        if let Some((_, pid)) = on.iter().find(|(_, pid)| *pid != child.pid()) {
            return Err(VoiceError::Failed(format!("port {port} is held by another process ({pid})")));
        }
        if on.iter().any(|(address, _)| !address.is_loopback()) {
            return Err(VoiceError::Refused(format!("the engine listens beyond this computer on port {port}")));
        }
        if !on.is_empty() {
            return Ok(());
        }
        if !child.alive() {
            return Err(VoiceError::Failed("the engine ended before it listened".into()));
        }
        if Instant::now() >= end {
            return Err(VoiceError::Timeout("the engine"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
