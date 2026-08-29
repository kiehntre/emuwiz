//! Advanced View's navigation shell: the grouped sidebar
//! (`docs/GUI_NAVIGATION_RESET_DESIGN.md` §3.2) and the active-entry
//! calculation deciding which destination should render selected.
//! Extracted verbatim from `main.rs` (2026-08-22, GUI extraction Phase B).
//!
//! `MainView`/`ToolsOverlay` themselves, the top-level render dispatch that
//! calls [`show_primary_navigation`], and the app-wide MainView metadata
//! functions physically adjacent to this code in the old `main.rs`
//! (`main_view_title`, `main_view_content_width`, `main_view_uses_page_scroll`,
//! `catalogue_status_load_needed`) all stay in `main.rs` - they are
//! consulted well beyond sidebar rendering (page headers, layout, scroll
//! policy), so moving them here would make this module reach back into
//! app-shell concerns rather than the reverse.

use super::*;

// Consulted only by the navigation/reachability test suite now that the
// 0.8.1 sidebar consolidation stopped rendering this flat list directly
// (see this module's own doc comment). Kept as the single source of truth
// those tests assert against; `--all-targets` clippy's non-test pass does
// not see that usage.
#[allow(dead_code)]
pub(crate) const PRIMARY_NAVIGATION_DESTINATIONS: [(MainView, &str); 17] = [
    (MainView::Home, "Home"),
    (MainView::Mount, "Mount"),
    (MainView::CheatsMods, "Cheats & Mods"),
    (MainView::CheatSources, "Cheat Sources"),
    (MainView::Problems, "Problems & Repair"),
    (MainView::RepairReview, "Repair Review"),
    (MainView::RepairHistory, "Repair History"),
    (MainView::LibraryViewHistory, "Library View History"),
    (MainView::DatSources, "DAT Sources"),
    (MainView::ActiveMounts, "Active Mounts"),
    (MainView::Library, "Library"),
    (MainView::Sources, "Sources"),
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
    QuickRename,
    Overlay(ToolsOverlay),
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

pub(crate) struct NavGroup {
    /// `None` for an entry rendered above every group heading (Home) -
    /// everything else gets a visible section label, directly addressing
    /// finding #1 ("duplicate and confusing navigation labels") by giving
    /// related destinations a group instead of competing in one flat list.
    pub(crate) heading: Option<&'static str>,
    pub(crate) entries: &'static [NavEntry],
}

/// Advanced View's grouped sidebar - `docs/GUI_NAVIGATION_RESET_DESIGN.md`
/// §3.2's approved structure (design direction locked in §9; this constant
/// is Phase 2 of implementing it, the remaining structural work after
/// Gamer View itself, the mode switch, and the bulk-action safety gates
/// were already built). Every entry routes through exactly the same
/// `self.view`/`self.tools_overlay` fields the old flat
/// `PRIMARY_NAVIGATION_DESTINATIONS` list already used - this only changes
/// how they are grouped and labelled for the Advanced View audience, per
/// §3.2's own reasoning ("this also addresses finding #1's duplicate and
/// confusing labels for the advanced audience, by giving related pages a
/// visible group heading instead of a flat list").
///
/// `PRIMARY_NAVIGATION_DESTINATIONS` above is not rendered directly any more -
/// it remains the canonical "which `MainView`s are genuinely reachable
/// primary destinations" registry a
/// number of existing, unrelated tests already assert against (title/width
/// policy coverage, Home-card destination mapping, Library-consolidation
/// counting), and continuing to use it for that keeps all of that
/// coverage intact untouched.
///
/// Placement notes for destinations §3.2's own table doesn't explicitly
/// assign (documented here rather than silently decided, since the locked
/// design predates them):
/// - `CanonicalOrganisation`: not mentioned in §3.2 at all (it predates
///   the QA review this design responds to). Placed under Library - a
///   rename/reorganisation workflow over identified games - so it is no
///   longer reachable only via a single Home card.
/// - `CheatSources`/`DatSources`: config pages for their respective
///   workflows, placed alongside them rather than invented a "Config"
///   group the locked design doesn't have.
/// - `CollectionDiscovery` (a `ToolsOverlay`, not a `MainView`): placed in
///   Sources per the newer UX work's finding that it was "buried in
///   generic Tools" - it now lives where a person is already looking when
///   they scan a source, not only behind the Tools menu (whose own
///   Collection Discovery entry is removed as redundant, see the Tools
///   menu code below).
/// - Doctor's two independent implementations (see Phase 1's finding that
///   they read different data and cannot be merged without losing
///   capability): grouped together under Diagnostics with human-facing
///   labels distinguishing them, not unified.
/// - `RepairReview` sits with Diagnostics (reviewing found problems before
///   deciding to fix them); `RepairHistory` sits with History & Journals
///   (§3.2's own "Rollback / journal detail" child) - it is exactly that.
///
/// # GUI consolidation: one "Problems & Repair" destination
///
/// A later pass (`MainView::Problems`) folded the three separate sidebar
/// rows above (`Doctor`, `RepairReview`, `RepairHistory`) into one visible
/// entry, `nav_view(MainView::Problems, "Problems & Repair")`, so a user no
/// longer has to choose between "Doctor" and "Repair" before knowing which
/// one applies. This changes routing and presentation only - it does not
/// revisit the "cannot be merged without losing capability" finding above:
/// `MainView::Doctor`/`RepairReview`/`RepairHistory` still exist, still
/// render their own genuinely different data through their own unchanged
/// engines, and are still individually reachable (by deep-link, and as
/// tabs/sections inside the one consolidated page - see
/// `problems_repair_page`'s module doc). `ToolsOverlay::DoctorChecks`
/// ("Automatic health report") is deliberately left as its own overlay
/// entry: it is a third, orthogonal mechanism the same Phase 1 finding
/// already decided to keep separate, and this task's own scope is the
/// Doctor-vs-Repair top-level destination choice, not the overlay/page
/// distinction.
///
/// # GUI consolidation: one "Sources" destination
///
/// A later pass folded `DatSources`, `CheatSources`, and Collection
/// Discovery's own sidebar rows above into the single `Sources` entry, over
/// four tabs (`SourcesTab`: Libraries/DATs/Cheats/Discovery - see
/// `sources_tab_for_main_view`/`ArchiveFsApp::show_sources_page`), so a user
/// no longer has to understand why DAT Sources and Cheat Sources were
/// separate sidebar concepts before knowing which one they needed.
/// `MainView::DatSources`/`CheatSources` still exist and still render their
/// own genuinely different data through their own unchanged engines (DAT
/// registry, cheat-source management); Collection Discovery moved from a
/// `ToolsOverlay` to `MainView::SourcesDiscovery` so it could share the same
/// tab chrome (an overlay always replaces the whole central panel, which
/// would have hidden the tab row itself). Every underlying engine -
/// source-folder config, DAT registry, BSFree, cheat providers, collection
/// discovery scanning - is completely untouched; only routing and
/// presentation changed.
pub(crate) const ADVANCED_NAV_GROUPS: &[NavGroup] = &[
    NavGroup {
        heading: None,
        entries: &[nav_view(MainView::Home, "Home")],
    },
    NavGroup {
        heading: Some("LIBRARY"),
        entries: &[
            nav_view(MainView::Library, "Library"),
            nav_quick_rename("Quick Rename"),
            nav_view(MainView::CanonicalOrganisation, "Library Organisation"),
        ],
    },
    NavGroup {
        heading: Some("MOUNT & ACTIVE MOUNTS"),
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
/// (`library_tab_for_main_view(current).is_some()`), not just
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
        library_tab_for_main_view(current).is_some()
    } else if candidate == MainView::Problems {
        problems_repair_tab_for_main_view(current).is_some()
    } else if candidate == MainView::Sources {
        sources_tab_for_main_view(current).is_some()
    } else {
        current == candidate
    }
}
/// Renders Advanced View's grouped sidebar (`ADVANCED_NAV_GROUPS`) and
/// returns whichever entry was clicked, if any. `current_overlay` is only
/// needed to render the selected state of overlay-targeted entries
/// (Collection Discovery, the automatic health report) correctly; it does
/// not gate anything.
pub(crate) fn show_primary_navigation(
    ui: &mut egui::Ui,
    current: MainView,
    current_overlay: ToolsOverlay,
    has_database: bool,
) -> Option<NavClick> {
    let mut clicked = None;
    // The sidebar's own scroll area, independent of the main content's.
    // `ADVANCED_NAV_GROUPS` has grown past a typical laptop-height window
    // (1536x864 and smaller): without this, `SidePanel::left` simply clips
    // whatever does not fit, and every group below the clip line - History &
    // Journals, Diagnostics, Settings - becomes permanently unreachable.
    // `auto_shrink([false, false])` is what makes that clip line the panel's
    // own height rather than the content's: a `ScrollArea` that auto-shrinks
    // to fit its content never needs to scroll in the first place, which
    // would silently reintroduce this exact bug.
    egui::ScrollArea::vertical()
        .id_salt("primary_navigation_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("EmuWiz").size(23.0).strong());
            ui.label(egui::RichText::new("Archive library manager").color(theme::muted(ui)));
            ui.add_space(18.0);
            for group in ADVANCED_NAV_GROUPS {
                if let Some(heading) = group.heading {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(heading)
                            .small()
                            .strong()
                            .color(theme::muted(ui)),
                    );
                }
                for entry in group.entries {
                    let (enabled, selected) = match entry.click {
                        NavClick::View(view) => (
                            navigation_destination_enabled(view, has_database),
                            entry.highlightable
                            // An open overlay renders in place of the main
                            // view's content (see the `CentralPanel` body's
                            // `if self.tools_overlay != ToolsOverlay::None`
                            // branch), so it takes visual precedence: a
                            // `View` entry must never show selected while an
                            // overlay is what the user is actually looking
                            // at, even though `current` (`self.view`) still
                            // holds whatever page was active before the
                            // overlay opened.
                            && current_overlay == ToolsOverlay::None
                            && navigation_destination_selected(current, view),
                        ),
                        NavClick::Overlay(overlay) => {
                            (true, entry.highlightable && current_overlay == overlay)
                        }
                        NavClick::QuickRename => (true, entry.highlightable),
                    };
                    let button = egui::Button::selectable(selected, entry.label)
                        .min_size(egui::vec2(ui.available_width(), 30.0));
                    if ui.add_enabled(enabled, button).clicked() {
                        clicked = Some(entry.click);
                    }
                }
            }
        });
    clicked
}
