//! Real temp-folder fixtures; never a real retroarch.cfg or save folder.

use super::*;
use crate::emulator_environment::HostReadOnlyFilesystem;
use std::fs;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("ra")).unwrap();
        Fixture { _dir: dir, root }
    }
    fn cfg(&self) -> PathBuf {
        self.root.join("ra/retroarch.cfg")
    }
    fn discover(&self, global: &[(&str, String)], cores: &[&str]) -> Vec<SaveDirectoryFinding> {
        discover_save_directories(
            &HostReadOnlyFilesystem,
            &self.cfg(),
            |key| {
                global
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.as_str())
            },
            &self.root,
            cores.iter().copied(),
        )
    }
}

fn status_of(
    findings: &[SaveDirectoryFinding],
    key: &str,
    core: Option<&str>,
) -> SaveDirectoryStatus {
    findings
        .iter()
        .find(|f| f.config_key == key && f.core_name.as_deref() == core)
        .unwrap_or_else(|| panic!("no finding for {key} {core:?}: {findings:?}"))
        .status
}

#[test]
fn classifies_global_folders_without_creating_anything() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("saves")).unwrap();
    fs::write(f.root.join("states"), b"file").unwrap();
    let found = f.discover(
        &[
            (
                "savefile_directory",
                f.root.join("saves").display().to_string(),
            ),
            (
                "savestate_directory",
                f.root.join("states").display().to_string(),
            ),
        ],
        &[],
    );
    assert_eq!(
        status_of(&found, "savefile_directory", None),
        SaveDirectoryStatus::Usable
    );
    assert_eq!(
        status_of(&found, "savestate_directory", None),
        SaveDirectoryStatus::NotADirectory
    );

    let missing = f.root.join("nope");
    let found = f.discover(
        &[("savefile_directory", missing.display().to_string())],
        &[],
    );
    assert_eq!(
        status_of(&found, "savefile_directory", None),
        SaveDirectoryStatus::Missing
    );
    assert!(!missing.exists(), "a missing folder is never created");
    assert_eq!(
        found[0].directory.as_ref().unwrap().display,
        missing.display().to_string()
    );
}

#[test]
fn tilde_symlink_and_unresolvable_values() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("real")).unwrap();
    std::os::unix::fs::symlink(f.root.join("real"), f.root.join("link")).unwrap();
    let found = f.discover(
        &[
            ("savefile_directory", "~/link".into()),
            ("savestate_directory", ":/relative".into()),
        ],
        &[],
    );
    assert_eq!(
        status_of(&found, "savefile_directory", None),
        SaveDirectoryStatus::Symlink
    );
    assert_eq!(
        status_of(&found, "savestate_directory", None),
        SaveDirectoryStatus::Uncheckable
    );
    // Empty or absent values are RetroArch's own default: nothing reported.
    assert!(
        f.discover(&[("savefile_directory", String::new())], &[])
            .is_empty()
    );
}

#[test]
fn unwritable_folder_is_reported() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return; // root can write anywhere; the check is meaningless.
    }
    let f = Fixture::new();
    let locked = f.root.join("locked");
    fs::create_dir_all(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let found = f.discover(&[("savefile_directory", locked.display().to_string())], &[]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        status_of(&found, "savefile_directory", None),
        SaveDirectoryStatus::NotWritable
    );
}

#[test]
fn per_core_override_is_read_from_config_or_rgui_directory() {
    let f = Fixture::new();
    let core_dir = f.root.join("ra/config/Snes9x");
    fs::create_dir_all(&core_dir).unwrap();
    fs::write(
        core_dir.join("Snes9x.cfg"),
        format!(
            "savestate_directory = \"{}\"\n",
            f.root.join("gone-states").display()
        ),
    )
    .unwrap();
    let found = f.discover(&[], &["Snes9x", "mGBA", "../escape"]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        status_of(&found, "savestate_directory", Some("Snes9x")),
        SaveDirectoryStatus::Missing
    );
    assert!(
        found[0]
            .settings_file
            .display
            .ends_with("config/Snes9x/Snes9x.cfg")
    );

    // A custom rgui_config_directory moves where overrides are looked up.
    let custom = f.root.join("overrides/mGBA");
    fs::create_dir_all(&custom).unwrap();
    fs::create_dir_all(f.root.join("ok")).unwrap();
    fs::write(
        custom.join("mGBA.cfg"),
        format!("savefile_directory = \"{}\"\n", f.root.join("ok").display()),
    )
    .unwrap();
    let found = f.discover(
        &[(
            "rgui_config_directory",
            f.root.join("overrides").display().to_string(),
        )],
        &["Snes9x", "mGBA"],
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(
        status_of(&found, "savefile_directory", Some("mGBA")),
        SaveDirectoryStatus::Usable
    );
}

#[test]
fn unreadable_override_is_uncheckable_not_ignored() {
    let f = Fixture::new();
    let core_dir = f.root.join("ra/config/Snes9x");
    fs::create_dir_all(&core_dir).unwrap();
    fs::write(f.root.join("elsewhere.cfg"), "").unwrap();
    std::os::unix::fs::symlink(f.root.join("elsewhere.cfg"), core_dir.join("Snes9x.cfg")).unwrap();
    let found = f.discover(&[], &["Snes9x"]);
    assert_eq!(found.len(), 2);
    assert!(
        found
            .iter()
            .all(|x| x.status == SaveDirectoryStatus::Uncheckable && x.directory.is_none())
    );
}
