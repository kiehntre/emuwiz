// Library-management and settings page rendering extracted mechanically from main.rs.

use super::*;

/// Renders the currently-previewed plan's counts and (filtered) entry
/// details - shared by every view's Preview output, since only one view's
/// plan is ever cached at a time (`ArchiveFsApp::library_view_last_plan`).
/// Performs no filesystem access itself; purely a display of the already-
/// computed `plan`.
pub(super) fn show_library_view_plan_summary(
    ui: &mut egui::Ui,
    plan: &LibraryViewPlan,
    filter: &mut LibraryViewPlanFilter,
) {
    if let Some(error) = &plan.unsafe_root_error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Unsafe destination - Apply/Repair are refused: {error}"),
        );
        return;
    }
    if let Some(error) = &plan.profile_error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Unsupported frontend profile - Apply/Repair are refused: {error}"),
        );
        return;
    }
    if let Some(conflict) = &plan.fingerprint_conflict {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Profile changed since this view was last applied - review before re-applying: \
                 {conflict}"
            ),
        );
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.label(format!("Create: {}", plan.counts.create));
        ui.label(format!("Correct: {}", plan.counts.correct));
        ui.label(format!("Repair: {}", plan.counts.repair));
        ui.label(format!("Remove: {}", plan.counts.remove));
        ui.label(format!("Collision: {}", plan.counts.collision));
        ui.label(format!("Skip: {}", plan.counts.skip));
    });

    ui.horizontal_wrapped(|ui| {
        for candidate in [
            LibraryViewPlanFilter::All,
            LibraryViewPlanFilter::Create,
            LibraryViewPlanFilter::Correct,
            LibraryViewPlanFilter::Repair,
            LibraryViewPlanFilter::Remove,
            LibraryViewPlanFilter::Collision,
            LibraryViewPlanFilter::Skip,
        ] {
            if ui
                .selectable_label(*filter == candidate, candidate.label())
                .clicked()
            {
                *filter = candidate;
            }
        }
    });

    let visible: Vec<&LibraryViewPlanEntry> = plan
        .entries
        .iter()
        .filter(|entry| filter.matches(entry.action))
        .collect();
    if visible.is_empty() {
        ui.label("No entries match this filter.");
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("library_view_plan_details")
        .max_height(240.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in visible {
                let path_text = entry
                    .destination_path
                    .as_ref()
                    .or(entry.archive_path.as_ref())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "?".to_string());
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("[{:?}]", entry.action));
                    ui.label(egui::RichText::new(path_text).monospace());
                    if let Some(reason) = &entry.reason {
                        ui.weak(reason);
                    }
                });
            }
        });
}

pub(super) fn show_library_view_source_selection(
    ui: &mut egui::Ui,
    sources: &[SourceFolderView],
    selected: &mut HashSet<PathBuf>,
) {
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Sources",
            Some("Leave every source unchecked to include all configured sources."),
        );
        egui::ScrollArea::vertical()
            .id_salt("library_view_form_sources")
            .max_height(210.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if sources.is_empty() {
                    ui.weak("No source folders are configured.");
                }
                for source in sources {
                    let mut checked = selected.contains(&source.path);
                    if ui
                        .checkbox(&mut checked, source.path.display().to_string())
                        .on_hover_text(source.path.display().to_string())
                        .changed()
                    {
                        if checked {
                            selected.insert(source.path.clone());
                        } else {
                            selected.remove(&source.path);
                        }
                    }
                }
            });
    });
}

pub(super) fn show_library_view_platform_selection(
    ui: &mut egui::Ui,
    selected: &mut HashSet<String>,
) {
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Platforms",
            Some("Leave every platform unchecked to include all known platforms."),
        );
        egui::ScrollArea::vertical()
            .id_salt("library_view_form_platforms")
            .max_height(210.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for platform in canonical_platform_names() {
                    let mut checked = selected.contains(platform);
                    if ui.checkbox(&mut checked, platform).changed() {
                        if checked {
                            selected.insert(platform.to_string());
                        } else {
                            selected.remove(platform);
                        }
                    }
                }
            });
    });
}

/// The Frontend Profile section of the Add/Edit View dialog: which frontend
/// (`FrontendProfileKind`) the view is nominally shaped for, and - only for
/// `Romm` - its explicit `catalogue platform -> RomM slug` overrides.
/// `Generic` is the default and, being the pre-existing behaviour, needs no
/// extra controls at all. `EsDe` is shown (so the vocabulary is visible -
/// milestone note: "should remain visible only if that fits the current UI
/// vocabulary") but its own radio option is disabled, with a short reason,
/// since `FrontendProfileKind::EsDe` still fails closed in the backend -
/// selecting it here would only ever produce a refused plan.
///
/// Writes overrides into `overrides` as an edited `(platform, slug)` list;
/// the caller (the dialog's submit handler) is the only place that turns
/// this into a real `FrontendPlatformMapping` - this function never talks to
/// `archivefs_core` at all, matching every other selection widget in this
/// dialog (`show_library_view_source_selection`/`show_library_view_platform_selection`).
#[allow(clippy::too_many_arguments)]
pub(super) fn show_library_view_profile_selection(
    ui: &mut egui::Ui,
    profile_kind: &mut FrontendProfileKind,
    overrides: &mut Vec<(String, String)>,
    platform_input: &mut String,
    slug_input: &mut String,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Frontend Profile",
            Some(
                "Which frontend this view is shaped for. Generic is the existing \
                 {platform}/{filename} layout and is unaffected by anything below.",
            ),
        );
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(profile_kind, FrontendProfileKind::Generic, "Generic");
            ui.selectable_value(profile_kind, FrontendProfileKind::Romm, "RomM");
            ui.add_enabled_ui(false, |ui| {
                ui.selectable_value(profile_kind, FrontendProfileKind::EsDe, "ES-DE");
            })
            .response
            .on_disabled_hover_text(
                "ES-DE planning is not implemented yet - selecting it would only produce a \
                 refused plan.",
            );
        });

        if *profile_kind == FrontendProfileKind::Romm {
            ui.add_space(6.0);
            widgets::banner(
                ui,
                "RomM layout",
                "Plans roms/<slug>/<filename> under the destination above. A platform with no \
                 resolved RomM slug is skipped individually, never guessed - add an explicit \
                 override below, or resolve it via a previously imported RomM identity cache.",
                widgets::StatusTone::Info,
            );
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Platform overrides").strong());
            if overrides.is_empty() {
                ui.weak("No explicit overrides yet.");
            } else {
                let mut remove_index = None;
                for (index, (platform, slug)) in overrides.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{platform} -> {slug}")).monospace());
                        if ui.small_button("Remove").clicked() {
                            remove_index = Some(index);
                        }
                    });
                }
                if let Some(index) = remove_index {
                    overrides.remove(index);
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Canonical platform:");
                show_text_edit_with_context_menu(ui, platform_input, clipboard, |text_edit| {
                    text_edit
                        .id_salt("library_view_form_romm_override_platform")
                        .desired_width(140.0)
                });
                ui.label("RomM slug:");
                show_text_edit_with_context_menu(ui, slug_input, clipboard, |text_edit| {
                    text_edit
                        .id_salt("library_view_form_romm_override_slug")
                        .desired_width(120.0)
                });
                let platform_trimmed = platform_input.trim();
                let slug_trimmed = slug_input.trim();
                let can_add = !platform_trimmed.is_empty() && !slug_trimmed.is_empty();
                if ui
                    .add_enabled(can_add, egui::Button::new("Add override"))
                    .clicked()
                {
                    overrides.retain(|(existing, _)| existing != platform_trimmed);
                    overrides.push((platform_trimmed.to_string(), slug_trimmed.to_string()));
                    platform_input.clear();
                    slug_input.clear();
                }
            });
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show_library_views_page(
    ui: &mut egui::Ui,
    views: &[LibraryViewConfig],
    all_source_folders: &[SourceFolderView],
    busy: bool,
    last_plan: Option<&(LibraryViewConfig, LibraryViewPlan)>,
    focus_archive: Option<&Path>,
    plan_filter: &mut LibraryViewPlanFilter,
    form_dialog: &mut Option<LibraryViewFormDialogState>,
    remove_dialog: &mut Option<LibraryViewRemoveDialogState>,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<LibraryViewAction> {
    let mut action = None;

    widgets::section_header(
        ui,
        "Views",
        Some(
            "Organised, symlink-based folder trees that point at your existing archives. \
             EmuWiz never moves, copies, renames, or deletes an original archive file.",
        ),
    );
    ui.add_space(2.0);
    if let Some(archive_path) = focus_archive {
        ui.add_space(4.0);
        match library_view_planned_entry_for(last_plan, archive_path) {
            Some(entry) => {
                let destination = entry
                    .destination_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "(no destination planned)".to_string());
                ui.label(format!(
                    "Preview status for {}: [{:?}] {destination}",
                    archive_path.display(),
                    entry.action
                ));
                if let Some(reason) = &entry.reason {
                    ui.weak(reason);
                }
            }
            None => {
                ui.weak(format!(
                    "No current preview covers {} - run Preview on the relevant view to see its \
                     status.",
                    archive_path.display()
                ));
            }
        }
        ui.add_space(4.0);
    }

    if ui
        .add_enabled(!busy, egui::Button::new("Add View"))
        .clicked()
    {
        *form_dialog = Some(LibraryViewFormDialogState::default());
    }
    ui.separator();

    if views.is_empty() {
        ui.label("No library views are configured yet. Click \"Add View\" to create one.");
    } else {
        egui::ScrollArea::vertical()
            .id_salt("library_views_list")
            .max_height(320.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for view in views {
                    // Compares the *whole* cached `LibraryViewConfig`, not
                    // just `id` - a stale plan computed for an earlier
                    // version of this same view (different source_folders,
                    // platforms, profile, ...) must never be shown or
                    // offered for Apply as if it still described `view`.
                    // `library_view_last_plan` is proactively cleared on
                    // every Add/Edit/Apply/Repair/Remove outcome already
                    // (see `poll_library_view_action`), but that is a
                    // single, easily-bypassed invalidation point (e.g. the
                    // config file changing on disk between two frames for
                    // any other reason); comparing full equality here is a
                    // second, always-correct guard that never depends on
                    // every mutation path remembering to invalidate the
                    // cache by hand.
                    let has_current_plan = last_plan
                        .as_ref()
                        .is_some_and(|(previewed, _)| previewed == view);
                    let can_apply = last_plan.as_ref().is_some_and(|(previewed, plan)| {
                        previewed == view && plan.is_safe_to_apply()
                    });
                    let group_response = ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.strong(&view.name);
                            if !view.enabled {
                                ui.weak("(disabled)");
                            }
                        });
                        ui.label(format!("Destination: {}", view.destination_root.display()));
                        ui.label(format!(
                            "Sources: {}",
                            if view.source_folders.is_empty() {
                                "all configured sources".to_string()
                            } else {
                                view.source_folders
                                    .iter()
                                    .map(|path| path.display().to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        ));
                        ui.label(format!(
                            "Platforms: {}",
                            if view.platforms.is_empty() {
                                "all known platforms".to_string()
                            } else {
                                view.platforms.join(", ")
                            }
                        ));
                        ui.label(format!("Layout: {}", view.layout_template.label()));

                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Preview"))
                                .clicked()
                            {
                                action = Some(LibraryViewAction::Preview(view.id.clone()));
                            }
                            if ui
                                .add_enabled(!busy && can_apply, egui::Button::new("Apply"))
                                .clicked()
                            {
                                action = Some(LibraryViewAction::Apply(view.id.clone()));
                            }
                            if ui
                                .add_enabled(!busy && can_apply, egui::Button::new("Repair"))
                                .clicked()
                            {
                                action = Some(LibraryViewAction::Repair(view.id.clone()));
                            }
                            if ui.add_enabled(!busy, egui::Button::new("Edit")).clicked() {
                                *form_dialog = Some(LibraryViewFormDialogState {
                                    editing_id: Some(view.id.clone()),
                                    name: view.name.clone(),
                                    destination_text: view.destination_root.display().to_string(),
                                    selected_source_folders: view
                                        .source_folders
                                        .iter()
                                        .cloned()
                                        .collect(),
                                    selected_platforms: view.platforms.iter().cloned().collect(),
                                    validation_message: None,
                                    profile_kind: view.profile.kind,
                                    romm_overrides: view
                                        .profile
                                        .policy
                                        .platform_mapping_overrides
                                        .iter()
                                        .map(|(platform, slug)| {
                                            (platform.to_string(), slug.to_string())
                                        })
                                        .collect(),
                                    romm_override_platform_input: String::new(),
                                    romm_override_slug_input: String::new(),
                                });
                            }
                            let enable_label = if view.enabled { "Disable" } else { "Enable" };
                            if ui
                                .add_enabled(!busy, egui::Button::new(enable_label))
                                .clicked()
                            {
                                action = Some(LibraryViewAction::SetEnabled {
                                    identifier: view.id.clone(),
                                    enabled: !view.enabled,
                                });
                            }
                            if ui.button("Copy destination path").clicked() {
                                let _ =
                                    clipboard.set_text(view.destination_root.display().to_string());
                            }
                            if ui.add_enabled(!busy, egui::Button::new("Remove")).clicked() {
                                *remove_dialog = Some(LibraryViewRemoveDialogState {
                                    view_id: view.id.clone(),
                                    view_name: view.name.clone(),
                                    keep_definition: true,
                                });
                            }
                        });

                        if has_current_plan {
                            ui.separator();
                            let (_, plan) = last_plan.expect("has_current_plan implies Some");
                            show_library_view_plan_summary(ui, plan, plan_filter);
                        }
                    });
                    group_response.response.context_menu(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Preview"))
                            .clicked()
                        {
                            action = Some(LibraryViewAction::Preview(view.id.clone()));
                            ui.close();
                        }
                        if ui
                            .add_enabled(!busy && can_apply, egui::Button::new("Apply"))
                            .clicked()
                        {
                            action = Some(LibraryViewAction::Apply(view.id.clone()));
                            ui.close();
                        }
                        if ui
                            .add_enabled(!busy && can_apply, egui::Button::new("Repair"))
                            .clicked()
                        {
                            action = Some(LibraryViewAction::Repair(view.id.clone()));
                            ui.close();
                        }
                        if ui.button("Copy destination path").clicked() {
                            let _ = clipboard.set_text(view.destination_root.display().to_string());
                            ui.close();
                        }
                        let enable_label = if view.enabled { "Disable" } else { "Enable" };
                        if ui
                            .add_enabled(!busy, egui::Button::new(enable_label))
                            .clicked()
                        {
                            action = Some(LibraryViewAction::SetEnabled {
                                identifier: view.id.clone(),
                                enabled: !view.enabled,
                            });
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(!busy, egui::Button::new("Remove View"))
                            .clicked()
                        {
                            *remove_dialog = Some(LibraryViewRemoveDialogState {
                                view_id: view.id.clone(),
                                view_name: view.name.clone(),
                                keep_definition: true,
                            });
                            ui.close();
                        }
                    });
                    ui.add_space(4.0);
                }
            });
    }

    if let Some(dialog) = form_dialog.as_mut() {
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        let title = if dialog.editing_id.is_some() {
            "Edit Library View"
        } else {
            "Add Library View"
        };
        let dialog_size =
            library_view_dialog_size(ui.ctx().input(|input| input.screen_rect().size()));
        egui::Window::new(title)
            .collapsible(false)
            .resizable(true)
            .default_size(dialog_size)
            .min_size(egui::vec2(
                dialog_size.x.min(460.0),
                dialog_size.y.min(440.0),
            ))
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let form_width = ui.available_width();
                let form_scroll_height = (ui.available_height() - 94.0).max(220.0);
                egui::ScrollArea::vertical()
                    .id_salt("library_view_form_content")
                    .max_height(form_scroll_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        widgets::card(ui, |ui| {
                            widgets::section_header(
                                ui,
                                "View details",
                                Some("Name the view and choose a destination outside every archive source."),
                            );
                            ui.label(egui::RichText::new("Name").strong());
                            let name_width = ui.available_width().max(180.0);
                            show_text_edit_with_context_menu(
                                ui,
                                &mut dialog.name,
                                clipboard,
                                |text_edit| {
                                    text_edit
                                        .id_salt("library_view_form_name")
                                        .desired_width(name_width)
                                },
                            );
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Destination").strong());
                            ui.horizontal(|ui| {
                                let destination_width = (ui.available_width() - 100.0).max(180.0);
                                show_text_edit_with_context_menu(
                                    ui,
                                    &mut dialog.destination_text,
                                    clipboard,
                                    |text_edit| {
                                        text_edit
                                            .id_salt("library_view_form_destination")
                                            .desired_width(destination_width)
                                    },
                                );
                                if ui.button("Browse...").clicked()
                                    && let Some(path) = rfd::FileDialog::new()
                                        .set_title("Select Library View Destination Folder")
                                        .pick_folder()
                                {
                                    dialog.destination_text = path.display().to_string();
                                    dialog.validation_message = None;
                                }
                            });
                        });
                        ui.add_space(8.0);

                        if library_view_selections_side_by_side(form_width) {
                            ui.columns(2, |columns| {
                                let (left, right) = columns.split_at_mut(1);
                                show_library_view_source_selection(
                                    &mut left[0],
                                    all_source_folders,
                                    &mut dialog.selected_source_folders,
                                );
                                show_library_view_platform_selection(
                                    &mut right[0],
                                    &mut dialog.selected_platforms,
                                );
                            });
                        } else {
                            show_library_view_source_selection(
                                ui,
                                all_source_folders,
                                &mut dialog.selected_source_folders,
                            );
                            ui.add_space(8.0);
                            show_library_view_platform_selection(
                                ui,
                                &mut dialog.selected_platforms,
                            );
                        }
                        ui.add_space(8.0);
                        widgets::banner(
                            ui,
                            "Layout",
                            "Creates {platform}/{filename}. No other layout is selected or implied.",
                            widgets::StatusTone::Info,
                        );
                        ui.add_space(8.0);
                        show_library_view_profile_selection(
                            ui,
                            &mut dialog.profile_kind,
                            &mut dialog.romm_overrides,
                            &mut dialog.romm_override_platform_input,
                            &mut dialog.romm_override_slug_input,
                            clipboard,
                        );
                    });

                ui.separator();
                if let Some(message) = &dialog.validation_message {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                let submit_blocker =
                    library_view_submit_blocker(&dialog.name, &dialog.destination_text, busy);
                if let Some(reason) = submit_blocker {
                    ui.weak(reason);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let submit_label = if dialog.editing_id.is_some() {
                        "Save"
                    } else {
                        "Add"
                    };
                    if ui
                        .add_enabled(submit_blocker.is_none(), egui::Button::new(submit_label))
                        .on_disabled_hover_text(submit_blocker.unwrap_or_default())
                        .clicked()
                    {
                        submit = true;
                    }
                    if widgets::action_button(
                        ui,
                        "Cancel",
                        widgets::ActionStyle::Quiet,
                        true,
                    )
                    .clicked()
                    {
                        cancel = true;
                    }
                    if submit_blocker.is_none() {
                        ui.label(
                            egui::RichText::new("Ready to validate and save")
                                .color(theme::muted(ui)),
                        );
                    }
                });
            });

        if submit {
            let name = dialog.name.trim().to_string();
            let destination_candidate = PathBuf::from(dialog.destination_text.trim());
            let all_source_paths: Vec<PathBuf> = all_source_folders
                .iter()
                .map(|source| source.path.clone())
                .collect();
            match validate_library_view_destination(&destination_candidate, &all_source_paths) {
                Ok(validated_destination) => {
                    let source_folders: Vec<PathBuf> =
                        dialog.selected_source_folders.iter().cloned().collect();
                    let platforms: Vec<String> =
                        dialog.selected_platforms.iter().cloned().collect();
                    let profile = library_view_form_profile(dialog);
                    action = Some(match &dialog.editing_id {
                        Some(identifier) => LibraryViewAction::Edit {
                            identifier: identifier.clone(),
                            name,
                            destination_root: validated_destination,
                            source_folders,
                            platforms,
                            profile,
                        },
                        None => LibraryViewAction::Add {
                            name,
                            destination_root: validated_destination,
                            source_folders,
                            platforms,
                            profile,
                        },
                    });
                }
                Err(error) => dialog.validation_message = Some(error.to_string()),
            }
        }
        if cancel || !open {
            *form_dialog = None;
        }
    }

    if let Some(dialog) = remove_dialog.clone() {
        let mut open = true;
        let mut confirmed = false;
        let mut cancel = false;
        let mut keep_definition = dialog.keep_definition;
        egui::Window::new("Remove this library view?")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.label(
                    "EmuWiz will remove only the managed symlinks recorded for this view. \
                     Original archive files are never touched.",
                );
                ui.add_space(4.0);
                ui.strong(&dialog.view_name);
                ui.add_space(4.0);
                ui.radio_value(
                    &mut keep_definition,
                    true,
                    "Keep the view's definition (recommended) - re-apply later to recreate its \
                     symlinks",
                );
                ui.radio_value(
                    &mut keep_definition,
                    false,
                    "Also remove the view's definition from configuration",
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Remove View"))
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if confirmed {
            action = Some(LibraryViewAction::Remove {
                identifier: dialog.view_id.clone(),
                keep_definition,
            });
            *remove_dialog = None;
        } else if cancel || !open {
            *remove_dialog = None;
        } else if let Some(current) = remove_dialog.as_mut() {
            current.keep_definition = keep_definition;
        }
    }

    action
}
pub(super) fn show_about_window(
    ctx: &egui::Context,
    open: &mut bool,
    database_state: &DatabaseState,
    diagnostics: &DiagnosticsState,
    mount_root: Option<&Path>,
    clipboard: &mut dyn ClipboardBackend,
) {
    egui::Window::new("About EmuWiz")
        .open(open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            show_about_contents(ui, database_state, diagnostics, mount_root, clipboard);
        });
}
pub(super) fn system_information_text(
    database_path: Option<&str>,
    config_path: Option<&str>,
    mount_root: Option<&str>,
) -> String {
    format!(
        "EmuWiz {}\nOS: {} ({})\nDesktop: {}\nSession: {}\nDatabase schema: v{}\nDatabase path: {}\nConfiguration path: {}\nMount root: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unset".to_string()),
        clipboard_environment_summary(),
        latest_schema_version(),
        database_path.unwrap_or("unknown"),
        config_path.unwrap_or("unknown"),
        mount_root.unwrap_or("not loaded"),
    )
}

pub(super) fn show_about_contents(
    ui: &mut egui::Ui,
    database_state: &DatabaseState,
    diagnostics: &DiagnosticsState,
    mount_root: Option<&Path>,
    clipboard: &mut dyn ClipboardBackend,
) {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::ABOUT,
        "EmuWiz",
        "A Linux archive library and safe mount manager.",
    );

    let database_path = database_state_path(database_state).map(|path| path.display().to_string());
    let config_path = match diagnostics {
        DiagnosticsState::Ready { report, .. } => report
            .config_path
            .as_ref()
            .map(|path| path.display().to_string()),
        _ => None,
    };
    let mount_root = mount_root.map(|path| path.display().to_string());

    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                format!("Version {}", env!("CARGO_PKG_VERSION")),
                widgets::StatusTone::Info,
            );
            ui.label(format!(
                "{} · {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ));
            ui.label(format!("Database schema v{}", latest_schema_version()));
        });
        ui.label(format!(
            "Desktop: {}",
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unset".to_string())
        ));
    });
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Runtime locations",
        Some("Paths are shown for troubleshooting and are not edited here."),
    );
    widgets::card(ui, |ui| {
        if let Some(path) = &database_path {
            if widgets::copyable_value(ui, "Database", path) {
                let _ = clipboard.set_text(path.clone());
            }
        } else {
            ui.label("Database: unknown");
        }
        if let Some(path) = &config_path {
            if widgets::copyable_value(ui, "Configuration", path) {
                let _ = clipboard.set_text(path.clone());
            }
        } else {
            ui.label("Configuration: unknown");
        }
        if let Some(path) = &mount_root {
            if widgets::copyable_value(ui, "Mount root", path) {
                let _ = clipboard.set_text(path.clone());
            }
        } else {
            ui.label("Mount root: not loaded");
        }
    });

    ui.add_space(12.0);
    if widgets::action_button(
        ui,
        "Copy system information",
        widgets::ActionStyle::Secondary,
        true,
    )
    .clicked()
    {
        let _ = clipboard.set_text(system_information_text(
            database_path.as_deref(),
            config_path.as_deref(),
            mount_root.as_deref(),
        ));
    }
}

/// The Mount page's mutable UI state, borrowed field-by-field from
/// `ArchiveFsApp` (same pattern as `HealthDashboardViewState`).
pub(super) struct MountPageViewState<'a> {
    pub(super) queue: &'a mut Vec<PathBuf>,
    pub(super) search: &'a mut String,
    pub(super) platform: &'a mut Option<String>,
    pub(super) confirm: &'a mut bool,
    pub(super) busy: bool,
    pub(super) block_reason: Option<&'a str>,
}
pub(super) fn show_mount_page(
    ui: &mut egui::Ui,
    live: Option<&LoadedData>,
    mount_all_result: Option<&MountAllResult>,
    view_state: MountPageViewState<'_>,
) -> Option<MountPageAction> {
    let MountPageViewState {
        queue,
        search,
        platform,
        confirm,
        busy,
        block_reason,
    } = view_state;
    let mut action = None;
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::MOUNT,
        "Mount",
        "Choose archives, review validated destinations, and mount the ready queue.",
    );
    if let Some(result) = mount_all_result {
        show_mount_all_result(ui, result);
        ui.separator();
    }
    let Some(data) = live else {
        ui.label("Live mount state is not loaded yet.");
        return None;
    };
    prune_mount_queue(queue, &data.records);
    let attempted = queued_pending_paths(queue, &data.records);

    let platform_counts = detected_platform_counts(
        data.records
            .iter()
            .map(|record| record.identity.platform.as_deref()),
    );
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(platform.is_none(), "All").clicked() {
            *platform = None;
        }
        for (candidate, count) in &platform_counts.named {
            if ui
                .selectable_label(
                    platform.as_deref() == Some(candidate.as_str()),
                    format!("{candidate} ({count})"),
                )
                .clicked()
            {
                *platform = Some(candidate.clone());
            }
        }
        if platform_counts.unknown > 0
            && ui
                .selectable_label(
                    platform.as_deref() == Some("Unknown"),
                    format!("Unknown ({})", platform_counts.unknown),
                )
                .clicked()
        {
            *platform = Some("Unknown".to_string());
        }
    });

    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Search");
            ui.add(egui::TextEdit::singleline(search).desired_width(240.0));
            if widgets::action_button(ui, "Clear", widgets::ActionStyle::Quiet, !search.is_empty())
                .clicked()
            {
                search.clear();
            }
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Secondary, !busy)
                .clicked()
            {
                action = Some(MountPageAction::Refresh);
            }
        })
        .inner
    });
    ui.add_space(4.0);

    let visible: Vec<&ArchiveRecord> = data
        .records
        .iter()
        .filter(|record| {
            platform.as_deref().is_none_or(|wanted| {
                record.identity.platform.as_deref().unwrap_or("Unknown") == wanted
            }) && mount_row_matches(record, search)
        })
        .collect();

    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if queue.len() == 1 {
                ui.label("1 archive queued.");
            } else {
                ui.label(format!("{} archives queued.", queue.len()));
            }
            let any_visible_unqueued_pending = visible.iter().any(|record| {
                record.mount_state == MountState::Pending
                    && !queue.contains(&record.mount_plan.archive.path)
            });
            if widgets::action_button(
                ui,
                "Queue all visible",
                widgets::ActionStyle::Secondary,
                any_visible_unqueued_pending,
            )
            .clicked()
            {
                for record in &visible {
                    if record.mount_state == MountState::Pending
                        && !queue.contains(&record.mount_plan.archive.path)
                    {
                        queue.push(record.mount_plan.archive.path.clone());
                    }
                }
            }
            if widgets::action_button(
                ui,
                "Clear queue",
                widgets::ActionStyle::Quiet,
                !queue.is_empty(),
            )
            .clicked()
            {
                queue.clear();
                *confirm = false;
            }
            let mount_enabled = !busy && !attempted.is_empty() && !*confirm;
            if widgets::action_button(
                ui,
                format!("Mount queue ({})", attempted.len()),
                widgets::ActionStyle::Primary,
                mount_enabled,
            )
            .clicked()
            {
                *confirm = true;
            }
        })
        .inner
    });
    if busy && let Some(reason) = block_reason {
        ui.label(reason);
    }

    if *confirm {
        match show_mount_queue_confirmation(ui, attempted.len(), busy) {
            Some(QueueConfirmChoice::Mount) => {
                action = Some(MountPageAction::MountQueue);
                *confirm = false;
            }
            Some(QueueConfirmChoice::Cancel) => *confirm = false,
            None => {}
        }
    }
    ui.separator();

    if data.records.is_empty() {
        widgets::empty_state(
            ui,
            "No archives found",
            "Add and scan a source folder before creating a mount queue.",
            None,
        );
        return action;
    }
    if visible.is_empty() {
        widgets::empty_state(
            ui,
            "No matching archives",
            "Change or clear the search to see available archives.",
            None,
        );
        return action;
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut toggle: Option<(PathBuf, bool)> = None;
            for record in &visible {
                let path = &record.mount_plan.archive.path;
                let queued = queue.contains(path);
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(&record.identity.display_name)
                                .size(17.0)
                                .strong(),
                        );
                        widgets::status_badge(
                            ui,
                            record.identity.platform.as_deref().unwrap_or("Unknown"),
                            widgets::StatusTone::Info,
                        );
                        let tone = match record.mount_state {
                            MountState::Pending => widgets::StatusTone::Success,
                            MountState::Mounted => widgets::StatusTone::Active,
                            MountState::MountPathExists => widgets::StatusTone::Blocked,
                            MountState::NotMountable => widgets::StatusTone::Info,
                        };
                        widgets::status_badge(ui, mount_validation_label(record.mount_state), tone);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if queued { "Remove" } else { "Add to queue" };
                            if widgets::action_button(
                                ui,
                                label,
                                if queued {
                                    widgets::ActionStyle::Quiet
                                } else {
                                    widgets::ActionStyle::Secondary
                                },
                                queued || record.mount_state == MountState::Pending,
                            )
                            .clicked()
                            {
                                toggle = Some((path.clone(), !queued));
                            }
                        });
                    });
                    if widgets::path_value(ui, "Destination", &record.mount_plan.mount_path) {
                        ui.ctx()
                            .copy_text(record.mount_plan.mount_path.display().to_string());
                    }
                    widgets::technical_details(ui, ("mount_queue_archive_location", path), |ui| {
                        if widgets::path_value(ui, "Archive", path) {
                            ui.ctx().copy_text(path.display().to_string());
                        }
                    });
                });
                ui.add_space(6.0);
            }
            if let Some((path, add)) = toggle {
                if add {
                    if !queue.contains(&path) {
                        queue.push(path);
                    }
                } else {
                    queue.retain(|queued_path| *queued_path != path);
                }
            }
        });
    action
}

/// What the Active Mounts page asks `update` to do. `Unmount` is only
/// ever returned after the page's own inline confirmation, and is then
/// routed through the exact `AppOperationRequest::Archive` /
/// `start_operation` path the Library's selected-archive panel uses.
pub(super) enum ActiveMountsPageAction {
    Unmount(PathBuf),
    OpenInLibrary(PathBuf),
    Refresh,
}
pub(super) fn show_active_mounts_page(
    ui: &mut egui::Ui,
    live_records: Option<&[ArchiveRecord]>,
    confirm_unmount: &mut Option<PathBuf>,
    cleanup_after_unmount: &mut bool,
    feedback: Option<&ActionFeedback>,
    busy: bool,
) -> Option<ActiveMountsPageAction> {
    let mut action = None;
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::MOUNT,
        "Active Mounts",
        "Review current mounts and unmount them normally after closing applications that use them.",
    );
    if let Some(feedback) = feedback {
        widgets::banner(
            ui,
            if feedback.succeeded {
                "Completed"
            } else {
                "Failed"
            },
            &feedback.message,
            if feedback.succeeded {
                widgets::StatusTone::Success
            } else {
                widgets::StatusTone::Blocked
            },
        );
    }
    let Some(records) = live_records else {
        ui.label("Live mount state is not loaded yet.");
        return None;
    };
    let mounted = pending_unmount_items(records);
    // A confirmation for an archive that is no longer mounted (unmounted
    // meanwhile, snapshot refreshed) must not survive as a stale prompt.
    if let Some(pending) = confirm_unmount.as_ref()
        && !mounted.iter().any(|item| item.archive_path == *pending)
    {
        *confirm_unmount = None;
    }
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                format!("{} active", mounted.len()),
                if mounted.is_empty() {
                    widgets::StatusTone::Pending
                } else {
                    widgets::StatusTone::Active
                },
            );
            if mounted.len() == 1 {
                ui.label("1 mounted archive.");
            } else {
                ui.label(format!("{} mounted archives.", mounted.len()));
            }
            if widgets::action_button(ui, "Refresh", widgets::ActionStyle::Secondary, !busy)
                .clicked()
            {
                action = Some(ActiveMountsPageAction::Refresh);
            }
        })
        .inner
    });
    if mounted.is_empty() {
        widgets::empty_state(
            ui,
            "Nothing is mounted",
            "Mounted archives will appear here with their destinations and normal unmount controls.",
            None,
        );
        return action;
    }
    ui.add_enabled_ui(!busy, |ui| {
        ui.checkbox(
            cleanup_after_unmount,
            "Clean empty mount directories after unmount",
        );
    });
    if let Some(pending) = confirm_unmount.clone() {
        let name = mounted
            .iter()
            .find(|item| item.archive_path == pending)
            .map(|item| item.display_name.as_str())
            .unwrap_or("this archive");
        widgets::card(ui, |ui| {
            widgets::status_badge(ui, "Confirmation", widgets::StatusTone::Warning);
            ui.strong(format!("Unmount {name}?"));
            ui.label("Close applications using this mount before unmounting.");
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    "Unmount now",
                    widgets::ActionStyle::Destructive,
                    !busy,
                )
                .clicked()
                {
                    action = Some(ActiveMountsPageAction::Unmount(pending.clone()));
                    *confirm_unmount = None;
                }
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked()
                {
                    *confirm_unmount = None;
                }
            });
        });
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for item in &mounted {
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(&item.display_name).size(17.0).strong());
                        widgets::status_badge(ui, "Mounted", widgets::StatusTone::Active);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::action_button(
                                ui,
                                "Unmount",
                                widgets::ActionStyle::Destructive,
                                !busy,
                            )
                            .clicked()
                            {
                                *confirm_unmount = Some(item.archive_path.clone());
                            }
                            if widgets::action_button(
                                ui,
                                "Open in Library",
                                widgets::ActionStyle::Quiet,
                                true,
                            )
                            .clicked()
                            {
                                action = Some(ActiveMountsPageAction::OpenInLibrary(
                                    item.archive_path.clone(),
                                ));
                            }
                        });
                    });
                    if widgets::path_value(ui, "Destination", &item.mount_path) {
                        ui.ctx().copy_text(item.mount_path.display().to_string());
                    }
                    if widgets::path_value(ui, "Archive", &item.archive_path) {
                        ui.ctx().copy_text(item.archive_path.display().to_string());
                    }
                });
                ui.add_space(6.0);
            }
        });
    action
}

/// Active Mounts' compact "Recent activity" - reuses
/// `widgets::activity_row_header`, the same shared row-header component
/// `show_sources_recent_activity` and `show_recent_cheat_activity` already
/// use, scoped to the `ActivityAction` variants a mount/unmount user would
/// recognise as theirs. No new row-rendering logic, only a new filter over
/// the same `OperationHistory` every other activity surface already reads.
/// No "view full history" link, matching the same precedent; full History
/// & Logs remains reachable from the sidebar as always.
pub(super) fn show_active_mounts_recent_activity(ui: &mut egui::Ui, history: &OperationHistory) {
    let entries: Vec<&HistoryEntry> = history
        .entries()
        .filter(|entry| {
            matches!(
                entry.action,
                ActivityAction::Mount
                    | ActivityAction::MountAll
                    | ActivityAction::Unmount
                    | ActivityAction::UnmountAll
                    | ActivityAction::LazyUnmount
                    | ActivityAction::Remount
                    | ActivityAction::Cleanup
            )
        })
        .take(5)
        .collect();
    widgets::section_header(
        ui,
        "Recent activity",
        Some("A compact view of this session's mount and unmount changes."),
    );
    if entries.is_empty() {
        ui.weak("No mount or unmount activity has been recorded in this session.");
        return;
    }
    for entry in entries {
        widgets::card(ui, |ui| {
            widgets::activity_row_header(
                ui,
                entry.outcome.to_string(),
                activity_outcome_tone(entry.outcome),
                entry.action.to_string(),
                Some(&format_history_timestamp(entry.timestamp)),
                |_ui| {},
            );
            ui.add(egui::Label::new(&entry.message).truncate())
                .on_hover_text(&entry.message);
        });
        ui.add_space(4.0);
    }
}

/// The shared rollback preview/review/confirm/apply/result card, reused
/// verbatim (extracted, not duplicated - see
/// docs/GUI_NAVIGATION_RESET_DESIGN.md's Undo requirement) by both
/// History & Logs and Gamer View's "Undo last change" so there is exactly
/// one rendering of this state machine, not a second copy that could
/// drift from it.
pub(super) fn show_shared_rollback_card(
    ui: &mut egui::Ui,
    rollback: &mut SharedRollbackState,
) -> Option<HistoryPageAction> {
    let mut action = None;
    widgets::card(ui, |ui| match rollback {
        SharedRollbackState::Idle => {
            ui.label("Rollback always begins with a fresh read-only preview and a separate confirmation.");
        }
        SharedRollbackState::Previewing { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Inspecting rollback state off the UI thread…");
            });
        }
        SharedRollbackState::Review { preview, .. } => {
            widgets::status_badge(
                ui,
                if preview.available {
                    "Rollback available"
                } else {
                    "Rollback blocked"
                },
                if preview.available {
                    widgets::StatusTone::Warning
                } else {
                    widgets::StatusTone::Blocked
                },
            );
            widgets::copyable_value(ui, "Rollback preview ID", &preview.preview_id);
            for entry in &preview.entries {
                ui.label(format!("Current state: {:?}", entry.outcome));
                if let Some(destination) = &entry.destination {
                    ui.label(format!("Destination: {}", destination.display));
                }
                if let Some(backup) = &entry.backup {
                    ui.label(format!("Backup: {}", backup.display));
                }
                if let Some(failure) = &entry.failure {
                    ui.label(format!("Blocker: {:?} · {}", failure.kind, failure.detail));
                }
            }
            ui.horizontal_wrapped(|ui| {
                if widgets::action_button(
                    ui,
                    "Confirm exact rollback",
                    widgets::ActionStyle::Primary,
                    preview.available,
                )
                .clicked()
                {
                    action = Some(HistoryPageAction::ConfirmRollback);
                }
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked()
                {
                    action = Some(HistoryPageAction::CancelRollback);
                }
            });
        }
        SharedRollbackState::Applying { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Restoring the previous files and checking the result…");
            });
        }
        SharedRollbackState::Result(result) => {
            widgets::status_badge(
                ui,
                format!("Rollback {:?}", result.status),
                if result.status == SharedApplyStatus::Success {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Warning
                },
            );
            for entry in &result.preview.entries {
                ui.label(format!("Rollback result: {:?}", entry.outcome));
            }
            if widgets::action_button(ui, "Close result", widgets::ActionStyle::Quiet, true)
                .clicked()
            {
                action = Some(HistoryPageAction::CancelRollback);
            }
        }
        SharedRollbackState::Failed(message) => {
            widgets::banner(ui, "Rollback failed", message, widgets::StatusTone::Blocked);
            if widgets::action_button(ui, "Close", widgets::ActionStyle::Quiet, true).clicked() {
                action = Some(HistoryPageAction::CancelRollback);
            }
        }
    });
    action
}

pub(super) fn shared_history_adapter_label(adapter: PreviewAdapter) -> &'static str {
    match adapter {
        PreviewAdapter::RetroArch => "RetroArch",
        PreviewAdapter::Pcsx2 => "PCSX2",
        PreviewAdapter::Dolphin => "Dolphin",
        PreviewAdapter::Xenia => "Xenia",
    }
}

pub(super) fn shared_history_status_label(status: SharedApplyStatus) -> &'static str {
    match status {
        SharedApplyStatus::DryRun => "Preview",
        SharedApplyStatus::Success => "Completed",
        SharedApplyStatus::PartialFailure => "Needs attention",
        SharedApplyStatus::Failed => "Failed",
    }
}

pub(super) fn shared_history_game_title(
    journal: &archivefs_core::patch_manager::SharedApplyJournal,
) -> String {
    journal
        .context
        .selected_archive
        .to_path_buf()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "selected game".to_string())
}

pub(super) fn shared_history_title(
    journal: &archivefs_core::patch_manager::SharedApplyJournal,
) -> String {
    let game = shared_history_game_title(journal);
    match (journal.status, journal.context.adapter) {
        (SharedApplyStatus::DryRun, _) => format!("Changes previewed for {game}"),
        (_, PreviewAdapter::Xenia) => format!("Patches added to {game}"),
        _ => format!("Cheats added to {game}"),
    }
}

pub(super) fn short_month(month: time::Month) -> &'static str {
    match month {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    }
}

/// Friendly local wall-clock display for a saved change. The raw Unix value
/// remains under Technical details for audit work.
pub(super) fn format_shared_history_time(timestamp: u64, now: SystemTime) -> String {
    let Ok(timestamp) = i64::try_from(timestamp) else {
        return "Time unavailable".to_string();
    };
    let Ok(utc) = time::OffsetDateTime::from_unix_timestamp(timestamp) else {
        return "Time unavailable".to_string();
    };
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    let local = utc.to_offset(offset);
    let now_seconds = now
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
        .map(|value| value.to_offset(offset));
    if now_seconds.is_some_and(|value| value.date() == local.date()) {
        format!("Today at {:02}:{:02}", local.hour(), local.minute())
    } else {
        format!(
            "{} {} {} at {:02}:{:02}",
            local.day(),
            short_month(local.month()),
            local.year(),
            local.hour(),
            local.minute()
        )
    }
}

/// The most "Session activity" cards drawn in one frame. Copy/Export still
/// act on every filtered entry regardless of this cap - it only bounds how
/// many cards are laid out at once, matching the 200-journal cap the
/// "Changes you can undo" section above already uses.
const HISTORY_ACTIVITY_RENDER_CAP: usize = 200;

pub(super) fn show_history_logs_page(
    ui: &mut egui::Ui,
    shared_history: &SharedHistoryState,
    rollback: &mut SharedRollbackState,
    selected_operation: Option<&str>,
    history: &mut OperationHistory,
    filters: &mut HistoryLogFilters,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<HistoryPageAction> {
    let mut action = None;
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::HISTORY,
        "History & Logs",
        "Filter, inspect, copy, or export operations from this application session.",
    );
    widgets::section_header(
        ui,
        "Recovery",
        Some("Undo the most recent cheat or patch changes if something went wrong."),
    );
    if let Some(rollback_action) = show_shared_rollback_card(ui, rollback) {
        action = Some(rollback_action);
    }
    widgets::section_header(
        ui,
        "Changes you can undo",
        Some("Recent cheat and patch changes, with rollback available when safe."),
    );
    if widgets::action_button(
        ui,
        "Refresh change history",
        widgets::ActionStyle::Secondary,
        !matches!(shared_history, SharedHistoryState::Loading { .. }),
    )
    .clicked()
    {
        action = Some(HistoryPageAction::Refresh);
    }
    match shared_history {
        SharedHistoryState::NotLoaded | SharedHistoryState::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading change history…");
            });
        }
        SharedHistoryState::Failed(message) => widgets::banner(
            ui,
            "Change history unavailable",
            message,
            widgets::StatusTone::Warning,
        ),
        SharedHistoryState::Ready(report) => {
            if !report.warnings.is_empty() {
                widgets::banner(
                    ui,
                    "Some history details could not be read",
                    &format!(
                        "{} saved record(s) could not be opened. Other changes remain available.",
                        report.warnings.len()
                    ),
                    widgets::StatusTone::Warning,
                );
            }
            if report.journals.is_empty() {
                ui.label("No undoable changes found.");
            } else {
                let mut journals: Vec<_> = report.journals.iter().collect();
                journals.sort_by(|left, right| {
                    right
                        .1
                        .timestamp_unix_seconds
                        .cmp(&left.1.timestamp_unix_seconds)
                        .then_with(|| right.1.operation_id.cmp(&left.1.operation_id))
                });
                egui::CollapsingHeader::new(format!(
                    "Saved change history ({})",
                    report.journals.len()
                ))
                .id_salt("history-saved-changes")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label("Expand to review saved changes and rollback previews.");
                    ui.add_space(4.0);
                    for (path, journal) in journals.into_iter().take(200) {
                        widgets::card(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                widgets::status_badge(
                                    ui,
                                    shared_history_status_label(journal.status),
                                    match journal.status {
                                        SharedApplyStatus::Success => widgets::StatusTone::Success,
                                        SharedApplyStatus::PartialFailure => {
                                            widgets::StatusTone::Warning
                                        }
                                        SharedApplyStatus::Failed => widgets::StatusTone::Blocked,
                                        SharedApplyStatus::DryRun => widgets::StatusTone::Info,
                                    },
                                );
                                ui.strong(shared_history_title(journal));
                                if selected_operation == Some(journal.operation_id.as_str()) {
                                    widgets::status_badge(
                                        ui,
                                        "Opened from apply result",
                                        widgets::StatusTone::Info,
                                    );
                                }
                            });
                            ui.label(format!(
                                "{} · {}",
                                shared_history_adapter_label(journal.context.adapter),
                                format_shared_history_time(
                                    journal.timestamp_unix_seconds,
                                    SystemTime::now()
                                )
                            ));
                            ui.label(format!(
                                "{} change{}",
                                journal.entries.len(),
                                if journal.entries.len() == 1 { "" } else { "s" }
                            ));
                            ui.label(format!(
                                "Rollback: {}",
                                if journal.rollback_operation_id.is_some() {
                                    "already completed"
                                } else if journal.status == SharedApplyStatus::Success {
                                    "preview may be available"
                                } else {
                                    "unavailable"
                                }
                            ));
                            widgets::technical_details(
                                ui,
                                ("journal_technical_detail", &journal.plan_id),
                                |ui| {
                                    widgets::copyable_value(
                                        ui,
                                        "Transaction ID",
                                        &journal.operation_id,
                                    );
                                    widgets::copyable_value(ui, "Plan ID", &journal.plan_id);
                                    ui.label(format!(
                                        "Raw timestamp: {}",
                                        journal.timestamp_unix_seconds
                                    ));
                                    widgets::copyable_value(
                                        ui,
                                        "Selected archive",
                                        &journal.context.selected_archive.display,
                                    );
                                    widgets::copyable_value(
                                        ui,
                                        "Source mode",
                                        &journal.context.source_mode,
                                    );
                                    widgets::copyable_value(
                                        ui,
                                        "Destination root",
                                        &journal.destination_root.display,
                                    );
                                    if widgets::path_value(
                                        ui,
                                        "Journal path",
                                        &PathBuf::from(&path.display),
                                    ) {
                                        let _ = clipboard.set_text(path.display.clone());
                                    }
                                    for entry in &journal.entries {
                                        ui.label(format!(
                                            "{:?} · {} · verification {} · backup {}",
                                            entry.outcome,
                                            entry.plan_entry.destination_relative_path.display,
                                            if entry.verification_succeeded {
                                                "passed"
                                            } else {
                                                "not complete"
                                            },
                                            if entry.backup_path.is_some() {
                                                "retained"
                                            } else {
                                                "not required"
                                            }
                                        ));
                                    }
                                },
                            );
                            let journal_path = path.to_path_buf().ok();
                            let destination_root = journal.destination_root.to_path_buf().ok();
                            let can_preview = journal.status == SharedApplyStatus::Success
                                && journal.rollback_operation_id.is_none()
                                && journal_path.is_some()
                                && destination_root.is_some()
                                && matches!(rollback, SharedRollbackState::Idle);
                            if widgets::action_button(
                                ui,
                                "Preview rollback",
                                widgets::ActionStyle::Secondary,
                                can_preview,
                            )
                            .clicked()
                            {
                                action = Some(HistoryPageAction::PreviewRollback {
                                    journal_path: journal_path.expect("enabled path"),
                                    destination_root: destination_root.expect("enabled root"),
                                });
                            }
                        });
                    }
                });
            }
        }
    }
    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Session activity",
        Some("This session's recorded operations - filter, copy, or export what's shown below."),
    );
    if history.entries().next().is_none() {
        widgets::empty_state(
            ui,
            "No session activity yet",
            "Mounts, scans, diagnostics, and trusted-source retrievals will be recorded here for this session.",
            None,
        );
        return action;
    }

    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut filters.text_query)
                    .hint_text("message, operation, or result")
                    .desired_width(200.0),
            );
            egui::ComboBox::from_label("Operation")
                .selected_text(
                    filters
                        .action
                        .map_or_else(|| "All Operations".to_string(), |action| action.to_string()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filters.action, None, "All Operations");
                    for action in ALL_ACTIVITY_ACTIONS {
                        ui.selectable_value(&mut filters.action, Some(action), action.to_string());
                    }
                });
            egui::ComboBox::from_label("Result")
                .selected_text(
                    filters
                        .outcome
                        .map_or_else(|| "All Results".to_string(), |outcome| outcome.to_string()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filters.outcome, None, "All Results");
                    for outcome in ALL_ACTIVITY_OUTCOMES {
                        ui.selectable_value(
                            &mut filters.outcome,
                            Some(outcome),
                            outcome.to_string(),
                        );
                    }
                });
            let sort_label = if filters.oldest_first {
                "Sort: Oldest First"
            } else {
                "Sort: Newest First"
            };
            if widgets::action_button(ui, sort_label, widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                filters.oldest_first = !filters.oldest_first;
            }
            if widgets::action_button(
                ui,
                "Clear filters",
                widgets::ActionStyle::Quiet,
                filters.action.is_some()
                    || filters.outcome.is_some()
                    || !filters.text_query.is_empty(),
            )
            .clicked()
            {
                filters.action = None;
                filters.outcome = None;
                filters.text_query.clear();
            }
        })
        .inner
    });

    // Owned copies end the immutable borrow of `history` before the
    // buttons below may mutate it (clear / record an export entry).
    let visible_entries: Vec<HistoryEntry> = visible_history_entries(history, filters)
        .into_iter()
        .cloned()
        .collect();
    let visible_texts: Vec<String> = visible_entries.iter().map(history_entry_text).collect();
    let total = history.entries().count();

    let mut export_requested = false;
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if visible_texts.len() == total {
                if total == 1 {
                    ui.label("1 entry.".to_string());
                } else {
                    ui.label(format!("{total} entries."));
                }
            } else {
                ui.label(format!("{} of {total} entries shown.", visible_texts.len()));
            }
            if widgets::action_button(
                ui,
                "Copy visible log",
                widgets::ActionStyle::Secondary,
                !visible_texts.is_empty(),
            )
            .clicked()
            {
                let _ = clipboard.set_text(visible_texts.join("\n"));
            }
            if widgets::action_button(
                ui,
                "Export log",
                widgets::ActionStyle::Secondary,
                !visible_texts.is_empty(),
            )
            .clicked()
            {
                export_requested = true;
            }
            if widgets::action_button(
                ui,
                "Clear history",
                widgets::ActionStyle::Destructive,
                !visible_texts.is_empty(),
            )
            .clicked()
            {
                history.clear();
            }
        })
        .inner
    });
    if export_requested
        && let Some(path) = rfd::FileDialog::new()
            .set_file_name("archivefs-operations-log.txt")
            .save_file()
    {
        match std::fs::write(&path, visible_texts.join("\n")) {
            Ok(()) => history.record(HistoryEntry::new(
                ActivityAction::LogExport,
                None,
                ActivityOutcome::Completed,
                format!(
                    "Exported {} log entries to {}",
                    visible_texts.len(),
                    path.display()
                ),
            )),
            Err(error) => history.record(HistoryEntry::new(
                ActivityAction::LogExport,
                None,
                ActivityOutcome::Failed,
                format!("Could not export log to {}: {error}", path.display()),
            )),
        }
    }
    ui.separator();

    if visible_texts.is_empty() {
        widgets::empty_state(
            ui,
            "No matching activity",
            "Change or clear the filters to see session events.",
            None,
        );
        return action;
    }
    // Copy/Export above already act on the full filtered set regardless of
    // this cap - only the on-screen list is bounded, so a very long session
    // never has to lay out thousands of cards per frame.
    if visible_texts.len() > HISTORY_ACTIVITY_RENDER_CAP {
        ui.label(
            egui::RichText::new(format!(
                "Showing the first {HISTORY_ACTIVITY_RENDER_CAP} of {} matching entries in the \
                 current sort order. Narrow the filters or search to see the rest.",
                visible_texts.len()
            ))
            .color(theme::muted(ui))
            .small(),
        );
        ui.add_space(4.0);
    }
    egui::CollapsingHeader::new(format!("Session entries ({})", visible_texts.len()))
        .id_salt("history-session-activity")
        .default_open(false)
        .show(ui, |ui| {
            ui.label("Expand to review the recorded operation cards.");
            ui.add_space(4.0);
            for (row_index, (entry, text)) in visible_entries
                .iter()
                .zip(&visible_texts)
                .enumerate()
                .take(HISTORY_ACTIVITY_RENDER_CAP)
            {
                widgets::card(ui, |ui| {
                    widgets::activity_row_header(
                        ui,
                        entry.outcome.to_string(),
                        activity_outcome_tone(entry.outcome),
                        entry.action.to_string(),
                        Some(&format_history_timestamp(entry.timestamp)),
                        |ui| {
                            if widgets::action_button(ui, "Copy", widgets::ActionStyle::Quiet, true)
                                .clicked()
                            {
                                let _ = clipboard.set_text(text.clone());
                            }
                        },
                    );
                    ui.add(egui::Label::new(&entry.message).selectable(true).wrap());
                    if let Some(path) = &entry.archive_path {
                        // Root cause of the "First use of widget ID .../Second use
                        // of widget ID ..." egui warning in this list: the salt
                        // used to be `("history_related_archive", path)` alone, so
                        // any two entries referencing the same archive (e.g. a
                        // mount followed by an unmount of the same file) collided
                        // on an identical `CollapsingHeader` ID. `row_index` is
                        // this render's own guaranteed-unique per-row discriminator
                        // (unlike the path, or `entry.timestamp`, which is not
                        // guaranteed unique at typical `SystemTime` resolution),
                        // so every row gets a distinct, stable-for-this-frame ID
                        // regardless of how many entries share the same archive.
                        widgets::technical_details(
                            ui,
                            ("history_related_archive", row_index, path),
                            |ui| {
                                if widgets::path_value(ui, "Archive", path) {
                                    let _ = clipboard.set_text(path.display().to_string());
                                }
                            },
                        );
                    }
                });
                ui.add_space(6.0);
            }
        });
    action
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show_settings_page(
    ui: &mut egui::Ui,
    database_state: &DatabaseState,
    diagnostics: &DiagnosticsState,
    retroarch_profiles: &RetroArchProfilesState,
    mount_root: Option<&Path>,
    busy: bool,
    clipboard: &mut dyn ClipboardBackend,
    custom_artwork_directory: Option<&Path>,
    artwork_cache: &mut PlatformArtworkCache,
    artwork_manager: &mut PlatformArtworkManagerState,
) -> Option<SettingsPageAction> {
    let mut action = None;
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::SETTINGS,
        "Settings",
        "Set up EmuWiz: locations, configuration, and integrations.",
    );

    let database_path = database_state_path(database_state).map(|path| path.display().to_string());
    let config_path = match diagnostics {
        DiagnosticsState::Ready { report, .. } => report
            .config_path
            .as_ref()
            .map(|path| path.display().to_string()),
        _ => None,
    };
    let mount_root_text = mount_root.map(|path| path.display().to_string());

    widgets::section_header(
        ui,
        "1. EmuWiz locations",
        Some("Full paths remain available through hover and Copy without dominating the page."),
    );
    widgets::card(ui, |ui| {
        if let Some(path) = &config_path {
            if widgets::copyable_value(ui, "Configuration", path) {
                let _ = clipboard.set_text(path.clone());
            }
        } else {
            ui.label("Configuration: unknown");
        }
        if let Some(path) = &database_path {
            if widgets::copyable_value(ui, "Database", path) {
                let _ = clipboard.set_text(path.clone());
            }
        } else {
            ui.label("Database: unknown");
        }
        if let Some(path) = mount_root {
            if widgets::path_value(ui, "Mount root", path) {
                let _ = clipboard.set_text(path.display().to_string());
            }
        } else {
            ui.label("Mount root: not loaded");
        }
        ui.label(egui::RichText::new("Mount-root changes apply to new mounts; existing mounts keep their current destinations until remounted.").color(theme::muted(ui)));
    });

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "2. Configuration validation",
        Some("EmuWiz validates the active configuration without changing it."),
    );
    let (validation_title, validation_detail, validation_tone) = match diagnostics {
        DiagnosticsState::Ready { report, .. } if diagnostics_can_continue(report) => (
            "Ready",
            "No blocking configuration issues were found.",
            widgets::StatusTone::Success,
        ),
        DiagnosticsState::Ready { .. } => (
            "Needs attention",
            "Configuration issues were found. Open diagnostics for the complete checks.",
            widgets::StatusTone::Warning,
        ),
        DiagnosticsState::Loading { .. } => (
            "Checking",
            "Configuration validation is running in the background.",
            widgets::StatusTone::Active,
        ),
        DiagnosticsState::Error { message, .. } => {
            ("Failed", message.as_str(), widgets::StatusTone::Blocked)
        }
    };
    widgets::banner(ui, validation_title, validation_detail, validation_tone);
    ui.horizontal_wrapped(|ui| {
        if widgets::action_button(
            ui,
            "Validate configuration",
            widgets::ActionStyle::Primary,
            !busy,
        )
        .clicked()
        {
            action = Some(SettingsPageAction::ValidateConfiguration);
        }
        if widgets::action_button(
            ui,
            "Open diagnostics",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            action = Some(SettingsPageAction::OpenDiagnostics);
        }
        if widgets::action_button(
            ui,
            "Open configuration folder",
            widgets::ActionStyle::Quiet,
            !busy,
        )
        .clicked()
        {
            action = Some(SettingsPageAction::OpenConfigFolder);
        }
    });

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "3. RetroArch integration",
        Some(
            "Eligible profiles are emphasised; blocked discoveries remain available for inspection.",
        ),
    );
    match retroarch_profiles {
        RetroArchProfilesState::NotScanned => {
            if widgets::empty_state(
                ui,
                "Profiles have not been scanned",
                "Scan supported RetroArch locations to find a usable cheat destination.",
                Some("Scan for profiles"),
            ) && !busy
            {
                action = Some(SettingsPageAction::RescanRetroArchProfiles);
            }
        }
        RetroArchProfilesState::Scanning { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Scanning for RetroArch profiles...");
            });
        }
        RetroArchProfilesState::Error(message) => {
            widgets::banner(
                ui,
                "Profile discovery failed",
                message,
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Rescan profiles", widgets::ActionStyle::Primary, !busy)
                .clicked()
            {
                action = Some(SettingsPageAction::RescanRetroArchProfiles);
            }
        }
        RetroArchProfilesState::Ready(discovery) => {
            if discovery.profiles.is_empty() {
                widgets::empty_state(
                    ui,
                    "No RetroArch profiles found",
                    "No supported installation was discovered in the current environment.",
                    None,
                );
            } else {
                for eligible in [true, false] {
                    for profile in discovery
                        .profiles
                        .iter()
                        .filter(|profile| profile.eligible == eligible)
                    {
                        widgets::card(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                widgets::status_badge(
                                    ui,
                                    if profile.eligible {
                                        "Eligible"
                                    } else {
                                        "Blocked"
                                    },
                                    profile_presentation_tone(profile.eligible),
                                );
                                ui.label(
                                    egui::RichText::new(&profile.profile_id).size(17.0).strong(),
                                );
                                ui.label(format!(
                                    "{} · {}",
                                    profile_kind_label(&profile.installation_type),
                                    profile_scope_label(&profile.scope)
                                ));
                            });
                            if widgets::copyable_value(
                                ui,
                                "Configuration",
                                &profile.configuration_path.display,
                            ) {
                                let _ =
                                    clipboard.set_text(profile.configuration_path.display.clone());
                            }
                            if let Some(destination) = &profile.cheat_destination_root {
                                if widgets::copyable_value(
                                    ui,
                                    "Cheat destination",
                                    &destination.display,
                                ) {
                                    let _ = clipboard.set_text(destination.display.clone());
                                }
                            } else {
                                ui.label("Cheat destination: unresolved");
                            }
                            if !profile.blockers.is_empty() {
                                ui.label(format!(
                                    "{} blocker{}",
                                    profile.blockers.len(),
                                    if profile.blockers.len() == 1 { "" } else { "s" }
                                ));
                                widgets::technical_details(
                                    ui,
                                    ("retroarch_profile_technical_blockers", &profile.profile_id),
                                    |ui| {
                                        for blocker in &profile.blockers {
                                            ui.label(format!(
                                                "{} — {}",
                                                blocker.code, blocker.detail
                                            ));
                                        }
                                    },
                                );
                            }
                        });
                        ui.add_space(6.0);
                    }
                }
            }
            if widgets::action_button(
                ui,
                "Rescan profiles",
                widgets::ActionStyle::Secondary,
                !busy,
            )
            .clicked()
            {
                action = Some(SettingsPageAction::RescanRetroArchProfiles);
            }
        }
    }

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "4. Diagnostics and environment report",
        Some("Copy a concise environment report for troubleshooting."),
    );
    widgets::card(ui, |ui| {
        if widgets::action_button(
            ui,
            "Copy environment report",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            let _ = clipboard.set_text(system_information_text(
                database_path.as_deref(),
                config_path.as_deref(),
                mount_root_text.as_deref(),
            ));
        }
    });

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        &crate::ui::icons::with_icon(crate::ui::icons::ARTWORK, "5. Platform artwork"),
        Some(
            "Manage local, upgrade-stable artwork overrides. EmuWiz never identifies a machine \
             from its picture: choose the canonical platform explicitly. Imports are normalised \
             off the UI thread and never overwrite the selected original.",
        ),
    );
    show_platform_artwork_manager(
        ui,
        custom_artwork_directory,
        artwork_cache,
        artwork_manager,
        &mut action,
    );

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(ui, "6. More settings coming later", None);
    widgets::banner(
        ui,
        "More settings coming later",
        "Appearance and maintenance options will be added here in a future update.",
        widgets::StatusTone::Info,
    );
    action
}

pub(super) fn history_entry_text(entry: &HistoryEntry) -> String {
    let archive = entry
        .archive_path
        .as_deref()
        .map(|path| format!(" · {}", path.display()))
        .unwrap_or_default();
    format!(
        "[{}] {} · {}{} — {}",
        format_history_timestamp(entry.timestamp),
        entry.action,
        entry.outcome,
        archive,
        entry.message
    )
}

pub(super) fn format_history_timestamp(timestamp: SystemTime) -> String {
    let seconds = timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    )
}

/// Which "Library Database" panel button (requirement 3) was clicked.
/// `RefreshStatus` and `RetryLoad` both trigger the same underlying
/// read-only reload (`start_database_action(.., false)`) - they are kept
/// as separate variants only because they are offered from different
/// states and read better as separate buttons to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DatabasePanelAction {
    ScanLibrary,
    ViewRecentlyFound,
    RefreshStatus,
    RetryLoad,
    /// Opens the read-only skipped-files drill-down for the most recently
    /// completed scan this session. Never re-scans or reclassifies
    /// anything - see [`show_skipped_files_window`].
    ViewSkippedFiles,
}

/// The best-known database path regardless of current state - even a
/// database that has never been created yet still has a *planned* path.
/// Shared by `show_database_panel` and the About window so both surfaces
/// can never disagree about which path they mean.
pub(super) fn database_state_path(state: &DatabaseState) -> Option<PathBuf> {
    match state {
        DatabaseState::NotCreated { database_path } => Some(database_path.clone()),
        DatabaseState::Loading { previous, .. }
        | DatabaseState::Outdated { previous, .. }
        | DatabaseState::Error { previous, .. } => previous
            .as_ref()
            .map(|snapshot| snapshot.database_path.clone()),
        DatabaseState::Ready { snapshot, .. } => Some(snapshot.database_path.clone()),
    }
}

/// Renders the compact "Library Database" status area (requirement 3).
/// Purely informational plus three buttons - never itself authorizes an
/// action, and never blocks the caller (all of its data comes from
/// `state`, already computed off the UI thread).
pub(super) fn show_database_panel(
    ui: &mut egui::Ui,
    state: &DatabaseState,
) -> Option<DatabasePanelAction> {
    let mut action = None;
    egui::CollapsingHeader::new("Library Database")
        .id_salt("library_database_panel")
        .default_open(!matches!(state, DatabaseState::Ready { .. }))
        .show(ui, |ui| {
            let database_path = database_state_path(state);

            egui::Grid::new("database_status_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.strong("Database path");
                    ui.label(
                        database_path
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "unresolved".to_string()),
                    );
                    ui.end_row();

                    ui.strong("Status");
                    ui.label(state.status_label());
                    ui.end_row();

                    if let Some(snapshot) = state.snapshot() {
                        ui.strong("Schema version");
                        ui.label(snapshot.schema_version.to_string());
                        ui.end_row();

                        ui.strong("Last completed scan");
                        ui.label(
                            snapshot
                                .last_completed_scan
                                .as_ref()
                                .map(|scan| {
                                    scan.finished_at.clone().unwrap_or_else(|| {
                                        format!("{} (in progress)", scan.started_at)
                                    })
                                })
                                .unwrap_or_else(|| "never".to_string()),
                        );
                        ui.end_row();

                        ui.strong("Cached archives");
                        ui.label(snapshot.stats.total_archives.to_string());
                        ui.end_row();

                        ui.strong("Present / missing");
                        ui.label(format!(
                            "{} / {}",
                            snapshot.stats.present_archives, snapshot.stats.missing_archives
                        ));
                        ui.end_row();
                    }

                    if let DatabaseState::Ready {
                        last_scan_summary: Some(summary),
                        ..
                    } = state
                    {
                        ui.strong("Last scan (this session)");
                        ui.label(format_scan_completion(summary));
                        ui.end_row();

                        let skipped_total = summary.skipped_files_total();
                        if skipped_total > 0 {
                            ui.strong("Skipped files");
                            ui.horizontal(|ui| {
                                ui.label(format!("{skipped_total} skipped"));
                                if widgets::action_button(
                                    ui,
                                    "Inspect…",
                                    widgets::ActionStyle::Quiet,
                                    true,
                                )
                                .clicked()
                                {
                                    action = Some(DatabasePanelAction::ViewSkippedFiles);
                                }
                            });
                            ui.end_row();
                        }
                    }

                    ui.strong("Action safety");
                    ui.label(
                        "Cached rows never authorize mount or unmount - only a validated live \
                         snapshot can.",
                    );
                    ui.end_row();
                });

            match state {
                DatabaseState::Outdated { health, .. } => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!(
                            "Database schema is outdated (found version {}); run a library scan \
                             to upgrade it.",
                            health
                                .schema_version
                                .map(|version| version.to_string())
                                .unwrap_or_else(|| "unknown".to_string())
                        ),
                    );
                }
                DatabaseState::Error { message, .. } => {
                    ui.colored_label(ui.visuals().error_fg_color, message.as_str());
                }
                DatabaseState::NotCreated { .. } => {
                    ui.label("No library database yet. Run a library scan to create one.");
                }
                DatabaseState::Loading { .. } | DatabaseState::Ready { .. } => {}
            }

            ui.horizontal(|ui| {
                let loading = state.is_loading();
                if ui
                    .add_enabled(!loading, egui::Button::new("Scan library"))
                    .clicked()
                {
                    action = Some(DatabasePanelAction::ScanLibrary);
                }
                if state
                    .snapshot()
                    .and_then(|snapshot| snapshot.recently_found.as_ref())
                    .is_some()
                    && ui.button("View recently found").clicked()
                {
                    action = Some(DatabasePanelAction::ViewRecentlyFound);
                }
                match state {
                    DatabaseState::Ready { .. } => {
                        if ui
                            .add_enabled(!loading, egui::Button::new("Refresh database status"))
                            .clicked()
                        {
                            action = Some(DatabasePanelAction::RefreshStatus);
                        }
                    }
                    DatabaseState::NotCreated { .. }
                    | DatabaseState::Outdated { .. }
                    | DatabaseState::Error { .. } => {
                        if ui
                            .add_enabled(!loading, egui::Button::new("Retry database load"))
                            .clicked()
                        {
                            action = Some(DatabasePanelAction::RetryLoad);
                        }
                    }
                    DatabaseState::Loading { .. } => {}
                }
                if loading {
                    ui.spinner();
                    ui.label(if state.is_scanning() {
                        "Scanning..."
                    } else {
                        "Loading..."
                    });
                }
            });
        });
    action
}

/// Renders the compact "Custom Platform Aliases" section: a collapsible
/// list of persisted aliases (each with a Remove button), plus an
/// alias-text field and a canonical-platform dropdown with an Add Alias
/// button. Purely a view over `aliases` (already loaded off the UI
/// thread as part of the cached database snapshot - see
/// `CachedLibrarySnapshot::platform_aliases`) plus the two caller-owned
/// input fields; never itself opens a database or blocks. `busy` (from
/// `ArchiveFsApp::alias_action`) disables every control here while one
/// alias action is already running, so two cannot overlap - it is
/// deliberately independent of `is_busy()`/mount safety, exactly like
/// the existing per-archive platform assignment controls.
pub(super) fn show_platform_aliases_panel(
    ui: &mut egui::Ui,
    aliases: &[PlatformAlias],
    new_alias_text: &mut String,
    new_alias_platform_choice: &mut Option<String>,
    busy: bool,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<AliasAction> {
    let mut action = None;
    egui::CollapsingHeader::new("Custom Platform Aliases")
        .id_salt("platform_aliases_panel")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(
                "Map a folder name to a platform (for example \"gc\" -> GameCube). Custom \
                 aliases outrank built-in detection but never a manual archive assignment. \
                 Changes take effect on the next library scan.",
            );
            ui.separator();

            if aliases.is_empty() {
                ui.label("No custom platform aliases defined.");
            } else {
                egui::Grid::new("platform_aliases_grid")
                    .num_columns(3)
                    .show(ui, |ui| {
                        for alias in aliases {
                            ui.label(&alias.alias);
                            ui.label(&alias.platform);
                            if ui.add_enabled(!busy, egui::Button::new("Remove")).clicked() {
                                action = Some(AliasAction::Remove {
                                    alias: alias.alias.clone(),
                                });
                            }
                            ui.end_row();
                        }
                    });
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Alias:");
                ui.add_enabled_ui(!busy, |ui| {
                    show_text_edit_with_context_menu(ui, new_alias_text, clipboard, |text_edit| {
                        text_edit.desired_width(120.0).hint_text("gc")
                    });
                });
                ui.label("Platform:");
                egui::ComboBox::from_id_salt("platform_alias_choice_combo")
                    .selected_text(
                        new_alias_platform_choice
                            .as_deref()
                            .unwrap_or("Select platform..."),
                    )
                    .show_ui(ui, |ui| {
                        for name in canonical_platform_names() {
                            ui.selectable_value(
                                new_alias_platform_choice,
                                Some(name.to_string()),
                                name,
                            );
                        }
                    });

                let resolved_action =
                    resolved_new_alias_action(new_alias_text, new_alias_platform_choice.as_deref());
                if ui
                    .add_enabled(
                        !busy && resolved_action.is_some(),
                        egui::Button::new("Add Alias"),
                    )
                    .clicked()
                {
                    action = resolved_action;
                }
                if busy {
                    ui.spinner();
                }
            });
        });
    action
}

/// The `AliasAction::Add` the Add Alias button constructs, factored out
/// so it is directly testable (mirrors `resolved_platform_choice`'s
/// existing convention for the per-archive platform editor). `None`
/// exactly when the button itself would be disabled: `alias` trims to
/// empty, or no platform has been chosen from the canonical-platform
/// picker yet.
pub(super) fn resolved_new_alias_action(
    alias: &str,
    platform_choice: Option<&str>,
) -> Option<AliasAction> {
    let trimmed_alias = alias.trim();
    if trimmed_alias.is_empty() {
        return None;
    }
    let platform = platform_choice?;
    Some(AliasAction::Add {
        alias: trimmed_alias.to_string(),
        platform: platform.to_string(),
    })
}

pub(super) fn duplicate_visible_entries(
    group: &CatalogueDuplicateGroup,
    include_missing: bool,
) -> Vec<&CatalogueDuplicateArchive> {
    group
        .entries
        .iter()
        .filter(|entry| include_missing || entry.present)
        .collect()
}

pub(super) fn duplicate_group_matches(
    group: &CatalogueDuplicateGroup,
    filters: &DuplicateReviewFilters,
) -> bool {
    if filters
        .platform
        .as_deref()
        .is_some_and(|platform| platform != group.platform)
    {
        return false;
    }
    let entries = duplicate_visible_entries(group, filters.include_missing);
    if entries.len() < 2 || (filters.more_than_two && entries.len() <= 2) {
        return false;
    }
    let search = filters.search.trim().to_lowercase();
    search.is_empty()
        || group.title.to_lowercase().contains(&search)
        || group.normalized_title.contains(&search)
        || entries.iter().any(|entry| {
            entry
                .path
                .to_string_lossy()
                .to_lowercase()
                .contains(&search)
        })
}

pub(super) fn visible_duplicate_group_indices(
    report: &CatalogueDuplicateReport,
    filters: &DuplicateReviewFilters,
    sort_field: DuplicateSortField,
    ascending: bool,
) -> Vec<usize> {
    let mut indices = report
        .groups
        .iter()
        .enumerate()
        .filter_map(|(index, group)| duplicate_group_matches(group, filters).then_some(index))
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        let left_group = &report.groups[*left];
        let right_group = &report.groups[*right];
        let left_entries = duplicate_visible_entries(left_group, filters.include_missing);
        let right_entries = duplicate_visible_entries(right_group, filters.include_missing);
        let ordering = match sort_field {
            DuplicateSortField::Title => left_group
                .normalized_title
                .cmp(&right_group.normalized_title),
            DuplicateSortField::Platform => left_group.platform.cmp(&right_group.platform),
            DuplicateSortField::Entries => left_entries.len().cmp(&right_entries.len()),
            DuplicateSortField::KnownSize => {
                visible_known_size(&left_entries).cmp(&visible_known_size(&right_entries))
            }
        }
        .then_with(|| {
            left_group
                .normalized_title
                .cmp(&right_group.normalized_title)
        })
        .then_with(|| left_group.platform.cmp(&right_group.platform));
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    indices
}

pub(super) fn visible_known_size(entries: &[&CatalogueDuplicateArchive]) -> u128 {
    entries
        .iter()
        .filter_map(|entry| entry.size_bytes)
        .map(u128::from)
        .sum()
}

pub(super) fn prune_duplicate_review_selection(
    selected_group: &mut Option<DuplicateGroupIdentity>,
    selected_archive: &mut Option<PathBuf>,
    report: Option<&CatalogueDuplicateReport>,
) {
    let selected_group_still_exists = selected_group.as_ref().is_some_and(|selected| {
        report.is_some_and(|report| {
            report
                .groups
                .iter()
                .any(|group| DuplicateGroupIdentity::from(group) == *selected)
        })
    });
    if !selected_group_still_exists {
        *selected_group = None;
        *selected_archive = None;
        return;
    }
    let selected_archive_still_exists = selected_archive.as_ref().is_none_or(|path| {
        report.is_some_and(|report| {
            report.groups.iter().any(|group| {
                selected_group.as_ref() == Some(&DuplicateGroupIdentity::from(group))
                    && group.entries.iter().any(|entry| entry.path == *path)
            })
        })
    });
    if !selected_archive_still_exists {
        *selected_archive = None;
    }
}
pub(super) enum DuplicateReviewAction {
    Close,
    ViewInLibrary(PathBuf),
    Inspect(PathBuf),
}

pub(super) struct DuplicateReviewViewState<'a> {
    pub(super) filters: &'a mut DuplicateReviewFilters,
    pub(super) sort_field: &'a mut DuplicateSortField,
    pub(super) sort_ascending: &'a mut bool,
    pub(super) selected_group: &'a mut Option<DuplicateGroupIdentity>,
    pub(super) selected_archive: &'a mut Option<PathBuf>,
    pub(super) clipboard: &'a mut dyn ClipboardBackend,
}

/// Reviewed for the Library IA migration: its "Back to Library" button
/// (`DuplicateReviewAction::Close`) is kept rather than removed, even
/// though the unified Library shell's Archives tab is now also one click
/// away via the tab row - the button sits right at the top of this
/// content, above the same internal scroll area the tab row itself is
/// pinned outside of, so it is not meaningfully redundant, and removing
/// a working exit action was judged not worth the small risk for a small
/// convenience gain. Its handler now calls `navigate_to_library_tab`.
pub(super) fn show_duplicate_review_panel(
    ui: &mut egui::Ui,
    report: &CatalogueDuplicateReport,
    view_state: DuplicateReviewViewState<'_>,
) -> Option<DuplicateReviewAction> {
    let DuplicateReviewViewState {
        filters,
        sort_field,
        sort_ascending,
        selected_group,
        selected_archive,
        clipboard,
    } = view_state;
    let mut action = None;
    widgets::section_header(
        ui,
        "Duplicates",
        Some(
            "Review only — EmuWiz will not change archive files here. Groups are likely \
             duplicates, not claims that files are byte-identical.",
        ),
    );
    if widgets::action_button(ui, "Back to Library", widgets::ActionStyle::Quiet, true).clicked() {
        action = Some(DuplicateReviewAction::Close);
    }
    ui.add_space(2.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("Search title or exact path:");
        show_text_edit_with_context_menu(ui, &mut filters.search, clipboard, |text_edit| {
            text_edit
                .id_salt("archivefs_duplicate_search")
                .desired_width(260.0)
        });
        ui.checkbox(&mut filters.include_missing, "Include missing entries");
        ui.checkbox(&mut filters.more_than_two, "More than two entries");
    });

    let mut platforms = report
        .groups
        .iter()
        .map(|group| group.platform.as_str())
        .collect::<Vec<_>>();
    platforms.sort_unstable();
    platforms.dedup();
    ui.horizontal(|ui| {
        ui.label("Platform:");
        egui::ComboBox::from_id_salt("duplicate_platform_filter")
            .selected_text(filters.platform.as_deref().unwrap_or("All platforms"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.platform, None, "All platforms");
                for platform in platforms {
                    ui.selectable_value(
                        &mut filters.platform,
                        Some(platform.to_string()),
                        platform,
                    );
                }
            });
        ui.label("Sort by:");
        egui::ComboBox::from_id_salt("duplicate_sort")
            .selected_text(sort_field.to_string())
            .show_ui(ui, |ui| {
                for field in [
                    DuplicateSortField::Title,
                    DuplicateSortField::Platform,
                    DuplicateSortField::Entries,
                    DuplicateSortField::KnownSize,
                ] {
                    ui.selectable_value(sort_field, field, field.to_string());
                }
            });
        ui.checkbox(sort_ascending, "Ascending");
    });

    let visible = visible_duplicate_group_indices(report, filters, *sort_field, *sort_ascending);
    let visible_entry_count = visible
        .iter()
        .map(|index| {
            duplicate_visible_entries(&report.groups[*index], filters.include_missing).len()
        })
        .sum::<usize>();
    ui.horizontal_wrapped(|ui| {
        summary_value(ui, "Duplicate groups", visible.len());
        summary_value(ui, "Archive entries involved", visible_entry_count);
        if !filters.include_missing {
            ui.label("Present entries only");
        }
    });
    ui.separator();

    if visible.is_empty() {
        ui.label("No likely duplicate groups match the current review filters.");
        return action;
    }

    ui.strong("Likely duplicate groups");
    egui::ScrollArea::vertical()
        .id_salt("duplicate_group_list")
        .max_height(180.0)
        .show(ui, |ui| {
            for index in &visible {
                let group = &report.groups[*index];
                let identity = DuplicateGroupIdentity::from(group);
                let entry_count = duplicate_visible_entries(group, filters.include_missing).len();
                let selected = selected_group.as_ref() == Some(&identity);
                let label_text = format!(
                    "{} — {} — {} entries",
                    group.title, group.platform, entry_count
                );
                let response = ui
                    .add(egui::Button::selectable(selected, &label_text).truncate())
                    .on_hover_text(&label_text);
                if response.clicked() {
                    *selected_group = Some(identity.clone());
                    *selected_archive = None;
                }
                let group_entries = duplicate_visible_entries(group, filters.include_missing);
                response.context_menu(|ui| {
                    if ui.button("Select duplicate group").clicked() {
                        *selected_group = Some(identity.clone());
                        *selected_archive = None;
                        ui.close();
                    }
                    if ui.button("Copy all paths in this group").clicked() {
                        let text = group_entries
                            .iter()
                            .map(|entry| entry.path.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let _ = clipboard.set_text(text);
                        ui.close();
                    }
                });
            }
        });

    let Some(group) = selected_group.as_ref().and_then(|selected| {
        visible
            .iter()
            .map(|index| &report.groups[*index])
            .find(|group| DuplicateGroupIdentity::from(*group) == *selected)
    }) else {
        ui.label("Select a likely duplicate group to inspect every archive in it.");
        return action;
    };
    let entries = duplicate_visible_entries(group, filters.include_missing);
    if entries.len() < 2 {
        ui.label("The selected group is hidden by the current entry filters.");
        return action;
    }

    ui.separator();
    ui.strong("Likely duplicate group");
    egui::Grid::new("duplicate_group_details")
        .num_columns(2)
        .show(ui, |ui| {
            detail_row(ui, "Title", &group.title);
            detail_row(ui, "Platform", &group.platform);
            detail_row(ui, "Entries", &entries.len().to_string());
            detail_row(ui, "Method", "Filename and platform");
            detail_row(ui, "Reason", &group.reason);
            let known_count = entries
                .iter()
                .filter(|entry| entry.size_bytes.is_some())
                .count();
            detail_row(
                ui,
                "Total known size",
                &format!(
                    "{} ({} of {} entries known)",
                    format_known_size(visible_known_size(&entries)),
                    known_count,
                    entries.len()
                ),
            );
        });

    ui.add_space(4.0);
    for entry in entries {
        let is_selected = selected_archive.as_ref() == Some(&entry.path);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let entry_path_text = entry.path.display().to_string();
            let response = ui
                .add(egui::Button::selectable(is_selected, &entry_path_text).truncate())
                .on_hover_text(&entry_path_text);
            if response.clicked() {
                *selected_archive = Some(entry.path.clone());
            }
            // Requirement: "No deletion controls. No automatic cleanup." -
            // only navigation, read-only inspection, and clipboard copies
            // are ever offered here.
            response.context_menu(|ui| {
                if ui.button("Show archive in Library").clicked() {
                    action = Some(DuplicateReviewAction::ViewInLibrary(entry.path.clone()));
                    ui.close();
                }
                let inspectable = is_inspectable(&entry.path);
                if ui
                    .add_enabled(inspectable, egui::Button::new("Inspect contents"))
                    .clicked()
                {
                    action = Some(DuplicateReviewAction::Inspect(entry.path.clone()));
                    ui.close();
                }
                if ui.button("Copy archive path").clicked() {
                    let _ = clipboard.set_text(entry_path_text.clone());
                    ui.close();
                }
            });
            let state = if entry.present { "Present" } else { "Missing" };
            let color = if entry.present {
                ui.visuals().text_color()
            } else {
                ui.visuals().warn_fg_color
            };
            ui.colored_label(color, state);
            ui.label(format!("Size: {}", format_duplicate_size(entry.size_bytes)));
            ui.label(format!(
                "Modified time: {}",
                format_modified_time(entry.modified_time_unix_seconds)
            ));
        });
    }

    if let Some(path) = selected_archive.as_ref()
        && let Some(entry) = group.entries.iter().find(|entry| entry.path == *path)
    {
        ui.separator();
        ui.strong("Selected duplicate archive");
        egui::Grid::new("selected_duplicate_archive_details")
            .num_columns(2)
            .show(ui, |ui| {
                detail_row(ui, "Exact archive path", &entry.path.display().to_string());
                detail_row(ui, "Platform", &group.platform);
                detail_row(
                    ui,
                    "State",
                    if entry.present { "Present" } else { "Missing" },
                );
                detail_row(ui, "Size", &format_duplicate_size(entry.size_bytes));
                detail_row(
                    ui,
                    "Modified time",
                    &format_modified_time(entry.modified_time_unix_seconds),
                );
            });
    }
    action
}

pub(super) fn format_known_size(size_bytes: u128) -> String {
    format_byte_count(size_bytes)
}

pub(super) fn format_duplicate_size(size_bytes: Option<u64>) -> String {
    size_bytes
        .map(|size| format_byte_count(u128::from(size)))
        .unwrap_or_else(|| "Unknown".to_string())
}

pub(super) fn format_byte_count(size_bytes: u128) -> String {
    const KIB: u128 = 1024;
    const MIB: u128 = KIB * 1024;
    const GIB: u128 = MIB * 1024;
    const TIB: u128 = GIB * 1024;
    let (unit_size, unit_name) = if size_bytes >= TIB {
        (TIB, "TiB")
    } else if size_bytes >= GIB {
        (GIB, "GiB")
    } else if size_bytes >= MIB {
        (MIB, "MiB")
    } else if size_bytes >= KIB {
        (KIB, "KiB")
    } else {
        return format!("{size_bytes} bytes");
    };
    let whole = size_bytes / unit_size;
    let tenth = (size_bytes % unit_size) * 10 / unit_size;
    format!("{whole}.{tenth} {unit_name} ({size_bytes} bytes)")
}

pub(super) fn format_modified_time(seconds: Option<i64>) -> String {
    seconds
        .map(format_unix_timestamp_utc)
        .unwrap_or_else(|| "Unknown".to_string())
}

// -----------------------------------------------------------------------
// v0.4.3-alpha: Health and Recovery Dashboard.
// -----------------------------------------------------------------------
pub(super) fn build_health_issues(
    records: &[ArchiveRecord],
    cached: &CachedLibrarySnapshot,
    lazy_unmount_offers: &HashSet<PathBuf>,
    remount_offers: &HashSet<PathBuf>,
) -> Vec<HealthIssue> {
    let persisted_by_path: HashMap<&Path, &PersistedArchive> = cached
        .archives
        .iter()
        .map(|persisted| (persisted.absolute_path.as_path(), persisted))
        .collect();

    let mut issues = Vec::new();

    for record in records {
        let path = record.mount_plan.archive.path.as_path();
        let persisted = persisted_by_path.get(path).copied();
        // Prefer the persisted effective platform when the database
        // already has an entry for this exact path - the same override
        // `ArchiveRow::with_persisted_platform` applies for display, so
        // this can never disagree with the library table about whether a
        // platform counts as unknown.
        let raw_platform = record
            .metadata
            .platform
            .as_deref()
            .or(record.identity.platform.as_deref());
        let platform = persisted
            .map(|persisted| persisted.platform.as_deref())
            .unwrap_or(raw_platform);
        let recovery_offer = if remount_offers.contains(path) {
            Some(RecoveryOffer::Remount)
        } else if lazy_unmount_offers.contains(path) {
            Some(RecoveryOffer::LazyUnmount)
        } else {
            None
        };
        let modified_time_unix_seconds = record
            .identity
            .modified_time
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64);
        let input = ArchiveHealthInput {
            path,
            platform,
            presence: ArchivePresence::Confirmed,
            mount_state: Some(record.mount_state),
            archive_health: Some(record.health),
            recovery_offer,
            last_seen_at: persisted.map(|persisted| persisted.last_seen_at.as_str()),
            size_bytes: record.identity.size_bytes,
            modified_time_unix_seconds,
        };
        if let Some(issue) = classify_archive_health(&input) {
            issues.push(issue);
        }
    }

    // Archives owned by a source that is itself currently unavailable/
    // permission-denied/scan-failed must never each become their own
    // `HealthIssue` below - that would flood the dashboard with one
    // "cached only"/"awaiting validation" entry per archive merely because
    // the drive is offline. `source_health_issues` (computed by the
    // caller from this same `cached.source_views`) already reports that
    // truthfully as one issue for the whole source; skip these archives
    // here so the two never duplicate the same underlying problem.
    let unavailable_source_ids: HashSet<i64> = cached
        .source_views
        .iter()
        .filter(|view| {
            matches!(
                view.availability,
                SourceAvailability::Unavailable
                    | SourceAvailability::PermissionDenied
                    | SourceAvailability::ScanFailed
            )
        })
        .filter_map(|view| view.id)
        .collect();

    let live_paths: HashSet<&Path> = records
        .iter()
        .map(|record| record.mount_plan.archive.path.as_path())
        .collect();
    for persisted in &cached.archives {
        if live_paths.contains(persisted.absolute_path.as_path()) {
            continue;
        }
        if unavailable_source_ids.contains(&persisted.source_folder_id) {
            continue;
        }
        // Older builds catalogued .cue/.m3u rows independently. Current scans
        // correctly skip those companion/metadata files, which makes the
        // retained legacy row look "missing" even while its disc set is fine.
        // Do not turn that history artefact into a missing-game warning. This
        // proves no BIN/CUE pairing and changes no persisted scan evidence.
        if is_known_disc_companion(&persisted.absolute_path) {
            continue;
        }
        let presence = if persisted.last_verified_missing_at.is_some() {
            ArchivePresence::Missing
        } else if persisted.absolute_path.exists() {
            ArchivePresence::AwaitingValidation
        } else {
            ArchivePresence::Unreachable
        };
        let input = ArchiveHealthInput {
            path: &persisted.absolute_path,
            platform: persisted.platform.as_deref(),
            presence,
            mount_state: None,
            // A database value is retained history, never proof that a
            // failure is happening in this GUI session. Supplying no mount
            // state lets the shared classifier preserve that distinction.
            archive_health: persisted_failure_health(&persisted.last_known_health),
            recovery_offer: None,
            last_seen_at: Some(&persisted.last_seen_at),
            size_bytes: persisted.size_bytes,
            modified_time_unix_seconds: persisted.modified_time_unix_seconds,
        };
        if let Some(issue) = classify_archive_health(&input) {
            issues.push(issue);
        }
    }

    issues.sort_by(|left, right| {
        left.category
            .severity_rank()
            .cmp(&right.category.severity_rank())
            .then_with(|| left.path.cmp(&right.path))
    });
    issues
}

pub(super) fn persisted_failure_health(value: &str) -> Option<archivefs_core::ArchiveHealth> {
    match value {
        "Failed" => Some(archivefs_core::ArchiveHealth::Failed),
        "MissingParts" => Some(archivefs_core::ArchiveHealth::MissingParts),
        "Corrupt" => Some(archivefs_core::ArchiveHealth::Corrupt),
        "Unsupported" => Some(archivefs_core::ArchiveHealth::Unsupported),
        "PermissionDenied" => Some(archivefs_core::ArchiveHealth::PermissionDenied),
        "RetryAvailable" => Some(archivefs_core::ArchiveHealth::RetryAvailable),
        _ => None,
    }
}

/// A human-readable state label - never `MountState`'s raw `Display`
/// alone for a cache-only row (which has no `MountState` at all), and
/// never a raw database source string. Reuses the exact same "Cached:
/// ..." wording `RowOrigin::label()` already established for the library
/// table, so the two screens never disagree about vocabulary.
pub(super) fn health_issue_state_text(issue: &HealthIssue) -> String {
    if let Some(state) = issue.mount_state {
        return state.to_string();
    }
    match issue.category {
        HealthCategory::Missing => "Cached: missing".to_string(),
        HealthCategory::AwaitingValidation => "Cached: awaiting validation".to_string(),
        HealthCategory::CachedOnly => "Cached: source unavailable".to_string(),
        _ => (if issue.present { "Present" } else { "Missing" }).to_string(),
    }
}

pub(super) fn health_issue_matches(issue: &HealthIssue, filters: &HealthDashboardFilters) -> bool {
    if !filters.category.matches(issue.category) {
        return false;
    }
    if let Some(platform) = &filters.platform {
        let issue_platform = issue.platform.as_deref().unwrap_or("Unknown");
        if issue_platform != platform {
            return false;
        }
    }
    let search = filters.search.trim().to_lowercase();
    if search.is_empty() {
        return true;
    }
    let path_text = issue.path.display().to_string().to_lowercase();
    let reason_text = issue.reason.to_lowercase();
    path_text.contains(&search) || reason_text.contains(&search)
}
pub(super) fn visible_health_issue_indices(
    issues: &[HealthIssue],
    filters: &HealthDashboardFilters,
    sort_field: HealthSortField,
    ascending: bool,
) -> Vec<usize> {
    let mut indices: Vec<usize> = issues
        .iter()
        .enumerate()
        .filter_map(|(index, issue)| health_issue_matches(issue, filters).then_some(index))
        .collect();
    indices.sort_by(|&left, &right| {
        let left_issue = &issues[left];
        let right_issue = &issues[right];
        let ordering = match sort_field {
            HealthSortField::Severity => left_issue
                .category
                .severity_rank()
                .cmp(&right_issue.category.severity_rank()),
            HealthSortField::Path => left_issue.path.cmp(&right_issue.path),
            HealthSortField::Platform => left_issue
                .platform
                .as_deref()
                .unwrap_or("Unknown")
                .cmp(right_issue.platform.as_deref().unwrap_or("Unknown")),
            HealthSortField::State => {
                health_issue_state_text(left_issue).cmp(&health_issue_state_text(right_issue))
            }
            HealthSortField::Reason => left_issue.reason.cmp(&right_issue.reason),
        }
        .then_with(|| left_issue.path.cmp(&right_issue.path));
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    indices
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct HealthOverview {
    pub(super) healthy: usize,
    pub(super) mounted: usize,
    pub(super) pending: usize,
    pub(super) missing: usize,
    pub(super) awaiting_validation: usize,
    pub(super) cached_only: usize,
    pub(super) retryable_failures: usize,
    pub(super) terminal_failures: usize,
    pub(super) historical_mount_failures: usize,
    pub(super) no_mount_required: usize,
    pub(super) mount_results_needing_context: usize,
    pub(super) unknown_platform: usize,
    pub(super) recovery_available: usize,
    pub(super) diagnostics_errors: usize,
}

pub(super) fn health_overview(
    issues: &[HealthIssue],
    live_archive_count: usize,
    mounted_count: usize,
    pending_count: usize,
    diagnostics_errors: usize,
) -> HealthOverview {
    let count = |category: HealthCategory| {
        issues
            .iter()
            .filter(|issue| issue.category == category)
            .count()
    };
    let missing = count(HealthCategory::Missing);
    let awaiting_validation = count(HealthCategory::AwaitingValidation);
    let cached_only = count(HealthCategory::CachedOnly);
    let retryable_failures = count(HealthCategory::RetryableFailure);
    let terminal_failures = count(HealthCategory::TerminalFailure);
    let historical_mount_failures = count(HealthCategory::HistoricalMountFailure);
    let no_mount_required = count(HealthCategory::MountNotRequired);
    let mount_results_needing_context = count(HealthCategory::MountFailureEvidenceInsufficient);
    let unknown_platform = count(HealthCategory::UnknownPlatform);
    let recovery_available = count(HealthCategory::RecoveryAvailable);
    // Every `UnknownPlatform`/failure/recovery issue is a live archive -
    // `classify_archive_health`'s priority order always classifies a
    // cached-only row as Missing/AwaitingValidation/CachedOnly first, so
    // it can never also count as UnknownPlatform here. This subtraction
    // can therefore never double count or misclassify a cached-only row.
    let unhealthy_live =
        retryable_failures + terminal_failures + recovery_available + unknown_platform;
    let healthy = live_archive_count.saturating_sub(unhealthy_live);
    HealthOverview {
        healthy,
        mounted: mounted_count,
        pending: pending_count,
        missing,
        awaiting_validation,
        cached_only,
        retryable_failures,
        terminal_failures,
        historical_mount_failures,
        no_mount_required,
        mount_results_needing_context,
        unknown_platform,
        recovery_available,
        diagnostics_errors,
    }
}
pub(super) enum HealthDashboardAction {
    BackToLibrary,
    Archive(OperationRequest),
    RefreshDiagnostics,
    OpenMissingReview,
    OpenDuplicateReview,
    ViewInLibrary(PathBuf),
    Inspect(PathBuf),
    FilterByCategory(HealthIssueFilter),
}
pub(super) fn health_issue_filter_for_category(category: HealthCategory) -> HealthIssueFilter {
    match category {
        HealthCategory::TerminalFailure => HealthIssueFilter::Terminal,
        HealthCategory::RetryableFailure => HealthIssueFilter::Retryable,
        HealthCategory::HistoricalMountFailure => HealthIssueFilter::Historical,
        HealthCategory::MountNotRequired => HealthIssueFilter::NoMountRequired,
        HealthCategory::MountFailureEvidenceInsufficient => HealthIssueFilter::NeedsContext,
        HealthCategory::RecoveryAvailable => HealthIssueFilter::RecoveryAvailable,
        HealthCategory::Missing => HealthIssueFilter::Missing,
        HealthCategory::AwaitingValidation => HealthIssueFilter::AwaitingValidation,
        HealthCategory::CachedOnly => HealthIssueFilter::CachedOnly,
        HealthCategory::UnknownPlatform => HealthIssueFilter::UnknownPlatform,
    }
}

pub(super) struct HealthDashboardViewState<'a> {
    pub(super) filters: &'a mut HealthDashboardFilters,
    pub(super) sort_field: &'a mut HealthSortField,
    pub(super) sort_ascending: &'a mut bool,
    pub(super) selected_issue: &'a mut Option<PathBuf>,
    pub(super) busy: bool,
    pub(super) clipboard: &'a mut dyn ClipboardBackend,
}

/// Reviewed for the Library IA migration: its "Back to Library" button
/// (`HealthDashboardAction::BackToLibrary`) is kept for the same reason
/// `show_duplicate_review_panel`'s is - not meaningfully redundant with
/// the tab row, and removing a working exit action wasn't worth it for a
/// small convenience gain. Its handler now calls
/// `navigate_to_library_tab`.
pub(super) fn show_health_dashboard_panel(
    ui: &mut egui::Ui,
    live_data: Option<&LoadedData>,
    cached: &CachedLibrarySnapshot,
    issues: &[HealthIssue],
    view_state: HealthDashboardViewState<'_>,
) -> Option<HealthDashboardAction> {
    let HealthDashboardViewState {
        filters,
        sort_field,
        sort_ascending,
        selected_issue,
        busy,
        clipboard,
    } = view_state;

    let mut action = None;
    widgets::section_header(
        ui,
        "Health",
        Some(
            "Read-only overview of archive and catalogue health. Filtering, sorting, and \
             selection here are independent of the ordinary library view and Duplicate Review.",
        ),
    );
    if widgets::action_button(ui, "Back to Library", widgets::ActionStyle::Quiet, true).clicked() {
        action = Some(HealthDashboardAction::BackToLibrary);
    }
    ui.add_space(2.0);

    let Some(data) = live_data else {
        ui.separator();
        ui.label("Health information is unavailable until the library is scanned.");
        return action;
    };

    let diagnostics_errors = data
        .doctor
        .checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Fail)
        .count();
    let overview = health_overview(
        issues,
        data.records.len(),
        data.stats.mounted_count,
        data.stats.pending_count,
        diagnostics_errors,
    );

    let metrics = [
        SummaryMetric {
            label: "Healthy / ready",
            value: overview.healthy,
            tone: widgets::StatusTone::Success,
        },
        SummaryMetric {
            label: "Mounted",
            value: overview.mounted,
            tone: widgets::StatusTone::Active,
        },
        SummaryMetric {
            label: "Pending",
            value: overview.pending,
            tone: widgets::StatusTone::Pending,
        },
        SummaryMetric {
            label: "Missing",
            value: overview.missing,
            tone: widgets::StatusTone::Blocked,
        },
        SummaryMetric {
            label: "Retryable failures",
            value: overview.retryable_failures,
            tone: widgets::StatusTone::Warning,
        },
        SummaryMetric {
            label: "Terminal failures",
            value: overview.terminal_failures,
            tone: widgets::StatusTone::Blocked,
        },
        SummaryMetric {
            label: "Historical mount failures",
            value: overview.historical_mount_failures,
            tone: widgets::StatusTone::Info,
        },
        SummaryMetric {
            label: "No mount required",
            value: overview.no_mount_required,
            tone: widgets::StatusTone::Info,
        },
        SummaryMetric {
            label: "Mount results needing context",
            value: overview.mount_results_needing_context,
            tone: widgets::StatusTone::Pending,
        },
        SummaryMetric {
            label: "Recovery available",
            value: overview.recovery_available,
            tone: widgets::StatusTone::Info,
        },
        SummaryMetric {
            label: "Awaiting validation",
            value: overview.awaiting_validation,
            tone: widgets::StatusTone::Warning,
        },
        SummaryMetric {
            label: "Cached-only",
            value: overview.cached_only,
            tone: widgets::StatusTone::Pending,
        },
        SummaryMetric {
            label: "Unknown platform",
            value: overview.unknown_platform,
            tone: widgets::StatusTone::Info,
        },
        SummaryMetric {
            label: "Diagnostics errors",
            value: overview.diagnostics_errors,
            tone: widgets::StatusTone::Blocked,
        },
    ];
    show_health_metric_cards(ui, &metrics);
    ui.label(
        "Healthy/Mounted/Pending/Retryable/Terminal/Recovery available/Unknown platform: current \
         live snapshot. Historical: retained catalogue evidence. No mount required: loose content \
         that bypasses the archive mount workflow. Missing/Awaiting validation/Cached-only: \
         persisted catalogue. Diagnostics errors: last Doctor check.",
    );
    ui.add_space(4.0);

    ui.horizontal_wrapped(|ui| {
        if ui.button("Refresh diagnostics").clicked() {
            action = Some(HealthDashboardAction::RefreshDiagnostics);
        }
        if ui
            .add_enabled(
                overview.missing > 0,
                egui::Button::new("Open Missing Review"),
            )
            .clicked()
        {
            action = Some(HealthDashboardAction::OpenMissingReview);
        }
        if ui.button("Open Duplicate Review").clicked() {
            action = Some(HealthDashboardAction::OpenDuplicateReview);
        }
    });

    if diagnostics_errors > 0 {
        ui.separator();
        ui.strong("Diagnostics issues");
        for check in data
            .doctor
            .checks
            .iter()
            .filter(|check| check.status == DoctorStatus::Fail)
        {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("{}: {}", check.name, check.detail),
            );
        }
    }
    let source_issues = source_health_issues(&cached.source_views);
    if !source_issues.is_empty() {
        ui.separator();
        ui.strong("Source issues");
        for issue in &source_issues {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("{}: {}", issue.path.display(), issue.reason),
            );
            ui.label(format!(
                "  {} catalogue entries preserved.",
                issue.archives_preserved
            ));
        }
    }
    ui.separator();

    ui.horizontal_wrapped(|ui| {
        ui.label("Search path or reason:");
        show_text_edit_with_context_menu(ui, &mut filters.search, clipboard, |text_edit| {
            text_edit
                .id_salt("archivefs_health_search")
                .desired_width(260.0)
        });
        ui.label("Filter:");
        egui::ComboBox::from_id_salt("health_category_filter")
            .selected_text(filters.category.label())
            .show_ui(ui, |ui| {
                for option in HealthIssueFilter::ALL {
                    ui.selectable_value(&mut filters.category, option, option.label());
                }
            });
    });

    let mut platforms = issues
        .iter()
        .map(|issue| issue.platform.as_deref().unwrap_or("Unknown"))
        .collect::<Vec<_>>();
    platforms.sort_unstable();
    platforms.dedup();
    ui.horizontal(|ui| {
        ui.label("Platform:");
        egui::ComboBox::from_id_salt("health_platform_filter")
            .selected_text(filters.platform.as_deref().unwrap_or("All platforms"))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut filters.platform, None, "All platforms");
                for platform in platforms {
                    ui.selectable_value(
                        &mut filters.platform,
                        Some(platform.to_string()),
                        platform,
                    );
                }
            });
        ui.label("Sort by:");
        egui::ComboBox::from_id_salt("health_sort")
            .selected_text(sort_field.to_string())
            .show_ui(ui, |ui| {
                for field in [
                    HealthSortField::Severity,
                    HealthSortField::Path,
                    HealthSortField::Platform,
                    HealthSortField::State,
                    HealthSortField::Reason,
                ] {
                    ui.selectable_value(sort_field, field, field.to_string());
                }
            });
        ui.checkbox(sort_ascending, "Ascending");
    });

    let visible = visible_health_issue_indices(issues, filters, *sort_field, *sort_ascending);
    ui.horizontal_wrapped(|ui| {
        summary_value(ui, "Issues shown", visible.len());
        summary_value(ui, "Total issues", issues.len());
    });
    ui.separator();

    if issues.is_empty() {
        ui.label("No archive health issues were found.");
        return action;
    }
    if visible.is_empty() {
        ui.label("No issues match the current health filters.");
        return action;
    }

    ui.strong("Health issues");
    egui::ScrollArea::vertical()
        .id_salt("health_issue_list")
        .max_height(220.0)
        .show(ui, |ui| {
            for &index in &visible {
                let issue = &issues[index];
                let selected = selected_issue.as_ref() == Some(&issue.path);
                let label_text = format!(
                    "{} — {} — {} — {}",
                    issue.path.display(),
                    issue.platform.as_deref().unwrap_or("Unknown"),
                    issue.category.label(),
                    issue.reason
                );
                // Path + platform + category + reason combined can easily
                // exceed the list's width - `.truncate()` keeps the row
                // itself from stretching the page, and the hover tooltip
                // (this list previously had none at all) always carries
                // every field in full.
                let response = ui
                    .add(egui::Button::selectable(selected, &label_text).truncate())
                    .on_hover_text(&label_text);
                if response.clicked() {
                    *selected_issue = Some(issue.path.clone());
                }
                // Requirement: "Do not invent retry actions for issues
                // that have no safe retry" - reuses `issue.recovery_action`
                // exactly as computed by `build_health_issues`/rendered by
                // the "Selected health issue" panel below, never a second
                // guess at what is safe to retry.
                response.context_menu(|ui| {
                    if ui.button("Show archive in Library").clicked() {
                        action = Some(HealthDashboardAction::ViewInLibrary(issue.path.clone()));
                        ui.close();
                    }
                    let inspectable = is_inspectable(&issue.path);
                    if ui
                        .add_enabled(inspectable, egui::Button::new("Inspect contents"))
                        .clicked()
                    {
                        action = Some(HealthDashboardAction::Inspect(issue.path.clone()));
                        ui.close();
                    }
                    if ui.button("Copy archive path").clicked() {
                        let _ = clipboard.set_text(issue.path.display().to_string());
                        ui.close();
                    }
                    if ui.button("Copy issue reason").clicked() {
                        let _ = clipboard.set_text(issue.reason.clone());
                        ui.close();
                    }
                    match issue.recovery_action {
                        Some(RecoveryAction::RetryMount) => {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Retry mount"))
                                .clicked()
                            {
                                action = Some(HealthDashboardAction::Archive(OperationRequest {
                                    action: ArchiveAction::Mount,
                                    archive_path: issue.path.clone(),
                                    cleanup_after_unmount: false,
                                }));
                                ui.close();
                            }
                        }
                        Some(RecoveryAction::Remount) => {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Remount"))
                                .clicked()
                            {
                                action = Some(HealthDashboardAction::Archive(OperationRequest {
                                    action: ArchiveAction::Remount,
                                    archive_path: issue.path.clone(),
                                    cleanup_after_unmount: false,
                                }));
                                ui.close();
                            }
                        }
                        Some(RecoveryAction::LazyUnmount) | None => {}
                    }
                    ui.separator();
                    if ui.button("Filter by this issue type").clicked() {
                        action = Some(HealthDashboardAction::FilterByCategory(
                            health_issue_filter_for_category(issue.category),
                        ));
                        ui.close();
                    }
                });
            }
        });

    let Some(issue) = selected_issue.as_ref().and_then(|selected| {
        visible
            .iter()
            .map(|&index| &issues[index])
            .find(|issue| issue.path == *selected)
    }) else {
        ui.label("Select a health issue to view details.");
        return action;
    };

    ui.separator();
    ui.strong("Selected health issue");
    let persisted = cached
        .archives
        .iter()
        .find(|persisted| persisted.absolute_path == issue.path);
    egui::Grid::new("selected_health_issue_details")
        .num_columns(2)
        .show(ui, |ui| {
            detail_row(ui, "Archive path", &issue.path.display().to_string());
            detail_row(
                ui,
                "Platform",
                issue.platform.as_deref().unwrap_or("Unknown"),
            );
            if let Some(persisted) = persisted
                && let Some(details) = cached.platform_details.get(&persisted.id)
            {
                for (label, value) in platform_provenance_lines(details) {
                    detail_row(ui, label, &value);
                }
            }
            detail_row(ui, "State", &health_issue_state_text(issue));
            detail_row(ui, "Present", if issue.present { "Yes" } else { "No" });
            detail_row(ui, "Health classification", issue.category.label());
            detail_row(ui, "Reason", &issue.reason);
            detail_row(ui, "Retryable", if issue.retryable { "Yes" } else { "No" });
            detail_row(
                ui,
                "Recovery available",
                if issue.recovery_available() {
                    "Yes"
                } else {
                    "No"
                },
            );
            detail_row(
                ui,
                "Last seen",
                issue.last_seen_at.as_deref().unwrap_or("Unknown"),
            );
            detail_row(ui, "Size", &format_duplicate_size(issue.size_bytes));
            detail_row(
                ui,
                "Modified time",
                &format_modified_time(issue.modified_time_unix_seconds),
            );
        });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        match issue.recovery_action {
            Some(RecoveryAction::RetryMount) => {
                if ui
                    .add_enabled(!busy, egui::Button::new("Retry mount"))
                    .on_hover_text("Retries mounting this archive, exactly like the Library's own Mount button.")
                    .clicked()
                {
                    action = Some(HealthDashboardAction::Archive(OperationRequest {
                        action: ArchiveAction::Mount,
                        archive_path: issue.path.clone(),
                        cleanup_after_unmount: false,
                    }));
                }
            }
            Some(RecoveryAction::Remount) => {
                if ui
                    .add_enabled(!busy, egui::Button::new("Remount"))
                    .on_hover_text("Remounts this archive, exactly like the Library's own Remount button.")
                    .clicked()
                {
                    action = Some(HealthDashboardAction::Archive(OperationRequest {
                        action: ArchiveAction::Remount,
                        archive_path: issue.path.clone(),
                        cleanup_after_unmount: false,
                    }));
                }
            }
            Some(RecoveryAction::LazyUnmount) | None => {}
        }
        if ui
            .button("View in Library")
            .on_hover_text(
                "Selects this archive in the Library view, where its full details and \
                 actions (including confirmation-guarded ones) are already available.",
            )
            .clicked()
        {
            action = Some(HealthDashboardAction::ViewInLibrary(issue.path.clone()));
        }
    });

    action
}

pub(super) struct LoadedViewState<'a> {
    pub(super) filter: &'a mut String,
    pub(super) filtered_rows: &'a mut Option<Vec<usize>>,
    pub(super) selected_archive: &'a mut Option<PathBuf>,
    pub(super) operation: Option<&'a RunningOperation>,
    pub(super) busy: bool,
    /// Why archive actions (Mount/Unmount/Lazy Unmount/Remount) are
    /// blocked for the selected live archive, if they are - see
    /// `archive_action_block_reason`, the single source of truth this is
    /// always derived from. `None` whenever actions are available.
    pub(super) block_reason: Option<&'static str>,
    /// The full boolean-by-boolean breakdown behind `block_reason` - see
    /// `action_readiness_debug_lines`. Rendered in an always-present,
    /// collapsed-by-default "Debug: action readiness" section.
    pub(super) action_readiness_debug_lines: &'a [String],
    pub(super) feedback: Option<&'a ActionFeedback>,
    pub(super) confirm_unmount: &'a mut Option<PathBuf>,
    pub(super) confirm_lazy_unmount: &'a mut Option<PathBuf>,
    pub(super) confirm_lazy_unmount_final: &'a mut Option<PathBuf>,
    pub(super) confirm_mount_all: &'a mut Option<MountAllConfirmation>,
    pub(super) focus_mount_all_cancel: &'a mut bool,
    pub(super) mount_all_typed_count: &'a mut String,
    pub(super) confirm_unmount_all: &'a mut Option<UnmountAllConfirmation>,
    pub(super) focus_unmount_all_cancel: &'a mut bool,
    pub(super) unmount_all_typed_count: &'a mut String,
    pub(super) confirm_unmount_selected: &'a mut Option<UnmountSelectedConfirmation>,
    pub(super) focus_unmount_selected_cancel: &'a mut bool,
    /// Row-context-menu "Mount selected" confirmation - previously
    /// dispatched with no confirmation at all (asymmetric with "Unmount
    /// selected", which already had one). The paths are re-derived fresh
    /// from `data.records` at confirm time, never trusted stale.
    pub(super) confirm_mount_selected: &'a mut Option<Vec<PathBuf>>,
    pub(super) focus_mount_selected_cancel: &'a mut bool,
    pub(super) mount_selected_typed_count: &'a mut String,
    /// Bulk platform assignment/clear confirmation - previously dispatched
    /// instantly with no confirmation at all, from both the selection
    /// action bar and the row context menu.
    pub(super) confirm_bulk_platform_action: &'a mut Option<(Vec<PathBuf>, BulkPlatformActionKind)>,
    pub(super) focus_bulk_platform_cancel: &'a mut bool,
    pub(super) bulk_platform_action_typed_count: &'a mut String,
    pub(super) focus_lazy_cancel: &'a mut bool,
    pub(super) focus_final_lazy_cancel: &'a mut bool,
    pub(super) lazy_unmount_offers: &'a HashSet<PathBuf>,
    pub(super) remount_offers: &'a HashSet<PathBuf>,
    pub(super) cleanup_after_unmount: &'a mut bool,
    pub(super) mount_all_result: Option<&'a MountAllResult>,
    pub(super) unmount_all_result: Option<&'a UnmountAllResult>,
    pub(super) history: &'a mut OperationHistory,
    pub(super) cached: Option<&'a CachedLibrarySnapshot>,
    pub(super) library_filters: &'a mut LibraryRowFilters,
    pub(super) platform_choice: &'a mut Option<String>,
    pub(super) platform_custom_text: &'a mut String,
    pub(super) platform_busy: bool,
    /// Shared RetroArch profile discovery state. Navigation itself is
    /// gated only by exact archive identity; the destination page uses
    /// this state to present scan, eligible, and blocked profile states.
    pub(super) retroarch_profiles: &'a RetroArchProfilesState,
    pub(super) selected_archives: &'a mut HashSet<PathBuf>,
    pub(super) bulk_platform_choice: &'a mut Option<String>,
    pub(super) bulk_platform_busy: bool,
    pub(super) missing_removal_available: bool,
    pub(super) missing_removal_busy: bool,
    pub(super) confirm_remove_missing: &'a mut Option<Vec<PathBuf>>,
    pub(super) missing_removal_typed_count: &'a mut String,
    pub(super) sort_field: &'a mut Option<SortField>,
    pub(super) sort_ascending: &'a mut bool,
    pub(super) library_scroll_offset: &'a mut f32,
    pub(super) clipboard: &'a mut dyn ClipboardBackend,
    /// A one-shot signal from the Library menu's "Select all visible" item;
    /// see `ArchiveFsApp::select_all_visible_requested`'s doc comment.
    /// Consumed and cleared the same frame it is seen.
    pub(super) select_all_visible_requested: &'a mut bool,
    /// The Library page's Source filter - see
    /// `ArchiveFsApp::library_source_filter`'s doc comment for what each
    /// of the three states means.
    pub(super) library_source_filter: &'a mut Option<Option<PathBuf>>,
    /// The Library table's resizable Archive path / Mount path column
    /// widths - see `LibraryColumnWidths`.
    pub(super) library_column_widths: &'a mut LibraryColumnWidths,
    pub(super) library_views_configured: bool,
    pub(super) library_view_last_plan: Option<&'a (LibraryViewConfig, LibraryViewPlan)>,
    pub(super) recent_scan: Option<&'a RecentScanAdditions>,
    pub(super) recent_view: bool,
    /// The Library platform strip's search box - narrows the wrapped chip
    /// list to matching platform names when a library detects dozens of
    /// distinct platforms. Empty means show every platform.
    pub(super) library_platform_query: &'a mut String,
}

pub(super) const REMOVE_MISSING_CANCEL_LABEL: &str = "Cancel";
pub(super) const REMOVE_MISSING_CONFIRM_LABEL: &str = "Remove Missing Entries";

pub(super) fn set_missing_review_mode(filters: &mut LibraryRowFilters, enabled: bool) {
    filters.missing = enabled;
    if enabled {
        filters.present = false;
        filters.awaiting_validation = false;
    }
}

pub(super) fn selected_missing_paths(
    cached: Option<&CachedLibrarySnapshot>,
    selected_archives: &HashSet<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    if selected_archives.is_empty() {
        return Err("Select one or more missing catalogue entries first.".to_string());
    }
    let cached = cached.ok_or_else(|| "The library database is unavailable.".to_string())?;
    let mut paths: Vec<PathBuf> = selected_archives.iter().cloned().collect();
    paths.sort();
    for path in &paths {
        let archive = cached
            .archives
            .iter()
            .find(|archive| archive.absolute_path == *path)
            .ok_or_else(|| {
                format!(
                    "{} is not an exact stored catalogue path. Nothing was removed.",
                    path.display()
                )
            })?;
        if archive.last_verified_missing_at.is_none() {
            return Err(format!(
                "{} is currently present. Only missing catalogue entries can be removed; nothing was removed.",
                path.display()
            ));
        }
    }
    Ok(paths)
}

pub(super) fn missing_removal_confirmation_text(count: usize) -> String {
    format!(
        "Remove {count} missing entr{} from the EmuWiz catalogue?\n\n\
         This removes only EmuWiz database records.\n\
         It will not delete archive files or mounted contents.\n\
         Entries will return if the archives are found in a later scan.",
        if count == 1 { "y" } else { "ies" }
    )
}
