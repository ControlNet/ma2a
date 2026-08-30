mod error;

use std::{fmt, net::Ipv4Addr, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, RelayMode, SecretKey, address_lookup::UserData, endpoint::presets,
    protocol::Router,
};
use ma2a_core::EndpointId;
use zeroize::Zeroizing;

use tokio::sync::mpsc;

use crate::{
    AddressPublisher, AddressPublisherError, PublicRelayFallbackConfig, SpaceAddressLookup,
    address_lookup::RuntimeAddressLookup,
    control::{CONTROL_ALPN, ControlCall, ControlClient, ControlHandler},
    enrollment::{EnrollmentCall, EnrollmentHandler, exchange},
    protocols::ENROLLMENT_ALPN,
};

pub use error::{InvalidEndpointSecret, NetError};

const INITIAL_ADDRESS_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(2);

/// A zeroizing Iroh Endpoint secret that never reveals private bytes through `Debug`.
#[derive(Clone)]
pub struct EndpointSecret(SecretKey);

impl EndpointSecret {
    /// Generates one fresh Iroh Endpoint identity.
    pub fn generate() -> Self {
        Self(SecretKey::generate())
    }

    /// Parses exact protected key bytes.
    ///
    /// # Errors
    /// Returns [`InvalidEndpointSecret`] unless exactly 32 bytes are provided.
    pub fn parse(bytes: &[u8]) -> Result<Self, InvalidEndpointSecret> {
        SecretKey::try_from(bytes)
            .map(Self)
            .map_err(|_| InvalidEndpointSecret::new(bytes.len()))
    }

    /// Returns the public Endpoint identity derived from this secret.
    pub fn endpoint_id(&self) -> EndpointId {
        self.0.public().into()
    }

    /// Copies private bytes into a zeroizing short-lived persistence buffer.
    pub fn protected_bytes(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.0.to_bytes())
    }

    pub(crate) const fn iroh_secret(&self) -> &SecretKey {
        &self.0
    }
}

impl fmt::Debug for EndpointSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EndpointSecret(..)")
    }
}

/// One long-lived privacy-preserving Iroh Endpoint and its enrollment-only router.
#[derive(Debug)]
pub struct RuntimeEndpoint {
    router: Router,
    observation: crate::address_observation::AddressObservation,
}

/// Enrollment routing inputs for a lookup-aware Runtime Endpoint bind.
#[derive(Debug)]
pub struct EndpointBindOptions {
    enrollment_calls: mpsc::Sender<EnrollmentCall>,
    bind_port: Option<u16>,
    public_relay_fallback: Option<PublicRelayFallbackConfig>,
    control_calls: Option<mpsc::Sender<ControlCall>>,
    control_enabled: bool,
}

impl EndpointBindOptions {
    /// Creates enrollment routing inputs with an optional fixed UDP port.
    pub const fn new(
        enrollment_calls: mpsc::Sender<EnrollmentCall>,
        bind_port: Option<u16>,
    ) -> Self {
        Self {
            enrollment_calls,
            bind_port,
            public_relay_fallback: None,
            control_calls: None,
            control_enabled: false,
        }
    }

    /// Enables explicit operator-supplied public relay transport fallback.
    #[must_use]
    pub fn with_public_relay_fallback(
        mut self,
        public_relay_fallback: PublicRelayFallbackConfig,
    ) -> Self {
        self.public_relay_fallback = Some(public_relay_fallback);
        self
    }

    /// Registers the existing-member control handler and initial ALPN eligibility.
    #[must_use]
    pub fn with_control(mut self, control_calls: mpsc::Sender<ControlCall>, enabled: bool) -> Self {
        self.control_calls = Some(control_calls);
        self.control_enabled = enabled;
        self
    }
}

impl RuntimeEndpoint {
    /// Binds direct transports with public discovery and relay publication disabled.
    ///
    /// # Errors
    /// Returns [`NetError`] when Iroh cannot bind the Endpoint or does not publish its initial
    /// local observation within two seconds.
    pub async fn bind(
        secret: EndpointSecret,
        enrollment_calls: mpsc::Sender<EnrollmentCall>,
        bind_port: Option<u16>,
    ) -> Result<Self, NetError> {
        Self::bind_with_lookup(
            secret,
            SpaceAddressLookup::default(),
            EndpointBindOptions::new(enrollment_calls, bind_port),
        )
        .await
    }

    /// Binds direct transports with the supplied private Space address lookup.
    ///
    /// # Errors
    /// Returns [`NetError`] when Iroh cannot bind the Endpoint or does not publish its initial
    /// local observation within two seconds.
    pub async fn bind_with_lookup(
        secret: EndpointSecret,
        lookup: SpaceAddressLookup,
        options: EndpointBindOptions,
    ) -> Result<Self, NetError> {
        let EndpointBindOptions {
            enrollment_calls,
            bind_port,
            public_relay_fallback,
            control_calls,
            control_enabled,
        } = options;
        let runtime_lookup = RuntimeAddressLookup::new(lookup);
        let observation = runtime_lookup.observation();
        let relay_mode =
            public_relay_fallback.map_or(RelayMode::Disabled, |fallback| fallback.relay_mode());
        let builder = Endpoint::builder(presets::Minimal)
            .secret_key(secret.0)
            .relay_mode(relay_mode)
            .clear_address_lookup()
            .address_lookup(runtime_lookup)
            .alpns(vec![ENROLLMENT_ALPN.to_vec()]);
        let builder = if let Some(port) = bind_port {
            builder
                .bind_addr((Ipv4Addr::UNSPECIFIED, port))
                .map_err(|_| NetError::enrollment())?
        } else {
            builder
        };
        let endpoint = builder.bind().await.map_err(NetError::bind)?;
        if let Err(error) = observation
            .wait_for_initial(INITIAL_ADDRESS_OBSERVATION_TIMEOUT)
            .await
        {
            endpoint.close().await;
            return Err(NetError::observation(error));
        }
        let router = Router::builder(endpoint)
            .accept(ENROLLMENT_ALPN, EnrollmentHandler::new(enrollment_calls))
            .accept(CONTROL_ALPN, ControlHandler::new(control_calls))
            .spawn();
        if !control_enabled {
            router.endpoint().set_alpns(vec![ENROLLMENT_ALPN.to_vec()]);
        }
        Ok(Self {
            router,
            observation,
        })
    }

    /// Returns the local UDP port advertised for direct enrollment.
    ///
    /// # Errors
    /// Returns [`NetError`] when the bound Endpoint has no direct address.
    pub fn bind_port(&self) -> Result<u16, NetError> {
        self.router
            .endpoint()
            .addr()
            .ip_addrs()
            .next()
            .map(std::net::SocketAddr::port)
            .ok_or_else(NetError::enrollment)
    }

    /// Returns the public Endpoint identity.
    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id().into()
    }

    /// Returns current direct and relay addressing observations.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.router.endpoint().addr()
    }

    /// Exchanges one bounded enrollment request over the reserved ALPN.
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] when connection, stream, framing, or response validation fails.
    pub async fn exchange_enrollment(
        &self,
        owner: EndpointAddr,
        request: &[u8],
    ) -> Result<(u8, Vec<Vec<u8>>), NetError> {
        exchange(self.router.endpoint(), owner, request).await
    }

    /// Exchanges one bounded control request over the existing-member ALPN.
    ///
    /// # Errors
    /// Returns [`NetError`] when connection, framing, timeout, or peer validation fails.
    pub async fn exchange_control(
        &self,
        peer: EndpointAddr,
        request: &[u8],
    ) -> Result<Vec<u8>, NetError> {
        self.control_client().exchange_addr(peer, request).await
    }

    /// Returns a cloneable active control dial client.
    pub fn control_client(&self) -> ControlClient {
        ControlClient::new(self.router.endpoint().clone())
    }

    /// Enables or disables control ALPN negotiation for new incoming connections.
    pub fn set_control_enabled(&self, enabled: bool) {
        let alpns = if enabled {
            vec![ENROLLMENT_ALPN.to_vec(), CONTROL_ALPN.to_vec()]
        } else {
            vec![ENROLLMENT_ALPN.to_vec()]
        };
        self.router.endpoint().set_alpns(alpns);
    }

    /// Creates a publisher backed by this running Endpoint's current observations.
    ///
    /// Live publisher construction is provenance-bound to this Runtime Endpoint:
    /// ```compile_fail
    /// use ma2a_net::AddressPublisher;
    ///
    /// let _ = AddressPublisher::from_endpoint;
    /// ```
    ///
    /// # Errors
    /// Returns [`AddressPublisherError`] when current transport data is not publishable.
    pub fn address_publisher(&self) -> Result<AddressPublisher, AddressPublisherError> {
        AddressPublisher::from_endpoint(self.router.endpoint(), self.observation.clone())
    }

    /// Updates application-defined data included in the live Iroh observation.
    pub fn set_user_data_for_address_lookup(&self, user_data: Option<UserData>) {
        self.router
            .endpoint()
            .set_user_data_for_address_lookup(user_data);
    }

    /// Closes the Endpoint and joins the protocol router.
    ///
    /// # Errors
    /// Returns [`NetError`] when the router task fails during shutdown.
    pub async fn shutdown(self) -> Result<bool, NetError> {
        let endpoint = self.router.endpoint().clone();
        self.router
            .shutdown()
            .await
            .map_err(|_| NetError::shutdown())?;
        Ok(endpoint.is_closed())
    }
}
