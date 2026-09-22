//! Credential epoch and session admission transaction coverage.

#[path = "common/support.rs"]
mod support;

use std::sync::{Arc, Barrier};

use ma2a_store::{
    PasswordReset, PasswordTransition, Repository, SessionAdmission, SessionCreate, SessionDigests,
    SessionRecord, SessionTimestamps, SessionTouch, StoreConfig, StoreError,
    derive_password_verifier,
};
use support::{TempState, TestResult};

#[test]
fn concurrent_session_admission_has_one_durable_winner() -> TestResult {
    // Given
    let state = TempState::new("session-admission")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let verifier = derive_password_verifier(b"session-admission-passphrase-9!")?;
    let credential = repository
        .change_password(
            PasswordTransition::Set,
            &PasswordReset {
                verifier,
                verifier_version: 1,
                now_ms: 1,
            },
        )?
        .ok_or("password setup did not commit")?;
    drop(repository);
    let sessions = [
        SessionRecord::new(
            SessionDigests::new([1; 32], [11; 32]),
            credential.auth_epoch(),
            SessionTimestamps::new([2, 2], [100, 200]),
        ),
        SessionRecord::new(
            SessionDigests::new([2; 32], [12; 32]),
            credential.auth_epoch(),
            SessionTimestamps::new([2, 2], [100, 200]),
        ),
    ];
    let barrier = Arc::new(Barrier::new(3));
    let handles = sessions.map(|session| {
        let config = config.clone();
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            let mut repository = Repository::open(&config)?;
            barrier.wait();
            repository.create_session_if_current(&session, SessionAdmission::new(1, 2))
        })
    });

    // When
    barrier.wait();
    let outcomes = handles.map(|handle| {
        handle
            .join()
            .map_err(|_| "session admission thread panicked")?
            .map_err(Into::into)
    });
    let outcomes = outcomes
        .into_iter()
        .collect::<Result<Vec<_>, Box<dyn std::error::Error + Send + Sync>>>()?;
    let repository = Repository::open(&config)?;

    // Then
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == SessionCreate::Created)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == SessionCreate::LimitReached)
            .count(),
        1
    );
    assert_eq!(
        sessions
            .iter()
            .filter_map(|session| repository.session(session.bearer_digest()).transpose())
            .collect::<Result<Vec<_>, _>>()?
            .len(),
        1
    );
    Ok(())
}

#[test]
fn password_change_rejects_nonconforming_verifier_state() -> TestResult {
    // Given
    let state = TempState::new("strict-verifier")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let valid = derive_password_verifier(b"valid-test-passphrase-9!")?;

    // When
    let wrong_version = repository.change_password(
        PasswordTransition::Set,
        &PasswordReset {
            verifier: valid,
            verifier_version: 2,
            now_ms: 1,
        },
    );
    let malformed = repository.change_password(
        PasswordTransition::Set,
        &PasswordReset {
            verifier: b"not-a-phc-verifier".to_vec(),
            verifier_version: 1,
            now_ms: 1,
        },
    );

    // Then
    assert!(matches!(wrong_version, Err(StoreError::PasswordHash)));
    assert!(matches!(malformed, Err(StoreError::PasswordHash)));
    assert!(repository.credential()?.is_none());
    Ok(())
}

#[test]
fn wrong_csrf_digest_does_not_touch_a_valid_session() -> TestResult {
    // Given
    let state = TempState::new("csrf-touch")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let verifier = derive_password_verifier(b"csrf-touch-passphrase-9!")?;
    let credential = repository
        .change_password(
            PasswordTransition::Set,
            &PasswordReset {
                verifier,
                verifier_version: 1,
                now_ms: 1,
            },
        )?
        .ok_or("password setup did not commit")?;
    let bearer = [3; 32];
    let csrf = [4; 32];
    repository.create_session(&SessionRecord::new(
        SessionDigests::new(bearer, csrf),
        credential.auth_epoch(),
        SessionTimestamps::new([2, 2], [100, 200]),
    ))?;

    // When
    let denied = repository.authenticate_and_touch_session_with_csrf(
        &SessionDigests::new(bearer, [5; 32]),
        SessionTouch::new(50, 75),
    )?;
    let persisted = repository.session(&bearer)?.ok_or("session missing")?;

    // Then
    assert!(denied.is_none());
    assert_eq!(
        persisted,
        SessionRecord::new(
            SessionDigests::new(bearer, csrf),
            credential.auth_epoch(),
            SessionTimestamps::new([2, 2], [100, 200]),
        )
    );
    Ok(())
}

#[test]
fn sliding_a_session_does_not_advance_the_revision_but_revoking_it_does() -> TestResult {
    // Given
    let state = TempState::new("session-slide-revision")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let verifier = derive_password_verifier(b"session-slide-passphrase-9!")?;
    let credential = repository
        .change_password(
            PasswordTransition::Set,
            &PasswordReset {
                verifier,
                verifier_version: 1,
                now_ms: 1,
            },
        )?
        .ok_or("password setup did not commit")?;
    let bearer = [7; 32];
    repository.create_session(&SessionRecord::new(
        SessionDigests::new(bearer, [8; 32]),
        credential.auth_epoch(),
        SessionTimestamps::new([2, 2], [100, 200]),
    ))?;
    let created_at = repository.revision()?;

    // When
    let slid = repository
        .authenticate_and_touch_session(&bearer, SessionTouch::new(50, 25))?
        .ok_or("a valid session was refused")?;
    let after_slide = repository.revision()?;
    let persisted = repository.session(&bearer)?.ok_or("session missing")?;
    repository.revoke_all_sessions(60)?;
    let after_revocation = repository.revision()?;

    // Then
    assert_eq!(slid, persisted);
    assert_ne!(
        persisted,
        SessionRecord::new(
            SessionDigests::new(bearer, [8; 32]),
            credential.auth_epoch(),
            SessionTimestamps::new([2, 2], [100, 200]),
        )
    );
    assert_eq!(after_slide, created_at);
    assert!(after_revocation > created_at);
    Ok(())
}
