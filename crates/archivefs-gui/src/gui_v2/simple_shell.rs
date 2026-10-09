//! Simple Mode v1 shell (slices 0 and 1): presentation only.
//!
//! This module owns three things and nothing else:
//!
//! 1. the seven Simple destinations and how each maps onto an *existing*
//!    [`Route`] - no route, section or deep link is added, renamed or removed;
//! 2. the slice-0 capability map: every control the Simple Mode v1 plan shows,
//!    classified by whether a backend capability exists for it today;
//! 3. the blue style profile and the compact sidebar painted when the opt-in
//!    `Simple v1` preference is on.
//!
//! It dispatches no backend command, reads no file and changes no page. When
//! the preference is off (the default) nothing here runs and the interface is
//! exactly the existing one. When it is on, **All Tools** restores the complete
//! existing navigation at any time.

use std::sync::Arc;

use eframe::egui;

use super::routes::{Route, Section};

// --------------------------------------------------------------------- state

pub(super) const UI_SCALE_MIN: f32 = 0.8;
pub(super) const UI_SCALE_MAX: f32 = 1.6;
pub(super) const UI_SCALE_STEP: f32 = 0.1;

/// Presentation state. `enabled` and `ui_scale` are saved preferences;
/// `all_tools` is a per-session view switch and is never persisted, so a
/// restart always returns to the short sidebar.
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
    fn default() -> Self {
        Self {
            enabled: false,
            all_tools: false,
            ui_scale: 1.0,
            classic: None,
            blue: None,
            zoom: None,
        }
    }
}

impl ShellState {
    /// The short sidebar is shown only when Simple is on and All Tools is not.
    pub fn simple_navigation(&self) -> bool {
        self.enabled && !self.all_tools
    }

    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = clamp_scale(scale);
    }

    /// Keeps the context style and zoom in step with the preference. Cheap
    /// when nothing changed: one pointer comparison per frame.
    ///
    /// With Simple never turned on this touches nothing, so the default
    /// interface is exactly the host's.
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
        } else if let Some(classic) = self.classic.take() {
            // Every slot the blue profile touched, restored at once.
            context.set_style(classic);
            self.blue = None;
            self.set_zoom(context, 1.0);
        }
    }

    fn set_zoom(&mut self, context: &egui::Context, zoom: f32) {
        if self.zoom != Some(zoom) {
            context.set_zoom_factor(zoom);
            self.zoom = Some(zoom);
        }
    }
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

/// The seven primary Simple destinations, in sidebar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Destination {
    Home,
    Library,
    CheatsMods,
    ManageLibrary,
    SetupDoctor,
    ActivityHistory,
    Settings,
}

pub(super) const DESTINATIONS: [Destination; 7] = [
    Destination::Home,
    Destination::Library,
    Destination::CheatsMods,
    Destination::ManageLibrary,
    Destination::SetupDoctor,
    Destination::ActivityHistory,
    Destination::Settings,
];

impl Destination {
    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Library => "Library",
            Self::CheatsMods => "Cheats & Mods",
            Self::ManageLibrary => "Manage Library",
            Self::SetupDoctor => "Setup",
            Self::ActivityHistory => "Activity",
            Self::Settings => "Settings",
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::Home => "Pick up where you left off.",
            Self::Library => "Browse and open your games.",
            Self::CheatsMods => "Cheats and mods for a game.",
            Self::ManageLibrary => "Check, organise, convert and fix.",
            Self::SetupDoctor => "Game folders, emulators and firmware.",
            Self::ActivityHistory => "Running jobs, results and undo.",
            Self::Settings => "Appearance and the full tool list.",
        }
    }

    /// The existing route this destination opens. Every one already exists.
    pub fn route(self) -> Route {
        match self {
            Self::Home => Route::Home,
            Self::Library => Route::BrowsePlay,
            Self::CheatsMods => Route::Section(Section::Mods),
            // The existing Organisation-family location. In the Simple view it
            // is painted as the Manage Library hub (five cards onto existing
            // routes); All Tools still shows the Organisation overview there.
            Self::ManageLibrary => Route::Section(Section::OrganisationFamily),
            Self::SetupDoctor => Route::Section(Section::Setup),
            Self::ActivityHistory => Route::Section(Section::Activity),
            Self::Settings => Route::Section(Section::Settings),
        }
    }
}

/// Which destination, if any, a location belongs to. `None` means the person
/// is somewhere only All Tools lists; the sidebar then highlights nothing
/// rather than a wrong entry.
pub(super) fn destination_for(route: &Route) -> Option<Destination> {
    if matches!(
        route,
        Route::BrowsePlay | Route::BrowsePlayGame(_) | Route::Game(_)
    ) {
        return Some(Destination::Library);
    }
    Some(match route.section() {
        Section::Home => Destination::Home,
        Section::Games | Section::Platforms | Section::Museum | Section::Launch => {
            Destination::Library
        }
        Section::Mods | Section::CheatsMods => Destination::CheatsMods,
        Section::Check
        | Section::Problems
        | Section::ProblemsRepair
        | Section::Build
        | Section::OrganisationFamily
        | Section::Duplicates
        | Section::Converter
        | Section::Conversion
        | Section::Storage
        | Section::MultiDisc => Destination::ManageLibrary,
        Section::Setup | Section::Emulators | Section::EmulatorsFamily | Section::Firmware => {
            Destination::SetupDoctor
        }
        Section::Activity | Section::History | Section::HistoryUndo => Destination::ActivityHistory,
        Section::Settings => Destination::Settings,
        Section::Saves
        | Section::SavesStates
        | Section::Tape
        | Section::Artwork
        | Section::ArtworkExtras
        | Section::Sources
        | Section::SourcesProviders
        | Section::Romm
        | Section::Dat
        | Section::DatVerification
        | Section::Mame
        | Section::Advanced
        | Section::AdvancedDiagnostics => return None,
    })
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
        ActivityHistory, CheatsMods, Home, Library, ManageLibrary, Settings, SetupDoctor,
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
            CheatsMods,
            "Selected game and emulator context",
            Existing,
            section(Section::Mods),
            "Mods & Cheats page with its game context.",
            "4a",
        ),
        control(
            CheatsMods,
            "Searchable cheat checkbox list",
            New,
            section(Section::Mods),
            "Cheat sources and planners exist; the checkbox catalogue is new presentation.",
            "4a",
        ),
        control(
            CheatsMods,
            "Preview changes",
            PreviewOnly,
            section(Section::Mods),
            "Existing per-adapter planners can preview; no grouped preview contract.",
            "4a",
        ),
        control(
            CheatsMods,
            "Apply N cheats (grouped)",
            Unavailable,
            None,
            "No typed multi-file group-operation contract exists.",
            "4b",
        ),
        control(
            CheatsMods,
            "Grouped Undo",
            Unavailable,
            None,
            "Independent journals are not one atomic Undo; needs a parent transaction.",
            "4b",
        ),
        control(
            ManageLibrary,
            "Check & Identify",
            Existing,
            section(Section::Check),
            "Existing Check Games route.",
            "5",
        ),
        control(
            ManageLibrary,
            "Organise & Export",
            Existing,
            section(Section::Build),
            "Existing Organisation route.",
            "5",
        ),
        control(
            ManageLibrary,
            "Review Duplicates",
            Existing,
            section(Section::Duplicates),
            "Existing Duplicates route.",
            "5",
        ),
        control(
            ManageLibrary,
            "Convert Media",
            Existing,
            section(Section::Converter),
            "Existing Converter route.",
            "5",
        ),
        control(
            ManageLibrary,
            "Fix Problems",
            Existing,
            section(Section::Problems),
            "Existing Problems & Repair route.",
            "5",
        ),
        control(
            ManageLibrary,
            "What / Why / Preview / Confirm / Result wizard",
            New,
            None,
            "Wizard framing over the existing routes; no new mutation path.",
            "5",
        ),
        control(
            SetupDoctor,
            "Find emulators / Add game folder / Check setup",
            Existing,
            section(Section::Setup),
            "Existing Setup & Doctor route.",
            "5",
        ),
        control(
            SetupDoctor,
            "Firmware status",
            Existing,
            section(Section::Firmware),
            "Existing BIOS / Firmware route.",
            "5",
        ),
        control(
            ActivityHistory,
            "Running, waiting and finished jobs",
            Existing,
            section(Section::Activity),
            "Existing Activity route.",
            "6",
        ),
        control(
            ActivityHistory,
            "History and eligible Undo",
            Existing,
            section(Section::History),
            "The one existing History system decides Undo eligibility.",
            "6",
        ),
        control(
            Settings,
            "Simple v1 opt-in and text scale",
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

/// Blue profile, sampled from the 7 October reference mock-ups. Scoped to this
/// module so the stock v2 palette in `ui::theme` is untouched while Simple is
/// opt-in.
pub(super) mod palette {
    use eframe::egui::Color32;

    pub const APP_BACKGROUND: Color32 = Color32::from_rgb(0x0d, 0x18, 0x25);
    pub const DEEP_BACKGROUND: Color32 = Color32::from_rgb(0x0a, 0x12, 0x1c);
    pub const SIDEBAR: Color32 = Color32::from_rgb(0x0f, 0x19, 0x26);
    pub const HEADER: Color32 = Color32::from_rgb(0x11, 0x20, 0x32);
    pub const CARD_SURFACE: Color32 = Color32::from_rgb(0x10, 0x21, 0x33);
    pub const RAISED_SURFACE: Color32 = Color32::from_rgb(0x14, 0x25, 0x39);
    pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x1e, 0x33, 0x4a);
    /// Selected navigation pill and other selected states: deliberately dim.
    pub const SELECTED: Color32 = Color32::from_rgb(0x0d, 0x3c, 0x74);
    pub const HOVER: Color32 = Color32::from_rgb(0x16, 0x2c, 0x45);
    /// Outlined secondary action (All Tools).
    pub const SECONDARY_ACTION: Color32 = Color32::from_rgb(0x15, 0x49, 0x88);
    /// The mock-up's `#0c84fd`, darkened until white text reaches 4.5:1.
    pub const PRIMARY_ACTION: Color32 = Color32::from_rgb(0x0b, 0x72, 0xdc);
    /// Bright accent for the selected-item edge, outlines and keyboard focus.
    pub const ACCENT: Color32 = Color32::from_rgb(0x3f, 0xa2, 0xff);
    pub const PRIMARY_TEXT: Color32 = Color32::from_rgb(0xee, 0xf4, 0xff);
    pub const SECONDARY_TEXT: Color32 = Color32::from_rgb(0x9f, 0xb3, 0xcc);
}

pub(super) const SIDEBAR_WIDTH: f32 = 240.0;
pub(super) const SIDEBAR_WIDTH_NARROW: f32 = 196.0;
pub(super) const HEADER_HEIGHT: f32 = 62.0;
/// Navigation rows are one tall, easy target.
pub(super) const NAV_TARGET_HEIGHT: f32 = 48.0;
pub(super) const SEARCH_WIDTH_MAX: f32 = 490.0;
pub(super) const SEARCH_WIDTH_MIN: f32 = 200.0;
pub(super) const PAGE_TITLE_SIZE: f32 = 32.0;
const CORNER_RADIUS: u8 = 12;
const ICON_SIDE: f32 = 22.0;

/// Turns a classic style into the blue profile. Works on a value, never on
/// the live context, so the result is the same whatever happened in between.
fn blue_style(style: &mut egui::Style) {
    {
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::proportional(PAGE_TITLE_SIZE),
        );
        style.spacing.button_padding = egui::vec2(16.0, 11.0);
        style.spacing.interact_size.y = 44.0;
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
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.corner_radius = radius;
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
        // The bright blue is reserved for the pressed/primary moment.
        visuals.widgets.active.bg_fill = palette::PRIMARY_ACTION;
        visuals.widgets.active.weak_bg_fill = palette::PRIMARY_ACTION;
        visuals.widgets.active.bg_stroke = egui::Stroke::new(2.0_f32, palette::ACCENT);
        visuals.widgets.open.bg_fill = palette::SELECTED;
        visuals.widgets.open.weak_bg_fill = palette::SELECTED;
        visuals.window_corner_radius = radius;
        visuals.menu_corner_radius = radius;
        visuals.window_stroke = egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE);
    }
}

pub(super) fn sidebar_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(palette::SIDEBAR)
        .inner_margin(egui::Margin::symmetric(12, 12))
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
        Destination::CheatsMods => {
            let points: Vec<_> = (0..10)
                .map(|index| {
                    let radius = if index % 2 == 0 { 9.5 } else { 4.2 };
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.0;
                    at(radius * angle.cos(), radius * angle.sin())
                })
                .collect();
            painter.add(Shape::closed_line(points, stroke));
        }
        Destination::ManageLibrary => {
            for y in [-6.0, 0.0, 6.0] {
                painter.add(Shape::ellipse_stroke(
                    at(0.0, y),
                    vec2(8.0, 2.8) * unit,
                    stroke,
                ));
            }
            painter.line_segment([at(-8.0, -6.0), at(-8.0, 6.0)], stroke);
            painter.line_segment([at(8.0, -6.0), at(8.0, 6.0)], stroke);
        }
        Destination::SetupDoctor => {
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
        Destination::ActivityHistory => {
            painter.circle_stroke(c, 9.0 * unit, stroke);
            painter.add(Shape::line(
                vec![at(0.0, -5.0), at(0.0, 0.0), at(4.0, 2.5)],
                stroke,
            ));
        }
        Destination::Settings => {
            painter.circle_stroke(c, 3.2 * unit, stroke);
            painter.circle_stroke(c, 7.0 * unit, stroke);
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = angle.sin_cos();
                painter.line_segment([at(7.0 * cos, 7.0 * sin), at(9.8 * cos, 9.8 * sin)], stroke);
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

/// Paints the seven-destination sidebar and returns the destination chosen
/// this frame. Navigation itself stays with the caller's router. `mascot` is
/// the bundled EmuWiz badge when it has been decoded; the brand reads fine
/// without it.
pub(super) fn show_sidebar(
    ui: &mut egui::Ui,
    current: &Route,
    mascot: Option<&egui::TextureHandle>,
) -> Option<Destination> {
    let selected = destination_for(current);
    let mut chosen = None;
    let narrow = ui.available_width() < 190.0;
    ui.horizontal(|ui| {
        if let Some(texture) = mascot {
            let side = if narrow { 36.0 } else { 46.0 };
            ui.add(
                egui::Image::new((texture.id(), egui::vec2(side, side)))
                    .sense(egui::Sense::hover()),
            );
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(
                egui::RichText::new("EmuWiz")
                    .size(if narrow { 22.0 } else { 26.0 })
                    .strong(),
            );
            // Wraps rather than running under the sidebar edge.
            ui.add(
                egui::Label::new(
                    egui::RichText::new("Play More. Manage Better.")
                        .size(12.5)
                        .color(palette::SECONDARY_TEXT),
                )
                .wrap(),
            );
        });
    });
    ui.add_space(if narrow { 10.0 } else { 18.0 });
    // The destinations scroll when the window is short or the interface is
    // enlarged; the version footer keeps its place below them. Nothing is
    // dropped: a row that does not fit is one wheel turn or Tab away.
    let rows = (ui.available_height() - FOOTER_HEIGHT).max(NAV_TARGET_HEIGHT);
    // The selected row is brought into view when the selection changes or the
    // room for rows does (a restored session, a link from a page, a resize, a
    // new scale) - and on no other frame, so wheel scrolling is never undone.
    let shown_for = (selected, rows.round() as i32);
    let shown_id = egui::Id::new("v2_simple_selected_shown");
    let reveal_selected = ui.data(|data| data.get_temp(shown_id)) != Some(shown_for);
    if reveal_selected {
        ui.data_mut(|data| data.insert_temp(shown_id, shown_for));
    }
    egui::ScrollArea::vertical()
        .id_salt("v2_simple_destinations")
        .max_height(rows)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            for destination in DESTINATIONS {
                let is_selected = selected == Some(destination);
                let reveal = reveal_selected && is_selected;
                if nav_item(ui, destination, is_selected, reveal).clicked() {
                    chosen = Some(destination);
                }
            }
            if selected.is_none() {
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "You are in All Tools: {}",
                            current.section().title()
                        ))
                        .size(14.0)
                        .color(palette::SECONDARY_TEXT),
                    )
                    .wrap(),
                );
            }
        });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(concat!("EmuWiz v", env!("CARGO_PKG_VERSION")))
            .size(14.0)
            .color(palette::SECONDARY_TEXT),
    );
    chosen
}

/// The stable identity of a destination's sidebar row (focus, tests).
pub(super) fn nav_id(destination: Destination) -> egui::Id {
    egui::Id::new(("v2_simple_destination", destination))
}

const FOOTER_HEIGHT: f32 = 30.0;
const NAV_LABEL_SIZE: f32 = 17.0;
const NAV_LABEL_MIN_SIZE: f32 = 12.5;

/// Where a row's icon and label go, and how large the label is, for a row of
/// `width`. The reference spacing when it fits; tighter, then smaller, when
/// the sidebar is narrow or the interface is enlarged - never cut off.
fn nav_metrics(width: f32, label_width_at_full_size: f32) -> (f32, f32, f32) {
    const RIGHT_PAD: f32 = 10.0;
    for (icon_x, text_x) in [(30.0, 66.0), (22.0, 48.0)] {
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

/// One navigation row: transparent until hovered, a dim blue pill with a
/// bright left edge when selected. A real focusable button for the keyboard
/// and for assistive technology.
fn nav_item(
    ui: &mut egui::Ui,
    destination: Destination,
    selected: bool,
    reveal: bool,
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
    // Keyboard focus must never land on a row that is scrolled out of view,
    // and the selected row is shown when asked. Immediately, not eased: the
    // row is on screen by the next frame.
    if response.gained_focus() || reveal {
        response.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 10.0, palette::SELECTED);
            painter.rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(4.0, rect.height())),
                egui::CornerRadius {
                    nw: 10,
                    sw: 10,
                    ne: 0,
                    se: 0,
                },
                palette::ACCENT,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 10.0, palette::HOVER);
        }
        let color = if selected || response.hovered() {
            palette::PRIMARY_TEXT
        } else {
            palette::SECONDARY_TEXT
        };
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
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                10.0,
                egui::Stroke::new(2.0_f32, palette::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
    }
    response.on_hover_text(destination.purpose())
}

/// The outlined All Tools button with its grid icon.
pub(super) fn all_tools_button(ui: &mut egui::Ui) -> egui::Response {
    let font = egui::FontId::proportional(16.0);
    let label = "All Tools";
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), palette::PRIMARY_TEXT)
        .size()
        .x;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(text_width + 66.0, 42.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(
            rect,
            8.0,
            if response.hovered() {
                palette::SELECTED
            } else {
                palette::SECONDARY_ACTION
            },
            egui::Stroke::new(
                if response.has_focus() {
                    2.0_f32
                } else {
                    1.0_f32
                },
                palette::ACCENT,
            ),
            egui::StrokeKind::Inside,
        );
        paint_grid_icon(
            painter,
            egui::pos2(rect.left() + 26.0, rect.center().y),
            palette::PRIMARY_TEXT,
        );
        painter.text(
            egui::pos2(rect.left() + 46.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            palette::PRIMARY_TEXT,
        );
    }
    response.on_hover_text("Show every tool in the full navigation. Nothing is hidden or removed.")
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
}

/// The five Manage Library tasks, in reading order. Descriptions say what the
/// destination offers, never a result.
pub(super) const MANAGE_TASKS: [ManageTask; 5] = [
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
];

pub(super) const MANAGE_TITLE: &str = "Manage Library";
pub(super) const MANAGE_PURPOSE: &str = "Choose a task. Each one opens its existing tool; nothing changes until you confirm a preview there.";

const TASK_CARD_HEIGHT: f32 = 168.0;
const TASK_CARD_MIN_WIDTH: f32 = 210.0;
const TASK_CARD_MAX_WIDTH: f32 = 300.0;
const TASK_CARD_GAP: f32 = 14.0;

/// How many cards fit on one row, and how wide each is.
pub(super) fn task_grid(available: f32) -> (usize, f32) {
    let columns = (((available + TASK_CARD_GAP) / (TASK_CARD_MIN_WIDTH + TASK_CARD_GAP)).floor()
        as usize)
        .clamp(1, MANAGE_TASKS.len());
    let width = ((available - TASK_CARD_GAP * (columns as f32 - 1.0)) / columns as f32)
        .clamp(TASK_CARD_MIN_WIDTH.min(available), TASK_CARD_MAX_WIDTH);
    (columns, width)
}

/// Paints the hub and returns the section whose card was activated.
pub(super) fn show_manage_library(ui: &mut egui::Ui) -> Option<Section> {
    let mut chosen = None;
    let (columns, width) = task_grid(ui.available_width());
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
    chosen
}

/// Stable across the hub's responsive row layout.
pub(super) fn task_id(section: Section) -> egui::Id {
    egui::Id::new(("v2_simple_manage_task", section))
}

fn task_card(ui: &mut egui::Ui, task: &ManageTask, width: f32) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(width, TASK_CARD_HEIGHT));
    let response = ui.interact(rect, task_id(task.section), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), task.title)
    });
    // Reveal keyboard focus once, in either Tab direction. Pointer input and
    // an already-focused card must not undo the person's wheel scrolling.
    if response.gained_focus()
        && !ui.input(|input| input.pointer.any_pressed() || input.pointer.any_released())
    {
        // Use the actual clip boundary: alignment-based scroll targets add
        // item spacing and can clip a tall card in a short viewport. Leave
        // one point for pixel rounding and move only as far as necessary.
        let visible = ui.clip_rect().shrink2(egui::vec2(0.0, 1.0));
        let delta = if rect.min.y < visible.min.y {
            visible.min.y - rect.min.y
        } else if rect.max.y > visible.max.y {
            visible.max.y - rect.max.y
        } else {
            0.0
        };
        if delta != 0.0 {
            ui.scroll_with_delta_animation(
                egui::vec2(0.0, delta),
                egui::style::ScrollAnimation::none(),
            );
        }
    }
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
    fn there_are_exactly_seven_distinct_destinations_on_existing_routes() {
        assert_eq!(DESTINATIONS.len(), 7);
        let labels: std::collections::BTreeSet<_> =
            DESTINATIONS.iter().map(|d| d.label()).collect();
        let routes: Vec<_> = DESTINATIONS.iter().map(|d| d.route()).collect();
        assert_eq!(labels.len(), 7);
        for (index, route) in routes.iter().enumerate() {
            assert!(!routes[..index].contains(route), "{route:?} is used twice");
            // Each destination opens a location it also highlights.
            assert_eq!(destination_for(route), Some(DESTINATIONS[index]));
        }
    }

    #[test]
    fn every_section_is_either_a_destination_or_explicitly_all_tools_only() {
        // `destination_for` is exhaustive over `Section`, so a new section
        // cannot be added without deciding where it belongs. This pins the
        // current decision: the specialist sections highlight nothing.
        let all_tools_only: Vec<_> = SECTIONS
            .iter()
            .copied()
            .filter(|section| destination_for(&Route::Section(*section)).is_none())
            .collect();
        assert_eq!(
            all_tools_only,
            [
                Section::Saves,
                Section::Tape,
                Section::Artwork,
                Section::Sources,
                Section::Romm,
                Section::Dat,
                Section::Advanced,
                Section::DatVerification,
                Section::SavesStates,
                Section::Mame,
                Section::ArtworkExtras,
                Section::SourcesProviders,
                Section::AdvancedDiagnostics,
            ]
        );
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
            Some(Destination::CheatsMods)
        );
        assert_eq!(
            destination_for(&Route::PlatformCheck("SNES".into())),
            Some(Destination::ManageLibrary)
        );
        assert_eq!(destination_for(&Route::MameWorkflow), None);
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
                entry.destination == Destination::CheatsMods
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
            entry.destination != Destination::CheatsMods
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
    fn scale_is_clamped_stepped_and_never_non_finite() {
        assert_eq!(clamp_scale(1.0), 1.0);
        assert_eq!(clamp_scale(0.1), UI_SCALE_MIN);
        assert_eq!(clamp_scale(9.0), UI_SCALE_MAX);
        assert_eq!(clamp_scale(f32::NAN), 1.0);
        assert_eq!(clamp_scale(f32::INFINITY), 1.0);
        assert!((clamp_scale(1.24) - 1.2).abs() < 1e-6);
    }

    #[test]
    fn simple_navigation_requires_the_opt_in_and_yields_to_all_tools() {
        let mut shell = ShellState::default();
        assert!(!shell.enabled && !shell.simple_navigation());
        shell.enabled = true;
        assert!(shell.simple_navigation());
        shell.all_tools = true;
        assert!(!shell.simple_navigation());
    }

    #[test]
    fn style_is_untouched_until_simple_is_turned_on_and_fully_restored_when_off() {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        let classic = (*context.style()).clone();
        let mut shell = ShellState::default();
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
            // The whole style, not selected fields.
            assert_eq!(*context.style(), classic);
            assert_eq!(context.zoom_factor(), 1.0);
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
        assert_eq!(nav_metrics(216.0, 118.0), (30.0, 66.0, NAV_LABEL_SIZE));
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
    fn manage_library_offers_the_five_blueprint_tasks_on_existing_sections() {
        let titles: Vec<_> = MANAGE_TASKS.iter().map(|task| task.title).collect();
        assert_eq!(
            titles,
            [
                "Check & Identify",
                "Organise & Export",
                "Review Duplicates",
                "Convert Media",
                "Fix Problems"
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
                Section::Problems
            ]
        );
        for task in MANAGE_TASKS {
            // Each card stays inside the Manage Library destination...
            assert_eq!(
                destination_for(&Route::Section(task.section)),
                Some(Destination::ManageLibrary)
            );
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
    fn manage_library_grid_reflows_from_five_across_to_one() {
        assert_eq!(task_grid(1380.0).0, 5);
        assert_eq!(task_grid(760.0).0, 3);
        assert_eq!(task_grid(230.0).0, 1);
        for available in [230.0, 500.0, 760.0, 1000.0, 1380.0, 2400.0] {
            let (columns, width) = task_grid(available);
            let used = columns as f32 * width + (columns as f32 - 1.0) * TASK_CARD_GAP;
            assert!(used <= available + 0.5, "{available}: {columns} x {width}");
        }
    }
}
