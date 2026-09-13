use super::*;
use std::collections::BTreeSet;
fn rec(name: &str, platform: &str) -> MediaRecord {
    let mut r = media_record(name, Some(platform));
    r.availability = MediaAvailability::Observed;
    r
}
fn unit(family: Option<MediaFamily>) -> OrdinalUnit {
    match family {
        Some(MediaFamily::Optical) => OrdinalUnit::Disc,
        Some(MediaFamily::Floppy) => OrdinalUnit::Disk,
        _ => OrdinalUnit::Tape,
    }
}
fn proven(
    mut r: MediaRecord,
    release: &str,
    number: Option<u16>,
    total: Option<u16>,
) -> MediaRecord {
    let mut e = MediaEvidence::new(EvidenceKind::TrustedDat, "synthetic authority v1");
    e.release = Some(IdentityKey::new("fixture-release", release));
    e.medium = Some(IdentityKey::new(
        "fixture-medium",
        format!("{release}:{number:?}"),
    ));
    e.equivalence = Equivalence::AuthorityMapping;
    e.ordinal = number.map(|number| MediaOrdinal {
        number,
        unit: unit(r.family),
    });
    e.expected_count = total.map(|count| ExpectedCount {
        count,
        unit: unit(r.family),
    });
    r.evidence.push(e);
    r
}
fn sets(records: Vec<MediaRecord>) -> Vec<MediaSet> {
    resolve_index(index_media(records)).sets
}
fn has(set: &MediaSet, kind: ConflictKind) -> bool {
    set.conflicts.iter().any(|c| c.kind == kind)
}
fn optical(numbers: &[u16], total: u16) -> Vec<MediaRecord> {
    numbers
        .iter()
        .map(|n| {
            proven(
                rec(
                    &format!("/disc/{n}/Orbit Quest (Disc {n} of {total}).chd"),
                    "PSX",
                ),
                "orbit",
                Some(*n),
                Some(total),
            )
        })
        .collect()
}
fn floppies(numbers: &[u16], total: u16) -> Vec<MediaRecord> {
    numbers
        .iter()
        .map(|n| {
            proven(
                rec(
                    &format!("/floppy/{n}/Clockwork (Disk {n} of {total}).adf"),
                    "Amiga",
                ),
                "clockwork",
                Some(*n),
                Some(total),
            )
        })
        .collect()
}
fn tape(number: u16, total: u16) -> MediaRecord {
    proven(
        rec(
            &format!("/tape/{number}/Moon Voyage (Tape {number} of {total}).tap"),
            "ZX Spectrum",
        ),
        "moon",
        Some(number),
        Some(total),
    )
}
#[test]
fn optical_two_and_three_disc_sets() {
    for count in [2, 3] {
        let s = sets(optical(&(1..=count).collect::<Vec<_>>(), count));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].state, MediaSetState::CompleteSet);
        assert_eq!(s[0].members.len(), count as usize);
    }
}
#[test]
fn optical_missing_middle_disc() {
    let s = sets(optical(&[1, 3], 3));
    assert_eq!(s[0].state, MediaSetState::IncompleteSet);
    assert!(s[0].conflicts.iter().any(|c| c.detail.contains("Disc 2")));
}
#[test]
fn alternate_chd_cue_is_one_medium_only_with_proof() {
    let mut records = optical(&[1, 2], 2);
    let mut alt = records[0].clone();
    alt.source.path = "/alternate/Orbit Quest (Disc 1 of 2).cue".into();
    alt.format = "cue".into();
    records.push(alt);
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    assert_eq!(s[0].members.len(), 2);
    assert_eq!(s[0].members[0].representations.len(), 2);
    let plan = media_swap_plan(&s[0], None);
    assert!(plan.ordered_media[0].preferred_representation.is_none());
    let p = MediaProfile {
        id: "selected".into(),
        platform: "PSX".into(),
        supported_formats: BTreeSet::from(["chd".into(), "cue".into()]),
        preferred_formats: vec!["chd".into()],
        readiness_hint: None,
    };
    assert_eq!(
        media_swap_plan(&s[0], Some(&p)).ordered_media[0]
            .preferred_representation
            .as_ref()
            .unwrap()
            .path
            .extension()
            .unwrap(),
        "chd"
    );
}
#[test]
fn filename_formats_do_not_prove_equivalence() {
    let s = sets(vec![
        rec("Orbit (Disc 1 of 2).chd", "PSX"),
        rec("Orbit (Disc 1 of 2).cue", "PSX"),
    ]);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].state, MediaSetState::AmbiguousSet);
    assert_eq!(s[0].members.len(), 2);
}
#[test]
fn bonus_is_not_a_missing_game_disc() {
    let mut records = optical(&[1], 2);
    let mut bonus = proven(
        rec("Orbit Quest (Bonus Disc).chd", "PSX"),
        "orbit",
        None,
        Some(2),
    );
    bonus.evidence.last_mut().unwrap().medium = Some(IdentityKey::new("fixture-medium", "bonus"));
    records.push(bonus);
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::IncompleteSet);
    assert!(s[0].members.iter().any(|m| m.role == MediaRole::BonusMedia));
}
fn role_pair(family: MediaFamily) -> Vec<MediaRecord> {
    let (platform, ext, label, a, b) = match family {
        MediaFamily::Optical => (
            "DOS",
            "iso",
            "Disc",
            MediaRole::InstallMedia,
            MediaRole::PlayMedia,
        ),
        MediaFamily::Floppy => (
            "Amiga",
            "adf",
            "Disk",
            MediaRole::BootMedia,
            MediaRole::DataMedia,
        ),
        MediaFamily::Tape => (
            "ZX Spectrum",
            "tap",
            "Tape",
            MediaRole::BootMedia,
            MediaRole::GameMedia,
        ),
    };
    [a, b]
        .iter()
        .enumerate()
        .map(|(i, role)| {
            let mut r = proven(
                rec(&format!("Role Quest ({label} role {i}).{ext}"), platform),
                "roles",
                None,
                None,
            );
            r.evidence
                .retain(|e| e.provenance.kind != EvidenceKind::Filename);
            let e = r.evidence.last_mut().unwrap();
            e.role = Some(*role);
            e.medium = Some(IdentityKey::new("fixture-medium", format!("role-{i}")));
            e.requirements = [a, b]
                .iter()
                .map(|role| MediumRequirement {
                    ordinal: None,
                    side: None,
                    role: Some(*role),
                    medium: None,
                    optional: false,
                })
                .collect();
            r
        })
        .collect()
}
#[test]
fn optical_install_play_manifest_without_invented_ordinals() {
    let s = sets(role_pair(MediaFamily::Optical));
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    assert_eq!(s[0].members[0].role, MediaRole::InstallMedia);
    assert!(s[0].members.iter().all(|m| m.ordinal.is_none()));
}
#[test]
fn floppy_boot_data_roles() {
    let s = sets(role_pair(MediaFamily::Floppy));
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    assert_eq!(s[0].members[0].role, MediaRole::BootMedia);
}
#[test]
fn floppy_two_and_four_disk_sets() {
    for count in [2, 4] {
        let s = sets(floppies(&(1..=count).collect::<Vec<_>>(), count));
        assert_eq!(s[0].state, MediaSetState::CompleteSet);
        assert_eq!(s[0].members.len(), count as usize);
    }
}
#[test]
fn floppy_missing_disk() {
    assert_eq!(
        sets(floppies(&[1, 3, 4], 4))[0].state,
        MediaSetState::IncompleteSet
    );
}
fn side_record(family: MediaFamily, disk: u16, side: u8) -> MediaRecord {
    let (platform, ext, label) = match family {
        MediaFamily::Floppy => ("Amiga", "adf", "Disk"),
        _ => ("ZX Spectrum", "tap", "Tape"),
    };
    let mut r = proven(
        rec(
            &format!(
                "Side Quest ({label} {disk} Side {}).{ext}",
                if side == 1 { "A" } else { "B" }
            ),
            platform,
        ),
        "sides",
        Some(disk),
        Some(2),
    );
    let e = r.evidence.last_mut().unwrap();
    e.side = Some(MediaSide { number: side });
    e.side_layout = Some(SideLayout::SeparateSideImages);
    e.expected_sides = BTreeSet::from([MediaSide { number: 1 }, MediaSide { number: 2 }]);
    e.medium = Some(IdentityKey::new("fixture-medium", format!("{disk}:{side}")));
    r
}
#[test]
fn floppy_four_sides_are_two_disks() {
    let records = (1..=2)
        .flat_map(|d| (1..=2).map(move |s| side_record(MediaFamily::Floppy, d, s)))
        .collect();
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    assert_eq!(s[0].members.len(), 2);
    let p = media_swap_plan(&s[0], None);
    assert_eq!(p.ordered_media.len(), 4);
    assert_eq!(p.transitions[0].kind, TransitionKind::ChangeSide);
    assert_eq!(p.transitions[1].kind, TransitionKind::InsertMedium);
}
#[test]
fn whole_floppy_sides_are_not_separate_swap_images() {
    let mut r = proven(
        rec("Clockwork (Disk 1).dsk", "Amstrad CPC"),
        "whole",
        Some(1),
        Some(1),
    );
    let e = r.evidence.last_mut().unwrap();
    e.side_layout = Some(SideLayout::WholeMedium);
    e.expected_sides = BTreeSet::from([MediaSide { number: 1 }, MediaSide { number: 2 }]);
    let s = sets(vec![r]);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    assert_eq!(media_swap_plan(&s[0], None).ordered_media.len(), 1);
}
#[test]
fn floppy_adf_ipf_representations_use_declared_medium_identity() {
    let mut records = floppies(&[1, 2], 2);
    let mut ipf = records[0].clone();
    ipf.source.path = "other/Disk1.ipf".into();
    ipf.format = "ipf".into();
    records.push(ipf);
    let s = sets(records);
    assert_eq!(s[0].members.len(), 2);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
}
#[test]
fn tape_one_and_two_with_missing_second() {
    assert_eq!(
        sets(vec![tape(1, 2), tape(2, 2)])[0].state,
        MediaSetState::CompleteSet
    );
    assert_eq!(
        sets(vec![tape(1, 2)])[0].state,
        MediaSetState::IncompleteSet
    );
}
#[test]
fn tape_sides_remain_two_tapes_not_four() {
    let records = (1..=2)
        .flat_map(|d| (1..=2).map(move |s| side_record(MediaFamily::Tape, d, s)))
        .collect();
    let s = sets(records);
    assert_eq!(s[0].members.len(), 2);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    assert_eq!(
        media_swap_plan(&s[0], None).semantics,
        Some(SwapSemantics::TapeLoad)
    );
}
#[test]
fn missing_tape_side_is_explicit() {
    let s = sets(vec![side_record(MediaFamily::Tape, 1, 1)]);
    assert!(has(&s[0], ConflictKind::MissingSide));
    assert_eq!(s[0].members.len(), 1);
}
#[test]
fn loader_program_data_relationships() {
    let mut records = role_pair(MediaFamily::Tape);
    let target = records[1].evidence.last().unwrap().medium.clone().unwrap();
    records[0]
        .evidence
        .last_mut()
        .unwrap()
        .relationships
        .push(MediaRelationship {
            target,
            kind: TransitionKind::LoaderToProgram,
        });
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    assert_eq!(
        media_swap_plan(&s[0], None).transitions[0].kind,
        TransitionKind::LoaderToProgram
    );
}
#[test]
fn tap_tzx_alternates_require_proof() {
    let mut r = tape(1, 1);
    let mut alt = r.clone();
    alt.source.path = "other/Moon Voyage.tzx".into();
    alt.format = "tzx".into();
    let s = sets(vec![r.clone(), alt.clone()]);
    assert_eq!(s[0].members.len(), 1);
    r.evidence.last_mut().unwrap().equivalence = Equivalence::None;
    alt.evidence.last_mut().unwrap().equivalence = Equivalence::None;
    assert_eq!(sets(vec![r, alt])[0].state, MediaSetState::AmbiguousSet);
}
#[test]
fn filename_only_count_remains_unverified_when_all_present() {
    let s = sets(vec![
        rec("Orbit (Disc 1 of 2).chd", "PSX"),
        rec("Orbit (Disc 2 of 2).chd", "PSX"),
    ]);
    assert_eq!(s[0].state, MediaSetState::UnverifiedSet);
    assert_eq!(s[0].confidence, MediaSetConfidence::Likely);
    assert_eq!(
        s[0].expected_count.as_ref().unwrap().1.kind,
        EvidenceKind::Filename
    );
}
#[test]
fn cross_directory_grouping_and_deterministic_output() {
    let records = optical(&[3, 1, 2], 3);
    let mut reversed = records.clone();
    reversed.reverse();
    assert_eq!(
        resolve_index(index_media(records)),
        resolve_index(index_media(reversed))
    );
    let s = sets(optical(&[3, 1, 2], 3));
    assert_eq!(
        s[0].members
            .iter()
            .map(|m| m.ordinal.as_ref().unwrap().number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}
#[test]
fn weak_ordinals_cannot_override_native_claims() {
    let mut r = rec("Orbit (Disc 2 of 2).chd", "PSX");
    r = proven(r, "orbit", Some(1), Some(2));
    r.evidence.last_mut().unwrap().provenance.kind = EvidenceKind::VerifiedNative;
    let s = sets(vec![r]);
    assert_eq!(s[0].members[0].ordinal.as_ref().unwrap().number, 1);
    assert_eq!(s[0].state, MediaSetState::ConflictingSet);
    assert!(has(&s[0], ConflictKind::OrdinalConflict));
}
#[test]
fn sequel_title_is_never_removed() {
    for pair in [
        ["Resident Evil (Disc 1).chd", "Resident Evil 2 (Disc 2).chd"],
        ["Resident Evil (1-2).chd", "Resident Evil 2 (1-2).chd"],
    ] {
        assert_eq!(sets(pair.iter().map(|p| rec(p, "PSX")).collect()).len(), 2);
    }
}
#[test]
fn region_revision_language_and_editions_are_partitioned() {
    for (a, b) in [
        ("USA", "Europe"),
        ("Rev A", "Rev B"),
        ("En", "Fr"),
        ("PAL", "NTSC"),
        ("Platinum", "Greatest Hits"),
        ("Prototype", ""),
        ("Translation", "Hack"),
    ] {
        for (platform, label, format) in [
            ("PSX", "Disc", "chd"),
            ("Amiga", "Disk", "adf"),
            ("ZX Spectrum", "Tape", "tap"),
        ] {
            let s = sets(vec![
                rec(&format!("Orbit ({a}) ({label} 1).{format}"), platform),
                rec(&format!("Orbit ({b}) ({label} 2).{format}"), platform),
            ]);
            assert_eq!(s.len(), 2, "{platform}: {a}/{b}");
        }
    }
}
#[test]
fn variant_conflicts_are_reported_even_when_sets_are_separate() {
    let s = sets(vec![
        rec("Orbit USA Disk 1.adf", "Amiga"),
        rec("Orbit Europe Disk 2.adf", "Amiga"),
    ]);
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|s| has(s, ConflictKind::VariantConflict)));
}
#[test]
fn same_title_but_unrelated_release_ids_do_not_group() {
    let a = proven(
        rec("Same Name (Disk 1).adf", "Amiga"),
        "release-one",
        Some(1),
        Some(2),
    );
    let b = proven(
        rec("Same Name (Disk 2).adf", "Amiga"),
        "release-two",
        Some(2),
        Some(2),
    );
    assert_eq!(sets(vec![a, b]).len(), 2);
}
#[test]
fn tape_titles_and_platforms_are_never_fuzzy_joined() {
    let s = sets(vec![
        rec("Alpha (Part 1).tap", "ZX Spectrum"),
        rec("Beta (Part 2).tap", "ZX Spectrum"),
        rec("Alpha (Part 2).tap", "Commodore 64"),
    ]);
    assert_eq!(s.len(), 3);
}
#[test]
fn directory_and_colliding_labels_cannot_group_unrelated_releases() {
    let mut a = proven(
        rec("/same/Alpha Disk1.d64", "Commodore 64"),
        "alpha",
        Some(1),
        Some(2),
    );
    let mut b = proven(
        rec("/same/Beta Disk2.d64", "Commodore 64"),
        "beta",
        Some(2),
        Some(2),
    );
    for r in [&mut a, &mut b] {
        let mut e = MediaEvidence::new(EvidenceKind::Embedded, "D64 label");
        e.notes.push("Disk label GAME; ID 01".into());
        r.evidence.push(e);
    }
    assert_eq!(sets(vec![a, b]).len(), 2);
}
#[test]
fn unknown_platform_never_groups_by_extension() {
    let mut a = rec("Orbit Disk1.adf", "Amiga");
    let mut b = rec("Orbit Disk2.adf", "Amiga");
    a.platform = None;
    b.platform = None;
    assert_eq!(sets(vec![a, b]).len(), 2);
}
#[test]
fn unknown_total_does_not_use_highest_observed_ordinal() {
    let s = sets(vec![proven(
        rec("Moon Tape1.tap", "ZX Spectrum"),
        "moon",
        Some(1),
        None,
    )]);
    assert_eq!(s[0].state, MediaSetState::UnverifiedSet);
    assert!(has(&s[0], ConflictKind::UnknownCount));
}
#[test]
fn unsupported_format_is_explicit() {
    assert_eq!(
        sets(vec![rec("Orbit.blob", "Amiga")])[0].state,
        MediaSetState::UnsupportedSet
    );
}
#[test]
fn role_aliases_and_ugly_names_keep_evidence_weak() {
    for (label, role) in [
        ("Boot Disk", MediaRole::BootMedia),
        ("Workbench Disk", MediaRole::UtilityMedia),
        ("Save Disk", MediaRole::SaveMedia),
        ("Install Disc", MediaRole::InstallMedia),
        ("Play Disc", MediaRole::PlayMedia),
        ("Loader Tape", MediaRole::BootMedia),
        ("Program Tape", MediaRole::GameMedia),
        ("Data Tape", MediaRole::DataMedia),
        ("Bonus Disc", MediaRole::BonusMedia),
        ("Extras Disc", MediaRole::ExtrasMedia),
    ] {
        let e = filename_evidence(&format!("Orbit ({label})"), Some(MediaFamily::Tape));
        assert_eq!(e.role, Some(role), "{label}");
        assert_eq!(e.provenance.kind, EvidenceKind::Filename);
    }
}
#[test]
fn ordinal_spelling_corpus() {
    for name in [
        "Orbit (Disc 1)",
        "Orbit Disk One",
        "Orbit.CD1",
        "Orbit_CD_1",
        "Orbit D1",
        "Orbit Tape One",
        "Orbit Cassette 1",
        "Orbit (1 of 2)",
        "Orbit (1of2)",
        "Orbit (1-2)",
        "Orbit (Part One)",
        "Orbit Reel 1",
    ] {
        let e = filename_evidence(name, Some(MediaFamily::Optical));
        assert_eq!(
            e.ordinal.as_ref().map(|o| o.number),
            Some(1),
            "{name}: {e:?}"
        );
        assert_eq!(e.title.as_deref(), Some("orbit"), "{name}");
    }
    for name in ["Orbit Side A", "Orbit Side 1"] {
        assert_eq!(
            filename_evidence(name, None).side,
            Some(MediaSide { number: 1 })
        );
    }
    assert_eq!(
        filename_evidence("Game Disk 1", None).title.as_deref(),
        Some("game")
    );
}
#[test]
fn ambiguous_parts_are_not_silently_chosen() {
    let e = filename_evidence("Orbit (Part 1) (Part 2)", Some(MediaFamily::Tape));
    assert!(e.ordinal.is_none());
    assert!(e.notes.iter().any(|n| n.contains("Conflicting")));
    let s = sets(vec![rec("Orbit (Part 1) (Part 2).tap", "ZX Spectrum")]);
    assert_eq!(s[0].state, MediaSetState::ConflictingSet);
}
#[test]
fn competing_second_disk_is_ambiguous() {
    let mut records = floppies(&[1, 2], 2);
    let mut b = records[1].clone();
    b.source.path = "competing.adf".into();
    b.evidence.last_mut().unwrap().medium = Some(IdentityKey::new("fixture-medium", "different"));
    records.push(b);
    assert_eq!(sets(records)[0].state, MediaSetState::AmbiguousSet);
}
#[test]
fn cyclic_or_missing_load_dependencies_block_plans() {
    let mut records = role_pair(MediaFamily::Tape);
    let id = records[0].evidence.last().unwrap().medium.clone().unwrap();
    let second = records[1].evidence.last().unwrap().medium.clone().unwrap();
    records[0]
        .evidence
        .last_mut()
        .unwrap()
        .relationships
        .push(MediaRelationship {
            target: second,
            kind: TransitionKind::LoaderToProgram,
        });
    records[1]
        .evidence
        .last_mut()
        .unwrap()
        .relationships
        .push(MediaRelationship {
            target: id,
            kind: TransitionKind::LoaderToProgram,
        });
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::ConflictingSet);
    assert!(!media_swap_plan(&s[0], None).blockers.is_empty());
}
#[test]
fn wrong_profile_cannot_select_a_representation() {
    let s = sets(optical(&[1], 1));
    let p = MediaProfile {
        id: "wrong".into(),
        platform: "Amiga".into(),
        supported_formats: BTreeSet::new(),
        preferred_formats: Vec::new(),
        readiness_hint: Some("Ready".into()),
    };
    assert!(
        media_swap_plan(&s[0], Some(&p))
            .blockers
            .iter()
            .any(|c| c.kind == ConflictKind::PlatformConflict)
    );
}
#[test]
fn current_verified_dat_summary_is_consumed_without_new_authority() {
    use crate::dat::library_identity_summary::*;
    let summary = |n| LibraryDatIdentitySummary {
        verification_state: DatVerificationState::VerifiedSingleMatch {
            algorithm: "SHA-1".into(),
        },
        source: DatSourceProvenance {
            source_id: "redump-fixture".into(),
            source_name: "Fixture".into(),
            ecosystem: None,
            variant: None,
            source_revision: Some("v1".into()),
            author: None,
            catalogue_names: vec![],
            dat_path: "fixture.xml".into(),
        },
        canonical: DatCanonicalIdentity {
            canonical_dat_name: Some(format!("Orbit (Disc {n} of 2)")),
            canonical_rom_name: Some(format!("disc{n}.bin")),
            region: None,
            revision: None,
        },
        hash_evidence: DatHashEvidenceSummary {
            matched_algorithm: Some("SHA-1".into()),
            matched_value: Some("a".repeat(40)),
            available_algorithms: vec!["SHA-1".into()],
        },
        provenance_freshness: DatProvenanceFreshness::Current,
        ambiguous_candidates: vec![],
        candidate_provenance: vec![],
        set_dependency: DatSetDependencySummary::Pending {
            reason: "not needed for topology".into(),
        },
    };
    let mut a = rec("mystery1.chd", "PSX");
    let mut b = rec("unrelatedname2.chd", "PSX");
    attach_dat_identity(&mut a, &summary(1));
    attach_dat_identity(&mut b, &summary(2));
    assert_eq!(sets(vec![a, b])[0].state, MediaSetState::CompleteSet);
    let mut stale = summary(1);
    stale.provenance_freshness = DatProvenanceFreshness::Stale;
    let mut r = rec("Orbit.chd", "PSX");
    attach_dat_identity(&mut r, &stale);
    assert!(
        r.evidence
            .iter()
            .all(|e| e.provenance.kind != EvidenceKind::TrustedDat)
    );
}
#[test]
fn native_dolphin_identity_uses_existing_reader() {
    let dir = tempfile::tempdir().unwrap();
    let mut records = Vec::new();
    for disc in 0..=1u8 {
        let path = dir.path().join(format!("Mystery{}.gcm", disc));
        let mut bytes = vec![0u8; 0x100];
        bytes[..6].copy_from_slice(b"GABC01");
        bytes[6] = disc;
        bytes[7] = 2;
        bytes[0x1c..0x20].copy_from_slice(&0xc2339f3du32.to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        let native =
            crate::game_identity::inspect_catalogued_game_identity(&path, Some("GameCube"));
        let mut r = rec(path.to_str().unwrap(), "GameCube");
        attach_native_identity(&mut r, &native);
        records.push(r);
    }
    let s = sets(records);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].members[0].ordinal.as_ref().unwrap().number, 1);
    assert_eq!(s[0].members[1].ordinal.as_ref().unwrap().number, 2);
    assert_eq!(s[0].state, MediaSetState::UnverifiedSet);
}
#[test]
fn deep_tape_adapter_preserves_entries_without_inventing_release() {
    let mut bytes = vec![19, 0, 0, 0];
    bytes.extend_from_slice(b"SYNTHETIC ");
    bytes.extend_from_slice(&[2, 0, 10, 0, 0, 0]);
    let checksum = bytes[2..].iter().fold(0, |sum, b| sum ^ b);
    bytes.push(checksum);
    let analysis = crate::tape_analysis::analyze_tape(&bytes).unwrap();
    let mut r = rec("Unknown.tap", "ZX Spectrum");
    attach_tape_analysis(&mut r, &analysis);
    assert!(
        r.evidence
            .last()
            .unwrap()
            .notes
            .iter()
            .any(|n| n.contains("SYNTHETIC"))
    );
    assert!(r.evidence.last().unwrap().release.is_none());
}
#[test]
fn cue_companions_are_not_counted_as_discs_and_inputs_are_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let cue = dir.path().join("Orbit (Disc 1 of 1).cue");
    let bin = dir.path().join("track.bin");
    std::fs::write(
        &cue,
        "FILE \"track.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(&bin, [0u8; 2048]).unwrap();
    let before = std::fs::read(&bin).unwrap();
    let limits = InspectionLimits {
        native_optical: false,
        ..Default::default()
    };
    let records = inspect_paths(
        &[cue.clone(), bin.clone()],
        Some("PSX"),
        &crate::safe_read::TrustedRoots::none(),
        &limits,
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source.path, cue);
    assert_eq!(std::fs::read(bin).unwrap(), before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}
#[test]
fn archive_member_locators_are_not_collapsed_by_container_path() {
    let mut a = proven(rec("/source/set.zip", "Amiga"), "archive", Some(1), Some(2));
    let mut b = a.clone();
    for (i, r) in [&mut a, &mut b].into_iter().enumerate() {
        r.family = Some(MediaFamily::Floppy);
        r.format = "adf".into();
        r.source.archive_member = Some(ArchiveMemberLocator {
            index: i,
            name_bytes: format!("disk{}.adf", i + 1).into_bytes(),
        });
        let e = r.evidence.last_mut().unwrap();
        e.ordinal = Some(MediaOrdinal {
            number: i as u16 + 1,
            unit: OrdinalUnit::Disk,
        });
        e.expected_count = Some(ExpectedCount {
            count: 2,
            unit: OrdinalUnit::Disk,
        });
        e.medium = Some(IdentityKey::new("fixture-medium", format!("m{i}")));
    }
    let s = sets(vec![a, b]);
    assert_eq!(s[0].members.len(), 2);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
}
#[test]
fn hundred_thousand_record_index_benchmark() {
    use std::time::Instant;
    let start = Instant::now();
    let mut records = Vec::with_capacity(100_000);
    for group in 0..25_000 {
        let (platform, ext, label) = match group % 3 {
            0 => ("PSX", "chd", "Disc"),
            1 => ("Amiga", "adf", "Disk"),
            _ => ("ZX Spectrum", "tap", "Tape"),
        };
        for n in 1..=4 {
            records.push(proven(
                rec(
                    &format!("/source/{n}/Synthetic Release {group} ({label} {n} of 4).{ext}"),
                    platform,
                ),
                &format!("release-{group}"),
                Some(n),
                Some(4),
            ));
        }
    }
    let setup = start.elapsed();
    let now = Instant::now();
    let index = index_media(records);
    let indexing = now.elapsed();
    let now = Instant::now();
    let report = resolve_index(index);
    let grouping = now.elapsed();
    assert_eq!(report.sets.len(), 25_000);
    assert!(
        report
            .sets
            .iter()
            .all(|s| s.state == MediaSetState::CompleteSet)
    );
    assert!(report.stats.candidate_comparisons <= 100_000 * 2);
    eprintln!(
        "TOPOLOGY_BENCH records=100000 sets={} setup_ms={} indexing_ms={} grouping_ms={} comparisons={}",
        report.sets.len(),
        setup.as_millis(),
        indexing.as_millis(),
        grouping.as_millis(),
        report.stats.candidate_comparisons
    );
}

#[test]
fn unnumbered_observed_medium_is_uncertain_not_definitely_missing() {
    let mut records = optical(&[1], 2);
    let mut unknown = proven(rec("Unknown.chd", "PSX"), "orbit", None, Some(2));
    unknown.evidence.last_mut().unwrap().medium =
        Some(IdentityKey::new("fixture-medium", "unknown"));
    records.push(unknown);
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::UnverifiedSet);
    assert!(has(&s[0], ConflictKind::UnknownOrdinal));
    assert!(!has(&s[0], ConflictKind::MissingMedium));
}
#[test]
fn trusted_release_and_count_do_not_prove_filename_positions() {
    let mut records = optical(&[1, 2], 2);
    for r in &mut records {
        r.evidence.last_mut().unwrap().ordinal = None;
    }
    assert_eq!(sets(records)[0].state, MediaSetState::UnverifiedSet);
}
#[test]
fn unverified_source_is_not_declared_missing() {
    let mut records = optical(&[1, 2], 2);
    records[1].availability = MediaAvailability::Unverified;
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::UnverifiedSet);
    assert!(!has(&s[0], ConflictKind::MissingMedium));
}
#[test]
fn side_specific_roles_do_not_create_extra_physical_media() {
    let mut records = Vec::new();
    for side in 1..=2 {
        let mut r = side_record(MediaFamily::Tape, 1, side);
        let e = r.evidence.last_mut().unwrap();
        e.expected_count.as_mut().unwrap().count = 1;
        e.role = Some(if side == 1 {
            MediaRole::BootMedia
        } else {
            MediaRole::DataMedia
        });
        records.push(r);
    }
    let s = sets(records);
    assert_eq!(s[0].members.len(), 1);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    let p = media_swap_plan(&s[0], None);
    assert_eq!(p.ordered_media[0].role, MediaRole::BootMedia);
    assert_eq!(p.ordered_media[1].role, MediaRole::DataMedia);
    assert_eq!(p.transitions[0].kind, TransitionKind::ChangeSide);
}
#[test]
fn unknown_side_layout_does_not_override_a_known_layout() {
    let mut r = side_record(MediaFamily::Tape, 1, 1);
    let mut e = MediaEvidence::new(EvidenceKind::VerifiedNative, "no side fact");
    e.side_layout = Some(SideLayout::Unknown);
    r.evidence.push(e);
    assert!(!has(&sets(vec![r])[0], ConflictKind::SideConflict));
}
#[test]
fn explicit_load_relationship_orders_unnumbered_media() {
    let mut records = role_pair(MediaFamily::Tape);
    let target = records[0].evidence.last().unwrap().medium.clone().unwrap();
    records[1]
        .evidence
        .last_mut()
        .unwrap()
        .relationships
        .push(MediaRelationship {
            target,
            kind: TransitionKind::LoadPart,
        });
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::CompleteSet);
    let p = media_swap_plan(&s[0], None);
    assert_eq!(p.ordered_media[0].role, MediaRole::GameMedia);
    assert_eq!(p.transitions[0].kind, TransitionKind::LoadPart);
}
#[test]
fn existing_canonical_optical_fingerprints_control_equivalence() {
    use crate::optical_fingerprint::*;
    let mut a = rec("Proof (Disc 1 of 1).cue", "PSX");
    let mut b = rec("Proof (Disc 1 of 1).chd", "PSX");
    for (r, representation) in [
        (&mut a, OpticalRepresentation::CueBin),
        (&mut b, OpticalRepresentation::Chd),
    ] {
        let f = CanonicalOpticalFingerprint {
            schema: "fixture-optical-v1",
            canonical_sha256: "a".repeat(64),
            structure: OpticalDiscStructure {
                track_count: 1,
                logical_sector_size: 2048,
                logical_sector_count: 10,
                track_mode: OpticalTrackMode::Mode1_2048,
            },
            source: r.source.path.clone(),
            representation,
        };
        attach_optical_fingerprint(r, &f);
    }
    let s = sets(vec![a.clone(), b.clone()]);
    assert_eq!(s[0].members.len(), 1);
    assert_eq!(s[0].members[0].representations.len(), 2);
    b.evidence
        .last_mut()
        .unwrap()
        .medium
        .as_mut()
        .unwrap()
        .value
        .push('x');
    assert_eq!(sets(vec![a, b])[0].state, MediaSetState::AmbiguousSet);
}
#[test]
fn compact_count_and_revision_tokens_keep_variants_separate() {
    let e = filename_evidence("Quest_Disk1-2_RevA", Some(MediaFamily::Floppy));
    assert_eq!(e.ordinal.unwrap().number, 1);
    assert_eq!(e.expected_count.unwrap().count, 2);
    assert_eq!(e.variant.revision.as_deref(), Some("a"));
    assert_eq!(e.title.as_deref(), Some("quest"));
    let a = rec("Quest v1.1 Disk 1.adf", "Amiga");
    let b = rec("Quest v1.2 Disk 2.adf", "Amiga");
    assert_eq!(sets(vec![a, b]).len(), 2);
}
#[test]
fn cyclic_and_unsafe_playlists_are_reported_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.m3u");
    let b = dir.path().join("b.m3u");
    std::fs::write(&a, "b.m3u\n../escape.iso\n").unwrap();
    std::fs::write(&b, "a.m3u\n").unwrap();
    let records = inspect_paths(
        std::slice::from_ref(&a),
        Some("PSX"),
        &crate::safe_read::TrustedRoots::none(),
        &InspectionLimits::default(),
    );
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .any(|r| r.warnings.iter().any(|w| w.contains("cyclic")))
    );
    assert!(
        sets(records)
            .iter()
            .all(|s| s.state == MediaSetState::UnsupportedSet)
    );
    assert_eq!(
        std::fs::read_to_string(a).unwrap(),
        "b.m3u\n../escape.iso\n"
    );
    assert_eq!(std::fs::read_to_string(b).unwrap(), "a.m3u\n");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}
#[test]
fn partial_manifest_cannot_hide_an_expected_disc_gap() {
    let mut r = optical(&[1], 2).remove(0);
    let e = r.evidence.last_mut().unwrap();
    e.requirements.push(MediumRequirement {
        ordinal: e.ordinal.clone(),
        side: None,
        role: None,
        medium: None,
        optional: false,
    });
    let s = sets(vec![r]);
    assert_eq!(s[0].state, MediaSetState::IncompleteSet);
    assert!(has(&s[0], ConflictKind::MissingMedium));
}
#[test]
fn dolphin_raw_region_byte_reuses_existing_locale_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Quest (USA) (Disc 1).gcm");
    let mut bytes = vec![0u8; 0x100];
    bytes[..6].copy_from_slice(b"GABE01");
    bytes[0x1c..0x20].copy_from_slice(&0xc2339f3du32.to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
    let native = crate::game_identity::inspect_catalogued_game_identity(&path, Some("GameCube"));
    let mut r = rec(path.to_str().unwrap(), "GameCube");
    attach_native_identity(&mut r, &native);
    let s = sets(vec![r]);
    assert!(!has(&s[0], ConflictKind::VariantConflict));
    assert_eq!(s[0].variant.region.as_deref(), Some("usa"));
}
#[test]
fn commodore_t64_directory_and_pulse_limitations_are_preserved() {
    let mut pulse = b"C64-TAPE-RAW".to_vec();
    pulse.extend([2, 0, 0, 0]);
    pulse.extend(1u32.to_le_bytes());
    pulse.push(32);
    let a = crate::tape_analysis::analyze_tape(&pulse).unwrap();
    assert!(a.entries.is_empty());
    let mut r = rec("Pulse Tape 1.tap", "Commodore 64");
    attach_tape_analysis(&mut r, &a);
    assert!(
        r.evidence
            .last()
            .unwrap()
            .notes
            .iter()
            .any(|n| n.contains("Pulse-only"))
    );
    assert!(r.evidence.last().unwrap().release.is_none());
    let mut bytes = vec![0u8; 98];
    let signature = b"C64S tape image file";
    bytes[..signature.len()].copy_from_slice(signature);
    bytes[32..34].copy_from_slice(&0x0101u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&1u16.to_le_bytes());
    bytes[36..38].copy_from_slice(&1u16.to_le_bytes());
    bytes[64] = 1;
    bytes[65] = 0x82;
    bytes[66..68].copy_from_slice(&0x0801u16.to_le_bytes());
    bytes[68..70].copy_from_slice(&0x0803u16.to_le_bytes());
    bytes[72..76].copy_from_slice(&96u32.to_le_bytes());
    bytes[80..89].copy_from_slice(b"SYNTHETIC");
    let a = crate::tape_analysis::analyze_tape(&bytes).unwrap();
    assert_eq!(a.entries[0].length, 2);
    let mut r = rec("Directory Tape 1.t64", "Commodore 64");
    attach_tape_analysis(&mut r, &a);
    assert!(
        r.evidence
            .last()
            .unwrap()
            .notes
            .iter()
            .any(|n| n.contains("SYNTHETIC") && n.contains("2049"))
    );
    assert!(r.evidence.last().unwrap().medium.is_none());
}
#[test]
fn unsupported_declared_format_cannot_be_made_complete_by_authority() {
    let mut r = optical(&[1], 1).remove(0);
    r.format = "unknown".into();
    assert_eq!(sets(vec![r])[0].state, MediaSetState::UnsupportedSet);
}
#[test]
fn competing_unnumbered_boot_media_do_not_prove_a_unique_swap_plan() {
    let mut records = role_pair(MediaFamily::Floppy);
    let mut extra = records[0].clone();
    extra.source.path = "different-boot.adf".into();
    extra.evidence.last_mut().unwrap().medium =
        Some(IdentityKey::new("fixture-medium", "another-boot"));
    records.push(extra);
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::AmbiguousSet);
    assert!(has(&s[0], ConflictKind::CompetingMedia));
}
#[test]
fn a_manifest_of_medium_ids_does_not_invent_unknown_positions() {
    let mut records = role_pair(MediaFamily::Tape);
    let requirements = records
        .iter()
        .map(|r| MediumRequirement {
            medium: r.evidence.last().unwrap().medium.clone(),
            ordinal: None,
            side: None,
            role: None,
            optional: false,
        })
        .collect::<Vec<_>>();
    for r in &mut records {
        let e = r.evidence.last_mut().unwrap();
        e.role = None;
        e.requirements = requirements.clone();
    }
    let s = sets(records);
    assert_eq!(s[0].state, MediaSetState::UnverifiedSet);
    assert!(has(&s[0], ConflictKind::UnknownOrdinal));
    assert!(!media_swap_plan(&s[0], None).blockers.is_empty());
}
