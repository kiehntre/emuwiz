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

pub(super) fn show_collection_discovery_panel(
    ui: &mut egui::Ui,
    summary: Option<&ScanPersistSummary>,
) {
    let Some(summary) = summary else {
        widgets::card(ui, |ui| {
            ui.label("No scan has completed yet this session.");
            ui.label(
                egui::RichText::new(
                    "Run a scan from the Sources page, then come back here to see what \
                     EmuWiz found in your collection.",
                )
                .color(theme::muted(ui)),
            );
        });
        return;
    };

    show_found_summary(ui, summary);
    ui.add_space(theme::SECTION_GAP);
    show_needs_attention_summary(ui, &summary.ingestion_skip_reasons);
    ui.add_space(theme::SECTION_GAP);
    show_platform_breakdown(ui, summary);
    ui.add_space(theme::SECTION_GAP);
    show_item_details(ui, summary);
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
        ContentKind::WhdloadInstall => "WHDLoad install",
        ContentKind::ExtractedGameFolder => "Game folder",
    }
}

fn show_item_details(ui: &mut egui::Ui, summary: &ScanPersistSummary) {
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
    if let Some(action) = suggested_action {
        ui.label(
            egui::RichText::new(format!("Suggested: {action}"))
                .small()
                .color(theme::muted(ui)),
        );
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
                show_collection_discovery_panel(ui, Some(&summary));
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
                show_collection_discovery_panel(ui, Some(&summary));
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
}
