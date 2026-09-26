//! Native GUI-v2 projection of the read-only Saturn optical manifest.

use archivefs_core::saturn_disc_manifest::{
    SaturnDescriptorType, SaturnDiscManifest, SaturnManifestIssue, SaturnManifestStatus,
    inspect_saturn_disc,
};
use archivefs_core::saturn_patch_readiness::saturn_manifest_fingerprint;
use eframe::egui;

use super::library::Game;

pub(super) fn show(ui: &mut egui::Ui, game: &Game) {
    if !game.platform.eq_ignore_ascii_case("Saturn")
        && !game.platform.eq_ignore_ascii_case("Sega Saturn")
    {
        return;
    }
    egui::CollapsingHeader::new("Saturn disc layout")
        .default_open(true)
        .show(ui, |ui| match inspect_saturn_disc(&game.archive.absolute_path) {
            Ok(manifest) => render_manifest(ui, &manifest),
            Err(error) => {
                ui.colored_label(
                    crate::ui::theme::WARNING,
                    format!("Disc manifest unavailable: {error}"),
                );
                ui.label("EmuWiz will not infer Saturn topology from a lone BIN or unsupported descriptor.");
                ui.separator();
                ui.strong("Saturn patch readiness");
                ui.label("Status: Not ready");
                ui.label("Why: a complete, safe Saturn manifest is required before a patch target can be previewed.");
                ui.label("Apply is unavailable: Saturn patch readiness is read-only.");
            }
        });
}

fn status_label(status: SaturnManifestStatus) -> &'static str {
    match status {
        SaturnManifestStatus::Complete => "Complete",
        SaturnManifestStatus::CompleteWithWarnings => "Complete with warnings",
        SaturnManifestStatus::Incomplete => "Incomplete",
        SaturnManifestStatus::Unsafe => "Unsafe",
        SaturnManifestStatus::Invalid => "Invalid",
    }
}

fn render_manifest(ui: &mut egui::Ui, manifest: &SaturnDiscManifest) {
    ui.label(format!("Disc status: {}", status_label(manifest.status)));
    ui.label(format!("Tracks: {}", manifest.tracks.len()));
    ui.label(format!(
        "Data tracks: {} · Audio tracks: {}",
        manifest
            .tracks
            .iter()
            .filter(|track| track.track_type
                == archivefs_core::saturn_disc_manifest::SaturnTrackType::Data)
            .count(),
        manifest
            .tracks
            .iter()
            .filter(|track| track.track_type
                == archivefs_core::saturn_disc_manifest::SaturnTrackType::Audio)
            .count()
    ));
    if let Some(system_id) = &manifest.system_id {
        ui.label(format!(
            "Product code: {}",
            if system_id.fact.product_number.is_empty() {
                "Unknown"
            } else {
                &system_id.fact.product_number
            }
        ));
        ui.label(format!(
            "Region: {}",
            if system_id.fact.area_symbols.is_empty() {
                "Unknown"
            } else {
                &system_id.fact.area_symbols
            }
        ));
        ui.label(format!(
            "Title: {}",
            if system_id.fact.game_title.is_empty() {
                "Unknown"
            } else {
                &system_id.fact.game_title
            }
        ));
    } else {
        ui.label("Product code: Unknown · Region: Unknown · Title: Unknown");
    }
    ui.label(format!(
        "Complete source set: {}",
        if manifest.status == SaturnManifestStatus::Complete
            || manifest.status == SaturnManifestStatus::CompleteWithWarnings
        {
            "Yes"
        } else {
            "No"
        }
    ));
    ui.label(format!(
        "Audio preserved: {}",
        if manifest.preservation.audio_preserved {
            "Yes"
        } else {
            "No"
        }
    ));
    if manifest.descriptor_type == SaturnDescriptorType::LoneBin {
        ui.colored_label(
            crate::ui::theme::WARNING,
            "Limited media: a descriptor is required to prove the full Saturn disc layout.",
        );
    }
    if !manifest.issues.is_empty() {
        ui.colored_label(
            crate::ui::theme::WARNING,
            format!("Layout warnings: {}", manifest.issues.len()),
        );
    }
    ui.collapsing("Advanced manifest details", |ui| {
        ui.label(format!("Descriptor: {}", manifest.source_descriptor.display()));
        ui.monospace(format!("Descriptor SHA-256: {}", manifest.descriptor_sha256));
        for component in &manifest.components {
            ui.monospace(format!("{} · {} bytes · {}", component.path.display(), component.size_bytes, component.sha256));
        }
        for track in &manifest.tracks {
            ui.label(format!("Track {:02} · {:?} · {:?} · {} sectors", track.number, track.track_type, track.mode, track.sector_count));
            ui.monospace(format!("INDEX 00 {:?} · INDEX 01 {} · pregap {:?} · postgap {:?}", track.index_00_frame, track.index_01_frame, track.in_file_pregap_frames.or(track.declared_pregap_frames), track.postgap_frames));
            if let Some(hash) = &track.audio_sha256 { ui.monospace(format!("Audio SHA-256: {hash}")); }
            if let Some(hash) = &track.data_logical_sha256 { ui.monospace(format!("Data logical SHA-256: {hash}")); }
        }
        if let Some(system_id) = &manifest.system_id {
            ui.label(format!("System ID location: {}", system_id.location));
            ui.label(format!("Maker: {} · Version: {} · Release date: {}", system_id.fact.maker_id, system_id.fact.version, system_id.fact.release_date));
            ui.label(format!("Device: {} · Peripherals: {}", system_id.fact.device_info, system_id.fact.peripherals));
            ui.monospace(format!("System ID raw bytes: {}", system_id.raw_hex));
        }
        for issue in &manifest.issues {
            let text = match issue {
                SaturnManifestIssue::UnsupportedRepresentation { detail } => detail.clone(),
                other => return_issue_text(other),
            };
            ui.colored_label(crate::ui::theme::WARNING, text);
        }
        ui.label(format!("Provenance: {}", manifest.provenance));
        ui.label("Read-only verifier: no fix, conversion, header rewrite, or source mutation is available here.");
    });
    render_patch_readiness_boundary(ui, manifest);
}

fn render_patch_readiness_boundary(ui: &mut egui::Ui, manifest: &SaturnDiscManifest) {
    egui::CollapsingHeader::new("Saturn patch readiness")
        .default_open(true)
        .show(ui, |ui| {
            ui.label("No patch selected");
            ui.label("Choose a patch to preview its source identity, target, and preservation impact.");
            ui.label("Status: Not ready");
            ui.label("Why: a patch must identify an exact Saturn source and an explicit target before preview is possible.");
            ui.label("Apply is unavailable: this feature is read-only and never rebuilds or mutates Saturn media.");
            ui.collapsing("Advanced readiness details", |ui| {
                ui.monospace(format!("Manifest fingerprint: {}", saturn_manifest_fingerprint(manifest)));
                ui.label(format!("Components bound: {}", manifest.components.len()));
                ui.label(format!("Track topology bound: {}", manifest.tracks.len()));
                ui.label("Target classification: unknown until a patch is inspected");
                ui.label("Provider claims remain external evidence and never become native Verified identity.");
            });
        });
}

fn return_issue_text(issue: &SaturnManifestIssue) -> String {
    format!("{issue:?}")
}
