use super::*;
fn base(n: usize, state: &str, metadata: usize) -> Authority {
    let entries:Vec<_>=(0..n).map(|i|serde_json::json!({
        "source_path":format!("/synthetic/{i}/{}", "é".repeat(100)),"destination_path":format!("/synthetic/new/{i}"),
        "original_basename":"original","proposed_basename":"new",
        "identity":{"size_bytes":5,"modified_unix":1,"kind":"regular_file","ino":1,"dev":1},
        "preflight_passed":true,"preflight_failures":[],"state":state,
        "failure_reason":null,"applied_at_unix":null,"rolled_back_at_unix":null,
        "unknown_entry_metadata":{"nested":[1,2,3]}
    })).collect();
    let bytes=serde_json::to_vec(&serde_json::json!({"transaction_id":"synthetic-a","plan_generation":1,
        "classifier_version":null,"created_at_unix":1,"source_scan_root":"/synthetic","state":"planned",
        "entries":entries,"created_directories":[],"recovery_resolution":null,"recovery_resolved_at_unix":null,
        "opaque_future_metadata":"x".repeat(metadata)})).unwrap();
    Authority::decode_legacy(&bytes, 16 * 1024 * 1024).unwrap()
}
fn frame(b: &Authority, i: u64, f: Fact, prev: [u8; 32]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(MAGIC);
    v.extend(VERSION.to_le_bytes());
    v.extend(b.binding());
    v.extend(i.to_le_bytes());
    v.push(f as u8);
    v.extend(prev);
    let h = Sha256::digest(&v);
    v.extend(h);
    assert_eq!(v.len(), FRAME_BYTES);
    v
}
fn sequence(b: &Authority, events: &[(u64, Fact)]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut prev = [0; 32];
    for &(i, f) in events {
        let r = frame(b, i, f, prev);
        prev = r[79..].try_into().unwrap();
        v.extend(r);
    }
    v
}
fn decode(b: &Authority, v: &[u8]) -> Result<Observation, Error> {
    materialize(b, v, 1024 * 1024)
}
#[test]
fn normal_apply_undo_and_seal() {
    let b = base(1, "planned", 0);
    let v = sequence(
        &b,
        &[
            (0, Fact::ApplyIntent),
            (0, Fact::ApplyComplete),
            (0, Fact::UndoIntent),
            (0, Fact::UndoComplete),
            (u64::MAX, Fact::RollbackSeal),
        ],
    );
    let o = decode(&b, &v).unwrap();
    assert_eq!(o.entries, [ObservedEntry::Undone]);
    assert!(o.rollback_recorded);
}
#[test]
fn interrupted_apply_and_partial_undo_never_complete() {
    let b = base(2, "planned", 0);
    for events in [
        vec![(0, Fact::ApplyIntent)],
        vec![
            (0, Fact::ApplyIntent),
            (0, Fact::ApplyComplete),
            (0, Fact::UndoIntent),
        ],
    ] {
        let o = decode(&b, &sequence(&b, &events)).unwrap();
        assert!(!o.rollback_recorded);
        let mut e = events;
        e.push((u64::MAX, Fact::RollbackSeal));
        assert_eq!(
            decode(&b, &sequence(&b, &e)).unwrap_err(),
            Error::Transition
        );
    }
}
#[test]
fn retries_failure_reconciliation_and_cancellation() {
    let b = base(2, "planned", 0);
    let events = [
        (0, Fact::ApplyIntent),
        (0, Fact::ApplyFailure),
        (0, Fact::ApplyComplete),
        (u64::MAX, Fact::Cancel),
        (1, Fact::NoMutation),
        (0, Fact::UndoIntent),
        (0, Fact::UndoFailure),
        (0, Fact::UndoComplete),
        (u64::MAX, Fact::RollbackSeal),
    ];
    let v = sequence(&b, &events);
    assert!(decode(&b, &v).unwrap().rollback_recorded);
    let first = &v[..FRAME_BYTES];
    let retries = first.repeat(1000);
    assert_eq!(
        decode(&b, &retries).unwrap().entries[0],
        ObservedEntry::ApplyUnresolved
    );
    assert_eq!(
        budget(b.original.len() as u64, 2, 0, u64::MAX)
            .unwrap()
            .slots,
        16
    );
    let bad = sequence(&b, &[(u64::MAX, Fact::Cancel), (0, Fact::ApplyIntent)]);
    assert!(decode(&b, &bad).is_err());
}
#[test]
fn missing_predecessors_reorder_and_conflict() {
    let b = base(1, "planned", 0);
    assert!(decode(&b, &sequence(&b, &[(0, Fact::ApplyComplete)])).is_err());
    let v = sequence(&b, &[(0, Fact::ApplyIntent), (0, Fact::ApplyComplete)]);
    assert!(decode(&b, &v[FRAME_BYTES..]).is_err());
    let mut reordered = v[FRAME_BYTES..].to_vec();
    reordered.extend(&v[..FRAME_BYTES]);
    assert!(decode(&b, &reordered).is_err());
    let mut conflict = v.clone();
    conflict.extend(frame(&b, 0, Fact::ApplyIntent, [9; 32]));
    assert_eq!(decode(&b, &conflict).unwrap_err(), Error::Conflict);
}
#[test]
fn identity_and_base_content_binding() {
    let b = base(1, "planned", 0);
    let changed = base(1, "planned", 1);
    let v = sequence(&b, &[(0, Fact::ApplyIntent)]);
    assert_eq!(decode(&changed, &v).unwrap_err(), Error::Binding);
    let text = String::from_utf8(b.original.clone())
        .unwrap()
        .replace("synthetic-a", "synthetic-b");
    let other = Authority::decode_legacy(text.as_bytes(), 100000).unwrap();
    assert_eq!(decode(&other, &v).unwrap_err(), Error::Binding);
}
#[test]
fn every_truncated_prefix_and_corruption_fail_closed() {
    let b = base(1, "planned", 0);
    let v = sequence(&b, &[(0, Fact::ApplyIntent)]);
    for end in 1..FRAME_BYTES {
        assert_eq!(decode(&b, &v[..end]).unwrap_err(), Error::Truncated);
    }
    for i in 0..FRAME_BYTES {
        let mut bad = v.clone();
        bad[i] ^= 1;
        assert!(decode(&b, &bad).is_err(), "byte {i}");
    }
    assert_eq!(
        materialize(&b, &v, FRAME_BYTES - 1).unwrap_err(),
        Error::Limit
    );
}
#[test]
fn legacy_75_100_and_large_unknown_metadata_preserved() {
    for n in [75, 100] {
        let b = base(n, "applied", 300000);
        assert!(b.original.len() > 256 * 1024);
        let o = decode(&b, &[]).unwrap();
        assert_eq!(o.entries, vec![ObservedEntry::Applied; n]);
        let again = Authority::decode_legacy(b.original_bytes(), b.original.len()).unwrap();
        assert_eq!(again.original_bytes(), b.original_bytes());
        let mut events = Vec::new();
        for i in 0..n {
            events.push((i as u64, Fact::UndoIntent));
            events.push((i as u64, Fact::UndoComplete));
        }
        events.push((u64::MAX, Fact::RollbackSeal));
        assert!(
            decode(&b, &sequence(&b, &events))
                .unwrap()
                .rollback_recorded
        );
    }
}
#[test]
fn directory_cleanup_required_before_seal() {
    let b = base(1, "applied", 0);
    let mut value: serde_json::Value = serde_json::from_slice(b.original_bytes()).unwrap();
    value["created_directories"] = serde_json::json!(["/synthetic/created"]);
    let b = Authority::decode_legacy(&serde_json::to_vec(&value).unwrap(), 100000).unwrap();
    let mut events = vec![(0, Fact::UndoIntent), (0, Fact::UndoComplete)];
    let mut bad = events.clone();
    bad.push((u64::MAX, Fact::RollbackSeal));
    assert!(decode(&b, &sequence(&b, &bad)).is_err());
    events.extend([
        (0, Fact::DirectoryIntent),
        (0, Fact::DirectoryComplete),
        (u64::MAX, Fact::RollbackSeal),
    ]);
    assert!(
        decode(&b, &sequence(&b, &events))
            .unwrap()
            .rollback_recorded
    );
}
#[test]
fn duplicate_json_keys_and_unknown_facts_refused() {
    assert!(Authority::decode_legacy(br#"{"a":{"x":1,"x":2}}"#, 100).is_err());
    let b = base(1, "planned", 0);
    let mut v = sequence(&b, &[(0, Fact::ApplyIntent)]);
    v[46] = 255;
    let h = Sha256::digest(&v[..79]);
    v[79..].copy_from_slice(&h);
    assert!(decode(&b, &v).is_err());
}
#[test]
fn budget_overflow_capacity_and_linear_scaling() {
    assert_eq!(budget(0, u64::MAX, 0, u64::MAX), Err(Error::Overflow));
    assert_eq!(budget(u64::MAX, 0, 0, u64::MAX), Err(Error::Overflow));
    assert_eq!(budget(0, 0, u64::MAX, u64::MAX), Err(Error::Overflow));
    let b = budget(10000, 50, 3, u64::MAX).unwrap();
    assert_eq!(
        budget(10000, 50, 3, b.accounted_bytes - 1),
        Err(Error::Limit)
    );
    assert_eq!(budget(10000, 50, 3, b.accounted_bytes).unwrap(), b);
    for n in [1, 10, 50, 100, 1000] {
        let b = budget(1024 + 3072 * n, n, 0, u64::MAX).unwrap();
        assert_eq!(b.logical_bytes, 1246 + 3849 * n);
        println!(
            "SCALING entries={n} logical={} accounted={}",
            b.logical_bytes, b.accounted_bytes
        );
    }
}

#[test]
fn complete_operation_authority_and_unknown_nested_metadata_are_retained() {
    let b = base(1, "applied", 0);
    let mut value: serde_json::Value = serde_json::from_slice(b.original_bytes()).unwrap();
    let identity = value["entries"][0]["identity"].clone();
    for operation in [
        serde_json::json!({"kind":"rename_move"}),
        serde_json::json!({"kind":"replace_existing","original_identity":identity,"destination_root":"/synthetic","future":{"x":1}}),
        serde_json::json!({"kind":"create_symlink","expected_target":"/synthetic/source","destination_root":"/synthetic"}),
        serde_json::json!({"kind":"create_hardlink","expected_source":"/synthetic/source","destination_root":"/synthetic"}),
        serde_json::json!({"kind":"create_copy","expected_source":"/synthetic/source","destination_root":"/synthetic"}),
    ] {
        value["entries"][0]["operation"] = operation;
        let bytes = serde_json::to_vec(&value).unwrap();
        let b = Authority::decode_legacy(&bytes, bytes.len()).unwrap();
        assert_eq!(b.original_bytes(), bytes);
        assert_eq!(decode(&b, &[]).unwrap().entries, [ObservedEntry::Applied]);
    }
}
#[test]
fn partial_legacy_rollback_and_ambiguous_apply_remain_unresolved() {
    for (legacy, expected) in [
        ("applying", ObservedEntry::ApplyUnresolved),
        ("apply_failed", ObservedEntry::ApplyUnresolved),
        ("rolling_back", ObservedEntry::UndoUnresolved),
        ("rollback_failed", ObservedEntry::UndoUnresolved),
    ] {
        let b = base(1, legacy, 0);
        let o = decode(&b, &[]).unwrap();
        assert_eq!(o.entries, [expected]);
        assert!(!o.rollback_recorded);
        assert!(decode(&b, &sequence(&b, &[(u64::MAX, Fact::RollbackSeal)])).is_err());
    }
    let b = base(1, "applying", 0);
    assert!(
        decode(
            &b,
            &sequence(&b, &[(0, Fact::NoMutation), (u64::MAX, Fact::RollbackSeal)])
        )
        .unwrap()
        .rollback_recorded
    );
}

#[test]
fn legacy_limits_malformed_and_unsupported_evidence() {
    let b = base(1, "applied", 0);
    assert_eq!(
        Authority::decode_legacy(b.original_bytes(), b.original_bytes().len() - 1).unwrap_err(),
        Error::Limit
    );
    for malformed in [b"".as_slice(), b"{", b"null", b"{}"] {
        assert_eq!(
            Authority::decode_legacy(malformed, 100).unwrap_err(),
            Error::Malformed
        );
    }
    let mut value: serde_json::Value = serde_json::from_slice(b.original_bytes()).unwrap();
    value["entries"][0]["operation"] = serde_json::json!({"kind":"future_unknown_operation"});
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        Authority::decode_legacy(&bytes, bytes.len()).unwrap_err(),
        Error::Malformed
    );
    let mut value: serde_json::Value = serde_json::from_slice(b.original_bytes()).unwrap();
    value["transaction_id"] = serde_json::json!("");
    let bytes = serde_json::to_vec(&value).unwrap();
    assert_eq!(
        Authority::decode_legacy(&bytes, bytes.len()).unwrap_err(),
        Error::Binding
    );
}

#[test]
fn long_paths_are_retained_as_evidence_without_filesystem_access() {
    let b = base(1, "applied", 0);
    let mut value: serde_json::Value = serde_json::from_slice(b.original_bytes()).unwrap();
    let path = format!("/not-a-real-rom-root/{}", "é/".repeat(5000));
    value["entries"][0]["source_path"] = serde_json::json!(path);
    let bytes = serde_json::to_vec(&value).unwrap();
    let decoded = Authority::decode_legacy(&bytes, bytes.len()).unwrap();
    assert_eq!(decoded.original_bytes(), bytes);
    assert_eq!(
        decoded.transaction.entries[0].source_path.to_str(),
        Some(path.as_str())
    );
    assert_eq!(
        decode(&decoded, &[]).unwrap().entries,
        [ObservedEntry::Applied]
    );
}
