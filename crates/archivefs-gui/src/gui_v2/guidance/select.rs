//! Deterministic guidance selection.
//!
//! A pure function of `(scripts, page, facts)`. No clock, no randomness, no
//! navigation history, no frame count, no I/O: the same input always selects the
//! same script, so a test (or a user filing a bug) can reproduce any answer.
//!
//! Ranking, strongest first:
//!
//! 1. **priority band** (100, 90, 80, 70, 60, 50, 40, 10). Band comes first so a
//!    narrow cosmetic message can never hide a refusal reason;
//! 2. **scope** (operation, game, source, collection): within a band, the message
//!    about what the person is doing wins;
//! 3. **page-specific** over any-page;
//! 4. **category** (why-blocked, warning, empty state, explain, tip, success);
//! 5. the **script ID**, so ties are never decided by table order.

#![allow(dead_code)]

use std::cmp::Reverse;

use super::model::{
    FactKind, GuidanceAction, GuidanceCategory, GuidanceFact, GuidanceLevel, GuidancePage,
    GuidanceScope, GuidanceTopic, MascotState, Params,
};
use super::script::{GuidanceScript, render};

/// A script that is eligible for this page and these facts, with its values.
#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub(crate) script: &'static GuidanceScript,
    pub(crate) params: Params,
    pub(crate) action: Option<GuidanceAction>,
    /// The facts that made it eligible.
    pub(crate) matched: Vec<GuidanceFact>,
}

/// Why the winner won.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Provenance {
    pub(crate) priority: u8,
    pub(crate) scope: GuidanceScope,
    pub(crate) page_specific: bool,
    /// The fact kinds that made the script eligible.
    pub(crate) matched_kinds: Vec<FactKind>,
    /// Other eligible scripts that lost, strongest first.
    pub(crate) outranked: Vec<&'static str>,
    /// Other eligible blockers/warnings (band 80 and above) that were not shown.
    /// A page can say "N other requirements" and open the existing details rather
    /// than alternating advice.
    pub(crate) other_blockers: Vec<&'static str>,
}

#[derive(Clone, Debug)]
pub(crate) struct GuidanceSelection {
    pub(crate) script: &'static GuidanceScript,
    pub(crate) params: Params,
    pub(crate) action: Option<GuidanceAction>,
    pub(crate) provenance: Provenance,
    matched: Vec<GuidanceFact>,
}

/// The selected guidance at a requested level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GuidanceItem {
    pub(crate) id: &'static str,
    pub(crate) category: GuidanceCategory,
    pub(crate) mascot: MascotState,
    pub(crate) requested_level: GuidanceLevel,
    /// The level the message actually is. If the script does not author the
    /// requested level this is `Quick`; text is never made up.
    pub(crate) level: GuidanceLevel,
    pub(crate) message: String,
    /// A typed offer only: the engine never executes or navigates.
    pub(crate) action: Option<GuidanceAction>,
    pub(crate) topics: &'static [GuidanceTopic],
    pub(crate) available_levels: Vec<GuidanceLevel>,
}

impl GuidanceSelection {
    /// Every level this script authors.
    pub(crate) fn available_levels(&self) -> Vec<GuidanceLevel> {
        let s = self.script;
        let mut levels = Vec::new();
        if s.minimal.is_some() {
            levels.push(GuidanceLevel::Minimal);
        }
        levels.push(GuidanceLevel::Quick);
        if s.explain.is_some() {
            levels.push(GuidanceLevel::Explain);
        }
        if s.technical.is_some() {
            levels.push(GuidanceLevel::Technical);
        }
        levels
    }

    /// The authored text for `level`, rendered with this selection's values.
    pub(crate) fn item(&self, level: GuidanceLevel) -> GuidanceItem {
        let s = self.script;
        let (resolved, template) = match level {
            GuidanceLevel::Minimal => match s.minimal {
                Some(text) => (GuidanceLevel::Minimal, text),
                None => (GuidanceLevel::Quick, s.quick),
            },
            GuidanceLevel::Quick => (GuidanceLevel::Quick, s.quick),
            GuidanceLevel::Explain => match s.explain {
                Some(text) => (GuidanceLevel::Explain, text),
                None => (GuidanceLevel::Quick, s.quick),
            },
            GuidanceLevel::Technical => match s.technical {
                Some(text) => (GuidanceLevel::Technical, text),
                None => (GuidanceLevel::Quick, s.quick),
            },
        };
        GuidanceItem {
            id: s.id,
            category: s.category,
            mascot: s.mascot,
            requested_level: level,
            level: resolved,
            // Eligibility already proved every text renders with these values.
            message: render(template, &self.params).unwrap_or_default(),
            action: self.action,
            topics: s.topics,
            available_levels: self.available_levels(),
        }
    }

    /// Identifies the semantic event this selection represents, for repeat
    /// suppression: the script plus what is materially different about the facts.
    pub(crate) fn semantic_key(&self) -> String {
        let mut parts: Vec<String> = self
            .matched
            .iter()
            .map(GuidanceFact::semantic_key)
            .collect();
        parts.sort();
        format!("{}|{}", self.script.id, parts.join("+"))
    }
}

/// Whether `script` could be shown here, and with which values.
fn candidate(
    script: &'static GuidanceScript,
    page: GuidancePage,
    facts: &[GuidanceFact],
) -> Option<Candidate> {
    if !script.pages.is_empty() && !script.pages.contains(&page) {
        return None;
    }
    let has = |kind: FactKind| facts.iter().any(|fact| fact.kind() == kind);
    if !script.requires.iter().all(|kind| has(*kind)) || script.excludes.iter().any(|k| has(*k)) {
        return None;
    }
    // Values come only from the facts the script requires, first fact wins.
    let matched: Vec<GuidanceFact> = script
        .requires
        .iter()
        .filter_map(|kind| facts.iter().find(|fact| fact.kind() == *kind).cloned())
        .collect();
    let mut params: Params = Vec::new();
    for fact in &matched {
        for (name, value) in fact.params() {
            if !params.iter().any(|(existing, _)| *existing == name) {
                params.push((name, value));
            }
        }
    }
    // Every authored text must render. A script that cannot fill one of its
    // placeholders is not eligible: nothing is invented to cover the gap.
    if !script.texts().all(|text| render(text, &params).is_some()) {
        return None;
    }
    let mut action = script.action;
    if let Some((_, alternate)) = script.action_alternates.iter().find(|(kind, _)| has(*kind)) {
        action = *alternate;
    }
    // Never offer to navigate to the page the person is already on.
    if action.is_some_and(|action| action.lands_on() == Some(page)) {
        action = None;
    }
    Some(Candidate {
        script,
        params,
        action,
        matched,
    })
}

/// Every eligible script for `page` and `facts`, strongest first.
pub(crate) fn eligible(
    scripts: &'static [GuidanceScript],
    page: GuidancePage,
    facts: &[GuidanceFact],
) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = scripts
        .iter()
        .filter_map(|script| candidate(script, page, facts))
        .collect();
    found.sort_by_key(|candidate| {
        let s = candidate.script;
        (
            Reverse(s.priority),
            Reverse(s.scope),
            Reverse(!s.pages.is_empty()),
            Reverse(s.category.precedence()),
            s.id,
        )
    });
    found
}

/// The one message to show, or `None`: there is no applicable script, which is a
/// valid answer (a hub, or a page with nothing to say, shows nothing).
pub(crate) fn select_in(
    scripts: &'static [GuidanceScript],
    page: GuidancePage,
    facts: &[GuidanceFact],
) -> Option<GuidanceSelection> {
    let mut found = eligible(scripts, page, facts).into_iter();
    let winner = found.next()?;
    let rest: Vec<Candidate> = found.collect();
    let provenance = Provenance {
        priority: winner.script.priority,
        scope: winner.script.scope,
        page_specific: !winner.script.pages.is_empty(),
        matched_kinds: winner.matched.iter().map(GuidanceFact::kind).collect(),
        outranked: rest.iter().map(|c| c.script.id).collect(),
        other_blockers: rest
            .iter()
            .filter(|c| {
                c.script.priority >= 80
                    && matches!(
                        c.script.category,
                        GuidanceCategory::WhyBlocked | GuidanceCategory::Warning
                    )
            })
            .map(|c| c.script.id)
            .collect(),
    };
    Some(GuidanceSelection {
        script: winner.script,
        params: winner.params,
        action: winner.action,
        provenance,
        matched: winner.matched,
    })
}

/// Selects from the production catalogue.
pub(crate) fn select(page: GuidancePage, facts: &[GuidanceFact]) -> Option<GuidanceSelection> {
    select_in(super::catalogue::CATALOGUE, page, facts)
}
