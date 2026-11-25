use iroh::protocol::RouterBuilder;
use tokio::sync::mpsc;

use super::RuntimeEndpoint;
use crate::{
    control::CONTROL_ALPN,
    echo_protocol::ECHO_ALPN,
    inbound::InboundLimiter,
    protocols::ENROLLMENT_ALPN,
    service_protocol::{
        STREAMING_SERVICES, ServiceCall, ServiceClient, ServiceHandler, ServiceMetrics,
        service_alpn,
    },
};

/// Registers one handler per streaming service ALPN.
pub(super) fn accept_services(
    mut router: RouterBuilder,
    calls: Option<&mpsc::Sender<ServiceCall>>,
    (metrics, inbound): (&ServiceMetrics, &InboundLimiter),
) -> RouterBuilder {
    for kind in STREAMING_SERVICES {
        if let Some(alpn) = service_alpn(kind) {
            router = router.accept(
                alpn,
                inbound.bound(ServiceHandler::new(kind, calls.cloned(), metrics.clone())),
            );
        }
    }
    router
}

impl RuntimeEndpoint {
    /// Returns the cloneable outbound streaming service client.
    pub fn service_client(&self) -> ServiceClient {
        self.service_client.clone()
    }

    /// Returns streaming service admission counters.
    pub fn service_metrics(&self) -> ServiceMetrics {
        self.service_metrics.clone()
    }

    /// Enables or disables every normal ALPN for new incoming connections.
    ///
    /// Membership in at least one Space enables them; zero Spaces leaves only enrollment.
    pub fn set_control_enabled(&self, enabled: bool) {
        let alpns = if enabled {
            [ENROLLMENT_ALPN, ECHO_ALPN, CONTROL_ALPN]
                .into_iter()
                .chain(STREAMING_SERVICES.into_iter().filter_map(service_alpn))
                .map(<[u8]>::to_vec)
                .collect()
        } else {
            vec![ENROLLMENT_ALPN.to_vec()]
        };
        self.router.endpoint().set_alpns(alpns);
    }
}
