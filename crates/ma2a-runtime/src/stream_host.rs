//! Inbound built-in streaming services: authorization, dispatch, and bounded lifetime.
//!
//! This task is separate from the Runtime actor: service streams can run for hours and never
//! block actor commands. Every stream is authorized against durable store state twice — once
//! before any byte is read, and again for the exact decoded request.

mod inbound;
pub(crate) mod journal;
pub(crate) mod operations;
pub(crate) mod reauthorize;

use std::time::Duration;

use ma2a_core::EndpointId;
use ma2a_net::ServiceCall;
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub(crate) use operations::OperationsRegistry;
pub(crate) use reauthorize::Authorizations;

use crate::store::StoreClient;

/// Grace period for running streams to observe shutdown before they are aborted.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Everything one inbound stream needs besides the stream itself.
#[derive(Clone, Debug)]
pub(crate) struct HostContext {
    pub(crate) local: EndpointId,
    pub(crate) store: StoreClient,
    pub(crate) shutdown: CancellationToken,
    pub(crate) operations: OperationsRegistry,
    pub(crate) authorizations: Authorizations,
}

/// Serves admitted inbound streams, keeping each authorized, until the Runtime stops.
pub(crate) async fn run(calls: mpsc::Receiver<ServiceCall>, context: HostContext) {
    tokio::join!(serve(calls, context.clone()), reauthorize::run(context));
}

async fn serve(mut calls: mpsc::Receiver<ServiceCall>, context: HostContext) {
    let mut streams = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = context.shutdown.cancelled() => break,
            call = calls.recv() => match call {
                Some(call) => {
                    streams.spawn(inbound::serve(call, context.clone()));
                }
                None => break,
            },
            joined = streams.join_next(), if !streams.is_empty() => {
                let _completed = joined;
            }
        }
    }
    drop(calls);
    let drained = tokio::time::timeout(SHUTDOWN_GRACE, async {
        while streams.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        streams.shutdown().await;
    }
}
