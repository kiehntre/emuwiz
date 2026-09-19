//! Task-oriented shell navigation. Existing views remain the dispatch authority;
//! these groups and subviews only decide how users reach them.

use super::*;
use crate::cheats_mods_preview::{EnhancementSection, current_enhancement_section};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    Home,
    Library,
    Setup,
    Organise,
    Enhance,
    Health,
    Settings,
}

pub(crate) const PRIMARY: [(Destination, &str, NavClick); 7] = [
    (Destination::Home, "Home", NavClick::View(MainView::Home)),
    (
        Destination::Library,
        "Library",
        NavClick::View(MainView::Library),
    ),
    (
        Destination::Setup,
        "Setup",
        NavClick::View(MainView::Sources),
    ),
    (
        Destination::Organise,
        "Organise & Export",
        NavClick::View(MainView::CanonicalOrganisation),
    ),
    (
        Destination::Enhance,
        "Enhance",
        NavClick::Enhancement(EnhancementSection::Cheats),
    ),
    (
        Destination::Health,
        "Health & Recovery",
        NavClick::View(MainView::Problems),
    ),
    (
        Destination::Settings,
        "Settings",
        NavClick::View(MainView::Settings),
    ),
];

/// Exhaustive mapping also covers old deep links and menu routes. Overlays own
/// the highlight while open; closing them returns to the underlying view.
pub(crate) fn destination(view: MainView, overlay: ToolsOverlay) -> Destination {
    match overlay {
        ToolsOverlay::SaveVault => return Destination::Enhance,
        ToolsOverlay::Diagnostics | ToolsOverlay::DoctorChecks => return Destination::Health,
        ToolsOverlay::PlatformAliases | ToolsOverlay::DatabaseStatus => {
            return Destination::Settings;
        }
        ToolsOverlay::Onboarding => return Destination::Setup,
        ToolsOverlay::None | ToolsOverlay::ArchiveInspector => {}
    }
    match view {
        MainView::Home => Destination::Home,
        MainView::Library
        | MainView::CheckGames
        | MainView::ReadyToPlay
        | MainView::RecentlyFound
        | MainView::Health
        | MainView::Duplicates
        | MainView::LibraryViews
        | MainView::Selected
        | MainView::DatSources
        | MainView::MediaSets
        | MainView::Museum => Destination::Library,
        MainView::Sources
        | MainView::SourcesDiscovery
        | MainView::EmulatorSetup
        | MainView::EmulatorInventory
        | MainView::BiosProjection => Destination::Setup,
        MainView::CanonicalOrganisation
        | MainView::PublisherProfiles
        | MainView::IdentifyRename
        | MainView::ExactDuplicateReview
        | MainView::DiscConversion => Destination::Organise,
        MainView::CheatsMods | MainView::CheatSources => Destination::Enhance,
        MainView::NeedsAttention
        | MainView::Problems
        | MainView::Doctor
        | MainView::RepairReview
        | MainView::RepairHistory
        | MainView::HistoryLogs
        | MainView::LibraryViewHistory
        | MainView::Mount
        | MainView::ActiveMounts
        | MainView::StorageHealth
        | MainView::TapeInspector => Destination::Health,
        MainView::Settings | MainView::About => Destination::Settings,
    }
}

pub(crate) fn entries(group: Destination) -> &'static [NavEntry] {
    match group {
        Destination::Home => const { &[] },
        Destination::Library => {
            const {
                &[
                    nav_view(MainView::Library, "My Games"),
                    nav_view(MainView::DatSources, "DATs & Verification"),
                    nav_view(MainView::ReadyToPlay, "Ready to Play"),
                ]
            }
        }
        Destination::Setup => {
            const {
                &[
                    nav_view(MainView::Sources, "Game Folders"),
                    nav_view(MainView::EmulatorSetup, "Emulators"),
                    nav_view(MainView::BiosProjection, "BIOS / Firmware"),
                ]
            }
        }
        Destination::Organise => {
            const {
                &[
                    nav_quick_rename("Clean & Rename"),
                    nav_view(MainView::CanonicalOrganisation, "Build Libraries"),
                    nav_view(MainView::ExactDuplicateReview, "Duplicates"),
                    nav_view(MainView::DiscConversion, "Converter"),
                ]
            }
        }
        Destination::Enhance => {
            const {
                &[
                    NavEntry {
                        click: NavClick::Enhancement(EnhancementSection::Cheats),
                        label: "Cheats",
                        highlightable: true,
                    },
                    NavEntry {
                        click: NavClick::Enhancement(EnhancementSection::Mods),
                        label: "Mods & ROM Hacks",
                        highlightable: true,
                    },
                    nav_overlay(ToolsOverlay::SaveVault, "Saves"),
                ]
            }
        }
        Destination::Health => {
            const {
                &[
                    nav_view(MainView::Problems, "Problems"),
                    nav_view(MainView::Doctor, "Advanced Diagnostics"),
                ]
            }
        }
        Destination::Settings => const { &[] },
    }
}

pub(crate) fn subviews(view: MainView, overlay: ToolsOverlay) -> &'static [NavEntry] {
    match (view, overlay) {
        (_, ToolsOverlay::SaveVault) => const { &[] },
        (MainView::EmulatorSetup | MainView::EmulatorInventory, ToolsOverlay::None) => {
            const {
                &[
                    nav_view(MainView::EmulatorSetup, "Setup & Readiness"),
                    nav_view(MainView::EmulatorInventory, "Installed Emulators & Updates"),
                ]
            }
        }
        (MainView::CanonicalOrganisation | MainView::PublisherProfiles, ToolsOverlay::None) => {
            const {
                &[
                    nav_view(MainView::CanonicalOrganisation, "Plan Libraries"),
                    nav_view(MainView::PublisherProfiles, "Export"),
                ]
            }
        }
        (MainView::Sources | MainView::SourcesDiscovery, ToolsOverlay::None) => {
            const {
                &[
                    nav_view(MainView::Sources, "Game Folders"),
                    nav_romm("RomM Connection & Browsing"),
                    nav_view(MainView::SourcesDiscovery, "Discovery"),
                ]
            }
        }
        (MainView::CheatsMods | MainView::CheatSources, ToolsOverlay::None) => {
            const { &[nav_view(MainView::CheatSources, "Cheat Sources")] }
        }
        (MainView::Problems | MainView::NeedsAttention, ToolsOverlay::None) => {
            const {
                &[
                    nav_view(MainView::Problems, "Problems & Repair"),
                    nav_view(MainView::NeedsAttention, "Needs Attention"),
                ]
            }
        }
        _ if destination(view, overlay) == Destination::Health => {
            const {
                &[
                    nav_view(MainView::Doctor, "Doctor"),
                    nav_overlay(ToolsOverlay::DoctorChecks, "Automatic Health Report"),
                    nav_overlay(ToolsOverlay::Diagnostics, "Configuration Diagnostics"),
                    nav_view(MainView::RepairReview, "Repair & Recovery"),
                ]
            }
        }
        _ => const { &[] },
    }
}

/// Less frequent tools retain their original routes. The legacy route registry
/// supplies their labels; it is no longer rendered as a primary sidebar.
pub(crate) fn advanced_entries(group: Destination) -> Vec<NavEntry> {
    let mut entries: Vec<_> = ADVANCED_NAV_GROUPS
        .iter()
        .filter(|group| {
            matches!(
                group.heading,
                Some("MOUNTS" | "MEDIA" | "HISTORY & JOURNALS")
            )
        })
        .flat_map(|group| group.entries)
        .copied()
        .filter(|entry| match entry.click {
            NavClick::View(MainView::MediaSets) => group == Destination::Library,
            NavClick::View(
                MainView::Mount
                | MainView::ActiveMounts
                | MainView::HistoryLogs
                | MainView::LibraryViewHistory,
            ) => group == Destination::Health,
            _ => false,
        })
        .collect();
    match group {
        Destination::Library => entries.push(nav_view(MainView::Museum, "Museum")),
        Destination::Health => entries.extend([
            nav_view(MainView::StorageHealth, "Storage Health"),
            nav_view(MainView::TapeInspector, "Tape Inspector"),
            nav_view(MainView::RepairHistory, "Repair History"),
        ]),
        Destination::Settings => entries.extend([
            nav_overlay(ToolsOverlay::PlatformAliases, "Platform Aliases"),
            nav_overlay(ToolsOverlay::DatabaseStatus, "Database Status"),
            nav_view(MainView::About, "About EmuWiz"),
        ]),
        _ => {}
    }
    entries
}

fn selected(
    entry: NavEntry,
    view: MainView,
    overlay: ToolsOverlay,
    section: EnhancementSection,
    parent: bool,
) -> bool {
    if !entry.highlightable {
        return false;
    }
    match entry.click {
        NavClick::Overlay(target) => overlay == target,
        NavClick::Enhancement(target) => {
            overlay == ToolsOverlay::None && view == MainView::CheatsMods && section == target
        }
        NavClick::QuickRename => overlay == ToolsOverlay::None && view == MainView::IdentifyRename,
        NavClick::Romm => false,
        NavClick::View(target) => {
            if !parent {
                return overlay == ToolsOverlay::None && target == view;
            }
            match target {
                MainView::Library => {
                    overlay == ToolsOverlay::None
                        && (library_tab_for_main_view(view).is_some()
                            || matches!(
                                view,
                                MainView::Selected | MainView::MediaSets | MainView::Museum
                            ))
                }
                MainView::Sources => {
                    overlay == ToolsOverlay::None
                        && matches!(view, MainView::Sources | MainView::SourcesDiscovery)
                }
                MainView::EmulatorSetup => {
                    overlay == ToolsOverlay::None
                        && matches!(view, MainView::EmulatorSetup | MainView::EmulatorInventory)
                }
                MainView::CanonicalOrganisation => {
                    overlay == ToolsOverlay::None
                        && matches!(
                            view,
                            MainView::CanonicalOrganisation | MainView::PublisherProfiles
                        )
                }
                MainView::Problems => {
                    overlay == ToolsOverlay::None
                        && matches!(view, MainView::Problems | MainView::NeedsAttention)
                }
                MainView::Doctor => {
                    destination(view, overlay) == Destination::Health
                        && !(overlay == ToolsOverlay::None
                            && matches!(view, MainView::Problems | MainView::NeedsAttention))
                }
                _ => overlay == ToolsOverlay::None && navigation_destination_selected(view, target),
            }
        }
    }
}

pub(crate) fn show_sidebar(
    ui: &mut egui::Ui,
    view: MainView,
    overlay: ToolsOverlay,
) -> Option<NavClick> {
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("primary_navigation_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("EmuWiz").size(23.0).strong());
            ui.label(egui::RichText::new("Game library manager").color(theme::muted(ui)));
            ui.add_space(18.0);
            let current = destination(view, overlay);
            for (group, label, target) in PRIMARY {
                if ui
                    .add(
                        egui::Button::selectable(current == group, label)
                            .min_size(egui::vec2(ui.available_width(), 34.0)),
                    )
                    .clicked()
                {
                    clicked = Some(target);
                }
            }
        });
    clicked
}

/// Only shell chrome, outside each unchanged page. Returns an intent and never
/// performs scans, persistence, downloads, repairs, or writes to game files.
pub(crate) fn show_subnavigation(
    ui: &mut egui::Ui,
    view: MainView,
    overlay: ToolsOverlay,
    has_database: bool,
) -> Option<NavClick> {
    let group = destination(view, overlay);
    let section = current_enhancement_section(ui.ctx());
    let mut clicked = None;
    let buttons = |ui: &mut egui::Ui, items: &[NavEntry], parent: bool| {
        let mut request = None;
        for entry in items {
            let enabled = match entry.click {
                NavClick::View(v) => navigation_destination_enabled(v, has_database),
                _ => true,
            };
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::selectable(
                        selected(*entry, view, overlay, section, parent),
                        entry.label,
                    ),
                )
                .clicked()
            {
                request = Some(entry.click);
            }
        }
        request
    };
    let advanced = advanced_entries(group);
    if !entries(group).is_empty() || !advanced.is_empty() {
        ui.horizontal_wrapped(|ui| {
            clicked = buttons(ui, entries(group), true);
            if !advanced.is_empty() {
                ui.menu_button("More tools", |ui| {
                    for entry in advanced {
                        if ui.button(entry.label).clicked() {
                            clicked = Some(entry.click);
                            ui.close();
                        }
                    }
                });
            }
        });
    }
    let children = subviews(view, overlay);
    if !children.is_empty() {
        ui.horizontal_wrapped(|ui| {
            if let Some(request) = buttons(ui, children, false) {
                clicked = Some(request);
            }
        });
    }
    clicked
}

#[cfg(test)]
mod tests;
