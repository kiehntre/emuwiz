//! Stable task locations, independent of legacy tabs and presentation modes.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) enum Section {
    #[default]
    Home,
    Setup,
    Games,
    Saves,
    Duplicates,
    MultiDisc,
    Storage,
    Platforms,
    Check,
    Problems,
    Build,
    Converter,
    Tape,
    Museum,
    Launch,
    Emulators,
    Firmware,
    Mods,
    Artwork,
    Sources,
    Romm,
    Dat,
    Activity,
    History,
    Settings,
    Advanced,
    DatVerification,
    CheatsMods,
    SavesStates,
    EmulatorsFamily,
    Mame,
    ArtworkExtras,
    Conversion,
    OrganisationFamily,
    ProblemsRepair,
    SourcesProviders,
    HistoryUndo,
    AdvancedDiagnostics,
}

pub(super) const SECTIONS: &[Section] = &[
    Section::Home,
    Section::Setup,
    Section::Games,
    Section::Saves,
    Section::Duplicates,
    Section::MultiDisc,
    Section::Storage,
    Section::Platforms,
    Section::Check,
    Section::Problems,
    Section::Build,
    Section::Converter,
    Section::Tape,
    Section::Museum,
    Section::Launch,
    Section::Emulators,
    Section::Firmware,
    Section::Mods,
    Section::Artwork,
    Section::Sources,
    Section::Romm,
    Section::Dat,
    Section::Activity,
    Section::History,
    Section::Settings,
    Section::Advanced,
    Section::DatVerification,
    Section::CheatsMods,
    Section::SavesStates,
    Section::EmulatorsFamily,
    Section::Mame,
    Section::ArtworkExtras,
    Section::Conversion,
    Section::OrganisationFamily,
    Section::ProblemsRepair,
    Section::SourcesProviders,
    Section::HistoryUndo,
    Section::AdvancedDiagnostics,
];

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Setup => "Setup & Doctor",
            Self::Games => "Games",
            Self::Saves => "Saves & States",
            Self::Duplicates => "Duplicates",
            Self::MultiDisc => "Multi-disc games",
            Self::Storage => "Storage",
            Self::Platforms => "Platforms",
            Self::Check => "Check Games",
            Self::Problems => "Problems & Repair",
            Self::Build => "Organisation",
            Self::Converter => "Converter",
            Self::Tape => "Tape Inspector",
            Self::Museum => "Museum",
            Self::Launch => "Launch",
            Self::Emulators => "Emulator Setup",
            Self::Firmware => "BIOS / Firmware",
            Self::Mods => "Mods & Cheats",
            Self::Artwork => "Artwork, Manuals & Extras",
            Self::Sources => "Sources",
            Self::Romm => "RomM Library",
            Self::Dat => "DAT Management",
            Self::Activity => "Activity",
            Self::History => "History",
            Self::Settings => "Settings",
            Self::Advanced => "Advanced",
            Self::DatVerification => "DATs & Verification",
            Self::CheatsMods => "Cheats & Mods",
            Self::SavesStates => "Saves & States",
            Self::EmulatorsFamily => "Emulators",
            Self::Mame => "MAME",
            Self::ArtworkExtras => "Artwork & Extras",
            Self::Conversion => "Conversion",
            Self::OrganisationFamily => "Organisation",
            Self::ProblemsRepair => "Problems & Repair",
            Self::SourcesProviders => "Sources & Providers",
            Self::HistoryUndo => "History & Undo",
            Self::AdvancedDiagnostics => "Advanced / Diagnostics",
        }
    }

    /// The sidebar label. The twelve feature-family overviews share their title
    /// with a task page of the same name (Saves & States, Organisation, Problems &
    /// Repair), so they read "… overview" to stay distinguishable at a glance.
    pub fn sidebar_title(self) -> &'static str {
        match self {
            Self::DatVerification => "DATs & Verification overview",
            Self::CheatsMods => "Cheats & Mods overview",
            Self::SavesStates => "Saves & States overview",
            Self::EmulatorsFamily => "Emulators overview",
            Self::Mame => "MAME overview",
            Self::ArtworkExtras => "Artwork & Extras overview",
            Self::Conversion => "Conversion overview",
            Self::OrganisationFamily => "Organisation overview",
            Self::ProblemsRepair => "Problems & Repair overview",
            Self::SourcesProviders => "Sources & Providers overview",
            Self::HistoryUndo => "History & Undo overview",
            Self::AdvancedDiagnostics => "Advanced / Diagnostics overview",
            other => other.title(),
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::Home => "Your games, and the things you can do with them.",
            Self::Setup => "Understand what EmuWiz needs and why a game may not be ready.",
            Self::Games => "Browse your games. Select one to see what you can do next.",
            Self::Saves => "See which saves are portable, emulator-bound or need careful handling.",
            Self::Duplicates => {
                "Review exact copies without silently collapsing different releases."
            }
            Self::MultiDisc => {
                "Check that every disc of your multi-disc games is present and in order."
            }
            Self::Storage => "See how much space your library uses and what could safely shrink.",
            Self::Platforms => "Choose a system to explore its games.",
            Self::Check => "Find missing, unknown, damaged or mismatched games.",
            Self::Problems => "Review problems and preview a fix before changing anything.",
            Self::Build => {
                "Arrange or publish verified games with a preview before anything changes."
            }
            Self::Converter => "Open the existing verified conversion tools.",
            Self::Tape => "Inspect supported tape images without changing them.",
            Self::Museum => "Browse the existing collection museum by platform.",
            Self::Launch => "Choose a game. EmuWiz checks its setup before starting it.",
            Self::Emulators => {
                "Find installed emulators and see what they need to play your games."
            }
            Self::Firmware => {
                "Review required BIOS and firmware without changing emulator settings."
            }
            Self::Mods => {
                "Find improvements for a game and preview every change before installing."
            }
            Self::Artwork => "Find covers, screenshots and information for your games.",
            Self::Sources => "Find your game folders and choose which ones to include.",
            Self::Romm => "Browse the read-only RomM library snapshot and its provenance.",
            Self::Dat => {
                "Manage the trusted game identification data EmuWiz uses to check releases."
            }
            Self::Activity => "See what is happening, how it is going and what to do next.",
            Self::History => "Review previous changes and the recovery options available for them.",
            Self::Settings => "Adjust this interface without changing your games.",
            Self::Advanced => "Explore detailed tools. Opening this page changes nothing.",
            Self::DatVerification
            | Self::CheatsMods
            | Self::SavesStates
            | Self::EmulatorsFamily
            | Self::Mame
            | Self::ArtworkExtras
            | Self::Conversion
            | Self::OrganisationFamily
            | Self::ProblemsRepair
            | Self::SourcesProviders
            | Self::HistoryUndo
            | Self::AdvancedDiagnostics => family_for_route(&Route::Section(self))
                .map(FeatureFamily::purpose)
                .unwrap_or("Choose a workflow. Opening a shortcut changes nothing."),
        }
    }

    pub fn action(self) -> &'static str {
        match self {
            Self::Check => "Check my games",
            Self::Setup => "Check my setup",
            Self::Duplicates => "Review duplicates",
            Self::MultiDisc => "Check my multi-disc games",
            Self::Storage => "Check my storage",
            Self::Problems => "Review problems",
            Self::Build => "Choose an organisation method",
            Self::Converter => "Open Converter",
            Self::Tape => "Inspect tape media",
            Self::Museum => "Open Museum",
            Self::Emulators => "Check Emulators",
            Self::Firmware => "Inspect BIOS / Firmware",
            Self::Mods => "Choose a game",
            Self::Artwork => "Manage artwork",
            Self::Sources => "Find game folders",
            Self::Romm => "Browse RomM library",
            Self::Dat => "Manage identification data",
            Self::History => "Review previous changes",
            Self::Advanced => "Open specialist tools",
            Self::DatVerification
            | Self::CheatsMods
            | Self::SavesStates
            | Self::EmulatorsFamily
            | Self::Mame
            | Self::ArtworkExtras
            | Self::Conversion
            | Self::OrganisationFamily
            | Self::ProblemsRepair
            | Self::SourcesProviders
            | Self::HistoryUndo
            | Self::AdvancedDiagnostics => "Open family",
            _ => "Browse my games",
        }
    }

    pub fn group(self) -> Option<&'static str> {
        match self {
            Self::Games => Some("LIBRARY"),
            Self::Saves => Some("LIBRARY"),
            Self::Duplicates | Self::MultiDisc => Some("LIBRARY"),
            Self::Launch => Some("PLAY"),
            Self::Mods => Some("TOOLS"),
            Self::Converter | Self::Storage | Self::Tape | Self::Museum => Some("TOOLS"),
            Self::Romm => Some("LIBRARY"),
            Self::DatVerification
            | Self::CheatsMods
            | Self::SavesStates
            | Self::EmulatorsFamily
            | Self::Mame
            | Self::ArtworkExtras
            | Self::Conversion
            | Self::OrganisationFamily
            | Self::ProblemsRepair
            | Self::SourcesProviders
            | Self::HistoryUndo
            | Self::AdvancedDiagnostics => Some("FAMILIES"),
            _ => None,
        }
    }
}

/// The stable information-architecture homes for GUI-v2 workflows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum FeatureFamily {
    DatsVerification,
    CheatsMods,
    SavesStates,
    Emulators,
    Mame,
    ArtworkExtras,
    Conversion,
    Organisation,
    ProblemsRepair,
    SourcesProviders,
    HistoryUndo,
    AdvancedDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FamilyVariant {
    Normal,
    Easy,
    Advanced,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FamilyAction {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub route: Route,
    pub variant: FamilyVariant,
}

impl FeatureFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::DatsVerification => "DATs & Verification",
            Self::CheatsMods => "Cheats & Mods",
            Self::SavesStates => "Saves & States",
            Self::Emulators => "Emulators",
            Self::Mame => "MAME",
            Self::ArtworkExtras => "Artwork & Extras",
            Self::Conversion => "Conversion",
            Self::Organisation => "Organisation",
            Self::ProblemsRepair => "Problems & Repair",
            Self::SourcesProviders => "Sources & Providers",
            Self::HistoryUndo => "History & Undo",
            Self::AdvancedDiagnostics => "Advanced / Diagnostics",
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::DatsVerification => {
                "Identify games, review evidence and make safe rename decisions."
            }
            Self::CheatsMods => "Keep cheats, mods and their conflicts discoverable in one place.",
            Self::SavesStates => "Manage saves, snapshots, restores and memory-card workflows.",
            Self::Emulators => "Set up emulators and review the firmware they require.",
            Self::Mame => "Inspect MAME collections without hiding the existing MAME workflows.",
            Self::ArtworkExtras => {
                "Review a game's artwork, associated manuals and supported extras together."
            }
            Self::Conversion => "Open the existing verified conversion workflows.",
            Self::Organisation => "Choose how verified games are arranged or published.",
            Self::ProblemsRepair => "Review the global problems inbox and safe repair paths.",
            Self::SourcesProviders => "Keep local sources, DATs and remote providers distinct.",
            Self::HistoryUndo => "Review the one authoritative history and its undo options.",
            Self::AdvancedDiagnostics => {
                "Open specialist tools with an explicit explanation of each one."
            }
        }
    }
}

pub(super) fn family_home(family: FeatureFamily) -> Route {
    Route::Section(match family {
        FeatureFamily::DatsVerification => Section::DatVerification,
        FeatureFamily::CheatsMods => Section::CheatsMods,
        FeatureFamily::SavesStates => Section::SavesStates,
        FeatureFamily::Emulators => Section::EmulatorsFamily,
        FeatureFamily::Mame => Section::Mame,
        FeatureFamily::ArtworkExtras => Section::ArtworkExtras,
        FeatureFamily::Conversion => Section::Conversion,
        FeatureFamily::Organisation => Section::OrganisationFamily,
        FeatureFamily::ProblemsRepair => Section::ProblemsRepair,
        FeatureFamily::SourcesProviders => Section::SourcesProviders,
        FeatureFamily::HistoryUndo => Section::HistoryUndo,
        FeatureFamily::AdvancedDiagnostics => Section::AdvancedDiagnostics,
    })
}

pub(super) fn family_for_route(route: &Route) -> Option<FeatureFamily> {
    if matches!(route, Route::BrowsePlay | Route::BrowsePlayGame(_)) {
        return None;
    }
    let section = route.section();
    Some(match section {
        Section::Check | Section::Dat | Section::DatVerification => FeatureFamily::DatsVerification,
        Section::Mods | Section::CheatsMods => FeatureFamily::CheatsMods,
        Section::Saves | Section::SavesStates => FeatureFamily::SavesStates,
        Section::Emulators | Section::Firmware | Section::EmulatorsFamily => {
            FeatureFamily::Emulators
        }
        Section::Mame => FeatureFamily::Mame,
        Section::Artwork | Section::ArtworkExtras | Section::Museum => FeatureFamily::ArtworkExtras,
        Section::Converter | Section::Storage | Section::Conversion => FeatureFamily::Conversion,
        Section::Build | Section::OrganisationFamily | Section::Games | Section::Launch => {
            FeatureFamily::Organisation
        }
        Section::Problems | Section::ProblemsRepair | Section::Duplicates | Section::MultiDisc => {
            FeatureFamily::ProblemsRepair
        }
        Section::Sources | Section::Romm | Section::SourcesProviders => {
            FeatureFamily::SourcesProviders
        }
        Section::History | Section::Activity | Section::HistoryUndo => FeatureFamily::HistoryUndo,
        Section::Advanced | Section::Settings | Section::Tape | Section::AdvancedDiagnostics => {
            FeatureFamily::AdvancedDiagnostics
        }
        Section::Home => return None,
        Section::Platforms => FeatureFamily::Organisation,
        Section::Setup => FeatureFamily::AdvancedDiagnostics,
    })
}

pub(super) fn family_children(family: FeatureFamily) -> Vec<FamilyAction> {
    use FamilyVariant::{Advanced, Easy, Normal};
    let action = |id, label, description, route, variant| FamilyAction {
        id,
        label,
        description,
        route,
        variant,
    };
    match family {
        FeatureFamily::DatsVerification => vec![
            action(
                "check",
                "Check Games",
                "Find missing, unknown, damaged or mismatched games.",
                Route::Section(Section::Check),
                Normal,
            ),
            action(
                "quick-rename",
                "Quick Rename",
                "Rename verified games through the existing easy organisation workflow.",
                Route::QuickRename,
                Easy,
            ),
            action(
                "advanced-rename",
                "Advanced Rename",
                "Review the full organisation and preview workflow.",
                Route::Section(Section::Build),
                Advanced,
            ),
            action(
                "dat-management",
                "DAT Management",
                "Manage trusted identification data.",
                Route::Section(Section::Dat),
                Normal,
            ),
            action(
                "repair",
                "Repair from DAT evidence",
                "Review repair candidates in the global problems inbox.",
                Route::Section(Section::Problems),
                Normal,
            ),
            action(
                "history",
                "History",
                "Review the authoritative change history.",
                Route::Section(Section::History),
                Normal,
            ),
        ],
        FeatureFamily::CheatsMods => vec![
            action(
                "cheats",
                "Cheats",
                "Open the existing cheats and mods workflow.",
                Route::Section(Section::Mods),
                Normal,
            ),
            action(
                "mods",
                "Mods",
                "Browse available game improvements.",
                Route::Section(Section::Mods),
                Normal,
            ),
            action(
                "conflicts",
                "Conflicts",
                "Review problems that need attention.",
                Route::Section(Section::Problems),
                Normal,
            ),
            action(
                "installed",
                "Installed",
                "Review the existing installed-mod workflow.",
                Route::Section(Section::Mods),
                Normal,
            ),
            action(
                "history",
                "History",
                "Review the authoritative change history.",
                Route::Section(Section::History),
                Normal,
            ),
        ],
        FeatureFamily::SavesStates => vec![
            action(
                "saves",
                "Saves",
                "Review portable and emulator-bound saves.",
                Route::Section(Section::Saves),
                Normal,
            ),
            action(
                "snapshots",
                "Snapshots",
                "Open the existing saves and states inventory.",
                Route::Section(Section::Saves),
                Normal,
            ),
            action(
                "restore",
                "Restore",
                "Review restore options in the saves workflow.",
                Route::Section(Section::Saves),
                Normal,
            ),
            action(
                "memory-cards",
                "Memory Cards",
                "Review memory-card state in the saves workflow.",
                Route::Section(Section::Saves),
                Normal,
            ),
            action(
                "history",
                "History",
                "Review the authoritative change history.",
                Route::Section(Section::History),
                Normal,
            ),
        ],
        FeatureFamily::Emulators => vec![
            action(
                "setup",
                "Emulator Setup",
                "Find installed emulators and their requirements.",
                Route::Section(Section::Emulators),
                Normal,
            ),
            action(
                "firmware",
                "BIOS / Firmware",
                "Review required firmware without changing settings.",
                Route::Section(Section::Firmware),
                Normal,
            ),
        ],
        FeatureFamily::Mame => vec![
            action(
                "health",
                "Health",
                "See whether the current MAME collection is healthy or needs attention.",
                Route::MameWorkflow,
                Normal,
            ),
            action(
                "repair",
                "Repair",
                "Review exact local evidence for missing or wrong members before changing anything.",
                Route::MameWorkflow,
                Normal,
            ),
            action(
                "reconstruction",
                "Reconstruction",
                "Preview a complete merged set from verified parent, clone, and member evidence.",
                Route::MameWorkflow,
                Normal,
            ),
            action(
                "verify",
                "Verify",
                "Check the current result; old repair receipts never replace current evidence.",
                Route::MameWorkflow,
                Normal,
            ),
            action(
                "playing-library",
                "Playing Library",
                "Create a clean play-focused view without changing the archival collection.",
                Route::MameWorkflow,
                Normal,
            ),
            action(
                "problems",
                "Problems",
                "Review set and member evidence inside the MAME workflow.",
                Route::MameWorkflow,
                Normal,
            ),
            action(
                "history-undo",
                "History & Undo",
                "Review previous repairs and reconstruction receipts separately from current health.",
                Route::MameWorkflow,
                Normal,
            ),
        ],
        FeatureFamily::ArtworkExtras => vec![action(
            "artwork-manuals-extras",
            "Artwork, Manuals & Extras",
            "Review the selected game's artwork, associated manuals and supported extras together.",
            Route::Section(Section::Artwork),
            Normal,
        )],
        FeatureFamily::Conversion => vec![
            action(
                "disc-conversion",
                "Disc Conversion",
                "Review supported disc conversion formats and safety before applying.",
                Route::Section(Section::Converter),
                Normal,
            ),
            action(
                "conversion-history",
                "History & Undo",
                "Review completed operations and available undo actions.",
                Route::Section(Section::History),
                Normal,
            ),
        ],
        FeatureFamily::Organisation => vec![
            action(
                "easy-organiser",
                "Easy Organiser",
                "Choose a guided organisation workflow.",
                Route::Section(Section::Build),
                Easy,
            ),
            action(
                "advanced-organiser",
                "Advanced Organiser",
                "Review specialist organisation options.",
                Route::Section(Section::Build),
                Advanced,
            ),
            action(
                "playing-library",
                "Playing Library",
                "Open the existing playing-library projection.",
                Route::Section(Section::Build),
                Normal,
            ),
            action(
                "history",
                "History",
                "Review the authoritative change history.",
                Route::Section(Section::History),
                Normal,
            ),
        ],
        FeatureFamily::ProblemsRepair => vec![
            action(
                "inbox",
                "Problems Inbox",
                "Review all currently known problems.",
                Route::Section(Section::Problems),
                Normal,
            ),
            action(
                "repair",
                "Repair",
                "Preview safe repair paths before anything changes.",
                Route::Section(Section::Problems),
                Normal,
            ),
            action(
                "history",
                "History",
                "Review repair history and undo options.",
                Route::Section(Section::History),
                Normal,
            ),
        ],
        FeatureFamily::SourcesProviders => vec![
            action(
                "local",
                "Local Sources",
                "Manage configured local game folders.",
                Route::Section(Section::Sources),
                Normal,
            ),
            action(
                "dat-sources",
                "DAT Sources",
                "Manage trusted DAT sources.",
                Route::Section(Section::Dat),
                Normal,
            ),
            action(
                "providers",
                "Metadata Providers",
                "Review provider configuration without merging concepts.",
                Route::Section(Section::Sources),
                Normal,
            ),
            action(
                "romm",
                "RomM",
                "Browse the read-only RomM snapshot.",
                Route::Section(Section::Romm),
                Normal,
            ),
            action(
                "remote-health",
                "Remote Health",
                "Review provider health in Sources.",
                Route::Section(Section::Sources),
                Normal,
            ),
        ],
        FeatureFamily::HistoryUndo => vec![
            action(
                "history",
                "History",
                "Open the one authoritative history system.",
                Route::Section(Section::History),
                Normal,
            ),
            action(
                "activity",
                "Activity",
                "See current and recent work.",
                Route::Section(Section::Activity),
                Normal,
            ),
            action(
                "undo",
                "Undo",
                "Review undo options in History.",
                Route::Section(Section::History),
                Normal,
            ),
        ],
        FeatureFamily::AdvancedDiagnostics => vec![
            action(
                "advanced",
                "Advanced Tools",
                "Open specialist tools explicitly.",
                Route::Section(Section::Advanced),
                Advanced,
            ),
            action(
                "diagnostics",
                "Diagnostics",
                "Review setup and diagnostic workflows.",
                Route::Section(Section::Setup),
                Normal,
            ),
            action(
                "tape",
                "Tape Inspector",
                "Inspect supported tape media.",
                Route::Section(Section::Tape),
                Normal,
            ),
            action(
                "settings",
                "Settings",
                "Adjust this interface without changing games.",
                Route::Section(Section::Settings),
                Normal,
            ),
        ],
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Route {
    #[default]
    Home,
    BrowsePlay,
    BrowsePlayGame(i64),
    MameWorkflow,
    Section(Section),
    Game(i64),
    QuickRename,
    Task {
        section: Section,
        game: i64,
    },
}

impl Route {
    pub fn section(&self) -> Section {
        match self {
            Self::Home => Section::Home,
            Self::BrowsePlay | Self::BrowsePlayGame(_) => Section::Games,
            Self::MameWorkflow => Section::Mame,
            Self::Section(section) | Self::Task { section, .. } => *section,
            Self::Game(_) => Section::Games,
            Self::QuickRename => Section::Dat,
        }
    }
    pub fn game(&self) -> Option<i64> {
        match self {
            Self::BrowsePlayGame(id) => Some(*id),
            Self::Game(id) | Self::Task { game: id, .. } => Some(*id),
            _ => None,
        }
    }

    /// The same route with its game id replaced.
    pub fn with_game(&self, new: i64) -> Self {
        match self {
            Self::BrowsePlayGame(_) => Self::BrowsePlayGame(new),
            Self::Game(_) => Self::Game(new),
            Self::Task { section, .. } => Self::Task {
                section: *section,
                game: new,
            },
            other => other.clone(),
        }
    }
}

/// Presentation-only location labels for the app chrome. Navigation remains
/// owned by `Route` and `Router`.
pub(super) fn breadcrumb_labels(route: &Route, game_title: Option<&str>) -> Vec<String> {
    if matches!(route, Route::BrowsePlay | Route::BrowsePlayGame(_)) {
        let mut labels = vec!["Browse & Play".to_string()];
        if let Some(game_title) = game_title.filter(|title| !title.trim().is_empty()) {
            labels.push(game_title.to_string());
        }
        return labels;
    }
    if matches!(route, Route::QuickRename) {
        return vec!["DATs & Verification".into(), "Quick Rename".into()];
    }
    if matches!(route, Route::Game(_)) {
        // Game Details is opened from the Games library, not from Organisation.
        let mut labels = vec!["Games".to_string()];
        if let Some(game_title) = game_title.filter(|title| !title.trim().is_empty()) {
            labels.push(game_title.to_string());
        }
        return labels;
    }
    if matches!(route, Route::Section(Section::Games)) {
        return vec!["Games".into()];
    }
    let section = route.section();
    let mut labels = family_for_route(route)
        .map(|family| vec![family.label().to_string()])
        .unwrap_or_default();
    let title = section.title().to_string();
    if labels.is_empty() {
        labels.push(title);
    } else if labels.last() != Some(&title) && section != Section::Games {
        labels.push(title);
    }
    if let Some(game_title) = game_title.filter(|title| !title.trim().is_empty()) {
        labels.push(game_title.to_string());
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::{Route, Section, breadcrumb_labels, family_for_route};

    #[test]
    fn game_details_breadcrumb_is_games_then_the_title() {
        assert_eq!(
            breadcrumb_labels(&Route::Game(7), Some("Pac-Man")),
            ["Games", "Pac-Man"]
        );
        assert_eq!(breadcrumb_labels(&Route::Game(7), None), ["Games"]);
    }

    #[test]
    fn breadcrumbs_follow_route_and_selected_game_context() {
        assert_eq!(breadcrumb_labels(&Route::Home, None), ["Home"]);
        assert_eq!(
            breadcrumb_labels(&Route::Section(Section::Games), None),
            ["Games"]
        );
        assert_eq!(
            breadcrumb_labels(&Route::Section(Section::Check), None),
            ["DATs & Verification", "Check Games"]
        );
        assert_eq!(
            breadcrumb_labels(
                &Route::Task {
                    section: Section::Mods,
                    game: 7
                },
                Some("Pac-Man")
            ),
            ["Cheats & Mods", "Mods & Cheats", "Pac-Man"]
        );
        assert_eq!(
            breadcrumb_labels(&Route::QuickRename, None),
            ["DATs & Verification", "Quick Rename"]
        );
        assert_eq!(Route::QuickRename.section(), Section::Dat);
    }

    #[test]
    fn browse_play_is_a_presentation_route_outside_feature_families() {
        assert_eq!(Route::BrowsePlay.section(), Section::Games);
        assert_eq!(Route::BrowsePlayGame(7).game(), Some(7));
        assert_eq!(family_for_route(&Route::BrowsePlay), None);
        assert_eq!(
            breadcrumb_labels(&Route::BrowsePlayGame(7), Some("Pac-Man")),
            ["Browse & Play", "Pac-Man"]
        );
    }
}

/// Migrate routes written by the pre-retirement shell.  `Advanced` used to
/// be the normal DAT page; it now intentionally names the specialist escape.
pub(super) fn migrate_route(route: Route) -> Route {
    if route == Route::Section(Section::Advanced) {
        Route::Section(Section::Dat)
    } else {
        route
    }
}

#[derive(Default)]
pub(super) struct Router {
    pub current: Route,
    back: Vec<Route>,
}

impl Router {
    pub fn go(&mut self, route: Route) {
        if self.current == route {
            return;
        }
        self.back.push(self.current.clone());
        if self.back.len() > 64 {
            self.back.remove(0);
        }
        self.current = route;
    }
    pub fn back(&mut self) {
        self.current = self.back.pop().unwrap_or(Route::Home);
    }

    /// Re-points every remembered game id (current page and back stack).
    pub fn remap_games(&mut self, resolve: impl Fn(i64) -> i64) {
        let remap = |route: &Route| match route.game() {
            Some(id) if resolve(id) != id => route.with_game(resolve(id)),
            _ => route.clone(),
        };
        self.current = remap(&self.current);
        for route in &mut self.back {
            *route = remap(route);
        }
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
}

pub(super) const HOME_TASKS: &[(Section, &str, &str, &str)] = &[
    (
        Section::Setup,
        "Setup & Doctor",
        "See what EmuWiz can use now, what needs attention, and the next safe step.",
        "Check my setup",
    ),
    (
        Section::Games,
        "Browse My Games",
        "Find a game, see its information and get ready to play.",
        "Browse my games",
    ),
    (
        Section::Check,
        "Check My Games",
        "Find missing, unknown, damaged or mismatched games.",
        "Check my games",
    ),
    (
        Section::Problems,
        "Fix Problems",
        "Understand problems and preview safe repairs.",
        "Review problems",
    ),
    (
        Section::Build,
        "Organisation",
        "Arrange verified games or prepare a linked library for your frontend.",
        "Choose an organisation method",
    ),
    (
        Section::Mods,
        "Mods & Cheats",
        "Choose a game and explore its available improvements.",
        "Choose a game",
    ),
    (
        Section::Launch,
        "Play",
        "Choose a game and check that it is ready to start.",
        "Choose a game to play",
    ),
];
