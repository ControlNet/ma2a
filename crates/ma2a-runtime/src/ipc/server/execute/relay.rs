use std::net::SocketAddr;

use ma2a_core::ProtocolError;
use ma2a_store::{RelayConfiguration, RelayTransportConfiguration};

use crate::api::{ApiError, CommandResult, PrivateRelayView, PublicRelayView, RelayAddress};

use super::super::ConnectionContext;

pub(super) async fn private_configure(
    context: &ConnectionContext,
    command: &crate::api::Command,
) -> Result<CommandResult, ApiError> {
    let requested = command.private_relay_configuration().ok_or_else(not_run)?;
    let mut configuration = current(context).await?;
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
                .ok_or_else(not_run)?
                .as_str()
                .to_owned(),
            private_key_path: requested
                .private_key_path
                .as_ref()
                .ok_or_else(not_run)?
                .as_str()
                .to_owned(),
        },
        crate::api::PrivateRelayMode::ExternalTermination => {
            if requested.certificate_path.is_some() || requested.private_key_path.is_some() {
                return Err(not_run());
            }
            RelayTransportConfiguration::ExternalTlsTermination
        }
    });
    refuse_unusable(&configuration)?;
    let committed = context
        .handle
        .reconfigure_private_relay(configuration.clone())
        .await
        .map_err(|error| relay_error(&error))?;
    Ok(CommandResult::private_relay_configured(after_commit(
        private_view(committed.value()),
        &committed,
    )?)
    .at_revision(committed.revision()))
}

pub(super) async fn private_disable(
    context: &ConnectionContext,
) -> Result<CommandResult, ApiError> {
    let mut configuration = current(context).await?;
    configuration.private_provider_enabled = false;
    configuration.listener_address = None;
    configuration.private_relay_url = None;
    configuration.served_spaces.clear();
    configuration.transport = None;
    let committed = context
        .handle
        .set_relay_configuration_committed(configuration.clone())
        .await
        .map_err(|error| relay_error(&error))?;
    Ok(CommandResult::private_relay_status(after_commit(
        private_view(committed.value()),
        &committed,
    )?)
    .at_revision(committed.revision()))
}

pub(super) async fn private_status(
    context: &ConnectionContext,
) -> Result<CommandResult, ProtocolError> {
    let status = context
        .handle
        .relay_status()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    Ok(CommandResult::private_relay_status(private_view(&status)?).at_revision(status.revision))
}

pub(super) async fn public_configure(
    context: &ConnectionContext,
    url: &str,
) -> Result<CommandResult, ApiError> {
    refuse_unusable(&RelayConfiguration {
        public_fallback_enabled: true,
        public_relay_urls: vec![url.to_owned()],
        private_provider_enabled: false,
        listener_address: None,
        private_relay_url: None,
        served_spaces: Vec::new(),
        transport: None,
    })?;
    let committed = context
        .handle
        .set_public_relay_configuration(true, vec![url.to_owned()])
        .await
        .map_err(|error| relay_error(&error))?;
    Ok(CommandResult::public_relay_configured(after_commit(
        public_view(committed.value()),
        &committed,
    )?)
    .at_revision(committed.revision()))
}

pub(super) async fn public_disable(context: &ConnectionContext) -> Result<CommandResult, ApiError> {
    let committed = context
        .handle
        .set_public_relay_configuration(false, Vec::new())
        .await
        .map_err(|error| relay_error(&error))?;
    Ok(CommandResult::public_relay_status(after_commit(
        public_view(committed.value()),
        &committed,
    )?)
    .at_revision(committed.revision()))
}

pub(super) async fn public_status(
    context: &ConnectionContext,
) -> Result<CommandResult, ProtocolError> {
    let status = context
        .handle
        .relay_status()
        .await
        .map_err(|_| ProtocolError::INTERNAL)?;
    PublicRelayView::new(
        status.configuration.public_fallback_enabled,
        status.configuration.public_relay_urls.first().cloned(),
        status.public_relay_online,
    )
    .map(|view| CommandResult::public_relay_status(view).at_revision(status.revision))
    .map_err(|_| ProtocolError::INTERNAL)
}

fn private_view(
    status: &crate::actor::RelayRuntimeStatus,
) -> Result<PrivateRelayView, ProtocolError> {
    let configuration: &RelayConfiguration = &status.configuration;
    let configured = configuration.private_provider_enabled;
    let applied_address = status
        .private_listen_addr
        .filter(|_| !status.convergence_pending);
    let address: SocketAddr = applied_address
        .map_or_else(
            || {
                configuration
                    .listener_address
                    .as_deref()
                    .unwrap_or("127.0.0.1:0")
                    .parse()
            },
            Ok,
        )
        .map_err(|_| ProtocolError::INTERNAL)?;
    let mode = match configuration.transport {
        Some(RelayTransportConfiguration::NativeTls { .. }) => "native_tls",
        Some(RelayTransportConfiguration::ExternalTlsTermination) | None => "external_termination",
    };
    Ok(PrivateRelayView::new(
        configured,
        RelayAddress::new(mode, &address.ip().to_string(), address.port())
            .map_err(|_| ProtocolError::INTERNAL)?,
        status.applied_private.is_some() && !status.convergence_pending,
    ))
}

/// Refuses, before anything runs, a configuration the Runtime could not apply, such as a
/// listener address that does not parse or an external TLS backend off loopback.
fn refuse_unusable(configuration: &RelayConfiguration) -> Result<(), ApiError> {
    ma2a_net::RuntimeRelayConfiguration::try_from(configuration.clone())
        .map(drop)
        .map_err(|_| not_run())
}

/// The stored configuration a change starts from, read before anything runs.
async fn current(context: &ConnectionContext) -> Result<RelayConfiguration, ApiError> {
    context
        .handle
        .relay_configuration()
        .await
        .map_err(|_| ApiError::new(ProtocolError::INTERNAL).without_effects())
}

/// Invalid input found before the request ran.
const fn not_run() -> ApiError {
    ApiError::invalid_input().without_effects()
}

/// A view that fails after its configuration committed still reports that commit.
fn after_commit<T, V>(
    view: Result<V, ProtocolError>,
    committed: &ma2a_store::Committed<T>,
) -> Result<V, ApiError> {
    view.map_err(|error| ApiError::from(error).after_commit(committed.revision()))
}

const fn relay_error(error: &crate::RuntimeError) -> ApiError {
    let api = ApiError::new(ProtocolError::INTERNAL);
    match error.committed_revision() {
        Some(revision) => api.after_commit(revision),
        None if error.is_pre_commit_relay_tls_refusal() => api.without_effects(),
        None => api,
    }
}

fn public_view(
    status: &crate::actor::RelayRuntimeStatus,
) -> Result<PublicRelayView, ProtocolError> {
    PublicRelayView::new(
        status.configuration.public_fallback_enabled,
        status.configuration.public_relay_urls.first().cloned(),
        status.public_relay_online,
    )
    .map_err(|_| ProtocolError::INTERNAL)
}
