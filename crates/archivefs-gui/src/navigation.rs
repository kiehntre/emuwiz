//! Existing route keys and tab projections. `primary` presents seven task
//! destinations without replacing page dispatch or feature-owned state.
//! The legacy route catalogues remain available to advanced tools and
//! reachability tests; they are not the rendered primary sidebar.

use super::*;

pub(crate) mod primary;

pub(crate) fn main_view_for_home_card(card: home_page::HomeCard) -> MainView {
    match card {
        home_page::HomeCard::BuildLibrary | home_page::HomeCard::RomM => MainView::Sources,
        home_page::HomeCard::BrowseGames => MainView::Library,
        home_page::HomeCard::DuplicateReview => MainView::ExactDuplicateReview,
        home_page::HomeCard::ConvertDiscs => MainView::DiscConversion,
        home_page::HomeCard::CheatsAndMods => MainView::CheatsMods,
        home_page::HomeCard::CanonicalOrganisation => MainView::CanonicalOrganisation,
        home_page::HomeCard::QuickRename => MainView::IdentifyRename,
        home_page::HomeCard::CheatSources => MainView::CheatSources,
        home_page::HomeCard::DatSources => MainView::DatSources,
        home_page::HomeCard::CheckSetup => MainView::EmulatorSetup,
        home_page::HomeCard::Settings => MainView::Settings,
        home_page::HomeCard::CheckProblems => MainView::Doctor,
    }
}

pub(crate) fn main_view_for_library_tab(tab: LibraryTab) -> MainView {
    match tab {
        LibraryTab::Archives => MainView::Library,
        LibraryTab::Health => MainView::Health,
        LibraryTab::Duplicates => MainView::Duplicates,
        LibraryTab::Views => MainView::LibraryViews,
        LibraryTab::RecentlyFound => MainView::RecentlyFound,
    }
}

pub(crate) fn library_tab_for_main_view(view: MainView) -> Option<LibraryTab> {
    match view {
        MainView::Library => Some(LibraryTab::Archives),
        MainView::Health => Some(LibraryTab::Health),
        MainView::Duplicates => Some(LibraryTab::Duplicates),
        MainView::LibraryViews => Some(LibraryTab::Views),
        MainView::RecentlyFound => Some(LibraryTab::RecentlyFound),
        _ => None,
    }
}

pub(crate) fn library_tab_label(tab: LibraryTab) -> &'static str {
    match tab {
        LibraryTab::Archives => "Archives",
        LibraryTab::Health => "Health",
        LibraryTab::Duplicates => "Duplicates",
        LibraryTab::Views => "Views",
        LibraryTab::RecentlyFound => "Recently Found",
    }
}

pub(crate) fn main_view_for_problems_repair_tab(tab: ProblemsRepairTab) -> MainView {
    match tab {
        ProblemsRepairTab::Overview => MainView::Problems,
        ProblemsRepairTab::Diagnostics => MainView::Doctor,
        ProblemsRepairTab::Repair => MainView::RepairReview,
    }
}

pub(crate) fn problems_repair_tab_for_main_view(view: MainView) -> Option<ProblemsRepairTab> {
    match view {
        MainView::Problems => Some(ProblemsRepairTab::Overview),
        MainView::Doctor => Some(ProblemsRepairTab::Diagnostics),
        MainView::RepairReview | MainView::RepairHistory => Some(ProblemsRepairTab::Repair),
        _ => None,
    }
}

pub(crate) fn main_view_for_sources_tab(tab: SourcesTab) -> MainView {
    match tab {
        SourcesTab::Libraries => MainView::Sources,
        SourcesTab::Dats => MainView::DatSources,
        SourcesTab::Cheats => MainView::CheatSources,
        SourcesTab::Discovery => MainView::SourcesDiscovery,
    }
}

pub(crate) fn sources_tab_for_main_view(view: MainView) -> Option<SourcesTab> {
    match view {
        MainView::Sources => Some(SourcesTab::Libraries),
        MainView::DatSources => Some(SourcesTab::Dats),
        MainView::CheatSources => Some(SourcesTab::Cheats),
        MainView::SourcesDiscovery => Some(SourcesTab::Discovery),
        _ => None,
    }
}

pub(crate) fn sources_tab_label(tab: SourcesTab) -> &'static str {
    match tab {
        SourcesTab::Libraries => "Libraries",
        SourcesTab::Dats => "DATs",
        SourcesTab::Cheats => "Cheats",
        SourcesTab::Discovery => "Discovery",
    }
}

pub(crate) const TOOLS_MENU_WORKFLOWS: [(&str, &str, MainView); 7] = [
    (
        "Museum",
        "Browse your collection by platform: what EmuWiz knows about each system.",
        MainView::Museum,
    ),
    (
        "Duplicate Finder",
        "Find identical or equivalent copies and quarantine the extras.",
        MainView::ExactDuplicateReview,
    ),
    (
        "Tape Inspector",
        "Inspect supported cassette and tape-image structure without modifying the source.",
        MainView::TapeInspector,
    ),
    (
        "Disc Conversion",
        "Convert supported CUE/BIN disc images to fingerprint-verified CHD.",
        MainView::DiscConversion,
    ),
    (
        "Storage Health",
        "Inspect library space usage and conservative future compression opportunities.",
        MainView::StorageHealth,
    ),
    (
        "Emulator Setup",
        "Read-only check of which emulators EmuWiz can find and their launch readiness.",
        MainView::EmulatorSetup,
    ),
    (
        "Emulator Manager",
        "Read-only inventory of installed emulator versions, channels, and locations.",
        MainView::EmulatorInventory,
    ),
];

pub(crate) const GAMER_MENU_LABEL: &str = "Menu";
pub(crate) const GAMER_MENU_ADD_FOLDER_LABEL: &str = "Add another game folder";
pub(crate) const GAMER_MENU_SCAN_LABEL: &str = "Scan for new games";
pub(crate) const GAMER_MENU_SETUP_LABEL: &str = "Emulator Setup";
pub(crate) const GAMER_MENU_ADVANCED_LABEL: &str = "Advanced View";

pub(crate) fn main_view_title(view: MainView) -> &'static str {
    match view {
        MainView::Home => "Home",
        MainView::CheckGames => "Check My Games",
        MainView::NeedsAttention => "Needs Attention",
        MainView::Library => "Library",
        MainView::ReadyToPlay => "Ready-to-Play",
        MainView::RecentlyFound => "Recently Found",
        MainView::Health => "Health",
        MainView::Duplicates => "Duplicates",
        MainView::Sources => "Sources",
        MainView::SourcesDiscovery => "Collection Discovery",
        MainView::LibraryViews => "Library Views",
        MainView::Mount => "Mount",
        MainView::Selected => "Selected",
        MainView::CheatsMods => "Cheats & Mods",
        MainView::CheatSources => "Cheat Sources",
        MainView::CanonicalOrganisation => "Library organisation",
        MainView::PublisherProfiles => "Publisher / Frontend Library",
        MainView::IdentifyRename => "Identify & Rename",
        MainView::RepairReview => "Repair Review",
        MainView::RepairHistory => "Repair History",
        MainView::ExactDuplicateReview => "Duplicate Finder",
        MainView::DiscConversion => "Converter",
        MainView::StorageHealth => "Storage Health",
        MainView::TapeInspector => "Tape Inspector",
        MainView::EmulatorSetup => "Emulator Setup",
        MainView::EmulatorInventory => "Emulator Manager",
        MainView::BiosProjection => "BIOS / Firmware",
        MainView::Museum => "Museum",
        MainView::LibraryViewHistory => "Library View History",
        MainView::DatSources => "DAT Sources",
        MainView::MediaSets => "Media Sets",
        MainView::ActiveMounts => "Active Mounts",
        MainView::Problems => "Problems & Repair",
        MainView::Doctor => "Doctor",
        MainView::HistoryLogs => "History & Logs",
        MainView::Settings => "Settings",
        MainView::About => "About",
    }
}

pub(crate) fn main_view_content_width(view: MainView) -> ui_layout::ContentWidth {
    match view {
        MainView::Home
        | MainView::NeedsAttention
        | MainView::Mount
        | MainView::Selected
        | MainView::CheatsMods
        | MainView::Library
        | MainView::ReadyToPlay
        | MainView::RecentlyFound
        | MainView::Health
        | MainView::Duplicates
        | MainView::Sources
        | MainView::SourcesDiscovery
        | MainView::LibraryViews
        | MainView::HistoryLogs
        | MainView::RepairHistory
        | MainView::ExactDuplicateReview
        | MainView::LibraryViewHistory => ui_layout::ContentWidth::Wide,
        MainView::Museum => ui_layout::ContentWidth::Wide,
        MainView::CheatSources
        | MainView::CheckGames
        | MainView::CanonicalOrganisation
        | MainView::PublisherProfiles
        | MainView::IdentifyRename
        | MainView::RepairReview
        | MainView::DiscConversion
        | MainView::StorageHealth
        | MainView::TapeInspector
        | MainView::EmulatorSetup
        | MainView::EmulatorInventory
        | MainView::BiosProjection
        | MainView::DatSources
        | MainView::MediaSets
        | MainView::Doctor
        | MainView::Settings
        | MainView::About
        | MainView::ActiveMounts => ui_layout::ContentWidth::Normal,
        MainView::Problems => ui_layout::ContentWidth::Wide,
    }
}

pub(crate) fn main_view_uses_page_scroll(view: MainView) -> bool {
    matches!(
        view,
        MainView::Home
            | MainView::CheckGames
            | MainView::NeedsAttention
            | MainView::Selected
            | MainView::Sources
            | MainView::SourcesDiscovery
            | MainView::CheatSources
            | MainView::DatSources
            | MainView::MediaSets
            | MainView::IdentifyRename
            | MainView::Problems
            | MainView::Doctor
            | MainView::EmulatorSetup
            | MainView::DiscConversion
            | MainView::StorageHealth
            | MainView::TapeInspector
            | MainView::HistoryLogs
            | MainView::Settings
            | MainView::About
            | MainView::RepairHistory
            | MainView::ExactDuplicateReview
            | MainView::LibraryViewHistory
            | MainView::ReadyToPlay
            | MainView::CanonicalOrganisation
            | MainView::PublisherProfiles
            | MainView::BiosProjection
    )
}

// Consulted only by the navigation/reachability test suite now that the
// 0.8.1 sidebar consolidation stopped rendering this flat list directly
// (see this module's own doc comment). Kept as the single source of truth
// those tests assert against; `--all-targets` clippy's non-test pass does
// not see that usage.
#[allow(dead_code)]
pub(crate) const PRIMARY_NAVIGATION_DESTINATIONS: [(MainView, &str); 26] = [
    (MainView::Home, "Home"),
    (MainView::NeedsAttention, "Needs Attention"),
    (MainView::Mount, "Mount"),
    (MainView::CheatsMods, "Cheats & Mods"),
    (MainView::CheatSources, "Cheat Sources"),
    (MainView::Problems, "Problems & Repair"),
    (MainView::RepairReview, "Repair Review"),
    (MainView::RepairHistory, "Repair History"),
    (MainView::ExactDuplicateReview, "Duplicate Finder"),
    (MainView::DiscConversion, "Disc Conversion"),
    (MainView::EmulatorSetup, "Emulator Setup"),
    (MainView::EmulatorInventory, "Emulator Manager"),
    (MainView::BiosProjection, "BIOS / Firmware"),
    (MainView::LibraryViewHistory, "Library View History"),
    (MainView::DatSources, "DAT Sources"),
    (MainView::MediaSets, "Media Sets"),
    (MainView::ActiveMounts, "Active Mounts"),
    (MainView::Library, "Library"),
    (MainView::ReadyToPlay, "Ready-to-Play"),
    (MainView::Sources, "Sources"),
    (MainView::PublisherProfiles, "Publisher / Frontend Library"),
    (MainView::SourcesDiscovery, "Collection Discovery"),
    (MainView::Doctor, "Doctor"),
    (MainView::HistoryLogs, "History & Logs"),
    (MainView::Settings, "Settings"),
    (MainView::About, "About"),
];

/// Where clicking a grouped Advanced View sidebar entry
/// (`ADVANCED_NAV_GROUPS`) leads - either a `MainView` page or a
/// `ToolsOverlay` panel, the two routing mechanisms that already exist
/// and are otherwise unchanged. This exists so the grouped sidebar can
/// present both kinds of destination side by side (for example Collection
/// Discovery, a `ToolsOverlay`, sitting naturally in the Sources group
/// next to `MainView::Sources`) without inventing a third, competing
/// routing concept - every entry still ultimately sets one of the two
/// fields `ArchiveFsApp` already has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavClick {
    View(MainView),
    Enhancement(crate::cheats_mods_preview::EnhancementSection),
    QuickRename,
    Overlay(ToolsOverlay),
    /// The RomM provider workflow. Has no `MainView` of its own - it is the
    /// RomM source card on Sources -> Libraries - so it routes through
    /// `ArchiveFsApp::navigate_to_sources_tab(SourcesTab::Libraries)`, the
    /// exact same call Home's "Connect RomM" card and the Sources top-menu
    /// "RomM" item make. Not highlightable (the "Sources" entry owns that
    /// destination's selected state).
    Romm,
}

#[derive(Clone, Copy)]
pub(crate) struct NavEntry {
    pub(crate) click: NavClick,
    pub(crate) label: &'static str,
    /// Whether this entry may render as "selected" when its destination is
    /// active. Almost always `true`; `false` only for a second entry that
    /// routes to a destination another entry already owns for highlighting
    /// purposes - without this, two entries would
    /// highlight simultaneously and look like a bug, since the sidebar has
    /// no way to tell "the same page, reached two ways" from "two different
    /// pages that happen to both be active".
    pub(crate) highlightable: bool,
}

pub(crate) const fn nav_view(view: MainView, label: &'static str) -> NavEntry {
    NavEntry {
        click: NavClick::View(view),
        label,
        highlightable: true,
    }
}

pub(crate) const fn nav_overlay(overlay: ToolsOverlay, label: &'static str) -> NavEntry {
    NavEntry {
        click: NavClick::Overlay(overlay),
        label,
        highlightable: true,
    }
}

pub(crate) const fn nav_quick_rename(label: &'static str) -> NavEntry {
    NavEntry {
        click: NavClick::QuickRename,
        label,
        highlightable: true,
    }
}

pub(crate) const fn nav_romm(label: &'static str) -> NavEntry {
    NavEntry {
        click: NavClick::Romm,
        label,
        highlightable: false,
    }
}

pub(crate) struct NavGroup {
    /// `None` for an entry rendered above every group heading (Home) -
    /// everything else gets a visible section label, directly addressing
    /// finding #1 ("duplicate and confusing navigation labels") by giving
    /// related destinations a group instead of competing in one flat list.
    pub(crate) heading: Option<&'static str>,
    pub(crate) entries: &'static [NavEntry],
}

/// Compatibility catalogue of the original grouped routes. This is NOT the
/// primary sidebar: `primary::PRIMARY` is its sole rendered source. Keep the
/// original route/label catalogue for advanced tools and migration reachability
/// checks; page dispatch and the existing Library/Sources/Problems tabs survive.
pub(crate) const ADVANCED_NAV_GROUPS: &[NavGroup] = &[
    NavGroup {
        heading: None,
        entries: &[
            nav_view(MainView::Home, "Home"),
            nav_view(MainView::NeedsAttention, "Needs Attention"),
        ],
    },
    NavGroup {
        heading: Some("LIBRARY"),
        entries: &[
            nav_view(MainView::Library, "Library"),
            nav_view(MainView::ReadyToPlay, "Ready-to-Play"),
            nav_quick_rename("Quick Rename"),
            nav_view(MainView::CanonicalOrganisation, "Library Organisation"),
            nav_view(MainView::PublisherProfiles, "Publisher / Frontend Library"),
        ],
    },
    // 0.8.1 "core workflows directly discoverable": the major task-oriented
    // workflows get their own visible sidebar group instead of being
    // reachable only from Home or buried under Problems & Repair. Duplicate
    // Finder and Disc Conversion are first-class `MainView`s now; Emulator
    // Setup is the dedicated Doctor-readiness destination; RomM routes to the
    // RomM provider card on Sources -> Libraries (same call as Home's
    // "Connect RomM" card - see `NavClick::Romm`).
    NavGroup {
        heading: Some("TOOLS & WORKFLOWS"),
        entries: &[
            nav_view(MainView::ExactDuplicateReview, "Duplicate Finder"),
            nav_view(MainView::DiscConversion, "Disc Conversion"),
            nav_view(MainView::EmulatorSetup, "Emulator Setup"),
            nav_view(MainView::EmulatorInventory, "Emulator Manager"),
            nav_view(MainView::BiosProjection, "BIOS / Firmware"),
            nav_romm("RomM"),
        ],
    },
    NavGroup {
        heading: Some("MOUNTS"),
        entries: &[
            nav_view(MainView::Mount, "Mounts"),
            nav_view(MainView::ActiveMounts, "Active mounts"),
        ],
    },
    NavGroup {
        heading: Some("CHEATS & MODS"),
        entries: &[nav_view(MainView::CheatsMods, "Cheats & Mods")],
    },
    NavGroup {
        heading: Some("SOURCES"),
        entries: &[nav_view(MainView::Sources, "Sources")],
    },
    NavGroup {
        heading: Some("MEDIA"),
        entries: &[nav_view(MainView::MediaSets, "Media Sets")],
    },
    NavGroup {
        heading: Some("HISTORY & JOURNALS"),
        entries: &[
            nav_view(MainView::HistoryLogs, "History & Logs"),
            nav_view(MainView::LibraryViewHistory, "Library View History"),
        ],
    },
    NavGroup {
        heading: Some("DIAGNOSTICS"),
        entries: &[
            nav_view(MainView::Problems, "Problems & Repair"),
            nav_overlay(ToolsOverlay::DoctorChecks, "Automatic health report"),
        ],
    },
    NavGroup {
        heading: Some("SETTINGS"),
        entries: &[nav_view(MainView::Settings, "Settings")],
    },
];
/// Whether `view`'s sidebar button (if it has one) should be clickable
/// given `has_database`. Only ever called with `PRIMARY_NAVIGATION_
/// DESTINATIONS` entries (`show_primary_navigation`'s loop, and its test
/// mirror), which no longer includes `MainView::Health`/`Duplicates` -
/// those two arms are unreachable through any live sidebar call site
/// today, but deliberately left in rather than pruned: the same
/// database-readiness gate would be the correct one to apply if the
/// unified Library shell's tab row ever needs to grey out the
/// Health/Duplicates tabs before a scan completes (their content bodies
/// already show a "Scan the library..." fallback instead, which was
/// judged sufficient for now - see docs/GUI_SIMPLIFICATION.md). Kept
/// correct and ready rather than deleted and potentially reinvented.
pub(crate) fn navigation_destination_enabled(view: MainView, has_database: bool) -> bool {
    !matches!(view, MainView::Health | MainView::Duplicates) || has_database
}

/// Whether the sidebar button for `candidate` should render as selected
/// given the currently active `current` view. Ordinary destinations use
/// exact equality; `MainView::Library`'s button is the sole sidebar entry
/// point into the unified Library shell, so it renders selected whenever
/// `current` is *any* of the five Library-related destinations
/// (`current == MainView::Library`), not just
/// `MainView::Library` itself - otherwise the sidebar would show no
/// selected destination at all while on the Health, Duplicates, or Views
/// tab.
///
/// `MainView::Problems` follows the identical rule for the consolidated
/// "Problems & Repair" destination: it renders selected while `current` is
/// `Problems` itself or any of the destinations its own tabs cover
/// (`Doctor`, `RepairReview`, `RepairHistory` - see
/// `problems_repair_tab_for_main_view`), so a deep-link that lands directly
/// on, say, `MainView::RepairReview` still shows the one sidebar button
/// selected rather than none.
///
/// `MainView::Sources` follows the identical rule: it renders selected
/// while `current` is `Sources` itself or any of `DatSources`/
/// `CheatSources`/`SourcesDiscovery` (see `sources_tab_for_main_view`).
pub(crate) fn navigation_destination_selected(current: MainView, candidate: MainView) -> bool {
    if candidate == MainView::Library {
        current == MainView::Library
    } else if candidate == MainView::Problems {
        problems_repair_tab_for_main_view(current).is_some()
    } else if candidate == MainView::Sources {
        sources_tab_for_main_view(current).is_some()
    } else {
        current == candidate
    }
}
/// The seven task destinations are the only primary sidebar rows.
pub(crate) fn show_primary_navigation(
    ui: &mut egui::Ui,
    current: MainView,
    current_overlay: ToolsOverlay,
    _has_database: bool,
) -> Option<NavClick> {
    primary::show_sidebar(ui, current, current_overlay)
}

/// Every top-level destination the app can show. `Health`, `Duplicates`,
/// and `LibraryViews` are **compatibility dispatch keys**, not separate
/// sidebar destinations any more (see `LibraryTab`): they exist purely so
/// `self.view` (still the single source of truth for what actually
/// renders) can name which Library tab is active without a second,
/// parallel field. Each maps 1:1 to a `LibraryTab` via
/// `library_tab_for_main_view`/`main_view_for_library_tab`.
///
/// Kept as real enum variants (Library IA migration Phase 3 decision,
/// evidence in docs/GUI_SIMPLIFICATION.md's "Library IA migration -
/// Phase 3" section) rather than removed and replaced with `LibraryTab`
/// alone: production code still keys the shell's content dispatch off
/// `self.view` matching them (`library_tab_for_main_view`,
/// `main_view_title`, `main_view_content_width`,
/// `main_view_uses_page_scroll` all still need an exhaustive `MainView`
/// match), and 50+ existing tests across three milestones construct or
/// compare against these three variants directly. No persisted,
/// external, or CLI state depends on them - the only reasons to keep
/// them are internal (production dispatch + test surface), not
/// backward-compatibility with anything outside this process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum MainView {
    #[default]
    Home,
    CheckGames,
    NeedsAttention,
    Library,
    /// Read-only view over already-gathered Ready-to-Play projections.
    ReadyToPlay,
    RecentlyFound,
    Health,
    Duplicates,
    Sources,
    /// Collection Discovery's content, dispatched as the "Discovery" tab of
    /// the consolidated Sources destination - see `sources_tab_for_main_view`.
    /// Was previously `ToolsOverlay::CollectionDiscovery`, a completely
    /// separate rendering mechanism reached only from its own now-removed
    /// sidebar row; folding it into `MainView` lets it share Sources' tab
    /// chrome like `DatSources`/`CheatSources` already do. The underlying
    /// renderer (`collection_discovery_page::show_collection_discovery_panel`)
    /// is unchanged.
    SourcesDiscovery,
    LibraryViews,
    Mount,
    Selected,
    CheatsMods,
    /// The registered cheat sources: which are consulted, in what order, and
    /// for which platforms. Its own destination rather than a section of
    /// Cheats & Mods, because it is configuration that outlives any one
    /// archive being worked on.
    CheatSources,
    /// Canonical organisation: planning and (only after explicit approval)
    /// applying moves of identified games into a configured master ROM root.
    CanonicalOrganisation,
    /// Read-only frontend publisher projections over the existing Playing
    /// Library / 1G1R plan. This destination has no execution path.
    PublisherProfiles,
    /// Evidence-backed filename cleanup for one chosen library folder. This
    /// is a task-oriented entry point over the existing DAT audit, rename
    /// plan, review, and journalled apply flow; DAT Sources remains the
    /// advanced catalogue-management page.
    IdentifyRename,
    /// Repair Review: preview-only review of a saved whole-library repair
    /// plan. Loads a `LibraryRepairPlan` produced by the CLI's
    /// `repair scan --plan-out` contract and shows its proposals. Nothing is
    /// applied from this page.
    RepairReview,
    /// Repair History: recent rename transactions journaled through the
    /// Repair Center (and any other flow sharing the same journal
    /// directory), with reverify status and safe undo when the core proves
    /// a transaction is reversible.
    //
    // Still routed and rendered (every `MainView` match handles it) and
    // exercised by the navigation tests, but since the 0.8.1 consolidation
    // it is reached as a tab within Problems & Repair rather than assigned
    // as a top-level `view`, so production code no longer constructs it
    // directly.
    #[allow(dead_code)]
    RepairHistory,
    /// Duplicate Finder: a DAT-independent duplicate/equivalent-content scan
    /// (`archivefs_core::repair::exact_duplicate`, plus the N64 and optical
    /// equivalent scanners) with evidence-backed canonical-copy selection and
    /// multi-file (CUE/GDI/M3U) protection, quarantined through the same
    /// transaction/journal/rollback engine every other repair flow already
    /// uses.
    ///
    /// Since 0.8.1's "core workflows directly discoverable" pass this is a
    /// first-class destination with its own sidebar and top-menu entry
    /// ("Duplicate Finder") - it is no longer routed through
    /// `ProblemsRepairTab::Repair` (`problems_repair_tab_for_main_view` no
    /// longer maps it), so arriving here never shows Repair Review / Repair
    /// History framing. Deliberately a separate destination from
    /// `MainView::Duplicates` (a read-only Library-tab duplicate viewer over
    /// a different, DAT-relative notion of "duplicate") - the two are
    /// unrelated and never share state.
    ExactDuplicateReview,
    /// Disc Conversion: verified CUE/BIN -> CHD conversion
    /// (`optical_conversion_page` over `archivefs_core::repair`'s
    /// `build_chd_conversion_plan` / `execute_chd_conversion` /
    /// `rollback_chd_conversion`). A first-class destination with its own
    /// sidebar and top-menu entry - the user never has to conceptually enter
    /// "Repair" to convert a disc image. Reuses the exact same
    /// `OpticalConversionPageState` and backend the Repair tab used before.
    DiscConversion,
    /// Read-only catalogue-backed storage usage and future conversion analysis.
    StorageHealth,
    /// Read-only presentation of bounded tape analysis for the selected game.
    TapeInspector,
    /// Emulator Setup: the read-only emulator readiness / profile check.
    /// Renders `doctor_page::show_doctor_page` over the shared
    /// `ArchiveFsApp::doctor_scan` - the same engine and state the Problems &
    /// Repair -> Diagnostics tab uses (no second scan, no divergent state) -
    /// but presented as a dedicated, clearly-named destination so emulator
    /// setup is discoverable without going through "Problems & Repair". The
    /// Doctor scan's "Emulators" and "Emulator profiles" categories carry the
    /// per-emulator rows.
    EmulatorSetup,
    /// Read-only inventory of installed emulator versions and channels.
    EmulatorInventory,
    /// Read-only master BIOS inventory and emulator projection planner.
    BiosProjection,
    /// Curated, read-only collection view backed by the loaded catalogue and
    /// existing evidence/artwork state.
    Museum,
    /// Library View History: a read-only view of the durable, append-only
    /// Library View apply/remove history
    /// (`archivefs_core::library_view_history`), re-read from disk on every
    /// visit/refresh. Deliberately distinct from `HistoryLogs`, which shows
    /// the in-memory `OperationHistory` recent-activity log that does not
    /// survive a restart - this page never touches that log.
    LibraryViewHistory,
    /// The registered DAT catalogues: which local DAT files and folders
    /// EmuWiz can check a library against. Its own destination for the
    /// same reason Cheat Sources is: it is configuration that outlives any
    /// one archive being worked on.
    DatSources,
    /// Read-only presentation of the existing media-set topology and swap
    /// plan. This page derives only from the loaded catalogue snapshot.
    MediaSets,
    ActiveMounts,
    /// The consolidated "Problems & Repair" destination: one sidebar entry
    /// over Overview/Diagnostics/Repair tabs - see `problems_repair_page`'s
    /// module doc. `Doctor`/`RepairReview`/`RepairHistory` below remain the
    /// actual rendering destinations each tab lands on (their own engines
    /// are untouched); `Problems` itself renders only the Overview tab and
    /// the shared tab chrome. `problems_repair_tab_for_main_view` is the
    /// `LibraryTab`-style projection tying all four together.
    Problems,
    Doctor,
    HistoryLogs,
    Settings,
    About,
}

/// The five lenses onto Library data, now visibly unified as tabs of one
/// Library page (see docs/GUI_SIMPLIFICATION.md's "Library IA migration"
/// section) even though each still has its own `MainView` variant and
/// render function underneath, retained for compatibility - see
/// `ArchiveFsApp::update`'s central-panel dispatch, where all five are
/// rendered from one block instead of five separate ones. `Archives`
/// corresponds to `MainView::Library` (the archive table); `Views`
/// corresponds to `MainView::LibraryViews` (saved library views) - named
/// differently from its `MainView` variant because "Library Views" would
/// read twice as "Library" now that it is a tab labelled "Library".
///
/// # Synchronization rule
///
/// `ArchiveFsApp::view` (`MainView`) remains the single source of truth
/// for which underlying render function actually runs - unchanged.
/// `ArchiveFsApp::library_tab` (`LibraryTab`) is a *derived* projection of
/// it: once per frame, before anything renders,
/// `library_tab_for_main_view(self.view)` is consulted, and if `self.view`
/// is one of the five Library-related destinations, `self.library_tab` is
/// set to match. If `self.view` is anything else (Mount, Settings, ...),
/// `self.library_tab` is left untouched, so it keeps remembering the last
/// Library tab visited. The unified Library shell then reads
/// `self.library_tab` to decide which tab's content to render.
///
/// This makes every existing way of navigating to a Library destination -
/// the sidebar's single "Library" button, or any of the ~11 scattered
/// `self.view = MainView::X` assignments elsewhere in the app - a correct
/// "legacy route" into the right `LibraryTab` automatically, with no call
/// site needing to know `LibraryTab` exists. The only sanctioned way to
/// write `library_tab` going the other direction (choosing a tab and
/// having `view` follow) is `ArchiveFsApp::navigate_to_library_tab`,
/// which the shell's `tab_row` calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum LibraryTab {
    #[default]
    Archives,
    Health,
    Duplicates,
    Views,
    RecentlyFound,
}

/// The three tabs of the consolidated "Problems & Repair" destination -
/// see `MainView::Problems`'s doc comment and `problems_repair_page`'s
/// module doc. Mirrors `LibraryTab` exactly: `ArchiveFsApp::view` remains
/// the single source of truth for which underlying render function runs;
/// `ArchiveFsApp::problems_repair_tab` is a *derived* projection of it via
/// `problems_repair_tab_for_main_view`, reconciled once per frame
/// (`reconcile_problems_repair_tab`) exactly like `reconcile_library_tab`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum ProblemsRepairTab {
    #[default]
    Overview,
    Diagnostics,
    Repair,
}

/// The four tabs of the consolidated "Sources" destination - see
/// `MainView::Sources`'s sibling variants below and `sources_page`'s module
/// doc. Mirrors `LibraryTab`/`ProblemsRepairTab` exactly: `ArchiveFsApp::view`
/// remains the single source of truth; `ArchiveFsApp::sources_tab` is a
/// *derived* projection of it via `sources_tab_for_main_view`, reconciled
/// once per frame (`reconcile_sources_tab`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum SourcesTab {
    #[default]
    Libraries,
    Dats,
    Cheats,
    Discovery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ToolsOverlay {
    #[default]
    None,
    /// A navigation-only wrapper around the existing PCSX2 Save Vault panel.
    SaveVault,
    Diagnostics,
    PlatformAliases,
    DatabaseStatus,
    DoctorChecks,
    ArchiveInspector,
    /// First-run onboarding (`onboarding.rs`): a thin step tracker that
    /// takes over the central panel exactly like `Diagnostics` does, but
    /// dispatches its body per-step to the real Sources/DAT Sources/
    /// Emulator Setup page methods rather than one fixed renderer.
    Onboarding,
}

/// The unified Library shell's chrome: the shared "Library" heading and
/// the five-tab selector, rendered identically regardless of which tab is
/// selected. Content dispatch (`match self.library_tab { ... }`) stays in
/// `ArchiveFsApp::update`'s central-panel closure, since each arm needs
/// direct `&mut self` field access the existing per-page renderers
/// already require (`self.health_duplicate_ui.health_filters`, `self.health_duplicate_ui.duplicate_filters`,
/// `self.library_views`, ...) - bundling all of that into this function's
/// parameters would mean exactly the giant parameter-heavy universal
/// renderer this milestone was asked to avoid. Broken out on its own so
/// the chrome itself - which tabs render, in which order, with which
/// labels, and that a click returns the right `LibraryTab` - is directly
/// testable without going through a full `eframe::App::update` call.
pub(crate) fn show_library_shell_header(
    ui: &mut egui::Ui,
    current_tab: LibraryTab,
) -> Option<LibraryTab> {
    show_library_shell_header_with_actions(ui, current_tab, |_| {})
}

pub(crate) fn show_library_shell_header_with_actions(
    ui: &mut egui::Ui,
    current_tab: LibraryTab,
    actions: impl FnOnce(&mut egui::Ui),
) -> Option<LibraryTab> {
    ui.horizontal_top(|ui| {
        let width = (ui.available_width() - 165.0).max(120.0);
        ui.allocate_ui_with_layout(egui::vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            widgets::workflow_header(ui, "My Games", "Browse and manage your games.");
        });
        actions(ui);
    });
    let tab_options: [(LibraryTab, &str); 5] = [
        (
            LibraryTab::Archives,
            library_tab_label(LibraryTab::Archives),
        ),
        (LibraryTab::Health, library_tab_label(LibraryTab::Health)),
        (
            LibraryTab::Duplicates,
            library_tab_label(LibraryTab::Duplicates),
        ),
        (LibraryTab::Views, library_tab_label(LibraryTab::Views)),
        (
            LibraryTab::RecentlyFound,
            library_tab_label(LibraryTab::RecentlyFound),
        ),
    ];
    let clicked = widgets::tab_row(ui, &tab_options, current_tab);
    ui.add_space(8.0);
    clicked
}
