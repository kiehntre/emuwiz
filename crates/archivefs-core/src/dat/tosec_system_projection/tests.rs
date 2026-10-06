use std::collections::BTreeMap;

use sha1::{Digest, Sha1};

use super::*;
use crate::dat::coverage_expectations::{
    CoverageRationale, CoverageSourceRole, ExpectedAuthoritativeSource,
    PlatformCoverageExpectation, TOSEC_PRIMARY_PLATFORMS, expected_authoritative_coverage,
};
use crate::dat::model::{ChecksumAlgorithm, DatEcosystem};
use crate::dat::tosec_release_pack::{TosecFriendlyCategory, classify_tosec_catalogue_name};
use crate::identity_source::tosec::{import_tosec_dat, observations_from_tosec_matches};
use crate::ingestion::content_registry::content_kind_for_extension;
use crate::platform_evidence_fusion::evidence_lineage::{ClaimType, Representation};

const TOSEC: ExpectedAuthoritativeSource = ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec);

#[test]
fn reviewed_systems_project_to_their_canonical_platform_exactly() {
    for (platform, systems) in rows() {
        for system in systems {
            assert_eq!(
                platform_for_tosec_system(system),
                PlatformForTosecSystem::Platform(platform),
                "{system}"
            );
        }
        assert_eq!(
            tosec_systems_for_platform(Some(platform)),
            TosecSystemsForPlatform::Systems(systems)
        );
    }
}

#[test]
fn platform_aliases_project_like_the_canonical_platform() {
    for (alias, canonical) in [
        ("c64", "Commodore 64"),
        ("commodorec64", "Commodore 64"),
        ("c128", "Commodore 128"),
        ("vic20", "VIC-20"),
        ("bbc", "BBC Micro"),
        ("elk", "Acorn Electron"),
        ("cpc", "Amstrad CPC"),
        ("atarist", "AtariST"),
        ("spectrum", "ZX Spectrum"),
        ("apple2", "Apple II"),
        ("atari800", "Atari 8-bit"),
    ] {
        assert_eq!(
            tosec_systems_for_platform(Some(alias)),
            tosec_systems_for_platform(Some(canonical)),
            "{alias}"
        );
        assert_ne!(
            tosec_systems_for_platform(Some(alias)),
            TosecSystemsForPlatform::NotMapped,
            "{alias}"
        );
        assert_eq!(
            expected_authoritative_coverage(Some(alias)),
            expected_authoritative_coverage(Some(canonical)),
            "{alias}"
        );
    }
}

#[test]
fn unknown_neighbouring_and_decorated_system_text_fails_closed() {
    for text in [
        "",
        "Acorn",
        "Commodore",
        "Commodore C64DTV",
        "commodore c64", // exact, case-sensitive
        "Commodore C16, C116 & Plus-4",
        "Sinclair ZX81",
        "Sinclair ZX80",
        "Acorn Atom",
        "Acorn Archimedes",
        "Atari ST - Compilations", // a classifier quirk string, not a system
        "Amstrad PCW",
        "Apple III",
        "MSX MSX",
        "MSX MSX2",
        "IBM PC Compatibles",
        "TOSEC - Commodore C64 - Games - [D64]",
        "Commodore C64 - Games - [D64] (TOSEC-v2025-02-16_CM)",
    ] {
        assert_eq!(
            platform_for_tosec_system(text),
            PlatformForTosecSystem::Unknown,
            "{text:?}"
        );
    }
}

#[test]
fn surrounding_whitespace_only_is_tolerated() {
    // Real pack folders carry a trailing space (`Commodore VIC20 `); that is
    // whitespace, not a different system.
    assert_eq!(
        platform_for_tosec_system(" Commodore VIC20 "),
        PlatformForTosecSystem::Platform("VIC-20")
    );
}

#[test]
fn a_system_name_never_maps_to_two_platforms_or_to_a_neighbour() {
    let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
    for (platform, systems) in rows() {
        for system in systems {
            assert!(
                owner.insert(system, platform).is_none(),
                "{system} is claimed by two platforms"
            );
        }
    }
    // BBC Micro and Acorn Electron stay explicit and separate.
    assert_eq!(owner["Acorn BBC"], "BBC Micro");
    assert_eq!(owner["Acorn Electron"], "Acorn Electron");
    assert_eq!(owner["Commodore C64"], "Commodore 64");
    assert_eq!(owner["Commodore C128"], "Commodore 128");
    assert!(!owner.contains_key("Commodore C64DTV"));
}

#[test]
fn only_platforms_that_expect_tosec_have_a_projection() {
    for platform in [
        "NES",
        "Game Boy",
        "PSX",
        "Arcade",
        "ScummVM",
        "MSX",
        "DOS",
        "PC",
        "Macintosh",
    ] {
        assert_eq!(
            tosec_systems_for_platform(Some(platform)),
            TosecSystemsForPlatform::NotMapped,
            "{platform}"
        );
    }
    assert_eq!(
        tosec_systems_for_platform(None),
        TosecSystemsForPlatform::NotMapped
    );
    assert_eq!(
        tosec_systems_for_platform(Some("not-a-platform")),
        TosecSystemsForPlatform::NotMapped
    );
}

#[test]
fn the_projection_and_the_coverage_expectation_stay_in_lockstep() {
    for platform in TOSEC_PRIMARY_PLATFORMS {
        assert!(
            rows().any(|(row, _)| row == *platform),
            "{platform} expects TOSEC but has no system projection"
        );
        match expected_authoritative_coverage(Some(platform)) {
            PlatformCoverageExpectation::ExpectedAuthoritativeSource { source, .. } => {
                assert_eq!(source.source, TOSEC, "{platform}");
                assert_eq!(source.role, CoverageSourceRole::Primary);
                assert_eq!(
                    source.rationale,
                    CoverageRationale::ExplicitTosecClassicMediaSupport
                );
            }
            other => panic!("{platform}: {other:?}"),
        }
    }
    for (platform, _) in rows() {
        let expects_tosec = match expected_authoritative_coverage(Some(platform)) {
            PlatformCoverageExpectation::ExpectedAuthoritativeSource { source, .. } => {
                source.source == TOSEC
            }
            PlatformCoverageExpectation::MultipleCandidateSources { sources, .. } => {
                sources.iter().any(|source| source.source == TOSEC)
            }
            _ => false,
        };
        assert!(
            expects_tosec,
            "{platform} has a projection but no TOSEC expectation"
        );
    }
}

/// Real catalogue-name shapes from the TOSEC pack, with the exact system the
/// existing release-pack classifier projects for them.
#[test]
fn real_pack_catalogue_names_project_through_the_existing_classifier() {
    for (catalogue, platform) in [
        (
            "Acorn BBC - Games - [SSD] (TOSEC-v2025-01-15_CM)",
            "BBC Micro",
        ),
        (
            "Acorn Electron - Games - [UEF] (TOSEC-v2022-06-08_CM)",
            "Acorn Electron",
        ),
        (
            "Amstrad CPC - Games - [DSK] (TOSEC-v2025-01-15_CM)",
            "Amstrad CPC",
        ),
        (
            "Commodore C64 - Games - [D64] (TOSEC-v2025-02-16_CM)",
            "Commodore 64",
        ),
        (
            "Commodore C128 - Games - [D71] (TOSEC-v2025-01-15_CM)",
            "Commodore 128",
        ),
        (
            "Commodore VIC20 - Games - [PRG] - Singlepart (TOSEC-v2025-01-15_CM)",
            "VIC-20",
        ),
        ("Atari ST - Games - [IPF] (TOSEC-v2023-11-07_CM)", "AtariST"),
        (
            "Atari 8bit - Games - [ATR] (TOSEC-v2025-01-15_CM)",
            "Atari 8-bit",
        ),
        (
            "Sinclair ZX Spectrum - Games - [TAP] (TOSEC-v2023-06-10_CM)",
            "ZX Spectrum",
        ),
        (
            "Apple II - Games - [WOZ] (TOSEC-v2024-07-03_CM)",
            "Apple II",
        ),
        (
            "Commodore Amiga - Games - [ADF] (TOSEC-v2025-01-15_CM)",
            "Amiga",
        ),
    ] {
        let classification = classify_tosec_catalogue_name(catalogue);
        assert_eq!(
            classification.category,
            TosecFriendlyCategory::Games,
            "{catalogue}"
        );
        assert_eq!(
            platform_for_tosec_system(&classification.system),
            PlatformForTosecSystem::Platform(platform),
            "{catalogue} -> {:?}",
            classification.system
        );
    }
}

/// TOSEC text never creates authority: neither a catalogue name, a pack folder
/// name, nor a bare ecosystem word is a platform.
#[test]
fn tosec_text_and_folder_names_never_create_coverage_authority() {
    for text in [
        "TOSEC",
        "Commodore C64 - Games - [D64] (TOSEC-v2025-02-16_CM)",
        "TOSEC - Commodore C64",
        "tosec-main",
        "TOSEC - DAT Pack - Complete (4743) (TOSEC-v2025-03-13)",
        "Commodore C64DTV",
        "Acorn",
    ] {
        assert!(
            matches!(
                expected_authoritative_coverage(Some(text)),
                PlatformCoverageExpectation::UnsupportedOrUnknown { .. }
            ),
            "{text:?}"
        );
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Production-path proof per newly mapped platform: a representative TOSEC
/// media extension is ingestible content, the real TOSEC importer reads a DAT
/// naming that media, and the *exact bytes* (not the name) produce an exact
/// match through the shared hash lookup.  The DAT's own name is only a label.
#[test]
fn each_reviewed_platform_has_ingestible_media_and_an_exact_hash_path() {
    for (platform, catalogue, extension) in [
        ("BBC Micro", "Acorn BBC - Games - [SSD]", "ssd"),
        ("Acorn Electron", "Acorn Electron - Games - [SSD]", "ssd"),
        ("Amstrad CPC", "Amstrad CPC - Games - [DSK]", "dsk"),
        ("Commodore 64", "Commodore C64 - Games - [D64]", "d64"),
        ("Commodore 128", "Commodore C128 - Games - [D71]", "d71"),
        ("VIC-20", "Commodore VIC20 - Games - [D64]", "d64"),
        ("AtariST", "Atari ST - Games - [ST]", "st"),
        ("Atari 8-bit", "Atari 8bit - Games - [ATR]", "atr"),
        ("ZX Spectrum", "Sinclair ZX Spectrum - Games - [TZX]", "tzx"),
        ("Apple II", "Apple II - Games - [WOZ]", "woz"),
    ] {
        assert!(
            content_kind_for_extension(extension).is_some(),
            "{platform}: .{extension} is not ingestible content"
        );
        let classification = classify_tosec_catalogue_name(catalogue);
        assert_eq!(
            platform_for_tosec_system(&classification.system),
            PlatformForTosecSystem::Platform(platform)
        );

        let delivered = format!("delivered bytes for {platform}").into_bytes();
        let sha = sha1_hex(&delivered);
        let dat = format!(
            r#"<?xml version="1.0"?>
<datafile><header><name>{catalogue}</name><description>{catalogue} (TOSEC-v2025-01-01)</description><version>2025-01-01</version><author>CRSV - Cassiel</author><homepage>TOSEC</homepage></header><game name="Example Game (1987)(Example Soft)"><rom name="example.{extension}" size="{}" sha1="{sha}"/></game></datafile>"#,
            delivered.len()
        );
        let dir = tempfile::tempdir().unwrap();
        // The file name deliberately says nothing about TOSEC or a platform.
        let path = dir.path().join("catalogue.dat");
        std::fs::write(&path, dat).unwrap();
        let source = import_tosec_dat(&path).unwrap();

        let exact = observations_from_tosec_matches(
            &source,
            ChecksumAlgorithm::Sha1,
            &sha,
            Representation::PhysicalFile,
        );
        assert!(
            exact
                .iter()
                .any(|observation| observation.claim == ClaimType::ExactBytesMatch),
            "{platform}: exact bytes must match"
        );
        // A different file of the same platform and name proves nothing.
        let other = sha1_hex(b"different bytes");
        assert!(
            observations_from_tosec_matches(
                &source,
                ChecksumAlgorithm::Sha1,
                &other,
                Representation::PhysicalFile
            )
            .is_empty(),
            "{platform}: no filename/category fallback exists"
        );
    }
}

/// Read-only probe of a real TOSEC release pack, run by hand:
/// `EMUWIZ_TOSEC_PACK_PROBE=<pack root> cargo test -p archivefs-core --lib
/// tosec_pack_probe -- --ignored --nocapture`.  Reads file *names* only; no
/// DAT is opened, hashed, imported or activated.
#[test]
#[ignore]
fn tosec_pack_probe() {
    use std::collections::BTreeSet;
    let Ok(root) = std::env::var("EMUWIZ_TOSEC_PACK_PROBE") else {
        return;
    };
    let mut dats = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(&root)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("dat") {
                let relative = path.strip_prefix(&root).unwrap().to_path_buf();
                let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                dats.push((relative, name));
            }
        }
    }
    dats.sort();
    eprintln!("PROBE total_dats={}", dats.len());
    for (platform, systems) in rows() {
        let mut matched = 0;
        let mut games = 0;
        let mut keys = BTreeSet::new();
        let mut media: BTreeMap<String, usize> = BTreeMap::new();
        let mut categories: BTreeMap<String, usize> = BTreeMap::new();
        let mut unrecognised = Vec::new();
        for (relative, name) in &dats {
            let top = relative
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            let c = classify_tosec_catalogue_name(name);
            if systems.contains(&c.system.as_str()) {
                matched += 1;
                if c.category == TosecFriendlyCategory::Games {
                    games += 1;
                }
                keys.insert((c.system.clone(), c.category, c.media));
                *media.entry(c.media.label().to_string()).or_default() += 1;
                *categories
                    .entry(c.category.label().to_string())
                    .or_default() += 1;
            } else if systems.iter().any(|s| name.starts_with(&format!("{s} - "))) {
                unrecognised.push(format!("[{top}] {name} => system {:?}", c.system));
            }
        }
        eprintln!(
            "PROBE {platform}: systems={systems:?} dats={matched} groups={} games_dats={games} media={media:?} categories={categories:?} unrecognised_names={}",
            keys.len(),
            unrecognised.len()
        );
        for line in unrecognised.iter().take(3) {
            eprintln!("PROBE   e.g. {line}");
        }
    }
}
