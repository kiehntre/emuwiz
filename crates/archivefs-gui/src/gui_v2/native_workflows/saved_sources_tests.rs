//! Blue's Sources page without the full library snapshot: the light saved-state
//! load must carry the backend's catalogue health, or the page reads "Checking"
//! forever.
use super::*;
use archivefs_core::catalogue_health::SourceHealthState;
use archivefs_core::{Config, Database, SourceFolderConfig, scan_and_persist};

#[test]
fn saved_sources_carry_catalogue_health_for_each_configured_source() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("game.gba"), b"fixture bytes").unwrap();
    let mut database = Database::open_or_create(temp.path().join("library.sqlite3")).unwrap();
    let config = Config {
        configured_source_roots: Some(vec![root.clone()]),
        source_folders: vec![root.clone()],
        mount_root: temp.path().join("mounts"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    scan_and_persist(&mut database, &config, "fixture").unwrap();
    let sources = [SourceFolderConfig {
        path: root.clone(),
        enabled: true,
        created_at: None,
    }];

    let saved = saved_sources_from(&database, &sources).expect("saved sources");

    assert_eq!(saved.views.len(), 1);
    let rows = crate::catalogue_health_ui::project_rows(&saved.views, &saved.health);
    let health = rows[0]
        .health
        .as_ref()
        .expect("health answered for the source");
    assert_eq!(health.state, SourceHealthState::Healthy);
    let overall = crate::catalogue_health_ui::Overall::from_rows(&rows);
    assert_ne!(overall.headline().0, "Checking");

    let mut history = SavedSourceHistory::default();
    assert!(history.health().is_none());
    history = SavedSourceHistory::Loaded(saved, std::time::Instant::now());
    assert_eq!(history.health().map(<[_]>::len), Some(1));
}
