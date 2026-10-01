//! Deterministic viewer state and the actions that drive it.
//!
//! Actions are viewer *commands*, not input events: a keyboard, mouse or
//! controller adapter maps its own events onto [`ManualViewerAction`] (the GUI's
//! `ViewerCommand` already does this for generic viewer input) and the state
//! machine here never knows which device produced them. Nothing is persisted
//! and nothing is written beside the document.

use super::ManualDocumentId;

/// Zoom steps used by `ZoomIn` / `ZoomOut`, in percent.
pub const ZOOM_STEPS: [u16; 9] = [25, 50, 75, 100, 125, 150, 200, 300, 400];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualViewerAction {
    NextPage,
    PreviousPage,
    FirstPage,
    LastPage,
    ZoomIn,
    ZoomOut,
    FitWidth,
    FitPage,
    ToggleFullscreen,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualZoom {
    FitPage,
    FitWidth,
    /// An explicit zoom, always one of [`ZOOM_STEPS`].
    Percent(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualViewerState {
    document: Option<ManualDocumentId>,
    page_count: usize,
    current_page: usize,
    zoom: ManualZoom,
    fullscreen: bool,
}

impl Default for ManualViewerState {
    fn default() -> Self {
        Self::closed()
    }
}

impl ManualViewerState {
    #[must_use]
    pub const fn closed() -> Self {
        Self {
            document: None,
            page_count: 0,
            current_page: 0,
            zoom: ManualZoom::FitPage,
            fullscreen: false,
        }
    }

    /// Show a document. A different document starts at its first page with
    /// fit-to-page zoom; reopening the *same* document (same path, size,
    /// modification time and inode) keeps the reading position, clamped to the
    /// page count. Fullscreen is a viewing-mode choice and carries over.
    pub fn open(&mut self, document: ManualDocumentId, page_count: usize) {
        if self.document.as_ref() == Some(&document) {
            self.page_count = page_count;
            self.current_page = self.current_page.min(page_count.saturating_sub(1));
            return;
        }
        self.document = Some(document);
        self.page_count = page_count;
        self.current_page = 0;
        self.zoom = ManualZoom::FitPage;
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.document.is_some()
    }

    #[must_use]
    pub fn document(&self) -> Option<&ManualDocumentId> {
        self.document.as_ref()
    }

    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.page_count
    }

    /// Zero-based index of the page being shown.
    #[must_use]
    pub const fn current_page(&self) -> usize {
        self.current_page
    }

    /// One-based page number for display, or 0 when there is nothing to show.
    #[must_use]
    pub const fn page_number(&self) -> usize {
        if self.page_count == 0 {
            0
        } else {
            self.current_page + 1
        }
    }

    #[must_use]
    pub const fn zoom(&self) -> ManualZoom {
        self.zoom
    }

    #[must_use]
    pub const fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    #[must_use]
    pub const fn can_go_next(&self) -> bool {
        self.document.is_some() && self.current_page + 1 < self.page_count
    }

    #[must_use]
    pub const fn can_go_previous(&self) -> bool {
        self.document.is_some() && self.current_page > 0
    }

    /// Jump to a specific page (for example a saved reading position).
    /// Out-of-range requests are ignored. Returns whether the page changed.
    pub fn go_to_page(&mut self, index: usize) -> bool {
        if self.document.is_none() || index >= self.page_count || index == self.current_page {
            return false;
        }
        self.current_page = index;
        true
    }

    /// Apply one action. Returns `true` when the state changed. Page movement
    /// stops at the first and last page; actions on a closed viewer do nothing.
    pub fn apply(&mut self, action: ManualViewerAction) -> bool {
        if self.document.is_none() {
            return false;
        }
        let before = self.clone();
        match action {
            ManualViewerAction::NextPage => {
                if self.can_go_next() {
                    self.current_page += 1;
                }
            }
            ManualViewerAction::PreviousPage => {
                if self.can_go_previous() {
                    self.current_page -= 1;
                }
            }
            ManualViewerAction::FirstPage => self.current_page = 0,
            ManualViewerAction::LastPage => {
                self.current_page = self.page_count.saturating_sub(1);
            }
            ManualViewerAction::ZoomIn => self.zoom = zoom_step(self.zoom, true),
            ManualViewerAction::ZoomOut => self.zoom = zoom_step(self.zoom, false),
            ManualViewerAction::FitWidth => self.zoom = ManualZoom::FitWidth,
            ManualViewerAction::FitPage => self.zoom = ManualZoom::FitPage,
            ManualViewerAction::ToggleFullscreen => self.fullscreen = !self.fullscreen,
            ManualViewerAction::Close => *self = Self::closed(),
        }
        *self != before
    }
}

/// Fit modes count as 100% for stepping, so the first `ZoomIn` from a fitted
/// page is 125% and the first `ZoomOut` is 75%.
fn zoom_step(zoom: ManualZoom, up: bool) -> ManualZoom {
    let current = match zoom {
        ManualZoom::Percent(percent) => percent,
        ManualZoom::FitPage | ManualZoom::FitWidth => 100,
    };
    let next = if up {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > current)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < current)
            .unwrap_or(ZOOM_STEPS[0])
    };
    ManualZoom::Percent(next)
}
