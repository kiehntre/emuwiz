//! S11-08: `DatIndex::build` shares each game's classification and source
//! metadata across its ROM references instead of cloning them per ROM.
//! These tests prove the index answers every lookup identically to a naive
//! scan of the parsed catalogue, and measure build cost on a large synthetic
//! catalogue (the measurement test is `#[ignore]`; run it by name).

use std::collections::BTreeMap;

use super::*;
use crate::dat::classification::{DatContentClassification, DatOriginalMetadata};
use crate::dat::model::{
    DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource, ParsedDat,
};

fn hex(prefix: u64, width: usize) -> String {
    format!("{prefix:0width$x}")
}

/// A synthetic catalogue shaped like a large No-Intro/MAME DAT: `games`
/// games, `roms_per_game` ROMs each, every ROM with CRC32/MD5/SHA-1, every
/// game carrying a dozen metadata fields and a classification with evidence.
fn synthetic(games: usize, roms_per_game: usize) -> ParsedDat {
    let mut entries = Vec::with_capacity(games);
    for g in 0..games {
        let mut fields = BTreeMap::new();
        for f in 0..12 {
            fields.insert(
                format!("field{f}"),
                format!("value {g} {f} with some length"),
            );
        }
        let roms = (0..roms_per_game)
            .map(|r| {
                let id = (g * roms_per_game + r) as u64 + 1;
                DatRomEntry {
                    name: format!("game{g}_rom{r}.bin"),
                    size_bytes: Some(1024 + id),
                    crc32: Some(hex(id, 8)),
                    md5: Some(hex(id, 32)),
                    sha1: Some(hex(id, 40)),
                    ..Default::default()
                }
            })
            .collect();
        entries.push(DatGameEntry {
            name: format!("Game {g}"),
            roms,
            original_metadata: DatOriginalMetadata { fields },
            content_classification: DatContentClassification::unknown(),
            ..Default::default()
        });
    }
    ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: DatEcosystem::GenericLogiqx,
            file_path: "synthetic.dat".into(),
            name: Some("Synthetic".into()),
            description: None,
            version: None,
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: games,
            rom_count: games * roms_per_game,
            parse_warnings: Vec::new(),
            packing_policy: DatPackingPolicy::Standard,
        },
        games: entries,
    }
}

/// Every ROM is reachable through each of its hashes and its filename, the
/// returned reference carries exactly its game's metadata, and references
/// of one game share a single metadata allocation.
#[test]
fn shared_metadata_index_answers_every_lookup_like_the_catalogue() {
    let dat = synthetic(40, 5);
    let index = DatIndex::build(&dat);
    for (game_index, game) in dat.games.iter().enumerate() {
        let mut first: Option<SharedRomRef> = None;
        for (rom_index, rom) in game.roms.iter().enumerate() {
            let key = DatMemberKey {
                game_index,
                location: MemberLocation::TopLevel { rom_index },
            };
            for hits in [
                index.lookup_crc32(rom.crc32.as_deref().unwrap()),
                index.lookup_md5(rom.md5.as_deref().unwrap()),
                index.lookup_sha1(rom.sha1.as_deref().unwrap()),
                index.lookup_filename(&rom.name),
            ] {
                let hit = hits
                    .iter()
                    .find(|candidate| candidate.key() == key)
                    .unwrap_or_else(|| panic!("{} not found", rom.name));
                assert_eq!(hit.game_name, game.name);
                assert_eq!(hit.rom_name, rom.name);
                assert_eq!(hit.size_bytes, rom.size_bytes);
                assert_eq!(*hit.original_metadata, game.original_metadata);
                assert_eq!(*hit.content_classification, game.content_classification);
                match &first {
                    None => first = Some(hit.clone()),
                    Some(first) => {
                        assert!(std::sync::Arc::ptr_eq(
                            &first.original_metadata,
                            &hit.original_metadata
                        ));
                        assert!(std::sync::Arc::ptr_eq(
                            &first.content_classification,
                            &hit.content_classification
                        ));
                    }
                }
            }
        }
    }
}

/// Audit verdicts are a pure function of the lookups above, so sharing the
/// metadata cannot change them; this pins it for a representative sample.
#[test]
fn shared_metadata_index_yields_identical_audit_verdicts() {
    use crate::dat::audit::{AuditVerdict, KnownFileEvidence, audit_files};
    let dat = synthetic(200, 3);
    let index = DatIndex::build(&dat);
    let known: Vec<KnownFileEvidence> = dat
        .games
        .iter()
        .flat_map(|game| game.roms.iter())
        .map(|rom| {
            KnownFileEvidence::new(format!("/lib/{}", rom.name), &rom.name)
                .with_size(rom.size_bytes.unwrap())
                .with_sha1(rom.sha1.clone().unwrap())
                .with_md5(rom.md5.clone().unwrap())
        })
        .collect();
    let report = audit_files(&known, &index);
    assert_eq!(report.summary.exact, known.len());
    for (entry, rom) in report
        .entries
        .iter()
        .zip(dat.games.iter().flat_map(|game| game.roms.iter()))
    {
        assert!(
            matches!(&entry.verdict, AuditVerdict::Exact { rom_name, algorithm, .. }
                if rom_name == &rom.name && *algorithm == "SHA-1"),
            "{:?}",
            entry.verdict
        );
    }
}

/// Measurement, not an assertion of speed: builds a 500k-ROM catalogue and
/// reports build time and resident-set growth. Run with
/// `--ignored shared_metadata_index_build_measurement -- --nocapture`.
#[test]
#[ignore]
fn shared_metadata_index_build_measurement() {
    fn rss_kib() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status.lines().find_map(|line| {
                    line.strip_prefix("VmRSS:")
                        .and_then(|rest| rest.trim().split_whitespace().next())
                        .and_then(|value| value.parse().ok())
                })
            })
            .unwrap_or(0)
    }
    let dat = synthetic(100_000, 5);
    let before = rss_kib();
    let started = std::time::Instant::now();
    let index = DatIndex::build(&dat);
    let elapsed = started.elapsed();
    let after = rss_kib();
    eprintln!(
        "DatIndex::build: {} games, {} roms, {:?}, RSS +{} MiB (sha1 buckets {})",
        dat.games.len(),
        dat.source.rom_count,
        elapsed,
        after.saturating_sub(before) / 1024,
        index.by_sha1.len()
    );
}
