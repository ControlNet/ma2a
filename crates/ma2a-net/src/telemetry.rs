use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use iroh::endpoint::Connection;
use iroh_base::{RelayUrl, TransportAddr};
use ma2a_core::EndpointId;

const MAX_REMOTE_ENDPOINTS: usize = 128;
const MAX_OBSERVATIONS_PER_REMOTE: usize = 8;

/// Stable bounded failure classes for connection telemetry and retry policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionErrorClass {
    /// Temporary transport or reachability failure.
    Transient,
    /// Permanent authorization denial.
    Authorization,
    /// Permanent ALPN or protocol-version mismatch.
    Version,
    /// Permanent membership revocation.
    Revocation,
    /// Permanent malformed target or request.
    MalformedInput,
    /// Permanent local policy denial.
    Policy,
    /// Explicit operation or Runtime cancellation.
    Cancelled,
}

/// Iroh-observed connection path state without MA2A path scoring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionPathState {
    /// An attempt is in progress.
    Connecting,
    /// Every observed open path is relay-backed.
    Relay,
    /// Every observed open path is direct IP.
    Direct,
    /// Paths are mixed, custom, absent, or otherwise unknown.
    MixedOrUnknown,
}

/// Bounded failure metadata retained for one observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionErrorObservation {
    class: ConnectionErrorClass,
    detail: String,
    observed_at_ms: u64,
    attempts: usize,
}

impl ConnectionErrorObservation {
    /// Returns the stable failure class.
    pub const fn class(&self) -> ConnectionErrorClass {
        self.class
    }
    /// Returns the bounded non-secret detail.
    pub fn detail(&self) -> &str {
        &self.detail
    }
    /// Returns the observation timestamp.
    pub const fn observed_at_ms(&self) -> u64 {
        self.observed_at_ms
    }
    /// Returns the number of attempts executed.
    pub const fn attempts(&self) -> usize {
        self.attempts
    }
}

/// One bounded observational connection state entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionObservation {
    remote_endpoint_id: EndpointId,
    path_state: ConnectionPathState,
    preferred_home_relay: Option<RelayUrl>,
    observed_at_ms: u64,
    last_success_at_ms: Option<u64>,
    last_error: Option<ConnectionErrorObservation>,
}

impl ConnectionObservation {
    /// Returns the TLS-authenticated remote Endpoint identity.
    pub const fn remote_endpoint_id(&self) -> EndpointId {
        self.remote_endpoint_id
    }
    /// Returns Iroh's observed path composition.
    pub const fn path_state(&self) -> ConnectionPathState {
        self.path_state
    }
    /// Returns the selected relay path when Iroh currently reports one.
    pub const fn preferred_home_relay(&self) -> Option<&RelayUrl> {
        self.preferred_home_relay.as_ref()
    }
    /// Returns the observation timestamp.
    pub const fn observed_at_ms(&self) -> u64 {
        self.observed_at_ms
    }
    /// Returns the most recent successful connection timestamp.
    pub const fn last_success_at_ms(&self) -> Option<u64> {
        self.last_success_at_ms
    }
    /// Returns the most recent bounded failure metadata.
    pub const fn last_error(&self) -> Option<&ConnectionErrorObservation> {
        self.last_error.as_ref()
    }
}

/// Cloneable bounded observational telemetry cache.
#[derive(Clone, Debug, Default)]
pub struct ConnectionTelemetry {
    state: Arc<Mutex<BTreeMap<EndpointId, VecDeque<ConnectionObservation>>>>,
}

#[derive(Clone, Copy)]
pub(crate) struct ConnectionObservationContext {
    target: EndpointId,
    now_ms: u64,
}

impl ConnectionObservationContext {
    pub(crate) const fn new(target: EndpointId, now_ms: u64) -> Self {
        Self { target, now_ms }
    }
}

impl ConnectionTelemetry {
    /// Returns retained observations for one exact remote Endpoint.
    pub fn observations(&self, target: EndpointId) -> Vec<ConnectionObservation> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&target)
            .map(|items| items.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn record_connecting(&self, target: EndpointId, now_ms: u64) {
        self.push(ConnectionObservation {
            remote_endpoint_id: target,
            path_state: ConnectionPathState::Connecting,
            preferred_home_relay: None,
            observed_at_ms: now_ms,
            last_success_at_ms: None,
            last_error: None,
        });
    }

    pub(crate) fn record_error(
        &self,
        context: ConnectionObservationContext,
        error: &crate::DialFailure,
    ) {
        self.push(ConnectionObservation {
            remote_endpoint_id: context.target,
            path_state: ConnectionPathState::MixedOrUnknown,
            preferred_home_relay: None,
            observed_at_ms: context.now_ms,
            last_success_at_ms: None,
            last_error: Some(ConnectionErrorObservation {
                class: error.class(),
                detail: error.detail().to_owned(),
                observed_at_ms: context.now_ms,
                attempts: error.attempts(),
            }),
        });
    }

    pub(crate) fn record_connection(
        &self,
        context: ConnectionObservationContext,
        connection: &Connection,
    ) {
        let paths = connection.paths();
        let mut direct = false;
        let mut relay = false;
        let mut unknown = false;
        let mut selected_relay = None;
        for path in &paths {
            match path.remote_addr() {
                TransportAddr::Ip(_) => direct = true,
                TransportAddr::Relay(url) => {
                    relay = true;
                    if path.is_selected() {
                        selected_relay = Some(url.clone());
                    }
                }
                _ => unknown = true,
            }
        }
        let path_state = match (direct, relay, unknown) {
            (true, false, false) => ConnectionPathState::Direct,
            (false, true, false) => ConnectionPathState::Relay,
            (false, false, _) | (true, true, _) | (_, _, true) => {
                ConnectionPathState::MixedOrUnknown
            }
        };
        self.push(ConnectionObservation {
            remote_endpoint_id: context.target,
            path_state,
            preferred_home_relay: selected_relay,
            observed_at_ms: context.now_ms,
            last_success_at_ms: Some(context.now_ms),
            last_error: None,
        });
    }

    fn push(&self, observation: ConnectionObservation) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.contains_key(&observation.remote_endpoint_id)
            && state.len() == MAX_REMOTE_ENDPOINTS
            && let Some(oldest) = state.keys().next().copied()
        {
            state.remove(&oldest);
        }
        let history = state.entry(observation.remote_endpoint_id).or_default();
        if history.len() == MAX_OBSERVATIONS_PER_REMOTE {
            history.pop_front();
        }
        history.push_back(observation);
        drop(state);
    }
}
