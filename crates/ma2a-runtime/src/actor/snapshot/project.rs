//! Projections from durable and observational state into wire views.

use crate::{
    api::{ConnectionObservationView, SnapshotSpaceView, SpaceChainHead, SpaceMemberView},
    error::{RuntimeError, RuntimeErrorKind},
};

pub(super) fn space_view(
    space: &ma2a_store::SnapshotSpace,
) -> Result<SnapshotSpaceView, RuntimeError> {
    SnapshotSpaceView::new(
        space.space_id(),
        space.label(),
        SpaceChainHead::new(
            space.generation(),
            space.chain_hash(),
            space.revoked_count(),
        ),
        space.member_count(),
    )
    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
}

/// The latest-connection projection keeps its existing three values, so an
/// in-progress attempt still reports `mixed_or_unknown` and `state` carries the
/// fact that it is connecting.
const fn path_label(state: ma2a_net::ConnectionPathState) -> &'static str {
    match state {
        ma2a_net::ConnectionPathState::Relay => "relay",
        ma2a_net::ConnectionPathState::Direct => "direct",
        _ => "mixed_or_unknown",
    }
}

/// Retained observations distinguish an in-progress attempt, because a history
/// entry has no separate state field to carry it.
const fn observation_path_label(state: ma2a_net::ConnectionPathState) -> &'static str {
    match state {
        ma2a_net::ConnectionPathState::Connecting => "connecting",
        ma2a_net::ConnectionPathState::Relay => "relay",
        ma2a_net::ConnectionPathState::Direct => "direct",
        _ => "mixed_or_unknown",
    }
}

#[expect(
    clippy::match_same_arms,
    reason = "unknown future error classes retain the conservative transient label"
)]
const fn error_label(class: ma2a_net::ConnectionErrorClass) -> &'static str {
    match class {
        ma2a_net::ConnectionErrorClass::Transient => "transient",
        ma2a_net::ConnectionErrorClass::Authorization => "authorization",
        ma2a_net::ConnectionErrorClass::Version => "version",
        ma2a_net::ConnectionErrorClass::Revocation => "revocation",
        ma2a_net::ConnectionErrorClass::MalformedInput => "malformed_input",
        ma2a_net::ConnectionErrorClass::Policy => "policy",
        ma2a_net::ConnectionErrorClass::Cancelled => "cancelled",
        _ => "transient",
    }
}

/// Projects the latest observation plus the bounded history Iroh still retains.
pub(super) fn connection_view(
    latest: &ma2a_net::ConnectionObservation,
    connections: &crate::RuntimeConnections,
) -> Result<crate::api::ConnectionView, RuntimeError> {
    let state = if latest.last_error().is_some() {
        "failed"
    } else if latest.last_success_at_ms().is_some() {
        "connected"
    } else {
        "connecting"
    };
    let observations = connections
        .observations(latest.remote_endpoint_id())
        .iter()
        .map(|observation| {
            ConnectionObservationView::new(
                observation.observed_at_ms(),
                observation_path_label(observation.path_state()),
                observation.rtt_ms(),
                observation
                    .last_error()
                    .map_or("none", |error| error_label(error.class())),
            )
        })
        .collect();
    crate::api::ConnectionView::new(
        latest.remote_endpoint_id(),
        state,
        path_label(latest.path_state()),
        latest.rtt_ms(),
        observations,
    )
    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
}

pub(super) fn detail_view(
    detail: &ma2a_store::SpaceDetails,
) -> Result<crate::api::SpaceDetailsView, RuntimeError> {
    let space = space_view(&detail.space)?;
    let members = detail
        .members
        .iter()
        .map(|member| {
            SpaceMemberView::new(
                member.endpoint_id(),
                member.label(),
                member.echo(),
                member.relay_provider(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    crate::api::SpaceDetailsView::new(detail.revision, space, members)
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
}
