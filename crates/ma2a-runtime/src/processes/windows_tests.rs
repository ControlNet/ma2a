//! Supervisor pipe names on Windows, where one directory has many spellings.

use std::{path::Path, sync::Arc};

use ma2a_core::service::ProcessId;
use ma2a_services::supervisor::SupervisorAddress;
use tokio_util::sync::CancellationToken;

use super::ProcessManager;
use crate::{clock::SystemClock, store::StoreClient};

fn address(state_dir: &Path, id: ProcessId) -> SupervisorAddress {
    let local = iroh::SecretKey::from_bytes(&[0x11; 32]).public().into();
    let store = StoreClient::new(tokio::sync::mpsc::channel(1).0);
    let manager = ProcessManager::new(
        (local, store, Arc::new(SystemClock)),
        (state_dir, None),
        CancellationToken::new(),
    );
    manager.address(id)
}

/// A daemon restarted under another spelling of its state directory must reach the
/// supervisors an earlier daemon started, or it records a live Process as lost.
#[test]
fn every_spelling_of_one_state_directory_names_one_supervisor_pipe()
-> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("ma2a-sup-name-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    let state = base.join("State");
    std::fs::create_dir_all(&state)?;
    let spelled = state.to_string_lossy().into_owned();
    let id = ProcessId::from_bytes([0x42; 16]);

    let named = address(&state, id);
    for alias in [
        spelled.to_lowercase(),
        spelled.to_uppercase(),
        format!("{spelled}\\"),
        spelled.replace('\\', "/"),
    ] {
        assert_eq!(address(Path::new(&alias), id), named, "{alias}");
    }
    std::fs::remove_dir_all(&base)?;
    Ok(())
}
