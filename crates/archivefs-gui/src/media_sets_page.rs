//! Read-only presentation of the core media-set topology and swap plan.
//!
//! The catalogue currently stores archive observations, not a second
//! persisted topology.  This page therefore derives a bounded projection from
//! the already-loaded catalogue rows.  It never walks source folders, opens a
//! media file, or writes state.  Filename-derived sets are deliberately shown
//! as unverified until the existing topology engine has stronger evidence.

use archivefs_core::{PersistedArchive, media_set::*};
use eframe::egui;
use std::path::Path;

const PAGE_SIZE: usize = 40;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MediaSetFilter {
    #[default]
    All,
    Complete,
    Incomplete,
    Ambiguous,
    Conflicting,
    Unverified,
    Optical,
    Floppy,
    Tape,
}

#[derive(Debug, Default)]
pub(crate) struct MediaSetsPageState {
    pub(crate) sets: Vec<MediaSet>,
    pub(crate) generation: Option<u64>,
    pub(crate) filter: MediaSetFilter,
    pub(crate) page: usize,
    pub(crate) selected: Option<usize>,
}

impl MediaSetsPageState {
    pub(crate) fn refresh(&mut self, archives: &[PersistedArchive], generation: u64) {
        if self.generation == Some(generation) {
            return;
        }
        let records = archives.iter().filter_map(record_from_catalogue).collect();
        self.sets = resolve_index(index_media(records)).sets;
        self.generation = Some(generation);
        self.page = 0;
        self.selected = None;
    }

    fn visible_indices(&self) -> Vec<usize> {
        self.sets
            .iter()
            .enumerate()
            .filter(|(_, set)| matches_filter(set, self.filter))
            .map(|(index, _)| index)
            .collect()
    }

    pub(crate) fn set_containing_path(&self, path: &Path) -> Option<&MediaSet> {
        self.sets.iter().find(|set| {
            set.members.iter().any(|member| {
                member
                    .representations
                    .iter()
                    .any(|representation| representation.record.source.path == path)
            })
        })
    }
}

fn family_for_extension(extension: &str) -> Option<MediaFamily> {
    match extension.to_ascii_lowercase().as_str() {
        "chd" | "cue" | "bin" | "iso" | "gdi" | "cdi" | "mds" | "ccd" => Some(MediaFamily::Optical),
        "adf" | "dsk" | "ipf" | "st" | "stx" | "img" => Some(MediaFamily::Floppy),
        "tap" | "tzx" | "tape" | "cas" => Some(MediaFamily::Tape),
        _ => None,
    }
}

fn record_from_catalogue(archive: &PersistedArchive) -> Option<MediaRecord> {
    let extension = archive
        .absolute_path
        .extension()
        .and_then(|value| value.to_str())?;
    let family = family_for_extension(extension)?;
    let platform = catalogue_platform_hint(archive);
    let mut evidence = filename_evidence(&archive.display_name, Some(family));
    if let Some(platform) = platform.as_deref() {
        evidence
            .notes
            .push(format!("Catalogue platform assignment: {platform}"));
    }
    Some(MediaRecord {
        source: MediaSource {
            path: archive.absolute_path.clone(),
            archive_member: None,
        },
        platform,
        family: Some(family),
        format: extension.to_ascii_lowercase(),
        availability: MediaAvailability::Observed,
        evidence: vec![evidence],
        warnings: vec!["Derived from catalogue filename/path evidence; content and DAT evidence were not re-read.".into()],
    })
}

/// Reuse the existing platform-assignment vocabulary when the catalogue row
/// has not retained an explicit assignment. The first relative-path component
/// is the configured library's platform folder (for example `dc`), not a
/// filename guess. Unknown or ambiguous aliases remain unresolved.
fn catalogue_platform_hint(archive: &PersistedArchive) -> Option<String> {
    catalogue_platform_from_assignment_or_path(archive.platform.as_deref(), &archive.relative_path)
}

fn catalogue_platform_from_assignment_or_path(
    assigned: Option<&str>,
    relative_path: &Path,
) -> Option<String> {
    if let Some(platform) = assigned {
        return Some(platform.to_owned());
    }
    let component = relative_path
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })?;
    archivefs_core::canonical_platform_for_alias(component).map(str::to_owned)
}

fn matches_filter(set: &MediaSet, filter: MediaSetFilter) -> bool {
    match filter {
        MediaSetFilter::All => true,
        MediaSetFilter::Complete => set.state == MediaSetState::CompleteSet,
        MediaSetFilter::Incomplete => set.state == MediaSetState::IncompleteSet,
        MediaSetFilter::Ambiguous => set.state == MediaSetState::AmbiguousSet,
        MediaSetFilter::Conflicting => set.state == MediaSetState::ConflictingSet,
        MediaSetFilter::Unverified => set.state == MediaSetState::UnverifiedSet,
        MediaSetFilter::Optical => set.family == Some(MediaFamily::Optical),
        MediaSetFilter::Floppy => set.family == Some(MediaFamily::Floppy),
        MediaSetFilter::Tape => set.family == Some(MediaFamily::Tape),
    }
}

fn state_label(state: MediaSetState) -> &'static str {
    match state {
        MediaSetState::CompleteSet => "Complete set",
        MediaSetState::IncompleteSet => "Incomplete set",
        MediaSetState::AmbiguousSet => "Ambiguous set",
        MediaSetState::ConflictingSet => "Conflicting set",
        MediaSetState::UnverifiedSet => "Unverified set",
        MediaSetState::UnsupportedSet => "Unsupported set",
    }
}

fn state_tone(state: MediaSetState) -> super::widgets::StatusTone {
    match state {
        MediaSetState::CompleteSet => super::widgets::StatusTone::Success,
        MediaSetState::IncompleteSet | MediaSetState::UnsupportedSet => {
            super::widgets::StatusTone::Warning
        }
        MediaSetState::AmbiguousSet | MediaSetState::ConflictingSet => {
            super::widgets::StatusTone::Blocked
        }
        MediaSetState::UnverifiedSet => super::widgets::StatusTone::Pending,
    }
}

fn action_safety(state: MediaSetState) -> &'static str {
    match state {
        MediaSetState::CompleteSet => "SAFE_TO_ACT (presentation only)",
        MediaSetState::ConflictingSet => "BLOCKED",
        MediaSetState::AmbiguousSet
        | MediaSetState::IncompleteSet
        | MediaSetState::UnverifiedSet
        | MediaSetState::UnsupportedSet => "REVIEW_REQUIRED",
    }
}

fn family_label(family: Option<MediaFamily>) -> &'static str {
    match family {
        Some(MediaFamily::Optical) => "Optical",
        Some(MediaFamily::Floppy) => "Floppy",
        Some(MediaFamily::Tape) => "Tape",
        None => "Unknown media",
    }
}

fn ordinal_label(
    ordinal: Option<&MediaOrdinal>,
    side: Option<&MediaSide>,
    family: Option<MediaFamily>,
) -> String {
    let base = ordinal
        .map(|o| match o.unit {
            OrdinalUnit::Disc => format!("Disc {}", o.number),
            OrdinalUnit::Disk => format!("Disk {}", o.number),
            OrdinalUnit::Tape => format!("Tape {}", o.number),
            OrdinalUnit::Medium => format!("Medium {}", o.number),
            OrdinalUnit::Part => format!("Part {}", o.number),
            OrdinalUnit::Reel => format!("Reel {}", o.number),
        })
        .unwrap_or_else(|| match family {
            Some(MediaFamily::Optical) => "Disc (number unknown)".into(),
            Some(MediaFamily::Floppy) => "Disk (number unknown)".into(),
            Some(MediaFamily::Tape) => "Tape (number unknown)".into(),
            None => "Media (number unknown)".into(),
        });
    side.map(|s| format!("{base} · Side {}", s.number))
        .unwrap_or(base)
}

fn completeness_line(set: &MediaSet) -> String {
    let expected = set
        .expected_count
        .as_ref()
        .map(|(count, _)| count.count as usize);
    match (expected, set.state) {
        (Some(expected), MediaSetState::CompleteSet) => format!(
            "{} of {} expected media present",
            set.members.len(),
            expected
        ),
        (Some(expected), _) => format!(
            "{} of {} expected media detected",
            set.members.len(),
            expected
        ),
        (None, _) => "Expected media count is unknown; completeness is unverified.".into(),
    }
}

fn title(set: &MediaSet) -> String {
    if set.identity.key.namespace == "provisional-title" {
        set.identity.key.value.clone()
    } else {
        format!("{}:{}", set.identity.key.namespace, set.identity.key.value)
    }
}

fn source_name(path: &Path) -> String {
    path.display().to_string()
}

fn show_plan(ui: &mut egui::Ui, plan: &MediaSwapPlan) {
    ui.heading("Media swap plan");
    ui.label("Inspection only. EmuWiz will not execute swaps or create a playlist here.");
    if plan.ordered_media.is_empty() {
        ui.label("No ordered media steps are available.");
    } else {
        for (index, step) in plan.ordered_media.iter().enumerate() {
            let action = if index == 0 {
                "Start with"
            } else {
                match plan.transitions.get(index - 1).map(|t| t.kind) {
                    Some(TransitionKind::ChangeSide) => "Then flip to",
                    Some(TransitionKind::LoaderToProgram) | Some(TransitionKind::ProgramToData) => {
                        "Then load"
                    }
                    _ => "When prompted, switch to",
                }
            };
            ui.label(format!(
                "{action}: {}",
                ordinal_label(
                    step.ordinal.as_ref(),
                    step.side.as_ref(),
                    plan.semantics.map(|s| match s {
                        SwapSemantics::OpticalSequence => MediaFamily::Optical,
                        SwapSemantics::FloppySwap => MediaFamily::Floppy,
                        SwapSemantics::TapeLoad => MediaFamily::Tape,
                    })
                )
            ));
            if let Some(path) = &step.preferred_representation {
                ui.small(format!("Preferred: {}", source_name(&path.path)));
            } else if !step.alternatives.is_empty() {
                ui.small(format!(
                    "{} representation(s); no preferred choice proven.",
                    step.alternatives.len()
                ));
            }
        }
    }
    for warning in &plan.warnings {
        ui.colored_label(super::theme::WARNING, warning);
    }
    for blocker in &plan.blockers {
        ui.colored_label(
            super::theme::DANGER,
            format!("Review required: {}", blocker.detail),
        );
    }
}

pub(crate) fn show_media_sets_page(ui: &mut egui::Ui, state: &mut MediaSetsPageState) {
    super::widgets::page_header_with_icon(
        ui,
        super::ui::icons::GAMES,
        "Media Sets",
        "Inspect multi-media releases and their read-only swap plans.",
    );
    ui.label("This view uses the loaded catalogue only. It does not scan folders or change files.");
    ui.horizontal_wrapped(|ui| {
        for (filter, label) in [
            (MediaSetFilter::All, "All"),
            (MediaSetFilter::Complete, "Complete"),
            (MediaSetFilter::Incomplete, "Incomplete"),
            (MediaSetFilter::Ambiguous, "Ambiguous"),
            (MediaSetFilter::Conflicting, "Conflicting"),
            (MediaSetFilter::Unverified, "Unverified"),
            (MediaSetFilter::Optical, "Optical"),
            (MediaSetFilter::Floppy, "Floppy"),
            (MediaSetFilter::Tape, "Tape"),
        ] {
            if ui.selectable_label(state.filter == filter, label).clicked() {
                state.filter = filter;
                state.page = 0;
                state.selected = None;
            }
        }
    });
    let visible = state.visible_indices();
    if visible.is_empty() {
        ui.add_space(16.0);
        ui.strong("No media sets found.");
        ui.label("Media-set evidence has not been collected for the current catalogue, or no supported optical, floppy, or tape files were indexed.");
        return;
    }
    let pages = visible.len().div_ceil(PAGE_SIZE);
    state.page = state.page.min(pages.saturating_sub(1));
    ui.horizontal(|ui| {
        ui.label(format!("{} set(s)", visible.len()));
        ui.separator();
        ui.label(format!("Page {} of {}", state.page + 1, pages));
        if ui
            .add_enabled(state.page > 0, egui::Button::new("Previous"))
            .clicked()
        {
            state.page -= 1;
        }
        if ui
            .add_enabled(state.page + 1 < pages, egui::Button::new("Next"))
            .clicked()
        {
            state.page += 1;
        }
    });
    let start = state.page * PAGE_SIZE;
    for index in visible.into_iter().skip(start).take(PAGE_SIZE) {
        let set = &state.sets[index];
        let selected = state.selected == Some(index);
        super::widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(title(set));
                super::widgets::status_badge(ui, state_label(set.state), state_tone(set.state));
                ui.label(family_label(set.family));
                if ui
                    .button(if selected {
                        "Hide details"
                    } else {
                        "Inspect set"
                    })
                    .clicked()
                {
                    state.selected = (!selected).then_some(index);
                }
            });
            ui.label(format!(
                "Platform: {} · Members detected: {} · Confidence: {:?}",
                set.platform.as_deref().unwrap_or("Unknown"),
                set.members.len(),
                set.confidence
            ));
            ui.small(format!(
                "Action safety: {} · Launch readiness is not evaluated by this read-only view.",
                action_safety(set.state)
            ));
            ui.label(completeness_line(set));
            if !set.conflicts.is_empty() {
                for conflict in &set.conflicts {
                    ui.colored_label(
                        if conflict.blocking {
                            super::theme::DANGER
                        } else {
                            super::theme::WARNING
                        },
                        &conflict.detail,
                    );
                }
            }
            if !set.warnings.is_empty() {
                ui.small(&set.warnings[0]);
            }
            if selected {
                ui.separator();
                ui.heading("Members");
                for member in &set.members {
                    ui.group(|ui| {
                        ui.strong(ordinal_label(
                            member.ordinal.as_ref(),
                            member.sides.iter().next(),
                            set.family,
                        ));
                        ui.label(format!(
                            "Role: {:?} · {} representation(s)",
                            member.role,
                            member.representations.len()
                        ));
                        for representation in &member.representations {
                            ui.label(format!(
                                "{} · {} · {:?}",
                                source_name(&representation.record.source.path),
                                representation.record.format,
                                representation.confidence
                            ));
                        }
                    });
                }
                for conflict in &set.conflicts {
                    if matches!(
                        conflict.kind,
                        ConflictKind::MissingMedium | ConflictKind::MissingSide
                    ) {
                        ui.group(|ui| {
                            ui.strong("MISSING");
                            ui.label(&conflict.detail);
                            ui.small(
                                "No path is shown because the expected media was not observed.",
                            );
                        });
                    }
                }
                show_plan(ui, &media_swap_plan(set, None));
            }
        });
    }
}

/// Compact read-only link for the Selected/Game Details surface.  It keeps
/// the item-detail route useful without copying topology into the selected
/// game model or adding an action that could mutate the catalogue.
pub(crate) fn show_selected_item_link(
    ui: &mut egui::Ui,
    state: &MediaSetsPageState,
    path: &Path,
) -> bool {
    let Some(set) = state.set_containing_path(path) else {
        return false;
    };
    let mut open = false;
    super::widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.strong("Media set");
            ui.label(title(set));
            super::widgets::status_badge(ui, state_label(set.state), state_tone(set.state));
        });
        ui.label(format!(
            "{} · {}",
            family_label(set.family),
            completeness_line(set)
        ));
        if ui.button("Inspect media set and swap plan").clicked() {
            open = true;
        }
    });
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::media_set::{EvidenceKind, IdentityKey, MediaEvidence};
    use std::path::PathBuf;

    fn fixture(family: MediaFamily, number: u16, title: &str, path: &str) -> MediaRecord {
        let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, "fixture");
        evidence.release = Some(IdentityKey::new("fixture-release", title));
        evidence.medium = Some(IdentityKey::new(
            "fixture-medium",
            format!("{title}:{number}"),
        ));
        evidence.ordinal = Some(MediaOrdinal {
            number,
            unit: match family {
                MediaFamily::Optical => OrdinalUnit::Disc,
                MediaFamily::Floppy => OrdinalUnit::Disk,
                MediaFamily::Tape => OrdinalUnit::Tape,
            },
        });
        evidence.expected_count = Some(ExpectedCount {
            count: 2,
            unit: evidence.ordinal.as_ref().unwrap().unit,
        });
        MediaRecord {
            source: MediaSource {
                path: PathBuf::from(path),
                archive_member: None,
            },
            platform: Some("PlayStation".into()),
            family: Some(family),
            format: "chd".into(),
            availability: MediaAvailability::Observed,
            evidence: vec![evidence],
            warnings: vec![],
        }
    }

    #[test]
    fn all_media_families_have_natural_labels() {
        assert_eq!(family_label(Some(MediaFamily::Optical)), "Optical");
        assert_eq!(
            ordinal_label(
                Some(&MediaOrdinal {
                    number: 2,
                    unit: OrdinalUnit::Disk
                }),
                Some(&MediaSide { number: 2 }),
                Some(MediaFamily::Floppy)
            ),
            "Disk 2 · Side 2"
        );
        assert_eq!(
            ordinal_label(
                Some(&MediaOrdinal {
                    number: 1,
                    unit: OrdinalUnit::Tape
                }),
                None,
                Some(MediaFamily::Tape)
            ),
            "Tape 1"
        );
    }

    #[test]
    fn fixture_topology_preserves_complete_missing_and_competing_states() {
        let complete = resolve_index(index_media(vec![
            fixture(MediaFamily::Optical, 1, "A", "/a/Disc 1 of 2.chd"),
            fixture(MediaFamily::Optical, 2, "A", "/a/Disc 2 of 2.chd"),
        ]))
        .sets;
        assert_eq!(complete[0].state, MediaSetState::CompleteSet);
        let incomplete = resolve_index(index_media(vec![fixture(
            MediaFamily::Optical,
            1,
            "B",
            "/b/Disc 1 of 2.chd",
        )]))
        .sets;
        assert_eq!(incomplete[0].state, MediaSetState::IncompleteSet);
        let competing = resolve_index(index_media(vec![
            fixture(MediaFamily::Optical, 1, "C", "/c/Disc 1 of 2.chd"),
            fixture(MediaFamily::Optical, 2, "C", "/c/a/Disc 2 of 2.chd"),
            fixture(MediaFamily::Optical, 2, "C", "/c/b/Disc 2 of 2.chd"),
        ]))
        .sets;
        assert!(
            competing
                .iter()
                .any(|set| set.state == MediaSetState::AmbiguousSet
                    || set.state == MediaSetState::ConflictingSet)
        );
    }

    #[test]
    fn filter_is_read_only_and_deterministic() {
        let sets = resolve_index(index_media(vec![fixture(
            MediaFamily::Optical,
            1,
            "D",
            "/d/Disc 1 of 2.chd",
        )]))
        .sets;
        assert!(!matches_filter(&sets[0], MediaSetFilter::Complete));
        assert!(matches_filter(&sets[0], MediaSetFilter::Optical));
    }

    #[test]
    fn floppy_tape_and_unknown_count_remain_distinct() {
        let floppy = resolve_index(index_media(vec![
            fixture(MediaFamily::Floppy, 1, "F", "/f/Disk 1 of 2.adf"),
            fixture(MediaFamily::Floppy, 2, "F", "/f/Disk 2 of 2.adf"),
        ]))
        .sets;
        let tape = resolve_index(index_media(vec![
            fixture(MediaFamily::Tape, 1, "T", "/t/Tape 1 of 2.tap"),
            fixture(MediaFamily::Tape, 2, "T", "/t/Tape 2 of 2.tap"),
        ]))
        .sets;
        assert!(
            floppy
                .iter()
                .any(|set| set.family == Some(MediaFamily::Floppy))
        );
        assert!(tape.iter().any(|set| set.family == Some(MediaFamily::Tape)));
        assert!(completeness_line(&floppy[0]).contains("2 of 2 expected media"));
        let mut unknown = fixture(MediaFamily::Optical, 1, "U", "/u/Disc 1.chd");
        unknown.evidence[0].expected_count = None;
        let unknown_set = resolve_index(index_media(vec![unknown])).sets;
        assert!(completeness_line(&unknown_set[0]).contains("unknown"));
    }

    #[test]
    fn swap_plan_is_declarative_and_exposes_alternatives() {
        let mut first = fixture(MediaFamily::Optical, 1, "R", "/r/Disc 1.chd");
        first.evidence[0].release = Some(IdentityKey::new("fixture-release", "R"));
        let mut alternate = first.clone();
        alternate.source.path = "/r/Disc 1.cue".into();
        alternate.format = "cue".into();
        let sets = resolve_index(index_media(vec![first, alternate])).sets;
        let plans = sets
            .iter()
            .map(|set| media_swap_plan(set, None))
            .collect::<Vec<_>>();
        assert!(plans.iter().all(|plan| !plan.ordered_media.is_empty()));
        assert!(plans.iter().all(|plan| {
            plan.ordered_media
                .iter()
                .all(|step| step.preferred_representation.is_none() || step.alternatives.len() <= 1)
        }));
    }

    #[test]
    fn real_library_platform_folder_resolves_multidisc_catalogue_records() {
        let paths = [
            "/mnt/usbdrive/games/dc/Headhunter (Europe) (En,Fr,De,Es) (Disc 1).chd",
            "/mnt/usbdrive/games/dc/Headhunter (Europe) (En,Fr,De,Es) (Disc 2).chd",
        ];
        let records = paths
            .iter()
            .map(|path| {
                let relative = PathBuf::from(path.strip_prefix("/mnt/usbdrive/games/").unwrap());
                let platform = catalogue_platform_from_assignment_or_path(None, &relative);
                let mut evidence = filename_evidence(
                    Path::new(path).file_stem().unwrap().to_str().unwrap(),
                    Some(MediaFamily::Optical),
                );
                evidence.notes.push(format!(
                    "Catalogue platform assignment: {}",
                    platform.as_deref().unwrap()
                ));
                MediaRecord {
                    source: MediaSource {
                        path: PathBuf::from(path),
                        archive_member: None,
                    },
                    platform,
                    family: Some(MediaFamily::Optical),
                    format: "chd".into(),
                    availability: MediaAvailability::Observed,
                    evidence: vec![evidence],
                    warnings: vec![],
                }
            })
            .collect();
        let sets = resolve_index(index_media(records)).sets;
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].platform.as_deref(), Some("Dreamcast"));
        assert_eq!(sets[0].members.len(), 2);
        assert_eq!(sets[0].expected_count, None);
        assert_eq!(sets[0].state, MediaSetState::UnverifiedSet);
        assert!(sets[0]
            .members
            .iter()
            .all(|member| member.ordinal.is_some()));
    }

    #[test]
    fn unknown_library_folder_does_not_fabricate_platform() {
        assert_eq!(
            catalogue_platform_from_assignment_or_path(
                None,
                Path::new("not-a-platform/game/Disc 1.chd")
            ),
            None
        );
    }
}
