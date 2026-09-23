//! Running the daemon that owns one state directory.

use std::{
    fs::{File, OpenOptions, TryLockError},
    io::{self, Write as _},
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use ma2a_runtime::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{DaemonIdentity, DaemonReport, IpcPaths, LocalApiServer, RuntimeBoot, ServerExit},
    web::{SystemClock, WebAuthConfig},
};
use ma2a_store::StoreConfig;
use tokio_util::sync::CancellationToken;

use crate::{
    AppError,
    daemon_control::{LAUNCH_NONCE_VARIABLE, acquire_startup_lock, endpoint_answers, open_lock},
};

/// Bounds a graceful Runtime shutdown before this process leaves regardless.
///
/// A Runtime that will not finish closing would otherwise keep the daemon, and
/// with it the singleton lock, alive indefinitely: the caller that asked it to
/// stop would wait for a release that never comes, and so would every later
/// start. Leaving on a bound is the escalation, and the operating system then
/// releases the lock and every handle as this process ends.
const RUNTIME_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);

/// Keeps the singleton lock open for exactly as long as this process lives.
///
/// The lock is never closed deliberately. Releasing it is left to the operating
/// system, which does it as part of this process ending, so another process
/// acquiring it has *observed* this daemon exit rather than inferred it from a
/// quiet socket. Dropping the lock during teardown instead would open a window
/// in which a replacement starts while this Runtime is still closing, which is
/// how one state directory ends up with two of them.
static SINGLETON_LOCK: OnceLock<File> = OnceLock::new();

pub(crate) async fn run(
    state_dir: PathBuf,
    paths: IpcPaths,
    detach_session: bool,
) -> Result<(), AppError> {
    #[cfg(unix)]
    if detach_session {
        rustix::process::setsid().map_err(io::Error::from)?;
    }
    paths.prepare()?;
    // The parent launcher holds startup.lock for daemon-detached. A foreground
    // daemon joins that same transition order and releases it at readiness.
    let startup_lock = if detach_session {
        None
    } else {
        let lock = open_lock(&paths.startup_lock_path())?;
        acquire_startup_lock(&lock).await?;
        Some(lock)
    };
    claim_singleton(&paths)?;
    serve(state_dir, paths, startup_lock).await
}

/// Claims the state directory, or refuses to run beside an existing owner.
///
/// Waiting for the other daemon and then reporting success, as an earlier
/// revision did, made a launch look like it had produced the daemon that answers
/// when it had produced nothing. A launch that did not become the owner failed.
fn claim_singleton(paths: &IpcPaths) -> Result<(), AppError> {
    let lock = open_lock(&paths.lock_path())?;
    match lock.try_lock() {
        Ok(()) => {
            let _held_until_exit = SINGLETON_LOCK.set(lock);
            Ok(())
        }
        Err(TryLockError::WouldBlock) => Err(AppError::Daemon(
            "another daemon already owns this state directory".to_owned(),
        )),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

async fn serve(
    state_dir: PathBuf,
    paths: IpcPaths,
    startup_lock: Option<File>,
) -> Result<(), AppError> {
    let launch_nonce = std::env::var(LAUNCH_NONCE_VARIABLE).ok();
    let runtime = Runtime::start(StoreConfig::new(&state_dir)).await?;
    let control = CurrentUserRuntime::open_at(
        &state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await
    .map_err(AppError::CurrentUser)?;
    let web = ma2a_runtime::web::WebLifecycle::new(
        control.web_auth().clone(),
        ma2a_runtime::web::WebRuntimeDependencies::new(
            control.web_auth().clone(),
            ma2a_runtime::web::WebAssets::new(crate::embedded_web::WEB_ASSETS),
            ma2a_runtime::ipc::LocalApiClient::new(paths.clone()),
        ),
    );
    let status = runtime.handle().status().await?;
    let identity = DaemonIdentity::of_current_process(
        launch_nonce,
        RuntimeBoot::new(status.boot_id(), status.endpoint_id()),
    );
    let mut bound = false;
    let serve_result = begin_serving(
        Launch {
            paths: paths.clone(),
            identity: identity.clone(),
            handle: runtime.handle(),
            control,
            web: web.clone(),
        },
        &mut bound,
        startup_lock,
    )
    .await;
    web.stop().await;
    let shutdown_result = shutdown_runtime(runtime).await;
    let cleanup_result = clean_up(&paths, &identity, bound);
    serve_result?;
    shutdown_result?;
    cleanup_result
}

/// Everything one daemon launch serves with, opened independently and used once.
struct Launch {
    paths: IpcPaths,
    identity: DaemonIdentity,
    handle: ma2a_runtime::RuntimeHandle,
    control: CurrentUserRuntime,
    web: ma2a_runtime::web::WebLifecycle,
}

/// Serves until asked to stop, recording in `bound` whether the endpoint is ours.
async fn begin_serving(
    launch: Launch,
    bound: &mut bool,
    startup_lock: Option<File>,
) -> Result<(), AppError> {
    // The record is made current before anything can answer, because a caller
    // that reaches this daemon's endpoint and reads a dead daemon's record
    // beside it would conclude that ownership is ambiguous and refuse to act.
    write_record(&launch.paths, &launch.identity)?;
    reclaim_endpoint(&launch.paths).await?;
    let server = LocalApiServer::bind(
        launch.paths.clone(),
        launch.handle,
        launch.control,
        launch.identity.clone(),
    )?;
    *bound = true;
    // The endpoint is bound, so a caller can connect from this point on.
    report_ready(&launch.identity)?;
    drop(startup_lock);
    let cancellation = CancellationToken::new();
    let serving = server
        .with_web_lifecycle(launch.web)
        .serve(cancellation.child_token());
    tokio::pin!(serving);
    tokio::select! {
        result = &mut serving => finish_serving(result),
        signal = termination_requested() => {
            cancellation.cancel();
            match signal {
                Ok(()) => finish_serving(serving.await),
                Err(error) => Err(error.into()),
            }
        }
    }
}

/// Removes an endpoint left behind, having first confirmed nothing is using it.
///
/// This daemon holds the singleton lock, so any endpoint here belongs to an
/// owner that has gone. If something is nonetheless still answering, it is not
/// accounted for and must not be unlinked underneath.
async fn reclaim_endpoint(paths: &IpcPaths) -> Result<(), AppError> {
    if endpoint_answers(paths).await {
        return Err(AppError::Daemon(
            "something is still answering this state directory's endpoint; refusing to replace it"
                .to_owned(),
        ));
    }
    Ok(paths.remove_stale_endpoint()?)
}

/// Reports readiness to whoever launched this daemon.
///
/// The readiness report is also the launcher's ownership lease. Until it lands,
/// the launcher still owns this process and will terminate it if the attempt
/// fails. If it cannot land — the launcher died, the pipe is gone — then nothing
/// owns this process any more, and carrying on would leave exactly the orphan
/// the lease exists to prevent. A readiness report that cannot be delivered is
/// therefore a failed start.
fn report_ready(identity: &DaemonIdentity) -> Result<(), AppError> {
    let mut stdout = io::stdout();
    stdout.write_all(identity.ready_line().as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn write_record(paths: &IpcPaths, identity: &DaemonIdentity) -> Result<(), AppError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut record = options.open(paths.daemon_record_path())?;
    record.write_all(identity.metadata().as_bytes())?;
    record.flush()?;
    Ok(())
}

/// Removes the daemon record, but only while it still describes this daemon.
///
/// A record naming another launch belongs to a daemon that took over after this
/// one, and erasing it would take away a live daemon's identity.
fn remove_record(paths: &IpcPaths, identity: &DaemonIdentity) {
    let path = paths.daemon_record_path();
    let ours = std::fs::read(&path)
        .ok()
        .and_then(|recorded| DaemonReport::parse(&recorded))
        .is_some_and(|recorded| {
            recorded.launch_nonce() == identity.launch_nonce()
                && recorded.incarnation().pid() == std::process::id()
        });
    if ours {
        let _removed = std::fs::remove_file(path);
    }
}

/// Waits for the operating system's request that this daemon stop.
///
/// A detached daemon has no terminal, so an interrupt alone would never reach
/// it; termination is the signal a launcher, a supervisor or an operator
/// actually sends, and answering it is what makes graceful termination real.
async fn termination_requested() -> io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            _interrupted = interrupt.recv() => Ok(()),
            _terminated = terminate.recv() => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        tokio::signal::ctrl_c().await
    }
}

/// Reports a Runtime that stopped underneath the transport as a daemon failure.
///
/// Cancellation and a completed graceful shutdown are ordinary exits. A Runtime
/// that stopped on its own is not: the daemon can no longer answer anything, so
/// it must fail loudly and let its supervisor or the operator start a new one
/// rather than linger as an endpoint that accepts requests and abandons them.
fn finish_serving(result: Result<ServerExit, ma2a_runtime::ipc::IpcError>) -> Result<(), AppError> {
    match result {
        Ok(ServerExit::RuntimeStopped) => Err(io::Error::other(
            "the Runtime stopped; the daemon cannot answer any command",
        )
        .into()),
        Ok(_) => Ok(()),
        Err(error) => Err(AppError::from(error)),
    }
}

async fn shutdown_runtime(runtime: Runtime) -> Result<(), AppError> {
    match tokio::time::timeout(RUNTIME_SHUTDOWN_DEADLINE, runtime.shutdown()).await {
        Ok(result) => {
            let _report = result?;
            Ok(())
        }
        Err(_elapsed) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the Runtime did not finish shutting down; the daemon is exiting anyway",
        )
        .into()),
    }
}

/// Removes what this daemon left behind, and nothing it did not create.
///
/// The endpoint is removed only when this daemon bound it. A daemon that refused
/// to start because something else was still answering the endpoint must leave
/// that endpoint exactly where it found it; unlinking it on the way out would
/// undo the very refusal that protected it.
fn clean_up(paths: &IpcPaths, identity: &DaemonIdentity, bound: bool) -> Result<(), AppError> {
    remove_record(paths, identity);
    if bound {
        paths.remove_stale_endpoint()?;
    }
    Ok(())
}
