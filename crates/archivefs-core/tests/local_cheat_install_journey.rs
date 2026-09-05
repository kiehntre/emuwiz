//! End-to-end proof that a user-selected local `.cht` file flows through
//! the exact, unmodified `cheat_journey` discover -> select -> preview ->
//! apply -> undo pipeline via `local_cheat_install`'s bridge, with no new
//! safety behavior introduced along the way.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::patch_manager::{
    CheatCandidateArchive, CheatCandidateClassification, CheatCandidateOptions,
    CheatDestinationRequest, CheatJourneyApplyApproval, CheatJourneyApplyOptions,
    CheatJourneyGameIdentity, CheatJourneyIdentityEvidence, CheatJourneyIdentityEvidenceKind,
    CheatJourneyIdentityState, CheatJourneyUndoConfirmation, CheatJourneyUndoOptions,
    LocalCheatFileError, SharedApplyStatus, apply_cheat_journey,
    discover_local_retroarch_cheat_file, preview_cheat_journey, preview_cheat_journey_undo,
    select_cheat_journey_candidate, undo_cheat_journey,
};

const PLATFORM: &str = "SNES";
const VALID_CHT: &str = "cheats = 1\ncheat0_desc = \"Infinite Health\"\ncheat0_code = \"AAAA\"\ncheat0_enable = false\n";
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "archivefs-local-cheat-install-{label}-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn path(&self, value: &str) -> PathBuf {
        self.0.join(value)
    }

    fn write(&self, value: &str, contents: &str) -> PathBuf {
        let path = self.path(value);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn game(&self) -> CheatJourneyGameIdentity {
        CheatJourneyGameIdentity {
            state: CheatJourneyIdentityState::Verified,
            selected_archive: self.write("library/Chrono Quest (USA).sfc", "rom"),
            identity_key: "library:chrono-quest-usa:sha256:deadbeef".into(),
            archive: CheatCandidateArchive {
                display_name: "Chrono Quest (USA)".into(),
                platform: Some(PLATFORM.into()),
                region: Some("USA".into()),
                content_basename: Some("Chrono Quest (USA)".into()),
                ..CheatCandidateArchive::default()
            },
            evidence: vec![CheatJourneyIdentityEvidence {
                kind: CheatJourneyIdentityEvidenceKind::CanonicalLibraryRecord,
                value: "game-42".into(),
            }],
        }
    }

    fn destination(&self) -> CheatDestinationRequest {
        let root = self.path("retroarch/cheats");
        fs::create_dir_all(root.join(PLATFORM)).unwrap();
        CheatDestinationRequest {
            profile_cheat_root: root,
            platform: Some(PLATFORM.into()),
            content_basename: Some("Chrono Quest (USA)".into()),
            playlist_name: None,
            catalogue_name: "Chrono Quest (USA)".into(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_locally_selected_cht_file_installs_and_is_fully_undoable() {
    let fixture = Fixture::new("happy-path");
    let local_file = fixture.write("downloads/Chrono Quest (USA).cht", VALID_CHT);

    let found = discover_local_retroarch_cheat_file(
        &HostReadOnlyFilesystem,
        &local_file,
        &fixture.game(),
        &CheatCandidateOptions::default(),
    )
    .expect("local file discovery succeeds");
    let candidate = found.candidate().expect("candidate present");
    assert_ne!(
        candidate.classification,
        CheatCandidateClassification::Unsupported
    );
    assert_ne!(
        candidate.classification,
        CheatCandidateClassification::CrossPlatform
    );
    assert!(candidate.manually_selectable);

    let mut selection = select_cheat_journey_candidate(
        &found.discovery,
        &found.location.catalogue_root,
        &found.location.catalogue_relative_path,
    )
    .expect("selection succeeds");
    assert!(selection.cheat_selection.set_selected(0, true));

    let request = fixture.destination();
    let preview = preview_cheat_journey(
        &selection,
        &found.location.catalogue_root,
        request.clone(),
        "retroarch-main",
        "local file",
    )
    .expect("preview succeeds");
    assert!(
        !fixture.path("managed/staging").exists(),
        "preview never stages or writes"
    );
    assert!(!preview.rendered_contents.is_empty());
    assert!(local_file.exists(), "the source file is never moved");
    assert_eq!(fs::read_to_string(&local_file).unwrap(), VALID_CHT);

    // Explicit confirmation is required: apply is only ever called with an
    // approval whose `preview_id` matches the exact preview just shown.
    let applied = apply_cheat_journey(
        &preview,
        &found.location.catalogue_root,
        &CheatJourneyApplyApproval {
            preview_id: preview.preview_id.clone(),
            approved: true,
            replacement_approved: false,
        },
        &CheatJourneyApplyOptions {
            staging_root: fixture.path("managed/staging"),
            operation_id: "local-cheat-apply".into(),
            timestamp_unix_seconds: 1_700_000_000,
            history_root: fixture.path("managed/history"),
            backup_root: fixture.path("managed/backups"),
        },
    )
    .expect("apply succeeds");
    assert_eq!(applied.result.journal.status, SharedApplyStatus::Success);
    let journal = applied.result.journal_path.as_ref().unwrap();
    assert!(
        journal.is_file(),
        "successful apply journals the transaction"
    );
    assert_eq!(
        fs::read(&preview.destination.path).unwrap(),
        preview.rendered_contents.as_bytes()
    );

    let undo_preview = preview_cheat_journey_undo(
        &applied.transaction_id,
        journal,
        &request.profile_cheat_root,
        &fixture.path("managed/backups"),
    );
    let undone = undo_cheat_journey(
        &undo_preview,
        &CheatJourneyUndoOptions {
            confirmation: CheatJourneyUndoConfirmation {
                preview_id: undo_preview.preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: "local-cheat-undo".into(),
            timestamp_unix_seconds: 1_700_000_100,
            history_root: fixture.path("managed/history"),
            backup_root: fixture.path("managed/backups"),
        },
    )
    .expect("undo succeeds");
    assert_eq!(undone.status, SharedApplyStatus::Success);
    assert!(
        !preview.destination.path.exists(),
        "rollback restores the prior (absent) state"
    );
}

#[test]
fn reapplying_the_same_local_file_is_reported_as_already_installed_not_a_conflict() {
    let fixture = Fixture::new("already-installed");
    let local_file = fixture.write("downloads/Chrono Quest (USA).cht", VALID_CHT);
    let request = fixture.destination();

    let discover_and_preview = |source: &Path, request: CheatDestinationRequest| {
        let found = discover_local_retroarch_cheat_file(
            &HostReadOnlyFilesystem,
            source,
            &fixture.game(),
            &CheatCandidateOptions::default(),
        )
        .unwrap();
        let mut selection = select_cheat_journey_candidate(
            &found.discovery,
            &found.location.catalogue_root,
            &found.location.catalogue_relative_path,
        )
        .unwrap();
        assert!(selection.cheat_selection.set_selected(0, true));
        (
            found.location.catalogue_root.clone(),
            preview_cheat_journey(
                &selection,
                &found.location.catalogue_root,
                request,
                "retroarch-main",
                "local file",
            )
            .unwrap(),
        )
    };

    let (catalogue_root, first_preview) = discover_and_preview(&local_file, request.clone());
    apply_cheat_journey(
        &first_preview,
        &catalogue_root,
        &CheatJourneyApplyApproval {
            preview_id: first_preview.preview_id.clone(),
            approved: true,
            replacement_approved: false,
        },
        &CheatJourneyApplyOptions {
            staging_root: fixture.path("managed/staging"),
            operation_id: "first-apply".into(),
            timestamp_unix_seconds: 1_700_000_000,
            history_root: fixture.path("managed/history"),
            backup_root: fixture.path("managed/backups"),
        },
    )
    .unwrap();

    // Re-run discovery/preview over the exact same file and destination.
    let (_, second_preview) = discover_and_preview(&local_file, request);
    assert_eq!(
        second_preview.action,
        archivefs_core::patch_manager::CheatJourneyPreviewAction::AlreadyInstalled,
        "identical content at the same destination is an idempotent no-op, never a conflict"
    );
}

#[test]
fn unsupported_and_malformed_local_files_never_reach_preview() {
    let fixture = Fixture::new("blocked");

    let wrong_extension = fixture.write("downloads/cheat.pnach", "patch=1,EE,0,extended,0");
    let error = discover_local_retroarch_cheat_file(
        &HostReadOnlyFilesystem,
        &wrong_extension,
        &fixture.game(),
        &CheatCandidateOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        LocalCheatFileError::UnsupportedExtension { .. }
    ));

    let malformed = fixture.write("downloads/broken.cht", "this is not a cheat file at all");
    let found = discover_local_retroarch_cheat_file(
        &HostReadOnlyFilesystem,
        &malformed,
        &fixture.game(),
        &CheatCandidateOptions::default(),
    )
    .expect("discovery itself does not error on malformed content");
    assert!(
        found.candidate().is_none() || !found.candidate().unwrap().manually_selectable,
        "malformed content is never installable"
    );
}
