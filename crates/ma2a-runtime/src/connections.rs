use ma2a_core::EndpointId;

/// Runtime projection of bounded observational Iroh connection telemetry.
#[derive(Clone, Debug)]
pub struct RuntimeConnections {
    telemetry: ma2a_net::ConnectionTelemetry,
}

impl RuntimeConnections {
    pub(crate) fn new(manager: &ma2a_net::ConnectionManager) -> Self {
        Self {
            telemetry: manager.telemetry(),
        }
    }

    /// Returns retained observations for one exact remote Endpoint.
    pub fn observations(&self, target: EndpointId) -> Vec<ma2a_net::ConnectionObservation> {
        self.telemetry.observations(target)
    }
}
