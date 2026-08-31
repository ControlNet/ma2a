use std::net::SocketAddr;

use ma2a_core::ProtocolError;
use ma2a_store::{RelayConfiguration, RelayTransportConfiguration};

use crate::api::{CommandResult, PrivateRelayView, PublicRelayView, RelayAddress};

use super::super::ConnectionContext;

pub(super) async fn private_configure(
    context: &ConnectionContext,
    command: &crate::api::Command,
) -> Result<CommandResult, ProtocolError> {
    let requested = command
        .private_relay_configuration()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let mut configuration = context
        .handle
        .relay_configuration()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    configuration.private_provider_enabled = true;
    configuration.listener_address = Some(requested.listen.as_str().to_owned());
    configuration.private_relay_url = Some(requested.public_url.as_str().to_owned());
    configuration
        .served_spaces
        .clone_from(&requested.served_spaces);
    configuration.transport = Some(match requested.mode {
        crate::api::PrivateRelayMode::NativeTls => RelayTransportConfiguration::NativeTls {
            certificate_path: requested
                .certificate_path
                .as_ref()
                .ok_or(ProtocolError::INVALID_INPUT)?
                .as_str()
                .to_owned(),
            private_key_path: requested
                .private_key_path
                .as_ref()
                .ok_or(ProtocolError::INVALID_INPUT)?
                .as_str()
                .to_owned(),
        },
        crate::api::PrivateRelayMode::ExternalTermination => {
            if requested.certificate_path.is_some() || requested.private_key_path.is_some() {
                return Err(ProtocolError::INVALID_INPUT);
            }
            RelayTransportConfiguration::ExternalTlsTermination
        }
    });
    context
        .handle
        .set_relay_configuration(configuration.clone())
        .await
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok(CommandResult::private_relay_configured(private_view(
        &configuration,
        true,
    )?))
}

pub(super) async fn private_disable(
    context: &ConnectionContext,
) -> Result<CommandResult, ProtocolError> {
    let mut configuration = context
        .handle
        .relay_configuration()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    configuration.private_provider_enabled = false;
    configuration.listener_address = None;
    configuration.private_relay_url = None;
    configuration.served_spaces.clear();
    configuration.transport = None;
    context
        .handle
        .set_relay_configuration(configuration.clone())
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    Ok(CommandResult::private_relay_status(private_view(
        &configuration,
        false,
    )?))
}

pub(super) async fn private_status(
    context: &ConnectionContext,
) -> Result<CommandResult, ProtocolError> {
    let configuration = context
        .handle
        .relay_configuration()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    Ok(CommandResult::private_relay_status(private_view(
        &configuration,
        configuration.private_provider_enabled,
    )?))
}

pub(super) async fn public_configure(
    context: &ConnectionContext,
    url: &str,
) -> Result<CommandResult, ProtocolError> {
    let mut configuration = context
        .handle
        .relay_configuration()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    configuration.public_fallback_enabled = true;
    configuration.public_relay_urls = vec![url.to_owned()];
    context
        .handle
        .set_relay_configuration(configuration)
        .await
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok(CommandResult::public_relay_configured(
        PublicRelayView::new(true, Some(url.to_owned()), false)
            .map_err(|_| ProtocolError::INVALID_INPUT)?,
    ))
}

pub(super) async fn public_disable(
    context: &ConnectionContext,
) -> Result<CommandResult, ProtocolError> {
    let mut configuration = context
        .handle
        .relay_configuration()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    configuration.public_fallback_enabled = false;
    configuration.public_relay_urls.clear();
    context
        .handle
        .set_relay_configuration(configuration)
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    Ok(CommandResult::public_relay_status(
        PublicRelayView::new(false, None, false).map_err(|_| ProtocolError::INTERNAL)?,
    ))
}

pub(super) async fn public_status(
    context: &ConnectionContext,
) -> Result<CommandResult, ProtocolError> {
    let configuration = context
        .handle
        .relay_configuration()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    PublicRelayView::new(
        configuration.public_fallback_enabled,
        configuration.public_relay_urls.first().cloned(),
        false,
    )
    .map(CommandResult::public_relay_status)
    .map_err(|_| ProtocolError::INTERNAL)
}

fn private_view(
    configuration: &RelayConfiguration,
    online: bool,
) -> Result<PrivateRelayView, ProtocolError> {
    let configured = configuration.private_provider_enabled;
    let address: SocketAddr = configuration
        .listener_address
        .as_deref()
        .unwrap_or("127.0.0.1:0")
        .parse()
        .map_err(|_| ProtocolError::INTERNAL)?;
    let mode = match configuration.transport {
        Some(RelayTransportConfiguration::NativeTls { .. }) => "native_tls",
        Some(RelayTransportConfiguration::ExternalTlsTermination) | None => "external_termination",
    };
    Ok(PrivateRelayView::new(
        configured,
        RelayAddress::new(mode, &address.ip().to_string(), address.port())
            .map_err(|_| ProtocolError::INTERNAL)?,
        online,
    ))
}
