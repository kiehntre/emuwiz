//! Read-only, provider-neutral inventory of emulator persistent state.
//!
//! Callers supply effective paths from their emulator/profile discovery. This
//! module never guesses a default path and never copies, converts, restores,
//! deletes, or uploads state. Opaque multi-game containers remain one record;
//! PS1/PS2 memory-card members are expanded only through the existing,
//! read-only card parser's proven observations.

use crate::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityKind, IdentityProvenance, IdentityStatus,
};
use crate::memory_card_inventory::{
    MemoryCardEntryIdentity, MemoryCardFormat, MemoryCardIdentityConfidence, Ps2SaveDirectory,
    inspect_memory_card,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_INVENTORY_ENTRIES: usize = 4_096;
pub const MAX_RECURSION_DEPTH: usize = 16;
pub const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistentStateType {
    NativeSave,
    MemoryCard,
    SaveState,
    NandOrVirtualDisk,
    ConfigBoundState,
    CloudManaged,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortabilityClass {
    SafeToCopy,
    CopyWithMetadata,
    VersionBound,
    EmulatorBound,
    NeedsReview,
    DoNotTouch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatePathOrigin {
    Native,
    Flatpak,
    Portable,
    ExplicitCustom,
    Configured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateEmulator {
    RetroArch,
    DuckStation,
    Pcsx2,
    Rpcs3,
    Ppsspp,
    Dolphin,
    Flycast,
    Mame,
    Hatari,
    FsUae,
    Xemu,
    Xenia,
    Cemu,
    Vita3k,
    Unknown,
}

impl StateEmulator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetroArch => "RetroArch",
            Self::DuckStation => "DuckStation",
            Self::Pcsx2 => "PCSX2",
            Self::Rpcs3 => "RPCS3",
            Self::Ppsspp => "PPSSPP",
            Self::Dolphin => "Dolphin",
            Self::Flycast => "Flycast",
            Self::Mame => "MAME",
            Self::Hatari => "Hatari",
            Self::FsUae => "FS-UAE",
            Self::Xemu => "xemu",
            Self::Xenia => "Xenia",
            Self::Cemu => "Cemu",
            Self::Vita3k => "Vita3K",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateInstallation {
    pub installation_id: String,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub profile: Option<String>,
    pub firmware_context: Option<String>,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistentStateRoot {
    pub emulator: StateEmulator,
    pub installation: Option<StateInstallation>,
    pub path: PathBuf,
    pub path_origin: StatePathOrigin,
    pub effective_from_configuration: bool,
    pub state_type_hint: Option<PersistentStateType>,
    pub container_path: Option<PathBuf>,
    pub identity_evidence: Vec<IdentityEvidence>,
}

impl PersistentStateRoot {
    pub fn configured(
        emulator: StateEmulator,
        path: impl Into<PathBuf>,
        state_type_hint: Option<PersistentStateType>,
    ) -> Self {
        Self {
            emulator,
            installation: None,
            path: path.into(),
            path_origin: StatePathOrigin::Configured,
            effective_from_configuration: true,
            state_type_hint,
            container_path: None,
            identity_evidence: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistentStateRecord {
    pub emulator: StateEmulator,
    pub selected_installation: Option<StateInstallation>,
    pub state_type: PersistentStateType,
    pub game_identity: Vec<IdentityEvidence>,
    pub path: PathBuf,
    pub container_path: Option<PathBuf>,
    pub slot_profile_account: Option<String>,
    pub emulator_version: Option<String>,
    pub firmware_context: Option<String>,
    pub portability_class: PortabilityClass,
    pub source_path_origin: StatePathOrigin,
    pub provenance: String,
    pub sha256: Option<String>,
    pub size_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistentStateInventory {
    pub records: Vec<PersistentStateRecord>,
    pub warnings: Vec<String>,
    pub roots_inspected: usize,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistentStateInventoryError {
    InvalidRoot(PathBuf, String),
    Io(PathBuf, String),
}

impl std::fmt::Display for PersistentStateInventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot(path, message) => {
                write!(f, "invalid state root {}: {message}", path.display())
            }
            Self::Io(path, message) => write!(
                f,
                "state inventory I/O error at {}: {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for PersistentStateInventoryError {}

pub fn inventory_persistent_state(roots: &[PersistentStateRoot]) -> PersistentStateInventory {
    let mut inventory = PersistentStateInventory {
        records: Vec::new(),
        warnings: Vec::new(),
        roots_inspected: roots.len(),
        read_only: true,
    };
    for root in roots {
        if inventory.records.len() >= MAX_INVENTORY_ENTRIES {
            inventory.warnings.push(format!(
                "inventory entry limit ({MAX_INVENTORY_ENTRIES}) reached; remaining state was not inspected"
            ));
            break;
        }
        if let Err(error) = inspect_root(root, &mut inventory) {
            inventory.warnings.push(error.to_string());
        }
    }
    inventory.records.sort_by(|a, b| {
        a.emulator
            .cmp(&b.emulator)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.state_type.cmp(&b.state_type))
    });
    inventory
}

fn inspect_root(
    root: &PersistentStateRoot,
    inventory: &mut PersistentStateInventory,
) -> Result<(), PersistentStateInventoryError> {
    let metadata = fs::symlink_metadata(&root.path)
        .map_err(|error| PersistentStateInventoryError::Io(root.path.clone(), error.to_string()))?;
    if metadata.file_type().is_symlink() {
        return Err(PersistentStateInventoryError::InvalidRoot(
            root.path.clone(),
            "symlink roots are not followed".into(),
        ));
    }
    if metadata.is_file() {
        inspect_path(root, &root.path, root.container_path.clone(), inventory)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(PersistentStateInventoryError::InvalidRoot(
            root.path.clone(),
            "root is not a regular file or directory".into(),
        ));
    }
    if is_system_container_root(&root.path, root.emulator) {
        add_record(
            root,
            &root.path,
            root.container_path.clone(),
            PersistentStateType::NandOrVirtualDisk,
            vec![
                "opaque system container retained as one record; contained games were not guessed"
                    .into(),
            ],
            inventory,
        )?;
        return Ok(());
    }
    walk_directory(root, &root.path, 0, inventory)
}

fn walk_directory(
    root: &PersistentStateRoot,
    directory: &Path,
    depth: usize,
    inventory: &mut PersistentStateInventory,
) -> Result<(), PersistentStateInventoryError> {
    if depth > MAX_RECURSION_DEPTH {
        inventory.warnings.push(format!(
            "state path depth limit reached at {}",
            directory.display()
        ));
        return Ok(());
    }
    let entries = fs::read_dir(directory).map_err(|error| {
        PersistentStateInventoryError::Io(directory.to_path_buf(), error.to_string())
    })?;
    for item in entries.take(MAX_INVENTORY_ENTRIES) {
        if inventory.records.len() >= MAX_INVENTORY_ENTRIES {
            inventory
                .warnings
                .push("inventory entry limit reached".into());
            break;
        }
        let item = item.map_err(|error| {
            PersistentStateInventoryError::Io(directory.to_path_buf(), error.to_string())
        })?;
        let path = item.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| PersistentStateInventoryError::Io(path.clone(), error.to_string()))?;
        if metadata.file_type().is_symlink() {
            inventory
                .warnings
                .push(format!("symlink skipped: {}", path.display()));
        } else if metadata.is_dir() {
            if should_record_directory(root, &path) {
                inspect_path(root, &path, root.container_path.clone(), inventory)?;
            }
            walk_directory(root, &path, depth + 1, inventory)?;
        } else if metadata.is_file() {
            inspect_path(root, &path, root.container_path.clone(), inventory)?;
        }
    }
    Ok(())
}

fn inspect_path(
    root: &PersistentStateRoot,
    path: &Path,
    container_path: Option<PathBuf>,
    inventory: &mut PersistentStateInventory,
) -> Result<(), PersistentStateInventoryError> {
    if is_memory_card_candidate(path, root.emulator)
        && let Ok(card) = inspect_memory_card(path)
    {
        add_memory_card_records(root, path, card, inventory)?;
        return Ok(());
    }
    let state_type = root
        .state_type_hint
        .unwrap_or_else(|| classify_path(root.emulator, path));
    if state_type == PersistentStateType::Unknown && !should_record_unknown(path) {
        return Ok(());
    }
    let mut warnings = Vec::new();
    if root.emulator == StateEmulator::Xenia {
        warnings.push(
            "Linux Xenia state layout is not stable; explicit configured evidence required".into(),
        );
    }
    if state_type == PersistentStateType::SaveState {
        warnings.push(
            "savestate is emulator/core/version bound; no cross-version portability is claimed"
                .into(),
        );
    }
    add_record(root, path, container_path, state_type, warnings, inventory)
}

fn add_memory_card_records(
    root: &PersistentStateRoot,
    path: &Path,
    card: crate::memory_card_inventory::MemoryCardInventory,
    inventory: &mut PersistentStateInventory,
) -> Result<(), PersistentStateInventoryError> {
    let mut warnings = card.warnings.clone();
    if card.format == MemoryCardFormat::Unknown {
        warnings.push("bytes did not prove a supported card format".into());
    }
    add_record(
        root,
        path,
        None,
        PersistentStateType::MemoryCard,
        warnings,
        inventory,
    )?;
    for entry in card.entries {
        if inventory.records.len() >= MAX_INVENTORY_ENTRIES {
            break;
        }
        let identity = memory_card_identity(&entry.identity, path);
        add_record_with_identity(
            root,
            path,
            Some(path.to_path_buf()),
            PersistentStateType::NativeSave,
            vec![format!("validated card entry: {}", entry.name)],
            identity.into_iter().collect(),
            inventory,
        )?;
    }
    if let Some(ps2) = card.ps2_inventory {
        for directory in ps2.save_directories {
            if inventory.records.len() >= MAX_INVENTORY_ENTRIES {
                break;
            }
            add_ps2_directory_record(root, path, &directory, inventory)?;
        }
    }
    Ok(())
}

fn add_ps2_directory_record(
    root: &PersistentStateRoot,
    card_path: &Path,
    directory: &Ps2SaveDirectory,
    inventory: &mut PersistentStateInventory,
) -> Result<(), PersistentStateInventoryError> {
    let identity = identity_from_name(
        &directory.entry.display_name,
        IdentityKind::Ps2Serial,
        card_path,
        "PS2 card directory name; review evidence only",
    );
    add_record_with_identity(
        root,
        card_path,
        Some(card_path.to_path_buf()),
        PersistentStateType::NativeSave,
        vec![format!(
            "validated PS2 card save directory: {}",
            directory.entry.display_name
        )],
        identity.into_iter().collect(),
        inventory,
    )
}

fn add_record(
    root: &PersistentStateRoot,
    path: &Path,
    container_path: Option<PathBuf>,
    state_type: PersistentStateType,
    warnings: Vec<String>,
    inventory: &mut PersistentStateInventory,
) -> Result<(), PersistentStateInventoryError> {
    add_record_with_identity(
        root,
        path,
        container_path,
        state_type,
        warnings,
        root.identity_evidence.clone(),
        inventory,
    )
}

fn add_record_with_identity(
    root: &PersistentStateRoot,
    path: &Path,
    container_path: Option<PathBuf>,
    state_type: PersistentStateType,
    mut warnings: Vec<String>,
    identity: Vec<IdentityEvidence>,
    inventory: &mut PersistentStateInventory,
) -> Result<(), PersistentStateInventoryError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        PersistentStateInventoryError::Io(path.to_path_buf(), error.to_string())
    })?;
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Ok(());
    }
    let sha256 = if metadata.is_file() && metadata.len() <= MAX_HASH_BYTES {
        Some(
            hash_file(path)
                .map_err(|error| PersistentStateInventoryError::Io(path.to_path_buf(), error))?,
        )
    } else {
        if metadata.is_file() {
            warnings.push(format!(
                "file exceeds bounded hash limit ({MAX_HASH_BYTES} bytes)"
            ));
        }
        None
    };
    let portability_class = portability_for(state_type, !identity.is_empty());
    let provenance = if root.effective_from_configuration {
        "effective configured emulator/profile path; read-only observation".into()
    } else {
        "explicit caller-supplied path; not a guessed default".into()
    };
    inventory.records.push(PersistentStateRecord {
        emulator: root.emulator,
        selected_installation: root.installation.clone(),
        state_type,
        game_identity: identity,
        path: path.to_path_buf(),
        container_path,
        slot_profile_account: root
            .installation
            .as_ref()
            .and_then(|item| item.profile.clone()),
        emulator_version: root
            .installation
            .as_ref()
            .and_then(|item| item.version.clone()),
        firmware_context: root
            .installation
            .as_ref()
            .and_then(|item| item.firmware_context.clone()),
        portability_class,
        source_path_origin: root.path_origin,
        provenance,
        sha256,
        size_bytes: metadata.len(),
        warnings,
    });
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count =
            std::io::Read::read(&mut file, &mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn memory_card_identity(
    identity: &MemoryCardEntryIdentity,
    path: &Path,
) -> Option<IdentityEvidence> {
    identity
        .product_code
        .as_ref()
        .map(|value| IdentityEvidence {
            kind: if value.starts_with(['B', 'S', 'L', 'U', 'E', 'P']) {
                IdentityKind::Ps1Serial
            } else {
                IdentityKind::Ps2Serial
            },
            status: match identity.confidence {
                MemoryCardIdentityConfidence::ConfirmedIdentity => IdentityStatus::Candidate,
                MemoryCardIdentityConfidence::SupportingEvidence => IdentityStatus::Candidate,
                MemoryCardIdentityConfidence::HeuristicOnly => IdentityStatus::Candidate,
                MemoryCardIdentityConfidence::Unknown => IdentityStatus::Missing,
            },
            value: Some(value.clone()),
            confidence: match identity.confidence {
                MemoryCardIdentityConfidence::ConfirmedIdentity => {
                    IdentityConfidence::StructuredMetadata
                }
                MemoryCardIdentityConfidence::SupportingEvidence => {
                    IdentityConfidence::StructuredMetadata
                }
                MemoryCardIdentityConfidence::HeuristicOnly => IdentityConfidence::FilenameOnly,
                MemoryCardIdentityConfidence::Unknown => IdentityConfidence::Unavailable,
            },
            provenance: IdentityProvenance {
                archive_path: path.to_path_buf(),
                member_path: None,
                member_index: None,
                method: "read-only memory-card entry inspection".into(),
            },
            diagnostic: identity.evidence.join("; "),
        })
}

fn identity_from_name(
    name: &str,
    kind: IdentityKind,
    path: &Path,
    diagnostic: &str,
) -> Option<IdentityEvidence> {
    let value = name
        .split(|character: char| !character.is_ascii_alphanumeric())
        .find(|part| {
            part.len() >= 8 && part.len() <= 16 && part.chars().any(|c| c.is_ascii_digit())
        })?;
    Some(IdentityEvidence {
        kind,
        status: IdentityStatus::Candidate,
        value: Some(value.to_string()),
        confidence: IdentityConfidence::FilenameOnly,
        provenance: IdentityProvenance {
            archive_path: path.to_path_buf(),
            member_path: None,
            member_index: None,
            method: "bounded container-name review evidence".into(),
        },
        diagnostic: diagnostic.into(),
    })
}

fn classify_path(emulator: StateEmulator, path: &Path) -> PersistentStateType {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "mlc01" | "dev_hdd0" | "nand" | "hdd" | "eeprom"
    ) || matches!(
        emulator,
        StateEmulator::Xemu | StateEmulator::Rpcs3 | StateEmulator::Cemu
    ) && matches!(extension.as_str(), "vhd" | "vhdx" | "qcow2" | "img" | "bin")
    {
        return PersistentStateType::NandOrVirtualDisk;
    }
    if matches!(extension.as_str(), "state" | "savestate" | "sta" | "sst") {
        return PersistentStateType::SaveState;
    }
    if matches!(extension.as_str(), "mcr" | "mcd" | "mc2" | "ps2" | "vmu") {
        return PersistentStateType::MemoryCard;
    }
    if emulator == StateEmulator::Mame && extension == "cfg" {
        return PersistentStateType::ConfigBoundState;
    }
    if matches!(emulator, StateEmulator::Hatari | StateEmulator::FsUae)
        && matches!(extension.as_str(), "st" | "msa" | "adf" | "hdf")
    {
        return PersistentStateType::NativeSave;
    }
    if emulator == StateEmulator::Ppsspp && name == "savedata" {
        return PersistentStateType::NativeSave;
    }
    if matches!(extension.as_str(), "sav" | "save" | "dat" | "nv" | "nvram") {
        return PersistentStateType::NativeSave;
    }
    if emulator == StateEmulator::Pcsx2 && extension == "psu" {
        return PersistentStateType::NativeSave;
    }
    PersistentStateType::Unknown
}

fn portability_for(state_type: PersistentStateType, has_identity: bool) -> PortabilityClass {
    match state_type {
        PersistentStateType::NativeSave if has_identity => PortabilityClass::SafeToCopy,
        PersistentStateType::NativeSave => PortabilityClass::NeedsReview,
        PersistentStateType::MemoryCard => PortabilityClass::CopyWithMetadata,
        PersistentStateType::SaveState => PortabilityClass::EmulatorBound,
        PersistentStateType::NandOrVirtualDisk => PortabilityClass::NeedsReview,
        PersistentStateType::ConfigBoundState => PortabilityClass::NeedsReview,
        PersistentStateType::CloudManaged | PersistentStateType::Unknown => {
            PortabilityClass::DoNotTouch
        }
    }
}

fn is_memory_card_candidate(path: &Path, emulator: StateEmulator) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(extension.as_str(), "mcr" | "mcd" | "mc2" | "ps2")
        || matches!(emulator, StateEmulator::DuckStation | StateEmulator::Pcsx2)
            && extension.is_empty()
}

fn is_system_container_root(path: &Path, emulator: StateEmulator) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(name.as_str(), "mlc01" | "dev_hdd0" | "nand")
        || matches!(emulator, StateEmulator::Xemu) && matches!(name.as_str(), "hdd" | "eeprom")
}

fn should_record_directory(root: &PersistentStateRoot, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    root.state_type_hint.is_some()
        || matches!(
            name.as_str(),
            "savedata" | "states" | "savestates" | "nvram" | "cfg" | "mlc01" | "dev_hdd0"
        )
}

fn should_record_unknown(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            name.starts_with("save") || name.starts_with("state") || name.starts_with("card")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn inventory_is_read_only_and_classifies_state_types() {
        let directory = tempdir().unwrap();
        let save = directory.path().join("game.sav");
        let state = directory.path().join("slot0.state");
        fs::write(&save, b"native-save").unwrap();
        fs::write(&state, b"savestate").unwrap();
        let before = (
            fs::metadata(&save).unwrap().len(),
            fs::metadata(&state).unwrap().len(),
        );
        let root =
            PersistentStateRoot::configured(StateEmulator::DuckStation, directory.path(), None);
        let inventory = inventory_persistent_state(&[root]);
        assert!(inventory.read_only);
        assert!(
            inventory
                .records
                .iter()
                .any(|record| record.state_type == PersistentStateType::NativeSave)
        );
        assert!(
            inventory
                .records
                .iter()
                .any(|record| record.state_type == PersistentStateType::SaveState)
        );
        assert_eq!(
            before,
            (
                fs::metadata(&save).unwrap().len(),
                fs::metadata(&state).unwrap().len()
            )
        );
    }

    #[test]
    fn configured_flatpak_and_custom_roots_are_preserved() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("PSP/SAVEDATA/ULUS12345");
        fs::create_dir_all(&path).unwrap();
        let file = path.join("PARAM.SFO");
        fs::File::create(&file)
            .unwrap()
            .write_all(b"synthetic")
            .unwrap();
        let mut root = PersistentStateRoot::configured(
            StateEmulator::Ppsspp,
            directory.path(),
            Some(PersistentStateType::NativeSave),
        );
        root.path_origin = StatePathOrigin::Flatpak;
        root.effective_from_configuration = true;
        let inventory = inventory_persistent_state(&[root]);
        assert!(
            inventory
                .records
                .iter()
                .all(|record| record.source_path_origin == StatePathOrigin::Flatpak)
        );
        assert!(
            inventory
                .records
                .iter()
                .any(|record| record.path.ends_with("PARAM.SFO"))
        );
    }

    #[test]
    fn ambiguous_identity_is_not_promoted_to_safe_native_save() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("ULUS12345.sav"), b"save").unwrap();
        let root = PersistentStateRoot::configured(StateEmulator::Ppsspp, directory.path(), None);
        let inventory = inventory_persistent_state(&[root]);
        let record = inventory
            .records
            .iter()
            .find(|record| record.path.ends_with("ULUS12345.sav"))
            .unwrap();
        assert_eq!(record.portability_class, PortabilityClass::NeedsReview);
    }

    #[test]
    fn missing_root_is_reported_without_creation() {
        let directory = tempdir().unwrap();
        let missing = directory.path().join("does-not-exist");
        let inventory = inventory_persistent_state(&[PersistentStateRoot::configured(
            StateEmulator::Mame,
            &missing,
            None,
        )]);
        assert!(inventory.records.is_empty());
        assert!(!missing.exists());
        assert!(
            inventory
                .warnings
                .iter()
                .any(|warning| warning.contains("does-not-exist"))
        );
    }

    #[test]
    fn system_containers_are_not_flattened() {
        let directory = tempdir().unwrap();
        let mlc = directory.path().join("mlc01");
        fs::create_dir_all(mlc.join("usr/save/00050000")).unwrap();
        fs::write(mlc.join("usr/save/00050000/save.dat"), b"opaque").unwrap();
        let inventory = inventory_persistent_state(&[PersistentStateRoot::configured(
            StateEmulator::Cemu,
            &mlc,
            None,
        )]);
        assert_eq!(inventory.records.len(), 1);
        assert_eq!(
            inventory.records[0].state_type,
            PersistentStateType::NandOrVirtualDisk
        );
    }

    #[test]
    fn multiple_installations_and_version_context_are_preserved() {
        let directory = tempdir().unwrap();
        let save = directory.path().join("save.dat");
        fs::write(&save, b"save").unwrap();
        let mut first = PersistentStateRoot::configured(
            StateEmulator::Pcsx2,
            &save,
            Some(PersistentStateType::NativeSave),
        );
        first.installation = Some(StateInstallation {
            installation_id: "native-old".into(),
            executable: None,
            version: Some("1.6.0".into()),
            profile: Some("default".into()),
            firmware_context: Some("BIOS-A".into()),
            selected: false,
        });
        let mut second = first.clone();
        second.installation = Some(StateInstallation {
            installation_id: "flatpak-new".into(),
            executable: None,
            version: Some("2.0.0".into()),
            profile: Some("default".into()),
            firmware_context: Some("BIOS-B".into()),
            selected: true,
        });
        second.path_origin = StatePathOrigin::Flatpak;
        let inventory = inventory_persistent_state(&[first, second]);
        assert_eq!(inventory.records.len(), 2);
        assert!(
            inventory
                .records
                .iter()
                .any(|record| record.emulator_version.as_deref() == Some("1.6.0"))
        );
        assert!(inventory.records.iter().any(|record| {
            record
                .selected_installation
                .as_ref()
                .is_some_and(|item| item.selected)
        }));
    }
}
