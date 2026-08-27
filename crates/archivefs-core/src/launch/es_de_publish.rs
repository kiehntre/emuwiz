//! Bridges the read-only "Build Playing Library" plan
//! ([`crate::playing_library::PlayingLibraryPlan`]) to ES-DE's own
//! `gamelist.xml`, so games elected into a playing library become visible
//! to an already-discovered ES-DE installation.
//!
//! # What this reuses, and does not duplicate
//!
//! - The election itself: [`PlayingLibraryPlan`] - this module never
//!   re-elects, re-verifies, or re-groups anything. Only
//!   `plan.elected_games` entries whose operation also survived conflict
//!   filtering into `plan.operations` are published - exactly the same
//!   subset [`crate::playing_library::apply_adapter::build_playing_library_transaction`]
//!   turns into symlinks.
//! - The platform -> ES-DE system mapping:
//!   [`crate::launch::es_de_export::es_de_system_for_platform`] - the same
//!   reviewed table every other ES-DE export path uses. A platform absent
//!   there is refused, never guessed at.
//! - The ES-DE installation/profile:
//!   [`crate::emulator_environment::es_de::EsDeProfile`], already
//!   discovered by
//!   [`crate::emulator_environment::es_de::discover_es_de_environment`] -
//!   this module never assumes a path and accepts a profile the caller
//!   already resolved.
//!
//! # What is genuinely new here
//!
//! No `gamelist.xml` reader or writer exists anywhere else in this crate.
//! This module adds the smallest one that is safe: it never re-serializes
//! or reformats a byte of an existing file. It only appends new `<game>`
//! blocks for playing-library links not already present (matched by an
//! exact `<path>` text comparison), inserted immediately before the
//! closing `</gameList>` tag. Every other byte of a pre-existing file -
//! every other `<game>`, comment, or unrecognized element - is preserved
//! character-for-character. A file that does not parse as one
//! well-formed `<gameList>...</gameList>` document is refused, never
//! guessed at (see [`EsDePublicationError::MalformedGamelist`]). This
//! module never creates or edits a `<system>` block in
//! `custom_systems/es_systems.xml`, nor touches `es_settings.xml` or
//! anything else - see [`EsDePublicationError::SystemNotConfigured`].
//!
//! # Why this does not go through `crate::dat::rename_apply`
//!
//! That engine's entire data model - `ObjectIdentity`, basename-based
//! no-clobber, `RenameMove`/`CreateSymlink` - exists to make one
//! filesystem *object* safely become another. The ROM-linking half of
//! "Build Playing Library" already reuses it unchanged (see
//! [`crate::playing_library::apply_adapter`]). Editing the text content
//! of one already-identified configuration file is a different kind of
//! operation with no natural fit in that model; forcing it in would mean
//! redesigning a shared, heavily-tested engine many unrelated features
//! depend on, not reusing it. Durability and rollback safety are instead
//! provided directly by this module: every publication first captures
//! the *exact* previous file content (or its prior absence), then writes
//! through [`crate::atomic_write_text`] - the same atomic-write primitive
//! `crate::identity_source::no_intro::pack_import` already uses for its
//! own durable state file - and [`rollback_es_de_gamelist_publication`]
//! restores that captured state exactly. This is a deliberate, narrower
//! alternative to extending the shared rename/symlink engine, called out
//! here for review rather than silently assumed.
//!
//! # Crash/restart durability
//!
//! The captured `previous_content` above only helps a caller that still
//! holds the in-memory [`EsDeGamelistPublication`] value - it does not
//! survive a process restart on its own. To make that durable,
//! [`apply_es_de_gamelist_publication`] first writes a small recovery
//! record (see [`EsDeGamelistRecoveryRecord`]) to a fixed, path-derived
//! location *before* touching `gamelist.xml` at all, then deletes that
//! record only after the real write has succeeded. If the process is
//! killed anywhere in between, the record survives on disk with no
//! in-memory state required to find it again -
//! [`recover_es_de_gamelist_publication`] reloads it from nothing but the
//! gamelist path and restores the exact prior bytes (or removes the file,
//! if it did not exist before).
//!
//! **Recovery policy, stated plainly**: a leftover recovery record always
//! means "the last publication attempt for this gamelist did not
//! provably finish" - never "it definitely failed" or "it definitely
//! succeeded", since the crash could have landed on either side of the
//! real write. [`recover_es_de_gamelist_publication`] always resolves
//! this the same way regardless of which side it landed on: restore the
//! captured previous state and discard the record. A real write that did
//! complete just before the crash gets undone rather than guessed at as
//! "probably fine" - safe, because re-publishing afterwards is cheap and
//! idempotent (see [`EsDeGamelistPublication::is_unchanged`]), unlike
//! silently trusting a write this module cannot prove happened.
//! [`plan_es_de_gamelist_publication`] and
//! [`apply_es_de_gamelist_publication`] both refuse outright
//! ([`EsDePublicationError::UnresolvedRecovery`]) while a recovery record
//! for that gamelist exists, rather than racing a second publication
//! against an unresolved one.
//!
//! # Recovery-record safety
//!
//! A recovery record is bounded before it is ever read
//! ([`MAX_RECOVERY_RECORD_BYTES`], checked from filesystem metadata
//! first, exactly like [`MAX_GAMELIST_BYTES`] for the gamelist itself),
//! and its own path is read with [`std::fs::symlink_metadata`] (never
//! followed) - a symlink sitting at the recovery path is refused as
//! [`EsDePublicationError::RecoveryCorrupt`], never read through.
//!
//! The record's own `gamelist_path` field is *never* trusted as a
//! restoration target - every restore uses the path the caller already
//! supplied, the same path used to derive the recovery path in the first
//! place. The field is checked only for *equality* against that trusted
//! path, and a mismatch fails closed
//! ([`EsDePublicationError::RecoveryPathMismatch`]) as tamper/corruption
//! evidence, never silently ignored.
//!
//! Writing the record itself goes through the same [`crate::atomic_write_text`]
//! every other write in this module uses, which is symlink-safe for its
//! *destination* by construction: it writes a brand-new, uniquely-named
//! temporary file in the same directory, then finishes with a single
//! `rename(2)`. POSIX `rename` replaces whatever directory entry
//! currently exists at the destination name - if that entry is a
//! symlink, the symlink itself is replaced, and the file it pointed to is
//! never opened, followed, or written through. An attacker who plants a
//! symlink at a recovery (or gamelist) path before a write therefore
//! cannot redirect that write anywhere else; the symlink is simply
//! discarded and replaced with a real file at the same name. This
//! guarantee is exercised directly by
//! `tests::writing_the_recovery_record_through_a_symlinked_path_never_touches_its_target`.
//! The one place `atomic_write_text` does follow a symlink is its
//! permission-copy step (`fs::metadata` on the *existing* destination, to
//! preserve its mode bits before the rename) - a metadata-only read that
//! never returns file content and never itself writes anywhere.

use std::fs;
use std::path::{Path, PathBuf};

use quick_xml::escape::escape;
use serde::{Deserialize, Serialize};

use crate::emulator_environment::es_de::EsDeProfile;
use crate::launch::es_de_export::es_de_system_for_platform;
use crate::playing_library::PlayingLibraryPlan;

/// Bumped only if this record's on-disk shape changes incompatibly. An
/// unrecognised version is treated as [`EsDePublicationError::RecoveryCorrupt`]
/// rather than guessed at.
const RECOVERY_SCHEMA_VERSION: u32 = 1;

/// Suffix appended to the gamelist's own file name to derive its recovery
/// record's path - see [`recovery_path_for`]. Deliberately not a shared
/// top-level directory: the record's location is derived purely from the
/// gamelist path, so it is always findable after a restart with no other
/// state.
const RECOVERY_FILE_SUFFIX: &str = ".es-de-publish-recovery.json";

/// The durable, path-derived record [`apply_es_de_gamelist_publication`]
/// writes *before* touching `gamelist.xml`, and
/// [`recover_es_de_gamelist_publication`] reloads after a restart with no
/// other state available. See the module doc comment's "Crash/restart
/// durability" section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
/// provably finish - see the module doc comment's "Recovery policy".
pub fn has_unresolved_es_de_gamelist_recovery(gamelist_path: &Path) -> bool {
    es_de_gamelist_recovery_path(gamelist_path).is_file()
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
pub const MAX_RECOVERY_RECORD_BYTES: u64 = 4 * MAX_GAMELIST_BYTES;

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
    /// module doc comment's "Recovery policy". Refused rather than
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
    /// identical to `previous_content` (or a fresh scaffold) plus the
    /// newly appended `<game>` blocks for [`Self::added`].
    pub new_content: String,
    /// Elections that will become new `<game>` entries.
    pub added: Vec<EsDePublicationEntry>,
    /// Elections whose destination path is already present in the
    /// existing gamelist - re-running an already-published plan lands
    /// every entry here instead of in `added`, which is exactly what
    /// makes re-publication idempotent.
    pub already_present: Vec<EsDePublicationEntry>,
}

impl EsDeGamelistPublication {
    /// `true` when there is nothing new to write.
    pub fn is_unchanged(&self) -> bool {
        self.added.is_empty()
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

    // Only elections whose operation survived conflict filtering into
    // `plan.operations` are eligible - exactly the same subset the
    // existing symlink apply consumes.
    let mut entries: Vec<EsDePublicationEntry> = Vec::new();
    for elected in &plan.elected_games {
        if plan.operations.contains(&elected.operation) {
            entries.push(EsDePublicationEntry {
                dat_entry_name: elected.dat_entry_name.clone(),
                destination_path: elected.operation.destination_path.clone(),
            });
        }
    }
    if entries.is_empty() {
        return Err(EsDePublicationError::NothingToPublish);
    }

    let insert_offset = match &previous_content {
        Some(text) => Some(locate_insertion_point(text, &gamelist_path)?),
        None => None,
    };

    let mut added = Vec::new();
    let mut already_present = Vec::new();
    let mut appended = String::new();
    for entry in entries {
        let escaped_path = escape(entry.destination_path.to_string_lossy().as_ref()).into_owned();
        let needle = format!("<path>{escaped_path}</path>");
        let already_there = previous_content
            .as_deref()
            .is_some_and(|text| text.contains(&needle));
        if already_there {
            already_present.push(entry);
            continue;
        }
        let escaped_name = escape(entry.dat_entry_name.as_str()).into_owned();
        appended.push_str(&format!(
            "\t<game>\n\t\t<path>{escaped_path}</path>\n\t\t<name>{escaped_name}</name>\n\t</game>\n"
        ));
        added.push(entry);
    }

    let new_content = match (&previous_content, insert_offset) {
        (Some(text), Some(offset)) if !added.is_empty() => {
            let mut rebuilt = String::with_capacity(text.len() + appended.len());
            rebuilt.push_str(&text[..offset]);
            rebuilt.push_str(&appended);
            rebuilt.push_str(&text[offset..]);
            rebuilt
        }
        (Some(text), _) => text.clone(),
        (None, _) => format!("<?xml version=\"1.0\"?>\n<gameList>\n{appended}</gameList>\n"),
    };

    Ok(EsDeGamelistPublication {
        es_de_system: mapping.es_de_system,
        gamelist_path,
        previous_content,
        new_content,
        added,
        already_present,
    })
}

/// Writes `publication.new_content` if - and only if - it actually differs
/// from what is already on disk. Idempotent: applying an unchanged
/// publication is a no-op, never even opening the file for write.
///
/// Durable across a crash: a recovery record naming
/// `publication.previous_content` is written *before* `gamelist.xml` is
/// touched, and removed only after that write succeeds - see the module
/// doc comment's "Crash/restart durability" section. Refuses outright if
/// an unresolved recovery record already exists for this gamelist path
/// (run [`recover_es_de_gamelist_publication`] first).
pub fn apply_es_de_gamelist_publication(
    publication: &EsDeGamelistPublication,
) -> Result<(), EsDePublicationError> {
    if publication.is_unchanged() {
        return Ok(());
    }
    let recovery_path = es_de_gamelist_recovery_path(&publication.gamelist_path);
    if recovery_path.is_file() {
        return Err(EsDePublicationError::UnresolvedRecovery { recovery_path });
    }

    write_recovery_record(
        &recovery_path,
        &publication.gamelist_path,
        &publication.previous_content,
    )?;

    if let Err(error) =
        crate::atomic_write_text(&publication.gamelist_path, &publication.new_content)
    {
        // The gamelist write itself failed - leave the recovery record in
        // place. Nothing was actually changed, but a caller must still
        // resolve the record before trying again, per the module's
        // recovery policy (never race a second attempt against one that
        // did not provably finish).
        return Err(EsDePublicationError::Io {
            path: publication.gamelist_path.clone(),
            detail: error.to_string(),
        });
    }

    // Requirement: successful finalization must not leave ambiguous
    // stale recovery state.
    fs::remove_file(&recovery_path).map_err(|error| EsDePublicationError::Io {
        path: recovery_path.clone(),
        detail: error.to_string(),
    })
}

/// Restores exactly the state a durable recovery record for
/// `gamelist_path` describes, then discards the record - the only
/// resolution path while [`has_unresolved_es_de_gamelist_recovery`]
/// reports `true`. Reloads everything it needs purely from
/// `gamelist_path`, with no in-memory [`EsDeGamelistPublication`] value
/// required - safe to call after a full process restart. See the module
/// doc comment's "Recovery policy" for why this always restores rather
/// than trying to guess whether the interrupted write actually finished.
pub fn recover_es_de_gamelist_publication(
    gamelist_path: &Path,
) -> Result<(), EsDePublicationError> {
    let recovery_path = es_de_gamelist_recovery_path(gamelist_path);
    let record = read_recovery_record(&recovery_path, gamelist_path)?;
    restore_content(gamelist_path, &record.previous_content)?;
    fs::remove_file(&recovery_path).map_err(|error| EsDePublicationError::Io {
        path: recovery_path.clone(),
        detail: error.to_string(),
    })
}

/// Restores exactly the state captured in `publication` - never a rebuild,
/// never a heuristic. If the file did not exist before publication, it is
/// removed, but only when its current content still matches exactly what
/// [`apply_es_de_gamelist_publication`] wrote; if some other process or
/// person changed it since, this leaves it alone rather than guessing.
///
/// Also clears any recovery record left behind by an [`apply_es_de_gamelist_publication`]
/// call that was interrupted before reaching its own cleanup step, so a
/// caller may use this in place of [`recover_es_de_gamelist_publication`]
/// to abort a same-session, not-yet-restarted publication - both leave
/// the gamelist and its recovery record in the same resolved state.
pub fn rollback_es_de_gamelist_publication(
    publication: &EsDeGamelistPublication,
) -> Result<(), EsDePublicationError> {
    if publication.is_unchanged() {
        return Ok(());
    }
    match &publication.previous_content {
        Some(text) => {
            crate::atomic_write_text(&publication.gamelist_path, text).map_err(|error| {
                EsDePublicationError::Io {
                    path: publication.gamelist_path.clone(),
                    detail: error.to_string(),
                }
            })?
        }
        None => match fs::read_to_string(&publication.gamelist_path) {
            Ok(current) if current == publication.new_content => {
                fs::remove_file(&publication.gamelist_path).map_err(|error| {
                    EsDePublicationError::Io {
                        path: publication.gamelist_path.clone(),
                        detail: error.to_string(),
                    }
                })?
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(EsDePublicationError::Io {
                    path: publication.gamelist_path.clone(),
                    detail: error.to_string(),
                });
            }
        },
    }
    let recovery_path = es_de_gamelist_recovery_path(&publication.gamelist_path);
    match fs::remove_file(&recovery_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(EsDePublicationError::Io {
            path: recovery_path,
            detail: error.to_string(),
        }),
    }
}

/// Writes a durable recovery record for `gamelist_path`, capturing
/// `previous_content`. Called before every real `gamelist.xml` write -
/// see the module doc comment's "Crash/restart durability" section.
fn write_recovery_record(
    recovery_path: &Path,
    gamelist_path: &Path,
    previous_content: &Option<String>,
) -> Result<(), EsDePublicationError> {
    let record = EsDeGamelistRecoveryRecord {
        schema_version: RECOVERY_SCHEMA_VERSION,
        gamelist_path: gamelist_path.to_path_buf(),
        previous_content: previous_content.clone(),
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
    let body = fs::read_to_string(recovery_path).map_err(|error| EsDePublicationError::Io {
        path: recovery_path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let record: EsDeGamelistRecoveryRecord =
        serde_json::from_str(&body).map_err(|error| EsDePublicationError::RecoveryCorrupt {
            path: recovery_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    if record.schema_version != RECOVERY_SCHEMA_VERSION {
        return Err(EsDePublicationError::RecoveryCorrupt {
            path: recovery_path.to_path_buf(),
            detail: format!(
                "unrecognised recovery schema version {}",
                record.schema_version
            ),
        });
    }
    if record.gamelist_path != expected_gamelist_path {
        return Err(EsDePublicationError::RecoveryPathMismatch {
            recovery_path: recovery_path.to_path_buf(),
            expected: expected_gamelist_path.to_path_buf(),
            recorded: record.gamelist_path,
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
    fs::read_to_string(path).map_err(|error| EsDePublicationError::GamelistUnreadable {
        path: path.clone(),
        detail: error.to_string(),
    })
}

/// The byte offset immediately before the document's `</gameList>` closing
/// tag - the one, unambiguous, safe place to insert new `<game>` blocks
/// without touching a single byte of existing content. Refuses (never
/// guesses) when that tag does not appear exactly once.
fn locate_insertion_point(text: &str, path: &PathBuf) -> Result<usize, EsDePublicationError> {
    const CLOSING_TAG: &str = "</gameList>";
    let mut matches = text.match_indices(CLOSING_TAG);
    let Some((offset, _)) = matches.next() else {
        return Err(EsDePublicationError::MalformedGamelist {
            path: path.clone(),
            detail: "no </gameList> closing tag was found".to_string(),
        });
    };
    if matches.next().is_some() {
        return Err(EsDePublicationError::MalformedGamelist {
            path: path.clone(),
            detail: "more than one </gameList> closing tag was found".to_string(),
        });
    }
    Ok(offset)
}

#[cfg(test)]
mod tests;
