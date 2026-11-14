//! "Move a game between library folders": the Blue view of the Ouroboros
//! relocation engine (`archivefs_core::transform`).
//!
//! What it does in this build: a read-only preview of one relocation
//! (strategy, dependencies, effects, blockers, plan digest) and a read-only
//! list of transformation journals with their true state, recovery
//! condition, retained originals and Undo availability.
//!
//! What it never does in this build: apply, recover or undo. Those buttons
//! are shown disabled with the reason, because production relocation awaits
//! an independent safety review and owner approval. Only a verified Complete
//! transaction is ever shown in the success colour.

use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc;

use archivefs_core::transform::{self, Plan, PlanKind, Preview, TxnState};

pub(super) const DISABLED_REASON: &str = "Not available in this build: moving games runs only inside \
EmuWiz's test suite, on scratch folders it creates itself. Moving real games needs its own \
authorisation, an independent safety review and the owner's approval.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    /// Verified Complete only.
    Success,
    /// Rolled back cleanly: nothing changed.
    Neutral,
    /// Needs a person: retained residue, interrupted, blocked.
    Warning,
    /// The journal cannot be trusted.
    Danger,
}

/// The single place that decides how a state looks. Only `Complete` may be
/// green; every unfinished or uncertain state is a warning or worse.
pub(super) fn presentation(state: &TxnState) -> (&'static str, Tone) {
    let tone = match state {
        TxnState::Complete => Tone::Success,
        TxnState::Compensated { residue } if residue.is_empty() => Tone::Neutral,
        TxnState::Compensated { .. } => Tone::Warning,
        TxnState::Blocked { .. } | TxnState::Interrupted { .. } | TxnState::Running => {
            Tone::Warning
        }
        TxnState::Corrupt { .. } => Tone::Danger,
    };
    (state.label(), tone)
}

fn colour(ui: &egui::Ui, tone: Tone) -> egui::Color32 {
    let v = ui.visuals();
    match tone {
        Tone::Success => egui::Color32::from_rgb(0x2e, 0x9e, 0x5b),
        Tone::Neutral => v.text_color(),
        Tone::Warning => v.warn_fg_color,
        Tone::Danger => v.error_fg_color,
    }
}

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub id: String,
    pub state: TxnState,
    pub plan: Option<Plan>,
    pub undo: Option<String>,
    pub undone_by: Option<String>,
}

enum Message {
    Preview(Box<Result<Preview, String>>),
    Rows(Result<Vec<Row>, String>),
}

#[derive(Default)]
pub(super) struct State {
    source: String,
    destination: String,
    preview: Option<Result<Preview, String>>,
    rows: Option<Result<Vec<Row>, String>>,
    busy: bool,
    rx: Option<mpsc::Receiver<Message>>,
}

impl State {
    fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(m) = rx.try_recv() {
            match m {
                Message::Preview(p) => self.preview = Some(*p),
                Message::Rows(r) => self.rows = Some(r),
            }
            self.busy = false;
        }
    }

    fn spawn(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Message + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.busy = true;
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }
}

pub(super) fn load_rows() -> Result<Vec<Row>, String> {
    let root = transform::default_journal_root().map_err(|e| e.to_string())?;
    let ctx = transform::Context::new(root);
    Ok(transform::list(&ctx)
        .into_iter()
        .map(|(id, state, plan)| {
            let relocation_complete = state == TxnState::Complete
                && plan
                    .as_ref()
                    .is_some_and(|p| matches!(p.kind, PlanKind::Relocate));
            let undone_by = relocation_complete
                .then(|| transform::undone_by(&ctx, &id))
                .flatten();
            let undo = (relocation_complete && undone_by.is_none()).then(|| {
                transform::undo_blocker(&ctx, &id).unwrap_or_else(|| "Undo is possible".into())
            });
            Row {
                id,
                state,
                plan,
                undo,
                undone_by,
            }
        })
        .collect())
}

fn run_preview(source: PathBuf, destination: PathBuf) -> Result<Preview, String> {
    let path = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let db = archivefs_core::Database::open_read_only(&path).map_err(|e| e.to_string())?;
    Ok(transform::plan_relocation(&db, &source, &destination))
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    if state.busy {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    if state.rows.is_none() && !state.busy {
        state.spawn(ui.ctx(), || Message::Rows(load_rows()));
    }
    ui.label(
        "Preview moving one loose game file to another library folder, on the same drive or a \
         different one. A move to another drive copies, verifies, then keeps the original in a \
         private retention folder; it is never deleted.",
    );
    ui.small(DISABLED_REASON);
    ui.add_space(6.0);
    egui::Grid::new("relocation_inputs")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Game file");
            ui.text_edit_singleline(&mut state.source);
            ui.end_row();
            ui.label("Move to folder");
            ui.text_edit_singleline(&mut state.destination);
            ui.end_row();
        });
    ui.horizontal(|ui| {
        let ready =
            !state.busy && !state.source.trim().is_empty() && !state.destination.trim().is_empty();
        if ui
            .add_enabled(ready, egui::Button::new("Preview (changes nothing)"))
            .clicked()
        {
            let (s, d) = (
                PathBuf::from(state.source.trim()),
                PathBuf::from(state.destination.trim()),
            );
            state.spawn(ui.ctx(), move || {
                Message::Preview(Box::new(run_preview(s, d)))
            });
        }
        ui.add_enabled(false, egui::Button::new("Move"))
            .on_disabled_hover_text(DISABLED_REASON);
        if state.busy {
            ui.spinner();
        }
    });
    match &state.preview {
        None => {}
        Some(Err(e)) => {
            ui.colored_label(ui.visuals().error_fg_color, format!("Preview failed: {e}"));
        }
        Some(Ok(p)) => preview_card(ui, p),
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.heading("Transformations");
        if ui
            .add_enabled(!state.busy, egui::Button::new("Refresh"))
            .clicked()
        {
            state.spawn(ui.ctx(), || Message::Rows(load_rows()));
        }
    });
    match &state.rows {
        None => {
            ui.label("Reading journals…");
        }
        Some(Err(e)) => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Journals unavailable: {e}"),
            );
        }
        Some(Ok(rows)) if rows.is_empty() => {
            ui.label("No transformations have been recorded.");
        }
        Some(Ok(rows)) => {
            for row in rows {
                row_card(ui, row);
            }
        }
    }
}

fn preview_card(ui: &mut egui::Ui, p: &Preview) {
    ui.group(|ui| {
        ui.strong(format!("{}  →  {}", p.source.display(), p.destination.display()));
        if let Some(s) = p.strategy {
            ui.label(match s {
                transform::Strategy::AtomicRename => "Same drive: one rename that never overwrites.",
                transform::Strategy::CopyVerifyRetire => {
                    "Different drives: copy to a private folder, verify every byte, publish without \
                     overwriting, update the library, then keep the original."
                }
            });
        }
        if let Some(d) = &p.dependencies {
            ui.label(format!(
                "Dependencies: {} established, {} unknown (unknown never counts as none)",
                d.edges, d.unknowns
            ));
        }
        for e in &p.effects {
            ui.small(format!("• {e}"));
        }
        if p.blockers.is_empty() {
            ui.label(format!("Ready. Plan digest {}", p.digest.as_deref().unwrap_or("-")));
        } else {
            for b in &p.blockers {
                ui.colored_label(ui.visuals().warn_fg_color, format!("Blocked: {b}"));
            }
        }
    });
}

fn row_card(ui: &mut egui::Ui, row: &Row) {
    let (label, tone) = presentation(&row.state);
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.colored_label(colour(ui, tone), label);
            ui.small(&row.id);
        });
        if let Some(p) = &row.plan {
            let kind = if matches!(p.kind, PlanKind::Undo { .. }) {
                "Undo of a move"
            } else {
                "Move"
            };
            ui.label(format!(
                "{kind}: {} → {}",
                p.source_path.display(),
                p.destination_path.display()
            ));
            if let Some(r) = &p.retired_path
                && matches!(p.kind, PlanKind::Relocate)
                && row.state == TxnState::Complete
                && row.undone_by.is_none()
            {
                ui.small(format!("Original kept at {}", r.display()));
            }
        }
        match &row.state {
            TxnState::Blocked {
                reason, safe_facts, ..
            } => {
                ui.small(format!("Why: {reason}"));
                for f in safe_facts {
                    ui.small(format!("Known: {f}"));
                }
                ui.add_enabled(false, egui::Button::new("Recover"))
                    .on_disabled_hover_text(DISABLED_REASON);
            }
            TxnState::Interrupted { .. } => {
                ui.small(
                    "Interrupted. Recovery will resume or reverse it; nothing is assumed finished.",
                );
                ui.add_enabled(false, egui::Button::new("Recover"))
                    .on_disabled_hover_text(DISABLED_REASON);
            }
            TxnState::Compensated { residue } => {
                for r in residue {
                    ui.small(format!("Kept for review (not deleted): {}", r.display()));
                }
            }
            TxnState::Corrupt { reason } => {
                ui.small(format!(
                    "The journal cannot be trusted ({reason}). Nothing will act on it."
                ));
            }
            _ => {}
        }
        if let Some(by) = &row.undone_by {
            ui.small(format!("Undone by {by}"));
        } else if let Some(undo) = &row.undo {
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Button::new("Undo"))
                    .on_disabled_hover_text(DISABLED_REASON);
                ui.small(undo);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_is_ever_green() {
        let states = [
            TxnState::Complete,
            TxnState::Compensated { residue: vec![] },
            TxnState::Compensated {
                residue: vec!["/x".into()],
            },
            TxnState::Blocked {
                step: 3,
                reason: "r".into(),
                safe_facts: vec![],
            },
            TxnState::Running,
            TxnState::Interrupted { last: None },
            TxnState::Corrupt { reason: "c".into() },
        ];
        for s in &states {
            let (_, tone) = presentation(s);
            assert_eq!(tone == Tone::Success, *s == TxnState::Complete, "{s:?}");
        }
        assert_eq!(
            presentation(&states[2]).1,
            Tone::Warning,
            "retained residue needs review"
        );
        assert_eq!(presentation(&states[6]).1, Tone::Danger);
    }

    #[test]
    fn risky_actions_explain_why_they_are_disabled() {
        assert!(DISABLED_REASON.contains("independent safety review"));
        assert!(DISABLED_REASON.contains("approval"));
        assert!(
            !DISABLED_REASON.contains("fixture"),
            "no marker or switch is suggested"
        );
    }
}
