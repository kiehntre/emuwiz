//! The one final confirmation before redundant copies move into EmuWiz's
//! recoverable quarantine (board `D10-DU-Preview`).
//!
//! It names exactly what stays (KEPT), what moves (QUARANTINED) and where,
//! says that nothing is deleted and that everything is re-checked first, and
//! makes no Undo promise: Undo is offered afterwards only when the record
//! proves it is safe then. Cancel is first and focused. Presentation only;
//! the caller runs the existing apply path on `Some(true)`.

use std::path::Path;

use eframe::egui::{self, RichText};

use crate::ui::{
    components::{ActionStyle, action_button},
    theme,
};

/// What one confirmation shows. Paths are the real ones from the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct QuarantinePlan {
    /// The copy that stays exactly where it is. `None` refuses to confirm.
    pub kept: Option<String>,
    pub moved: Vec<String>,
    /// Where the moved copies go, as precisely as it is known now.
    pub destination: String,
}

pub(super) const NOTHING_DELETED: &str = "Nothing is deleted. Everything is re-checked immediately before anything moves. If a file changed, nothing is moved.";
pub(super) const UNDO_NOT_PROMISED: &str = "Undo: offered afterwards only if EmuWiz's record proves it is safe at that time. It is not promised now.";
pub(super) const NO_KEEPER: &str =
    "Choose the copy to keep first; nothing can move until one is kept.";

impl QuarantinePlan {
    pub(super) fn title(&self) -> String {
        match self.moved.len() {
            1 => "Move 1 copy to quarantine?".into(),
            count => format!("Move {count} copies to quarantine?"),
        }
    }

    pub(super) fn confirm_label(&self) -> String {
        match self.moved.len() {
            1 => "Move 1 file to quarantine".into(),
            count => format!("Move {count} files to quarantine"),
        }
    }

    /// Only a plan that keeps a copy and moves something can be confirmed.
    pub(super) fn can_confirm(&self) -> bool {
        self.kept.is_some() && !self.moved.is_empty()
    }
}

/// The quarantine folder inside a trusted game folder.
pub(super) fn quarantine_folder(trusted_root: &Path) -> String {
    trusted_root
        .join(archivefs_core::repair::QUARANTINE_DIRECTORY_NAME)
        .display()
        .to_string()
}

/// Shows the confirmation. `Some(true)` moves, `Some(false)` cancels.
pub(super) fn show(context: &egui::Context, id: egui::Id, plan: &QuarantinePlan) -> Option<bool> {
    let mut answer = None;
    let modal = egui::Modal::new(id)
        .backdrop_color(egui::Color32::from_rgba_unmultiplied(5, 10, 22, 188))
        .show(context, |ui| {
            ui.set_max_width(640.0);
            ui.label(theme::section_title(plan.title()));
            ui.add_space(6.0);
            section(
                ui,
                "KEPT · stays exactly where it is",
                theme::blue::OK,
                "KEEP",
                plan.kept.iter(),
            );
            section(
                ui,
                "QUARANTINED · moved, not deleted",
                theme::blue::ACCENT,
                "MOVE",
                plan.moved.iter(),
            );
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0_f32, theme::border_color(ui)))
                .corner_radius(10)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Moved to:").strong());
                        ui.monospace(&plan.destination);
                    });
                    ui.label(NOTHING_DELETED);
                    ui.label(UNDO_NOT_PROMISED);
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let cancel = ui.button("Cancel");
                // Cancel first, once per opening.
                let opened = id.with("opened");
                if !ui
                    .ctx()
                    .data(|data| data.get_temp::<bool>(opened).unwrap_or(false))
                {
                    cancel.request_focus();
                    ui.ctx().data_mut(|data| data.insert_temp(opened, true));
                }
                if cancel.clicked() {
                    answer = Some(false);
                }
                if plan.can_confirm() {
                    if action_button(ui, plan.confirm_label(), ActionStyle::Primary, true).clicked()
                    {
                        answer = Some(true);
                    }
                } else {
                    crate::ui::components::disabled_action_with_reason(
                        ui,
                        plan.confirm_label(),
                        NO_KEEPER,
                    );
                }
            });
        });
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    if answer.is_some() {
        context.data_mut(|data| data.remove::<bool>(id.with("opened")));
    }
    answer
}

fn section<'a>(
    ui: &mut egui::Ui,
    heading: &str,
    color: egui::Color32,
    verb: &str,
    paths: impl Iterator<Item = &'a String>,
) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.7)))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(heading).strong().color(color));
            let mut any = false;
            for path in paths {
                any = true;
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(verb).strong().size(12.5).color(color));
                    ui.monospace(path);
                });
            }
            if !any {
                ui.label(RichText::new("None chosen").color(theme::muted(ui)));
            }
        });
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(kept: Option<&str>, moved: &[&str]) -> QuarantinePlan {
        QuarantinePlan {
            kept: kept.map(Into::into),
            moved: moved.iter().map(|path| path.to_string()).collect(),
            destination: "/games/.emuwiz-quarantine".into(),
        }
    }

    #[test]
    fn wording_counts_the_files_and_promises_no_undo() {
        let two = plan(Some("/games/a.chd"), &["/games/b.chd", "/games/c.chd"]);
        assert_eq!(two.title(), "Move 2 copies to quarantine?");
        assert_eq!(two.confirm_label(), "Move 2 files to quarantine");
        assert_eq!(
            plan(Some("/k"), &["/m"]).title(),
            "Move 1 copy to quarantine?"
        );
        assert!(UNDO_NOT_PROMISED.contains("not promised"));
        assert!(NOTHING_DELETED.contains("Nothing is deleted"));
    }

    #[test]
    fn a_plan_without_a_kept_copy_or_anything_to_move_cannot_be_confirmed() {
        assert!(plan(Some("/k"), &["/m"]).can_confirm());
        assert!(!plan(None, &["/m"]).can_confirm());
        assert!(!plan(Some("/k"), &[]).can_confirm());
    }

    /// Renders the dialog for a few frames; `click` presses a label's centre.
    fn render(plan: &QuarantinePlan, click: Option<&str>) -> (Vec<String>, Option<bool>, bool) {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        let id = egui::Id::new("test_quarantine_confirm");
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 900.0));
        let mut answer = None;
        let mut output = context.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |context| answer = show(context, id, plan),
        );
        for _ in 0..2 {
            output = context.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |context| answer = show(context, id, plan),
            );
        }
        let mut texts = Vec::new();
        let mut target = None;
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                texts.push(text.galley.text().to_owned());
                if Some(text.galley.text()) == click {
                    target = Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center());
                }
            }
        }
        let cancel_focused = context.memory(|memory| memory.focused()).is_some();
        if let Some(point) = target {
            for pressed in [true, false] {
                let _ = context.run(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events: vec![
                            egui::Event::PointerMoved(point),
                            egui::Event::PointerButton {
                                pos: point,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |context| {
                        if let Some(value) = show(context, id, plan) {
                            answer = Some(value);
                        }
                    },
                );
            }
        }
        (texts, answer, cancel_focused)
    }

    #[test]
    fn the_dialog_names_what_stays_what_moves_and_where_and_cancel_comes_first() {
        let plan = plan(
            Some("/games/ssd/Game.chd"),
            &["/games/hdd/Game.chd", "/games/old/Game copy.chd"],
        );
        let (texts, answer, focused) = render(&plan, None);
        for wanted in [
            "KEPT · stays exactly where it is",
            "QUARANTINED · moved, not deleted",
            "/games/ssd/Game.chd",
            "/games/hdd/Game.chd",
            "/games/old/Game copy.chd",
            "/games/.emuwiz-quarantine",
            NOTHING_DELETED,
            UNDO_NOT_PROMISED,
            "Move 2 files to quarantine",
        ] {
            assert!(texts.iter().any(|text| text == wanted), "{wanted} missing");
        }
        assert_eq!(answer, None, "nothing is decided before a click");
        assert!(focused, "Cancel holds the focus when the dialog opens");
        let (_, answer, _) = render(&plan, Some("Cancel"));
        assert_eq!(answer, Some(false));
        let (_, answer, _) = render(&plan, Some("Move 2 files to quarantine"));
        assert_eq!(answer, Some(true));
    }

    #[test]
    fn without_a_kept_copy_the_move_cannot_be_confirmed() {
        let plan = plan(None, &["/games/hdd/Game.chd"]);
        let (texts, _, _) = render(&plan, None);
        assert!(texts.iter().any(|text| text == NO_KEEPER));
        let (_, answer, _) = render(&plan, Some("Move 1 file to quarantine"));
        assert_ne!(answer, Some(true));
    }

    #[test]
    fn the_destination_is_the_core_quarantine_folder() {
        assert_eq!(
            quarantine_folder(Path::new("/games")),
            "/games/.emuwiz-quarantine"
        );
    }
}
