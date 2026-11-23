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
            link_target_roots: None,
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
fn moving_real_files_is_guarded_and_link_modes_refuse_a_disc_without_its_sheet() {
    let mut lib = Lib::new();
    let disc = lib.game("psx", "Quest.iso", "psx");
    let solo = lib.game("psx", "Solo.iso", "psx");
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
    // Owner 2026-11-23: a linked library never gets a disc without its
    // sheet. The disc a sheet names is refused with the reason; a game
    // without a sheet is linked as before.
    for mode in [
        OrganisationMode::BuildLinkedLibrary,
        OrganisationMode::OrganiseSymlinkOnly,
    ] {
        let mut linked = lib.plan(
            mode,
            &[(&disc, "Quest (USA).iso"), (&solo, "Solo (USA).iso")],
        );
        let before = statuses(&linked);
        guard_bulk_rename_plan(&lib.db, &mut linked, &no_saves());
        let after = statuses(&linked);
        if before["Quest.iso"].0 == OrganisationStatus::Suggested {
            assert_eq!(
                after["Quest.iso"].0,
                OrganisationStatus::Blocked,
                "{mode:?}"
            );
            assert!(
                after["Quest.iso"].1.contains("Quest.cue names this file"),
                "{mode:?}: {}",
                after["Quest.iso"].1
            );
            assert!(
                after["Quest.iso"].1.contains("is not linked with it"),
                "{mode:?}"
            );
            assert!(
                after["Quest.iso"].1.contains("nothing will be linked"),
                "{mode:?}"
            );
        }
        assert_eq!(
            after["Solo.iso"], before["Solo.iso"],
            "{mode:?}: a game without a sheet is unchanged"
        );
    }
    // A linked plan must actually exercise the refusal (not only skip it).
    let mut linked = lib.plan(
        OrganisationMode::BuildLinkedLibrary,
        &[(&disc, "Quest (USA).iso")],
    );
    assert_eq!(
        linked.entries[0].status,
        OrganisationStatus::Suggested,
        "{:?}",
        linked.entries[0].reason
    );
    assert_eq!(guard_bulk_rename_plan(&lib.db, &mut linked, &no_saves()), 1);
    // A sheet that appears after a linked preview refuses that Apply.
    let late = lib.game("psx", "Late.iso", "psx");
    let linked = lib.plan(
        OrganisationMode::BuildLinkedLibrary,
        &[(&late, "Late (USA).iso")],
    );
    let approved = approved_suggested(&linked);
    assert_eq!(approved.len(), 1);
    std::fs::write(lib.root.join("psx").join("Late.m3u"), "Late.iso\n").unwrap();
    let refused = refuse_if_newly_blocked(&lib.db, &linked, &approved, &no_saves()).unwrap_err();
    assert!(refused.contains("Nothing was changed"), "{refused}");
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

// ---- shared save index: each folder read once per preview ----

use super::game_rename::{SaveSearchIndex, find_retroarch_saves, find_retroarch_saves_with};
use super::rename_guard::guard_bulk_rename_plan_with_stats;

/// `count` games in one `nes` folder, a RetroArch save folder with two core
/// folders (one with a nested folder), and a save for every 100th game.
fn crowded_library(count: usize) -> (Lib, Vec<PathBuf>, RetroArchSaveLocations) {
    let mut lib = Lib::new();
    let saves = lib.temp.path().join("saves");
    for core in ["Mesen", "FCEUmm/nested"] {
        std::fs::create_dir_all(saves.join(core)).unwrap();
    }
    std::fs::create_dir_all(lib.root.join("nes").join("states")).unwrap();
    let mut games = Vec::with_capacity(count);
    for index in 0..count {
        let game = lib.game("nes", &format!("Game {index:05}.nes"), "nes");
        if index % 100 == 0 {
            std::fs::write(
                saves.join("Mesen").join(format!("Game {index:05}.srm")),
                b"s",
            )
            .unwrap();
        }
        games.push(game);
    }
    let locations = RetroArchSaveLocations {
        directories: vec![saves],
        ..Default::default()
    };
    (lib, games, locations)
}

fn rename_plan(lib: &Lib, games: &[PathBuf]) -> OrganisationPlan {
    let names: Vec<String> = games
        .iter()
        .map(|game| {
            let stem = game.file_stem().unwrap().to_string_lossy().into_owned();
            format!("{stem} (USA).nes")
        })
        .collect();
    let pairs: Vec<(&Path, &str)> = games
        .iter()
        .zip(&names)
        .map(|(game, name)| (game.as_path(), name.as_str()))
        .collect();
    lib.plan(OrganisationMode::RenameInPlace, &pairs)
}

#[test]
fn thousands_of_games_in_one_folder_read_each_folder_once() {
    let (lib, games, locations) = crowded_library(3000);
    let mut plan = rename_plan(&lib, &games);
    assert_eq!(plan.suggested().count(), 3000);
    let stats = guard_bulk_rename_plan_with_stats(&lib.db, &mut plan, &locations);
    assert_eq!(stats.checked, 3000);
    assert_eq!(stats.blocked, 30, "every 100th game has a save");
    // Game folder + its `states` sub-folder + save root + Mesen + FCEUmm + nested:
    // six folders, read once each, for 3000 games.
    assert_eq!(stats.saves.folder_reads, 6, "{stats:?}");
    assert!(stats.saves.entries_read < 3000 + 50, "{stats:?}");
    // Binary search: a game compares only the names starting with its stem
    // (plus one), not the 3000-entry folder.
    assert!(stats.saves.entries_compared < 3000 * 6 * 3, "{stats:?}");
    assert_eq!(
        stats.sidecar_folder_reads, 2,
        "nes folder and the library root: {stats:?}"
    );
    let rows = statuses(&plan);
    assert_eq!(rows["Game 00100.nes"].0, OrganisationStatus::Blocked);
    assert_eq!(rows["Game 00101.nes"].0, OrganisationStatus::Suggested);
}

#[test]
fn the_shared_index_finds_exactly_what_a_fresh_search_finds() {
    let mut lib = Lib::new();
    let names = [
        "Super Mario Bros.nes",
        "Super Mario Bros. 3.nes",
        "Gradius Vol.nes",
        "Card.nes",
        "Plain.nes",
    ];
    let games: Vec<PathBuf> = names.iter().map(|n| lib.game("mix", n, "nes")).collect();
    let folder = lib.root.join("mix");
    std::fs::create_dir_all(folder.join("core")).unwrap();
    for save in [
        "Super Mario Bros.srm",
        "Super Mario Bros.state1.png",
        "Super Mario Bros. 3.srm",
        "Gradius Vol.2.mcr",
        "Card.1.mcr",
        "Card.notes.txt",
    ] {
        std::fs::write(folder.join(save), b"s").unwrap();
    }
    std::fs::write(folder.join("core").join("Plain.state"), b"s").unwrap();
    let saves = lib.temp.path().join("ra");
    std::fs::create_dir_all(saves.join("core/deep")).unwrap();
    std::fs::write(saves.join("core/deep/Plain.anything.here"), b"s").unwrap();
    let locations = RetroArchSaveLocations {
        directories: vec![saves],
        ..Default::default()
    };
    let mut shared = SaveSearchIndex::default();
    for game in &games {
        let fresh = find_retroarch_saves(game, &locations).unwrap();
        let indexed = find_retroarch_saves_with(game, &locations, &mut shared).unwrap();
        assert_eq!(indexed, fresh, "{}", game.display());
    }
    let mario = find_retroarch_saves(&games[0], &locations).unwrap();
    assert_eq!(
        mario.len(),
        2,
        "{mario:?}: the `. 3` save belongs to the other game"
    );
    assert!(find_retroarch_saves(&games[4], &locations).unwrap().len() == 2);
}

#[test]
fn a_linked_or_unreadable_folder_still_refuses_every_game_that_needs_it() {
    let (lib, games, locations) = crowded_library(50);
    let outside = lib.temp.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, lib.root.join("nes").join("linked")).unwrap();
    let mut plan = rename_plan(&lib, &games);
    let stats = guard_bulk_rename_plan_with_stats(&lib.db, &mut plan, &locations);
    assert_eq!(stats.blocked, 50, "the cached refusal reaches every game");
    assert!(
        plan.entries
            .iter()
            .all(|e| e.reason.as_deref().unwrap().contains("linked folder"))
    );
    assert_eq!(stats.saves.folder_reads, 1, "{stats:?}");

    if nix_is_root() {
        return; // root reads anything; the permission case cannot be shown
    }
    let (lib, games, locations) = crowded_library(20);
    let locked = lib.root.join("nes").join("states");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut plan = rename_plan(&lib, &games);
    let stats = guard_bulk_rename_plan_with_stats(&lib.db, &mut plan, &locations);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(stats.blocked, 20, "{stats:?}");
    assert!(
        plan.entries
            .iter()
            .all(|e| e.reason.as_deref().unwrap().contains("could not be read"))
    );
}

fn nix_is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(str::to_owned))
        .is_some_and(|line| line.split_whitespace().nth(1) == Some("0"))
}

fn rss_kib(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0)
}

/// Benchmark (ignored by default): 4000 games in one folder.
/// `cargo-iso test -p archivefs-core --lib -- --ignored --nocapture bulk_guard_benchmark`
#[test]
#[ignore]
fn bulk_guard_benchmark_4000_games_in_one_folder() {
    let started = std::time::Instant::now();
    let (lib, games, locations) = crowded_library(4000);
    let setup = started.elapsed();
    let started = std::time::Instant::now();
    let mut plan = rename_plan(&lib, &games);
    let planning = started.elapsed();
    let rss_before = rss_kib("VmRSS:");
    let started = std::time::Instant::now();
    let stats = guard_bulk_rename_plan_with_stats(&lib.db, &mut plan, &locations);
    let guard = started.elapsed();
    let rss_after = rss_kib("VmRSS:");
    // The old behaviour, for comparison: every game searched with a fresh index.
    let started = std::time::Instant::now();
    let mut per_game = super::game_rename::SaveSearchStats::default();
    for game in &games {
        let mut fresh = SaveSearchIndex::default();
        let _ = find_retroarch_saves_with(game, &locations, &mut fresh);
        per_game.folder_reads += fresh.stats.folder_reads;
        per_game.entries_read += fresh.stats.entries_read;
    }
    let old_saves_only = started.elapsed();
    println!(
        "BENCH games=4000 setup={setup:?} plan={planning:?} guard={guard:?} \
         checked={} blocked={} save_folder_reads={} save_entries_read={} \
         save_entries_compared={} sidecar_folder_reads={} rss_before_kib={rss_before} \
         rss_after_kib={rss_after} peak_kib={} | OLD per-game save search only: \
         time={old_saves_only:?} folder_reads={} entries_read={}",
        stats.checked,
        stats.blocked,
        stats.saves.folder_reads,
        stats.saves.entries_read,
        stats.saves.entries_compared,
        stats.sidecar_folder_reads,
        rss_kib("VmHWM:"),
        per_game.folder_reads,
        per_game.entries_read,
    );
    assert_eq!(stats.blocked, 40);
}

// ---- mount boundaries (pretend mount points: tests cannot mount without root) ----

use super::rename_guard::guard_with_index;

fn mounted(paths: &[&Path]) -> SaveSearchIndex {
    SaveSearchIndex::with_mount_points(paths.iter().map(|p| std::fs::canonicalize(p).unwrap()))
}

fn reasons(plan: &OrganisationPlan) -> Vec<(String, String)> {
    plan.entries
        .iter()
        .filter(|e| e.status == OrganisationStatus::Blocked)
        .map(|e| {
            let name = e
                .source_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, e.reason.clone().unwrap_or_default())
        })
        .collect()
}

/// A folder standing in for a mounted archive: top-level files plus a
/// sub-folder whose contents must never be read.
fn archive_mount(at: &Path, top: &[&str], inner: &[&str]) {
    std::fs::create_dir_all(at.join("inner")).unwrap();
    for name in top {
        std::fs::write(at.join(name), b"x").unwrap();
    }
    for name in inner {
        std::fs::write(at.join("inner").join(name), b"x").unwrap();
    }
}

#[test]
fn an_unrelated_sibling_mount_is_skipped_and_never_entered() {
    let (lib, games, locations) = crowded_library(50);
    let mount = lib.root.join("nes").join("Archive Pack");
    // `inner/Game 00003.srm` is below the mount's top level: never read.
    archive_mount(
        &mount,
        &["Other Game.nes", "readme.txt"],
        &["Game 00003.srm"],
    );
    let mut plan = rename_plan(&lib, &games);
    let stats = guard_with_index(&lib.db, &mut plan, &locations, mounted(&[&mount]));
    assert_eq!(reasons(&plan).len(), 1, "{:?}", reasons(&plan));
    assert_eq!(reasons(&plan)[0].0, "Game 00000.nes", "the one real save");
    assert_eq!(stats.saves.mounts_seen, 1, "{stats:?}");
    // nes, states, mount top level, save root, Mesen, FCEUmm, nested: 7.
    assert_eq!(
        stats.saves.folder_reads, 7,
        "the mount's inner folder is not read: {stats:?}"
    );
}

#[test]
fn a_mount_holding_this_games_save_refuses_only_that_game() {
    let (lib, games, locations) = crowded_library(50);
    let mount = lib.root.join("nes").join("Archive Pack");
    archive_mount(&mount, &["Game 00007.srm", "Game 00007 (Beta).srm"], &[]);
    let mut plan = rename_plan(&lib, &games);
    guard_with_index(&lib.db, &mut plan, &locations, mounted(&[&mount]));
    let blocked = reasons(&plan);
    assert_eq!(blocked.len(), 2, "{blocked:?}");
    let seven = blocked
        .iter()
        .find(|(name, _)| name == "Game 00007.nes")
        .unwrap();
    assert!(
        seven.1.contains("separate mounted filesystem"),
        "{}",
        seven.1
    );
    assert!(
        seven.1.contains("a save named after this game"),
        "{}",
        seven.1
    );
}

#[test]
fn nested_mounts_are_never_entered_and_deep_save_mounts_refuse() {
    // A mount inside a mount next to the games: neither inner level is read.
    let (lib, games, locations) = crowded_library(30);
    let outer = lib.root.join("nes").join("Pack");
    archive_mount(&outer, &["unrelated.nes"], &["Game 00001.srm"]);
    let inner = outer.join("inner");
    let mut plan = rename_plan(&lib, &games);
    let stats = guard_with_index(&lib.db, &mut plan, &locations, mounted(&[&outer, &inner]));
    assert_eq!(reasons(&plan).len(), 1, "{:?}", reasons(&plan));
    assert_eq!(stats.saves.folder_reads, 7, "{stats:?}");

    // A mount as a grandchild of the save folder: its top level is checked,
    // as any grandchild is, and an unrelated one is skipped.
    let (lib, games, locations) = crowded_library(30);
    let nested = locations.directories[0].join("FCEUmm").join("nested");
    archive_mount(&nested, &["Someone Else.srm"], &["Game 00002.srm"]);
    let mut plan = rename_plan(&lib, &games);
    guard_with_index(&lib.db, &mut plan, &locations, mounted(&[&nested]));
    assert_eq!(reasons(&plan).len(), 1, "{:?}", reasons(&plan));

    // A mount directly in the save folder holding folders of its own: the
    // search would look inside those, which means entering the mount, so its
    // relevance cannot be shown and every game refuses with that reason.
    let (lib, games, locations) = crowded_library(30);
    let core = locations.directories[0].join("Mounted Core");
    archive_mount(&core, &["Someone Else.srm"], &[]);
    let mut plan = rename_plan(&lib, &games);
    guard_with_index(&lib.db, &mut plan, &locations, mounted(&[&core]));
    let blocked = reasons(&plan);
    assert_eq!(blocked.len(), 30, "{blocked:?}");
    assert!(
        blocked
            .iter()
            .all(|(_, r)| r.contains("cannot be ruled out without searching the mount"))
    );

    // The same mount with no folders of its own: top level checked, skipped.
    std::fs::remove_dir(core.join("inner")).unwrap();
    let mut plan = rename_plan(&lib, &games);
    guard_with_index(&lib.db, &mut plan, &locations, mounted(&[&core]));
    assert_eq!(reasons(&plan).len(), 1, "{:?}", reasons(&plan));
}

#[test]
fn an_unreadable_mount_refuses_every_game_that_would_check_it() {
    if nix_is_root() {
        return; // root reads anything; the permission case cannot be shown
    }
    let (lib, games, locations) = crowded_library(20);
    let mount = lib.root.join("nes").join("Locked Pack");
    archive_mount(&mount, &["x.nes"], &[]);
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&mount, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut plan = rename_plan(&lib, &games);
    let index = mounted(&[&mount]);
    let stats = guard_with_index(&lib.db, &mut plan, &locations, index);
    std::fs::set_permissions(&mount, std::fs::Permissions::from_mode(0o755)).unwrap();
    let blocked = reasons(&plan);
    assert_eq!(blocked.len(), 20, "{blocked:?} {stats:?}");
    assert!(
        blocked.iter().all(|(_, r)| r.contains("could not be read")),
        "{blocked:?}"
    );
}

#[test]
fn games_inside_a_mounted_archive_are_searched_normally() {
    let (lib, games, locations) = crowded_library(20);
    let folder = lib.root.join("nes");
    std::fs::write(folder.join("states").join("Game 00005.state"), b"s").unwrap();
    let mut plan = rename_plan(&lib, &games);
    let stats = guard_with_index(&lib.db, &mut plan, &locations, mounted(&[&folder]));
    let blocked: Vec<String> = reasons(&plan).into_iter().map(|(n, _)| n).collect();
    assert_eq!(blocked, ["Game 00000.nes", "Game 00005.nes"]);
    assert_eq!(
        stats.saves.mounts_seen, 0,
        "its own sub-folders share its filesystem"
    );
}

#[test]
fn one_game_and_bulk_checks_agree_with_mounts_present() {
    let (lib, games, locations) = crowded_library(40);
    let mount = lib.root.join("nes").join("Archive Pack");
    archive_mount(&mount, &["Game 00011.srm"], &["Game 00012.srm"]);
    let mut shared = mounted(&[&mount]);
    for game in &games {
        let alone = find_retroarch_saves_with(game, &locations, &mut mounted(&[&mount]));
        let bulk = find_retroarch_saves_with(game, &locations, &mut shared);
        assert_eq!(alone, bulk, "{}", game.display());
    }
    assert!(find_retroarch_saves_with(&games[11], &locations, &mut mounted(&[&mount])).is_err());
    assert!(
        find_retroarch_saves_with(&games[12], &locations, &mut mounted(&[&mount]))
            .unwrap()
            .is_empty()
    );
}

/// Benchmark (ignored by default): 4000 games in one folder beside five
/// mounted-archive stand-ins of 200 entries each, plus a nested mount.
#[test]
#[ignore]
fn bulk_guard_benchmark_4000_games_with_mounts() {
    let started = std::time::Instant::now();
    let (lib, games, locations) = crowded_library(4000);
    let mut mounts = Vec::new();
    for pack in 0..5 {
        let at = lib.root.join("nes").join(format!("Pack {pack}"));
        let names: Vec<String> = (0..200)
            .map(|i| format!("Other {pack} {i:03}.nes"))
            .collect();
        let top: Vec<&str> = names.iter().map(String::as_str).collect();
        archive_mount(&at, &top, &["Game 00001.srm"]);
        mounts.push(at.join("inner"));
        mounts.push(at);
    }
    let setup = started.elapsed();
    let mut plan = rename_plan(&lib, &games);
    let refs: Vec<&Path> = mounts.iter().map(PathBuf::as_path).collect();
    let index = mounted(&refs);
    let rss_before = rss_kib("VmRSS:");
    let started = std::time::Instant::now();
    let stats = guard_with_index(&lib.db, &mut plan, &locations, index);
    let guard = started.elapsed();
    println!(
        "BENCH-MOUNTS games=4000 mounts=5(+5 nested) setup={setup:?} guard={guard:?} checked={} \
         blocked={} save_folder_reads={} save_entries_read={} save_entries_compared={} \
         mounts_seen={} sidecar_folder_reads={} rss_before_kib={rss_before} rss_after_kib={} peak_kib={}",
        stats.checked,
        stats.blocked,
        stats.saves.folder_reads,
        stats.saves.entries_read,
        stats.saves.entries_compared,
        stats.saves.mounts_seen,
        stats.sidecar_folder_reads,
        rss_kib("VmRSS:"),
        rss_kib("VmHWM:"),
    );
    assert_eq!(stats.blocked, 40);
    assert_eq!(stats.saves.folder_reads, 6 + 5, "{stats:?}");
}

/// Profile (ignored by default): where the guard's time goes, at two sizes.
#[test]
#[ignore]
fn bulk_guard_phase_profile() {
    use super::game_rename::{owning_source_root, refuse_arcade_set};
    use std::time::Instant;
    for count in [1000usize, 2000] {
        let (lib, games, locations) = crowded_library(count);
        let t = Instant::now();
        for _ in &games {
            lib.db.list_source_folders().unwrap();
        }
        let folders = t.elapsed();
        let t = Instant::now();
        for game in &games {
            lib.db.find_archive_id_by_absolute_path(game).unwrap();
        }
        let find_id = t.elapsed();
        let t = Instant::now();
        for game in &games {
            let _ = live_resolution_for(&lib.db, game, 1);
        }
        let resolution = t.elapsed();
        let t = Instant::now();
        for game in &games {
            let root = owning_source_root(&lib.db, game).unwrap();
            let _ = refuse_arcade_set(&lib.db, game, &root, 1);
        }
        let arcade_total = t.elapsed();
        let t = Instant::now();
        let mut index = SaveSearchIndex::default();
        for game in &games {
            let _ = find_retroarch_saves_with(game, &locations, &mut index);
        }
        let saves = t.elapsed();
        let mut plan = rename_plan(&lib, &games);
        let t = Instant::now();
        guard_bulk_rename_plan_with_stats(&lib.db, &mut plan, &locations);
        let guard = t.elapsed();
        println!(
            "PROFILE games={count} guard={guard:?} | list_source_folders x{count}={folders:?} \
             find_archive_id x{count}={find_id:?} live_resolution x{count}={resolution:?} \
             source_root+arcade x{count}={arcade_total:?} saves(shared)={saves:?}"
        );
    }
}

/// Real mount boundary (ignored by default; needs a real mount, made without
/// root by a user namespace). `EMUWIZ_REAL_MOUNT_GAMES` names an empty
/// scratch folder whose `Pack` sub-folder is a separate mount, e.g.
/// `bwrap --dev-bind / / --unshare-user --tmpfs $DIR/Pack -- <test binary> ...`.
#[test]
#[ignore]
fn a_real_mount_beside_the_game_is_detected_by_device_and_mount_list() {
    let Some(games) = std::env::var_os("EMUWIZ_REAL_MOUNT_GAMES").map(PathBuf::from) else {
        panic!("EMUWIZ_REAL_MOUNT_GAMES is not set");
    };
    let pack = games.join("Pack");
    use std::os::unix::fs::MetadataExt;
    let device = |p: &Path| std::fs::metadata(p).unwrap().dev();
    assert_ne!(
        device(&games),
        device(&pack),
        "Pack must be a separate mount"
    );
    let game = games.join("Real Game.nes");
    std::fs::write(&game, b"g").unwrap();
    std::fs::write(pack.join("Someone Else.srm"), b"s").unwrap();
    std::fs::create_dir(pack.join("deep")).unwrap();
    std::fs::write(pack.join("deep").join("Real Game.srm"), b"s").unwrap();
    let none = RetroArchSaveLocations::default();
    // Real detection (default index reads /proc/self/mountinfo and devices).
    let mut index = SaveSearchIndex::default();
    assert_eq!(
        find_retroarch_saves_with(&game, &none, &mut index),
        Ok(vec![])
    );
    assert_eq!(index.stats.mounts_seen, 1, "{:?}", index.stats);
    assert_eq!(
        index.stats.folder_reads, 2,
        "game folder + mount top level only"
    );
    std::fs::write(pack.join("Real Game.srm"), b"s").unwrap();
    let refused = find_retroarch_saves(&game, &none).unwrap_err();
    assert!(refused.contains("separate mounted filesystem"), "{refused}");
    println!("REAL-MOUNT ok: {refused}");
}

#[test]
fn the_one_pass_platform_lookup_matches_the_per_file_lookup() {
    use super::rename_guard::LiveResolutions;
    let mut lib = Lib::new();
    let mut files = vec![
        lib.game("nes", "Plain.nes", "nes"),
        lib.game("arcade", "galaga.zip", "arcade"),
        lib.game("psx", "quest.iso", "psx"),
    ];
    let outside = lib.root.join("nes").join("Not Scanned.nes");
    std::fs::write(&outside, b"x").unwrap();
    files.push(outside);
    files.push(lib.root.join("nes").join("gone.nes"));
    // One path, two archive rows (a nested second source): both lookups
    // treat it as unknown rather than picking one.
    let psx = lib.root.join("psx");
    lib.db
        .register_source_folders(&[lib.root.clone(), psx.clone()])
        .unwrap();
    let nested = lib
        .db
        .list_source_folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.path == psx)
        .unwrap()
        .id;
    let archive = crate::Archive::from_path_in_root(&psx.join("quest.iso"), &psx).unwrap();
    lib.db.upsert_archive(nested, &psx, &archive).unwrap();
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&psx.join("quest.iso"))
            .is_err()
    );

    let batched = LiveResolutions::read(&lib.db);
    for generation in [0, 1, 7] {
        for file in &files {
            assert_eq!(
                batched.for_path(file, generation),
                live_resolution_for(&lib.db, file, generation),
                "{} at generation {generation}",
                file.display()
            );
        }
    }
}

#[test]
fn grouped_collision_detection_matches_the_all_pairs_version() {
    use super::plan::{detect_collisions, detect_collisions_pairwise};
    let mut lib = Lib::new();
    let source = lib.game("nes", "Source.nes", "nes");
    let template = lib
        .plan(
            OrganisationMode::RenameInPlace,
            &[(&source, "Template (USA).nes")],
        )
        .entries
        .remove(0);
    let here = lib.root.join("nes");
    let there = lib.root.join("other");
    std::fs::create_dir_all(&there).unwrap();
    for existing in ["Taken (USA).nes", "case (usa).nes", "Linked.nes"] {
        std::fs::write(here.join(existing), b"x").unwrap();
    }
    std::os::unix::fs::symlink(&source, here.join("Link Here.nes")).unwrap();
    let pool = [
        here.join("A.nes"),
        here.join("a.nes"),
        here.join("Taken (USA).nes"),
        here.join("TAKEN (usa).nes"),
        here.join("Case (USA).nes"),
        here.join("Link Here.nes"),
        here.join("B.nes"),
        there.join("A.nes"),
        there.join("b.NES"),
        PathBuf::new(),
    ];
    let statuses = [
        OrganisationStatus::Suggested,
        OrganisationStatus::Suggested,
        OrganisationStatus::AlreadyOrganised,
        OrganisationStatus::Blocked,
        OrganisationStatus::Conflict,
    ];
    let modes = [
        OrganisationMode::RenameInPlace,
        OrganisationMode::BuildLinkedLibrary,
    ];
    let mut seed: u64 = 0x5eed;
    let mut next = |bound: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % bound
    };
    for _ in 0..60 {
        let size = 1 + next(120);
        let entries: Vec<_> = (0..size)
            .map(|_| {
                let mut entry = template.clone();
                entry.destination_path = pool[next(pool.len())].clone();
                entry.status = statuses[next(statuses.len())];
                entry.reason = (next(3) == 0).then(|| "earlier reason".to_string());
                entry.mode = modes[next(modes.len())];
                if next(4) == 0 {
                    entry.source_path = source.clone();
                }
                entry
            })
            .collect();
        let mut grouped = entries.clone();
        let mut pairwise = entries;
        detect_collisions(&lib.root, &mut grouped);
        detect_collisions_pairwise(&lib.root, &mut pairwise);
        assert_eq!(grouped, pairwise);
    }
}

/// Profile (ignored by default): where catalogue preparation and the Apply
/// revalidation spend their time.
#[test]
#[ignore]
fn catalogue_and_revalidate_phase_profile() {
    use std::time::Instant;
    let sizes: Vec<usize> = std::env::var("PREVIEW_BENCH_SIZES")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1000, 2000]);
    for count in sizes {
        let (lib, games, _) = crowded_library(count);
        let plan = rename_plan(&lib, &games);
        let approved: BTreeSet<String> = plan
            .suggested()
            .map(|e| e.source_path.to_string_lossy().into_owned())
            .collect();
        let t = Instant::now();
        let mut transaction = build_organisation_transaction(&plan, &approved, 1).unwrap();
        let build = t.elapsed();
        let t = Instant::now();
        crate::database::organisation_paths::bind(&mut transaction, &lib.db).unwrap();
        let bind = t.elapsed();
        let t = Instant::now();
        let mut found = 0;
        for entry in &plan.entries {
            if transaction
                .entries
                .iter()
                .any(|e| e.source_path == entry.source_path)
            {
                found += 1;
            }
        }
        let copy_back = t.elapsed();
        let t = Instant::now();
        for entry in plan.suggested() {
            let _ = live_resolution_for(&lib.db, &entry.source_path, 1);
        }
        let resolutions = t.elapsed();
        let t = Instant::now();
        for entry in plan.suggested() {
            let candidate = OrganisationCandidate {
                source_path: entry.source_path.clone(),
                resolution: live_resolution_for(&lib.db, &entry.source_path, 1),
                canonical_name: entry
                    .destination_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned()),
                content_classification: None,
                original_metadata: Default::default(),
            };
            let _ = build_organisation_plan(&OrganisationPlanRequest {
                master_root: &plan.master_root,
                mode: plan.mode,
                content_policy: plan.content_policy,
                candidates: std::slice::from_ref(&candidate),
                generation: 1,
                unknown_folder: None,
                link_target_roots: None,
            });
        }
        let replans = t.elapsed();
        println!(
            "CATALOGUE-PROFILE games={count} | prepare: build_transaction={build:?} bind={bind:?} \
             copy_back_find={copy_back:?} ({found}) | revalidate: live_resolution={resolutions:?} \
             resolution+one_entry_plan={replans:?}"
        );
    }
}

#[test]
fn batched_apply_revalidation_decides_exactly_as_the_per_entry_check() {
    use super::transaction::{
        revalidate_organisation_plan, revalidate_organisation_plan_per_entry,
    };
    let (mut lib, games, _) = crowded_library(40);
    let plan = rename_plan(&lib, &games);
    let database = lib.temp.path().join("library.sqlite3");
    let same_names = |path: &Path| {
        let stem = path.file_stem()?.to_string_lossy().into_owned();
        Some(format!("{stem} (USA).nes"))
    };
    let both = |plan: &OrganisationPlan, names: &dyn Fn(&Path) -> Option<String>| {
        let batched = revalidate_organisation_plan(plan, &database, names);
        let per_entry = revalidate_organisation_plan_per_entry(plan, &database, names);
        assert_eq!(batched, per_entry);
        batched
    };
    assert_eq!(both(&plan, &same_names), Ok(()));

    // A proposed name that changed since the preview.
    let renamed = |path: &Path| {
        if path.ends_with("Game 00007.nes") {
            Some("Something Else.nes".to_string())
        } else {
            same_names(path)
        }
    };
    assert!(
        both(&plan, &renamed)
            .unwrap_err()
            .contains("Game 00007.nes")
    );

    // The destination appeared on disk; a case-only sibling appeared.
    let nes = lib.root.join("nes");
    std::fs::write(nes.join("Game 00012 (USA).nes"), b"x").unwrap();
    assert!(
        both(&plan, &same_names)
            .unwrap_err()
            .contains("Game 00012.nes")
    );
    std::fs::remove_file(nes.join("Game 00012 (USA).nes")).unwrap();
    std::fs::write(nes.join("GAME 00003 (usa).nes"), b"x").unwrap();
    assert!(
        both(&plan, &same_names)
            .unwrap_err()
            .contains("Game 00003.nes")
    );
    std::fs::remove_file(nes.join("GAME 00003 (usa).nes")).unwrap();

    // Two failures: both report the first in plan order.
    std::fs::write(nes.join("Game 00030 (USA).nes"), b"x").unwrap();
    std::fs::write(nes.join("Game 00020 (USA).nes"), b"x").unwrap();
    assert!(
        both(&plan, &same_names)
            .unwrap_err()
            .contains("Game 00020.nes")
    );
    for name in ["Game 00030 (USA).nes", "Game 00020 (USA).nes"] {
        std::fs::remove_file(nes.join(name)).unwrap();
    }

    // The platform changed; the source file went away.
    let id = lib
        .db
        .find_archive_id_by_absolute_path(&games[5])
        .unwrap()
        .unwrap();
    lib.db.set_manual_platform(id, "snes").unwrap();
    assert!(
        both(&plan, &same_names)
            .unwrap_err()
            .contains("Game 00005.nes")
    );
    lib.db.set_manual_platform(id, "nes").unwrap();
    std::fs::remove_file(&games[9]).unwrap();
    assert!(
        both(&plan, &same_names)
            .unwrap_err()
            .contains("Game 00009.nes")
    );
}

/// Benchmark (ignored by default; meant for `--release`): the preview and
/// Apply-check stages Blue runs, through the core functions, at 1000, 2000
/// and 4000 games (override with PREVIEW_BENCH_SIZES=1000,2000).
#[test]
#[ignore]
fn core_pipeline_benchmark() {
    use super::rename_guard::LiveResolutions;
    use std::time::Instant;
    let sizes: Vec<usize> = std::env::var("PREVIEW_BENCH_SIZES")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1000, 2000, 4000]);
    let rss = |field: &str| rss_kib(field);
    for count in sizes {
        let started = Instant::now();
        let (lib, games, locations) = crowded_library(count);
        let setup = started.elapsed();
        let database = lib.temp.path().join("library.sqlite3");
        let rss_start = rss("VmRSS:");
        // Preview.
        let t = Instant::now();
        let resolutions = LiveResolutions::read(&lib.db);
        let candidates: Vec<OrganisationCandidate> = games
            .iter()
            .map(|game| OrganisationCandidate {
                source_path: game.clone(),
                resolution: resolutions.for_path(game, 1),
                canonical_name: Some(format!(
                    "{} (USA).nes",
                    game.file_stem().unwrap().to_string_lossy()
                )),
                content_classification: None,
                original_metadata: Default::default(),
            })
            .collect();
        let lookup = t.elapsed();
        let t = Instant::now();
        let mut plan = build_organisation_plan(&OrganisationPlanRequest {
            master_root: &lib.root,
            mode: OrganisationMode::RenameInPlace,
            content_policy: ContentSelectionPolicy::AllEntries,
            candidates: &candidates,
            generation: 1,
            unknown_folder: None,
            link_target_roots: None,
        });
        let planning = t.elapsed();
        let t = Instant::now();
        let stats = guard_bulk_rename_plan_with_stats(&lib.db, &mut plan, &locations);
        let guard = t.elapsed();
        let t = Instant::now();
        super::transaction::prepare_organisation_catalogue(&mut plan, &lib.db).unwrap();
        let catalogue = t.elapsed();
        let preview = lookup + planning + guard + catalogue;
        let rss_preview = rss("VmRSS:");
        // Apply checks (nothing is applied).
        let approved: BTreeSet<String> = plan
            .suggested()
            .map(|e| e.source_path.to_string_lossy().into_owned())
            .collect();
        let names =
            |path: &Path| Some(format!("{} (USA).nes", path.file_stem()?.to_string_lossy()));
        let t = Instant::now();
        let revalidated =
            super::transaction::revalidate_organisation_plan(&plan, &database, &names);
        let revalidate = t.elapsed();
        let t = Instant::now();
        let reguarded = refuse_if_newly_blocked(&lib.db, &plan, &approved, &locations);
        let reguard = t.elapsed();
        println!(
            "CORE-PIPELINE games={count} setup={setup:?} | PREVIEW total={preview:?} = \
             lookup={lookup:?} plan={planning:?} guard={guard:?} catalogue={catalogue:?} | \
             suggested={} blocked={} | rss_start_kib={rss_start} rss_after_preview_kib={rss_preview} \
             peak_kib={} | APPLY checks total={:?} = revalidate={revalidate:?} ({}) reguard={reguard:?} ({})",
            plan.suggested().count(),
            stats.blocked,
            rss("VmHWM:"),
            revalidate + reguard,
            if revalidated.is_ok() { "ok" } else { "refused" },
            if reguarded.is_ok() { "ok" } else { "refused" },
        );
        assert!(revalidated.is_ok() && reguarded.is_ok());
    }
}
