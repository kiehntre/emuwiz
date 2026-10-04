//! Carrying a Problems & Repair finding's next step to its destination.
//!
//! A finding's [`ProblemAction`] names a typed route and the thing to open on.
//! Applying it only selects something that already exists (a system filter, a
//! platform, a duplicate group, a panel) and navigates. It never starts a scan,
//! a verification or any other job, and a context that no longer matches the
//! library is ignored: the person still lands on the destination page.

use super::library::Library;
use super::problems::{ProblemAction, ProblemContext};
use super::{App, library::DuplicateReport};

/// What the Problems and Duplicates pages should position themselves on after
/// a finding's action. Cleared when it has been used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ProblemNav {
    /// Show only this exact-duplicate group (by SHA-256).
    pub(super) duplicate_focus: Option<String>,
    /// Scroll the Problems page to the Missing games review.
    pub(super) focus_missing_review: bool,
}

/// Whether a context still points at something real. Used before applying it.
pub(super) fn context_is_valid(
    context: &ProblemContext,
    library: &Library,
    duplicates: Option<&DuplicateReport>,
) -> bool {
    match context {
        ProblemContext::None | ProblemContext::MissingReview => true,
        ProblemContext::GamesSystem(name) | ProblemContext::CheckPlatform(name) => {
            library.platforms.contains_key(name)
        }
        ProblemContext::DuplicateGroup(sha) => {
            duplicates.is_some_and(|report| report.groups.iter().any(|g| &g.sha256 == sha))
        }
    }
}

impl App {
    /// Home's "needs attention" opens Problems & Repair on the actionable
    /// view, the one the count comes from, with no stale search.
    pub(super) fn open_problems_from_home(&mut self) {
        self.problem_filter = super::problems::ProblemFilter::Actionable;
        self.problem_query.clear();
        self.go(super::routes::Route::Section(
            super::routes::Section::Problems,
        ));
    }

    /// Opens a finding's destination with its context. Never enqueues work.
    pub(super) fn run_problem_action(&mut self, action: &ProblemAction) {
        let valid = context_is_valid(
            &action.context,
            &self.library,
            self.duplicate_report.as_ref(),
        );
        if valid {
            match &action.context {
                ProblemContext::None => {}
                ProblemContext::GamesSystem(name) => {
                    self.filter.select_platform(name.clone());
                }
                ProblemContext::CheckPlatform(name) => {
                    self.check_platform = Some(name.clone());
                    self.verification = None;
                }
                ProblemContext::DuplicateGroup(sha) => {
                    self.problem_nav.duplicate_focus = Some(sha.clone());
                }
                ProblemContext::MissingReview => {
                    self.problem_nav.focus_missing_review = true;
                }
            }
        }
        self.go(action.route.clone());
    }
}
