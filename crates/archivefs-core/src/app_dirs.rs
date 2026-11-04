//! App-directory resolution with legacy ArchiveFS compatibility.
//!
//! EmuWiz prefers its own `emuwiz` directories but keeps using the legacy
//! `archivefs` directories of a pre-rename install. Under each base
//! directory (`~/.config` or `$XDG_CONFIG_HOME` for config, `~/.local/share`
//! or `$XDG_DATA_HOME` for data) the two candidates are compared:
//!
//! | `emuwiz`    | `archivefs` | result                                   |
//! |-------------|-------------|------------------------------------------|
//! | absent      | absent      | `emuwiz` (fresh install)                 |
//! | any         | absent      | `emuwiz`                                 |
//! | absent      | any         | `archivefs`                              |
//! | empty       | present     | `archivefs` (an empty new folder never   |
//! |             |             | displaces an existing install)           |
//! | established | empty       | `emuwiz`                                 |
//! | established | established | error: ambiguous, nothing is chosen      |
//!
//! "Established" means the directory holds one of the kind's marker files
//! (the library database or the main config files), is not a plain directory
//! (a symlink or other entry the user placed deliberately), or cannot be
//! inspected. "Empty" is a plain directory without any marker; it may still
//! contain files EmuWiz writes on start-up, such as window state.
//!
//! An explicit `EMUWIZ_CONFIG_HOME` / `EMUWIZ_DATA_HOME` root is always used
//! as given; that is the isolation and the way to resolve an ambiguous case.
//! XDG roots only replace the base directory: `XDG_CONFIG_HOME=$HOME/.config`
//! behaves exactly like an unset variable, and an isolated XDG root only ever
//! looks at its own `emuwiz`/`archivefs` children, never the real home.
//!
//! Resolution is read-only: nothing is created, copied, merged or moved, so
//! it is idempotent across restarts. Whatever directory is chosen is where
//! later writes go, which keeps the same choice on the next start.

use std::env;
use std::path::{Path, PathBuf};

use crate::{ArchiveFsError, Result};

/// The EmuWiz config directory name under `~/.config`.
pub const CONFIG_DIR_NAME: &str = "emuwiz";
/// The legacy ArchiveFS config directory name under `~/.config`.
pub const LEGACY_CONFIG_DIR_NAME: &str = "archivefs";
/// The EmuWiz data directory name under `~/.local/share`.
pub const DATA_DIR_NAME: &str = "emuwiz";
/// The legacy ArchiveFS data directory name under `~/.local/share`.
pub const LEGACY_DATA_DIR_NAME: &str = "archivefs";
pub const DATA_ROOT_OVERRIDE_ENV: &str = "EMUWIZ_DATA_HOME";
pub const CONFIG_ROOT_OVERRIDE_ENV: &str = "EMUWIZ_CONFIG_HOME";

/// Files whose presence means a config directory holds a user's settings.
const CONFIG_MARKERS: &[&str] = &["config.toml", "dat_sources.toml"];
/// Files whose presence means a data directory holds a user's library.
const DATA_MARKERS: &[&str] = &["library.sqlite3", "index.json"];

#[derive(Clone, Copy)]
enum Kind {
    Config,
    Data,
}

impl Kind {
    fn markers(self) -> &'static [&'static str] {
        match self {
            Self::Config => CONFIG_MARKERS,
            Self::Data => DATA_MARKERS,
        }
    }

    fn override_env(self) -> &'static str {
        match self {
            Self::Config => CONFIG_ROOT_OVERRIDE_ENV,
            Self::Data => DATA_ROOT_OVERRIDE_ENV,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Config => "settings",
            Self::Data => "library data",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    Absent,
    Empty,
    Established,
}

fn home() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| ArchiveFsError::Config("HOME is not set".to_string()))
}

/// Read-only classification of one candidate directory. Anything that cannot
/// be proven to be an empty plain directory counts as established, so an
/// uncertain candidate is never silently displaced.
fn presence(path: &Path, kind: Kind) -> Presence {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Presence::Absent,
        Err(_) => Presence::Established,
        Ok(metadata) if !metadata.file_type().is_dir() => Presence::Established,
        Ok(_) => {
            if kind
                .markers()
                .iter()
                .any(|marker| entry_may_exist(&path.join(marker)))
            {
                Presence::Established
            } else {
                Presence::Empty
            }
        }
    }
}

fn entry_may_exist(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Picks between the EmuWiz and legacy directories under one base, following
/// the table in the module documentation. Read-only.
fn choose_dir(primary: &Path, legacy: &Path, kind: Kind) -> Result<PathBuf> {
    use Presence::{Absent, Empty, Established};
    match (presence(primary, kind), presence(legacy, kind)) {
        (_, Absent) => Ok(primary.to_path_buf()),
        (Absent | Empty, _) => Ok(legacy.to_path_buf()),
        (Established, Empty) => Ok(primary.to_path_buf()),
        (Established, Established) => Err(ArchiveFsError::Config(format!(
            "EmuWiz found {noun} in two places and will not choose between them: {primary} and {legacy}. \
             Nothing was changed. Start EmuWiz with {env} set to the folder you want to use, \
             or move the folder you no longer use out of {base}.",
            noun = kind.noun(),
            primary = primary.display(),
            legacy = legacy.display(),
            env = kind.override_env(),
            base = primary.parent().unwrap_or(primary).display(),
        ))),
    }
}

fn resolve(
    home: &Path,
    explicit_root: Option<&Path>,
    xdg_base: Option<&Path>,
    default_base: &[&str],
    names: (&str, &str),
    kind: Kind,
) -> Result<PathBuf> {
    if let Some(root) = explicit_root {
        return Ok(root.to_path_buf());
    }
    let base = match xdg_base {
        Some(base) => base.to_path_buf(),
        None => default_base
            .iter()
            .fold(home.to_path_buf(), |path, part| path.join(part)),
    };
    choose_dir(&base.join(names.0), &base.join(names.1), kind)
}

pub(crate) fn config_dir_in(home: &Path) -> Result<PathBuf> {
    config_dir_in_with_roots(home, None, None)
}

/// Pure config-directory resolver; see the module documentation.
pub(crate) fn config_dir_in_with_roots(
    home: &Path,
    explicit_root: Option<&Path>,
    xdg_config_home: Option<&Path>,
) -> Result<PathBuf> {
    resolve(
        home,
        explicit_root,
        xdg_config_home,
        &[".config"],
        (CONFIG_DIR_NAME, LEGACY_CONFIG_DIR_NAME),
        Kind::Config,
    )
}

pub(crate) fn data_dir_in(home: &Path) -> Result<PathBuf> {
    data_dir_in_with_roots(home, None, None)
}

/// Pure data-directory resolver; see the module documentation.
pub(crate) fn data_dir_in_with_roots(
    home: &Path,
    explicit_root: Option<&Path>,
    xdg_data_home: Option<&Path>,
) -> Result<PathBuf> {
    resolve(
        home,
        explicit_root,
        xdg_data_home,
        &[".local", "share"],
        (DATA_DIR_NAME, LEGACY_DATA_DIR_NAME),
        Kind::Data,
    )
}

#[cfg(test)]
pub(crate) fn config_path_in(home: &Path, leaf: &str) -> PathBuf {
    config_dir_in(home).unwrap().join(leaf)
}

#[cfg(test)]
pub(crate) fn data_path_in(home: &Path, leaf: &str) -> PathBuf {
    data_dir_in(home).unwrap().join(leaf)
}

fn absolute_env(name: &str, what: &str) -> Result<Option<PathBuf>> {
    let value = env::var_os(name).map(PathBuf::from);
    if value.as_deref().is_some_and(|path| !path.is_absolute()) {
        return Err(ArchiveFsError::Config(format!(
            "catalogue/{what} root override must be an absolute path"
        )));
    }
    Ok(value)
}

/// The config directory root for the current user; see the module
/// documentation for how the EmuWiz and legacy directories are chosen.
pub fn config_dir() -> Result<PathBuf> {
    let home = home()?;
    let explicit_root = absolute_env(CONFIG_ROOT_OVERRIDE_ENV, "config")?;
    let xdg_config_home = absolute_env("XDG_CONFIG_HOME", "config")?;
    config_dir_in_with_roots(&home, explicit_root.as_deref(), xdg_config_home.as_deref())
}

/// The data directory root for the current user; see the module
/// documentation for how the EmuWiz and legacy directories are chosen.
pub fn data_dir() -> Result<PathBuf> {
    let home = home()?;
    let explicit_root = absolute_env(DATA_ROOT_OVERRIDE_ENV, "data")?;
    let xdg_data_home = absolute_env("XDG_DATA_HOME", "data")?;
    data_dir_in_with_roots(&home, explicit_root.as_deref(), xdg_data_home.as_deref())
}

/// The path to one config file (e.g. `config.toml`) under the effective
/// config directory.
pub fn config_path(leaf: &str) -> Result<PathBuf> {
    Ok(config_dir()?.join(leaf))
}

/// The path to one data file (e.g. `library.sqlite3`) under the effective
/// data directory.
pub fn data_path(leaf: &str) -> Result<PathBuf> {
    Ok(data_dir()?.join(leaf))
}

/// Whether the legacy `archivefs` config directory exists for the current
/// user. Used by diagnostics and migration tooling.
pub fn legacy_config_dir_exists() -> bool {
    home()
        .map(|home| entry_may_exist(&home.join(".config").join(LEGACY_CONFIG_DIR_NAME)))
        .unwrap_or(false)
}

/// Whether the legacy `archivefs` data directory exists for the current user.
pub fn legacy_data_dir_exists() -> bool {
    home()
        .map(|home| entry_may_exist(&home.join(".local").join("share").join(LEGACY_DATA_DIR_NAME)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A private temp HOME for one test, with the given directory trees
    /// already created.
    fn home_with(name: &str, dirs: &[&[&str]]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("archivefs-app-dirs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for parts in dirs {
            std::fs::create_dir_all(join(&root, parts)).unwrap();
        }
        root
    }

    fn join(root: &Path, parts: &[&str]) -> PathBuf {
        parts
            .iter()
            .fold(root.to_path_buf(), |path, part| path.join(part))
    }

    /// Writes `contents` to `home/<parts>`, creating parent directories.
    fn put(home: &Path, parts: &[&str], contents: &str) {
        let path = join(home, parts);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// An established legacy install: settings and a library database.
    fn legacy_install(home: &Path) {
        put(home, &[".config", "archivefs", "config.toml"], "legacy");
        put(
            home,
            &[".local", "share", "archivefs", "library.sqlite3"],
            "legacy-db",
        );
    }

    /// What a first start of a new build leaves in an otherwise unused
    /// EmuWiz directory: window state and empty bookkeeping, no library.
    fn startup_leftovers(home: &Path) {
        put(home, &[".config", "emuwiz", "gui-v2.json"], "{}");
        put(
            home,
            &[".local", "share", "emuwiz", "undo-seals.sqlite3"],
            "",
        );
        std::fs::create_dir_all(join(home, &[".local", "share", "emuwiz", "sealed-writers"]))
            .unwrap();
    }

    /// Every file and directory under `root`, for no-write assertions.
    fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                for entry in std::fs::read_dir(&path).unwrap() {
                    pending.push(entry.unwrap().path());
                }
                out.push((path, Vec::new()));
            } else if meta.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                out.push((path, bytes));
            } else {
                out.push((path, b"<other>".to_vec()));
            }
        }
        out.sort();
        out
    }

    #[test]
    fn fresh_install_uses_emuwiz_paths() {
        let home = home_with("fresh", &[]);
        assert_eq!(
            config_path_in(&home, "config.toml"),
            home.join(".config/emuwiz/config.toml")
        );
        assert_eq!(
            data_path_in(&home, "library.sqlite3"),
            home.join(".local/share/emuwiz/library.sqlite3")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn legacy_only_install_transparently_reuses_archivefs_paths() {
        let home = home_with("legacy", &[]);
        legacy_install(&home);
        assert_eq!(
            config_dir_in(&home).unwrap(),
            home.join(".config/archivefs")
        );
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The reported defect: an empty `emuwiz` folder (here holding only what
    /// a first start writes) displaced an established legacy library.
    #[test]
    fn an_empty_emuwiz_directory_never_displaces_an_established_legacy_install() {
        let home = home_with("empty-new", &[]);
        legacy_install(&home);
        startup_leftovers(&home);
        assert_eq!(
            config_dir_in(&home).unwrap(),
            home.join(".config/archivefs")
        );
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );

        // Bare empty directories behave the same.
        let bare = home_with(
            "empty-new-bare",
            &[&[".config", "emuwiz"], &[".local", "share", "emuwiz"]],
        );
        legacy_install(&bare);
        assert_eq!(
            config_dir_in(&bare).unwrap(),
            bare.join(".config/archivefs")
        );
        assert_eq!(
            data_dir_in(&bare).unwrap(),
            bare.join(".local/share/archivefs")
        );
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn an_established_emuwiz_directory_wins_over_an_empty_legacy_one() {
        let home = home_with(
            "new-established",
            &[&[".config", "archivefs"], &[".local", "share", "archivefs"]],
        );
        put(&home, &[".config", "emuwiz", "config.toml"], "new");
        put(
            &home,
            &[".local", "share", "emuwiz", "library.sqlite3"],
            "new-db",
        );
        assert_eq!(config_dir_in(&home).unwrap(), home.join(".config/emuwiz"));
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/emuwiz")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn both_directories_empty_keep_the_legacy_install() {
        let home = home_with(
            "both-empty",
            &[
                &[".config", "archivefs"],
                &[".config", "emuwiz"],
                &[".local", "share", "archivefs"],
                &[".local", "share", "emuwiz"],
            ],
        );
        assert_eq!(
            config_dir_in(&home).unwrap(),
            home.join(".config/archivefs")
        );
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn two_established_directories_are_ambiguous_and_name_both_and_the_way_out() {
        let home = home_with("dual", &[]);
        legacy_install(&home);
        put(&home, &[".config", "emuwiz", "config.toml"], "new");
        put(
            &home,
            &[".local", "share", "emuwiz", "library.sqlite3"],
            "new-db",
        );
        let before = tree(&home);

        for (error, env, primary, legacy) in [
            (
                config_dir_in(&home).unwrap_err().to_string(),
                CONFIG_ROOT_OVERRIDE_ENV,
                home.join(".config/emuwiz"),
                home.join(".config/archivefs"),
            ),
            (
                data_dir_in(&home).unwrap_err().to_string(),
                DATA_ROOT_OVERRIDE_ENV,
                home.join(".local/share/emuwiz"),
                home.join(".local/share/archivefs"),
            ),
        ] {
            assert!(error.contains(&primary.display().to_string()), "{error}");
            assert!(error.contains(&legacy.display().to_string()), "{error}");
            assert!(error.contains(env), "{error}");
            assert!(error.contains("Nothing was changed"), "{error}");
        }
        // Nothing merged, moved or overwritten; an explicit root resolves it.
        assert_eq!(tree(&home), before);
        let chosen = home.join(".local/share/archivefs");
        assert_eq!(
            data_dir_in_with_roots(&home, Some(&chosen), None).unwrap(),
            chosen
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// `XDG_*_HOME` pointing at the default base must not change anything:
    /// this is how a launcher that exports XDG variables used to create
    /// empty `emuwiz` folders beside an established library.
    #[test]
    fn an_xdg_root_equal_to_the_default_base_resolves_like_no_xdg_root() {
        let home = home_with("xdg-default", &[]);
        legacy_install(&home);
        let config_base = home.join(".config");
        let data_base = home.join(".local/share");
        assert_eq!(
            config_dir_in_with_roots(&home, None, Some(&config_base)).unwrap(),
            home.join(".config/archivefs")
        );
        assert_eq!(
            data_dir_in_with_roots(&home, None, Some(&data_base)).unwrap(),
            home.join(".local/share/archivefs")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_isolated_xdg_root_only_sees_its_own_children_never_the_real_home() {
        let home = home_with("xdg-isolated", &[]);
        legacy_install(&home);
        let xdg = home.join("xdg-data");
        // Nothing under the XDG root: a fresh EmuWiz directory there, never
        // the home's established legacy library.
        assert_eq!(
            data_dir_in_with_roots(&home, None, Some(&xdg)).unwrap(),
            xdg.join(DATA_DIR_NAME)
        );
        // A legacy copy placed under the XDG root is used, like under HOME.
        put(&xdg, &["archivefs", "library.sqlite3"], "copy");
        assert_eq!(
            data_dir_in_with_roots(&home, None, Some(&xdg)).unwrap(),
            xdg.join(LEGACY_DATA_DIR_NAME)
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn explicit_root_wins_over_xdg_legacy_and_ambiguity() {
        let home = home_with("precedence", &[]);
        legacy_install(&home);
        put(
            &home,
            &[".local", "share", "emuwiz", "library.sqlite3"],
            "new-db",
        );
        assert!(data_dir_in(&home).is_err());
        let explicit = home.join("requested");
        let xdg = home.join("xdg");
        assert_eq!(
            data_dir_in_with_roots(&home, Some(&explicit), Some(&xdg)).unwrap(),
            explicit
        );
        assert_eq!(
            config_dir_in_with_roots(&home, Some(&explicit), Some(&xdg)).unwrap(),
            explicit
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn config_and_data_directories_resolve_independently_mixed_states() {
        // EmuWiz config dir exists, but the database lives in legacy.
        let home = home_with(
            "mixed",
            &[&[".config", "emuwiz"], &[".local", "share", "archivefs"]],
        );
        assert_eq!(config_dir_in(&home).unwrap(), home.join(".config/emuwiz"));
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Restart behaviour: resolution is read-only, and repeating it (as every
    /// start does) gives the same answer for every layout.
    #[test]
    fn resolution_never_writes_and_is_stable_across_restarts() {
        let fresh = home_with("nowrites", &[]);
        let _ = config_path_in(&fresh, "config.toml");
        let _ = data_path_in(&fresh, "library.sqlite3");
        assert!(!fresh.join(".config").exists());
        assert!(!fresh.join(".local").exists());

        let home = home_with("restart", &[]);
        legacy_install(&home);
        startup_leftovers(&home);
        let before = tree(&home);
        let first = (config_dir_in(&home).unwrap(), data_dir_in(&home).unwrap());
        for _ in 0..3 {
            assert_eq!(
                (config_dir_in(&home).unwrap(), data_dir_in(&home).unwrap()),
                first
            );
        }
        // A later start that writes window state into the chosen legacy
        // directory keeps choosing it.
        put(
            &home,
            &[".config", "archivefs", "gui-v2.json"],
            "{\"route\":1}",
        );
        assert_eq!(
            config_dir_in(&home).unwrap(),
            home.join(".config/archivefs")
        );
        let after: Vec<_> = tree(&home)
            .into_iter()
            .filter(|(path, _)| !path.ends_with(".config/archivefs/gui-v2.json"))
            .collect();
        assert_eq!(after, before);
        let _ = std::fs::remove_dir_all(&fresh);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_primary_symlink_still_has_precedence_over_an_empty_legacy_dir() {
        use std::os::unix::fs::symlink;

        let home = home_with("broken-primary", &[&[".config", "archivefs"]]);
        let primary = home.join(".config/emuwiz");
        symlink(home.join("missing-target"), &primary).unwrap();

        assert_eq!(config_dir_in(&home).unwrap(), primary);
        // Beside an established legacy install it is ambiguous, not a silent
        // switch to either side.
        put(&home, &[".config", "archivefs", "config.toml"], "legacy");
        assert!(config_dir_in(&home).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_legacy_directory_is_never_displaced_by_an_empty_emuwiz_one() {
        use std::os::unix::fs::symlink;

        let home = home_with(
            "legacy-link",
            &[&["elsewhere"], &[".local", "share", "emuwiz"]],
        );
        std::fs::create_dir_all(home.join(".local/share")).unwrap();
        symlink(home.join("elsewhere"), home.join(".local/share/archivefs")).unwrap();
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
