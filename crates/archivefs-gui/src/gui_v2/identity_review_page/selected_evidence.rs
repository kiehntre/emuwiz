//! Read-only presentation of the saved file inspection. This never decides
//! identity, reads content, or renews a DAT audit's freshness.
use crate::gui_v2::identity_review::provenance_text;
use archivefs_core::game_identity::{GameIdentityReport, IdentityConfidence};
use eframe::egui;
use std::path::Path;

/// Read-only extras shown when nothing is saved: the evidence the existing
/// bounded live inspection already computed, and the blocker-specific reason
/// the system has no identity check. Neither is persisted or re-read here.
#[derive(Default)]
pub(super) struct Extras {
    pub live_lines: Vec<String>,
    pub blocker_note: Option<String>,
}

/// One bounded line per fact the live inspection found or could not read.
/// Pure presentation over the already-computed report; sanitises every
/// untrusted string and states plainly that nothing is saved from it.
pub(super) fn live_evidence_lines(
    identity: &GameIdentityReport,
    structural_facts: &[archivefs_core::content_evidence::ContentEvidence],
) -> Vec<String> {
    const MAX_LINES: usize = 12;
    let mut lines = Vec::new();
    for fact in &identity.evidence {
        let mut line = format!(
            "{}: {} ({})",
            fact.kind,
            saved_text(fact.value.as_deref().unwrap_or("could not be read")),
            fact.status
        );
        if !fact.diagnostic.is_empty() {
            line.push_str(&format!(" - {}", saved_text(&fact.diagnostic)));
        }
        lines.push(line);
    }
    for fact in structural_facts {
        lines.push(format!(
            "{}: {} - {}",
            fact.kind.label(),
            saved_text(&fact.value),
            saved_text(&fact.detail)
        ));
    }
    for warning in &identity.warnings {
        lines.push(format!("Could not read: {}", saved_text(warning)));
    }
    let total = lines.len();
    lines.truncate(MAX_LINES);
    if total > MAX_LINES {
        lines.push(format!("… and {} more not shown here.", total - MAX_LINES));
    }
    if lines.is_empty() {
        lines.push("EmuWiz read this file and found no identity facts it can use.".into());
    }
    lines
}

#[cfg(test)]
pub(super) fn show(ui: &mut egui::Ui, path: &Path, report: Option<&GameIdentityReport>) {
    show_with(ui, path, report, &Extras::default());
}

pub(super) fn show_with(
    ui: &mut egui::Ui,
    path: &Path,
    report: Option<&GameIdentityReport>,
    extras: &Extras,
) {
    ui.label("Saved file inspection. Recorded facts explain identification; they do not establish current DAT verification or whole-file integrity. This view does not recheck file freshness.");
    if let Some(note) = &extras.blocker_note {
        ui.strong(note);
    }
    let Some(report) = report.filter(|report| report.archive_path == path) else {
        if extras.live_lines.is_empty() && extras.blocker_note.is_none() {
            ui.label("No saved identity evidence is available for this file. A filename or an unavailable report cannot establish a verified identity.");
            return;
        }
        ui.label("No identity evidence is saved for this file.");
        if !extras.live_lines.is_empty() {
            ui.label("What follows was read just now, read-only, and is not saved.");
            ui.strong("What EmuWiz found in the file");
            for line in &extras.live_lines {
                ui.label(line);
            }
        }
        return;
    };
    if !report.complete {
        ui.label("The inspection was incomplete. Missing evidence remains unknown.");
    }
    if report.evidence.is_empty() {
        ui.label("The inspection recorded no identity facts.");
    }
    for index in page(ui, path, "Identity facts", report.evidence.len()) {
        let fact = &report.evidence[index];
        ui.push_id(index, |ui| {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong(format!(
                    "{}: {}",
                    fact.kind,
                    saved_text(fact.value.as_deref().unwrap_or("not recorded"))
                ));
                ui.label(format!("Recorded status: {}", fact.status));
                ui.label(confidence(fact.confidence));
                ui.label(format!(
                    "Evidence source: {}",
                    saved_text(&fact.provenance.method)
                ));
                if fact.provenance.archive_path != report.archive_path {
                    ui.label(format!(
                        "Recorded source: {} — attribution is inconsistent with the selected file.",
                        saved_text(&fact.provenance.archive_path.to_string_lossy())
                    ));
                }
                if let Some(member) = &fact.provenance.member_path {
                    ui.label(format!(
                        "Archive member: {}",
                        saved_text(&String::from_utf8_lossy(member))
                    ));
                }
                if let Some(index) = fact.provenance.member_index {
                    ui.label(format!("Archive member index (zero-based): {index}"));
                }
                if !fact.diagnostic.is_empty() {
                    ui.label(format!("Why: {}", saved_text(&fact.diagnostic)));
                }
            });
        });
    }
    for index in page(ui, path, "Inspection warnings", report.warnings.len()) {
        ui.label(format!(
            "Inspection warning: {}",
            saved_text(&report.warnings[index])
        ));
    }
}

/// Saved diagnostics are untrusted and may be large. Bound layout work without
/// rewriting the underlying evidence, and keep truncation explicit.
fn saved_text(value: &str) -> String {
    let mut characters = value.chars();
    let shown: String = characters.by_ref().take(1024).collect();
    let mut text = provenance_text(&shown);
    if characters.next().is_some() {
        text.push_str("… (saved value truncated for display)");
    }
    text
}

/// Bound each frame while keeping every saved fact and warning accessible.
fn page(ui: &mut egui::Ui, path: &Path, label: &str, count: usize) -> std::ops::Range<usize> {
    const PAGE_SIZE: usize = 8;
    if count <= PAGE_SIZE {
        return 0..count;
    }
    let id = ui.make_persistent_id(("saved_identity_evidence", path, label));
    let last = (count - 1) / PAGE_SIZE;
    let mut page = ui
        .ctx()
        .data_mut(|data| data.get_temp::<usize>(id).unwrap_or(0))
        .min(last);
    ui.push_id(id, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{label}: page {} of {} ({count} recorded)",
                page + 1,
                last + 1
            ));
            if ui
                .add_enabled(page > 0, egui::Button::new("Previous"))
                .clicked()
            {
                page -= 1;
            }
            if ui
                .add_enabled(page < last, egui::Button::new("Next"))
                .clicked()
            {
                page += 1;
            }
        });
    });
    ui.ctx().data_mut(|data| data.insert_temp(id, page));
    page * PAGE_SIZE..((page + 1) * PAGE_SIZE).min(count)
}

fn confidence(confidence: IdentityConfidence) -> &'static str {
    match confidence {
        IdentityConfidence::ExactBytes => {
            "Confidence: exact bytes — the recorded content evidence; a DAT match is reported separately."
        }
        IdentityConfidence::StructuredMetadata => {
            "Confidence: structured metadata — a header or internal field; not a whole-file catalogue match."
        }
        IdentityConfidence::CatalogueContext => {
            "Confidence: catalogue context — contextual evidence, not an exact content match by itself."
        }
        IdentityConfidence::FilenameOnly => {
            "Confidence: filename clue only — cannot verify identity."
        }
        IdentityConfidence::Unavailable => "Confidence: unavailable — cannot verify identity.",
    }
}

#[cfg(test)]
mod tests;
