//! Native v2 presentation for the existing read-only archive evidence.
//!
//! This module only adapts bounded metadata from the core archive readers. It
//! never extracts a member, writes an archive, or opens the legacy interface.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use archivefs_core::dat::archive::{
    limits::ArchiveLimits, rar::RarProvider, sevenz::SevenZArchiveSource,
};
use archivefs_core::ingestion::ArchiveFormat;
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::{
    InspectorEntry, InspectorEntryClassification, InspectorEntryKind, InspectorReport,
    inspect_archive,
};
use eframe::egui;

const MEMBER_LIMIT: usize = 50_000;
const RAR_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveInspectorTarget {
    pub(crate) game_id: Option<i64>,
    pub(crate) title: String,
    pub(crate) path: PathBuf,
    pub(crate) media: String,
    pub(crate) platform: String,
}

#[derive(Clone, Debug)]
struct ArchiveInspection {
    report: InspectorReport,
    format: ArchiveFormat,
    listed_logical_size: Option<u64>,
}

enum Status {
    Idle,
    Loading {
        target: ArchiveInspectorTarget,
        receiver: Receiver<Result<ArchiveInspection, String>>,
    },
    Ready {
        target: ArchiveInspectorTarget,
        inspection: ArchiveInspection,
    },
    Error {
        target: ArchiveInspectorTarget,
        message: String,
    },
}

pub(crate) struct ArchiveInspectorPageState {
    status: Status,
    rows: Option<(std::sync::Weak<super::library::Library>, Vec<InspectorRow>)>,
}

impl Default for ArchiveInspectorPageState {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            rows: None,
        }
    }
}

/// One physical archive in the inspector list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InspectorRow {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) media: String,
    /// Set only when two different files would otherwise look identical.
    pub(crate) location: Option<String>,
}

/// Lists each physical archive once. Catalogue rows that point at the same
/// file (for example two overlapping source folders) collapse to the first
/// row; distinct files stay distinct, and are told apart by location when
/// their titles collide. Pure and read-only apart from resolving paths.
pub(crate) fn inspector_rows(games: &[super::library::Game]) -> Vec<InspectorRow> {
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<(InspectorRow, PathBuf)> = Vec::new();
    for game in games {
        if !is_supported_archive(&game.archive.archive_kind) {
            continue;
        }
        let path = &game.archive.absolute_path;
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if !seen.insert(canonical.clone()) {
            continue;
        }
        rows.push((
            InspectorRow {
                id: game.archive.id,
                title: game.title.clone(),
                media: super::library::media_kind_label(&game.archive.archive_kind).to_string(),
                location: None,
            },
            canonical,
        ));
    }
    let mut counts = std::collections::HashMap::<(String, String), usize>::new();
    for (row, _) in &rows {
        *counts
            .entry((row.title.clone(), row.media.clone()))
            .or_default() += 1;
    }
    for (row, path) in &mut rows {
        if counts[&(row.title.clone(), row.media.clone())] > 1 {
            row.location = Some(
                path.parent()
                    .and_then(|parent| parent.file_name())
                    .map(|name| format!("in {}", name.to_string_lossy()))
                    .unwrap_or_else(|| path.display().to_string()),
            );
        }
    }
    // A short folder name can still collide (for example two `PS1` folders
    // under different parents). Rows that remain identical fall back to the
    // full folder so distinct files are always distinguishable.
    let mut label_counts = std::collections::HashMap::<(String, String, String), usize>::new();
    for (row, _) in &rows {
        if let Some(location) = &row.location {
            *label_counts
                .entry((row.title.clone(), row.media.clone(), location.clone()))
                .or_default() += 1;
        }
    }
    for (row, path) in &mut rows {
        let collides = row.location.as_ref().is_some_and(|location| {
            label_counts[&(row.title.clone(), row.media.clone(), location.clone())] > 1
        });
        if collides {
            row.location = Some(format!(
                "in {}",
                path.parent().unwrap_or(path.as_path()).display()
            ));
        }
    }
    rows.into_iter().map(|(row, _)| row).collect()
}

impl ArchiveInspectorPageState {
    /// Rows for the current library snapshot, recomputed only when the
    /// snapshot changes so re-entering the page never re-scans or accumulates.
    pub(crate) fn rows(
        &mut self,
        library: &std::sync::Arc<super::library::Library>,
    ) -> Vec<InspectorRow> {
        // A `Weak` keeps the allocation reserved, so a later library can never
        // be mistaken for this one by reusing the same address.
        let current = std::sync::Arc::downgrade(library);
        if self
            .rows
            .as_ref()
            .is_none_or(|(cached, _)| !cached.ptr_eq(&current))
        {
            self.rows = Some((current, inspector_rows(&library.games)));
        }
        self.rows
            .as_ref()
            .map(|(_, rows)| rows.clone())
            .unwrap_or_default()
    }
}

pub(crate) fn archive_format(kind: &str) -> Option<ArchiveFormat> {
    match kind.to_ascii_lowercase().as_str() {
        "zip" => Some(ArchiveFormat::Zip),
        "sevenzip" | "7z" => Some(ArchiveFormat::SevenZip),
        "rar" => Some(ArchiveFormat::Rar),
        _ => None,
    }
}

pub(crate) fn is_supported_archive(kind: &str) -> bool {
    archive_format(kind).is_some()
}

impl ArchiveInspectorTarget {
    pub(crate) fn from_game(game_id: i64, game: &super::library::Game) -> Option<Self> {
        archive_format(&game.archive.archive_kind)?;
        Some(Self {
            game_id: Some(game_id),
            title: game.title.clone(),
            path: game.archive.absolute_path.clone(),
            media: super::library::media_kind_label(&game.archive.archive_kind).to_string(),
            platform: game.platform.clone(),
        })
    }
}

impl ArchiveInspectorPageState {
    fn current_target(&self) -> Option<&ArchiveInspectorTarget> {
        match &self.status {
            Status::Idle => None,
            Status::Loading { target, .. }
            | Status::Ready { target, .. }
            | Status::Error { target, .. } => Some(target),
        }
    }

    pub(crate) fn set_target(
        &mut self,
        target: Option<ArchiveInspectorTarget>,
        ctx: &egui::Context,
    ) {
        if self.current_target() == target.as_ref() {
            self.poll(ctx);
            return;
        }
        let Some(target) = target else {
            self.status = Status::Idle;
            return;
        };
        let path = target.path.clone();
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = inspect_path(&path);
            let _ = sender.send(result);
            repaint.request_repaint();
        });
        self.status = Status::Loading { target, receiver };
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let result = match &self.status {
            Status::Loading { receiver, .. } => match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(
                    "The archive inspection worker stopped unexpectedly.".into(),
                )),
            },
            _ => None,
        };
        let Some(result) = result else {
            return;
        };
        let target = match &self.status {
            Status::Loading { target, .. } => target.clone(),
            _ => return,
        };
        self.status = match result {
            Ok(inspection) => Status::Ready { target, inspection },
            Err(message) => Status::Error { target, message },
        };
        ctx.request_repaint();
    }
}

fn inspect_path(path: &Path) -> Result<ArchiveInspection, String> {
    let format = archive_format(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default(),
    )
    .ok_or_else(|| "This file is not a supported ZIP, 7z or RAR archive.".to_string())?;
    match format {
        ArchiveFormat::Zip => inspect_archive(path)
            .map(|report| inspection(format, report))
            .map_err(|error| error.to_string()),
        ArchiveFormat::SevenZip => inspect_sevenz(path, format),
        ArchiveFormat::Rar => inspect_rar(path, format),
        ArchiveFormat::Tar => unreachable!("TAR is not exposed by archive_format"),
    }
}

fn inspection(format: ArchiveFormat, report: InspectorReport) -> ArchiveInspection {
    let listed_logical_size = report
        .entries
        .iter()
        .map(|entry| entry.uncompressed_size)
        .try_fold(0_u64, |total, size| total.checked_add(size));
    ArchiveInspection {
        report,
        format,
        listed_logical_size,
    }
}

fn inspect_sevenz(path: &Path, format: ArchiveFormat) -> Result<ArchiveInspection, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The 7z archive has no trusted parent directory.".to_string())?
        .canonicalize()
        .map_err(|error| format!("could not establish a trusted archive directory: {error}"))?;
    let trusted = TrustedRoots::from_paths([parent]);
    let cancel = AtomicBool::new(false);
    let source = SevenZArchiveSource::open(path, &trusted, ArchiveLimits::default(), &cancel)
        .map_err(|error| format!("7z inspection refused this archive: {error:?}"))?;
    let members: Vec<_> = source.member_metadata().collect();
    let total = members.len();
    let entries = members
        .into_iter()
        .take(MEMBER_LIMIT)
        .map(|(name, size)| InspectorEntry {
            name: name.to_string(),
            kind: if name.ends_with('/') {
                InspectorEntryKind::Directory
            } else {
                InspectorEntryKind::File
            },
            uncompressed_size: size,
            compressed_size: None,
            compression_method: None,
            classification: archivefs_core::classify_entry(name, name.ends_with('/')),
        })
        .collect();
    let report = InspectorReport {
        entries,
        truncated: total > MEMBER_LIMIT,
        total_entries_in_archive: total,
    };
    Ok(inspection(format, report))
}

fn inspect_rar(path: &Path, format: ArchiveFormat) -> Result<ArchiveInspection, String> {
    let provider = RarProvider::discover(RAR_TIMEOUT)
        .map_err(|error| format!("RAR inspection is unavailable: {error}"))?;
    let session = provider
        .open(path, RAR_TIMEOUT)
        .map_err(|error| format!("RAR inspection refused this archive: {error}"))?;
    let total = session.members.len();
    let entries = session
        .members
        .iter()
        .take(MEMBER_LIMIT)
        .map(|member| InspectorEntry {
            name: member.path.clone(),
            kind: InspectorEntryKind::File,
            uncompressed_size: member.size,
            compressed_size: member.packed_size,
            compression_method: Some(member.method.clone()),
            classification: archivefs_core::classify_entry(&member.path, false),
        })
        .collect();
    let report = InspectorReport {
        entries,
        truncated: total > MEMBER_LIMIT,
        total_entries_in_archive: total,
    };
    Ok(inspection(format, report))
}

fn format_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return "unknown".into();
    };
    if size < 1024 {
        return format!("{size} B");
    }
    if size < 1024 * 1024 {
        return format!("{:.1} KiB", size as f64 / 1024.0);
    }
    format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
}

fn show_entry(ui: &mut egui::Ui, entry: &InspectorEntry) {
    ui.horizontal_wrapped(|ui| {
        ui.label(&entry.name);
        ui.weak(format!(
            "{} · {}",
            entry.classification.label(),
            format_size(Some(entry.uncompressed_size))
        ));
    });
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut ArchiveInspectorPageState,
    target: Option<ArchiveInspectorTarget>,
) {
    state.set_target(target, ui.ctx());
    ui.heading("Archive Inspector");
    ui.label("Read-only archive evidence. EmuWiz lists metadata without extracting or changing anything.");

    if !matches!(&state.status, Status::Idle) {
        // The target is shown below from the active status; all non-idle
        // states carry the same immutable target for this inspection.
        if let Some(target) = state.current_target() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong(&target.title);
                ui.label(format!("{} · {}", target.media, target.path.display()));
                ui.label(format!("Platform: {}", target.platform));
            });
        }
        ui.add_space(8.0);
        match &state.status {
            Status::Loading { .. } => {
                ui.spinner();
                ui.label("Inspecting archive metadata in the background…");
            }
            Status::Error { message, .. } => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "Archive inspection unavailable",
                );
                ui.label(message);
                ui.collapsing("Technical details", |ui| ui.monospace(message));
            }
            Status::Ready { inspection, .. } => show_report(ui, inspection),
            Status::Idle => unreachable!(),
        }
        return;
    }
    ui.label(
        "Choose an archive-backed game from Games or Advanced media tools to inspect its contents.",
    );
}

fn show_report(ui: &mut egui::Ui, inspection: &ArchiveInspection) {
    let report = &inspection.report;
    let likely = report
        .entries
        .iter()
        .filter(|entry| entry.classification == InspectorEntryClassification::LikelyContent)
        .count();
    let nested = report
        .entries
        .iter()
        .filter(|entry| entry.classification == InspectorEntryClassification::NestedArchive)
        .count();
    ui.horizontal_wrapped(|ui| {
        ui.strong(inspection.format.label());
        ui.label(format!("{} entries", report.total_entries_in_archive));
        ui.label(format!("{likely} likely game/media files"));
        ui.label(format!(
            "{} total listed",
            format_size(inspection.listed_logical_size)
        ));
    });
    if report.truncated {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Only the first {} entries are shown; the listing is bounded.",
                report.entries.len()
            ),
        );
    }
    if nested > 0 {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!("{nested} nested archive(s) found. EmuWiz will not open them automatically."),
        );
    }
    let likely_entries: Vec<_> = report
        .entries
        .iter()
        .filter(|entry| entry.classification == InspectorEntryClassification::LikelyContent)
        .collect();
    ui.collapsing(
        format!("Likely game/media files ({})", likely_entries.len()),
        |ui| {
            if likely_entries.is_empty() {
                ui.label("No likely game or media files were recognised by their member names.");
            } else {
                for entry in likely_entries {
                    show_entry(ui, entry);
                }
            }
        },
    );
    ui.collapsing(format!("All members ({})", report.entries.len()), |ui| {
        egui::ScrollArea::vertical()
            .max_height(280.0)
            .show(ui, |ui| {
                for entry in &report.entries {
                    show_entry(ui, entry);
                }
            });
    });
    ui.collapsing("Technical details", |ui| {
        ui.label("Member names and declared sizes only; no member data was extracted.");
        ui.label("Platform identity comes from the selected catalogue record, not archive filenames alone.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_archive_formats_are_explicit() {
        assert_eq!(archive_format("zip"), Some(ArchiveFormat::Zip));
        assert_eq!(archive_format("7z"), Some(ArchiveFormat::SevenZip));
        assert_eq!(archive_format("sevenzip"), Some(ArchiveFormat::SevenZip));
        assert_eq!(archive_format("rar"), Some(ArchiveFormat::Rar));
        assert_eq!(archive_format("tar"), None);
    }

    #[test]
    fn classifications_surface_nested_and_likely_content_without_opening_members() {
        assert_eq!(
            archivefs_core::classify_entry("disc/game.iso", false),
            InspectorEntryClassification::LikelyContent
        );
        assert_eq!(
            archivefs_core::classify_entry("payload.zip", false),
            InspectorEntryClassification::NestedArchive
        );
    }

    #[test]
    fn bounded_report_is_truthful() {
        let report = InspectorReport {
            entries: vec![],
            truncated: true,
            total_entries_in_archive: MEMBER_LIMIT + 1,
        };
        assert!(report.truncated);
        assert!(report.total_entries_in_archive > report.entries.len());
    }

    #[test]
    fn zip_sevenzip_and_rar_reports_share_the_same_read_only_projection() {
        for format in [
            ArchiveFormat::Zip,
            ArchiveFormat::SevenZip,
            ArchiveFormat::Rar,
        ] {
            let report = InspectorReport {
                entries: vec![InspectorEntry {
                    name: "game.iso".into(),
                    kind: InspectorEntryKind::File,
                    uncompressed_size: 42,
                    compressed_size: Some(21),
                    compression_method: None,
                    classification: InspectorEntryClassification::LikelyContent,
                }],
                truncated: false,
                total_entries_in_archive: 1,
            };
            let view = inspection(format, report);
            assert_eq!(view.format, format);
            assert_eq!(view.listed_logical_size, Some(42));
            assert_eq!(view.report.entries.len(), 1);
        }
    }
}
