//! `proxy.open`: a loopback listener that relays each accepted connection to one remote
//! destination, for as long as the calling task lives.
//!
//! A listener lives in this daemon only: it ends with its task, and also when the daemon
//! stops, after which a re-executed call is answered from its recorded result.

use ma2a_agent::{
    protocol::ToolError,
    tools::{ProxyListening, ProxyOpen},
};
use ma2a_core::{
    RequestId,
    service::{OpenBody, ProxyHost, ProxyRequest},
};
use ma2a_services::client;
use tokio::net::TcpListener;

use super::{Call, Outcome, ok};

/// Idle timeout of each relayed connection: ten minutes.
const IDLE_TIMEOUT_MS: u64 = 600_000;
/// Pause after a failed accept before trying again.
const ACCEPT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);

impl Call<'_> {
    pub(super) async fn proxy_open(&self, args: ProxyOpen) -> Outcome {
        let host = ProxyHost::new(&args.host)
            .map_err(|_| ToolError::new("invalid_tool", "invalid host"))?;
        // Refused before anything listens, so an invalid destination is never answered.
        let request = ProxyRequest::new(host, args.port, IDLE_TIMEOUT_MS)
            .map_err(|_| ToolError::new("invalid_tool", "invalid port"))?;
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
        let manager = self.manager.clone();
        let (endpoint, task) = (args.endpoint, self.task);
        let workspace = self.workspace.clone();
        let agent = self.agent.clone();
        // Each accepted connection is opened on behalf of the `proxy.open` call.
        let opened_by = self.id.to_owned();
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
                let Ok(request_id) = RequestId::random() else {
                    continue;
                };
                let call = Call {
                    manager: &manager,
                    task,
                    id: &opened_by,
                    agent: agent.clone(),
                    workspace: workspace.clone(),
                    request_id,
                };
                let Ok((mut reader, mut writer)) =
                    call.open(&endpoint, OpenBody::Proxy(request.clone())).await
                else {
                    continue;
                };
                let stop = token.clone();
                manager.0.tasks.spawn(async move {
                    let (read, write) = socket.into_split();
                    tokio::select! {
                        _relayed = client::proxy::run((&mut reader, &mut writer), (read, write)) => {}
                        () = stop.cancelled() => {}
                    }
                });
            }
        });
        ok(&ProxyListening {
            listen: listen.to_string(),
        })
    }
}
