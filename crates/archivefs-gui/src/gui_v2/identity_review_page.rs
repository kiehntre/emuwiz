//! The Review Identity screen: why a game did not verify automatically, and
//! what the person can do about it. Browsing it is read-only; the only
//! mutation is the explicit, confirmed "Choose system" action, which goes
//! through the canonical manual platform assignment.
use super::{
    App,
    activity::CancelPolicy,
    backend::Command,
    identity_review::{DatKnowledge, DumpQuality, Review, ReviewState, review_for},
    library::Game,
    routes::{Route, Section},
};
use eframe::egui;
use std::path::Path;

/// The system picker: nothing is preselected and nothing changes until the
/// person confirms.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SystemChoice {
    pub query: String,
    pub selected: Option<&'static str>,
}

/// A platform's identity tally, from the same state function every page uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CheckSummary {
    pub verified: usize,
    pub matched: usize,
    pub need_choice: usize,
    /// Ready to verify: data may exist, but nothing has been compared yet.
    pub not_checked: usize,
    /// No usable identification data is installed for this platform.
    pub no_data: usize,
    /// The data was searched (or the file could not be read) and nothing matched.
    pub no_match: usize,
    /// The games that failed automatic verification, in library order.
    pub rows: Vec<(i64, &'static str)>,
}

pub(super) fn summarize_platform(
    library: &super::library::Library,
    platform: &str,
) -> CheckSummary {
    let mut summary = CheckSummary::default();
    for game in library
        .games
        .iter()
        .filter(|game| game.platform == platform)
    {
        let review = review_for(game, &library.identity_context, None);
        match review.state {
            ReviewState::Verified(_) => {
                summary.verified += 1;
                continue;
            }
            ReviewState::ReferenceMatched => {
                summary.matched += 1;
                continue;
            }
            ReviewState::Conflict { .. }
            | ReviewState::Ambiguous { .. }
            | ReviewState::NoSystem => {
                summary.need_choice += 1;
            }
            ReviewState::NotCompared => summary.not_checked += 1,
            ReviewState::NoData { .. } => summary.no_data += 1,
            ReviewState::NoMatch(_) => summary.no_match += 1,
        }
        summary.rows.push((game.archive.id, review.list_label()));
    }
    summary
}

#[derive(Default)]
pub(super) struct ReviewUi {
    /// The platform tally last computed, keyed by library snapshot and platform.
    pub check_cache: Option<(usize, String, CheckSummary)>,
    /// Recorded DAT answer for `knowledge_for`, once loaded.
    pub knowledge: Option<DatKnowledge>,
    pub knowledge_for: Option<i64>,
    pub knowledge_job: Option<u64>,
    pub choosing: Option<SystemChoice>,
    pub assign_job: Option<(u64, i64)>,
    /// Set when a setup page was opened from a review, so it can offer a way back.
    pub dat_context: Option<(i64, String)>,
}

impl ReviewUi {
    /// Forget what was loaded so the next view re-reads it (for example after
    /// returning from identification-data setup).
    pub(super) fn invalidate(&mut self) {
        self.knowledge = None;
        self.knowledge_for = None;
    }
}

/// How many systems the picker lists at once.
const MAX_CHOICES: usize = 12;

/// Canonical systems matching `query`, plus how many matched in all.
pub(super) fn system_choices(query: &str) -> (Vec<(&'static str, &'static str)>, usize) {
    let needle = query.trim().to_lowercase();
    let all: Vec<_> = archivefs_core::platform::canonical_ids()
        .into_iter()
        .map(|id| (id, archivefs_core::platform::display_name_for(id)))
        .filter(|(id, name)| {
            needle.is_empty()
                || name.to_lowercase().contains(&needle)
                || id.to_lowercase().contains(&needle)
        })
        .collect();
    let total = all.len();
    (all.into_iter().take(MAX_CHOICES).collect(), total)
}

/// Read-only: the recorded DAT answer for one library item.
pub(super) fn knowledge_at(
    database_path: &Path,
    archive_id: i64,
) -> Result<Option<DatKnowledge>, String> {
    let database = archivefs_core::Database::open_read_only(database_path)
        .map_err(|error| error.to_string())?;
    let mut all = Vec::new();
    for persisted in database
        .library_dat_identities_for_item(archive_id)
        .map_err(|error| error.to_string())?
    {
        if let Some(summary) = database
            .library_dat_identity_summary_for_item(
                archive_id,
                &persisted.source.source_id,
                None,
                None,
                true,
            )
            .map_err(|error| error.to_string())?
        {
            all.push(DatKnowledge::from_summary(&summary));
        }
    }
    Ok(DatKnowledge::best(all))
}

/// The explicit system choice, through the canonical manual assignment. Only a
/// system from the registry is accepted: nothing is guessed or typed freehand.
pub(super) fn assign_system_at(
    database_path: &Path,
    archive_path: &Path,
    platform_id: &str,
) -> Result<(), String> {
    if !archivefs_core::platform::canonical_ids().contains(&platform_id) {
        return Err("That is not a system EmuWiz knows.".into());
    }
    crate::platform_source_actions::apply_platform_action_at(
        database_path,
        archive_path,
        &crate::platform_source_actions::PlatformAction::Set(platform_id.to_string()),
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn file_name(game: &Game) -> String {
    game.archive.relative_path.file_name().map_or_else(
        || game.title.clone(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl App {
    /// Where a person goes to set up identification data for `game`'s system,
    /// remembering which game they came from.
    pub(super) fn open_identification_data(&mut self, game: i64, platform: &str) {
        self.review.dat_context = Some((game, platform.to_string()));
        self.go(Route::Section(Section::Dat));
    }

    pub(super) fn review_identity(&mut self, ui: &mut egui::Ui, id: i64) {
        let library = self.library.clone();
        let Some(game) = library.game(id) else {
            ui.heading("Review identity");
            ui.label("This game is not in the current game list. It may have moved or its folder may be disconnected.");
            if ui.button("Return to games").clicked() {
                self.go(Route::Section(Section::Games));
            }
            return;
        };
        if self.review.knowledge_for != Some(id) && self.review.knowledge_job.is_none() {
            let job = self.activity.queue_with(
                "Reading what EmuWiz knows about this game",
                Route::ReviewIdentity(id),
                CancelPolicy::NotCancellable,
            );
            self.review.knowledge_job = Some(job);
            self.send(job, Command::LoadIdentityKnowledge { archive_id: id });
        }
        let loaded = self.review.knowledge_for == Some(id);
        let knowledge = loaded.then(|| self.review.knowledge.clone()).flatten();
        let review = review_for(game, &library.identity_context, knowledge.as_ref());
        let mut route = None;
        let mut choose_system = false;
        let mut open_dat = false;
        egui::ScrollArea::vertical()
            .id_salt(("v2_review_identity", id))
            .show(ui, |ui| {
                ui.heading("Review identity");
                ui.strong(file_name(game));
                ui.label(if game.platform == super::library::UNKNOWN_PLATFORM {
                    "System not chosen yet".to_string()
                } else {
                    game.platform.clone()
                });
                ui.add_space(8.0);
                if !loaded {
                    ui.label("Checking what EmuWiz has recorded for this game…");
                }
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    show_state(
                        ui,
                        game,
                        &review,
                        knowledge.as_ref(),
                        &mut route,
                        &mut choose_system,
                        &mut open_dat,
                    );
                });
                ui.add_space(8.0);
                ui.collapsing("Technical details", |ui| {
                    ui.label(format!("File: {}", game.archive.absolute_path.display()));
                    ui.label(format!("Catalogue id: {}", game.archive.id));
                    match &knowledge {
                        Some(known) => ui.label(&known.technical),
                        None => ui.label("No recorded identification-data check for this game."),
                    };
                });
                if ui.button("Back to game").clicked() {
                    route = Some(Route::Game(id));
                }
            });
        if choose_system {
            self.review.choosing = Some(SystemChoice::default());
        }
        if open_dat {
            let platform = library
                .game(id)
                .map(|game| game.platform.clone())
                .unwrap_or_default();
            self.open_identification_data(id, &platform);
            return;
        }
        self.show_system_chooser(ui.ctx(), id);
        if let Some(route) = route {
            self.go(route);
        }
    }

    fn show_system_chooser(&mut self, context: &egui::Context, id: i64) {
        let Some(mut choice) = self.review.choosing.clone() else {
            return;
        };
        let Some(game) = self.library.game(id) else {
            self.review.choosing = None;
            return;
        };
        let name = file_name(game);
        let busy = self.review.assign_job.is_some();
        let mut confirm = None;
        let mut cancel = false;
        egui::Window::new("Choose system")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(context, |ui| {
                ui.set_max_width(460.0);
                ui.label(format!("Which system is {name} for?"));
                ui.label("EmuWiz does not guess. Pick the system yourself; nothing changes until you confirm.");
                ui.add(egui::TextEdit::singleline(&mut choice.query).hint_text("Search systems"));
                let (matches, total) = system_choices(&choice.query);
                for (system, display) in &matches {
                    if ui
                        .selectable_label(choice.selected == Some(*system), *display)
                        .clicked()
                    {
                        choice.selected = Some(*system);
                    }
                }
                if total > matches.len() {
                    ui.label(format!("Showing {} of {total}. Type to narrow the list.", matches.len()));
                }
                if let Some(selected) = choice.selected {
                    ui.separator();
                    ui.strong(format!(
                        "Assign {} to {name}?",
                        archivefs_core::platform::display_name_for(selected)
                    ));
                    ui.label("This records your choice for this one game. No game file is changed.");
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(choice.selected.is_some() && !busy, egui::Button::new("Confirm system"))
                        .clicked()
                    {
                        confirm = choice.selected;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.review.choosing = None;
        } else if let Some(system) = confirm {
            self.start_system_assignment(id, system);
        } else {
            self.review.choosing = Some(choice);
        }
    }

    fn start_system_assignment(&mut self, id: i64, system: &'static str) {
        if self.review.assign_job.is_some() {
            return;
        }
        let Some(game) = self.library.game(id) else {
            self.review.choosing = None;
            return;
        };
        let archive_path = game.archive.absolute_path.clone();
        let job = self.activity.queue_with(
            "Saving your system choice",
            Route::ReviewIdentity(id),
            CancelPolicy::NotCancellable,
        );
        self.review.assign_job = Some((job, id));
        self.review.choosing = None;
        self.send(
            job,
            Command::AssignSystem {
                archive_id: id,
                archive_path,
                platform: system.to_string(),
            },
        );
    }

    /// The assignment finished: re-read only what changed (this game's recorded
    /// answer) and reload the saved library so every page sees the new system.
    pub(super) fn finish_system_assignment(&mut self, id: i64) {
        self.review.assign_job = None;
        self.review.invalidate();
        let _ = id;
        self.load(false);
    }

    /// The tally Check Games shows, recomputed only when the library or the
    /// chosen platform changes.
    pub(super) fn check_summary(&mut self, platform: &str) -> CheckSummary {
        let key = std::sync::Arc::as_ptr(&self.library) as usize;
        if let Some((cached, name, summary)) = &self.review.check_cache
            && *cached == key
            && name == platform
        {
            return summary.clone();
        }
        let summary = summarize_platform(&self.library, platform);
        self.review.check_cache = Some((key, platform.to_string(), summary.clone()));
        summary
    }

    /// The one-line identity status every page shows for a game.
    pub(super) fn identity_review_for(&self, id: i64) -> Option<Review> {
        let game = self.library.game(id)?;
        let known = (self.review.knowledge_for == Some(id))
            .then(|| self.review.knowledge.as_ref())
            .flatten();
        Some(review_for(game, &self.library.identity_context, known))
    }
}

fn show_state(
    ui: &mut egui::Ui,
    game: &Game,
    review: &Review,
    knowledge: Option<&DatKnowledge>,
    route: &mut Option<Route>,
    choose_system: &mut bool,
    open_dat: &mut bool,
) {
    ui.heading(review.headline());
    match &review.state {
        ReviewState::Verified(facts) => {
            ui.strong(&facts.title);
            ui.label(format!(
                "{}{}",
                game.platform,
                facts
                    .region
                    .as_ref()
                    .map_or(String::new(), |region| format!(" · {region}"))
            ));
            if let Some(release) = facts.release {
                ui.label(format!("Release: {release}"));
            }
            if let Some(label) = facts.dump.label() {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 120, 20),
                    format!("{label}. Identified, but not a clean preservation dump."),
                );
            }
            ui.label(match (&facts.source, facts.by_file_evidence) {
                (Some(source), _) => {
                    format!("Evidence: trusted hash match · Reference source: {source}")
                }
                (None, true) => "Evidence: the file's own contents".to_string(),
                (None, false) => "Evidence: trusted identification data".to_string(),
            });
            // Automatic: there is deliberately nothing to confirm.
        }
        _ => {
            ui.label(review.explanation());
            if let ReviewState::Ambiguous { candidates } = &review.state {
                ui.strong("Possible matches");
                if candidates.is_empty() {
                    ui.label("The candidate names were not kept with this check.");
                }
                for candidate in candidates.iter().take(12) {
                    ui.label(format!("• {candidate}"));
                }
                ui.label("EmuWiz cannot choose between these for you yet. More evidence (a different dump or data source) is needed.");
            }
            if let (ReviewState::Conflict { .. }, Some(known)) = (&review.state, knowledge) {
                ui.label(format!("Reference source: {}", known.source_name));
            }
            ui.add_space(4.0);
            match &review.state {
                ReviewState::NoSystem => {
                    if ui.button("Choose system").clicked() {
                        *choose_system = true;
                    }
                }
                ReviewState::NoData {
                    reference_source_exists: true,
                }
                | ReviewState::NotCompared
                | ReviewState::NoMatch(_) => {
                    if ui.button("Set up identification data").clicked() {
                        *open_dat = true;
                    }
                    if ui.button("View file details").clicked() {
                        *route = Some(Route::Task {
                            section: Section::Advanced,
                            game: game.archive.id,
                        });
                    }
                }
                ReviewState::Conflict { .. } | ReviewState::Ambiguous { .. } => {
                    if ui.button("View file details").clicked() {
                        *route = Some(Route::Task {
                            section: Section::Advanced,
                            game: game.archive.id,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    let _ = DumpQuality::Clean;
}
