//! In-memory registry of active streaming operations and a bounded content-free audit.
//!
//! Nothing here is persisted: after a restart no operation can appear active.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use ma2a_core::{
    EndpointId, RequestId, ServiceKind,
    service::{
        AgentRequest, FilesRequest, OpenBody, ProcessRequest, ServiceError, TransferRequest,
    },
};
use ma2a_services::{OperationEnd, OperationReport};

/// Most completed operations retained for `operations_status`.
pub(crate) const AUDIT_CAPACITY: usize = 256;

/// Whether this Runtime served or initiated an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Inbound,
    Outbound,
}

/// Returns the stable, content-free name of an operation.
pub(crate) const fn operation_name(body: &OpenBody) -> &'static str {
    match body {
        OpenBody::Files(FilesRequest::Stat(_)) => "stat",
        OpenBody::Files(FilesRequest::List(..)) => "list",
        OpenBody::Files(FilesRequest::Mkdir(..)) => "mkdir",
        OpenBody::Files(FilesRequest::Read(..)) => "read",
        OpenBody::Files(FilesRequest::Write(..)) => "write",
        OpenBody::Files(FilesRequest::Rename(..)) => "rename",
        OpenBody::Files(FilesRequest::Remove(..)) => "remove",
        OpenBody::Transfer(TransferRequest::Get(..)) => "download",
        OpenBody::Transfer(TransferRequest::Put(..)) => "upload",
        OpenBody::Exec(_) => "exec",
        OpenBody::Shell(_) => "shell",
        OpenBody::Proxy(_) => "proxy",
        OpenBody::Process(ProcessRequest::Start(_)) => "process_start",
        OpenBody::Process(ProcessRequest::List(_)) => "process_list",
        OpenBody::Process(ProcessRequest::Inspect(_)) => "process_inspect",
        OpenBody::Process(ProcessRequest::Logs(..)) => "process_logs",
        OpenBody::Process(ProcessRequest::Attach(..)) => "process_attach",
        OpenBody::Process(ProcessRequest::Stdin(..)) => "process_stdin",
        OpenBody::Process(ProcessRequest::Signal(..)) => "process_signal",
        OpenBody::Process(ProcessRequest::Terminate(..)) => "process_terminate",
        OpenBody::Process(ProcessRequest::Delete(_)) => "process_delete",
        OpenBody::Agent(AgentRequest::Agents(_)) => "agent_discover",
        OpenBody::Agent(AgentRequest::Submit(_)) => "agent_submit",
        OpenBody::Agent(AgentRequest::Send(..)) => "agent_send",
        OpenBody::Agent(AgentRequest::Get(..)) => "agent_get",
        OpenBody::Agent(AgentRequest::Tasks(..)) => "agent_tasks",
        OpenBody::Agent(AgentRequest::Watch(..)) => "agent_watch",
        OpenBody::Agent(AgentRequest::Cancel(..)) => "agent_cancel",
        OpenBody::Agent(AgentRequest::Artifact(..)) => "agent_artifact",
    }
}

/// Identity of one operation; paths, argv, environment, and content are structurally absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OperationKey {
    pub(crate) request_id: RequestId,
    pub(crate) peer: EndpointId,
    pub(crate) kind: ServiceKind,
    pub(crate) operation: &'static str,
    pub(crate) direction: Direction,
}

/// One operation still running.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActiveOperation {
    pub(crate) key: OperationKey,
    pub(crate) started: Instant,
}

/// One completed operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OperationRecord {
    pub(crate) key: OperationKey,
    pub(crate) end: OperationEnd,
    pub(crate) bytes_in: u64,
    pub(crate) bytes_out: u64,
    pub(crate) duration_ms: u64,
}

#[derive(Debug, Default)]
struct Inner {
    next: u64,
    active: BTreeMap<u64, ActiveOperation>,
    recent: VecDeque<OperationRecord>,
}

/// Cloneable registry shared by the stream host, the stream plane, and the local API.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationsRegistry(Arc<Mutex<Inner>>);

/// Point-in-time view: active operations and the most recent completed ones, newest first.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationsSnapshot {
    pub(crate) active: Vec<(ActiveOperation, u64)>,
    pub(crate) recent: Vec<OperationRecord>,
}

impl OperationsRegistry {
    /// Creates an empty registry.
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(Inner {
            next: 0,
            active: BTreeMap::new(),
            recent: VecDeque::new(),
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Registers one running operation until its guard finishes or drops.
    pub(crate) fn start(&self, key: OperationKey) -> OperationGuard {
        let id = {
            let mut inner = self.lock();
            inner.next = inner.next.wrapping_add(1);
            let id = inner.next;
            inner.active.insert(
                id,
                ActiveOperation {
                    key,
                    started: Instant::now(),
                },
            );
            id
        };
        OperationGuard {
            registry: self.clone(),
            id,
            finished: false,
        }
    }

    /// Records one operation that ended before it could start, such as a rejected open.
    pub(crate) fn record(&self, key: OperationKey, report: OperationReport) {
        self.start(key).finish(report);
    }

    /// Returns active operations with their elapsed milliseconds and recent records.
    pub(crate) fn snapshot(&self) -> OperationsSnapshot {
        let inner = self.lock();
        OperationsSnapshot {
            active: inner
                .active
                .values()
                .map(|operation| (*operation, elapsed_ms(operation.started)))
                .collect(),
            recent: inner.recent.iter().rev().copied().collect(),
        }
    }

    fn complete(&self, id: u64, report: OperationReport) {
        let mut inner = self.lock();
        let Some(operation) = inner.active.remove(&id) else {
            return;
        };
        if inner.recent.len() == AUDIT_CAPACITY {
            inner.recent.pop_front();
        }
        inner.recent.push_back(OperationRecord {
            key: operation.key,
            end: report.end,
            bytes_in: report.bytes_in,
            bytes_out: report.bytes_out,
            duration_ms: elapsed_ms(operation.started),
        });
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Keeps one operation active; dropping it unfinished records a cancellation.
#[derive(Debug)]
pub(crate) struct OperationGuard {
    registry: OperationsRegistry,
    id: u64,
    finished: bool,
}

impl OperationGuard {
    /// Completes the operation with its report.
    pub(crate) fn finish(mut self, report: OperationReport) {
        self.finished = true;
        self.registry.complete(self.id, report);
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.registry.complete(
                self.id,
                OperationReport::new(OperationEnd::Failed(ServiceError::Cancelled)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::{EndpointId, RequestId, ServiceKind, service::ServiceError};
    use ma2a_services::{OperationEnd, OperationReport};

    use super::{AUDIT_CAPACITY, Direction, OperationKey, OperationsRegistry};

    fn key() -> OperationKey {
        OperationKey {
            request_id: RequestId::try_from([1_u8; 16].as_slice()).expect("request id"),
            peer: EndpointId::from(iroh::SecretKey::generate().public()),
            kind: ServiceKind::Exec,
            operation: "exec",
            direction: Direction::Inbound,
        }
    }

    #[test]
    fn dropped_guards_never_leave_an_operation_active() {
        let registry = OperationsRegistry::default();
        let guard = registry.start(key());
        assert_eq!(registry.snapshot().active.len(), 1);
        drop(guard);
        let snapshot = registry.snapshot();
        assert!(snapshot.active.is_empty());
        assert_eq!(
            snapshot.recent.first().map(|record| record.end),
            Some(OperationEnd::Failed(ServiceError::Cancelled))
        );
    }

    #[test]
    fn the_audit_is_bounded_and_newest_first() {
        let registry = OperationsRegistry::default();
        for bytes in 0..(AUDIT_CAPACITY as u64 + 10) {
            registry.record(
                key(),
                OperationReport::new(OperationEnd::Completed).with_bytes(bytes, 0),
            );
        }
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.recent.len(), AUDIT_CAPACITY);
        assert_eq!(
            snapshot.recent.first().map(|record| record.bytes_in),
            Some(AUDIT_CAPACITY as u64 + 9)
        );
    }
}
