//! Scope enumeration, filesystem boundaries and result groups.
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::*;

fn enumerate(scope: &OrganisationScope, sources: &[PathBuf]) -> Option<ScopeEnumeration> {
    let mut ignore = |_: archivefs_core::job_progress::JobProgress| {};
    let mut progress = ProgressReporter::new(&mut ignore);
    enumerate_scope(scope, sources, None, &mut progress)
}

fn game(id: i64, path: &Path) -> ScopedGame {
    ScopedGame {
        archive_id: id,
        path: path.to_path_buf(),
        known_missing: false,
    }
}

fn write(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"rom").unwrap();
    path.to_path_buf()
}

/// Pretends `path` (and everything under it) is on device 4242.
fn other_device_under(path: PathBuf) {
    DEVICE_OVERRIDE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |p: &Path| {
            p.starts_with(&path).then_some(4242)
        }))
    });
}

fn no_device_override() {
    DEVICE_OVERRIDE.with(|hook| *hook.borrow_mut() = None);
}

#[test]
fn selected_games_never_read_an_unrelated_source() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    let unrelated = dir.path().join("unrelated");
    let a = write(&gbc.join("a.gbc"));
    // Any read of the unrelated source would fail and be reported.
    std::fs::create_dir_all(unrelated.join("locked")).unwrap();
    std::fs::set_permissions(
        unrelated.join("locked"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let scope = OrganisationScope::Games {
        games: vec![game(1, &a)],
        hidden_by_filter: 0,
    };
    let found = enumerate(&scope, &[gbc.clone(), unrelated.clone()]).unwrap();
    std::fs::set_permissions(
        unrelated.join("locked"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(
        found.files,
        vec![ScopedFile {
            path: a,
            archive_id: Some(1)
        }]
    );
    assert!(found.incomplete.is_empty(), "{:?}", found.incomplete);
}

#[test]
fn a_selected_game_that_is_gone_makes_the_scope_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    std::fs::create_dir_all(&gbc).unwrap();
    let mut gone = game(1, &gbc.join("gone.gbc"));
    gone.known_missing = true; // picked games are strict even when known missing
    let scope = OrganisationScope::Games {
        games: vec![gone],
        hidden_by_filter: 0,
    };
    let found = enumerate(&scope, &[gbc]).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.incomplete.len(), 1);
}

#[test]
fn a_platform_lists_known_missing_games_as_not_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    let a = write(&gbc.join("a.gbc"));
    let mut gone = game(2, &gbc.join("gone.gbc"));
    gone.known_missing = true;
    let scope = OrganisationScope::Platforms {
        names: ["Game Boy Color".to_string()].into(),
        games: vec![game(1, &a), gone],
    };
    let found = enumerate(&scope, std::slice::from_ref(&gbc)).unwrap();
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.not_on_disk, vec![gbc.join("gone.gbc")]);
    assert!(
        found.incomplete.is_empty(),
        "known missing is not incomplete"
    );
    // Missing without the Library knowing: incomplete, Apply stays off.
    std::fs::remove_file(&a).unwrap();
    let found = enumerate(&scope, &[gbc]).unwrap();
    assert_eq!(found.incomplete.len(), 1);
}

#[test]
fn a_game_on_another_filesystem_is_skipped_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    let mounted = write(&gbc.join("mounted-archive/inner.gbc"));
    let plain = write(&gbc.join("plain.gbc"));
    other_device_under(gbc.join("mounted-archive"));
    let scope = OrganisationScope::Games {
        games: vec![game(1, &mounted), game(2, &plain)],
        hidden_by_filter: 0,
    };
    let found = enumerate(&scope, &[gbc]).unwrap();
    no_device_override();
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.files[0].path, plain);
    assert_eq!(found.other_filesystems, vec![mounted]);
}

#[test]
fn a_game_outside_the_configured_sources_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    std::fs::create_dir_all(&gbc).unwrap();
    let elsewhere = write(&dir.path().join("elsewhere/a.gbc"));
    let scope = OrganisationScope::Games {
        games: vec![game(1, &elsewhere)],
        hidden_by_filter: 0,
    };
    let found = enumerate(&scope, &[gbc]).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.incomplete.len(), 1);
}

#[test]
fn a_folder_scope_has_no_file_limit_and_keeps_supporting_files_out() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    for i in 0..2_500 {
        write(&gbc.join(format!("sub{}/g{i}.gbc", i % 7)));
    }
    write(&gbc.join("media/cover.png"));
    let found = enumerate(&OrganisationScope::Folders(vec![gbc.clone()]), &[gbc]).unwrap();
    assert_eq!(found.files.len(), 2_500);
    assert!(found.incomplete.is_empty(), "{:?}", found.incomplete);
}

#[test]
fn a_folder_walk_never_enters_another_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    write(&gbc.join("a.gbc"));
    write(&gbc.join("ratarmount-mount/inside.gbc"));
    other_device_under(gbc.join("ratarmount-mount"));
    let found = enumerate(
        &OrganisationScope::Folders(vec![gbc.clone()]),
        std::slice::from_ref(&gbc),
    )
    .unwrap();
    no_device_override();
    assert_eq!(found.files.len(), 1, "{:?}", found.files);
    assert_eq!(found.other_filesystems, vec![gbc.join("ratarmount-mount")]);
    assert!(found.incomplete.is_empty());
}

#[test]
fn a_folder_walk_never_follows_a_symlinked_folder() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    write(&gbc.join("a.gbc"));
    write(&dir.path().join("outside/b.gbc"));
    symlink(dir.path().join("outside"), gbc.join("linked")).unwrap();
    let found = enumerate(
        &OrganisationScope::Folders(vec![gbc.clone()]),
        std::slice::from_ref(&gbc),
    )
    .unwrap();
    let paths: Vec<_> = found.files.iter().map(|f| f.path.clone()).collect();
    // The link object itself is listed (and later refused: not a regular
    // file); nothing behind it is read.
    assert_eq!(paths, vec![gbc.join("a.gbc"), gbc.join("linked")]);
}

#[test]
fn a_folder_outside_the_configured_sources_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    std::fs::create_dir_all(&gbc).unwrap();
    let elsewhere = dir.path().join("elsewhere");
    write(&elsewhere.join("a.gbc"));
    let found = enumerate(&OrganisationScope::Folders(vec![elsewhere]), &[gbc]).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.incomplete.len(), 1);
    let mut picker = ScopePicker {
        folder_draft: dir.path().join("elsewhere").display().to_string(),
        ..Default::default()
    };
    picker.add_folder(&[dir.path().join("gbc")]);
    assert!(picker.folders.is_empty() && picker.folder_error.is_some());
}

#[test]
fn a_cancelled_enumeration_returns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let gbc = dir.path().join("gbc");
    write(&gbc.join("a.gbc"));
    let cancel = AtomicBool::new(true);
    let mut ignore = |_: archivefs_core::job_progress::JobProgress| {};
    let mut progress = ProgressReporter::new(&mut ignore);
    let folders = OrganisationScope::Folders(vec![gbc.clone()]);
    assert!(
        enumerate_scope(
            &folders,
            std::slice::from_ref(&gbc),
            Some(&cancel),
            &mut progress
        )
        .is_none()
    );
    let games = OrganisationScope::Games {
        games: vec![game(1, &gbc.join("a.gbc"))],
        hidden_by_filter: 0,
    };
    assert!(enumerate_scope(&games, &[gbc], Some(&cancel), &mut progress).is_none());
}

#[test]
fn the_scope_fingerprint_and_label_change_with_the_scope() {
    let a = OrganisationScope::Games {
        games: vec![game(1, Path::new("/x/a.gbc"))],
        hidden_by_filter: 0,
    };
    let b = OrganisationScope::Games {
        games: vec![game(2, Path::new("/x/b.gbc"))],
        hidden_by_filter: 0,
    };
    let hidden = OrganisationScope::Games {
        games: vec![game(1, Path::new("/x/a.gbc"))],
        hidden_by_filter: 1,
    };
    assert_ne!(a.fingerprint(), b.fingerprint());
    assert_ne!(a.fingerprint(), hidden.fingerprint());
    assert!(hidden.label().contains("1 hidden by the Library filter"));
}

#[test]
fn saved_checks_map_to_honest_groups() {
    use archivefs_core::VerifiedDatName as V;
    let evidence = |name, current| archivefs_core::VerifiedDatEvidence {
        name,
        raw_sha1: None,
        has_current_check: current,
    };
    assert_eq!(SavedCheck::from_evidence(None), SavedCheck::Missing);
    assert_eq!(
        SavedCheck::from_evidence(Some(&evidence(V::None, true))),
        SavedCheck::NoMatch
    );
    assert_eq!(
        SavedCheck::from_evidence(Some(&evidence(V::None, false))),
        SavedCheck::NotCurrent
    );
    assert_eq!(
        SavedCheck::from_evidence(Some(&evidence(V::NotUsable("stale".into()), false))),
        SavedCheck::NotCurrent
    );
    assert_eq!(
        SavedCheck::from_evidence(Some(&evidence(V::NotUsable("ambiguous".into()), true))),
        SavedCheck::Other
    );
}

#[test]
fn rows_fall_into_the_five_groups() {
    use archivefs_core::dat::rom_organisation::{
        NEEDS_VERIFICATION_REASON, OrganisationPlanEntry, OrganisationStatus,
    };
    let entry = |status, reason: Option<&str>| OrganisationPlanEntry {
        source_identity: None,
        catalogue_binding: None,
        disc_set: None,
        source_path: "/x/a.gbc".into(),
        destination_path: "/x/A.gbc".into(),
        platform: None,
        platform_display_name: String::new(),
        platform_source: String::new(),
        slug: None,
        layout_folder: None,
        mode: OrganisationMode::RenameInPlace,
        content_classification: None,
        original_metadata: Default::default(),
        status,
        reason: reason.map(str::to_string),
        deferred_capture: None,
    };
    use OrganisationStatus as S;
    let group = |status, reason, check| rename_group(&entry(status, reason), None, check);
    assert_eq!(
        group(S::Suggested, None, SavedCheck::Other),
        RenameGroup::Ready
    );
    // Keeping its name is "already correct" only with a verified name.
    let verified = super::super::NameChoice {
        name: None,
        source: super::super::NameSource::VerifiedDat {
            catalogue: "c".into(),
        },
        note: None,
        skipped_archive: false,
    };
    assert_eq!(
        rename_group(
            &entry(S::AlreadyOrganised, None),
            Some(&verified),
            SavedCheck::Other
        ),
        RenameGroup::AlreadyCorrect
    );
    let original = super::super::NameChoice {
        source: super::super::NameSource::Original,
        ..verified.clone()
    };
    for (check, expected) in [
        (SavedCheck::Missing, RenameGroup::NeedsVerification),
        (SavedCheck::NotCurrent, RenameGroup::NeedsVerification),
        (SavedCheck::NoMatch, RenameGroup::NoCatalogueMatch),
        (SavedCheck::Other, RenameGroup::NoCatalogueMatch),
    ] {
        assert_eq!(
            rename_group(&entry(S::AlreadyOrganised, None), Some(&original), check),
            expected,
            "{check:?}"
        );
    }
    let skipped_archive = super::super::NameChoice {
        skipped_archive: true,
        ..original.clone()
    };
    assert_eq!(
        rename_group(
            &entry(S::AlreadyOrganised, None),
            Some(&skipped_archive),
            SavedCheck::Other
        ),
        RenameGroup::NoCatalogueMatch
    );
    assert_eq!(
        group(
            S::Blocked,
            Some(NEEDS_VERIFICATION_REASON),
            SavedCheck::Other
        ),
        RenameGroup::NeedsVerification
    );
    assert_eq!(
        group(S::Blocked, Some("x"), SavedCheck::Missing),
        RenameGroup::NeedsVerification
    );
    assert_eq!(
        group(S::Blocked, Some("x"), SavedCheck::NotCurrent),
        RenameGroup::NeedsVerification
    );
    assert_eq!(
        group(S::Blocked, Some("x"), SavedCheck::NoMatch),
        RenameGroup::NoCatalogueMatch
    );
    assert_eq!(
        group(S::Blocked, Some("save"), SavedCheck::Other),
        RenameGroup::Blocked
    );
    assert_eq!(
        group(S::Conflict, None, SavedCheck::Other),
        RenameGroup::Blocked
    );
}

#[test]
fn a_romm_named_row_says_it_is_a_hint_and_stays_blocked() {
    use archivefs_core::dat::rom_organisation::{
        NEEDS_VERIFICATION_REASON, OrganisationPlan, OrganisationPlanEntry, OrganisationStatus,
    };
    let entry = |path: &str| OrganisationPlanEntry {
        source_identity: None,
        catalogue_binding: None,
        disc_set: None,
        source_path: path.into(),
        destination_path: Path::new(path).with_file_name("Hint (USA).gbc"),
        platform: None,
        platform_display_name: String::new(),
        platform_source: String::new(),
        slug: None,
        layout_folder: None,
        mode: OrganisationMode::RenameInPlace,
        content_classification: None,
        original_metadata: Default::default(),
        status: OrganisationStatus::Blocked,
        reason: Some(NEEDS_VERIFICATION_REASON.into()),
        deferred_capture: None,
    };
    let mut plan = OrganisationPlan {
        master_root: "/x".into(),
        mode: OrganisationMode::RenameInPlace,
        content_policy: Default::default(),
        classifier_version: "test".into(),
        generation: 1,
        entries: vec![
            entry("/x/romm.gbc"),
            entry("/x/verified.gbc"),
            entry("/x/other.gbc"),
        ],
    };
    let choice = |source| super::super::NameChoice {
        name: Some("Hint (USA).gbc".into()),
        source,
        note: None,
        skipped_archive: false,
    };
    let choices = [
        (
            PathBuf::from("/x/romm.gbc"),
            choice(super::super::NameSource::Romm),
        ),
        (
            PathBuf::from("/x/verified.gbc"),
            choice(super::super::NameSource::VerifiedDat {
                catalogue: "c".into(),
            }),
        ),
    ]
    .into();
    explain_unverified_names(&mut plan, &choices);
    let reasons: Vec<_> = plan
        .entries
        .iter()
        .map(|e| e.reason.as_deref().unwrap())
        .collect();
    assert_eq!(
        reasons,
        [
            super::super::hint_gate::UNVERIFIED_HINT_REASON,
            NEEDS_VERIFICATION_REASON,
            super::super::hint_gate::UNVERIFIED_NAME_REASON,
        ]
    );
    assert!(
        plan.entries
            .iter()
            .all(|e| e.status == OrganisationStatus::Blocked)
    );
    // A current check that found no catalogue entry: "No catalogue match".
    let romm = &plan.entries[0];
    assert_eq!(
        rename_group(romm, choices.get(&romm.source_path), SavedCheck::NoMatch),
        RenameGroup::NoCatalogueMatch
    );
}

#[test]
fn idle_disk_priority_is_restored_afterwards() {
    // SAFETY: plain integer syscall on the calling thread.
    let before = unsafe { libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0) };
    {
        let _idle = IdleDiskPriority::enter();
        let during = unsafe { libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0) };
        assert_eq!(during >> IOPRIO_CLASS_SHIFT, IOPRIO_CLASS_IDLE);
    }
    let after = unsafe { libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0) };
    assert_eq!(before, after);
}
