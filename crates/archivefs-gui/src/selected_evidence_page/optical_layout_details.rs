//! Read-only presentation of an already-computed CUE/BIN or GDI layout report.
//! No filesystem access and no readiness decisions; classification lives in
//! `archivefs_core::optical_layout_inspection`.

use archivefs_core::optical_layout_inspection::{
    CompanionStatus, OpticalContainer, OpticalLayoutReport, OpticalLayoutState,
};
use eframe::egui;

use crate::ui::components::{self as widgets, StatusTone};

const VISIBLE_ROWS: usize = 12;

fn state(state: OpticalLayoutState) -> (&'static str, StatusTone) {
    match state {
        OpticalLayoutState::Valid => ("Structurally valid", StatusTone::Success),
        OpticalLayoutState::Missing => ("Companion missing", StatusTone::Blocked),
        OpticalLayoutState::Ambiguous => ("Ambiguous - needs a decision", StatusTone::Warning),
        OpticalLayoutState::Mismatch => {
            ("Sizes disagree (stale or truncated)", StatusTone::Blocked)
        }
        OpticalLayoutState::Unsafe => ("Unsafe reference", StatusTone::Blocked),
        OpticalLayoutState::Unsupported => ("Not supported by this check", StatusTone::Pending),
        OpticalLayoutState::Unreadable => ("Could not be read", StatusTone::Blocked),
    }
}

fn companion_line(name: &str, status: &CompanionStatus) -> String {
    match status {
        CompanionStatus::Present { bytes } => format!("{name}: present, {bytes} bytes"),
        CompanionStatus::Missing => format!("{name}: missing"),
        CompanionStatus::Ambiguous { candidates } => format!(
            "{name}: missing; case-different files exist ({}) and none was chosen",
            candidates.join(", ")
        ),
        CompanionStatus::Unsafe => format!("{name}: unsafe reference, not opened"),
    }
}

fn track_line(track: &archivefs_core::optical_layout_inspection::TrackRow) -> String {
    let length = track.frames.map_or_else(
        || "length not derivable".to_owned(),
        |f| format!("{f} frames"),
    );
    format!(
        "Track {:02} · {} · {} · {} · {} · gap: {}",
        track.number, track.kind, track.companion, track.start, length, track.gap
    )
}

/// Plain-text projection of everything shown, for tests and accessibility.
pub(super) fn lines(report: &OpticalLayoutReport) -> Vec<String> {
    let mut out = vec![state(report.state).0.to_owned(), report.summary.clone()];
    out.extend(report.tracks.iter().map(track_line));
    out.extend(
        report
            .companions
            .iter()
            .map(|c| companion_line(&c.name, &c.status)),
    );
    out.extend(report.notes.iter().cloned());
    out.push(format!("Checked: {}", report.basis));
    out.extend(report.limits.iter().map(|l| (*l).to_owned()));
    out.push(next_step(report).to_owned());
    out
}

fn next_step(report: &OpticalLayoutReport) -> &'static str {
    match report.state {
        OpticalLayoutState::Valid => {
            "Next: Converter reviews this disc before any conversion; Launch shows whether an emulator can start it."
        }
        _ => "Next: fix the files outside EmuWiz, then re-check before using Converter or Launch.",
    }
}

pub(super) fn show(ui: &mut egui::Ui, report: &OpticalLayoutReport) {
    let title = match report.container {
        OpticalContainer::CueBin => "Disc layout (CUE/BIN)",
        OpticalContainer::Gdi => "Disc layout (GDI)",
    };
    ui.label(title);
    let (label, tone) = state(report.state);
    widgets::status_rows(ui, &[("Layout", label, tone)]);
    ui.label(&report.summary);
    for track in report.tracks.iter().take(VISIBLE_ROWS) {
        ui.label(track_line(track));
    }
    if report.tracks.len() > VISIBLE_ROWS {
        ui.weak(format!(
            "{} more tracks in Technical details.",
            report.tracks.len() - VISIBLE_ROWS
        ));
    }
    for note in &report.notes {
        ui.colored_label(ui.visuals().warn_fg_color, note);
    }
    ui.weak(format!("Checked: {}", report.basis));
    for limit in &report.limits {
        ui.weak(*limit);
    }
    ui.weak(next_step(report));
    widgets::technical_details(ui, "structural-optical-layout-details", |ui| {
        for companion in &report.companions {
            ui.label(companion_line(&companion.name, &companion.status));
        }
        for track in report.tracks.iter().skip(VISIBLE_ROWS) {
            ui.label(track_line(track));
        }
    });
}

#[cfg(test)]
mod tests;
