//! Library page (and Recently Found, which shares this exact renderer)
//! presentation: the row table, its header/sort/keyboard navigation,
//! per-row and bulk context menus, selection controls, and the top-level
//! `show_loaded_data` that ties them together with the Selected archive
//! panel (`selected_game_panel.rs`). Extracted verbatim from `main.rs`
//! (2026-08-22, GUI extraction pass 3).
//!
//! Deliberately excludes mount-decision functions this code calls that are
//! genuinely shared with other pages (`available_action`,
//! `individual_actions_available`, `selected_persisted_archive`) and
//! `ArrowDirection`, which the unrelated Cheat Archive Picker's own
//! keyboard navigation also uses - all stayed in `main.rs`.

use super::*;

pub(crate) fn show_loaded_data(
    ui: &mut egui::Ui,
    data: &LoadedData,
    view_state: LoadedViewState<'_>,
) -> Option<AppOperationRequest> {
    let LoadedViewState {
        filter,
        filtered_rows,
        selected_archive,
        operation,
        busy,
        block_reason,
        action_readiness_debug_lines,
        feedback,
        confirm_unmount,
        confirm_lazy_unmount,
        confirm_lazy_unmount_final,
        confirm_mount_all,
        focus_mount_all_cancel,
        mount_all_typed_count,
        confirm_unmount_all,
        focus_unmount_all_cancel,
        unmount_all_typed_count,
        confirm_unmount_selected,
        focus_unmount_selected_cancel,
        confirm_mount_selected,
        focus_mount_selected_cancel,
        mount_selected_typed_count,
        confirm_bulk_platform_action,
        focus_bulk_platform_cancel,
        bulk_platform_action_typed_count,
        focus_lazy_cancel,
        focus_final_lazy_cancel,
        lazy_unmount_offers,
        remount_offers,
        cleanup_after_unmount,
        mount_all_result,
        unmount_all_result,
        cached,
        library_filters,
        history,
        platform_choice,
        platform_custom_text,
        platform_busy,
        retroarch_profiles,
        selected_archives,
        bulk_platform_choice,
        bulk_platform_busy,
        missing_removal_available,
        missing_removal_unavailable_reason,
        missing_removal_busy,
        confirm_remove_missing,
        missing_removal_typed_count,
        sort_field,
        sort_ascending,
        library_scroll_offset,
        clipboard,
        select_all_visible_requested,
        library_source_filter,
        library_column_widths,
        library_views_configured,
        library_view_last_plan,
        recent_scan,
        recent_view,
        library_platform_query,
    } = view_state;
    let mut requested_action = None;
    let pending_count = data.stats.pending_count;
    let mounted_count = data.stats.mounted_count;
    if recent_view {
        widgets::page_header_with_icon(
            ui,
            crate::ui::icons::RECENT,
            "Recently Found",
            "Persistent additions from the most recent completed scan.",
        );
    }
    if let Some(recent) = recent_scan {
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("Scan {}", recent.scan.scan_run_id));
                ui.label(format!("Added {}", recent.scan.archives_added));
                ui.label(format!("Updated {}", recent.scan.archives_updated));
                ui.label(format!(
                    "Skipped {}",
                    recent.scan.skipped_unsupported_extension
                        + recent.scan.skipped_ambiguous_platform
                ));
                ui.label(format!("Errors {}", recent.scan.errors_count));
            });
            if recent.truncated {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Recently Found reached its 10,000-entry display bound.",
                );
            }
        });
    }
    // Merged rows are rebuilt fresh every frame (cheap for realistic
    // library sizes, and always exactly consistent with the current
    // self.state/self.database_state - see build_display_rows). Only the
    // *cached* filtered_rows index list is invalidated on the discrete
    // events that actually change this merge (poll_load, poll_database_load),
    // not every frame - see ArchiveFsApp::poll_load/poll_database_load.
    // Hoisted above every other use (including the "Selected archive"
    // panel, which renders higher up the page than the library table
    // itself) so the Source filter/owning-source display and the table
    // below always agree on exactly one merged row list.
    let merged_rows = build_display_rows(&data.records, &data.rows, cached);
    let platform_counts = detected_platform_counts(
        merged_rows
            .iter()
            .map(|row| (!row.unknown_platform).then_some(row.platform.as_str())),
    );
    // Wrapping, not a fixed-width strip or a horizontal scroll area: a
    // real library can detect dozens of distinct platforms, and every one
    // of them with a non-zero count must stay reachable without scrolling
    // past visible controls (unlike the previous fixed six-tab list).
    //
    // A search box only appears once the platform count is large enough
    // that scanning the wrapped chips by eye stops being practical - below
    // that threshold it would just be one more control competing for space
    // for no benefit.
    const PLATFORM_SEARCH_THRESHOLD: usize = 10;
    if platform_counts.named.len() >= PLATFORM_SEARCH_THRESHOLD {
        ui.horizontal(|ui| {
            ui.label("Find platform:");
            ui.add(
                egui::TextEdit::singleline(library_platform_query)
                    .id_salt("library_platform_query")
                    .desired_width(200.0)
                    .hint_text("Type to narrow the list below"),
            );
            if !library_platform_query.is_empty() && ui.small_button("Clear").clicked() {
                library_platform_query.clear();
            }
        });
        ui.add_space(4.0);
    } else {
        library_platform_query.clear();
    }
    let platform_query = library_platform_query.to_lowercase();
    ui.horizontal_wrapped(|ui| {
        let all_selected = library_filters.platform.is_none();
        if platform_query.is_empty()
            && ui
                .selectable_label(all_selected, format!("All ({})", merged_rows.len()))
                .clicked()
            && !all_selected
        {
            library_filters.platform = None;
            *selected_archive = None;
            // Consistent with the named-platform and Unknown branches below
            // (docs/GUI_NAVIGATION_RESET_DESIGN.md mandatory risk #3): a
            // platform-selection change must never leave a stale
            // multi-selection behind just because this was the "All"
            // branch specifically.
            selected_archives.clear();
            filtered_rows.take();
        }
        for (platform, count) in &platform_counts.named {
            if !platform_query.is_empty() && !platform.to_lowercase().contains(&platform_query) {
                continue;
            }
            let selected = library_filters.platform.as_deref() == Some(platform.as_str());
            if ui
                .selectable_label(selected, format!("{platform} ({count})"))
                .clicked()
                && !selected
            {
                library_filters.platform = Some(platform.clone());
                *selected_archive = None;
                selected_archives.clear();
                filtered_rows.take();
            }
        }
        if platform_counts.unknown > 0
            && (platform_query.is_empty() || "unknown".contains(&platform_query))
        {
            let selected = library_filters.platform.as_deref() == Some("Unknown");
            if ui
                .selectable_label(selected, format!("Unknown ({})", platform_counts.unknown))
                .clicked()
                && !selected
            {
                library_filters.platform = Some("Unknown".to_string());
                *selected_archive = None;
                selected_archives.clear();
                filtered_rows.take();
            }
        }
    });
    ui.add_space(4.0);
    // Natural-height summary: it may grow only with content visible now;
    // no persisted panel height can starve the result table on a later
    // frame or after a window resize.
    ui.horizontal_wrapped(|ui| {
        summary_value(ui, "Total archives", data.stats.total_archives);
        summary_value(ui, "Mounted", data.stats.mounted_count);
        summary_value(ui, "Pending", data.stats.pending_count);
        if widgets::action_button(
            ui,
            "Mount all",
            widgets::ActionStyle::Primary,
            mount_all_available(pending_count, busy),
        )
        .clicked()
        {
            *confirm_mount_all = Some(MountAllConfirmation);
            *focus_mount_all_cancel = true;
            history.record(HistoryEntry::new(
                ActivityAction::MountAll,
                None,
                ActivityOutcome::Offered,
                format!("Mount All offered for {} pending archives.", pending_count),
            ));
        }
        if widgets::action_button(
            ui,
            "Unmount all",
            widgets::ActionStyle::Destructive,
            mounted_count > 0 && !busy,
        )
        .clicked()
        {
            *confirm_unmount_all = Some(UnmountAllConfirmation);
            *focus_unmount_all_cancel = true;
            history.record(HistoryEntry::new(
                ActivityAction::UnmountAll,
                None,
                ActivityOutcome::Offered,
                format!("Unmount All offered for {mounted_count} mounted archives."),
            ));
        }
        ui.separator();
        let (readiness, tone) = if data.doctor.is_ready() {
            ("Doctor ready", widgets::StatusTone::Success)
        } else {
            ("Doctor needs attention", widgets::StatusTone::Warning)
        };
        widgets::status_badge(ui, readiness, tone);
    });

    // The focused-archive mount / context actions render here, directly
    // under the bulk "Mount all" / "Unmount all" / "Doctor ready" row, so
    // every mount action - bulk and per-archive - sits in one place instead
    // of the focused-archive controls floating alone in the middle of the
    // page between the results banners and the filter card (manual smoke
    // feedback). Collapsed by default; state persists for the session.
    let selected_persisted = selected_persisted_archive(cached, selected_archive.as_deref());
    let selected_source_path = selected_row_index(&merged_rows, selected_archive.as_deref())
        .and_then(|index| merged_rows[index].source_path.as_deref());
    let selected_actions = if let Some(path) = selected_archive.as_deref() {
        egui::CollapsingHeader::new(format!("Focused archive · {}", path.display()))
            .id_salt("library_focused_archive_details")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("library_selected_archive_scroll")
                    .max_height((ui.available_height() * 0.35).clamp(120.0, 280.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        show_selected_archive(
                            ui,
                            selected_record(&data.records, selected_archive.as_deref()),
                            selected_persisted,
                            selected_platform_details(cached, selected_persisted),
                            selected_source_path,
                            SelectedArchiveViewState {
                                operation,
                                busy,
                                block_reason,
                                action_readiness_debug_lines,
                                confirm_unmount,
                                confirm_lazy_unmount,
                                focus_lazy_cancel,
                                lazy_unmount_offers,
                                remount_offers,
                                cleanup_after_unmount,
                                platform_choice,
                                platform_custom_text,
                                platform_busy,
                                clipboard,
                            },
                        )
                    })
                    .inner
            })
            .body_returned
            .unwrap_or_default()
    } else {
        ui.weak("No focused archive. Select a Library row to establish workflow context.");
        SelectedArchiveActions::default()
    };
    if let Some(request) = selected_actions.operation {
        requested_action = Some(AppOperationRequest::Archive(request));
    }
    if let Some((archive_path, action)) = selected_actions.platform {
        requested_action = Some(AppOperationRequest::PlatformAssignment {
            archive_path,
            action,
        });
    }
    if let Some(archive_path) = selected_actions.inspect {
        requested_action = Some(AppOperationRequest::InspectArchive(archive_path));
    }
    if let Some(archive_path) = selected_actions.cheats_mods {
        requested_action = Some(AppOperationRequest::OpenCheatsMods(archive_path));
    }

    if let Some(result) = mount_all_result {
        show_mount_all_result(ui, result);
    }
    if let Some(result) = unmount_all_result {
        show_unmount_all_result(ui, result);
    }

    if let Some(feedback) = feedback {
        ui.separator();
        let color = if feedback.succeeded {
            egui::Color32::from_rgb(70, 170, 90)
        } else {
            ui.visuals().error_fg_color
        };
        ui.colored_label(color, &feedback.message);
        if let Some(warning) = &feedback.warning {
            ui.colored_label(egui::Color32::from_rgb(210, 140, 40), warning);
        }
        if let Some(more_information) = &feedback.more_information {
            widgets::technical_details(
                ui,
                (
                    "action_feedback_more_information",
                    more_information.as_str(),
                ),
                |ui| {
                    ui.label(more_information);
                },
            );
        }
        if let Some(cleanup) = &feedback.cleanup {
            let color = if cleanup.succeeded {
                egui::Color32::from_rgb(70, 170, 90)
            } else {
                ui.visuals().error_fg_color
            };
            ui.colored_label(color, &cleanup.message);
        }
    }
    if confirm_mount_all.is_some() {
        let actions_available = !busy;
        widgets::centered_window("Mount All pending archives?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "{} pending archives will be mounted under {}.",
                    pending_count,
                    data.mount_root.display()
                ));
                ui.label(
                    "Archives are mounted one at a time. Large libraries may take several minutes.",
                );
                ui.label("A failure will be recorded, and later archives will still be attempted.");
                ui.add_space(8.0);
                let confirm_enabled = show_bulk_action_typed_count_gate(
                    ui,
                    pending_count,
                    mount_all_typed_count,
                    mount_all_available(pending_count, busy),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = ui.add_enabled(
                        actions_available,
                        egui::Button::new("Cancel").fill(ui.visuals().selection.bg_fill),
                    );
                    if *focus_mount_all_cancel {
                        cancel.request_focus();
                        *focus_mount_all_cancel = false;
                    }
                    if cancel.clicked() {
                        history.record(HistoryEntry::new(
                            ActivityAction::MountAll,
                            None,
                            ActivityOutcome::Cancelled,
                            "Mount All cancelled before starting.",
                        ));
                        *confirm_mount_all = None;
                        mount_all_typed_count.clear();
                    }
                    if ui
                        .add_enabled(confirm_enabled, egui::Button::new("Mount All"))
                        .clicked()
                    {
                        // Re-derived fresh from the live snapshot at the
                        // moment of confirmation, never the possibly-stale
                        // count the dialog opened with (decision 1's
                        // "re-derive eligibility immediately before
                        // execution").
                        requested_action = Some(AppOperationRequest::MountAll(
                            pending_mount_items(&data.records),
                        ));
                        *confirm_mount_all = None;
                        mount_all_typed_count.clear();
                    }
                });
            });
    }

    if confirm_unmount_all.is_some() {
        let actions_available = !busy;
        widgets::centered_window("Unmount All mounted archives?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "{mounted_count} mounted archives under {} will be unmounted one at a time.",
                    data.mount_root.display()
                ));
                ui.label("Close applications using these mounts before continuing. Files that are still open may prevent normal unmounting.");
                ui.label("Close emulators, file managers, terminals, media players, and other applications using mounted files.");
                ui.label("A failure will be recorded, and later archives will still be attempted.");
                ui.label(format!(
                    "Cleanup after each successful unmount: {}.",
                    if *cleanup_after_unmount { "enabled" } else { "disabled" }
                ));
                ui.add_space(8.0);
                let confirm_enabled = show_bulk_action_typed_count_gate(
                    ui,
                    mounted_count,
                    unmount_all_typed_count,
                    mounted_count > 0 && !busy,
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = ui.add_enabled(
                        actions_available,
                        egui::Button::new("Cancel").fill(ui.visuals().selection.bg_fill),
                    );
                    if *focus_unmount_all_cancel {
                        cancel.request_focus();
                        *focus_unmount_all_cancel = false;
                    }
                    if cancel.clicked() {
                        history.record(HistoryEntry::new(
                            ActivityAction::UnmountAll,
                            None,
                            ActivityOutcome::Cancelled,
                            "Unmount All cancelled before starting.",
                        ));
                        *confirm_unmount_all = None;
                        unmount_all_typed_count.clear();
                    }
                    if ui
                        .add_enabled(confirm_enabled, egui::Button::new("Unmount All"))
                        .clicked()
                    {
                        requested_action = Some(AppOperationRequest::UnmountAll {
                            items: pending_unmount_items(&data.records),
                            cleanup_after_unmount: *cleanup_after_unmount,
                        });
                        *confirm_unmount_all = None;
                        unmount_all_typed_count.clear();
                    }
                });
            });
    }

    if confirm_unmount_selected.is_some() {
        let mounted_selected = mounted_selected_unmount_items(&data.records, selected_archives);
        let mounted_selected_count = mounted_selected.len();
        let selected_count = selected_archives.len();
        let actions_available = !busy;
        widgets::centered_window("Unmount selected mounted archives?")
            .collapsible(false)
            .resizable(false)
            .default_width(700.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                // Keep the decision controls reachable even on a short viewport. Long safety
                // copy may scroll, but confirmation and cancellation must never be clipped.
                let detail_height = (ui.ctx().input(|input| input.screen_rect().height()) * 0.55)
                    .clamp(80.0, 360.0);
                egui::ScrollArea::vertical()
                    .id_salt("unmount_selected_confirmation_details")
                    .max_height(detail_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.label(format!(
                            "{mounted_selected_count} of the {selected_count} selected archives \
                             are currently mounted."
                        ));
                        ui.label(format!(
                            "Only those {mounted_selected_count} mounted archives will be \
                             unmounted, one at a time. The rest of the selection is not currently \
                             mounted and will not be touched."
                        ));
                        ui.label("Close applications using these mounts before continuing. Files that are still open may prevent normal unmounting.");
                        ui.label("A failure will be recorded, and later archives will still be attempted.");
                        ui.label(format!(
                            "Cleanup after each successful unmount: {}.",
                            if *cleanup_after_unmount {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        ));
                        ui.label(
                            "Original archive files will not be deleted or modified - \
                             unmounting only detaches the read-only mount, it never touches the \
                             archive itself.",
                        );
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = ui.add_enabled(
                        actions_available,
                        egui::Button::new("Cancel").fill(ui.visuals().selection.bg_fill),
                    );
                    if *focus_unmount_selected_cancel {
                        cancel.request_focus();
                        *focus_unmount_selected_cancel = false;
                    }
                    if cancel.clicked() {
                        history.record(HistoryEntry::new(
                            ActivityAction::UnmountAll,
                            None,
                            ActivityOutcome::Cancelled,
                            "Unmount selected cancelled before starting.",
                        ));
                        *confirm_unmount_selected = None;
                    }
                    if ui
                        .add_enabled(
                            mounted_selected_count > 0 && !busy,
                            egui::Button::new("Unmount selected"),
                        )
                        .clicked()
                    {
                        requested_action = Some(AppOperationRequest::UnmountAll {
                            items: mounted_selected,
                            cleanup_after_unmount: *cleanup_after_unmount,
                        });
                        *confirm_unmount_selected = None;
                    }
                });
            });
    }

    if let Some(paths) = confirm_mount_selected.clone() {
        // Re-derived fresh every frame the dialog is open, not just at
        // confirm time - so the count shown in the preview never
        // disagrees with what "Mount selected" will actually do
        // (decision 1's "re-derive eligibility immediately before
        // execution").
        let items = mount_all_items_for_paths(&data.records, &paths);
        let count = items.len();
        let actions_available = !busy;
        widgets::centered_window("Mount selected pending archives?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "{count} of the {} selected archives are ready to mount under {}.",
                    paths.len(),
                    data.mount_root.display()
                ));
                ui.label(
                    "Only those pending archives will be mounted; the rest of the selection is \
                     not currently eligible and will not be touched.",
                );
                ui.add_space(8.0);
                let confirm_enabled = show_bulk_action_typed_count_gate(
                    ui,
                    count,
                    mount_selected_typed_count,
                    actions_available && count > 0,
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = ui.add_enabled(
                        actions_available,
                        egui::Button::new("Cancel").fill(ui.visuals().selection.bg_fill),
                    );
                    if *focus_mount_selected_cancel {
                        cancel.request_focus();
                        *focus_mount_selected_cancel = false;
                    }
                    if cancel.clicked() {
                        history.record(HistoryEntry::new(
                            ActivityAction::MountAll,
                            None,
                            ActivityOutcome::Cancelled,
                            "Mount selected cancelled before starting.",
                        ));
                        *confirm_mount_selected = None;
                        mount_selected_typed_count.clear();
                    }
                    if ui
                        .add_enabled(confirm_enabled, egui::Button::new("Mount selected"))
                        .clicked()
                    {
                        requested_action = Some(AppOperationRequest::MountAll(
                            mount_all_items_for_paths(&data.records, &paths),
                        ));
                        *confirm_mount_selected = None;
                        mount_selected_typed_count.clear();
                    }
                });
            });
    }

    if let Some((archive_paths, kind)) = confirm_bulk_platform_action.clone() {
        let count = archive_paths.len();
        let actions_available = !bulk_platform_busy;
        let description = match &kind {
            BulkPlatformActionKind::Set(platform) => {
                format!("Set the platform of {count} selected archives to {platform}.")
            }
            BulkPlatformActionKind::Clear => {
                format!("Clear the manually assigned platform of {count} selected archives.")
            }
        };
        widgets::centered_window("Change platform for selected archives?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(description);
                ui.add_space(8.0);
                let confirm_enabled = show_bulk_action_typed_count_gate(
                    ui,
                    count,
                    bulk_platform_action_typed_count,
                    actions_available && count > 0,
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = ui.add_enabled(
                        actions_available,
                        egui::Button::new("Cancel").fill(ui.visuals().selection.bg_fill),
                    );
                    if *focus_bulk_platform_cancel {
                        cancel.request_focus();
                        *focus_bulk_platform_cancel = false;
                    }
                    if cancel.clicked() {
                        *confirm_bulk_platform_action = None;
                        bulk_platform_action_typed_count.clear();
                    }
                    if ui
                        .add_enabled(confirm_enabled, egui::Button::new("Confirm change"))
                        .clicked()
                    {
                        requested_action = Some(AppOperationRequest::BulkPlatformAssignment {
                            archive_paths: archive_paths.clone(),
                            kind: kind.clone(),
                        });
                        *confirm_bulk_platform_action = None;
                        bulk_platform_action_typed_count.clear();
                    }
                });
            });
    }

    if let Some(archive_path) = confirm_lazy_unmount.clone() {
        let actions_available =
            lazy_confirmation_available(&archive_path, lazy_unmount_offers, busy);
        widgets::centered_window("Use Lazy Unmount?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(LAZY_UNMOUNT_WARNING);
                ui.label(archive_path.display().to_string());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = ui.add_enabled(
                        actions_available,
                        egui::Button::new("Cancel").fill(ui.visuals().selection.bg_fill),
                    );
                    if *focus_lazy_cancel {
                        cancel.request_focus();
                        *focus_lazy_cancel = false;
                    }
                    if cancel.clicked() {
                        record_recovery_activity(
                            history,
                            ActivityAction::LazyUnmount,
                            &archive_path,
                            ActivityOutcome::Cancelled,
                            "User cancelled lazy unmount.",
                        );
                        *confirm_lazy_unmount = None;
                    }
                    if ui
                        .add_enabled(
                            actions_available,
                            egui::Button::new("Try Normal Unmount Again"),
                        )
                        .clicked()
                    {
                        record_recovery_activity(
                            history,
                            ActivityAction::Unmount,
                            &archive_path,
                            ActivityOutcome::Retried,
                            "Normal unmount retried.",
                        );
                        requested_action = Some(AppOperationRequest::Archive(OperationRequest {
                            action: ArchiveAction::Unmount,
                            archive_path: archive_path.clone(),
                            cleanup_after_unmount: *cleanup_after_unmount,
                        }));
                        *confirm_lazy_unmount = None;
                    }
                    if ui
                        .add_enabled(actions_available, egui::Button::new("Lazy Unmount"))
                        .clicked()
                    {
                        advance_to_final_lazy_confirmation(
                            confirm_lazy_unmount,
                            confirm_lazy_unmount_final,
                            focus_final_lazy_cancel,
                            &archive_path,
                        );
                    }
                });
            });
    }

    if let Some(archive_path) = confirm_lazy_unmount_final.clone() {
        let actions_available =
            lazy_confirmation_available(&archive_path, lazy_unmount_offers, busy);
        widgets::centered_window("Confirm Lazy Unmount")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label("This is the final confirmation. Close applications using this mount before continuing.");
                ui.label(archive_path.display().to_string());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let cancel = ui.add_enabled(
                        actions_available,
                        egui::Button::new("Cancel").fill(ui.visuals().selection.bg_fill),
                    );
                    if *focus_final_lazy_cancel {
                        cancel.request_focus();
                        *focus_final_lazy_cancel = false;
                    }
                    if cancel.clicked() {
                        record_recovery_activity(
                            history,
                            ActivityAction::LazyUnmount,
                            &archive_path,
                            ActivityOutcome::Cancelled,
                            "User cancelled lazy unmount.",
                        );
                        *confirm_lazy_unmount_final = None;
                    }
                    if ui
                        .add_enabled(actions_available, egui::Button::new("Confirm Lazy Unmount"))
                        .clicked()
                    {
                        record_recovery_activity(
                            history,
                            ActivityAction::LazyUnmount,
                            &archive_path,
                            ActivityOutcome::Confirmed,
                            "Lazy unmount confirmed.",
                        );
                        requested_action = Some(AppOperationRequest::Archive(OperationRequest {
                            action: ArchiveAction::LazyUnmount,
                            archive_path: archive_path.clone(),
                            cleanup_after_unmount: *cleanup_after_unmount,
                        }));
                        *confirm_lazy_unmount_final = None;
                    }
                });
            });
    }

    if let Some(archive_path) = confirm_unmount.clone() {
        let actions_available = confirmation_actions_available(busy);
        widgets::centered_window("Confirm unmount")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label("Unmount this archive?");
                ui.label(archive_path.display().to_string());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(actions_available, egui::Button::new("Cancel"))
                        .clicked()
                    {
                        *confirm_unmount = None;
                    }
                    if ui
                        .add_enabled(actions_available, egui::Button::new("Unmount"))
                        .clicked()
                    {
                        requested_action = Some(AppOperationRequest::Archive(OperationRequest {
                            action: ArchiveAction::Unmount,
                            archive_path: archive_path.clone(),
                            cleanup_after_unmount: *cleanup_after_unmount,
                        }));
                        *confirm_unmount = None;
                    }
                });
            });
    }

    ui.separator();

    // Requirement: the bulk platform action bar must render immediately
    // above the Search/filter controls, in the CentralPanel's ordinary
    // top-to-bottom flow - never after the table's ScrollAreas. Both
    // ScrollAreas below use `auto_shrink([false, false])`, which makes
    // them greedily claim *all* remaining vertical space in `ui`; a
    // widget placed after them in the same vertical layout would be
    // squeezed into whatever sliver of height (often zero) is left over,
    // which is why the bar previously never appeared despite a correct
    // selection count. Uses the exact same `selected_archives` `HashSet`
    // that `show_archive_rows` highlights rows from - never a second,
    // possibly-stale copy.
    if let Some(action) = show_bulk_platform_action_bar(
        ui,
        selected_archives,
        bulk_platform_choice,
        bulk_platform_busy,
    ) {
        // Previously dispatched instantly with no confirmation at all -
        // now gated the same way every other bulk action is (decisions
        // 1-3), and shares the exact same confirmation state as the row
        // context menu's "Set platform"/"Clear platform" so there is
        // exactly one bulk-platform confirmation dialog, not two.
        *confirm_bulk_platform_action = Some((selected_archives.iter().cloned().collect(), action));
        *focus_bulk_platform_cancel = true;
    }

    widgets::card(ui, |ui| {
        ui.strong("Find and filter");

        let mut filter_changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Search:");
            let search_width = (ui.available_width() - 72.0).clamp(180.0, 520.0);
            filter_changed |=
                show_text_edit_with_context_menu(ui, filter, clipboard, |text_edit| {
                    text_edit
                        .id(egui::Id::new(SEARCH_FILTER_TEXT_EDIT_ID))
                        .hint_text("archive, mount path, platform, or state")
                        .desired_width(search_width)
                })
                .changed();
            if !filter.is_empty() && ui.small_button("Clear").clicked() {
                filter.clear();
                filter_changed = true;
            }
        });
        if filter_changed {
            *filtered_rows = matching_row_indices(&merged_rows, filter);
        }

        egui::CollapsingHeader::new("More filters")
            .id_salt("library_more_filters")
            .default_open(false)
            .show(ui, |ui| {
                let unknown_count = merged_rows
                    .iter()
                    .filter(|row| row.unknown_platform)
                    .count();
                let mut filters_changed = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Show:");
                    filters_changed |= ui
                        .checkbox(&mut library_filters.present, "Present")
                        .changed();
                    filters_changed |= ui
                        .checkbox(&mut library_filters.missing, "Missing")
                        .changed();
                    filters_changed |= ui
                        .checkbox(
                            &mut library_filters.awaiting_validation,
                            "Awaiting validation",
                        )
                        .changed();
                    filters_changed |= ui
                        .checkbox(&mut library_filters.known_platform, "Known platform")
                        .changed();
                    filters_changed |= ui
                        .checkbox(
                            &mut library_filters.unknown_platform,
                            format!("Unknown platform ({unknown_count})"),
                        )
                        .changed();
                    if library_filters.is_active() && ui.small_button("Clear filters").clicked() {
                        *library_filters = LibraryRowFilters::default();
                        filters_changed = true;
                    }
                });
                let _ = filters_changed;

                if unknown_platform_banner_visible(library_filters, unknown_count) {
                    widgets::banner(
                        ui,
                        &unknown_platform_aggregate_headline(unknown_count),
                        UNKNOWN_PLATFORM_EXPLANATION,
                        widgets::StatusTone::Info,
                    );
                }

                let configured_sources: &[SourceFolderView] = cached
                    .map(|cached| cached.source_views.as_slice())
                    .unwrap_or(&[]);
                if !configured_sources.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Source:");
                        let selected_text = match library_source_filter {
                            None => "All sources".to_string(),
                            Some(None) => "Unassigned / Legacy".to_string(),
                            Some(Some(path)) => path.display().to_string(),
                        };
                        egui::ComboBox::from_id_salt("library_source_filter")
                            .selected_text(selected_text)
                            .width((ui.available_width() - 16.0).clamp(220.0, 520.0))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(library_source_filter, None, "All sources");
                                for source in configured_sources {
                                    ui.selectable_value(
                                        library_source_filter,
                                        Some(Some(source.path.clone())),
                                        source.path.display().to_string(),
                                    );
                                }
                                ui.selectable_value(
                                    library_source_filter,
                                    Some(None),
                                    "Unassigned / Legacy",
                                );
                            });
                    });
                }

                let missing_count = merged_rows
                    .iter()
                    .filter(|row| row.origin == RowOrigin::CachedMissing)
                    .count();
                let mut missing_only = library_filters.missing
                    && !library_filters.present
                    && !library_filters.awaiting_validation;
                let selected_missing = selected_missing_paths(cached, selected_archives);
                if !missing_only {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Missing catalogue entries: {missing_count}"));
                        if ui
                            .checkbox(&mut missing_only, "Show missing only")
                            .changed()
                        {
                            set_missing_review_mode(library_filters, missing_only);
                        }
                        let enabled = missing_removal_available && selected_missing.is_ok();
                        let response = ui
                            .add_enabled(enabled, egui::Button::new(REMOVE_MISSING_CONFIRM_LABEL));
                        if !enabled && let Err(reason) = &selected_missing {
                            response.clone().on_hover_text(reason);
                        }
                        if response.clicked()
                            && let Ok(paths) = &selected_missing
                        {
                            *confirm_remove_missing = Some(paths.clone());
                        }
                        if missing_removal_busy {
                            ui.spinner();
                            ui.label("Removing catalogue entries...");
                        }
                    });
                }
                if missing_only && missing_count > 0 {
                    // The action is repeated below the filter card when this
                    // review mode is active so it is visible without opening
                    // More filters. Keep the explanatory text here as well
                    // for the ordinary in-card transition into this mode.
                    ui.label("Showing missing entries only.");
                }

                if let Some(paths) = confirm_remove_missing.clone() {
                    let confirmation_selection: HashSet<PathBuf> = paths.iter().cloned().collect();
                    let still_valid =
                        selected_missing_paths(cached, &confirmation_selection).is_ok();
                    widgets::centered_window(format!(
                        "Remove {} missing catalogue entr{}?",
                        paths.len(),
                        if paths.len() == 1 { "y" } else { "ies" }
                    ))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        ui.label(missing_removal_confirmation_text(paths.len()));
                        ui.add_space(8.0);
                        let confirm_enabled = show_bulk_action_typed_count_gate(
                            ui,
                            paths.len(),
                            missing_removal_typed_count,
                            missing_removal_available && still_valid,
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button(REMOVE_MISSING_CANCEL_LABEL).clicked() {
                                *confirm_remove_missing = None;
                                missing_removal_typed_count.clear();
                            }
                            if ui
                                .add_enabled(
                                    confirm_enabled,
                                    egui::Button::new(REMOVE_MISSING_CONFIRM_LABEL),
                                )
                                .clicked()
                            {
                                requested_action =
                                    Some(AppOperationRequest::RemoveMissing(paths.clone()));
                                *confirm_remove_missing = None;
                                missing_removal_typed_count.clear();
                            }
                        });
                    });
                }
            });
    });
    let missing_count = merged_rows
        .iter()
        .filter(|row| row.origin == RowOrigin::CachedMissing)
        .count();
    let missing_only =
        library_filters.missing && !library_filters.present && !library_filters.awaiting_validation;
    if missing_only && missing_count > 0 {
        let selected_missing = selected_missing_paths(cached, selected_archives);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Missing catalogue entries: {missing_count}"));
            let enabled = missing_removal_available && selected_missing.is_ok();
            let response = ui.add_enabled(enabled, egui::Button::new(REMOVE_MISSING_CONFIRM_LABEL));
            if !enabled && let Err(reason) = &selected_missing {
                response.clone().on_hover_text(reason);
            }
            if response.clicked()
                && let Ok(paths) = &selected_missing
            {
                *confirm_remove_missing = Some(paths.clone());
            }
            if missing_removal_busy {
                ui.spinner();
                ui.label("Removing catalogue entries...");
            }
        });
        ui.add(
                egui::Label::new(
                    "Reviewing stale catalogue entries only. Removing them changes EmuWiz's catalogue, not original ROM/archive files, source folders, symlinks, or mounts. A file can be catalogued again after a later successful scan; removal has no persistent undo journal.",
                )
                .wrap(),
            );
        let selection_reason = selected_missing.as_ref().err().map(String::as_str);
        if let Some(reason) = missing_removal_disabled_reason(
            missing_removal_unavailable_reason.as_deref(),
            selected_archives.len(),
            selection_reason,
        ) {
            ui.weak(reason);
        }
    }
    ui.add_space(8.0);

    let base_indices: Vec<usize> = filtered_rows
        .clone()
        .unwrap_or_else(|| (0..merged_rows.len()).collect());
    let mut visible_indices: Vec<usize> = base_indices
        .into_iter()
        .filter(|&index| {
            !library_filters.is_active() || library_filters.matches(&merged_rows[index])
        })
        .filter(|&index| match library_source_filter {
            None => true,
            Some(wanted) => merged_rows[index].source_path.as_ref() == wanted.as_ref(),
        })
        .filter(|&index| {
            !recent_view
                || recent_scan
                    .is_some_and(|recent| recent_scan_contains(recent, &merged_rows[index].path))
        })
        .collect();
    if let Some(field) = *sort_field {
        sort_visible_indices(&merged_rows, &mut visible_indices, field, *sort_ascending);
    }
    let visible_count = visible_indices.len();

    show_selection_controls_row(ui, &merged_rows, &visible_indices, selected_archives);
    ui.add_space(4.0);
    if *select_all_visible_requested {
        *selected_archives = select_all_visible(&merged_rows, &visible_indices);
        *select_all_visible_requested = false;
    }

    let mut requested_scroll_pos: Option<usize> = None;
    if !keyboard_shortcuts_blocked_by_focus(ui.ctx()) {
        let (escape_pressed, select_all_pressed, arrow_down_pressed, arrow_up_pressed, ctrl_held) =
            ui.input(|input| {
                (
                    input.key_pressed(egui::Key::Escape),
                    input.modifiers.ctrl && input.key_pressed(egui::Key::A),
                    input.key_pressed(egui::Key::ArrowDown),
                    input.key_pressed(egui::Key::ArrowUp),
                    input.modifiers.ctrl,
                )
            });

        if escape_pressed {
            selected_archives.clear();
        }
        if select_all_pressed {
            *selected_archives = select_all_visible(&merged_rows, &visible_indices);
        }
        if arrow_down_pressed || arrow_up_pressed {
            let direction = if arrow_down_pressed {
                ArrowDirection::Down
            } else {
                ArrowDirection::Up
            };
            if let Some(new_focus) = next_focus_in_visible_order(
                &merged_rows,
                &visible_indices,
                selected_archive.as_deref(),
                direction,
            ) {
                apply_arrow_focus_change(
                    selected_archives,
                    selected_archive,
                    new_focus.clone(),
                    ctrl_held,
                );
                requested_scroll_pos = visible_indices
                    .iter()
                    .position(|&index| merged_rows[index].path == new_focus);
            }
        }
    }

    let row_height = fixed_row_height(
        ui.text_style_height(&egui::TextStyle::Body),
        ui.spacing().interact_size.y,
    );
    let horizontal_spacing = ui.spacing().item_spacing.x;
    let selected_index = selected_row_index(&merged_rows, selected_archive.as_deref());
    let table_message = if recent_view && recent_scan.is_none() {
        Some(LibraryTableMessage::NoCompletedScan)
    } else if recent_scan.is_some_and(|recent| recent.archives.is_empty()) {
        Some(LibraryTableMessage::NoRecentAdditions)
    } else {
        library_table_message(merged_rows.is_empty(), visible_count)
    };
    match table_message {
        Some(LibraryTableMessage::NoCompletedScan) => {
            widgets::empty_state(
                ui,
                &crate::ui::icons::with_icon(crate::ui::icons::RECENT, "No completed scan yet"),
                "Run a library scan to populate Recently Found.",
                None,
            );
        }
        Some(LibraryTableMessage::NoRecentAdditions) => {
            widgets::empty_state(
                ui,
                "No newly added files",
                "The most recent completed scan added no new library entries. Updated entries remain in the main Library.",
                None,
            );
        }
        Some(LibraryTableMessage::EmptyLibrary) => {
            widgets::empty_state(
                ui,
                &crate::ui::icons::with_icon(crate::ui::icons::GAMES, "No games yet"),
                EMPTY_LIBRARY_MESSAGE,
                None,
            );
        }
        Some(LibraryTableMessage::NoFilterResults) => {
            widgets::empty_state(
                ui,
                "No matching archives",
                ZERO_FILTER_RESULTS_MESSAGE,
                None,
            );
        }
        None => {
            let mut clicked = None;
            let mut menu_action = None;
            let mut displayed_column_widths =
                if *library_column_widths == LibraryColumnWidths::default() {
                    responsive_library_column_widths(ui.available_width(), horizontal_spacing)
                } else {
                    *library_column_widths
                };
            let initial_displayed_column_widths = displayed_column_widths;
            let row_menu_context = RowMenuContext {
                records: &data.records,
                cached,
                busy,
                block_reason,
                platform_busy,
                retroarch_profiles,
                library_views_configured,
                library_view_last_plan,
            };
            egui::ScrollArea::horizontal()
                .id_salt("archive_status_horizontal")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let initial_widths = displayed_column_widths.as_array();
                    ui.set_min_width(table_width(horizontal_spacing, &initial_widths));
                    if let Some(clicked_field) = show_header_row(
                        ui,
                        &COLUMN_HEADERS,
                        &COLUMN_SORT_FIELDS,
                        row_height,
                        *sort_field,
                        *sort_ascending,
                        &mut displayed_column_widths,
                    ) {
                        apply_header_click(sort_field, sort_ascending, clicked_field);
                    }
                    ui.separator();
                    // Re-read after `show_header_row`, which may have just
                    // resized a column via its drag handle this very
                    // frame - the rows below must always paint with
                    // *this* frame's widths, never a one-frame-stale copy.
                    let widths = displayed_column_widths.as_array();

                    let body_height = ui.available_height().max(row_height);
                    let mut vertical_scroll_area = egui::ScrollArea::vertical()
                        .id_salt("archive_status_vertical")
                        .max_height(body_height)
                        .auto_shrink([false, false]);
                    if let Some(pos) = requested_scroll_pos {
                        let row_stride = row_height + ui.spacing().item_spacing.y;
                        vertical_scroll_area = vertical_scroll_area.vertical_scroll_offset(
                            compute_scroll_offset_for_focus(
                                pos,
                                row_stride,
                                *library_scroll_offset,
                                body_height,
                            ),
                        );
                    }
                    let scroll_output = vertical_scroll_area.show_rows(
                        ui,
                        row_height,
                        visible_count,
                        |ui, row_range| {
                            let result = show_archive_rows(
                                ui,
                                &merged_rows,
                                Some(&visible_indices),
                                row_range,
                                row_height,
                                selected_index,
                                selected_archives,
                                selected_archive,
                                &widths,
                                &row_menu_context,
                            );
                            clicked = result.clicked;
                            menu_action = result.menu_action;
                        },
                    );
                    *library_scroll_offset = scroll_output.state.offset.y;
                });
            if displayed_column_widths != initial_displayed_column_widths {
                *library_column_widths = displayed_column_widths;
            }
            // Requirement 2: an ordinary click replaces the whole selection
            // with just this row; a Ctrl-click toggles only this row,
            // leaving every other currently-selected row untouched. Either
            // way the details panel's "focused" row (selected_archive)
            // becomes whatever was just clicked, and its platform picker
            // resets - it must never keep showing a choice made for a
            // different, previously-focused archive.
            if let Some((index, ctrl_held)) = clicked {
                let path = merged_rows[index].path.clone();
                apply_row_click(selected_archives, selected_archive, path, ctrl_held);
                *platform_choice = None;
                platform_custom_text.clear();
            }
            // Dispatches every row context-menu action through the exact
            // same `AppOperationRequest`/direct-state paths the existing
            // buttons already use - see `RowContextMenuAction`'s doc
            // comment. `MountSelected`/`UnmountSelected` reuse `MountAll`/
            // `UnmountAll` unchanged (just a selection-scoped item list),
            // so `update()` needs no new match arm at all for those.
            if let Some(action) = menu_action {
                match action {
                    RowContextMenuAction::Operation(request) => {
                        requested_action = Some(AppOperationRequest::Archive(request));
                    }
                    RowContextMenuAction::Inspect(archive_path) => {
                        requested_action = Some(AppOperationRequest::InspectArchive(archive_path));
                    }
                    RowContextMenuAction::Platform(archive_path, platform_action) => {
                        requested_action = Some(AppOperationRequest::PlatformAssignment {
                            archive_path,
                            action: platform_action,
                        });
                    }
                    RowContextMenuAction::MountSelected(paths) => {
                        // Previously dispatched immediately with no
                        // confirmation at all (asymmetric with "Unmount
                        // selected" just below, which already had one) -
                        // now gated the same way every other bulk action is
                        // (decisions 1-3).
                        *confirm_mount_selected = Some(paths);
                        *focus_mount_selected_cancel = true;
                    }
                    RowContextMenuAction::UnmountSelected => {
                        *confirm_unmount_selected = Some(UnmountSelectedConfirmation);
                        *focus_unmount_selected_cancel = true;
                    }
                    RowContextMenuAction::BulkPlatform(archive_paths, kind) => {
                        // Previously dispatched instantly with no
                        // confirmation at all - now gated the same way
                        // every other bulk action is (decisions 1-3).
                        *confirm_bulk_platform_action = Some((archive_paths, kind));
                        *focus_bulk_platform_cancel = true;
                    }
                    RowContextMenuAction::CopyText(text) => {
                        let _ = clipboard.set_text(text);
                    }
                    RowContextMenuAction::ShowOnlySource(source_path) => {
                        *library_source_filter = Some(source_path);
                    }
                    RowContextMenuAction::ShowInLibraryViews(archive_path) => {
                        requested_action =
                            Some(AppOperationRequest::ShowInLibraryViews(archive_path));
                    }
                    RowContextMenuAction::CheatsMods(archive_path) => {
                        requested_action = Some(AppOperationRequest::OpenCheatsMods(archive_path));
                    }
                    RowContextMenuAction::ClearSelection => {
                        selected_archives.clear();
                        *selected_archive = None;
                    }
                }
            }
        }
    }

    requested_action
}

pub(crate) fn recent_scan_contains(recent: &RecentScanAdditions, path: &Path) -> bool {
    recent
        .archives
        .iter()
        .any(|archive| archive.absolute_path == path)
}

pub(crate) fn fixed_row_height(text_height: f32, interact_height: f32) -> f32 {
    text_height.max(interact_height)
}

pub(crate) fn table_width(horizontal_spacing: f32, widths: &[f32; 4]) -> f32 {
    widths.iter().sum::<f32>() + horizontal_spacing * (widths.len().saturating_sub(1) as f32)
}
pub(crate) fn show_column_resize_handle(
    ui: &mut egui::Ui,
    id: egui::Id,
    handle_rect: egui::Rect,
    width: &mut f32,
) {
    let response = ui.interact(handle_rect, id, egui::Sense::drag());
    if response.dragged() {
        *width = (*width + response.drag_delta().x)
            .clamp(MIN_RESIZABLE_COLUMN_WIDTH, MAX_RESIZABLE_COLUMN_WIDTH);
    }
    if response.hovered() || response.dragged() {
        ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::ResizeColumn);
    }
    let stroke_color = if response.dragged() {
        ui.visuals().widgets.active.bg_fill
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    ui.painter().vline(
        handle_rect.center().x,
        handle_rect.y_range(),
        egui::Stroke::new(2.0_f32, stroke_color),
    );
}
pub(crate) fn show_header_row(
    ui: &mut egui::Ui,
    cells: &[&str; 4],
    fields: &[SortField; 4],
    row_height: f32,
    sort_field: Option<SortField>,
    sort_ascending: bool,
    column_widths: &mut LibraryColumnWidths,
) -> Option<SortField> {
    let mut clicked_field = None;
    let spacing = ui.spacing().item_spacing.x;
    let widths = column_widths.as_array();
    let total_width = table_width(spacing, &widths);
    let (_, header_rect) = ui.allocate_space(egui::vec2(total_width, row_height));

    let mut x = header_rect.left();
    for (index, ((text, width), field)) in cells
        .iter()
        .zip(widths)
        .zip(fields.iter().copied())
        .enumerate()
    {
        // Resizable columns reserve their own trailing edge for the drag
        // handle (see `COLUMN_RESIZE_HANDLE_WIDTH`), so the button itself
        // never overlaps the handle's own interact rect.
        let is_resizable = index == 2 || index == 3;
        let button_width = if is_resizable {
            width - COLUMN_RESIZE_HANDLE_WIDTH
        } else {
            width
        };
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(x, header_rect.top()),
            egui::vec2(button_width, row_height),
        );
        let label = if sort_field == Some(field) {
            format!(
                "{text} {}",
                if sort_ascending {
                    "\u{25B2}"
                } else {
                    "\u{25BC}"
                }
            )
        } else {
            (*text).to_string()
        };
        let response = ui
            .put(
                cell_rect,
                egui::Button::new(egui::RichText::new(label).strong()).frame(false),
            )
            .on_hover_text(*text);
        if response.clicked() {
            clicked_field = Some(field);
        }

        if is_resizable {
            let handle_rect = egui::Rect::from_min_size(
                egui::pos2(x + width - COLUMN_RESIZE_HANDLE_WIDTH, header_rect.top()),
                egui::vec2(COLUMN_RESIZE_HANDLE_WIDTH, row_height),
            );
            let handle_id = egui::Id::new("library_column_resize_handle").with(index);
            let target_width = if index == 2 {
                &mut column_widths.archive_path
            } else {
                &mut column_widths.mount_path
            };
            show_column_resize_handle(ui, handle_id, handle_rect, target_width);
        }

        x += width + spacing;
    }
    clicked_field
}

/// Finds which cell rect (laid out left-to-right starting at `row_left`,
/// each `widths[i]` wide with `spacing` between them, matching exactly
/// how `show_data_row`/`show_inspector_row` paint them) contains
/// `pointer_x`, if any. Pure and independent of any live `egui::Context`;
/// see `hovered_cell_full_text`, its only caller, and its own doc
/// comment for why this is kept testable in isolation from real font
/// metrics/hover state. Generic over the number of columns (a slice, not
/// a fixed-size array) so both the four-column Library table and the
/// two-column Archive Inspector row share this one implementation.
pub(crate) fn cell_index_at(
    pointer_x: f32,
    row_left: f32,
    widths: &[f32],
    spacing: f32,
) -> Option<usize> {
    let mut x = row_left;
    for (index, width) in widths.iter().enumerate() {
        if pointer_x >= x && pointer_x < x + width {
            return Some(index);
        }
        x += width + spacing;
    }
    None
}
pub(crate) fn hovered_cell_full_text<'a>(
    pointer_x: Option<f32>,
    row_left: f32,
    cells: &[&'a str],
    widths: &[f32],
    spacing: f32,
    mut measure_width: impl FnMut(&str) -> f32,
) -> Option<&'a str> {
    let index = cell_index_at(pointer_x?, row_left, widths, spacing)?;
    let text = cells[index];
    (measure_width(text) > widths[index]).then_some(text)
}

/// Renders one selectable archive table row as a *single* clickable
/// region (`Sense::click()` on one allocated `Rect`, identified by
/// `id_source` - the archive's exact path, never a lossy display string)
/// with the four cells' text painted passively inside it.
///
/// This replaced an earlier version that rendered each of the four cells
/// as its own separate `egui::Button`, with the row's overall
/// clicked-ness computed by OR-ing all four `Response::clicked()` values
/// together. That meant a row had no single, authoritative `Response` of
/// its own: four independent interactive widgets shared the row's
/// hover/press state, with real gaps between them (the `horizontal`
/// layout's item spacing) that belonged to no widget's sense area at
/// all, and Ctrl-click reliability regressed as a direct result - see the
/// fix for the real-world Nobara bug report this was rewritten for.
///
/// Cell text is painted directly with `Painter::text` rather than as
/// separate child `Label` widgets. This was not just a style choice:
/// registering more than one child widget inside the row's own interact
/// `Rect` (even purely non-interactive `Label`s, `Sense::hover()`-only)
/// was empirically confirmed, while fixing the Ctrl-click bug, to make
/// egui's hit-testing stop recognizing the row's *own* `Response` as
/// hovered/clicked at all in some cases - see the headless
/// `simulate_row_click`-based tests below, which reproduce this exact
/// failure mode against the old approach. Direct painting registers no
/// widgets at all, so there is nothing left inside the row that could
/// ever compete with its own click/hover sensing.
///
/// `widths` (added for resizable columns) pushes this pre-existing,
/// tightly-scoped rendering primitive one parameter past clippy's default
/// threshold. Every parameter here is plain, read-only per-row display
/// data with no natural grouping that would not just be `RowVisualState`-
/// style busywork for its own sake - see `show_archive_rows`'s identical
/// justification, its only caller.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_data_row(
    ui: &mut egui::Ui,
    cells: &[&str; 4],
    row_height: f32,
    id_source: &Path,
    multi_selected: bool,
    focused: bool,
    text_color: Option<egui::Color32>,
    widths: &[f32; 4],
) -> egui::Response {
    let spacing = ui.spacing().item_spacing.x;
    let width = table_width(spacing, widths);
    // Reserve the row's layout space first (advancing the cursor exactly
    // as any other widget would), then sense clicks/hover for that exact
    // `Rect` under a stable `Id` derived from `id_source` - not egui's
    // auto-generated one. `show_rows` virtualizes this list, so the
    // *same* screen position can render a *different* archive across
    // scroll frames; a stable, identity-derived `Id` (rather than one
    // implied only by rendering order/position) means a press-then-scroll
    // gesture can never have its release misattributed to whatever
    // archive now happens to occupy that same position.
    let (_, rect) = ui.allocate_space(egui::vec2(width, row_height));
    let row_id = egui::Id::new("archive_table_row").with(id_source);
    let mut response = ui.interact(rect, row_id, egui::Sense::click());

    // Paint the background *before* the text, so a selected/hovered row
    // gets one clean, contiguous highlight across all four columns
    // (requirement: "a clearly visible selected background", not four
    // separately-tinted buttons with unhighlighted gaps between them).
    // `selection.bg_fill` and `hovered.weak_bg_fill` are egui's own
    // default palette entries - the same colors any ordinary selected or
    // hovered widget would use.
    let visuals = ui.visuals();
    if multi_selected {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
    }
    // The Ctrl+Up/Down "focus doesn't visibly move" fix: a *border*, not
    // another fill, so it stays visible whether or not this row is also
    // `multi_selected` - moving focus with Ctrl held between two rows that
    // are both already multi-selected must still show something change.
    // `warn_fg_color` is deliberately a different hue from
    // `selection.bg_fill`/`.stroke` so the two states never look like the
    // same highlight at a glance.
    if focused {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            0.0,
            egui::Stroke::new(2.0_f32, visuals.warn_fg_color),
            egui::StrokeKind::Inside,
        );
    }

    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let color = text_color.unwrap_or_else(|| ui.visuals().text_color());
    let mut x = rect.left();
    for (text, column_width) in cells.iter().zip(widths.iter().copied()) {
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(x, rect.top()),
            egui::vec2(column_width, row_height),
        );
        ui.painter().with_clip_rect(cell_rect).text(
            egui::pos2(x + 2.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            font_id.clone(),
            color,
        );
        x += column_width + spacing;
    }
    let pointer_x = response.hover_pos().map(|pos| pos.x);
    if let Some(full_text) =
        hovered_cell_full_text(pointer_x, rect.left(), cells, widths, spacing, |text| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_string(), font_id.clone(), color)
                    .size()
                    .x
            })
        })
    {
        response = response.on_hover_text(full_text);
    }

    response
}

/// What a Library row's right-click context menu can request - collected
/// the same way `show_selected_archive` collects its own Mount/Unmount/
/// Inspect/Platform request (a plain returned value; nothing inside the
/// menu closure mutates app state directly), so `show_loaded_data`
/// dispatches every one of these through the exact same
/// `AppOperationRequest`/direct-state paths the existing buttons already
/// use - never a second, independently-drifting execution path.
pub(crate) enum RowContextMenuAction {
    Operation(OperationRequest),
    Inspect(PathBuf),
    Platform(PathBuf, PlatformAction),
    /// Pre-filtered to exactly the selected archives whose `MountState` is
    /// `Pending`/`MountPathExists` (mirrors `pending_mount_items`'s own
    /// filter) - the caller only has to look each path up to build
    /// `MountAllItem`s, never re-derive eligibility.
    MountSelected(Vec<PathBuf>),
    /// A request to *open* the "Unmount selected" confirmation dialog -
    /// carries no item list, unlike `MountSelected`: Mount has no
    /// confirmation step to begin with, but Unmount's eligible set is
    /// deliberately recomputed fresh both while the dialog is open and
    /// again the instant it is confirmed (see `UnmountSelectedConfirmation`'s
    /// doc comment), never captured at the moment this menu item was
    /// clicked.
    UnmountSelected,
    BulkPlatform(Vec<PathBuf>, BulkPlatformActionKind),
    CopyText(String),
    ShowOnlySource(Option<PathBuf>),
    /// "Show in Library View preview" - see
    /// `AppOperationRequest::ShowInLibraryViews`'s doc comment.
    ShowInLibraryViews(PathBuf),
    /// Open the first-class Cheats & Mods workspace for this exact
    /// archive - same gating as the Selected page's entry.
    CheatsMods(PathBuf),
    ClearSelection,
}

/// Read-only state a Library row's context menu needs beyond what
/// `show_archive_rows` already takes - bundled in one struct so adding it
/// doesn't push `show_archive_rows`'s own parameter list further past
/// clippy's threshold (see its existing `#[allow(too_many_arguments)]`).
pub(crate) struct RowMenuContext<'a> {
    pub(crate) records: &'a [ArchiveRecord],
    pub(crate) cached: Option<&'a CachedLibrarySnapshot>,
    pub(crate) busy: bool,
    pub(crate) block_reason: Option<&'static str>,
    pub(crate) platform_busy: bool,
    /// See `LoadedViewState::retroarch_profiles`; profile readiness is
    /// presented on the destination page rather than used to hide it.
    pub(crate) retroarch_profiles: &'a RetroArchProfilesState,
    pub(crate) library_views_configured: bool,
    pub(crate) library_view_last_plan: Option<&'a (LibraryViewConfig, LibraryViewPlan)>,
}
pub(crate) fn apply_row_right_click(
    selected_archives: &mut HashSet<PathBuf>,
    selected_archive: &mut Option<PathBuf>,
    path: PathBuf,
) {
    if !selected_archives.contains(&path) {
        selected_archives.clear();
        selected_archives.insert(path.clone());
        *selected_archive = Some(path);
    }
}

pub(crate) fn cheats_mods_row_action(path: &Path) -> RowContextMenuAction {
    RowContextMenuAction::CheatsMods(path.to_path_buf())
}
pub(crate) fn show_row_context_menu(
    ui: &mut egui::Ui,
    row: &ArchiveRow,
    selected_archives: &HashSet<PathBuf>,
    ctx: &RowMenuContext<'_>,
) -> Option<RowContextMenuAction> {
    if selected_archives.len() > 1 {
        show_bulk_row_context_menu(ui, selected_archives, ctx)
    } else {
        show_single_row_context_menu(ui, row, ctx)
    }
}
pub(crate) fn library_view_planned_entry_for<'a>(
    last_plan: Option<&'a (LibraryViewConfig, LibraryViewPlan)>,
    archive_path: &Path,
) -> Option<&'a LibraryViewPlanEntry> {
    let (_, plan) = last_plan?;
    plan.entries
        .iter()
        .find(|entry| entry.archive_path.as_deref() == Some(archive_path))
}

/// The narrower "does this archive have a real planned destination"
/// question `library_view_planned_entry_for` also answers - `None` for a
/// `Skip*` entry (no destination was ever computed) as well as for "no
/// entry at all", so a caller offering "copy this path" never offers to
/// copy nothing.
pub(crate) fn library_view_planned_destination_for<'a>(
    last_plan: Option<&'a (LibraryViewConfig, LibraryViewPlan)>,
    archive_path: &Path,
) -> Option<&'a Path> {
    library_view_planned_entry_for(last_plan, archive_path)?
        .destination_path
        .as_deref()
}
pub(crate) fn show_single_row_context_menu(
    ui: &mut egui::Ui,
    row: &ArchiveRow,
    ctx: &RowMenuContext<'_>,
) -> Option<RowContextMenuAction> {
    let mut action = None;
    let record = selected_record(ctx.records, Some(&row.path));
    let persisted = selected_persisted_archive(ctx.cached, Some(&row.path));

    if let Some(record) = record.filter(|record| record.is_mount_input()) {
        let archive_action = available_action(record.mount_state);
        let label = match archive_action {
            ArchiveAction::Mount => "Mount",
            ArchiveAction::Unmount => "Unmount",
            ArchiveAction::LazyUnmount | ArchiveAction::Remount => {
                unreachable!("available_action only ever returns Mount or Unmount")
            }
        };
        let enabled = individual_actions_available(ctx.busy);
        let mut button = ui.add_enabled(enabled, egui::Button::new(label));
        if !enabled && let Some(reason) = ctx.block_reason {
            button = button.on_disabled_hover_text(reason);
        }
        if button.clicked() {
            action = Some(RowContextMenuAction::Operation(OperationRequest {
                action: archive_action,
                archive_path: row.path.clone(),
                cleanup_after_unmount: false,
            }));
            ui.close();
        }
        // Same source of truth as the main panel's inline reason label
        // (`show_selected_archive`) - both read the identical `block_reason`
        // `archive_action_block_reason` produced, never a second guess.
        if !enabled && let Some(reason) = ctx.block_reason {
            ui.label(reason);
        }
    } else if record.is_some() {
        ui.add_enabled(false, egui::Button::new("No mount required"))
            .on_disabled_hover_text("Loose ROM · no EmuWiz mount required");
        ui.label("Loose ROM · no EmuWiz mount required");
    } else {
        ui.add_enabled(false, egui::Button::new("Mount"))
            .on_disabled_hover_text("Archive is catalogue-only.");
        ui.label("Archive is catalogue-only.");
    }

    let inspectable = is_inspectable(&row.path);
    let inspect_button = ui.add_enabled(inspectable, egui::Button::new("Inspect contents"));
    let inspect_button = if inspectable {
        inspect_button.on_hover_text(
            "Read-only: view this archive's internal entries without extracting them.",
        )
    } else {
        inspect_button.on_disabled_hover_text("Unsupported archive format.")
    };
    if inspect_button.clicked() {
        action = Some(RowContextMenuAction::Inspect(row.path.clone()));
        ui.close();
    }

    // Same gating as the Selected page's entry button - this menu only
    // renders for a single-row selection, so the count is truthfully 1.
    let cheat_blocker = cheat_entry_blocker(
        Some(&row.path),
        1,
        Some(ctx.records),
        ctx.retroarch_profiles,
    );
    let cheat_button = ui.add_enabled(cheat_blocker.is_none(), egui::Button::new("Cheats & Mods"));
    let cheat_button = if let Some(reason) = cheat_blocker {
        cheat_button.on_disabled_hover_text(reason)
    } else {
        cheat_button
    };
    if cheat_button.clicked() {
        action = Some(cheats_mods_row_action(&row.path));
        ui.close();
    }

    ui.separator();
    if ui.button("Copy archive path").clicked() {
        action = Some(RowContextMenuAction::CopyText(row.archive_path.clone()));
        ui.close();
    }
    if ui.button("Copy mount path").clicked() {
        action = Some(RowContextMenuAction::CopyText(row.mount_path.clone()));
        ui.close();
    }
    if ui.button("Copy source path").clicked() {
        let source_text = row
            .source_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Unassigned / Legacy".to_string());
        action = Some(RowContextMenuAction::CopyText(source_text));
        ui.close();
    }

    ui.separator();
    if let Some(persisted) = persisted {
        ui.menu_button("Set platform", |ui| {
            if let Some(name) = widgets::platform_picker(
                ui,
                ("row_context_menu_set_platform", &row.path),
                &canonical_platform_names(),
                persisted.platform.as_deref(),
                true,
            ) {
                action = Some(RowContextMenuAction::Platform(
                    row.path.clone(),
                    PlatformAction::Set(name.to_string()),
                ));
                ui.close();
            }
        });
        let manual_set = persisted.platform_source.as_deref() == Some(MANUAL_PLATFORM_SOURCE);
        if ui
            .add_enabled(manual_set, egui::Button::new("Clear manual platform"))
            .clicked()
        {
            action = Some(RowContextMenuAction::Platform(
                row.path.clone(),
                PlatformAction::Clear,
            ));
            ui.close();
        }
    } else {
        ui.add_enabled(false, egui::Button::new("Set platform"))
            .on_disabled_hover_text("Not yet in the library database.");
    }

    ui.separator();
    if ui.button("Show only this source").clicked() {
        action = Some(RowContextMenuAction::ShowOnlySource(
            row.source_path.clone(),
        ));
        ui.close();
    }
    if ui.button("Clear selection").clicked() {
        action = Some(RowContextMenuAction::ClearSelection);
        ui.close();
    }

    ui.separator();
    let show_button = ui.add_enabled(
        ctx.library_views_configured,
        egui::Button::new("Show in Library View preview"),
    );
    let show_button = if ctx.library_views_configured {
        show_button
    } else {
        show_button.on_disabled_hover_text("No Library View is configured yet.")
    };
    if show_button.clicked() {
        action = Some(RowContextMenuAction::ShowInLibraryViews(row.path.clone()));
        ui.close();
    }
    let planned_path = library_view_planned_destination_for(ctx.library_view_last_plan, &row.path);
    let copy_planned_button = ui.add_enabled(
        planned_path.is_some(),
        egui::Button::new("Copy planned view path"),
    );
    let copy_planned_button = if planned_path.is_some() {
        copy_planned_button
    } else {
        copy_planned_button
            .on_disabled_hover_text("Preview a Library View to see this archive's planned path.")
    };
    if let Some(path) = planned_path
        && copy_planned_button.clicked()
    {
        action = Some(RowContextMenuAction::CopyText(path.display().to_string()));
        ui.close();
    }

    action
}

/// The multi-selection row context menu. "Mount selected"/"Unmount
/// selected" reuse `MountState` filtering identical to
/// `pending_mount_items`/`pending_unmount_items` (only genuinely eligible
/// archives are ever included), and the caller executes them through
/// `start_mount_all`/`start_unmount_all` - the exact same batch engine
/// `Mount All`/`Unmount All` already use, never a second implementation.
pub(crate) fn show_bulk_row_context_menu(
    ui: &mut egui::Ui,
    selected_archives: &HashSet<PathBuf>,
    ctx: &RowMenuContext<'_>,
) -> Option<RowContextMenuAction> {
    let mut action = None;
    let selected_count = selected_archives.len();

    let mountable: Vec<PathBuf> = ctx
        .records
        .iter()
        .filter(|record| {
            selected_archives.contains(&record.mount_plan.archive.path)
                && record.is_mount_input()
                && matches!(
                    record.mount_state,
                    MountState::Pending | MountState::MountPathExists
                )
        })
        .map(|record| record.mount_plan.archive.path.clone())
        .collect();
    let unmountable: Vec<PathBuf> = ctx
        .records
        .iter()
        .filter(|record| {
            selected_archives.contains(&record.mount_plan.archive.path)
                && record.mount_state == MountState::Mounted
        })
        .map(|record| record.mount_plan.archive.path.clone())
        .collect();

    let mount_enabled = individual_actions_available(ctx.busy) && !mountable.is_empty();
    let mut mount_button = ui.add_enabled(
        mount_enabled,
        egui::Button::new(format!("Mount selected ({})", mountable.len())),
    );
    if !mount_enabled {
        mount_button = mount_button.on_disabled_hover_text(
            ctx.block_reason
                .unwrap_or("No selected archive is ready to mount."),
        );
    }
    if mount_button.clicked() {
        action = Some(RowContextMenuAction::MountSelected(mountable));
        ui.close();
    }

    let unmount_enabled = individual_actions_available(ctx.busy) && !unmountable.is_empty();
    let mut unmount_button = ui.add_enabled(
        unmount_enabled,
        egui::Button::new(format!("Unmount selected ({})", unmountable.len())),
    );
    if !unmount_enabled {
        unmount_button = unmount_button.on_disabled_hover_text(
            ctx.block_reason
                .unwrap_or("No selected archive is currently mounted."),
        );
    }
    if unmount_button.clicked() {
        action = Some(RowContextMenuAction::UnmountSelected);
        ui.close();
    }

    ui.separator();
    ui.add_enabled_ui(!ctx.platform_busy, |ui| {
        ui.menu_button("Set platform for selected", |ui| {
            if let Some(name) = widgets::platform_picker(
                ui,
                "row_context_menu_bulk_set_platform",
                &canonical_platform_names(),
                None,
                true,
            ) {
                action = Some(RowContextMenuAction::BulkPlatform(
                    selected_archives.iter().cloned().collect(),
                    BulkPlatformActionKind::Set(name.to_string()),
                ));
                ui.close();
            }
        });
    });

    ui.separator();
    if ui
        .button(format!("Copy selected archive paths ({selected_count})"))
        .clicked()
    {
        let mut paths: Vec<&PathBuf> = selected_archives.iter().collect();
        paths.sort();
        let text = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        action = Some(RowContextMenuAction::CopyText(text));
        ui.close();
    }
    if ui.button("Clear selection").clicked() {
        action = Some(RowContextMenuAction::ClearSelection);
        ui.close();
    }

    action
}

/// One frame's outcome from `show_archive_rows`: the row clicked (existing
/// left-click/Ctrl-click handling, unchanged), plus at most one context-
/// menu action requested this frame (at most one menu can be open at a
/// time, so these can never conflict).
pub(crate) struct ArchiveRowsFrameResult {
    clicked: Option<(usize, bool)>,
    menu_action: Option<RowContextMenuAction>,
}

/// Renders one page of table rows. A row can be `multi_selected` (a member
/// of the exact `ArchiveRow::path` identity set), the single "focused" row
/// (`selected_index`), both, or neither - these are rendered as visually
/// distinct states (see `show_data_row`), not collapsed into one "is
/// selected" flag, so that Ctrl+Up/Down moving focus among an existing
/// multi-selection is still visible. Returns `Some((row_index, ctrl_held))`
/// for the row clicked this frame, if any - `ctrl_held` is read once, from
/// the same frame's input state every row in this call shares, so the
/// caller can distinguish an ordinary click (replace the selection) from a
/// Ctrl-click (toggle just this row) without this function needing to know
/// anything about selection semantics itself.
///
/// `widths` (added for resizable columns) pushes this one parameter past
/// clippy's default threshold - see `show_data_row`'s identical
/// justification.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_archive_rows(
    ui: &mut egui::Ui,
    rows: &[ArchiveRow],
    filtered_rows: Option<&[usize]>,
    row_range: Range<usize>,
    row_height: f32,
    selected_index: Option<usize>,
    selected_archives: &mut HashSet<PathBuf>,
    selected_archive: &mut Option<PathBuf>,
    widths: &[f32; 4],
    menu_context: &RowMenuContext<'_>,
) -> ArchiveRowsFrameResult {
    let mut clicked = None;
    let mut menu_action = None;
    let visuals = ui.visuals().clone();
    let ctrl_held = ui.input(|input| input.modifiers.ctrl);
    for visible_index in row_range {
        let row_index = filtered_rows
            .map(|indices| indices[visible_index])
            .unwrap_or(visible_index);
        let row = &rows[row_index];
        let cells = [
            row.platform.as_str(),
            row.state.as_str(),
            row.archive_path.as_str(),
            row.mount_path.as_str(),
        ];
        let is_multi_selected = selected_archives.contains(&row.path);
        let is_focused = selected_index == Some(row_index);
        let response = show_data_row(
            ui,
            &cells,
            row_height,
            &row.path,
            is_multi_selected,
            is_focused,
            row.row_text_color(&visuals),
            widths,
        );
        if response.clicked() {
            clicked = Some((row_index, ctrl_held));
        }
        if response.secondary_clicked() {
            apply_row_right_click(selected_archives, selected_archive, row.path.clone());
        }
        let selected_ref: &HashSet<PathBuf> = selected_archives;
        response.context_menu(|ui| {
            if let Some(requested) = show_row_context_menu(ui, row, selected_ref, menu_context) {
                menu_action = Some(requested);
            }
        });
    }
    ArchiveRowsFrameResult {
        clicked,
        menu_action,
    }
}

pub(crate) fn selected_record<'a>(
    records: &'a [ArchiveRecord],
    selected_archive: Option<&Path>,
) -> Option<&'a ArchiveRecord> {
    selected_record_index(records, selected_archive).map(|index| &records[index])
}

pub(crate) fn selected_record_index(
    records: &[ArchiveRecord],
    selected_archive: Option<&Path>,
) -> Option<usize> {
    let selected_archive = selected_archive?;
    records
        .iter()
        .position(|record| record.mount_plan.archive.path == selected_archive)
}

/// Like `selected_record_index`, but over the merged live+cache row list
/// via each row's exact-byte `path` identity - never a lossy display
/// string (requirement 5). Used to drive table-row highlighting for both
/// live and cache-only rows; selecting a cache-only row still leaves
/// `selected_record` (which only searches live records) returning `None`,
/// so no action button is ever offered for it.
pub(crate) fn selected_row_index(
    rows: &[ArchiveRow],
    selected_archive: Option<&Path>,
) -> Option<usize> {
    let selected_archive = selected_archive?;
    rows.iter().position(|row| row.path == selected_archive)
}

/// Applies one row click to the selection state - requirement 2's exact
/// semantics. An ordinary click (`ctrl_held = false`) replaces the whole
/// multi-selection with just `path`; a Ctrl-click toggles only `path`,
/// leaving every other currently-selected row untouched. Either way,
/// `selected_archive` (the details panel's "focused" row) becomes
/// `path`. Factored out from `show_loaded_data`'s row-click handling so
/// it is directly testable without an `egui::Ui`.
pub(crate) fn apply_row_click(
    selected_archives: &mut HashSet<PathBuf>,
    selected_archive: &mut Option<PathBuf>,
    path: PathBuf,
    ctrl_held: bool,
) {
    if ctrl_held {
        if !selected_archives.remove(&path) {
            selected_archives.insert(path.clone());
        }
    } else {
        selected_archives.clear();
        selected_archives.insert(path.clone());
    }
    *selected_archive = Some(path);
}

/// Whether the compact bulk platform action bar should be shown -
/// requirement 3: only when more than one row is selected. Factored out
/// as its own pure predicate (mirroring `mount_all_available`) so the
/// condition is directly testable without an `egui::Ui`.
pub(crate) fn bulk_action_bar_visible(selected_archives: &HashSet<PathBuf>) -> bool {
    selected_archives.len() > 1
}

/// Whether "Select all visible" should be enabled - requirement 6:
/// disabled whenever the current search/filters leave zero library rows
/// visible. Factored out as its own pure predicate (mirroring
/// `mount_all_available`/`bulk_action_bar_visible`) so it is directly
/// testable without an `egui::Ui`.
pub(crate) fn select_all_visible_button_enabled(visible_count: usize) -> bool {
    visible_count > 0
}
pub(crate) fn selection_status_text(selected_count: usize) -> String {
    match selected_count {
        0 => "No archives selected".to_string(),
        1 => "1 archive selected".to_string(),
        n => format!("{n} archives selected"),
    }
}

/// Renders the "Showing X of Y archives" / selection-status / "Select all
/// visible" row - the ordinary-library selection controls that sit above
/// the table, next to the always-visible `selection_status_text` label
/// (see that function's doc comment) and near the bulk action bar's
/// "Clear selected"/"Clear selection" (`show_bulk_platform_action_bar`).
/// Factored out from `show_loaded_data` (mirroring
/// `show_bulk_platform_action_bar`) so it can be rendered and click-tested
/// standalone.
///
/// v0.4.2-alpha follow-up requirement: "Select all visible" is a
/// mouse-only equivalent of Ctrl+A. It calls `select_all_visible` with
/// this same frame's own `merged_rows`/`visible_indices` - the exact same
/// helper and inputs the Ctrl+A handler in `show_loaded_data` dispatches
/// to - so there is no second selection implementation to drift out of
/// sync with search/filters/sort. Disabled whenever zero rows are
/// currently visible; clicking it while every visible row is already
/// selected is a no-op rebuild of the identical `HashSet`.
pub(crate) fn show_selection_controls_row(
    ui: &mut egui::Ui,
    merged_rows: &[ArchiveRow],
    visible_indices: &[usize],
    selected_archives: &mut HashSet<PathBuf>,
) {
    let visible_count = visible_indices.len();
    ui.horizontal(|ui| {
        ui.label(format!(
            "Showing {} of {} archives",
            visible_count,
            merged_rows.len()
        ));
        ui.separator();
        ui.label(selection_status_text(selected_archives.len()));
        ui.separator();
        if ui
            .add_enabled(
                select_all_visible_button_enabled(visible_count),
                egui::Button::new("Select all visible"),
            )
            .clicked()
        {
            *selected_archives = select_all_visible(merged_rows, visible_indices);
        }
    });
}

pub(crate) const EMPTY_LIBRARY_MESSAGE: &str = "Add a source or scan your library to find games.";
pub(crate) const ZERO_FILTER_RESULTS_MESSAGE: &str =
    "No archives match the current search and filters.";

/// Requirement 4: distinguishes "the library itself has no archives at
/// all" from "the library has archives, but the current search/filters
/// hide every one of them" - factored out as its own pure predicate
/// (mirroring `bulk_action_bar_visible`) so the choice of message is
/// directly testable without an `egui::Ui`, and so the table is never
/// left rendering as an unexplained blank area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LibraryTableMessage {
    EmptyLibrary,
    NoFilterResults,
    NoCompletedScan,
    NoRecentAdditions,
}

pub(crate) fn library_table_message(
    merged_rows_is_empty: bool,
    visible_count: usize,
) -> Option<LibraryTableMessage> {
    if merged_rows_is_empty {
        Some(LibraryTableMessage::EmptyLibrary)
    } else if visible_count == 0 {
        Some(LibraryTableMessage::NoFilterResults)
    } else {
        None
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SortField {
    Platform,
    State,
    ArchivePath,
    MountPath,
}

pub(crate) const COLUMN_SORT_FIELDS: [SortField; 4] = [
    SortField::Platform,
    SortField::State,
    SortField::ArchivePath,
    SortField::MountPath,
];

pub(crate) fn sort_field_key(row: &ArchiveRow, field: SortField) -> &str {
    match field {
        SortField::Platform => &row.platform,
        SortField::State => &row.state,
        SortField::ArchivePath => &row.archive_path,
        SortField::MountPath => &row.mount_path,
    }
}

/// Sorts `indices` (a filtered view into `merged_rows`, never the rows
/// themselves - requirement 2: "must not mutate database order or
/// archive identity") by the chosen column. `Vec::sort_by` is a stable
/// sort, and the exact `ArchiveRow::path` is always the final
/// tie-breaker (in fixed ascending order, independent of `ascending`, so
/// ties resolve identically either direction) - together these make the
/// result fully deterministic regardless of `merged_rows`'s incoming
/// order.
pub(crate) fn sort_visible_indices(
    merged_rows: &[ArchiveRow],
    indices: &mut [usize],
    field: SortField,
    ascending: bool,
) {
    indices.sort_by(|&left, &right| {
        let left_row = &merged_rows[left];
        let right_row = &merged_rows[right];
        let primary = sort_field_key(left_row, field).cmp(sort_field_key(right_row, field));
        let primary = if ascending {
            primary
        } else {
            primary.reverse()
        };
        primary.then_with(|| left_row.path.cmp(&right_row.path))
    });
}

/// Applies one header click to the current sort state - requirement 2:
/// clicking a new column selects it (starting ascending); clicking the
/// already-active column toggles its direction. Factored out from
/// `show_loaded_data` so this decision is directly testable without an
/// `egui::Ui`.
pub(crate) fn apply_header_click(
    sort_field: &mut Option<SortField>,
    sort_ascending: &mut bool,
    clicked_field: SortField,
) {
    if *sort_field == Some(clicked_field) {
        *sort_ascending = !*sort_ascending;
    } else {
        *sort_field = Some(clicked_field);
        *sort_ascending = true;
    }
}

/// Requirement 1: Ctrl+A must select exactly the archives currently
/// visible after filters are applied - never a hidden/filtered-out row.
/// Paths are cloned directly out of `merged_rows` at the positions
/// `visible_indices` names, computed fresh this same frame, so this can
/// never select against a stale filter/sort state from an earlier frame.
pub(crate) fn select_all_visible(
    merged_rows: &[ArchiveRow],
    visible_indices: &[usize],
) -> HashSet<PathBuf> {
    visible_indices
        .iter()
        .map(|&index| merged_rows[index].path.clone())
        .collect()
}
/// Requirement 1: computes the next focused archive for Up/Down,
/// stepping strictly through `visible_indices` - the exact filtered *and
/// sorted* order currently on screen - by searching for the current
/// focus's exact path rather than trusting any previously-computed row
/// index. A raw index saved from an earlier frame can be invalidated by
/// a filter or sort change between frames; re-deriving the position from
/// `current_focus`'s `PathBuf` identity every call means there is no
/// stale index to go stale. Clamps at either end rather than wrapping.
pub(crate) fn next_focus_in_visible_order(
    merged_rows: &[ArchiveRow],
    visible_indices: &[usize],
    current_focus: Option<&Path>,
    direction: ArrowDirection,
) -> Option<PathBuf> {
    if visible_indices.is_empty() {
        return None;
    }
    let current_pos = current_focus.and_then(|path| {
        visible_indices
            .iter()
            .position(|&index| merged_rows[index].path == path)
    });
    let next_pos = match (current_pos, direction) {
        (Some(pos), ArrowDirection::Down) => (pos + 1).min(visible_indices.len() - 1),
        (Some(pos), ArrowDirection::Up) => pos.saturating_sub(1),
        (None, _) => 0,
    };
    Some(merged_rows[visible_indices[next_pos]].path.clone())
}
pub(crate) fn apply_arrow_focus_change(
    selected_archives: &mut HashSet<PathBuf>,
    selected_archive: &mut Option<PathBuf>,
    new_focus: PathBuf,
    ctrl_held: bool,
) {
    if !ctrl_held {
        selected_archives.clear();
        selected_archives.insert(new_focus.clone());
    }
    *selected_archive = Some(new_focus);
}

/// Auto-scroll fix for Ctrl+Up/Down: computes the vertical `ScrollArea`
/// offset needed to bring the row at visible position `focus_pos` (rows
/// `row_stride` pixels apart) into view, given the scroll area's
/// `current_offset` (its own offset as of the end of the previous frame)
/// and `viewport_height`. Performs the smallest scroll that satisfies
/// this: if the row is already fully within
/// `current_offset..current_offset + viewport_height`, `current_offset` is
/// returned unchanged (no jump on every keypress); otherwise the offset is
/// clamped to align the row to whichever edge it just crossed.
pub(crate) fn compute_scroll_offset_for_focus(
    focus_pos: usize,
    row_stride: f32,
    current_offset: f32,
    viewport_height: f32,
) -> f32 {
    let row_top = focus_pos as f32 * row_stride;
    let row_bottom = row_top + row_stride;
    if row_top < current_offset {
        row_top
    } else if row_bottom > current_offset + viewport_height {
        (row_bottom - viewport_height).max(0.0)
    } else {
        current_offset
    }
}

/// Requirement 1's last bullet: keyboard shortcuts (Escape, Ctrl+A,
/// arrow navigation) must not fire while a text field or `ComboBox` is
/// actively receiving keyboard input. `mem.focused()` is `Some` exactly
/// when a widget (a `TextEdit`'s cursor, for example) currently holds
/// keyboard focus; `Popup::is_any_open` additionally covers an open
/// `ComboBox` dropdown, which does not itself hold "focus" in that sense
/// but should equally suppress these shortcuts while its own keyboard
/// navigation is active.
pub(crate) fn keyboard_shortcuts_blocked_by_focus(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| memory.focused().is_some()) || egui::Popup::is_any_open(ctx)
}
