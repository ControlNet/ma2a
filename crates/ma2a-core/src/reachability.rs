/// Relay-backed reachability derived from desired candidates and Iroh observations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "runtime and API projections must handle every reachability state"
)]
pub enum RelayReachability {
    /// The Endpoint has no current valid Space memberships.
    NoActiveSpaces,
    /// Iroh has compatible candidates but no connected observed home yet.
    AwaitingIrohHome,
    /// Iroh reports a connected home relay from the supplied candidate map.
    IrohHomeConnected,
    /// Active Spaces have neither a common private relay nor public fallback.
    DegradedNoCommonHome,
}
