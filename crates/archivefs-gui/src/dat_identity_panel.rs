//! Read-only presentation of DAT identity already persisted by the core.
//!
//! This module deliberately performs no matching, hashing, file I/O, network
//! access, or mutation.  It only turns cached core summaries into UI text.

use archivefs_core::dat::library_identity_summary::{
    DatProvenanceFreshness, DatSetDependencySummary, DatVerificationState,
    LibraryDatIdentitySummary,
};
use eframe::egui;

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

fn show_summary(ui: &mut egui::Ui, summary: &LibraryDatIdentitySummary) {
    let (label, tone) = status(summary);
    widgets::status_badge(ui, label, tone);

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
                ui.collapsing(title, |ui| show_summary(ui, summary));
            }
        });
    } else {
        show_summary(ui, &summaries[0]);
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
                show_dat_identity_section(ui, &[]);
                show_dat_identity_section(ui, &[first.clone(), second.clone()]);
            });
        });
    }
}
