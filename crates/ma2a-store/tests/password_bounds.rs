//! Synthetic password fixtures exercise UTF-8 byte boundaries, not real credentials.

use ma2a_store::{StoreError, derive_password_verifier, verify_password};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn accepts_passwords_from_one_through_1024_bytes() -> TestResult {
    let multibyte = format!("{}a", "\u{4e2d}".repeat(341));
    assert_eq!(multibyte.len(), 1_024);
    for password in [
        vec![b'a'],
        vec![b'a'; 11],
        vec![b'a'; 1_024],
        multibyte.into_bytes(),
    ] {
        let verifier = derive_password_verifier(&password)?;
        assert!(verify_password(&verifier, &password)?);
        assert!(!verify_password(&verifier, b"b")?);
    }
    Ok(())
}

#[test]
fn rejects_empty_and_oversized_passwords_for_setting_and_verification() -> TestResult {
    let verifier = derive_password_verifier(b"boundary-test-fixture")?;
    for password in [
        Vec::new(),
        vec![b'a'; 1_025],
        "\u{4e2d}".repeat(342).into_bytes(),
    ] {
        assert!(matches!(
            derive_password_verifier(&password),
            Err(StoreError::InvalidPasswordLength)
        ));
        assert!(matches!(
            verify_password(&verifier, &password),
            Err(StoreError::InvalidPasswordLength)
        ));
    }
    Ok(())
}
