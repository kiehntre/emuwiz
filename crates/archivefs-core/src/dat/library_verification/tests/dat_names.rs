//! S1: a rename may use a saved catalogue name only while the saved check is
//! current, unique and a single match.
use super::*;
use crate::database::{
    SavedDatEvidenceStatus as Status, SavedDatNameRow, VerifiedDatName, decide_verified_dat_name,
};

const MATCHING: &[u8] = b"matching synthetic tape";

fn name_of(f: &Fixture, id: i64) -> VerifiedDatName {
    f.sized();
    Database::open_read_only(&f.db)
        .unwrap()
        .verified_dat_name_for_archive(id, &f.configured())
        .unwrap()
}

fn verified_fixture() -> Fixture {
    let f = Fixture::new();
    f.add(1, "a.tap", Some(MATCHING), Some("ZX Spectrum"));
    f.add(2, "b.tap", Some(b"different tape"), Some("ZX Spectrum"));
    // No system: never checked, so no saved row at all.
    f.add(3, "c.tap", Some(b"never checked"), None);
    let t = f.run();
    assert_eq!((t.verified, t.no_match), (1, 1));
    f
}

#[test]
fn a_current_single_match_gives_the_catalogue_file_name() {
    let f = verified_fixture();
    assert_eq!(
        name_of(&f, 1),
        VerifiedDatName::Usable {
            rom_name: "Synthetic.tap".into(),
            game_name: "Synthetic".into(),
            dat_source_id: "synthetic".into(),
        }
    );
    // A genuine "not in this catalogue" and an unchecked game give no name.
    assert_eq!(name_of(&f, 2), VerifiedDatName::None);
    assert_eq!(name_of(&f, 3), VerifiedDatName::None);
}

#[test]
fn a_file_changed_after_the_check_gives_no_name() {
    let f = verified_fixture();
    std::fs::write(f.root.join("a.tap"), b"rewritten after the check!!").unwrap();
    assert!(
        matches!(name_of(&f, 1), VerifiedDatName::NotUsable(r) if r.contains("file has changed"))
    );
    // The same bytes written back are a new object: still not usable.
    std::fs::remove_file(f.root.join("a.tap")).unwrap();
    std::fs::write(f.root.join("a.tap"), MATCHING).unwrap();
    assert!(matches!(name_of(&f, 1), VerifiedDatName::NotUsable(_)));
}

#[test]
fn a_changed_or_missing_catalogue_gives_no_name() {
    let mut f = verified_fixture();
    f.write_dat("Sinclair ZX Spectrum - Games - [TAP] (revised)");
    assert!(matches!(name_of(&f, 1), VerifiedDatName::NotUsable(r) if r.contains("catalogue")));
    // A catalogue that is no longer configured at all.
    let unconfigured = Database::open_read_only(&f.db)
        .unwrap()
        .verified_dat_name_for_archive(1, &Default::default())
        .unwrap();
    assert!(matches!(unconfigured, VerifiedDatName::NotUsable(r) if r.contains("not available")));
}

#[test]
fn evidence_saved_before_full_proof_gives_no_name() {
    let f = verified_fixture();
    f.strip_proof();
    assert!(
        matches!(name_of(&f, 1), VerifiedDatName::NotUsable(r) if r.contains("check the game again"))
    );
}

fn row(source: &str, status: Status, verdict: &str, rom: Option<&str>) -> SavedDatNameRow {
    SavedDatNameRow {
        dat_source_id: source.into(),
        status,
        saved_verdict: verdict.into(),
        canonical_rom_name: rom.map(Into::into),
        canonical_dat_name: rom.map(|r| r.trim_end_matches(".gba").into()),
    }
}

#[test]
fn the_rule_refuses_ambiguity_disagreement_and_missing_names() {
    use VerifiedDatName as V;
    let ok = row(
        "a",
        Status::Current,
        "verified_single_match",
        Some("Game (USA).gba"),
    );
    assert!(matches!(
        decide_verified_dat_name(std::slice::from_ref(&ok)),
        V::Usable { .. }
    ));
    // Two catalogues agreeing is still one name.
    let same = row(
        "b",
        Status::Current,
        "verified_single_match",
        Some("Game (USA).gba"),
    );
    assert!(matches!(
        decide_verified_dat_name(&[ok.clone(), same]),
        V::Usable { .. }
    ));
    // Disagreeing catalogues: never pick one.
    let other = row(
        "b",
        Status::Current,
        "verified_single_match",
        Some("Game (Europe).gba"),
    );
    assert!(
        matches!(decide_verified_dat_name(&[ok.clone(), other]), V::NotUsable(r) if r.contains("disagree"))
    );
    // A current "more than one possible match" beside a match: refuse.
    let conflict = row("b", Status::Current, "conflicting", None);
    assert!(matches!(
        decide_verified_dat_name(&[ok.clone(), conflict]),
        V::NotUsable(_)
    ));
    // A match without a recorded file name: refuse rather than guess.
    let unnamed = row("a", Status::Current, "verified_single_match", None);
    assert!(matches!(
        decide_verified_dat_name(&[unnamed]),
        V::NotUsable(_)
    ));
    // A stale match never counts, even beside a current no-match.
    let stale = row(
        "a",
        Status::CatalogueChanged,
        "verified_single_match",
        Some("Game (USA).gba"),
    );
    let negative = row("b", Status::Current, "no_match", None);
    assert!(matches!(
        decide_verified_dat_name(&[stale, negative.clone()]),
        V::NotUsable(_)
    ));
    // A current no-match alone, or nothing at all: no name and no complaint.
    assert_eq!(decide_verified_dat_name(&[negative]), V::None);
    assert_eq!(decide_verified_dat_name(&[]), V::None);
}
