//! Game-specific firmware readiness over an already selected launch candidate.
//!
//! This is a pure projection. It performs no discovery, hashing, filesystem
//! I/O, network access, emulator configuration, copying, or downloading. The
//! selected [`LaunchCandidate`](crate::launch::planning::LaunchCandidate) is
//! the authoritative binding between a game and an emulator/profile; global
//! BIOS inventory is only evidence supplied to that candidate.
//!
//! It is an explanation layer over the coarse launch-readiness
//! [`FirmwareReadiness`], not a second firmware authority: it never changes a
//! launch decision, never invents a firmware fact, and fails closed. In
//! particular:
//!
//! * `Ready` needs content-level evidence (an exact known hash, or an identity
//!   the emulator itself verified) or the candidate's own `Verified`, and a
//!   freshness record that is `Current`. A filename, presence, or a
//!   signature-and-size match alone is only `PresentUnverified`.
//! * Evidence that contradicts the candidate (accepted firmware where the
//!   launch plan says `Missing`, or a requirement the descriptor calls
//!   optional where the plan says it is needed) is `Unknown`, never a guess.
//! * Unknown freshness never produces `Ready`.

use std::path::PathBuf;

use serde::Serialize;

use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use crate::launch::planning::{LaunchCandidate, LaunchTarget};
use crate::launch::readiness::FirmwareReadiness;

/// The user-facing readiness verdict for one game and one selected
/// emulator/profile. Adapter-specific evidence is retained in
/// [`FirmwareEvidenceSummary`] instead of being flattened into this enum.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GameFirmwareState {
    NotRequired,
    Ready,
    PresentUnverified,
    Missing,
    WrongRegion,
    WrongVersion,
    HashMismatch,
    MultipleValidOptions,
    Unknown,
    EmulatorManaged,
    Stale,
}

/// Whether the inputs used to calculate a projection still describe the
/// selected game/profile/files/requirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FirmwareFreshnessState {
    Current,
    Stale,
    Unknown,
}

/// Typed explanation for the freshness state. A change to any one component
/// invalidates a prior readiness answer; no Ready result is permanent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FirmwareFreshness {
    pub state: FirmwareFreshnessState,
    pub profile_fingerprint: Option<String>,
    pub firmware_path_fingerprint: Option<String>,
    pub firmware_file_fingerprint: Option<String>,
    pub game_identity_fingerprint: Option<String>,
    pub requirement_fingerprint: Option<String>,
    pub stale_reason: Option<FirmwareStaleReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FirmwareStaleReason {
    ProfileChanged,
    FirmwarePathChanged,
    FirmwareFileChanged,
    GameIdentityChanged,
    RequirementChanged,
    InputUnavailable,
}

impl FirmwareFreshness {
    pub fn current(snapshot: FirmwareFreshnessSnapshot) -> Self {
        Self {
            state: if snapshot.all_known() {
                FirmwareFreshnessState::Current
            } else {
                FirmwareFreshnessState::Unknown
            },
            profile_fingerprint: snapshot.profile_fingerprint,
            firmware_path_fingerprint: snapshot.firmware_path_fingerprint,
            firmware_file_fingerprint: snapshot.firmware_file_fingerprint,
            game_identity_fingerprint: snapshot.game_identity_fingerprint,
            requirement_fingerprint: snapshot.requirement_fingerprint,
            stale_reason: None,
        }
    }

    /// Compare a previously computed snapshot with current inputs. This is a
    /// value comparison only; callers own the gathering of fingerprints.
    pub fn compare(
        previous: &FirmwareFreshnessSnapshot,
        current: FirmwareFreshnessSnapshot,
    ) -> Self {
        let reason = if previous.profile_fingerprint != current.profile_fingerprint {
            Some(stale_reason(
                &previous.profile_fingerprint,
                &current.profile_fingerprint,
                FirmwareStaleReason::ProfileChanged,
            ))
        } else if previous.firmware_path_fingerprint != current.firmware_path_fingerprint {
            Some(stale_reason(
                &previous.firmware_path_fingerprint,
                &current.firmware_path_fingerprint,
                FirmwareStaleReason::FirmwarePathChanged,
            ))
        } else if previous.firmware_file_fingerprint != current.firmware_file_fingerprint {
            Some(stale_reason(
                &previous.firmware_file_fingerprint,
                &current.firmware_file_fingerprint,
                FirmwareStaleReason::FirmwareFileChanged,
            ))
        } else if previous.game_identity_fingerprint != current.game_identity_fingerprint {
            Some(stale_reason(
                &previous.game_identity_fingerprint,
                &current.game_identity_fingerprint,
                FirmwareStaleReason::GameIdentityChanged,
            ))
        } else if previous.requirement_fingerprint != current.requirement_fingerprint {
            Some(stale_reason(
                &previous.requirement_fingerprint,
                &current.requirement_fingerprint,
                FirmwareStaleReason::RequirementChanged,
            ))
        } else {
            None
        };

        let mut result = Self::current(current);
        if let Some(reason) = reason {
            result.state = FirmwareFreshnessState::Stale;
            result.stale_reason = Some(reason);
        }
        result
    }
}

/// A component that was known and can no longer be gathered is
/// `InputUnavailable`; otherwise the named component changed.
fn stale_reason(
    previous: &Option<String>,
    current: &Option<String>,
    changed: FirmwareStaleReason,
) -> FirmwareStaleReason {
    if previous.is_some() && current.is_none() {
        FirmwareStaleReason::InputUnavailable
    } else {
        changed
    }
}

/// Fingerprints gathered by an adapter/profile owner. The projection never
/// computes or persists them.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct FirmwareFreshnessSnapshot {
    pub profile_fingerprint: Option<String>,
    pub firmware_path_fingerprint: Option<String>,
    pub firmware_file_fingerprint: Option<String>,
    pub game_identity_fingerprint: Option<String>,
    pub requirement_fingerprint: Option<String>,
}

impl FirmwareFreshnessSnapshot {
    fn all_known(&self) -> bool {
        self.profile_fingerprint.is_some()
            && self.firmware_path_fingerprint.is_some()
            && self.firmware_file_fingerprint.is_some()
            && self.game_identity_fingerprint.is_some()
            && self.requirement_fingerprint.is_some()
    }
}

/// How an evidence item was established. The ordering is intentional: a
/// filename is never promoted to a verified hash match.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FirmwareEvidenceStrength {
    ExactKnownHash,
    EmulatorVerifiedIdentity,
    StrongSignatureAndSize,
    FilenameOrPresenceOnly,
}

impl FirmwareEvidenceStrength {
    /// Only an exact known hash or an identity the emulator itself verified can
    /// make firmware `Ready`. A signature-and-size match is a useful hint but
    /// not verification of the contents.
    pub fn is_content_verified(self) -> bool {
        matches!(self, Self::ExactKnownHash | Self::EmulatorVerifiedIdentity)
    }
}

/// Adapter-owned detail retained for consumers that need to explain a
/// projection without inventing generic region/version rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FirmwareEvidenceStatus {
    Accepted,
    HashMismatch,
    RegionMismatchProven,
    VersionMismatchProven,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FirmwareEvidenceSummary {
    pub option_id: Option<String>,
    pub path: Option<PathBuf>,
    pub filename: Option<String>,
    pub sha256: Option<String>,
    pub strength: FirmwareEvidenceStrength,
    pub status: FirmwareEvidenceStatus,
    pub region: Option<String>,
    pub version: Option<String>,
    pub provenance: String,
}

/// One accepted firmware/system-file choice. Multiple entries are valid when
/// an emulator supports alternative BIOSes or revisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FirmwareOption {
    pub id: String,
    pub label: String,
    pub accepted_sha256: Vec<String>,
    pub region: Option<String>,
    pub version: Option<String>,
    pub emulator_managed: bool,
}

/// Adapter-owned requirement descriptor. It is deliberately descriptive and
/// does not become a second firmware database.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FirmwareRequirementDescriptor {
    pub id: String,
    pub label: String,
    pub required: bool,
    pub options: Vec<FirmwareOption>,
    pub emulator_managed: bool,
    pub region_requirement_proven: bool,
    pub version_requirement_proven: bool,
}

impl FirmwareRequirementDescriptor {
    pub fn not_required(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            required: false,
            options: Vec::new(),
            emulator_managed: false,
            region_requirement_proven: false,
            version_requirement_proven: false,
        }
    }
}

/// Inputs for one game/profile projection. All values are already gathered by
/// an adapter or launch planner; this function only composes them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameFirmwareReadinessInput<'a> {
    pub game_id: Option<String>,
    pub platform: String,
    pub release_identity: Option<String>,
    pub game_region: Option<String>,
    pub game_revision: Option<String>,
    pub candidate: &'a LaunchCandidate,
    pub requirement: FirmwareRequirementDescriptor,
    pub evidence: Vec<FirmwareEvidenceSummary>,
    pub selected_option_id: Option<String>,
    pub freshness: FirmwareFreshness,
}

/// Shared game-specific answer consumed by launch/readiness surfaces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GameFirmwareReadiness {
    pub game_id: Option<String>,
    pub platform: String,
    pub release_identity: Option<String>,
    pub game_region: Option<String>,
    pub game_revision: Option<String>,
    pub emulator: String,
    pub profile_id: Option<String>,
    pub requirement: FirmwareRequirementDescriptor,
    pub state: GameFirmwareState,
    pub evidence: Vec<FirmwareEvidenceSummary>,
    pub selected_option: Option<FirmwareOption>,
    pub freshness: FirmwareFreshness,
}

impl GameFirmwareReadiness {
    pub fn project(input: GameFirmwareReadinessInput<'_>) -> Self {
        let (emulator, profile_id) = target_identity(input.candidate);
        let selected_option = input
            .selected_option_id
            .as_deref()
            .and_then(|id| {
                input
                    .requirement
                    .options
                    .iter()
                    .find(|option| option.id == id)
            })
            .cloned();

        let state = project_state(
            input.candidate.firmware,
            &input.requirement,
            &input.evidence,
            selected_option.as_ref(),
            input.freshness.state,
        );

        Self {
            game_id: input.game_id,
            platform: input.platform,
            release_identity: input.release_identity,
            game_region: input.game_region,
            game_revision: input.game_revision,
            emulator,
            profile_id,
            requirement: input.requirement,
            state,
            evidence: input.evidence,
            selected_option,
            freshness: input.freshness,
        }
    }
}

fn target_identity(candidate: &LaunchCandidate) -> (String, Option<String>) {
    match &candidate.target {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => ((*adapter_id).to_string(), Some(profile_id.clone())),
        LaunchTarget::RetroArchCore {
            profile, core_stem, ..
        } => (
            format!("retroarch:{core_stem}"),
            Some(retroarch_profile_id(profile)),
        ),
    }
}

/// A stable, human-readable profile id (for example `native:user`), instead of
/// a `Debug` rendering that could change with the type.
fn retroarch_profile_id(profile: &ProfileRef) -> String {
    let kind = match profile.profile_kind {
        ProfileKind::Native => "native",
        ProfileKind::AppImage => "appimage",
        ProfileKind::Flatpak => "flatpak",
    };
    let scope = match profile.scope {
        ProfileScope::User => "user",
        ProfileScope::System => "system",
    };
    format!("{kind}:{scope}")
}

fn project_state(
    candidate_firmware: FirmwareReadiness,
    requirement: &FirmwareRequirementDescriptor,
    evidence: &[FirmwareEvidenceSummary],
    selected_option: Option<&FirmwareOption>,
    freshness: FirmwareFreshnessState,
) -> GameFirmwareState {
    if freshness == FirmwareFreshnessState::Stale {
        return GameFirmwareState::Stale;
    }
    if !requirement.required {
        // The descriptor and the launch candidate must agree that nothing is
        // needed; a candidate that reports firmware as present, missing or
        // verified contradicts "not required", so say so rather than guess.
        return match candidate_firmware {
            FirmwareReadiness::NotRequired | FirmwareReadiness::Unknown => {
                GameFirmwareState::NotRequired
            }
            _ => GameFirmwareState::Unknown,
        };
    }
    if requirement.emulator_managed {
        return GameFirmwareState::EmulatorManaged;
    }

    // With an explicit choice, only evidence about that option (or evidence not
    // tied to any option) counts; a bad unrelated file must not mask a good
    // selected one.
    let relevant: Vec<&FirmwareEvidenceSummary> = evidence
        .iter()
        .filter(|item| match selected_option {
            Some(option) => item.option_id.as_deref().is_none_or(|id| id == option.id),
            None => true,
        })
        .collect();

    for (status, state) in [
        (
            FirmwareEvidenceStatus::RegionMismatchProven,
            GameFirmwareState::WrongRegion,
        ),
        (
            FirmwareEvidenceStatus::VersionMismatchProven,
            GameFirmwareState::WrongVersion,
        ),
        (
            FirmwareEvidenceStatus::HashMismatch,
            GameFirmwareState::HashMismatch,
        ),
    ] {
        if relevant.iter().any(|item| item.status == status) {
            return state;
        }
    }

    let accepted: Vec<&FirmwareEvidenceSummary> = relevant
        .iter()
        .copied()
        .filter(|item| item.status == FirmwareEvidenceStatus::Accepted)
        .collect();
    let verified: Vec<&FirmwareEvidenceSummary> = accepted
        .iter()
        .copied()
        .filter(|item| item.strength.is_content_verified())
        .collect();

    if selected_option.is_none() {
        let mut distinct = verified
            .iter()
            .filter_map(|item| item.option_id.as_deref())
            .collect::<Vec<_>>();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() > 1 {
            return GameFirmwareState::MultipleValidOptions;
        }
    }

    // Accepted firmware where the launch plan itself says it is missing is a
    // contradiction between two sources, not something to resolve by guessing.
    if candidate_firmware == FirmwareReadiness::Missing && !accepted.is_empty() {
        return GameFirmwareState::Unknown;
    }

    if !verified.is_empty() || candidate_firmware == FirmwareReadiness::Verified {
        // No Ready answer without proof that the inputs are still current.
        return if freshness == FirmwareFreshnessState::Current {
            GameFirmwareState::Ready
        } else {
            GameFirmwareState::Unknown
        };
    }

    match candidate_firmware {
        FirmwareReadiness::Missing => GameFirmwareState::Missing,
        FirmwareReadiness::PresentUnverified => GameFirmwareState::PresentUnverified,
        FirmwareReadiness::Unknown if !accepted.is_empty() => GameFirmwareState::PresentUnverified,
        // `Verified` returned above; kept here so the match stays exhaustive.
        FirmwareReadiness::Unknown
        | FirmwareReadiness::NotRequired
        | FirmwareReadiness::Verified => GameFirmwareState::Unknown,
    }
}

#[cfg(test)]
mod tests;
