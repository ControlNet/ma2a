use std::{
    error::Error,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey, endpoint::presets};
use ma2a_core::{
    ControlPageV1, ControlResponseV1, EndpointId, SignedSpaceAddressRecordV1, SpaceId,
};
use ma2a_net::{
    CONTROL_ALPN, EndpointBindOptions, EndpointSecret, RuntimeEndpoint, SpaceAddressLookup,
};
use ma2a_store::{Repository, StoreConfig};
use tokio::sync::mpsc;

pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempState(pub(super) PathBuf);

impl TempState {
    pub(super) fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-inbound-post-apply-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test inbound post-apply state directory")?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn endpoint_hex(id: EndpointId) -> String {
    id.as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

pub(super) fn sync_peers(value: &serde_json::Value) -> TestResult<Vec<String>> {
    let items = value
        .pointer("/control_sync/peer_endpoint_ids")
        .and_then(serde_json::Value::as_array)
        .ok_or("missing control_sync.peer_endpoint_ids")?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "non-string peer endpoint id".into())
        })
        .collect()
}

pub(super) fn now_ms() -> TestResult<u64> {
    Ok(u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?)
}

pub(super) fn endpoint_addr_from_repo(
    cfg: &StoreConfig,
    space_id: SpaceId,
    endpoint_id: EndpointId,
) -> TestResult<EndpointAddr> {
    let repo = Repository::open(cfg)?;
    let record = repo
        .address_record(space_id, endpoint_id)?
        .ok_or("missing address record")?;
    let signed = SignedSpaceAddressRecordV1::parse_canonical_bytes(record.signed_record())?;
    let addrs = signed.record().endpoint_data().addresses().iter().cloned();
    Ok(EndpointAddr::from_parts(
        endpoint_id.to_public_key()?,
        addrs,
    ))
}

pub(super) async fn send_control_raw(
    sender_key: &SecretKey,
    target_addr: EndpointAddr,
    payload: &[u8],
) -> TestResult<(u8, Vec<u8>)> {
    let sender = Endpoint::builder(presets::Minimal)
        .secret_key(sender_key.clone())
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind()
        .await?;
    let conn = sender.connect(target_addr, CONTROL_ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(payload).await?;
    send.finish()?;
    let mut status = [0_u8; 1];
    recv.read_exact(&mut status).await?;
    let mut len = [0_u8; 4];
    recv.read_exact(&mut len).await?;
    let len = usize::try_from(u32::from_be_bytes(len))?;
    let mut body = vec![0_u8; len];
    recv.read_exact(&mut body).await?;
    conn.close(0_u8.into(), b"");
    sender.close().await;
    Ok((status[0], body))
}

pub(super) async fn bind_peer_a(
    peer_a_key: &SecretKey,
    space_id: SpaceId,
) -> TestResult<(RuntimeEndpoint, tokio::task::JoinHandle<()>)> {
    let (enroll_tx, _enroll_rx) = mpsc::channel(1);
    let (ctrl_tx, mut ctrl_rx) = mpsc::channel(4);
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&peer_a_key.to_bytes())?,
        SpaceAddressLookup::default(),
        EndpointBindOptions::new(enroll_tx, None).with_control(ctrl_tx, true),
    )
    .await?;
    let responder = tokio::spawn(async move {
        while let Some(call) = ctrl_rx.recv().await {
            if let Some(auth) = call.authorize()
                && let Ok((_, responder)) = auth.request().await
                && let Ok(page) = ControlPageV1::new(space_id, false, Vec::new())
                && let Ok(bytes) =
                    ControlResponseV1::new(vec![page]).and_then(|response| response.encode())
            {
                responder.respond(Ok(bytes));
            }
        }
    });
    Ok((endpoint, responder))
}
