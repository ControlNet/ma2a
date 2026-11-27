use std::{
    error::Error,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_core::{EndpointId, InviteEntropy, RequestId, SpaceId, default_member_label};
use ma2a_store::StoreConfig;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use zeroize::Zeroizing;

use crate::{
    EnrollmentAttempt, EnrollmentCreation, Runtime,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{
        PasswordAction, SystemClock, WebAssets, WebAuthConfig, WebRuntimeDependencies,
        WebServerConfig, build_runtime_router,
    },
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempState(pub(super) PathBuf);

impl TempState {
    pub(super) fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-inbound-view-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test inbound view state directory")?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

pub(super) struct MemberRouter {
    _state: TempState,
    pub(super) runtime: Runtime,
    _control: CurrentUserRuntime,
    cancel: CancellationToken,
    server: tokio::task::JoinHandle<Result<crate::ipc::ServerExit, crate::ipc::IpcError>>,
    paths: IpcPaths,
    router: Router,
    cookie: String,
}

impl MemberRouter {
    pub(super) async fn start(label: &str) -> TestResult<Self> {
        let state = TempState::new(label)?;
        let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
        let control = CurrentUserRuntime::open_at(
            &state.0,
            Arc::new(SystemClock::default()),
            WebAuthConfig::default(),
        )
        .await?;
        control
            .web_auth()
            .change_password(
                PasswordAction::Set,
                Zeroizing::new("test-only-inbound-view-8!".to_owned()),
            )
            .await?;
        let session = control
            .web_auth()
            .login(Zeroizing::new("test-only-inbound-view-8!".to_owned()))
            .await?;
        let cookie = format!("ma2a_session={}", session.bearer());
        let paths = IpcPaths::new(&state.0)?;
        let server =
            LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control.clone(), None)
                .await?;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(server.serve(cancel.child_token()));
        let router = build_runtime_router(
            WebRuntimeDependencies::new(
                control.web_auth().clone(),
                WebAssets::new(&[("index.html", b"ok")]),
                LocalApiClient::new(paths.clone()),
            ),
            WebServerConfig::default(),
            43_213,
        )
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            12_350,
        ))));
        Ok(Self {
            _state: state,
            runtime,
            _control: control,
            cancel,
            server: task,
            paths,
            router,
            cookie,
        })
    }

    pub(super) async fn fetch_snapshot(&self) -> TestResult<(String, serde_json::Value)> {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/snapshot")
                    .header("host", "127.0.0.1:43213")
                    .header("cookie", &self.cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let boot_id = response
            .headers()
            .get("x-ma2a-runtime-boot-id")
            .ok_or("missing boot id header")?
            .to_str()?
            .to_owned();
        let payload = serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
        Ok((boot_id, payload))
    }

    pub(super) async fn open_sse(
        &self,
        boot_id: &str,
        (since, view_sequence): (u64, u64),
    ) -> TestResult<axum::body::BodyDataStream> {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/events?since={since}&view_sequence={view_sequence}&boot_id={boot_id}"
                    ))
                    .header("host", "127.0.0.1:43213")
                    .header("cookie", &self.cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        Ok(response.into_body().into_data_stream())
    }

    pub(super) async fn shutdown(self) -> TestResult {
        self.cancel.cancel();
        self.server.await??;
        self.paths.remove_stale_endpoint()?;
        self.runtime.shutdown().await?;
        Ok(())
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

pub(super) fn json_u64(value: &serde_json::Value, key: &str) -> TestResult<u64> {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("missing {key}").into())
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

pub(super) async fn enroll_pair(
    owner: &Runtime,
    member: &Runtime,
    request_seed: u8,
) -> TestResult<(SpaceId, ma2a_core::SignedInviteTicket, RequestId)> {
    let space_id = owner
        .handle()
        .create_owned_space("stale-inbound-view".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    let member_id = member.handle().status().await?.endpoint_id();
    let request_id = RequestId::try_from([request_seed; 16].as_slice())?;
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket.clone(),
            request_id,
            default_member_label(member_id),
        ))
        .await?;
    member.handle().sync_control().await?;
    owner.handle().sync_control().await?;
    Ok((space_id, ticket, request_id))
}
