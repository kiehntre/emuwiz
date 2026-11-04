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
//! "Empty" is a plain directory holding nothing but provably disposable
//! first-start leftovers: Blue's `gui-v2.json` still equal to its defaults,
//! an Undo seal store recording no seal, per-process writer registrations
//! and the thumbnail cache. Everything else is "established": any other file
//! or folder, known or unknown (customised preferences, launch preferences,
//! library views, a seal store with seals, a library), a symlink or other
//! non-directory the user placed deliberately, and anything that cannot be
//! read or verified. Unknown content therefore never silently loses
//! precedence.
//!
//! An explicit `EMUWIZ_CONFIG_HOME` / `EMUWIZ_DATA_HOME` root is always used
//! as given; that is the isolation and the way to resolve an ambiguous case.
//! XDG roots only replace the base directory: `XDG_CONFIG_HOME=$HOME/.config`
//! behaves exactly like an unset variable, and an isolated XDG root only ever
//! looks at its own `emuwiz`/`archivefs` children, never the real home.
//!
//! Resolution is read-only: nothing in the inspected folders is created,
//! changed, merged or moved (a seal store is inspected through a private
//! temporary copy), so it is idempotent across restarts. Whatever directory is chosen is where
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

/// Directories a config directory may hold and still count as empty: none.
const CONFIG_DISPOSABLE: &[&str] = &[];
/// Directories a data directory may hold and still count as empty:
/// per-process writer registrations and the regenerable thumbnail cache,
/// each only as a real directory.
const DATA_DISPOSABLE: &[&str] = &[
    crate::dat::rename_apply::writer_gate::SEALED_WRITERS_DIR,
    crate::dat::rename_apply::writer_gate::FINALISE_WRITERS_DIR,
    "gui-v2-thumbnails-v1",
];

/// Blue's settings file in the config directory.
pub const GUI_PREFERENCES_FILE: &str = "gui-v2.json";
/// What Blue writes to [`GUI_PREFERENCES_FILE`] on a first start: its
/// default preferences, unchanged. Only a file equal to this (as JSON) counts
/// as disposable; any navigation or chosen setting makes it meaningful. The
/// GUI crate's tests keep this equal to its `Preferences::default()`.
#[doc(hidden)]
pub const FIRST_START_GUI_PREFERENCES_JSON: &str = r#"{"route":"Home","filter":{"search":"","platform":"","readiness":"All","unverified_only":false,"list":false},"playing_library":{"preferred_regions":"Europe, USA, Japan, Other","preferred_languages":"","prefer_newest_revision":true,"prefer_parent":true,"exclude_beta":true,"exclude_proto":true,"exclude_demo":true,"exclude_sample":true},"welcome_dismissed":false,"beginner_hints_enabled":true,"document_roots":[],"document_associations":{},"document_reading":{},"simple_shell":true,"classic_view":false,"ui_scale":1.0}"#;
/// Bigger files are never compared; they are meaningful.
const FIRST_START_FILE_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
enum Kind {
    Config,
    Data,
}

impl Kind {
    fn disposable(self) -> &'static [&'static str] {
        match self {
            Self::Config => CONFIG_DISPOSABLE,
            Self::Data => DATA_DISPOSABLE,
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
        Ok(_) if holds_only_disposable_entries(path, kind) => Presence::Empty,
        Ok(_) => Presence::Established,
    }
}

/// True only when every entry is provably disposable: a recognised
/// directory, Blue's settings file still holding exactly its first-start
/// defaults, or an Undo seal store (with its WAL companions) that records
/// nothing. Any other entry, anything unreadable or unverifiable, or a name
/// that is not valid UTF-8 is meaningful.
fn holds_only_disposable_entries(path: &Path, kind: Kind) -> bool {
    use crate::dat::rename_apply::seal::SEAL_STORE_FILENAME;
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    let seal_wal = format!("{SEAL_STORE_FILENAME}-wal");
    let seal_shm = format!("{SEAL_STORE_FILENAME}-shm");
    // Names and types first; contents are read only if every entry is a
    // candidate, and the seal store at most once.
    let mut check_preferences = false;
    let mut check_seal_store = false;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            return false;
        };
        let Ok(file_type) = entry.file_type() else {
            return false;
        };
        if kind.disposable().contains(&name.as_str()) && file_type.is_dir() {
            continue;
        }
        if !file_type.is_file() {
            return false;
        }
        match kind {
            Kind::Config if name == GUI_PREFERENCES_FILE => check_preferences = true,
            Kind::Data if name == SEAL_STORE_FILENAME || name == seal_wal || name == seal_shm => {
                check_seal_store = true;
            }
            _ => return false,
        }
    }
    (!check_preferences || holds_first_start_gui_preferences(&path.join(GUI_PREFERENCES_FILE)))
        && (!check_seal_store || seal_store_records_nothing(&path.join(SEAL_STORE_FILENAME)))
}

/// Whether Blue's settings file equals its first-start defaults. Read-only.
fn holds_first_start_gui_preferences(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > FIRST_START_FILE_LIMIT {
        return false;
    }
    let (Some(bytes), Ok(default)) = (
        read_plain_file(path),
        serde_json::from_str::<serde_json::Value>(FIRST_START_GUI_PREFERENCES_JSON),
    ) else {
        return false;
    };
    serde_json::from_slice::<serde_json::Value>(&bytes).is_ok_and(|value| value == default)
}

/// Length and modification time of a plain file (never a symlink).
fn plain_file_stamp(path: &Path) -> Option<(u64, std::time::SystemTime)> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    metadata
        .is_file()
        .then(|| metadata.modified().ok().map(|time| (metadata.len(), time)))
        .flatten()
}

/// Reads a whole plain file of at most [`FIRST_START_FILE_LIMIT`] bytes
/// without following a symlink. Read-only.
fn read_plain_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK: a FIFO swapped in after the type check cannot hang.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(FIRST_START_FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= FIRST_START_FILE_LIMIT).then_some(bytes)
}

/// Whether the Undo seal store holds no seal and nothing else: at most the
/// `journal_seals` table (and its key index), with no rows.
///
/// The store in the inspected folder is never opened by SQLite: even a
/// read-only open creates or rewrites `-wal`/`-shm` files and can discard a
/// WAL. Instead its main file and WAL are copied into a private temporary
/// directory and the copy is inspected; the copy is refused if either
/// original changed while it was taken. An empty or oversized main file,
/// anything unreadable, a lock, other schema or any row means it records
/// something.
fn seal_store_records_nothing(path: &Path) -> bool {
    let wal = path.with_file_name(format!(
        "{}-wal",
        crate::dat::rename_apply::seal::SEAL_STORE_FILENAME
    ));
    let Some(main_before) = plain_file_stamp(path) else {
        return false;
    };
    // A first start always writes the database header; an empty main file
    // beside a WAL is a crash or truncation whose WAL SQLite would discard.
    if main_before.0 == 0 {
        return false;
    }
    let wal_before = match std::fs::symlink_metadata(&wal) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(_) => match plain_file_stamp(&wal) {
            Some(stamp) => Some(stamp),
            None => return false,
        },
        Err(_) => return false,
    };
    let Some(main_bytes) = read_plain_file(path) else {
        return false;
    };
    let wal_bytes = match wal_before {
        Some(_) => match read_plain_file(&wal) {
            Some(bytes) => Some(bytes),
            None => return false,
        },
        None => None,
    };
    let unchanged = plain_file_stamp(path) == Some(main_before)
        && main_bytes.len() as u64 == main_before.0
        && match wal_before {
            Some(stamp) => {
                plain_file_stamp(&wal) == Some(stamp)
                    && wal_bytes.as_ref().map(|bytes| bytes.len() as u64) == Some(stamp.0)
            }
            None => !entry_may_exist(&wal),
        };
    // Timestamps can be too coarse to see an in-place rewrite: re-read both
    // files and require the very same bytes.
    let reread_matches = read_plain_file(path).as_ref() == Some(&main_bytes)
        && match &wal_bytes {
            Some(bytes) => read_plain_file(&wal).as_ref() == Some(bytes),
            None => !entry_may_exist(&wal),
        };
    if !unchanged || !reread_matches {
        return false;
    }
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let Ok(scratch) = builder.tempdir() else {
        return false;
    };
    let copy = scratch.path().join("seals.sqlite3");
    if write_private(&copy, &main_bytes).is_err() {
        return false;
    }
    if let Some(bytes) = &wal_bytes
        && write_private(&scratch.path().join("seals.sqlite3-wal"), bytes).is_err()
    {
        return false;
    }
    inspect_seal_store_copy(&copy, wal_bytes.as_deref()).unwrap_or(false)
}

/// Creates a new owner-only file in the private scratch directory.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// Every WAL frame SQLite accepts, or `None` when the WAL is not a
/// well-formed sequence of whole frames for its page size.
fn wal_frame_count(wal: &[u8]) -> Option<i64> {
    const HEADER: usize = 32;
    const FRAME_HEADER: usize = 24;
    if wal.is_empty() {
        return Some(0);
    }
    let page_size = u32::from_be_bytes(wal.get(8..12)?.try_into().ok()?) as usize;
    if wal.len() < HEADER || !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
        return None;
    }
    let frames = wal.len() - HEADER;
    frames
        .is_multiple_of(FRAME_HEADER + page_size)
        .then(|| (frames / (FRAME_HEADER + page_size)) as i64)
}

/// Inspects a private copy of the seal store (opened normally so its WAL is
/// recovered into the copy). When a WAL was copied, every one of its frames
/// must be accepted by SQLite: a WAL it would partly ignore (a damaged or
/// stale frame), or one beside a store not in WAL mode, could hide a seal,
/// so it counts as recording something.
fn inspect_seal_store_copy(copy: &Path, wal: Option<&[u8]>) -> rusqlite::Result<bool> {
    use rusqlite::{Connection, OpenFlags};
    let connection = Connection::open_with_flags(
        copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    if let Some(wal) = wal.filter(|wal| !wal.is_empty()) {
        let Some(frames) = wal_frame_count(wal) else {
            return Ok(false);
        };
        let mode: String = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Ok(false);
        }
        // (busy, frames in the WAL SQLite accepts, frames checkpointed); on
        // the private copy only.
        let (busy, accepted): (i64, i64) =
            connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        if busy != 0 || accepted != frames {
            return Ok(false);
        }
    }
    connection.pragma_update(None, "query_only", true)?;
    let mut statement = connection.prepare("SELECT type, name FROM sqlite_master")?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let only_seal_table = objects.iter().all(|(kind, name)| {
        (kind == "table" && name == "journal_seals")
            || (kind == "index" && name.starts_with("sqlite_autoindex_journal_seals_"))
    });
    if !only_seal_table {
        return Ok(false);
    }
    if objects.iter().any(|(kind, _)| kind == "table") {
        let rows: i64 =
            connection.query_row("SELECT count(*) FROM journal_seals", [], |row| row.get(0))?;
        return Ok(rows == 0);
    }
    Ok(true)
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
    // Classify contents only when both candidates exist: otherwise the answer
    // is already decided and neither folder is inspected further.
    if !entry_may_exist(legacy) {
        return Ok(primary.to_path_buf());
    }
    if !entry_may_exist(primary) {
        return Ok(legacy.to_path_buf());
    }
    // The legacy folder is classified only when the EmuWiz one is established;
    // an empty EmuWiz folder decides for the legacy one by itself.
    let primary_presence = presence(primary, kind);
    if primary_presence != Established {
        return Ok(legacy.to_path_buf());
    }
    match presence(legacy, kind) {
        Absent | Empty => Ok(primary.to_path_buf()),
        Established => Err(ArchiveFsError::Config(format!(
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

#[cfg(test)]
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

fn ambiguous_under(
    explicit_root: Option<&Path>,
    base: &Path,
    names: (&str, &str),
    kind: Kind,
) -> bool {
    explicit_root.is_none()
        && entry_may_exist(&base.join(names.0))
        && entry_may_exist(&base.join(names.1))
        && presence(&base.join(names.0), kind) == Presence::Established
        && presence(&base.join(names.1), kind) == Presence::Established
}

/// Whether the current environment has two established setups (EmuWiz and
/// legacy ArchiveFS) for config or data, the case where [`config_dir`] or
/// [`data_dir`] refuses to choose. Only that case: a missing HOME, an
/// invalid override or an empty EmuWiz folder beside a legacy one is not a
/// conflict. Read-only.
pub fn two_established_setups() -> bool {
    let Ok(home) = home() else {
        return false;
    };
    two_established_setups_in(
        &home,
        (
            env::var_os(CONFIG_ROOT_OVERRIDE_ENV)
                .map(PathBuf::from)
                .as_deref(),
            env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).as_deref(),
        ),
        (
            env::var_os(DATA_ROOT_OVERRIDE_ENV)
                .map(PathBuf::from)
                .as_deref(),
            env::var_os("XDG_DATA_HOME").map(PathBuf::from).as_deref(),
        ),
    )
}

fn two_established_setups_in(
    home: &Path,
    (explicit_config, xdg_config): (Option<&Path>, Option<&Path>),
    (explicit_data, xdg_data): (Option<&Path>, Option<&Path>),
) -> bool {
    let config_base = xdg_config.map_or_else(|| home.join(".config"), Path::to_path_buf);
    let data_base = xdg_data.map_or_else(|| home.join(".local").join("share"), Path::to_path_buf);
    ambiguous_under(
        explicit_config,
        &config_base,
        (CONFIG_DIR_NAME, LEGACY_CONFIG_DIR_NAME),
        Kind::Config,
    ) || ambiguous_under(
        explicit_data,
        &data_base,
        (DATA_DIR_NAME, LEGACY_DATA_DIR_NAME),
        Kind::Data,
    )
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

    /// Only recognised disposable leftovers: an empty config folder, and
    /// writer registrations plus the thumbnail cache in the data folder.
    fn startup_leftovers(home: &Path) {
        std::fs::create_dir_all(join(home, &[".config", "emuwiz"])).unwrap();
        for dir in DATA_DISPOSABLE {
            put(home, &[".local", "share", "emuwiz", dir, "1292226"], "pid");
        }
    }

    /// Legacy populated; the EmuWiz folder of `kind` holds only `entry`.
    /// Returns whether that folder was refused as ambiguous.
    fn ambiguous_beside_legacy(name: &str, kind: Kind, entry: &[&str], contents: &str) -> bool {
        let home = home_with(name, &[]);
        legacy_install(&home);
        let base: &[&str] = match kind {
            Kind::Config => &[".config", "emuwiz"],
            Kind::Data => &[".local", "share", "emuwiz"],
        };
        let parts: Vec<&str> = base.iter().chain(entry).copied().collect();
        put(&home, &parts, contents);
        let before = tree(&home);
        let result = match kind {
            Kind::Config => config_dir_in(&home),
            Kind::Data => data_dir_in(&home),
        };
        // Refusing never changes either side.
        assert_eq!(tree(&home), before);
        let refused = result.is_err();
        let _ = std::fs::remove_dir_all(&home);
        refused
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
    fn launch_preferences_alone_make_the_new_folder_meaningful() {
        assert!(ambiguous_beside_legacy(
            "launch-prefs",
            Kind::Config,
            &["launch_preferences.json"],
            "{\"version\":1,\"preferences\":[]}",
        ));
    }

    #[test]
    fn library_views_alone_make_the_new_folder_meaningful() {
        assert!(ambiguous_beside_legacy(
            "library-views",
            Kind::Config,
            &["library_views.json"],
            "[]",
        ));
    }

    #[test]
    fn unknown_persistent_content_makes_the_new_folder_meaningful() {
        // Files EmuWiz does not recognise and unparseable window state are
        // kept, never silently outranked.
        assert!(ambiguous_beside_legacy(
            "unknown-config",
            Kind::Config,
            &["notes.txt"],
            "mine"
        ));
        assert!(ambiguous_beside_legacy(
            "window-state",
            Kind::Config,
            &["gui-v2.json"],
            "{}"
        ));
        assert!(ambiguous_beside_legacy(
            "unknown-data",
            Kind::Data,
            &["future-store", "x.db"],
            "x"
        ));
        // Not a database at all: unverifiable, so meaningful.
        assert!(ambiguous_beside_legacy(
            "seals-garbage",
            Kind::Data,
            &["undo-seals.sqlite3"],
            "not sqlite"
        ));
        // A disposable name used for a file rather than a directory is not
        // the recognised artefact.
        assert!(ambiguous_beside_legacy(
            "disposable-as-file",
            Kind::Data,
            &["sealed-writers"],
            "x"
        ));
    }

    /// Creates the Undo seal store the way the store does (WAL, the
    /// `journal_seals` table) with `seals` rows, and returns the writer
    /// connection still open, so the WAL and shared-memory companions exist
    /// as after a first start that has not checkpointed.
    fn seal_store(dir: &Path, seals: usize) -> rusqlite::Connection {
        std::fs::create_dir_all(dir).unwrap();
        let connection = rusqlite::Connection::open(dir.join("undo-seals.sqlite3")).unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS journal_seals (
                    namespace_id TEXT NOT NULL,
                    transaction_id TEXT NOT NULL,
                    workflow TEXT NOT NULL,
                    namespace TEXT NOT NULL,
                    generation INTEGER NOT NULL,
                    digest BLOB,
                    pending_generation INTEGER,
                    pending_digest BLOB,
                    PRIMARY KEY (namespace_id, transaction_id)
                );",
            )
            .unwrap();
        for seal in 0..seals {
            connection
                .execute(
                    "INSERT INTO journal_seals VALUES (?1, 't', 'w', 'n', 1, x'00', NULL, NULL)",
                    [format!("ns-{seal}")],
                )
                .unwrap();
        }
        connection
    }

    /// Blue's first-start settings, re-serialised in another key order and
    /// layout: compared as JSON, not as bytes.
    fn first_start_preferences_reordered() -> String {
        let value: serde_json::Value =
            serde_json::from_str(FIRST_START_GUI_PREFERENCES_JSON).unwrap();
        let mut entries: Vec<_> = value.as_object().unwrap().iter().collect();
        entries.reverse();
        let reordered: serde_json::Map<_, _> = entries
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        serde_json::to_string_pretty(&serde_json::Value::Object(reordered)).unwrap()
    }

    /// The precise first-start case: default `gui-v2.json`, an Undo seal
    /// store with no seals (WAL companions present), writer registrations.
    #[test]
    fn fresh_default_startup_files_keep_the_legacy_library() {
        let home = home_with("first-start", &[]);
        legacy_install(&home);
        startup_leftovers(&home);
        put(
            &home,
            &[".config", "emuwiz", GUI_PREFERENCES_FILE],
            &first_start_preferences_reordered(),
        );
        let writer = seal_store(&home.join(".local/share/emuwiz"), 0);
        assert!(
            home.join(".local/share/emuwiz/undo-seals.sqlite3-wal")
                .exists()
        );
        let before = tree(&home);

        assert_eq!(
            config_dir_in(&home).unwrap(),
            home.join(".config/archivefs")
        );
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        assert!(!two_established_setups_in(
            &home,
            (None, None),
            (None, None)
        ));
        // Nothing anywhere under HOME changed, the inspected EmuWiz data
        // folder (seal store, WAL, shared memory) included.
        assert_eq!(tree(&home), before);
        drop(writer);

        // After the writer closes (WAL checkpointed away) it is still empty,
        // and inspecting it creates no -wal/-shm beside it.
        let closed = tree(&home);
        assert!(
            !home
                .join(".local/share/emuwiz/undo-seals.sqlite3-wal")
                .exists()
        );
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        assert_eq!(tree(&home), closed);
        assert!(
            !home
                .join(".local/share/emuwiz/undo-seals.sqlite3-wal")
                .exists()
        );
        assert!(
            !home
                .join(".local/share/emuwiz/undo-seals.sqlite3-shm")
                .exists()
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A seal store is only inspected when both folders exist, and never in
    /// place: a legacy folder holding one, with no EmuWiz folder, is left
    /// exactly as it was.
    #[test]
    fn a_seal_store_is_never_opened_where_it_lies() {
        let home = home_with("seal-legacy-only", &[]);
        legacy_install(&home);
        drop(seal_store(&home.join(".local/share/archivefs"), 0));
        let before = tree(&home);
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        assert!(!two_established_setups_in(
            &home,
            (None, None),
            (None, None)
        ));
        assert_eq!(tree(&home), before);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Copies a live store (main file + WAL, writer still open so nothing is
    /// checkpointed) into the EmuWiz data folder of `home`, letting `edit`
    /// change the copied WAL bytes.
    fn place_uncheckpointed_store(home: &Path, seals: usize, edit: impl FnOnce(&mut Vec<u8>)) {
        let source = home.join("live-store");
        let writer = seal_store(&source, seals);
        let main = std::fs::read(source.join("undo-seals.sqlite3")).unwrap();
        let mut wal = std::fs::read(source.join("undo-seals.sqlite3-wal")).unwrap();
        edit(&mut wal);
        drop(writer);
        std::fs::remove_dir_all(&source).unwrap();
        let data = home.join(".local/share/emuwiz");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("undo-seals.sqlite3"), main).unwrap();
        std::fs::write(data.join("undo-seals.sqlite3-wal"), wal).unwrap();
    }

    #[test]
    fn an_intact_uncheckpointed_empty_store_is_still_disposable() {
        let home = home_with("wal-intact", &[]);
        legacy_install(&home);
        place_uncheckpointed_store(&home, 0, |_| {});
        let before = tree(&home);
        assert_eq!(
            data_dir_in(&home).unwrap(),
            home.join(".local/share/archivefs")
        );
        assert_eq!(tree(&home), before);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A WAL frame SQLite would reject could hide a seal: never "empty".
    #[test]
    fn a_wal_with_a_damaged_or_partial_frame_is_meaningful() {
        for (name, seals, edit) in [
            (
                "wal-damaged-empty",
                0,
                (|wal: &mut Vec<u8>| *wal.last_mut().unwrap() ^= 0xff) as fn(&mut Vec<u8>),
            ),
            ("wal-damaged-seal", 1, |wal: &mut Vec<u8>| {
                *wal.last_mut().unwrap() ^= 0xff
            }),
            ("wal-partial-frame", 0, |wal: &mut Vec<u8>| {
                wal.truncate(wal.len() - 1)
            }),
            ("wal-intact-seal", 1, |_: &mut Vec<u8>| {}),
        ] {
            let home = home_with(name, &[]);
            legacy_install(&home);
            place_uncheckpointed_store(&home, seals, edit);
            let before = tree(&home);
            assert!(data_dir_in(&home).is_err(), "{name} must be meaningful");
            assert_eq!(tree(&home), before, "{name} must be left untouched");
            let _ = std::fs::remove_dir_all(&home);
        }
    }

    /// A truncated main file with the seal only in a WAL: SQLite would
    /// discard that WAL, so it is unverifiable and meaningful, and the WAL
    /// is left in place.
    #[test]
    fn a_truncated_seal_store_with_a_wal_is_meaningful_and_untouched() {
        let home = home_with("seal-truncated", &[]);
        legacy_install(&home);
        let data = home.join(".local/share/emuwiz");
        let writer = seal_store(&data, 1);
        let wal = std::fs::read(data.join("undo-seals.sqlite3-wal")).unwrap();
        drop(writer);
        std::fs::write(data.join("undo-seals.sqlite3"), b"").unwrap();
        std::fs::write(data.join("undo-seals.sqlite3-wal"), &wal).unwrap();
        let _ = std::fs::remove_file(data.join("undo-seals.sqlite3-shm"));
        let before = tree(&home);
        assert!(data_dir_in(&home).is_err());
        assert_eq!(tree(&home), before);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn customised_gui_preferences_make_the_new_folder_meaningful() {
        let mut value: serde_json::Value =
            serde_json::from_str(FIRST_START_GUI_PREFERENCES_JSON).unwrap();
        value["ui_scale"] = serde_json::json!(1.25);
        assert!(ambiguous_beside_legacy(
            "custom-scale",
            Kind::Config,
            &[GUI_PREFERENCES_FILE],
            &value.to_string(),
        ));
        let mut value: serde_json::Value =
            serde_json::from_str(FIRST_START_GUI_PREFERENCES_JSON).unwrap();
        value["document_roots"] = serde_json::json!(["/home/me/manuals"]);
        assert!(ambiguous_beside_legacy(
            "custom-documents",
            Kind::Config,
            &[GUI_PREFERENCES_FILE],
            &value.to_string(),
        ));
        // Browsing also changes it; a used profile is never discarded.
        let mut value: serde_json::Value =
            serde_json::from_str(FIRST_START_GUI_PREFERENCES_JSON).unwrap();
        value["route"] = serde_json::json!({"Game": 42});
        assert!(ambiguous_beside_legacy(
            "browsed",
            Kind::Config,
            &[GUI_PREFERENCES_FILE],
            &value.to_string(),
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_default_settings_file_is_not_disposable() {
        let home = home_with("settings-link", &[&[".config", "emuwiz"]]);
        legacy_install(&home);
        put(&home, &["elsewhere.json"], FIRST_START_GUI_PREFERENCES_JSON);
        std::os::unix::fs::symlink(
            home.join("elsewhere.json"),
            home.join(".config/emuwiz").join(GUI_PREFERENCES_FILE),
        )
        .unwrap();
        assert!(config_dir_in(&home).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn populated_undo_state_makes_the_new_folder_meaningful() {
        let home = home_with("seals-populated", &[]);
        legacy_install(&home);
        startup_leftovers(&home);
        // With the writer still open (seal only in the WAL) and after close.
        let writer = seal_store(&home.join(".local/share/emuwiz"), 1);
        assert!(data_dir_in(&home).is_err());
        assert!(two_established_setups_in(&home, (None, None), (None, None)));
        drop(writer);
        assert!(data_dir_in(&home).is_err());

        // Any other schema object is unverified content too.
        let other = home_with("seals-other-table", &[]);
        legacy_install(&other);
        let writer = seal_store(&other.join(".local/share/emuwiz"), 0);
        writer.execute_batch("CREATE TABLE extra (x)").unwrap();
        drop(writer);
        assert!(data_dir_in(&other).is_err());
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_never_treated_as_empty() {
        use std::os::unix::fs::PermissionsExt;

        let home = home_with("unreadable", &[&[".local", "share", "emuwiz"]]);
        legacy_install(&home);
        let new_data = home.join(".local/share/emuwiz");
        std::fs::set_permissions(&new_data, std::fs::Permissions::from_mode(0o000)).unwrap();
        let privileged = std::fs::read_dir(&new_data).is_ok();
        let result = data_dir_in(&home);
        std::fs::set_permissions(&new_data, std::fs::Permissions::from_mode(0o755)).unwrap();
        if privileged {
            // Root reads through mode 000; the permission case cannot be
            // produced here, so assert the readable-empty behaviour instead.
            assert_eq!(result.unwrap(), home.join(".local/share/archivefs"));
        } else {
            assert!(
                result.is_err(),
                "an unreadable folder must not count as empty"
            );
        }

        // An unreadable legacy folder beside an empty new one keeps the
        // legacy choice; it is not displaced either.
        let legacy = home.join(".local/share/archivefs");
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = data_dir_in(&home);
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(result.unwrap(), legacy);
        let _ = std::fs::remove_dir_all(&home);
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
        // A stray empty EmuWiz folder under the XDG root still never reaches
        // the real home.
        std::fs::create_dir_all(xdg.join(DATA_DIR_NAME)).unwrap();
        assert_eq!(
            data_dir_in_with_roots(&home, None, Some(&xdg)).unwrap(),
            xdg.join(DATA_DIR_NAME)
        );
        assert!(!two_established_setups_in(
            &home,
            (None, None),
            (None, Some(&xdg))
        ));
        std::fs::remove_dir(xdg.join(DATA_DIR_NAME)).unwrap();
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

    /// The setup-conflict signal agrees with resolution: an empty EmuWiz
    /// folder beside a legacy install is not a conflict; two established
    /// ones are, unless an explicit root has chosen.
    #[test]
    fn two_established_setups_matches_the_resolver_refusal() {
        let home = home_with("conflict-signal", &[]);
        let none = (None, None);
        assert!(!two_established_setups_in(&home, none, none));
        legacy_install(&home);
        startup_leftovers(&home);
        assert!(!two_established_setups_in(&home, none, none));
        assert!(config_dir_in(&home).is_ok() && data_dir_in(&home).is_ok());

        put(
            &home,
            &[".local", "share", "emuwiz", "library.sqlite3"],
            "new-db",
        );
        assert!(data_dir_in(&home).is_err());
        assert!(two_established_setups_in(&home, none, none));

        let chosen = home.join(".local/share/archivefs");
        assert!(!two_established_setups_in(
            &home,
            none,
            (Some(&chosen), None)
        ));
        // An XDG root at the default base sees the same conflict.
        let data_base = home.join(".local/share");
        assert!(two_established_setups_in(
            &home,
            none,
            (None, Some(&data_base))
        ));
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
