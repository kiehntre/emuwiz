//! The record for one discovered save, and the evidence behind it.

use crate::game_identity::IdentityEvidence;
use crate::save_snapshots::SaveArtifactType;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a save is, decided from its content where the format allows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveFormat {
    /// One save on a PlayStation memory card.
    Ps1CardSave,
    /// One save folder on a PlayStation 2 memory card.
    Ps2CardSave,
    /// One GameCube save file (`.gci`), as Dolphin keeps them.
    GameCubeGci,
    /// A PlayStation 3 save folder with a `PARAM.SFO` (RPCS3).
    Ps3Savedata,
    /// A PSP save folder with a `PARAM.SFO` (PPSSPP).
    PspSavedata,
    /// A cartridge's battery memory as a libretro core writes it
    /// (`.srm`, `.sav`, `.eep`, `.fla`...). The file carries no game identity.
    CartridgeBattery,
    /// An emulator save state: a dump of one emulator build's internals.
    SaveState,
}

impl SaveFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ps1CardSave => "PlayStation memory-card save",
            Self::Ps2CardSave => "PlayStation 2 memory-card save",
            Self::GameCubeGci => "GameCube save file (.gci)",
            Self::Ps3Savedata => "PlayStation 3 save folder",
            Self::PspSavedata => "PSP save folder",
            Self::CartridgeBattery => "Cartridge battery save",
            Self::SaveState => "Save state",
        }
    }

    pub fn artifact_type(self) -> SaveArtifactType {
        match self {
            Self::Ps1CardSave | Self::Ps2CardSave => SaveArtifactType::PsMemoryCard,
            Self::GameCubeGci => SaveArtifactType::DolphinMemoryCard,
            Self::Ps3Savedata => SaveArtifactType::SaveDirectory,
            Self::PspSavedata => SaveArtifactType::PspSavedata,
            Self::CartridgeBattery => SaveArtifactType::Sram,
            Self::SaveState => SaveArtifactType::SaveState,
        }
    }
}

/// How firmly a save is tied to a game. Ordered weakest to strongest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationStrength {
    /// Nothing ties this save to a game.
    None,
    /// Only the file name suggests a game. A hint, never an association.
    NameOnly,
    /// The emulator names saves after the game file it ran, and its own play
    /// history lists exactly one game file with this name. The save's bytes
    /// do not say which game wrote them.
    EmulatorConvention,
    /// The game's own identifier is written inside the save
    /// (a product code, a GameCube game code, a `PARAM.SFO` title id).
    ContentProven,
}

impl AssociationStrength {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Not tied to a game",
            Self::NameOnly => "File name only (a hint, not confirmed)",
            Self::EmulatorConvention => {
                "Named by the emulator after the game it ran (not confirmed by the save itself)"
            }
            Self::ContentProven => "The game's identifier is written inside the save",
        }
    }
}

/// Which game a save belongs to, and how that is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveIdentity {
    pub strength: AssociationStrength,
    /// For example `SLUS-20827` or `GEDE01`.
    pub product_code: Option<String>,
    pub region: Option<String>,
    /// The title the game wrote into the save, when there is one.
    pub title: Option<String>,
    pub evidence: Vec<IdentityEvidence>,
    /// Why this is not certain, in plain words. Empty when nothing is in doubt.
    pub ambiguity: Vec<String>,
}

impl SaveIdentity {
    pub fn none(why: &str) -> Self {
        Self {
            strength: AssociationStrength::None,
            product_code: None,
            region: None,
            title: None,
            evidence: Vec::new(),
            ambiguity: vec![why.to_string()],
        }
    }
}

/// Whether the save may be in use right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InUse {
    /// A reviewed check found the emulator closed and nothing holding the file.
    NotDetected,
    EmulatorRunning,
    /// Some program has the file open for writing.
    OpenByAProgram,
    /// EmuWiz has no reliable check for this emulator. Treated as "may be in use".
    Unknown,
}

impl InUse {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotDetected => "Not in use",
            Self::EmulatorRunning => "The emulator is running",
            Self::OpenByAProgram => "A program has this save open",
            Self::Unknown => "Not checked: EmuWiz cannot tell for this emulator",
        }
    }
}

/// One save found on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredSave {
    /// Stable for the same file and entry: 16 hex digits.
    pub id: String,
    /// The file or folder on disk. For a card save, the card.
    pub source_path: PathBuf,
    /// The save's name inside its container (card entry / folder name).
    pub entry_name: Option<String>,
    pub emulator: String,
    /// Which install of the emulator: `native` or `flatpak`.
    pub profile: String,
    /// Only when established; EmuWiz does not guess a version.
    pub emulator_version: Option<String>,
    pub platform: Option<String>,
    pub format: SaveFormat,
    pub artifact_type: SaveArtifactType,
    /// Bytes of this save (for a card save: its files on the card).
    pub size_bytes: u64,
    /// SHA-256 of this save's own content, when it could be read.
    pub content_sha256: Option<String>,
    /// SHA-256 of the whole container file, for a save inside a card.
    pub container_sha256: Option<String>,
    /// The file's modification time on disk (seconds since 1970).
    pub modified_unix: Option<i64>,
    /// The time the console/emulator wrote inside the save, as stored.
    pub internal_modified: Option<String>,
    pub identity: SaveIdentity,
    pub in_use: InUse,
    /// What was actually checked about the bytes.
    pub integrity: Vec<String>,
    pub notes: Vec<String>,
    /// What EmuWiz cannot do with this save, stated plainly.
    pub unsupported: Vec<String>,
}

pub(crate) fn stable_id(path: &std::path::Path, entry: Option<&str>) -> String {
    let mut key = path.to_string_lossy().into_owned().into_bytes();
    key.push(0);
    key.extend_from_slice(entry.unwrap_or("").as_bytes());
    super::sha256_hex(&key)[..16].to_string()
}
