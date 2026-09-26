//! Transaction receipts retain their own state even when another writer advances Store.

#[path = "common/support.rs"]
mod support;

use ma2a_store::{
    PasswordReset, PasswordTransition, Repository, StoreConfig, derive_password_verifier,
};
use support::{TempState, TestResult};

#[test]
fn credential_and_session_receipts_do_not_reread_a_later_revision() -> TestResult {
    let state = TempState::new("committed-auth-results")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let mut other = Repository::open(&config)?;
    let before = repository.revision()?;
    let reset = PasswordReset {
        verifier: derive_password_verifier(b"transaction-receipt-test-passphrase")?,
        verifier_version: 1,
        now_ms: 1,
    };
    let committed = repository
        .change_password_committed(PasswordTransition::Set, &reset)?
        .ok_or("credential change was refused")?;
    assert_eq!(committed.revision(), before + 1);
    assert_eq!(Some(committed.value().clone()), repository.credential()?);
    other.advance_revision()?;
    assert_eq!(committed.revision(), before + 1);
    assert_eq!(repository.revision()?, before + 2);
    let revoked = repository.revoke_all_sessions_committed(2)?;
    assert!(*revoked.value());
    assert_eq!(revoked.revision(), before + 2);
    other.advance_revision()?;
    assert_eq!(revoked.revision(), before + 2);
    assert_eq!(repository.revision()?, before + 3);
    Ok(())
}
