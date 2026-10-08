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
//!   `<target>/<profile>/emuwiz-test-env/<exe>-<pid>` (beside the test binary,
//!   not under `/tmp`, which sandbox-visibility logic treats as host-only).
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
    real_home: Option<PathBuf>,
    sentinels: Vec<(PathBuf, Option<(u64, std::time::SystemTime)>)>,
}

static ISOLATION: OnceLock<Option<Isolation>> = OnceLock::new();

fn stat(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Isolates this process. Idempotent. Must run before any other thread starts;
/// [`install_test_environment!`] guarantees that by running it before `main`.
pub fn isolate_process_environment() {
    ISOLATION.get_or_init(|| {
        if std::env::var_os(REAL_ENVIRONMENT_OPT_IN).is_some_and(|value| value == "1") {
            return None;
        }
        let real_home = std::env::var_os("HOME").map(PathBuf::from);
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
        let root = base.join(format!("{exe}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let dirs = [
            ("HOME", home.clone()),
            ("XDG_RUNTIME_DIR", root.join("run")),
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
            real_home,
            sentinels,
        })
    });
}

extern "C" fn finish_isolation() {
    let Some(Some(isolation)) = ISOLATION.get() else {
        return;
    };
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
    // Only ever remove the tree this process created.
    if isolation
        .root
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "emuwiz-test-env")
    {
        let _ = std::fs::remove_dir_all(&isolation.root);
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
            crate::app_dirs::data_dir_in_with_roots(home, None, None),
            home.join(".local/share/emuwiz")
        );
        assert_eq!(
            crate::app_dirs::data_dir_in_with_roots(home, None, Some(Path::new("/xdg"))),
            Path::new("/xdg/emuwiz")
        );
        assert_eq!(
            crate::app_dirs::data_dir_in_with_roots(
                home,
                Some(Path::new("/explicit")),
                Some(Path::new("/xdg"))
            ),
            Path::new("/explicit")
        );
        assert_eq!(
            crate::app_dirs::config_dir_in_with_roots(home, None, None),
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
}
