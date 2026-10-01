//! Service v1 codecs: manifests, descriptors, requests, versions, and the built-in catalog.

use ma2a_core::{
    EndpointId, RequestId, ServiceKind,
    service::{
        DescribePage, HostedCall, Interaction, MAX_CALL_BODY_LEN, MAX_DESCRIBE_PAGE,
        MAX_MANIFEST_LEN, MAX_OUTCOME_FRAME_LEN, MAX_SERVICE_DESCRIPTION_LEN, MAX_SERVICE_VERSIONS,
        MAX_VERSION_OPERATIONS, OpenBody, OpenHeader, OperationName, ServiceAccess,
        ServiceDescriptor, ServiceError, ServiceManifest, ServiceName, ServiceOrigin,
        ServiceRequest, ServiceVersion, admission, builtin_descriptor, select_version, sorts_after,
        stream_limits,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn operation(name: &str, interaction: Interaction) -> TestResult<(OperationName, Interaction)> {
    Ok((OperationName::new(name)?, interaction))
}

fn version(number: u16, names: &[&str]) -> TestResult<ServiceVersion> {
    let operations = names
        .iter()
        .map(|name| operation(name, Interaction::Unary))
        .collect::<TestResult<Vec<_>>>()?;
    Ok(ServiceVersion::new(number, operations)?)
}

/// The largest manifest the bounds allow: every name and list at its maximum.
fn largest_manifest() -> TestResult<ServiceManifest> {
    let versions = (1..=u16::try_from(MAX_SERVICE_VERSIONS)?)
        .map(|number| {
            let operations = (0..MAX_VERSION_OPERATIONS / 2)
                .map(|index| {
                    let name = format!("o{index:02}{}", "x".repeat(29));
                    operation(&name, Interaction::Resource)
                })
                .collect::<TestResult<Vec<_>>>()?;
            Ok(ServiceVersion::new(number * 1_000, operations)?)
        })
        .collect::<TestResult<Vec<_>>>()?;
    Ok(ServiceManifest::new(
        &"d".repeat(MAX_SERVICE_DESCRIPTION_LEN),
        versions,
        64,
    )?)
}

#[test]
fn manifests_roundtrip_and_enforce_their_bounds() -> TestResult {
    let manifest = ServiceManifest::new(
        "Example service.",
        vec![
            version(1, &["echo", "upper"])?,
            version(2, &["echo", "count"])?,
        ],
        4,
    )?;
    let bytes = manifest.to_bytes()?;
    assert_eq!(ServiceManifest::from_bytes(&bytes)?, manifest);
    assert_eq!(
        manifest.version(2).and_then(|v| v.operation("count")),
        Some(Interaction::Unary)
    );
    assert!(manifest.version(3).is_none());

    let largest = largest_manifest()?;
    assert!(largest.to_bytes()?.len() <= MAX_MANIFEST_LEN);

    let one = || version(1, &["echo"]);
    assert!(ServiceManifest::new("x", vec![], 1).is_err());
    assert!(
        ServiceManifest::new("x", vec![one()?, one()?], 1).is_err(),
        "not ascending"
    );
    assert!(ServiceManifest::new("x", vec![version(2, &["a"])?, one()?], 1).is_err());
    assert!(ServiceManifest::new("x", vec![one()?], 0).is_err());
    assert!(ServiceManifest::new("x", vec![one()?], 65).is_err());
    assert!(ServiceManifest::new("a\0b", vec![one()?], 1).is_err());
    assert!(ServiceManifest::new(&"d".repeat(257), vec![one()?], 1).is_err());
    assert!(ServiceVersion::new(0, vec![operation("a", Interaction::Unary)?]).is_err());
    assert!(ServiceVersion::new(1, vec![]).is_err());
    let duplicate = vec![
        operation("a", Interaction::Unary)?,
        operation("a", Interaction::Bytes)?,
    ];
    assert!(ServiceVersion::new(1, duplicate).is_err());
    let nine = (1..=9)
        .map(|n| version(n, &["a"]))
        .collect::<TestResult<Vec<_>>>()?;
    assert!(ServiceManifest::new("x", nine, 1).is_err());
    // A manifest's canonical bytes refuse trailing input.
    let mut trailing = bytes;
    trailing.push(0);
    assert!(ServiceManifest::from_bytes(&trailing).is_err());
    Ok(())
}

#[test]
fn a_full_describe_page_fits_one_outcome_frame_and_roundtrips() -> TestResult {
    let largest = largest_manifest()?;
    let descriptors = (0..MAX_DESCRIBE_PAGE)
        .map(|index| {
            let name = ServiceName::new(&format!("s{index:02}{}", "n".repeat(60)))?;
            Ok(ServiceDescriptor::hosted(name, &largest, index % 2 == 0))
        })
        .collect::<TestResult<Vec<_>>>()?;
    let page = DescribePage::new(descriptors, Some("s15".to_owned()));
    let bytes = DescribePage::encode_outcome(Ok(&page))?;
    assert!(bytes.len() <= MAX_OUTCOME_FRAME_LEN, "{}", bytes.len());
    assert_eq!(DescribePage::decode_outcome(&bytes)??, page);

    let builtin = DescribePage::new(vec![builtin_descriptor(ServiceKind::Files)?], None);
    let bytes = DescribePage::encode_outcome(Ok(&builtin))?;
    assert_eq!(DescribePage::decode_outcome(&bytes)??, builtin);
    let refused = DescribePage::encode_outcome(Err(ServiceError::Unauthorized))?;
    assert_eq!(
        DescribePage::decode_outcome(&refused)?,
        Err(ServiceError::Unauthorized)
    );
    Ok(())
}

#[test]
fn service_requests_roundtrip_in_the_open_header() -> TestResult {
    let target: EndpointId = iroh_base::SecretKey::from_bytes(&[7_u8; 32])
        .public()
        .into();
    let request_id = RequestId::try_from([9_u8; 16].as_slice())?;
    let call = HostedCall::new(
        (ServiceName::new("example")?, 2),
        OperationName::new("count")?,
        b"3".to_vec(),
    )?;
    for request in [
        ServiceRequest::Describe(None),
        ServiceRequest::Describe(Some("files".to_owned())),
        ServiceRequest::Call(call.clone()),
    ] {
        let header = OpenHeader::new(request_id, target, OpenBody::Service(request.clone()));
        let decoded = OpenHeader::decode(&header.encode()?)?;
        assert_eq!(decoded.body(), &OpenBody::Service(request.clone()));
        assert_eq!(decoded.body().kind(), ServiceKind::Service);
    }
    let body = OpenBody::Service(ServiceRequest::Call(call));
    assert!(body.is_mutation());
    assert_eq!(
        body.access(),
        ServiceAccess::Hosted(ServiceName::new("example")?)
    );
    assert!(!OpenBody::Service(ServiceRequest::Describe(None)).is_mutation());
    assert_eq!(
        OpenBody::Service(ServiceRequest::Describe(None)).access(),
        ServiceAccess::Discover
    );
    let name = || ServiceName::new("example");
    let echo = || OperationName::new("echo");
    assert!(HostedCall::new((name()?, 0), echo()?, Vec::new()).is_err());
    assert!(HostedCall::new((name()?, 1), echo()?, vec![0; MAX_CALL_BODY_LEN + 1]).is_err());
    assert!(HostedCall::new((name()?, 1), echo()?, vec![0; MAX_CALL_BODY_LEN]).is_ok());
    Ok(())
}

#[test]
fn version_selection_is_the_highest_version_both_sides_speak() -> TestResult {
    let manifest = ServiceManifest::new(
        "x",
        vec![
            version(1, &["a"])?,
            version(2, &["a"])?,
            version(5, &["a"])?,
        ],
        1,
    )?;
    let descriptor = ServiceDescriptor::hosted(ServiceName::new("example")?, &manifest, true);
    assert_eq!(select_version(&[1, 2, 3], &descriptor), Some(2));
    assert_eq!(select_version(&[5, 1], &descriptor), Some(5));
    assert_eq!(select_version(&[3, 4], &descriptor), None);
    assert_eq!(select_version(&[], &descriptor), None);
    Ok(())
}

#[test]
fn pages_order_built_ins_first_then_hosted_names() -> TestResult {
    let files = ServiceOrigin::Builtin(ServiceKind::Files);
    let agent = ServiceOrigin::Builtin(ServiceKind::Agent);
    let alpha = ServiceOrigin::Hosted(ServiceName::new("alpha")?);
    let beta = ServiceOrigin::Hosted(ServiceName::new("beta")?);
    assert!(sorts_after(&files, None));
    assert!(sorts_after(&agent, Some("files")));
    assert!(!sorts_after(&files, Some("agent")));
    assert!(sorts_after(&alpha, Some("agent")));
    assert!(!sorts_after(&agent, Some("alpha")));
    assert!(sorts_after(&beta, Some("alpha")));
    assert!(!sorts_after(&alpha, Some("alpha")));
    Ok(())
}

#[test]
fn every_built_in_family_has_a_valid_catalog_entry() -> TestResult {
    for kind in ServiceKind::ALL {
        let descriptor = builtin_descriptor(kind)?;
        assert_eq!(descriptor.origin(), &ServiceOrigin::Builtin(kind));
        assert_eq!(descriptor.versions().len(), 1);
        assert!(descriptor.available());
        let (per_peer, global) = stream_limits(kind);
        assert!(per_peer >= 1 && per_peer <= global, "{kind:?}");
        assert_eq!(admission(kind).is_none(), kind == ServiceKind::Echo);
        // A built-in name is never a hosted service name.
        assert!(ServiceName::new(kind.name()).is_err());
    }
    assert_eq!(
        admission(ServiceKind::Service),
        Some(ServiceAccess::Discover)
    );
    Ok(())
}

#[test]
fn hosted_accept_and_outcome_payloads_are_bounded_byte_strings() -> TestResult {
    use ma2a_core::service::{
        MAX_CALL_ACCEPT_LEN, MAX_CALL_RESULT_LEN, decode_call_accept, decode_call_outcome,
        encode_call_accept, encode_call_outcome, outcome_status,
    };
    for header in [&b""[..], b"v2", &[7; MAX_CALL_ACCEPT_LEN]] {
        assert_eq!(decode_call_accept(&encode_call_accept(header)?)?, header);
    }
    assert!(encode_call_accept(&[0; MAX_CALL_ACCEPT_LEN + 1]).is_err());
    // A text string, trailing bytes, and a non-minimal length are all refused.
    assert!(decode_call_accept(&[0x60]).is_err());
    assert!(decode_call_accept(&[0x40, 0]).is_err());
    assert!(decode_call_accept(&[0x58, 1, 9]).is_err());

    let largest = vec![1; MAX_CALL_RESULT_LEN];
    let encoded = encode_call_outcome(Ok(&largest))?;
    assert!(encoded.len() <= MAX_OUTCOME_FRAME_LEN);
    assert_eq!(outcome_status(&encoded)?, None);
    assert_eq!(decode_call_outcome(&encoded)?, Ok(largest));
    assert!(encode_call_outcome(Ok(&vec![0; MAX_CALL_RESULT_LEN + 1])).is_err());
    let failed = encode_call_outcome(Err(ServiceError::Cancelled))?;
    assert_eq!(outcome_status(&failed)?, Some(ServiceError::Cancelled));
    assert_eq!(decode_call_outcome(&failed)?, Err(ServiceError::Cancelled));
    Ok(())
}
