//! Read-only presentation of DAT identity already persisted by the core.
//!
//! This module deliberately performs no matching, hashing, file I/O, network
//! access, or mutation. It only turns cached core summaries into UI text.
//!
//! # DAT verification vs structural identity
//!
//! This module presents only what a DAT catalogue says about a selected
//! item ("matched against a known catalogue"). It never renders and never
//! conflates itself with `selected_evidence_page`'s structural evidence
//! ("evidence from the game/disc itself" - headers, boot sectors, serials,
//! executable CRCs). `selected_game_panel.rs` keeps the two visually
//! separate; this module's own section caption states the distinction
//! again at the point of use so neither can be mistaken for the other.

use archivefs_core::dat::dependency::DependencyState;
use archivefs_core::dat::library_identity_summary::{
    DatProvenanceFreshness, DatSetDependencySummary, DatVerificationState,
    LibraryDatIdentitySummary,
};
use archivefs_core::dat::set::{BadMetadataReason, NeedsReviewReason, SetState};
use eframe::egui;

use crate::theme;
use crate::widgets;

/// The small, closed set of plain-language statuses a person sees for one
/// DAT check. Every variant maps to an exact backend state (see
/// [`present_summary`]/[`present_missing`]) - this is a display projection
/// only, never a second copy of the verification rules themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatIdentityStatus {
    /// Exactly one catalogue entry matched by a cryptographic hash, and the
    /// stored result still matches the item as currently known.
    Verified,
    /// The same cryptographic match, but the file or catalogue has changed
    /// since it was recorded.
    VerifiedNeedsRecheck,
    /// A CRC32(+size)-only match - likely correct, never proven.
    LikelyMatch,
    /// More than one candidate, or conflicting evidence: EmuWiz found more
    /// than one plausible answer and did not choose.
    NeedsReview,
    /// The checked catalogue has no matching entry for this item.
    NotFoundInDat,
    /// Only a filename matched, or no comparable hash was available at all.
    MoreEvidenceNeeded,
    /// No DAT result is stored for this item yet.
    NotCheckedYet,
}

impl DatIdentityStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Verified => "Verified",
            Self::VerifiedNeedsRecheck => "Verified, needs re-check",
            Self::LikelyMatch => "Likely match",
            Self::NeedsReview => "Needs review",
            Self::NotFoundInDat => "Not found in this DAT",
            Self::MoreEvidenceNeeded => "More evidence needed",
            Self::NotCheckedYet => "Not checked yet",
        }
    }

    /// Never [`widgets::StatusTone::Blocked`] (red/danger): every DAT state
    /// this panel can show is either a settled positive result or a
    /// harmless "needs a look" state, never a destructive failure.
    fn tone(self) -> widgets::StatusTone {
        match self {
            Self::Verified => widgets::StatusTone::Success,
            Self::VerifiedNeedsRecheck | Self::LikelyMatch | Self::NeedsReview => {
                widgets::StatusTone::Warning
            }
            Self::NotFoundInDat | Self::MoreEvidenceNeeded | Self::NotCheckedYet => {
                widgets::StatusTone::Info
            }
        }
    }
}

/// One DAT check, fully translated into what a person should see: a status,
/// a one-sentence explanation, the handful of default facts worth always
/// showing, and whether a "Verify Games" action makes sense from here.
/// Every field is `Option`/empty-safe; nothing here is ever fabricated when
/// the source summary does not carry it.
pub(crate) struct DatIdentityPresentation {
    pub(crate) status: DatIdentityStatus,
    pub(crate) explanation: String,
    /// "Verified as" - the catalogue's own name for the matched entry. Only
    /// set for a settled single/probable match; never a guess among
    /// candidates.
    pub(crate) verified_as: Option<String>,
    /// "Catalogue" - the DAT publisher family (No-Intro, Redump, ...).
    pub(crate) catalogue: Option<String>,
    /// "Match basis" - which evidence type confirmed it, in plain words.
    /// Never claims cryptographic certainty for a CRC-only match.
    pub(crate) match_basis: Option<String>,
    /// "Checked against" - the specific configured source this result came
    /// from.
    pub(crate) checked_against: Option<String>,
    /// The No-Intro representation only when the audited catalogue stated
    /// one.  It is explanatory provenance, never a confidence signal.
    pub(crate) variant: Option<String>,
    /// A short supporting sentence about whether this result is still
    /// current, when there is anything worth saying beyond the headline.
    pub(crate) freshness_note: Option<String>,
    /// Whether a "Verify Games" call to action belongs here. `false` only
    /// for a clean, current `Verified` result, where the honest answer is
    /// "no action needed".
    pub(crate) show_verify_action: bool,
}

/// A plain-language, one-sentence explanation of a DAT verification state:
/// what the catalogue evidence actually established, stated so a novice knows
/// how far to trust it. It never claims more certainty than the state itself
/// carries - a probable/filename-only/ambiguous match says so explicitly.
pub(crate) fn explain_verification(state: &DatVerificationState) -> String {
    match state {
        DatVerificationState::VerifiedSingleMatch { algorithm } => format!(
            "Exactly one catalogue entry matched this file's {algorithm} hash. This is a \
             cryptographically verified match."
        ),
        DatVerificationState::Probable => "One catalogue entry matched by CRC32 (with size) only. \
             This is likely correct but is not a cryptographically verified match."
            .to_string(),
        DatVerificationState::AmbiguousMultipleCandidates {
            algorithm,
            candidate_count,
        } => format!(
            "This file's {algorithm} hash matches {candidate_count} different catalogue entries, so \
             its exact identity cannot be settled from the catalogue alone."
        ),
        DatVerificationState::Conflicting { detail } => detail.clone(),
        DatVerificationState::NoMatch => "No catalogue entry matched this file's hashes. This does \
             not necessarily mean the file is bad - the catalogue checked may simply not include it."
            .to_string(),
        DatVerificationState::FilenameOnlyNotVerified => {
            "Only the filename matched a catalogue entry - the file's contents were not verified \
             against it."
                .to_string()
        }
        DatVerificationState::NoUsableEvidence => {
            "No hash was available to compare, and the filename matched nothing in the catalogue."
                .to_string()
        }
    }
}

fn match_basis_label(algorithm: &str) -> String {
    match algorithm {
        "SHA-256" => "SHA-256 match".to_string(),
        "SHA-1" => "SHA-1 match".to_string(),
        "MD5" => "MD5 match".to_string(),
        "CRC32" | "CRC32+size" => "CRC match".to_string(),
        other => format!("{other} match"),
    }
}

fn freshness_note(freshness: DatProvenanceFreshness) -> Option<String> {
    match freshness {
        DatProvenanceFreshness::Current => None,
        DatProvenanceFreshness::Stale => {
            Some("The file or catalogue has changed since this result was recorded.".to_string())
        }
        DatProvenanceFreshness::Unknown => Some(
            "Whether this result is still current could not be confirmed - the catalogue may no \
             longer be configured, or there isn't enough matching evidence to compare."
                .to_string(),
        ),
    }
}

/// Builds the presentation for one persisted DAT check. Pure: reads only
/// the fields already on `summary`.
pub(crate) fn present_summary(summary: &LibraryDatIdentitySummary) -> DatIdentityPresentation {
    let stale = matches!(summary.provenance_freshness, DatProvenanceFreshness::Stale);
    let (status, explanation) = match &summary.verification_state {
        DatVerificationState::VerifiedSingleMatch { .. } if stale => (
            DatIdentityStatus::VerifiedNeedsRecheck,
            "This game matched a catalogue entry, but the file or catalogue has changed since \
             that check. Run Verify Games again to confirm it still matches."
                .to_string(),
        ),
        DatVerificationState::VerifiedSingleMatch { .. } => (
            DatIdentityStatus::Verified,
            explain_verification(&summary.verification_state),
        ),
        DatVerificationState::Probable => (
            DatIdentityStatus::LikelyMatch,
            explain_verification(&summary.verification_state),
        ),
        DatVerificationState::AmbiguousMultipleCandidates { .. }
        | DatVerificationState::Conflicting { .. } => (
            DatIdentityStatus::NeedsReview,
            explain_verification(&summary.verification_state),
        ),
        DatVerificationState::NoMatch => (
            DatIdentityStatus::NotFoundInDat,
            explain_verification(&summary.verification_state),
        ),
        DatVerificationState::FilenameOnlyNotVerified | DatVerificationState::NoUsableEvidence => (
            DatIdentityStatus::MoreEvidenceNeeded,
            explain_verification(&summary.verification_state),
        ),
    };

    let verified_as = if matches!(
        summary.verification_state,
        DatVerificationState::VerifiedSingleMatch { .. } | DatVerificationState::Probable
    ) {
        summary.canonical.canonical_dat_name.clone()
    } else {
        None
    };
    let catalogue = summary
        .source
        .ecosystem
        .map(|ecosystem| ecosystem.label().to_string());
    let checked_against =
        (!summary.source.source_name.is_empty()).then(|| summary.source.source_name.clone());
    let variant = summary
        .source
        .variant
        .map(|variant| variant.label().to_string());
    let match_basis = summary
        .hash_evidence
        .matched_algorithm
        .as_deref()
        .map(match_basis_label);

    // "No action needed" is only honest for a clean, current Verified
    // result; every other state - including Verified with unconfirmed or
    // stale freshness - gets the same gentle nudge toward Verify Games.
    let show_verify_action = !matches!(
        (status, summary.provenance_freshness),
        (DatIdentityStatus::Verified, DatProvenanceFreshness::Current)
    );

    DatIdentityPresentation {
        status,
        explanation,
        verified_as,
        catalogue,
        match_basis,
        checked_against,
        variant,
        freshness_note: freshness_note(summary.provenance_freshness),
        show_verify_action,
    }
}

/// Builds the presentation for an item with no persisted DAT result at all -
/// never audited against any DAT, or audited only against a source that no
/// longer retains a row for it. This deliberately covers both "no DAT
/// applies yet" and "nothing usable was found" with one honest message
/// rather than guessing which one it is: distinguishing them would require
/// reading the currently configured DAT sources at render time, which this
/// read-only panel does not do.
pub(crate) fn present_missing() -> DatIdentityPresentation {
    DatIdentityPresentation {
        status: DatIdentityStatus::NotCheckedYet,
        explanation: "No DAT result is stored for this game yet. Configure a game catalogue in \
                      DAT Sources, then run Verify Games to check it."
            .to_string(),
        verified_as: None,
        catalogue: None,
        match_basis: None,
        checked_against: None,
        variant: None,
        freshness_note: None,
        show_verify_action: true,
    }
}

fn render_presentation(ui: &mut egui::Ui, presentation: &DatIdentityPresentation) {
    widgets::status_badge(ui, presentation.status.label(), presentation.status.tone());
    ui.label(&presentation.explanation);
    let mut rows: Vec<(&str, &str)> = Vec::new();
    if let Some(value) = presentation.verified_as.as_deref() {
        rows.push(("Verified as", value));
    }
    if let Some(value) = presentation.catalogue.as_deref() {
        rows.push(("Catalogue", value));
    }
    if let Some(value) = presentation.match_basis.as_deref() {
        rows.push(("Match basis", value));
    }
    if let Some(value) = presentation.checked_against.as_deref() {
        rows.push(("Checked against", value));
    }
    if let Some(value) = presentation.variant.as_deref() {
        rows.push(("Variant", value));
    }
    for (label, value) in rows {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{label}:"));
            ui.label(value);
        });
    }
    if let Some(note) = presentation.freshness_note.as_deref() {
        ui.label(egui::RichText::new(note).color(theme::muted(ui)));
    }
}

/// Renders a "Verify Games" call to action when `show` is true. Returns
/// whether it was clicked - the caller (ultimately `selected_game_panel`)
/// is responsible for routing that into the real, existing Verify Games
/// page; this function never navigates and never starts an audit itself.
fn verify_games_action(ui: &mut egui::Ui, show: bool) -> bool {
    if !show {
        return false;
    }
    ui.add_space(4.0);
    widgets::action_button(ui, "Verify Games", widgets::ActionStyle::Secondary, true).clicked()
}

/// Whether the selected item's current filename already matches the name the
/// catalogue entry carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CatalogueNameCheck {
    /// Nothing to compare against: no match, an ambiguous match with no single
    /// name, the catalogue carried no entry name, or the current filename is
    /// unknown.
    Unknown,
    /// The current filename's stem already equals the catalogue entry's stem.
    Matches,
    /// The catalogue's name for this entry differs from the current filename.
    Differs { catalogue_name: String },
}

/// Compares the selected item's current filename against the catalogue entry's
/// own name. Pure string work over the *stem* (name without its final
/// extension), case-insensitive - it opens no file and, deliberately, never
/// proposes a concrete rename target. Producing a safe, sanitised rename
/// remains the job of DAT Sources -> Quick Rename; this only reports whether
/// the names already agree and, if not, what the catalogue calls the entry.
///
/// Fails closed: any state that is not a settled single match
/// (`is_no_match` / `is_ambiguous`), a missing catalogue name, or a missing
/// current filename all return [`CatalogueNameCheck::Unknown`].
pub(crate) fn catalogue_name_check(
    current_basename: Option<&str>,
    summary: &LibraryDatIdentitySummary,
) -> CatalogueNameCheck {
    if summary.is_no_match() || summary.is_ambiguous() {
        return CatalogueNameCheck::Unknown;
    }
    let Some(catalogue_name) = summary
        .canonical
        .canonical_rom_name
        .as_deref()
        .or(summary.canonical.canonical_dat_name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return CatalogueNameCheck::Unknown;
    };
    let Some(current) = current_basename
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return CatalogueNameCheck::Unknown;
    };
    let stem = |name: &str| {
        name.rsplit_once('.')
            .map(|(base, _)| base)
            .unwrap_or(name)
            .trim()
            .to_ascii_lowercase()
    };
    if stem(current) == stem(catalogue_name) {
        CatalogueNameCheck::Matches
    } else {
        CatalogueNameCheck::Differs {
            catalogue_name: catalogue_name.to_string(),
        }
    }
}

fn bad_metadata_reason_label(reason: BadMetadataReason) -> &'static str {
    match reason {
        BadMetadataReason::NoDump => "the catalogue declares this ROM unverifiable (no known dump)",
        BadMetadataReason::BadDump => "the catalogue itself marks this ROM as a known bad dump",
    }
}

fn needs_review_reason_label(reason: NeedsReviewReason) -> &'static str {
    match reason {
        NeedsReviewReason::AmbiguousMemberAttribution => {
            "a member's hash matched more than one catalogue entry"
        }
        NeedsReviewReason::UnsupportedSetStructure => {
            "this set's shape cannot be verified safely yet"
        }
        NeedsReviewReason::PartialArchivePass => "not every archive member was examined yet",
        NeedsReviewReason::DuplicateGameName => {
            "the catalogue lists more than one entry with this exact name"
        }
        NeedsReviewReason::DuplicateArchiveEvidence => {
            "the same archive member was reported more than once"
        }
        NeedsReviewReason::ContradictoryMemberFlags => {
            "the catalogue's own member flags contradict each other"
        }
        NeedsReviewReason::UnknownLoadflag => "a ROM carries an unrecognised load flag",
        NeedsReviewReason::UnsupportedSoftware => {
            "the catalogue marks this software entry unsupported or malformed"
        }
        NeedsReviewReason::NoDeclaredMembers => "the catalogue entry declares no ROMs or disks",
        NeedsReviewReason::OnlyNonFileOrOptionalMembers => {
            "every declared member is optional, so there is nothing required to anchor this set"
        }
        NeedsReviewReason::AmbiguousDependency => {
            "more than one candidate dependency target was found"
        }
        NeedsReviewReason::DependencyCycle => "a dependency chain refers back to itself",
        NeedsReviewReason::ContradictoryDependencyMetadata => {
            "the dependency metadata contradicts itself or the catalogue"
        }
        NeedsReviewReason::UnsupportedDependencyStructure => {
            "this dependency (e.g. samples) is not checked by this build"
        }
        NeedsReviewReason::DependencyEvidenceIncomplete => {
            "the scan needed to confirm this dependency did not finish"
        }
    }
}

fn set_state_label(state: &SetState) -> String {
    match state {
        SetState::Complete => "Complete - every required member verified".to_string(),
        SetState::Incomplete => {
            "Incomplete - at least one required member is missing or unverified".to_string()
        }
        SetState::BadMetadata(reason) => {
            format!("Needs review - {}", bad_metadata_reason_label(*reason))
        }
        SetState::NeedsReview(reason) => {
            format!("Needs review - {}", needs_review_reason_label(*reason))
        }
    }
}

fn dependency_state_label(state: DependencyState) -> &'static str {
    match state {
        DependencyState::NotApplicable => "No dependencies declared",
        DependencyState::NotEvaluated => "Not evaluated (the catalogue entry is ambiguous)",
        DependencyState::Satisfied => "Satisfied",
        DependencyState::Missing => "Missing a required dependency",
        DependencyState::Ambiguous => "Ambiguous - more than one candidate dependency",
        DependencyState::Cycle => "A dependency cycle was detected",
        DependencyState::Contradictory => "Contradictory dependency information",
        DependencyState::Unsupported => "This dependency type is not checked by this build",
        DependencyState::EvidenceUnavailable => "Not enough evidence to evaluate",
    }
}

/// Renders one DAT source's result, including the shared advanced-details
/// disclosure. Returns whether its "Verify Games" call to action was
/// clicked.
fn show_summary(
    ui: &mut egui::Ui,
    current_basename: Option<&str>,
    summary: &LibraryDatIdentitySummary,
) -> bool {
    let presentation = present_summary(summary);
    render_presentation(ui, &presentation);

    match catalogue_name_check(current_basename, summary) {
        CatalogueNameCheck::Matches => {
            ui.label(
                egui::RichText::new("This file's name matches the catalogue entry.")
                    .color(theme::muted(ui)),
            );
        }
        CatalogueNameCheck::Differs { catalogue_name } => {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Catalogue name for this entry:");
                ui.label(catalogue_name);
            });
            ui.label(
                egui::RichText::new(
                    "The file on disk is named differently. You can preview a rename from \
                     DAT Sources \u{2192} Quick Rename; EmuWiz never renames files on its own.",
                )
                .color(theme::muted(ui)),
            );
        }
        CatalogueNameCheck::Unknown => {}
    }

    widgets::technical_details(
        ui,
        ("dat-identity-technical", &summary.source.source_id),
        |ui| {
            let mut rows: Vec<(&str, String)> = Vec::new();
            if !summary.source.source_name.is_empty() {
                rows.push(("Source", summary.source.source_name.clone()));
            }
            if let Some(value) = &summary.source.source_revision {
                rows.push(("Catalogue revision", value.clone()));
            }
            if let Some(value) = &summary.canonical.canonical_dat_name {
                rows.push(("Canonical DAT name", value.clone()));
            }
            if let Some(value) = &summary.canonical.canonical_rom_name {
                rows.push(("Canonical member", value.clone()));
            }
            if let Some(value) = &summary.canonical.region {
                rows.push(("Region", value.clone()));
            }
            if let Some(value) = &summary.canonical.revision {
                rows.push(("Revision", value.clone()));
            }
            if let Some(value) = &summary.hash_evidence.matched_value {
                rows.push(("Matched value", value.clone()));
            }
            if !summary.hash_evidence.available_algorithms.is_empty() {
                rows.push((
                    "Available hashes",
                    summary.hash_evidence.available_algorithms.join(", "),
                ));
            }
            rows.push((
                "Freshness",
                match summary.provenance_freshness {
                    DatProvenanceFreshness::Current => "Current",
                    DatProvenanceFreshness::Stale => "Stale",
                    DatProvenanceFreshness::Unknown => "Unknown",
                }
                .to_string(),
            ));
            for (label, value) in rows {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("{label}:"));
                    ui.label(value);
                });
            }
        },
    );

    if !summary.ambiguous_candidates.is_empty() {
        ui.collapsing("Candidate DAT names", |ui| {
            for candidate in &summary.ambiguous_candidates {
                ui.strong(candidate);
                for provenance in summary
                    .candidate_provenance
                    .iter()
                    .filter(|provenance| provenance.game_name == *candidate)
                {
                    let mut detail = provenance.source.source_name.clone();
                    if let Some(variant) = provenance.source.variant {
                        detail.push_str(" · ");
                        detail.push_str(variant.label());
                    }
                    ui.label(detail);
                }
            }
        });
    }
    if let DatSetDependencySummary::Resolved {
        set_name,
        members_required,
        members_verified,
        members_missing,
        members_bad,
        members_borrowed,
        disks_required,
        disks_verified,
        dependency_state,
        dependency_requirements,
        state,
        ..
    } = &summary.set_dependency
    {
        ui.collapsing("Set details", |ui| {
            ui.label(format!("Set: {set_name}"));
            ui.label(format!("State: {}", set_state_label(state)));
            ui.label(format!(
                "Members: {members_verified}/{members_required} verified"
            ));
            ui.label(format!(
                "Missing: {members_missing} · bad: {members_bad} · borrowed: {members_borrowed}"
            ));
            ui.label(format!("Disks: {disks_verified}/{disks_required} verified"));
            ui.label(format!(
                "Dependency: {} ({dependency_requirements} requirement{})",
                dependency_state_label(*dependency_state),
                if *dependency_requirements == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        });
    }

    verify_games_action(ui, presentation.show_verify_action)
}

/// Renders the selected item's DAT check section and returns whether its
/// "Verify Games" call to action was clicked - the caller routes that into
/// the existing Verify Games page. This function never navigates itself and
/// never starts an audit merely because the panel was opened.
pub(crate) fn show_dat_identity_section(
    ui: &mut egui::Ui,
    current_basename: Option<&str>,
    summaries: &[LibraryDatIdentitySummary],
) -> bool {
    ui.add_space(6.0);
    ui.strong("DAT check");
    ui.label(
        egui::RichText::new(
            "Matched against a known catalogue - separate from the game's own identity \
             evidence above.",
        )
        .color(theme::muted(ui))
        .small(),
    );
    if summaries.is_empty() {
        let presentation = present_missing();
        render_presentation(ui, &presentation);
        return verify_games_action(ui, presentation.show_verify_action);
    }
    if summaries.len() > 1 {
        let names = summaries
            .iter()
            .filter_map(|summary| summary.canonical.canonical_dat_name.as_deref())
            .collect::<std::collections::BTreeSet<_>>();
        if names.len() > 1 {
            widgets::status_badge(
                ui,
                "Needs review · conflicting DAT sources",
                widgets::StatusTone::Warning,
            );
            ui.label(
                "More than one configured catalogue reports a different entry for this game. \
                 Open each source below and review them; EmuWiz never picks one automatically.",
            );
        } else {
            widgets::status_badge(
                ui,
                "Multiple DAT sources · review each source",
                widgets::StatusTone::Warning,
            );
            ui.label("More than one configured catalogue has a result for this game.");
        }
        let mut any_action = false;
        ui.collapsing("DAT sources", |ui| {
            for summary in summaries {
                let title = if summary.source.source_name.is_empty() {
                    "Unnamed DAT source"
                } else {
                    summary.source.source_name.as_str()
                };
                ui.collapsing(title, |ui| {
                    if show_summary(ui, current_basename, summary) {
                        any_action = true;
                    }
                });
            }
        });
        return verify_games_action(ui, any_action);
    }
    show_summary(ui, current_basename, &summaries[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(state: DatVerificationState) -> LibraryDatIdentitySummary {
        LibraryDatIdentitySummary {
            verification_state: state,
            source: archivefs_core::dat::library_identity_summary::DatSourceProvenance {
                source_id: "source".into(),
                source_name: "No-Intro".into(),
                ecosystem: None,
                variant: None,
                source_revision: Some("rev".into()),
                author: None,
                catalogue_names: vec![],
                dat_path: "x".into(),
            },
            canonical: archivefs_core::dat::library_identity_summary::DatCanonicalIdentity {
                canonical_dat_name: Some("Title".into()),
                canonical_rom_name: Some("Title.rom".into()),
                region: Some("USA".into()),
                revision: Some("Rev 1".into()),
            },
            hash_evidence: archivefs_core::dat::library_identity_summary::DatHashEvidenceSummary {
                matched_algorithm: Some("SHA-1".into()),
                matched_value: Some("abc".into()),
                available_algorithms: vec!["SHA-1".into()],
            },
            provenance_freshness: DatProvenanceFreshness::Current,
            ambiguous_candidates: vec!["Other".into()],
            candidate_provenance: Vec::new(),
            set_dependency: DatSetDependencySummary::Pending {
                reason: "not retained".into(),
            },
        }
    }

    #[test]
    fn presentation_shows_known_variant_without_changing_verified_status() {
        let mut summary = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        summary.source.variant =
            Some(archivefs_core::identity_source::no_intro::NoIntroVariant::Headerless);
        let presentation = present_summary(&summary);

        assert_eq!(presentation.status, DatIdentityStatus::Verified);
        assert_eq!(presentation.variant.as_deref(), Some("Headerless"));
    }

    fn collect_text(output: &egui::FullOutput) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn render(
        current_basename: Option<&str>,
        summaries: &[LibraryDatIdentitySummary],
    ) -> (String, bool) {
        let context = egui::Context::default();
        let mut clicked = false;
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                clicked = show_dat_identity_section(ui, current_basename, summaries);
            });
        });
        (collect_text(&output), clicked)
    }

    // --- Status truthfulness -------------------------------------------

    #[test]
    fn every_state_has_a_truthful_status() {
        let states = [
            DatVerificationState::VerifiedSingleMatch {
                algorithm: "SHA-1".into(),
            },
            DatVerificationState::Probable,
            DatVerificationState::AmbiguousMultipleCandidates {
                algorithm: "SHA-1".into(),
                candidate_count: 2,
            },
            DatVerificationState::Conflicting {
                detail: "conflict".into(),
            },
            DatVerificationState::NoMatch,
            DatVerificationState::FilenameOnlyNotVerified,
            DatVerificationState::NoUsableEvidence,
        ];
        for state in states {
            let presentation = present_summary(&summary(state));
            assert!(!presentation.status.label().is_empty());
        }
    }

    #[test]
    fn every_verification_state_has_a_nonempty_plain_explanation() {
        let states = [
            DatVerificationState::VerifiedSingleMatch {
                algorithm: "SHA-1".into(),
            },
            DatVerificationState::Probable,
            DatVerificationState::AmbiguousMultipleCandidates {
                algorithm: "SHA-1".into(),
                candidate_count: 2,
            },
            DatVerificationState::Conflicting {
                detail: "the two configured DATs disagree".into(),
            },
            DatVerificationState::NoMatch,
            DatVerificationState::FilenameOnlyNotVerified,
            DatVerificationState::NoUsableEvidence,
        ];
        for state in states {
            let text = explain_verification(&state);
            assert!(!text.trim().is_empty(), "empty explanation for {state:?}");
        }
    }

    #[test]
    fn a_verified_explanation_names_the_algorithm_and_never_overstates_a_probable_match() {
        let verified = explain_verification(&DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-256".into(),
        });
        assert!(verified.contains("SHA-256"));
        assert!(verified.to_lowercase().contains("verified"));

        let probable = explain_verification(&DatVerificationState::Probable);
        assert!(
            probable
                .to_lowercase()
                .contains("not a cryptographically verified")
        );
    }

    // --- Verified state ---------------------------------------------------

    #[test]
    fn a_current_verified_match_needs_no_action_and_shows_the_expected_defaults() {
        let s = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        let presentation = present_summary(&s);
        assert_eq!(presentation.status, DatIdentityStatus::Verified);
        assert_eq!(presentation.status.label(), "Verified");
        assert!(!presentation.show_verify_action);
        assert_eq!(presentation.verified_as.as_deref(), Some("Title"));
        assert_eq!(presentation.match_basis.as_deref(), Some("SHA-1 match"));
        assert!(presentation.catalogue.is_none()); // no ecosystem set in this fixture
        assert_eq!(presentation.checked_against.as_deref(), Some("No-Intro"));

        let (text, clicked) = render(Some("Title.rom"), std::slice::from_ref(&s));
        assert!(text.contains("Verified"));
        assert!(text.contains("Verified as"));
        assert!(text.contains("Match basis"));
        assert!(text.contains("Checked against"));
        assert!(!text.contains("Verify Games"));
        assert!(!clicked);
    }

    #[test]
    fn a_stale_verified_match_is_elevated_to_needs_recheck_and_offers_verify_games() {
        let mut s = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        s.provenance_freshness = DatProvenanceFreshness::Stale;
        let presentation = present_summary(&s);
        assert_eq!(presentation.status, DatIdentityStatus::VerifiedNeedsRecheck);
        assert_eq!(presentation.status.label(), "Verified, needs re-check");
        assert!(presentation.show_verify_action);

        let (text, _clicked) = render(Some("Title.rom"), std::slice::from_ref(&s));
        assert!(text.contains("Verified, needs re-check"));
        assert!(text.contains("Verify Games"));
        assert!(text.to_lowercase().contains("changed since"));
        // Never implies the ROM itself is corrupt merely because the DAT
        // state is stale.
        assert!(!text.to_lowercase().contains("corrupt"));
    }

    #[test]
    fn an_unknown_freshness_verified_match_still_offers_a_gentle_recheck_nudge() {
        let mut s = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        s.provenance_freshness = DatProvenanceFreshness::Unknown;
        let presentation = present_summary(&s);
        // Still "Verified" (not demoted), but the action is offered because
        // "no action needed" would overstate a genuinely unconfirmed result.
        assert_eq!(presentation.status, DatIdentityStatus::Verified);
        assert!(presentation.show_verify_action);
        assert!(presentation.freshness_note.is_some());
    }

    // --- No-DAT / unverified states ----------------------------------------

    #[test]
    fn no_persisted_summary_shows_a_useful_not_checked_yet_state_with_an_action() {
        let (text, clicked_without_click) = render(Some("Mystery Game.zip"), &[]);
        assert!(text.contains("Not checked yet"));
        assert!(text.contains("DAT Sources"));
        assert!(text.contains("Verify Games"));
        assert!(!text.to_lowercase().contains("verified match"));
        assert!(!clicked_without_click); // rendering never auto-clicks
    }

    #[test]
    fn no_match_is_distinguished_from_more_evidence_needed() {
        let no_match = present_summary(&summary(DatVerificationState::NoMatch));
        assert_eq!(no_match.status, DatIdentityStatus::NotFoundInDat);
        assert_eq!(no_match.status.label(), "Not found in this DAT");
        assert!(
            no_match
                .explanation
                .to_lowercase()
                .contains("no catalogue entry matched")
        );
        assert!(!no_match.explanation.to_lowercase().contains("corrupt"));

        let filename_only =
            present_summary(&summary(DatVerificationState::FilenameOnlyNotVerified));
        assert_eq!(filename_only.status, DatIdentityStatus::MoreEvidenceNeeded);
        assert!(
            filename_only
                .explanation
                .to_lowercase()
                .contains("filename")
        );

        let no_evidence = present_summary(&summary(DatVerificationState::NoUsableEvidence));
        assert_eq!(no_evidence.status, DatIdentityStatus::MoreEvidenceNeeded);
        assert!(no_evidence.explanation.to_lowercase().contains("no hash"));

        // Both "more evidence needed" cases share one status label but
        // never an identical explanation - they are genuinely different
        // situations.
        assert_ne!(filename_only.explanation, no_evidence.explanation);
    }

    #[test]
    fn probable_match_is_never_labelled_verified() {
        let probable = present_summary(&summary(DatVerificationState::Probable));
        assert_eq!(probable.status, DatIdentityStatus::LikelyMatch);
        assert_ne!(probable.status.label(), "Verified");
        assert!(probable.verified_as.is_some());
    }

    // --- Conflict presentation ----------------------------------------------

    #[test]
    fn ambiguous_candidates_are_needs_review_never_a_silent_pick_and_state_the_count() {
        let s = summary(DatVerificationState::AmbiguousMultipleCandidates {
            algorithm: "SHA-1".into(),
            candidate_count: 3,
        });
        let presentation = present_summary(&s);
        assert_eq!(presentation.status, DatIdentityStatus::NeedsReview);
        assert!(presentation.verified_as.is_none());
        assert!(presentation.explanation.contains('3'));
        assert!(presentation.show_verify_action);
        assert_ne!(presentation.status.tone(), widgets::StatusTone::Blocked);

        // The candidate list lives behind the shared collapsed-by-default
        // disclosure (`ui.collapsing`), so a one-shot render only proves the
        // headline and the disclosure's own header exist - not its body,
        // which egui never paints until expanded (see
        // `technical_details_hides_its_body_until_expanded`). The count and
        // the actual candidate names are already asserted above, straight
        // from the pure projection.
        let (text, _) = render(Some("whatever.bin"), std::slice::from_ref(&s));
        assert!(text.contains("Needs review"));
        assert!(text.contains("Candidate DAT names"));
    }

    #[test]
    fn conflicting_evidence_shows_the_real_detail_up_front() {
        let s = summary(DatVerificationState::Conflicting {
            detail: "the two configured DATs disagree about this file".into(),
        });
        let presentation = present_summary(&s);
        assert_eq!(presentation.status, DatIdentityStatus::NeedsReview);
        assert_eq!(
            presentation.explanation,
            "the two configured DATs disagree about this file"
        );
        assert_ne!(presentation.status.tone(), widgets::StatusTone::Blocked);
    }

    #[test]
    fn multiple_sources_naming_different_entries_are_flagged_as_conflicting() {
        let mut first = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        first.source.source_name = "No-Intro".into();
        let mut second = first.clone();
        second.source.source_name = "Redump".into();
        second.canonical.canonical_dat_name = Some("Different Title".into());

        // The per-source names render inside the "DAT sources" disclosure's
        // own body, collapsed by default - not asserted here for the same
        // reason as the ambiguous-candidates test above. The outer headline
        // and the disclosure header itself are not behind another
        // collapse, so they do render.
        let (text, _) = render(Some("Some Game.zip"), &[first, second]);
        assert!(text.contains("conflicting DAT sources"));
        assert!(text.contains("DAT sources"));
    }

    #[test]
    fn multiple_agreeing_sources_are_multiple_not_conflicting() {
        let mut first = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        first.source.source_name = "No-Intro".into();
        let mut second = first.clone();
        second.source.source_name = "Redump".into();

        let (text, _) = render(Some("Some Game.zip"), &[first, second]);
        assert!(text.contains("Multiple DAT sources"));
        assert!(!text.contains("conflicting DAT sources"));
    }

    // --- DAT vs structural identity / no raw enum strings -------------------

    #[test]
    fn the_dat_section_names_itself_distinctly_from_structural_evidence() {
        let (text, _) = render(None, &[]);
        assert!(text.contains("DAT check"));
        assert!(text.to_lowercase().contains("known catalogue"));
    }

    #[test]
    fn no_state_ever_renders_a_raw_enum_or_debug_token() {
        let states = [
            DatVerificationState::VerifiedSingleMatch {
                algorithm: "SHA-1".into(),
            },
            DatVerificationState::Probable,
            DatVerificationState::AmbiguousMultipleCandidates {
                algorithm: "SHA-1".into(),
                candidate_count: 2,
            },
            DatVerificationState::Conflicting {
                detail: "conflict".into(),
            },
            DatVerificationState::NoMatch,
            DatVerificationState::FilenameOnlyNotVerified,
            DatVerificationState::NoUsableEvidence,
        ];
        for state in states {
            let (text, _) = render(Some("Game.rom"), &[summary(state)]);
            assert!(!text.contains("VerifiedSingleMatch"));
            assert!(!text.contains("AmbiguousMultipleCandidates"));
            assert!(!text.contains("NoUsableEvidence"));
            assert!(!text.contains("FilenameOnlyNotVerified"));
        }
    }

    #[test]
    fn set_state_and_dependency_labels_never_leak_a_raw_debug_enum() {
        let needs_review = set_state_label(&SetState::NeedsReview(
            NeedsReviewReason::PartialArchivePass,
        ));
        assert!(!needs_review.contains("NeedsReview("));
        assert!(!needs_review.contains("PartialArchivePass"));
        assert!(needs_review.contains("Needs review"));

        let bad_metadata = set_state_label(&SetState::BadMetadata(BadMetadataReason::BadDump));
        assert!(!bad_metadata.contains("BadMetadata("));
        assert!(!bad_metadata.contains("BadDump"));
        assert!(bad_metadata.contains("Needs review"));

        assert_eq!(
            set_state_label(&SetState::Complete),
            "Complete - every required member verified"
        );
        assert!(set_state_label(&SetState::Incomplete).starts_with("Incomplete"));

        let unavailable = dependency_state_label(DependencyState::EvidenceUnavailable);
        assert!(!unavailable.contains("EvidenceUnavailable"));
        assert!(unavailable.to_lowercase().contains("evidence"));
        assert_eq!(
            dependency_state_label(DependencyState::Satisfied),
            "Satisfied"
        );
    }

    #[test]
    fn resolved_set_dependency_renders_its_disclosure_header_without_panicking() {
        let mut s = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        s.set_dependency = DatSetDependencySummary::Resolved {
            set_name: "somegame".into(),
            source_id: "src".into(),
            state: SetState::NeedsReview(NeedsReviewReason::PartialArchivePass),
            members_required: 3,
            members_verified: 1,
            members_missing: 2,
            members_bad: 0,
            members_borrowed: 0,
            disks_required: 0,
            disks_verified: 0,
            dependency_state: DependencyState::EvidenceUnavailable,
            dependency_requirements: 1,
        };
        // The row's own detail text lives behind the collapsed-by-default
        // "Set details" disclosure; the label content itself is proven raw-
        // enum-free directly above. This only proves the section renders at
        // all (its header) without panicking.
        let (text, _) = render(Some("somegame.zip"), std::slice::from_ref(&s));
        assert!(text.contains("Set details"));
    }

    // --- Match basis translation --------------------------------------------

    #[test]
    fn match_basis_translates_hash_algorithms_and_never_invents_one() {
        assert_eq!(match_basis_label("SHA-1"), "SHA-1 match");
        assert_eq!(match_basis_label("SHA-256"), "SHA-256 match");
        assert_eq!(match_basis_label("MD5"), "MD5 match");
        assert_eq!(match_basis_label("CRC32"), "CRC match");
        assert_eq!(match_basis_label("CRC32+size"), "CRC match");

        let mut no_hash = summary(DatVerificationState::NoMatch);
        no_hash.hash_evidence.matched_algorithm = None;
        assert!(present_summary(&no_hash).match_basis.is_none());
    }

    // --- Empty/null safety ---------------------------------------------------

    #[test]
    fn missing_optional_source_and_revision_render_without_panicking_or_none_text() {
        let mut s = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        s.source.source_name = String::new();
        s.source.source_revision = None;
        s.canonical.canonical_dat_name = None;
        s.canonical.canonical_rom_name = None;
        let (text, _) = render(None, std::slice::from_ref(&s));
        assert!(!text.to_lowercase().contains("none"));
    }

    #[test]
    fn absent_dat_evidence_renders_without_fabricating_metadata() {
        let (text, _) = render(Some("Mystery Game.zip"), &[]);
        assert!(text.contains("Not checked yet"));
        assert!(!text.to_lowercase().contains("verified match"));
    }

    #[test]
    fn catalogue_name_check_matches_when_the_stem_is_equal_ignoring_extension_and_case() {
        // Loose file already named exactly as the catalogue member.
        let mut s = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        s.canonical.canonical_rom_name = Some("Sonic The Hedgehog (USA, Europe).md".into());
        assert_eq!(
            catalogue_name_check(Some("sonic the hedgehog (usa, europe).MD"), &s),
            CatalogueNameCheck::Matches
        );

        // Archive whose stem matches the catalogue <game> name; the member's
        // own extension differs and must not cause a false "differs".
        s.canonical.canonical_rom_name = Some("Sonic The Hedgehog (USA, Europe).md".into());
        s.canonical.canonical_dat_name = Some("Sonic The Hedgehog (USA, Europe)".into());
        assert_eq!(
            catalogue_name_check(Some("Sonic The Hedgehog (USA, Europe).zip"), &s),
            CatalogueNameCheck::Matches
        );
    }

    #[test]
    fn catalogue_name_check_reports_the_catalogue_name_when_it_differs_without_proposing_a_rename()
    {
        let mut s = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        s.canonical.canonical_rom_name = Some("Sonic The Hedgehog (USA, Europe).md".into());
        assert_eq!(
            catalogue_name_check(Some("sonic1.bin"), &s),
            CatalogueNameCheck::Differs {
                catalogue_name: "Sonic The Hedgehog (USA, Europe).md".into(),
            }
        );
    }

    #[test]
    fn catalogue_name_check_fails_closed_for_ambiguous_no_match_and_missing_inputs() {
        let ambiguous = summary(DatVerificationState::AmbiguousMultipleCandidates {
            algorithm: "SHA-1".into(),
            candidate_count: 3,
        });
        assert_eq!(
            catalogue_name_check(Some("whatever.bin"), &ambiguous),
            CatalogueNameCheck::Unknown
        );

        let no_match = summary(DatVerificationState::NoMatch);
        assert_eq!(
            catalogue_name_check(Some("whatever.bin"), &no_match),
            CatalogueNameCheck::Unknown
        );

        // A settled match but no current filename to compare.
        let verified = summary(DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        });
        assert_eq!(
            catalogue_name_check(None, &verified),
            CatalogueNameCheck::Unknown
        );

        // A settled match but the catalogue carried no entry name.
        let mut nameless = verified;
        nameless.canonical.canonical_rom_name = None;
        nameless.canonical.canonical_dat_name = None;
        assert_eq!(
            catalogue_name_check(Some("whatever.bin"), &nameless),
            CatalogueNameCheck::Unknown
        );
    }
}
