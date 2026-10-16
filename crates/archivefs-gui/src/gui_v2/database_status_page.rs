//! Native Blue "Database status" (boards `D11-LG-DatabaseStatus`, `-DB*`).
//!
//! Shows where EmuWiz keeps what it knows about the library, and whether it
//! is ready, not created yet, needs upgrading or could not be loaded. It
//! reuses the legacy controller unchanged: `start_database_load` /
//! `poll_database_load` over `DatabaseState`. A plain status refresh opens
//! the catalogue read-only.
//!
//! An outdated catalogue is upgraded only through that same loader with a
//! scan (`load_database_snapshot(true)`), which keeps a verified backup
//! before the migration chain runs, and only after the "Upgrade the library
//! database?" confirmation (approved C item), which says there is no Undo.
//! An ordinary scan uses the Blue app's own confirmed "Scan your game
//! folders?" flow, so there is one scan path for the library.

use eframe::egui::{self, RichText};

use super::simple_shell::palette;
use crate::database_load::{
    CachedLibrarySnapshot, DatabaseGeneration, DatabaseState, poll_database_load,
    start_database_load,
};
use crate::ui::components::{self as widgets, ActionStyle, StatusTone};
use crate::ui::theme::{self, blue};

pub(super) const NO_UNDO: &str = "There is no Undo for a database upgrade.";
pub(super) const SCAN_CHANGES: &str = "A scan reads your source folders and updates EmuWiz's own database. Game files are not changed.";

type Starter = fn(
    egui::Context,
    DatabaseGeneration,
    Option<Box<CachedLibrarySnapshot>>,
    bool,
) -> DatabaseState;

pub(super) struct DatabaseStatusState {
    /// `None` keeps the page inert (tests, previews).
    starter: Option<Starter>,
    pub db: Option<DatabaseState>,
    generation: DatabaseGeneration,
    pub confirm_upgrade: bool,
    /// (succeeded, message) of the last load, scan or upgrade.
    pub feedback: Option<(bool, String)>,
    /// Set when an upgrade or scan finished, so the app reloads its library.
    pub library_changed: bool,
    /// The app's own scan was running last frame (refresh when it ends).
    pub saw_app_scan: bool,
    /// The app's own scan is running now: an upgrade scan must not start
    /// beside it (two scan writers on one catalogue).
    pub app_scan: bool,
}

impl Default for DatabaseStatusState {
    /// Inert: never loads or upgrades anything.
    fn default() -> Self {
        Self {
            starter: None,
            db: None,
            generation: DatabaseGeneration::INITIAL,
            confirm_upgrade: false,
            feedback: None,
            library_changed: false,
            saw_app_scan: false,
            app_scan: false,
        }
    }
}

/// What the page asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Request {
    /// Open the Blue app's own confirmed scan.
    Scan,
    OpenActivity,
}

impl DatabaseStatusState {
    pub(super) fn live() -> Self {
        Self {
            starter: Some(start_database_load),
            ..Self::default()
        }
    }

    /// The upgrade scan (which writes the catalogue) is running.
    pub(super) fn upgrading(&self) -> bool {
        self.db.as_ref().is_some_and(DatabaseState::is_scanning)
    }

    pub(super) fn loading(&self) -> bool {
        self.db.as_ref().is_some_and(DatabaseState::is_loading)
    }

    fn previous(&mut self) -> Option<Box<CachedLibrarySnapshot>> {
        match self.db.take()? {
            DatabaseState::Ready { snapshot, .. } => Some(snapshot),
            DatabaseState::Outdated { previous, .. }
            | DatabaseState::Error { previous, .. }
            | DatabaseState::Loading { previous, .. } => previous,
            DatabaseState::NotCreated { .. } => None,
        }
    }

    /// Starts a read-only status load, or (`upgrade`) the backed-up upgrade
    /// and scan. Refuses while loading or when inert.
    fn start(&mut self, context: &egui::Context, upgrade: bool) -> bool {
        let Some(starter) = self.starter else {
            return false;
        };
        if self.loading() {
            return false;
        }
        self.generation = self.generation.next();
        let previous = self.previous();
        self.db = Some(starter(context.clone(), self.generation, previous, upgrade));
        true
    }

    pub(super) fn refresh(&mut self, context: &egui::Context) -> bool {
        self.start(context, false)
    }

    /// Opens the upgrade confirmation; only an outdated catalogue has one.
    pub(super) fn ask_upgrade(&mut self) {
        if matches!(self.db, Some(DatabaseState::Outdated { .. }))
            && !self.loading()
            && !self.app_scan
        {
            self.confirm_upgrade = true;
        }
    }

    /// The confirmation's answer. The upgrade starts only on Confirm, only
    /// for a catalogue that is still outdated.
    pub(super) fn answer_upgrade(&mut self, upgrade: bool, context: &egui::Context) -> bool {
        let ask = std::mem::take(&mut self.confirm_upgrade);
        if !(ask
            && upgrade
            && !self.app_scan
            && matches!(self.db, Some(DatabaseState::Outdated { .. })))
        {
            return false;
        }
        self.start(context, true)
    }

    pub(super) fn poll(&mut self, context: &egui::Context) {
        if self.starter.is_none() {
            return;
        }
        if self.db.is_none() {
            self.refresh(context);
            return;
        }
        let was_scanning = self.db.as_ref().is_some_and(DatabaseState::is_scanning);
        let Some(db) = self.db.as_mut() else {
            return;
        };
        let mut no_source_scan = None;
        if let Some(settled) = poll_database_load(db, self.generation, &mut no_source_scan) {
            if let Some(feedback) = settled.feedback {
                self.feedback = Some((feedback.succeeded, feedback.message));
            } else if let Some(DatabaseState::Error { message, .. }) = &self.db {
                self.feedback = Some((false, message.clone()));
            }
            if was_scanning && matches!(self.db, Some(DatabaseState::Ready { .. })) {
                self.library_changed = true;
            }
        }
    }
}

fn status_text(db: Option<&DatabaseState>) -> (&'static str, StatusTone) {
    match db {
        None => ("Loading…", StatusTone::Pending),
        Some(DatabaseState::Loading { scanning: true, .. }) => ("Scanning…", StatusTone::Active),
        Some(DatabaseState::Loading { .. }) => ("Loading…", StatusTone::Pending),
        Some(DatabaseState::Ready { .. }) => ("Ready", StatusTone::Success),
        Some(DatabaseState::NotCreated { .. }) => ("Not created yet", StatusTone::Pending),
        Some(DatabaseState::Outdated { .. }) => ("Needs upgrading", StatusTone::Warning),
        Some(DatabaseState::Error { .. }) => ("Could not be loaded", StatusTone::Warning),
    }
}

fn schema_line(db: &DatabaseState) -> Option<String> {
    let latest = archivefs_core::latest_schema_version();
    match db {
        DatabaseState::Outdated { health, .. } => Some(format!(
            "{} (current requires {latest})",
            health
                .schema_version
                .map_or_else(|| "unknown".to_string(), |version| version.to_string())
        )),
        other => other
            .snapshot()
            .map(|snapshot| snapshot.schema_version.to_string()),
    }
}

fn path_of(db: &DatabaseState) -> Option<String> {
    match db {
        DatabaseState::NotCreated { database_path } => Some(database_path.display().to_string()),
        DatabaseState::Outdated { health, .. } => Some(health.resolved_path.display().to_string()),
        other => other
            .snapshot()
            .map(|snapshot| snapshot.database_path.display().to_string()),
    }
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut DatabaseStatusState,
    app_scan_running: bool,
) -> Option<Request> {
    state.app_scan = app_scan_running;
    let mut request = None;
    let wide = ui.available_width() >= 1100.0;
    let context = ui.ctx().clone();
    let mut body = |ui: &mut egui::Ui| {
        match &state.db {
            Some(DatabaseState::Outdated { .. }) => widgets::banner(
                ui,
                "Library database needs upgrading",
                "Your library was created by an older EmuWiz version. Upgrade it before using current catalogue features.",
                StatusTone::Warning,
            ),
            Some(DatabaseState::Error { .. }) => widgets::banner(
                ui,
                "The library database could not be loaded",
                "Nothing was changed. Retry, or open Activity for details.",
                StatusTone::Warning,
            ),
            Some(DatabaseState::Loading { scanning: true, .. }) => widgets::banner(
                ui,
                "Scanning…",
                "You can keep using EmuWiz. Nothing is deleted.",
                StatusTone::Info,
            ),
            Some(DatabaseState::NotCreated { .. }) => widgets::banner(
                ui,
                "No library database yet",
                "Scan your game folders to create one. Nothing exists to upgrade.",
                StatusTone::Info,
            ),
            _ => {}
        }
        if let Some((succeeded, message)) = &state.feedback {
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(RichText::new(message).color(if *succeeded {
                    blue::OK
                } else {
                    blue::WARN
                }))
                .wrap(),
            );
        }
        ui.add_space(6.0);
        let loading = state.loading();
        ui.horizontal_wrapped(|ui| match &state.db {
            Some(DatabaseState::Outdated { .. }) => {
                if widgets::action_button(
                    ui,
                    "Scan and upgrade library…",
                    ActionStyle::Primary,
                    !loading && !app_scan_running,
                )
                .on_disabled_hover_text("A scan is already running. Upgrade when it has finished.")
                .clicked()
                {
                    state.ask_upgrade();
                }
                if widgets::action_button(
                    ui,
                    "Refresh database status",
                    ActionStyle::Secondary,
                    !loading,
                )
                .clicked()
                {
                    state.refresh(&context);
                }
            }
            Some(DatabaseState::Error { .. }) => {
                if widgets::action_button(ui, "Retry database load", ActionStyle::Primary, !loading)
                    .clicked()
                {
                    state.refresh(&context);
                }
                if widgets::action_button(ui, "Open Activity details", ActionStyle::Secondary, true)
                    .clicked()
                {
                    request = Some(Request::OpenActivity);
                }
            }
            _ => {
                let scanning = loading || app_scan_running;
                if widgets::action_button(ui, "Scan library", ActionStyle::Secondary, !scanning)
                    .on_disabled_hover_text("A scan is already running.")
                    .clicked()
                {
                    request = Some(Request::Scan);
                }
                if widgets::action_button(
                    ui,
                    "Refresh database status",
                    ActionStyle::Secondary,
                    !loading,
                )
                .clicked()
                {
                    state.refresh(&context);
                }
                if scanning {
                    ui.label(
                        RichText::new("A scan is already running.")
                            .size(13.0)
                            .color(palette::SECONDARY_TEXT),
                    );
                }
            }
        });
        ui.add_space(6.0);
        super::game_detail_blue::side_card(ui, "", |ui| {
            let (status, _) = status_text(state.db.as_ref());
            let mut rows: Vec<(&str, String)> = vec![(
                "Database path",
                state
                    .db
                    .as_ref()
                    .and_then(path_of)
                    .unwrap_or_else(|| "Not resolved yet".into()),
            )];
            rows.push(("Status", status.to_string()));
            if let Some(db) = &state.db {
                if let Some(schema) = schema_line(db) {
                    rows.push(("Schema version", schema));
                }
                if let Some(snapshot) = db.snapshot() {
                    rows.push((
                        "Last completed scan",
                        if db.is_scanning() {
                            "(in progress)".into()
                        } else {
                            snapshot
                                .last_completed_scan
                                .as_ref()
                                .map(|scan| {
                                    scan.finished_at.clone().unwrap_or_else(|| {
                                        format!("{} (in progress)", scan.started_at)
                                    })
                                })
                                .unwrap_or_else(|| "Never".into())
                        },
                    ));
                    rows.push(("Cached archives", snapshot.stats.total_archives.to_string()));
                    rows.push((
                        "Present / missing",
                        format!(
                            "{} / {}",
                            snapshot.stats.present_archives, snapshot.stats.missing_archives
                        ),
                    ));
                }
                if let DatabaseState::Ready {
                    last_scan_summary: Some(summary),
                    ..
                } = db
                {
                    rows.push(("Skipped files", summary.skipped_files_total().to_string()));
                }
            }
            for (label, value) in rows {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(170.0, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(170.0);
                            ui.label(RichText::new(label).color(blue::DIM_TEXT));
                        },
                    );
                    let text = RichText::new(value).color(blue::BODY_TEXT);
                    ui.add(
                        egui::Label::new(if label == "Database path" {
                            text.monospace().size(12.5)
                        } else {
                            text
                        })
                        .wrap(),
                    );
                });
            }
            ui.label(
                RichText::new(
                    "Cached rows never allow a mount or unmount. Only a fresh live check can.",
                )
                .size(13.0)
                .color(palette::SECONDARY_TEXT),
            );
        });
    };
    let side = |ui: &mut egui::Ui| {
        super::game_detail_blue::side_card(ui, "WHAT A SCAN CHANGES", |ui| {
            ui.add(egui::Label::new(RichText::new(SCAN_CHANGES).color(blue::BODY_TEXT)).wrap());
            ui.label(RichText::new(NO_UNDO).color(palette::SECONDARY_TEXT));
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
    if state.confirm_upgrade {
        upgrade_dialog(&context, state);
    }
    request
}

pub(super) fn upgrade_message(state: &DatabaseStatusState) -> String {
    let latest = archivefs_core::latest_schema_version();
    let from = match &state.db {
        Some(DatabaseState::Outdated { health, .. }) => health.schema_version.map_or_else(
            || "an older schema".to_string(),
            |version| format!("schema {version}"),
        ),
        _ => "an older schema".to_string(),
    };
    format!(
        "EmuWiz will keep a verified backup of its library database, update it from {from} to {latest} and then scan your sources. Your game files are not changed. {NO_UNDO}"
    )
}

fn upgrade_dialog(context: &egui::Context, state: &mut DatabaseStatusState) {
    let id = egui::Id::new("v2_database_upgrade_confirm");
    let mut answer = None;
    let message = upgrade_message(state);
    let modal = egui::Modal::new(id).show(context, |ui| {
        ui.set_max_width(600.0);
        ui.heading("Upgrade the library database?");
        ui.add(egui::Label::new(message).wrap());
        ui.horizontal(|ui| {
            let cancel = ui.button("Cancel");
            let opened = id.with("opened");
            if !ui
                .ctx()
                .data(|data| data.get_temp::<bool>(opened).unwrap_or(false))
            {
                cancel.request_focus();
                ui.ctx().data_mut(|data| data.insert_temp(opened, true));
            }
            if cancel.clicked() {
                answer = Some(false);
            }
            if widgets::action_button(ui, "Scan and upgrade", ActionStyle::Primary, true).clicked()
            {
                answer = Some(true);
            }
        });
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(false);
    }
    if let Some(answer) = answer {
        context.data_mut(|data| data.remove::<bool>(id.with("opened")));
        state.answer_upgrade(answer, context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::DatabaseHealth;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static STARTS: AtomicUsize = AtomicUsize::new(0);
    static UPGRADES: AtomicUsize = AtomicUsize::new(0);

    fn fake_start(
        _: egui::Context,
        _: DatabaseGeneration,
        _: Option<Box<CachedLibrarySnapshot>>,
        upgrade: bool,
    ) -> DatabaseState {
        STARTS.fetch_add(1, Ordering::SeqCst);
        if upgrade {
            UPGRADES.fetch_add(1, Ordering::SeqCst);
        }
        DatabaseState::Error {
            message: "test".into(),
            previous: None,
        }
    }

    fn outdated() -> DatabaseState {
        DatabaseState::Outdated {
            health: DatabaseHealth {
                resolved_path: "/data/library.sqlite3".into(),
                database_exists: true,
                database_opens: true,
                schema_version: Some(7),
                migrations_current: false,
                foreign_keys_enabled: true,
                error: None,
            },
            previous: None,
        }
    }

    #[test]
    fn an_upgrade_runs_only_after_its_confirmation_and_says_there_is_no_undo() {
        let context = egui::Context::default();
        let mut state = DatabaseStatusState {
            starter: Some(fake_start),
            db: Some(outdated()),
            ..Default::default()
        };
        let before = UPGRADES.load(Ordering::SeqCst);
        // Cancel: nothing starts.
        state.ask_upgrade();
        assert!(state.confirm_upgrade);
        assert!(!state.answer_upgrade(false, &context));
        assert_eq!(UPGRADES.load(Ordering::SeqCst), before);
        // An answer without the dialog open never starts anything.
        assert!(!state.answer_upgrade(true, &context));
        assert_eq!(UPGRADES.load(Ordering::SeqCst), before);
        // The message names the schemas, the backup and the missing Undo.
        state.ask_upgrade();
        let message = upgrade_message(&state);
        assert!(message.contains("schema 7"));
        assert!(message.contains("verified backup"));
        assert!(message.contains(NO_UNDO));
        assert!(message.contains("game files are not changed"));
        // Confirm: exactly one upgrade starts.
        assert!(state.answer_upgrade(true, &context));
        assert!(UPGRADES.load(Ordering::SeqCst) >= before + 1);
    }

    #[test]
    fn no_upgrade_starts_while_the_apps_own_scan_runs() {
        let context = egui::Context::default();
        let mut state = DatabaseStatusState {
            starter: Some(fake_start),
            db: Some(outdated()),
            app_scan: true,
            ..Default::default()
        };
        state.ask_upgrade();
        assert!(!state.confirm_upgrade, "no confirmation while a scan runs");
        // A dialog opened before the scan started cannot start it either.
        state.app_scan = false;
        state.ask_upgrade();
        state.app_scan = true;
        assert!(!state.answer_upgrade(true, &context));
        assert!(matches!(state.db, Some(DatabaseState::Outdated { .. })));
    }

    #[test]
    fn only_an_outdated_catalogue_offers_an_upgrade() {
        let mut state = DatabaseStatusState {
            starter: Some(fake_start),
            db: Some(DatabaseState::Error {
                message: "x".into(),
                previous: None,
            }),
            ..Default::default()
        };
        state.ask_upgrade();
        assert!(!state.confirm_upgrade);
        state.db = Some(DatabaseState::NotCreated {
            database_path: "/data/library.sqlite3".into(),
        });
        state.ask_upgrade();
        assert!(!state.confirm_upgrade);
    }

    #[test]
    fn an_inert_page_never_loads() {
        let context = egui::Context::default();
        let mut state = DatabaseStatusState::default();
        state.poll(&context);
        assert!(state.db.is_none());
        assert!(!state.refresh(&context));
    }

    #[test]
    fn the_states_render_their_own_actions() {
        let context = egui::Context::default();
        crate::gui_v2::readable_style(&context);
        for (db, wanted) in [
            (outdated(), "Scan and upgrade library…"),
            (
                DatabaseState::Error {
                    message: "broken".into(),
                    previous: None,
                },
                "Retry database load",
            ),
            (
                DatabaseState::NotCreated {
                    database_path: "/data/library.sqlite3".into(),
                },
                "Scan library",
            ),
        ] {
            let mut state = DatabaseStatusState {
                db: Some(db),
                ..Default::default()
            };
            let mut texts = Vec::new();
            for width in [600.0, 1500.0] {
                let mut used = 0.0_f32;
                let output = context.run(
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
                                assert_eq!(show(ui, &mut state, false), None);
                                used = ui.min_rect().right();
                            });
                        });
                    },
                );
                assert!(used <= width, "used {used} of {width}");
                fn gather(shape: &egui::Shape, out: &mut Vec<String>) {
                    match shape {
                        egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| gather(s, out)),
                        _ => {}
                    }
                }
                for shape in &output.shapes {
                    gather(&shape.shape, &mut texts);
                }
            }
            assert!(
                texts.iter().any(|text| text == wanted),
                "{wanted}: {texts:?}"
            );
            assert!(
                !state.confirm_upgrade,
                "rendering never opens the confirmation"
            );
        }
    }
}
