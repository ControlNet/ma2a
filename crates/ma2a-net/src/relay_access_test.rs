use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh_base::{EndpointId as IrohEndpointId, SecretKey};
use iroh_relay::server::{Access, AccessControl as _, ClientRequest, ConnectionId};
use tokio::sync::Notify;

use super::{
    DisconnectClients, PrivateRelayAccess,
    pending::{
        PENDING_DISCONNECT_RETRY_INTERVAL, PendingDisconnects, WorkerGates,
        apply_space_authorizations,
    },
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
type DisconnectLog = Arc<Mutex<Vec<(IrohEndpointId, Option<ConnectionId>)>>>;

#[derive(Clone, Debug, Default)]
struct ControlledClients {
    registered: Arc<Mutex<HashSet<(IrohEndpointId, ConnectionId)>>>,
    disconnected: DisconnectLog,
    notify: Arc<Notify>,
}

impl ControlledClients {
    fn register(&self, endpoint_id: IrohEndpointId, connection_id: ConnectionId) {
        if let Ok(mut registered) = self.registered.lock() {
            registered.insert((endpoint_id, connection_id));
        }
    }

    fn is_registered(&self, endpoint_id: IrohEndpointId, connection_id: ConnectionId) -> bool {
        self.registered
            .lock()
            .is_ok_and(|registered| registered.contains(&(endpoint_id, connection_id)))
    }

    fn disconnect_calls(&self) -> Vec<(IrohEndpointId, Option<ConnectionId>)> {
        self.disconnected
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    async fn wait_for_disconnect(
        &self,
        endpoint_id: IrohEndpointId,
        connection_id: ConnectionId,
    ) -> Result<(), &'static str> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if !self.is_registered(endpoint_id, connection_id)
                && self
                    .disconnect_calls()
                    .contains(&(endpoint_id, Some(connection_id)))
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("timed out waiting for pending disconnect");
            }
            tokio::select! {
                () = self.notify.notified() => {}
                () = tokio::time::sleep(PENDING_DISCONNECT_RETRY_INTERVAL) => {}
            }
        }
    }
}

impl DisconnectClients for ControlledClients {
    fn disconnect(&self, endpoint_id: IrohEndpointId, connection_id: Option<ConnectionId>) -> bool {
        let found = connection_id.map_or_else(
            || {
                self.registered.lock().is_ok_and(|mut registered| {
                    let before = registered.len();
                    registered.retain(|(registered_id, _)| *registered_id != endpoint_id);
                    registered.len() != before
                })
            },
            |id| {
                self.registered
                    .lock()
                    .is_ok_and(|mut registered| registered.remove(&(endpoint_id, id)))
            },
        );
        if found {
            if let Ok(mut calls) = self.disconnected.lock() {
                calls.push((endpoint_id, connection_id));
            }
            self.notify.notify_waiters();
        }
        found
    }
}

fn test_authorization(
    member: &SecretKey,
    marker: u8,
) -> Result<ma2a_core::SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
    let authority = ma2a_core::SpaceAuthoritySecret::from_bytes([marker.wrapping_add(0x10); 32]);
    let member = ma2a_core::SpaceMemberV1::new(
        member.public().into(),
        format!("member-{marker:02x}"),
        ma2a_core::MemberCapabilities::new(true, true),
    )?;
    let genesis = ma2a_core::SpaceGenesisV1::new(
        ma2a_core::SpaceGenesisIdentity::new(
            [marker.wrapping_add(1); 32],
            1,
            authority.public_key(),
        )?,
        ma2a_core::SpaceGenesisOwner::new(member, ma2a_core::SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    Ok(ma2a_core::SpaceAuthorizationView::from_chain(
        &ma2a_core::SpaceChain::from_genesis(genesis)?,
    ))
}

fn test_request(
    member: &SecretKey,
) -> Result<ClientRequest, Box<dyn std::error::Error + Send + Sync>> {
    let (parts, ()) = http::Request::builder()
        .uri("http://localhost/relay")
        .body(())?
        .into_parts();
    Ok(ClientRequest::new(
        member.public(),
        iroh_relay::http::ProtocolVersion::V2,
        parts,
    ))
}

#[tokio::test(start_paused = true)]
async fn readmission_during_worker_iteration_leaves_eligible_connection_open() -> TestResult {
    let member = SecretKey::from_bytes(&[0x91; 32]);
    let authorization = test_authorization(&member, 0x91)?;
    let access = PrivateRelayAccess::new(member.public().into(), &[authorization.space_id()]);
    let clients = ControlledClients::default();
    let pending = PendingDisconnects::spawn_with(access.clone(), clients.clone());
    access.replace_from_spaces(std::slice::from_ref(&authorization));
    let request = test_request(&member)?;
    let connection_id = request.connection_id();
    assert_eq!(access.on_connect(&request).await, Access::Allow);

    let (entered_rx, release_tx) = WorkerGates::arm(&pending.gates().after_batch);
    apply_space_authorizations((&access, &clients, &pending), &[]);
    tokio::time::timeout(Duration::from_secs(2), entered_rx).await??;

    // Re-admit and register the connection before the worker checks admission.
    apply_space_authorizations((&access, &clients, &pending), &[authorization]);
    clients.register(member.public(), connection_id);
    let _released = release_tx.send(());

    tokio::time::advance(PENDING_DISCONNECT_RETRY_INTERVAL * 3).await;
    tokio::task::yield_now().await;

    assert!(
        clients.is_registered(member.public(), connection_id),
        "superseded withdrawal disconnected a re-admitted connection"
    );
    assert!(clients.disconnect_calls().is_empty());

    pending.shutdown().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn newer_withdrawal_before_worker_removal_keeps_pending_retry() -> TestResult {
    let member = SecretKey::from_bytes(&[0x92; 32]);
    let authorization = test_authorization(&member, 0x92)?;
    let access = PrivateRelayAccess::new(member.public().into(), &[authorization.space_id()]);
    let clients = ControlledClients::default();
    let pending = PendingDisconnects::spawn_with(access.clone(), clients.clone());
    access.replace_from_spaces(std::slice::from_ref(&authorization));
    let request = test_request(&member)?;
    let connection_id = request.connection_id();
    assert_eq!(access.on_connect(&request).await, Access::Allow);

    let (batch_entered_rx, batch_release_tx) = WorkerGates::arm(&pending.gates().after_batch);
    apply_space_authorizations((&access, &clients, &pending), &[]);
    tokio::time::timeout(Duration::from_secs(2), batch_entered_rx).await??;

    // Re-admit so the worker marks this connection complete, then pause before removal.
    apply_space_authorizations((&access, &clients, &pending), &[authorization]);
    let (remove_entered_rx, remove_release_tx) = WorkerGates::arm(&pending.gates().before_remove);
    let _released = batch_release_tx.send(());
    tokio::time::timeout(Duration::from_secs(2), remove_entered_rx).await??;

    // A newer withdrawal requeues the same connection before the stale completion removes it.
    apply_space_authorizations((&access, &clients, &pending), &[]);
    clients.register(member.public(), connection_id);
    let _released = remove_release_tx.send(());

    clients
        .wait_for_disconnect(member.public(), connection_id)
        .await?;
    assert!(!clients.is_registered(member.public(), connection_id));

    pending.shutdown().await;
    Ok(())
}
