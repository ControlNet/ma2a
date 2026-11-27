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

/// Ends the session program and (on Unix) every process in its session before terminal teardown.
pub(super) struct Terminator {
    killer: Box<dyn ChildKiller + Send + Sync>,
    #[cfg(unix)]
    group: Option<u32>,
}

impl Terminator {
    #[cfg(unix)]
    pub(super) async fn terminate(mut self, leader_running: bool) {
        if let Some(leader) = self
            .group
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            super::reap::end_session(leader, leader_running).await;
        }
        let _killed = self.killer.kill();
    }

    #[cfg(windows)]
    #[expect(
        clippy::unused_async,
        reason = "the Unix counterpart waits for jobs to hang up"
    )]
    pub(super) async fn terminate(mut self, _leader_running: bool) {
        let _killed = self.killer.kill();
    }
}

/// One live session: its terminal, channels to its I/O threads, and its killer.
pub(super) struct Session {
    pub(super) master: std::sync::Mutex<Box<dyn MasterPty + Send>>,
    pub(super) output: mpsc::Receiver<Vec<u8>>,
    pub(super) input: mpsc::Sender<Vec<u8>>,
    pub(super) exited: oneshot::Receiver<Termination>,
    terminator: Option<Terminator>,
    /// The terminal's device path and the end of the input thread, to release a blocked write.
    #[cfg(unix)]
    input_thread: (Option<std::path::PathBuf>, oneshot::Receiver<()>),
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
    let (input_ended, input_done) = oneshot::channel();
    #[cfg(unix)]
    let input_thread = (pair.master.tty_name(), input_done);
    #[cfg(windows)]
    drop(input_done);
    tokio::task::spawn_blocking(move || pump_output(reader, &output_sender));
    tokio::task::spawn_blocking(move || {
        pump_input(writer, input_receiver);
        let _sent = input_ended.send(());
    });
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
        terminator: Some(Terminator {
            killer,
            #[cfg(unix)]
            group,
        }),
        #[cfg(unix)]
        input_thread,
    })
}

impl Session {
    /// Extracts the process terminator so outbound stop handling can end the session before waiting
    /// to finish an in-flight frame.
    pub(super) const fn take_terminator(&mut self) -> Option<Terminator> {
        self.terminator.take()
    }

    /// Ends the session program and every job it started, then releases the terminal.
    ///
    /// `leader_running` is false once the program has exited by itself.
    #[cfg(unix)]
    pub(super) async fn close(self, leader_running: bool) {
        let Self {
            master,
            input,
            terminator,
            input_thread: (terminal, mut input_done),
            ..
        } = self;
        if let Some(terminator) = terminator {
            terminator.terminate(leader_running).await;
        }
        drop(input);
        super::reap::release_input(terminal.as_deref(), &mut input_done).await;
        drop(master);
    }

    /// Ends the session program; closing the pseudoconsole ends its attached clients.
    #[cfg(windows)]
    pub(super) async fn close(mut self, leader_running: bool) {
        if let Some(terminator) = self.terminator.take() {
            terminator.terminate(leader_running).await;
        }
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
