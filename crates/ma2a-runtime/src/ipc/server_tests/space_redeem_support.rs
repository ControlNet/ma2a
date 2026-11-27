//! Shared IPC enrollment test helpers for missing-address and exact-retry recovery tests.
use std::time::Duration;

use iroh_tickets::Ticket as _;
use ma2a_core::SignedInviteTicket;
use tokio::time::timeout;

use super::*;
use crate::{
    actor::maintenance::FaultPoint,
    error::{RuntimeError, RuntimeErrorKind},
};

pub(super) const PROMPT: Duration = Duration::from_secs(10);

pub(super) struct ServedRuntime {
    pub(super) state: TempState,
    pub(super) config: StoreConfig,
    pub(super) runtime: Runtime,
    server: LiveServer,
    pub(super) client: LocalApiClient,
}

impl ServedRuntime {
    pub(super) async fn start(name: &str) -> TestResult<Self> {
        let state = TempState::new_named(name)?;
        let config = StoreConfig::new(&state.0);
        let runtime = Runtime::start(config.clone()).await?;
        let (server, client) = serve_runtime(&state, &runtime).await?;
        Ok(Self {
            state,
            config,
            runtime,
            server,
            client,
        })
    }

    pub(super) async fn shutdown(self) -> TestResult {
        self.server.cancel().await?;
        timeout(PROMPT, self.runtime.shutdown()).await??;
        Ok(())
    }
}

pub(super) async fn serve_runtime(
    dir: &TempState,
    runtime: &Runtime,
) -> TestResult<(LiveServer, LocalApiClient)> {
    let paths = IpcPaths::new(&dir.0)?;
    let control = CurrentUserRuntime::open_at(
        &dir.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?,
    );
    Ok((server, LocalApiClient::new(paths)))
}

pub(super) async fn call_json(
    client: &LocalApiClient,
    body: serde_json::Value,
) -> TestResult<serde_json::Value> {
    let command = api::decode_command(&serde_json::to_vec(&body)?)?;
    let response = timeout(PROMPT, client.call(&command)).await??;
    Ok(serde_json::from_slice(&response)?)
}

pub(super) fn invitation_row(
    owner_config: &StoreConfig,
    invitation_id: &[u8; 16],
) -> TestResult<(u8, Option<Vec<u8>>)> {
    Ok(
        rusqlite::Connection::open(owner_config.database_path())?.query_row(
            "SELECT status, consumed_request_id FROM invitations WHERE invitation_id = ?1",
            [invitation_id.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?,
    )
}

pub(super) fn arm_address_fault(owner: &Runtime, count: usize) {
    owner.handle().clear_background_faults();
    for _ in 0..count {
        owner.handle().fail_background_once(
            FaultPoint::AddressObservation,
            RuntimeError::new(RuntimeErrorKind::AddressObservationUnavailable),
        );
    }
}

pub(super) async fn create_space_and_invite(
    owner: &ServedRuntime,
    name: &str,
    ids: (&str, &str),
) -> TestResult<(String, String, SignedInviteTicket)> {
    let created = call_json(
        &owner.client,
        serde_json::json!({
            "version": 1,
            "operation": "space_create",
            "request_id": ids.0,
            "name": name,
        }),
    )
    .await?;
    assert_eq!(created.get("error"), None, "{created}");
    let space_hex = created
        .pointer("/result/payload/space_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("space_id missing")?
        .to_owned();
    let (ticket_str, ticket) = invite_over_ipc(owner, &space_hex, (ids.1, "invite.ticket")).await?;
    Ok((space_hex, ticket_str, ticket))
}

pub(super) async fn invite_over_ipc(
    owner: &ServedRuntime,
    space_hex: &str,
    (request_id, ticket_name): (&str, &str),
) -> TestResult<(String, SignedInviteTicket)> {
    let ticket_path = owner.state.0.join(ticket_name);
    let invited = call_json(
        &owner.client,
        serde_json::json!({
            "version": 1,
            "operation": "space_invite",
            "request_id": request_id,
            "space_id": space_hex,
            "ttl_ms": 300_000,
            "output_path": ticket_path.to_str().ok_or("invalid ticket path")?,
        }),
    )
    .await?;
    assert_eq!(invited.get("error"), None, "{invited}");
    let ticket_str = std::fs::read_to_string(&ticket_path)?.trim().to_owned();
    let ticket = SignedInviteTicket::decode_string(&ticket_str)?;
    Ok((ticket_str, ticket))
}
