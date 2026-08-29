use std::{error::Error, fmt, net::Ipv4Addr, time::Duration};

use iroh::{
    Endpoint, EndpointAddr, RelayMode, SecretKey, address_lookup::UserData, endpoint::presets,
    protocol::Router,
};
use ma2a_core::EndpointId;
use zeroize::Zeroizing;

use tokio::sync::mpsc;

use crate::{
    AddressPublisher, AddressPublisherError, SpaceAddressLookup,
    address_lookup::RuntimeAddressLookup,
    address_observation::AddressObservationWaitError,
    enrollment::{EnrollmentCall, EnrollmentHandler, exchange},
    protocols::ENROLLMENT_ALPN,
};

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
            .map_err(|_| InvalidEndpointSecret { found: bytes.len() })
    }

    /// Returns the public Endpoint identity derived from this secret.
    pub fn endpoint_id(&self) -> EndpointId {
        self.0.public().into()
    }

    /// Copies private bytes into a zeroizing short-lived persistence buffer.
    pub fn protected_bytes(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.0.to_bytes())
    }
}

impl fmt::Debug for EndpointSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EndpointSecret(..)")
    }
}

/// Invalid protected Endpoint key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidEndpointSecret {
    found: usize,
}

impl InvalidEndpointSecret {
    /// Returns the invalid byte length without exposing key material.
    pub const fn found_length(self) -> usize {
        self.found
    }
}

impl fmt::Display for InvalidEndpointSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "protected Endpoint key has invalid length {}; expected 32 bytes",
            self.found
        )
    }
}

impl Error for InvalidEndpointSecret {}

/// Failures while binding or shutting down the Iroh Endpoint.
#[derive(Debug)]
pub struct NetError(NetErrorKind);

#[derive(Debug)]
enum NetErrorKind {
    Bind(iroh::endpoint::BindError),
    Observation(AddressObservationWaitError),
    Shutdown,
    Enrollment,
}

impl fmt::Display for NetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            NetErrorKind::Bind(error) => write!(formatter, "Iroh Endpoint bind failed: {error}"),
            NetErrorKind::Observation(error) => {
                write!(formatter, "Iroh Endpoint observation failed: {error}")
            }
            NetErrorKind::Shutdown => formatter.write_str("Iroh Endpoint shutdown task failed"),
            NetErrorKind::Enrollment => formatter.write_str("Iroh enrollment exchange failed"),
        }
    }
}

impl NetError {
    pub(crate) const fn enrollment() -> Self {
        Self(NetErrorKind::Enrollment)
    }
}

impl Error for NetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.0 {
            NetErrorKind::Bind(error) => Some(error),
            NetErrorKind::Observation(error) => Some(error),
            NetErrorKind::Shutdown | NetErrorKind::Enrollment => None,
        }
    }
}

/// One long-lived privacy-preserving Iroh Endpoint and its enrollment-only router.
#[derive(Debug)]
pub struct RuntimeEndpoint {
    router: Router,
    observation: crate::address_observation::AddressObservation,
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
            enrollment_calls,
            bind_port,
            SpaceAddressLookup::default(),
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
        enrollment_calls: mpsc::Sender<EnrollmentCall>,
        bind_port: Option<u16>,
        lookup: SpaceAddressLookup,
    ) -> Result<Self, NetError> {
        let runtime_lookup = RuntimeAddressLookup::new(lookup);
        let observation = runtime_lookup.observation();
        let builder = Endpoint::builder(presets::Minimal)
            .secret_key(secret.0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .address_lookup(runtime_lookup)
            .alpns(vec![ENROLLMENT_ALPN.to_vec()]);
        let builder = if let Some(port) = bind_port {
            builder
                .bind_addr((Ipv4Addr::UNSPECIFIED, port))
                .map_err(|_| NetError(NetErrorKind::Enrollment))?
        } else {
            builder
        };
        let endpoint = builder
            .bind()
            .await
            .map_err(|error| NetError(NetErrorKind::Bind(error)))?;
        if let Err(error) = observation
            .wait_for_initial(INITIAL_ADDRESS_OBSERVATION_TIMEOUT)
            .await
        {
            endpoint.close().await;
            return Err(NetError(NetErrorKind::Observation(error)));
        }
        let router = Router::builder(endpoint)
            .accept(ENROLLMENT_ALPN, EnrollmentHandler::new(enrollment_calls))
            .spawn();
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
            .map_err(|_| NetError(NetErrorKind::Shutdown))?;
        Ok(endpoint.is_closed())
    }
}
