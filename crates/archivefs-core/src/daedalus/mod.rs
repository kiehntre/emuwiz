//! Daedalus: evidence-driven diagnosis of emulator launch problems.
//!
//! The launch subsystem already decides, before Play, whether a game *can*
//! be launched ([`crate::launch::readiness`]). Nothing explained what
//! happened *after* a launch went wrong: the exit status and the first part
//! of stderr were shown as text and the cause was guessed from rendered
//! error strings. Daedalus fills that gap without replacing any of it.
//!
//! # Contract
//!
//! - **Evidence first.** Every conclusion names the observations that
//!   support it ([`evidence::EvidenceId`]) and what limits confidence in it.
//!   A conclusion without supporting evidence cannot be constructed.
//! - **Unknown stays unknown.** A failure no rule can explain is reported as
//!   unexplained, with the evidence areas that were missing. No rule guesses
//!   a cause from a generic error message.
//! - **Text is data.** Emulator logs, stderr and anything a game or third
//!   party can influence are matched against fixed signatures and otherwise
//!   only displayed, sanitised and bounded. They are never interpreted as
//!   instructions, and on their own they never raise a conclusion above
//!   [`rules::Confidence::Possible`].
//! - **Deterministic.** The same evidence and rule set always produce the
//!   same report. Rules are versioned; a report records the digest of the
//!   rule set that produced it.
//! - **Read-only.** Nothing in this module writes a file, changes a setting
//!   or starts a process. A diagnosis is advice: it grants no permission and
//!   bypasses no existing safety check.
//!
//! # Layout
//!
//! - [`evidence`]: the evidence model and its coverage report.
//! - [`collect`]: assembles evidence from what a finished launch left behind.
//! - [`text`]: bounded, sanitised handling of untrusted text.
//! - [`signatures`]: fixed log signatures and the scanner that applies them.
//! - [`process`]: evidence from a finished process.
//! - [`retroarch`]: RetroArch log signatures and settings evidence.
//! - [`limitations`]: the built-in table of known compatibility limits.
//! - [`rules`]: the versioned rule set and the diagnostic engine.

pub mod collect;
pub mod evidence;
pub mod limitations;
pub mod process;
pub mod retroarch;
pub mod rules;
pub mod signatures;
pub mod text;

#[cfg(test)]
mod test_fixtures;

pub use evidence::{
    AreaCoverage, Evidence, EvidenceArea, EvidenceId, EvidenceSet, EvidenceState, Fact,
    FingerprintCheck, SourceFingerprint, Trust,
};
pub use rules::{
    Confidence, Diagnosis, DiagnosisCategory, DiagnosisReport, Verdict, diagnose, rule_set,
};
