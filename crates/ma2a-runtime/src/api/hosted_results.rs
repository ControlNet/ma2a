//! Results of the hosted-service local API commands.

use base64::Engine as _;
use ma2a_core::{
    EndpointId, RequestId,
    service::{OperationName, ServiceManifest, ServiceName},
};
use ma2a_store::HostedServiceRecord;
use serde_json::{Value, json};

use super::{
    agent_results::bounded,
    codec_fields::encode_hex,
    codec_hosted::manifest_value,
    responses::{CommandResult, ResultKind},
};

/// Most registrations one `service_hosted_list` page carries.
pub const MAX_HOSTED_LIST_PAGE: usize = 16;

/// Whether a hosted service is stopped, started without a ready link, or ready.
#[expect(
    clippy::exhaustive_enums,
    reason = "the three states are the frozen local API vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedServiceState {
    /// The operator has not started it.
    Stopped,
    /// Started, but no attached process reports it ready.
    Unavailable,
    /// Started, and its process accepts calls.
    Ready,
}

impl HostedServiceState {
    const fn name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Unavailable => "unavailable",
            Self::Ready => "ready",
        }
    }
}

/// One registration as the local operator sees it, with what runs now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedServiceView {
    record: HostedServiceRecord,
    state: HostedServiceState,
    running_calls: usize,
    /// Calls per version since this Runtime started, by ascending version.
    calls: Vec<(u16, u64)>,
}

impl HostedServiceView {
    /// Pairs one durable registration with its state, running calls, and calls per version.
    #[must_use]
    pub fn new(
        record: HostedServiceRecord,
        (state, running_calls, calls): (HostedServiceState, usize, Vec<(u16, u64)>),
    ) -> Self {
        Self {
            record,
            state,
            running_calls,
            calls,
        }
    }

    /// Returns the current state.
    pub const fn state(&self) -> HostedServiceState {
        self.state
    }

    /// Returns the durable registration.
    pub fn into_record(self) -> HostedServiceRecord {
        self.record
    }

    fn to_value(&self) -> Value {
        let calls: Vec<Value> = self
            .calls
            .iter()
            .map(|(version, count)| json!({"version": version, "count": count}))
            .collect();
        json!({
            "service": self.record.name.as_str(),
            "manifest": manifest_value(&self.record.manifest),
            "enabled": self.record.enabled,
            "state": self.state.name(),
            "running_calls": self.running_calls,
            "calls": calls,
            "created_at_ms": self.record.created_at_ms,
            "updated_at_ms": self.record.updated_at_ms,
        })
    }
}

/// The result of one hosted-service command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HostedResult {
    Services(Vec<Value>, Option<String>),
    Registered(Value, bool),
    Unregistered(String),
    Started(Value),
    Stopped(Value),
    Attached(Value),
    Call(Value),
}

impl HostedResult {
    pub(super) const fn result_type(&self) -> &'static str {
        match self {
            Self::Services(..) => "hosted_services",
            Self::Registered(..) => "service_registered",
            Self::Unregistered(_) => "service_unregistered",
            Self::Started(_) => "service_started",
            Self::Stopped(_) => "service_stopped",
            Self::Attached(_) => "service_attached",
            Self::Call(_) => "service_call",
        }
    }

    pub(super) fn to_value(&self) -> Value {
        match self {
            Self::Services(services, next) => json!({"services": services, "next": next}),
            Self::Registered(service, created) => json!({"service": service, "created": created}),
            Self::Unregistered(service) => json!({"service": service}),
            Self::Started(value)
            | Self::Stopped(value)
            | Self::Attached(value)
            | Self::Call(value) => value.clone(),
        }
    }
}

impl CommandResult {
    /// Creates one bounded `hosted_services` page from up to one registration more than a
    /// page; the page continues when that extra one exists or the page ran out of room.
    pub fn hosted_services(mut services: Vec<HostedServiceView>) -> Self {
        let more = services.len() > MAX_HOSTED_LIST_PAGE;
        services.truncate(MAX_HOSTED_LIST_PAGE);
        let (values, cut) = bounded(services.iter().map(HostedServiceView::to_value).collect());
        let next = (more || cut)
            .then(|| {
                values
                    .len()
                    .checked_sub(1)
                    .and_then(|last| services.get(last))
            })
            .flatten()
            .map(|service| service.record.name.as_str().to_owned());
        Self::hosted(HostedResult::Services(values, next))
    }

    /// Creates a `service_registered` result; `created` when the service is new.
    pub fn service_registered(service: &HostedServiceView, created: bool) -> Self {
        Self::hosted(HostedResult::Registered(service.to_value(), created))
    }

    /// Creates a `service_unregistered` result.
    pub fn service_unregistered(service: &ServiceName) -> Self {
        Self::hosted(HostedResult::Unregistered(service.as_str().to_owned()))
    }

    /// Creates a `service_started` result.
    pub fn service_started(service: &HostedServiceView) -> Self {
        Self::hosted(HostedResult::Started(service.to_value()))
    }

    /// Creates a `service_stopped` result.
    pub fn service_stopped(service: &HostedServiceView) -> Self {
        Self::hosted(HostedResult::Stopped(service.to_value()))
    }

    /// Creates the `service_attached` answer that turns a connection into a link.
    pub fn service_attached(service: &ServiceName, manifest: &ServiceManifest) -> Self {
        Self::hosted(HostedResult::Attached(json!({
            "service": service.as_str(),
            "manifest": manifest_value(manifest),
        })))
    }

    /// Creates the `service_call` answer that hands one call to its service.
    pub fn service_call(
        (service, call): (&ServiceName, [u8; 16]),
        (caller, request_id): (EndpointId, RequestId),
        (version, operation, body): (u16, &OperationName, &[u8]),
    ) -> Self {
        Self::hosted(HostedResult::Call(json!({
            "service": service.as_str(),
            "call_id": encode_hex(&call),
            "caller_endpoint_id": encode_hex(caller.as_bytes()),
            "request_id": encode_hex(request_id.as_bytes()),
            "version": version,
            "operation": operation.as_str(),
            "body": base64::engine::general_purpose::STANDARD.encode(body),
        })))
    }

    const fn hosted(result: HostedResult) -> Self {
        Self(ResultKind::Hosted(result), None)
    }
}
