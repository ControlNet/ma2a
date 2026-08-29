use std::{collections::BTreeMap, fs, sync::Mutex};

use ma2a_core::RequestId;
use ma2a_store::StoreConfig;

use crate::{Runtime, api::CommandResult};

use super::{ReplayEntry, dispatch};

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
    let request_id = RequestId::try_from(&[1_u8; 16][..]).expect("request identifier");
    replay.lock().expect("lock replay state").insert(
        request_id,
        ReplayEntry {
            fingerprint: [0_u8; 32],
            result: CommandResult::shutting_down(),
        },
    );
    let conflicting = br#"{"version":1,"operation":"graceful_shutdown","request_id":"01010101010101010101010101010101"}"#;

    // When
    let rejected = dispatch(conflicting, runtime.handle(), &replay)
        .await
        .expect("dispatch conflicting shutdown");

    // Then
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
