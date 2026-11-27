#![allow(unsafe_code, reason = "audited Windows named-pipe FFI boundary")]

//! Watching a Windows named-pipe client process for exit when unread bytes remain buffered.
//!
//! When a named pipe client closes or terminates while unread bytes remain in the pipe buffer,
//! Windows keeps the server pipe in `FILE_PIPE_CONNECTED_STATE` (for `ReadFile`, `PeekNamedPipe`,
//! `GetNamedPipeClientProcessId`, and `NtQueryInformationFile`) until the inbound queue drains.
//! Waiting on the peer's process handle detects a terminated client process out of band while
//! keeping live clients undisturbed.

use std::{
    io,
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::SYNCHRONIZE,
    System::Threading::{OpenProcess, WaitForSingleObject},
};

use super::windows::PlatformStream;

/// Resolves once the connected named-pipe peer process exits or the pipe reports an error.
pub(super) async fn wait_peer_disconnect(stream: &PlatformStream) {
    let PlatformStream::NamedPipe(pipe) = stream;
    let Ok(peer_pid) = pipe.inner().peer_process_id() else {
        return;
    };
    if peer_pid == std::process::id() {
        // In-process peers cannot exit independently; fall back to in-band EOF.
        std::future::pending::<()>().await;
        return;
    }
    // SAFETY: [Category 8 - FFI boundary] `OpenProcess` takes a plain integer PID and returns
    // either null or a new owned process handle checked before wrapping.
    let raw = unsafe { OpenProcess(SYNCHRONIZE, 0, peer_pid) };
    if raw.is_null() {
        if io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER.cast_signed())
        {
            return;
        }
        std::future::pending::<()>().await;
        return;
    }
    // SAFETY: [Category 8 - FFI boundary] `raw` is a non-null handle returned by `OpenProcess`
    // above and is uniquely owned by `process`.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    loop {
        // SAFETY: [Category 8 - FFI boundary] `process` is a live `SYNCHRONIZE`-capable handle
        // owned for the duration of this call; timeout 0 polls without blocking.
        let wait = unsafe { WaitForSingleObject(process.as_raw_handle(), 0) };
        if wait == WAIT_OBJECT_0 || pipe.inner().peer_process_id().is_err() {
            return;
        }
        if wait != WAIT_TIMEOUT {
            std::future::pending::<()>().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
