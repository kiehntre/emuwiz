//! One reusable verified game, built only through production code paths.
//!
//! A tiny synthetic SNES cartridge and a matching No-Intro-style DAT are
//! written into a disposable profile. The source is registered with
//! `add_source_folder_at`, scanned with `scan_source_folder_at`, audited with
//! the real DAT audit and saved through the DAT page's own `persist_audit`;
//! the Library is then loaded exactly as Blue loads it. Nothing is inserted
//! into the database directly and no path outside the temporary directory is
//! read or written.
//!
//! The same fixture can be put into the mismatched, changed and missing
//! states a real collection reaches, so tests can check that only the
//! normal state verifies.

use super::library::{Game, Library};
use archivefs_core::dat::sources::audit_cache::AuditCacheConfig;
use archivefs_core::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use archivefs_core::dat::sources::{
    DatSourceEntry, DatSourceKind, DatSourceRegistry, save_dat_sources_config_to,
};
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::{ScanPersistSummary, add_source_folder_at, scan_source_folder_at};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(crate) const DAT_SOURCE_ID: &str = "fixture-snes";
pub(crate) const GAME_NAME: &str = "Synthetic Hero (World)";
pub(crate) const ROM_FILE: &str = "Synthetic Hero (World).sfc";

/// The ROM state the fixture is built in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RomState {
    /// The bytes the DAT lists.
    Normal,
    /// Same name and size, bytes the DAT does not list.
    Mismatched,
}

pub(crate) struct VerifiedGame {
    dir: tempfile::TempDir,
    /// Config and data root: `EMUWIZ_CONFIG_HOME` and `EMUWIZ_DATA_HOME`
    /// both point here for a real GUI process.
    pub(crate) profile: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) database: PathBuf,
    pub(crate) roms: PathBuf,
    pub(crate) rom: PathBuf,
    pub(crate) dat: PathBuf,
}

/// Deterministic, harmless cartridge bytes (not a real program).
pub(crate) fn rom_bytes(state: RomState) -> Vec<u8> {
    let seed: u8 = match state {
        RomState::Normal => 0x5a,
        RomState::Mismatched => 0xa5,
    };
    (0..4096u32)
        .map(|index| seed ^ (index as u8).wrapping_mul(31))
        .collect()
}

impl VerifiedGame {
    /// Registered, scanned and DAT-verified, ready for the Library.
    pub(crate) fn new() -> Self {
        Self::built(RomState::Normal)
    }

    /// The whole pipeline run for a ROM in `state`.
    pub(crate) fn built(state: RomState) -> Self {
        let fixture = Self::unregistered(state);
        fixture.register_source();
        fixture.scan();
        fixture.verify();
        fixture
    }

    /// Files and DAT source only: no source registered, nothing scanned.
    pub(crate) fn unregistered(state: RomState) -> Self {
        let dir = tempfile::tempdir().expect("fixture root");
        let root = dir.path().to_path_buf();
        let profile = root.join("profile");
        let roms = root.join("collection");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(&roms).unwrap();
        std::fs::create_dir_all(root.join("mounts")).unwrap();
        let config = profile.join("config.toml");
        // A config that exists keeps add_source_folder_at from writing a
        // starter config that names the real home folder's mount root.
        std::fs::write(
            &config,
            format!(
                "source_folders = []\nmount_root = \"{}\"\n",
                root.join("mounts").display()
            ),
        )
        .unwrap();
        let rom = roms.join(ROM_FILE);
        std::fs::write(&rom, rom_bytes(state)).unwrap();

        // Hashes of the listed bytes come from core's own hasher.
        let listed = rom_bytes(RomState::Normal);
        let reference = root.join("listed.bin");
        std::fs::write(&reference, &listed).unwrap();
        let hashes = archivefs_core::identity_source::hashing::hash_file_reporting(
            &reference,
            &TrustedRoots::none(),
            None,
            &|_| {},
        )
        .unwrap();
        std::fs::remove_file(&reference).unwrap();
        let dat = root.join("dats").join("snes.dat");
        std::fs::create_dir_all(dat.parent().unwrap()).unwrap();
        std::fs::write(
            &dat,
            format!(
                "<?xml version=\"1.0\"?><datafile><header>\
                 <name>Nintendo - Super Nintendo Entertainment System</name>\
                 <description>Nintendo - Super Nintendo Entertainment System</description>\
                 <version>1</version><author>No-Intro</author><homepage>No-Intro</homepage>\
                 </header><game name=\"{GAME_NAME}\"><description>{GAME_NAME}</description>\
                 <rom name=\"{ROM_FILE}\" size=\"{}\" crc=\"{}\" sha1=\"{}\"/></game></datafile>",
                listed.len(),
                hashes.crc32,
                hashes.sha1
            ),
        )
        .unwrap();
        let mut registry = DatSourceRegistry::new();
        registry
            .add(DatSourceEntry::new(
                DAT_SOURCE_ID.into(),
                "Fixture No-Intro SNES".into(),
                dat.clone(),
                DatSourceKind::File,
            ))
            .unwrap();
        save_dat_sources_config_to(profile.join("dat_sources.toml"), &registry.to_config())
            .unwrap();
        Self {
            database: profile.join("library.sqlite3"),
            dir,
            profile,
            config,
            roms,
            rom,
            dat,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Production source management: the same call "Add source" makes.
    pub(crate) fn register_source(&self) {
        add_source_folder_at(&self.config, &self.database, &self.roms).expect("register source");
    }

    /// Production scan of the registered source.
    pub(crate) fn scan(&self) -> ScanPersistSummary {
        let summary =
            scan_source_folder_at(&self.config, &self.database, &self.roms, "fixture-scan")
                .expect("scan source");
        assert!(
            summary.folder_errors.is_empty(),
            "{:?}",
            summary.folder_errors
        );
        summary
    }

    /// Real DAT audit, saved through the DAT page's own writer.
    pub(crate) fn verify(&self) -> crate::dat_sources_page::FixtureDatSaveOutcome {
        let outcome = run_dat_audit_with_cache(
            &DatAuditRequest {
                source_id: DAT_SOURCE_ID.into(),
                source_display_name: "No-Intro: Nintendo - Super Nintendo Entertainment System"
                    .into(),
                dat_path: self.dat.clone(),
                dat_kind: DatSourceKind::File,
                scan_root: self.roms.clone(),
                limits: Default::default(),
                policy: None,
                platform: None,
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
            AuditCacheConfig::Disabled,
        )
        .expect("DAT audit");
        let (result, _) = crate::dat_sources_page::persist_audit(
            Some(&self.database),
            &outcome,
            1,
            &AtomicBool::new(false),
        );
        result.outcome
    }

    /// The Library exactly as Blue loads it from this profile.
    pub(super) fn library(&self) -> Library {
        super::dat_exact_tests::load_library(&self.database).expect("load library")
    }

    pub(super) fn game(&self) -> Option<Game> {
        self.library()
            .games
            .iter()
            .find(|game| game.archive.absolute_path == self.rom)
            .cloned()
    }

    /// The Library's platform for the game, only while Blue shows it verified.
    pub(crate) fn verified_platform(&self) -> Option<String> {
        self.game()
            .filter(|game| game.identified)
            .and_then(|game| game.archive.platform)
    }

    /// The file is rewritten in place with other bytes of the same size.
    pub(crate) fn change_rom(&self) {
        std::fs::write(&self.rom, rom_bytes(RomState::Mismatched)).unwrap();
    }

    pub(crate) fn remove_rom(&self) {
        std::fs::remove_file(&self.rom).unwrap();
    }
}

impl VerifiedGame {
    /// The Playing Library page as a user leaves it after choosing this
    /// fixture's catalogue in the shared picker: the inventory is the
    /// profile's saved `dat_sources.toml`, and the selection is the picker
    /// row's own reference. No test-only DAT path is involved.
    pub(crate) fn playing_library_page(
        &self,
        destination: &Path,
    ) -> crate::playing_library_page::PlayingLibraryPageState {
        use archivefs_core::dat::sources::load_dat_sources_config_from;
        let config = load_dat_sources_config_from(self.profile.join("dat_sources.toml")).unwrap();
        let (local_registry, problems) = DatSourceRegistry::from_config(&config);
        assert!(problems.is_empty(), "{problems:?}");
        let mut state = crate::playing_library_page::PlayingLibraryPageState::with_journal_dir(
            self.root().join("journals"),
        );
        state.catalogue_picker.ensure_loaded_with(
            crate::dat_catalogue_picker::CatalogueInventorySnapshot {
                local_registry,
                managed_sources: Default::default(),
                managed_root: self.profile.join("managed-dats"),
            },
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !state.catalogue_picker.poll() {
            assert!(
                std::time::Instant::now() < deadline,
                "catalogue list never loaded"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let row = state
            .catalogue_picker
            .summaries()
            .iter()
            .find(|summary| summary.technical_path.as_deref() == Some(self.dat.as_path()))
            .expect("the fixture catalogue is listed");
        state.selected_catalogue = Some(row.reference.clone());
        state.source_root_draft = self.roms.display().to_string();
        state.destination_root_draft = destination.display().to_string();
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(game: &Game) -> String {
        format!(
            "identified={} summary={} status={} exact={:?} platform={:?} health={}",
            game.identified,
            game.identity_summary(),
            game.status(),
            game.dat_exact_label(),
            game.archive.platform,
            game.archive.last_known_health
        )
    }

    #[test]
    fn normal_game_is_discovered_and_verified_by_the_real_pipeline() {
        let fixture = VerifiedGame::unregistered(RomState::Normal);
        fixture.register_source();
        let scan = fixture.scan();
        assert!(scan.counts.archives_added == 1, "{:?}", scan.counts);
        assert_eq!(
            fixture.verify(),
            crate::dat_sources_page::FixtureDatSaveOutcome::Success
        );
        let game = fixture.game().expect("scanned into the Library");
        assert!(game.identified, "{}", describe(&game));
        assert_eq!(game.identity_summary(), "Verified", "{}", describe(&game));
        assert_eq!(
            game.archive.platform.as_deref(),
            Some("SNES"),
            "{}",
            describe(&game)
        );
        assert!(game.dat_exact_label().is_some(), "{}", describe(&game));
    }

    #[test]
    fn mismatched_bytes_are_scanned_but_never_verified() {
        let fixture = VerifiedGame::built(RomState::Mismatched);
        let game = fixture.game().expect("still discovered");
        assert!(!game.identified, "{}", describe(&game));
        assert_ne!(game.identity_summary(), "Verified", "{}", describe(&game));
    }

    #[test]
    fn a_changed_file_loses_its_verification_after_rescan_and_reaudit() {
        let fixture = VerifiedGame::new();
        assert!(fixture.game().unwrap().identified);
        fixture.change_rom();
        fixture.scan();
        fixture.verify();
        let game = fixture.game().expect("still catalogued");
        assert!(!game.identified, "{}", describe(&game));
    }

    #[test]
    fn a_missing_file_is_reported_and_not_verified() {
        let fixture = VerifiedGame::new();
        let before = fixture.game().expect("verified before removal");
        assert!(before.identified, "{}", describe(&before));
        fixture.remove_rom();
        fixture.scan();
        // A fresh Library load, exactly as Blue reloads after a scan.
        let game = fixture.game().expect("a missing game stays catalogued");
        let text = describe(&game);
        assert!(
            game.archive.last_verified_missing_at.is_some(),
            "file state must be Missing: {text}"
        );
        assert!(
            !game.identified,
            "a missing file is not currently verified: {text}"
        );
        assert_ne!(game.identity_summary(), "Verified", "{text}");
        assert!(game.dat_exact_label().is_none(), "{text}");
        // The historical audit evidence is kept; only its projection changed.
        let saved: String = rusqlite::Connection::open(&fixture.database)
            .unwrap()
            .query_row(
                "SELECT verification_state FROM library_dat_identities WHERE archive_id = ?1",
                [game.archive.id],
                |row| row.get(0),
            )
            .expect("the saved DAT evidence is not deleted");
        assert_eq!(saved, "verified_single_match");
    }

    #[test]
    fn library_and_playing_library_survive_a_restart() {
        let fixture = VerifiedGame::new();
        let destination = fixture.root().join("playing");
        let mut page = fixture.playing_library_page(&destination);
        page.preview();
        assert!(page.error().is_none(), "{:?}", page.error());
        assert_eq!(page.plan().expect("plan").elected_games.len(), 1);
        page.request_apply();
        page.confirm_apply();
        assert!(page.apply_error().is_none(), "{:?}", page.apply_error());
        let applied = page.applied().expect("applied").transaction_id.clone();
        drop(page);

        // "Restart": nothing in memory survives, only the profile on disk.
        let game = fixture.game().expect("still in the Library");
        assert!(game.identified, "{}", describe(&game));
        let journal = archivefs_core::dat::rename_apply::journal::read_journal(
            &archivefs_core::dat::rename_apply::journal::journal_path(
                &fixture.root().join("journals"),
                &applied,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            journal.state,
            archivefs_core::dat::rename_apply::model::TransactionState::Applied
        );
        let links: Vec<_> = walk(&destination);
        assert_eq!(links.len(), 1, "{links:?}");
        assert_eq!(std::fs::read_link(&links[0]).unwrap(), fixture.rom);
        // A new page, picking the catalogue again, sees the same game.
        let mut again = fixture.playing_library_page(&destination);
        again.preview();
        assert!(again.error().is_none(), "{:?}", again.error());
        assert_eq!(again.plan().expect("plan").elected_games.len(), 1);
    }

    /// A confirmed Check My Games verification asks Blue to reload saved
    /// identities once, so Browse & Play stops showing "Not checked".
    #[test]
    fn a_confirmed_verification_reloads_blue_identities_once() {
        let context = eframe::egui::Context::default();
        let mut workflows = super::super::native_workflows::NativeWorkflows::new(context.clone());
        let mut activity = super::super::activity::Activity::default();
        workflows.poll(&context, &mut activity);
        let _ = workflows.take_dat_identity_reload();
        workflows.poll(&context, &mut activity);
        assert!(
            !workflows.take_dat_identity_reload(),
            "nothing new, no reload"
        );
        workflows
            .app
            .sources_ui
            .dat_sources_ui
            .simple
            .record_verification_for_test();
        workflows.poll(&context, &mut activity);
        assert!(
            workflows.take_dat_identity_reload(),
            "a new verification reloads"
        );
        workflows.poll(&context, &mut activity);
        assert!(!workflows.take_dat_identity_reload(), "only once");
    }

    /// Launch identity for a Blue-selected loose ROM: the page's catalogue
    /// row supplies its platform, so the real evidence worker inspects it as
    /// SNES and launch planning gets a resolved identity. With no catalogue
    /// platform the worker cannot know the system and stays Unknown.
    #[test]
    fn the_catalogue_platform_lets_the_evidence_worker_resolve_launch_identity() {
        use archivefs_core::launch::CanonicalIdentityStatus;
        let fixture = VerifiedGame::new();
        let game = fixture.game().unwrap();
        let resolve = |row: &archivefs_core::PersistedArchive| {
            let context = eframe::egui::Context::default();
            let mut workflows =
                super::super::native_workflows::NativeWorkflows::new(context.clone());
            workflows.note_catalogue_row(row);
            workflows
                .app
                .start_selected_evidence_load(context.clone(), fixture.rom.clone());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                workflows.app.poll_selected_evidence();
                workflows
                    .app
                    .maybe_start_selected_evidence_enrichment(&context);
                if let crate::selected_evidence_page::SelectedEvidenceState::Ready {
                    report, ..
                } = &workflows.app.selected_evidence_ui.selected_evidence
                    && !matches!(
                        report.enrichment,
                        crate::selected_evidence_page::SelectedEvidenceEnrichmentStatus::Pending
                    )
                    && !matches!(
                        workflows
                            .app
                            .selected_evidence_ui
                            .selected_evidence_enrichment,
                        crate::SelectedEvidenceEnrichmentState::Loading { .. }
                    )
                {
                    return archivefs_core::launch::canonical_identity_from_game_report(
                        &report.game_identity_report,
                    )
                    .0;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "evidence never finished"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        match resolve(&game.archive) {
            CanonicalIdentityStatus::Resolved(identity) => {
                assert_eq!(identity.platform_id, "SNES");
                assert_eq!(identity.game_key.len(), 64, "exact-bytes SHA-256");
            }
            other => panic!("expected a resolved SNES identity, got {other:?}"),
        }
        let mut unassigned = game.archive.clone();
        unassigned.platform = None;
        assert!(matches!(
            resolve(&unassigned),
            CanonicalIdentityStatus::Unknown
        ));
    }

    /// Blue never loads live library records, so the launch planner takes
    /// a never-mounted row's content from the page's persisted row, by
    /// core's shared per-kind rule - never for a mount input, a missing row,
    /// a folder, another path or a file that has gone.
    #[test]
    fn a_persisted_loose_row_resolves_launch_content_by_cores_rule() {
        use archivefs_core::launch::{LaunchContainerKind, LaunchContentKind};
        let fixture = VerifiedGame::new();
        let game = fixture.game().unwrap();
        let content_for = |row: &archivefs_core::PersistedArchive| {
            let mut workflows = super::super::native_workflows::NativeWorkflows::new(
                eframe::egui::Context::default(),
            );
            workflows.note_catalogue_row(row);
            workflows
                .app
                .catalogued_loose_content_ref(&row.absolute_path)
        };
        let content = content_for(&game.archive).expect("the loose ROM is runnable content");
        assert_eq!(
            content.resolved_path.as_deref(),
            Some(fixture.rom.as_path())
        );
        assert_eq!(content.container, Some(LaunchContainerKind::PlainFile));
        assert!(!content.requires_mount);

        let row_at = |name: &str, kind: &str| {
            let path = fixture.roms.join(name);
            std::fs::write(&path, b"synthetic").unwrap();
            let mut row = game.archive.clone();
            row.absolute_path = path;
            row.archive_kind = kind.into();
            row
        };
        // Mega Drive and Virtual Boy: cartridge content, as core decides.
        for row in [
            row_at("Game.md", "megadrive_rom"),
            row_at("Game.vb", "direct_game_image"),
        ] {
            let content = content_for(&row).expect("cartridge content");
            assert_eq!(
                content.kind,
                Some(LaunchContentKind::Cartridge),
                "{:?}",
                row.absolute_path
            );
        }
        // A CUE sheet whose files do not pass the strict inspector stays
        // unresolved, with no plain-file fallback, exactly as core decides.
        let cue = content_for(&row_at("Disc.cue", "direct_game_image")).unwrap();
        assert_eq!(cue.resolved_path, None);
        assert_eq!(cue.container, None);
        assert_eq!(cue.kind, Some(LaunchContentKind::OpticalDisc));
        assert!(!cue.has_runnable_path());

        let mut zip = row_at("Game.zip", "zip");
        assert!(content_for(&zip).is_none(), "a mount input needs a mount");
        zip.archive_kind = "direct_game_image".into();
        zip.last_verified_missing_at = Some("now".into());
        assert!(content_for(&zip).is_none(), "a missing row is not runnable");
        let mut folder = game.archive.clone();
        folder.absolute_path = fixture.roms.clone();
        assert!(content_for(&folder).is_none(), "only a regular file");
        // Another path, or the file going away after the row was noted.
        let mut workflows =
            super::super::native_workflows::NativeWorkflows::new(eframe::egui::Context::default());
        workflows.note_catalogue_row(&game.archive);
        assert!(
            workflows
                .app
                .catalogued_loose_content_ref(&fixture.roms.join("Game.md"))
                .is_none()
        );
        fixture.remove_rom();
        assert!(
            workflows
                .app
                .catalogued_loose_content_ref(&fixture.rom)
                .is_none()
        );
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_symlink() {
                found.push(path);
            } else if path.is_dir() {
                found.extend(walk(&path));
            }
        }
        found
    }

    #[test]
    fn blue_games_list_shows_the_verified_game_at_both_window_sizes() {
        let fixture = VerifiedGame::new();
        for size in [[1280.0, 800.0], [700.0, 520.0]] {
            let context = eframe::egui::Context::default();
            let mut app = super::super::tests::fixture(&context);
            app.library = std::sync::Arc::new(fixture.library());
            app.indices = (0..app.library.games.len()).collect();
            app.router.current = super::super::Route::Section(super::super::Section::Games);
            // Real reachability: the card must be scrolled into its clip,
            // not painted outside it.
            let reach = super::super::tests::page_reachability::scroll_to_text;
            let shown = super::super::tests::page_reachability::visible_text;
            let output = reach(&context, &mut app, size, "Synthetic Hero (World)");
            assert!(shown(&output, "Synthetic Hero (World)"), "{size:?}");
            // Blue's list badge for a DAT-verified game not yet launched.
            let output = reach(
                &context,
                &mut app,
                size,
                "Identified · not yet checked for play",
            );
            assert!(shown(&output, "Identified"), "{size:?}");
        }
    }
}
