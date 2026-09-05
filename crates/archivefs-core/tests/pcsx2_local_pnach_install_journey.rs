//! End-to-end proof that a user-selected local PCSX2 `.pnach` file flows
//! through the exact, unmodified `pcsx2_install_plan`/`shared_transaction`
//! pipeline via `local_cheat_install_pcsx2`'s bridge, with no new write
//! engine and no new safety behavior introduced along the way.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::patch_manager::{
    LocalPcsx2FileError, LocalPcsx2InstallState, Pcsx2GameIdentity, Pcsx2IdentityState,
    Pcsx2InstallPreviewRequest, Pcsx2InstallationType, Pcsx2PatchCategory, Pcsx2PatchDirectory,
    Pcsx2PatchDirectoryState, Pcsx2Profile, Pcsx2ProfileScope, SharedApplyConfirmation,
    SharedApplyOptions, SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions,
    build_pcsx2_install_preview, build_shared_transaction_plan, check_local_pcsx2_install_state,
    discover_local_pcsx2_pnach_file, execute_shared_apply, execute_shared_rollback,
    preview_shared_rollback, stage_pcsx2_pnach,
};

const VALID_PNACH: &str =
    "gametitle=Fixture Game\ncomment=Infinite Health\npatch=1,EE,20123456,word,00000064\n";
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "archivefs-pcsx2-local-journey-{label}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("profile")).unwrap();
        fs::write(
            root.join("profile/game.iso"),
            b"immutable game image fixture",
        )
        .unwrap();
        Self(root)
    }

    fn path(&self, value: &str) -> PathBuf {
        self.0.join(value)
    }

    fn profile_root(&self) -> PathBuf {
        self.0.join("profile")
    }

    fn write(&self, value: &str, contents: &str) -> PathBuf {
        let path = self.path(value);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn profile(&self) -> Pcsx2Profile {
        let cheats = self.profile_root().join("cheats");
        Pcsx2Profile {
            profile_id: "fixture-profile".to_string(),
            installation_type: Pcsx2InstallationType::Portable,
            scope: Pcsx2ProfileScope::Portable,
            configuration_path: self.profile_root(),
            provenance: "disposable integration fixture",
            eligible: true,
            blockers: Vec::new(),
            patch_directories: vec![Pcsx2PatchDirectory {
                state: if cheats.exists() {
                    Pcsx2PatchDirectoryState::Available
                } else {
                    Pcsx2PatchDirectoryState::Missing
                },
                path: cheats,
                category: Pcsx2PatchCategory::Cheats,
                warning: None,
                identity: None,
            }],
            configuration_identity: None,
            executable_candidates: Vec::new(),
        }
    }

    fn identity(&self) -> Pcsx2GameIdentity {
        Pcsx2GameIdentity {
            archive_path: self.profile_root().join("game.iso"),
            title: "Fixture Game".to_string(),
            region: Some("NTSC-U".to_string()),
            serial: Some("SLUS-20312".to_string()),
            executable_crc: Some("A1B2C3D4".to_string()),
            state: Pcsx2IdentityState::Verified,
            evidence: vec!["exact fixture bytes".to_string()],
            plain_failure_reason: None,
        }
    }

    fn destination(&self) -> PathBuf {
        self.profile_root().join("cheats/SLUS-20312_A1B2C3D4.pnach")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_locally_selected_pnach_file_installs_and_is_fully_undoable() {
    let fixture = Fixture::new("happy-path");
    let local_file = fixture.write("downloads/SLUS-20312_A1B2C3D4.pnach", VALID_PNACH);
    let identity = fixture.identity();
    let profile = fixture.profile();

    let discovery =
        discover_local_pcsx2_pnach_file(&local_file, &identity).expect("discovery succeeds");
    assert_eq!(
        check_local_pcsx2_install_state(&profile, &discovery).unwrap(),
        LocalPcsx2InstallState::New
    );
    assert!(local_file.exists(), "the source file is never moved");
    assert_eq!(fs::read_to_string(&local_file).unwrap(), VALID_PNACH);

    let staged = stage_pcsx2_pnach(
        &fixture.path("staging"),
        &profile,
        discovery.detected_serial.as_deref(),
        &discovery.detected_crc,
        std::slice::from_ref(&discovery.cheat),
    )
    .unwrap();
    assert!(
        !fixture.destination().exists(),
        "staging never writes the destination"
    );

    let preview = build_pcsx2_install_preview(&Pcsx2InstallPreviewRequest {
        selected_archive: identity.archive_path.clone(),
        profile: profile.clone(),
        identity: identity.clone(),
        staged,
    })
    .expect("preview succeeds");
    assert_eq!(preview.report.summary.blocked, 0);
    assert!(
        !fixture.destination().exists(),
        "preview never writes the destination"
    );

    // Explicit confirmation is required: apply only ever runs against a
    // plan built from this exact preview report, approved by its plan_id.
    let plan = build_shared_transaction_plan(
        &preview.report,
        "fixture-profile",
        "pcsx2-local-file",
        &preview.staged.staging_root,
    )
    .unwrap();
    let result = execute_shared_apply(
        &plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: true,
                replacement_approved: true,
            }),
            operation_id: "pcsx2-local-apply".to_string(),
            timestamp_unix_seconds: 1_700_000_000,
            current_context: plan.context.clone(),
            history_root: fixture.path("history"),
            backup_root: fixture.path("backups"),
        },
    );
    assert_eq!(
        result.journal.status,
        SharedApplyStatus::Success,
        "entries: {:#?}",
        result.journal.entries
    );
    let journal_path = result
        .journal_path
        .as_ref()
        .expect("a successful apply journals");
    assert!(journal_path.is_file());
    let installed = fs::read_to_string(fixture.destination()).unwrap();
    assert!(installed.contains(&format!(
        "// ArchiveFS managed block: {}",
        discovery.cheat.id
    )));
    assert!(installed.contains("patch=1,EE,20123456,word,00000064"));

    let rollback_preview = preview_shared_rollback(
        journal_path,
        &fixture.profile_root(),
        &fixture.path("backups"),
    );
    assert!(rollback_preview.available);
    let rollback = execute_shared_rollback(
        &rollback_preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: rollback_preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: "pcsx2-local-undo".to_string(),
            timestamp_unix_seconds: 1_700_000_100,
            history_root: fixture.path("history"),
            backup_root: fixture.path("backups"),
        },
    );
    assert_eq!(rollback.status, SharedApplyStatus::Success);
    assert!(
        !fixture.destination().exists(),
        "rollback restores the prior (absent) state"
    );
}

#[test]
fn reapplying_the_same_local_file_is_reported_as_already_installed_not_a_conflict() {
    let fixture = Fixture::new("already-installed");
    let local_file = fixture.write("downloads/SLUS-20312_A1B2C3D4.pnach", VALID_PNACH);
    let identity = fixture.identity();
    let profile = fixture.profile();

    let first = discover_local_pcsx2_pnach_file(&local_file, &identity).unwrap();
    assert_eq!(
        check_local_pcsx2_install_state(&profile, &first).unwrap(),
        LocalPcsx2InstallState::New
    );
    let staged = stage_pcsx2_pnach(
        &fixture.path("staging"),
        &profile,
        first.detected_serial.as_deref(),
        &first.detected_crc,
        std::slice::from_ref(&first.cheat),
    )
    .unwrap();
    fs::create_dir_all(staged.destination_path.parent().unwrap()).unwrap();
    fs::write(&staged.destination_path, &staged.contents).unwrap();

    // Re-run discovery over the exact same file: same content hash, same
    // managed block ID, and the destination now already contains it.
    let second = discover_local_pcsx2_pnach_file(&local_file, &identity).unwrap();
    assert_eq!(second.cheat.id, first.cheat.id);
    assert_eq!(
        check_local_pcsx2_install_state(&profile, &second).unwrap(),
        LocalPcsx2InstallState::AlreadyInstalled,
        "identical content at the same destination is an idempotent no-op, never a conflict"
    );
}

#[test]
fn wrong_game_crc_is_blocked_before_any_write() {
    let fixture = Fixture::new("wrong-crc");
    let local_file = fixture.write("downloads/DEADBEEF.pnach", VALID_PNACH);
    let identity = fixture.identity(); // verified CRC is A1B2C3D4, file targets DEADBEEF

    let error = discover_local_pcsx2_pnach_file(&local_file, &identity).unwrap_err();
    assert!(matches!(
        error,
        LocalPcsx2FileError::IdentityConflict { .. }
    ));
    assert!(!fixture.profile_root().join("cheats").exists());
}

#[test]
fn unresolved_game_identity_is_blocked_before_any_write() {
    let fixture = Fixture::new("unresolved");
    let local_file = fixture.write("downloads/A1B2C3D4.pnach", VALID_PNACH);
    let mut identity = fixture.identity();
    identity.state = Pcsx2IdentityState::MissingCrc;
    identity.executable_crc = None;

    let error = discover_local_pcsx2_pnach_file(&local_file, &identity).unwrap_err();
    assert!(matches!(
        error,
        LocalPcsx2FileError::IdentityUnresolved { .. }
    ));
    assert!(!fixture.profile_root().join("cheats").exists());
}

#[test]
fn malformed_pnach_is_rejected_before_any_write() {
    let fixture = Fixture::new("malformed");
    let local_file = fixture.write("downloads/A1B2C3D4.pnach", "not a pnach file\n");
    let identity = fixture.identity();

    let error = discover_local_pcsx2_pnach_file(&local_file, &identity).unwrap_err();
    assert!(matches!(error, LocalPcsx2FileError::Malformed { .. }));
    assert!(!fixture.profile_root().join("cheats").exists());
}

#[test]
fn symlinked_source_is_rejected_before_any_write() {
    let fixture = Fixture::new("symlink");
    let real = fixture.write("downloads/real.pnach", VALID_PNACH);
    let link = fixture.path("downloads/A1B2C3D4.pnach");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let identity = fixture.identity();
        let error = discover_local_pcsx2_pnach_file(&link, &identity).unwrap_err();
        assert!(matches!(error, LocalPcsx2FileError::IsSymlink { .. }));
    }
    assert!(!fixture.profile_root().join("cheats").exists());
}
