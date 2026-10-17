//! DuckStation's native cheat install inside the existing Cheats & Mods
//! workflow.
//!
//! It follows the same stages as every other adapter - route, profile,
//! source/candidate/picker, preview, explicit review, apply, result - and the
//! same shared state (`CheatWorkflowState`, `CheatTransactionState`,
//! `workflow.preview`). What differs is only that the change is TWO files
//! (`SERIAL.cht` and the per-game `SERIAL.ini`) that the adapter applies and
//! undoes as ONE operation, so this adapter carries a composite transaction
//! state. The profile always comes from the emulator profile resolver.
//!
//! Undo lives in History: the two child journals are grouped under the
//! adapter's receipt there, so they can never be undone one at a time.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::*;
use archivefs_core::emulator_profile_resolver::ProfileFolder;

use super::duckstation_environment::{select_for_cheats, shared_history_roots};
use crate::emulator_setup::profile_resolution::{ResolutionHealth, layout_label};
use archivefs_core::patch_manager::{
    DiscCountAuthority, DiscQuery, DiscTopologyProof, DuckStationDatabaseAuthority,
    DuckStationNativeApplyFailure, DuckStationNativeApplyOptions, DuckStationNativeOperation,
    DuckStationNativeRefusal, DuckStationNativeRequest, TrustedDatAuthority,
    apply_duckstation_native_plan, duckstation_native_batch_from_catalogue,
    plan_duckstation_native, prove_disc_topology,
};

/// What the identity and library evidence say about this game, as far as the
/// DuckStation planner is concerned.
pub(crate) struct DuckStationGameFacts {
    pub(crate) serial: Option<String>,
    /// The single-disc proof (or exactly which proof is missing).
    pub(crate) proof: DiscTopologyProof,
}

fn failure_message(failure: DuckStationNativeApplyFailure) -> (String, bool) {
    match failure {
        DuckStationNativeApplyFailure::Refused(refusal) => (refusal.explain(), false),
        DuckStationNativeApplyFailure::PublishFailedRolledBack { stage, detail } => (
            format!(
                "The change stopped at \"{stage}\": {detail}. Everything that had been written was rolled back and verified; both files are as they were."
            ),
            false,
        ),
        DuckStationNativeApplyFailure::RollbackIncomplete { stage, detail } => (
            format!(
                "The change stopped at \"{stage}\": {detail}. The two files may not agree; check them before trying again."
            ),
            true,
        ),
    }
}

/// The facts the planner needs: the verified serial and the cached single-disc
/// proof. Nothing is guessed: with no proof the result is `Unproven`.
pub(crate) fn duckstation_facts(
    workflow: &CheatWorkflowState,
    _snapshot: Option<&CachedLibrarySnapshot>,
) -> DuckStationGameFacts {
    let serial = ready_game_identity(workflow)
        .and_then(|report| report.verified_ps1_serial())
        .map(str::to_owned);
    let proof = workflow
        .duckstation
        .disc_proof
        .as_ref()
        .filter(|cached| {
            // A proof read from another installation's database is not reused.
            cached.serial == serial
                && cached.profile_key
                    == workflow
                        .duckstation
                        .resolved
                        .as_ref()
                        .map(|profile| profile.identity.storage_key())
        })
        .map(|cached| cached.proof.clone())
        .unwrap_or(DiscTopologyProof::Unproven {
            missing: vec![
                archivefs_core::patch_manager::MissingDiscProof::ProviderNoOpinion {
                    name: "disc proof".into(),
                    reason: "not computed yet".into(),
                },
            ],
        });
    DuckStationGameFacts { serial, proof }
}

/// Recomputes the single-disc proof when - and only when - its inputs changed.
/// Authorities, strongest first: the library's trusted DAT media sets, then the
/// resolved DuckStation installation's own game database.
pub(crate) fn compute_disc_proof(
    workflow: &CheatWorkflowState,
    snapshot: Option<&CachedLibrarySnapshot>,
) -> Option<DuckStationDiscProofCache> {
    let profile = workflow.duckstation.resolved.as_ref()?;
    let serial = ready_game_identity(workflow)
        .and_then(|report| report.verified_ps1_serial())
        .map(str::to_owned);
    let profile_key = Some(profile.identity.storage_key());
    let sets: &[archivefs_core::media_set::MediaSet] =
        snapshot.map_or(&[], |snapshot| snapshot.media_topology.as_slice());
    if let Some(cached) = &workflow.duckstation.disc_proof
        && cached.serial == serial
        && cached.profile_key == profile_key
        && cached.dat_sets_len == sets.len()
    {
        return None;
    }
    let dat = TrustedDatAuthority { sets };
    let database = DuckStationDatabaseAuthority::for_profile(&profile.profile_root);
    let authorities: [&dyn DiscCountAuthority; 2] = [&dat, &database];
    let proof = prove_disc_topology(
        &DiscQuery {
            serial: serial.as_deref(),
            archive: &workflow.archive_path,
        },
        &authorities,
    );
    Some(DuckStationDiscProofCache {
        serial,
        profile_key,
        dat_sets_len: sets.len(),
        proof,
    })
}

/// The release region a verified PS1 serial's prefix names, when it names one.
fn serial_region(serial: &str) -> Option<&'static str> {
    let prefix = serial.split(['-', '_']).next()?.to_ascii_uppercase();
    match prefix.as_str() {
        "SCUS" | "SLUS" | "LSP" => Some("USA"),
        "SCES" | "SLES" | "SCED" | "SLED" => Some("Europe"),
        "SCPS" | "SLPS" | "SLPM" | "SCPM" | "SIPS" | "PAPX" => Some("Japan"),
        _ => None,
    }
}

/// The regions a catalogue cheat file's region text names (`"USA, Europe"`
/// names two). Unrecognised text names none.
fn catalogue_regions(region: &str) -> Vec<&'static str> {
    let mut regions = Vec::new();
    for part in region.split([',', '/', '+']) {
        let found = match part.trim().to_ascii_lowercase().as_str() {
            "usa" | "us" | "u" | "america" | "north america" | "ntsc-u" => Some("USA"),
            "europe" | "eu" | "e" | "pal" | "uk" | "united kingdom" | "germany" | "france"
            | "spain" | "italy" | "australia" => Some("Europe"),
            "japan" | "jp" | "j" | "ntsc-j" => Some("Japan"),
            _ => None,
        };
        if let Some(found) = found
            && !regions.contains(&found)
        {
            regions.push(found);
        }
    }
    regions
}

/// A cheat file made for another region writes to the wrong addresses, so a
/// region the serial and the catalogue both name, and that differs, refuses.
pub(crate) fn duckstation_region_conflict(
    serial: Option<&str>,
    candidate_region: Option<&str>,
) -> Option<String> {
    let serial = serial?;
    let disc = serial_region(serial)?;
    let named = catalogue_regions(candidate_region?);
    (!named.is_empty() && !named.contains(&disc)).then(|| {
        format!(
            "This cheat file is for {} releases, but the disc's verified serial {serial} is a {disc} release. Cheat codes are region-specific, so nothing was prepared.",
            named.join(" / ")
        )
    })
}

impl ArchiveFsApp {
    /// Re-resolves which DuckStation installation + profile this workflow
    /// uses. Called whenever the workflow is built or the user asks; the
    /// preview key embeds the result, so a change invalidates stale previews.
    pub(crate) fn refresh_duckstation_resolution(&mut self) {
        // The same resolver answer and launch authority Launch acts on.
        let selection = super::duckstation_environment::duckstation_inputs(
            &self.emulator_readiness.emulator_setup_overrides,
        )
        .map(|inputs| select_for_cheats(&inputs));
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        workflow.duckstation.resolved = selection.as_ref().and_then(|item| item.profile.clone());
        workflow.duckstation.selection = selection;
        workflow.duckstation.last_refusal = None;
    }

    pub(crate) fn duckstation_game_facts(&self) -> Option<DuckStationGameFacts> {
        let workflow = self.cheat_workflow.as_ref()?;
        Some(duckstation_facts(workflow, self.database_state.snapshot()))
    }

    /// Builds the two-file preview. Read-only: nothing is written.
    pub(crate) fn start_duckstation_install_preview(&mut self) {
        // Ask the resolver again: the user may have changed the installation
        // in Emulator setup since this workflow opened. The preview key embeds
        // the resolved pair, so an older review cannot be confirmed.
        if self
            .cheat_workflow
            .as_ref()
            .is_some_and(|workflow| workflow.adapter == CheatEmulatorAdapter::DuckStation)
        {
            self.refresh_duckstation_resolution();
        }
        let facts = self.duckstation_game_facts();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        if workflow.adapter != CheatEmulatorAdapter::DuckStation {
            return;
        }
        let archive = workflow.archive_path.clone();
        let key = cheat_preview_key(workflow);
        let reject = |history: &mut OperationHistory, message: String| {
            history.record(HistoryEntry::new(
                ActivityAction::CheatPreview,
                Some(archive.clone()),
                ActivityOutcome::Rejected,
                message,
            ));
        };
        let Some(profile) = workflow.duckstation.resolved.clone() else {
            reject(
                &mut self.history,
                "DuckStation preview blocked: no usable DuckStation installation was resolved."
                    .into(),
            );
            return;
        };
        let Some(selection) = workflow.candidate_selection.as_ref() else {
            return;
        };
        let entries = match selection.selection.resolve(&selection.loaded.document) {
            Ok(entries) => entries,
            Err(error) => {
                reject(
                    &mut self.history,
                    format!("DuckStation preview blocked: {}", error.detail),
                );
                return;
            }
        };
        let candidate_display_name = selection.candidate.display_name.clone();
        let candidate_region = selection.candidate.region.clone();
        let candidate_path = selection.candidate.catalogue_relative_path.clone();
        let facts = facts.unwrap_or(DuckStationGameFacts {
            serial: None,
            proof: DiscTopologyProof::Unproven {
                missing: vec![archivefs_core::patch_manager::MissingDiscProof::NoVerifiedSerial],
            },
        });
        if let Some(conflict) =
            duckstation_region_conflict(facts.serial.as_deref(), candidate_region.as_deref())
        {
            reject(
                &mut self.history,
                format!("DuckStation preview refused: {conflict}"),
            );
            workflow.duckstation.region_refusal = Some((candidate_path, conflict));
            workflow.preview_request = None;
            workflow.preview = CheatStepResource::NotLoaded;
            workflow.transaction = CheatTransactionState::Idle;
            return;
        }
        workflow.duckstation.region_refusal = None;
        let acknowledge = workflow.duckstation.acknowledge_database_shadowing;

        let planned = (|| {
            let cheats = duckstation_native_batch_from_catalogue(&entries);
            let request = DuckStationNativeRequest::for_resolved_profile_with_proof(
                &profile,
                archive.clone(),
                facts.serial.into_iter().collect(),
                &facts.proof,
                DuckStationNativeOperation::AddMany { cheats },
                acknowledge,
            )?;
            let plan = plan_duckstation_native(&request)?;
            Ok::<_, DuckStationNativeRefusal>(plan)
        })();
        let response = match planned {
            Ok(plan) => {
                workflow.duckstation.last_refusal = None;
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatPreview,
                    Some(archive),
                    ActivityOutcome::Completed,
                    "DuckStation preview completed; nothing has been written.",
                ));
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::NativePlanned,
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,
                    bsfree_wii_generated: None,
                    duckstation_generated: Some(GeneratedDuckStationInstall {
                        plan,
                        profile,
                        candidate_display_name,
                    }),
                }
            }
            Err(refusal) => {
                reject(
                    &mut self.history,
                    format!("DuckStation preview refused: {}", refusal.explain()),
                );
                workflow.duckstation.last_refusal = Some(refusal.clone());
                CheatPreviewResponse {
                    key: key.clone(),
                    outcome: CheatPreviewOutcome::Failed(CheatPreviewFailure::DuckStationNative(
                        refusal,
                    )),
                    materialized: None,
                    generated: None,
                    dolphin_generated: None,
                    xenia_generated: None,
                    pcsx2_generated: None,
                    gamecube_gamehacking_generated: None,
                    bsfree_gamecube_generated: None,
                    bsfree_wii_generated: None,
                    duckstation_generated: None,
                }
            }
        };
        workflow.preview_request = Some(key);
        workflow.preview = CheatStepResource::Ready(response);
        workflow.transaction = CheatTransactionState::Idle;
    }

    /// Moves a successful preview to the explicit review stage.
    pub(crate) fn review_duckstation_apply(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let CheatStepResource::Ready(response) = &workflow.preview else {
            return;
        };
        let Some(generated) = response.duckstation_generated.clone() else {
            return;
        };
        if response.key != cheat_preview_key(workflow) {
            return;
        }
        workflow.transaction = CheatTransactionState::NativeReview {
            key: response.key.clone(),
            generated: Box::new(generated),
        };
    }

    /// Applies both files as one operation, after explicit confirmation.
    pub(crate) fn start_duckstation_apply(&mut self, context: egui::Context) {
        // What Launch is authorised to start NOW, not when the preview ran.
        let current = super::duckstation_environment::duckstation_inputs(
            &self.emulator_readiness.emulator_setup_overrides,
        )
        .map(|inputs| select_for_cheats(&inputs));
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let state = std::mem::replace(&mut workflow.transaction, CheatTransactionState::Idle);
        let CheatTransactionState::NativeReview { key, generated } = state else {
            workflow.transaction = state;
            return;
        };
        if key != cheat_preview_key(workflow) {
            return;
        }
        let still_authorised = current
            .as_ref()
            .and_then(|selection| selection.profile.as_ref())
            .is_some_and(|profile| {
                profile.executable == generated.profile.executable
                    && profile.profile_root == generated.profile.profile_root
            });
        if !still_authorised {
            workflow.duckstation.resolved = current.as_ref().and_then(|item| item.profile.clone());
            workflow.duckstation.selection = current;
            workflow.transaction = CheatTransactionState::NativeFailed {
                key,
                message: "The DuckStation installation Launch uses has changed since this preview, so nothing was written. Preview again to install into the current one.".into(),
                needs_attention: false,
            };
            return;
        }
        let archive = workflow.archive_path.clone();
        let Some((history_root, backup_root)) = shared_history_roots() else {
            workflow.transaction = CheatTransactionState::NativeFailed {
                key,
                message:
                    "EmuWiz's history or backup folder is unavailable, so nothing was written."
                        .into(),
                needs_attention: false,
            };
            return;
        };
        let operation_id = generate_shared_operation_id();
        let options = DuckStationNativeApplyOptions {
            general_approved: true,
            replacement_approved: generated.plan.preview.backup_will_be_made,
            operation_id: operation_id.clone(),
            timestamp_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0),
            history_root,
            backup_root,
        };
        let (sender, receiver) = mpsc::channel();
        workflow.transaction = CheatTransactionState::NativeApplying {
            key: key.clone(),
            receiver,
        };
        workflow.transaction_notice = None;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatInstall,
            Some(archive),
            ActivityOutcome::Started,
            format!("DuckStation apply '{operation_id}' started (two files, one operation)."),
        ));
        thread::spawn(move || {
            let result =
                apply_duckstation_native_plan(&generated.plan, &options).map_err(failure_message);
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    /// Rebuilds the list of earlier DuckStation operations for this game from
    /// the receipts and journals on disk. Read-only.
    pub(crate) fn refresh_duckstation_history(&mut self) {
        let roots = shared_history_roots();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let serial = ready_game_identity(workflow)
            .and_then(|report| report.verified_ps1_serial())
            .map(str::to_owned);
        workflow.duckstation.earlier.clear();
        workflow.duckstation.earlier_for = serial.clone();
        let (Some(serial), Some((history_root, backup_root))) = (serial, roots) else {
            return;
        };
        let report = archivefs_core::patch_manager::discover_shared_apply_history(&history_root);
        let receipts =
            archivefs_core::patch_manager::discover_duckstation_native_receipts(&history_root);
        workflow.duckstation.earlier =
            archivefs_core::patch_manager::group_shared_history(&report, receipts, &backup_root)
                .into_iter()
                .filter_map(|row| match row {
                    archivefs_core::patch_manager::GroupedHistoryRow::DuckStationNative(group)
                        if group.receipt.serial == serial =>
                    {
                        Some(*group)
                    }
                    _ => None,
                })
                .collect();
        workflow.duckstation.earlier.sort_by(|a, b| {
            b.receipt
                .timestamp_unix_seconds
                .cmp(&a.receipt.timestamp_unix_seconds)
        });
    }

    /// Previews the ONE undo of a recorded operation (read-only).
    pub(crate) fn start_duckstation_undo_preview(&mut self, receipt_path: PathBuf) {
        let roots = shared_history_roots();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let Some((_, backup_root)) = roots else {
            workflow.duckstation.undo =
                NativeUndoState::Failed("EmuWiz's history or backup folder is unavailable.".into());
            return;
        };
        workflow.duckstation.undo =
            match archivefs_core::patch_manager::read_duckstation_native_receipt(&receipt_path) {
                Ok(receipt) => {
                    let preview = archivefs_core::patch_manager::preview_duckstation_native_undo(
                        &receipt,
                        &backup_root,
                    );
                    NativeUndoState::Review {
                        receipt: Box::new(receipt),
                        preview,
                    }
                }
                Err(error) => {
                    NativeUndoState::Failed(format!("The change record could not be read: {error}"))
                }
            };
    }

    /// Runs the undo after explicit confirmation. The safety check is repeated
    /// NOW: anything that changed since the preview blocks it instead of being
    /// overwritten.
    pub(crate) fn confirm_duckstation_undo(&mut self, context: egui::Context) {
        let roots = shared_history_roots();
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let state = std::mem::take(&mut workflow.duckstation.undo);
        let NativeUndoState::Review { receipt, .. } = state else {
            workflow.duckstation.undo = state;
            return;
        };
        let Some((history_root, backup_root)) = roots else {
            workflow.duckstation.undo =
                NativeUndoState::Failed("EmuWiz's history or backup folder is unavailable.".into());
            return;
        };
        let fresh =
            archivefs_core::patch_manager::preview_duckstation_native_undo(&receipt, &backup_root);
        if !fresh.available {
            // Something changed since the preview: show the new blockers, run nothing.
            workflow.duckstation.undo = NativeUndoState::Review {
                receipt,
                preview: fresh,
            };
            return;
        }
        let options = DuckStationNativeApplyOptions {
            general_approved: true,
            replacement_approved: true,
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0),
            history_root,
            backup_root,
        };
        let archive = workflow.archive_path.clone();
        let (sender, receiver) = mpsc::channel();
        workflow.duckstation.undo = NativeUndoState::Applying { receiver };
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatInstall,
            Some(archive),
            ActivityOutcome::Started,
            format!(
                "Undo of DuckStation operation '{}' started (both files, settings first).",
                receipt.operation_id
            ),
        ));
        thread::spawn(move || {
            let serial = receipt.serial.clone();
            let result = archivefs_core::patch_manager::undo_duckstation_native(&receipt, &options)
                .map(|()| {
                    format!("Both DuckStation files for {serial} were restored and verified.")
                })
                .map_err(|failure| failure_message(failure).0);
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    fn poll_duckstation_undo(&mut self) {
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let NativeUndoState::Applying { receiver } = &workflow.duckstation.undo else {
            return;
        };
        let outcome = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The undo worker stopped unexpectedly; check both DuckStation files.".into())
            }
        };
        let archive = workflow.archive_path.clone();
        let succeeded = outcome.is_ok();
        workflow.duckstation.undo = match &outcome {
            Ok(message) => NativeUndoState::Done(message.clone()),
            Err(message) => NativeUndoState::Failed(message.clone()),
        };
        if succeeded {
            // The result card for the change that was just undone is stale.
            if matches!(
                workflow.transaction,
                CheatTransactionState::NativeResult { .. }
            ) {
                workflow.transaction = CheatTransactionState::Idle;
            }
            workflow.preview = CheatStepResource::NotLoaded;
        }
        workflow.duckstation.earlier_for = None;
        self.history.record(HistoryEntry::new(
            ActivityAction::CheatInstall,
            Some(archive),
            if succeeded {
                ActivityOutcome::Completed
            } else {
                ActivityOutcome::Failed
            },
            match outcome {
                Ok(message) | Err(message) => message,
            },
        ));
        self.shared_history = SharedHistoryState::NotLoaded;
    }

    /// Collects a finished apply. History is reloaded so the grouped row
    /// appears immediately.
    pub(crate) fn poll_duckstation_transaction(&mut self) {
        self.poll_duckstation_undo();
        if self.cheat_workflow.as_ref().is_some_and(|workflow| {
            workflow.adapter == CheatEmulatorAdapter::DuckStation
                && ready_game_identity(workflow).is_some()
                && workflow.duckstation.earlier_for
                    != ready_game_identity(workflow)
                        .and_then(|report| report.verified_ps1_serial())
                        .map(str::to_owned)
        }) {
            self.refresh_duckstation_history();
        }
        if let Some(workflow) = self.cheat_workflow.as_ref()
            && workflow.adapter == CheatEmulatorAdapter::DuckStation
            && let Some(fresh) = compute_disc_proof(workflow, self.database_state.snapshot())
            && let Some(workflow) = self.cheat_workflow.as_mut()
        {
            workflow.duckstation.disc_proof = Some(fresh);
        }
        let Some(workflow) = self.cheat_workflow.as_mut() else {
            return;
        };
        let stale = match &workflow.transaction {
            CheatTransactionState::NativeReview { key, .. }
            | CheatTransactionState::NativeResult { key, .. }
            | CheatTransactionState::NativeFailed { key, .. } => {
                key != &cheat_preview_key(workflow)
            }
            _ => false,
        };
        if stale {
            workflow.transaction = CheatTransactionState::Idle;
            return;
        }
        let CheatTransactionState::NativeApplying { key, receiver } = &workflow.transaction else {
            return;
        };
        let key = key.clone();
        let archive = workflow.archive_path.clone();
        match receiver.try_recv() {
            Ok(Ok(receipt)) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatInstall,
                    Some(archive),
                    ActivityOutcome::Completed,
                    format!(
                        "DuckStation operation '{}' finished. Undo it from History as one change.",
                        receipt.operation_id
                    ),
                ));
                workflow.transaction = CheatTransactionState::NativeResult {
                    key,
                    receipt: Box::new(receipt),
                };
                workflow.duckstation.earlier_for = None;
                self.shared_history = SharedHistoryState::NotLoaded;
            }
            Ok(Err((message, needs_attention))) => {
                self.history.record(HistoryEntry::new(
                    ActivityAction::CheatInstall,
                    Some(archive),
                    ActivityOutcome::Failed,
                    format!("DuckStation apply did not complete: {message}"),
                ));
                workflow.transaction = CheatTransactionState::NativeFailed {
                    key,
                    message,
                    needs_attention,
                };
                self.shared_history = SharedHistoryState::NotLoaded;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                workflow.transaction = CheatTransactionState::NativeFailed {
                    key,
                    message:
                        "The apply worker stopped unexpectedly; check History before trying again."
                            .into(),
                    needs_attention: true,
                };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering (part of the Cheats & Mods page, not a separate page)
// ---------------------------------------------------------------------------

pub(crate) fn show_duckstation_workflow(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
    facts: &DuckStationGameFacts,
    busy: bool,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "DuckStation",
        Some("Cheats are installed into the DuckStation installation EmuWiz resolved for you."),
    );
    widgets::card(ui, |ui| {
        match &workflow.duckstation.selection {
            Some(selection) => {
                crate::emulator_setup::profile_resolution::show_profile_resolution(
                    ui,
                    &selection.presentation,
                );
                for notice in selection.authority.launch_notices() {
                    ui.colored_label(ui.visuals().warn_fg_color, notice);
                }
            }
            None => {
                ui.label("DuckStation could not be looked for on this machine.");
            }
        }
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(ui, "Check again", widgets::ActionStyle::Quiet, true)
                .clicked()
            {
                action = Some(CheatWorkflowAction::RescanDuckStation);
            }
            ui.small(
                "Cheats use the same DuckStation installation Launch uses. To change it, go to Emulator setup.",
            );
        });
    });

    let Some(profile) = workflow.duckstation.resolved.clone() else {
        let blocked = matches!(
            workflow
                .duckstation
                .selection
                .as_ref()
                .map(|item| item.presentation.health),
            Some(ResolutionHealth::Unavailable | ResolutionHealth::Ambiguous)
        ) || workflow.duckstation.selection.as_ref().is_some_and(|item| {
            matches!(
                item.authority,
                crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority::Refused { .. }
            )
        });
        widgets::banner(
            ui,
            if blocked {
                "Choose a DuckStation installation first"
            } else {
                "No usable DuckStation installation"
            },
            "Cheats cannot be previewed until one DuckStation installation is selected in Emulator setup.",
            widgets::StatusTone::Pending,
        );
        return action;
    };

    // The two files this installation will change, as ONE change.
    widgets::card(ui, |ui| {
        ui.strong("What will change");
        let cheats = profile.folders.get(&ProfileFolder::Cheats);
        let game_settings = profile.folders.get(&ProfileFolder::GameSettings);
        let serial = facts.serial.as_deref().unwrap_or("SERIAL");
        ui.label(format!(
            "{serial}.cht  in  {}",
            cheats.map_or("(unresolved)".to_string(), |f| f.path.display().to_string())
        ));
        ui.label(format!(
            "{serial}.ini  in  {}",
            game_settings.map_or("(unresolved)".to_string(), |f| f.path.display().to_string())
        ));
        ui.small(format!(
            "Settings file {} · using {} ({} mode)",
            profile.main_config.display(),
            layout_label(profile.layout),
            workflow
                .duckstation
                .selection
                .as_ref()
                .map(|item| item.presentation.mode_label)
                .unwrap_or("AUTO"),
        ));
    });

    // Identity requirements.
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            match &facts.serial {
                Some(serial) => widgets::status_badge(
                    ui,
                    format!("Verified PS1 serial {serial}"),
                    widgets::StatusTone::Success,
                ),
                None => widgets::status_badge(
                    ui,
                    "PS1 serial not verified yet",
                    widgets::StatusTone::Pending,
                ),
            };
            widgets::status_badge(
                ui,
                match &facts.proof {
                    DiscTopologyProof::SingleDisc { .. } => "Single disc confirmed",
                    DiscTopologyProof::MultiDisc { .. } => "Multi-disc game",
                    DiscTopologyProof::Conflict { .. } => "Sources disagree",
                    DiscTopologyProof::Unproven { .. } => "Single disc not confirmed",
                },
                if matches!(facts.proof, DiscTopologyProof::SingleDisc { .. }) {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Pending
                },
            );
        });
        ui.label(facts.proof.describe());
        ui.small("Several cheats can be installed in one change. Hash-specific cheat files and multi-disc games are refused honestly.");
    });

    // Source: the same trusted-catalogue stages every catalogue adapter uses.
    action = show_cheat_workflow_step2(ui, workflow, busy, clipboard).or(action);
    ui.add_space(theme::SECTION_GAP);
    action = show_cheat_candidate_stages(ui, workflow, clipboard).or(action);
    ui.add_space(theme::SECTION_GAP);
    action = show_duckstation_preview_and_apply(ui, workflow).or(action);
    ui.add_space(theme::SECTION_GAP);
    action = show_duckstation_undo_and_history(ui, workflow).or(action);
    action
}

/// The one Undo, and the earlier operations recorded for this game. Each
/// operation is ONE row however many files and cheats it covers.
fn show_duckstation_undo_and_history(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    use archivefs_core::patch_manager::NativeGroupState;
    let mut action = None;
    match &workflow.duckstation.undo {
        NativeUndoState::Idle => {}
        NativeUndoState::Review { receipt, preview } => {
            widgets::card(ui, |ui| {
                widgets::status_badge(
                    ui,
                    if preview.available {
                        "Undo available (both files)"
                    } else {
                        "Undo blocked"
                    },
                    if preview.available {
                        widgets::StatusTone::Warning
                    } else {
                        widgets::StatusTone::Blocked
                    },
                );
                ui.strong(format!("Undo DuckStation cheats for {}", receipt.serial));
                ui.label("Restores the game's settings file first, then its cheat file, as one change. A file you changed since is never overwritten.");
                for reason in &preview.reasons {
                    ui.label(format!("Blocked: {reason}"));
                }
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        "Confirm undo",
                        widgets::ActionStyle::Primary,
                        preview.available,
                    )
                    .clicked()
                    {
                        action = Some(CheatWorkflowAction::ConfirmDuckStationUndo);
                    }
                    if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true)
                        .clicked()
                    {
                        action = Some(CheatWorkflowAction::CancelDuckStationUndo);
                    }
                });
            });
        }
        NativeUndoState::Applying { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Restoring both DuckStation files and checking the result…");
            });
        }
        NativeUndoState::Done(message) => {
            widgets::banner(ui, "Undo complete", message, widgets::StatusTone::Success);
            if widgets::action_button(ui, "Close", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(CheatWorkflowAction::CancelDuckStationUndo);
            }
        }
        NativeUndoState::Failed(message) => {
            widgets::banner(
                ui,
                "Undo did not complete",
                message,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Close", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(CheatWorkflowAction::CancelDuckStationUndo);
            }
        }
    }
    if workflow.duckstation.earlier.is_empty() {
        return action;
    }
    widgets::section_header(
        ui,
        "Earlier DuckStation changes for this game",
        Some("Each is one change covering both files. Undo restores all of it or none of it."),
    );
    if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Quiet, true).clicked() {
        action = Some(CheatWorkflowAction::RefreshDuckStationHistory);
    }
    let can_start = matches!(workflow.duckstation.undo, NativeUndoState::Idle);
    for group in &workflow.duckstation.earlier {
        let receipt = &group.receipt;
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let (label, tone) = match &group.state {
                    NativeGroupState::UndoAvailable => ("Applied", widgets::StatusTone::Success),
                    NativeGroupState::AlreadyUndone => ("Undone", widgets::StatusTone::Info),
                    NativeGroupState::PartiallyUndone { .. } => {
                        ("Partly undone", widgets::StatusTone::Warning)
                    }
                    NativeGroupState::Blocked(_) => {
                        ("Undo unavailable", widgets::StatusTone::Warning)
                    }
                };
                widgets::status_badge(ui, label, tone);
                let count = receipt.cheats_added.len() + receipt.cheats_changed.len();
                ui.strong(format!(
                    "{count} cheat{} · {}",
                    if count == 1 { "" } else { "s" },
                    format_unix_timestamp_utc(receipt.timestamp_unix_seconds as i64)
                ));
            });
            for name in receipt.cheats_added.iter().chain(&receipt.cheats_changed) {
                ui.label(format!("• {name}"));
            }
            if let NativeGroupState::Blocked(reasons) = &group.state {
                for reason in reasons {
                    ui.label(reason);
                }
            }
            if let NativeGroupState::PartiallyUndone {
                restored,
                remaining,
                reasons,
                ..
            } = &group.state
            {
                ui.label(format!(
                    "An earlier undo stopped partway: {} restored, {} still changed.",
                    restored.join(", "),
                    remaining.join(", ")
                ));
                for reason in reasons {
                    ui.label(reason);
                }
            }
            let actionable = match &group.state {
                NativeGroupState::UndoAvailable => Some("Preview undo"),
                NativeGroupState::PartiallyUndone {
                    resumable: true, ..
                } => Some("Finish undo"),
                _ => None,
            };
            if let Some(path) = &receipt.receipt_path
                && let Some(label) = actionable
                && widgets::action_button(ui, label, widgets::ActionStyle::Secondary, can_start)
                    .clicked()
            {
                action = Some(CheatWorkflowAction::PreviewDuckStationUndo(path.clone()));
            }
        });
    }
    action
}

fn show_duckstation_preview_and_apply(
    ui: &mut egui::Ui,
    workflow: &mut CheatWorkflowState,
) -> Option<CheatWorkflowAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "Preview and install",
        Some("One preview, one confirmation, one undo - even though two files change."),
    );
    match &workflow.transaction {
        CheatTransactionState::NativeApplying { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Installing both files…");
            });
            return action;
        }
        CheatTransactionState::NativeResult { receipt, .. } => {
            widgets::banner(
                ui,
                "Cheat installed",
                &format!(
                    "{} cheat(s) added for {}. Both files were written and read back. DuckStation switches on the cheats this game's settings list when it next starts the game; EmuWiz has not seen them running. Undo it from History as one change.",
                    receipt.cheats_added.len() + receipt.cheats_changed.len(),
                    receipt.serial
                ),
                widgets::StatusTone::Success,
            );
            if let Some(path) = &receipt.receipt_path
                && matches!(workflow.duckstation.undo, NativeUndoState::Idle)
                && widgets::action_button(
                    ui,
                    "Undo this change",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
            {
                action = Some(CheatWorkflowAction::PreviewDuckStationUndo(path.clone()));
            }
            for warning in &receipt.warnings {
                ui.small(format!("{warning:?}"));
            }
            return action;
        }
        CheatTransactionState::NativeFailed {
            message,
            needs_attention,
            ..
        } => {
            widgets::banner(
                ui,
                if *needs_attention {
                    "Needs attention"
                } else {
                    "Nothing was left half-done"
                },
                message,
                if *needs_attention {
                    widgets::StatusTone::Blocked
                } else {
                    widgets::StatusTone::Warning
                },
            );
            return action;
        }
        _ => {}
    }

    if let Some((refused_for, conflict)) = &workflow.duckstation.region_refusal
        && workflow
            .candidate_selection
            .as_ref()
            .is_some_and(|selection| selection.candidate.catalogue_relative_path == *refused_for)
    {
        widgets::banner(
            ui,
            "This cheat file is for another region",
            conflict,
            widgets::StatusTone::Blocked,
        );
    }
    let CheatStepResource::Ready(response) = &workflow.preview else {
        match &workflow.preview {
            CheatStepResource::Loading { .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Building the preview…");
                });
            }
            _ => {
                ui.label("Pick a cheat above, then choose \"Preview the installed file\".");
            }
        }
        return action;
    };
    match &response.outcome {
        CheatPreviewOutcome::Failed(CheatPreviewFailure::DuckStationNative(refusal)) => {
            widgets::banner(
                ui,
                "DuckStation cannot install this cheat yet",
                &refusal.explain(),
                widgets::StatusTone::Pending,
            );
            if matches!(refusal, DuckStationNativeRefusal::DatabaseShadowingUnknown)
                && ui
                    .checkbox(
                        &mut workflow.duckstation.acknowledge_database_shadowing,
                        "I understand DuckStation may also load community cheats with the same name",
                    )
                    .changed()
            {
                action = Some(CheatWorkflowAction::BuildInstallPreview);
            }
            return action;
        }
        CheatPreviewOutcome::NativePlanned => {}
        _ => return action,
    }
    let Some(generated) = response.duckstation_generated.as_ref() else {
        return action;
    };
    let preview = &generated.plan.preview;
    widgets::card(ui, |ui| {
        ui.strong(format!(
            "{} for {}",
            generated.candidate_display_name, preview.serial
        ));
        ui.small(format!(
            "Installing into {} ({})",
            generated.profile.profile_root.display(),
            layout_label(generated.profile.layout)
        ));
        ui.label(format!(
            "File 1 of 2: {}  ({})",
            preview.cht_path.display(),
            if preview.cht_exists {
                "updated"
            } else {
                "created"
            }
        ));
        ui.label(format!(
            "File 2 of 2: {}  ({})",
            preview.game_ini_path.display(),
            if preview.game_ini_exists {
                "updated"
            } else {
                "created"
            }
        ));
        // Which cheats this game's settings will switch on. Being in the file
        // and switched on in settings is all EmuWiz can confirm; whether the
        // game reacts is only known once DuckStation runs it.
        for name in &preview.cheats_added {
            let state = if preview.enable_entries_added.contains(name) {
                "switched on in this game's settings"
            } else {
                "added, left off"
            };
            ui.label(format!("+ adds cheat \"{name}\" ({state})"));
        }
        for name in &preview.enable_entries_removed {
            ui.label(format!("- switches off cheat \"{name}\""));
        }
        for name in &preview.cheats_changed {
            ui.label(format!("~ changes cheat \"{name}\""));
        }
        if preview.backup_will_be_made {
            ui.small("Existing files are backed up first.");
        }
        if preview.no_op {
            ui.label("Already installed - no change to apply.");
        }
        for warning in &preview.warnings {
            ui.colored_label(ui.visuals().warn_fg_color, format!("• {warning:?}"));
        }
    });
    match &workflow.transaction {
        CheatTransactionState::NativeReview { .. } => {
            widgets::banner(
                ui,
                "Confirm",
                "Install this cheat? Both files change together, and one Undo in History restores both.",
                widgets::StatusTone::Info,
            );
            ui.horizontal(|ui| {
                if widgets::action_button(ui, "Install", widgets::ActionStyle::Primary, true)
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::ConfirmApply);
                }
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    action = Some(CheatWorkflowAction::CancelApply);
                }
            });
        }
        _ => {
            if widgets::action_button(
                ui,
                "Review changes",
                widgets::ActionStyle::Primary,
                !preview.no_op,
            )
            .clicked()
            {
                action = Some(CheatWorkflowAction::ReviewApply);
            }
        }
    }
    action
}
