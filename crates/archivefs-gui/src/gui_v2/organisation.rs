//! Native Organisation landing and flow coordinator.
//!
//! This module owns presentation state only. Canonical organisation delegates
//! to `rom_organisation_page`; Playing Library, RomM, ES-DE and RetroDECK all
//! delegate to `playing_library_page` and their existing core planners.

use super::{
    App,
    routes::{Route, Section},
};
use crate::ui::{components as widgets, platform_artwork::paint_platform_glyph_at, theme};
use crate::{
    playing_library_page::PlayingLibraryDestination,
    rom_organisation_page::{self, RomOrganisationPageAction},
};
use eframe::egui::{self, RichText};
use std::path::PathBuf;

use archivefs_core::dat::mame_arcade_join::{
    load_verified_mame_0174, refresh_mame_member_evidence,
};
use archivefs_core::dat::mame_merged_reconstruction::{
    MAME_RECONSTRUCTION_WORKFLOW, MameMergedReconstructionPlan, apply_staged_reconstruction_output,
    build_merged_reconstruction_plan,
};
use archivefs_core::dat::rename_apply::model::{RenameTransaction, TransactionState};
use archivefs_core::dat::rename_apply::{
    default_rename_transaction_dir, rollback_transaction_confined,
};
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::{Database, default_database_path};
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum OrganisationView {
    #[default]
    Landing,
    VerifiedGames,
    PlayingLibrary,
    MameNormalizer,
}

pub(super) struct OrganisationState {
    pub(super) view: OrganisationView,
    pub(super) mame_root: Option<PathBuf>,
    pub(super) mame_dat: Option<PathBuf>,
    pub(super) mame_plan: Option<MameMergedReconstructionPlan>,
    pub(super) mame_message: Option<String>,
    pub(super) mame_evidence_set: String,
    pub(super) mame_confirmation: String,
    pub(super) mame_publish_pending: bool,
    pub(super) mame_undo_pending: Option<String>,
    pub(super) mame_history: Vec<RenameTransaction>,
}

impl Default for OrganisationState {
    fn default() -> Self {
        Self {
            view: OrganisationView::Landing,
            mame_root: None,
            mame_dat: None,
            mame_plan: None,
            mame_message: None,
            mame_evidence_set: String::new(),
            mame_confirmation: String::new(),
            mame_publish_pending: false,
            mame_undo_pending: None,
            mame_history: Vec::new(),
        }
    }
}

pub(super) fn mame_publish_confirmation_phrase(outputs: usize) -> String {
    format!("PUBLISH MAME {outputs} OUTPUTS")
}

fn mame_undo_confirmation_phrase(parent: &str) -> String {
    format!("UNDO MAME {parent}")
}

/// The visual identity for one organisation target. Purely presentational:
/// it only selects an accent colour and a small drawn motif, never a
/// behaviour or a backend state. Kept distinct so Playing Library, RomM,
/// ES-DE and RetroDECK read apart from each other at a glance without
/// becoming a "poster wall" of unrelated artwork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardKind {
    /// Canonical rename/arrange of verified files in place - not a
    /// destination, so it gets the neutral folder identity.
    VerifiedGames,
    PlayingLibrary,
    RomM,
    EsDe,
    RetroDeck,
}

impl CardKind {
    fn accent(self) -> egui::Color32 {
        match self {
            CardKind::VerifiedGames => theme::SECONDARY_TEXT,
            CardKind::PlayingLibrary => theme::PRIMARY_ACTION_HOVER,
            // A blue-family indigo: a distinct identity for RomM that still
            // sits inside the app's existing blue theme rather than
            // introducing an unrelated hue.
            CardKind::RomM => egui::Color32::from_rgb(99, 102, 241),
            CardKind::EsDe => theme::TEAL,
            CardKind::RetroDeck => theme::AMBER,
        }
    }

    /// Paints this target's small restrained motif into `rect`. Every shape
    /// here is drawn with the painter directly (the same technique already
    /// used for Disc Conversion's/Tape Analysis's hero motifs) - no new
    /// bitmap asset, no external logo, nothing fetched or decoded per frame.
    fn paint(self, ui: &egui::Ui, rect: egui::Rect) {
        let painter = ui.painter();
        let accent = self.accent();
        match self {
            CardKind::VerifiedGames => {
                // A folder: body plus a small top tab. Reads as "your
                // existing files, tidied in place".
                let body = egui::Rect::from_min_max(
                    rect.min + egui::vec2(rect.width() * 0.08, rect.height() * 0.28),
                    rect.max - egui::vec2(rect.width() * 0.08, rect.height() * 0.12),
                );
                let tab = egui::Rect::from_min_size(
                    body.min,
                    egui::vec2(body.width() * 0.42, rect.height() * 0.12),
                );
                painter.rect_filled(tab, 1.5, accent.gamma_multiply(0.85));
                painter.rect_stroke(
                    body,
                    2.0,
                    egui::Stroke::new(1.6_f32, accent),
                    egui::StrokeKind::Inside,
                );
            }
            CardKind::PlayingLibrary => {
                // A tidy shelf: one baseline with a handful of curated
                // "spines" of varying height standing on it - a clean,
                // deliberately organised set rather than a messy pile.
                let base_y = rect.max.y - rect.height() * 0.14;
                painter.line_segment(
                    [
                        egui::pos2(rect.min.x + rect.width() * 0.06, base_y),
                        egui::pos2(rect.max.x - rect.width() * 0.06, base_y),
                    ],
                    egui::Stroke::new(1.8_f32, accent),
                );
                let heights = [0.42, 0.62, 0.5, 0.7, 0.38];
                let count = heights.len() as f32;
                let gap = rect.width() * 0.03;
                let usable = rect.width() * 0.82;
                let spine_w = (usable - gap * (count - 1.0)) / count;
                for (index, fraction) in heights.iter().enumerate() {
                    let x = rect.min.x + rect.width() * 0.09 + index as f32 * (spine_w + gap);
                    let h = rect.height() * fraction;
                    let spine = egui::Rect::from_min_max(
                        egui::pos2(x, base_y - h),
                        egui::pos2(x + spine_w, base_y),
                    );
                    let tint = if index % 2 == 0 { 1.0 } else { 0.7 };
                    painter.rect_filled(spine, 1.0, accent.gamma_multiply(tint));
                }
            }
            CardKind::RomM => {
                // A small server stack: horizontal bars, each with a status
                // light - "a library prepared for a server to read".
                let bars = 3;
                let gap = rect.height() * 0.08;
                let bar_h = (rect.height() * 0.7 - gap * (bars as f32 - 1.0)) / bars as f32;
                for index in 0..bars {
                    let top = rect.min.y + rect.height() * 0.12 + index as f32 * (bar_h + gap);
                    let bar = egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + rect.width() * 0.1, top),
                        egui::vec2(rect.width() * 0.8, bar_h),
                    );
                    painter.rect_stroke(
                        bar,
                        1.5,
                        egui::Stroke::new(1.4_f32, accent),
                        egui::StrokeKind::Inside,
                    );
                    painter.circle_filled(
                        egui::pos2(bar.max.x - bar.height() * 0.6, bar.center().y),
                        bar.height() * 0.18,
                        accent,
                    );
                }
            }
            CardKind::EsDe => {
                // A 2x2 gamelist grid: "your games, laid out for a
                // frontend to read".
                let pad = rect.width() * 0.1;
                let gap = rect.width() * 0.08;
                let cell = (rect.width() - pad * 2.0 - gap) / 2.0;
                for row in 0..2 {
                    for col in 0..2 {
                        let min = egui::pos2(
                            rect.min.x + pad + col as f32 * (cell + gap),
                            rect.min.y + pad + row as f32 * (cell + gap),
                        );
                        let tile = egui::Rect::from_min_size(min, egui::vec2(cell, cell));
                        painter.rect_filled(tile, 2.0, accent.gamma_multiply(0.35));
                        painter.rect_stroke(
                            tile,
                            2.0,
                            egui::Stroke::new(1.2_f32, accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            }
            CardKind::RetroDeck => {
                // The existing bundled "handheld" category glyph, tinted
                // with RetroDECK's own accent - a portable device, visually
                // separate from ES-DE's frontend grid and RomM's server
                // stack, with no new artwork invented.
                paint_platform_glyph_at(
                    painter,
                    rect.center(),
                    rect.width() * 0.85,
                    accent,
                    "handheld",
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ActionCard {
    title: &'static str,
    description: &'static str,
    semantics: &'static str,
    destination: Option<PlayingLibraryDestination>,
    kind: CardKind,
}

const ACTIONS: [ActionCard; 5] = [
    ActionCard {
        title: "Organise verified games",
        description: "Rename or arrange verified files into clean platform folders.",
        semantics: "Moves or renames original files · Preview required · Undo available",
        destination: None,
        kind: CardKind::VerifiedGames,
    },
    ActionCard {
        title: "Build a clean playing library",
        description: "Create a tidy linked library without moving your original games.",
        semantics: "Source untouched · Creates links · Preview required · Undo available",
        destination: Some(PlayingLibraryDestination::Generic),
        kind: CardKind::PlayingLibrary,
    },
    ActionCard {
        title: "Organise for RomM",
        description: "Create a RomM-ready library using reviewed platform folders and safe links.",
        semantics: "Source untouched · Creates links · Visibility check required",
        destination: Some(PlayingLibraryDestination::Romm),
        kind: CardKind::RomM,
    },
    ActionCard {
        title: "Export to ES-DE",
        description: "Build a clean linked library, then update gamelist.xml safely.",
        semantics: "Source untouched · Creates links · Metadata updated separately",
        destination: Some(PlayingLibraryDestination::EsDe),
        kind: CardKind::EsDe,
    },
    ActionCard {
        title: "Prepare for RetroDECK",
        description: "Create the linked layout RetroDECK expects and publish its ES-DE metadata.",
        semantics: "Source untouched · Creates links · Sandbox visibility required",
        destination: Some(PlayingLibraryDestination::RetroDeck),
        kind: CardKind::RetroDeck,
    },
];

fn primary(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(text).strong())
            .fill(theme::PRIMARY_ACTION)
            .min_size(egui::vec2(180.0, 44.0)),
    )
    .clicked()
}

/// Small square motif plate, matching the plate treatment already used for
/// platform tiles on Home/Platforms (`gui_v2::visual_pages`): a dark backing
/// square with the motif painted inside it.
fn motif_plate(ui: &mut egui::Ui, side: f32, kind: CardKind) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    ui.painter().rect_filled(rect, 8.0, theme::DEEP_BACKGROUND);
    kind.paint(ui, rect.shrink(side * 0.12));
}

fn show_mame_normalizer(ui: &mut egui::Ui, state: &mut OrganisationState) {
    ui.label("Merged reconstruction is read-only until an explicitly reviewed staged apply.");
    ui.horizontal_wrapped(|ui| {
        if ui.button("Choose MAME folder…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose MAME folder")
                .pick_folder()
        {
            state.mame_root = Some(path);
            state.mame_plan = None;
        }
        if ui.button("Choose DAT…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("DAT files", &["dat", "xml"])
                .pick_file()
        {
            state.mame_dat = Some(path);
            state.mame_plan = None;
        }
    });
    ui.label(format!(
        "MAME folder: {}",
        state
            .mame_root
            .as_deref()
            .map_or("not selected".into(), |p| p.display().to_string())
    ));
    ui.label(format!(
        "DAT: {}",
        state
            .mame_dat
            .as_deref()
            .map_or("not selected".into(), |p| p.display().to_string())
    ));
    ui.separator();
    ui.heading("Refresh physical member evidence");
    ui.label("Read-only: reusable checksum evidence is stored before any repair is considered.");
    ui.horizontal(|ui| {
        ui.label("Optional set/family:");
        ui.text_edit_singleline(&mut state.mame_evidence_set);
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Refresh selected family").clicked() {
            let family = state.mame_evidence_set.trim().to_string();
            if family.is_empty() {
                state.mame_message = Some("Enter a MAME set name first.".into());
            } else {
                refresh_mame_evidence(state, Some(family));
            }
        }
        if ui.button("Refresh selected folder").clicked() {
            refresh_mame_evidence(state, None);
        }
    });
    if ui.button("Preview merged reconstruction").clicked() {
        match (&state.mame_root, &state.mame_dat) {
            (Some(root), Some(dat_path)) => match current_mame_reconstruction_plan(
                root,
                dat_path,
                state.mame_evidence_set.trim(),
            ) {
                Ok(plan) => state.mame_plan = Some(plan),
                Err(error) => state.mame_message = Some(error),
            },
            _ => state.mame_message = Some("Choose both the MAME folder and its DAT first.".into()),
        }
    }
    if let Some(message) = &state.mame_message {
        ui.label(message);
    }
    if let Some(plan) = &state.mame_plan {
        ui.label(format!(
            "Parent: {} · clones: {}",
            plan.parent,
            plan.clones.join(", ")
        ));
        ui.label(format!("Destination: {}", plan.destination.display()));
        ui.label(format!(
            "{} required members · {} verified sources",
            plan.required_members.len(),
            plan.sources.len()
        ));
        for (label, values) in [
            ("Missing", &plan.missing_members),
            ("Duplicate candidates", &plan.duplicate_candidates),
            ("Bad hashes", &plan.hash_mismatches),
            ("Unresolved ownership", &plan.unresolved_ownership),
            ("Collisions", &plan.collisions),
        ] {
            if !values.is_empty() {
                ui.label(format!("{label}: {}", values.join(", ")));
            }
        }
        ui.label(if plan.ready_to_apply {
            "Ready to publish after explicit confirmation."
        } else {
            "Publish blocked: evidence is not sufficient."
        });
        for reason in &plan.reasons {
            ui.label(format!("• {reason}"));
        }
        ui.collapsing("Verified ownership", |ui| {
            for source in &plan.sources {
                ui.label(format!(
                    "{} ← {}::{}",
                    source.target_name,
                    source.archive_path.display(),
                    source.current_name
                ));
            }
        });
        if plan.ready_to_apply {
            if !state.mame_publish_pending {
                if ui.button("Publish reconstructed merged output").clicked() {
                    state.mame_publish_pending = true;
                    state.mame_confirmation.clear();
                }
            } else {
                ui.group(|ui| {
                    ui.strong("Confirm publication");
                    let phrase = mame_publish_confirmation_phrase(1);
                    ui.label(format!(
                        "Type {phrase} exactly to create the destination ZIP."
                    ));
                    ui.text_edit_singleline(&mut state.mame_confirmation);
                    let confirmed = state.mame_confirmation == phrase;
                    if ui
                        .add_enabled(confirmed, egui::Button::new("Publish now"))
                        .clicked()
                    {
                        state.publish_mame();
                    }
                    if ui.button("Cancel").clicked() {
                        state.mame_publish_pending = false;
                        state.mame_confirmation.clear();
                    }
                });
            }
        }
    }
    if !state.mame_history.is_empty() {
        ui.separator();
        ui.heading("MAME reconstruction history");
        let history = state.mame_history.clone();
        for transaction in history.iter().rev() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(format!(
                    "{} · {} · {} output",
                    transaction.transaction_id,
                    transaction.state.label(),
                    transaction.entries.len()
                ));
                if let Some(entry) = transaction.entries.first() {
                    ui.label(format!("Destination: {}", entry.destination_path.display()));
                }
                ui.label(format!("Recorded: {}", transaction.created_at_unix));
                ui.label(match transaction.state {
                    TransactionState::Applied => "Verified publication complete; undo is available.",
                    TransactionState::RolledBack => "Rolled back; the generated destination is removed.",
                    _ => "Recovery is required before this transaction can be considered complete.",
                });
                if transaction.is_rollbackable() {
                    if state.mame_undo_pending.as_deref() != Some(transaction.transaction_id.as_str())
                        && ui.button("Undo published MAME output").clicked()
                    {
                        state.mame_undo_pending = Some(transaction.transaction_id.clone());
                        state.mame_confirmation.clear();
                    }
                    if state.mame_undo_pending.as_deref() == Some(transaction.transaction_id.as_str()) {
                        let phrase = mame_undo_confirmation_phrase(
                            transaction
                                .unknown
                                .get("mame_parent")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("SET"),
                        );
                        ui.label(format!("Type {phrase} exactly to confirm undo."));
                        ui.text_edit_singleline(&mut state.mame_confirmation);
                        let confirmed = state.mame_confirmation == phrase;
                        if ui
                            .add_enabled(confirmed, egui::Button::new("Undo now"))
                            .clicked()
                        {
                            state.undo_mame(&transaction.transaction_id);
                        }
                        if ui.button("Cancel undo").clicked() {
                            state.mame_undo_pending = None;
                            state.mame_confirmation.clear();
                        }
                    }
                }
                ui.collapsing("Advanced transaction details", |ui| {
                    ui.label(format!("State: {}", transaction.state.label()));
                    ui.label("The shared journal is the recovery record; source archives are never rollback targets.");
                });
            });
        }
    }
}

fn current_mame_reconstruction_plan(
    root: &std::path::Path,
    dat_path: &std::path::Path,
    requested_set: &str,
) -> Result<MameMergedReconstructionPlan, String> {
    if requested_set.is_empty() {
        return Err("Enter a parent or clone set name before previewing.".into());
    }
    let dat = load_verified_mame_0174(dat_path)?;
    let digest = Sha256::digest(std::fs::read(dat_path).map_err(|e| e.to_string())?);
    let dat_sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let database_path = default_database_path().map_err(|e| e.to_string())?;
    let database = Database::open_read_only(&database_path).map_err(|e| e.to_string())?;
    let joins = database
        .mame_arcade_join_paths_for_dat(&dat_sha256)
        .map_err(|e| e.to_string())?;
    if joins.is_empty() {
        return Err(
            "no persisted verified MAME evidence is available; refresh this family first".into(),
        );
    }
    build_merged_reconstruction_plan(root, &dat.parsed, &joins, requested_set, &dat_sha256)
}

fn mame_staging_root(root: &std::path::Path) -> PathBuf {
    root.join(".emuwiz-mame-staging")
}

fn is_mame_reconstruction(transaction: &RenameTransaction) -> bool {
    transaction
        .unknown
        .get("workflow")
        .and_then(serde_json::Value::as_str)
        == Some(MAME_RECONSTRUCTION_WORKFLOW)
}

fn load_mame_history() -> Vec<RenameTransaction> {
    let Ok(journal_dir) = default_rename_transaction_dir() else {
        return Vec::new();
    };
    let (transactions, _) = archivefs_core::dat::rename_apply::list_journals(&journal_dir);
    transactions
        .into_iter()
        .filter(is_mame_reconstruction)
        .collect()
}

impl OrganisationState {
    pub(super) fn publish_mame(&mut self) {
        let (Some(plan), Some(root), Some(dat_path)) = (
            self.mame_plan.clone(),
            self.mame_root.clone(),
            self.mame_dat.clone(),
        ) else {
            self.mame_message = Some("Preview a MAME reconstruction before publishing it.".into());
            return;
        };
        if !plan.ready_to_apply {
            self.mame_message =
                Some("Publish is blocked until every reconstruction problem is resolved.".into());
            return;
        }
        let current =
            match current_mame_reconstruction_plan(&root, &dat_path, self.mame_evidence_set.trim())
            {
                Ok(current) => current,
                Err(error) => {
                    self.mame_message = Some(format!("The reviewed plan is stale: {error}"));
                    return;
                }
            };
        if current != plan {
            self.mame_message = Some(
                "The reviewed MAME plan is stale because its evidence or destination changed. Preview again before publishing.".into(),
            );
            self.mame_plan = Some(current);
            return;
        }
        let journal_dir = match default_rename_transaction_dir() {
            Ok(path) => path,
            Err(error) => {
                self.mame_message = Some(format!(
                    "MAME publication could not start because its recovery journal is unavailable: {error}"
                ));
                return;
            }
        };
        let staging_root = mame_staging_root(&root);
        match apply_staged_reconstruction_output(&plan, &staging_root, &journal_dir) {
            Ok(outcome) => {
                let transaction = outcome.transaction;
                self.mame_history
                    .retain(|item| item.transaction_id != transaction.transaction_id);
                self.mame_history.push(transaction);
                self.mame_confirmation.clear();
                self.mame_publish_pending = false;
                self.mame_message = Some(format!(
                    "Published {} with staged verification complete. The source archives were not changed; recovery is available from this transaction's journal.",
                    plan.destination.display()
                ));
            }
            Err(error) => {
                self.mame_history = load_mame_history();
                self.mame_message = Some(format!(
                    "MAME publication stopped safely before claiming success: {error}"
                ));
            }
        }
    }

    pub(super) fn undo_mame(&mut self, transaction_id: &str) {
        let Some(index) = self
            .mame_history
            .iter()
            .position(|transaction| transaction.transaction_id == transaction_id)
        else {
            self.mame_message =
                Some("That MAME transaction is no longer in the recovery journal.".into());
            return;
        };
        let mut transaction = self.mame_history[index].clone();
        if !transaction.is_rollbackable() {
            self.mame_message = Some(format!(
                "Undo is unavailable for this MAME transaction; its recorded state is {}.",
                transaction.state.label()
            ));
            return;
        }
        let Some(destination_parent) = transaction
            .entries
            .first()
            .and_then(|entry| entry.destination_path.parent())
            .map(PathBuf::from)
        else {
            self.mame_message =
                Some("Undo is unavailable because the journal has no destination.".into());
            return;
        };
        let staging_root = PathBuf::from(&transaction.source_scan_root);
        let journal_dir = match default_rename_transaction_dir() {
            Ok(path) => path,
            Err(error) => {
                self.mame_message = Some(format!("MAME recovery journal is unavailable: {error}"));
                return;
            }
        };
        let cancel = AtomicBool::new(false);
        match rollback_transaction_confined(
            &mut transaction,
            &journal_dir,
            &cancel,
            &TrustedRoots::from_paths([staging_root.as_path(), &destination_parent]),
        ) {
            Ok(outcome) => {
                self.mame_history[index] = outcome.transaction;
                self.mame_confirmation.clear();
                self.mame_undo_pending = None;
                self.mame_message = Some(
                    "MAME reconstruction was rolled back safely; the source archives were not touched.".into(),
                );
            }
            Err(error) => {
                self.mame_history = load_mame_history();
                self.mame_message = Some(format!(
                    "MAME undo stopped safely and needs review: {error}"
                ));
            }
        }
    }
}

fn refresh_mame_evidence(state: &mut OrganisationState, requested_set: Option<String>) {
    let (Some(root), Some(dat_path)) = (&state.mame_root, &state.mame_dat) else {
        state.mame_message = Some("Choose both the MAME folder and its verified DAT first.".into());
        return;
    };
    match load_verified_mame_0174(dat_path) {
        Ok(dat) => match default_database_path().and_then(|path| Database::open_or_create(&path)) {
            Ok(mut database) => match refresh_mame_member_evidence(
                &mut database,
                &dat,
                root,
                requested_set.as_deref(),
            ) {
                Ok(report) => {
                    state.mame_message = Some(format!(
                        "Evidence refresh complete: {} sets, {} members; ROM files were not changed.",
                        report.sets_published, report.members_seen
                    ))
                }
                Err(error) => {
                    state.mame_message = Some(format!("Evidence refresh stopped safely: {error}"))
                }
            },
            Err(error) => {
                state.mame_message = Some(format!("Could not open the evidence database: {error}"))
            }
        },
        Err(error) => {
            state.mame_message = Some(format!(
                "The selected DAT is not the verified MAME 0.174 Arcade DAT: {error}"
            ))
        }
    }
}

/// A destination's short heading identity (icon plate + accent-coloured
/// label) reused above the delegated preview/apply views so a returning
/// user can tell at a glance which target they are inside without the
/// wording changing behaviour in any way.
fn destination_kind(destination: PlayingLibraryDestination) -> CardKind {
    match destination {
        PlayingLibraryDestination::Generic => CardKind::PlayingLibrary,
        PlayingLibraryDestination::Romm => CardKind::RomM,
        PlayingLibraryDestination::EsDe => CardKind::EsDe,
        PlayingLibraryDestination::RetroDeck => CardKind::RetroDeck,
    }
}

fn destination_label(destination: PlayingLibraryDestination) -> &'static str {
    match destination {
        PlayingLibraryDestination::Generic => "Playing Library",
        PlayingLibraryDestination::Romm => "RomM",
        PlayingLibraryDestination::EsDe => "ES-DE",
        PlayingLibraryDestination::RetroDeck => "RetroDECK",
    }
}

/// A compact accent header for a delegated sub-view: an icon plate plus the
/// destination's name in its own accent colour. Presentation only - it
/// carries no state and changes no routing.
fn sub_view_heading(ui: &mut egui::Ui, kind: CardKind, label: &str) {
    ui.horizontal(|ui| {
        motif_plate(ui, 30.0, kind);
        ui.label(
            RichText::new(label)
                .strong()
                .size(theme::SECTION_TITLE_SIZE)
                .color(kind.accent()),
        );
    });
}

impl App {
    pub(super) fn organisation_page(&mut self, ui: &mut egui::Ui) {
        let mut selected = None;
        let mut back = false;
        let mut canonical_action = None;
        let mut playing_action = None;
        let mut review_pending = false;
        let mut open_history = false;
        let mut open_mame_history = false;

        egui::ScrollArea::vertical()
            .id_salt("v2_organisation")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.organisation.view {
                OrganisationView::Landing => {
                    widgets::page_hero(
                        ui,
                        |ui, size| {
                            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                            CardKind::PlayingLibrary.paint(ui, rect);
                        },
                        "Choose what you want to organise",
                        "Turn a verified collection into something tidy, understandable and ready to use.",
                        Some((
                            "Nothing changes until you preview and confirm",
                            widgets::StatusTone::Info,
                        )),
                        None,
                        |_ui| {},
                        |_ui| {},
                    );
                    if self.playing_library.attention_snapshot().items().next().is_some() {
                        widgets::banner(
                            ui,
                            "An earlier organisation operation needs attention",
                            "Review its saved recovery state before starting another publication.",
                            widgets::StatusTone::Warning,
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Review").clicked() {
                                review_pending = true;
                            }
                            if ui.button("History").clicked() {
                                open_history = true;
                            }
                        });
                        ui.add_space(theme::SPACE_SM);
                    }
                    if !self.organisation.mame_history.is_empty() {
                        widgets::banner(
                            ui,
                            "MAME reconstruction history is available",
                            "Review publication and recovery state before starting another reconstruction.",
                            widgets::StatusTone::Info,
                        );
                        if ui.button("Review MAME history").clicked() {
                            open_mame_history = true;
                        }
                        ui.add_space(theme::SPACE_SM);
                    }
                    if ui.button(RichText::new("Fix my MAME library").strong().color(theme::TEAL)).clicked() {
                        self.organisation.view = OrganisationView::MameNormalizer;
                    }
                    ui.label("Source untouched until a reviewed preview is confirmed.");
                    ui.label("Build a clean playing library");
                    for card in ACTIONS {
                        widgets::workflow_card(ui, card.kind.accent(), |ui| {
                            ui.horizontal(|ui| {
                                motif_plate(ui, 56.0, card.kind);
                                ui.add_space(theme::SPACE_SM);
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(card.title)
                                            .size(theme::SECTION_TITLE_SIZE)
                                            .strong(),
                                    );
                                    ui.label(card.description);
                                    ui.label(RichText::new(card.semantics).strong().color(card.kind.accent()));
                                    ui.label(
                                        RichText::new("Ready when its required source, destination and verification data are available.")
                                            .color(theme::muted(ui)),
                                    );
                                    ui.add_space(theme::SPACE_XS);
                                    if primary(ui, card.title) {
                                        selected = Some(card.destination);
                                    }
                                });
                            });
                        });
                        ui.add_space(theme::SPACE_SM);
                    }
                    widgets::workflow_card(ui, theme::TEAL, |ui| {
                        ui.horizontal(|ui| {
                            motif_plate(ui, 56.0, CardKind::VerifiedGames);
                            ui.add_space(theme::SPACE_SM);
                            ui.vertical(|ui| {
                                ui.label(RichText::new("Fix my MAME library").size(theme::SECTION_TITLE_SIZE).strong());
                                ui.label("Use verified MAME evidence to review safe set repairs.");
                                ui.label(RichText::new("Preview required · Evidence must be sufficient · Recovery remains available").strong().color(theme::TEAL));
                            });
                        });
                    });
                    ui.add_space(theme::SPACE_SM);
                    ui.collapsing("Advanced organisation options", |ui| {
                        ui.label("These options stay inside the native v2 workflow; the old Build window is not used.");
                        ui.label("Required normal option: choose a destination, then preview and explicitly confirm the safe transaction.");
                        ui.label("Advanced but supported: adjust 1G1R region, language, revision and Beta/Proto/Demo/Sample preferences inside Playing Library.");
                        if ui.button("Open 1G1R preferences").clicked() {
                            selected = Some(Some(PlayingLibraryDestination::Generic));
                        }
                        ui.label("RomM, ES-DE and RetroDECK each retain their own visibility/publication checks and recovery actions.");
                        ui.label("Specialist/deferred: obsolete duplicate controls from the former Build shell are not reproduced here.");
                        if ui.button("Review organisation history").clicked() {
                            open_history = true;
                        }
                    });
                }
                OrganisationView::VerifiedGames => {
                    self.invalidate_changed_canonical_organisation_plan();
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Organisation").clicked() {
                            back = true;
                        }
                        sub_view_heading(ui, CardKind::VerifiedGames, "Organise verified games");
                    });
                    ui.label("1 Choose  ·  2 Preview  ·  3 Confirm  ·  4 Apply");
                    ui.label("MOVE changes the original file's folder. RENAME changes its name in place. LINK leaves the original untouched and creates a shortcut in a new library.");
                    canonical_action = rom_organisation_page::show_rom_organisation_page_with_busy(
                        ui,
                        &mut self.canonical_organisation,
                        self.canonical_organisation_job.is_some(),
                        true,
                    );
                }
                OrganisationView::PlayingLibrary => {
                    self.invalidate_changed_playing_library_plan();
                    let kind = destination_kind(self.playing_library.destination);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Organisation").clicked() {
                            back = true;
                        }
                        sub_view_heading(
                            ui,
                            kind,
                            destination_label(self.playing_library.destination),
                        );
                    });
                    ui.label("1 Choose  ·  2 Preview  ·  3 Confirm  ·  4 Apply");
                    ui.label("Original files stay untouched. EmuWiz creates reviewed links in the destination, then publishes metadata separately when required.");
                    if self.playing_library.destination == PlayingLibraryDestination::Romm {
                        ui.label("This creates files for RomM to scan. It does not edit your RomM server.");
                    }
                    if self.playing_library.destination == PlayingLibraryDestination::RetroDeck {
                        ui.label("RetroDECK uses ES-DE internally and also needs both linked paths to be visible inside its sandbox.");
                    }
                    playing_action = crate::playing_library_page::show_playing_library_page_with_busy(
                        ui,
                        &mut self.playing_library,
                        self.playing_library_job.is_some(),
                    );
                }
                OrganisationView::MameNormalizer => {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Organisation").clicked() {
                            back = true;
                        }
                        ui.heading("Fix my MAME library");
                    });
                    show_mame_normalizer(ui, &mut self.organisation);
                }
            });

        if back {
            self.organisation.view = OrganisationView::Landing;
        }
        if let Some(destination) = selected {
            match destination {
                None => self.organisation.view = OrganisationView::VerifiedGames,
                Some(destination) => {
                    self.playing_library.set_destination(destination);
                    self.organisation.view = OrganisationView::PlayingLibrary;
                }
            }
        }
        if review_pending {
            self.organisation.view = OrganisationView::PlayingLibrary;
        }
        if open_history {
            self.go(Route::Section(Section::History));
        }
        if open_mame_history {
            self.organisation.view = OrganisationView::MameNormalizer;
        }
        if let Some(action) = canonical_action {
            match action {
                RomOrganisationPageAction::Preview => self
                    .start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Preview),
                RomOrganisationPageAction::Apply => self
                    .start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Apply),
                RomOrganisationPageAction::Rollback => self.start_canonical_organisation_job(
                    super::CanonicalOrganisationJobKind::Rollback,
                ),
            }
        }
        if let Some(action) = playing_action {
            self.handle_playing_library_action(action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_has_exactly_the_five_backed_normal_actions() {
        assert_eq!(ACTIONS.len(), 5);
        assert_eq!(ACTIONS[0].title, "Organise verified games");
        assert_eq!(
            ACTIONS[1].destination,
            Some(PlayingLibraryDestination::Generic)
        );
        assert_eq!(
            ACTIONS[2].destination,
            Some(PlayingLibraryDestination::Romm)
        );
        assert_eq!(
            ACTIONS[3].destination,
            Some(PlayingLibraryDestination::EsDe)
        );
        assert_eq!(
            ACTIONS[4].destination,
            Some(PlayingLibraryDestination::RetroDeck)
        );
        assert!(ACTIONS.iter().all(|card| card.semantics.contains("Preview")
            || card.semantics.contains("required")
            || card.semantics.contains("separately")));
    }

    #[test]
    fn each_action_card_carries_the_expected_visual_identity() {
        assert_eq!(ACTIONS[0].kind, CardKind::VerifiedGames);
        assert_eq!(ACTIONS[1].kind, CardKind::PlayingLibrary);
        assert_eq!(ACTIONS[2].kind, CardKind::RomM);
        assert_eq!(ACTIONS[3].kind, CardKind::EsDe);
        assert_eq!(ACTIONS[4].kind, CardKind::RetroDeck);
    }

    /// Playing Library, RomM, ES-DE and RetroDECK must each read as visually
    /// distinct at a glance (no two organisation targets sharing an accent
    /// colour), while the whole set still sits inside the app's blue theme
    /// family rather than introducing an unrelated palette.
    #[test]
    fn organisation_target_accents_are_all_distinct() {
        let accents = [
            CardKind::VerifiedGames.accent(),
            CardKind::PlayingLibrary.accent(),
            CardKind::RomM.accent(),
            CardKind::EsDe.accent(),
            CardKind::RetroDeck.accent(),
        ];
        for (i, a) in accents.iter().enumerate() {
            for (j, b) in accents.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "kinds {i} and {j} share an accent colour");
                }
            }
        }
    }

    #[test]
    fn destination_kind_and_label_cover_every_playing_library_destination() {
        assert_eq!(
            destination_kind(PlayingLibraryDestination::Generic),
            CardKind::PlayingLibrary
        );
        assert_eq!(
            destination_kind(PlayingLibraryDestination::Romm),
            CardKind::RomM
        );
        assert_eq!(
            destination_kind(PlayingLibraryDestination::EsDe),
            CardKind::EsDe
        );
        assert_eq!(
            destination_kind(PlayingLibraryDestination::RetroDeck),
            CardKind::RetroDeck
        );
        assert_eq!(
            destination_label(PlayingLibraryDestination::Generic),
            "Playing Library"
        );
        assert_eq!(destination_label(PlayingLibraryDestination::Romm), "RomM");
        assert_eq!(destination_label(PlayingLibraryDestination::EsDe), "ES-DE");
        assert_eq!(
            destination_label(PlayingLibraryDestination::RetroDeck),
            "RetroDECK"
        );
    }

    /// Every motif must paint without panicking even in a tiny allocation -
    /// the landing page's cards must still render sensibly at narrow widths
    /// (1280x720) where the motif plate is the smallest.
    #[test]
    fn every_motif_paints_without_panicking_at_a_small_size() {
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                for kind in [
                    CardKind::VerifiedGames,
                    CardKind::PlayingLibrary,
                    CardKind::RomM,
                    CardKind::EsDe,
                    CardKind::RetroDeck,
                ] {
                    motif_plate(ui, 18.0, kind);
                }
            });
        });
    }

    fn mame_plan(ready: bool) -> MameMergedReconstructionPlan {
        MameMergedReconstructionPlan {
            dat_version: "fixture".into(),
            dat_sha256: "fixture".into(),
            parent: "pacman".into(),
            clones: vec!["puckman".into()],
            destination: PathBuf::from("/mame/pacman.zip"),
            required_members: Vec::new(),
            sources: Vec::new(),
            missing_members: if ready {
                Vec::new()
            } else {
                vec!["missing.bin".into()]
            },
            duplicate_candidates: Vec::new(),
            hash_mismatches: Vec::new(),
            unresolved_ownership: Vec::new(),
            collisions: Vec::new(),
            ready_to_apply: ready,
            reasons: if ready {
                Vec::new()
            } else {
                vec!["required ROM members are missing".into()]
            },
        }
    }

    fn rendered_mame_text(state: &mut OrganisationState) -> Vec<String> {
        let context = egui::Context::default();
        context
            .run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    show_mame_normalizer(ui, state);
                });
            })
            .shapes
            .iter()
            .flat_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => vec![text.galley.text().to_string()],
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn mame_ready_preview_exposes_publish_but_blocked_preview_does_not() {
        let mut ready = OrganisationState {
            mame_plan: Some(mame_plan(true)),
            ..OrganisationState::default()
        };
        let ready_text = rendered_mame_text(&mut ready).join("\n");
        assert!(ready_text.contains("Publish reconstructed merged output"));

        let mut blocked = OrganisationState {
            mame_plan: Some(mame_plan(false)),
            ..OrganisationState::default()
        };
        let blocked_text = rendered_mame_text(&mut blocked).join("\n");
        assert!(blocked_text.contains("Publish blocked"));
        assert!(!blocked_text.contains("Publish reconstructed merged output"));
    }

    #[test]
    fn mame_publish_confirmation_is_deterministic_and_not_a_yes_no_prompt() {
        assert_eq!(
            mame_publish_confirmation_phrase(1),
            "PUBLISH MAME 1 OUTPUTS"
        );
        assert_eq!(
            mame_publish_confirmation_phrase(3),
            "PUBLISH MAME 3 OUTPUTS"
        );
    }
}
