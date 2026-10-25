// Verified evidence currently requires Unix device/inode/change-time identity.
#![cfg(unix)]

use archivefs_core::ingestion::cue_bin::{CueError, resolve_cue_layout};
use archivefs_core::ingestion::cue_preservation::*;
use archivefs_core::ingestion::cue_timeline::{
    PregapEvidence, build_timeline, chd_conversion_preservation,
};
use archivefs_core::optical_fingerprint::fingerprint_cue_bin_exact;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const STORED: &str =
    "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 00 00:00:00\n INDEX 01 00:00:02\n";
const SINGLE: &str = "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n";

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
fn compare(a: &Path, b: &Path) -> CuePreservationComparison {
    compare_cue_evidence(&collect_cue_preservation(a), &collect_cue_preservation(b))
}
fn sha(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn identical_layout_and_stored_bytes_are_proven_without_redump_or_chd_claims() {
    let (_a, a) = fixture(STORED, &[1, 2, 3, 4, 5, 6, 7]);
    let (_b, b) = fixture(STORED, &[1, 2, 3, 4, 5, 6, 7]);
    let first = collect_cue_preservation(&a).unwrap();
    assert_eq!(first, collect_cue_preservation(&a).unwrap());
    assert_eq!(first.schema(), CUE_PRESERVATION_SCHEMA);
    assert_eq!(first.cue_text(), STORED);
    let result = compare(&a, &b);
    assert_eq!(result.outcome, CueComparisonOutcome::Equivalent);
    assert!(result.differences.is_empty());
    assert_eq!(result.programs_match, Some(true));
    assert!(!first.timeline().evidence.redump_equivalence_verified);
    assert!(fingerprint_cue_bin_exact(&a).is_err());
    assert!(chd_conversion_preservation(first.timeline()).is_err());
    assert_eq!(
        first.timeline(),
        &build_timeline(&resolve_cue_layout(&a).unwrap()).unwrap()
    );
    assert_eq!(first.sources()[0].identity.file.size, 7 * 2048);
    assert_eq!(
        first.sources()[0].physical_sha256,
        sha(&bytes(&[1, 2, 3, 4, 5, 6, 7]))
    );
    let gap = first.tracks()[0].source_backed_pregap.as_ref().unwrap();
    assert_eq!((gap.range.byte_offset, gap.range.byte_len), (0, 2 * 2048));
    assert_eq!(gap.physical_sha256, sha(&bytes(&[1, 2])));
    assert_eq!(
        first.tracks()[0].program.physical_sha256,
        sha(&bytes(&[3, 4, 5, 6, 7]))
    );
}

#[test]
fn matching_program_does_not_hide_different_stored_pregap_bytes() {
    let (_a, a) = fixture(STORED, &[1, 2, 3, 4, 5, 6, 7]);
    let (_b, b) = fixture(STORED, &[9, 8, 3, 4, 5, 6, 7]);
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert_eq!(c.programs_match, Some(true));
    assert!(
        c.differences
            .contains(&CueLayoutDifference::StoredPregapContent { track: 1 })
    );
}

#[test]
fn source_backed_index00_and_synthetic_pregap_are_distinct() {
    let (_a, a) = fixture(STORED, &[1, 2, 3, 4, 5, 6, 7]);
    let synthetic = SINGLE.replace(" INDEX 01", " PREGAP 00:00:02\n INDEX 01");
    let (_b, b) = fixture(&synthetic, &[3, 4, 5, 6, 7]);
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert_eq!(c.programs_match, Some(true));
    assert!(
        c.differences
            .contains(&CueLayoutDifference::PregapStorage { track: 1 })
    );
    let e = collect_cue_preservation(&b).unwrap();
    assert!(e.tracks()[0].source_backed_pregap.is_none());
    assert_eq!(
        e.timeline().tracks[0].pregap,
        PregapEvidence::Synthetic { frames: 2 }
    );
}

#[test]
fn shared_bin_track_boundaries_use_the_next_index00() {
    let sheet = format!("{SINGLE} TRACK 02 MODE1/2048\n INDEX 00 00:00:03\n INDEX 01 00:00:04\n");
    let (_dir, p) = fixture(&sheet, &[1, 2, 3, 4, 5, 6, 7]);
    let e = collect_cue_preservation(&p).unwrap();
    assert_eq!(e.sources().len(), 1);
    assert_eq!(
        e.layout().tracks[0].file_ordinal,
        e.layout().tracks[1].file_ordinal
    );
    assert_eq!(e.tracks()[0].program.range.frames, 3);
    assert_eq!(
        e.tracks()[1]
            .source_backed_pregap
            .as_ref()
            .unwrap()
            .range
            .byte_offset,
        3 * 2048
    );
    assert_eq!(e.tracks()[1].program.range.byte_offset, 4 * 2048);
    assert_eq!(
        e.tracks()[0].program.physical_sha256,
        sha(&bytes(&[1, 2, 3]))
    );
    assert_eq!(compare(&p, &p).outcome, CueComparisonOutcome::Equivalent);
}

#[test]
fn multiple_files_keep_indexes_relative_to_each_file() {
    let sheet = format!(
        "{SINGLE}FILE \"second.bin\" BINARY\n TRACK 02 MODE1/2048\n INDEX 00 00:00:00\n INDEX 01 00:00:01\n"
    );
    let (dir, p) = fixture(&sheet, &[1, 2, 3]);
    std::fs::write(dir.path().join("second.bin"), bytes(&[4, 5, 6])).unwrap();
    let e = collect_cue_preservation(&p).unwrap();
    assert_eq!(e.sources().len(), 2);
    assert_eq!(e.sources()[1].ordinal, 1);
    assert_eq!(e.layout().tracks[1].file_ordinal, 1);
    assert_eq!(e.tracks()[1].program.range.byte_offset, 2048);
    assert_eq!(e.timeline().tracks[1].cue_frame_index01, 4);
    assert_eq!(compare(&p, &p).outcome, CueComparisonOutcome::Equivalent);
}

#[test]
fn additional_indexes_are_retained_and_compared_even_with_matching_program() {
    let (_a, a) = fixture(
        &format!("{STORED} INDEX 02 00:00:04\n"),
        &[1, 2, 3, 4, 5, 6, 7],
    );
    let (_b, b) = fixture(
        &format!("{STORED} INDEX 02 00:00:05\n"),
        &[1, 2, 3, 4, 5, 6, 7],
    );
    let e = collect_cue_preservation(&a).unwrap();
    assert_eq!(e.timeline().tracks[0].extra_indexes, vec![(2, 4)]);
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert_eq!(c.programs_match, Some(true));
    assert!(
        c.differences
            .contains(&CueLayoutDifference::IndexBoundaries { track: 1 })
    );
}

#[test]
fn moving_index00_with_identical_program_and_file_bytes_changes_layout() {
    let (_a, a) = fixture(STORED, &[1, 2, 3, 4, 5, 6, 7]);
    let (_b, b) = fixture(
        &STORED.replace("INDEX 00 00:00:00", "INDEX 00 00:00:01"),
        &[1, 2, 3, 4, 5, 6, 7],
    );
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert_eq!(c.programs_match, Some(true));
    assert!(
        c.differences
            .contains(&CueLayoutDifference::IndexBoundaries { track: 1 })
    );
}

#[test]
fn truncated_sector_data_is_never_equivalent_even_to_itself() {
    let (dir, p) = fixture(SINGLE, &[1, 2, 3]);
    std::fs::write(dir.path().join("disc.bin"), vec![0; 2049]).unwrap();
    assert!(matches!(
        collect_cue_preservation(&p),
        Err(CuePreservationError::Timeline(_))
    ));
    assert_eq!(compare(&p, &p).outcome, CueComparisonOutcome::Unsupported);
}

#[test]
fn missing_backing_files_are_indeterminate_and_named() {
    let (_dir, p) = fixture(&SINGLE.replace("disc.bin", "missing.bin"), &[1, 2, 3]);
    assert!(matches!(
        collect_cue_preservation(&p),
        Err(CuePreservationError::Cue(CueError::MissingDataFile(_)))
    ));
    let c = compare(&p, &p);
    assert_eq!(c.outcome, CueComparisonOutcome::Indeterminate);
    assert!(!c.refusals.is_empty());
    assert_eq!(c.programs_match, None);
}

#[test]
fn unsafe_relative_and_absolute_file_references_are_refused() {
    for name in ["../escape.bin", "/tmp/absolute.bin"] {
        let (_dir, p) = fixture(&SINGLE.replace("disc.bin", name), &[1, 2, 3]);
        assert!(matches!(
            collect_cue_preservation(&p),
            Err(CuePreservationError::Cue(CueError::UnsafeReference))
        ));
        assert_eq!(compare(&p, &p).outcome, CueComparisonOutcome::Unsupported);
    }
}

#[test]
fn overflow_and_malformed_indexes_are_refused_by_the_canonical_parser() {
    for marker in [
        " INDEX 01 18446744073709551615:00:00\n",
        " INDEX 01 4099276460824344:00:00\n",
        " INDEX 01 +1:00:00\n",
        " INDEX 01 00:00:75\n",
        " INDEX 01 00:00:00\n INDEX 01 00:00:01\n",
    ] {
        let (_dir, p) = fixture(
            &format!("FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n{marker}"),
            &[1, 2, 3],
        );
        assert!(matches!(
            collect_cue_preservation(&p),
            Err(CuePreservationError::Cue(CueError::Malformed(_)))
        ));
        assert_eq!(compare(&p, &p).outcome, CueComparisonOutcome::Unsupported);
    }
}

#[test]
fn changed_source_identity_after_inspection_refuses_identical_replacement_bytes() {
    let (dir, p) = fixture(STORED, &[1, 2, 3, 4, 5, 6, 7]);
    let inspected = inspect_cue_preservation(&p).unwrap();
    let replacement = dir.path().join("replacement.bin");
    std::fs::write(&replacement, bytes(&[1, 2, 3, 4, 5, 6, 7])).unwrap();
    std::fs::rename(replacement, dir.path().join("disc.bin")).unwrap();
    assert!(matches!(
        read_cue_preservation(&inspected),
        Err(CuePreservationError::ChangedSource { .. })
    ));
}

#[test]
fn unassigned_prefix_bytes_are_recorded_and_cannot_hide_behind_program_hashes() {
    let no_index00 = STORED.replace(" INDEX 00 00:00:00\n", "");
    let (_a, a) = fixture(&no_index00, &[1, 2, 3, 4, 5, 6, 7]);
    let (_b, b) = fixture(&no_index00, &[8, 9, 3, 4, 5, 6, 7]);
    let e = collect_cue_preservation(&a).unwrap();
    assert!(e.tracks()[0].source_backed_pregap.is_none());
    let prefix = e.sources()[0].unassigned_prefix.as_ref().unwrap();
    assert_eq!(prefix.physical_sha256, sha(&bytes(&[1, 2])));
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert_eq!(c.programs_match, Some(true));
}

#[test]
fn ambiguous_gaps_and_unrepresented_directives_are_explicitly_unsupported() {
    for sheet in [
        format!("{STORED} PREGAP 00:00:02\n"),
        format!("{SINGLE} FLAGS DCP\n"),
        format!("{SINGLE}FILE \"disc.bin\" BINARY\n"),
    ] {
        let (_dir, p) = fixture(&sheet, &[1, 2, 3, 4, 5, 6, 7]);
        let c = compare(&p, &p);
        assert_eq!(c.outcome, CueComparisonOutcome::Unsupported);
        assert!(!c.refusals.is_empty());
    }
}

#[test]
fn zero_length_synthetic_declarations_are_not_erased() {
    let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
    let (_b, b) = fixture(&format!("{SINGLE} POSTGAP 00:00:00\n"), &[1, 2, 3]);
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert!(
        c.differences
            .contains(&CueLayoutDifference::SyntheticGaps { track: 1 })
    );
}

#[cfg(unix)]
#[test]
fn changed_resolved_file_relationship_after_inspection_is_detected() {
    let (dir, p) = fixture(&SINGLE.replace("disc.bin", "link.bin"), &[1, 2, 3]);
    std::os::unix::fs::symlink("disc.bin", dir.path().join("link.bin")).unwrap();
    let inspected = inspect_cue_preservation(&p).unwrap();
    std::fs::write(dir.path().join("other.bin"), bytes(&[1, 2, 3])).unwrap();
    std::os::unix::fs::symlink("other.bin", dir.path().join("new-link.bin")).unwrap();
    std::fs::rename(dir.path().join("new-link.bin"), dir.path().join("link.bin")).unwrap();
    assert!(matches!(
        read_cue_preservation(&inspected),
        Err(CuePreservationError::ChangedSource { .. })
    ));
}

#[cfg(unix)]
#[test]
fn physical_file_aliases_are_distinct_from_independent_identical_copies() {
    let sheet =
        format!("{SINGLE}FILE \"second.bin\" BINARY\n TRACK 02 MODE1/2048\n INDEX 01 00:00:00\n");
    let (a_dir, a) = fixture(&sheet, &[1, 2, 3]);
    let (b_dir, b) = fixture(&sheet, &[1, 2, 3]);
    std::fs::hard_link(
        a_dir.path().join("disc.bin"),
        a_dir.path().join("second.bin"),
    )
    .unwrap();
    std::fs::write(b_dir.path().join("second.bin"), bytes(&[1, 2, 3])).unwrap();
    let e = collect_cue_preservation(&a).unwrap();
    assert_eq!(e.sources()[1].shares_source_with, Some(0));
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert_eq!(c.programs_match, Some(true));
    assert!(
        c.differences
            .contains(&CueLayoutDifference::SourceRelationships)
    );
}

#[test]
fn shifting_a_shared_track_boundary_changes_the_program_partition() {
    let sheet = format!("{SINGLE} TRACK 02 MODE1/2048\n INDEX 00 00:00:03\n INDEX 01 00:00:04\n");
    let (_a, a) = fixture(&sheet, &[1, 2, 3, 4, 5, 6, 7]);
    let (_b, b) = fixture(
        &sheet.replace("INDEX 00 00:00:03", "INDEX 00 00:00:02"),
        &[1, 2, 3, 4, 5, 6, 7],
    );
    let c = compare(&a, &b);
    assert_eq!(c.outcome, CueComparisonOutcome::Different);
    assert_eq!(c.programs_match, Some(false));
    assert!(
        c.differences
            .contains(&CueLayoutDifference::IndexBoundaries { track: 1 })
    );
    assert!(
        c.differences
            .contains(&CueLayoutDifference::IndexBoundaries { track: 2 })
    );
}

#[cfg(unix)]
#[test]
fn unreadable_backing_file_is_indeterminate_with_a_path_diagnostic() {
    use std::os::unix::fs::PermissionsExt;
    // Root bypasses the permission fixture; production tests run unprivileged.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let (dir, p) = fixture(SINGLE, &[1, 2, 3]);
    let bin = dir.path().join("disc.bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o0)).unwrap();
    let c = compare(&p, &p);
    assert_eq!(c.outcome, CueComparisonOutcome::Indeterminate);
    assert!(
        c.refusals
            .iter()
            .all(|r| r.to_string().contains("disc.bin"))
    );
}

#[test]
fn binary_audio_and_same_width_mixed_tracks_have_physical_evidence_only() {
    for mode in ["MODE1/2352", "MODE2/2352", "AUDIO"] {
        let sheet = format!(
            "FILE \"disc.bin\" BINARY\n TRACK 01 {mode}\n INDEX 00 00:00:00\n INDEX 01 00:00:01\n TRACK 02 AUDIO\n INDEX 01 00:00:03\n"
        );
        let (dir, p) = fixture(&sheet, &[1]);
        let raw: Vec<_> = (1..=5).flat_map(|v| vec![v; 2352]).collect();
        std::fs::write(dir.path().join("disc.bin"), &raw).unwrap();
        let e = collect_cue_preservation(&p).unwrap();
        assert_eq!(
            e.tracks()[0]
                .source_backed_pregap
                .as_ref()
                .unwrap()
                .physical_sha256,
            sha(&raw[..2352])
        );
        assert_eq!(compare(&p, &p).outcome, CueComparisonOutcome::Equivalent);
        assert!(!e.timeline().evidence.redump_equivalence_verified);
        assert!(chd_conversion_preservation(e.timeline()).is_err());
        assert!(fingerprint_cue_bin_exact(&p).is_err());
    }
}

#[test]
fn unsupported_storage_types_and_sector_mappings_never_produce_proof() {
    for sheet in [
        SINGLE.replace("BINARY", "WAVE"),
        SINGLE.replace("MODE1/2048", "MODE2/2336"),
        format!("{SINGLE} TRACK 02 AUDIO\n INDEX 01 00:00:01\n"),
        format!("{SINGLE} REM SESSION 2\n"),
    ] {
        let (_dir, p) = fixture(&sheet, &[1, 2, 3]);
        let c = compare(&p, &p);
        assert_eq!(c.outcome, CueComparisonOutcome::Unsupported);
        assert!(!c.refusals.is_empty());
    }
}

#[test]
fn replaced_cue_identity_is_rejected_even_with_identical_text() {
    let (dir, p) = fixture(SINGLE, &[1, 2, 3]);
    let inspection = inspect_cue_preservation(&p).unwrap();
    let replacement = dir.path().join("replacement.cue");
    std::fs::write(&replacement, SINGLE).unwrap();
    std::fs::rename(replacement, &p).unwrap();
    assert!(matches!(
        read_cue_preservation(&inspection),
        Err(CuePreservationError::ChangedSource { .. })
    ));
}

#[cfg(unix)]
#[test]
fn changed_bytes_with_restored_mtime_are_not_the_inspected_identity() {
    use std::fs::{File, FileTimes};
    let (dir, p) = fixture(SINGLE, &[1, 2, 3]);
    let bin = dir.path().join("disc.bin");
    let before = std::fs::metadata(&bin).unwrap();
    let inspection = inspect_cue_preservation(&p).unwrap();
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
    assert!(matches!(
        read_cue_preservation(&inspection),
        Err(CuePreservationError::ChangedSource { .. })
    ));
}

#[test]
fn harmless_comments_do_not_change_physical_equivalence() {
    let (_a, a) = fixture(SINGLE, &[1, 2, 3]);
    let (_b, b) = fixture(&format!("REM COMMENT archival note\n{SINGLE}"), &[1, 2, 3]);
    assert_ne!(
        collect_cue_preservation(&a).unwrap().cue_sha256(),
        collect_cue_preservation(&b).unwrap().cue_sha256()
    );
    assert_eq!(compare(&a, &b).outcome, CueComparisonOutcome::Equivalent);
}

#[test]
fn descending_additional_index_numbers_are_ambiguous_not_equivalent() {
    let (_dir, p) = fixture(
        &format!("{SINGLE} INDEX 03 00:00:01\n INDEX 02 00:00:02\n"),
        &[1, 2, 3, 4],
    );
    let c = compare(&p, &p);
    assert_eq!(c.outcome, CueComparisonOutcome::Unsupported);
    assert!(!c.refusals.is_empty());
}

#[test]
fn source_evidence_limit_is_checked_without_reading_a_sparse_giant_file() {
    let (dir, p) = fixture(SINGLE, &[1]);
    std::fs::File::options()
        .write(true)
        .open(dir.path().join("disc.bin"))
        .unwrap()
        .set_len(MAX_SOURCE_BYTES + 2048)
        .unwrap();
    let result = collect_cue_preservation(&p);
    assert!(matches!(
        result,
        Err(CuePreservationError::Unsupported { .. })
    ));
    assert!(result.unwrap_err().to_string().contains("8 GiB"));
}
