//! "Organise my games": turns platform-check results into organisation plans
//! and applies them through the shared journalled organiser.
//!
//! Only [`Tier::is_movable`] findings move to a platform folder, and (when the
//! person asks) checked games with no platform evidence move to
//! [`UNKNOWN_FOLDER`]. Ambiguous and Probable results never move. Each plan is
//! per registered source folder, so a move never leaves its source root (the
//! organiser refuses cross-filesystem moves anyway). The destination is the
//! source root's existing folder for the platform (`nes`, `snes`, ...) or the
//! canonical registry folder, created on apply. Platform corrections commit in
//! the same SQLite transaction as each move; History/Undo reverses both.

use super::{Finding, Tier};
use crate::dat::rename_apply::{ApplyError, ApplyOutcome};
use crate::dat::rom_organisation::{
    OrganisationCandidate, OrganisationMode, OrganisationPlan, OrganisationPlanRequest,
    PlatformCorrection, build_organisation_plan, build_organisation_transaction,
    prepare_organisation_catalogue_with_platforms,
};
use crate::platform::identity::{
    PlatformIdentityConfidence, PlatformIdentityEvidence, PlatformIdentityResolution,
    PlatformIdentitySource,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Literal destination for checked games with no identifiable platform.
pub const UNKNOWN_FOLDER: &str = "Unnamed/Unknown";

/// A checked game with no platform evidence at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownGame {
    pub archive_id: i64,
    pub path: PathBuf,
}

/// A game the person assigned to a system themselves, sitting outside that
/// system's folder. It moves with its manual choice unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenGame {
    pub archive_id: i64,
    pub path: PathBuf,
    pub platform: String,
}

/// One source folder's plan plus the corrections sealed into it.
#[derive(Debug, Clone)]
pub struct RootPlan {
    pub root: PathBuf,
    pub plan: OrganisationPlan,
    pub corrections: BTreeMap<PathBuf, PlatformCorrection>,
    /// Plain reason for every finding that was not offered for a move.
    pub left_alone: Vec<(PathBuf, String)>,
}

fn correction_for(finding: &Finding) -> Option<PlatformCorrection> {
    match finding.tier {
        Tier::Confirmed => Some(PlatformCorrection {
            platform: finding.detected.clone(),
            source: crate::database::VERIFIED_DAT_PLATFORM_SOURCE,
        }),
        Tier::PlatformConfirmed => Some(PlatformCorrection {
            platform: finding.detected.clone(),
            source: "header_identity",
        }),
        Tier::Probable | Tier::Ambiguous => None,
    }
}

fn resolution(
    correction: &PlatformCorrection,
    generation: u64,
    finding: &Finding,
) -> PlatformIdentityResolution {
    let (source, confidence) = if correction.source == "header_identity" {
        (
            PlatformIdentitySource::ExistingStrongIdentity,
            PlatformIdentityConfidence::Strong,
        )
    } else {
        (
            PlatformIdentitySource::VerifiedDat,
            PlatformIdentityConfidence::Verified,
        )
    };
    PlatformIdentityResolution::Resolved {
        generation,
        platform: correction.platform.clone(),
        display_name: crate::platform::display_name_for(&correction.platform).to_string(),
        confidence,
        evidence: vec![PlatformIdentityEvidence {
            platform: correction.platform.clone(),
            source,
            confidence,
            generation,
            detail: finding.lines.join(" "),
        }],
    }
}

/// The registered source root holding `path` (longest match).
pub fn root_of<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

fn why_left(tier: Tier) -> &'static str {
    match tier {
        Tier::Probable => {
            "Only a structural heuristic points at another system; nothing was moved. Use Choose system if you know it."
        }
        Tier::Ambiguous => {
            "The evidence points at more than one system; nothing was moved. Review it and choose the system."
        }
        _ => "",
    }
}

/// Builds the read-only plans, one per source root. Never touches the disk
/// beyond reading object kinds and destination occupancy.
pub fn plan_organisation(
    findings: &[&Finding],
    unknown: &[UnknownGame],
    roots: &[PathBuf],
    generation: u64,
) -> Vec<RootPlan> {
    plan_organisation_with_choices(findings, &[], unknown, roots, generation)
}

/// [`plan_organisation`] plus games whose system the person chose.
pub fn plan_organisation_with_choices(
    findings: &[&Finding],
    chosen: &[ChosenGame],
    unknown: &[UnknownGame],
    roots: &[PathBuf],
    generation: u64,
) -> Vec<RootPlan> {
    let mut by_root: BTreeMap<
        PathBuf,
        (
            Vec<OrganisationCandidate>,
            BTreeMap<PathBuf, PlatformCorrection>,
            Vec<(PathBuf, String)>,
        ),
    > = BTreeMap::new();
    let mut orphans = Vec::new();
    for finding in findings {
        let Some(root) = root_of(&finding.path, roots) else {
            orphans.push(finding.path.clone());
            continue;
        };
        let slot = by_root.entry(root.clone()).or_default();
        match correction_for(finding) {
            Some(correction) => {
                slot.0.push(OrganisationCandidate {
                    source_path: finding.path.clone(),
                    resolution: resolution(&correction, generation, finding),
                    canonical_name: None,
                    content_classification: None,
                    original_metadata: Default::default(),
                });
                slot.1.insert(finding.path.clone(), correction);
            }
            None => slot
                .2
                .push((finding.path.clone(), why_left(finding.tier).to_string())),
        }
    }
    for game in chosen {
        let Some(root) = root_of(&game.path, roots) else {
            orphans.push(game.path.clone());
            continue;
        };
        by_root
            .entry(root.clone())
            .or_default()
            .0
            .push(OrganisationCandidate {
                source_path: game.path.clone(),
                resolution: PlatformIdentityResolution::Resolved {
                    generation,
                    platform: game.platform.clone(),
                    display_name: crate::platform::display_name_for(&game.platform).to_string(),
                    confidence: PlatformIdentityConfidence::High,
                    evidence: vec![PlatformIdentityEvidence {
                        platform: game.platform.clone(),
                        source: PlatformIdentitySource::Manual,
                        confidence: PlatformIdentityConfidence::High,
                        generation,
                        detail: "your system choice".into(),
                    }],
                },
                canonical_name: None,
                content_classification: None,
                original_metadata: Default::default(),
            });
    }
    let mut unknown_roots = BTreeSet::new();
    for game in unknown {
        let Some(root) = root_of(&game.path, roots) else {
            orphans.push(game.path.clone());
            continue;
        };
        unknown_roots.insert(root.clone());
        by_root
            .entry(root.clone())
            .or_default()
            .0
            .push(OrganisationCandidate {
                source_path: game.path.clone(),
                resolution: PlatformIdentityResolution::Unknown { generation },
                canonical_name: None,
                content_classification: None,
                original_metadata: Default::default(),
            });
    }
    let unknown_folder = Path::new(UNKNOWN_FOLDER);
    let mut plans: Vec<RootPlan> = by_root
        .into_iter()
        .map(|(root, (candidates, corrections, left_alone))| {
            let plan = build_organisation_plan(&OrganisationPlanRequest {
                master_root: &root,
                mode: OrganisationMode::MoveRealFile,
                content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
                candidates: &candidates,
                generation,
                unknown_folder: unknown_roots.contains(&root).then_some(unknown_folder),
            });
            RootPlan {
                root,
                plan,
                corrections,
                left_alone,
            }
        })
        .collect();
    if !orphans.is_empty() {
        let reason = "This file is not inside a registered source folder, so EmuWiz cannot keep its records; nothing was moved.";
        if let Some(first) = plans.first_mut() {
            first
                .left_alone
                .extend(orphans.into_iter().map(|p| (p, reason.to_string())));
        }
    }
    plans
}

/// Seals the catalogue bindings (paths + platform corrections) into the plan.
/// A failure means the preview is stale or unsafe and must be rebuilt.
pub fn prepare(root_plan: &mut RootPlan, database: &crate::Database) -> Result<(), String> {
    prepare_organisation_catalogue_with_platforms(
        &mut root_plan.plan,
        database,
        &root_plan.corrections,
    )
}

/// Applies the approved entries with the shared journalled executor. The
/// journal lands in `journal_dir` (the History folder), so Undo is the
/// ordinary organisation Undo.
pub fn apply(
    root_plan: &RootPlan,
    approved: &BTreeSet<String>,
    database: &crate::Database,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<
    ApplyOutcome,
    (
        ApplyError,
        Option<crate::dat::rename_apply::RenameTransaction>,
    ),
> {
    let plan = &root_plan.plan;
    let mut transaction = build_organisation_transaction(plan, approved, plan.generation)
        .map_err(|e| (ApplyError::Journal(e), None))?;
    std::fs::create_dir_all(journal_dir).map_err(|e| (ApplyError::Journal(e.to_string()), None))?;
    let trusted =
        crate::safe_read::TrustedRoots::from_paths([
            std::fs::canonicalize(&root_plan.root).unwrap_or_else(|_| root_plan.root.clone())
        ]);
    crate::dat::rom_organisation::apply_organisation_transaction(
        &mut transaction,
        approved,
        plan.generation,
        trusted,
        journal_dir,
        cancel,
        plan.mode,
        &plan.master_root,
        database,
    )
    .map_err(|e| (e, Some(transaction)))
}

/// Counts for the result screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub to_move: usize,
    pub to_unknown: usize,
    pub already_correct: usize,
    pub collisions: usize,
    pub blocked: usize,
    pub left_alone: usize,
}

pub fn summarise(plans: &[RootPlan]) -> Summary {
    use crate::dat::rom_organisation::OrganisationStatus as S;
    let mut s = Summary::default();
    for p in plans {
        s.left_alone += p.left_alone.len();
        for e in &p.plan.entries {
            match e.status {
                S::Suggested if e.platform.is_none() => s.to_unknown += 1,
                S::Suggested => s.to_move += 1,
                S::AlreadyOrganised => s.already_correct += 1,
                S::Conflict => s.collisions += 1,
                S::Blocked | S::Unsupported => s.blocked += 1,
            }
        }
    }
    s
}

#[cfg(test)]
mod tests;
