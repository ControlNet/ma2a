use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use iroh::endpoint::Connection;
use ma2a_core::{EndpointId, ServiceKind, service::ServiceError};

use crate::ConnectionManager;

use super::{ServiceStream, service_alpn};

/// Most cached outbound service connections; evicted entries close with their last stream.
const MAX_CACHED_CONNECTIONS: usize = 32;

/// Cloneable outbound client that opens one stream per service operation.
///
/// Connections are reused per exact target and service. Opening a stream writes nothing, so a
/// stale cached connection is replaced by exactly one fresh dial without any side effect.
#[derive(Clone, Debug)]
pub struct ServiceClient {
    connections: ConnectionManager,
    cache: Arc<Mutex<BTreeMap<(EndpointId, u8), Connection>>>,
}

impl ServiceClient {
    pub(crate) fn new(connections: ConnectionManager) -> Self {
        Self {
            connections,
            cache: Arc::default(),
        }
    }

    /// Opens one bidirectional stream to the exact target Endpoint.
    ///
    /// # Errors
    /// Returns [`ServiceError::Unavailable`] when the target cannot be reached or does not
    /// negotiate the service ALPN, and [`ServiceError::InvalidInput`] for Echo or a malformed
    /// target identity.
    pub async fn open(
        &self,
        target: EndpointId,
        kind: ServiceKind,
    ) -> Result<ServiceStream, ServiceError> {
        let alpn = service_alpn(kind).ok_or(ServiceError::InvalidInput)?;
        let key = (target, kind.code());
        if let Some(connection) = self.cached(key) {
            if let Ok((send, receive)) = connection.open_bi().await {
                return Ok(ServiceStream::outbound(connection, send, receive));
            }
            self.forget(key, &connection);
        }
        let address = target
            .to_public_key()
            .map_err(|_| ServiceError::InvalidInput)?
            .into();
        let connection = self
            .connections
            .connect_addr(address, alpn)
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        let (send, receive) = connection
            .open_bi()
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        self.remember(key, connection.clone());
        Ok(ServiceStream::outbound(connection, send, receive))
    }

    fn cached(&self, key: (EndpointId, u8)) -> Option<Connection> {
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache
            .get(&key)
            .filter(|connection| connection.close_reason().is_none())
            .cloned()
    }

    fn forget(&self, key: (EndpointId, u8), stale: &Connection) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache
            .get(&key)
            .is_some_and(|connection| connection.stable_id() == stale.stable_id())
        {
            cache.remove(&key);
        }
    }

    fn remember(&self, key: (EndpointId, u8), connection: Connection) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|_, cached| cached.close_reason().is_none());
        if cache.len() >= MAX_CACHED_CONNECTIONS && !cache.contains_key(&key) {
            cache.pop_first();
        }
        cache.insert(key, connection);
    }
}
