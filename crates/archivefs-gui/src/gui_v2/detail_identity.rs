//! Game Details' one identity answer, shared by the Identity line and the
//! launch readiness card so they can never disagree.
//!
//! The DAT part reuses the panel's own per-game current-proof decision
//! (`game_verification_guidance::dat_verified_now`, #279). A Library flag or a
//! saved exact-match row never establishes current verification by itself:
//! until this game's panel has re-read its saved evidence, a DAT-backed
//! identity reads "Checking verification". Identity is never launch
//! readiness; nothing here enables Play.
use super::game_verification_guidance::{Guidance, dat_verified_now};
use super::identity_review::{Review, review_for};
use super::library::{Game, IdentityContext};

/// The panel finished but could not conclude anything either way.
const INCONCLUSIVE: [&str; 2] = [
    "Verification evidence unavailable",
    "Catalogue status unavailable",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DetailIdentity {
    /// A DAT match is on record, but this game's panel has not answered yet.
    Checking,
    /// The panel answered, but could not read the evidence to confirm it.
    NotConfirmed,
    /// The settled answer: the canonical review when its proof holds now,
    /// otherwise what is known without the unproven DAT match.
    Known(Review),
}

impl DetailIdentity {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Checking => "Checking verification",
            Self::NotConfirmed => "Verification not yet confirmed",
            Self::Known(review) => review.list_label(),
        }
    }

    pub(super) fn is_verified(&self) -> bool {
        matches!(self, Self::Known(review) if review.is_verified())
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
    if !review.is_verified() {
        return DetailIdentity::Known(review);
    }
    // The file's own evidence verifies it without any DAT row.
    let without_dat = without_dat_match(game);
    let by_file = review_for(&without_dat, context, None);
    if by_file.is_verified() {
        return DetailIdentity::Known(review);
    }
    match panel {
        None => DetailIdentity::Checking,
        Some(answer) if dat_verified_now(game, Some(answer)) => DetailIdentity::Known(review),
        Some(answer) if INCONCLUSIVE.contains(&answer.reason) => DetailIdentity::NotConfirmed,
        Some(_) => DetailIdentity::Known(by_file),
    }
}

/// True when this game's panel has answered and its answer disproves the
/// Library's earlier current match: the proof no longer holds now.
/// Inconclusive answers (unreadable evidence) never count as disproof.
pub(super) fn disproves_library_match(game: &Game, panel: Option<&Guidance>) -> bool {
    game.dat_exact.is_some()
        && panel.is_some_and(|answer| {
            !dat_verified_now(game, Some(answer)) && !INCONCLUSIVE.contains(&answer.reason)
        })
}

fn without_dat_match(game: &Game) -> Game {
    let mut game = game.clone();
    game.dat_exact = None;
    game.dat_exact_hashes = None;
    game
}

impl super::App {
    /// Game Details' identity for `game`; see the module documentation.
    pub(super) fn detail_identity(&self, game: &Game) -> Option<DetailIdentity> {
        let id = game.archive.id;
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
        }
    }
}

#[cfg(test)]
mod tests;
