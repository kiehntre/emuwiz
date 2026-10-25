//! Neo Geo MVS/AES multi-ROM set coherence: a thin, read-only projection.
//!
//! Neo Geo MVS/AES software identity is a multi-ROM set problem, not a
//! single-file cartridge header (`docs/research/STRANGE_CARTRIDGE_IDENTITY_AUDIT.md`).
//! The generic set-completeness/dependency engine that answers "is this
//! catalogue set's storage complete" already exists and is fully reused
//! here, unmodified:
//!
//! - [`crate::dat::set`] (Stage 2c): `Complete`/`Incomplete`/`BadMetadata`/
//!   `NeedsReview` set storage classification from MAME/FBNeo/Logiqx DAT
//!   evidence, with `nodump`/`baddump` handling and ambiguous-attribution
//!   refusal already implemented.
//! - [`crate::dat::dependency`] (Stage 2d): `cloneof`/`romof`/BIOS/device/
//!   parent-CHD dependency resolution, with BIOS (`DependencyKind::Bios`)
//!   modelled as a *separate* requirement kind from the parent/clone
//!   relationship (`DependencyKind::ParentSet`/`RomSource`) - never bundled
//!   into the game's own storage completeness.
//! - [`crate::dat::sources::audit_run::run_dat_audit`]: bounded, read-only
//!   inspection of one directory or one archive (its own `scan_root` doc:
//!   "a single-file target is useful for a deliberately bounded evidence
//!   check"), reusing the existing archive-member infrastructure and
//!   safety limits - no new archive extraction logic.
//!
//! This module adds no second implementation of any of that. It adds only
//! what is genuinely missing:
//!
//! 1. **ROM hardware-role evidence** ([`NeoGeoRomRole`], [`neogeo_rom_role`]):
//!    a MAME `-listxml` `<rom region="...">` value is real, authoritative
//!    evidence of which CPU/memory region a ROM loads into. The parser did
//!    not previously capture it (`dat::parsers::mame_listxml`, fixed
//!    alongside this module); this maps the real `neogeo.cpp` driver region
//!    names to the P/S/M/V/C role vocabulary. A role is `None` whenever
//!    `region` is absent or unrecognised - never guessed from a filename.
//! 2. **The task's five-state coherence vocabulary** ([`NeoGeoCoherence`],
//!    [`neogeo_coherence`]): a documented, honest relabelling of the
//!    existing [`SetState`]/[`NeedsReviewReason`] variants, not a new
//!    completeness rule.
//! 3. **An itemised BIOS-only status** ([`NeoGeoBiosStatus`],
//!    [`neogeo_bios_status`]): [`SetDependencyReport`] already tracks BIOS
//!    as its own [`DependencyKind::Bios`] requirement; this extracts just
//!    that kind's outcome so a caller can report "game set: Incomplete,
//!    BIOS: Ready" as two separate facts, per Task E.
//! 4. **A Neo Geo driver gate** ([`is_neogeo_mame_driver`]): MAME's own
//!    `sourcefile="neogeo.cpp"` is real, stable, authoritative driver
//!    evidence - proof this `<machine>` genuinely is Neo Geo MVS/AES
//!    hardware, never inferred from a game/set/file name. FBNeo DATs are
//!    not currently proven to carry an equivalent field in this codebase;
//!    a caller with independent authoritative FBNeo driver evidence may
//!    still call [`project_neogeo_set`] directly.
//! 5. **One typed projection** ([`NeoGeoSetSummary`],
//!    [`project_neogeo_set`]) assembling the above over an already-computed
//!    `(DatGameEntry, SetResolution)` pair - pure, no I/O, no re-hashing, no
//!    re-classification.
//!
//! # MVS vs AES - deliberately not modelled as a boolean
//!
//! A Neo Geo ROM set's own members (P/S/M/V/C ROMs) do not, by themselves,
//! uniquely prove MVS (arcade) vs AES (home) mode - that distinction lives
//! in the *BIOS*/driver context, not the game cartridge contents. This
//! module therefore never asserts an `is_mvs`/`is_aes` field: it exposes the
//! Neo Geo cartridge family plus the DAT's own `romof` (ROM-source/BIOS
//! set name) as plain, separate evidence, and leaves any MVS/AES
//! distinction to whatever authoritative BIOS-set evidence a caller already
//! has - never inferred from a folder name like `mvs`/`aes` (Task F).

use serde::Serialize;

use super::dependency::{DependencyKind, DependencyOutcome, SetDependencyReport};
use super::model::DatGameEntry;
use super::set::{NeedsReviewReason, SetResolution, SetState};

/// Real MAME `neogeo.cpp` driver source-file names. `neogeo.c` is the
/// pre-2016 filename MAME used before its C++ driver rename; both are
/// checked so an older DAT snapshot is still recognised.
const NEOGEO_MAME_SOURCE_FILES: &[&str] = &["neogeo.cpp", "neogeo.c"];

/// Whether `source_file` (a DAT `<machine sourcefile="...">` value) proves
/// this entry is Neo Geo MVS/AES arcade hardware. This is the *only*
/// authoritative signal this module uses to gate Neo Geo classification -
/// never a game name, set name, or containing folder/archive name.
pub fn is_neogeo_mame_driver(source_file: Option<&str>) -> bool {
    source_file.is_some_and(|value| {
        let base = value.rsplit(['/', '\\']).next().unwrap_or(value);
        NEOGEO_MAME_SOURCE_FILES
            .iter()
            .any(|driver| base.eq_ignore_ascii_case(driver))
    })
}

/// One ROM's conceptual hardware role inside a Neo Geo MVS/AES set, proven
/// only by the DAT's own `region=` attribute. See the module doc's point 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NeoGeoRomRole {
    /// `maincpu` region - the 68000 program ("P") ROM(s).
    Program,
    /// `fixed`/`fixedbios` region - the fixed (text/HUD) layer ("S") ROM.
    FixedLayer,
    /// `audiocpu` region - the Z80 sound-driver program ("M") ROM.
    SoundProgram,
    /// `ymsnd*` regions - the YM2610 ADPCM sample ("V") ROM(s).
    AdpcmSamples,
    /// `sprites` region - the sprite/graphics ("C") ROM(s).
    Graphics,
    /// A recognised Neo Geo/BIOS-scoped region name (e.g. `mainbios`,
    /// `audiobios`, `zoomy`, `audiocrypt`) that is real MAME evidence but
    /// does not map to one of the game-cartridge P/S/M/V/C roles above.
    Other,
}

impl NeoGeoRomRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Program => "Program (P)",
            Self::FixedLayer => "Fixed layer (S)",
            Self::SoundProgram => "Sound program (M)",
            Self::AdpcmSamples => "ADPCM samples (V)",
            Self::Graphics => "Graphics (C)",
            Self::Other => "Other region",
        }
    }
}

/// Maps a DAT `<rom region="...">` value (case-insensitive) to a
/// [`NeoGeoRomRole`], from the real MAME `neogeo.cpp` driver's own region
/// names. `None` when `region` is empty or not a recognised Neo Geo region -
/// never a guess, and never derived from the ROM's own file name.
pub fn neogeo_rom_role(region: &str) -> Option<NeoGeoRomRole> {
    Some(match region.to_ascii_lowercase().as_str() {
        "maincpu" => NeoGeoRomRole::Program,
        "fixed" | "fixedbios" => NeoGeoRomRole::FixedLayer,
        "audiocpu" => NeoGeoRomRole::SoundProgram,
        "ymsnd" | "ymsnd.deltat" | "ymsnd:adpcma" | "ymsnd:adpcmb" => NeoGeoRomRole::AdpcmSamples,
        "sprites" => NeoGeoRomRole::Graphics,
        "mainbios" | "audiobios" | "zoomy" | "audiocrypt" => NeoGeoRomRole::Other,
        _ => return None,
    })
}

/// The task's own five-state multi-ROM set coherence vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NeoGeoCoherence {
    /// All required members verified present - [`SetState::Complete`].
    Complete,
    /// A known set with required member(s) missing -
    /// [`SetState::Incomplete`].
    Incomplete,
    /// The DAT itself declares this set's data unverifiable/known-bad
    /// (`nodump`/`baddump`) - [`SetState::BadMetadata`]. This is the
    /// existing engine's closest proven concept to "conflicting metadata";
    /// it is not a claim that two specific members were cross-checked
    /// against each other, only that the catalogue cannot vouch for this
    /// set's own declared content.
    Conflicting,
    /// Evidence genuinely fits more than one named DAT candidate -
    /// [`NeedsReviewReason::AmbiguousMemberAttribution`],
    /// [`NeedsReviewReason::AmbiguousDependency`], or
    /// [`NeedsReviewReason::DuplicateGameName`]. Never auto-resolved to one
    /// winner (Task C).
    Ambiguous,
    /// Every other [`NeedsReviewReason`] - the parser/member shape could
    /// not be represented or verified safely (contradictory flags, unknown
    /// loadflag, unsupported dependency structure, an incomplete archive
    /// pass, ...). Genuinely insufficient/untrustworthy evidence, distinct
    /// from a real multi-candidate ambiguity.
    Unknown,
}

/// Maps the existing [`SetState`] to [`NeoGeoCoherence`]. Pure relabelling -
/// see [`NeoGeoCoherence`]'s own variant docs for the exact mapping and its
/// reasoning.
pub fn neogeo_coherence(state: &SetState) -> NeoGeoCoherence {
    match state {
        SetState::Complete => NeoGeoCoherence::Complete,
        SetState::Incomplete => NeoGeoCoherence::Incomplete,
        SetState::BadMetadata(_) => NeoGeoCoherence::Conflicting,
        SetState::NeedsReview(reason) => match reason {
            NeedsReviewReason::AmbiguousMemberAttribution
            | NeedsReviewReason::AmbiguousDependency
            | NeedsReviewReason::DuplicateGameName => NeoGeoCoherence::Ambiguous,
            _ => NeoGeoCoherence::Unknown,
        },
    }
}

/// The Neo Geo BIOS dependency's own status, kept entirely separate from
/// [`NeoGeoCoherence`] (the *game* set's own storage completeness) - see
/// Task E and the module doc's point 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NeoGeoBiosStatus {
    /// This set declares no BIOS dependency at all (e.g. it is itself a
    /// BIOS set, or the DAT records no such relationship for it).
    NotApplicable,
    /// Every BIOS requirement resolved [`DependencyOutcome::Satisfied`].
    Ready,
    /// At least one BIOS requirement resolved
    /// [`DependencyOutcome::Missing`], and none resolved anything less
    /// certain than that.
    Missing,
    /// The BIOS dependency could not be confidently resolved either way
    /// (ambiguous, contradictory, unsupported, or the scan that would have
    /// proven it did not finish).
    Unknown,
}

/// Extracts just the [`DependencyKind::Bios`] requirement(s)' rolled-up
/// status from an already-computed [`SetDependencyReport`] - never
/// re-resolves dependencies itself.
pub fn neogeo_bios_status(report: &SetDependencyReport) -> NeoGeoBiosStatus {
    let bios_outcomes: Vec<DependencyOutcome> = report
        .requirements
        .iter()
        .filter(|requirement| requirement.kind == DependencyKind::Bios)
        .map(|requirement| requirement.outcome)
        .collect();
    if bios_outcomes.is_empty() {
        return NeoGeoBiosStatus::NotApplicable;
    }
    if bios_outcomes
        .iter()
        .all(|outcome| *outcome == DependencyOutcome::Satisfied)
    {
        return NeoGeoBiosStatus::Ready;
    }
    if bios_outcomes.iter().all(|outcome| {
        matches!(
            outcome,
            DependencyOutcome::Satisfied | DependencyOutcome::Missing
        )
    }) {
        return NeoGeoBiosStatus::Missing;
    }
    NeoGeoBiosStatus::Unknown
}

/// One declared ROM member's coherence view: whether the archive/directory
/// under inspection proved it present, and its authoritative role when the
/// DAT's `region` establishes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeoGeoSetMember {
    pub name: String,
    pub role: Option<NeoGeoRomRole>,
    /// `true` when this member is required for [`NeoGeoCoherence::Complete`]
    /// (excludes `nodump`/`baddump`/optional/borrowed members - see
    /// [`SetResolution::members_required`]).
    pub required: bool,
    /// `true` when this archive/directory's evidence verified this member
    /// present.
    pub present: bool,
    /// `true` when the DAT flags this member `nodump`/`baddump`.
    pub bad: bool,
    /// `true` when this member is borrowed from a parent/dependency set
    /// rather than required locally.
    pub borrowed: bool,
}

/// One Neo Geo MVS/AES software set's complete, read-only coherence view -
/// a pure projection, never a second identity/completeness engine. See the
/// module doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NeoGeoSetSummary {
    /// The canonical DAT `<game name>` - never a directory/archive name
    /// (Task A: "Do NOT derive canonical identity from directory name
    /// alone").
    pub set_name: String,
    /// The DAT's own human-readable `<description>`, when present.
    pub display_name: Option<String>,
    /// Which DAT source this resolution came from - preserved so two
    /// disagreeing authoritative sources are never silently merged (Task
    /// H); a caller comparing MAME vs FBNeo results keeps them as separate
    /// [`NeoGeoSetSummary`] values, one per source.
    pub source_id: String,
    pub coherence: NeoGeoCoherence,
    pub bios_status: NeoGeoBiosStatus,
    /// The DAT's own `cloneof` parent set name, when this is a clone or
    /// revision. Never flattened into this set's own identity (Task D).
    pub parent_set: Option<String>,
    /// The DAT's own `romof` ROM-source set name, kept separate from
    /// `parent_set`: a `romof` target is frequently the BIOS-providing set
    /// rather than the gameplay parent (see
    /// [`crate::dat::dependency::DependencyKind`]'s own doc on why
    /// `ParentSet`/`RomSource` are never conflated).
    pub rom_source_set: Option<String>,
    pub members: Vec<NeoGeoSetMember>,
    pub members_required: usize,
    pub members_verified: usize,
}

/// Projects one already-computed `(DatGameEntry, SetResolution)` pair - both
/// produced by the existing DAT audit pipeline - into a [`NeoGeoSetSummary`].
/// `game` and `resolution` must describe the same catalogue set (same
/// `game.name` / `resolution.identity.game_name`); callers typically already
/// hold this pairing from indexing a `DatAuditOutcome`. Pure: no hashing, no
/// archive access, no re-classification.
pub fn project_neogeo_set(game: &DatGameEntry, resolution: &SetResolution) -> NeoGeoSetSummary {
    use std::collections::BTreeSet;

    let required: BTreeSet<&str> = resolution
        .members_required
        .iter()
        .map(String::as_str)
        .collect();
    let verified: BTreeSet<&str> = resolution
        .members_verified
        .iter()
        .map(String::as_str)
        .collect();
    let borrowed: BTreeSet<&str> = resolution
        .members_borrowed
        .iter()
        .map(String::as_str)
        .collect();
    let bad: BTreeSet<&str> = resolution
        .members_bad
        .iter()
        .map(|member| member.rom_name.as_str())
        .collect();

    let members = game
        .roms
        .iter()
        .map(|rom| NeoGeoSetMember {
            name: rom.name.clone(),
            role: rom.region.as_deref().and_then(neogeo_rom_role),
            required: required.contains(rom.name.as_str()),
            present: verified.contains(rom.name.as_str()),
            bad: bad.contains(rom.name.as_str()),
            borrowed: borrowed.contains(rom.name.as_str()),
        })
        .collect();

    NeoGeoSetSummary {
        set_name: resolution.identity.game_name.clone(),
        display_name: game.description.clone(),
        source_id: resolution.identity.source_id.clone(),
        coherence: neogeo_coherence(&resolution.state),
        bios_status: neogeo_bios_status(&resolution.dependencies),
        parent_set: game.clone_of.clone(),
        rom_source_set: game.rom_of.clone(),
        members,
        members_required: resolution.members_required.len(),
        members_verified: resolution.members_verified.len(),
    }
}

#[cfg(test)]
mod tests;
