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

    /// Game Details' one identity wording for `game` (`identity_wording`).
    pub(super) fn detail_identity_wording(&self, game: &Game) -> Option<IdentityWording> {
        let identity = self.detail_identity(game)?;
        let review = self.identity_review_for(game.archive.id);
        Some(identity_wording(Some(&identity), game, review.as_ref()))
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
        GuessBasis::FileEvidence => "From the file's own evidence. Not verified.",
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
    // Stacked, each part wrapping to the column width: a single wrapped row
    // let the long question and the small notes overlap in the narrow
    // right-hand panel.
    ui.vertical(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("{LIKELY_MATCH_QUESTION} {}", game.title)).strong(),
            )
            .wrap(),
        );
        ui.add(egui::Label::new(egui::RichText::new(basis_text(guess.basis)).small()).wrap());
        if !guess.alternatives.is_empty() {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("Or: {}", guess.alternatives.join(" · "))).small(),
                )
                .wrap(),
            );
        }
        ui.horizontal_wrapped(|ui| {
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
    });
    route
}

/// How sure Game Details is which game this file is: identification
/// confidence only. A missing BIOS, emulator or catalogue, and whether Play
/// can run, never change it; the readiness card states those separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Confidence {
    /// Current, trusted evidence verifies this exact game.
    Verified,
    /// A saved catalogue match is being re-read; no answer yet.
    Checking,
    /// One likely identity, not verified.
    Probable,
    /// More than one candidate, or the evidence disagrees.
    Ambiguous,
    /// Nothing beyond the file name establishes which game this is.
    Unknown,
}

impl Confidence {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Verified => "Verified identity",
            Self::Probable => "Probable identity (not verified)",
            Self::Ambiguous => "Ambiguous identity",
            Self::Unknown => "Unknown identity",
            Self::Checking => "Identity still being checked",
        }
    }
}

/// Game Details' identity wording. Every identity statement on the page (the
/// Identity line, the readiness card's identity chip, Advanced details and
/// the Game Doctor) reads this one answer, so no two places can disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IdentityWording {
    pub confidence: Confidence,
    /// The short answer each "Identity:" line shows.
    pub label: &'static str,
    /// What the answer rests on, or which identification step is still
    /// missing. Never a launch requirement or an action's state.
    pub note: Option<&'static str>,
    /// The answer is Ambiguous only because a saved catalogue comparison
    /// found several candidates.
    pub from_saved_comparison: bool,
}

impl IdentityWording {
    pub(super) fn is_verified(&self) -> bool {
        self.confidence == Confidence::Verified
    }
}

/// The one mapping from Game Details' identity answer (`decide`) and the
/// canonical review to wording. Pure; reads no evidence and decides nothing:
/// a hint stays a hint, and only a verified review reads "Verified".
pub(super) fn identity_wording(
    identity: Option<&DetailIdentity>,
    game: &Game,
    review: Option<&Review>,
) -> IdentityWording {
    use super::identity_review::{NoMatchReason, ReviewState};
    let wording = |confidence, label, note: &'static str| IdentityWording {
        confidence,
        label,
        note: Some(note),
        from_saved_comparison: false,
    };
    let basis = likely_match(game, review).basis;
    let guessed = match basis {
        GuessBasis::PossibleCatalogueMatch => Confidence::Probable,
        GuessBasis::FileEvidence => Confidence::Probable,
        GuessBasis::FileName => Confidence::Unknown,
    };
    // Not verified: what the guess rests on leads, the missing step follows.
    let hint = |note| {
        let label = match basis {
            GuessBasis::PossibleCatalogueMatch => "Possible match · not verified",
            GuessBasis::FileEvidence => "Likely · not verified",
            GuessBasis::FileName => "Name hint only · not verified",
        };
        wording(guessed, label, note)
    };
    let mut answer = match identity {
        None => IdentityWording {
            confidence: Confidence::Unknown,
            label: "Not known yet",
            note: None,
            from_saved_comparison: false,
        },
        Some(DetailIdentity::Checking) => wording(
            Confidence::Checking,
            "Checking verification",
            "A saved catalogue match is being re-read. It is not verified until that finishes.",
        ),
        Some(DetailIdentity::NotConfirmed) => wording(
            Confidence::Probable,
            "Verification not yet confirmed",
            "A saved catalogue match could not be confirmed as current. Not verified.",
        ),
        Some(DetailIdentity::RecheckNeeded) => wording(
            match review.map(|review| &review.state) {
                Some(ReviewState::Ambiguous { .. } | ReviewState::Conflict { .. }) => {
                    Confidence::Ambiguous
                }
                _ => guessed,
            },
            "Re-check needed",
            "The earlier catalogue match no longer holds for this file. Not verified.",
        ),
        Some(DetailIdentity::Known(known)) => match &known.state {
            ReviewState::Verified(facts) if facts.by_file_evidence => wording(
                Confidence::Verified,
                "Verified",
                "From the file's own contents, not from a catalogue.",
            ),
            ReviewState::Verified(_) => wording(
                Confidence::Verified,
                "Verified",
                "Exact match in trusted identification data.",
            ),
            ReviewState::ReferenceMatched => wording(
                Confidence::Probable,
                "Matched to reference data",
                "A set audit against arcade reference data. Not an exact verification.",
            ),
            ReviewState::Ambiguous { .. } => wording(
                Confidence::Ambiguous,
                "Several possible matches",
                "More than one trusted entry fits this file. EmuWiz will not guess.",
            ),
            ReviewState::Conflict { .. } => wording(
                Confidence::Ambiguous,
                "Evidence disagrees",
                "Trusted evidence points to different games. EmuWiz will not pick one.",
            ),
            ReviewState::NoSystem => wording(
                Confidence::Unknown,
                "Unknown · system not known",
                "No trusted system is on record for this file, so it cannot be looked up.",
            ),
            ReviewState::NotCompared => hint("Not checked against a catalogue yet."),
            ReviewState::NoMatch(NoMatchReason::NotInData) => hint(
                "Not in the installed catalogue for this system. That does not mean the file is bad.",
            ),
            ReviewState::NoMatch(NoMatchReason::WeakEvidenceOnly) => {
                hint("Only a weak clue matched a catalogue. That is not verification.")
            }
            ReviewState::NoMatch(NoMatchReason::NoUsableEvidence) => {
                hint("The file could not be read well enough to compare it.")
            }
            ReviewState::NoData {
                reference_source_exists: true,
            } => hint("Identification data for this system is not installed."),
            ReviewState::NoData {
                reference_source_exists: false,
            } => hint("No trusted identification data exists for this system."),
        },
    };
    // A saved catalogue comparison that found several candidates is
    // ambiguity, not a probable identity, whatever the review settled on.
    if matches!(
        answer.confidence,
        Confidence::Probable | Confidence::Unknown
    ) && game.dat_state == Some(archivefs_core::LibraryDatState::Ambiguous)
    {
        answer.confidence = Confidence::Ambiguous;
        answer.from_saved_comparison = true;
        if answer.label.ends_with("· not verified") {
            answer.label = "Several possible matches · not verified";
        }
    }
    answer
}
