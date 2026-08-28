//! Content/target/candidate/plan data model, and the pure
//! [`build_launch_plan`] planner.
//!
//! # Purity contract
//!
//! [`build_launch_plan`] performs no I/O of any kind: no filesystem read,
//! no network call, no process spawn, no write. Every input is data the
//! caller has *already gathered* elsewhere (identity resolution, content/
//! mount inspection, [`crate::patch_manager`] profile discovery,
//! [`crate::emulator_environment::retroarch::RetroArchEnvironmentReport`]).
//! This function only combines and classifies what it is handed.
//!
//! # Why small input-projection structs instead of the real adapter types
//!
//! [`crate::patch_manager::DuckStationProfile`],
//! [`crate::patch_manager::Pcsx2Profile`], and friends each carry
//! adapter-specific shapes (different path fields, different blocker
//! enums) that would force this pure planner to special-case every
//! adapter by name. [`StandaloneProfileInput`] is the deliberately small,
//! adapter-agnostic shape this module actually needs: an adapter key
//! (matching [`crate::launch::platform_map::LaunchCompatibility::standalone_adapters`]),
//! a profile id, whether the adapter itself considers it eligible, and an
//! already-projected [`crate::launch::readiness::FirmwareReadiness`] (via
//! [`crate::launch::readiness`]'s projection functions - never recomputed
//! here). No existing adapter type is redesigned to produce this; a caller
//! builds one per discovered profile.

use std::path::PathBuf;

use crate::emulator_environment::retroarch::{
    CoreInfoFinding, ProfileRef, RetroArchEnvironmentReport,
};
use crate::launch::platform_map::{launch_compatibility_for_platform, retroarch_platform_matches};
use crate::launch::readiness::{
    FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchReadiness, LaunchWarning,
    LaunchWarningKind, retroarch_core_firmware_readiness,
};

/// The canonical identity this plan was built for, exactly as core's
/// identity layer already resolved it. This module never computes or
/// widens this value - it only ever branches on which variant it already
/// is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalIdentityStatus {
    /// A platform and an opaque game key (a verified serial/disc id/hash -
    /// this module never interprets it, only carries it through to
    /// [`LaunchPlan::game_key`]) were both resolved with confidence.
    Resolved(ResolvedIdentity),
    /// Identity could not be resolved at all.
    Unknown,
    /// More than one incompatible identity conclusion exists - never
    /// silently resolved to one winner.
    Conflicting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIdentity {
    /// A [`crate::platform::Platform::id`] value.
    pub platform_id: String,
    /// An opaque, already-verified identity key (e.g. a PS1 serial, a PS2
    /// executable CRC, a Dolphin Game ID) - carried through, never parsed
    /// or reinterpreted by this module.
    pub game_key: String,
}

/// What kind of content this candidate would actually run, when known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchContentKind {
    OpticalDisc,
    Cartridge,
    Executable,
    Unknown,
}

/// The container format the content is currently held in, when known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchContainerKind {
    PlainFile,
    Chd,
    CueBin,
    Archive,
    Unknown,
}

/// The one piece of content a [`LaunchPlan`] is being built for. Built once
/// per plan and shared by every [`LaunchCandidate`] - content identity does
/// not vary per emulator target.
///
/// This never mounts or materializes anything: `resolved_path` is `Some`
/// only when a caller-supplied, already-available runnable path genuinely
/// exists (e.g. a plain file, or an archive member already mounted by
/// something else this module does not do). If the content lives inside a
/// container that would need mounting to become runnable,
/// `requires_mount` is `true` and `resolved_path` stays `None` unless that
/// mount has already happened elsewhere - see [`Self::has_runnable_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchContentRef {
    pub kind: Option<LaunchContentKind>,
    pub container: Option<LaunchContainerKind>,
    pub resolved_path: Option<PathBuf>,
    pub requires_mount: bool,
    /// What was actually observed, in a person's words - this module's own
    /// equivalent of provenance, matching the convention
    /// [`crate::content_evidence::ContentEvidence::detail`] already uses.
    pub provenance: String,
}

impl LaunchContentRef {
    pub fn has_runnable_path(&self) -> bool {
        self.resolved_path.is_some()
    }
}

/// What a [`LaunchCandidate`] would actually launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    /// A discovered standalone adapter profile
    /// (e.g. `duckstation`/`pcsx2`/`dolphin`).
    Standalone {
        /// Matches [`crate::launch::platform_map::LaunchCompatibility::standalone_adapters`]
        /// entries and [`crate::patch_manager::remember_emulator_profile_to`]'s
        /// own `adapter` key.
        adapter_id: &'static str,
        profile_id: String,
        profile_path: Option<PathBuf>,
    },
    /// An installed RetroArch core whose own `.info` metadata resolved to
    /// this plan's platform - see
    /// [`crate::launch::platform_map::retroarch_platform_candidate`].
    RetroArchCore {
        profile: ProfileRef,
        core_stem: String,
        platform_id: &'static str,
    },
}

/// How this candidate came to be the one worth calling out, if at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidatePreference {
    /// Matches a caller-supplied [`RememberedPreference`].
    Remembered,
    /// The only eligible candidate found for this platform.
    SoleEligible,
    /// More than one eligible candidate exists and nothing distinguishes
    /// them yet.
    Undetermined,
}

/// One way this game could potentially be played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCandidate {
    pub target: LaunchTarget,
    pub content: LaunchContentRef,
    pub firmware: FirmwareReadiness,
    pub blockers: Vec<LaunchBlocker>,
    pub warnings: Vec<LaunchWarning>,
    pub readiness: LaunchReadiness,
    pub preference: CandidatePreference,
}

/// Full description of every currently-knowable way to play one game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// `None` when [`CanonicalIdentityStatus`] was `Unknown`/`Conflicting` -
    /// this plan never invents a platform to fill it in.
    pub platform_id: Option<String>,
    pub game_key: Option<String>,
    pub candidates: Vec<LaunchCandidate>,
    pub summary: LaunchPlanSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LaunchPlanSummary {
    pub candidates: usize,
    pub ready: usize,
    pub ready_with_warnings: usize,
    pub blocked: usize,
}

/// One caller-discovered standalone emulator profile, projected to the
/// small shape this planner needs. See the module doc comment for why this
/// exists instead of the real per-adapter profile types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneProfileInput {
    /// Matches [`crate::launch::platform_map::LaunchCompatibility::standalone_adapters`].
    pub adapter_id: &'static str,
    pub profile_id: String,
    pub profile_path: Option<PathBuf>,
    /// Whether the adapter itself already considers this profile usable
    /// (its own `eligible` field - never recomputed here).
    pub eligible: bool,
    /// Already projected via [`crate::launch::readiness`] - this planner
    /// never re-derives it from an adapter-specific enum.
    pub firmware: FirmwareReadiness,
}

/// A caller-supplied remembered profile choice, decoupled from
/// [`crate::patch_manager::RememberedEmulatorProfile`] so this module does
/// not need to import the profile-memory file format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RememberedPreference {
    pub adapter_id: String,
    pub profile_id: String,
}

fn identity_blocker(status: &CanonicalIdentityStatus) -> Option<LaunchBlocker> {
    match status {
        CanonicalIdentityStatus::Resolved(_) => None,
        CanonicalIdentityStatus::Unknown => Some(LaunchBlocker::new(
            LaunchBlockerKind::IdentityUnresolved,
            "canonical game identity could not be resolved",
        )),
        CanonicalIdentityStatus::Conflicting => Some(LaunchBlocker::new(
            LaunchBlockerKind::IdentityConflict,
            "canonical game identity evidence conflicts and was not resolved to one answer",
        )),
    }
}

fn content_blocker(content: &LaunchContentRef) -> Option<LaunchBlocker> {
    if content.has_runnable_path() {
        return None;
    }
    let detail = if content.requires_mount {
        "content is inside a container that has not been mounted, so no runnable path exists yet"
    } else {
        "no runnable content path was resolved"
    };
    Some(LaunchBlocker::new(
        LaunchBlockerKind::ContentNotResolved,
        detail,
    ))
}

fn firmware_condition(
    firmware: FirmwareReadiness,
) -> (Option<LaunchBlocker>, Option<LaunchWarning>) {
    match firmware {
        FirmwareReadiness::Verified | FirmwareReadiness::NotRequired => (None, None),
        FirmwareReadiness::PresentUnverified => (
            None,
            Some(LaunchWarning::new(
                LaunchWarningKind::FirmwarePresentUnverified,
                "required firmware is present but its contents were not verified",
            )),
        ),
        FirmwareReadiness::Missing => (
            Some(LaunchBlocker::new(
                LaunchBlockerKind::RequiredFirmwareMissing,
                "required firmware/BIOS is missing",
            )),
            None,
        ),
        // Honest uncertainty is not a proven failure: Phase 1 surfaces the
        // readiness value on the candidate itself (never hidden) but does
        // not block or warn on it - see `readiness.rs`'s own doc comment.
        FirmwareReadiness::Unknown => (None, None),
    }
}

fn readiness_from(blockers: &[LaunchBlocker], warnings: &[LaunchWarning]) -> LaunchReadiness {
    if !blockers.is_empty() {
        LaunchReadiness::Blocked
    } else if !warnings.is_empty() {
        LaunchReadiness::ReadyWithWarnings
    } else {
        LaunchReadiness::Ready
    }
}

fn build_standalone_candidates(
    platform_id: &str,
    content: &LaunchContentRef,
    profiles: &[StandaloneProfileInput],
) -> Vec<LaunchCandidate> {
    let Some(compat) = launch_compatibility_for_platform(platform_id) else {
        return Vec::new();
    };
    profiles
        .iter()
        .filter(|profile| compat.standalone_adapters.contains(&profile.adapter_id))
        .map(|profile| {
            let mut blockers = Vec::new();
            let mut warnings = Vec::new();
            if !profile.eligible {
                blockers.push(LaunchBlocker::new(
                    LaunchBlockerKind::ProfileIneligible,
                    "the discovered profile is not eligible",
                ));
            }
            if let Some(blocker) = content_blocker(content) {
                blockers.push(blocker);
            }
            let (firmware_blocker, firmware_warning) = firmware_condition(profile.firmware);
            blockers.extend(firmware_blocker);
            warnings.extend(firmware_warning);
            let readiness = readiness_from(&blockers, &warnings);
            LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: profile.adapter_id,
                    profile_id: profile.profile_id.clone(),
                    profile_path: profile.profile_path.clone(),
                },
                content: content.clone(),
                firmware: profile.firmware,
                blockers,
                warnings,
                readiness,
                preference: CandidatePreference::Undetermined,
            }
        })
        .collect()
}

struct RetroArchCoreMatch {
    profile: ProfileRef,
    core_stem: String,
    info: CoreInfoFinding,
}

fn matching_retroarch_cores(
    platform_id: &str,
    environment: &RetroArchEnvironmentReport,
) -> Vec<RetroArchCoreMatch> {
    let mut matches = Vec::new();
    for profile in &environment.profiles {
        for core in &profile.cores {
            if retroarch_platform_matches(&core.info, platform_id) {
                matches.push(RetroArchCoreMatch {
                    profile: ProfileRef {
                        profile_kind: profile.profile_kind,
                        scope: profile.scope,
                    },
                    core_stem: core.core_stem.clone(),
                    info: core.info.clone(),
                });
            }
        }
    }
    matches
}

fn build_retroarch_candidates(
    platform_id: &'static str,
    content: &LaunchContentRef,
    environment: &RetroArchEnvironmentReport,
    core_hints: &[&'static str],
) -> Vec<LaunchCandidate> {
    let matches = matching_retroarch_cores(platform_id, environment);
    if matches.is_empty() {
        return Vec::new();
    }
    let distinct_stems: std::collections::BTreeSet<String> =
        matches.iter().map(|m| m.core_stem.clone()).collect();
    // More than one genuinely different core resolves to this platform.
    // If a reviewed hint names exactly one of them, that one is preferred
    // (ranking only - it never decided the platform, only which
    // already-valid core to prefer); otherwise automatic selection would
    // be unsafe and every tied candidate is blocked as ambiguous.
    let hinted: Vec<String> = distinct_stems
        .iter()
        .filter(|stem| core_hints.contains(&stem.as_str()))
        .cloned()
        .collect();
    let ambiguous = distinct_stems.len() > 1 && hinted.len() != 1;

    matches
        .into_iter()
        .map(|found| {
            let mut blockers = Vec::new();
            let mut warnings = Vec::new();
            if let Some(blocker) = content_blocker(content) {
                blockers.push(blocker);
            }
            let firmware = retroarch_core_firmware_readiness(&found.info);
            let (firmware_blocker, firmware_warning) = firmware_condition(firmware);
            blockers.extend(firmware_blocker);
            warnings.extend(firmware_warning);
            if ambiguous {
                blockers.push(LaunchBlocker::new(
                    LaunchBlockerKind::AmbiguousCore,
                    format!(
                        "more than one installed RetroArch core resolves to {platform_id} and none is reviewed-preferred"
                    ),
                ));
            }
            let readiness = readiness_from(&blockers, &warnings);
            let preference = if hinted.contains(&found.core_stem) {
                CandidatePreference::SoleEligible
            } else {
                CandidatePreference::Undetermined
            };
            LaunchCandidate {
                target: LaunchTarget::RetroArchCore {
                    profile: found.profile,
                    core_stem: found.core_stem,
                    platform_id,
                },
                content: content.clone(),
                firmware,
                blockers,
                warnings,
                readiness,
                preference,
            }
        })
        .collect()
}

/// Applies remembered-profile preference to standalone candidates, and
/// surfaces [`LaunchWarningKind::MultipleEligibleProfiles`] when more than
/// one is eligible and nothing is remembered.
fn apply_preference(candidates: &mut [LaunchCandidate], remembered: &[RememberedPreference]) {
    let eligible_indices: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.readiness != LaunchReadiness::Blocked)
        .map(|(index, _)| index)
        .collect();

    let mut remembered_index = None;
    for &index in &eligible_indices {
        if let LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } = &candidates[index].target
            && remembered.iter().any(|preference| {
                preference.adapter_id == *adapter_id && preference.profile_id == *profile_id
            })
        {
            remembered_index = Some(index);
            break;
        }
    }

    if let Some(index) = remembered_index {
        candidates[index].preference = CandidatePreference::Remembered;
        return;
    }

    if eligible_indices.len() == 1 {
        candidates[eligible_indices[0]].preference = CandidatePreference::SoleEligible;
        return;
    }

    if eligible_indices.len() > 1 {
        for &index in &eligible_indices {
            candidates[index].preference = CandidatePreference::Undetermined;
            candidates[index].warnings.push(LaunchWarning::new(
                LaunchWarningKind::MultipleEligibleProfiles,
                "more than one eligible profile exists for this platform and none is remembered",
            ));
            candidates[index].readiness =
                readiness_from(&candidates[index].blockers, &candidates[index].warnings);
        }
    }
}

/// Builds the complete [`LaunchPlan`] for one game from already-gathered
/// data. Pure: no filesystem read, no network call, no process spawn, no
/// write, no mutation of `identity`/`content`/discovery inputs.
///
/// `identity` decides *whether* a platform is even considered - see
/// [`CanonicalIdentityStatus`]'s own doc comment. When it is
/// `Unknown`/`Conflicting`, this function deliberately returns an *empty*
/// candidate list rather than guessing which platform's adapters might be
/// relevant: enumerating "DuckStation is installed" for a game whose
/// platform is not even known would imply a relevance this module has no
/// evidence for. [`LaunchBlockerKind::IdentityUnresolved`]/
/// [`LaunchBlockerKind::IdentityConflict`] remain part of the shared
/// vocabulary for a future phase that attaches them to specific
/// candidates; Phase 1 keeps the simpler, equally fail-closed "no
/// candidate" answer.
pub fn build_launch_plan(
    identity: &CanonicalIdentityStatus,
    content: &LaunchContentRef,
    standalone_profiles: &[StandaloneProfileInput],
    retroarch: &RetroArchEnvironmentReport,
    remembered: &[RememberedPreference],
) -> LaunchPlan {
    let CanonicalIdentityStatus::Resolved(resolved) = identity else {
        // `identity_blocker` names the exact reason a future phase would
        // attach here; recorded once so the branch reads as intentional,
        // not merely unimplemented.
        let _ = identity_blocker(identity);
        return LaunchPlan {
            platform_id: None,
            game_key: None,
            candidates: Vec::new(),
            summary: LaunchPlanSummary::default(),
        };
    };

    let compat = launch_compatibility_for_platform(&resolved.platform_id);
    let mut candidates =
        build_standalone_candidates(&resolved.platform_id, content, standalone_profiles);

    if let Some(platform) = crate::platform::platform_by_id(&resolved.platform_id) {
        candidates.extend(build_retroarch_candidates(
            platform.id,
            content,
            retroarch,
            compat
                .map(|entry| entry.retroarch_core_hints)
                .unwrap_or(&[]),
        ));
    }

    if candidates.is_empty() {
        candidates.push(no_candidate_placeholder(content));
    }

    apply_preference(&mut candidates, remembered);

    let summary = summarize(&candidates);
    LaunchPlan {
        platform_id: Some(resolved.platform_id.clone()),
        game_key: Some(resolved.game_key.clone()),
        candidates,
        summary,
    }
}

/// A platform resolved cleanly but nothing installed targets it at all -
/// surfaced as one explicit [`LaunchBlockerKind::NoInstallationCandidate`]
/// entry rather than an empty list, so a caller can tell "we know the
/// platform, nothing can play it yet" apart from "identity itself failed".
fn no_candidate_placeholder(content: &LaunchContentRef) -> LaunchCandidate {
    let blockers = vec![LaunchBlocker::new(
        LaunchBlockerKind::NoInstallationCandidate,
        "no discovered standalone profile or installed RetroArch core targets this platform",
    )];
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "none",
            profile_id: String::new(),
            profile_path: None,
        },
        content: content.clone(),
        firmware: FirmwareReadiness::Unknown,
        readiness: LaunchReadiness::Blocked,
        warnings: Vec::new(),
        blockers,
        preference: CandidatePreference::Undetermined,
    }
}

fn summarize(candidates: &[LaunchCandidate]) -> LaunchPlanSummary {
    let mut summary = LaunchPlanSummary {
        candidates: candidates.len(),
        ..Default::default()
    };
    for candidate in candidates {
        match candidate.readiness {
            LaunchReadiness::Ready => summary.ready += 1,
            LaunchReadiness::ReadyWithWarnings => summary.ready_with_warnings += 1,
            LaunchReadiness::Blocked => summary.blocked += 1,
        }
    }
    summary
}
