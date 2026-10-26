//! Dispatch for the native Blue pages of former legacy-only tools (Batch 10).
//! Each tool owns its module; this only routes to it and holds their state.

use eframe::egui;

use super::App;
use super::routes::ToolPage;

#[derive(Default)]
pub(super) struct ToolPagesState {
    pub needs_attention: super::needs_attention_page::NeedsAttentionState,
    pub mounts: super::mounts_page::MountsState,
    pub media_sets: super::media_sets_tool::MediaSetsToolState,
    pub library_views: super::library_views_page::State,
    pub view_history: super::view_history_page::ViewHistoryState,
    pub platform_aliases: super::platform_aliases_page::PlatformAliasesState,
    pub database: super::database_status_page::DatabaseStatusState,
    pub alternative_provider_preview: super::alternative_provider_preview::State,
    pub cheat_extras: super::cheat_extras_page::CheatExtrasState,
}

impl ToolPagesState {
    pub(super) fn live() -> Self {
        Self {
            needs_attention: super::needs_attention_page::NeedsAttentionState::live(),
            mounts: super::mounts_page::MountsState::live(),
            media_sets: Default::default(),
            library_views: Default::default(),
            view_history: Default::default(),
            platform_aliases: super::platform_aliases_page::PlatformAliasesState::live(),
            database: super::database_status_page::DatabaseStatusState::live(),
            alternative_provider_preview: Default::default(),
            cheat_extras: Default::default(),
        }
    }
}

impl App {
    pub(super) fn tool_page(&mut self, ui: &mut egui::Ui, tool: ToolPage) {
        match tool {
            ToolPage::AlternativeProviders => super::alternative_provider_preview::show(
                ui,
                &mut self.tools.alternative_provider_preview,
            ),
            ToolPage::NeedsAttention => {
                self.tools.needs_attention.poll(ui.ctx());
                if let Some(route) =
                    super::needs_attention_page::show(ui, &mut self.tools.needs_attention)
                {
                    self.go(route);
                }
            }
            ToolPage::MediaSets => self.media_sets_tool_page(ui),
            ToolPage::CheatExtras => {
                super::cheat_extras_page::show(ui, &mut self.tools.cheat_extras);
            }
            ToolPage::DatabaseStatus => {
                use super::database_status_page::{Request, show};
                let app_scan = self.load_job.is_some();
                let state = &mut self.tools.database;
                state.poll(ui.ctx());
                // The app's own scan finished: show the new status.
                if state.saw_app_scan && !app_scan {
                    state.refresh(ui.ctx());
                }
                state.saw_app_scan = app_scan;
                let request = show(ui, state, app_scan);
                // Kept until no other load is in flight, so the reload is not lost.
                if self.load_job.is_none()
                    && std::mem::take(&mut self.tools.database.library_changed)
                {
                    self.load(false);
                }
                match request {
                    Some(Request::Scan) => self.confirm_scan = true,
                    Some(Request::OpenActivity) => self.go(super::routes::Route::Section(
                        super::routes::Section::Activity,
                    )),
                    None => {}
                }
            }
            ToolPage::PlatformAliases => {
                let state = &mut self.tools.platform_aliases;
                state.poll(ui.ctx());
                if let Some(action) = super::platform_aliases_page::show(ui, state) {
                    state.start(action, ui.ctx());
                }
            }
            ToolPage::LibraryViews => self.library_views_page(ui),
            ToolPage::ViewHistory => {
                use super::view_history_page::{Request, show};
                match show(ui, &mut self.tools.view_history) {
                    Some(Request::OpenViews) => {
                        self.go(super::routes::Route::Tool(ToolPage::LibraryViews))
                    }
                    Some(Request::OpenActivity) => self.go(super::routes::Route::Section(
                        super::routes::Section::Activity,
                    )),
                    None => {}
                }
            }
            ToolPage::Mounts => {
                use super::mounts_page::{PageRequest, show};
                self.tools.mounts.poll(ui.ctx());
                match show(ui, &mut self.tools.mounts) {
                    Some(PageRequest::Run(job)) => {
                        self.tools.mounts.spawn(job, ui.ctx());
                    }
                    Some(PageRequest::OpenSources) => {
                        self.go(super::routes::Route::Section(
                            super::routes::Section::Sources,
                        ));
                    }
                    Some(PageRequest::OpenInLibrary(path)) => {
                        let game = self
                            .library
                            .games
                            .iter()
                            .find(|game| game.archive.absolute_path == path)
                            .map(|game| game.archive.id);
                        match game {
                            Some(id) => self.go(super::routes::Route::Game(id)),
                            None => {
                                self.tools.mounts.feedback = Some((
                                    false,
                                    format!(
                                        "{} is not in the game list yet. Scan your game folders, then try again.",
                                        path.display()
                                    ),
                                ));
                            }
                        }
                    }
                    None => {}
                }
            }
        }
    }
}
