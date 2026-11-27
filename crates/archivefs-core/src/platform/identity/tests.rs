use std::path::{Path, PathBuf};

use crate::dat::audit::{AuditEntry, AuditReport, AuditSummary, AuditVerdict};
use crate::dat::sources::audit_run::DatAuditOutcome;
use crate::identity_source::model::{
    ExternalIdentityRecord, ExternalVerification, IdentityProvider,
};
use crate::identity_source::{
    cache::{CACHE_FORMAT_VERSION, IdentityCache},
    romm::normalise::NormalisedPlatform,
};

use super::*;

const GENERATION: u64 = 7;

fn inferred(platform: &str) -> PlatformIdentityEvidence {
    PlatformIdentityEvidence::canonical(
        platform,
        PlatformIdentitySource::Inference,
        PlatformIdentityConfidence::Inferred,
        GENERATION,
        "existing lower-confidence inference",
    )
    .unwrap()
}

fn strong(platform: &str) -> PlatformIdentityEvidence {
    PlatformIdentityEvidence::canonical(
        platform,
        PlatformIdentitySource::ExistingStrongIdentity,
        PlatformIdentityConfidence::Strong,
        GENERATION,
        "existing structured identity",
    )
    .unwrap()
}

fn romm_record(
    platform: Option<&str>,
    verification: ExternalVerification,
) -> ExternalIdentityRecord {
    ExternalIdentityRecord {
        provider: IdentityProvider::Romm,
        server_id: "https://romm.example".to_string(),
        provider_platform_id: Some("4".to_string()),
        provider_game_id: "42".to_string(),
        provider_file_id: None,
        provider_path: "roms/psp/Game.cso".to_string(),
        archivefs_path: Some(PathBuf::from("/library/Game.cso")),
        title: Some("Game".to_string()),
        platform_candidate: platform.map(str::to_string),
        provider_platform_name: Some("psp".to_string()),
        regions: Vec::new(),
        revision: None,
        hashes: Vec::new(),
        file_size_bytes: Some(123),
        metadata_provider_ids: Vec::new(),
        artwork: None,
        related_files: Vec::new(),
        sibling_game_ids: Vec::new(),
        imported_at_unix_seconds: 1,
        provider_updated_at: None,
        verification,
        conflicts: Vec::new(),
        evidence: Vec::new(),
        synopsis: None,
        genres: Vec::new(),
        players: None,
        rating: None,
        release_year: None,
        howlongtobeat: None,
    }
}

fn romm(platform: &str) -> PlatformIdentityEvidence {
    PlatformIdentityEvidence::from_romm(
        &romm_record(Some(platform), ExternalVerification::StrongExternal),
        GENERATION,
    )
    .unwrap()
}

fn dat_outcome(platform: Option<&str>, verdict: AuditVerdict) -> DatAuditOutcome {
    DatAuditOutcome {
        population: Default::default(),
        source_id: "dat-1".to_string(),
        source_display_name: "No-Intro PSP".to_string(),
        dat_path: "/catalogues/psp.dat".to_string(),
        scan_root: "/library".to_string(),
        // The real header of the assigned system's catalogue, so evidence
        // takes the normal path where header and assignment agree.
        catalogue_names: vec![
            match platform {
                Some("PSX") => "Sony - PlayStation",
                Some("PSP") => "Sony - PlayStation Portable",
                _ => "Fixture catalogue",
            }
            .to_string(),
        ],
        catalogue_entries: 1,
        catalogue_roms: 1,
        catalogue_version: None,
        catalogue_sha256: None,
        catalogue_inventory: None,
        catalogue_author: None,
        catalogue_homepage: None,
        catalogue_ecosystem: Some(crate::dat::model::DatEcosystem::Redump),
        catalogue_variant: None,
        unreadable_catalogues: Vec::new(),
        report: AuditReport {
            entries: vec![AuditEntry {
                local_path: "/library/Game.cso".to_string(),
                local_filename: "Game.cso".to_string(),
                verdict,
            }],
            summary: AuditSummary::default(),
        },
        evidence_sources: Vec::new(),
        archives: Vec::new(),
        sets: Vec::new(),
        unhashed: Vec::new(),
        files_scanned: 1,
        bytes_hashed: 123,
        archive_bytes_hashed: 0,
        truncated: false,
        policy: None,
        content: Default::default(),
        platform: platform.map(str::to_string),
        cache: Default::default(),
        known_hashes: Default::default(),
        loose_discs: Default::default(),
    }
}

fn dat(platform: &str) -> PlatformIdentityEvidence {
    PlatformIdentityEvidence::from_verified_dat(
        &dat_outcome(
            Some(platform),
            AuditVerdict::Exact {
                game_name: "Game".to_string(),
                rom_name: "Game.cso".to_string(),
                algorithm: "SHA-1",
            },
        ),
        Path::new("/library/Game.cso"),
        GENERATION,
    )
    .unwrap()
}

fn resolved(resolution: &PlatformIdentityResolution) -> (&str, &[PlatformIdentityEvidence]) {
    match resolution {
        PlatformIdentityResolution::Resolved {
            platform, evidence, ..
        } => (platform, evidence),
        other => panic!("expected resolved identity, got {other:?}"),
    }
}

#[test]
fn unknown_plus_recognised_romm_psp_resolves_to_canonical_psp() {
    let resolution = resolve_platform_identity(GENERATION, [romm("PSP")]);
    assert_eq!(resolved(&resolution).0, "PSP");
}

#[test]
fn unknown_plus_verified_dat_psp_resolves_to_canonical_psp() {
    let resolution = resolve_platform_identity(GENERATION, [dat("PSP")]);
    assert_eq!(resolved(&resolution).0, "PSP");
}

#[test]
fn agreeing_dat_and_romm_resolve_once_and_retain_both_sources() {
    let resolution = resolve_platform_identity(GENERATION, [dat("PSP"), romm("PSP")]);
    let (platform, evidence) = resolved(&resolution);
    assert_eq!(platform, "PSP");
    assert_eq!(evidence.len(), 2);
    assert!(
        evidence
            .iter()
            .any(|item| item.source == PlatformIdentitySource::Romm)
    );
    assert!(
        evidence
            .iter()
            .any(|item| item.source == PlatformIdentitySource::VerifiedDat)
    );
}

#[test]
fn a_verified_dat_platform_outranks_conflicting_romm_metadata() {
    // In either order: RomM is external metadata, the DAT verification is
    // EmuWiz's own exact evidence.
    for evidence in [[dat("PSX"), romm("PSP")], [romm("PSP"), dat("PSX")]] {
        let resolution = resolve_platform_identity(GENERATION, evidence);
        let (platform, evidence) = resolved(&resolution);
        assert_eq!(platform, "PSX");
        assert!(
            evidence
                .iter()
                .all(|item| item.source == PlatformIdentitySource::VerifiedDat),
            "the disagreeing RomM record is not part of the answer: {evidence:?}"
        );
    }
    // Manual still outranks both.
    let manual = PlatformIdentityEvidence::manual("PSP", GENERATION).unwrap();
    let resolution = resolve_platform_identity(GENERATION, [manual, dat("PSX"), romm("PSP")]);
    assert_eq!(resolution.platform(), Some("PSP"));
}

#[test]
fn without_a_dat_verification_conflicting_evidence_still_needs_review() {
    // Two DATs disagreeing: no current, unambiguous verification, so RomM
    // does not settle it either.
    let resolution = resolve_platform_identity(GENERATION, [dat("PSX"), dat("PSP"), romm("PSP")]);
    assert!(resolution.is_conflict());
    // A non-exact DAT verdict is not evidence at all, so RomM stands alone
    // and strong local evidence against it still needs review.
    let probable = PlatformIdentityEvidence::from_verified_dat(
        &dat_outcome(
            Some("PSX"),
            AuditVerdict::Probable {
                game_name: "Game".to_string(),
                rom_name: "Game.cso".to_string(),
            },
        ),
        Path::new("/library/Game.cso"),
        GENERATION,
    );
    assert!(probable.is_none());
    let resolution = resolve_platform_identity(GENERATION, [strong("PSX"), romm("PSP")]);
    assert!(resolution.is_conflict());
}

#[test]
fn existing_strong_psp_and_conflicting_romm_require_review() {
    let resolution = resolve_platform_identity(GENERATION, [strong("PSP"), romm("PSX")]);
    assert!(resolution.is_conflict());
    assert_eq!(resolution.platform(), None);
}

#[test]
fn existing_strong_psp_and_conflicting_dat_require_review() {
    let resolution = resolve_platform_identity(GENERATION, [strong("PSP"), dat("PSX")]);
    assert!(resolution.is_conflict());
    assert_eq!(resolution.platform(), None);
}

#[test]
fn existing_strong_psp_and_agreeing_romm_resolve_to_psp() {
    let resolution = resolve_platform_identity(GENERATION, [strong("PSP"), romm("PSP")]);
    assert_eq!(resolved(&resolution).0, "PSP");
    assert!(!resolution.is_conflict());
}

#[test]
fn existing_strong_psp_and_agreeing_dat_resolve_to_psp() {
    let resolution = resolve_platform_identity(GENERATION, [strong("PSP"), dat("PSP")]);
    assert_eq!(resolved(&resolution).0, "PSP");
    assert!(!resolution.is_conflict());
}

#[test]
fn strong_identity_and_authoritative_resolution_are_order_independent() {
    for provider in [romm("PSX"), dat("PSX")] {
        let forward = resolve_platform_identity(GENERATION, [strong("PSP"), provider.clone()]);
        let reverse = resolve_platform_identity(GENERATION, [provider, strong("PSP")]);
        assert_eq!(forward, reverse);
        assert!(forward.is_conflict());
    }
}

#[test]
fn conflict_is_independent_of_input_order_and_timing() {
    let forward = resolve_platform_identity(GENERATION, [dat("PSX"), romm("PSP")]);
    let reverse = resolve_platform_identity(GENERATION, [romm("PSP"), dat("PSX")]);
    assert_eq!(forward, reverse);
}

#[test]
fn manual_psp_wins_over_conflicting_romm() {
    let resolution = resolve_platform_identity(
        GENERATION,
        [
            PlatformIdentityEvidence::manual("PSP", GENERATION).unwrap(),
            romm("PSX"),
        ],
    );
    let (platform, evidence) = resolved(&resolution);
    assert_eq!(platform, "PSP");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].source, PlatformIdentitySource::Manual);
}

#[test]
fn manual_psp_wins_over_conflicting_dat() {
    let resolution = resolve_platform_identity(
        GENERATION,
        [
            dat("PSX"),
            PlatformIdentityEvidence::manual("PSP", GENERATION).unwrap(),
        ],
    );
    assert_eq!(resolved(&resolution).0, "PSP");
}

#[test]
fn unrecognised_romm_platform_remains_unknown() {
    let record = romm_record(None, ExternalVerification::StrongExternal);
    assert!(PlatformIdentityEvidence::from_romm(&record, GENERATION).is_none());
    assert_eq!(
        resolve_platform_identity(GENERATION, std::iter::empty()),
        PlatformIdentityResolution::Unknown {
            generation: GENERATION
        }
    );
}

#[test]
fn aliases_normalise_through_the_existing_registry() {
    let evidence = PlatformIdentityEvidence::canonical(
        "sony-playstation-portable",
        PlatformIdentitySource::Romm,
        PlatformIdentityConfidence::High,
        GENERATION,
        "alias",
    )
    .unwrap();
    assert_eq!(evidence.platform, "PSP");
}

#[test]
fn substring_near_misses_are_not_platform_evidence() {
    for value in ["psp-clone", "notpsp", "portable-psp-backup"] {
        assert!(
            PlatformIdentityEvidence::canonical(
                value,
                PlatformIdentitySource::Romm,
                PlatformIdentityConfidence::High,
                GENERATION,
                "provider text",
            )
            .is_none(),
            "{value} must not be guessed into PSP"
        );
    }
}

#[test]
fn stale_provider_evidence_cannot_overwrite_current_manual_identity() {
    let mut stale_romm = romm("PSX");
    stale_romm.generation = GENERATION - 1;
    let resolution = resolve_platform_identity(
        GENERATION,
        [
            stale_romm,
            PlatformIdentityEvidence::manual("PSP", GENERATION).unwrap(),
        ],
    );
    assert_eq!(resolved(&resolution).0, "PSP");
}

#[test]
fn romm_provenance_and_confidence_are_honest() {
    let resolution = resolve_platform_identity(GENERATION, [romm("PSP")]);
    let (_, evidence) = resolved(&resolution);
    assert_eq!(evidence[0].source.label(), "RomM");
    assert_eq!(evidence[0].confidence, PlatformIdentityConfidence::High);
}

#[test]
fn dat_provenance_requires_a_cryptographic_exact_verdict() {
    let probable = dat_outcome(
        Some("PSP"),
        AuditVerdict::Probable {
            game_name: "Game".to_string(),
            rom_name: "Game.cso".to_string(),
        },
    );
    assert!(
        PlatformIdentityEvidence::from_verified_dat(
            &probable,
            Path::new("/library/Game.cso"),
            GENERATION,
        )
        .is_none()
    );
    let verified = dat("PSP");
    assert_eq!(verified.source, PlatformIdentitySource::VerifiedDat);
    assert_eq!(verified.confidence, PlatformIdentityConfidence::Verified);
}

#[test]
fn existing_known_platform_is_not_downgraded_by_absent_or_stale_evidence() {
    let mut stale = romm("PSX");
    stale.generation -= 1;
    let resolution = resolve_platform_identity(GENERATION, [strong("PSP"), stale]);
    assert_eq!(resolved(&resolution).0, "PSP");
}

#[test]
fn stronger_provider_evidence_enriches_a_lower_confidence_inference() {
    let resolution = resolve_platform_identity(GENERATION, [inferred("PSX"), romm("PSP")]);
    assert_eq!(resolved(&resolution).0, "PSP");
}

#[test]
fn enrichment_performs_no_filesystem_mutation() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-platform-identity-{}-{}",
        std::process::id(),
        GENERATION
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let rom_path = root.join("Game.cso");
    std::fs::write(&rom_path, b"unchanged bytes").unwrap();
    let before = std::fs::read(&rom_path).unwrap();

    let resolution = resolve_platform_identity(GENERATION, [dat("PSP"), romm("PSP")]);

    assert_eq!(resolved(&resolution).0, "PSP");
    assert_eq!(std::fs::read(&rom_path).unwrap(), before);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn future_romm_directory_slug_comes_from_imported_canonical_mapping() {
    let cache = IdentityCache {
        format_version: CACHE_FORMAT_VERSION,
        provider: IdentityProvider::Romm,
        server_id: "https://romm.example".to_string(),
        server_version: None,
        source_fingerprint: "fixture".to_string(),
        imported_at_unix_seconds: 1,
        platforms: vec![
            NormalisedPlatform {
                provider_platform_id: Some("4".to_string()),
                provider_slug: "psp".to_string(),
                provider_name: Some("PlayStation Portable".to_string()),
                canonical: Some("PSP".to_string()),
                rom_count: Some(1),
            },
            NormalisedPlatform {
                provider_platform_id: Some("5".to_string()),
                provider_slug: "mystery".to_string(),
                provider_name: Some("Mystery".to_string()),
                canonical: None,
                rom_count: Some(1),
            },
        ],
        records: Vec::new(),
        rejected_hashes: Vec::new(),
        unknown_platforms: vec!["mystery".to_string()],
        server_reported_total: Some(0),
    };
    assert_eq!(cache.romm_slug_for_platform("PSP"), Some("psp"));
    assert_eq!(cache.romm_slug_for_platform("not-a-platform"), None);
}

/// Builds a minimal identity cache whose `platforms` list is exactly the
/// given `(provider_slug, canonical)` pairs, for exercising
/// `romm_slug_for_platform`'s ambiguity handling directly.
fn cache_with_platforms(platforms: Vec<(&str, Option<&str>)>) -> IdentityCache {
    IdentityCache {
        format_version: CACHE_FORMAT_VERSION,
        provider: IdentityProvider::Romm,
        server_id: "https://romm.example".to_string(),
        server_version: None,
        source_fingerprint: "fixture".to_string(),
        imported_at_unix_seconds: 1,
        platforms: platforms
            .into_iter()
            .enumerate()
            .map(|(index, (slug, canonical))| NormalisedPlatform {
                provider_platform_id: Some(index.to_string()),
                provider_slug: slug.to_string(),
                provider_name: None,
                canonical: canonical.map(str::to_string),
                rom_count: Some(1),
            })
            .collect(),
        records: Vec::new(),
        rejected_hashes: Vec::new(),
        unknown_platforms: Vec::new(),
        server_reported_total: Some(0),
    }
}

#[test]
fn romm_slug_for_platform_fails_closed_on_ambiguous_mapping() {
    // A single distinct slug resolves.
    let cache = cache_with_platforms(vec![("nes", Some("NES"))]);
    assert_eq!(cache.romm_slug_for_platform("NES"), Some("nes"));

    // Duplicate identical entries collapse to one and still resolve.
    let cache = cache_with_platforms(vec![("nes", Some("NES")), ("nes", Some("NES"))]);
    assert_eq!(cache.romm_slug_for_platform("NES"), Some("nes"));

    // Two distinct slugs for one canonical platform are ambiguous.
    let cache = cache_with_platforms(vec![("fds", Some("NES")), ("nes", Some("NES"))]);
    assert_eq!(cache.romm_slug_for_platform("NES"), None);

    // Zero matching slugs is unresolved.
    let cache = cache_with_platforms(vec![("psp", Some("PSP"))]);
    assert_eq!(cache.romm_slug_for_platform("NES"), None);
}

#[test]
fn a_cue_sheet_match_is_platform_evidence_only_when_its_whole_disc_is_verified() {
    let mut outcome = dat_outcome(
        Some("PSX"),
        AuditVerdict::Exact {
            game_name: "Game".to_string(),
            rom_name: "Game.cue".to_string(),
            algorithm: "SHA-1",
        },
    );
    outcome.report.entries[0].local_path = "/library/Game.cue".to_string();
    let cue = Path::new("/library/Game.cue");
    assert!(PlatformIdentityEvidence::from_verified_dat(&outcome, cue, GENERATION).is_none());

    let mut disc = crate::dat::sources::audit_run::LooseDiscAudit {
        cue_path: "/library/Game.cue".to_string(),
        members: Vec::new(),
        verified_game: None,
        detail: Some("a track did not match".into()),
    };
    outcome.loose_discs = vec![disc.clone()];
    assert!(PlatformIdentityEvidence::from_verified_dat(&outcome, cue, GENERATION).is_none());

    disc.verified_game = Some("Game".into());
    disc.detail = None;
    outcome.loose_discs = vec![disc];
    let evidence = PlatformIdentityEvidence::from_verified_dat(&outcome, cue, GENERATION).unwrap();
    assert_eq!(evidence.source, PlatformIdentitySource::VerifiedDat);
}

/// Owner policy 2026-11-26: a system is established only by one exact
/// SHA-256/SHA-1 match from a catalogue whose header does not contradict
/// the system it was assigned.
#[test]
fn only_a_single_strong_match_from_an_agreeing_catalogue_is_system_evidence() {
    let path = Path::new("/library/Game.cso");
    let exact = |algorithm| AuditVerdict::Exact {
        game_name: "Game".to_string(),
        rom_name: "Game.cso".to_string(),
        algorithm,
    };
    let evidence = |outcome: &DatAuditOutcome| {
        PlatformIdentityEvidence::from_verified_dat(outcome, path, GENERATION)
    };
    assert!(evidence(&dat_outcome(Some("PSP"), exact("SHA-1"))).is_some());
    assert!(evidence(&dat_outcome(Some("PSP"), exact("SHA-256/SHA-1"))).is_some());
    assert!(evidence(&dat_outcome(Some("PSP"), exact("MD5"))).is_none());
    assert!(evidence(&dat_outcome(Some("PSP"), exact("SHA-1/MD5"))).is_none());
    let several = AuditVerdict::ExactMultipleCandidates {
        algorithm: "SHA-1",
        count: 2,
        game_names: vec!["Game".into(), "Game (Rev 1)".into()],
    };
    assert!(evidence(&dat_outcome(Some("PSP"), several)).is_none());
    // A header naming another system contradicts the assignment.
    let mut contradicted = dat_outcome(Some("PSP"), exact("SHA-1"));
    contradicted.catalogue_names = vec!["Nintendo - Game Boy Advance".to_string()];
    assert!(evidence(&contradicted).is_none());
    // A header naming no system: the assignment alone decides.
    let mut unnamed = dat_outcome(Some("PSP"), exact("SHA-1"));
    unnamed.catalogue_names = vec!["Fixture catalogue".to_string()];
    assert!(evidence(&unnamed).is_some());
    // No assigned system: nothing to establish.
    assert!(evidence(&dat_outcome(None, exact("SHA-1"))).is_none());
}
