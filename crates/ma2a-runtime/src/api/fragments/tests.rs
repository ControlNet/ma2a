use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn worst_case_fragment_and_complete_snapshot_are_lossless() -> TestResult {
    // Synthetic test bytes cover UTF-8 boundaries, escaping and large legal aggregates.
    let bytes = "🦀\\\"\n".repeat(30_000).into_bytes();
    let mut assembly = SnapshotAssembly::default();
    let fragments = SnapshotFragments::new(bytes.clone(), u64::MAX, "ff".repeat(16));
    for frame in fragments {
        let frame = frame?;
        // 32,768 hex characters plus a fixed header fits even maximum-width numbers.
        assert!(frame.len() < 33_000);
        assembly.push(&frame)?;
    }
    let (restored, revision, boot) = assembly.finish()?;
    assert_eq!(restored, bytes);
    assert_eq!(revision, u64::MAX);
    assert_eq!(boot, "ff".repeat(16));
    Ok(())
}

#[test]
fn missing_reordered_mixed_and_duplicate_fragments_fail_closed() -> TestResult {
    let frames = SnapshotFragments::new(vec![b'x'; 40_000], 7, "ab".repeat(16))
        .collect::<Result<Vec<_>, _>>()?;
    let first = frames.first().ok_or("missing first")?;
    let second = frames.get(1).ok_or("missing second")?;
    assert!(SnapshotAssembly::default().push(second).is_err());
    let mut partial = SnapshotAssembly::default();
    assert!(!partial.push(first)?);
    assert!(partial.finish().is_err());
    for field in ["revision", "runtime_boot_id", "index"] {
        let mut changed: serde_json::Value = serde_json::from_slice(second)?;
        *changed.get_mut(field).ok_or("missing fixture field")? = if field == "runtime_boot_id" {
            serde_json::json!("cd".repeat(16))
        } else {
            serde_json::json!(99)
        };
        let mut assembly = SnapshotAssembly::default();
        assembly.push(first)?;
        assert!(assembly.push(&serde_json::to_vec(&changed)?).is_err());
    }
    let mut duplicate = SnapshotAssembly::default();
    duplicate.push(first)?;
    assert!(duplicate.push(first).is_err());
    Ok(())
}

#[test]
fn largest_possible_fragment_header_stays_below_the_wire_budget() -> TestResult {
    let fragment = SnapshotFragment {
        kind: FragmentType::SnapshotFragment,
        revision: u64::MAX,
        runtime_boot_id: "ff".repeat(16),
        index: u64::MAX,
        last: false,
        data_hex: "ff".repeat(SNAPSHOT_FRAGMENT_BYTES),
    };
    assert!(serde_json::to_vec(&fragment)?.len() < 33_000);
    Ok(())
}

#[test]
fn serialized_fragment_fields_match_the_shared_schema() -> TestResult {
    let frame = SnapshotFragments::new(b"{}".to_vec(), 9, "ab".repeat(16))
        .next()
        .ok_or("missing frame")??;
    let value: serde_json::Value = serde_json::from_slice(&frame)?;
    let schema: serde_json::Value = serde_json::from_str(crate::api::LOCAL_API_SCHEMA_JSON)?;
    let fields = schema
        .pointer("/types/snapshot_fragment/fields")
        .and_then(serde_json::Value::as_object)
        .ok_or("missing fragment schema")?;
    let actual = value.as_object().ok_or("missing fragment object")?;
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        fields.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        actual.get("type"),
        Some(&serde_json::json!("snapshot_fragment"))
    );
    assert_eq!(actual.get("data_hex"), Some(&serde_json::json!("7b7d")));
    Ok(())
}
