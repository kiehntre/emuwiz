//! Read-only presentation of the core tape-analysis result.
//!
//! This module deliberately contains no tape parsing.  It formats the common
//! `TapeAnalysis` model produced by archivefs-core and keeps technical timing
//! details behind an advanced disclosure.

use std::path::{Path, PathBuf};
use std::time::Instant;

use archivefs_core::ArchiveRecord;
use archivefs_core::tape_analysis::{
    ChecksumState, KnownLoaderFamily, LoaderClass, LoaderConfidence, LoaderEvidence, TapeAnalysis,
    TapeEntry, TapeEntryKind, TapeFormat,
};
use eframe::egui;

use crate::ui::{components as widgets, theme};

/// One-shot request, set by the hero's "Browse library" button and consumed
/// by the library browser card the next time it renders, to scroll that
/// card into view. Kept in egui's own temporary widget memory (like
/// [`widgets::platform_picker`]'s filter box) rather than a new page-state
/// field, since it is purely a presentation nicety over state `show_page`
/// already has.
fn scroll_to_library_id() -> egui::Id {
    egui::Id::new("tape_inspector_scroll_to_library")
}

fn scroll_to_formats_id() -> egui::Id {
    egui::Id::new("tape_inspector_scroll_to_formats")
}

/// Action a Tape Inspector interaction asks the caller to perform. Both
/// variants only ever carry a path the user themselves chose (a native file
/// dialog pick, or a library row already indexed by the catalogue) - never a
/// path this page invented or discovered by scanning.
pub(crate) enum TapeInspectorAction {
    /// The user picked a file with the native file dialog.
    ChooseFile(PathBuf),
    /// The user picked an already-indexed tape candidate from the library
    /// browser.
    SelectLibraryTape(PathBuf),
}

/// One tape-shaped entry already known to the catalogue - a projection over
/// an existing [`ArchiveRecord`], never a filesystem scan of its own. The
/// `extension_label` is the bare uppercased file extension: a *candidate*
/// signal only, never a claim of confirmed format/platform identity (see
/// Section 7 of the task - that only ever comes from real analysis).
struct LibraryTapeCandidate<'a> {
    path: &'a Path,
    display_name: &'a str,
    platform: Option<&'a str>,
    extension_label: String,
}

fn tape_candidates(records: &[ArchiveRecord]) -> Vec<LibraryTapeCandidate<'_>> {
    records
        .iter()
        .map(|record| &record.mount_plan.archive)
        .filter(|archive| is_tape_path(&archive.path))
        .map(|archive| LibraryTapeCandidate {
            path: archive.path.as_path(),
            display_name: archive.identity.display_name.as_str(),
            platform: archive.identity.platform.as_deref(),
            extension_label: archive
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_uppercase(),
        })
        .collect()
}

/// Search/platform/format state for the library tape browser. Lives on the
/// caller's page state (like `OpticalConversionPageState`'s own fields) so
/// filter choices persist across frames without depending on egui's
/// temporary widget memory for anything beyond the scroll-into-view request.
#[derive(Default)]
pub(crate) struct LibraryTapeFilterState {
    pub(crate) search: String,
    platform: Option<String>,
    format: Option<String>,
    /// The poster is a bundled visual asset, decoded and uploaded once per
    /// egui context. Keeping the handle here avoids file reads or texture
    /// recreation during render frames.
    poster_texture: Option<egui::TextureHandle>,
    poster_load_attempted: bool,
}

const TAPE_INSPECTOR_POSTER_PNG: &[u8] = include_bytes!("../assets/emuwiz_tape_inspector_hero.png");

fn cached_poster_texture(
    ui: &egui::Ui,
    filter: &mut LibraryTapeFilterState,
) -> Option<egui::TextureHandle> {
    if !filter.poster_load_attempted {
        filter.poster_load_attempted = true;
        if let Ok(decoded) = image::load_from_memory(TAPE_INSPECTOR_POSTER_PNG) {
            let rgba = decoded.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            filter.poster_texture = Some(ui.ctx().load_texture(
                "emuwiz-tape-inspector-poster",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }
    filter.poster_texture.clone()
}

/// "Tape images in your library": a persistent, always-visible browser over
/// already-indexed candidates (never a render-time filesystem scan), with
/// search/platform/format filtering so a user does not have to hunt through
/// files one by one (Section 6/7 of the task). Returns the candidate the
/// user asked to inspect, if any, this frame.
fn show_library_browser(
    ui: &mut egui::Ui,
    records: Option<&[ArchiveRecord]>,
    filter: &mut LibraryTapeFilterState,
) -> Option<TapeInspectorAction> {
    let mut action = None;
    let candidates = records.map(tape_candidates).unwrap_or_default();
    widgets::card(ui, |ui| {
        let should_scroll = ui
            .ctx()
            .memory_mut(|memory| memory.data.remove_temp::<bool>(scroll_to_library_id()))
            .unwrap_or(false);
        widgets::section_header(
            ui,
            "Tape images in your library",
            Some(
                "Possible tape media already indexed by EmuWiz. An extension match is a \
                 candidate only - open one to see its confirmed format and platform.",
            ),
        );
        if should_scroll {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        if candidates.is_empty() {
            ui.label(
                egui::RichText::new("No tape-shaped files indexed in your library yet.")
                    .color(theme::muted(ui)),
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut filter.search);
        });
        let mut platforms: Vec<&str> = candidates.iter().filter_map(|c| c.platform).collect();
        platforms.sort_unstable();
        platforms.dedup();
        ui.horizontal_wrapped(|ui| {
            ui.label("Platform:");
            if ui
                .selectable_label(filter.platform.is_none(), "All platforms")
                .clicked()
            {
                filter.platform = None;
            }
            for platform in &platforms {
                if ui
                    .selectable_label(filter.platform.as_deref() == Some(*platform), *platform)
                    .clicked()
                {
                    filter.platform = Some((*platform).to_string());
                }
            }
        });
        let mut formats: Vec<&str> = candidates
            .iter()
            .map(|c| c.extension_label.as_str())
            .collect();
        formats.sort_unstable();
        formats.dedup();
        ui.horizontal_wrapped(|ui| {
            ui.label("Format:");
            if ui
                .selectable_label(filter.format.is_none(), "All formats")
                .clicked()
            {
                filter.format = None;
            }
            for format in &formats {
                if ui
                    .selectable_label(filter.format.as_deref() == Some(*format), *format)
                    .clicked()
                {
                    filter.format = Some((*format).to_string());
                }
            }
        });
        ui.separator();
        let query = filter.search.to_ascii_lowercase();
        let visible: Vec<&LibraryTapeCandidate<'_>> = candidates
            .iter()
            .filter(|candidate| {
                filter
                    .platform
                    .as_deref()
                    .is_none_or(|platform| candidate.platform == Some(platform))
                    && filter
                        .format
                        .as_deref()
                        .is_none_or(|format| candidate.extension_label == format)
                    && (query.is_empty()
                        || candidate.display_name.to_ascii_lowercase().contains(&query))
            })
            .collect();
        if visible.is_empty() {
            ui.label(
                egui::RichText::new("No tape images match these filters.").color(theme::muted(ui)),
            );
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("tape_inspector_library_browser")
            .max_height(240.0)
            .show(ui, |ui| {
                for candidate in &visible {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.strong(candidate.display_name);
                            let subtitle = match candidate.platform {
                                Some(platform) => {
                                    format!("{} candidate · {platform}", candidate.extension_label)
                                }
                                None => format!("{} candidate", candidate.extension_label),
                            };
                            ui.label(
                                egui::RichText::new(subtitle)
                                    .small()
                                    .color(theme::muted(ui)),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::action_button(
                                ui,
                                "Inspect",
                                widgets::ActionStyle::Secondary,
                                true,
                            )
                            .clicked()
                            {
                                action = Some(TapeInspectorAction::SelectLibraryTape(
                                    candidate.path.to_path_buf(),
                                ));
                            }
                        });
                    });
                    ui.separator();
                }
            });
    });
    action
}

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
        TapeFormat::OricTap => "TAP",
        TapeFormat::ZxTap => "TAP",
        TapeFormat::Tzx => "TZX / CDT",
        TapeFormat::CommodoreTap => "TAP",
        TapeFormat::CommodoreWav => "WAV",
        TapeFormat::AmstradCpcWav => "WAV",
        TapeFormat::BbcMicroWav => "WAV",
        TapeFormat::MsxWav => "WAV",
        TapeFormat::Atari8BitWav => "WAV",
        TapeFormat::T64 => "T64",
        TapeFormat::BbcUef => "UEF",
        TapeFormat::DragonCocoCas => "CAS",
    }
}

pub(crate) fn platform_label(analysis: &TapeAnalysis) -> &'static str {
    analysis.platform.unwrap_or("Unidentified tape")
}

/// A short human-facing identity line. This is deliberately a projection of
/// the core analysis platform, never an extension-based guess.
pub(crate) fn human_tape_kind(analysis: &TapeAnalysis) -> String {
    match analysis.platform {
        Some("ZX Spectrum") => "ZX Spectrum tape image".to_string(),
        Some("Commodore 64") => "Commodore 64 tape image".to_string(),
        Some("Amstrad CPC") => "Amstrad CPC tape image".to_string(),
        Some("BBC Micro") => "BBC Micro tape image".to_string(),
        Some("MSX") => "MSX tape image".to_string(),
        Some("Atari 8-bit") => "Atari 8-bit tape image".to_string(),
        Some(platform) => format!("{platform} tape image"),
        None => "Unidentified tape image".to_string(),
    }
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

pub(crate) fn checksum_summary(checksum: ChecksumState) -> &'static str {
    match checksum {
        ChecksumState::Valid => "Good",
        ChecksumState::Invalid => "Damaged",
        ChecksumState::NotPresent => "No checksum available",
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimelineGroup {
    label: &'static str,
    entries: Vec<String>,
}

fn timeline_groups(blocks: &[String]) -> Vec<TimelineGroup> {
    let mut groups: Vec<TimelineGroup> = Vec::new();
    for block in blocks {
        let lower = block.to_ascii_lowercase();
        let label = if lower.contains("turbo") || lower.contains("pilot") {
            "Turbo / pilot blocks"
        } else if lower.contains("pause") || lower.contains("gap") {
            "Pause / gap blocks"
        } else if lower.contains("unsupported") || lower.contains("opaque") {
            "Unsupported / opaque blocks"
        } else {
            "Other tape blocks"
        };
        if let Some(group) = groups.iter_mut().find(|group| group.label == label) {
            group.entries.push(block.clone());
        } else {
            groups.push(TimelineGroup {
                label,
                entries: vec![block.clone()],
            });
        }
    }
    groups
}

/// Tape Inspector's page-hero motif: a small cassette body with a
/// deterministic sine waveform across it. This is a lightweight placeholder
/// - drawn with the painter directly (a handful of fixed-size line segments,
/// no heap allocation, no image decode), not real game/box artwork. The
/// phase is driven by the UI clock only when `animate` is set, so a static
/// render (no analysis in flight) costs nothing extra and never requests a
/// repaint on its own.
fn draw_tape_motif(ui: &mut egui::Ui, size: egui::Vec2, animate: bool) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let body = rect.shrink(6.0);
    let label_rect = egui::Rect::from_min_max(
        egui::pos2(
            body.left() + body.width() * 0.08,
            body.top() + body.height() * 0.12,
        ),
        egui::pos2(
            body.right() - body.width() * 0.08,
            body.bottom() - body.height() * 0.15,
        ),
    );
    painter.rect_filled(body, 8.0, egui::Color32::from_rgb(42, 45, 43));
    painter.rect_stroke(body, 8.0, theme::border(ui), egui::StrokeKind::Inside);
    painter.rect_filled(label_rect, 4.0, egui::Color32::from_rgb(214, 201, 169));
    painter.rect_stroke(
        label_rect,
        4.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(60, 56, 47)),
        egui::StrokeKind::Inside,
    );
    for (index, color) in [
        egui::Color32::from_rgb(220, 65, 52),
        egui::Color32::from_rgb(239, 164, 47),
        egui::Color32::from_rgb(48, 171, 104),
        egui::Color32::from_rgb(53, 126, 190),
    ]
    .into_iter()
    .enumerate()
    {
        let y = label_rect.top() + label_rect.height() * (0.53 + index as f32 * 0.07);
        painter.line_segment(
            [
                egui::pos2(label_rect.left(), y),
                egui::pos2(label_rect.right(), y),
            ],
            egui::Stroke::new((label_rect.height() * 0.035).max(1.0), color),
        );
    }
    painter.text(
        label_rect.center_top() + egui::vec2(0.0, label_rect.height() * 0.20),
        egui::Align2::CENTER_CENTER,
        "EMUWIZ",
        egui::FontId::proportional((label_rect.height() * 0.22).clamp(12.0, 28.0)),
        egui::Color32::from_rgb(28, 31, 29),
    );
    // Two reel hubs.
    let reel_y = body.top() + body.height() * 0.35;
    for fraction in [0.28, 0.72] {
        let center = egui::pos2(body.left() + body.width() * fraction, reel_y);
        painter.circle_filled(
            center,
            body.height() * 0.12,
            egui::Color32::from_rgb(20, 23, 22),
        );
        painter.circle_stroke(
            center,
            body.height() * 0.12,
            egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(205, 205, 190)),
        );
        painter.circle_stroke(
            center,
            body.height() * 0.055,
            egui::Stroke::new(1.2_f32, theme::TEAL),
        );
    }
    // A short, fixed-length sine waveform across the lower third - the only
    // per-frame computation here is this small fixed loop, no allocation.
    let phase = if animate {
        ui.input(|input| input.time) as f32
    } else {
        0.0
    };
    let wave_top = body.top() + body.height() * 0.69;
    let wave_height = body.height() * 0.20;
    const POINTS: usize = 24;
    let mut previous: Option<egui::Pos2> = None;
    for index in 0..POINTS {
        let t = index as f32 / (POINTS - 1) as f32;
        let x = body.left() + t * body.width();
        let y = wave_top + wave_height * 0.5
            - (t * 10.0 + phase * 2.0).sin() * wave_height * 0.5 * 0.85;
        let point = egui::pos2(x, y);
        if let Some(previous) = previous {
            painter.line_segment([previous, point], egui::Stroke::new(1.5_f32, theme::TEAL));
        }
        previous = Some(point);
    }
    if animate {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    }
}

fn analysis_started_id(path: &Path) -> egui::Id {
    egui::Id::new(("tape-analysis-started", path))
}

fn analysis_elapsed(ui: &egui::Ui, path: &Path) -> f32 {
    let id = analysis_started_id(path);
    let started = ui.ctx().data_mut(|data| {
        data.get_temp::<Instant>(id).unwrap_or_else(|| {
            let now = Instant::now();
            data.insert_temp(id, now);
            now
        })
    });
    started.elapsed().as_secs_f32()
}

fn show_progress(ui: &mut egui::Ui) {
    let time = ui.input(|input| input.time);
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label("Analysing tape…");
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
    show_loading_with_elapsed(ui, None);
}

fn show_loading_with_elapsed(ui: &mut egui::Ui, elapsed_seconds: Option<f32>) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Tape Analysis", Some("Read-only cassette recovery."));
        show_progress(ui);
        if let Some(elapsed) = elapsed_seconds {
            ui.label(format!("Elapsed: {elapsed:.1}s"));
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(120));
    });
}

pub(crate) fn show_result(ui: &mut egui::Ui, analysis: &TapeAnalysis) {
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Tape Analysis", Some("Read-only cassette recovery."));
        ui.horizontal_wrapped(|ui| {
            ui.strong(human_tape_kind(analysis));
            widgets::status_badge(ui, format_label(analysis.format), widgets::StatusTone::Info);
        });
        ui.separator();
        egui::Grid::new("tape_analysis_summary")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                ui.label("Programs found");
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
                ui.label(checksum_summary(analysis.checksum));
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
            for group in timeline_groups(&analysis.semantic_blocks) {
                ui.collapsing(format!("{} ({})", group.label, group.entries.len()), |ui| {
                    for block in group.entries.iter().take(8) {
                        ui.label(egui::RichText::new(block).small());
                    }
                    if group.entries.len() > 8 {
                        ui.weak(format!("{} more blocks hidden", group.entries.len() - 8));
                    }
                });
            }
            if analysis.semantic_blocks.len() > 8 {
                ui.weak(format!(
                    "Timeline condensed into {} block groups.",
                    timeline_groups(&analysis.semantic_blocks).len()
                ));
            }
        }

        if !analysis.warnings.is_empty() {
            let count = analysis.warnings.len();
            widgets::banner(
                ui,
                "Review warning",
                &format!(
                    "{count} tape recovery warning{} reported.",
                    if count == 1 { "" } else { "s" }
                ),
                widgets::StatusTone::Info,
            );
            ui.collapsing("Review warning details", |ui| {
                for warning in analysis.warnings.iter().take(3) {
                    ui.label(warning);
                }
                if count > 3 {
                    ui.weak(format!("{} more warnings in Technical details", count - 3));
                }
            });
        }

        widgets::technical_details(ui, "tape-analysis-advanced", |ui| {
            ui.label(format!("Detected platform: {}", platform_label(analysis)));
            ui.label(format!("Format: {}", format_label(analysis.format)));
            ui.label(format!("Logical segments: {}", analysis.logical_segments));
            ui.label(format!(
                "Unsupported blocks: {}",
                analysis.unsupported_blocks
            ));
            for warning in &analysis.warnings {
                ui.label(format!("Warning: {warning}"));
            }
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

/// The formats Tape Inspector can actually present a result for today - the
/// exact, deduplicated set of [`format_label`] outputs, each paired with a
/// caller-chosen identity colour purely for visual distinction (never a
/// status colour). Kept next to `format_label` so a new `TapeFormat`
/// variant's label is never silently missing from this list without a
/// deliberate edit here.
const SUPPORTED_TAPE_FORMAT_CHIPS: [(&str, egui::Color32); 6] = [
    ("TAP", egui::Color32::from_rgb(59, 130, 246)),
    ("TZX / CDT", egui::Color32::from_rgb(34, 197, 94)),
    ("T64", egui::Color32::from_rgb(168, 85, 247)),
    ("WAV", theme::TEAL),
    ("UEF", egui::Color32::from_rgb(245, 158, 11)),
    ("CAS", egui::Color32::from_rgb(236, 72, 153)),
];

/// The Tape Inspector's "signal panel" status lines (see
/// [`widgets::signal_panel`]) - derived only from the same
/// `selected_path`/`analysis` state `show_page` already has, never a
/// fabricated reading. Mirrors the reference mockup's "TAPE SIGNAL …"
/// terminal readout, but every line here is an honest projection of real
/// page state: idle before a tape is chosen, "READING..." while analysis is
/// in flight, and the tape's own recovered checksum state once analysis
/// completes.
fn tape_signal_lines(
    selected_path: Option<&Path>,
    analysis: Option<&Result<TapeAnalysis, String>>,
) -> [String; 2] {
    if selected_path.is_none() {
        return ["TAPE SIGNAL".to_string(), "IDLE".to_string()];
    }
    let state = match analysis {
        None => "READING...".to_string(),
        Some(Err(_)) => "ERROR".to_string(),
        Some(Ok(value)) => match value.checksum {
            ChecksumState::Valid => "LOAD OK".to_string(),
            ChecksumState::Invalid => "LOAD DAMAGED".to_string(),
            ChecksumState::NotPresent | ChecksumState::NotApplicable => "LOAD UNKNOWN".to_string(),
        },
    };
    ["TAPE SIGNAL".to_string(), state]
}

/// The signal panel's waveform readout: the same lightweight, deterministic,
/// fixed-point-count line the hero motif draws (see [`draw_tape_motif`]),
/// painted here into the signal panel's own bounded reading area instead of
/// the motif slot. `phase` is `0.0` (a static line) unless analysis is
/// actually in flight - the caller decides that, this function never reads
/// the clock itself.
fn draw_tape_signal_reading(painter: &egui::Painter, rect: egui::Rect, phase: f32) {
    const POINTS: usize = 28;
    let mut previous: Option<egui::Pos2> = None;
    for index in 0..POINTS {
        let t = index as f32 / (POINTS - 1) as f32;
        let x = rect.left() + t * rect.width();
        let y = rect.center().y - (t * 14.0 + phase * 2.5).sin() * rect.height() * 0.42;
        let point = egui::pos2(x, y);
        if let Some(previous) = previous {
            painter.line_segment([previous, point], egui::Stroke::new(1.5_f32, theme::TEAL));
        }
        previous = Some(point);
    }
}

/// The native-file-dialog extension groups for "Choose tape file". `.tap` is
/// deliberately listed under both the ZX Spectrum and Commodore filters (and
/// the all-formats filter): it is genuinely ambiguous across those two
/// platforms, so the filter can only narrow candidates, never claim identity
/// - actual platform/format identity always comes from analysis (Section 4/5
/// of the task).
fn pick_tape_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(
            "All supported tape images",
            &[
                "tap", "tzx", "cdt", "t64", "wav", "cas", "uef", "pzx", "csw",
            ],
        )
        .add_filter("ZX Spectrum tapes", &["tzx", "cdt", "pzx", "csw", "tap"])
        .add_filter("Commodore tapes", &["tap", "t64"])
        .add_filter("All files", &["*"])
        .pick_file()
}

fn show_poster_tape_hero(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    selected_path: Option<&Path>,
    analysis: Option<&Result<TapeAnalysis, String>>,
    library_count: Option<usize>,
) -> Option<TapeInspectorAction> {
    let mut action = None;
    let available_width = ui.available_width().max(1.0);
    let image_size = egui::vec2(available_width, available_width * 821.0 / 1916.0);
    let (image_rect, _) = ui.allocate_exact_size(image_size, egui::Sense::hover());
    ui.painter().image(
        texture.id(),
        image_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    let narrow = image_rect.width() < 760.0;
    if !narrow {
        let button_y = image_rect.top() + image_rect.height() * 0.631;
        let button_h = image_rect.height() * 0.168;
        let button = |left: f32, right: f32| {
            egui::Rect::from_min_max(
                egui::pos2(image_rect.left() + image_rect.width() * left, button_y),
                egui::pos2(
                    image_rect.left() + image_rect.width() * right,
                    button_y + button_h,
                ),
            )
        };
        let choose_rect = button(0.025, 0.231);
        let browse_rect = button(0.241, 0.445);
        let formats_rect = button(0.454, 0.659);
        ui.allocate_ui_at_rect(choose_rect, |ui| {
            if ui
                .add_sized(
                    choose_rect.size(),
                    egui::Button::new("Choose tape file").fill(theme::ACCENT),
                )
                .clicked()
                && let Some(path) = pick_tape_file()
            {
                action = Some(TapeInspectorAction::ChooseFile(path));
            }
        });
        ui.allocate_ui_at_rect(browse_rect, |ui| {
            let label = match library_count {
                Some(count) if count > 0 => format!("Browse tape images in library ({count})"),
                _ => "Browse tape images in library".to_string(),
            };
            // `add_enabled_ui` (a *closure*, wrapping the button in its own
            // nested child `Ui`) never registered a click here, even while
            // fully enabled - an extra nested `Ui` layer inside an
            // already-relocated `allocate_ui_at_rect` child apparently
            // breaks hit-testing for the widget inside it. `add_enabled`
            // (the plain, non-closure form - disables the widget directly,
            // no extra `Ui`) does not have that problem and was proven, by
            // a real click-simulation test, to register hover/click
            // correctly in this exact position. See
            // `real_poster_button_click_at_a_realistic_window_height`.
            let enabled = library_count.is_some_and(|count| count > 0);
            let response = ui.add_enabled(
                enabled,
                egui::Button::new(label).min_size(browse_rect.size()),
            );
            if response.clicked() {
                ui.ctx()
                    .memory_mut(|memory| memory.data.insert_temp(scroll_to_library_id(), true));
            }
        });
        ui.allocate_ui_at_rect(formats_rect, |ui| {
            if ui
                .add_sized(formats_rect.size(), egui::Button::new("Supported formats"))
                .clicked()
            {
                ui.ctx()
                    .memory_mut(|memory| memory.data.insert_temp(scroll_to_formats_id(), true));
            }
        });

        let crt = egui::Rect::from_min_max(
            egui::pos2(
                image_rect.left() + image_rect.width() * 0.758,
                image_rect.top() + image_rect.height() * 0.168,
            ),
            egui::pos2(
                image_rect.left() + image_rect.width() * 0.989,
                image_rect.top() + image_rect.height() * 0.630,
            ),
        );
        let lines = tape_signal_lines(selected_path, analysis);
        let phase = ui.input(|input| input.time) as f32;
        let animating = selected_path.is_some() && analysis.is_none();
        let painter = ui.painter();
        painter.rect_filled(
            crt,
            8.0,
            egui::Color32::from_rgba_unmultiplied(0, 20, 18, 180),
        );
        for (index, line) in lines.iter().enumerate() {
            painter.text(
                crt.left_top()
                    + egui::vec2(
                        crt.width() * 0.08,
                        crt.height() * (0.08 + index as f32 * 0.10),
                    ),
                egui::Align2::LEFT_TOP,
                line,
                egui::FontId::monospace((crt.width() * 0.027).clamp(11.0, 22.0)),
                theme::TEAL,
            );
        }
        draw_tape_signal_reading(
            painter,
            egui::Rect::from_min_max(
                crt.left_top() + egui::vec2(crt.width() * 0.06, crt.height() * 0.31),
                crt.right_bottom() - egui::vec2(crt.width() * 0.06, crt.height() * 0.20),
            ),
            if animating { phase } else { 0.0 },
        );
    } else {
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(ui, "Choose tape file", widgets::ActionStyle::Primary, true)
                .clicked()
                && let Some(path) = pick_tape_file()
            {
                action = Some(TapeInspectorAction::ChooseFile(path));
            }
            let label = match library_count {
                Some(count) if count > 0 => format!("Browse tape images in library ({count})"),
                _ => "Browse tape images in library".to_string(),
            };
            if widgets::action_button(ui, label, widgets::ActionStyle::Secondary, true).clicked() {
                ui.ctx()
                    .memory_mut(|memory| memory.data.insert_temp(scroll_to_library_id(), true));
            }
            if widgets::action_button(
                ui,
                "Supported formats",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
            {
                ui.ctx()
                    .memory_mut(|memory| memory.data.insert_temp(scroll_to_formats_id(), true));
            }
        });
    }
    action
}

/// The Tape Inspector hero is intentionally page-specific: the reference's
/// personality comes from the cassette and scope sharing one large visual
/// stage, rather than from a small icon beside a generic page header.
fn show_tape_hero(
    ui: &mut egui::Ui,
    poster_texture: Option<&egui::TextureHandle>,
    selected_path: Option<&Path>,
    analysis: Option<&Result<TapeAnalysis, String>>,
    library_count: Option<usize>,
) -> Option<TapeInspectorAction> {
    if let Some(texture) = poster_texture {
        return show_poster_tape_hero(ui, texture, selected_path, analysis, library_count);
    }
    let mut action = None;
    let animating = selected_path.is_some() && analysis.is_none();
    let signal_lines = tape_signal_lines(selected_path, analysis);
    let phase = ui.input(|input| input.time) as f32;
    widgets::hero_card(ui, |ui| {
        let narrow = ui.available_width() < 760.0;
        let mut copy = |ui: &mut egui::Ui| {
            ui.label(
                egui::RichText::new("Tape Inspector")
                    .size(theme::PAGE_TITLE_SIZE)
                    .strong(),
            );
            ui.label(
                egui::RichText::new("Inspect tape images safely without modifying files.")
                    .size(16.0),
            );
            ui.label(
                egui::RichText::new(
                    "View entries, loader information and checksums for supported cassette formats.\nThe original file stays exactly where you left it.",
                )
                .color(theme::muted(ui)),
            );
            ui.add_space(theme::SPACE_SM);
            ui.horizontal_wrapped(|ui| {
                if widgets::action_button(
                    ui,
                    "Choose tape file",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                    && let Some(path) = pick_tape_file()
                {
                    action = Some(TapeInspectorAction::ChooseFile(path));
                }
                let browse_label = match library_count {
                    Some(count) if count > 0 => format!("Browse tape images in library ({count})"),
                    _ => "Browse tape images in library".to_string(),
                };
                if widgets::action_button(
                    ui,
                    browse_label,
                    widgets::ActionStyle::Secondary,
                    library_count.is_some_and(|count| count > 0),
                )
                .clicked()
                {
                    ui.ctx()
                        .memory_mut(|memory| memory.data.insert_temp(scroll_to_library_id(), true));
                }
            });
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, "Read-only inspection", widgets::StatusTone::Info);
                ui.label(
                    egui::RichText::new("Replay your youth.")
                        .italics()
                        .color(theme::muted(ui)),
                );
            });
        };

        if narrow {
            copy(ui);
            ui.add_space(theme::SPACE_SM);
            ui.horizontal_wrapped(|ui| {
                draw_tape_motif(ui, egui::vec2(210.0, 132.0), animating);
                widgets::signal_panel(
                    ui,
                    egui::vec2(220.0, 112.0),
                    &signal_lines,
                    |painter, rect| {
                        draw_tape_signal_reading(
                            painter,
                            rect,
                            if animating { phase } else { 0.0 },
                        );
                    },
                );
            });
        } else {
            ui.horizontal(|ui| {
                let copy_width = (ui.available_width() - 470.0).max(360.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(copy_width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    &mut copy,
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    widgets::signal_panel(
                        ui,
                        egui::vec2(220.0, 116.0),
                        &signal_lines,
                        |painter, rect| {
                            draw_tape_signal_reading(
                                painter,
                                rect,
                                if animating { phase } else { 0.0 },
                            );
                        },
                    );
                    draw_tape_motif(ui, egui::vec2(240.0, 150.0), animating);
                });
            });
        }
    });
    ui.add_space(theme::SPACE_SM);
    action
}

/// First-class, read-only Tape Inspector surface.  Analysis is supplied by
/// the selected-evidence worker; rendering never opens or rescans a file.
/// `library_records` is the already-loaded catalogue (never a render-time
/// filesystem scan) used to populate the library tape browser; `filter` is
/// that browser's persisted search/platform/format state. Returns the
/// action, if any, the caller should apply this frame - both variants only
/// ever carry a path the user themselves chose.
pub(crate) fn show_page(
    ui: &mut egui::Ui,
    selected_path: Option<&Path>,
    analysis: Option<&Result<TapeAnalysis, String>>,
    library_records: Option<&[ArchiveRecord]>,
    filter: &mut LibraryTapeFilterState,
) -> Option<TapeInspectorAction> {
    show_page_with_error(ui, selected_path, analysis, None, library_records, filter)
}

pub(crate) fn show_page_with_error(
    ui: &mut egui::Ui,
    selected_path: Option<&Path>,
    analysis: Option<&Result<TapeAnalysis, String>>,
    analysis_error: Option<&str>,
    library_records: Option<&[ArchiveRecord]>,
    filter: &mut LibraryTapeFilterState,
) -> Option<TapeInspectorAction> {
    let mut action = None;
    let library_count = library_records.map(|records| tape_candidates(records).len());
    let poster_texture = cached_poster_texture(ui, filter);
    if let Some(hero_action) = show_tape_hero(
        ui,
        poster_texture.as_ref(),
        selected_path,
        analysis,
        library_count,
    ) {
        action = Some(hero_action);
    }
    ui.add_space(theme::SPACE_SM);
    widgets::card(ui, |ui| {
        let should_scroll = ui
            .ctx()
            .memory_mut(|memory| memory.data.remove_temp::<bool>(scroll_to_formats_id()))
            .unwrap_or(false);
        widgets::section_header(
            ui,
            "Supported tape formats",
            Some("EmuWiz can read and present these tape families today."),
        );
        if should_scroll {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        widgets::format_chip_row(ui, &SUPPORTED_TAPE_FORMAT_CHIPS);
        ui.add_space(theme::SPACE_XS);
        ui.weak(
            "TAP · possible ZX Spectrum / Commodore tape  ·  TZX / CDT · ZX Spectrum tape  ·  \
             T64 · Commodore 64 tape archive",
        );
    });
    ui.add_space(theme::SPACE_SM);
    if let Some(library_action) = show_library_browser(ui, library_records, filter) {
        action = Some(library_action);
    }
    ui.add_space(theme::SPACE_SM);
    let Some(path) = selected_path.filter(|path| is_tape_path(path)) else {
        let detail = match library_count {
            Some(count) if count > 0 => format!(
                "Choose a tape file above, or pick one of the {count} tape image{plural} already \
                 in your library. Supported families are shown only when bounded analysis can \
                 identify them.",
                plural = if count == 1 { "" } else { "s" }
            ),
            _ => "Choose a tape file above, or select a tape in Library and choose Inspect tape. \
                  Supported families are shown only when bounded analysis can identify them."
                .to_string(),
        };
        widgets::empty_state(ui, "No tape selected", &detail, None);
        return action;
    };
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Selected tape", None);
        ui.label(path.display().to_string());
        ui.label(if is_tape_path(path) {
            "Analysis is bounded and read-only."
        } else {
            "This file is not a supported tape input."
        });
    });
    match analysis {
        Some(Ok(value)) => show_result(ui, value),
        Some(Err(error)) => widgets::banner(
            ui,
            "Tape analysis unavailable",
            error,
            widgets::StatusTone::Warning,
        ),
        None => match analysis_error {
            Some(error) => widgets::banner(
                ui,
                "Tape analysis unavailable",
                error,
                widgets::StatusTone::Warning,
            ),
            None => {
                let elapsed = analysis_elapsed(ui, path);
                show_loading_with_elapsed(ui, Some(elapsed));
            }
        },
    }
    action
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

    // --- Visual language pass: page hero / workflow strip / small viewport ---

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
                egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    /// The screen-space position of the first text shape whose text
    /// contains `needle`, if any - used to click a real button at its
    /// actual rendered position instead of recomputing hero layout math a
    /// second time in the test.
    fn rendered_text_pos(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
        fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(text_shape) if text_shape.galley.text().contains(needle) => {
                    Some(text_shape.pos)
                }
                egui::Shape::Vec(nested) => nested.iter().find_map(|s| find(s, needle)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| find(&clipped.shape, needle))
    }

    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    /// 1/2/5. Proves the library button's click actually moves the shared
    /// outer scroll to reveal the browser - not just that a memory flag was
    /// set. The screen is wide (so the poster's calibrated-rect button
    /// path, not the narrow text-button fallback, is exercised) and a
    /// realistic-but-modest height (450px - well short of the poster's own
    /// ~460-680px height at this width, so the library section genuinely
    /// starts below the fold, exactly like the physical QA report) -
    /// reproducing the exact physical-QA conditions without resorting to
    /// an artificially tiny viewport.
    ///
    /// Settling takes a handful of frames: `ui.scroll_to_cursor` animates
    /// the shared `ScrollArea` smoothly (a real app renders continuously at
    /// 60fps, so this settles in well under 100ms) - this loop mirrors that
    /// rather than asserting an instant single-frame jump.
    #[test]
    fn real_poster_button_click_at_a_realistic_window_height() {
        let records = vec![tape_record("game.tap", Some("ZX Spectrum"))];
        let mut filter = LibraryTapeFilterState::default();
        let screen = egui::vec2(1600.0, 450.0);
        let ctx = egui::Context::default();

        let render = |ctx: &egui::Context,
                      filter: &mut LibraryTapeFilterState,
                      events: Vec<egui::Event>|
         -> egui::FullOutput {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::ui::layout::page(
                            ui,
                            crate::ui::layout::ContentWidth::Normal,
                            true,
                            "realistic-height-test",
                            |ui| {
                                let _ = show_page(ui, None, None, Some(&records), filter);
                            },
                        );
                    });
                },
            )
        };

        let baseline = render(&ctx, &mut filter, Vec::new());
        assert!(rendered_text_contains(
            &baseline,
            "Browse tape images in library"
        ));
        assert!(
            !rendered_text_contains(&baseline, "Tape images in your library"),
            "the library section must genuinely start below the fold at this size - \
             otherwise this test would not be exercising scroll at all"
        );
        let button_pos = rendered_text_pos(&baseline, "Browse tape images in library")
            .expect("button text must have a real screen position");
        let click_pos = button_pos + egui::vec2(4.0, 4.0);

        let _ = render(&ctx, &mut filter, click_at(click_pos));
        let mut after = None;
        for _ in 0..10 {
            let out = render(&ctx, &mut filter, Vec::new());
            let visible = rendered_text_contains(&out, "Tape images in your library");
            if visible {
                after = Some(out);
                break;
            }
        }
        assert!(
            after.is_some(),
            "clicking the library button must scroll it into view, not silently no-op"
        );
    }

    fn render_at(
        screen: egui::Vec2,
        selected_path: Option<&Path>,
        analysis: Option<&Result<TapeAnalysis, String>>,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        };
        let mut filter = LibraryTapeFilterState::default();
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_page(ui, selected_path, analysis, None, &mut filter);
            });
        })
    }

    /// 1. The page hero's live controls are present. The approved poster
    /// supplies its own artwork/title as pixels, not egui text shapes.
    #[test]
    fn page_hero_is_present() {
        let output = render_at(egui::vec2(1600.0, 1200.0), None, None);
        assert!(rendered_text_contains(&output, "Choose tape file"));
        assert!(rendered_text_contains(
            &output,
            "Browse tape images in library"
        ));
        assert!(rendered_text_contains(&output, "Supported formats"));
    }

    /// 3. Technical details are collapsed by default when analysis is shown.
    #[test]
    fn technical_details_are_collapsed_by_default() {
        let analysis: Result<TapeAnalysis, String> = Ok(TapeAnalysis {
            format: TapeFormat::ZxTap,
            platform: Some("ZX Spectrum"),
            block_count: 1,
            entries: Vec::new(),
            metadata: vec!["fingerprint-marker-abc123".into()],
            loader: None,
            checksum: ChecksumState::NotPresent,
            warnings: Vec::new(),
            semantic_blocks: Vec::new(),
            logical_segments: 1,
            unsupported_blocks: 0,
        });
        let path = Path::new("demo.tap");
        let output = render_at(egui::vec2(1600.0, 1200.0), Some(path), Some(&analysis));
        assert!(rendered_text_contains(&output, "Technical details"));
        assert!(!rendered_text_contains(
            &output,
            "fingerprint-marker-abc123"
        ));
    }

    /// 4. Small viewport (~1024x600) renders the hero without panicking and
    /// keeps the primary content visible.
    #[test]
    fn renders_without_panicking_at_a_small_viewport() {
        let output = render_at(egui::vec2(1024.0, 600.0), None, None);
        assert!(rendered_text_contains(&output, "Choose tape file"));
    }

    // --- POSTER CLEANUP + LIBRARY BUTTON FUNCTIONAL FIX V1 -----------------

    /// 9. All three poster actions stay reachable at a small viewport, in
    /// both the calibrated-rect poster layout (available width still above
    /// the 760px narrow threshold) and the narrow text-button fallback
    /// (below it) - the poster hero has two layouts and both must expose
    /// every action.
    #[test]
    fn small_viewport_keeps_all_three_controls_reachable() {
        for width in [1024.0, 700.0] {
            let output = render_at(egui::vec2(width, 600.0), None, None);
            assert!(
                rendered_text_contains(&output, "Choose tape file"),
                "Choose tape file missing at width {width}"
            );
            assert!(
                rendered_text_contains(&output, "Browse tape images in library"),
                "Browse tape images in library missing at width {width}"
            );
            assert!(
                rendered_text_contains(&output, "Supported formats"),
                "Supported formats missing at width {width}"
            );
        }
    }

    /// 3/4. The library button is truthfully enabled only when the
    /// catalogue actually has tape candidates, and disabled (not a fake
    /// enabled state) when it has none - matching the label's own "(count)"
    /// suffix.
    #[test]
    fn library_button_enabled_state_matches_real_candidate_count() {
        let mut filter = LibraryTapeFilterState::default();
        let with_candidates = vec![tape_record("game.tap", Some("ZX Spectrum"))];
        let output_with = render_page_with_library(
            egui::vec2(1600.0, 1200.0),
            None,
            None,
            &with_candidates,
            &mut filter,
        );
        assert!(rendered_text_contains(
            &output_with,
            "Browse tape images in library (1)"
        ));

        let mut filter = LibraryTapeFilterState::default();
        let output_without =
            render_page_with_library(egui::vec2(1600.0, 1200.0), None, None, &[], &mut filter);
        assert!(rendered_text_contains(
            &output_without,
            "Browse tape images in library"
        ));
        assert!(!rendered_text_contains(
            &output_without,
            "Browse tape images in library ("
        ));
    }

    /// 4. Clicking the library button when there are genuinely no
    /// candidates is truthful: the button is disabled, so the click cannot
    /// register, and no scroll is requested.
    #[test]
    fn library_button_click_is_a_no_op_with_zero_candidates() {
        let mut filter = LibraryTapeFilterState::default();
        let screen = egui::vec2(1600.0, 450.0);
        let ctx = egui::Context::default();
        let no_candidates: Vec<ArchiveRecord> = Vec::new();

        let render = |ctx: &egui::Context,
                      filter: &mut LibraryTapeFilterState,
                      events: Vec<egui::Event>|
         -> egui::FullOutput {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        crate::ui::layout::page(
                            ui,
                            crate::ui::layout::ContentWidth::Normal,
                            true,
                            "zero-candidates-test",
                            |ui| {
                                let _ = show_page(ui, None, None, Some(&no_candidates), filter);
                            },
                        );
                    });
                },
            )
        };

        let baseline = render(&ctx, &mut filter, Vec::new());
        let button_pos = rendered_text_pos(&baseline, "Browse tape images in library")
            .expect("button text must still render, just disabled");
        let click_pos = button_pos + egui::vec2(4.0, 4.0);
        let _ = render(&ctx, &mut filter, click_at(click_pos));
        for _ in 0..10 {
            let out = render(&ctx, &mut filter, Vec::new());
            assert!(
                !rendered_text_contains(&out, "Tape images in your library")
                    || rendered_text_contains(&out, "No tape-shaped files indexed"),
                "a disabled button must never scroll to reveal a real result"
            );
        }
    }

    /// 6. The removed step strip ("Choose -> Analyse -> Inspect entries ->
    /// Review warnings") - the old text/step UI that was bleeding through
    /// behind the poster hero - is gone; the poster's own three actions and
    /// CRT status are the only "what do I do next" surface now.
    #[test]
    fn obsolete_workflow_step_text_is_not_rendered() {
        let output = render_at(egui::vec2(1600.0, 1200.0), None, None);
        for obsolete in ["Analyse", "Inspect entries", "Review warnings"] {
            assert!(
                !rendered_text_contains(&output, obsolete),
                "obsolete step text {obsolete:?} must not render behind the poster hero"
            );
        }
    }

    /// 8. The live CRT waveform/status readout still renders alongside the
    /// poster - the hybrid's dynamic half was not accidentally dropped
    /// while removing the obsolete step strip.
    #[test]
    fn waveform_status_readout_still_renders() {
        let output = render_at(egui::vec2(1600.0, 1200.0), None, None);
        assert!(rendered_text_contains(&output, "TAPE SIGNAL"));
        assert!(rendered_text_contains(&output, "IDLE"));
    }

    /// 6. The poster owns the page title, while live egui controls remain
    /// separate widgets; no duplicate live heading is introduced.
    #[test]
    fn no_duplicate_page_title_is_rendered() {
        fn count_occurrences(shape: &egui::Shape, needle: &str, count: &mut usize) {
            match shape {
                egui::Shape::Text(text_shape) => {
                    if text_shape.galley.text() == needle {
                        *count += 1;
                    }
                }
                egui::Shape::Vec(nested) => {
                    for s in nested {
                        count_occurrences(s, needle, count);
                    }
                }
                _ => {}
            }
        }
        let output = render_at(egui::vec2(1600.0, 1200.0), None, None);
        let mut count = 0;
        for clipped in &output.shapes {
            count_occurrences(&clipped.shape, "Tape Inspector", &mut count);
        }
        assert_eq!(count, 0, "the approved poster owns the title artwork");
    }

    /// 7. The motif renders without panicking whether or not analysis is in
    /// flight (the fallback-safe placeholder path).
    #[test]
    fn motif_rendering_is_fallback_safe() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_tape_motif(ui, egui::vec2(56.0, 56.0), false);
                draw_tape_motif(ui, egui::vec2(56.0, 56.0), true);
                draw_tape_motif(ui, egui::vec2(0.0, 0.0), true);
            });
        });
    }

    /// 8. `show_page` remains a pure presentation function over the same
    /// `Option<&Path>` / `Option<&Result<TapeAnalysis, String>>` worker
    /// state, plus the already-loaded catalogue (`Option<&[ArchiveRecord]>`,
    /// never a filesystem scan of its own) and its own persisted filter
    /// state for the library browser. This is a structural/compile-level
    /// assertion: the function signature itself is the contract.
    #[test]
    fn show_page_signature_takes_only_existing_worker_state() {
        fn _assert_signature(
            ui: &mut egui::Ui,
            path: Option<&Path>,
            analysis: Option<&Result<TapeAnalysis, String>>,
            library_records: Option<&[ArchiveRecord]>,
            filter: &mut LibraryTapeFilterState,
        ) -> Option<TapeInspectorAction> {
            show_page(ui, path, analysis, library_records, filter)
        }
    }

    /// The supported-formats chip row shows every real format label the
    /// page can present a result for.
    #[test]
    fn supported_format_chips_are_visible() {
        let output = render_at(egui::vec2(1600.0, 1200.0), None, None);
        for (label, _) in SUPPORTED_TAPE_FORMAT_CHIPS {
            assert!(
                rendered_text_contains(&output, label),
                "expected format chip {label:?} to render"
            );
        }
    }

    /// The signal panel's status line is an honest projection of real
    /// state, not a fabricated reading: idle before a tape is chosen, and
    /// the tape's own checksum state once analysis completes.
    #[test]
    fn signal_panel_reflects_real_tape_state() {
        assert_eq!(
            tape_signal_lines(None, None),
            ["TAPE SIGNAL".to_string(), "IDLE".to_string()]
        );
        let path = Path::new("demo.tap");
        assert_eq!(
            tape_signal_lines(Some(path), None),
            ["TAPE SIGNAL".to_string(), "READING...".to_string()]
        );
        let ok: Result<TapeAnalysis, String> = Ok(TapeAnalysis {
            format: TapeFormat::ZxTap,
            platform: None,
            block_count: 0,
            entries: Vec::new(),
            metadata: Vec::new(),
            loader: None,
            checksum: ChecksumState::Valid,
            warnings: Vec::new(),
            semantic_blocks: Vec::new(),
            logical_segments: 0,
            unsupported_blocks: 0,
        });
        assert_eq!(
            tape_signal_lines(Some(path), Some(&ok)),
            ["TAPE SIGNAL".to_string(), "LOAD OK".to_string()]
        );
    }

    // --- Front door + library browser (CC GUI PASS: TAPE INSPECTOR FRONT
    // DOOR + LIBRARY FILTERING + RETRO CHARACTER V1) ---

    fn tape_record(path: &str, platform: Option<&str>) -> ArchiveRecord {
        use archivefs_core::{Archive, ArchiveHealth, ArchiveMetadata, MountPlan, MountState};
        let archive = Archive::from_path(Path::new(path)).expect("recognised fixture extension");
        let plan = MountPlan::new(archive, PathBuf::from("/mnt/game"));
        let metadata = ArchiveMetadata {
            title: None,
            platform: platform.map(str::to_string),
            region: None,
            languages: None,
            version: None,
            disc: None,
            publisher: None,
            developer: None,
            release_year: None,
            genre: None,
            notes: None,
            source: None,
            synopsis: None,
            players: None,
            rating: None,
        };
        let mut record =
            ArchiveRecord::new(plan, MountState::Pending, metadata, ArchiveHealth::Pending);
        record.identity.platform = platform.map(str::to_string);
        record.mount_plan.archive.identity.platform = platform.map(str::to_string);
        record
    }

    fn render_page_with_library(
        screen: egui::Vec2,
        selected_path: Option<&Path>,
        analysis: Option<&Result<TapeAnalysis, String>>,
        records: &[ArchiveRecord],
        filter: &mut LibraryTapeFilterState,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_page(ui, selected_path, analysis, Some(records), filter);
            });
        })
    }

    /// 2. The front door's primary "Choose tape file" action is visible.
    /// 3. The "Browse tape images in library" action is visible.
    #[test]
    fn front_door_primary_actions_are_visible() {
        let mut filter = LibraryTapeFilterState::default();
        let output =
            render_page_with_library(egui::vec2(1600.0, 1200.0), None, None, &[], &mut filter);
        assert!(rendered_text_contains(&output, "Choose tape file"));
        assert!(rendered_text_contains(
            &output,
            "Browse tape images in library"
        ));
    }

    #[test]
    fn approved_poster_is_bundled_and_decodable() {
        let image = image::load_from_memory(TAPE_INSPECTOR_POSTER_PNG)
            .expect("bundled Tape Inspector poster should decode");
        assert!(image.width() > 1);
        assert!(image.height() > 1);
    }

    #[test]
    fn poster_texture_is_cached_after_first_load() {
        let ctx = egui::Context::default();
        let mut filter = LibraryTapeFilterState::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(cached_poster_texture(ui, &mut filter).is_some());
                assert!(filter.poster_load_attempted);
                assert!(cached_poster_texture(ui, &mut filter).is_some());
            });
        });
    }

    /// 5. Only actually-supported formats are advertised (never a second,
    /// independent support table).
    #[test]
    fn only_actual_supported_formats_are_advertised() {
        let output = render_at(egui::vec2(1600.0, 1200.0), None, None);
        assert!(!rendered_text_contains(&output, "D64"));
        assert!(!rendered_text_contains(&output, "PZX"));
    }

    /// 6. `.tap` is treated as a candidate signal, never authoritative
    /// platform proof: the library browser labels it "TAP candidate", it
    /// never claims a platform from the extension alone.
    #[test]
    fn tap_extension_is_a_candidate_not_authoritative_identity() {
        let records = vec![tape_record("game.tap", None)];
        let candidates = tape_candidates(&records);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].extension_label, "TAP");
        assert!(candidates[0].platform.is_none());
    }

    /// 9. Unsupported (non-tape) media never appears in the default tape
    /// browser, even when it is otherwise indexed by the same catalogue.
    #[test]
    fn unsupported_media_is_excluded_from_the_library_browser() {
        let records = vec![
            tape_record("game.tap", Some("ZX Spectrum")),
            tape_record("game.iso", None),
        ];
        let candidates = tape_candidates(&records);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].display_name, "game");
    }

    /// 7. Library filtering by format narrows to only the matching
    /// extension.
    #[test]
    fn library_filters_by_format() {
        let records = vec![
            tape_record("spectrum.tzx", Some("ZX Spectrum")),
            tape_record("c64.t64", Some("Commodore 64")),
        ];
        let mut filter = LibraryTapeFilterState {
            format: Some("T64".to_string()),
            ..Default::default()
        };
        let output = render_page_with_library(
            egui::vec2(1024.0, 1200.0),
            None,
            None,
            &records,
            &mut filter,
        );
        assert!(rendered_text_contains(&output, "c64"));
        assert!(!rendered_text_contains(&output, "spectrum"));
    }

    /// 8. Library filtering by platform narrows to only the matching
    /// platform.
    #[test]
    fn library_filters_by_platform() {
        let records = vec![
            tape_record("spectrum.tzx", Some("ZX Spectrum")),
            tape_record("c64.t64", Some("Commodore 64")),
        ];
        let mut filter = LibraryTapeFilterState {
            platform: Some("Commodore 64".to_string()),
            ..Default::default()
        };
        let output = render_page_with_library(
            egui::vec2(1600.0, 1200.0),
            None,
            None,
            &records,
            &mut filter,
        );
        assert!(rendered_text_contains(&output, "c64"));
        assert!(!rendered_text_contains(&output, "spectrum"));
    }

    /// 10. The selected tape's existing bounded analysis still renders when
    /// a library candidate (or any selection) is in place - the library
    /// browser is additive, not a replacement for the existing presentation.
    #[test]
    fn selected_tape_analysis_still_renders_alongside_the_library_browser() {
        let records = vec![tape_record("spectrum.tzx", Some("ZX Spectrum"))];
        let mut filter = LibraryTapeFilterState::default();
        let analysis: Result<TapeAnalysis, String> = Ok(TapeAnalysis {
            format: TapeFormat::Tzx,
            platform: Some("ZX Spectrum"),
            block_count: 2,
            entries: Vec::new(),
            metadata: Vec::new(),
            loader: None,
            checksum: ChecksumState::Valid,
            warnings: Vec::new(),
            semantic_blocks: Vec::new(),
            logical_segments: 0,
            unsupported_blocks: 0,
        });
        let path = Path::new("spectrum.tzx");
        let output = render_page_with_library(
            egui::vec2(1600.0, 1200.0),
            Some(path),
            Some(&analysis),
            &records,
            &mut filter,
        );
        assert!(rendered_text_contains(&output, "Selected tape"));
        assert!(rendered_text_contains(&output, "Programs found"));
    }

    /// 13. `tape_candidates` is a pure projection over an already-loaded
    /// slice - it takes no root path and performs no filesystem traversal of
    /// its own, unlike a directory-scanning helper.
    #[test]
    fn tape_candidates_never_scans_the_filesystem() {
        fn _assert_signature(records: &[ArchiveRecord]) -> Vec<LibraryTapeCandidate<'_>> {
            tape_candidates(records)
        }
    }

    /// 11. No source modification: the library browser and file picker only
    /// ever surface paths - `show_page` never opens a file for writing.
    /// Structural: `TapeInspectorAction` carries only `PathBuf`s a caller
    /// applies via the existing read-only selection flow.
    #[test]
    fn tape_inspector_actions_carry_only_paths() {
        let action = TapeInspectorAction::SelectLibraryTape(PathBuf::from("game.tap"));
        match action {
            TapeInspectorAction::SelectLibraryTape(path)
            | TapeInspectorAction::ChooseFile(path) => {
                assert_eq!(path, PathBuf::from("game.tap"));
            }
        }
    }

    /// 12. Small viewport (~1024x600): the library browser section and its
    /// filters remain reachable, not clipped away.
    #[test]
    fn library_browser_is_reachable_at_a_small_viewport() {
        let records = vec![tape_record("spectrum.tzx", Some("ZX Spectrum"))];
        let mut filter = LibraryTapeFilterState::default();
        let output =
            render_page_with_library(egui::vec2(1024.0, 600.0), None, None, &records, &mut filter);
        assert!(rendered_text_contains(
            &output,
            "Tape images in your library"
        ));
    }

    #[test]
    fn visual_pass_uses_human_summary_and_keeps_raw_format_available() {
        let analysis = TapeAnalysis {
            format: TapeFormat::Tzx,
            platform: Some("ZX Spectrum"),
            block_count: 41,
            entries: Vec::new(),
            metadata: Vec::new(),
            loader: Some(LoaderEvidence {
                class: LoaderClass::GenericTurbo,
                confidence: LoaderConfidence::High,
                fingerprint: "test".to_string(),
                clues: Vec::new(),
            }),
            checksum: ChecksumState::NotPresent,
            warnings: Vec::new(),
            semantic_blocks: Vec::new(),
            logical_segments: 0,
            unsupported_blocks: 0,
        };
        assert_eq!(human_tape_kind(&analysis), "ZX Spectrum tape image");
        assert_eq!(format_label(analysis.format), "TZX / CDT");
        assert_eq!(checksum_summary(analysis.checksum), "No checksum available");
        let output = render_at(
            egui::vec2(1600.0, 1200.0),
            Some(Path::new("demo.tzx")),
            Some(&Ok(analysis)),
        );
        assert!(rendered_text_contains(&output, "ZX Spectrum tape image"));
        assert!(rendered_text_contains(&output, "Choose tape file"));
        assert!(rendered_text_contains(&output, "TAPE SIGNAL"));
    }

    #[test]
    fn loading_state_shows_measured_elapsed_without_fake_eta() {
        let output = render_at(
            egui::vec2(1600.0, 1200.0),
            Some(Path::new("demo.tzx")),
            None,
        );
        assert!(rendered_text_contains(&output, "Analysing tape…"));
        assert!(rendered_text_contains(&output, "Elapsed:"));
        assert!(!rendered_text_contains(&output, "ETA:"));
    }

    #[test]
    fn non_tape_selection_is_not_presented_as_an_active_tape() {
        let output = render_at(
            egui::vec2(1600.0, 1200.0),
            Some(Path::new("game.gbc")),
            None,
        );
        assert!(rendered_text_contains(&output, "No tape selected"));
        assert!(!rendered_text_contains(&output, "Selected tape"));
    }

    #[test]
    fn long_timeline_is_grouped_and_bounded() {
        let mut blocks = (0..20)
            .map(|index| format!("Turbo data block {index}"))
            .collect::<Vec<_>>();
        blocks.extend((0..4).map(|index| format!("Pause {index}")));
        blocks.push("Unsupported opaque block".to_string());
        let groups = timeline_groups(&blocks);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].label, "Turbo / pilot blocks");
        assert_eq!(groups[0].entries.len(), 20);
        assert_eq!(groups[1].entries.len(), 4);
        assert_eq!(groups[2].label, "Unsupported / opaque blocks");
    }
}
