//! Native GUI-v2 projection of the read-only Saturn optical manifest.

use crate::saturn_evidence_cache::{Cache, Observed};
use archivefs_core::saturn_disc_manifest::{
    SaturnDescriptorType, SaturnDiscManifest, SaturnManifestIssue, SaturnManifestStatus,
    inspect_saturn_disc,
};
use archivefs_core::saturn_patch_readiness::{
    optical::{self, SaturnOpticalPreflight},
    saturn_manifest_fingerprint,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Evidence {
    manifest: SaturnDiscManifest,
    fingerprint: String,
    optical: SaturnOpticalPreflight,
}
impl Evidence {
    fn inspect(source: &std::path::Path) -> Result<Self, String> {
        let manifest =
            inspect_saturn_disc(source).map_err(|e| format!("Disc manifest unavailable: {e}"))?;
        let fingerprint = saturn_manifest_fingerprint(&manifest);
        let optical = optical::assess(&manifest);
        Ok(Self {
            manifest,
            fingerprint,
            optical,
        })
    }
}
type PanelCache = Arc<Mutex<Cache<PathBuf, Evidence>>>;
use eframe::egui;

use super::library::Game;

pub(super) fn applies(game: &Game) -> bool {
    game.platform.eq_ignore_ascii_case("Saturn")
        || game.platform.eq_ignore_ascii_case("Sega Saturn")
}

pub(super) fn show(ui: &mut egui::Ui, game: &Game) {
    if !applies(game) {
        return;
    }
    egui::CollapsingHeader::new("Saturn disc layout")
        .default_open(true)
        .show(ui, |ui| {
            let shared = ui.data_mut(|data| {
                data.get_temp_mut_or_default::<PanelCache>(egui::Id::new(
                    "saturn_disc_evidence_cache",
                ))
                .clone()
            });
            let Ok(mut cache) = shared.try_lock() else {
                return;
            };
            let source = &game.archive.absolute_path;
            cache.update(source);
            if ui.button("Recheck disc evidence").clicked() {
                cache.invalidate();
            }
            let path = source.clone();
            cache.start(source.clone(), ui.ctx().clone(), move || {
                Observed::inspect(&path, None, || Evidence::inspect(&path))
            });
            ui.ctx().request_repaint_after(Duration::from_millis(250));
            match cache.value() {
                Some(Ok(evidence)) => render_manifest(ui, evidence),
                Some(Err(error)) => render_unavailable(ui, error),
                None => {
                    if let Some(error) = &cache.error {
                        render_unavailable(ui, error);
                    } else {
                        ui.spinner();
                        ui.label(
                            "Checking disc evidence… No current source evidence is available.",
                        );
                        ui.label(
                            "Saturn patch readiness: Not ready while source evidence is checked.",
                        );
                        ui.label("Apply is unavailable: Saturn patch readiness is read-only.");
                    }
                }
            }
        });
}

fn render_unavailable(ui: &mut egui::Ui, error: &str) {
    ui.colored_label(crate::ui::theme::warning(ui), error);
    ui.label("EmuWiz will not infer Saturn topology from a lone BIN or unsupported descriptor.");
    ui.separator();
    ui.strong("Saturn patch readiness");
    ui.label("Status: Not ready");
    ui.label(
        "Why: a complete, safe Saturn manifest is required before a patch target can be previewed.",
    );
    ui.label("Apply is unavailable: Saturn patch readiness is read-only.");
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

fn render_manifest(ui: &mut egui::Ui, evidence: &Evidence) {
    let manifest = &evidence.manifest;
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
            crate::ui::theme::warning(ui),
            "Limited media: a descriptor is required to prove the full Saturn disc layout.",
        );
    }
    if !manifest.issues.is_empty() {
        ui.colored_label(
            crate::ui::theme::warning(ui),
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
            ui.monospace(format!("INDEX 00 {:?} · INDEX 01 {} · stored pregap {:?} · synthetic PREGAP {:?} · POSTGAP {:?}", track.index_00_frame, track.index_01_frame, track.in_file_pregap_frames, track.declared_pregap_frames, track.postgap_frames));
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
            ui.colored_label(crate::ui::theme::warning(ui), text);
        }
        ui.label(format!("Provenance: {}", manifest.provenance));
        ui.label("Read-only verifier: no fix, conversion, header rewrite, or source mutation is available here.");
    });
    render_patch_readiness_boundary(ui, evidence);
}

fn render_patch_readiness_boundary(ui: &mut egui::Ui, evidence: &Evidence) {
    let manifest = &evidence.manifest;
    let optical = &evidence.optical;
    egui::CollapsingHeader::new("Saturn patch readiness")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(if optical.refusals.is_empty() {
                "Source optical preflight: checked snapshot; source metadata unchanged"
            } else {
                "Source optical preflight: refused"
            });
            for reason in &optical.refusals {
                ui.colored_label(crate::ui::theme::warning(ui), reason);
            }
            for limit in &optical.limits { ui.label(limit); }
            ui.label("No patch selected");
            ui.label("Choose a patch to preview its source identity, target, and preservation impact.");
            ui.label("Status: Not ready");
            ui.label("Why: a patch must identify an exact Saturn source and an explicit target before preview is possible.");
            ui.label("Apply is unavailable: this feature is read-only and never rebuilds or mutates Saturn media.");
            ui.collapsing("Advanced readiness details", |ui| {
                ui.monospace(format!("Manifest fingerprint: {}", evidence.fingerprint));
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

#[cfg(test)]
pub(super) fn finish_worker_for_test(context: &egui::Context) {
    let cache = context.data_mut(|data| {
        data.get_temp::<PanelCache>(egui::Id::new("saturn_disc_evidence_cache"))
            .unwrap()
    });
    cache.lock().unwrap().finish_for_test();
}

#[cfg(test)]
mod optical_preflight_tests {
    use super::*;
    #[test]
    fn readiness_displays_ambiguous_gap_refusal_and_evidence_limits() {
        let root = tempfile::tempdir().unwrap();
        let mut data = vec![b' '; 2048 * 2];
        data[2048..2064].copy_from_slice(b"SEGA SEGASATURN ");
        std::fs::write(root.path().join("data.bin"), data).unwrap();
        let cue = root.path().join("disc.cue");
        std::fs::write(&cue, "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:00:01\nPREGAP 00:00:01\n").unwrap();
        let evidence = Evidence::inspect(&cue).unwrap();
        // Rendering must use the paired result, not reopen or rehash sources.
        // Cache invalidation (tested separately) happens before this renderer.
        std::fs::remove_file(&cue).unwrap();
        let context = egui::Context::default();
        let output = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_patch_readiness_boundary(ui, &evidence);
            });
        });
        let shapes = format!("{:?}", output.shapes);
        assert!(shapes.contains("Source optical preflight: refused"));
        assert!(shapes.contains("Ambiguous"));
        assert!(shapes.contains("full disc equivalence"));
        assert!(shapes.contains("Apply is unavailable"));
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn manifest_revision_replaces_the_paired_fingerprint_and_preflight() {
        let root = tempfile::tempdir().unwrap();
        let cue = root.path().join("disc.cue");
        let mut bytes = vec![b' '; 2048 * 2];
        bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
        std::fs::write(root.path().join("data.bin"), bytes).unwrap();
        let text = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
        std::fs::write(&cue, text).unwrap();
        let mut cache = Cache::<PathBuf, Evidence>::default();
        let refresh = |cache: &mut Cache<PathBuf, Evidence>| {
            cache.update(&cue);
            let path = cue.clone();
            assert!(
                cache.start(cue.clone(), Default::default(), move || Observed::inspect(
                    &path,
                    None,
                    || Evidence::inspect(&path)
                ))
            );
            cache.finish_for_test();
        };
        refresh(&mut cache);
        let old = cache.value().unwrap().as_ref().unwrap().fingerprint.clone();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&cue, format!("{text}POSTGAP 00:00:03\n")).unwrap();
        cache.update(&cue);
        assert!(cache.value().is_none());
        refresh(&mut cache);
        let fresh = cache.value().unwrap().as_ref().unwrap();
        assert_ne!(fresh.fingerprint, old);
        assert_eq!(fresh.manifest.tracks[0].postgap_frames, Some(3));
        assert!(fresh.optical.refusals.is_empty());
    }
}
