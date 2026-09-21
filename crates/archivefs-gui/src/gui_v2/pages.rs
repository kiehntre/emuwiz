//! Paint-only pages: layout, in-memory projections and explicit user intents.
use super::{
    App,
    activity::Phase,
    artwork::Picture,
    backend::Command,
    library::{Game, media_kind_label},
    media_sources::{Kind, Source},
    onboarding,
    problems::{Category, Problem, ProblemSummary, Severity},
    routes::{HOME_TASKS, Route, SECTIONS, Section},
};
use crate::ui::components::mrwiz_tip;
use archivefs_core::dat::rename_apply::model::TransactionState;
use eframe::egui::{self, Color32, RichText};

fn primary(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(text).strong())
            .fill(Color32::from_rgb(30, 85, 137))
            .min_size(egui::vec2(180.0, 44.0)),
    )
    .clicked()
}

/// One bounded main-content scroller; sidebar and header are outside it.
fn check_scroll(ui: &mut egui::Ui, platform: Option<&str>, content: impl FnOnce(&mut egui::Ui)) {
    let salt = ("v2_check_scroll", platform);
    let id = ui.make_persistent_id(salt);
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false]);
    // This page has buttons, not text inputs. Button focus must not disable paging.
    if ui.input(|i| !i.modifiers.alt && !i.modifiers.ctrl) {
        let previous = egui::scroll_area::State::load(ui.ctx(), id)
            .unwrap_or_default()
            .offset
            .y;
        let maximum = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(id.with("content_height")))
            .map(|height| (height - ui.available_height()).max(0.0))
            .unwrap_or(0.0);
        let page = ui.available_height() * 0.9;
        let target = ui.input(|i| {
            if i.key_pressed(egui::Key::Home) {
                Some(0.0)
            } else if i.key_pressed(egui::Key::End) {
                Some(maximum)
            } else if i.key_pressed(egui::Key::PageDown) {
                Some((previous + page).min(maximum))
            } else if i.key_pressed(egui::Key::PageUp) {
                Some((previous - page).max(0.0))
            } else {
                None
            }
        });
        if let Some(target) = target {
            scroll = scroll.vertical_scroll_offset(target);
        }
    }
    let output = scroll.show(ui, content);
    ui.ctx()
        .data_mut(|d| d.insert_temp(id.with("content_height"), output.content_size.y));
}

impl App {
    pub(super) fn show(&mut self, context: &egui::Context) {
        if context.input(|input| {
            input.key_pressed(egui::Key::Escape)
                || (input.modifiers.alt && input.key_pressed(egui::Key::ArrowLeft))
        }) {
            self.back();
        }
        if context.input(|input| input.modifiers.alt && input.key_pressed(egui::Key::Home)) {
            self.go(Route::Home);
        }
        egui::TopBottomPanel::bottom("v2_activity_bar").show(context, |ui| {
            ui.horizontal_wrapped(|ui| {
                let running = self.activity.running();
                if running == 0 {
                    ui.label("Ready · browsing does not change your game files");
                } else {
                    ui.spinner();
                    ui.label(format!("{running} jobs in progress"));
                }
                if ui.button("View Activity").clicked() {
                    self.go(Route::Section(Section::Activity));
                }
            });
            if let Some(job) = self.activity.jobs.values().rev().find(|job| job.active()) {
                ui.label(&job.title);
                if let Some(fraction) = job.fraction() {
                    ui.add(egui::ProgressBar::new(fraction).show_percentage());
                }
            }
        });
        let sidebar_width = if context.content_rect().width() < 900.0 {
            178.0
        } else {
            230.0
        };
        egui::SidePanel::left("v2_navigation")
            .exact_width(sidebar_width)
            .resizable(false)
            .show(context, |ui| {
                ui.heading("EmuWiz");
                ui.label("GUI v2 · live review");
                egui::ScrollArea::vertical()
                    .id_salt("v2_sidebar_scroll")
                    .show(ui, |ui| {
                        for section in SECTIONS {
                            if let Some(group) = section.group() {
                                ui.separator();
                                ui.strong(group);
                            }
                            let selected = self.router.current.section() == *section;
                            if ui
                                .add_sized(
                                    [ui.available_width(), 40.0],
                                    egui::Button::new(section.title()).selected(selected).wrap(),
                                )
                                .clicked()
                            {
                                self.go(if *section == Section::Home {
                                    Route::Home
                                } else {
                                    Route::Section(*section)
                                });
                            }
                        }
                        ui.label("Tab: move focus\nEnter: open\nAlt+Left: back");
                    });
            });
        egui::CentralPanel::default().show(context, |ui| {
            self.header(ui);
            if let Some(notice) = &self.notice {
                let mut dismiss = false;
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.strong("Needs attention");
                    ui.label(&notice.message);
                    ui.collapsing("Technical details", |ui| {
                        ui.label(&notice.technical);
                    });
                    dismiss = ui.button("Dismiss message").clicked();
                });
                if dismiss {
                    self.notice = None;
                }
            }
            match self.router.current.clone() {
                Route::Home | Route::Section(Section::Home) => self.home(ui),
                Route::Section(Section::Games | Section::Launch) => self.games(ui),
                Route::Section(Section::Emulators) => self.emulator_setup(ui),
                Route::Section(Section::Sources) => self.sources(ui),
                Route::Section(Section::Dat) => self.dat_sources(ui),
                Route::Section(Section::Artwork) => self.artwork_metadata(ui, None),
                Route::Section(Section::Mods) => self.mods_page(ui, None),
                Route::Section(Section::Check) => self.check_games(ui),
                Route::Section(Section::Duplicates) => self.duplicates(ui),
                Route::Section(Section::Problems) => self.problems(ui),
                Route::Section(Section::Build) => self.organisation_page(ui),
                Route::Section(Section::Setup) => self.setup_doctor(ui),
                Route::Section(Section::Platforms) => self.platforms(ui),
                Route::Game(id) => self.game_detail(ui, id),
                Route::Section(Section::Activity) => self.activities(ui),
                Route::Section(Section::History) => self.history(ui),
                Route::Section(Section::Settings) => self.settings(ui),
                Route::Section(Section::Advanced) => self.advanced(ui),
                Route::Task {
                    section: Section::Build,
                    ..
                } => self.organisation_page(ui),
                Route::Task {
                    section: Section::Mods,
                    game,
                } => self.mods_page(ui, Some(game)),
                Route::Task {
                    section: Section::Launch,
                    game,
                } => self.launch(ui, game),
                Route::Task {
                    section: Section::Artwork,
                    game,
                } => self.artwork_metadata(ui, Some(game)),
                Route::Task { section, .. } => self.handoff(ui, section),
            }
        });
        if self.confirm_scan {
            egui::Window::new("Scan your game folders?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show(context, |ui| {
                ui.set_max_width(440.0);
                ui.label("This reads your configured game folders and updates EmuWiz's game list. It does not rename, repair or change your original games.");
                ui.label("Once started, this scanner must finish safely. You can keep browsing and follow it in Activity.");
                if primary(ui, "Scan configured folders") { self.confirm_scan = false; self.load(true); }
                if ui.button("Cancel — keep browsing").clicked() { self.confirm_scan = false; }
            });
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Back").clicked() {
                self.back();
            }
            if ui.button("Home").clicked() {
                self.go(Route::Home);
            }
            ui.label("›");
            ui.strong(self.router.current.section().title());
            if let Some(game) = self
                .router
                .current
                .game()
                .and_then(|id| self.library.game(id))
            {
                ui.label("›");
                ui.label(&game.title);
            }
        });
        let title = match &self.router.current {
            Route::Game(id) => self
                .library
                .game(*id)
                .map(|game| game.title.as_str())
                .unwrap_or("Game no longer listed"),
            _ => self.router.current.section().title(),
        };
        ui.heading(title);
        ui.label(if matches!(self.router.current, Route::Game(_)) {
            "Your game's information, readiness and next actions, in one place."
        } else {
            self.router.current.section().purpose()
        });
        ui.separator();
    }

    fn setup_doctor(&mut self, ui: &mut egui::Ui) {
        let action = onboarding::show(
            ui,
            self.environment.as_ref(),
            &self.library,
            self.welcome_dismissed,
            self.doctor_platform.as_deref(),
        );
        self.handle_onboarding_action(ui.ctx(), action);
    }

    fn home(&mut self, ui: &mut egui::Ui) {
        mrwiz_tip(
            ui,
            "I can point out safe next steps here; nothing is changed by browsing.",
            &mut self.mrwiz_dismissed,
        );
        if self.loaded {
            ui.label(format!(
                "{} games · {} systems · {} need attention",
                self.library.games.len(),
                self.library.platforms.len(),
                self.library.attention
            ));
        } else {
            ui.label("Loading your existing game list. You can already explore the tasks below.");
        }
        if let Some(environment) = self.environment.as_ref() {
            let ready_count = environment.setup_ready_count(&self.library);
            let attention_count = environment.setup_attention_count(&self.library);
            let open_setup = egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading("System setup");
                ui.label(format!(
                    "{ready_count} ready · {attention_count} need attention"
                ));
                ui.button("Open Setup & Doctor").clicked()
            });
            if open_setup.inner {
                self.go(Route::Section(Section::Setup));
            }
        }
        egui::ScrollArea::vertical().id_salt("v2_home").show(ui, |ui| {
            if self.loaded && self.library.games.is_empty() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Let's find your games");
                    ui.label("No games are listed yet. Start by discovering your game folders.");
                    if primary(ui, "Add my games") { self.go(Route::Section(Section::Sources)); }
                    ui.label("Next: review the folders found before choosing what to scan.");
                });
            }
            for (section, title, purpose, action) in HOME_TASKS {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width()); ui.heading(*title); ui.label(*purpose);
                    if primary(ui, action) { self.go(Route::Section(*section)); }
                    ui.label(match section { Section::Games | Section::Launch | Section::Mods => "Next: choose a platform or game.", _ => "Next: review setup and continue in the existing workflow. Nothing changes by opening it." });
                });
            }
        });
    }

    fn games(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Find a game");
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.filter.search)
                    .hint_text("Search title or system")
                    .desired_width(240.0),
            );
            let mut changed = search.changed();
            let previous_platform = self.filter.platform.clone();
            egui::ComboBox::from_id_salt("v2_platform_filter")
                .selected_text(if self.filter.platform.is_empty() {
                    "All systems"
                } else {
                    &self.filter.platform
                })
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut self.filter.platform, String::new(), "All systems")
                        .changed();
                    for (platform, count) in &self.library.platforms {
                        changed |= ui
                            .selectable_value(
                                &mut self.filter.platform,
                                platform.clone(),
                                format!("{platform} ({count})"),
                            )
                            .changed();
                    }
                });
            if self.filter.platform != previous_platform {
                self.filter.select_platform(self.filter.platform.clone());
            }
            changed |= ui
                .checkbox(&mut self.filter.attention_only, "Needs attention")
                .changed();
            changed |= ui
                .selectable_value(&mut self.filter.list, false, "Grid")
                .changed();
            changed |= ui
                .selectable_value(&mut self.filter.list, true, "List")
                .changed();
            if changed {
                self.change_filter();
            }
        });
        ui.horizontal_wrapped(|ui| {
            if !self.filter_inflight && self.filter_dirty.is_none() {
                ui.label(format!("{} catalogued games shown", self.indices.len()));
            }
            if self.filter_inflight || self.filter_dirty.is_some() {
                ui.spinner();
                ui.label("Updating results…");
            }
            if ui.button("Clear filters").clicked() {
                self.filter = Default::default();
                self.change_filter();
            }
            if ui
                .add_enabled(
                    self.load_job.is_none(),
                    egui::Button::new("Reload game list"),
                )
                .clicked()
            {
                self.load(false);
            }
        });
        if !self.filter.platform.is_empty() {
            ui.label(format!("{}: {} catalogued games before search or health filters. Other systems remain separate.", self.filter.platform, self.library.platforms.get(&self.filter.platform).copied().unwrap_or(0)));
            if self.filter.platform == "Arcade" {
                ui.label("This is the saved game list, not a count of files on disk. Extracted arcade ROM parts may not be catalogued as games.");
            }
            if ui
                .small_button("Missing games? Review folders and scan")
                .clicked()
            {
                self.go(Route::Section(Section::Sources));
            }
        }
        ui.collapsing("Advanced details", |ui| {
            egui::ScrollArea::vertical().id_salt("v2_filter_details").max_height(150.0).show(ui, |ui| {
            ui.label(format!("Selected platform ID: {} · display label: {}", self.filter.platform, self.filter.platform));
            ui.label("Platform IDs are the catalogue's exact current assignments; no alias grouping or artwork filter is applied.");
            ui.label(format!("Search: {:?} · needs attention only: {} · source/path filter: none", self.filter.search, self.filter.attention_only));
            ui.label(format!("Logical filtered rows: {} · updating: {}", self.indices.len(), self.filter_inflight || self.filter_dirty.is_some()));
            for (platform, sources) in &self.library.platform_sources {
                if self.filter.platform.is_empty() || *platform == self.filter.platform {
                    ui.label(format!("{platform}: {} total catalogue rows", self.library.platforms[platform]));
                    for ((id, path), count) in sources {
                        ui.label(format!("Source {id}: {} · {count} rows", path.display()));
                    }
                }
            }
            });
        });
        if !self.loaded {
            ui.spinner();
            ui.label("Loading your game list in the background. Progress is in Activity.");
            return;
        }
        if self.filter_inflight || self.filter_dirty.is_some() {
            // Never paint the previous platform's result under the new selection.
            return;
        }
        if self.indices.is_empty() && !self.filter_inflight && self.filter_dirty.is_none() {
            ui.heading(if self.library.games.is_empty() {
                "Your games can go here"
            } else {
                "No games match these choices"
            });
            ui.label("Try another system or clear the search. If a game folder is missing, add it through Sources.");
            if primary(
                ui,
                if self.library.games.is_empty() {
                    "Add my games"
                } else {
                    "Show all games"
                },
            ) {
                if self.library.games.is_empty() {
                    self.go(Route::Section(Section::Sources));
                } else {
                    self.filter = Default::default();
                    self.change_filter();
                }
            }
            return;
        }
        let list = self.filter.list;
        let columns = if list {
            1
        } else {
            ((ui.available_width() + 12.0) / 205.0).floor().max(1.0) as usize
        };
        let width = ((ui.available_width() - 12.0 * columns.saturating_sub(1) as f32)
            / columns as f32)
            .max(150.0);
        let height = if list { 94.0 } else { 272.0 };
        let rows = self.indices.len().div_ceil(columns);
        let library = self.library.clone();
        egui::ScrollArea::vertical()
            .id_salt(("v2_games", list, &self.filter.platform))
            .show_rows(ui, height, rows, |ui, visible| {
                for row in visible {
                    ui.horizontal(|ui| {
                        for column in 0..columns {
                            let Some(index) = self.indices.get(row * columns + column) else {
                                continue;
                            };
                            let Some(game) = library.games.get(*index) else {
                                continue;
                            };
                            ui.push_id(game.archive.id, |ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(width, height),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        egui::Frame::group(ui.style()).show(ui, |ui| {
                                            ui.set_width(width - 18.0);
                                            if list {
                                                ui.horizontal(|ui| {
                                                    self.picture(
                                                        ui,
                                                        game,
                                                        Kind::Cover,
                                                        egui::vec2(46.0, 62.0),
                                                    );
                                                    ui.vertical(|ui| {
                                                        self.game_link(ui, game);
                                                        self.duplicate_badge(ui, game);
                                                        ui.add(
                                                            egui::Label::new(format!(
                                                                "{} · {}",
                                                                game.platform,
                                                                game.status()
                                                            ))
                                                            .truncate(),
                                                        );
                                                    });
                                                });
                                            } else {
                                                self.picture(
                                                    ui,
                                                    game,
                                                    Kind::Cover,
                                                    egui::vec2((width - 20.0).min(175.0), 150.0),
                                                );
                                                self.game_link(ui, game);
                                                self.duplicate_badge(ui, game);
                                                ui.add(egui::Label::new(&game.platform).truncate());
                                                ui.add(egui::Label::new(game.status()).truncate());
                                            }
                                        });
                                    },
                                );
                            });
                        }
                    });
                }
            });
    }

    fn game_link(&mut self, ui: &mut egui::Ui, game: &Game) {
        if ui
            .add_sized(
                [ui.available_width(), 40.0],
                egui::Button::new(&game.title).truncate(),
            )
            .on_hover_text("Open game details")
            .clicked()
        {
            self.go(Route::Game(game.archive.id));
        }
    }

    fn duplicate_badge(&mut self, ui: &mut egui::Ui, game: &Game) {
        let Some((count, _key)) = self.duplicate_report.as_ref().and_then(|report| {
            report.groups.iter().find_map(|group| {
                group
                    .members
                    .iter()
                    .any(|member| member.path == game.archive.absolute_path)
                    .then_some((group.members.len(), group.sha256.clone()))
            })
        }) else {
            return;
        };
        if ui.small_button(format!("{count} exact copies")).clicked() {
            self.go(Route::Section(Section::Duplicates));
        }
    }

    fn picture(&mut self, ui: &mut egui::Ui, game: &Game, kind: Kind, size: egui::Vec2) {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let key = self.artwork.request(game.archive.id, kind);
        if let Some(Picture::Ready { texture, .. }) = self.artwork.pictures.get(&key) {
            let aspect = texture.size_vec2();
            let factor = (size.x / aspect.x).min(size.y / aspect.y);
            let target = egui::Rect::from_center_size(rect.center(), aspect * factor);
            ui.painter().image(
                texture.id(),
                target,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            ui.painter()
                .rect_filled(rect, 8.0, Color32::from_rgb(38, 51, 69));
            let letter = game
                .title
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(36.0),
                Color32::from_rgb(183, 205, 227),
            );
            if size.x > 90.0 {
                let label = match self.artwork.pictures.get(&key) {
                    Some(Picture::Missing) => "No picture yet",
                    Some(Picture::Failed(_)) => "Picture unavailable",
                    _ if self.artwork.paused => "Pictures paused",
                    _ => "Loading picture…",
                };
                ui.painter().text(
                    rect.center_bottom() - egui::vec2(0.0, 13.0),
                    egui::Align2::CENTER_BOTTOM,
                    label,
                    egui::FontId::proportional(16.0),
                    Color32::WHITE,
                );
            }
        }
    }

    fn platforms(&mut self, ui: &mut egui::Ui) {
        if self.library.platforms.is_empty() {
            ui.label("No systems are listed yet. Discover your game folders to get started.");
            if primary(ui, "Add my games") {
                self.go(Route::Section(Section::Sources));
            }
            return;
        }
        let library = self.library.clone();
        egui::ScrollArea::vertical()
            .id_salt("v2_platforms")
            .show(ui, |ui| {
                for (platform, count) in &library.platforms {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.heading(platform);
                        ui.label(format!("{count} games available to browse"));
                        let ready = platform.eq_ignore_ascii_case("Arcade") || *count > 0;
                        ui.label(if ready {
                            "Verification data ready · game folder configured"
                        } else {
                            "Needs setup"
                        });
                        if ui
                            .button(if ready {
                                format!("Check {platform}")
                            } else {
                                format!("Set up {platform}")
                            })
                            .clicked()
                        {
                            self.check_platform(platform.clone());
                        }
                        if primary(ui, &format!("View {platform} games")) {
                            self.filter.select_platform(platform.clone());
                            self.filter.attention_only = false;
                            self.change_filter();
                            self.go(Route::Section(Section::Games));
                        }
                        ui.label("Next: select a game to see readiness, pictures and actions.");
                    });
                }
            });
    }

    fn check_games(&mut self, ui: &mut egui::Ui) {
        let platform = self.check_platform.clone();
        check_scroll(ui, platform.as_deref(), |ui| self.check_games_content(ui));
    }

    fn check_games_content(&mut self, ui: &mut egui::Ui) {
        ui.label("Read-only verification. Your original game files are never renamed, moved or deleted here.");
        if self.check_platform.is_none() {
            ui.heading("Choose a platform");
            for (platform, count) in self.library.platforms.clone() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.heading(&platform);
                    ui.label(format!("{count} games"));
                    ui.label(if platform.eq_ignore_ascii_case("Arcade") {
                        "MAME Arcade verification data ready"
                    } else {
                        "Verification data ready"
                    });
                    if primary(ui, &format!("Check {platform}")) {
                        self.check_platform(platform.clone());
                    }
                });
            }
            if self.library.platforms.is_empty() {
                ui.label("Add a game folder first, then return here to choose a platform.");
            }
            return;
        }
        let platform = self.check_platform.clone().unwrap_or_default();
        ui.heading(format!("{platform} verification"));
        ui.label(if platform.eq_ignore_ascii_case("Arcade") {
            "MAME / Arcade · machine verification data is independent of software lists."
        } else {
            "Verification data is available for this platform."
        });
        if let Some(result) = &self.verification {
            ui.label(format!("Checked {} files", result.total));
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Matched: {}", result.matched));
                ui.label(format!("Needs attention: {}", result.attention));
                ui.label(format!("Unknown: {}", result.unknown));
                ui.label(format!("Missing: {}", result.missing));
            });
            for (label, count, explanation) in [
                (
                    "Matched",
                    result.matched,
                    "These files match the saved identity evidence.",
                ),
                (
                    "Needs attention",
                    result.attention,
                    "These files have a saved health or scan warning.",
                ),
                (
                    "Unknown",
                    result.unknown,
                    "EmuWiz has not yet established a trusted identity.",
                ),
                (
                    "Missing expected files",
                    result.missing,
                    "The recorded path is not available right now.",
                ),
            ] {
                if count > 0 && ui.button(format!("Review {label} ({count})")).clicked() {
                    self.filter.select_platform(platform.clone());
                    self.filter.attention_only = label == "Needs attention";
                    self.change_filter();
                    self.go(Route::Section(Section::Games));
                }
                if count > 0 {
                    ui.label(explanation);
                }
            }
            if ui.button("Choose another platform").clicked() {
                self.check_platform = None;
                self.verification = None;
            }
        } else {
            ui.label("Ready to check this platform.");
            if primary(ui, &format!("Check {platform}")) {
                self.start_verification();
            }
        }
        if let Some(id) = self
            .verification_job
            .and_then(|id| self.activity.jobs.get(&id))
            .filter(|job| job.active())
        {
            ui.separator();
            ui.label("Checking in Activity");
            if let Some((done, total)) = id.progress {
                ui.add(
                    egui::ProgressBar::new((done as f32 / total.max(1) as f32).min(1.0))
                        .show_percentage(),
                );
            }
        }
    }

    fn duplicates(&mut self, ui: &mut egui::Ui) {
        mrwiz_tip(
            ui,
            "I found exact copies only when the backend proves their bytes match.",
            &mut self.mrwiz_dismissed,
        );
        ui.label("Only byte-identical files are called exact duplicates. Different regions, revisions and titles remain separate unless the backend proves identical content.");
        if self.duplicate_report.is_none() {
            if self.duplicate_job.is_some() {
                ui.spinner();
                ui.label("Hashing candidate files safely…");
            } else if primary(ui, "Find exact duplicates") {
                self.start_duplicate_scan();
            }
            return;
        }
        let report = self.duplicate_report.clone().unwrap_or_default();
        ui.label(format!(
            "{} files examined · {} exact groups",
            report.files_examined,
            report.groups.len()
        ));
        for (index, group) in report.groups.iter().enumerate() {
            let key = format!("{}:{}", group.sha256, index);
            if self.duplicate_ignored.contains(&key) {
                continue;
            }
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.heading(format!("{} · {} copies", group.kind, group.members.len()));
                ui.label(format!("{} bytes · {}", group.size_bytes, group.sha256));
                for member in &group.members {
                    ui.label(format!("{} · {} · {} bytes", member.title, member.path.display(), member.size_bytes));
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Keep both").clicked() { self.duplicate_ignored.insert(key.clone()); }
                    if ui.button("Ignore group").clicked() { self.duplicate_ignored.insert(key.clone()); }
                    if ui.button("Quarantine duplicate").clicked() { self.go(Route::Section(Section::Problems)); }
                });
                ui.label("Quarantine is recoverable and must be reviewed in Problems & Repair; there is no delete action here.");
            });
        }
    }

    fn problems(&mut self, ui: &mut egui::Ui) {
        let summary = ProblemSummary::from_library(&self.library, self.duplicate_report.as_ref());
        egui::ScrollArea::vertical().id_salt("v2_problems").show(ui, |ui| {
            ui.label("Here is what needs attention, with the safest next action. Opening this page is read-only.");
            if let Some(message) = self.repair_result.clone() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.strong("Repair complete");
                    ui.label(message);
                    if ui.button("Open History").clicked() { self.go(Route::Section(Section::History)); }
                });
            }
            if let Some(preview) = self.repair_preview.clone() {
                self.duplicate_preview(ui, &preview);
            }
            if summary.problems.is_empty() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Nothing needs attention right now.");
                    ui.label("EmuWiz has no saved file, identity, or duplicate findings to show.");
                    if primary(ui, "Verify my games") { self.go(Route::Section(Section::Check)); }
                });
                return;
            }
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("Needs attention: {}", summary.count(Severity::NeedsAttention)));
                ui.label(format!("Warnings: {}", summary.count(Severity::Warning)));
                ui.label(format!("Informational: {}", summary.count(Severity::Informational)));
            });
            if primary(ui, "Review problems") && self.problem_selected.is_none() {
                self.problem_selected = summary.problems.first().map(|problem| problem.id.clone());
            }
            if self.duplicate_report.is_none() {
                ui.label("Exact duplicates have not been checked in this session.");
                if ui.button("Check for exact duplicates").clicked() { self.start_duplicate_scan(); }
            }
            let mut selected = self.problem_selected.clone();
            for category in [Category::Files, Category::Duplicates, Category::Identity, Category::Verification] {
                let entries: Vec<_> = summary.problems.iter().filter(|problem| problem.category == category).collect();
                if entries.is_empty() { continue; }
                ui.separator();
                ui.heading(category.label());
                for problem in entries {
                    let is_selected = selected.as_deref() == Some(problem.id.as_str());
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(&problem.title);
                            ui.label(problem.severity.label());
                            if ui.button(if is_selected { "Hide details" } else { "View details" }).clicked() {
                                selected = (!is_selected).then(|| problem.id.clone());
                            }
                        });
                        ui.label(&problem.affected);
                        ui.label(format!("Recommended action: {}", problem.action));
                        if problem.category == Category::Duplicates
                            && self.repair_preview.is_none()
                            && self.repair_job.is_none()
                            && self.duplicate_report.is_some()
                            && let Some(index) = problem.id.strip_prefix("duplicate-").and_then(|id| id.split('-').next()).and_then(|id| id.parse::<usize>().ok())
                            && ui.button("Preview safe quarantine").clicked()
                        {
                            self.start_duplicate_preview(index);
                        }
                        if is_selected { self.problem_details(ui, problem); }
                    });
                }
            }
            self.problem_selected = selected;
        });
    }

    fn duplicate_preview(
        &mut self,
        ui: &mut egui::Ui,
        preview: &super::backend::DuplicateRepairPreview,
    ) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Repair preview: quarantine duplicate files");
            ui.label("EmuWiz will move only the redundant byte-identical copies into its recoverable quarantine. It will not delete the retained copy or alter unrelated files.");
            ui.label(format!("{} file(s) will change · {} byte(s) · retained copy: {}", preview.proposals.len(), preview.group.reclaimable_bytes, preview.group.recommendation.retained_path().map_or("unknown".into(), |path| path.display().to_string())));
            for proposal in &preview.proposals {
                if let archivefs_core::repair::RepairAction::MovePath { destination } = &proposal.action {
                    ui.label(format!("Move {} → {}", proposal.source_path.display(), destination.display()));
                }
            }
            ui.label("Backups are not needed: the transaction journal records the move and undo restores the original path if the files remain unchanged.");
            ui.label("Safe to undo: yes, while the quarantine and original paths remain under EmuWiz's transaction control.");
            ui.horizontal_wrapped(|ui| {
                if primary(ui, "Confirm quarantine") { self.repair_confirm = true; }
                if ui.button("Cancel preview").clicked() { self.repair_preview = None; }
            });
            ui.collapsing("Advanced Details", |ui| {
                ui.monospace(format!("Trusted root: {}\nJournal directory: {}", preview.trusted_root.display(), preview.journal_dir.display()));
            });
        });
        if self.repair_confirm {
            egui::Window::new("Confirm safe quarantine").collapsible(false).resizable(false).show(ui.ctx(), |ui| {
                ui.label(format!("Move {} redundant file(s) to recoverable quarantine?", preview.proposals.len()));
                ui.label("The current preview will be revalidated immediately before any change. If a file changed, EmuWiz will refuse the whole repair.");
                if primary(ui, "Apply quarantine") { self.apply_duplicate_preview(); }
                if ui.button("Cancel").clicked() { self.repair_confirm = false; }
            });
        }
    }

    fn history(&mut self, ui: &mut egui::Ui) {
        ui.label("Previous repairs and playing-library builds are shown from the durable transaction journal. Browsing history changes nothing.");
        let has_cheat_history = self
            .native_workflows
            .as_ref()
            .is_some_and(super::native_workflows::NativeWorkflows::has_cheat_history);
        if self.repair_history.is_empty()
            && self.playing_library_history.is_empty()
            && self.canonical_organisation_history.is_empty()
            && !has_cheat_history
        {
            ui.heading("No repair history yet");
            ui.label(
                "When a supported repair completes, its receipt and undo status will appear here.",
            );
            return;
        }
        let mut open_build = false;
        let open_cheats = self
            .native_workflows
            .as_ref()
            .is_some_and(|workflows| workflows.show_cheat_history(ui));
        if !self.playing_library_history.is_empty() {
            ui.heading("Playing libraries");
            for transaction in self.playing_library_history.iter().rev() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.heading("Built Playing Library");
                    ui.label(format!(
                        "{} · {} link(s)",
                        transaction.transaction_id,
                        transaction.entries.len()
                    ));
                    ui.label(format!("Destination: {}", transaction.source_scan_root));
                    match transaction.state {
                        TransactionState::Applied => {
                            ui.strong("Ready to undo");
                            if ui.button("Open Build Library to preview undo").clicked() {
                                open_build = true;
                            }
                        }
                        TransactionState::RolledBack => {
                            ui.label("Already undone");
                        }
                        _ => {
                            ui.label("Needs review — the transaction did not finish normally.");
                        }
                    }
                    ui.collapsing("Advanced Details", |ui| {
                        ui.label(
                            "Shared journaled link transaction from the Playing Library planner.",
                        );
                    });
                });
            }
        }
        if !self.canonical_organisation_history.is_empty() {
            ui.heading("Organised verified games");
            for transaction in self.canonical_organisation_history.iter().rev() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.heading("Organised verified games");
                    ui.label(format!(
                        "{} · {} item(s)",
                        transaction.transaction_id,
                        transaction.entries.len()
                    ));
                    ui.label(format!("Source: {}", transaction.source_scan_root));
                    ui.label(match transaction.state {
                        TransactionState::Applied => "Undo available from Organisation",
                        TransactionState::RolledBack => "Already undone",
                        _ => "Needs review — recovery state is recorded in the journal",
                    });
                    ui.collapsing("Advanced Details", |ui| {
                        ui.label(format!("State: {}", transaction.state.label()));
                    });
                });
            }
        }
        let mut undo = None;
        egui::ScrollArea::vertical()
            .id_salt("v2_repair_history")
            .show(ui, |ui| {
                for (index, record) in self.repair_history.iter().enumerate().rev() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.heading("Duplicate quarantine");
                        ui.label(format!(
                            "Transaction {} · {} · {} file(s)",
                            record.transaction.transaction_id,
                            record.transaction.state.label(),
                            record.transaction.entries.len()
                        ));
                        ui.label(format!(
                            "Affected folder: {}",
                            record.trusted_root.display()
                        ));
                        match record.transaction.state {
                            TransactionState::Applied => {
                                ui.strong("Ready to undo");
                                if primary(ui, "Preview undo") {
                                    undo = Some(index);
                                }
                            }
                            TransactionState::RolledBack => {
                                ui.label("Already undone");
                            }
                            TransactionState::RollbackFailed => {
                                ui.label("Undo is not available — the rollback needs review.");
                            }
                            _ => {
                                ui.label("Needs review — the transaction did not finish normally.");
                            }
                        }
                        ui.collapsing("Advanced Details", |ui| {
                            ui.monospace(format!("Journal: {}", record.journal_dir.display()));
                        });
                    });
                }
            });
        if let Some(index) = undo {
            self.undo_confirm = Some(index);
        }
        if let Some(index) = self.undo_confirm
            && let Some(transaction_id) = self
                .repair_history
                .get(index)
                .map(|record| record.transaction.transaction_id.clone())
        {
            egui::Window::new("Confirm undo").collapsible(false).resizable(false).show(ui.ctx(), |ui| {
                    ui.label("EmuWiz will revalidate the quarantined files and restore them to their original paths. If anything changed unexpectedly, it will refuse safely.");
                    if primary(ui, "Undo this repair") { self.undo_history_entry(index); }
                    if ui.button("Cancel").clicked() { self.undo_confirm = None; }
                    ui.collapsing("Advanced Details", |ui| { ui.monospace(format!("Transaction: {transaction_id}")); });
                });
        }
        if open_build {
            self.go(Route::Section(Section::Build));
        }
        if open_cheats {
            self.go(Route::Section(Section::Mods));
        }
    }

    fn problem_details(&mut self, ui: &mut egui::Ui, problem: &Problem) {
        ui.separator();
        ui.strong("What happened");
        ui.label(&problem.title);
        ui.strong("Why it matters");
        ui.label(&problem.why);
        ui.strong("What EmuWiz can do");
        ui.label(&problem.action);
        ui.strong("Safety and undo");
        ui.label(&problem.safety);
        ui.label(&problem.undo);
        match problem.category {
            Category::Duplicates => {
                if primary(ui, "Review duplicate groups") {
                    self.go(Route::Section(Section::Duplicates));
                }
            }
            Category::Files | Category::Identity | Category::Verification => {
                if primary(ui, "Review games and verify") {
                    self.go(Route::Section(Section::Games));
                }
            }
        }
        ui.collapsing("Advanced details", |ui| {
            ui.monospace(&problem.technical);
        });
    }

    fn game_detail(&mut self, ui: &mut egui::Ui, id: i64) {
        let library = self.library.clone();
        let Some(game) = library.game(id) else {
            ui.label("This game is not in the current game list. It may have moved or its folder may be disconnected.");
            if primary(ui, "Return to games") {
                self.go(Route::Section(Section::Games));
            }
            return;
        };
        let latest_verification = self
            .verification
            .as_ref()
            .filter(|result| result.platform == game.platform)
            .and_then(|result| result.statuses.get(&id))
            .cloned();
        egui::ScrollArea::vertical().id_salt(("v2_detail", id)).show(ui, |ui| {
            if primary(ui, "Play") { self.go(Route::Task { section: Section::Launch, game: id }); }
            ui.label("Next: review the existing launch check. Nothing starts until you choose Launch there.");
            ui.horizontal_top(|ui| {
                self.picture(ui, game, Kind::Cover, egui::vec2(150.0, 200.0));
                ui.vertical(|ui| {
                    ui.strong(&game.platform);
                    ui.label(format!("Media: {}", media_kind_label(&game.archive.archive_kind)));
                    ui.label(format!("Source: {}", game.archive.relative_path.display()));
                    if let Some(status) = &latest_verification {
                        ui.label(format!("Latest verification: {status}"));
                    }
                    if let Some(detail) = self.detail.as_ref().filter(|detail| detail.game == id) {
                        if latest_verification.is_none() { ui.label(if detail.saved_checks > 0 { "Previous verification available" } else { "Not verified yet" }); }
                        if !detail.file_present { ui.label("Needs attention · game file is unavailable"); }
                        else if !detail.unchanged { ui.label("Needs attention · game file changed since the last scan"); }
                        ui.label(detail.emulator_status());
                        if detail.saved_checks > 0 && ui.button("View verification result").clicked() {
                            self.go(Route::Section(Section::Check));
                        }
                    } else if self.detail_failed == Some(id) {
                        ui.label("Readiness could not be checked. Your game has not been changed.");
                        if ui.button("Retry readiness check").clicked() { self.detail_failed = None; }
                    } else { ui.spinner(); ui.label("Checking saved information and looking for installed emulators…"); }
                });
            });
            ui.horizontal_wrapped(|ui| {
                for (label, section) in [("Verify", Section::Check), ("Artwork & Metadata", Section::Artwork), ("Mods & Cheats", Section::Mods), ("Fix Problems", Section::Problems)] {
                    if ui.button(label).clicked() {
                        if section == Section::Check {
                            self.check_platform = Some(game.platform.clone());
                            self.go(Route::Section(Section::Check));
                        } else {
                            self.go(Route::Task { section, game: id });
                        }
                    }
                }
                if ui.button("Open Folder").clicked() {
                    let job = self.activity.queue("Opening the game folder", Route::Game(id), false);
                    self.send(job, Command::OpenFolder(game.archive.absolute_path.clone()));
                }
            });
            let key = self.artwork.key(id, Kind::Cover);
            if let Some(description) = self.artwork.index.as_ref().and_then(|index| index.descriptions.get(&id)) {
                ui.separator(); ui.strong("About this game"); ui.label(description);
            }
            if matches!(self.artwork.pictures.get(&key), Some(Picture::Failed(_))) && ui.button("Retry picture").clicked() { self.artwork.retry(key); }
            let count = self.artwork.index.as_ref().and_then(|index| index.screenshots.get(&id)).map_or(0, Vec::len);
            ui.separator(); ui.strong("Screenshots");
            if count == 0 {
                ui.label(if self.artwork.index.is_none() { "Looking for screenshots…" } else { "No screenshots available. See Advanced details for the search results." });
            }
            else if !self.screenshots { if ui.button(format!("Show screenshots ({count})")).clicked() { self.screenshots = true; } }
            else {
                for ordinal in 0..count {
                    self.picture(ui, game, Kind::Screenshot(ordinal), egui::vec2(240.0, 180.0));
                    let key = self.artwork.key(id, Kind::Screenshot(ordinal));
                    if matches!(self.artwork.pictures.get(&key), Some(Picture::Failed(_))) && ui.button(format!("Retry screenshot {}", ordinal + 1)).clicked() { self.artwork.retry(key); }
                }
            }
            ui.collapsing("Advanced details", |ui| {
                ui.label(if game.identified { "Identified in the saved game list" } else { "Identity is not confirmed" });
                ui.monospace(format!("Media kind: {}", game.archive.archive_kind));
                if let Some(detail) = self.detail.as_ref().filter(|detail| detail.game == id) { ui.label(&detail.technical); }
                if let Some(index) = &self.artwork.index
                    && let Some(diagnostic) = index.diagnostics.get(&id)
                {
                    ui.label(diagnostic);
                }
                ui.label("An installed emulator is not proof that this game can launch. The existing launch planner rechecks identity, media, firmware and profiles.");
                match self.artwork.pictures.get(&key) {
                    Some(Picture::Ready { timings, .. }) => {
                        ui.monospace(format!("{timings:#?}"));
                        ui.label("Remote provider processing includes the existing core's decode, resize and cache publication. V2 decode/resize timings describe the delivered thumbnail; the core does not expose separate internal timings.");
                    }
                    Some(Picture::Failed(error)) => { ui.label(error); }
                    _ => {}
                }
            });
        });
    }

    fn mods_page(&mut self, ui: &mut egui::Ui, game_id: Option<i64>) {
        let selected = game_id.and_then(|id| self.library.game(id));
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()));
        let destination = crate::gui_v2::mods::show_mods_page(
            ui,
            &mut self.mods,
            selected,
            workflows,
            &mut self.activity,
        );
        if let Some(destination) = destination {
            self.go(destination);
        }
    }

    fn launch(&mut self, ui: &mut egui::Ui, game_id: i64) {
        let Some((title, platform, identified, media, path)) =
            self.library.game(game_id).map(|game| {
                (
                    game.title.clone(),
                    game.platform.clone(),
                    game.identified,
                    media_kind_label(&game.archive.archive_kind).to_string(),
                    game.archive.absolute_path.clone(),
                )
            })
        else {
            ui.label("This game is no longer in the current library.");
            if primary(ui, "Return to games") {
                self.go(Route::Section(Section::Games));
            }
            return;
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading(title);
            ui.label(format!("Platform: {platform}"));
            ui.label(format!("Media: {media}"));
            ui.label(if identified {
                "Verified identity is available"
            } else {
                "Game identity needs review"
            });
        });
        ui.add_space(12.0);
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()));
        if workflows.show_launch(ui, game_id, &path, &mut self.activity) {
            self.go(Route::Section(Section::Emulators));
        }
    }

    fn emulator_setup(&mut self, ui: &mut egui::Ui) {
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()));
        egui::ScrollArea::vertical()
            .id_salt("v2_native_emulator_setup")
            .show(ui, |ui| workflows.show_setup(ui));
    }

    fn sources(&mut self, ui: &mut egui::Ui) {
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()));
        workflows.show_sources(ui, &mut self.activity);
    }

    fn dat_sources(&mut self, ui: &mut egui::Ui) {
        let workflows = self
            .native_workflows
            .get_or_insert_with(|| super::native_workflows::NativeWorkflows::new(ui.ctx().clone()));
        workflows.show_dat_sources(ui, &mut self.activity);
    }

    fn advanced(&mut self, ui: &mut egui::Ui) {
        check_scroll(ui, None, |ui| {
            ui.heading("Advanced tools");
            ui.label("These tools are for specialist inspection and troubleshooting. Normal organisation, identification data and setup have their own native pages.");
            if primary(ui, "Open specialist interface") {
                self.legacy(Section::Advanced);
            }
            ui.separator();
            ui.strong("Specialist tools");
            ui.label("Mounts, media-set inspection, storage and journal details remain available in the existing technical interface.");
            ui.add_space(8.0);
            ui.strong("Normal pages");
            if ui.button("Open DAT Management").clicked() {
                self.go(Route::Section(Section::Dat));
            }
            if ui.button("Open Organisation").clicked() {
                self.go(Route::Section(Section::Build));
            }
            ui.collapsing("Advanced details", |ui| {
                ui.label("The specialist interface preserves legacy mount, media, storage and history tools. Opening it changes nothing.");
            });
        });
    }

    fn artwork_metadata(&mut self, ui: &mut egui::Ui, selected: Option<i64>) {
        let library = self.library.clone();
        let mut metadata_changed = false;
        egui::ScrollArea::vertical()
            .id_salt(("v2_artwork_metadata", selected))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label("Existing local media and provider evidence is matched by exact game identity. EmuWiz does not guess from similar titles.");
                if ui
                    .add_enabled(!self.artwork.index_loading, egui::Button::new("Recheck providers"))
                    .clicked()
                {
                    self.refresh_artwork_index();
                }
                if self.artwork.index_loading {
                    ui.spinner();
                    ui.label("Checking metadata and artwork providers…");
                }
                let Some(game_id) = selected else {
                    let (covers, screenshots, descriptions) = self.artwork.index.as_ref().map_or(
                        (0, 0, 0),
                        |index| (index.covers.len(), index.screenshots.len(), index.descriptions.len()),
                    );
                    ui.heading("Library artwork");
                    ui.label(format!("{covers} covers · {screenshots} games with screenshots · {descriptions} metadata descriptions"));
                    let workflows = self.native_workflows.get_or_insert_with(|| {
                        super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
                    });
                    workflows.show_metadata_tools(ui, None, &mut self.activity);
                    if library.games.is_empty() {
                        ui.label("No games are available yet. Add or scan a source first.");
                        if primary(ui, "Open Sources") {
                            self.go(Route::Section(Section::Sources));
                        }
                    } else {
                        if library.games.len() > 200 {
                            ui.label("Showing the first 200 games here. Open any other game from Games, then choose Artwork & Metadata.");
                        }
                        for game in library.games.iter().take(200) {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    self.picture(ui, game, Kind::Cover, egui::vec2(72.0, 96.0));
                                    ui.vertical(|ui| {
                                        ui.strong(&game.title);
                                        ui.label(&game.platform);
                                        ui.label(self.artwork_summary(game.archive.id, game.screenscraper.is_some()));
                                        if ui.button("View artwork and metadata").clicked() {
                                            self.go(Route::Task { section: Section::Artwork, game: game.archive.id });
                                        }
                                    });
                                });
                            });
                        }
                    }
                    return;
                };
                let Some(game) = library.game(game_id) else {
                    ui.label("This game is no longer in the current library.");
                    return;
                };
                ui.heading(&game.title);
                ui.label(&game.platform);
                ui.horizontal_top(|ui| {
                    self.picture(ui, game, Kind::Cover, egui::vec2(180.0, 240.0));
                    ui.vertical(|ui| {
                        ui.strong("Metadata");
                        if let Some(description) = self.artwork.index.as_ref().and_then(|index| index.descriptions.get(&game_id)) {
                            ui.label(description);
                        } else if let Some(saved) = &game.screenscraper {
                            ui.label(saved.values.synopsis.as_deref().unwrap_or("ScreenScraper metadata is saved for this game."));
                        } else if !game.identified {
                            ui.label("Identity needs review before provider metadata can be matched safely.");
                        } else if self.artwork.index.as_ref().is_some_and(|index| !index.warnings.is_empty()) {
                            ui.label("A metadata provider is unavailable, so this search may be incomplete. Existing local artwork remains usable.");
                        } else {
                            ui.label("No metadata record matched this game.");
                        }
                        self.artwork_provenance(ui, game_id, game.screenscraper.as_ref());
                    });
                });
                let count = self.artwork.index.as_ref().and_then(|index| index.screenshots.get(&game_id)).map_or(0, Vec::len);
                ui.separator();
                ui.strong("Screenshots");
                if count == 0 {
                    ui.label(if self.artwork.index_loading { "Looking for screenshots…" } else { "No screenshot found. Advanced Details explains which providers were checked." });
                } else {
                    for ordinal in 0..count {
                        self.picture(ui, game, Kind::Screenshot(ordinal), egui::vec2(320.0, 220.0));
                    }
                }
                ui.collapsing("Advanced Details", |ui| {
                    ui.monospace(format!("Original path: {}", game.archive.absolute_path.display()));
                    if let Some(saved) = &game.screenscraper {
                        ui.label(format!("ScreenScraper record: {}", saved.receipt.provider_record_id));
                        ui.label(format!("Match evidence: {}", saved.receipt.match_basis));
                    }
                    if let Some(index) = &self.artwork.index {
                        if let Some(diagnostic) = index.diagnostics.get(&game_id) { ui.label(diagnostic); }
                        ui.label(format!("Provider lookup: {} ms", index.elapsed_ms));
                        for warning in &index.warnings { ui.label(warning); }
                    }
                });
                let workflows = self.native_workflows.get_or_insert_with(|| {
                    super::native_workflows::NativeWorkflows::new(ui.ctx().clone())
                });
                metadata_changed = workflows.show_metadata_tools(
                    ui,
                    Some((game_id, &game.archive.absolute_path)),
                    &mut self.activity,
                );
            });
        if metadata_changed {
            self.load(false);
        }
    }

    fn artwork_summary(&self, game: i64, screenscraper: bool) -> String {
        let Some(index) = &self.artwork.index else {
            return "Checking providers…".into();
        };
        let cover = index.covers.contains_key(&game);
        let screenshots = index.screenshots.get(&game).map_or(0, Vec::len);
        let metadata = index.descriptions.contains_key(&game) || screenscraper;
        format!(
            "{} · {screenshots} screenshot(s) · {}",
            if cover {
                "Cover ready"
            } else {
                "No cover found"
            },
            if metadata {
                "Metadata ready"
            } else {
                "No metadata match"
            }
        )
    }

    fn artwork_provenance(
        &self,
        ui: &mut egui::Ui,
        game: i64,
        screenscraper: Option<
            &archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment,
        >,
    ) {
        ui.strong("Sources");
        let index = self.artwork.index.as_ref();
        if let Some(source) = index.and_then(|index| index.covers.get(&game)) {
            ui.label(match source {
                Source::Local(path) => format!("Local file · {}", path.display()),
                Source::Remote { record, .. } => {
                    format!("RomM · record {}", record.provider_game_id)
                }
            });
        }
        if let Some(saved) = screenscraper {
            ui.label(format!(
                "ScreenScraper · record {}",
                saved.receipt.provider_record_id
            ));
        }
        if index.is_some_and(|index| {
            !index.covers.contains_key(&game) && !index.descriptions.contains_key(&game)
        }) && screenscraper.is_none()
        {
            ui.label("No provider record matched.");
        }
    }

    fn handoff(&mut self, ui: &mut egui::Ui, section: Section) {
        egui::ScrollArea::vertical().id_salt(("v2_task", section)).show(ui, |ui| {
            ui.heading("This workflow is available in the existing interface");
            ui.label("This task opens the existing interface in a separate window. GUI v2 stays open so you can return safely.");
            if let Some(game) = self.router.current.game().and_then(|id| self.library.game(id)) { ui.strong(format!("Selected game: {}", game.title)); }
            let action = if section == Section::Launch { "Continue to Play" } else if section == Section::Mods { "Open this game's Mods & Cheats" } else { section.action() };
            if primary(ui, action) { self.legacy(section); }
            ui.label("Next: review what EmuWiz found. Opening the workflow does not approve changes or launch a game.");
            ui.separator();
            if ui.button("View games").clicked() { self.go(Route::Section(Section::Games)); }
            if section == Section::Sources {
                ui.label(format!("{} configured game folders are enabled.", self.library.sources));
                if self.library.sources > 0 && ui.add_enabled(self.load_job.is_none(), egui::Button::new("Scan configured folders…")).clicked() { self.confirm_scan = true; }
                ui.label("Discovery comes first. You can choose folders manually in the existing workflow if discovery cannot find them.");
            }
            ui.collapsing("Advanced details", |ui| {
                ui.label("Migration status: deliberate legacy handoff. Existing safety checks, previews and confirmations remain authoritative.");
                ui.label("The v2 Activity page tracks opening the window. Jobs started there report progress inside that window, not here.");
                if ui.button("Open full technical interface").clicked() { self.legacy(Section::Advanced); }
            });
        });
    }

    fn activities(&mut self, ui: &mut egui::Ui) {
        ui.label("Work continues when you leave this page. No estimated completion time is shown unless it is known.");
        if self.activity.jobs.is_empty() {
            ui.label("Nothing is running yet. Browse your games to get started.");
            if primary(ui, "Browse my games") {
                self.go(Route::Section(Section::Games));
            }
        }
        let mut destination = None;
        egui::ScrollArea::vertical().id_salt("v2_jobs").show(ui, |ui| {
            for (id, job) in self.activity.jobs.iter().rev() {
                ui.push_id(id, |ui| { egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width()); ui.heading(&job.title);
                    ui.strong(match job.phase { Phase::Queued => "Queued", Phase::Running => "Working", Phase::Complete => "Complete", Phase::Failed => "Needs attention", Phase::Cancelled => "Cancelled safely" });
                    ui.label(&job.summary);
                    ui.label(format!("Elapsed: {} seconds", job.elapsed().as_secs()));
                    if let Some((done, total)) = job.progress { ui.label(format!("{done} / {total} requests finished (including off-screen cancellations)")); }
                    if let Some(fraction) = job.fraction() { ui.add(egui::ProgressBar::new(fraction).show_percentage()); }
                    if let Some(item) = &job.item { ui.label(item); }
                    if job.active() {
                        if job.cancel.is_some() { if primary(ui, "Cancel safely") { job.request_cancel(); } }
                        else { ui.label("This operation must finish safely; you can keep browsing."); }
                    } else if primary(ui, if job.phase == Phase::Failed { "Return to task / retry" } else { "View result" }) { destination = Some(job.result.clone()); }
                    ui.collapsing("Technical details", |ui| { ui.label(&job.technical); });
                }); });
            }
        });
        if let Some(route) = destination {
            self.go(route);
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.label("Readable text and a permanent sidebar are on by default. The library view and current location are remembered separately from the legacy interface.");
        if primary(ui, "Return Home") {
            self.go(Route::Home);
        }
        ui.label("Next: choose a task. All existing application settings remain available below.");
        ui.collapsing("Advanced details", |ui| {
            ui.label(format!(
                "Library load: {} ms · {} games",
                self.library.load_ms,
                self.library.games.len()
            ));
            if let Some(index) = &self.artwork.index {
                ui.label(format!(
                    "Artwork lookup index: {} ms · {} covers",
                    index.elapsed_ms,
                    index.covers.len()
                ));
                for warning in &index.warnings {
                    ui.label(warning);
                }
            }
            ui.label(format!(
                "Artwork workers: {} local + {} remote. Timing details are on each game.",
                super::artwork::LOCAL_WORKERS,
                super::artwork::REMOTE_WORKERS
            ));
            if ui.button("Open existing application settings").clicked() {
                self.legacy(Section::Settings);
            }
        });
    }
}
