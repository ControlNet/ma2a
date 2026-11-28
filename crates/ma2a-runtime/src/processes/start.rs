//! `start`: bind the `RequestId`, launch the supervisor, commit `starting`, then `go`.
//!
//! The binding is committed before any supervisor exists and `go` is sent only after
//! `starting` is committed, so a crash at any point can neither duplicate a Process nor
//! leave a child that no record admits to.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        FrameKind, ProcessAccept, ProcessId, ProcessReason, ProcessReply, ProcessStart,
        ProcessState, ServiceAccess, ServiceError,
    },
};
use ma2a_services::{
    FrameWriter, OperationEnd, OperationReport,
    supervisor::{Channel, Credentials, SupervisorRequest},
};
use ma2a_store::{Committed, NewProcess, ProcessCreation};
use tokio::io::AsyncWrite;
use tokio_util::sync::CancellationToken;

use super::{ProcessManager, follow::request, go::GoDispatch, host::finish};

/// Fingerprints a canonical start body; a retry must send exactly the same bytes.
fn fingerprint(start: &[u8]) -> [u8; 32] {
    *blake3::Hasher::new_derive_key("ma2a process start v1")
        .update(start)
        .finalize()
        .as_bytes()
}

/// Aborts this daemon at one named point of the start sequence (debug builds only).
fn fault(point: &str) {
    #[cfg(debug_assertions)]
    if std::env::var_os("MA2A_TEST_PROCESS_FAULT").is_some_and(|value| value == point) {
        ma2a_services::supervisor::crash();
    }
    #[cfg(not(debug_assertions))]
    let _ = point;
}

impl ProcessManager {
    /// Records one start for `creator`, or answers the Process its `RequestId` already made.
    #[cfg(all(test, unix))]
    pub(super) async fn create(
        &self,
        binding: (EndpointId, RequestId),
        start: &ProcessStart,
    ) -> Result<(ProcessCreation, Credentials), ServiceError> {
        let ((creation, _), credentials) = self.create_inner(binding, (start, false, None)).await?;
        Ok((creation, credentials))
    }

    /// Records one local start, returning any archived Process whose directory must be removed.
    pub(super) async fn create_local(
        &self,
        binding: (EndpointId, RequestId),
        start: &ProcessStart,
    ) -> Result<((ProcessCreation, Option<ProcessId>), Credentials), ServiceError> {
        self.create_inner(binding, (start, true, None)).await
    }

    async fn create_inner(
        &self,
        (creator, request_id): (EndpointId, RequestId),
        (start, reclaim_retained, stop): (&ProcessStart, bool, Option<&CancellationToken>),
    ) -> Result<((ProcessCreation, Option<ProcessId>), Credentials), ServiceError> {
        self.wait_for_startup(stop).await?;
        let bytes = start.to_bytes().map_err(|_| ServiceError::InvalidInput)?;
        let name = start.name().map(str::to_owned);
        let id = ProcessId::random().map_err(|_| ServiceError::Internal)?;
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).map_err(|_| ServiceError::Internal)?;
        let created_ms = self.now_ms();
        let local = self.0.local;
        let outcome =
            self.store()
                .processes(
                    move |repository| -> Result<
                        Committed<(ProcessCreation, Option<ProcessId>)>,
                        ServiceError,
                    > {
                        if !reclaim_retained && creator != local {
                            crate::store::authorize(
                                repository,
                                (local, creator),
                                &ServiceAccess::Process,
                            )?;
                        }
                        let new = NewProcess {
                            id,
                            binding: (creator, request_id),
                            fingerprint: fingerprint(&bytes),
                            start: (&bytes, name.as_deref()),
                            secret,
                            created_ms,
                        };
                        if reclaim_retained {
                            repository.create_local_process(&new)
                        } else {
                            repository.create_process(&new).map(|committed| {
                                Committed::new(committed.revision(), (committed.into_value(), None))
                            })
                        }
                        .map_err(|_| ServiceError::Unavailable)
                    },
                )
                .await??
                .into_value();
        Ok((outcome, Credentials::new(id, secret)))
    }

    /// Serves one remote `start`.
    pub(super) async fn start<W: AsyncWrite + Unpin + Send>(
        &self,
        ((binding, start), stop): (((EndpointId, RequestId), ProcessStart), &CancellationToken),
        writer: &mut FrameWriter<W>,
    ) -> OperationReport {
        if self.0.hosting.is_none() {
            return super::host::refuse(writer, ServiceError::Unavailable).await;
        }
        // A relative working directory is refused before anything is recorded, as in Exec.
        let relative = |cwd: &str| !std::path::Path::new(cwd).is_absolute();
        if start.spec().cwd().is_some_and(relative) {
            return super::host::refuse(writer, ServiceError::InvalidInput).await;
        }
        let ((creation, _), credentials) = match self
            .create_inner(binding, (&start, false, Some(stop)))
            .await
        {
            Ok(created) => created,
            Err(error) => return super::host::refuse(writer, error).await,
        };
        let (record, existing) = match creation {
            ProcessCreation::Created(record) => (record, false),
            ProcessCreation::Existing(record) => (record, true),
            ProcessCreation::Conflict => {
                return super::host::refuse(writer, ServiceError::Conflict).await;
            }
            ProcessCreation::LimitExceeded => {
                return super::host::refuse(writer, ServiceError::ConcurrencyExceeded).await;
            }
        };
        let accept = ProcessAccept::Started(record.id, existing).encode();
        let Ok(accept) = accept else {
            return super::host::refuse(writer, ServiceError::Internal).await;
        };
        // The workload is independent of its caller: a caller gone before the accept still
        // gets its Process, which a retry with the same RequestId then answers.
        let accepted = super::host::write_accept(writer, &accept).await;
        let outcome = if existing {
            Ok(record)
        } else {
            fault("after_created");
            self.bring_up(credentials, start).await
        };
        if let Err(error) = accepted {
            return OperationReport::new(OperationEnd::Failed(error));
        }
        fault("before_response");
        finish(
            writer,
            outcome.map(|record| ProcessReply::Summary(record.summary())),
        )
        .await
    }

    /// Launches, commits `starting`, sends `go`, and hands the Process to its follower.
    pub(super) async fn bring_up(
        &self,
        credentials: Credentials,
        start: ProcessStart,
    ) -> Result<ma2a_store::ProcessRecord, ServiceError> {
        let id = credentials.id();
        let Ok(proven) = self.launch(credentials, start).await else {
            let failed = (ProcessState::Failed, Some(ProcessReason::SpawnFailed));
            return match self.step(id, failed).await {
                Ok(committed) => Ok(committed.into_value()),
                Err(error) => {
                    self.settle_created(
                        (id, None),
                        super::maintain::CreatedSettlement::SpawnFailed,
                    );
                    Err(error)
                }
            };
        };
        fault("after_launch");
        let watching = open_watch(proven).await;
        let stepped = match self.step(id, (ProcessState::Starting, None)).await {
            Ok(committed) => committed.into_value(),
            Err(error) => {
                // Without `starting` no `go` may be sent; abort the waiting supervisor and
                // retry settling the `created` row if the Store is still unavailable.
                drop(watching);
                self.settle_created((id, None), super::maintain::CreatedSettlement::Interrupted);
                return Err(error);
            }
        };
        if stepped.state != ProcessState::Starting {
            // Another path already moved this Process out of `created`; abort the proved
            // supervisor so no unmanaged child can start.
            drop(watching);
            if let Ok(channel) = self.address(id).connect(&credentials).await {
                let _aborted = request(channel, SupervisorRequest::Abort).await;
            }
            return Ok(stepped);
        }
        let address = self.address(id);
        let reported = match address.connect(&credentials).await {
            Ok(channel) => match self.dispatch_go((id, stepped.creator), channel).await {
                GoDispatch::Reported(reported) => Ok(reported),
                GoDispatch::Aborted => {
                    drop(watching);
                    return self.record(id).await?.ok_or(ServiceError::UnknownProcess);
                }
                GoDispatch::AbortFailed(error) => {
                    drop(watching);
                    self.follow(id, None);
                    return Err(error);
                }
                GoDispatch::Deferred(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        fault("after_go");
        if let Ok(reported) = reported {
            self.follow(id, watching.ok());
            return self.apply(id, reported).await.map(Committed::into_value);
        }
        // Without the open watch the follower reaches the supervisor itself, and for a
        // Process still `starting` that sends `go` again; a repeated `go` has no effect.
        drop(watching);
        self.follow(id, None);
        self.record(id).await?.ok_or(ServiceError::UnknownProcess)
    }
}

/// Turns a proven connection into an accepted `watch`.
///
/// A supervisor that never answers holds the start or its follower no longer than any
/// supervisor answer.
pub(super) async fn open_watch((mut reader, mut writer): Channel) -> Result<Channel, ServiceError> {
    let answered = tokio::time::timeout(super::attach::SUPERVISOR_ANSWER, async {
        writer
            .frame(FrameKind::Open, &SupervisorRequest::Watch.to_bytes())
            .await?;
        reader.expect().await
    });
    let accept = answered.await.map_err(|_| ServiceError::Unavailable)??;
    if accept.kind() == FrameKind::Accept {
        Ok((reader, writer))
    } else {
        Err(ServiceError::Unavailable)
    }
}
