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

use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::mame_arcade_join::{
    load_verified_mame_0174, refresh_mame_member_evidence,
};
use archivefs_core::dat::mame_normalizer::{
    MameCollectionMode, MameNormalisationPlan, detect_mame_collection_mode,
    plan_mame_normalisation, plan_mame_normalisation_from_verified_joins,
};
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::{Database, default_database_path};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum OrganisationView {
    #[default]
    Landing,
    VerifiedGames,
    PlayingLibrary,
    MameNormalizer,
}

#[derive(Default)]
pub(super) struct OrganisationState {
    pub(super) view: OrganisationView,
    pub(super) mame_root: Option<PathBuf>,
    pub(super) mame_dat: Option<PathBuf>,
    pub(super) mame_plan: Option<MameNormalisationPlan>,
    pub(super) mame_mode: MameCollectionMode,
    pub(super) mame_detected_mode: Option<MameCollectionMode>,
    pub(super) mame_message: Option<String>,
    pub(super) mame_evidence_set: String,
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
    ui.label("Wizzy only changes files when the selected DAT proves their identity by checksum.");
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
    ui.horizontal(|ui| {
        ui.label("Collection layout:");
        for (mode, label) in [
            (MameCollectionMode::Merged, "Merged"),
            (MameCollectionMode::Split, "Split"),
            (MameCollectionMode::NonMerged, "Non-merged"),
            (MameCollectionMode::NotSure, "Not sure"),
        ] {
            ui.radio_value(&mut state.mame_mode, mode, label);
        }
    });
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
    if ui.button("Preview fixes").clicked() {
        match (&state.mame_root, &state.mame_dat) {
            (Some(root), Some(dat_path)) => match parse_dat_file(dat_path, DatLimits::default()) {
                Ok(outcome) if state.mame_mode == MameCollectionMode::NotSure => {
                    state.mame_detected_mode = detect_mame_collection_mode(root, &outcome.dat).ok();
                    state.mame_message =
                        Some("Confirm the detected layout, then preview fixes again.".into());
                }
                Ok(outcome) => {
                    match verified_mame_plan(root, dat_path, &outcome.dat, state.mame_mode) {
                        Ok(plan) => state.mame_plan = Some(plan),
                        Err(error) => state.mame_message = Some(error),
                    }
                }
                Err(error) => {
                    state.mame_message = Some(format!("The DAT could not be read: {error}"))
                }
            },
            _ => state.mame_message = Some("Choose both the MAME folder and its DAT first.".into()),
        }
    }
    if let Some(message) = &state.mame_message {
        ui.label(message);
    }
    if let Some(plan) = &state.mame_plan {
        let summary = &plan.summary;
        ui.label(format!(
            "{} sets checked · {} safe · {} need attention",
            summary.total_sets,
            summary.safe,
            summary.needs_attention + summary.collisions + summary.missing_data + summary.unknown
        ));
        ui.horizontal(|ui| {
            if ui.button("Apply verified repairs").clicked()
                && let Some(root) = &state.mame_root
            {
                let journal = root.join(".emuwiz-mame-normaliser.json");
                state.mame_message =
                    match archivefs_core::dat::mame_normalizer::apply_mame_normalisation(
                        plan, &journal,
                    ) {
                        Ok(count) => Some(format!("Applied {count} safe set repairs.")),
                        Err(error) => Some(format!("Repair stopped safely: {error}")),
                    };
            }
            if ui.button("Undo this repair batch").clicked()
                && let Some(root) = &state.mame_root
            {
                let journal = root.join(".emuwiz-mame-normaliser.json");
                state.mame_message =
                    match archivefs_core::dat::mame_normalizer::undo_mame_normalisation(&journal) {
                        Ok(count) => Some(format!("Undid {count} safe set repairs.")),
                        Err(error) => Some(format!("Undo stopped safely: {error}")),
                    };
            }
        });
    }
}

fn verified_mame_plan(
    root: &std::path::Path,
    dat_path: &std::path::Path,
    dat: &archivefs_core::dat::model::ParsedDat,
    mode: MameCollectionMode,
) -> Result<MameNormalisationPlan, String> {
    let digest = Sha256::digest(std::fs::read(dat_path).map_err(|e| e.to_string())?);
    let dat_sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if let Ok(path) = default_database_path()
        && let Ok(database) = Database::open_read_only(&path)
        && let Ok(joins) = database.mame_arcade_join_paths_for_dat(&dat_sha256)
        && !joins.is_empty()
    {
        return plan_mame_normalisation_from_verified_joins(root, dat, &joins, mode);
    }
    plan_mame_normalisation(root, dat, mode)
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
        let mut advanced = false;
        let mut back = false;
        let mut canonical_action = None;
        let mut playing_action = None;
        let mut review_pending = false;
        let mut open_history = false;

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
                    if ui.button("Advanced organisation tools").clicked() {
                        advanced = true;
                    }
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
        if advanced {
            self.legacy(Section::Build);
        }
        if review_pending {
            self.organisation.view = OrganisationView::PlayingLibrary;
        }
        if open_history {
            self.go(Route::Section(Section::History));
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
}
