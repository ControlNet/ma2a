use std::{collections::BTreeSet, error::Error, fmt, net::SocketAddr, path::PathBuf};

use iroh::{RelayMap, RelayMode};
use iroh_base::RelayUrl;
use ma2a_core::{MAX_SPACE_MEMBERS, SpaceId};
use ma2a_store::{RelayConfiguration, RelayTransportConfiguration};

use crate::NativeRelayTlsConfig;

const MAX_PUBLIC_FALLBACK_RELAYS: usize = 16;

/// Parsed runtime relay configuration loaded from Store-owned persistence records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeRelayConfiguration {
    public_fallback: Option<PublicRelayFallbackConfig>,
    private_provider: Option<PrivateRelayProviderConfig>,
}

impl RuntimeRelayConfiguration {
    /// Returns explicit public transport fallback when enabled.
    pub const fn public_fallback(&self) -> Option<&PublicRelayFallbackConfig> {
        self.public_fallback.as_ref()
    }

    /// Returns the local private relay provider role when enabled.
    pub const fn private_provider(&self) -> Option<&PrivateRelayProviderConfig> {
        self.private_provider.as_ref()
    }
}

impl TryFrom<RelayConfiguration> for RuntimeRelayConfiguration {
    type Error = RelayConfigError;

    fn try_from(configuration: RelayConfiguration) -> Result<Self, Self::Error> {
        let public_fallback = if configuration.public_fallback_enabled {
            Some(PublicRelayFallbackConfig::new(
                configuration
                    .public_relay_urls
                    .into_iter()
                    .map(|url| {
                        url.parse()
                            .map_err(|_| RelayConfigError::InvalidPersistedConfiguration)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )?)
        } else if configuration.public_relay_urls.is_empty() {
            None
        } else {
            return Err(RelayConfigError::InvalidPersistedConfiguration);
        };
        let private_provider = if configuration.private_provider_enabled {
            let listen_addr = configuration
                .listener_address
                .ok_or(RelayConfigError::InvalidPersistedConfiguration)?
                .parse()
                .map_err(|_| RelayConfigError::InvalidPersistedConfiguration)?;
            let public_relay_url = configuration
                .private_relay_url
                .ok_or(RelayConfigError::InvalidPersistedConfiguration)?
                .parse()
                .map_err(|_| RelayConfigError::InvalidPersistedConfiguration)?;
            let transport = match configuration
                .transport
                .ok_or(RelayConfigError::InvalidPersistedConfiguration)?
            {
                RelayTransportConfiguration::NativeTls {
                    certificate_path,
                    private_key_path,
                } => PrivateRelayTransport::NativeTls(NativeRelayTlsConfig::new(
                    PathBuf::from(certificate_path),
                    PathBuf::from(private_key_path),
                )),
                RelayTransportConfiguration::ExternalTlsTermination => {
                    PrivateRelayTransport::ExternalTlsTermination
                }
            };
            Some(PrivateRelayProviderConfig::new(
                PrivateRelayProviderLocation::new(listen_addr, public_relay_url),
                configuration.served_spaces,
                transport,
            )?)
        } else if configuration.listener_address.is_none()
            && configuration.private_relay_url.is_none()
            && configuration.served_spaces.is_empty()
            && configuration.transport.is_none()
        {
            None
        } else {
            return Err(RelayConfigError::InvalidPersistedConfiguration);
        };
        Ok(Self {
            public_fallback,
            private_provider,
        })
    }
}

/// Operator-selected transport mode for a private relay provider.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrivateRelayTransport {
    /// The embedded relay terminates TLS using operator-supplied files.
    NativeTls(NativeRelayTlsConfig),
    /// A compatible local proxy terminates TLS and forwards WebSocket upgrades.
    ExternalTlsTermination,
}

/// Listener and externally advertised URL for one private relay provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateRelayProviderLocation {
    listen_addr: SocketAddr,
    public_relay_url: RelayUrl,
}

impl PrivateRelayProviderLocation {
    /// Creates the local listener and public relay URL pair.
    pub const fn new(listen_addr: SocketAddr, public_relay_url: RelayUrl) -> Self {
        Self {
            listen_addr,
            public_relay_url,
        }
    }
}

/// Optional private relay provider role hosted by an existing Runtime Endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivateRelayProviderConfig {
    listen_addr: SocketAddr,
    public_relay_url: RelayUrl,
    served_spaces: Vec<SpaceId>,
    transport: PrivateRelayTransport,
}

impl PrivateRelayProviderConfig {
    /// Creates a private provider configuration without introducing another identity.
    ///
    /// # Errors
    /// Returns [`RelayConfigError`] for empty, duplicate, oversized, or unsafe configuration.
    pub fn new(
        location: PrivateRelayProviderLocation,
        mut served_spaces: Vec<SpaceId>,
        transport: PrivateRelayTransport,
    ) -> Result<Self, RelayConfigError> {
        let PrivateRelayProviderLocation {
            listen_addr,
            public_relay_url,
        } = location;
        if public_relay_url.scheme() != "https" {
            return Err(RelayConfigError::PublicUrlRequiresTls);
        }
        if served_spaces.is_empty() || served_spaces.len() > MAX_SPACE_MEMBERS {
            return Err(RelayConfigError::InvalidServedSpaces);
        }
        served_spaces.sort_unstable();
        let unique = served_spaces.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != served_spaces.len() {
            return Err(RelayConfigError::InvalidServedSpaces);
        }
        if matches!(transport, PrivateRelayTransport::ExternalTlsTermination)
            && !listen_addr.ip().is_loopback()
        {
            return Err(RelayConfigError::ExternalListenerMustBeLoopback);
        }
        Ok(Self {
            listen_addr,
            public_relay_url,
            served_spaces,
            transport,
        })
    }

    /// Returns the embedded relay or proxy-backend listener address.
    pub const fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Returns the externally advertised HTTPS relay URL.
    pub const fn public_relay_url(&self) -> &RelayUrl {
        &self.public_relay_url
    }

    /// Returns exact Spaces served by this provider.
    pub fn served_spaces(&self) -> &[SpaceId] {
        &self.served_spaces
    }

    /// Returns the operator-selected TLS deployment mode.
    pub const fn transport(&self) -> &PrivateRelayTransport {
        &self.transport
    }
}

/// Explicit operator-supplied public Iroh Relay transport fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicRelayFallbackConfig {
    relay_urls: Vec<RelayUrl>,
}

impl PublicRelayFallbackConfig {
    /// Creates a nonempty bounded public relay fallback set.
    ///
    /// # Errors
    /// Returns [`RelayConfigError`] for empty, duplicate, oversized, or non-HTTPS entries.
    pub fn new(mut relay_urls: Vec<RelayUrl>) -> Result<Self, RelayConfigError> {
        if relay_urls.is_empty() || relay_urls.len() > MAX_PUBLIC_FALLBACK_RELAYS {
            return Err(RelayConfigError::InvalidPublicFallback);
        }
        if relay_urls.iter().any(|url| url.scheme() != "https") {
            return Err(RelayConfigError::PublicUrlRequiresTls);
        }
        relay_urls.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
        let unique = relay_urls
            .iter()
            .map(|url| url.as_str())
            .collect::<BTreeSet<_>>();
        if unique.len() != relay_urls.len() {
            return Err(RelayConfigError::InvalidPublicFallback);
        }
        Ok(Self { relay_urls })
    }

    /// Returns operator-supplied public relay URLs.
    pub fn relay_urls(&self) -> &[RelayUrl] {
        &self.relay_urls
    }

    /// Returns the explicit Iroh custom relay mode for Endpoint construction.
    pub fn relay_mode(&self) -> RelayMode {
        RelayMode::Custom(self.relay_urls.iter().cloned().collect::<RelayMap>())
    }
}

/// Invalid private provider or public fallback configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelayConfigError {
    /// Private provider served Spaces are empty, duplicated, or over the bound.
    InvalidServedSpaces,
    /// Public fallback entries are empty, duplicated, or over the bound.
    InvalidPublicFallback,
    /// Publicly advertised relay URLs must use HTTPS.
    PublicUrlRequiresTls,
    /// Plain external-termination backends must be restricted to loopback.
    ExternalListenerMustBeLoopback,
    /// Store-owned relay configuration is malformed or internally inconsistent.
    InvalidPersistedConfiguration,
}

impl fmt::Display for RelayConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidServedSpaces => {
                formatter.write_str("private relay served Spaces are invalid")
            }
            Self::InvalidPublicFallback => {
                formatter.write_str("public relay fallback set is invalid")
            }
            Self::PublicUrlRequiresTls => formatter.write_str("public relay URL must use HTTPS"),
            Self::ExternalListenerMustBeLoopback => formatter
                .write_str("external TLS termination backend listener must use a loopback address"),
            Self::InvalidPersistedConfiguration => {
                formatter.write_str("persisted relay configuration is invalid")
            }
        }
    }
}

impl Error for RelayConfigError {}
