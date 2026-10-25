//! Native Blue "Platform aliases" (boards `D11-LG-PlatformAliases`, `-PA*`).
//!
//! Teaches EmuWiz what a folder name means ("gc" -> GameCube). Same backend
//! as the legacy panel: the list comes from `Database::list_platform_aliases`
//! (opened read-only), and Add / Remove go through `apply_alias_action_at`.
//! Only EmuWiz's own alias table changes; no game file is moved or renamed,
//! and the list applies on the next library scan. As in the legacy panel,
//! Add and Remove act immediately; a removed alias can be added again.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::PlatformAlias;
use eframe::egui::{self, RichText};

use super::simple_shell::palette;
use crate::platform_source_actions::{AliasAction, apply_alias_action_at};
use crate::ui::components::{self as widgets, ActionStyle, StatusTone};
use crate::ui::theme::{self, blue};

pub(super) const WHAT_CHANGES: &str = "Only EmuWiz's own alias list. No game files are moved or renamed. The list is applied on the next scan.";

type AliasList = Result<Vec<PlatformAlias>, String>;

#[derive(Default)]
pub(super) struct PlatformAliasesState {
    /// EmuWiz's catalogue. `None` keeps the page inert (tests, previews).
    database: Option<PathBuf>,
    pub aliases: Option<AliasList>,
    loading: Option<Receiver<AliasList>>,
    running: Option<(AliasAction, Receiver<Result<(), String>>)>,
    pub alias_text: String,
    pub platform: Option<String>,
    /// (succeeded, message) of the last Add or Remove.
    pub feedback: Option<(bool, String)>,
}

impl PlatformAliasesState {
    pub(super) fn live() -> Self {
        Self {
            database: archivefs_core::default_database_path().ok(),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn at(database: PathBuf) -> Self {
        Self {
            database: Some(database),
            ..Self::default()
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.running.is_some()
    }

    fn read(database: &std::path::Path) -> AliasList {
        if !database.is_file() {
            // No catalogue yet means no aliases yet; nothing is created.
            return Ok(Vec::new());
        }
        archivefs_core::Database::open_read_only(database)
            .and_then(|db| db.list_platform_aliases())
            .map_err(|error| error.to_string())
    }

    pub(super) fn poll(&mut self, context: &egui::Context) {
        let Some(database) = self.database.clone() else {
            return;
        };
        if self.aliases.is_none() && self.loading.is_none() {
            let (sender, receiver) = mpsc::channel();
            let context = context.clone();
            let path = database.clone();
            std::thread::spawn(move || {
                let _ = sender.send(Self::read(&path));
                context.request_repaint();
            });
            self.loading = Some(receiver);
        }
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(list) => {
                    self.aliases = Some(list);
                    self.loading = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.aliases = Some(Err("the alias list could not be read".into()));
                    self.loading = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let finished =
            self.running
                .as_ref()
                .and_then(|(action, receiver)| match receiver.try_recv() {
                    Ok(result) => Some((action.clone(), result)),
                    Err(TryRecvError::Disconnected) => Some((
                        action.clone(),
                        Err("the alias change stopped unexpectedly".into()),
                    )),
                    Err(TryRecvError::Empty) => None,
                });
        if let Some((action, result)) = finished {
            self.running = None;
            self.finish(action, result);
            // Re-read the list from the catalogue, never from memory.
            self.aliases = None;
        }
    }

    pub(super) fn finish(&mut self, action: AliasAction, result: Result<(), String>) {
        self.feedback = Some(match (&action, result) {
            (AliasAction::Add { alias, platform }, Ok(())) => {
                self.alias_text.clear();
                self.platform = None;
                (
                    true,
                    format!(
                        "Alias added: \u{201c}{alias}\u{201d} -> {platform}. Run a library scan to apply it."
                    ),
                )
            }
            (AliasAction::Remove { alias }, Ok(())) => (
                true,
                format!(
                    "Alias removed: \u{201c}{alias}\u{201d}. Run a library scan to apply this change."
                ),
            ),
            (_, Err(error)) => (false, format!("Nothing was changed: {error}")),
        });
    }

    /// The Add the form describes now, or `None` while it is incomplete.
    pub(super) fn pending_add(&self) -> Option<AliasAction> {
        let alias = self.alias_text.trim();
        let platform = self.platform.as_ref()?;
        (!alias.is_empty()).then(|| AliasAction::Add {
            alias: alias.to_string(),
            platform: platform.clone(),
        })
    }

    /// Starts one alias change. Refuses while another runs or when inert.
    pub(super) fn start(&mut self, action: AliasAction, context: &egui::Context) -> bool {
        let Some(database) = self.database.clone() else {
            return false;
        };
        if self.busy() {
            return false;
        }
        self.feedback = None;
        let (sender, receiver) = mpsc::channel();
        let context = context.clone();
        let worker = action.clone();
        std::thread::spawn(move || {
            let _ =
                sender.send(apply_alias_action_at(&database, &worker).map_err(|e| e.to_string()));
            context.request_repaint();
        });
        self.running = Some((action, receiver));
        true
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut PlatformAliasesState) -> Option<AliasAction> {
    let mut action = None;
    let wide = ui.available_width() >= 1100.0;
    let mut body = |ui: &mut egui::Ui| {
        ui.add(
            egui::Label::new(
                RichText::new(
                    "Map a folder name to a platform (for example \u{201c}gc\u{201d} -> GameCube). Custom aliases outrank built-in detection but never a manual archive assignment. Changes take effect on the next library scan.",
                )
                .color(blue::BODY_TEXT),
            )
            .wrap(),
        );
        ui.add_space(theme::SPACE_SM);
        if let Some((succeeded, message)) = &state.feedback {
            widgets::banner(
                ui,
                if *succeeded { "Saved" } else { "Not saved" },
                message,
                if *succeeded {
                    StatusTone::Success
                } else {
                    StatusTone::Warning
                },
            );
            ui.add_space(6.0);
        }
        let busy = state.busy();
        super::game_detail_blue::side_card(ui, "CUSTOM PLATFORM ALIASES", |ui| {
            match &state.aliases {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Reading the alias list…");
                    });
                }
                Some(Err(error)) => {
                    ui.label(
                        RichText::new(format!("The alias list could not be read: {error}"))
                            .color(blue::WARN),
                    );
                }
                Some(Ok(aliases)) if aliases.is_empty() => {
                    ui.label(
                    RichText::new("No custom platform aliases yet. Built-in detection is used for every folder.")
                        .color(palette::SECONDARY_TEXT),
                );
                }
                Some(Ok(aliases)) => {
                    for alias in aliases {
                        ui.push_id(("v2_alias", &alias.normalized_alias), |ui| {
                        egui::Frame::new()
                            .fill(blue::INSET)
                            .stroke(egui::Stroke::new(1.0_f32, palette::BORDER_SUBTLE))
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(140.0, 0.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.set_width(140.0);
                                            ui.label(RichText::new(&alias.alias).monospace().color(blue::PRIMARY_TEXT));
                                        },
                                    );
                                    ui.label(RichText::new(&alias.platform).color(blue::BODY_TEXT));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if widgets::action_button(ui, "Remove", ActionStyle::Secondary, !busy)
                                            .on_hover_text("Removes this alias from EmuWiz's list. No game file changes.")
                                            .clicked()
                                        {
                                            action = Some(AliasAction::Remove {
                                                alias: alias.alias.clone(),
                                            });
                                        }
                                    });
                                });
                            });
                    });
                        ui.add_space(4.0);
                    }
                }
            }
        });
        super::game_detail_blue::side_card(ui, "ADD AN ALIAS", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Alias");
                ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut state.alias_text)
                        .hint_text("gc")
                        .desired_width(140.0),
                );
                ui.label("Platform");
                ui.add_enabled_ui(!busy, |ui| {
                    egui::ComboBox::from_id_salt("v2_alias_platform")
                        .selected_text(state.platform.as_deref().unwrap_or("Select platform…"))
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            for name in archivefs_core::canonical_platform_names() {
                                ui.selectable_value(
                                    &mut state.platform,
                                    Some(name.to_string()),
                                    name,
                                );
                            }
                        });
                });
                let add = state.pending_add();
                if widgets::action_button(
                    ui,
                    "Add Alias",
                    ActionStyle::Primary,
                    !busy && add.is_some(),
                )
                .on_disabled_hover_text("Type an alias and choose a platform.")
                .clicked()
                {
                    action = add;
                }
                if busy {
                    ui.spinner();
                }
            });
        });
    };
    let side = |ui: &mut egui::Ui| {
        super::game_detail_blue::side_card(ui, "WHAT THIS CHANGES", |ui| {
            ui.add(egui::Label::new(RichText::new(WHAT_CHANGES).color(blue::BODY_TEXT)).wrap());
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Add and Remove act immediately. A removed alias can be added again.",
                    )
                    .color(palette::SECONDARY_TEXT),
                )
                .wrap(),
            );
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
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(root: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for entry in walk(root) {
            if entry.is_file() {
                out.push((entry.clone(), std::fs::read(&entry).unwrap()));
            }
        }
        out.sort();
        out
    }

    fn walk(root: &std::path::Path) -> Vec<PathBuf> {
        let mut all = Vec::new();
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                all.extend(walk(&path));
            }
            all.push(path);
        }
        all
    }

    fn wait(state: &mut PlatformAliasesState, context: &egui::Context) {
        for _ in 0..500 {
            state.poll(context);
            if !state.busy() && state.aliases.is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("alias work did not finish");
    }

    #[test]
    fn adding_and_removing_an_alias_changes_only_the_alias_table() {
        let root = tempfile::tempdir().unwrap();
        let games = root.path().join("games/gc");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(games.join("Game.iso"), b"disc image").unwrap();
        let before = tree(&root.path().join("games"));
        let database = root.path().join("data/library.sqlite3");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        let context = egui::Context::default();
        let mut state = PlatformAliasesState::at(database.clone());
        wait(&mut state, &context);
        assert_eq!(state.aliases.as_ref().unwrap().as_ref().unwrap().len(), 0);
        assert!(!database.exists(), "reading never creates the catalogue");

        state.alias_text = "  gc ".into();
        assert_eq!(state.pending_add(), None, "no platform chosen yet");
        state.platform = Some("GameCube".into());
        let add = state.pending_add().unwrap();
        assert!(state.start(add, &context));
        assert!(
            !state.start(AliasAction::Remove { alias: "gc".into() }, &context),
            "one at a time"
        );
        wait(&mut state, &context);
        let aliases = state.aliases.clone().unwrap().unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].alias, "gc");
        assert_eq!(aliases[0].platform, "GameCube");
        assert!(state.alias_text.is_empty() && state.platform.is_none());

        assert!(state.start(AliasAction::Remove { alias: "gc".into() }, &context));
        wait(&mut state, &context);
        assert!(state.aliases.clone().unwrap().unwrap().is_empty());
        assert_eq!(state.feedback.as_ref().map(|(ok, _)| *ok), Some(true));
        assert_eq!(
            tree(&root.path().join("games")),
            before,
            "game files untouched"
        );
    }

    #[test]
    fn a_failed_change_says_nothing_was_changed() {
        let mut state = PlatformAliasesState::default();
        state.finish(
            AliasAction::Remove {
                alias: "nope".into(),
            },
            Err("no platform alias matches 'nope'".into()),
        );
        let (ok, message) = state.feedback.clone().unwrap();
        assert!(!ok);
        assert!(message.starts_with("Nothing was changed"));
    }

    #[test]
    fn an_inert_page_never_reads_or_writes() {
        let context = egui::Context::default();
        let mut state = PlatformAliasesState::default();
        state.poll(&context);
        assert!(state.aliases.is_none() && state.loading.is_none());
        assert!(!state.start(AliasAction::Remove { alias: "gc".into() }, &context));
    }
}
