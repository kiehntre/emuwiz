use super::*;
use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use crate::launch::planning::{CandidatePreference, LaunchContentRef, LaunchTarget};
use crate::launch::readiness::{LaunchReadiness, LaunchWarning};

fn candidate(firmware: FirmwareReadiness, profile: &str) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "test-emulator",
            profile_id: profile.to_string(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: None,
            requires_mount: false,
            provenance: "synthetic fixture".into(),
        },
        firmware,
        blockers: Vec::new(),
        warnings: Vec::<LaunchWarning>::new(),
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn option(id: &str, label: &str, hash: &str, region: &str) -> FirmwareOption {
    FirmwareOption {
        id: id.into(),
        label: label.into(),
        accepted_sha256: vec![hash.into()],
        region: Some(region.into()),
        version: Some("1".into()),
        emulator_managed: false,
    }
}

fn required() -> FirmwareRequirementDescriptor {
    FirmwareRequirementDescriptor {
        id: "test-bios".into(),
        label: "Test BIOS".into(),
        required: true,
        options: vec![option("us-v1", "US v1", "good", "US")],
        emulator_managed: false,
        region_requirement_proven: true,
        version_requirement_proven: true,
    }
}

fn required_two_options() -> FirmwareRequirementDescriptor {
    let mut requirement = required();
    requirement
        .options
        .push(option("jp-v1", "JP v1", "good-jp", "JP"));
    requirement
}

fn evidence(
    option_id: Option<&str>,
    strength: FirmwareEvidenceStrength,
    status: FirmwareEvidenceStatus,
) -> FirmwareEvidenceSummary {
    FirmwareEvidenceSummary {
        option_id: option_id.map(str::to_string),
        path: Some(PathBuf::from("/firmware/test.bin")),
        filename: Some("test.bin".into()),
        sha256: Some("good".into()),
        strength,
        status,
        region: Some("US".into()),
        version: Some("1".into()),
        provenance: "synthetic adapter evidence".into(),
    }
}

fn accepted_hash(option_id: &str) -> FirmwareEvidenceSummary {
    evidence(
        Some(option_id),
        FirmwareEvidenceStrength::ExactKnownHash,
        FirmwareEvidenceStatus::Accepted,
    )
}

fn project(
    candidate: &LaunchCandidate,
    requirement: FirmwareRequirementDescriptor,
    evidence: Vec<FirmwareEvidenceSummary>,
    selected_option_id: Option<&str>,
    freshness: FirmwareFreshness,
) -> GameFirmwareReadiness {
    GameFirmwareReadiness::project(GameFirmwareReadinessInput {
        game_id: Some("game-1".into()),
        platform: "test-platform".into(),
        release_identity: Some("revision-1".into()),
        game_region: Some("US".into()),
        game_revision: Some("1".into()),
        candidate,
        requirement,
        evidence,
        selected_option_id: selected_option_id.map(str::to_string),
        freshness,
    })
}

fn snapshot() -> FirmwareFreshnessSnapshot {
    FirmwareFreshnessSnapshot {
        profile_fingerprint: Some("profile-1".into()),
        firmware_path_fingerprint: Some("path-1".into()),
        firmware_file_fingerprint: Some("file-1".into()),
        game_identity_fingerprint: Some("game-1".into()),
        requirement_fingerprint: Some("requirement-1".into()),
    }
}

fn current_freshness() -> FirmwareFreshness {
    FirmwareFreshness::current(snapshot())
}

// ---- states (ported from the historical module, re-checked) ---------------

#[test]
fn not_required_is_explicit() {
    let result = project(
        &candidate(FirmwareReadiness::NotRequired, "ppsspp"),
        FirmwareRequirementDescriptor::not_required("ppsspp", "No external firmware"),
        Vec::new(),
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::NotRequired);
}

#[test]
fn exact_hash_is_ready() {
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "duckstation"),
        required(),
        vec![accepted_hash("us-v1")],
        Some("us-v1"),
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Ready);
}

#[test]
fn emulator_verified_identity_is_ready() {
    let result = project(
        &candidate(FirmwareReadiness::PresentUnverified, "flycast"),
        required(),
        vec![evidence(
            Some("us-v1"),
            FirmwareEvidenceStrength::EmulatorVerifiedIdentity,
            FirmwareEvidenceStatus::Accepted,
        )],
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Ready);
}

#[test]
fn missing_firmware_is_missing() {
    let result = project(
        &candidate(FirmwareReadiness::Missing, "pcsx2"),
        required(),
        Vec::new(),
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Missing);
}

#[test]
fn filename_only_never_becomes_ready() {
    let result = project(
        &candidate(FirmwareReadiness::PresentUnverified, "duckstation"),
        required(),
        vec![evidence(
            Some("us-v1"),
            FirmwareEvidenceStrength::FilenameOrPresenceOnly,
            FirmwareEvidenceStatus::Accepted,
        )],
        Some("us-v1"),
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::PresentUnverified);
}

#[test]
fn wrong_hash_is_not_ready() {
    let result = project(
        &candidate(FirmwareReadiness::PresentUnverified, "duckstation"),
        required(),
        vec![evidence(
            Some("us-v1"),
            FirmwareEvidenceStrength::ExactKnownHash,
            FirmwareEvidenceStatus::HashMismatch,
        )],
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::HashMismatch);
}

#[test]
fn region_and_version_mismatch_require_proven_adapter_evidence() {
    let region = project(
        &candidate(FirmwareReadiness::PresentUnverified, "flycast"),
        required(),
        vec![evidence(
            Some("us-v1"),
            FirmwareEvidenceStrength::EmulatorVerifiedIdentity,
            FirmwareEvidenceStatus::RegionMismatchProven,
        )],
        None,
        current_freshness(),
    );
    assert_eq!(region.state, GameFirmwareState::WrongRegion);

    let version = project(
        &candidate(FirmwareReadiness::PresentUnverified, "hatari"),
        required(),
        vec![evidence(
            Some("us-v1"),
            FirmwareEvidenceStrength::EmulatorVerifiedIdentity,
            FirmwareEvidenceStatus::VersionMismatchProven,
        )],
        None,
        current_freshness(),
    );
    assert_eq!(version.state, GameFirmwareState::WrongVersion);
}

#[test]
fn multiple_valid_options_are_not_arbitrarily_selected() {
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "flycast"),
        required_two_options(),
        vec![accepted_hash("us-v1"), accepted_hash("jp-v1")],
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::MultipleValidOptions);
}

#[test]
fn selected_option_is_respected() {
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "flycast"),
        required_two_options(),
        vec![accepted_hash("us-v1"), accepted_hash("jp-v1")],
        Some("jp-v1"),
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Ready);
    assert_eq!(result.selected_option.as_ref().unwrap().id, "jp-v1");
}

#[test]
fn emulator_managed_firmware_is_explicit() {
    let mut requirement = required();
    requirement.emulator_managed = true;
    let result = project(
        &candidate(FirmwareReadiness::NotRequired, "pcengine-cd"),
        requirement,
        Vec::new(),
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::EmulatorManaged);
}

#[test]
fn profile_and_firmware_changes_make_projection_stale() {
    let previous = snapshot();
    let mut changed = previous.clone();
    changed.profile_fingerprint = Some("profile-2".into());
    let profile = FirmwareFreshness::compare(&previous, changed);
    assert_eq!(profile.state, FirmwareFreshnessState::Stale);
    assert_eq!(
        profile.stale_reason,
        Some(FirmwareStaleReason::ProfileChanged)
    );

    let mut changed = previous.clone();
    changed.firmware_file_fingerprint = Some("file-2".into());
    let file = FirmwareFreshness::compare(&previous, changed);
    assert_eq!(
        file.stale_reason,
        Some(FirmwareStaleReason::FirmwareFileChanged)
    );
}

#[test]
fn stale_projection_does_not_claim_ready() {
    let previous = snapshot();
    let mut changed = previous.clone();
    changed.game_identity_fingerprint = Some("game-2".into());
    let freshness = FirmwareFreshness::compare(&previous, changed);
    let result = project(
        &candidate(FirmwareReadiness::Verified, "duckstation"),
        required(),
        vec![accepted_hash("us-v1")],
        None,
        freshness,
    );
    assert_eq!(result.state, GameFirmwareState::Stale);
}

#[test]
fn global_inventory_without_candidate_evidence_does_not_imply_ready() {
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "duckstation"),
        required(),
        Vec::new(),
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Unknown);
}

#[test]
fn unknown_is_preserved_for_unproven_adapters() {
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "retroarch:unknown-core"),
        required(),
        vec![evidence(
            None,
            FirmwareEvidenceStrength::FilenameOrPresenceOnly,
            FirmwareEvidenceStatus::Unknown,
        )],
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Unknown);
}

// ---- freshness fingerprints ------------------------------------------------

#[test]
fn freshness_is_stable_for_unchanged_evidence() {
    let previous = snapshot();
    let again = FirmwareFreshness::compare(&previous, snapshot());
    assert_eq!(again.state, FirmwareFreshnessState::Current);
    assert_eq!(again.stale_reason, None);
    // Comparing is a pure value operation: repeating it gives the same answer.
    assert_eq!(again, FirmwareFreshness::compare(&previous, snapshot()));
    assert_eq!(again, current_freshness());
}

#[test]
fn freshness_changes_for_each_relevant_component() {
    type Edit = fn(&mut FirmwareFreshnessSnapshot);
    let cases: [(Edit, FirmwareStaleReason); 5] = [
        (
            |s| s.profile_fingerprint = Some("other".into()),
            FirmwareStaleReason::ProfileChanged,
        ),
        (
            |s| s.firmware_path_fingerprint = Some("other".into()),
            FirmwareStaleReason::FirmwarePathChanged,
        ),
        (
            |s| s.firmware_file_fingerprint = Some("other".into()),
            FirmwareStaleReason::FirmwareFileChanged,
        ),
        (
            |s| s.game_identity_fingerprint = Some("other".into()),
            FirmwareStaleReason::GameIdentityChanged,
        ),
        (
            |s| s.requirement_fingerprint = Some("other".into()),
            FirmwareStaleReason::RequirementChanged,
        ),
    ];
    for (edit, expected) in cases {
        let previous = snapshot();
        let mut current = snapshot();
        edit(&mut current);
        let result = FirmwareFreshness::compare(&previous, current);
        assert_eq!(result.state, FirmwareFreshnessState::Stale);
        assert_eq!(result.stale_reason, Some(expected));
    }
}

#[test]
fn an_input_that_can_no_longer_be_gathered_is_reported_as_unavailable() {
    let previous = snapshot();
    let mut current = snapshot();
    current.firmware_file_fingerprint = None;
    let result = FirmwareFreshness::compare(&previous, current);
    assert_eq!(result.state, FirmwareFreshnessState::Stale);
    assert_eq!(
        result.stale_reason,
        Some(FirmwareStaleReason::InputUnavailable)
    );
}

#[test]
fn missing_fingerprints_mean_unknown_freshness_never_ready() {
    let freshness = FirmwareFreshness::current(FirmwareFreshnessSnapshot::default());
    assert_eq!(freshness.state, FirmwareFreshnessState::Unknown);
    let result = project(
        &candidate(FirmwareReadiness::Verified, "duckstation"),
        required(),
        vec![accepted_hash("us-v1")],
        None,
        freshness,
    );
    assert_eq!(result.state, GameFirmwareState::Unknown);
}

// ---- fail-closed hardening added in this port ------------------------------

#[test]
fn a_signature_and_size_match_alone_is_present_unverified() {
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "duckstation"),
        required(),
        vec![evidence(
            Some("us-v1"),
            FirmwareEvidenceStrength::StrongSignatureAndSize,
            FirmwareEvidenceStatus::Accepted,
        )],
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::PresentUnverified);
}

#[test]
fn accepted_firmware_contradicting_a_missing_candidate_is_unknown() {
    let result = project(
        &candidate(FirmwareReadiness::Missing, "pcsx2"),
        required(),
        vec![accepted_hash("us-v1")],
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Unknown);
}

#[test]
fn a_not_required_descriptor_contradicting_the_candidate_is_unknown() {
    for firmware in [
        FirmwareReadiness::Missing,
        FirmwareReadiness::PresentUnverified,
        FirmwareReadiness::Verified,
    ] {
        let result = project(
            &candidate(firmware, "ppsspp"),
            FirmwareRequirementDescriptor::not_required("ppsspp", "No external firmware"),
            Vec::new(),
            None,
            current_freshness(),
        );
        assert_eq!(result.state, GameFirmwareState::Unknown, "{firmware:?}");
    }
}

#[test]
fn a_bad_unrelated_option_does_not_mask_the_selected_one() {
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "flycast"),
        required_two_options(),
        vec![
            evidence(
                Some("us-v1"),
                FirmwareEvidenceStrength::ExactKnownHash,
                FirmwareEvidenceStatus::HashMismatch,
            ),
            accepted_hash("jp-v1"),
        ],
        Some("jp-v1"),
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Ready);
    // Without a choice the same evidence is still reported as a mismatch.
    let unselected = project(
        &candidate(FirmwareReadiness::Unknown, "flycast"),
        required_two_options(),
        vec![
            evidence(
                Some("us-v1"),
                FirmwareEvidenceStrength::ExactKnownHash,
                FirmwareEvidenceStatus::HashMismatch,
            ),
            accepted_hash("jp-v1"),
        ],
        None,
        current_freshness(),
    );
    assert_eq!(unselected.state, GameFirmwareState::HashMismatch);
}

#[test]
fn unverified_alternatives_are_not_multiple_valid_options() {
    let weak = |id: &str| {
        evidence(
            Some(id),
            FirmwareEvidenceStrength::FilenameOrPresenceOnly,
            FirmwareEvidenceStatus::Accepted,
        )
    };
    let result = project(
        &candidate(FirmwareReadiness::PresentUnverified, "flycast"),
        required_two_options(),
        vec![weak("us-v1"), weak("jp-v1")],
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::PresentUnverified);
}

#[test]
fn the_candidates_own_verified_firmware_is_consumed_as_authority() {
    let result = project(
        &candidate(FirmwareReadiness::Verified, "duckstation"),
        required(),
        Vec::new(),
        None,
        current_freshness(),
    );
    assert_eq!(result.state, GameFirmwareState::Ready);
}

// ---- identity, serialisation, purity ----------------------------------------

#[test]
fn retroarch_targets_get_a_stable_readable_profile_id() {
    let mut retro = candidate(FirmwareReadiness::Unknown, "unused");
    retro.target = LaunchTarget::RetroArchCore {
        profile: ProfileRef {
            profile_kind: ProfileKind::Flatpak,
            scope: ProfileScope::System,
        },
        core_stem: "mednafen_saturn".into(),
        platform_id: "Saturn",
    };
    let result = project(&retro, required(), Vec::new(), None, current_freshness());
    assert_eq!(result.emulator, "retroarch:mednafen_saturn");
    assert_eq!(result.profile_id.as_deref(), Some("flatpak:system"));
}

#[test]
fn states_serialise_as_stable_screaming_snake_names() {
    for (state, name) in [
        (GameFirmwareState::Ready, "READY"),
        (GameFirmwareState::PresentUnverified, "PRESENT_UNVERIFIED"),
        (GameFirmwareState::WrongRegion, "WRONG_REGION"),
        (GameFirmwareState::WrongVersion, "WRONG_VERSION"),
        (GameFirmwareState::HashMismatch, "HASH_MISMATCH"),
        (
            GameFirmwareState::MultipleValidOptions,
            "MULTIPLE_VALID_OPTIONS",
        ),
        (GameFirmwareState::EmulatorManaged, "EMULATOR_MANAGED"),
    ] {
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            format!("\"{name}\"")
        );
    }
}

#[test]
fn projection_carries_the_supplied_facts_through_unchanged() {
    let evidence_in = vec![accepted_hash("us-v1")];
    let requirement = required();
    let result = project(
        &candidate(FirmwareReadiness::Unknown, "duckstation"),
        requirement.clone(),
        evidence_in.clone(),
        Some("us-v1"),
        current_freshness(),
    );
    assert_eq!(result.evidence, evidence_in);
    assert_eq!(result.requirement, requirement);
    assert_eq!(result.freshness, current_freshness());
}

#[test]
fn projection_performs_no_writes() {
    // Evidence points at real files; projecting must not create, change or
    // delete anything under that directory (or anywhere else it could reach).
    let dir = tempfile::tempdir().unwrap();
    let firmware = dir.path().join("bios.bin");
    std::fs::write(&firmware, b"synthetic firmware bytes").unwrap();
    let before_bytes = std::fs::read(&firmware).unwrap();
    let before_modified = std::fs::metadata(&firmware).unwrap().modified().unwrap();
    let listing = |path: &std::path::Path| {
        let mut names: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before_listing = listing(dir.path());

    let mut item = accepted_hash("us-v1");
    item.path = Some(firmware.clone());
    let missing_path = dir.path().join("does-not-exist.bin");
    let mut absent = accepted_hash("us-v1");
    absent.path = Some(missing_path.clone());

    for freshness in [
        current_freshness(),
        FirmwareFreshness::current(Default::default()),
    ] {
        let _ = project(
            &candidate(FirmwareReadiness::Unknown, "duckstation"),
            required(),
            vec![item.clone(), absent.clone()],
            None,
            freshness,
        );
    }

    assert_eq!(std::fs::read(&firmware).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&firmware).unwrap().modified().unwrap(),
        before_modified
    );
    assert_eq!(listing(dir.path()), before_listing);
    assert!(!missing_path.exists(), "projection must not create files");
}
