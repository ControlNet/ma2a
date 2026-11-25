//! A flag a blocking Files job checks between steps, set once nobody waits for the job.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Tells a blocking job that its caller stopped waiting: the stream was cancelled, the grant
/// revoked, or the deadline passed. A job that changes many entries checks it before each one.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stop(Arc<AtomicBool>);

impl Stop {
    /// Whether the job should make no further change.
    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Returns a guard that sets this flag when dropped, as the waiting future is.
    pub(crate) fn on_drop(&self) -> SetOnDrop {
        SetOnDrop(Arc::clone(&self.0))
    }

    /// Test-only hook invoked after one entry removal so tests can pause between entries.
    #[cfg(not(test))]
    #[expect(
        clippy::unused_self,
        reason = "the test build intercepts per-entry removal progress here"
    )]
    pub(crate) const fn step(&self, _name: &str) {}

    /// Test-only hook invoked after one entry removal so tests can pause between entries.
    #[cfg(test)]
    #[expect(clippy::unused_self, reason = "matches the non-test method on Stop")]
    pub(crate) fn step(&self, name: &str) {
        let hook = tests::take_step_hook(name);
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// Sets its [`Stop`] when dropped.
pub(crate) struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Mutex, mpsc},
        time::Duration,
    };

    use ma2a_core::service::{FilesRequest, FilesResult, FrameKind, RemotePath, ServiceError};

    use crate::{FrameReader, FrameWriter, report::OperationEnd};

    type StepHook = (&'static str, Box<dyn FnOnce() + Send>);

    static STEP_HOOK: Mutex<Option<StepHook>> = Mutex::new(None);

    pub(super) fn take_step_hook(name: &str) -> Option<Box<dyn FnOnce() + Send>> {
        let mut slot = STEP_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot
            .as_ref()
            .is_some_and(|(prefix, _)| name.starts_with(prefix))
        {
            slot.take().map(|(_, hook)| hook)
        } else {
            None
        }
    }

    #[tokio::test]
    async fn a_job_sees_its_stop_once_nobody_waits_for_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let (started, wait_started) = tokio::sync::oneshot::channel();
        let (saw, seen) = mpsc::channel();
        let job = super::super::blocking(move |stop| {
            let _sent = started.send(());
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !stop.is_set() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            let _sent = saw.send(stop.is_set());
            Ok(())
        });
        let waiting = tokio::spawn(job);
        wait_started.await?;
        // Dropping the waiting future, as a revoked stream does, sets the flag.
        waiting.abort();
        let observed =
            tokio::task::spawn_blocking(move || seen.recv_timeout(Duration::from_secs(10)))
                .await??;
        assert!(observed);
        Ok(())
    }

    #[tokio::test]
    async fn closing_the_caller_stream_stops_a_recursive_removal_before_later_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        // Given: a tree with 30 files and a barrier right after the first deletion.
        let root = std::env::temp_dir().join(format!("ma2a-eof-remove-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&root);
        let tree = root.join("tree");
        std::fs::create_dir_all(&tree)?;
        for index in 0..30 {
            std::fs::write(tree.join(format!("eof-cancel-{index:02}")), b"x")?;
        }
        let (first_gone, wait_first) = tokio::sync::oneshot::channel();
        let (release, wait_release) = mpsc::channel::<()>();
        *STEP_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((
            "eof-cancel-",
            Box::new(move || {
                let _sent = first_gone.send(());
                let _released = wait_release.recv_timeout(Duration::from_secs(10));
            }),
        ));

        let (client, server) = tokio::io::duplex(1 << 16);
        let (client_read, client_write) = tokio::io::split(client);
        let (server_read, server_write) = tokio::io::split(server);
        let (mut client_reader, mut client_writer) = (
            FrameReader::new(client_read),
            FrameWriter::new(client_write),
        );
        let (mut server_reader, mut server_writer) = (
            FrameReader::new(server_read),
            FrameWriter::new(server_write),
        );
        let root_arg = root.canonicalize()?;
        let request = FilesRequest::Remove(RemotePath::parse("docs:/tree")?, true);
        let mut host = tokio::spawn(async move {
            super::super::serve_files(root_arg, request, (&mut server_reader, &mut server_writer))
                .await
        });

        // When: one entry is gone and the caller cleanly closes its write half before releasing.
        tokio::time::timeout(Duration::from_secs(5), wait_first).await??;
        client_writer.shutdown().await?;
        let settled = tokio::time::timeout(Duration::from_millis(200), &mut host).await;
        let _released = release.send(());
        let report = match settled {
            Ok(joined) => joined?,
            Err(_) => tokio::time::timeout(Duration::from_secs(5), host).await??,
        };

        // Then: the blocking job released its permit, later entries stay, and cancel is reported.
        let permits = tokio::time::timeout(
            Duration::from_secs(5),
            super::super::BLOCKING_JOBS
                .acquire_many(u32::try_from(super::super::MAX_BLOCKING_JOBS).unwrap_or(u32::MAX)),
        )
        .await??;
        drop(permits);
        let remaining = std::fs::read_dir(&tree).map_or(0, Iterator::count);
        let _cleaned = std::fs::remove_dir_all(&root);

        let frame = client_reader.expect().await?;
        assert_eq!(frame.kind(), FrameKind::Outcome);
        assert_eq!(
            FilesResult::decode_outcome(frame.payload())?,
            Err(ServiceError::Cancelled)
        );
        assert_eq!(report.end, OperationEnd::Failed(ServiceError::Cancelled));
        assert_eq!(remaining, 29);
        Ok(())
    }
}
