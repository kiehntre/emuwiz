//! Native Blue "Needs Attention" (boards `D11-LG-NeedsAttention`, `-NA*`).
//!
//! Read-only. It shows the saved attention evidence the legacy page also
//! reads (`needs_attention::load_saved_snapshot`: catalogue opened read-only,
//! plus operation receipts), filtered and paged by core's
//! `AttentionSnapshot::page`. Every button only navigates to the workflow
//! that owns the item; nothing here changes a file, walks the library or
//! touches the network.

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use archivefs_core::attention::{
    ATTENTION_PAGE_SIZE, AttentionCategory, AttentionDestination, AttentionFilters, AttentionItem,
    AttentionSeverity, AttentionSnapshot, AttentionStateFilter,
};
use eframe::egui::{self, RichText};

use super::routes::{Route, Section};
use super::simple_shell::palette;
use crate::ui::components::{self as widgets, StatusTone};
use crate::ui::theme::{self, blue};

/// Saved evidence refreshes at most this often while the page is in use.
pub(super) const REFRESH: Duration = Duration::from_secs(30);
/// Below this width the details panel follows the list.
pub(super) const SIDE_BY_SIDE_MIN_WIDTH: f32 = 1100.0;

pub(super) const READ_ONLY_NOTE: &str =
    "Buttons here only open the workflow that owns the item. Nothing on this page changes a file.";
pub(super) const SESSION_NOTE: &str = "Diagnostics and launch checks run in Setup & Doctor and Browse & Play; their findings are not repeated here until saved.";

#[derive(Default)]
pub(super) struct NeedsAttentionState {
    /// Off in tests and previews: the page then shows only what it is given.
    auto_load: bool,
    pub(super) snapshot: AttentionSnapshot,
    pub(super) loaded: bool,
    receiver: Option<Receiver<AttentionSnapshot>>,
    last_started: Option<Instant>,
    pub(super) filters: AttentionFilters,
    pub(super) selected: Option<String>,
}

impl NeedsAttentionState {
    pub(super) fn live() -> Self {
        Self {
            auto_load: true,
            ..Self::default()
        }
    }

    /// Takes a finished load and starts the next one when due.
    pub(super) fn poll(&mut self, context: &egui::Context) {
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(snapshot) => {
                    self.snapshot = snapshot;
                    self.loaded = true;
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.snapshot = AttentionSnapshot::default();
                    self.snapshot.coverage_notes.push(
                        "Attention refresh failed; saved evidence is unavailable right now.".into(),
                    );
                    self.loaded = true;
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let due = self
            .last_started
            .is_none_or(|last| last.elapsed() >= REFRESH);
        if self.auto_load && self.receiver.is_none() && due {
            self.last_started = Some(Instant::now());
            let (sender, receiver) = mpsc::channel();
            self.receiver = Some(receiver);
            let context = context.clone();
            std::thread::spawn(move || {
                let _ = sender.send(crate::needs_attention::load_saved_snapshot());
                context.request_repaint();
            });
        }
        if self.auto_load {
            context.request_repaint_after(REFRESH);
        }
    }

    /// Unresolved items, for the Home card. `None` until the first load.
    pub(super) fn unresolved(&self) -> Option<usize> {
        self.loaded
            .then(|| self.snapshot.counts().iter().sum::<usize>())
    }
}

/// The Blue page that owns each destination.
pub(super) fn route_for(destination: AttentionDestination) -> Route {
    Route::Section(match destination {
        AttentionDestination::Sources | AttentionDestination::Discovery => Section::Sources,
        AttentionDestination::DatReview => Section::Dat,
        AttentionDestination::Duplicates | AttentionDestination::ExactDuplicates => {
            Section::Duplicates
        }
        AttentionDestination::Problems => Section::Problems,
        AttentionDestination::EmulatorSetup => Section::Emulators,
        AttentionDestination::LaunchReadiness => Section::Launch,
        AttentionDestination::History => Section::History,
        AttentionDestination::Romm => Section::Romm,
        AttentionDestination::EsDe | AttentionDestination::LibraryOrganisation => Section::Build,
        AttentionDestination::CheatsMods => Section::Mods,
        AttentionDestination::DiscConversion => Section::Converter,
    })
}

fn open_label(destination: AttentionDestination) -> String {
    format!("Open {}", route_for(destination).section().title())
}

fn severity_tone(severity: AttentionSeverity) -> StatusTone {
    match severity {
        AttentionSeverity::Blocking | AttentionSeverity::ActionNeeded => StatusTone::Warning,
        AttentionSeverity::Warning => StatusTone::Pending,
        AttentionSeverity::Info => StatusTone::Info,
    }
}

fn state_label(state: AttentionStateFilter) -> &'static str {
    match state {
        AttentionStateFilter::Unresolved => "Unresolved",
        AttentionStateFilter::Resolved => "Resolved history",
        AttentionStateFilter::All => "All states",
    }
}

fn unix_label(value: Option<i64>) -> String {
    value.map_or_else(|| "Unknown".into(), |seconds| seconds.to_string())
}

/// Renders the page; returns where to go when an Open button was pressed.
pub(super) fn show(ui: &mut egui::Ui, state: &mut NeedsAttentionState) -> Option<Route> {
    let mut go = None;
    if !state.loaded {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new("Loading saved attention evidence in the background…")
                    .color(palette::SECONDARY_TEXT),
            );
        });
        ui.add_space(theme::SPACE_SM);
    }
    counts_row(ui, &state.snapshot);
    if state.snapshot.limited {
        widgets::banner(
            ui,
            "Partial coverage",
            "Summary or receipt limits were reached. Counts are a lower bound; review complete details in the source workflows.",
            StatusTone::Warning,
        );
    }
    filter_row(ui, state);
    ui.add_space(theme::SPACE_SM);

    let wide = ui.available_width() >= SIDE_BY_SIDE_MIN_WIDTH;
    if wide {
        let gap = 20.0;
        let right = (ui.available_width() * 0.32).clamp(340.0, 560.0);
        let left = ui.available_width() - right - gap;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                egui::vec2(left, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left);
                    go = list(ui, state);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(right, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right);
                    side_panels(ui, state);
                },
            );
        });
    } else {
        go = list(ui, state);
        ui.add_space(theme::SPACE_SM);
        side_panels(ui, state);
    }
    go
}

fn counts_row(ui: &mut egui::Ui, snapshot: &AttentionSnapshot) {
    let counts = snapshot.counts();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{} unresolved", counts.iter().sum::<usize>()))
                .strong()
                .size(18.0)
                .color(blue::PRIMARY_TEXT),
        );
        for (severity, count) in AttentionSeverity::ALL.into_iter().zip(counts) {
            if count > 0 {
                widgets::status_badge(
                    ui,
                    format!("{count} {}", severity.label().to_lowercase()),
                    severity_tone(severity),
                );
            }
        }
    });
    ui.add_space(theme::SPACE_XS);
}

fn filter_row(ui: &mut egui::Ui, state: &mut NeedsAttentionState) {
    let before = state.filters.clone();
    let platforms: std::collections::BTreeSet<String> = state
        .snapshot
        .items()
        .filter_map(|item| item.platform.clone())
        .collect();
    let workflows: std::collections::BTreeSet<String> = state
        .snapshot
        .items()
        .map(|item| item.source_workflow.clone())
        .collect();
    let filters = &mut state.filters;
    let height = ui.spacing().interact_size.y;
    // Each control gets a fixed slot, so the row wraps: a ComboBox placed
    // directly in a wrapping row does not report its size up front and
    // pushed the page wider than the window.
    let slot = |ui: &mut egui::Ui, width: f32, add: &mut dyn FnMut(&mut egui::Ui)| {
        ui.allocate_ui_with_layout(
            egui::vec2(width, height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| add(ui),
        );
    };
    const COMBO: f32 = 160.0;
    ui.horizontal_wrapped(|ui| {
        slot(ui, 230.0, &mut |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut filters.search)
                    .hint_text("Search attention items")
                    .desired_width(220.0),
            );
        });
        slot(ui, COMBO, &mut |ui| {
            egui::ComboBox::from_id_salt("v2_attention_severity")
                .width(COMBO - 12.0)
                .selected_text(
                    filters
                        .severity
                        .map_or("All severities", AttentionSeverity::label),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filters.severity, None, "All severities");
                    for severity in AttentionSeverity::ALL {
                        ui.selectable_value(
                            &mut filters.severity,
                            Some(severity),
                            severity.label(),
                        );
                    }
                });
        });
        slot(ui, COMBO, &mut |ui| {
            egui::ComboBox::from_id_salt("v2_attention_category")
                .width(COMBO - 12.0)
                .selected_text(
                    filters
                        .category
                        .map_or("All categories", AttentionCategory::label),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filters.category, None, "All categories");
                    for category in AttentionCategory::ALL {
                        ui.selectable_value(
                            &mut filters.category,
                            Some(category),
                            category.label(),
                        );
                    }
                });
        });
        slot(ui, COMBO, &mut |ui| {
            egui::ComboBox::from_id_salt("v2_attention_platform")
                .width(COMBO - 12.0)
                .selected_text(filters.platform.as_deref().unwrap_or("All platforms"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filters.platform, None, "All platforms");
                    for platform in &platforms {
                        ui.selectable_value(
                            &mut filters.platform,
                            Some(platform.clone()),
                            platform,
                        );
                    }
                });
        });
        slot(ui, COMBO, &mut |ui| {
            egui::ComboBox::from_id_salt("v2_attention_workflow")
                .width(COMBO - 12.0)
                .selected_text(filters.workflow.as_deref().unwrap_or("All workflows"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut filters.workflow, None, "All workflows");
                    for workflow in &workflows {
                        ui.selectable_value(
                            &mut filters.workflow,
                            Some(workflow.clone()),
                            workflow,
                        );
                    }
                });
        });
        slot(ui, COMBO, &mut |ui| {
            egui::ComboBox::from_id_salt("v2_attention_state")
                .width(COMBO - 12.0)
                .selected_text(state_label(filters.state))
                .show_ui(ui, |ui| {
                    for option in [
                        AttentionStateFilter::Unresolved,
                        AttentionStateFilter::Resolved,
                        AttentionStateFilter::All,
                    ] {
                        ui.selectable_value(&mut filters.state, option, state_label(option));
                    }
                });
        });
        slot(ui, 150.0, &mut |ui| {
            ui.checkbox(&mut filters.newest_first, "Newest first");
        });
    });
    if state.filters != before {
        state.filters.page = 0;
    }
}

fn list(ui: &mut egui::Ui, state: &mut NeedsAttentionState) -> Option<Route> {
    let mut go = None;
    let page = state.snapshot.page(&state.filters);
    state.filters.page = page.page;
    // Keep a selection only while it is on the visible page.
    if !page
        .items
        .iter()
        .any(|item| Some(&item.id) == state.selected.as_ref())
    {
        state.selected = page.items.first().map(|item| item.id.clone());
    }
    if page.total == 0 && state.loaded {
        let nothing = state.snapshot.counts().iter().sum::<usize>() == 0;
        widgets::empty_state(
            ui,
            if nothing {
                "Nothing needs your attention"
            } else {
                "No items match these filters"
            },
            if nothing {
                "No unresolved issues were found in the checked sources. Unchecked sources are not assumed healthy."
            } else {
                "Clear a filter or the search to see the other items."
            },
            None,
        );
    }
    for item in &page.items {
        let selected = Some(&item.id) == state.selected.as_ref();
        let (clicked_row, open) = item_row(ui, item, selected);
        if clicked_row {
            state.selected = Some(item.id.clone());
        }
        if open {
            go = Some(route_for(item.destination));
        }
    }
    ui.add_space(theme::SPACE_XS);
    let last = (page.page + 1) * ATTENTION_PAGE_SIZE >= page.total;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(page.page > 0, egui::Button::new("Previous"))
            .on_disabled_hover_text("First page")
            .clicked()
        {
            state.filters.page -= 1;
        }
        if ui
            .add_enabled(!last, egui::Button::new("Next"))
            .on_disabled_hover_text("Last page")
            .clicked()
        {
            state.filters.page += 1;
        }
        ui.label(
            RichText::new(format!(
                "Page {} · {} matching summaries · up to {ATTENTION_PAGE_SIZE} per page",
                page.page + 1,
                page.total
            ))
            .color(palette::SECONDARY_TEXT),
        );
    });
    go
}

/// One item: severity, title, summary and its Open button. Returns
/// (row selected, Open pressed).
fn item_row(ui: &mut egui::Ui, item: &AttentionItem, selected: bool) -> (bool, bool) {
    let mut open = false;
    let mut select = false;
    let frame = egui::Frame::new()
        .fill(if selected {
            blue::SELECTED_FILL
        } else {
            blue::CARD_SURFACE_TOP
        })
        .stroke(egui::Stroke::new(
            1.0_f32,
            if selected {
                blue::SELECTED_OUTLINE
            } else {
                palette::BORDER_SUBTLE
            },
        ))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10));
    ui.push_id(("v2_attention_item", &item.id), |ui| {
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            let button_width = 230.0_f32.min(ui.available_width() * 0.4);
            ui.horizontal_top(|ui| {
                let text_width = (ui.available_width() - button_width - 12.0).max(160.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(text_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.horizontal_wrapped(|ui| {
                            widgets::status_badge(
                                ui,
                                item.severity.label(),
                                severity_tone(item.severity),
                            );
                            let title = RichText::new(&item.title)
                                .strong()
                                .color(blue::PRIMARY_TEXT);
                            if ui
                                .add(egui::Label::new(title).sense(egui::Sense::click()))
                                .on_hover_text("Show evidence and recovery details")
                                .clicked()
                            {
                                select = true;
                            }
                        });
                        let mut line = item.summary.clone();
                        if !item.recommended_action.is_empty() {
                            if !line.is_empty() {
                                line.push(' ');
                            }
                            line.push_str(&item.recommended_action);
                            line.push('.');
                        }
                        ui.label(RichText::new(line).color(blue::BODY_TEXT));
                        if !selected
                            && ui
                                .small_button("Details")
                                .on_hover_text("Show evidence and recovery details")
                                .clicked()
                        {
                            select = true;
                        }
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let style = if selected {
                        widgets::ActionStyle::Primary
                    } else {
                        widgets::ActionStyle::Secondary
                    };
                    if widgets::action_button(ui, open_label(item.destination), style, true)
                        .clicked()
                    {
                        open = true;
                    }
                });
            });
        });
    });
    ui.add_space(6.0);
    (select, open)
}

fn side_panels(ui: &mut egui::Ui, state: &NeedsAttentionState) {
    let selected = state
        .selected
        .as_ref()
        .and_then(|id| state.snapshot.items().find(|item| &item.id == id));
    super::game_detail_blue::side_card(ui, "EVIDENCE AND RECOVERY DETAILS", |ui| {
        let Some(item) = selected else {
            ui.label(
                RichText::new("Choose an item to see its evidence.").color(palette::SECONDARY_TEXT),
            );
            return;
        };
        // Plain rows, not a Grid: wrapped values in a Grid size their column
        // to the text and push the page wider than the window.
        let mut row = |label: &str, value: String| {
            ui.horizontal_top(|ui| {
                let label_width = 140.0_f32.min(ui.available_width() * 0.4);
                ui.allocate_ui_with_layout(
                    egui::vec2(label_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(label_width);
                        ui.label(RichText::new(label).color(blue::DIM_TEXT));
                    },
                );
                ui.add(egui::Label::new(RichText::new(value).color(blue::BODY_TEXT)).wrap());
            });
        };
        row(
            "Affected path",
            item.affected
                .clone()
                .unwrap_or_else(|| "Not recorded".into()),
        );
        row(
            "Workflow",
            format!(
                "{} · {} affected",
                item.source_workflow, item.affected_count
            ),
        );
        row("Provenance", item.provenance.clone());
        row("Recoverability", item.recoverability.clone());
        row("First detected", unix_label(item.first_detected));
        row("Last observed", unix_label(item.last_observed));
        row(
            "Source record ids",
            if item.source_records.is_empty() {
                "None recorded".into()
            } else {
                item.source_records.join(", ")
            },
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Times are Unix seconds; unknown is not inferred. {READ_ONLY_NOTE}"
            ))
            .size(13.0)
            .color(blue::DIM_TEXT),
        );
    });
    super::game_detail_blue::side_card(ui, "COVERAGE AND FRESHNESS", |ui| {
        let snapshot = &state.snapshot;
        ui.label(
            RichText::new(format!(
                "Refreshes automatically (at most every {} seconds). {} source records; {} catalogue queries; {} ms. Unchecked sources are not assumed healthy.",
                REFRESH.as_secs(),
                snapshot.source_rows,
                snapshot.query_count,
                snapshot.query_millis
            ))
            .color(blue::BODY_TEXT),
        );
        ui.label(RichText::new(SESSION_NOTE).color(palette::SECONDARY_TEXT));
        for note in &snapshot.coverage_notes {
            ui.label(RichText::new(note).color(palette::SECONDARY_TEXT));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::attention::AttentionItem;

    fn item(
        id: &str,
        severity: AttentionSeverity,
        destination: AttentionDestination,
    ) -> AttentionItem {
        let mut item = AttentionItem::new(
            id.into(),
            AttentionCategory::Dat,
            severity,
            format!("Title {id}"),
            destination,
        );
        item.summary = "Summary.".into();
        item
    }

    fn render(state: &mut NeedsAttentionState, width: f32) -> (Option<Route>, Vec<String>) {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        let mut go = None;
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 900.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    go = show(ui, state);
                });
            },
        );
        let mut texts = Vec::new();
        fn gather(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| gather(shape, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            gather(&shape.shape, &mut texts);
        }
        (go, texts)
    }

    #[test]
    fn the_page_never_grows_wider_than_its_column() {
        for width in [600.0, 950.0, 1100.0, 1600.0] {
            let mut state = NeedsAttentionState::default();
            state.loaded = true;
            for (index, severity) in AttentionSeverity::ALL.into_iter().enumerate() {
                let mut entry = item(
                    &format!("id-{index}"),
                    severity,
                    AttentionDestination::ExactDuplicates,
                );
                entry.summary =
                    "A long summary that must wrap rather than widen the page. ".repeat(4);
                entry.provenance =
                    "Saved catalogue evidence; no live filesystem verification on this page".into();
                entry.source_records = vec!["duplicate:name_candidates".repeat(3)];
                entry.platform = Some(format!("Platform {index}"));
                state.snapshot.insert(entry);
            }
            let context = egui::Context::default();
            crate::gui_v2::readable_style(&context);
            let mut used = 0.0_f32;
            for _ in 0..3 {
                context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 900.0),
                        )),
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                show(ui, &mut state);
                                used = ui.min_rect().right();
                            });
                        });
                    },
                );
            }
            assert!(used <= width, "at {width} the page used {used}");
        }
    }

    #[test]
    fn every_destination_opens_an_existing_blue_page() {
        use AttentionDestination as D;
        for destination in [
            D::Sources,
            D::Discovery,
            D::DatReview,
            D::Duplicates,
            D::Problems,
            D::EmulatorSetup,
            D::LaunchReadiness,
            D::History,
            D::Romm,
            D::EsDe,
            D::LibraryOrganisation,
            D::CheatsMods,
            D::DiscConversion,
            D::ExactDuplicates,
        ] {
            let route = route_for(destination);
            assert!(matches!(route, Route::Section(_)), "{destination:?}");
            assert!(open_label(destination).starts_with("Open "));
        }
        assert_eq!(
            route_for(D::DatReview),
            Route::Section(Section::Dat),
            "DAT items open DAT Management"
        );
    }

    #[test]
    fn the_page_lists_items_with_details_and_navigates_only() {
        let mut state = NeedsAttentionState::default();
        state.loaded = true;
        state.snapshot.insert(item(
            "a",
            AttentionSeverity::Blocking,
            AttentionDestination::DatReview,
        ));
        state.snapshot.insert(item(
            "b",
            AttentionSeverity::Info,
            AttentionDestination::History,
        ));
        let (go, texts) = render(&mut state, 1600.0);
        assert_eq!(go, None, "rendering alone never navigates");
        for wanted in [
            "2 unresolved",
            "Title a",
            "Title b",
            "EVIDENCE AND RECOVERY DETAILS",
            "COVERAGE AND FRESHNESS",
            "Open DAT Management",
            "Open History",
        ] {
            assert!(
                texts.iter().any(|text| text.contains(wanted)),
                "{wanted} missing from {texts:?}"
            );
        }
        // The first item is selected for the details panel.
        assert!(state.selected.is_some());
    }

    #[test]
    fn empty_and_narrow_layouts_stay_honest() {
        let mut state = NeedsAttentionState::default();
        state.loaded = true;
        let (_, texts) = render(&mut state, 600.0);
        assert!(
            texts
                .iter()
                .any(|text| text == "Nothing needs your attention")
        );
        assert!(
            texts
                .iter()
                .any(|text| text.contains("not assumed healthy"))
        );
        let mut loading = NeedsAttentionState::default();
        let (_, texts) = render(&mut loading, 600.0);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("Loading saved attention"))
        );
        assert!(
            !texts
                .iter()
                .any(|text| text == "Nothing needs your attention")
        );
    }

    #[test]
    fn a_default_state_never_starts_a_load() {
        let context = egui::Context::default();
        let mut state = NeedsAttentionState::default();
        state.poll(&context);
        assert!(state.receiver.is_none());
        assert_eq!(state.unresolved(), None);
    }

    #[test]
    fn loading_saved_evidence_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("data/library.sqlite");
        let receipts = archivefs_core::operation::AttentionReceiptPaths {
            rename: Some(root.path().join("rename")),
            library_views: Some(root.path().join("views")),
            shared_apply: Some(root.path().join("shared")),
            database: Some(database.clone()),
        };
        let snapshot = crate::needs_attention::load_saved_snapshot_from(Some(database), &receipts);
        assert_eq!(snapshot.counts().iter().sum::<usize>(), 0);
        assert!(
            snapshot
                .coverage_notes
                .iter()
                .any(|note| note.contains("Catalogue evidence unavailable"))
        );
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            0,
            "no catalogue, receipt folder or file was created"
        );
    }
}
