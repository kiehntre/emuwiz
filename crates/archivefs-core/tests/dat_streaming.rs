//! Synthetic fixtures only; the large stress case is opt-in.
archivefs_core::install_test_environment!();

use std::io::Write;
use std::path::Path;

use archivefs_core::Database;
use archivefs_core::dat::classification::classify_catalogue;
use archivefs_core::dat::expected_inventory::{
    ExpectedDatEntryRecord, project_expected_dat_inventory,
};
use archivefs_core::dat::index::DatIndex;
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::model::ParsedDat;
use archivefs_core::dat::parser::ParseError;
use archivefs_core::dat::parsers::{parse_dat_file, visit_dat_file_raw};
use archivefs_core::dat::sources::validation::validate_dat_source;
use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind};

fn source(path: &Path, kind: DatSourceKind) -> DatSourceEntry {
    DatSourceEntry::new("stream".into(), "stream".into(), path.into(), kind)
}

fn generate(path: &Path, n: usize, family: &str, tail: &str) {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let (root, game) = match family {
        "mame" => ("mame", "machine"),
        "software" => ("softwarelist", "software"),
        _ => ("datafile", "game"),
    };
    if family == "text" {
        writeln!(out, "clrmamepro (\n name \"TOSEC\"\n)").unwrap();
    } else {
        writeln!(out, "<?xml version=\"1.0\"?><{root}>").unwrap();
    }
    for i in 0..n {
        // Deliberately repeat hashes and names while preserving source order.
        let name = format!("日本語 Café (Europe) (v1.0) {}", i / 2);
        if family == "text" {
            writeln!(out, "game (\n name \"{name}\"\n cloneof \"parent\"\n rom ( name \"r{i}\" size 4 crc DEADBEEF )\n)").unwrap();
        } else {
            writeln!(out, "<{game} name=\"{name}\" cloneof=\"parent\" romof=\"bios\"><description>Título {i}</description><rom name=\"r{i}\" size=\"4\" crc=\"DEADBEEF\" sha1=\"1111111111111111111111111111111111111111\"/></{game}>").unwrap();
        }
    }
    write!(out, "{tail}").unwrap();
    if tail.is_empty() && family != "text" {
        // Metadata after records is intentional; streaming must report final
        // headers, and collected classification must still use those headers.
        if root == "datafile" {
            writeln!(
                out,
                "<header><name>No-Intro</name><version>1</version></header>"
            )
            .unwrap();
        }
        writeln!(out, "</{root}>").unwrap();
    }
}

fn assert_parity(path: &Path, limits: DatLimits) {
    let collected = parse_dat_file(path, limits).unwrap();
    let mut games = Vec::new();
    let summary = visit_dat_file_raw(path, limits, &mut |game| {
        games.push(game);
        Ok(())
    })
    .unwrap();
    assert_eq!(summary.warnings, collected.warnings);
    let mut streamed = ParsedDat {
        source: summary.source,
        games,
    };
    classify_catalogue(&mut streamed);
    assert_eq!(
        serde_json::to_value(&streamed).unwrap(),
        serde_json::to_value(&collected.dat).unwrap()
    );
    let (report, projected) = validate_dat_source(&source(path, DatSourceKind::File), limits);
    assert_eq!(report.entry_count, collected.dat.source.entry_count);
    assert_eq!(report.rom_count, collected.dat.source.rom_count);
    assert_eq!(
        projected,
        project_expected_dat_inventory(&collected.dat.games)
    );
    let left = DatIndex::build(&collected.dat);
    let right = DatIndex::build(&streamed);
    assert_eq!(
        left.lookup_crc32("deadbeef"),
        right.lookup_crc32("deadbeef")
    );
}

#[test]
fn tiny_and_thousands_match_collected_semantics_for_all_formats() {
    let dir = tempfile::tempdir().unwrap();
    for family in ["logiqx", "mame", "software", "text"] {
        for n in [0, 1, 3000] {
            let path = dir.path().join(format!("{family}-{n}.dat"));
            generate(&path, n, family, "");
            assert_parity(&path, DatLimits::default());
        }
    }
}

#[test]
fn software_parts_and_parent_metadata_survive_streaming() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("parts.xml");
    std::fs::write(&path, r#"<softwarelist name="list"><software name="parent"></software><software name="clone" cloneof="parent" supported="partial"><part name="cart" interface="slot"><dataarea name="prg"><rom name="r" crc="DEADBEEF"/></dataarea><diskarea name="disk"><disk name="d" sha1="1111111111111111111111111111111111111111"/></diskarea></part></software></softwarelist>"#).unwrap();
    assert_parity(&path, DatLimits::default());
}

#[test]
fn unicode_and_valid_fields_cross_reader_buffers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("long.dat");
    let description = "長い題名 &amp; Café ".repeat(6000);
    std::fs::write(&path, format!("<datafile><game name=\"{}\"><description>{description}</description><rom name=\"r\" crc=\"DEADBEEF\"/></game></datafile>", "界".repeat(2000))).unwrap();
    assert_parity(
        &path,
        DatLimits::builder()
            .max_description_length(256 * 1024)
            .build(),
    );
}

#[test]
fn malformed_tail_is_seen_after_incremental_delivery_and_projection_is_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.dat");
    generate(
        &path,
        4000,
        "logiqx",
        "<game name=\"broken\"><rom></game></datafile>",
    );
    let mut visited = 0;
    assert!(
        visit_dat_file_raw(&path, DatLimits::default(), &mut |_| {
            visited += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(visited, 4000);
    let (report, projected) =
        validate_dat_source(&source(&path, DatSourceKind::File), DatLimits::default());
    assert!(!report.files[0].outcome.is_parsed());
    assert!(projected.entries.is_empty());
    assert_eq!(projected.duplicate_names_skipped, 0);
}

#[test]
fn failed_folder_file_restores_duplicate_tracking_for_next_file() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("a.dat");
    let bad = dir.path().join("b.dat");
    let next = dir.path().join("c.dat");
    generate(&good, 2, "logiqx", "");
    generate(&bad, 6, "logiqx", "<broken></datafile>");
    generate(&next, 4, "logiqx", "");
    let (_, result) = validate_dat_source(
        &source(dir.path(), DatSourceKind::Folder),
        DatLimits::default(),
    );
    let mut expected = project_expected_dat_inventory(
        &parse_dat_file(&good, DatLimits::default())
            .unwrap()
            .dat
            .games,
    );
    expected.extend_from(
        &parse_dat_file(&next, DatLimits::default())
            .unwrap()
            .dat
            .games,
    );
    assert_eq!(result, expected);
}

#[test]
fn truncated_xml_and_text_keep_existing_error_and_warning_contracts() {
    let dir = tempfile::tempdir().unwrap();
    for (family, tail) in [
        ("logiqx", "<game name=\"tail\">"),
        ("mame", "<machine name=\"tail\">"),
        ("software", "<software name=\"tail\">"),
        ("text", "game (\n name \"tail\"\n"),
    ] {
        let path = dir.path().join(format!("{family}.dat"));
        generate(&path, 10, family, tail);
        let old = parse_dat_file(&path, DatLimits::default());
        let new = visit_dat_file_raw(&path, DatLimits::default(), &mut |_| Ok(()));
        match (old, new) {
            (Ok(old), Ok(new)) => {
                assert_eq!(old.warnings, new.warnings);
                assert_eq!(
                    serde_json::to_value(old.dat.source).unwrap(),
                    serde_json::to_value(new.source).unwrap()
                );
            }
            (Err(old), Err(new)) => assert_eq!(old.to_string(), new.to_string()),
            _ => panic!("truncation changed semantics for {family}"),
        }
    }
}

#[test]
fn visitor_cancellation_stops_before_malformed_tail_in_each_parser() {
    let dir = tempfile::tempdir().unwrap();
    for family in ["logiqx", "mame", "software", "text"] {
        let path = dir.path().join(format!("{family}.dat"));
        generate(&path, 3000, family, "<malformed");
        let mut count = 0;
        let result = visit_dat_file_raw(&path, DatLimits::default(), &mut |_| {
            count += 1;
            if count == 37 {
                return Err(ParseError::Io {
                    path: path.clone(),
                    error: std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"),
                });
            }
            Ok(())
        });
        assert_eq!(count, 37);
        assert!(
            matches!(result, Err(ParseError::Io { error, .. }) if error.kind() == std::io::ErrorKind::Interrupted)
        );
    }
}

#[test]
fn streaming_enforces_entry_and_field_limits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("limits.dat");
    for family in ["logiqx", "mame", "software", "text"] {
        generate(&path, 20, family, "");
        let limits = DatLimits::builder().max_entries(7).build();
        let old = parse_dat_file(&path, limits).unwrap_err();
        let new = visit_dat_file_raw(&path, limits, &mut |_| Ok(())).unwrap_err();
        assert_eq!(old.to_string(), new.to_string());
    }
    std::fs::write(
        &path,
        format!(
            "<datafile><game name=\"{}\"></game></datafile>",
            "x".repeat(100_000)
        ),
    )
    .unwrap();
    assert!(visit_dat_file_raw(&path, DatLimits::default(), &mut |_| Ok(())).is_err());
    std::fs::write(
        &path,
        format!(
            "<datafile><game name=\"x\"><description>{}</description></game></datafile>",
            "x".repeat(100_000)
        ),
    )
    .unwrap();
    // Logiqx descriptions are truncated with a diagnostic rather than refused.
    assert_parity(&path, DatLimits::default());
    let summary = visit_dat_file_raw(&path, DatLimits::default(), &mut |game| {
        assert_eq!(game.description.unwrap().len(), 65_536);
        Ok(())
    })
    .unwrap();
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.code == "game_description_truncated")
    );
    std::fs::write(
        &path,
        format!("clrmamepro (\n name \"TOSEC\"\n)\n{}", "x".repeat(200_000)),
    )
    .unwrap();
    assert!(visit_dat_file_raw(&path, DatLimits::default(), &mut |_| Ok(())).is_err());
}

#[test]
fn inventory_write_failure_rolls_back_rows_and_source_metadata_then_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::open_or_create(dir.path().join("db.sqlite")).unwrap();
    let row = |name: &str| ExpectedDatEntryRecord {
        canonical_identity: name.into(),
        display_name: name.into(),
        dat_game_id: None,
        rom_count: 1,
    };
    db.replace_expected_dat_inventory("stream", Some("old"), None, &[row("old")], 3)
        .unwrap();
    let before = db.expected_dat_inventory_meta("stream").unwrap();
    // Duplicate primary key fails after the delete, metadata update and first insert.
    assert!(
        db.replace_expected_dat_inventory(
            "stream",
            Some("new"),
            None,
            &[row("new"), row("new")],
            0
        )
        .is_err()
    );
    assert_eq!(
        db.expected_dat_canonical_identities("stream").unwrap(),
        ["old".to_string()].into_iter().collect()
    );
    assert_eq!(db.expected_dat_inventory_meta("stream").unwrap(), before);
    db.replace_expected_dat_inventory("stream", Some("recovered"), None, &[row("recovered")], 0)
        .unwrap();
    assert_eq!(
        db.expected_dat_canonical_identities("stream").unwrap(),
        ["recovered".to_string()].into_iter().collect()
    );
}

#[test]
#[ignore = "opt-in generated 100k stress test"]
fn streaming_100k_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stress.dat");
    generate(&path, 100_000, "logiqx", "");
    let mut count = 0;
    let summary = visit_dat_file_raw(&path, DatLimits::default(), &mut |_| {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 100_000);
    assert_eq!(summary.source.entry_count, count);
    assert_eq!(summary.source.rom_count, count);
}

#[test]
fn validation_is_wired_to_the_incremental_visitor() {
    // This is an intentional architecture guard: output-only parity tests would
    // also pass if validation regressed to collecting ParsedDat before projecting.
    let validation = include_str!("../src/dat/sources/validation.rs");
    assert!(validation.contains("visit_dat_file_raw(path"));
    assert!(!validation.contains("parse_dat_file(path"));
}
