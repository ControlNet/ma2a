//! Best-effort SSE projection classes with revision-gap detection.

use ma2a_core::{EndpointId, ProtocolError, SpaceId};
use serde_json::{Value, json};

use super::{ApiError, MAX_COLLECTION_ITEMS, MAX_LOCAL_EVENT_BYTES, codec_fields::encode_hex};

/// Exact ordered event class inventory carried by the schema and TypeScript contract.
pub const EVENT_NAMES: [&str; 9] = [
    "snapshot_invalidated",
    "endpoint_changed",
    "spaces_changed",
    "control_sync_changed",
    "relay_candidates_changed",
    "relay_state_changed",
    "reachability_changed",
    "echo_summary_changed",
    "ui_auth_changed",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum EventKind {
    SnapshotInvalidated,
    EndpointChanged(Vec<EndpointId>),
    SpacesChanged(Vec<SpaceId>),
    ControlSyncChanged(Vec<EndpointId>),
    RelayCandidatesChanged(Vec<EndpointId>),
    RelayStateChanged,
    ReachabilityChanged(Vec<EndpointId>),
    EchoSummaryChanged(Vec<EndpointId>),
    UiAuthChanged,
}

/// One non-durable local Runtime event projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEvent {
    revision: u64,
    kind: EventKind,
}

impl RuntimeEvent {
    /// Creates a client invalidation event that always forces resnapshot.
    pub const fn snapshot_invalidated(revision: u64) -> Self {
        Self {
            revision,
            kind: EventKind::SnapshotInvalidated,
        }
    }

    /// Creates an event after enforcing changed-entity bounds.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 entity IDs are supplied.
    pub fn endpoint_changed(
        revision: u64,
        endpoint_ids: Vec<EndpointId>,
    ) -> Result<Self, ApiError> {
        bounded_event(revision, EventKind::EndpointChanged(endpoint_ids))
    }

    /// Creates a Space change event after enforcing entity bounds.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 Space IDs are supplied.
    pub fn spaces_changed(revision: u64, space_ids: Vec<SpaceId>) -> Result<Self, ApiError> {
        bounded_event(revision, EventKind::SpacesChanged(space_ids))
    }

    /// Creates a control-sync change event keyed by peer Endpoint.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 peer IDs are supplied.
    pub fn control_sync_changed(
        revision: u64,
        endpoint_ids: Vec<EndpointId>,
    ) -> Result<Self, ApiError> {
        bounded_event(revision, EventKind::ControlSyncChanged(endpoint_ids))
    }

    /// Creates a relay-candidate change event keyed by candidate Endpoint.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 candidate IDs are supplied.
    pub fn relay_candidates_changed(
        revision: u64,
        endpoint_ids: Vec<EndpointId>,
    ) -> Result<Self, ApiError> {
        bounded_event(revision, EventKind::RelayCandidatesChanged(endpoint_ids))
    }

    /// Creates an observed relay-state change event.
    pub const fn relay_state_changed(revision: u64) -> Self {
        Self {
            revision,
            kind: EventKind::RelayStateChanged,
        }
    }

    /// Creates a reachability change event keyed by affected Endpoint.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 Endpoint IDs are supplied.
    pub fn reachability_changed(
        revision: u64,
        endpoint_ids: Vec<EndpointId>,
    ) -> Result<Self, ApiError> {
        bounded_event(revision, EventKind::ReachabilityChanged(endpoint_ids))
    }

    /// Creates an Echo summary change event keyed by target Endpoint.
    ///
    /// # Errors
    /// Returns invalid input when more than 256 target IDs are supplied.
    pub fn echo_summary_changed(
        revision: u64,
        endpoint_ids: Vec<EndpointId>,
    ) -> Result<Self, ApiError> {
        bounded_event(revision, EventKind::EchoSummaryChanged(endpoint_ids))
    }

    /// Creates a UI authentication state change event.
    pub const fn ui_auth_changed(revision: u64) -> Self {
        Self {
            revision,
            kind: EventKind::UiAuthChanged,
        }
    }

    /// Returns the post-change authoritative revision.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the exact event class discriminant.
    pub const fn event_type(&self) -> &'static str {
        match self.kind {
            EventKind::SnapshotInvalidated => "snapshot_invalidated",
            EventKind::EndpointChanged(_) => "endpoint_changed",
            EventKind::SpacesChanged(_) => "spaces_changed",
            EventKind::ControlSyncChanged(_) => "control_sync_changed",
            EventKind::RelayCandidatesChanged(_) => "relay_candidates_changed",
            EventKind::RelayStateChanged => "relay_state_changed",
            EventKind::ReachabilityChanged(_) => "reachability_changed",
            EventKind::EchoSummaryChanged(_) => "echo_summary_changed",
            EventKind::UiAuthChanged => "ui_auth_changed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContinuityKind {
    Apply,
    Resnapshot,
}

/// Whether an SSE event may be applied or requires an authoritative resnapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventContinuity(ContinuityKind);

impl EventContinuity {
    /// Returns whether this event is the exact next revision.
    pub const fn may_apply(self) -> bool {
        matches!(self.0, ContinuityKind::Apply)
    }

    /// Returns whether a disconnect, duplicate, reorder, or gap requires resnapshot.
    pub const fn requires_resnapshot(self) -> bool {
        matches!(self.0, ContinuityKind::Resnapshot)
    }
}

/// Classifies strict event continuity; all nonconsecutive revisions require resnapshot.
pub const fn classify_event_revision(previous: u64, incoming: u64) -> EventContinuity {
    match previous.checked_add(1) {
        Some(expected) if expected == incoming => EventContinuity(ContinuityKind::Apply),
        Some(_) | None => EventContinuity(ContinuityKind::Resnapshot),
    }
}

/// Serializes one SSE data payload and enforces the event bound.
///
/// # Errors
/// Returns an internal error when serialization fails or exceeds the event bound.
pub fn encode_event(event: &RuntimeEvent) -> Result<Vec<u8>, ApiError> {
    let value = json!({
        "type": event.event_type(),
        "revision": event.revision,
        "changed": changed_value(&event.kind),
    });
    let encoded = serde_json::to_vec(&value).map_err(|_| ApiError::new(ProtocolError::INTERNAL))?;
    if encoded.len() > MAX_LOCAL_EVENT_BYTES {
        Err(ApiError::new(ProtocolError::INTERNAL))
    } else {
        Ok(encoded)
    }
}

fn bounded_event(revision: u64, kind: EventKind) -> Result<RuntimeEvent, ApiError> {
    let count = match &kind {
        EventKind::EndpointChanged(ids)
        | EventKind::ControlSyncChanged(ids)
        | EventKind::RelayCandidatesChanged(ids)
        | EventKind::ReachabilityChanged(ids)
        | EventKind::EchoSummaryChanged(ids) => ids.len(),
        EventKind::SpacesChanged(ids) => ids.len(),
        EventKind::SnapshotInvalidated
        | EventKind::RelayStateChanged
        | EventKind::UiAuthChanged => 0,
    };
    if count > MAX_COLLECTION_ITEMS {
        Err(ApiError::invalid_input())
    } else {
        Ok(RuntimeEvent { revision, kind })
    }
}

fn changed_value(kind: &EventKind) -> Value {
    match kind {
        EventKind::EndpointChanged(ids)
        | EventKind::ControlSyncChanged(ids)
        | EventKind::RelayCandidatesChanged(ids)
        | EventKind::ReachabilityChanged(ids)
        | EventKind::EchoSummaryChanged(ids) => {
            json!({"endpoint_ids": ids.iter().map(|id| encode_hex(id.as_bytes())).collect::<Vec<_>>() })
        }
        EventKind::SpacesChanged(ids) => {
            json!({"space_ids": ids.iter().map(|id| encode_hex(id.as_bytes())).collect::<Vec<_>>() })
        }
        EventKind::SnapshotInvalidated
        | EventKind::RelayStateChanged
        | EventKind::UiAuthChanged => json!({}),
    }
}
