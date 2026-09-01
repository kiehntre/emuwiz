//! The Home page: "What would you like to do?"
//!
//! # Why this page exists
//!
//! Every one of the seven workflows here already shipped on its own page.
//! What was missing was a single place that names them as tasks, in plain
//! language, and says honestly whether each one looks ready, not yet set
//! up, or currently unavailable. Nothing on this page is a new capability -
//! it is a front door onto capabilities that already exist.
//!
//! # The view model
//!
//! Following `romm_source`/`cheat_sources_page`, authoritative state is
//! turned into a [`HomeView`] by the pure [`build_home_view`], and the
//! drawing code only draws. `main.rs` builds [`HomeInputs`] from whatever
//! is already loaded on `ArchiveFsApp` - never from a fresh read.
//!
//! Some cards read state that EmuWiz deliberately does not load until the
//! user visits that page. Home omits those badges until their real state is
//! known rather than presenting an ordinary lazy load as a problem.

use crate::ui::{components as widgets, theme};
use eframe::egui;

/// One of the task-oriented destinations Home can send a user to.
/// `main.rs` maps each variant to the `MainView` (and, for the two that
/// need it, the extra dispatch logic) its sidebar button already uses -
/// Home never invents a second way to reach a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HomeCard {
    BuildLibrary,
    ConvertDiscs,
    BrowseGames,
    DuplicateReview,
    CheatsAndMods,
    CanonicalOrganisation,
    QuickRename,
    CheatSources,
    DatSources,
    RomM,
    CheckSetup,
    Settings,
}

/// Honest readiness for a card, distinguishing four situations that read
/// very differently to a user:
///
/// - [`Self::NotConfigured`]: nothing is set up yet. Expected on a fresh
///   install, never shown as a fault.
/// - [`Self::Unavailable`]: it is configured, but something about it is
///   currently not working (a real error/warning, or a provider state that
///   is not ready). Never conflated with "not configured".
/// - [`Self::Ready`]: configured and, as far as already-loaded state shows,
///   usable.
/// - [`Self::Unknown`]: Home has loaded an active task state but not enough
///   evidence to claim ready or unavailable. Lazily loaded destinations
///   omit the badge entirely until their page has established a state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CardReadiness {
    NotConfigured(String),
    Unavailable(String),
    Ready(String),
    Unknown(String),
}

impl CardReadiness {
    fn tone(&self) -> widgets::StatusTone {
        match self {
            Self::NotConfigured(_) => widgets::StatusTone::Pending,
            Self::Unavailable(_) => widgets::StatusTone::Warning,
            Self::Ready(_) => widgets::StatusTone::Success,
            Self::Unknown(_) => widgets::StatusTone::Info,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::NotConfigured(text)
            | Self::Unavailable(text)
            | Self::Ready(text)
            | Self::Unknown(text) => text,
        }
    }
}

/// How much visual presence a Home card gets. Primary destinations (the
/// major jobs a user comes to EmuWiz for) are larger and drawn first;
/// secondary/admin destinations stay available but quieter. Nothing is
/// hidden - this is hierarchy, not removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomeCardTier {
    Primary,
    Secondary,
}

/// Concept colour for a primary workflow. It never communicates readiness or
/// severity; those continue to use semantic status badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomeAccent {
    Games,
    Organise,
    Check,
    Cheats,
    Verify,
    Settings,
}

impl HomeAccent {
    fn color(self) -> egui::Color32 {
        match self {
            Self::Games => egui::Color32::from_rgb(88, 145, 232),
            Self::Organise => egui::Color32::from_rgb(151, 112, 220),
            Self::Check => egui::Color32::from_rgb(72, 174, 153),
            Self::Cheats => egui::Color32::from_rgb(211, 101, 126),
            Self::Verify => egui::Color32::from_rgb(214, 164, 67),
            Self::Settings => egui::Color32::from_rgb(132, 151, 174),
        }
    }
}

/// One rendered card: what it is, why it matters, whether it looks ready,
/// and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HomeCardView {
    pub(crate) card: HomeCard,
    /// A leading glyph drawn next to the title. Always accompanied by the
    /// text title - never the only navigation cue.
    pub(crate) icon: &'static str,
    pub(crate) title: &'static str,
    pub(crate) explanation: &'static str,
    /// Optional supporting terminology, always rendered in a collapsed
    /// disclosure rather than in the task-first summary.
    pub(crate) technical_detail: Option<&'static str>,
    pub(crate) tier: HomeCardTier,
    pub(crate) accent: Option<HomeAccent>,
    /// `None` for the one card (Settings) with no single configured/not
    /// configured state to report.
    pub(crate) readiness: Option<CardReadiness>,
    pub(crate) action_label: &'static str,
    /// A second, smaller link some cards offer alongside their primary
    /// action - e.g. Cheats & Mods also links to Cheat Sources.
    pub(crate) secondary: Option<(HomeCard, &'static str)>,
}

/// Whether, and how, to show the banner above the card grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomeBanner {
    /// No configuration file has ever been seen this session: a genuine
    /// first run, not a fault.
    FreshInstall,
    /// A configuration file was seen and confirmed earlier this session,
    /// and is no longer found. Never shown with [`Self::FreshInstall`]'s
    /// cheerful wording - see `missing_config_is_first_run` in `main.rs`,
    /// which this page reuses rather than re-deriving the distinction.
    ConfigDisappeared,
    /// Nothing to say: either configured, or a check is still loading.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HomeView {
    pub(crate) banner: HomeBanner,
    pub(crate) cards: Vec<HomeCardView>,
}

/// Everything [`build_home_view`] needs, gathered by `main.rs` from state
/// that is already loaded - building this never reads a file, starts a
/// thread, or makes a request.
pub(crate) struct HomeInputs {
    pub(crate) source_folder_count: usize,
    pub(crate) has_database: bool,
    /// A read of `ArchiveFsApp::doctor_scan` - the exact state the "Set up
    /// emulators" card's destination renders, so the badge and that page can
    /// never disagree.
    pub(crate) setup_check: SetupCheckSummary,
    pub(crate) config_missing: bool,
    /// Mirrors `missing_config_is_first_run(config_previously_confirmed)`.
    pub(crate) first_run: bool,
    /// `None` until the Cheat Sources page has been visited this session.
    pub(crate) cheat_sources_enabled_count: Option<usize>,
    /// `None` until the DAT Sources page has been visited this session.
    pub(crate) dat_sources_registered_count: Option<usize>,
    /// `None` until the Sources page has loaded RomM status this session.
    /// The label is already plain language (`ProviderState::label`).
    pub(crate) romm_state_label: Option<RommReadinessLabel>,
}

/// The three buckets a `ProviderState` collapses into for Home, plus its
/// existing display label - built by `main.rs` so this module does not
/// need to depend on `archivefs_core::identity_source`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RommReadinessLabel {
    NotConfigured(&'static str),
    Unavailable(&'static str),
    Ready(&'static str),
}

/// A compressed read of `ArchiveFsApp::doctor_scan` - the exact state the
/// "Set up emulators" card's destination (Problems & Repair -> Diagnostics)
/// renders. Built by `main.rs` (see `setup_check_summary`) so this module
/// needs no dependency on the Doctor engine, and so the card summarises the
/// *same* state a click opens - never the separate `self.diagnostics`
/// background report, which is a different subsystem with a different notion
/// of "last run".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupCheckSummary {
    /// Doctor has not finished a run this session.
    NeverRun,
    /// A Doctor run is in flight and there is no earlier result on screen.
    Running,
    /// Doctor finished, but no check could actually run (every subsystem was
    /// unavailable). Never reported as a pass.
    NoChecksRun,
    /// Doctor finished with nothing actionable.
    Healthy,
    /// Doctor finished with warnings only (nothing blocking).
    Warnings(usize),
    /// Doctor finished with at least one blocking (error/critical) finding.
    NeedsAttention(usize),
}

/// Turns already-loaded state into what Home draws. Pure: the same inputs
/// always produce the same view, and nothing here touches disk or a
/// socket.
pub(crate) fn build_home_view(inputs: &HomeInputs) -> HomeView {
    let banner = if inputs.config_missing {
        if inputs.first_run {
            HomeBanner::FreshInstall
        } else {
            HomeBanner::ConfigDisappeared
        }
    } else {
        HomeBanner::None
    };

    let library_readiness = if inputs.source_folder_count == 0 {
        CardReadiness::NotConfigured("No source folders yet".to_string())
    } else {
        CardReadiness::Ready(format!(
            "{} source folder{} configured",
            inputs.source_folder_count,
            if inputs.source_folder_count == 1 {
                ""
            } else {
                "s"
            }
        ))
    };

    let browse_readiness = if inputs.source_folder_count == 0 {
        CardReadiness::NotConfigured("Add a source folder first".to_string())
    } else if !inputs.has_database {
        CardReadiness::Unknown("Not scanned yet this session".to_string())
    } else {
        CardReadiness::Ready("Ready to browse".to_string())
    };

    let cheats_readiness = match inputs.cheat_sources_enabled_count {
        None => None,
        Some(0) => Some(CardReadiness::NotConfigured(
            "No cheat sources enabled".to_string(),
        )),
        Some(n) => Some(CardReadiness::Ready(format!(
            "{n} cheat source{} enabled",
            plural(n)
        ))),
    };

    let dat_readiness = match inputs.dat_sources_registered_count {
        None => None,
        Some(0) => Some(CardReadiness::NotConfigured(
            "No trusted catalogues added yet".to_string(),
        )),
        Some(n) => Some(CardReadiness::Ready(format!(
            "{n} trusted catalogue{} added",
            plural(n)
        ))),
    };

    let romm_readiness = match &inputs.romm_state_label {
        None => None,
        Some(RommReadinessLabel::NotConfigured(label)) => {
            Some(CardReadiness::NotConfigured((*label).to_string()))
        }
        Some(RommReadinessLabel::Unavailable(label)) => {
            Some(CardReadiness::Unavailable((*label).to_string()))
        }
        Some(RommReadinessLabel::Ready(label)) => Some(CardReadiness::Ready((*label).to_string())),
    };

    let setup_readiness = summarize_setup_checks(inputs.setup_check);

    let build_library = HomeCardView {
        card: HomeCard::BuildLibrary,
        icon: crate::ui::icons::SOURCES,
        title: if inputs.source_folder_count == 0 {
            "Add your games"
        } else {
            "Build my library"
        },
        explanation: if inputs.source_folder_count == 0 {
            "Choose the folder where your games are stored. EmuWiz will scan it without changing the files."
        } else {
            "Choose and review the folders EmuWiz scans for games."
        },
        technical_detail: None,
        tier: if inputs.source_folder_count == 0 {
            HomeCardTier::Primary
        } else {
            HomeCardTier::Secondary
        },
        accent: (inputs.source_folder_count == 0).then_some(HomeAccent::Games),
        readiness: Some(library_readiness),
        action_label: if inputs.source_folder_count == 0 {
            "Add game folder"
        } else {
            "Open Sources"
        },
        secondary: None,
    };

    let mut cards = vec![
        // --- Primary destinations: the major jobs -------------------------
        HomeCardView {
            card: HomeCard::BrowseGames,
            icon: crate::ui::icons::GAMES,
            title: "My Games",
            explanation: "Browse your games. See the library EmuWiz has found, organised and searchable.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Games),
            readiness: Some(browse_readiness),
            action_label: "Open Library",
            secondary: None,
        },
        HomeCardView {
            card: HomeCard::CanonicalOrganisation,
            icon: crate::ui::icons::ORGANISE,
            title: "Organise",
            explanation: "Rename and tidy your library. Preview how your games can be renamed or moved into platform folders; nothing moves until you approve it.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Organise),
            readiness: None,
            action_label: "Open Organise",
            secondary: Some((HomeCard::QuickRename, "Quick Rename")),
        },
        HomeCardView {
            card: HomeCard::CheckSetup,
            icon: crate::ui::icons::CHECK,
            title: "Set up emulators",
            explanation: "Check which emulators EmuWiz can find and whether each is ready to launch games.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Check),
            readiness: Some(setup_readiness),
            action_label: "Open Emulator Setup",
            secondary: None,
        },
        HomeCardView {
            card: HomeCard::CheatsAndMods,
            icon: crate::ui::icons::CHEATS,
            title: "Cheats & Mods",
            explanation: "Find cheats and game enhancements for a selected game.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Cheats),
            readiness: cheats_readiness,
            action_label: "Open Cheats & Mods",
            secondary: Some((HomeCard::CheatSources, "Manage Cheat Sources")),
        },
        HomeCardView {
            card: HomeCard::DatSources,
            icon: crate::ui::icons::VERIFY,
            title: "Verify your games",
            explanation: "Check game names, versions, and known-good file matches using trusted game catalogues.",
            technical_detail: Some(
                "These trusted game catalogues are commonly called DAT files. Verification is read-only: nothing is renamed, moved, or rewritten.",
            ),
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Verify),
            readiness: dat_readiness,
            action_label: "Verify games",
            secondary: None,
        },
        HomeCardView {
            card: HomeCard::DuplicateReview,
            icon: crate::ui::icons::CHECK,
            title: "Find duplicate games",
            explanation: "Find identical or equivalent copies in your library, keep one, and move the rest into a recoverable quarantine. Nothing is permanently deleted.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Check),
            readiness: None,
            action_label: "Open duplicate finder",
            secondary: None,
        },
        HomeCardView {
            card: HomeCard::ConvertDiscs,
            icon: crate::ui::icons::GAMES,
            title: "Convert discs",
            explanation: "Convert supported disc images into verified CHD files.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Games),
            readiness: None,
            action_label: "Open disc conversion",
            secondary: None,
        },
        HomeCardView {
            card: HomeCard::Settings,
            icon: crate::ui::icons::SETTINGS,
            title: "Settings",
            explanation: "Choose game folders and preferences. Advanced storage options are available when needed.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Settings),
            readiness: None,
            action_label: "Open Settings",
            secondary: None,
        },
        HomeCardView {
            card: HomeCard::QuickRename,
            icon: crate::ui::icons::CLEAN_UP,
            title: "Quick Rename",
            explanation: "Safely identify and rename games using verified catalogue evidence.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Organise),
            readiness: None,
            action_label: "Choose a library",
            secondary: None,
        },
        HomeCardView {
            card: HomeCard::RomM,
            icon: crate::ui::icons::ROMM,
            title: "Connect RomM",
            explanation: "Connect EmuWiz to your RomM server and browse its records. RomM is treated as a read-only source: nothing in your RomM library is ever changed.",
            technical_detail: None,
            tier: HomeCardTier::Primary,
            accent: Some(HomeAccent::Organise),
            readiness: romm_readiness,
            action_label: "Open RomM",
            secondary: None,
        },
    ];

    cards.insert(
        if inputs.source_folder_count == 0 {
            0
        } else {
            8
        },
        build_library,
    );

    HomeView { banner, cards }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Turns the Doctor scan summary into one readiness for the "Set up
/// emulators" card. The card's action opens the Doctor page, which renders
/// this exact state, so the two can never contradict each other. Only a
/// completed, clean run with at least one check actually performed is ever
/// shown as a pass - a never-run, in-flight, or zero-checks-performed state
/// is `Unknown`, and any warning or blocking finding is `Unavailable`.
fn summarize_setup_checks(summary: SetupCheckSummary) -> CardReadiness {
    match summary {
        SetupCheckSummary::NeverRun => CardReadiness::Unknown("Not checked yet".to_string()),
        SetupCheckSummary::Running => CardReadiness::Unknown("Checking...".to_string()),
        SetupCheckSummary::NoChecksRun => CardReadiness::Unknown("No checks could run".to_string()),
        SetupCheckSummary::NeedsAttention(count) => {
            let verb = if count == 1 { "needs" } else { "need" };
            CardReadiness::Unavailable(format!("{count} check{} {verb} attention", plural(count)))
        }
        SetupCheckSummary::Warnings(count) => {
            CardReadiness::Unavailable(format!("{count} warning{}", plural(count)))
        }
        SetupCheckSummary::Healthy => CardReadiness::Ready("All checks passed".to_string()),
    }
}

/// Draws Home and reports which card (primary or secondary action) was
/// clicked, if any. Draws only what `view` says - no state, no I/O.
pub(crate) fn show_home_page(ui: &mut egui::Ui, view: &HomeView) -> Option<HomeCard> {
    let mut clicked = None;

    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::HOME,
        "Home",
        "What would you like to do?",
    );

    match view.banner {
        HomeBanner::FreshInstall => {
            widgets::banner(
                ui,
                "Welcome to EmuWiz",
                "EmuWiz is not configured yet - that is expected on a fresh install, not an \
                 error. Pick a task below to get started.",
                widgets::StatusTone::Info,
            );
            ui.add_space(theme::SECTION_GAP);
        }
        HomeBanner::ConfigDisappeared => {
            widgets::banner(
                ui,
                "EmuWiz settings could not be found",
                "EmuWiz found your settings earlier, but they are no longer available. Check \
                 the problem before starting another task.",
                widgets::StatusTone::Warning,
            );
            ui.add_space(theme::SECTION_GAP);
        }
        HomeBanner::None => {}
    }

    ui.vertical(|ui| {
        for card in &view.cards {
            let (icon_size, title_size) = match card.tier {
                HomeCardTier::Primary => (26.0, 18.0),
                HomeCardTier::Secondary => (16.0, 15.0),
            };
            let draw = |ui: &mut egui::Ui| {
                ui.horizontal_wrapped(|ui| {
                    let icon = egui::RichText::new(card.icon).size(icon_size);
                    ui.label(match card.accent {
                        Some(accent) => icon.color(accent.color()).strong(),
                        None => icon.color(theme::muted(ui)),
                    });
                    ui.label(egui::RichText::new(card.title).size(title_size).strong());
                    if let Some(readiness) = &card.readiness {
                        widgets::status_badge(ui, readiness.label(), readiness.tone());
                    }
                });
                ui.label(
                    egui::RichText::new(card.explanation)
                        .color(theme::muted(ui))
                        .size(if card.tier == HomeCardTier::Primary {
                            14.5
                        } else {
                            13.0
                        }),
                );
                if let Some(detail) = card.technical_detail {
                    widgets::technical_details(ui, ("home-card", card.card), |ui| {
                        ui.label(detail);
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if widgets::action_button(
                        ui,
                        card.action_label,
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                    {
                        clicked = Some(card.card);
                    }
                    if let Some((secondary_card, secondary_label)) = card.secondary
                        && widgets::action_button(
                            ui,
                            secondary_label,
                            widgets::ActionStyle::Secondary,
                            true,
                        )
                        .clicked()
                    {
                        clicked = Some(secondary_card);
                    }
                });
            };
            if let Some(accent) = card.accent {
                widgets::workflow_card(ui, accent.color(), draw);
            } else {
                widgets::card(ui, draw);
            }
            ui.add_space(theme::SECTION_GAP / 2.0);
        }
    });

    clicked
}

#[cfg(test)]
mod tests;
