//! Turns the existing local cheat-pack preview into the launch planner's
//! candidates for one game.
//!
//! Nothing here re-assesses a cheat. The pack preview already parsed each
//! `.cht` entry, assessed its applicability against the game's evidence,
//! fingerprinted its source file and reconciled duplicates; this module only
//! decides which of those observations may *become launchable candidates* and
//! says plainly why the rest cannot.
//!
//! A cheat becomes a candidate only if it is a native RetroArch `.cht` entry
//! that parsed cleanly, carries a usable description and code, has its source
//! file fingerprinted (so the launch can detect a change), and its
//! applicability is not a hard refusal. Anything else is listed in
//! `excluded` with the reason, so the chooser can show it disabled.

use std::path::PathBuf;

use super::cheat_launch_plan::{
    ApplicabilityVerdict, CheatCandidate, CheatLaunchFormat, CheatVariant, applicability_verdict,
};
use crate::patch_manager::{
    CheatApplicabilityState, CheatPackAction, CheatPackFileState, CheatPackPreview,
    CheatSourceReference, UserCheatFormat,
};

/// Why a cheat cannot be offered for launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateExclusion {
    /// Not a RetroArch `.cht` file (other emulators' formats are not launched here).
    NotARetroArchCht,
    /// The file or the entry did not parse into a usable description and code.
    Malformed,
    /// The source file has no recorded fingerprint, so a later change could not be detected.
    NoSourceFingerprint,
    /// The applicability assessment refuses it for this game.
    NotApplicable(CheatApplicabilityState),
}

impl CandidateExclusion {
    /// Short plain-language reason for a disabled entry.
    #[must_use]
    pub fn explanation(&self) -> String {
        match self {
            Self::NotARetroArchCht => "not a RetroArch cheat file".into(),
            Self::Malformed => "the cheat could not be read cleanly".into(),
            Self::NoSourceFingerprint => "its file could not be fingerprinted".into(),
            Self::NotApplicable(state) => format!("not applicable to this game ({state:?})"),
        }
    }
}

/// A cheat that is shown but cannot be launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedCheat {
    pub logical_key: String,
    pub title: Option<String>,
    pub source: Option<PathBuf>,
    pub reason: CandidateExclusion,
}

#[derive(Debug, Clone, Default)]
pub struct RetroArchLaunchCandidates {
    /// Launchable, still subject to the planner's explicit-selection rules.
    pub candidates: Vec<CheatCandidate>,
    /// Shown but unavailable.
    pub excluded: Vec<ExcludedCheat>,
}

/// Candidates for `game_key` (a key from `preview.games`).
#[must_use]
pub fn retroarch_launch_candidates(
    preview: &CheatPackPreview,
    game_key: &str,
) -> RetroArchLaunchCandidates {
    let mut out = RetroArchLaunchCandidates::default();
    for logical in preview
        .logical_cheats
        .iter()
        .filter(|logical| logical.game_key == game_key)
    {
        let mut variants = Vec::new();
        let mut first_exclusion: Option<ExcludedCheat> = None;
        let mut title: Option<String> = None;
        for &index in &logical.observation_indices {
            let Some(observation) = preview.observations.get(index) else {
                continue;
            };
            let Some(file) = preview.files.get(observation.file_index) else {
                continue;
            };
            // The preview records paths relative to its root; the launch
            // re-hashes the source by the path it is given, so it must be the
            // real one, and it must stay inside the root.
            let escapes = file.path.is_absolute()
                || file
                    .path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir));
            let full_path = preview.root.join(&file.path);
            let entry_title = observation
                .native_cht
                .as_ref()
                .and_then(|entry| entry.description.clone());
            title = title.or(entry_title.clone());
            let exclude = |reason: CandidateExclusion| ExcludedCheat {
                logical_key: logical.key.clone(),
                title: entry_title.clone(),
                source: Some(full_path.clone()),
                reason,
            };
            if escapes {
                first_exclusion.get_or_insert(exclude(CandidateExclusion::Malformed));
                continue;
            }
            if file.format != Some(UserCheatFormat::RetroarchCht) {
                first_exclusion.get_or_insert(exclude(CandidateExclusion::NotARetroArchCht));
                continue;
            }
            let usable_entry = observation.native_cht.as_ref().filter(|entry| {
                entry.warnings.is_empty()
                    && entry
                        .description
                        .as_deref()
                        .is_some_and(|d| !d.trim().is_empty())
                    && entry.code.as_deref().is_some_and(|c| !c.trim().is_empty())
            });
            let (Some(entry), Some(report)) = (usable_entry, observation.assessment.as_ref())
            else {
                first_exclusion.get_or_insert(exclude(CandidateExclusion::Malformed));
                continue;
            };
            if file.state != CheatPackFileState::Accepted
                || matches!(
                    observation.action,
                    CheatPackAction::WouldRejectMalformed | CheatPackAction::WouldRejectUnsupported
                )
            {
                first_exclusion.get_or_insert(exclude(CandidateExclusion::Malformed));
                continue;
            }
            let fingerprint = file
                .provenance
                .as_ref()
                .map(|p| p.source_sha256.clone())
                .filter(|hash| !hash.is_empty());
            let Some(fingerprint) = fingerprint else {
                first_exclusion.get_or_insert(exclude(CandidateExclusion::NoSourceFingerprint));
                continue;
            };
            if applicability_verdict(report) == ApplicabilityVerdict::Blocked {
                first_exclusion
                    .get_or_insert(exclude(CandidateExclusion::NotApplicable(report.state)));
                continue;
            }
            variants.push(CheatVariant {
                variant_id: format!(
                    "{}:{}",
                    &fingerprint[..fingerprint.len().min(16)],
                    entry.index
                ),
                source: CheatSourceReference {
                    provider: "local-cht".into(),
                    source_id: file
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    source_path: Some(full_path.clone()),
                    source_sha256: Some(fingerprint),
                    entry_index: Some(entry.index),
                },
                format: CheatLaunchFormat::RetroArchCht,
                applicability: report.clone(),
                entry: Some(entry.clone()),
            });
        }
        if variants.is_empty() {
            if let Some(excluded) = first_exclusion {
                out.excluded.push(excluded);
            }
            continue;
        }
        let unresolved_conflict = logical
            .reconciliation
            .as_ref()
            .is_some_and(|result| result.groups.iter().any(CheatCandidate::requires_choice));
        out.candidates.push(CheatCandidate {
            logical_id: logical.key.clone(),
            title: title.unwrap_or_else(|| logical.key.clone()),
            variants,
            unresolved_conflict,
        });
    }
    out
}

#[cfg(test)]
mod tests;
