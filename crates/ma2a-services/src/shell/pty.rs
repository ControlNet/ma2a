//! Blocking PTY/ConPTY session bridged to bounded asynchronous channels.

use std::io::{Read as _, Write as _};

use ma2a_core::service::{
    MAX_DATA_CHUNK_LEN, ServiceError, ShellRequest, TerminalSize, Termination,
};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{mpsc, oneshot};

use crate::process;

/// Output chunks buffered between the PTY reader thread and the network, bounding memory.
const OUTPUT_QUEUE: usize = 2;
/// Input chunks buffered between the network and the PTY writer thread.
const INPUT_QUEUE: usize = 2;

/// One live session: its terminal, channels to its I/O threads, and its killer.
pub(super) struct Session {
    pub(super) master: std::sync::Mutex<Box<dyn MasterPty + Send>>,
    pub(super) output: mpsc::Receiver<Vec<u8>>,
    pub(super) input: mpsc::Sender<Vec<u8>>,
    pub(super) exited: oneshot::Receiver<Termination>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    #[cfg(unix)]
    group: Option<u32>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Session").finish_non_exhaustive()
    }
}

pub(super) const fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows(),
        cols: size.cols(),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn default_program() -> String {
    #[cfg(unix)]
    let (variable, fallback) = ("SHELL", "/bin/sh");
    #[cfg(windows)]
    let (variable, fallback) = ("COMSPEC", "cmd.exe");
    std::env::var(variable).unwrap_or_else(|_| fallback.to_owned())
}

/// Opens a terminal and starts the session program on it.
pub(super) fn spawn(request: &ShellRequest) -> Result<Session, ServiceError> {
    let spec = request.spec();
    let program = spec.program().map_or_else(default_program, str::to_owned);
    let mut command = CommandBuilder::new(program);
    command.args(spec.args());
    command.cwd(process::working_directory(spec)?);
    command.env("TERM", request.term());
    for (name, value) in spec.env() {
        command.env(name, value);
    }
    let pair = native_pty_system()
        .openpty(pty_size(request.size()))
        .map_err(|_| ServiceError::SpawnFailed)?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|_| ServiceError::SpawnFailed)?;
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| ServiceError::SpawnFailed);
    let writer = pair
        .master
        .take_writer()
        .map_err(|_| ServiceError::SpawnFailed);
    let (Ok(reader), Ok(writer)) = (reader, writer) else {
        let _killed = child.kill();
        return Err(ServiceError::SpawnFailed);
    };
    let killer = child.clone_killer();
    #[cfg(unix)]
    let group = child.process_id();
    let (output_sender, output) = mpsc::channel(OUTPUT_QUEUE);
    let (input, input_receiver) = mpsc::channel(INPUT_QUEUE);
    let (exit_sender, exited) = oneshot::channel();
    tokio::task::spawn_blocking(move || pump_output(reader, &output_sender));
    tokio::task::spawn_blocking(move || pump_input(writer, input_receiver));
    tokio::task::spawn_blocking(move || {
        let status = child.wait().ok();
        let termination = status.map_or(Termination::Unknown, |status| {
            if status.signal().is_some() {
                Termination::Unknown
            } else {
                Termination::Exited(status.exit_code())
            }
        });
        let _sent = exit_sender.send(termination);
    });
    Ok(Session {
        master: std::sync::Mutex::new(pair.master),
        output,
        input,
        exited,
        killer,
        #[cfg(unix)]
        group,
    })
}

impl Session {
    /// Kills the session program and, on Unix, its process group.
    ///
    /// Jobs a shell moved into other process groups receive SIGHUP when the terminal closes.
    pub(super) fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self
            .group
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _signalled =
                rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _killed = self.killer.kill();
    }
}

fn pump_output(mut reader: Box<dyn std::io::Read + Send>, output: &mpsc::Sender<Vec<u8>>) {
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let chunk = buffer.get(..read).unwrap_or_default().to_vec();
        if output.blocking_send(chunk).is_err() {
            return;
        }
    }
}

fn pump_input(mut writer: Box<dyn std::io::Write + Send>, mut input: mpsc::Receiver<Vec<u8>>) {
    while let Some(chunk) = input.blocking_recv() {
        if writer
            .write_all(&chunk)
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}
