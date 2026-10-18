//! Navigation from saved identity/health evidence. Never owns a repair or audit.
use super::super::{
    game_verification_guidance::{Action as VerificationAction, Guidance},
    guidance::{self, GuidanceCategory, GuidanceTip, MascotState},
    identity_review::{Review, ReviewState},
    library::Game,
    routes::{Route, Section},
};
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Verification(VerificationAction),
    FindMissingFile,
    RepairReview,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Verification(VerificationAction::SelectDat | VerificationAction::Validate) => {
                "Choose DAT"
            }
            Self::Verification(VerificationAction::Audit) => "Verify",
            Self::Verification(VerificationAction::Recheck) => "Re-check",
            Self::Verification(VerificationAction::Review) => "Review Identity",
            Self::FindMissingFile => "Find Missing File",
            Self::RepairReview => "Repair Review",
        }
    }

    fn route(self, game: &Game) -> Route {
        match self {
            Self::Verification(action) => action.route(game),
            Self::FindMissingFile => Route::LibraryFiles,
            Self::RepairReview => Route::Section(Section::Build),
        }
    }
}

struct Advice {
    message: String,
    action: Option<Action>,
}

fn advice(game: &Game, review: &Review, loaded: bool, verification: Option<&Guidance>) -> Advice {
    let (message, action) = if game.archive.last_verified_missing_at.is_some()
        || game.archive.last_known_health == "missing"
    {
        ("A saved library check recorded this file as missing. Its drive may be disconnected or the file may have moved; this does not establish damaged media. Open Find Missing File, then explicitly run the existing library files check and review possible moves. Nothing is relinked on opening.".into(), Some(Action::FindMissingFile))
    } else if matches!(
        game.archive.last_known_health.as_str(),
        "corrupt" | "damaged"
    ) {
        ("A saved media-health check reported damage. Repair Review may already contain another file's plan: check that its scope matches this file before Preview, or load the appropriate plan. A saved problem does not guarantee that a repair is available. Apply still requires explicit confirmation.".into(), Some(Action::RepairReview))
    } else if !loaded {
        ("I am reading the saved identity and audit evidence. Missing evidence is not proof of damaged media; opening this panel does not start repair or verification.".into(), None)
    } else if matches!(review.state, ReviewState::NoSystem) {
        ("The system has not been established. Use Choose system above and review its confirmation before selecting a compatible DAT. Missing system evidence does not show that the media is damaged.".into(), Some(Action::Verification(VerificationAction::Review)))
    } else if game.archive.last_known_health == "error" {
        ("A saved health check could not read this file. Check its source and permissions, then Re-check through the existing verification Preview. A read error alone does not prove damaged media.".into(), Some(Action::Verification(VerificationAction::Recheck)))
    } else if let Some(verification) = verification {
        (
            format!("{}. {}", verification.reason, verification.explanation),
            Some(Action::Verification(verification.action)),
        )
    } else {
        ("I am reading saved audit and catalogue status to find the next step. Opening this panel does not run a content check, and unavailable identity facts do not establish damaged media.".into(), None)
    };
    Advice { message, action }
}

pub(super) fn show(
    ui: &mut egui::Ui,
    game: &Game,
    review: &Review,
    loaded: bool,
    verification: Option<&Guidance>,
    current: &Route,
) -> Option<Action> {
    let advice = advice(game, review, loaded, verification);
    guidance::show_tip(
        ui,
        &GuidanceTip {
            key: "identity_contextual_next_step",
            category: if advice.action == Some(Action::RepairReview) {
                GuidanceCategory::Warning
            } else {
                GuidanceCategory::Explain
            },
            mascot: MascotState::Thinking,
            message: advice.message,
        },
    );
    // Review Identity already contains the evidence and the system chooser.
    // Do not repeat its action as a link back to the page already open.
    advice
        .action
        .filter(|action| action.route(game) != *current)
        .filter(|action| ui.button(action.label()).clicked())
}

impl super::super::App {
    pub(super) fn run_identity_context_action(&mut self, id: i64, action: Action) {
        let Some(game) = self.library.game(id).cloned() else {
            return;
        };
        match action {
            Action::RepairReview => {
                self.organisation.view = super::super::organisation::OrganisationView::RepairReview;
            }
            Action::FindMissingFile => {
                let tab = super::super::library_reconciliation::Tab::Missing;
                self.reconcile.tab = Some(tab);
                self.reconcile.filter.insert(
                    tab,
                    game.archive.absolute_path.to_string_lossy().into_owned(),
                );
                self.reconcile.page.remove(&tab);
                self.reconcile.filter_cache.remove(&tab);
            }
            Action::Verification(VerificationAction::Validate) => {
                self.open_identification_data(id, &game.platform);
                return;
            }
            _ => {}
        }
        self.go(action.route(&game));
    }
}

#[cfg(test)]
mod tests;
