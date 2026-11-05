//! Game Details' one identity answer, shared by the Identity line and the
//! launch readiness card so they can never disagree.
//!
//! The DAT part reuses the panel's own per-game current-proof decision
//! (`game_verification_guidance::dat_verified_now`, #279). A Library flag or a
//! saved exact-match row never establishes current verification by itself:
//! until this game's panel has re-read its saved evidence, a DAT-backed
//! identity reads "Checking verification"; an answer that neither confirms
//! nor invalidates it reads "Verification not yet confirmed"; only strict
//! invalidation reads "Re-check needed". Identity is never launch
//! readiness; nothing here enables Play.
use super::game_verification_guidance::{Guidance, dat_verified_now};
use super::identity_review::{Review, review_for};
use super::launch_readiness_summary::IdentitySummary;
use super::library::{Game, IdentityContext};
use super::routes::Route;
use eframe::egui;

/// The panel's only answer that can invalidate earlier proof, and the one
/// explanation under it that is a temporary absence rather than evidence.
/// Pinned against the panel's real `explain()` in this module's tests.
const STALE_OR_CHANGED: &str = "File evidence stale or changed";
const FILE_UNAVAILABLE: &str =
    "The game file is unavailable. Reconnect its location, then re-check it against a DAT.";

/// What this game's finished panel answer establishes about its DAT proof.
/// The one place panel answers are classified for identity purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProofCheck {
    /// Current proof confirmed by this game's own re-read (#297's rule).
    Confirmed,
    /// Strict evidence shows the earlier proof no longer holds: a saved row
    /// is marked stale, the audited file object changed, or no saved row
    /// re-reads as current under the core's freshness rules.
    Invalidated,
    /// Neither: setup or coverage gaps, unreadable evidence, a cancelled or
    /// failed worker, an unavailable file, or any answer not listed here.
    /// Never Verified, and never grounds to drop saved evidence.
    NotEstablished,
}

pub(super) fn classify(game: &Game, answer: &Guidance) -> ProofCheck {
    if dat_verified_now(game, Some(answer)) {
        ProofCheck::Confirmed
    } else if answer.reason == STALE_OR_CHANGED && answer.explanation != FILE_UNAVAILABLE {
        ProofCheck::Invalidated
    } else {
        ProofCheck::NotEstablished
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DetailIdentity {
    /// A DAT match is on record, but this game's panel has not answered yet.
    Checking,
    /// The panel answered without establishing current proof either way.
    NotConfirmed,
    /// This game's fresh check invalidated its earlier proof.
    RecheckNeeded,
    /// The settled answer: the canonical review when its proof holds now,
    /// otherwise what is known without the unproven DAT match.
    Known(Review),
}

impl DetailIdentity {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Checking => "Checking verification",
            Self::NotConfirmed => "Verification not yet confirmed",
            Self::RecheckNeeded => "Re-check needed",
            Self::Known(review) => review.list_label(),
        }
    }

    pub(super) fn is_verified(&self) -> bool {
        matches!(self, Self::Known(review) if review.is_verified())
    }

    /// The readiness card's identity line for an unconfirmed DAT identity.
    pub(super) fn card_identity(&self) -> Option<IdentitySummary> {
        match self {
            Self::Checking => Some(IdentitySummary::VerificationPending),
            Self::NotConfirmed => Some(IdentitySummary::VerificationNotConfirmed),
            Self::RecheckNeeded => Some(IdentitySummary::RecheckNeeded),
            Self::Known(_) => None,
        }
    }
}

/// `review` is the canonical review (`App::identity_review_for`); `panel` is
/// this game's own finished panel answer, if any (`State::result_for`).
pub(super) fn decide(
    review: Review,
    game: &Game,
    context: &IdentityContext,
    panel: Option<&Guidance>,
) -> DetailIdentity {
    // The file's own evidence verifies it without any DAT row.
    let without_dat = without_dat_match(game);
    let by_file = review_for(&without_dat, context, None);
    if by_file.is_verified() {
        return DetailIdentity::Known(review);
    }
    if game.dat_proof_invalidated {
        return DetailIdentity::RecheckNeeded;
    }
    if !review.is_verified() {
        return DetailIdentity::Known(review);
    }
    match panel.map(|answer| classify(game, answer)) {
        None => DetailIdentity::Checking,
        Some(ProofCheck::Confirmed) => DetailIdentity::Known(review),
        Some(ProofCheck::Invalidated) => DetailIdentity::RecheckNeeded,
        Some(ProofCheck::NotEstablished) => DetailIdentity::NotConfirmed,
    }
}

/// True only when this game's own finished answer invalidates the Library's
/// earlier current match. Anything not established keeps saved evidence.
pub(super) fn disproves_library_match(game: &Game, panel: Option<&Guidance>) -> bool {
    game.dat_exact.is_some()
        && panel.is_some_and(|answer| classify(game, answer) == ProofCheck::Invalidated)
}

fn without_dat_match(game: &Game) -> Game {
    let mut game = game.clone();
    game.dat_exact = None;
    game.dat_exact_hashes = None;
    game.dat_state = None;
    game
}

impl super::App {
    /// Game Details' identity for `game`; see the module documentation.
    /// Always reads the current in-memory entry, never the page's snapshot,
    /// so an invalidation earlier in the same frame is already reflected.
    pub(super) fn detail_identity(&self, game: &Game) -> Option<DetailIdentity> {
        let id = game.archive.id;
        let game = self.library.game(id)?;
        let review = self.identity_review_for(id)?;
        Some(decide(
            review,
            game,
            &self.library.identity_context,
            self.review.verification_guidance.result_for(id),
        ))
    }

    /// Owner-approved: once this game's own fresh check disproves its
    /// earlier current match, drop that match from the in-memory Library so
    /// its badge, counts and Game Details agree. The database and every other
    /// game are untouched; the next Library load re-derives the flag.
    pub(super) fn forget_disproved_dat_match(&mut self, id: i64) {
        let Some(game) = self.library.game(id) else {
            return;
        };
        if !disproves_library_match(game, self.review.verification_guidance.result_for(id)) {
            return;
        }
        let library = std::sync::Arc::make_mut(&mut self.library);
        if let Some(game) = library.games.iter_mut().find(|game| game.archive.id == id) {
            game.dat_exact = None;
            game.dat_exact_hashes = None;
            game.saved_hashes = None;
            game.dat_proof_invalidated = true;
        }
    }
}

#[cfg(test)]
mod tests;

/// How Blue arrived at its best guess. None of these is verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GuessBasis {
    /// A saved catalogue comparison suggested it, without current proof.
    PossibleCatalogueMatch,
    /// The file's own evidence (header, serial or launch identity).
    FileEvidence,
    /// Only the file name and folder.
    FileName,
}

/// Blue's most likely identity for a game that is not verified, and any
/// alternatives the saved evidence names. Read-only; never saved as proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LikelyMatch {
    pub basis: GuessBasis,
    pub alternatives: Vec<String>,
}

pub(super) fn likely_match(
    game: &Game,
    review: Option<&super::identity_review::Review>,
) -> LikelyMatch {
    use archivefs_core::LibraryDatState;
    let basis = if matches!(
        game.dat_state,
        Some(LibraryDatState::Verified | LibraryDatState::Probable)
    ) {
        GuessBasis::PossibleCatalogueMatch
    } else if game.identified {
        GuessBasis::FileEvidence
    } else {
        GuessBasis::FileName
    };
    let alternatives = match review.map(|review| &review.state) {
        Some(super::identity_review::ReviewState::Ambiguous { candidates }) => candidates
            .iter()
            .filter(|name| **name != game.title)
            .take(3)
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    LikelyMatch {
        basis,
        alternatives,
    }
}

pub(super) fn basis_text(basis: GuessBasis) -> &'static str {
    match basis {
        GuessBasis::PossibleCatalogueMatch => {
            "Possible match from a saved catalogue comparison. Not verified."
        }
        GuessBasis::FileEvidence => {
            "From the file's own evidence. Not checked against a catalogue."
        }
        GuessBasis::FileName => "From the file name and folder only. Not verified.",
    }
}

pub(super) const LIKELY_MATCH_QUESTION: &str = "Does this look like your game?";
pub(super) const CHOOSE_ANOTHER_MATCH: &str = "Choose another match";
pub(super) const CHECK_THIS_GAME: &str = "Check this game";
/// Saying yes is only the person's judgement; nothing is saved as proof.
pub(super) const LOOKS_RIGHT_NOTE: &str = "Saying it looks right does not verify the file or change launch checks. Play above runs its own launch check; Check this game compares the file with a catalogue.";

/// The "Does this look like your game?" card: Blue's guess, how it got
/// there, alternatives, and the two working routes. Returns the route chosen.
pub(super) fn show_likely_match(
    ui: &mut egui::Ui,
    game: &Game,
    guess: &LikelyMatch,
) -> Option<Route> {
    let id = game.archive.id;
    let mut route = None;
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("{LIKELY_MATCH_QUESTION} {}", game.title));
        ui.small(basis_text(guess.basis));
        if !guess.alternatives.is_empty() {
            ui.small(format!("Or: {}", guess.alternatives.join(" · ")));
        }
        if ui
            .small_button(CHOOSE_ANOTHER_MATCH)
            .on_hover_text(LOOKS_RIGHT_NOTE)
            .clicked()
        {
            route = Some(Route::ReviewIdentity(id));
        }
        if ui
            .small_button(CHECK_THIS_GAME)
            .on_hover_text(LOOKS_RIGHT_NOTE)
            .clicked()
        {
            route = Some(Route::GameCheck(id));
        }
    });
    route
}
