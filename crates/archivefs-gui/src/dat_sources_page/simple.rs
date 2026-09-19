//! Platform-first presentation over the existing DAT registry and audit worker.
//! No provider activation, rename, or new identity rules live here.
use super::*;
use crate::simple_mode::primary_button;
use archivefs_core::dat::catalogue_selection::{CatalogueVariant, InstalledCatalogueSummary};
use archivefs_core::dat::model::DatEcosystem;

#[derive(Default)]
pub(crate) struct SimpleCheckUi {
    platform: Option<String>,
    query: String,
    setup: bool,
    selected: BTreeMap<String, CatalogueRef>,
    folders: BTreeMap<String, PathBuf>,
    declined: BTreeSet<String>,
    result_for: Option<(String, CatalogueRef, PathBuf)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Ready,
    NeedsSetup,
    NeedsAttention,
    NotSupported,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::NeedsSetup => "Needs setup",
            Self::NeedsAttention => "Needs attention",
            Self::NotSupported => "Not supported",
        }
    }
}

#[derive(Clone, Debug)]
struct Choice {
    reference: CatalogueRef,
    label: String,
    ready: bool,
}

/// Persisted *parsed* ecosystem, never a filename/header-name guess. Require
/// one regular file: a folder can contain mixed arcade/computer catalogues.
pub(super) fn arcade_label(entry: &DatSourceEntry) -> Option<String> {
    if entry.kind != DatSourceKind::File
        || !entry.path.is_file()
        || entry.health.is_stale_for(&entry.path, entry.kind)
        || !matches!(
            entry.health.state(),
            DatHealthState::Valid | DatHealthState::ValidWithWarnings
        )
        || entry.health.entry_count.unwrap_or(0) == 0
    {
        return None;
    }
    let [revision] = entry.health.arcade_catalogue_revisions.as_slice() else {
        return None;
    };
    (revision.ecosystem == DatEcosystem::MAMEArcade).then(|| match revision.version.as_deref() {
        Some(version) => format!("MAME {version} Arcade"),
        None => "MAME Arcade".to_string(),
    })
}

fn row_ready(row: &DatSourceRowView) -> bool {
    row.enabled
        && !row.health_stale
        && !row.incomplete_load
        && matches!(
            row.health_state,
            DatHealthState::Valid | DatHealthState::ValidWithWarnings
        )
        && row.entry_count.unwrap_or(0) > 0
        && Path::new(&row.path).exists()
        && row
            .detail
            .as_ref()
            .is_none_or(|detail| detail.duplicate_identities.is_empty())
}

pub(super) fn arcade_ready_count(view: &DatSourcesPageView) -> usize {
    view.rows
        .iter()
        .filter(|row| row.arcade_verification.is_some() && row_ready(row))
        .count()
}

fn choices(
    view: &DatSourcesPageView,
    managed: &[InstalledCatalogueSummary],
    platform: &str,
) -> Vec<Choice> {
    let mut choices: Vec<_> = view
        .rows
        .iter()
        .filter(|row| {
            row.enabled
                && row
                    .platform_id
                    .as_deref()
                    .and_then(archivefs_core::canonical_platform_for_alias)
                    == Some(platform)
        })
        .map(|row| Choice {
            reference: CatalogueRef::local(&row.id),
            label: row
                .arcade_verification
                .clone()
                .unwrap_or_else(|| row.display_name.clone()),
            ready: row_ready(row),
        })
        .collect();
    choices.extend(
        managed
            .iter()
            .filter(|row| {
                matches!(row.reference, CatalogueRef::ManagedCurrent { .. })
                    && row.enabled
                    && row.variant.confirmed() != Some(&CatalogueVariant::Bios)
                    && row
                        .platform
                        .confirmed()
                        .and_then(|p| archivefs_core::canonical_platform_for_alias(p))
                        == Some(platform)
            })
            .map(|row| Choice {
                reference: row.reference.clone(),
                label: row.display_name.clone(),
                ready: row.capabilities.verify && row.availability.is_ready(),
            }),
    );
    choices
}

fn selected_choice<'a>(
    choices: &'a [Choice],
    selected: Option<&CatalogueRef>,
) -> Option<&'a Choice> {
    if let Some(selected) = selected {
        // A disappeared explicit choice is not replaced by another catalogue.
        return choices.iter().find(|choice| &choice.reference == selected);
    }
    match choices {
        [choice] => Some(choice),
        _ => None,
    }
}

fn status(choices: &[Choice], selected: Option<&CatalogueRef>, supported: bool) -> Status {
    if let Some(choice) = selected_choice(choices, selected) {
        return if choice.ready {
            Status::Ready
        } else {
            Status::NeedsAttention
        };
    }
    if !choices.is_empty() || selected.is_some() {
        Status::NeedsAttention
    } else if supported {
        Status::NeedsSetup
    } else {
        Status::NotSupported
    }
}

pub(super) fn assignment_prompt(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    state: &mut SimpleCheckUi,
) -> Option<DatSourcesPageAction> {
    let mut action = None;
    // There is exactly one canonical Arcade platform. Multiple candidate
    // catalogues are *not* elected: each confirmation assigns only that row,
    // and the platform chooser still asks which data to verify against.
    for row in view.rows.iter().filter(|row| {
        row.enabled
            && row.platform_id.is_none()
            && row.arcade_verification.is_some()
            && row_ready(row)
    }) {
        if state.declined.contains(&row.id) {
            continue;
        }
        ui.push_id(("arcade-assignment", &row.id), |ui| {
            widgets::full_width_card(ui, |ui| {
                ui.strong(row.arcade_verification.as_deref().unwrap_or("MAME Arcade"));
                ui.label("This looks like MAME arcade verification data. Use it for Arcade?");
                ui.label("This changes only EmuWiz's setup, not your games. You can review it before saving.");
                ui.horizontal(|ui| {
                    if primary_button(ui, "Yes — use for Arcade", !view.background_busy).clicked() {
                        state.platform = Some("Arcade".into());
                        action = Some(DatSourcesPageAction::SetPlatform { id: row.id.clone(), platform: Some("Arcade".into()) });
                    }
                    if ui.button("No").clicked() { state.declined.insert(row.id.clone()); }
                });
            });
        });
    }
    action
}

pub(super) fn show(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    // Reuse the existing asynchronous inventory for managed catalogues. Local
    // rows always come from the live draft, not a second stale registry copy.
    if ui_state.catalogue_picker.poll() || ui_state.catalogue_picker.loading {
        ui.ctx().request_repaint();
    }
    if ui_state.catalogue_picker.error.is_some()
        && ui.button("Try reading verification setup again").clicked()
    {
        ui_state.catalogue_picker.invalidate();
        ui.ctx().request_repaint();
    }
    let managed = ui_state.catalogue_picker.summaries();
    let inventory_pending =
        ui_state.catalogue_picker.loading || ui_state.catalogue_picker.error.is_some();
    let state = &mut ui_state.simple;
    let mut action = None;
    widgets::workflow_header(
        ui,
        "Check My Games",
        "See whether your games are recognised, complete and healthy. Checking does not change your games.",
    );
    if let Some(running) = &view.running {
        ui.strong(if running.cancellation_requested {
            "Stopping the check — your games are unchanged."
        } else if running.cancellable {
            "Checking — you can cancel safely; your games are unchanged."
        } else {
            "Reading verification data — please wait. Your games are unchanged."
        });
        if let Some(progress) = &running.progress {
            ui.label(progress.position());
        }
        if ui
            .add_enabled(
                running.cancellable && !running.cancellation_requested,
                egui::Button::new("Cancel check"),
            )
            .clicked()
        {
            action = Some(DatSourcesPageAction::CancelJob);
        }
        ui.ctx().request_repaint();
    }
    for (index, error) in [
        view.load_error.as_ref(),
        view.action_error.as_ref(),
        view.audit_error.as_ref(),
        match &view.save_state {
            DatSaveState::Failed(error) => Some(error),
            _ => None,
        },
        ui_state.catalogue_picker.error.as_ref(),
    ]
    .into_iter()
    .flatten()
    .enumerate()
    {
        widgets::banner(
            ui,
            "This check needs attention",
            "EmuWiz could not finish reading or saving the setup. Your game files have not changed. Open Change setup, check the selected data and folder, then try again.",
            widgets::StatusTone::Warning,
        );
        widgets::technical_details(ui, ("simple-error", index), |ui| {
            ui.label(error);
        });
    }
    if let Some(prompt) = assignment_prompt(ui, view, state) {
        action = Some(prompt);
    }
    let Some(platform) = state.platform.clone() else {
        ui.label("Choose a platform to see its setup and next step.");
        ui.add(egui::TextEdit::singleline(&mut state.query).hint_text("Find a platform"));
        let mut platforms: BTreeSet<String> = ["Arcade", "PlayStation 2", "Dreamcast", "Amiga"]
            .map(str::to_string)
            .into_iter()
            .collect();
        platforms.extend(
            view.authority_rows
                .iter()
                .filter(|row| row.expected_sources.iter().any(|source| source.active))
                .map(|row| row.platform.clone())
                .filter(|p| p != "Unknown platform"),
        );
        if !state.query.trim().is_empty() {
            platforms.extend(
                platform_choices(&state.query)
                    .into_iter()
                    .map(|(id, _)| id.to_string()),
            );
        }
        platforms.extend(view.rows.iter().filter_map(|row| {
            row.platform_id
                .as_deref()
                .and_then(archivefs_core::canonical_platform_for_alias)
                .map(str::to_string)
        }));
        for platform in platforms
            .into_iter()
            .filter(|p| p.to_lowercase().contains(&state.query.to_lowercase()))
        {
            let candidates = choices(view, managed, &platform);
            let supported = view
                .authority_rows
                .iter()
                .find(|row| row.platform == platform)
                .is_none_or(|row| !row.expected_sources.is_empty());
            let status = status(&candidates, state.selected.get(&platform), supported);
            widgets::full_width_card(ui, |ui| {
                ui.heading(&platform);
                ui.strong(status.label());
                let label = if status == Status::NeedsSetup {
                    format!("Set up {platform}")
                } else if status == Status::NotSupported {
                    format!("See options for {platform}")
                } else {
                    format!("Check {platform} games")
                };
                if primary_button(ui, &label, true).clicked() {
                    state.platform = Some(platform.clone());
                    state.setup = status != Status::Ready;
                }
            });
            ui.add_space(12.0);
        }
        if ui.button("Import verification data…").clicked()
            && let Some(path) = choose_local_dat_file("Import verification data")
        {
            action = Some(DatSourcesPageAction::ImportVerificationData { path });
        }
        ui.collapsing("Advanced details", |ui| {
            if ui.button("Open DATs & Verification").clicked() {
                action = Some(DatSourcesPageAction::OpenDatSources);
            }
        });
        return action;
    };
    let candidates = choices(view, managed, &platform);
    let chosen = selected_choice(&candidates, state.selected.get(&platform));
    let ready = chosen.is_some_and(|choice| choice.ready);
    let supported = view
        .authority_rows
        .iter()
        .find(|row| row.platform == platform)
        .is_none_or(|row| !row.expected_sources.is_empty());
    ui.label(format!("Check My Games › {platform}"));
    if ui.button("← Back to platforms").clicked() {
        state.platform = None;
        return action;
    }
    widgets::full_width_card(ui, |ui| {
        ui.heading(if platform == "Arcade" {
            "MAME / Arcade"
        } else {
            &platform
        });
        let current = status(&candidates, state.selected.get(&platform), supported);
        ui.strong(current.label());
        if current == Status::NotSupported {
            ui.label("EmuWiz has no recommended verification data for this platform yet. You can still View games and play them. Change setup lets you import compatible data you already have.");
        }
        if let Some(chosen) = chosen {
            ui.label(format!("Verification data: {}", chosen.label));
        }
        if inventory_pending {
            ui.label("Checking which verification data is available. Please wait before starting.");
        }
        if platform == "Arcade" {
            ui.label(
                "Installed MAME program: not checked here (optional). Imported verification data works without it.",
            );
        }
        if candidates.len() > 1 {
            ui.label("More than one set of verification data is available. Choose the one that matches your collection.");
            egui::ComboBox::from_id_salt(("simple-catalogue", &platform))
                .selected_text(chosen.map_or("Choose verification data", |c| c.label.as_str()))
                .show_ui(ui, |ui| {
                    for choice in &candidates {
                        if ui
                            .selectable_label(
                                chosen.is_some_and(|c| c.reference == choice.reference),
                                &choice.label,
                            )
                            .clicked()
                        {
                            state
                                .selected
                                .insert(platform.clone(), choice.reference.clone());
                            state.result_for = None;
                        }
                    }
                });
        }
        if !ready {
            ui.label("This platform is not ready to check yet. Import verification data you already have, or review the data below.");
        }
        if !state.folders.contains_key(&platform)
            && let [folder] = view.library_folders.as_slice()
        {
            state.folders.insert(platform.clone(), folder.clone());
        }
        ui.strong("Games folder");
        let folder = state.folders.get(&platform).cloned();
        if let Some(folder) = &folder {
            ui.label(folder.display().to_string());
        } else {
            ui.label("Choose the folder containing this platform's games.");
        }
        if folder.as_ref().is_some_and(|path| !path.is_dir()) {
            ui.strong("This games folder is unavailable. Connect its drive or choose another folder before starting the check. Your games have not been changed.");
        }
        if !view.library_folders.is_empty() {
            egui::ComboBox::from_id_salt(("simple-folder", &platform))
                .selected_text("Choose a saved games folder")
                .show_ui(ui, |ui| {
                    for saved in &view.library_folders {
                        if ui
                            .selectable_label(
                                folder.as_ref() == Some(saved),
                                saved.display().to_string(),
                            )
                            .clicked()
                        {
                            state.folders.insert(platform.clone(), saved.clone());
                            state.result_for = None;
                        }
                    }
                });
        }
        if ui.button("Choose another games folder…").clicked()
            && let Some(folder) = rfd::FileDialog::new()
                .set_title(format!("Choose {platform} games folder"))
                .pick_folder()
        {
            state.folders.insert(platform.clone(), folder);
            state.result_for = None;
        }
        ui.add_space(12.0);
        if view.dirty {
            ui.label("Review and save your setup before checking. No games will be changed.");
            ui.collapsing("Review setup changes", |ui| {
                for change in &view.pending_consequences {
                    ui.label(change);
                }
            });
            if primary_button(ui, "Save setup", !view.background_busy).clicked() {
                action = Some(DatSourcesPageAction::Save);
            }
            if ui.button("Cancel setup changes").clicked() {
                action = Some(DatSourcesPageAction::Revert);
            }
        } else if ready {
            let label = if platform == "Arcade" {
                "Verify Arcade Collection".to_string()
            } else {
                format!("Verify {platform} games")
            };
            if primary_button(
                ui,
                &label,
                !inventory_pending
                    && !view.background_busy
                    && folder.as_ref().is_some_and(|p| p.is_dir()),
            )
            .clicked()
                && let (Some(chosen), Some(folder)) = (chosen, folder)
            {
                state.result_for =
                    Some((platform.clone(), chosen.reference.clone(), folder.clone()));
                action = Some(match &chosen.reference {
                    CatalogueRef::Local { source_id, .. } => DatSourcesPageAction::Audit {
                        id: source_id.clone(),
                        scan_root: folder,
                    },
                    _ => DatSourcesPageAction::VerifyCatalogue {
                        reference: chosen.reference.clone(),
                        scan_root: folder,
                    },
                });
            }
            ui.label("Next: see which files matched, which need attention, and what to do next. Nothing is renamed or repaired by this check.");
        } else if primary_button(ui, "Set up verification", !view.background_busy).clicked() {
            state.setup = true;
        }
        if ui
            .button(if state.setup {
                "Close setup"
            } else {
                "Change setup"
            })
            .clicked()
        {
            state.setup = !state.setup;
        }
        if ui.button("View games").clicked() {
            action = Some(DatSourcesPageAction::ViewPlatformGames {
                platform: platform.clone(),
            });
        }
    });
    if state.setup {
        widgets::full_width_card(ui, |ui| {
            ui.heading(format!("Set up {platform}"));
            ui.label("Import verification data you trust, check it, then save the setup. Installing an emulator is a separate task.");
            if primary_button(ui, "Import verification data…", !view.background_busy).clicked()
                && let Some(path) = choose_local_dat_file("Import verification data (DAT or XML)")
            {
                action = Some(DatSourcesPageAction::ImportVerificationData { path });
            }
            for row in view.rows.iter().filter(|row| {
                row.platform_id.is_none()
                    || row
                        .platform_id
                        .as_deref()
                        .and_then(archivefs_core::canonical_platform_for_alias)
                        == Some(&platform)
            }) {
                ui.push_id(("setup-row", &row.id), |ui| {
                    ui.strong(&row.display_name);
                    if !row_ready(row) {
                        ui.label("The verification data needs checking before it can be used.");
                        if ui
                            .add_enabled(
                                !view.background_busy,
                                egui::Button::new("Check this verification data"),
                            )
                            .clicked()
                        {
                            action = Some(DatSourcesPageAction::Validate { id: row.id.clone() });
                        }
                    } else if row.platform_id.is_none()
                        && ui.button(format!("Use for {platform}")).clicked()
                    {
                        action = Some(DatSourcesPageAction::SetPlatform {
                            id: row.id.clone(),
                            platform: Some(platform.clone()),
                        });
                    }
                });
            }
        });
    }
    if let Some(audit) = view.audit.as_deref()
        && state.result_for.as_ref().is_some_and(|(p, reference, _)| p == &platform &&
            (reference.token() == audit.source_id || matches!(reference, CatalogueRef::Local {source_id, ..} if source_id == &audit.source_id))) {
        ui.label(format!("Check My Games › {platform} › Verification results"));
        widgets::full_width_card(ui, |ui| {
            ui.heading("Verification results");
            ui.label(format!("Last check: {} · {}", audit.source_display_name, audit.scan_root_short));
            ui.label(format!("{} files checked. Your game files were not changed.", audit.files_scanned));
            for category in &audit.categories {
                let label = match category.label { "Exact" => "Verified matches", "Not in catalogue" => "Not recognised by this data", "Filename only" => "Name matched; contents not verified", other => other };
                ui.label(format!("{label}: {}", category.count));
            }
            if audit.truncated || !audit.unhashed.is_empty() || !audit.unreadable_catalogues.is_empty() {
                ui.strong("Some files could not be checked. This is a partial result, not a clean bill of health.");
            }
            ui.label("A file that is not recognised is not necessarily broken. Check that the verification data matches your collection before considering a repair.");
            if primary_button(ui, "Back to platform setup", true).clicked() { state.result_for = None; }
            widgets::technical_details(ui, "simple-audit-details", |ui| { show_audit_result(ui, audit); });
        });
    }
    ui.collapsing("Advanced details", |ui| {
        ui.label("Explore the saved information here. Opening details does not change your games or activate anything.");
        ui.label(format!("MAME software lists: {}", if view.managed_rows.iter().any(|r| r.installed) { "Ready" } else { "Not configured — not required for Arcade" }));
        if ui.button("Open DATs & Verification").clicked() { action = Some(DatSourcesPageAction::OpenDatSources); }
    });
    action
}

#[cfg(test)]
mod tests;
