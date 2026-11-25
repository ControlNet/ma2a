//! Gives a fresh state directory its Endpoint identity without starting a Runtime.
//!
//! Shared by the integration tests of `ma2a-runtime` and `ma2a-app`, and by the
//! `ma2a-runtime` unit tests, through `#[path]`.

use std::path::Path;

use ma2a_core::EndpointId;
use ma2a_net::EndpointSecret;
use ma2a_store::{KeyKind, KeyMaterial, KeyReference, KeyStore};

/// Writes a new Endpoint key into `state_dir` and returns its identifier.
///
/// A test that writes to a Store before its Runtime first starts needs the Endpoint's
/// identity first. Minting it with a throwaway Runtime instead would also save that Runtime's
/// port, and another test process could take the port before the real start binds it again.
pub(crate) fn seed_endpoint(
    state_dir: &Path,
) -> Result<EndpointId, Box<dyn std::error::Error + Send + Sync>> {
    let secret = EndpointSecret::generate();
    let protected = secret.protected_bytes();
    KeyStore::open(state_dir)?.write(KeyMaterial::new(
        KeyKind::Endpoint,
        &KeyReference::parse("endpoint-identity-v1")?,
        &protected[..],
    ))?;
    Ok(secret.endpoint_id())
}
