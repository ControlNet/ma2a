pub(crate) mod echo;

use ma2a_core::ServiceKind;

/// Dispatches one decoded bounded request-response call.
///
/// Only Echo uses the Phase 1 request-response envelope; every streaming service is
/// served by its own stream host and can never reach this seam.
pub(crate) fn dispatch(
    kind: ServiceKind,
    input: &echo::EchoDispatchInput,
) -> echo::EchoDispatchOutcome {
    match kind {
        ServiceKind::Echo => echo::dispatch(input),
        ServiceKind::Files
        | ServiceKind::Transfer
        | ServiceKind::Exec
        | ServiceKind::Shell
        | ServiceKind::Proxy
        | ServiceKind::Process => echo::EchoDispatchOutcome {
            response: Err(ma2a_core::EchoError::Unavailable),
            audit: None,
        },
    }
}
