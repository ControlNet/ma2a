pub(crate) mod echo;

use ma2a_core::{EchoError, ServiceKind};

pub(crate) fn dispatch(
    kind: ServiceKind,
    input: &echo::EchoDispatchInput,
) -> Result<ma2a_net::EchoServiceResponse, EchoError> {
    match kind {
        ServiceKind::Echo => echo::dispatch(input),
    }
}
