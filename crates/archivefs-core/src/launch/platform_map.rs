//! Canonical-platform -> downstream-target compatibility, and RetroArch
//! candidate generation.
//!
//! # RetroArch metadata is downstream only
//!
//! Everything in this file reads *from* [`crate::platform::PLATFORMS`] -
//! nothing ever writes to it, nothing here is consulted by
//! [`crate::platform::detect`]/[`crate::dat::identity`]/
//! [`crate::platform_evidence_fusion`], and no [`LaunchCompatibility`] entry
//! is ever treated as identity evidence. A RetroArch core's `.info`
//! `systemname`/`database` text can, at most, produce a *launch candidate*
//! for a platform this crate's identity layer has *already* resolved
//! independently - see [`crate::launch::planning::build_launch_plan`] for
//! where that check actually happens. This module only ever answers "what
//! platform would this core, if chosen, target" - never "what platform is
//! this game."
//!
//! # No giant hand-written platform -> core table
//!
//! [`LAUNCH_COMPATIBILITY`] is deliberately small: it lists only the
//! *standalone* adapter targets (reviewed one at a time, exactly as
//! audited) and a handful of purely corroborating
//! [`LaunchCompatibility::retroarch_core_hints`]. Candidate *generation*
//! for RetroArch does not come from a table at all - it comes from
//! [`retroarch_platform_candidate`], which resolves an installed core's own
//! `.info` `systemname`/`database` text through the platform registry's
//! existing, already-reviewed alias resolver
//! ([`crate::platform::platform_for_alias`]), the same exact-match-only
//! mechanism [`crate::dat::identity`] already uses for DAT header text.
//! `corename`, `manufacturer`, and `categories` are read by
//! [`crate::emulator_environment::retroarch`] but never consulted here at
//! all: a libretro-internal core name is not standardized text, and using
//! it to authorize a platform would be exactly the kind of fuzzy,
//! unreviewed inference this module refuses to do.
//! `supported_extensions` is read only to narrow/rank an *already-valid*
//! candidate - see [`extension_narrows_candidate`] - never to create one.

use crate::emulator_environment::retroarch::CoreInfoFinding;
use crate::platform::{platform_by_id, platform_for_alias};

/// How confidently a [`LaunchCompatibility`] entry's target(s) are known to
/// apply to its platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingConfidence {
    /// One reviewed standalone adapter is the platform's only real target.
    Exact,
    /// A real, reviewed target exists, but more than one is plausible (e.g.
    /// Dolphin serves both GameCube and Wii) or the mapping still carries a
    /// caveat worth naming.
    StronglyKnown,
    /// Real evidence exists but is genuinely shared with another platform;
    /// automatic selection among the possibilities would be unsafe.
    Ambiguous,
    /// No standalone adapter and no reviewed RetroArch hint exist for this
    /// platform in this build.
    Unsupported,
}

/// One canonical platform's known downstream compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchCompatibility {
    /// A [`crate::platform::Platform::id`] value - never a new namespace.
    pub platform_id: &'static str,
    /// Stable adapter keys - the same strings
    /// [`crate::patch_manager::remember_emulator_profile_to`] already uses
    /// (e.g. `"duckstation"`, `"pcsx2"`), never a display label.
    pub standalone_adapters: &'static [&'static str],
    /// Reviewed libretro core names this platform is known to have a real
    /// core for, purely as a ranking/corroboration hint - see the module
    /// doc comment. Never sufficient on its own to create a candidate.
    pub retroarch_core_hints: &'static [&'static str],
    pub confidence: MappingConfidence,
}

/// The reviewed table. Every row here mirrors exactly what the prior
/// read-only audit already confirmed in code - see each `*_local.rs`
/// adapter's own module doc comment for the platform it targets.
/// Deliberately incomplete: a platform absent from this table is
/// [`MappingConfidence::Unsupported`] for standalone-adapter purposes
/// (RetroArch candidate generation still works for it via
/// [`retroarch_platform_candidate`], independent of this table).
pub const LAUNCH_COMPATIBILITY: &[LaunchCompatibility] = &[
    LaunchCompatibility {
        platform_id: "PSX",
        standalone_adapters: &["duckstation"],
        retroarch_core_hints: &["mednafen_psx", "mednafen_psx_hw", "pcsx_rearmed"],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "PS2",
        standalone_adapters: &["pcsx2"],
        retroarch_core_hints: &[],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "PS3",
        standalone_adapters: &["rpcs3"],
        retroarch_core_hints: &[],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "PSP",
        standalone_adapters: &["ppsspp"],
        retroarch_core_hints: &["ppsspp"],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "Xbox",
        standalone_adapters: &["xemu"],
        retroarch_core_hints: &[],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "Xbox360",
        standalone_adapters: &["xenia"],
        retroarch_core_hints: &[],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "GameCube",
        standalone_adapters: &["dolphin"],
        retroarch_core_hints: &["dolphin"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Wii",
        standalone_adapters: &["dolphin"],
        retroarch_core_hints: &["dolphin"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Dreamcast",
        standalone_adapters: &["flycast"],
        retroarch_core_hints: &["flycast"],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "Sega CD",
        standalone_adapters: &[],
        retroarch_core_hints: &["genesis_plus_gx"],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "AtariST",
        standalone_adapters: &["hatari"],
        retroarch_core_hints: &["hatari"],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "Amiga",
        standalone_adapters: &["amiga_whdload"],
        retroarch_core_hints: &["puae"],
        confidence: MappingConfidence::Exact,
    },
    LaunchCompatibility {
        platform_id: "NES",
        standalone_adapters: &[],
        retroarch_core_hints: &["nestopia"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "SNES",
        standalone_adapters: &[],
        retroarch_core_hints: &["snes9x"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Game Boy",
        standalone_adapters: &[],
        retroarch_core_hints: &["gambatte"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Game Boy Color",
        standalone_adapters: &[],
        retroarch_core_hints: &["gambatte"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Game Boy Advance",
        standalone_adapters: &[],
        retroarch_core_hints: &["mgba"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Virtual Boy",
        standalone_adapters: &[],
        retroarch_core_hints: &["beetle_vb", "mednafen_vb"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "N64",
        standalone_adapters: &[],
        retroarch_core_hints: &["mupen64plus_next"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "MegaDrive",
        standalone_adapters: &[],
        retroarch_core_hints: &["genesis_plus_gx"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Atari2600",
        standalone_adapters: &[],
        retroarch_core_hints: &["stella"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Atari5200",
        standalone_adapters: &[],
        retroarch_core_hints: &["a5200"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Atari Lynx",
        standalone_adapters: &[],
        retroarch_core_hints: &["handy"],
        confidence: MappingConfidence::StronglyKnown,
    },
    LaunchCompatibility {
        platform_id: "Arcade",
        standalone_adapters: &["mame"],
        retroarch_core_hints: &["mame", "mame2003_plus", "fbneo"],
        confidence: MappingConfidence::Exact,
    },
    // ScummVM's command and execution adapters are complete and require a
    // detector-verified engine/game identity plus an extracted game folder.
    // This row only makes that already-reviewed adapter reachable by generic
    // launch planning; it is not identity evidence and does not infer a game
    // from a filename or directory name.
    LaunchCompatibility {
        platform_id: "ScummVM",
        standalone_adapters: &["scummvm"],
        retroarch_core_hints: &[],
        confidence: MappingConfidence::Exact,
    },
];

/// The reviewed row for `platform_id`, if any.
pub fn launch_compatibility_for_platform(
    platform_id: &str,
) -> Option<&'static LaunchCompatibility> {
    LAUNCH_COMPATIBILITY
        .iter()
        .find(|entry| entry.platform_id == platform_id)
}

/// Every reviewed row whose `standalone_adapters` names `adapter_id` - the
/// reverse of [`launch_compatibility_for_platform`], used to find which
/// platform(s) a *discovered installed profile* is a candidate for without
/// requiring the caller to already know the platform.
pub fn platforms_for_standalone_adapter(adapter_id: &str) -> Vec<&'static str> {
    LAUNCH_COMPATIBILITY
        .iter()
        .filter(|entry| entry.standalone_adapters.contains(&adapter_id))
        .map(|entry| entry.platform_id)
        .collect()
}

/// The same separators [`crate::dat::identity`] already established for
/// splitting header/RDB-style text (`"Manufacturer - System Name"`,
/// `"System / Alt Name"`) into individually resolvable segments. Kept as
/// a small local copy rather than reaching into that module's private
/// helpers, so this module's only dependency on identity code is the
/// public, reviewed [`platform_for_alias`] resolver itself.
const SEGMENT_SEPARATORS: &[&str] = &[" - ", "/", "&", ","];

fn segments(text: &str) -> Vec<String> {
    let mut parts = vec![text.to_string()];
    for separator in SEGMENT_SEPARATORS {
        parts = parts
            .into_iter()
            .flat_map(|part| {
                part.split(separator)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

/// Resolves one piece of RetroArch `.info` text (a `systemname` or
/// `database` value) to a canonical platform id, using only the existing
/// alias resolver and only exact matches - no fuzzy matching. Tries the
/// whole string first (the common case: `.info` `systemname` values are
/// often already a bare platform name), then each conservatively-split
/// segment. Returns `None` - never a guess - the instant no segment
/// resolves.
fn resolve_info_text(text: &str) -> Option<&'static str> {
    if let Some(platform) = platform_for_alias(text) {
        return Some(platform.id);
    }
    for segment in segments(text) {
        if let Some(platform) = platform_for_alias(&segment) {
            return Some(platform.id);
        }
    }
    None
}

/// The platform a RetroArch core's own `.info` metadata resolves to, if
/// any. Only `systemname` and `database` are ever consulted, in that
/// order, and only through [`resolve_info_text`]'s exact-alias resolution -
/// `corename`, `manufacturer`, and `categories` are never read here at
/// all. `.info` metadata that failed to parse
/// ([`CoreInfoFinding`] variants other than `Found`) or that resolves to
/// nothing recognizable answers `None` - "Unknown, no candidate" - never a
/// guess.
pub fn retroarch_platform_candidate(info: &CoreInfoFinding) -> Option<&'static str> {
    let CoreInfoFinding::Found {
        system_name,
        database,
        ..
    } = info
    else {
        return None;
    };
    system_name
        .as_deref()
        .and_then(resolve_info_text)
        .or_else(|| database.as_deref().and_then(resolve_info_text))
}

/// Whether a core's reviewed `.info` metadata explicitly names `platform_id`.
///
/// A single-system core resolves straight through
/// [`retroarch_platform_candidate`]. A multi-system core, though, advertises
/// a whole hardware family in one `database` value: Genesis Plus GX's real
/// `systemname` is `Sega 8/16-bit (Various)` (which resolves to nothing) and
/// its `database` is the `|`-separated list `Sega - Game Gear|Sega - Master
/// System - Mark III|Sega - Mega-CD - Sega CD|Sega - Mega Drive - Genesis|
/// ...`, whose *first* resolvable entry is Master System. Gambatte's is
/// `Nintendo - Game Boy|Nintendo - Game Boy Color`; mGBA's adds
/// `|Nintendo - Game Boy Advance`. [`retroarch_platform_candidate`] can only
/// name one platform, so on its own it hides every other system the core
/// genuinely runs.
///
/// When the single candidate does not match the requested platform, this
/// also checks each `|`-separated `database` alternative through the same
/// exact-alias [`resolve_info_text`] resolution and returns `true` when any
/// of them names `platform_id` exactly. Nothing here is fuzzy: an
/// alternative that does not resolve exactly contributes nothing, an absent
/// or empty `database` contributes nothing, and `corename`, `manufacturer`,
/// `categories`, and the core's filename are still never consulted.
///
/// A multi-system core is therefore a legitimate candidate for every
/// platform its own `database` explicitly names - intended. The candidate
/// still has to pass every readiness/firmware/content check unchanged, and
/// disambiguation among several matching cores is left to
/// [`LaunchCompatibility::retroarch_core_hints`] exactly as before. (The
/// previous Sega CD-only form of this check is now just one case of the
/// general rule.)
pub fn retroarch_platform_matches(info: &CoreInfoFinding, platform_id: &str) -> bool {
    if retroarch_platform_candidate(info) == Some(platform_id) {
        return true;
    }
    matches!(
        info,
        CoreInfoFinding::Found { database: Some(database), .. }
            if database
                .split('|')
                .any(|alternative| resolve_info_text(alternative) == Some(platform_id))
    )
}

/// Whether `extension` (already lowercased, no dot) is plausible content
/// for `platform_id` at all, per the platform registry's own
/// `strong_extensions`/`weak_extensions`. This never creates or authorizes
/// a candidate by itself - see the module doc comment - it exists only so
/// a caller disambiguating between several *already-platform-valid*
/// RetroArch cores can use a content file's extension as one more ranking
/// signal, exactly the same "narrows, never decides" role
/// [`crate::platform::Platform::weak_extensions`] already plays for file
/// detection.
pub fn extension_narrows_candidate(extension: &str, platform_id: &str) -> bool {
    platform_by_id(platform_id).is_some_and(|platform| platform.accepts_extension(extension))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator_environment::retroarch::FirmwareRequirement;

    fn found(system_name: Option<&str>, database: Option<&str>) -> CoreInfoFinding {
        CoreInfoFinding::Found {
            display_name: None,
            display_version: None,
            system_name: system_name.map(str::to_string),
            supported_extensions: Vec::new(),
            core_name: Some("some_core".to_string()),
            manufacturer: None,
            categories: None,
            database: database.map(str::to_string),
            firmware: Vec::new(),
        }
    }

    #[test]
    fn every_row_platform_id_exists_in_the_registry() {
        for entry in LAUNCH_COMPATIBILITY {
            assert!(
                platform_by_id(entry.platform_id).is_some(),
                "{} is not a real Platform::id",
                entry.platform_id
            );
        }
    }

    #[test]
    fn virtual_boy_uses_reviewed_retroarch_core_hints() {
        let info = found(Some("Nintendo - Virtual Boy"), None);
        assert_eq!(retroarch_platform_candidate(&info), Some("Virtual Boy"));
        assert!(retroarch_platform_matches(&info, "Virtual Boy"));
        let entry = launch_compatibility_for_platform("Virtual Boy").expect("reviewed row");
        assert_eq!(entry.retroarch_core_hints, &["beetle_vb", "mednafen_vb"]);
        assert!(entry.standalone_adapters.is_empty());
    }

    #[test]
    fn every_required_platform_is_covered() {
        for platform_id in [
            "PSX",
            "PS2",
            "PS3",
            "PSP",
            "Xbox",
            "Xbox360",
            "GameCube",
            "Wii",
            "Dreamcast",
            "AtariST",
            "Amiga",
            "NES",
            "SNES",
            "Game Boy",
            "Game Boy Color",
            "Game Boy Advance",
            "N64",
            "MegaDrive",
            "Atari2600",
            "Atari5200",
            "Atari Lynx",
        ] {
            assert!(
                launch_compatibility_for_platform(platform_id).is_some(),
                "{platform_id} is missing from LAUNCH_COMPATIBILITY"
            );
        }
    }

    #[test]
    fn psx_maps_to_duckstation_exactly() {
        let entry = launch_compatibility_for_platform("PSX").unwrap();
        assert_eq!(entry.standalone_adapters, &["duckstation"]);
        assert_eq!(entry.confidence, MappingConfidence::Exact);
    }

    #[test]
    fn dolphin_serves_both_gamecube_and_wii() {
        assert!(platforms_for_standalone_adapter("dolphin").contains(&"GameCube"));
        assert!(platforms_for_standalone_adapter("dolphin").contains(&"Wii"));
    }

    #[test]
    fn unsupported_platform_has_no_row() {
        assert!(launch_compatibility_for_platform("Saturn").is_none());
    }

    #[test]
    fn classic_console_rows_have_exactly_one_reviewed_hint() {
        for platform_id in [
            "NES",
            "SNES",
            "Game Boy",
            "Game Boy Color",
            "Game Boy Advance",
            "N64",
            "MegaDrive",
            "Atari2600",
            "Atari5200",
            "Atari Lynx",
        ] {
            let row = launch_compatibility_for_platform(platform_id).unwrap();
            assert!(row.standalone_adapters.is_empty(), "{platform_id}");
            assert_eq!(row.retroarch_core_hints.len(), 1, "{platform_id}");
            assert_eq!(row.confidence, MappingConfidence::StronglyKnown);
        }
    }

    #[test]
    fn systemname_that_is_already_a_bare_platform_name_resolves() {
        let info = found(Some("SNES"), None);
        assert_eq!(retroarch_platform_candidate(&info), Some("SNES"));
    }

    #[test]
    fn header_style_systemname_resolves_via_segment_split() {
        // The real snes9x.info shape confirmed in
        // `emulator_environment::retroarch`'s own tests.
        let info = found(Some("Nintendo - SNES / SFC"), None);
        assert_eq!(retroarch_platform_candidate(&info), Some("SNES"));
    }

    #[test]
    fn reviewed_genesis_plus_gx_sega_cd_metadata_resolves_to_sega_cd() {
        let info = found(
            Some("Sega - MS/GG/MD/CD"),
            Some(
                "Sega - Game Gear|Sega - Master System - Mark III|Sega - Mega-CD - Sega CD|Sega - Mega Drive - Genesis",
            ),
        );
        assert_eq!(retroarch_platform_candidate(&info), Some("GameGear"));
        assert!(retroarch_platform_matches(&info, "Sega CD"));
        assert!(
            launch_compatibility_for_platform("Sega CD")
                .expect("Sega CD has a reviewed RetroArch mapping")
                .retroarch_core_hints
                .contains(&"genesis_plus_gx")
        );
    }

    #[test]
    fn database_is_used_only_when_systemname_does_not_resolve() {
        let info = found(None, Some("Nintendo - Nintendo Entertainment System"));
        assert_eq!(retroarch_platform_candidate(&info), Some("NES"));
    }

    #[test]
    fn non_resolving_metadata_stays_unknown() {
        let info = found(Some("Totally Unrecognized Core System"), None);
        assert_eq!(retroarch_platform_candidate(&info), None);
    }

    #[test]
    fn corename_alone_never_creates_a_candidate() {
        let info = CoreInfoFinding::Found {
            display_name: None,
            display_version: None,
            system_name: None,
            supported_extensions: Vec::new(),
            core_name: Some("Nestopia".to_string()),
            manufacturer: Some("Nintendo".to_string()),
            categories: None,
            database: None,
            firmware: vec![FirmwareRequirement {
                index: 0,
                path: None,
                description: None,
                optional: true,
            }],
        };
        assert_eq!(retroarch_platform_candidate(&info), None);
    }

    #[test]
    fn missing_info_never_creates_a_candidate() {
        assert_eq!(
            retroarch_platform_candidate(&CoreInfoFinding::Missing),
            None
        );
    }

    #[test]
    fn extension_narrowing_never_authorizes_a_platform_on_its_own() {
        // A shared CD extension is plausible for several platforms; this
        // helper only ever answers the narrow yes/no question for one
        // already-chosen platform, never picks among them.
        assert!(extension_narrows_candidate("iso", "PSX"));
        assert!(extension_narrows_candidate("iso", "PS2"));
        assert!(!extension_narrows_candidate("z64", "PSX"));
    }

    // Real libretro-core-info metadata: `systemname` is a shared-family
    // display string that resolves to nothing (or to the wrong first
    // platform), and `database` is the authoritative `|`-separated list.

    fn genesis_plus_gx_real() -> CoreInfoFinding {
        found(
            Some("Sega 8/16-bit (Various)"),
            Some(
                "Sega - Game Gear|Sega - Master System - Mark III|Sega - Mega-CD - Sega CD|Sega - Mega Drive - Genesis|Sega - PICO|Sega - SG-1000",
            ),
        )
    }

    fn gambatte_real() -> CoreInfoFinding {
        found(
            Some("Game Boy/Game Boy Color"),
            Some("Nintendo - Game Boy|Nintendo - Game Boy Color"),
        )
    }

    fn mgba_real() -> CoreInfoFinding {
        found(
            Some("Game Boy/Game Boy Color/Game Boy Advance"),
            Some("Nintendo - Game Boy|Nintendo - Game Boy Color|Nintendo - Game Boy Advance"),
        )
    }

    #[test]
    fn genesis_plus_gx_real_metadata_matches_every_database_platform() {
        let info = genesis_plus_gx_real();
        // Its single resolvable candidate is not Mega Drive.
        assert_ne!(retroarch_platform_candidate(&info), Some("MegaDrive"));
        // But every platform its own database names is now a match.
        assert!(retroarch_platform_matches(&info, "MegaDrive"));
        assert!(retroarch_platform_matches(&info, "MasterSystem"));
        assert!(retroarch_platform_matches(&info, "GameGear"));
        assert!(retroarch_platform_matches(&info, "Sega CD"));
    }

    #[test]
    fn gambatte_real_metadata_matches_game_boy_and_game_boy_color() {
        let info = gambatte_real();
        assert!(retroarch_platform_matches(&info, "Game Boy"));
        assert!(retroarch_platform_matches(&info, "Game Boy Color"));
    }

    #[test]
    fn mgba_real_metadata_matches_gb_gbc_and_gba() {
        let info = mgba_real();
        assert!(retroarch_platform_matches(&info, "Game Boy"));
        assert!(retroarch_platform_matches(&info, "Game Boy Color"));
        assert!(retroarch_platform_matches(&info, "Game Boy Advance"));
    }

    #[test]
    fn multi_system_core_does_not_match_an_unrelated_platform() {
        assert!(!retroarch_platform_matches(&genesis_plus_gx_real(), "PSX"));
        assert!(!retroarch_platform_matches(&genesis_plus_gx_real(), "N64"));
        assert!(!retroarch_platform_matches(
            &gambatte_real(),
            "Game Boy Advance"
        ));
        assert!(!retroarch_platform_matches(&mgba_real(), "SNES"));
    }

    #[test]
    fn empty_or_malformed_database_never_creates_a_match() {
        // No database at all: only the single resolvable candidate matches.
        let systemname_only = found(Some("Game Boy/Game Boy Color/Game Boy Advance"), None);
        assert!(retroarch_platform_matches(&systemname_only, "Game Boy"));
        assert!(!retroarch_platform_matches(
            &systemname_only,
            "Game Boy Color"
        ));
        assert!(!retroarch_platform_matches(
            &systemname_only,
            "Game Boy Advance"
        ));

        for junk in [
            "",
            "|||",
            "   |   ",
            "Not - A - Real - System|also nonsense",
        ] {
            let info = found(None, Some(junk));
            assert!(!retroarch_platform_matches(&info, "MegaDrive"));
            assert!(!retroarch_platform_matches(&info, "Game Boy"));
        }

        assert!(!retroarch_platform_matches(
            &CoreInfoFinding::Missing,
            "NES"
        ));
    }

    #[test]
    fn sega_cd_database_alternative_still_matches() {
        // The previous Sega CD-only special case is preserved by the general rule,
        // for both the real and the older fabricated systemname shapes.
        assert!(retroarch_platform_matches(
            &genesis_plus_gx_real(),
            "Sega CD"
        ));
        let fabricated = found(
            Some("Sega - MS/GG/MD/CD"),
            Some(
                "Sega - Game Gear|Sega - Master System - Mark III|Sega - Mega-CD - Sega CD|Sega - Mega Drive - Genesis",
            ),
        );
        assert!(retroarch_platform_matches(&fabricated, "Sega CD"));
    }

    #[test]
    fn single_system_core_matches_only_its_own_platform() {
        // Stella: systemname resolves directly; database names just one system.
        let stella = found(Some("Atari 2600"), Some("Atari - 2600"));
        assert_eq!(retroarch_platform_candidate(&stella), Some("Atari2600"));
        assert!(retroarch_platform_matches(&stella, "Atari2600"));
        assert!(!retroarch_platform_matches(&stella, "Atari7800"));
        assert!(!retroarch_platform_matches(&stella, "MegaDrive"));

        // snes9x-style: header-style systemname, single-system database.
        let snes9x = found(
            Some("Nintendo - SNES / SFC"),
            Some("Nintendo - Super Nintendo Entertainment System"),
        );
        assert!(retroarch_platform_matches(&snes9x, "SNES"));
        assert!(!retroarch_platform_matches(&snes9x, "NES"));
        assert!(!retroarch_platform_matches(&snes9x, "Game Boy"));
    }
}
