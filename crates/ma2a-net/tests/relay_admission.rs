//! Private relay current-membership admission coverage.

use std::{net::Ipv4Addr, time::Duration};

use iroh_base::RelayUrl;
use iroh_base::SecretKey;
use iroh_dns::dns::DnsResolver;
use iroh_relay::client::ClientBuilder;
use iroh_relay::server::{Access, AccessControl as _, ClientRequest};
use iroh_relay::tls::{CaTlsConfig, default_provider};
use ma2a_core::{
    MemberCapabilities, SpaceAuthoritySecret, SpaceAuthorizationView, SpaceChain,
    SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{
    PrivateRelayAccess, PrivateRelayProviderConfig, PrivateRelayProviderLocation,
    PrivateRelayServer, PrivateRelayTransport,
};
use n0_future::StreamExt as _;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn authorization(
    member: &SecretKey,
    marker: u8,
) -> Result<SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
    let authority = SpaceAuthoritySecret::from_bytes([marker; 32]);
    let member = SpaceMemberV1::new(
        member.public().into(),
        format!("member-{marker:02x}"),
        MemberCapabilities::new(true, false),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([marker.wrapping_add(1); 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(member, SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    Ok(SpaceAuthorizationView::from_chain(
        &SpaceChain::from_genesis(genesis)?,
    ))
}

fn request(endpoint: &SecretKey) -> Result<ClientRequest, http::Error> {
    let request = http::Request::builder()
        .uri("http://localhost/relay")
        .body(())?;
    let (parts, ()) = request.into_parts();
    Ok(ClientRequest::new(
        endpoint.public(),
        iroh_relay::http::ProtocolVersion::V2,
        parts,
    ))
}

#[tokio::test]
async fn current_member_is_admitted_and_nonmember_is_denied() -> TestResult {
    // Given
    let member = SecretKey::from_bytes(&[0x51; 32]);
    let attacker = SecretKey::from_bytes(&[0x52; 32]);
    let authorization = authorization(&member, 0x61)?;
    let access = PrivateRelayAccess::new(member.public().into(), &[authorization.space_id()]);
    access.replace_from_spaces(&[authorization]);

    // When
    let member_request = request(&member)?;
    let attacker_request = request(&attacker)?;
    let member_access = access.on_connect(&member_request).await;
    let attacker_access = access.on_connect(&attacker_request).await;

    // Then
    assert_eq!(member_access, Access::Allow);
    assert!(matches!(attacker_access, Access::Deny { .. }));
    Ok(())
}

#[tokio::test]
async fn absent_provider_authority_fails_closed_for_new_connections() -> TestResult {
    // Given
    let former_member = SecretKey::from_bytes(&[0x53; 32]);
    let current_member = SecretKey::from_bytes(&[0x54; 32]);
    let former_authorization = authorization(&former_member, 0x62)?;
    let access = PrivateRelayAccess::new(
        former_member.public().into(),
        &[former_authorization.space_id()],
    );
    access.replace_from_spaces(&[former_authorization]);

    // When
    access.replace_from_spaces(&[authorization(&current_member, 0x63)?]);
    let former_request = request(&former_member)?;
    let current_request = request(&current_member)?;
    let former_access = access.on_connect(&former_request).await;
    let current_access = access.on_connect(&current_request).await;

    // Then
    assert!(matches!(former_access, Access::Deny { .. }));
    assert!(matches!(current_access, Access::Deny { .. }));
    Ok(())
}

#[tokio::test]
async fn losing_one_of_two_served_spaces_preserves_the_existing_connection() -> TestResult {
    // Given
    let provider = SecretKey::from_bytes(&[0x57; 32]);
    let first = authorization(&provider, 0x65)?;
    let second = authorization(&provider, 0x66)?;
    let access = PrivateRelayAccess::new(
        provider.public().into(),
        &[first.space_id(), second.space_id()],
    );
    access.replace_from_spaces(&[first.clone(), second.clone()]);
    let connected = request(&provider)?;
    assert_eq!(access.on_connect(&connected).await, Access::Allow);

    // When
    let after_first_loss = access.replace_from_spaces(&[second]);
    let after_final_loss = access.replace_from_spaces(&[]);

    // Then
    assert!(after_first_loss.is_empty());
    assert_eq!(
        after_final_loss,
        vec![(provider.public(), connected.connection_id())]
    );
    Ok(())
}

#[tokio::test]
async fn disconnect_callback_prunes_only_the_matching_connection() -> TestResult {
    // Given
    let provider = SecretKey::from_bytes(&[0x58; 32]);
    let authorization = authorization(&provider, 0x67)?;
    let access = PrivateRelayAccess::new(provider.public().into(), &[authorization.space_id()]);
    access.replace_from_spaces(&[authorization]);
    let first = request(&provider)?;
    let second = request(&provider)?;
    assert_eq!(access.on_connect(&first).await, Access::Allow);
    assert_eq!(access.on_connect(&second).await, Access::Allow);

    // When
    access.on_disconnect(provider.public(), first.connection_id());
    let revoked = access.replace_from_spaces(&[]);

    // Then
    assert_eq!(revoked, vec![(provider.public(), second.connection_id())]);
    Ok(())
}

#[tokio::test]
async fn real_external_termination_backend_admits_member_and_rejects_nonmember() -> TestResult {
    // Given
    let member = SecretKey::from_bytes(&[0x55; 32]);
    let attacker = SecretKey::from_bytes(&[0x56; 32]);
    let authorization = authorization(&member, 0x64)?;
    let served_space = authorization.space_id();
    let access = PrivateRelayAccess::new(member.public().into(), &[served_space]);
    access.replace_from_spaces(&[authorization]);
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://relay.example.invalid".parse()?,
        ),
        vec![served_space],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let server = PrivateRelayServer::spawn(&config, access).await?;
    let relay_url: RelayUrl = format!("http://{}", server.listen_addr()).parse()?;
    let tls = CaTlsConfig::default().client_config(default_provider())?;

    // When
    let member_result = tokio::time::timeout(
        Duration::from_secs(5),
        ClientBuilder::new(relay_url.clone(), member, DnsResolver::new())
            .tls_client_config(tls.clone())
            .connect(),
    )
    .await?;
    let attacker_result = tokio::time::timeout(
        Duration::from_secs(5),
        ClientBuilder::new(relay_url, attacker, DnsResolver::new())
            .tls_client_config(tls)
            .connect(),
    )
    .await?;

    // Then
    let member_client = member_result?;
    assert!(attacker_result.is_err());
    drop(member_client);
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn losing_final_eligibility_disconnects_an_existing_relay_client() -> TestResult {
    // Given
    let provider = SecretKey::from_bytes(&[0x59; 32]);
    let authorization = authorization(&provider, 0x68)?;
    let served_space = authorization.space_id();
    let access = PrivateRelayAccess::new(provider.public().into(), &[served_space]);
    access.replace_from_spaces(&[authorization]);
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://relay.example.invalid".parse()?,
        ),
        vec![served_space],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let server = PrivateRelayServer::spawn(&config, access).await?;
    let relay_url: RelayUrl = format!("http://{}", server.listen_addr()).parse()?;
    let tls = CaTlsConfig::default().client_config(default_provider())?;
    let mut client = ClientBuilder::new(relay_url, provider, DnsResolver::new())
        .tls_client_config(tls)
        .connect()
        .await?;

    // When
    server.replace_from_spaces(&[]);
    let closed = tokio::time::timeout(Duration::from_secs(5), client.next()).await?;

    // Then
    assert!(closed.is_none() || closed.is_some_and(|message| message.is_err()));
    server.shutdown().await?;
    Ok(())
}
