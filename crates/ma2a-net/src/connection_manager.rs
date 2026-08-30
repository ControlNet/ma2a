use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
};

use iroh::{Endpoint, endpoint::Connection};
use iroh_base::RelayUrl;
use n0_future::StreamExt as _;
use tokio::task::JoinSet;

use crate::{
    ConnectionManagerOptions, ConnectionObservationContext, ConnectionTelemetry, DialCancellation,
    DialFailure, DialRequest, DialRetryPolicy, LocalIrohRelayMap, RelayReconfigureError,
    RelayReconfigureOutcome,
    dial::{DialDependencies, dial_with_retry},
    reconfigure::apply_relay_map,
};

const MAX_ACTIVE_CONNECTIONS: usize = 128;

/// Cloneable bounded Iroh connection owner and observational telemetry source.
#[derive(Clone)]
pub struct ConnectionManager(Arc<ManagerInner>);

struct ManagerInner {
    endpoint: Endpoint,
    options: ConnectionManagerOptions,
    telemetry: ConnectionTelemetry,
    cancellation: DialCancellation,
    active: Mutex<BTreeMap<usize, Connection>>,
    observers: Mutex<JoinSet<()>>,
    configured_relays: tokio::sync::Mutex<BTreeSet<RelayUrl>>,
}

impl ConnectionManager {
    /// Creates a production manager around one running Iroh Endpoint.
    pub fn new(endpoint: Endpoint) -> Self {
        Self::with_options(endpoint, ConnectionManagerOptions::default())
    }

    /// Creates a manager with injected deterministic dependencies.
    pub fn with_options(endpoint: Endpoint, options: ConnectionManagerOptions) -> Self {
        Self::with_options_and_relays(endpoint, options, BTreeSet::new())
    }

    pub(crate) fn with_relays(endpoint: Endpoint, configured_relays: BTreeSet<RelayUrl>) -> Self {
        Self::with_options_and_relays(
            endpoint,
            ConnectionManagerOptions::default(),
            configured_relays,
        )
    }

    fn with_options_and_relays(
        endpoint: Endpoint,
        options: ConnectionManagerOptions,
        configured_relays: BTreeSet<RelayUrl>,
    ) -> Self {
        Self(Arc::new(ManagerInner {
            endpoint,
            options,
            telemetry: ConnectionTelemetry::default(),
            cancellation: DialCancellation::new(),
            active: Mutex::new(BTreeMap::new()),
            observers: Mutex::new(JoinSet::new()),
            configured_relays: tokio::sync::Mutex::new(configured_relays),
        }))
    }

    /// Resolves and dials the exact target with finite transient recovery.
    ///
    /// # Errors
    /// Returns [`DialFailure`] when resolution, connection, identity verification, or local bounds
    /// prevent establishing the requested connection.
    pub async fn connect(&self, request: DialRequest) -> Result<Connection, DialFailure> {
        self.connect_with_policy(request, self.0.options.retry)
            .await
    }

    async fn connect_with_policy(
        &self,
        request: DialRequest,
        retry: DialRetryPolicy,
    ) -> Result<Connection, DialFailure> {
        self.0
            .telemetry
            .record_connecting(request.target(), self.0.options.clock.now_ms());
        let result = dial_with_retry(
            &self.0.endpoint,
            &request,
            DialDependencies {
                driver: self.0.options.driver.as_ref(),
                clock: self.0.options.clock.as_ref(),
                jitter: self.0.options.jitter.as_ref(),
                retry,
                manager_cancellation: &self.0.cancellation,
            },
        )
        .await;
        match result {
            Ok(connection) => self.register(request.target(), connection),
            Err(error) => {
                self.0.telemetry.record_error(
                    ConnectionObservationContext::new(
                        request.target(),
                        self.0.options.clock.now_ms(),
                    ),
                    &error,
                );
                Err(error)
            }
        }
    }

    pub(crate) async fn connect_addr(
        &self,
        endpoint_addr: iroh::EndpointAddr,
        alpn: &[u8],
    ) -> Result<Connection, DialFailure> {
        self.connect_with_policy(
            DialRequest::from_addr(endpoint_addr, alpn.to_vec()),
            DialRetryPolicy::single_attempt(),
        )
        .await
    }

    /// Returns the bounded observational telemetry cache.
    pub fn telemetry(&self) -> ConnectionTelemetry {
        self.0.telemetry.clone()
    }

    /// Applies Iroh's supported asynchronous relay-map mutation API.
    ///
    /// # Errors
    /// Returns [`RelayReconfigureError`] when the Endpoint is closed, changes identity, or the
    /// supplied relay map cannot be applied.
    pub async fn reconfigure(
        &self,
        relay_map: &LocalIrohRelayMap,
    ) -> Result<RelayReconfigureOutcome, RelayReconfigureError> {
        let mut configured = self.0.configured_relays.lock().await;
        apply_relay_map(&self.0.endpoint, &mut configured, relay_map).await
    }

    /// Cancels dials, closes active connections, and joins path observers.
    pub async fn shutdown(&self) {
        self.0.cancellation.cancel();
        let connections = {
            let mut active = self
                .0
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *active)
        };
        for connection in connections.into_values() {
            connection.close(0_u8.into(), b"");
        }
        let mut observers = {
            let mut owned = self
                .0
                .observers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::replace(&mut *owned, JoinSet::new())
        };
        observers.shutdown().await;
    }

    fn register(
        &self,
        target: ma2a_core::EndpointId,
        connection: Connection,
    ) -> Result<Connection, DialFailure> {
        if connection.remote_id()
            != target.to_public_key().map_err(|_| {
                DialFailure::malformed_input("connected Endpoint identity is invalid")
            })?
        {
            connection.close(1_u8.into(), b"");
            return Err(DialFailure::authorization_denied(
                "connected Endpoint identity differs from target",
            ));
        }
        let stable_id = connection.stable_id();
        {
            let mut active = self
                .0
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if active.len() == MAX_ACTIVE_CONNECTIONS {
                connection.close(1_u8.into(), b"");
                return Err(DialFailure::policy_denied(
                    "active connection bound reached",
                ));
            }
            active.insert(stable_id, connection.clone());
        }
        self.0.telemetry.record_connection(
            ConnectionObservationContext::new(target, self.0.options.clock.now_ms()),
            &connection,
        );
        self.spawn_observer(target, connection.clone());
        Ok(connection)
    }

    fn spawn_observer(&self, target: ma2a_core::EndpointId, connection: Connection) {
        let manager = self.clone();
        let stable_id = connection.stable_id();
        let mut observers = self
            .0
            .observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while observers.try_join_next().is_some() {}
        observers.spawn(async move {
            let mut events = connection.path_events();
            while events.next().await.is_some() {
                manager.0.telemetry.record_connection(
                    ConnectionObservationContext::new(target, manager.0.options.clock.now_ms()),
                    &connection,
                );
            }
            manager
                .0
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&stable_id);
        });
    }
}

impl fmt::Debug for ConnectionManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionManager")
            .field("endpoint_id", &self.0.endpoint.id())
            .finish_non_exhaustive()
    }
}
