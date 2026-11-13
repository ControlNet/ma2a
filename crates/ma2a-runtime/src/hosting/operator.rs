//! The local operator's registrations: register, unregister, start, stop, and list.

use ma2a_core::service::{ServiceError, ServiceManifest, ServiceName};
use ma2a_store::ServiceRegistration;
use tokio_util::sync::CancellationToken;

use super::{HostedServices, MAX_HOSTED_SERVICES, Notice};
use crate::api::HostedServiceView;

impl HostedServices {
    /// Registers a new service, stopped, or replaces a stopped one and its call counts;
    /// `true` when it is new.
    ///
    /// A started service is a conflict, and a new one beyond [`MAX_HOSTED_SERVICES`] is
    /// `concurrency_exceeded`.
    pub(crate) async fn register(
        &self,
        name: &ServiceName,
        manifest: ServiceManifest,
    ) -> Result<(HostedServiceView, bool), ServiceError> {
        let _lifecycle = self.0.lifecycle.lock().await;
        let (at_ms, name) = (self.now_ms(), name.clone());
        let limit = u32::try_from(MAX_HOSTED_SERVICES).unwrap_or(u32::MAX);
        let registered = self
            .store(move |repository| {
                let full = repository.hosted_service(&name)?.is_none()
                    && repository
                        .hosted_services((None, limit))?
                        .into_value()
                        .len()
                        >= MAX_HOSTED_SERVICES;
                if full {
                    return Ok(None);
                }
                let registered = repository.register_service((&name, &manifest), at_ms)?;
                Ok(Some(registered.into_value()))
            })
            .await?;
        match registered {
            None => Err(ServiceError::ConcurrencyExceeded),
            Some(ServiceRegistration::Registered(record, created)) => {
                // Counts belong to one registration: a replaced manifest may no longer
                // list the versions they were counted for.
                if let Some(live) = self.state().services.get_mut(&record.name) {
                    live.calls.clear();
                }
                Ok((self.observe(*record), created))
            }
            Some(ServiceRegistration::Active) => Err(ServiceError::Conflict),
            Some(ServiceRegistration::Missing) => Err(ServiceError::Internal),
        }
    }

    /// Removes a stopped service's registration and forgets its counters.
    pub(crate) async fn unregister(&self, name: &ServiceName) -> Result<(), ServiceError> {
        let _lifecycle = self.0.lifecycle.lock().await;
        let job_name = name.clone();
        let removed = self
            .store(move |repository| repository.unregister_service(&job_name))
            .await?
            .into_value();
        match removed {
            ServiceRegistration::Registered(..) => {
                self.state().services.remove(name);
                Ok(())
            }
            ServiceRegistration::Active => Err(ServiceError::Conflict),
            ServiceRegistration::Missing => Err(ServiceError::UnknownService),
        }
    }

    /// Starts a service: it is unavailable until its process attaches and reports ready.
    pub(crate) async fn start(
        &self,
        name: &ServiceName,
    ) -> Result<HostedServiceView, ServiceError> {
        let _lifecycle = self.0.lifecycle.lock().await;
        let record = self.set_enabled(name, true).await?;
        let mut state = self.state();
        let live = state.services.entry(name.clone()).or_default();
        if live.stop.is_cancelled() {
            live.stop = CancellationToken::new();
        }
        drop(state);
        Ok(self.observe(record))
    }

    /// Stops a service: its link is told to stop and ends, calls it has not accepted are
    /// withdrawn, and running calls are cancelled.
    pub(crate) async fn stop(&self, name: &ServiceName) -> Result<HostedServiceView, ServiceError> {
        let _lifecycle = self.0.lifecycle.lock().await;
        let record = self.set_enabled(name, false).await?;
        let mut state = self.state();
        if let Some(live) = state.services.get_mut(name) {
            live.stop.cancel();
            if let Some(link) = live.link.take() {
                let _told = link.notices.try_send(Notice::Stop);
            }
        }
        state.pending.retain(|_, pending| pending.service != *name);
        drop(state);
        Ok(self.observe(record))
    }

    async fn set_enabled(
        &self,
        name: &ServiceName,
        enabled: bool,
    ) -> Result<ma2a_store::HostedServiceRecord, ServiceError> {
        let (at_ms, name) = (self.now_ms(), name.clone());
        self.store(move |repository| repository.set_service_enabled(&name, (enabled, at_ms)))
            .await?
            .into_value()
            .ok_or(ServiceError::UnknownService)
    }

    /// Returns the revision of the read and at most `limit` registrations after `after`.
    pub(crate) async fn list(
        &self,
        (after, limit): (Option<ServiceName>, u32),
    ) -> Result<(u64, Vec<HostedServiceView>), ServiceError> {
        let listed = self
            .store(move |repository| repository.hosted_services((after.as_ref(), limit)))
            .await?;
        let revision = listed.revision();
        let records = listed.into_value();
        Ok((
            revision,
            records
                .into_iter()
                .map(|record| self.observe(record))
                .collect(),
        ))
    }

    /// Returns the Store revision, which every committed operator change has reached.
    pub(crate) async fn revision(&self) -> Result<u64, ServiceError> {
        self.store(|repository| repository.revision()).await
    }
}
