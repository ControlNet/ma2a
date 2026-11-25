//! `proxy.open`: a loopback listener that relays each accepted connection to one remote
//! destination, for as long as the calling task lives.
//!
//! A listener lives in this daemon only: it ends with its task, and also when the daemon
//! stops, after which a re-executed call is answered from its recorded result.

use ma2a_agent::{
    protocol::ToolError,
    tools::{ProxyListening, ProxyOpen},
};
use std::{path::PathBuf, sync::Arc};

use ma2a_core::{
    RequestId, ServiceKind,
    service::{AgentId, AgentTaskId, OpenBody, ProxyHost, ProxyRequest},
};
use ma2a_services::client;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};

use super::{Call, Outcome, ok};
use crate::agents::AgentManager;

/// Idle timeout of each relayed connection: ten minutes.
const IDLE_TIMEOUT_MS: u64 = 600_000;
/// Most connections one listener opens or relays at once: the size of the Runtime's local
/// stream pool, as for `ma2a proxy forward`. Later ones wait in the listen backlog.
const MAX_RELAYS: usize = 64;
/// Pause after a failed accept before trying again.
const ACCEPT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);

impl Call<'_> {
    pub(super) async fn proxy_open(&self, args: ProxyOpen) -> Outcome {
        let host = ProxyHost::new(&args.host)
            .map_err(|_| ToolError::new("invalid_tool", "invalid host"))?;
        // Refused before anything listens, so an invalid destination is never answered.
        let request = ProxyRequest::new(host, args.port, IDLE_TIMEOUT_MS)
            .map_err(|_| ToolError::new("invalid_tool", "invalid port"))?;
        self.check_target(&args.endpoint, ServiceKind::Proxy)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| super::workspace::local(&error))?;
        let listen = listener
            .local_addr()
            .map_err(|error| super::workspace::local(&error))?;
        // Cancelled with its task, and with the daemon.
        let token = self.manager.0.shutdown.child_token();
        self.manager
            .coordination()
            .listeners
            .entry(self.task)
            .or_default()
            .push(token.clone());
        // Each accepted connection is opened on behalf of the `proxy.open` call.
        let shared = Arc::new((
            args.endpoint,
            self.id.to_owned(),
            self.agent.clone(),
            self.workspace.clone(),
            request,
        ));
        let (manager, task) = (self.manager.clone(), self.task);
        let permits = Arc::new(Semaphore::new(MAX_RELAYS));
        self.manager.0.tasks.spawn(async move {
            loop {
                let accepted = tokio::select! {
                    () = token.cancelled() => return,
                    accepted = listener.accept() => accepted,
                };
                let Ok((socket, _)) = accepted else {
                    // Running out of descriptors passes; retrying at once would only spin.
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                    continue;
                };
                // At the bound, the next connection waits here and later ones in the backlog.
                let acquired = Arc::clone(&permits).acquire_owned();
                let Ok(permit) = (tokio::select! {
                    () = token.cancelled() => return,
                    permit = acquired => permit,
                }) else {
                    return;
                };
                let (relaying, shared, stop) =
                    (manager.clone(), Arc::clone(&shared), token.clone());
                // Opening can stall; it never holds the next connection, and the task's end
                // still ends it.
                manager.0.tasks.spawn(async move {
                    tokio::select! {
                        () = relay((&relaying, task), &shared, socket) => {}
                        () = stop.cancelled() => {}
                    }
                    drop(permit);
                });
            }
        });
        ok(&ProxyListening {
            listen: listen.to_string(),
        })
    }
}

/// What every connection of one listener shares: target, call, Agent, workspace, request.
type Shared = (String, String, AgentId, PathBuf, ProxyRequest);

/// Opens one accepted connection's Proxy stream and relays it until either side ends.
async fn relay((manager, task): (&AgentManager, AgentTaskId), shared: &Shared, socket: TcpStream) {
    let (endpoint, opened_by, agent, workspace, request) = shared;
    let Ok(request_id) = RequestId::random() else {
        return;
    };
    let call = Call {
        manager,
        task,
        id: opened_by,
        agent: agent.clone(),
        workspace: workspace.clone(),
        request_id,
    };
    let Ok((mut reader, mut writer)) = call.open(endpoint, OpenBody::Proxy(request.clone())).await
    else {
        return;
    };
    let idle = std::time::Duration::from_millis(request.idle_timeout_ms());
    let (read, write) = socket.into_split();
    let _relayed = client::proxy::run(
        (&mut reader, &mut writer),
        (read, write),
        (idle, std::future::pending()),
    )
    .await;
}
