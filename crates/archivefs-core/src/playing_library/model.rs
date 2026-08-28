//! The Playing Library planner model ("Build Playing Library").
//!
//! A playing library is a curated 1-game-1-ROM style view of an already
//! identified/verified archival collection: from each *authoritatively
//! related* game family, one representative release is elected and proposed
//! as a linked-library symlink pointing at the untouched source archive.
//! Nothing is moved, renamed, deleted, or copied; planning never touches the
//! filesystem at all.
//!
//! # Identity rule (read this before touching anything)
//!
//! Family grouping here is **not** identity inference. The only accepted
//! grouping evidence is a resolved parent/clone chain inside one parsed DAT
//! catalogue (`cloneof` / `cloneofid`), resolved through the crate's single
//! auditable name→identity conversion spot ([`crate::dat::dependency::DependencyGraph`]).
//! There is no filename-similarity, fuzzy title, or inferred-parent path in
//! this module, and there must never be one: two releases whose names merely
//! look alike stay separate families forever. A false negative (two variants
//! left as separate groups) is acceptable; a false grouping is not.
//!
//! Release evidence (region, revision, languages, release class) is read as
//! **strict parenthesized tokens** from the *provider-published* canonical
//! DAT entry name - the same discipline [`crate::dat::classification`]
//! already applies to multi-disc tokens. It is never read from a local
//! archive filename, because a local filename is not trusted metadata.
//!
//! # Election is explainable or it does not happen
//!
//! Every election carries step-by-step reasoning lines and per-rejected-
//! candidate reasons built from explicit policy comparisons. No opaque score
//! exists anywhere. If candidates remain indistinguishable after every
//! trusted policy field has been compared, the group is reported
//! unresolved - no alphabetical fallback, no arbitrary pick.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which non-retail release classes can be excluded from election.
///
/// Exclusion only ever happens when the DAT entry's own canonical name
/// carries the matching strict token (`(Beta)`, `(Proto)`, `(Demo)`,
/// `(Sample)`); absence of any token means "unknown status", which is never
/// treated as bad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseClass {
    Beta,
    Proto,
    Demo,
    Sample,
}

impl ReleaseClass {
    /// The exact delimited token that constitutes evidence for this class,
    /// matched case-insensitively against whole parenthesized/comma-separated
    /// tokens only.
    pub fn token(self) -> &'static str {
        match self {
            Self::Beta => "beta",
            Self::Proto => "proto",
            Self::Demo => "demo",
            Self::Sample => "sample",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Beta => "Beta",
            Self::Proto => "Proto",
            Self::Demo => "Demo",
            Self::Sample => "Sample",
        }
    }

    pub const fn all() -> [ReleaseClass; 4] {
        [Self::Beta, Self::Proto, Self::Demo, Self::Sample]
    }
}

/// A strictly parsed revision token: `(Rev 1)`, `(Rev 1.5)`, `(Rev 1A)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct RevisionNumber {
    pub major: u16,
    pub minor: u16,
    /// Suffix letter (`(Rev 1A)`), compared `0 < 'A'` so `Rev 1A` outranks
    /// plain `Rev 1` deterministically.
    pub letter: char,
}

/// One released ROM variant of a game family, as EmuWiz verified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayingLibraryCandidate {
    /// Index into `ParsedDat::games` of the DAT entry this archive was
    /// hash-matched to by the caller's trusted verification flow.
    pub dat_entry_index: usize,
    /// The untouched source archive the plan will point a symlink at.
    ///
    /// Planning copies this path verbatim into the proposal; nothing reads,
    /// writes, moves, or renames it.
    pub source_path: PathBuf,
}

/// The deterministic policy knobs for one playing library build.
///
/// Every field defaults to "no preference expressed" so that an all-default
/// policy elects nothing it cannot justify and excludes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlayingLibraryPolicy {
    /// Preferred regions, most-preferred first (for example
    /// `["Europe", "USA", "Japan"]`). Matched case-insensitively against
    /// recognized provider region tokens parsed from the DAT entry name.
    /// Empty means no preference is expressed and region contributes
    /// nothing to election.
    #[serde(default)]
    pub preferred_regions: Vec<String>,
    /// Preferred languages, most-preferred first. Same semantics as
    /// [`Self::preferred_regions`]; only populated when real language
    /// evidence `(En)`, `(Fr)`, ... exists in the catalogue naming.
    #[serde(default)]
    pub preferred_languages: Vec<String>,
    /// Prefer the newest *verified* revision (a strictly parsed `(Rev N)`
    /// / `(Rev N.M)` / `(Rev NA)` token). Disabled means revisions are
    /// simply not compared.
    #[serde(default)]
    pub prefer_newest_revision: bool,
    /// Prefer the family's declared parent entry over its clones, but only
    /// where an authoritative parent/clone relationship actually exists
    /// (which is the only relationship this model groups on at all).
    #[serde(default)]
    pub prefer_parent: bool,
    /// Release classes excluded from election - and only when the catalogued
    /// name explicitly carries the class token. Unknown release status is
    /// never excluded by anything in this list.
    #[serde(default)]
    pub excluded_release_classes: Vec<ReleaseClass>,
}

/// Why one candidate was rejected during election.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedCandidate {
    /// The canonical DAT entry name of the rejected candidate.
    pub dat_entry_name: String,
    pub source_path: PathBuf,
    /// Explicit human-readable reasons, one per decisive fact.
    pub reasons: Vec<String>,
}

/// How the elected candidate won, without any opaque number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectionExplanation {
    /// Ordered reasoning steps explaining why the winner wins, e.g.
    /// `"preferred region \"Europe\" ranked above ..."` then
    /// `"verified revision Rev 1 ranked above ..."`.
    pub steps: Vec<String>,
    /// Every other candidate with its explicit rejection reason(s).
    pub rejected: Vec<RejectedCandidate>,
}

/// One elected representative of one game family.
///
/// A release is not always one filesystem file: a CUE sheet needs its
/// referenced BIN/audio tracks, a GDI descriptor needs every track it
/// declares, and an M3U playlist needs each referenced disc plus that
/// disc's own companions. [`Self::launcher_operation`] is the one file a
/// frontend should be pointed at to play the release; every other
/// required file is [`Self::companion_operations`] - empty for an
/// ordinary single-file release (CHD, ISO, RVZ, a loose cartridge ROM, or
/// an archive), which behaves exactly as before this field existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectedGame {
    /// The canonical DAT entry name of the elected release.
    pub dat_entry_name: String,
    /// The family root's canonical DAT entry name.
    pub family_root_name: String,
    pub explanation: ElectionExplanation,
    /// The proposed linked-library operation for the file a frontend
    /// should launch: the CUE/GDI/M3U file itself for a multi-file
    /// release, or the sole file for an ordinary single-file release -
    /// see [`crate::launch::es_de_publish`], which points ES-DE at this
    /// path and never at a companion.
    pub launcher_operation: LinkedLibraryOperation,
    /// Every other file this release requires alongside
    /// [`Self::launcher_operation`] - referenced BIN/audio tracks for a
    /// CUE, every other track for a GDI, or each disc (and that disc's
    /// own companions) for an M3U. Always empty for an ordinary
    /// single-file release.
    pub companion_operations: Vec<LinkedLibraryOperation>,
}

impl ElectedGame {
    /// Every operation this election proposes, launcher first - the exact
    /// set [`crate::playing_library::apply_adapter::build_playing_library_transaction`]
    /// turns into linked-library symlinks, and the same set the planner's
    /// own destination-conflict check inspects.
    pub fn all_operations(&self) -> impl Iterator<Item = &LinkedLibraryOperation> {
        std::iter::once(&self.launcher_operation).chain(self.companion_operations.iter())
    }
}

/// A group that could not be elected deterministically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedGroup {
    pub family_root_name: String,
    /// Canonical DAT entry names of every equally-ranked candidate.
    pub tied_candidates: Vec<String>,
    pub reason: String,
}

/// One candidate deliberately left out of election because its DAT entry
/// name carries an explicitly excluded release-class token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedCandidate {
    pub dat_entry_name: String,
    pub source_path: PathBuf,
    /// The concrete release class(es) found and excluded, labelled.
    pub excluded_classes: Vec<String>,
}

/// A proposed non-destructive linked-library operation.
///
/// This is a *plan record*: applying it means creating `destination_path`
/// as a symlink to `source_path` via the existing linked-library apply
/// engine. The original file at `source_path` is never modified, renamed,
/// moved, or deleted by any consumer of this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedLibraryOperation {
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
}

/// One destination-name conflict found while planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationConflict {
    /// Case-collapsed destination file name the contenders share.
    pub destination_basename: String,
    /// Canonical DAT entry names competing for the same destination name.
    pub contenders: Vec<String>,
    /// The absolute destination paths involved (case-collapsed identical).
    pub destinations: Vec<PathBuf>,
}

/// The complete read-only result of one planning run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayingLibraryPlan {
    pub destination_root: PathBuf,
    pub policy: PlayingLibraryPolicy,
    /// Total matched archives examined.
    pub archives_examined: usize,
    /// Distinct authoritative game families among them.
    pub families_examined: usize,
    pub elected_games: Vec<ElectedGame>,
    pub unresolved_groups: Vec<UnresolvedGroup>,
    pub exclusions: Vec<ExcludedCandidate>,
    /// Archives in singleton families are always elected trivially; this
    /// counts them so callers can reconcile
    /// "2084 files -> XXX families -> YYY games".
    pub singleton_families: usize,
    pub conflicts: Vec<DestinationConflict>,
    /// Only conflict-free elections appear here, launcher and companion
    /// operations flattened together. Every operation points at an
    /// original source file; nothing else is ever produced.
    pub operations: Vec<LinkedLibraryOperation>,
    /// A CUE/GDI/M3U launcher file the planner found alongside matched
    /// candidates but could not safely turn into a multi-file election -
    /// a missing/unsafe/ambiguous companion reference, or companions that
    /// verify against more than one distinct DAT game. Never silently
    /// dropped: requirement 6's "plain reasons" land here rather than in
    /// [`ElectedGame::explanation`], since the launcher never became a
    /// candidate at all.
    pub rejected_launchers: Vec<RejectedLauncher>,
}

/// Why a discovered CUE/GDI/M3U launcher never became an election
/// candidate - see [`PlayingLibraryPlan::rejected_launchers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedLauncher {
    pub launcher_path: PathBuf,
    pub reason: String,
}
