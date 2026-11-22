//! Read-only save discovery.
//!
//! Looks only in the save folders of emulators EmuWiz knows, never across
//! the whole disk. Every file is opened read-only without following a link.
//! The work is bounded (file count, bytes hashed), can be cancelled between
//! files, and reuses earlier results for files that have not changed.
//!
//! What ties a save to a game is decided here and nowhere else:
//!
//! | Save | Tie to a game |
//! |---|---|
//! | PS1 / PS2 card save | product code in the card's own entry: content-proven |
//! | GameCube `.gci` | game code in the file header: content-proven |
//! | PS3 / PSP save folder | `PARAM.SFO` inside the folder: content-proven |
//! | Cartridge battery save | none in the bytes. "Named by the emulator" only when RetroArch's own history lists exactly one game file of that name; otherwise a name hint |
//! | Save state | same as a battery save, and never portable |

use super::model::{
    AssociationStrength, DiscoveredSave, InUse, SaveFormat, SaveIdentity, stable_id,
};
use super::store::{ContainedSave, InUseObserver};
use super::{ContinuityError, sha256_hex};
use crate::emulator_environment::HostReadOnlyFilesystem;
use crate::emulator_environment::retroarch::content_history::read_content_history_for_config;
use crate::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityKind, IdentityProvenance, IdentityStatus,
};
use crate::memory_card_manager::discovery::find_cards_in_standard_folders;
use crate::memory_card_manager::{
    CardFormat, CardHealth, CardPlatform, read_card_with_fingerprints,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Largest single file discovery will hash.
pub const MAX_HASH_BYTES: u64 = 128 * 1024 * 1024;
pub const DEFAULT_MAX_FILES: usize = 20_000;
const MAX_FOLDER_SAVE_FILES: usize = 512;
pub(crate) const CARD_HAS_WARNINGS: &str =
    "this card has warnings, so saves are not exported from it";
const GCI_HEADER: usize = 0x40;
const GCI_BLOCK: usize = 0x2000;

const BATTERY_SUFFIXES: [&str; 8] = ["srm", "sav", "eep", "fla", "mpk", "ram", "bkr", "ngf"];

/// Where to look. Built from the real profile, or from any folder standing
/// in for a home directory (tests, a second device profile, a scratch copy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRoots {
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
}

impl DiscoveryRoots {
    /// The standard layout under one home folder.
    pub fn under_home(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            config: home.join(".config"),
            data: home.join(".local/share"),
        }
    }

    /// The current user's profile, honouring `XDG_CONFIG_HOME` / `XDG_DATA_HOME`.
    pub fn current() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").filter(|v| !v.is_empty())?);
        let or = |var: &str, fallback: PathBuf| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or(fallback)
        };
        Some(Self {
            config: or("XDG_CONFIG_HOME", home.join(".config")),
            data: or("XDG_DATA_HOME", home.join(".local/share")),
            home,
        })
    }
}

/// What discovery remembers about a file so it need not read it again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryCache {
    pub format_version: u32,
    entries: BTreeMap<PathBuf, CacheEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CacheEntry {
    /// Size, modification time, change time, device and inode of every file
    /// that went into the result. Any difference means "read it again".
    signature: String,
    saves: Vec<DiscoveredSave>,
}

impl DiscoveryCache {
    pub const FORMAT_VERSION: u32 = 1;

    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|cache| cache.format_version == Self::FORMAT_VERSION)
            .unwrap_or_default()
    }

    /// Writes the cache beside the store (work file, then rename).
    pub fn save(&self, path: &Path) -> Result<(), ContinuityError> {
        let mut cache = self.clone();
        cache.format_version = Self::FORMAT_VERSION;
        let text = serde_json::to_vec(&cache).map_err(|e| ContinuityError::Io(e.to_string()))?;
        super::write_state_file(path, &text)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Default)]
pub struct DiscoveryOptions<'a> {
    /// Stop after this many candidate files (0 = [`DEFAULT_MAX_FILES`]).
    pub max_files: usize,
    pub cancel: Option<&'a AtomicBool>,
    pub cache: Option<&'a mut DiscoveryCache>,
    /// Checks whether each save's emulator is running. `None` reports
    /// [`InUse::Unknown`] for everything.
    pub observer: Option<&'a dyn InUseObserver>,
    /// Look only where this emulator and profile ("native" / "flatpak")
    /// keep their saves; nothing else is examined or read.
    pub only: Option<(&'a str, &'a str)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiscoveryStats {
    pub files_examined: usize,
    pub files_read: usize,
    pub bytes_hashed: u64,
    pub reused_from_cache: usize,
    /// The file limit was reached; the list is incomplete.
    pub stopped_at_limit: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiscoveryReport {
    pub saves: Vec<DiscoveredSave>,
    /// Files that are in a save folder but were not listed, with the reason.
    pub skipped: Vec<String>,
    /// The path of every entry of `skipped`: a save file, folder or card
    /// that is there but whose content was not taken in.
    pub unread: Vec<PathBuf>,
    /// Places whose contents could not be listed or located at all (a
    /// folder that cannot be opened, a save folder that is a link, an
    /// emulator configuration that cannot be read). The list of saves is
    /// then not complete, and which saves are missing from it is not known.
    pub incomplete: Vec<String>,
    /// The folders that were listed for saves whose location is a setting
    /// (RetroArch's save and state folders).
    pub places: Vec<PathBuf>,
    /// For a narrowed look (a launch): every file in the started emulator's
    /// memory-card folders, and every card-shaped file among its other
    /// saves, with the fingerprint of its whole content. A card is thereby
    /// seen to change even when it holds no save that can be listed, or
    /// cannot be taken apart at all.
    pub whole_files: Vec<(PathBuf, String)>,
    pub stats: DiscoveryStats,
    /// Formats discovery does not look for at all.
    pub not_covered: Vec<&'static str>,
}

/// Emulators for which EmuWiz reports a verified safe point. Each one's
/// process rules were checked against a real launch of the installed
/// emulator (`scripts/qa/emulator_safe_point_probe.py`), not only against
/// synthetic process lists.
pub const SAFE_POINT_EMULATORS: [&str; 5] =
    ["PCSX2", "RetroArch", "DuckStation", "RPCS3", "PPSSPP"];

/// Emulators whose process rules exist and pass synthetic tests, but have
/// never been checked against a real installation. They are treated exactly
/// like an unknown emulator: never a verified safe point.
pub const SAFE_POINT_NOT_VERIFIED: [(&str, &str); 1] = [(
    "Dolphin",
    "standalone Dolphin was not installed when this was written, so how its processes and save writes really behave has not been observed",
)];

pub fn safe_point_supported(emulator: &str) -> bool {
    SAFE_POINT_EMULATORS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(emulator))
}

/// The production in-use check: the reviewed process scan and kernel lease
/// used by save snapshots and the PS2 card restore.
///
/// - **Verified safe** ([`InUse::NotDetected`]) needs both: no process of the
///   emulator in any form (name, executable, command line, Flatpak app id or
///   scope, launcher), and no program holding the file. With a kernel lease
///   the second part is answered for every process on the machine, including
///   ones that cannot be inspected; without one, a single uninspectable
///   process makes the answer unknown.
/// - **Active**: an emulator process exists ([`InUse::EmulatorRunning`]), or
///   some program has the file open ([`InUse::OpenByAProgram`]).
/// - **Unknown**: anything else, including an emulator this scan has no
///   identity for.
pub struct ProcessObserver {
    proc_root: PathBuf,
    /// Tests only: also answer for emulators in [`SAFE_POINT_NOT_VERIFIED`].
    include_not_verified: bool,
}

impl ProcessObserver {
    pub fn new() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            include_not_verified: false,
        }
    }

    /// For tests: read processes from a stand-in for `/proc`.
    pub fn with_proc_root(proc_root: PathBuf) -> Self {
        Self {
            proc_root,
            include_not_verified: false,
        }
    }

    /// Synthetic tests of process rules that production does not yet trust.
    #[cfg(test)]
    pub(crate) fn synthetic_including_not_verified(proc_root: PathBuf) -> Self {
        Self {
            proc_root,
            include_not_verified: true,
        }
    }

    fn answers_for(&self, emulator: &str) -> bool {
        safe_point_supported(emulator)
            || (self.include_not_verified
                && SAFE_POINT_NOT_VERIFIED
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(emulator)))
    }

    fn scan(&self) -> crate::memory_card_inventory::restore_guard::ProcScanQuiescence {
        crate::memory_card_inventory::restore_guard::ProcScanQuiescence::with_root(
            self.proc_root.clone(),
        )
    }
}

impl Default for ProcessObserver {
    fn default() -> Self {
        Self::new()
    }
}

fn binding_for(
    emulator: &str,
    path: &Path,
) -> crate::save_snapshots::tree_restore::safety::DirectorySaveBinding {
    use crate::save_snapshots::tree_restore::safety::{DirectorySaveBinding, ResolvedSaveGame};
    if emulator.eq_ignore_ascii_case("pcsx2") {
        // Exactly the binding the PS2 card restore was reviewed with.
        return crate::memory_card_inventory::restore_guard::ps2_card_binding(path, "save");
    }
    DirectorySaveBinding {
        game: ResolvedSaveGame::Unique("save".to_string()),
        emulator: emulator.to_ascii_lowercase(),
        profile: path.to_string_lossy().into_owned(),
        artifact_type: crate::save_snapshots::SaveArtifactType::Sram,
    }
}

fn from_quiescence(
    state: crate::save_snapshots::tree_restore::safety::EmulatorQuiescence,
) -> InUse {
    use crate::save_snapshots::tree_restore::safety::EmulatorQuiescence;
    match state {
        EmulatorQuiescence::Closed => InUse::NotDetected,
        EmulatorQuiescence::Running => InUse::EmulatorRunning,
        EmulatorQuiescence::CardInUse => InUse::OpenByAProgram,
        EmulatorQuiescence::Unknown => InUse::Unknown,
    }
}

impl InUseObserver for ProcessObserver {
    fn observe(&self, emulator: &str, path: &Path) -> InUse {
        use crate::memory_card_inventory::restore_guard::observe_card_now;
        if !self.answers_for(emulator) {
            return InUse::Unknown;
        }
        from_quiescence(observe_card_now(
            &self.scan(),
            &binding_for(emulator, path),
            path,
        ))
    }

    fn guard<'a>(
        &'a self,
        emulator: &'a str,
        path: &'a Path,
    ) -> Box<dyn super::store::CopyGuard + 'a> {
        use crate::memory_card_inventory::restore_guard::{CardLeaseAttempt, CardWriteLease};
        let (lease, writers) = if self.answers_for(emulator) {
            match CardWriteLease::acquire(path, None) {
                CardLeaseAttempt::Held(lease) => (Some(lease), false),
                CardLeaseAttempt::Writers => (None, true),
                CardLeaseAttempt::Changed | CardLeaseAttempt::Unavailable => (None, false),
            }
        } else {
            (None, false)
        };
        Box::new(LeaseGuard {
            observer: self,
            emulator,
            path,
            lease,
            writers,
        })
    }
}

/// One save file, leased for the length of a copy where the kernel allows.
struct LeaseGuard<'a> {
    observer: &'a ProcessObserver,
    emulator: &'a str,
    path: &'a Path,
    lease: Option<crate::memory_card_inventory::restore_guard::CardWriteLease>,
    /// The lease was refused because a program already had the file open.
    writers: bool,
}

impl super::store::CopyGuard for LeaseGuard<'_> {
    fn state(&self) -> InUse {
        use crate::save_snapshots::tree_restore::safety::QuiescenceProvider;
        if !self.observer.answers_for(self.emulator) {
            return InUse::Unknown;
        }
        let binding = binding_for(self.emulator, self.path);
        let scan = self.observer.scan();
        match &self.lease {
            Some(lease) => {
                let state = from_quiescence(scan.observe_under_card_lease(&binding, lease));
                // A break at any time since the lease was taken means a
                // program opened the file for writing.
                if state != InUse::EmulatorRunning && !lease.intact() {
                    InUse::OpenByAProgram
                } else {
                    state
                }
            }
            None => match from_quiescence(scan.observe(&binding)) {
                InUse::EmulatorRunning => InUse::EmulatorRunning,
                _ if self.writers => InUse::OpenByAProgram,
                state => state,
            },
        }
    }

    fn kernel_watched(&self) -> bool {
        self.lease.is_some()
    }
}

struct Run<'a, 'b> {
    options: DiscoveryOptions<'a>,
    report: DiscoveryReport,
    limit: usize,
    _marker: std::marker::PhantomData<&'b ()>,
}

impl Run<'_, '_> {
    /// Whether `emulator`'s saves at `path` are being looked for at all.
    fn wanted(&self, emulator: &str, path: &Path) -> bool {
        self.options.only.is_none_or(|(only, profile)| {
            only.eq_ignore_ascii_case(emulator) && profile.eq_ignore_ascii_case(profile_of(path))
        })
    }

    fn cancelled(&self) -> bool {
        self.options
            .cancel
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Counts one candidate; false once the limit is reached.
    fn admit(&mut self) -> bool {
        if self.report.stats.files_examined >= self.limit {
            self.report.stats.stopped_at_limit = true;
            return false;
        }
        self.report.stats.files_examined += 1;
        true
    }

    fn in_use(&self, emulator: &str, path: &Path) -> InUse {
        match self.options.observer {
            Some(observer) => observer.observe(emulator, path),
            None => InUse::Unknown,
        }
    }

    /// Returns cached saves for `key` when `signature` still matches.
    fn cached(&mut self, key: &Path, signature: &str) -> Option<Vec<DiscoveredSave>> {
        let saves = self
            .options
            .cache
            .as_ref()?
            .entries
            .get(key)
            .filter(|entry| entry.signature == signature)?
            .saves
            .clone();
        self.report.stats.reused_from_cache += 1;
        Some(saves)
    }

    fn remember(&mut self, key: &Path, signature: String, saves: &[DiscoveredSave]) {
        if let Some(cache) = self.options.cache.as_mut() {
            cache.entries.insert(
                key.to_path_buf(),
                CacheEntry {
                    signature,
                    saves: saves.to_vec(),
                },
            );
        }
    }
}

fn profile_of(path: &Path) -> &'static str {
    if path.components().any(|part| part.as_os_str() == ".var") {
        "flatpak"
    } else {
        "native"
    }
}

fn signature(meta: &fs::Metadata) -> String {
    format!(
        "{}:{}.{}:{}.{}:{}:{}",
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
        meta.dev(),
        meta.ino()
    )
}

/// Reads a regular file without following a link, up to `MAX_HASH_BYTES`.
fn read_plain(path: &Path) -> Result<(Vec<u8>, fs::Metadata), String> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                "is a link, not a file".to_string()
            } else {
                format!("cannot be opened ({error})")
            }
        })?;
    let meta = file
        .metadata()
        .map_err(|error| format!("cannot be checked ({error})"))?;
    if !meta.is_file() {
        return Err("is not a regular file".into());
    }
    if meta.len() > MAX_HASH_BYTES {
        return Err(format!("is larger than {MAX_HASH_BYTES} bytes"));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&mut file)
        .take(MAX_HASH_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot be read ({error})"))?;
    Ok((bytes, meta))
}

fn evidence(
    kind: IdentityKind,
    value: &str,
    confidence: IdentityConfidence,
    path: &Path,
    method: &str,
    diagnostic: &str,
) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        // A save says which game wrote it; it is not a verified game file.
        status: IdentityStatus::Candidate,
        value: Some(value.to_string()),
        confidence,
        provenance: IdentityProvenance {
            archive_path: path.to_path_buf(),
            member_path: None,
            member_index: None,
            method: method.to_string(),
        },
        diagnostic: diagnostic.to_string(),
    }
}

/// The entries of `dir`, sorted. A folder that is not there has none; one
/// that is there but cannot be listed is noted in `incomplete`.
fn listed(dir: &Path, incomplete: &mut Vec<String>) -> Vec<PathBuf> {
    use std::io::ErrorKind;
    let mut children = Vec::new();
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => children.push(entry.path()),
                    Err(error) => {
                        incomplete.push(format!("{} could not be listed ({error})", dir.display()))
                    }
                }
            }
        }
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
        Err(error) => incomplete.push(format!("{} could not be opened ({error})", dir.display())),
    }
    children.sort();
    children
}

fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Finds saves under `roots`. Never writes, except to `options.cache` in memory.
pub fn discover(
    roots: &DiscoveryRoots,
    options: DiscoveryOptions<'_>,
) -> Result<DiscoveryReport, ContinuityError> {
    let limit = if options.max_files == 0 {
        DEFAULT_MAX_FILES
    } else {
        options.max_files
    };
    let mut run = Run {
        options,
        report: DiscoveryReport::default(),
        limit,
        _marker: std::marker::PhantomData,
    };
    run.report.not_covered = vec![
        "Wii saves (NAND title data)",
        "GameCube raw memory-card images (.raw / .gcp): only .gci files are read",
        "Standalone emulators' cartridge saves outside RetroArch (mGBA, melonDS, ...)",
        "Save states of standalone emulators",
        "Xbox, Dreamcast VMU, Saturn, Switch and Vita saves",
    ];
    let flatpak = |app: &str, rest: &str| roots.home.join(".var/app").join(app).join(rest);

    cards(&mut run, roots)?;
    if run.options.only.is_some() {
        for (folder, emulator) in [
            (roots.config.join("PCSX2/memcards"), "PCSX2"),
            (flatpak("net.pcsx2.PCSX2", "config/PCSX2/memcards"), "PCSX2"),
            (roots.data.join("duckstation/memcards"), "DuckStation"),
            (roots.config.join("duckstation/memcards"), "DuckStation"),
            (
                flatpak("org.duckstation.DuckStation", "data/duckstation/memcards"),
                "DuckStation",
            ),
            (
                flatpak("org.duckstation.DuckStation", "config/duckstation/memcards"),
                "DuckStation",
            ),
        ] {
            if run.wanted(emulator, &folder) {
                card_folder_whole(&mut run, &folder)?;
            }
        }
    }
    for (base, emulator) in [
        (roots.data.join("dolphin-emu/GC"), "Dolphin"),
        (
            flatpak("org.DolphinEmu.dolphin-emu", "data/dolphin-emu/GC"),
            "Dolphin",
        ),
    ] {
        if run.wanted(emulator, &base) {
            gamecube(&mut run, &base, emulator)?;
        }
    }
    for base in [
        roots.config.join("rpcs3/dev_hdd0/home"),
        flatpak("net.rpcs3.RPCS3", "config/rpcs3/dev_hdd0/home"),
    ] {
        if !run.wanted("RPCS3", &base) {
            continue;
        }
        for user in listed(&base, &mut run.report.incomplete) {
            sfo_folders(
                &mut run,
                &user.join("savedata"),
                "RPCS3",
                SaveFormat::Ps3Savedata,
            )?;
        }
    }
    for base in [
        roots.config.join("ppsspp/PSP/SAVEDATA"),
        flatpak("org.ppsspp.PPSSPP", "config/ppsspp/PSP/SAVEDATA"),
    ] {
        if run.wanted("PPSSPP", &base) {
            sfo_folders(&mut run, &base, "PPSSPP", SaveFormat::PspSavedata)?;
        }
    }
    for config in [
        roots.config.join("retroarch"),
        flatpak("org.libretro.RetroArch", "config/retroarch"),
    ] {
        if run.wanted("RetroArch", &config) {
            retroarch(&mut run, &config, &roots.home)?;
        }
    }
    run.report.saves.sort_by(|a, b| {
        (&a.emulator, &a.profile, &a.source_path, &a.entry_name).cmp(&(
            &b.emulator,
            &b.profile,
            &b.source_path,
            &b.entry_name,
        ))
    });
    Ok(run.report)
}

/// Takes one file whole: its content's fingerprint, or a note that it could
/// not be read.
fn whole_file(run: &mut Run<'_, '_>, path: &Path) {
    match read_plain(path) {
        Ok((bytes, _)) => {
            run.report.stats.files_read += 1;
            run.report.stats.bytes_hashed += bytes.len() as u64;
            run.report
                .whole_files
                .push((path.to_path_buf(), sha256_hex(&bytes)));
        }
        Err(why) => {
            run.report.skipped.push(format!("{} {why}", path.display()));
            run.report.unread.push(path.to_path_buf());
        }
    }
}

/// Every entry of a memory-card folder, whole. A folder that cannot be
/// listed makes the look incomplete; a file that cannot be read, a link and
/// a folder card (whose content is not fingerprinted) are noted as unread.
fn card_folder_whole(run: &mut Run<'_, '_>, folder: &Path) -> Result<(), ContinuityError> {
    for path in listed(folder, &mut run.report.incomplete) {
        check(run)?;
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => whole_file(run, &path),
            Ok(meta) if meta.is_dir() => {
                if crate::memory_card_manager::discovery::sniff_format(&path)
                    == CardFormat::Pcsx2Folder
                {
                    run.report.unread.push(path);
                }
            }
            _ => {
                run.report
                    .skipped
                    .push(format!("{} is not a plain file", path.display()));
                run.report.unread.push(path);
            }
        }
    }
    Ok(())
}

fn check(run: &Run<'_, '_>) -> Result<(), ContinuityError> {
    if run.cancelled() {
        Err(ContinuityError::Cancelled)
    } else {
        Ok(())
    }
}

fn cards(run: &mut Run<'_, '_>, roots: &DiscoveryRoots) -> Result<(), ContinuityError> {
    let mut found = find_cards_in_standard_folders(&roots.home, &roots.config, &roots.data);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    for card in found {
        check(run)?;
        if card.format == CardFormat::NotACard {
            continue;
        }
        if !run.admit() {
            break;
        }
        let emulator = card
            .emulator
            .clone()
            .unwrap_or_else(|| "Unknown".to_string());
        if !run.wanted(&emulator, &card.path) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&card.path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            run.report
                .skipped
                .push(format!("{} is a link, not a card", card.path.display()));
            run.report.unread.push(card.path.clone());
            continue;
        }
        let sig = signature(&meta);
        let in_use = run.in_use(&emulator, &card.path);
        if !meta.is_dir() {
            if let Some(mut saves) = run.cached(&card.path, &sig) {
                saves.iter_mut().for_each(|save| save.in_use = in_use);
                run.report.saves.extend(saves);
                continue;
            }
        }
        let (report, fingerprints) = match read_card_with_fingerprints(&card.path) {
            Ok(read) => read,
            Err(error) => {
                run.report
                    .skipped
                    .push(format!("{}: {error}", card.path.display()));
                run.report.unread.push(card.path.clone());
                continue;
            }
        };
        run.report.stats.files_read += 1;
        run.report.stats.bytes_hashed += report.size_bytes;
        let (format, platform, kind) = match report.format.platform() {
            Some(CardPlatform::PlayStation) => (
                SaveFormat::Ps1CardSave,
                "PlayStation",
                IdentityKind::Ps1Serial,
            ),
            Some(CardPlatform::PlayStation2) => (
                SaveFormat::Ps2CardSave,
                "PlayStation 2",
                IdentityKind::Ps2Serial,
            ),
            None => continue,
        };
        let mut saves = Vec::new();
        for save in &report.saves {
            let mut identity = match &save.product_code {
                Some(code) => SaveIdentity {
                    strength: AssociationStrength::ContentProven,
                    product_code: Some(code.clone()),
                    region: save.region.map(str::to_string),
                    title: save.title.clone(),
                    evidence: vec![evidence(
                        kind,
                        code,
                        IdentityConfidence::StructuredMetadata,
                        &card.path,
                        "memory-card directory entry",
                        &format!(
                            "product code in the name the game gave its save ({})",
                            save.name
                        ),
                    )],
                    ambiguity: Vec::new(),
                },
                None => SaveIdentity {
                    title: save.title.clone(),
                    ..SaveIdentity::none(
                        "the save's name on the card does not contain a product code",
                    )
                },
            };
            let mut integrity = vec![format!(
                "card format decided from content: {}",
                report.format.label()
            )];
            match report.readers_agree {
                Some(true) => {
                    integrity.push("two independent card readers list the same saves".into())
                }
                Some(false) => {
                    integrity.push("the two card readers DISAGREE about this card".into());
                    identity
                        .ambiguity
                        .push("the card did not read cleanly".into());
                }
                None => {}
            }
            let fingerprint = fingerprints.get(&save.name).cloned();
            if fingerprint.is_some() {
                integrity.push("SHA-256 taken over this save's own data on the card".into());
            }
            let mut unsupported = report.unsupported.clone();
            if report.health != CardHealth::Healthy {
                unsupported.push(CARD_HAS_WARNINGS.to_string());
            }
            saves.push(DiscoveredSave {
                id: stable_id(&card.path, Some(&save.name)),
                source_path: card.path.clone(),
                entry_name: Some(save.name.clone()),
                emulator: emulator.clone(),
                profile: profile_of(&card.path).to_string(),
                emulator_version: None,
                platform: Some(platform.to_string()),
                format,
                artifact_type: format.artifact_type(),
                size_bytes: save.size_bytes,
                content_sha256: fingerprint,
                container_sha256: report.sha256.clone(),
                modified_unix: Some(meta.mtime()),
                internal_modified: save.modified.clone(),
                identity,
                in_use,
                integrity,
                notes: save.notes.clone(),
                unsupported,
            });
        }
        if !meta.is_dir() {
            run.remember(&card.path, sig, &saves);
        }
        run.report.saves.extend(saves);
    }
    Ok(())
}

/// The saves on one card as snapshot records, for a manifest.
pub fn contained_saves(saves: &[DiscoveredSave], card: &Path) -> Vec<ContainedSave> {
    saves
        .iter()
        .filter(|save| save.source_path == card)
        .filter_map(|save| {
            let name = save.entry_name.clone()?;
            let prefix = match save.format {
                SaveFormat::Ps1CardSave => "ps1",
                SaveFormat::Ps2CardSave => "ps2",
                _ => return None,
            };
            Some(ContainedSave {
                key: format!("{prefix}:{name}"),
                name,
                product_code: save.identity.product_code.clone(),
                title: save.identity.title.clone(),
                fingerprint: save.content_sha256.clone()?,
            })
        })
        .collect()
}

/// A parsed GameCube `.gci` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GciHeader {
    /// Four-character game code plus two-character maker code, e.g. `GEDE01`.
    pub game_id: String,
    pub region: Option<&'static str>,
    pub internal_name: String,
    pub blocks: u16,
}

/// Reads a `.gci` header. `None` unless the codes are printable, the block
/// count is in range and the file length is exactly header + blocks.
pub fn parse_gci(bytes: &[u8]) -> Option<GciHeader> {
    let header = bytes.get(..GCI_HEADER)?;
    let code = &header[..6];
    if !code
        .iter()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return None;
    }
    let blocks = u16::from_be_bytes([header[0x38], header[0x39]]);
    if blocks == 0 || blocks > 2043 || bytes.len() != GCI_HEADER + usize::from(blocks) * GCI_BLOCK {
        return None;
    }
    let name = &header[0x08..0x28];
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    let region = match code[3] {
        b'E' => Some("USA"),
        b'P' => Some("Europe"),
        b'J' => Some("Japan"),
        _ => None,
    };
    Some(GciHeader {
        game_id: String::from_utf8_lossy(code).into_owned(),
        region,
        internal_name: String::from_utf8_lossy(&name[..end]).into_owned(),
        blocks,
    })
}

fn gamecube(run: &mut Run<'_, '_>, base: &Path, emulator: &str) -> Result<(), ContinuityError> {
    for region_dir in listed(base, &mut run.report.incomplete) {
        for card_dir in listed(&region_dir, &mut run.report.incomplete)
            .into_iter()
            .filter(|p| real_dir(p))
        {
            for path in listed(&card_dir, &mut run.report.incomplete) {
                check(run)?;
                if path
                    .extension()
                    .is_none_or(|ext| !ext.eq_ignore_ascii_case("gci"))
                {
                    continue;
                }
                if !run.admit() {
                    return Ok(());
                }
                let Ok(meta) = fs::symlink_metadata(&path) else {
                    continue;
                };
                let sig = signature(&meta);
                let in_use = run.in_use(emulator, &path);
                if let Some(mut saves) = run.cached(&path, &sig) {
                    saves.iter_mut().for_each(|save| save.in_use = in_use);
                    run.report.saves.extend(saves);
                    continue;
                }
                let (bytes, meta) = match read_plain(&path) {
                    Ok(read) => read,
                    Err(why) => {
                        run.report.skipped.push(format!("{} {why}", path.display()));
                        run.report.unread.push(path.clone());
                        continue;
                    }
                };
                run.report.stats.files_read += 1;
                run.report.stats.bytes_hashed += bytes.len() as u64;
                let Some(header) = parse_gci(&bytes) else {
                    run.report.skipped.push(format!(
                        "{} is named .gci but is not a complete GameCube save file",
                        path.display()
                    ));
                    run.report.unread.push(path.clone());
                    continue;
                };
                let folder_region = region_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                let mut ambiguity = Vec::new();
                let expected = match header.region {
                    Some("USA") => Some("USA"),
                    Some("Europe") => Some("EUR"),
                    Some("Japan") => Some("JAP"),
                    _ => None,
                };
                if let (Some(expected), Some(actual)) = (expected, folder_region.as_deref()) {
                    if expected != actual {
                        ambiguity.push(format!(
                            "the save is for a {} game but sits in Dolphin's {actual} folder",
                            header.region.unwrap_or("?")
                        ));
                    }
                }
                let save = DiscoveredSave {
                    id: stable_id(&path, None),
                    source_path: path.clone(),
                    entry_name: Some(header.internal_name.clone()),
                    emulator: emulator.to_string(),
                    profile: profile_of(&path).to_string(),
                    emulator_version: None,
                    platform: Some("GameCube".to_string()),
                    format: SaveFormat::GameCubeGci,
                    artifact_type: SaveFormat::GameCubeGci.artifact_type(),
                    size_bytes: bytes.len() as u64,
                    content_sha256: Some(sha256_hex(&bytes)),
                    container_sha256: None,
                    modified_unix: Some(meta.mtime()),
                    internal_modified: None,
                    identity: SaveIdentity {
                        strength: AssociationStrength::ContentProven,
                        product_code: Some(header.game_id.clone()),
                        region: header.region.map(str::to_string),
                        title: None,
                        evidence: vec![evidence(
                            IdentityKind::DolphinGameId,
                            &header.game_id,
                            IdentityConfidence::StructuredMetadata,
                            &path,
                            "GameCube save header",
                            "game and maker code in the first six bytes of the save file",
                        )],
                        ambiguity,
                    },
                    in_use,
                    integrity: vec![
                        format!("file length is exactly the header plus {} blocks", header.blocks),
                        "SHA-256 taken over the whole file".to_string(),
                        "the save's own internal checksums were NOT checked (they are game-specific)".to_string(),
                    ],
                    notes: Vec::new(),
                    unsupported: vec!["EmuWiz cannot place a .gci onto a raw GameCube memory-card image".to_string()],
                };
                run.remember(&path, sig, std::slice::from_ref(&save));
                run.report.saves.push(save);
            }
        }
    }
    Ok(())
}

fn product_code_from(directory: &str) -> Option<String> {
    let head = directory.get(..9)?;
    let (letters, digits) = head.split_at(4);
    (letters.bytes().all(|b| b.is_ascii_uppercase()) && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| head.to_string())
}

fn sfo_folders(
    run: &mut Run<'_, '_>,
    base: &Path,
    emulator: &str,
    format: SaveFormat,
) -> Result<(), ContinuityError> {
    for folder in listed(base, &mut run.report.incomplete)
        .into_iter()
        .filter(|p| real_dir(p))
    {
        check(run)?;
        let sfo_path = folder.join("PARAM.SFO");
        if fs::symlink_metadata(&sfo_path).is_err() {
            continue;
        }
        if !run.admit() {
            return Ok(());
        }
        // The folder's signature covers every file directly inside it.
        let members: Vec<PathBuf> = listed(&folder, &mut run.report.incomplete);
        if members.len() > MAX_FOLDER_SAVE_FILES {
            run.report.skipped.push(format!(
                "{} holds more than {MAX_FOLDER_SAVE_FILES} files",
                folder.display()
            ));
            run.report.unread.push(folder.clone());
            continue;
        }
        let mut sig = String::new();
        let mut newest = 0i64;
        let mut all_plain = true;
        for member in &members {
            match fs::symlink_metadata(member) {
                Ok(meta) if meta.is_file() => {
                    newest = newest.max(meta.mtime());
                    sig.push_str(&format!(
                        "{}={};",
                        member.file_name().unwrap_or_default().to_string_lossy(),
                        signature(&meta)
                    ));
                }
                _ => all_plain = false,
            }
        }
        let in_use = run.in_use(emulator, &sfo_path);
        if let Some(mut saves) = run.cached(&folder, &sig) {
            saves.iter_mut().for_each(|save| save.in_use = in_use);
            run.report.saves.extend(saves);
            continue;
        }
        let folder_name = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut text = Vec::new();
        let mut total = 0u64;
        let mut sfo = None;
        let mut readable = all_plain;
        for member in &members {
            match read_plain(member) {
                Ok((bytes, _)) => {
                    total += bytes.len() as u64;
                    let name = member
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if name == "PARAM.SFO" {
                        sfo = crate::param_sfo::parse_param_sfo(&bytes);
                    }
                    // EmuWiz's own record in a recovered folder is not part
                    // of the save, here or in a snapshot of it.
                    if name == super::store::RECOVERY_RECEIPT_NAME {
                        continue;
                    }
                    text.extend_from_slice(name.as_bytes());
                    text.push(0);
                    text.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                    text.extend_from_slice(sha256_hex(&bytes).as_bytes());
                }
                Err(_) => readable = false,
            }
        }
        run.report.stats.files_read += members.len();
        run.report.stats.bytes_hashed += total;
        let Some(sfo) = sfo else {
            run.report
                .skipped
                .push(format!("{}: PARAM.SFO could not be read", folder.display()));
            run.report.unread.push(folder.clone());
            continue;
        };
        let declared = sfo.get_text("SAVEDATA_DIRECTORY").map(str::to_string);
        let title = sfo.get_text("TITLE").map(str::to_string);
        let mut ambiguity = Vec::new();
        let code = declared.as_deref().and_then(product_code_from);
        if declared.as_deref() != Some(folder_name.as_str()) {
            ambiguity.push(format!(
                "the folder is named {folder_name} but its PARAM.SFO says {}",
                declared.as_deref().unwrap_or("nothing")
            ));
        }
        let kind = if format == SaveFormat::Ps3Savedata {
            IdentityKind::Ps3TitleId
        } else {
            IdentityKind::PspDiscId
        };
        let identity = match &code {
            Some(code) => SaveIdentity {
                strength: AssociationStrength::ContentProven,
                product_code: Some(code.clone()),
                region: None,
                title,
                evidence: vec![evidence(
                    kind,
                    code,
                    IdentityConfidence::StructuredMetadata,
                    &sfo_path,
                    "PARAM.SFO SAVEDATA_DIRECTORY",
                    "product code at the start of the save folder name recorded inside PARAM.SFO",
                )],
                ambiguity,
            },
            None => SaveIdentity {
                title,
                ..SaveIdentity::none("PARAM.SFO does not record a product code")
            },
        };
        let mut integrity = vec!["PARAM.SFO parsed".to_string()];
        if readable {
            integrity.push("SHA-256 taken over every file in the folder, by name".to_string());
        } else {
            integrity.push(
                "some entries in the folder are not plain files; no content checksum".to_string(),
            );
        }
        integrity.push("the console's own save signature (PARAM.PFD) was NOT verified".to_string());
        let save = DiscoveredSave {
            id: stable_id(&folder, None),
            source_path: folder.clone(),
            entry_name: Some(folder_name),
            emulator: emulator.to_string(),
            profile: profile_of(&folder).to_string(),
            emulator_version: None,
            platform: Some(
                if format == SaveFormat::Ps3Savedata {
                    "PlayStation 3"
                } else {
                    "PSP"
                }
                .to_string(),
            ),
            format,
            artifact_type: format.artifact_type(),
            size_bytes: total,
            content_sha256: readable.then(|| sha256_hex(&text)),
            container_sha256: None,
            modified_unix: Some(newest),
            internal_modified: None,
            identity,
            in_use,
            integrity,
            notes: Vec::new(),
            unsupported: vec!["EmuWiz does not convert or re-sign this kind of save".to_string()],
        };
        run.remember(&folder, sig, std::slice::from_ref(&save));
        run.report.saves.push(save);
    }
    Ok(())
}

fn configured_dir(config_text: &str, key: &str, home: &Path, fallback: PathBuf) -> PathBuf {
    for line in config_text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = value.trim().trim_matches('"');
        if value.is_empty() || value == "default" {
            break;
        }
        return match value.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(value),
        };
    }
    fallback
}

fn is_state_suffix(suffix: &str) -> bool {
    let rest = suffix.strip_prefix("state").unwrap_or("x");
    rest.is_empty() || rest == ".auto" || rest.bytes().all(|b| b.is_ascii_digit())
}

fn retroarch(run: &mut Run<'_, '_>, config_dir: &Path, home: &Path) -> Result<(), ContinuityError> {
    let config_file = config_dir.join("retroarch.cfg");
    let config_text = match fs::read_to_string(&config_file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            // Where this RetroArch keeps its saves cannot be told.
            run.report.incomplete.push(format!(
                "{} could not be read ({error})",
                config_file.display()
            ));
            return Ok(());
        }
    };
    let saves_dir = configured_dir(
        &config_text,
        "savefile_directory",
        home,
        config_dir.join("saves"),
    );
    let states_dir = configured_dir(
        &config_text,
        "savestate_directory",
        home,
        config_dir.join("states"),
    );
    // Game files RetroArch itself says it ran, by name without extension.
    let history = read_content_history_for_config(&HostReadOnlyFilesystem, &config_file, home);
    let mut played: BTreeMap<String, Vec<(String, Option<String>)>> = BTreeMap::new();
    for entry in &history.entries {
        let Some(raw) = entry.content_path.raw.as_deref() else {
            continue;
        };
        let leaf = raw.rsplit(['/', '#']).next().unwrap_or(raw);
        let stem = leaf.rsplit_once('.').map_or(leaf, |(stem, _)| stem);
        let list = played.entry(stem.to_string()).or_default();
        if !list.iter().any(|(path, _)| path == raw) {
            list.push((raw.to_string(), entry.core_path.clone()));
        }
    }
    let mut observed: BTreeMap<PathBuf, InUse> = BTreeMap::new();
    for (dir, states) in [(saves_dir, false), (states_dir, true)] {
        match fs::symlink_metadata(&dir) {
            // Only a folder that is there counts as a place searched: one
            // that appears or goes between two reads (a drive mounted or
            // not) makes them reads of different places.
            Ok(meta) if meta.is_dir() => run.report.places.push(dir.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Ok(_) => {
                // A link (or a file) where the folder should be: not followed.
                run.report.incomplete.push(format!(
                    "{} is not a plain folder; what it holds was not read",
                    dir.display()
                ));
                continue;
            }
            Err(error) => {
                run.report
                    .incomplete
                    .push(format!("{} could not be checked ({error})", dir.display()));
                continue;
            }
        }
        let mut files = Vec::new();
        for child in listed(&dir, &mut run.report.incomplete) {
            if real_dir(&child) {
                // One level down: RetroArch's "sort saves into folders by core".
                files.extend(
                    listed(&child, &mut run.report.incomplete)
                        .into_iter()
                        .map(|path| (path, true)),
                );
            } else {
                files.push((child, false));
            }
        }
        for (path, in_core_folder) in files {
            check(run)?;
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let Some((stem, suffix)) = name.split_once('.').and_then(|_| {
                // The stem is everything before the save suffix; game names contain dots.
                let lower = name.to_ascii_lowercase();
                if let Some(at) = lower.rfind(".state") {
                    return Some((name[..at].to_string(), lower[at + 1..].to_string()));
                }
                name.rsplit_once('.')
                    .map(|(stem, ext)| (stem.to_string(), ext.to_ascii_lowercase()))
            }) else {
                continue;
            };
            let format = if is_state_suffix(&suffix) {
                SaveFormat::SaveState
            } else if !states && BATTERY_SUFFIXES.contains(&suffix.as_str()) {
                SaveFormat::CartridgeBattery
            } else {
                // Not a name saves go by. At a launch, a memory card under
                // another name (.mcr, .mcd) is still taken whole.
                if !states
                    && run.options.only.is_some()
                    && crate::memory_card_manager::discovery::sniff_format(&path)
                        != CardFormat::NotACard
                {
                    whole_file(run, &path);
                }
                continue;
            };
            let Ok(link_meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if link_meta.is_dir() {
                continue;
            }
            if !run.admit() {
                return Ok(());
            }
            let sig = signature(&link_meta);
            // One process check per folder: every file in it shares the answer.
            let folder = path.parent().unwrap_or(&dir).to_path_buf();
            let in_use = match observed.get(&folder) {
                Some(state) => *state,
                None => {
                    let state = run.in_use("RetroArch", &path);
                    observed.insert(folder, state);
                    state
                }
            };
            if let Some(mut saves) = run.cached(&path, &sig) {
                saves.iter_mut().for_each(|save| save.in_use = in_use);
                run.report.saves.extend(saves);
                continue;
            }
            let (bytes, meta) = match read_plain(&path) {
                Ok(read) => read,
                Err(why) => {
                    run.report.skipped.push(format!("{} {why}", path.display()));
                    run.report.unread.push(path.clone());
                    continue;
                }
            };
            if crate::memory_card_manager::discovery::sniff_format(&path) != CardFormat::NotACard {
                // A PlayStation card kept as .srm. In a full listing it is
                // shown save by save where cards are looked for, and
                // reported as not read elsewhere (a save folder of the
                // person's own choosing). For a launch (a narrowed look,
                // which only compares content) the file is taken whole
                // below as well: an empty card holds no save to list, and
                // must still be seen to change.
                if run.options.only.is_none() {
                    if !run.report.saves.iter().any(|save| save.source_path == path) {
                        run.report.skipped.push(format!(
                            "{} is a memory card that was not read save by save",
                            path.display()
                        ));
                        run.report.unread.push(path.clone());
                    }
                    continue;
                }
            }
            run.report.stats.files_read += 1;
            run.report.stats.bytes_hashed += bytes.len() as u64;
            let mut notes = Vec::new();
            if !bytes.is_empty() && bytes.iter().all(|byte| *byte == 0x00 || *byte == 0xFF) {
                notes.push("holds no data: every byte is blank (the emulator creates the file even when nothing was saved)".to_string());
            }
            if meta.nlink() > 1 {
                notes.push("this file has more than one name on disk (hard link)".to_string());
            }
            let core_folder = in_core_folder
                .then(|| {
                    path.parent()
                        .and_then(Path::file_name)
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .flatten();
            if let Some(core) = &core_folder {
                notes.push(format!("in RetroArch's per-core folder \"{core}\""));
            }
            let identity = match played.get(&stem).map(Vec::as_slice) {
                Some([(content, core)]) => SaveIdentity {
                    strength: AssociationStrength::EmulatorConvention,
                    product_code: None,
                    region: None,
                    title: None,
                    evidence: vec![evidence(
                        IdentityKind::LooseRomTitle,
                        content,
                        IdentityConfidence::FilenameOnly,
                        &path,
                        "RetroArch history + save naming rule",
                        &format!(
                            "RetroArch names a save after the game file it ran; its history lists exactly one game file named \"{stem}\"{}",
                            core.as_deref()
                                .map(|c| format!(" (core {c})"))
                                .unwrap_or_default()
                        ),
                    )],
                    ambiguity: vec![
                        "the save's bytes do not say which game wrote them".to_string(),
                    ],
                },
                Some(many) => SaveIdentity {
                    strength: AssociationStrength::NameOnly,
                    ambiguity: vec![format!(
                        "RetroArch's history lists {} different game files named \"{stem}\"; any of them could own this save",
                        many.len()
                    )],
                    ..SaveIdentity::none("")
                },
                None => SaveIdentity {
                    strength: AssociationStrength::NameOnly,
                    ..SaveIdentity::none(
                        "only the file name suggests a game; RetroArch's history does not list a game file with this name",
                    )
                },
            };
            let mut identity = identity;
            identity.ambiguity.retain(|line| !line.is_empty());
            let unsupported = if format == SaveFormat::SaveState {
                vec![
                    "the core and core version that wrote this state are not recorded in a form EmuWiz can read".to_string(),
                    "a save state only loads in the same core, usually the same version; it is never offered to another emulator".to_string(),
                ]
            } else {
                Vec::new()
            };
            let save = DiscoveredSave {
                id: stable_id(&path, None),
                source_path: path.clone(),
                entry_name: None,
                emulator: "RetroArch".to_string(),
                profile: profile_of(&path).to_string(),
                emulator_version: None,
                platform: None,
                format,
                artifact_type: format.artifact_type(),
                size_bytes: bytes.len() as u64,
                content_sha256: Some(sha256_hex(&bytes)),
                container_sha256: None,
                modified_unix: Some(meta.mtime()),
                internal_modified: None,
                identity,
                in_use,
                integrity: vec![
                    "SHA-256 taken over the whole file".to_string(),
                    "the game's own save checksum, if it has one, was NOT checked".to_string(),
                ],
                notes,
                unsupported,
            };
            run.remember(&path, sig, std::slice::from_ref(&save));
            run.report.saves.push(save);
        }
    }
    Ok(())
}
