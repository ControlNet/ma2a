use std::collections::BTreeSet;

use ma2a_net::EndpointSecret;
use ma2a_store::{EndpointRecord, KeyKind, KeyMaterial, KeyReference, StoreConfig, StoreError};

use super::StoreBackend;
use crate::error::{RuntimeError, RuntimeErrorKind};

const ENDPOINT_KEY_REFERENCE: &str = "endpoint-identity-v1";

pub(crate) struct Identity {
    pub(crate) secret: EndpointSecret,
    pub(crate) endpoint_id: ma2a_core::EndpointId,
    pub(crate) memberships: BTreeSet<ma2a_core::SpaceId>,
    pub(crate) bind_port: Option<u16>,
}

impl StoreBackend {
    pub(super) fn record_observation(
        &mut self,
        observation: &ma2a_store::EndpointObservationUpdate,
        if_changed: bool,
    ) -> Result<u64, RuntimeError> {
        let result = if if_changed {
            self.repository
                .record_endpoint_observation_if_changed(observation)
        } else {
            self.repository.record_endpoint_observation(observation)
        };
        result.map_err(Into::into)
    }

    pub(crate) fn open(config: &StoreConfig) -> Result<Self, RuntimeError> {
        Ok(Self {
            repository: ma2a_store::Repository::open(config)?,
            key_store: ma2a_store::KeyStore::open(config.state_dir())?,
            pending_relay_publication: None,
        })
    }

    pub(super) fn initialize(&mut self) -> Result<Identity, RuntimeError> {
        let reference = KeyReference::parse(ENDPOINT_KEY_REFERENCE)?;
        let record = self.repository.endpoint()?;
        if let Some(endpoint) = &record
            && endpoint.key_reference() != &reference
        {
            return Err(RuntimeError::new(RuntimeErrorKind::KeyReferenceMismatch));
        }
        let secret = match self.key_store.read(KeyKind::Endpoint, &reference) {
            Ok(protected) => EndpointSecret::parse(protected.as_ref())?,
            Err(StoreError::MissingProtectedKey { .. }) if record.is_none() => {
                let secret = EndpointSecret::generate();
                let bytes = secret.protected_bytes();
                self.key_store.write(KeyMaterial::new(
                    KeyKind::Endpoint,
                    &reference,
                    bytes.as_ref(),
                ))?;
                secret
            }
            Err(error) => return Err(error.into()),
        };
        let endpoint_id = secret.endpoint_id();
        if let Some(endpoint) = record {
            if endpoint.endpoint_id() != endpoint_id {
                return Err(RuntimeError::new(RuntimeErrorKind::IdentityMismatch));
            }
        } else {
            self.repository
                .set_endpoint(&EndpointRecord::new(endpoint_id, reference))?;
        }
        let memberships = self.repository.memberships_for(endpoint_id)?;
        let bind_port = self.repository.endpoint_bind_port()?;
        Ok(Identity {
            secret,
            endpoint_id,
            memberships,
            bind_port,
        })
    }
}
