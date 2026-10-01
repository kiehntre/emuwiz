//! Synthetic fixtures only: tiny generated AmigaDOS floppies, no game images.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::game_identity::{IdentityKind, IdentityStatus};
use crate::media_set::{
    Equivalence, EvidenceKind, ExpectedCount, IdentityKey, MediaAvailability, MediaEvidence,
    MediaOrdinal, MediaRecord, MediaSet, MediaSetState, OrdinalUnit, index_media, media_record,
    resolve_index,
};
use crate::patch_manager::CheatReviewChoice;

fn p32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// A minimal valid flat AmigaDOS floppy; the volume label makes each one distinct.
fn flat_adf(volume: &str) -> Vec<u8> {
    const SECTORS: usize = 128;
    const ROOT: usize = SECTORS / 2;
    let mut img = vec![0_u8; SECTORS * 512];
    img[..4].copy_from_slice(b"DOS\x01");
    p32(&mut img, 8, ROOT as u32);
    let mut root = [0_u8; 512];
    p32(&mut root, 0, 2);
    p32(&mut root, 12, 72);
    root[0x1B0] = volume.len() as u8;
    root[0x1B1..0x1B1 + volume.len()].copy_from_slice(volume.as_bytes());
    p32(&mut root, 508, 1);
    let mut sum = 0_u32;
    for offset in (0..512).step_by(4) {
        if offset != 20 {
            sum = sum.wrapping_add(u32::from_be_bytes(
                root[offset..offset + 4].try_into().unwrap(),
            ));
        }
    }
    p32(&mut root, 20, (sum as i32).wrapping_neg() as u32);
    img[ROOT * 512..(ROOT + 1) * 512].copy_from_slice(&root);
    img
}

struct Fx {
    dir: tempfile::TempDir,
}

impl Fx {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn adf(&self, file: &str, volume: &str) -> AdfMedia {
        let path = self.dir.path().join(file);
        std::fs::write(&path, flat_adf(volume)).unwrap();
        inspect_adf_media(&path, None).unwrap()
    }
}

fn record(
    media: &AdfMedia,
    release: &str,
    disk: u16,
    total: u16,
    revision: Option<&str>,
) -> MediaRecord {
    let mut r = media_record(&media.path, Some("Amiga"));
    r.availability = MediaAvailability::Observed;
    let mut e = MediaEvidence::new(EvidenceKind::TrustedDat, "synthetic authority v1");
    e.release = Some(IdentityKey::new("fixture-release", release));
    e.medium = Some(IdentityKey::new(
        "fixture-medium",
        format!("{release}:{disk}"),
    ));
    e.equivalence = Equivalence::AuthorityMapping;
    e.ordinal = Some(MediaOrdinal {
        number: disk,
        unit: OrdinalUnit::Disk,
    });
    e.expected_count = Some(ExpectedCount {
        count: total,
        unit: OrdinalUnit::Disk,
    });
    e.variant.revision = revision.map(str::to_owned);
    r.evidence.push(e);
    r
}

fn set_of(records: Vec<MediaRecord>) -> MediaSet {
    resolve_index(index_media(records)).sets.remove(0)
}

fn entry(
    media: &[(&AdfMedia, Option<u16>)],
    scope: &str,
    disk: Option<u16>,
    writes: Value,
) -> Value {
    let mut target = json!({
        "media": media.iter().map(|(m, d)| match d {
            Some(d) => json!({"sha256": m.sha256, "disk": d}),
            None => json!({"sha256": m.sha256}),
        }).collect::<Vec<_>>(),
        "scope": scope,
    });
    if let Some(d) = disk {
        target["disk"] = json!(d);
    }
    json!({
        "platform": "Amiga",
        "title": "Infinite Lives",
        "mechanism": "memory_write",
        "target": target,
        "writes": writes,
        "source": {"name": "synthetic"},
    })
}

fn lives() -> Value {
    json!([{"width": "word", "address": "0x0001A2B4", "value": 9}])
}

fn import(entries: Vec<Value>) -> AmigaTrainerImport {
    let doc = json!({"schema": AMIGA_TRAINER_SCHEMA, "entries": entries});
    import_amiga_trainers_from_bytes(doc.to_string().as_bytes(), "synthetic.json").unwrap()
}

fn plan(imp: &AmigaTrainerImport, ctx: &AmigaTrainerContext<'_>) -> AmigaTrainerPlan {
    plan_amiga_adf_trainers(imp, ctx, &BTreeMap::new())
}

fn one(fx_media: &AdfMedia, entry: Value) -> AmigaTrainerAssessment {
    let imp = import(vec![entry]);
    let ctx = AmigaTrainerContext::for_media(fx_media);
    plan(&imp, &ctx).assessments.remove(0)
}

fn disk1_entry(m: &AdfMedia) -> Value {
    entry(&[(m, Some(1))], "disk", Some(1), lives())
}

// ---- identity ---------------------------------------------------------------

#[test]
fn exact_verified_adf_reaches_runtime_required_and_never_preparable() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let a = one(&m, disk1_entry(&m));
    assert_eq!(
        a.status,
        AmigaTrainerStatus::RequiresEmulatorRuntime,
        "{a:?}"
    );
    assert!(a.blockers.is_empty());
    assert_eq!(a.selection, AmigaTrainerSelection::Selected);
    let imp = import(vec![disk1_entry(&m)]);
    let p = plan(&imp, &AmigaTrainerContext::for_media(&m));
    assert_eq!(p.selected, vec![0]);
    assert_eq!(p.count(AmigaTrainerStatus::Preparable), 0);
    assert!(!p.requires_scratch_copy);
}

#[test]
fn unverified_identity_is_refused() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let mut ctx = AmigaTrainerContext::for_media(&m);
    for f in &mut ctx.facts {
        if f.kind == IdentityKind::LooseRomSha256 {
            f.status = IdentityStatus::Candidate;
        }
    }
    let p = plan(&import(vec![disk1_entry(&m)]), &ctx);
    let a = &p.assessments[0];
    assert_eq!(a.status, AmigaTrainerStatus::PreviewOnly);
    assert!(
        a.blockers
            .contains(&AmigaTrainerBlocker::IdentityNotVerified)
    );
    assert!(p.selected.is_empty());
}

#[test]
fn empty_identity_is_refused() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let mut ctx = AmigaTrainerContext::for_media(&m);
    for f in &mut ctx.facts {
        if f.kind == IdentityKind::LooseRomSha256 {
            f.value = Some("  ".into());
        }
    }
    let a = plan(&import(vec![disk1_entry(&m)]), &ctx)
        .assessments
        .remove(0);
    assert_eq!(a.status, AmigaTrainerStatus::PreviewOnly);
    assert!(a.blockers.contains(&AmigaTrainerBlocker::IdentityEmpty));
    ctx.facts.clear();
    let a = plan(&import(vec![disk1_entry(&m)]), &ctx)
        .assessments
        .remove(0);
    assert!(
        a.blockers
            .contains(&AmigaTrainerBlocker::IdentityNotVerified)
    );
}

#[test]
fn identity_for_other_bytes_is_refused() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let other = fx.adf("b.adf", "Beta");
    let mut ctx = AmigaTrainerContext::for_media(&m);
    ctx.facts = other.identity_facts();
    let a = plan(&import(vec![disk1_entry(&m)]), &ctx)
        .assessments
        .remove(0);
    assert!(a.blockers.contains(&AmigaTrainerBlocker::IdentityMismatch));
    assert_eq!(a.status, AmigaTrainerStatus::PreviewOnly);
}

#[test]
fn filename_and_title_similarity_never_authorise() {
    let fx = Fx::new();
    let real = fx.adf("Infinite Lives.adf", "Alpha");
    let lookalike = fx.adf("Infinite Lives (copy).adf", "Alpha2");
    // Trainer targets `real`; the lookalike has the same title/name stem.
    let a = one(&lookalike, disk1_entry(&real));
    assert_eq!(a.status, AmigaTrainerStatus::Unsupported);
    assert_eq!(a.blockers, vec![AmigaTrainerBlocker::MediaNotTargeted]);
}

#[test]
fn wrong_platform_is_refused() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let mut e = disk1_entry(&m);
    e["platform"] = json!("Atari ST");
    let a = one(&m, e);
    assert_eq!(a.status, AmigaTrainerStatus::Unsupported);
    assert!(matches!(
        a.blockers[0],
        AmigaTrainerBlocker::PlatformMismatch { .. }
    ));
}

#[test]
fn non_amiga_bytes_are_not_inspected_as_adf() {
    let fx = Fx::new();
    let path = fx.dir.path().join("fake.adf");
    std::fs::write(&path, vec![0_u8; 901_120]).unwrap();
    assert!(matches!(
        inspect_adf_media(&path, None),
        Err(AdfMediaError::NotAnAmigaFloppy(_))
    ));
}

// ---- release / revision -----------------------------------------------------

#[test]
fn wrong_or_unknown_revision_is_handled_through_canonical_applicability() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let set = set_of(vec![record(&m, "clock", 1, 1, Some("A"))]);
    let ctx = AmigaTrainerContext::for_media(&m).with_media_set(&set);

    let mut wrong = disk1_entry(&m);
    wrong["target"]["revision"] = json!("B");
    let a = plan(&import(vec![wrong]), &ctx).assessments.remove(0);
    assert_eq!(a.status, AmigaTrainerStatus::Unsupported, "{a:?}");
    assert!(a.blockers.contains(&AmigaTrainerBlocker::RevisionMismatch));

    let mut right = disk1_entry(&m);
    right["target"]["revision"] = json!("A");
    let a = plan(&import(vec![right]), &ctx).assessments.remove(0);
    assert_eq!(
        a.status,
        AmigaTrainerStatus::RequiresEmulatorRuntime,
        "{a:?}"
    );

    // A revision-scoped trainer with no set evidence cannot be confirmed.
    let mut scoped = entry(&[(&m, None)], "revision", None, lives());
    scoped["target"]["revision"] = json!("A");
    let a = one(&m, scoped);
    assert_eq!(a.status, AmigaTrainerStatus::PreviewOnly);
}

#[test]
fn wrong_release_is_refused() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let set = set_of(vec![record(&m, "clock", 1, 1, None)]);
    let ctx = AmigaTrainerContext::for_media(&m).with_media_set(&set);
    let mut e = disk1_entry(&m);
    e["target"]["release"] = json!({"namespace": "fixture-release", "value": "someone-else"});
    let a = plan(&import(vec![e]), &ctx).assessments.remove(0);
    assert_eq!(a.status, AmigaTrainerStatus::Unsupported);
    assert!(a.blockers.contains(&AmigaTrainerBlocker::ReleaseMismatch));
}

// ---- multi-disk -------------------------------------------------------------

fn two_disks(fx: &Fx, rev: Option<&str>) -> (AdfMedia, AdfMedia, MediaSet) {
    let d1 = fx.adf(
        &format!("d1{}.adf", rev.unwrap_or("")),
        &format!("Disk1{}", rev.unwrap_or("")),
    );
    let d2 = fx.adf(
        &format!("d2{}.adf", rev.unwrap_or("")),
        &format!("Disk2{}", rev.unwrap_or("")),
    );
    let set = set_of(vec![
        record(&d1, "clock", 1, 2, rev),
        record(&d2, "clock", 2, 2, rev),
    ]);
    (d1, d2, set)
}

#[test]
fn disk1_trainer_applies_to_disk1_and_never_to_disk2() {
    let fx = Fx::new();
    let (d1, d2, set) = two_disks(&fx, None);
    assert_eq!(set.state, MediaSetState::CompleteSet);
    let e = entry(&[(&d1, Some(1)), (&d2, Some(2))], "disk", Some(1), lives());

    let imp = import(vec![e]);
    let p1 = plan(
        &imp,
        &AmigaTrainerContext::for_media(&d1).with_media_set(&set),
    );
    assert_eq!(
        p1.assessments[0].status,
        AmigaTrainerStatus::RequiresEmulatorRuntime
    );
    assert_eq!(p1.disk_ordinal, Some(1));

    let p2 = plan(
        &imp,
        &AmigaTrainerContext::for_media(&d2).with_media_set(&set),
    );
    let a = &p2.assessments[0];
    assert_eq!(a.status, AmigaTrainerStatus::Unsupported);
    assert!(a.blockers.contains(&AmigaTrainerBlocker::WrongDisk {
        wanted: 1,
        found: 2
    }));
    assert!(p2.selected.is_empty());
}

#[test]
fn disk_scope_without_any_disk_evidence_is_not_assumed() {
    let fx = Fx::new();
    let d1 = fx.adf("d1.adf", "Disk1");
    // The trainer's media entry carries no disk number and there is no set.
    let e = entry(&[(&d1, None)], "disk", Some(1), lives());
    let a = one(&d1, e);
    assert_eq!(a.status, AmigaTrainerStatus::PreviewOnly);
    assert!(a.blockers.contains(&AmigaTrainerBlocker::DiskUnconfirmed));
}

#[test]
fn reordered_disk_set_is_refused() {
    let fx = Fx::new();
    let d1 = fx.adf("d1.adf", "Disk1");
    let d2 = fx.adf("d2.adf", "Disk2");
    // The set says d1 is disk 2 and d2 is disk 1: swapped relative to the trainer.
    let set = set_of(vec![
        record(&d1, "clock", 2, 2, None),
        record(&d2, "clock", 1, 2, None),
    ]);
    let e = entry(&[(&d1, Some(1)), (&d2, Some(2))], "disk", Some(1), lives());
    let p = plan(
        &import(vec![e]),
        &AmigaTrainerContext::for_media(&d1).with_media_set(&set),
    );
    assert_eq!(p.assessments[0].status, AmigaTrainerStatus::Unsupported);
    assert!(
        p.assessments[0]
            .blockers
            .iter()
            .any(|b| matches!(b, AmigaTrainerBlocker::WrongDisk { .. }))
    );
}

#[test]
fn different_revisions_with_different_hashes_do_not_cross_over() {
    let fx = Fx::new();
    let (a1, _a2, _sa) = two_disks(&fx, Some("A"));
    let (b1, _b2, sb) = two_disks(&fx, Some("B"));
    assert_ne!(a1.sha256, b1.sha256);
    // Trainer authored for revision A's disk 1, tried on revision B's disk 1.
    let e = entry(&[(&a1, Some(1))], "disk", Some(1), lives());
    let p = plan(
        &import(vec![e]),
        &AmigaTrainerContext::for_media(&b1).with_media_set(&sb),
    );
    assert_eq!(
        p.assessments[0].blockers,
        vec![AmigaTrainerBlocker::MediaNotTargeted]
    );
    assert_eq!(p.assessments[0].status, AmigaTrainerStatus::Unsupported);
}

#[test]
fn same_title_different_hashes_are_distinct_targets() {
    let fx = Fx::new();
    let x = fx.adf("x.adf", "Same");
    let y = fx.adf("y.adf", "Same ");
    assert_ne!(x.sha256, y.sha256);
    let a = one(&y, disk1_entry(&x));
    assert_eq!(a.blockers, vec![AmigaTrainerBlocker::MediaNotTargeted]);
}

#[test]
fn whole_title_needs_a_complete_verified_set() {
    let fx = Fx::new();
    let d1 = fx.adf("d1.adf", "Disk1");
    let d2 = fx.adf("d2.adf", "Disk2");
    let d3 = fx.adf("d3.adf", "Disk3");
    let whole = |m: &AdfMedia| entry(&[(m, None)], "whole_title", None, lives());

    // No set at all.
    let a = one(&d1, whole(&d1));
    assert!(a.blockers.contains(&AmigaTrainerBlocker::SetRequired));
    assert_eq!(a.status, AmigaTrainerStatus::PreviewOnly);

    // Disk 2 of 3 missing.
    let missing = set_of(vec![
        record(&d1, "clock", 1, 3, None),
        record(&d3, "clock", 3, 3, None),
    ]);
    let a = plan(
        &import(vec![whole(&d1)]),
        &AmigaTrainerContext::for_media(&d1).with_media_set(&missing),
    )
    .assessments
    .remove(0);
    assert_eq!(a.status, AmigaTrainerStatus::PreviewOnly);
    assert!(a.blockers.contains(&AmigaTrainerBlocker::SetIncomplete));

    // Complete.
    let full = set_of(vec![
        record(&d1, "clock", 1, 3, None),
        record(&d2, "clock", 2, 3, None),
        record(&d3, "clock", 3, 3, None),
    ]);
    let a = plan(
        &import(vec![whole(&d2)]),
        &AmigaTrainerContext::for_media(&d2).with_media_set(&full),
    )
    .assessments
    .remove(0);
    assert_eq!(
        a.status,
        AmigaTrainerStatus::RequiresEmulatorRuntime,
        "{a:?}"
    );
}

// ---- duplicates / conflicts -------------------------------------------------

#[test]
fn identical_trainers_collapse_and_keep_provenance() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let imp = import(vec![disk1_entry(&m), disk1_entry(&m)]);
    let p = plan(&imp, &AmigaTrainerContext::for_media(&m));
    assert!(p.conflict_groups.is_empty(), "{p:?}");
    let dups = p
        .assessments
        .iter()
        .filter(|a| matches!(a.selection, AmigaTrainerSelection::DuplicateOf(_)))
        .count();
    assert_eq!(dups, 1, "{p:?}");
    // Both are still present with their own source index.
    assert_eq!(imp.trainers[0].index, 0);
    assert_eq!(imp.trainers[1].index, 1);
}

fn conflicting(m: &AdfMedia) -> AmigaTrainerImport {
    let mut other = disk1_entry(m);
    other["writes"] = json!([{"width": "word", "address": "0x0001A2B4", "value": 3}]);
    import(vec![disk1_entry(m), other])
}

#[test]
fn conflicting_trainers_need_an_explicit_choice() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let imp = conflicting(&m);
    let ctx = AmigaTrainerContext::for_media(&m);
    let p = plan(&imp, &ctx);
    assert_eq!(p.conflict_groups.len(), 1, "{p:?}");
    assert!(p.selected.is_empty());
    assert!(
        p.assessments
            .iter()
            .all(|a| a.selection == AmigaTrainerSelection::NeedsChoice
                && a.status == AmigaTrainerStatus::PreviewOnly)
    );

    let group = p.conflict_groups[0].group_index;
    let chosen = plan_amiga_adf_trainers(
        &imp,
        &ctx,
        &BTreeMap::from([(group, CheatReviewChoice::KeepA)]),
    );
    assert_eq!(chosen.selected.len(), 1, "{chosen:?}");
    let skipped = plan_amiga_adf_trainers(
        &imp,
        &ctx,
        &BTreeMap::from([(group, CheatReviewChoice::Skip)]),
    );
    assert!(skipped.selected.is_empty());
}

// ---- import: malformed, overflow, bounds ------------------------------------

fn import_one(patch: impl FnOnce(&mut Value)) -> AmigaTrainerImport {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let mut e = disk1_entry(&m);
    patch(&mut e);
    import(vec![e, disk1_entry(&m)])
}

#[test]
fn malformed_entries_fail_locally_and_do_not_poison_neighbours() {
    let bad_addresses = [
        json!("0xZZ"),
        json!("-1"),
        json!("1_000"),
        json!(""),
        json!("0x"),
        json!("0x100000000"), // exceeds 32 bits
        json!("99999999999999999999999"),
        json!("0x1A2B3"), // odd address for a word write
    ];
    for bad in bad_addresses {
        let imp = import_one(|e| e["writes"][0]["address"] = bad.clone());
        assert_eq!(imp.trainers.len(), 1, "{bad}");
        assert_eq!(imp.rejected, 1, "{bad}");
        assert_eq!(imp.diagnostics[0].index, 0);
    }
}

#[test]
fn value_overflow_and_end_of_address_space_are_rejected() {
    let imp = import_one(|e| e["writes"] = json!([{"width": "byte", "address": 16, "value": 256}]));
    assert_eq!(imp.rejected, 1);
    let imp =
        import_one(|e| e["writes"] = json!([{"width": "word", "address": 2, "value": "0x10000"}]));
    assert_eq!(imp.rejected, 1);
    let imp = import_one(|e| {
        e["writes"] = json!([{"width": "long", "address": "0xFFFFFFFE", "value": 1}])
    });
    assert_eq!(imp.rejected, 1);
    let imp = import_one(|e| {
        e["writes"] = json!([{"width": "word", "address": 2, "value": 1, "original": "0x1FFFF"}])
    });
    assert_eq!(imp.rejected, 1);
    let imp = import_one(|e| {
        e["writes"] = json!([{"width": "long", "address": "0xFFFFFFFC", "value": "0xFFFFFFFF"}])
    });
    assert_eq!(imp.rejected, 0);
}

#[test]
fn structural_and_free_form_input_is_rejected() {
    // Unknown field, e.g. an attempted command, is refused (deny_unknown_fields).
    let imp = import_one(|e| e["command"] = json!("rm -rf /"));
    assert_eq!(imp.rejected, 1);
    for patch in [
        |e: &mut Value| e["target"]["media"][0]["sha256"] = json!("abc"),
        |e: &mut Value| e["target"]["media"] = json!([]),
        |e: &mut Value| e["title"] = json!("   "),
        |e: &mut Value| e["title"] = json!("x".repeat(MAX_TITLE_BYTES + 1)),
        |e: &mut Value| e["mechanism"] = json!("shell"),
        |e: &mut Value| e["writes"] = json!([]),
        |e: &mut Value| e["target"]["disk"] = json!(0),
        |e: &mut Value| e["target"]["scope"] = json!("everything"),
    ] {
        let imp = import_one(patch);
        assert_eq!(imp.trainers.len(), 1);
        assert_eq!(imp.rejected, 1);
    }
}

#[test]
fn import_is_bounded() {
    let w = json!([{"width": "byte", "address": 1, "value": 1}]);
    let many: Vec<Value> = (0..MAX_WRITES_PER_TRAINER + 1)
        .map(|i| json!({"width": "byte", "address": i, "value": 1}))
        .collect();
    let imp = import_one(|e| e["writes"] = Value::Array(many.clone()));
    assert_eq!(imp.rejected, 1);

    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let mut e = disk1_entry(&m);
    e["writes"] = w;
    let entries = vec![e; MAX_TRAINERS_PER_FILE + 10];
    let doc = json!({"schema": AMIGA_TRAINER_SCHEMA, "entries": entries}).to_string();
    let imp = import_amiga_trainers_from_bytes(doc.as_bytes(), "big.json");
    // Either the whole file is over the byte cap, or the entry count is capped.
    match imp {
        Ok(i) => {
            assert_eq!(i.trainers.len(), MAX_TRAINERS_PER_FILE);
            assert_eq!(i.dropped_over_limit, 10);
        }
        Err(ImportError::TooLarge { .. }) => {}
        Err(other) => panic!("{other}"),
    }

    let huge = vec![b' '; MAX_TRAINER_SOURCE_BYTES + 1];
    assert!(matches!(
        import_amiga_trainers_from_bytes(&huge, "huge.json"),
        Err(ImportError::TooLarge { .. })
    ));
    assert!(matches!(
        import_amiga_trainers_from_bytes(b"{\"schema\":\"other/9\",\"entries\":[]}", "x"),
        Err(ImportError::UnsupportedSchema(_))
    ));
    assert!(matches!(
        import_amiga_trainers_from_bytes(b"not json", "x"),
        Err(ImportError::Malformed(_))
    ));
}

#[test]
fn oversized_adf_is_refused_without_reading_it_whole() {
    let fx = Fx::new();
    let path = fx.dir.path().join("big.adf");
    let f = std::fs::File::create(&path).unwrap();
    f.set_len(ADF_MAX_BYTES + 1).unwrap();
    assert!(matches!(
        inspect_adf_media(&path, None),
        Err(AdfMediaError::TooLarge { .. })
    ));
}

// ---- immutability / alternate media / mechanisms ----------------------------

#[test]
fn planning_never_touches_the_original_image() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let before = std::fs::read(&m.path).unwrap();
    let meta = std::fs::metadata(&m.path).unwrap();
    let dir_before: Vec<PathBuf> = std::fs::read_dir(fx.dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    let p = plan(
        &import(vec![disk1_entry(&m)]),
        &AmigaTrainerContext::for_media(&m),
    );
    assert_eq!(p.source_policy, SourceMediaPolicy::ReadOnlyOriginal);
    assert_eq!(std::fs::read(&m.path).unwrap(), before);
    let after = std::fs::metadata(&m.path).unwrap();
    assert_eq!(after.len(), meta.len());
    assert_eq!(after.modified().unwrap(), meta.modified().unwrap());
    let dir_after: Vec<PathBuf> = std::fs::read_dir(fx.dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dir_before.len(), dir_after.len());
}

#[test]
fn symlinked_adf_is_refused() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let link = fx.dir.path().join("link.adf");
    std::os::unix::fs::symlink(&m.path, &link).unwrap();
    assert!(matches!(
        inspect_adf_media(&link, None),
        Err(AdfMediaError::NotARegularFile)
    ));
}

#[test]
fn pre_trained_adf_is_alternate_media_not_an_applied_cheat() {
    let fx = Fx::new();
    let path = fx.dir.path().join("t.adf");
    std::fs::write(&path, flat_adf("Trained")).unwrap();
    let m = inspect_adf_media(&path, Some("Some Game (1991)(Vendor)(Disk 1 of 2)[t +3]")).unwrap();
    assert_eq!(m.alternate, AlternateMedia::Trained);
    let a = one(&m, disk1_entry(&m));
    assert_eq!(a.status, AmigaTrainerStatus::Unsupported);
    assert!(a.blockers.contains(&AmigaTrainerBlocker::AlternateMedia(
        AlternateMedia::Trained
    )));

    assert_eq!(
        classify_alternate_media("Game (1991)[cr]"),
        AlternateMedia::Cracked
    );
    assert_eq!(
        classify_alternate_media("Game (1991)[h Foo]"),
        AlternateMedia::Hacked
    );
    assert_eq!(
        classify_alternate_media("Game (1991)[m]"),
        AlternateMedia::Modified
    );
    assert_eq!(
        classify_alternate_media("Game (1991)[a]"),
        AlternateMedia::NoFlag
    );
    assert_eq!(
        classify_alternate_media("Game (1991)"),
        AlternateMedia::NoFlag
    );

    // A declaration of a pre-trained disk is never a plan either.
    let fx2 = Fx::new();
    let clean = fx2.adf("c.adf", "Clean");
    let mut e = disk1_entry(&clean);
    e["mechanism"] = json!("trained_disk");
    let a = one(&clean, e);
    assert_eq!(a.status, AmigaTrainerStatus::Unsupported);
    assert_eq!(
        a.blockers,
        vec![AmigaTrainerBlocker::Mechanism(
            AmigaTrainerMechanism::TrainedDisk
        )]
    );
}

#[test]
fn non_memory_mechanisms_are_unsupported_with_a_reason() {
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    for (name, mechanism) in [
        ("boot_menu", AmigaTrainerMechanism::BootMenu),
        (
            "action_replay_code",
            AmigaTrainerMechanism::ActionReplayCode,
        ),
        ("save_state_edit", AmigaTrainerMechanism::SaveStateEdit),
        ("disk_patch", AmigaTrainerMechanism::DiskPatch),
    ] {
        let mut e = disk1_entry(&m);
        e["mechanism"] = json!(name);
        let a = one(&m, e);
        assert_eq!(a.status, AmigaTrainerStatus::Unsupported, "{name}");
        assert!(mechanism.unsupported_reason().is_some());
    }
    assert!(
        AmigaTrainerMechanism::DiskPatch
            .unsupported_reason()
            .unwrap()
            .contains("scratch-copy launch integration")
    );
}

#[test]
fn runtime_options_are_evidence_only_and_project_nothing() {
    for option in amiga_runtime_options() {
        assert!(!option.scripted_form_evidenced, "{option:?}");
    }
    let readiness = trainer_readiness_for(AmigaMediaFamily::AdfFloppy);
    assert_ne!(readiness.status, AmigaTrainerStatus::Preparable);
    assert_eq!(
        trainer_readiness_for(AmigaMediaFamily::WhdloadInstall).status,
        AmigaTrainerStatus::Unsupported
    );
}

#[test]
fn one_trainer_is_never_selected_for_a_write_that_it_does_not_have() {
    // Width/value mapping into canonical operations keeps exact widths.
    let fx = Fx::new();
    let m = fx.adf("a.adf", "Alpha");
    let imp = import(vec![entry(
        &[(&m, Some(1))],
        "disk",
        Some(1),
        json!([
            {"width": "byte", "address": 3, "value": "0xFF"},
            {"width": "long", "address": 8, "value": "0xDEADBEEF", "original": 0, "timing": "every_frame"}
        ]),
    )]);
    let t = &imp.trainers[0];
    assert_eq!(t.writes.len(), 2);
    assert_eq!(t.writes[0].width, AmigaMemoryWidth::Byte);
    assert_eq!(t.writes[1].value, 0xDEAD_BEEF);
    assert_eq!(t.writes[1].timing, AmigaTrainerTiming::EveryFrame);
}
