//! Stable task locations, independent of legacy tabs and presentation modes.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) enum Section {
    #[default]
    Home,
    Games,
    Platforms,
    Check,
    Problems,
    Build,
    Launch,
    Emulators,
    Mods,
    Artwork,
    Sources,
    Activity,
    History,
    Settings,
    Advanced,
}

pub(super) const SECTIONS: &[Section] = &[
    Section::Home,
    Section::Games,
    Section::Platforms,
    Section::Check,
    Section::Problems,
    Section::Build,
    Section::Launch,
    Section::Emulators,
    Section::Mods,
    Section::Artwork,
    Section::Sources,
    Section::Activity,
    Section::History,
    Section::Settings,
    Section::Advanced,
];

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Games => "Games",
            Self::Platforms => "Platforms",
            Self::Check => "Check Games",
            Self::Problems => "Problems & Repair",
            Self::Build => "Build Library",
            Self::Launch => "Launch",
            Self::Emulators => "Emulator Setup",
            Self::Mods => "Mods & Cheats",
            Self::Artwork => "Artwork & Metadata",
            Self::Sources => "Sources",
            Self::Activity => "Activity",
            Self::History => "History",
            Self::Settings => "Settings",
            Self::Advanced => "Advanced",
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::Home => "Your games, and the things you can do with them.",
            Self::Games => "Browse your games. Select one to see what you can do next.",
            Self::Platforms => "Choose a system to explore its games.",
            Self::Check => "Find missing, unknown, damaged or mismatched games.",
            Self::Problems => "Review problems and preview a fix before changing anything.",
            Self::Build => {
                "Make a playing library for your favourite launcher, keeping originals safe."
            }
            Self::Launch => "Choose a game. EmuWiz checks its setup before starting it.",
            Self::Emulators => {
                "Find installed emulators and see what they need to play your games."
            }
            Self::Mods => {
                "Find improvements for a game and preview every change before installing."
            }
            Self::Artwork => "Find covers, screenshots and information for your games.",
            Self::Sources => "Find your game folders and choose which ones to include.",
            Self::Activity => "See what is happening, how it is going and what to do next.",
            Self::History => "Review previous changes and the recovery options available for them.",
            Self::Settings => "Adjust this interface without changing your games.",
            Self::Advanced => "Explore detailed tools. Opening this page changes nothing.",
        }
    }

    pub fn action(self) -> &'static str {
        match self {
            Self::Check => "Check my games",
            Self::Problems => "Review problems",
            Self::Build => "Build my library",
            Self::Emulators => "Find installed emulators",
            Self::Mods => "Choose a game",
            Self::Artwork => "Manage artwork",
            Self::Sources => "Find game folders",
            Self::History => "Review previous changes",
            Self::Advanced => "Open Legacy / Advanced interface",
            _ => "Browse my games",
        }
    }

    pub fn group(self) -> Option<&'static str> {
        match self {
            Self::Games => Some("LIBRARY"),
            Self::Launch => Some("PLAY"),
            Self::Mods => Some("TOOLS"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Route {
    #[default]
    Home,
    Section(Section),
    Game(i64),
    Task {
        section: Section,
        game: i64,
    },
}

impl Route {
    pub fn section(&self) -> Section {
        match self {
            Self::Home => Section::Home,
            Self::Section(section) | Self::Task { section, .. } => *section,
            Self::Game(_) => Section::Games,
        }
    }
    pub fn game(&self) -> Option<i64> {
        match self {
            Self::Game(id) | Self::Task { game: id, .. } => Some(*id),
            _ => None,
        }
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
}

pub(super) const HOME_TASKS: &[(Section, &str, &str, &str)] = &[
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
        "Build My Library",
        "Prepare a playing library without moving your originals.",
        "Build my library",
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
