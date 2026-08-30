use super::reconcile::{WhdloadDatContext, WhdloadSlaveMatch, reconcile_whdload_slaves};
use super::slave::SlaveArtifact;
use super::{
    exact_slave_match_observation, inspect_whdload_slave_file, parse_whdload_slave,
    structural_slave_observation,
};
use crate::dat::index::DatIndex;
use crate::dat::model::{
    DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource, ParsedDat,
};
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimType, EvidenceChannel, LineageRelation, Representation, SourceArtifactIdentity,
    SourceFamily,
};
use std::path::Path;
use tempfile::tempdir;

// --- DAT reconciliation fixtures ---------------------------------------

/// A one-DAT hash index mapping each `(sha1 -> game name)` pair to a
/// single-ROM game entry. The reused generic [`DatIndex`], never an
/// Amiga-specific catalogue.
fn dat_index(entries: &[(&str, &str)]) -> DatIndex {
    let games = entries
        .iter()
        .map(|(sha1, game)| DatGameEntry {
            name: (*game).to_string(),
            roms: vec![DatRomEntry {
                name: format!("{game}.slave"),
                sha1: Some((*sha1).to_string()),
                ..Default::default()
            }],
            ..Default::default()
        })
        .collect();
    let dat = ParsedDat {
        source: DatSource {
            format: DatFormat::ClrMamePro,
            ecosystem: DatEcosystem::GenericClrMamePro,
            file_path: "whdload.dat".into(),
            name: Some("Commodore - Amiga - WHDLoad".into()),
            description: None,
            version: Some("2024-01-01".into()),
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: entries.len(),
            rom_count: entries.len(),
            parse_warnings: Vec::new(),
            packing_policy: DatPackingPolicy::Standard,
        },
        games,
    };
    DatIndex::build(&dat)
}

fn retroplay_context<'a>(
    index: &'a DatIndex,
    artifact: &'a SourceArtifactIdentity,
) -> WhdloadDatContext<'a> {
    WhdloadDatContext {
        index,
        source_family: SourceFamily::Retroplay,
        upstream_version: Some("2024-01-01"),
        source_artifact: Some(artifact),
    }
}

fn dat_artifact() -> SourceArtifactIdentity {
    SourceArtifactIdentity {
        source_family: SourceFamily::Retroplay,
        upstream_version: Some("2024-01-01".to_string()),
        artifact_sha256: Some("d".repeat(64)),
        artifact_name: Some("Commodore - Amiga - WHDLoad (2024-01-01).dat".to_string()),
    }
}

fn slave_artifact(dir: &Path, name: &str, version: u16) -> SlaveArtifact {
    inspect_whdload_slave_file(&write(dir, name, &slave(version))).unwrap()
}

fn put16(v: &mut [u8], at: usize, n: u16) {
    v[at..at + 2].copy_from_slice(&n.to_be_bytes());
}
fn put32(v: &mut [u8], at: usize, n: u32) {
    v[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn slave(version: u16) -> Vec<u8> {
    let size: usize = match version {
        1..=3 => 30,
        4..=7 => 32,
        8..=9 => 36,
        10..=15 => 42,
        16 => 50,
        17..=19 => 52,
        20 => 54,
        _ => 54,
    };
    let mut code = vec![0; (size + 64).next_multiple_of(4)];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    put16(&mut code, 12, version);
    put16(&mut code, 14, 3);
    put32(&mut code, 16, 524288);
    put32(&mut code, 20, 1);
    put32(&mut code, 24, 2);
    if size >= 30 {
        put16(&mut code, 28, 0);
    }
    if size >= 36 {
        put16(&mut code, 34, size as u16);
    }
    if size >= 42 {
        put16(&mut code, 36, (size + 8) as u16);
        put16(&mut code, 38, (size + 16) as u16);
        put16(&mut code, 40, (size + 24) as u16);
    }
    if size >= 50 {
        put32(&mut code, 42, 512 * 1024);
        put16(&mut code, 46, 0x1234);
        put16(&mut code, 48, (size + 32) as u16);
    }
    code[size..size + 5].copy_from_slice(b"Game\0");
    code[size + 8..size + 13].copy_from_slice(b"Copy\0");
    code[size + 16..size + 21].copy_from_slice(b"Info\0");
    code[size + 24..size + 29].copy_from_slice(b"Kick\0");
    code[size + 32..size + 39].copy_from_slice(b"Config\0");
    let mut out = Vec::new();
    for n in [
        0x3f3_u32,
        0,
        1,
        0,
        0,
        (code.len() / 4) as u32,
        0x3e9,
        (code.len() / 4) as u32,
    ] {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&code);
    out.extend_from_slice(&0x3f2_u32.to_be_bytes());
    out
}
fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}
#[test]
fn valid_supported_slave_parses() {
    let p = parse_whdload_slave(&slave(20)).unwrap();
    assert_eq!(p.runtime_version, 20);
    assert_eq!(p.name.as_deref(), Some("Game"));
}
#[test]
fn every_size_boundary_is_supported() {
    for v in [1, 3, 4, 7, 8, 9, 10, 15, 16, 17, 19, 20] {
        assert_eq!(parse_whdload_slave(&slave(v)).unwrap().runtime_version, v)
    }
}
#[test]
fn future_version_fails_closed() {
    assert!(parse_whdload_slave(&slave(21)).is_err())
}
#[test]
fn hunk_header_required() {
    let mut b = slave(20);
    b[..4].copy_from_slice(&0_u32.to_be_bytes());
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn security_and_id_required() {
    let mut b = slave(20);
    b[32] = 0;
    assert!(parse_whdload_slave(&b).is_err());
    let mut b = slave(20);
    b[36] = 0;
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn truncated_and_bad_hunk_fail() {
    assert!(parse_whdload_slave(&[0; 3]).is_err());
    let mut b = slave(20);
    b.truncate(36);
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn rptr_bounds_and_unterminated_fail() {
    let mut b = slave(8);
    put16(&mut b, 32 + 34, 0xffff);
    assert!(parse_whdload_slave(&b).is_err());
    let mut b = slave(8);
    let start = 32 + 36;
    for x in &mut b[start..] {
        *x = b'A'
    }
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn kickstart_metadata_v16_plus() {
    let p = parse_whdload_slave(&slave(16)).unwrap();
    assert_eq!(p.kick_name.as_deref(), Some("Kick"));
    assert_eq!(p.kick_size, Some(512 * 1024));
    assert_eq!(p.kick_crc, Some(0x1234));
    assert_eq!(p.config.as_deref(), Some("Config"));
}
#[test]
fn artifact_hashes_and_evidence_are_whole_slave_only() {
    let d = tempdir().unwrap();
    let a = inspect_whdload_slave_file(&write(d.path(), "Anything_1649.hdf", &slave(20))).unwrap();
    assert_eq!(a.hashes.sha1.len(), 40);
    assert_eq!(a.hashes.sha256.len(), 64);
    let structural = structural_slave_observation(&a);
    assert_eq!(structural.platform_candidate.as_deref(), Some("Amiga"));
    assert_eq!(
        structural.provenance.representation,
        Representation::WHDLoadSlave
    );
    assert_eq!(structural.provenance.channel, EvidenceChannel::LocalWHDLoad);
    assert_eq!(structural.provenance.upstream_source, SourceFamily::WHDLoad);
    assert_eq!(structural.provenance.lineage, LineageRelation::Independent);
    assert_eq!(structural.provenance.upstream_version, None);
    let exact = exact_slave_match_observation(&a, Some("Game".into()), None);
    assert_eq!(exact.claim, ClaimType::ExactSlaveMatch);
}
#[test]
fn filename_never_changes_identity() {
    let d = tempdir().unwrap();
    let a = inspect_whdload_slave_file(&write(d.path(), "GoldenAxe_v1.4_0017.hdf", &slave(20)))
        .unwrap();
    assert!(
        structural_slave_observation(&a)
            .notes
            .unwrap()
            .contains("runtime")
    );
    assert_eq!(
        exact_slave_match_observation(&a, None, None).release_candidate,
        None
    )
}

// --- DAT reconciliation (identity_source::whdload::reconcile) -----------

#[test]
fn parsed_slave_plus_exact_dat_hash_yields_exact_identity() {
    let d = tempdir().unwrap();
    let slave = slave_artifact(d.path(), "Superfrog_v1.2_0801.slave", 20);
    let index = dat_index(&[(&slave.hashes.sha1, "Superfrog (1993)(Team17)")]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(std::slice::from_ref(&slave), Some(&ctx));

    assert!(!result.ambiguous);
    assert_eq!(result.slaves.len(), 1);
    assert_eq!(result.slaves[0].outcome, WhdloadSlaveMatch::Exact);
    assert_eq!(
        result.slaves[0].matched_release.as_deref(),
        Some("Superfrog (1993)(Team17)")
    );
    assert_eq!(result.agreed_release(), Some("Superfrog (1993)(Team17)"));

    let exact = result
        .observations
        .iter()
        .find(|o| o.claim == ClaimType::ExactSlaveMatch)
        .expect("an exact-slave-match observation");
    assert_eq!(
        exact.release_candidate.as_deref(),
        Some("Superfrog (1993)(Team17)")
    );
    // Attributed to the DAT it came from, carrying the hash actually used.
    assert_eq!(exact.provenance.channel, EvidenceChannel::LocalDat);
    assert_eq!(exact.provenance.upstream_source, SourceFamily::Retroplay);
    assert_eq!(
        exact.provenance.upstream_version.as_deref(),
        Some("2024-01-01")
    );
    assert_eq!(
        exact.hash_or_value.as_deref(),
        Some(slave.hashes.sha1.as_str())
    );
    // The structural observation is still present alongside it.
    assert!(
        result
            .observations
            .iter()
            .any(|o| o.claim == ClaimType::PlatformCandidate
                && o.platform_candidate.as_deref() == Some("Amiga"))
    );
}

#[test]
fn valid_slave_with_no_dat_match_is_structural_only() {
    let d = tempdir().unwrap();
    let slave = slave_artifact(d.path(), "Unknown_0001.slave", 20);
    let index = dat_index(&[("0".repeat(40).as_str(), "Some Other Game")]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(std::slice::from_ref(&slave), Some(&ctx));

    assert!(!result.ambiguous);
    assert_eq!(result.slaves[0].outcome, WhdloadSlaveMatch::StructuralOnly);
    assert_eq!(result.slaves[0].matched_release, None);
    assert_eq!(result.agreed_release(), None);
    assert!(
        result
            .observations
            .iter()
            .all(|o| o.claim != ClaimType::ExactSlaveMatch)
    );
    assert!(
        result
            .observations
            .iter()
            .any(|o| o.platform_candidate.as_deref() == Some("Amiga"))
    );
}

#[test]
fn no_dat_context_preserves_structural_evidence_only() {
    let d = tempdir().unwrap();
    let slave = slave_artifact(d.path(), "Whatever_1649.slave", 16);

    let result = reconcile_whdload_slaves(std::slice::from_ref(&slave), None);

    assert!(!result.ambiguous);
    assert_eq!(result.slaves[0].outcome, WhdloadSlaveMatch::StructuralOnly);
    assert_eq!(result.observations.len(), 1);
    assert_eq!(result.observations[0].claim, ClaimType::PlatformCandidate);
}

#[test]
fn malformed_slave_is_refused_by_the_parser_before_reconciliation() {
    // The reconciler only ever receives already-parsed artifacts; a
    // truncated slave never becomes one.
    let mut bytes = slave(20);
    bytes.truncate(40);
    assert!(parse_whdload_slave(&bytes).is_err());
    let d = tempdir().unwrap();
    assert!(inspect_whdload_slave_file(&write(d.path(), "broken.slave", &bytes)).is_err());
}

#[test]
fn a_filename_that_looks_like_a_version_or_aga_never_becomes_identity() {
    let d = tempdir().unwrap();
    // Valid slave bytes, misleading name.
    let slave = slave_artifact(d.path(), "GoldenAxe_v1.4_AGA_0017.slave", 20);
    let index = dat_index(&[("f".repeat(40).as_str(), "Unrelated")]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(std::slice::from_ref(&slave), Some(&ctx));

    assert_eq!(result.slaves[0].outcome, WhdloadSlaveMatch::StructuralOnly);
    assert_eq!(result.slaves[0].matched_release, None);
    let structural = result
        .observations
        .iter()
        .find(|o| o.claim == ClaimType::PlatformCandidate)
        .unwrap();
    assert_eq!(structural.release_candidate, None);
    assert_eq!(structural.platform_candidate.as_deref(), Some("Amiga"));
    assert!(
        result
            .observations
            .iter()
            .all(|o| o.claim != ClaimType::ExactSlaveMatch)
    );
}

#[test]
fn two_slaves_matching_the_same_release_do_not_conflict() {
    let d = tempdir().unwrap();
    let a = slave_artifact(d.path(), "Turrican2.slave", 20);
    let b = slave_artifact(d.path(), "Turrican2_alt.slave", 16);
    assert_ne!(a.hashes.sha1, b.hashes.sha1);
    let index = dat_index(&[
        (&a.hashes.sha1, "Turrican II (1991)(Rainbow Arts)"),
        (&b.hashes.sha1, "Turrican II (1991)(Rainbow Arts)"),
    ]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(&[a, b], Some(&ctx));

    assert!(
        !result.ambiguous,
        "same release from two slaves is corroboration, not conflict"
    );
    assert_eq!(
        result.agreed_release(),
        Some("Turrican II (1991)(Rainbow Arts)")
    );
    assert_eq!(
        result
            .observations
            .iter()
            .filter(|o| o.claim == ClaimType::ExactSlaveMatch)
            .count(),
        2,
        "each slave contributes its own corroborating exact observation"
    );
}

#[test]
fn two_slaves_matching_conflicting_releases_are_ambiguous() {
    let d = tempdir().unwrap();
    let a = slave_artifact(d.path(), "GameA.slave", 20);
    let b = slave_artifact(d.path(), "GameB.slave", 16);
    let index = dat_index(&[
        (&a.hashes.sha1, "Superfrog"),
        (&b.hashes.sha1, "Pinball Dreams"),
    ]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(&[a, b], Some(&ctx));

    assert!(result.ambiguous);
    assert_eq!(result.agreed_release(), None);
    // Both exact observations are kept for review, not silently dropped.
    assert_eq!(
        result
            .observations
            .iter()
            .filter(|o| o.claim == ClaimType::ExactSlaveMatch)
            .count(),
        2
    );
}

#[test]
fn one_slave_matching_multiple_distinct_releases_is_ambiguous_and_asserts_nothing() {
    let d = tempdir().unwrap();
    let slave = slave_artifact(d.path(), "Colliding.slave", 20);
    // Same SHA-1 recorded by the DAT under two different games (a real
    // collision the shared index preserves).
    let index = dat_index(&[
        (&slave.hashes.sha1, "Release One"),
        (&slave.hashes.sha1, "Release Two"),
    ]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(std::slice::from_ref(&slave), Some(&ctx));

    assert!(result.ambiguous);
    assert_eq!(result.slaves[0].outcome, WhdloadSlaveMatch::ExactAmbiguous);
    assert!(
        result
            .observations
            .iter()
            .all(|o| o.claim != ClaimType::ExactSlaveMatch),
        "a slave that hits two releases asserts no exact identity"
    );
}

#[test]
fn an_unmatched_extra_slave_never_erases_a_valid_exact_match() {
    let d = tempdir().unwrap();
    let matched = slave_artifact(d.path(), "Main.slave", 20);
    let extra = slave_artifact(d.path(), "Loader.slave", 16);
    let index = dat_index(&[(&matched.hashes.sha1, "Ruff 'n' Tumble")]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(&[matched, extra], Some(&ctx));

    assert!(!result.ambiguous);
    assert_eq!(result.agreed_release(), Some("Ruff 'n' Tumble"));
    assert_eq!(result.slaves[0].outcome, WhdloadSlaveMatch::Exact);
    assert_eq!(result.slaves[1].outcome, WhdloadSlaveMatch::StructuralOnly);
}

#[test]
fn matched_dat_source_and_revision_provenance_is_retained() {
    let d = tempdir().unwrap();
    let slave = slave_artifact(d.path(), "Zool.slave", 20);
    let index = dat_index(&[(&slave.hashes.sha1, "Zool")]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let result = reconcile_whdload_slaves(std::slice::from_ref(&slave), Some(&ctx));
    let exact = result
        .observations
        .iter()
        .find(|o| o.claim == ClaimType::ExactSlaveMatch)
        .unwrap();

    assert_eq!(
        exact.provenance.upstream_version.as_deref(),
        Some("2024-01-01")
    );
    let source_artifact = exact
        .provenance
        .source_artifact
        .as_ref()
        .expect("the matched DAT artifact identity is retained");
    assert_eq!(source_artifact.source_family, SourceFamily::Retroplay);
    assert_eq!(
        source_artifact.artifact_name.as_deref(),
        Some("Commodore - Amiga - WHDLoad (2024-01-01).dat")
    );
    assert_eq!(exact.provenance.lineage, LineageRelation::Independent);
    assert_eq!(
        exact.provenance.representation,
        Representation::WHDLoadSlave
    );
}

#[test]
fn exact_slave_match_observation_has_a_real_production_path_through_discovery() {
    // `ingestion::discover_source_with_whdload_dat` -> `discover_whdload_folder`
    // -> `reconcile_whdload_slaves` -> `exact_slave_match_observation`.
    let d = tempdir().unwrap();
    let install = d.path().join("Superfrog");
    std::fs::create_dir_all(&install).unwrap();
    let slave_path = write(&install, "Superfrog.slave", &slave(20));
    let inspected = inspect_whdload_slave_file(&slave_path).unwrap();
    let index = dat_index(&[(&inspected.hashes.sha1, "Superfrog (1993)(Team17)")]);
    let artifact = dat_artifact();
    let ctx = retroplay_context(&index, &artifact);

    let report = crate::ingestion::discover_source_with_whdload_dat(d.path(), Some(&ctx)).unwrap();
    let item = report
        .items
        .iter()
        .find(|item| item.content == Some(crate::ingestion::ContentKind::WhdloadInstall))
        .expect("the WHDLoad install is discovered");

    assert_eq!(
        item.validation_state,
        crate::ingestion::ValidationState::Accepted
    );
    // Platform hint comes from verified slave structure, not the folder name.
    assert_eq!(item.platform_hint.as_deref(), Some("Amiga"));
    // The exact catalogue identity reached the user-visible explanation.
    assert!(
        item.explanation
            .contains("Exact catalogue match: Superfrog (1993)(Team17)."),
        "{}",
        item.explanation
    );
}

#[test]
fn discovery_without_a_dat_context_still_gives_structural_amiga_evidence() {
    let d = tempdir().unwrap();
    let install = d.path().join("Some Game_v2.0_AGA");
    std::fs::create_dir_all(&install).unwrap();
    write(&install, "Game.slave", &slave(20));

    let report = crate::ingestion::discover_source(d.path()).unwrap();
    let item = report
        .items
        .iter()
        .find(|item| item.content == Some(crate::ingestion::ContentKind::WhdloadInstall))
        .unwrap();

    assert_eq!(
        item.validation_state,
        crate::ingestion::ValidationState::Accepted
    );
    assert_eq!(item.platform_hint.as_deref(), Some("Amiga"));
    assert!(!item.explanation.contains("Exact catalogue match"));
}
