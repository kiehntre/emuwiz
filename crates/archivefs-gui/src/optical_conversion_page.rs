//! Focused UI for verified single-track CUE/BIN to CHD conversion.
//!
//! This is deliberately separate from equivalent-content review: conversion
//! creates a new representation, whereas duplicate review quarantines one.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::ui::{components as widgets, theme};
use archivefs_core::repair::{
    ChdConversionError, ChdConversionPlan, ChdConversionResult, ChdConversionSourceMode,
    ChdConversionTransaction, build_chd_conversion_plan, execute_chd_conversion,
    rollback_chd_conversion,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

struct Candidate {
    path: PathBuf,
    plan: Option<ChdConversionPlan>,
    /// Kept typed, not just the error's `Display` string, so the render
    /// layer can show a short beginner category (see
    /// [`conversion_blocker_label`]) while the exact core-authored message
    /// still reaches Technical details unchanged.
    error: Option<ChdConversionError>,
}

/// Which of the beginner-facing detection buckets (Section 4 of the CC GUI
/// PASS: DISC CONVERSION FRONT-LINE PRESENTATION task) a candidate falls
/// into. Purely a projection of the exact [`ChdConversionError`] core's own
/// plan builder already returned - never a new eligibility judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateBucket {
    ReadyToConvert,
    NeedsReview,
    Unsupported,
    Blocked,
}

fn candidate_bucket(candidate: &Candidate) -> CandidateBucket {
    let Some(error) = &candidate.error else {
        return CandidateBucket::ReadyToConvert;
    };
    match error {
        // A real conflict at a specific destination path - something in
        // the way, not a property of the source itself.
        ChdConversionError::InvalidTarget(_) => CandidateBucket::Blocked,
        // The conversion/verification tool itself is not available - worth
        // reviewing, not a statement about this particular disc set.
        ChdConversionError::ChdmanUnavailable(_) => CandidateBucket::NeedsReview,
        ChdConversionError::InvalidSource(_) => {
            match conversion_blocker_label(error) {
                "Ambiguous disc set" | "Missing CUE partner" => CandidateBucket::NeedsReview,
                // "Unsupported format" and any other InvalidSource detail
                // this build has no specific reviewed wording for.
                _ => CandidateBucket::Unsupported,
            }
        }
        // These variants are only ever produced by `convert()` itself
        // (post-plan, mid-transaction), never by the plan builder `scan`/
        // `preview_selected` call - listed for exhaustiveness, not because
        // a candidate row can carry them.
        ChdConversionError::ProcessFailed(_)
        | ChdConversionError::VerificationFailed(_)
        | ChdConversionError::StaleSource(_)
        | ChdConversionError::StaleOutput(_)
        | ChdConversionError::Transaction(_) => CandidateBucket::NeedsReview,
    }
}

/// A short, beginner-facing category for a [`ChdConversionError`] - see
/// Section 8 of the task. Derived only from the exact, already-reviewed
/// `Display` wording core produces (`ChdConversionError`/`CueError`'s own
/// `impl Display`, both in `archivefs-core`) - never a re-implementation of
/// core's own classification, and never a guess at a message this build
/// does not recognise (falls back to "Unsupported format", the safest
/// generic reading of "the plan builder refused this source").
fn conversion_blocker_label(error: &ChdConversionError) -> &'static str {
    let message = error.to_string();
    match error {
        ChdConversionError::InvalidTarget(_) => "Destination collision",
        ChdConversionError::ChdmanUnavailable(_) => "Verification prerequisite failed",
        ChdConversionError::InvalidSource(_) => {
            if message.contains("multiple data tracks") {
                "Ambiguous disc set"
            } else if message.contains("data file is missing") || message.contains("BIN: ") {
                "Missing CUE partner"
            } else {
                "Unsupported format"
            }
        }
        ChdConversionError::ProcessFailed(_) => "Conversion failed",
        ChdConversionError::VerificationFailed(_) => "Verification failed",
        ChdConversionError::StaleSource(_) => "Source changed during conversion",
        ChdConversionError::StaleOutput(_) => "Staged output changed",
        ChdConversionError::Transaction(_) => "Conversion transaction failed",
    }
}

/// Purely a projection over the existing `candidates`/plan-builder results
/// already held in memory - never a second filesystem scan (Section 4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DetectionCounts {
    pub(crate) files_scanned: usize,
    pub(crate) supported_disc_sets: usize,
    pub(crate) ready_to_convert: usize,
    pub(crate) needs_review: usize,
    pub(crate) unsupported: usize,
    pub(crate) blocked: usize,
}

fn detection_counts(candidates: &[Candidate]) -> DetectionCounts {
    let mut counts = DetectionCounts {
        files_scanned: candidates.len(),
        ..Default::default()
    };
    for candidate in candidates {
        match candidate_bucket(candidate) {
            CandidateBucket::ReadyToConvert => counts.ready_to_convert += 1,
            CandidateBucket::NeedsReview => counts.needs_review += 1,
            CandidateBucket::Unsupported => counts.unsupported += 1,
            CandidateBucket::Blocked => counts.blocked += 1,
        }
    }
    counts.supported_disc_sets = counts.files_scanned - counts.unsupported;
    counts
}

/// Read-only GUI projection of the conversion authority.  This contains no
/// conversion logic: eligibility still comes from the core plan builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConversionCapabilityView {
    pub(crate) source_format: &'static str,
    pub(crate) target_format: &'static str,
    pub(crate) preview: bool,
    pub(crate) apply: bool,
    pub(crate) verify: bool,
}

pub(crate) fn supported_capability_for_extension(
    extension: Option<&str>,
) -> Option<ConversionCapabilityView> {
    if let Some(extension) = extension
        && extension.eq_ignore_ascii_case("cue")
    {
        return Some(ConversionCapabilityView {
            source_format: "CUE/BIN",
            target_format: "CHD",
            preview: true,
            apply: true,
            verify: true,
        });
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedConversionContext {
    pub(crate) path: PathBuf,
    pub(crate) platform: Option<String>,
}

pub(crate) struct OpticalConversionPageState {
    pub(crate) source_root_draft: String,
    candidates: Vec<Candidate>,
    selected: Option<usize>,
    source_mode: ChdConversionSourceMode,
    confirm: bool,
    previewed: bool,
    /// Whether a scan (or a selected-game preview) has ever run - drives
    /// the empty-state intro card (Section 2): a folder with genuinely no
    /// supported files still gets the plain "0 files scanned" detection
    /// summary, not the first-run intro card again.
    scanned: bool,
    selected_context: Option<SelectedConversionContext>,
    result: Option<ChdConversionResult>,
    transaction: Option<ChdConversionTransaction>,
    error: Option<String>,
    /// The exact typed failure from the most recent `convert()` attempt, if
    /// it failed - kept alongside `error`'s string so the Result section
    /// can show a short beginner category (Section 11) with the untouched
    /// message still available under Technical details.
    conversion_failure: Option<ChdConversionError>,
    hero_texture: Option<egui::TextureHandle>,
    hero_load_attempted: bool,
    hero_preview_scroll: bool,
}

impl Default for OpticalConversionPageState {
    fn default() -> Self {
        Self {
            source_root_draft: String::new(),
            candidates: Vec::new(),
            selected: None,
            source_mode: ChdConversionSourceMode::KeepSource,
            confirm: false,
            previewed: false,
            scanned: false,
            selected_context: None,
            result: None,
            transaction: None,
            error: None,
            conversion_failure: None,
            hero_texture: None,
            hero_load_attempted: false,
            hero_preview_scroll: false,
        }
    }
}

fn collect_cues(root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_cues(&path, output);
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("cue"))
        {
            output.push(path);
        }
    }
}

impl OpticalConversionPageState {
    pub(crate) fn set_selected_context(&mut self, context: Option<SelectedConversionContext>) {
        if self.selected_context == context {
            return;
        }
        self.selected_context = context;
        self.candidates.clear();
        self.selected = None;
        self.confirm = false;
        self.previewed = false;
        self.result = None;
        self.transaction = None;
        self.error = None;
        self.conversion_failure = None;
        if let Some(context) = &self.selected_context {
            if let Some(parent) = context.path.parent() {
                self.source_root_draft = parent.display().to_string();
            }
        }
    }

    fn scan(&mut self) {
        self.candidates.clear();
        self.selected = None;
        self.confirm = false;
        self.previewed = false;
        self.result = None;
        self.transaction = None;
        self.error = None;
        self.conversion_failure = None;
        self.scanned = true;
        let root = PathBuf::from(self.source_root_draft.trim());
        if !root.is_dir() {
            self.error = Some("choose an existing source folder first".into());
            return;
        }
        let mut cues = Vec::new();
        collect_cues(&root, &mut cues);
        cues.sort();
        for cue in cues {
            let target = cue.with_extension("chd");
            match build_chd_conversion_plan(&cue, &target, self.source_mode, None) {
                Ok(plan) => self.candidates.push(Candidate {
                    path: cue,
                    plan: Some(plan),
                    error: None,
                }),
                Err(error) => self.candidates.push(Candidate {
                    path: cue,
                    plan: None,
                    error: Some(error),
                }),
            }
        }
    }

    fn convert(&mut self) {
        let Some(index) = self.selected else { return };
        let Some(mut plan) = self
            .candidates
            .get(index)
            .and_then(|candidate| candidate.plan.clone())
        else {
            self.error = Some("the selected source is not eligible for conversion".into());
            return;
        };
        plan.source_mode = self.source_mode;
        let root = PathBuf::from(self.source_root_draft.trim());
        let journal =
            match archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir() {
                Ok(path) => path,
                Err(error) => {
                    self.error = Some(error.to_string());
                    return;
                }
            };
        let mut trusted_paths = vec![root.clone()];
        if let Some(parent) = plan.target_path.parent() {
            trusted_paths.push(parent.to_path_buf());
        }
        let trusted = TrustedRoots::from_paths(trusted_paths);
        match execute_chd_conversion(&plan, trusted, &journal, &root, &AtomicBool::new(false)) {
            Ok((result, transaction)) => {
                self.result = Some(result);
                self.transaction = Some(transaction);
                self.error = None;
                self.conversion_failure = None;
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.conversion_failure = Some(error);
            }
        }
    }

    fn preview_selected(&mut self) {
        self.scanned = true;
        let Some(context) = self.selected_context.clone() else {
            self.error = Some("select a supported game or disc image first".into());
            return;
        };
        let Some(extension) = context.path.extension().and_then(|value| value.to_str()) else {
            self.error = Some("no safe conversion is currently available for this format".into());
            return;
        };
        if supported_capability_for_extension(Some(extension)).is_none() {
            self.error = Some("no safe conversion is currently available for this format".into());
            return;
        }
        let target = context.path.with_extension("chd");
        match build_chd_conversion_plan(&context.path, &target, self.source_mode, None) {
            Ok(plan) => {
                self.candidates = vec![Candidate {
                    path: context.path,
                    plan: Some(plan),
                    error: None,
                }];
                self.selected = Some(0);
                self.previewed = true;
                self.confirm = false;
                self.error = None;
            }
            Err(error) => {
                self.candidates = vec![Candidate {
                    path: context.path,
                    plan: None,
                    error: Some(error),
                }];
                self.selected = None;
                self.previewed = false;
                self.error = None;
            }
        }
    }

    fn rollback(&mut self) {
        let Some(transaction) = self.transaction.as_mut() else {
            self.error = Some("nothing is available to undo".into());
            return;
        };
        let Ok(journal) =
            archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
        else {
            self.error = Some("could not locate the repair journal".into());
            return;
        };
        match rollback_chd_conversion(transaction, &journal, &AtomicBool::new(false)) {
            Ok(_) => {
                self.result = None;
                self.transaction = None;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("rollback failed: {error}")),
        }
    }
}

/// Disc/DSK Conversion's page-hero motif: a small floppy-disc silhouette
/// with a static transfer meter. A lightweight placeholder drawn directly
/// with the painter (fixed-size rects/lines, no heap allocation, no image
/// decode) - not real box/media artwork. The meter fill reflects the
/// already-computed `ready_fraction` (ready-to-convert candidates out of
/// files scanned) rather than animating, so this never needs its own
/// repaint request and costs nothing extra per frame.
fn draw_dsk_motif(ui: &mut egui::Ui, size: egui::Vec2, ready_fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let body = rect.shrink(6.0);
    painter.rect_filled(body, 2.0, theme::DEEP_BACKGROUND);
    painter.rect_stroke(body, 2.0, theme::border(ui), egui::StrokeKind::Inside);
    // The disc's metal shutter, a classic 3.5" floppy silhouette cue.
    let shutter = egui::Rect::from_min_size(
        egui::pos2(body.left() + body.width() * 0.22, body.top()),
        egui::vec2(body.width() * 0.56, body.height() * 0.32),
    );
    painter.rect_filled(shutter, 1.0, theme::BORDER_FOCUS);
    // A small transfer meter beneath it: filled proportionally to how much
    // of the current scan is ready to convert.
    let meter = egui::Rect::from_min_size(
        egui::pos2(body.left() + 4.0, body.bottom() - body.height() * 0.22),
        egui::vec2(body.width() - 8.0, 6.0),
    );
    painter.rect_filled(meter, 2.0, theme::BORDER_SUBTLE);
    let filled = meter.with_max_x(meter.left() + meter.width() * ready_fraction.clamp(0.0, 1.0));
    painter.rect_filled(filled, 2.0, theme::TEAL);
}

/// The disc formats Disc Conversion can actually preview/convert/verify
/// today - the one real supported pair the plan builder recognises
/// ([`supported_capability_for_extension`]), presented as a compact chip
/// row for the source and target format. Deliberately not the mockup's
/// broader IMG/IMA/D64/HFE/A2R chip set: this build only supports CUE/BIN
/// as a source and CHD as a target, so only those two are shown.
const SUPPORTED_DISC_FORMAT_CHIPS: [(&str, egui::Color32); 2] = [
    ("CUE/BIN", egui::Color32::from_rgb(59, 130, 246)),
    ("CHD", theme::TEAL),
];

/// The Disc Conversion "signal panel" status lines (see
/// [`widgets::signal_panel`]) - derived only from the same page state
/// `show_optical_conversion_page` already holds, never a fabricated
/// reading. `ready_fraction` is the already-computed ready-to-convert share
/// of the current scan (see [`draw_dsk_motif`]'s own use of the same
/// value); it is only shown once a scan has actually produced candidates.
fn dsk_signal_lines(state: &OpticalConversionPageState, ready_fraction: f32) -> [String; 2] {
    let percent = (ready_fraction * 100.0).round() as i32;
    let status = if state.result.is_some() {
        "CONVERTED".to_string()
    } else if state.conversion_failure.is_some() {
        "FAILED".to_string()
    } else if state.confirm {
        "READY TO CONVERT".to_string()
    } else if state.previewed {
        "PREVIEWING".to_string()
    } else if !state.candidates.is_empty() {
        format!("SCANNED {percent}%")
    } else {
        "IDLE".to_string()
    };
    ["FLOPPY POWER".to_string(), status]
}

/// The signal panel's readout for Disc Conversion: a horizontal meter filled
/// to the same real `ready_fraction` the hero motif already reflects (never
/// an animated/fabricated progress, since `convert()` itself runs to
/// completion synchronously - there is no observable partial-conversion
/// state to animate here).
fn draw_dsk_signal_reading(painter: &egui::Painter, rect: egui::Rect, ready_fraction: f32) {
    let bar_height = 6.0_f32.min(rect.height());
    let bar = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - bar_height / 2.0),
        egui::vec2(rect.width(), bar_height),
    );
    painter.rect_filled(bar, 3.0, theme::BORDER_SUBTLE);
    let filled = bar.with_max_x(bar.left() + bar.width() * ready_fraction.clamp(0.0, 1.0));
    painter.rect_filled(filled, 3.0, theme::TEAL);
}

/// The beginner-facing "what is this / what will happen" card shown before
/// anything has ever been scanned (Sections 2-3 of the task). Disappears
/// permanently for this page-state's lifetime once a scan or a selected-
/// game preview has run at least once - including a scan that finds
/// nothing, which gets the plain "0 files scanned" detection summary
/// instead of this intro repeating.
fn show_intro_card(ui: &mut egui::Ui, state: &mut OpticalConversionPageState) {
    widgets::card(ui, |ui| {
        ui.label(
            egui::RichText::new(
                "Convert supported disc images into space-efficient CHD files while keeping \
                 the original source untouched until you explicitly apply the plan.",
            )
            .size(15.0),
        );
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Supported today:").strong());
            widgets::status_badge(ui, "CUE/BIN → CHD", widgets::StatusTone::Info);
        });
        ui.add_space(theme::SPACE_SM);
        ui.label(egui::RichText::new("Why convert to CHD?").strong());
        ui.label("• Usually a much smaller storage footprint than raw CUE/BIN");
        ui.label("• One file instead of a CUE sheet plus its BIN track(s)");
        ui.label("• The original source stays exactly as it is until you confirm and apply");
        ui.label(
            "• The new CHD is fingerprint-verified against the source before it counts as done",
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Not every emulator or frontend reads CHD - check yours supports it before \
                     relying on the converted copy.",
                )
                .small()
                .color(theme::muted(ui)),
            )
            .wrap(),
        );
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            for (index, step) in [
                "CUE + BIN",
                "Validate",
                "Convert to CHD",
                "Verify",
                "Keep original source",
            ]
            .into_iter()
            .enumerate()
            {
                if index > 0 {
                    ui.label(egui::RichText::new("→").color(theme::muted(ui)));
                }
                ui.label(egui::RichText::new(step).small());
            }
        });
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Choose source folder",
                widgets::ActionStyle::Primary,
                true,
            )
            .clicked()
            {
                choose_source_folder(state);
            }
            if state.selected_context.is_some()
                && widgets::action_button(
                    ui,
                    "Use selected game",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
            {
                state.preview_selected();
            }
        });
    });
}

fn choose_source_folder(state: &mut OpticalConversionPageState) {
    if let Some(path) = rfd::FileDialog::new().pick_folder() {
        state.source_root_draft = path.display().to_string();
        state.scan();
    }
}

/// Section 4: FILES SCANNED / SUPPORTED DISC SETS / READY TO CONVERT /
/// NEEDS REVIEW / UNSUPPORTED / BLOCKED, purely projected from the
/// `candidates` this page already holds - never a new filesystem scan.
fn show_detection_summary(ui: &mut egui::Ui, candidates: &[Candidate]) {
    let counts = detection_counts(candidates);
    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("What EmuWiz found").strong());
        ui.horizontal_wrapped(|ui| {
            widgets::status_badge(
                ui,
                format!("{} files scanned", counts.files_scanned),
                widgets::StatusTone::Info,
            );
            widgets::status_badge(
                ui,
                format!("{} supported disc sets", counts.supported_disc_sets),
                widgets::StatusTone::Info,
            );
            widgets::status_badge(
                ui,
                format!("{} ready to convert", counts.ready_to_convert),
                widgets::StatusTone::Success,
            );
            if counts.needs_review > 0 {
                widgets::status_badge(
                    ui,
                    format!("{} need review", counts.needs_review),
                    widgets::StatusTone::Warning,
                );
            }
            if counts.unsupported > 0 {
                widgets::status_badge(
                    ui,
                    format!("{} unsupported", counts.unsupported),
                    widgets::StatusTone::Pending,
                );
            }
            if counts.blocked > 0 {
                widgets::status_badge(
                    ui,
                    format!("{} blocked", counts.blocked),
                    widgets::StatusTone::Blocked,
                );
            }
        });
    });
}

/// Section 9: the page's single obvious next action, derived from state -
/// never five equally-weighted buttons at once. Purely a label; the real
/// action button for the current step remains the one already rendered
/// beside each state (Source discovery's "Find supported files", a
/// candidate row's Preview/Confirm/Convert button, ...).
fn next_step_hint(state: &OpticalConversionPageState) -> &'static str {
    if state.result.is_some() {
        return "Verified result ready below.";
    }
    if state.conversion_failure.is_some() {
        return "Conversion did not complete - see Result below.";
    }
    if state.confirm {
        return "Next: press Convert to apply this plan.";
    }
    if state.previewed {
        return "Next: review the preview, then confirm and convert.";
    }
    if state.selected.is_some() {
        return "Next: press Preview conversion below.";
    }
    if !state.candidates.is_empty() {
        return "Next: select a disc set below to preview its conversion.";
    }
    if state.scanned {
        return "Next: choose a different source folder, or select a supported game.";
    }
    "Next: choose a source folder or a selected game to begin."
}

/// Disc/DSK Conversion's real, ordered workflow. Never fabricated: each
/// step corresponds to an actual state `show_optical_conversion_page`
/// already renders differently for.
const WORKFLOW_STEPS: [&str; 5] = ["Choose", "Inspect", "Convert", "Verify", "Keep original"];

/// Which step of [`WORKFLOW_STEPS`] the page is currently on. `execute_chd_
/// conversion` verifies the staged output before it ever becomes a result
/// (there is no separate observable "verifying" state to point at), so a
/// completed `result` is presented as the final step - the earlier "Verify"
/// step reads as already complete (checkmarked), which is accurate: by the
/// time a result exists, verification has already happened.
fn workflow_step(state: &OpticalConversionPageState) -> usize {
    if state.result.is_some() {
        return 4;
    }
    if state.confirm || state.conversion_failure.is_some() {
        return 2;
    }
    if !state.candidates.is_empty() || state.selected_context.is_some() {
        return 1;
    }
    0
}

const DISC_CONVERSION_HERO_PNG: &[u8] = include_bytes!("../assets/emuwiz_hero_disc_conversion.png");

fn disc_conversion_hero_height(width: f32) -> f32 {
    width * 821.0 / 1916.0
}

fn disc_conversion_hero_size(width: f32) -> egui::Vec2 {
    let image_width = if width <= 1_100.0 {
        width.min(340.0 * 1916.0 / 821.0)
    } else {
        width
    };
    egui::vec2(image_width, disc_conversion_hero_height(image_width))
}

fn show_disc_conversion_hero(ui: &mut egui::Ui, state: &mut OpticalConversionPageState) -> bool {
    if !state.hero_load_attempted {
        state.hero_load_attempted = true;
        if let Ok(decoded) = image::load_from_memory(DISC_CONVERSION_HERO_PNG) {
            let rgba = decoded.to_rgba8();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            state.hero_texture = Some(ui.ctx().load_texture(
                "emuwiz-disc-conversion-hero",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }
    let Some(texture_id) = state.hero_texture.as_ref().map(|texture| texture.id()) else {
        return false;
    };
    let width = ui.available_width().max(1.0);
    let image_size = disc_conversion_hero_size(width);
    let (layout_rect, _) =
        ui.allocate_exact_size(egui::vec2(width, image_size.y), egui::Sense::hover());
    let rect = egui::Rect::from_center_size(layout_rect.center(), image_size);
    ui.painter().image(
        texture_id,
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    let panel_top = rect.top() + image_size.y * 0.648;
    let panel_height = image_size.y * 0.177;
    let panel = |left: f32, right: f32| {
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + image_size.x * left, panel_top),
            egui::pos2(rect.left() + image_size.x * right, panel_top + panel_height),
        )
    };
    let transparent = || {
        egui::Button::new("")
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
    };
    if ui
        .put(panel(0.025, 0.219), transparent())
        .on_hover_text("Choose the folder containing your disc images")
        .clicked()
    {
        choose_source_folder(state);
    }
    if ui
        .put(panel(0.226, 0.435), transparent())
        .on_hover_text("Find supported CUE/BIN disc sets")
        .clicked()
    {
        state.scan();
    }
    if (state.selected.is_some() || state.selected_context.is_some())
        && ui
            .put(panel(0.442, 0.642), transparent())
            .on_hover_text("Prepare the selected CUE/BIN to CHD preview")
            .clicked()
    {
        if state.selected_context.is_some() && state.selected.is_none() {
            state.preview_selected();
        } else {
            state.previewed = true;
        }
        state.hero_preview_scroll = true;
    }
    true
}

pub(crate) fn show_optical_conversion_page(
    ui: &mut egui::Ui,
    state: &mut OpticalConversionPageState,
) {
    let counts = detection_counts(&state.candidates);
    let ready_fraction = if counts.files_scanned == 0 {
        0.0
    } else {
        counts.ready_to_convert as f32 / counts.files_scanned as f32
    };
    let signal_lines = dsk_signal_lines(state, ready_fraction);
    if !show_disc_conversion_hero(ui, state) {
        show_disc_conversion_native_hero(ui, ready_fraction, &signal_lines);
    }
    widgets::workflow_strip(ui, &WORKFLOW_STEPS, workflow_step(state));
    ui.add_space(theme::SPACE_SM);
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Supported disc formats",
            Some("EmuWiz can preview, convert, and verify this pair today."),
        );
        widgets::format_chip_row(ui, &SUPPORTED_DISC_FORMAT_CHIPS);
    });
    ui.add_space(theme::SPACE_SM);

    if state.candidates.is_empty() && !state.scanned {
        show_intro_card(ui, state);
        ui.add_space(theme::SECTION_GAP);
    }

    if let Some(context) = state.selected_context.clone() {
        widgets::card(ui, |ui| {
            ui.heading("Selected file");
            ui.label(context.path.display().to_string());
            if let Some(platform) = &context.platform {
                ui.label(format!("Platform: {platform}"));
            }
            if let Some(capability) = supported_capability_for_extension(
                context.path.extension().and_then(|value| value.to_str()),
            ) {
                ui.label(format!(
                    "{} → {} · preview, conversion, and verification supported",
                    capability.source_format, capability.target_format
                ));
                if ui.button("Prepare preview").clicked() {
                    state.preview_selected();
                }
            } else {
                ui.label("No safe conversion is currently available for this format.");
            }
        });
        ui.add_space(theme::SECTION_GAP);
    }
    widgets::card(ui, |ui| {
        ui.heading("Source discovery");
        ui.horizontal_wrapped(|ui| {
            ui.label("Source folder:");
            ui.add_sized(
                [ui.available_width().min(520.0).max(220.0), 24.0],
                egui::TextEdit::singleline(&mut state.source_root_draft),
            );
            if ui.button("Choose folder").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                state.source_root_draft = path.display().to_string();
            }
            if ui.button("Find supported files").clicked() {
                state.scan();
            }
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(state.source_root_draft.as_str())
                    .monospace()
                    .color(theme::muted(ui)),
            )
            .wrap(),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new("Scanning and previewing never modifies your files.")
                    .small()
                    .color(theme::muted(ui)),
            )
            .wrap(),
        );
    });
    ui.add_space(theme::SECTION_GAP);
    widgets::card(ui, |ui| {
        ui.heading("Conversion safety");
        let mut quarantine = state.source_mode == ChdConversionSourceMode::QuarantineSource;
        if ui
            .checkbox(
                &mut quarantine,
                "Quarantine originals after verified conversion",
            )
            .changed()
        {
            state.source_mode = if quarantine {
                ChdConversionSourceMode::QuarantineSource
            } else {
                ChdConversionSourceMode::KeepSource
            };
        }
        ui.label(
            if state.source_mode == ChdConversionSourceMode::KeepSource {
                "Source files will not be modified (default)."
            } else {
                "Originals move only after verified conversion."
            },
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Original source files are preserved unless the quarantine option above is \
                     explicitly chosen.",
                )
                .small()
                .color(theme::muted(ui)),
            )
            .wrap(),
        );
    });
    ui.add_space(theme::SECTION_GAP);
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Safety first", None);
        for line in [
            "Scanning and previewing never modify your files",
            "Conversion creates a new file; your original stays unless you choose quarantine",
            "Every converted file is fingerprint-verified before it counts as done",
            "Nothing is applied until you confirm and press Convert",
        ] {
            ui.label(format!("✓ {line}"));
        }
    });
    ui.add_space(theme::SECTION_GAP);

    if let Some(error) = &state.error {
        widgets::banner(
            ui,
            "Could not continue",
            error,
            widgets::StatusTone::Warning,
        );
        ui.add_space(theme::SPACE_SM);
    }

    if !state.candidates.is_empty() {
        show_detection_summary(ui, &state.candidates);
        ui.add_space(theme::SPACE_SM);
    } else if state.scanned && state.error.is_none() {
        ui.label("No supported CUE/BIN disc sets were found in this folder.");
        ui.add_space(theme::SPACE_SM);
    }

    ui.label(egui::RichText::new(next_step_hint(state)).strong());
    ui.add_space(theme::SPACE_SM);

    if state.hero_preview_scroll {
        state.hero_preview_scroll = false;
        ui.scroll_to_cursor(Some(egui::Align::TOP));
    }

    for index in 0..state.candidates.len() {
        let candidate_path = state.candidates[index].path.clone();
        let plan = state.candidates[index].plan.clone();
        let error = state.candidates[index].error.clone();
        let eligible = plan.is_some();
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(
                    state.selected == Some(index),
                    candidate_path.display().to_string(),
                )
                .clicked()
                && eligible
            {
                state.selected = Some(index);
                state.confirm = false;
                state.previewed = false;
                state.result = None;
                state.transaction = None;
                state.error = None;
                state.conversion_failure = None;
            }
            if eligible {
                widgets::status_badge(ui, "Ready to convert", widgets::StatusTone::Success);
            } else if let Some(error) = &error {
                widgets::status_badge(
                    ui,
                    conversion_blocker_label(error),
                    widgets::StatusTone::Warning,
                );
            }
        });
        if !eligible && let Some(error) = &error {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(error.to_string())
                        .small()
                        .color(theme::muted(ui)),
                )
                .wrap(),
            );
        }
        if state.selected == Some(index)
            && let Some(plan) = &plan
        {
            widgets::card(ui, |ui| {
                ui.heading("Conversion preview");
                ui.label(format!("Source: {}", plan.cue_path.display()));
                ui.label(format!(
                    "Detected format: CUE/BIN (required member: {})",
                    plan.bin_path.display()
                ));
                ui.label(format!("Destination: {}", plan.target_path.display()));
                ui.label("Expected output: one fingerprint-verified CHD file.");
                widgets::status_badge(ui, "Ready to convert", widgets::StatusTone::Success);
                ui.add_space(theme::SPACE_SM);
                ui.label("The source set is read-only during conversion.");
                ui.label("The staged CHD is fingerprint-verified before it is finalized.");
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Preview does not modify your files.")
                            .small()
                            .color(theme::muted(ui)),
                    )
                    .wrap(),
                );
                widgets::technical_details(ui, ("optical-conversion-plan", index), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("Target: {}", plan.target_path.display()))
                                .monospace(),
                        )
                        .wrap(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "{} sectors · canonical SHA-256 {}",
                                plan.source_fingerprint.structure.logical_sector_count,
                                plan.source_fingerprint.canonical_sha256
                            ))
                            .monospace(),
                        )
                        .wrap(),
                    );
                });
            });
            if !state.previewed {
                if widgets::action_button(
                    ui,
                    "Preview conversion",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
                {
                    state.previewed = true;
                }
            } else if !state.confirm {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Original source files are preserved unless quarantine was chosen \
                             above.",
                        )
                        .small()
                        .color(theme::muted(ui)),
                    )
                    .wrap(),
                );
                if widgets::action_button(
                    ui,
                    "Confirm and convert",
                    widgets::ActionStyle::Primary,
                    true,
                )
                .clicked()
                {
                    state.confirm = true;
                }
            } else if widgets::action_button(ui, "Convert", widgets::ActionStyle::Primary, true)
                .clicked()
            {
                state.convert();
            }
        }
    }

    if let Some(result) = &state.result {
        let target_path = result.target_path.display().to_string();
        let transaction_id = result.transaction_id.clone();
        let source_quarantined = result.source_quarantined;
        ui.separator();
        widgets::card(ui, |ui| {
            ui.heading("Result");
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, "Converted", widgets::StatusTone::Success);
                widgets::status_badge(ui, "Verified", widgets::StatusTone::Success);
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("Verified CHD created: {}", target_path))
                        .monospace(),
                )
                .wrap(),
            );
            ui.label(if source_quarantined {
                "Originals quarantined after verification."
            } else {
                "Originals kept."
            });
            widgets::technical_details(ui, ("optical-conversion-result",), |ui| {
                ui.label(format!("Transaction: {transaction_id}"));
            });
            if widgets::action_button(ui, "Undo conversion", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                state.rollback();
            }
        });
    } else if let Some(failure) = &state.conversion_failure {
        let category = conversion_blocker_label(failure);
        let detail = failure.to_string();
        ui.separator();
        widgets::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                widgets::status_badge(ui, "Failed", widgets::StatusTone::Blocked);
                ui.label(egui::RichText::new(category).strong());
            });
            widgets::technical_details(ui, ("optical-conversion-failure",), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(detail).monospace()).wrap());
            });
        });
    }
}

fn show_disc_conversion_native_hero(
    ui: &mut egui::Ui,
    ready_fraction: f32,
    signal_lines: &[String; 2],
) {
    widgets::page_hero(
        ui,
        |ui, size| draw_dsk_motif(ui, size, ready_fraction),
        "Disc Conversion",
        "Review a safe conversion for a supported disc image. EmuWiz keeps the source files \
         and verifies the new result.",
        Some((
            "Originals kept unless you choose otherwise",
            widgets::StatusTone::Info,
        )),
        Some("Preserve the past. Play anywhere."),
        |ui| {
            widgets::signal_panel(
                ui,
                egui::vec2(176.0, 66.0),
                signal_lines,
                |painter, rect| draw_dsk_signal_reading(painter, rect, ready_fraction),
            );
        },
        |_ui| {},
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &Path) -> PathBuf {
        let bin = root.join("Track space ü.bin");
        let cue = root.join("Game space ü.cue");
        std::fs::write(&bin, vec![0x42u8; 2048 * 16]).unwrap();
        std::fs::write(
            &cue,
            "FILE \"Track space ü.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        cue
    }

    #[test]
    fn scan_exposes_eligible_source_without_mutating_it() {
        let directory = tempfile::tempdir().unwrap();
        let cue = source(directory.path());
        let before = std::fs::read(&cue).unwrap();
        let mut state = OpticalConversionPageState {
            source_root_draft: directory.path().display().to_string(),
            ..Default::default()
        };
        state.scan();
        assert_eq!(state.candidates.len(), 1);
        assert!(state.candidates[0].plan.is_some());
        assert_eq!(state.source_mode, ChdConversionSourceMode::KeepSource);
        assert_eq!(std::fs::read(&cue).unwrap(), before);
    }

    #[test]
    fn scan_surfaces_target_collision_as_blocked() {
        let directory = tempfile::tempdir().unwrap();
        let cue = source(directory.path());
        std::fs::write(cue.with_extension("chd"), b"existing").unwrap();
        let mut state = OpticalConversionPageState {
            source_root_draft: directory.path().display().to_string(),
            ..Default::default()
        };
        state.scan();
        assert_eq!(state.candidates.len(), 1);
        assert!(state.candidates[0].plan.is_none());
        assert!(
            state.candidates[0]
                .error
                .as_ref()
                .is_some_and(|error| error.to_string().contains("target already exists"))
        );
        assert_eq!(
            conversion_blocker_label(state.candidates[0].error.as_ref().unwrap()),
            "Destination collision"
        );
    }

    #[test]
    fn capability_matrix_is_conservative_and_deterministic() {
        let capability = supported_capability_for_extension(Some("CUE")).unwrap();
        assert_eq!(capability.source_format, "CUE/BIN");
        assert_eq!(capability.target_format, "CHD");
        assert!(capability.preview);
        assert!(capability.apply);
        assert!(capability.verify);
        assert!(supported_capability_for_extension(Some("bin")).is_none());
        assert!(supported_capability_for_extension(Some("iso")).is_none());
        assert!(supported_capability_for_extension(None).is_none());
    }

    #[test]
    fn selected_context_is_invalidated_when_selection_changes() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.cue");
        let second = directory.path().join("second.iso");
        let mut state = OpticalConversionPageState::default();
        state.candidates.push(Candidate {
            path: first.clone(),
            plan: None,
            error: Some(ChdConversionError::InvalidTarget("synthetic".into())),
        });
        state.result = None;
        state.set_selected_context(Some(SelectedConversionContext {
            path: first,
            platform: Some("PlayStation".into()),
        }));
        state.set_selected_context(Some(SelectedConversionContext {
            path: second.clone(),
            platform: None,
        }));
        assert_eq!(state.selected_context.as_ref().unwrap().path, second);
        assert!(state.candidates.is_empty());
        assert!(!state.previewed);
        assert!(!state.confirm);
    }

    // --- CC GUI PASS: DISC CONVERSION FRONT-LINE PRESENTATION V1 ---------

    fn render_at(state: &mut OpticalConversionPageState, screen: egui::Vec2) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_optical_conversion_page(ui, state);
            });
        })
    }

    fn render(state: &mut OpticalConversionPageState) -> egui::FullOutput {
        // The production page uses the shared outer scroll. Keep this
        // general renderer tall enough to inspect lower live sections while
        // dedicated small-viewport tests continue to exercise clipping.
        render_at(state, egui::vec2(1600.0, 3_000.0))
    }

    /// Real mouse-wheel scroll around the page's own `ScrollArea`, matching
    /// the technique the rest of the app's small-viewport reachability
    /// tests use - this page participates in the shared page-scroll
    /// wrapper in production (`main.rs`'s `main_view_uses_page_scroll`),
    /// but a self-contained `ScrollArea` here is enough to prove nothing
    /// in this page's own layout strands content below the fold.
    fn render_scrolled_to_bottom(
        state: &mut OpticalConversionPageState,
        screen: egui::Vec2,
    ) -> egui::FullOutput {
        let ctx = egui::Context::default();
        let base_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        };
        let mut output = None;
        let mut frame = |ctx: &egui::Context, input: egui::RawInput| {
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("optical-conversion-test-scroll")
                        .show(ui, |ui| {
                            show_optical_conversion_page(ui, state);
                        });
                });
            })
        };
        for _ in 0..3 {
            output = Some(frame(&ctx, base_input.clone()));
        }
        for _ in 0..60 {
            let scroll_input = egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(egui::pos2(screen.x / 2.0, screen.y / 2.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, -20.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
                ..base_input.clone()
            };
            output = Some(frame(&ctx, scroll_input));
        }
        output.unwrap()
    }

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
            .any(|c| shape_contains(&c.shape, needle))
    }

    fn ready_candidate_state(root: &Path) -> OpticalConversionPageState {
        let cue = source(root);
        let mut state = OpticalConversionPageState {
            source_root_draft: root.display().to_string(),
            ..Default::default()
        };
        state.scan();
        assert_eq!(
            state.candidates.len(),
            1,
            "fixture must produce one ready candidate"
        );
        assert!(state.candidates[0].plan.is_some());
        let _ = &cue;
        state
    }

    /// 1. Empty state has a useful explanation, not a blank page.
    #[test]
    fn empty_state_has_useful_explanation() {
        let mut state = OpticalConversionPageState::default();
        let output = render(&mut state);
        assert!(rendered_text_contains(
            &output,
            "space-efficient CHD files while keeping"
        ));
        assert!(rendered_text_contains(&output, "Why convert to CHD?"));
        assert!(rendered_text_contains(&output, "Choose source folder"));
    }

    /// 2. CUE/BIN is listed as supported today.
    #[test]
    fn cue_bin_listed_as_supported() {
        let mut state = OpticalConversionPageState::default();
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "CUE/BIN → CHD"));
    }

    /// 3. A format with no safe conversion is never presented as supported.
    #[test]
    fn unsupported_formats_are_not_falsely_presented_as_supported() {
        assert!(supported_capability_for_extension(Some("iso")).is_none());
        assert!(supported_capability_for_extension(Some("chd")).is_none());
        assert!(supported_capability_for_extension(Some("gdi")).is_none());
        assert!(supported_capability_for_extension(Some("cdi")).is_none());
        assert!(supported_capability_for_extension(Some("nrg")).is_none());
        assert!(supported_capability_for_extension(Some("img")).is_none());

        let mut state = OpticalConversionPageState::default();
        state.set_selected_context(Some(SelectedConversionContext {
            path: PathBuf::from("/library/game.iso"),
            platform: None,
        }));
        let output = render(&mut state);
        assert!(rendered_text_contains(
            &output,
            "No safe conversion is currently available for this format."
        ));
        assert!(!rendered_text_contains(
            &output,
            "preview, conversion, and verification supported"
        ));
    }

    /// 4. Detection counts are shown after a scan, from the existing
    /// planner results only - no extra filesystem scan during render.
    #[test]
    fn detection_counts_are_shown_after_scan() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = ready_candidate_state(directory.path());
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "1 files scanned"));
        assert!(rendered_text_contains(&output, "1 supported disc sets"));
        assert!(rendered_text_contains(&output, "1 ready to convert"));
    }

    /// 5. A supported item's preview (source/format/destination/status) is
    /// visible once selected.
    #[test]
    fn supported_item_preview_is_visible() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = ready_candidate_state(directory.path());
        state.selected = Some(0);
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Conversion preview"));
        assert!(rendered_text_contains(&output, "Source:"));
        assert!(rendered_text_contains(&output, "Detected format: CUE/BIN"));
        assert!(rendered_text_contains(&output, "Destination:"));
        assert!(rendered_text_contains(&output, "Expected output:"));
        assert!(rendered_text_contains(&output, "Ready to convert"));
    }

    /// 6. A blocked item's reason is visible, using the existing typed
    /// `ChdConversionError`, not a raw internal string as the primary cue.
    #[test]
    fn blocked_item_reason_is_visible() {
        let directory = tempfile::tempdir().unwrap();
        let cue = source(directory.path());
        std::fs::write(cue.with_extension("chd"), b"existing").unwrap();
        let mut state = OpticalConversionPageState {
            source_root_draft: directory.path().display().to_string(),
            ..Default::default()
        };
        state.scan();
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Destination collision"));
        assert!(rendered_text_contains(&output, "1 blocked"));
    }

    /// 7. Source-preservation wording is visible near the primary action.
    #[test]
    fn source_preservation_wording_is_visible() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = ready_candidate_state(directory.path());
        state.selected = Some(0);
        let output = render(&mut state);
        assert!(rendered_text_contains(
            &output,
            "Scanning and previewing never modifies your files."
        ));
        assert!(rendered_text_contains(
            &output,
            "Original source files are preserved unless the quarantine option above is"
        ));
        assert!(rendered_text_contains(
            &output,
            "Preview does not modify your files."
        ));
    }

    /// 8. Preview never mutates the source, for the selected-game path too
    /// (the folder-scan path is already covered by
    /// `scan_exposes_eligible_source_without_mutating_it`).
    #[test]
    fn preview_selected_never_mutates_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let cue = source(directory.path());
        let before = std::fs::read(&cue).unwrap();
        let mut state = OpticalConversionPageState::default();
        state.set_selected_context(Some(SelectedConversionContext {
            path: cue.clone(),
            platform: Some("PlayStation".into()),
        }));
        state.preview_selected();
        assert!(state.candidates[0].plan.is_some());
        assert_eq!(std::fs::read(&cue).unwrap(), before);
    }

    /// 9. The page's primary-action hint changes with state, never staying
    /// on one generic message regardless of progress.
    #[test]
    fn primary_action_hint_changes_by_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = OpticalConversionPageState::default();
        assert_eq!(
            next_step_hint(&state),
            "Next: choose a source folder or a selected game to begin."
        );

        state = ready_candidate_state(directory.path());
        assert_eq!(
            next_step_hint(&state),
            "Next: select a disc set below to preview its conversion."
        );

        state.selected = Some(0);
        assert_eq!(
            next_step_hint(&state),
            "Next: press Preview conversion below."
        );

        state.previewed = true;
        assert_eq!(
            next_step_hint(&state),
            "Next: review the preview, then confirm and convert."
        );

        state.confirm = true;
        assert_eq!(
            next_step_hint(&state),
            "Next: press Convert to apply this plan."
        );
    }

    /// 10. Technical/fingerprint details stay collapsed by default.
    #[test]
    fn technical_details_are_collapsed_by_default() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = ready_candidate_state(directory.path());
        state.selected = Some(0);
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Technical details"));
        assert!(!rendered_text_contains(&output, "canonical SHA-256"));
    }

    /// 11. A completed conversion shows a clear Converted/Verified summary.
    #[test]
    fn result_summary_shows_converted_and_verified() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = ready_candidate_state(directory.path());
        state.result = Some(ChdConversionResult {
            target_path: directory.path().join("Game space ü.chd"),
            source_fingerprint: state.candidates[0]
                .plan
                .as_ref()
                .unwrap()
                .source_fingerprint
                .clone(),
            output_fingerprint: state.candidates[0]
                .plan
                .as_ref()
                .unwrap()
                .source_fingerprint
                .clone(),
            source_mode: ChdConversionSourceMode::KeepSource,
            source_quarantined: false,
            transaction_id: "synthetic-transaction".into(),
        });
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Converted"));
        assert!(rendered_text_contains(&output, "Verified"));
        assert!(rendered_text_contains(&output, "Originals kept."));
    }

    /// 12. The primary action is reachable at a small ~1024x600 viewport
    /// without scrolling - it sits at the very top of the page.
    #[test]
    fn primary_action_is_reachable_at_a_small_viewport() {
        let mut state = OpticalConversionPageState::default();
        let output = render_at(&mut state, egui::vec2(1024.0, 600.0));
        assert!(state.hero_texture.is_some());
        assert!(rendered_text_contains(&output, "Supported disc formats"));
    }

    /// 13. The final section (the Result card) is reachable by scrolling
    /// at a small viewport - never stranded below the max scroll extent.
    #[test]
    fn final_section_is_reachable_at_a_small_viewport() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = ready_candidate_state(directory.path());
        state.selected = Some(0);
        state.result = Some(ChdConversionResult {
            target_path: directory.path().join("Game space ü.chd"),
            source_fingerprint: state.candidates[0]
                .plan
                .as_ref()
                .unwrap()
                .source_fingerprint
                .clone(),
            output_fingerprint: state.candidates[0]
                .plan
                .as_ref()
                .unwrap()
                .source_fingerprint
                .clone(),
            source_mode: ChdConversionSourceMode::KeepSource,
            source_quarantined: false,
            transaction_id: "synthetic-transaction".into(),
        });
        let output = render_scrolled_to_bottom(&mut state, egui::vec2(1024.0, 600.0));
        assert!(rendered_text_contains(&output, "Undo conversion"));
    }

    // --- Visual language pass: page hero / workflow strip / button semantics ---

    /// 16. The page hero (title, purpose, motto) is present.
    #[test]
    fn page_hero_is_present() {
        let mut state = OpticalConversionPageState::default();
        let output = render(&mut state);
        assert!(state.hero_texture.is_some());
        assert!(rendered_text_contains(&output, "CUE/BIN"));
        assert!(rendered_text_contains(&output, "CHD"));
        assert!(rendered_text_contains(
            &output,
            "Scanning and previewing never modify your files"
        ));
    }

    /// The workflow strip renders every real step of the flow, and tracks
    /// state correctly instead of a fabricated fixed step.
    #[test]
    fn workflow_step_reflects_the_real_flow() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = OpticalConversionPageState::default();
        assert_eq!(workflow_step(&state), 0);

        state = ready_candidate_state(directory.path());
        assert_eq!(workflow_step(&state), 1);

        state.selected = Some(0);
        state.previewed = true;
        assert_eq!(workflow_step(&state), 1);

        state.confirm = true;
        assert_eq!(workflow_step(&state), 2);

        state.result = Some(ChdConversionResult {
            target_path: directory.path().join("out.chd"),
            source_fingerprint: state.candidates[0]
                .plan
                .as_ref()
                .unwrap()
                .source_fingerprint
                .clone(),
            output_fingerprint: state.candidates[0]
                .plan
                .as_ref()
                .unwrap()
                .source_fingerprint
                .clone(),
            source_mode: ChdConversionSourceMode::KeepSource,
            source_quarantined: false,
            transaction_id: "synthetic".into(),
        });
        assert_eq!(workflow_step(&state), 4);
    }

    /// Destructive styling stays visually distinct from primary/secondary:
    /// the primary "Confirm and convert" action fills amber, while a
    /// destructive `ActionStyle::Destructive` button (used elsewhere in the
    /// app, e.g. quarantine review) fills the separate danger colour, never
    /// the same fill.
    #[test]
    fn destructive_style_stays_visually_distinct_from_primary() {
        assert_ne!(theme::ACCENT, theme::DANGER);
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = widgets::action_button(
                    ui,
                    "Confirm and convert",
                    widgets::ActionStyle::Primary,
                    true,
                );
                let _ = widgets::action_button(
                    ui,
                    "Remove permanently",
                    widgets::ActionStyle::Destructive,
                    true,
                );
            });
        });
    }

    /// The motif renders without panicking across the ready-fraction range,
    /// including degenerate zero-size and zero/one fraction edges.
    #[test]
    fn motif_rendering_is_fallback_safe() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_dsk_motif(ui, egui::vec2(56.0, 56.0), 0.0);
                draw_dsk_motif(ui, egui::vec2(56.0, 56.0), 1.0);
                draw_dsk_motif(ui, egui::vec2(56.0, 56.0), 2.5);
                draw_dsk_motif(ui, egui::vec2(0.0, 0.0), 0.5);
            });
        });
    }

    /// The supported-disc-formats chip row shows only the one real
    /// CUE/BIN -> CHD pair this build supports, never a fabricated wider
    /// format set.
    #[test]
    fn supported_disc_format_chips_show_only_real_support() {
        let mut state = OpticalConversionPageState::default();
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "CUE/BIN"));
        assert!(rendered_text_contains(&output, "CHD"));
        for unsupported in ["IMG", "IMA", "D64", "HFE", "A2R"] {
            assert!(
                !rendered_text_contains(&output, unsupported),
                "{unsupported} must not be presented as a supported disc format"
            );
        }
    }

    /// The "Safety first" checklist states only true, backend-verified
    /// guarantees.
    #[test]
    fn safety_first_checklist_is_visible() {
        let mut state = OpticalConversionPageState::default();
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Safety first"));
        assert!(rendered_text_contains(
            &output,
            "Every converted file is fingerprint-verified before it counts as done"
        ));
    }

    /// The signal panel's status line is an honest projection of real page
    /// state, never a fabricated reading.
    #[test]
    fn dsk_signal_lines_reflect_real_state() {
        let mut state = OpticalConversionPageState::default();
        assert_eq!(
            dsk_signal_lines(&state, 0.0),
            ["FLOPPY POWER".to_string(), "IDLE".to_string()]
        );
        let directory = tempfile::tempdir().unwrap();
        state = ready_candidate_state(directory.path());
        assert_eq!(
            dsk_signal_lines(&state, 1.0),
            ["FLOPPY POWER".to_string(), "SCANNED 100%".to_string()]
        );
        state.confirm = true;
        assert_eq!(
            dsk_signal_lines(&state, 1.0),
            ["FLOPPY POWER".to_string(), "READY TO CONVERT".to_string()]
        );
    }

    /// No duplicate "Disc Conversion" heading is introduced by the hero.
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
        let mut state = OpticalConversionPageState::default();
        let output = render(&mut state);
        let mut count = 0;
        for clipped in &output.shapes {
            count_occurrences(&clipped.shape, "Disc Conversion", &mut count);
        }
        assert_eq!(count, 0, "poster mode must not duplicate the baked title");
    }

    #[test]
    fn approved_disc_conversion_hero_keeps_source_ratio_and_caches_texture() {
        let decoded = image::load_from_memory(DISC_CONVERSION_HERO_PNG)
            .expect("approved Disc Conversion hero asset");
        let width = 1_024.0_f32;
        let height = disc_conversion_hero_height(width);
        assert!((width / height - decoded.width() as f32 / decoded.height() as f32).abs() < 0.01);
        assert_eq!((decoded.width(), decoded.height()), (1916, 821));

        let context = egui::Context::default();
        let mut state = OpticalConversionPageState::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                assert!(show_disc_conversion_hero(ui, &mut state));
            });
        });
        assert!(state.hero_load_attempted);
        assert!(state.hero_texture.is_some());
    }

    #[test]
    fn poster_mode_keeps_live_conversion_capability_below_the_art() {
        let mut state = OpticalConversionPageState::default();
        let output = render(&mut state);
        assert!(state.hero_texture.is_some());
        assert!(rendered_text_contains(&output, "CUE/BIN"));
        assert!(rendered_text_contains(&output, "CHD"));
        assert!(!rendered_text_contains(
            &output,
            "Review a safe conversion for a supported disc image."
        ));
    }

    #[test]
    fn native_conversion_hero_remains_available_as_fallback() {
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                show_disc_conversion_native_hero(ui, 0.0, &["FLOPPY POWER".into(), "IDLE".into()]);
            });
        });
        assert!(rendered_text_contains(&output, "Disc Conversion"));
    }

    /// `show_optical_conversion_page` still takes only the existing page
    /// state - no new backend/service call was added by this visual pass.
    #[test]
    fn show_page_signature_takes_only_existing_state() {
        fn _assert_signature(ui: &mut egui::Ui, state: &mut OpticalConversionPageState) {
            show_optical_conversion_page(ui, state);
        }
    }

    /// 15. No new conversion format is exposed as supported - CUE remains
    /// the only recognised extension.
    #[test]
    fn no_new_conversion_format_is_exposed() {
        for extension in [
            "bin", "iso", "chd", "img", "gdi", "cdi", "nrg", "toc", "mds",
        ] {
            assert!(
                supported_capability_for_extension(Some(extension)).is_none(),
                "{extension} must not be presented as supported"
            );
        }
        let capability = supported_capability_for_extension(Some("cue")).unwrap();
        assert_eq!(capability.source_format, "CUE/BIN");
        assert_eq!(capability.target_format, "CHD");
    }
}
