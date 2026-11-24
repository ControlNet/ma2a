use std::{io, time::Duration};

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};

use super::ConnectionManager;

type TestResult<T = ()> = Result<T, io::Error>;

const HOLD_ALPN: &[u8] = b"ma2a/test-hold";

/// Keeps every accepted connection open until the dialer closes it.
#[derive(Clone, Debug)]
struct Hold;

impl ProtocolHandler for Hold {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        connection.closed().await;
        Ok(())
    }
}

async fn test_endpoint(provenance: &'static str) -> TestResult<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance(provenance))
        .bind()
        .await
        .map_err(io::Error::other)
}

fn active(manager: &ConnectionManager) -> usize {
    manager
        .0
        .active
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len()
}

#[tokio::test]
async fn a_connection_its_users_dropped_frees_its_active_place() -> TestResult {
    // Given: a peer that never closes a connection itself.
    let router = Router::builder(test_endpoint("ma2a_manager_hold_server").await?)
        .accept(HOLD_ALPN, Hold)
        .spawn();
    let manager = ConnectionManager::new(test_endpoint("ma2a_manager_hold_client").await?);

    // When: three connections are dialed and dropped by their only users.
    for _ in 0..3 {
        let connection = manager
            .connect_addr(router.endpoint().addr(), HOLD_ALPN)
            .await
            .map_err(io::Error::other)?;
        drop(connection);
    }

    // Then: each closes, and the manager no longer counts it.
    let freed = tokio::time::timeout(Duration::from_secs(5), async {
        while active(&manager) > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        freed.is_ok(),
        "{} connections still counted",
        active(&manager)
    );
    manager.shutdown().await;
    router.shutdown().await.map_err(io::Error::other)?;
    Ok(())
}
