use super::*;
use crate::game_identity::{IdentityConfidence, IdentityProvenance};
use crate::persistent_state_inventory::{PortabilityClass, StatePathOrigin};
use crate::save_state_resume_readiness::{SaveStateResumeVerdict, plan_save_state_resume};
use sha2::{Digest, Sha256};

fn identity(kind: IdentityKind, value: &str) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        value: Some(value.into()),
        status: IdentityStatus::Verified,
        confidence: IdentityConfidence::ExactBytes,
        provenance: IdentityProvenance {
            archive_path: "synthetic".into(),
            member_path: None,
            member_index: None,
            method: "synthetic fixture".into(),
        },
        diagnostic: "synthetic fixture".into(),
    }
}
const STATE_HASH: &str = "4ba69735ca53765ed6a709edb56c6ea236b7193a3b29a6b390c346f0f4340e4e";

fn fixture() -> (
    PersistentStateRecord,
    SaveStateCapture,
    SaveStateCompatibilityTarget,
) {
    let game = vec![identity(IdentityKind::Ps1Serial, "SCUS-12345")];
    let record = PersistentStateRecord {
        sram_origin: None,
        emulator: StateEmulator::DuckStation,
        selected_installation: None,
        state_type: PersistentStateType::SaveState,
        game_identity: game.clone(),
        path: "synthetic.state".into(),
        container_path: None,
        slot_profile_account: Some("profile".into()),
        emulator_version: Some("1.0".into()),
        firmware_context: None,
        portability_class: PortabilityClass::VersionBound,
        source_path_origin: StatePathOrigin::Configured,
        provenance: "synthetic inventory".into(),
        sha256: Some(STATE_HASH.into()),
        size_bytes: 5,
        warnings: vec![],
    };
    let capture = SaveStateCapture {
        emulator: record.emulator,
        version: record.emulator_version.clone(),
        core: None,
        core_version: None,
        profile: Some("profile".into()),
        game: game.clone(),
        state_sha256: STATE_HASH.into(),
    };
    let target = SaveStateCompatibilityTarget {
        emulator: capture.emulator,
        version: capture.version.clone(),
        core: None,
        core_version: None,
        profile: capture.profile.clone(),
        game,
    };
    (record, capture, target)
}
fn contract(
    capture: &SaveStateCapture,
    target: &SaveStateCompatibilityTarget,
) -> SaveStateFormatEvidence {
    SaveStateFormatEvidence::Supported {
        capture: capture.clone(),
        target: target.clone(),
        source: "synthetic adapter verified format and exact version contract".into(),
    }
}
fn assess(
    record: &PersistentStateRecord,
    capture: &SaveStateCapture,
    target: &SaveStateCompatibilityTarget,
    format: &SaveStateFormatEvidence,
) -> SaveStateAssessment {
    assess_save_state_compatibility(
        record,
        Some(capture),
        Some(target),
        format,
        Some(STATE_HASH),
    )
}
#[test]
fn exact_capture_and_verified_contract_support_manual_review_only() {
    let (r, c, t) = fixture();
    let a = assess(&r, &c, &t, &contract(&c, &t));
    assert_eq!(a.compatibility, SaveStateCompatibility::Supported);
    let readiness = plan_save_state_resume(a, vec![]);
    assert_eq!(
        readiness.verdict,
        SaveStateResumeVerdict::ReadyForManualReview
    );
    assert!(!readiness.resume_available);
}
#[test]
fn matching_metadata_and_extensions_never_prove_format_support() {
    let (r, c, t) = fixture();
    assert_eq!(
        assess(&r, &c, &t, &SaveStateFormatEvidence::Unknown).compatibility,
        SaveStateCompatibility::Unknown
    );
    assert_eq!(
        assess_save_state_compatibility(&r, None, Some(&t), &contract(&c, &t), Some(STATE_HASH))
            .compatibility,
        SaveStateCompatibility::Unknown
    );
}
#[test]
fn different_emulator_and_retroarch_core_are_incompatible() {
    let (r, mut c, mut t) = fixture();
    t.emulator = StateEmulator::Pcsx2;
    assert_eq!(
        assess(&r, &c, &t, &SaveStateFormatEvidence::Unknown).compatibility,
        SaveStateCompatibility::Incompatible
    );
    c.emulator = StateEmulator::RetroArch;
    t.emulator = StateEmulator::RetroArch;
    c.core = Some("core-a".into());
    t.core = Some("core-b".into());
    assert_eq!(
        assess(&r, &c, &t, &SaveStateFormatEvidence::Unknown).compatibility,
        SaveStateCompatibility::Incompatible
    );
}
#[test]
fn missing_and_different_versions_are_unknown_not_proven_incompatible() {
    for version in [None, Some("".into()), Some("2.0".into())] {
        let (r, mut c, mut t) = fixture();
        c.version = version.clone();
        t.version = version;
        assert_eq!(
            assess(&r, &c, &t, &SaveStateFormatEvidence::Unknown).compatibility,
            SaveStateCompatibility::Unknown
        );
    }
    let (r, c, mut t) = fixture();
    t.version = Some("2.0".into());
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
}
#[test]
fn retroarch_requires_known_core_and_core_version_even_with_contract() {
    let (r, mut c, mut t) = fixture();
    c.emulator = StateEmulator::RetroArch;
    t.emulator = c.emulator;
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
    c.core = Some("core-a".into());
    t.core = c.core.clone();
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
    c.core_version = Some("build1".into());
    t.core_version = c.core_version.clone();
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Supported
    );
}
#[test]
fn changed_selection_invalidates_previously_verified_format_contract() {
    let (r, c, mut t) = fixture();
    let previous = contract(&c, &t);
    t.profile = Some("other".into());
    let a = assess(&r, &c, &t, &previous);
    assert_eq!(a.compatibility, SaveStateCompatibility::Unknown);
    assert!(a.reasons.iter().any(|r| r.contains("profile changed")));
    t.emulator = StateEmulator::Pcsx2;
    assert_eq!(
        assess(&r, &c, &t, &previous).compatibility,
        SaveStateCompatibility::Incompatible
    );
}
#[test]
fn platform_and_revision_do_not_establish_game_identity() {
    let items = vec![
        identity(IdentityKind::Platform, "PlayStation"),
        identity(IdentityKind::DolphinRevision, "0"),
    ];
    assert!(!same_game(&items, &items));
    let (r, mut c, mut t) = fixture();
    c.game = items.clone();
    t.game = items;
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
}
#[test]
fn ambiguous_conflicting_filename_and_stale_game_identity_stay_unknown() {
    let (r, c, t) = fixture();
    for mode in 0..4 {
        let mut c = c.clone();
        match mode {
            0 => c.game[0].status = IdentityStatus::Ambiguous,
            1 => c.game.push(identity(IdentityKind::Ps1Serial, "SCUS-99999")),
            2 => {
                c.game[0].status = IdentityStatus::Candidate;
                c.game[0].confidence = IdentityConfidence::FilenameOnly;
            }
            _ => c.game[0].value = Some("SCUS-99999".into()),
        }
        assert_eq!(
            assess(&r, &c, &t, &contract(&c, &t)).compatibility,
            SaveStateCompatibility::Unknown
        );
    }
}
#[test]
fn stale_capture_cannot_prove_an_emulator_mismatch() {
    let (r, mut c, mut t) = fixture();
    c.state_sha256 = "old hash".into();
    t.emulator = StateEmulator::Pcsx2;
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
}
#[test]
fn missing_state_and_unsupported_format() {
    let (r, c, t) = fixture();
    assert_eq!(
        assess_save_state_compatibility(&r, Some(&c), Some(&t), &contract(&c, &t), None)
            .compatibility,
        SaveStateCompatibility::Unknown
    );
    assert_eq!(
        assess(
            &r,
            &c,
            &t,
            &SaveStateFormatEvidence::Unsupported("adapter cannot read this format".into())
        )
        .compatibility,
        SaveStateCompatibility::Incompatible
    );
}
#[test]
fn ordinary_saves_are_not_save_states_and_blockers_remain() {
    let (mut r, c, t) = fixture();
    r.state_type = PersistentStateType::NativeSave;
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
    r.state_type = PersistentStateType::SaveState;
    let blocker = crate::launch::LaunchBlocker {
        kind: crate::launch::LaunchBlockerKind::RequiredFirmwareMissing,
        detail: "synthetic missing firmware".into(),
    };
    let ready =
        plan_save_state_resume(assess(&r, &c, &t, &contract(&c, &t)), vec![blocker.clone()]);
    assert_eq!(ready.launch_blockers, vec![blocker]);
    assert_eq!(ready.verdict, SaveStateResumeVerdict::LaunchBlocked);
    assert!(!ready.resume_available);
}
#[test]
fn assessment_does_not_write_saves_or_configuration_or_execute_an_emulator() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("game.state");
    let config = dir.path().join("emulator.ini");
    std::fs::write(&state, b"state").unwrap();
    std::fs::write(&config, b"config").unwrap();
    let before: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|x| x.unwrap().path())
        .collect();
    let hash = Sha256::digest(b"state")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let (mut r, mut c, t) = fixture();
    r.path = state.clone();
    r.sha256 = Some(hash.clone());
    c.state_sha256 = hash.clone();
    let a = assess_save_state_compatibility(&r, Some(&c), Some(&t), &contract(&c, &t), Some(&hash));
    assert_eq!(a.compatibility, SaveStateCompatibility::Supported);
    assert_eq!(std::fs::read(&state).unwrap(), b"state");
    assert_eq!(std::fs::read(&config).unwrap(), b"config");
    let after: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|x| x.unwrap().path())
        .collect();
    assert_eq!(before, after);
    std::fs::remove_file(&state).unwrap();
    let missing = assess_save_state_compatibility(&r, Some(&c), Some(&t), &contract(&c, &t), None);
    assert_eq!(missing.compatibility, SaveStateCompatibility::Unknown);
}

#[test]
fn whitespace_core_is_missing_evidence() {
    let (r, mut c, mut t) = fixture();
    c.emulator = StateEmulator::RetroArch;
    t.emulator = c.emulator;
    c.core = Some("   ".into());
    t.core = c.core.clone();
    c.core_version = Some("1".into());
    t.core_version = c.core_version.clone();
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
}

#[test]
fn filename_confidence_cannot_be_promoted_by_verified_status() {
    let (r, mut c, mut t) = fixture();
    c.game[0].confidence = IdentityConfidence::FilenameOnly;
    t.game[0].confidence = IdentityConfidence::FilenameOnly;
    assert!(!same_game(&c.game, &t.game));
    assert_eq!(
        assess(&r, &c, &t, &contract(&c, &t)).compatibility,
        SaveStateCompatibility::Unknown
    );
}

#[test]
fn malformed_hash_is_not_freshness_evidence() {
    let (mut r, mut c, t) = fixture();
    r.sha256 = Some("not a digest".into());
    c.state_sha256 = "not a digest".into();
    assert_eq!(
        assess_save_state_compatibility(
            &r,
            Some(&c),
            Some(&t),
            &contract(&c, &t),
            Some("not a digest")
        )
        .compatibility,
        SaveStateCompatibility::Unknown
    );
}
