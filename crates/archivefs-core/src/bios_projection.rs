//! Read-only BIOS/firmware inventory and projection planning.
//!
//! This module deliberately plans only.  It never creates links, copies, or
//! changes emulator configuration.  A master collection is treated as an
//! untrusted source tree and all traversal is bounded and symlink-free.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MAX_DEPTH: usize = 3;
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BiosContentClass {
    ImmutableFirmware,
    WritableState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BiosMatchStatus {
    VerifiedMatch,
    FilenameOnly,
    HashMismatch,
    Ambiguous,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BiosProjectionMethod {
    DirectPath,
    SymlinkFile,
    SymlinkDirectory,
    LocalWritableCopy,
    LocalWritableReflink,
    FirmwareInstallRequired,
    ExternalSystemData,
    NoBiosRequired,
    RomsetDependency,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BiosProjectionStatus {
    Ready,
    AlreadyProjected,
    ReviewRequired,
    Blocked,
    Missing,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BiosTargetState {
    NotInspected,
    Missing,
    ExistingRegularFile,
    ExistingCorrectLink,
    ExistingWrongLink,
    UnsafeSpecialFile,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BiosEvidenceSource {
    Filename,
    Hash,
    DatCatalogue,
    Config,
    KnownDefault,
    Profile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosEvidence {
    pub relative_path: PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub source: BiosEvidenceSource,
    pub match_status: BiosMatchStatus,
    pub platform: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosMasterInventory {
    pub root: PathBuf,
    pub entries: Vec<BiosEvidence>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosProjectionTarget {
    pub description: String,
    pub path: Option<PathBuf>,
    pub current_state: BiosTargetState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosRequirement {
    pub name: String,
    pub emulator: String,
    pub expected_filenames: Vec<String>,
    pub expected_sha256: Option<String>,
    pub target: BiosProjectionTarget,
    pub content_class: BiosContentClass,
    pub method: BiosProjectionMethod,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BiosProjectionAction {
    UseSource(PathBuf),
    CreateFileLink {
        source: PathBuf,
        target: BiosProjectionTarget,
    },
    KeepWritableLocal {
        target: BiosProjectionTarget,
    },
    NoAction,
    RequiresExternalInstall,
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosProjectionPlan {
    pub master_root: PathBuf,
    pub emulator: String,
    pub requirements: Vec<BiosRequirement>,
    pub matches: Vec<Option<BiosEvidence>>,
    pub actions: Vec<BiosProjectionAction>,
    pub status: BiosProjectionStatus,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosProjectionResult {
    pub inventory: BiosMasterInventory,
    pub plans: Vec<BiosProjectionPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BiosInventoryError {
    RootMissing,
    RootNotDirectory,
    RootUnreadable(String),
}

impl std::fmt::Display for BiosInventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootMissing => write!(f, "master BIOS root does not exist"),
            Self::RootNotDirectory => write!(f, "master BIOS root is not a directory"),
            Self::RootUnreadable(e) => write!(f, "master BIOS root is unreadable: {e}"),
        }
    }
}
impl std::error::Error for BiosInventoryError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosProjectionTransaction {
    pub journal_id: String,
    pub emulator: String,
    pub requirement_ids: Vec<String>,
    pub applied: Vec<BiosAppliedItem>,
    pub already_correct: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BiosAppliedItem {
    pub source: PathBuf,
    pub target: PathBuf,
    pub method: BiosProjectionMethod,
    pub pre_state: BiosTargetState,
    pub post_state: BiosTargetState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BiosProjectionApplyError {
    ConfirmationRequired,
    NoEligibleItems,
    StalePlan(String),
    UnsafeTarget(PathBuf),
    TargetConflict(PathBuf),
    WritableStateNotSupported(String),
    UnsupportedMethod(BiosProjectionMethod),
    SourceInvalid(PathBuf, String),
    Io(String),
}

impl std::fmt::Display for BiosProjectionApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfirmationRequired => write!(f, "typed BIOS plan confirmation is required"),
            Self::NoEligibleItems => write!(f, "the BIOS plan contains no eligible apply actions"),
            Self::StalePlan(detail) => write!(f, "BIOS plan is stale: {detail}"),
            Self::UnsafeTarget(path) => write!(
                f,
                "target is outside the approved emulator root: {}",
                path.display()
            ),
            Self::TargetConflict(path) => write!(
                f,
                "target already contains unrelated content: {}",
                path.display()
            ),
            Self::WritableStateNotSupported(name) => write!(
                f,
                "writable emulator state is not supported in Phase 2: {name}"
            ),
            Self::UnsupportedMethod(method) => write!(
                f,
                "projection method is not executable in Phase 2: {method:?}"
            ),
            Self::SourceInvalid(path, detail) => write!(
                f,
                "source is no longer valid ({}): {detail}",
                path.display()
            ),
            Self::Io(detail) => write!(f, "BIOS projection failed: {detail}"),
        }
    }
}
impl std::error::Error for BiosProjectionApplyError {}

pub const APPLY_CONFIRMATION_PREFIX: &str = "APPLY BIOS PLAN ";

pub fn apply_confirmation(plan_count: usize) -> String {
    format!("{APPLY_CONFIRMATION_PREFIX}{plan_count}")
}

/// Inspect a caller-selected master root without following links or writing.
pub fn inspect_master_root(root: &Path) -> Result<BiosMasterInventory, BiosInventoryError> {
    let meta = fs::symlink_metadata(root).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            BiosInventoryError::RootMissing
        } else {
            BiosInventoryError::RootUnreadable(e.to_string())
        }
    })?;
    if !meta.is_dir() {
        return Err(BiosInventoryError::RootNotDirectory);
    }
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    walk(root, root, 0, &mut entries, &mut warnings);
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(BiosMasterInventory {
        root: root.to_path_buf(),
        entries,
        warnings,
    })
}

fn walk(
    root: &Path,
    current: &Path,
    depth: usize,
    entries: &mut Vec<BiosEvidence>,
    warnings: &mut Vec<String>,
) {
    if entries.len() >= MAX_ENTRIES {
        warnings.push(format!("inventory limit reached ({MAX_ENTRIES} entries)"));
        return;
    }
    if depth > MAX_DEPTH {
        warnings.push(format!(
            "traversal depth limit reached at {}",
            current.display()
        ));
        return;
    }
    let read_dir = match fs::read_dir(current) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!(
                "unreadable directory {}: {error}",
                current.display()
            ));
            return;
        }
    };
    let mut children: Vec<_> = read_dir.filter_map(Result::ok).collect();
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if entries.len() >= MAX_ENTRIES {
            warnings.push(format!("inventory limit reached ({MAX_ENTRIES} entries)"));
            return;
        }
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(format!("unreadable entry {}: {error}", path.display()));
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            warnings.push(format!("skipped symlink {}", path.display()));
            continue;
        }
        if meta.is_dir() {
            walk(root, &path, depth + 1, entries, warnings);
            continue;
        }
        if !meta.is_file() {
            warnings.push(format!("skipped non-regular entry {}", path.display()));
            continue;
        }
        let relative = match path.strip_prefix(root) {
            Ok(value) if !value.is_absolute() => value.to_path_buf(),
            _ => {
                warnings.push(format!("rejected unsafe path {}", path.display()));
                continue;
            }
        };
        if meta.len() > MAX_FILE_BYTES {
            warnings.push(format!("skipped oversized file {}", relative.display()));
            continue;
        }
        let sha256 = if meta.len() <= MAX_HASH_BYTES {
            hash_file(&path, warnings)
        } else {
            None
        };
        entries.push(BiosEvidence {
            relative_path: relative,
            filename: entry.file_name().to_string_lossy().into_owned(),
            size_bytes: meta.len(),
            sha256,
            source: BiosEvidenceSource::Filename,
            match_status: BiosMatchStatus::Unknown,
            platform: None,
        });
    }
}

fn hash_file(path: &Path, warnings: &mut Vec<String>) -> Option<String> {
    let mut file = match fs::File::open(path) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("unreadable file {}: {error}", path.display()));
            return None;
        }
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => hasher.update(&buffer[..count]),
            Err(error) => {
                warnings.push(format!("could not hash {}: {error}", path.display()));
                return None;
            }
        }
    }
    let digest = hasher.finalize();
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn target(description: &str) -> BiosProjectionTarget {
    BiosProjectionTarget {
        description: description.into(),
        path: None,
        current_state: BiosTargetState::NotInspected,
    }
}

/// Inspect a future projection target without following or changing it.
/// Source/link identity comparison is intentionally left to a caller that has
/// selected a specific source; this primitive only reports safe filesystem
/// shape and never treats a regular file as an existing projection.
pub fn inspect_target(path: &Path) -> BiosTargetState {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BiosTargetState::Missing;
        }
        Err(_) => return BiosTargetState::Unknown,
    };
    if metadata.file_type().is_symlink() {
        BiosTargetState::ExistingCorrectLink
    } else if metadata.is_file() {
        BiosTargetState::ExistingRegularFile
    } else {
        BiosTargetState::UnsafeSpecialFile
    }
}
fn req(
    emulator: &str,
    name: &str,
    names: &[&str],
    class: BiosContentClass,
    method: BiosProjectionMethod,
    target_description: &str,
) -> BiosRequirement {
    BiosRequirement {
        name: name.into(),
        emulator: emulator.into(),
        expected_filenames: names.iter().map(|s| (*s).into()).collect(),
        expected_sha256: None,
        target: target(target_description),
        content_class: class,
        method,
    }
}

fn requirements(emulator: &str) -> Vec<BiosRequirement> {
    use BiosContentClass::{ImmutableFirmware as I, WritableState as W};
    use BiosProjectionMethod::*;
    match emulator {
        "RetroArch" => vec![req(
            emulator,
            "Known core BIOS",
            &["scph1000.bin", "scph1001.bin", "kick40068.A1200"],
            I,
            SymlinkFile,
            "RetroArch system directory",
        )],
        "PCSX2" => vec![req(
            emulator,
            "PS2 BIOS",
            &["SCPH-10000.BIN", "SCPH-30004R.BIN", "scph-70004.bin"],
            I,
            SymlinkFile,
            "PCSX2 configured BIOS directory",
        )],
        "DuckStation" => vec![req(
            emulator,
            "PS1 BIOS",
            &[
                "scph1000.bin",
                "scph1001.bin",
                "scph5500.bin",
                "scph5501.bin",
                "scph5502.bin",
            ],
            I,
            SymlinkFile,
            "DuckStation BIOS directory",
        )],
        "Dolphin" => vec![
            req(
                emulator,
                "GameCube IPL",
                &["IPL.bin"],
                I,
                SymlinkFile,
                "Dolphin GC/USA",
            ),
            req(
                emulator,
                "Dolphin NAND/SYSCONF",
                &["SYSCONF"],
                W,
                LocalWritableCopy,
                "Dolphin writable user/system state",
            ),
        ],
        "FS-UAE" => vec![req(
            emulator,
            "Amiga Kickstart",
            &["kick40068.A1200", "kick39106.A1200", "kick31.rom"],
            I,
            SymlinkFile,
            "FS-UAE Kickstarts directory",
        )],
        "Hatari" => vec![req(
            emulator,
            "Atari ST TOS",
            &["tos.img"],
            I,
            DirectPath,
            "Hatari TOS configuration",
        )],
        "Atari800" => vec![
            req(
                emulator,
                "Atari OS-B",
                &["ATARIOSB.ROM"],
                I,
                DirectPath,
                "Atari800 ROM_400/800_CUSTOM",
            ),
            req(
                emulator,
                "Atari XL/XE",
                &["ATARIXL.ROM"],
                I,
                DirectPath,
                "Atari800 ROM_XL/XE_CUSTOM",
            ),
            req(
                emulator,
                "Atari 5200",
                &["5200.rom"],
                I,
                DirectPath,
                "Atari800 ROM_5200_CUSTOM",
            ),
            req(
                emulator,
                "Atari BASIC",
                &["ATARIBAS.ROM"],
                I,
                DirectPath,
                "Atari800 ROM_BASIC_CUSTOM",
            ),
        ],
        "Caprice32" => vec![req(
            emulator,
            "Amstrad AMSDOS",
            &["amsdos.rom", "cpc_amsdos.rom"],
            I,
            SymlinkFile,
            "Caprice32 rom_path (amsdos.rom alias)",
        )],
        "xemu" => vec![
            req(
                emulator,
                "MCPX",
                &["mcpx_1.0.bin"],
                I,
                SymlinkFile,
                "xemu MCPX path",
            ),
            req(
                emulator,
                "Complex flash",
                &["Complex_4627v1.03.bin"],
                I,
                SymlinkFile,
                "xemu flash path",
            ),
            req(
                emulator,
                "EEPROM",
                &["eeprom.bin"],
                W,
                LocalWritableCopy,
                "xemu local EEPROM state",
            ),
            req(
                emulator,
                "Xbox HDD",
                &["xbox_hdd.qcow2"],
                W,
                LocalWritableCopy,
                "xemu local HDD state",
            ),
        ],
        "MAME" => vec![req(
            emulator,
            "BIOS/device ROM dependencies",
            &[],
            I,
            RomsetDependency,
            "MAME configured rompath and dependency graph",
        )],
        "PPSSPP" => vec![req(
            emulator,
            "BIOS",
            &[],
            I,
            NoBiosRequired,
            "PPSSPP does not require a conventional BIOS",
        )],
        "RPCS3" => vec![req(
            emulator,
            "PS3 firmware",
            &[],
            I,
            FirmwareInstallRequired,
            "RPCS3 firmware installation",
        )],
        "Azahar" => vec![req(
            emulator,
            "System data and keys",
            &[],
            I,
            ExternalSystemData,
            "Azahar external system data/keys",
        )],
        "Ryubing" => vec![req(
            emulator,
            "Firmware and keys",
            &[],
            I,
            ExternalSystemData,
            "Ryubing external firmware/keys",
        )],
        _ => vec![],
    }
}

pub const SUPPORTED_ADAPTERS: &[&str] = &[
    "RetroArch",
    "PCSX2",
    "DuckStation",
    "Dolphin",
    "FS-UAE",
    "Hatari",
    "Atari800",
    "Caprice32",
    "xemu",
    "MAME",
    "PPSSPP",
    "RPCS3",
    "Azahar",
    "Ryubing",
];

pub fn plan_projections(inventory: &BiosMasterInventory) -> BiosProjectionResult {
    let plans = SUPPORTED_ADAPTERS
        .iter()
        .map(|emulator| plan_for_emulator(inventory, emulator))
        .collect();
    BiosProjectionResult {
        inventory: inventory.clone(),
        plans,
    }
}

pub fn plan_for_emulator(inventory: &BiosMasterInventory, emulator: &str) -> BiosProjectionPlan {
    let requirements = requirements(emulator);
    let mut matches = Vec::new();
    let mut actions = Vec::new();
    let mut warnings = Vec::new();
    let mut status = BiosProjectionStatus::Ready;
    for requirement in &requirements {
        let candidates: Vec<_> = requirement
            .expected_filenames
            .iter()
            .flat_map(|name| {
                inventory
                    .entries
                    .iter()
                    .filter(move |entry| entry.filename.eq_ignore_ascii_case(name))
            })
            .cloned()
            .collect();
        let found = if candidates.len() == 1 {
            Some(candidates[0].clone())
        } else {
            None
        };
        let match_status = if requirement.method == BiosProjectionMethod::NoBiosRequired {
            BiosMatchStatus::Unknown
        } else if candidates.is_empty() && requirement.expected_filenames.is_empty() {
            BiosMatchStatus::Unknown
        } else if candidates.is_empty() {
            BiosMatchStatus::Missing
        } else if candidates.len() > 1 {
            BiosMatchStatus::Ambiguous
        } else if let Some(expected) = &requirement.expected_sha256 {
            if candidates[0].sha256.as_deref() == Some(expected.as_str()) {
                BiosMatchStatus::VerifiedMatch
            } else {
                BiosMatchStatus::HashMismatch
            }
        } else {
            BiosMatchStatus::FilenameOnly
        };
        let action = match (requirement.method, match_status) {
            (BiosProjectionMethod::NoBiosRequired, _) => BiosProjectionAction::NoAction,
            (BiosProjectionMethod::RomsetDependency, _) => {
                warnings.push("MAME BIOS/device files remain governed by the arcade dependency model; no generic BIOS directory is planned.".into());
                BiosProjectionAction::Review
            }
            (
                BiosProjectionMethod::FirmwareInstallRequired
                | BiosProjectionMethod::ExternalSystemData,
                _,
            ) => BiosProjectionAction::RequiresExternalInstall,
            (
                BiosProjectionMethod::LocalWritableCopy
                | BiosProjectionMethod::LocalWritableReflink,
                _,
            ) => {
                warnings.push(format!("{} is writable emulator state and must remain local; it must never be linked to the immutable master store.", requirement.name));
                BiosProjectionAction::KeepWritableLocal {
                    target: requirement.target.clone(),
                }
            }
            (_, BiosMatchStatus::VerifiedMatch | BiosMatchStatus::FilenameOnly) => {
                BiosProjectionAction::CreateFileLink {
                    source: found.as_ref().unwrap().relative_path.clone(),
                    target: requirement.target.clone(),
                }
            }
            _ => BiosProjectionAction::Review,
        };
        match match_status {
            BiosMatchStatus::Missing => {
                status = BiosProjectionStatus::Missing;
                warnings.push(format!(
                    "{} is missing from the selected master root.",
                    requirement.name
                ));
            }
            BiosMatchStatus::Ambiguous | BiosMatchStatus::HashMismatch => {
                status = BiosProjectionStatus::ReviewRequired;
                warnings.push(format!(
                    "{} has ambiguous or conflicting source evidence.",
                    requirement.name
                ));
            }
            _ => {}
        }
        matches.push(found);
        actions.push(action);
    }
    if requirements.iter().any(|r| {
        r.method == BiosProjectionMethod::FirmwareInstallRequired
            || r.method == BiosProjectionMethod::ExternalSystemData
    }) {
        status = BiosProjectionStatus::Unsupported;
    }
    if requirements
        .iter()
        .any(|r| r.method == BiosProjectionMethod::RomsetDependency)
    {
        status = BiosProjectionStatus::ReviewRequired;
    }
    if requirements.is_empty() {
        status = BiosProjectionStatus::Unsupported;
    }
    BiosProjectionPlan {
        master_root: inventory.root.clone(),
        emulator: emulator.into(),
        requirements,
        matches,
        actions,
        status,
        warnings,
    }
}

/// Apply only explicit immutable file/directory-link actions. The approved
/// root and typed confirmation are mandatory; no implicit emulator path is
/// ever selected. Direct config paths and writable state are refused.
pub fn apply_plan(
    plan: &BiosProjectionPlan,
    approved_target_root: &Path,
    confirmation: &str,
) -> Result<BiosProjectionTransaction, BiosProjectionApplyError> {
    if confirmation != apply_confirmation(plan.requirements.len()) {
        return Err(BiosProjectionApplyError::ConfirmationRequired);
    }
    if !approved_target_root.is_absolute() {
        return Err(BiosProjectionApplyError::UnsafeTarget(
            approved_target_root.to_path_buf(),
        ));
    }
    preflight_apply(plan, approved_target_root)?;
    let mut transaction = BiosProjectionTransaction {
        journal_id: format!("bios-{}", plan.requirements.len()),
        emulator: plan.emulator.clone(),
        requirement_ids: Vec::new(),
        applied: Vec::new(),
        already_correct: Vec::new(),
    };
    for (index, requirement) in plan.requirements.iter().enumerate() {
        if requirement.content_class == BiosContentClass::WritableState {
            return Err(BiosProjectionApplyError::WritableStateNotSupported(
                requirement.name.clone(),
            ));
        }
        let action = plan
            .actions
            .get(index)
            .ok_or(BiosProjectionApplyError::NoEligibleItems)?;
        let BiosProjectionAction::CreateFileLink {
            source: relative_source,
            target,
        } = action
        else {
            if matches!(requirement.method, BiosProjectionMethod::NoBiosRequired) {
                continue;
            }
            return Err(BiosProjectionApplyError::UnsupportedMethod(
                requirement.method,
            ));
        };
        if requirement.method != BiosProjectionMethod::SymlinkFile
            && requirement.method != BiosProjectionMethod::SymlinkDirectory
        {
            return Err(BiosProjectionApplyError::UnsupportedMethod(
                requirement.method,
            ));
        }
        let Some(target_path) = &target.path else {
            return Err(BiosProjectionApplyError::StalePlan(format!(
                "{} has no concrete target path",
                requirement.name
            )));
        };
        if !confined_path(target_path, approved_target_root) {
            return Err(BiosProjectionApplyError::UnsafeTarget(target_path.clone()));
        }
        let source = plan.master_root.join(relative_source);
        let source_meta = fs::symlink_metadata(&source).map_err(|error| {
            BiosProjectionApplyError::SourceInvalid(source.clone(), error.to_string())
        })?;
        if !source_meta.is_file() {
            return Err(BiosProjectionApplyError::SourceInvalid(
                source,
                "source is not a regular immutable file".into(),
            ));
        }
        let Some(Some(evidence)) = plan.matches.get(index) else {
            return Err(BiosProjectionApplyError::SourceInvalid(
                source,
                "source evidence is missing".into(),
            ));
        };
        if let Some(expected) = &evidence.sha256 {
            let mut warnings = Vec::new();
            let actual = hash_file(&source, &mut warnings).ok_or_else(|| {
                BiosProjectionApplyError::SourceInvalid(
                    source.clone(),
                    "source could not be hashed".into(),
                )
            })?;
            if &actual != expected {
                return Err(BiosProjectionApplyError::StalePlan(format!(
                    "source hash changed for {}",
                    requirement.name
                )));
            }
        }
        let pre_state = inspect_target(target_path);
        match pre_state {
            BiosTargetState::Missing => {
                if let Some(parent) = target_path.parent() {
                    create_confined_parents(parent, approved_target_root)?;
                }
                create_link(&source, target_path)
                    .map_err(|error| BiosProjectionApplyError::Io(error.to_string()))?;
                let post_state = inspect_target(target_path);
                if post_state != BiosTargetState::ExistingCorrectLink {
                    return Err(BiosProjectionApplyError::Io(
                        "created link could not be verified".into(),
                    ));
                }
                transaction.requirement_ids.push(requirement.name.clone());
                transaction.applied.push(BiosAppliedItem {
                    source,
                    target: target_path.clone(),
                    method: requirement.method,
                    pre_state,
                    post_state,
                });
            }
            BiosTargetState::ExistingCorrectLink => {
                let actual = fs::read_link(target_path)
                    .map_err(|error| BiosProjectionApplyError::Io(error.to_string()))?;
                if actual != source {
                    return Err(BiosProjectionApplyError::TargetConflict(
                        target_path.clone(),
                    ));
                }
                transaction.already_correct.push(target_path.clone());
            }
            _ => {
                return Err(BiosProjectionApplyError::TargetConflict(
                    target_path.clone(),
                ));
            }
        }
    }
    if transaction.applied.is_empty() && transaction.already_correct.is_empty() {
        return Err(BiosProjectionApplyError::NoEligibleItems);
    }
    Ok(transaction)
}

pub fn rollback_plan(
    transaction: &BiosProjectionTransaction,
) -> Result<(), BiosProjectionApplyError> {
    for item in transaction.applied.iter().rev() {
        let metadata = fs::symlink_metadata(&item.target)
            .map_err(|error| BiosProjectionApplyError::Io(error.to_string()))?;
        if !metadata.file_type().is_symlink() {
            return Err(BiosProjectionApplyError::TargetConflict(
                item.target.clone(),
            ));
        }
        let actual = fs::read_link(&item.target)
            .map_err(|error| BiosProjectionApplyError::Io(error.to_string()))?;
        if actual != item.source {
            return Err(BiosProjectionApplyError::TargetConflict(
                item.target.clone(),
            ));
        }
        fs::remove_file(&item.target)
            .map_err(|error| BiosProjectionApplyError::Io(error.to_string()))?;
    }
    Ok(())
}

fn confined_path(path: &Path, root: &Path) -> bool {
    path.is_absolute() && path.strip_prefix(root).is_ok()
}

fn preflight_apply(
    plan: &BiosProjectionPlan,
    approved_target_root: &Path,
) -> Result<(), BiosProjectionApplyError> {
    for (index, requirement) in plan.requirements.iter().enumerate() {
        if requirement.content_class == BiosContentClass::WritableState {
            return Err(BiosProjectionApplyError::WritableStateNotSupported(
                requirement.name.clone(),
            ));
        }
        let action = plan
            .actions
            .get(index)
            .ok_or(BiosProjectionApplyError::NoEligibleItems)?;
        let BiosProjectionAction::CreateFileLink {
            source: relative_source,
            target,
        } = action
        else {
            if matches!(requirement.method, BiosProjectionMethod::NoBiosRequired) {
                continue;
            }
            return Err(BiosProjectionApplyError::UnsupportedMethod(
                requirement.method,
            ));
        };
        if !matches!(
            requirement.method,
            BiosProjectionMethod::SymlinkFile | BiosProjectionMethod::SymlinkDirectory
        ) {
            return Err(BiosProjectionApplyError::UnsupportedMethod(
                requirement.method,
            ));
        }
        let Some(target_path) = &target.path else {
            return Err(BiosProjectionApplyError::StalePlan(format!(
                "{} has no concrete target path",
                requirement.name
            )));
        };
        if !confined_path(target_path, approved_target_root) {
            return Err(BiosProjectionApplyError::UnsafeTarget(target_path.clone()));
        }
        let source = plan.master_root.join(relative_source);
        let metadata = fs::symlink_metadata(&source).map_err(|error| {
            BiosProjectionApplyError::SourceInvalid(source.clone(), error.to_string())
        })?;
        if !metadata.is_file() {
            return Err(BiosProjectionApplyError::SourceInvalid(
                source,
                "source is not a regular immutable file".into(),
            ));
        }
        if let Some(expected) = plan
            .matches
            .get(index)
            .and_then(|item| item.as_ref())
            .and_then(|item| item.sha256.as_ref())
        {
            let mut warnings = Vec::new();
            let actual = hash_file(&source, &mut warnings).ok_or_else(|| {
                BiosProjectionApplyError::SourceInvalid(
                    source.clone(),
                    "source could not be hashed".into(),
                )
            })?;
            if &actual != expected {
                return Err(BiosProjectionApplyError::StalePlan(format!(
                    "source hash changed for {}",
                    requirement.name
                )));
            }
        }
        if !matches!(
            inspect_target(target_path),
            BiosTargetState::Missing | BiosTargetState::ExistingCorrectLink
        ) {
            return Err(BiosProjectionApplyError::TargetConflict(
                target_path.clone(),
            ));
        }
    }
    Ok(())
}

fn create_confined_parents(path: &Path, root: &Path) -> Result<(), BiosProjectionApplyError> {
    if !confined_path(path, root) {
        return Err(BiosProjectionApplyError::UnsafeTarget(path.to_path_buf()));
    }
    let mut current = root.to_path_buf();
    let relative = path
        .strip_prefix(root)
        .map_err(|_| BiosProjectionApplyError::UnsafeTarget(path.to_path_buf()))?;
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(BiosProjectionApplyError::TargetConflict(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|e| BiosProjectionApplyError::Io(e.to_string()))?
            }
            Err(error) => return Err(BiosProjectionApplyError::Io(error.to_string())),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}
#[cfg(windows)]
fn create_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(source, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    fn file(root: &Path, name: &str, bytes: &[u8]) {
        fs::write(root.join(name), bytes).unwrap();
    }
    #[test]
    fn scans_deterministically_and_preserves_source() {
        let dir = tempdir().unwrap();
        file(dir.path(), "tos.img", b"tos");
        file(dir.path(), "kick40068.A1200", b"kick");
        let before = fs::read(dir.path().join("tos.img")).unwrap();
        let inventory = inspect_master_root(dir.path()).unwrap();
        assert_eq!(inventory.entries[0].filename, "kick40068.A1200");
        assert!(inventory.entries[0].sha256.is_some());
        assert_eq!(fs::read(dir.path().join("tos.img")).unwrap(), before);
    }
    #[test]
    fn representative_semantics_are_explicit() {
        let dir = tempdir().unwrap();
        file(dir.path(), "cpc_amsdos.rom", b"a");
        file(dir.path(), "mcpx_1.0.bin", b"m");
        let result = plan_projections(&inspect_master_root(dir.path()).unwrap());
        let xemu = result.plans.iter().find(|p| p.emulator == "xemu").unwrap();
        assert_eq!(
            xemu.requirements[2].content_class,
            BiosContentClass::WritableState
        );
        assert_eq!(
            xemu.requirements[2].method,
            BiosProjectionMethod::LocalWritableCopy
        );
        assert!(
            xemu.warnings
                .iter()
                .any(|w| w.contains("must remain local"))
        );
        let caprice = result
            .plans
            .iter()
            .find(|p| p.emulator == "Caprice32")
            .unwrap();
        assert!(matches!(
            caprice.actions[0],
            BiosProjectionAction::CreateFileLink { .. }
        ));
    }
    #[test]
    fn special_systems_do_not_get_fake_file_projections() {
        let dir = tempdir().unwrap();
        let result = plan_projections(&inspect_master_root(dir.path()).unwrap());
        let p = |name| result.plans.iter().find(|p| p.emulator == name).unwrap();
        assert_eq!(
            p("PPSSPP").requirements[0].method,
            BiosProjectionMethod::NoBiosRequired
        );
        assert_eq!(
            p("MAME").requirements[0].method,
            BiosProjectionMethod::RomsetDependency
        );
        assert_eq!(p("RPCS3").status, BiosProjectionStatus::Unsupported);
    }
    #[test]
    fn ambiguity_is_not_confirmed() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::create_dir(dir.path().join("b")).unwrap();
        file(&dir.path().join("a"), "scph1001.bin", b"a");
        file(&dir.path().join("b"), "scph1001.bin", b"b");
        let p = plan_for_emulator(&inspect_master_root(dir.path()).unwrap(), "DuckStation");
        assert_eq!(p.status, BiosProjectionStatus::ReviewRequired);
        assert!(p.warnings.iter().any(|w| w.contains("ambiguous")));
    }

    #[test]
    fn target_inspection_is_read_only_and_distinguishes_shape() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("target");
        assert_eq!(inspect_target(&file_path), BiosTargetState::Missing);
        fs::write(&file_path, b"existing").unwrap();
        assert_eq!(
            inspect_target(&file_path),
            BiosTargetState::ExistingRegularFile
        );
        assert_eq!(fs::read(&file_path).unwrap(), b"existing");
    }

    #[cfg(unix)]
    #[test]
    fn immutable_link_apply_and_rollback_are_source_preserving() {
        let dir = tempdir().unwrap();
        file(dir.path(), "mcpx_1.0.bin", b"mcpx");
        let inventory = inspect_master_root(dir.path()).unwrap();
        let mut plan = plan_for_emulator(&inventory, "xemu");
        plan.requirements.truncate(1);
        plan.matches.truncate(1);
        plan.actions.truncate(1);
        plan.requirements[0].target.path = Some(dir.path().join("target").join("mcpx_1.0.bin"));
        let target_root = dir.path().join("target");
        let source_before = fs::read(dir.path().join("mcpx_1.0.bin")).unwrap();
        let transaction = apply_plan(&plan, &target_root, &apply_confirmation(1)).unwrap();
        assert!(target_root.join("mcpx_1.0.bin").is_symlink());
        assert_eq!(
            fs::read_link(target_root.join("mcpx_1.0.bin")).unwrap(),
            dir.path().join("mcpx_1.0.bin")
        );
        rollback_plan(&transaction).unwrap();
        assert!(!target_root.join("mcpx_1.0.bin").exists());
        assert_eq!(
            fs::read(dir.path().join("mcpx_1.0.bin")).unwrap(),
            source_before
        );
    }

    #[test]
    fn writable_state_is_refused_before_any_target_change() {
        let dir = tempdir().unwrap();
        file(dir.path(), "eeprom.bin", b"state");
        let inventory = inspect_master_root(dir.path()).unwrap();
        let mut plan = plan_for_emulator(&inventory, "xemu");
        plan.requirements = vec![plan.requirements[2].clone()];
        plan.matches = vec![plan.matches[2].clone()];
        plan.actions = vec![plan.actions[2].clone()];
        let error = apply_plan(
            &plan,
            dir.path(),
            &apply_confirmation(plan.requirements.len()),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            BiosProjectionApplyError::WritableStateNotSupported(_)
        ));
        assert!(!dir.path().join("eeprom.bin").is_symlink());
    }
}
