use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::*;

/// The Archive Inspector's column/sort choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum InspectorSortField {
    #[default]
    Path,
    Size,
    Classification,
}

impl std::fmt::Display for InspectorSortField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Path => "Path",
            Self::Size => "Size",
            Self::Classification => "Classification",
        })
    }
}

pub(crate) type InspectorMessage = (RefreshGeneration, Result<InspectorReport, String>);

/// What the Archive Inspector overlay is currently showing for its archive.
pub(crate) enum ArchiveInspectorStatus {
    Loading {
        generation: RefreshGeneration,
        receiver: Receiver<InspectorMessage>,
    },
    Ready(InspectorReport),
    Error(String),
}

/// Complete state for the Archive Inspector overlay for one archive.
pub(crate) struct ArchiveInspectorState {
    pub(crate) archive_path: PathBuf,
    pub(crate) status: ArchiveInspectorStatus,
    pub(crate) search: String,
    pub(crate) classification_filter: Option<InspectorEntryClassification>,
    pub(crate) sort_field: InspectorSortField,
    pub(crate) sort_ascending: bool,
    pub(crate) selected_entry: Option<String>,
    pub(crate) path_column_width: f32,
}

pub(crate) type ArchivePreparationMessage = (
    RefreshGeneration,
    Result<archivefs_core::ArchiveMemberResolution, String>,
);

/// Session-only launch preparation. The archive itself is never rewritten or
/// extracted; this retains the exact member selected from the existing mount.
#[derive(Default)]
pub(crate) enum ArchivePreparationState {
    #[default]
    Idle,
    Inspecting {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        generation: RefreshGeneration,
        receiver: Receiver<ArchivePreparationMessage>,
    },
    Choosing {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        candidates: Vec<archivefs_core::PreparedMemberCandidate>,
    },
    PendingMount {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        candidate: archivefs_core::PreparedMemberCandidate,
    },
    Ready {
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        mount_path: PathBuf,
        candidate: archivefs_core::PreparedMemberCandidate,
    },
    Failed {
        archive_path: PathBuf,
        message: String,
    },
}

impl ArchiveInspectorState {
    pub(crate) fn loading(
        archive_path: PathBuf,
        generation: RefreshGeneration,
        receiver: Receiver<InspectorMessage>,
    ) -> Self {
        Self {
            archive_path,
            status: ArchiveInspectorStatus::Loading {
                generation,
                receiver,
            },
            search: String::new(),
            classification_filter: None,
            sort_field: InspectorSortField::default(),
            sort_ascending: true,
            selected_entry: None,
            path_column_width: DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH,
        }
    }
}

pub(crate) const DEFAULT_INSPECTOR_PATH_COLUMN_WIDTH: f32 = 520.0;

impl ArchiveFsApp {
    pub(crate) fn start_archive_inspection(
        &mut self,
        context: egui::Context,
        archive_path: PathBuf,
    ) {
        self.archive_inspector_generation = self.archive_inspector_generation.next();
        let generation = self.archive_inspector_generation;
        let (sender, receiver) = mpsc::channel();
        let job_path = archive_path.clone();
        std::thread::spawn(move || {
            let result = inspect_archive(&job_path).map_err(|error| error.to_string());
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
        self.archive_inspector = Some(ArchiveInspectorState::loading(
            archive_path,
            generation,
            receiver,
        ));
        self.tools_overlay = ToolsOverlay::ArchiveInspector;
    }

    /// Mirrors `poll_load`'s receiver-then-apply shape so the borrow checker
    /// never sees a conflict between reading and updating inspector state.
    pub(crate) fn poll_archive_inspection(&mut self) {
        let result = match self
            .archive_inspector
            .as_ref()
            .map(|inspector| &inspector.status)
        {
            Some(ArchiveInspectorStatus::Loading {
                generation,
                receiver,
            }) => match receiver.try_recv() {
                Ok(message) => Some(message),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    *generation,
                    Err("The inspection worker stopped unexpectedly.".to_string()),
                )),
            },
            Some(ArchiveInspectorStatus::Ready(_) | ArchiveInspectorStatus::Error(_)) | None => {
                None
            }
        };

        let Some((generation, result)) = result else {
            return;
        };
        if generation != self.archive_inspector_generation {
            return;
        }
        if let Some(inspector) = self.archive_inspector.as_mut() {
            inspector.status = match result {
                Ok(report) => ArchiveInspectorStatus::Ready(report),
                Err(message) => ArchiveInspectorStatus::Error(message),
            };
        }
    }

    pub(crate) fn current_archive_record(&self, archive_path: &Path) -> Option<ArchiveRecord> {
        match &self.state {
            LoadState::Ready(data) => data
                .records
                .iter()
                .find(|record| record.mount_plan.archive.path == archive_path)
                .cloned(),
            LoadState::Loading { previous, .. } => previous.as_ref().and_then(|data| {
                data.records
                    .iter()
                    .find(|record| record.mount_plan.archive.path == archive_path)
                    .cloned()
            }),
            LoadState::Error(_) => None,
        }
    }

    pub(crate) fn start_archive_preparation(
        &mut self,
        context: egui::Context,
        archive_path: PathBuf,
    ) {
        let Some(record) = self.current_archive_record(&archive_path) else {
            self.archive_preparation = ArchivePreparationState::Failed {
                archive_path,
                message: "This game isn't in your library any more. Refresh and try again."
                    .to_string(),
            };
            return;
        };
        let platform = record
            .metadata
            .platform
            .clone()
            .or_else(|| record.identity.platform.clone());
        self.archive_preparation_generation = self.archive_preparation_generation.next();
        let generation = self.archive_preparation_generation;
        let (sender, receiver) = mpsc::channel();
        let job_path = archive_path.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                archivefs_core::inspect_archive(&job_path)
                    .map(|report| {
                        archivefs_core::resolve_prepared_members(&report, platform.as_deref())
                    })
                    .map_err(|error| error.to_string())
            }))
            .unwrap_or_else(|_| {
                Err("archive preparation stopped while inspecting this archive".to_string())
            });
            let _ = sender.send((generation, result));
            context.request_repaint();
        });
        self.archive_preparation = ArchivePreparationState::Inspecting {
            archive_path,
            identity: record.identity,
            generation,
            receiver,
        };
    }

    pub(crate) fn poll_archive_preparation(&mut self, context: &egui::Context) {
        let result = match &self.archive_preparation {
            ArchivePreparationState::Inspecting {
                generation,
                receiver,
                ..
            } => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    *generation,
                    Err("archive preparation worker stopped unexpectedly".to_string()),
                )),
            },
            _ => None,
        };
        let Some((generation, result)) = result else {
            return;
        };
        if generation != self.archive_preparation_generation {
            return;
        }
        let ArchivePreparationState::Inspecting { archive_path, .. } = &self.archive_preparation
        else {
            return;
        };
        let archive_path = archive_path.clone();
        match result {
            Ok(archivefs_core::ArchiveMemberResolution::One(candidate)) => {
                self.begin_archive_member_preparation(context, archive_path, candidate);
            }
            Ok(archivefs_core::ArchiveMemberResolution::Multiple(candidates)) => {
                let identity = self
                    .current_archive_record(&archive_path)
                    .map(|record| record.identity);
                if let Some(identity) = identity {
                    self.archive_preparation = ArchivePreparationState::Choosing {
                        archive_path,
                        identity,
                        candidates,
                    };
                } else {
                    self.archive_preparation = ArchivePreparationState::Failed {
                        archive_path,
                        message: "The selected game changed while its archive was being inspected. Refresh and try again.".to_string(),
                    };
                }
            }
            Ok(archivefs_core::ArchiveMemberResolution::None(message)) | Err(message) => {
                self.archive_preparation = ArchivePreparationState::Failed {
                    archive_path,
                    message,
                };
            }
        }
    }

    fn begin_archive_member_preparation(
        &mut self,
        context: &egui::Context,
        archive_path: PathBuf,
        candidate: archivefs_core::PreparedMemberCandidate,
    ) {
        let Some(record) = self.current_archive_record(&archive_path) else {
            self.archive_preparation = ArchivePreparationState::Failed {
                archive_path,
                message: "This game isn't in your library any more. Refresh and try again."
                    .to_string(),
            };
            return;
        };
        let identity = record.identity.clone();
        if record.mount_state == MountState::Mounted {
            match archivefs_core::prepared_member_path(
                &record.mount_plan.mount_path,
                &candidate.member_name,
            ) {
                Ok(_member_path) => {
                    self.archive_preparation = ArchivePreparationState::Ready {
                        archive_path,
                        identity,
                        mount_path: record.mount_plan.mount_path,
                        candidate,
                    };
                }
                Err(message) => {
                    self.archive_preparation = ArchivePreparationState::Failed {
                        archive_path,
                        message,
                    };
                }
            }
            return;
        }
        self.archive_preparation = ArchivePreparationState::PendingMount {
            archive_path: archive_path.clone(),
            identity,
            candidate,
        };
        let started = self.start_operation(
            context.clone(),
            ArchiveAction::Mount,
            archive_path.clone(),
            false,
        );
        if !started {
            self.archive_preparation = ArchivePreparationState::Failed {
                archive_path,
                message: "Another archive operation is already running. Try Prepare game again when it finishes.".to_string(),
            };
        }
    }

    pub(crate) fn select_archive_member(
        &mut self,
        context: &egui::Context,
        archive_path: PathBuf,
        member_name: String,
    ) {
        let candidate = match &self.archive_preparation {
            ArchivePreparationState::Choosing {
                archive_path: state_path,
                candidates,
                ..
            } if *state_path == archive_path => candidates
                .iter()
                .find(|candidate| candidate.member_name == member_name)
                .cloned(),
            _ => None,
        };
        if let Some(candidate) = candidate {
            self.begin_archive_member_preparation(context, archive_path, candidate);
        }
    }

    pub(crate) fn reconcile_archive_preparation(&mut self) {
        let state_path = match &self.archive_preparation {
            ArchivePreparationState::Idle => None,
            ArchivePreparationState::Inspecting { archive_path, .. }
            | ArchivePreparationState::Choosing { archive_path, .. }
            | ArchivePreparationState::PendingMount { archive_path, .. }
            | ArchivePreparationState::Ready { archive_path, .. }
            | ArchivePreparationState::Failed { archive_path, .. } => Some(archive_path),
        };
        if state_path != self.archive_context.focused.as_ref() {
            if state_path.is_some() {
                self.archive_preparation_generation = self.archive_preparation_generation.next();
                self.archive_preparation = ArchivePreparationState::Idle;
            }
            return;
        }
        let Some(path) = state_path.cloned() else {
            return;
        };
        let Some(record) = self.current_archive_record(&path) else {
            self.archive_preparation = ArchivePreparationState::Idle;
            return;
        };
        let state = std::mem::take(&mut self.archive_preparation);
        self.archive_preparation = match state {
            ArchivePreparationState::PendingMount {
                archive_path,
                identity,
                candidate,
            } if record.mount_state == MountState::Mounted => {
                self.finalize_archive_member(archive_path, identity, candidate, &record)
            }
            ArchivePreparationState::Choosing { identity, .. } if identity != record.identity => {
                ArchivePreparationState::Idle
            }
            ArchivePreparationState::Inspecting { identity, .. } if identity != record.identity => {
                ArchivePreparationState::Idle
            }
            ArchivePreparationState::Ready {
                archive_path: _archive_path,
                identity,
                mount_path,
                candidate,
            } if identity != record.identity
                || record.mount_state != MountState::Mounted
                || mount_path != record.mount_plan.mount_path
                || archivefs_core::prepared_member_path(&mount_path, &candidate.member_name)
                    .is_err() =>
            {
                ArchivePreparationState::Idle
            }
            other => other,
        };
    }

    fn finalize_archive_member(
        &self,
        archive_path: PathBuf,
        identity: archivefs_core::ArchiveIdentity,
        candidate: archivefs_core::PreparedMemberCandidate,
        record: &ArchiveRecord,
    ) -> ArchivePreparationState {
        match archivefs_core::prepared_member_path(
            &record.mount_plan.mount_path,
            &candidate.member_name,
        ) {
            Ok(_) => ArchivePreparationState::Ready {
                archive_path,
                identity,
                mount_path: record.mount_plan.mount_path.clone(),
                candidate,
            },
            Err(message) => ArchivePreparationState::Failed {
                archive_path,
                message,
            },
        }
    }

    pub(crate) fn archive_preparation_view(
        &self,
        archive_path: &Path,
    ) -> (
        bool,
        Option<Vec<archivefs_core::PreparedMemberCandidate>>,
        Option<String>,
    ) {
        match &self.archive_preparation {
            ArchivePreparationState::Ready {
                archive_path: path, ..
            } if path == archive_path => (true, None, None),
            ArchivePreparationState::Choosing {
                archive_path: path,
                candidates,
                ..
            } if path == archive_path => (false, Some(candidates.clone()), None),
            ArchivePreparationState::Inspecting {
                archive_path: path, ..
            } if path == archive_path => (
                false,
                None,
                Some("Inspecting this archive safely…".to_string()),
            ),
            ArchivePreparationState::PendingMount {
                archive_path: path, ..
            } if path == archive_path => (false, None, Some("Preparing this game…".to_string())),
            ArchivePreparationState::Failed {
                archive_path: path,
                message,
            } if path == archive_path => (false, None, Some(message.clone())),
            _ => (false, None, None),
        }
    }

    pub(crate) fn resolved_archive_member_path(&self, record: &ArchiveRecord) -> Option<PathBuf> {
        let ArchivePreparationState::Ready {
            archive_path,
            identity,
            mount_path,
            candidate,
        } = &self.archive_preparation
        else {
            return None;
        };
        if archive_path != &record.mount_plan.archive.path
            || identity != &record.identity
            || mount_path != &record.mount_plan.mount_path
            || record.mount_state != MountState::Mounted
        {
            return None;
        }
        archivefs_core::prepared_member_path(mount_path, &candidate.member_name).ok()
    }
}

pub(crate) fn show_archive_inspector_panel(
    ui: &mut egui::Ui,
    state: &mut ArchiveInspectorState,
    clipboard: &mut dyn ClipboardBackend,
) -> bool {
    let close = widgets::show_tools_overlay_header(ui, "Archive Inspector");
    ui.horizontal(|ui| {
        ui.label("Archive:");
        let path_text = state.archive_path.display().to_string();
        ui.add(egui::Label::new(&path_text).selectable(true).wrap());
        if ui.small_button("Copy").clicked() {
            let _ = clipboard.set_text(path_text.clone());
        }
    });
    ui.add_space(4.0);

    match &state.status {
        ArchiveInspectorStatus::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Inspecting archive - this runs in the background.");
            });
            return close;
        }
        ArchiveInspectorStatus::Error(message) => {
            ui.colored_label(ui.visuals().error_fg_color, message);
            return close;
        }
        ArchiveInspectorStatus::Ready(_) => {}
    }
    let ArchiveInspectorStatus::Ready(report) = &state.status else {
        unreachable!("every other status already returned above");
    };

    if report.truncated {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Showing the first {} of {} entries - this view is incomplete. Use a more \
                 specific search to find a particular entry.",
                report.entries.len(),
                report.total_entries_in_archive
            ),
        );
        ui.add_space(2.0);
    }

    ui.horizontal_wrapped(|ui| {
        for classification in InspectorEntryClassification::ALL {
            let count = report
                .entries
                .iter()
                .filter(|entry| entry.classification == classification)
                .count();
            summary_value(ui, classification.label(), count);
        }
    });
    ui.separator();

    ui.horizontal_wrapped(|ui| {
        ui.label("Search path:");
        show_text_edit_with_context_menu(ui, &mut state.search, clipboard, |text_edit| {
            text_edit
                .id_salt("archivefs_inspector_search")
                .desired_width(260.0)
        });
        ui.label("Classification:");
        egui::ComboBox::from_id_salt("inspector_classification_filter")
            .selected_text(
                state
                    .classification_filter
                    .map(InspectorEntryClassification::label)
                    .unwrap_or("All"),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.classification_filter, None, "All");
                for classification in InspectorEntryClassification::ALL {
                    ui.selectable_value(
                        &mut state.classification_filter,
                        Some(classification),
                        classification.label(),
                    );
                }
            });
        ui.label("Sort by:");
        egui::ComboBox::from_id_salt("inspector_sort_field")
            .selected_text(state.sort_field.to_string())
            .show_ui(ui, |ui| {
                for field in [
                    InspectorSortField::Path,
                    InspectorSortField::Size,
                    InspectorSortField::Classification,
                ] {
                    ui.selectable_value(&mut state.sort_field, field, field.to_string());
                }
            });
        ui.checkbox(&mut state.sort_ascending, "Ascending");
    });

    let visible = visible_inspector_entry_indices(
        &report.entries,
        &state.search,
        state.classification_filter,
        state.sort_field,
        state.sort_ascending,
    );
    ui.horizontal_wrapped(|ui| {
        summary_value(ui, "Entries shown", visible.len());
        summary_value(ui, "Total entries", report.entries.len());
    });
    ui.separator();

    if report.entries.is_empty() {
        ui.label("This archive has no entries.");
        return close;
    }
    if visible.is_empty() {
        ui.label("No entries match the current search/filter.");
        return close;
    }

    let row_height = ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.spacing().interact_size.y);
    let spacing = ui.spacing().item_spacing.x;

    ui.strong("Entries");
    egui::ScrollArea::horizontal()
        .id_salt("inspector_entries_horizontal")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let start_of_frame_width = state.path_column_width;
            ui.set_min_width(start_of_frame_width + spacing + INSPECTOR_DETAILS_COLUMN_WIDTH);

            let mut path_header_rect = None;
            ui.horizontal(|ui| {
                let response = ui.add_sized(
                    [start_of_frame_width, row_height],
                    egui::Label::new(egui::RichText::new("Path").strong()),
                );
                path_header_rect = Some(response.rect);
                ui.add_sized(
                    [INSPECTOR_DETAILS_COLUMN_WIDTH, row_height],
                    egui::Label::new(egui::RichText::new("Details").strong()),
                );
            });
            if let Some(path_header_rect) = path_header_rect {
                let handle_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        path_header_rect.right() - COLUMN_RESIZE_HANDLE_WIDTH,
                        path_header_rect.top(),
                    ),
                    egui::vec2(COLUMN_RESIZE_HANDLE_WIDTH, path_header_rect.height()),
                );
                show_column_resize_handle(
                    ui,
                    egui::Id::new("inspector_path_column_resize"),
                    handle_rect,
                    &mut state.path_column_width,
                );
            }
            ui.separator();

            // Re-read after the resize handle, which may have just
            // changed `state.path_column_width` this very frame - the
            // rows below must always paint with *this* frame's width,
            // never a one-frame-stale copy (matches the Library table's
            // identical fix in `show_loaded_data`).
            let widths = [state.path_column_width, INSPECTOR_DETAILS_COLUMN_WIDTH];

            let body_height = ui.available_height().max(row_height);
            egui::ScrollArea::vertical()
                .id_salt("inspector_entries_vertical")
                .max_height(body_height)
                .auto_shrink([false, false])
                .show_rows(ui, row_height, visible.len(), |ui, row_range| {
                    for visible_index in row_range {
                        let entry_index = visible[visible_index];
                        let entry = &report.entries[entry_index];
                        let selected = state.selected_entry.as_deref() == Some(entry.name.as_str());
                        let response = show_inspector_row(ui, entry, row_height, selected, &widths);
                        if response.clicked() {
                            state.selected_entry = Some(entry.name.clone());
                        }
                    }
                });
        });

    let Some(selected_name) = state.selected_entry.clone() else {
        ui.label("Select an entry to view its details.");
        return close;
    };
    let Some(selected_entry) = report
        .entries
        .iter()
        .find(|entry| entry.name == selected_name)
    else {
        return close;
    };

    ui.separator();
    ui.strong("Selected entry");
    egui::Grid::new("inspector_selected_entry_details")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            detail_row_with_copy(ui, "Path", &selected_entry.name, clipboard);
            detail_row(
                ui,
                "Type",
                match selected_entry.kind {
                    InspectorEntryKind::File => "File",
                    InspectorEntryKind::Directory => "Directory",
                },
            );
            detail_row(ui, "Classification", selected_entry.classification.label());
            detail_row(
                ui,
                "Uncompressed size",
                &format_size(Some(selected_entry.uncompressed_size)),
            );
            detail_row(
                ui,
                "Compressed size",
                &format_size(selected_entry.compressed_size),
            );
            detail_row(
                ui,
                "Compression method",
                selected_entry
                    .compression_method
                    .as_deref()
                    .unwrap_or("Unknown"),
            );
        });

    close
}

pub(crate) const INSPECTOR_DETAILS_COLUMN_WIDTH: f32 = 300.0;

/// Whether one entry matches the Archive Inspector's current search text
/// (case-insensitive substring against its exact stored name) and
/// classification filter - pure, so it is directly testable without
/// rendering anything, mirroring `health_issue_matches`'s existing
/// convention in this file.
pub(crate) fn inspector_entry_matches(
    entry: &InspectorEntry,
    search_lower: &str,
    classification_filter: Option<InspectorEntryClassification>,
) -> bool {
    if let Some(filter) = classification_filter
        && entry.classification != filter
    {
        return false;
    }
    search_lower.is_empty() || entry.name.to_lowercase().contains(search_lower)
}

/// Filters and sorts the already-inspected entry list without ever
/// mutating it - `entries` is only ever read here, exactly like
/// `visible_health_issue_indices` reads its own `issues` slice.
pub(crate) fn visible_inspector_entry_indices(
    entries: &[InspectorEntry],
    search: &str,
    classification_filter: Option<InspectorEntryClassification>,
    sort_field: InspectorSortField,
    sort_ascending: bool,
) -> Vec<usize> {
    let search_lower = search.trim().to_lowercase();
    let mut indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            inspector_entry_matches(entry, &search_lower, classification_filter).then_some(index)
        })
        .collect();
    indices.sort_by(|&left, &right| {
        let (left_entry, right_entry) = (&entries[left], &entries[right]);
        let ordering = match sort_field {
            InspectorSortField::Path => left_entry.name.cmp(&right_entry.name),
            InspectorSortField::Size => left_entry
                .uncompressed_size
                .cmp(&right_entry.uncompressed_size),
            InspectorSortField::Classification => {
                left_entry.classification.cmp(&right_entry.classification)
            }
        }
        .then_with(|| left_entry.name.cmp(&right_entry.name));
        if sort_ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    indices
}

pub(crate) fn inspector_entry_details_text(entry: &InspectorEntry) -> String {
    match entry.kind {
        InspectorEntryKind::Directory => "Directory".to_string(),
        InspectorEntryKind::File => format!(
            "{} \u{2014} {} \u{2014} compressed {} \u{2014} {}",
            entry.classification.label(),
            format_size(Some(entry.uncompressed_size)),
            format_size(entry.compressed_size),
            entry.compression_method.as_deref().unwrap_or("Unknown"),
        ),
    }
}

/// Renders one Archive Inspector entry row - the same technique
/// `show_data_row` uses for the Library table (a single `Sense::click()`
/// region with `Painter`-painted cell text, never a child widget inside
/// that region), generalised to two columns via the same now-slice-based
/// `cell_index_at`/`hovered_cell_full_text` helpers the Library table
/// itself uses. Selection here is single (`selected: bool`, no
/// multi-select/Ctrl-click) - "Selecting one entry shows its complete
/// details" never needed the Library table's fuller multi-select model.
pub(crate) fn show_inspector_row(
    ui: &mut egui::Ui,
    entry: &InspectorEntry,
    row_height: f32,
    selected: bool,
    widths: &[f32],
) -> egui::Response {
    let spacing = ui.spacing().item_spacing.x;
    let width = widths.iter().sum::<f32>() + spacing * (widths.len().saturating_sub(1) as f32);
    let (_, rect) = ui.allocate_space(egui::vec2(width, row_height));
    let row_id = egui::Id::new("inspector_row").with(&entry.name);
    let mut response = ui.interact(rect, row_id, egui::Sense::click());

    let visuals = ui.visuals();
    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
    }

    let details_text = inspector_entry_details_text(entry);
    let cells: [&str; 2] = [entry.name.as_str(), details_text.as_str()];
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    let mut x = rect.left();
    for (text, column_width) in cells.iter().zip(widths.iter().copied()) {
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(x, rect.top()),
            egui::vec2(column_width, row_height),
        );
        ui.painter().with_clip_rect(cell_rect).text(
            egui::pos2(x + 2.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            *text,
            font_id.clone(),
            color,
        );
        x += column_width + spacing;
    }

    let pointer_x = response.hover_pos().map(|pos| pos.x);
    if let Some(full_text) =
        hovered_cell_full_text(pointer_x, rect.left(), &cells, widths, spacing, |text| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_string(), font_id.clone(), color)
                    .size()
                    .x
            })
        })
    {
        response = response.on_hover_text(full_text.to_string());
    }

    response
}
