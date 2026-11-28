use iroh::{EndpointAddr, SecretKey};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactKind,
    ControlArtifactV1, ControlCursorV1, ControlPageV1, ControlRequestV1, EndpointId, InviteEntropy,
    MemberCapabilities, RequestId, SpaceAddressRecordV1, SpaceId, SpaceManifestMembership,
    SpaceMemberV1, SpaceRevocationV1, default_member_label,
};
use ma2a_net::{EndpointSecret, RuntimeEndpoint};
use ma2a_store::{OwnedSpaceUpdate, Repository, StoreConfig};
use tokio::sync::oneshot;

use super::support::{
    TempState, TestResult, bind_peer_a, endpoint_addr_from_repo, endpoint_hex, now_ms,
    send_control_raw, sync_peers,
};
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime};

pub(super) struct Setup {
    _owner_dir: TempState,
    _provider_dir: TempState,
    pub(super) owner: Runtime,
    pub(super) provider: Runtime,
    pub(super) space_id: SpaceId,
    pub(super) provider_id: EndpointId,
    pub(super) provider_addr: EndpointAddr,
    pub(super) peer_a_key: SecretKey,
    pub(super) peer_a_id: EndpointId,
    pub(super) third_id: EndpointId,
    pub(super) owner_cfg: StoreConfig,
    pub(super) peer_a_endpoint: RuntimeEndpoint,
    pub(super) peer_a_responder: tokio::task::JoinHandle<()>,
}

async fn push_initial_address(
    (provider_cfg, provider_addr, space_id): (&StoreConfig, EndpointAddr, SpaceId),
    (peer_a_key, peer_a_endpoint): (&SecretKey, &RuntimeEndpoint),
) -> TestResult {
    let sender_id = EndpointId::from(peer_a_key.public());
    let chain_g1 = Repository::open(provider_cfg)?
        .load_space_chain(space_id)?
        .ok_or("missing space chain on provider")?;
    let peer_a_data = peer_a_endpoint.endpoint_data()?;
    let t = now_ms()?;
    let signed_addr = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, sender_id),
        AddressRecordValidity::new(1, t.saturating_sub(1_000), t + 300_000)?,
        AddressEndpointDataV1::from_parts(
            peer_a_data.addresses().to_vec(),
            peer_a_data.user_data().map(str::to_owned),
        )?,
    )
    .sign(peer_a_key)?;
    let init_page = ControlPageV1::new(
        space_id,
        false,
        vec![ControlArtifactV1::new(
            ControlArtifactKind::ADDRESS_RECORD,
            signed_addr.canonical_bytes().to_vec(),
        )?],
    )?;
    let init_cursor = ControlCursorV1::new(
        space_id,
        chain_g1.latest_generation(),
        chain_g1.latest_hash(),
    );
    let init_payload = ControlRequestV1::new(vec![init_cursor])?
        .with_push_pages(vec![init_page])?
        .encode()?;
    let (status, _) = send_control_raw(peer_a_key, provider_addr, &init_payload).await?;
    assert_eq!(status, 0);
    Ok(())
}

pub(super) async fn setup_scenario(label: &str, seed: u8) -> TestResult<Setup> {
    let owner_dir = TempState::new(&format!("{label}-owner"))?;
    let provider_dir = TempState::new(&format!("{label}-provider"))?;
    let owner_cfg = StoreConfig::new(&owner_dir.0);
    let provider_cfg = StoreConfig::new(&provider_dir.0);
    let owner = Runtime::start(owner_cfg.clone()).await?;
    let provider = Runtime::start(provider_cfg.clone()).await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    let provider_id = provider.handle().status().await?.endpoint_id();

    let space_id = owner
        .handle()
        .create_owned_space(format!("post-apply-{label}"))
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_id,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    provider
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([seed; 16].as_slice())?,
            default_member_label(provider_id),
        ))
        .await?;

    let peer_a_key = SecretKey::from_bytes(&[seed.wrapping_add(1); 32]);
    let sender_id = EndpointId::from(peer_a_key.public());
    let third_id = EndpointSecret::parse(&[seed.wrapping_add(2); 32])?.endpoint_id();
    let caps = MemberCapabilities::new(true, true);
    let mut members = vec![
        SpaceMemberV1::new(owner_id, default_member_label(owner_id), caps)?,
        SpaceMemberV1::new(provider_id, default_member_label(provider_id), caps)?,
        SpaceMemberV1::new(sender_id, "peer-a".to_owned(), caps)?,
        SpaceMemberV1::new(third_id, "peer-c".to_owned(), caps)?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    owner
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            space_id,
            now_ms()?,
            SpaceManifestMembership::new(members, Vec::new()),
        ))
        .await?;

    provider.handle().sync_control().await?;
    provider.handle().publish_address().await?;
    let provider_addr = endpoint_addr_from_repo(&provider_cfg, space_id, provider_id)?;
    let (peer_a_endpoint, peer_a_responder) = bind_peer_a(&peer_a_key, space_id).await?;
    push_initial_address(
        (&provider_cfg, provider_addr.clone(), space_id),
        (&peer_a_key, &peer_a_endpoint),
    )
    .await?;

    provider.handle().sync_control_with(sender_id).await?;
    assert!(
        sync_peers(&provider.handle().snapshot().await?.to_value())?
            .contains(&endpoint_hex(sender_id))
    );

    Ok(Setup {
        _owner_dir: owner_dir,
        _provider_dir: provider_dir,
        owner,
        provider,
        space_id,
        provider_id,
        provider_addr,
        peer_a_key,
        peer_a_id: sender_id,
        third_id,
        owner_cfg,
        peer_a_endpoint,
        peer_a_responder,
    })
}

pub(super) async fn advance_and_build_push(
    s: &Setup,
    revoke_sender: bool,
    forked_cursor: bool,
) -> TestResult<(Vec<u8>, oneshot::Sender<()>, tokio::task::JoinHandle<()>)> {
    let (owner_entered, owner_release) = s.owner.handle().arm_control_round_gate();
    let owner_handle = s.owner.handle();
    let owner_blocker = tokio::spawn(async move {
        let _ = owner_handle.sync_control().await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), owner_entered).await??;
    let owner_id = s.owner.handle().status().await?.endpoint_id();
    let caps = MemberCapabilities::new(true, true);
    let (mut kept, revocations) = if revoke_sender {
        (
            vec![
                SpaceMemberV1::new(owner_id, default_member_label(owner_id), caps)?,
                SpaceMemberV1::new(s.provider_id, default_member_label(s.provider_id), caps)?,
            ],
            vec![
                SpaceRevocationV1::new(s.peer_a_id),
                SpaceRevocationV1::new(s.third_id),
            ],
        )
    } else {
        (
            vec![
                SpaceMemberV1::new(owner_id, default_member_label(owner_id), caps)?,
                SpaceMemberV1::new(s.provider_id, default_member_label(s.provider_id), caps)?,
                SpaceMemberV1::new(s.peer_a_id, "peer-a".to_owned(), caps)?,
            ],
            vec![SpaceRevocationV1::new(s.third_id)],
        )
    };
    kept.sort_by_key(SpaceMemberV1::endpoint_id);
    s.owner
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            s.space_id,
            now_ms()?,
            SpaceManifestMembership::new(kept, revocations),
        ))
        .await?;
    let chain_g2 = Repository::open(&s.owner_cfg)?
        .load_space_chain(s.space_id)?
        .ok_or("missing gen 2 chain on owner")?;
    let manifest_g2 = chain_g2
        .manifests()
        .last()
        .ok_or("missing gen 2 manifest")?;
    let page = ControlPageV1::new(
        s.space_id,
        false,
        vec![ControlArtifactV1::new(
            ControlArtifactKind::MANIFEST,
            manifest_g2.canonical_bytes().to_vec(),
        )?],
    )?;
    let cursor = if forked_cursor {
        ControlCursorV1::new(s.space_id, 1, [0x5a; 32])
    } else {
        ControlCursorV1::new(
            s.space_id,
            chain_g2.latest_generation(),
            chain_g2.latest_hash(),
        )
    };
    let payload = ControlRequestV1::new(vec![cursor])?
        .with_push_pages(vec![page])?
        .encode()?;
    Ok((payload, owner_release, owner_blocker))
}
