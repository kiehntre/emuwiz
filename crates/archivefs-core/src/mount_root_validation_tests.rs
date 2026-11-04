use super::*;

#[test]
fn mount_root_lexical_validation_refuses_root_and_unsafe_spelling_without_io() {
    // Pure lexical validation: never probe the real filesystem root.
    for path in [
        "/",
        "//",
        "/./",
        "/tmp/./mounts",
        "/tmp/../mounts",
        "/tmp/mount\nroot",
    ] {
        assert!(
            validate_mount_root_path(Path::new(path)).is_err(),
            "{path:?}"
        );
    }
    assert!(validate_mount_root_path(Path::new("/tmp/ordinary mounts")).is_ok());
}

#[cfg(unix)]
#[test]
fn mount_root_lexical_validation_refuses_non_unicode_without_io() {
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/mount-\xff".to_vec()));
    assert!(validate_mount_root_path(&path).is_err());
}

#[test]
fn mount_root_raw_dot_selection_preserves_the_existing_config() {
    let fixture = tempfile::tempdir().unwrap();
    fs::create_dir(fixture.path().join("mounts")).unwrap();
    let config = fixture.path().join("config.toml");
    let original = b"source_folders = []\nmount_root = \"/old\"\n";
    fs::write(&config, original).unwrap();
    let selected = PathBuf::from(format!("{}/./mounts", fixture.path().display()));
    assert!(set_mount_root_to(&config, &selected).is_err());
    assert_eq!(fs::read(config).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn mount_root_symlink_ancestor_is_rejected_before_the_writable_probe() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = tempfile::tempdir().unwrap();
    let real = fixture.path().join("real");
    fs::create_dir(&real).unwrap();
    let target = real.join("mounts");
    fs::create_dir(&target).unwrap();
    let linked = fixture.path().join("linked");
    symlink(&real, &linked).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o555)).unwrap();
    let config = fixture.path().join("config.toml");
    let original = b"source_folders = []\nmount_root = \"/old\"\n";
    fs::write(&config, original).unwrap();
    let error = set_mount_root_to(&config, &linked.join("mounts")).unwrap_err();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(error.to_string().contains("not a safe location"), "{error}");
    assert_eq!(fs::read(config).unwrap(), original);
}

// --- Mount root / source folder disjointness (Night Watch G1-01, D4-06) ---

fn legacy_config(path: &Path, sources: &[&Path], mount_root: &Path) -> Vec<u8> {
    let sources = sources
        .iter()
        .map(|source| format!("\"{}\"", source.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let contents = format!(
        "source_folders = [{sources}]\nmount_root = \"{}\"\nratarmount_bin = \"ratarmount\"\n",
        mount_root.display()
    );
    fs::write(path, &contents).unwrap();
    contents.into_bytes()
}

#[test]
fn set_mount_root_refuses_a_folder_overlapping_a_source() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let source = root.join("library");
    fs::create_dir_all(source.join("mnt")).unwrap();
    let config = root.join("config.toml");
    let original = legacy_config(&config, &[&source], &root.join("old-mounts"));
    // Equal to, inside, and containing the source.
    for candidate in [source.clone(), source.join("mnt"), root.to_path_buf()] {
        let error = set_mount_root_to(&config, &candidate).unwrap_err();
        assert!(
            error.to_string().contains("source folder"),
            "{}: {error}",
            candidate.display()
        );
        assert_eq!(fs::read(&config).unwrap(), original);
    }
    // A disjoint sibling is still accepted.
    let disjoint = root.join("mounts");
    fs::create_dir(&disjoint).unwrap();
    assert_eq!(set_mount_root_to(&config, &disjoint).unwrap(), disjoint);
    assert_eq!(Config::load_from(&config).unwrap().mount_root, disjoint);
}

#[cfg(unix)]
#[test]
fn set_mount_root_refuses_the_real_folder_behind_a_symlinked_source() {
    let fixture = tempfile::tempdir().unwrap();
    let real = fixture.path().join("real-library");
    fs::create_dir_all(real.join("sub")).unwrap();
    let linked = fixture.path().join("library-link");
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let config = fixture.path().join("config.toml");
    let original = legacy_config(&config, &[&linked], &fixture.path().join("old"));
    for candidate in [real.clone(), real.join("sub")] {
        assert!(
            set_mount_root_to(&config, &candidate).is_err(),
            "{}",
            candidate.display()
        );
        assert_eq!(fs::read(&config).unwrap(), original);
    }
}

#[test]
fn add_source_refuses_a_folder_overlapping_the_mount_root() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let mount_root = root.join("mnt");
    fs::create_dir_all(mount_root.join("Unknown/Good")).unwrap();
    let existing = root.join("existing");
    fs::create_dir(&existing).unwrap();
    let config = root.join("config.toml");
    let database = root.join("library.sqlite3");
    let original = legacy_config(&config, &[&existing], &mount_root);
    // Equal to, inside, and containing the mount root.
    for candidate in [
        mount_root.clone(),
        mount_root.join("Unknown/Good"),
        root.to_path_buf(),
    ] {
        let error = add_source_folder_at(&config, &database, &candidate).unwrap_err();
        assert!(
            error.to_string().contains("preparation folder"),
            "{}: {error}",
            candidate.display()
        );
        assert_eq!(fs::read(&config).unwrap(), original);
    }
    assert!(!database.exists(), "a refused add never opens the database");
    // A disjoint folder is still added.
    let games = root.join("games");
    fs::create_dir(&games).unwrap();
    add_source_folder_at(&config, &database, &games).unwrap();
    assert!(
        Config::load_from(&config)
            .unwrap()
            .source_folders
            .contains(&games)
    );
}

#[test]
fn add_source_refuses_a_parent_of_a_not_yet_created_mount_root() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let candidate = root.join("library");
    fs::create_dir(&candidate).unwrap();
    let config = root.join("config.toml");
    let original = legacy_config(&config, &[], &candidate.join("mounts/not-yet"));
    assert!(add_source_folder_at(&config, &root.join("db.sqlite3"), &candidate).is_err());
    assert_eq!(fs::read(&config).unwrap(), original);
}

#[test]
fn master_rom_root_refuses_a_folder_overlapping_the_mount_root() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let mount_root = root.join("mnt");
    fs::create_dir(&mount_root).unwrap();
    let config = root.join("config.toml");
    let original = legacy_config(&config, &[], &mount_root);
    for candidate in [
        mount_root.clone(),
        mount_root.join("roms"),
        root.to_path_buf(),
    ] {
        assert!(
            set_master_rom_root_to(&config, &candidate).is_err(),
            "{}",
            candidate.display()
        );
        assert_eq!(fs::read(&config).unwrap(), original);
    }
    set_master_rom_root_to(&config, &root.join("organised")).unwrap();
}

#[test]
fn config_check_reports_a_mount_root_inside_a_source_and_creates_nothing() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("library");
    fs::create_dir(&source).unwrap();
    let mount_root = source.join("mnt");
    let config = fixture.path().join("config.toml");
    legacy_config(&config, &[&source], &mount_root);
    let report = run_config_check(&config);
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.status == ConfigCheckStatus::Error
                && check.detail.contains("source folder")
                && check.detail.contains(&source.display().to_string())),
        "{:#?}",
        report.checks
    );
    assert!(
        !mount_root.exists(),
        "config-check created the mount root inside a source"
    );
}

#[test]
fn config_check_still_passes_a_disjoint_mount_root() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("library");
    fs::create_dir(&source).unwrap();
    let mount_root = fixture.path().join("mounts");
    let config = fixture.path().join("config.toml");
    legacy_config(&config, &[&source], &mount_root);
    let report = run_config_check(&config);
    assert!(
        !report
            .checks
            .iter()
            .any(|check| check.detail.contains("overlaps") || check.name.contains("separate")),
        "{:#?}",
        report.checks
    );
    assert!(
        mount_root.is_dir(),
        "a disjoint mount root is still created"
    );
}

#[test]
fn scanner_never_treats_a_mount_root_inside_a_source_as_library_content() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("library");
    fs::create_dir_all(source.join("roms")).unwrap();
    fs::write(source.join("roms/Outer.zip"), b"archive").unwrap();
    let mount_root = source.join("mnt");
    // What a live mount of Outer.zip would expose: a member that is itself
    // a supported archive.
    fs::create_dir_all(mount_root.join("Unknown/Outer")).unwrap();
    fs::write(mount_root.join("Unknown/Outer/Inner.zip"), b"archive").unwrap();
    let config = Config {
        source_folders: vec![source.clone()],
        mount_root: mount_root.clone(),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    };
    let archives = ArchiveScanner::new(&config).scan_archives().unwrap();
    let paths: Vec<_> = archives
        .iter()
        .map(|archive| archive.path.clone())
        .collect();
    assert_eq!(paths, vec![source.join("roms/Outer.zip")], "{paths:?}");
}

#[test]
fn targeted_mount_cleanup_refuses_a_mount_root_containing_a_source() {
    let fixture = tempfile::tempdir().unwrap();
    let mount_root = fixture.path().join("mnt");
    let empty_in_source = mount_root.join("roms/Empty");
    fs::create_dir_all(&empty_in_source).unwrap();
    let config = Config {
        source_folders: vec![mount_root.join("roms")],
        mount_root: mount_root.clone(),
        ratarmount_bin: "ratarmount".to_string(),
        master_rom_root: None,
    };
    assert!(cleanup_selected_mount_tree(&config, &empty_in_source).is_err());
    assert!(cleanup_selected_mount_dir(&config, &empty_in_source).is_err());
    assert!(empty_in_source.is_dir());
}
