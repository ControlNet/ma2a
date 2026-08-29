use ed25519_dalek::{Signer as _, SigningKey};
use iroh_base::SecretKey;
use ma2a_core::{
    AuthorizationEndpoints, AuthorizationRequest, EndpointId, GENESIS_SIGNATURE_DOMAIN,
    MemberCapabilities, RemoteOperation, SignedSpaceGenesisV1, SpaceAuthoritySecret,
    SpaceAuthorizationView, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1,
    SpaceRevocationV1,
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug)]
pub(super) struct Endpoints {
    pub(super) caller: EndpointId,
    pub(super) target: EndpointId,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CapabilityGrants {
    echo: bool,
    relay: bool,
}

impl CapabilityGrants {
    pub(super) const NONE: Self = Self::new(false, false);
    pub(super) const ECHO: Self = Self::new(true, false);
    pub(super) const RELAY: Self = Self::new(false, true);

    const fn new(echo: bool, relay: bool) -> Self {
        Self { echo, relay }
    }

    const fn member_capabilities(self) -> MemberCapabilities {
        MemberCapabilities::new(self.echo, self.relay)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PolicyGrants {
    echo: bool,
    relay: bool,
}

impl PolicyGrants {
    pub(super) const NONE: Self = Self::new(false, false);
    pub(super) const ECHO: Self = Self::new(true, false);
    pub(super) const RELAY: Self = Self::new(false, true);

    const fn new(echo: bool, relay: bool) -> Self {
        Self { echo, relay }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct AccessSpec {
    pub(super) caller: CapabilityGrants,
    pub(super) target: CapabilityGrants,
    pub(super) policy: PolicyGrants,
}

impl AccessSpec {
    pub(super) const fn new(
        caller: CapabilityGrants,
        target: CapabilityGrants,
        policy: PolicyGrants,
    ) -> Self {
        Self {
            caller,
            target,
            policy,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Revocation {
    None,
    Caller,
    Target,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SpaceSpec {
    authority: u8,
    access: AccessSpec,
    revocation: Revocation,
}

pub(super) const fn spec(authority: u8, access: AccessSpec, revocation: Revocation) -> SpaceSpec {
    SpaceSpec {
        authority,
        access,
        revocation,
    }
}

pub(super) fn endpoints() -> Endpoints {
    Endpoints {
        caller: SecretKey::from_bytes(&[0x21; 32]).public().into(),
        target: SecretKey::from_bytes(&[0x22; 32]).public().into(),
    }
}

pub(super) const fn request(
    endpoints: Endpoints,
    operation: RemoteOperation,
) -> AuthorizationRequest {
    AuthorizationRequest::new(
        AuthorizationEndpoints::new(endpoints.caller, endpoints.target),
        operation,
        None,
    )
}

pub(super) fn view(
    endpoints: Endpoints,
    specification: SpaceSpec,
) -> TestResult<SpaceAuthorizationView> {
    let authority = SpaceAuthoritySecret::from_bytes([specification.authority; 32]);
    let target = member(
        endpoints.target,
        "target",
        specification.access.target.member_capabilities(),
    )?;
    let genesis = signed_genesis(&authority, target.clone(), specification.access.policy)?;
    let caller = member(
        endpoints.caller,
        "caller",
        specification.access.caller.member_capabilities(),
    )?;
    let mut members = vec![caller.clone(), target.clone()];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let first = SpaceManifestV1::new(
        SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
        2,
        SpaceManifestMembership::new(members, Vec::new()),
    )?
    .sign(&authority)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    chain.apply(&first)?;
    let revocation = match specification.revocation {
        Revocation::None => None,
        Revocation::Caller => Some((vec![target], endpoints.caller)),
        Revocation::Target => Some((vec![caller], endpoints.target)),
    };
    if let Some((current_members, revoked_endpoint)) = revocation {
        let revoked = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
            3,
            SpaceManifestMembership::new(
                current_members,
                vec![SpaceRevocationV1::new(revoked_endpoint)],
            ),
        )?
        .sign(&authority)?;
        chain.apply(&revoked)?;
    }
    Ok(SpaceAuthorizationView::from_chain(&chain))
}

fn signed_genesis(
    authority: &SpaceAuthoritySecret,
    target: SpaceMemberV1,
    policy: PolicyGrants,
) -> TestResult<SignedSpaceGenesisV1> {
    let default = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new(
            [authority.public_key().as_bytes()[0].wrapping_add(1); 32],
            1,
            authority.public_key(),
        )?,
        SpaceGenesisOwner::new(target, SpacePolicyV1::phase_one_default()),
    )
    .sign(authority)?;
    explicit_policy_genesis(&default, authority, policy)
}

fn explicit_policy_genesis(
    default: &SignedSpaceGenesisV1,
    authority: &SpaceAuthoritySecret,
    policy: PolicyGrants,
) -> TestResult<SignedSpaceGenesisV1> {
    const POLICY_SUFFIX: [u8; 8] = [0xa3, 0x00, 0xf5, 0x01, 0xf5, 0x02, 0x18, 0x40];
    let mut body = default.canonical_body_bytes().to_vec();
    let policy_offset = body
        .windows(POLICY_SUFFIX.len())
        .rposition(|window| window == POLICY_SUFFIX)
        .ok_or("canonical policy suffix missing")?;
    *body
        .get_mut(policy_offset + 2)
        .ok_or("canonical Echo policy missing")? = cbor_bool(policy.echo);
    *body
        .get_mut(policy_offset + 4)
        .ok_or("canonical relay policy missing")? = cbor_bool(policy.relay);

    let mut message = Vec::with_capacity(GENESIS_SIGNATURE_DOMAIN.len() + body.len());
    message.extend_from_slice(GENESIS_SIGNATURE_DOMAIN);
    message.extend_from_slice(&body);
    let signing_key = SigningKey::from_bytes(&authority.secret_bytes());
    let signature = signing_key.sign(&message).to_bytes();

    let mut encoded = default.canonical_bytes().to_vec();
    let body_offset = encoded
        .windows(default.canonical_body_bytes().len())
        .position(|window| window == default.canonical_body_bytes())
        .ok_or("canonical genesis body missing")?;
    let body_end = body_offset
        .checked_add(body.len())
        .ok_or("canonical body range overflow")?;
    encoded
        .get_mut(body_offset..body_end)
        .ok_or("canonical body range missing")?
        .copy_from_slice(&body);
    let signature_offset = encoded
        .len()
        .checked_sub(signature.len())
        .ok_or("canonical signature missing")?;
    encoded
        .get_mut(signature_offset..)
        .ok_or("canonical signature range missing")?
        .copy_from_slice(&signature);
    Ok(SignedSpaceGenesisV1::from_canonical_bytes(&encoded)?)
}

fn member(
    endpoint_id: EndpointId,
    label: &str,
    capabilities: MemberCapabilities,
) -> TestResult<SpaceMemberV1> {
    Ok(SpaceMemberV1::new(
        endpoint_id,
        label.to_owned(),
        capabilities,
    )?)
}

const fn cbor_bool(value: bool) -> u8 {
    if value { 0xf5 } else { 0xf4 }
}
