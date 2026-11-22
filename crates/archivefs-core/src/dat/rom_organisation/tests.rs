//! Integration tests for canonical ROM organisation: planning, apply, rollback,
//! symlink semantics, collisions, crash recovery and cancellation.
//!
//! Every mutation test uses temporary directories only - never a real user ROM
//! directory. Planning tests snapshot the source tree and assert zero changes.

use super::transaction::apply_filesystem_organisation_transaction as apply_organisation_transaction;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::dat::classification::{
    CLASSIFIER_VERSION, ClassifierConfidence, ContentSelectionPolicy, DatContentClass,
    DatContentClassification,
};
use crate::dat::rom_organisation::*;
use crate::platform::identity::{
    PlatformIdentityConfidence, PlatformIdentityEvidence, PlatformIdentityResolution,
    PlatformIdentitySource, resolve_platform_identity,
};
use crate::safe_read::TrustedRoots;

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn cancelled() -> AtomicBool {
    AtomicBool::new(true)
}

/// No RomM mapping is available at all in these fixtures: generic
/// organisation must not need one.
fn resolved(platform: &str, source: PlatformIdentitySource) -> PlatformIdentityResolution {
    PlatformIdentityResolution::Resolved {
        generation: 1,
        platform: platform.to_string(),
        display_name: crate::platform::display_name_for(platform).to_string(),
        confidence: PlatformIdentityConfidence::High,
        evidence: vec![PlatformIdentityEvidence {
            platform: platform.to_string(),
            source,
            confidence: PlatformIdentityConfidence::High,
            generation: 1,
            detail: "test evidence".to_string(),
        }],
    }
}

fn candidate(
    dir: &Path,
    name: &str,
    resolution: PlatformIdentityResolution,
) -> OrganisationCandidate {
    let source_path = dir.join(name);
    std::fs::write(&source_path, b"fixture contents").unwrap();
    OrganisationCandidate {
        source_path,
        resolution,
        canonical_name: Some(name.to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    }
}

fn candidate_with_name(
    dir: &Path,
    name: &str,
    canonical_name: &str,
    resolution: PlatformIdentityResolution,
) -> OrganisationCandidate {
    let mut cand = candidate(dir, name, resolution);
    cand.canonical_name = Some(canonical_name.to_string());
    cand
}

fn plan_for(
    master_root: &Path,
    mode: OrganisationMode,
    candidates: &[OrganisationCandidate],
    generation: u64,
) -> OrganisationPlan {
    build_organisation_plan(&OrganisationPlanRequest {
        master_root,
        mode,
        content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
        candidates,
        generation,
        unknown_folder: None,
        link_target_roots: None,
    })
}

/// Reorganise existing links, with these approved library folders.
fn plan_with_link_roots(
    master_root: &Path,
    candidates: &[OrganisationCandidate],
    roots: &[PathBuf],
) -> OrganisationPlan {
    build_organisation_plan(&OrganisationPlanRequest {
        master_root,
        mode: OrganisationMode::OrganiseSymlinkOnly,
        content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
        candidates,
        generation: 1,
        unknown_folder: None,
        link_target_roots: Some(roots),
    })
}

fn apply_plan(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> crate::dat::rename_apply::executor::ApplyOutcome {
    // A configured master root exists (the user created it); the platform
    // directory does not yet exist and is what apply creates.
    std::fs::create_dir_all(&plan.master_root).unwrap();
    let mut tx =
        build_organisation_transaction(plan, approved, plan.generation).expect("build transaction");
    let mut trusted_roots =
        vec![std::fs::canonicalize(&plan.master_root).unwrap_or_else(|_| plan.master_root.clone())];
    for entry in plan.suggested() {
        if let Some(parent) = entry.source_path.parent()
            && let Ok(canonical) = std::fs::canonicalize(parent)
        {
            trusted_roots.push(canonical);
        }
    }
    apply_organisation_transaction(
        &mut tx,
        approved,
        plan.generation,
        TrustedRoots::from_paths(trusted_roots),
        journal_dir,
        cancel,
        plan.mode,
        &plan.master_root,
    )
    .expect("apply")
}

fn approved_of(sources: &[&Path]) -> BTreeSet<String> {
    sources
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// Destination derivation and platform safety
// ---------------------------------------------------------------------------

#[test]
fn psp_identity_proposes_master_root_psp_name() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Lumines.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let entry = &plan.entries[0];
    assert_eq!(entry.status, OrganisationStatus::Suggested);
    assert_eq!(
        entry.destination_path,
        master.join("Sony PlayStation Portable").join("Lumines.iso")
    );
    assert_eq!(
        entry.layout_folder.as_deref(),
        Some("Sony PlayStation Portable")
    );
    // Generic organisation never consults RomM: the mapping fact stays empty.
    assert_eq!(entry.slug.as_deref(), None);
}

#[test]
fn neutral_registry_folder_is_used_without_any_romm_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    // The registry display name is "Sony PlayStation Portable"; generic
    // organisation uses it directly, with no RomM cache in sight.
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(
        plan.entries[0]
            .destination_path
            .parent()
            .unwrap()
            .file_name()
            .unwrap(),
        "Sony PlayStation Portable"
    );
}

#[test]
fn verified_dat_and_manual_and_romm_all_map_to_the_same_neutral_folder() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    for source_kind in [
        PlatformIdentitySource::VerifiedDat,
        PlatformIdentitySource::Manual,
        PlatformIdentitySource::Romm,
    ] {
        let name = format!("g-{:?}.iso", source_kind);
        let cand = candidate(&source, &name, resolved("PSP", source_kind));
        let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
        assert_eq!(
            plan.entries[0]
                .destination_path
                .parent()
                .unwrap()
                .file_name()
                .unwrap(),
            "Sony PlayStation Portable",
            "provenance {source_kind:?} must use the same neutral EmuWiz folder"
        );
        assert_eq!(
            plan.entries[0].platform_source,
            source_kind.label().to_string()
        );
    }
}

#[test]
fn unknown_platform_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        PlatformIdentityResolution::Unknown { generation: 1 },
    );
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
    assert!(plan.entries[0].reason.is_some());
}

#[test]
fn platform_conflict_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        PlatformIdentityResolution::Conflict {
            generation: 1,
            evidence: Vec::new(),
        },
    );
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

#[test]
fn missing_romm_mapping_does_not_block_generic_organisation() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    // Atari 2600 has no RomM slug mapping anywhere in this fixture - and no
    // RomM cache is even constructed. Generic organisation must still work,
    // using the neutral EmuWiz registry folder.
    let cand = candidate(
        &source,
        "Game.bin",
        resolved("Atari2600", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert_eq!(
        plan.entries[0].destination_path,
        master.join("Atari 2600").join("Game.bin")
    );
    assert_eq!(plan.entries[0].layout_folder.as_deref(), Some("Atari 2600"));
    assert_eq!(plan.entries[0].slug.as_deref(), None);
}

#[test]
fn rename_in_place_stays_in_the_current_directory() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::RenameInPlace,
        std::slice::from_ref(&cand),
        1,
    );
    let entry = &plan.entries[0];
    assert_eq!(
        entry.destination_path,
        source.join("Game.iso"),
        "rename in place must stay in the source directory"
    );
    assert!(
        entry.layout_folder.is_none(),
        "rename in place needs no platform folder"
    );
}

#[test]
fn move_mode_proposes_the_canonical_directory() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.nds",
        resolved("Nintendo DS", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(
        plan.entries[0].destination_path,
        master.join("Nintendo DS").join("Game.nds")
    );
}

#[test]
fn already_organised_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let organised = master.join("Sony PlayStation Portable");
    std::fs::create_dir_all(&organised).unwrap();
    let cand = candidate(
        &organised,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::AlreadyOrganised);
}

// ---------------------------------------------------------------------------
// Collisions
// ---------------------------------------------------------------------------

#[test]
fn existing_destination_is_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    // The destination already exists.
    let psp = master.join("Sony PlayStation Portable");
    std::fs::create_dir_all(&psp).unwrap();
    std::fs::write(psp.join("Game.iso"), b"taken").unwrap();
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Conflict);
}

#[test]
fn case_only_destination_is_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let psp = master.join("Sony PlayStation Portable");
    std::fs::create_dir_all(&psp).unwrap();
    std::fs::write(psp.join("game.iso"), b"case twin").unwrap();
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Conflict);
}

#[test]
fn two_plans_targeting_one_destination_are_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    // Two candidates that derive to the same canonical name.
    let a = candidate(
        &source,
        "A.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let b = candidate(
        &source,
        "B.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let mut candidates = vec![a, b];
    candidates[0].canonical_name = Some("Same.iso".to_string());
    candidates[1].canonical_name = Some("Same.iso".to_string());
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &candidates, 1);
    assert!(
        plan.entries
            .iter()
            .all(|e| e.status == OrganisationStatus::Conflict)
    );
}

// ---------------------------------------------------------------------------
// Planning is read-only
// ---------------------------------------------------------------------------

#[test]
fn planning_creates_no_directories_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let before: Vec<PathBuf> = collect_tree(dir.path());
    let _ = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert!(!master.exists(), "planning must not create the master root");
    assert!(
        !master.join("Sony PlayStation Portable").exists(),
        "planning must not create the platform directory"
    );
    assert_eq!(collect_tree(dir.path()), before, "planning changes nothing");
}

fn collect_tree(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    queue.push(path.clone());
                }
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Apply and rollback
// ---------------------------------------------------------------------------

#[test]
fn same_filesystem_real_file_move_succeeds_and_preserves_content() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(&plan, &approved_of(&[&cand.source_path]), &journal, &cancel);
    assert_eq!(
        outcome.transaction.state,
        crate::dat::rename_apply::TransactionState::Applied
    );
    assert!(!cand.source_path.exists(), "source moved away");
    let dest = master.join("Sony PlayStation Portable").join("Game.iso");
    assert!(dest.exists());
    assert_eq!(std::fs::read(&dest).unwrap(), b"fixture contents");
}

#[test]
fn rollback_restores_original_path_and_content() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(&plan, &approved_of(&[&cand.source_path]), &journal, &cancel);
    let mut tx = outcome.transaction;
    let rollback = rollback_organisation_transaction(&mut tx, &journal, &cancel, &master).unwrap();
    assert!(cand.source_path.exists(), "source path restored");
    assert_eq!(
        std::fs::read(&cand.source_path).unwrap(),
        b"fixture contents"
    );
    assert!(
        !master
            .join("Sony PlayStation Portable")
            .join("Game.iso")
            .exists()
    );
    assert!(
        rollback
            .directories_removed
            .contains(&master.join("Sony PlayStation Portable")),
        "the created platform directory is removed when empty: {:?}",
        rollback.directories_removed
    );
}

#[test]
fn cross_filesystem_move_is_refused_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let source_file = source.join("Game.iso");
    std::fs::write(&source_file, b"fixture contents").unwrap();

    // `/proc` is a different filesystem than any temp directory on Linux.
    let Some(master) = different_filesystem_root(dir.path()) else {
        return; // environment has no second filesystem; the helper is covered below
    };
    let master = master.join("roms");
    let cand = OrganisationCandidate {
        source_path: source_file.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: Some("Game.iso".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    // apply_plan_result creates master_root; for the /proc master root that
    // would fail, so drive apply directly and assert refusal.
    let mut tx =
        build_organisation_transaction(&plan, &approved_of(&[&source_file]), 1).expect("build");
    let cancel = no_cancel();
    let result = apply_organisation_transaction(
        &mut tx,
        &approved_of(&[&source_file]),
        1,
        TrustedRoots::from_paths([dir.path()]),
        &journal,
        &cancel,
        plan.mode,
        &plan.master_root,
    );
    assert!(result.is_err(), "a cross-filesystem move must be refused");
    assert!(source_file.exists(), "the source is never touched");
}

fn apply_plan_result(
    plan: &OrganisationPlan,
    approved: &BTreeSet<String>,
    journal_dir: &Path,
    cancel: &AtomicBool,
) -> Result<
    crate::dat::rename_apply::executor::ApplyOutcome,
    crate::dat::rename_apply::executor::ApplyError,
> {
    std::fs::create_dir_all(&plan.master_root).unwrap();
    let mut tx =
        build_organisation_transaction(plan, approved, plan.generation).expect("build transaction");
    let mut trusted_roots =
        vec![std::fs::canonicalize(&plan.master_root).unwrap_or_else(|_| plan.master_root.clone())];
    for entry in plan.suggested() {
        if let Some(parent) = entry.source_path.parent()
            && let Ok(canonical) = std::fs::canonicalize(parent)
        {
            trusted_roots.push(canonical);
        }
    }
    apply_organisation_transaction(
        &mut tx,
        approved,
        plan.generation,
        TrustedRoots::from_paths(trusted_roots),
        journal_dir,
        cancel,
        plan.mode,
        &plan.master_root,
    )
}

/// A root on a different filesystem than `dir` (Linux: procfs), used to prove
/// a cross-filesystem move is refused. Returns `None` when no second
/// filesystem is observable.
fn different_filesystem_root(dir: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dir_dev = std::fs::metadata(dir).ok()?.dev();
        let proc = Path::new("/proc");
        let proc_dev = std::fs::metadata(proc).ok()?.dev();
        if proc_dev != dir_dev {
            return Some(proc.to_path_buf());
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

#[test]
fn apply_creates_only_the_canonical_platform_directory() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&master).unwrap();
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let _ = apply_plan(&plan, &approved_of(&[&cand.source_path]), &journal, &cancel);
    assert!(master.join("Sony PlayStation Portable").is_dir());
    let children: Vec<String> = std::fs::read_dir(&master)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        children,
        vec!["Sony PlayStation Portable"],
        "only the canonical platform dir is created"
    );
}

#[test]
fn rollback_never_removes_a_pre_existing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    // The platform directory already exists and contains a user file.
    let psp = master.join("Sony PlayStation Portable");
    std::fs::create_dir_all(&psp).unwrap();
    std::fs::write(psp.join("user-note.txt"), b"mine").unwrap();

    let _cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    // Destination is occupied -> plan Conflict, not suggested, so nothing is
    // built; but to exercise the rollback path, apply a different plan into a
    // fresh platform dir and check a pre-existing sibling is untouched.
    let cand2 = candidate(
        &source,
        "Other.iso",
        resolved("Switch", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand2),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(
        &plan,
        &approved_of(&[&cand2.source_path]),
        &journal,
        &cancel,
    );
    let mut tx = outcome.transaction;
    let _ = rollback_organisation_transaction(&mut tx, &journal, &cancel, &master).unwrap();
    assert!(
        psp.exists(),
        "the pre-existing directory must never be removed"
    );
    assert_eq!(std::fs::read(psp.join("user-note.txt")).unwrap(), b"mine");
    assert!(
        !master.join("Nintendo Switch").exists(),
        "the created dir is removed"
    );
}

#[test]
fn source_identity_changed_is_rejected_at_apply() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    // Build the transaction (identity snapshot) then change the source.
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    std::fs::create_dir_all(&master).unwrap();
    let mut tx = build_organisation_transaction(&plan, &approved_of(&[&cand.source_path]), 1)
        .expect("build");
    std::fs::write(&cand.source_path, b"different bytes").unwrap();
    let cancel = no_cancel();
    let mut trusted_roots = vec![std::fs::canonicalize(&master).unwrap_or(master.clone())];
    if let Some(parent) = cand.source_path.parent()
        && let Ok(c) = std::fs::canonicalize(parent)
    {
        trusted_roots.push(c);
    }
    let result = apply_organisation_transaction(
        &mut tx,
        &approved_of(&[&cand.source_path]),
        1,
        TrustedRoots::from_paths(trusted_roots),
        &journal,
        &cancel,
        plan.mode,
        &plan.master_root,
    );
    assert!(result.is_err());
    assert!(cand.source_path.exists());
    assert_eq!(
        std::fs::read(&cand.source_path).unwrap(),
        b"different bytes"
    );
    assert!(
        !master.join("Sony PlayStation Portable").exists(),
        "no mutation happened"
    );
}

#[test]
fn destination_created_after_preview_is_rejected_at_apply() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    std::fs::create_dir_all(&master).unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let mut tx = build_organisation_transaction(&plan, &approved_of(&[&cand.source_path]), 1)
        .expect("build");
    // The destination file appears after the preview.
    let psp = master.join("Sony PlayStation Portable");
    std::fs::create_dir_all(&psp).unwrap();
    std::fs::write(psp.join("Game.iso"), b"sneaky").unwrap();
    let mut trusted_roots = vec![std::fs::canonicalize(&master).unwrap_or(master.clone())];
    if let Some(parent) = cand.source_path.parent()
        && let Ok(c) = std::fs::canonicalize(parent)
    {
        trusted_roots.push(c);
    }
    let result = apply_organisation_transaction(
        &mut tx,
        &approved_of(&[&cand.source_path]),
        1,
        TrustedRoots::from_paths(trusted_roots),
        &journal,
        &cancel,
        plan.mode,
        &plan.master_root,
    );
    assert!(
        result.is_err(),
        "an appearing destination must abort the batch"
    );
    assert_eq!(
        std::fs::read(psp.join("Game.iso")).unwrap(),
        b"sneaky",
        "the appearing file is never overwritten"
    );
}

#[test]
fn stale_generation_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    // Apply against a newer generation: stale.
    std::fs::create_dir_all(&master).unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let mut tx = build_organisation_transaction(&plan, &approved_of(&[&cand.source_path]), 1)
        .expect("build");
    let cancel = no_cancel();
    let mut trusted_roots = vec![std::fs::canonicalize(&master).unwrap_or(master.clone())];
    if let Some(parent) = cand.source_path.parent()
        && let Ok(c) = std::fs::canonicalize(parent)
    {
        trusted_roots.push(c);
    }
    let result = apply_organisation_transaction(
        &mut tx,
        &approved_of(&[&cand.source_path]),
        2, // current generation is 2, plan was generation 1
        TrustedRoots::from_paths(trusted_roots),
        &journal,
        &cancel,
        plan.mode,
        &plan.master_root,
    );
    assert!(result.is_err(), "a stale plan must never apply");
}

// ---------------------------------------------------------------------------
// Symlink semantics
// ---------------------------------------------------------------------------

#[test]
fn symlink_object_move_preserves_the_target_text_and_never_dereferences() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let real_target = dir.path().join("elsewhere").join("real.iso");
    std::fs::create_dir_all(real_target.parent().unwrap()).unwrap();
    std::fs::write(&real_target, b"real content").unwrap();
    let link = source.join("Game.iso");
    std::os::unix::fs::symlink(&real_target, &link).unwrap();

    let cand = OrganisationCandidate {
        source_path: link.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: Some("Game.iso".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_with_link_roots(
        &master,
        std::slice::from_ref(&cand),
        &[std::fs::canonicalize(real_target.parent().unwrap()).unwrap()],
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(&plan, &approved_of(&[&link]), &journal, &cancel);
    assert_eq!(
        outcome.transaction.state,
        crate::dat::rename_apply::TransactionState::Applied
    );
    let moved = master.join("Sony PlayStation Portable").join("Game.iso");
    assert!(moved.symlink_metadata().is_ok(), "the link object moved");
    assert_eq!(
        std::fs::read_link(&moved).unwrap(),
        real_target,
        "the link target text is preserved exactly"
    );
    assert!(
        real_target.exists(),
        "the target is never dereferenced or moved"
    );
    assert_eq!(std::fs::read(&real_target).unwrap(), b"real content");
}

#[test]
fn symlink_only_mode_rejects_a_regular_file_source() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(&master, OrganisationMode::OrganiseSymlinkOnly, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
    assert!(plan.entries[0].source_path.exists());
}

#[test]
fn move_real_file_mode_rejects_a_symlink_source() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let target = dir.path().join("real.bin");
    std::fs::write(&target, b"real").unwrap();
    let link = source.join("Game.iso");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let cand = OrganisationCandidate {
        source_path: link.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: Some("Game.iso".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

#[test]
fn a_broken_existing_link_is_refused_and_left_as_it_is() {
    // Owner policy 2026-11-22: a broken link has no game file to be named
    // from, so it is refused (it used to be moved).
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let link = source.join("Game.iso");
    std::os::unix::fs::symlink(dir.path().join("nowhere.bin"), &link).unwrap();
    let cand = OrganisationCandidate {
        source_path: link.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: Some("Game.iso".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::OrganiseSymlinkOnly, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
    assert!(
        plan.entries[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("broken")
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        dir.path().join("nowhere.bin")
    );
}

#[test]
fn a_link_to_a_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let target_dir = dir.path().join("target-dir");
    std::fs::create_dir_all(&target_dir).unwrap();
    let link = source.join("Game.iso");
    std::os::unix::fs::symlink(&target_dir, &link).unwrap();
    let cand = OrganisationCandidate {
        source_path: link.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: Some("Game.iso".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::OrganiseSymlinkOnly, &[cand], 1);
    assert_eq!(
        plan.entries[0].status,
        OrganisationStatus::Blocked,
        "a link to a folder has no game file to be named from (owner policy 2026-11-22)"
    );
    assert!(target_dir.exists(), "the target directory is never touched");
}

#[test]
fn a_directory_source_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let rom_dir = source.join("GameFolder");
    std::fs::create_dir_all(&rom_dir).unwrap();
    let cand = OrganisationCandidate {
        source_path: rom_dir.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: Some("GameFolder".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

// ---------------------------------------------------------------------------
// Cancellation, crash recovery
// ---------------------------------------------------------------------------

#[test]
fn cancellation_before_first_mutation_moves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let _cancel = cancelled();
    let result = apply_plan_result(
        &plan,
        &approved_of(&[&cand.source_path]),
        &journal,
        &_cancel,
    );
    assert!(result.is_err());
    assert!(cand.source_path.exists());
    assert!(
        !master.join("Sony PlayStation Portable").exists(),
        "no platform directory was created"
    );
    assert!(
        !master
            .join("Sony PlayStation Portable")
            .join("Game.iso")
            .exists(),
        "no file was moved"
    );
}

#[test]
fn no_filesystem_escape_from_the_master_root() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    // A hostile candidate whose canonical name attempts a traversal. The
    // derive step blocks it; the destination stays inside the master root.
    let mut cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    cand.canonical_name = Some("../escape.iso".to_string());
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_ne!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert!(
        !dir.path().join("escape.iso").exists(),
        "a traversal name must never produce a file"
    );
    assert!(
        !master.join("..").join("escape.iso").exists(),
        "destination must never escape the master root"
    );
}

// ---------------------------------------------------------------------------
// Canonical-name supply (blocker: GUI never supplied canonical filenames)
// ---------------------------------------------------------------------------

#[test]
fn a_verified_canonical_name_produces_the_canonical_destination_filename() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let source_path = source.join("Game_ugly.iso");
    std::fs::write(&source_path, b"data").unwrap();
    let mut cand = OrganisationCandidate {
        source_path: source_path.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: None,
        content_classification: None,
        original_metadata: Default::default(),
    };
    // Ugly source name + authoritative canonical name.
    cand.canonical_name = Some("Game (Europe).iso".to_string());
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let entry = &plan.entries[0];
    assert_eq!(entry.status, OrganisationStatus::Suggested);
    assert_eq!(
        entry.destination_path,
        master
            .join("Sony PlayStation Portable")
            .join("Game (Europe).iso"),
        "the canonical name must drive the destination filename"
    );
}

#[test]
fn rename_in_place_proposes_a_rename_when_the_canonical_name_differs() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let source_path = source.join("game_ugly.iso");
    std::fs::write(&source_path, b"data").unwrap();
    let mut cand = OrganisationCandidate {
        source_path: source_path.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: None,
        content_classification: None,
        original_metadata: Default::default(),
    };
    cand.canonical_name = Some("Game (Europe).iso".to_string());
    let plan = plan_for(
        &master,
        OrganisationMode::RenameInPlace,
        std::slice::from_ref(&cand),
        1,
    );
    let entry = &plan.entries[0];
    assert_eq!(
        entry.status,
        OrganisationStatus::Suggested,
        "a differing canonical name must propose a rename, not read as already organised"
    );
    assert_eq!(entry.destination_path, source.join("Game (Europe).iso"));
}

#[test]
fn move_mode_uses_the_canonical_filename() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let source_path = source.join("whatever.nds");
    std::fs::write(&source_path, b"data").unwrap();
    let mut cand = OrganisationCandidate {
        source_path: source_path.clone(),
        resolution: resolved("Nintendo DS", PlatformIdentitySource::Romm),
        canonical_name: None,
        content_classification: None,
        original_metadata: Default::default(),
    };
    cand.canonical_name = Some("Sonic Rush (Europe).nds".to_string());
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(
        plan.entries[0].destination_path,
        master.join("Nintendo DS").join("Sonic Rush (Europe).nds")
    );
}

#[test]
fn no_canonical_evidence_falls_back_to_the_source_basename() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let source_path = source.join("Game_ugly.iso");
    std::fs::write(&source_path, b"data").unwrap();
    let cand = OrganisationCandidate {
        source_path: source_path.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: None,
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(
        plan.entries[0].destination_path,
        master
            .join("Sony PlayStation Portable")
            .join("Game_ugly.iso"),
        "without a canonical name the existing basename is preserved"
    );
}

#[test]
fn an_unverified_identity_never_supplies_an_authoritative_name() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let source_path = source.join("Game.iso");
    std::fs::write(&source_path, b"data").unwrap();
    // An Unknown identity with a (suspiciously supplied) canonical name must
    // still be Blocked - the name never rescues an unverified platform.
    let cand = OrganisationCandidate {
        source_path: source_path.clone(),
        resolution: PlatformIdentityResolution::Unknown { generation: 1 },
        canonical_name: Some("Trusted (USA).iso".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

// ---------------------------------------------------------------------------
// Directory ownership (blocker: pre-existing platform dir could be journalled
// as EmuWiz-owned and removed by rollback)
// ---------------------------------------------------------------------------

#[test]
fn a_pre_existing_empty_platform_directory_is_never_recorded_as_owned() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&master).unwrap();
    // The platform directory already exists (empty).
    let psp = master.join("Sony PlayStation Portable");
    std::fs::create_dir(&psp).unwrap();
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(&plan, &approved_of(&[&cand.source_path]), &journal, &cancel);
    assert!(
        !outcome.transaction.created_directories.contains(&psp),
        "a pre-existing directory must never be recorded as owned: {:?}",
        outcome.transaction.created_directories
    );
    assert!(psp.exists(), "the pre-existing directory stays");

    // Rollback (and recovery, which relies on created_directories) never
    // removes it.
    let mut tx = outcome.transaction;
    let _ = rollback_organisation_transaction(&mut tx, &journal, &cancel, &master).unwrap();
    assert!(
        psp.exists(),
        "rollback never removes a pre-existing directory"
    );
}

#[test]
fn a_newly_created_platform_directory_is_recorded_as_owned_and_removed_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&master).unwrap();
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(&plan, &approved_of(&[&cand.source_path]), &journal, &cancel);
    let psp = master.join("Sony PlayStation Portable");
    assert!(
        outcome.transaction.created_directories.contains(&psp),
        "a directory this apply created must be recorded as owned"
    );
    let mut tx = outcome.transaction;
    let rollback = rollback_organisation_transaction(&mut tx, &journal, &cancel, &master).unwrap();
    assert!(
        rollback.directories_removed.contains(&psp),
        "the owned, now-empty directory is removed: {:?}",
        rollback.directories_removed
    );
    assert!(!psp.exists());
}

#[test]
fn two_files_sharing_one_created_platform_directory_own_it_once() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&master).unwrap();
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let a = candidate(
        &source,
        "A.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let b = candidate(
        &source,
        "B.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        &[a.clone(), b.clone()],
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(
        &plan,
        &approved_of(&[&a.source_path, &b.source_path]),
        &journal,
        &cancel,
    );
    let psp = master.join("Sony PlayStation Portable");
    let owned = outcome
        .transaction
        .created_directories
        .iter()
        .filter(|dir| **dir == psp)
        .count();
    assert_eq!(owned, 1, "one shared directory is owned once");
    let mut tx = outcome.transaction;
    let rollback = rollback_organisation_transaction(&mut tx, &journal, &cancel, &master).unwrap();
    assert!(rollback.directories_removed.contains(&psp));
    assert!(!psp.exists());
}

#[test]
fn partial_rollback_leaves_a_non_empty_directory_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&master).unwrap();
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let cancel = no_cancel();
    let outcome = apply_plan(&plan, &approved_of(&[&cand.source_path]), &journal, &cancel);
    // A user file appears in the created directory after apply.
    let psp = master.join("Sony PlayStation Portable");
    std::fs::write(psp.join("user-note.txt"), b"mine").unwrap();
    let mut tx = outcome.transaction;
    let rollback = rollback_organisation_transaction(&mut tx, &journal, &cancel, &master).unwrap();
    assert!(
        rollback.directories_remaining.contains(&psp),
        "a non-empty owned directory is reported as remaining: {:?}",
        rollback.directories_remaining
    );
    assert!(psp.exists());
    assert_eq!(std::fs::read(psp.join("user-note.txt")).unwrap(), b"mine");
}

#[test]
fn an_old_journal_without_the_created_directories_field_loads_safely() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    // An old-format journal with no created_directories key.
    std::fs::write(
        journal.join("old-1.json"),
        r#"{
  "transaction_id": "old-1",
  "plan_generation": 1,
  "created_at_unix": 1,
  "source_scan_root": "/tmp/roms",
  "state": "applying",
  "entries": []
}
"#,
    )
    .unwrap();
    let loaded =
        crate::dat::rename_apply::journal::read_journal(&journal.join("old-1.json")).unwrap();
    assert!(
        loaded.created_directories.is_empty(),
        "a legacy journal must default to no owned directories"
    );
}

#[test]
fn a_crash_between_create_dir_and_ownership_journal_is_conservative() {
    // Simulate: a platform directory exists but the journal never recorded it
    // as owned (the crash window between create_dir and the ownership write).
    // Recovery/rollback must not delete it.
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&master).unwrap();
    let psp = master.join("Sony PlayStation Portable");
    std::fs::create_dir(&psp).unwrap();
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.iso",
        resolved("PSP", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::MoveRealFile,
        std::slice::from_ref(&cand),
        1,
    );
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let mut tx =
        build_organisation_transaction(&plan, &approved_of(&[&cand.source_path]), 1).unwrap();
    // The journal that was durable before the ownership write: created_directories empty.
    tx.state = crate::dat::rename_apply::TransactionState::Applying;
    crate::dat::rename_apply::journal::write_journal(&journal, &tx).unwrap();
    assert!(tx.created_directories.is_empty());

    let cancel = no_cancel();
    let _ = rollback_organisation_transaction(&mut tx, &journal, &cancel, &master).unwrap();
    assert!(
        psp.exists(),
        "an unproven (un-journalled) directory is never deleted by recovery"
    );
}

// ---------------------------------------------------------------------------
// Live identity revalidation (blocker: stale platform identity not revalidated)
// ---------------------------------------------------------------------------

use crate::database::Database;

/// Creates a database with `file` registered under its parent folder and a
/// platform assignment, returning the open handle (assignments must be made
/// while the mutable handle is alive).
fn db_with_assignment(dir: &Path, file: &Path, platform: &str, source: &str) -> Database {
    let db_path = dir.join("test.db");
    let mut db = Database::open_or_create(&db_path).unwrap();
    let folder = file.parent().unwrap();
    let registered = db.register_source_folders(&[folder.to_path_buf()]).unwrap();
    let archive = crate::Archive::from_path(file).unwrap();
    db.upsert_archive(registered[0].id, folder, &archive)
        .unwrap();
    let archive_id = db
        .find_archive_id_by_absolute_path(file)
        .unwrap()
        .expect("archive registered");
    db.assign_platform(archive_id, Some(platform), source)
        .unwrap();
    db
}

fn resolution_from_db(db: &Database, file: &Path, generation: u64) -> PlatformIdentityResolution {
    let archive_id = db.find_archive_id_by_absolute_path(file).unwrap().unwrap();
    let evidence = db
        .current_platform_identity_evidence(archive_id, generation)
        .unwrap();
    resolve_platform_identity(generation, evidence)
}

fn plan_from_live_db(
    dir: &Path,
    file: &Path,
    db: &Database,
    mode: OrganisationMode,
    generation: u64,
) -> OrganisationPlan {
    let master = dir.join("roms");
    std::fs::create_dir_all(&master).unwrap();
    let cand = OrganisationCandidate {
        source_path: file.to_path_buf(),
        resolution: resolution_from_db(db, file, generation),
        canonical_name: None,
        content_classification: None,
        original_metadata: Default::default(),
    };
    plan_for(&master, mode, &[cand], generation)
}

#[test]
fn a_platform_changed_by_another_process_rejects_the_stale_apply() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("library");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_file = source_dir.join("Game.iso");
    std::fs::write(&source_file, b"data").unwrap();

    let mut db = db_with_assignment(dir.path(), &source_file, "PSP", "romm");
    let plan = plan_from_live_db(
        dir.path(),
        &source_file,
        &db,
        OrganisationMode::MoveRealFile,
        1,
    );
    assert_eq!(plan.entries[0].platform.as_deref(), Some("PSP"));
    assert_eq!(
        plan.entries[0].layout_folder.as_deref(),
        Some("Sony PlayStation Portable")
    );

    // Another EmuWiz process changes PSP -> PS1 after planning.
    let archive_id = db
        .find_archive_id_by_absolute_path(&source_file)
        .unwrap()
        .unwrap();
    db.assign_platform(archive_id, Some("PS1"), "romm").unwrap();
    drop(db);

    let master = dir.path().join("roms");
    let error =
        revalidate_organisation_plan(&plan, &dir.path().join("test.db"), &|_| None).unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert!(source_file.exists(), "zero mutation");
    assert!(
        !master.join("Sony PlayStation Portable").exists(),
        "zero mutation"
    );
}

#[test]
fn a_resolved_identity_becoming_unresolvable_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("library");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_file = source_dir.join("Game.iso");
    std::fs::write(&source_file, b"data").unwrap();

    let mut db = db_with_assignment(dir.path(), &source_file, "PSP", "romm");
    let plan = plan_from_live_db(
        dir.path(),
        &source_file,
        &db,
        OrganisationMode::MoveRealFile,
        1,
    );

    // Another process replaces the assignment with text that cannot resolve
    // to a canonical platform: the identity becomes Unknown.
    let archive_id = db
        .find_archive_id_by_absolute_path(&source_file)
        .unwrap()
        .unwrap();
    db.assign_platform(archive_id, Some("not-a-registered-platform"), "romm")
        .unwrap();
    drop(db);
    let error =
        revalidate_organisation_plan(&plan, &dir.path().join("test.db"), &|_| None).unwrap_err();
    assert!(error.contains("changed"), "{error}");
}

#[test]
fn a_changed_romm_mapping_does_not_invalidate_a_generic_plan() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("library");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_file = source_dir.join("Game.iso");
    std::fs::write(&source_file, b"data").unwrap();

    let db = db_with_assignment(dir.path(), &source_file, "PSP", "romm");
    let plan = plan_from_live_db(
        dir.path(),
        &source_file,
        &db,
        OrganisationMode::MoveRealFile,
        1,
    );
    assert_eq!(
        plan.entries[0].layout_folder.as_deref(),
        Some("Sony PlayStation Portable")
    );

    // A RomM mapping change is a RomM-specific fact. Generic plans derive
    // destinations from the neutral EmuWiz folder, so revalidation must NOT
    // consult any RomM mapping and must still accept the plan.
    let changed_slug = |platform: &str| -> Option<String> {
        if platform == "PSP" {
            Some("psp-portable".to_string())
        } else {
            None
        }
    };
    let _ = changed_slug; // the new revalidate signature takes no slug source
    revalidate_organisation_plan(&plan, &dir.path().join("test.db"), &|_| None)
        .expect("a generic plan is not affected by a RomM mapping change");
}

#[test]
fn a_changed_canonical_name_is_rejected_when_the_destination_changes() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("library");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_file = source_dir.join("Game_ugly.iso");
    std::fs::write(&source_file, b"data").unwrap();

    let db = db_with_assignment(dir.path(), &source_file, "PSP", "romm");
    // Plan with a canonical name.
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&master).unwrap();
    let cand = OrganisationCandidate {
        source_path: source_file.clone(),
        resolution: resolution_from_db(&db, &source_file, 1),
        canonical_name: Some("Game (Europe).iso".to_string()),
        content_classification: None,
        original_metadata: Default::default(),
    };
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(
        plan.entries[0].destination_path,
        master
            .join("Sony PlayStation Portable")
            .join("Game (Europe).iso")
    );

    // The authoritative canonical name changes between plan and apply.
    let changed_name = |path: &Path| -> Option<String> {
        if path == source_file {
            Some("Game (Japan).iso".to_string())
        } else {
            None
        }
    };
    let error = revalidate_organisation_plan(&plan, &dir.path().join("test.db"), &changed_name)
        .unwrap_err();
    assert!(error.contains("changed"), "{error}");
}

#[test]
fn an_unchanged_live_identity_passes_revalidation() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("library");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_file = source_dir.join("Game.iso");
    std::fs::write(&source_file, b"data").unwrap();

    let db = db_with_assignment(dir.path(), &source_file, "PSP", "romm");
    let plan = plan_from_live_db(
        dir.path(),
        &source_file,
        &db,
        OrganisationMode::MoveRealFile,
        1,
    );
    drop(db);
    revalidate_organisation_plan(&plan, &dir.path().join("test.db"), &|_| None)
        .expect("an unchanged live identity must pass");
}

#[test]
fn games_only_organisation_blocks_unknown_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    let master = dir.path().join("roms");
    std::fs::create_dir_all(&library).unwrap();
    let source = library.join("Game.iso");
    std::fs::write(&source, b"data").unwrap();
    let candidate = OrganisationCandidate {
        source_path: source.clone(),
        resolution: resolved("PSP", PlatformIdentitySource::Romm),
        canonical_name: Some("Game (Europe).iso".to_string()),
        content_classification: Some(DatContentClassification::unknown()),
        original_metadata: Default::default(),
    };
    let plan = build_organisation_plan(&OrganisationPlanRequest {
        master_root: &master,
        mode: OrganisationMode::MoveRealFile,
        content_policy: ContentSelectionPolicy::GamesOnly,
        candidates: &[candidate],
        generation: 1,
        unknown_folder: None,
        link_target_roots: None,
    });
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
    assert!(
        plan.entries[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("Unknown")
    );
    assert!(source.exists());
    assert!(!master.exists());
}

#[test]
fn games_only_organisation_allows_confirmed_game_classes() {
    for class in [
        DatContentClass::Game,
        DatContentClass::GameCompilation,
        DatContentClass::RequiredMultidiscPart,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        let master = dir.path().join("roms");
        std::fs::create_dir_all(&library).unwrap();
        let source = library.join("Game.iso");
        std::fs::write(&source, b"data").unwrap();
        let candidate = OrganisationCandidate {
            source_path: source,
            resolution: resolved("PSP", PlatformIdentitySource::Romm),
            canonical_name: Some("Game (Europe).iso".to_string()),
            content_classification: Some(DatContentClassification {
                class,
                confidence: ClassifierConfidence::High,
                evidence: Vec::new(),
                classifier_version: CLASSIFIER_VERSION.to_string(),
            }),
            original_metadata: Default::default(),
        };
        let plan = build_organisation_plan(&OrganisationPlanRequest {
            master_root: &master,
            mode: OrganisationMode::MoveRealFile,
            content_policy: ContentSelectionPolicy::GamesOnly,
            candidates: &[candidate],
            generation: 1,
            unknown_folder: None,
            link_target_roots: None,
        });
        assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    }
}

// ---------------------------------------------------------------------------
// Neutral EmuWiz platform layout identity (RomM-independent organisation)
// ---------------------------------------------------------------------------

#[test]
fn neutral_platform_layout_identity_is_stable_safe_and_romm_independent() {
    // Atari 2600's registry display name is the layout folder.
    assert_eq!(
        crate::platform::canonical_layout_folder("Atari2600"),
        Some("Atari 2600".to_string())
    );
    // Deterministic: pure registry lookup, no cache, no I/O, no RomM.
    assert_eq!(
        crate::platform::canonical_layout_folder("Atari2600"),
        crate::platform::canonical_layout_folder("Atari2600")
    );
    // Safe as a single path component.
    let folder = crate::platform::canonical_layout_folder("Atari2600").unwrap();
    assert!(!folder.is_empty());
    assert!(!folder.contains(['/', '\\', '\0']));
    assert_eq!(folder.trim(), folder);
    // An unregistered id has no invented identity.
    assert_eq!(
        crate::platform::canonical_layout_folder("not-a-platform"),
        None
    );
}

#[test]
fn rename_in_place_needs_no_romm_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate_with_name(
        &source,
        "Combat.bin",
        "Combat (USA).bin",
        resolved("Atari2600", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(&master, OrganisationMode::RenameInPlace, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert!(plan.entries[0].layout_folder.is_none());
}

#[test]
fn build_linked_library_plans_without_any_romm_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("emuwiz-library");
    let source = dir.path().join("sources");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Combat.bin",
        resolved("Atari2600", PlatformIdentitySource::Romm),
    );
    let plan = plan_for(&library, OrganisationMode::BuildLinkedLibrary, &[cand], 1);
    let entry = &plan.entries[0];
    assert_eq!(entry.status, OrganisationStatus::Suggested);
    assert_eq!(
        entry.destination_path,
        library.join("Atari 2600").join("Combat.bin")
    );
    assert_eq!(entry.layout_folder.as_deref(), Some("Atari 2600"));
}

#[test]
fn romm_specific_resolution_still_fails_honestly_when_mapping_is_missing() {
    // Pick a canonical platform whose PRODUCTION RomM resolution is honestly
    // unmapped (no override, no live cache, not in the vetted table).
    let unmapped: Vec<&str> = crate::platform_evidence_fusion::romm_platform_mapping::static_coverage_by_status()
        .get(&crate::platform_evidence_fusion::romm_platform_mapping::RommMappingSupportStatus::Unmapped)
        .cloned()
        .unwrap_or_default();
    let Some(platform_id) = unmapped.first() else {
        // Every platform happens to be mapped: the honesty claim is vacuous
        // but not violated.
        return;
    };
    let overrides = crate::library_views::FrontendPlatformMapping::default();
    assert_eq!(
        crate::platform_evidence_fusion::romm_platform_mapping::production_romm_slug(
            platform_id,
            &overrides,
            None
        ),
        None,
        "the RomM-specific resolver must keep refusing an unmapped platform"
    );
    assert_eq!(
        crate::platform_evidence_fusion::romm_platform_mapping::production_romm_status(
            platform_id,
            &overrides,
            None
        ),
        crate::platform_evidence_fusion::romm_platform_mapping::RommMappingSupportStatus::Unmapped
    );

    // The SAME platform organises fine generically: neutral identity, no RomM.
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let cand = candidate(
        &source,
        "Game.bin",
        resolved(platform_id, PlatformIdentitySource::Manual),
    );
    let plan = plan_for(&master, OrganisationMode::MoveRealFile, &[cand], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    let expected_folder =
        crate::platform::canonical_layout_folder(platform_id).expect("neutral folder");
    assert_eq!(
        plan.entries[0]
            .destination_path
            .parent()
            .map(Path::to_path_buf),
        Some(master.join(&expected_folder))
    );
}

// --- progress and cancellation of the preview --------------------------------

fn preview_fixture(count: usize) -> (tempfile::TempDir, PathBuf, Vec<OrganisationCandidate>) {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("roms");
    let source = dir.path().join("library");
    std::fs::create_dir_all(&source).unwrap();
    let candidates = (0..count)
        .map(|index| {
            candidate(
                &source,
                &format!("Game{index}.iso"),
                resolved("PSP", PlatformIdentitySource::Romm),
            )
        })
        .collect();
    (dir, master, candidates)
}

fn tree_listing(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(read) = std::fs::read_dir(&dir) {
            for entry in read.flatten() {
                found.push(entry.path());
                if entry.path().is_dir() {
                    stack.push(entry.path());
                }
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_preview_reports_its_real_phases_and_matches_the_plain_planner() {
    use crate::job_progress::{JobProgress, ProgressReporter};
    let (dir, master, candidates) = preview_fixture(6);
    let request = OrganisationPlanRequest {
        master_root: &master,
        mode: OrganisationMode::MoveRealFile,
        content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
        candidates: &candidates,
        generation: 1,
        unknown_folder: None,
        link_target_roots: None,
    };
    let mut seen: Vec<JobProgress> = Vec::new();
    let plan = {
        let mut sink = |progress| seen.push(progress);
        let mut reporter = ProgressReporter::new(&mut sink);
        build_organisation_plan_reporting(&request, None, &mut reporter).unwrap()
    };
    assert_eq!(plan, build_organisation_plan(&request));
    let mut phases: Vec<&str> = seen.iter().map(|p| p.phase.as_str()).collect();
    phases.dedup();
    assert_eq!(
        phases,
        [
            "Analysing files",
            "Checking destination conflicts",
            "Building the preview"
        ]
    );
    let analysing = seen
        .iter()
        .filter(|p| p.phase == "Analysing files")
        .next_back()
        .unwrap();
    assert_eq!((analysing.completed, analysing.total), (6, Some(6)));
    drop(dir);
}

#[test]
fn a_cancelled_preview_yields_no_plan_and_changes_nothing_on_disk() {
    use crate::job_progress::ProgressReporter;
    let (dir, master, candidates) = preview_fixture(8);
    let before = tree_listing(dir.path());
    let request = OrganisationPlanRequest {
        master_root: &master,
        mode: OrganisationMode::MoveRealFile,
        content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
        candidates: &candidates,
        generation: 1,
        unknown_folder: None,
        link_target_roots: None,
    };
    let cancel = AtomicBool::new(false);
    let mut sink = |progress: crate::job_progress::JobProgress| {
        if progress.phase == "Analysing files" && progress.completed >= 3 {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    };
    let mut reporter = ProgressReporter::with_interval(&mut sink, std::time::Duration::ZERO);
    assert!(build_organisation_plan_reporting(&request, Some(&cancel), &mut reporter).is_none());
    assert_eq!(
        before,
        tree_listing(dir.path()),
        "a preview never touches the filesystem"
    );
    assert!(!master.exists());
}

/// Real-library check (Atari 2600): the catalogue stores the canonical id with
/// a `folder_alias` provenance. That persisted assignment must reach the
/// planner as usable platform evidence, while a file with no catalogue row is
/// refused even when its folder is literally named `atari2600` - a folder name
/// is never identity on its own.
#[test]
fn persisted_canonical_atari2600_assignment_survives_into_the_plan_but_a_folder_name_does_not() {
    use crate::platform::identity::resolve_platform_identity;
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("atari2600");
    std::fs::create_dir_all(&library).unwrap();
    let master = dir.path().join("roms");

    // What `current_platform_identity_evidence` yields for source `folder_alias`.
    let evidence = PlatformIdentityEvidence::canonical(
        "Atari2600",
        PlatformIdentitySource::Inference,
        PlatformIdentityConfidence::Inferred,
        1,
        "persisted platform assignment from folder_alias",
    )
    .expect("the canonical id is accepted");
    let catalogued = candidate(
        &library,
        "Combat (USA).bin",
        resolve_platform_identity(1, [evidence]),
    );
    // No catalogue row -> no evidence at all -> Unknown, whatever the folder says.
    let uncatalogued = candidate(
        &library,
        "Uncatalogued (USA).bin",
        resolve_platform_identity(1, []),
    );
    let plan = plan_for(
        &master,
        OrganisationMode::RenameInPlace,
        &[catalogued, uncatalogued],
        1,
    );
    let reason = |index: usize| plan.entries[index].reason.clone().unwrap_or_default();
    assert!(
        !reason(0).contains("no platform identity"),
        "canonical assignment lost: {}",
        reason(0)
    );
    assert_eq!(plan.entries[1].status, OrganisationStatus::Blocked);
    assert!(reason(1).contains("no platform identity could be resolved"));
}

#[path = "catalogue_tests.rs"]
mod catalogue_tests;

// --- Platform correction: existing folders and Unknown placement -----------

fn plan_with_unknown(
    master_root: &Path,
    candidates: &[OrganisationCandidate],
    unknown: Option<&Path>,
) -> OrganisationPlan {
    build_organisation_plan(&OrganisationPlanRequest {
        master_root,
        mode: OrganisationMode::MoveRealFile,
        content_policy: ContentSelectionPolicy::AllEntries,
        candidates,
        generation: 1,
        unknown_folder: unknown,
        link_target_roots: None,
    })
}

#[test]
fn an_existing_scanner_folder_for_the_platform_is_the_destination() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    std::fs::create_dir_all(root.join("snes")).unwrap();
    std::fs::create_dir_all(root.join("nes")).unwrap();
    let cand = candidate(
        &root.join("snes"),
        "Game.zip",
        resolved("NES", PlatformIdentitySource::VerifiedDat),
    );
    let plan = plan_with_unknown(&root, &[cand], None);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert_eq!(
        plan.entries[0].destination_path,
        root.join("nes").join("Game.zip")
    );
    assert_eq!(layout_folder_in(&root, "NES").unwrap(), "nes");
}

#[test]
fn without_an_existing_folder_the_canonical_folder_is_planned() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    std::fs::create_dir_all(root.join("snes")).unwrap();
    let canonical = crate::platform::canonical_layout_folder("NES").unwrap();
    assert_eq!(layout_folder_in(&root, "NES").unwrap(), canonical);
}

#[test]
fn two_existing_non_canonical_folders_for_one_platform_block_the_move() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    std::fs::create_dir_all(root.join("snes")).unwrap();
    std::fs::create_dir_all(root.join("nes")).unwrap();
    std::fs::create_dir_all(root.join("famicom")).unwrap();
    let cand = candidate(
        &root.join("snes"),
        "Game.zip",
        resolved("NES", PlatformIdentitySource::VerifiedDat),
    );
    let plan = plan_with_unknown(&root, &[cand], None);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
    assert!(
        plan.entries[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("several existing folders")
    );
}

#[test]
fn unknown_games_move_to_the_unknown_folder_only_when_requested() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    std::fs::create_dir_all(root.join("mixed")).unwrap();
    let cand = candidate(
        &root.join("mixed"),
        "Mystery.bin",
        PlatformIdentityResolution::Unknown { generation: 1 },
    );
    let blocked = plan_with_unknown(&root, std::slice::from_ref(&cand), None);
    assert_eq!(blocked.entries[0].status, OrganisationStatus::Blocked);
    let unknown = Path::new("Unnamed/Unknown");
    let plan = plan_with_unknown(&root, std::slice::from_ref(&cand), Some(unknown));
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert_eq!(
        plan.entries[0].destination_path,
        root.join("Unnamed").join("Unknown").join("Mystery.bin")
    );
    assert_eq!(plan.entries[0].platform, None);
    // Unsafe Unknown folders are refused.
    for bad in ["../x", "/abs", ""] {
        let plan = plan_with_unknown(&root, std::slice::from_ref(&cand), Some(Path::new(bad)));
        assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked, "{bad}");
    }
}

#[test]
fn conflicting_platforms_never_move_even_with_an_unknown_folder() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    std::fs::create_dir_all(root.join("mixed")).unwrap();
    let conflict = PlatformIdentityResolution::Conflict {
        generation: 1,
        evidence: Vec::new(),
    };
    let cand = candidate(&root.join("mixed"), "Both.zip", conflict);
    let plan = plan_with_unknown(&root, &[cand], Some(Path::new("Unnamed/Unknown")));
    assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
}

// ---------------------------------------------------------------------------
// Reorganise existing links: named from the target game file (owner 2026-11-22)
// ---------------------------------------------------------------------------

struct LinkFixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    master: PathBuf,
    games: PathBuf,
    links: PathBuf,
}

impl LinkFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (master, games, links) = (root.join("roms"), root.join("games"), root.join("links"));
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(&links).unwrap();
        Self {
            _dir: dir,
            root,
            master,
            games,
            links,
        }
    }
    /// A game file and a link to it under `link_name`.
    fn link(&self, game: &str, link_name: &str) -> (PathBuf, PathBuf) {
        let target = self.games.join(game);
        if !target.exists() {
            std::fs::write(&target, format!("contents of {game}")).unwrap();
        }
        let link = self.links.join(link_name);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        (target, link)
    }
    fn candidate(&self, link: &Path, verified_name: Option<&str>) -> OrganisationCandidate {
        OrganisationCandidate {
            source_path: link.to_path_buf(),
            resolution: resolved("PSP", PlatformIdentitySource::Romm),
            canonical_name: verified_name.map(str::to_string),
            content_classification: None,
            original_metadata: Default::default(),
        }
    }
    /// Reorganise existing links, with `games` as the approved folder.
    fn plan(&self, candidates: &[OrganisationCandidate], _generation: u64) -> OrganisationPlan {
        plan_with_link_roots(&self.master, candidates, std::slice::from_ref(&self.games))
    }
    fn folder(&self) -> PathBuf {
        self.master.join("Sony PlayStation Portable")
    }
}

#[test]
fn an_unverified_existing_link_is_named_from_its_target_game_file() {
    // The link was made earlier under a RomM title; with no verified name
    // (the GUI passes none) it is named from the game file, not kept.
    let fx = LinkFixture::new();
    let (target, link) = fx.link("ugly_game.iso", "Romm Title (USA).iso");
    let plan = fx.plan(&[fx.candidate(&link, None)], 1);
    let row = &plan.entries[0];
    assert_eq!(row.status, OrganisationStatus::Suggested, "{row:?}");
    assert_eq!(row.destination_path, fx.folder().join("ugly_game.iso"));
    let before = std::fs::read(&target).unwrap();
    let journal = fx.root.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let outcome = apply_plan(&plan, &approved_of(&[&link]), &journal, &no_cancel());
    assert_eq!(
        outcome.transaction.state,
        crate::dat::rename_apply::TransactionState::Applied
    );
    let moved = fx.folder().join("ugly_game.iso");
    assert_eq!(
        std::fs::read_link(&moved).unwrap(),
        target,
        "exact target kept"
    );
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the old link name is gone"
    );
    assert_eq!(
        std::fs::read(&target).unwrap(),
        before,
        "game file untouched"
    );
    assert!(std::fs::symlink_metadata(&target).unwrap().is_file());
}

#[test]
fn a_verified_existing_link_takes_the_verified_name() {
    let fx = LinkFixture::new();
    let (_, link) = fx.link("ugly_game.iso", "Romm Title (USA).iso");
    let plan = fx.plan(&[fx.candidate(&link, Some("Verified Game (USA).iso"))], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    assert_eq!(
        plan.entries[0].destination_path,
        fx.folder().join("Verified Game (USA).iso")
    );
}

#[test]
fn an_existing_link_already_named_after_its_game_in_place_is_left_alone() {
    let fx = LinkFixture::new();
    std::fs::create_dir_all(fx.folder()).unwrap();
    let target = fx.games.join("game.iso");
    std::fs::write(&target, b"g").unwrap();
    let link = fx.folder().join("game.iso");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let plan = fx.plan(&[fx.candidate(&link, None)], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::AlreadyOrganised);
}

#[test]
fn two_existing_links_whose_games_share_a_name_are_both_conflicts() {
    let fx = LinkFixture::new();
    let (_, first) = fx.link("same.iso", "Romm A.iso");
    let sub = fx.games.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let other = sub.join("same.iso");
    std::fs::write(&other, b"other").unwrap();
    let second = fx.links.join("Romm B.iso");
    std::os::unix::fs::symlink(&other, &second).unwrap();
    let plan = fx.plan(
        &[fx.candidate(&first, None), fx.candidate(&second, None)],
        1,
    );
    assert_eq!(plan.entries.len(), 2, "neither is dropped");
    for row in &plan.entries {
        assert_eq!(row.status, OrganisationStatus::Conflict, "{row:?}");
        assert!(row.reason.is_some());
    }
    assert!(
        std::fs::symlink_metadata(&first).is_ok() && std::fs::symlink_metadata(&second).is_ok()
    );
}

#[test]
fn an_existing_link_onto_an_occupied_name_is_a_conflict_not_an_overwrite() {
    let fx = LinkFixture::new();
    let (_, link) = fx.link("game.iso", "Romm.iso");
    std::fs::create_dir_all(fx.folder()).unwrap();
    std::fs::write(fx.folder().join("game.iso"), b"someone else's file").unwrap();
    let plan = fx.plan(&[fx.candidate(&link, None)], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Conflict);
    assert_eq!(
        std::fs::read(fx.folder().join("game.iso")).unwrap(),
        b"someone else's file"
    );
}

#[test]
fn a_link_whose_new_place_is_its_own_game_says_so() {
    // The destination folder already holds the game itself (the master root
    // overlaps the game folder): a self-target conflict, worded as such,
    // distinct from a different file in the way. Nothing is overwritten.
    let fx = LinkFixture::new();
    std::fs::create_dir_all(fx.folder()).unwrap();
    let own = fx.folder().join("own.iso");
    std::fs::write(&own, b"the game").unwrap();
    let own_link = fx.links.join("Romm Own.iso");
    std::os::unix::fs::symlink(&own, &own_link).unwrap();
    let (_, other_link) = fx.link("taken.iso", "Romm Taken.iso");
    std::fs::write(fx.folder().join("taken.iso"), b"someone else's file").unwrap();
    let plan = plan_with_link_roots(
        &fx.master,
        &[
            fx.candidate(&own_link, None),
            fx.candidate(&other_link, None),
        ],
        &[fx.master.clone(), fx.games.clone()],
    );
    let row = |link: &Path| plan.entries.iter().find(|e| e.source_path == link).unwrap();
    assert_eq!(row(&own_link).status, OrganisationStatus::Conflict);
    assert!(
        row(&own_link)
            .reason
            .as_deref()
            .unwrap()
            .contains("its own game file"),
        "{:?}",
        row(&own_link).reason
    );
    assert_eq!(row(&other_link).status, OrganisationStatus::Conflict);
    assert_eq!(
        row(&other_link).reason.as_deref(),
        Some("the destination already exists; nothing is overwritten")
    );
    assert_eq!(std::fs::read(&own).unwrap(), b"the game");
    assert_eq!(
        std::fs::read(fx.folder().join("taken.iso")).unwrap(),
        b"someone else's file"
    );
    assert!(
        std::fs::symlink_metadata(&own_link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn reorganised_links_need_a_folder_separate_from_every_game_folder() {
    use super::link_target::separate_link_destination as check;
    let fx = LinkFixture::new();
    let games = vec![fx.games.clone()];
    let inside = fx.games.join("links-here");
    std::fs::create_dir_all(&inside).unwrap();
    let separate = fx.root.join("reorganised");
    std::fs::create_dir_all(&separate).unwrap();
    // A link to a folder inside the game folder resolves there: refused.
    let disguised = fx.root.join("disguised");
    std::os::unix::fs::symlink(&inside, &disguised).unwrap();
    for (destination, says) in [
        (&fx.games, "is your game folder"),
        (&inside, "is inside your game folder"),
        (&fx.root, "holds your game folder"),
        (&disguised, "is inside your game folder"),
    ] {
        let error = check(destination, &games).unwrap_err();
        assert!(error.contains(says), "{}: {error}", destination.display());
        assert!(error.contains("Nothing was changed"), "{error}");
    }
    assert_eq!(check(&separate, &games).unwrap(), separate);
    // Unknown game folders, an unreadable game folder or a missing
    // destination cannot be confirmed separate: refused.
    assert!(check(&separate, &[]).is_err());
    assert!(check(&separate, &[fx.root.join("no-such-games")]).is_err());
    assert!(check(&fx.root.join("missing"), &games).is_err());
}

#[test]
fn relative_absolute_escaping_chained_and_broken_existing_links() {
    let fx = LinkFixture::new();
    std::fs::write(fx.games.join("game.iso"), b"g").unwrap();
    std::fs::write(fx.games.join("other.iso"), b"o").unwrap();
    let outside = fx.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("away.iso"), b"a").unwrap();
    let link = |name: &str, text: &Path| {
        let link = fx.links.join(name);
        std::os::unix::fs::symlink(text, &link).unwrap();
        link
    };
    // Safe: a relative '..' link and an absolute link to a game in the
    // approved folder, named from the game file.
    std::fs::create_dir_all(fx.links.join("sub")).unwrap();
    let relative = fx.links.join("sub").join("Romm Rel.iso");
    std::os::unix::fs::symlink("../../games/game.iso", &relative).unwrap();
    let absolute = link("Romm Abs.iso", &fx.games.join("other.iso"));
    let plan = fx.plan(
        &[fx.candidate(&relative, None), fx.candidate(&absolute, None)],
        1,
    );
    for (link, name) in [(&relative, "game.iso"), (&absolute, "other.iso")] {
        let row = plan
            .entries
            .iter()
            .find(|e| &e.source_path == link)
            .unwrap();
        assert_eq!(row.status, OrganisationStatus::Suggested, "{row:?}");
        assert_eq!(row.destination_path, fx.folder().join(name));
    }
    // A plain relative target in the link's own folder is resolved there, and
    // is outside the approved folder.
    std::fs::write(fx.links.join("local.iso"), b"l").unwrap();
    let local = link("Romm Local.iso", Path::new("local.iso"));
    // Refused.
    let escaping = link("Escaping.iso", Path::new("../outside/away.iso"));
    let sneaky = link("Sneaky.iso", Path::new("../games/../outside/away.iso"));
    let outside_abs = link("Outside.iso", &outside.join("away.iso"));
    let chained = link("Chained.iso", &relative);
    let loop_a = fx.links.join("Loop A.iso");
    let loop_b = fx.links.join("Loop B.iso");
    std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
    std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();
    let alias = fx.root.join("alias");
    std::os::unix::fs::symlink(&fx.games, &alias).unwrap();
    let via_folder = link("Via Folder.iso", &alias.join("game.iso"));
    let to_folder = link("To Folder.iso", &fx.games);
    let broken = link("Broken.iso", &fx.games.join("gone.iso"));
    std::fs::write(fx.games.join(" spaced.iso"), b"s").unwrap();
    let spaced = link("Spaced.iso", &fx.games.join(" spaced.iso"));
    for (link, words) in [
        (&local, "outside the library's approved folders"),
        (&escaping, "outside the library's approved folders"),
        (&sneaky, "outside the library's approved folders"),
        (&outside_abs, "outside the library's approved folders"),
        (&chained, "through another link"),
        (&loop_a, "through another link"),
        (&via_folder, "through another link"),
        (&to_folder, "does not point at a game file"),
        (&broken, "broken"),
        (&spaced, "can be used safely"),
    ] {
        let before = std::fs::read_link(link).unwrap();
        let plan = fx.plan(&[fx.candidate(link, None)], 1);
        let row = &plan.entries[0];
        assert_eq!(
            row.status,
            OrganisationStatus::Blocked,
            "{}",
            link.display()
        );
        assert!(row.reason.as_deref().unwrap().contains(words), "{row:?}");
        assert_eq!(std::fs::read_link(link).unwrap(), before, "left as it is");
    }
    // No approved folders: every link is refused (fail closed).
    for roots in [None, Some(&[][..]), Some(&[alias.clone()][..])] {
        let plan = build_organisation_plan(&OrganisationPlanRequest {
            master_root: &fx.master,
            mode: OrganisationMode::OrganiseSymlinkOnly,
            content_policy: crate::dat::classification::ContentSelectionPolicy::AllEntries,
            candidates: &[fx.candidate(&relative, None)],
            generation: 1,
            unknown_folder: None,
            link_target_roots: roots,
        });
        assert_eq!(
            plan.entries[0].status,
            OrganisationStatus::Blocked,
            "{roots:?}"
        );
        assert!(
            plan.entries[0]
                .reason
                .as_deref()
                .unwrap()
                .contains("approved folders")
        );
    }
}

#[test]
fn a_folder_swapped_for_a_link_after_the_preview_is_not_applied() {
    // Target substitution through a path component: the game's folder is
    // replaced by a link to another folder holding a same-named file.
    let fx = LinkFixture::new();
    let sub = fx.games.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("game.iso"), b"real").unwrap();
    std::fs::create_dir_all(fx.links.join("deep")).unwrap();
    let link = fx.links.join("deep").join("Romm.iso");
    std::os::unix::fs::symlink("../../games/sub/game.iso", &link).unwrap();
    let plan = fx.plan(&[fx.candidate(&link, None)], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    let decoy = fx.root.join("decoy");
    std::fs::create_dir_all(&decoy).unwrap();
    std::fs::write(decoy.join("game.iso"), b"decoy").unwrap();
    std::fs::rename(&sub, fx.root.join("sub-moved")).unwrap();
    std::os::unix::fs::symlink(&decoy, &sub).unwrap();
    // Re-planning at Apply (what revalidation does) now refuses the link.
    let again = fx.plan(&[fx.candidate(&link, None)], 1);
    assert_eq!(again.entries[0].status, OrganisationStatus::Blocked);
    assert!(
        again.entries[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("through another link")
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new("../../games/sub/game.iso")
    );
}

#[test]
fn an_existing_link_retargeted_after_the_preview_is_not_applied() {
    let fx = LinkFixture::new();
    let (target, link) = fx.link("game.iso", "Romm.iso");
    let plan = fx.plan(&[fx.candidate(&link, None)], 1);
    assert_eq!(plan.entries[0].status, OrganisationStatus::Suggested);
    // After the preview the link is pointed at another game.
    std::fs::write(fx.games.join("other.iso"), b"o").unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(fx.games.join("other.iso"), &link).unwrap();
    let journal = fx.root.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    std::fs::create_dir_all(&fx.master).unwrap();
    // Refused at the first check (building the transaction), or failing
    // that at Apply: either way nothing moves.
    let approved = approved_of(&[&link]);
    let applied = match build_organisation_transaction(&plan, &approved, 1) {
        Err(error) => {
            assert!(error.contains("source identity changed"), "{error}");
            false
        }
        Ok(mut tx) => matches!(
            apply_organisation_transaction(
                &mut tx,
                &approved,
                1,
                TrustedRoots::from_paths(vec![fx.master.clone(), fx.links.clone()]),
                &journal,
                &no_cancel(),
                OrganisationMode::OrganiseSymlinkOnly,
                &fx.master,
            ),
            Ok(ref outcome) if outcome.transaction.state
                == crate::dat::rename_apply::TransactionState::Applied
        ),
    };
    assert!(!applied, "a retargeted link must not be moved");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        fx.games.join("other.iso")
    );
    assert!(std::fs::symlink_metadata(fx.folder().join("game.iso")).is_err());
    assert!(target.exists());
}

#[test]
fn a_relative_link_moves_only_when_it_still_reaches_its_game_from_the_new_folder() {
    let fx = LinkFixture::new();
    std::fs::write(fx.games.join("game.iso"), b"g").unwrap();
    // links/x.iso -> ../games/game.iso: from roms/<folder>/ that would be
    // roms/games/game.iso, a different file, so it is refused.
    let shallow = fx.links.join("Shallow.iso");
    std::os::unix::fs::symlink("../games/game.iso", &shallow).unwrap();
    // links/sub/x.iso -> ../../games/game.iso: the same depth as
    // roms/<folder>/, so it still reaches the game and may move.
    std::fs::create_dir_all(fx.links.join("sub")).unwrap();
    let same_depth = fx.links.join("sub").join("Deep.iso");
    std::os::unix::fs::symlink("../../games/game.iso", &same_depth).unwrap();
    // Down and back up: refused as uncheckable.
    let zigzag = fx.links.join("Zigzag.iso");
    std::os::unix::fs::symlink("../games/../games/game.iso", &zigzag).unwrap();
    let plan = fx.plan(&[fx.candidate(&same_depth, None)], 1);
    assert_eq!(
        plan.entries[0].status,
        OrganisationStatus::Suggested,
        "{:?}",
        plan.entries[0]
    );
    for (link, words) in [(&shallow, "different file"), (&zigzag, "down and back up")] {
        let plan = fx.plan(&[fx.candidate(link, None)], 1);
        assert_eq!(plan.entries[0].status, OrganisationStatus::Blocked);
        assert!(
            plan.entries[0].reason.as_deref().unwrap().contains(words),
            "{:?}",
            plan.entries[0]
        );
    }
    // Applied, the moved link still reaches the same game file.
    let journal = fx.root.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let outcome = apply_plan(&plan, &approved_of(&[&same_depth]), &journal, &no_cancel());
    assert_eq!(
        outcome.transaction.state,
        crate::dat::rename_apply::TransactionState::Applied
    );
    let moved = fx.folder().join("game.iso");
    assert_eq!(
        std::fs::read_link(&moved).unwrap(),
        Path::new("../../games/game.iso")
    );
    assert_eq!(
        std::fs::canonicalize(&moved).unwrap(),
        fx.games.join("game.iso")
    );
}
