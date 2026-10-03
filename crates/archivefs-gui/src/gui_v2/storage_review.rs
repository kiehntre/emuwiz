//! "Where is my space going?": a read-only storage review. Classification,
//! eligibility and savings come only from the core storage-health analysis and
//! the conversion-tool capability probe; this file groups and presents them.
//! Nothing here converts, moves or deletes; the only actions are navigation.
use super::{App, Route, Section, backend::Command, library::SharedLibrary};
use crate::ui::theme;
use archivefs_core::storage_conversion::{
    ConversionToolInventory, ToolCapabilityStatus, capability_for_item, probe_conversion_tools,
};
use archivefs_core::storage_health::{
    ConversionEligibility, StorageEstimateKind, StorageHealthInput, StorageHealthItem,
    StorageHealthReport, StorageOpportunityKind, analyze_storage_health,
};
use eframe::egui::{self, RichText};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Group {
    CanShrink,
    NeedsTool,
    SameGameTwice,
    WontRecommend,
    Efficient,
}

impl Group {
    pub(super) const ALL: [Group; 5] = [
        Group::CanShrink,
        Group::NeedsTool,
        Group::SameGameTwice,
        Group::WontRecommend,
        Group::Efficient,
    ];
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::CanShrink => "Can shrink safely",
            Self::NeedsTool => "Needs a tool first",
            Self::SameGameTwice => "Could be the same game twice",
            Self::WontRecommend => "EmuWiz won't recommend changing these",
            Self::Efficient => "Already efficient",
        }
    }
    pub(super) fn meaning(self) -> &'static str {
        match self {
            Self::CanShrink => {
                "A conversion is available on this computer and EmuWiz knows how to check the result. Nothing changes until you choose it in the Converter."
            }
            Self::NeedsTool => {
                "These could be compressed, but the program that does it isn't installed. EmuWiz never installs anything automatically."
            }
            Self::SameGameTwice => {
                "The contents look the same as another file. No saving is assumed until you review them in Duplicates."
            }
            Self::WontRecommend => {
                "EmuWiz can't prove a conversion would be safe or reversible enough, so it offers no action. Reasons are under Details."
            }
            Self::Efficient => "Already stored in a space-efficient format. Nothing to do.",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ItemView {
    pub path: String,
    pub platform: String,
    pub format: String,
    pub logical: Option<u64>,
    pub allocated: Option<u64>,
    pub group: Group,
    pub savings: Option<(u64, u64)>,
    pub estimate: &'static str,
    pub why: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct GroupTotals {
    pub items: usize,
    pub logical: u64,
    /// Sum of ranges for items that have one; `unmeasured` counts the rest.
    pub savings: (u64, u64),
    pub measured: usize,
    pub unmeasured: usize,
    pub per_platform: BTreeMap<String, (usize, u64)>,
}

#[derive(Debug, Default)]
pub(super) struct StorageReview {
    pub total_logical: u64,
    pub items: Vec<ItemView>,
    pub totals: BTreeMap<Group, GroupTotals>,
    pub tools: Vec<String>,
    pub notes: Vec<String>,
}

fn estimate_label(kind: StorageEstimateKind) -> &'static str {
    match kind {
        StorageEstimateKind::ExactMeasured => "measured",
        StorageEstimateKind::EstimatedRange => "estimated range",
        StorageEstimateKind::RoughOpportunity => "rough guess",
        StorageEstimateKind::Unknown => "can't be measured",
    }
}

fn group_of(item: &StorageHealthItem, eligibility: ConversionEligibility) -> Group {
    match item.opportunity.kind {
        StorageOpportunityKind::AlreadyEfficient => Group::Efficient,
        StorageOpportunityKind::PossibleDuplicateContent => Group::SameGameTwice,
        StorageOpportunityKind::Compressible => match eligibility {
            ConversionEligibility::ConversionReady => Group::CanShrink,
            ConversionEligibility::ToolMissing => Group::NeedsTool,
            ConversionEligibility::FormatAlreadyEfficient => Group::Efficient,
            _ => Group::WontRecommend,
        },
        _ => Group::WontRecommend,
    }
}

/// Pure projection of the backend's answers; no new decisions.
pub(super) fn project(
    report: &StorageHealthReport,
    inventory: &ConversionToolInventory,
) -> StorageReview {
    let mut review = StorageReview {
        total_logical: report.total_logical_size_bytes,
        ..StorageReview::default()
    };
    for tool in &inventory.tools {
        review.tools.push(format!(
            "{}: {}",
            tool.name,
            match tool.status {
                ToolCapabilityStatus::Missing => "not installed".to_string(),
                _ => tool
                    .path
                    .as_ref()
                    .map_or("found".into(), |p| p.display().to_string()),
            }
        ));
    }
    review.notes = report.warnings.iter().map(|w| w.message.clone()).collect();
    for item in &report.items {
        let capability = capability_for_item(item, inventory);
        let group = group_of(item, capability.eligibility);
        let estimate = &item.opportunity.estimate;
        let savings = match (
            estimate.minimum_savings_bytes,
            estimate.maximum_savings_bytes,
        ) {
            (Some(min), Some(max)) if group == Group::CanShrink || group == Group::NeedsTool => {
                Some((min, max))
            }
            _ => None,
        };
        let mut why = vec![format!(
            "Savings: {} — {}",
            estimate_label(estimate.kind),
            estimate.explanation
        )];
        why.push(format!("Round trip: {}", capability.round_trip));
        if !capability.verification_required.is_empty() {
            why.push(format!("Checked by: {}", capability.verification_required));
        }
        if let Some(tool) = &capability.tool {
            why.push(format!(
                "Tool: {tool}{}{}",
                capability
                    .mode
                    .as_ref()
                    .map_or(String::new(), |m| format!(" ({m})")),
                capability
                    .tool_version
                    .as_ref()
                    .map_or(String::new(), |v| format!(" · {v}"))
            ));
        }
        why.push(format!("Cleanup: {}", item.opportunity.cleanup_eligibility));
        if let Some(warning) = &item.opportunity.warning {
            why.push(format!("Warning: {warning}"));
        }
        why.extend(
            item.warnings
                .iter()
                .map(|w| format!("{}: {}", w.code, w.message)),
        );
        let platform = item
            .platform
            .clone()
            .unwrap_or_else(|| "Unknown system".into());
        let totals = review.totals.entry(group).or_default();
        totals.items += 1;
        totals.logical += item.logical_size_bytes.unwrap_or(0);
        match savings {
            Some((min, max)) => {
                totals.savings.0 += min;
                totals.savings.1 += max;
                totals.measured += 1;
            }
            None => totals.unmeasured += 1,
        }
        let entry = totals.per_platform.entry(platform.clone()).or_default();
        entry.0 += 1;
        entry.1 += item.logical_size_bytes.unwrap_or(0);
        review.items.push(ItemView {
            path: item.path.display().to_string(),
            platform,
            format: item.format.to_string(),
            logical: item.logical_size_bytes,
            allocated: item.allocated_size_bytes,
            group,
            savings,
            estimate: estimate_label(estimate.kind),
            why,
        });
    }
    review
}

/// Headline: only backed numbers; never sums unknowns into a total.
pub(super) fn headline(review: &StorageReview) -> String {
    let (min, max, measured) = [Group::CanShrink, Group::NeedsTool]
        .iter()
        .filter_map(|g| review.totals.get(g))
        .fold((0u64, 0u64, 0usize), |a, t| {
            (a.0 + t.savings.0, a.1 + t.savings.1, a.2 + t.measured)
        });
    if measured == 0 {
        let compressible: usize = [Group::CanShrink, Group::NeedsTool]
            .iter()
            .filter_map(|g| review.totals.get(g))
            .map(|t| t.items)
            .sum();
        return if compressible > 0 {
            format!(
                "{compressible} item(s) could be compressed, but the saving can't be measured without trying it in the Converter."
            )
        } else {
            "No savings can be measured right now.".into()
        };
    }
    if min == max {
        format!("About {} could be freed.", bytes(min))
    } else {
        format!("About {}–{} could be freed.", bytes(min), bytes(max))
    }
}

pub(super) fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ---- worker side ---------------------------------------------------------

pub(super) fn inputs(library: &SharedLibrary) -> Vec<StorageHealthInput> {
    library
        .games
        .iter()
        .map(|game| StorageHealthInput {
            path: game.archive.absolute_path.clone(),
            platform: game.archive.platform.clone(),
            logical_size_bytes: game.archive.size_bytes,
            format_hint: Some(game.archive.archive_kind.clone()),
            content_hash: game
                .archive
                .identity_report
                .as_ref()
                .and_then(|report| report.verified_loose_rom_sha256().map(str::to_owned)),
        })
        .collect()
}

pub(super) fn analyse(library: &SharedLibrary) -> StorageReview {
    let report = analyze_storage_health(&inputs(library));
    project(&report, &probe_conversion_tools())
}

// ---- UI side -------------------------------------------------------------

#[derive(Default)]
pub(super) struct StorageState {
    pub review: Option<Arc<StorageReview>>,
    pub job: Option<u64>,
    pub failed: bool,
    /// Identity of the library the review was computed for.
    pub for_library: usize,
}

impl StorageState {
    fn should_start(&self, stale: bool) -> bool {
        (self.review.is_none() || stale) && self.job.is_none() && !self.failed
    }
    fn accept(&mut self, current_library: usize, review: StorageReview) {
        self.job = None;
        if self.for_library == current_library {
            self.review = Some(Arc::new(review));
        }
    }
}

const SHOWN_PER_GROUP: usize = 25;

impl App {
    fn library_key(&self) -> usize {
        Arc::as_ptr(&self.library) as usize
    }

    fn start_storage_review(&mut self) {
        if self.storage.job.is_some() {
            return;
        }
        self.storage.failed = false;
        self.storage.review = None;
        let id = self.activity.queue(
            "Measuring your library",
            Route::Section(Section::Storage),
            false,
        );
        self.storage.job = Some(id);
        self.storage.for_library = self.library_key();
        self.send(
            id,
            Command::StorageReview {
                library: self.library.clone(),
            },
        );
    }

    pub(super) fn storage_review_done(&mut self, review: StorageReview) {
        self.storage.accept(self.library_key(), review);
    }

    pub(super) fn storage_review_failed(&mut self) {
        self.storage.job = None;
        self.storage.failed = true;
    }

    pub(super) fn storage_page(&mut self, ui: &mut egui::Ui) {
        if self.library.games.is_empty() {
            ui.heading("Where is my space going?");
            ui.label("There are no games in the catalogue yet, so there is nothing to measure.");
            if ui.button("Open Sources").clicked() {
                self.go(Route::Section(Section::Sources));
            }
            return;
        }
        let stale = self.storage.review.is_some() && self.storage.for_library != self.library_key();
        if self.storage.should_start(stale) {
            self.start_storage_review();
        }
        ui.heading("Where is my space going?");
        if self.storage.failed {
            ui.colored_label(theme::WARNING, "Storage review could not finish. Check the drive and try Check again. No files were changed; Activity has the error details.");
            if ui.button("Open Activity details").clicked() {
                self.go(Route::Section(Section::Activity));
            }
        }
        if stale {
            ui.label(
                "The library changed. Wait for the new check before relying on space estimates.",
            );
        }
        ui.label("This review measures your catalogue and checks which files could be stored more compactly. Nothing is changed, converted or deleted here.");
        ui.horizontal_wrapped(|ui| {
            if self.storage.job.is_some() {
                ui.spinner();
                ui.label("Measuring your library… you can keep browsing.");
            } else if ui.button("Check again").clicked() {
                self.storage.review = None;
                self.start_storage_review();
            }
        });
        let Some(review) = self.storage.review.clone() else {
            return;
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new(format!("Your catalogue is {}.", bytes(review.total_logical))).strong());
            ui.label(headline(&review));
            let unmeasured: usize = [Group::CanShrink, Group::NeedsTool]
                .iter()
                .filter_map(|g| review.totals.get(g))
                .map(|t| t.unmeasured)
                .sum();
            let measured: usize = [Group::CanShrink, Group::NeedsTool]
                .iter()
                .filter_map(|g| review.totals.get(g))
                .map(|t| t.measured)
                .sum();
            if unmeasured > 0 && measured > 0 {
                ui.small(format!("{unmeasured} compressible item(s) have no measurable saving yet and aren't counted."));
            }
        });
        for group in Group::ALL {
            let Some(totals) = review.totals.get(&group) else {
                continue;
            };
            self.storage_group(ui, &review, group, totals);
        }
        ui.add_space(6.0);
        ui.collapsing("Conversion tools on this computer", |ui| {
            for line in &review.tools {
                ui.small(line);
            }
            for note in &review.notes {
                ui.small(note);
            }
        });
    }

    fn storage_group(
        &mut self,
        ui: &mut egui::Ui,
        review: &StorageReview,
        group: Group,
        totals: &GroupTotals,
    ) {
        let mut header = format!(
            "{} ({}) · {}",
            group.title(),
            totals.items,
            bytes(totals.logical)
        );
        if totals.measured > 0 && matches!(group, Group::CanShrink | Group::NeedsTool) {
            header.push_str(&format!(
                " · about {}–{} could be freed",
                bytes(totals.savings.0),
                bytes(totals.savings.1)
            ));
        }
        egui::CollapsingHeader::new(RichText::new(header).strong())
            .id_salt(("storage_group", group as u8))
            .default_open(matches!(group, Group::CanShrink | Group::NeedsTool))
            .show(ui, |ui| {
                ui.label(group.meaning());
                if matches!(group, Group::CanShrink | Group::NeedsTool) {
                    for (platform, (count, size)) in &totals.per_platform {
                        ui.small(format!("{platform}: {count} item(s), {}", bytes(*size)));
                    }
                }
                if group == Group::CanShrink
                    && ui
                        .add(
                            egui::Button::new(RichText::new("Open Converter").strong())
                                .fill(theme::PRIMARY_ACTION),
                        )
                        .clicked()
                {
                    self.go(Route::Section(Section::Converter));
                }
                if group == Group::SameGameTwice && ui.button("Review in Duplicates").clicked() {
                    self.go(Route::Section(Section::Duplicates));
                }
                if group == Group::Efficient {
                    return;
                }
                let items: Vec<&ItemView> =
                    review.items.iter().filter(|i| i.group == group).collect();
                for item in items.iter().take(SHOWN_PER_GROUP) {
                    ui.push_id(&item.path, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(file_name(&item.path)).strong());
                            ui.small(format!(
                                "{} · {} · {}",
                                item.platform,
                                item.format,
                                item.logical.map_or("size unknown".into(), bytes)
                            ));
                            if let Some((min, max)) = item.savings {
                                ui.small(format!(
                                    "saves ~{}–{} ({})",
                                    bytes(min),
                                    bytes(max),
                                    item.estimate
                                ));
                            }
                        });
                        ui.collapsing(format!("Why — {}", file_name(&item.path)), |ui| {
                            ui.small(&item.path);
                            if let Some(allocated) = item.allocated {
                                ui.small(format!("Space on disk: {}", bytes(allocated)));
                            }
                            for line in &item.why {
                                ui.small(line);
                            }
                        });
                    });
                }
                if items.len() > SHOWN_PER_GROUP {
                    ui.small(format!(
                        "…and {} more not listed here.",
                        items.len() - SHOWN_PER_GROUP
                    ));
                }
            });
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::storage_conversion::ConversionToolRecord;
    use std::fs;
    use std::path::PathBuf;

    fn tools(chdman_found: bool) -> ConversionToolInventory {
        ConversionToolInventory {
            tools: vec![ConversionToolRecord {
                name: "chdman".into(),
                path: chdman_found.then(|| PathBuf::from("/usr/bin/chdman")),
                version: chdman_found.then(|| "0.264".into()),
                status: if chdman_found {
                    ToolCapabilityStatus::VersionSupported
                } else {
                    ToolCapabilityStatus::Missing
                },
                capabilities: vec!["createcd".into()],
                capability_source: "test".into(),
            }],
        }
    }

    /// Tiny synthetic library on disk: a CUE/BIN set, a CHD, a zip and two same-hash files.
    fn fixture() -> (tempfile::TempDir, Vec<StorageHealthInput>) {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, len: usize| {
            let path = dir.path().join(name);
            fs::write(&path, vec![7u8; len]).unwrap();
            path
        };
        let cue = dir.path().join("Disc A.cue");
        fs::write(
            &cue,
            "FILE \"Disc A.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        write("Disc A.bin", 4704);
        let input = |path: PathBuf, platform: &str, hash: Option<&str>| StorageHealthInput {
            logical_size_bytes: fs::metadata(&path).ok().map(|m| m.len()),
            path,
            platform: Some(platform.into()),
            format_hint: None,
            content_hash: hash.map(str::to_owned),
        };
        let inputs = vec![
            input(cue, "PlayStation", None),
            input(write("Done.chd", 3000), "PlayStation", None),
            input(write("Pack.zip", 2000), "NES", None),
            input(write("Twin 1.z64", 1000), "Nintendo 64", Some("same")),
            input(write("Twin 2.v64", 1000), "Nintendo 64", Some("same")),
        ];
        (dir, inputs)
    }

    fn groups(review: &StorageReview) -> BTreeMap<Group, Vec<String>> {
        let mut map: BTreeMap<Group, Vec<String>> = BTreeMap::new();
        for item in &review.items {
            map.entry(item.group).or_default().push(
                std::path::Path::new(&item.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into(),
            );
        }
        map
    }

    #[test]
    fn failed_storage_review_requires_explicit_retry() {
        let mut state = StorageState::default();
        assert!(state.should_start(false));
        state.failed = true;
        assert!(!state.should_start(false));
        assert!(!state.should_start(true));
        state.failed = false;
        state.job = Some(1);
        assert!(!state.should_start(false));
    }
    #[test]
    fn storage_result_from_old_library_is_not_published() {
        let mut state = StorageState {
            for_library: 1,
            job: Some(1),
            ..Default::default()
        };
        state.accept(2, StorageReview::default());
        assert!(state.review.is_none());
        assert!(state.job.is_none());
        state.accept(1, StorageReview::default());
        assert!(state.review.is_some());
    }

    #[test]
    fn real_analysis_puts_each_file_in_a_group_that_matches_the_backend() {
        let (_dir, inputs) = fixture();
        let report = analyze_storage_health(&inputs);
        let review = project(&report, &tools(true));
        let groups = groups(&review);
        assert!(groups[&Group::Efficient].contains(&"Done.chd".to_string()));
        assert!(
            groups[&Group::SameGameTwice].len() == 2,
            "same-hash pair: {groups:?}"
        );
        // The CUE is either shrinkable (tool present) or explained; never silently dropped.
        assert_eq!(review.items.len(), inputs.len());
        assert_eq!(
            review.totals.values().map(|t| t.items).sum::<usize>(),
            inputs.len()
        );
    }

    #[test]
    fn missing_tool_moves_a_compressible_disc_to_needs_a_tool() {
        let (_dir, inputs) = fixture();
        let report = analyze_storage_health(&inputs);
        let with = groups(&project(&report, &tools(true)));
        let without = groups(&project(&report, &tools(false)));
        let cue = "Disc A.cue".to_string();
        assert!(with[&Group::CanShrink].contains(&cue));
        assert!(without[&Group::NeedsTool].contains(&cue));
        assert!(!without.contains_key(&Group::CanShrink));
    }

    #[test]
    fn unmeasurable_savings_are_never_turned_into_a_number() {
        let (_dir, inputs) = fixture();
        let review = project(&analyze_storage_health(&inputs), &tools(true));
        for item in &review.items {
            if item.savings.is_none() {
                assert!(!item.why.is_empty());
            }
        }
        assert_eq!(
            headline(&StorageReview::default()),
            "No savings can be measured right now."
        );
        // The disc is shrinkable but the backend gives no range: say so, no invented number.
        assert!(
            headline(&review).contains("can't be measured"),
            "{}",
            headline(&review)
        );
    }

    #[test]
    fn ranged_savings_sum_as_a_range() {
        let mut review = StorageReview::default();
        review.totals.insert(
            Group::CanShrink,
            GroupTotals {
                items: 2,
                savings: (1024 * 1024, 3 * 1024 * 1024),
                measured: 2,
                ..Default::default()
            },
        );
        review.totals.insert(
            Group::NeedsTool,
            GroupTotals {
                items: 1,
                savings: (1024 * 1024, 1024 * 1024),
                measured: 1,
                ..Default::default()
            },
        );
        assert_eq!(headline(&review), "About 2.0 MB–4.0 MB could be freed.");
    }

    #[test]
    fn wont_recommend_items_carry_a_reason_and_no_savings() {
        let (_dir, inputs) = fixture();
        let review = project(&analyze_storage_health(&inputs), &tools(true));
        for item in review
            .items
            .iter()
            .filter(|i| i.group == Group::WontRecommend)
        {
            assert!(item.savings.is_none());
            assert!(item.why.iter().any(|l| l.starts_with("Round trip")));
        }
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(bytes(10), "10 B");
        assert_eq!(bytes(1536), "1.5 KB");
    }
}
