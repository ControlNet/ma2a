use ma2a_core::{
    AuthorizationDenied, AuthorizationPermit, AuthorizationRequest, SpaceAuthorizationView,
    authorize_endpoint,
};

/// Performs the single normal remote authorization decision before service-body parsing.
///
/// The caller must provide current verified views for every decision; this function owns no cache.
///
/// # Errors
/// Returns the uniform non-leaking denial when no independently evaluated Space completely allows.
pub fn authorize_remote(
    request: &AuthorizationRequest,
    current_spaces: &[SpaceAuthorizationView],
) -> Result<AuthorizationPermit, AuthorizationDenied> {
    authorize_endpoint(request, current_spaces)
}
