//! Process-level isolation for test binaries.
//!
//! Application code finds the user's configuration, application data, caches
//! and emulator installs through `$HOME` and the XDG variables (about a
//! hundred call sites). A test that exercises any of them would otherwise read
//! or write the real user's files. This module points all of them at a private
//! directory tree **once, before `main`**, so no test thread exists yet and
//! there is nothing to race. Tests then keep using the normal production path
//! helpers; the helpers simply resolve inside the test-owned tree.
//!
//! Each test binary installs it with [`install_test_environment!`], which
//! registers a Linux `.init_array` constructor in that binary. The module only
//! exists for test builds (`cfg(test)` or the `test-support` feature, which
//! dependents enable from `[dev-dependencies]` only), so release and ordinary
//! debug builds contain none of it.
//!
//! * `HOME` and `XDG_RUNTIME_DIR` are set to directories under
//!   `<target>/<profile>/emuwiz-test-env/<exe>-<pid>-<start>-<n>` (beside the
//!   test binary, not under `/tmp`, which sandbox-visibility logic treats as
//!   host-only). The pid alone is not unique: test processes in different PID
//!   namespaces (containers, sandboxes) that share one build directory can have
//!   the same pid at the same time. Each tree is therefore owned through a
//!   sibling `<name>.lock` that its process holds locked (`flock`) for its whole
//!   life; a tree is only ever removed by its owner, or by a later process that
//!   has proven the owner gone by taking that lock.
//! * `XDG_{CONFIG,DATA,CACHE,STATE}_HOME` and `EMUWIZ_DATA_HOME` /
//!   `EMUWIZ_CONFIG_HOME` are removed, so an inherited override cannot point a
//!   test back at real data and everything resolves under the private `$HOME`
//!   exactly as for a user with no overrides (legacy-directory rules included).
//! * The tree is removed when the process exits.
//! * At exit a few sentinel files in the *real* home are compared with their
//!   state at start-up; a change prints a loud warning. (A real EmuWiz running
//!   at the same time can trigger it - it is a tripwire, not a verdict.)
//!
//! Deliberate real-machine runs must say so: set
//! `EMUWIZ_TEST_REAL_ENVIRONMENT=1` and the isolation is skipped entirely.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Opt-in for separately authorised tests that must see the real machine.
pub const REAL_ENVIRONMENT_OPT_IN: &str = "EMUWIZ_TEST_REAL_ENVIRONMENT";

/// Set by a test that spawns a child test process which must see the parent's
/// private tree (for example the Undo seal store the parent wrote). The value
/// is the parent's [`isolated_root`]. The child adopts it without creating,
/// clearing or removing anything; the parent still owns and removes the tree.
pub const ADOPT_ENVIRONMENT: &str = "EMUWIZ_TEST_ADOPT_ENVIRONMENT";

/// Files in the real home that no test may change. Relative to the real home.
const SENTINELS: &[&str] = &[
    ".local/share/archivefs/rom_organisation_approvals.json",
    ".local/share/archivefs/library.sqlite3",
    ".local/share/emuwiz/library.sqlite3",
    ".config/archivefs/gui-v2.json",
    ".config/archivefs/gui_mode.txt",
    ".config/archivefs/onboarding_state.txt",
    ".config/emuwiz/gui-v2.json",
    ".config/emuwiz/gui_mode.txt",
    ".config/emuwiz/onboarding_state.txt",
    ".config/archivefs/config.toml",
    ".config/emuwiz/config.toml",
];

const XDG_VARIABLES: [&str; 4] = [
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

struct Isolation {
    root: PathBuf,
    /// False for an adopted tree, which only its creating process may remove.
    owned: bool,
    /// The locked `<root>.lock` proving this process owns `root`; held (never
    /// read) until exit. `None` for an adopted tree.
    _owner_lock: Option<std::fs::File>,
    real_home: Option<PathBuf>,
    sentinels: Vec<(PathBuf, Option<(u64, std::time::SystemTime)>)>,
}

static ISOLATION: OnceLock<Option<Isolation>> = OnceLock::new();

fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
}

fn stat(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// The parent's tree named by [`ADOPT_ENVIRONMENT`], accepted only when it is a
/// real (non-symlink) per-process directory directly under `emuwiz-test-env`
/// that already holds the expected layout. Anything else is ignored and the
/// process isolates itself normally.
fn adopted_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os(ADOPT_ENVIRONMENT)?);
    let under_test_env = root
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "emuwiz-test-env");
    (under_test_env
        && is_real_directory(&root)
        && is_real_directory(&root.join("home"))
        && is_real_directory(&root.join("run")))
    .then_some(root)
}

/// `cargo test` puts the toolchain's library directory in `LD_LIBRARY_PATH`.
/// Production code that launches emulators (rightly) refuses any inherited
/// runtime-override variable, so tests that exercise it would fail under cargo
/// and pass when the binary is run directly. The test binary is already
/// loaded, so dropping the variable here only affects children and lookups.
fn drop_cargo_loader_path() {
    // SAFETY: called from the pre-`main` constructor, single-threaded.
    unsafe { std::env::remove_var("LD_LIBRARY_PATH") };
}

/// Isolates this process. Idempotent. Must run before any other thread starts;
/// [`install_test_environment!`] guarantees that by running it before `main`.
pub fn isolate_process_environment() {
    ISOLATION.get_or_init(|| {
        if std::env::var_os(REAL_ENVIRONMENT_OPT_IN).is_some_and(|value| value == "1") {
            return None;
        }
        drop_cargo_loader_path();
        let real_home = std::env::var_os("HOME").map(PathBuf::from);
        if let Some(adopted) = adopted_root() {
            let home = adopted.join("home");
            // SAFETY: runs from a constructor before `main`, single-threaded.
            unsafe {
                std::env::set_var("HOME", &home);
                std::env::set_var("XDG_RUNTIME_DIR", adopted.join("run"));
                std::env::set_var(
                    crate::dat::rename_apply::writer_gate::PROC_ROOT_ENV,
                    adopted.join("no-processes"),
                );
                for name in XDG_VARIABLES {
                    std::env::remove_var(name);
                }
                std::env::remove_var(crate::app_dirs::DATA_ROOT_OVERRIDE_ENV);
                std::env::remove_var(crate::app_dirs::CONFIG_ROOT_OVERRIDE_ENV);
            }
            return Some(Isolation {
                root: adopted,
                owned: false,
                _owner_lock: None,
                real_home,
                sentinels: Vec::new(),
            });
        }
        let sentinels = real_home
            .iter()
            .flat_map(|home| SENTINELS.iter().map(move |leaf| home.join(leaf)))
            .map(|path| {
                let state = stat(&path);
                (path, state)
            })
            .collect();
        let exe = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "test".into());
        let exe: String = exe
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        // Not under `/tmp`: Flatpak sandbox rules treat the host `/tmp` as invisible,
        // and several launch tests (rightly) refuse a home that lives there. The
        // tree sits beside the test binary instead, inside this worktree's own
        // build directory, and is removed on exit.
        let base = std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.parent()
                    .and_then(Path::parent)
                    .map(|profile_dir| profile_dir.join("emuwiz-test-env"))
            })
            .unwrap_or_else(|| std::env::temp_dir().join("emuwiz-test-env"));
        // The shared parent must be a real directory we can create, never a
        // symlink someone planted. Trees abandoned by processes that died
        // without running their exit hook are cleared first, but only after
        // their owner lock proves the owner gone; a live tree - whatever its
        // pid, in whatever PID namespace - is never touched.
        std::fs::create_dir_all(&base).expect("create the test environment parent");
        assert!(is_real_directory(&base), "{base:?} must not be a symlink");
        prune_abandoned_trees(&base);
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let (root, owner_lock) = claim_unique_root(&base, &exe, std::process::id(), started);
        let home = root.join("home");
        let dirs = [
            ("HOME", home.clone()),
            ("XDG_RUNTIME_DIR", root.join("run")),
            // An empty process table: tests never inspect real running
            // processes for the mixed-version writer gate.
            (
                crate::dat::rename_apply::writer_gate::PROC_ROOT_ENV,
                root.join("no-processes"),
            ),
        ];
        for (_, path) in &dirs {
            std::fs::create_dir_all(path).expect("create the test environment tree");
        }
        // SAFETY: this runs from a constructor before `main`, when the process
        // is single-threaded, so no other thread can read the environment.
        unsafe {
            for (name, path) in &dirs {
                std::env::set_var(name, path);
            }
            // An explicit XDG root would outrank the `$HOME` fallback (and the
            // legacy-directory rule), so remove them rather than redirect them:
            // everything then resolves under the private `$HOME`, exactly as it
            // does for a user with no XDG overrides.
            for name in XDG_VARIABLES {
                std::env::remove_var(name);
            }
            std::env::remove_var(crate::app_dirs::DATA_ROOT_OVERRIDE_ENV);
            std::env::remove_var(crate::app_dirs::CONFIG_ROOT_OVERRIDE_ENV);
            libc::atexit(finish_isolation);
        }
        Some(Isolation {
            root,
            owned: true,
            _owner_lock: Some(owner_lock),
            real_home,
            sentinels,
        })
    });
}

extern "C" fn finish_isolation() {
    let Some(Some(isolation)) = ISOLATION.get() else {
        return;
    };
    if !isolation.owned {
        return;
    }
    for (path, before) in &isolation.sentinels {
        if stat(path) != *before {
            eprintln!(
                "\n!!! TEST ISOLATION WARNING: {} changed while the tests ran. \
                 Either a test reached the real home (a bug) or EmuWiz itself \
                 was running at the same time.\n",
                path.display()
            );
        }
    }
    // Only ever remove the tree this process created: a real directory (never a
    // symlink) directly under `emuwiz-test-env`. `remove_dir_all` does not follow
    // symlinks inside it either.
    if is_real_directory(&isolation.root)
        && isolation
            .root
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "emuwiz-test-env")
    {
        let _ = std::fs::remove_dir_all(&isolation.root);
        // Still holding the lock: nobody else can be acting on this name.
        let _ = std::fs::remove_file(owner_lock_path(&isolation.root));
    }
}

/// `<base>/<name>.lock` for the tree `<base>/<name>`.
fn owner_lock_path(root: &Path) -> PathBuf {
    let mut name = root.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    root.with_file_name(name)
}

/// Whether the open `file` is still the file at `path` (same device and
/// inode, not following a final symlink): a lock on an unlinked or replaced
/// file proves nothing about the name.
fn still_named(file: &std::fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), std::fs::symlink_metadata(path)) {
        (Ok(open), Ok(named)) => {
            named.file_type().is_file() && open.dev() == named.dev() && open.ino() == named.ino()
        }
        _ => false,
    }
}

fn try_lock(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: `flock` on a descriptor this function borrows for the call.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Creates and locks a fresh `<name>.lock`, then the tree `<name>`, with a
/// name no other process can hold: `<exe>-<pid>-<started>-<n>`, where the
/// lock file is created exclusively. The lock is taken *before* the tree
/// exists, so a pruner can never see an unlocked tree that is about to be used.
fn claim_unique_root(base: &Path, exe: &str, pid: u32, started: u128) -> (PathBuf, std::fs::File) {
    use std::os::unix::fs::OpenOptionsExt;
    for attempt in 0..1024u32 {
        let root = base.join(format!("{exe}-{pid}-{started:x}-{attempt}"));
        let lock_path = owner_lock_path(&root);
        let lock = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock_path)
        {
            Ok(lock) => lock,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create the test environment lock {lock_path:?}: {error}"),
        };
        // A pruner may have taken (and unlinked) the brand-new lock between
        // the create and this flock; then the name is not ours: try the next.
        if !try_lock(&lock) || !still_named(&lock, &lock_path) {
            continue;
        }
        match std::fs::create_dir(&root) {
            Ok(()) => {
                assert!(is_real_directory(&root), "{root:?} must not be a symlink");
                return (root, lock);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Something already sits at a name whose lock we just
                // created: never adopt it. Give the name back, try the next.
                let _ = std::fs::remove_file(&lock_path);
            }
            Err(error) => panic!("create the test environment root {root:?}: {error}"),
        }
    }
    panic!("no free test environment name under {base:?}");
}

/// Removes trees whose owner is proven gone: their `<name>.lock` is a regular
/// file (never a symlink) that this process can lock, and is still the file
/// at that name. A held lock means a live owner, in any PID namespace, and the
/// tree is left alone; so is anything without a lock (for example trees from
/// older test binaries), which cannot be proven abandoned.
fn prune_abandoned_trees(base: &Path) {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let lock_path = entry.path();
        let Some(stem) = lock_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".lock"))
        else {
            continue;
        };
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(lock) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock_path)
        else {
            continue;
        };
        if !try_lock(&lock) || !still_named(&lock, &lock_path) {
            continue;
        }
        let root = base.join(stem);
        if is_real_directory(&root) {
            let _ = std::fs::remove_dir_all(&root);
        } else if std::fs::symlink_metadata(&root).is_ok() {
            // A planted link (or file) is removed itself, never its target.
            let _ = std::fs::remove_file(&root);
        }
        let _ = std::fs::remove_file(&lock_path);
    }
}

/// The private tree this process runs in, or `None` when the real-machine
/// opt-in is active (or isolation was never installed).
pub fn isolated_root() -> Option<&'static Path> {
    ISOLATION
        .get()
        .and_then(|isolation| isolation.as_ref())
        .map(|isolation| isolation.root.as_path())
}

/// The real home directory seen before isolation. For the tripwire test only:
/// production-path assertions use it to prove nothing resolves there.
pub fn real_home_before_isolation() -> Option<&'static Path> {
    ISOLATION
        .get()
        .and_then(|isolation| isolation.as_ref())
        .and_then(|isolation| isolation.real_home.as_deref())
}

/// Environment variable names this module points at the private tree.
pub fn isolated_variables() -> [OsString; 2] {
    ["HOME".into(), "XDG_RUNTIME_DIR".into()]
}

/// The private `$HOME` for tests that must create fixture files in it (an
/// emulator profile, say). Refuses - rather than writes - when this process is
/// not isolated, so a fixture can never be created in the real home.
pub fn private_home_for_fixtures() -> PathBuf {
    let root = isolated_root().expect(
        "this fixture writes into $HOME, so it needs the isolated test environment \
         (it is skipped-by-failure under EMUWIZ_TEST_REAL_ENVIRONMENT=1)",
    );
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    assert!(
        home.starts_with(root),
        "HOME {home:?} is outside the isolated tree {root:?}"
    );
    home
}

/// This binary's test environment trees belonging to process `pid` (any
/// start), beside this process's own tree. For a parent that re-executed this
/// test binary as a child: the child's tree name is not predictable, but it
/// always starts `<exe>-<pid>-`.
pub fn trees_of_process(pid: u32) -> Vec<PathBuf> {
    let Some(root) = isolated_root() else {
        return Vec::new();
    };
    let (Some(base), Some(name)) = (root.parent(), root.file_name().and_then(|n| n.to_str()))
    else {
        return Vec::new();
    };
    // Only a tree this process created names its pid; an adopted one does not.
    let Some(at) = name.rfind(&format!("-{}-", std::process::id())) else {
        return Vec::new();
    };
    let wanted = format!("{}-{pid}-", &name[..at]);
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&wanted) && !name.ends_with(".lock"))
        })
        .collect()
}

/// Removes every test environment tree beside this process's own whose owner
/// is proven gone (its owner lock can be taken). For a parent that killed or
/// reaped a re-executed child, whose exit hook may not have run. A live
/// tree, in any PID namespace, is never touched.
pub fn clear_abandoned_test_environments() {
    if let Some(base) = isolated_root().and_then(Path::parent) {
        prune_abandoned_trees(base);
    }
}

/// Variables that are removed so they cannot redirect a test to real data.
pub fn cleared_variables() -> impl Iterator<Item = &'static str> {
    XDG_VARIABLES.into_iter().chain([
        crate::app_dirs::DATA_ROOT_OVERRIDE_ENV,
        crate::app_dirs::CONFIG_ROOT_OVERRIDE_ENV,
    ])
}

/// Installs the isolation in the calling test binary. Put it once at the top of
/// each test crate root (`lib.rs` under `#[cfg(test)]`, or an integration
/// test file). On non-Linux targets it expands to nothing.
#[macro_export]
macro_rules! install_test_environment {
    () => {
        #[cfg(target_os = "linux")]
        #[used]
        #[unsafe(link_section = ".init_array")]
        static EMUWIZ_TEST_ENVIRONMENT_CONSTRUCTOR: extern "C" fn() = {
            extern "C" fn install() {
                $crate::test_environment::isolate_process_environment();
            }
            install
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn isolated() -> Option<&'static Path> {
        isolated_root()
    }

    #[test]
    fn this_test_binary_runs_inside_a_private_environment() {
        let Some(root) = isolated() else {
            // Only legitimate when the real-machine opt-in is set.
            assert_eq!(
                std::env::var_os(REAL_ENVIRONMENT_OPT_IN).as_deref(),
                Some(std::ffi::OsStr::new("1")),
                "isolation was not installed in this test binary"
            );
            return;
        };
        for name in isolated_variables() {
            let value = PathBuf::from(std::env::var_os(&name).expect("variable is set"));
            assert!(
                value.starts_with(root),
                "{name:?} = {value:?} escapes {root:?}"
            );
            assert!(value.is_dir(), "{name:?} directory must exist");
        }
        for name in cleared_variables() {
            assert!(std::env::var_os(name).is_none(), "{name} must be cleared");
        }
    }

    #[test]
    fn production_directory_helpers_resolve_inside_the_private_tree() {
        let Some(root) = isolated() else { return };
        let data = crate::app_dirs::data_dir().unwrap();
        let config = crate::app_dirs::config_dir().unwrap();
        assert!(data.starts_with(root), "{data:?}");
        assert!(config.starts_with(root), "{config:?}");
        if let Some(real) = real_home_before_isolation() {
            // The private tree sits in the build directory (which may itself be under
            // the real home); what matters is that it is not the user's data/config.
            for forbidden in [".local/share", ".config", ".var"] {
                assert!(!data.starts_with(real.join(forbidden)), "{data:?}");
                assert!(!config.starts_with(real.join(forbidden)), "{config:?}");
            }
            assert_ne!(
                std::env::var_os("HOME").map(PathBuf::from).as_deref(),
                Some(real)
            );
        }
    }

    #[test]
    fn production_path_semantics_are_unchanged_by_the_isolation() {
        // The pure resolvers still map a home to the documented layout, with
        // XDG and explicit roots taking precedence exactly as before.
        let home = Path::new("/nonexistent-emuwiz-home");
        assert_eq!(
            crate::app_dirs::data_dir_in_with_roots(home, None, None).unwrap(),
            home.join(".local/share/emuwiz")
        );
        assert_eq!(
            crate::app_dirs::data_dir_in_with_roots(home, None, Some(Path::new("/xdg"))).unwrap(),
            Path::new("/xdg/emuwiz")
        );
        assert_eq!(
            crate::app_dirs::data_dir_in_with_roots(
                home,
                Some(Path::new("/explicit")),
                Some(Path::new("/xdg"))
            )
            .unwrap(),
            Path::new("/explicit")
        );
        assert_eq!(
            crate::app_dirs::config_dir_in_with_roots(home, None, None).unwrap(),
            home.join(".config/emuwiz")
        );
    }

    #[test]
    fn parallel_tests_write_to_distinct_files_inside_the_tree() {
        let Some(root) = isolated() else { return };
        let data = crate::app_dirs::data_dir().unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let path = data.join(format!("parallel-probe-{index}.txt"));
                std::thread::spawn(move || {
                    for round in 0..25 {
                        std::fs::write(&path, format!("{index}:{round}")).unwrap();
                        assert_eq!(
                            std::fs::read_to_string(&path).unwrap(),
                            format!("{index}:{round}")
                        );
                    }
                    path
                })
            })
            .collect();
        for handle in handles {
            let path = handle.join().unwrap();
            assert!(path.starts_with(root));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn approvals_written_through_the_production_path_never_reach_the_real_home() {
        let Some(root) = isolated() else { return };
        let sidecar = crate::app_dirs::data_path("rom_organisation_approvals.json").unwrap();
        assert!(sidecar.starts_with(root));
        let before = real_home_before_isolation()
            .map(|home| stat(&home.join(".local/share/archivefs/rom_organisation_approvals.json")));
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(
            &sidecar,
            r#"{"version":1,"approved":["/roms/synthetic.iso"]}"#,
        )
        .unwrap();
        let after = real_home_before_isolation()
            .map(|home| stat(&home.join(".local/share/archivefs/rom_organisation_approvals.json")));
        assert_eq!(before, after);
        std::fs::remove_file(&sidecar).unwrap();
    }

    fn scratch_base(name: &str) -> PathBuf {
        let base = isolated_root()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("ownership-{name}"))
            .join("emuwiz-test-env");
        let _ = std::fs::remove_dir_all(base.parent().unwrap());
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn the_same_pid_in_two_pid_namespaces_gets_two_distinct_live_trees() {
        let base = scratch_base("same-pid");
        // Two processes that both see themselves as pid 2, started together.
        let (first, first_lock) = claim_unique_root(&base, "suite", 2, 7);
        let (second, second_lock) = claim_unique_root(&base, "suite", 2, 7);
        assert_ne!(first, second);
        std::fs::write(first.join("live.txt"), "first").unwrap();
        // The second process's start-up clean-up must not touch the first.
        prune_abandoned_trees(&base);
        assert_eq!(
            std::fs::read_to_string(first.join("live.txt")).unwrap(),
            "first"
        );
        assert!(is_real_directory(&second));
        drop((first_lock, second_lock));
        std::fs::remove_dir_all(base.parent().unwrap()).unwrap();
    }

    #[test]
    fn only_a_tree_whose_owner_is_gone_is_pruned() {
        let base = scratch_base("abandoned");
        let (live, live_lock) = claim_unique_root(&base, "suite", 10, 1);
        let (dead, dead_lock) = claim_unique_root(&base, "suite", 11, 1);
        std::fs::write(dead.join("left-behind.txt"), "x").unwrap();
        drop(dead_lock); // the owner died without its exit hook
        prune_abandoned_trees(&base);
        assert!(is_real_directory(&live));
        assert!(owner_lock_path(&live).exists());
        assert!(!dead.exists(), "{dead:?} should have been pruned");
        assert!(!owner_lock_path(&dead).exists());
        drop(live_lock);
        std::fs::remove_dir_all(base.parent().unwrap()).unwrap();
    }

    #[test]
    fn trees_without_a_provable_owner_lock_are_left_alone() {
        let base = scratch_base("unprovable");
        // A tree from an older test binary (no lock), and a planted symlink
        // lock pointing at an unlocked file elsewhere.
        let legacy = base.join("suite-42");
        std::fs::create_dir_all(legacy.join("home")).unwrap();
        let elsewhere = base.parent().unwrap().join("elsewhere.lock");
        std::fs::write(&elsewhere, "").unwrap();
        let planted = base.join("suite-43");
        std::fs::create_dir_all(&planted).unwrap();
        std::os::unix::fs::symlink(&elsewhere, base.join("suite-43.lock")).unwrap();
        prune_abandoned_trees(&base);
        assert!(is_real_directory(&legacy.join("home")));
        assert!(is_real_directory(&planted));
        assert!(elsewhere.exists());
        std::fs::remove_dir_all(base.parent().unwrap()).unwrap();
    }

    #[test]
    fn this_process_holds_the_lock_on_its_own_tree() {
        let Some(root) = isolated() else { return };
        let Some(Some(isolation)) = ISOLATION.get() else {
            return;
        };
        if !isolation.owned {
            return;
        }
        let lock = std::fs::File::open(owner_lock_path(root)).unwrap();
        assert!(
            !try_lock(&lock),
            "the owner lock must be held for the process lifetime"
        );
    }

    #[test]
    fn a_symlink_is_never_treated_as_an_owned_directory() {
        let root = isolated_root()
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        let victim = root.join("symlink-victim-dir");
        let link = root.join("symlink-probe");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("keep.txt"), "x").unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert!(is_real_directory(&victim));
        assert!(!is_real_directory(&link), "a symlink must not be owned");
        // Removing the link (as the stale-tree path would) leaves its target.
        std::fs::remove_dir_all(&link)
            .or_else(|_| std::fs::remove_file(&link))
            .unwrap();
        assert!(victim.join("keep.txt").exists());
        std::fs::remove_dir_all(&victim).unwrap();
    }
}
