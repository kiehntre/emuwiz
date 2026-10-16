//! Read-only Dreamcast boot and local DCP review in the existing GUI.
use super::library::Game;
use archivefs_core::dreamcast_boot_evidence::{
    add_stronger_evidence_conflicts,
    media::{DreamcastMediaInspection, inspect_image, review_patch},
};
use archivefs_core::dreamcast_patch_readiness::DreamcastPatchReadiness;
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(super) fn applies(game: &Game) -> bool {
    game.platform.eq_ignore_ascii_case("Dreamcast")
}

#[derive(Default)]
struct Panel {
    busy: bool,
    image: Option<DreamcastMediaInspection>,
    patch: Option<DreamcastPatchReadiness>,
    patch_path: String,
    error: Option<String>,
}
type SharedPanel = Arc<Mutex<Panel>>;

fn start(shared: SharedPanel, ctx: egui::Context, source: PathBuf, patch: Option<PathBuf>) {
    let expected = {
        let mut state = shared.lock().unwrap();
        if state.busy {
            return;
        }
        state.busy = true;
        state.error = None;
        state.patch = None;
        if patch.is_none() {
            state.image = None;
        }
        state.image.clone()
    };
    std::thread::spawn(move || {
        let result = match patch {
            Some(path) => expected
                .as_ref()
                .ok_or_else(|| "Inspect the image first".to_string())
                .and_then(|image| {
                    review_patch(image, &path)
                        .map(|patch| (None, Some(patch)))
                        .map_err(|e| e.to_string())
                }),
            None => inspect_image(&source)
                .map(|image| (Some(image), None))
                .map_err(|e| e.to_string()),
        };
        let mut state = shared.lock().unwrap();
        match result {
            Ok((image, patch)) => {
                if let Some(image) = image {
                    state.image = Some(image);
                }
                state.patch = patch;
            }
            Err(error) => {
                state.image = None;
                state.error = Some(error);
            }
        }
        state.busy = false;
        ctx.request_repaint();
    });
}

pub(super) fn show(ui: &mut egui::Ui, game: &Game) {
    if !applies(game) {
        return;
    }
    let id = egui::Id::new(("dreamcast_read_only", &game.archive.absolute_path));
    let shared = ui.ctx().data_mut(|data| {
        data.get_temp::<SharedPanel>(id).unwrap_or_else(|| {
            let state = SharedPanel::default();
            data.insert_temp(id, state.clone());
            state
        })
    });
    let inspect;
    let mut review = None;
    {
        let mut state = shared.lock().unwrap();
        ui.heading("Dreamcast boot metadata & DCP review");
        ui.label("Read-only inspection. Original images and launch settings stay untouched.");
        ui.label("GDI (proven zero-offset layouts) and cooked 2048-byte ISO. CDI, CHD, CUE and loose BIN are unsupported here.");
        inspect = ui
            .add_enabled(!state.busy, egui::Button::new("Inspect / refresh IP.BIN"))
            .clicked();
        if state.busy {
            ui.spinner();
            ui.label("Inspecting local evidence…");
        }
        if let Some(error) = &state.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some(image) = &state.image {
            render_image(ui, image, game);
        }
        ui.separator();
        ui.label("Local DCP package path (nothing is downloaded)");
        let busy = state.busy;
        if ui
            .add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut state.patch_path).id(egui::Id::new((
                    "dc_patch_path",
                    &game.archive.absolute_path,
                ))),
            )
            .changed()
        {
            state.patch = None;
            state.error = None;
        }
        let can_review =
            !state.busy && state.image.is_some() && !state.patch_path.trim().is_empty();
        if ui
            .add_enabled(
                can_review,
                egui::Button::new("Preview DCP contents & compatibility"),
            )
            .clicked()
        {
            review = Some(PathBuf::from(state.patch_path.trim()));
        }
        if let Some(patch) = &state.patch {
            render_patch(ui, patch);
        }
        ui.add_enabled(false, egui::Button::new("Apply DCP to disc image"));
        ui.label("Apply unavailable: standard DCP has no exact source binding, and this panel has no verified DCP-to-image application route. Existing extracted-tree and exact-length GDI extent patching are separate backends; neither is wired to DCP packages here.");
    }
    if inspect {
        start(
            shared.clone(),
            ui.ctx().clone(),
            game.archive.absolute_path.clone(),
            None,
        );
    }
    if let Some(path) = review {
        start(
            shared,
            ui.ctx().clone(),
            game.archive.absolute_path.clone(),
            Some(path),
        );
    }
}

fn render_image(ui: &mut egui::Ui, image: &DreamcastMediaInspection, game: &Game) {
    ui.label(format!("Snapshot: {}", image.source.display()));
    ui.label(format!(
        "Selected data: {} · logical offset 0",
        image.data_path.display()
    ));
    ui.label(format!("IP.BIN (32 KiB) SHA-256: {}", image.boot_sha256));
    ui.label("Snapshot evidence covers boot bytes and observed file attributes, not complete track contents. Refresh to inspect again; DCP preview rechecks this evidence.");
    ui.label(format!("Structural status: {:?}", image.inspection.status));
    for issue in &image.inspection.issues {
        ui.colored_label(ui.visuals().warn_fg_color, issue);
    }
    if let Some(ip) = &image.inspection.ip_bin {
        let mut meta = ip.metadata.clone();
        let product = game.archive.identity_report.as_ref().and_then(|r| {
            r.verified_value(archivefs_core::game_identity::IdentityKind::DreamcastProductCode)
        });
        add_stronger_evidence_conflicts(&mut meta, product, None);
        egui::Grid::new(("dc_boot_fields", &image.source))
            .striped(true)
            .show(ui, |ui| {
                for (name, field) in [
                    ("Hardware", &meta.hardware_id),
                    ("Maker", &meta.maker_id),
                    ("Product identifier", &meta.product_number),
                    ("Revision", &meta.product_version),
                    ("Region declaration", &meta.area_symbols),
                    ("Boot filename declaration", &meta.boot_filename),
                    ("Release date", &meta.release_date),
                    ("Title", &meta.software_title),
                    ("Peripheral flags", &meta.peripheral_flags),
                ] {
                    ui.label(name);
                    ui.label(format!("{} ({:?})", field.value, field.validity));
                    ui.end_row();
                }
            });
        ui.label(format!(
            "VGA declaration: {:?} · Product CRC: {:?}",
            meta.vga_compatibility, ip.product_crc.status
        ));
        ui.label("Declarations do not establish runtime compatibility or bootability. Boot executable presence is not checked by this image inspector.");
        for warning in meta.warnings.iter().chain(&meta.conflicts) {
            ui.colored_label(ui.visuals().warn_fg_color, warning);
        }
        ui.collapsing("Raw metadata fields", |ui| {
            ui.monospace(format!("{:02X?}", &ip.raw_bytes()[..256]));
        });
    }
    if let Some(desc) = &image.topology {
        ui.collapsing("Preserved GDI track relationships", |ui| {
            ui.label(format!("Descriptor SHA-256: {}", image.descriptor_sha256.as_deref().unwrap_or("unavailable")));
            for track in &desc.tracks {
                ui.label(format!("Track {} · LBA {} · {:?} · {} bytes/sector · file offset {} · {}", track.number, track.start_lba, track.track_type, track.sector_size.bytes(), track.file_offset, track.source_filename.display()));
            }
            ui.label("Audio and all tracks remain untouched; topology is observed, not an output verification.");
        });
    }
}

fn render_patch(ui: &mut egui::Ui, patch: &DreamcastPatchReadiness) {
    ui.label(format!(
        "Package snapshot: {}",
        patch.package.path.display()
    ));
    ui.label(format!("Package SHA-256: {}", patch.package.package_sha256));
    ui.label("Source: user-selected local package; publisher authenticity and game target are not established.");
    ui.label(format!(
        "Compatibility: {:?} · {:?}",
        patch.state, patch.target_match
    ));
    ui.label(&patch.explanation);
    ui.label(
        "Matching filenames, titles and product identifiers never establish patch compatibility.",
    );
    ui.label(format!(
        "IP.BIN impact: {:?} · Track impact: {:?}",
        patch.package.ip_bin_impact, patch.package.track_impact
    ));
    for warning in &patch.warnings {
        ui.colored_label(ui.visuals().warn_fg_color, warning);
    }
    ui.collapsing("Intended package operations (unapplied)", |ui| {
        for entry in &patch.package.entries {
            ui.label(format!("{:?} · {} · {} bytes", entry.kind, entry.relative_path, entry.size_bytes));
            ui.monospace(&entry.sha256);
            for warning in &entry.warnings { ui.label(warning); }
        }
        ui.label("Replacement/delta classifications describe package members; source filesystem targets and final output bytes have not been verified.");
    });
}

#[cfg(test)]
mod tests;
