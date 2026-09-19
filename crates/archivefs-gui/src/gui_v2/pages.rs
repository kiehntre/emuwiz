//! Paint-only pages: layout, in-memory projections and explicit user intents.
use super::{
    App,
    activity::Phase,
    artwork::Picture,
    backend::Command,
    library::Game,
    media_sources::Kind,
    routes::{HOME_TASKS, Route, SECTIONS, Section},
};
use eframe::egui::{self, Color32, RichText};

fn primary(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(
        egui::Button::new(RichText::new(text).strong())
            .fill(Color32::from_rgb(30, 85, 137))
            .min_size(egui::vec2(180.0, 44.0)),
    )
    .clicked()
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
                        ui.separator();
                        if ui.button("Legacy / Advanced interface").clicked() {
                            self.go(Route::Section(Section::Advanced));
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
                Route::Section(Section::Games | Section::Launch | Section::Mods) => self.games(ui),
                Route::Section(Section::Platforms) => self.platforms(ui),
                Route::Game(id) => self.game_detail(ui, id),
                Route::Section(Section::Activity) => self.activities(ui),
                Route::Section(Section::Settings) => self.settings(ui),
                Route::Task { section, .. } | Route::Section(section) => self.handoff(ui, section),
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

    fn home(&mut self, ui: &mut egui::Ui) {
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
            ui.label(format!("{} games shown", self.indices.len()));
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
        if !self.loaded {
            ui.spinner();
            ui.label("Loading your game list in the background. Progress is in Activity.");
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
                        if primary(ui, &format!("View {platform} games")) {
                            self.filter.platform = platform.clone();
                            self.filter.search.clear();
                            self.filter.attention_only = false;
                            self.change_filter();
                            self.go(Route::Section(Section::Games));
                        }
                        ui.label("Next: select a game to see readiness, pictures and actions.");
                    });
                }
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
        egui::ScrollArea::vertical().id_salt(("v2_detail", id)).show(ui, |ui| {
            if primary(ui, "Play") { self.go(Route::Task { section: Section::Launch, game: id }); }
            ui.label("Next: review the existing launch check. Nothing starts until you choose Launch there.");
            ui.horizontal_top(|ui| {
                self.picture(ui, game, Kind::Cover, egui::vec2(150.0, 200.0));
                ui.vertical(|ui| {
                    ui.strong(&game.platform);
                    ui.label(if game.identified { "Identified in the saved game list" } else { "Identity not confirmed yet — use Verify to check this game" });
                    if let Some(detail) = self.detail.as_ref().filter(|detail| detail.game == id) {
                        ui.label(if !detail.file_present { "Needs attention · game file is no longer available" } else if !detail.unchanged { "Needs attention · game file changed since the last scan" } else { "Game file available · size and date match the last scan" });
                        ui.label(detail.emulator_status());
                        ui.label(if detail.saved_checks > 0 { "Previous verification information is available. Verify checks it again." } else { "Not verified yet. Verify shows the available setup." });
                    } else if self.detail_failed == Some(id) {
                        ui.label("Readiness could not be checked. Your game has not been changed.");
                        if ui.button("Retry readiness check").clicked() { self.detail_failed = None; }
                    } else { ui.spinner(); ui.label("Checking saved information and looking for installed emulators…"); }
                });
            });
            ui.horizontal_wrapped(|ui| {
                for (label, section) in [("Verify", Section::Check), ("Mods & Cheats", Section::Mods), ("Fix Problems", Section::Problems)] {
                    if ui.button(label).clicked() { self.go(Route::Task { section, game: id }); }
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
            if count == 0 { ui.label("No screenshots found yet. You can check available pictures in Artwork & Metadata."); }
            else if !self.screenshots { if ui.button(format!("Show screenshots ({count})")).clicked() { self.screenshots = true; } }
            else {
                for ordinal in 0..count {
                    self.picture(ui, game, Kind::Screenshot(ordinal), egui::vec2(240.0, 180.0));
                    let key = self.artwork.key(id, Kind::Screenshot(ordinal));
                    if matches!(self.artwork.pictures.get(&key), Some(Picture::Failed(_))) && ui.button(format!("Retry screenshot {}", ordinal + 1)).clicked() { self.artwork.retry(key); }
                }
            }
            ui.collapsing("Advanced details", |ui| {
                if let Some(detail) = self.detail.as_ref().filter(|detail| detail.game == id) { ui.label(&detail.technical); }
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
