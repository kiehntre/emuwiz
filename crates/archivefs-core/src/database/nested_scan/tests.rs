//! Nested archive mounts inside a source. The `real_*` tests mount actual
//! ratarmount (FUSE) views of small tar files; they are skipped, with a
//! message, where `ratarmount` or `fusermount3` is unavailable.
use super::super::*;
use super::NestedMountState;
use std::process::{Child, Command, Stdio};

fn temp_dir(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "archivefs-nested-mount-{name}-{}-{}",
        std::process::id(),
        now_unix_seconds()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn config_for(source: &Path, mount_root: &Path) -> Config {
    Config {
        configured_source_roots: Some(vec![source.to_path_buf()]),
        source_folders: vec![source.to_path_buf()],
        mount_root: mount_root.to_path_buf(),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    }
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn on_path(program: &str) -> Option<PathBuf> {
    env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|dir| Path::new(dir).join(program))
        .find(|candidate| candidate.is_file())
}

fn fuse_available() -> bool {
    let ok = on_path("ratarmount").is_some()
        && (on_path("fusermount3").is_some() || on_path("fusermount").is_some())
        && Path::new("/dev/fuse").exists();
    if !ok {
        eprintln!("skipped: ratarmount/fusermount or /dev/fuse unavailable");
    }
    ok
}

/// A tar of `files` with fixed timestamps, so the same list always yields
/// the same archive content.
fn make_tar(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let staging = dir.join(format!("{name}.staging"));
    let _ = fs::remove_dir_all(&staging);
    for (path, bytes) in files {
        write(&staging.join(path), bytes);
    }
    let tar = dir.join(format!("{name}.tar"));
    let status = Command::new("tar")
        .arg("--mtime=@1700000000")
        .arg("--owner=0")
        .arg("--group=0")
        .arg("-cf")
        .arg(&tar)
        .arg("-C")
        .arg(&staging)
        .arg(".")
        .status()
        .unwrap();
    assert!(status.success());
    tar
}

fn mount_id(path: &Path) -> Option<u64> {
    crate::catalogue_health::directory_mount(path)
        .ok()
        .map(|mount| mount.mount_id)
}

struct Ratar {
    mount_point: PathBuf,
    child: Option<Child>,
}

impl Ratar {
    fn mount(tar: &Path, mount_point: &Path) -> Self {
        fs::create_dir_all(mount_point).unwrap();
        let parent = mount_id(mount_point.parent().unwrap());
        let child = Command::new("ratarmount")
            .arg("-f")
            .arg(tar)
            .arg(mount_point)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut fixture = Self {
            mount_point: mount_point.to_path_buf(),
            child: Some(child),
        };
        for _ in 0..300 {
            if mount_id(mount_point).is_some() && mount_id(mount_point) != parent {
                return fixture;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        fixture.unmount();
        panic!("ratarmount did not mount {}", mount_point.display());
    }

    fn unmount(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let program = if on_path("fusermount3").is_some() {
            "fusermount3"
        } else {
            "fusermount"
        };
        let _ = Command::new(program)
            .arg("-u")
            .arg(&self.mount_point)
            .stderr(Stdio::null())
            .status();
        let _ = child.wait();
    }
}

impl Drop for Ratar {
    fn drop(&mut self) {
        self.unmount();
    }
}

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    mount_point: PathBuf,
    config: Config,
    database: Database,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = temp_dir(name);
        let source = root.join("games");
        write(&source.join("top.zip"), b"top");
        write(&source.join("gone.zip"), b"gone");
        let mount_point = source.join("gba/ratarmount");
        fs::create_dir_all(&mount_point).unwrap();
        let config = config_for(&source, &root.join("virtual"));
        let database = Database::open_or_create(root.join("library.sqlite3")).unwrap();
        Self {
            root,
            source,
            mount_point,
            config,
            database,
        }
    }

    fn tar(&self, name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        make_tar(&self.root, name, files)
    }

    fn scan(&mut self) -> ScanPersistSummary {
        let summary = scan_and_persist(&mut self.database, &self.config, "nested-test").unwrap();
        assert!(
            summary.folder_errors.is_empty(),
            "{:?}",
            summary.folder_errors
        );
        summary
    }

    fn row(&self, relative: &str) -> Option<PersistedArchive> {
        let path = self.source.join(relative);
        self.database
            .load_archives()
            .unwrap()
            .into_iter()
            .find(|archive| archive.absolute_path == path)
    }

    fn missing(&self, relative: &str) -> bool {
        self.row(relative)
            .unwrap_or_else(|| panic!("{relative} has a catalogue row"))
            .last_verified_missing_at
            .is_some()
    }

    fn rows(&self) -> usize {
        self.database.load_archives().unwrap().len()
    }
}

fn mount_state(summary: &ScanPersistSummary) -> NestedMountState {
    assert_eq!(
        summary.nested_mounts.len(),
        1,
        "{:?}",
        summary.nested_mounts
    );
    summary.nested_mounts[0].state
}

const GAMES: &[(&str, &[u8])] = &[("Alpha.gba", b"alpha rom"), ("Beta.gba", b"beta rom")];

#[test]
fn real_archive_mount_is_indexed_reconciled_and_reaccepted_after_a_restart() {
    if !fuse_available() {
        return;
    }
    let mut fx = Fixture::new("restart");
    let tar = fx.tar("collection", GAMES);
    let mut mounted = Ratar::mount(&tar, &fx.mount_point);
    // A symlink to the mount is never followed into it.
    std::os::unix::fs::symlink(&fx.mount_point, fx.source.join("link")).unwrap();

    let first = fx.scan();
    assert_eq!(mount_state(&first), NestedMountState::New);
    assert_eq!(first.nested_mounts[0].archives, 2);
    assert_eq!(fx.rows(), 4, "top, gone and both games inside the mount");
    assert!(fx.row("gba/ratarmount/Alpha.gba").is_some());
    assert!(fx.row("link/Alpha.gba").is_none());

    fs::remove_file(fx.source.join("gone.zip")).unwrap();
    let second = fx.scan();
    assert_eq!(mount_state(&second), NestedMountState::Verified);
    assert!(
        fx.missing("gone.zip"),
        "the rest of the source reconciles again"
    );
    assert!(!fx.missing("top.zip"));
    assert!(!fx.missing("gba/ratarmount/Alpha.gba"));
    assert!(!fx.missing("gba/ratarmount/Beta.gba"));

    // A reboot: the same archive comes back as a new mount instance.
    mounted.unmount();
    let mut remounted = Ratar::mount(&tar, &fx.mount_point);
    let third = fx.scan();
    assert_eq!(mount_state(&third), NestedMountState::Restarted);
    assert!(!fx.missing("gba/ratarmount/Alpha.gba"));
    assert_eq!(fx.rows(), 4, "no duplicate identities after a restart");
    let fourth = fx.scan();
    assert_eq!(mount_state(&fourth), NestedMountState::Verified);
    remounted.unmount();
    let _ = fs::remove_dir_all(&fx.root);
}

#[test]
fn real_changed_archive_is_reread_and_reconciled_only_from_the_next_scan() {
    if !fuse_available() {
        return;
    }
    let mut fx = Fixture::new("changed");
    let old = fx.tar("old", GAMES);
    let new = fx.tar(
        "new",
        &[("Alpha.gba", b"alpha rom"), ("Gamma.gba", b"gamma rom")],
    );
    let mut mounted = Ratar::mount(&old, &fx.mount_point);
    fx.scan();
    assert_eq!(mount_state(&fx.scan()), NestedMountState::Verified);
    mounted.unmount();

    let mut replaced = Ratar::mount(&new, &fx.mount_point);
    let changed = fx.scan();
    assert_eq!(mount_state(&changed), NestedMountState::Changed);
    assert!(
        fx.row("gba/ratarmount/Gamma.gba").is_some(),
        "new game indexed"
    );
    assert!(
        !fx.missing("gba/ratarmount/Beta.gba"),
        "never marked missing on the scan that saw the change"
    );
    assert_eq!(mount_state(&fx.scan()), NestedMountState::Verified);
    assert!(
        fx.missing("gba/ratarmount/Beta.gba"),
        "reconciled once stable"
    );
    assert!(!fx.missing("gba/ratarmount/Alpha.gba"));
    replaced.unmount();
    let _ = fs::remove_dir_all(&fx.root);
}

#[test]
fn real_unmounted_archive_keeps_its_games_while_the_rest_reconciles() {
    if !fuse_available() {
        return;
    }
    let mut fx = Fixture::new("unmounted");
    let tar = fx.tar("collection", GAMES);
    let mut mounted = Ratar::mount(&tar, &fx.mount_point);
    fx.scan();
    fx.scan();
    mounted.unmount();

    fs::remove_file(fx.source.join("gone.zip")).unwrap();
    let summary = fx.scan();
    assert_eq!(mount_state(&summary), NestedMountState::Unavailable);
    assert!(!fx.missing("gba/ratarmount/Alpha.gba"));
    assert!(!fx.missing("gba/ratarmount/Beta.gba"));
    assert!(fx.missing("gone.zip"));
    let _ = fs::remove_dir_all(&fx.root);
}

#[test]
fn real_mount_replaced_or_removed_during_a_scan_saves_nothing_from_it() {
    if !fuse_available() {
        return;
    }
    let mut fx = Fixture::new("mid-scan");
    let tar = fx.tar("collection", GAMES);
    let other = fx.tar("other", &[("Substitute.gba", b"not the same")]);
    let mounted = Ratar::mount(&tar, &fx.mount_point);
    fx.scan();
    fx.scan();

    // Substituted after discovery read the original mount.
    let slot = std::rc::Rc::new(std::cell::RefCell::new(Some(mounted)));
    let replacement = std::rc::Rc::new(std::cell::RefCell::new(None));
    {
        let (slot, replacement) = (slot.clone(), replacement.clone());
        let (other, mount_point) = (other.clone(), fx.mount_point.clone());
        super::test_hook::after_discovery(move || {
            slot.borrow_mut().take().unwrap().unmount();
            *replacement.borrow_mut() = Some(Ratar::mount(&other, &mount_point));
        });
    }
    fs::remove_file(fx.source.join("gone.zip")).unwrap();
    let summary = fx.scan();
    assert_eq!(mount_state(&summary), NestedMountState::Failed);
    assert!(fx.row("gba/ratarmount/Substitute.gba").is_none());
    assert!(!fx.missing("gba/ratarmount/Alpha.gba"));
    assert!(fx.missing("gone.zip"), "the parent source still completed");

    // Removed after discovery: nothing from it saved, nothing called missing.
    replacement.borrow_mut().take().unwrap().unmount();
    let mut again = Ratar::mount(&tar, &fx.mount_point);
    fx.scan();
    super::test_hook::after_discovery({
        let mount_point = fx.mount_point.clone();
        move || {
            let _ = Command::new(if on_path("fusermount3").is_some() {
                "fusermount3"
            } else {
                "fusermount"
            })
            .arg("-u")
            .arg(&mount_point)
            .status();
        }
    });
    let summary = fx.scan();
    assert_eq!(mount_state(&summary), NestedMountState::Failed);
    assert!(!fx.missing("gba/ratarmount/Alpha.gba"));
    assert!(!fx.missing("gba/ratarmount/Beta.gba"));
    again.unmount();
    let _ = fs::remove_dir_all(&fx.root);
}

/// Runs everywhere: a nested filesystem of a type that is not scanned is
/// left alone, its existing entries are kept, and the rest of the source
/// still reconciles (the mount ID is simulated; the type is the real one).
#[test]
fn unsupported_nested_filesystem_is_left_alone_and_the_rest_still_reconciles() {
    let mut fx = Fixture::new("unsupported");
    write(&fx.mount_point.join("Inner.gba"), b"inner rom");
    fx.scan();
    assert_eq!(fx.rows(), 3);

    crate::catalogue_health::test_mounts::simulate(&fx.mount_point);
    fs::remove_file(fx.source.join("gone.zip")).unwrap();
    write(&fx.source.join("new.zip"), b"new");
    let summary = fx.scan();
    assert_eq!(mount_state(&summary), NestedMountState::Unsupported);
    assert_eq!(summary.counts.archives_added, 1);
    assert!(fx.missing("gone.zip"));
    assert!(!fx.missing("top.zip"));
    assert!(
        !fx.missing("gba/ratarmount/Inner.gba"),
        "kept, not called missing"
    );
    let status = fx.database.list_source_folders().unwrap()[0].clone();
    assert_eq!(status.last_scan_status, Some(SourceScanStatus::Success));
    let _ = fs::remove_dir_all(&fx.root);
}
