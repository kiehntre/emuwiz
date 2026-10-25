//! Blue Library truth after a scan that could not finish reading a game's
//! file. The row is produced by the same production calls Blue uses to add
//! and scan a source; the saved identity report is kept as history, its
//! "Verified" standing is withdrawn while the row is marked for retry, and a
//! completed rescan restores it. Nothing is written to any real library.

use super::App;
use super::library::Library;
use archivefs_core::{ArchiveHealth, Database, PersistedArchive, synthetic_media};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Profile {
    root: PathBuf,
    config_path: PathBuf,
    database_path: PathBuf,
    games_dir: PathBuf,
    rom_path: PathBuf,
}

impl Profile {
    /// One disposable source holding a synthetic PS2 image: an extension the
    /// scanner re-inspects on every pass, a platform whose identity report the
    /// scan itself persists, and a detector-owned platform assignment.
    fn build() -> Self {
        let root = std::env::temp_dir().join(format!(
            "emuwiz-blue-stale-inspection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let games_dir = root.join("games");
        let config_dir = root.join("config");
        let data_dir = root.join("data");
        for dir in [&games_dir, &config_dir, &data_dir, &root.join("mount")] {
            fs::create_dir_all(dir).unwrap();
        }
        let rom_path = games_dir.join("EmuWiz Synthetic Disc (USA).iso");
        fs::write(&rom_path, synthetic_media::ps2_iso()).unwrap();
        let config_path = config_dir.join("config.toml");
        fs::write(
            &config_path,
            format!(
                "source_folders = []\nmount_root = \"{}\"\nratarmount_bin = \"ratarmount\"\n",
                root.join("mount").display()
            ),
        )
        .unwrap();
        let database_path = data_dir.join("library.sqlite3");
        archivefs_core::add_source_folder_at(&config_path, &database_path, &games_dir).unwrap();
        Self {
            root,
            config_path,
            database_path,
            games_dir,
            rom_path,
        }
    }

    fn scan(&self, reason: &str) {
        archivefs_core::scan_source_folder_at(
            &self.config_path,
            &self.database_path,
            &self.games_dir,
            reason,
        )
        .unwrap();
    }

    fn row(&self) -> PersistedArchive {
        Database::open_read_only(&self.database_path)
            .unwrap()
            .load_archives()
            .unwrap()
            .into_iter()
            .find(|row| row.absolute_path == self.rom_path)
            .expect("the scanned image is catalogued")
    }

    fn library(&self) -> Library {
        super::backend::load_library_with_catalogues(&self.database_path, &Default::default())
            .unwrap()
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.rom_path, fs::Permissions::from_mode(0o644));
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Every text galley painted on the Blue Library page at a normal window
/// size, after a warm-up frame.
fn painted_library(library: Library) -> Vec<String> {
    let context = eframe::egui::Context::default();
    let mut app: App = super::tests::fixture(&context);
    app.simple.enabled = true;
    app.library = Arc::new(library);
    app.router.current = super::Route::BrowsePlay;
    let mut strings = Vec::new();
    for _ in 0..2 {
        let output = context.run(
            eframe::egui::RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(
                    eframe::egui::Pos2::ZERO,
                    eframe::egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            },
            |ctx| app.show(ctx),
        );
        fn collect(shape: &eframe::egui::Shape, out: &mut Vec<String>) {
            match shape {
                eframe::egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                eframe::egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        strings.clear();
        output
            .shapes
            .iter()
            .for_each(|s| collect(&s.shape, &mut strings));
    }
    strings
}

fn has(strings: &[String], needle: &str) -> bool {
    strings.iter().any(|value| value.contains(needle))
}

fn make_unreadable(path: &Path) -> bool {
    fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(path).is_ok() {
        // root can read anything; the refusal cannot be staged.
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        return false;
    }
    true
}

#[test]
fn an_incomplete_inspection_withdraws_verified_in_the_blue_library_until_a_rescan_completes() {
    let profile = Profile::build();

    // 1. A completed scan: detector-owned platform, saved identity report,
    //    shown as Verified with no click.
    profile.scan("stale-inspection-first");
    let verified = profile.row();
    assert_eq!(
        verified.platform_source.as_deref(),
        Some("registry_detector"),
        "{verified:?}"
    );
    let saved_report = verified
        .identity_report
        .clone()
        .expect("the scan persists the PS2 identity report");
    let library = profile.library();
    assert_eq!(library.games.len(), 1);
    let game = &library.games[0];
    assert!(game.identified, "{game:?}");
    assert_eq!(game.identity_summary(), "Verified");
    let strings = painted_library(library);
    assert!(has(&strings, "Verified"), "{strings:?}");

    // 2. A scan that cannot read the file: size and mtime are unchanged, the
    //    saved report is kept, but it no longer counts as current.
    if !make_unreadable(&profile.rom_path) {
        return;
    }
    profile.scan("stale-inspection-fault");
    fs::set_permissions(&profile.rom_path, fs::Permissions::from_mode(0o644)).unwrap();
    let stale = profile.row();
    assert_eq!(
        stale.last_known_health,
        ArchiveHealth::RetryAvailable.to_string()
    );
    assert_eq!(
        stale.platform, verified.platform,
        "the assignment is history, not retracted"
    );
    assert_eq!(stale.platform_source, verified.platform_source);
    assert_eq!(stale.size_bytes, verified.size_bytes);
    assert_eq!(
        stale.modified_time_unix_seconds,
        verified.modified_time_unix_seconds
    );
    assert_eq!(
        stale.identity_report.as_ref(),
        Some(&saved_report),
        "the saved report is retained, not overwritten by the failed pass"
    );
    let library = profile.library();
    let game = &library.games[0];
    assert!(
        !game.identified,
        "a stale saved report is not a current identification"
    );
    assert!(game.inspection_incomplete);
    assert_ne!(game.identity_summary(), "Verified");
    assert!(
        game.identity_summary().contains("Rescan"),
        "{}",
        game.identity_summary()
    );
    assert!(game.status().contains("Rescan"), "{}", game.status());
    let strings = painted_library(library);
    assert!(!has(&strings, "Verified"), "{strings:?}");
    assert!(has(&strings, "Rescan"), "{strings:?}");

    // 3. The retry completes: the same file, read in full, is Verified again
    //    from the retained report.
    profile.scan("stale-inspection-retry");
    let restored = profile.row();
    assert_ne!(
        restored.last_known_health,
        ArchiveHealth::RetryAvailable.to_string()
    );
    assert_eq!(restored.identity_report.as_ref(), Some(&saved_report));
    let library = profile.library();
    let game = &library.games[0];
    assert!(game.identified);
    assert_eq!(game.identity_summary(), "Verified");
    let strings = painted_library(library);
    assert!(has(&strings, "Verified"), "{strings:?}");
    assert!(!has(&strings, "Rescan"), "{strings:?}");
}
