use ma2a_net::{PrivateRelayServer, RuntimeRelayConfiguration};

use super::{
    Actor,
    relay_server::{PrivateRelayApply, RelayCompletion},
};
use crate::error::{RuntimeError, RuntimeErrorKind};

#[cfg(test)]
pub(super) struct TlsLoadGate {
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
    pub(super) released_in_time: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
pub(super) static TLS_LOAD_GATE: std::sync::Mutex<Option<TlsLoadGate>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub(super) static TLS_LOAD_GATE_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

pub(super) fn load_native_tls_blocking(
    tls: &ma2a_net::NativeRelayTlsConfig,
) -> Result<ma2a_net::LoadedRelayTls, RuntimeError> {
    load_native_tls_raw(tls).map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
}

fn load_native_tls_raw(
    tls: &ma2a_net::NativeRelayTlsConfig,
) -> Result<ma2a_net::LoadedRelayTls, ma2a_net::RelayTlsError> {
    #[cfg(test)]
    {
        let gate = TLS_LOAD_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(gate) = gate {
            let _entered = gate.entered.send(());
            let woke = gate
                .release
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok();
            gate.released_in_time
                .store(woke, std::sync::atomic::Ordering::SeqCst);
        }
    }
    tls.load_tls()
}

impl Actor {
    pub(super) async fn relay_runtime_status(
        &self,
    ) -> Result<super::RelayRuntimeStatus, RuntimeError> {
        let desired = self.store.relay_configuration_committed().await?;
        let revision = desired.revision();
        self.relay_status_for(desired.into_value(), revision)
    }

    fn relay_status_for(
        &self,
        configuration: ma2a_store::RelayConfiguration,
        revision: u64,
    ) -> Result<super::RelayRuntimeStatus, RuntimeError> {
        let desired = RuntimeRelayConfiguration::try_from(configuration.clone())
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let applied = self
            .private_relay_server
            .as_ref()
            .map(PrivateRelayServer::configuration);
        let convergence_pending =
            self.maintenance.relay_configuration.is_some() || applied != desired.private_provider();
        Ok(super::RelayRuntimeStatus {
            revision,
            configuration,
            applied_private: self
                .private_relay_server
                .as_ref()
                .map(|server| server.configuration().clone()),
            convergence_pending,
            private_listen_addr: self
                .private_relay_server
                .as_ref()
                .map(PrivateRelayServer::listen_addr),
            public_relay_online: self.state.relay.public_relay_online(),
        })
    }

    pub(super) async fn set_relay_configuration(
        &mut self,
        configuration: ma2a_store::RelayConfiguration,
        (mode, preloaded_tls): (PrivateRelayApply, Option<ma2a_net::LoadedRelayTls>),
    ) -> Result<ma2a_store::Committed<super::RelayRuntimeStatus>, RuntimeError> {
        let runtime = RuntimeRelayConfiguration::try_from(configuration.clone())
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let clear_published = mode == PrivateRelayApply::Reload
            || self
                .private_relay_server
                .as_ref()
                .map(PrivateRelayServer::configuration)
                != runtime.private_provider();
        // Every URL parsed, so it is saved and reported as parsed, as startup would save it.
        let configuration = crate::store::with_parsed_relay_urls(configuration);
        let revision = self
            .store
            .set_relay_configuration(configuration.clone())
            .await?;
        self.state.revision = self.state.revision.max(revision);
        if clear_published {
            self.maintenance.published_relay.clear();
        }
        self.maintenance.relay_configuration = Some(RelayCompletion::Pending(mode));
        self.reconcile_relay_configuration_with(preloaded_tls)
            .await
            .map_err(|error| error.after_commit(revision))?;
        Ok(ma2a_store::Committed::new(
            revision,
            self.relay_status_for(configuration, revision)
                .map_err(|error| error.after_commit(revision))?,
        ))
    }

    pub(crate) async fn reconcile_relay_configuration(&mut self) -> Result<(), RuntimeError> {
        self.reconcile_relay_configuration_with(None).await
    }

    pub(crate) async fn reconcile_relay_configuration_maintenance(
        &mut self,
    ) -> Result<(), RuntimeError> {
        let Some((runtime, mode)) = self.pending_desired_relay().await? else {
            return Ok(());
        };
        if let Some(provider) = runtime.private_provider()
            && let ma2a_net::PrivateRelayTransport::NativeTls(tls) = provider.transport()
            && self.needs_private_relay_restart(Some(provider), mode)
        {
            self.maintenance.relay_configuration = Some(RelayCompletion::Pending(mode));
            if self.maintenance.relay_tls_loading.as_ref() != Some(tls) {
                let tls_key = tls.clone();
                let tls_task = tls.clone();
                self.maintenance.relay_tls_loading = Some(tls.clone());
                self.maintenance.relay_tls_loads.spawn(async move {
                    let result =
                        tokio::task::spawn_blocking(move || load_native_tls_raw(&tls_task))
                            .await
                            .unwrap_or(Err(ma2a_net::RelayTlsError::TlsConfiguration));
                    (tls_key, result)
                });
            }
            return Ok(());
        }
        self.reconcile_desired_relay(runtime, (mode, None)).await
    }

    pub(crate) async fn finish_relay_tls_load(
        &mut self,
        (loaded_for, loaded): (
            ma2a_net::NativeRelayTlsConfig,
            Result<ma2a_net::LoadedRelayTls, ma2a_net::RelayTlsError>,
        ),
    ) -> Result<(), RuntimeError> {
        if self.maintenance.relay_tls_loading.as_ref() == Some(&loaded_for) {
            self.maintenance.relay_tls_loading = None;
        }
        let Some((runtime, mode)) = self.pending_desired_relay().await? else {
            return Ok(());
        };
        let Some(provider) = runtime.private_provider() else {
            return Ok(());
        };
        let ma2a_net::PrivateRelayTransport::NativeTls(current_tls) = provider.transport() else {
            return Ok(());
        };
        if current_tls != &loaded_for || !self.needs_private_relay_restart(Some(provider), mode) {
            return Ok(());
        }
        let loaded = loaded.map_err(|error| {
            RuntimeError::new(RuntimeErrorKind::PrivateRelay(
                ma2a_net::PrivateRelayServerError::Tls(error),
            ))
        })?;
        self.reconcile_desired_relay(runtime, (mode, Some(loaded)))
            .await
    }

    async fn pending_desired_relay(
        &self,
    ) -> Result<Option<(RuntimeRelayConfiguration, PrivateRelayApply)>, RuntimeError> {
        let desired = self.store.relay_configuration().await?;
        let runtime = RuntimeRelayConfiguration::try_from(desired)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let Some(mode) = self.pending_relay_mode(runtime.private_provider())? else {
            return Ok(None);
        };
        Ok(Some((runtime, mode)))
    }

    fn pending_relay_mode(
        &self,
        desired: Option<&ma2a_net::PrivateRelayProviderConfig>,
    ) -> Result<Option<PrivateRelayApply>, RuntimeError> {
        let applied = self
            .private_relay_server
            .as_ref()
            .map(PrivateRelayServer::configuration);
        match self.maintenance.relay_configuration {
            Some(RelayCompletion::ShutdownFailed) => {
                Err(RuntimeError::new(RuntimeErrorKind::Shutdown))
            }
            Some(RelayCompletion::Pending(mode)) => Ok(Some(mode)),
            None if applied == desired => Ok(None),
            None => Ok(Some(PrivateRelayApply::IfChanged)),
        }
    }

    fn needs_private_relay_restart(
        &self,
        desired: Option<&ma2a_net::PrivateRelayProviderConfig>,
        mode: PrivateRelayApply,
    ) -> bool {
        let applied = self
            .private_relay_server
            .as_ref()
            .map(PrivateRelayServer::configuration);
        !(applied == desired && mode == PrivateRelayApply::IfChanged)
    }

    async fn reconcile_relay_configuration_with(
        &mut self,
        preloaded_tls: Option<ma2a_net::LoadedRelayTls>,
    ) -> Result<(), RuntimeError> {
        let Some((runtime, mode)) = self.pending_desired_relay().await? else {
            return Ok(());
        };
        self.reconcile_desired_relay(runtime, (mode, preloaded_tls))
            .await
    }

    async fn reconcile_desired_relay(
        &mut self,
        runtime: RuntimeRelayConfiguration,
        (mode, preloaded_tls): (PrivateRelayApply, Option<ma2a_net::LoadedRelayTls>),
    ) -> Result<(), RuntimeError> {
        self.maintenance.relay_configuration = Some(RelayCompletion::Pending(mode));
        self.apply_private_relay(runtime.private_provider(), (mode, preloaded_tls))
            .await?;
        // Once a server is installed, later publication failures must not restart it again.
        self.maintenance.relay_configuration =
            Some(RelayCompletion::Pending(PrivateRelayApply::IfChanged));
        self.refresh_private_relay_access().await?;
        self.refresh_local_relay_publication().await?;
        self.refresh_relay_candidates().await?;
        self.maintenance.relay_configuration = None;
        let _receiver_count = self
            .events
            .send(crate::state::RuntimeEvent::memberships_changed(
                self.state.revision,
            ));
        Ok(())
    }

    pub(crate) async fn refresh_private_relay_access(&self) -> Result<(), RuntimeError> {
        if let Some(server) = &self.private_relay_server {
            let authorizations = self
                .store
                .relay_authorizations(self.state.endpoint_id)
                .await?;
            server.replace_from_spaces(&authorizations);
        }
        Ok(())
    }
}
