//! Endpoint-centric single-Space authorization for built-in remote services.

use crate::{
    AuthorizationDenied, AuthorizationEndpoints, AuthorizationPermit, Capability,
    SpaceAuthorizationView,
};

use super::{ProxyHost, RootAccess, RootName, SpaceExposureView};

/// The complete access one remote service request needs from one Space.
#[expect(
    clippy::exhaustive_enums,
    reason = "every service access must be evaluated by one exhaustive rule"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceAccess {
    /// Pre-read admission to the Files or Transfer families: some root may be readable.
    FilesAdmission,
    /// Read, stat, list, or download below one exported root.
    FilesRead(RootName),
    /// Mutate or upload below one read-write exported root.
    FilesWrite(RootName),
    /// Run one-shot Exec.
    Exec,
    /// Open one interactive Shell.
    Shell,
    /// Pre-read admission to the Proxy family: some destination may be allowed.
    ProxyAdmission,
    /// Connect to one exact destination.
    ProxyConnect(ProxyHost, u16),
}

/// The authenticated pair, the requested access, and this Runtime's per-Space exposure.
#[derive(Clone, Copy, Debug)]
pub struct ServiceAuthorizationRequest<'a> {
    endpoints: AuthorizationEndpoints,
    access: &'a ServiceAccess,
    exposures: &'a [SpaceExposureView],
}

impl<'a> ServiceAuthorizationRequest<'a> {
    /// Creates a request without any caller-selected Space.
    pub const fn new(
        endpoints: AuthorizationEndpoints,
        access: &'a ServiceAccess,
        exposures: &'a [SpaceExposureView],
    ) -> Self {
        Self {
            endpoints,
            access,
            exposures,
        }
    }
}

/// Returns the first Space whose signed grants and local exposure completely allow the access.
///
/// Membership of both Endpoints, the caller's signed capability, and the target's local
/// exposure are all evaluated against the same Space. A grant in one Space and an exposure in
/// another never combine.
///
/// # Errors
/// Returns the uniform [`AuthorizationDenied`] for every failure.
pub fn authorize_service(
    request: &ServiceAuthorizationRequest<'_>,
    spaces: &[SpaceAuthorizationView],
) -> Result<AuthorizationPermit, AuthorizationDenied> {
    spaces
        .iter()
        .find(|space| permits(request, space))
        .map(AuthorizationPermit::for_space)
        .ok_or(AuthorizationDenied::ACCESS_DENIED)
}

fn permits(request: &ServiceAuthorizationRequest<'_>, space: &SpaceAuthorizationView) -> bool {
    let (caller, target) = request.endpoints.parts();
    if !space.contains_member(caller) || !space.contains_member(target) {
        return false;
    }
    let Some(exposure) = request
        .exposures
        .iter()
        .find(|exposure| exposure.space_id() == Some(space.space_id()))
    else {
        return false;
    };
    let granted = |capability| space.allows(caller, capability);
    match request.access {
        ServiceAccess::FilesAdmission => {
            (granted(Capability::FILES_READ) || granted(Capability::FILES_WRITE))
                && !exposure.roots().is_empty()
        }
        ServiceAccess::FilesRead(root) => {
            granted(Capability::FILES_READ) && exposure.root(root).is_some()
        }
        ServiceAccess::FilesWrite(root) => {
            granted(Capability::FILES_WRITE)
                && exposure
                    .root(root)
                    .is_some_and(|root| root.access() == RootAccess::ReadWrite)
        }
        ServiceAccess::Exec => granted(Capability::EXEC) && exposure.exec(),
        ServiceAccess::Shell => granted(Capability::SHELL) && exposure.shell(),
        ServiceAccess::ProxyAdmission => {
            granted(Capability::PROXY) && !exposure.proxy_rules().is_empty()
        }
        ServiceAccess::ProxyConnect(host, port) => {
            granted(Capability::PROXY)
                && exposure
                    .proxy_rules()
                    .iter()
                    .any(|rule| rule.allows(host, *port))
        }
    }
}
