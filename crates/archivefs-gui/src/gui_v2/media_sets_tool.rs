//! Native Blue "Media Sets" (boards `D11-LG-MediaSets`, `-MS*`): every
//! multi-media set by the engine's own state, with Optical, Floppy and Tape
//! chips and each set's members and swap plan.
//!
//! Same review as Multi-disc games (`media_sets::MediaSetReview`, the core
//! `media_set` engine over the loaded catalogue); this view only filters it
//! differently. Read-only: no file is opened, no swap is run, no playlist is
//! written.

use archivefs_core::media_set::{MediaFamily, MediaSetState};
use eframe::egui::{self, RichText};

use super::App;
use super::media_sets::SetView;
use super::routes::{Route, Section};
use super::simple_shell::palette;
use crate::ui::components::{self as widgets, ActionStyle, StatusTone};
use crate::ui::theme::{self, blue};

pub(super) const PAGE_SIZE: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SetChip {
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

impl SetChip {
    pub(super) const ALL: [Self; 9] = [
        Self::All,
        Self::Complete,
        Self::Incomplete,
        Self::Ambiguous,
        Self::Conflicting,
        Self::Unverified,
        Self::Optical,
        Self::Floppy,
        Self::Tape,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Complete => "Complete",
            Self::Incomplete => "Incomplete",
            Self::Ambiguous => "Ambiguous",
            Self::Conflicting => "Conflicting",
            Self::Unverified => "Unverified",
            Self::Optical => "Optical",
            Self::Floppy => "Floppy",
            Self::Tape => "Tape",
        }
    }

    pub(super) fn admits(self, set: &SetView) -> bool {
        match self {
            Self::All => true,
            Self::Complete => set.engine_state == MediaSetState::CompleteSet,
            Self::Incomplete => set.engine_state == MediaSetState::IncompleteSet,
            Self::Ambiguous => set.engine_state == MediaSetState::AmbiguousSet,
            Self::Conflicting => set.engine_state == MediaSetState::ConflictingSet,
            Self::Unverified => set.engine_state == MediaSetState::UnverifiedSet,
            Self::Optical => set.family == Some(MediaFamily::Optical),
            Self::Floppy => set.family == Some(MediaFamily::Floppy),
            Self::Tape => set.family == Some(MediaFamily::Tape),
        }
    }
}

#[derive(Default)]
pub(super) struct MediaSetsToolState {
    pub chip: SetChip,
    pub query: String,
    pub page: usize,
    /// The set whose members and swap plan are shown.
    pub open: Option<String>,
}

/// The engine state as a short badge, with its tone.
fn state_badge(state: MediaSetState) -> (&'static str, StatusTone) {
    match state {
        MediaSetState::CompleteSet => ("Complete", StatusTone::Success),
        MediaSetState::IncompleteSet => ("Incomplete", StatusTone::Warning),
        MediaSetState::AmbiguousSet => ("Ambiguous", StatusTone::Warning),
        MediaSetState::ConflictingSet => ("Conflicting", StatusTone::Warning),
        MediaSetState::UnverifiedSet => ("Unverified", StatusTone::Pending),
        MediaSetState::UnsupportedSet => ("Unsupported", StatusTone::Pending),
    }
}

pub(super) fn visible<'a>(sets: &'a [SetView], chip: SetChip, query: &str) -> Vec<&'a SetView> {
    let query = query.trim().to_lowercase();
    sets.iter()
        .filter(|set| chip.admits(set))
        .filter(|set| query.is_empty() || set.search.contains(&query))
        .collect()
}

impl App {
    pub(super) fn media_sets_tool_page(&mut self, ui: &mut egui::Ui) {
        if self.library.games.is_empty() {
            if widgets::empty_state(
                ui,
                "No games in the catalogue yet",
                "Media sets are found among the games EmuWiz has scanned. Add a game folder first.",
                Some("Open Sources"),
            ) {
                self.go(Route::Section(Section::Sources));
            }
            return;
        }
        let stale = self.ensure_media_sets_review();
        let mut go = None;
        let wide = ui.available_width() >= 1100.0;
        let review = self.multi.review.clone();
        let busy = self.multi.job.is_some();
        let failed = self.multi.failed;
        let state = &mut self.tools.media_sets;
        let mut body = |ui: &mut egui::Ui| {
            chips(ui, state);
            ui.add_space(theme::SPACE_SM);
            if failed {
                widgets::banner(
                    ui,
                    "The media-set check could not finish",
                    "Nothing was changed. Open Multi-disc games to retry.",
                    StatusTone::Warning,
                );
            }
            let Some(review) = review.as_ref() else {
                if busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking for media sets… you can keep browsing.");
                    });
                }
                return;
            };
            if stale {
                widgets::banner(
                    ui,
                    "Earlier results",
                    "These come from an earlier library view. Wait for the new check before relying on them.",
                    StatusTone::Warning,
                );
            }
            let shown = visible(&review.sets, state.chip, &state.query);
            if review.sets.is_empty() {
                widgets::empty_state(
                    ui,
                    "No media sets found",
                    "No multi-disc, multi-disk or multi-tape release was found in the catalogue.",
                    None,
                );
                return;
            }
            if shown.is_empty() {
                widgets::empty_state(
                    ui,
                    "No sets match",
                    "Choose another chip or clear the search.",
                    None,
                );
            }
            let pages = shown.len().div_ceil(PAGE_SIZE).max(1);
            state.page = state.page.min(pages - 1);
            for set in shown.iter().skip(state.page * PAGE_SIZE).take(PAGE_SIZE) {
                if let Some(route) = set_row(ui, set, &mut state.open) {
                    go = Some(route);
                }
            }
            ui.horizontal_wrapped(|ui| {
                if pages > 1 {
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
                }
                ui.label(
                    RichText::new(format!(
                        "{} sets · Page {} of {pages}",
                        shown.len(),
                        state.page + 1
                    ))
                    .color(palette::SECONDARY_TEXT),
                );
            });
        };
        let mut open_multi = false;
        let mut side = |ui: &mut egui::Ui| {
            super::game_detail_blue::side_card(ui, "SAME ENGINE AS MULTI-DISC GAMES", |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "Media Sets and the Multi-disc page read the same saved evidence. This view adds Floppy and Tape and the raw state names. Swap plans are for reading only.",
                        )
                        .color(blue::BODY_TEXT),
                    )
                    .wrap(),
                );
                ui.add_space(6.0);
                if widgets::action_button(ui, "Open Multi-disc games", ActionStyle::Secondary, true)
                    .clicked()
                {
                    open_multi = true;
                }
            });
        };
        if wide {
            let gap = 20.0;
            let right = (ui.available_width() * 0.3).clamp(340.0, 480.0);
            let left = ui.available_width() - right - gap;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui_with_layout(
                    egui::vec2(left, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(left);
                        body(ui);
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(right, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(right);
                        side(ui);
                    },
                );
            });
        } else {
            body(ui);
            ui.add_space(theme::SPACE_SM);
            side(ui);
        }
        if open_multi {
            go = Some(Route::Section(Section::MultiDisc));
        }
        if let Some(route) = go {
            self.go(route);
        }
    }
}

fn chips(ui: &mut egui::Ui, state: &mut MediaSetsToolState) {
    let before = (state.chip, state.query.clone());
    ui.horizontal_wrapped(|ui| {
        for chip in SetChip::ALL {
            if ui
                .add(egui::Button::selectable(state.chip == chip, chip.label()))
                .clicked()
            {
                state.chip = chip;
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text("title or path")
                .desired_width(200.0),
        );
    });
    if before != (state.chip, state.query.clone()) {
        state.page = 0;
    }
}

/// One set; returns a navigation when "Open game details" is pressed.
fn set_row(ui: &mut egui::Ui, set: &SetView, open: &mut Option<String>) -> Option<Route> {
    let mut go = None;
    let expanded = open.as_deref() == Some(set.title.as_str());
    ui.push_id(("v2_media_set", &set.title), |ui| {
        egui::Frame::new()
            .fill(blue::CARD_SURFACE_TOP)
            .stroke(egui::Stroke::new(
                1.0_f32,
                if expanded {
                    blue::SELECTED_OUTLINE
                } else {
                    palette::BORDER_SUBTLE
                },
            ))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let button_width = 280.0_f32.min(ui.available_width() * 0.45);
                    let text_width = (ui.available_width() - button_width - 12.0).max(140.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(text_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.label(RichText::new(&set.title).strong().color(blue::PRIMARY_TEXT));
                            let members = set.members.len();
                            let expected = set.present.len() + set.missing.len();
                            let count = if set.missing.is_empty() || expected == 0 {
                                format!("{members} member{}", if members == 1 { "" } else { "s" })
                            } else {
                                format!("{members} of {expected} members")
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} · {count}",
                                    set.platform, set.media
                                ))
                                .size(13.0)
                                .color(palette::SECONDARY_TEXT),
                            );
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if widgets::action_button(
                            ui,
                            if expanded {
                                "Hide details"
                            } else {
                                "Inspect set"
                            },
                            ActionStyle::Secondary,
                            true,
                        )
                        .clicked()
                        {
                            *open = (!expanded).then(|| set.title.clone());
                        }
                        let (label, tone) = state_badge(set.engine_state);
                        widgets::status_badge(ui, label, tone);
                    });
                });
                if !expanded {
                    return;
                }
                ui.separator();
                ui.label(
                    RichText::new("MEMBERS")
                        .strong()
                        .size(12.5)
                        .color(blue::DIM_TEXT),
                );
                for member in &set.members {
                    member_row(ui, &member.label, &member.paths.join(" · "), true);
                }
                for missing in &set.missing {
                    member_row(
                        ui,
                        missing,
                        "MISSING — no path is shown because the expected media was not observed.",
                        false,
                    );
                }
                for conflict in &set.conflicts {
                    ui.label(RichText::new(conflict).color(blue::WARN));
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new("MEDIA SWAP PLAN")
                        .strong()
                        .size(12.5)
                        .color(blue::DIM_TEXT),
                );
                for line in &set.swap {
                    ui.add(
                        egui::Label::new(RichText::new(line).color(blue::BODY_TEXT).size(13.0))
                            .wrap(),
                    );
                }
                ui.label(
                    RichText::new(
                        "Inspection only. EmuWiz will not run swaps or create a playlist here.",
                    )
                    .color(palette::SECONDARY_TEXT),
                );
                ui.collapsing("Why EmuWiz says this", |ui| {
                    for line in &set.why {
                        ui.add(egui::Label::new(RichText::new(line).size(12.5)).wrap());
                    }
                });
                if let Some(id) = set.game_id
                    && ui.button("Open game details").clicked()
                {
                    go = Some(Route::Game(id));
                }
            });
    });
    ui.add_space(6.0);
    go
}

fn member_row(ui: &mut egui::Ui, label: &str, detail: &str, found: bool) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(90.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(90.0);
                ui.label(RichText::new(label).color(blue::DIM_TEXT));
            },
        );
        let badge = 90.0;
        let text = (ui.available_width() - badge - 8.0).max(120.0);
        ui.allocate_ui_with_layout(
            egui::vec2(text, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(text);
                ui.add(
                    egui::Label::new(
                        RichText::new(detail)
                            .monospace()
                            .size(12.5)
                            .color(blue::BODY_TEXT),
                    )
                    .wrap(),
                );
            },
        );
        widgets::status_badge(
            ui,
            if found { "Found" } else { "Missing" },
            if found {
                StatusTone::Success
            } else {
                StatusTone::Warning
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::super::media_sets::{Bucket, MemberView};
    use super::*;

    fn set(title: &str, state: MediaSetState, family: Option<MediaFamily>) -> SetView {
        SetView {
            title: title.into(),
            platform: "PlayStation".into(),
            media: "Optical",
            family,
            engine_state: state,
            state: "",
            bucket: Bucket::CantTell,
            filename_only: false,
            completeness: String::new(),
            present: vec!["Disc 1".into()],
            missing: Vec::new(),
            members: vec![MemberView {
                label: "Disc 1".into(),
                paths: vec![format!("/g/{title} (Disc 1).chd")],
            }],
            conflicts: Vec::new(),
            why: Vec::new(),
            swap: Vec::new(),
            game_id: None,
            search: title.to_lowercase(),
        }
    }

    #[test]
    fn chips_filter_by_the_engine_state_and_media_family() {
        let sets = vec![
            set("A", MediaSetState::CompleteSet, Some(MediaFamily::Optical)),
            set("B", MediaSetState::IncompleteSet, Some(MediaFamily::Floppy)),
            set("C", MediaSetState::UnverifiedSet, Some(MediaFamily::Tape)),
            set(
                "D",
                MediaSetState::ConflictingSet,
                Some(MediaFamily::Optical),
            ),
            set("E", MediaSetState::AmbiguousSet, None),
        ];
        let titles = |chip| -> Vec<String> {
            visible(&sets, chip, "")
                .into_iter()
                .map(|set| set.title.clone())
                .collect()
        };
        assert_eq!(titles(SetChip::All).len(), 5);
        assert_eq!(titles(SetChip::Complete), ["A"]);
        assert_eq!(titles(SetChip::Incomplete), ["B"]);
        assert_eq!(titles(SetChip::Unverified), ["C"]);
        assert_eq!(titles(SetChip::Conflicting), ["D"]);
        assert_eq!(titles(SetChip::Ambiguous), ["E"]);
        assert_eq!(titles(SetChip::Optical), ["A", "D"]);
        assert_eq!(titles(SetChip::Floppy), ["B"]);
        assert_eq!(titles(SetChip::Tape), ["C"]);
        assert_eq!(visible(&sets, SetChip::All, " b ").len(), 1);
    }

    #[test]
    fn an_inspected_set_shows_members_missing_media_and_the_read_only_plan() {
        let mut incomplete = set(
            "Glacier Run",
            MediaSetState::IncompleteSet,
            Some(MediaFamily::Optical),
        );
        incomplete.missing = vec!["Disc 2".into()];
        incomplete.swap = vec!["Start with: Disc 1 — /g/Glacier Run (Disc 1).chd".into()];
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        for width in [520.0, 1400.0] {
            let mut open = Some("Glacier Run".to_string());
            let mut texts = Vec::new();
            let mut used = 0.0_f32;
            let mut output = None;
            for _ in 0..3 {
                output = Some(context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 900.0),
                        )),
                        ..Default::default()
                    },
                    |context| {
                        egui::CentralPanel::default().show(context, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                texts.clear();
                                assert_eq!(set_row(ui, &incomplete, &mut open), None);
                                used = ui.min_rect().right();
                            });
                        });
                    },
                ));
            }
            let output = output.unwrap();
            fn gather(shape: &egui::Shape, out: &mut Vec<String>) {
                match shape {
                    egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| gather(shape, out)),
                    _ => {}
                }
            }
            for shape in &output.shapes {
                gather(&shape.shape, &mut texts);
            }
            for wanted in [
                "MEMBERS",
                "Found",
                "Missing",
                "MEDIA SWAP PLAN",
                "Inspection only. EmuWiz will not run swaps or create a playlist here.",
                "Incomplete",
                "Hide details",
            ] {
                assert!(
                    texts.iter().any(|text| text.contains(wanted)),
                    "{wanted} missing at {width}: {texts:?}"
                );
            }
            assert!(used <= width, "at {width} the set used {used}");
        }
    }

    #[test]
    fn every_engine_state_has_a_badge() {
        for state in [
            MediaSetState::CompleteSet,
            MediaSetState::IncompleteSet,
            MediaSetState::AmbiguousSet,
            MediaSetState::ConflictingSet,
            MediaSetState::UnverifiedSet,
            MediaSetState::UnsupportedSet,
        ] {
            let (label, _) = state_badge(state);
            assert!(!label.is_empty() && !label.ends_with(" set"));
        }
    }
}
