use super::*;
use archivefs_core::dat::library_identity_summary::*;
use archivefs_core::game_identity::IdentityPlatform;
use archivefs_core::standalone_patch::{apply_standalone_patch, inspect_standalone_patch};
use std::path::Path;

fn number(mut value: u64) -> Vec<u8> {
    let mut result = Vec::new();
    loop {
        let low = (value & 127) as u8;
        value >>= 7;
        if value == 0 {
            result.push(low | 128);
            return result;
        }
        result.push(low);
        value -= 1;
    }
}
fn patch(source: &[u8], target: &[u8]) -> Vec<u8> {
    let mut bytes = b"BPS1".to_vec();
    bytes.extend(number(source.len() as u64));
    bytes.extend(number(target.len() as u64));
    bytes.push(128);
    bytes.extend(number((((target.len() - 1) as u64) << 2) | 1));
    bytes.extend(target);
    bytes.extend(crc32(source).to_le_bytes());
    bytes.extend(crc32(target).to_le_bytes());
    bytes.extend(crc32(&bytes).to_le_bytes());
    bytes
}
fn identity_for(path: &Path) -> GameIdentityReport {
    let mut id = super::tests::identity(path);
    let bytes = std::fs::read(path).unwrap();
    use sha2::{Digest, Sha256};
    id.evidence[0].value = Some(
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    );
    id
}
fn candidate(
    root: &Path,
    source: &[u8],
    patch_base: &[u8],
    platform: IdentityPlatform,
) -> StandalonePatchCandidate {
    let base = root.join("same-title.bin");
    let patch_path = root.join("same-title.bps");
    std::fs::write(&base, source).unwrap();
    std::fs::write(&patch_path, patch(patch_base, b"patched output")).unwrap();
    let mut identity = identity_for(&base);
    identity.platform = platform;
    patch_representation::prepare(
        &patch_path,
        &base,
        &identity,
        &[],
        root.join("derived.bin"),
        root.into(),
    )
    .unwrap()
}
fn summary() -> LibraryDatIdentitySummary {
    LibraryDatIdentitySummary {
        verification_state: DatVerificationState::Conflicting {
            detail: "synthetic contradiction".into(),
        },
        source: DatSourceProvenance {
            source_id: "fixture".into(),
            source_name: "Synthetic DAT".into(),
            ecosystem: None,
            variant: None,
            source_revision: Some("1".into()),
            catalogue_sha256: None,
            author: None,
            catalogue_names: vec![],
            dat_path: "unused fixture provenance".into(),
        },
        canonical: DatCanonicalIdentity::default(),
        hash_evidence: DatHashEvidenceSummary {
            matched_algorithm: None,
            matched_value: None,
            available_algorithms: vec![],
        },
        provenance_freshness: DatProvenanceFreshness::Current,
        ambiguous_candidates: vec![],
        candidate_provenance: vec![],
        set_dependency: DatSetDependencySummary::Pending {
            reason: "not arcade".into(),
        },
    }
}
#[test]
fn patch_guidance_normal_apply_preserves_source_and_existing_result_provenance() {
    let root = tempfile::tempdir().unwrap();
    let c = candidate(
        root.path(),
        b"base bytes",
        b"base bytes",
        IdentityPlatform::Snes,
    );
    assert!(c.plan.is_some());
    let plan = c.plan.unwrap();
    let result = apply_standalone_patch(&plan).unwrap();
    assert_eq!(
        std::fs::read(&result.output_path).unwrap(),
        b"patched output"
    );
    assert_eq!(
        std::fs::read(root.path().join("same-title.bin")).unwrap(),
        b"base bytes"
    );
    assert_eq!(result.provenance.header_adjustment, HeaderAdjustment::None);
    assert_eq!(result.provenance.base_sha256, c.source_sha256);
}
#[test]
fn patch_guidance_explicit_snes_header_route_still_works_without_touching_original() {
    let root = tempfile::tempdir().unwrap();
    let payload = vec![0x42; 32768];
    let mut original = vec![0xa5; 512];
    original.extend(&payload);
    let c = candidate(root.path(), &original, &payload, IdentityPlatform::Snes);
    assert_eq!(c.adjustment, HeaderAdjustment::Strip512ByteCopierHeader);
    assert_eq!(
        c.guidance.match_result.compatibility,
        PatchCompatibility::Incompatible,
        "guidance alone must not authorize a transform"
    );
    let result = apply_standalone_patch(c.plan.as_ref().unwrap()).unwrap();
    assert_eq!(
        result.provenance.header_adjustment,
        HeaderAdjustment::Strip512ByteCopierHeader
    );
    assert_eq!(
        std::fs::read(root.path().join("same-title.bin")).unwrap(),
        original
    );
}
#[test]
fn patch_guidance_wrong_content_even_with_matching_title_has_no_plan() {
    let root = tempfile::tempdir().unwrap();
    let c = candidate(
        root.path(),
        b"wrong content",
        b"correct bytes",
        IdentityPlatform::Snes,
    );
    assert_eq!(c.matching.compatibility, PatchCompatibility::Incompatible);
    assert!(c.plan.is_none());
    assert!(!root.path().join("derived.bin").exists());
}
#[test]
fn patch_guidance_unknown_representation_never_invents_conversion() {
    let root = tempfile::tempdir().unwrap();
    let c = candidate(
        root.path(),
        b"unknown data",
        b"other layout",
        IdentityPlatform::Other,
    );
    assert!(c.plan.is_none());
    assert!(c.guidance.required_transformation.is_none());
}
#[test]
fn patch_guidance_conflicting_dat_refuses_but_stale_history_is_not_failed_matching() {
    let root = tempfile::tempdir().unwrap();
    let c = candidate(
        root.path(),
        b"base bytes",
        b"base bytes",
        IdentityPlatform::Snes,
    );
    let source = root.path().join("same-title.bin");
    let mut s = summary();
    let identity = identity_for(&source);
    let prepare = |summaries: &[LibraryDatIdentitySummary]| {
        patch_representation::prepare(
            &c.inspection.path,
            &source,
            &identity,
            summaries,
            root.path().join("new.bin"),
            root.path().into(),
        )
        .unwrap()
    };
    let blocked = prepare(&[s.clone()]);
    assert!(blocked.plan.is_none());
    assert!(blocked.notes.iter().any(|n| n.contains("Conflicting DAT")));
    s.provenance_freshness = DatProvenanceFreshness::Stale;
    let historical = prepare(&[s]);
    assert!(historical.plan.is_some());
    assert!(
        historical
            .notes
            .iter()
            .any(|n| n.contains("Historical DAT result retained"))
    );
}
#[test]
fn patch_guidance_normalized_dat_evidence_does_not_disable_existing_explicit_header_route() {
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let payload = vec![0x42; 32768];
    let mut original = vec![0xa5; 512];
    original.extend(&payload);
    let c = candidate(root.path(), &original, &payload, IdentityPlatform::Snes);
    let source = root.path().join("same-title.bin");
    let identity = identity_for(&source);
    let mut s = summary();
    s.verification_state = DatVerificationState::VerifiedSingleMatch {
        algorithm: "SHA-256".into(),
    };
    s.hash_evidence.matched_algorithm = Some("SHA-256".into());
    s.hash_evidence.matched_value = Some(
        Sha256::digest(&payload)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    );
    let prepared = patch_representation::prepare(
        &c.inspection.path,
        &source,
        &identity,
        &[s],
        root.path().join("new.bin"),
        root.path().into(),
    )
    .unwrap();
    assert!(prepared.plan.is_some());
    assert!(
        prepared
            .notes
            .iter()
            .any(|note| note.contains("normalized representation"))
    );
    assert_eq!(
        prepared.adjustment,
        HeaderAdjustment::Strip512ByteCopierHeader
    );
}
#[test]
fn patch_guidance_changed_cached_hash_or_source_after_preview_refuses() {
    let root = tempfile::tempdir().unwrap();
    let c = candidate(
        root.path(),
        b"base bytes",
        b"base bytes",
        IdentityPlatform::Snes,
    );
    let source = root.path().join("same-title.bin");
    let identity = identity_for(&source);
    let mut s = summary();
    s.verification_state = DatVerificationState::VerifiedSingleMatch {
        algorithm: "SHA-256".into(),
    };
    s.hash_evidence.matched_algorithm = Some("SHA-256".into());
    s.hash_evidence.matched_value = Some("0".repeat(64));
    let blocked = patch_representation::prepare(
        &c.inspection.path,
        &source,
        &identity,
        &[s],
        root.path().join("new.bin"),
        root.path().into(),
    )
    .unwrap();
    assert!(blocked.plan.is_none());
    std::fs::write(&source, b"new bytes!").unwrap();
    assert!(apply_standalone_patch(c.plan.as_ref().unwrap()).is_err());
    assert!(!root.path().join("derived.bin").exists());
    let stale = patch_representation::prepare(
        &c.inspection.path,
        &source,
        &identity,
        &[],
        root.path().join("new.bin"),
        root.path().into(),
    )
    .unwrap();
    assert!(stale.plan.is_none());
}
#[test]
fn patch_guidance_incorrect_bps_copy_offset_never_publishes_output() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("base.bin");
    std::fs::write(&source, b"abcd").unwrap();
    let mut bytes = b"BPS1".to_vec();
    bytes.extend(number(4));
    bytes.extend(number(1));
    bytes.push(128);
    bytes.extend(number(2));
    bytes.extend(number(20));
    bytes.extend(crc32(b"abcd").to_le_bytes());
    bytes.extend(crc32(b"x").to_le_bytes());
    bytes.extend(crc32(&bytes).to_le_bytes());
    let p = root.path().join("bad-offset.bps");
    std::fs::write(&p, bytes).unwrap();
    let i = inspect_standalone_patch(&p).unwrap();
    let plan = build_standalone_patch_apply_plan_with_header(
        &i,
        &source,
        root.path().join("out.bin"),
        root.path(),
        HeaderAdjustment::None,
    )
    .unwrap();
    assert!(apply_standalone_patch(&plan).is_err());
    assert!(!root.path().join("out.bin").exists());
    assert_eq!(std::fs::read(source).unwrap(), b"abcd");
}
#[test]
fn patch_guidance_evidence_change_disables_confirmation_and_preserves_inflight_receipt() {
    let root = tempfile::tempdir().unwrap();
    let c = candidate(
        root.path(),
        b"base bytes",
        b"base bytes",
        IdentityPlatform::Snes,
    );
    let mut state = StandalonePatchPageState {
        confirmed: true,
        evidence_key: Some(c.evidence_key.clone()),
        candidates: vec![c],
        ..Default::default()
    };
    let source = root.path().join("same-title.bin");
    let identity = identity_for(&source);
    let changed = summary();
    let (sender, receiver) = mpsc::channel();
    state.applying = Some(receiver);
    let context = egui::Context::default();
    let _ = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_standalone_patch_panel(ui, &mut state, &source, &identity, &[], &[changed.clone()])
        });
    });
    assert!(!state.confirmed);
    assert!(state.applying.is_some());
    assert_eq!(
        state.candidates.len(),
        1,
        "keep the reviewed operation evidence"
    );
    assert_ne!(
        state.evidence_key.as_deref(),
        Some(state.candidates[0].evidence_key.as_str())
    );
    drop(sender);
}
#[test]
fn patch_guidance_renderer_shows_advisory_transform_and_no_automatic_conversion() {
    let payload = vec![0x42; 32768];
    let mut bytes = vec![0xa5; 512];
    bytes.extend(&payload);
    let root = tempfile::tempdir().unwrap();
    let c = candidate(root.path(), &bytes, &payload, IdentityPlatform::Snes);
    let context = egui::Context::default();
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            patch_representation::show(ui, &c.guidance, &c.notes)
        });
    });
    let text = format!("{:?}", output.shapes);
    assert!(text.contains("ROM representation guidance"));
    assert!(text.contains("Planning only"));
    assert!(text.contains("not cryptographic"));
}

#[test]
fn patch_guidance_dat_no_match_does_not_override_patch_source_checksum() {
    let root = tempfile::tempdir().unwrap();
    let c = candidate(
        root.path(),
        b"base bytes",
        b"base bytes",
        IdentityPlatform::Snes,
    );
    let source = root.path().join("same-title.bin");
    let identity = identity_for(&source);
    let mut s = summary();
    s.verification_state = DatVerificationState::NoMatch;
    let prepared = patch_representation::prepare(
        &c.inspection.path,
        &source,
        &identity,
        &[s],
        root.path().join("new.bin"),
        root.path().into(),
    )
    .unwrap();
    assert!(prepared.plan.is_some());
    assert_eq!(
        prepared.matching.compatibility,
        PatchCompatibility::Compatible
    );
}
