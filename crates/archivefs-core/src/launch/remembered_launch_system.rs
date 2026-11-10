//! "Remember this launch choice": the person's explicit, saved answer to
//! "which system should this unidentified file be started as?" for one
//! unchanged file (owner decision 2026-11-10).
//!
//! It exists so that a game whose system comes only from a folder name does
//! not need a one-time "Try anyway" on every play. It is deliberately narrow:
//!
//! - It is written only when the person ticks "Remember this launch choice"
//!   on the Launch page. "Try anyway" on its own never writes it.
//! - It is bound to one file *as it was when chosen*: absolute path, size and
//!   modification time. Any change to the file makes the entry inert (it is
//!   never applied to different bytes).
//! - It is launch-only. It never changes the library's recorded system, a
//!   manual system choice, catalogue evidence or verified identity, and
//!   anything the file says about itself (a header naming another system, a
//!   later content identification) still wins. Format, media, firmware,
//!   executable and click-time checks are unchanged.
//!
//! The entries live in their own versioned file next to the emulator choices
//! ([`super::launch_choice::LAUNCH_PREFERENCES_FILE`]) and reuse the same
//! bounded, regular-file-only reading and atomic writing. They are not added
//! to that file because its strict format is shared with installed releases,
//! which would then refuse every saved emulator choice.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// File name inside the EmuWiz configuration directory.
pub const REMEMBERED_LAUNCH_SYSTEMS_FILE: &str = "remembered_launch_systems.json";
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const FORMAT_VERSION: u32 = 1;

/// One remembered choice: start `path` (unchanged) as `platform_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RememberedLaunchSystem {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub modified_ns: u64,
    pub platform_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RememberedFile {
    version: u32,
    entries: Vec<RememberedLaunchSystem>,
}

/// `(size, modified_ns)` of a regular file, following no special files.
fn observe(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((metadata.len(), u64::try_from(modified).ok()?))
}

impl RememberedLaunchSystem {
    /// An entry for `path` as it is now, or `None` when it is not a readable
    /// regular file or `platform_id` is not a known system.
    pub fn for_current_file(path: &Path, platform_id: &str) -> Option<Self> {
        let platform_id = crate::canonical_platform_for_alias(platform_id)?;
        if !path.is_absolute() {
            return None;
        }
        let (size, modified_ns) = observe(path)?;
        Some(Self {
            path: path.to_path_buf(),
            size,
            modified_ns,
            platform_id: platform_id.into(),
        })
    }

    /// True only while the file still has the size and time it was chosen at.
    pub fn matches_unchanged(&self, path: &Path) -> bool {
        self.path == path && observe(path) == Some((self.size, self.modified_ns))
    }

    fn validate(&self) -> Result<(), String> {
        if !self.path.is_absolute()
            || crate::canonical_platform_for_alias(&self.platform_id)
                != Some(self.platform_id.as_str())
        {
            return Err(
                "A remembered launch choice needs an absolute file path and a known system.".into(),
            );
        }
        Ok(())
    }
}

pub fn default_remembered_launch_systems_path() -> Result<PathBuf, String> {
    crate::app_dirs::config_path(REMEMBERED_LAUNCH_SYSTEMS_FILE).map_err(|error| error.to_string())
}

/// A missing file means nothing is remembered. Anything else that is not a
/// small, regular, well-formed file is an error (and nothing is applied).
pub fn load_remembered_launch_systems_at(
    path: &Path,
) -> Result<Vec<RememberedLaunchSystem>, String> {
    let file = match super::process_spawn::open_regular_file_nonblocking(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{} is too large", path.display()));
    }
    let parsed: RememberedFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not valid: {error}", path.display()))?;
    if parsed.version != FORMAT_VERSION {
        return Err(format!(
            "{} uses unsupported version {}",
            path.display(),
            parsed.version
        ));
    }
    for (index, entry) in parsed.entries.iter().enumerate() {
        entry.validate()?;
        if parsed.entries[..index]
            .iter()
            .any(|earlier| earlier.path == entry.path)
        {
            return Err(format!(
                "{} remembers more than one system for the same file",
                path.display()
            ));
        }
    }
    Ok(parsed.entries)
}

/// Saves `entry`, replacing only an earlier entry for the same file. Refuses
/// to replace a file that cannot be read.
pub fn remember_launch_system_at(
    path: &Path,
    entry: &RememberedLaunchSystem,
) -> Result<(), String> {
    entry.validate()?;
    let _writer = write_lock();
    let mut entries = load_remembered_launch_systems_at(path)?;
    entries.retain(|existing| existing.path != entry.path);
    entries.push(entry.clone());
    write_entries(path, entries)
}

/// Forgets the remembered choice for exactly `file`.
pub fn forget_launch_system_at(path: &Path, file: &Path) -> Result<(), String> {
    let _writer = write_lock();
    let mut entries = load_remembered_launch_systems_at(path)?;
    let before = entries.len();
    entries.retain(|existing| existing.path != file);
    if entries.len() == before {
        return Ok(());
    }
    write_entries(path, entries)
}

fn write_entries(path: &Path, mut entries: Vec<RememberedLaunchSystem>) -> Result<(), String> {
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let mut text = serde_json::to_string_pretty(&RememberedFile {
        version: FORMAT_VERSION,
        entries,
    })
    .map_err(|error| error.to_string())?;
    text.push('\n');
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "{} would exceed its size limit; forget some remembered launch choices first",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::atomic_write_text(path, &text).map_err(|error| error.to_string())?;
    invalidate_cache();
    Ok(())
}

/// Serialises read-modify-write within this process, so two saves (for
/// example from the UI and a background task) never drop each other's entry.
fn write_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

type Snapshot = (PathBuf, Option<(u64, u64)>, Vec<RememberedLaunchSystem>);

fn cache() -> &'static Mutex<Option<Snapshot>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<Snapshot>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn invalidate_cache() {
    if let Ok(mut cached) = cache().lock() {
        *cached = None;
    }
}

/// The remembered system for `file`, read from the store at `store`, only
/// while `file` is unchanged. An unreadable store applies nothing. The store
/// is re-read whenever its own size or time changes.
pub fn remembered_launch_system_at(store: &Path, file: &Path) -> Option<&'static str> {
    let observed = observe(store);
    let mut cached = cache().lock().ok()?;
    let fresh = cached
        .as_ref()
        .is_some_and(|(cached_store, seen, _)| cached_store == store && *seen == observed);
    if !fresh {
        let entries = load_remembered_launch_systems_at(store).unwrap_or_default();
        *cached = Some((store.to_path_buf(), observed, entries));
    }
    let (_, _, entries) = cached.as_ref()?;
    let entry = entries.iter().find(|entry| entry.path == file)?;
    if !entry.matches_unchanged(file) {
        return None;
    }
    crate::canonical_platform_for_alias(&entry.platform_id)
}

/// [`remembered_launch_system_at`] for the default configuration directory.
pub fn remembered_launch_system(file: &Path) -> Option<&'static str> {
    remembered_launch_system_at(&default_remembered_launch_systems_path().ok()?, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_remembered_choice_applies_only_to_the_unchanged_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir
            .path()
            .join("config")
            .join(REMEMBERED_LAUNCH_SYSTEMS_FILE);
        let file = game(&dir, "Jet Set Radio.chd", b"MComprHD-not-really");
        assert_eq!(remembered_launch_system_at(&store, &file), None);

        let entry = RememberedLaunchSystem::for_current_file(&file, "Dreamcast").unwrap();
        remember_launch_system_at(&store, &entry).unwrap();
        assert_eq!(
            remembered_launch_system_at(&store, &file),
            Some("Dreamcast")
        );
        let other = game(&dir, "Other.chd", b"x");
        assert_eq!(remembered_launch_system_at(&store, &other), None);

        // Different bytes (size changes) make the entry inert, not reassigned.
        std::fs::write(&file, b"MComprHD-replaced-with-a-different-file").unwrap();
        assert_eq!(remembered_launch_system_at(&store, &file), None);
    }

    #[test]
    fn forgetting_removes_only_that_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(REMEMBERED_LAUNCH_SYSTEMS_FILE);
        let a = game(&dir, "a.chd", b"a");
        let b = game(&dir, "b.chd", b"b");
        for (file, system) in [(&a, "Dreamcast"), (&b, "PC Engine CD")] {
            let entry = RememberedLaunchSystem::for_current_file(file, system).unwrap();
            remember_launch_system_at(&store, &entry).unwrap();
        }
        // Re-remembering replaces, never duplicates.
        let entry = RememberedLaunchSystem::for_current_file(&a, "Philips CD-i").unwrap();
        remember_launch_system_at(&store, &entry).unwrap();
        assert_eq!(load_remembered_launch_systems_at(&store).unwrap().len(), 2);
        forget_launch_system_at(&store, &a).unwrap();
        assert_eq!(remembered_launch_system_at(&store, &a), None);
        assert_eq!(
            remembered_launch_system_at(&store, &b),
            Some("PC Engine CD")
        );
    }

    #[test]
    fn an_unreadable_or_unknown_store_applies_nothing_and_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(REMEMBERED_LAUNCH_SYSTEMS_FILE);
        let file = game(&dir, "a.chd", b"a");
        std::fs::write(&store, b"{\"version\":2,\"entries\":[]}").unwrap();
        assert_eq!(remembered_launch_system_at(&store, &file), None);
        let entry = RememberedLaunchSystem::for_current_file(&file, "Dreamcast").unwrap();
        assert!(remember_launch_system_at(&store, &entry).is_err());
        assert_eq!(
            std::fs::read(&store).unwrap(),
            b"{\"version\":2,\"entries\":[]}"
        );
    }

    #[test]
    fn unknown_systems_and_relative_paths_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = game(&dir, "a.chd", b"a");
        assert!(RememberedLaunchSystem::for_current_file(&file, "Not A System").is_none());
        assert!(
            RememberedLaunchSystem::for_current_file(Path::new("a.chd"), "Dreamcast").is_none()
        );
    }

    /// Older EmuWiz builds read `launch_preferences.json` strictly
    /// (`deny_unknown_fields`, version 1) and never this file. Remembering a
    /// launch choice must leave that file byte-for-byte unchanged.
    #[test]
    fn remembering_never_touches_the_emulator_preferences_older_builds_read() {
        use crate::launch::launch_choice::{
            LAUNCH_PREFERENCES_FILE, LaunchPreference, load_launch_preferences_at,
            set_launch_preference_at,
        };
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let preferences = config.join(LAUNCH_PREFERENCES_FILE);
        set_launch_preference_at(
            &preferences,
            &LaunchPreference::system("PS2", "standalone/pcsx2/x"),
        )
        .unwrap();
        let before = std::fs::read(&preferences).unwrap();

        let file = game(&dir, "Avenger (Japan).chd", b"chd");
        let store = config.join(REMEMBERED_LAUNCH_SYSTEMS_FILE);
        let entry = RememberedLaunchSystem::for_current_file(&file, "PC Engine CD").unwrap();
        remember_launch_system_at(&store, &entry).unwrap();
        forget_launch_system_at(&store, &file).unwrap();
        remember_launch_system_at(&store, &entry).unwrap();

        assert_eq!(std::fs::read(&preferences).unwrap(), before);
        assert_eq!(load_launch_preferences_at(&preferences).unwrap().len(), 1);
        assert_ne!(REMEMBERED_LAUNCH_SYSTEMS_FILE, LAUNCH_PREFERENCES_FILE);
        // The store is self-describing and versioned for later builds.
        let text = std::fs::read_to_string(&store).unwrap();
        assert!(text.contains("\"version\": 1"), "{text}");
    }
}
