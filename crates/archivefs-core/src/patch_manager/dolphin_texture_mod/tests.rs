//! Tests for the Dolphin texture-mod slice.
//!
//! Identity/profile/source fixtures are built by hand (all fields are
//! public); the missing-root/install/rollback tests drive the real shared
//! transaction entry points (`build_shared_transaction_plan`,
//! `execute_shared_apply`, `preview_shared_rollback`,
//! `execute_shared_rollback`), never a private helper.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityKind, IdentityProvenance,
    IdentityStatus,
};
use crate::patch_manager::resolved_emulator_profile::{
    EmulatorDestinationDirectories, EmulatorInstallationType, EmulatorProfileConfidence,
    ResolvedEmulatorProfile,
};
use crate::patch_manager::shared_transaction::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyOutcome, SharedApplyStatus,
    SharedRollbackConfirmation, SharedRollbackOptions, build_shared_transaction_plan,
    execute_shared_apply, execute_shared_rollback, generate_shared_operation_id,
    preview_shared_rollback,
};
use crate::patch_manager::{
    DolphinInstallationType, DolphinProfileScope, DolphinSettingsDirectoryState,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "archivefs-dolphin-texture-mod-{tag}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Identity fixtures
// ---------------------------------------------------------------------------

fn provenance() -> IdentityProvenance {
    IdentityProvenance {
        archive_path: PathBuf::from("/library/game.iso"),
        member_path: None,
        member_index: None,
        method: "test fixture".to_string(),
    }
}

fn evidence(
    kind: IdentityKind,
    status: IdentityStatus,
    value: Option<&str>,
    confidence: IdentityConfidence,
) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        status,
        value: value.map(str::to_string),
        confidence,
        provenance: provenance(),
        diagnostic: "test fixture evidence".to_string(),
    }
}

fn report(
    archive_path: &str,
    platform: IdentityPlatform,
    evidence: Vec<IdentityEvidence>,
) -> GameIdentityReport {
    GameIdentityReport {
        archive_path: PathBuf::from(archive_path),
        platform,
        format: IdentityImageFormat::Iso,
        evidence,
        warnings: Vec::new(),
        bytes_read: 4096,
        archive_members_inspected: 0,
        metadata_paths_inspected: 0,
        nested_container_depth: 0,
        complete: true,
    }
}

fn verified_game_id_report(platform: IdentityPlatform, game_id: &str) -> GameIdentityReport {
    report(
        "/library/game.iso",
        platform,
        vec![evidence(
            IdentityKind::DolphinGameId,
            IdentityStatus::Verified,
            Some(game_id),
            IdentityConfidence::ExactBytes,
        )],
    )
}

// ---------------------------------------------------------------------------
// Profile fixtures
// ---------------------------------------------------------------------------

fn sample_profile(mods_root: Option<PathBuf>, eligible: bool) -> DolphinProfile {
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
                mods: mods_root,
                game_settings: None,
            },
            discovery_evidence: Vec::new(),
            confidence: EmulatorProfileConfidence::KnownPath,
            priority: 0,
            writable: true,
        },
    }
}

// --- GameCube verified GAMEID builds exact destination ----------------------

#[test]
fn gamecube_verified_game_id_builds_exact_destination() {
    let report = verified_game_id_report(IdentityPlatform::GameCube, "GALE01");
    let identity =
        verified_dolphin_texture_identity(&report, Path::new("/library/game.iso")).unwrap();
    assert_eq!(identity.game_id, "GALE01");
    assert_eq!(identity.platform, IdentityPlatform::GameCube);

    let profile = sample_profile(
        Some(PathBuf::from("/home/user/.local/share/dolphin-emu/Load")),
        true,
    );
    let root = dolphin_texture_mod_destination_root(&profile).unwrap();
    assert_eq!(
        root,
        PathBuf::from("/home/user/.local/share/dolphin-emu/Load/Textures")
    );
}

// --- Wii verified GAMEID builds exact destination ---------------------------

#[test]
fn wii_verified_game_id_builds_exact_destination() {
    let report = verified_game_id_report(IdentityPlatform::Wii, "RMCE01");
    let identity =
        verified_dolphin_texture_identity(&report, Path::new("/library/game.iso")).unwrap();
    assert_eq!(identity.game_id, "RMCE01");
    assert_eq!(identity.platform, IdentityPlatform::Wii);
}

// --- wrong platform blocks ---------------------------------------------------

#[test]
fn wrong_platform_blocks() {
    let report = verified_game_id_report(IdentityPlatform::PlayStation2, "SLUS-98765");
    let result = verified_dolphin_texture_identity(&report, Path::new("/library/game.iso"));
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::WrongPlatform
    );
}

// --- missing/candidate/ambiguous/stale identity blocks -----------------------

#[test]
fn missing_identity_blocks() {
    let report = report(
        "/library/game.iso",
        IdentityPlatform::GameCube,
        vec![evidence(
            IdentityKind::DolphinGameId,
            IdentityStatus::Missing,
            None,
            IdentityConfidence::Unavailable,
        )],
    );
    let result = verified_dolphin_texture_identity(&report, Path::new("/library/game.iso"));
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::IdentityUnverified
    );
}

#[test]
fn candidate_identity_blocks() {
    let report = report(
        "/library/game.iso",
        IdentityPlatform::GameCube,
        vec![evidence(
            IdentityKind::DolphinGameId,
            IdentityStatus::Candidate,
            Some("GALE01"),
            IdentityConfidence::CatalogueContext,
        )],
    );
    let result = verified_dolphin_texture_identity(&report, Path::new("/library/game.iso"));
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::IdentityUnverified
    );
}

#[test]
fn ambiguous_identity_blocks() {
    let report = report(
        "/library/game.iso",
        IdentityPlatform::GameCube,
        vec![evidence(
            IdentityKind::DolphinGameId,
            IdentityStatus::Ambiguous,
            Some("GALE01"),
            IdentityConfidence::CatalogueContext,
        )],
    );
    let result = verified_dolphin_texture_identity(&report, Path::new("/library/game.iso"));
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::IdentityUnverified
    );
}

#[test]
fn stale_identity_for_a_different_archive_blocks() {
    let report = verified_game_id_report(IdentityPlatform::GameCube, "GALE01");
    let result = verified_dolphin_texture_identity(&report, Path::new("/library/other.iso"));
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::IdentityArchiveMismatch
    );
}

// --- unsafe GAMEID blocks -----------------------------------------------------

#[test]
fn unsafe_game_id_blocks() {
    let report = verified_game_id_report(IdentityPlatform::GameCube, "../escape");
    let result = verified_dolphin_texture_identity(&report, Path::new("/library/game.iso"));
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::UnsafeGameId
    );
}

// --- ineligible profile blocks -----------------------------------------------

#[test]
fn ineligible_profile_blocks() {
    let profile = sample_profile(
        Some(PathBuf::from("/home/user/.local/share/dolphin-emu/Load")),
        false,
    );
    let result = dolphin_texture_mod_destination_root(&profile);
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::ProfileIneligible
    );
}

#[test]
fn missing_mods_root_blocks() {
    let profile = sample_profile(None, true);
    let result = dolphin_texture_mod_destination_root(&profile);
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::ProfileRootUnavailable
    );
}

// --- source non-PNG blocks ----------------------------------------------------

#[test]
fn source_non_png_blocks() {
    let dir = TestDir::new("non-png");
    let path = dir.0.join("texture.jpg");
    std::fs::write(&path, b"not a png").unwrap();
    let result = validate_dolphin_texture_source(&path);
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::SourceNotPng
    );
}

// --- source symlink blocks -----------------------------------------------------

#[cfg(unix)]
#[test]
fn source_symlink_blocks() {
    use std::os::unix::fs::symlink;
    let dir = TestDir::new("symlink");
    let real = dir.0.join("real.png");
    std::fs::write(&real, b"png bytes").unwrap();
    let link = dir.0.join("link.png");
    symlink(&real, &link).unwrap();
    let result = validate_dolphin_texture_source(&link);
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::SourceSymlink
    );
}

// --- oversized file blocks -----------------------------------------------------

#[test]
fn oversized_file_blocks() {
    let dir = TestDir::new("oversized");
    let path = dir.0.join("huge.png");
    let bytes = vec![0u8; (SHARED_MAX_SOURCE_BYTES + 1) as usize];
    std::fs::write(&path, &bytes).unwrap();
    let result = validate_dolphin_texture_source(&path);
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::SourceTooLarge
    );
}

// --- unsafe filename blocks -----------------------------------------------------

#[cfg(unix)]
#[test]
fn unsafe_non_utf8_filename_blocks() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let dir = TestDir::new("unsafe-filename");
    let name = OsString::from_vec(vec![b't', b'e', 0xff, b'.', b'p', b'n', b'g']);
    let path = dir.0.join(&name);
    std::fs::write(&path, b"png bytes").unwrap();
    let result = validate_dolphin_texture_source(&path);
    assert_eq!(
        result.unwrap_err().kind,
        DolphinTextureModErrorKind::SourceUnsafeFilename
    );
}

// --- valid source accepted ------------------------------------------------------

#[test]
fn valid_png_source_is_accepted() {
    let dir = TestDir::new("valid-source");
    let path = dir.0.join("Metal.png");
    std::fs::write(&path, b"png bytes").unwrap();
    let source = validate_dolphin_texture_source(&path).unwrap();
    assert_eq!(source.file_name, "Metal.png");
}

// ---------------------------------------------------------------------------
// Full transaction-path tests
// ---------------------------------------------------------------------------

fn dolphin_apply_options(
    history_root: PathBuf,
    backup_root: PathBuf,
    plan: &crate::patch_manager::shared_transaction::SharedTransactionPlan,
    replacement_approved: bool,
) -> SharedApplyOptions {
    SharedApplyOptions {
        dry_run: false,
        confirmation: Some(SharedApplyConfirmation {
            plan_id: plan.plan_id.clone(),
            general_approved: true,
            replacement_approved,
        }),
        operation_id: generate_shared_operation_id(),
        timestamp_unix_seconds: 1_700_000_000,
        current_context: plan.context.clone(),
        history_root,
        backup_root,
    }
}

/// Builds a real preview + plan for one texture install against `mods_root`
/// (the profile's `Load` directory - the destination root ends up
/// `mods_root/Textures`), using the exact same core functions the GUI will
/// call.
fn build_real_plan(
    archive: &Path,
    game_id: &str,
    mods_root: &Path,
    source_dir: &Path,
    file_name: &str,
    bytes: &[u8],
) -> crate::patch_manager::shared_transaction::SharedTransactionPlan {
    let source_path = source_dir.join(file_name);
    std::fs::write(&source_path, bytes).unwrap();

    let report = verified_game_id_report(IdentityPlatform::GameCube, game_id);
    let identity = verified_dolphin_texture_identity(&report, archive).unwrap();
    let profile = sample_profile(Some(mods_root.to_path_buf()), true);
    let destination_root = dolphin_texture_mod_destination_root(&profile).unwrap();
    let source = validate_dolphin_texture_source(&source_path).unwrap();

    let request = DolphinTextureModPreviewRequest {
        selected_archive: archive.to_path_buf(),
        identity,
        destination_root,
        source,
    };
    let plan = build_dolphin_texture_mod_preview(&request).unwrap();
    let DolphinTextureModPlan::Install { report } = plan else {
        panic!("expected an installable plan");
    };
    build_shared_transaction_plan(
        &report,
        &profile.profile_id,
        DOLPHIN_TEXTURE_MOD_SOURCE_MODE,
        source_dir,
    )
    .unwrap()
}

// --- missing destination -> Install ---------------------------------------------

#[test]
fn missing_destination_is_eligible_to_install() {
    let dir = TestDir::new("missing-destination");
    let source_dir = dir.0.join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let mods_root = dir.0.join("dolphin-load");
    std::fs::create_dir_all(&mods_root).unwrap();

    let report = verified_game_id_report(IdentityPlatform::GameCube, "GALE01");
    let identity =
        verified_dolphin_texture_identity(&report, Path::new("/library/game.iso")).unwrap();
    let profile = sample_profile(Some(mods_root.clone()), true);
    let destination_root = dolphin_texture_mod_destination_root(&profile).unwrap();
    let source_path = source_dir.join("Metal.png");
    std::fs::write(&source_path, b"png bytes").unwrap();
    let source = validate_dolphin_texture_source(&source_path).unwrap();

    let request = DolphinTextureModPreviewRequest {
        selected_archive: PathBuf::from("/library/game.iso"),
        identity,
        destination_root,
        source,
    };
    let plan = build_dolphin_texture_mod_preview(&request).unwrap();
    assert!(matches!(plan, DolphinTextureModPlan::Install { .. }));
}

// --- identical destination -> AlreadyInstalled/no transaction -------------------

#[test]
fn identical_destination_is_already_installed_with_no_transaction() {
    let dir = TestDir::new("identical-destination");
    let source_dir = dir.0.join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let mods_root = dir.0.join("dolphin-load");
    std::fs::create_dir_all(&mods_root).unwrap();
    let existing = mods_root.join("Textures").join("GALE01");
    std::fs::create_dir_all(&existing).unwrap();
    std::fs::write(existing.join("Metal.png"), b"same bytes").unwrap();

    let report = verified_game_id_report(IdentityPlatform::GameCube, "GALE01");
    let identity =
        verified_dolphin_texture_identity(&report, Path::new("/library/game.iso")).unwrap();
    let profile = sample_profile(Some(mods_root.clone()), true);
    let destination_root = dolphin_texture_mod_destination_root(&profile).unwrap();
    let source_path = source_dir.join("Metal.png");
    std::fs::write(&source_path, b"same bytes").unwrap();
    let source = validate_dolphin_texture_source(&source_path).unwrap();

    let request = DolphinTextureModPreviewRequest {
        selected_archive: PathBuf::from("/library/game.iso"),
        identity,
        destination_root,
        source,
    };
    let plan = build_dolphin_texture_mod_preview(&request).unwrap();
    assert!(matches!(
        plan,
        DolphinTextureModPlan::AlreadyInstalled { .. }
    ));
}

// --- different destination -> HardConflict/no Replace ----------------------------

#[test]
fn different_destination_is_a_hard_conflict_never_a_replace() {
    let dir = TestDir::new("different-destination");
    let source_dir = dir.0.join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let mods_root = dir.0.join("dolphin-load");
    std::fs::create_dir_all(&mods_root).unwrap();
    let existing = mods_root.join("Textures").join("GALE01");
    std::fs::create_dir_all(&existing).unwrap();
    std::fs::write(existing.join("Metal.png"), b"old bytes").unwrap();

    let report = verified_game_id_report(IdentityPlatform::GameCube, "GALE01");
    let identity =
        verified_dolphin_texture_identity(&report, Path::new("/library/game.iso")).unwrap();
    let profile = sample_profile(Some(mods_root.clone()), true);
    let destination_root = dolphin_texture_mod_destination_root(&profile).unwrap();
    let source_path = source_dir.join("Metal.png");
    std::fs::write(&source_path, b"new bytes").unwrap();
    let source = validate_dolphin_texture_source(&source_path).unwrap();

    let request = DolphinTextureModPreviewRequest {
        selected_archive: PathBuf::from("/library/game.iso"),
        identity,
        destination_root,
        source,
    };
    let plan = build_dolphin_texture_mod_preview(&request).unwrap();
    assert!(matches!(plan, DolphinTextureModPlan::Conflict { .. }));
    // The wrapper never proposes `Replace` as something this feature would
    // ever act on - `Conflict` is exactly that policy decision.
}

// --- fresh missing Load/Textures root installs through the bootstrap system -----

#[test]
fn fresh_missing_load_textures_root_installs_through_the_bootstrap_system() {
    let dir = TestDir::new("fresh-root-bootstrap");
    let source_dir = dir.0.join("source");
    std::fs::create_dir(&source_dir).unwrap();
    // `mods_root` itself does not exist yet - neither does `Textures` nor
    // `Textures/GALE01` beneath it. The whole chain must be created by the
    // committed shared-transaction root-bootstrap support.
    let mods_root = dir.0.join("dolphin-load");
    assert!(!mods_root.exists());

    let plan = build_real_plan(
        Path::new("/library/game.iso"),
        "GALE01",
        &mods_root,
        &source_dir,
        "Metal.png",
        b"png bytes",
    );
    let history_root = dir.0.join("history");
    let backup_root = dir.0.join("backups");
    let result = execute_shared_apply(
        &plan,
        &dolphin_apply_options(history_root, backup_root, &plan, false),
    );
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    let installed = mods_root.join("Textures").join("GALE01").join("Metal.png");
    assert!(installed.is_file());
    assert_eq!(std::fs::read(&installed).unwrap(), b"png bytes");
}

// --- transaction journal contains correct context --------------------------------

#[test]
fn transaction_journal_contains_correct_context() {
    let dir = TestDir::new("journal-context");
    let source_dir = dir.0.join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let mods_root = dir.0.join("dolphin-load");

    let plan = build_real_plan(
        Path::new("/library/game.iso"),
        "GALE01",
        &mods_root,
        &source_dir,
        "Metal.png",
        b"png bytes",
    );
    let history_root = dir.0.join("history");
    let backup_root = dir.0.join("backups");
    let result = execute_shared_apply(
        &plan,
        &dolphin_apply_options(history_root, backup_root, &plan, false),
    );
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    assert_eq!(
        result.journal.context.adapter,
        crate::patch_manager::shared_preview::PreviewAdapter::Dolphin
    );
    assert_eq!(
        result
            .journal
            .context
            .selected_archive
            .to_path_buf()
            .unwrap(),
        PathBuf::from("/library/game.iso")
    );
    assert_eq!(result.journal.context.verified_game_identity, "GALE01");
    assert_eq!(result.journal.context.profile_id, "dolphin-native");
    assert_eq!(
        result.journal.context.source_mode,
        DOLPHIN_TEXTURE_MOD_SOURCE_MODE
    );
}

// --- rollback removes unchanged EmuWiz-installed PNG -------------------------------

#[test]
fn rollback_removes_unchanged_installed_png() {
    let dir = TestDir::new("rollback-removes");
    let source_dir = dir.0.join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let mods_root = dir.0.join("dolphin-load");

    let plan = build_real_plan(
        Path::new("/library/game.iso"),
        "GALE01",
        &mods_root,
        &source_dir,
        "Metal.png",
        b"png bytes",
    );
    let history_root = dir.0.join("history");
    let backup_root = dir.0.join("backups");
    let result = execute_shared_apply(
        &plan,
        &dolphin_apply_options(history_root.clone(), backup_root.clone(), &plan, false),
    );
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    assert_eq!(
        result.journal.entries[0].outcome,
        SharedApplyOutcome::InstalledNew
    );
    let journal_path = result.journal_path.unwrap();
    let destination_root = mods_root.join("Textures");
    let rollback = preview_shared_rollback(&journal_path, &destination_root, &backup_root);
    assert!(rollback.available);
    let rolled_back = execute_shared_rollback(
        &rollback,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: rollback.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 1_700_000_001,
            history_root,
            backup_root,
        },
    );
    assert_eq!(rolled_back.status, SharedApplyStatus::Success);
    let installed = mods_root.join("Textures").join("GALE01").join("Metal.png");
    assert!(!installed.exists());
}

// --- rollback refuses modified destination ------------------------------------------

#[test]
fn rollback_refuses_a_modified_destination() {
    let dir = TestDir::new("rollback-refuses-modified");
    let source_dir = dir.0.join("source");
    std::fs::create_dir(&source_dir).unwrap();
    let mods_root = dir.0.join("dolphin-load");

    let plan = build_real_plan(
        Path::new("/library/game.iso"),
        "GALE01",
        &mods_root,
        &source_dir,
        "Metal.png",
        b"png bytes",
    );
    let history_root = dir.0.join("history");
    let backup_root = dir.0.join("backups");
    let result = execute_shared_apply(
        &plan,
        &dolphin_apply_options(history_root.clone(), backup_root.clone(), &plan, false),
    );
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    let journal_path = result.journal_path.unwrap();
    let installed = mods_root.join("Textures").join("GALE01").join("Metal.png");

    // The user (or Dolphin itself) modifies the installed texture after
    // install - rollback must never discard that change.
    std::fs::write(&installed, b"modified by someone else").unwrap();

    let destination_root = mods_root.join("Textures");
    let rollback = preview_shared_rollback(&journal_path, &destination_root, &backup_root);
    let rolled_back = execute_shared_rollback(
        &rollback,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: rollback.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: 1_700_000_001,
            history_root,
            backup_root,
        },
    );
    assert_ne!(rolled_back.status, SharedApplyStatus::Success);
    assert_eq!(
        std::fs::read(&installed).unwrap(),
        b"modified by someone else"
    );
}
