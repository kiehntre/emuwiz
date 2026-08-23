use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::patch_manager::{
    CheatCandidateArchive, CheatCandidateClassification, CheatCandidateOptions,
    CheatDestinationRequest, CheatJourneyApplyApproval, CheatJourneyApplyOptions,
    CheatJourneyErrorKind, CheatJourneyGameIdentity, CheatJourneyIdentityEvidence,
    CheatJourneyIdentityEvidenceKind, CheatJourneyIdentityState, CheatJourneyUndoConfirmation,
    CheatJourneyUndoOptions, SharedApplyStatus, SharedRollbackOutcome, apply_cheat_journey,
    discover_cheat_journey, load_cheat_catalogue_snapshot, preview_cheat_journey,
    preview_cheat_journey_undo, select_cheat_journey_candidate, undo_cheat_journey,
};

const PLATFORM: &str = "Nintendo - Nintendo Entertainment System";
const VALID_CHT: &str = "cheats = 2\ncheat0_desc = \"Health\"\ncheat0_code = \"AAAA\"\ncheat0_enable = false\ncheat1_desc = \"Lives\"\ncheat1_code = \"BBBB\"\ncheat1_enable = false\n";
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "archivefs-cheat-journey-{label}-{}-{}-{}",
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

    fn setup_catalogue(&self, duplicate: bool, malformed: bool) -> PathBuf {
        let root = self.path("catalogue");
        self.write(
            &format!("catalogue/{PLATFORM}/Chrono Quest (USA).cht"),
            VALID_CHT,
        );
        if duplicate {
            self.write(
                &format!("catalogue/{PLATFORM}/extra/Chrono Quest (USA).cht"),
                VALID_CHT,
            );
        }
        if malformed {
            self.write(
                &format!("catalogue/{PLATFORM}/Chrono Quest (USA) Bad.cht"),
                "not a cheat file\n",
            );
        }
        root
    }

    fn game(&self) -> CheatJourneyGameIdentity {
        CheatJourneyGameIdentity {
            state: CheatJourneyIdentityState::Verified,
            selected_archive: self.write("library/Chrono Quest (USA).zip", "rom"),
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn discovery(
    fixture: &Fixture,
    catalogue: &Path,
) -> archivefs_core::patch_manager::CheatJourneyDiscovery {
    let snapshot =
        load_cheat_catalogue_snapshot(&HostReadOnlyFilesystem, "fixture-provider", catalogue);
    discover_cheat_journey(
        &fixture.game(),
        &snapshot,
        &CheatCandidateOptions::default(),
    )
    .unwrap()
}

fn selected(
    fixture: &Fixture,
    catalogue: &Path,
) -> archivefs_core::patch_manager::CheatJourneySelection {
    let found = discovery(fixture, catalogue);
    let path = found.candidates.candidates[0]
        .candidate
        .catalogue_relative_path
        .clone();
    let mut selected = select_cheat_journey_candidate(&found, catalogue, &path).unwrap();
    assert!(selected.cheat_selection.set_selected(0, true));
    selected
}

fn destination(fixture: &Fixture) -> CheatDestinationRequest {
    let root = fixture.path("retroarch/cheats");
    fs::create_dir_all(root.join(PLATFORM)).unwrap();
    CheatDestinationRequest {
        profile_cheat_root: root,
        platform: Some(PLATFORM.into()),
        content_basename: Some("Chrono Quest (USA)".into()),
        playlist_name: None,
        catalogue_name: "Chrono Quest (USA)".into(),
    }
}

#[test]
fn complete_journey_is_explicit_read_only_before_apply_and_exactly_undoable() {
    let fixture = Fixture::new("complete");
    let catalogue = fixture.setup_catalogue(false, true);
    let found = discovery(&fixture, &catalogue);
    assert_eq!(found.candidates.candidates.len(), 1);
    assert_eq!(found.candidates.candidates[0].provider, "fixture-provider");
    assert_eq!(
        found.candidates.candidates[0].candidate.classification,
        CheatCandidateClassification::Strong
    );
    assert_eq!(
        found.excluded_candidate_count, 1,
        "a malformed candidate is excluded while the valid one remains"
    );

    let selection = selected(&fixture, &catalogue);
    let request = destination(&fixture);
    let preview = preview_cheat_journey(
        &selection,
        &catalogue,
        request.clone(),
        "retroarch-main",
        "local trusted catalogue",
    )
    .unwrap();
    assert!(
        !fixture.path("managed/staging").exists(),
        "preview never stages or writes"
    );
    assert!(!preview.rendered_contents.is_empty());

    let applied = apply_cheat_journey(
        &preview,
        &catalogue,
        &CheatJourneyApplyApproval {
            preview_id: preview.preview_id.clone(),
            approved: true,
            replacement_approved: false,
        },
        &CheatJourneyApplyOptions {
            staging_root: fixture.path("managed/staging"),
            operation_id: "journey-apply".into(),
            timestamp_unix_seconds: 1_700_000_000,
            history_root: fixture.path("managed/history"),
            backup_root: fixture.path("managed/backups"),
        },
    )
    .unwrap();
    assert_eq!(applied.result.journal.status, SharedApplyStatus::Success);
    let journal = applied.result.journal_path.as_ref().unwrap();
    assert!(journal.is_file(), "successful apply records history");
    assert_eq!(
        fs::read(&preview.destination.path).unwrap(),
        preview.rendered_contents.as_bytes()
    );
    fixture.write(
        &format!("retroarch/cheats/{PLATFORM}/Handmade.cht"),
        "user code",
    );

    let undo_preview = preview_cheat_journey_undo(
        &applied.transaction_id,
        journal,
        &request.profile_cheat_root,
        &fixture.path("managed/backups"),
    );
    assert!(undo_preview.preview.available);
    let undo = undo_cheat_journey(
        &undo_preview,
        &CheatJourneyUndoOptions {
            confirmation: CheatJourneyUndoConfirmation {
                preview_id: undo_preview.preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: "journey-undo".into(),
            timestamp_unix_seconds: 1_700_000_001,
            history_root: fixture.path("managed/history"),
            backup_root: fixture.path("managed/backups"),
        },
    )
    .unwrap();
    assert_eq!(undo.status, SharedApplyStatus::Success);
    assert!(!preview.destination.path.exists());
    assert_eq!(
        fs::read(fixture.path(&format!("retroarch/cheats/{PLATFORM}/Handmade.cht"))).unwrap(),
        b"user code"
    );
    let repeated = preview_cheat_journey_undo(
        &applied.transaction_id,
        journal,
        &request.profile_cheat_root,
        &fixture.path("managed/backups"),
    );
    assert_eq!(
        repeated.preview.entries[0].outcome,
        SharedRollbackOutcome::AlreadyRolledBack
    );
}

#[test]
fn identity_ambiguity_destination_and_staleness_fail_closed() {
    let fixture = Fixture::new("fail-closed");
    let catalogue = fixture.setup_catalogue(true, false);
    let snapshot =
        load_cheat_catalogue_snapshot(&HostReadOnlyFilesystem, "fixture-provider", &catalogue);
    let mut unknown = fixture.game();
    unknown.state = CheatJourneyIdentityState::Unknown;
    assert_eq!(
        discover_cheat_journey(&unknown, &snapshot, &CheatCandidateOptions::default())
            .unwrap_err()
            .kind,
        CheatJourneyErrorKind::IdentityUnknown
    );
    let mut conflicting = fixture.game();
    conflicting.state = CheatJourneyIdentityState::Conflicting;
    assert_eq!(
        discover_cheat_journey(&conflicting, &snapshot, &CheatCandidateOptions::default())
            .unwrap_err()
            .kind,
        CheatJourneyErrorKind::IdentityConflicting
    );

    let found = discovery(&fixture, &catalogue);
    assert!(
        found
            .candidates
            .candidates
            .iter()
            .all(|candidate| candidate.candidate.classification
                == CheatCandidateClassification::Ambiguous)
    );
    let first = found.candidates.candidates[0]
        .candidate
        .catalogue_relative_path
        .clone();
    let mut explicit = select_cheat_journey_candidate(&found, &catalogue, &first).unwrap();
    explicit.cheat_selection.set_selected(0, true);
    let mut unsafe_destination = destination(&fixture);
    unsafe_destination.profile_cheat_root = PathBuf::from("relative-profile");
    assert_eq!(
        preview_cheat_journey(
            &explicit,
            &catalogue,
            unsafe_destination,
            "profile",
            "local"
        )
        .unwrap_err()
        .kind,
        CheatJourneyErrorKind::DestinationUnavailable
    );

    let request = destination(&fixture);
    let preview =
        preview_cheat_journey(&explicit, &catalogue, request.clone(), "profile", "local").unwrap();
    fs::write(&preview.destination.path, "changed after preview").unwrap();
    assert_eq!(
        apply_cheat_journey(
            &preview,
            &catalogue,
            &CheatJourneyApplyApproval {
                preview_id: preview.preview_id.clone(),
                approved: true,
                replacement_approved: true
            },
            &CheatJourneyApplyOptions {
                staging_root: fixture.path("managed/staging"),
                operation_id: "stale".into(),
                timestamp_unix_seconds: 1,
                history_root: fixture.path("managed/history"),
                backup_root: fixture.path("managed/backups")
            },
        )
        .unwrap_err()
        .kind,
        CheatJourneyErrorKind::PreviewChanged
    );
}

#[test]
fn no_matches_stays_a_successful_empty_discovery() {
    let fixture = Fixture::new("no-matches");
    let catalogue = fixture.path("catalogue");
    fs::create_dir_all(&catalogue).unwrap();
    let found = discovery(&fixture, &catalogue);
    assert!(found.candidates.candidates.is_empty());
}
