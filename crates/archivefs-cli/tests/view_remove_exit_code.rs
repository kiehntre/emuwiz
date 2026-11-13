//! `view remove` must exit non-zero when the view definition is kept because
//! something managed-looking was left unreconciled (review condition, PR #384):
//! a script must never read "removed" from exit 0 while the view still exists.
//! Synthetic files under a temporary HOME only.
use std::path::Path;
use std::process::{Command, Output};

use archivefs_core::{
    FrontendProfile, LibraryViewConfig, LibraryViewLayoutTemplate, load_library_view_configs_from,
    save_library_view_configs_to,
};

const VIEW_ID: &str = "view-remove-exit";

fn run_remove(home: &Path, extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"));
    command
        .args(["view", "remove", VIEW_ID])
        .args(extra)
        .env("HOME", home)
        .env("EMUWIZ_CONFIG_HOME", home.join("config"))
        .env("EMUWIZ_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"));
    command.output().unwrap()
}

/// One configured view whose pending intent record names a path at which a
/// user's regular file now sits - so removal must keep the definition.
fn setup(home: &Path, blocked: bool) -> std::path::PathBuf {
    let source = home.join("source");
    let destination = home.join("dest");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(destination.join("NES")).unwrap();
    std::fs::write(source.join("A.zip"), b"a").unwrap();
    let config_path = home.join("config").join("library_views.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    save_library_view_configs_to(
        &config_path,
        &[LibraryViewConfig {
            id: VIEW_ID.to_string(),
            name: "Exit Code View".to_string(),
            destination_root: destination.clone(),
            enabled: true,
            source_folders: Vec::new(),
            platforms: Vec::new(),
            layout_template: LibraryViewLayoutTemplate::PlatformFilename,
            profile: FrontendProfile::default(),
        }],
    )
    .unwrap();
    if blocked {
        std::fs::write(destination.join("NES/A.zip"), b"user file").unwrap();
        let data = home.join("data").join("library_views");
        std::fs::create_dir_all(&data).unwrap();
        let record = serde_json::json!({
            "view_id": VIEW_ID,
            "destination_root": destination.to_str().unwrap(),
            "process_id": 1,
            "recorded_at": "2026-11-06T00:00:00Z",
            "entries": [{
                "relative_link_path": "NES/A.zip",
                "target_path": source.join("A.zip").to_str().unwrap(),
                "action": "Create",
                "archive_identity": null,
                "platform": "NES",
                "source_folder_path": source.to_str().unwrap(),
            }],
        });
        std::fs::write(
            data.join(format!("{VIEW_ID}.intent.json")),
            serde_json::to_string_pretty(&record).unwrap(),
        )
        .unwrap();
    }
    config_path
}

#[test]
fn view_remove_exits_non_zero_when_the_definition_is_kept() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path();
    let config_path = setup(home, true);
    let output = run_remove(home, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "kept definition must not exit 0\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(stderr.contains("was kept"), "{stderr}");
    let views = load_library_view_configs_from(&config_path).unwrap();
    assert_eq!(views.len(), 1, "definition kept");
    assert_eq!(
        std::fs::read(home.join("dest/NES/A.zip")).unwrap(),
        b"user file"
    );
}

#[test]
fn view_remove_exits_zero_when_the_definition_is_removed_or_kept_on_request() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path();
    setup(home, true);
    // --keep-definition: keeping it is what was asked for, so not a failure.
    let output = run_remove(home, &["--keep-definition"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let base = tempfile::tempdir().unwrap();
    let home = base.path();
    let config_path_clean = setup(home, false);
    let output = run_remove(home, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        load_library_view_configs_from(&config_path_clean)
            .unwrap()
            .is_empty()
    );
}
