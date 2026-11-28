use std::{
    fmt,
    sync::{Arc, Mutex},
};

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
    /// The connection opened and verified at start, kept for every auth/session check:
    /// reopening runs the full startup verification, whose cost grows with history.
    sessions: Arc<Mutex<Repository>>,
    clock: Arc<dyn Clock>,
    config: WebAuthConfig,
    session_cookie: String,
    csrf_cookie: String,
}

impl WebAuthService {
    pub(super) fn now_ms(&self) -> i64 {
        self.inner.clock.now_ms()
    }

    /// Returns the host cookie name used for this Runtime's session bearer token.
    #[must_use]
    pub fn session_cookie_name(&self) -> &str {
        &self.inner.session_cookie
    }

    /// Returns the host cookie name used for this Runtime's CSRF token.
    #[must_use]
    pub fn csrf_cookie_name(&self) -> &str {
        &self.inner.csrf_cookie
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
        let (sessions, state_dir) = tokio::task::spawn_blocking(move || {
            let repo = Repository::open(&store)?;
            let dir = std::fs::canonicalize(store.state_dir())
                .unwrap_or_else(|_| store.state_dir().to_path_buf());
            Ok::<_, ma2a_store::StoreError>((repo, dir))
        })
        .await
        .map_err(|_| AuthFailure::Internal)?
        .map_err(|_| AuthFailure::Internal)?;
        let scope = blake3::Hasher::new()
            .update(b"ma2a-web-cookie-scope-v1\0")
            .update(state_dir.as_os_str().as_encoded_bytes())
            .finalize()
            .to_hex();
        let suffix = &scope[..32];
        Ok(Self {
            inner: Arc::new(WebAuthInner {
                sessions: Arc::new(Mutex::new(sessions)),
                clock,
                config,
                session_cookie: format!("ma2a_session_{suffix}"),
                csrf_cookie: format!("ma2a_csrf_{suffix}"),
            }),
        })
    }

    /// Reports whether CLI password initialization is required.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::Internal`] when credential state cannot be read.
    pub async fn setup_required(&self) -> Result<bool, AuthFailure> {
        self.with_repository(|repo| Ok(repo.credential()?.is_none()))
            .await
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
        let now_ms = self.inner.clock.now_ms();
        let sessions = Arc::clone(&self.inner.sessions);
        tokio::task::spawn_blocking(move || {
            let verifier =
                derive_password_verifier(password.as_bytes()).map_err(|_| AuthFailure::Internal)?;
            let changed = sessions
                .lock()
                .map_err(|_| AuthFailure::Internal)?
                .change_password_committed(
                    match action {
                        PasswordAction::Init => PasswordTransition::Init,
                        PasswordAction::Set => PasswordTransition::Set,
                        PasswordAction::Reset => PasswordTransition::Reset,
                    },
                    &PasswordReset {
                        verifier,
                        verifier_version: 1,
                        now_ms,
                    },
                )?;
            changed
                .map(|credential| CredentialState {
                    auth_epoch: credential.value().auth_epoch(),
                    revision: credential.revision(),
                })
                .ok_or(AuthFailure::Conflict)
        })
        .await
        .map_err(|_| AuthFailure::Internal)?
    }

    /// Verifies the password and rotates fresh bearer and CSRF tokens.
    ///
    /// # Errors
    ///
    /// Returns [`AuthFailure::Unauthorized`] for invalid credentials or a closed internal failure.
    pub async fn login(&self, password: Zeroizing<String>) -> Result<LoginSession, AuthFailure> {
        let sessions = Arc::clone(&self.inner.sessions);
        let now_ms = self.inner.clock.now_ms();
        let max_sessions = self.inner.config.max_sessions;
        let pair = csrf::generate_pair()?;
        let issued = tokio::task::spawn_blocking(move || {
            let credential = {
                let repository = sessions.lock().map_err(|_| AuthFailure::Internal)?;
                repository.credential()?.ok_or(AuthFailure::Unauthorized)?
            };
            // Verify Argon2id outside the sessions lock so concurrent requests never queue on it.
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
            let mut repository = sessions.lock().map_err(|_| AuthFailure::Internal)?;
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
        self.authenticate_session(bearer, None).await
    }

    pub(super) async fn validate(&self, bearer: &str) -> Result<AuthenticatedSession, AuthFailure> {
        let digest = csrf::bearer_digest(bearer)?;
        let now_ms = self.inner.clock.now_ms();
        self.with_repository(move |repo| {
            repo.validate_session(&digest, now_ms)?
                .ok_or(AuthFailure::Unauthorized)?;
            Ok(AuthenticatedSession {
                bearer_digest: digest,
            })
        })
        .await
    }

    pub(super) async fn authenticate_mutation(
        &self,
        bearer: &str,
        csrf_token: &str,
    ) -> Result<AuthenticatedSession, AuthFailure> {
        self.authenticate_session(bearer, Some(csrf::csrf_digest(csrf_token)?))
            .await
    }

    async fn authenticate_session(
        &self,
        bearer: &str,
        csrf_digest: Option<[u8; 32]>,
    ) -> Result<AuthenticatedSession, AuthFailure> {
        let digest = csrf::bearer_digest(bearer)?;
        let now_ms = self.inner.clock.now_ms();
        self.with_repository(move |repo| {
            let touch = SessionTouch::new(now_ms, WebAuthConfig::IDLE_TIMEOUT_MS);
            let session = match csrf_digest {
                Some(csrf) => repo.authenticate_and_touch_session_with_csrf(
                    &SessionDigests::new(digest, csrf),
                    touch,
                )?,
                None => repo.authenticate_and_touch_session(&digest, touch)?,
            }
            .ok_or(AuthFailure::Unauthorized)?;
            if !bool::from(session.bearer_digest().ct_eq(&digest)) {
                return Err(AuthFailure::Unauthorized);
            }
            Ok(AuthenticatedSession {
                bearer_digest: digest,
            })
        })
        .await
    }

    /// Revokes one authenticated session.
    ///
    /// # Errors
    ///
    /// Returns a closed internal failure when revocation cannot be persisted.
    pub async fn logout(&self, session: AuthenticatedSession) -> Result<(), AuthFailure> {
        let digest = *session.bearer_digest();
        self.with_repository(move |repo| Ok(repo.delete_session(&digest)?))
            .await
    }

    /// Revokes every browser session through the trusted local-control path.
    ///
    /// # Errors
    /// Returns a closed internal failure when revocation cannot be persisted.
    pub async fn revoke_all_sessions(&self) -> Result<(), AuthFailure> {
        self.revoke_all_sessions_committed().await.map(|_| ())
    }

    pub(crate) async fn revoke_all_sessions_committed(
        &self,
    ) -> Result<ma2a_store::Committed<bool>, AuthFailure> {
        let now_ms = self.inner.clock.now_ms();
        self.with_repository(move |repo| Ok(repo.revoke_all_sessions_committed(now_ms)?))
            .await
    }

    async fn with_repository<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Repository) -> Result<T, AuthFailure> + Send + 'static,
    ) -> Result<T, AuthFailure> {
        let sessions = Arc::clone(&self.inner.sessions);
        tokio::task::spawn_blocking(move || {
            let mut repository = sessions.lock().map_err(|_| AuthFailure::Internal)?;
            action(&mut repository)
        })
        .await
        .map_err(|_| AuthFailure::Internal)?
    }
}

impl fmt::Debug for WebAuthService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebAuthService { store: [REDACTED] }")
    }
}
