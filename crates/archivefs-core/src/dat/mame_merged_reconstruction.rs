//! Checksum-backed MAME merged-set reconstruction.
//!
//! This is deliberately a new projection over the current MAME join/evidence
//! model.  It does not use the historical `mame_normalizer` plan, its repair
//! journal, or filename-based ownership rules.  A plan is pure data and is
//! safe to display before an explicit apply decision.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::mame_arcade_join::ArcadeJoinEvidence;
use super::model::{DatRomEntry, ParsedDat};

/// Durable journal marker used by GUI-v2 and recovery history to distinguish
/// MAME reconstruction transactions from the other rename-based workflows.
pub const MAME_RECONSTRUCTION_WORKFLOW: &str = "mame_merged_reconstruction";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconstructionMemberRequirement {
    pub owner_set: String,
    pub member_name: String,
    pub size_bytes: Option<u64>,
    pub sha1: Option<String>,
    pub crc32: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconstructionMemberSource {
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
                    archive_path: archive_path.clone(),
                    member_path: archive_path.join(&current_name),
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
    if plan
        .sources
        .iter()
        .any(|source| !source.archive_path.is_dir())
    {
        plan.reasons.push(
            "only extracted source-set directories are supported by this staged writer".into(),
        );
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

/// Writes only a staging ZIP. It never opens a source archive for writing and
/// never removes or renames a source member. Publication is intentionally a
/// separate caller action after [`verify_staged_output`].
pub fn stage_reconstruction_output(
    plan: &MameMergedReconstructionPlan,
    staging_root: &Path,
) -> Result<PathBuf, String> {
    if !plan.ready_to_apply {
        return Err("reconstruction plan is blocked; no staged output was created".into());
    }
    std::fs::create_dir_all(staging_root).map_err(|e| e.to_string())?;
    let staged = staging_root.join(format!("{}.zip.staged", plan.parent));
    let file = std::fs::File::create(&staged).map_err(|e| e.to_string())?;
    let mut writer = zip::ZipWriter::new(file);
    for source in &plan.sources {
        let bytes = std::fs::read(&source.member_path).map_err(|e| {
            format!(
                "could not read staged source member {}: {e}",
                source.member_path.display()
            )
        })?;
        writer
            .start_file(
                &source.target_name,
                zip::write::SimpleFileOptions::default(),
            )
            .map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut writer, &bytes).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())?;
    verify_staged_output(plan, &staged)?;
    Ok(staged)
}

/// Verifies the staged archive against the reviewed member list before any
/// publication. Duplicate ZIP names and checksum/size mismatches fail closed.
pub fn verify_staged_output(
    plan: &MameMergedReconstructionPlan,
    staged: &Path,
) -> Result<(), String> {
    let file = std::fs::File::open(staged).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() != plan.required_members.len() {
        return Err(format!(
            "staged output has {} members; expected {}",
            archive.len(),
            plan.required_members.len()
        ));
    }
    for requirement in &plan.required_members {
        let mut member = archive
            .by_name(&requirement.member_name)
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut member, &mut bytes).map_err(|e| e.to_string())?;
        if requirement
            .size_bytes
            .is_some_and(|size| size != bytes.len() as u64)
        {
            return Err(format!(
                "staged size mismatch for {}",
                requirement.member_name
            ));
        }
        if let Some(expected) = &requirement.sha1 {
            use sha1::Digest;
            let actual = sha1::Sha1::digest(&bytes);
            let actual = actual
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            if &actual != expected {
                return Err(format!(
                    "staged SHA-1 mismatch for {}",
                    requirement.member_name
                ));
            }
        }
    }
    Ok(())
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

    let staged = stage_reconstruction_output(plan, staging_root)?;
    let identity = capture_identity(&staged).map_err(|e| e.to_string())?;
    let destination = plan.destination.clone();
    if destination.exists() || destination.parent().is_none_or(|parent| !parent.is_dir()) {
        return Err(
            "destination collision or missing destination directory; nothing was published".into(),
        );
    }
    let source_key = staged.to_string_lossy().into_owned();
    let entry = TransactionEntry {
        source_path: staged.clone(),
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
        operation: Default::default(),
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
        use sha1::{Digest, Sha1};

        let digest = Sha1::digest(b"mame member")
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

    #[test]
    fn failed_staged_verification_never_publishes_the_destination() {
        let (mut plan, root, member_path) =
            publish_fixture("0000000000000000000000000000000000000000");
        plan.sources[0].observed_sha1 = Some("0000000000000000000000000000000000000000".into());
        let staging = root.join("staging");
        let journal = root.join("journal");
        let error = apply_staged_reconstruction_output(&plan, &staging, &journal).unwrap_err();

        assert!(error.contains("staged SHA-1 mismatch"));
        assert!(!plan.destination.exists());
        assert_eq!(std::fs::read(&member_path).unwrap(), b"mame member");
        let _ = std::fs::remove_dir_all(root);
    }
}
