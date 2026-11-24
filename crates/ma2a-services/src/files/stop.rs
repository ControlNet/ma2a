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
    use std::time::Duration;

    #[tokio::test]
    async fn a_job_sees_its_stop_once_nobody_waits_for_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let (started, wait_started) = tokio::sync::oneshot::channel();
        let (saw, seen) = std::sync::mpsc::channel();
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
}
