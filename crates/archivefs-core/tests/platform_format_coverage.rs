//! End-to-end platform × format regression: every extension the registry
//! says the Library scan stores must survive Add Source → Scan → Persist →
//! Rescan → Reopen, and every extension it says is not stored must be
//! reported as skipped rather than vanish silently. Synthetic bytes only;
//! never scans a configured real source.
archivefs_core::install_test_environment!();

use archivefs_core::format_coverage_matrix::{
    DEFERRED_STRONG_EXTENSIONS, ExtensionClaim, LibraryRoute, static_matrix,
    stored_platform_from_extension_alone,
};
use archivefs_core::platform::PLATFORMS;
use archivefs_core::{Config, Database, SkipReason, scan_and_persist};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn junk(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u8).wrapping_mul(0x9D) ^ 0x3C)
        .collect()
}

#[test]
fn every_stored_extension_survives_scan_rescan_and_reopen_and_every_other_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("collection");
    let neutral = source.join("mixed");
    std::fs::create_dir_all(&neutral).unwrap();
    let matrix = static_matrix();

    // One loose file per distinct extension in a folder naming no platform.
    let mut expected_rows: BTreeMap<PathBuf, Option<&'static str>> = BTreeMap::new();
    let mut expected_skips: BTreeSet<PathBuf> = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for row in &matrix.rows {
        if !seen.insert(row.extension) {
            continue;
        }
        let path = neutral.join(format!("sample-{0}.{0}", row.extension));
        std::fs::write(&path, junk(4096)).unwrap();
        match row.library_route {
            LibraryRoute::RegisteredMedia => {
                let platform = stored_platform_from_extension_alone(row.extension);
                expected_rows.insert(path, platform);
            }
            // Junk bytes never pass a structural gate; shared Mega Drive
            // candidates need corroboration; deferred claims are not stored.
            LibraryRoute::StructuralGate
            | LibraryRoute::Corroborated
            | LibraryRoute::NotPersisted => {
                expected_skips.insert(path);
            }
        }
    }

    // One file per platform inside a folder that names the platform: the
    // folder decides the stored platform for every stored route.
    for platform in PLATFORMS {
        let Some(alias) = platform.folder_aliases.first() else {
            continue;
        };
        let extension = platform
            .strong_extensions
            .iter()
            .chain(platform.weak_extensions.iter())
            .find(|extension| {
                matrix.rows.iter().any(|row| {
                    row.platform_id == platform.id
                        && row.extension == **extension
                        && row.library_route == LibraryRoute::RegisteredMedia
                })
            });
        let Some(extension) = extension else {
            continue;
        };
        let folder = source.join("by-folder").join(alias);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("game.{extension}"));
        std::fs::write(&path, junk(4096)).unwrap();
        expected_rows.insert(path, Some(platform.id));
    }

    let db_path = dir.path().join("library.sqlite3");
    let config = Config {
        source_folders: vec![source.clone()],
        mount_root: dir.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };

    let mut database = Database::open_or_create(&db_path).unwrap();
    let first = scan_and_persist(&mut database, &config, "platform-format-coverage").unwrap();
    assert!(
        first.folder_errors.is_empty(),
        "scan errors: {:?}",
        first.folder_errors
    );
    eprintln!("scummvm scan report: {:?}", first.scummvm);
    let reported_skips: BTreeMap<PathBuf, &'static str> = first
        .skipped_files
        .iter()
        .map(|skipped| (skipped.path.clone(), skipped.reason.label()))
        .collect();
    for path in &expected_skips {
        assert!(
            reported_skips.contains_key(path),
            "{} was neither stored nor reported as skipped - a silent loss",
            path.display()
        );
    }
    for path in expected_rows.keys() {
        assert!(
            !reported_skips.contains_key(path),
            "{} is a stored extension but the scan skipped it ({})",
            path.display(),
            reported_skips[path]
        );
    }
    // Deferred strong claims are visible as skipped, with the reason the
    // scanner gives for an unregistered extension.
    for entry in DEFERRED_STRONG_EXTENSIONS {
        let path = neutral.join(format!("sample-{0}.{0}", entry.extension));
        assert_eq!(
            reported_skips.get(&path),
            Some(&SkipReason::UnsupportedExtension.label()),
            ".{} is deferred, so the scan must say it is unsupported",
            entry.extension
        );
    }

    let check_rows = |database: &Database, pass: &str| {
        let rows = database.load_archives().unwrap();
        let by_path: BTreeMap<PathBuf, _> = rows
            .into_iter()
            .map(|row| (row.absolute_path.clone(), row))
            .collect();
        let missing: Vec<String> = expected_rows
            .keys()
            .filter(|path| !by_path.contains_key(*path))
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        let unexpected: Vec<String> = by_path
            .iter()
            .filter(|(path, _)| !expected_rows.contains_key(*path))
            .map(|(path, row)| {
                format!(
                    "{} [{} '{}' {:?}]",
                    path.display(),
                    row.archive_kind,
                    row.display_name,
                    row.platform_source
                )
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{pass}: stored extensions with no Library row (silent loss): {missing:?}; \
             rows nobody expected: {unexpected:?}"
        );
        for (path, platform) in &expected_rows {
            let row = &by_path[path];
            assert_eq!(
                row.platform.as_deref(),
                *platform,
                "{pass}: {} stored with the wrong platform",
                path.display()
            );
            assert!(row.last_verified_missing_at.is_none());
        }
        for path in &expected_skips {
            assert!(
                !by_path.contains_key(path),
                "{pass}: {} must not have a Library row",
                path.display()
            );
        }
        assert_eq!(
            by_path.len(),
            expected_rows.len(),
            "{pass}: unexpected extra rows: {:?}",
            by_path
                .keys()
                .filter(|path| !expected_rows.contains_key(*path))
                .collect::<Vec<_>>()
        );
    };
    check_rows(&database, "first scan");

    // Rescan: fingerprints are reused, nothing disappears, nothing doubles.
    scan_and_persist(&mut database, &config, "platform-format-coverage-rescan").unwrap();
    check_rows(&database, "rescan");
    drop(database);

    // Reopen: what was stored is what comes back after a restart.
    let reopened = Database::open_or_create(&db_path).unwrap();
    check_rows(&reopened, "reopen");
}

#[test]
fn no_strong_claim_is_stored_with_a_platform_the_registry_did_not_name() {
    // A stored strong extension in a neutral folder either names its single
    // claimant or stays unassigned; it is never assigned to a platform that
    // does not claim the extension.
    for row in static_matrix()
        .rows
        .iter()
        .filter(|row| row.claim == ExtensionClaim::Strong)
    {
        if let Some(platform) = row.stored_platform {
            let claimants: Vec<&str> =
                archivefs_core::platform::platform_candidates_for_extension(row.extension);
            assert!(
                claimants.contains(&platform),
                ".{} alone resolved to {platform}, which does not claim it",
                row.extension
            );
        }
    }
}
