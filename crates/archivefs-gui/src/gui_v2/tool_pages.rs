//! Dispatch for the native Blue pages of former legacy-only tools (Batch 10).
//! Each tool owns its module; this only routes to it and holds their state.

use eframe::egui;

use super::App;
use super::routes::ToolPage;

#[derive(Default)]
pub(super) struct ToolPagesState {
    pub needs_attention: super::needs_attention_page::NeedsAttentionState,
    pub mounts: super::mounts_page::MountsState,
}

impl ToolPagesState {
    pub(super) fn live() -> Self {
        Self {
            needs_attention: super::needs_attention_page::NeedsAttentionState::live(),
            mounts: super::mounts_page::MountsState::live(),
        }
    }
}

impl App {
    pub(super) fn tool_page(&mut self, ui: &mut egui::Ui, tool: ToolPage) {
        match tool {
            ToolPage::NeedsAttention => {
                self.tools.needs_attention.poll(ui.ctx());
                if let Some(route) =
                    super::needs_attention_page::show(ui, &mut self.tools.needs_attention)
                {
                    self.go(route);
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
