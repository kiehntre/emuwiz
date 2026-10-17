//! Proof that a PlayStation game is a single-disc game, from stated authorities.
//!
//! The DuckStation cheat planner refuses unless a game is *positively* shown to
//! be a single disc. That proof must not hinge on one source: a library whose
//! discs have never been matched to a DAT must still be usable when something
//! else authoritative speaks. So the proof is a small hierarchy of
//! [`DiscCountAuthority`] providers, each of which answers with a typed opinion:
//!
//! * [`TrustedDatAuthority`] - the library's trusted, persisted media-topology
//!   evidence (a proven set whose only member is this image).
//! * [`DuckStationGameDatabase`] - the DuckStation installation's own game
//!   database (`cache/gamedb.cache`): the serial is listed as a known game and
//!   is in none of its multi-disc sets. This is serial-keyed, so it works for
//!   discs that never matched a DAT hash (for example a CHD conversion).
//! * Any other authority implements the trait and is consulted the same way.
//!
//! # Why a positive claim from the game database is justified
//!
//! The database is DuckStation's own: its multi-disc *set table* is what
//! DuckStation itself uses to decide a game has several discs. A single-disc
//! claim therefore rests on two facts, both required: (1) the database
//! positively LISTS the serial as a game (a serial it has never heard of proves
//! nothing), and (2) the serial is in none of its sets. Coverage is not assumed
//! blindly: a database that is implausibly small for the real thing (a stub or
//! a partial download) is refused as a whole ([`MIN_DISC_SETS`],
//! [`MIN_LISTED_GAMES`]), and the real database has been cross-checked against
//! the Redump PlayStation DAT (more sets than the DAT has multi-disc groups) and
//! against the real library (every disc named like a multi-disc release
//! resolves to multi-disc or to "no verifiable serial", never to single).
//! If a multi-disc game were missing from the table, the only consequence of a
//! wrong single-disc claim is a cheat file for that one disc's serial.
//!
//! What is deliberately NOT evidence: filenames, "(Disc 1)" text, fuzzy
//! metadata, a lone file in a folder, or the mere absence of a record. An
//! authority that does not speak says *why* it did not
//! ([`MissingDiscProof`]), and that reason reaches the person.
//!
//! Combination is conservative: any authority naming a multi-disc set makes the
//! game multi-disc; a single-disc claim contradicted by a multi-disc claim is a
//! conflict and is refused; with no positive claim the result is unproven.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::duckstation_native::{DuckStationDiscTopology, DuckStationNativeRefusal};
use crate::media_set::{MediaSet, MediaSetConfidence, MediaSetState};

/// Where a disc-count statement came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DiscAuthority {
    /// A trusted DAT-backed media set the library persisted.
    TrustedDatSet { set_id: String },
    /// The DuckStation installation's own game database.
    DuckStationGameDatabase { database: PathBuf, version: u32 },
    /// Any further provider.
    Other { name: String, version: String },
}

impl DiscAuthority {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::TrustedDatSet { set_id } => format!("trusted DAT set {set_id}"),
            Self::DuckStationGameDatabase { database, version } => {
                format!(
                    "DuckStation's game database v{version} ({})",
                    database.display()
                )
            }
            Self::Other { name, version } => format!("{name} {version}"),
        }
    }
}

/// Why one proof route did not establish single-disc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MissingDiscProof {
    /// The disc's own PS1 serial has not been verified.
    NoVerifiedSerial,
    /// The library has no trusted DAT media set for this image.
    NoTrustedDatSet,
    /// A DAT set exists but is not a proven one-disc set.
    DatSetNotProvenSingleDisc,
    /// The DuckStation game database could not be used.
    GameDatabaseUnavailable { reason: String },
    /// The database is fine but does not list this serial.
    SerialNotInGameDatabase { database: PathBuf },
    /// A further provider declined.
    ProviderNoOpinion { name: String, reason: String },
}

impl MissingDiscProof {
    #[must_use]
    pub fn explain(&self) -> String {
        match self {
            Self::NoVerifiedSerial => {
                "the disc's PS1 serial has not been verified from the disc itself".into()
            }
            Self::NoTrustedDatSet => {
                "no trusted DAT set in the library covers this image (it has not been matched to a DAT)".into()
            }
            Self::DatSetNotProvenSingleDisc => {
                "the library's DAT set for this image is not a proven one-disc set".into()
            }
            Self::GameDatabaseUnavailable { reason } => {
                format!("DuckStation's game database could not be used ({reason})")
            }
            Self::SerialNotInGameDatabase { database } => format!(
                "DuckStation's game database ({}) does not list this serial",
                database.display()
            ),
            Self::ProviderNoOpinion { name, reason } => format!("{name}: {reason}"),
        }
    }
}

/// One authority's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscOpinion {
    Single(DiscAuthority),
    Multi {
        authority: DiscAuthority,
        serials: Vec<String>,
    },
    NoOpinion(MissingDiscProof),
}

/// What an authority is asked about.
#[derive(Debug, Clone, Copy)]
pub struct DiscQuery<'a> {
    /// The verified PS1 serial of the disc image, if any.
    pub serial: Option<&'a str>,
    pub archive: &'a Path,
}

/// A source that can positively state a game's disc count. Implementations
/// must answer only from their own authoritative data.
pub trait DiscCountAuthority {
    fn assess(&self, query: &DiscQuery<'_>) -> DiscOpinion;
}

/// The combined, explainable result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DiscTopologyProof {
    SingleDisc {
        authority: DiscAuthority,
    },
    MultiDisc {
        authority: DiscAuthority,
        serials: Vec<String>,
    },
    Conflict {
        detail: String,
    },
    Unproven {
        missing: Vec<MissingDiscProof>,
    },
}

impl DiscTopologyProof {
    /// The coarse topology the planner understands.
    #[must_use]
    pub fn topology(&self) -> DuckStationDiscTopology {
        match self {
            Self::SingleDisc { .. } => DuckStationDiscTopology::SingleDisc,
            Self::MultiDisc { .. } => DuckStationDiscTopology::MultiDisc,
            Self::Conflict { .. } | Self::Unproven { .. } => DuckStationDiscTopology::Unknown,
        }
    }

    /// The typed refusal for a proof that does not allow cheats, naming WHICH
    /// proof is missing.
    #[must_use]
    pub fn refusal(&self) -> Option<DuckStationNativeRefusal> {
        match self {
            Self::SingleDisc { .. } => None,
            Self::MultiDisc { .. } => Some(DuckStationNativeRefusal::UnsupportedMultiDisc),
            Self::Conflict { detail } => Some(DuckStationNativeRefusal::DiscAuthoritiesConflict {
                detail: detail.clone(),
            }),
            Self::Unproven { missing } => Some(DuckStationNativeRefusal::DiscProofMissing {
                missing: missing.iter().map(MissingDiscProof::explain).collect(),
            }),
        }
    }

    /// One line for the UI.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::SingleDisc { authority } => {
                format!("Single disc confirmed by {}", authority.label())
            }
            Self::MultiDisc { serials, .. } => format!(
                "Part of a multi-disc set ({}) - not supported yet",
                serials.join(", ")
            ),
            Self::Conflict { detail } => format!("Sources disagree: {detail}"),
            Self::Unproven { missing } => format!(
                "Single disc not confirmed: {}",
                missing
                    .iter()
                    .map(MissingDiscProof::explain)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        }
    }
}

/// Asks every authority and combines the answers (see the module docs).
#[must_use]
pub fn prove_disc_topology(
    query: &DiscQuery<'_>,
    authorities: &[&dyn DiscCountAuthority],
) -> DiscTopologyProof {
    let mut single: Option<DiscAuthority> = None;
    let mut multi: Option<(DiscAuthority, Vec<String>)> = None;
    let mut missing = Vec::new();
    if query.serial.is_none() {
        missing.push(MissingDiscProof::NoVerifiedSerial);
    }
    for authority in authorities {
        match authority.assess(query) {
            DiscOpinion::Single(source) => {
                single.get_or_insert(source);
            }
            DiscOpinion::Multi { authority, serials } => {
                multi.get_or_insert((authority, serials));
            }
            DiscOpinion::NoOpinion(reason) => {
                if !missing.contains(&reason) {
                    missing.push(reason);
                }
            }
        }
    }
    match (single, multi) {
        (Some(single), Some((multi, _))) => DiscTopologyProof::Conflict {
            detail: format!(
                "{} says one disc but {} says a multi-disc set",
                single.label(),
                multi.label()
            ),
        },
        (None, Some((authority, serials))) => DiscTopologyProof::MultiDisc { authority, serials },
        // A single-disc claim needs the disc's own verified serial too.
        (Some(authority), None) if query.serial.is_some() => {
            DiscTopologyProof::SingleDisc { authority }
        }
        (Some(_), None) | (None, None) => DiscTopologyProof::Unproven { missing },
    }
}

// ---------------------------------------------------------------------------
// Authority 1: the library's trusted DAT media sets
// ---------------------------------------------------------------------------

/// The library's persisted, trusted media-topology evidence.
pub struct TrustedDatAuthority<'a> {
    pub sets: &'a [MediaSet],
}

impl DiscCountAuthority for TrustedDatAuthority<'_> {
    fn assess(&self, query: &DiscQuery<'_>) -> DiscOpinion {
        let Some(set) = self.sets.iter().find(|set| {
            set.members.iter().any(|member| {
                member
                    .representations
                    .iter()
                    .any(|representation| representation.record.source.path == query.archive)
            })
        }) else {
            return DiscOpinion::NoOpinion(MissingDiscProof::NoTrustedDatSet);
        };
        let expected = set.expected_count.as_ref().map(|(count, _)| count.count);
        let set_id = set.identity.key.value.clone();
        if set.members.len() > 1 || expected.is_some_and(|count| count > 1) {
            return DiscOpinion::Multi {
                authority: DiscAuthority::TrustedDatSet { set_id },
                serials: Vec::new(),
            };
        }
        let proven = set.confidence == MediaSetConfidence::Proven
            && set.identity.verified
            && set.state != MediaSetState::ConflictingSet
            && set.state != MediaSetState::AmbiguousSet
            && !set.conflicts.iter().any(|conflict| conflict.blocking)
            && set.members.len() == 1
            && expected == Some(1);
        if proven {
            DiscOpinion::Single(DiscAuthority::TrustedDatSet { set_id })
        } else {
            DiscOpinion::NoOpinion(MissingDiscProof::DatSetNotProvenSingleDisc)
        }
    }
}

// ---------------------------------------------------------------------------
// Authority 2: DuckStation's own game database
// ---------------------------------------------------------------------------

const GAMEDB_SIGNATURE: &[u8; 4] = b"HLCE";
/// The only cache layout version this reader understands. Anything else is
/// "unavailable", never guessed at.
const GAMEDB_VERSION: u32 = 33;
const GAMEDB_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// A database with fewer multi-disc sets than this is a stub or partial copy
/// (the real one has hundreds); its silence about a game proves nothing.
pub const MIN_DISC_SETS: usize = 100;
/// Likewise for the number of games it lists (the real one has over ten thousand).
pub const MIN_LISTED_GAMES: usize = 2_000;
const MAX_SETS: u32 = 8192;
const MAX_SERIALS_PER_SET: u32 = 64;
const MAX_STRING_BYTES: u32 = 1024;

/// DuckStation's game database as stored in the profile's `cache` folder:
/// the multi-disc sets it knows and the serials it lists as games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckStationGameDatabase {
    pub path: PathBuf,
    pub version: u32,
    /// Each set's serials.
    pub disc_sets: Vec<Vec<String>>,
    /// Every serial the database lists as a game.
    pub known_serials: BTreeSet<String>,
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Option<u32> {
        let bytes = self.data.get(self.at..self.at + 4)?;
        self.at += 4;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        let length = self.u32()?;
        if length > MAX_STRING_BYTES {
            return None;
        }
        let bytes = self.data.get(self.at..self.at + length as usize)?;
        self.at += length as usize;
        Some(bytes)
    }
}

fn is_ps1_serial_bytes(bytes: &[u8]) -> bool {
    bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_uppercase)
        && bytes[4] == b'-'
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

/// A disc-set member: upstream data occasionally holds a non-standard serial
/// (for example `PSRM-017100`), so sets accept any short upper-case/digit/dash
/// token; only the strict 10-character shape counts as a *listed game*.
fn is_set_serial_bytes(bytes: &[u8]) -> bool {
    (5..=16).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'-')
}

impl DuckStationGameDatabase {
    /// The cache path inside a DuckStation profile (honouring `[Folders] Cache`).
    #[must_use]
    pub fn path_in_profile(profile_root: &Path) -> PathBuf {
        let configured = fs::read_to_string(profile_root.join("settings.ini"))
            .ok()
            .and_then(|text| super::duckstation_native::ini_value(&text, "Folders", "Cache").ok())
            .flatten()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let folder = match configured {
            Some(value) if Path::new(&value).is_absolute() => PathBuf::from(value),
            Some(value) => profile_root.join(value),
            None => profile_root.join("cache"),
        };
        folder.join("gamedb.cache")
    }

    /// Reads and strictly validates the cache. Any deviation from the layout
    /// this reader was verified against is an error, so an unfamiliar future
    /// format makes the authority silent instead of wrong.
    pub fn load(path: &Path) -> Result<Self, String> {
        let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > GAMEDB_MAX_BYTES {
            return Err("the game database is missing, oversized or not a file".into());
        }
        let mut data = Vec::new();
        fs::File::open(path)
            .and_then(|file| file.take(GAMEDB_MAX_BYTES).read_to_end(&mut data))
            .map_err(|error| error.to_string())?;
        Self::parse(path, &data)
    }

    pub fn parse(path: &Path, data: &[u8]) -> Result<Self, String> {
        Self::parse_with_floor(path, data, MIN_DISC_SETS, MIN_LISTED_GAMES)
    }

    /// [`Self::parse`] with explicit plausibility floors (tests use small ones
    /// for small synthetic caches; production uses the constants).
    pub fn parse_with_floor(
        path: &Path,
        data: &[u8],
        min_sets: usize,
        min_games: usize,
    ) -> Result<Self, String> {
        if data.get(..4) != Some(GAMEDB_SIGNATURE.as_slice()) {
            return Err("not a DuckStation game database cache".into());
        }
        let mut cursor = Cursor { data, at: 4 };
        let version = cursor.u32().ok_or("truncated header")?;
        if version != GAMEDB_VERSION {
            return Err(format!(
                "cache layout version {version} is not one EmuWiz can read (expected {GAMEDB_VERSION})"
            ));
        }
        cursor.at = 24; // two 64-bit stamps follow the version
        let set_count = cursor.u32().ok_or("truncated header")?;
        let _entry_count = cursor.u32().ok_or("truncated header")?;
        let _serial_count = cursor.u32().ok_or("truncated header")?;
        if set_count == 0 || set_count > MAX_SETS {
            return Err("implausible disc-set count".into());
        }
        let mut disc_sets = Vec::with_capacity(set_count as usize);
        for _ in 0..set_count {
            // name, sort title, native title, display title, then the serials.
            for _ in 0..4 {
                cursor.string().ok_or("truncated disc-set record")?;
            }
            let serial_count = cursor.u32().ok_or("truncated disc-set record")?;
            if serial_count == 0 || serial_count > MAX_SERIALS_PER_SET {
                return Err("implausible number of serials in a disc set".into());
            }
            let mut serials = Vec::new();
            for _ in 0..serial_count {
                let bytes = cursor.string().ok_or("truncated disc-set record")?;
                if !is_set_serial_bytes(bytes) {
                    return Err("a disc-set member is not serial-shaped".into());
                }
                serials.push(String::from_utf8_lossy(bytes).into_owned());
            }
            disc_sets.push(serials);
        }
        // The game entries start right after the sets; the first thing in an
        // entry is its serial. If that is not what follows, the layout is not
        // the one verified, so refuse to use the file at all.
        let entries_start = cursor.at;
        let mut probe = Cursor {
            data,
            at: entries_start,
        };
        match probe.string() {
            Some(first) if is_ps1_serial_bytes(first) => {}
            _ => return Err("the game entries do not follow the disc sets".into()),
        }
        // Serials the database lists as games: length-prefixed serial strings
        // in the entries section.
        let mut known_serials = BTreeSet::new();
        let rest = &data[entries_start..];
        let mut index = 0;
        while index + 14 <= rest.len() {
            if rest[index..index + 4] == [10, 0, 0, 0]
                && is_ps1_serial_bytes(&rest[index + 4..index + 14])
            {
                known_serials
                    .insert(String::from_utf8_lossy(&rest[index + 4..index + 14]).into_owned());
                index += 14;
            } else {
                index += 1;
            }
        }
        if known_serials.is_empty() {
            return Err("the game database lists no games".into());
        }
        if disc_sets.len() < min_sets || known_serials.len() < min_games {
            return Err(format!(
                "the game database looks incomplete ({} disc sets, {} games; expected at least {min_sets} and {min_games}), so its silence about a game proves nothing",
                disc_sets.len(),
                known_serials.len()
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            version,
            disc_sets,
            known_serials,
        })
    }
}

/// [`DuckStationGameDatabase`] as an authority. Loading is up to the caller
/// (so a failure becomes a stated reason, not a silent skip).
pub struct DuckStationDatabaseAuthority {
    pub path: PathBuf,
    pub database: Result<DuckStationGameDatabase, String>,
}

impl DuckStationDatabaseAuthority {
    #[must_use]
    pub fn for_profile(profile_root: &Path) -> Self {
        let path = DuckStationGameDatabase::path_in_profile(profile_root);
        let database = DuckStationGameDatabase::load(&path);
        Self { path, database }
    }
}

impl DiscCountAuthority for DuckStationDatabaseAuthority {
    fn assess(&self, query: &DiscQuery<'_>) -> DiscOpinion {
        let database = match &self.database {
            Ok(database) => database,
            Err(reason) => {
                return DiscOpinion::NoOpinion(MissingDiscProof::GameDatabaseUnavailable {
                    reason: reason.clone(),
                });
            }
        };
        let Some(serial) = query.serial else {
            return DiscOpinion::NoOpinion(MissingDiscProof::NoVerifiedSerial);
        };
        let authority = DiscAuthority::DuckStationGameDatabase {
            database: database.path.clone(),
            version: database.version,
        };
        if let Some(set) = database
            .disc_sets
            .iter()
            .find(|set| set.iter().any(|candidate| candidate == serial))
        {
            return DiscOpinion::Multi {
                authority,
                serials: set.clone(),
            };
        }
        // Not in any set: single-disc only if the database positively lists
        // this game. A serial it has never heard of proves nothing.
        if database.known_serials.contains(serial) {
            DiscOpinion::Single(authority)
        } else {
            DiscOpinion::NoOpinion(MissingDiscProof::SerialNotInGameDatabase {
                database: database.path.clone(),
            })
        }
    }
}

#[cfg(test)]
#[path = "duckstation_disc_authority_tests.rs"]
mod tests;
