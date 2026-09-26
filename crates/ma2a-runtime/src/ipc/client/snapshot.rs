use serde_json::Value;

use crate::{
    api::{self, fragments::SnapshotAssembly},
    ipc::{IpcError, framing::read_frame, platform},
};

pub(super) async fn assemble(
    stream: &mut platform::PlatformStream,
    expected: (u64, &str),
    first: Vec<u8>,
) -> Result<Vec<u8>, IpcError> {
    let (correlation, result_type) = expected;
    let mut assembly = SnapshotAssembly::default();
    let mut complete = assembly.push(&first)?;
    while !complete {
        let frame = read_frame(stream, api::MAX_LOCAL_RESPONSE_BYTES).await?;
        if frame.correlation != correlation {
            return Err(IpcError::CorrelationMismatch);
        }
        complete = assembly.push(&frame.payload)?;
    }
    let (bytes, revision, boot) = assembly.finish()?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| IpcError::InvalidFrame)?;
    if value.get("revision").and_then(Value::as_u64) != Some(revision)
        || value.get("runtime_boot_id").and_then(Value::as_str) != Some(boot.as_str())
        || value.pointer("/result/type").and_then(Value::as_str) != Some(result_type)
        || (result_type == "snapshot"
            && value
                .pointer("/result/payload/revision")
                .and_then(Value::as_u64)
                != Some(revision))
    {
        return Err(IpcError::InvalidFrame);
    }
    Ok(bytes)
}
