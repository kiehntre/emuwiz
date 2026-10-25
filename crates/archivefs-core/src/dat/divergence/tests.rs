use super::*;
use crate::dat::model::{DatFormat, DatGameEntry, DatPackingPolicy, DatSource};

fn source(file_path: &str, name: &str, ecosystem: DatEcosystem) -> DatSource {
    DatSource {
        format: DatFormat::Logiqx,
        ecosystem,
        file_path: file_path.to_string(),
        name: Some(name.to_string()),
        description: None,
        version: None,
        author: None,
        homepage: None,
        clrmamepro_header: None,
        entry_count: 0,
        rom_count: 0,
        parse_warnings: Vec::new(),
        packing_policy: DatPackingPolicy::Standard,
    }
}

fn dat_with(source: DatSource, games: Vec<DatGameEntry>) -> ParsedDat {
    ParsedDat { source, games }
}

fn game(name: &str) -> DatGameEntry {
    DatGameEntry {
        name: name.to_string(),
        ..DatGameEntry::default()
    }
}

#[test]
fn agreeing_metadata_and_filename_produce_no_divergence() {
    let dat = dat_with(
        source(
            "/dats/Nintendo - Game Boy (20240115-000000).dat",
            "Nintendo - Game Boy",
            DatEcosystem::NoIntro,
        ),
        vec![game("Tetris")],
    );

    let report = compare_dat_metadata_to_filename(&dat);
    assert!(report.divergences.is_empty(), "{:?}", report.divergences);
}

#[test]
fn a_provider_disagreement_is_reported_without_reclassifying() {
    // The filename claims TOSEC, but the parser already determined this is
    // a No-Intro DAT from its own structured header - that parsed fact
    // must never be overwritten by the filename's own claim.
    let dat = dat_with(
        source(
            "/dats/Nintendo - Game Boy (TOSEC-v2024).dat",
            "Nintendo - Game Boy",
            DatEcosystem::NoIntro,
        ),
        vec![game("Tetris")],
    );

    let report = compare_dat_metadata_to_filename(&dat);
    assert_eq!(dat.source.ecosystem, DatEcosystem::NoIntro);
    assert_eq!(report.divergences.len(), 1);
    assert_eq!(report.divergences[0].field, DivergenceField::Ecosystem);
    assert!(report.divergences[0].metadata_evidence.contains("No-Intro"));
    assert!(report.divergences[0].filename_hint.contains("tosec"));
}

#[test]
fn a_region_disagreement_between_header_text_and_filename_is_reported() {
    let dat = dat_with(
        source(
            "/dats/Sonic (Japan).dat",
            "Sonic (Europe)",
            DatEcosystem::GenericLogiqx,
        ),
        vec![game("Sonic")],
    );

    let report = compare_dat_metadata_to_filename(&dat);
    assert_eq!(
        report
            .divergences
            .iter()
            .filter(|d| d.field == DivergenceField::Region)
            .count(),
        1,
        "{:?}",
        report.divergences
    );
}

#[test]
fn a_revision_or_version_disagreement_is_reported() {
    let mut src = source(
        "/dats/Nintendo - Game Boy (20240115-000000).dat",
        "Nintendo - Game Boy",
        DatEcosystem::NoIntro,
    );
    src.version = Some("20230101-000000".to_string());
    let dat = dat_with(src, vec![game("Tetris")]);

    let report = compare_dat_metadata_to_filename(&dat);
    assert_eq!(
        report
            .divergences
            .iter()
            .filter(|d| d.field == DivergenceField::RevisionOrVersion)
            .count(),
        1,
        "{:?}",
        report.divergences
    );
}

#[test]
fn punctuation_spacing_and_case_differences_alone_create_no_noise() {
    // Same ecosystem, same header text as the filename modulo case and an
    // extra separator/underscore - none of that is a material disagreement.
    let dat = dat_with(
        source(
            "/dats/nintendo_-_game_boy (20240115-000000).dat",
            "Nintendo - Game Boy",
            DatEcosystem::NoIntro,
        ),
        vec![game("Tetris")],
    );

    let report = compare_dat_metadata_to_filename(&dat);
    assert!(report.divergences.is_empty(), "{:?}", report.divergences);
}

#[test]
fn bios_claimed_by_filename_but_absent_from_the_catalogue_is_reported() {
    let dat = dat_with(
        source(
            "/dats/Nintendo - Game Boy (BIOS).dat",
            "Nintendo - Game Boy",
            DatEcosystem::NoIntro,
        ),
        vec![game("Tetris")],
    );

    let report = compare_dat_metadata_to_filename(&dat);
    assert!(
        report
            .divergences
            .iter()
            .any(|d| d.field == DivergenceField::BiosFirmware),
        "{:?}",
        report.divergences
    );
}

#[test]
fn a_retool_style_filename_with_no_clone_relationships_at_all_is_reported() {
    let dat = dat_with(
        source(
            "/dats/Nintendo - Game Boy (Retool).dat",
            "Nintendo - Game Boy",
            DatEcosystem::NoIntro,
        ),
        vec![game("Tetris"), game("Tetris (Japan)")],
    );

    let report = compare_dat_metadata_to_filename(&dat);
    assert!(
        report
            .divergences
            .iter()
            .any(|d| d.field == DivergenceField::ParentCloneRelationship),
        "{:?}",
        report.divergences
    );
}

#[test]
fn an_ordinary_catalogue_with_real_clone_relationships_and_a_plain_filename_is_quiet() {
    // The overwhelmingly common case: real `cloneof` data, filename does
    // not mention "Retool" at all. This must never be noisy.
    let mut clone_game = game("Tetris (Japan)");
    clone_game.clone_of = Some("Tetris".to_string());
    let dat = dat_with(
        source(
            "/dats/Nintendo - Game Boy (20240115-000000).dat",
            "Nintendo - Game Boy",
            DatEcosystem::NoIntro,
        ),
        vec![game("Tetris"), clone_game],
    );

    let report = compare_dat_metadata_to_filename(&dat);
    assert!(
        report
            .divergences
            .iter()
            .all(|d| d.field != DivergenceField::ParentCloneRelationship),
        "{:?}",
        report.divergences
    );
}
