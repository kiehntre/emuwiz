use super::*;
use crate::dat::model::{DatGameEntry, DatPackingPolicy};

fn dat(eco: DatEcosystem, name: Option<&str>, games: Vec<DatGameEntry>) -> ParsedDat {
    ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: eco,
            file_path: "mem.dat".into(),
            name: name.map(str::to_string),
            description: None,
            version: Some("1".into()),
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: games.len(),
            rom_count: games.iter().map(|g| g.roms.len()).sum(),
            parse_warnings: Vec::new(),
            packing_policy: DatPackingPolicy::Standard,
        },
        games,
    }
}

fn rom(name: &str, size: Option<u64>, crc: Option<&str>, sha1: Option<&str>) -> DatRomEntry {
    DatRomEntry {
        name: name.into(),
        size_bytes: size,
        crc32: crc.map(str::to_string),
        sha1: sha1.map(str::to_string),
        ..Default::default()
    }
}

fn game(name: &str, roms: Vec<DatRomEntry>) -> DatGameEntry {
    DatGameEntry {
        name: name.into(),
        roms,
        ..Default::default()
    }
}

const S1: &str = "1111111111111111111111111111111111111111";
const S2: &str = "2222222222222222222222222222222222222222";
const S3: &str = "3333333333333333333333333333333333333333";

fn cmp(a: &ParsedDat, b: &ParsedDat) -> DuelReport {
    compare(a, b, [None, None], DuelOptions::default()).unwrap()
}
fn ni(games: Vec<DatGameEntry>) -> ParsedDat {
    dat(DatEcosystem::NoIntro, Some("Sys"), games)
}

#[test]
fn identical_hash_different_name_is_verified_rename() {
    let a = ni(vec![game(
        "Alpha",
        vec![rom("a.bin", Some(4), Some("aaaaaaaa"), Some(S1))],
    )]);
    let b = ni(vec![game(
        "Alpha (Rev 1)",
        vec![rom("a.bin", Some(4), Some("aaaaaaaa"), Some(S1))],
    )]);
    let r = cmp(&a, &b);
    assert_eq!(r.renamed.len(), 1);
    let m = r.renamed[0].matched_by.as_ref().unwrap();
    assert_eq!((m.basis, m.confidence), ("sha1", Confidence::Verified));
    assert_eq!(
        r.renamed[0].name_relation,
        NameRelation::DifferentGameSameRom
    );
    assert!(r.only_in_a.is_empty() && r.only_in_b.is_empty());
}

#[test]
fn same_name_conflicting_hash_or_size_is_a_conflict() {
    let a = ni(vec![game(
        "G",
        vec![rom("r", Some(4), Some("aaaaaaaa"), Some(S1))],
    )]);
    let b = ni(vec![game(
        "G",
        vec![rom("r", Some(8), Some("bbbbbbbb"), Some(S2))],
    )]);
    let r = cmp(&a, &b);
    assert_eq!(r.same_name_conflicts.len(), 1);
    let fields: Vec<_> = r.same_name_conflicts[0]
        .contradictions
        .iter()
        .map(|c| c.field)
        .collect();
    assert!(fields.contains(&"sha1") && fields.contains(&"size_bytes"));
    assert!(r.renamed.is_empty());
    // nothing is also reported as merely "only in"
    assert!(r.only_in_a.is_empty() && r.only_in_b.is_empty());
}

#[test]
fn crc_size_only_is_probable_never_verified() {
    let a = ni(vec![game(
        "G",
        vec![rom("r", Some(4), Some("aaaaaaaa"), None)],
    )]);
    let b = ni(vec![game(
        "H",
        vec![rom("r", Some(4), Some("aaaaaaaa"), None)],
    )]);
    let r = cmp(&a, &b);
    let m = r.renamed[0].matched_by.as_ref().unwrap();
    assert_eq!(
        (m.basis, m.confidence),
        ("crc32_size", Confidence::Probable)
    );
    assert_eq!(r.summary.renamed_probable, 1);
    assert_eq!(r.summary.renamed_verified, 0);
}

#[test]
fn crc_without_size_is_not_a_match() {
    let a = ni(vec![game(
        "G",
        vec![rom("r", None, Some("aaaaaaaa"), None)],
    )]);
    let b = ni(vec![game(
        "H",
        vec![rom("x", None, Some("aaaaaaaa"), None)],
    )]);
    let r = cmp(&a, &b);
    assert!(r.renamed.is_empty());
    assert_eq!(r.summary.only_in_a_unverifiable, 1);
    assert_eq!(r.summary.only_in_b_unverifiable, 1);
}

#[test]
fn crc_size_equal_but_sha1_differs_is_a_conflict_not_a_match() {
    let a = ni(vec![game(
        "G",
        vec![rom("r", Some(4), Some("aaaaaaaa"), Some(S1))],
    )]);
    let b = ni(vec![game(
        "H",
        vec![rom("q", Some(4), Some("aaaaaaaa"), Some(S2))],
    )]);
    let r = cmp(&a, &b);
    assert!(r.renamed.is_empty());
    assert_eq!(r.cross_name_conflicts.len(), 1);
}

#[test]
fn same_name_without_comparable_data_is_unverified_not_agreement() {
    let a = ni(vec![game("G", vec![rom("r", None, None, None)])]);
    let b = ni(vec![game(
        "G",
        vec![rom("r", Some(4), Some("aaaaaaaa"), Some(S1))],
    )]);
    let r = cmp(&a, &b);
    assert_eq!(r.summary.same_name_unverified, 1);
    assert_eq!(
        r.summary.verified_same_name + r.summary.probable_same_name,
        0
    );
}

#[test]
fn missing_checksums_are_unverifiable_not_confirmed_absent() {
    let a = ni(vec![game(
        "G",
        vec![
            rom("lonely", None, None, None),
            rom("hashed", Some(1), None, Some(S3)),
        ],
    )]);
    let b = ni(vec![game(
        "Z",
        vec![rom(
            "z",
            Some(1),
            None,
            Some("ffffffffffffffffffffffffffffffffffffffff"),
        )],
    )]);
    let r = cmp(&a, &b);
    let by: BTreeMap<_, _> = r
        .only_in_a
        .iter()
        .map(|e| (e.record.rom.as_str(), &e.absence))
        .collect();
    assert_eq!(by["lonely"], &Absence::Unverifiable);
    assert_eq!(by["hashed"], &Absence::NoCounterpart);
}

#[test]
fn duplicate_checksum_matches_stay_visible_as_ambiguous() {
    // Two different names in A share one hash; B has the same hash once.
    let a = ni(vec![game(
        "G",
        vec![
            rom("one", Some(4), None, Some(S1)),
            rom("two", Some(4), None, Some(S1)),
        ],
    )]);
    let b = ni(vec![game("G", vec![rom("one", Some(4), None, Some(S1))])]);
    let r = cmp(&a, &b);
    assert_eq!(r.ambiguous.len(), 1);
    assert_eq!(r.ambiguous[0].a.len(), 2);
    assert_eq!(r.duplicate_checksums_a.len(), 1);
    assert_eq!(r.duplicate_checksums_a[0].record_count, 2);
}

#[test]
fn repeated_identical_names_across_games_pair_up_without_ambiguity() {
    let rep = |g: &str| game(g, vec![rom("shared.bin", Some(4), None, Some(S1))]);
    let a = ni(vec![rep("parent"), rep("clone")]);
    let b = ni(vec![rep("parent"), rep("clone")]);
    let r = cmp(&a, &b);
    assert!(r.ambiguous.is_empty());
    assert_eq!(r.summary.verified_same_name, 2);
}

#[test]
fn strong_hash_wins_over_filename_equality_across_games() {
    // Same ROM file name in different games must not be matched by name.
    let a = ni(vec![game(
        "G1",
        vec![rom("rom.bin", Some(4), None, Some(S1))],
    )]);
    let b = ni(vec![game(
        "G2",
        vec![rom("rom.bin", Some(4), None, Some(S2))],
    )]);
    let r = cmp(&a, &b);
    assert!(r.renamed.is_empty() && r.same_name_conflicts.is_empty());
    assert_eq!(r.summary.only_in_a_no_counterpart, 1);
    assert_eq!(r.summary.only_in_b_no_counterpart, 1);
}

#[test]
fn bios_flag_difference_is_reported_on_a_verified_match() {
    let mut ra = rom("r", Some(4), None, Some(S1));
    ra.bios = Some("yes".into());
    let a = ni(vec![game("G", vec![ra])]);
    let b = ni(vec![game("G", vec![rom("r", Some(4), None, Some(S1))])]);
    let r = cmp(&a, &b);
    assert_eq!(r.summary.bios_flag_differences, 1);
    assert!(r.renamed[0].bios_differs);
}

#[test]
fn incompatible_catalogues_are_refused_unless_explicitly_allowed() {
    let a = dat(DatEcosystem::NoIntro, Some("Nintendo - NES"), vec![]);
    let b = dat(DatEcosystem::Redump, Some("Nintendo - NES"), vec![]);
    assert_eq!(
        compare(&a, &b, [None, None], DuelOptions::default())
            .unwrap_err()
            .compatibility,
        Compatibility::IncompatibleEcosystem
    );
    let c = dat(DatEcosystem::NoIntro, Some("Sega - Mega Drive"), vec![]);
    assert_eq!(
        compare(&a, &c, [None, None], DuelOptions::default())
            .unwrap_err()
            .compatibility,
        Compatibility::IncompatibleCatalogue
    );
    let opt = DuelOptions {
        allow_incompatible: true,
        ..Default::default()
    };
    let r = compare(&a, &b, [None, None], opt).unwrap();
    assert!(!r.naming_comparable);
    // variants stay comparable but are labelled
    let v1 = dat(
        DatEcosystem::NoIntro,
        Some("Nintendo - NES (Headered)"),
        vec![],
    );
    let v2 = dat(
        DatEcosystem::NoIntro,
        Some("Nintendo - NES (Headerless)"),
        vec![],
    );
    assert_eq!(cmp(&v1, &v2).compatibility, Compatibility::VariantDiffers);
    let unknown = dat(DatEcosystem::NoIntro, None, vec![]);
    assert_eq!(
        compare(&unknown, &v1, [None, None], DuelOptions::default())
            .unwrap_err()
            .compatibility,
        Compatibility::UnverifiedNamespace
    );
}

#[test]
fn invalid_checksum_text_is_ignored_and_counted() {
    let a = ni(vec![game(
        "G",
        vec![rom("r", Some(4), Some("zzzz"), Some("nothex"))],
    )]);
    let r = cmp(&a, &ni(vec![]));
    assert_eq!(r.sources[0].invalid_checksum_fields, 2);
    assert_eq!(r.summary.only_in_a_unverifiable, 1);
}

#[test]
fn output_is_deterministic_regardless_of_input_order() {
    let g = |n: &str, s: &str| game(n, vec![rom("r", Some(1), None, Some(s))]);
    let a1 = ni(vec![g("B", S1), g("A", S2), g("C", S3)]);
    let a2 = ni(vec![g("C", S3), g("B", S1), g("A", S2)]);
    let b = ni(vec![g("Z", S1), g("Y", S2)]);
    let j1 = serde_json::to_string(&cmp(&a1, &b)).unwrap();
    let j2 = serde_json::to_string(&cmp(&a2, &b)).unwrap();
    assert_eq!(j1, j2);
}

#[test]
fn degenerate_shared_hash_buckets_are_bounded() {
    let many: Vec<_> = (0..300)
        .map(|i| rom(&format!("r{i}"), Some(0), None, Some(S1)))
        .collect();
    let a = ni(vec![game("G", many.clone())]);
    let b = ni(vec![game("H", many)]);
    let r = cmp(&a, &b);
    assert!(r.summary.candidate_buckets_truncated > 0);
    assert!(r.ambiguous.iter().all(|g| g.a.len() <= MAX_GROUP_MEMBERS));
}

#[test]
fn section_listing_is_capped_but_counts_stay_complete() {
    let a = ni((0..10)
        .map(|i| {
            game(
                &format!("G{i}"),
                vec![rom("r", Some(1), None, Some(&format!("{i:040x}")))],
            )
        })
        .collect());
    let opt = DuelOptions {
        max_items_per_section: 3,
        ..Default::default()
    };
    let r = compare(
        &a,
        &ni(vec![game(
            "Z",
            vec![rom(
                "z",
                Some(1),
                None,
                Some("ffffffffffffffffffffffffffffffffffffffff"),
            )],
        )]),
        [None, None],
        opt,
    )
    .unwrap();
    assert_eq!(r.only_in_a.len(), 3);
    assert_eq!(r.summary.only_in_a_no_counterpart, 10);
    assert_eq!(r.truncated_sections, vec!["only_in_a"]);
}

#[test]
fn crc_size_only_pairs_are_probable_not_verified_even_when_name_paired() {
    let two = |g: &str| {
        game(
            g,
            vec![
                rom("r1", Some(4), Some("aaaaaaaa"), None),
                rom("r2", Some(4), Some("aaaaaaaa"), None),
            ],
        )
    };
    let r = cmp(&ni(vec![two("g")]), &ni(vec![two("g")]));
    assert_eq!(r.summary.verified_same_name, 0);
    assert_eq!(r.summary.probable_same_name, 2);
}

#[test]
fn name_paired_multi_record_group_keeps_bios_difference_visible() {
    let mk = |bios: bool| {
        let mut r1 = rom("r1", Some(4), None, Some(S1));
        if bios {
            r1.bios = Some("yes".into());
        }
        vec![
            game("g1", vec![r1]),
            game("g2", vec![rom("r1", Some(4), None, Some(S1))]),
        ]
    };
    let r = cmp(&ni(mk(true)), &ni(mk(false)));
    assert_eq!(r.summary.bios_flag_differences, 1);
    assert_eq!(r.summary.verified_by_sha1, 2);
}

#[test]
fn capped_bucket_cannot_hide_the_same_name_match() {
    let junk = |p: &str| -> Vec<DatRomEntry> {
        (0..8)
            .map(|i| {
                rom(
                    &format!("{p}{i}"),
                    Some(9),
                    Some(&format!("{i:08x}")),
                    Some(S1),
                )
            })
            .collect()
    };
    let mut ga = junk("a");
    ga.push(rom("x", Some(4), None, Some(S1)));
    let mut gb = junk("b");
    gb.push(rom("x", Some(4), None, Some(S1)));
    let r = cmp(&ni(vec![game("g", ga)]), &ni(vec![game("g", gb)]));
    assert!(r.summary.candidate_buckets_truncated > 0);
    assert!(r.summary.verified_same_name >= 1, "{:?}", r.summary);
    assert!(!r.same_name_unverified.iter().any(|p| p.a.rom == "x"));
}

#[test]
fn incomparable_algorithms_are_unverifiable_not_confirmed_absent() {
    let md5 = DatRomEntry {
        name: "q".into(),
        size_bytes: Some(4),
        md5: Some("a".repeat(32)),
        ..Default::default()
    };
    let a = ni(vec![game("G", vec![rom("p", Some(4), None, Some(S1))])]);
    let b = ni(vec![game("H", vec![md5])]);
    let r = cmp(&a, &b);
    assert_eq!(r.summary.only_in_a_unverifiable, 1);
    assert_eq!(r.summary.only_in_b_unverifiable, 1);
    assert_eq!(
        r.summary.only_in_a_no_counterpart + r.summary.only_in_b_no_counterpart,
        0
    );
}

#[test]
fn duplicate_same_name_record_is_not_silently_dropped() {
    let a = ni(vec![game("G", vec![rom("r", Some(4), None, Some(S1))])]);
    let b = ni(vec![game(
        "G",
        vec![
            rom("r", Some(4), None, Some(S1)),
            rom("r", Some(4), None, Some(S1)),
        ],
    )]);
    let r = cmp(&a, &b);
    assert_eq!(r.ambiguous.len(), 1);
    assert_eq!(r.ambiguous[0].b.len(), 2);
}

#[test]
fn missing_header_name_requires_explicit_override() {
    let a = dat(DatEcosystem::NoIntro, None, vec![]);
    let b = ni(vec![]);
    assert!(compare(&a, &b, [None, None], DuelOptions::default()).is_err());
    let opt = DuelOptions {
        allow_incompatible: true,
        ..Default::default()
    };
    assert!(
        !compare(&a, &b, [None, None], opt)
            .unwrap()
            .naming_comparable
    );
}

#[test]
fn same_name_different_hashes_order_by_evidence_not_input_position() {
    let g = |s: &str| game("G", vec![rom("r", Some(1), None, Some(s))]);
    let (b1, b2) = (ni(vec![g(S1), g(S2)]), ni(vec![g(S2), g(S1)]));
    let other = ni(vec![game("G", vec![rom("r", Some(2), None, Some(S3))])]);
    assert_eq!(
        serde_json::to_string(&cmp(&b1, &other)).unwrap(),
        serde_json::to_string(&cmp(&b2, &other)).unwrap()
    );
}
