//! The one link of a started service, and the hand-over of each call it accepts.

use ma2a_core::service::{ServiceError, ServiceManifest, ServiceName};
use tokio::sync::{mpsc, oneshot};

use super::{Accepted, CallDetails, HostedServices, Link, NOTICE_BUFFER, Notice};

/// The Runtime's end of one attached link.
#[derive(Debug)]
pub(crate) struct LinkHandle {
    pub(crate) id: u64,
    pub(crate) notices: mpsc::Receiver<Notice>,
}

impl HostedServices {
    /// Attaches the link of a started service and returns its registered manifest.
    ///
    /// A service has at most one link: a second attach is a conflict, and an attach while
    /// stopped is unavailable.
    pub(crate) async fn attach(
        &self,
        name: &ServiceName,
    ) -> Result<(LinkHandle, ServiceManifest), ServiceError> {
        let _lifecycle = self.0.lifecycle.lock().await;
        let record = self
            .registration(name)
            .await?
            .ok_or(ServiceError::UnknownService)?;
        if !record.enabled {
            return Err(ServiceError::Unavailable);
        }
        let mut state = self.state();
        state.links = state.links.wrapping_add(1);
        let id = state.links;
        let live = state.services.entry(name.clone()).or_default();
        if live.link.is_some() {
            return Err(ServiceError::Conflict);
        }
        let (sender, notices) = mpsc::channel(NOTICE_BUFFER);
        live.link = Some(Link {
            id,
            ready: false,
            notices: sender,
        });
        drop(state);
        Ok((LinkHandle { id, notices }, record.manifest))
    }

    /// Records what the service reports on link `id`: whether it accepts calls now.
    pub(crate) fn report(&self, (name, id): (&ServiceName, u64), ready: bool) {
        let mut state = self.state();
        if let Some(link) = state
            .services
            .get_mut(name)
            .and_then(|live| live.link.as_mut())
            .filter(|link| link.id == id)
        {
            link.ready = ready;
        }
    }

    /// Ends link `id`: the service is unavailable at once, and every call it has not yet
    /// accepted is withdrawn with no possible effect.
    pub(crate) fn detach(&self, name: &ServiceName, id: u64) {
        let mut state = self.state();
        let Some(live) = state.services.get_mut(name) else {
            return;
        };
        if live.link.as_ref().is_some_and(|link| link.id == id) {
            live.link = None;
            state.pending.retain(|_, pending| pending.service != *name);
        }
    }

    /// Hands one pending call of `name` to the connection that accepts it.
    ///
    /// The call is no longer pending afterwards, so it can never also be withdrawn.
    pub(crate) fn accept(
        &self,
        name: &ServiceName,
        call: [u8; 16],
    ) -> Result<(CallDetails, oneshot::Sender<Accepted>), ServiceError> {
        let mut state = self.state();
        let belongs = state
            .pending
            .get(&call)
            .is_some_and(|pending| pending.service == *name);
        let pending = belongs.then(|| state.pending.remove(&call)).flatten();
        drop(state);
        let pending = pending.ok_or(ServiceError::NotFound)?;
        Ok((pending.details, pending.deliver))
    }

    /// Withdraws a call its service has not accepted; `false` once it was handed over.
    pub(super) fn withdraw(&self, call: [u8; 16]) -> bool {
        self.state().pending.remove(&call).is_some()
    }
}
