use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};

use super::{WebAssets, WebAuthService, rate_limit::LoginRateLimit};

/// Dependencies for Runtime-backed Web routes.
#[derive(Clone, Debug)]
pub struct WebRuntimeDependencies {
    auth: WebAuthService,
    assets: WebAssets,
    runtime: crate::ipc::LocalApiClient,
}

impl WebRuntimeDependencies {
    /// Creates Runtime-backed Web route dependencies.
    pub const fn new(
        auth: WebAuthService,
        assets: WebAssets,
        runtime: crate::ipc::LocalApiClient,
    ) -> Self {
        Self {
            auth,
            assets,
            runtime,
        }
    }
}

/// Bounded loopback HTTP server settings.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct WebServerConfig {
    pub(super) request_timeout: Duration,
}

impl WebServerConfig {
    /// Maximum accepted request body.
    pub const MAX_BODY_BYTES: usize = 16_384;
    /// Login attempts accepted per fixed rolling window.
    pub const LOGIN_ATTEMPTS: usize = 5;
    const LOGIN_WINDOW_MS: i64 = 60_000;

    /// Overrides the per-request deadline.
    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

impl Default for WebServerConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct WebState {
    pub(super) auth: WebAuthService,
    pub(super) assets: WebAssets,
    pub(super) config: WebServerConfig,
    pub(super) port: u16,
    pub(super) login_rate: Arc<LoginRateLimit>,
    pub(super) runtime: Option<crate::ipc::LocalApiClient>,
    pub(super) event_connections: Arc<tokio::sync::Semaphore>,
}

struct WebStateDependencies {
    auth: WebAuthService,
    assets: WebAssets,
    runtime: Option<crate::ipc::LocalApiClient>,
}

impl WebState {
    fn new(dependencies: WebStateDependencies, config: WebServerConfig, port: u16) -> Self {
        Self {
            auth: dependencies.auth,
            assets: dependencies.assets,
            config,
            port,
            login_rate: Arc::new(LoginRateLimit::new(
                WebServerConfig::LOGIN_ATTEMPTS,
                WebServerConfig::LOGIN_WINDOW_MS,
            )),
            runtime: dependencies.runtime,
            event_connections: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }
}

/// Builds the loopback router for a selected listener port.
#[expect(
    clippy::too_many_arguments,
    reason = "the public testable router seam mirrors its four independent construction inputs"
)]
pub fn build_router(
    auth: WebAuthService,
    assets: WebAssets,
    config: WebServerConfig,
    port: u16,
) -> Router {
    routes(
        WebState::new(
            WebStateDependencies {
                auth,
                assets,
                runtime: None,
            },
            config,
            port,
        ),
        false,
    )
}

/// Builds the authenticated loopback router backed by the daemon Runtime API.
pub fn build_runtime_router(
    dependencies: WebRuntimeDependencies,
    config: WebServerConfig,
    port: u16,
) -> Router {
    routes(
        WebState::new(
            WebStateDependencies {
                auth: dependencies.auth,
                assets: dependencies.assets,
                runtime: Some(dependencies.runtime),
            },
            config,
            port,
        ),
        true,
    )
}

fn routes(state: WebState, runtime: bool) -> Router {
    let router = Router::new()
        .route("/api/v1/web/auth/state", get(super::auth_state))
        .route("/api/v1/web/auth/login", post(super::login))
        .route("/api/v1/web/auth/logout", post(super::logout))
        .route("/api/v1/web/session/touch", post(super::touch_session));
    let router = if runtime {
        router
            .route("/api/v1/snapshot", get(super::runtime_routes::snapshot))
            .route("/api/v1/events", get(super::runtime_routes::events))
            .route(
                "/api/v1/spaces/create",
                post(super::runtime_mutations::mutation),
            )
            .route(
                "/api/v1/spaces/invite",
                post(super::runtime_mutations::mutation),
            )
            .route(
                "/api/v1/spaces/redeem",
                post(super::runtime_mutations::mutation),
            )
            .route(
                "/api/v1/spaces/revoke",
                post(super::runtime_mutations::mutation),
            )
            .route(
                "/api/v1/control-sync/trigger",
                post(super::runtime_mutations::mutation),
            )
            .route(
                "/api/v1/relays/private/configure",
                post(super::runtime_mutations::mutation),
            )
            .route(
                "/api/v1/relays/public/configure",
                post(super::runtime_mutations::mutation),
            )
            .route("/api/v1/echo", post(super::runtime_mutations::mutation))
            .route(
                "/api/v1/sessions/revoke-all",
                post(super::runtime_mutations::mutation),
            )
    } else {
        router
    };
    router
        .fallback(super::asset)
        .layer(DefaultBodyLimit::max(WebServerConfig::MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            super::headers::request_guards,
        ))
        .with_state(state)
}
