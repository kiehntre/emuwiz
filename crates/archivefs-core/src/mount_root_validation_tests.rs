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
