//! Native Blue "Library View History" (boards `D11-LG-LibraryViewHistory`,
//! `-LV*`).
//!
//! Read-only. It renders the legacy page's own state
//! (`LibraryViewHistoryPageState`), which reads the history folder on disk
//! through `list_library_view_history_at` every time it loads or refreshes.
//! An unreadable record is reported and left exactly as it is; the other
//! records are still shown. Nothing here can undo, repair or re-apply a
//! Library View.

use archivefs_core::{
    LibraryViewHistoryEntry, LibraryViewHistoryOperation, LibraryViewHistoryRecord,
};
use eframe::egui::{self, RichText};

use super::simple_shell::palette;
use crate::library_view_history_page::{
    LibraryViewHistoryPageState, operation_label, profile_kind_label,
};
use crate::ui::components::{self as widgets, ActionStyle, StatusTone};
use crate::ui::theme::{self, blue};

pub(super) const READ_ONLY_NOTE: &str =
    "Nothing here can undo, repair or re-apply a Library View. Use Views to do that.";

/// What the page asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Request {
    /// Library Views has no Blue page yet: open the specialist interface.
    OpenViews,
    OpenActivity,
}

#[derive(Default)]
pub(super) struct ViewHistoryState {
    pub page: Option<LibraryViewHistoryPageState>,
    /// The record whose technical details are shown (its file path).
    pub open: Option<std::path::PathBuf>,
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut ViewHistoryState) -> Option<Request> {
    let page = state
        .page
        .get_or_insert_with(LibraryViewHistoryPageState::load);
    let mut request = None;
    let wide = ui.available_width() >= 1100.0;
    let open = &mut state.open;
    let mut body = |ui: &mut egui::Ui| {
        let unreadable = page
            .entries
            .iter()
            .filter(|entry| matches!(entry, LibraryViewHistoryEntry::Malformed { .. }))
            .count();
        if unreadable > 0 {
            widgets::banner(
                ui,
                if unreadable == 1 {
                    "A history record could not be read"
                } else {
                    "Some history records could not be read"
                },
                "The other records are shown. The unreadable file is left exactly as it is.",
                StatusTone::Warning,
            );
            ui.add_space(6.0);
        }
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(ui, "Refresh", ActionStyle::Secondary, true).clicked() {
                page.refresh();
            }
            let records = page.entries.len() - unreadable;
            ui.label(
                RichText::new(format!(
                    "{records} record{} · newest first",
                    if records == 1 { "" } else { "s" }
                ))
                .color(palette::SECONDARY_TEXT),
            );
        });
        ui.add_space(6.0);
        if page.entries.is_empty() {
            widgets::empty_state(
                ui,
                "No Library View operations recorded yet",
                "Applying or removing a Library View adds a record here.",
                None,
            );
            return;
        }
        for entry in &page.entries {
            match entry {
                LibraryViewHistoryEntry::Record { path, record } => {
                    record_row(ui, path, record, open);
                }
                LibraryViewHistoryEntry::Malformed { path, error } => {
                    ui.push_id(("v2_view_history_bad", path), |ui| {
                        widgets::banner(
                            ui,
                            "Unreadable record",
                            &format!("{}: {error}", path.display()),
                            StatusTone::Warning,
                        );
                    });
                    ui.add_space(6.0);
                }
            }
        }
    };
    let mut side = |ui: &mut egui::Ui| {
        super::game_detail_blue::side_card(ui, "READ-ONLY", |ui| {
            ui.add(egui::Label::new(RichText::new(READ_ONLY_NOTE).color(blue::BODY_TEXT)).wrap());
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Read fresh from the history folder on disk each time, not from the in-session activity log, which does not survive a restart.",
                    )
                    .color(palette::SECONDARY_TEXT),
                )
                .wrap(),
            );
            ui.add_space(6.0);
            if widgets::action_button(ui, "Open Views", ActionStyle::Secondary, true)
                .on_hover_text(
                    "Library Views opens in the specialist interface, in a separate window.",
                )
                .clicked()
            {
                request = Some(Request::OpenViews);
            }
            if widgets::action_button(ui, "Open Activity", ActionStyle::Secondary, true).clicked() {
                request = Some(Request::OpenActivity);
            }
        });
    };
    if wide {
        let gap = 20.0;
        let right = (ui.available_width() * 0.3).clamp(340.0, 480.0);
        let left = ui.available_width() - right - gap;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                egui::vec2(left, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left);
                    body(ui);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(right, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right);
                    side(ui);
                },
            );
        });
    } else {
        body(ui);
        ui.add_space(theme::SPACE_SM);
        side(ui);
    }
    request
}

pub(super) fn counts_line(record: &LibraryViewHistoryRecord) -> String {
    let mut line = format!(
        "Created {} · Repaired {} · Removed {} · Unchanged {} · Failed {}",
        record.created, record.repaired, record.removed, record.unchanged, record.failed
    );
    match record.skipped_or_collision {
        Some(count) => line.push_str(&format!(" · Skipped/collision {count}")),
        // Remove has no plan to count skips from.
        None if record.operation == LibraryViewHistoryOperation::Remove => {
            line.push_str(" · Skipped 0")
        }
        None => {}
    }
    line
}

fn record_row(
    ui: &mut egui::Ui,
    path: &std::path::Path,
    record: &LibraryViewHistoryRecord,
    open: &mut Option<std::path::PathBuf>,
) {
    let expanded = open.as_deref() == Some(path);
    ui.push_id(("v2_view_history", path), |ui| {
        egui::Frame::new()
            .fill(blue::CARD_SURFACE_TOP)
            .stroke(egui::Stroke::new(
                1.0_f32,
                if expanded {
                    blue::SELECTED_OUTLINE
                } else {
                    palette::BORDER_SUBTLE
                },
            ))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(ui, operation_label(record.operation), StatusTone::Info);
                    widgets::status_badge(
                        ui,
                        if record.success {
                            "Succeeded"
                        } else {
                            "Failed"
                        },
                        if record.success {
                            StatusTone::Success
                        } else {
                            StatusTone::Warning
                        },
                    );
                    ui.label(
                        RichText::new(&record.view_name)
                            .strong()
                            .color(blue::PRIMARY_TEXT),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} · {}",
                            profile_kind_label(record.profile_kind),
                            record.timestamp
                        ))
                        .size(12.5)
                        .color(palette::SECONDARY_TEXT),
                    );
                });
                ui.add(
                    egui::Label::new(RichText::new(counts_line(record)).color(blue::BODY_TEXT))
                        .wrap(),
                );
                ui.horizontal_wrapped(|ui| {
                    if !record.warnings.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "{} warning{}",
                                record.warnings.len(),
                                if record.warnings.len() == 1 { "" } else { "s" }
                            ))
                            .color(blue::WARN),
                        );
                    }
                    if ui
                        .link(if expanded {
                            "Hide technical details"
                        } else {
                            "Technical details"
                        })
                        .clicked()
                    {
                        *open = (!expanded).then(|| path.to_path_buf());
                    }
                });
                if !expanded {
                    return;
                }
                ui.separator();
                for (label, value) in [
                    ("Destination", record.destination_root.clone()),
                    ("Manifest path", record.manifest_path.clone()),
                    ("View id", record.view_id.clone()),
                    ("Planned count", record.planned_count.to_string()),
                    ("Schema version", record.schema_version.to_string()),
                ] {
                    detail_row(ui, label, &value, true);
                }
                for warning in &record.warnings {
                    detail_row(ui, "Warning", warning, false);
                }
            });
    });
    ui.add_space(6.0);
}

fn detail_row(ui: &mut egui::Ui, label: &str, value: &str, mono: bool) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(150.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(150.0);
                ui.label(RichText::new(label).color(blue::DIM_TEXT));
            },
        );
        let text = RichText::new(value).color(blue::BODY_TEXT);
        ui.add(
            egui::Label::new(if mono {
                text.monospace().size(12.5)
            } else {
                text
            })
            .wrap(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::FrontendProfileKind;

    fn record(name: &str, failed: usize) -> LibraryViewHistoryRecord {
        LibraryViewHistoryRecord {
            schema_version: 1,
            timestamp: "2026-10-16T09:00:00Z".into(),
            operation: LibraryViewHistoryOperation::Apply,
            view_id: "view-1".into(),
            view_name: name.into(),
            profile_kind: FrontendProfileKind::EsDe,
            destination_root: "/links/es-de".into(),
            manifest_path: "/data/view-1.manifest.json".into(),
            planned_count: 12,
            created: 10,
            repaired: 0,
            removed: 0,
            unchanged: 0,
            failed,
            skipped_or_collision: Some(2),
            success: failed == 0,
            warnings: (0..failed)
                .map(|index| format!("warning {index}"))
                .collect(),
        }
    }

    fn snapshot(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn records_and_an_unreadable_file_render_without_changing_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("2026-10-16-a.json"),
            serde_json::to_vec(&record("ES-DE links", 1)).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("2026-10-16-b.json"),
            serde_json::to_vec(&record("RomM links", 0)).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("2026-10-16-c.json"), b"{ not json").unwrap();
        let before = snapshot(dir.path());

        let mut state = ViewHistoryState {
            page: Some(LibraryViewHistoryPageState::load_with_history_dir(
                dir.path().to_path_buf(),
            )),
            open: Some(dir.path().join("2026-10-16-a.json")),
        };
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        for width in [600.0, 1500.0] {
            let mut request = None;
            let mut used = 0.0_f32;
            let mut output = None;
            for _ in 0..2 {
                output = Some(context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1200.0),
                        )),
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                request = show(ui, &mut state);
                                used = ui.min_rect().right();
                            });
                        });
                    },
                ));
            }
            assert_eq!(request, None);
            assert!(used <= width, "at {width} used {used}");
            let mut texts = Vec::new();
            fn gather(shape: &egui::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| gather(shape, out)),
                    _ => {}
                }
            }
            for shape in &output.unwrap().shapes {
                gather(&shape.shape, &mut texts);
            }
            for wanted in [
                "A history record could not be read",
                "2 records · newest first",
                "ES-DE links",
                "RomM links",
                "Succeeded",
                "Failed",
                "Manifest path",
                "READ-ONLY",
            ] {
                assert!(
                    texts.iter().any(|text| text.contains(wanted)),
                    "{wanted} missing at {width}: {texts:?}"
                );
            }
        }
        assert_eq!(
            snapshot(dir.path()),
            before,
            "the history folder is unchanged"
        );
    }

    #[test]
    fn counts_name_skips_only_where_they_exist() {
        let apply = record("A", 0);
        assert!(counts_line(&apply).ends_with("Skipped/collision 2"));
        let mut remove = record("R", 0);
        remove.operation = LibraryViewHistoryOperation::Remove;
        remove.skipped_or_collision = None;
        assert!(counts_line(&remove).ends_with("Skipped 0"));
    }
}
