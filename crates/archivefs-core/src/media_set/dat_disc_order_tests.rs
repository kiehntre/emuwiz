use super::*;

#[test]
fn redump_and_tosec_disc_tokens_give_the_stated_position() {
    assert_eq!(
        dat_disc_token("Final Fantasy VII (USA) (Disc 2)"),
        Some(DatDiscToken {
            base_title: "Final Fantasy VII (USA)".into(),
            unit: OrdinalUnit::Disc,
            number: 2,
            total: None,
        })
    );
    assert_eq!(
        dat_disc_token("Orbit Quest (Disc 3) (Rev 1)").map(|t| (t.base_title, t.number)),
        Some(("Orbit Quest (Rev 1)".into(), 3))
    );
    assert_eq!(
        dat_disc_token("Orbit Quest (1996)(Acme)(Disc 2 of 3)"),
        Some(DatDiscToken {
            base_title: "Orbit Quest (1996)(Acme)".into(),
            unit: OrdinalUnit::Disc,
            number: 2,
            total: Some(3),
        })
    );
    let disk = dat_disc_token("Orbit Quest (1989)(Acme)(Disk 2 of 3)").unwrap();
    assert_eq!(
        (disk.unit, disk.number, disk.total),
        (OrdinalUnit::Disk, 2, Some(3))
    );
    assert!(disk.fits(MediaFamily::Floppy) && !disk.fits(MediaFamily::Optical));
    let tape = dat_disc_token("Orbit Quest (Europe) (Tape 1)").unwrap();
    assert_eq!(
        (tape.unit, tape.number, tape.total),
        (OrdinalUnit::Tape, 1, None)
    );
    assert!(tape.fits(MediaFamily::Tape) && !tape.fits(MediaFamily::Floppy));
    assert!(
        !dat_disc_token("X (Disc 1)")
            .unwrap()
            .fits(MediaFamily::Floppy)
    );
    assert_eq!(dat_order_family("adf"), Some(MediaFamily::Floppy));
    assert_eq!(dat_order_family("tzx"), Some(MediaFamily::Tape));
    assert_eq!(dat_order_family("bin"), None);
}

#[test]
fn unrecognised_or_ambiguous_disc_tokens_give_nothing() {
    for name in [
        "Orbit Quest (USA)",
        "Disc 2 Orbit Quest (USA)",
        "Orbit Quest (USA) (Disc A)",
        "Orbit Quest (USA) (Disc 0)",
        "Orbit Quest (USA) (Disc 2b)",
        "Orbit Quest (USA) (Disc 1) (Disc 2)",
        "Orbit Quest (USA) (Disc 3 of 2)",
        "Orbit Quest (USA) (Bonus Disc)",
        "Orbit Quest (USA) (Disc 1) (Tape 2)",
        "Orbit Quest (1989)(Acme)(Disk 1 of 2)(Side A)",
        "Orbit Quest (1989)(Acme)(Disk 1 Side B)",
        "Orbit Quest (USA) (Disc 1 of 1)",
        "(Disc 1)",
    ] {
        assert_eq!(dat_disc_token(name), None, "{name}");
    }
}

#[test]
fn a_total_needs_every_disc_from_one_listed_by_the_catalogue() {
    let catalogue = [
        "Orbit Quest (USA) (Disc 1)",
        "Orbit Quest (USA) (Disc 2)",
        "Orbit Quest (USA) (Disc 3)",
        "Orbit Quest (Europe) (Disc 1)",
        "Orbit Quest (Europe) (Disc 2)",
        "Orbit Quest (USA) (Disc 1) (Rev 1)",
    ];
    let total = |name: &str, names: &[&str]| {
        catalogue_disc_total(&dat_disc_token(name).unwrap(), names.iter().copied())
    };
    assert_eq!(total("Orbit Quest (USA) (Disc 2)", &catalogue), Some(3));
    assert_eq!(total("Orbit Quest (Europe) (Disc 1)", &catalogue), Some(2));
    // One listed disc is not a set, and the highest number is never a total.
    assert_eq!(
        total("Orbit Quest (USA) (Disc 1) (Rev 1)", &catalogue),
        None
    );
    assert_eq!(
        total(
            "Gap (USA) (Disc 1)",
            &["Gap (USA) (Disc 1)", "Gap (USA) (Disc 3)"]
        ),
        None
    );
    assert_eq!(
        total(
            "Repeat (USA) (Disc 1)",
            &[
                "Repeat (USA) (Disc 1)",
                "Repeat (USA) (disc 1)",
                "Repeat (USA) (Disc 2)"
            ]
        ),
        None
    );
    assert_eq!(
        total("Mixed (Disc 1)", &["Mixed (Disc 1)", "Mixed (Disc 2 of 2)"]),
        None
    );
    // Units are counted separately: tapes never complete a disc set.
    assert_eq!(
        total(
            "Orbit (Tape 1)",
            &["Orbit (Tape 1)", "Orbit (Tape 2)", "Orbit (Disc 3)"]
        ),
        Some(2)
    );
}

fn candidate(name: &str, path: &str) -> DatDiscCandidate {
    DatDiscCandidate {
        path: path.into(),
        platform: "psx".into(),
        format: "cue".into(),
        source_id: "redump-psx".into(),
        source_revision: "2026-10-01".into(),
        catalogue_sha256: Some("ab".repeat(32)),
        canonical_name: name.into(),
        matched_algorithm: "SHA-1".into(),
        matched_value: Some("00".repeat(20)),
    }
}

fn set_of(entries: &[(&str, &str)], total: Option<u16>) -> Vec<MediaSet> {
    let records = entries
        .iter()
        .map(|(name, path)| {
            let candidate = candidate(name, path);
            dat_disc_record(
                &candidate,
                MediaFamily::Optical,
                &dat_disc_token(name).unwrap(),
                total,
            )
        })
        .collect();
    crate::media_set::resolve_index(crate::media_set::index_media(records)).sets
}

#[test]
fn catalogue_order_wins_over_misleading_local_file_names() {
    let sets = set_of(
        &[
            ("Orbit Quest (USA) (Disc 1)", "/games/z-orbit disc 3.cue"),
            ("Orbit Quest (USA) (Disc 2)", "/games/a-orbit disc 1.cue"),
            (
                "Orbit Quest (USA) (Disc 3)",
                "/other/orbit (Europe) (Disc 2).cue",
            ),
        ],
        Some(3),
    );
    assert_eq!(sets.len(), 1);
    let set = &sets[0];
    assert_eq!(set.state, MediaSetState::CompleteSet, "{:?}", set.conflicts);
    assert_eq!(set.confidence, MediaSetConfidence::Proven);
    assert!(is_dat_disc_order_set(set));
    let order = set
        .members
        .iter()
        .map(|m| {
            m.representations[0]
                .record
                .source
                .path
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        [
            "/games/z-orbit disc 3.cue",
            "/games/a-orbit disc 1.cue",
            "/other/orbit (Europe) (Disc 2).cue"
        ]
    );
    let claim = dat_disc_claim(&set.members[1].representations[0]).unwrap();
    assert_eq!(
        claim,
        (
            "redump-psx".into(),
            "2026-10-01".into(),
            "Orbit Quest (USA) (Disc 2)".into()
        )
    );
}

#[test]
fn missing_disc_unknown_total_duplicates_and_other_releases_never_complete() {
    let missing = set_of(
        &[
            ("Orbit Quest (USA) (Disc 1)", "/g/1.cue"),
            ("Orbit Quest (USA) (Disc 3)", "/g/3.cue"),
        ],
        Some(3),
    );
    assert_eq!(missing[0].state, MediaSetState::IncompleteSet);

    let unknown = set_of(
        &[
            ("Orbit Quest (USA) (Disc 1)", "/g/1.cue"),
            ("Orbit Quest (USA) (Disc 2)", "/g/2.cue"),
        ],
        None,
    );
    assert_eq!(unknown[0].state, MediaSetState::UnverifiedSet);
    assert!(
        unknown[0]
            .conflicts
            .iter()
            .any(|c| c.kind == ConflictKind::UnknownCount)
    );

    let duplicate = set_of(
        &[
            ("Orbit Quest (USA) (Disc 1)", "/g/1.cue"),
            ("Orbit Quest (USA) (Disc 1)", "/copy/1.cue"),
            ("Orbit Quest (USA) (Disc 2)", "/g/2.cue"),
        ],
        Some(2),
    );
    assert_eq!(duplicate[0].state, MediaSetState::AmbiguousSet);

    let regions = set_of(
        &[
            ("Orbit Quest (USA) (Disc 1)", "/g/1.cue"),
            ("Orbit Quest (Europe) (Disc 2)", "/g/2.cue"),
        ],
        Some(2),
    );
    assert_eq!(regions.len(), 2);
    assert!(
        regions
            .iter()
            .all(|set| set.state == MediaSetState::IncompleteSet)
    );
}
