//! Checksum-backed MAME merged-set reconstruction.
//!
//! This is deliberately a new projection over the current MAME join/evidence
//! model.  It does not use the historical `mame_normalizer` plan, its repair
//! journal, or filename-based ownership rules.  A plan is pure data and is
//! safe to display before an explicit apply decision.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::dat::archive::ArchiveMemberSource;
use crate::safe_read::TrustedRoots;

use super::mame_arcade_join::{ArcadeJoinClass, ArcadeJoinEvidence};
use super::model::{DatRomEntry, ParsedDat};

/// Durable journal marker used by GUI-v2 and recovery history to distinguish
/// MAME reconstruction transactions from the other rename-based workflows.
pub const MAME_RECONSTRUCTION_WORKFLOW: &str = "mame_merged_reconstruction";
pub const MAX_RECONSTRUCTION_STAGED_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconstructionMemberRequirement {
    pub owner_set: String,
    pub member_name: String,
    pub size_bytes: Option<u64>,
    pub sha1: Option<String>,
    pub crc32: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceArchiveIdentity {
    pub size_bytes: u64,
    pub modified_unix_nanos: Option<u128>,
}

impl SourceArchiveIdentity {
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            size_bytes: metadata.len(),
            modified_unix_nanos: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_nanos()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconstructionMemberSource {
    /// Size and modification time of a packed source archive when the plan was
    /// made. Staging refuses a changed archive and asks for a new preview; the
    /// member's own checksum is still verified either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_identity: Option<SourceArchiveIdentity>,
    pub archive_path: PathBuf,
    pub member_path: PathBuf,
    pub current_name: String,
    pub target_name: String,
    pub observed_sha1: Option<String>,
    pub observed_crc32: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameMergedReconstructionPlan {
    pub dat_version: String,
    pub dat_sha256: String,
    pub parent: String,
    pub clones: Vec<String>,
    pub destination: PathBuf,
    pub required_members: Vec<ReconstructionMemberRequirement>,
    pub sources: Vec<ReconstructionMemberSource>,
    pub missing_members: Vec<String>,
    pub duplicate_candidates: Vec<String>,
    pub hash_mismatches: Vec<String>,
    pub unresolved_ownership: Vec<String>,
    pub collisions: Vec<String>,
    pub ready_to_apply: bool,
    pub reasons: Vec<String>,
}

impl MameMergedReconstructionPlan {
    pub fn blocked(&self) -> bool {
        !self.ready_to_apply
    }
}

/// Explicit publication action; replacement retains the exact original at the
/// transaction's staged source path for undo. No authority is inferred from a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconstructionPublicationAction {
    Create,
    ReplaceExisting {
        original: super::rename_apply::model::ObjectIdentity,
    },
    Unchanged {
        current: super::rename_apply::model::ObjectIdentity,
    },
}

/// A read-only, source/target-bound publication review over the canonical family
/// plan. Private fields prevent changing requirements after review. The ordinary
/// family planner and its ownership/duplicate/missing-member rules are unchanged.
#[derive(Debug)]
pub struct ReviewedReconstructionPublication {
    plan: MameMergedReconstructionPlan,
    action: ReconstructionPublicationAction,
    sources: Vec<(PathBuf, super::rename_apply::model::ObjectIdentity)>,
}

impl ReviewedReconstructionPublication {
    pub fn action(&self) -> &ReconstructionPublicationAction {
        &self.action
    }
    pub fn plan(&self) -> &MameMergedReconstructionPlan {
        &self.plan
    }

    fn revalidate(&self) -> Result<(), String> {
        use super::rename_apply::identity::{capture_identity, identity_matches};
        for (path, expected) in &self.sources {
            let actual = capture_identity(path).map_err(|e| e.to_string())?;
            if !identity_matches(expected, &actual) {
                return Err("stale reconstruction source; review again".into());
            }
        }
        match &self.action {
            ReconstructionPublicationAction::Create => {
                if std::fs::symlink_metadata(&self.plan.destination).is_ok() {
                    return Err("stale reconstruction destination; review again".into());
                }
            }
            ReconstructionPublicationAction::ReplaceExisting { original: expected }
            | ReconstructionPublicationAction::Unchanged { current: expected } => {
                let actual = capture_identity(&self.plan.destination).map_err(|e| e.to_string())?;
                if !identity_matches(expected, &actual) {
                    return Err("stale reconstruction target; review again".into());
                }
            }
        }
        Ok(())
    }

    /// Explicitly apply this reviewed plan. `None` is a verified no-op, without
    /// staging or journal writes. Otherwise return the canonical outcome/undo
    /// receipt; callers must inspect its transaction state before reporting success.
    pub fn apply(
        &self,
        staging_root: &Path,
        journal_dir: &Path,
    ) -> Result<Option<super::rename_apply::executor::ApplyOutcome>, String> {
        self.revalidate()?;
        if matches!(
            self.action,
            ReconstructionPublicationAction::Unchanged { .. }
        ) {
            return Ok(None);
        }
        let staged = stage_reconstruction_output(&self.plan, staging_root)?;
        self.revalidate()?;
        let operation = match &self.action {
            ReconstructionPublicationAction::ReplaceExisting { original } => {
                super::rename_apply::model::TransactionOperation::ReplaceExisting {
                    original_identity: original.clone(),
                    destination_root: self
                        .plan
                        .destination
                        .parent()
                        .ok_or("target parent missing")?
                        .to_owned(),
                }
            }
            _ => Default::default(),
        };
        publish_staged(&self.plan, &staged, staging_root, journal_dir, operation).map(Some)
    }
}

/// Review publication separately from family evidence, without changing DAT
/// authority. An incomplete target needs a matching current DAT join AND at
/// least one live checksum-proven parent-owned member. Entirely unrecognisable
/// targets are foreign, even when named `parent.zip`.
pub fn review_reconstruction_publication(
    plan: &MameMergedReconstructionPlan,
    target_evidence: Option<&ArcadeJoinEvidence>,
) -> Result<ReviewedReconstructionPublication, String> {
    use super::rename_apply::identity::capture_identity;
    use super::rename_apply::model::ObjectKind;
    if !super::rename_apply::preflight::is_safe_basename(&plan.parent) {
        return Err("unsafe reconstruction parent name".into());
    }
    let mut reviewed = plan.clone();
    let action = match std::fs::symlink_metadata(&plan.destination) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ReconstructionPublicationAction::Create
        }
        Err(e) => return Err(e.to_string()),
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("target is not a regular archive".into());
            }
            let original = capture_identity(&plan.destination).map_err(|e| e.to_string())?;
            let members = read_zip_evidence(&plan.destination)?;
            if verify_members(plan, &members).is_ok() {
                return Ok(ReviewedReconstructionPublication {
                    plan: reviewed,
                    action: ReconstructionPublicationAction::Unchanged { current: original },
                    sources: Vec::new(),
                });
            }
            let evidence = target_evidence
                .ok_or("existing target needs checksum-backed ownership evidence")?;
            if evidence.logical_set_name != plan.parent
                || evidence.dat_set_name.as_deref() != Some(&plan.parent)
                || evidence.dat_sha256 != plan.dat_sha256
                || evidence.dat_version != plan.dat_version
                || matches!(
                    evidence.class,
                    ArcadeJoinClass::Ambiguous | ArcadeJoinClass::NotFound
                )
                || !target_has_owned_member(plan, evidence, &members)
            {
                return Err("foreign or unproven existing target; replacement refused".into());
            }
            reviewed
                .collisions
                .retain(|path| path != &plan.destination.display().to_string());
            if reviewed.collisions.is_empty() {
                reviewed
                    .reasons
                    .retain(|reason| reason != "destination collision cannot be proven safe");
            }
            reviewed.ready_to_apply = reviewed.reasons.is_empty()
                && reviewed.collisions.is_empty()
                && reviewed.missing_members.is_empty()
                && reviewed.duplicate_candidates.is_empty()
                && reviewed.hash_mismatches.is_empty()
                && reviewed.unresolved_ownership.is_empty()
                && !reviewed.required_members.is_empty()
                && reviewed.sources.len() == reviewed.required_members.len();
            ReconstructionPublicationAction::ReplaceExisting { original }
        }
    };
    if reviewed.blocked() {
        return Err("reconstruction family plan remains blocked".into());
    }
    let paths: BTreeSet<_> = reviewed
        .sources
        .iter()
        .map(|source| {
            if source.archive_path.is_dir() {
                source.member_path.clone()
            } else {
                source.archive_path.clone()
            }
        })
        .collect();
    let mut sources = Vec::new();
    for path in paths {
        let identity = capture_identity(&path).map_err(|e| e.to_string())?;
        if identity.kind != ObjectKind::RegularFile {
            return Err("donor is not a regular file".into());
        }
        sources.push((path, identity));
    }
    let result = ReviewedReconstructionPublication {
        plan: reviewed,
        action,
        sources,
    };
    result.revalidate()?;
    Ok(result)
}

fn read_zip_evidence(
    path: &Path,
) -> Result<Vec<crate::dat::archive::ArchiveMemberEvidence>, String> {
    let cancel = AtomicBool::new(false);
    let trusted = TrustedRoots::from_paths(path.parent().into_iter());
    let mut source = crate::dat::archive::zip::ZipArchiveSource::open(
        path,
        &trusted,
        crate::dat::archive::limits::ArchiveLimits::default(),
        &cancel,
    )
    .map_err(|e| format!("ZIP inspection refused: {e:?}"))?;
    let mut budget = crate::dat::archive::ArchiveRunBudget::new(MAX_RECONSTRUCTION_STAGED_BYTES);
    let outcome = source.verify_all(&cancel, &mut budget);
    if outcome.completion != crate::dat::archive::ArchivePassCompletion::Complete {
        return Err("ZIP inspection incomplete or source changed".into());
    }
    Ok(outcome.members)
}

fn target_has_owned_member(
    plan: &MameMergedReconstructionPlan,
    evidence: &ArcadeJoinEvidence,
    members: &[crate::dat::archive::ArchiveMemberEvidence],
) -> bool {
    let required: BTreeMap<_, _> = plan
        .required_members
        .iter()
        .map(|r| (r.member_name.as_str(), r))
        .collect();
    let known: BTreeMap<_, _> = evidence
        .members
        .iter()
        .filter_map(|m| m.current_name.as_deref().map(|name| (name, m)))
        .collect();
    if known.len()
        != evidence
            .members
            .iter()
            .filter(|m| m.current_name.is_some())
            .count()
    {
        return false;
    }
    members.iter().any(|member| {
        let Some(known) = known.get(member.member_name_display.as_str()) else {
            return false;
        };
        let Some(required) = required.get(known.name.as_str()) else {
            return false;
        };
        required.owner_set == plan.parent
            && member_matches(required, member)
            && known.kind != super::mame_arcade_join::MemberEvidenceKind::Unknown
            && (known.observed_sha1.is_some() || known.observed_crc32.is_some())
            && member.hashes.as_ref().is_some_and(|hashes| {
                known
                    .observed_sha1
                    .as_ref()
                    .is_none_or(|h| h.eq_ignore_ascii_case(&hashes.sha1))
                    && known
                        .observed_crc32
                        .as_ref()
                        .is_none_or(|h| h.eq_ignore_ascii_case(&hashes.crc32))
            })
    })
}

fn member_matches(
    required: &ReconstructionMemberRequirement,
    member: &crate::dat::archive::ArchiveMemberEvidence,
) -> bool {
    member.is_hash_complete()
        && required.size_bytes == Some(member.logical_size)
        && (required.sha1.is_some() || required.crc32.is_some())
        && member.hashes.as_ref().is_some_and(|hashes| {
            required
                .sha1
                .as_ref()
                .is_none_or(|h| h.eq_ignore_ascii_case(&hashes.sha1))
                && required
                    .crc32
                    .as_ref()
                    .is_none_or(|h| h.eq_ignore_ascii_case(&hashes.crc32))
        })
}

fn verify_members(
    plan: &MameMergedReconstructionPlan,
    members: &[crate::dat::archive::ArchiveMemberEvidence],
) -> Result<(), String> {
    if members.len() != plan.required_members.len() || members.is_empty() {
        return Err("staged member inventory count mismatch".into());
    }
    let required: BTreeMap<_, _> = plan
        .required_members
        .iter()
        .map(|r| (r.member_name.as_bytes(), r))
        .collect();
    if required.len() != plan.required_members.len() {
        return Err("duplicate output requirement name".into());
    }
    let mut names = BTreeSet::new();
    for member in members {
        if !names.insert(&member.member_name_raw) {
            return Err("staged duplicate member name".into());
        }
        let required = required
            .get(member.member_name_raw.as_slice())
            .ok_or("staged unexpected member")?;
        if !member_matches(required, member) {
            return Err(format!(
                "staged SHA-1/CRC/size mismatch for {}",
                required.member_name
            ));
        }
    }
    Ok(())
}

/// The parent and clone set names [`build_merged_reconstruction_plan`] plans
/// for `requested_set`. A caller can restrict the persisted joins it loads to
/// these names: the plan ignores every other set's join anyway.
pub fn reconstruction_family_names(
    dat: &ParsedDat,
    requested_set: &str,
) -> Result<Vec<String>, String> {
    let selected = dat
        .games
        .iter()
        .find(|game| game.name == requested_set)
        .ok_or_else(|| {
            format!("MAME set is absent from the selected catalogue: {requested_set}")
        })?;
    let parent = selected
        .clone_of
        .as_deref()
        .unwrap_or(selected.name.as_str());
    Ok(dat
        .games
        .iter()
        .filter(|game| game.name == parent || game.clone_of.as_deref() == Some(parent))
        .map(|game| game.name.clone())
        .collect())
}

/// Build a deterministic plan from the current parsed MAME catalogue and
/// persisted join evidence. `joins` must contain the exact archive path paired
/// with each SHA-bound join. Directory names and member filenames are never
/// used as identity evidence.
pub fn build_merged_reconstruction_plan(
    root: &Path,
    dat: &ParsedDat,
    joins: &[(PathBuf, ArcadeJoinEvidence)],
    requested_set: &str,
    dat_sha256: &str,
) -> Result<MameMergedReconstructionPlan, String> {
    let selected = dat
        .games
        .iter()
        .find(|game| game.name == requested_set)
        .ok_or_else(|| {
            format!("MAME set is absent from the selected catalogue: {requested_set}")
        })?;
    let parent = selected
        .clone_of
        .as_deref()
        .unwrap_or(selected.name.as_str());
    let family = dat
        .games
        .iter()
        .filter(|game| game.name == parent || game.clone_of.as_deref() == Some(parent))
        .collect::<Vec<_>>();
    if family.iter().any(|game| game.name == parent) == false || family.len() < 2 {
        return Err(format!("parent/clone identity is incomplete for {parent}"));
    }
    let clones = family
        .iter()
        .filter(|game| game.name != parent)
        .map(|game| game.name.clone())
        .collect::<Vec<_>>();
    let mut required = Vec::new();
    let mut by_identity = BTreeSet::new();
    for game in &family {
        for rom in game.roms.iter().filter(|rom| !is_non_physical(rom)) {
            let key = rom_identity(rom).ok_or_else(|| {
                format!(
                    "required member has insufficient catalogue hash: {}",
                    rom.name
                )
            })?;
            if by_identity.insert(key) {
                required.push(ReconstructionMemberRequirement {
                    owner_set: game.name.clone(),
                    member_name: rom.name.clone(),
                    size_bytes: rom.size_bytes,
                    sha1: rom.sha1.as_ref().map(|v| v.to_ascii_lowercase()),
                    crc32: rom.crc32.as_ref().map(|v| v.to_ascii_lowercase()),
                });
            }
        }
    }
    required.sort_by(|a, b| a.member_name.cmp(&b.member_name));

    let mut candidates: BTreeMap<String, Vec<ReconstructionMemberSource>> = BTreeMap::new();
    for (archive_path, join) in joins {
        if !family.iter().any(|game| game.name == join.logical_set_name)
            || join.dat_set_name.as_deref() != Some(join.logical_set_name.as_str())
            || join.dat_sha256 != dat_sha256
            || join.dat_version != dat.source.version.clone().unwrap_or_default()
        {
            continue;
        }
        for member in &join.members {
            if member.kind == super::mame_arcade_join::MemberEvidenceKind::Unknown
                || member.current_name.is_none()
                || member.observed_sha1.is_none() && member.observed_crc32.is_none()
            {
                continue;
            }
            let Some(identity) = evidence_identity(
                member.observed_sha1.as_deref(),
                member.observed_crc32.as_deref(),
            ) else {
                continue;
            };
            let current_name = member.current_name.clone().unwrap();
            candidates
                .entry(identity)
                .or_default()
                .push(ReconstructionMemberSource {
                    archive_identity: (!archive_path.is_dir())
                        .then(|| SourceArchiveIdentity::of(archive_path))
                        .flatten(),
                    archive_path: archive_path.clone(),
                    member_path: if archive_path.is_dir() {
                        archive_path.join(&current_name)
                    } else {
                        PathBuf::from(&current_name)
                    },
                    current_name,
                    target_name: member.name.clone(),
                    observed_sha1: member.observed_sha1.clone(),
                    observed_crc32: member.observed_crc32.clone(),
                });
        }
    }

    let mut plan = MameMergedReconstructionPlan {
        dat_version: dat.source.version.clone().unwrap_or_default(),
        dat_sha256: dat_sha256.to_string(),
        parent: parent.to_string(),
        clones,
        destination: root.join(format!("{parent}.zip")),
        required_members: required,
        sources: Vec::new(),
        missing_members: Vec::new(),
        duplicate_candidates: Vec::new(),
        hash_mismatches: Vec::new(),
        unresolved_ownership: Vec::new(),
        collisions: Vec::new(),
        ready_to_apply: false,
        reasons: Vec::new(),
    };
    for requirement in &plan.required_members {
        let Some(identity) = rom_identity_from_requirement(requirement) else {
            plan.unresolved_ownership
                .push(requirement.member_name.clone());
            continue;
        };
        let matches = candidates.get(&identity).cloned().unwrap_or_default();
        match matches.as_slice() {
            [] => plan.missing_members.push(requirement.member_name.clone()),
            [source] => {
                if source.target_name != requirement.member_name {
                    plan.hash_mismatches.push(requirement.member_name.clone());
                } else {
                    plan.sources.push(source.clone());
                }
            }
            _ => plan
                .duplicate_candidates
                .push(requirement.member_name.clone()),
        }
    }
    plan.sources.sort_by(|a, b| {
        a.target_name
            .cmp(&b.target_name)
            .then(a.member_path.cmp(&b.member_path))
    });
    if plan.destination.exists() {
        plan.collisions.push(plan.destination.display().to_string());
    }
    // Two different required payloads cannot share one destination member name,
    // and a destination member name must be a plain file name.
    let mut destination_names = BTreeSet::new();
    for requirement in &plan.required_members {
        if !is_plain_member_name(&requirement.member_name) {
            plan.collisions.push(format!(
                "unsafe destination member name: {}",
                requirement.member_name
            ));
        } else if !destination_names.insert(requirement.member_name.clone()) {
            plan.collisions.push(format!(
                "duplicate destination member name: {}",
                requirement.member_name
            ));
        }
    }
    let unsupported_sources: BTreeSet<String> = plan
        .sources
        .iter()
        .filter(|source| {
            !source.archive_path.is_dir()
                && !source
                    .archive_path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        })
        .map(|source| source.archive_path.display().to_string())
        .collect();
    if !unsupported_sources.is_empty() {
        // Only extracted folders and ZIP archives can supply members; a member
        // inside another archive type is not extracted by this workflow.
        plan.reasons.push(format!(
            "source is neither an extracted set directory nor a ZIP archive: {}",
            unsupported_sources
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !plan.missing_members.is_empty() {
        plan.reasons.push("required ROM members are missing".into());
    }
    if !plan.duplicate_candidates.is_empty() {
        plan.reasons
            .push("one or more required ROMs have multiple incompatible sources".into());
    }
    if !plan.hash_mismatches.is_empty() {
        plan.reasons
            .push("persisted member hashes disagree with catalogue ownership".into());
    }
    if !plan.unresolved_ownership.is_empty() {
        plan.reasons.push("ROM ownership is unresolved".into());
    }
    if !plan.collisions.is_empty() {
        plan.reasons
            .push("destination collision cannot be proven safe".into());
    }
    plan.ready_to_apply = plan.reasons.is_empty()
        && plan.sources.len() == plan.required_members.len()
        && plan.required_members.len() >= 1;
    Ok(plan)
}

/// Discovers packed set archives whose filename is only used as a bounded
/// candidate filter.  Every usable member is still selected by its decoded
/// checksum when the reconstruction plan is built; the ZIP basename never
/// establishes ROM identity.
pub fn discover_packed_zip_sources(
    root: &Path,
    dat: &ParsedDat,
    requested_set: &str,
    dat_sha256: &str,
) -> Result<Vec<(PathBuf, ArcadeJoinEvidence)>, String> {
    let selected = dat
        .games
        .iter()
        .find(|game| game.name == requested_set)
        .ok_or_else(|| {
            format!("MAME set is absent from the selected catalogue: {requested_set}")
        })?;
    let parent = selected
        .clone_of
        .as_deref()
        .unwrap_or(selected.name.as_str());
    let family = dat
        .games
        .iter()
        .filter(|game| game.name == parent || game.clone_of.as_deref() == Some(parent))
        .map(|game| game.name.clone())
        .collect::<BTreeSet<_>>();
    let mut target_names = BTreeMap::new();
    for game in &dat.games {
        if family.contains(&game.name) {
            for rom in game.roms.iter().filter(|rom| !is_non_physical(rom)) {
                if let Some(identity) = rom_identity(rom) {
                    target_names
                        .entry(identity)
                        .or_insert_with(|| rom.name.clone());
                }
            }
        }
    }
    let trusted = crate::safe_read::TrustedRoots::from_paths(std::iter::once(root));
    let cancel = AtomicBool::new(false);
    let mut paths = std::fs::read_dir(root)
        .map_err(|error| format!("scan packed MAME sources: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        // Name filters first: `is_file` is a stat, and it only matters for the few
        // entries that could be this family's archives. Same result, one stat per
        // candidate instead of one per entry in the folder.
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        })
        .filter(|path| {
            path.file_stem()
                .map(|stem| family.contains(&stem.to_string_lossy().to_ascii_lowercase()))
                .unwrap_or(false)
        })
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort();

    let mut discovered = Vec::new();
    for path in paths {
        let mut source = crate::dat::archive::zip::ZipArchiveSource::open(
            &path,
            &trusted,
            crate::dat::archive::limits::ArchiveLimits::default(),
            &cancel,
        )
        .map_err(|error| format!("packed MAME source {} refused: {error:?}", path.display()))?;
        let mut budget = crate::dat::archive::ArchiveRunBudget::new(
            crate::dat::archive::limits::MAX_ARCHIVE_LOGICAL_BYTES,
        );
        let outcome = source.verify_all(&cancel, &mut budget);
        if !matches!(
            outcome.completion,
            crate::dat::archive::ArchivePassCompletion::Complete
        ) {
            return Err(format!(
                "packed MAME source {} changed or could not be fully verified",
                path.display()
            ));
        }
        if let Some(refused) = outcome.members.iter().find(|member| {
            !matches!(
                member.status,
                crate::dat::archive::ArchiveMemberStatus::HashComplete
            )
        }) {
            return Err(format!(
                "packed MAME source {} member {} refused: {:?}",
                path.display(),
                refused.member_name_display,
                refused.status
            ));
        }
        let members = outcome
            .members
            .into_iter()
            .filter_map(|member| {
                let hashes = member.hashes?;
                if member.member_name_display.is_empty() {
                    return None;
                }
                let identity = evidence_identity(Some(&hashes.sha1), Some(&hashes.crc32))?;
                let target_name = target_names.get(&identity)?.clone();
                Some(super::mame_arcade_join::ArcadeMemberEvidence {
                    name: target_name,
                    kind: super::mame_arcade_join::MemberEvidenceKind::Present,
                    current_name: Some(member.member_name_display),
                    checksum: Some(hashes.sha1.clone()),
                    observed_sha1: Some(hashes.sha1),
                    observed_crc32: Some(hashes.crc32),
                })
            })
            .collect::<Vec<_>>();
        if members.is_empty() {
            continue;
        }
        let set_name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| parent.to_string());
        discovered.push((
            path,
            ArcadeJoinEvidence {
                logical_set_name: set_name.clone(),
                dat_set_name: Some(set_name),
                class: ArcadeJoinClass::ExactSetMatch,
                description: None,
                manufacturer: None,
                year: None,
                clone_of: None,
                rom_of: None,
                parent_description: None,
                runnable: Some("yes".into()),
                mechanical: false,
                is_bios: false,
                is_device: false,
                expected_member_count: members.len(),
                members,
                dependencies: Vec::new(),
                launchable_normal_game: true,
                dat_version: dat.source.version.clone().unwrap_or_default(),
                dat_sha256: dat_sha256.to_string(),
                dat_path: String::new(),
                audited_at: format!("unix:{}", crate::dat::sources::now_unix()),
            },
        ));
    }
    Ok(discovered)
}

/// Free bytes on the filesystem that holds `path` (or its nearest existing
/// ancestor); `None` when it cannot be determined, in which case the check is
/// skipped rather than guessed.
fn free_space_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut probe = path;
    while !probe.exists() {
        probe = probe.parent()?;
    }
    let c_path = std::ffi::CString::new(probe.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated path and `stats` is a valid out-pointer.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

/// Writes only a staging ZIP. It never opens a source archive for writing and
/// never removes or renames a source member. Publication is intentionally a
/// separate caller action after [`verify_staged_output`].
pub fn stage_reconstruction_output(
    plan: &MameMergedReconstructionPlan,
    staging_root: &Path,
) -> Result<PathBuf, String> {
    stage_reconstruction_output_with(plan, staging_root, &free_space_bytes)
}

/// [`stage_reconstruction_output`] with the free-space probe injected.
pub fn stage_reconstruction_output_with(
    plan: &MameMergedReconstructionPlan,
    staging_root: &Path,
    free_space: &dyn Fn(&Path) -> Option<u64>,
) -> Result<PathBuf, String> {
    if !plan.ready_to_apply {
        return Err("reconstruction plan is blocked; no staged output was created".into());
    }
    if !super::rename_apply::preflight::is_safe_basename(&plan.parent)
        || !staging_root.is_absolute()
        || staging_root.components().any(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err("unsafe reconstruction staging path".into());
    }
    let mut ancestor = PathBuf::new();
    for component in staging_root.components() {
        ancestor.push(component);
        match std::fs::symlink_metadata(&ancestor) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("unsafe staging ancestor".into()),
        }
    }

    // Everything that can be refused before a single byte is written.
    let mut total_bytes = 0_u64;
    let mut largest_member = 0_u64;
    for source in &plan.sources {
        let requirement = plan
            .required_members
            .iter()
            .find(|requirement| requirement.member_name == source.target_name)
            .ok_or_else(|| {
                format!(
                    "source has no reviewed requirement for {}",
                    source.target_name
                )
            })?;
        let member_size = requirement
            .size_bytes
            .unwrap_or(MAX_RECONSTRUCTION_STAGED_BYTES.saturating_add(1));
        total_bytes = total_bytes
            .checked_add(member_size)
            .ok_or_else(|| "reconstruction staged-byte bound overflowed".to_string())?;
        if total_bytes > MAX_RECONSTRUCTION_STAGED_BYTES {
            return Err(format!(
                "reconstruction staged-byte bound exceeded ({MAX_RECONSTRUCTION_STAGED_BYTES} bytes)"
            ));
        }
        largest_member = largest_member.max(member_size);
        // A packed source archive must be the one that was reviewed.
        if !source.archive_path.is_dir()
            && let Some(expected) = source.archive_identity
            && SourceArchiveIdentity::of(&source.archive_path) != Some(expected)
        {
            return Err(format!(
                "source archive {} changed since the preview; nothing was staged. Preview again.",
                source.archive_path.display()
            ));
        }
    }
    // The staged ZIP (at most the sum of its members) plus one member being
    // copied out of its source archive, with a small margin.
    let needed = total_bytes
        .saturating_add(largest_member)
        .saturating_add(1024 * 1024);
    if let Some(free) = free_space(staging_root)
        && free < needed
    {
        return Err(format!(
            "not enough free space to stage the reconstruction: {needed} bytes needed, {free} available; nothing was staged"
        ));
    }

    std::fs::create_dir_all(staging_root).map_err(|e| e.to_string())?;
    let staged = staging_root.join(format!("{}.zip.staged", plan.parent));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .map_err(|e| e.to_string())?;
    let temp_root = staging_root.join("source-members");
    let mut temporaries = Vec::new();
    let result = stage_members(plan, file, &temp_root, &mut temporaries);
    // The staging ZIP is ours (create_new succeeded) and disposable on failure:
    // nothing was published, and no half-built set is left behind.
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    for temporary in temporaries {
        let _ = std::fs::remove_file(temporary);
    }
    let _ = std::fs::remove_dir(&temp_root);
    result?;
    verify_staged_output(plan, &staged).inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })?;
    Ok(staged)
}

fn stage_members(
    plan: &MameMergedReconstructionPlan,
    file: std::fs::File,
    temp_root: &Path,
    temporaries: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let mut writer = zip::ZipWriter::new(file);
    let cancel = AtomicBool::new(false);
    for (index, source) in plan.sources.iter().enumerate() {
        let requirement = plan
            .required_members
            .iter()
            .find(|requirement| requirement.member_name == source.target_name)
            .ok_or_else(|| {
                format!(
                    "source has no reviewed requirement for {}",
                    source.target_name
                )
            })?;
        let member_size = requirement
            .size_bytes
            .unwrap_or(MAX_RECONSTRUCTION_STAGED_BYTES.saturating_add(1));
        let source_file = if source.archive_path.is_dir() {
            std::fs::File::open(&source.member_path).map_err(|e| {
                format!(
                    "could not read staged source member {}: {e}",
                    source.member_path.display()
                )
            })?
        } else {
            let temporary = temp_root.join(format!("{index}.member"));
            let trusted = TrustedRoots::from_paths(source.archive_path.parent().into_iter());
            // Exactly the entry the plan chose, by exact name and by every
            // authoritative checksum: never "something similar".
            let requirement_request = crate::dat::archive::zip::ZipMemberRequest {
                member_name: Some(source.current_name.clone()),
                require_exact_name: true,
                size_bytes: requirement.size_bytes,
                sha1: requirement.sha1.clone(),
                crc32: requirement.crc32.clone(),
            };
            temporaries.push(temporary.clone());
            crate::dat::archive::zip::copy_zip_member_to(
                &source.archive_path,
                &trusted,
                &crate::dat::archive::limits::ArchiveLimits::default(),
                &cancel,
                &requirement_request,
                &temporary,
            )
            .map_err(|error| format!("ZIP member {} refused: {error}", source.current_name))?;
            std::fs::File::open(&temporary).map_err(|e| e.to_string())?
        };
        writer
            .start_file(
                &source.target_name,
                zip::write::SimpleFileOptions::default(),
            )
            .map_err(|e| e.to_string())?;
        let mut source_file = source_file;
        use std::io::Read;
        let copied = std::io::copy(&mut (&mut source_file).take(member_size + 1), &mut writer)
            .map_err(|e| e.to_string())?;
        if copied != member_size {
            return Err("source member size changed while staging".into());
        }
        drop(source_file);
        if let Some(temporary) = temporaries.last().filter(|_| !source.archive_path.is_dir()) {
            let _ = std::fs::remove_file(temporary);
        }
    }
    writer
        .finish()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Verifies the staged archive against the reviewed member list before any
/// publication. Duplicate ZIP names and checksum/size mismatches fail closed.
pub fn verify_staged_output(
    plan: &MameMergedReconstructionPlan,
    staged: &Path,
) -> Result<(), String> {
    verified_staged_members(plan, staged).map(|_| ())
}

/// [`verify_staged_output`], returning the staged members' own evidence so the
/// receipt can record the hashes that were actually staged (not just the ones
/// that were expected).
fn verified_staged_members(
    plan: &MameMergedReconstructionPlan,
    staged: &Path,
) -> Result<Vec<crate::dat::archive::ArchiveMemberEvidence>, String> {
    let members = read_zip_evidence(staged)?;
    verify_members(plan, &members)?;
    Ok(members)
}

/// Journal key under which a reconstruction's source provenance is recorded.
pub const MAME_RECONSTRUCTION_PROVENANCE_KEY: &str = "mame_reconstruction_provenance";

/// What a receipt must be able to say about every staged member: where it came
/// from (archive and member, or a loose file), what the DAT required, what was
/// actually staged, and the checksum-based basis on which the member was
/// accepted as owned. Filenames are recorded, never used as the basis.
fn reconstruction_provenance(
    plan: &MameMergedReconstructionPlan,
    staged: &[crate::dat::archive::ArchiveMemberEvidence],
) -> Result<serde_json::Value, String> {
    let mut members = Vec::with_capacity(plan.sources.len());
    for source in &plan.sources {
        let requirement = plan
            .required_members
            .iter()
            .find(|requirement| requirement.member_name == source.target_name)
            .ok_or_else(|| format!("no reviewed requirement for {}", source.target_name))?;
        let actual = staged
            .iter()
            .find(|member| member.member_name_display == source.target_name)
            .ok_or_else(|| format!("staged output has no member {}", source.target_name))?;
        let hashes = actual
            .hashes
            .as_ref()
            .ok_or_else(|| format!("staged member {} has no hashes", source.target_name))?;
        let packed = !source.archive_path.is_dir();
        members.push(serde_json::json!({
            "target_name": source.target_name,
            "owner_set": requirement.owner_set,
            "packed": packed,
            "source_archive": source.archive_path.display().to_string(),
            "source_member": if packed {
                source.current_name.clone()
            } else {
                source.member_path.display().to_string()
            },
            "expected": {
                "size_bytes": requirement.size_bytes,
                "sha1": requirement.sha1,
                "crc32": requirement.crc32,
            },
            "staged": {
                "size_bytes": actual.logical_size,
                "sha1": hashes.sha1,
                "crc32": hashes.crc32,
            },
            "ownership": {
                "basis": "checksum match against persisted MAME join evidence",
                "observed_sha1": source.observed_sha1,
                "observed_crc32": source.observed_crc32,
            },
        }));
    }
    Ok(serde_json::json!({
        "schema": 1,
        "dat_version": plan.dat_version,
        "dat_sha256": plan.dat_sha256,
        "parent": plan.parent,
        "destination": plan.destination.display().to_string(),
        "members": members,
    }))
}

/// Publishes a verified staged output through the shared journaled rename
/// executor. The caller is responsible for obtaining explicit user approval;
/// this function never publishes a blocked plan and never touches a source
/// member. A returned transaction is the recovery/undo handle.
pub fn apply_staged_reconstruction_output(
    plan: &MameMergedReconstructionPlan,
    staging_root: &Path,
    journal_dir: &Path,
) -> Result<crate::dat::rename_apply::executor::ApplyOutcome, String> {
    let staged = stage_reconstruction_output(plan, staging_root)?;
    publish_staged(plan, &staged, staging_root, journal_dir, Default::default())
}

fn publish_staged(
    plan: &MameMergedReconstructionPlan,
    staged: &Path,
    staging_root: &Path,
    journal_dir: &Path,
    operation: crate::dat::rename_apply::model::TransactionOperation,
) -> Result<crate::dat::rename_apply::executor::ApplyOutcome, String> {
    use crate::dat::rename_apply::executor::{
        ApplyError, ApplyExecution, HardConflictMode, apply_transaction,
    };
    use crate::dat::rename_apply::identity::capture_identity;
    use crate::dat::rename_apply::model::{
        EntryState, RenameTransaction, TransactionEntry, TransactionState,
    };
    use crate::dat::rename_apply::preflight::DirectoryPolicy;
    use crate::safe_read::TrustedRoots;
    use std::collections::BTreeSet;
    use std::sync::atomic::AtomicBool;

    let identity = capture_identity(staged).map_err(|e| e.to_string())?;
    let staged_members = verified_staged_members(plan, staged)?;
    let provenance = reconstruction_provenance(plan, &staged_members)?;
    let after = capture_identity(staged).map_err(|e| e.to_string())?;
    if !crate::dat::rename_apply::identity::identity_matches(&identity, &after) {
        return Err("staged reconstruction changed during verification".into());
    }
    let destination = plan.destination.clone();
    if (matches!(
        operation,
        crate::dat::rename_apply::model::TransactionOperation::RenameMove
    ) && std::fs::symlink_metadata(&destination).is_ok())
        || destination.parent().is_none_or(|parent| !parent.is_dir())
    {
        return Err(
            "destination collision or missing destination directory; nothing was published".into(),
        );
    }
    let source_key = staged.to_string_lossy().into_owned();
    let entry = TransactionEntry {
        source_path: staged.to_path_buf(),
        destination_path: destination.clone(),
        original_basename: staged
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default(),
        proposed_basename: destination
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default(),
        identity,
        operation,
        preflight_passed: false,
        preflight_failures: Vec::new(),
        state: EntryState::Planned,
        failure_reason: None,
        applied_at_unix: None,
        rolled_back_at_unix: None,
        unknown: Default::default(),
    };
    let mut transaction = RenameTransaction {
        transaction_id: crate::dat::rename_apply::journal::new_transaction_id(
            crate::dat::sources::now_unix(),
        ),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.to_string()),
        created_at_unix: crate::dat::sources::now_unix(),
        source_scan_root: staging_root.to_string_lossy().into_owned(),
        state: TransactionState::Planned,
        entries: vec![entry],
        created_directories: Vec::new(),
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: Default::default(),
    };
    transaction.unknown.insert(
        "workflow".into(),
        serde_json::Value::String(MAME_RECONSTRUCTION_WORKFLOW.into()),
    );
    transaction.unknown.insert(
        "mame_parent".into(),
        serde_json::Value::String(plan.parent.clone()),
    );
    transaction
        .unknown
        .insert(MAME_RECONSTRUCTION_PROVENANCE_KEY.into(), provenance);
    let mut approved = BTreeSet::new();
    approved.insert(source_key);
    let cancel = AtomicBool::new(false);
    apply_transaction(&mut ApplyExecution {
        transaction: &mut transaction,
        approved_paths: approved,
        current_generation: 1,
        trusted: TrustedRoots::from_paths([staging_root, plan.destination.parent().unwrap()]),
        journal_dir: journal_dir.to_path_buf(),
        hard_conflict_mode: HardConflictMode::AbortAll,
        cancel: &cancel,
        directory_policy: DirectoryPolicy::SameFilesystem,
        allow_symlink_source: false,
    })
    .map_err(|error| match error {
        ApplyError::HardConflicts(conflicts) => format!(
            "reconstruction preflight refused {} path(s): {}",
            conflicts.len(),
            conflicts
                .into_iter()
                .map(|(path, reasons)| format!("{} ({})", path.display(), reasons.join(", ")))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        other => other.to_string(),
    })
}

/// A destination ZIP member name: no absolute path, traversal, backslash,
/// drive prefix or NUL (a relative path inside the set is allowed).
fn is_plain_member_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', '\0'])
        && !name.starts_with('/')
        && !(name.len() >= 2
            && name.as_bytes()[1] == b':'
            && name.as_bytes()[0].is_ascii_alphabetic())
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn is_non_physical(rom: &DatRomEntry) -> bool {
    rom.optional
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("yes"))
        || rom
            .status
            .as_deref()
            .is_some_and(|v| v.eq_ignore_ascii_case("nodump"))
        || rom.loadflag.is_some()
}

fn rom_identity(rom: &DatRomEntry) -> Option<String> {
    rom.sha1
        .as_ref()
        .map(|v| format!("sha1:{}", v.to_ascii_lowercase()))
        .or_else(|| {
            rom.crc32
                .as_ref()
                .map(|v| format!("crc32:{}", v.to_ascii_lowercase()))
        })
}

fn rom_identity_from_requirement(rom: &ReconstructionMemberRequirement) -> Option<String> {
    rom.sha1
        .as_ref()
        .map(|v| format!("sha1:{v}"))
        .or_else(|| rom.crc32.as_ref().map(|v| format!("crc32:{v}")))
}

fn evidence_identity(sha1: Option<&str>, crc32: Option<&str>) -> Option<String> {
    sha1.map(|v| format!("sha1:{}", v.to_ascii_lowercase()))
        .or_else(|| crc32.map(|v| format!("crc32:{}", v.to_ascii_lowercase())))
}

#[cfg(test)]
mod packed_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatSource};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static NEXT_PUBLISH_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn dat(games: Vec<DatGameEntry>) -> ParsedDat {
        ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: "fixture".into(),
                name: None,
                description: None,
                version: Some("test".into()),
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: games.len(),
                rom_count: games.iter().map(|g| g.roms.len()).sum(),
                parse_warnings: vec![],
                packing_policy: DatPackingPolicy::Standard,
            },
            games,
        }
    }
    fn game(name: &str, clone_of: Option<&str>, roms: &[(&str, &str)]) -> DatGameEntry {
        DatGameEntry {
            name: name.into(),
            clone_of: clone_of.map(str::to_owned),
            roms: roms
                .iter()
                .map(|(n, h)| DatRomEntry {
                    name: (*n).into(),
                    sha1: Some((*h).into()),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }
    fn join(set: &str, sha: &str, names: &[(&str, &str)]) -> ArcadeJoinEvidence {
        ArcadeJoinEvidence {
            logical_set_name: set.into(),
            dat_set_name: Some(set.into()),
            class: super::super::mame_arcade_join::ArcadeJoinClass::ExactSetMatch,
            description: None,
            manufacturer: None,
            year: None,
            clone_of: None,
            rom_of: None,
            parent_description: None,
            runnable: None,
            mechanical: false,
            is_bios: false,
            is_device: false,
            expected_member_count: names.len(),
            members: names
                .iter()
                .map(
                    |(name, hash)| super::super::mame_arcade_join::ArcadeMemberEvidence {
                        name: (*name).into(),
                        kind: super::super::mame_arcade_join::MemberEvidenceKind::Present,
                        current_name: Some(format!("{name}.bin")),
                        checksum: Some((*hash).into()),
                        observed_sha1: Some((*hash).into()),
                        observed_crc32: None,
                    },
                )
                .collect(),
            dependencies: vec![],
            launchable_normal_game: true,
            dat_version: "test".into(),
            dat_sha256: sha.into(),
            dat_path: "fixture".into(),
            audited_at: "now".into(),
        }
    }
    #[test]
    fn a_member_in_an_unsupported_archive_type_blocks_the_plan_and_names_the_archive() {
        let root = std::env::temp_dir().join(format!("emuwiz-merged-7z-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let parsed = dat(vec![
            game("parent", None, &[("a", "a")]),
            game("clone", Some("parent"), &[("c", "c")]),
        ]);
        let archive = PathBuf::from("/roms/parent.7z");
        let joins = vec![
            (archive.clone(), join("parent", "digest", &[("a", "a")])),
            (root.join("clone"), join("clone", "digest", &[("c", "c")])),
        ];
        let plan =
            build_merged_reconstruction_plan(&root, &parsed, &joins, "parent", "digest").unwrap();
        assert!(plan.blocked());
        let reason = plan
            .reasons
            .iter()
            .find(|reason| reason.contains("neither an extracted set directory nor a ZIP"))
            .expect("the unsupported-source reason is present");
        assert!(reason.contains("/roms/parent.7z"), "{reason}");
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn complete_family_is_deterministic_and_ready() {
        let root = std::env::temp_dir().join(format!("emuwiz-merged-plan-{}", std::process::id()));
        let _ = std::fs::create_dir_all(root.join("parent"));
        let _ = std::fs::create_dir_all(root.join("clone"));
        let parsed = dat(vec![
            game("parent", None, &[("a", "a"), ("b", "b")]),
            game("clone", Some("parent"), &[("a", "a"), ("c", "c")]),
        ]);
        let joins = vec![
            (
                root.join("parent"),
                join("parent", "digest", &[("a", "a"), ("b", "b")]),
            ),
            (root.join("clone"), join("clone", "digest", &[("c", "c")])),
        ];
        let plan =
            build_merged_reconstruction_plan(&root, &parsed, &joins, "parent", "digest").unwrap();
        assert!(plan.ready_to_apply);
        assert_eq!(
            plan.sources
                .iter()
                .map(|s| s.target_name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn a_plan_from_only_the_family_joins_equals_the_plan_from_every_join() {
        let parsed = dat(vec![
            game("parent", None, &[("a", "a"), ("b", "b")]),
            game("clone", Some("parent"), &[("c", "c")]),
            game("stranger", None, &[("a", "a"), ("z", "z")]),
        ]);
        let family = reconstruction_family_names(&parsed, "clone").unwrap();
        assert_eq!(family, vec!["parent".to_string(), "clone".to_string()]);
        assert_eq!(
            reconstruction_family_names(&parsed, "parent").unwrap(),
            family
        );
        assert!(reconstruction_family_names(&parsed, "absent").is_err());

        let every = vec![
            (
                PathBuf::from("/s/parent"),
                join("parent", "digest", &[("a", "a"), ("b", "b")]),
            ),
            (
                PathBuf::from("/s/clone"),
                join("clone", "digest", &[("c", "c")]),
            ),
            // An unrelated set with the same bytes must not become a duplicate source.
            (
                PathBuf::from("/s/stranger"),
                join("stranger", "digest", &[("a", "a"), ("z", "z")]),
            ),
        ];
        let only_family: Vec<_> = every
            .iter()
            .filter(|(_, e)| family.contains(&e.logical_set_name))
            .cloned()
            .collect();
        let root = Path::new("/output");
        let from_every =
            build_merged_reconstruction_plan(root, &parsed, &every, "clone", "digest").unwrap();
        let from_family =
            build_merged_reconstruction_plan(root, &parsed, &only_family, "clone", "digest")
                .unwrap();
        assert_eq!(from_every, from_family);
        assert!(from_every.duplicate_candidates.is_empty());
    }

    #[test]
    fn packed_source_discovery_only_considers_family_zip_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let parsed = dat(vec![
            game("parent", None, &[("a", "a")]),
            game("clone", Some("parent"), &[("c", "c")]),
        ]);
        // A directory that merely has a family archive's name is not an archive,
        // and a file for another set is never opened (it is not a valid zip).
        std::fs::create_dir(root.join("parent.zip")).unwrap();
        std::fs::write(root.join("stranger.zip"), b"not a zip").unwrap();
        std::fs::write(root.join("parent.txt"), b"x").unwrap();
        assert!(
            discover_packed_zip_sources(root, &parsed, "parent", "digest")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn duplicate_and_missing_members_block() {
        let parsed = dat(vec![
            game("parent", None, &[("a", "a"), ("b", "b")]),
            game("clone", Some("parent"), &[]),
        ]);
        let joins = vec![
            (
                PathBuf::from("/one"),
                join("parent", "digest", &[("a", "a")]),
            ),
            (
                PathBuf::from("/two"),
                join("parent", "digest", &[("a", "a")]),
            ),
        ];
        let plan = build_merged_reconstruction_plan(
            Path::new("/output"),
            &parsed,
            &joins,
            "parent",
            "digest",
        )
        .unwrap();
        assert!(!plan.ready_to_apply);
        assert_eq!(plan.duplicate_candidates, vec!["a"]);
        assert_eq!(plan.missing_members, vec!["b"]);
    }

    fn publish_fixture(expected_sha1: &str) -> (MameMergedReconstructionPlan, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "emuwiz-mame-publish-{}-{}-{}",
            std::process::id(),
            crate::dat::sources::now_unix(),
            NEXT_PUBLISH_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let source_root = root.join("source-set");
        std::fs::create_dir_all(&source_root).unwrap();
        let member_path = source_root.join("member.bin");
        std::fs::write(&member_path, b"mame member").unwrap();
        let plan = MameMergedReconstructionPlan {
            dat_version: "test".into(),
            dat_sha256: "digest".into(),
            parent: "parent".into(),
            clones: vec!["clone".into()],
            destination: root.join("parent.zip"),
            required_members: vec![ReconstructionMemberRequirement {
                owner_set: "parent".into(),
                member_name: "member.bin".into(),
                size_bytes: Some(b"mame member".len() as u64),
                sha1: Some(expected_sha1.into()),
                crc32: None,
            }],
            sources: vec![ReconstructionMemberSource {
                archive_identity: None,
                archive_path: source_root,
                member_path: member_path.clone(),
                current_name: "member.bin".into(),
                target_name: "member.bin".into(),
                observed_sha1: Some(expected_sha1.into()),
                observed_crc32: None,
            }],
            missing_members: Vec::new(),
            duplicate_candidates: Vec::new(),
            hash_mismatches: Vec::new(),
            unresolved_ownership: Vec::new(),
            collisions: Vec::new(),
            ready_to_apply: true,
            reasons: Vec::new(),
        };
        (plan, root, member_path)
    }

    #[test]
    fn staged_publish_journals_the_output_and_leaves_source_member_untouched() {
        use crate::dat::rename_apply::journal::list_journals;
        use sha1::Digest;

        let digest = sha1::Sha1::digest(b"mame member")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let (plan, root, member_path) = publish_fixture(&digest);
        let staging = root.join("staging");
        let journal = root.join("journal");
        let outcome = apply_staged_reconstruction_output(&plan, &staging, &journal)
            .unwrap_or_else(|error| panic!("publication fixture failed: {error}"));

        assert!(plan.destination.exists());
        assert_eq!(std::fs::read(&member_path).unwrap(), b"mame member");
        assert_eq!(
            outcome.transaction.state,
            crate::dat::rename_apply::model::TransactionState::Applied
        );
        let (journals, problems) = list_journals(&journal);
        assert!(problems.is_empty());
        assert_eq!(journals.len(), 1);
        assert_eq!(
            journals[0].unknown["workflow"],
            MAME_RECONSTRUCTION_WORKFLOW
        );
        let mut transaction = outcome.transaction;
        let rollback = crate::dat::rename_apply::rollback_transaction(
            &mut transaction,
            &journal,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            rollback.transaction.state,
            crate::dat::rename_apply::model::TransactionState::RolledBack
        );
        assert!(!plan.destination.exists());
        assert_eq!(std::fs::read(&member_path).unwrap(), b"mame member");
        let _ = std::fs::remove_dir_all(root);
    }

    /// The executor reports a publication that failed after preflight as
    /// `Ok(ApplyOutcome)` whose transaction is `ApplyFailed`, so the outer
    /// `Result` alone never proves that anything was published.
    #[cfg(unix)]
    #[test]
    fn a_failed_publication_is_an_ok_outcome_whose_transaction_is_apply_failed() {
        use crate::dat::rename_apply::journal::list_journals;
        use crate::dat::rename_apply::model::{EntryState, TransactionState};
        use sha1::Digest;
        use std::os::unix::fs::PermissionsExt;

        let digest = sha1::Sha1::digest(b"mame member")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let (plan, root, member_path) = publish_fixture(&digest);
        let staging = root.join("staging");
        let journal = root.join("journal");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&journal).unwrap();
        // The destination directory refuses new entries, so the final rename
        // fails after every preflight check has passed.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = apply_staged_reconstruction_output(&plan, &staging, &journal);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        let outcome = result.expect("the executor reports this failure inside Ok");
        assert_eq!(outcome.transaction.state, TransactionState::ApplyFailed);
        assert_eq!(
            outcome.transaction.entries[0].state,
            EntryState::ApplyFailed
        );
        assert!(outcome.transaction.entries[0].failure_reason.is_some());
        assert_eq!(outcome.summary.applied, 0);
        assert_eq!(outcome.summary.failed, 1);
        assert!(!plan.destination.exists());
        assert_eq!(std::fs::read(&member_path).unwrap(), b"mame member");
        // The failed transaction stays in the recovery journal for diagnosis.
        let (journals, problems) = list_journals(&journal);
        assert!(problems.is_empty());
        assert_eq!(journals.len(), 1);
        assert_eq!(journals[0].state, TransactionState::ApplyFailed);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failed_staged_verification_never_publishes_the_destination() {
        let (mut plan, root, member_path) =
            publish_fixture("0000000000000000000000000000000000000000");
        plan.sources[0].observed_sha1 = Some("0000000000000000000000000000000000000000".into());
        let staging = root.join("staging");
        let journal = root.join("journal");
        let error = apply_staged_reconstruction_output(&plan, &staging, &journal).unwrap_err();

        assert!(error.contains("staged SHA-1/CRC/size mismatch"));
        assert!(!plan.destination.exists());
        assert_eq!(std::fs::read(&member_path).unwrap(), b"mame member");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn packed_zip_member_is_staged_without_modifying_the_source_archive() {
        use sha1::Digest;
        use zip::write::SimpleFileOptions;

        let digest = sha1::Sha1::digest(b"mame member")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let (mut plan, root, _loose_member) = publish_fixture(&digest);
        let packed = root.join("parent.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&packed).unwrap());
        writer
            .start_file("member.bin", SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, b"mame member").unwrap();
        writer.finish().unwrap();
        let before = std::fs::read(&packed).unwrap();
        plan.sources[0].archive_path = packed.clone();
        plan.sources[0].member_path = PathBuf::from("member.bin");
        plan.sources[0].current_name = "member.bin".into();
        let staging = root.join("staging");
        let staged = stage_reconstruction_output(&plan, &staging).unwrap();
        verify_staged_output(&plan, &staged).unwrap();
        assert_eq!(std::fs::read(&packed).unwrap(), before);
        assert!(!root.join("member.bin").exists());
        let _ = std::fs::remove_dir_all(root);
    }
    mod replacement_publication {
        use super::*;
        use crate::dat::rename_apply::{journal, rollback};
        use std::fs;
        use std::io::Write;

        fn zip(path: &Path, members: &[(&str, &[u8])]) {
            let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
            for (name, bytes) in members {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        fn sha(bytes: &[u8]) -> String {
            use sha1::Digest;
            sha1::Sha1::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        }
        struct Fixture {
            root: tempfile::TempDir,
            plan: MameMergedReconstructionPlan,
            evidence: ArcadeJoinEvidence,
        }
        impl Fixture {
            fn new(members: &[(&str, &[u8])]) -> Self {
                let root = tempfile::tempdir().unwrap();
                let a = sha(b"a");
                let b = sha(b"b");
                let mut parsed = dat(vec![
                    game("parent", None, &[("a.bin", &a), ("b.bin", &b)]),
                    game("clone", Some("parent"), &[("b.bin", &b)]),
                ]);
                for game in &mut parsed.games {
                    for rom in &mut game.roms {
                        rom.size_bytes = Some(1);
                    }
                }
                zip(&root.path().join("parent.zip"), members);
                zip(&root.path().join("clone.zip"), &[("b.bin", b"b")]);
                let mut evidence = join("parent", "digest", &[("a.bin", &a)]);
                evidence.members[0].current_name = Some("a.bin".into());
                let mut donor = join("clone", "digest", &[("b.bin", &b)]);
                donor.members[0].current_name = Some("b.bin".into());
                let plan = build_merged_reconstruction_plan(
                    root.path(),
                    &parsed,
                    &[
                        (root.path().join("parent.zip"), evidence.clone()),
                        (root.path().join("clone.zip"), donor),
                    ],
                    "parent",
                    "digest",
                )
                .unwrap();
                Self {
                    root,
                    plan,
                    evidence,
                }
            }
            fn review(&self) -> Result<ReviewedReconstructionPublication, String> {
                review_reconstruction_publication(&self.plan, Some(&self.evidence))
            }
            fn stage(&self) -> PathBuf {
                self.root.path().join("stage")
            }
            fn journal(&self) -> PathBuf {
                self.root.path().join("journal")
            }
        }

        #[test]
        fn incomplete_and_bad_hash_targets_preview_replacement_without_writes() {
            for members in [
                vec![("a.bin", b"a".as_slice())],
                vec![("a.bin", b"a".as_slice()), ("b.bin", b"x".as_slice())],
            ] {
                let f = Fixture::new(&members);
                let original = fs::read(&f.plan.destination).unwrap();
                assert!(f.plan.blocked()); // Existing canonical family collision projection is unchanged.
                let review = f.review().unwrap();
                assert!(matches!(
                    review.action(),
                    ReconstructionPublicationAction::ReplaceExisting { .. }
                ));
                assert!(review.plan().ready_to_apply);
                assert_eq!(fs::read(&f.plan.destination).unwrap(), original);
                assert!(!f.stage().exists());
                assert!(!f.journal().exists());
            }
        }

        #[test]
        fn packed_donor_replace_verifies_inventory_and_undo_restores_original_bytes() {
            let f = Fixture::new(&[("a.bin", b"a"), ("b.bin", b"x"), ("obsolete", b"z")]);
            let original = fs::read(&f.plan.destination).unwrap();
            let donor = fs::read(f.root.path().join("clone.zip")).unwrap();
            let reviewed = f.review().unwrap();
            let result = reviewed.apply(&f.stage(), &f.journal()).unwrap().unwrap();
            assert_eq!(
                result.transaction.state,
                crate::dat::rename_apply::TransactionState::Applied
            );
            verify_staged_output(reviewed.plan(), &f.plan.destination).unwrap();
            let preserved = &result.transaction.entries[0].source_path;
            assert_eq!(fs::read(preserved).unwrap(), original);
            assert_eq!(fs::read(f.root.path().join("clone.zip")).unwrap(), donor);
            // Reusing staging must not destroy the preserved original.
            assert!(stage_reconstruction_output(reviewed.plan(), &f.stage()).is_err());
            assert_eq!(fs::read(preserved).unwrap(), original);
            let path =
                journal::journal_path(&f.journal(), &result.transaction.transaction_id).unwrap();
            let mut transaction = journal::read_journal(&path).unwrap();
            rollback::rollback_transaction_confined(
                &mut transaction,
                &f.journal(),
                &AtomicBool::new(false),
                &TrustedRoots::from_paths([f.root.path()]),
            )
            .unwrap();
            assert_eq!(fs::read(&f.plan.destination).unwrap(), original);
            rollback::rollback_transaction_confined(
                &mut transaction,
                &f.journal(),
                &AtomicBool::new(false),
                &TrustedRoots::from_paths([f.root.path()]),
            )
            .unwrap();
            assert_eq!(fs::read(&f.plan.destination).unwrap(), original);
            assert_eq!(fs::read(f.root.path().join("clone.zip")).unwrap(), donor);
        }

        #[test]
        fn valid_existing_target_is_read_only_noop() {
            let f = Fixture::new(&[("a.bin", b"a"), ("b.bin", b"b")]);
            let original = fs::read(&f.plan.destination).unwrap();
            let review = f.review().unwrap();
            assert!(matches!(
                review.action(),
                ReconstructionPublicationAction::Unchanged { .. }
            ));
            assert!(review.apply(&f.stage(), &f.journal()).unwrap().is_none());
            assert_eq!(fs::read(&f.plan.destination).unwrap(), original);
            assert!(!f.stage().exists());
            assert!(!f.journal().exists());
        }

        #[test]
        fn reviewed_missing_target_still_uses_create_path() {
            let mut f = Fixture::new(&[("a.bin", b"a")]);
            let donor = f.root.path().join("a-donor.zip");
            fs::rename(&f.plan.destination, &donor).unwrap();
            f.plan.sources[0].archive_path = donor;
            f.plan.collisions.clear();
            f.plan.reasons.clear();
            f.plan.ready_to_apply = true;
            let review = review_reconstruction_publication(&f.plan, None).unwrap();
            assert_eq!(review.action(), &ReconstructionPublicationAction::Create);
            review.apply(&f.stage(), &f.journal()).unwrap().unwrap();
            verify_staged_output(&f.plan, &f.plan.destination).unwrap();
        }

        #[test]
        fn target_or_donor_changes_after_review_refuse_before_staging() {
            for donor in [false, true] {
                let f = Fixture::new(&[("a.bin", b"a")]);
                let review = f.review().unwrap();
                let target_before = fs::read(&f.plan.destination).unwrap();
                if donor {
                    zip(&f.root.path().join("clone.zip"), &[("b.bin", b"x")]);
                } else {
                    zip(&f.plan.destination, &[("a.bin", b"a"), ("later", b"x")]);
                }
                assert!(review.apply(&f.stage(), &f.journal()).is_err());
                assert!(!f.stage().exists());
                if donor {
                    assert_eq!(fs::read(&f.plan.destination).unwrap(), target_before);
                }
            }
        }

        #[test]
        fn changed_published_target_blocks_undo_without_clobbering_user_changes() {
            let f = Fixture::new(&[("a.bin", b"a")]);
            let result = f
                .review()
                .unwrap()
                .apply(&f.stage(), &f.journal())
                .unwrap()
                .unwrap();
            fs::write(&f.plan.destination, b"later user data").unwrap();
            let mut transaction = result.transaction;
            let result = rollback::rollback_transaction_confined(
                &mut transaction,
                &f.journal(),
                &AtomicBool::new(false),
                &TrustedRoots::from_paths([f.root.path()]),
            )
            .unwrap();
            assert_ne!(
                result.transaction.state,
                crate::dat::rename_apply::TransactionState::RolledBack
            );
            assert_eq!(fs::read(&f.plan.destination).unwrap(), b"later user data");
            assert!(transaction.entries[0].source_path.exists());
        }

        #[test]
        fn foreign_target_and_unproven_or_stale_dat_evidence_refuse() {
            let f = Fixture::new(&[("alien", b"q")]);
            assert!(f.review().is_err());
            let mut f = Fixture::new(&[("a.bin", b"a")]);
            assert!(review_reconstruction_publication(&f.plan, None).is_err());
            f.evidence.dat_sha256 = "different catalogue".into();
            assert!(f.review().is_err());
        }

        #[test]
        fn symlink_and_malformed_target_refuse() {
            let f = Fixture::new(&[("a.bin", b"a")]);
            fs::remove_file(&f.plan.destination).unwrap();
            std::os::unix::fs::symlink(f.root.path().join("clone.zip"), &f.plan.destination)
                .unwrap();
            assert!(f.review().is_err());
            fs::remove_file(&f.plan.destination).unwrap();
            fs::write(&f.plan.destination, b"malformed zip").unwrap();
            assert!(f.review().is_err());
        }

        #[test]
        fn bad_donor_evidence_cannot_publish_and_original_is_untouched() {
            let f = Fixture::new(&[("a.bin", b"a")]);
            let original = fs::read(&f.plan.destination).unwrap();
            zip(&f.root.path().join("clone.zip"), &[("b.bin", b"x")]);
            // Stale persisted join claims b; actual staging must verify it.
            let review = f.review().unwrap();
            assert!(review.apply(&f.stage(), &f.journal()).is_err());
            assert_eq!(fs::read(&f.plan.destination).unwrap(), original);
            assert!(!f.journal().exists());
        }

        #[test]
        fn verifier_refuses_unexpected_members_and_crc_only_wrong_bytes() {
            let mut f = Fixture::new(&[("a.bin", b"a")]);
            let output = f.root.path().join("output.zip");
            zip(
                &output,
                &[("a.bin", b"a"), ("b.bin", b"b"), ("extra", b"x")],
            );
            assert!(verify_staged_output(&f.plan, &output).is_err());
            for required in &mut f.plan.required_members {
                required.sha1 = None;
                let mut crc = crate::identity_source::hashing::Crc32::new();
                crc.update(if required.member_name == "a.bin" {
                    b"a"
                } else {
                    b"b"
                });
                required.crc32 = Some(crc.finish_hex());
            }
            zip(&output, &[("a.bin", b"a"), ("b.bin", b"x")]);
            assert!(verify_staged_output(&f.plan, &output).is_err());
            zip(&output, &[("a.bin", b"a"), ("b.bin", b"b")]);
            verify_staged_output(&f.plan, &output).unwrap();
        }
        #[test]
        fn duplicate_target_members_can_be_replaced_from_unambiguous_clean_donors() {
            let mut f = Fixture::new(&[("a.bin", b"a"), ("c.bin", b"x")]);
            let mut bytes = fs::read(&f.plan.destination).unwrap();
            for at in 0..bytes.len().saturating_sub(4) {
                if &bytes[at..at + 5] == b"c.bin" {
                    bytes[at] = b'a';
                }
            }
            fs::write(&f.plan.destination, &bytes).unwrap();
            let original = bytes;
            let donor = f.root.path().join("clean-parent.zip");
            zip(&donor, &[("a.bin", b"a")]);
            f.plan.sources[0].archive_path = donor;
            f.plan.sources[0].archive_identity =
                SourceArchiveIdentity::of(&f.plan.sources[0].archive_path);
            let review = f.review().unwrap();
            let output = review.apply(&f.stage(), &f.journal()).unwrap().unwrap();
            verify_staged_output(&f.plan, &f.plan.destination).unwrap();
            assert_eq!(
                fs::read(&output.transaction.entries[0].source_path).unwrap(),
                original
            );
        }

        #[test]
        fn bounded_family_with_large_packed_member_and_many_small_members() {
            use sha1::Digest;
            let mut f = Fixture::new(&[("a.bin", b"a")]);
            let donor = f.root.path().join("clone.zip");
            let mut writer = zip::ZipWriter::new(fs::File::create(&donor).unwrap());
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file("b.bin", options).unwrap();
            writer.write_all(b"b").unwrap();
            writer.start_file("large.bin", options).unwrap();
            let mut chunk = [0u8; 64 * 1024];
            let mut seed = 0x12345678u32;
            for byte in &mut chunk {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            let mut digest = sha1::Sha1::new();
            for _ in 0..512 {
                writer.write_all(&chunk).unwrap();
                digest.update(chunk);
            }
            let mut additions = vec![(
                "large.bin".to_owned(),
                32 * 1024 * 1024,
                digest
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            )];
            for i in 0..256u32 {
                let name = format!("small-{i:03}.bin");
                let bytes = i.to_le_bytes();
                writer.start_file(&name, options).unwrap();
                writer.write_all(&bytes).unwrap();
                additions.push((name, 4, sha(&bytes)));
            }
            writer.finish().unwrap();
            for (name, size, hash) in additions {
                f.plan
                    .required_members
                    .push(ReconstructionMemberRequirement {
                        owner_set: "clone".into(),
                        member_name: name.clone(),
                        size_bytes: Some(size),
                        sha1: Some(hash.clone()),
                        crc32: None,
                    });
                f.plan.sources.push(ReconstructionMemberSource {
                    archive_identity: None,
                    archive_path: donor.clone(),
                    member_path: PathBuf::from(&name),
                    current_name: name.clone(),
                    target_name: name,
                    observed_sha1: Some(hash),
                    observed_crc32: None,
                });
            }
            for source in &mut f.plan.sources {
                source.archive_identity = SourceArchiveIdentity::of(&source.archive_path);
            }
            let before = capture_for_test(&donor);
            let started = std::time::Instant::now();
            let reviewed = f.review().unwrap();
            reviewed.apply(&f.stage(), &f.journal()).unwrap().unwrap();
            verify_staged_output(&f.plan, &f.plan.destination).unwrap();
            assert_eq!(f.plan.required_members.len(), 259);
            assert_eq!(capture_for_test(&donor), before);
            eprintln!(
                "MAME replacement synthetic: 259 members, 32 MiB largest, elapsed {:?}",
                started.elapsed()
            );
        }

        fn capture_for_test(path: &Path) -> crate::dat::rename_apply::model::ObjectIdentity {
            crate::dat::rename_apply::identity::capture_identity(path).unwrap()
        }
    }
}
