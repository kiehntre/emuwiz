//! Native, keyboard-accessible controls; the approved artwork is decoration only.
use super::*;

// Only Mr Wiz: exclude the baked-in buttons and the fictitious READY monitor.
const WIZ_UV: egui::Rect =
    egui::Rect::from_min_max(egui::pos2(0.574, 0.01), egui::pos2(0.795, 0.679));

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut OpticalConversionPageState,
    texture: egui::TextureId,
) {
    let motif_size = if widgets::blue_compact(ui) {
        egui::vec2(56.0, 73.0)
    } else {
        egui::vec2(80.0, 104.0)
    };
    widgets::page_hero_with_motif_size(
        ui,
        motif_size,
        |ui, size| {
            ui.add(egui::Image::new((texture, size)).uv(WIZ_UV));
        },
        "Disc Conversion",
        "Create a verified CHD copy of a supported disc image. Review before converting.",
        None,
        Some("Originals kept unless you explicitly choose quarantine."),
        |_| {},
        |ui| {
            controls(ui, state);
        },
    );
}

fn controls(ui: &mut egui::Ui, state: &mut OpticalConversionPageState) -> [egui::Response; 3] {
    let pending = state.recovery_pending();
    let can_scan = !pending && !state.source_root_draft.trim().is_empty();
    let selected_plan = state
        .selected
        .and_then(|index| state.candidates.get(index))
        .is_some_and(|candidate| candidate.plan.is_some());
    let selected_source = state.selected_context.as_ref().is_some_and(|context| {
        context
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(|ext| supported_capability_for_extension(Some(ext)))
            .is_some()
    });
    let can_preview = !pending && (selected_plan || selected_source);
    let responses = ui
        .horizontal_wrapped(|ui| {
            let choose = widgets::action_button(
                ui,
                "Choose source folder",
                widgets::ActionStyle::Primary,
                !pending,
            );
            let scan = widgets::action_button(
                ui,
                "Find supported files",
                widgets::ActionStyle::Secondary,
                can_scan,
            );
            let preview = widgets::action_button(
                ui,
                "Prepare preview",
                widgets::ActionStyle::Secondary,
                can_preview,
            );
            [choose, scan, preview]
        })
        .inner;
    if pending {
        ui.label("Recovery required: use Undo conversion or Repair History before choosing another source.");
    } else if !can_scan && !can_preview {
        ui.label("Choose a source folder to find files, then select a supported disc set to prepare preview.");
    } else if !can_scan {
        ui.label("Choose a source folder to enable Find supported files.");
    } else if !can_preview {
        ui.label("Prepare preview is unavailable until a supported disc set is selected below.");
    }
    if responses[0].clicked() {
        choose_source_folder(state);
        state.hero_hint = Some(hero_scan_hint(state, "No folder was chosen."));
    }
    if responses[1].clicked() {
        state.scan();
        state.hero_hint = Some(hero_scan_hint(state, "Choose a source folder first."));
    }
    if responses[2].clicked() {
        if selected_plan {
            state.previewed = true;
        } else {
            state.preview_selected();
        }
        state.hero_preview_scroll = true;
        state.hero_hint = Some(if state.previewed {
            "Preview prepared below. Nothing has been converted.".into()
        } else {
            "Preview unavailable. Review the source or conversion-tool explanation below.".into()
        });
    }
    responses
}

#[cfg(test)]
mod tests;
