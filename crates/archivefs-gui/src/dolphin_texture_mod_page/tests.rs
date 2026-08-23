//! Tests for the Dolphin texture-mod panel.
//!
//! State-transition tests construct `DolphinTextureModStage` values
//! directly (this submodule sees private items of its parent) rather than
//! simulating full pointer-click sequences - consistent with how sibling
//! read-only-render page test suites in this crate already work, and
//! sufficient here since the actual business logic each button click
//! invokes (`build_dolphin_texture_mod_preview`,
//! `build_shared_transaction_plan`, `execute_shared_apply`,
//! `execute_shared_rollback`) already has its own dedicated, real
//! transaction-path test coverage in `archivefs_core`.

use std::path::PathBuf;

use archivefs_core::game_identity::{
    GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityKind,
    IdentityPlatform, IdentityProvenance, IdentityStatus,
};
use archivefs_core::patch_manager::{
    DolphinInstallationType, DolphinProfileScope, DolphinSettingsDirectoryState,
    EmulatorDestinationDirectories, EmulatorInstallationType, EmulatorProfileConfidence,
    PreviewAdapter, ResolvedEmulatorProfile, SharedApplyContext, SharedApplyEntry,
    SharedApplyOutcome, SharedApplyResult, SharedApplyStatus, SharedPlanEntry, SharedPreviewReport,
    SharedTransactionPath, SharedTransactionStage,
};

use super::*;

fn provenance() -> IdentityProvenance {
    IdentityProvenance {
        archive_path: PathBuf::from("/library/game.iso"),
        member_path: None,
        member_index: None,
        method: "test fixture".to_string(),
    }
}

fn verified_game_id_report(game_id: &str) -> GameIdentityReport {
    GameIdentityReport {
        archive_path: PathBuf::from("/library/game.iso"),
        platform: IdentityPlatform::GameCube,
        format: IdentityImageFormat::Iso,
        evidence: vec![IdentityEvidence {
            kind: IdentityKind::DolphinGameId,
            status: IdentityStatus::Verified,
            value: Some(game_id.to_string()),
            confidence: IdentityConfidence::ExactBytes,
            provenance: provenance(),
            diagnostic: "test fixture evidence".to_string(),
        }],
        warnings: Vec::new(),
        bytes_read: 4096,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    }
}

fn sample_profile(eligible: bool) -> DolphinProfile {
    DolphinProfile {
        profile_id: "dolphin-native".to_string(),
        installation_type: DolphinInstallationType::Native,
        scope: DolphinProfileScope::User,
        configuration_path: PathBuf::from("/home/user/.local/share/dolphin-emu"),
        provenance: "test fixture".to_string(),
        eligible,
        blockers: Vec::new(),
        game_settings_path: PathBuf::from("/home/user/.local/share/dolphin-emu/GameSettings"),
        game_settings_state: DolphinSettingsDirectoryState::Available,
        game_settings_warning: None,
        configuration_identity: None,
        game_settings_identity: None,
        resolved: ResolvedEmulatorProfile {
            emulator_executable: None,
            installation_type: EmulatorInstallationType::NativeSystem,
            configuration_root: PathBuf::from("/home/user/.local/share/dolphin-emu"),
            data_user_root: PathBuf::from("/home/user/.local/share/dolphin-emu"),
            active_explicit_profile: None,
            destinations: EmulatorDestinationDirectories {
                cheats: None,
                patches: None,
                mods: Some(PathBuf::from("/home/user/.local/share/dolphin-emu/Load")),
                game_settings: None,
            },
            discovery_evidence: Vec::new(),
            confidence: EmulatorProfileConfidence::KnownPath,
            priority: 0,
            writable: true,
        },
    }
}

fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
    fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
            egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|clipped| shape_contains(&clipped.shape, needle))
}

fn render_panel(
    state: &mut DolphinTextureModPageState,
    archive_path: &Path,
    profile: &DolphinProfile,
    identity_report: Option<&GameIdentityReport>,
) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_dolphin_texture_mod_panel(ui, state, archive_path, profile, identity_report);
        });
    })
}

fn render_stage(stage: DolphinTextureModStage) -> egui::FullOutput {
    let mut stage = Some(stage);
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            match stage.take().expect("run called once") {
                DolphinTextureModStage::PreviewReady {
                    plan,
                    source_parent,
                } => {
                    show_preview(
                        ui,
                        &mut DolphinTextureModPageState::default(),
                        &plan,
                        &source_parent,
                        "dolphin-native",
                    );
                }
                DolphinTextureModStage::ConfirmationPending { plan } => {
                    show_confirmation(
                        ui,
                        &mut DolphinTextureModPageState::default(),
                        plan,
                        Path::new("/home/user/.local/share/dolphin-emu/Load/Textures"),
                    );
                }
                DolphinTextureModStage::Applied {
                    result,
                    destination_root,
                } => {
                    show_applied(
                        ui,
                        &mut DolphinTextureModPageState::default(),
                        result,
                        destination_root,
                    );
                }
                _ => {}
            }
        });
    })
}

// --- blocked prerequisites ----------------------------------------------------

#[test]
fn blocked_when_no_identity_report_is_loaded() {
    let mut state = DolphinTextureModPageState::default();
    let profile = sample_profile(true);
    let output = render_panel(&mut state, Path::new("/library/game.iso"), &profile, None);
    assert!(rendered_text_contains(
        &output,
        "Load ROM Identity & Evidence first."
    ));
}

#[test]
fn blocked_when_identity_is_unverified() {
    let mut state = DolphinTextureModPageState::default();
    let profile = sample_profile(true);
    let report = GameIdentityReport {
        archive_path: PathBuf::from("/library/game.iso"),
        platform: IdentityPlatform::GameCube,
        format: IdentityImageFormat::Iso,
        evidence: Vec::new(),
        warnings: Vec::new(),
        bytes_read: 0,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    };
    let output = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile,
        Some(&report),
    );
    assert!(rendered_text_contains(&output, "Identity unavailable"));
}

#[test]
fn blocked_when_profile_is_ineligible() {
    let mut state = DolphinTextureModPageState::default();
    let profile = sample_profile(false);
    let report = verified_game_id_report("GALE01");
    let output = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile,
        Some(&report),
    );
    assert!(rendered_text_contains(
        &output,
        "Dolphin profile unavailable"
    ));
}

// --- choose PNG action ---------------------------------------------------------

#[test]
fn idle_state_shows_the_choose_png_button() {
    let mut state = DolphinTextureModPageState::default();
    let profile = sample_profile(true);
    let report = verified_game_id_report("GALE01");
    let output = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile,
        Some(&report),
    );
    assert!(rendered_text_contains(&output, "Choose PNG"));
    assert!(rendered_text_contains(&output, "GALE01"));
}

// --- preview rendering/state -----------------------------------------------------

fn sample_source_entry(
    state: PreviewDestinationStateForTest,
) -> archivefs_core::patch_manager::SharedPreviewEntry {
    use archivefs_core::patch_manager::{
        PreviewDestinationState, PreviewEligibility, PreviewProposedAction, PreviewState,
    };
    let (destination_state, proposed_action, preview_state) = match state {
        PreviewDestinationStateForTest::Missing => (
            PreviewDestinationState::Missing,
            PreviewProposedAction::Install,
            PreviewState::InstallNew,
        ),
        PreviewDestinationStateForTest::Identical => (
            PreviewDestinationState::RegularFileIdentical,
            PreviewProposedAction::Skip,
            PreviewState::AlreadyInstalled,
        ),
        PreviewDestinationStateForTest::Different => (
            PreviewDestinationState::RegularFileDifferent,
            PreviewProposedAction::Replace,
            PreviewState::ReplaceDifferent,
        ),
    };
    archivefs_core::patch_manager::SharedPreviewEntry {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: PathBuf::from("/library/game.iso"),
        verified_identity: Some("GALE01".to_string()),
        match_strength: archivefs_core::patch_manager::PreviewMatchStrength::VerifiedExact,
        source_path: Some(PathBuf::from("/source/Metal.png")),
        source_digest: Some("digest".to_string()),
        destination_root: PathBuf::from("/home/user/.local/share/dolphin-emu/Load/Textures"),
        destination_relative_path: Some(PathBuf::from("GALE01/Metal.png")),
        destination_path: Some(PathBuf::from(
            "/home/user/.local/share/dolphin-emu/Load/Textures/GALE01/Metal.png",
        )),
        destination_state,
        existing_destination_digest: None,
        state: preview_state,
        proposed_action,
        eligibility: PreviewEligibility::Eligible,
        blockers: Vec::new(),
        warnings: Vec::new(),
        backup_required: false,
        explicit_replacement_permission_required: false,
    }
}

enum PreviewDestinationStateForTest {
    Missing,
    Identical,
    Different,
}

fn sample_report_with(state: PreviewDestinationStateForTest) -> SharedPreviewReport {
    SharedPreviewReport {
        request_archive: PathBuf::from("/library/game.iso"),
        adapter: PreviewAdapter::Dolphin,
        entries: vec![sample_source_entry(state)],
        conflicts: Vec::new(),
        warnings: Vec::new(),
        summary: Default::default(),
        complete: true,
    }
}

#[test]
fn missing_destination_preview_shows_install_button() {
    let plan = DolphinTextureModPlan::Install {
        report: sample_report_with(PreviewDestinationStateForTest::Missing),
    };
    let output = render_stage(DolphinTextureModStage::PreviewReady {
        plan,
        source_parent: PathBuf::from("/source"),
    });
    assert!(rendered_text_contains(&output, "Eligible to install"));
    assert!(rendered_text_contains(&output, "Install"));
}

#[test]
fn identical_destination_shows_already_installed() {
    let plan = DolphinTextureModPlan::AlreadyInstalled {
        report: sample_report_with(PreviewDestinationStateForTest::Identical),
    };
    let output = render_stage(DolphinTextureModStage::PreviewReady {
        plan,
        source_parent: PathBuf::from("/source"),
    });
    assert!(rendered_text_contains(&output, "Already installed"));
}

// --- hard conflict has no Install -------------------------------------------------

#[test]
fn hard_conflict_never_shows_an_install_button() {
    let plan = DolphinTextureModPlan::Conflict {
        report: sample_report_with(PreviewDestinationStateForTest::Different),
    };
    let output = render_stage(DolphinTextureModStage::PreviewReady {
        plan,
        source_parent: PathBuf::from("/source"),
    });
    assert!(rendered_text_contains(
        &output,
        "Different file already installed"
    ));
    assert!(!rendered_text_contains(&output, "Eligible to install"));
}

// --- install confirmation --------------------------------------------------------

#[test]
fn confirmation_pending_renders_confirm_and_cancel() {
    let plan = SharedTransactionPlan {
        schema_version: 1,
        plan_id: "plan-1".to_string(),
        context: SharedApplyContext {
            adapter: PreviewAdapter::Dolphin,
            selected_archive: SharedTransactionPath::from_path(Path::new("/library/game.iso")),
            verified_game_identity: "GALE01".to_string(),
            profile_id: "dolphin-native".to_string(),
            source_mode: DOLPHIN_TEXTURE_MOD_SOURCE_MODE.to_string(),
        },
        approved_source_root: SharedTransactionPath::from_path(Path::new("/source")),
        destination_root: SharedTransactionPath::from_path(Path::new(
            "/home/user/.local/share/dolphin-emu/Load/Textures",
        )),
        entries: vec![SharedPlanEntry {
            adapter: PreviewAdapter::Dolphin,
            selected_archive: SharedTransactionPath::from_path(Path::new("/library/game.iso")),
            verified_game_identity: "GALE01".to_string(),
            source_path: SharedTransactionPath::from_path(Path::new("/source/Metal.png")),
            source_digest: "digest".to_string(),
            destination_root: SharedTransactionPath::from_path(Path::new(
                "/home/user/.local/share/dolphin-emu/Load/Textures",
            )),
            destination_relative_path: SharedTransactionPath::from_path(Path::new(
                "GALE01/Metal.png",
            )),
            destination_pre_state: archivefs_core::patch_manager::PreviewDestinationState::Missing,
            destination_pre_digest: None,
            proposed_action: archivefs_core::patch_manager::PreviewProposedAction::Install,
            backup_required: false,
            parent_creation_approved: true,
            content_verification: None,
        }],
    };
    let output = render_stage(DolphinTextureModStage::ConfirmationPending { plan });
    assert!(rendered_text_contains(&output, "Confirm install"));
    assert!(rendered_text_contains(&output, "Cancel"));
}

// --- successful apply / undo ------------------------------------------------------

fn sample_apply_result(
    outcome: SharedApplyOutcome,
    journal_path: Option<PathBuf>,
) -> SharedApplyResult {
    let plan_entry = SharedPlanEntry {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: SharedTransactionPath::from_path(Path::new("/library/game.iso")),
        verified_game_identity: "GALE01".to_string(),
        source_path: SharedTransactionPath::from_path(Path::new("/source/Metal.png")),
        source_digest: "digest".to_string(),
        destination_root: SharedTransactionPath::from_path(Path::new(
            "/home/user/.local/share/dolphin-emu/Load/Textures",
        )),
        destination_relative_path: SharedTransactionPath::from_path(Path::new("GALE01/Metal.png")),
        destination_pre_state: archivefs_core::patch_manager::PreviewDestinationState::Missing,
        destination_pre_digest: None,
        proposed_action: archivefs_core::patch_manager::PreviewProposedAction::Install,
        backup_required: false,
        parent_creation_approved: true,
        content_verification: None,
    };
    let journal = archivefs_core::patch_manager::SharedApplyJournal {
        schema_version: 1,
        operation_id: "op-1".to_string(),
        plan_id: "plan-1".to_string(),
        timestamp_unix_seconds: 1_700_000_000,
        context: SharedApplyContext {
            adapter: PreviewAdapter::Dolphin,
            selected_archive: SharedTransactionPath::from_path(Path::new("/library/game.iso")),
            verified_game_identity: "GALE01".to_string(),
            profile_id: "dolphin-native".to_string(),
            source_mode: DOLPHIN_TEXTURE_MOD_SOURCE_MODE.to_string(),
        },
        approved_source_root: SharedTransactionPath::from_path(Path::new("/source")),
        destination_root: SharedTransactionPath::from_path(Path::new(
            "/home/user/.local/share/dolphin-emu/Load/Textures",
        )),
        created_root_directories: Vec::new(),
        dry_run: false,
        entries: vec![SharedApplyEntry {
            plan_entry,
            destination_existed_before_apply: Some(false),
            destination_parent_existed_before_apply: Some(false),
            observed_source_digest: Some("digest".to_string()),
            observed_destination_digest: None,
            backup_path: None,
            backup_digest: None,
            temporary_path: None,
            final_destination_digest: Some("digest".to_string()),
            created_directories: Vec::new(),
            replacement_approved: false,
            verification_succeeded: true,
            outcome,
            stages: vec![SharedTransactionStage::Success],
            warnings: Vec::new(),
            failures: Vec::new(),
        }],
        status: SharedApplyStatus::Success,
        rollback_operation_id: None,
    };
    SharedApplyResult {
        journal,
        journal_path,
        journal_failure: None,
    }
}

#[test]
fn successful_apply_renders_installed_and_undo_button() {
    let result = sample_apply_result(
        SharedApplyOutcome::InstalledNew,
        Some(PathBuf::from("/history/op-1.json")),
    );
    let output = render_stage(DolphinTextureModStage::Applied {
        result,
        destination_root: PathBuf::from("/home/user/.local/share/dolphin-emu/Load/Textures"),
    });
    assert!(rendered_text_contains(&output, "Installed"));
    assert!(rendered_text_contains(&output, "Undo"));
}

#[test]
fn apply_without_a_journal_path_never_offers_undo() {
    let result = sample_apply_result(SharedApplyOutcome::InstalledNew, None);
    let output = render_stage(DolphinTextureModStage::Applied {
        result,
        destination_root: PathBuf::from("/home/user/.local/share/dolphin-emu/Load/Textures"),
    });
    assert!(!rendered_text_contains(&output, "Undo"));
}

// --- state resets when archive/profile/GAMEID changes -----------------------------

#[test]
fn state_resets_when_the_verified_game_id_changes() {
    let mut state = DolphinTextureModPageState::default();
    let profile = sample_profile(true);
    let report_one = verified_game_id_report("GALE01");
    let _ = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile,
        Some(&report_one),
    );
    // Manually drive into a non-idle stage - this submodule sees the
    // parent's private items, exactly like driving it through real clicks
    // would, without needing a full pointer-event simulation.
    state.stage = Some(DolphinTextureModStage::Failed {
        detail: "an earlier attempt failed".to_string(),
    });

    let report_two = verified_game_id_report("RMCE01");
    let output = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile,
        Some(&report_two),
    );
    assert!(
        !rendered_text_contains(&output, "an earlier attempt failed"),
        "a different verified GameID must never reuse the previous game's stage"
    );
    assert!(rendered_text_contains(&output, "Choose PNG"));
}

#[test]
fn state_resets_when_the_archive_changes() {
    let mut state = DolphinTextureModPageState::default();
    let profile = sample_profile(true);
    let report = verified_game_id_report("GALE01");
    let _ = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile,
        Some(&report),
    );
    state.stage = Some(DolphinTextureModStage::Failed {
        detail: "an earlier attempt failed".to_string(),
    });

    let other_report = GameIdentityReport {
        archive_path: PathBuf::from("/library/other.iso"),
        ..verified_game_id_report("GALE01")
    };
    let output = render_panel(
        &mut state,
        Path::new("/library/other.iso"),
        &profile,
        Some(&other_report),
    );
    assert!(!rendered_text_contains(
        &output,
        "an earlier attempt failed"
    ));
    assert!(rendered_text_contains(&output, "Choose PNG"));
}

#[test]
fn state_resets_when_the_profile_changes() {
    let mut state = DolphinTextureModPageState::default();
    let profile_one = sample_profile(true);
    let report = verified_game_id_report("GALE01");
    let _ = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile_one,
        Some(&report),
    );
    state.stage = Some(DolphinTextureModStage::Failed {
        detail: "an earlier attempt failed".to_string(),
    });

    let mut profile_two = sample_profile(true);
    profile_two.profile_id = "dolphin-flatpak".to_string();
    let output = render_panel(
        &mut state,
        Path::new("/library/game.iso"),
        &profile_two,
        Some(&report),
    );
    assert!(!rendered_text_contains(
        &output,
        "an earlier attempt failed"
    ));
    assert!(rendered_text_contains(&output, "Choose PNG"));
}
