//! Bulk rename guard: synthetic library folders and a disposable database.
use super::game_rename::RetroArchSaveLocations;
use super::model::{OrganisationMode, OrganisationPlan, OrganisationStatus};
use super::plan::{OrganisationCandidate, OrganisationPlanRequest, build_organisation_plan};
use super::rename_guard::{guard_bulk_rename_plan, refuse_if_newly_blocked};
use super::transaction::{
    apply_organisation_transaction, build_organisation_transaction, live_resolution_for,
    prepare_organisation_catalogue, revalidate_organisation_plan,
    rollback_organisation_transaction,
};
use crate::dat::classification::ContentSelectionPolicy;
use crate::dat::rename_apply::{RollbackResult, TransactionState};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// A library folder with several games in `games/<system>/`.
struct Lib {
    temp: tempfile::TempDir,
    db: crate::Database,
    root: PathBuf,
}

impl Lib {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("games");
        std::fs::create_dir_all(&root).unwrap();
        let mut db = crate::Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
        db.register_source_folders(std::slice::from_ref(&root))
            .unwrap();
        Self { temp, db, root }
    }

    /// Adds a game file to the library with a manual platform.
    fn game(&mut self, folder: &str, file: &str, platform: &str) -> PathBuf {
        let dir = self.root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, format!("synthetic rom {file}")).unwrap();
        let source = self.db.list_source_folders().unwrap()[0].id;
        let archive = crate::Archive::from_path_in_root(&path, &self.root).unwrap();
        let id = self
            .db
            .upsert_archive(source, &self.root, &archive)
            .unwrap()
            .archive_id;
        self.db.set_manual_platform(id, platform).unwrap();
        path
    }

    fn journal(&self) -> PathBuf {
        self.temp.path().join("journal")
    }

    /// A bulk rename-in-place plan: each source gets the proposed name.
    fn plan(&self, mode: OrganisationMode, names: &[(&Path, &str)]) -> OrganisationPlan {
        let candidates: Vec<OrganisationCandidate> = names
            .iter()
            .map(|(source, name)| OrganisationCandidate {
                source_path: source.to_path_buf(),
                resolution: live_resolution_for(&self.db, source, 1),
                canonical_name: Some((*name).to_string()),
                content_classification: None,
                original_metadata: Default::default(),
            })
            .collect();
        build_organisation_plan(&OrganisationPlanRequest {
            master_root: &self.root,
            mode,
            content_policy: ContentSelectionPolicy::AllEntries,
            candidates: &candidates,
            generation: 1,
            unknown_folder: None,
        })
    }
}

fn no_saves() -> RetroArchSaveLocations {
    RetroArchSaveLocations::default()
}

fn statuses(plan: &OrganisationPlan) -> BTreeMap<String, (OrganisationStatus, String)> {
    plan.entries
        .iter()
        .map(|entry| {
            (
                entry
                    .source_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                (entry.status, entry.reason.clone().unwrap_or_default()),
            )
        })
        .collect()
}

fn approved_suggested(plan: &OrganisationPlan) -> BTreeSet<String> {
    plan.suggested()
        .map(|entry| entry.source_path.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_bulk_plan_blocks_what_the_one_game_rename_refuses() {
    let mut lib = Lib::new();
    let safe = lib.game("nes", "safe game.nes", "nes");
    let disc = lib.game("psx", "Quest (Disc 1).iso", "psx");
    std::fs::write(
        lib.root.join("psx").join("Quest.cue"),
        "FILE \"Quest (Disc 1).iso\" BINARY\n",
    )
    .unwrap();
    let in_m3u = lib.game("psx/multi", "Saga (Disc 2).chd", "psx");
    std::fs::write(
        lib.root.join("psx").join("Saga.m3u"),
        "multi/Saga (Disc 2).chd\n",
    )
    .unwrap();
    let arcade = lib.game("arcade", "pacman.zip", "Arcade");
    let saved = lib.game("snes", "Hero.sfc", "snes");
    let saves = lib.temp.path().join("saves");
    std::fs::create_dir_all(saves.join("Snes9x")).unwrap();
    std::fs::write(saves.join("Snes9x").join("Hero.srm"), b"save").unwrap();
    let locations = RetroArchSaveLocations {
        directories: vec![saves.clone()],
        ..Default::default()
    };

    let mut plan = lib.plan(
        OrganisationMode::RenameInPlace,
        &[
            (&safe, "Safe Game (USA).nes"),
            (&disc, "Quest (USA) (Disc 1).iso"),
            (&in_m3u, "Saga (USA) (Disc 2).chd"),
            (&arcade, "Pac-Man (Namco).zip"),
            (&saved, "Hero (USA).sfc"),
        ],
    );
    assert_eq!(
        plan.suggested().count(),
        5,
        "the plan alone sees no problem"
    );
    assert_eq!(guard_bulk_rename_plan(&lib.db, &mut plan, &locations), 4);

    let rows = statuses(&plan);
    assert_eq!(rows["safe game.nes"].0, OrganisationStatus::Suggested);
    let (status, reason) = &rows["Quest (Disc 1).iso"];
    assert_eq!(*status, OrganisationStatus::Blocked);
    assert!(
        reason.contains("Quest.cue") && reason.contains("stop the game launching"),
        "{reason}"
    );
    let (status, reason) = &rows["Saga (Disc 2).chd"];
    assert_eq!(
        *status,
        OrganisationStatus::Blocked,
        "an m3u in a parent folder counts"
    );
    assert!(reason.contains("Saga.m3u"), "{reason}");
    let (status, reason) = &rows["pacman.zip"];
    assert_eq!(*status, OrganisationStatus::Blocked);
    assert!(reason.contains("set name"), "{reason}");
    let (status, reason) = &rows["Hero.sfc"];
    assert_eq!(*status, OrganisationStatus::Blocked);
    assert!(
        reason.contains("1 RetroArch save") && reason.contains("Game Details"),
        "{reason}"
    );
    for path in [&safe, &disc, &in_m3u, &arcade, &saved] {
        assert!(path.exists(), "the guard never changes a file");
    }
}

#[test]
fn an_arcade_romset_folder_blocks_even_without_the_arcade_platform() {
    let mut lib = Lib::new();
    let set = lib.game("mame", "galaga.zip", "nes");
    lib.db
        .set_source_role(&lib.root, crate::SourceRole::ArcadeRomset)
        .unwrap();
    let mut plan = lib.plan(
        OrganisationMode::RenameInPlace,
        &[(&set, "Galaga (Namco).zip")],
    );
    assert_eq!(plan.suggested().count(), 1, "{:?}", plan.entries[0].reason);
    assert_eq!(guard_bulk_rename_plan(&lib.db, &mut plan, &no_saves()), 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

#[test]
fn moving_real_files_is_guarded_and_link_modes_are_left_alone() {
    let mut lib = Lib::new();
    let disc = lib.game("psx", "Quest.iso", "psx");
    std::fs::write(
        lib.root.join("psx").join("Quest.cue"),
        "FILE \"Quest.iso\" BINARY\n",
    )
    .unwrap();
    let mut moved = lib.plan(
        OrganisationMode::MoveRealFile,
        &[(&disc, "Quest (USA).iso")],
    );
    let suggested_before = moved.suggested().count();
    let blocked = guard_bulk_rename_plan(&lib.db, &mut moved, &no_saves());
    assert_eq!(
        blocked, suggested_before,
        "every movable row with a cue is blocked"
    );
    let mut linked = lib.plan(
        OrganisationMode::BuildLinkedLibrary,
        &[(&disc, "Quest (USA).iso")],
    );
    let before = statuses(&linked);
    assert_eq!(guard_bulk_rename_plan(&lib.db, &mut linked, &no_saves()), 0);
    assert_eq!(statuses(&linked), before, "links never touch the real file");
}

#[test]
fn filename_collisions_stay_conflicts_and_the_guard_keeps_them() {
    let mut lib = Lib::new();
    let first = lib.game("nes", "a.nes", "nes");
    let second = lib.game("nes", "b.nes", "nes");
    let taken = lib.game("nes", "Taken.nes", "nes");
    let mut plan = lib.plan(
        OrganisationMode::RenameInPlace,
        &[
            (&first, "Same.nes"),
            (&second, "same.NES"),
            (&taken, "Taken.nes"),
        ],
    );
    let other = lib.game("nes", "c.nes", "nes");
    let mut with_existing = lib.plan(OrganisationMode::RenameInPlace, &[(&other, "Taken.nes")]);
    guard_bulk_rename_plan(&lib.db, &mut plan, &no_saves());
    guard_bulk_rename_plan(&lib.db, &mut with_existing, &no_saves());
    let rows = statuses(&plan);
    assert_eq!(
        rows["a.nes"].0,
        OrganisationStatus::Conflict,
        "case-only clash"
    );
    assert_eq!(rows["b.nes"].0, OrganisationStatus::Conflict);
    assert_eq!(rows["Taken.nes"].0, OrganisationStatus::AlreadyOrganised);
    assert_eq!(
        with_existing.entries[0].status,
        OrganisationStatus::Conflict
    );
}

#[test]
fn unreadable_save_settings_block_instead_of_guessing() {
    let mut lib = Lib::new();
    let game = lib.game("nes", "Old.nes", "nes");
    let mut plan = lib.plan(OrganisationMode::RenameInPlace, &[(&game, "New.nes")]);
    let locations = RetroArchSaveLocations {
        uncheckable: vec![
            "/somewhere/retroarch.cfg (RetroArch settings not fully readable)".into(),
        ],
        ..Default::default()
    };
    assert_eq!(guard_bulk_rename_plan(&lib.db, &mut plan, &locations), 1);
    assert!(
        plan.entries[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("could not be checked")
    );
}

#[test]
fn a_cue_or_save_that_appears_after_the_preview_refuses_the_apply() {
    let mut lib = Lib::new();
    let disc = lib.game("psx", "Late.iso", "psx");
    let other = lib.game("nes", "Other.nes", "nes");
    let mut plan = lib.plan(
        OrganisationMode::RenameInPlace,
        &[(&disc, "Late (USA).iso"), (&other, "Other (USA).nes")],
    );
    assert_eq!(guard_bulk_rename_plan(&lib.db, &mut plan, &no_saves()), 0);
    let approved = approved_suggested(&plan);
    assert!(refuse_if_newly_blocked(&lib.db, &plan, &approved, &no_saves()).is_ok());

    std::fs::write(
        lib.root.join("psx").join("Late.cue"),
        "FILE \"Late.iso\" BINARY\n",
    )
    .unwrap();
    let refused = refuse_if_newly_blocked(&lib.db, &plan, &approved, &no_saves()).unwrap_err();
    assert!(
        refused.contains("Late.cue") && refused.contains("Nothing was changed"),
        "{refused}"
    );
    // Not approved: it is not part of this Apply, so it does not refuse it.
    let only_other: BTreeSet<String> = [other.to_string_lossy().into_owned()].into();
    assert!(refuse_if_newly_blocked(&lib.db, &plan, &only_other, &no_saves()).is_ok());

    let saves = lib.temp.path().join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    std::fs::write(saves.join("Other.srm"), b"new save").unwrap();
    let locations = RetroArchSaveLocations {
        directories: vec![saves],
        ..Default::default()
    };
    let refused = refuse_if_newly_blocked(&lib.db, &plan, &only_other, &locations).unwrap_err();
    assert!(refused.contains("RetroArch save"), "{refused}");
    assert!(disc.exists() && other.exists());
}

#[test]
fn one_cue_sheet_blocks_every_disc_it_names_in_a_large_folder() {
    let mut lib = Lib::new();
    let mut names = Vec::new();
    for index in 0..40 {
        let bin = lib.game("psx", &format!("Game {index} (Track 1).iso"), "psx");
        std::fs::write(
            lib.root.join("psx").join(format!("Game {index}.cue")),
            format!("FILE \"Game {index} (Track 1).iso\" BINARY\n"),
        )
        .unwrap();
        names.push((bin, format!("Game {index} (USA) (Track 1).iso")));
    }
    let free = lib.game("psx", "Loose.iso", "psx");
    names.push((free, "Loose (USA).iso".into()));
    let pairs: Vec<(&Path, &str)> = names
        .iter()
        .map(|(p, n)| (p.as_path(), n.as_str()))
        .collect();
    let mut plan = lib.plan(OrganisationMode::RenameInPlace, &pairs);
    assert_eq!(guard_bulk_rename_plan(&lib.db, &mut plan, &no_saves()), 40);
    let rows = statuses(&plan);
    assert_eq!(rows["Loose.iso"].0, OrganisationStatus::Suggested);
    assert!(rows["Game 7 (Track 1).iso"].1.contains("Game 7.cue"));
}

#[test]
fn a_guarded_bulk_rename_applies_only_safe_rows_and_undo_restores_them() {
    let mut lib = Lib::new();
    let safe = lib.game("nes", "safe.nes", "nes");
    let disc = lib.game("psx", "Disc.iso", "psx");
    std::fs::write(
        lib.root.join("psx").join("Disc.cue"),
        "FILE \"Disc.iso\" BINARY\n",
    )
    .unwrap();
    let mut plan = lib.plan(
        OrganisationMode::RenameInPlace,
        &[(&safe, "Safe (USA).nes"), (&disc, "Disc (USA).iso")],
    );
    guard_bulk_rename_plan(&lib.db, &mut plan, &no_saves());
    prepare_organisation_catalogue(&mut plan, &lib.db).unwrap();
    let approved = approved_suggested(&plan);
    assert_eq!(
        approved.len(),
        1,
        "only the safe row is approved by default"
    );
    refuse_if_newly_blocked(&lib.db, &plan, &approved, &no_saves()).unwrap();

    let wanted: BTreeMap<PathBuf, String> = [(safe.clone(), "Safe (USA).nes".to_string())].into();
    let name_for = |path: &Path| wanted.get(path).cloned();
    revalidate_organisation_plan(&plan, lib.db.path(), &name_for).unwrap();
    let mut transaction =
        build_organisation_transaction(&plan, &approved, plan.generation).unwrap();
    assert_eq!(transaction.entries.len(), 1);
    std::fs::create_dir_all(lib.journal()).unwrap();
    let cancel = AtomicBool::new(false);
    let outcome = apply_organisation_transaction(
        &mut transaction,
        &approved,
        plan.generation,
        crate::safe_read::TrustedRoots::from_paths(vec![lib.root.clone()]),
        &lib.journal(),
        &cancel,
        OrganisationMode::RenameInPlace,
        &lib.root,
        &lib.db,
    )
    .unwrap();
    assert_eq!(outcome.transaction.state, TransactionState::Applied);
    let renamed = lib.root.join("nes").join("Safe (USA).nes");
    assert!(renamed.exists() && !safe.exists());
    assert!(disc.exists(), "the blocked disc is untouched");

    let mut transaction = outcome.transaction;
    let undo =
        rollback_organisation_transaction(&mut transaction, &lib.journal(), &cancel, &lib.root)
            .unwrap();
    assert_eq!(undo.rollback.result, RollbackResult::FullyRolledBack);
    assert!(safe.exists() && !renamed.exists() && disc.exists());
}
