use std::path::Path;

use tempfile::tempdir;

use crate::dat::model::ChecksumAlgorithm;
use crate::platform_evidence_fusion::evidence_lineage::{
    AgreementStatus, ClaimStrength, ClaimType, EvidenceChannel, EvidenceObservation, IdentityScope,
    LineageRelation, Provenance, Representation, SourceFamily, hasheous_observation,
    merge_evidence, observation_declares_provenance,
};

use super::convert::{claim_for_representation, lookup_tosec, observations_from_tosec_matches};
use super::filename_metadata::parse_tosec_filename_metadata;
use super::import::{TosecImportError, import_tosec_dat};

const SHA_ONE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_TWO: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const MD5_ONE: &str = "11111111111111111111111111111111";

const CLASSIC_TOSEC_DAT: &str = r#"clrmamepro (
    name "TOSEC - Commodore 64 - Games - PRG"
    description "Classic computer preservation catalogue (TOSEC)"
    version "2025-01-01"
    author "TOSEC"
)
game (
    name "Example Game (1987)(Example Soft)(US)(En)(v1.2)(Rev 2) [cr][t +2][!]"
    rom ( name "example.prg" size 42 crc 1234ABCD md5 11111111111111111111111111111111 sha1 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa )
)
game (
    name "CRC Only (1988)(US)"
    rom ( name "crc.prg" size 7 crc ABCD1234 )
)
"#;

const TOSEC_LOGIQX: &str = r#"<?xml version="1.0"?>
<datafile><header><name>TOSEC - ZX Spectrum - Games - TZX</name><description>TOSEC test</description><author>TOSEC</author></header><game name="Example"><rom name="example.tzx" size="1" sha1="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"/></game></datafile>"#;

fn write_dat(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn imported_tosec() -> super::import::ImportedTosecSource {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "classic.dat", CLASSIC_TOSEC_DAT);
    import_tosec_dat(&path).unwrap()
}

fn exact(source: &super::import::ImportedTosecSource) -> EvidenceObservation {
    observations_from_tosec_matches(
        source,
        ChecksumAlgorithm::Sha1,
        SHA_ONE,
        Representation::PhysicalFile,
    )
    .into_iter()
    .find(|observation| observation.claim == ClaimType::ExactBytesMatch)
    .unwrap()
}

#[test]
fn classic_clrmamepro_tosec_import_succeeds() {
    let source = imported_tosec();
    assert_eq!(source.system_name, "TOSEC - Commodore 64 - Games - PRG");
    assert_eq!(source.entry_count, 2);
    assert_eq!(source.rom_count, 2);
}

#[test]
fn logiqx_tosec_internal_metadata_is_recognised() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "anything.dat", TOSEC_LOGIQX);
    let source = import_tosec_dat(&path).unwrap();
    assert_eq!(source.entry_count, 1);
}

#[test]
fn logiqx_tosec_detection_uses_description_when_needed() {
    let xml = r#"<datafile><header><name>ZX Spectrum</name><description>TOSEC catalogue</description></header><game name="G"><rom name="g" size="1" crc="12345678"/></game></datafile>"#;
    let dir = tempdir().unwrap();
    let source = import_tosec_dat(&write_dat(dir.path(), "not-tosec.dat", xml)).unwrap();
    assert_eq!(source.system_name, "ZX Spectrum");
}

#[test]
fn filename_with_tosec_but_generic_internal_header_is_refused() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "TOSEC - definitely.dat",
        "clrmamepro (\n name \"Commodore 64 Games\"\n)\ngame ( name \"G\" rom ( name \"g\" size 1 crc 12345678 ) )",
    );
    assert!(matches!(
        import_tosec_dat(&path),
        Err(TosecImportError::NotTosec { .. })
    ));
}

#[test]
fn non_tosec_no_intro_dat_is_refused() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "ordinary.dat",
        r#"<datafile><header><name>Nintendo - Game Boy</name><author>No-Intro</author></header></datafile>"#,
    );
    assert!(matches!(
        import_tosec_dat(&path),
        Err(TosecImportError::NotTosec { .. })
    ));
}

#[test]
fn tosec_iso_and_pix_are_deferred_from_classic_media_scope() {
    let dir = tempdir().unwrap();
    for (filename, internal_name) in [
        ("classic.dat", "TOSEC-ISO - PC"),
        ("iso.dat", "TOSEC-PIX - Manuals"),
    ] {
        let dat = format!(
            "clrmamepro (\n name \"{internal_name}\"\n)\ngame (\n name \"G\"\n rom ( name \"g\" size 1 crc 12345678 )\n)"
        );
        assert!(matches!(
            import_tosec_dat(&write_dat(dir.path(), filename, &dat)),
            Err(TosecImportError::OutOfScope { .. })
        ));
    }
}

#[test]
fn malformed_dat_is_a_clean_error() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "broken.dat",
        "<?xml version=\"1.0\"?><datafile><header><name>TOSEC - Broken</name></header><game name=\"G\"><rom size=\"1\"",
    );
    assert!(import_tosec_dat(&path).is_err());
}

#[test]
fn bom_prefixed_logiqx_tosec_imports() {
    let dir = tempdir().unwrap();
    let contents = format!("\u{feff}{TOSEC_LOGIQX}");
    let source = import_tosec_dat(&write_dat(dir.path(), "bom.dat", &contents)).unwrap();
    assert_eq!(source.entry_count, 1);
}

#[test]
fn internal_version_is_preserved() {
    assert_eq!(
        imported_tosec().upstream_version.as_deref(),
        Some("2025-01-01")
    );
}

#[test]
fn no_version_is_not_fabricated_from_artifact_filename() {
    let dir = tempdir().unwrap();
    let path = write_dat(
        dir.path(),
        "TOSEC - 2099.dat",
        "clrmamepro (\n name \"TOSEC - C64\"\n author \"TOSEC\"\n)\ngame ( name \"G\" rom ( name \"g\" size 1 crc 12345678 ) )",
    );
    assert_eq!(import_tosec_dat(&path).unwrap().upstream_version, None);
}

#[test]
fn artifact_hash_is_deterministic_and_sha256_shaped() {
    let dir = tempdir().unwrap();
    let path = write_dat(dir.path(), "classic.dat", CLASSIC_TOSEC_DAT);
    let one = import_tosec_dat(&path).unwrap();
    let two = import_tosec_dat(&path).unwrap();
    assert_eq!(one.artifact_sha256, two.artifact_sha256);
    assert_eq!(one.artifact_sha256.len(), 64);
}

#[test]
fn large_artifact_hash_is_recorded_without_affecting_entry_count() {
    let dir = tempdir().unwrap();
    let mut contents = CLASSIC_TOSEC_DAT.to_string();
    contents.push_str(&"\n".repeat(256 * 1024));
    let source = import_tosec_dat(&write_dat(dir.path(), "large.dat", &contents)).unwrap();
    assert_eq!(source.entry_count, 2);
    assert_eq!(source.artifact_sha256.len(), 64);
}

#[test]
fn artifact_provenance_includes_name_hash_version_and_counts() {
    let source = imported_tosec();
    let manifest = source.manifest_line();
    assert!(manifest.contains("2025-01-01"));
    assert!(manifest.contains(&source.artifact_sha256));
    assert!(manifest.contains("entries: 2"));
    assert!(manifest.contains("roms: 2"));
}

#[test]
fn sha1_lookup_is_exact_and_collision_preserving() {
    let source = imported_tosec();
    let hits = lookup_tosec(&source.index, ChecksumAlgorithm::Sha1, SHA_ONE);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].rom_name, "example.prg");
}

#[test]
fn md5_lookup_is_exact() {
    let source = imported_tosec();
    assert_eq!(
        lookup_tosec(&source.index, ChecksumAlgorithm::Md5, MD5_ONE).len(),
        1
    );
}

#[test]
fn crc_only_lookup_is_available() {
    let source = imported_tosec();
    assert_eq!(
        lookup_tosec(&source.index, ChecksumAlgorithm::Crc32, "abcd1234").len(),
        1
    );
}

#[test]
fn unknown_hash_is_a_neutral_empty_result() {
    let source = imported_tosec();
    assert!(lookup_tosec(&source.index, ChecksumAlgorithm::Sha1, SHA_TWO).is_empty());
    assert!(
        observations_from_tosec_matches(
            &source,
            ChecksumAlgorithm::Sha1,
            SHA_TWO,
            Representation::PhysicalFile,
        )
        .is_empty()
    );
}

#[test]
fn collisions_remain_multiple_candidates_never_first_match() {
    let dat = "clrmamepro (\n name \"TOSEC - C64\"\n)\ngame (\n name \"A\"\n rom ( name \"a\" size 1 crc deadbeef )\n)\ngame (\n name \"B\"\n rom ( name \"b\" size 2 crc deadbeef )\n)";
    let dir = tempdir().unwrap();
    let source = import_tosec_dat(&write_dat(dir.path(), "collisions.dat", dat)).unwrap();
    let hits = lookup_tosec(&source.index, ChecksumAlgorithm::Crc32, "deadbeef");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].game_name, "A");
    assert_eq!(hits[1].game_name, "B");
}

#[test]
fn sha1_and_md5_matches_are_strong() {
    let source = imported_tosec();
    assert_eq!(exact(&source).claim_strength, ClaimStrength::Strong);
    let md5 = observations_from_tosec_matches(
        &source,
        ChecksumAlgorithm::Md5,
        MD5_ONE,
        Representation::PhysicalFile,
    );
    assert_eq!(
        md5.iter()
            .find(|item| item.claim == ClaimType::ExactBytesMatch)
            .unwrap()
            .claim_strength,
        ClaimStrength::Strong
    );
}

#[test]
fn crc_only_match_is_corroborated_not_strong() {
    let source = imported_tosec();
    let observations = observations_from_tosec_matches(
        &source,
        ChecksumAlgorithm::Crc32,
        "abcd1234",
        Representation::PhysicalFile,
    );
    assert_eq!(
        observations
            .iter()
            .find(|item| item.claim == ClaimType::ExactBytesMatch)
            .unwrap()
            .claim_strength,
        ClaimStrength::Corroborated
    );
}

#[test]
fn caller_controlled_physical_representation_maps_to_exact_bytes() {
    assert_eq!(
        claim_for_representation(Representation::PhysicalFile),
        ClaimType::ExactBytesMatch
    );
}

#[test]
fn caller_controlled_normalized_representation_maps_to_exact_normalized() {
    assert_eq!(
        claim_for_representation(Representation::NormalizedRom),
        ClaimType::ExactNormalizedMatch
    );
}

#[test]
fn representation_is_not_guessed_from_a_tosec_member_name() {
    let source = imported_tosec();
    let observations = observations_from_tosec_matches(
        &source,
        ChecksumAlgorithm::Sha1,
        SHA_ONE,
        Representation::ArchiveMember,
    );
    let exactish = observations
        .iter()
        .find(|item| item.claim == ClaimType::PlatformCandidate)
        .unwrap();
    assert_eq!(
        exactish.provenance.representation,
        Representation::ArchiveMember
    );
    assert_ne!(exactish.claim, ClaimType::ExactBytesMatch);
}

#[test]
fn exact_observation_has_local_tosec_independent_provenance() {
    let source = imported_tosec();
    let observation = exact(&source);
    assert_eq!(observation.provenance.channel, EvidenceChannel::LocalTosec);
    assert_eq!(observation.provenance.upstream_source, SourceFamily::TOSEC);
    assert_eq!(observation.provenance.lineage, LineageRelation::Independent);
    assert!(observation_declares_provenance(&observation));
}

#[test]
fn exact_observation_carries_source_artifact() {
    let source = imported_tosec();
    let observation = exact(&source);
    assert_eq!(
        observation
            .provenance
            .source_artifact
            .unwrap()
            .artifact_sha256
            .as_deref(),
        Some(source.artifact_sha256.as_str())
    );
}

#[test]
fn classic_name_metadata_preserves_tokens_verbatim() {
    let metadata = parse_tosec_filename_metadata(
        "Example Game (1987)(Example Soft)(US)(En)(v1.2)(Rev 2) [cr][t +2][!]",
    );
    assert_eq!(metadata.title, "Example Game");
    assert_eq!(metadata.year.as_deref(), Some("1987"));
    assert_eq!(metadata.publisher.as_deref(), Some("Example Soft"));
    assert_eq!(metadata.countries, ["US"]);
    assert_eq!(metadata.languages, ["En"]);
    assert_eq!(metadata.version.as_deref(), Some("v1.2"));
    assert_eq!(metadata.revision.as_deref(), Some("Rev 2"));
    assert!(metadata.flags.cracked && metadata.flags.trainer && metadata.flags.verified_good);
}

#[test]
fn all_requested_dump_markers_are_recognised() {
    let flags = parse_tosec_filename_metadata("G [cr][t][h][a][f][m][p][b][o][u][v][!]").flags;
    assert!(flags.cracked && flags.trainer && flags.hacked && flags.alternate && flags.fixed);
    assert!(flags.modified && flags.pirated && flags.bad_dump && flags.overdump);
    assert!(flags.underdump && flags.virus && flags.verified_good);
}

#[test]
fn quality_markers_are_explicit_weak_dump_status_not_platform_evidence() {
    let dat = "clrmamepro (\n name \"TOSEC - C64\"\n)\ngame (\n name \"Broken (1987) [b][o][u][v]\"\n rom ( name \"g\" size 1 sha1 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa )\n)";
    let dir = tempdir().unwrap();
    let source = import_tosec_dat(&write_dat(dir.path(), "quality.dat", dat)).unwrap();
    let observations = observations_from_tosec_matches(
        &source,
        ChecksumAlgorithm::Sha1,
        SHA_ONE,
        Representation::PhysicalFile,
    );
    let quality: Vec<_> = observations
        .iter()
        .filter(|item| item.claim == ClaimType::VariantStatus)
        .collect();
    assert_eq!(quality.len(), 4);
    assert!(
        quality
            .iter()
            .all(|item| item.claim_strength == ClaimStrength::Weak)
    );
    assert!(
        quality
            .iter()
            .all(|item| item.claim != ClaimType::PlatformCandidate)
    );
}

#[test]
fn verified_good_is_separate_from_exact_hash_strength() {
    let source = imported_tosec();
    let observations = observations_from_tosec_matches(
        &source,
        ChecksumAlgorithm::Sha1,
        SHA_ONE,
        Representation::PhysicalFile,
    );
    assert_eq!(exact(&source).claim_strength, ClaimStrength::Strong);
    assert!(
        observations
            .iter()
            .any(|item| item.claim == ClaimType::VariantStatus
                && item.hash_or_value.as_deref() == Some("verified good")
                && item.claim_strength == ClaimStrength::Corroborated)
    );
}

#[test]
fn filename_metadata_is_only_emitted_after_a_hash_match() {
    let source = imported_tosec();
    let observations = observations_from_tosec_matches(
        &source,
        ChecksumAlgorithm::Sha1,
        SHA_TWO,
        Representation::PhysicalFile,
    );
    assert!(observations.is_empty());
}

#[test]
fn clean_and_cracked_dumps_share_game_label_but_keep_distinct_dump_hashes() {
    let dat = format!(
        "clrmamepro (\n name \"TOSEC - C64\"\n)\ngame (\n name \"Shared Game (1987)\"\n rom ( name \"clean\" size 1 sha1 {SHA_ONE} )\n)\ngame (\n name \"Shared Game (1987) [cr]\"\n rom ( name \"cracked\" size 1 sha1 {SHA_TWO} )\n)"
    );
    let dir = tempdir().unwrap();
    let source = import_tosec_dat(&write_dat(dir.path(), "variants.dat", &dat)).unwrap();
    let clean = exact_from_hash(&source, SHA_ONE);
    let cracked = exact_from_hash(&source, SHA_TWO);
    assert_eq!(clean.release_candidate, cracked.release_candidate);
    assert_ne!(clean.hash_or_value, cracked.hash_or_value);
    assert!(
        observations_from_tosec_matches(
            &source,
            ChecksumAlgorithm::Sha1,
            SHA_TWO,
            Representation::PhysicalFile
        )
        .iter()
        .any(|item| item.claim == ClaimType::VariantStatus
            && item.hash_or_value.as_deref() == Some("cracked"))
    );
}

fn exact_from_hash(source: &super::import::ImportedTosecSource, hash: &str) -> EvidenceObservation {
    observations_from_tosec_matches(
        source,
        ChecksumAlgorithm::Sha1,
        hash,
        Representation::PhysicalFile,
    )
    .into_iter()
    .find(|item| item.claim == ClaimType::ExactBytesMatch)
    .unwrap()
}

#[test]
fn local_and_hasheous_tosec_are_same_source_agreement() {
    let source = imported_tosec();
    let local = exact(&source);
    let relay = hasheous_observation(
        "TOSEC",
        Representation::PhysicalFile,
        ClaimType::ExactBytesMatch,
        Some(SHA_ONE.to_string()),
        None,
    );
    let summary = merge_evidence(&[local, relay]);
    assert_eq!(summary[0].status, AgreementStatus::SameSourceAgreement);
}

#[test]
fn local_tosec_and_local_structural_platform_observations_are_independent() {
    let source = imported_tosec();
    let local = observations_from_tosec_matches(
        &source,
        ChecksumAlgorithm::Sha1,
        SHA_ONE,
        Representation::PhysicalFile,
    )
    .into_iter()
    .find(|item| item.claim == ClaimType::PlatformCandidate)
    .unwrap();
    let structural = EvidenceObservation {
        provenance: Provenance {
            channel: EvidenceChannel::LocalStructural,
            upstream_source: SourceFamily::Unknown,
            upstream_version: None,
            source_artifact: None,
            imported_at_unix: None,
            retrieved_at_unix: None,
            generator_version: None,
            lineage: LineageRelation::Independent,
            representation: Representation::StructuralMetadata,
        },
        claim: ClaimType::PlatformCandidate,
        claim_strength: ClaimStrength::Strong,
        identity_scope: IdentityScope::PlatformIdentity,
        hash_or_value: None,
        platform_candidate: Some(source.system_name.clone()),
        release_candidate: None,
        notes: None,
    };
    assert_eq!(
        merge_evidence(&[local, structural])[0].status,
        AgreementStatus::IndependentAgreement
    );
}

#[test]
fn conflicting_strong_preservation_source_fails_closed() {
    let source = imported_tosec();
    let tosec = exact(&source);
    let conflicting_no_intro = hasheous_observation(
        "NoIntro",
        Representation::PhysicalFile,
        ClaimType::ExactBytesMatch,
        Some(SHA_TWO.to_string()),
        None,
    );
    assert_eq!(
        merge_evidence(&[tosec, conflicting_no_intro])[0].status,
        AgreementStatus::IndependentSourceConflict
    );
}

#[test]
fn representation_mismatch_is_not_implicitly_folded() {
    let source = imported_tosec();
    let tosec = exact(&source);
    let conflicting_representation = hasheous_observation(
        "NoIntro",
        Representation::NormalizedRom,
        ClaimType::ExactBytesMatch,
        Some(SHA_TWO.to_string()),
        None,
    );
    assert_eq!(
        merge_evidence(&[tosec, conflicting_representation])[0].status,
        AgreementStatus::RepresentationConflict
    );
}
