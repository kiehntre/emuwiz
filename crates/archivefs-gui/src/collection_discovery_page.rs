// Collection Health / Discovery Visibility panel, extracted mechanically
// from main.rs in the same style as sources_page.rs/administration_pages.rs.
//
// Purpose: make the universal ingestion engine's findings (`ingestion`
// crate module) understandable to a person who has never heard of a
// "ContentKind" or a "ContainerKind" - what did EmuWiz find, how much, what
// needs attention, and what can be done about it. Every count and row here
// is read from the most recent scan's `ScanPersistSummary` - this panel
// computes nothing of its own and never mutates anything.

use super::*;
use archivefs_core::ScanPersistSummary;
use archivefs_core::ingestion::{ContentKind, GameDiscovery, SkipReason, SkipReasonCounts};
use archivefs_core::{DiscoveryDetailFilter, DiscoveryDetailRecord, DiscoveryDetailsPage};

/// Renders the read-only "Skipped files" drill-down: a bounded sample of
/// the files the most recently completed scan skipped, with a concise
/// reason and a simple reason filter. Purely informational - this never
/// re-scans, reclassifies, renames, or otherwise touches anything on disk;
/// it only reads [`ScanPersistSummary::skipped_files`], the exact detail
/// `scan_and_persist` already produced from the same scan whose aggregate
/// counts are shown elsewhere in the Database Status panel.
///
/// `summary` is `None` when no scan has completed yet this session (or the
/// database state changed underneath the window); the window still opens
/// and says so, rather than silently doing nothing.
///
/// Not virtualised: `skipped_files` is hard-bounded at
/// [`archivefs_core::MAX_RETAINED_SKIPPED_FILES`] (1000) entries, small
/// enough that a plain scrolling list of simple labels stays practical
/// without the fixed-row-height virtualisation `repair_review_page` needs
/// for its potentially much larger proposal lists.
///
/// Extracted verbatim from `main.rs` (2026-08-22, GUI extraction pass 2):
/// thematically part of this page's "what didn't make it in, and why"
/// concern, not a standalone overlay.
pub(super) fn show_skipped_files_window(
    context: &egui::Context,
    open: &mut bool,
    summary: Option<&ScanPersistSummary>,
    filter: &mut Option<archivefs_core::SkipReason>,
) {
    egui::Window::new("Skipped files")
        .open(open)
        .resizable(true)
        .default_width(560.0)
        .show(context, |ui| {
            let Some(summary) = summary else {
                ui.label("No scan has completed yet this session.");
                return;
            };
            let total = summary.skipped_files_total();
            // Gate on both counters, not `total` alone: a scan can have
            // zero legacy-bucket skips (`skipped_files_total`) while still
            // having ingestion-recognised-but-unmatched items to point at
            // below - the empty state must not claim nothing was skipped
            // when there is exactly that to redirect to.
            if total == 0 && summary.ingestion_skipped.is_empty() {
                ui.label("Nothing was skipped in the most recent scan.");
                return;
            }
            if total > 0 {
                let retained = summary.skipped_files.len();
                if summary.skipped_files_truncated() {
                    ui.label(format!(
                        "Showing {retained} of {total} skipped files - the rest were skipped \
                         for the same reasons but are not individually listed."
                    ));
                } else {
                    ui.label(format!("{retained} skipped file(s)."));
                }
            }
            ui.label(
                egui::RichText::new(
                    "Explanatory only - nothing here changes how a file was classified or \
                     mutates any file.",
                )
                .color(theme::muted(ui)),
            );

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Filter:");
                if ui.selectable_label(filter.is_none(), "All").clicked() {
                    *filter = None;
                }
                for reason in [
                    archivefs_core::SkipReason::UnsupportedExtension,
                    archivefs_core::SkipReason::AmbiguousPlatform,
                ] {
                    if ui
                        .selectable_label(*filter == Some(reason), reason.label())
                        .clicked()
                    {
                        *filter = Some(reason);
                    }
                }
            });

            ui.add_space(6.0);
            // Taller than before, and with a lighter per-row treatment (a
            // small reason badge - omitted when the active filter already
            // says the same thing for every visible row - plus the
            // filename shown prominently, full path on hover rather than
            // spelled out on every line) so a list of dozens or hundreds
            // of entries stays scannable instead of reading like raw
            // per-file diagnostics.
            egui::ScrollArea::vertical()
                .max_height(480.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let selected_filter = *filter;
                    for item in summary
                        .skipped_files
                        .iter()
                        .filter(|item| match selected_filter {
                            Some(reason) => item.reason == reason,
                            None => true,
                        })
                    {
                        ui.horizontal(|ui| {
                            if selected_filter.is_none() {
                                widgets::status_badge(
                                    ui,
                                    item.reason.label(),
                                    widgets::StatusTone::Info,
                                );
                            }
                            let filename = item
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| item.path.display().to_string());
                            ui.label(egui::RichText::new(filename).strong())
                                .on_hover_text(item.path.display().to_string());
                        });
                    }
                    if !summary.ingestion_skipped.is_empty() {
                        ui.add_space(10.0);
                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "This scan also found items EmuWiz recognised but couldn't \
                                 confidently match - see Tools -> Collection Discovery for the \
                                 full breakdown with suggested actions.",
                            )
                            .color(theme::muted(ui)),
                        );
                    }
                });
        });
}

/// Which "Needs attention" bucket the item-details list is narrowed to.
/// Mirrors [`SkipReason`]'s variants but without `InvalidContent`'s String
/// payload, so it can be used as plain, comparable egui widget-memory
/// state (2026-08-22, live-QA Phase 8: added so a Needs Attention row can
/// be clicked to filter the detail list down to just that reason, and so
/// the huge "unknown items" bucket has a dedicated `[View unknown items]`
/// action instead of only ever appearing mixed into a small combined
/// sample).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NeedsAttentionFilter {
    UnsupportedExtension,
    MissingPairedFile,
    AmbiguousPlatform,
    NoIdentityMatch,
    InvalidContent,
}

impl NeedsAttentionFilter {
    fn matches(self, reason: &SkipReason) -> bool {
        matches!(
            (self, reason),
            (Self::UnsupportedExtension, SkipReason::UnsupportedExtension)
                | (Self::MissingPairedFile, SkipReason::MissingPairedFile)
                | (Self::AmbiguousPlatform, SkipReason::AmbiguousPlatform)
                | (
                    Self::NoIdentityMatch,
                    SkipReason::RecognizedContentNoIdentityMatch
                )
                | (Self::InvalidContent, SkipReason::InvalidContent(_))
        )
    }
}

fn filter_memory_id() -> egui::Id {
    egui::Id::new("collection_discovery_needs_attention_filter")
}

fn active_filter(ui: &egui::Ui) -> Option<NeedsAttentionFilter> {
    ui.memory(|memory| memory.data.get_temp(filter_memory_id()))
        .flatten()
}

fn set_active_filter(ui: &egui::Ui, filter: Option<NeedsAttentionFilter>) {
    ui.memory_mut(|memory| memory.data.insert_temp(filter_memory_id(), filter));
}

/// Groups digits into thousands with `,` separators (`167499` ->
/// `"167,499"`). EmuWiz has no locale-formatting dependency anywhere else,
/// so this stays a plain, dependency-free implementation rather than
/// pulling one in for a single call site.
fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

/// `discovery_run` is `Some((database_path, run_id))` whenever a
/// persisted, pageable detail set exists for the run `summary` describes -
/// see [`crate::CachedLibrarySnapshot::database_path`] and
/// `CompletedScanSummary::scan_run_id`/`ScanPersistSummary::scan_run_id`.
/// `None` only when no completed scan has ever been persisted (a summary
/// can still be `Some` from an in-session scan while the database read
/// path is otherwise unavailable) - the panel falls back to the old bounded
/// sample in that case rather than showing nothing.
pub(super) fn show_collection_discovery_panel(
    ui: &mut egui::Ui,
    summary: Option<&ScanPersistSummary>,
    discovery_run: Option<(&Path, i64)>,
) {
    match summary {
        Some(summary) => {
            show_found_summary(ui, summary);
            ui.add_space(theme::SECTION_GAP);
            show_needs_attention_summary(ui, &summary.ingestion_skip_reasons);
            ui.add_space(theme::SECTION_GAP);
            show_platform_breakdown(ui, summary);
            ui.add_space(theme::SECTION_GAP);
        }
        None if discovery_run.is_some() => {
            widgets::card(ui, |ui| {
                ui.label("Showing details from the most recent completed scan.");
                ui.label(
                    egui::RichText::new(
                        "Run a new scan to refresh the session summary and collection totals.",
                    )
                    .color(theme::muted(ui)),
                );
            });
            ui.add_space(theme::SECTION_GAP);
        }
        None => {
            widgets::card(ui, |ui| {
                ui.label("No scan has completed yet.");
                ui.label(
                    egui::RichText::new(
                        "Run a scan from the Sources page, then come back here to see what \
                         EmuWiz found in your collection.",
                    )
                    .color(theme::muted(ui)),
                );
            });
            return;
        }
    }

    match discovery_run {
        Some((database_path, run_id)) => show_item_details_paged(ui, database_path, run_id),
        None => show_item_details_bounded_sample(
            ui,
            summary.expect("bounded samples require a session scan summary"),
        ),
    }
}

fn show_found_summary(ui: &mut egui::Ui, summary: &ScanPersistSummary) {
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Collection scan complete",
            Some("What EmuWiz found, grouped by what it actually is - not how it's stored."),
        );
        let stats = &summary.ingestion_stats;
        let rows: Vec<(String, String)> = [
            ("ROMs", stats.loose_roms),
            ("Disc images", stats.disc_images),
            ("Amiga", stats.amiga_images),
            ("Computer disks", stats.computer_disks),
            ("Game folders", stats.game_folders),
            ("Archives", stats.archives),
        ]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(label, count)| (label.to_string(), count.to_string()))
        .collect();
        if rows.is_empty() {
            ui.label("Nothing recognisable was found in the scanned source(s).");
            return;
        }
        for (label, count) in &rows {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(count).strong().size(16.0));
                ui.label(label);
            });
        }
        ui.label(
            egui::RichText::new(
                "\"Archives\" are ZIP/RAR/7-Zip/TAR files - EmuWiz doesn't yet look inside \
                 every format to say what's inside each one.",
            )
            .small()
            .color(theme::muted(ui)),
        )
        .on_hover_text(
            "Internally: an \"archive\" is a container, distinct from the game content it \
             may hold. RAR and 7-Zip contents specifically aren't inspected yet.",
        );
    });
}

/// Natural-language, singular/plural-aware sentence for one Needs
/// attention row, plus the one-line explanation shown under it (2026-08-22,
/// live-QA Phase 8: replaces the earlier `"{count} {text}"` construction,
/// e.g. `"167499 unknown item(s) EmuWiz doesn't recognise yet"`, with real
/// pluralisation and thousands separators).
fn needs_attention_wording(filter: NeedsAttentionFilter, count: usize) -> (String, &'static str) {
    let items = if count == 1 { "item" } else { "items" };
    let n = format_count(count);
    match filter {
        NeedsAttentionFilter::UnsupportedExtension => {
            let wasnt_werent = if count == 1 { "wasn't" } else { "weren't" };
            (
                format!("{n} {items} {wasnt_werent} recognised"),
                "These have a file type EmuWiz doesn't know how to classify yet - things like \
                 box art, manuals, and other non-game files are already excluded from this \
                 count, so what's left here is genuinely unfamiliar file types.",
            )
        }
        NeedsAttentionFilter::MissingPairedFile => {
            let is_are = if count == 1 { "is" } else { "are" };
            let its_their = if count == 1 { "its" } else { "their" };
            (
                format!("{n} disc {items} {is_are} missing {its_their} matching .cue/.bin file"),
                "A disc image needs both files present together to be read.",
            )
        }
        NeedsAttentionFilter::AmbiguousPlatform => {
            let has_have = if count == 1 { "has" } else { "have" };
            (
                format!("{n} {items} {has_have} a format shared by more than one platform"),
                "Nothing nearby confirmed which platform, so these were left unassigned rather \
                 than guessed.",
            )
        }
        NeedsAttentionFilter::NoIdentityMatch => {
            let was_were = if count == 1 { "was" } else { "were" };
            (
                format!("{n} {items} {was_were} recognised but not matched to a known game"),
                "The content looks like a real game file, but nothing matched it to a known \
                 title. These can still be added manually.",
            )
        }
        NeedsAttentionFilter::InvalidContent => (
            format!("{n} {items} looked right but couldn't be read"),
            "These have a recognised extension but could not be read as that format - they may \
             be corrupt, partial, or a false match.",
        ),
    }
}

fn show_needs_attention_summary(ui: &mut egui::Ui, skip: &SkipReasonCounts) {
    if skip.total() == 0 {
        return;
    }
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Needs attention",
            Some("Click a row to see the actual items behind it."),
        );
        let rows: Vec<(NeedsAttentionFilter, usize)> = [
            (
                NeedsAttentionFilter::UnsupportedExtension,
                skip.unsupported_extension,
            ),
            (
                NeedsAttentionFilter::MissingPairedFile,
                skip.missing_paired_file,
            ),
            (
                NeedsAttentionFilter::AmbiguousPlatform,
                skip.ambiguous_platform,
            ),
            (
                NeedsAttentionFilter::NoIdentityMatch,
                skip.no_identity_match,
            ),
            (NeedsAttentionFilter::InvalidContent, skip.invalid_content),
        ]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .collect();
        let current = active_filter(ui);
        for (filter, count) in &rows {
            let (headline, explanation) = needs_attention_wording(*filter, *count);
            let is_active = current == Some(*filter);
            let action_label = if *filter == NeedsAttentionFilter::UnsupportedExtension {
                if is_active {
                    "Hide unknown items"
                } else {
                    "View unknown items"
                }
            } else if is_active {
                "Hide details"
            } else {
                "View details"
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&headline).strong());
                if widgets::action_button(
                    ui,
                    action_label,
                    if is_active {
                        widgets::ActionStyle::Primary
                    } else {
                        widgets::ActionStyle::Secondary
                    },
                    true,
                )
                .clicked()
                {
                    set_active_filter(ui, if is_active { None } else { Some(*filter) });
                }
            });
            ui.label(
                egui::RichText::new(explanation)
                    .small()
                    .color(theme::muted(ui)),
            );
            ui.add_space(6.0);
        }
    });
}

fn show_platform_breakdown(ui: &mut egui::Ui, summary: &ScanPersistSummary) {
    if summary.ingestion_platform_counts.is_empty() {
        return;
    }
    widgets::card(ui, |ui| {
        widgets::section_header(ui, "Recognised by platform", None);
        for (platform, count) in &summary.ingestion_platform_counts {
            ui.horizontal(|ui| {
                ui.label(format!("{count}"));
                ui.weak("·");
                ui.label(platform);
            });
        }
    });
}

/// A plain-language description of one item: platform + content type when
/// both are known ("Game Boy Advance ROM"), content type alone when the
/// platform isn't ("Disc image"), or a generic fallback when neither
/// resolved. Never surfaces `ContentKind`/`ContainerKind` by name.
fn human_item_kind(item: &GameDiscovery) -> String {
    match (&item.platform_hint, item.content) {
        (Some(platform), Some(content)) => format!("{platform} {}", content_label(content)),
        (None, Some(content)) => content_label(content).to_string(),
        (_, None) => "Unrecognised file".to_string(),
    }
}

fn content_label(content: ContentKind) -> &'static str {
    match content {
        ContentKind::RomCartridge => "ROM",
        ContentKind::DiscImage => "Disc image",
        ContentKind::AmigaImage => "Amiga image",
        ContentKind::ComputerDisk => "Computer disk image",
        ContentKind::TapeImage => "Cassette/tape image",
        ContentKind::WhdloadInstall => "WHDLoad install",
        ContentKind::ExtractedGameFolder => "Game folder",
    }
}

/// Fallback rendering for when no persisted, pageable run is available -
/// the original bounded-sample behaviour, kept verbatim so the panel still
/// shows *something* rather than nothing in that case.
fn show_item_details_bounded_sample(ui: &mut egui::Ui, summary: &ScanPersistSummary) {
    if summary.ingestion_recognised_sample.is_empty() && summary.ingestion_skipped.is_empty() {
        return;
    }
    let filter = active_filter(ui);
    widgets::card(ui, |ui| match filter {
        Some(active) => {
            let total = needs_attention_count(active, &summary.ingestion_skip_reasons);
            let matching: Vec<&GameDiscovery> = summary
                .ingestion_skipped
                .iter()
                .filter(|item| {
                    item.skip_reason
                        .as_ref()
                        .is_some_and(|reason| active.matches(reason))
                })
                .collect();
            widgets::section_header(
                ui,
                "Item details",
                Some(&format!(
                    "{} total; showing {} representative example{} below.",
                    format_count(total),
                    matching.len(),
                    if matching.len() == 1 { "" } else { "s" }
                )),
            );
            if widgets::action_button(
                ui,
                "Show all item details instead",
                widgets::ActionStyle::Quiet,
                true,
            )
            .clicked()
            {
                set_active_filter(ui, None);
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for item in matching {
                        let reason = item.skip_reason.as_ref().expect("filtered above");
                        show_detail_row(
                            ui,
                            item,
                            widgets::StatusTone::Warning,
                            reason.label(),
                            Some(reason.suggested_action()),
                        );
                    }
                });
        }
        None => {
            widgets::section_header(
                ui,
                "Item details",
                Some(
                    "A sample of what was found - not the full list for very large \
                         collections.",
                ),
            );
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for item in &summary.ingestion_recognised_sample {
                        show_detail_row(ui, item, widgets::StatusTone::Success, "Recognised", None);
                    }
                    for item in &summary.ingestion_skipped {
                        let Some(reason) = &item.skip_reason else {
                            continue;
                        };
                        show_detail_row(
                            ui,
                            item,
                            widgets::StatusTone::Warning,
                            reason.label(),
                            Some(reason.suggested_action()),
                        );
                    }
                });
        }
    });
}

/// How many rows one Collection Discovery page requests at a time -
/// mid-range of the project's own "100-500" guidance, matching
/// `MAX_RETAINED_SKIPPED_FILES`'s existing order of magnitude without
/// approaching it (this bound is per-*page*, not per-run).
const DISCOVERY_PAGE_SIZE: i64 = 200;

/// The paging/filter state one open Collection Discovery panel remembers
/// between frames - plain egui widget memory, exactly like
/// [`active_filter`]/[`set_active_filter`] above. `page` is the
/// already-fetched result for `(run_id, offset, filter)`; it is `None`
/// only before the very first fetch this session, or after an error.
#[derive(Clone)]
struct DiscoveryPageState {
    run_id: i64,
    offset: i64,
    filter: DiscoveryDetailFilter,
    page: Option<DiscoveryDetailsPage>,
    error: Option<String>,
}

fn discovery_page_memory_id() -> egui::Id {
    egui::Id::new("collection_discovery_paged_state")
}

fn discovery_page_state(ui: &egui::Ui, run_id: i64) -> DiscoveryPageState {
    ui.memory(|memory| memory.data.get_temp(discovery_page_memory_id()))
        .filter(|state: &DiscoveryPageState| state.run_id == run_id)
        .unwrap_or(DiscoveryPageState {
            run_id,
            offset: 0,
            filter: DiscoveryDetailFilter::All,
            page: None,
            error: None,
        })
}

fn set_discovery_page_state(ui: &egui::Ui, state: DiscoveryPageState) {
    ui.memory_mut(|memory| memory.data.insert_temp(discovery_page_memory_id(), state));
}

/// The one bucket a "Needs attention" row narrows the paged detail list to -
/// mirrors [`NeedsAttentionFilter`] plus the always-available "Recognised"
/// bucket, translated 1:1 into the persisted-query vocabulary
/// [`DiscoveryDetailFilter`] (see `archivefs_core::database`'s doc comment
/// on why this stays the same small, naturally-represented bucket set
/// rather than growing into free-text search).
fn discovery_detail_filter_label(filter: DiscoveryDetailFilter) -> &'static str {
    match filter {
        DiscoveryDetailFilter::All => "All",
        DiscoveryDetailFilter::Recognised => "Recognised",
        DiscoveryDetailFilter::Unverified => "Unverified",
        DiscoveryDetailFilter::Unsupported => "Unsupported",
        DiscoveryDetailFilter::MissingPairedFile => "Missing pair",
        DiscoveryDetailFilter::AmbiguousPlatform => "Ambiguous platform",
        DiscoveryDetailFilter::InvalidContent => "Could not be read",
        DiscoveryDetailFilter::Archive => "Archive",
        DiscoveryDetailFilter::Disk => "Disk",
        DiscoveryDetailFilter::Tape => "Tape",
    }
}

fn status_tone_for_row(row: &DiscoveryDetailRecord) -> (widgets::StatusTone, String) {
    match &row.skip_reason {
        None => (widgets::StatusTone::Success, "Recognised".to_string()),
        Some(reason) => (widgets::StatusTone::Warning, reason.label().to_string()),
    }
}

fn discovery_detail_filename(row: &DiscoveryDetailRecord) -> String {
    row.path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| row.path.display().to_string())
}

fn discovery_detail_kind_label(row: &DiscoveryDetailRecord) -> String {
    match (&row.platform_hint, row.content) {
        (Some(platform), Some(content)) => format!("{platform} {}", content_label(content)),
        (None, Some(content)) => content_label(content).to_string(),
        (_, None) => "Unrecognised file".to_string(),
    }
}

/// The full paged Item details section: fetches (only when the requested
/// `(run_id, offset, filter)` changed since the last frame) one bounded
/// page of persisted [`DiscoveryDetailRecord`] rows directly from the
/// database, and renders exactly that page - never the whole result set,
/// however large. Loading another page re-queries the already-persisted
/// table; it never re-scans the filesystem.
fn show_item_details_paged(ui: &mut egui::Ui, database_path: &Path, run_id: i64) {
    let mut state = discovery_page_state(ui, run_id);

    let needs_fetch = match &state.page {
        Some(page) => {
            page.run_id != run_id || page.offset != state.offset || page.filter != state.filter
        }
        None => true,
    };
    if needs_fetch {
        match fetch_discovery_page(database_path, run_id, state.offset, state.filter) {
            Ok(page) => {
                state.page = Some(page);
                state.error = None;
            }
            Err(message) => {
                state.page = None;
                state.error = Some(message);
            }
        }
    }

    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Item details",
            Some("Every recognised and skipped item found, paged from the database."),
        );

        let Some(page) = &state.page else {
            ui.label(
                egui::RichText::new(
                    state
                        .error
                        .as_deref()
                        .unwrap_or("Item details are not available for this scan."),
                )
                .color(theme::muted(ui)),
            );
            set_discovery_page_state(ui, state);
            return;
        };

        if page.run_status != archivefs_core::DiscoveryRunStatus::Completed {
            ui.label(
                egui::RichText::new(
                    "This scan did not finish (interrupted or failed) - these rows may be \
                     incomplete.",
                )
                .color(theme::muted(ui)),
            );
        }

        ui.horizontal_wrapped(|ui| {
            ui.label("Filter:");
            for filter in [
                DiscoveryDetailFilter::All,
                DiscoveryDetailFilter::Recognised,
                DiscoveryDetailFilter::Unverified,
                DiscoveryDetailFilter::Unsupported,
                DiscoveryDetailFilter::MissingPairedFile,
                DiscoveryDetailFilter::AmbiguousPlatform,
                DiscoveryDetailFilter::InvalidContent,
                DiscoveryDetailFilter::Archive,
                DiscoveryDetailFilter::Disk,
                DiscoveryDetailFilter::Tape,
            ] {
                if ui
                    .selectable_label(
                        state.filter == filter,
                        discovery_detail_filter_label(filter),
                    )
                    .clicked()
                    && state.filter != filter
                {
                    state.filter = filter;
                    state.offset = 0;
                }
            }
        });

        let total = page.total_matching;
        let showing_from = if total == 0 { 0 } else { page.offset + 1 };
        let showing_to = page.offset + page.rows.len() as i64;
        ui.label(format!(
            "{} item{} - showing {}-{}",
            format_count(total as usize),
            if total == 1 { "" } else { "s" },
            format_count(showing_from as usize),
            format_count(showing_to as usize),
        ));

        ui.add_space(6.0);
        // Bounded: exactly `page.rows.len()` widgets are ever built for one
        // frame, however large `total` is - never one widget per item in
        // the full result set.
        egui::ScrollArea::vertical()
            .max_height(420.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in &page.rows {
                    let (tone, status) = status_tone_for_row(row);
                    ui.horizontal(|ui| {
                        widgets::status_badge(ui, &status, tone);
                        ui.label(egui::RichText::new(discovery_detail_filename(row)).strong())
                            .on_hover_text(row.path.display().to_string());
                    });
                    ui.label(
                        egui::RichText::new(discovery_detail_kind_label(row))
                            .color(theme::muted(ui)),
                    );
                    ui.add_space(6.0);
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let has_previous = page.offset > 0;
            if widgets::action_button(
                ui,
                "Previous",
                widgets::ActionStyle::Secondary,
                has_previous,
            )
            .clicked()
                && has_previous
            {
                state.offset = (page.offset - page.limit).max(0);
            }
            let has_next = page.offset + (page.rows.len() as i64) < page.total_matching;
            if widgets::action_button(ui, "Next", widgets::ActionStyle::Secondary, has_next)
                .clicked()
                && has_next
            {
                state.offset = page.offset + page.limit;
            }
        });

        set_discovery_page_state(ui, state);
    });
}

/// Opens a short-lived read-only connection and fetches exactly one page -
/// this never holds a `Database` handle across frames, matching how other
/// one-off GUI actions in this codebase already open the database
/// synchronously for a single call (see e.g. the platform-assignment
/// actions in `main.rs`). A bounded `LIMIT`/`OFFSET` query against the
/// `(scan_run_id, id)` index stays fast even for a very large table, so
/// this is safe to run on click rather than needing the heavier
/// background-thread `Gathered` pattern the full library snapshot uses.
fn fetch_discovery_page(
    database_path: &Path,
    run_id: i64,
    offset: i64,
    filter: DiscoveryDetailFilter,
) -> Result<DiscoveryDetailsPage, String> {
    let database = Database::open_read_only(database_path).map_err(|error| error.to_string())?;
    database
        .query_discovery_details(run_id, offset, DISCOVERY_PAGE_SIZE, filter)
        .map_err(|error| error.to_string())
}

fn needs_attention_count(filter: NeedsAttentionFilter, skip: &SkipReasonCounts) -> usize {
    match filter {
        NeedsAttentionFilter::UnsupportedExtension => skip.unsupported_extension,
        NeedsAttentionFilter::MissingPairedFile => skip.missing_paired_file,
        NeedsAttentionFilter::AmbiguousPlatform => skip.ambiguous_platform,
        NeedsAttentionFilter::NoIdentityMatch => skip.no_identity_match,
        NeedsAttentionFilter::InvalidContent => skip.invalid_content,
    }
}

fn show_detail_row(
    ui: &mut egui::Ui,
    item: &GameDiscovery,
    tone: widgets::StatusTone,
    status: &str,
    suggested_action: Option<&str>,
) {
    let filename = item
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| item.path.display().to_string());
    ui.horizontal(|ui| {
        widgets::status_badge(ui, status, tone);
        ui.label(egui::RichText::new(filename).strong())
            .on_hover_text(item.path.display().to_string());
    });
    ui.label(egui::RichText::new(human_item_kind(item)).color(theme::muted(ui)));
    let action = if matches!(
        item.skip_reason,
        Some(SkipReason::RecognizedContentNoIdentityMatch)
    ) {
        item.content.map(|content| {
            let extension = item
                .path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(" (.{value})"))
                .unwrap_or_default();
            format!(
                "{}{} recognised, but no configured DAT/evidence verified its platform.",
                content_label(content),
                extension
            )
        })
    } else {
        suggested_action.map(str::to_string)
    };
    if let Some(action) = action {
        ui.label(egui::RichText::new(action).small().color(theme::muted(ui)));
    }
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::ingestion::{SkipReason, ValidationState};

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

    fn item(file_name: &str, skip_reason: SkipReason) -> GameDiscovery {
        GameDiscovery {
            path: PathBuf::from(file_name),
            container: archivefs_core::ingestion::ContainerKind::DirectFile,
            content: None,
            platform_hint: None,
            identity_candidate: None,
            validation_state: ValidationState::Skipped,
            explanation: "test fixture".to_string(),
            skip_reason: Some(skip_reason),
        }
    }

    #[test]
    fn format_count_groups_digits_into_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(167_499), "167,499");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn needs_attention_wording_uses_natural_pluralised_language() {
        let (headline, _) =
            needs_attention_wording(NeedsAttentionFilter::UnsupportedExtension, 167_499);
        assert_eq!(headline, "167,499 items weren't recognised");
        assert!(
            !headline.contains("item(s)"),
            "must not use item(s)-style wording"
        );

        let (singular, _) = needs_attention_wording(NeedsAttentionFilter::UnsupportedExtension, 1);
        assert_eq!(singular, "1 item wasn't recognised");
    }

    /// Clicking a "Needs attention" row's action button ("View unknown
    /// items") narrows the Item details list to just that reason, shows
    /// the exact total (not just the bounded sample size), and offers a
    /// way back to the combined view (2026-08-22, live-QA Phase 8: the
    /// panel previously had no way to see more than the small mixed
    /// sample, and the "167,499 unknown items" figure had no detail view
    /// at all).
    #[test]
    fn selecting_a_needs_attention_filter_narrows_item_details_to_that_reason_with_an_exact_total()
    {
        let mut ingestion_stats = archivefs_core::ingestion::DiscoveryStats::default();
        ingestion_stats.loose_roms = 1;
        let mut ingestion_skip_reasons = archivefs_core::ingestion::SkipReasonCounts::default();
        // The exact total (500) is deliberately larger than the bounded
        // sample (2 items) actually carried by the summary, mirroring a
        // real large collection where only a bounded sample is retained.
        ingestion_skip_reasons.unsupported_extension = 500;
        ingestion_skip_reasons.missing_paired_file = 3;

        let summary = ScanPersistSummary {
            scan_run_id: 1,
            counts: archivefs_core::ScanRunCounts::default(),
            folder_errors: Vec::new(),
            platform_assignment_warnings: Vec::new(),
            skipped_files: Vec::new(),
            ingestion_stats,
            ingestion_skip_reasons,
            ingestion_platform_counts: std::collections::BTreeMap::new(),
            ingestion_skipped: vec![
                item("mystery1.xyz", SkipReason::UnsupportedExtension),
                item("mystery2.xyz", SkipReason::UnsupportedExtension),
                item("disc.bin", SkipReason::MissingPairedFile),
            ],
            ingestion_recognised_sample: Vec::new(),
        };

        let ctx = egui::Context::default();
        // Frame 1: the unfiltered view offers "View unknown items".
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_collection_discovery_panel(ui, Some(&summary), None);
            });
        });
        assert!(rendered_text_contains(
            &output,
            "500 items weren't recognised"
        ));
        assert!(rendered_text_contains(&output, "View unknown items"));
        assert!(rendered_text_contains(&output, "disc.bin"));

        // Frame 2: select the filter exactly as the button click would.
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                set_active_filter(ui, Some(NeedsAttentionFilter::UnsupportedExtension));
            });
        });
        let _ = output;

        // Frame 3: item details is now narrowed to just the unsupported-
        // extension items, with the exact total shown separately from the
        // (smaller) bounded sample actually listed.
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_collection_discovery_panel(ui, Some(&summary), None);
            });
        });
        assert!(rendered_text_contains(&output, "mystery1.xyz"));
        assert!(rendered_text_contains(&output, "mystery2.xyz"));
        assert!(
            !rendered_text_contains(&output, "disc.bin"),
            "the missing-paired-file item must not appear while filtered to unsupported extensions"
        );
        assert!(rendered_text_contains(&output, "500"));
        assert!(rendered_text_contains(
            &output,
            "Show all item details instead"
        ));
    }

    // --- Paged Item details (persisted `discovery_details`) ------------------------------------

    fn temp_gui_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "archivefs-gui-discovery-page-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Scans `count` synthetic unsupported-extension files into a fresh
    /// database and returns `(database_path, run_id)`, exactly the shape
    /// `show_collection_discovery_panel`'s `discovery_run` parameter wants.
    fn scanned_database_with_n_items(name: &str, count: usize) -> (PathBuf, i64) {
        let root = temp_gui_test_dir(name);
        let source = root.join("roms");
        std::fs::create_dir_all(&source).unwrap();
        for index in 0..count {
            std::fs::write(
                source.join(format!("item{index:05}.xyz")),
                format!("fixture {index}"),
            )
            .unwrap();
        }
        let database_path = root.join("library.sqlite3");
        let config = archivefs_core::Config {
            source_folders: vec![source],
            mount_root: root.join("mount"),
            ratarmount_bin: String::new(),
            master_rom_root: None,
        };
        let mut database = archivefs_core::Database::open_or_create(&database_path).unwrap();
        let summary = archivefs_core::scan_and_persist(&mut database, &config, "gui-test").unwrap();
        database.close().unwrap();
        (database_path, summary.scan_run_id)
    }

    fn minimal_summary_for(scan_run_id: i64, total_unsupported: usize) -> ScanPersistSummary {
        let mut ingestion_skip_reasons = archivefs_core::ingestion::SkipReasonCounts::default();
        ingestion_skip_reasons.unsupported_extension = total_unsupported;
        ScanPersistSummary {
            scan_run_id,
            counts: archivefs_core::ScanRunCounts::default(),
            folder_errors: Vec::new(),
            platform_assignment_warnings: Vec::new(),
            skipped_files: Vec::new(),
            ingestion_stats: archivefs_core::ingestion::DiscoveryStats::default(),
            ingestion_skip_reasons,
            ingestion_platform_counts: std::collections::BTreeMap::new(),
            ingestion_skipped: Vec::new(),
            ingestion_recognised_sample: Vec::new(),
        }
    }

    /// The exact filenames on one page, read directly from the database -
    /// used as ground truth so these tests never have to assume the
    /// filesystem walk's own order matches filename order.
    fn expect_filenames(database_path: &Path, run_id: i64, offset: i64, limit: i64) -> Vec<String> {
        let database = archivefs_core::Database::open_read_only(database_path).unwrap();
        database
            .query_discovery_details(run_id, offset, limit, DiscoveryDetailFilter::All)
            .unwrap()
            .rows
            .iter()
            .map(discovery_detail_filename)
            .collect()
    }

    #[test]
    fn paged_panel_shows_exact_total_and_current_range_not_a_representative_example() {
        let (database_path, run_id) = scanned_database_with_n_items("total-and-range", 450);
        let summary = minimal_summary_for(run_id, 450);

        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_collection_discovery_panel(
                    ui,
                    Some(&summary),
                    Some((database_path.as_path(), run_id)),
                );
            });
        });

        assert!(rendered_text_contains(&output, "450 items"));
        assert!(rendered_text_contains(&output, "showing 1-200"));
        assert!(
            !rendered_text_contains(&output, "representative example"),
            "a persisted, pageable run must never fall back to representative-example wording"
        );
    }

    #[test]
    fn persisted_details_remain_browseable_without_a_session_summary() {
        let (database_path, run_id) = scanned_database_with_n_items("after-restart", 1);
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_collection_discovery_panel(ui, None, Some((database_path.as_path(), run_id)));
            });
        });

        assert!(rendered_text_contains(
            &output,
            "Showing details from the most recent completed scan."
        ));
        assert!(rendered_text_contains(&output, "1 item - showing 1-1"));
        assert!(!rendered_text_contains(
            &output,
            "No scan has completed yet."
        ));
    }

    #[test]
    fn next_and_previous_only_change_which_page_is_shown() {
        let (database_path, run_id) = scanned_database_with_n_items("next-previous", 450);
        let summary = minimal_summary_for(run_id, 450);
        let discovery_run = Some((database_path.as_path(), run_id));
        let first_page_names = expect_filenames(&database_path, run_id, 0, DISCOVERY_PAGE_SIZE);
        let second_page_names = expect_filenames(
            &database_path,
            run_id,
            DISCOVERY_PAGE_SIZE,
            DISCOVERY_PAGE_SIZE,
        );
        let first_only = &first_page_names[0];
        let second_only = &second_page_names[0];
        assert_ne!(first_only, second_only, "pages must not overlap");

        let ctx = egui::Context::default();
        let first_frame = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_collection_discovery_panel(ui, Some(&summary), discovery_run);
            });
        });
        assert!(rendered_text_contains(&first_frame, "showing 1-200"));
        assert!(rendered_text_contains(&first_frame, first_only));
        assert!(!rendered_text_contains(&first_frame, second_only));

        // Simulate the "Next" click exactly as the button handler does.
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut state = discovery_page_state(ui, run_id);
                state.offset += DISCOVERY_PAGE_SIZE;
                set_discovery_page_state(ui, state);
            });
        });
        let _ = output;

        let second_frame = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_collection_discovery_panel(ui, Some(&summary), discovery_run);
            });
        });
        assert!(rendered_text_contains(&second_frame, "showing 201-400"));
        // The total and filter set are unaffected by paging - only the
        // visible range and rows changed.
        assert!(rendered_text_contains(&second_frame, "450 items"));
        assert!(rendered_text_contains(&second_frame, second_only));
        assert!(!rendered_text_contains(&second_frame, first_only));
    }

    #[test]
    fn a_huge_result_set_never_renders_more_than_one_pages_worth_of_rows() {
        let total = (DISCOVERY_PAGE_SIZE as usize) * 30;
        let (database_path, run_id) = scanned_database_with_n_items("huge-result", total);
        let summary = minimal_summary_for(run_id, total);
        let first_page_names = expect_filenames(&database_path, run_id, 0, DISCOVERY_PAGE_SIZE);
        let last_page_names = expect_filenames(
            &database_path,
            run_id,
            total as i64 - DISCOVERY_PAGE_SIZE,
            DISCOVERY_PAGE_SIZE,
        );
        let on_first_page = &first_page_names[0];
        let only_on_last_page = last_page_names
            .iter()
            .find(|name| !first_page_names.contains(name))
            .expect("a 6,000-item result has names outside its own first page");

        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_collection_discovery_panel(
                    ui,
                    Some(&summary),
                    Some((database_path.as_path(), run_id)),
                );
            });
        });

        assert!(rendered_text_contains(
            &output,
            &format!("{} items", format_count(total))
        ));
        // Only the first page's worth of filenames were ever rendered as
        // widgets - the rest of a 6,000-item result never became egui
        // shapes at all.
        assert!(rendered_text_contains(&output, on_first_page));
        assert!(!rendered_text_contains(&output, only_on_last_page));
    }
}
