//! Windows console control through the console-mode API.
#![allow(unsafe_code, reason = "audited Windows console-mode FFI boundary")]

use std::io;

use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE},
    System::Console::{
        CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
        ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, STD_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetConsoleMode,
    },
};

/// Puts the console in raw virtual-terminal mode until dropped, then restores both modes.
#[derive(Debug)]
pub struct RawMode {
    input: CONSOLE_MODE,
    output: CONSOLE_MODE,
}

impl RawMode {
    /// Enables raw mode, or returns `None` when stdin is not a console.
    ///
    /// # Errors
    /// Returns the console failure of a console that refused the change.
    pub fn enable() -> io::Result<Option<Self>> {
        let (Some(input), Some(output)) = (mode(STD_INPUT_HANDLE), mode(STD_OUTPUT_HANDLE)) else {
            return Ok(None);
        };
        let raw_input = (input & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        set_mode(STD_INPUT_HANDLE, raw_input)?;
        if let Err(error) = set_mode(
            STD_OUTPUT_HANDLE,
            output | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        ) {
            let _restored = set_mode(STD_INPUT_HANDLE, input);
            return Err(error);
        }
        Ok(Some(Self { input, output }))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _input = set_mode(STD_INPUT_HANDLE, self.input);
        let _output = set_mode(STD_OUTPUT_HANDLE, self.output);
    }
}

/// Returns the standard handle, or `None` when the process has none.
fn handle(which: STD_HANDLE) -> Option<HANDLE> {
    // SAFETY: `GetStdHandle` takes a plain constant and only reads process state.
    let handle = unsafe { GetStdHandle(which) };
    (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(handle)
}

/// Returns the console mode of a standard handle, or `None` when it is not a console.
fn mode(which: STD_HANDLE) -> Option<CONSOLE_MODE> {
    let handle = handle(which)?;
    let mut mode: CONSOLE_MODE = 0;
    // SAFETY: `handle` is a live standard handle and `mode` is a valid, writable
    // `CONSOLE_MODE` for the duration of the call.
    let succeeded = unsafe { GetConsoleMode(handle, &raw mut mode) };
    (succeeded != 0).then_some(mode)
}

fn set_mode(which: STD_HANDLE, mode: CONSOLE_MODE) -> io::Result<()> {
    let handle = handle(which).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    // SAFETY: `handle` is a live standard handle; the mode is passed by value.
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Returns the columns and rows of the visible console window.
pub(super) fn size() -> Option<(u16, u16)> {
    let handle = handle(STD_OUTPUT_HANDLE)?;
    // SAFETY: an all-zero `CONSOLE_SCREEN_BUFFER_INFO` is a valid plain-data value.
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: `handle` is a live standard handle and `info` is valid and writable for the
    // duration of the call.
    if unsafe { GetConsoleScreenBufferInfo(handle, &raw mut info) } == 0 {
        return None;
    }
    let window = info.srWindow;
    let cols = u16::try_from(i32::from(window.Right) - i32::from(window.Left) + 1).ok()?;
    let rows = u16::try_from(i32::from(window.Bottom) - i32::from(window.Top) + 1).ok()?;
    Some((cols, rows))
}

/// Console resize notifications, polled because a console reports resizes only as input.
pub(super) struct ResizeEvents(tokio::time::Interval);

impl ResizeEvents {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "matches the Unix constructor, whose signal registration can fail"
    )]
    pub(super) fn new() -> io::Result<Self> {
        let mut interval = tokio::time::interval(super::RESIZE_POLL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Ok(Self(interval))
    }

    /// Waits for the next poll; the caller compares sizes.
    pub(super) async fn next(&mut self) -> bool {
        self.0.tick().await;
        true
    }
}
