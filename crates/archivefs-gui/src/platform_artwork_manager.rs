//! Platform artwork management session.
//!
//! Owns the managed directory, session texture cache, inspection/import work,
//! picker and confirmation drafts, feedback and mutation-driven invalidation.
//! Reuses ui::platform_artwork's canonical registry, decoding and fallback policy,
//! and core::platform_artwork's safe file operations. No second mapping system.
//!
//! Does not own navigation, platform selection, library/configuration state,
//! RomM covers/screenshots, or generic image/desktop services. The app supplies
//! the directory and folder-opening service once and retains this session.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use archivefs_core::ArchiveFsError;
use eframe::egui;

use crate::ui::components as widgets;
use crate::ui::platform_artwork::{
    PlatformArtworkCache, PlatformArtworkPaint, PlatformAssetCategory, bundled_platform_artwork,
    canonical_platform_asset_id, custom_platform_artwork_path, paint_platform_artwork_at,
    platform_asset_category,
};

/// All mutable artwork ownership lives here for the existing app lifetime.
pub(crate) struct PlatformArtworkManager {
    directory: Option<PathBuf>,
    cache: PlatformArtworkCache,
    state: PlatformArtworkManagerState,
    open_folder: fn(&Path) -> archivefs_core::Result<()>,
}

/// A short-lived borrow for the existing Library/Gamer painters. It does not
/// duplicate the cache or change its lifetime. Keep their low-level contract
/// intact rather than migrating unrelated cover/screenshot rendering.
pub(crate) struct ArtworkRenderAssets<'a> {
    pub(crate) directory: Option<&'a Path>,
    pub(crate) cache: &'a mut PlatformArtworkCache,
}

impl PlatformArtworkManager {
    pub(crate) fn new(
        directory: Option<PathBuf>,
        open_folder: fn(&Path) -> archivefs_core::Result<()>,
    ) -> Self {
        Self {
            directory,
            cache: PlatformArtworkCache::default(),
            state: PlatformArtworkManagerState::default(),
            open_folder,
        }
    }

    pub(crate) fn render_assets(&mut self) -> ArtworkRenderAssets<'_> {
        ArtworkRenderAssets {
            directory: self.directory.as_deref(),
            cache: &mut self.cache,
        }
    }

    /// Preserve the existing lazy Settings-entry scan; never run it at startup.
    pub(crate) fn prepare_settings(&mut self, context: &egui::Context) {
        if self.state.status.is_none() && self.state.task.is_none() {
            self.dispatch(context.clone(), PlatformArtworkManagerAction::Rescan);
        }
    }

    /// Return a domain action to preserve the Settings page's single-action
    /// arbitration; main dispatches it without implementing the operation.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) -> Option<PlatformArtworkManagerAction> {
        let mut action = None;
        show_platform_artwork_manager(
            ui,
            self.directory.as_deref(),
            &mut self.cache,
            &mut self.state,
            &mut action,
        );
        action
    }

    /// Same whole-cache invalidation as the previous managed-mutation path.
    pub(crate) fn invalidate(&mut self) {
        self.cache.clear();
    }

    pub(crate) fn dispatch(
        &mut self,
        context: egui::Context,
        action: PlatformArtworkManagerAction,
    ) {
        if self.state.task.is_some() {
            return;
        }
        let Some(root) = self.directory.clone() else {
            self.state.message = Some((
                false,
                "EmuWiz could not resolve its local data directory.".to_owned(),
            ));
            return;
        };
        if matches!(action, PlatformArtworkManagerAction::OpenFolder) {
            if let Err(error) = std::fs::create_dir_all(&root)
                .map_err(ArchiveFsError::from)
                .and_then(|()| (self.open_folder)(&root))
            {
                self.state.message = Some((false, error.to_string()));
            }
            return;
        }
        let preview = self.state.bulk_preview.clone();
        let replace = self.state.replace_existing;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            use archivefs_core::platform_artwork as artwork;
            let result = match action {
                PlatformArtworkManagerAction::Rescan => PlatformArtworkTaskResult::Status(
                    artwork::inspect_platform_artwork(&root).map_err(|error| error.to_string()),
                ),
                PlatformArtworkManagerAction::Import {
                    platform_id,
                    source,
                } => PlatformArtworkTaskResult::Mutation(
                    artwork::import_platform_artwork(&root, &platform_id, &source, replace)
                        .map(|result| {
                            format!(
                                "Imported {} as {}{}.",
                                platform_id,
                                result.destination.display(),
                                result
                                    .warnings
                                    .first()
                                    .map(|warning| format!(" Warning: {warning}"))
                                    .unwrap_or_default()
                            )
                        })
                        .map_err(|error| error.to_string()),
                ),
                PlatformArtworkManagerAction::PreviewFolder(source) => {
                    PlatformArtworkTaskResult::BulkPreview(
                        artwork::preview_import_folder(&root, &source)
                            .map_err(|error| error.to_string()),
                    )
                }
                PlatformArtworkManagerAction::ApplyFolder => PlatformArtworkTaskResult::Mutation(
                    preview
                        .ok_or_else(|| "Run folder preview before importing.".to_owned())
                        .and_then(|preview| {
                            artwork::apply_import_folder(&root, &preview, replace)
                                .map_err(|error| error.to_string())
                        })
                        .map(|result| {
                            format!(
                                "Imported {} image(s); {} item(s) remained for review.",
                                result.imported.len(),
                                result.skipped.len()
                            )
                        }),
                ),
                PlatformArtworkManagerAction::Remove(platform_id) => {
                    PlatformArtworkTaskResult::Mutation(
                        artwork::remove_custom_platform_artwork(&root, &platform_id, true)
                            .map(|removed| {
                                if removed {
                                    format!("Restored the default artwork for {platform_id}.")
                                } else {
                                    format!("No custom artwork existed for {platform_id}.")
                                }
                            })
                            .map_err(|error| error.to_string()),
                    )
                }
                PlatformArtworkManagerAction::OpenFolder => unreachable!(),
            };
            let _ = sender.send(result);
            context.request_repaint();
        });
        self.state.task = Some(receiver);
    }

    pub(crate) fn poll(&mut self, context: &egui::Context) {
        let Some(receiver) = &self.state.task else {
            return;
        };
        let Ok(result) = receiver.try_recv() else {
            return;
        };
        self.state.task = None;
        match result {
            PlatformArtworkTaskResult::Status(result) => match result {
                Ok(status) => {
                    self.state.status = Some(status);
                }
                Err(error) => self.state.message = Some((false, error)),
            },
            PlatformArtworkTaskResult::BulkPreview(result) => match result {
                Ok(preview) => {
                    self.state.bulk_preview = Some(preview);
                    self.state.message = Some((
                        true,
                        "Folder preview complete; nothing was written.".to_owned(),
                    ));
                }
                Err(error) => self.state.message = Some((false, error)),
            },
            PlatformArtworkTaskResult::Mutation(result) => {
                self.invalidate();
                self.state.message = Some(match result {
                    Ok(message) => (true, message),
                    Err(error) => (false, error),
                });
                self.dispatch(context.clone(), PlatformArtworkManagerAction::Rescan);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ArtworkManagerFilter {
    #[default]
    All,
    Missing,
    Custom,
    FallbackOnly,
}

enum PlatformArtworkTaskResult {
    Status(Result<archivefs_core::platform_artwork::PlatformArtworkStatus, String>),
    Mutation(Result<String, String>),
    BulkPreview(Result<archivefs_core::platform_artwork::BulkArtworkPreview, String>),
}

#[derive(Default)]
struct PlatformArtworkManagerState {
    search: String,
    filter: ArtworkManagerFilter,
    status: Option<archivefs_core::platform_artwork::PlatformArtworkStatus>,
    bulk_preview: Option<archivefs_core::platform_artwork::BulkArtworkPreview>,
    replace_existing: bool,
    pending_import: Option<(String, PathBuf)>,
    pending_remove: Option<String>,
    message: Option<(bool, String)>,
    task: Option<mpsc::Receiver<PlatformArtworkTaskResult>>,
    /// An in-flight native file dialog, run on a background thread so the egui
    /// frame is never blocked while it is open.
    pending_pick: Option<FilePickRequest>,
}

/// A native image-picker file dialog running on a background thread.
struct FilePickRequest {
    platform_id: String,
    /// Whether this platform already has a custom image (replacement flow).
    custom: bool,
    receiver: mpsc::Receiver<Option<PathBuf>>,
}

/// The outcome of draining one file-picker channel, as a tiny pure state
/// machine so the caller (and tests) reason about it without a real dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FilePickDrain {
    /// The picker is still running - leave `pending_pick` intact.
    Pending,
    /// The user pressed Cancel (`Ok(None)`): clear the picker, change nothing,
    /// show no error.
    Cancelled,
    /// The user chose a file: clear the picker and continue the import.
    Picked(PathBuf),
    /// The picker thread ended without sending a result: clear the picker and
    /// surface a friendly error so the buttons become available again.
    Disconnected,
}

/// Reads one state from the picker channel. Pure and deterministic: no file
/// dialog is required.
fn drain_file_pick(receiver: &mpsc::Receiver<Option<PathBuf>>) -> FilePickDrain {
    match receiver.try_recv() {
        Ok(Some(path)) => FilePickDrain::Picked(path),
        Ok(None) => FilePickDrain::Cancelled,
        Err(mpsc::TryRecvError::Empty) => FilePickDrain::Pending,
        Err(mpsc::TryRecvError::Disconnected) => FilePickDrain::Disconnected,
    }
}

/// The plain-language error shown when the picker thread exits without a
/// result (for example the dialog failed to open).
const FILE_PICKER_DISCONNECTED_MESSAGE: &str =
    "The image picker closed unexpectedly. Please try again.";

pub(crate) enum PlatformArtworkManagerAction {
    Rescan,
    Import {
        platform_id: String,
        source: PathBuf,
    },
    PreviewFolder(PathBuf),
    ApplyFolder,
    Remove(String),
    OpenFolder,
}

fn current_artwork_source(
    root: Option<&Path>,
    platform_id: &str,
    status: Option<&archivefs_core::platform_artwork::PlatformArtworkStatus>,
) -> (&'static str, bool) {
    let is_valid = |path: &Path| {
        !status.is_some_and(|status| {
            status
                .invalid_custom_files
                .iter()
                .any(|invalid| invalid.path == path)
        })
    };
    let asset_id = canonical_platform_asset_id(platform_id);
    if custom_platform_artwork_path(root, &asset_id).is_some_and(|path| is_valid(&path)) {
        return ("Custom", true);
    }
    if bundled_platform_artwork(&asset_id).is_some() {
        return ("Bundled", false);
    }
    let category = platform_asset_category(platform_id);
    if category != PlatformAssetCategory::Unknown
        && custom_platform_artwork_path(root, category.asset_id())
            .is_some_and(|path| is_valid(&path))
    {
        return ("Category fallback (custom)", false);
    }
    if category == PlatformAssetCategory::Unknown {
        ("Unknown fallback", false)
    } else {
        ("Category fallback", false)
    }
}

fn show_platform_artwork_manager(
    ui: &mut egui::Ui,
    root: Option<&Path>,
    artwork_cache: &mut PlatformArtworkCache,
    manager: &mut PlatformArtworkManagerState,
    action: &mut Option<PlatformArtworkManagerAction>,
) {
    let running = manager.task.is_some();
    // Drain a finished background file dialog: pick_file() ran on a worker
    // thread so the frame was never blocked. When it returns, process exactly
    // what the inline call used to.
    if let Some(pick) = manager.pending_pick.as_mut() {
        match drain_file_pick(&pick.receiver) {
            FilePickDrain::Pending => {
                // Picker still open: keep `pending_pick` so no second dialog
                // starts and the frame keeps draining.
            }
            FilePickDrain::Cancelled => {
                // User pressed Cancel: nothing changed, no error, buttons
                // become available again.
                manager.pending_pick = None;
            }
            FilePickDrain::Disconnected => {
                // Thread ended without a result: release the picker and tell
                // the user, so the buttons are usable again.
                manager.pending_pick = None;
                manager.message = Some((false, FILE_PICKER_DISCONNECTED_MESSAGE.to_string()));
            }
            FilePickDrain::Picked(source) => {
                let FilePickRequest {
                    platform_id,
                    custom,
                    ..
                } = manager.pending_pick.take().expect("just drained");
                if custom {
                    manager.pending_import = Some((platform_id, source));
                } else {
                    manager.replace_existing = false;
                    *action = Some(PlatformArtworkManagerAction::Import {
                        platform_id,
                        source,
                    });
                }
            }
        }
    }
    widgets::card(ui, |ui| {
        ui.label(format!(
            "Managed folder: {}",
            root.map_or_else(
                || "Unavailable".to_owned(),
                |path| path.display().to_string()
            )
        ));
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new("Open artwork folder"))
                .clicked()
            {
                *action = Some(PlatformArtworkManagerAction::OpenFolder);
            }
            if ui
                .add_enabled(!running, egui::Button::new("Rescan custom artwork"))
                .clicked()
            {
                *action = Some(PlatformArtworkManagerAction::Rescan);
            }
            if ui
                .add_enabled(!running, egui::Button::new("Preview folder import"))
                .clicked()
                && let Some(folder) = rfd::FileDialog::new().pick_folder()
            {
                *action = Some(PlatformArtworkManagerAction::PreviewFolder(folder));
            }
            if running {
                ui.spinner();
                ui.label("Validating artwork…");
            }
        });
        if let Some((succeeded, message)) = &manager.message {
            widgets::banner(
                ui,
                if *succeeded {
                    "Artwork updated"
                } else {
                    "Artwork error"
                },
                message,
                if *succeeded {
                    widgets::StatusTone::Success
                } else {
                    widgets::StatusTone::Blocked
                },
            );
        }
        if let Some(status) = &manager.status {
            ui.label(format!(
                "{} canonical platforms · {} custom · {} bundled · {} fallback-only · {} invalid · {} unknown · {} bytes",
                status.total_canonical_platforms,
                status.custom_images,
                status.bundled_images,
                status.fallback_only_platforms,
                status.invalid_custom_files.len(),
                status.unknown_files.len(),
                status.total_custom_disk_bytes
            ));
            if !status.invalid_custom_files.is_empty() || !status.unknown_files.is_empty() {
                ui.collapsing("Invalid and unknown files", |ui| {
                    for invalid in &status.invalid_custom_files {
                        ui.label(format!(
                            "Invalid: {} — {}",
                            invalid.path.display(),
                            invalid.reason
                        ));
                    }
                    for unknown in &status.unknown_files {
                        ui.label(format!("Unknown: {}", unknown.display()));
                    }
                    ui.weak("Rescan never deletes these files.");
                });
            }
        }
        if let Some(preview) = &manager.bulk_preview {
            let recognised = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::Recognised
                })
                .count();
            let unknown = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::UnknownFilename
                })
                .count();
            let invalid = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::Invalid
                })
                .count();
            let duplicates = preview
                .entries
                .iter()
                .filter(|entry| {
                    entry.disposition
                        == archivefs_core::platform_artwork::BulkArtworkDisposition::DuplicateTarget
                })
                .count();
            ui.separator();
            ui.label(format!("Folder preview: {recognised} recognised · {unknown} unknown · {invalid} invalid · {duplicates} duplicate target(s)."));
            for entry in preview.entries.iter().take(10) {
                ui.label(format!(
                    "{:?}: {} — {}",
                    entry.disposition,
                    entry.source.display(),
                    entry.detail
                ));
            }
            if preview.entries.len() > 10 {
                ui.collapsing(
                    format!("Show all {} reviewed files", preview.entries.len()),
                    |ui| {
                        for entry in &preview.entries {
                            ui.label(format!(
                                "{:?}: {}",
                                entry.disposition,
                                entry.source.display()
                            ));
                        }
                    },
                );
            }
            ui.checkbox(
                &mut manager.replace_existing,
                "Replace existing custom artwork after confirmation",
            );
            if ui
                .add_enabled(
                    !running && recognised > 0,
                    egui::Button::new("Import recognised images"),
                )
                .clicked()
            {
                *action = Some(PlatformArtworkManagerAction::ApplyFolder);
            }
        }
    });

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.label("Search platforms:");
        ui.text_edit_singleline(&mut manager.search);
        egui::ComboBox::from_id_salt("platform_artwork_filter")
            .selected_text(match manager.filter {
                ArtworkManagerFilter::All => "All platforms",
                ArtworkManagerFilter::Missing => "Missing artwork",
                ArtworkManagerFilter::Custom => "Custom artwork",
                ArtworkManagerFilter::FallbackOnly => "Fallback only",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::All,
                    "All platforms",
                );
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::Missing,
                    "Missing artwork",
                );
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::Custom,
                    "Custom artwork",
                );
                ui.selectable_value(
                    &mut manager.filter,
                    ArtworkManagerFilter::FallbackOnly,
                    "Fallback only",
                );
            });
    });

    let search = manager.search.trim().to_ascii_lowercase();
    for platform in archivefs_core::platform::PLATFORMS {
        let (source_label, custom) =
            current_artwork_source(root, platform.id, manager.status.as_ref());
        let fallback = source_label.contains("fallback");
        let missing = bundled_platform_artwork(&canonical_platform_asset_id(platform.id)).is_none()
            && !custom;
        if !search.is_empty()
            && !platform.display_name.to_ascii_lowercase().contains(&search)
            && !platform.id.to_ascii_lowercase().contains(&search)
        {
            continue;
        }
        if !match manager.filter {
            ArtworkManagerFilter::All => true,
            ArtworkManagerFilter::Missing => missing,
            ArtworkManagerFilter::Custom => custom,
            ArtworkManagerFilter::FallbackOnly => fallback,
        } {
            continue;
        }
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                let (response, _) =
                    ui.allocate_painter(egui::vec2(72.0, 72.0), egui::Sense::hover());
                let asset_id = canonical_platform_asset_id(platform.id);
                paint_platform_artwork_at(
                    ui,
                    artwork_cache,
                    root,
                    PlatformArtworkPaint {
                        center: response.rect.center(),
                        size: 64.0,
                        color: ui.visuals().text_color().gamma_multiply(0.8),
                        asset_id: &asset_id,
                        fallback_asset_id: platform_asset_category(platform.id).asset_id(),
                    },
                );
                ui.vertical(|ui| {
                    ui.heading(platform.display_name);
                    ui.label(format!("Canonical ID: {}", platform.id));
                    ui.label(format!("Current source: {source_label}"));
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                !running && manager.pending_pick.is_none(),
                                egui::Button::new("Choose image"),
                            )
                            .clicked()
                        {
                            // Run the native dialog on a background thread: a
                            // blocking `pick_file()` on the egui thread freezes
                            // the UI until the dialog closes.
                            let (sender, receiver) = mpsc::channel();
                            std::thread::spawn(move || {
                                let picked = rfd::FileDialog::new()
                                    .add_filter("Static image", &["png", "jpg", "jpeg", "webp"])
                                    .pick_file();
                                let _ = sender.send(picked);
                            });
                            manager.pending_pick = Some(FilePickRequest {
                                platform_id: platform.id.to_owned(),
                                custom,
                                receiver,
                            });
                        }
                        if let Some((pending_platform, _)) = &manager.pending_import
                            && pending_platform == platform.id
                        {
                            ui.label("Replace the existing custom image?");
                            if ui
                                .add_enabled(!running, egui::Button::new("Confirm replacement"))
                                .clicked()
                                && let Some((platform_id, source)) = manager.pending_import.take()
                            {
                                manager.replace_existing = true;
                                *action = Some(PlatformArtworkManagerAction::Import {
                                    platform_id,
                                    source,
                                });
                            }
                            if ui.button("Cancel replacement").clicked() {
                                manager.pending_import = None;
                            }
                        }
                        if custom
                            && manager.pending_remove.as_deref() != Some(platform.id)
                            && ui
                                .add_enabled(!running, egui::Button::new("Remove custom image"))
                                .clicked()
                        {
                            manager.pending_remove = Some(platform.id.to_owned());
                        }
                        if manager.pending_remove.as_deref() == Some(platform.id) {
                            ui.label("Remove EmuWiz's custom copy?");
                            if ui
                                .add_enabled(!running, egui::Button::new("Confirm restore default"))
                                .clicked()
                            {
                                manager.pending_remove = None;
                                *action = Some(PlatformArtworkManagerAction::Remove(
                                    platform.id.to_owned(),
                                ));
                            }
                            if ui.button("Cancel").clicked() {
                                manager.pending_remove = None;
                            }
                        }
                    });
                });
            });
        });
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests;
