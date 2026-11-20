//! Serving one registered service: its link, its calls, and reconnection.

use std::{collections::HashMap, fmt, future::Future, pin::Pin, sync::Arc, time::Duration};

use base64::Engine as _;
use ma2a_core::{
    EndpointId, RequestId,
    service::{OperationName, ServiceError, ServiceName},
};
use ma2a_runtime::ipc::{IpcPaths, LinkReader, LocalApiClient, LocalStream};
use serde_json::{Map, Value, json};
use tokio::{sync::mpsc, task::JoinSet};

use crate::{Call, Ended, Error, wire};

/// First wait before attaching again after the link or the Runtime is lost.
const FIRST_BACKOFF: Duration = Duration::from_millis(250);
/// Longest wait between two attach attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(5);
/// How long running calls may take to end after the link ends.
const DRAIN: Duration = Duration::from_secs(10);

type Handler =
    Arc<dyn Fn(Call) -> Pin<Box<dyn Future<Output = Result<Ended, Error>> + Send>> + Send + Sync>;
type Handlers = Arc<HashMap<(u16, String), Handler>>;

/// How a link ended.
enum LinkEnd {
    /// The operator stopped the service.
    Stopped,
    /// The link or the Runtime was lost; attach again.
    Lost,
}

/// One service's handlers, served through the local Runtime.
#[derive(Default)]
pub struct ServiceHost {
    handlers: HashMap<(u16, String), Handler>,
}

impl fmt::Debug for ServiceHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut operations: Vec<_> = self.handlers.keys().collect();
        operations.sort();
        formatter
            .debug_struct("ServiceHost")
            .field("operations", &operations)
            .finish()
    }
}

impl ServiceHost {
    /// Creates a host without handlers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Serves `operation` of `version` with `handler`; a later handler for the same pair
    /// replaces the earlier one.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] for an invalid operation name or version 0.
    pub fn handle<F, Fut>(
        mut self,
        (version, operation): (u16, &str),
        handler: F,
    ) -> Result<Self, Error>
    where
        F: Fn(Call) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Ended, Error>> + Send + 'static,
    {
        let operation =
            OperationName::new(operation).map_err(|_| Error::Protocol("invalid operation name"))?;
        if version == 0 {
            return Err(Error::Protocol("versions start at 1"));
        }
        let handler: Handler = Arc::new(move |call| Box::pin(handler(call)));
        self.handlers
            .insert((version, operation.as_str().to_owned()), handler);
        Ok(self)
    }

    /// Attaches to the Runtime at `paths` as `service` and serves calls until the operator
    /// stops the service.
    ///
    /// A lost link or Runtime is attached again with backoff (250 ms doubling to 5 s), and so
    /// is a service that is not started yet: a host may start before its Runtime or before the
    /// operator starts it.
    ///
    /// # Errors
    /// Returns [`Error::MissingHandler`] when the registered manifest lists an operation this
    /// host cannot serve, [`Error::Refused`] when the service is not registered, and
    /// [`Error::Ipc`] when the Runtime speaks another local API version or its IPC path is
    /// not this user's.
    pub async fn serve(self, paths: IpcPaths, service: &ServiceName) -> Result<(), Error> {
        let client = LocalApiClient::new(paths);
        let handlers: Handlers = Arc::new(self.handlers);
        let mut backoff = FIRST_BACKOFF;
        loop {
            match attach(&client, service).await {
                Ok((manifest, stream)) => {
                    covered(&manifest, &handlers)?;
                    backoff = FIRST_BACKOFF;
                    if matches!(
                        run_link((&client, service), &handlers, stream).await,
                        LinkEnd::Stopped
                    ) {
                        return Ok(());
                    }
                }
                Err(error) if !error.retries() => return Err(error),
                Err(_) => {}
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }
}

/// Attaches the link; the registered manifest and the upgraded connection.
async fn attach(
    client: &LocalApiClient,
    service: &ServiceName,
) -> Result<(Map<String, Value>, LocalStream), Error> {
    let mut fields = Map::new();
    fields.insert("operation".to_owned(), json!("service_attach"));
    fields.insert("service".to_owned(), json!(service.as_str()));
    let (response, stream) = client.open_hosted(&wire::command(fields)?).await?;
    let mut payload = wire::payload(&response, "service_attached")?;
    let stream = stream.ok_or(Error::Protocol("the Runtime attached without a link"))?;
    match payload.remove("manifest") {
        Some(Value::Object(manifest)) => Ok((manifest, stream)),
        _ => Err(Error::Protocol("the Runtime attached without a manifest")),
    }
}

/// Refuses to serve a manifest listing an operation without a handler.
fn covered(manifest: &Map<String, Value>, handlers: &Handlers) -> Result<(), Error> {
    let versions = manifest
        .get("versions")
        .and_then(Value::as_array)
        .ok_or(Error::Protocol("the manifest lists no versions"))?;
    for version in versions {
        let number = version
            .get("version")
            .and_then(Value::as_u64)
            .and_then(|number| u16::try_from(number).ok())
            .ok_or(Error::Protocol("the manifest has a malformed version"))?;
        let operations = version
            .get("operations")
            .and_then(Value::as_array)
            .ok_or(Error::Protocol("the manifest has a malformed version"))?;
        for operation in operations {
            let name = operation
                .get("name")
                .and_then(Value::as_str)
                .ok_or(Error::Protocol("the manifest has a malformed operation"))?;
            if !handlers.contains_key(&(number, name.to_owned())) {
                return Err(Error::MissingHandler {
                    version: number,
                    operation: name.to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// One message the Runtime sends on the link.
enum Notice {
    Call([u8; 16]),
    Stop,
}

fn notice(message: &[u8]) -> Option<Notice> {
    let message: Value = serde_json::from_slice(message).ok()?;
    match message.get("t").and_then(Value::as_str)? {
        "call" => wire::unhex(message.get("call")?.as_str()?)
            .ok()
            .map(Notice::Call),
        "stop" => Some(Notice::Stop),
        _ => None,
    }
}

/// Reads link messages in a task of their own: a read cut short would split a frame.
fn read_notices(mut reader: LinkReader) -> mpsc::Receiver<Option<Notice>> {
    let (sender, notices) = mpsc::channel(16);
    tokio::spawn(async move {
        loop {
            let message = reader.next().await.ok().as_deref().and_then(notice);
            let ended = message.is_none() || matches!(message, Some(Notice::Stop));
            if sender.send(message).await.is_err() || ended {
                return;
            }
        }
    });
    notices
}

/// Reports ready, then accepts every call the Runtime announces until the link ends.
async fn run_link(
    (client, service): (&LocalApiClient, &ServiceName),
    handlers: &Handlers,
    stream: LocalStream,
) -> LinkEnd {
    let (reader, mut reports) = stream.into_link();
    if reports.send(br#"{"t":"ready"}"#).await.is_err() {
        return LinkEnd::Lost;
    }
    let mut notices = read_notices(reader);
    let mut calls = JoinSet::new();
    let end = loop {
        tokio::select! {
            notice = notices.recv() => match notice.flatten() {
                Some(Notice::Call(call)) => {
                    let accepted = accept((client.clone(), service.clone()), call, Arc::clone(handlers));
                    calls.spawn(accepted);
                }
                Some(Notice::Stop) => break LinkEnd::Stopped,
                None => break LinkEnd::Lost,
            },
            Some(_) = calls.join_next(), if !calls.is_empty() => {}
        }
    };
    // Running calls end on their own: a stop reaches them as `cancel`, and a lost Runtime
    // closes their connections.
    let _drained =
        tokio::time::timeout(DRAIN, async { while calls.join_next().await.is_some() {} }).await;
    drop(reports);
    end
}

/// Accepts one announced call and runs its handler; a withdrawn call is skipped.
async fn accept(
    (client, service): (LocalApiClient, ServiceName),
    call: [u8; 16],
    handlers: Handlers,
) {
    let Ok(Some(call)) = open_call(&client, &service, call).await else {
        return;
    };
    let handler = handlers
        .get(&(call.version(), call.operation().as_str().to_owned()))
        .cloned();
    let _ended = match handler {
        Some(handler) => handler(call).await,
        None => call.reject(ServiceError::InvalidInput).await,
    };
}

async fn open_call(
    client: &LocalApiClient,
    service: &ServiceName,
    call: [u8; 16],
) -> Result<Option<Call>, Error> {
    let mut fields = Map::new();
    fields.insert("operation".to_owned(), json!("service_accept"));
    fields.insert("service".to_owned(), json!(service.as_str()));
    fields.insert("call_id".to_owned(), json!(wire::hex(&call)));
    let (response, stream) = client.open_hosted(&wire::command(fields)?).await?;
    let Ok(details) = wire::payload(&response, "service_call") else {
        return Ok(None);
    };
    let stream = stream.ok_or(Error::Protocol(
        "the Runtime handed a call without a stream",
    ))?;
    let text = |field: &str| {
        details
            .get(field)
            .and_then(Value::as_str)
            .ok_or(Error::Protocol("the Runtime handed a malformed call"))
    };
    let caller = EndpointId::try_from(wire::unhex::<32>(text("caller_endpoint_id")?)?.as_slice())
        .map_err(|_| Error::Protocol("the Runtime handed a malformed call"))?;
    let request_id = RequestId::try_from(wire::unhex::<16>(text("request_id")?)?.as_slice())
        .map_err(|_| Error::Protocol("the Runtime handed a malformed call"))?;
    let version = details
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|version| u16::try_from(version).ok())
        .ok_or(Error::Protocol("the Runtime handed a malformed call"))?;
    let operation = OperationName::new(text("operation")?)
        .map_err(|_| Error::Protocol("the Runtime handed a malformed call"))?;
    let body = base64::engine::general_purpose::STANDARD
        .decode(text("body")?)
        .map_err(|_| Error::Protocol("the Runtime handed a malformed call"))?;
    Ok(Some(Call::new(
        (caller, request_id),
        (version, operation, body),
        stream,
    )))
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
