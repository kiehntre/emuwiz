//! Valid synthetic media through the real pipeline: a file whose bytes prove
//! its format must be stored with that platform, whatever folder it sits in,
//! and discovery must accept it with the same platform. Junk-byte gate checks
//! live in `platform_format_coverage.rs`; this file is about content.
archivefs_core::install_test_environment!();

use archivefs_core::ingestion::{ValidationState, discover_source};
use archivefs_core::synthetic_media::samples;
use archivefs_core::{Config, Database, scan_and_persist};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Formats whose bytes identify a single platform. Shared containers
/// (`.tap`, `.tzx`) are deliberately absent: they prove a family of
/// machines, and the honest outcome for them is "needs confirmation".
fn proves_platform(extension: &str, structure: &str) -> bool {
    !matches!(extension, "tap" | "tzx") && !structure.contains("dual-mode")
}

#[test]
fn valid_media_in_a_neutral_folder_is_stored_with_the_platform_its_bytes_prove() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("collection");
    let neutral = source.join("unsorted");
    std::fs::create_dir_all(&neutral).unwrap();
    let mut expected: BTreeMap<PathBuf, (&'static str, bool)> = BTreeMap::new();
    for (index, sample) in samples().into_iter().enumerate() {
        let path = neutral.join(format!(
            "sample-{index:02}-{}.{}",
            sample.platform_id.replace(' ', "_"),
            sample.extension
        ));
        std::fs::write(&path, &sample.bytes).unwrap();
        expected.insert(
            path,
            (
                sample.platform_id,
                proves_platform(sample.extension, sample.structure),
            ),
        );
    }

    let report = discover_source(&neutral).unwrap();
    let mut discovery_failures = Vec::new();
    for item in &report.items {
        let (platform, proves) = expected[&item.path];
        if proves && item.validation_state != ValidationState::Accepted {
            discovery_failures.push(format!(
                "{}: discovery did not accept valid bytes: {:?} / {}",
                item.path.file_name().unwrap().to_string_lossy(),
                item.skip_reason,
                item.explanation
            ));
        }
        if proves && item.platform_hint.as_deref() != Some(platform) {
            discovery_failures.push(format!(
                "{}: discovery platform hint {:?}, bytes prove {platform}",
                item.path.file_name().unwrap().to_string_lossy(),
                item.platform_hint
            ));
        }
    }

    let db_path = dir.path().join("library.sqlite3");
    let config = Config {
        source_folders: vec![source.clone()],
        mount_root: dir.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    let mut database = Database::open_or_create(&db_path).unwrap();
    let summary = scan_and_persist(&mut database, &config, "platform-format-content").unwrap();
    assert!(
        summary.folder_errors.is_empty(),
        "{:?}",
        summary.folder_errors
    );
    let rows: BTreeMap<PathBuf, _> = database
        .load_archives()
        .unwrap()
        .into_iter()
        .map(|row| (row.absolute_path.clone(), row))
        .collect();
    let mut store_failures = Vec::new();
    for (path, (platform, proves)) in &expected {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(row) = rows.get(path) else {
            store_failures.push(format!("{name}: no Library row"));
            continue;
        };
        if *proves && row.platform.as_deref() != Some(*platform) {
            store_failures.push(format!(
                "{name}: stored platform {:?} ({:?}), bytes prove {platform}",
                row.platform, row.platform_source
            ));
        }
    }
    assert!(
        discovery_failures.is_empty() && store_failures.is_empty(),
        "discovery:\n  {}\nstore:\n  {}",
        discovery_failures.join("\n  "),
        store_failures.join("\n  ")
    );
}
