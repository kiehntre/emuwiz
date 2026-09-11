//! Read-only presentation of an already-computed LaserDisc verifier result.
//! No filesystem access, probing, range arithmetic or readiness decisions.

use archivefs_core::laserdisc_set::{
    LaserdiscFamily, LaserdiscFrameRangeStatus, LaserdiscMediaMetadata, LaserdiscProbeStatus,
    LaserdiscReadiness, LaserdiscSetEvidence, VideoAssetEvidence,
};
use eframe::egui;

use crate::ui::components::{self as widgets, StatusTone};

fn readiness(status: LaserdiscReadiness) -> (&'static str, StatusTone) {
    match status {
        LaserdiscReadiness::Ready => ("Ready", StatusTone::Success),
        LaserdiscReadiness::Partial => ("Partial", StatusTone::Warning),
        LaserdiscReadiness::Broken => ("Broken", StatusTone::Blocked),
        LaserdiscReadiness::Unknown => ("Unknown", StatusTone::Pending),
    }
}

fn frame_range(status: LaserdiscFrameRangeStatus) -> (&'static str, StatusTone) {
    match status {
        LaserdiscFrameRangeStatus::RangeValid => {
            ("Within reported frame count", StatusTone::Success)
        }
        LaserdiscFrameRangeStatus::RangeExceedsMedia => {
            ("Exceeds reported media length", StatusTone::Blocked)
        }
        LaserdiscFrameRangeStatus::RangeUnverified => ("Unverified", StatusTone::Pending),
        LaserdiscFrameRangeStatus::MetadataUnavailable => {
            ("Metadata unavailable", StatusTone::Pending)
        }
        LaserdiscFrameRangeStatus::MalformedMapping => ("Malformed mapping", StatusTone::Blocked),
    }
}

fn probe_label(status: LaserdiscProbeStatus) -> &'static str {
    match status {
        LaserdiscProbeStatus::Available => "Available",
        LaserdiscProbeStatus::Unavailable => "Unavailable",
        LaserdiscProbeStatus::Failed => "Failed",
    }
}

fn duration(metadata: &LaserdiscMediaMetadata) -> String {
    metadata.duration_millis.map_or_else(
        || "unknown duration".into(),
        |millis| {
            let seconds = millis / 1000;
            format!(
                "{}:{:02}:{:02}.{:03}",
                seconds / 3600,
                (seconds / 60) % 60,
                seconds % 60,
                millis % 1000
            )
        },
    )
}

fn video_summary(asset: &VideoAssetEvidence) -> String {
    match asset.metadata.as_ref() {
        Some(metadata) if metadata.probe_status == LaserdiscProbeStatus::Available => {
            let resolution = match (metadata.width, metadata.height) {
                (Some(width), Some(height)) => format!("{width}×{height}"),
                _ => "unknown resolution".into(),
            };
            format!(
                "{}: {resolution} · {}",
                asset.media_name,
                duration(metadata)
            )
        }
        Some(metadata) => format!(
            "{}: metadata probe {}",
            asset.media_name,
            probe_label(metadata.probe_status).to_ascii_lowercase()
        ),
        None => format!("{}: metadata not recorded", asset.media_name),
    }
}

pub(super) fn show(ui: &mut egui::Ui, evidence: &LaserdiscSetEvidence) {
    ui.label("LaserDisc set");
    let family = match evidence.detected_family {
        LaserdiscFamily::Daphne => "Daphne",
        LaserdiscFamily::HypseusSinge => "Hypseus / Singe",
        LaserdiscFamily::Mame => "MAME",
        LaserdiscFamily::Unknown => "Not determined",
    };
    let (status, status_tone) = readiness(evidence.readiness);
    let (range, range_tone) = frame_range(evidence.frame_range_status);
    let media = format!(
        "{} present, {} missing",
        evidence.present_media.len(),
        evidence.missing_media.len()
    );
    widgets::status_rows(
        ui,
        &[
            ("Family", family, StatusTone::Info),
            ("Set status", status, status_tone),
            ("Media", &media, StatusTone::Info),
            ("Frame mapping", range, range_tone),
        ],
    );
    ui.weak("Set verification, not an emulator launch test or a whole-video integrity check.");
    for asset in evidence.video_assets.iter().take(3) {
        ui.label(video_summary(asset));
    }
    if evidence.video_assets.len() > 3 {
        ui.weak(format!(
            "{} more media entries in Technical details.",
            evidence.video_assets.len() - 3
        ));
    }
    if matches!(
        evidence.frame_range_status,
        LaserdiscFrameRangeStatus::RangeUnverified | LaserdiscFrameRangeStatus::MetadataUnavailable
    ) {
        ui.label("Frame coverage is not proven. Duration and frame rate are not used to invent a frame count.");
    }
    for name in evidence.missing_media.iter().take(3) {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Missing or empty media: {name}"),
        );
    }
    for warning in evidence.warnings.iter().take(3) {
        ui.colored_label(ui.visuals().warn_fg_color, warning);
    }
    if evidence.missing_media.len() > 3 || evidence.warnings.len() > 3 {
        ui.weak("More missing media or warnings are listed in Technical details.");
    }
    widgets::technical_details(
        ui,
        ("structural-laserdisc-details", &evidence.set_root),
        |ui| {
            show_technical(ui, evidence);
        },
    );
}

fn show_technical(ui: &mut egui::Ui, evidence: &LaserdiscSetEvidence) {
    ui.label(format!("Source set: {}", evidence.set_root.display()));
    if let Some(path) = &evidence.framefile_path {
        ui.label(format!("Framefile: {}", path.display()));
    }
    ui.label(format!(
        "Components: {} ROM, {} script, {} config · {} frame mappings",
        evidence.rom_components.len(),
        evidence.script_components.len(),
        evidence.config_components.len(),
        evidence.mappings.len()
    ));
    ui.weak("Metadata comes from the existing bounded ffprobe summary. Range verdicts come from the set verifier; no extra probe runs when these details are opened.");
    for range in &evidence.frame_ranges {
        let (label, _) = frame_range(range.status);
        ui.label(format!(
            "{}: referenced mapping frames {}–{} · {label}",
            range.media_name, range.first_referenced_frame, range.last_referenced_frame
        ));
    }
    for asset in &evidence.video_assets {
        ui.separator();
        ui.label(video_summary(asset));
        ui.label(format!("Media path: {}", asset.path.display()));
        if let Some(size) = asset.size_bytes {
            ui.label(format!("File size: {size} bytes"));
        }
        if !asset.readable {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Media was not readable at inspection time.",
            );
        }
        if let Some(note) = &asset.metadata_note {
            ui.label(note);
        }
        if let Some(metadata) = &asset.metadata {
            ui.label(format!(
                "Metadata probe: {}",
                probe_label(metadata.probe_status)
            ));
            // Unavailable/failed probes use zero placeholders in core. Do not
            // display those as observed stream counts or missing video proof.
            if metadata.probe_status == LaserdiscProbeStatus::Available {
                ui.label(format!(
                    "Container: {} · Video codec: {}",
                    metadata.container_format.as_deref().unwrap_or("unknown"),
                    metadata.video_codec.as_deref().unwrap_or("unknown")
                ));
                ui.label(format!(
                    "Reported frame rate: {} · Reported frame count: {}",
                    metadata.frame_rate.as_deref().unwrap_or("unknown"),
                    metadata.reported_frame_count.map_or_else(
                        || "unknown (not estimated)".into(),
                        |count| count.to_string()
                    )
                ));
                ui.label(format!(
                    "Streams: {} video, {} audio",
                    metadata.video_stream_count, metadata.audio_stream_count
                ));
            }
            for warning in &metadata.warnings {
                ui.colored_label(ui.visuals().warn_fg_color, warning);
            }
        }
    }
    for name in evidence.missing_media.iter().skip(3) {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("Missing or empty media: {name}"),
        );
    }
    for warning in evidence.warnings.iter().skip(3) {
        ui.colored_label(ui.visuals().warn_fg_color, warning);
    }
}

#[cfg(test)]
mod tests;
