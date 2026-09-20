//! One read-only, provider-neutral view of installed mod transactions.
//!
//! The shared transaction journal and backup store remain authoritative. This
//! module only normalises their existing evidence for history screens; it
//! never writes journals, mutates destinations, or invents missing identity.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::mod_provider::{ModCompatibilityLevel, ModPackageJoin, ModProviderProvenance};
use crate::patch_manager::{
    PreviewAdapter, SharedApplyJournal, SharedApplyOutcome, SharedHistoryReport,
    SharedRollbackOutcome, SharedRollbackPreview, discover_shared_apply_history,
    preview_shared_rollback,
};
use crate::standalone_patch::DerivedPatchProvenance;

pub const MOD_RECEIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModKind {
    Pcsx2Texture,
    PpssppTexture,
    CemuGraphicPack,
    Rpcs3Ordinary,
    LocalPackage,
    StandalonePatch,
}

impl ModKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pcsx2Texture => "PCSX2 texture mod",
            Self::PpssppTexture => "PPSSPP texture mod",
            Self::CemuGraphicPack => "Cemu graphic pack",
            Self::Rpcs3Ordinary => "RPCS3 ordinary mod",
            Self::LocalPackage => "Local mod package",
            Self::StandalonePatch => "Standalone patch",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ModRollbackStatus {
    ReadyToUndo,
    AlreadyUndone,
    CannotSafelyUndo { reason: String },
    NeedsReview { reason: String },
}

/// The trust state of provider information after it reaches local history.
/// This is deliberately separate from the ordinary installer receipt: a
/// provider claim supplements local evidence and never replaces it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModProviderHistoryState {
    VerifiedProviderFile,
    ProviderMetadataOnly,
    BrowserAcquired,
    ExternalHost,
    ChecksumChanged,
    ProviderAssociationUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModProviderHistoryAttachError {
    CompatibilityNotVerified,
    ChecksumMismatch,
    JoinProviderMismatch,
    JoinFileMismatch,
}

impl std::fmt::Display for ModProviderHistoryAttachError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CompatibilityNotVerified => "provider compatibility is not verified",
            Self::ChecksumMismatch => "provider and local checksums do not match",
            Self::JoinProviderMismatch => "local package join belongs to another provider",
            Self::JoinFileMismatch => "local package join belongs to another provider file",
        })
    }
}

impl std::error::Error for ModProviderHistoryAttachError {}

impl ModRollbackStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ReadyToUndo => "Ready to undo",
            Self::AlreadyUndone => "Already undone",
            Self::CannotSafelyUndo { .. } => "Can't safely undo",
            Self::NeedsReview { .. } => "Needs review",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModConflictReference {
    pub transaction_id: String,
    pub destination: String,
    pub later_transaction: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModReceiptSummary {
    pub schema_version: u32,
    pub transaction_id: String,
    pub kind: ModKind,
    pub platform: Option<String>,
    pub emulator: Option<String>,
    pub game_title: Option<String>,
    pub verified_identity: Option<String>,
    pub source_package: Option<String>,
    pub source_fingerprint: Option<String>,
    pub installed_at_unix: u64,
    pub files_created: usize,
    pub files_replaced: usize,
    pub files_unchanged: usize,
    pub destination_root: Option<String>,
    pub journal_path: Option<String>,
    pub backup_root: Option<String>,
    pub rollback: ModRollbackStatus,
    pub conflicts: Vec<ModConflictReference>,
    pub notes: Vec<String>,
    /// Optional provider lineage. Missing on legacy receipts by design.
    #[serde(default)]
    pub provider_provenance: Option<ModProviderProvenance>,
    #[serde(default)]
    pub provider_state: Option<ModProviderHistoryState>,
}

impl ModReceiptSummary {
    pub fn changed_files(&self) -> usize {
        self.files_created + self.files_replaced
    }

    pub fn game_group_key(&self) -> String {
        self.verified_identity
            .clone()
            .or_else(|| self.game_title.clone())
            .unwrap_or_else(|| "unknown-game".into())
    }

    pub fn from_shared_journal(
        journal: &SharedApplyJournal,
        journal_path: Option<&Path>,
        backup_root: &Path,
        rollback: &SharedRollbackPreview,
    ) -> Option<Self> {
        let kind = kind_for_adapter(journal.context.adapter)?;
        let (platform, emulator) = system_for_kind(kind);
        let mut source_paths = BTreeSet::new();
        let mut source_digests = BTreeSet::new();
        let mut created = 0;
        let mut replaced = 0;
        let mut unchanged = 0;
        let mut notes = Vec::new();
        for entry in &journal.entries {
            source_paths.insert(entry.plan_entry.selected_archive.display.clone());
            source_digests.insert(entry.plan_entry.source_digest.clone());
            match entry.outcome {
                SharedApplyOutcome::InstalledNew => created += 1,
                SharedApplyOutcome::ReplacedExisting => replaced += 1,
                SharedApplyOutcome::AlreadyInstalled => unchanged += 1,
                _ => {}
            }
            notes.extend(entry.warnings.iter().cloned());
            notes.extend(entry.failures.iter().map(|failure| failure.detail.clone()));
        }
        let source_fingerprint = digest_set(&source_digests);
        if journal.status != crate::patch_manager::SharedApplyStatus::Success {
            notes.push(format!("Transaction status: {:?}", journal.status));
        }
        Some(Self {
            schema_version: MOD_RECEIPT_SCHEMA_VERSION,
            transaction_id: journal.operation_id.clone(),
            kind,
            platform,
            emulator,
            game_title: None,
            verified_identity: nonempty(&journal.context.verified_game_identity),
            source_package: source_paths.into_iter().next(),
            source_fingerprint,
            installed_at_unix: journal.timestamp_unix_seconds,
            files_created: created,
            files_replaced: replaced,
            files_unchanged: unchanged,
            destination_root: Some(journal.destination_root.display.clone()),
            journal_path: journal_path.map(|path| path.display().to_string()),
            backup_root: Some(backup_root.display().to_string()),
            rollback: rollback_status(journal, rollback),
            conflicts: Vec::new(),
            notes: dedup_sorted(notes),
            provider_provenance: None,
            provider_state: None,
        })
    }

    /// Projects a standalone patch application while being honest that the
    /// existing patch workflow returns provenance but does not persist a
    /// shared rollback journal.
    pub fn from_patch_provenance(provenance: &DerivedPatchProvenance) -> Self {
        Self {
            schema_version: MOD_RECEIPT_SCHEMA_VERSION,
            transaction_id: format!("patch:{}", provenance.output_sha256),
            kind: ModKind::StandalonePatch,
            platform: None,
            emulator: None,
            game_title: None,
            verified_identity: None,
            source_package: Some(provenance.patch_path.display().to_string()),
            source_fingerprint: Some(provenance.patch_sha256.clone()),
            installed_at_unix: provenance.applied_at_unix_seconds,
            files_created: 0,
            files_replaced: 1,
            files_unchanged: 0,
            destination_root: provenance
                .output_path
                .parent()
                .map(|path| path.display().to_string()),
            journal_path: None,
            backup_root: None,
            rollback: ModRollbackStatus::CannotSafelyUndo {
                reason:
                    "This standalone patch has provenance but no shared rollback journal or backup."
                        .into(),
            },
            conflicts: Vec::new(),
            notes: vec![format!(
                "Applied with {} ({:?})",
                provenance.application, provenance.format
            )],
            provider_provenance: None,
            provider_state: None,
        }
    }

    /// Projects a derived patch output while retaining the provider package
    /// lineage. The output remains a standalone derived artifact, not the
    /// original provider file.
    pub fn from_patch_provenance_with_provider(
        provenance: &DerivedPatchProvenance,
        provider: ModProviderProvenance,
    ) -> Result<Self, ModProviderHistoryAttachError> {
        Self::from_patch_provenance(provenance).attach_provider_provenance(provider)
    }

    /// Attaches provider data only after the provider module has already
    /// established a strong checksum-backed association.
    pub fn attach_provider_provenance(
        mut self,
        provider: ModProviderProvenance,
    ) -> Result<Self, ModProviderHistoryAttachError> {
        if provider.compatibility.level != ModCompatibilityLevel::Verified {
            return Err(ModProviderHistoryAttachError::CompatibilityNotVerified);
        }
        if let (Some(reported), Some(local)) = (
            provider.reported_checksum.as_ref(),
            provider.locally_calculated_checksum.as_ref(),
        ) && (reported.algorithm != local.algorithm
            || !reported.value.eq_ignore_ascii_case(&local.value))
        {
            return Err(ModProviderHistoryAttachError::ChecksumMismatch);
        }
        self.provider_state = Some(provider_history_state(&provider));
        self.provider_provenance = Some(provider);
        Ok(self)
    }

    /// Same attachment seam for the provider-file-ID path. The join object is
    /// produced only by the provider backend's strong local-package join.
    pub fn attach_provider_provenance_after_join(
        self,
        provider: ModProviderProvenance,
        join: &ModPackageJoin,
    ) -> Result<Self, ModProviderHistoryAttachError> {
        if provider.provider != join.provider {
            return Err(ModProviderHistoryAttachError::JoinProviderMismatch);
        }
        if provider.provider_file_id.as_deref() != Some(join.provider_file_id.as_str()) {
            return Err(ModProviderHistoryAttachError::JoinFileMismatch);
        }
        let Some(local) = provider.locally_calculated_checksum.as_ref() else {
            return Err(ModProviderHistoryAttachError::ChecksumMismatch);
        };
        if local != &join.local_checksum {
            return Err(ModProviderHistoryAttachError::ChecksumMismatch);
        }
        self.attach_provider_provenance(provider)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModHistoryFilter {
    pub game: Option<String>,
    pub platform: Option<String>,
    pub emulator: Option<String>,
    pub kind: Option<ModKind>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModHistory {
    pub receipts: Vec<ModReceiptSummary>,
    pub problems: Vec<String>,
    pub complete: bool,
}

impl ModHistory {
    pub fn from_shared_history(history_root: &Path, backup_root: &Path) -> Self {
        let report = discover_shared_apply_history(history_root);
        Self::from_report(&report, backup_root)
    }

    pub fn from_report(report: &SharedHistoryReport, backup_root: &Path) -> Self {
        let mut history = Self {
            receipts: report
                .journals
                .iter()
                .filter_map(|(path, journal)| {
                    let journal_path = path.to_path_buf().ok();
                    let destination = journal.destination_root.to_path_buf().ok()?;
                    let rollback = preview_shared_rollback(
                        journal_path.as_deref()?,
                        &destination,
                        backup_root,
                    );
                    ModReceiptSummary::from_shared_journal(
                        journal,
                        journal_path.as_deref(),
                        backup_root,
                        &rollback,
                    )
                })
                .collect(),
            problems: report
                .warnings
                .iter()
                .map(|warning| format!("{}: {}", warning.path.display, warning.failure.detail))
                .collect(),
            complete: report.complete,
        };
        history.apply_conflict_references();
        history.sort_newest_first();
        history
    }

    pub fn filtered(&self, filter: &ModHistoryFilter) -> Vec<&ModReceiptSummary> {
        self.receipts
            .iter()
            .filter(|receipt| {
                filter
                    .game
                    .as_deref()
                    .is_none_or(|value| receipt.game_group_key().eq_ignore_ascii_case(value))
            })
            .filter(|receipt| {
                filter
                    .platform
                    .as_deref()
                    .is_none_or(|value| receipt.platform.as_deref() == Some(value))
            })
            .filter(|receipt| {
                filter
                    .emulator
                    .as_deref()
                    .is_none_or(|value| receipt.emulator.as_deref() == Some(value))
            })
            .filter(|receipt| filter.kind.is_none_or(|value| receipt.kind == value))
            .collect()
    }

    pub fn grouped_by_game(&self) -> BTreeMap<String, Vec<&ModReceiptSummary>> {
        let mut groups = BTreeMap::new();
        for receipt in &self.receipts {
            groups
                .entry(receipt.game_group_key())
                .or_insert_with(Vec::new)
                .push(receipt);
        }
        groups
    }

    fn sort_newest_first(&mut self) {
        self.receipts.sort_by(|left, right| {
            right
                .installed_at_unix
                .cmp(&left.installed_at_unix)
                .then_with(|| left.transaction_id.cmp(&right.transaction_id))
        });
    }

    fn apply_conflict_references(&mut self) {
        let mut destinations: BTreeMap<String, Vec<(usize, u64)>> = BTreeMap::new();
        for (index, receipt) in self.receipts.iter().enumerate() {
            // The journal is the authoritative file list; the summary counts
            // are intentionally not expanded into guessed paths.
            let Some(journal_path) = receipt.journal_path.as_deref() else {
                continue;
            };
            let Ok(bytes) = std::fs::read(journal_path) else {
                continue;
            };
            let Ok(journal) = serde_json::from_slice::<SharedApplyJournal>(&bytes) else {
                continue;
            };
            for entry in journal.entries {
                if matches!(
                    entry.outcome,
                    SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
                ) {
                    let path = format!(
                        "{}/{}",
                        entry.plan_entry.destination_root.display,
                        entry.plan_entry.destination_relative_path.display
                    );
                    destinations
                        .entry(path)
                        .or_default()
                        .push((index, receipt.installed_at_unix));
                }
            }
        }
        for (destination, entries) in destinations {
            if entries.len() < 2 {
                continue;
            }
            for (index, timestamp) in &entries {
                for (other, other_timestamp) in &entries {
                    if index == other {
                        continue;
                    }
                    let other_transaction_id = self.receipts[*other].transaction_id.clone();
                    self.receipts[*index].conflicts.push(ModConflictReference {
                        transaction_id: other_transaction_id,
                        destination: destination.clone(),
                        later_transaction: *other_timestamp > *timestamp,
                    });
                }
            }
        }
        for receipt in &mut self.receipts {
            receipt.conflicts.sort_by(|left, right| {
                left.destination
                    .cmp(&right.destination)
                    .then_with(|| left.transaction_id.cmp(&right.transaction_id))
            });
            receipt.conflicts.dedup();
            if !receipt.conflicts.is_empty() {
                receipt.notes.push("Potential mod conflict: another transaction touched one or more of the same destination files.".into());
            }
            receipt.notes = dedup_sorted(std::mem::take(&mut receipt.notes));
        }
    }
}

fn kind_for_adapter(adapter: PreviewAdapter) -> Option<ModKind> {
    match adapter {
        PreviewAdapter::Pcsx2 => Some(ModKind::Pcsx2Texture),
        PreviewAdapter::Ppsspp => Some(ModKind::PpssppTexture),
        PreviewAdapter::CemuGraphicPack => Some(ModKind::CemuGraphicPack),
        PreviewAdapter::Rpcs3OrdinaryMod => Some(ModKind::Rpcs3Ordinary),
        PreviewAdapter::LocalModPackage => Some(ModKind::LocalPackage),
        PreviewAdapter::RetroArch | PreviewAdapter::Dolphin | PreviewAdapter::Xenia => None,
    }
}

fn system_for_kind(kind: ModKind) -> (Option<String>, Option<String>) {
    match kind {
        ModKind::Pcsx2Texture => (Some("PS2".into()), Some("PCSX2".into())),
        ModKind::PpssppTexture => (Some("PSP".into()), Some("PPSSPP".into())),
        ModKind::CemuGraphicPack => (Some("Wii U".into()), Some("Cemu".into())),
        ModKind::Rpcs3Ordinary => (Some("PS3".into()), Some("RPCS3".into())),
        ModKind::LocalPackage | ModKind::StandalonePatch => (None, None),
    }
}

fn provider_history_state(provider: &ModProviderProvenance) -> ModProviderHistoryState {
    if let (Some(reported), Some(local)) = (
        provider.reported_checksum.as_ref(),
        provider.locally_calculated_checksum.as_ref(),
    ) && (reported.algorithm != local.algorithm
        || !reported.value.eq_ignore_ascii_case(&local.value))
    {
        return ModProviderHistoryState::ChecksumChanged;
    }
    if provider.external_host {
        return ModProviderHistoryState::ExternalHost;
    }
    if provider.locally_calculated_checksum.is_some() {
        return ModProviderHistoryState::VerifiedProviderFile;
    }
    if provider.acquisition_mode == crate::mod_provider::ModAcquisitionMode::BrowserRequired {
        return ModProviderHistoryState::BrowserAcquired;
    }
    ModProviderHistoryState::ProviderMetadataOnly
}

fn rollback_status(
    journal: &SharedApplyJournal,
    preview: &SharedRollbackPreview,
) -> ModRollbackStatus {
    if !journal.entries.is_empty()
        && journal
            .entries
            .iter()
            .all(|entry| entry.outcome == SharedApplyOutcome::AlreadyInstalled)
    {
        return ModRollbackStatus::AlreadyUndone;
    }
    if journal.rollback_operation_id.is_some()
        || preview
            .entries
            .iter()
            .all(|entry| entry.outcome == SharedRollbackOutcome::AlreadyRolledBack)
    {
        return ModRollbackStatus::AlreadyUndone;
    }
    if preview.available {
        return ModRollbackStatus::ReadyToUndo;
    }
    let reason = preview
        .entries
        .iter()
        .find_map(|entry| match entry.outcome {
            SharedRollbackOutcome::DestinationChanged => Some(
                "One or more installed files have changed since this mod was installed.".into(),
            ),
            SharedRollbackOutcome::BackupMissing => Some(
                "A required backup is missing; EmuWiz will not remove the destination file.".into(),
            ),
            SharedRollbackOutcome::BackupChanged => {
                Some("A required backup changed after install; review is required.".into())
            }
            SharedRollbackOutcome::DestinationMissing => {
                Some("An installed destination file is missing; review is required.".into())
            }
            SharedRollbackOutcome::DestinationUnsafe | SharedRollbackOutcome::RootMismatch => {
                Some("The destination no longer matches the recorded safe root.".into())
            }
            SharedRollbackOutcome::JournalMalformed | SharedRollbackOutcome::JournalUnsupported => {
                Some("The saved transaction journal is malformed or unsupported.".into())
            }
            _ => None,
        })
        .unwrap_or_else(|| "The transaction has no complete proof that rollback is safe.".into());
    if journal.status == crate::patch_manager::SharedApplyStatus::Success {
        ModRollbackStatus::CannotSafelyUndo { reason }
    } else {
        ModRollbackStatus::NeedsReview { reason }
    }
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

fn digest_set(values: &BTreeSet<String>) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn dedup_sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_catalogue::{ModCatalogueHash, ModCatalogueHashAlgorithm};
    use crate::mod_provider::{ModAcquisitionMode, ModCompatibilityAssessment, ModProviderId};
    use crate::patch_manager::{
        PreviewDestinationState, PreviewProposedAction, SharedApplyContext, SharedApplyEntry,
        SharedApplyStatus, SharedPlanEntry, SharedTransactionPath,
    };
    use crate::standalone_patch::{HeaderAdjustment, StandalonePatchFormat};
    use std::path::PathBuf;

    fn provider_provenance(
        mode: ModAcquisitionMode,
        external_host: bool,
        local_checksum: Option<&str>,
        level: ModCompatibilityLevel,
    ) -> ModProviderProvenance {
        ModProviderProvenance {
            schema_version: 1,
            provider: ModProviderId::new("moddb").unwrap(),
            provider_item_id: "darkwatch".into(),
            provider_release_id: Some("darkwatch-release".into()),
            provider_file_id: Some("darkwatch-file".into()),
            canonical_url: "https://www.moddb.com/addons/darkwatch".into(),
            acquisition_mode: mode,
            reported_filename: Some("SLES-53564.rar".into()),
            reported_checksum: Some(ModCatalogueHash {
                algorithm: ModCatalogueHashAlgorithm::Md5,
                value: "0123456789abcdef0123456789abcdef".into(),
            }),
            locally_calculated_checksum: local_checksum.map(|value| ModCatalogueHash {
                algorithm: ModCatalogueHashAlgorithm::Md5,
                value: value.into(),
            }),
            acquired_at_unix_secs: Some(1_700_000_000),
            compatibility: ModCompatibilityAssessment {
                level,
                matched_identity: Vec::new(),
                reasons: vec!["verified local fixture identity".into()],
            },
            external_host,
        }
    }

    fn path(value: &str) -> SharedTransactionPath {
        SharedTransactionPath::from_path(Path::new(value))
    }

    fn journal(
        id: &str,
        timestamp: u64,
        destination: &str,
        outcome: SharedApplyOutcome,
    ) -> SharedApplyJournal {
        SharedApplyJournal {
            schema_version: 1,
            operation_id: id.into(),
            plan_id: format!("plan-{id}"),
            timestamp_unix_seconds: timestamp,
            context: SharedApplyContext {
                adapter: PreviewAdapter::Rpcs3OrdinaryMod,
                selected_archive: path("/mods/last-of-us.zip"),
                verified_game_identity: "BLUS12345".into(),
                profile_id: "rpcs3-profile".into(),
                source_mode: "rpcs3_ordinary_mod".into(),
            },
            approved_source_root: path("/mods/source"),
            destination_root: path("/rpcs3/dev_hdd0/game/BLUS12345"),
            created_root_directories: Vec::new(),
            dry_run: false,
            entries: vec![SharedApplyEntry {
                plan_entry: SharedPlanEntry {
                    adapter: PreviewAdapter::Rpcs3OrdinaryMod,
                    selected_archive: path("/mods/last-of-us.zip"),
                    verified_game_identity: "BLUS12345".into(),
                    source_path: path("/mods/source/a.bin"),
                    source_digest: "a".repeat(64),
                    destination_root: path("/rpcs3/dev_hdd0/game/BLUS12345"),
                    destination_relative_path: path(destination),
                    destination_pre_state: PreviewDestinationState::Missing,
                    destination_pre_digest: None,
                    proposed_action: PreviewProposedAction::Install,
                    backup_required: false,
                    parent_creation_approved: true,
                    content_verification: None,
                },
                destination_existed_before_apply: Some(false),
                destination_parent_existed_before_apply: Some(true),
                observed_source_digest: Some("a".repeat(64)),
                observed_destination_digest: None,
                backup_path: None,
                backup_digest: None,
                temporary_path: None,
                final_destination_digest: Some("b".repeat(64)),
                created_directories: Vec::new(),
                replacement_approved: true,
                verification_succeeded: true,
                outcome,
                stages: Vec::new(),
                warnings: Vec::new(),
                failures: Vec::new(),
            }],
            status: SharedApplyStatus::Success,
            rollback_operation_id: None,
        }
    }

    fn preview(
        id: &str,
        destination: &str,
        outcome: SharedRollbackOutcome,
    ) -> SharedRollbackPreview {
        SharedRollbackPreview {
            schema_version: 1,
            preview_id: format!("preview-{id}"),
            journal_path: path(&format!("/history/{id}.json")),
            original_operation_id: id.into(),
            destination_root: path("/rpcs3/dev_hdd0/game/BLUS12345"),
            entries: vec![crate::patch_manager::SharedRollbackEntry {
                destination: Some(path(destination)),
                backup: None,
                expected_installed_digest: Some("b".repeat(64)),
                observed_destination_digest: Some("b".repeat(64)),
                observed_backup_digest: None,
                outcome,
                failure: None,
            }],
            available: outcome == SharedRollbackOutcome::Available,
        }
    }

    #[test]
    fn rpcs3_receipt_projects_identity_counts_and_ready_rollback() {
        let value = ModReceiptSummary::from_shared_journal(
            &journal(
                "rpcs3-1",
                10,
                "USRDIR/a.bin",
                SharedApplyOutcome::InstalledNew,
            ),
            Some(Path::new("/history/rpcs3-1.json")),
            Path::new("/backups"),
            &preview("rpcs3-1", "USRDIR/a.bin", SharedRollbackOutcome::Available),
        )
        .unwrap();
        assert_eq!(value.kind, ModKind::Rpcs3Ordinary);
        assert_eq!(value.verified_identity.as_deref(), Some("BLUS12345"));
        assert_eq!(value.files_created, 1);
        assert_eq!(value.rollback, ModRollbackStatus::ReadyToUndo);
    }

    #[test]
    fn all_shared_mod_adapters_get_stable_kinds() {
        assert_eq!(
            kind_for_adapter(PreviewAdapter::Pcsx2),
            Some(ModKind::Pcsx2Texture)
        );
        assert_eq!(
            kind_for_adapter(PreviewAdapter::CemuGraphicPack),
            Some(ModKind::CemuGraphicPack)
        );
        assert_eq!(
            kind_for_adapter(PreviewAdapter::LocalModPackage),
            Some(ModKind::LocalPackage)
        );
    }

    #[test]
    fn exact_provider_join_projects_into_history_and_survives_rollback_state() {
        let receipt = ModReceiptSummary::from_shared_journal(
            &journal(
                "provider-rpcs3",
                10,
                "USRDIR/a.bin",
                SharedApplyOutcome::InstalledNew,
            ),
            None,
            Path::new("/backups"),
            &preview(
                "provider-rpcs3",
                "USRDIR/a.bin",
                SharedRollbackOutcome::Available,
            ),
        )
        .unwrap()
        .attach_provider_provenance(provider_provenance(
            ModAcquisitionMode::DirectPermitted,
            false,
            Some("0123456789abcdef0123456789abcdef"),
            ModCompatibilityLevel::Verified,
        ))
        .unwrap();
        assert_eq!(
            receipt.provider_state,
            Some(ModProviderHistoryState::VerifiedProviderFile)
        );
        assert_eq!(receipt.rollback, ModRollbackStatus::ReadyToUndo);
        assert!(receipt.provider_provenance.is_some());
    }

    #[test]
    fn checksum_mismatch_and_unverified_title_only_refuse_attachment() {
        let receipt = ModReceiptSummary::from_patch_provenance(&DerivedPatchProvenance {
            base_path: PathBuf::from("/games/base.iso"),
            base_sha256: "a".repeat(64),
            patch_path: PathBuf::from("/mods/patch.ips"),
            patch_sha256: "b".repeat(64),
            format: StandalonePatchFormat::Ips,
            output_path: PathBuf::from("/derived/output.iso"),
            output_sha256: "c".repeat(64),
            expected_source_crc32: None,
            expected_output_crc32: None,
            header_adjustment: HeaderAdjustment::None,
            applied_at_unix_seconds: 1,
            application: "fixture".into(),
        });
        assert_eq!(
            receipt
                .clone()
                .attach_provider_provenance(provider_provenance(
                    ModAcquisitionMode::DirectPermitted,
                    false,
                    Some("ffffffffffffffffffffffffffffffff"),
                    ModCompatibilityLevel::Verified,
                ))
                .unwrap_err(),
            ModProviderHistoryAttachError::ChecksumMismatch
        );
        assert_eq!(
            receipt
                .attach_provider_provenance(provider_provenance(
                    ModAcquisitionMode::DirectPermitted,
                    false,
                    Some("0123456789abcdef0123456789abcdef"),
                    ModCompatibilityLevel::TitleOnly,
                ))
                .unwrap_err(),
            ModProviderHistoryAttachError::CompatibilityNotVerified
        );
    }

    #[test]
    fn browser_external_and_derived_provider_states_are_retained() {
        let browser = ModReceiptSummary::from_patch_provenance(&DerivedPatchProvenance {
            base_path: PathBuf::from("/games/base.iso"),
            base_sha256: "a".repeat(64),
            patch_path: PathBuf::from("/mods/patch.ips"),
            patch_sha256: "b".repeat(64),
            format: StandalonePatchFormat::Ips,
            output_path: PathBuf::from("/derived/output.iso"),
            output_sha256: "c".repeat(64),
            expected_source_crc32: None,
            expected_output_crc32: None,
            header_adjustment: HeaderAdjustment::None,
            applied_at_unix_seconds: 1,
            application: "fixture".into(),
        })
        .attach_provider_provenance(provider_provenance(
            ModAcquisitionMode::BrowserRequired,
            false,
            None,
            ModCompatibilityLevel::Verified,
        ))
        .unwrap();
        assert_eq!(
            browser.provider_state,
            Some(ModProviderHistoryState::BrowserAcquired)
        );
        assert_eq!(browser.source_package.as_deref(), Some("/mods/patch.ips"));

        let external = browser
            .attach_provider_provenance(provider_provenance(
                ModAcquisitionMode::ExternalHost,
                true,
                Some("0123456789abcdef0123456789abcdef"),
                ModCompatibilityLevel::Verified,
            ))
            .unwrap();
        assert_eq!(
            external.provider_state,
            Some(ModProviderHistoryState::ExternalHost)
        );
    }

    #[test]
    fn legacy_receipt_without_provider_fields_still_deserializes() {
        let receipt = ModReceiptSummary::from_patch_provenance(&DerivedPatchProvenance {
            base_path: PathBuf::from("/games/base.iso"),
            base_sha256: "a".repeat(64),
            patch_path: PathBuf::from("/mods/patch.ips"),
            patch_sha256: "b".repeat(64),
            format: StandalonePatchFormat::Ips,
            output_path: PathBuf::from("/derived/output.iso"),
            output_sha256: "c".repeat(64),
            expected_source_crc32: None,
            expected_output_crc32: None,
            header_adjustment: HeaderAdjustment::None,
            applied_at_unix_seconds: 1,
            application: "fixture".into(),
        });
        let mut value = serde_json::to_value(&receipt).unwrap();
        value.as_object_mut().unwrap().remove("provider_provenance");
        value.as_object_mut().unwrap().remove("provider_state");
        let decoded: ModReceiptSummary = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.provider_provenance, None);
        assert_eq!(decoded.provider_state, None);
        assert_eq!(decoded.transaction_id, receipt.transaction_id);
    }

    #[test]
    fn pcsx2_and_cemu_receipts_preserve_created_replaced_and_unchanged_counts() {
        let mut pcsx2 = journal(
            "pcsx2",
            1,
            "textures/a.png",
            SharedApplyOutcome::InstalledNew,
        );
        pcsx2.context.adapter = PreviewAdapter::Pcsx2;
        let mut cemu = journal(
            "cemu",
            2,
            "graphic-pack/rules.txt",
            SharedApplyOutcome::ReplacedExisting,
        );
        cemu.context.adapter = PreviewAdapter::CemuGraphicPack;
        let mut unchanged = journal(
            "unchanged",
            3,
            "same.bin",
            SharedApplyOutcome::AlreadyInstalled,
        );
        unchanged.context.adapter = PreviewAdapter::CemuGraphicPack;
        assert_eq!(
            ModReceiptSummary::from_shared_journal(
                &pcsx2,
                None,
                Path::new("/b"),
                &preview("pcsx2", "textures/a.png", SharedRollbackOutcome::Available)
            )
            .unwrap()
            .kind,
            ModKind::Pcsx2Texture
        );
        assert_eq!(
            ModReceiptSummary::from_shared_journal(
                &cemu,
                None,
                Path::new("/b"),
                &preview(
                    "cemu",
                    "graphic-pack/rules.txt",
                    SharedRollbackOutcome::Available
                )
            )
            .unwrap()
            .files_replaced,
            1
        );
        let unchanged_receipt = ModReceiptSummary::from_shared_journal(
            &unchanged,
            None,
            Path::new("/b"),
            &preview(
                "unchanged",
                "same.bin",
                SharedRollbackOutcome::NoChangeRequired,
            ),
        )
        .unwrap();
        assert_eq!(unchanged_receipt.files_unchanged, 1);
        assert_eq!(unchanged_receipt.rollback, ModRollbackStatus::AlreadyUndone);
    }

    #[test]
    fn rolled_back_and_missing_backup_states_are_fail_safe() {
        let mut rolled_back = journal("rolled", 1, "file.bin", SharedApplyOutcome::InstalledNew);
        rolled_back.rollback_operation_id = Some("rollback-1".into());
        let rolled = ModReceiptSummary::from_shared_journal(
            &rolled_back,
            None,
            Path::new("/b"),
            &preview(
                "rolled",
                "file.bin",
                SharedRollbackOutcome::AlreadyRolledBack,
            ),
        )
        .unwrap();
        assert_eq!(rolled.rollback, ModRollbackStatus::AlreadyUndone);
        let missing = ModReceiptSummary::from_shared_journal(
            &journal("missing", 2, "file.bin", SharedApplyOutcome::InstalledNew),
            None,
            Path::new("/b"),
            &preview("missing", "file.bin", SharedRollbackOutcome::BackupMissing),
        )
        .unwrap();
        assert!(
            matches!(missing.rollback, ModRollbackStatus::CannotSafelyUndo { ref reason } if reason.contains("backup is missing"))
        );
    }

    #[test]
    fn changed_destination_blocks_rollback_with_plain_reason() {
        let value = ModReceiptSummary::from_shared_journal(
            &journal(
                "changed",
                10,
                "file.bin",
                SharedApplyOutcome::ReplacedExisting,
            ),
            None,
            Path::new("/backups"),
            &preview(
                "changed",
                "file.bin",
                SharedRollbackOutcome::DestinationChanged,
            ),
        )
        .unwrap();
        assert!(
            matches!(value.rollback, ModRollbackStatus::CannotSafelyUndo { ref reason } if reason.contains("changed since"))
        );
    }

    #[test]
    fn standalone_patch_projection_preserves_format_and_no_rollback_claim() {
        let value = ModReceiptSummary::from_patch_provenance(&DerivedPatchProvenance {
            base_path: PathBuf::from("/roms/base.rom"),
            base_sha256: "a".repeat(64),
            patch_path: PathBuf::from("/patches/game.ips"),
            patch_sha256: "b".repeat(64),
            format: StandalonePatchFormat::Ips,
            output_path: PathBuf::from("/roms/patched.rom"),
            output_sha256: "c".repeat(64),
            expected_source_crc32: None,
            expected_output_crc32: None,
            header_adjustment: HeaderAdjustment::None,
            applied_at_unix_seconds: 20,
            application: "test applier".into(),
        });
        assert_eq!(value.kind, ModKind::StandalonePatch);
        assert!(matches!(
            value.rollback,
            ModRollbackStatus::CannotSafelyUndo { .. }
        ));
    }

    #[test]
    fn history_orders_groups_and_references_overlapping_destinations() {
        let first = journal("first", 10, "same.bin", SharedApplyOutcome::InstalledNew);
        let second = journal(
            "second",
            20,
            "same.bin",
            SharedApplyOutcome::ReplacedExisting,
        );
        let root = tempfile::tempdir().unwrap();
        let first_path = root.path().join("first.json");
        let second_path = root.path().join("second.json");
        std::fs::write(&first_path, serde_json::to_vec(&first).unwrap()).unwrap();
        std::fs::write(&second_path, serde_json::to_vec(&second).unwrap()).unwrap();
        let report = SharedHistoryReport {
            journals: vec![
                (SharedTransactionPath::from_path(&first_path), first),
                (SharedTransactionPath::from_path(&second_path), second),
            ],
            warnings: Vec::new(),
            complete: true,
        };
        let history = ModHistory::from_report(&report, root.path());
        assert_eq!(history.receipts[0].transaction_id, "second");
        assert_eq!(history.grouped_by_game().len(), 1);
        assert!(
            history
                .filtered(&ModHistoryFilter {
                    kind: Some(ModKind::Rpcs3Ordinary),
                    ..Default::default()
                })
                .len()
                == 2
        );
        assert_eq!(history.receipts[0].conflicts.len(), 1);
        assert_eq!(history.receipts[1].conflicts.len(), 1);
        assert!(
            history.receipts[0]
                .notes
                .iter()
                .any(|note| note.contains("Potential mod conflict"))
        );
    }
}
