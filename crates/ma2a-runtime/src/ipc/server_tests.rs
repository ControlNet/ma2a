use std::{collections::BTreeMap, fs, sync::Mutex};

use ma2a_store::StoreConfig;

use crate::Runtime;

use super::dispatch;

#[tokio::test]
async fn conflicting_shutdown_dispatch_does_not_request_server_shutdown() {
    // Given
    let state_dir =
        std::env::temp_dir().join(format!("ma2a-shutdown-dispatch-{}", std::process::id()));
    fs::create_dir_all(&state_dir).expect("create state directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))
            .expect("secure state directory");
    }
    let runtime = Runtime::start(StoreConfig::new(&state_dir))
        .await
        .expect("start runtime");
    let replay = Mutex::new(BTreeMap::new());
    let first = br#"{"version":1,"operation":"graceful_shutdown","request_id":"00000000000000000000000000000001"}"#;
    let conflicting = br#"{"version":1,"operation":"session_revoke_all","request_id":"00000000000000000000000000000001"}"#;
    let accepted = dispatch(first, runtime.handle(), &replay)
        .await
        .expect("dispatch accepted shutdown");

    // When
    let rejected = dispatch(conflicting, runtime.handle(), &replay)
        .await
        .expect("dispatch conflicting shutdown");

    // Then
    assert!(accepted.1);
    assert!(!rejected.1);
    let rejected_value: serde_json::Value =
        serde_json::from_slice(&rejected.0).expect("decode conflicting response");
    assert_eq!(
        rejected_value
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some("conflict")
    );
    runtime.shutdown().await.expect("shutdown runtime");
    fs::remove_dir_all(state_dir).expect("remove state directory");
}
