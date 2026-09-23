//! Native Organisation landing and flow coordinator.
//!
//! This module owns presentation state only. Canonical organisation delegates
//! to `rom_organisation_page`; Playing Library, RomM, ES-DE and RetroDECK all
//! delegate to `playing_library_page` and their existing core planners.

use super::{
    App,
    routes::{Route, Section},
};
use crate::ui::theme;
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
    MameCollectionMode, MameFixStatus, MameNormalisationPlan, detect_mame_collection_mode,
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

#[derive(Clone, Copy)]
struct ActionCard {
    title: &'static str,
    description: &'static str,
    semantics: &'static str,
    destination: Option<PlayingLibraryDestination>,
}

const ACTIONS: [ActionCard; 5] = [
    ActionCard {
        title: "Organise verified games",
        description: "Rename or arrange verified files into clean platform folders.",
        semantics: "Moves or renames original files · Preview required · Undo available",
        destination: None,
    },
    ActionCard {
        title: "Build a clean playing library",
        description: "Create a tidy linked library without moving your original games.",
        semantics: "Source untouched · Creates links · Preview required · Undo available",
        destination: Some(PlayingLibraryDestination::Generic),
    },
    ActionCard {
        title: "Organise for RomM",
        description: "Create a RomM-ready library using reviewed platform folders and safe links.",
        semantics: "Source untouched · Creates links · Visibility check required",
        destination: Some(PlayingLibraryDestination::Romm),
    },
    ActionCard {
        title: "Export to ES-DE",
        description: "Build a clean linked library, then update gamelist.xml safely.",
        semantics: "Source untouched · Creates links · Metadata updated separately",
        destination: Some(PlayingLibraryDestination::EsDe),
    },
    ActionCard {
        title: "Prepare for RetroDECK",
        description: "Create the linked layout RetroDECK expects and publish its ES-DE metadata.",
        semantics: "Source untouched · Creates links · Sandbox visibility required",
        destination: Some(PlayingLibraryDestination::RetroDeck),
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
                    ui.heading("Choose what you want to organise");
                    ui.label("Choose how you want EmuWiz to arrange or publish your verified games. Nothing changes until you preview and confirm.");
                    ui.label("Every option explains whether original files move, links are created, or only frontend metadata changes.");
                    if self.playing_library.attention_snapshot().items().next().is_some() {
                        crate::ui::components::banner(
                            ui,
                            "An earlier organisation operation needs attention",
                            "Review its saved recovery state before starting another publication.",
                            crate::ui::components::StatusTone::Warning,
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Review").clicked() {
                                review_pending = true;
                            }
                            if ui.button("History").clicked() {
                                open_history = true;
                            }
                        });
                    }
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.heading("Fix my MAME library");
                        ui.label("Use verified DAT checksums to repair MAME set and ROM member names safely.");
                        ui.label(RichText::new("Preview first · No fuzzy matching · Undo available").strong());
                        if primary(ui, "Fix my MAME library") {
                            self.organisation.view = OrganisationView::MameNormalizer;
                        }
                    });
                    ui.add_space(8.0);
                    for card in ACTIONS {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.heading(card.title);
                            ui.label(card.description);
                            ui.label(RichText::new(card.semantics).strong());
                            ui.label("Ready when its required source, destination and verification data are available.");
                            if primary(ui, card.title) {
                                selected = Some(card.destination);
                            }
                        });
                        ui.add_space(8.0);
                    }
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
                        ui.strong("1 Choose  ·  2 Preview  ·  3 Confirm  ·  4 Apply");
                    });
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
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("← Organisation").clicked() {
                            back = true;
                        }
                        ui.strong("1 Choose  ·  2 Preview  ·  3 Confirm  ·  4 Apply");
                    });
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
                    show_mame_normalizer(ui, &mut self.organisation);
                    if ui.button("← Organisation").clicked() { back = true; }
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

fn show_mame_normalizer(ui: &mut egui::Ui, state: &mut OrganisationState) {
    ui.heading("Fix my MAME library");
    ui.label("Wizzy only changes files when the selected DAT proves their identity by checksum.");
    ui.horizontal_wrapped(|ui| {
        if ui.button("Choose MAME folder…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Choose MAME folder")
                .pick_folder()
        {
            state.mame_root = Some(path);
            state.mame_plan = None;
            state.mame_detected_mode = None;
            state.mame_message = None;
        }
        if ui.button("Choose DAT…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("DAT files", &["dat", "xml"])
                .pick_file()
        {
            state.mame_dat = Some(path);
            state.mame_plan = None;
            state.mame_detected_mode = None;
            state.mame_message = None;
        }
    });
    ui.label(format!(
        "MAME folder: {}",
        state
            .mame_root
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not selected".into())
    ));
    ui.label(format!(
        "DAT: {}",
        state
            .mame_dat
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not selected".into())
    ));
    ui.horizontal(|ui| {
        ui.label("How is your collection organised?");
        let previous_mode = state.mame_mode;
        for (mode, label) in [
            (MameCollectionMode::Merged, "Merged"),
            (MameCollectionMode::Split, "Split"),
            (MameCollectionMode::NonMerged, "Non-merged"),
            (MameCollectionMode::NotSure, "Not sure"),
        ] {
            ui.radio_value(&mut state.mame_mode, mode, label);
        }
        if state.mame_mode != previous_mode {
            state.mame_detected_mode = None;
            state.mame_plan = None;
            state.mame_message = None;
        }
    });
    ui.label(match state.mame_mode {
        MameCollectionMode::Merged => {
            "Merged: parent archives may also contain clone-specific files."
        }
        MameCollectionMode::Split => {
            "Split: clones keep their own files and may use shared parent files."
        }
        MameCollectionMode::NonMerged => {
            "Non-merged: every set archive contains all files it needs."
        }
        MameCollectionMode::NotSure => {
            "Not sure: EmuWiz can inspect a small sample and suggest a layout."
        }
    });
    ui.separator();
    ui.heading("Refresh physical member evidence");
    ui.label("Read-only: hashes extracted directory members and stores reusable, versioned location evidence. It never changes ROM files.");
    ui.horizontal(|ui| {
        ui.label("Optional set/family:");
        ui.text_edit_singleline(&mut state.mame_evidence_set);
    });
    ui.horizontal_wrapped(|ui| {
        let family = state.mame_evidence_set.trim().to_string();
        if ui.button("Refresh selected family").clicked() {
            if family.is_empty() {
                state.mame_message = Some("Enter a MAME set name first; this action scans only that parent/clone family.".into());
            } else {
                refresh_mame_evidence(state, Some(family));
            }
        }
        if ui.button("Refresh selected folder").clicked() {
            refresh_mame_evidence(state, None);
        }
        if ui.button("Refresh whole Arcade library").clicked() {
            state.mame_message = Some("This may inspect the full configured Arcade source. Confirm the selected folder and start deliberately.".into());
            refresh_mame_evidence(state, None);
        }
    });
    if ui.button("Preview fixes").clicked() {
        match (&state.mame_root, &state.mame_dat) {
            (Some(root), Some(dat_path)) => match parse_dat_file(dat_path, DatLimits::default()) {
                Ok(outcome) if state.mame_mode == MameCollectionMode::NotSure => {
                    match detect_mame_collection_mode(root, &outcome.dat) {
                        Ok(detected) => {
                            state.mame_detected_mode = Some(detected);
                            state.mame_message = Some(format!(
                                "Your collection looks like {detected:?}. Confirm that layout above, then preview fixes again."
                            ));
                        }
                        Err(error) => {
                            state.mame_message = Some(format!(
                                "The collection layout could not be sampled safely: {error}"
                            ))
                        }
                    }
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
        ui.colored_label(egui::Color32::LIGHT_RED, message);
    }
    if let Some(plan) = &state.mame_plan {
        let s = &plan.summary;
        ui.separator();
        ui.label(format!("{} sets checked · {} can be fixed automatically · {} already correct · {} need attention", s.total_sets, s.safe, s.already_correct, s.needs_attention + s.collisions + s.missing_data + s.unknown));
        if s.split_rebuilds > 0 {
            ui.label(format!(
                "{} Split parent/clone families will rebuild extracted directories safely · {} verified members will be placed",
                s.split_rebuilds, s.moved_members
            ));
        }
        for set in plan
            .sets
            .iter()
            .filter(|set| set.status != MameFixStatus::AlreadyCorrect)
            .take(12)
        {
            ui.label(format!(
                "{} → {} ({:?})",
                set.current_path.display(),
                set.correct_path.display(),
                set.status
            ));
            for member in &set.members {
                ui.label(format!("  {} → {}", member.current, member.correct));
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Rebuild verified directories / Fix library").clicked()
                && let Some(root) = &state.mame_root
            {
                let family = state.mame_evidence_set.trim();
                if family.is_empty() {
                    state.mame_message = Some("Enter one parent/clone family before applying. Directory Split apply is intentionally family-scoped; preview remains read-only until then.".into());
                    return;
                }
                let Some(family_plan) = family_plan(plan, family) else {
                    state.mame_message = Some(format!("No verified Split rebuild plan was found for family {family}. Nothing was changed."));
                    return;
                };
                let journal = root.join(".emuwiz-mame-normaliser.json");
                match archivefs_core::dat::mame_normalizer::apply_mame_normalisation(&family_plan, &journal) {
                    Ok(count) => state.mame_message = Some(format!("Applied {count} safe set repairs. Verification is complete for staged archives.")),
                    Err(error) => state.mame_message = Some(format!("Repair stopped safely: {error}")),
                }
            }
            if ui.button("Undo this repair batch").clicked()
                && let Some(root) = &state.mame_root
            {
                let journal = root.join(".emuwiz-mame-normaliser.json");
                match archivefs_core::dat::mame_normalizer::undo_mame_normalisation(&journal) {
                    Ok(count) => state.mame_message = Some(format!("Undid {count} safe set repairs.")),
                    Err(error) => state.mame_message = Some(format!("Undo stopped safely: {error}")),
                }
            }
        });
    }
}

fn family_plan(plan: &MameNormalisationPlan, family: &str) -> Option<MameNormalisationPlan> {
    let paths = plan
        .split_rebuilds
        .iter()
        .filter(|rebuild| {
            [
                &rebuild.parent_path,
                &rebuild.clone_path,
                &rebuild.parent_target,
                &rebuild.clone_target,
            ]
            .into_iter()
            .any(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy() == family)
            })
        })
        .flat_map(|rebuild| [rebuild.parent_path.clone(), rebuild.clone_path.clone()])
        .collect::<std::collections::BTreeSet<_>>();
    if paths.is_empty() {
        return None;
    }
    let mut selected = plan.clone();
    selected.split_rebuilds.retain(|rebuild| {
        paths.contains(&rebuild.parent_path) && paths.contains(&rebuild.clone_path)
    });
    selected
        .sets
        .retain(|set| paths.contains(&set.current_path));
    Some(selected)
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
    if let Ok(database_path) = default_database_path()
        && let Ok(database) = Database::open_read_only(&database_path)
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
                    state.mame_plan = None;
                    state.mame_message = Some(format!(
                        "Evidence refresh complete: {} sets, {} members; {} reused, {} rehashed, {} actionable, {} failed. ROM files were not changed.",
                        report.sets_published,
                        report.members_seen,
                        report.members_reused,
                        report.members_rehashed,
                        report.members_actionable,
                        report.members_failed
                    ));
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
    fn mame_layout_starts_unconfirmed() {
        assert_eq!(
            OrganisationState::default().mame_mode,
            MameCollectionMode::NotSure
        );
    }
}
