//! EmuWiz Blue shell: presentation only.
//!
//! This module owns three things and nothing else:
//!
//! 1. the primary destinations and how each maps onto an *existing*
//!    [`Route`] - no route, section or deep link is added, renamed or removed;
//! 2. the capability map: every control the Blue plan shows, classified by
//!    whether a backend capability exists for it today;
//! 3. the Blue style profile and the sidebar.
//!
//! It dispatches no backend command, reads no file and changes no page.
//! EmuWiz Blue is the standard view for new and existing installations
//! ([`blue_enabled`]); **All Tools** restores the complete existing
//! navigation at any time. The Classic view stays available from Settings
//! while the move to Blue is finished; with it chosen nothing here runs and
//! the interface is exactly the previous one.

use std::sync::Arc;

use eframe::egui;

use super::routes::{Route, Section};

// --------------------------------------------------------------------- state

pub(super) const UI_SCALE_MIN: f32 = 0.8;
pub(super) const UI_SCALE_MAX: f32 = 1.6;
pub(super) const UI_SCALE_STEP: f32 = 0.1;

/// Presentation state. `enabled` and `ui_scale` are saved preferences;
/// `all_tools` (the All Tools browser is open) is per session and never
/// persisted, so a restart always opens the current location itself.
///
/// This type owns the context style while Simple is on. It remembers the
/// complete classic style it replaced and restores that exact value when
/// Simple is turned off, and it re-asserts the blue profile whenever anything
/// else restyles the context (the embedded workflow host calls
/// `readable_style` when it is first built).
#[derive(Clone, Debug)]
pub(super) struct ShellState {
    pub enabled: bool,
    pub all_tools: bool,
    /// The All Tools browser's search and filter (session only).
    pub tools: super::all_tools::AllToolsState,
    pub ui_scale: f32,
    /// The complete style in force when Simple was turned on.
    classic: Option<Arc<egui::Style>>,
    /// The blue style this type installed, as the context holds it. A
    /// different pointer in the context means something else restyled it.
    blue: Option<Arc<egui::Style>>,
    /// The zoom last requested, so it is only set when it changes.
    zoom: Option<f32>,
}

impl Default for ShellState {
    /// EmuWiz Blue is the standard view, from the very first frame.
    fn default() -> Self {
        Self {
            enabled: true,
            all_tools: false,
            tools: Default::default(),
            ui_scale: 1.0,
            classic: None,
            blue: None,
            zoom: None,
        }
    }
}

impl ShellState {
    /// The Classic fallback view, as an explicit choice.
    pub fn classic_view() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// The Blue chrome (eight-row sidebar, search header) is shown whenever
    /// Blue is on, including while the All Tools browser is open.
    pub fn simple_navigation(&self) -> bool {
        self.enabled
    }

    /// The All Tools browser is open over the current location.
    pub fn all_tools_open(&self) -> bool {
        self.enabled && self.all_tools
    }

    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = clamp_scale(scale);
    }

    /// Ctrl+Plus, Ctrl+Equals, Ctrl+Minus and Ctrl+0 change the saved
    /// `ui_scale`, within the same bounds as the Settings buttons, and never
    /// egui's own unsaved zoom. Returns true when the saved value changed.
    pub fn apply_zoom_shortcuts(&mut self, context: &egui::Context) -> bool {
        // egui would otherwise zoom by its own step and ignore our bounds.
        context.options_mut(|options| options.zoom_with_keyboard = false);
        let action = context.input_mut(|input| {
            let command = egui::Modifiers::COMMAND;
            if input.consume_key(command, egui::Key::Plus)
                || input.consume_key(command, egui::Key::Equals)
            {
                Some(self.ui_scale + UI_SCALE_STEP)
            } else if input.consume_key(command, egui::Key::Minus) {
                Some(self.ui_scale - UI_SCALE_STEP)
            } else if input.consume_key(command, egui::Key::Num0) {
                Some(1.0)
            } else {
                None
            }
        });
        let Some(target) = action else {
            return false;
        };
        let before = self.ui_scale;
        self.set_ui_scale(target);
        (self.ui_scale - before).abs() > f32::EPSILON
    }

    /// Keeps the context style and zoom in step with the preference. Cheap
    /// when nothing changed: one pointer comparison per frame.
    ///
    /// With Simple never turned on and the scale at 100% this changes
    /// nothing, so the default interface is exactly the host's.
    pub fn sync_style(&mut self, context: &egui::Context) {
        if self.enabled {
            let current = context.style();
            let ours = self
                .blue
                .as_ref()
                .is_some_and(|blue| Arc::ptr_eq(blue, &current));
            if !ours {
                // First frame after turning on: `current` is the classic
                // style. Later: something restyled the context under us, and
                // the classic style already remembered is still the truth.
                let classic = self.classic.get_or_insert(current).clone();
                let mut style = (*classic).clone();
                blue_style(&mut style);
                context.set_style(style);
                self.blue = Some(context.style());
            }
            self.set_zoom(context, self.ui_scale);
        } else {
            if let Some(classic) = self.classic.take() {
                // Every slot the blue profile touched, restored at once.
                context.set_style(classic);
                self.blue = None;
            }
            // Classic honours the same saved scale. At 100% this is the
            // host's own zoom, so the default interface is unchanged.
            self.set_zoom(context, self.ui_scale);
        }
    }

    /// Draws the Blue focus ring (3 px accent, 3 px gap) around whichever
    /// control has keyboard focus. egui's own widgets have no ring of their
    /// own; custom-painted ones that already drew one through
    /// `theme::paint_focus_ring` are skipped. Call once, after the page.
    pub fn paint_focus_ring(&self, context: &egui::Context) {
        if !self.enabled {
            return;
        }
        let Some(id) = context.memory(|memory| memory.focused()) else {
            return;
        };
        if crate::ui::theme::focus_ring_painted(context, id) {
            return;
        }
        let Some(response) = context.read_response(id) else {
            return;
        };
        // Scrolled out of view: nothing to ring.
        if !response.interact_rect.is_positive() {
            return;
        }
        let reach =
            crate::ui::theme::blue::FOCUS_RING_GAP + crate::ui::theme::blue::FOCUS_RING_WIDTH;
        let painter = context
            .layer_painter(response.layer_id)
            .with_clip_rect(response.interact_rect.expand(reach));
        let radius = f32::from(CORNER_RADIUS);
        crate::ui::theme::paint_blue_focus_ring(&painter, response.rect, radius);
    }

    fn set_zoom(&mut self, context: &egui::Context, zoom: f32) {
        if self.zoom != Some(zoom) {
            context.set_zoom_factor(zoom);
            self.zoom = Some(zoom);
        }
    }
}

/// Which view saved preferences open: EmuWiz Blue unless the person
/// explicitly chose the Classic view. The old opt-in `simple_shell` flag is
/// deliberately ignored (see `Preferences::simple_shell`).
pub(super) fn blue_enabled(preferences: &super::backend::Preferences) -> bool {
    !preferences.classic_view
}

pub(super) fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        // Whole steps only, so a saved value never drifts.
        ((scale / UI_SCALE_STEP).round() * UI_SCALE_STEP).clamp(UI_SCALE_MIN, UI_SCALE_MAX)
    } else {
        1.0
    }
}

// -------------------------------------------------------------- destinations

/// The eight approved primary destinations (decision D1), in sidebar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Destination {
    Home,
    Library,
    Setup,
    OrganiseExport,
    Tools,
    Enhance,
    HealthRecovery,
    Settings,
}

pub(super) const DESTINATIONS: [Destination; 8] = [
    Destination::Home,
    Destination::Library,
    Destination::Setup,
    Destination::OrganiseExport,
    Destination::Tools,
    Destination::Enhance,
    Destination::HealthRecovery,
    Destination::Settings,
];

impl Destination {
    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Library => "Library",
            Self::Setup => "Setup",
            Self::OrganiseExport => "Organise & Export",
            Self::Tools => "Tools",
            Self::Enhance => "Enhance",
            Self::HealthRecovery => "Health & Recovery",
            Self::Settings => "Settings",
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::Home => "Pick up where you left off.",
            Self::Library => "Browse, check and open your games.",
            Self::Setup => "Game folders, emulators and firmware.",
            Self::OrganiseExport => "Organise, rename and export your collection.",
            Self::Tools => "Every tool, grouped by what you want to get done.",
            Self::Enhance => "Cheats, mods, saves and artwork for your games.",
            Self::HealthRecovery => "Problems, repairs, history and undo.",
            Self::Settings => "Appearance and everyday preferences.",
        }
    }

    /// The existing route this destination opens. `None` for Tools, which
    /// opens the All Tools browser over the current location.
    pub fn route(self) -> Option<Route> {
        Some(match self {
            Self::Home => Route::Home,
            Self::Library => Route::BrowsePlay,
            Self::Setup => Route::Section(Section::Setup),
            // The existing Organisation-family location. In the Blue view it
            // is painted as the Manage Library hub (cards onto existing
            // routes); the Classic view shows the Organisation overview there.
            Self::OrganiseExport => Route::Section(Section::OrganisationFamily),
            Self::Tools => return None,
            Self::Enhance => Route::Section(Section::Mods),
            Self::HealthRecovery => Route::Section(Section::Problems),
            Self::Settings => Route::Section(Section::Settings),
        })
    }
}

/// Which destination a location belongs to, following the approved boards.
/// `None` only for Activity, which is reached from the status bar ("View
/// Activity") and highlights no row, as on its boards.
pub(super) fn destination_for(route: &Route) -> Option<Destination> {
    match route {
        Route::BrowsePlay
        | Route::BrowsePlayGame(_)
        | Route::Game(_)
        | Route::ReviewIdentity(_)
        | Route::PlatformCheck(_)
        | Route::LibraryFiles => return Some(Destination::Library),
        Route::QuickRename | Route::MameWorkflow => return Some(Destination::OrganiseExport),
        _ => {}
    }
    Some(match route.section() {
        Section::Home => Destination::Home,
        Section::Games
        | Section::Launch
        | Section::Duplicates
        | Section::MultiDisc
        | Section::Romm
        | Section::Check
        | Section::DatVerification => Destination::Library,
        Section::Setup
        | Section::Emulators
        | Section::EmulatorsFamily
        | Section::Firmware
        | Section::Sources
        | Section::SourcesProviders => Destination::Setup,
        Section::OrganisationFamily | Section::Build | Section::Dat | Section::Mame => {
            Destination::OrganiseExport
        }
        Section::Converter
        | Section::Conversion
        | Section::Tape
        | Section::Museum
        | Section::Platforms
        | Section::Storage => Destination::Tools,
        Section::Mods
        | Section::CheatsMods
        | Section::Saves
        | Section::SavesStates
        | Section::Artwork
        | Section::ArtworkExtras => Destination::Enhance,
        Section::Problems | Section::ProblemsRepair | Section::History | Section::HistoryUndo => {
            Destination::HealthRecovery
        }
        Section::Settings | Section::Advanced | Section::AdvancedDiagnostics => {
            Destination::Settings
        }
        Section::Activity => return None,
    })
}

/// The simple view's location trail. Its first crumb names the sidebar row
/// that is highlighted, so a page never reads as living somewhere else (Setup
/// was "Advanced / Diagnostics", Play was "Organisation", a game "Games").
/// Only that parent is replaced; page and game crumbs are kept as they are.
pub(super) fn simple_breadcrumb_labels(route: &Route, game_title: Option<&str>) -> Vec<String> {
    let mut labels = super::routes::breadcrumb_labels(route, game_title);
    let family = super::routes::family_for_route(route).map(|family| family.label());
    if let (Some(destination), Some(first)) = (destination_for(route), labels.first_mut())
        && (family == Some(first.as_str()) || first == "Games")
    {
        *first = destination.label().to_string();
    }
    labels.dedup();
    labels
}

// ------------------------------------------------------------ capability map

/// Whether the backend can do what a planned control promises, today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum Capability {
    /// An existing route already provides it.
    Existing,
    /// It can be shown and previewed, but must not offer Apply yet.
    PreviewOnly,
    /// It needs new presentation work (a later slice) over existing backends.
    New,
    /// No backend contract exists; it must stay hidden or disabled.
    Unavailable,
}

/// One planned Simple Mode v1 control.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct PlannedControl {
    pub destination: Destination,
    pub control: &'static str,
    pub capability: Capability,
    /// The existing route that backs it, when one does.
    pub route: Option<Route>,
    /// The evidence for the classification, or what is missing.
    pub basis: &'static str,
    /// The delivery slice that surfaces it.
    pub slice: &'static str,
}

/// Slice 0: every control the plan draws, classified against current main.
/// A control is `Existing` only when a route for it exists; anything that
/// would write without a proven contract is `PreviewOnly` or `Unavailable`.
///
/// Slice 0 is a reviewed inventory: later slices read it; nothing renders
/// from it yet, so it is exercised by tests only for now.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn capability_map() -> Vec<PlannedControl> {
    use Capability::{Existing, New, PreviewOnly, Unavailable};
    use Destination::{
        Enhance, HealthRecovery, Home, Library, OrganiseExport, Settings, Setup, Tools,
    };
    let section = |section| Some(Route::Section(section));
    let control = |destination, control, capability, route, basis, slice| PlannedControl {
        destination,
        control,
        capability,
        route,
        basis,
        slice,
    };
    vec![
        control(
            Home,
            "Header search",
            Existing,
            Some(Route::BrowsePlay),
            "Library filter search, shown in Browse & Play.",
            "1",
        ),
        control(
            Home,
            "Continue Playing hero",
            New,
            Some(Route::BrowsePlay),
            "Needs a recent-play projection; shown only when real recent data exists.",
            "2",
        ),
        control(
            Home,
            "Mr Wiz tip",
            Existing,
            Some(Route::Home),
            "Deterministic guidance already renders on Home.",
            "2",
        ),
        control(
            Home,
            "Quick action: Check Games",
            Existing,
            section(Section::Check),
            "Existing Check Games route.",
            "2",
        ),
        control(
            Home,
            "Quick action: Organise",
            Existing,
            section(Section::Build),
            "Existing Organisation route.",
            "2",
        ),
        control(
            Home,
            "Quick action: Duplicates",
            Existing,
            section(Section::Duplicates),
            "Existing Duplicates route.",
            "2",
        ),
        control(
            Home,
            "Quick action: Convert",
            Existing,
            section(Section::Converter),
            "Existing Converter route.",
            "2",
        ),
        control(
            Home,
            "Quick action: Fix Problems",
            Existing,
            section(Section::Problems),
            "Existing Problems & Repair route.",
            "2",
        ),
        control(
            Home,
            "Recently Played / Recently Added",
            New,
            None,
            "No recent-play or recently-added projection is exposed to the GUI yet.",
            "2",
        ),
        control(
            Home,
            "Setup and active-job cards",
            Existing,
            section(Section::Activity),
            "Activity and Setup & Doctor already report these.",
            "2",
        ),
        control(
            Library,
            "Artwork grid / list toggle and search",
            Existing,
            Some(Route::BrowsePlay),
            "Browse & Play toolbar.",
            "3",
        ),
        control(
            Library,
            "Platform chips",
            Existing,
            section(Section::Platforms),
            "Platforms route and the library platform filter.",
            "3",
        ),
        control(
            Library,
            "Filters: All / Ready / Needs attention / Not checked",
            New,
            Some(Route::BrowsePlay),
            "Attention and unverified filters exist; a Ready filter needs a readiness projection.",
            "3",
        ),
        control(
            Library,
            "Game Detail pane",
            Existing,
            Some(Route::BrowsePlay),
            "Game Details route; the inline pane is new presentation.",
            "3",
        ),
        control(
            Library,
            "Play / Prepare",
            Existing,
            section(Section::Launch),
            "Launch readiness and the Launch route decide availability.",
            "3",
        ),
        control(
            Library,
            "Saves",
            Existing,
            section(Section::Saves),
            "Existing Saves & States route.",
            "3",
        ),
        control(
            Enhance,
            "Selected game and emulator context",
            Existing,
            section(Section::Mods),
            "Mods & Cheats page with its game context.",
            "4a",
        ),
        control(
            Enhance,
            "Searchable cheat checkbox list",
            New,
            section(Section::Mods),
            "Cheat sources and planners exist; the checkbox catalogue is new presentation.",
            "4a",
        ),
        control(
            Enhance,
            "Preview changes",
            PreviewOnly,
            section(Section::Mods),
            "Existing per-adapter planners can preview; no grouped preview contract.",
            "4a",
        ),
        control(
            Enhance,
            "Apply N cheats (grouped)",
            Unavailable,
            None,
            "No typed multi-file group-operation contract exists.",
            "4b",
        ),
        control(
            Enhance,
            "Grouped Undo",
            Unavailable,
            None,
            "Independent journals are not one atomic Undo; needs a parent transaction.",
            "4b",
        ),
        control(
            OrganiseExport,
            "Check & Identify",
            Existing,
            section(Section::Check),
            "Existing Check Games route.",
            "5",
        ),
        control(
            OrganiseExport,
            "Organise & Export",
            Existing,
            section(Section::Build),
            "Existing Organisation route.",
            "5",
        ),
        control(
            OrganiseExport,
            "Review Duplicates",
            Existing,
            section(Section::Duplicates),
            "Existing Duplicates route.",
            "5",
        ),
        control(
            OrganiseExport,
            "Convert Media",
            Existing,
            section(Section::Converter),
            "Existing Converter route.",
            "5",
        ),
        control(
            OrganiseExport,
            "Fix Problems",
            Existing,
            section(Section::Problems),
            "Existing Problems & Repair route.",
            "5",
        ),
        control(
            OrganiseExport,
            "What / Why / Preview / Confirm / Result wizard",
            New,
            None,
            "Wizard framing over the existing routes; no new mutation path.",
            "5",
        ),
        control(
            Setup,
            "Find emulators / Add game folder / Check setup",
            Existing,
            section(Section::Setup),
            "Existing Setup & Doctor route.",
            "5",
        ),
        control(
            Setup,
            "Firmware status",
            Existing,
            section(Section::Firmware),
            "Existing BIOS / Firmware route.",
            "5",
        ),
        control(
            HealthRecovery,
            "Running, waiting and finished jobs",
            Existing,
            section(Section::Activity),
            "Existing Activity route.",
            "6",
        ),
        control(
            HealthRecovery,
            "History and eligible Undo",
            Existing,
            section(Section::History),
            "The one existing History system decides Undo eligibility.",
            "6",
        ),
        control(
            Tools,
            "All Tools browser: groups, search and safety filter",
            New,
            None,
            "Presentation over existing routes only; each entry opens its existing page.",
            "Blue 2",
        ),
        control(
            Settings,
            "Classic view fallback and text scale",
            Existing,
            section(Section::Settings),
            "This slice.",
            "1",
        ),
        control(
            Settings,
            "All Tools / Advanced",
            Existing,
            section(Section::Advanced),
            "The complete existing navigation, always reachable.",
            "1",
        ),
    ]
}

// --------------------------------------------------------------------- style

/// Blue profile: the approved EmuWiz Blue tokens. Scoped to the Simple shell
/// so the stock v2 palette in `ui::theme` is untouched while Simple is opt-in.
pub(super) mod palette {
    //! The shell's names for the approved Blue tokens in `ui::theme::blue`,
    //! the single source of truth.
    use crate::ui::theme::blue;
    use eframe::egui::Color32;

    pub const APP_BACKGROUND: Color32 = blue::APP_BACKGROUND;
    pub const DEEP_BACKGROUND: Color32 = blue::DEEP_BACKGROUND;
    pub const SIDEBAR: Color32 = blue::DEEP_BACKGROUND;
    /// The top bar sits on the window background, divided by a border.
    pub const HEADER: Color32 = blue::APP_BACKGROUND;
    pub const CARD_SURFACE: Color32 = blue::CARD_SURFACE;
    pub const RAISED_SURFACE: Color32 = blue::RAISED_SURFACE;
    pub const BORDER_SUBTLE: Color32 = blue::BORDER_SUBTLE;
    pub const BORDER_STRONG: Color32 = blue::BORDER_STRONG;
    /// Selected rows and pills: fill plus [`SELECTED_OUTLINE`].
    pub const SELECTED: Color32 = blue::SELECTED_FILL;
    pub const SELECTED_OUTLINE: Color32 = blue::SELECTED_OUTLINE;
    /// Between the card and the selected fill, so hover never reads as selected.
    pub const HOVER: Color32 = Color32::from_rgb(0x14, 0x27, 0x4a);
    /// Outlined secondary action (All Tools).
    pub const SECONDARY_ACTION: Color32 = blue::SELECTED_FILL;
    pub const PRIMARY_ACTION: Color32 = blue::PRIMARY_ACTION;
    /// Bright accent for the selected-item edge, outlines and keyboard focus.
    pub const ACCENT: Color32 = blue::ACCENT;
    pub const PRIMARY_TEXT: Color32 = blue::PRIMARY_TEXT;
    pub const SECONDARY_TEXT: Color32 = blue::SECONDARY_TEXT;
    pub const DIM_TEXT: Color32 = blue::DIM_TEXT;
}

pub(super) const SIDEBAR_WIDTH: f32 = 232.0;
pub(super) const SIDEBAR_WIDTH_NARROW: f32 = 208.0;
/// The compact icon rail (the 1024x600 boards).
pub(super) const RAIL_WIDTH: f32 = 64.0;
pub(super) const HEADER_HEIGHT: f32 = 64.0;

/// Shell sizes for a window of `width` x `height` logical points: the
/// approved 1920 / 1440 / 1024 measurements, chosen by the app's own width
/// classes (Standard below 1240 uses the rail) and by a short window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ShellMetrics {
    pub sidebar: f32,
    pub rail: bool,
    pub header: f32,
    pub status: f32,
}

pub(super) fn shell_metrics(width: f32, height: f32) -> ShellMetrics {
    if width < 1240.0 || height < 680.0 {
        ShellMetrics {
            sidebar: RAIL_WIDTH,
            rail: true,
            header: 54.0,
            status: 36.0,
        }
    } else if width < 1680.0 {
        ShellMetrics {
            sidebar: SIDEBAR_WIDTH_NARROW,
            rail: false,
            header: 60.0,
            status: 40.0,
        }
    } else {
        ShellMetrics {
            sidebar: SIDEBAR_WIDTH,
            rail: false,
            header: HEADER_HEIGHT,
            status: 44.0,
        }
    }
}
/// Navigation rows are one tall, easy target.
pub(super) const NAV_TARGET_HEIGHT: f32 = 48.0;
pub(super) const SEARCH_WIDTH_MAX: f32 = 490.0;
pub(super) const SEARCH_WIDTH_MIN: f32 = 200.0;
pub(super) const PAGE_TITLE_SIZE: f32 = 32.0;
const CORNER_RADIUS: u8 = 12;
const DIALOG_CORNER_RADIUS: u8 = 16;
const WIDGET_CORNER_RADIUS: u8 = 6;
const ICON_SIDE: f32 = 22.0;

/// Turns a classic style into the blue profile. Works on a value, never on
/// the live context, so the result is the same whatever happened in between.
fn blue_style(style: &mut egui::Style) {
    {
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::proportional(PAGE_TITLE_SIZE),
        );
        // Blue's four-step scale: page 32, section 24, card 20, body 18.
        crate::ui::theme::set_role_sizes(style, PAGE_TITLE_SIZE, 24.0, 20.0);
        style.spacing.button_padding = egui::vec2(16.0, 11.0);
        style.spacing.interact_size.y = 44.0;
        // A checkbox and radio box large enough to read and to hit.
        style.spacing.icon_width = 18.0;
        style.spacing.icon_width_inner = 10.0;
        let visuals = &mut style.visuals;
        visuals.panel_fill = palette::APP_BACKGROUND;
        visuals.window_fill = palette::CARD_SURFACE;
        visuals.faint_bg_color = palette::CARD_SURFACE;
        visuals.extreme_bg_color = palette::DEEP_BACKGROUND;
        visuals.override_text_color = Some(palette::PRIMARY_TEXT);
        visuals.selection.bg_fill = palette::SELECTED;
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, palette::PRIMARY_TEXT);
        visuals.hyperlink_color = palette::ACCENT;
        let radius = egui::CornerRadius::same(CORNER_RADIUS);
        // egui draws a checkbox with the widget radius: anything near half the
        // box makes it a circle with no visible state. Widgets use a small
        // radius and a larger box; action buttons set the spec's 10 px.
        let widget_radius = egui::CornerRadius::same(WIDGET_CORNER_RADIUS);
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.corner_radius = widget_radius;
        }
        // Thin card borders one step lighter than the surface.
        visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE);
        visuals.widgets.inactive.bg_fill = palette::RAISED_SURFACE;
        visuals.widgets.inactive.weak_bg_fill = palette::RAISED_SURFACE;
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE);
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, palette::PRIMARY_TEXT);
        visuals.widgets.hovered.bg_fill = palette::HOVER;
        visuals.widgets.hovered.weak_bg_fill = palette::HOVER;
        visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, palette::ACCENT);
        // egui also uses `active` for a focused control. Keep it a selected
        // shade, not the primary blue, so focusing a secondary button never
        // makes it look primary; the focus ring is the focus signal.
        visuals.widgets.active.bg_fill = palette::SELECTED;
        visuals.widgets.active.weak_bg_fill = palette::SELECTED;
        visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, palette::SELECTED_OUTLINE);
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, palette::PRIMARY_TEXT);
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, palette::PRIMARY_TEXT);
        // Weak (hint, caption) text stays readable on every Blue surface.
        visuals.weak_text_color = Some(palette::DIM_TEXT);
        visuals.widgets.open.bg_fill = palette::SELECTED;
        visuals.widgets.open.weak_bg_fill = palette::SELECTED;
        // Dialogs: radius 16, a stronger edge so they lift off the page.
        visuals.window_corner_radius = egui::CornerRadius::same(DIALOG_CORNER_RADIUS);
        visuals.menu_corner_radius = radius;
        visuals.window_stroke = egui::Stroke::new(1.0_f32, palette::BORDER_STRONG);
    }
}

pub(super) fn sidebar_frame(rail: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(palette::SIDEBAR)
        .inner_margin(if rail {
            egui::Margin::symmetric(8, 10)
        } else {
            egui::Margin::symmetric(14, 12)
        })
}

/// The Blue status bar: deep background, a subtle top edge, page padding.
pub(super) fn status_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(palette::DEEP_BACKGROUND)
        .inner_margin(egui::Margin::symmetric(20, 4))
}

pub(super) fn header_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(palette::HEADER)
        .inner_margin(egui::Margin::symmetric(24, 10))
}

/// How wide the header search is for a given free width: as wide as the
/// reference when there is room, never squeezing the All Tools button out.
pub(super) fn search_width(available: f32) -> f32 {
    (available - 190.0).clamp(SEARCH_WIDTH_MIN, SEARCH_WIDTH_MAX)
}

// --------------------------------------------------------------------- icons

/// Line icons drawn with the painter, so no icon font or new asset is needed.
fn paint_icon(
    painter: &egui::Painter,
    destination: Destination,
    rect: egui::Rect,
    color: egui::Color32,
) {
    use egui::{Shape, pos2, vec2};
    let stroke = egui::Stroke::new(1.7_f32, color);
    let c = rect.center();
    let unit = rect.width() / 22.0;
    let at = |x: f32, y: f32| pos2(c.x + x * unit, c.y + y * unit);
    match destination {
        Destination::Home => {
            painter.add(Shape::line(
                vec![at(-9.0, 0.0), at(0.0, -8.5), at(9.0, 0.0)],
                stroke,
            ));
            painter.add(Shape::line(
                vec![at(-6.5, -1.5), at(-6.5, 8.0), at(6.5, 8.0), at(6.5, -1.5)],
                stroke,
            ));
        }
        Destination::Library => {
            for (x, y) in [(-5.0, -5.0), (5.0, -5.0), (-5.0, 5.0), (5.0, 5.0)] {
                painter.rect_stroke(
                    egui::Rect::from_center_size(at(x, y), vec2(7.0, 7.0) * unit),
                    2.0,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
        Destination::Setup => {
            // A game controller.
            painter.rect_stroke(
                egui::Rect::from_center_size(c, vec2(20.0, 12.0) * unit),
                5.0 * unit,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([at(-7.0, 0.0), at(-3.0, 0.0)], stroke);
            painter.line_segment([at(-5.0, -2.0), at(-5.0, 2.0)], stroke);
            painter.circle_filled(at(4.0, 1.5), 1.2 * unit, color);
            painter.circle_filled(at(6.5, -1.5), 1.2 * unit, color);
        }
        Destination::OrganiseExport => {
            // A folder.
            painter.add(Shape::closed_line(
                vec![
                    at(-9.0, -6.5),
                    at(-3.0, -6.5),
                    at(-1.0, -4.0),
                    at(9.0, -4.0),
                    at(9.0, 7.0),
                    at(-9.0, 7.0),
                ],
                stroke,
            ));
        }
        Destination::Tools => {
            // A wrench.
            painter.line_segment(
                [at(-7.5, 7.5), at(2.0, -2.0)],
                egui::Stroke::new(2.4, color),
            );
            painter.circle_stroke(at(4.5, -4.5), 4.2 * unit, stroke);
        }
        Destination::Enhance => {
            // A sparkle: four long and four short rays.
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = angle.sin_cos();
                let (inner, outer) = if index % 2 == 0 {
                    (3.0, 9.5)
                } else {
                    (3.0, 6.0)
                };
                painter.line_segment(
                    [at(inner * cos, inner * sin), at(outer * cos, outer * sin)],
                    stroke,
                );
            }
        }
        Destination::HealthRecovery => {
            // A shield with a cross.
            painter.add(Shape::closed_line(
                vec![
                    at(0.0, -9.0),
                    at(8.0, -6.0),
                    at(7.0, 2.0),
                    at(0.0, 9.0),
                    at(-7.0, 2.0),
                    at(-8.0, -6.0),
                ],
                stroke,
            ));
            painter.line_segment([at(0.0, -3.5), at(0.0, 3.5)], stroke);
            painter.line_segment([at(-3.5, 0.0), at(3.5, 0.0)], stroke);
        }
        Destination::Settings => {
            // Three sliders.
            for (y, knob) in [(-5.5, 3.0), (0.0, -3.5), (5.5, 1.5)] {
                painter.line_segment([at(-8.5, y), at(8.5, y)], stroke);
                painter.circle_filled(at(knob, y), 2.2 * unit, color);
            }
        }
    }
}

/// A magnifier, for the header search field.
pub(super) fn paint_search_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.7_f32, color);
    painter.circle_stroke(center + egui::vec2(-1.5, -1.5), 5.5, stroke);
    painter.line_segment(
        [center + egui::vec2(2.6, 2.6), center + egui::vec2(7.0, 7.0)],
        stroke,
    );
}

/// The 2x2 grid on the All Tools button.
pub(super) fn paint_grid_icon(painter: &egui::Painter, center: egui::Pos2, color: egui::Color32) {
    for (x, y) in [(-4.5, -4.5), (4.5, -4.5), (-4.5, 4.5), (4.5, 4.5)] {
        painter.rect_filled(
            egui::Rect::from_center_size(center + egui::vec2(x, y), egui::vec2(6.5, 6.5)),
            1.5,
            color,
        );
    }
}

// ------------------------------------------------------------------- sidebar

/// Paints the eight-destination sidebar and returns the destination chosen
/// this frame. Navigation itself stays with the caller's router. `mascot` is
/// the bundled EmuWiz badge when it has been decoded; the brand reads fine
/// without it. With `rail` only the icons are shown (labels on hover and to
/// assistive technology). `browser_open` highlights Tools while the All Tools
/// browser is shown.
pub(super) fn show_sidebar(
    ui: &mut egui::Ui,
    current: &Route,
    mascot: Option<&egui::TextureHandle>,
    browser_open: bool,
    rail: bool,
) -> Option<Destination> {
    let selected = if browser_open {
        Some(Destination::Tools)
    } else {
        destination_for(current)
    };
    let mut chosen = None;
    if rail {
        ui.vertical_centered(|ui| match mascot {
            Some(texture) => {
                ui.add(
                    egui::Image::new((texture.id(), egui::vec2(36.0, 36.0)))
                        .sense(egui::Sense::hover()),
                )
                .on_hover_text("EmuWiz");
            }
            None => {
                ui.label(egui::RichText::new("EW").size(18.0).strong());
            }
        });
        ui.add_space(14.0);
    } else {
        ui.horizontal(|ui| {
            if let Some(texture) = mascot {
                ui.add(
                    egui::Image::new((texture.id(), egui::vec2(44.0, 44.0)))
                        .sense(egui::Sense::hover()),
                );
            }
            ui.label(egui::RichText::new("EmuWiz").size(26.0).strong());
        });
        ui.add_space(18.0);
    }
    // The destinations scroll when the window is short or the interface is
    // enlarged; the footer keeps its place below them. Nothing is dropped: a
    // row that does not fit is one wheel turn or Tab away.
    let footer = if rail { 0.0 } else { FOOTER_HEIGHT };
    let rows = (ui.available_height() - footer).max(NAV_TARGET_HEIGHT);
    show_reveal_scroll(
        ui,
        "v2_simple_destinations",
        rows,
        &DESTINATIONS.map(nav_id),
        selected.map(nav_id),
        |ui| {
            #[cfg(test)]
            {
                let clip = ui.clip_rect();
                ui.ctx()
                    .data_mut(|data| data.insert_temp(sidebar_list_clip_id(), clip));
            }
            ui.spacing_mut().item_spacing.y = 6.0;
            for destination in DESTINATIONS {
                let is_selected = selected == Some(destination);
                if nav_item(ui, destination, is_selected, rail).clicked() {
                    chosen = Some(destination);
                }
            }
        },
    );
    if !rail {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Local-first.\nRead-only by default.")
                .size(13.0)
                .color(palette::DIM_TEXT),
        )
        .on_hover_text(concat!("EmuWiz v", env!("CARGO_PKG_VERSION")));
    }
    chosen
}

/// Where tests find the visible part of the destination list.
#[cfg(test)]
pub(super) fn sidebar_list_clip_id() -> egui::Id {
    egui::Id::new("v2_simple_destinations_clip")
}

/// The stable identity of a destination's sidebar row (focus, tests).
pub(super) fn nav_id(destination: Destination) -> egui::Id {
    egui::Id::new(("v2_simple_destination", destination))
}

const FOOTER_HEIGHT: f32 = 44.0;
const NAV_LABEL_SIZE: f32 = 17.0;
const NAV_LABEL_MIN_SIZE: f32 = 12.5;

/// Where a row's icon and label go, and how large the label is, for a row of
/// `width`. The reference spacing when it fits; tighter, then smaller, when
/// the sidebar is narrow or the interface is enlarged - never cut off.
fn nav_metrics(width: f32, label_width_at_full_size: f32) -> (f32, f32, f32) {
    const RIGHT_PAD: f32 = 10.0;
    for (icon_x, text_x) in [(28.0, 56.0), (22.0, 48.0)] {
        if label_width_at_full_size <= width - text_x - RIGHT_PAD {
            return (icon_x, text_x, NAV_LABEL_SIZE);
        }
    }
    let room = (width - 48.0 - RIGHT_PAD).max(1.0);
    let size = (NAV_LABEL_SIZE * room / label_width_at_full_size)
        .clamp(NAV_LABEL_MIN_SIZE, NAV_LABEL_SIZE);
    // Whole-ish steps keep text crisp.
    (22.0, 48.0, (size * 2.0).floor() / 2.0)
}

/// One navigation row: transparent until hovered, the primary-blue pill when
/// selected (as on the approved boards). A real focusable button for the
/// keyboard and for assistive technology.
fn nav_item(
    ui: &mut egui::Ui,
    destination: Destination,
    selected: bool,
    rail: bool,
) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), NAV_TARGET_HEIGHT));
    let response = ui.interact(rect, nav_id(destination), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            destination.label(),
        )
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 10.0, palette::PRIMARY_ACTION);
        } else if response.hovered() {
            painter.rect_filled(rect, 10.0, palette::HOVER);
        }
        let color = if selected || response.hovered() {
            palette::PRIMARY_TEXT
        } else {
            palette::SECONDARY_TEXT
        };
        if rail {
            let icon =
                egui::Rect::from_center_size(rect.center(), egui::vec2(ICON_SIDE, ICON_SIDE));
            paint_icon(painter, destination, icon, color);
        } else {
            let full_width = painter
                .layout_no_wrap(
                    destination.label().to_owned(),
                    egui::FontId::proportional(NAV_LABEL_SIZE),
                    color,
                )
                .size()
                .x;
            let (icon_x, text_x, size) = nav_metrics(rect.width(), full_width);
            let icon = egui::Rect::from_center_size(
                egui::pos2(rect.left() + icon_x, rect.center().y),
                egui::vec2(ICON_SIDE, ICON_SIDE),
            );
            paint_icon(painter, destination, icon, color);
            painter.text(
                egui::pos2(rect.left() + text_x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                destination.label(),
                egui::FontId::proportional(size),
                color,
            );
        }
    }
    crate::ui::theme::paint_focus_ring(ui, &response, rect, 10.0);
    if rail {
        response.on_hover_text(format!(
            "{}\n{}",
            destination.label(),
            destination.purpose()
        ))
    } else {
        response.on_hover_text(destination.purpose())
    }
}

/// The All Tools button with its grid icon: the header's primary action.
/// `open` shows it pressed while the browser is shown.
pub(super) fn all_tools_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let font = egui::FontId::proportional(16.0);
    let label = "All Tools";
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), palette::PRIMARY_TEXT)
        .size()
        .x;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(text_width + 62.0, 42.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), open, label)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if response.hovered() || open {
            palette::PRIMARY_ACTION.gamma_multiply(1.15)
        } else {
            palette::PRIMARY_ACTION
        };
        painter.rect(
            rect,
            10.0,
            fill,
            egui::Stroke::new(1.0_f32, palette::SELECTED_OUTLINE),
            egui::StrokeKind::Inside,
        );
        paint_grid_icon(
            painter,
            egui::pos2(rect.left() + 24.0, rect.center().y),
            palette::PRIMARY_TEXT,
        );
        painter.text(
            egui::pos2(rect.left() + 42.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            palette::PRIMARY_TEXT,
        );
    }
    crate::ui::theme::paint_focus_ring(ui, &response, rect, 10.0);
    response.on_hover_text(
        "Every tool, grouped by what you want to get done. Nothing is hidden or removed.",
    )
}

// ---------------------------------------------------------- manage library

/// One card on the Manage Library hub. Navigation only: each opens a route
/// that already exists and already does the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ManageTask {
    pub title: &'static str,
    pub description: &'static str,
    pub section: Section,
    icon: TaskIcon,
    tint: egui::Color32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskIcon {
    Search,
    Folder,
    Copies,
    Picture,
    Wrench,
    Star,
}

/// The six Manage Library tasks, in reading order (board D-Manage-Normal). Descriptions say what the
/// destination offers, never a result.
pub(super) const MANAGE_TASKS: [ManageTask; 6] = [
    ManageTask {
        title: "Check & Identify",
        description: "Compare your games with known release catalogues.",
        section: Section::Check,
        icon: TaskIcon::Search,
        tint: egui::Color32::from_rgb(0x1f, 0x6f, 0xd6),
    },
    ManageTask {
        title: "Organise & Export",
        description: "Preview tidier names and folders before anything changes.",
        section: Section::Build,
        icon: TaskIcon::Folder,
        tint: egui::Color32::from_rgb(0x5b, 0x4b, 0xc4),
    },
    ManageTask {
        title: "Review Duplicates",
        description: "Find exact duplicate files and review them.",
        section: Section::Duplicates,
        icon: TaskIcon::Copies,
        tint: egui::Color32::from_rgb(0xa8, 0x3a, 0x6a),
    },
    ManageTask {
        title: "Convert Media",
        description: "Convert supported disc images to another format.",
        section: Section::Converter,
        icon: TaskIcon::Picture,
        tint: egui::Color32::from_rgb(0x1e, 0x8a, 0x5a),
    },
    ManageTask {
        title: "Fix Problems",
        description: "See what needs attention and the next step.",
        section: Section::Problems,
        icon: TaskIcon::Wrench,
        tint: egui::Color32::from_rgb(0xb0, 0x62, 0x2a),
    },
    ManageTask {
        title: "Cheats & Mods",
        description: "Cheats and mods for a selected game, previewed first.",
        section: Section::Mods,
        icon: TaskIcon::Star,
        tint: egui::Color32::from_rgb(0x8e, 0x3f, 0xb0),
    },
];

pub(super) const MANAGE_TITLE: &str = "Manage Library";
pub(super) const MANAGE_PURPOSE: &str = "Choose a task. Each one opens its existing tool; nothing changes until you confirm a preview there.";

const TASK_CARD_HEIGHT: f32 = 168.0;
const TASK_CARD_MIN_WIDTH: f32 = 210.0;
const TASK_CARD_MAX_WIDTH: f32 = 300.0;
const TASK_CARD_GAP: f32 = 14.0;

/// How many cards fit on one row, and how wide each is.
const MANAGE_MAX_COLUMNS: usize = 3;

pub(super) fn task_grid(available: f32) -> (usize, f32) {
    let columns = (((available + TASK_CARD_GAP) / (TASK_CARD_MIN_WIDTH + TASK_CARD_GAP)).floor()
        as usize)
        // Two rows of three at most, as on the board.
        .clamp(1, MANAGE_MAX_COLUMNS);
    let width = ((available - TASK_CARD_GAP * (columns as f32 - 1.0)) / columns as f32)
        .clamp(TASK_CARD_MIN_WIDTH.min(available), TASK_CARD_MAX_WIDTH);
    (columns, width)
}

/// Paints the hub and returns the section whose card was activated.
pub(super) fn show_manage_library(ui: &mut egui::Ui) -> Option<Section> {
    let mut chosen = None;
    // A side column with the safety notes and links when there is room.
    let side = ui.available_width() >= MANAGE_SIDE_MIN_WIDTH;
    let cards_width = if side {
        ui.available_width() - MANAGE_SIDE_WIDTH - TASK_CARD_GAP
    } else {
        ui.available_width()
    };
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(cards_width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(cards_width);
                manage_safety_legend(ui);
                let (columns, width) = task_grid(cards_width);
                for row in MANAGE_TASKS.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = TASK_CARD_GAP;
                        for task in row {
                            if task_card(ui, task, width).clicked() {
                                chosen = Some(task.section);
                            }
                        }
                    });
                    ui.add_space(TASK_CARD_GAP - ui.spacing().item_spacing.y);
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Looking for something else? All Tools at the top right lists every tool.",
                    )
                    .color(palette::SECONDARY_TEXT),
                );
            },
        );
        if side {
            ui.add_space(TASK_CARD_GAP);
            ui.allocate_ui_with_layout(
                egui::vec2(MANAGE_SIDE_WIDTH, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(MANAGE_SIDE_WIDTH);
                    if let Some(section) = manage_side_column(ui) {
                        chosen = Some(section);
                    }
                },
            );
        }
    });
    chosen
}

const MANAGE_SIDE_MIN_WIDTH: f32 = 1180.0;
const MANAGE_SIDE_WIDTH: f32 = 380.0;

/// The two safety tags the cards use, explained once above them.
fn manage_safety_legend(ui: &mut egui::Ui) {
    use crate::ui::components::{SafetyTag, safety_tag};
    ui.horizontal_wrapped(|ui| {
        safety_tag(ui, SafetyTag::ReadOnly);
        ui.label(egui::RichText::new("looking only").color(palette::SECONDARY_TEXT));
        safety_tag(ui, SafetyTag::AsksFirst);
        ui.label(
            egui::RichText::new("shows a preview, then waits for you")
                .color(palette::SECONDARY_TEXT),
        );
    });
    ui.add_space(8.0);
}

/// History and Storage links, and how files stay safe (board right column).
fn manage_side_column(ui: &mut egui::Ui) -> Option<Section> {
    let mut chosen = None;
    let card = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        egui::Frame::new()
            .fill(crate::ui::theme::blue::CARD_SURFACE_TOP)
            .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
            .corner_radius(14)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui);
            });
        ui.add_space(TASK_CARD_GAP);
    };
    card(ui, &mut |ui| {
        ui.label(crate::ui::theme::card_title("History"));
        ui.label(
            egui::RichText::new(
                "A record of what EmuWiz did earlier, not the current state of your files.",
            )
            .color(palette::SECONDARY_TEXT),
        );
        if ui.link("Open History ›").clicked() {
            chosen = Some(Section::History);
        }
    });
    card(ui, &mut |ui| {
        ui.label(crate::ui::theme::card_title("Where is my space going?"));
        ui.label(
            egui::RichText::new("See how much your library uses and what could safely shrink.")
                .color(palette::SECONDARY_TEXT),
        );
        ui.horizontal_wrapped(|ui| {
            crate::ui::components::safety_tag(ui, crate::ui::components::SafetyTag::ReadOnly);
            if ui.link("Open Storage ›").clicked() {
                chosen = Some(Section::Storage);
            }
        });
    });
    card(ui, &mut |ui| {
        ui.label(crate::ui::theme::card_title("How your files stay safe"));
        for line in MANAGE_SAFETY_NOTES {
            ui.label(egui::RichText::new(line).color(palette::SECONDARY_TEXT));
        }
    });
    chosen
}

pub(super) const MANAGE_SAFETY_NOTES: [&str; 4] = [
    "1 · Looking and checking only read your files.",
    "2 · Anything that changes files shows a preview first.",
    "3 · EmuWiz waits for your confirmation.",
    "4 · Changes are recorded in History, with Undo where it is supported.",
];

/// Stable across the hub's responsive row layout.
pub(super) fn task_id(section: Section) -> egui::Id {
    egui::Id::new(("v2_simple_manage_task", section))
}

/// egui queues reverse focus until the next pass. Complete that pass within
/// the same rendered frame; do not delay or consume the keyboard event.
pub(super) fn prepare_keyboard_reveal(context: &egui::Context) {
    note_input_modality(context);
    if context.input(|input| input.modifiers.shift_only() && input.key_pressed(egui::Key::Tab)) {
        context.request_discard("finish reverse Simple keyboard focus before display");
    }
}

/// Keep the existing page scroller and identity. Only the Simple Manage hub
/// uses focus reveal; Classic and every other page keep their usual scroller.
pub(super) fn show_page_scroll<R>(
    ui: &mut egui::Ui,
    section: Section,
    manage: bool,
    content: impl FnOnce(&mut egui::Ui) -> R,
) {
    let salt = ("v2_page_content", section);
    if manage {
        show_reveal_scroll(
            ui,
            salt,
            ui.available_height(),
            &MANAGE_TASKS.map(|task| task_id(task.section)),
            None,
            content,
        );
    } else {
        let keys = if section == Section::Check {
            PageKeys::Full
        } else {
            PageKeys::PageOnly
        };
        show_focus_reveal_scroll_inner(ui, salt, [false, false], keys, false, content);
    }
}

fn keyboard_modality_slot() -> egui::Id {
    egui::Id::new("v2_last_input_was_keyboard")
}

/// Remembers whether the person last acted with the keyboard or the pointer, so
/// a page opened by Enter can take focus while one opened by a click does not.
fn note_input_modality(context: &egui::Context) {
    let (key, pointer) = context.input(|input| {
        (
            input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Key { pressed: true, .. })),
            input.pointer.any_pressed(),
        )
    });
    if key || pointer {
        context.data_mut(|data| data.insert_temp(keyboard_modality_slot(), key && !pointer));
    }
}

/// The first enabled, focusable widget `layer` gained after `first` widgets.
fn first_focusable_since(
    context: &egui::Context,
    layer: egui::LayerId,
    first: usize,
) -> Option<egui::Id> {
    context.viewport(|viewport| {
        viewport
            .this_pass
            .widgets
            .layers()
            .find(|(candidate, _)| **candidate == layer)
            .and_then(|(_, widgets)| {
                widgets
                    .iter()
                    .skip(first)
                    .find(|widget| widget.enabled && widget.sense.is_focusable())
                    .map(|widget| widget.id)
            })
    })
}

/// On the first frame of a visit that the keyboard started, put focus on the
/// first control of the page. egui's own order begins at the status bar and
/// sidebar, so without this a keyboard user would tab through all of them to
/// reach the page they just opened. A visit is a run of consecutive frames in
/// which the scroller was drawn; a pointer-started visit never takes focus.
fn focus_first_control_on_keyboard_entry(ui: &egui::Ui, id: egui::Id, first: Option<egui::Id>) {
    let context = ui.ctx();
    let frame = context.cumulative_frame_nr();
    let visit_slot = id.with("last_visit_frame");
    let pending_slot = id.with("entry_focus_pending");
    let stale_slot = id.with("entry_focus_stale");
    let previous = context.data(|data| data.get_temp::<u64>(visit_slot));
    context.data_mut(|data| data.insert_temp(visit_slot, frame));
    let fresh = previous.is_none_or(|last| frame.saturating_sub(last) > 1);
    let focused = context.memory(|memory| memory.focused());
    let keyboard = context
        .data(|data| data.get_temp::<bool>(keyboard_modality_slot()))
        .unwrap_or(false);
    if fresh {
        context.data_mut(|data| {
            data.insert_temp(pending_slot, keyboard);
            // The control that opened this page (Play) still holds focus on the
            // first frame; that focus belongs to the old page.
            data.insert_temp(stale_slot, focused);
        });
    }
    let pending = context
        .data(|data| data.get_temp::<bool>(pending_slot))
        .unwrap_or(false);
    if !pending {
        return;
    }
    // The page's controls can arrive a moment after the page opens (its
    // readiness check), so the request waits for the first focusable control. It
    // ends once it has focused one, or the person focuses something else or
    // clicks.
    let stale = context
        .data(|data| data.get_temp::<Option<egui::Id>>(stale_slot))
        .flatten();
    let moved_elsewhere = focused.is_some() && focused != stale;
    let clicked = context.input(|input| input.pointer.any_pressed());
    if moved_elsewhere || clicked {
        context.data_mut(|data| data.insert_temp(pending_slot, false));
    } else if let Some(first) = first {
        context.memory_mut(|memory| memory.request_focus(first));
        context.data_mut(|data| data.insert_temp(pending_slot, false));
    }
}

/// An ordinary vertical scroller whose content is brought into view when a
/// control in it takes keyboard focus. Pages and the Classic sidebar share it,
/// so there is one focus-reveal behaviour. Pointer presses and the wheel never
/// scroll. `shrink` is egui's `auto_shrink`, so a caller keeps its own sizing.
pub(super) fn show_focus_reveal_scroll<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + Copy,
    shrink: [bool; 2],
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    show_focus_reveal_scroll_inner(ui, salt, shrink, PageKeys::None, false, content)
}

/// As [`show_focus_reveal_scroll`], and a visit that the keyboard started
/// (Enter or Space on the control that opened the page) lands focus on the
/// page's first control.
pub(super) fn show_focus_reveal_scroll_entering<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + Copy,
    shrink: [bool; 2],
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    show_focus_reveal_scroll_inner(ui, salt, shrink, PageKeys::None, true, content)
}

/// Which paging keys a scroller answers. Page scrollers answer PageUp/Down;
/// only Check also takes Home/End, which shelves on other pages already use.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PageKeys {
    None,
    PageOnly,
    Full,
}

fn show_focus_reveal_scroll_inner<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + Copy,
    shrink: [bool; 2],
    keys: PageKeys,
    enter_focus: bool,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = ui.make_persistent_id(salt);
    // A focused text field keeps its own PageUp/Down outside Check, which has
    // only buttons, so existing button focus never blocks paging.
    let paging = match keys {
        PageKeys::None => false,
        PageKeys::PageOnly => !ui.ctx().text_edit_focused(),
        PageKeys::Full => true,
    };
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink(shrink)
        .animated(false);
    // PageUp/Down (and Check's Home/End) belong to the page, not a nested
    // viewport.
    if paging && ui.input(|i| !i.modifiers.alt && !i.modifiers.ctrl) {
        let offset = egui::scroll_area::State::load(ui.ctx(), id)
            .unwrap_or_default()
            .offset
            .y;
        let height = ui.available_height();
        let maximum = ui
            .data(|d| d.get_temp::<f32>(id.with("content_height")))
            .map(|content| (content - height).max(0.0))
            .unwrap_or(0.0);
        let target = ui.input(|i| {
            let ends = keys == PageKeys::Full;
            if ends && i.key_pressed(egui::Key::Home) {
                Some(0.0)
            } else if ends && i.key_pressed(egui::Key::End) {
                Some(maximum)
            } else if i.key_pressed(egui::Key::PageDown) {
                Some((offset + height * 0.9).min(maximum))
            } else if i.key_pressed(egui::Key::PageUp) {
                Some((offset - height * 0.9).max(0.0))
            } else {
                None
            }
        });
        if let Some(target) = target {
            scroll = scroll.vertical_scroll_offset(target);
        }
    }
    let output = scroll.show(ui, |ui| {
        let layer = ui.layer_id();
        let first = widgets_in_layer(ui.ctx(), layer);
        let inner = content(ui);
        let owned = widgets_created_since(ui.ctx(), layer, first);
        if enter_focus {
            let target = first_focusable_since(ui.ctx(), layer, first);
            focus_first_control_on_keyboard_entry(ui, id, target);
        }
        reveal_new_keyboard_focus(ui, egui::Id::new(salt), &owned);
        inner
    });
    if keys != PageKeys::None {
        ui.data_mut(|d| d.insert_temp(id.with("content_height"), output.content_size.y));
    }
    output.inner
}

/// Widgets registered so far in `layer` during this pass. egui only appends,
/// so the entries added between two calls are exactly what ran in between.
fn widgets_in_layer(context: &egui::Context, layer: egui::LayerId) -> usize {
    context.viewport(|viewport| {
        viewport
            .this_pass
            .widgets
            .layers()
            .find(|(candidate, _)| **candidate == layer)
            .map_or(0, |(_, widgets)| widgets.len())
    })
}

/// The widgets a scroller's own content created: what was registered in
/// `layer` after `first`. Status-bar, header and sidebar controls are drawn
/// outside that closure, so they are never in this set whatever their rect.
fn widgets_created_since(
    context: &egui::Context,
    layer: egui::LayerId,
    first: usize,
) -> Vec<egui::Id> {
    context.viewport(|viewport| {
        viewport
            .this_pass
            .widgets
            .layers()
            .find(|(candidate, _)| **candidate == layer)
            .map(|(_, widgets)| widgets.iter().skip(first).map(|widget| widget.id).collect())
            .unwrap_or_default()
    })
}

/// Maximum eligible UI observations for a keyboard reveal, including passes
/// where the control is already visible. Ineligible requests end immediately.
/// Observations spent waiting for a nested scroller are counted separately.
const REVEAL_PASSES: u8 = 8;
/// Maximum observations a valid target may sit outside this scroller's content
/// (a card in a nested shelf that has not scrolled yet) before the request ends.
/// These never consume `REVEAL_PASSES`.
const REVEAL_WAIT_PASSES: u8 = 8;
/// Consecutive passes a revealed control must stay inside the viewport before
/// the reveal is finished. Panels (the status bar) resize one frame late.
const REVEAL_STABLE_PASSES: u8 = 3;

#[cfg(test)]
mod focus_reveal_tests;

/// Reveal newly focused controls owned by this scroller. Manual input cancels
/// the entire pending request; held focus cannot restart it on an idle pass. A
/// target still outside this scroller's content (inside a nested scroller that
/// has not moved yet) keeps waiting on its own bounded budget, so waiting never
/// uses up the scroll attempts.
fn reveal_new_keyboard_focus(ui: &mut egui::Ui, key: egui::Id, owned: &[egui::Id]) {
    let context = ui.ctx().clone();
    let slot = key.with("last_focus");
    let pending_slot = key.with("pending_reveal");
    let focused = context.memory(|memory| memory.focused());
    let before = context
        .data(|data| data.get_temp::<Option<egui::Id>>(slot))
        .flatten();
    context.data_mut(|data| data.insert_temp(slot, focused));
    let Some(id) = focused else {
        context.data_mut(|data| data.insert_temp(pending_slot, None::<(egui::Id, u8, u8)>));
        return;
    };
    let manual = context.input(|input| {
        input.pointer.any_pressed()
            || input.pointer.any_released()
            || input.pointer.any_down()
            // Use actual input, not lingering smooth-scroll deltas, so a new
            // keyboard focus transition after the wheel can reveal again.
            || input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::MouseWheel { .. }))
    });
    // (control, observations spent, consecutive passes inside the viewport). A
    // control taller than its scroller can never fit entirely, and a panel
    // may resize a frame late, so the reveal is bounded but not one-shot.
    let waits_slot = key.with("reveal_waits");
    let mut waits = context
        .data(|data| data.get_temp::<u8>(waits_slot))
        .unwrap_or(0);
    let mut pending = context
        .data(|data| data.get_temp::<Option<(egui::Id, u8, u8)>>(pending_slot))
        .flatten();
    if manual || !owned.contains(&id) {
        pending = None;
    } else if Some(id) != before {
        pending = Some((id, 0, 0));
        waits = 0;
    } else if pending.map(|(pending_id, _, _)| pending_id) != Some(id) {
        pending = None;
    }
    if let Some((_, spent, stable)) = pending {
        // Only a missing response ends the request early. A target outside this
        // scroller's content is normal for a card in a nested scroller (the game
        // shelf) that has not moved yet: wait for it on a separate budget.
        pending = None;
        if let Some(response) = context.read_response(id) {
            let (spent, stable) = if ui.clip_rect().contains_rect(response.rect) {
                (spent.saturating_add(1), stable.saturating_add(1))
            } else if ui.min_rect().intersects(response.rect) {
                ui.scroll_to_rect(response.rect, None);
                context.request_repaint();
                (spent.saturating_add(1), 0)
            } else {
                waits = waits.saturating_add(1);
                (spent, 0)
            };
            if spent < REVEAL_PASSES && stable < REVEAL_STABLE_PASSES && waits < REVEAL_WAIT_PASSES
            {
                context.request_repaint();
                pending = Some((id, spent, stable));
            }
        }
    }
    if pending.is_none() {
        waits = 0;
    }
    context.data_mut(|data| data.insert_temp(waits_slot, waits));
    context.data_mut(|data| data.insert_temp(pending_slot, pending));
}

#[derive(Clone, Default)]
struct RevealLayout {
    focused: Option<egui::Id>,
    selected: Option<egui::Id>,
    size: egui::Vec2,
    /// Rectangles in content coordinates, independent of the scroll offset.
    controls: Vec<(egui::Id, egui::Rect)>,
    clip_start: f32,
    clip_height: f32,
}

/// The nearest absolute offset fitting a rect inside the real clip, with one
/// point for pixel rounding. Repeating this calculation is idempotent.
fn reveal_offset(offset: f32, rect: egui::Rect, clip_start: f32, height: f32) -> f32 {
    let first = rect.max.y - clip_start - height + 1.0;
    let last = rect.min.y - clip_start - 1.0;
    if first <= last {
        offset.clamp(first, last).max(0.0)
    } else {
        // A control taller than the viewport cannot fit. Keep its top readable.
        last.max(0.0)
    }
}

/// Selection changes reveal once; new keyboard focus takes precedence. Pointer
/// focus, wheel input and unchanged repaints never submit a reveal intent.
fn show_reveal_scroll<R>(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + Copy,
    height: f32,
    controls: &[egui::Id],
    selected: Option<egui::Id>,
    content: impl FnOnce(&mut egui::Ui) -> R,
) {
    let id = ui.make_persistent_id(egui::Id::new(salt));
    let layout_id = id.with("simple_reveal_layout");
    let previous = ui
        .data(|data| data.get_temp::<RevealLayout>(layout_id))
        .unwrap_or_default();
    let size = egui::vec2(ui.available_width(), height.min(ui.available_height()));
    let selection_changed = selected != previous.selected || size != previous.size;
    let pointer = ui.input(|input| input.pointer.any_pressed() || input.pointer.any_released());
    let focus = || {
        ui.ctx()
            .memory(|memory| memory.focused())
            .filter(|id| controls.contains(id))
    };
    let intent = |focused: Option<egui::Id>| {
        if !pointer && focused != previous.focused && focused.is_some() {
            focused
        } else if selection_changed {
            selected
        } else {
            None
        }
    };
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(salt)
        .max_height(height)
        .auto_shrink([false, false])
        .animated(false);
    // On a reverse-focus pass the target is known before layout. Use its last
    // content rect, never a relative delta from its earlier screen position.
    if let Some(target) = intent(focus())
        && let Some((_, rect)) = previous.controls.iter().find(|(id, _)| *id == target)
        && previous.clip_height > 0.0
    {
        let offset = egui::scroll_area::State::load(ui.ctx(), id)
            .unwrap_or_default()
            .offset
            .y;
        scroll = scroll.vertical_scroll_offset(reveal_offset(
            offset,
            *rect,
            previous.clip_start,
            previous.clip_height,
        ));
    }
    let mut layout = RevealLayout {
        selected,
        size,
        ..Default::default()
    };
    let mut drawn_offset = 0.0;
    let output = scroll.show_viewport(ui, |ui, viewport| {
        let origin = ui.max_rect().min;
        drawn_offset = viewport.min.y;
        layout.clip_start = ui.clip_rect().min.y - origin.y - drawn_offset;
        layout.clip_height = ui.clip_rect().height();
        let inner = content(ui);
        // Read inside this pass: egui's post-frame read_response can return
        // the preceding pass after a multi-pass frame.
        layout.controls = controls
            .iter()
            .filter_map(|id| {
                ui.ctx()
                    .read_response(*id)
                    .map(|response| (*id, response.rect.translate(-origin.to_vec2())))
            })
            .collect();
        layout.focused = ui
            .ctx()
            .memory(|memory| memory.focused())
            .filter(|id| controls.contains(id));
        inner
    });
    // Forward focus arrives during widget layout. Store one absolute target
    // and discard that stale picture, so its replacement uses the new offset.
    if let Some(target) = intent(layout.focused)
        && let Some((_, rect)) = layout.controls.iter().find(|(id, _)| *id == target)
    {
        let target = reveal_offset(drawn_offset, *rect, layout.clip_start, layout.clip_height)
            .min((output.content_size.y - output.inner_rect.height()).max(0.0));
        if (target - drawn_offset).abs() > 0.01 {
            let mut state = output.state;
            state.offset.y = target;
            state.store(ui.ctx(), output.id);
            ui.ctx()
                .request_discard("display the absolute Simple reveal offset");
        }
    }
    ui.data_mut(|data| data.insert_temp(layout_id, layout));
}

fn task_card(ui: &mut egui::Ui, task: &ManageTask, width: f32) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(width, TASK_CARD_HEIGHT));
    let response = ui.interact(rect, task_id(task.section), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), task.title)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let active = response.hovered() || response.has_focus();
        painter.rect(
            rect,
            CORNER_RADIUS,
            if active {
                palette::HOVER
            } else {
                palette::RAISED_SURFACE
            },
            egui::Stroke::new(
                if response.has_focus() {
                    2.0_f32
                } else {
                    1.0_f32
                },
                if active {
                    palette::ACCENT
                } else {
                    palette::BORDER_SUBTLE
                },
            ),
            egui::StrokeKind::Inside,
        );
        let tile =
            egui::Rect::from_min_size(rect.min + egui::vec2(18.0, 18.0), egui::vec2(46.0, 46.0));
        painter.rect_filled(tile, 10.0, task.tint);
        paint_task_icon(painter, task.icon, tile.center(), egui::Color32::WHITE);
        painter.text(
            rect.min + egui::vec2(18.0, 80.0),
            egui::Align2::LEFT_TOP,
            task.title,
            egui::FontId::proportional(18.0),
            palette::PRIMARY_TEXT,
        );
        let description = painter.layout(
            task.description.to_owned(),
            egui::FontId::proportional(14.5),
            palette::SECONDARY_TEXT,
            rect.width() - 36.0 - 34.0,
        );
        painter.galley(
            rect.min + egui::vec2(18.0, 108.0),
            description,
            palette::SECONDARY_TEXT,
        );
        // The card's safety tag, painted (the card is one button), in the
        // same words and classification as All Tools.
        if let Some(tag) = super::all_tools::section_safety(task.section).tag() {
            let color = tag.color(ui);
            let label = painter.layout_no_wrap(
                tag.label().to_owned(),
                egui::FontId::proportional(12.5),
                color,
            );
            // Top right, beside the icon tile: free at every card width.
            let chip = egui::Rect::from_min_size(
                egui::pos2(
                    rect.right() - 18.0 - label.size().x - 16.0,
                    rect.top() + 18.0,
                ),
                egui::vec2(label.size().x + 16.0, 22.0),
            );
            painter.rect(
                chip,
                7.0,
                color.gamma_multiply(0.14),
                egui::Stroke::new(1.0_f32, color.gamma_multiply(0.7)),
                egui::StrokeKind::Inside,
            );
            painter.galley(
                chip.min + egui::vec2(8.0, (chip.height() - label.size().y) / 2.0),
                label,
                color,
            );
        }
        // The round arrow: this card goes somewhere.
        let arrow = rect.max - egui::vec2(28.0, 28.0);
        painter.circle_stroke(arrow, 13.0, egui::Stroke::new(1.2_f32, palette::ACCENT));
        let stroke = egui::Stroke::new(1.7_f32, palette::ACCENT);
        painter.line_segment(
            [arrow - egui::vec2(5.0, 0.0), arrow + egui::vec2(5.0, 0.0)],
            stroke,
        );
        painter.line_segment(
            [arrow + egui::vec2(1.0, -4.0), arrow + egui::vec2(5.0, 0.0)],
            stroke,
        );
        painter.line_segment(
            [arrow + egui::vec2(1.0, 4.0), arrow + egui::vec2(5.0, 0.0)],
            stroke,
        );
    }
    response
}

fn paint_task_icon(painter: &egui::Painter, icon: TaskIcon, c: egui::Pos2, color: egui::Color32) {
    use egui::{Shape, vec2};
    let stroke = egui::Stroke::new(2.0_f32, color);
    match icon {
        TaskIcon::Search => {
            painter.circle_stroke(c + vec2(-2.0, -2.0), 7.5, stroke);
            painter.line_segment([c + vec2(3.6, 3.6), c + vec2(10.0, 10.0)], stroke);
        }
        TaskIcon::Folder => {
            painter.add(Shape::closed_line(
                vec![
                    c + vec2(-11.0, -8.0),
                    c + vec2(-3.0, -8.0),
                    c + vec2(0.0, -4.5),
                    c + vec2(11.0, -4.5),
                    c + vec2(11.0, 9.0),
                    c + vec2(-11.0, 9.0),
                ],
                stroke,
            ));
        }
        TaskIcon::Copies => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c + vec2(-3.0, -3.0), vec2(14.0, 17.0)),
                2.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.add(Shape::line(
                vec![
                    c + vec2(8.0, -4.0),
                    c + vec2(8.0, 10.0),
                    c + vec2(-5.0, 10.0),
                ],
                stroke,
            ));
        }
        TaskIcon::Picture => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, vec2(22.0, 17.0)),
                2.5,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.add(Shape::line(
                vec![
                    c + vec2(-8.0, 5.0),
                    c + vec2(-2.5, -1.0),
                    c + vec2(2.0, 3.5),
                    c + vec2(5.0, 0.5),
                    c + vec2(8.5, 5.0),
                ],
                stroke,
            ));
            painter.circle_filled(c + vec2(5.0, -4.0), 1.8, color);
        }
        TaskIcon::Wrench => {
            painter.line_segment(
                [c + vec2(-9.0, 9.0), c + vec2(2.0, -2.0)],
                egui::Stroke::new(3.4_f32, color),
            );
            painter.circle_stroke(c + vec2(5.5, -5.5), 5.2, stroke);
        }
        TaskIcon::Star => {
            let points: Vec<_> = (0..10)
                .map(|index| {
                    let radius = if index % 2 == 0 { 10.0 } else { 4.4 };
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.0;
                    c + vec2(radius * angle.cos(), radius * angle.sin())
                })
                .collect();
            painter.add(Shape::closed_line(points, stroke));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_v2::routes::SECTIONS;

    fn luminance(color: egui::Color32) -> f32 {
        let channel = |value: u8| {
            let value = value as f32 / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(foreground: egui::Color32, background: egui::Color32) -> f32 {
        let (a, b) = (luminance(foreground), luminance(background));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn there_are_exactly_eight_distinct_destinations_on_existing_routes() {
        assert_eq!(DESTINATIONS.len(), 8);
        let labels: Vec<_> = DESTINATIONS.iter().map(|d| d.label()).collect();
        assert_eq!(
            labels,
            [
                "Home",
                "Library",
                "Setup",
                "Organise & Export",
                "Tools",
                "Enhance",
                "Health & Recovery",
                "Settings"
            ]
        );
        let routes: Vec<_> = DESTINATIONS.iter().filter_map(|d| d.route()).collect();
        // Every destination but Tools (the All Tools browser) opens a route.
        assert_eq!(routes.len(), 7);
        assert_eq!(Destination::Tools.route(), None);
        for (index, route) in routes.iter().enumerate() {
            assert!(!routes[..index].contains(route), "{route:?} is used twice");
            // Each destination opens a location it also highlights.
            let destination = destination_for(route).unwrap();
            assert_eq!(destination.route().as_ref(), Some(route));
        }
    }

    #[test]
    fn every_section_highlights_one_row_except_activity() {
        // `destination_for` is exhaustive over `Section`, so a new section
        // cannot be added without deciding where it belongs. Activity is
        // reached from the status bar and highlights nothing (its boards).
        let unhighlighted: Vec<_> = SECTIONS
            .iter()
            .copied()
            .filter(|section| destination_for(&Route::Section(*section)).is_none())
            .collect();
        assert_eq!(unhighlighted, [Section::Activity]);
        // Spot checks against the approved boards.
        for (section, destination) in [
            (Section::Duplicates, Destination::Library),
            (Section::Check, Destination::Library),
            (Section::Dat, Destination::OrganiseExport),
            (Section::Mame, Destination::OrganiseExport),
            (Section::Platforms, Destination::Tools),
            (Section::Museum, Destination::Tools),
            (Section::Converter, Destination::Tools),
            (Section::Mods, Destination::Enhance),
            (Section::Saves, Destination::Enhance),
            (Section::Problems, Destination::HealthRecovery),
            (Section::History, Destination::HealthRecovery),
            (Section::Firmware, Destination::Setup),
            (Section::Advanced, Destination::Settings),
        ] {
            assert_eq!(
                destination_for(&Route::Section(section)),
                Some(destination),
                "{section:?}"
            );
        }
    }

    #[test]
    fn game_and_task_routes_keep_their_destination() {
        assert_eq!(destination_for(&Route::Game(7)), Some(Destination::Library));
        assert_eq!(
            destination_for(&Route::BrowsePlayGame(7)),
            Some(Destination::Library)
        );
        assert_eq!(
            destination_for(&Route::Task {
                section: Section::Mods,
                game: 7
            }),
            Some(Destination::Enhance)
        );
        assert_eq!(
            destination_for(&Route::PlatformCheck("SNES".into())),
            Some(Destination::Library)
        );
        assert_eq!(
            destination_for(&Route::MameWorkflow),
            Some(Destination::OrganiseExport)
        );
        assert_eq!(
            destination_for(&Route::QuickRename),
            Some(Destination::OrganiseExport)
        );
    }

    #[test]
    fn shell_sizes_follow_the_approved_measurements_and_width_classes() {
        let wide = shell_metrics(1920.0, 1080.0);
        assert_eq!(
            (wide.sidebar, wide.header, wide.status),
            (232.0, 64.0, 44.0)
        );
        assert!(!wide.rail);
        let standard = shell_metrics(1440.0, 900.0);
        assert_eq!(
            (standard.sidebar, standard.header, standard.status),
            (208.0, 60.0, 40.0)
        );
        let compact = shell_metrics(1024.0, 600.0);
        assert!(compact.rail);
        assert_eq!(
            (compact.sidebar, compact.header, compact.status),
            (RAIL_WIDTH, 54.0, 36.0)
        );
        // Across the app's own width classes (760 and 1240).
        assert!(shell_metrics(759.0, 900.0).rail);
        assert!(shell_metrics(1239.0, 900.0).rail);
        assert!(!shell_metrics(1240.0, 900.0).rail);
        // A short window uses the rail even when wide.
        assert!(shell_metrics(1920.0, 600.0).rail);
    }

    #[test]
    fn capability_map_never_claims_a_capability_without_a_route() {
        let map = capability_map();
        for entry in &map {
            match entry.capability {
                Capability::Existing | Capability::PreviewOnly => assert!(
                    entry.route.is_some(),
                    "{} is {:?} but names no route",
                    entry.control,
                    entry.capability
                ),
                Capability::Unavailable => assert!(
                    entry.route.is_none(),
                    "{} is unavailable and must not route anywhere",
                    entry.control
                ),
                Capability::New => {}
            }
            assert!(!entry.basis.is_empty() && !entry.slice.is_empty());
        }
        // Every destination is covered.
        for destination in DESTINATIONS {
            assert!(map.iter().any(|entry| entry.destination == destination));
        }
    }

    #[test]
    fn grouped_cheat_apply_and_undo_are_not_claimed() {
        let map = capability_map();
        let grouped: Vec<_> = map
            .iter()
            .filter(|entry| {
                entry.destination == Destination::Enhance
                    && (entry.control.contains("Apply") || entry.control.contains("Undo"))
            })
            .collect();
        assert_eq!(grouped.len(), 2);
        for entry in grouped {
            assert_eq!(
                entry.capability,
                Capability::Unavailable,
                "{}",
                entry.control
            );
        }
        // Nothing that writes is marked Existing for Cheats & Mods.
        assert!(map.iter().all(|entry| {
            entry.destination != Destination::Enhance
                || entry.capability != Capability::Existing
                || !entry.control.to_lowercase().contains("apply")
        }));
    }

    #[test]
    fn blue_palette_keeps_text_readable() {
        for background in [
            palette::APP_BACKGROUND,
            palette::SIDEBAR,
            palette::CARD_SURFACE,
            palette::RAISED_SURFACE,
            palette::HEADER,
            palette::SELECTED,
            palette::SECONDARY_ACTION,
            palette::HOVER,
        ] {
            let ratio = contrast(palette::PRIMARY_TEXT, background);
            assert!(
                ratio >= 4.5,
                "primary text on {background:?} is {ratio:.2}:1"
            );
        }
        for background in [
            palette::APP_BACKGROUND,
            palette::SIDEBAR,
            palette::CARD_SURFACE,
        ] {
            let ratio = contrast(palette::SECONDARY_TEXT, background);
            assert!(
                ratio >= 4.5,
                "secondary text on {background:?} is {ratio:.2}:1"
            );
        }
        // White on the primary action must reach AA; this is why the reference
        // #0c84fd (3.66:1) was darkened.
        assert!(contrast(egui::Color32::WHITE, palette::PRIMARY_ACTION) >= 4.5);
        assert!(contrast(palette::ACCENT, palette::APP_BACKGROUND) >= 3.0);
        assert!(contrast(palette::ACCENT, palette::SELECTED) >= 3.0);
    }

    #[test]
    fn blue_tokens_match_the_approved_table() {
        use crate::ui::theme::blue;
        let hex =
            |color: egui::Color32| format!("{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
        for (token, expected) in [
            (blue::APP_BACKGROUND, "0A1224"),
            (blue::DEEP_BACKGROUND, "080F1F"),
            (blue::CARD_SURFACE, "0E1A33"),
            (blue::CARD_SURFACE_TOP, "112042"),
            (blue::INSET, "0B1630"),
            (blue::RAISED_SURFACE, "1A2D52"),
            (blue::BORDER_SUBTLE, "1F3358"),
            (blue::BORDER_STRONG, "2E4A7D"),
            (blue::SELECTED_FILL, "17356D"),
            (blue::SELECTED_OUTLINE, "4C95FF"),
            (blue::PRIMARY_TEXT, "F5F9FF"),
            (blue::BODY_TEXT, "C6D2EA"),
            (blue::SECONDARY_TEXT, "AEBCD8"),
            (blue::DIM_TEXT, "8496B8"),
            (blue::ACCENT, "6AB0FF"),
            (blue::OK, "34D399"),
            (blue::WARN, "F5B83D"),
            (blue::BAD, "F87171"),
            (blue::BAD_TEXT, "FCA5A5"),
            (blue::UNDO, "6B5CF0"),
        ] {
            assert_eq!(hex(token), expected);
        }
        // The one permitted deviation: the primary action is the approved
        // #2F7BFF darkened along the same hue for text contrast.
        assert_eq!(hex(blue::PRIMARY_ACTION), "296CE0");
        assert_eq!(blue::FOCUS_RING_WIDTH, 3.0);
        assert_eq!(blue::FOCUS_RING_GAP, 3.0);
    }

    #[test]
    fn primary_text_and_primary_buttons_reach_aa_contrast() {
        use crate::ui::theme::blue;
        for background in [
            blue::APP_BACKGROUND,
            blue::DEEP_BACKGROUND,
            blue::CARD_SURFACE,
            blue::CARD_SURFACE_TOP,
            blue::INSET,
            blue::RAISED_SURFACE,
            blue::SELECTED_FILL,
        ] {
            for text in [blue::PRIMARY_TEXT, blue::BODY_TEXT, blue::SECONDARY_TEXT] {
                let ratio = contrast(text, background);
                assert!(ratio >= 4.5, "{text:?} on {background:?} is {ratio:.2}:1");
            }
        }
        // Dim labels on the page and card surfaces they are used on.
        for background in [blue::APP_BACKGROUND, blue::CARD_SURFACE, blue::INSET] {
            assert!(contrast(blue::DIM_TEXT, background) >= 4.5);
        }
        // Primary, destructive and Undo buttons carry primary text.
        assert!(contrast(blue::PRIMARY_TEXT, blue::PRIMARY_ACTION) >= 4.5);
        assert!(contrast(blue::PRIMARY_TEXT, blue::UNDO) >= 4.5);
        let destructive = egui::Color32::from_rgb(0xb4, 0x2a, 0x2a);
        assert!(contrast(blue::PRIMARY_TEXT, destructive) >= 4.5);
        // Status colours read as text on cards, and the focus ring stands out.
        for status in [
            blue::OK,
            blue::WARN,
            blue::BAD,
            blue::BAD_TEXT,
            blue::UNDO_TEXT,
        ] {
            assert!(contrast(status, blue::CARD_SURFACE) >= 4.5);
        }
        assert!(contrast(blue::ACCENT, blue::APP_BACKGROUND) >= 3.0);
        assert!(contrast(blue::ACCENT, blue::CARD_SURFACE) >= 3.0);
    }

    #[test]
    fn scale_is_clamped_stepped_and_never_non_finite() {
        assert_eq!(clamp_scale(1.0), 1.0);
        assert_eq!(clamp_scale(0.1), UI_SCALE_MIN);
        assert_eq!(clamp_scale(9.0), UI_SCALE_MAX);
        assert_eq!(clamp_scale(f32::NAN), 1.0);
        assert_eq!(clamp_scale(f32::INFINITY), 1.0);
        assert!((clamp_scale(1.24) - 1.2).abs() < 1e-6);
    }

    #[test]
    fn blue_navigation_is_the_default_keeps_its_chrome_for_all_tools_and_yields_to_classic() {
        let mut shell = ShellState::default();
        assert!(shell.enabled && shell.simple_navigation() && !shell.all_tools_open());
        shell.all_tools = true;
        // The browser opens inside the Blue chrome.
        assert!(shell.simple_navigation() && shell.all_tools_open());
        shell.enabled = false;
        assert!(!shell.simple_navigation() && !shell.all_tools_open());
    }

    fn preferences_from(json: &str) -> crate::gui_v2::backend::Preferences {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_fresh_installation_opens_emuwiz_blue() {
        // No preferences file: the backend restores the defaults.
        assert!(blue_enabled(&crate::gui_v2::backend::Preferences::default()));
        assert!(blue_enabled(&preferences_from("{}")));
        // And the first frame, before preferences arrive, is already Blue.
        assert!(ShellState::default().enabled);
    }

    #[test]
    fn existing_users_from_the_opt_in_preview_era_open_emuwiz_blue() {
        // Every earlier save wrote `simple_shell`, so `false` there means
        // "never opted in to the preview", not a choice of Classic.
        let before = preferences_from(r#"{"route":"Home","simple_shell":false,"ui_scale":1.2}"#);
        assert!(blue_enabled(&before));
        assert!(
            (before.ui_scale - 1.2).abs() < 1e-6,
            "other settings survive"
        );
        assert!(blue_enabled(&preferences_from(r#"{"simple_shell":true}"#)));
    }

    #[test]
    fn an_explicit_classic_choice_is_kept_across_restarts() {
        let chosen = preferences_from(r#"{"simple_shell":true,"classic_view":true}"#);
        assert!(!blue_enabled(&chosen));
    }

    #[test]
    fn switching_views_persists_and_stays_readable_by_older_builds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gui-v2.json");
        for blue in [false, true] {
            let preferences = crate::gui_v2::backend::Preferences {
                simple_shell: blue,
                classic_view: !blue,
                ..Default::default()
            };
            crate::gui_v2::backend::save_preferences(&path, &preferences).unwrap();
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            // An older build reads only `simple_shell` and opens the same view.
            assert_eq!(raw["simple_shell"], serde_json::Value::Bool(blue));
            let restored = preferences_from(&raw.to_string());
            assert_eq!(blue_enabled(&restored), blue);
        }
    }

    #[test]
    fn classic_fallback_leaves_the_style_untouched_and_blue_is_fully_restored_when_off() {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        let classic = (*context.style()).clone();
        let mut shell = ShellState {
            enabled: false,
            ..Default::default()
        };
        shell.sync_style(&context);
        assert_eq!(*context.style(), classic);
        assert_eq!(context.zoom_factor(), 1.0);

        for scale in [1.3, 0.8, 1.6] {
            shell.enabled = true;
            shell.set_ui_scale(scale);
            shell.sync_style(&context);
            // egui applies a requested zoom at the start of the next frame.
            let _ = context.run(egui::RawInput::default(), |_| {});
            assert_eq!(context.style().visuals.panel_fill, palette::APP_BACKGROUND);
            assert_ne!(*context.style(), classic);
            assert!((context.zoom_factor() - scale).abs() < 1e-6);

            shell.enabled = false;
            shell.sync_style(&context);
            let _ = context.run(egui::RawInput::default(), |_| {});
            // The whole style, not selected fields. Classic keeps the saved
            // scale (Slice 2A); only the style returns to its own.
            assert_eq!(*context.style(), classic);
            assert!((context.zoom_factor() - scale).abs() < 1e-6);
        }
    }

    #[test]
    fn an_outside_restyle_is_corrected_without_forgetting_classic() {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        let classic = (*context.style()).clone();
        let mut shell = ShellState {
            enabled: true,
            ..Default::default()
        };
        shell.sync_style(&context);
        let blue = (*context.style()).clone();
        // What the embedded workflow host does when it is first built.
        crate::gui_v2::readable_style(&context);
        assert_ne!(*context.style(), blue);
        shell.sync_style(&context);
        assert_eq!(*context.style(), blue);
        // A settled frame changes nothing further.
        let held = context.style();
        shell.sync_style(&context);
        assert!(Arc::ptr_eq(&held, &context.style()));
        shell.enabled = false;
        shell.sync_style(&context);
        assert_eq!(*context.style(), classic);
    }

    #[test]
    fn navigation_labels_tighten_then_shrink_but_never_overrun_the_row() {
        // Roomy: the reference spacing at full size.
        assert_eq!(nav_metrics(216.0, 118.0), (28.0, 56.0, NAV_LABEL_SIZE));
        // Narrow: tighter spacing first...
        assert_eq!(nav_metrics(172.0, 110.0), (22.0, 48.0, NAV_LABEL_SIZE));
        // ...then a smaller label, within the readable floor.
        for (width, label) in [(172.0, 122.0), (150.0, 122.0), (120.0, 122.0)] {
            let (_, text_x, size) = nav_metrics(width, label);
            assert!((NAV_LABEL_MIN_SIZE..=NAV_LABEL_SIZE).contains(&size));
            if size > NAV_LABEL_MIN_SIZE {
                assert!(text_x + label * size / NAV_LABEL_SIZE <= width - 9.5);
            }
        }
    }

    #[test]
    fn manage_library_offers_the_six_board_tasks_on_existing_sections() {
        let titles: Vec<_> = MANAGE_TASKS.iter().map(|task| task.title).collect();
        assert_eq!(
            titles,
            [
                "Check & Identify",
                "Organise & Export",
                "Review Duplicates",
                "Convert Media",
                "Fix Problems",
                "Cheats & Mods"
            ]
        );
        let sections: Vec<_> = MANAGE_TASKS.iter().map(|task| task.section).collect();
        assert_eq!(
            sections,
            [
                Section::Check,
                Section::Build,
                Section::Duplicates,
                Section::Converter,
                Section::Problems,
                Section::Mods
            ]
        );
        for task in MANAGE_TASKS {
            // Each card opens a location with an approved sidebar row...
            assert!(destination_for(&Route::Section(task.section)).is_some());
            // ...and its words describe a tool, never an outcome.
            let words = format!("{} {}", task.title, task.description).to_lowercase();
            for claim in ["verified", "complete", "healthy", "ready", "fixed", "done"] {
                assert!(!words.contains(claim), "{} claims {claim}", task.title);
            }
            // White icon on the coloured tile stays legible.
            assert!(
                contrast(egui::Color32::WHITE, task.tint) >= 3.0,
                "{}",
                task.title
            );
        }
    }

    #[test]
    fn manage_library_grid_reflows_from_three_across_to_one() {
        assert_eq!(task_grid(1380.0).0, 3);
        assert_eq!(task_grid(760.0).0, 3);
        assert_eq!(task_grid(230.0).0, 1);
        for available in [230.0, 500.0, 760.0, 1000.0, 1380.0, 2400.0] {
            let (columns, width) = task_grid(available);
            let used = columns as f32 * width + (columns as f32 - 1.0) * TASK_CARD_GAP;
            assert!(used <= available + 0.5, "{available}: {columns} x {width}");
        }
    }
}
