//! Platform-first presentation over the existing DAT registry and audit worker.
//! No provider activation, rename, or new identity rules live here.
use super::*;
use crate::simple_mode::primary_button;
use archivefs_core::dat::catalogue_selection::{CatalogueVariant, InstalledCatalogueSummary};
use archivefs_core::dat::model::DatEcosystem;

mod catalogue_list;

#[derive(Default)]
pub(crate) struct SimpleCheckUi {
    platform: Option<String>,
    pub(super) verification: super::native_verification::Controller,
    catalogue_query: String,
    catalogue_list: catalogue_list::CatalogueList,
    pending_catalogue: Option<CatalogueRef>,
    pending_collection: Option<PathBuf>,
    query: String,
    setup: bool,
    selected: BTreeMap<String, CatalogueRef>,
    folders: BTreeMap<String, PathBuf>,
    declined: BTreeSet<String>,
    result_for: Option<(String, CatalogueRef, PathBuf)>,
    /// A platform chosen by a caller (Check Games) rather than by a click here:
    /// the first render opens setup if, and only if, the platform is not ready.
    open_setup_if_not_ready: bool,
}

impl SimpleCheckUi {
    /// Arrive already on `platform`, as if it had been chosen in the list.
    pub(crate) fn select_platform(&mut self, platform: &str) {
        if self.platform.as_deref() != Some(platform) {
            self.platform = Some(platform.to_string());
            self.setup = false;
            self.result_for = None;
        }
        self.open_setup_if_not_ready = true;
    }

    pub(super) fn open_verification(
        &mut self,
        catalogue: CatalogueRef,
        platform: Option<String>,
        collection: PathBuf,
    ) {
        self.pending_catalogue = Some(catalogue);
        self.pending_collection = Some(collection.clone());
        self.platform = platform.clone();
        if let Some(platform) = platform {
            self.folders.insert(platform, collection);
        }
        self.result_for = None;
    }

    pub(crate) fn platform(&self) -> Option<&str> {
        self.platform.as_deref()
    }

    /// Successful confirmed verifications; a change means saved DAT
    /// evidence is newer than what a host has loaded.
    pub(crate) fn verification_completions(&self) -> u64 {
        self.verification.completions()
    }

    #[cfg(test)]
    pub(crate) fn record_verification_for_test(&mut self) {
        self.verification.record_completion_for_test();
    }
}

/// After a confirmed verification proved a local catalogue compatible with
/// `platform`, ask (never assume) whether to record that system for it. Yes
/// saves that one assignment at once; Not now changes nothing. An
/// already-assigned or declined row is not asked.
fn assignment_after_verification(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    state: &mut SimpleCheckUi,
    platform: &str,
) -> Option<DatSourcesPageAction> {
    match &view.platform_assignment {
        Some(Ok(message)) => {
            ui.colored_label(theme::SUCCESS, message);
        }
        Some(Err(message)) => {
            ui.colored_label(theme::WARNING, message);
        }
        None => {}
    }
    let (id, proven) = state.verification.assignable()?.clone();
    let row = view.rows.iter().find(|row| row.id == id);
    let Some(row) = row.filter(|row| {
        proven == platform && row.platform_id.is_none() && !state.declined.contains(&row.id)
    }) else {
        state.verification.clear_assignable();
        return None;
    };
    let mut action = None;
    widgets::full_width_card(ui, |ui| {
        ui.strong(format!(
            "Use {} for {platform} from now on?",
            row.display_name
        ));
        ui.label(format!(
            "Its catalogue header identifies {platform} and the verification you confirmed matched it. \
             Saving this keeps it selected after a restart and lets launch checks use it."
        ));
        ui.label("Yes saves this to EmuWiz's setup now. Your games are not changed.");
        ui.horizontal(|ui| {
            if primary_button(
                ui,
                &format!("Yes — use for {platform}"),
                !view.background_busy,
            )
            .clicked()
            {
                action = Some(DatSourcesPageAction::AssignPlatformNow {
                    id: row.id.clone(),
                    platform: platform.to_string(),
                });
                state.verification.clear_assignable();
            }
            if ui.button("Not now").clicked() {
                state.declined.insert(row.id.clone());
                state.verification.clear_assignable();
            }
        });
    });
    action
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
    compatible: bool,
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

fn row_metadata_ready(row: &DatSourceRowView) -> bool {
    row.enabled
        && !row.health_stale
        && !row.incomplete_load
        && matches!(
            row.health_state,
            DatHealthState::Valid | DatHealthState::ValidWithWarnings
        )
        && row.entry_count.unwrap_or(0) > 0
        && row
            .detail
            .as_ref()
            .is_none_or(|detail| detail.duplicate_identities.is_empty())
}

fn row_ready(row: &DatSourceRowView) -> bool {
    row_metadata_ready(row) && Path::new(&row.path).exists()
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
    choices_for_selection(view, managed, platform, None)
}

fn choices_for_selection(
    view: &DatSourcesPageView,
    managed: &[InstalledCatalogueSummary],
    platform: &str,
    selected: Option<&CatalogueRef>,
) -> Vec<Choice> {
    let rows: Vec<_> = view
        .rows
        .iter()
        .filter(|row| {
            row.enabled
                && (row
                    .platform_id
                    .as_deref()
                    .and_then(archivefs_core::canonical_platform_for_alias)
                    == Some(platform)
                    || platform != "Arcade"
                        && row.platform_id.is_none()
                        && selected == Some(&CatalogueRef::local(&row.id)))
        })
        .collect();
    let only_one = rows.len() == 1;
    let mut choices: Vec<_> = rows
        .into_iter()
        .map(|row| Choice {
            reference: CatalogueRef::local(&row.id),
            label: row
                .arcade_verification
                .clone()
                .unwrap_or_else(|| row.display_name.clone()),
            // Unselected choices need no filesystem check: they cannot start a
            // preview. Re-evaluate the actual explicit choice before offering it.
            ready: if only_one || selected == Some(&CatalogueRef::local(&row.id)) {
                row_ready(row)
            } else {
                row_metadata_ready(row)
            },
            compatible: row.platform_id.is_some(),
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
                compatible: true,
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
        return if choice.ready && choice.compatible {
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

/// One plain sentence about where this platform's verification data stands.
fn state_guidance(
    platform: &str,
    status: Status,
    has_choices: bool,
    installed_unused: bool,
) -> String {
    match status {
        Status::Ready => "Verification data is ready. You can now check these games.".to_string(),
        Status::NeedsAttention if has_choices => {
            format!("Choose which {platform} verification data to use.")
        }
        Status::NeedsAttention | Status::NeedsSetup if installed_unused => {
            format!("{platform} verification data is installed but not enabled yet.")
        }
        Status::NeedsAttention => {
            format!("{platform} verification data needs checking before it can be used.")
        }
        Status::NeedsSetup => format!("{platform} identification data is not set up yet."),
        Status::NotSupported => {
            format!("EmuWiz has no recommended identification data for {platform} yet.")
        }
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

pub(super) fn show_with_inputs(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    ui_state: &mut DatSourcesPageUi,
    database: Option<PathBuf>,
    trusted: archivefs_core::safe_read::TrustedRoots,
) -> Option<DatSourcesPageAction> {
    // Reuse the existing asynchronous inventory for managed catalogues. Local
    // rows always come from the live draft, not a second stale registry copy.
    if ui_state.catalogue_picker.poll() {
        ui.ctx().request_repaint();
    } else if ui_state.catalogue_picker.loading {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    if ui_state.catalogue_picker.error.is_some()
        && ui.button("Try reading verification setup again").clicked()
    {
        ui_state.catalogue_picker.invalidate();
        ui.ctx().request_repaint();
    }
    let inventory_revision = ui_state.catalogue_picker.revision();
    let inventory = ui_state.catalogue_picker.snapshot();
    let managed = ui_state.catalogue_picker.summaries();
    let inventory_pending =
        ui_state.catalogue_picker.loading || ui_state.catalogue_picker.error.is_some();
    let state = &mut ui_state.simple;
    let mut action = None;
    widgets::workflow_header(
        ui,
        "Check My Games",
        "Verify assigned library games against trusted catalogue content hashes. Game files are unchanged.",
    );
    if state.verification.busy() {
        state
            .verification
            .show(ui, inventory, database, trusted, false);
        return None;
    }
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
        let mut platforms: BTreeSet<String> = [
            "Game Boy",
            "Game Boy Color",
            "ZX Spectrum",
            "Arcade",
            "PlayStation 2",
            "Dreamcast",
            "Amiga",
        ]
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
    if let Some(reference) = state.pending_catalogue.take() {
        state.selected.insert(platform.clone(), reference);
        if let Some(collection) = state.pending_collection.take() {
            state.folders.insert(platform.clone(), collection);
        }
    }
    let candidates = choices_for_selection(view, managed, &platform, state.selected.get(&platform));
    let chosen = selected_choice(&candidates, state.selected.get(&platform));
    let ready = chosen.is_some_and(|choice| choice.ready);
    let supported = view
        .authority_rows
        .iter()
        .find(|row| row.platform == platform)
        .is_none_or(|row| !row.expected_sources.is_empty());
    let installed_unused = view.rows.iter().any(|row| {
        !row.enabled
            && row
                .platform_id
                .as_deref()
                .and_then(archivefs_core::canonical_platform_for_alias)
                == Some(platform.as_str())
    });
    if std::mem::take(&mut state.open_setup_if_not_ready) {
        state.setup = !ready && supported;
    }
    ui.label(format!("Check My Games › {platform}"));
    if ui.button("‹ Back to platforms").clicked() {
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
        ui.label("Catalogue installed › compatible catalogue selected › Preview › confirm verification. Games remain Not checked until a content check has completed; catalogue counts are not verified-game counts.");
        if current == Status::NotSupported {
            ui.label("EmuWiz has no recommended verification data for this platform yet. You can still View games and play them. Change setup lets you import compatible data you already have.");
        }
        if candidates.len() == 1
            && let Some(chosen) = chosen
        {
            ui.label(format!("Verification data: {}", chosen.label));
        }
        if !ready || chosen.is_some_and(|choice| !choice.compatible) || platform == "Arcade" {
            if chosen.is_some_and(|choice| !choice.compatible) {
                ui.label("Catalogue selected; platform compatibility is not established yet. Preview must prove compatibility before verification can be confirmed.");
            } else {
                ui.label(state_guidance(
                    &platform,
                    current,
                    !candidates.is_empty(),
                    installed_unused,
                ));
            }
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
            ui.label(if platform == "Arcade" { "More than one set of verification data is available. Choose the one that matches your collection." } else { "Choose an installed catalogue; Preview proves platform compatibility." });
            ui.add(
                egui::TextEdit::singleline(&mut state.catalogue_query)
                    .hint_text("Find an installed catalogue"),
            );
            egui::ComboBox::from_id_salt(("simple-catalogue", &platform))
                .selected_text(chosen.map_or("Choose verification data", |c| c.label.as_str()))
                .show_ui(ui, |ui| {
                    for choice in candidates.iter().filter(|c| {
                        c.label
                            .to_lowercase()
                            .contains(&state.catalogue_query.to_lowercase())
                    }) {
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
        if platform == "Arcade"
            && !state.folders.contains_key(&platform)
            && let [folder] = view.library_folders.as_slice()
        {
            state.folders.insert(platform.clone(), folder.clone());
        }
        ui.strong("Library collection");
        let folder = state.folders.get(&platform).cloned();
        egui::ComboBox::from_id_salt(("verification-collection", &platform))
            .selected_text(
                folder
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "All library roots for this system".into()),
            )
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(folder.is_none(), "All library roots for this system")
                    .clicked()
                {
                    state.folders.remove(&platform);
                }
                for saved in &view.library_folders {
                    if ui
                        .selectable_label(
                            folder.as_ref() == Some(saved),
                            saved.display().to_string(),
                        )
                        .clicked()
                    {
                        state.folders.insert(platform.clone(), saved.clone());
                    }
                }
            });
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
        } else if ready && platform == "Arcade" {
            if primary_button(
                ui,
                "Verify Arcade Collection",
                !view.background_busy && folder.as_ref().is_some_and(|p| p.is_dir()),
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
        } else if ready {
            if let Some(chosen) = chosen.filter(|c| {
                state
                    .selected
                    .get(&platform)
                    .is_none_or(|r| r == &c.reference)
            }) {
                state
                    .verification
                    .select(super::native_verification::Selection {
                        platform: platform.clone(),
                        catalogue: chosen.reference.clone(),
                        collection: state.folders.get(&platform).cloned(),
                        database: database.clone(),
                        source_binding: inventory
                            .as_ref()
                            .map(|i| match &chosen.reference {
                                CatalogueRef::Local { source_id, .. } => {
                                    format!("{:?}", i.local_registry.get(source_id))
                                }
                                _ => chosen.reference.token(),
                            })
                            .unwrap_or_default(),
                    });
                state.verification.show(
                    ui,
                    inventory.clone(),
                    database.clone(),
                    trusted.clone(),
                    !inventory_pending && !view.background_busy,
                );
                if let Some(assign) = assignment_after_verification(ui, view, state, &platform) {
                    action = Some(assign);
                }
            }
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
            if let Some(list_action) =
                catalogue_list::show(ui, view, state, &platform, managed, inventory_revision)
            {
                action = Some(list_action);
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
            if !audit.population_note.is_empty() {
                ui.label(&audit.population_note);
            }
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

#[cfg(test)]
fn show(
    ui: &mut egui::Ui,
    view: &DatSourcesPageView,
    state: &mut DatSourcesPageUi,
) -> Option<DatSourcesPageAction> {
    show_with_inputs(
        ui,
        view,
        state,
        None,
        archivefs_core::safe_read::TrustedRoots::none(),
    )
}
