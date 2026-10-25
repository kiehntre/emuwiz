use super::*;
use crate::selected_evidence_page::*;
use archivefs_core::content_evidence::{
    ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind,
};
use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
use archivefs_core::identity_source::no_intro::import_no_intro_dat;
use archivefs_core::platform_evidence_fusion::identity_orchestrator::{
    IdentityInspectionInput, inspect_identity,
};
use archivefs_core::platform_evidence_fusion::identity_presentation::{
    IdentityStatus, present_identity,
};
use std::path::{Path, PathBuf};

struct Fixture {
    dir: tempfile::TempDir,
    path: PathBuf,
    hashes: LocalHashes,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("selected.bin");
        std::fs::write(
            &path,
            b"synthetic selected-file bytes, not a filename identity",
        )
        .unwrap();
        let hashes = archivefs_core::identity_source::hashing::hash_file(
            &path,
            &archivefs_core::safe_read::TrustedRoots::from_paths([dir.path()]),
            None,
        )
        .unwrap();
        Self { dir, path, hashes }
    }
    fn source(&self, filename: &str, platform: &str, sha1: &str) -> ImportedNoIntroSource {
        let path = self.dir.path().join(filename);
        std::fs::write(&path, format!(r#"<?xml version="1.0"?><datafile>
<header><name>{platform}</name><author>No-Intro</author><version>20260908-{filename}</version></header>
<game name="Verified release"><rom name="verified.bin" size="{}" sha1="{sha1}"/></game>
</datafile>"#, self.hashes.bytes_hashed)).unwrap();
        import_no_intro_dat(&path).unwrap()
    }
    fn report(&self) -> SelectedEvidenceReport {
        gather_selected_evidence_fast(&self.path, None).unwrap()
    }
    fn payload(&self, source: &ImportedNoIntroSource) -> SelectedEvidenceEnrichment {
        enrichment_from_hashes(self.hashes.clone(), &[(None, source)])
    }
}

fn strong_base() -> IdentityResult {
    inspect_identity(IdentityInspectionInput {
        content_evidence: vec![ContentEvidence::new(
            ContentEvidenceKind::BootStructure,
            "Nintendo Game Boy logo",
            ContentEvidenceConfidence::Strong,
            "original bounded content inspector",
        )],
        ..Default::default()
    })
}
fn apply(
    report: &mut SelectedEvidenceReport,
    payload: SelectedEvidenceEnrichment,
    path: &Path,
) -> bool {
    apply_selected_evidence_enrichment_bound(report, payload, Some(path), 7, 7)
}

#[test]
fn selected_dat_fusion_preserves_base_identity_and_source_provenance() {
    let f = Fixture::new();
    let source = f.source("gb.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let mut report = f.report();
    let mut base = strong_base();
    base.caveats.push("original limitation");
    report.identity = present_identity(&base);
    report.identity_result = base.clone();
    let original_game_identity = format!("{:?}", report.game_identity_report);
    assert!(apply(&mut report, f.payload(&source), &f.path));
    let provenance = report.dat_identity.as_ref().unwrap();
    assert_eq!(provenance.base, base);
    assert_eq!(report.identity_result.content, base.content);
    assert_eq!(report.identity_result.set_identity, base.set_identity);
    assert!(
        report
            .identity_result
            .caveats
            .contains(&"original limitation")
    );
    assert_eq!(report.identity, present_identity(&report.identity_result));
    assert_eq!(report.identity.status, IdentityStatus::ContentAndDatAgree);
    assert_eq!(
        provenance.sources[0].artifact.artifact_sha256.as_deref(),
        Some(source.artifact_sha256.as_str())
    );
    assert_eq!(
        provenance.sources[0].artifact.upstream_version,
        source.upstream_version
    );
    assert_eq!(
        provenance.sources[0].catalogue,
        identify_dat_source(&source.dat)
    );
    assert_eq!(
        format!("{:?}", report.game_identity_report),
        original_game_identity
    );
}

#[test]
fn selected_dat_fusion_does_not_turn_a_filename_or_nonmatch_into_identity() {
    let f = Fixture::new();
    let source = f.source("Game Boy.dat", "Unclassified catalogue", &f.hashes.sha1);
    let mut report = f.report();
    assert!(apply(&mut report, f.payload(&source), &f.path));
    assert!(
        report
            .identity_result
            .dat
            .as_ref()
            .unwrap()
            .platform()
            .is_none()
    );
    assert_eq!(report.identity.platform, None);
    let wrong = f.source("wrong.dat", "Nintendo - Game Boy", &"0".repeat(40));
    let mut report = f.report();
    let base = report.identity_result.clone();
    assert!(apply(&mut report, f.payload(&wrong), &f.path));
    assert_eq!(report.identity_result, base);
    assert!(report.dat_identity.is_none());
}

#[test]
fn selected_dat_fusion_keeps_strong_content_when_dat_has_only_weak_platform_hints() {
    let f = Fixture::new();
    let source = f.source("NES.dat", "Unclassified catalogue", &f.hashes.sha1);
    let mut report = f.report();
    let base = strong_base();
    report.identity_result = base.clone();
    report.identity = present_identity(&base);
    assert!(apply(&mut report, f.payload(&source), &f.path));
    assert_eq!(report.identity.platform, Some("Game Boy"));
    assert_eq!(report.identity_result.content, base.content);
    assert_eq!(report.dat_identity.as_ref().unwrap().base, base);
}

#[test]
fn selected_dat_fusion_rejects_old_selection_generation_and_hash_payload() {
    let f = Fixture::new();
    let source = f.source("gb.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let mut report = f.report();
    let base = report.identity_result.clone();
    assert!(!apply_selected_evidence_enrichment_bound(
        &mut report,
        f.payload(&source),
        Some(&f.path),
        8,
        7
    ));
    assert!(!apply_selected_evidence_enrichment_bound(
        &mut report,
        f.payload(&source),
        Some(Path::new("new.bin")),
        7,
        7
    ));
    assert!(!apply_selected_evidence_enrichment_bound(
        &mut report,
        f.payload(&source),
        None,
        7,
        7
    ));
    let mut wrong_hash = f.payload(&source);
    wrong_hash.hashes.sha1 = "f".repeat(40);
    assert!(!apply(&mut report, wrong_hash, &f.path));
    let mut wrong_path = f.payload(&source);
    wrong_path.hashes.fingerprint.path = PathBuf::from("old.bin");
    assert!(!apply(&mut report, wrong_path, &f.path));
    assert_eq!(report.identity_result, base);
    assert!(report.hashes.is_none());
    assert!(report.dat_identity.is_none());
}

#[test]
fn selected_dat_fusion_retains_multiple_source_artifacts_without_voting_or_first_pick() {
    let f = Fixture::new();
    let a = f.source("a.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let b = f.source("b.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let mut registry = DatSourceRegistry::new();
    for (id, source) in [("a", &a), ("b", &b)] {
        registry
            .add(DatSourceEntry::new(
                id.to_string(),
                id.to_string(),
                source.artifact_path.clone(),
                DatSourceKind::File,
            ))
            .unwrap();
    }
    let mut cache = crate::selected_evidence_no_intro::NoIntroSourceCache::new();
    let all = cache.resolve_all(&registry, None).to_vec();
    assert_eq!(all.len(), 2);
    let refs: Vec<_> = all
        .iter()
        .map(|(label, source)| (Some(label), source.as_ref()))
        .collect();
    let payload = compute_selected_evidence_enrichment_from_sources(&f.path, &refs, None).unwrap();
    let mut report = f.report();
    assert!(apply(&mut report, payload, &f.path));
    let sources = &report.dat_identity.as_ref().unwrap().sources;
    assert_eq!(
        sources
            .iter()
            .map(|s| s.registry_id.as_deref())
            .collect::<Vec<_>>(),
        [Some("a"), Some("b")]
    );
    assert_ne!(
        sources[0].artifact.artifact_sha256,
        sources[1].artifact.artifact_sha256
    );
    assert_eq!(
        report.identity_result.dat.as_ref().unwrap().platform(),
        Some("Game Boy")
    );
    assert!(matches!(
        report.identity_result.representation_match,
        Some(RepresentationMatchOutcome::PhysicalOnly {
            verdict: AuditVerdict::ExactMultipleCandidates { count: 2, .. }
        })
    ));
    assert_eq!(report.base_observations.len(), 4);
    let reversed = fuse(
        &report.dat_identity.as_ref().unwrap().base,
        &sources.iter().rev().cloned().collect::<Vec<_>>(),
    );
    assert_eq!(reversed, report.identity_result);
}

#[test]
fn selected_dat_fusion_conflicting_dat_platforms_fail_closed_and_retain_both_sources() {
    let f = Fixture::new();
    let a = f.source("a.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let b = f.source(
        "b.dat",
        "Nintendo - Nintendo Entertainment System",
        &f.hashes.sha1,
    );
    let mut report = f.report();
    let payload = enrichment_from_hashes(f.hashes.clone(), &[(None, &a), (None, &b)]);
    assert!(apply(&mut report, payload, &f.path));
    assert!(report.identity_result.dat.as_ref().unwrap().is_ambiguous());
    assert_eq!(report.identity.status, IdentityStatus::Ambiguous);
    assert_eq!(report.dat_identity.as_ref().unwrap().sources.len(), 2);
}

#[test]
fn selected_dat_fusion_content_dat_disagreement_keeps_base_and_blocks_preview() {
    let f = Fixture::new();
    let source = f.source(
        "nes.dat",
        "Nintendo - Nintendo Entertainment System",
        &f.hashes.sha1,
    );
    let mut report = f.report();
    let base = strong_base();
    report.identity_result = base.clone();
    report.identity = present_identity(&base);
    assert!(apply(&mut report, f.payload(&source), &f.path));
    assert_eq!(report.identity.status, IdentityStatus::Conflict);
    assert_eq!(report.identity_result.content, base.content);
}

#[test]
fn selected_dat_fusion_does_not_rematch_or_overwrite_an_already_verified_report() {
    let f = Fixture::new();
    let source = f.source("gb.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let mut report = f.report();
    assert!(apply(&mut report, f.payload(&source), &f.path));
    let identity = report.identity_result.clone();
    let provenance = report.dat_identity.clone();
    report.enrichment = SelectedEvidenceEnrichmentStatus::Pending;
    assert!(!needs_selected_evidence_enrichment(&report));
    assert!(!apply(
        &mut report,
        enrichment_from_hashes(f.hashes.clone(), &[]),
        &f.path
    ));
    assert_eq!(report.identity_result, identity);
    assert_eq!(report.dat_identity, provenance);
}

#[test]
fn selected_dat_fusion_preserves_existing_normalized_identity_and_all_other_axes() {
    let f = Fixture::new();
    let source = f.source("gb.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let verified = VerifiedSelectedDat::lookup(None, &source, &f.hashes).unwrap();
    let mut base = strong_base();
    base.representation_match = Some(RepresentationMatchOutcome::NormalizedOnly {
        verdict: AuditVerdict::Exact {
            game_name: "Existing".into(),
            rom_name: "existing.gb".into(),
            algorithm: "SHA-256",
        },
    });
    let result = fuse(&base, &[verified]);
    assert_eq!(result.content, base.content);
    assert_eq!(result.representation_match, base.representation_match);
    assert_eq!(result.set_identity, base.set_identity);
    assert_eq!(fuse(&base, &[]), base);
}

#[test]
fn selected_dat_fusion_cannot_mask_a_preexisting_content_conflict() {
    let f = Fixture::new();
    let source = f.source("gb.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let mut input = IdentityInspectionInput::default();
    for signature in ["Nintendo Game Boy logo", "GBA cartridge header"] {
        input.content_evidence.push(ContentEvidence::new(
            ContentEvidenceKind::BootStructure,
            signature,
            ContentEvidenceConfidence::Strong,
            "original conflicting content",
        ));
    }
    let base = inspect_identity(input);
    assert_eq!(
        base.content.outcome,
        archivefs_core::platform_evidence_fusion::FusionOutcome::Conflict
    );
    let mut report = f.report();
    report.identity_result = base.clone();
    report.identity = present_identity(&base);
    assert!(apply(&mut report, f.payload(&source), &f.path));
    assert_eq!(report.identity.status, IdentityStatus::Conflict);
    assert_eq!(report.identity_result.content, base.content);
}

#[test]
fn selected_dat_fusion_same_path_refresh_accepts_only_the_newest_generation() {
    let f = Fixture::new();
    let source = f.source("gb.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let mut report = f.report();
    assert!(!apply_selected_evidence_enrichment_bound(
        &mut report,
        f.payload(&source),
        Some(&f.path),
        9,
        8
    ));
    assert!(apply_selected_evidence_enrichment_bound(
        &mut report,
        f.payload(&source),
        Some(&f.path),
        9,
        9
    ));
    let accepted = report.identity_result.clone();
    assert!(!apply_selected_evidence_enrichment_bound(
        &mut report,
        f.payload(&source),
        Some(&f.path),
        9,
        8
    ));
    assert_eq!(report.identity_result, accepted);
}

#[test]
fn selected_dat_fusion_only_attributes_a_match_to_sources_that_verified_the_bytes() {
    let f = Fixture::new();
    let a = f.source("a.dat", "Nintendo - Game Boy", &f.hashes.sha1);
    let b = f.source(
        "b.dat",
        "Nintendo - Nintendo Entertainment System",
        &"0".repeat(40),
    );
    let mut report = f.report();
    let payload = enrichment_from_hashes(f.hashes.clone(), &[(None, &a), (None, &b)]);
    assert!(apply(&mut report, payload, &f.path));
    assert_eq!(report.dat_identity.as_ref().unwrap().sources.len(), 1);
    let NoIntroLookupResult::Matched { system_name, .. } = &report.no_intro else {
        panic!("matched")
    };
    assert_eq!(system_name, "Nintendo - Game Boy");
    assert_eq!(
        report.identity_result.dat.as_ref().unwrap().platform(),
        Some("Game Boy")
    );
}
