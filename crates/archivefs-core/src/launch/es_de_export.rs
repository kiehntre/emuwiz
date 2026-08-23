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
        platform_id: "Dreamcast",
        es_de_system: "dreamcast",
        es_de_fullname: "Sega Dreamcast",
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
    use crate::platform::platform_by_id;

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
            "Xbox",
            "Xbox360",
            "GameCube",
            "Wii",
            "Dreamcast",
            "AtariST",
            "Amiga",
            "NES",
            "SNES",
            "MegaDrive",
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
        let outcome = build_es_de_entry_plan(&resolved("Saturn"), &usable_content(), None);
        assert_eq!(
            outcome,
            EsDeExportOutcome::NoEntry(NoEntryReason::PlatformUnmapped {
                platform_id: "Saturn".to_string()
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
