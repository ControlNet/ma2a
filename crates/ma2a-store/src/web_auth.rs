use std::str;

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher as _, PasswordVerifier as _, phc::PasswordHash},
};
use rusqlite::OptionalExtension as _;

use crate::{PasswordReset, Repository, StoreError, repository::increment_revision};

/// Argon2id memory cost in KiB.
pub const ARGON2_MEMORY_KIB: u32 = 19_456;
/// Argon2id iteration count.
pub const ARGON2_TIME_COST: u32 = 2;
/// Argon2id parallelism degree.
pub const ARGON2_PARALLELISM: u32 = 1;
/// Argon2id output size in bytes.
pub const ARGON2_OUTPUT_BYTES: usize = 32;
const PASSWORD_MIN_BYTES: usize = 1;
const PASSWORD_MAX_BYTES: usize = 1_024;

/// Persisted password verifier metadata without recoverable credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    verifier: Vec<u8>,
    verifier_version: u32,
    auth_epoch: u64,
}

/// Required credential-state transition for a trusted local command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PasswordTransition {
    /// Establish a verifier only when none exists.
    Set,
    /// Require an existing verifier before changing it.
    Reset,
}

impl CredentialRecord {
    /// Returns the PHC-encoded verifier bytes.
    #[must_use]
    pub fn verifier(&self) -> &[u8] {
        &self.verifier
    }

    /// Returns the verifier format version.
    #[must_use]
    pub const fn verifier_version(&self) -> u32 {
        self.verifier_version
    }

    /// Returns the epoch that invalidates older sessions.
    #[must_use]
    pub const fn auth_epoch(&self) -> u64 {
        self.auth_epoch
    }
}

impl std::fmt::Debug for CredentialRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialRecord")
            .field("verifier", &"[REDACTED]")
            .field("verifier_version", &self.verifier_version)
            .field("auth_epoch", &self.auth_epoch)
            .finish()
    }
}

/// Derives a PHC-encoded Argon2id verifier using the locked resource bounds.
///
/// # Errors
///
/// Returns [`StoreError::InvalidPasswordLength`] outside the accepted byte range
/// or [`StoreError::PasswordHash`] when secure salt generation or hashing fails.
pub fn derive_password_verifier(password: &[u8]) -> Result<Vec<u8>, StoreError> {
    if !(PASSWORD_MIN_BYTES..=PASSWORD_MAX_BYTES).contains(&password.len()) {
        return Err(StoreError::InvalidPasswordLength);
    }
    let verifier = argon2()?
        .hash_password(password)
        .map_err(|_| StoreError::PasswordHash)?;
    let encoded = verifier.to_string().into_bytes();
    validate_password_verifier(&encoded, 1)?;
    Ok(encoded)
}

/// Verifies a password only against the locked Argon2id PHC shape.
///
/// # Errors
///
/// Returns [`StoreError::InvalidPasswordLength`] outside the accepted byte range
/// or [`StoreError::PasswordHash`] for malformed or non-conforming verifiers.
pub fn verify_password(verifier: &[u8], password: &[u8]) -> Result<bool, StoreError> {
    if !(PASSWORD_MIN_BYTES..=PASSWORD_MAX_BYTES).contains(&password.len()) {
        return Err(StoreError::InvalidPasswordLength);
    }
    let parsed = validate_password_verifier(verifier, 1)?;
    match argon2()?.verify_password(password, &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
        Err(_) => Err(StoreError::PasswordHash),
    }
}

impl Repository {
    /// Reads the configured password verifier, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when credential state cannot be read.
    pub fn credential(&self) -> Result<Option<CredentialRecord>, StoreError> {
        let credential = self
            .connection
            .query_row(
                "SELECT password_verifier, verifier_version, auth_epoch
                 FROM ui_credentials WHERE singleton = 1",
                [],
                |row| {
                    Ok(CredentialRecord {
                        verifier: row.get(0)?,
                        verifier_version: row.get(1)?,
                        auth_epoch: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)?;
        if let Some(record) = credential.as_ref() {
            validate_password_verifier(&record.verifier, record.verifier_version)?;
        }
        Ok(credential)
    }

    /// Replaces the password verifier and revokes every session atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when credential or session state cannot be committed.
    pub fn change_password(
        &mut self,
        transition: PasswordTransition,
        reset: &PasswordReset,
    ) -> Result<Option<CredentialRecord>, StoreError> {
        validate_password_verifier(&reset.verifier, reset.verifier_version)?;
        let transaction = self.immediate()?;
        let configured = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM ui_credentials WHERE singleton = 1)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        match (transition, configured) {
            (PasswordTransition::Set, false) | (PasswordTransition::Reset, true) => {}
            (PasswordTransition::Set, true) | (PasswordTransition::Reset, false) => {
                return Ok(None);
            }
        }
        transaction.execute(
            "INSERT INTO ui_credentials(
                singleton, password_verifier, verifier_version, updated_at_ms, auth_epoch
             ) VALUES (1, ?1, ?2, ?3, 1)
             ON CONFLICT(singleton) DO UPDATE SET
                password_verifier = excluded.password_verifier,
                verifier_version = excluded.verifier_version,
                updated_at_ms = excluded.updated_at_ms,
                auth_epoch = ui_credentials.auth_epoch + 1",
            (
                reset.verifier.as_slice(),
                reset.verifier_version,
                reset.now_ms,
            ),
        )?;
        transaction.execute(
            "UPDATE sessions SET revoked_at_ms = ?1 WHERE revoked_at_ms IS NULL",
            [reset.now_ms],
        )?;
        let auth_epoch = transaction.query_row(
            "SELECT auth_epoch FROM ui_credentials WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(Some(CredentialRecord {
            verifier: reset.verifier.clone(),
            verifier_version: reset.verifier_version,
            auth_epoch,
        }))
    }
}

fn argon2() -> Result<Argon2<'static>, StoreError> {
    Ok(Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        argon_params()?,
    ))
}

fn argon_params() -> Result<Params, StoreError> {
    Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        Some(ARGON2_OUTPUT_BYTES),
    )
    .map_err(|_| StoreError::PasswordHash)
}

fn validate_password_verifier(
    verifier: &[u8],
    verifier_version: u32,
) -> Result<PasswordHash, StoreError> {
    if verifier_version != 1 {
        return Err(StoreError::PasswordHash);
    }
    let encoded = str::from_utf8(verifier).map_err(|_| StoreError::PasswordHash)?;
    let parsed = PasswordHash::new(encoded).map_err(|_| StoreError::PasswordHash)?;
    let params = Params::try_from(&parsed).map_err(|_| StoreError::PasswordHash)?;
    if parsed.algorithm != Algorithm::Argon2id.ident()
        || parsed.version != Some(u32::from(Version::V0x13))
        || params != argon_params()?
        || parsed
            .hash
            .is_none_or(|hash| hash.len() != ARGON2_OUTPUT_BYTES)
    {
        return Err(StoreError::PasswordHash);
    }
    Ok(parsed)
}
