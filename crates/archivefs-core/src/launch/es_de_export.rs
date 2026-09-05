//! Read-only ES-DE (EmulationStation Desktop Edition) export/launch-entry
//! plan - first slice.
//!
//! Given an already-resolved canonical platform identity
//! ([`CanonicalIdentityStatus`]) and an already-resolved content path
//! ([`LaunchContentRef`]) for one game, this module describes how that
//! game would appear as an ES-DE entry: which ES-DE system/folder it
//! belongs under, what its game path would be, whether that path is
//! already usable, whether the underlying content still needs mounting,
//! and - when a [`LaunchPlan`] for the same game is already available -
//! which emulator/core choice is already known.
//!
//! # What this module is not
//!
//! - It never writes `es_systems.xml` or `gamelist.xml` - see
//!   [`crate::emulator_environment::es_de`] for read-only *discovery* of an
//!   existing ES-DE install's own files, which this module does not touch
//!   or depend on.
//! - It never launches ES-DE or anything else - [`build_es_de_entry_plan`]
//!   is a pure function.
//! - It never mounts or extracts an archive.
//! - It never performs identity fusion or platform detection from a file
//!   extension - [`CanonicalIdentityStatus`] is consumed exactly as
//!   already resolved upstream, and [`ES_DE_SYSTEM_MAP`] is keyed on
//!   [`crate::platform::Platform::id`] only.
//!
//! # Reuse, not duplication
//!
//! This module deliberately does not redefine platform identity, content
//! resolution, or emulator/core selection - it consumes
//! [`crate::launch::planning::CanonicalIdentityStatus`],
//! [`crate::launch::planning::LaunchContentRef`], and (optionally)
//! [`crate::launch::planning::LaunchPlan`]/[`crate::launch::planning::LaunchTarget`]
//! exactly as [`crate::launch::planning::build_launch_plan`] already
//! produces them. Its own local [`EsDeEntryBlocker`]/
//! [`EsDeEntryBlockerKind`] vocabulary exists rather than extending
//! [`crate::launch::readiness::LaunchBlockerKind`] because this module
//! needs a distinction that vocabulary does not draw
//! (`ContentRequiresMount` vs. a bare unresolved path) - see
//! `emulator_environment::mod`'s own doc comment on why a second adapter
//! target keeps its own local vocabulary rather than forcing a shared
//! trait before a second real shape exists to justify one.
//!
//! # Fail-closed rules
//!
//! - [`CanonicalIdentityStatus::Unknown`]/[`CanonicalIdentityStatus::Conflicting`]
//!   -> [`EsDeExportOutcome::NoEntry`], never a guessed platform.
//! - A platform with no reviewed row in [`ES_DE_SYSTEM_MAP`] ->
//!   [`EsDeExportOutcome::NoEntry`] - this module never invents an ES-DE
//!   system short name.
//! - [`LaunchContentRef::has_runnable_path`] `false` because no path was
//!   ever resolved -> [`EsDeEntryPlan::status`] is
//!   [`EsDeEntryStatus::Blocked`] with [`EsDeEntryBlockerKind::ContentUnresolved`].
//! - [`LaunchContentRef::has_runnable_path`] `false` because the content is
//!   inside a container that needs mounting
//!   ([`LaunchContentRef::requires_mount`]) ->
//!   [`EsDeEntryStatus::Blocked`] with
//!   [`EsDeEntryBlockerKind::ContentRequiresMount`], with its own explicit
//!   detail text distinguishing it from a bare unresolved path.
//! - An emulator/core choice is only ever surfaced
//!   ([`EsDeEntryPlan::emulator_choice`]) when a caller-supplied
//!   [`LaunchPlan`] already names exactly one non-blocked, already-preferred
//!   candidate ([`CandidatePreference::Remembered`] or
//!   [`CandidatePreference::SoleEligible`]) - an
//!   [`CandidatePreference::Undetermined`] tie is never resolved into a
//!   guess here.
//!
//! # ES-DE system mapping table
//!
//! [`ES_DE_SYSTEM_MAP`] was checked against ES-DE's own reference
//! `resources/systems/linux/es_systems.xml`
//! (`gitlab.com/es-de/emulationstation-de`, `master` branch) on
//! 2026-08-23, reading the exact `<name>`/`<fullname>` pair for each
//! platform this milestone covers. One platform needed a documented
//! judgment call: ES-DE ships **two** distinct Mega Drive/Genesis system
//! folders in that reference file - `genesis` (`Sega Genesis`) and
//! `megadrive` (`Sega Mega Drive`), plus a Japan-only `megadrivejp`
//! variant - as region-labelled alternatives for the same hardware, not as
//! different platforms. EmuWiz's own canonical platform id is
//! `MegaDrive`, so [`ES_DE_SYSTEM_MAP`] maps it to
//! ES-DE's `megadrive` system for naming-convention consistency; this is a
//! documented, reviewed choice between two *equally valid* real ES-DE
//! system names for the one platform EmuWiz already resolved, not a guess
//! at platform identity itself.

use std::path::PathBuf;

use crate::launch::planning::{
    CandidatePreference, CanonicalIdentityStatus, LaunchContentRef, LaunchPlan, LaunchTarget,
};
use crate::launch::readiness::LaunchReadiness;

/// One reviewed row mapping a [`crate::platform::Platform::id`] to its
/// ES-DE system short name and full display name - see the module doc
/// comment for how each row was verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EsDeSystemMapping {
    /// A [`crate::platform::Platform::id`] value - never a new namespace.
    pub platform_id: &'static str,
    /// ES-DE's `<name>` value - the system's folder name under
    /// `%ROMPATH%` and its `gamelists`/`downloaded_media` subfolder name.
    pub es_de_system: &'static str,
    /// ES-DE's `<fullname>` value, preserved verbatim from the source file
    /// (including its own idiosyncrasies - see the module doc comment).
    pub es_de_fullname: &'static str,
}

/// The reviewed table. Deliberately incomplete: a platform absent here has
/// no ES-DE mapping in this build, and [`build_es_de_entry_plan`] fails
/// closed to [`EsDeExportOutcome::NoEntry`] rather than guessing one.
pub const ES_DE_SYSTEM_MAP: &[EsDeSystemMapping] = &[
    EsDeSystemMapping {
        platform_id: "PSX",
        es_de_system: "psx",
        es_de_fullname: "Sony PlayStation",
    },
    EsDeSystemMapping {
        platform_id: "PS2",
        es_de_system: "ps2",
        es_de_fullname: "Sony PlayStation 2",
    },
    EsDeSystemMapping {
        platform_id: "PS3",
        es_de_system: "ps3",
        es_de_fullname: "Sony PlayStation 3",
    },
    EsDeSystemMapping {
        platform_id: "PSP",
        es_de_system: "psp",
        es_de_fullname: "Sony PlayStation Portable",
    },
    EsDeSystemMapping {
        platform_id: "PlayStation Vita",
        es_de_system: "psvita",
        es_de_fullname: "Sony PlayStation Vita",
    },
    EsDeSystemMapping {
        platform_id: "Nintendo 3DS",
        es_de_system: "3ds",
        es_de_fullname: "Nintendo 3DS",
    },
    EsDeSystemMapping {
        platform_id: "Nintendo DS",
        es_de_system: "nds",
        es_de_fullname: "Nintendo DS",
    },
    EsDeSystemMapping {
        platform_id: "Xbox",
        es_de_system: "xbox",
        es_de_fullname: "Microsoft Xbox",
    },
    EsDeSystemMapping {
        platform_id: "Xbox360",
        es_de_system: "xbox360",
        es_de_fullname: "Microsoft Xbox 360",
    },
    EsDeSystemMapping {
        platform_id: "GameCube",
        es_de_system: "gc",
        es_de_fullname: "Nintendo GameCube",
    },
    EsDeSystemMapping {
        platform_id: "Wii",
        es_de_system: "wii",
        es_de_fullname: "Nintendo Wii",
    },
    EsDeSystemMapping {
        platform_id: "WiiU",
        es_de_system: "wiiu",
        es_de_fullname: "Nintendo Wii U",
    },
    EsDeSystemMapping {
        platform_id: "Dreamcast",
        es_de_system: "dreamcast",
        es_de_fullname: "Sega Dreamcast",
    },
    EsDeSystemMapping {
        platform_id: "Saturn",
        es_de_system: "saturn",
        es_de_fullname: "Sega Saturn",
    },
    EsDeSystemMapping {
        platform_id: "Sega CD",
        es_de_system: "segacd",
        es_de_fullname: "Sega CD",
    },
    EsDeSystemMapping {
        platform_id: "AtariST",
        es_de_system: "atarist",
        es_de_fullname: "Atari ST",
    },
    EsDeSystemMapping {
        platform_id: "Amiga",
        es_de_system: "amiga",
        es_de_fullname: "Commodore Amiga",
    },
    EsDeSystemMapping {
        platform_id: "AmigaCD32",
        es_de_system: "amigacd32",
        es_de_fullname: "Commodore Amiga CD32",
    },
    EsDeSystemMapping {
        platform_id: "Commodore CDTV",
        es_de_system: "cdtv",
        es_de_fullname: "Commodore CDTV",
    },
    EsDeSystemMapping {
        platform_id: "Arcade",
        es_de_system: "arcade",
        es_de_fullname: "Arcade",
    },
    EsDeSystemMapping {
        platform_id: "DOS",
        es_de_system: "dos",
        es_de_fullname: "DOS",
    },
    EsDeSystemMapping {
        platform_id: "ScummVM",
        es_de_system: "scummvm",
        es_de_fullname: "ScummVM",
    },
    EsDeSystemMapping {
        platform_id: "Game Boy",
        es_de_system: "gb",
        es_de_fullname: "Nintendo Game Boy",
    },
    EsDeSystemMapping {
        platform_id: "Game Boy Color",
        es_de_system: "gbc",
        es_de_fullname: "Nintendo Game Boy Color",
    },
    EsDeSystemMapping {
        platform_id: "Game Boy Advance",
        es_de_system: "gba",
        es_de_fullname: "Nintendo Game Boy Advance",
    },
    EsDeSystemMapping {
        platform_id: "NES",
        es_de_system: "nes",
        es_de_fullname: "Nintendo Entertainment System",
    },
    EsDeSystemMapping {
        platform_id: "SNES",
        es_de_system: "snes",
        // Preserved exactly as ES-DE's own reference file spells it - see
        // the module doc comment.
        es_de_fullname: "Nintendo Super Entertainment System",
    },
    EsDeSystemMapping {
        platform_id: "MegaDrive",
        es_de_system: "megadrive",
        es_de_fullname: "Sega Mega Drive",
    },
    EsDeSystemMapping {
        platform_id: "N64",
        es_de_system: "n64",
        es_de_fullname: "Nintendo 64",
    },
    // --- Batch 1 additions (docs/ESDE_PARITY_AUDIT.md §10) -------------
    // Every `es_de_system`/`es_de_fullname` pair below was read verbatim
    // from ES-DE's own upstream `resources/systems/linux/es_systems.xml`
    // (gitlab.com/es-de/emulationstation-de, `master` branch) - never
    // guessed from a plausible-looking short name.
    EsDeSystemMapping {
        platform_id: "MasterSystem",
        es_de_system: "mastersystem",
        es_de_fullname: "Sega Master System",
    },
    EsDeSystemMapping {
        platform_id: "GameGear",
        es_de_system: "gamegear",
        es_de_fullname: "Sega Game Gear",
    },
    EsDeSystemMapping {
        platform_id: "Sega 32X",
        es_de_system: "sega32x",
        es_de_fullname: "Sega 32X",
    },
    EsDeSystemMapping {
        platform_id: "Atari2600",
        es_de_system: "atari2600",
        es_de_fullname: "Atari 2600",
    },
    EsDeSystemMapping {
        platform_id: "Atari5200",
        es_de_system: "atari5200",
        es_de_fullname: "Atari 5200",
    },
    EsDeSystemMapping {
        platform_id: "Atari7800",
        es_de_system: "atari7800",
        es_de_fullname: "Atari 7800 ProSystem",
    },
    EsDeSystemMapping {
        platform_id: "Atari Lynx",
        es_de_system: "atarilynx",
        es_de_fullname: "Atari Lynx",
    },
    EsDeSystemMapping {
        platform_id: "Atari Jaguar",
        es_de_system: "atarijaguar",
        es_de_fullname: "Atari Jaguar",
    },
    EsDeSystemMapping {
        platform_id: "ColecoVision",
        es_de_system: "colecovision",
        es_de_fullname: "Coleco ColecoVision",
    },
    EsDeSystemMapping {
        platform_id: "Vectrex",
        es_de_system: "vectrex",
        es_de_fullname: "Vectrex",
    },
    EsDeSystemMapping {
        platform_id: "Neo Geo Pocket",
        es_de_system: "ngp",
        es_de_fullname: "SNK Neo Geo Pocket",
    },
    EsDeSystemMapping {
        platform_id: "Neo Geo Pocket Color",
        es_de_system: "ngpc",
        es_de_fullname: "SNK Neo Geo Pocket Color",
    },
    // --- Batch 3 additions (docs/ESDE_PARITY_AUDIT.md §10) -------------
    // Every `es_de_system`/`es_de_fullname` pair below was read verbatim
    // from ES-DE's own upstream `resources/systems/linux/es_systems.xml`
    // (gitlab.com/es-de/emulationstation-de, `master` branch) - never
    // guessed from a plausible-looking short name.
    EsDeSystemMapping {
        platform_id: "Virtual Boy",
        es_de_system: "virtualboy",
        es_de_fullname: "Nintendo Virtual Boy",
    },
    EsDeSystemMapping {
        platform_id: "MSX",
        es_de_system: "msx",
        es_de_fullname: "MSX",
    },
    EsDeSystemMapping {
        platform_id: "MSX2",
        es_de_system: "msx2",
        es_de_fullname: "MSX2",
    },
    EsDeSystemMapping {
        platform_id: "Commodore 64",
        es_de_system: "c64",
        es_de_fullname: "Commodore 64",
    },
    EsDeSystemMapping {
        platform_id: "ZX Spectrum",
        es_de_system: "zxspectrum",
        es_de_fullname: "Sinclair ZX Spectrum",
    },
    EsDeSystemMapping {
        platform_id: "Intellivision",
        es_de_system: "intellivision",
        es_de_fullname: "Mattel Electronics Intellivision",
    },
    EsDeSystemMapping {
        platform_id: "WonderSwan",
        es_de_system: "wonderswan",
        es_de_fullname: "Bandai WonderSwan",
    },
    EsDeSystemMapping {
        platform_id: "WonderSwan Color",
        es_de_system: "wonderswancolor",
        es_de_fullname: "Bandai WonderSwan Color",
    },
    EsDeSystemMapping {
        platform_id: "NeoGeo",
        es_de_system: "neogeo",
        es_de_fullname: "SNK Neo Geo",
    },
    // --- Batch 4 additions (docs/ESDE_BATCH4_MAPPING_PLAN.md §5) ------
    // Every `es_de_system`/`es_de_fullname` pair below was read verbatim
    // from ES-DE's current upstream
    // `resources/systems/linux/es_systems.xml`, never guessed from an
    // emulator command label or an EmuWiz alias. In particular, ES-DE's
    // system names are `pcengine` and `pcenginecd`, not `pce`/`pcecd`.
    EsDeSystemMapping {
        platform_id: "3DO",
        es_de_system: "3do",
        es_de_fullname: "3DO Interactive Multiplayer",
    },
    EsDeSystemMapping {
        platform_id: "Acorn Archimedes",
        es_de_system: "archimedes",
        es_de_fullname: "Acorn Archimedes",
    },
    EsDeSystemMapping {
        platform_id: "Acorn Electron",
        es_de_system: "electron",
        es_de_fullname: "Acorn Electron",
    },
    EsDeSystemMapping {
        platform_id: "Amstrad CPC",
        es_de_system: "amstradcpc",
        es_de_fullname: "Amstrad CPC",
    },
    EsDeSystemMapping {
        platform_id: "Apple II",
        es_de_system: "apple2",
        es_de_fullname: "Apple II",
    },
    EsDeSystemMapping {
        platform_id: "BBC Micro",
        es_de_system: "bbcmicro",
        es_de_fullname: "Acorn Computers BBC Micro",
    },
    EsDeSystemMapping {
        platform_id: "FM Towns",
        es_de_system: "fmtowns",
        es_de_fullname: "Fujitsu FM Towns",
    },
    EsDeSystemMapping {
        platform_id: "Macintosh",
        es_de_system: "macintosh",
        es_de_fullname: "Apple Macintosh",
    },
    EsDeSystemMapping {
        platform_id: "NEC PC-8801",
        es_de_system: "pc88",
        es_de_fullname: "NEC PC-8800 Series",
    },
    EsDeSystemMapping {
        platform_id: "NGage",
        es_de_system: "ngage",
        es_de_fullname: "Nokia N-Gage",
    },
    EsDeSystemMapping {
        platform_id: "PC Engine",
        es_de_system: "pcengine",
        es_de_fullname: "NEC PC Engine",
    },
    EsDeSystemMapping {
        platform_id: "PC Engine CD",
        es_de_system: "pcenginecd",
        es_de_fullname: "NEC PC Engine CD",
    },
    // --- Batch 5 additions (docs/ESDE_FINAL_PARITY_PLAN.md §6) --------
    // Exact, one-to-one upstream ES-DE systems only. Policy-bearing regional,
    // equivalent-canonical, and no-target platforms deliberately remain absent.
    EsDeSystemMapping {
        platform_id: "VIC-20",
        es_de_system: "vic20",
        es_de_fullname: "Commodore VIC-20",
    },
    EsDeSystemMapping {
        platform_id: "Neo Geo CD",
        es_de_system: "neogeocd",
        es_de_fullname: "SNK Neo Geo CD",
    },
    EsDeSystemMapping {
        platform_id: "Switch",
        es_de_system: "switch",
        es_de_fullname: "Nintendo Switch",
    },
    EsDeSystemMapping {
        platform_id: "PC-FX",
        es_de_system: "pcfx",
        es_de_fullname: "NEC PC-FX",
    },
    EsDeSystemMapping {
        platform_id: "Philips CD-i",
        es_de_system: "cdimono1",
        es_de_fullname: "Philips CD-i",
    },
    EsDeSystemMapping {
        platform_id: "PS4",
        es_de_system: "ps4",
        es_de_fullname: "Sony PlayStation 4",
    },
    EsDeSystemMapping {
        platform_id: "Sharp X68000",
        es_de_system: "x68000",
        es_de_fullname: "Sharp X68000",
    },
];

/// The reviewed row for `platform_id`, if any.
pub fn es_de_system_for_platform(platform_id: &str) -> Option<&'static EsDeSystemMapping> {
    ES_DE_SYSTEM_MAP
        .iter()
        .find(|entry| entry.platform_id == platform_id)
}

/// Why no [`EsDeEntryPlan`] could be produced at all - distinct from
/// [`EsDeEntryStatus::Blocked`], which still names a system/path for a
/// platform this module *does* know how to export. See the module doc
/// comment's "Fail-closed rules".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoEntryReason {
    /// [`CanonicalIdentityStatus::Unknown`].
    IdentityUnresolved,
    /// [`CanonicalIdentityStatus::Conflicting`].
    IdentityConflict,
    /// The resolved platform has no row in [`ES_DE_SYSTEM_MAP`].
    PlatformUnmapped { platform_id: String },
}

/// Why an [`EsDeEntryPlan`] is [`EsDeEntryStatus::Blocked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EsDeEntryBlockerKind {
    /// [`LaunchContentRef`] has no resolved path and does not need
    /// mounting - some other reason it was never resolved.
    ContentUnresolved,
    /// [`LaunchContentRef::requires_mount`] is `true`: the content lives
    /// inside a container (e.g. an archive) that has not been mounted, so
    /// no path ES-DE could be pointed at exists yet.
    ContentRequiresMount,
}

/// One blocking condition on an [`EsDeEntryPlan`] - structured, never
/// free-text-only, mirroring [`crate::launch::readiness::LaunchBlocker`]'s
/// own shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeEntryBlocker {
    pub kind: EsDeEntryBlockerKind,
    pub detail: String,
}

impl EsDeEntryBlocker {
    fn new(kind: EsDeEntryBlockerKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EsDeEntryStatus {
    /// A usable game path is already resolved. This never implies an
    /// emulator/core choice is also known - see
    /// [`EsDeEntryPlan::emulator_choice`].
    Ready,
    /// At least one [`EsDeEntryBlocker`] - see the module doc comment.
    Blocked,
}

/// How this game would appear as one ES-DE entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EsDeEntryPlan {
    /// A [`crate::platform::Platform::id`] value.
    pub platform_id: String,
    pub es_de_system: &'static str,
    pub es_de_fullname: &'static str,
    /// The game's ES-DE-usable path, when [`Self::path_usable`] is `true`.
    /// Never fabricated: this is exactly
    /// [`LaunchContentRef::resolved_path`], carried through unchanged.
    pub game_path: Option<PathBuf>,
    /// Whether [`Self::game_path`] is already usable right now - equivalent
    /// to [`LaunchContentRef::has_runnable_path`].
    pub path_usable: bool,
    /// Whether the underlying content still needs mounting/preparation
    /// before it can be usable - equivalent to
    /// [`LaunchContentRef::requires_mount`].
    pub requires_mount: bool,
    /// The already-known emulator/core choice for this game, when a
    /// caller-supplied [`LaunchPlan`] names exactly one - see the module
    /// doc comment's "Fail-closed rules". `None` means "not yet known",
    /// never "none exists".
    pub emulator_choice: Option<LaunchTarget>,
    pub status: EsDeEntryStatus,
    pub blockers: Vec<EsDeEntryBlocker>,
}

/// The full result of attempting to describe one game as an ES-DE entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EsDeExportOutcome {
    /// No ES-DE entry can be described at all - see [`NoEntryReason`].
    NoEntry(NoEntryReason),
    Entry(EsDeEntryPlan),
}

/// The single already-known emulator/core choice from `plan`, if exactly
/// one non-blocked candidate is already preferred - never picks among an
/// [`CandidatePreference::Undetermined`] tie. Pure: only reads `plan`.
fn chosen_emulator(plan: &LaunchPlan) -> Option<LaunchTarget> {
    let mut preferred = plan.candidates.iter().filter(|candidate| {
        candidate.readiness != LaunchReadiness::Blocked
            && matches!(
                candidate.preference,
                CandidatePreference::Remembered | CandidatePreference::SoleEligible
            )
    });
    let candidate = preferred.next()?;
    if preferred.next().is_some() {
        // More than one already-preferred candidate should never happen
        // (`build_launch_plan` only ever marks one), but this module never
        // guesses among ties even if it somehow did.
        return None;
    }
    Some(candidate.target.clone())
}

/// Builds the [`EsDeExportOutcome`] for one game from already-gathered
/// data. Pure: no filesystem read, no network call, no process spawn, no
/// write, and no mutation of `identity`/`content`/`launch_plan`.
///
/// `launch_plan`, when supplied, must already have been built (by
/// [`crate::launch::planning::build_launch_plan`]) for the same game this
/// `identity`/`content` describe - this function does not verify that
/// itself, exactly as [`build_launch_plan`] itself does not re-verify its
/// own inputs came from a matching identity/content resolution elsewhere.
pub fn build_es_de_entry_plan(
    identity: &CanonicalIdentityStatus,
    content: &LaunchContentRef,
    launch_plan: Option<&LaunchPlan>,
) -> EsDeExportOutcome {
    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(resolved) => resolved,
        CanonicalIdentityStatus::Unknown => {
            return EsDeExportOutcome::NoEntry(NoEntryReason::IdentityUnresolved);
        }
        CanonicalIdentityStatus::Conflicting => {
            return EsDeExportOutcome::NoEntry(NoEntryReason::IdentityConflict);
        }
    };

    let Some(mapping) = es_de_system_for_platform(&resolved.platform_id) else {
        return EsDeExportOutcome::NoEntry(NoEntryReason::PlatformUnmapped {
            platform_id: resolved.platform_id.clone(),
        });
    };

    let path_usable = content.has_runnable_path();
    let mut blockers = Vec::new();
    if !path_usable {
        blockers.push(if content.requires_mount {
            EsDeEntryBlocker::new(
                EsDeEntryBlockerKind::ContentRequiresMount,
                "content is inside a container (e.g. an archive) that has not been mounted, \
                 so no path exists yet for ES-DE to use",
            )
        } else {
            EsDeEntryBlocker::new(
                EsDeEntryBlockerKind::ContentUnresolved,
                "no runnable content path was resolved, so an ES-DE game path cannot be \
                 determined",
            )
        });
    }

    let status = if blockers.is_empty() {
        EsDeEntryStatus::Ready
    } else {
        EsDeEntryStatus::Blocked
    };

    EsDeExportOutcome::Entry(EsDeEntryPlan {
        platform_id: resolved.platform_id.clone(),
        es_de_system: mapping.es_de_system,
        es_de_fullname: mapping.es_de_fullname,
        game_path: content.resolved_path.clone(),
        path_usable,
        requires_mount: content.requires_mount,
        emulator_choice: launch_plan.and_then(chosen_emulator),
        status,
        blockers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::planning::{LaunchCandidate, LaunchPlanSummary, ResolvedIdentity};
    use crate::launch::readiness::FirmwareReadiness;
    use crate::platform::{platform_by_id, platform_for_alias};

    fn resolved(platform_id: &str) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: platform_id.to_string(),
            game_key: "SLUS-00594".to_string(),
        })
    }

    fn usable_content() -> LaunchContentRef {
        LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: Some(PathBuf::from("/library/psx/Game.chd")),
            requires_mount: false,
            provenance: "test fixture".to_string(),
        }
    }

    fn unresolved_content() -> LaunchContentRef {
        LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: None,
            requires_mount: false,
            provenance: "test fixture".to_string(),
        }
    }

    fn archive_content() -> LaunchContentRef {
        LaunchContentRef {
            kind: None,
            container: Some(crate::launch::planning::LaunchContainerKind::Archive),
            resolved_path: None,
            requires_mount: true,
            provenance: "test fixture".to_string(),
        }
    }

    #[test]
    fn every_mapping_row_platform_id_exists_in_the_registry() {
        for entry in ES_DE_SYSTEM_MAP {
            assert!(
                platform_by_id(entry.platform_id).is_some(),
                "{} is not a real Platform::id",
                entry.platform_id
            );
        }
    }

    #[test]
    fn every_required_platform_is_mapped() {
        for platform_id in [
            "PSX",
            "PS2",
            "PS3",
            "PSP",
            "PlayStation Vita",
            "Nintendo 3DS",
            "Nintendo DS",
            "Xbox",
            "Xbox360",
            "GameCube",
            "Wii",
            "WiiU",
            "Dreamcast",
            "Saturn",
            "Sega CD",
            "AtariST",
            "Amiga",
            "AmigaCD32",
            "Commodore CDTV",
            "Arcade",
            "DOS",
            "ScummVM",
            "Game Boy",
            "Game Boy Color",
            "Game Boy Advance",
            "NES",
            "SNES",
            "MegaDrive",
            "N64",
            "MasterSystem",
            "GameGear",
            "Sega 32X",
            "Atari2600",
            "Atari5200",
            "Atari7800",
            "Atari Lynx",
            "Atari Jaguar",
            "ColecoVision",
            "Vectrex",
            "Neo Geo Pocket",
            "Neo Geo Pocket Color",
            "Virtual Boy",
            "MSX",
            "MSX2",
            "Commodore 64",
            "ZX Spectrum",
            "Intellivision",
            "WonderSwan",
            "WonderSwan Color",
            "NeoGeo",
            "3DO",
            "Acorn Archimedes",
            "Acorn Electron",
            "Amstrad CPC",
            "Apple II",
            "BBC Micro",
            "FM Towns",
            "Macintosh",
            "NEC PC-8801",
            "NGage",
            "PC Engine",
            "PC Engine CD",
            "VIC-20",
            "Neo Geo CD",
            "Switch",
            "PC-FX",
            "Philips CD-i",
            "PS4",
            "Sharp X68000",
        ] {
            assert!(
                es_de_system_for_platform(platform_id).is_some(),
                "{platform_id} is missing from ES_DE_SYSTEM_MAP"
            );
        }
    }

    #[test]
    fn psx_maps_to_the_psx_system_exactly() {
        let mapping = es_de_system_for_platform("PSX").unwrap();
        assert_eq!(mapping.es_de_system, "psx");
        assert_eq!(mapping.es_de_fullname, "Sony PlayStation");
    }

    #[test]
    fn gamecube_maps_to_gc_not_gamecube() {
        // ES-DE's own short name is "gc", not "gamecube" - a plausible but
        // wrong guess this table must not make.
        let mapping = es_de_system_for_platform("GameCube").unwrap();
        assert_eq!(mapping.es_de_system, "gc");
    }

    #[test]
    fn megadrive_maps_to_megadrive_not_genesis() {
        let mapping = es_de_system_for_platform("MegaDrive").unwrap();
        assert_eq!(mapping.es_de_system, "megadrive");
    }

    // --- Batch 1 (docs/ESDE_PARITY_AUDIT.md §10) -----------------------

    /// (canonical platform id, expected ES-DE `<name>`, expected ES-DE
    /// `<fullname>`) - every value read verbatim from ES-DE's own
    /// `resources/systems/linux/es_systems.xml` (see the module doc
    /// comment above `ES_DE_SYSTEM_MAP`), never guessed.
    const BATCH_1: &[(&str, &str, &str)] = &[
        ("MasterSystem", "mastersystem", "Sega Master System"),
        ("GameGear", "gamegear", "Sega Game Gear"),
        ("Sega 32X", "sega32x", "Sega 32X"),
        ("Atari2600", "atari2600", "Atari 2600"),
        ("Atari5200", "atari5200", "Atari 5200"),
        ("Atari7800", "atari7800", "Atari 7800 ProSystem"),
        ("Atari Lynx", "atarilynx", "Atari Lynx"),
        ("Atari Jaguar", "atarijaguar", "Atari Jaguar"),
        ("ColecoVision", "colecovision", "Coleco ColecoVision"),
        ("Vectrex", "vectrex", "Vectrex"),
        ("Neo Geo Pocket", "ngp", "SNK Neo Geo Pocket"),
        ("Neo Geo Pocket Color", "ngpc", "SNK Neo Geo Pocket Color"),
    ];

    #[test]
    fn batch_1_platforms_map_to_es_de_exactly() {
        for (platform_id, system, fullname) in BATCH_1 {
            let mapping = es_de_system_for_platform(platform_id)
                .unwrap_or_else(|| panic!("missing ES-DE mapping for {platform_id}"));
            assert_eq!(
                mapping.es_de_system, *system,
                "wrong es_de_system for {platform_id}"
            );
            assert_eq!(
                mapping.es_de_fullname, *fullname,
                "wrong es_de_fullname for {platform_id}"
            );
        }
    }

    #[test]
    fn batch_1_resolved_identity_produces_a_ready_entry() {
        for (platform_id, system, _) in BATCH_1 {
            let outcome = build_es_de_entry_plan(&resolved(platform_id), &usable_content(), None);
            let EsDeExportOutcome::Entry(plan) = outcome else {
                panic!("expected an ES-DE entry for {platform_id}");
            };
            assert_eq!(plan.status, EsDeEntryStatus::Ready);
            assert!(plan.path_usable);
            assert_eq!(
                plan.es_de_system, *system,
                "wrong destination system for {platform_id}"
            );
        }
    }

    /// A known registry alias resolves to the same canonical platform, and
    /// therefore the same ES-DE row, as the canonical id itself - proving
    /// the ES-DE map is never a second, alias-unaware identity source.
    #[test]
    fn batch_1_known_aliases_resolve_to_the_same_es_de_row_as_the_canonical_id() {
        for (platform_id, alias) in [
            ("MasterSystem", "sms"),
            ("GameGear", "gg"),
            ("Sega 32X", "32x"),
            ("Atari2600", "a2600"),
            ("Atari5200", "a5200"),
            ("Atari7800", "a7800"),
            ("Atari Lynx", "lynx"),
            ("Atari Jaguar", "jaguar"),
            ("ColecoVision", "coleco"),
            ("Vectrex", "gcevectrex"),
            ("Neo Geo Pocket", "neogeopocket"),
            ("Neo Geo Pocket Color", "neogeopocketcolor"),
        ] {
            let via_alias = platform_for_alias(alias)
                .unwrap_or_else(|| panic!("{alias} did not resolve to a platform"));
            assert_eq!(
                via_alias.id, platform_id,
                "alias {alias} resolved to the wrong platform"
            );
            let canonical_mapping = es_de_system_for_platform(platform_id).unwrap();
            let alias_mapping = es_de_system_for_platform(via_alias.id).unwrap();
            assert_eq!(
                alias_mapping.es_de_system, canonical_mapping.es_de_system,
                "alias {alias} must produce the same ES-DE system as {platform_id} itself"
            );
        }
    }

    // --- Batch 3 (docs/ESDE_PARITY_AUDIT.md §10) -----------------------

    /// (canonical platform id, expected ES-DE `<name>`, expected ES-DE
    /// `<fullname>`) - every value read verbatim from ES-DE's own
    /// `resources/systems/linux/es_systems.xml`, never guessed.
    const BATCH_3: &[(&str, &str, &str)] = &[
        ("Virtual Boy", "virtualboy", "Nintendo Virtual Boy"),
        ("MSX", "msx", "MSX"),
        ("MSX2", "msx2", "MSX2"),
        ("Commodore 64", "c64", "Commodore 64"),
        ("ZX Spectrum", "zxspectrum", "Sinclair ZX Spectrum"),
        (
            "Intellivision",
            "intellivision",
            "Mattel Electronics Intellivision",
        ),
        ("WonderSwan", "wonderswan", "Bandai WonderSwan"),
        (
            "WonderSwan Color",
            "wonderswancolor",
            "Bandai WonderSwan Color",
        ),
        ("NeoGeo", "neogeo", "SNK Neo Geo"),
    ];

    /// (canonical platform id, expected ES-DE `<name>`, expected ES-DE
    /// `<fullname>`) read verbatim from the current ES-DE Linux systems
    /// file, as recorded in `docs/ESDE_BATCH4_MAPPING_PLAN.md`.
    const BATCH_4: &[(&str, &str, &str)] = &[
        ("3DO", "3do", "3DO Interactive Multiplayer"),
        ("Acorn Archimedes", "archimedes", "Acorn Archimedes"),
        ("Acorn Electron", "electron", "Acorn Electron"),
        ("Amstrad CPC", "amstradcpc", "Amstrad CPC"),
        ("Apple II", "apple2", "Apple II"),
        ("BBC Micro", "bbcmicro", "Acorn Computers BBC Micro"),
        ("FM Towns", "fmtowns", "Fujitsu FM Towns"),
        ("Macintosh", "macintosh", "Apple Macintosh"),
        ("NEC PC-8801", "pc88", "NEC PC-8800 Series"),
        ("NGage", "ngage", "Nokia N-Gage"),
        ("PC Engine", "pcengine", "NEC PC Engine"),
        ("PC Engine CD", "pcenginecd", "NEC PC Engine CD"),
    ];

    /// Batch 5 is intentionally only the seven direct, one-to-one mappings
    /// approved in `docs/ESDE_FINAL_PARITY_PLAN.md`.
    const BATCH_5: &[(&str, &str, &str)] = &[
        ("VIC-20", "vic20", "Commodore VIC-20"),
        ("Neo Geo CD", "neogeocd", "SNK Neo Geo CD"),
        ("Switch", "switch", "Nintendo Switch"),
        ("PC-FX", "pcfx", "NEC PC-FX"),
        ("Philips CD-i", "cdimono1", "Philips CD-i"),
        ("PS4", "ps4", "Sony PlayStation 4"),
        ("Sharp X68000", "x68000", "Sharp X68000"),
    ];

    #[test]
    fn batch_5_platforms_map_to_es_de_exactly() {
        for (platform_id, system, fullname) in BATCH_5 {
            let mapping = es_de_system_for_platform(platform_id)
                .unwrap_or_else(|| panic!("missing ES-DE mapping for {platform_id}"));
            assert_eq!(mapping.es_de_system, *system);
            assert_eq!(mapping.es_de_fullname, *fullname);
        }
    }

    #[test]
    fn batch_5_resolved_identity_produces_a_ready_entry() {
        for (platform_id, system, _) in BATCH_5 {
            let outcome = build_es_de_entry_plan(&resolved(platform_id), &usable_content(), None);
            let EsDeExportOutcome::Entry(plan) = outcome else {
                panic!("expected an ES-DE entry for {platform_id}");
            };
            assert_eq!(plan.status, EsDeEntryStatus::Ready);
            assert!(plan.path_usable);
            assert_eq!(plan.es_de_system, *system);
        }
    }

    #[test]
    fn batch_5_aliases_normalize_to_the_same_canonical_row() {
        for (platform_id, alias) in [
            ("VIC-20", "vic"),
            ("Neo Geo CD", "ngcd"),
            ("Switch", "nintendoswitch"),
            ("PC-FX", "necpcfx"),
            ("Philips CD-i", "cdi"),
            ("PS4", "playstation4"),
            ("Sharp X68000", "x68k"),
        ] {
            for spelling in [alias.to_string(), alias.to_ascii_uppercase()] {
                let canonical = platform_for_alias(&spelling)
                    .unwrap_or_else(|| panic!("{spelling} did not resolve to a platform"));
                assert_eq!(canonical.id, platform_id);
                assert_eq!(
                    es_de_system_for_platform(canonical.id),
                    es_de_system_for_platform(platform_id),
                    "{spelling} did not resolve to {platform_id}'s ES-DE row"
                );
            }
        }
    }

    #[test]
    fn batch_5_distinct_neighbours_and_deferred_platforms_remain_safe() {
        let neogeocd = es_de_system_for_platform("Neo Geo CD").unwrap();
        assert_eq!(neogeocd.es_de_system, "neogeocd");
        for platform_id in ["NeoGeo", "Neo Geo Pocket", "Neo Geo Pocket Color"] {
            assert_ne!(
                neogeocd.es_de_system,
                es_de_system_for_platform(platform_id).unwrap().es_de_system,
                "Neo Geo CD must remain distinct from {platform_id}"
            );
        }
        let pcfx = es_de_system_for_platform("PC-FX").unwrap();
        assert_ne!(
            pcfx.es_de_system,
            es_de_system_for_platform("PC Engine").unwrap().es_de_system
        );
        assert_ne!(
            pcfx.es_de_system,
            es_de_system_for_platform("PC Engine CD")
                .unwrap()
                .es_de_system
        );
        let cdi = es_de_system_for_platform("Philips CD-i").unwrap();
        assert_ne!(
            cdi.es_de_system,
            es_de_system_for_platform("Arcade").unwrap().es_de_system
        );
        for platform_id in [
            "Atari 8-bit",
            "Commodore 128",
            "NeoGeo64",
            "PC",
            "PC-98",
            "NEC PC-9801",
            "TurboGrafx-16",
            "not-a-real-platform",
        ] {
            assert!(
                es_de_system_for_platform(platform_id).is_none(),
                "{platform_id} must remain refused until its policy is resolved"
            );
        }
    }

    #[test]
    fn batch_4_platforms_map_to_es_de_exactly() {
        for (platform_id, system, fullname) in BATCH_4 {
            let mapping = es_de_system_for_platform(platform_id)
                .unwrap_or_else(|| panic!("missing ES-DE mapping for {platform_id}"));
            assert_eq!(mapping.es_de_system, *system);
            assert_eq!(mapping.es_de_fullname, *fullname);
        }
    }

    #[test]
    fn batch_4_resolved_identity_produces_a_ready_entry() {
        for (platform_id, system, _) in BATCH_4 {
            let outcome = build_es_de_entry_plan(&resolved(platform_id), &usable_content(), None);
            let EsDeExportOutcome::Entry(plan) = outcome else {
                panic!("expected an ES-DE entry for {platform_id}");
            };
            assert_eq!(plan.status, EsDeEntryStatus::Ready);
            assert!(plan.path_usable);
            assert_eq!(plan.es_de_system, *system);
        }
    }

    #[test]
    fn batch_4_aliases_normalize_to_the_same_canonical_row() {
        for (platform_id, alias) in [
            ("3DO", "threedo"),
            ("Acorn Archimedes", "archie"),
            ("Acorn Electron", "elk"),
            ("Amstrad CPC", "cpc"),
            ("Apple II", "apple2"),
            ("BBC Micro", "bbcb"),
            ("FM Towns", "towns"),
            ("Macintosh", "mac"),
            ("NEC PC-8801", "pc88"),
            ("NGage", "nokiangage"),
            ("PC Engine", "pce"),
            ("PC Engine CD", "pcecd"),
        ] {
            for spelling in [alias.to_string(), alias.to_ascii_uppercase()] {
                let canonical = platform_for_alias(&spelling)
                    .unwrap_or_else(|| panic!("{spelling} did not resolve to a platform"));
                assert_eq!(canonical.id, platform_id);
                assert_eq!(
                    es_de_system_for_platform(canonical.id),
                    es_de_system_for_platform(platform_id),
                    "{spelling} did not resolve to {platform_id}'s ES-DE row"
                );
            }
        }
    }

    #[test]
    fn batch_4_pc_engine_targets_and_neighbours_remain_distinct() {
        let pcengine = es_de_system_for_platform("PC Engine").unwrap();
        let pcenginecd = es_de_system_for_platform("PC Engine CD").unwrap();
        assert_eq!(pcengine.es_de_system, "pcengine");
        assert_eq!(pcenginecd.es_de_system, "pcenginecd");
        assert_ne!(pcengine.es_de_system, pcenginecd.es_de_system);

        for platform_id in ["TurboGrafx-16", "PC-98", "NEC PC-9801"] {
            assert!(
                es_de_system_for_platform(platform_id).is_none(),
                "{platform_id} must remain deferred rather than borrowing a Batch 4 target"
            );
        }
        assert_ne!(
            es_de_system_for_platform("Acorn Electron")
                .unwrap()
                .es_de_system,
            es_de_system_for_platform("BBC Micro").unwrap().es_de_system
        );
        assert_ne!(
            es_de_system_for_platform("Apple II").unwrap().es_de_system,
            es_de_system_for_platform("Macintosh").unwrap().es_de_system
        );
    }

    #[test]
    fn batch_3_platforms_map_to_es_de_exactly() {
        for (platform_id, system, fullname) in BATCH_3 {
            let mapping = es_de_system_for_platform(platform_id)
                .unwrap_or_else(|| panic!("missing ES-DE mapping for {platform_id}"));
            assert_eq!(
                mapping.es_de_system, *system,
                "wrong es_de_system for {platform_id}"
            );
            assert_eq!(
                mapping.es_de_fullname, *fullname,
                "wrong es_de_fullname for {platform_id}"
            );
        }
    }

    #[test]
    fn batch_3_resolved_identity_produces_a_ready_entry() {
        for (platform_id, system, _) in BATCH_3 {
            let outcome = build_es_de_entry_plan(&resolved(platform_id), &usable_content(), None);
            let EsDeExportOutcome::Entry(plan) = outcome else {
                panic!("expected an ES-DE entry for {platform_id}");
            };
            assert_eq!(plan.status, EsDeEntryStatus::Ready);
            assert!(plan.path_usable);
            assert_eq!(
                plan.es_de_system, *system,
                "wrong destination system for {platform_id}"
            );
        }
    }

    #[test]
    fn batch_3_known_aliases_resolve_to_the_same_es_de_row_as_the_canonical_id() {
        for (platform_id, alias) in [
            ("Virtual Boy", "vb"),
            ("MSX", "msx1"),
            ("MSX2", "msx2plus"),
            ("Commodore 64", "c64"),
            ("ZX Spectrum", "speccy"),
            ("Intellivision", "intv"),
            ("WonderSwan", "bandaiwonderswan"),
            ("WonderSwan Color", "wsc"),
            ("NeoGeo", "neogeomvs"),
        ] {
            let via_alias = platform_for_alias(alias)
                .unwrap_or_else(|| panic!("{alias} did not resolve to a platform"));
            assert_eq!(
                via_alias.id, platform_id,
                "alias {alias} resolved to the wrong platform"
            );
            let canonical_mapping = es_de_system_for_platform(platform_id).unwrap();
            let alias_mapping = es_de_system_for_platform(via_alias.id).unwrap();
            assert_eq!(
                alias_mapping.es_de_system, canonical_mapping.es_de_system,
                "alias {alias} must produce the same ES-DE system as {platform_id} itself"
            );
        }
    }

    #[test]
    fn unrecognised_platform_names_are_never_mapped_to_a_batch_3_row() {
        for bogus in ["NeoGeoPlus", "MSX3", "not-a-real-platform", ""] {
            assert!(
                es_de_system_for_platform(bogus).is_none(),
                "{bogus:?} must not resolve to any ES-DE row"
            );
            assert!(
                platform_for_alias(bogus).is_none(),
                "{bogus:?} must not resolve to any canonical platform"
            );
        }
    }

    /// Batch 3 must never disturb NeoGeo's neighbouring, distinctly-mapped
    /// relatives (Neo Geo Pocket/Pocket Color, already Batch 1 rows) - each
    /// keeps its own distinct ES-DE target rather than collapsing onto the
    /// arcade `neogeo` row.
    #[test]
    fn neogeo_and_its_pocket_relatives_remain_distinctly_mapped() {
        let neogeo = es_de_system_for_platform("NeoGeo").unwrap();
        let ngp = es_de_system_for_platform("Neo Geo Pocket").unwrap();
        let ngpc = es_de_system_for_platform("Neo Geo Pocket Color").unwrap();
        assert_eq!(neogeo.es_de_system, "neogeo");
        assert_eq!(ngp.es_de_system, "ngp");
        assert_eq!(ngpc.es_de_system, "ngpc");
        assert_ne!(neogeo.es_de_system, ngp.es_de_system);
        assert_ne!(neogeo.es_de_system, ngpc.es_de_system);
        assert_ne!(ngp.es_de_system, ngpc.es_de_system);
    }

    /// A wrong/unknown platform name is refused outright - never coerced
    /// into one of the Batch 1 rows or any other row by fuzzy matching.
    #[test]
    fn unrecognised_platform_names_are_never_mapped_to_a_batch_1_row() {
        for bogus in [
            "SegaMasterSystem2",
            "MasterSystemPlus",
            "not-a-real-platform",
            "",
        ] {
            assert!(
                es_de_system_for_platform(bogus).is_none(),
                "{bogus:?} must not resolve to any ES-DE row"
            );
            assert!(
                platform_for_alias(bogus).is_none(),
                "{bogus:?} must not resolve to any canonical platform"
            );
        }
    }

    /// Still-unmapped policy/no-target platforms must keep failing closed
    /// rather than falling back to a merely related Batch 1--5 row.
    #[test]
    fn platforms_still_unmapped_after_batch_5_remain_refused() {
        // NeoGeo/WonderSwan/Intellivision were the still-unmapped examples
        // when this test was first written for Batch 1; Batch 3 has since
        // mapped all three (see `batch_3_platforms_map_to_es_de_exactly`),
        // so they moved out of this list rather than being asserted here
        // and in a "now mapped" test simultaneously.
        for platform_id in [
            "Atari 8-bit",
            "Commodore 128",
            "NeoGeo64",
            "PC",
            "PC-98",
            "NEC PC-9801",
            "TurboGrafx-16",
        ] {
            assert!(
                platform_by_id(platform_id).is_some(),
                "{platform_id} should be a real registry id (fixture drift)"
            );
            assert!(
                es_de_system_for_platform(platform_id).is_none(),
                "{platform_id} must still be unmapped after Batch 5"
            );
        }
    }

    #[test]
    fn every_mapped_es_de_system_target_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in ES_DE_SYSTEM_MAP {
            assert!(
                seen.insert(entry.es_de_system),
                "duplicate ES-DE system target {:?} (platform {})",
                entry.es_de_system,
                entry.platform_id
            );
        }
    }

    #[test]
    fn no_platform_id_appears_twice_in_the_map() {
        let mut seen = std::collections::HashSet::new();
        for entry in ES_DE_SYSTEM_MAP {
            assert!(
                seen.insert(entry.platform_id),
                "platform {} has more than one ES-DE mapping row",
                entry.platform_id
            );
        }
    }

    #[test]
    fn classic_nintendo_platforms_map_to_es_de_exactly() {
        for (platform_id, system, fullname) in [
            ("Game Boy", "gb", "Nintendo Game Boy"),
            ("Game Boy Color", "gbc", "Nintendo Game Boy Color"),
            ("Game Boy Advance", "gba", "Nintendo Game Boy Advance"),
            ("N64", "n64", "Nintendo 64"),
        ] {
            let mapping = es_de_system_for_platform(platform_id)
                .unwrap_or_else(|| panic!("missing ES-DE mapping for {platform_id}"));
            assert_eq!(mapping.es_de_system, system);
            assert_eq!(mapping.es_de_fullname, fullname);
        }
    }

    #[test]
    fn parity_batch_maps_to_one_verified_es_de_system_each() {
        for (platform_id, system, fullname) in [
            ("PlayStation Vita", "psvita", "Sony PlayStation Vita"),
            ("Nintendo 3DS", "3ds", "Nintendo 3DS"),
            ("Nintendo DS", "nds", "Nintendo DS"),
            ("WiiU", "wiiu", "Nintendo Wii U"),
            ("DOS", "dos", "DOS"),
            ("ScummVM", "scummvm", "ScummVM"),
            ("Arcade", "arcade", "Arcade"),
            ("AmigaCD32", "amigacd32", "Commodore Amiga CD32"),
            ("Commodore CDTV", "cdtv", "Commodore CDTV"),
        ] {
            let matches: Vec<_> = ES_DE_SYSTEM_MAP
                .iter()
                .filter(|entry| entry.platform_id == platform_id)
                .collect();
            assert_eq!(matches.len(), 1, "expected one row for {platform_id}");
            assert_eq!(matches[0].es_de_system, system);
            assert_eq!(matches[0].es_de_fullname, fullname);
        }
    }

    #[test]
    fn parity_batch_aliases_resolve_to_canonical_rows_without_extra_targets() {
        for (alias, platform_id) in [
            ("psvita", "PlayStation Vita"),
            ("N3DS", "Nintendo 3DS"),
            ("ds", "Nintendo DS"),
            ("Nintendo Wii U", "WiiU"),
            ("ms-dos", "DOS"),
            ("scumm", "ScummVM"),
            ("fbneo", "Arcade"),
            ("cd32", "AmigaCD32"),
            ("amigacdtv", "Commodore CDTV"),
        ] {
            let canonical = crate::platform::platform_for_alias(alias)
                .unwrap_or_else(|| panic!("alias {alias} did not resolve"));
            assert_eq!(canonical.id, platform_id);
            assert!(es_de_system_for_platform(canonical.id).is_some());
        }
    }

    #[test]
    fn unknown_platforms_remain_fail_closed_and_map_has_no_duplicate_systems() {
        assert!(es_de_system_for_platform("not-a-platform").is_none());
        for (left_index, left) in ES_DE_SYSTEM_MAP.iter().enumerate() {
            assert!(
                ES_DE_SYSTEM_MAP[left_index + 1..]
                    .iter()
                    .all(|right| right.platform_id != left.platform_id),
                "duplicate platform mapping for {}",
                left.platform_id
            );
            assert!(
                ES_DE_SYSTEM_MAP[left_index + 1..]
                    .iter()
                    .all(|right| right.es_de_system != left.es_de_system),
                "duplicate ES-DE system mapping for {}",
                left.es_de_system
            );
        }
    }

    #[test]
    fn multi_emulator_platforms_are_mapped_by_platform_not_emulator() {
        for platform_id in ["Arcade", "AmigaCD32", "Commodore CDTV"] {
            let mapping = es_de_system_for_platform(platform_id).unwrap();
            assert!(!mapping.es_de_system.contains("mame"));
            assert!(!mapping.es_de_system.contains("fbneo"));
            assert!(!mapping.es_de_system.contains("amiberry"));
        }
    }

    #[test]
    fn classic_nintendo_resolved_identity_produces_a_ready_entry() {
        for platform_id in ["Game Boy", "Game Boy Color", "Game Boy Advance", "N64"] {
            let outcome = build_es_de_entry_plan(&resolved(platform_id), &usable_content(), None);
            let EsDeExportOutcome::Entry(plan) = outcome else {
                panic!("expected an ES-DE entry for {platform_id}");
            };
            assert_eq!(plan.status, EsDeEntryStatus::Ready);
            assert!(plan.path_usable);
            assert!(plan.blockers.is_empty());
        }
    }

    #[test]
    fn ready_entry_for_a_mapped_platform_with_a_usable_path() {
        let outcome = build_es_de_entry_plan(&resolved("PSX"), &usable_content(), None);
        let EsDeExportOutcome::Entry(plan) = outcome else {
            panic!("expected an entry");
        };
        assert_eq!(plan.status, EsDeEntryStatus::Ready);
        assert!(plan.blockers.is_empty());
        assert_eq!(plan.es_de_system, "psx");
        assert_eq!(plan.game_path, Some(PathBuf::from("/library/psx/Game.chd")));
        assert!(plan.path_usable);
        assert!(!plan.requires_mount);
    }

    #[test]
    fn unknown_identity_produces_no_entry() {
        let outcome =
            build_es_de_entry_plan(&CanonicalIdentityStatus::Unknown, &usable_content(), None);
        assert_eq!(
            outcome,
            EsDeExportOutcome::NoEntry(NoEntryReason::IdentityUnresolved)
        );
    }

    #[test]
    fn conflicting_identity_produces_no_entry() {
        let outcome = build_es_de_entry_plan(
            &CanonicalIdentityStatus::Conflicting,
            &usable_content(),
            None,
        );
        assert_eq!(
            outcome,
            EsDeExportOutcome::NoEntry(NoEntryReason::IdentityConflict)
        );
    }

    #[test]
    fn unmapped_platform_produces_no_entry_never_a_guess() {
        let outcome = build_es_de_entry_plan(
            &resolved("Not a reviewed platform"),
            &usable_content(),
            None,
        );
        assert_eq!(
            outcome,
            EsDeExportOutcome::NoEntry(NoEntryReason::PlatformUnmapped {
                platform_id: "Not a reviewed platform".to_string()
            })
        );
    }

    #[test]
    fn unresolved_path_blocks_with_content_unresolved() {
        let outcome = build_es_de_entry_plan(&resolved("PSX"), &unresolved_content(), None);
        let EsDeExportOutcome::Entry(plan) = outcome else {
            panic!("expected an entry");
        };
        assert_eq!(plan.status, EsDeEntryStatus::Blocked);
        assert_eq!(plan.blockers.len(), 1);
        assert_eq!(
            plan.blockers[0].kind,
            EsDeEntryBlockerKind::ContentUnresolved
        );
        assert_eq!(plan.game_path, None);
    }

    #[test]
    fn archive_needing_mount_blocks_with_a_distinct_explanation() {
        let outcome = build_es_de_entry_plan(&resolved("PS2"), &archive_content(), None);
        let EsDeExportOutcome::Entry(plan) = outcome else {
            panic!("expected an entry");
        };
        assert_eq!(plan.status, EsDeEntryStatus::Blocked);
        assert_eq!(plan.blockers.len(), 1);
        assert_eq!(
            plan.blockers[0].kind,
            EsDeEntryBlockerKind::ContentRequiresMount
        );
        assert!(plan.requires_mount);
        assert!(plan.blockers[0].detail.contains("mounted"));
    }

    #[test]
    fn no_launch_plan_means_no_emulator_choice() {
        let outcome = build_es_de_entry_plan(&resolved("PSX"), &usable_content(), None);
        let EsDeExportOutcome::Entry(plan) = outcome else {
            panic!("expected an entry");
        };
        assert_eq!(plan.emulator_choice, None);
    }

    #[test]
    fn sole_eligible_candidate_from_a_launch_plan_becomes_the_emulator_choice() {
        let target = LaunchTarget::Standalone {
            adapter_id: "duckstation",
            profile_id: "default".to_string(),
            profile_path: Some(PathBuf::from("/home/user/.local/share/duckstation")),
        };
        let plan = LaunchPlan {
            platform_id: Some("PSX".to_string()),
            game_key: Some("SLUS-00594".to_string()),
            candidates: vec![LaunchCandidate {
                target: target.clone(),
                content: usable_content(),
                firmware: FirmwareReadiness::Verified,
                blockers: Vec::new(),
                warnings: Vec::new(),
                readiness: LaunchReadiness::Ready,
                preference: CandidatePreference::SoleEligible,
            }],
            summary: LaunchPlanSummary {
                candidates: 1,
                ready: 1,
                ready_with_warnings: 0,
                blocked: 0,
            },
        };

        let outcome = build_es_de_entry_plan(&resolved("PSX"), &usable_content(), Some(&plan));
        let EsDeExportOutcome::Entry(entry) = outcome else {
            panic!("expected an entry");
        };
        assert_eq!(entry.emulator_choice, Some(target));
    }

    #[test]
    fn undetermined_preference_never_becomes_a_guessed_emulator_choice() {
        let candidate = |adapter_id: &'static str| LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id,
                profile_id: "default".to_string(),
                profile_path: None,
            },
            content: usable_content(),
            firmware: FirmwareReadiness::NotRequired,
            blockers: Vec::new(),
            warnings: Vec::new(),
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::Undetermined,
        };
        let plan = LaunchPlan {
            platform_id: Some("GameCube".to_string()),
            game_key: Some("GALE01".to_string()),
            candidates: vec![candidate("dolphin"), candidate("dolphin")],
            summary: LaunchPlanSummary {
                candidates: 2,
                ready: 2,
                ready_with_warnings: 0,
                blocked: 0,
            },
        };

        let outcome = build_es_de_entry_plan(&resolved("GameCube"), &usable_content(), Some(&plan));
        let EsDeExportOutcome::Entry(entry) = outcome else {
            panic!("expected an entry");
        };
        assert_eq!(entry.emulator_choice, None);
    }

    #[test]
    fn blocked_candidate_is_never_offered_as_the_emulator_choice() {
        let plan = LaunchPlan {
            platform_id: Some("PSX".to_string()),
            game_key: Some("SLUS-00594".to_string()),
            candidates: vec![LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: "duckstation",
                    profile_id: "default".to_string(),
                    profile_path: None,
                },
                content: unresolved_content(),
                firmware: FirmwareReadiness::Verified,
                blockers: vec![crate::launch::readiness::LaunchBlocker::new(
                    crate::launch::readiness::LaunchBlockerKind::ContentNotResolved,
                    "no runnable content path was resolved",
                )],
                warnings: Vec::new(),
                readiness: LaunchReadiness::Blocked,
                preference: CandidatePreference::SoleEligible,
            }],
            summary: LaunchPlanSummary {
                candidates: 1,
                ready: 0,
                ready_with_warnings: 0,
                blocked: 1,
            },
        };

        let outcome = build_es_de_entry_plan(&resolved("PSX"), &usable_content(), Some(&plan));
        let EsDeExportOutcome::Entry(entry) = outcome else {
            panic!("expected an entry");
        };
        assert_eq!(entry.emulator_choice, None);
    }
}
