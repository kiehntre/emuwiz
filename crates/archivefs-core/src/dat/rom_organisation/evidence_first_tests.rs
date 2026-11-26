//! Evidence-first Standard Bulk Rename: the preview reads no file; Apply
//! reads each approved file once and renames it only if it is the same
//! object, unchanged since preview, with the raw SHA-1 its saved catalogue
//! check recorded. The journal keeps the same SHA-256 proof as before.
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::dat::rename_apply::identity::{capture_identity, identity_matches};
use crate::dat::rom_organisation::*;
use crate::platform::identity::{
    PlatformIdentityConfidence, PlatformIdentityEvidence, PlatformIdentityResolution,
    PlatformIdentitySource,
};

const BODY: &[u8] = b"fixture contents";

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest;
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verified_dat() -> PlatformIdentityResolution {
    PlatformIdentityResolution::Resolved {
        generation: 1,
        platform: "Game Boy Color".into(),
        display_name: "Game Boy Color".into(),
        confidence: PlatformIdentityConfidence::High,
        evidence: vec![PlatformIdentityEvidence {
            platform: "Game Boy Color".into(),
            source: PlatformIdentitySource::VerifiedDat,
            confidence: PlatformIdentityConfidence::High,
            generation: 1,
            detail: "test evidence".into(),
        }],
    }
}

struct Case {
    _dir: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    candidates: Vec<OrganisationCandidate>,
}

/// One game file `Old.gbc` whose verified name is `New (World).gbc`.
fn case() -> Case {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("gbc");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("Old.gbc");
    std::fs::write(&source, BODY).unwrap();
    let candidates = vec![OrganisationCandidate {
        source_path: source.clone(),
        resolution: verified_dat(),
        canonical_name: Some("New (World).gbc".into()),
        content_classification: None,
        original_metadata: Default::default(),
    }];
    Case {
        _dir: dir,
        root,
        source,
        candidates,
    }
}

fn preview(case: &Case, raw_sha1: &HashMap<PathBuf, String>) -> OrganisationPlan {
    let mut ignore = |_: crate::job_progress::JobProgress| {};
    let mut progress = crate::job_progress::ProgressReporter::new(&mut ignore);
    build_rename_plan_from_evidence(
        &OrganisationPlanRequest {
            master_root: &case.root,
            mode: OrganisationMode::RenameInPlace,
            content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
            candidates: &case.candidates,
            generation: 1,
            unknown_folder: None,
            link_target_roots: None,
        },
        raw_sha1,
        None,
        &mut progress,
    )
    .expect("not cancelled")
}

fn sha1_of(path: &Path, sha1: String) -> HashMap<PathBuf, String> {
    HashMap::from([(path.to_path_buf(), sha1)])
}

fn approved(path: &Path) -> BTreeSet<String> {
    BTreeSet::from([path.to_string_lossy().into_owned()])
}

#[test]
fn the_preview_suggests_from_saved_evidence_without_reading_the_file() {
    let case = case();
    // Unreadable: any read during the preview would fail and block the row.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&case.source, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY).to_uppercase()));
    let entry = &plan.entries[0];
    assert_eq!(
        entry.status,
        OrganisationStatus::Suggested,
        "{:?}",
        entry.reason
    );
    assert_eq!(entry.destination_path, case.root.join("New (World).gbc"));
    let deferred = entry.deferred_capture.as_ref().expect("deferred capture");
    assert_eq!(
        deferred.expected_raw_sha1,
        sha1_hex(BODY),
        "stored lower-case"
    );
    assert_eq!(deferred.snapshot.size_bytes, BODY.len() as u64);
    // No content proof was taken, so nothing at preview can authorise Apply.
    assert!(entry.source_identity.as_ref().unwrap().freshness.is_none());
}

/// The library row for `case.source`, in a fresh catalogue database.
fn library(case: &Case) -> (crate::Database, i64) {
    let mut db =
        crate::Database::open_or_create(case.root.parent().unwrap().join("library.sqlite3"))
            .unwrap();
    db.register_source_folders(std::slice::from_ref(&case.root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    let archive = crate::Archive::from_path_in_root(&case.source, &case.root).unwrap();
    let id = db
        .upsert_archive(folder, &case.root, &archive)
        .unwrap()
        .archive_id;
    (db, id)
}

#[test]
fn preparing_an_evidence_first_preview_reads_no_file_and_binds_at_apply() {
    use std::os::unix::fs::PermissionsExt;
    let case = case();
    let (db, id) = library(&case);
    let mut plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    // Unreadable at preview: the preview (and its catalogue step) still
    // completes, the row stays ready, and nothing is bound yet.
    std::fs::set_permissions(&case.source, std::fs::Permissions::from_mode(0o000)).unwrap();
    prepare_organisation_catalogue(&mut plan, &db).expect("no read at preview");
    std::fs::set_permissions(&case.source, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert!(plan.entries[0].catalogue_binding.is_none());
    let approved = approved(&case.source);
    let journal = case.root.parent().unwrap().join("journal");
    // The permission changes changed the file's change time: that preview
    // can no longer be applied at all.
    let stale = build_organisation_transaction(&plan, &approved, 1).unwrap_err();
    assert!(stale.contains("changed since preview"), "{stale}");
    // A fresh preview of the now readable file.
    let mut plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    prepare_organisation_catalogue(&mut plan, &db).unwrap();
    assert!(plan.entries[0].catalogue_binding.is_none());
    // Unbound, Apply refuses rather than leave the library row behind.
    let mut unbound = build_organisation_transaction(&plan, &approved, 1).unwrap();
    let refused = apply_organisation_transaction(
        &mut unbound,
        &approved,
        1,
        crate::safe_read::TrustedRoots::from_paths([case.root.clone()]),
        &journal,
        &std::sync::atomic::AtomicBool::new(false),
        OrganisationMode::RenameInPlace,
        &case.root,
        &db,
    );
    let refused = refused.expect_err("an unbound rename is refused");
    assert!(
        format!("{refused:?}").contains("no catalogue binding"),
        "{refused:?}"
    );
    assert!(case.source.exists());
    // Bound at Apply from Apply's read: the rename moves the library row,
    // and Undo moves it back.
    let mut transaction = build_organisation_transaction(&plan, &approved, 1).unwrap();
    bind_organisation_transaction(&mut transaction, &db).unwrap();
    let destination = case.root.join("New (World).gbc");
    apply_organisation_transaction(
        &mut transaction,
        &approved,
        1,
        crate::safe_read::TrustedRoots::from_paths([case.root.clone()]),
        &journal,
        &std::sync::atomic::AtomicBool::new(false),
        OrganisationMode::RenameInPlace,
        &case.root,
        &db,
    )
    .unwrap();
    assert!(destination.exists() && !case.source.exists());
    assert_eq!(
        db.find_archive_id_by_absolute_path(&destination).unwrap(),
        Some(id)
    );
    assert_eq!(
        db.find_archive_id_by_absolute_path(&case.source).unwrap(),
        None
    );
    rollback_organisation_transaction(
        &mut transaction,
        &journal,
        &std::sync::atomic::AtomicBool::new(false),
        &case.root,
    )
    .unwrap();
    assert!(case.source.exists() && !destination.exists());
    assert_eq!(std::fs::read(&case.source).unwrap(), BODY);
    assert_eq!(
        db.find_archive_id_by_absolute_path(&case.source).unwrap(),
        Some(id)
    );
}

#[test]
fn a_file_without_current_saved_evidence_needs_verification() {
    let case = case();
    let plan = preview(&case, &HashMap::new());
    let entry = &plan.entries[0];
    assert_eq!(entry.status, OrganisationStatus::Blocked);
    assert_eq!(entry.reason.as_deref(), Some(NEEDS_VERIFICATION_REASON));
    // Held back with no deferred capture, so Apply can never use it; the
    // proposed name stays visible for display only.
    assert!(entry.deferred_capture.is_none());
    assert_eq!(entry.destination_path, case.root.join("New (World).gbc"));
    // Even approved, a Blocked row never enters a transaction.
    if let Ok(transaction) = build_organisation_transaction(&plan, &approved(&case.source), 1) {
        assert!(
            transaction.entries.is_empty(),
            "a blocked row is never applied"
        );
    }
    assert!(case.source.exists() && !case.root.join("New (World).gbc").exists());
}

#[test]
fn only_a_regular_file_can_be_renamed_from_saved_evidence() {
    let case = case();
    let link = case.root.join("Link.gbc");
    std::os::unix::fs::symlink(&case.source, &link).unwrap();
    let mut case = case;
    case.candidates[0].source_path = link.clone();
    let plan = preview(&case, &sha1_of(&link, sha1_hex(BODY)));
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
    assert_eq!(
        plan.entries[0].reason.as_deref(),
        Some(NEEDS_VERIFICATION_REASON)
    );
}

#[test]
fn apply_reads_the_file_once_and_journals_the_same_sha256_proof() {
    let case = case();
    let plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    let transaction = build_organisation_transaction(&plan, &approved(&case.source), 1)
        .expect("build transaction");
    let journalled = &transaction.entries[0].identity;
    // Exactly what a content-captured preview would have journalled.
    assert!(identity_matches(
        journalled,
        &capture_identity(&case.source).unwrap()
    ));
    assert!(journalled.freshness.is_some());
}

#[test]
fn apply_refuses_a_file_whose_sha1_is_not_the_catalogue_checks() {
    let case = case();
    let plan = preview(&case, &sha1_of(&case.source, sha1_hex(b"another dump")));
    let error = build_organisation_transaction(&plan, &approved(&case.source), 1).unwrap_err();
    assert!(error.contains("SHA-1 differs"), "{error}");
    assert_eq!(std::fs::read(&case.source).unwrap(), BODY, "untouched");
}

#[test]
fn apply_refuses_a_file_rewritten_after_the_preview() {
    let case = case();
    let plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    // Same size, different bytes, written in place. Timestamps may not move
    // within the disk's clock step, so either the file-state check or the
    // final SHA-1 check may be the one that refuses; both rename nothing.
    std::fs::write(&case.source, b"FIXTURE CONTENTS").unwrap();
    let error = build_organisation_transaction(&plan, &approved(&case.source), 1).unwrap_err();
    assert!(
        error.contains("changed since preview") || error.contains("(SHA-1 differs)"),
        "{error}"
    );
    // No transaction exists to apply, and the folder is exactly as it was.
    assert!(case.source.exists(), "the source is still there");
    assert!(
        !case.root.join("New (World).gbc").exists(),
        "no destination"
    );
    let names: Vec<_> = std::fs::read_dir(&case.root)
        .unwrap()
        .map(|item| item.unwrap().file_name())
        .collect();
    assert_eq!(names, ["Old.gbc"]);
    assert_eq!(std::fs::read(&case.source).unwrap(), b"FIXTURE CONTENTS");
}

#[test]
fn apply_refuses_a_replacement_with_identical_bytes_and_restored_times() {
    let case = case();
    let plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    let before = std::fs::metadata(&case.source).unwrap();
    // A different object at the same path: same name, bytes and mtime.
    let other = case.root.join("replacement.tmp");
    std::fs::write(&other, BODY).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    std::fs::rename(&other, &case.source).unwrap();
    let error = build_organisation_transaction(&plan, &approved(&case.source), 1).unwrap_err();
    assert!(error.contains("changed since preview"), "{error}");
}

#[test]
fn apply_refuses_a_file_touched_after_the_preview_even_with_the_same_content() {
    let case = case();
    let plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    // Metadata-only change (ctime moves): the preview no longer describes it.
    std::fs::File::options()
        .write(true)
        .open(&case.source)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    let error = build_organisation_transaction(&plan, &approved(&case.source), 1).unwrap_err();
    assert!(error.contains("changed since preview"), "{error}");
}

#[test]
fn an_older_plan_without_deferred_capture_keeps_the_content_check() {
    let case = case();
    let mut plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    // As if read from an older plan: no deferred capture, no content proof.
    plan.entries[0].deferred_capture = None;
    let error = build_organisation_transaction(&plan, &approved(&case.source), 1).unwrap_err();
    assert!(error.contains("changed since preview"), "{error}");
}

/// Cancelling Apply while it reads the approved files stops before the next
/// read: the unreadable file below would otherwise fail with a permission
/// error, and nothing is changed.
#[test]
fn a_cancelled_apply_stops_before_reading_the_next_file() {
    use std::os::unix::fs::PermissionsExt;
    let case = case();
    let plan = preview(&case, &sha1_of(&case.source, sha1_hex(BODY)));
    assert!(plan.entries[0].deferred_capture.is_some());
    std::fs::set_permissions(&case.source, std::fs::Permissions::from_mode(0o000)).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let refused =
        build_organisation_transaction_cancellable(&plan, &approved(&case.source), 1, &cancel)
            .unwrap_err();
    std::fs::set_permissions(&case.source, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(refused, BUILD_CANCELLED);
    assert_eq!(std::fs::read(&case.source).unwrap(), BODY);
    assert!(!case.root.join("New (World).gbc").exists());
}
