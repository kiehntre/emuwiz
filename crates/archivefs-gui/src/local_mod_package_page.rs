//! Beginner-facing local, ordinary (non-cheat) mod package workflow.
//! Inspection is read-only; applying and undoing use the shared transaction
//! journal and backup machinery.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

use archivefs_core::game_identity::GameIdentityReport;
use archivefs_core::mod_catalogue::ModCatalogueRecord;
use archivefs_core::mod_catalogue_review::review_catalogue_record;
use archivefs_core::mod_package::{
    LocalModPackageCandidateInspection, LocalModPackagePlan, ModCompatibilityState,
    SelectedGameForMod, build_local_mod_package_transaction_plan,
    inspect_local_mod_package_candidates,
};
use archivefs_core::patch_manager::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyOutcome, SharedApplyResult,
    SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions, SharedRollbackPreview,
    SharedTransactionPlan, default_shared_backup_root, default_shared_history_root,
    execute_shared_apply, execute_shared_rollback, generate_shared_operation_id,
    preview_shared_rollback,
};
use archivefs_core::rom_hack_catalogue::{
    CatalogueField, RomHackCatalogueImportPreview, RomHackCatalogueMapping,
    commit_local_rom_hack_catalogue, inspect_local_rom_hack_catalogue,
    inspect_local_rom_hack_catalogue_member,
};
use archivefs_core::standalone_patch::{
    HeaderAdjustment, MAX_APPLY_BYTES, PatchCompatibility, StandalonePatchApplyPlan,
    StandalonePatchApplyResult, StandalonePatchInspection, StandalonePatchMatch,
    apply_standalone_patch, build_standalone_patch_apply_plan_with_header,
    inspect_standalone_patch, match_patch_source_with_header,
};
use eframe::egui;

use crate::ui::components as widgets;

enum Stage {
    Pick(Receiver<Option<PathBuf>>, SelectedGameForMod, Vec<PathBuf>),
    Candidates(LocalModPackageCandidateInspection, usize),
    #[allow(dead_code)]
    Planned(LocalModPackagePlan),
    Confirm(SharedTransactionPlan),
    Applying(Receiver<SharedApplyResult>, SharedTransactionPlan),
    Applied(SharedApplyResult),
    Rollback(SharedRollbackPreview),
    RollingBack(Receiver<archivefs_core::patch_manager::SharedRollbackResult>),
    /// A rollback worker reported back. Carries the honest final status so
    /// the render arm can present success, partial, or failed distinctly -
    /// a successful undo must never reach `Stage::Failed`.
    RolledBack(SharedApplyStatus),
    Failed(String),
}

#[derive(Default)]
pub struct LocalModPackagePageState {
    key: Option<(PathBuf, PathBuf)>,
    stage: Option<Stage>,
    provider_selection: Option<(String, String)>,
    standalone: StandalonePatchPageState,
}

#[derive(Default)]
struct StandalonePatchPageState {
    picker: Option<Receiver<Result<StandalonePatchCandidate, String>>>,
    candidates: Vec<StandalonePatchCandidate>,
    selected: usize,
    applying: Option<Receiver<Result<StandalonePatchApplyResult, String>>>,
    applied: Option<StandalonePatchApplyResult>,
    failure: Option<String>,
    catalogue_picker: Option<Receiver<Result<RomHackCatalogueImportPreview, String>>>,
    catalogue_commit: Option<Receiver<Result<Vec<ModCatalogueRecord>, String>>>,
    catalogue_preview: Option<RomHackCatalogueImportPreview>,
    catalogue_mapping: RomHackCatalogueMapping,
    catalogue_records: Vec<ModCatalogueRecord>,
    catalogue_selected: Option<usize>,
    catalogue_action: Option<Receiver<Result<ModCatalogueRecord, String>>>,
}

struct StandalonePatchCandidate {
    inspection: StandalonePatchInspection,
    matching: StandalonePatchMatch,
    adjustment: HeaderAdjustment,
    plan: Option<StandalonePatchApplyPlan>,
    plan_error: Option<String>,
}

impl LocalModPackagePageState {
    pub fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            Some(Stage::Pick(..) | Stage::Applying(_, _) | Stage::RollingBack(_))
        ) || self.standalone.picker.is_some()
            || self.standalone.applying.is_some()
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = poll_standalone(&mut self.standalone);
        let Some(stage) = self.stage.take() else {
            return changed;
        };
        match stage {
            Stage::Pick(receiver, selected_game, mut package_roots) => match receiver.try_recv() {
                Ok(Some(path)) => {
                    package_roots.push(path);
                    let inspection =
                        inspect_local_mod_package_candidates(selected_game, &package_roots);
                    self.stage = Some(Stage::Candidates(
                        inspection.clone(),
                        best_candidate_index(&inspection.plans),
                    ));
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {}
                Err(TryRecvError::Empty) => {
                    self.stage = Some(Stage::Pick(receiver, selected_game, package_roots))
                }
            },
            Stage::Applying(receiver, plan) => match receiver.try_recv() {
                Ok(result) => self.stage = Some(Stage::Applied(result)),
                Err(TryRecvError::Empty) => self.stage = Some(Stage::Applying(receiver, plan)),
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(Stage::Failed(
                        "The mod apply worker stopped before reporting a result.".into(),
                    ))
                }
            },
            Stage::RollingBack(receiver) => match receiver.try_recv() {
                Ok(result) => self.stage = Some(Stage::RolledBack(result.status)),
                Err(TryRecvError::Empty) => self.stage = Some(Stage::RollingBack(receiver)),
                Err(TryRecvError::Disconnected) => {
                    self.stage = Some(Stage::Failed(
                        "The rollback worker stopped before reporting a result.".into(),
                    ))
                }
            },
            other => self.stage = Some(other),
        }
        changed = true;
        changed
    }
}

fn poll_standalone(state: &mut StandalonePatchPageState) -> bool {
    let mut changed = false;
    if let Some(receiver) = state.picker.take() {
        match receiver.try_recv() {
            Ok(Ok(candidate)) => {
                state.candidates.push(candidate);
                state.selected = state.candidates.len().saturating_sub(1);
                state.applied = None;
                state.failure = None;
                changed = true;
            }
            Ok(Err(error)) => {
                state.failure = Some(error);
                changed = true;
            }
            Err(TryRecvError::Empty) => state.picker = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                state.failure = Some("The patch inspection worker stopped early.".into());
                changed = true;
            }
        }
    }
    if let Some(receiver) = state.applying.take() {
        match receiver.try_recv() {
            Ok(Ok(result)) => {
                state.applied = Some(result);
                state.failure = None;
                changed = true;
            }
            Ok(Err(error)) => {
                state.failure = Some(error);
                changed = true;
            }
            Err(TryRecvError::Empty) => state.applying = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                state.failure = Some("The patch apply worker stopped early.".into());
                changed = true;
            }
        }
    }
    if let Some(receiver) = state.catalogue_picker.take() {
        match receiver.try_recv() {
            Ok(Ok(preview)) => {
                state.catalogue_preview = Some(preview);
                state.failure = None;
                changed = true;
            }
            Ok(Err(error)) => {
                state.failure = Some(error);
                changed = true;
            }
            Err(TryRecvError::Empty) => state.catalogue_picker = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                state.failure = Some("The catalogue import worker stopped early.".into());
                changed = true;
            }
        }
    }
    if let Some(receiver) = state.catalogue_commit.take() {
        match receiver.try_recv() {
            Ok(Ok(records)) => {
                state.catalogue_records = records;
                state.catalogue_selected = None;
                state.failure = None;
                changed = true;
            }
            Ok(Err(error)) => {
                state.failure = Some(error);
                changed = true;
            }
            Err(TryRecvError::Empty) => state.catalogue_commit = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                state.failure = Some("The catalogue commit worker stopped early.".into());
                changed = true;
            }
        }
    }
    if let Some(receiver) = state.catalogue_action.take() {
        match receiver.try_recv() {
            Ok(Ok(record)) => {
                if let Some(existing) = state.catalogue_records.iter_mut().find(|item| {
                    item.provider.name == record.provider.name
                        && item.provider.record_id == record.provider.record_id
                }) {
                    *existing = record;
                } else {
                    state.catalogue_records.push(record);
                }
                state.failure = None;
                changed = true;
            }
            Ok(Err(error)) => {
                state.failure = Some(error);
                changed = true;
            }
            Err(TryRecvError::Empty) => state.catalogue_action = Some(receiver),
            Err(TryRecvError::Disconnected) => {
                state.failure = Some("The catalogue association worker stopped early.".into());
                changed = true;
            }
        }
    }
    changed
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn patch_candidate_rank(candidate: &StandalonePatchCandidate) -> u8 {
    if candidate.plan.is_some() {
        return if candidate.adjustment == HeaderAdjustment::None {
            0
        } else {
            1
        };
    }
    match candidate.matching.compatibility {
        PatchCompatibility::ReviewRequired | PatchCompatibility::Unknown => 2,
        PatchCompatibility::Incompatible => 3,
        PatchCompatibility::Compatible => 3,
    }
}

fn base_facts(path: &std::path::Path) -> Result<(u64, u32, String), String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_APPLY_BYTES {
        return Err("The selected ROM is larger than the safe apply limit.".into());
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(&bytes);
    let sha = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok((bytes.len() as u64, crc32(&bytes), sha))
}

fn begin_patch_pick(
    state: &mut StandalonePatchPageState,
    archive_path: &std::path::Path,
    platform: archivefs_core::game_identity::IdentityPlatform,
) {
    let base_path = archive_path.to_path_buf();
    let allow_header = platform == archivefs_core::game_identity::IdentityPlatform::Snes;
    let output_root = archive_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let output_stem = archive_path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "patched-rom".into());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new().pick_file() else {
            return;
        };
        let result = (|| {
            let inspection = inspect_standalone_patch(&path).map_err(|error| error.to_string())?;
            let base = std::fs::read(&base_path).map_err(|error| error.to_string())?;
            if base.len() as u64 > MAX_APPLY_BYTES {
                return Err("The selected ROM is larger than the safe apply limit.".into());
            }
            let (matching, adjustment) =
                match_patch_source_with_header(&inspection, Some(&base), false, allow_header);
            let extension = base_path
                .extension()
                .map(|value| format!(".{}", value.to_string_lossy()))
                .unwrap_or_default();
            let output = output_root.join(format!("{output_stem}.patched{extension}"));
            let plan = if matches!(matching.compatibility, PatchCompatibility::Compatible) {
                match build_standalone_patch_apply_plan_with_header(
                    &inspection,
                    &base_path,
                    output,
                    output_root,
                    adjustment,
                ) {
                    Ok(plan) => Some(plan),
                    Err(error) => {
                        return Ok(StandalonePatchCandidate {
                            inspection,
                            matching,
                            adjustment,
                            plan: None,
                            plan_error: Some(error.to_string()),
                        });
                    }
                }
            } else {
                None
            };
            Ok(StandalonePatchCandidate {
                inspection,
                matching,
                adjustment,
                plan,
                plan_error: None,
            })
        })();
        let _ = sender.send(result);
    });
    state.picker = Some(receiver);
}

fn begin_catalogue_import(state: &mut StandalonePatchPageState) {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let path = rfd::FileDialog::new()
                .add_filter("ROM-hack catalogue", &["json", "zip"])
                .pick_file()
                .ok_or_else(|| "No catalogue selected.".to_string())?;
            inspect_local_rom_hack_catalogue(&path).map_err(|e| e.to_string())
        })();
        let _ = sender.send(result);
    });
    state.catalogue_picker = Some(receiver);
}

fn begin_catalogue_member_inspection(
    state: &mut StandalonePatchPageState,
    path: PathBuf,
    member: String,
) {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(
            inspect_local_rom_hack_catalogue_member(path, &member).map_err(|e| e.to_string()),
        );
    });
    state.catalogue_picker = Some(receiver);
}

fn commit_catalogue_import(state: &mut StandalonePatchPageState) {
    let Some(preview) = state.catalogue_preview.take() else {
        return;
    };
    let path = preview.source_path.clone();
    let member = preview.selected_member.clone();
    let mapping = state.catalogue_mapping.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let records = commit_local_rom_hack_catalogue(&path, member.as_deref(), &mapping)
                .map_err(|e| e.to_string())?;
            let database_path =
                archivefs_core::default_database_path().map_err(|e| e.to_string())?;
            let mut database = archivefs_core::Database::open_or_create(database_path)
                .map_err(|e| e.to_string())?;
            for record in &records {
                database
                    .upsert_mod_catalogue_record(record)
                    .map_err(|e| e.to_string())?;
            }
            database.close().map_err(|e| e.to_string())?;
            Ok(records)
        })();
        let _ = sender.send(result);
    });
    state.catalogue_commit = Some(receiver);
}

fn associate_selected_patch(
    state: &mut StandalonePatchPageState,
    record: &ModCatalogueRecord,
    patch_path: PathBuf,
    patch_sha256: String,
) {
    let mut updated = record.clone();
    let Some(metadata) = updated.rom_hack.as_mut() else {
        return;
    };
    metadata.local_patch_path = Some(patch_path);
    metadata.associated_patch_sha256 = Some(patch_sha256);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let database_path =
                archivefs_core::default_database_path().map_err(|e| e.to_string())?;
            let mut database = archivefs_core::Database::open_or_create(database_path)
                .map_err(|e| e.to_string())?;
            database
                .upsert_mod_catalogue_record(&updated)
                .map_err(|e| e.to_string())?;
            database.close().map_err(|e| e.to_string())?;
            Ok(updated)
        })();
        let _ = sender.send(result);
    });
    state.catalogue_action = Some(receiver);
}

fn catalogue_records_for_view(
    base: &[ModCatalogueRecord],
    imported: &[ModCatalogueRecord],
) -> Vec<ModCatalogueRecord> {
    let mut records = base.to_vec();
    for record in imported {
        if let Some(existing) = records.iter_mut().find(|item| {
            item.provider.name == record.provider.name
                && item.provider.record_id == record.provider.record_id
        }) {
            *existing = record.clone();
        } else {
            records.push(record.clone());
        }
    }
    records.sort_by(|left, right| {
        left.display_title
            .cmp(&right.display_title)
            .then_with(|| left.provider.name.cmp(&right.provider.name))
            .then_with(|| left.provider.record_id.cmp(&right.provider.record_id))
    });
    records
}

fn rom_hack_compatibility(
    record: &ModCatalogueRecord,
    identity: &GameIdentityReport,
    base: Option<(u64, u32, &str)>,
) -> (&'static str, widgets::StatusTone, String) {
    let Some(metadata) = record.rom_hack.as_ref() else {
        return (
            "Browse-only metadata",
            widgets::StatusTone::Info,
            "This catalogue entry has no ROM-hack identity fields.".into(),
        );
    };
    if let Some(platform) = record.platform
        && canonical_platform_identity(platform) != identity.platform
    {
        return (
            "Incompatible revision",
            widgets::StatusTone::Blocked,
            "The catalogue platform does not match this game.".into(),
        );
    }
    let Some((size, crc, sha)) = base else {
        return (
            "Unknown compatibility",
            widgets::StatusTone::Warning,
            "The selected ROM could not be hashed for comparison.".into(),
        );
    };
    if metadata
        .required_base_size
        .is_some_and(|expected| expected != size)
    {
        return (
            "Incompatible revision",
            widgets::StatusTone::Blocked,
            "The required ROM size differs from the selected ROM.".into(),
        );
    }
    if metadata
        .required_base_crc32
        .is_some_and(|expected| expected != crc)
    {
        return (
            "Incompatible revision",
            widgets::StatusTone::Blocked,
            "The required ROM CRC32 differs from the selected ROM.".into(),
        );
    }
    let sha256_hashes: Vec<_> = metadata
        .required_base_hashes
        .iter()
        .filter(|hash| {
            matches!(
                hash.algorithm,
                archivefs_core::mod_catalogue::ModCatalogueHashAlgorithm::Sha256
            )
        })
        .collect();
    if sha256_hashes.iter().any(|hash| {
        matches!(
            hash.algorithm,
            archivefs_core::mod_catalogue::ModCatalogueHashAlgorithm::Sha256
        ) && hash.value.eq_ignore_ascii_case(sha)
    }) {
        return (
            "Exact match",
            widgets::StatusTone::Success,
            "The selected ROM SHA-256 exactly matches the catalogue.".into(),
        );
    }
    if !sha256_hashes.is_empty() {
        return (
            "Incompatible revision",
            widgets::StatusTone::Blocked,
            "The required ROM SHA-256 differs from the selected ROM.".into(),
        );
    }
    if metadata.required_base_crc32.is_some() && metadata.required_base_size.is_some() {
        return (
            "Exact match",
            widgets::StatusTone::Success,
            "The selected ROM CRC32 and size exactly match the catalogue.".into(),
        );
    }
    (
        "Unknown compatibility",
        widgets::StatusTone::Warning,
        "This entry matches only title/platform metadata; it remains browse-only until the patch itself is verified.".into(),
    )
}

fn canonical_platform_identity(
    platform: archivefs_core::mod_package::ModCanonicalPlatform,
) -> archivefs_core::game_identity::IdentityPlatform {
    use archivefs_core::game_identity::IdentityPlatform;
    use archivefs_core::mod_package::ModCanonicalPlatform;
    match platform {
        ModCanonicalPlatform::PlayStation3 => IdentityPlatform::PlayStation3,
        ModCanonicalPlatform::PlayStation2 => IdentityPlatform::PlayStation2,
        ModCanonicalPlatform::GameCube => IdentityPlatform::GameCube,
        ModCanonicalPlatform::Wii => IdentityPlatform::Wii,
        ModCanonicalPlatform::MegaDrive => IdentityPlatform::MegaDrive,
        ModCanonicalPlatform::Snes => IdentityPlatform::Snes,
        ModCanonicalPlatform::Xbox360 => IdentityPlatform::Xbox360,
    }
}

fn show_rom_hack_catalogue(
    ui: &mut egui::Ui,
    state: &mut StandalonePatchPageState,
    archive_path: &std::path::Path,
    identity: &GameIdentityReport,
    records: &[ModCatalogueRecord],
) {
    let merged = catalogue_records_for_view(records, &state.catalogue_records);
    let facts = base_facts(archive_path).ok();
    widgets::card(ui, |ui| {
        ui.heading("ROM Hacks catalogue");
        ui.label(
            "Metadata imported from a local file only. Catalogue entries never download patches.",
        );
        if widgets::action_button(
            ui,
            "Import local ROM-hack catalogue",
            widgets::ActionStyle::Secondary,
            state.catalogue_picker.is_none(),
        )
        .clicked()
        {
            begin_catalogue_import(state);
        }
        if state.catalogue_picker.is_some() {
            ui.label("Inspecting catalogue metadata locally…");
        }
        if let Some(preview) = state.catalogue_preview.clone() {
            ui.separator();
            ui.heading("Import preview");
            ui.label(format!(
                "Detected format: {:?} · shape: {:?}",
                preview.input_format, preview.shape
            ));
            ui.label(format!(
                "Records detected: {} · valid: {} · rejected: {}",
                preview.records_detected,
                preview.records_valid,
                preview.rejected.len()
            ));
            ui.label(format!(
                "Mapped fields: {} · ignored fields: {}",
                preview.mapped_fields.len(),
                preview.ignored_fields.len()
            ));
            if !preview.mapped_fields.is_empty() {
                ui.small(
                    preview
                        .mapped_fields
                        .iter()
                        .map(|(field, path)| format!("{} ← {path}", field.label()))
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
            }
            if !preview.ignored_fields.is_empty() {
                ui.small(format!(
                    "Ignored fields: {}",
                    preview.ignored_fields.join(", ")
                ));
            }
            if !preview.duplicate_record_ids.is_empty() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Duplicate record IDs will be handled idempotently.",
                );
            }
            if !preview.missing_identity.is_empty() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Some records have no checksum identity and remain browse-only.",
                );
            }
            if !preview.rejected.is_empty() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "Rejected records: {}",
                        preview
                            .rejected
                            .iter()
                            .map(|r| format!("#{} ({})", r.index + 1, r.reason))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
            if !preview.candidates.is_empty() {
                ui.label("ZIP catalogue candidates:");
                for candidate in &preview.candidates {
                    let selected =
                        preview.selected_member.as_deref() == Some(candidate.member_name.as_str());
                    if ui
                        .selectable_label(
                            selected,
                            format!(
                                "{} · score {} · {} records",
                                candidate.member_name, candidate.score, candidate.records
                            ),
                        )
                        .clicked()
                    {
                        begin_catalogue_member_inspection(
                            state,
                            preview.source_path.clone(),
                            candidate.member_name.clone(),
                        );
                    }
                }
            }
            ui.label(format!(
                "Sample records: {}",
                preview
                    .sample_records
                    .iter()
                    .map(|r| r.display_title.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            let zip_member_missing = preview.input_format
                == archivefs_core::rom_hack_catalogue::LocalCatalogueInputFormat::Zip
                && preview.selected_member.is_none();
            let mapping_resolves = !zip_member_missing
                && (!preview.requires_mapping
                    || state
                        .catalogue_mapping
                        .paths
                        .contains_key(&CatalogueField::HackTitle));
            if preview.requires_mapping {
                ui.colored_label(egui::Color32::YELLOW, "This import needs an explicit ZIP member or unambiguous field mapping; no database write is available yet.");
                ui.label("Adjust mapping (temporary for this import session):");
                for field in [
                    CatalogueField::HackTitle,
                    CatalogueField::BaseGameTitle,
                    CatalogueField::Platform,
                    CatalogueField::Author,
                    CatalogueField::Version,
                    CatalogueField::PatchFormat,
                    CatalogueField::Crc32,
                    CatalogueField::Sha1,
                    CatalogueField::Sha256,
                    CatalogueField::SourceSize,
                    CatalogueField::Region,
                    CatalogueField::Revision,
                    CatalogueField::HeaderExpectation,
                    CatalogueField::ReleaseDate,
                    CatalogueField::SourceRecordId,
                ] {
                    let mut path = state
                        .catalogue_mapping
                        .paths
                        .get(&field)
                        .cloned()
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label(field.label());
                        if ui.text_edit_singleline(&mut path).changed() {
                            if path.trim().is_empty() {
                                state.catalogue_mapping.paths.remove(&field);
                            } else {
                                state.catalogue_mapping.paths.insert(field, path.clone());
                            }
                        }
                    });
                }
            }
            let import_enabled = mapping_resolves
                && state.catalogue_picker.is_none()
                && state.catalogue_commit.is_none();
            ui.horizontal(|ui| {
                if ui.add_enabled(import_enabled, egui::Button::new("Import")).clicked() { commit_catalogue_import(state); }
                if ui.button("Adjust mapping").clicked() { state.failure = Some("Automatic mapping is conservative. Select a catalogue with one unambiguous alias per field, or use the core import mapping API for an explicit field path.".into()); }
                if ui.button("Cancel").clicked() { state.catalogue_preview = None; state.failure = None; }
            });
        }
        if merged.is_empty() {
            ui.label("No ROM-hack metadata is installed locally.");
            return;
        }
        ui.heading("Known hacks");
        for (index, record) in merged.iter().enumerate() {
            let (label, tone, reason) = rom_hack_compatibility(
                record,
                identity,
                facts
                    .as_ref()
                    .map(|(size, crc, sha)| (*size, *crc, sha.as_str())),
            );
            if ui
                .selectable_label(
                    state.catalogue_selected == Some(index),
                    format!(
                        "{} {} — {label}",
                        record.display_title,
                        record.version.as_deref().unwrap_or("")
                    ),
                )
                .clicked()
            {
                state.catalogue_selected = Some(index);
            }
            ui.small(format!(
                "{} · {} · {}",
                record.author.as_deref().unwrap_or("Author unknown"),
                record
                    .rom_hack
                    .as_ref()
                    .and_then(|m| m.patch_format.as_deref())
                    .unwrap_or("format unknown"),
                reason
            ));
            if tone == widgets::StatusTone::Blocked {
                ui.small("Incompatible revisions remain visible for reference.");
            }
        }
        if let Some(index) = state.catalogue_selected.and_then(|index| merged.get(index)) {
            let (label, tone, reason) = rom_hack_compatibility(
                index,
                identity,
                facts
                    .as_ref()
                    .map(|(size, crc, sha)| (*size, *crc, sha.as_str())),
            );
            widgets::status_badge(ui, label, tone);
            ui.label(reason);
            if let Some(metadata) = index.rom_hack.as_ref() {
                ui.label(format!(
                    "Base game: {}",
                    metadata.base_game_title.as_deref().unwrap_or("Unknown")
                ));
                ui.label(format!(
                    "Expected revision: {}",
                    index.declared_revision.as_deref().unwrap_or("Unknown")
                ));
                ui.label(format!(
                    "Patch format: {}",
                    metadata.patch_format.as_deref().unwrap_or("Unknown")
                ));
                ui.label(format!(
                    "Release date: {}",
                    metadata.release_date.as_deref().unwrap_or("Unknown")
                ));
                ui.label(format!(
                    "Header expectation: {}",
                    match metadata.header_expectation {
                        Some(archivefs_core::mod_catalogue::RomHackHeaderExpectation::Headered) => {
                            "headered"
                        }
                        Some(
                            archivefs_core::mod_catalogue::RomHackHeaderExpectation::Headerless,
                        ) => {
                            "headerless"
                        }
                        Some(archivefs_core::mod_catalogue::RomHackHeaderExpectation::Either) => {
                            "either"
                        }
                        None => "not specified",
                    }
                ));
                ui.label(if metadata.local_patch_path.is_some() {
                    "Local patch payload is associated."
                } else {
                    "Known hack — patch file is not installed locally."
                });
                if metadata.local_patch_path.is_none() {
                    ui.label(
                        "Choose a local patch above, then associate it here after inspection.",
                    );
                    let selected_patch =
                        state.candidates.get(state.selected).and_then(|candidate| {
                            (candidate.inspection.state
                                == archivefs_core::standalone_patch::PatchInspectionState::Valid)
                                .then(|| {
                                    (
                                        candidate.inspection.path.clone(),
                                        candidate.inspection.patch_sha256.clone(),
                                    )
                                })
                        });
                    if let Some((patch_path, patch_sha256)) = selected_patch
                        && ui.button("Associate selected local patch").clicked()
                    {
                        associate_selected_patch(state, index, patch_path, patch_sha256);
                    }
                }
            }
        }
    });
}

fn show_standalone_patch_panel(
    ui: &mut egui::Ui,
    state: &mut StandalonePatchPageState,
    archive_path: &std::path::Path,
    identity: &GameIdentityReport,
    catalogue_records: &[ModCatalogueRecord],
) {
    widgets::section_header(
        ui,
        "Standalone ROM patches",
        Some(
            "Choose a local patch and preview a new derived ROM. Your original ROM is never changed.",
        ),
    );
    show_rom_hack_catalogue(ui, state, archive_path, identity, catalogue_records);
    widgets::card(ui, |ui| {
        ui.heading("Base ROM");
        ui.label(archive_path.display().to_string());
        match base_facts(archive_path) {
            Ok((size, crc, sha)) => {
                ui.label(format!(
                    "Platform: {:?} · Size: {size} bytes · CRC32: {crc:08X}",
                    identity.platform
                ));
                ui.small(format!("SHA-256: {sha}"));
            }
            Err(error) => widgets::banner(
                ui,
                "Base ROM unavailable",
                &error,
                widgets::StatusTone::Blocked,
            ),
        }
        if widgets::action_button(
            ui,
            "Choose local patch file",
            widgets::ActionStyle::Secondary,
            state.picker.is_none(),
        )
        .clicked()
        {
            begin_patch_pick(state, archive_path, identity.platform);
        }
        if state.picker.is_some() {
            ui.label("Inspecting patch read-only…");
        }
        if let Some(error) = state.failure.as_ref() {
            widgets::banner(
                ui,
                "Patch workflow stopped",
                error,
                widgets::StatusTone::Blocked,
            );
        }
    });
    if state.candidates.is_empty() {
        return;
    }
    let mut order: Vec<_> = (0..state.candidates.len()).collect();
    order.sort_by_key(|index| {
        (
            patch_candidate_rank(&state.candidates[*index]),
            state.candidates[*index].inspection.path.clone(),
        )
    });
    state.selected = state.selected.min(state.candidates.len() - 1);
    widgets::card(ui, |ui| {
        ui.heading("Local patch candidates");
        ui.label("Candidates remain visible, including patches that cannot safely apply.");
        for index in order {
            let candidate = &state.candidates[index];
            let label = match candidate.matching.compatibility {
                PatchCompatibility::Compatible
                    if candidate.adjustment == HeaderAdjustment::None =>
                {
                    "Exact match"
                }
                PatchCompatibility::Compatible => "Compatible after explicit header adjustment",
                PatchCompatibility::Incompatible => "Incompatible",
                PatchCompatibility::ReviewRequired => "Possible match — needs review",
                PatchCompatibility::Unknown => "Unknown compatibility",
            };
            if ui
                .selectable_label(
                    index == state.selected,
                    format!("{} — {label}", candidate.inspection.path.display()),
                )
                .clicked()
            {
                state.selected = index;
                state.applied = None;
            }
        }
    });
    let candidate = &state.candidates[state.selected];
    widgets::card(ui, |ui| {
        ui.heading("Patch preview");
        ui.label(format!("Patch: {}", candidate.inspection.path.display()));
        ui.label(format!(
            "Format: {:?} · Size: {} bytes",
            candidate.inspection.format, candidate.inspection.patch_size
        ));
        ui.small(format!(
            "Patch SHA-256: {}",
            candidate.inspection.patch_sha256
        ));
        if let Some(source) = candidate.inspection.source_crc32 {
            ui.label(format!("Expected source CRC32: {source:08X}"));
        }
        if let Some(target) = candidate.inspection.target_crc32 {
            ui.label(format!("Expected output CRC32: {target:08X}"));
        }
        let (headline, tone) = match candidate.matching.compatibility {
            PatchCompatibility::Compatible if candidate.adjustment == HeaderAdjustment::None => {
                ("Exact match", widgets::StatusTone::Success)
            }
            PatchCompatibility::Compatible => {
                ("Header adjustment required", widgets::StatusTone::Warning)
            }
            PatchCompatibility::Incompatible => ("Incompatible", widgets::StatusTone::Blocked),
            PatchCompatibility::ReviewRequired => ("Possible match", widgets::StatusTone::Warning),
            PatchCompatibility::Unknown => ("Unknown", widgets::StatusTone::Warning),
        };
        widgets::status_badge(ui, headline, tone);
        ui.label(&candidate.matching.reason);
        ui.label(match candidate.adjustment {
            HeaderAdjustment::None => "Transformation: none".to_string(),
            HeaderAdjustment::Strip512ByteCopierHeader => {
                "Transformation: strip 512-byte copier header for patch input".to_string()
            }
        });
        if let Some(plan) = candidate.plan.as_ref() {
            ui.label(format!("Output: {}", plan.reviewed.output_path.display()));
            ui.label("Source modified: NO");
            if state.applying.is_none()
                && state.applied.is_none()
                && widgets::action_button(
                    ui,
                    "Create derived patched ROM",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
            {
                let (sender, receiver) = mpsc::channel();
                let worker_plan = plan.clone();
                std::thread::spawn(move || {
                    let result =
                        apply_standalone_patch(&worker_plan).map_err(|error| error.to_string());
                    let _ = sender.send(result);
                });
                state.applying = Some(receiver);
            }
        } else if let Some(error) = candidate.plan_error.as_ref() {
            widgets::banner(
                ui,
                "Cannot apply this patch",
                error,
                widgets::StatusTone::Blocked,
            );
        } else {
            widgets::banner(
                ui,
                "Cannot safely apply this patch",
                "The selected patch is inspectable, but its source identity is not an exact safe match.",
                widgets::StatusTone::Blocked,
            );
        }
        if state.applying.is_some() {
            ui.label("Applying to a new derived file…");
        }
        if let Some(result) = state.applied.as_ref() {
            widgets::status_badge(ui, "Derived ROM created", widgets::StatusTone::Success);
            ui.label(format!("Output: {}", result.output_path.display()));
            ui.label(format!("Output SHA-256: {}", result.output_sha256));
            ui.label("The original ROM was not changed. Provenance was recorded with the result.");
        }
    });
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How a finished apply or rollback should read to a beginner: one headline,
/// one plain sentence, and a tone drawn from the existing GUI conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StatusPresentation {
    headline: &'static str,
    detail: &'static str,
    tone: widgets::StatusTone,
}

/// Presentation for a completed `execute_shared_apply`. A written journal
/// (`journal_path.is_some()`) does not by itself mean the mod applied: the
/// journal is written for `PartialFailure` and `Failed` outcomes too, so the
/// only honest signal is `SharedApplyJournal::status`.
fn apply_presentation(status: SharedApplyStatus) -> StatusPresentation {
    match status {
        SharedApplyStatus::Success => StatusPresentation {
            headline: "Mod installed",
            detail: "Game files were changed only by the confirmed plan.",
            tone: widgets::StatusTone::Success,
        },
        SharedApplyStatus::PartialFailure => StatusPresentation {
            headline: "Mod only partly applied",
            detail: "Some of this mod's files were not applied, so the mod is not fully installed. Undo restores the files that did change.",
            tone: widgets::StatusTone::Warning,
        },
        SharedApplyStatus::Failed => StatusPresentation {
            headline: "Mod was not applied",
            detail: "None of this mod's changes were applied. Your game files are unchanged.",
            tone: widgets::StatusTone::Blocked,
        },
        SharedApplyStatus::DryRun => StatusPresentation {
            headline: "Preview only",
            detail: "Nothing was written to your game files.",
            tone: widgets::StatusTone::Info,
        },
    }
}

/// Presentation for a completed `execute_shared_rollback`. A successful undo
/// must render as success, never as `Stage::Failed`.
fn rollback_presentation(status: SharedApplyStatus) -> StatusPresentation {
    match status {
        SharedApplyStatus::Success => StatusPresentation {
            headline: "Mod removed",
            detail: "The previous game files were restored.",
            tone: widgets::StatusTone::Success,
        },
        SharedApplyStatus::PartialFailure => StatusPresentation {
            headline: "Undo only partly finished",
            detail: "Some files were restored, but not all. Check this game before playing.",
            tone: widgets::StatusTone::Warning,
        },
        SharedApplyStatus::Failed | SharedApplyStatus::DryRun => StatusPresentation {
            headline: "Undo did not complete",
            detail: "The previous game files were not restored.",
            tone: widgets::StatusTone::Blocked,
        },
    }
}

fn compatibility_presentation(
    state: ModCompatibilityState,
) -> (&'static str, widgets::StatusTone, &'static str) {
    match state {
        ModCompatibilityState::Compatible => (
            "Ready to apply",
            widgets::StatusTone::Success,
            "The package matches the selected game's verified identity.",
        ),
        ModCompatibilityState::Incompatible => (
            "Not available for this game",
            widgets::StatusTone::Blocked,
            "The package identity does not match the selected game.",
        ),
        ModCompatibilityState::Unknown => (
            "Needs game identity",
            widgets::StatusTone::Warning,
            "EmuWiz cannot safely verify which game this package targets.",
        ),
    }
}

fn candidate_order(plans: &[LocalModPackagePlan]) -> Vec<usize> {
    let mut order: Vec<_> = (0..plans.len()).collect();
    order.sort_by(|left, right| {
        candidate_rank(&plans[*left])
            .cmp(&candidate_rank(&plans[*right]))
            .then_with(|| {
                candidate_sort_key(&plans[*left]).cmp(&candidate_sort_key(&plans[*right]))
            })
    });
    order
}

fn best_candidate_index(plans: &[LocalModPackagePlan]) -> usize {
    candidate_order(plans).into_iter().next().unwrap_or(0)
}

fn candidate_rank(plan: &LocalModPackagePlan) -> u8 {
    if !plan.blockers.is_empty() || !plan.conflicts.is_empty() {
        return 3;
    }
    match plan.compatibility.state {
        ModCompatibilityState::Compatible => 0,
        ModCompatibilityState::Unknown => 2,
        ModCompatibilityState::Incompatible => 3,
    }
}

fn candidate_sort_key(plan: &LocalModPackagePlan) -> (String, PathBuf) {
    (
        plan.package
            .as_ref()
            .map(|package| package.package_id.clone())
            .unwrap_or_default(),
        plan.package_root.clone(),
    )
}

fn candidate_state_label(plan: &LocalModPackagePlan) -> &'static str {
    if !plan.blockers.is_empty() || !plan.conflicts.is_empty() {
        "Needs review"
    } else {
        match plan.compatibility.state {
            ModCompatibilityState::Compatible => "Ready to apply",
            ModCompatibilityState::Incompatible => "Unavailable for this game",
            ModCompatibilityState::Unknown => "Needs game identity",
        }
    }
}

/// Whether an apply result actually changed at least one game file that a
/// rollback could restore. `AlreadyInstalled` is deliberately excluded (the
/// file was already in place, so there is nothing to undo), as is a result
/// with no written journal. This is what gates the Undo control for every
/// status: on `Failed` no entry ever qualifies, on `PartialFailure` only the
/// subset that genuinely changed does.
fn has_restorable_changes(result: &SharedApplyResult) -> bool {
    result.journal_path.is_some()
        && result.journal.entries.iter().any(|entry| {
            matches!(
                entry.outcome,
                SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
            )
        })
}

fn catalogue_candidate_label(
    record: &ModCatalogueRecord,
    identity: &GameIdentityReport,
) -> &'static str {
    let selected = SelectedGameForMod {
        game_root: identity
            .archive_path
            .parent()
            .unwrap_or(identity.archive_path.as_path())
            .to_path_buf(),
        identity: identity.clone(),
    };
    match review_catalogue_record(record, &selected, None)
        .compatibility
        .state
    {
        ModCompatibilityState::Compatible => "Compatible catalogue match",
        ModCompatibilityState::Incompatible => "Not for this game",
        ModCompatibilityState::Unknown => "Needs review",
    }
}

fn show_provider_catalogue_candidates(
    ui: &mut egui::Ui,
    state: &mut LocalModPackagePageState,
    identity: &GameIdentityReport,
    records: &[ModCatalogueRecord],
) {
    let records: Vec<_> = records
        .iter()
        .filter(|record| record.rom_hack.is_none())
        .collect();
    if records.is_empty() {
        return;
    }
    let selected = state.provider_selection.as_ref();
    let mut clicked = None;
    widgets::card(ui, |ui| {
        ui.heading("Catalogue mod candidates");
        ui.label("These records were imported from a provider catalogue. They are browse-only until a matching package is available locally.");
        let mut order: Vec<_> = records.clone();
        order.sort_by(|left, right| {
            catalogue_candidate_label(right, identity)
                .cmp(catalogue_candidate_label(left, identity))
                .then_with(|| left.provider.name.cmp(&right.provider.name))
                .then_with(|| left.provider.record_id.cmp(&right.provider.record_id))
        });
        for record in order {
            let key = (
                record.provider.name.clone(),
                record.provider.record_id.clone(),
            );
            let label = format!(
                "{} — {}",
                record.display_title,
                catalogue_candidate_label(record, identity)
            );
            if ui.selectable_label(selected == Some(&key), label).clicked() {
                clicked = Some(key);
            }
            ui.small(format!(
                "Provider: {} · Record: {}",
                record.provider.name, record.provider.record_id
            ));
        }
    });
    if let Some(key) = clicked {
        state.provider_selection = Some(key);
        state.stage = None;
    }
    if let Some(key) = state.provider_selection.as_ref()
        && let Some(record) = records
            .iter()
            .find(|record| record.provider.name == key.0 && record.provider.record_id == key.1)
    {
        widgets::card(ui, |ui| {
            ui.heading(&record.display_title);
            ui.label(format!("Provider: {}", record.provider.name));
            ui.label(format!("Source: {}", record.provider.source_page_url));
            ui.label("Catalogue record only — the package has not been downloaded.");
            ui.label("Apply is unavailable until a local package is inspected and reviewed.");
            if let Some(version) = &record.version {
                ui.label(format!("Version: {version}"));
            }
            if let Some(description) = &record.description {
                ui.label(description);
            }
            if ui.button("Clear catalogue selection").clicked() {
                state.provider_selection = None;
            }
        });
    }
}

#[allow(dead_code)]
pub fn show_local_mod_package_panel(
    ui: &mut egui::Ui,
    state: &mut LocalModPackagePageState,
    archive_path: &std::path::Path,
    identity: Option<&GameIdentityReport>,
) {
    show_local_mod_package_panel_with_catalogue(ui, state, archive_path, identity, &[]);
}

pub fn show_local_mod_package_panel_with_catalogue(
    ui: &mut egui::Ui,
    state: &mut LocalModPackagePageState,
    archive_path: &std::path::Path,
    identity: Option<&GameIdentityReport>,
    catalogue_records: &[ModCatalogueRecord],
) {
    let Some(identity) = identity else {
        widgets::section_header(ui, "Ordinary game mods", None);
        widgets::card(ui, |ui| {
            ui.label("Load exact game identity evidence before choosing a mod.");
        });
        return;
    };
    let Some(game_root) = archive_path
        .parent()
        .filter(|path| path.is_dir())
        .map(PathBuf::from)
    else {
        widgets::section_header(ui, "Ordinary game mods", None);
        widgets::card(ui, |ui| {
            ui.label("EmuWiz cannot identify a safe game folder for this file, so it will not offer mod installation.");
        });
        return;
    };
    let key = (archive_path.to_path_buf(), game_root.clone());
    if state.key.as_ref() != Some(&key) {
        state.key = Some(key);
        state.stage = None;
        state.provider_selection = None;
        state.standalone = StandalonePatchPageState::default();
    }
    widgets::section_header(
        ui,
        "Ordinary game mods",
        Some("Choose a local mod folder. EmuWiz previews every file before anything changes."),
    );
    show_standalone_patch_panel(
        ui,
        &mut state.standalone,
        archive_path,
        identity,
        catalogue_records,
    );
    show_provider_catalogue_candidates(ui, state, identity, catalogue_records);
    if state.provider_selection.is_some() {
        return;
    }
    if state.stage.is_none() {
        if widgets::action_button(
            ui,
            "Choose local mod folder",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(rfd::FileDialog::new().pick_folder());
            });
            state.stage = Some(Stage::Pick(
                receiver,
                SelectedGameForMod {
                    game_root: game_root.clone(),
                    identity: identity.clone(),
                },
                Vec::new(),
            ));
        }
        return;
    }
    let stage = state.stage.take().unwrap();
    match stage {
        Stage::Pick(receiver, selected_game, package_roots) => {
            ui.label("Waiting for folder selection…");
            state.stage = Some(Stage::Pick(receiver, selected_game, package_roots));
        }
        Stage::Planned(plan) => {
            show_plan(ui, state, plan, archive_path, identity, &game_root);
        }
        Stage::Candidates(inspection, selected) => {
            let selected = selected.min(inspection.plans.len().saturating_sub(1));
            let mut next_selected = selected;
            widgets::card(ui, |ui| {
                ui.heading("Local mod candidates");
                ui.label("Each selected local mod folder is shown here as a separate candidate. EmuWiz never chooses between candidates silently.");
                if inspection.plans.is_empty() {
                    ui.label("No readable mod candidates were found.");
                } else {
                    for index in candidate_order(&inspection.plans) {
                        let plan = &inspection.plans[index];
                        let title = plan
                            .package
                            .as_ref()
                            .map(|package| format!("{} {}", package.title, package.version))
                            .unwrap_or_else(|| plan.package_root.display().to_string());
                        let state_label = candidate_state_label(plan);
                        if ui
                            .selectable_label(index == selected, format!("{title} — {state_label}"))
                            .clicked()
                        {
                            next_selected = index;
                        }
                        ui.small(format!("Source: {}", plan.package_root.display()));
                    }
                }
                if !inspection.blockers.is_empty() {
                    for blocker in &inspection.blockers {
                        widgets::banner(
                            ui,
                            "Some mod candidates need review",
                            &blocker.detail,
                            widgets::StatusTone::Warning,
                        );
                    }
                }
                if widgets::action_button(
                    ui,
                    "Add another local mod folder",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
                {
                    let (sender, receiver) = mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = sender.send(rfd::FileDialog::new().pick_folder());
                    });
                    let roots = inspection
                        .plans
                        .iter()
                        .map(|plan| plan.package_root.clone())
                        .collect();
                    state.stage = Some(Stage::Pick(
                        receiver,
                        SelectedGameForMod {
                            game_root: game_root.clone(),
                            identity: identity.clone(),
                        },
                        roots,
                    ));
                }
            });
            if state.stage.is_none() {
                if let Some(plan) = inspection.plans.get(next_selected).cloned() {
                    let left_candidates =
                        show_plan(ui, state, plan, archive_path, identity, &game_root);
                    if left_candidates {
                        return;
                    }
                }
                if state.stage.is_none() {
                    state.stage = Some(Stage::Candidates(inspection, next_selected));
                }
            }
        }
        Stage::Confirm(plan) => {
            widgets::card(ui, |ui| {
                ui.label("Original game: unchanged");
                ui.label("Only the confirmed files below will be changed.");
                for entry in plan.entries.iter().take(12) {
                    ui.label(format!(
                        "Planned: {}",
                        entry.destination_relative_path.display
                    ));
                }
                ui.label(format!(
                    "Apply {}? Nothing is written until you confirm.",
                    plan.entries.len()
                ));
                ui.horizontal(|ui| {
                    if widgets::action_button(ui, "Keep preview", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        state.stage = None;
                    }
                    if widgets::action_button(
                        ui,
                        "Confirm apply",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        let (sender, receiver) = mpsc::channel();
                        let worker_plan = plan.clone();
                        std::thread::spawn(move || {
                            let result = (|| {
                                let history_root =
                                    default_shared_history_root().map_err(|e| e.detail)?;
                                let backup_root =
                                    default_shared_backup_root().map_err(|e| e.detail)?;
                                Ok::<_, String>(execute_shared_apply(
                                    &worker_plan,
                                    &SharedApplyOptions {
                                        dry_run: false,
                                        confirmation: Some(SharedApplyConfirmation {
                                            plan_id: worker_plan.plan_id.clone(),
                                            general_approved: true,
                                            replacement_approved: true,
                                        }),
                                        operation_id: generate_shared_operation_id(),
                                        timestamp_unix_seconds: now(),
                                        current_context: worker_plan.context.clone(),
                                        history_root,
                                        backup_root,
                                    },
                                ))
                            })();
                            if let Ok(result) = result {
                                let _ = sender.send(result);
                            }
                        });
                        state.stage = Some(Stage::Applying(receiver, plan));
                    }
                });
            });
        }
        Stage::Applying(receiver, plan) => {
            ui.label("Applying mod safely…");
            state.stage = Some(Stage::Applying(receiver, plan));
        }
        Stage::Applied(result) => {
            let presentation = apply_presentation(result.journal.status);
            let can_undo = has_restorable_changes(&result);
            widgets::card(ui, |ui| {
                widgets::status_badge(ui, presentation.headline, presentation.tone);
                ui.label(presentation.detail);
                if can_undo
                    && let Some(journal) = result.journal_path.as_ref()
                    && widgets::action_button(
                        ui,
                        "Undo this mod",
                        widgets::ActionStyle::Destructive,
                        true,
                    )
                    .clicked()
                    && let (Ok(backup), Ok(_history)) =
                        (default_shared_backup_root(), default_shared_history_root())
                {
                    state.stage = Some(Stage::Rollback(preview_shared_rollback(
                        journal, &game_root, &backup,
                    )));
                }
            });
            if !matches!(state.stage, Some(Stage::Rollback(_))) {
                state.stage = Some(Stage::Applied(result));
            }
        }
        Stage::Rollback(preview) => {
            widgets::card(ui, |ui| {
                ui.label(if preview.available {
                    "Undo this mod and restore the exact previous files?"
                } else {
                    "This mod can no longer be safely undone."
                });
                if widgets::action_button(
                    ui,
                    "Confirm undo",
                    widgets::ActionStyle::Destructive,
                    preview.available,
                )
                .clicked()
                    && let (Ok(history_root), Ok(backup_root)) =
                        (default_shared_history_root(), default_shared_backup_root())
                {
                    let (sender, receiver) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = execute_shared_rollback(
                            &preview,
                            &SharedRollbackOptions {
                                confirmation: SharedRollbackConfirmation {
                                    preview_id: preview.preview_id.clone(),
                                    approved: true,
                                },
                                rollback_operation_id: generate_shared_operation_id(),
                                timestamp_unix_seconds: now(),
                                history_root,
                                backup_root,
                            },
                        );
                        let _ = sender.send(result);
                    });
                    state.stage = Some(Stage::RollingBack(receiver));
                }
            });
        }
        Stage::RollingBack(receiver) => {
            ui.label("Undoing mod…");
            state.stage = Some(Stage::RollingBack(receiver));
        }
        Stage::RolledBack(status) => {
            let presentation = rollback_presentation(status);
            let mut dismissed = false;
            widgets::card(ui, |ui| {
                widgets::status_badge(ui, presentation.headline, presentation.tone);
                ui.label(presentation.detail);
                if widgets::action_button(ui, "Done", widgets::ActionStyle::Quiet, true).clicked() {
                    dismissed = true;
                }
            });
            if !dismissed {
                state.stage = Some(Stage::RolledBack(status));
            }
        }
        Stage::Failed(detail) => {
            widgets::banner(
                ui,
                "Mod workflow stopped",
                &detail,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Start over", widgets::ActionStyle::Quiet, true).clicked()
            {
                state.stage = None;
            } else {
                state.stage = Some(Stage::Failed(detail));
            }
        }
    }
}

fn show_plan(
    ui: &mut egui::Ui,
    state: &mut LocalModPackagePageState,
    plan: LocalModPackagePlan,
    archive_path: &std::path::Path,
    identity: &GameIdentityReport,
    game_root: &std::path::Path,
) -> bool {
    let mut left_candidates = false;
    widgets::card(ui, |ui| {
        if let Some(package) = plan.package.as_ref() {
            ui.heading(format!("{} {}", package.title, package.version));
        }
        let (compatibility, tone, explanation) =
            compatibility_presentation(plan.compatibility.state);
        widgets::status_badge(ui, compatibility, tone);
        ui.label(explanation);
        for blocker in &plan.blockers {
            widgets::banner(
                ui,
                "Cannot apply this mod — needs review",
                &blocker.detail,
                widgets::StatusTone::Blocked,
            );
        }
        for conflict in &plan.conflicts {
            widgets::banner(
                ui,
                "Conflict detected",
                &conflict.detail,
                widgets::StatusTone::Warning,
            );
        }
        if plan.blockers.is_empty() && plan.conflicts.is_empty() {
            ui.label("Original game: unchanged until you confirm apply.");
            ui.label(format!(
                "{} file(s) will be added or replaced below {}.",
                plan.operations.len(),
                game_root.display()
            ));
            for operation in plan.operations.iter().take(12) {
                ui.label(format!(
                    "{}: {}",
                    match operation.kind {
                        archivefs_core::mod_package::ModOperationKind::CreateFile => "Add",
                        archivefs_core::mod_package::ModOperationKind::ReplaceFile => "Replace",
                        archivefs_core::mod_package::ModOperationKind::PatchFile => "Patch",
                        archivefs_core::mod_package::ModOperationKind::DeleteFile => "Delete",
                    },
                    operation.destination_path.display()
                ));
            }
            if let Ok(transaction) = build_local_mod_package_transaction_plan(&plan)
                && widgets::action_button(
                    ui,
                    "Review and apply mod",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
            {
                state.stage = Some(Stage::Confirm(transaction));
            }
        }
        if widgets::action_button(
            ui,
            "Choose another folder",
            widgets::ActionStyle::Quiet,
            true,
        )
        .clicked()
        {
            state.stage = None;
            left_candidates = true;
        }
    });
    let _ = (archive_path, identity);
    left_candidates
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityKind, IdentityPlatform, IdentityProvenance, IdentityStatus,
    };
    use archivefs_core::mod_package::{
        LocalModPackageRequest, SelectedGameForMod, inspect_local_mod_package,
    };
    use archivefs_core::patch_manager::{
        SharedApplyConfirmation, SharedApplyOptions, SharedApplyStatus, execute_shared_apply,
    };
    use eframe::egui;

    use super::*;

    fn identity(game_bin: &Path) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: game_bin.to_path_buf(),
            platform: IdentityPlatform::Snes,
            format: IdentityImageFormat::LooseCartridgeRom,
            evidence: vec![IdentityEvidence {
                kind: IdentityKind::LooseRomSha256,
                status: IdentityStatus::Verified,
                value: Some("game-sha".to_string()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: IdentityProvenance {
                    archive_path: game_bin.to_path_buf(),
                    member_path: None,
                    member_index: None,
                    method: "test".to_string(),
                },
                diagnostic: String::new(),
            }],
            warnings: Vec::new(),
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    /// A game tree with `game.bin`, and a local mod package directory holding
    /// one `operation` (a raw JSON object) plus optional payload bytes.
    fn scenario(
        operation: &str,
        payload: Option<(&str, &[u8])>,
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().unwrap();
        let game_root = temp.path().join("game root");
        fs::create_dir(&game_root).unwrap();
        fs::write(game_root.join("game.bin"), b"original").unwrap();
        let package_root = temp.path().join("mod");
        fs::create_dir(&package_root).unwrap();
        let manifest = format!(
            r#"{{"format_version":1,"package_id":"t.mod","title":"T","version":"1.0","supported_platform":"snes","supported_game":{{"identities":[{{"kind":"loose_rom_sha256","value":"game-sha"}}]}},"operations":[{operation}],"provenance":{{"source":"test"}}}}"#
        );
        fs::write(package_root.join("emuwiz.mod.json"), manifest).unwrap();
        if let Some((rel, bytes)) = payload {
            let path = package_root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        (temp, game_root, package_root)
    }

    fn plan_for(game_root: &Path, package_root: &Path) -> LocalModPackagePlan {
        inspect_local_mod_package(LocalModPackageRequest {
            selected_game: SelectedGameForMod {
                game_root: game_root.to_path_buf(),
                identity: identity(&game_root.join("game.bin")),
            },
            package_root: package_root.to_path_buf(),
        })
    }

    fn render(
        state: &mut LocalModPackagePageState,
        archive: &Path,
        id: Option<&GameIdentityReport>,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let draw = |ctx: &egui::Context| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_local_mod_package_panel(ui, state, archive, id);
            });
        };
        ctx.run(egui::RawInput::default(), draw)
    }

    fn catalogue_fixture() -> ModCatalogueRecord {
        ModCatalogueRecord {
            provider: archivefs_core::mod_catalogue::ModCatalogueProvider {
                name: "Synthetic provider".into(),
                record_id: "record-1".into(),
                source_page_url: "https://example.invalid/mod/record-1".into(),
                schema_version: None,
                imported_at: None,
                snapshot_sha256: None,
            },
            display_title: "Catalogue-only translation".into(),
            author: None,
            version: Some("1.0".into()),
            description: Some("Imported metadata".into()),
            title_hint: None,
            platform: None,
            category: archivefs_core::mod_catalogue::ModCatalogueCategory::GameMod,
            payloads: Vec::new(),
            declared_identity: vec![archivefs_core::mod_catalogue::ModCatalogueIdentity {
                kind: archivefs_core::mod_package::ModIdentityKind::LooseRomSha256,
                value: "game-sha".into(),
            }],
            declared_region: None,
            declared_revision: None,
            destination_intent: archivefs_core::mod_catalogue::ModDestinationIntent::Unknown,
            instructions: None,
            provenance: archivefs_core::mod_catalogue::ModCatalogueProvenance {
                source_terms_url: None,
                licence: None,
                author_or_uploader: None,
                note: None,
            },
            rom_hack: None,
        }
    }

    #[test]
    fn provider_record_renders_as_browse_only_candidate() {
        let temp = tempfile::TempDir::new().unwrap();
        let archive = temp.path().join("game/game.bin");
        fs::create_dir_all(archive.parent().unwrap()).unwrap();
        fs::write(&archive, b"game").unwrap();
        let id = identity(&archive);
        let record = catalogue_fixture();
        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), archive.parent().unwrap().to_path_buf())),
            provider_selection: Some(("Synthetic provider".into(), "record-1".into())),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_local_mod_package_panel_with_catalogue(
                    ui,
                    &mut state,
                    &archive,
                    Some(&id),
                    std::slice::from_ref(&record),
                );
            });
        });
        assert!(text_contains(&output, "Catalogue-only translation"));
        assert!(text_contains(&output, "package has not been downloaded"));
        assert!(text_contains(&output, "Apply is unavailable"));
    }

    fn text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn walk(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(t) => t.galley.text().contains(needle),
                egui::Shape::Vec(v) => v.iter().any(|s| walk(s, needle)),
                _ => false,
            }
        }
        output.shapes.iter().any(|c| walk(&c.shape, needle))
    }

    #[test]
    fn without_identity_evidence_the_panel_refuses_and_fabricates_no_success() {
        let temp = tempfile::TempDir::new().unwrap();
        let archive = temp.path().join("game root/game.bin");
        fs::create_dir_all(archive.parent().unwrap()).unwrap();
        fs::write(&archive, b"x").unwrap();
        let mut state = LocalModPackagePageState::default();
        let output = render(&mut state, &archive, None);
        assert!(text_contains(&output, "Load exact game identity evidence"));
        assert!(!text_contains(&output, "Mod apply finished"));
        assert!(!text_contains(&output, "Review and apply mod"));
        assert!(state.stage.is_none());
    }

    #[test]
    fn without_a_safe_game_folder_the_panel_refuses() {
        let temp = tempfile::TempDir::new().unwrap();
        // archive_path whose parent is not a directory.
        let archive = temp.path().join("nope/game.bin");
        let id = identity(&archive);
        let mut state = LocalModPackagePageState::default();
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "cannot identify a safe game folder"));
        assert!(!text_contains(&output, "Choose local mod folder"));
    }

    #[test]
    fn initial_state_only_offers_a_folder_choice() {
        let (_temp, game_root, _pkg) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            None,
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let mut state = LocalModPackagePageState::default();
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Choose local mod folder"));
        assert!(!text_contains(&output, "Confirm apply"));
        assert!(!text_contains(&output, "Mod apply finished"));
    }

    #[test]
    fn multiple_local_candidates_render_with_plain_states() {
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            Some(("p/new.bin", b"replacement")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let selected = plan_for(&game_root, &package_root);
        let mut review = selected.clone();
        review.package_root = game_root.join("review-mod");
        review.package.as_mut().unwrap().package_id = "review.mod".into();
        review.package.as_mut().unwrap().title = "Review Mod".into();
        review.compatibility.state = ModCompatibilityState::Unknown;
        review.eligible_for_later_apply = false;
        let inspection = LocalModPackageCandidateInspection {
            plans: vec![review, selected],
            blockers: Vec::new(),
        };
        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Candidates(inspection, 1)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Local mod candidates"));
        assert!(text_contains(&output, "Review Mod"));
        assert!(text_contains(&output, "Needs game identity"));
        assert!(text_contains(&output, "Ready to apply"));
        assert!(text_contains(&output, "Choose another folder"));
    }

    #[test]
    fn candidate_order_prioritizes_compatible_then_review_then_incompatible() {
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            Some(("p/new.bin", b"replacement")),
        );
        let mut compatible = plan_for(&game_root, &package_root);
        compatible.package.as_mut().unwrap().package_id = "z-compatible".into();
        let mut review = compatible.clone();
        review.package.as_mut().unwrap().package_id = "a-review".into();
        review.compatibility.state = ModCompatibilityState::Unknown;
        review.eligible_for_later_apply = false;
        let mut incompatible = compatible.clone();
        incompatible.package.as_mut().unwrap().package_id = "a-incompatible".into();
        incompatible.compatibility.state = ModCompatibilityState::Incompatible;
        incompatible.eligible_for_later_apply = false;
        let plans = vec![incompatible, review, compatible];
        let order = candidate_order(&plans);
        assert_eq!(order, vec![2, 1, 0]);
        assert_eq!(best_candidate_index(&plans), 2);
    }

    #[test]
    fn a_safe_inspected_package_reaches_a_preview_with_an_apply_control() {
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            Some(("p/new.bin", b"replacement")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        assert!(plan.eligible_for_later_apply);
        assert!(build_local_mod_package_transaction_plan(&plan).is_ok());

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Planned(plan)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Review and apply mod"));
        assert!(text_contains(&output, "Ready to apply"));
        assert!(text_contains(&output, "Original game: unchanged"));
        assert!(text_contains(&output, "game.bin"));
        assert!(!text_contains(&output, "Cannot apply this mod"));
        // Rendering the preview writes nothing.
        assert_eq!(fs::read(archive).unwrap(), b"original");
    }

    #[test]
    fn a_blocked_package_shows_the_refusal_and_cannot_arm_apply() {
        // Delete is refused as an unsupported operation.
        let (_temp, game_root, package_root) =
            scenario(r#"{"kind":"delete","destination":"game.bin"}"#, None);
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        assert!(!plan.eligible_for_later_apply);
        assert!(build_local_mod_package_transaction_plan(&plan).is_err());

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Planned(plan)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Cannot apply this mod"));
        assert!(!text_contains(&output, "Review and apply mod"));
    }

    #[test]
    fn the_confirm_stage_requires_explicit_confirmation_and_writes_nothing_on_render() {
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"replace","payload":"p/new.bin","destination":"game.bin"}"#,
            Some(("p/new.bin", b"replacement")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Confirm(transaction)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(
            &output,
            "Nothing is written until you confirm"
        ));
        assert!(text_contains(&output, "Confirm apply"));
        assert!(!text_contains(&output, "Mod apply finished"));
        assert_eq!(
            fs::read(archive).unwrap(),
            b"original",
            "just rendering the confirm stage must not apply anything"
        );
    }

    #[test]
    fn the_applied_stage_exposes_an_undo_control() {
        // A create at a fresh nested path: an install has a verified payload
        // digest and needs no pre-existing source, so the end-to-end apply is
        // deterministic here. (The full replace-restore round trip is covered
        // by the core `mod_package` execution tests.)
        let (temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();

        let history_root = temp.path().join("history");
        let backup_root = temp.path().join("backups");
        let result = execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: transaction.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: "gui-test-apply".into(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root,
                backup_root,
            },
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(result.journal_path.is_some());

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Applied(result)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Undo this mod"));
        assert!(matches!(state.stage, Some(Stage::Applied(_))));
    }

    // --- status -> presentation --------------------------------------------

    #[test]
    fn apply_presentation_never_reads_as_a_clean_install_below_success() {
        let success = apply_presentation(SharedApplyStatus::Success);
        assert_eq!(success.headline, "Mod installed");
        assert_eq!(success.tone, widgets::StatusTone::Success);

        let partial = apply_presentation(SharedApplyStatus::PartialFailure);
        assert_ne!(partial.headline, "Mod installed");
        assert_eq!(partial.tone, widgets::StatusTone::Warning);
        assert!(partial.detail.contains("not fully installed"));

        let failed = apply_presentation(SharedApplyStatus::Failed);
        assert_ne!(failed.headline, "Mod installed");
        assert_eq!(failed.tone, widgets::StatusTone::Blocked);
        assert!(failed.detail.contains("unchanged"));
    }

    #[test]
    fn rollback_presentation_reports_success_as_success_not_failure() {
        let success = rollback_presentation(SharedApplyStatus::Success);
        assert_eq!(success.headline, "Mod removed");
        assert_eq!(success.tone, widgets::StatusTone::Success);
        assert_ne!(success.headline, "Mod workflow stopped");

        assert_eq!(
            rollback_presentation(SharedApplyStatus::PartialFailure).tone,
            widgets::StatusTone::Warning
        );
        assert_eq!(
            rollback_presentation(SharedApplyStatus::Failed).tone,
            widgets::StatusTone::Blocked
        );
    }

    /// Applies `manifest` (already-substituted) from `package_root` against a
    /// `game.bin` game tree and returns the raw `SharedApplyResult`.
    fn apply_result(
        temp: &Path,
        game_root: &Path,
        package_root: &Path,
        operation_id: &str,
    ) -> SharedApplyResult {
        let plan = plan_for(game_root, package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();
        execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: transaction.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: operation_id.to_string(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root: temp.join("history"),
                backup_root: temp.join("backups"),
            },
        )
    }

    #[test]
    fn successful_apply_renders_success_wording_and_keeps_undo() {
        let (temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let result = apply_result(temp.path(), &game_root, &package_root, "gui-success");
        assert_eq!(result.journal.status, SharedApplyStatus::Success);
        assert!(has_restorable_changes(&result));

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Applied(result)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Mod installed"));
        assert!(text_contains(&output, "Undo this mod"));
    }

    #[test]
    fn failed_apply_is_not_shown_as_a_clean_install_and_offers_no_undo() {
        // A written journal is not proof of success: force `Failed` with a
        // confirmation whose plan id does not match, then confirm the render
        // arm keys on `journal.status`, not on `journal_path`.
        let (_temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();
        let result = execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: "not-the-real-plan-id".into(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: "gui-failed".into(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root: _temp.path().join("history"),
                backup_root: _temp.path().join("backups"),
            },
        );
        assert_eq!(result.journal.status, SharedApplyStatus::Failed);
        assert!(!has_restorable_changes(&result));

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Applied(result)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Mod was not applied"));
        assert!(!text_contains(&output, "Mod installed"));
        assert!(!text_contains(&output, "Undo this mod"));
    }

    #[test]
    fn partial_apply_is_not_presented_as_clean_success_but_keeps_undo_for_the_changed_subset() {
        let temp = tempfile::TempDir::new().unwrap();
        let game_root = temp.path().join("game root");
        fs::create_dir(&game_root).unwrap();
        fs::write(game_root.join("game.bin"), b"original").unwrap();
        let package_root = temp.path().join("mod");
        fs::create_dir(&package_root).unwrap();
        let manifest = r#"{"format_version":1,"package_id":"t.mod","title":"T","version":"1.0","supported_platform":"snes","supported_game":{"identities":[{"kind":"loose_rom_sha256","value":"game-sha"}]},"operations":[{"kind":"create","payload":"p/a.bin","destination":"mods/a.bin"},{"kind":"create","payload":"p/b.bin","destination":"mods/b.bin"}],"provenance":{"source":"test"}}"#;
        fs::write(package_root.join("emuwiz.mod.json"), manifest).unwrap();
        fs::create_dir_all(package_root.join("p")).unwrap();
        fs::write(package_root.join("p/a.bin"), b"aaaa").unwrap();
        fs::write(package_root.join("p/b.bin"), b"bbbb").unwrap();

        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let plan = plan_for(&game_root, &package_root);
        let transaction = build_local_mod_package_transaction_plan(&plan).unwrap();

        // Race: one create's destination now exists with different bytes, so
        // that entry is skipped while the other still installs.
        fs::create_dir_all(game_root.join("mods")).unwrap();
        fs::write(game_root.join("mods/b.bin"), b"squatter").unwrap();

        let result = execute_shared_apply(
            &transaction,
            &SharedApplyOptions {
                dry_run: false,
                confirmation: Some(SharedApplyConfirmation {
                    plan_id: transaction.plan_id.clone(),
                    general_approved: true,
                    replacement_approved: true,
                }),
                operation_id: "gui-partial".into(),
                timestamp_unix_seconds: 1_700_000_000,
                current_context: transaction.context.clone(),
                history_root: temp.path().join("history"),
                backup_root: temp.path().join("backups"),
            },
        );
        assert_eq!(result.journal.status, SharedApplyStatus::PartialFailure);
        assert!(
            result.journal_path.is_some(),
            "a journal is written even on partial failure"
        );
        assert!(has_restorable_changes(&result));

        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::Applied(result)),
            ..Default::default()
        };
        let output = render(&mut state, &archive, Some(&id));
        assert!(!text_contains(&output, "Mod installed"));
        assert!(text_contains(&output, "only partly applied"));
        assert!(text_contains(&output, "Undo this mod"));
    }

    #[test]
    fn a_completed_rollback_that_succeeded_becomes_rolled_back_not_failed() {
        let (temp, game_root, package_root) = scenario(
            r#"{"kind":"create","payload":"p/new.bin","destination":"mods/added.bin"}"#,
            Some(("p/new.bin", b"added-bytes")),
        );
        let archive = game_root.join("game.bin");
        let id = identity(&archive);
        let history_root = temp.path().join("history");
        let backup_root = temp.path().join("backups");
        let result = apply_result(temp.path(), &game_root, &package_root, "gui-rb-apply");
        let journal_path = result.journal_path.clone().unwrap();

        let preview = preview_shared_rollback(&journal_path, &game_root, &backup_root);
        assert!(preview.available);
        let rollback = execute_shared_rollback(
            &preview,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: preview.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: generate_shared_operation_id(),
                timestamp_unix_seconds: 1_700_000_100,
                history_root,
                backup_root,
            },
        );
        assert_eq!(rollback.status, SharedApplyStatus::Success);

        let (sender, receiver) = mpsc::channel();
        sender.send(rollback).unwrap();
        let mut state = LocalModPackagePageState {
            key: Some((archive.clone(), game_root.clone())),
            provider_selection: None,
            stage: Some(Stage::RollingBack(receiver)),
            ..Default::default()
        };
        assert!(state.poll());
        assert!(
            matches!(
                state.stage,
                Some(Stage::RolledBack(SharedApplyStatus::Success))
            ),
            "a successful rollback must not land in Stage::Failed"
        );

        let output = render(&mut state, &archive, Some(&id));
        assert!(text_contains(&output, "Mod removed"));
        assert!(!text_contains(&output, "Mod workflow stopped"));
    }

    #[test]
    fn a_non_success_rollback_status_renders_a_non_success_state() {
        let temp = tempfile::TempDir::new().unwrap();
        let game_root = temp.path().join("game root");
        fs::create_dir(&game_root).unwrap();
        let archive = game_root.join("game.bin");
        fs::write(&archive, b"x").unwrap();
        let id = identity(&archive);

        for status in [SharedApplyStatus::PartialFailure, SharedApplyStatus::Failed] {
            let mut state = LocalModPackagePageState {
                key: Some((archive.clone(), game_root.clone())),
                provider_selection: None,
                stage: Some(Stage::RolledBack(status)),
                ..Default::default()
            };
            let output = render(&mut state, &archive, Some(&id));
            assert!(!text_contains(&output, "Mod removed"));
            let expected = match status {
                SharedApplyStatus::PartialFailure => "Undo only partly finished",
                _ => "Undo did not complete",
            };
            assert!(text_contains(&output, expected));
        }
    }

    #[test]
    fn a_rollback_worker_that_disconnects_still_fails_loudly() {
        let (sender, receiver) =
            mpsc::channel::<archivefs_core::patch_manager::SharedRollbackResult>();
        drop(sender);
        let mut state = LocalModPackagePageState {
            key: None,
            provider_selection: None,
            stage: Some(Stage::RollingBack(receiver)),
            ..Default::default()
        };
        assert!(state.poll());
        assert!(matches!(state.stage, Some(Stage::Failed(_))));
    }
}
