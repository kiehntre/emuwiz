//! The Selected archive/game detail panel: Advanced View's read-only
//! identity/metadata grid plus its mount/unmount/platform-assignment
//! action buttons. Extracted verbatim from `main.rs` (2026-08-22, GUI
//! extraction pass 3).
//!
//! Deliberately excludes the mount-decision functions this panel calls
//! that are *also* called from Library's row list (`available_action`,
//! `individual_actions_available`) and the ones used only by Library
//! (`confirmation_actions_available`, `record_recovery_activity`,
//! `advance_to_final_lazy_confirmation`, `lazy_confirmation_available`) -
//! all of those decide mount/unmount semantics or record activity, not
//! how the panel renders, and stayed in `main.rs` per the same reasoning
//! applied throughout this extraction. `lazy_unmount_available`,
//! `remount_available`, `remount_is_offered`, and
//! `resolved_platform_choice` moved here because they are called only
//! from this panel.

use super::*;

/// Resolves the platform text a "Set Platform" click should apply:
/// `platform_custom_text` (trimmed, rejecting empty) when
/// `CUSTOM_PLATFORM_CHOICE` is selected, otherwise the selected
/// canonical name directly. `None` means nothing valid to apply yet
/// (no selection, or an empty custom field) - the caller uses this to
/// keep "Set Platform" disabled.
pub(crate) fn resolved_platform_choice<'a>(
    choice: Option<&'a str>,
    custom_text: &'a str,
) -> Option<&'a str> {
    match choice {
        Some(CUSTOM_PLATFORM_CHOICE) => {
            let trimmed = custom_text.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        }
        Some(name) => Some(name),
        None => None,
    }
}
pub(crate) fn lazy_unmount_available(
    record: &ArchiveRecord,
    offered_archives: &HashSet<PathBuf>,
    busy: bool,
) -> bool {
    !busy
        && record.mount_state == MountState::Mounted
        && offered_archives.contains(&record.mount_plan.archive.path)
}

pub(crate) fn remount_available(
    record: &ArchiveRecord,
    offered_archives: &HashSet<PathBuf>,
    busy: bool,
) -> bool {
    !busy
        && record.mount_state != MountState::Mounted
        && offered_archives.contains(&record.mount_plan.archive.path)
}

pub(crate) fn remount_is_offered(
    record: &ArchiveRecord,
    offered_archives: &HashSet<PathBuf>,
) -> bool {
    record.mount_state != MountState::Mounted
        && offered_archives.contains(&record.mount_plan.archive.path)
}

pub(crate) struct SelectedArchiveViewState<'a> {
    pub(crate) operation: Option<&'a RunningOperation>,
    pub(crate) busy: bool,
    pub(crate) block_reason: Option<&'static str>,
    pub(crate) action_readiness_debug_lines: &'a [String],
    pub(crate) confirm_unmount: &'a mut Option<PathBuf>,
    pub(crate) confirm_lazy_unmount: &'a mut Option<PathBuf>,
    pub(crate) focus_lazy_cancel: &'a mut bool,
    pub(crate) lazy_unmount_offers: &'a HashSet<PathBuf>,
    pub(crate) remount_offers: &'a HashSet<PathBuf>,
    pub(crate) cleanup_after_unmount: &'a mut bool,
    pub(crate) platform_choice: &'a mut Option<String>,
    pub(crate) platform_custom_text: &'a mut String,
    pub(crate) platform_busy: bool,
    pub(crate) clipboard: &'a mut dyn ClipboardBackend,
}

#[derive(Default)]
pub(crate) struct SelectedArchiveActions {
    pub(crate) operation: Option<OperationRequest>,
    pub(crate) platform: Option<(PathBuf, PlatformAction)>,
    pub(crate) inspect: Option<PathBuf>,
    pub(crate) cheats_mods: Option<PathBuf>,
}

pub(crate) fn show_selected_archive(
    ui: &mut egui::Ui,
    record: Option<&ArchiveRecord>,
    persisted: Option<&PersistedArchive>,
    platform_details: Option<&PlatformProvenanceDetails>,
    source_path: Option<&Path>,
    view_state: SelectedArchiveViewState<'_>,
) -> SelectedArchiveActions {
    let SelectedArchiveViewState {
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
    } = view_state;
    let mut request = None;
    let mut platform_request = None;
    let mut inspect_request = None;
    let mut cheats_mods_request = None;
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Selected archive",
            Some("Inspect identity, mount state, health, and metadata for the focused row."),
        );
        if record.is_none() && persisted.is_none() {
            ui.label("Select an archive row to view details.");
            return;
        }

        let Some(record) = record else {
            if let Some(persisted) = persisted {
                let archive_path_text = persisted.absolute_path.display().to_string();
                if widgets::copyable_value(ui, "Archive path", &archive_path_text) {
                    let _ = clipboard.set_text(archive_path_text.clone());
                }
                let source_text = source_path
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Unassigned / Legacy".to_string());
                ui.label(format!("Source: {source_text}"));
                if persisted.last_verified_missing_at.is_some() {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        "Status: Missing from the latest successful source-folder scan",
                    );
                    ui.label(format!("Last seen: {}", persisted.last_seen_at));
                }
                ui.label(
                    "Known to the library database, not confirmed by the latest live snapshot. \
                     Mount/unmount actions are unavailable until it is - platform assignment \
                     below is metadata only and unaffected.",
                );
                if ui
                    .add_enabled(
                        is_inspectable(&persisted.absolute_path),
                        egui::Button::new("Inspect contents"),
                    )
                    .on_hover_text(
                        "Read-only: view this archive's internal entries without extracting them.",
                    )
                    .clicked()
                {
                    inspect_request = Some(persisted.absolute_path.clone());
                }
            }
            let action = show_platform_section(
                ui,
                persisted,
                platform_details,
                platform_choice,
                platform_custom_text,
                platform_busy,
                clipboard,
            );
            if let (Some(persisted), Some(action)) = (persisted, action) {
                platform_request = Some((persisted.absolute_path.clone(), action));
            }
            return;
        };

        egui::Grid::new("selected_archive_details")
            .num_columns(2)
            .striped(true)
            // Bounded (2026-08-22, live-QA Phase 8): an `egui::Grid` sizes
            // each column to its widest cell's natural content width unless
            // capped, so a genuinely long absolute path - most often the
            // Mount path, since a mount root plus platform plus a long game
            // name adds up fast - could push this whole card, and the page
            // around it, wider than the viewport with no way to reach the
            // clipped content. `detail_row_with_copy`'s `.truncate()` only
            // has anything to truncate against once the cell it is drawn
            // into actually has a bounded width to truncate to.
            .max_col_width(theme::CONTENT_MAX_WIDTH * 0.6)
            .show(ui, |ui| {
                detail_row_with_copy(
                    ui,
                    "Archive path",
                    &record.mount_plan.archive.path.display().to_string(),
                    clipboard,
                );
                detail_row_with_copy(
                    ui,
                    "Mount path",
                    &record.mount_plan.mount_path.display().to_string(),
                    clipboard,
                );
                detail_row_with_copy(
                    ui,
                    "Source",
                    &source_path
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "Unassigned / Legacy".to_string()),
                    clipboard,
                );
                detail_row(
                    ui,
                    "Platform",
                    record
                        .metadata
                        .platform
                        .as_deref()
                        .or(record.identity.platform.as_deref())
                        .unwrap_or("Unknown"),
                );
                detail_row(
                    ui,
                    "Archive format",
                    archive_kind_name(record.mount_plan.archive.kind),
                );
                detail_row(ui, "Size", &format_size(record.identity.size_bytes));
                detail_row(ui, "Mount state", &record.mount_state.to_string());
                detail_row(ui, "Health", &record.health.to_string());
                optional_detail_row(ui, "Title", record.metadata.title.as_deref());
                optional_detail_row(ui, "Region", record.metadata.region.as_deref());
                optional_detail_row(ui, "Version", record.metadata.version.as_deref());
                optional_detail_row(ui, "Disc", record.metadata.disc.as_deref());
                optional_detail_row(ui, "Publisher", record.metadata.publisher.as_deref());
                optional_detail_row(ui, "Developer", record.metadata.developer.as_deref());
                optional_detail_row(ui, "Genre", record.metadata.genre.as_deref());
                optional_detail_row(ui, "Notes", record.metadata.notes.as_deref());
                optional_detail_row(ui, "Metadata source", record.metadata.source.as_deref());
                if let Some(year) = record.metadata.release_year {
                    detail_row(ui, "Release year", &year.to_string());
                }
                if let Some(languages) = &record.metadata.languages {
                    detail_row(ui, "Languages", &languages.join(", "));
                }
            });

        ui.add_space(6.0);
        let can_lazy_unmount = lazy_unmount_available(record, lazy_unmount_offers, busy);
        let remount_offered = remount_is_offered(record, remount_offers);
        let action = if remount_offered {
            ArchiveAction::Remount
        } else {
            available_action(record.mount_state)
        };
        ui.strong("Options");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    is_inspectable(&record.mount_plan.archive.path),
                    egui::Button::new("Inspect contents"),
                )
                .on_hover_text(
                    "Read-only: view this archive's internal entries without extracting them.",
                )
                .clicked()
            {
                inspect_request = Some(record.mount_plan.archive.path.clone());
            }
            if widgets::action_button(ui, "Cheats & Mods", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                cheats_mods_request = Some(record.mount_plan.archive.path.clone());
            }
        });
        if !record.is_mount_input() {
            widgets::banner(
                ui,
                "No mount required",
                "Loose ROM · no EmuWiz mount required. Inspect, Cheats & Mods, copy-path, and library metadata actions remain available.",
                widgets::StatusTone::Info,
            );
            return;
        }
        ui.add_enabled_ui(!busy, |ui| {
            ui.checkbox(
                cleanup_after_unmount,
                "Clean empty mount directories after unmount",
            );
        });
        ui.add_space(4.0);
        if remount_offered {
            ui.colored_label(egui::Color32::from_rgb(210, 140, 40), REMOUNT_GUIDANCE);
        }
        let label = match action {
            ArchiveAction::Mount => "Mount",
            ArchiveAction::Unmount => "Unmount",
            ArchiveAction::LazyUnmount => "Lazy Unmount",
            ArchiveAction::Remount => "Remount",
        };
        let primary_enabled = match action {
            ArchiveAction::Remount => remount_available(record, remount_offers, busy),
            ArchiveAction::Mount | ArchiveAction::Unmount | ArchiveAction::LazyUnmount => {
                individual_actions_available(busy)
            }
        };
        ui.horizontal(|ui| {
            let mut button = ui.add_enabled(primary_enabled, egui::Button::new(label));
            if !primary_enabled && let Some(reason) = block_reason {
                button = button.on_disabled_hover_text(reason);
            }
            if button.clicked() {
                let archive_path = record.mount_plan.archive.path.clone();
                match action {
                    ArchiveAction::Mount => {
                        request = Some(OperationRequest {
                            action,
                            archive_path,
                            cleanup_after_unmount: false,
                        })
                    }
                    ArchiveAction::Unmount => *confirm_unmount = Some(archive_path),
                    ArchiveAction::LazyUnmount => unreachable!("lazy unmount uses recovery button"),
                    ArchiveAction::Remount => {
                        request = Some(OperationRequest {
                            action,
                            archive_path,
                            cleanup_after_unmount: false,
                        })
                    }
                }
            }
            if can_lazy_unmount
                && ui
                    .add(egui::Button::new("Lazy Unmount"))
                    .on_hover_text(
                        "Emergency recovery option available because normal unmount failed.",
                    )
                    .clicked()
            {
                *confirm_lazy_unmount = Some(record.mount_plan.archive.path.clone());
                *focus_lazy_cancel = true;
            }
            if let Some(operation) = operation {
                ui.spinner();
                ui.label(match operation.action {
                    ArchiveAction::Mount => "Mounting...",
                    ArchiveAction::Unmount => "Unmounting...",
                    ArchiveAction::LazyUnmount => "Lazy unmounting...",
                    ArchiveAction::Remount => "Remounting...",
                });
            }
        });
        if !primary_enabled && let Some(reason) = block_reason {
            ui.colored_label(ui.visuals().weak_text_color(), reason);
        }
        // Always present (not just when blocked) so its mere presence
        // proves the running binary actually contains this code - see
        // `action_readiness_debug_lines`'s doc comment for why this exists
        // (the "Doctor: Ready" summary above can legitimately disagree
        // with this gate, and previously gave no way to see why).
        egui::CollapsingHeader::new("Debug: action readiness")
            .default_open(false)
            .show(ui, |ui| {
                for line in action_readiness_debug_lines {
                    ui.label(line);
                }
            });

        let action = show_platform_section(
            ui,
            persisted,
            platform_details,
            platform_choice,
            platform_custom_text,
            platform_busy,
            clipboard,
        );
        if let Some(action) = action {
            platform_request = Some((record.mount_plan.archive.path.clone(), action));
        }
    });
    SelectedArchiveActions {
        operation: request,
        platform: platform_request,
        inspect: inspect_request,
        cheats_mods: cheats_mods_request,
    }
}

/// Renders the "Set platform" / "Clear manual platform" controls tucked
/// into the selected-archive details - available whenever `persisted` is
/// `Some` (the library database knows this archive), live or cache-only
/// row alike, since this is metadata only, never a mount action (see
/// `show_selected_archive`'s two call sites above). Uses
/// `canonical_platform_names` (the same central list the CLI's
/// `library-set-platform` validates against - never a second,
/// independently-drifting list here), with `CUSTOM_PLATFORM_CHOICE` as
/// the escape hatch for a platform not in that list, mirroring the CLI's
/// `--custom` flag.
pub(crate) fn show_platform_section(
    ui: &mut egui::Ui,
    persisted: Option<&PersistedArchive>,
    platform_details: Option<&PlatformProvenanceDetails>,
    platform_choice: &mut Option<String>,
    platform_custom_text: &mut String,
    platform_busy: bool,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<PlatformAction> {
    ui.add_space(6.0);
    ui.separator();
    ui.strong("Platform");
    let Some(persisted) = persisted else {
        ui.label(
            "Not yet in the library database. Run a library scan to enable platform assignment.",
        );
        return None;
    };

    let fallback_details;
    let details = if let Some(details) = platform_details {
        details
    } else {
        fallback_details = PlatformProvenanceDetails {
            platform: persisted.platform.clone(),
            source: persisted.platform_source.clone(),
            matched_component: None,
            automatic_fallback: None,
        };
        &fallback_details
    };
    for (label, value) in platform_provenance_lines(details) {
        ui.label(format!("{label}: {value}"));
    }
    if details.platform.is_none() {
        widgets::banner(
            ui,
            "Why is this Unknown?",
            UNKNOWN_PLATFORM_EXPLANATION,
            widgets::StatusTone::Info,
        );
    }
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("platform_choice_combo")
            .selected_text(platform_choice.as_deref().unwrap_or("Select platform..."))
            .show_ui(ui, |ui| {
                for name in canonical_platform_names() {
                    ui.selectable_value(platform_choice, Some(name.to_string()), name);
                }
                ui.selectable_value(
                    platform_choice,
                    Some(CUSTOM_PLATFORM_CHOICE.to_string()),
                    CUSTOM_PLATFORM_CHOICE,
                );
            });
        if platform_choice.as_deref() == Some(CUSTOM_PLATFORM_CHOICE) {
            show_text_edit_with_context_menu(ui, platform_custom_text, clipboard, |text_edit| {
                text_edit
            });
        }
    });
    let resolved = resolved_platform_choice(platform_choice.as_deref(), platform_custom_text)
        .map(str::to_string);
    let mut platform_request = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !platform_busy && resolved.is_some(),
                egui::Button::new("Set Platform"),
            )
            .clicked()
            && let Some(platform) = resolved
        {
            platform_request = Some(PlatformAction::Set(platform));
        }
        if persisted.platform_source.as_deref() == Some(MANUAL_PLATFORM_SOURCE)
            && ui
                .add_enabled(!platform_busy, egui::Button::new("Clear Manual Platform"))
                .clicked()
        {
            platform_request = Some(PlatformAction::Clear);
        }
        if platform_busy {
            ui.spinner();
            ui.label("Updating platform...");
        }
    });
    platform_request
}

/// Renders the compact bulk platform action bar - shown only when more
/// than one row is selected (requirement 3): a single selected row
/// already has its own platform picker in the details panel
/// (`show_platform_section`), and showing both for one row would be
/// redundant and ambiguous about which one actually applies. Uses
/// `canonical_platform_names()` (the same central list
/// `show_platform_section`/the CLI validate against) with no free-form
/// custom-text escape hatch - deliberately narrower than the single-row
/// picker, matching the bulk feature's "simple by default" scope
/// (requirement 4).
/// Renders the compact bulk platform action bar - shown only when more
/// than one row is selected (requirement 3): a single selected row
/// already has its own platform picker in the details panel
/// (`show_platform_section`), and showing both for one row would be
/// redundant and ambiguous about which one actually applies. Uses
/// `canonical_platform_names()` (the same central list
/// `show_platform_section`/the CLI validate against) with no free-form
/// custom-text escape hatch - deliberately narrower than the single-row
/// picker, matching the bulk feature's "simple by default" scope
/// (requirement 4).
///
/// Takes `selected_archives` by `&mut` - not because this function
/// starts any database write itself (it only ever returns the requested
/// `BulkPlatformActionKind` for the caller to dispatch asynchronously,
/// exactly as before), but because "Clear selection" is a purely local,
/// synchronous UI action with nothing to dispatch: it just empties the
/// *same* `HashSet` `show_archive_rows` highlights rows from, directly.
pub(crate) fn show_bulk_platform_action_bar(
    ui: &mut egui::Ui,
    selected_archives: &mut HashSet<PathBuf>,
    bulk_platform_choice: &mut Option<String>,
    bulk_platform_busy: bool,
) -> Option<BulkPlatformActionKind> {
    if !bulk_action_bar_visible(selected_archives) {
        return None;
    }

    let mut action = None;
    egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .show(ui, |ui| {
            ui.strong(format!("{} archives selected", selected_archives.len()));
            ui.horizontal(|ui| {
                ui.label("Platform:");
                egui::ComboBox::from_id_salt("bulk_platform_choice_combo")
                    .selected_text(
                        bulk_platform_choice
                            .as_deref()
                            .unwrap_or("Select platform..."),
                    )
                    .show_ui(ui, |ui| {
                        for name in canonical_platform_names() {
                            ui.selectable_value(bulk_platform_choice, Some(name.to_string()), name);
                        }
                    });
                if ui
                    .add_enabled(
                        !bulk_platform_busy && bulk_platform_choice.is_some(),
                        egui::Button::new("Set selected"),
                    )
                    .clicked()
                    && let Some(platform) = bulk_platform_choice.clone()
                {
                    action = Some(BulkPlatformActionKind::Set(platform));
                }
                if ui
                    .add_enabled(!bulk_platform_busy, egui::Button::new("Clear selected"))
                    .clicked()
                {
                    action = Some(BulkPlatformActionKind::Clear);
                }
                if ui.button("Clear selection").clicked() {
                    selected_archives.clear();
                }
                if bulk_platform_busy {
                    ui.spinner();
                    ui.label("Updating...");
                }
            });
        });
    ui.add_space(4.0);
    action
}
