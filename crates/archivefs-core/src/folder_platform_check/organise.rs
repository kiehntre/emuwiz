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
};
use crate::platform::identity::{
    PlatformIdentityConfidence, PlatformIdentityEvidence, PlatformIdentityResolution,
    PlatformIdentitySource,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub mod folder_move;

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

/// A file that moves with its set's lead (a CUE/GDI track, an M3U disc).
#[derive(Debug, Clone)]
pub struct SetMember {
    pub path: PathBuf,
    pub destination: PathBuf,
    /// Captured at preview; apply refuses if the file changed since.
    pub identity: Option<crate::dat::rename_apply::model::ObjectIdentity>,
    /// The member's own library binding, when it has a library row.
    pub binding: Option<serde_json::Value>,
}

/// One source folder's plan plus the corrections sealed into it.
#[derive(Debug, Clone)]
pub struct RootPlan {
    pub root: PathBuf,
    pub plan: OrganisationPlan,
    pub corrections: BTreeMap<PathBuf, PlatformCorrection>,
    /// Plain reason for every finding that was not offered for a move.
    pub left_alone: Vec<(PathBuf, String)>,
    /// Set leads -> the files that move with them, as one unit.
    pub sets: BTreeMap<PathBuf, Vec<SetMember>>,
    /// Picked games the library has no row for.
    pub uncatalogued: BTreeSet<PathBuf>,
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
/// One root's planning inputs: candidates, platform corrections, and the
/// games left alone with their reasons.
type RootParts = (
    Vec<OrganisationCandidate>,
    BTreeMap<PathBuf, PlatformCorrection>,
    Vec<(PathBuf, String)>,
);

pub fn plan_organisation_with_choices(
    findings: &[&Finding],
    chosen: &[ChosenGame],
    unknown: &[UnknownGame],
    roots: &[PathBuf],
    generation: u64,
) -> Vec<RootPlan> {
    let mut by_root: BTreeMap<PathBuf, RootParts> = BTreeMap::new();
    let mut orphans = Vec::new();
    // Package boundaries first: a set moves whole, a member never alone, and
    // anything that cannot be bounded stays put with the reason.
    let mut resolver = crate::game_package::PackageResolver::new();
    let all_paths: Vec<&Path> = findings
        .iter()
        .map(|f| f.path.as_path())
        .chain(chosen.iter().map(|g| g.path.as_path()))
        .chain(unknown.iter().map(|g| g.path.as_path()))
        .collect();
    let mut packages: BTreeMap<PathBuf, Result<crate::game_package::GamePackage, String>> =
        BTreeMap::new();
    for path in &all_paths {
        packages.insert(path.to_path_buf(), resolver.package_of(path));
    }
    let members_of_leads: BTreeSet<PathBuf> = packages
        .values()
        .flatten()
        .flat_map(|p| p.paths().into_iter().skip(1))
        .collect();
    let mut refused: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    let mut usable = |path: &Path| -> bool {
        match packages.get(path) {
            Some(Ok(p)) if p.shape == crate::game_package::PackageShape::GameFolder => {
                if let Some(root) = root_of(path, roots) {
                    refused.push((
                        root.clone(),
                        path.to_path_buf(),
                        "This game is a folder; it is offered under Game folders.".into(),
                    ));
                }
                false
            }
            Some(Ok(_)) => true,
            // Moves with a set that is itself being organised.
            Some(Err(_)) if members_of_leads.contains(path) => false,
            Some(Err(why)) => {
                if let Some(root) = root_of(path, roots) {
                    refused.push((
                        root.clone(),
                        path.to_path_buf(),
                        format!("Not moved: {why}."),
                    ));
                }
                false
            }
            None => false,
        }
    };
    let findings: Vec<&Finding> = findings
        .iter()
        .copied()
        .filter(|f| usable(&f.path))
        .collect();
    let chosen: Vec<&ChosenGame> = chosen.iter().filter(|g| usable(&g.path)).collect();
    let unknown: Vec<&UnknownGame> = unknown.iter().filter(|g| usable(&g.path)).collect();
    for finding in &findings {
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
    for game in &chosen {
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
    for game in &unknown {
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
    for (root, path, why) in refused {
        by_root.entry(root).or_default().2.push((path, why));
    }
    // Chosen games without an archive id were picked from disk.
    let picked_files: BTreeSet<PathBuf> = chosen
        .iter()
        .filter(|g| g.archive_id == 0)
        .map(|g| g.path.clone())
        .collect();
    let unknown_folder = Path::new(UNKNOWN_FOLDER);
    let mut plans: Vec<RootPlan> = by_root
        .into_iter()
        .map(|(root, (candidates, corrections, left_alone))| {
            let mut plan = build_organisation_plan(&OrganisationPlanRequest {
                master_root: &root,
                mode: OrganisationMode::MoveRealFile,
                content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
                candidates: &candidates,
                generation,
                unknown_folder: unknown_roots.contains(&root).then_some(unknown_folder),
            });
            let sets = attach_sets(&mut plan, &packages);
            let uncatalogued = picked_files
                .iter()
                .filter(|p| p.starts_with(&root))
                .cloned()
                .collect();
            RootPlan {
                root,
                plan,
                corrections,
                left_alone,
                sets,
                uncatalogued,
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

/// Gives every Suggested set lead its members' destinations (beside the
/// lead's). A member destination that is taken, or claimed twice, turns the
/// whole set into a collision: nothing in it moves.
fn attach_sets(
    plan: &mut OrganisationPlan,
    packages: &BTreeMap<PathBuf, Result<crate::game_package::GamePackage, String>>,
) -> BTreeMap<PathBuf, Vec<SetMember>> {
    use crate::dat::rom_organisation::OrganisationStatus;
    let mut sets = BTreeMap::new();
    let mut claimed: BTreeSet<PathBuf> = plan
        .suggested()
        .map(|e| e.destination_path.clone())
        .collect();
    for entry in &mut plan.entries {
        let Some(Ok(package)) = packages.get(&entry.source_path) else {
            continue;
        };
        let crate::game_package::PackageShape::FileSet { members } = &package.shape else {
            continue;
        };
        if entry.status != OrganisationStatus::Suggested {
            continue;
        }
        let Some(folder) = entry.destination_path.parent() else {
            continue;
        };
        let mut list = Vec::new();
        let mut clash = None;
        for member in members {
            let Some(file) = member.file_name() else {
                clash = Some(format!("{} has no file name", member.display()));
                break;
            };
            let destination = folder.join(file);
            if std::fs::symlink_metadata(&destination).is_ok()
                || !claimed.insert(destination.clone())
            {
                clash = Some(format!(
                    "{} already exists at the destination; nothing is ever overwritten",
                    file.to_string_lossy()
                ));
                break;
            }
            list.push(SetMember {
                path: member.clone(),
                destination,
                identity: None,
                binding: None,
            });
        }
        match clash {
            Some(why) => {
                entry.status = OrganisationStatus::Conflict;
                entry.reason = Some(format!("The set cannot move complete: {why}."));
            }
            None => {
                sets.insert(entry.source_path.clone(), list);
            }
        }
    }
    sets
}

fn member_entry(
    lead: &Path,
    member: &SetMember,
    identity: crate::dat::rename_apply::model::ObjectIdentity,
) -> crate::dat::rename_apply::model::TransactionEntry {
    use crate::dat::rename_apply::model::{EntryState, TransactionEntry, TransactionOperation};
    let mut unknown = BTreeMap::from([(
        crate::database::organisation_paths::COMPANION_KEY.to_string(),
        serde_json::json!({"lead": lead.to_string_lossy()}),
    )]);
    if let Some(binding) = &member.binding {
        unknown.insert("organisation_catalogue_v1".into(), binding.clone());
    }
    let base = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    TransactionEntry {
        source_path: member.path.clone(),
        destination_path: member.destination.clone(),
        original_basename: base(&member.path),
        proposed_basename: base(&member.destination),
        identity,
        operation: TransactionOperation::RenameMove,
        preflight_passed: false,
        preflight_failures: Vec::new(),
        state: EntryState::Planned,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown,
    }
}

/// A picked game the library has no row for moves unbound; one that does
/// have a row (it was catalogued after all) keeps its binding.
fn mark_uncatalogued(
    entry: &mut crate::dat::rename_apply::model::TransactionEntry,
    uncatalogued: &BTreeSet<PathBuf>,
    database: &crate::Database,
) -> Result<(), String> {
    if uncatalogued.contains(&entry.source_path)
        && database
            .find_archive_id_by_absolute_path(&entry.source_path)
            .map_err(|e| e.to_string())?
            .is_none()
    {
        entry.unknown.insert(
            crate::database::organisation_paths::UNCATALOGUED_KEY.into(),
            serde_json::json!(true),
        );
    }
    Ok(())
}

/// Seals the catalogue bindings (paths + platform corrections) into the plan,
/// and captures every set member's identity and binding.
/// A failure means the preview is stale or unsafe and must be rebuilt.
pub fn prepare(root_plan: &mut RootPlan, database: &crate::Database) -> Result<(), String> {
    let plan = &mut root_plan.plan;
    if plan.suggested().next().is_none() {
        return Ok(());
    }
    let approved: BTreeSet<String> = plan
        .suggested()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect();
    let mut transaction = build_organisation_transaction(plan, &approved, plan.generation)?;
    for entry in &mut transaction.entries {
        if let Some(correction) = root_plan.corrections.get(&entry.source_path) {
            entry.unknown.insert(
                crate::database::organisation_paths::PLATFORM_CHANGE_KEY.into(),
                serde_json::json!({"platform": correction.platform, "source": correction.source}),
            );
        }
        mark_uncatalogued(entry, &root_plan.uncatalogued, database)?;
    }
    for (lead, members) in &mut root_plan.sets {
        for member in members.iter_mut() {
            let identity = crate::dat::rename_apply::identity::capture_identity(&member.path)
                .map_err(|e| format!("{}: {e}", member.path.display()))?;
            member.identity = Some(identity.clone());
            transaction
                .entries
                .push(member_entry(lead, member, identity));
        }
    }
    crate::database::organisation_paths::bind(&mut transaction, database)?;
    for entry in &mut plan.entries {
        if let Some(bound) = transaction
            .entries
            .iter()
            .find(|e| e.source_path == entry.source_path)
        {
            entry.catalogue_binding = bound.unknown.get("organisation_catalogue_v1").cloned();
        }
    }
    for members in root_plan.sets.values_mut() {
        for member in members.iter_mut() {
            member.binding = transaction
                .entries
                .iter()
                .find(|e| e.source_path == member.path)
                .and_then(|e| e.unknown.get("organisation_catalogue_v1").cloned());
        }
    }
    Ok(())
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
        Option<Box<crate::dat::rename_apply::RenameTransaction>>,
    ),
> {
    let plan = &root_plan.plan;
    let mut transaction = build_organisation_transaction(plan, approved, plan.generation)
        .map_err(|e| (ApplyError::Journal(e), None))?;
    for entry in &mut transaction.entries {
        if !entry.unknown.contains_key("organisation_catalogue_v1")
            && root_plan.uncatalogued.contains(&entry.source_path)
        {
            entry.unknown.insert(
                crate::database::organisation_paths::UNCATALOGUED_KEY.into(),
                serde_json::json!(true),
            );
        }
    }
    for (lead, members) in &root_plan.sets {
        if !approved.contains(&*lead.to_string_lossy()) {
            continue;
        }
        for member in members {
            let identity = member.identity.clone().ok_or_else(|| {
                (
                    ApplyError::Journal("the set preview was not sealed; preview again".into()),
                    None,
                )
            })?;
            transaction
                .entries
                .push(member_entry(lead, member, identity));
        }
    }
    let has_sets = !root_plan.sets.is_empty();
    // Approving a set approves exactly the files it names.
    let mut approved = approved.clone();
    for (lead, members) in &root_plan.sets {
        if approved.contains(&*lead.to_string_lossy()) {
            approved.extend(
                members
                    .iter()
                    .map(|m| m.path.to_string_lossy().into_owned()),
            );
        }
    }
    let approved = &approved;
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
    .map_err(|error| {
        // A set moves whole or not at all: a failure part-way puts back what
        // this run already moved.
        if has_sets && transaction.applied_count() > 0 {
            let _ = crate::dat::rom_organisation::rollback_organisation_transaction(
                &mut transaction,
                journal_dir,
                &AtomicBool::new(false),
                &plan.master_root,
            );
        }
        (error, Some(Box::new(transaction)))
    })
}

/// A game folder offered for a move: found by its layout (a PS3 disc
/// folder) or picked by the person with a system they chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderCandidate {
    pub path: PathBuf,
    pub platform: String,
    pub evidence: String,
    /// The person chose this folder's system (not detected).
    pub chosen: bool,
}

/// Read-only previews for game folders.
pub fn preview_folders(
    database: &crate::Database,
    roots: &[PathBuf],
    candidates: &[FolderCandidate],
) -> Vec<(FolderCandidate, folder_move::FolderPreview)> {
    candidates
        .iter()
        .map(|c| {
            (
                c.clone(),
                folder_move::preview(database, roots, &c.path, &c.platform, &c.evidence),
            )
        })
        .collect()
}

/// PS3 disc folders found by layout in the library roots (read-only).
pub fn found_game_folders(roots: &[PathBuf]) -> Vec<FolderCandidate> {
    let start = std::time::Instant::now();
    let (found, cost) = crate::game_package::find_game_folders_measured(roots, 3);
    log::info!(
        "organise: PS3 folder search listed {} folders, made {} layout checks, found {} in {} ms",
        cost.directories_read,
        cost.layout_checks,
        found.len(),
        start.elapsed().as_millis()
    );
    found
        .into_iter()
        .map(|f| FolderCandidate {
            path: f.path,
            platform: f.platform,
            evidence: f.evidence,
            chosen: false,
        })
        .collect()
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
