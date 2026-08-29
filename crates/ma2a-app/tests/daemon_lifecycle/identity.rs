use std::{error::Error, fmt::Write as _, path::Path};

use ma2a_runtime::{
    api,
    ipc::{IpcPaths, LocalApiClient},
};
use ma2a_store::{Repository, StoreConfig};
use serde_json::Value;

pub(super) fn assert_live_endpoint_matches_persisted(
    state_dir: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let command = api::decode_command(br#"{"version":1,"operation":"endpoint_info"}"#)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let response =
        runtime.block_on(LocalApiClient::new(IpcPaths::new(state_dir)?).call(&command))?;
    let live: Value = serde_json::from_slice(&response)?;
    let live_endpoint_id = live
        .pointer("/result/payload/endpoint_id")
        .and_then(Value::as_str)
        .ok_or("live Endpoint ID missing")?;
    let persisted = Repository::open(&StoreConfig::new(state_dir))?
        .endpoint()?
        .ok_or("persisted Endpoint identity missing")?;
    let mut persisted_endpoint_id = String::with_capacity(64);
    for byte in persisted.endpoint_id().as_bytes() {
        write!(&mut persisted_endpoint_id, "{byte:02x}")?;
    }
    assert_eq!(live_endpoint_id, persisted_endpoint_id);
    Ok(())
}
