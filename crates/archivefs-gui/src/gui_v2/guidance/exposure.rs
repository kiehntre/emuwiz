//! Repeat suppression: when an already-selected message is shown in full, shown
//! compactly, or held back (design section 7).
//!
//! Selection and exposure are deliberately separate. [`super::select`] always
//! chooses the same script for the same facts; this module decides only how
//! much of it to present given what the person has already seen *this session*.
//! It is a pure, in-memory state with an injected clock: no persistence, no
//! navigation trail, no game names, no telemetry.
//!
//! * A **blocker** can be collapsed but is never suppressed: the reason stays
//!   discoverable beside its action until it is resolved.
//! * **First-use help** appears once per session and scope, then as a compact link,
//!   and is rate-limited (one unsolicited tip per topic per session, and a minimum
//!   cooldown between tips). The stricter rule wins.
//! * A **success** is shown until acknowledged for that operation, then suppressed
//!   for the rest of the session. A different operation is a new event.
//! * A non-blocking **warning** can be collapsed for the same evidence; a changed
//!   reason shows it again.
//!
//! The suppression key is the selection's semantic key, so a changed title, reason
//! or operation is a new event while a changed count is not.

#![allow(dead_code)]

use std::collections::HashSet;

use super::model::GuidanceTopic;
use super::script::RepeatPolicy;
use super::select::GuidanceSelection;

/// Minimum time between unsolicited optional tips.
pub(crate) const OPTIONAL_TIP_COOLDOWN_SECS: u64 = 30 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exposure {
    /// Quick text and its action, with the explanation available.
    Expanded,
    /// A compact "Why?" affordance only.
    Compact,
    /// Nothing is shown.
    Suppressed,
}

/// Something that happened to a message, reported by the page after layout. A
/// message counts as `Seen` only once it was actually visible, never because its
/// widget was constructed below the fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExposureEvent {
    Seen,
    Acknowledged,
    Collapsed,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ExposureState {
    seen: HashSet<String>,
    acknowledged: HashSet<String>,
    collapsed: HashSet<String>,
    tip_topics_shown: HashSet<GuidanceTopic>,
    last_optional_tip_at: Option<u64>,
}

impl ExposureState {
    /// How much of `selection` to present at `now_secs`.
    pub(crate) fn decide(&self, selection: &GuidanceSelection, now_secs: u64) -> Exposure {
        let key = selection.semantic_key();
        match selection.script.repeat {
            RepeatPolicy::AlwaysShown => Exposure::Expanded,
            RepeatPolicy::Blocker | RepeatPolicy::CollapsibleWarning => {
                if self.collapsed.contains(&key) {
                    Exposure::Compact
                } else {
                    Exposure::Expanded
                }
            }
            RepeatPolicy::PerOperation => {
                if self.acknowledged.contains(&key) {
                    Exposure::Suppressed
                } else {
                    Exposure::Expanded
                }
            }
            RepeatPolicy::FirstUse => {
                if self.seen.contains(&key) {
                    return Exposure::Compact;
                }
                let topic_used = selection
                    .script
                    .topics
                    .iter()
                    .any(|topic| self.tip_topics_shown.contains(topic));
                let cooling = self
                    .last_optional_tip_at
                    .is_some_and(|at| now_secs.saturating_sub(at) < OPTIONAL_TIP_COOLDOWN_SECS);
                if topic_used || cooling {
                    Exposure::Suppressed
                } else {
                    Exposure::Expanded
                }
            }
        }
    }

    /// An explicit "Help me with this" always opens the current explanation,
    /// whatever has been seen or collapsed.
    pub(crate) fn requested(&self, _selection: &GuidanceSelection) -> Exposure {
        Exposure::Expanded
    }

    pub(crate) fn record(
        &mut self,
        selection: &GuidanceSelection,
        event: ExposureEvent,
        now_secs: u64,
    ) {
        let key = selection.semantic_key();
        match event {
            ExposureEvent::Seen => {
                if selection.script.repeat == RepeatPolicy::FirstUse && !self.seen.contains(&key) {
                    self.last_optional_tip_at = Some(now_secs);
                    self.tip_topics_shown
                        .extend(selection.script.topics.iter().copied());
                }
                self.seen.insert(key);
            }
            ExposureEvent::Acknowledged => {
                self.acknowledged.insert(key);
            }
            ExposureEvent::Collapsed => {
                self.collapsed.insert(key);
            }
        }
    }

    /// "Reset guidance": forgets everything this session has acknowledged.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}
