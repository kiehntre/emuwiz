//! Preservation-aware ES-DE gamelist publication using existing Playing Library
//! elections, shared hashed object identities and the shared atomic text writer.
//!
//! Plans bind reviewed bytes and directory/file identities. Apply, rollback and
//! recovery fail closed on conflicts. A persistent advisory lock inode excludes
//! cooperating EmuWiz attempts and is released by the kernel on process exit.
//! Schema-v2 recovery records bind both before and after images; legacy records
//! remain untouched and require manual review. Successful publication still
//! removes its recovery record; durable combined RetroDECK receipts remain a
//! controller integration task.
//!
//! Existing XML is structurally validated without reserializing metadata.
//! Comments, unknown elements and attributes retain their original bytes.
//!
//! Concurrency boundary: external editors and ES-DE need not take our lock.
//! Fresh checks immediately before shared atomic replacement reduce the window
//! but cannot make the subsequent pathname rename a cross-application CAS.
//! Parent/path replacement after a check is also a residual race. Conflicts
//! detected after a write retain the recovery record; never blindly restore.

use std::fs;
use std::path::{Path, PathBuf};

use quick_xml::escape::escape;
use serde::{Deserialize, Serialize};

use crate::emulator_environment::es_de::EsDeProfile;
use crate::launch::es_de_export::es_de_system_for_platform;
use crate::playing_library::PlayingLibraryPlan;
mod matching;
mod preservation;
mod xml;
pub use matching::MatchEvidence;
pub use preservation::DestinationEvidence;
use preservation::{PublicationLock, conflict, read_current};

/// Bumped only if this record's on-disk shape changes incompatibly. An
/// unrecognised version is treated as [`EsDePublicationError::RecoveryCorrupt`]
/// rather than guessed at.
const RECOVERY_SCHEMA_VERSION: u32 = 2;

/// Suffix appended to the gamelist's own file name to derive its recovery
/// record's path - see [`recovery_path_for`]. Deliberately not a shared
/// top-level directory: the record's location is derived purely from the
/// gamelist path, so it is always findable after a restart with no other
/// state.
const RECOVERY_FILE_SUFFIX: &str = ".es-de-publish-recovery.json";

/// The durable, path-derived record [`apply_es_de_gamelist_publication`]
/// writes *before* touching `gamelist.xml`, and
/// [`recover_es_de_gamelist_publication`] reloads after a restart with no
/// other state available. See docs/architecture/ES_DE_PUBLICATION_PRESERVATION.md.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EsDeGamelistRecoveryRecord {
    schema_version: u32,
    /// The gamelist path this record belongs to - redundant with the
    /// record's own file location, kept anyway so a record found by
    /// directory listing (not by this module's own lookup) is still
    /// self-describing.
    gamelist_path: PathBuf,
    /// The file's exact content before the publication that wrote this
    /// record, or `None` if it did not exist.
    previous_content: Option<String>,
    #[serde(default)]
    after_content: Option<String>,
    #[serde(default)]
    destination_evidence: Option<DestinationEvidence>,
    #[serde(default)]
    digest: Option<String>,
}

/// The fixed recovery-record path for `gamelist_path` - a sibling file in
/// the same directory, derived purely from the gamelist's own file name.
pub fn es_de_gamelist_recovery_path(gamelist_path: &Path) -> PathBuf {
    let mut file_name = gamelist_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    file_name.push(RECOVERY_FILE_SUFFIX);
    gamelist_path.with_file_name(file_name)
}

/// `true` when a previous publication for `gamelist_path` did not
/// provably finish (see docs/architecture/ES_DE_PUBLICATION_PRESERVATION.md).
pub fn has_unresolved_es_de_gamelist_recovery(gamelist_path: &Path) -> bool {
    match fs::symlink_metadata(es_de_gamelist_recovery_path(gamelist_path)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        _ => true,
    }
}

/// A `gamelist.xml` this module will not read past. Matches this crate's
/// other bounded-configuration-read limits in spirit (see
/// `crate::identity_source::no_intro::pack_import::NO_INTRO_PACK_MAX_DAT_BYTES`).
/// Reviewed bound: a real ES-DE gamelist rarely exceeds a few MiB even for
/// a very large system; 64 MiB is generous headroom while still refusing
/// an unbounded read.
pub const MAX_GAMELIST_BYTES: u64 = 64 * 1024 * 1024;

/// A recovery record's content is a JSON encoding of, at most, one whole
/// gamelist (`previous_content`) plus a small fixed overhead - JSON string
/// escaping can expand control characters and non-ASCII bytes, so this is
/// deliberately larger than [`MAX_GAMELIST_BYTES`] rather than equal to
/// it, sized to comfortably hold a worst-case escaped encoding of a
/// maximum-size gamelist without being unbounded.
pub const MAX_RECOVERY_RECORD_BYTES: u64 = 8 * MAX_GAMELIST_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EsDePublicationError {
    /// [`es_de_system_for_platform`] has no row for this platform.
    PlatformUnmapped { platform_id: String },
    /// The resolved ES-DE system does not appear in the profile's already
    /// discovered systems - see the module doc comment: this module never
    /// invents a new `<system>` block.
    SystemNotConfigured { es_de_system: &'static str },
    /// The gamelist path is not valid UTF-8 - refused rather than risking
    /// a lossy round-trip on write.
    UnsupportedPathEncoding { path: String },
    /// The gamelist path exists but is not a plain, readable file.
    GamelistUnreadable { path: PathBuf, detail: String },
    /// The gamelist file exceeds [`MAX_GAMELIST_BYTES`]; refused before
    /// reading it.
    GamelistTooLarge { path: PathBuf, bytes: u64 },
    /// The existing gamelist does not parse as one well-formed
    /// `<gameList>...</gameList>` document - refused, never guessed at.
    MalformedGamelist { path: PathBuf, detail: String },
    /// `plan.operations` has no election this profile/platform combination
    /// can publish - nothing to do.
    NothingToPublish,
    /// A recovery record for this gamelist already exists - see the
    /// preservation design doc. Refused rather than
    /// racing a second publication against an unresolved one.
    UnresolvedRecovery { recovery_path: PathBuf },
    /// [`recover_es_de_gamelist_publication`] found no recovery record for
    /// this gamelist path - nothing to recover.
    NoRecoveryRecord { path: PathBuf },
    /// A recovery record exists but does not parse, or names an
    /// unrecognised [`RECOVERY_SCHEMA_VERSION`] - refused rather than
    /// guessed at, since guessing here could silently discard the one
    /// record able to restore a gamelist's true prior state.
    RecoveryCorrupt { path: PathBuf, detail: String },
    /// A recovery record exceeds [`MAX_RECOVERY_RECORD_BYTES`]; refused
    /// before reading it.
    RecoveryTooLarge { path: PathBuf, bytes: u64 },
    /// A recovery record's own `gamelist_path` field does not exactly
    /// match the path the caller asked to recover - never trusted as a
    /// restoration target regardless (see
    /// [`recover_es_de_gamelist_publication`]'s doc comment), but a
    /// mismatch here means the record is tampered with or corrupt, and is
    /// refused rather than silently restored anyway.
    RecoveryPathMismatch {
        recovery_path: PathBuf,
        expected: PathBuf,
        recorded: PathBuf,
    },
    /// Writing or restoring the gamelist file failed.
    Io { path: PathBuf, detail: String },
}

impl std::fmt::Display for EsDePublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlatformUnmapped { platform_id } => {
                write!(
                    formatter,
                    "no ES-DE system is mapped for platform {platform_id}"
                )
            }
            Self::SystemNotConfigured { es_de_system } => write!(
                formatter,
                "ES-DE system \"{es_de_system}\" was not found in the discovered profile"
            ),
            Self::UnsupportedPathEncoding { path } => {
                write!(formatter, "gamelist path is not valid UTF-8: {path}")
            }
            Self::GamelistUnreadable { path, detail } => {
                write!(formatter, "{}: {detail}", path.display())
            }
            Self::GamelistTooLarge { path, bytes } => write!(
                formatter,
                "{} is {bytes} bytes, exceeding the {MAX_GAMELIST_BYTES}-byte bound",
                path.display()
            ),
            Self::MalformedGamelist { path, detail } => {
                write!(
                    formatter,
                    "{} is not a well-formed gamelist: {detail}",
                    path.display()
                )
            }
            Self::NothingToPublish => {
                formatter.write_str("no election produced anything this profile can publish")
            }
            Self::UnresolvedRecovery { recovery_path } => write!(
                formatter,
                "a previous publication did not finish (recovery record at {}); \
                 run recovery before publishing again",
                recovery_path.display()
            ),
            Self::NoRecoveryRecord { path } => {
                write!(
                    formatter,
                    "no recovery record exists for {}",
                    path.display()
                )
            }
            Self::RecoveryCorrupt { path, detail } => {
                write!(
                    formatter,
                    "recovery record {} is unusable: {detail}",
                    path.display()
                )
            }
            Self::RecoveryTooLarge { path, bytes } => write!(
                formatter,
                "recovery record {} is {bytes} bytes, exceeding the {MAX_RECOVERY_RECORD_BYTES}-byte bound",
                path.display()
            ),
            Self::RecoveryPathMismatch {
                recovery_path,
                expected,
                recorded,
            } => write!(
                formatter,
                "recovery record {} names {} but {} was expected; refusing a mismatched record",
                recovery_path.display(),
                recorded.display(),
                expected.display()
            ),
            Self::Io { path, detail } => write!(formatter, "{}: {detail}", path.display()),
        }
    }
}

impl std::error::Error for EsDePublicationError {}

/// One playing-library election this publication would add (or has
/// already found present) as an ES-DE `<game>` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDePublicationEntry {
    pub dat_entry_name: String,
    /// Exactly the playing-library symlink path the election produced -
    /// never the original archive path.
    pub destination_path: PathBuf,
}

/// A new entry deliberately left out of this publication, with the existing
/// entry that made it ambiguous. Nothing is written for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeRefusedEntry {
    pub entry: EsDePublicationEntry,
    /// The existing `<path>` text, exactly as decoded.
    pub existing_path: String,
    pub reason: &'static str,
}

/// The complete preview of one ES-DE gamelist publication - safe to
/// inspect, log, or show a user before [`apply_es_de_gamelist_publication`]
/// is ever called. Nothing is written to disk while building this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeGamelistPublication {
    pub es_de_system: &'static str,
    pub gamelist_path: PathBuf,
    /// The file's exact content before this publication, or `None` if it
    /// did not exist. Captured so rollback can restore it exactly.
    pub previous_content: Option<String>,
    /// The exact bytes [`apply_es_de_gamelist_publication`] would write -
    /// `previous_content` (or a fresh scaffold) with the new `<game>` blocks
    /// for [`Self::added`] inserted before `</gameList>`; an empty
    /// `<gameList/>` root is opened into `<gameList>`...`</gameList>`.
    pub new_content: String,
    /// Elections that will become new `<game>` entries.
    pub added: Vec<EsDePublicationEntry>,
    /// Elections whose destination path is already present in the
    /// existing gamelist - re-running an already-published plan lands
    /// every entry here instead of in `added`, which is exactly what
    /// makes re-publication idempotent.
    pub already_present: Vec<EsDePublicationEntry>,
    /// Elections not added because an existing entry with the same file name
    /// cannot be proven to be a different game (see `matching`). Existing
    /// entries are never edited; the rest of the publication proceeds.
    pub refused: Vec<EsDeRefusedEntry>,
    /// File identities the add/present/refuse decisions relied on; re-checked
    /// before Apply writes, so a changed link or ROM makes the preview stale.
    pub match_evidence: MatchEvidence,
    /// Namespace and before-image proof captured during review.
    pub destination_evidence: DestinationEvidence,
}

/// What a publication leaves untouched in the existing gamelist. Publication
/// only inserts new `<game>` entries before `</gameList>`; it never changes,
/// reorders or removes existing entries, so "changed" is always zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EsDePreservedSummary {
    pub game_entries: usize,
    pub folder_entries: usize,
    /// Other root-level elements such as `<alternativeEmulator>`.
    pub other_elements: usize,
    /// Bytes of the existing file kept verbatim: all of it, except the `/>`
    /// of an empty `<gameList/>` root, which becomes `>` ... `</gameList>`.
    pub preserved_bytes: usize,
}

impl EsDeGamelistPublication {
    /// `true` when there is nothing new to write.
    pub fn is_unchanged(&self) -> bool {
        self.added.is_empty()
    }

    /// Preview of preserved content. `None` when no gamelist existed; an
    /// error when the previous content no longer inspects (never guessed).
    pub fn preserved_summary(&self) -> Result<Option<EsDePreservedSummary>, EsDePublicationError> {
        let Some(previous) = &self.previous_content else {
            return Ok(None);
        };
        let xml = xml::inspect(previous, &self.gamelist_path)?;
        Ok(Some(EsDePreservedSummary {
            game_entries: xml.games,
            folder_entries: xml.folders,
            other_elements: xml.other_elements,
            preserved_bytes: previous.len() - if xml.empty_root { 2 } else { 0 },
        }))
    }
}

/// Builds a complete, previewable publication plan. Performs a single
/// bounded read of the target gamelist (if one exists) and otherwise
/// touches no filesystem state.
pub fn plan_es_de_gamelist_publication(
    plan: &PlayingLibraryPlan,
    platform_id: &str,
    profile: &EsDeProfile,
) -> Result<EsDeGamelistPublication, EsDePublicationError> {
    if plan.operations.is_empty() {
        return Err(EsDePublicationError::NothingToPublish);
    }
    let mapping = es_de_system_for_platform(platform_id).ok_or_else(|| {
        EsDePublicationError::PlatformUnmapped {
            platform_id: platform_id.to_string(),
        }
    })?;
    let locations = profile
        .system_data
        .iter()
        .find(|entry| entry.system_name == mapping.es_de_system)
        .ok_or(EsDePublicationError::SystemNotConfigured {
            es_de_system: mapping.es_de_system,
        })?;

    if locations.gamelist_file.path.lossy {
        return Err(EsDePublicationError::UnsupportedPathEncoding {
            path: locations.gamelist_file.path.display.clone(),
        });
    }
    let gamelist_path = PathBuf::from(&locations.gamelist_file.path.display);

    if has_unresolved_es_de_gamelist_recovery(&gamelist_path) {
        return Err(EsDePublicationError::UnresolvedRecovery {
            recovery_path: es_de_gamelist_recovery_path(&gamelist_path),
        });
    }

    // `locations` came from a discovery snapshot that may predate this
    // call (e.g. a previous publication in the same session already
    // created this file) - the profile is only trusted for *which* system
    // and path this is, never for whether the file currently exists, so
    // this re-probes live rather than trusting a possibly-stale
    // `FsProbe` value.
    let destination_evidence = DestinationEvidence::capture(&gamelist_path)?;
    let previous_content = match fs::symlink_metadata(&gamelist_path) {
        Ok(metadata) if metadata.is_file() => Some(read_bounded_gamelist(&gamelist_path)?),
        Ok(_) => {
            return Err(EsDePublicationError::GamelistUnreadable {
                path: gamelist_path,
                detail: "path exists but is not a regular file".to_string(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(EsDePublicationError::GamelistUnreadable {
                path: gamelist_path,
                detail: error.to_string(),
            });
        }
    };

    // Only elections whose launcher operation survived conflict filtering
    // into `plan.operations` are eligible - exactly the same subset the
    // existing symlink apply consumes. Always the launcher's own
    // destination path, never a companion's: a CUE/GDI/M3U-based release
    // must be pointed at its CUE/GDI/M3U file, not one of its BIN/track
    // companions.
    let mut entries: Vec<(EsDePublicationEntry, PathBuf)> = Vec::new();
    for elected in &plan.elected_games {
        if plan.operations.contains(&elected.launcher_operation) {
            entries.push((
                EsDePublicationEntry {
                    dat_entry_name: elected.dat_entry_name.clone(),
                    destination_path: elected.launcher_operation.destination_path.clone(),
                },
                elected.launcher_operation.source_path.clone(),
            ));
        }
    }
    if entries.is_empty() {
        return Err(EsDePublicationError::NothingToPublish);
    }

    destination_evidence.verify_before(&gamelist_path, &previous_content)?;
    let layout = match &previous_content {
        Some(text) => Some(xml::inspect(text, &gamelist_path)?),
        None => None,
    };

    let rom_root = matching::system_rom_root(profile, mapping.es_de_system);
    let mut existing =
        matching::ExistingEntries::index(layout.as_ref().map(|xml| &xml.paths), &rom_root);
    let mut added = Vec::new();
    let mut already_present = Vec::new();
    let mut refused = Vec::new();
    let mut appended = String::new();
    let mut planned = std::collections::BTreeSet::new();
    for (entry, source) in entries {
        let destination_text = entry.destination_path.to_str().ok_or_else(|| {
            EsDePublicationError::UnsupportedPathEncoding {
                path: entry.destination_path.to_string_lossy().into_owned(),
            }
        })?;
        let escaped_path = escape(destination_text).into_owned();
        let destination = matching::lexical_absolute(destination_text)
            .unwrap_or_else(|| entry.destination_path.clone());
        // Two elections for one destination would otherwise append two
        // identical entries; the second is present once the first is added.
        if !planned.insert(destination.clone()) {
            already_present.push(entry);
            continue;
        }
        match existing.decide(&destination, &source) {
            matching::Decision::Add => {}
            matching::Decision::AlreadyPresent => {
                already_present.push(entry);
                continue;
            }
            matching::Decision::Refuse { existing, reason } => {
                refused.push(EsDeRefusedEntry {
                    entry,
                    existing_path: existing,
                    reason,
                });
                continue;
            }
        }
        let escaped_name = escape(entry.dat_entry_name.as_str()).into_owned();
        appended.push_str(&format!(
            "\t<game>\n\t\t<path>{escaped_path}</path>\n\t\t<name>{escaped_name}</name>\n\t</game>\n"
        ));
        added.push(entry);
    }

    let new_content = match (&previous_content, layout) {
        (Some(text), Some(xml)) if !added.is_empty() => {
            let offset = xml.offset;
            let mut rebuilt = String::with_capacity(text.len() + appended.len());
            rebuilt.push_str(&text[..offset]);
            if xml.empty_root {
                rebuilt.push_str(">\n");
                rebuilt.push_str(&appended);
                rebuilt.push_str("</gameList>");
                rebuilt.push_str(&text[offset + 2..]);
            } else {
                rebuilt.push_str(&appended);
                rebuilt.push_str(&text[offset..]);
            }
            rebuilt
        }
        (Some(text), _) => text.clone(),
        (None, _) => format!("<?xml version=\"1.0\"?>\n<gameList>\n{appended}</gameList>\n"),
    };

    if new_content.len() as u64 > MAX_GAMELIST_BYTES {
        return Err(EsDePublicationError::GamelistTooLarge {
            path: gamelist_path,
            bytes: new_content.len() as u64,
        });
    }
    xml::inspect(&new_content, &gamelist_path)?;
    Ok(EsDeGamelistPublication {
        es_de_system: mapping.es_de_system,
        gamelist_path,
        previous_content,
        new_content,
        added,
        already_present,
        refused,
        match_evidence: existing.evidence,
        destination_evidence,
    })
}

/// Applies only a fresh reviewed before-image. Reusing a nonempty applied
/// snapshot is stale; fresh replanning of unchanged data is a no-write no-op.
/// Conflict errors leave external bytes untouched.
pub fn apply_es_de_gamelist_publication(
    publication: &EsDeGamelistPublication,
) -> Result<(), EsDePublicationError> {
    let path = &publication.gamelist_path;
    xml::inspect(&publication.new_content, path)?;
    verify_insertion_only(publication)?;
    publication
        .destination_evidence
        .verify_before(path, &publication.previous_content)?;
    if has_unresolved_es_de_gamelist_recovery(path) {
        return Err(EsDePublicationError::UnresolvedRecovery {
            recovery_path: es_de_gamelist_recovery_path(path),
        });
    }
    if publication.is_unchanged() {
        return Ok(());
    }
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| conflict(path, "missing parent"))?,
    )
    .map_err(|e| conflict(path, e.to_string()))?;
    let _lock = PublicationLock::acquire(path)?;
    publication.destination_evidence.verify_directories(path)?;
    publication
        .match_evidence
        .verify()
        .map_err(|detail| conflict(path, format!("preview is stale: {detail}")))?;
    if has_unresolved_es_de_gamelist_recovery(path) {
        return Err(EsDePublicationError::UnresolvedRecovery {
            recovery_path: es_de_gamelist_recovery_path(path),
        });
    }
    publication
        .destination_evidence
        .verify_before(path, &publication.previous_content)?;
    let record = publication_record(publication)?;
    persist_record(&record)?;
    // Recheck after the journal write, before entering the existing atomic
    // writer. This is not atomic exclusion of non-cooperating external writers.
    publication
        .destination_evidence
        .verify_before(path, &publication.previous_content)?;
    publication_fault("before_write", path)?;
    publication
        .destination_evidence
        .verify_before(path, &publication.previous_content)?;
    crate::atomic_write_text(path, &publication.new_content)
        .map_err(|e| conflict(path, e.to_string()))?;
    publication_fault("after_write", path)?;
    record
        .destination_evidence
        .as_ref()
        .unwrap()
        .verify_directories(path)?;
    if read_current(path)?.as_deref() != Some(&publication.new_content) {
        return Err(conflict(
            path,
            "output changed before finalization; recovery evidence retained",
        ));
    }
    clear_record(path, &record)
}

/// `new_content` must be `previous_content` with text inserted only at the
/// root's closing span, so a hand-built or tampered publication can never
/// replace existing entries.
fn verify_insertion_only(
    publication: &EsDeGamelistPublication,
) -> Result<(), EsDePublicationError> {
    let Some(previous) = &publication.previous_content else {
        return Ok(());
    };
    let new = &publication.new_content;
    let xml = xml::inspect(previous, &publication.gamelist_path)?;
    let (head, tail) = if xml.empty_root {
        (&previous[..xml.offset], &previous[xml.offset + 2..])
    } else {
        (&previous[..xml.offset], &previous[xml.offset..])
    };
    let refuse = || {
        conflict(
            &publication.gamelist_path,
            "publication would change existing gamelist content; only insertion is allowed",
        )
    };
    if new.len() < head.len() + tail.len() || !new.starts_with(head) || !new.ends_with(tail) {
        return Err(refuse());
    }
    // Independently of the offsets above, the result must hold exactly the
    // previous entries plus one real `<game>` per added election, each with
    // its path, so an entry swallowed by a comment or attribute is refused.
    let after = xml::inspect(new, &publication.gamelist_path)?;
    let every_added_path_present = publication.added.iter().all(|entry| {
        entry
            .destination_path
            .to_str()
            .is_some_and(|path| after.paths.contains(path) && !xml.paths.contains(path))
    });
    if after.games != xml.games + publication.added.len()
        || after.folders != xml.folders
        || after.other_elements != xml.other_elements
        || !every_added_path_present
    {
        return Err(refuse());
    }
    Ok(())
}

/// Detailed inverse outcome, while the existing Result<()> entry points stay
/// source-compatible with production controllers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EsDeRestoreOutcome {
    Restored,
    AlreadyRestored,
}

pub fn recover_es_de_gamelist_publication(path: &Path) -> Result<(), EsDePublicationError> {
    recover_es_de_gamelist_publication_with_outcome(path).map(|_| ())
}

pub fn recover_es_de_gamelist_publication_with_outcome(
    path: &Path,
) -> Result<EsDeRestoreOutcome, EsDePublicationError> {
    let record = read_recovery_record(&es_de_gamelist_recovery_path(path), path)?;
    let evidence = record
        .destination_evidence
        .as_ref()
        .ok_or_else(|| conflict(path, "missing destination evidence"))?;
    evidence.verify_directories(path)?;
    let _lock = PublicationLock::acquire(path)?;
    if read_recovery_record(&es_de_gamelist_recovery_path(path), path)? != record {
        return Err(conflict(path, "recovery record changed"));
    }
    let result = guarded_restore(
        path,
        &record.previous_content,
        record.after_content.as_ref().unwrap(),
        evidence,
    )?;
    clear_record(path, &record)?;
    Ok(result)
}

pub fn rollback_es_de_gamelist_publication(
    publication: &EsDeGamelistPublication,
) -> Result<(), EsDePublicationError> {
    rollback_es_de_gamelist_publication_with_outcome(publication).map(|_| ())
}

pub fn rollback_es_de_gamelist_publication_with_outcome(
    publication: &EsDeGamelistPublication,
) -> Result<EsDeRestoreOutcome, EsDePublicationError> {
    let path = &publication.gamelist_path;
    publication.destination_evidence.verify_directories(path)?;
    if publication.is_unchanged() {
        if read_current(path)? != publication.previous_content {
            return Err(conflict(
                path,
                "unchanged publication has been externally modified",
            ));
        }
        return Ok(EsDeRestoreOutcome::AlreadyRestored);
    }
    let _lock = PublicationLock::acquire(path)?;
    let record = if has_unresolved_es_de_gamelist_recovery(path) {
        let record = read_recovery_record(&es_de_gamelist_recovery_path(path), path)?;
        if record.previous_content != publication.previous_content
            || record.after_content.as_ref() != Some(&publication.new_content)
        {
            return Err(conflict(
                path,
                "recovery record belongs to another publication",
            ));
        }
        record
            .destination_evidence
            .as_ref()
            .unwrap()
            .verify_directories(path)?;
        Some(record)
    } else {
        None
    };
    let result = guarded_restore(
        path,
        &publication.previous_content,
        &publication.new_content,
        &publication.destination_evidence,
    )?;
    if let Some(record) = record {
        clear_record(path, &record)?;
    }
    Ok(result)
}

fn guarded_restore(
    path: &Path,
    before: &Option<String>,
    after: &str,
    evidence: &DestinationEvidence,
) -> Result<EsDeRestoreOutcome, EsDePublicationError> {
    evidence.verify_directories(path)?;
    let current = read_current(path)?;
    if current == *before {
        return Ok(EsDeRestoreOutcome::AlreadyRestored);
    }
    if current.as_deref() != Some(after) {
        return Err(conflict(
            path,
            "current gamelist does not match published output; left untouched",
        ));
    }
    evidence.verify_directories(path)?;
    if read_current(path)?.as_deref() != Some(after) {
        return Err(conflict(path, "gamelist changed before restoration"));
    }
    restore_content(path, before)?;
    if read_current(path)? != *before {
        return Err(conflict(path, "restoration changed before confirmation"));
    }
    Ok(EsDeRestoreOutcome::Restored)
}

fn publication_record(
    p: &EsDeGamelistPublication,
) -> Result<EsDeGamelistRecoveryRecord, EsDePublicationError> {
    let mut record = EsDeGamelistRecoveryRecord {
        schema_version: RECOVERY_SCHEMA_VERSION,
        gamelist_path: p.gamelist_path.clone(),
        previous_content: p.previous_content.clone(),
        after_content: Some(p.new_content.clone()),
        destination_evidence: Some(DestinationEvidence::capture(&p.gamelist_path)?),
        digest: None,
    };
    record.digest = Some(record_digest(&record)?);
    Ok(record)
}

fn record_digest(record: &EsDeGamelistRecoveryRecord) -> Result<String, EsDePublicationError> {
    use sha2::{Digest, Sha256};
    let mut unsigned = record.clone();
    unsigned.digest = None;
    let bytes = serde_json::to_vec(&unsigned)
        .map_err(|e| conflict(&record.gamelist_path, e.to_string()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn persist_record(record: &EsDeGamelistRecoveryRecord) -> Result<(), EsDePublicationError> {
    let path = es_de_gamelist_recovery_path(&record.gamelist_path);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(EsDePublicationError::UnresolvedRecovery {
                recovery_path: path,
            });
        }
        Err(e) => return Err(conflict(&path, e.to_string())),
    }
    let body = serde_json::to_string_pretty(record).map_err(|e| conflict(&path, e.to_string()))?;
    crate::atomic_write_text(&path, &body).map_err(|e| conflict(&path, e.to_string()))
}

fn clear_record(
    path: &Path,
    expected: &EsDeGamelistRecoveryRecord,
) -> Result<(), EsDePublicationError> {
    let record_path = es_de_gamelist_recovery_path(path);
    if read_recovery_record(&record_path, path)? != *expected {
        return Err(conflict(path, "receipt changed; retained for review"));
    }
    publication_fault("finalize", path)?;
    fs::remove_file(&record_path).map_err(|e| conflict(&record_path, e.to_string()))?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static PUBLICATION_FAULT: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
    static PUBLICATION_HOOK: std::cell::RefCell<Option<Box<dyn Fn(&str, &Path)>>> = const { std::cell::RefCell::new(None) };
}
fn publication_fault(stage: &str, path: &Path) -> Result<(), EsDePublicationError> {
    #[cfg(test)]
    PUBLICATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow().as_ref() {
            hook(stage, path);
        }
    });
    #[cfg(test)]
    if PUBLICATION_FAULT.with(|fault| *fault.borrow() == Some(stage)) {
        return Err(conflict(path, format!("injected {stage} failure")));
    }
    let _ = (stage, path);
    Ok(())
}

/// Writes a durable recovery record for `gamelist_path`, capturing
/// `previous_content`. Called before every real `gamelist.xml` write -
/// see docs/architecture/ES_DE_PUBLICATION_PRESERVATION.md.
#[cfg(test)]
fn write_recovery_record(
    recovery_path: &Path,
    gamelist_path: &Path,
    previous_content: &Option<String>,
) -> Result<(), EsDePublicationError> {
    let record = EsDeGamelistRecoveryRecord {
        schema_version: 1,
        gamelist_path: gamelist_path.to_path_buf(),
        previous_content: previous_content.clone(),
        after_content: None,
        destination_evidence: None,
        digest: None,
    };
    let body = serde_json::to_string_pretty(&record).map_err(|error| EsDePublicationError::Io {
        path: recovery_path.to_path_buf(),
        detail: error.to_string(),
    })?;
    crate::atomic_write_text(recovery_path, &body).map_err(|error| EsDePublicationError::Io {
        path: recovery_path.to_path_buf(),
        detail: error.to_string(),
    })
}

/// Reads and validates a recovery record, bound to `expected_gamelist_path`.
///
/// Refuses (never guesses at) a record that: exceeds
/// [`MAX_RECOVERY_RECORD_BYTES`] (checked from filesystem metadata before
/// any read, mirroring [`read_bounded_gamelist`]); fails to parse; names
/// an unrecognised schema version; or whose own `gamelist_path` field does
/// not exactly match `expected_gamelist_path`
/// ([`EsDePublicationError::RecoveryPathMismatch`]). That last check
/// exists purely as tamper/corruption detection - the record's
/// `gamelist_path` field is *never* used as a restoration target by any
/// caller of this function; every caller restores to the path it already
/// trusted before this record was ever read.
fn read_recovery_record(
    recovery_path: &Path,
    expected_gamelist_path: &Path,
) -> Result<EsDeGamelistRecoveryRecord, EsDePublicationError> {
    let metadata = fs::symlink_metadata(recovery_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            EsDePublicationError::NoRecoveryRecord {
                path: recovery_path.to_path_buf(),
            }
        } else {
            EsDePublicationError::Io {
                path: recovery_path.to_path_buf(),
                detail: error.to_string(),
            }
        }
    })?;
    if !metadata.is_file() {
        return Err(EsDePublicationError::RecoveryCorrupt {
            path: recovery_path.to_path_buf(),
            detail: "recovery path exists but is not a regular file".to_string(),
        });
    }
    if metadata.len() > MAX_RECOVERY_RECORD_BYTES {
        return Err(EsDePublicationError::RecoveryTooLarge {
            path: recovery_path.to_path_buf(),
            bytes: metadata.len(),
        });
    }
    let body = bounded_text(recovery_path, MAX_RECOVERY_RECORD_BYTES)?;
    let record: EsDeGamelistRecoveryRecord =
        serde_json::from_str(&body).map_err(|error| EsDePublicationError::RecoveryCorrupt {
            path: recovery_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    if record.gamelist_path != expected_gamelist_path {
        return Err(EsDePublicationError::RecoveryPathMismatch {
            recovery_path: recovery_path.to_path_buf(),
            expected: expected_gamelist_path.to_path_buf(),
            recorded: record.gamelist_path,
        });
    }
    if record.schema_version != RECOVERY_SCHEMA_VERSION {
        return Err(EsDePublicationError::RecoveryCorrupt {
            path: recovery_path.to_path_buf(),
            detail: format!(
                "recovery schema {} lacks supported after-state evidence; manual review required",
                record.schema_version
            ),
        });
    }
    if record.after_content.is_none()
        || record.destination_evidence.is_none()
        || record.digest.as_deref() != Some(record_digest(&record)?.as_str())
    {
        return Err(EsDePublicationError::RecoveryCorrupt {
            path: recovery_path.into(),
            detail: "missing or changed preservation evidence; manual review required".into(),
        });
    }
    Ok(record)
}

/// Restores `path` to exactly `content` (or removes it, if `content` is
/// `None`) - the one primitive both [`recover_es_de_gamelist_publication`]
/// and [`rollback_es_de_gamelist_publication`]'s `previous_content: None`
/// case need for "put this file back exactly as it was".
fn restore_content(path: &Path, content: &Option<String>) -> Result<(), EsDePublicationError> {
    match content {
        Some(text) => {
            crate::atomic_write_text(path, text).map_err(|error| EsDePublicationError::Io {
                path: path.to_path_buf(),
                detail: error.to_string(),
            })
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(EsDePublicationError::Io {
                path: path.to_path_buf(),
                detail: error.to_string(),
            }),
        },
    }
}

fn read_bounded_gamelist(path: &PathBuf) -> Result<String, EsDePublicationError> {
    let metadata =
        fs::metadata(path).map_err(|error| EsDePublicationError::GamelistUnreadable {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    if metadata.len() > MAX_GAMELIST_BYTES {
        return Err(EsDePublicationError::GamelistTooLarge {
            path: path.clone(),
            bytes: metadata.len(),
        });
    }
    bounded_text(path, MAX_GAMELIST_BYTES)
}

/// Bounded descriptor read with leaf no-follow and shared hashed identity proof.
fn bounded_text(path: &Path, limit: u64) -> Result<String, EsDePublicationError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| conflict(path, e.to_string()))?;
    let before = file.metadata().map_err(|e| conflict(path, e.to_string()))?;
    if !before.is_file() || before.len() > limit {
        return Err(conflict(path, "read requires a bounded regular file"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| conflict(path, e.to_string()))?;
    if bytes.len() as u64 > limit {
        return Err(conflict(path, "file grew beyond read bound"));
    }
    let current = crate::dat::rename_apply::capture_identity(path)
        .map_err(|e| conflict(path, e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if current.dev != before.dev() || current.ino != before.ino() {
            return Err(conflict(path, "file replaced during read"));
        }
    }
    if current.kind != crate::dat::rename_apply::ObjectKind::RegularFile
        || current.size_bytes != before.len()
        || current.freshness.as_ref().is_none_or(|proof| {
            proof.modified
                != before
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                || proof.sha256 != <[u8; 32]>::from(Sha256::digest(&bytes))
        })
    {
        return Err(conflict(path, "file changed during read"));
    }
    String::from_utf8(bytes).map_err(|e| conflict(path, e.to_string()))
}

#[cfg(test)]
mod tests;
