pub(crate) mod echo;

use ma2a_core::ServiceKind;

pub(crate) fn dispatch(
    kind: ServiceKind,
    input: &echo::EchoDispatchInput,
) -> echo::EchoDispatchOutcome {
    match kind {
        ServiceKind::Echo => echo::dispatch(input),
    }
}
