//! Read-only presentation of DAT identity already persisted by the core.
//!
//! This module deliberately performs no matching, hashing, file I/O, network
//! access, or mutation.  It only turns cached core summaries into UI text.

use archivefs_core::dat::library_identity_summary::{
    DatProvenanceFreshness, DatSetDependencySummary, DatVerificationState,
    LibraryDatIdentitySummary,
};
use eframe::egui;

use crate::theme;
use crate::widgets;

fn status(summary: &LibraryDatIdentitySummary) -> (&'static str, widgets::StatusTone) {
    match summary.verification_state {
        DatVerificationState::VerifiedSingleMatch { .. } => {
            ("Verified", widgets::StatusTone::Success)
        }
        DatVerificationState::Probable => ("Probable DAT match", widgets::StatusTone::Warning),
        DatVerificationState::AmbiguousMultipleCandidates { .. }
        | DatVerificationState::Conflicting { .. } => {
            ("Needs review", widgets::StatusTone::Warning)
        }
        DatVerificationState::NoMatch
        | DatVerificationState::FilenameOnlyNotVerified
        | DatVerificationState::NoUsableEvidence => (
            "Unverified / No stored DAT identity",
            widgets::StatusTone::Info,
        ),
    }
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
        DatVerificationState::NoMatch => {
            "No catalogue entry matched this file's hashes.".to_string()
        }
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

fn show_summary(
    ui: &mut egui::Ui,
    current_basename: Option<&str>,
    summary: &LibraryDatIdentitySummary,
) {
    let (label, tone) = status(summary);
    widgets::status_badge(ui, label, tone);
    ui.label(explain_verification(&summary.verification_state));

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

    let mut rows: Vec<(&str, String)> = Vec::new();
    if let Some(ecosystem) = summary.source.ecosystem {
        rows.push(("Ecosystem", ecosystem.label().to_string()));
    }
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
    if let Some(value) = &summary.hash_evidence.matched_algorithm {
        rows.push(("Matched algorithm", value.clone()));
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

    if !summary.ambiguous_candidates.is_empty() {
        ui.collapsing("Candidate DAT names", |ui| {
            for candidate in &summary.ambiguous_candidates {
                ui.label(candidate);
            }
        });
    }
    if let DatVerificationState::Conflicting { detail } = &summary.verification_state {
        ui.collapsing("Explanation", |ui| ui.label(detail));
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
            ui.label(format!("State: {state:?}"));
            ui.label(format!(
                "Members: {members_verified}/{members_required} verified"
            ));
            ui.label(format!(
                "Missing: {members_missing} · bad: {members_bad} · borrowed: {members_borrowed}"
            ));
            ui.label(format!("Disks: {disks_verified}/{disks_required} verified"));
            ui.label(format!(
                "Dependency: {dependency_state:?} ({dependency_requirements} requirements)"
            ));
        });
    }
}

pub(crate) fn show_dat_identity_section(
    ui: &mut egui::Ui,
    current_basename: Option<&str>,
    summaries: &[LibraryDatIdentitySummary],
) {
    ui.add_space(6.0);
    ui.strong("DAT Identity");
    if summaries.is_empty() {
        ui.label("Unverified / No stored DAT identity");
        return;
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
        } else {
            widgets::status_badge(
                ui,
                "Multiple DAT sources · review each source",
                widgets::StatusTone::Warning,
            );
        }
        ui.collapsing("DAT sources", |ui| {
            for summary in summaries {
                let title = if summary.source.source_name.is_empty() {
                    "Unnamed DAT source"
                } else {
                    summary.source.source_name.as_str()
                };
                ui.collapsing(title, |ui| show_summary(ui, current_basename, summary));
            }
        });
    } else {
        show_summary(ui, current_basename, &summaries[0]);
    }
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
            provenance_freshness: DatProvenanceFreshness::Unknown,
            ambiguous_candidates: vec!["Other".into()],
            set_dependency: DatSetDependencySummary::Pending {
                reason: "not retained".into(),
            },
        }
    }

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
            assert!(!status(&summary(state)).0.is_empty());
        }
    }

    #[test]
    fn empty_and_multiple_source_panels_render() {
        let context = egui::Context::default();
        let first = summary(DatVerificationState::NoMatch);
        let mut second = first.clone();
        second.source.source_name = "Redump".into();
        second.canonical.canonical_dat_name = Some("Different".into());
        let _ = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_dat_identity_section(ui, None, &[]);
                show_dat_identity_section(ui, Some("Some Game.zip"), &[first.clone()]);
                show_dat_identity_section(
                    ui,
                    Some("Some Game.zip"),
                    &[first.clone(), second.clone()],
                );
            });
        });
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

    #[test]
    fn absent_dat_evidence_renders_without_fabricating_metadata() {
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_dat_identity_section(ui, Some("Mystery Game.zip"), &[]);
            });
        });
        // The empty-state text is shown and nothing pretends to be a match.
        let rendered = collect_text(&output);
        assert!(rendered.contains("No stored DAT identity"));
        assert!(!rendered.to_lowercase().contains("verified match"));
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
}
