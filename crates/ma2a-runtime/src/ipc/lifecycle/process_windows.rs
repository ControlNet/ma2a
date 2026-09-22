#![allow(unsafe_code, reason = "audited Windows process-times FFI boundary")]

//! Reading a Windows process creation time, which has no safe equivalent.

use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME},
    System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
};

const EMPTY: FILETIME = FILETIME {
    dwLowDateTime: 0,
    dwHighDateTime: 0,
};

/// Returns the process creation time as a 100-nanosecond file time.
///
/// `None` means the process could not be opened or queried, which includes the
/// case where it no longer exists. Callers treat that as "cannot tell" and fall
/// back to an authority that does not depend on the platform.
pub(super) fn started_at(pid: u32) -> Option<u64> {
    // SAFETY: the returned handle is checked before any use and closed on every
    // path out of this function.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut created = EMPTY;
    let mut ignored = [EMPTY; 3];
    // SAFETY: `handle` came from a successful OpenProcess above, and every out
    // parameter points at an owned, initialized FILETIME that outlives the call.
    let queried = unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut ignored[0],
            &raw mut ignored[1],
            &raw mut ignored[2],
        )
    };
    // SAFETY: `handle` came from a successful OpenProcess and is closed once.
    let _closed = unsafe { CloseHandle(handle) };
    if queried == 0 {
        return None;
    }
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}
