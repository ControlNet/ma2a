use std::{fmt, sync::Arc};

use ma2a_store::{
    PasswordReset, PasswordTransition, Repository, SessionAdmission, SessionCreate, SessionDigests,
    SessionRecord, SessionTimestamps, SessionTouch, StoreConfig, derive_password_verifier,
    verify_password,
};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use super::{
    csrf,
    types::{
        AuthFailure, AuthenticatedSession, Clock, CredentialState, LoginSession, PasswordAction,
        WebAuthConfig,
    },
};

/// Cloneable authentication service backed by the current-user store.
#[derive(Clone)]
pub struct WebAuthService {
    inner: Arc<WebAuthInner>,
}

struct WebAuthInner {
    store: StoreConfig,
    clock: Arc<dyn Clock>,
    config: WebAuthConfig,
}

impl WebAuthService {
    pub(super) fn now_ms(&self) -> i64 {
        self.inner.clock.now_ms()
    }

    /// Opens and validates the authentication store.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::Internal`] when the store cannot be opened.
    pub async fn open(
        store: StoreConfig,
        clock: Arc<dyn Clock>,
        config: WebAuthConfig,
    ) -> Result<Self, AuthFailure> {
        let open_config = store.clone();
        tokio::task::spawn_blocking(move || Repository::open(&open_config))
            .await
            .map_err(|_| AuthFailure::Internal)?
            .map_err(|_| AuthFailure::Internal)?;
        Ok(Self {
            inner: Arc::new(WebAuthInner {
                store,
                clock,
                config,
            }),
        })
    }

    /// Reports whether CLI password initialization is required.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::Internal`] when credential state cannot be read.
    pub async fn setup_required(&self) -> Result<bool, AuthFailure> {
        let config = self.inner.store.clone();
        tokio::task::spawn_blocking(move || Repository::open(&config)?.credential())
            .await
            .map_err(|_| AuthFailure::Internal)?
            .map(|credential| credential.is_none())
            .map_err(|_| AuthFailure::Internal)
    }

    /// Performs a CLI-authorized password lifecycle transition.
    ///
    /// # Errors
    ///
    /// Returns a conflict for an invalid set/reset transition or a closed internal failure.
    pub async fn change_password(
        &self,
        action: PasswordAction,
        password: Zeroizing<String>,
    ) -> Result<CredentialState, AuthFailure> {
        let config = self.inner.store.clone();
        let now_ms = self.inner.clock.now_ms();
        tokio::task::spawn_blocking(move || {
            let verifier = derive_password_verifier(password.as_bytes())?;
            Repository::open(&config)?
                .change_password(
                    match action {
                        PasswordAction::Set => PasswordTransition::Set,
                        PasswordAction::Reset => PasswordTransition::Reset,
                    },
                    &PasswordReset {
                        verifier,
                        verifier_version: 1,
                        now_ms,
                    },
                )
                .map(|credential| {
                    credential.map(|credential| CredentialState {
                        auth_epoch: credential.auth_epoch(),
                    })
                })
        })
        .await
        .map_err(|_| AuthFailure::Internal)?
        .map_err(|_: ma2a_store::StoreError| AuthFailure::Internal)?
        .ok_or(AuthFailure::Conflict)
    }

    /// Verifies the password and rotates fresh bearer and CSRF tokens.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::Unauthorized`] for invalid credentials or a closed internal failure.
    pub async fn login(&self, password: Zeroizing<String>) -> Result<LoginSession, AuthFailure> {
        let config = self.inner.store.clone();
        let now_ms = self.inner.clock.now_ms();
        let max_sessions = self.inner.config.max_sessions;
        let pair = csrf::generate_pair()?;
        let issued = tokio::task::spawn_blocking(move || {
            let mut repository = Repository::open(&config)?;
            let credential = repository.credential()?.ok_or(AuthFailure::Unauthorized)?;
            if !verify_password(credential.verifier(), password.as_bytes()).unwrap_or(false) {
                return Err(AuthFailure::Unauthorized);
            }
            let session = SessionRecord::new(
                SessionDigests::new(pair.bearer_digest, pair.csrf_digest),
                credential.auth_epoch(),
                SessionTimestamps::new(
                    [now_ms, now_ms],
                    [
                        now_ms + WebAuthConfig::IDLE_TIMEOUT_MS,
                        now_ms + WebAuthConfig::ABSOLUTE_TIMEOUT_MS,
                    ],
                ),
            );
            match repository
                .create_session_if_current(&session, SessionAdmission::new(max_sessions, now_ms))?
            {
                SessionCreate::Created => Ok((pair.bearer, pair.csrf)),
                SessionCreate::StaleCredential | SessionCreate::LimitReached => {
                    Err(AuthFailure::Unauthorized)
                }
                _ => Err(AuthFailure::Internal),
            }
        })
        .await
        .map_err(|_| AuthFailure::Internal)??;
        Ok(LoginSession {
            bearer: issued.0,
            csrf_token: issued.1,
        })
    }

    /// Authenticates and slides one unexpired session.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::Unauthorized`] for malformed, expired, revoked, or stale sessions.
    pub async fn authenticate(&self, bearer: &str) -> Result<AuthenticatedSession, AuthFailure> {
        let digest = csrf::bearer_digest(bearer)?;
        let config = self.inner.store.clone();
        let now_ms = self.inner.clock.now_ms();
        tokio::task::spawn_blocking(move || {
            let mut repository = Repository::open(&config)?;
            let session = repository
                .authenticate_and_touch_session(
                    &digest,
                    SessionTouch::new(now_ms, WebAuthConfig::IDLE_TIMEOUT_MS),
                )?
                .ok_or(AuthFailure::Unauthorized)?;
            if !bool::from(session.bearer_digest().ct_eq(&digest)) {
                return Err(AuthFailure::Unauthorized);
            }
            Ok(AuthenticatedSession {
                bearer_digest: digest,
                csrf_digest: *session.csrf_digest(),
            })
        })
        .await
        .map_err(|_| AuthFailure::Internal)?
    }

    /// Revokes one authenticated session.
    ///
    /// # Errors
    ///
    /// Returns a closed internal failure when revocation cannot be persisted.
    pub async fn logout(&self, session: AuthenticatedSession) -> Result<(), AuthFailure> {
        let config = self.inner.store.clone();
        let digest = *session.bearer_digest();
        tokio::task::spawn_blocking(move || Repository::open(&config)?.delete_session(&digest))
            .await
            .map_err(|_| AuthFailure::Internal)?
            .map_err(|_| AuthFailure::Internal)
    }

    /// Revokes every browser session through the trusted local-control path.
    ///
    /// # Errors
    /// Returns a closed internal failure when revocation cannot be persisted.
    pub async fn revoke_all_sessions(&self) -> Result<(), AuthFailure> {
        let config = self.inner.store.clone();
        let now_ms = self.inner.clock.now_ms();
        tokio::task::spawn_blocking(move || Repository::open(&config)?.revoke_all_sessions(now_ms))
            .await
            .map_err(|_| AuthFailure::Internal)?
            .map_err(|_| AuthFailure::Internal)
    }
}

impl fmt::Debug for WebAuthService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebAuthService { store: [REDACTED] }")
    }
}
