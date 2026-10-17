//! Native Organisation landing and flow coordinator.
//!
//! This module owns presentation state only. Canonical organisation delegates
//! to `rom_organisation_page`; Playing Library, RomM, ES-DE and RetroDECK all
//! delegate to `playing_library_page` and their existing core planners.

mod repair;
mod frontend_export;

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

use archivefs_core::dat::rom_organisation::OrganisationMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum OrganisationView {
    #[default]
    Landing,
    VerifiedGames,
    PlayingLibrary,
    MameNormalizer,
    MameHealth,
    RepairReview,
    RepairHistory,
    FrontendExport,
}

pub(super) struct OrganisationState {
    pub(super) r2: super::r2_rename::State,
    pub(super) view: OrganisationView,
    pub(super) frontend_export: crate::publisher_profile_page::PublisherProfilePageState,
    /// MAME merged reconstruction: preview, publish and Undo run as
    /// background jobs owned by the panel.
    pub(super) mame_reconstruction: super::mame_reconstruction::MameReconstructionPanel,
    pub(super) mame_health: super::mame_collection_health::HealthPage,
}

impl Default for OrganisationState {
    fn default() -> Self {
        Self {
            view: OrganisationView::Landing,
            r2: Default::default(),
            frontend_export: Default::default(),
            mame_reconstruction: Default::default(),
            mame_health: Default::default(),
        }
    }
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

/// Below this window height (in points) the landing page drops its hero so the
/// primary action stays on screen.
const COMPACT_HERO_VIEWPORT_HEIGHT: f32 = 580.0;

const ACTIONS: [ActionCard; 5] = [
    ActionCard {
        title: "Rename verified games",
        description: "Rename files to trusted DAT names after reviewing the preview.",
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
            .fill(theme::primary_action_fill(ui))
            .min_size(egui::vec2(180.0, 44.0)),
    )
    .clicked()
}

/// Small square motif plate, matching the plate treatment already used for
/// platform tiles on Home/Platforms (`gui_v2::visual_pages`): a dark backing
/// square with the motif painted inside it.
fn motif_plate(ui: &mut egui::Ui, side: f32, kind: CardKind) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    ui.painter().rect_filled(rect, 8.0, theme::deep_fill(ui));
    kind.paint(ui, rect.shrink(side * 0.12));
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

/// Normal-mode review destinations use the existing canonical routes.
const REVIEW_TASKS: [(&str, &str, &str, Section); 3] = [
    (
        "Duplicates",
        "Review matching contents and the copy to keep. Detection never deletes files automatically.",
        "Review duplicates",
        Section::Duplicates,
    ),
    (
        "Collection Review",
        "Review uncertain identities, conflicts and problems before choosing an action.",
        "Review collection",
        Section::Problems,
    ),
    (
        "MAME organisation",
        "Use the MAME workflow to check sets and shared parent files. Generic file organisation cannot replace MAME set checks.",
        "Open MAME",
        Section::Mame,
    ),
];

impl App {
    pub(super) fn organisation_page(&mut self, ui: &mut egui::Ui) {
        if self.organisation.view == OrganisationView::MameHealth {
            if ui.button("← Organisation").clicked() {
                self.organisation.view = OrganisationView::Landing;
            }
            super::mame_collection_health::show_read_only(ui, &mut self.organisation.mame_health);
            return;
        }
        let mut selected = None;
        let mut back = false;
        let mut canonical_action = None;
        let mut playing_action = None;
        let mut review_pending = false;
        let mut open_history = false;
        let mut open_mame_history = false;

        egui::ScrollArea::vertical()
            .id_salt(match self.organisation.view {
                OrganisationView::RepairReview => "v2_repair_review",
                OrganisationView::RepairHistory => "v2_repair_history",
                _ => "v2_organisation",
            })
            .auto_shrink([false, false])
            .show(ui, |ui| match self.organisation.view {
                OrganisationView::RepairReview | OrganisationView::RepairHistory => {
                    let history = self.organisation.view == OrganisationView::RepairHistory;
                    repair::show(self, ui, history);
                }
                OrganisationView::FrontendExport => frontend_export::show(self, ui),
                OrganisationView::Landing => {
                    let compact = ui.ctx().content_rect().height() < COMPACT_HERO_VIEWPORT_HEIGHT;
                    // On a short window (for example 1024x600 at 160% scaling is
                    // 640x375 points) the hero would push the primary action
                    // below the fold, where it cannot be focused. Keep the page
                    // title and the safety statement, drop the illustration.
                    if compact {
                        widgets::status_badge(
                            ui,
                            "Nothing changes until you preview and confirm",
                            widgets::StatusTone::Info,
                        );
                        ui.add_space(theme::SPACE_SM);
                    } else {
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
                    }
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
                    if !self.organisation.mame_reconstruction.history.is_empty() {
                        widgets::banner(
                            ui,
                            "MAME rebuild history is available",
                            "Review publication and recovery state before starting another reconstruction.",
                            widgets::StatusTone::Info,
                        );
                        if ui.button("Review MAME history").clicked() {
                            open_mame_history = true;
                        }
                        ui.add_space(theme::SPACE_SM);
                    }
                    if !compact {
                        ui.label("Review your collection, understand the recommendation, then preview before applying.");
                    }
                    widgets::workflow_card(ui, theme::accent(ui), |ui| {
                        if !compact {
                            ui.heading("Playing Library");
                        }
                        ui.horizontal_wrapped(|ui| {
                            if primary(ui, "Preview Playing Library") {
                                selected = Some(Some(PlayingLibraryDestination::Generic));
                            }
                            if ui.button("Frontend export profiles").clicked() { self.organisation.view = OrganisationView::FrontendExport; }
                            if ui.button("Repair Review").clicked() { self.organisation.view = OrganisationView::RepairReview; }
                            if ui.button("Repair History").clicked() { self.organisation.view = OrganisationView::RepairHistory; }
                        });
                        ui.label("A clean play-focused view made from your existing verified games.");
                        ui.label("Reads originals · Creates separate linked output · Originals preserved · Preview required · Undo available for supported operations");
                    });
                    ui.add_space(theme::SPACE_SM);
                    for (title, explanation, button, section) in REVIEW_TASKS {
                        ui.push_id(("organisation-task", section), |ui| {
                            widgets::card(ui, |ui| {
                                ui.heading(title);
                                ui.label(explanation);
                                if ui.button(button).clicked() {
                                    self.go(Route::Section(section));
                                }
                            });
                        });
                    }
                    ui.collapsing("Other output destinations and verified-file naming", |ui| {
                        for card in ACTIONS {
                            ui.push_id(card.title, |ui| {
                                widgets::workflow_card(ui, card.kind.accent(), |ui| {
                                    ui.horizontal(|ui| {
                                        motif_plate(ui, 40.0, card.kind);
                                        ui.vertical(|ui| {
                                            ui.strong(card.title);
                                            ui.label(card.description);
                                            ui.label(card.semantics);
                                            if ui.button(card.title).clicked() {
                                                selected = Some(card.destination);
                                            }
                                        });
                                    });
                                });
                            });
                        }
                    });
                    ui.add_space(theme::SPACE_SM);
                    // Read-only inspection: kept below the primary action and
                    // the destinations so it never pushes them off-screen.
                    widgets::workflow_card(ui, theme::accent(ui), |ui| {
                        ui.heading("MAME Collection Health");
                        ui.label("Read-only arcade set inspection: ROM checksums, shared support files and preservation gaps. Storage completeness does not prove game runnability.");
                        if ui.button("Check MAME collection health").clicked() {
                            self.organisation.view = OrganisationView::MameHealth;
                        }
                    });
                    ui.add_space(theme::SPACE_SM);
                    ui.collapsing("Advanced organisation options", |ui| {
                        ui.label("Everything here happens in this window.");
                        ui.label("Choose where the result goes, look at the preview, then confirm.");
                        ui.label("Advanced: in Playing Library you can prefer a region, language or version, and choose whether to include betas, prototypes, demos and samples (1G1R = one game, one ROM).");
                        if ui.button("Open 1G1R preferences").clicked() {
                            selected = Some(Some(PlayingLibraryDestination::Generic));
                        }
                        ui.label("RomM, ES-DE and RetroDECK each keep their own checks and undo options.");
                        ui.label("Some old duplicate-handling options from the previous version are not included here.");
                        if ui.button("Review organisation history").clicked() {
                            open_history = true;
                        }
                    });
                }
                OrganisationView::VerifiedGames => {
                    self.organisation_job_card(ui);
                    self.invalidate_changed_canonical_organisation_plan();
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Organisation").clicked() {
                            back = true;
                        }
                        sub_view_heading(ui, CardKind::VerifiedGames, "Rename verified games");
                    });
                    ui.label("1 Choose  ·  2 Preview  ·  3 Confirm  ·  4 Apply");
                    ui.label("MOVE changes the original file's folder. RENAME changes its name in place. LINK leaves the original untouched and creates a shortcut in a new library.");
                    super::r2_rename::choice(ui,&mut self.canonical_organisation.advanced_r2_selected,
                        self.canonical_organisation_job.is_some()||self.organisation.r2.busy(),self.organisation.r2.can_choose_standard());
                    canonical_action = rom_organisation_page::show_rom_organisation_page_with_busy(
                        ui,
                        &mut self.canonical_organisation,
                        self.canonical_organisation_job.is_some() || self.organisation.r2.busy(),
                        true,
                    );
                    if self.canonical_organisation.advanced_r2_selected {
                        self.organisation.r2.show(ui, self.canonical_organisation.r2_selection(), false);
                        if self.organisation.r2.can_choose_standard() && !self.organisation.r2.busy() && ui.button("Use Standard Rename instead").clicked(){ self.canonical_organisation.advanced_r2_selected=false; }
                    }
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
                    ui.label("Original files stay untouched. EmuWiz creates links in the destination you chose, then adds game info separately if needed.");
                    if self.playing_library.destination == PlayingLibraryDestination::Romm {
                        ui.label("This creates files for RomM to scan. It does not edit your RomM server.");
                    }
                    if self.playing_library.destination == PlayingLibraryDestination::RetroDeck {
                        ui.label("RetroDECK runs ES-DE inside a sandbox, so both the original and linked folders must be visible to it.");
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
                    widgets::workflow_card(ui, theme::accent(ui), |ui| {
                        ui.strong("What these MAME words mean");
                        ui.label("Set name: the whole package, for example pacman.zip.");
                        ui.label("ROM file: one file inside a set. A clone game can need files that live in its parent set.");
                        ui.label("Missing file, wrong checksum, duplicate and name clash are different problems and are reported separately.");
                        ui.label("BAD_DUMP means the only known copy is imperfect. NO_DUMP means no good copy is known. Renaming files cannot fix either.");
                        ui.label("Repairing a file inside a ZIP in place is not supported.");
                        ui.label("CHD files can be previewed but not repaired.");
                    });
                    if ui.button("Open read-only MAME Health").clicked() {
                        self.organisation.view = OrganisationView::MameHealth;
                    }
                    ui.label("MAME sets stay in the canonical MAME workflow; generic Playing Library organisation is not offered here.");
                    self.organisation.mame_reconstruction.show(ui);
                }
                OrganisationView::MameHealth => unreachable!("Health renders through its dedicated read-only entry"),
            });

        if back {
            if self.router.current == Route::MameWorkflow {
                self.go(Route::Section(Section::Mame));
            } else {
                self.organisation.view = OrganisationView::Landing;
            }
        }
        if let Some(destination) = selected {
            match destination {
                None => {
                    // The novice "Rename verified games" intent is a real
                    // projection of the existing DAT-backed rename engine,
                    // not a generic move workflow with a friendlier label.
                    self.canonical_organisation
                        .set_mode(OrganisationMode::RenameInPlace);
                    self.organisation.view = OrganisationView::VerifiedGames;
                }
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
                RomOrganisationPageAction::OpenAdvanced => {}
                RomOrganisationPageAction::OpenDat => {}
                RomOrganisationPageAction::OpenCheck => {}
                RomOrganisationPageAction::OpenMame => {}
                RomOrganisationPageAction::OpenHistory => {}
                RomOrganisationPageAction::OpenGames => {}
            }
        }
        if let Some(action) = playing_action {
            self.handle_playing_library_action(action);
        }
    }

    /// The running (or just-finished) organisation/rename job, drawn by the one
    /// shared job card. The job lives in Activity, so it is still here when the
    /// person leaves this page and comes back.
    fn organisation_job_card(&mut self, ui: &mut egui::Ui) {
        let Some(job) = self
            .canonical_organisation_job
            .as_ref()
            .and_then(|job| self.activity.jobs.get(&job.id))
        else {
            return;
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            if super::job_card::show(ui, job, true).cancel {
                job.request_cancel();
            }
        });
    }

    pub(super) fn quick_rename(&mut self, ui: &mut egui::Ui) {
        self.organisation_job_card(ui);
        self.invalidate_changed_canonical_organisation_plan();
        let action = rom_organisation_page::show_quick_rename_page_with_busy(
            ui,
            &mut self.canonical_organisation,
            self.canonical_organisation_job.is_some(),
        );
        if let Some(action) = action {
            match action {
                RomOrganisationPageAction::Preview => self
                    .start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Preview),
                RomOrganisationPageAction::Apply => self
                    .start_canonical_organisation_job(super::CanonicalOrganisationJobKind::Apply),
                RomOrganisationPageAction::Rollback => self.start_canonical_organisation_job(
                    super::CanonicalOrganisationJobKind::Rollback,
                ),
                RomOrganisationPageAction::OpenAdvanced => {
                    self.organisation.view = OrganisationView::VerifiedGames;
                    self.go(Route::Section(Section::Build));
                }
                RomOrganisationPageAction::OpenDat => self.go(Route::Section(Section::Dat)),
                RomOrganisationPageAction::OpenCheck => self.go(Route::Section(Section::Check)),
                RomOrganisationPageAction::OpenMame => {
                    self.organisation.view = OrganisationView::MameNormalizer;
                    self.go(Route::Section(Section::Build));
                }
                RomOrganisationPageAction::OpenHistory => self.go(Route::Section(Section::History)),
                RomOrganisationPageAction::OpenGames => self.go(Route::Section(Section::Games)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organisation_usability_mame_uses_canonical_route_and_warns_about_generic_sets() {
        let task = REVIEW_TASKS
            .iter()
            .find(|task| task.0 == "MAME organisation")
            .unwrap();
        assert_eq!(task.3, Section::Mame);
        assert_eq!(task.2, "Open MAME");
        assert!(
            task.1
                .contains("Generic file organisation cannot replace MAME set checks")
        );
        assert!(!REVIEW_TASKS.iter().any(|task| task.3 == Section::Build));
    }

    #[test]
    fn organisation_usability_review_tasks_reuse_duplicates_and_problem_routes() {
        assert_eq!(REVIEW_TASKS[0].3, Section::Duplicates);
        assert_eq!(REVIEW_TASKS[1].3, Section::Problems);
        assert!(
            REVIEW_TASKS[0]
                .1
                .contains("never deletes files automatically")
        );
    }

    #[test]
    fn landing_has_exactly_the_five_backed_normal_actions() {
        assert_eq!(ACTIONS.len(), 5);
        assert_eq!(ACTIONS[0].title, "Rename verified games");
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
