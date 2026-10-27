//! EmuWiz Blue "All Tools" browser (design boards `D5-Tools-*`).
//!
//! Every existing location, grouped by what the person wants to get done,
//! with a search box and a safety filter. Presentation only: each entry opens
//! a route that already exists; nothing here dispatches a backend command.
//! It replaces the earlier swap to the Classic sidebar inside the Blue view;
//! the Classic view itself stays available from Settings.

use eframe::egui;

use super::routes::{Route, SECTIONS, Section, ToolPage};
use super::simple_shell::palette;
use crate::ui::components::{SafetyTag, safety_tag};

/// What opening an entry can lead to. The browser's filter chips use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolSafety {
    /// Opening it and using it only reads: it never changes game files.
    LookingOnly,
    /// It can change files, always after a preview and a confirmation.
    AsksFirst,
    /// It opens in a separate (specialist) window.
    OpensElsewhere,
}

impl ToolSafety {
    pub(super) fn tag(self) -> Option<SafetyTag> {
        match self {
            Self::LookingOnly => Some(SafetyTag::ReadOnly),
            Self::AsksFirst => Some(SafetyTag::AsksFirst),
            Self::OpensElsewhere => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ToolFilter {
    #[default]
    All,
    LookingOnly,
    AsksFirst,
    OpensElsewhere,
}

impl ToolFilter {
    const ALL: [Self; 4] = [
        Self::All,
        Self::LookingOnly,
        Self::AsksFirst,
        Self::OpensElsewhere,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::LookingOnly => "Looking only",
            Self::AsksFirst => "Asks first",
            Self::OpensElsewhere => "Opens elsewhere",
        }
    }

    fn admits(self, safety: ToolSafety) -> bool {
        match self {
            Self::All => true,
            Self::LookingOnly => safety == ToolSafety::LookingOnly,
            Self::AsksFirst => safety == ToolSafety::AsksFirst,
            Self::OpensElsewhere => safety == ToolSafety::OpensElsewhere,
        }
    }
}

/// Per-session browser state; never persisted.
#[derive(Clone, Debug, Default)]
pub(super) struct AllToolsState {
    pub query: String,
    pub filter: ToolFilter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Tool {
    pub route: Route,
    pub title: &'static str,
    pub purpose: &'static str,
    pub safety: ToolSafety,
}

pub(super) struct ToolGroup {
    pub title: &'static str,
    pub tools: Vec<Tool>,
}

/// How a section is classified. Conservative: only pages whose purpose is
/// reading are "Looking only"; anything that can change a file asks first.
pub(super) fn section_safety(section: Section) -> ToolSafety {
    match section {
        Section::Advanced => ToolSafety::OpensElsewhere,
        Section::Games
        | Section::Platforms
        | Section::Museum
        | Section::Check
        | Section::Storage
        | Section::Tape
        | Section::Activity
        | Section::Launch
        | Section::Home => ToolSafety::LookingOnly,
        _ => ToolSafety::AsksFirst,
    }
}

fn section_tool(section: Section) -> Tool {
    Tool {
        route: Route::Section(section),
        title: section.title(),
        purpose: section.purpose(),
        safety: section_safety(section),
    }
}

/// The groups, in reading order. Every section except Home (always on the
/// sidebar) appears exactly once, plus the two non-section locations.
pub(super) fn groups() -> Vec<ToolGroup> {
    use Section as S;
    let group = |title: &'static str, sections: &[Section]| ToolGroup {
        title,
        tools: sections.iter().copied().map(section_tool).collect(),
    };
    let mut games = group(
        "Games & collection",
        &[
            S::Games,
            S::Platforms,
            S::Sources,
            S::Launch,
            S::Museum,
            S::Romm,
            S::MultiDisc,
        ],
    );
    games.tools.insert(
        0,
        Tool {
            route: Route::BrowsePlay,
            title: "Browse & Play",
            purpose: "Browse your games. Select one to see what you can do next.",
            safety: ToolSafety::LookingOnly,
        },
    );
    let mut verification = group(
        "Verification & identification",
        &[S::Check, S::DatVerification, S::Dat],
    );
    verification.tools.push(Tool {
        route: Route::QuickRename,
        title: "Quick Rename",
        purpose: "Preview safe rename decisions from verified evidence.",
        safety: ToolSafety::AsksFirst,
    });
    let mut health = group(
        "Health & recovery",
        &[
            S::ProblemsRepair,
            S::Problems,
            S::HistoryUndo,
            S::History,
            S::Activity,
        ],
    );
    health.tools.insert(
        2,
        Tool {
            route: Route::Tool(ToolPage::NeedsAttention),
            title: ToolPage::NeedsAttention.title(),
            purpose: ToolPage::NeedsAttention.purpose(),
            safety: ToolSafety::LookingOnly,
        },
    );
    games.tools.push(Tool {
        route: Route::Tool(ToolPage::MediaSets),
        title: ToolPage::MediaSets.title(),
        purpose: ToolPage::MediaSets.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    games.tools.push(Tool {
        route: Route::Tool(ToolPage::NetplayPreview),
        title: ToolPage::NetplayPreview.title(),
        purpose: ToolPage::NetplayPreview.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    games.tools.push(Tool {
        route: Route::Tool(ToolPage::SourceImpact),
        title: ToolPage::SourceImpact.title(),
        purpose: ToolPage::SourceImpact.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    let mut storage = group("Storage & duplicates", &[S::Storage, S::Duplicates]);
    games.tools.push(Tool {
        route: Route::Tool(ToolPage::ProfileSettings),
        title: ToolPage::ProfileSettings.title(),
        purpose: ToolPage::ProfileSettings.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    storage.tools.push(Tool {
        route: Route::Tool(ToolPage::OpticalCompare),
        title: ToolPage::OpticalCompare.title(),
        purpose: ToolPage::OpticalCompare.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    let mut cheats = group("Cheats, mods & patches", &[S::CheatsMods, S::Mods]);
    cheats.tools.push(Tool {
        route: Route::Tool(ToolPage::WhdloadOptions),
        title: ToolPage::WhdloadOptions.title(),
        purpose: ToolPage::WhdloadOptions.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    cheats.tools.push(Tool {
        route: Route::Tool(ToolPage::PatchPackagePreview),
        title: ToolPage::PatchPackagePreview.title(),
        purpose: ToolPage::PatchPackagePreview.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    cheats.tools.push(Tool {
        route: Route::Tool(ToolPage::CheatExtras),
        title: ToolPage::CheatExtras.title(),
        purpose: ToolPage::CheatExtras.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    let mut advanced = group(
        "Advanced diagnostics",
        &[S::AdvancedDiagnostics, S::Settings, S::Advanced],
    );
    advanced.tools.insert(
        2,
        Tool {
            route: Route::Tool(ToolPage::PlatformAliases),
            title: ToolPage::PlatformAliases.title(),
            purpose: ToolPage::PlatformAliases.purpose(),
            safety: ToolSafety::AsksFirst,
        },
    );
    advanced.tools.insert(
        3,
        Tool {
            route: Route::Tool(ToolPage::DatabaseStatus),
            title: ToolPage::DatabaseStatus.title(),
            purpose: ToolPage::DatabaseStatus.purpose(),
            safety: ToolSafety::AsksFirst,
        },
    );
    let mut organise = group(
        "Organise & export",
        &[S::OrganisationFamily, S::Build, S::Mame],
    );
    organise.tools.push(Tool {
        route: Route::Tool(ToolPage::LibraryViews),
        title: ToolPage::LibraryViews.title(),
        purpose: ToolPage::LibraryViews.purpose(),
        safety: ToolSafety::AsksFirst,
    });
    organise.tools.push(Tool {
        route: Route::Tool(ToolPage::ViewHistory),
        title: ToolPage::ViewHistory.title(),
        purpose: ToolPage::ViewHistory.purpose(),
        safety: ToolSafety::LookingOnly,
    });
    storage.tools.push(Tool {
        route: Route::Tool(ToolPage::Mounts),
        title: ToolPage::Mounts.title(),
        purpose: ToolPage::Mounts.purpose(),
        safety: ToolSafety::AsksFirst,
    });
    vec![
        games,
        verification,
        group(
            "Artwork & metadata",
            &[S::ArtworkExtras, S::Artwork, S::SourcesProviders],
        ),
        cheats,
        group("Media conversion", &[S::Conversion, S::Converter, S::Tape]),
        storage,
        group("Saves", &[S::SavesStates, S::Saves]),
        organise,
        group(
            "Emulator utilities",
            &[S::Setup, S::EmulatorsFamily, S::Emulators, S::Firmware],
        ),
        health,
        advanced,
    ]
}

/// Whether `tool` (in `group`) is shown for this search and filter.
pub(super) fn matches(state: &AllToolsState, group: &str, tool: &Tool) -> bool {
    if !state.filter.admits(tool.safety) {
        return false;
    }
    let query = state.query.trim().to_lowercase();
    query.is_empty()
        || query.split_whitespace().all(|word| {
            tool.title.to_lowercase().contains(word)
                || tool.purpose.to_lowercase().contains(word)
                || group.to_lowercase().contains(word)
        })
}

/// The stable identity of an entry's button (focus, tests).
pub(super) fn tool_id(route: &Route) -> egui::Id {
    egui::Id::new(("v2_all_tools_entry", format!("{route:?}")))
}

pub(super) const TITLE: &str = "All Tools";
pub(super) const PURPOSE: &str = "Everything EmuWiz can do, grouped by what you want to get done.";
const CARD_MIN_WIDTH: f32 = 360.0;
const CARD_GAP: f32 = 14.0;

/// How many group cards fit across `available` points.
pub(super) fn columns(available: f32) -> usize {
    (((available + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor() as usize).clamp(1, 3)
}

/// Paints the browser and returns the route the person chose this frame.
pub(super) fn show(ui: &mut egui::Ui, state: &mut AllToolsState) -> Option<Route> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Show").color(palette::SECONDARY_TEXT));
        for filter in ToolFilter::ALL {
            if ui
                .selectable_label(state.filter == filter, filter.label())
                .clicked()
            {
                state.filter = filter;
            }
        }
    });
    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::singleline(&mut state.query)
            .id(egui::Id::new("v2_all_tools_search"))
            .hint_text("Find a tool, for example \u{201c}convert\u{201d}, \u{201c}duplicates\u{201d} or \u{201c}cheats\u{201d}")
            .desired_width(ui.available_width().min(620.0)),
    );
    ui.add_space(12.0);
    let visible: Vec<(&'static str, Vec<Tool>)> = groups()
        .into_iter()
        .filter_map(|group| {
            let tools: Vec<Tool> = group
                .tools
                .into_iter()
                .filter(|tool| matches(state, group.title, tool))
                .collect();
            (!tools.is_empty()).then_some((group.title, tools))
        })
        .collect();
    if visible.is_empty() {
        ui.label(egui::RichText::new("No tool matches that.").strong());
        ui.label(
            egui::RichText::new("Try another word, or show All. Every tool is still in its group.")
                .color(palette::SECONDARY_TEXT),
        );
        if ui.button("Clear search").clicked() {
            state.query.clear();
            state.filter = ToolFilter::All;
        }
        return None;
    }
    let columns = columns(ui.available_width());
    let width = (ui.available_width() - CARD_GAP * (columns as f32 - 1.0)) / columns as f32;
    for row in visible.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for (title, tools) in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(width);
                        if let Some(route) = group_card(ui, title, tools) {
                            chosen = Some(route);
                        }
                    },
                );
            }
        });
        ui.add_space(CARD_GAP);
    }
    chosen
}

fn group_card(ui: &mut egui::Ui, title: &str, tools: &[Tool]) -> Option<Route> {
    let mut chosen = None;
    egui::Frame::new()
        .fill(palette::CARD_SURFACE)
        .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(crate::ui::theme::card_title(title));
                ui.label(
                    egui::RichText::new(tools.len().to_string())
                        .size(13.0)
                        .color(palette::SECONDARY_TEXT),
                );
            });
            ui.add_space(4.0);
            for tool in tools {
                if tool_row(ui, tool).clicked() {
                    chosen = Some(tool.route.clone());
                }
            }
        });
    chosen
}

/// One entry: title, purpose and safety tag. A real focusable button; Enter
/// or a click opens the existing location.
fn tool_row(ui: &mut egui::Ui, tool: &Tool) -> egui::Response {
    let width = ui.available_width();
    let inner = ui
        .scope(|ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let tag_width = 150.0;
                ui.vertical(|ui| {
                    ui.set_width((width - tag_width).max(120.0));
                    ui.label(egui::RichText::new(tool.title).strong());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(tool.purpose)
                                .size(13.5)
                                .color(palette::SECONDARY_TEXT),
                        )
                        .wrap(),
                    );
                });
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match tool.safety.tag() {
                        Some(tag) => {
                            safety_tag(ui, tag);
                        }
                        None => {
                            ui.label(
                                egui::RichText::new("Opens a separate window")
                                    .size(12.5)
                                    .color(palette::SECONDARY_TEXT),
                            );
                        }
                    },
                );
            });
        })
        .response;
    let rect = inner.rect.expand2(egui::vec2(0.0, 3.0));
    let response = ui.interact(rect, tool_id(&tool.route), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tool.title)
    });
    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0_f32, palette::BORDER_STRONG),
            egui::StrokeKind::Inside,
        );
    }
    crate::ui::theme::paint_focus_ring(ui, &response, rect, 8.0);
    ui.add_space(4.0);
    response.on_hover_text(format!("Open {}", tool.title))
}

/// Every section the browser does not list. Only Home, which the sidebar
/// always shows.
pub(super) fn unlisted_sections() -> Vec<Section> {
    let listed: Vec<Route> = groups()
        .into_iter()
        .flat_map(|group| group.tools.into_iter().map(|tool| tool.route))
        .collect();
    SECTIONS
        .iter()
        .copied()
        .filter(|section| !listed.contains(&Route::Section(*section)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_native_tool_is_registered_exactly_once() {
        let listed: Vec<_> = groups()
            .into_iter()
            .flat_map(|group| group.tools.into_iter())
            .filter_map(|tool| match tool.route {
                Route::Tool(page) => Some(page),
                _ => None,
            })
            .collect();
        assert_eq!(listed.len(), ToolPage::ALL.len());
        for page in ToolPage::ALL {
            assert_eq!(
                listed.iter().filter(|&&listed| listed == page).count(),
                1,
                "{page:?} must remain reachable exactly once in All Tools"
            );
        }
    }

    #[test]
    fn every_section_but_home_is_listed_exactly_once() {
        assert_eq!(unlisted_sections(), vec![Section::Home]);
        let mut routes: Vec<String> = groups()
            .into_iter()
            .flat_map(|group| {
                group
                    .tools
                    .into_iter()
                    .map(|tool| format!("{:?}", tool.route))
            })
            .collect();
        let total = routes.len();
        routes.sort();
        routes.dedup();
        assert_eq!(routes.len(), total, "a location is listed twice");
        // Browse & Play and Quick Rename are locations without a section.
        assert!(routes.contains(&format!("{:?}", Route::BrowsePlay)));
        assert!(routes.contains(&format!("{:?}", Route::QuickRename)));
    }

    #[test]
    fn search_matches_titles_purposes_and_groups_and_filters_by_safety() {
        let mut state = AllToolsState::default();
        let all: usize = groups().iter().map(|group| group.tools.len()).sum();
        let shown = |state: &AllToolsState| -> usize {
            groups()
                .iter()
                .map(|group| {
                    group
                        .tools
                        .iter()
                        .filter(|tool| matches(state, group.title, tool))
                        .count()
                })
                .sum()
        };
        assert_eq!(shown(&state), all);
        state.query = "duplicates".into();
        assert!(shown(&state) >= 1 && shown(&state) < all);
        state.query = "zzzz-no-such-tool".into();
        assert_eq!(shown(&state), 0);
        state.query.clear();
        state.filter = ToolFilter::OpensElsewhere;
        assert_eq!(shown(&state), 1);
        state.filter = ToolFilter::LookingOnly;
        let looking = shown(&state);
        state.filter = ToolFilter::AsksFirst;
        assert_eq!(looking + shown(&state) + 1, all);
    }

    #[test]
    fn pages_that_change_files_are_never_called_looking_only() {
        for section in [
            Section::Build,
            Section::Duplicates,
            Section::Converter,
            Section::Conversion,
            Section::Mods,
            Section::CheatsMods,
            Section::Saves,
            Section::SavesStates,
            Section::Problems,
            Section::Dat,
            Section::Mame,
            Section::History,
            Section::Emulators,
            Section::Sources,
            Section::MultiDisc,
        ] {
            assert_eq!(
                section_safety(section),
                ToolSafety::AsksFirst,
                "{section:?}"
            );
        }
    }

    #[test]
    fn group_cards_reflow_from_three_across_to_one() {
        assert_eq!(columns(1600.0), 3);
        assert_eq!(columns(1000.0), 2);
        assert_eq!(columns(500.0), 1);
        assert_eq!(columns(100.0), 1);
    }
}
