//! A round splits a peer's shared Spaces across requests that each fit one frame.

use std::{error::Error, path::PathBuf};

use iroh::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlRequestV1, EndpointId,
    MAX_CONTROL_SPACES_PER_REQUEST, MemberCapabilities, SpaceAddressRecordV1,
    SpaceAuthorizationView, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{
    AddressRecordTarget, AddressRecordValidation, OwnedSpaceUpdate, Repository, SpaceCreation,
    StoreConfig, ValidatedAddressRecord,
};

use super::{ControlRoundRequest, ControlRoundScope, prepare_round};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let path =
            std::env::temp_dir().join(format!("ma2a-control-round-{label}-{}", std::process::id()));
        std::fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

fn member(endpoint: EndpointId, name: &str) -> TestResult<SpaceMemberV1> {
    Ok(SpaceMemberV1::new(
        endpoint,
        name.to_owned(),
        MemberCapabilities::new(true, false),
    )?)
}

/// Creates one Space shared by `local` and `peer`, holding a fresh address record of `peer`.
fn share_space(repository: &mut Repository, local: &SpaceMemberV1, peer: &SecretKey) -> TestResult {
    let peer_id = EndpointId::from(peer.public());
    let created = repository.create_owned_space(&SpaceCreation::new(
        10,
        local.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![local.clone(), member(peer_id, "peer")?];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let advanced = repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        11,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    let signed = SpaceAddressRecordV1::new(
        AddressRecordScope::new(created.space_id(), peer_id),
        AddressRecordValidity::new(1, 12, 100)?,
        AddressEndpointDataV1::new(Vec::new())?,
    )
    .sign(peer)?;
    let validated = ValidatedAddressRecord::parse(
        signed.canonical_bytes(),
        AddressRecordValidation::new(
            AddressRecordTarget::new(created.space_id(), peer_id),
            &SpaceAuthorizationView::from_chain(advanced.chain()),
            50,
        ),
    )?;
    repository.advance_validated_address(&validated)?;
    Ok(())
}

#[test]
fn a_peer_sharing_more_spaces_than_one_frame_gets_several_requests() -> TestResult {
    // Given: 65 Spaces shared with one peer, and one Space shared with another.
    let state = TempState::new("split")?;
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x11; 32]).public());
    let local = member(local_id, "local")?;
    let (busy, quiet) = (
        SecretKey::from_bytes(&[0x22; 32]),
        SecretKey::from_bytes(&[0x33; 32]),
    );
    for _space in 0..=MAX_CONTROL_SPACES_PER_REQUEST {
        share_space(&mut repository, &local, &busy)?;
    }
    share_space(&mut repository, &local, &quiet)?;

    // When
    let round = prepare_round(
        &repository,
        &ControlRoundRequest {
            local_endpoint_id: local_id,
            rotation: 0,
            now_ms: 50,
            scope: ControlRoundScope::all(),
        },
    )?;

    // Then: every request decodes, the busy peer's Spaces are all covered, and the quiet peer
    // is not queued behind the busy peer's second request.
    let mut covered = Vec::new();
    for prepared in &round {
        let request = ControlRequestV1::decode(&prepared.request)?;
        covered.push((prepared.peer, request.cursors().len()));
    }
    let (busy_id, quiet_id) = (
        EndpointId::from(busy.public()),
        EndpointId::from(quiet.public()),
    );
    let busy_spaces: usize = covered
        .iter()
        .filter(|(peer, _)| *peer == busy_id)
        .map(|(_, count)| count)
        .sum();
    assert_eq!(busy_spaces, MAX_CONTROL_SPACES_PER_REQUEST + 1);
    assert_eq!(covered.len(), 3, "{covered:?}");
    assert!(
        covered
            .iter()
            .take(2)
            .any(|(peer, count)| *peer == quiet_id && *count == 1),
        "{covered:?}"
    );
    Ok(())
}

#[test]
fn a_requester_ahead_of_this_chain_gets_an_empty_page_not_a_rejection() -> TestResult {
    // Given: a shared Space at generation 1 here, and a requester that already holds generation 5,
    // as after this Runtime committed only the prefix the requester pushed.
    let state = TempState::new("ahead")?;
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x11; 32]).public());
    let local = member(local_id, "local")?;
    share_space(&mut repository, &local, &SecretKey::from_bytes(&[0x22; 32]))?;
    let spaces = repository.control_spaces_for(local_id)?;
    let shared = spaces.first().ok_or("no shared Space")?;
    let ahead = ma2a_core::ControlCursorV1::new(shared.chain().space_id(), 5, [0x5a; 32]);

    // When
    let page = super::pages::pull_page(
        shared,
        &ahead,
        super::pages::PullPageRequest {
            now_ms: 50,
            artifact_budget: 16_000,
        },
    );

    // Then: nothing to send, and the exchange stands.
    let page = page.map_err(|rejection| format!("{rejection:?}"))?;
    assert!(page.artifacts().is_empty());
    assert!(!page.has_more());

    // A cursor at a generation this chain holds with another hash is still a fork.
    let forked = ma2a_core::ControlCursorV1::new(shared.chain().space_id(), 1, [0x5a; 32]);
    let rejected = super::pages::pull_page(
        shared,
        &forked,
        super::pages::PullPageRequest {
            now_ms: 50,
            artifact_budget: 16_000,
        },
    );
    assert!(rejected.is_err());
    Ok(())
}

#[test]
fn a_response_answering_another_requests_spaces_is_refused() -> TestResult {
    // Given: one peer whose shared Spaces take two requests.
    let state = TempState::new("shards")?;
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x11; 32]).public());
    let local = member(local_id, "local")?;
    let peer = SecretKey::from_bytes(&[0x22; 32]);
    for _space in 0..=MAX_CONTROL_SPACES_PER_REQUEST {
        share_space(&mut repository, &local, &peer)?;
    }
    let round = prepare_round(
        &repository,
        &ControlRoundRequest {
            local_endpoint_id: local_id,
            rotation: 0,
            now_ms: 50,
            scope: ControlRoundScope::all(),
        },
    )?;
    let [first, second] = round.as_slice() else {
        return Err(format!("expected two requests, got {}", round.len()).into());
    };
    // A legitimate answer to the first request: one page, with no change, per Space it asked.
    let pages = first
        .spaces
        .iter()
        .map(|space| ma2a_core::ControlPageV1::new(*space, false, Vec::new()))
        .collect::<Result<Vec<_>, _>>()?;
    let payload = ma2a_core::ControlResponseV1::new(pages)?.encode()?;
    let input = super::ControlExchangeInput {
        local_endpoint_id: local_id,
        remote_endpoint_id: EndpointId::from(peer.public()),
        payload,
        now_ms: 50,
    };

    // When / Then: as the answer to the second request it is refused, and changes nothing.
    let revision = repository.revision()?;
    let refused = super::apply_response(&mut repository, &input, &second.spaces);
    assert!(matches!(
        refused,
        Err(super::ControlFailure::Rejected(
            ma2a_net::ControlRejection::Invalid
        ))
    ));
    assert_eq!(repository.revision()?, revision);
    // As the answer to the first request, the same pages complete a round with no change.
    super::apply_response(&mut repository, &input, &first.spaces)
        .map_err(|failure| failure.to_string())?;
    Ok(())
}
