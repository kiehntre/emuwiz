//! Read-only presentation of the core tape-analysis result.
//!
//! This module deliberately contains no tape parsing.  It formats the common
//! `TapeAnalysis` model produced by archivefs-core and keeps technical timing
//! details behind an advanced disclosure.

use std::path::Path;

use archivefs_core::tape_analysis::{
    ChecksumState, KnownLoaderFamily, LoaderClass, LoaderConfidence, LoaderEvidence, TapeAnalysis,
    TapeEntry, TapeEntryKind, TapeFormat,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

pub(crate) fn is_tape_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("tap" | "tzx" | "cdt" | "t64" | "wav" | "cas" | "uef" | "pzx" | "csw")
    )
}

/// Whether the selected-file panel should reserve space for the asynchronous
/// tape-analysis progress card.  Keeping this decision next to the supported
/// extension gate prevents ordinary ROM selections from showing a misleading
/// tape-analysis animation while their general evidence inspection runs.
pub(crate) fn should_show_loading(path: &Path) -> bool {
    is_tape_path(path)
}

pub(crate) fn analyze_bytes(
    bytes: &[u8],
    platform_hint: Option<&str>,
) -> Result<TapeAnalysis, String> {
    if !bytes.starts_with(b"RIFF") {
        return archivefs_core::tape_analysis::analyze_tape(bytes)
            .map_err(|error| format!("tape analysis unavailable: {error:?}"));
    }
    let platform = platform_hint.unwrap_or("");
    let mut analysis = match platform {
        "ZX Spectrum" => archivefs_core::tape_audio::recovered_tape_analysis(
            &archivefs_core::tape_audio::decode_spectrum_wav(bytes)
                .map_err(|error| format!("WAV analysis unavailable: {error:?}"))?,
        )
        .ok_or_else(|| "WAV did not yield a complete ZX Spectrum block".to_string())?,
        "Commodore 64" => archivefs_core::tape_audio::commodore_wav_tape_analysis(bytes)
            .map_err(|error| format!("WAV analysis unavailable: {error:?}"))?,
        "Amstrad CPC" => archivefs_core::tape_audio::amstrad_cpc_custom_wav_tape_analysis(bytes)
            .map_err(|error| format!("WAV analysis unavailable: {error:?}"))?,
        "BBC Micro" => archivefs_core::bbc_tape::bbc_wav_tape_analysis(bytes)
            .map_err(|error| format!("WAV analysis unavailable: {error:?}"))?,
        "MSX" => msx_analysis(bytes)?,
        "Atari 8-bit" => archivefs_core::atari_tape::atari_wav_tape_analysis(bytes)
            .map_err(|error| format!("WAV analysis unavailable: {error:?}"))?,
        _ => return Err("WAV analysis needs a known tape platform hint".into()),
    };
    if platform == "Commodore 64"
        && let Ok(custom) = archivefs_core::tape_audio::decode_commodore_custom_wav(bytes)
        && !custom.stages.is_empty()
    {
        attach_custom_loader(
            &mut analysis,
            custom.loader_class,
            custom.fingerprint,
            custom.stages.len(),
            custom.warnings,
        );
    } else if platform == "BBC Micro"
        && let Ok(custom) = archivefs_core::bbc_tape::decode_bbc_custom_wav(bytes)
        && !custom.stages.is_empty()
    {
        attach_custom_loader(
            &mut analysis,
            custom.loader_class,
            archivefs_core::tape_audio::custom_loader_fingerprint(&custom.stages),
            custom.stages.len(),
            custom.warnings,
        );
    } else if platform == "Atari 8-bit"
        && let Ok(custom) = archivefs_core::atari_tape::decode_atari_custom_wav(bytes)
        && !custom.stages.is_empty()
    {
        attach_custom_loader(
            &mut analysis,
            custom.loader_class,
            custom.fingerprint,
            custom.stages.len(),
            custom.warnings,
        );
    }
    Ok(analysis)
}

fn attach_custom_loader(
    analysis: &mut TapeAnalysis,
    class: &str,
    fingerprint: String,
    stage_count: usize,
    warnings: Vec<String>,
) {
    analysis.loader = Some(LoaderEvidence {
        class: match class {
            "GenericTurbo" => LoaderClass::GenericTurbo,
            "MultiStage" => LoaderClass::MultiStage,
            "CustomPulse" => LoaderClass::CustomPulse,
            _ => LoaderClass::UnknownCustom,
        },
        confidence: LoaderConfidence::Medium,
        fingerprint,
        clues: vec![format!(
            "{stage_count} bounded custom stage(s) after a standard anchor"
        )],
    });
    analysis.logical_segments += stage_count;
    analysis.warnings.extend(warnings);
}

fn msx_analysis(bytes: &[u8]) -> Result<TapeAnalysis, String> {
    let recovery = archivefs_core::msx_tape::decode_msx_wav(bytes)
        .map_err(|error| format!("WAV analysis unavailable: {error:?}"))?;
    let entries = recovery
        .files
        .iter()
        .map(|file| TapeEntry {
            name: file.filename.clone(),
            kind: TapeEntryKind::Data,
            load_address: None,
            length: file.payload_length as u64,
            checksum: match file.integrity {
                archivefs_core::msx_tape::MsxIntegrity::Valid => ChecksumState::Valid,
                archivefs_core::msx_tape::MsxIntegrity::Invalid => ChecksumState::Invalid,
                archivefs_core::msx_tape::MsxIntegrity::NotPresent
                | archivefs_core::msx_tape::MsxIntegrity::Unknown => ChecksumState::NotPresent,
            },
        })
        .collect::<Vec<_>>();
    let checksum = if entries
        .iter()
        .any(|entry| entry.checksum == ChecksumState::Invalid)
    {
        ChecksumState::Invalid
    } else if entries
        .iter()
        .any(|entry| entry.checksum == ChecksumState::Valid)
    {
        ChecksumState::Valid
    } else {
        ChecksumState::NotPresent
    };
    Ok(TapeAnalysis {
        format: TapeFormat::MsxWav,
        platform: Some("MSX"),
        block_count: entries.len(),
        entries,
        metadata: vec!["MSX standard cassette waveform recovery".into()],
        loader: None,
        checksum,
        warnings: recovery.warnings,
        semantic_blocks: vec!["MSX standard data".into()],
        logical_segments: recovery.files.len().max(1),
        unsupported_blocks: 0,
    })
}

pub(crate) fn format_label(format: TapeFormat) -> &'static str {
    match format {
        TapeFormat::ZxTap => "TAP",
        TapeFormat::Tzx => "TZX / CDT",
        TapeFormat::CommodoreTap => "TAP",
        TapeFormat::CommodoreWav => "WAV",
        TapeFormat::AmstradCpcWav => "WAV",
        TapeFormat::BbcMicroWav => "WAV",
        TapeFormat::MsxWav => "WAV",
        TapeFormat::Atari8BitWav => "WAV",
        TapeFormat::T64 => "T64",
    }
}

pub(crate) fn platform_label(analysis: &TapeAnalysis) -> &'static str {
    analysis.platform.unwrap_or("Unidentified tape")
}

pub(crate) fn loader_label(loader: Option<&LoaderClass>) -> &'static str {
    match loader {
        Some(LoaderClass::RomStandard) => "Standard loader",
        Some(LoaderClass::GenericTurbo) => "Turbo loader",
        Some(LoaderClass::CustomPulse) => "Custom pulse loader",
        Some(LoaderClass::MultiStage) => "Multi-stage loader",
        Some(LoaderClass::UnknownCustom) => "Unknown custom loader",
        Some(LoaderClass::KnownFamily(KnownLoaderFamily::Alkatraz)) => "Known loader family",
        None => "Not identified",
    }
}

pub(crate) fn checksum_label(checksum: ChecksumState) -> &'static str {
    match checksum {
        ChecksumState::Valid => "Good",
        ChecksumState::Invalid => "Damaged",
        ChecksumState::NotPresent => "Unknown",
        ChecksumState::NotApplicable => "Not applicable",
    }
}

fn entry_kind_label(kind: TapeEntryKind) -> &'static str {
    match kind {
        TapeEntryKind::Basic => "BASIC",
        TapeEntryKind::Code => "Code",
        TapeEntryKind::Data => "Data",
        TapeEntryKind::Directory => "Directory",
    }
}

fn show_progress(ui: &mut egui::Ui) {
    let time = ui.input(|input| input.time);
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label("Reading tape analysis…");
    });
    ui.label(
        egui::RichText::new(match ((time * 2.0) as usize) % 6 {
            0 => "Reading waveform",
            1 => "Finding carrier / pilot",
            2 => "Detecting sync",
            3 => "Recovering blocks",
            4 => "Checking data",
            _ => "Identifying loader",
        })
        .color(theme::muted(ui))
        .small(),
    );
    ui.horizontal(|ui| {
        for index in 0..12 {
            let active = ((time * 4.0) as usize + index) % 12 < 5;
            let color = if active {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().widgets.inactive.bg_fill
            };
            ui.colored_label(color, "▰");
        }
    });
}

pub(crate) fn show_loading(ui: &mut egui::Ui) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Tape Analysis", Some("Read-only cassette recovery."));
        show_progress(ui);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    });
}

pub(crate) fn show_result(ui: &mut egui::Ui, analysis: &TapeAnalysis) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Tape Analysis", Some("Read-only cassette recovery."));
        ui.horizontal_wrapped(|ui| {
            ui.strong(platform_label(analysis));
            widgets::status_badge(ui, format_label(analysis.format), widgets::StatusTone::Info);
        });
        ui.separator();
        egui::Grid::new("tape_analysis_summary")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                ui.label("Programs / entries");
                ui.label(analysis.entries.len().to_string());
                ui.end_row();
                ui.label("Blocks recovered");
                ui.label(analysis.block_count.to_string());
                ui.end_row();
                ui.label("Loader");
                ui.label(loader_label(
                    analysis.loader.as_ref().map(|loader| &loader.class),
                ));
                ui.end_row();
                ui.label("Integrity");
                ui.label(checksum_label(analysis.checksum));
                ui.end_row();
            });

        if !analysis.entries.is_empty() {
            ui.add_space(4.0);
            ui.strong("Recovered content");
            egui::Grid::new("tape_analysis_entries")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for heading in ["Name", "Type", "Size", "Load", "Integrity"] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    for entry in analysis.entries.iter().take(256) {
                        ui.label(entry.name.as_deref().unwrap_or("Unnamed"));
                        ui.label(entry_kind_label(entry.kind));
                        ui.label(format_size(entry.length));
                        ui.label(
                            entry
                                .load_address
                                .map(|address| format!("0x{address:04X}"))
                                .unwrap_or_else(|| "—".into()),
                        );
                        ui.label(checksum_label(entry.checksum));
                        ui.end_row();
                    }
                });
        }

        if !analysis.semantic_blocks.is_empty() {
            ui.add_space(4.0);
            ui.strong("Tape timeline");
            ui.horizontal_wrapped(|ui| {
                for (index, block) in analysis.semantic_blocks.iter().enumerate() {
                    if index > 0 {
                        ui.label("→");
                    }
                    ui.label(egui::RichText::new(block).small());
                }
            });
        }

        for warning in &analysis.warnings {
            widgets::banner(
                ui,
                "Partial tape recovery",
                warning,
                widgets::StatusTone::Info,
            );
        }

        widgets::technical_details(ui, "tape-analysis-advanced", |ui| {
            ui.label(format!("Format: {}", format_label(analysis.format)));
            ui.label(format!("Logical segments: {}", analysis.logical_segments));
            ui.label(format!(
                "Unsupported blocks: {}",
                analysis.unsupported_blocks
            ));
            if let Some(loader) = &analysis.loader {
                ui.label(format!("Confidence: {:?}", loader.confidence));
                if !loader.fingerprint.is_empty() {
                    ui.label(format!("Loader fingerprint: {}", loader.fingerprint));
                }
                for clue in &loader.clues {
                    ui.label(format!("Evidence: {clue}"));
                }
            }
            for metadata in &analysis.metadata {
                ui.label(format!("Metadata: {metadata}"));
            }
            ui.weak("Raw PCM is not retained or rendered.");
        });
    });
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{} MiB", bytes / (1024 * 1024))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_labels_never_expose_enum_names() {
        assert_eq!(format_label(TapeFormat::Atari8BitWav), "WAV");
        assert_eq!(
            loader_label(Some(&LoaderClass::GenericTurbo)),
            "Turbo loader"
        );
        assert_eq!(checksum_label(ChecksumState::Invalid), "Damaged");
    }

    #[test]
    fn tape_extensions_are_bounded_to_known_inputs() {
        assert!(is_tape_path(Path::new("demo.tzx")));
        assert!(is_tape_path(Path::new("demo.wav")));
        assert!(!is_tape_path(Path::new("demo.iso")));
    }

    #[test]
    fn tape_progress_is_hidden_for_non_tape_selections() {
        assert!(should_show_loading(Path::new("demo.wav")));
        assert!(!should_show_loading(Path::new("demo.rom")));
        assert!(!should_show_loading(Path::new("demo.iso")));
    }
}
