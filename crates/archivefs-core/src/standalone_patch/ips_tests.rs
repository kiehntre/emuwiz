//! IPS regression fixtures use literal wire bytes and expected ROM contents.
//! They exercise the existing inspector, preparation and durable publisher.

use super::*;

#[test]
fn literal_data_and_rle_hunks_preserve_untouched_tail() {
    let base = b"abcdefghi";
    for (patch, expected) in [
        (
            b"PATCH\0\0\x01\0\x02XYEOF".as_slice(),
            b"aXYdefghi".as_slice(),
        ),
        (
            b"PATCH\0\0\x02\0\0\0\x03ZEOF".as_slice(),
            b"abZZZfghi".as_slice(),
        ),
    ] {
        let (state, fields) = parse_ips(patch);
        assert_eq!(state, PatchInspectionState::Valid);
        assert_eq!(
            fields.target_size, None,
            "a hunk end is not an exact ROM size"
        );
        assert_eq!(apply_ips(base, patch).unwrap(), expected);
        assert_eq!(base, b"abcdefghi");
    }
}

#[test]
fn out_of_order_and_overlapping_hunks_keep_earlier_changes() {
    // Write the high hunk first, then a low hunk and an overlapping RLE.
    // The last record wins only where it actually writes.
    let patch = b"PATCH\0\0\x06\0\x02XY\0\0\x01\0\x02QQ\0\0\x02\0\0\0\x03ZEOF";
    assert_eq!(apply_ips(b"abcdefghij", patch).unwrap(), b"aQZZZfXYij");
}

#[test]
fn growth_zero_fills_gap_without_erasing_source_or_high_hunk() {
    let patch = b"PATCH\0\0\x05\0\x02XY\0\0\x01\0\x01ZEOF";
    let output = apply_ips(b"ABC", patch).unwrap();
    assert_eq!(output, b"AZC\0\0XY");
    assert_eq!(apply_ips(&output, patch).unwrap(), output);
}

#[test]
fn explicit_eof_size_is_decoded_and_is_the_only_resize_request() {
    // Retain the existing explicit EOF-size extension, including empty
    // output and zero-filled expansion. Ordinary hunks may never shrink.
    for (length, expected) in [
        (0u8, b"".as_slice()),
        (4, b"AXCD".as_slice()),
        (10, b"AXCDEFGH\0\0".as_slice()),
    ] {
        let mut patch = b"PATCH\0\0\x01\0\x01XEOF\0\0".to_vec();
        patch.push(length);
        let (state, fields) = parse_ips(&patch);
        assert_eq!(state, PatchInspectionState::Valid);
        assert_eq!(fields.target_size, Some(u64::from(length)));
        assert_eq!(apply_ips(b"ABCDEFGH", &patch).unwrap(), expected);
        // The public path must agree with inspection's exact-size evidence,
        // including a zero-byte derivative and explicit expansion.
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("base.rom");
        let patch_path = dir.path().join("resize.ips");
        fs::write(&base_path, b"ABCDEFGH").unwrap();
        fs::write(&patch_path, &patch).unwrap();
        let inspection = inspect_standalone_patch(&patch_path).unwrap();
        let plan = build_standalone_patch_apply_plan(
            &inspection,
            &base_path,
            dir.path().join("derivative.rom"),
            dir.path(),
        )
        .unwrap();
        assert_eq!(plan.reviewed.expected_output_size, Some(u64::from(length)));
        let published = apply_standalone_patch(&plan).unwrap();
        assert_eq!(published.output_size, u64::from(length));
        assert_eq!(published.output_sha256, hex_digest(expected));
        assert_eq!(fs::read(&plan.reviewed.output_path).unwrap(), expected);
        assert_eq!(fs::read(&base_path).unwrap(), b"ABCDEFGH");
        assert_eq!(fs::read(&patch_path).unwrap(), patch);
    }
    // A final resize deliberately discards a high hunk outside that size.
    let patch = b"PATCH\0\0\x06\0\x01XEOF\0\0\x04";
    assert_eq!(apply_ips(b"ABCDEFGH", patch).unwrap(), b"ABCD");
}

#[test]
fn empty_patch_preserves_source_without_claiming_zero_output() {
    let (state, fields) = parse_ips(b"PATCHEOF");
    assert_eq!(state, PatchInspectionState::Valid);
    assert_eq!(fields.target_size, None);
    assert_eq!(
        apply_ips(b"unchanged ROM", b"PATCHEOF").unwrap(),
        b"unchanged ROM"
    );
}

#[test]
fn maximum_wire_offset_and_rle_length_have_bounded_growth() {
    let patch = b"PATCH\xff\xff\xff\0\0\xff\xffZEOF";
    let output = apply_ips(b"ROM", patch).unwrap();
    let start = 0x00ff_ffffusize;
    let end = start + usize::from(u16::MAX);
    assert_eq!(output.len(), end);
    assert_eq!(&output[..3], b"ROM");
    assert!(output[3..start].iter().all(|byte| *byte == 0));
    assert!(output[start..].iter().all(|byte| *byte == b'Z'));
    assert!((end as u64) < MAX_APPLY_BYTES);
}

#[test]
fn record_budget_is_enforced_before_output_preparation() {
    // Six bytes per record: a small, valid-looking stream with too many
    // writes must fail rather than doing unbounded record work.
    let mut patch = b"PATCH".to_vec();
    for _ in 0..=MAX_RECORDS {
        patch.extend_from_slice(b"\0\0\0\0\x01X");
    }
    patch.extend_from_slice(b"EOF");
    let (state, fields) = parse_ips(&patch);
    assert_eq!(state, PatchInspectionState::Invalid);
    assert_eq!(fields.error.as_deref(), Some("too many records"));
    assert!(
        matches!(apply_ips(b"ROM", &patch), Err(StandalonePatchError::Malformed(reason)) if reason.contains("too many records"))
    );
    // Exactly the cap is still valid; the original bytes after the first
    // patched byte must survive even after a million overlapping writes.
    patch.drain(5..11);
    assert_eq!(parse_ips(&patch).0, PatchInspectionState::Valid);
    assert_eq!(apply_ips(b"ROM", &patch).unwrap(), b"XOM");
}

#[test]
fn malformed_frames_and_truncated_neighbours_never_panic() {
    let valid = b"PATCH\0\0\x01\0\x02XY\0\0\x02\0\0\0\x03ZEOF";
    let mut cases: Vec<Vec<u8>> = (0..valid.len()).map(|end| valid[..end].to_vec()).collect();
    cases.extend([
        b"xxxxxEOF".to_vec(),
        b"PATCH\0\0\0\0\0\0\0ZEOF".to_vec(), // zero-length RLE
        b"PATCH\0\0\0\0\x02XEOF".to_vec(),   // missing record/EOF framing
        b"PATCHEOFx".to_vec(),
        b"PATCHEOFxy".to_vec(),
        b"PATCHEOFxyzw".to_vec(),
    ]);
    for (index, patch) in cases.iter().enumerate() {
        let result = std::panic::catch_unwind(|| {
            let (state, _) = parse_ips(patch);
            assert_eq!(state, PatchInspectionState::Invalid, "fixture {index}");
            assert!(apply_ips(b"original", patch).is_err(), "fixture {index}");
        });
        assert!(result.is_ok(), "fixture {index} panicked");
    }
}

fn reviewed_fixture() -> (tempfile::TempDir, StandalonePatchApplyPlan) {
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().join("base.rom");
    let patch_path = dir.path().join("edit.ips");
    fs::write(&base_path, b"abcdefghi").unwrap();
    fs::write(&patch_path, b"PATCH\0\0\x01\0\x02XYEOF").unwrap();
    let inspection = inspect_standalone_patch(&patch_path).unwrap();
    let plan = build_standalone_patch_apply_plan(
        &inspection,
        &base_path,
        dir.path().join("derivative.rom"),
        dir.path(),
    )
    .unwrap();
    (dir, plan)
}

#[test]
fn deterministic_preparation_and_publication_preserve_original_and_patch() {
    let (dir, plan) = reviewed_fixture();
    let base_before = fs::read(&plan.reviewed.base_path).unwrap();
    let patch_before = fs::read(&plan.reviewed.patch_path).unwrap();
    let expected = b"aXYdefghi";
    assert_eq!(plan.reviewed.expected_output_size, None);
    assert!(plan.reviewed.confirmation_required);
    assert!(!plan.reviewed.overwrite_existing);
    let first = prepare_standalone_patch_output(&plan).unwrap();
    let second = prepare_standalone_patch_output(&plan).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.bytes, expected);
    assert!(!plan.reviewed.output_path.exists());
    let published = apply_standalone_patch(&plan).unwrap();
    assert_eq!(fs::read(&plan.reviewed.output_path).unwrap(), expected);
    assert_eq!(published.output_size, expected.len() as u64);
    assert_eq!(published.output_sha256, hex_digest(expected));
    assert_eq!(published.provenance.base_sha256, hex_digest(&base_before));
    assert_eq!(published.provenance.patch_sha256, hex_digest(&patch_before));
    assert_eq!(published.provenance.expected_source_crc32, None);
    assert_eq!(published.provenance.expected_output_crc32, None);
    assert_eq!(fs::read(&plan.reviewed.base_path).unwrap(), base_before);
    assert_eq!(fs::read(&plan.reviewed.patch_path).unwrap(), patch_before);
    assert!(
        apply_standalone_patch(&plan).is_err(),
        "repeat must not overwrite"
    );
    assert_eq!(fs::read(&plan.reviewed.output_path).unwrap(), expected);
    assert_eq!(fs::read(&plan.reviewed.base_path).unwrap(), base_before);
    assert!(
        crate::patch_output_recovery::discover_pending_patch_outputs(dir.path())
            .0
            .is_empty()
    );
}

#[test]
fn absent_source_checksum_stays_weak_and_changed_reviewed_source_fails_closed() {
    let (dir, plan) = reviewed_fixture();
    let inspection = inspect_standalone_patch(&plan.reviewed.patch_path).unwrap();
    assert_eq!(inspection.source_size, None);
    assert_eq!(inspection.source_crc32, None);
    assert_eq!(inspection.target_crc32, None);
    for source in [b"abcdefghi".as_slice(), b"wrong ROM".as_slice()] {
        assert_eq!(
            match_patch_source(&inspection, Some(source), false).compatibility,
            PatchCompatibility::Unknown
        );
        assert_eq!(
            match_patch_source(&inspection, Some(source), true).compatibility,
            PatchCompatibility::ReviewRequired
        );
    }
    fs::write(&plan.reviewed.base_path, b"wrong ROM").unwrap();
    for result in [
        prepare_standalone_patch_output(&plan).map(|_| ()),
        apply_standalone_patch(&plan).map(|_| ()),
    ] {
        assert!(
            matches!(result, Err(StandalonePatchError::Malformed(reason)) if reason == "base changed since review")
        );
    }
    assert!(!plan.reviewed.output_path.exists());
    assert_eq!(fs::read(&plan.reviewed.base_path).unwrap(), b"wrong ROM");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn changed_patch_hash_fails_closed_before_any_output_is_written() {
    let (dir, plan) = reviewed_fixture();
    // IPS contains no patch CRC; the reviewed SHA-256 still detects edits.
    fs::write(&plan.reviewed.patch_path, b"PATCH\0\0\x01\0\x02ZZEOF").unwrap();
    for result in [
        prepare_standalone_patch_output(&plan).map(|_| ()),
        apply_standalone_patch(&plan).map(|_| ()),
    ] {
        assert!(
            matches!(result, Err(StandalonePatchError::Malformed(reason)) if reason == "patch changed since review")
        );
    }
    assert!(!plan.reviewed.output_path.exists());
    assert_eq!(fs::read(&plan.reviewed.base_path).unwrap(), b"abcdefghi");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}
