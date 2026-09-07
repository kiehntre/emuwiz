// Source configuration and catalogue page rendering extracted mechanically from main.rs.

use super::*;

/// Sources configuration and scan review are text-led workflows. Keep their
/// cards on one readable column even though the parent Sources view also hosts
/// wider specialist tabs such as DAT sources and RomM.
pub(super) fn sources_content_width(available_width: f32) -> f32 {
    available_width.max(0.0).min(theme::CONTENT_MAX_WIDTH)
}

pub(super) fn sources_content_column<R>(
    ui: &mut egui::Ui,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let width = sources_content_width(ui.available_width());
    let gutter = ((ui.available_width() - width) * 0.5).max(0.0);
    ui.horizontal_top(|ui| {
        ui.add_space(gutter);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            add_contents,
        )
        .inner
    })
    .inner
}

/// Renders the Sources page's compact echo of its most recently completed
/// scan (see [`SourcesLastScan`]) directly on the page, next to the
/// source/action it belongs to - not only reachable via the separate
/// Tools -> Database Status panel. Returns `true` when the "Inspect
/// skipped" action was clicked; the caller reuses the exact same
/// `show_skipped_files`/`skipped_files_filter` state (and
/// `show_skipped_files_window`) Database Status already uses, so this
/// never creates a second scanner or a duplicate skip-detail model.
pub(super) fn show_sources_last_scan_banner(
    ui: &mut egui::Ui,
    last_scan: &SourcesLastScan,
) -> bool {
    let mut inspect_clicked = false;
    widgets::card(ui, |ui| {
        ui.vertical(|ui| {
            ui.strong("Last scan");
            let scope_label = match &last_scan.scope {
                SourcesScanScope::One(path) => path.display().to_string(),
                SourcesScanScope::AllEnabled => "All enabled sources".to_string(),
            };
            ui.label(scope_label);
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} archive{} found",
                    last_scan.archives_found,
                    if last_scan.archives_found == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
                ui.weak("·");
                ui.label(format!("{} skipped", last_scan.skipped_total));
            });
            // The archive count above is the existing scanner's own
            // total; this breakdown is the additive mixed-collection view
            // from `archivefs_core::ingestion`, so a mostly-loose-ROM
            // collection isn't hidden behind an "archives found" number
            // that only ever describes a fraction of it.
            let stats = &last_scan.ingestion_stats;
            let breakdown = [
                ("Loose ROMs", stats.loose_roms),
                ("Disc images", stats.disc_images),
                ("Amiga images", stats.amiga_images),
                ("Computer disks", stats.computer_disks),
                ("Game folders", stats.game_folders),
                ("Unknown", stats.unknown),
            ];
            if breakdown.iter().any(|(_, count)| *count > 0) {
                ui.add_space(4.0);
                ui.weak("What was found in this source:");
                for (label, count) in breakdown {
                    if count > 0 {
                        ui.label(format!("{label}: {count}"));
                    }
                }
            }
            if last_scan.skipped_total > 0
                && widgets::action_button(ui, "Inspect skipped", widgets::ActionStyle::Quiet, true)
                    .clicked()
            {
                inspect_clicked = true;
            }
        });
    });
    inspect_clicked
}

pub(super) enum CatalogueManagerState {
    NotLoaded,
    Loading(Receiver<Result<CheatSourceList, CheatSourceError>>),
    Ready(CheatSourceList),
    Failed(CheatSourceError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CatalogueRetrievalKind {
    Download,
    Update,
}

pub(super) struct CatalogueReview {
    pub(super) source_id: String,
    pub(super) kind: CatalogueRetrievalKind,
}

pub(super) struct RunningCatalogueRetrieval {
    pub(super) generation: u64,
    pub(super) source_id: String,
    pub(super) cancellation: CheatSourceCancellation,
    pub(super) receiver: Receiver<Result<CheatSourceFetchResult, CheatSourceError>>,
    pub(super) progress_receiver: Receiver<CheatSourceProgress>,
    pub(super) progress: Option<CheatSourceProgress>,
    pub(super) cancellation_requested: bool,
}

pub(super) enum CatalogueManagerAction {
    Refresh,
    Review {
        source_id: String,
        kind: CatalogueRetrievalKind,
    },
    Confirm,
    CancelReview,
    CancelRunning,
}

pub(super) enum SourcesPageAction {
    AddFolder(PathBuf),
    ApplyMountRoot(PathBuf),
    ScanOne(PathBuf),
    ScanAll,
    RefreshStatus,
    AssignPlatform {
        path: PathBuf,
        platform: String,
    },
    SetEnabled {
        path: PathBuf,
        enabled: bool,
    },
    ConfirmRemove {
        path: PathBuf,
        keep_catalogue: bool,
    },
    /// Navigation only: hand the user to the existing Sources -> Discovery
    /// tab. The detailed scan results stay owned by that one page.
    ViewScanDetails,
    /// From the Sources row context menu's "Show archives from this
    /// source" - navigates to the Library page filtered to exactly this
    /// source, reusing the same `library_source_filter` the Library
    /// page's own Source dropdown already drives (never a second,
    /// independently-drifting filter mechanism).
    ViewInLibrary(PathBuf),
}

/// The visible outcome of the most recent "Apply folder" for the temporary
/// preparation folder, rendered inside the Sources -> Libraries mount-root
/// card. Set from the background `SetupAction::SetMountRoot` result; `None`
/// until one has finished.
pub(super) struct MountRootFeedback {
    pub(super) succeeded: bool,
    /// One plain sentence for a beginner (success confirmation or a
    /// friendly failure summary).
    pub(super) summary: String,
    /// The raw backend/config/I-O error, shown only under "Technical
    /// details" on failure.
    pub(super) detail: Option<String>,
    /// A visible warning when the config write succeeded but EmuWiz could
    /// not reload its configuration afterwards.
    pub(super) warning: Option<String>,
}

pub(super) fn catalogue_status_label(status: CheatCatalogueStatus) -> &'static str {
    match status {
        CheatCatalogueStatus::Missing => "Missing",
        CheatCatalogueStatus::Ready => "Ready",
        CheatCatalogueStatus::ReadyWithWarnings => "Verified with warnings",
        CheatCatalogueStatus::Stale => "Stale",
        CheatCatalogueStatus::InvalidManifest => "Invalid manifest",
        CheatCatalogueStatus::Incomplete => "Incomplete",
        CheatCatalogueStatus::UnsupportedSchema => "Unsupported schema",
        CheatCatalogueStatus::VerificationFailed => "Verification failed",
        CheatCatalogueStatus::RetrievalFailed => "Retrieval failed",
        CheatCatalogueStatus::Cancelled => "Cancelled",
        CheatCatalogueStatus::ResourceLimitReached => "Resource limit reached",
    }
}

pub(super) fn show_retroarch_catalogue_manager(
    ui: &mut egui::Ui,
    state: &CatalogueManagerState,
    review: Option<&CatalogueReview>,
    running: Option<&RunningCatalogueRetrieval>,
    last_result: Option<&Result<CheatSourceFetchResult, CheatSourceError>>,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<CatalogueManagerAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "RetroArch cheat catalogue",
        Some(
            "Explicitly acquire and verify third-party content from the official Libretro database.",
        ),
    );
    match state {
        CatalogueManagerState::NotLoaded | CatalogueManagerState::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Verifying local catalogue status…");
            });
        }
        CatalogueManagerState::Failed(error) => {
            widgets::banner(
                ui,
                "Catalogue status unavailable",
                &error.to_string(),
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Retry", widgets::ActionStyle::Secondary, true).clicked()
            {
                action = Some(CatalogueManagerAction::Refresh);
            }
        }
        CatalogueManagerState::Ready(list) => {
            for entry in &list.entries {
                widgets::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&entry.source.display_name);
                    });
                    let status_tone = match entry.status {
                        CheatCatalogueStatus::Ready => widgets::StatusTone::Success,
                        CheatCatalogueStatus::ReadyWithWarnings => widgets::StatusTone::Warning,
                        CheatCatalogueStatus::Missing => widgets::StatusTone::Pending,
                        CheatCatalogueStatus::Stale | CheatCatalogueStatus::Cancelled => {
                            widgets::StatusTone::Warning
                        }
                        _ => widgets::StatusTone::Blocked,
                    };
                    widgets::status_strip(
                        ui,
                        &[
                            (catalogue_status_label(entry.status), status_tone),
                            (
                                "Official repository · third-party content",
                                widgets::StatusTone::Info,
                            ),
                        ],
                    );
                    ui.label("Catalogue download does not install cheats. Apply remains a separate confirmed transaction.");
                    ui.label("Updating this snapshot does not modify RetroArch or emulator files, and a game may legitimately have no matching cheat.");
                    if let Some(revision) = &entry.current_cached_version {
                        ui.label(format!("Active revision: {revision}"));
                    }
                    ui.horizontal_wrapped(|ui| {
                        if let Some(count) = entry.catalogue_file_count {
                            ui.label(format!("Active files: {count}"));
                        }
                        if let Some(count) = entry.indexed_file_count {
                            ui.label(format!("Indexed: {count}"));
                        }
                        if let Some(count) = entry.excluded_file_count
                            && count > 0
                        {
                            ui.label(format!(
                                "{count} cheat file{} could not be read (see Technical details below)",
                                if count == 1 { "" } else { "s" }
                            ));
                        }
                        if let Some(bytes) = entry.total_bytes {
                            ui.label(format!("Verified size: {}", format_size(Some(bytes))));
                        }
                        if let Some(timestamp) = entry.fetched_at_unix_seconds {
                            ui.label(format!(
                                "Last successful update: {}",
                                format_unix_timestamp_utc(timestamp as i64)
                            ));
                        }
                    });
                    show_cheat_warnings_summary(
                        ui,
                        &entry.warnings,
                        ("catalogue_entry_warnings", &entry.source.source_id),
                        clipboard,
                    );
                    if let Some(error) = &entry.last_error {
                        widgets::banner(
                            ui,
                            if entry.setup_usable {
                                "Update failed · existing catalogue remains active and usable"
                            } else {
                                "Update failed · no usable active catalogue"
                            },
                            cheat_source_error_human_detail(error),
                            widgets::StatusTone::Warning,
                        );
                        widgets::technical_details(
                            ui,
                            ("catalogue_entry_last_error", &entry.source.source_id),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(error.to_string()).monospace(),
                                    )
                                    .wrap(),
                                );
                            },
                        );
                        if let Some(timestamp) = entry.last_error_at_unix_seconds {
                            ui.label(format!(
                                "Last failed update: {}",
                                format_unix_timestamp_utc(timestamp as i64)
                            ));
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        let idle = running.is_none() && review.is_none();
                        if entry.status == CheatCatalogueStatus::Missing
                            && widgets::action_button(
                                ui,
                                "Download",
                                widgets::ActionStyle::Primary,
                                idle,
                            )
                            .clicked()
                        {
                            action = Some(CatalogueManagerAction::Review {
                                source_id: entry.source.source_id.clone(),
                                kind: CatalogueRetrievalKind::Download,
                            });
                        }
                        if entry.status != CheatCatalogueStatus::Missing
                            && widgets::action_button(
                                ui,
                                "Update",
                                widgets::ActionStyle::Primary,
                                idle,
                            )
                            .clicked()
                        {
                            action = Some(CatalogueManagerAction::Review {
                                source_id: entry.source.source_id.clone(),
                                kind: CatalogueRetrievalKind::Update,
                            });
                        }
                        if widgets::action_button(
                            ui,
                            "Verify",
                            widgets::ActionStyle::Secondary,
                            running.is_none(),
                        )
                        .clicked()
                        {
                            action = Some(CatalogueManagerAction::Refresh);
                        }
                    });
                    widgets::technical_details(
                        ui,
                        ("provider_technical_details", &entry.source.source_id),
                        |ui| {
                            widgets::copyable_value(ui, "Provider ID", &entry.source.source_id);
                            widgets::copyable_value(
                                ui,
                                "Canonical repository",
                                &entry.source.canonical_repository_url,
                            );
                            widgets::copyable_value(
                                ui,
                                "Revision resolver",
                                &entry.source.revision_url,
                            );
                            widgets::copyable_value(
                                ui,
                                "Immutable archive template",
                                &entry.source.download_url,
                            );
                            if let Some(digest) = &entry.archive_sha256 {
                                widgets::copyable_value(ui, "Snapshot SHA-256", digest);
                            }
                            if !entry.exclusion_examples.is_empty() {
                                ui.separator();
                                ui.strong("Bounded exclusion examples");
                                egui::ScrollArea::vertical()
                                    .id_salt((
                                        "provider_exclusion_examples_scroll",
                                        &entry.source.source_id,
                                    ))
                                    .max_height(220.0)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        for example in &entry.exclusion_examples {
                                            ui.label(format!(
                                                "{} {}",
                                                example.relative_path.as_deref().unwrap_or(
                                                    "A file (path bytes are not representable \
                                                     safely as UTF-8)"
                                                ),
                                                cheat_source_exclusion_reason(example.kind)
                                            ));
                                        }
                                    });
                            }
                            ui.label(format!("Trust classification: {}", entry.trust_status));
                            if ui.button("Copy provider details").clicked() {
                                let _ = clipboard.set_text(format!(
                                    "{}\n{}",
                                    entry.source.canonical_repository_url,
                                    entry.source.revision_url
                                ));
                            }
                        },
                    );
                });
            }
        }
    }
    if let Some(running) = running {
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(if running.cancellation_requested {
                    "Cancellation requested; the active snapshot will remain unchanged."
                } else {
                    "Catalogue retrieval is running; the active snapshot remains usable until activation."
                });
            });
            if let Some(progress) = &running.progress {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(
                        ui,
                        catalogue_progress_label(progress.phase),
                        if progress.phase == CheatSourceProgressPhase::Retrying {
                            widgets::StatusTone::Warning
                        } else {
                            widgets::StatusTone::Info
                        },
                    );
                    if progress.attempt != 0 {
                        ui.label(format!(
                            "Attempt {} of {}",
                            progress.attempt, progress.maximum_attempts
                        ));
                    }
                });
                if progress.phase == CheatSourceProgressPhase::Downloading {
                    let received = format_transfer_bytes(progress.bytes_received);
                    if let Some(total) = progress.total_bytes {
                        let percentage = if total == 0 {
                            0.0
                        } else {
                            progress.bytes_received as f64 * 100.0 / total as f64
                        };
                        ui.label(format!(
                            "Received {received} of {} ({percentage:.1}%)",
                            format_transfer_bytes(total)
                        ));
                        ui.add(
                            egui::ProgressBar::new(
                                (progress.bytes_received as f32 / total.max(1) as f32)
                                    .clamp(0.0, 1.0),
                            )
                            .show_percentage(),
                        );
                    } else {
                        ui.label(format!("Received {received}; server size is unknown."));
                    }
                }
                if progress.phase == CheatSourceProgressPhase::Retrying {
                    ui.label(format!(
                        "Retrying after {} seconds.",
                        progress.retry_delay_seconds.unwrap_or_default()
                    ));
                }
            }
            if widgets::action_button(
                ui,
                "Cancel",
                widgets::ActionStyle::Secondary,
                !running.cancellation_requested,
            )
            .clicked()
            {
                action = Some(CatalogueManagerAction::CancelRunning);
            }
        });
    }
    if let Some(result) = last_result {
        match result {
            Ok(fetch) => widgets::banner(
                ui,
                "Cheat database updated",
                &format!(
                    "Revision {} is active; {} files were verified. No cheats were installed.",
                    fetch.manifest.resolved_revision,
                    fetch.manifest.files.len()
                ),
                widgets::StatusTone::Success,
            ),
            Err(error) => widgets::failure_summary(
                ui,
                "catalogue_result_error",
                cheat_source_error_headline(error),
                Some("Your existing cheat database, if any, remains active and usable."),
                &error.to_string(),
            ),
        }
    }
    if let Some(review) = review {
        let mut open = true;
        widgets::centered_window(match review.kind {
            CatalogueRetrievalKind::Download => "Review catalogue download",
            CatalogueRetrievalKind::Update => "Review catalogue update",
        })
        .resizable(false)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.label("Network access begins only after you confirm this exact request.");
            ui.label(format!("Provider: {}", review.source_id));
            if let Ok(root) = default_cheat_source_cache_root() {
                ui.label(format!("Managed destination: {}", root.display()));
            }
            ui.label("The master reference will be resolved to an exact commit, then an immutable HTTPS archive will be downloaded and verified.");
            ui.horizontal(|ui| {
                if ui.button("Confirm retrieval").clicked() {
                    action = Some(CatalogueManagerAction::Confirm);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(CatalogueManagerAction::CancelReview);
                }
            });
        });
        if !open {
            action = Some(CatalogueManagerAction::CancelReview);
        }
    }
    action
}

/// Plain-language headline for a failed catalogue fetch/update. `error.code`
/// values like "download_too_large" or "revision_response_invalid" are
/// internal identifiers meant for the Technical details disclosure (already
/// shown separately via `error.to_string()`), never primary UI text - a
/// live-QA finding (Phase 8) reported seeing the raw code and byte counts
/// directly in the failure banner.
pub(super) fn cheat_source_error_headline(error: &CheatSourceError) -> &'static str {
    match error.code.as_str() {
        "cancelled" => "Cheat database download cancelled",
        "download_too_large" => {
            "Cheat database update could not finish because the download was larger than the current safety limit"
        }
        _ => "Cheat database update failed",
    }
}

/// Plain-language explanation shown as a failed catalogue entry's banner
/// body; the exact `error.to_string()` (internal code plus byte counts)
/// moves to a Technical details disclosure alongside it instead.
pub(super) fn cheat_source_error_human_detail(error: &CheatSourceError) -> &'static str {
    match error.code.as_str() {
        "cancelled" => "The download was cancelled.",
        "download_too_large" => {
            "The download was larger than the current safety limit and could not finish."
        }
        _ => "An unexpected error occurred while updating the cheat database.",
    }
}

/// Plain-language reason for one excluded catalogue file, shown next to its
/// path in the Technical details disclosure. Replaces raw `{:?}` Debug
/// formatting of `CheatSourceExclusionKind` (e.g. "MalformedCht: ..."),
/// which a live-QA finding (Phase 8) flagged as reading like an internal
/// error code rather than something a person could understand.
pub(super) fn cheat_source_exclusion_reason(kind: CheatSourceExclusionKind) -> &'static str {
    match kind {
        CheatSourceExclusionKind::MalformedCht => "could not be read",
        CheatSourceExclusionKind::UnsupportedContentEncoding => {
            "is not UTF-8 text and could not be read"
        }
        CheatSourceExclusionKind::UnsupportedPathEncoding => {
            "has a file name that could not be read safely"
        }
        CheatSourceExclusionKind::UnsupportedContent => "is not a recognised cheat file",
    }
}

pub(super) fn catalogue_progress_label(phase: CheatSourceProgressPhase) -> &'static str {
    match phase {
        CheatSourceProgressPhase::ResolvingRevision => "Resolving exact revision",
        CheatSourceProgressPhase::Connecting => "Connecting",
        CheatSourceProgressPhase::Downloading => "Downloading",
        CheatSourceProgressPhase::Retrying => "Retrying",
        CheatSourceProgressPhase::VerifyingArchive => "Verifying archive digest",
        CheatSourceProgressPhase::Extracting => "Extracting safely",
        CheatSourceProgressPhase::VerifyingFiles => "Verifying catalogue files",
        CheatSourceProgressPhase::Activating => "Activating verified snapshot",
        CheatSourceProgressPhase::Cancelled => "Cancelled",
    }
}

pub(super) fn format_transfer_bytes(bytes: u64) -> String {
    const MIB: f64 = (1024 * 1024) as f64;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

// ---------------------------------------------------------------------
// Dolphin cheat catalogue management card (Cheats & Mods)
// ---------------------------------------------------------------------

/// A snapshot cheap enough to hold in `App` state: the parsed catalogue (if
/// any) plus the last update-check timestamp, loaded together by one
/// background read so the card never shows one without the other.
pub(super) struct DolphinCatalogueStatusSnapshot {
    pub(super) catalogue: Option<DolphinCatalogue>,
    pub(super) last_check_unix_seconds: Option<u64>,
}

pub(super) enum DolphinCatalogueManagerState {
    NotLoaded,
    Loading(Receiver<Result<DolphinCatalogueStatusSnapshot, DolphinCatalogueError>>),
    Ready(Box<DolphinCatalogueStatusSnapshot>),
    Failed(DolphinCatalogueError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DolphinCatalogueRetrievalKind {
    Download,
    Update,
    /// Re-parses the archive already pinned to the active commit, without
    /// checking upstream for a newer one.
    Rebuild,
}

pub(super) struct RunningDolphinCatalogueRetrieval {
    pub(super) generation: u64,
    pub(super) kind: DolphinCatalogueRetrievalKind,
    pub(super) cancellation: CheatSourceCancellation,
    pub(super) receiver: Receiver<Result<DolphinCatalogueFetchResult, DolphinCatalogueError>>,
    pub(super) progress_receiver: Receiver<CheatSourceProgress>,
    pub(super) progress: Option<CheatSourceProgress>,
    pub(super) cancellation_requested: bool,
}

pub(super) enum DolphinCatalogueManagerAction {
    Refresh,
    Review(DolphinCatalogueRetrievalKind),
    Confirm,
    CancelReview,
    CancelRunning,
    CheckForUpdates,
    RequestRemove,
    ConfirmRemove,
    CancelRemove,
}

pub(super) fn dolphin_catalogue_retrieval_kind_verb(
    kind: DolphinCatalogueRetrievalKind,
) -> &'static str {
    match kind {
        DolphinCatalogueRetrievalKind::Download => "download",
        DolphinCatalogueRetrievalKind::Update => "update",
        DolphinCatalogueRetrievalKind::Rebuild => "rebuild",
    }
}

/// Renders the beginner-facing Dolphin cheat catalogue card described in
/// the Dolphin cheat catalogue design: no-catalogue prompt, downloading
/// progress, ready summary, update-available affordance, and an honest
/// failure banner that never exposes raw transport errors at this level
/// (those live under `widgets::technical_details`).
/// Groups `show_dolphin_catalogue_manager`'s small "current moment" values
/// (as opposed to the state/running/result data it also needs) so the
/// function stays under the usual argument-count limit.
pub(super) struct DolphinCatalogueCardContext {
    pub(super) review: Option<DolphinCatalogueRetrievalKind>,
    pub(super) update_available: Option<bool>,
    pub(super) remove_confirm: bool,
    pub(super) now_unix_seconds: u64,
}

pub(super) fn show_dolphin_catalogue_manager(
    ui: &mut egui::Ui,
    state: &DolphinCatalogueManagerState,
    running: Option<&RunningDolphinCatalogueRetrieval>,
    last_result: Option<&Result<DolphinCatalogueFetchResult, DolphinCatalogueError>>,
    context: DolphinCatalogueCardContext,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<DolphinCatalogueManagerAction> {
    let DolphinCatalogueCardContext {
        review,
        update_available,
        remove_confirm,
        now_unix_seconds,
    } = context;
    let mut action = None;
    widgets::section_header(
        ui,
        "Dolphin cheat catalogue",
        Some(
            "A locally cached index of Gecko cheat definitions from the official Dolphin upstream project - downloaded once, searched instantly offline afterwards.",
        ),
    );
    let idle = running.is_none() && review.is_none() && !remove_confirm;
    match state {
        DolphinCatalogueManagerState::NotLoaded | DolphinCatalogueManagerState::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking Dolphin cheat catalogue status…");
            });
        }
        DolphinCatalogueManagerState::Failed(error) => {
            widgets::banner(
                ui,
                "Catalogue status unavailable",
                &error.to_string(),
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Retry", widgets::ActionStyle::Secondary, true).clicked()
            {
                action = Some(DolphinCatalogueManagerAction::Refresh);
            }
        }
        DolphinCatalogueManagerState::Ready(snapshot) => {
            widgets::card(ui, |ui| match &snapshot.catalogue {
                None => {
                    widgets::status_strip(
                        ui,
                        &[(
                            "Dolphin cheat catalogue not downloaded",
                            widgets::StatusTone::Pending,
                        )],
                    );
                    if widgets::action_button(
                        ui,
                        "Download catalogue",
                        widgets::ActionStyle::Primary,
                        idle,
                    )
                    .clicked()
                    {
                        action = Some(DolphinCatalogueManagerAction::Review(
                            DolphinCatalogueRetrievalKind::Download,
                        ));
                    }
                }
                Some(catalogue) => {
                    let stale = catalogue.metadata.is_stale(now_unix_seconds);
                    let show_update = stale || update_available == Some(true);
                    let tone = if show_update {
                        widgets::StatusTone::Warning
                    } else {
                        widgets::StatusTone::Success
                    };
                    widgets::status_strip(
                        ui,
                        &[(
                            if show_update {
                                "Update available"
                            } else {
                                "Dolphin catalogue ready"
                            },
                            tone,
                        )],
                    );
                    ui.label(format!(
                        "{} games · {} cheats",
                        catalogue.games.len(),
                        catalogue.metadata.total_usable_gecko_entries
                    ));
                    ui.label(format!(
                        "Updated {}",
                        format_unix_timestamp_utc(
                            catalogue.metadata.fetched_at_unix_seconds as i64
                        )
                    ));
                    if catalogue.metadata.malformed_or_skipped_files > 0 {
                        ui.label(format!(
                            "{} upstream file(s) had no usable Gecko codes.",
                            catalogue.metadata.malformed_or_skipped_files
                        ));
                    }
                    ui.horizontal_wrapped(|ui| {
                        if widgets::action_button(
                            ui,
                            "Update catalogue",
                            widgets::ActionStyle::Primary,
                            idle,
                        )
                        .clicked()
                        {
                            action = Some(DolphinCatalogueManagerAction::Review(
                                DolphinCatalogueRetrievalKind::Update,
                            ));
                        }
                        if widgets::action_button(
                            ui,
                            "Check for updates",
                            widgets::ActionStyle::Secondary,
                            idle,
                        )
                        .clicked()
                        {
                            action = Some(DolphinCatalogueManagerAction::CheckForUpdates);
                        }
                        if widgets::action_button(
                            ui,
                            "Rebuild local index",
                            widgets::ActionStyle::Secondary,
                            idle,
                        )
                        .clicked()
                        {
                            action = Some(DolphinCatalogueManagerAction::Review(
                                DolphinCatalogueRetrievalKind::Rebuild,
                            ));
                        }
                        if widgets::action_button(
                            ui,
                            "Remove downloaded catalogue",
                            widgets::ActionStyle::Secondary,
                            idle,
                        )
                        .clicked()
                        {
                            action = Some(DolphinCatalogueManagerAction::RequestRemove);
                        }
                    });
                    if !catalogue.metadata.warnings.is_empty() {
                        show_cheat_warnings_summary(
                            ui,
                            &catalogue.metadata.warnings,
                            (
                                "dolphin_catalogue_warnings",
                                &catalogue.metadata.resolved_commit,
                            ),
                            clipboard,
                        );
                    }
                    widgets::technical_details(
                        ui,
                        (
                            "dolphin_catalogue_technical_details",
                            &catalogue.metadata.resolved_commit,
                        ),
                        |ui| {
                            widgets::copyable_value(
                                ui,
                                "Repository",
                                &catalogue.metadata.canonical_repository_url,
                            );
                            widgets::copyable_value(
                                ui,
                                "Resolved commit",
                                &catalogue.metadata.resolved_commit,
                            );
                            widgets::copyable_value(
                                ui,
                                "Source archive",
                                &catalogue.metadata.source_archive_url,
                            );
                            widgets::copyable_value(
                                ui,
                                "Archive SHA-256",
                                &catalogue.metadata.archive_sha256,
                            );
                            ui.label(format!(
                                "Downloaded: {}",
                                format_transfer_bytes(catalogue.metadata.downloaded_bytes)
                            ));
                            ui.label(format!(
                                "GameSettings files inspected: {}",
                                catalogue.metadata.game_settings_files_inspected
                            ));
                            ui.label(format!(
                                    "Non-matching files skipped (wildcard names, non-GameSettings paths): {}",
                                    catalogue.metadata.non_matching_files_skipped
                                ));
                            ui.label(format!("Licence: {}", catalogue.metadata.license));
                            ui.label(catalogue.metadata.attribution.clone());
                            if let Some(timestamp) = snapshot.last_check_unix_seconds {
                                ui.label(format!(
                                    "Last update check: {}",
                                    format_unix_timestamp_utc(timestamp as i64)
                                ));
                            }
                        },
                    );
                }
            });
        }
    }
    if let Some(running) = running {
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(if running.cancellation_requested {
                    "Cancellation requested; the active catalogue will remain unchanged."
                } else {
                    match running.kind {
                        DolphinCatalogueRetrievalKind::Download => {
                            "Downloading Dolphin cheat catalogue…"
                        }
                        DolphinCatalogueRetrievalKind::Update => {
                            "Updating Dolphin cheat catalogue…"
                        }
                        DolphinCatalogueRetrievalKind::Rebuild => {
                            "Rebuilding the local Dolphin cheat index…"
                        }
                    }
                });
            });
            if let Some(progress) = &running.progress {
                ui.horizontal_wrapped(|ui| {
                    widgets::status_badge(
                        ui,
                        catalogue_progress_label(progress.phase),
                        if progress.phase == CheatSourceProgressPhase::Retrying {
                            widgets::StatusTone::Warning
                        } else {
                            widgets::StatusTone::Info
                        },
                    );
                });
                if progress.phase == CheatSourceProgressPhase::Downloading {
                    let received = format_transfer_bytes(progress.bytes_received);
                    ui.label(format!("Received {received}."));
                }
            }
            if widgets::action_button(
                ui,
                "Cancel",
                widgets::ActionStyle::Secondary,
                !running.cancellation_requested,
            )
            .clicked()
            {
                action = Some(DolphinCatalogueManagerAction::CancelRunning);
            }
        });
    }
    if let Some(result) = last_result {
        match result {
            Ok(fetch) => widgets::banner(
                ui,
                "Dolphin catalogue ready",
                &format!(
                    "{} games · {} cheats. Updated {}.",
                    fetch.catalogue.games.len(),
                    fetch.catalogue.metadata.total_usable_gecko_entries,
                    format_unix_timestamp_utc(
                        fetch.catalogue.metadata.fetched_at_unix_seconds as i64
                    )
                ),
                widgets::StatusTone::Success,
            ),
            Err(error) => widgets::failure_summary(
                ui,
                "dolphin_catalogue_result_error",
                if error.kind == DolphinCatalogueErrorKind::Cancelled {
                    "Catalogue download cancelled"
                } else {
                    "Could not update the catalogue"
                },
                Some("Your existing catalogue is still available."),
                &error.to_string(),
            ),
        }
    }
    if let Some(kind) = review {
        let mut open = true;
        widgets::centered_window(format!(
            "{}{} the Dolphin cheat catalogue?",
            dolphin_catalogue_retrieval_kind_verb(kind)[..1].to_uppercase(),
            &dolphin_catalogue_retrieval_kind_verb(kind)[1..]
        ))
        .resizable(false)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            ui.label("Network access begins only after you confirm this exact request.");
            if let Ok(root) = default_dolphin_catalogue_cache_root() {
                ui.label(format!("Managed destination: {}", root.display()));
            }
            ui.label("This only affects EmuWiz's own catalogue cache. Your Dolphin profile and installed codes are never touched by this action.");
            ui.horizontal(|ui| {
                if ui.button("Confirm").clicked() {
                    action = Some(DolphinCatalogueManagerAction::Confirm);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(DolphinCatalogueManagerAction::CancelReview);
                }
            });
        });
        if !open {
            action = Some(DolphinCatalogueManagerAction::CancelReview);
        }
    }
    if remove_confirm {
        let mut open = true;
        widgets::centered_window("Remove downloaded catalogue?")
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.label("This removes only EmuWiz's own catalogue cache.");
                ui.label("It never removes installed Dolphin codes and never alters your Dolphin User/GameSettings files.");
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        action = Some(DolphinCatalogueManagerAction::ConfirmRemove);
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(DolphinCatalogueManagerAction::CancelRemove);
                    }
                });
            });
        if !open {
            action = Some(DolphinCatalogueManagerAction::CancelRemove);
        }
    }
    action
}

/// One-shot gate mirroring `catalogue_status_load_needed`: load the status
/// snapshot the first time the Cheats & Mods Dolphin workflow needs it.
pub(super) fn dolphin_catalogue_status_load_needed(
    view: MainView,
    state: &DolphinCatalogueManagerState,
) -> bool {
    view == MainView::CheatsMods && matches!(state, DolphinCatalogueManagerState::NotLoaded)
}

/// The cheat-database readiness summary the Sources Overview shows -
/// derived entirely from state `show_retroarch_catalogue_manager` (the
/// card rendered lower on the page) already owns, never a second,
/// independently-drifting classification. `running` takes priority over
/// `state`'s own per-entry status, since a retrieval in progress is the
/// most current, most actionable fact.
pub(super) fn sources_catalogue_overview_status(
    state: &CatalogueManagerState,
    running: Option<&RunningCatalogueRetrieval>,
) -> (String, widgets::StatusTone) {
    if let Some(running) = running {
        return if running.cancellation_requested {
            (
                "Cheat database update: cancelling".to_string(),
                widgets::StatusTone::Warning,
            )
        } else {
            (
                "Cheat database update in progress".to_string(),
                widgets::StatusTone::Active,
            )
        };
    }
    match state {
        CatalogueManagerState::NotLoaded | CatalogueManagerState::Loading(_) => (
            "Checking cheat database status…".to_string(),
            widgets::StatusTone::Pending,
        ),
        CatalogueManagerState::Failed(_) => (
            "Cheat database status unavailable".to_string(),
            widgets::StatusTone::Blocked,
        ),
        CatalogueManagerState::Ready(list) => {
            let worst = list
                .entries
                .iter()
                .map(|entry| entry.status)
                .max_by_key(|status| match status {
                    CheatCatalogueStatus::Ready => 0,
                    CheatCatalogueStatus::ReadyWithWarnings => 1,
                    CheatCatalogueStatus::Stale | CheatCatalogueStatus::Cancelled => 2,
                    CheatCatalogueStatus::Missing => 3,
                    _ => 4,
                });
            match worst {
                None => (
                    "No cheat database provider configured".to_string(),
                    widgets::StatusTone::Pending,
                ),
                Some(status) => {
                    let tone = match status {
                        CheatCatalogueStatus::Ready => widgets::StatusTone::Success,
                        CheatCatalogueStatus::ReadyWithWarnings
                        | CheatCatalogueStatus::Stale
                        | CheatCatalogueStatus::Cancelled => widgets::StatusTone::Warning,
                        CheatCatalogueStatus::Missing => widgets::StatusTone::Pending,
                        _ => widgets::StatusTone::Blocked,
                    };
                    (
                        format!("Cheat database: {}", catalogue_status_label(status)),
                        tone,
                    )
                }
            }
        }
    }
}

/// The Sources page's Overview: how many source folders are configured
/// and their availability at a glance, plus the cheat database's current
/// readiness - both summarised from data the sections below already show
/// in full detail (the configured-source list and the RetroArch cheat
/// catalogue card), never a third, independently-computed source of
/// truth.
pub(super) fn show_sources_overview(
    ui: &mut egui::Ui,
    sources: &[SourceFolderView],
    catalogue_available: bool,
    catalogue_manager: &CatalogueManagerState,
    catalogue_retrieval: Option<&RunningCatalogueRetrieval>,
) {
    widgets::section_header(
        ui,
        "Overview",
        Some("Configured source folders and cheat database readiness at a glance."),
    );
    widgets::card(ui, |ui| {
        let last_scan_succeeded = sources
            .iter()
            .filter(|source| {
                source.enabled
                    && source.availability == SourceAvailability::Available
                    && source.last_scan_status == Some(archivefs_core::SourceScanStatus::Success)
            })
            .count();
        let not_scanned_yet = sources
            .iter()
            .filter(|source| source.enabled && source.last_scan_status.is_none())
            .count();
        let disabled = sources.iter().filter(|source| !source.enabled).count();
        let blocked = sources
            .iter()
            .filter(|source| {
                source.enabled
                    && matches!(
                        source.availability,
                        SourceAvailability::Unavailable
                            | SourceAvailability::PermissionDenied
                            | SourceAvailability::ScanFailed
                    )
            })
            .count();
        ui.label(format!(
            "{} configured source folder{}",
            sources.len(),
            if sources.len() == 1 { "" } else { "s" }
        ));
        if !catalogue_available {
            ui.weak("Catalogue unavailable; scan history and archive counts could not be loaded.");
        }
        let mut items: Vec<(String, widgets::StatusTone)> = vec![(
            format!("{last_scan_succeeded} last scan succeeded"),
            widgets::StatusTone::Success,
        )];
        if not_scanned_yet > 0 {
            items.push((
                format!("{not_scanned_yet} not scanned yet"),
                widgets::StatusTone::Pending,
            ));
        }
        if disabled > 0 {
            items.push((format!("{disabled} disabled"), widgets::StatusTone::Pending));
        }
        if blocked > 0 {
            items.push((format!("{blocked} blocked"), widgets::StatusTone::Blocked));
        }
        let (catalogue_label, catalogue_tone) =
            sources_catalogue_overview_status(catalogue_manager, catalogue_retrieval);
        items.push((catalogue_label, catalogue_tone));
        let item_refs: Vec<(&str, widgets::StatusTone)> = items
            .iter()
            .map(|(label, tone)| (label.as_str(), *tone))
            .collect();
        widgets::status_strip(ui, &item_refs);
    });
}

/// A friendly local display label, deliberately derived from the configured
/// path rather than persisted as a second source identity.
fn game_folder_title(path: &Path) -> String {
    path.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Game folder".to_string())
}

/// The page has no live reachability probe. These labels describe only the
/// configured state and the last known scan result already in SourceFolderView.
pub(super) fn game_source_status(
    view: &SourceFolderView,
) -> (
    &'static str,
    widgets::StatusTone,
    &'static str,
    &'static str,
) {
    if !view.enabled {
        return (
            "Disabled",
            widgets::StatusTone::Pending,
            "This folder is paused. Its previous library entries are kept.",
            "Enable and scan",
        );
    }
    match view.availability {
        SourceAvailability::Available if view.last_scan_status.is_none() => (
            "Not scanned yet",
            widgets::StatusTone::Pending,
            "This folder is configured, but EmuWiz has not scanned it yet.",
            "Scan now",
        ),
        SourceAvailability::Available => (
            "Last scan succeeded",
            widgets::StatusTone::Success,
            "This is the last known scan result; opening this page does not check the folder again.",
            "Rescan",
        ),
        SourceAvailability::Unavailable => (
            "Folder unavailable at last scan",
            widgets::StatusTone::Blocked,
            "Connect or mount this folder, then try again. Your previous library entries are kept.",
            "Try again",
        ),
        SourceAvailability::PermissionDenied => (
            "Permission needed",
            widgets::StatusTone::Blocked,
            "EmuWiz could not read this folder. Check its permissions, then try again. Your previous library entries are kept.",
            "Try again",
        ),
        SourceAvailability::ScanFailed => (
            "Scan needs attention",
            widgets::StatusTone::Blocked,
            "The last scan could not finish. Try again; scan details are available if it continues.",
            "Try again",
        ),
        SourceAvailability::Disabled => unreachable!("disabled sources return above"),
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
pub(super) fn show_sources_page(
    ui: &mut egui::Ui,
    sources: &[SourceFolderView],
    archives: &[PersistedArchive],
    mount_root: Option<&Path>,
    busy: bool,
    add_dialog: &mut Option<SourcesAddDialogState>,
    remove_dialog: &mut Option<SourcesRemoveDialogState>,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<SourcesPageAction> {
    let mut mount_root_draft = None;
    show_sources_page_with_mount_root(
        ui,
        sources,
        archives,
        mount_root,
        true,
        busy,
        &mut mount_root_draft,
        false,
        None,
        add_dialog,
        remove_dialog,
        clipboard,
    )
}

pub(super) fn stage_mount_root_pick(draft: &mut Option<PathBuf>, picked: Option<PathBuf>) {
    if let Some(path) = picked {
        *draft = Some(path);
    }
}

pub(super) fn mount_root_apply_available(draft: Option<&Path>, busy: bool) -> bool {
    draft.is_some() && !busy
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show_sources_page_with_mount_root(
    ui: &mut egui::Ui,
    sources: &[SourceFolderView],
    archives: &[PersistedArchive],
    mount_root: Option<&Path>,
    catalogue_available: bool,
    busy: bool,
    mount_root_draft: &mut Option<PathBuf>,
    mount_root_busy: bool,
    mount_root_feedback: Option<&MountRootFeedback>,
    add_dialog: &mut Option<SourcesAddDialogState>,
    remove_dialog: &mut Option<SourcesRemoveDialogState>,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<SourcesPageAction> {
    let mut action = None;

    // No `widgets::page_header` here any more: the Sources page's one
    // "Sources" heading now lives at the app-level call site, above the
    // Overview section this function's content follows - see the
    // `MainView::Sources` dispatch in `update`. Nothing here changed
    // otherwise; this function keeps its exact signature and every
    // existing test that calls it directly still gets the same content,
    // just without the now-redundant page-level heading repeating.
    widgets::section_header(
        ui,
        "Game sources",
        Some(
            "A game source is a folder where your games already live. EmuWiz reads it to build your library; it never moves, renames, or deletes your files.",
        ),
    );

    if !catalogue_available {
        widgets::card(ui, |ui| {
            ui.weak("Catalogue unavailable. Configured folders remain visible, but scan status and archive counts are unavailable.");
        });
        ui.add_space(12.0);
    }

    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(ui, "Add game folder", widgets::ActionStyle::Primary, !busy)
                .clicked()
            {
                *add_dialog = Some(SourcesAddDialogState::default());
            }
            if sources.iter().filter(|source| source.enabled).count() >= 2
                && widgets::action_button(
                    ui,
                    "Scan all enabled",
                    widgets::ActionStyle::Secondary,
                    !busy,
                )
                .clicked()
            {
                action = Some(SourcesPageAction::ScanAll);
            }
            if widgets::action_button(
                ui,
                "Reload saved status",
                widgets::ActionStyle::Quiet,
                !busy,
            )
            .clicked()
            {
                action = Some(SourcesPageAction::RefreshStatus);
            }
        })
        .inner
    });
    ui.add_space(12.0);

    if sources.is_empty() {
        if widgets::empty_state(
            ui,
            "Add your first game folder",
            "Choose the existing readable folder where your games live. EmuWiz will add it without reorganising or changing any files; then you can scan it to build your library.",
            Some("Add game folder"),
        ) {
            *add_dialog = Some(SourcesAddDialogState::default());
        }
    } else {
        // The containing Sources page already owns the vertical scrollbar.
        // Keeping this list in that shared flow avoids a short nested scroll
        // viewport that hid later source cards while the page still had room.
        ui.vertical(|ui| {
            for view in sources {
                widgets::card(ui, |ui| {
                        let (status, status_tone, guidance, primary_action) =
                            game_source_status(view);
                        ui.strong(format!("Game folder: {}", game_folder_title(&view.path)));
                        widgets::status_strip(
                            ui,
                            &[(status, status_tone), ("Configured", widgets::StatusTone::Info)],
                        );
                        ui.add_space(4.0);
                        ui.weak("Folder path");
                        if widgets::path_value(ui, "Path", &view.path) {
                                    let _ = clipboard.set_text(view.path.display().to_string());
                        }
                        ui.label(egui::RichText::new(guidance).color(theme::muted(ui)));
                        ui.add_space(4.0);
                        egui::Grid::new(("source_facts_grid", &view.path))
                            .num_columns(2)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.weak("Archive entries found:");
                                ui.label(
                                    view.last_archive_count
                                        .map(|count| count.to_string())
                                        .unwrap_or_else(|| "No scan results yet".to_string()),
                                );
                                ui.end_row();
                                ui.weak("Last scan:");
                                ui.label(view.last_scan_at.as_deref().unwrap_or("Not scanned yet"));
                                ui.end_row();
                                ui.label("Platform:");
                                ui.label(source_platform_value_label(&source_platform_state(view, archives)));
                                ui.end_row();
                            });
                        if view.last_archive_count == Some(0)
                            && view.last_scan_status
                                == Some(archivefs_core::SourceScanStatus::Success)
                        {
                            ui.label("The scan finished, but found no supported game items here. Check that this is the folder containing your games, then view scan details or choose another folder.");
                        }
                        if let Some(error) = &view.last_scan_error {
                            widgets::technical_details(ui, ("source_scan_error", &view.path), |ui| {
                                ui.label(error);
                            });
                        }
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            if widgets::action_button(ui, primary_action, widgets::ActionStyle::Primary, !busy).clicked() {
                                if view.enabled {
                                    action = Some(SourcesPageAction::ScanOne(view.path.clone()));
                                } else {
                                    action = Some(SourcesPageAction::SetEnabled { path: view.path.clone(), enabled: true });
                                }
                            }
                            if widgets::action_button(ui, "View scan details", widgets::ActionStyle::Secondary, true).clicked() {
                                action = Some(SourcesPageAction::ViewScanDetails);
                            }
                            if view.enabled
                                && widgets::action_button(ui, "Disable", widgets::ActionStyle::Quiet, !busy).clicked()
                            {
                                action = Some(SourcesPageAction::SetEnabled { path: view.path.clone(), enabled: false });
                            }
                            if widgets::action_button(ui, "Remove from EmuWiz", widgets::ActionStyle::Destructive, !busy).clicked() {
                                *remove_dialog = Some(SourcesRemoveDialogState {
                                    path: view.path.clone(),
                                    last_archive_count: view.last_archive_count,
                                    keep_catalogue: true,
                                });
                            }
                            ui.menu_button("More", |ui| {
                                if ui.button("View items in library").clicked() {
                                    action = Some(SourcesPageAction::ViewInLibrary(view.path.clone()));
                                    ui.close();
                                }
                                if ui.button("Copy folder path").clicked() {
                                    let _ = clipboard.set_text(view.path.display().to_string());
                                    ui.close();
                                }
                                ui.separator();
                                ui.label(format!(
                                    "Preview: up to {} Unknown entries can be updated on rescan.",
                                    view.unknown_archive_count
                                        ));
                                        if let Some(current) = &view.assigned_platform {
                                            ui.label(format!("Current: {current}"));
                                        }
                                        if let Some(platform) = widgets::platform_picker(
                                            ui,
                                            ("sources_assign_platform_menu", &view.path),
                                            &canonical_platform_names(),
                                            view.assigned_platform.as_deref(),
                                            !busy,
                                        ) {
                                            action = Some(SourcesPageAction::AssignPlatform {
                                                path: view.path.clone(),
                                                platform: platform.to_string(),
                                            });
                                            ui.close();
                                        }
                                        ui.small("Incompatible direct images remain Unknown.");
                                    });
                        });
                });
                ui.add_space(theme::SPACE_SM);
            }
        });
    }

    ui.add_space(theme::SECTION_GAP);
    widgets::section_header(
        ui,
        "Mount destination",
        Some("Choose where EmuWiz temporarily prepares archived games."),
    );
    widgets::card(ui, |ui| {
        ui.strong("Temporary game preparation folder");
        if let Some(root) = mount_root {
            if widgets::path_value(ui, "Mount root", root) {
                let _ = clipboard.set_text(root.display().to_string());
            }
        } else {
            ui.label("Mount root: unknown");
        }
        ui.label(
            egui::RichText::new(
                "EmuWiz temporarily makes archived games available here while you play. It does not rewrite your original game files.",
            )
            .color(theme::muted(ui)),
        );
        ui.label(
            egui::RichText::new(
                "Choose an existing writable folder. EmuWiz will not create it automatically.",
            )
            .color(theme::muted(ui)),
        );
        ui.horizontal(|ui| {
            if widgets::action_button(
                ui,
                "Choose folder",
                widgets::ActionStyle::Secondary,
                !mount_root_busy,
            )
            .clicked()
            {
                let picked = rfd::FileDialog::new()
                    .set_title("Choose Temporary Game Preparation Folder")
                    .pick_folder();
                stage_mount_root_pick(mount_root_draft, picked);
            }
            if mount_root_apply_available(mount_root_draft.as_deref(), mount_root_busy) {
                if widgets::action_button(ui, "Apply folder", widgets::ActionStyle::Primary, true)
                    .clicked()
                    && let Some(path) = mount_root_draft.clone()
                {
                    action = Some(SourcesPageAction::ApplyMountRoot(path));
                }
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Quiet, true).clicked()
                {
                    *mount_root_draft = None;
                }
            }
        });
        if let Some(draft) = mount_root_draft.as_deref()
            && widgets::path_value(ui, "Selected folder", draft)
        {
            let _ = clipboard.set_text(draft.display().to_string());
        }
        if let Some(feedback) = mount_root_feedback {
            let color = if feedback.succeeded {
                egui::Color32::from_rgb(70, 170, 90)
            } else {
                ui.visuals().error_fg_color
            };
            ui.colored_label(color, &feedback.summary);
            if let Some(warning) = &feedback.warning {
                ui.colored_label(ui.visuals().warn_fg_color, warning);
            }
            if let Some(detail) = &feedback.detail {
                widgets::technical_details(
                    ui,
                    ("sources_mount_root_feedback_detail", detail.as_str()),
                    |ui| {
                        ui.label(detail);
                    },
                );
            }
        }
    });

    if let Some(dialog) = add_dialog.as_mut() {
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new("Add Folder")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.label("Add an existing, readable directory as an EmuWiz source folder.");
                ui.horizontal(|ui| {
                    ui.label("Path:");
                    show_text_edit_with_context_menu(
                        ui,
                        &mut dialog.path_text,
                        clipboard,
                        |text_edit| {
                            text_edit
                                .id_salt("sources_add_dialog_path")
                                .desired_width(320.0)
                        },
                    );
                    // rfd's `pick_folder` is synchronous and returns
                    // `None` on cancellation or picker failure - never
                    // panics, and a cancelled pick leaves whatever the
                    // user had already typed untouched.
                    if ui.button("Browse...").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_title("Select Archive Source Folder")
                            .pick_folder()
                    {
                        dialog.path_text = path.display().to_string();
                        dialog.validation_message = None;
                    }
                });
                if let Some(message) = &dialog.validation_message {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy && !dialog.path_text.trim().is_empty(),
                            egui::Button::new("Add"),
                        )
                        .clicked()
                    {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if submit {
            let candidate = PathBuf::from(dialog.path_text.trim());
            let existing: Vec<PathBuf> = sources.iter().map(|source| source.path.clone()).collect();
            match validate_new_source_folder(&candidate, &existing) {
                Ok(validated) => action = Some(SourcesPageAction::AddFolder(validated)),
                Err(error) => dialog.validation_message = Some(error.to_string()),
            }
        }
        if cancel || !open {
            *add_dialog = None;
        }
    }

    if let Some(dialog) = remove_dialog.clone() {
        let mut open = true;
        let mut confirmed = false;
        let mut cancel = false;
        let mut keep_catalogue = dialog.keep_catalogue;
        egui::Window::new("Remove this source from EmuWiz?")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.label("EmuWiz will not delete the folder or any files inside it.");
                ui.add_space(4.0);
                ui.strong(dialog.path.display().to_string());
                ui.add_space(4.0);
                ui.radio_value(
                    &mut keep_catalogue,
                    true,
                    "Keep catalogue entries (recommended)",
                );
                let remove_label = match dialog.last_archive_count {
                    Some(count) => {
                        format!("Remove catalogue entries belonging to this source ({count} rows)")
                    }
                    None => "Remove catalogue entries belonging to this source".to_string(),
                };
                ui.radio_value(&mut keep_catalogue, false, remove_label);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Remove Source"))
                        .clicked()
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if confirmed {
            action = Some(SourcesPageAction::ConfirmRemove {
                path: dialog.path.clone(),
                keep_catalogue,
            });
            *remove_dialog = None;
        } else if cancel || !open {
            *remove_dialog = None;
        } else if let Some(current) = remove_dialog.as_mut() {
            current.keep_catalogue = keep_catalogue;
        }
    }

    action
}

pub(super) fn bsfree_state_label(state: CheatProviderSourceState) -> &'static str {
    match state {
        CheatProviderSourceState::NotInstalled => "Not installed",
        CheatProviderSourceState::Downloading => "Downloading",
        CheatProviderSourceState::Validating => "Validating",
        CheatProviderSourceState::Ready => "Ready",
        CheatProviderSourceState::UpdateAvailable => "Update available",
        CheatProviderSourceState::Invalid => "Invalid",
        CheatProviderSourceState::UnsupportedSchema => "Unsupported schema",
        CheatProviderSourceState::DownloadFailed => "Download failed",
        CheatProviderSourceState::ValidationFailed => "Validation failed",
        CheatProviderSourceState::Disabled => "Disabled",
    }
}

pub(super) fn bsfree_state_tone(state: CheatProviderSourceState) -> widgets::StatusTone {
    match state {
        CheatProviderSourceState::Ready => widgets::StatusTone::Success,
        CheatProviderSourceState::Downloading | CheatProviderSourceState::Validating => {
            widgets::StatusTone::Active
        }
        CheatProviderSourceState::NotInstalled | CheatProviderSourceState::Disabled => {
            widgets::StatusTone::Pending
        }
        CheatProviderSourceState::UpdateAvailable
        | CheatProviderSourceState::DownloadFailed
        | CheatProviderSourceState::ValidationFailed => widgets::StatusTone::Warning,
        CheatProviderSourceState::Invalid | CheatProviderSourceState::UnsupportedSchema => {
            widgets::StatusTone::Blocked
        }
    }
}

pub(super) fn show_bsfree_source_card(
    ui: &mut egui::Ui,
    manager: &BsFreeManagerState,
    busy: bool,
    state: &mut BsFreeGuiState,
    clipboard: &mut dyn ClipboardBackend,
) -> Option<BsFreeOperation> {
    let mut action = None;
    widgets::section_header(
        ui,
        "BSFree Archive",
        Some("Optional third-party, read-only historical cheat catalogue."),
    );
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("BSFree Archive");
            match manager {
                BsFreeManagerState::NotLoaded => {
                    widgets::status_badge(ui, "Checking local state", widgets::StatusTone::Pending)
                }
                BsFreeManagerState::Ready(status) => widgets::status_badge(
                    ui,
                    bsfree_state_label(status.state),
                    bsfree_state_tone(status.state),
                ),
                BsFreeManagerState::Failed(_) => {
                    widgets::status_badge(ui, "Status unavailable", widgets::StatusTone::Warning)
                }
            }
            widgets::status_badge(
                ui,
                "GameCube/Wii cheats installable via Dolphin",
                widgets::StatusTone::Info,
            );
        });
        ui.label(
            "This catalogue covers all supported BSFree systems. GameCube and Wii cheats can be \
             installed with Dolphin; other formats remain browse only.",
        );
        widgets::technical_details(ui, "bsfree-source-provenance", |ui| {
            ui.label("Source: BSFree Archive");
            ui.label("Maintainer: Andrew Mackrodt");
            ui.label("Origin: Historical bsfree.org database");
            ui.label("Distribution status: Optional third-party download");
            ui.label("Verification: Historical community data, not verified by EmuWiz");
        });
        widgets::banner(
            ui,
            "Database-content licence not established",
            "The upstream application code is MIT, but EmuWiz does not claim that licence covers the historical cheat dataset.",
            widgets::StatusTone::Warning,
        );

        match manager {
            BsFreeManagerState::Ready(status) => {
                if widgets::path_value(ui, "Local destination", &status.database_path) {
                    let _ = clipboard.set_text(status.database_path.display().to_string());
                }
                if let Some(validation) = &status.validation {
                    widgets::status_strip(
                        ui,
                        &[
                            ("Immutable read-only SQLite", widgets::StatusTone::Success),
                            ("Query-only", widgets::StatusTone::Success),
                        ],
                    );
                    widgets::copyable_value(
                        ui,
                        "Database SHA-256",
                        &validation.result.source_fingerprint.sha256,
                    );
                    ui.label(format!(
                        "{} systems · {} games · {} codes · {} bytes",
                        validation.counts.systems,
                        validation.counts.games,
                        validation.counts.codes,
                        validation.result.source_fingerprint.size_bytes
                    ));
                    ui.label(format!(
                        "Validated: {}",
                        format_unix_timestamp_utc(
                            validation.result.validated_at_unix_seconds as i64
                        )
                    ));
                }
                if let Some(error) = &status.last_error {
                    widgets::banner(
                        ui,
                        "Last operation failed",
                        &error.message,
                        widgets::StatusTone::Warning,
                    );
                }
            }
            BsFreeManagerState::Failed(message) => widgets::banner(
                ui,
                "Could not inspect BSFree source",
                message,
                widgets::StatusTone::Warning,
            ),
            BsFreeManagerState::NotLoaded => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading local source metadata...");
                });
            }
        }

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if widgets::action_button(
                ui,
                "Download database",
                widgets::ActionStyle::Primary,
                !busy,
            )
            .clicked()
            {
                state.download_confirm = true;
            }
            if widgets::action_button(
                ui,
                "Validate",
                widgets::ActionStyle::Secondary,
                !busy
                    && matches!(manager, BsFreeManagerState::Ready(status) if status.fingerprint.is_some()),
            )
            .clicked()
            {
                action = Some(BsFreeOperation::Validate);
            }
            if let BsFreeManagerState::Ready(status) = manager {
                if widgets::action_button(
                    ui,
                    if status.enabled { "Disable" } else { "Enable" },
                    widgets::ActionStyle::Quiet,
                    !busy,
                )
                .clicked()
                {
                    action = Some(BsFreeOperation::SetEnabled(!status.enabled));
                }
                if widgets::action_button(
                    ui,
                    "Remove local copy",
                    widgets::ActionStyle::Destructive,
                    !busy && status.fingerprint.is_some(),
                )
                .clicked()
                {
                    state.remove_confirm = true;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Import local BSFree SQLite database");
            if widgets::action_button(
                ui,
                "Choose database file…",
                widgets::ActionStyle::Secondary,
                !busy,
            )
            .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose BSFree SQLite database")
                    .add_filter("SQLite database", &["db", "sqlite", "sqlite3"])
                    .pick_file()
            {
                state.import_path = path.display().to_string();
            }
            if !state.import_path.trim().is_empty() {
                ui.label(egui::RichText::new(state.import_path.trim()).color(theme::muted(ui)));
            }
            if widgets::action_button(
                ui,
                "Import local database",
                widgets::ActionStyle::Secondary,
                !busy && !state.import_path.trim().is_empty(),
            )
            .clicked()
            {
                action = Some(BsFreeOperation::Import(PathBuf::from(
                    state.import_path.trim(),
                )));
            }
        });
        widgets::technical_details(ui, "bsfree_import_manual_path", |ui| {
            ui.label(
                egui::RichText::new(
                    "Type or paste a path directly instead of using the file picker.",
                )
                .color(theme::muted(ui)),
            );
            ui.add(
                egui::TextEdit::singleline(&mut state.import_path)
                    .desired_width(360.0)
                    .hint_text("/path/to/bsfree.4cfee26.db"),
            );
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("BSFree operation in progress...");
            });
        }
        if state.download_confirm {
            widgets::banner(
                ui,
                "Download optional third-party database?",
                "Approximately 283 MiB. Network access is required. The database-content licence is not established, it will be stored in EmuWiz's data directory, and no cheats will be installed.",
                widgets::StatusTone::Warning,
            );
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    "Confirm download",
                    widgets::ActionStyle::Primary,
                    !busy,
                )
                .clicked()
                {
                    state.download_confirm = false;
                    action = Some(BsFreeOperation::Download);
                }
                if ui.button("Cancel").clicked() {
                    state.download_confirm = false;
                }
            });
        }
        if state.remove_confirm {
            widgets::banner(
                ui,
                "Remove EmuWiz's local BSFree copy?",
                "Only the BSFree database and EmuWiz-owned BSFree metadata are removed. Emulator profiles and other cheat providers are untouched.",
                widgets::StatusTone::Warning,
            );
            ui.horizontal(|ui| {
                if widgets::action_button(
                    ui,
                    "Confirm removal",
                    widgets::ActionStyle::Destructive,
                    !busy,
                )
                .clicked()
                {
                    state.remove_confirm = false;
                    action = Some(BsFreeOperation::Remove);
                }
                if ui.button("Cancel").clicked() {
                    state.remove_confirm = false;
                }
            });
        }
    });
    action
}

pub(super) fn bsfree_match_label(confidence: ProviderGameMatchConfidence) -> &'static str {
    match confidence {
        ProviderGameMatchConfidence::ExactHashPlatform => "Exact hash + platform",
        ProviderGameMatchConfidence::ExactSerialPlatformRegion => {
            "Exact serial + platform + region"
        }
        ProviderGameMatchConfidence::ExactUpstreamRelease => "Exact upstream release",
        ProviderGameMatchConfidence::ExactTitlePlatformRegionRevision => {
            "Exact title + platform + region + revision"
        }
        ProviderGameMatchConfidence::ExactTitlePlatform => "Exact title + platform",
        ProviderGameMatchConfidence::ProbableTitlePlatform => "Probable title + platform",
        ProviderGameMatchConfidence::Ambiguous => "Ambiguous candidates",
        ProviderGameMatchConfidence::NoMatch => "No match",
    }
}

pub(super) fn bsfree_compatibility_label(compatibility: DeviceFormatCompatibility) -> &'static str {
    match compatibility {
        DeviceFormatCompatibility::DirectlyInstallable => "Directly installable",
        DeviceFormatCompatibility::PotentiallyConvertible => "Potentially convertible",
        DeviceFormatCompatibility::ReferenceOnly => "Reference only",
        DeviceFormatCompatibility::Unsupported => "Unsupported",
        DeviceFormatCompatibility::Unknown => "Unknown format",
    }
}

/// Honest, per-code capability statement for a BSFree cheat row. GameCube
/// hex-pair codes classify as installable through the existing Dolphin
/// adapter (Gecko-equivalent or native Action Replay); every other platform
/// and every unsupported format stays reference-only. This never claims
/// EmuWiz has verified the BSFree database - it states only which formats
/// an existing adapter can represent.
pub(super) fn bsfree_code_capability(
    cheat: &BsFreeCheat,
    platform_id: Option<&str>,
) -> (&'static str, widgets::StatusTone) {
    if platform_id == Some("GameCube") {
        match classify_bsfree_gamecube_cheat(cheat).code_format {
            BsFreeGameCubeCodeFormat::GeckoEquivalent => {
                ("Supported by Dolphin (Gecko)", widgets::StatusTone::Success)
            }
            BsFreeGameCubeCodeFormat::ActionReplayNative => (
                "Supported by Dolphin (Action Replay)",
                widgets::StatusTone::Success,
            ),
            BsFreeGameCubeCodeFormat::Unsupported | BsFreeGameCubeCodeFormat::Malformed => (
                "Unsupported for this emulator",
                widgets::StatusTone::Pending,
            ),
        }
    } else {
        ("Reference only", widgets::StatusTone::Pending)
    }
}

fn bsfree_platform_capability(platform_id: Option<&str>) -> (&'static str, widgets::StatusTone) {
    match platform_id {
        Some("GameCube") | Some("Wii") => ("Installable via Dolphin", widgets::StatusTone::Success),
        Some(_) => ("Browse only", widgets::StatusTone::Pending),
        None => ("Unsupported / unmapped", widgets::StatusTone::Warning),
    }
}

fn bsfree_platform_display(system: &BsFreeSystem) -> &str {
    system
        .mapping
        .archivefs_platform_display_name
        .as_deref()
        .unwrap_or(&system.name)
}

pub(super) fn show_bsfree_game_browser(
    ui: &mut egui::Ui,
    manager: &BsFreeManagerState,
    busy: bool,
    state: &mut BsFreeGuiState,
    context: Option<&(PathBuf, String, String)>,
) -> Option<BsFreeOperation> {
    let mut action = None;
    if let Some((archive_path, title, platform)) = context
        && state.search_context.as_ref() != Some(archive_path)
    {
        state.search_context = Some(archive_path.clone());
        state.search_title.clone_from(title);
        state.search_platform.clone_from(platform);
        state.search_result = None;
        state.selected_game = None;
        state.cheats = None;
        state.search_system_id = None;
    }

    // Whether this browser was opened for a GameCube game. Only then is the
    // "installable via Dolphin" claim relevant to what is being browsed; for
    // any other platform (or no platform) the catalogue is browse-only, and
    // saying otherwise would imply this platform's cheats can be installed.
    let is_dolphin_browse = context.is_some_and(|(_, _, platform)| {
        matches!(
            archivefs_core::canonical_platform_for_alias(platform),
            Some("GameCube" | "Wii")
        )
    });

    widgets::section_header(
        ui,
        "BSFree Archive",
        Some("Search an optional historical catalogue by title and platform."),
    );
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if is_dolphin_browse {
                widgets::status_badge(
                    ui,
                    "GameCube/Wii: installable via Dolphin",
                    widgets::StatusTone::Info,
                );
                ui.label(
                    "GameCube cheats can be installed with Dolphin; Wii cheats can be installed with Dolphin. Other BSFree formats remain browse only.",
                );
            } else {
                widgets::status_badge(ui, "Browse only", widgets::StatusTone::Pending);
                ui.label(
                    "Cheats here are for reference: they can be viewed and copied, but EmuWiz \
                     does not install them for this platform.",
                );
            }
        });
        ui.label("Match based on platform and title. Exact game revision is not verified.");

        let usable = matches!(manager, BsFreeManagerState::Ready(status) if status.usable);
        if usable && state.platforms.is_none() && !busy {
            action = Some(BsFreeOperation::LoadSystems);
        }
        match manager {
            BsFreeManagerState::Ready(status) if !status.usable => widgets::banner(
                ui,
                "BSFree source is not ready",
                "Download or import and validate the optional database from Cheats → Sources.",
                widgets::StatusTone::Pending,
            ),
            BsFreeManagerState::Failed(message) => widgets::banner(
                ui,
                "BSFree status unavailable",
                message,
                widgets::StatusTone::Warning,
            ),
            BsFreeManagerState::NotLoaded => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking BSFree source...");
                });
            }
            BsFreeManagerState::Ready(_) => {}
        }

        if let Some(platforms) = state.platforms.as_ref() {
            match platforms {
                Err(message) => widgets::banner(
                    ui,
                    "BSFree platform catalogue unavailable",
                    message,
                    widgets::StatusTone::Warning,
                ),
                Ok(platforms) => {
                    let platforms = platforms.clone();
                    let previous_platform = state.search_platform.clone();
                    let previous_system_id = state.search_system_id;
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Platform");
                        egui::ComboBox::from_id_salt("bsfree-platform-selector")
                            .selected_text(
                                if state.search_platform.is_empty()
                                    && state.search_system_id.is_none()
                                {
                                    "All platforms".to_string()
                                } else if let Some(system_id) = state.search_system_id {
                                    platforms
                                        .iter()
                                        .find(|system| system.upstream_id == system_id)
                                        .map(bsfree_platform_display)
                                        .unwrap_or("Selected BSFree system")
                                        .to_string()
                                } else {
                                    state.search_platform.clone()
                                },
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut state.search_platform,
                                    String::new(),
                                    "All platforms",
                                );
                                state.search_system_id = None;
                                let query = state.platform_query.trim().to_ascii_lowercase();
                                for system in platforms.iter().filter(|system| {
                                    query.is_empty()
                                        || bsfree_platform_display(system)
                                            .to_ascii_lowercase()
                                            .contains(&query)
                                        || system.name.to_ascii_lowercase().contains(&query)
                                }) {
                                    let display = bsfree_platform_display(system).to_string();
                                    let capability = bsfree_platform_capability(
                                        system.mapping.archivefs_platform_id.as_deref(),
                                    )
                                    .0;
                                    let label = format!("{display} · {capability}");
                                    if ui
                                        .selectable_label(
                                            state.search_system_id == Some(system.upstream_id)
                                                || (state.search_system_id.is_none()
                                                    && state.search_platform
                                                        == system
                                                            .mapping
                                                            .archivefs_platform_id
                                                            .clone()
                                                            .unwrap_or_default()),
                                            label,
                                        )
                                        .clicked()
                                    {
                                        state.search_platform = system
                                            .mapping
                                            .archivefs_platform_id
                                            .clone()
                                            .unwrap_or_default();
                                        state.search_system_id = system
                                            .mapping
                                            .archivefs_platform_id
                                            .is_none()
                                            .then_some(system.upstream_id);
                                        ui.close_menu();
                                    }
                                }
                            });
                        ui.label("Filter");
                        ui.add(
                            egui::TextEdit::singleline(&mut state.platform_query)
                                .desired_width(150.0)
                                .hint_text("Find a BSFree platform"),
                        );
                        ui.label(format!(
                            "{} platforms · {} cheats",
                            platforms.len(),
                            platforms
                                .iter()
                                .map(|system| system.cheat_count)
                                .sum::<u64>()
                        ));
                    });
                    for system in &platforms {
                        if state.search_system_id == Some(system.upstream_id)
                            || (state.search_system_id.is_none()
                                && state.search_platform
                                    == system
                                        .mapping
                                        .archivefs_platform_id
                                        .clone()
                                        .unwrap_or_default())
                        {
                            let (label, tone) = bsfree_platform_capability(
                                system.mapping.archivefs_platform_id.as_deref(),
                            );
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!(
                                    "{}: {} cheats",
                                    bsfree_platform_display(system),
                                    system.cheat_count
                                ));
                                widgets::status_badge(ui, label, tone);
                            });
                            break;
                        }
                    }
                    if previous_platform != state.search_platform
                        || previous_system_id != state.search_system_id
                    {
                        state.search_result = None;
                        state.selected_game = None;
                        state.cheats = None;
                    }
                }
            }
        }

        ui.horizontal_wrapped(|ui| {
            ui.label("Title");
            ui.add(egui::TextEdit::singleline(&mut state.search_title).desired_width(300.0));
            if widgets::action_button(
                ui,
                "Search BSFree Archive",
                widgets::ActionStyle::Secondary,
                usable && !busy && !state.search_title.trim().is_empty(),
            )
            .clicked()
            {
                action = Some(BsFreeOperation::Search(BsFreeGameSearchRequest {
                    platform_id: (!state.search_platform.trim().is_empty())
                        .then(|| state.search_platform.trim().to_string()),
                    system_id: state.search_system_id,
                    title: state.search_title.trim().to_string(),
                    version: None,
                    device_id: None,
                    upstream_game_id: None,
                    page: PageRequest::games(0),
                }));
            }
        });
        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the local BSFree database...");
            });
        }

        if let Some(result) = &state.search_result {
            ui.separator();
            match result {
                Err(message) => widgets::banner(
                    ui,
                    "BSFree search failed",
                    message,
                    widgets::StatusTone::Warning,
                ),
                Ok(result) => {
                    ui.horizontal_wrapped(|ui| {
                        widgets::status_badge(
                            ui,
                            bsfree_match_label(result.confidence),
                            if result.confidence == ProviderGameMatchConfidence::NoMatch {
                                widgets::StatusTone::Pending
                            } else {
                                widgets::StatusTone::Info
                            },
                        );
                        ui.label(format!(
                            "{} candidate(s); revision not verified",
                            result.page.total
                        ));
                    });
                    ui.label(&result.explanation);
                    for game in &result.page.rows {
                        ui.group(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(&game.name);
                                widgets::status_badge(
                                    ui,
                                    game.system
                                        .archivefs_platform_display_name
                                        .as_deref()
                                        .unwrap_or(&game.system.name),
                                    widgets::StatusTone::Info,
                                );
                                widgets::status_badge(
                                    ui,
                                    &game.device.name,
                                    widgets::StatusTone::Pending,
                                );
                            });
                            ui.label(format!(
                                "Version: {} · {} cheats · BSFree game UID {} / historical game ID {}",
                                game.version.as_deref().unwrap_or("not supplied"),
                                game.cheat_count,
                                game.upstream_uid,
                                game.upstream_game_id
                            ));
                            if ui
                                .add_enabled(!busy, egui::Button::new("Browse cheats"))
                                .clicked()
                            {
                                action = Some(BsFreeOperation::LoadGame {
                                    upstream_uid: game.upstream_uid,
                                    offset: 0,
                                });
                            }
                        });
                    }
                }
            }
        }

        if let (Some(game), Some(cheats)) = (&state.selected_game, &state.cheats) {
            ui.separator();
            ui.heading(&game.name);
            ui.label(format!(
                "{} · {} · {}",
                game.system
                    .archivefs_platform_display_name
                    .as_deref()
                    .unwrap_or(&game.system.name),
                game.device.name,
                game.version.as_deref().unwrap_or("version not supplied")
            ));
            match cheats {
                Err(message) => widgets::banner(
                    ui,
                    "Could not load BSFree cheats",
                    message,
                    widgets::StatusTone::Warning,
                ),
                Ok(page) => {
                    ui.label(format!(
                        "Showing {}–{} of {} cheats",
                        page.offset.saturating_add(1),
                        page.offset.saturating_add(page.rows.len() as u32),
                        page.total
                    ));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !busy && page.offset > 0,
                                egui::Button::new("Previous 100"),
                            )
                            .clicked()
                        {
                            action = Some(BsFreeOperation::LoadGame {
                                upstream_uid: game.upstream_uid,
                                offset: page
                                    .offset
                                    .saturating_sub(PageRequest::DEFAULT_CHEAT_LIMIT as u32),
                            });
                        }
                        if ui
                            .add_enabled(!busy && page.has_more, egui::Button::new("Next 100"))
                            .clicked()
                        {
                            action = Some(BsFreeOperation::LoadGame {
                                upstream_uid: game.upstream_uid,
                                offset: page.offset.saturating_add(page.limit as u32),
                            });
                        }
                    });
                    for cheat in &page.rows {
                        ui.group(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(&cheat.name);
                                widgets::status_badge(
                                    ui,
                                    bsfree_compatibility_label(cheat.compatibility),
                                    widgets::StatusTone::Pending,
                                );
                                let (capability_label, capability_tone) = bsfree_code_capability(
                                    cheat,
                                    game.system.archivefs_platform_id.as_deref(),
                                );
                                widgets::status_badge(ui, capability_label, capability_tone);
                            });
                            if let Some(section) = &cheat.section {
                                ui.label(format!("Section: {}", section.name));
                            }
                            if let Some(author) = &cheat.author {
                                ui.label(format!("Author: {}", author.name));
                            }
                            if let Some(note) = &cheat.note {
                                ui.label(note);
                            }
                            ui.collapsing("Raw code and provenance", |ui| {
                                ui.monospace(&cheat.code);
                                ui.label(format!(
                                    "Provider: BSFree Archive · device: {} · row ID: {}",
                                    cheat.device.name, cheat.upstream_id
                                ));
                            });
                        });
                    }
                }
            }
        }
    });
    action
}

/// The Sources page's compact "Recent activity" - reuses
/// `widgets::activity_row_header` (the same shared row-header component
/// Phase 2's activity consolidation introduced for the bottom activity
/// bar, the History & Logs page, and the Cheats & Mods mini card), scoped
/// to the `ActivityAction` variants a Sources-page user would recognise as
/// theirs: adding/enabling/disabling/removing/scanning a source folder,
/// and cheat-database retrieval (shared with Cheats & Mods, since a
/// database update started from either page is the same event). No new
/// activity-rendering logic - only a new filter over the same
/// `OperationHistory` every other activity surface already reads.
///
/// There is no "view full history" link here, matching
/// `show_recent_cheat_activity`'s precedent on Cheats & Mods (which
/// doesn't have one either) - not invented for this page specifically.
/// Full History & Logs remains reachable from the sidebar as always.
pub(super) fn show_sources_recent_activity(ui: &mut egui::Ui, history: &OperationHistory) {
    let entries: Vec<&HistoryEntry> = history
        .entries()
        .filter(|entry| {
            matches!(
                entry.action,
                ActivityAction::SourceAdded
                    | ActivityAction::SourceEnabled
                    | ActivityAction::SourceDisabled
                    | ActivityAction::SourceScan
                    | ActivityAction::SourceRemoved
                    | ActivityAction::CheatSourceRetrieval
            )
        })
        .take(5)
        .collect();
    widgets::section_header(
        ui,
        "Recent activity",
        Some("A compact view of this session's source and cheat-database changes."),
    );
    if entries.is_empty() {
        ui.weak("No source or cheat-database activity has been recorded in this session.");
        return;
    }
    for entry in entries {
        widgets::card(ui, |ui| {
            widgets::activity_row_header(
                ui,
                entry.outcome.to_string(),
                activity_outcome_tone(entry.outcome),
                entry.action.to_string(),
                Some(&format_history_timestamp(entry.timestamp)),
                |_ui| {},
            );
            ui.add(egui::Label::new(&entry.message).truncate())
                .on_hover_text(&entry.message);
        });
        ui.add_space(4.0);
    }
}

/// The consolidated Sources destination's shared heading and tab selector,
/// rendered identically regardless of which tab is active - mirrors
/// `show_library_shell_header`'s role for the unified Library shell and
/// `problems_repair_page::show_problems_repair_tabs`'s role for Problems &
/// Repair. Returns the newly clicked tab, if any; the caller
/// (`ArchiveFsApp::show_sources_page`) applies it via
/// `navigate_to_sources_tab`.
pub(super) fn show_sources_tabs(ui: &mut egui::Ui, current: SourcesTab) -> Option<SourcesTab> {
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::SOURCES,
        "Sources",
        "Manage where EmuWiz finds games, DAT catalogues, and cheats.",
    );
    let tab_options: [(SourcesTab, &str); 4] = [
        (
            SourcesTab::Libraries,
            sources_tab_label(SourcesTab::Libraries),
        ),
        (SourcesTab::Dats, sources_tab_label(SourcesTab::Dats)),
        (SourcesTab::Cheats, sources_tab_label(SourcesTab::Cheats)),
        (
            SourcesTab::Discovery,
            sources_tab_label(SourcesTab::Discovery),
        ),
    ];
    let clicked = widgets::tab_row(ui, &tab_options, current);
    ui.add_space(8.0);
    clicked
}

/// The "Discovery" tab: Collection Discovery's summary of the most recent
/// scan's universal ingestion results. Was previously
/// `ToolsOverlay::CollectionDiscovery`; the underlying renderer
/// (`collection_discovery_page::show_collection_discovery_panel`) and the
/// data it reads are unchanged - only the routing mechanism moved from an
/// overlay to a Sources tab.
pub(super) fn show_sources_discovery_tab(ui: &mut egui::Ui, database_state: &DatabaseState) {
    sources_content_column(ui, |ui| {
        widgets::section_header(
            ui,
            "Collection Discovery",
            Some(
                "A plain-language summary of the most recent scan's universal ingestion results: what was found, what needs attention, and what to do about it.",
            ),
        );
        let summary = match database_state {
            DatabaseState::Ready {
                last_scan_summary, ..
            } => last_scan_summary.as_ref(),
            _ => None,
        };
        // Prefer the persisted database's own record of the most recent
        // *completed* scan over the session-local `last_scan_summary` - this
        // run id survives an app restart, so paging keeps working even when
        // the summary above is `None` (nothing scanned yet this session).
        let discovery_run = match database_state {
            DatabaseState::Ready { snapshot, .. } => snapshot
                .last_completed_scan
                .as_ref()
                .map(|scan| (snapshot.database_path.as_path(), scan.scan_run_id)),
            _ => None,
        };
        collection_discovery_page::show_collection_discovery_panel(ui, summary, discovery_run);
    });
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn sources_content_column_is_readable_on_desktop_without_clipping_narrow_views() {
        assert_eq!(sources_content_width(600.0), 600.0);
        assert_eq!(sources_content_width(1_024.0), 1_024.0);
        assert_eq!(sources_content_width(1_100.0), theme::CONTENT_MAX_WIDTH);
        assert_eq!(sources_content_width(1_440.0), theme::CONTENT_MAX_WIDTH);
        assert_eq!(sources_content_width(1_920.0), theme::CONTENT_MAX_WIDTH);
    }
}
