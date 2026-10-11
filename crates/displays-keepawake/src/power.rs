// A keep-awake request: PowerCreateRequest, not SetThreadExecutionState, which
// belongs to one thread. Dropping it clears every request type it set, so the
// machine may sleep again; a request also dies with the process.

use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Power::{
    PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestExecutionRequired,
    PowerRequestSystemRequired, PowerSetRequest, POWER_REQUEST_TYPE,
};
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};

pub struct Request {
    handle: HANDLE,
    set: Vec<POWER_REQUEST_TYPE>,
}

// The handle is only used to set and clear requests, which any thread may do.
unsafe impl Send for Request {}

impl Request {
    /// System always; the display too unless it may sleep.
    pub fn new(display_may_sleep: bool) -> Result<Request, String> {
        unsafe {
            let ctx = REASON_CONTEXT {
                Version: 0,
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                Reason: REASON_CONTEXT_0 { SimpleReasonString: PWSTR(w!("Keep awake").as_ptr() as *mut u16) },
            };
            let handle = PowerCreateRequest(&ctx).map_err(|e| e.message())?;
            // Dropped half-built, it clears what it did set.
            let mut request = Request { handle, set: Vec::new() };
            let mut kinds = vec![PowerRequestSystemRequired, PowerRequestExecutionRequired];
            if !display_may_sleep {
                kinds.push(PowerRequestDisplayRequired);
            }
            for k in kinds {
                PowerSetRequest(request.handle, k).map_err(|e| e.message())?;
                request.set.push(k);
            }
            Ok(request)
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        unsafe {
            for k in &self.set {
                let _ = PowerClearRequest(self.handle, *k);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}
