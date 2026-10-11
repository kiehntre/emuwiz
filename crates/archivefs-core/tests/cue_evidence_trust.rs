#![cfg(unix)]

use archivefs_core::ingestion::cue_bin::CueError;
use archivefs_core::ingestion::cue_evidence_trust::*;
use archivefs_core::ingestion::cue_preservation::*;
use archivefs_core::ingestion::cue_timeline::{PregapEvidence, chd_conversion_preservation};
use archivefs_core::safe_read::SafeReadRefusal;
use std::fs::{File, FileTimes};
use std::path::{Path, PathBuf};

const SINGLE: &str = "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n";
const STORED: &str =
    "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 00 00:00:00\n INDEX 01 00:00:02\n";
type Record = Result<DiscLayoutEvidence, CuePreservationError>;

fn bytes(sectors: &[u8]) -> Vec<u8> {
    sectors.iter().flat_map(|v| vec![*v; 2048]).collect()
}
fn fixture(text: &str, sectors: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("disc.bin"), bytes(sectors)).unwrap();
    let cue = dir.path().join("disc.cue");
    std::fs::write(&cue, text).unwrap();
    (dir, cue)
}
fn assert_unprotected(assessment: &CueObservationAssessment<'_>) {
    for side in [OperandSide::Left, OperandSide::Right] {
        let operand = assessment.operand(side);
        assert!(matches!(
            operand.representation_trust(),
            RepresentationTrust::MutableReadObservation
        ));
        assert!(matches!(
            operand.require_verified_capture(),
            Err(StableCaptureRefusal::CustodyBackendUnavailable)
        ));
    }
}
fn assert_matching_observation(left: &Record, right: &Record) {
    let a = assess_cue_observations(left, right);
    assert_eq!(a.relation(), ObservedRelation::MatchingBytesObserved);
    assert!(a.differences().is_empty());
    assert_eq!(a.observed_programs_match(), Some(true));
    assert_eq!(a.refusals().count(), 0);
    assert_unprotected(&a);
    for side in [OperandSide::Left, OperandSide::Right] {
        assert_eq!(
            a.operand(side).source_assessment(),
            SourceAssessment::IndeterminateSource(SourceUncertainty::ContinuousStabilityUnproven)
        );
        assert!(matches!(
            a.operand(side).layout_support(),
            LayoutSupport::SupportedLayout {
                contract: CUE_PRESERVATION_SCHEMA
            }
        ));
    }
}

#[test]
fn stable_matching_records_are_observations_with_original_provenance() {
    let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
    let (_b, b) = fixture(SINGLE, &[1, 2, 3]);
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    assert_matching_observation(&left, &right);
    let assessment = assess_cue_observations(&left, &right);
    assert!(std::ptr::eq(
        assessment.operand(OperandSide::Left).evidence().unwrap(),
        left.as_ref().unwrap()
    ));
    assert_eq!(
        assessment
            .operand(OperandSide::Left)
            .evidence()
            .unwrap()
            .layout()
            .cue_path,
        a
    );
    assert_ne!(
        left.as_ref().unwrap().sources()[0].path,
        right.as_ref().unwrap().sources()[0].path
    );
}

#[test]
fn different_physical_content_remains_a_historical_difference() {
    let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
    let (_b, b) = fixture(SINGLE, &[9, 2, 3]);
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    let assessment = assess_cue_observations(&left, &right);
    assert_eq!(
        assessment.relation(),
        ObservedRelation::DifferentBytesObserved
    );
    assert!(
        assessment
            .differences()
            .contains(&CueLayoutDifference::SourceContent { ordinal: 0 })
    );
    assert!(
        assessment
            .differences()
            .contains(&CueLayoutDifference::ProgramContent { track: 1 })
    );
    assert_eq!(assessment.observed_programs_match(), Some(false));
    assert_unprotected(&assessment);
}

#[test]
fn stored_index00_and_synthetic_pregap_keep_their_physical_distinction() {
    let (_a, a) = fixture(STORED, &[1, 2, 3, 4, 5]);
    let synthetic = SINGLE.replace(" INDEX 01", " PREGAP 00:00:02\n INDEX 01");
    let (_b, b) = fixture(&synthetic, &[3, 4, 5]);
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    let assessment = assess_cue_observations(&left, &right);
    assert_eq!(
        assessment.relation(),
        ObservedRelation::DifferentBytesObserved
    );
    assert!(
        assessment
            .differences()
            .contains(&CueLayoutDifference::PregapStorage { track: 1 })
    );
    assert_eq!(assessment.observed_programs_match(), Some(true));
    assert!(matches!(
        right.as_ref().unwrap().timeline().tracks[0].pregap,
        PregapEvidence::Synthetic { frames: 2 }
    ));
    assert!(chd_conversion_preservation(left.as_ref().unwrap().timeline()).is_err());
    assert!(chd_conversion_preservation(right.as_ref().unwrap().timeline()).is_err());
    assert_unprotected(&assessment);
}

#[test]
fn matching_program_bytes_do_not_erase_stored_pregap_differences() {
    let (_a, a) = fixture(STORED, &[1, 2, 3, 4, 5]);
    let (_b, b) = fixture(STORED, &[9, 8, 3, 4, 5]);
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    let assessment = assess_cue_observations(&left, &right);
    assert_eq!(
        assessment.relation(),
        ObservedRelation::DifferentBytesObserved
    );
    assert!(
        assessment
            .differences()
            .contains(&CueLayoutDifference::StoredPregapContent { track: 1 })
    );
    assert_eq!(assessment.observed_programs_match(), Some(true));
    assert_unprotected(&assessment);
}

#[test]
fn comment_and_path_differences_are_retained_without_changing_observed_relation() {
    let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
    let (dir, b) = fixture(
        &format!(
            "REM COMMENT archival note\n{}",
            SINGLE.replace("disc.bin", "other.bin")
        ),
        &[1, 2, 3],
    );
    std::fs::rename(dir.path().join("disc.bin"), dir.path().join("other.bin")).unwrap();
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    assert_matching_observation(&left, &right);
    assert_ne!(
        left.as_ref().unwrap().cue_sha256(),
        right.as_ref().unwrap().cue_sha256()
    );
    assert_ne!(
        left.as_ref().unwrap().cue_text(),
        right.as_ref().unwrap().cue_text()
    );
}

#[test]
fn stored_records_never_promise_freshness_after_same_size_mutation() {
    let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
    let (dir, b) = fixture(SINGLE, &[1, 2, 3]);
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    let old = right.as_ref().unwrap().sources()[0].physical_sha256.clone();
    let bin = dir.path().join("disc.bin");
    std::fs::write(&bin, bytes(&[9, 2, 3])).unwrap();
    assert_eq!(std::fs::metadata(&bin).unwrap().len(), 3 * 2048);
    assert_matching_observation(&left, &right);
    assert_eq!(right.as_ref().unwrap().sources()[0].physical_sha256, old);
    let current = collect_cue_preservation(&b);
    assert_eq!(
        assess_cue_observations(&left, &current).relation(),
        ObservedRelation::DifferentBytesObserved
    );
    std::fs::write(a.with_file_name("disc.bin"), bytes(&[8, 2, 3])).unwrap();
    assert_matching_observation(&left, &right); // Both stored operands are now stale.
}

#[test]
fn restored_mtime_never_upgrades_stored_matching_records() {
    let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
    let (dir, b) = fixture(SINGLE, &[1, 2, 3]);
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    let bin = dir.path().join("disc.bin");
    let before = std::fs::metadata(&bin).unwrap();
    std::fs::write(&bin, bytes(&[9, 2, 3])).unwrap();
    File::options()
        .write(true)
        .open(&bin)
        .unwrap()
        .set_times(FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    assert_eq!(
        std::fs::metadata(&bin).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
    assert_eq!(std::fs::read(&bin).unwrap()[0], 9);
    assert_matching_observation(&left, &right);
}

#[test]
fn detected_atomic_replacement_retains_changed_source_and_clears_program_match() {
    let (dir, cue) = fixture(SINGLE, &[1, 2, 3]);
    let inspection = inspect_cue_preservation(&cue).unwrap();
    let earlier = read_cue_preservation(&inspection);
    let replacement = dir.path().join("replacement.bin");
    std::fs::write(&replacement, bytes(&[9, 2, 3])).unwrap();
    std::fs::rename(&replacement, dir.path().join("disc.bin")).unwrap();
    let changed = read_cue_preservation(&inspection);
    assert!(matches!(
        changed,
        Err(CuePreservationError::ChangedSource { .. })
    ));
    let a = assess_cue_observations(&earlier, &changed);
    assert_eq!(a.relation(), ObservedRelation::NotCompared);
    assert_eq!(a.observed_programs_match(), None);
    assert!(matches!(
        a.operand(OperandSide::Right).source_assessment(),
        SourceAssessment::ChangedSource(_)
    ));
    assert_eq!(a.refusals().next().unwrap().operand, OperandSide::Right);
    assert_unprotected(&a);
    assert_matching_observation(&earlier, &earlier); // Historical facts, even after replacement.
}

#[test]
fn symlink_relationship_replacement_is_not_hidden_or_revalidated_by_adapter() {
    let (dir, cue) = fixture(SINGLE, &[1, 2, 3]);
    let first = dir.path().join("first.bin");
    std::fs::rename(dir.path().join("disc.bin"), &first).unwrap();
    let link = dir.path().join("disc.bin");
    std::os::unix::fs::symlink(&first, &link).unwrap();
    let inspection = inspect_cue_preservation(&cue).unwrap();
    let earlier = read_cue_preservation(&inspection);
    std::fs::write(dir.path().join("second.bin"), bytes(&[9, 2, 3])).unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(dir.path().join("second.bin"), &link).unwrap();
    let changed = read_cue_preservation(&inspection);
    assert!(matches!(
        changed,
        Err(CuePreservationError::ChangedSource { .. })
    ));
    assert_eq!(
        assess_cue_observations(&earlier, &changed).relation(),
        ObservedRelation::NotCompared
    );
    assert_matching_observation(&earlier, &earlier);
    assert_eq!(earlier.as_ref().unwrap().sources()[0].path, first);
}

#[test]
fn multiple_files_and_hardlink_alias_graph_remain_comparison_facts() {
    let sheet =
        format!("{SINGLE}FILE \"second.bin\" BINARY\n TRACK 02 MODE1/2048\n INDEX 01 00:00:00\n");
    let (da, a) = fixture(&sheet, &[1, 2, 3]);
    std::fs::hard_link(da.path().join("disc.bin"), da.path().join("second.bin")).unwrap();
    let (db, b) = fixture(&sheet, &[1, 2, 3]);
    std::fs::hard_link(db.path().join("disc.bin"), db.path().join("second.bin")).unwrap();
    let left = collect_cue_preservation(&a);
    let right = collect_cue_preservation(&b);
    assert_matching_observation(&left, &right);
    assert_eq!(
        left.as_ref().unwrap().sources()[1].shares_source_with,
        Some(0)
    );
    std::fs::remove_file(db.path().join("second.bin")).unwrap();
    std::fs::write(db.path().join("second.bin"), bytes(&[1, 2, 3])).unwrap();
    let independent = collect_cue_preservation(&b);
    let a = assess_cue_observations(&left, &independent);
    assert_eq!(a.relation(), ObservedRelation::DifferentBytesObserved);
    assert!(
        a.differences()
            .contains(&CueLayoutDifference::SourceRelationships)
    );
    assert_eq!(a.observed_programs_match(), Some(true));
    assert_unprotected(&a);
}

#[test]
fn missing_and_io_failures_are_indeterminate_not_observed_differences() {
    let (dir, cue) = fixture(SINGLE, &[1, 2, 3]);
    std::fs::remove_file(dir.path().join("disc.bin")).unwrap();
    let missing = collect_cue_preservation(&cue);
    assert!(matches!(
        missing,
        Err(CuePreservationError::Cue(CueError::MissingDataFile(_)))
    ));
    let eio: Record = Err(CuePreservationError::Io {
        path: cue,
        detail: std::io::Error::from_raw_os_error(libc::EIO).to_string(),
    });
    let a = assess_cue_observations(&missing, &eio);
    assert_eq!(a.relation(), ObservedRelation::NotCompared);
    assert_eq!(a.observed_programs_match(), None);
    assert_eq!(a.refusals().count(), 2);
    for side in [OperandSide::Left, OperandSide::Right] {
        assert_eq!(
            a.operand(side).layout_support(),
            LayoutSupport::NotEstablished
        );
        assert!(matches!(
            a.operand(side).source_assessment(),
            SourceAssessment::IndeterminateSource(SourceUncertainty::CollectionFailed(_))
        ));
    }
    assert_unprotected(&a);
}

#[test]
fn unreadable_source_is_not_promoted_to_supported_or_matching() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "Unreadable-mode reproduction unsupported under root; typed EIO covered separately"
        );
        return;
    }
    let (dir, cue) = fixture(SINGLE, &[1, 2, 3]);
    let bin = dir.path().join("disc.bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0)).unwrap();
    assert!(File::open(&bin).is_err());
    let unreadable = collect_cue_preservation(&cue);
    assert!(unreadable.is_err());
    let a = assess_cue_observations(&unreadable, &unreadable);
    assert_eq!(a.relation(), ObservedRelation::NotCompared);
    assert_eq!(a.observed_programs_match(), None);
    assert_unprotected(&a);
}

#[test]
fn unsupported_left_and_changed_right_retain_both_dimensions_and_attribution() {
    let unsupported: Record = Err(CuePreservationError::Unsupported {
        detail: "unrepresented layout".into(),
    });
    let changed: Record = Err(CuePreservationError::ChangedSource {
        path: PathBuf::from("changed.bin"),
    });
    for (left, right, unsupported_side, changed_side) in [
        (
            &unsupported,
            &changed,
            OperandSide::Left,
            OperandSide::Right,
        ),
        (
            &changed,
            &unsupported,
            OperandSide::Right,
            OperandSide::Left,
        ),
    ] {
        let a = assess_cue_observations(left, right);
        assert_eq!(a.relation(), ObservedRelation::NotCompared);
        assert_eq!(a.observed_programs_match(), None);
        assert!(matches!(
            a.operand(unsupported_side).layout_support(),
            LayoutSupport::UnsupportedLayout(_)
        ));
        assert!(matches!(
            a.operand(changed_side).source_assessment(),
            SourceAssessment::ChangedSource(_)
        ));
        let errors: Vec<_> = a.refusals().collect();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].operand, OperandSide::Left);
        assert_eq!(errors[1].operand, OperandSide::Right);
        assert_eq!(errors[0].reason, left.as_ref().err().unwrap());
        assert_eq!(errors[1].reason, right.as_ref().err().unwrap());
        assert_unprotected(&a);
    }
}

#[test]
fn unsupported_declarations_never_match_even_when_both_refusals_are_identical() {
    let (_dir, cue) = fixture(&format!("{SINGLE}FLAGS DCP\n"), &[1, 2, 3]);
    let record = collect_cue_preservation(&cue);
    assert!(record.is_err());
    let a = assess_cue_observations(&record, &record);
    assert_eq!(a.relation(), ObservedRelation::NotCompared);
    assert!(matches!(
        a.operand(OperandSide::Left).layout_support(),
        LayoutSupport::UnsupportedLayout(_)
    ));
    assert_eq!(a.refusals().count(), 2);
    assert_unprotected(&a);
}

#[test]
fn changed_while_opening_is_positive_change_evidence() {
    let changed: Record = Err(CuePreservationError::SafeRead {
        path: PathBuf::from("disc.bin"),
        refusal: SafeReadRefusal::ChangedWhileOpening,
    });
    let a = assess_cue_observations(&changed, &changed);
    assert!(matches!(
        a.operand(OperandSide::Left).source_assessment(),
        SourceAssessment::ChangedSource(_)
    ));
    assert_eq!(a.relation(), ObservedRelation::NotCompared);
    assert_unprotected(&a);
}

#[test]
fn cloning_repeating_and_disappearing_sources_grant_no_capture_capability() {
    let (dir, cue) = fixture(SINGLE, &[1, 2, 3]);
    let record = collect_cue_preservation(&cue);
    let cloned = record.clone();
    std::fs::remove_file(dir.path().join("disc.bin")).unwrap();
    std::fs::remove_file(&cue).unwrap();
    for _ in 0..20 {
        assert_matching_observation(&record, &cloned);
    }
    assert!(!cue.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn writable_mapping_with_real_equal_metadata_never_supplies_stable_capture() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    fn stamp(path: &Path) -> (u64, u64, u64, i64, i64, i64, i64) {
        let m = std::fs::metadata(path).unwrap();
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    }
    struct Mapping(*mut libc::c_void, usize);
    impl Drop for Mapping {
        fn drop(&mut self) {
            assert_eq!(unsafe { libc::munmap(self.0, self.1) }, 0);
        }
    }
    for mutated_side in [OperandSide::Left, OperandSide::Right] {
        let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
        let (_b, b) = fixture(SINGLE, &[1, 2, 3]);
        let path = match mutated_side {
            OperandSide::Left => &a,
            OperandSide::Right => &b,
        }
        .with_file_name("disc.bin");
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let len = file.metadata().unwrap().len() as usize;
        // SAFETY: a fresh fixture is mapped for its full, fixed size; it remains
        // allocated until Mapping drops. No other Rust reference aliases it.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(ptr, libc::MAP_FAILED);
        let mapping = Mapping(ptr, len);
        let byte = mapping.0.cast::<u8>();
        unsafe {
            std::ptr::write_volatile(byte, std::ptr::read_volatile(byte));
        }
        drop(file); // The earlier writable mapping remains usable after close.
        let left = collect_cue_preservation(&a);
        let right = collect_cue_preservation(&b);
        assert!(left.is_ok() && right.is_ok());
        let before = stamp(&path);
        unsafe {
            std::ptr::write_volatile(byte, 9);
        }
        let after = stamp(&path);
        assert_eq!(std::fs::read(&path).unwrap()[0], 9);
        assert_eq!(
            before, after,
            "Exact equal-metadata mmap reproduction unsupported on this filesystem; never fabricate metadata"
        );
        assert_eq!(
            compare_cue_evidence(&left, &right).outcome,
            CueComparisonOutcome::Equivalent
        );
        assert_matching_observation(&left, &right);
    }
}

#[test]
fn adapter_only_work_is_independent_of_payload_size_and_never_reopens_sources() {
    use std::time::Instant;
    for size in [8_u64 * 1024 * 1024, 64 * 1024 * 1024] {
        let (dir, cue) = fixture(SINGLE, &[0]);
        let bin = dir.path().join("disc.bin");
        File::options()
            .write(true)
            .open(&bin)
            .unwrap()
            .set_len(size)
            .unwrap();
        let record = collect_cue_preservation(&cue);
        assert!(record.is_ok());
        let before = record.clone();
        std::fs::remove_file(&bin).unwrap();
        std::fs::remove_file(&cue).unwrap();
        // External syscall tracing brackets ONLY adapter calls, not collection.
        eprintln!("ADAPTER_ONLY_BEGIN bytes={size}");
        let start = Instant::now();
        for _ in 0..1000 {
            let assessment = assess_cue_observations(&record, &record);
            assert_eq!(
                assessment.relation(),
                ObservedRelation::MatchingBytesObserved
            );
            assert_unprotected(&assessment);
            std::hint::black_box(assessment);
        }
        let elapsed = start.elapsed();
        eprintln!(
            "ADAPTER_ONLY_END bytes={size} calls=1000 nanos={}",
            elapsed.as_nanos()
        );
        assert_eq!(record, before);
        assert!(!bin.exists() && !cue.exists());
    }
}

#[test]
fn maximum_file_track_and_alias_records_add_no_payload_reads_to_adapter() {
    let sheet: String = (1..=99)
        .map(|n| {
            format!("FILE \"disc.bin\" BINARY\n TRACK {n:02} MODE1/2048\n INDEX 01 00:00:00\n")
        })
        .collect();
    let (dir, cue) = fixture(&sheet, &[1]);
    let record = collect_cue_preservation(&cue);
    let evidence = record.as_ref().unwrap();
    assert_eq!(evidence.sources().len(), 99);
    assert_eq!(evidence.tracks().len(), 99);
    assert_eq!(evidence.sources()[98].shares_source_with, Some(0));
    std::fs::remove_file(dir.path().join("disc.bin")).unwrap();
    std::fs::remove_file(&cue).unwrap();
    assert_matching_observation(&record, &record);
}
