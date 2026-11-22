//! The typed vocabulary of the recovery engine.
//!
//! Every [`Finding`] names its evidence, how sure the engine is, what it
//! blocks, and the actions it proposes. Every [`ProposedAction`] says whether
//! it only reads, creates new files, or changes existing state, so a caller
//! (CLI, Blue) can never present a write as harmless.

use std::path::PathBuf;

use serde::Serialize;

/// The game the engine is asked about. The platform is the catalogue's own
/// value and is treated as a claim, never as verified identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoverySubject {
    pub path: PathBuf,
    /// Canonical platform id recorded for this file, if any.
    pub platform_id: Option<String>,
    /// How that platform was recorded (`folder_alias`, `manual`, ...).
    pub platform_source: Option<String>,
}

impl RecoverySubject {
    pub fn new(path: impl Into<PathBuf>, platform_id: Option<&str>) -> Self {
        Self {
            path: path.into(),
            platform_id: platform_id.map(str::to_string),
            platform_source: None,
        }
    }

    pub fn with_platform_source(mut self, source: Option<&str>) -> Self {
        self.platform_source = source.map(str::to_string);
        self
    }
}

/// Where a piece of evidence was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    /// The catalogue row or the caller's statement about the file.
    Catalogue,
    /// File metadata (type, size) read without following symlinks.
    FileSystem,
    /// The ZIP central directory (names, sizes, CRC-32, flags).
    ArchiveIndex,
    /// Decoded bytes of an archive member, checked against its CRC-32.
    MemberContent,
    /// A parsed cue sheet.
    CueSheet,
    /// A structure read from disc image bytes (ISO 9660 descriptor).
    DiscStructure,
    /// A firmware file found in an emulator's own folder.
    FirmwareFile,
    /// The emulator installation discovered on this machine.
    EmulatorInventory,
    /// A receipt written by an earlier verified preparation.
    PreparedReceipt,
    /// The engine's reviewed capability matrix.
    CapabilityMatrix,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Evidence {
    pub source: EvidenceSource,
    pub statement: String,
}

impl Evidence {
    pub fn new(source: EvidenceSource, statement: impl Into<String>) -> Self {
        Self {
            source,
            statement: statement.into(),
        }
    }
}

/// How strongly the evidence supports the finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// A hint only (a name or an extension).
    Low,
    /// Structure agrees, but something is not proven.
    Medium,
    /// Read from the file's own structure and internally consistent.
    High,
    /// Checked against an independent checksum or a fixed rule.
    Certain,
}

/// What a finding stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocking {
    /// The game cannot start until this is resolved.
    Launch,
    /// The engine cannot prepare a playable copy until this is resolved.
    Preparation,
    /// Nothing; information for the person.
    Nothing,
}

/// What performing an action does to the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionEffect {
    /// Reads only.
    ReadOnly,
    /// Creates new files in EmuWiz's own prepared-media folder; never
    /// overwrites or deletes anything that existed before.
    CreatesNewFiles,
    /// Changes or removes something that already exists.
    ChangesExistingState,
}

/// Who carries an action out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionPerformer {
    /// The engine can do it after the person confirms.
    Engine,
    /// Only the person can do it (add their own firmware, install a core).
    Person,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// Extract a verified disc set from its ZIP into the prepared-media folder.
    PrepareDiscSetFromZip,
    /// Re-read a prepared copy and compare it with its receipt.
    VerifyPreparedCopy,
    /// Remove a prepared copy this engine created (the Undo of preparation).
    RemovePreparedCopy,
    /// Start the game from its prepared copy through the normal launch checks.
    PlayPreparedCopy,
    /// Put the person's own firmware file where the emulator reads it.
    AddOwnFirmware,
    /// Install or enable an emulator core.
    InstallEmulatorCore,
    /// Confirm or change the game's system on its Launch page.
    ConfirmSystem,
    /// Use another copy or format; EmuWiz cannot make this file playable.
    UseAnotherCopy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProposedAction {
    pub kind: ActionKind,
    pub effect: ActionEffect,
    pub performer: ActionPerformer,
    pub title: String,
    pub detail: String,
}

impl ProposedAction {
    pub fn new(
        kind: ActionKind,
        effect: ActionEffect,
        performer: ActionPerformer,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            effect,
            performer,
            title: title.into(),
            detail: detail.into(),
        }
    }
}

/// Stable identifiers; the wording may change, these do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCode {
    /// The file is missing, a symlink or not a regular file.
    SourceUnavailable,
    /// The ZIP could not be read safely (corrupt, encrypted, over a limit).
    ArchiveRefused,
    /// A complete disc set (cue sheet and every track it names) is stored
    /// inside a ZIP the emulator cannot open directly.
    DiscSetInsideZip,
    /// The ZIP holds disc files but they do not form one complete, safe set.
    DiscSetIncomplete,
    /// A verified prepared copy of this exact content already exists.
    PreparedCopyReady,
    /// A prepared copy exists but no longer matches its receipt.
    PreparedCopyDamaged,
    /// An earlier preparation was interrupted and left staging files.
    PreparationInterrupted,
    /// The system is known only from the folder the file is in.
    SystemFromFolderOnly,
    /// Required firmware is absent from the emulator's folder.
    FirmwareMissing,
    /// Firmware is present; says whether its checksum is a known release.
    FirmwarePresent,
    /// No installed emulator can run this system.
    EmulatorRouteMissing,
    /// An installed emulator can run this system.
    EmulatorRouteAvailable,
    /// The engine has a reviewed rule that this format cannot be made
    /// playable by any safe preparation.
    FormatUnsupported,
    /// The engine has no rule for this file and says so.
    NotAssessed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: FindingCode,
    pub title: String,
    pub explanation: String,
    pub confidence: Confidence,
    pub blocking: Blocking,
    pub evidence: Vec<Evidence>,
    pub actions: Vec<ProposedAction>,
}

/// The one-line answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryVerdict {
    /// A verified prepared copy exists and nothing found here blocks launch.
    PlayableFromPreparedCopy,
    /// The engine can prepare a playable copy; nothing else blocks.
    FixableByPreparation,
    /// A playable copy exists or can be prepared, but the person must also
    /// do something (firmware, emulator core).
    FixableWithYourHelp,
    /// A reviewed rule says this cannot be made playable from this file.
    Unsupported,
    /// The file could not be read safely.
    Refused,
    /// No rule covers this file; nothing is claimed.
    NotAssessed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryReport {
    pub subject: RecoverySubject,
    pub verdict: RecoveryVerdict,
    pub findings: Vec<Finding>,
    /// The preparation the engine would carry out, when one applies.
    pub plan: Option<super::prepared_media::PreparationPlan>,
    /// The verified prepared copy for this content, when one exists.
    pub prepared: Option<super::prepared_media::PreparedSetSummary>,
    /// The id of a prepared copy for this content that fails its checks and
    /// may be removed, when there is one.
    pub damaged_copy: Option<String>,
    /// A prepared copy with the same file names, sizes and ZIP checksums
    /// exists but is not linked to this file (it was renamed, moved or is a
    /// duplicate). A full content comparison can link it without extracting.
    pub relinkable: bool,
}

impl RecoveryReport {
    pub fn finding(&self, code: FindingCode) -> Option<&Finding> {
        self.findings.iter().find(|finding| finding.code == code)
    }

    /// Every proposed action across all findings, in finding order.
    pub fn actions(&self) -> impl Iterator<Item = &ProposedAction> {
        self.findings
            .iter()
            .flat_map(|finding| finding.actions.iter())
    }
}
