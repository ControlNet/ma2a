//! `space_invite`: one invitation, with its ticket written to a file only this account reads.

use iroh_tickets::Ticket as _;
use ma2a_core::ProtocolError;

use super::{authority_error, committed_space, not_run, without_effects};
use crate::api::{ApiError, CommandResult};

use super::super::super::ConnectionContext;
#[path = "invite_file.rs"]
mod invite_file;

pub(in super::super) async fn invite(
    context: &ConnectionContext,
    command: &crate::api::Command,
) -> Result<CommandResult, ApiError> {
    let (space_id, ttl_ms, output_path) = command.space_invite().ok_or_else(not_run)?;
    let entropy =
        ma2a_core::InviteEntropy::random().map_err(|_| without_effects(ProtocolError::INTERNAL))?;
    let creation =
        crate::EnrollmentCreation::new(space_id, ttl_ms, entropy).map_err(|_| not_run())?;
    // Refused before the invitation is committed, so a bad path never costs a revision, and
    // the same request can run once the path is fixed. Only valid input gets this far, so an
    // invalid request never creates a staging file.
    let output_path = output_path.to_owned();
    let mut destination = Staged(Some(
        tokio::task::spawn_blocking(move || invite_file::Destination::open(&output_path))
            .await
            .unwrap_or(Err(ProtocolError::INTERNAL))
            .map_err(without_effects)?,
    ));
    let created = match context
        .handle
        .create_enrollment_invite_committed(creation)
        .await
    {
        Ok(created) => created,
        Err(error) => {
            // The refusal is answered only once its staging is gone.
            destination.remove().await;
            return Err(authority_error(&error));
        }
    };
    let encoded = created.ticket().encode_string().into_bytes();
    let written = if injected_write_failure(context) {
        Err(ProtocolError::INTERNAL)
    } else {
        let destination = destination.0.take().ok_or(ProtocolError::INTERNAL);
        tokio::task::spawn_blocking(move || destination?.write(&encoded))
            .await
            .unwrap_or(Err(ProtocolError::INTERNAL))
    };
    written.map_err(|error| ApiError::new(error).after_commit(created.revision()))?;
    Ok(
        CommandResult::space_invitation_created(committed_space(created.chain())?)
            .at_revision(created.revision()),
    )
}

/// Deterministic fault point: the ticket file fails to be written after the commit.
#[cfg(test)]
fn injected_write_failure(context: &ConnectionContext) -> bool {
    context.mutation_hooks.take_invite_write_failure()
}

#[cfg(not(test))]
const fn injected_write_failure(_context: &ConnectionContext) -> bool {
    false
}

/// A staged destination that, dropped before it is written, is removed on the blocking pool:
/// a refused or cancelled request never runs file system cleanup on an async worker. A refusal
/// waits for that removal before it is answered; a cancelled request has no one to answer.
struct Staged<T: Send + 'static>(Option<T>);

impl<T: Send + 'static> Staged<T> {
    /// Removes the staged value on the blocking pool and waits until it is gone.
    async fn remove(mut self) {
        if let Some(staged) = self.0.take() {
            let _removed = tokio::task::spawn_blocking(move || drop(staged)).await;
        }
    }
}

impl<T: Send + 'static> Drop for Staged<T> {
    fn drop(&mut self) {
        let Some(staged) = self.0.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(move || drop(staged))),
            Err(_) => drop(staged),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use super::Staged;

    /// Waits, when dropped, until the test releases it, as cleanup on a slow file system
    /// would, then reports whether it was released rather than gave up.
    struct SlowCleanup(mpsc::Receiver<()>, mpsc::Sender<bool>);

    impl Drop for SlowCleanup {
        fn drop(&mut self) {
            let released = self.0.recv_timeout(Duration::from_secs(2)).is_ok();
            let _sent = self.1.send(released);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_staged_value_dropped_on_a_worker_is_cleaned_up_off_it()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Given: a staged value whose cleanup waits.
        let ((release, gate), (finished, done)) = (mpsc::channel(), mpsc::channel());
        let staged = Staged(Some(SlowCleanup(gate, finished)));

        // When: it is dropped on the only worker, which then runs a timer.
        drop(staged);
        tokio::time::sleep(Duration::from_millis(10)).await;
        release.send(())?;

        // Then: the worker was free while the cleanup waited, and the cleanup finished.
        let released =
            tokio::task::spawn_blocking(move || done.recv_timeout(Duration::from_secs(5)))
                .await??;
        assert!(released);
        Ok(())
    }
}
