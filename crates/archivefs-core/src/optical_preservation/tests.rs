use super::*;
use crate::ingestion::cue_bin::{
    CuePregap, CueTrackMode, resolve_cue_all_files, resolve_cue_layout, resolve_data_track,
};
use crate::repair::optical_conversion::{
    ChdConversionError, ChdConversionSourceMode, build_chd_conversion_plan,
};

fn fixture(text: &str, files: &[(&str, usize)]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cue = dir.path().join("disc.cue");
    std::fs::write(&cue, text).unwrap();
    for &(name, length) in files {
        std::fs::write(dir.path().join(name), vec![0x51; length]).unwrap();
    }
    (dir, cue)
}

fn refused(text: &str, files: &[(&str, usize)]) {
    let (dir, cue) = fixture(text, files);
    let before_cue = std::fs::read(&cue).unwrap();
    let before_files: Vec<_> = files
        .iter()
        .map(|(name, _)| std::fs::read(dir.path().join(name)).unwrap())
        .collect();
    let target = dir.path().join("output.chd");
    // Missing converter binding proves refusal happens before tool resolution.
    let result = build_chd_conversion_plan(
        &cue,
        &target,
        ChdConversionSourceMode::KeepSource,
        Some(&dir.path().join("converter-must-not-run")),
    );
    assert!(
        matches!(result, Err(ChdConversionError::InvalidSource(_))),
        "{result:?}"
    );
    assert_eq!(std::fs::read(&cue).unwrap(), before_cue);
    for ((name, _), bytes) in files.iter().zip(before_files) {
        assert_eq!(std::fs::read(dir.path().join(name)).unwrap(), bytes);
    }
    use crate::conversion_planner::*;
    let request = ConversionRequest {
        source: cue.clone(),
        destination: target.clone(),
        platform: ConversionPlatform::Optical,
        source_format: ConversionFormat::Unknown,
        target_format: ConversionFormat::Chd,
        wit_mode: None,
        available_free_space: None,
        required_free_space: 0,
    };
    assert!(matches!(
        plan_chd_conversion(&request, ChdMediaKind::Cd),
        Err(ConversionPlanError::Unsupported(_))
    ));
    assert!(!target.exists());
    assert!(!dir.path().join("journal").exists());
}

#[test]
fn simple_data_layout_is_ordered_and_deterministic() {
    let (_dir, cue) = fixture(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        &[("data.bin", 2048 * 16)],
    );
    let layout = source_layout(&cue, ChdConversionSourceMode::KeepSource).unwrap();
    assert_eq!(
        source_layout(&cue, ChdConversionSourceMode::KeepSource).unwrap(),
        layout
    );
    assert_eq!(layout.tracks.len(), 1);
    assert_eq!(layout.tracks[0].number, 1);
    assert_eq!(layout.tracks[0].index_01.unwrap().frames, 0);
    assert!(layout.tracks[0].index_00.is_none());
    assert!(layout.tracks[0].pregap.is_none());
    assert!(layout.tracks[0].postgap.is_none());
}

#[test]
fn simple_audio_is_inspectable_but_not_conversion_verified() {
    let text = "FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n";
    let (_dir, cue) = fixture(text, &[("audio.bin", 2352 * 16)]);
    assert_eq!(
        resolve_cue_layout(&cue).unwrap().tracks[0].mode,
        CueTrackMode::Audio
    );
    refused(text, &[("audio.bin", 2352 * 16)]);
}

#[test]
fn stored_index_00_and_synthetic_pregap_remain_distinct_at_inspection() {
    let index00 =
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:02:00\n";
    let synthetic =
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGAP 00:02:00\nINDEX 01 00:00:00\n";
    let (_dir, cue) = fixture(index00, &[("data.bin", 2048 * 166)]);
    let data = resolve_data_track(&cue).unwrap();
    assert_eq!(data.index_01_frame, 150);
    assert_eq!(data.data_frame_count, 16);
    assert_eq!(
        data.pregap,
        CuePregap::InFile {
            start_frame: 0,
            frames: 150
        }
    );
    let (_dir, cue) = fixture(synthetic, &[("data.bin", 2048 * 16)]);
    let data = resolve_data_track(&cue).unwrap();
    assert_eq!(data.index_01_frame, 0);
    assert_eq!(data.data_frame_count, 16);
    assert_eq!(data.pregap, CuePregap::Synthetic { frames: 150 });
    refused(index00, &[("data.bin", 2048 * 166)]);
    refused(synthetic, &[("data.bin", 2048 * 16)]);
}

#[test]
fn pregap_plus_index_00_cannot_authorize_conversion() {
    let text = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGAP 00:02:00\nINDEX 00 00:00:00\nINDEX 01 00:00:02\n";
    let (_dir, cue) = fixture(text, &[("data.bin", 2048 * 16)]);
    let track = &resolve_cue_layout(&cue).unwrap().tracks[0];
    assert_eq!(track.pregap.unwrap().frames, 150);
    assert_eq!(track.index_00.unwrap().frames, 0);
    assert_eq!(track.index_01.unwrap().frames, 2);
    refused(text, &[("data.bin", 2048 * 16)]);
}

#[test]
fn multi_index_track_cannot_authorize_conversion() {
    refused(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nINDEX 02 00:00:04\nINDEX 03 00:00:08\n",
        &[("data.bin", 2048 * 16)],
    );
}

#[test]
fn postgap_and_zero_length_gap_declarations_are_refused() {
    for gap in ["POSTGAP 00:00:02", "POSTGAP 00:00:00", "PREGAP 00:00:00"] {
        refused(
            &format!("FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n{gap}\n"),
            &[("data.bin", 2048 * 16)],
        );
    }
}

#[test]
fn mixed_mode_shared_bin_boundaries_are_inspectable_but_not_conversion_verified() {
    let text = "FILE \"mixed.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 00 00:00:04\nINDEX 01 00:00:06\n";
    let (_dir, cue) = fixture(text, &[("mixed.bin", 2352 * 16)]);
    let layout = resolve_cue_layout(&cue).unwrap();
    assert_eq!(layout.tracks[0].path, layout.tracks[1].path);
    let data = resolve_data_track(&cue).unwrap();
    assert_eq!(data.data_frame_count, 4);
    refused(text, &[("mixed.bin", 2352 * 16)]);
}

#[test]
fn multi_file_order_is_declaration_order_and_offsets_reset() {
    let text = "FILE \"z.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"a.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n";
    let (_dir, cue) = fixture(text, &[("z.bin", 2048 * 16), ("a.bin", 2352 * 16)]);
    let layout = resolve_cue_layout(&cue).unwrap();
    let files = resolve_cue_all_files(&cue).unwrap();
    assert!(files[0].ends_with("z.bin"));
    assert!(files[1].ends_with("a.bin"));
    assert_eq!(resolve_cue_all_files(&cue).unwrap(), files);
    assert_eq!(layout.tracks[1].index_01.unwrap().frames, 0);
    assert_eq!(resolve_data_track(&cue).unwrap().data_frame_count, 16);
    refused(text, &[("z.bin", 2048 * 16), ("a.bin", 2352 * 16)]);
}

#[test]
fn one_bin_multi_track_cannot_authorize_conversion() {
    refused(
        "FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 00 00:00:04\nINDEX 01 00:00:06\n",
        &[("audio.bin", 2352 * 16)],
    );
}

#[test]
fn hidden_track_one_audio_is_preserved_in_source_and_refused_for_conversion() {
    refused(
        "FILE \"audio.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 00 00:00:00\nINDEX 01 00:02:00\n",
        &[("audio.bin", 2352 * 166)],
    );
}

#[test]
fn malformed_indexes_and_gaps_fail_closed() {
    for body in [
        "INDEX 00 00:00:00", // no INDEX 01
        "INDEX 01 00:00:00\nINDEX 00 00:00:02",
        "INDEX 00 00:00:02\nINDEX 01 00:00:01",
        "INDEX 01 00:00:00\nINDEX 03 00:00:08\nINDEX 02 00:00:04",
        "INDEX 01 00:00:00\nINDEX 02 00:00:08\nINDEX 03 00:00:04",
        "INDEX 01 00:00:00\nINDEX 01 00:00:00",
        "INDEX 01 00:00:00\nINDEX 02 00:00:04\nINDEX 02 00:00:08",
        "PREGAP 00:02:00\nPREGAP 00:02:00\nINDEX 01 00:00:00",
        "INDEX 01 00:00:75",
        "INDEX 01 00:60:00",
        "INDEX 01 18446744073709551615:00:00",
        "INDEX 01 00:00:16", // offset at EOF
        "INDEX 01 00:00:00 trailing",
    ] {
        refused(
            &format!("FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\n{body}\n"),
            &[("data.bin", 2048 * 16)],
        );
    }
}

#[test]
fn overlapping_and_descending_shared_tracks_fail_closed() {
    for boundary in ["00:00:00", "00:00:02"] {
        refused(
            &format!(
                "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:04\nTRACK 02 MODE1/2048\nINDEX 01 {boundary}\n"
            ),
            &[("data.bin", 2048 * 16)],
        );
    }
}

#[test]
fn missing_empty_truncated_and_out_of_file_sources_fail_closed() {
    let simple = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
    refused(simple, &[]);
    for length in [0, 1, 2047, 2049, 2048 * 16 - 1] {
        refused(simple, &[("data.bin", length)]);
    }
    refused(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:01:00\nINDEX 01 00:02:00\n",
        &[("data.bin", 2048)],
    );
}

#[test]
fn unsupported_modes_file_types_flags_and_directives_fail_closed() {
    for mode in ["MODE1/2352", "MODE2/2352", "MODE2/2336", "CDG", "UNKNOWN"] {
        refused(
            &format!("FILE \"data.bin\" BINARY\nTRACK 01 {mode}\nINDEX 01 00:00:00\n"),
            &[("data.bin", 2352 * 16)],
        );
    }
    for kind in ["WAVE", "MOTOROLA", "MP3", "BINARY trailing", ""] {
        refused(
            &format!("FILE \"data.bin\" {kind}\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n"),
            &[("data.bin", 2048 * 16)],
        );
    }
    for extra in [
        "FLAGS PRE",
        "REM SESSION 2",
        "FILEX \"data.bin\" BINARY",
        "éééé",
        "INDEX\0 01 00:00:00",
    ] {
        refused(
            &format!("FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n{extra}\n"),
            &[("data.bin", 2048 * 16)],
        );
    }
}

#[test]
fn unsafe_or_ambiguous_file_names_fail_closed() {
    for name in [
        "../outside.bin",
        "/etc/passwd",
        "data.bin\" BINARY \"other.bin",
        "dir\\data.bin",
        "",
    ] {
        refused(
            &format!("FILE \"{name}\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n"),
            &[("data.bin", 2048 * 16)],
        );
    }
}

#[test]
fn quoted_unicode_spaces_and_shell_metacharacters_are_literal_file_names() {
    let name = "track space ü $() `literal`.bin";
    let (_dir, cue) = fixture(
        &format!("file \"{name}\" binary\r\n\ttrack 1 mode1/2048\r\n\tindex 1 00:00:00\r\n"),
        &[(name, 2048 * 16)],
    );
    assert!(
        source_layout(&cue, ChdConversionSourceMode::KeepSource)
            .unwrap()
            .tracks[0]
            .path
            .ends_with(name)
    );
}

#[test]
fn conversion_cue_size_is_bounded() {
    refused(&" ".repeat(MAX_CONVERSION_CUE_BYTES as usize + 1), &[]);
}

#[test]
fn converter_frame_count_bounds_are_checked_before_payload_hashing() {
    let (dir, cue) = fixture(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        &[("data.bin", 2048)],
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("data.bin"))
        .unwrap()
        .set_len((i32::MAX as u64 + 1) * 2048)
        .unwrap();
    assert!(
        source_layout(&cue, ChdConversionSourceMode::KeepSource)
            .unwrap_err()
            .contains("converter frame bounds")
    );
}

#[test]
fn file_msf_uses_75_frames_without_adding_a_150_frame_lead_in() {
    use crate::ingestion::cue_bin::CueTimestamp;
    assert_eq!(CueTimestamp::parse("00:02:00").unwrap().frames, 150);
    assert_eq!(CueTimestamp::parse("03:12:00").unwrap().frames, 14400);
    assert_eq!(CueTimestamp::parse("03:14:00").unwrap().frames, 14550);
    assert_eq!(CueTimestamp::parse("00:59:74").unwrap().frames, 4499);
    assert!(CueTimestamp::parse("00:00:75").is_err());
    assert!(CueTimestamp::parse("00:60:00").is_err());
}

fn metadata_chd(path: &Path, tag: &[u8; 4], metadata: &str) {
    // Metadata-only synthetic v5 CHD; layout checks never decode track data.
    let payload = metadata.as_bytes();
    let mut bytes = vec![0; 124 + 16 + payload.len()];
    bytes[..8].copy_from_slice(b"MComprHD");
    bytes[8..12].copy_from_slice(&124_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(&5_u32.to_be_bytes());
    bytes[32..40].copy_from_slice(&(2448_u64 * 16).to_be_bytes());
    bytes[48..56].copy_from_slice(&124_u64.to_be_bytes());
    bytes[56..60].copy_from_slice(&(2448_u32 * 8).to_be_bytes());
    bytes[60..64].copy_from_slice(&2448_u32.to_be_bytes());
    bytes[124..128].copy_from_slice(tag);
    bytes[129..132].copy_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
    bytes[140..].copy_from_slice(payload);
    std::fs::write(path, bytes).unwrap();
}

const SIMPLE_CHD_METADATA: &str =
    "TRACK:1 TYPE:MODE1 SUBTYPE:NONE FRAMES:16 PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0";

#[test]
fn output_metadata_must_prove_mode_frames_gaps_and_subchannel() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.chd");
    metadata_chd(&path, b"CHT2", SIMPLE_CHD_METADATA);
    verify_output_layout(&path, 16).unwrap();
    assert!(verify_output_layout(&path, 15).is_err());
    for (before, after) in [
        ("TRACK:1", "TRACK:2"),
        ("TYPE:MODE1", "TYPE:MODE1_RAW"),
        ("SUBTYPE:NONE", "SUBTYPE:RW"),
        ("FRAMES:16", "FRAMES:15"),
        ("PREGAP:0", "PREGAP:2"),
        ("PREGAP:0", ""),
        ("POSTGAP:0", "POSTGAP:2"),
        ("POSTGAP:0", ""),
        ("PGTYPE:MODE1", "PGTYPE:VMODE1"),
        ("PGSUB:NONE", "PGSUB:RW_RAW"),
    ] {
        metadata_chd(&path, b"CHT2", &SIMPLE_CHD_METADATA.replace(before, after));
        assert!(
            verify_output_layout(&path, 16).is_err(),
            "{before} -> {after}"
        );
    }
}

#[test]
fn unknown_legacy_and_malformed_output_metadata_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.chd");
    for tag in [b"CHTR", b"CHGD", b"CHSE", b"XXXX"] {
        metadata_chd(&path, tag, SIMPLE_CHD_METADATA);
        assert!(verify_output_layout(&path, 16).is_err());
    }
    metadata_chd(&path, b"CHT2", "TRACK:1 TYPE:MODE1");
    assert!(verify_output_layout(&path, 16).is_err());
    std::fs::write(&path, b"truncated CHD").unwrap();
    assert!(verify_output_layout(&path, 16).is_err());
}

#[test]
fn duplicate_unknown_or_hidden_metadata_tokens_cannot_authorize_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.chd");
    for metadata in [
        SIMPLE_CHD_METADATA.replace("PREGAP:0", "PREGAP:2 PREGAP:0"),
        SIMPLE_CHD_METADATA.replace("POSTGAP:0", "POSTGAP:2 POSTGAP:0"),
        format!("{SIMPLE_CHD_METADATA} INDEX:2"),
        format!("{SIMPLE_CHD_METADATA}\0POSTGAP:2"),
        format!("{SIMPLE_CHD_METADATA}\0\0"),
    ] {
        metadata_chd(&path, b"CHT2", &metadata);
        assert!(verify_output_layout(&path, 16).is_err());
    }
    metadata_chd(&path, b"CHT2", &format!("{SIMPLE_CHD_METADATA}\0"));
    verify_output_layout(&path, 16).unwrap();
}

#[test]
fn additional_track_or_session_metadata_cannot_authorize_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metadata.chd");
    for tag in [b"CHT2", b"CHSE"] {
        metadata_chd(&path, b"CHT2", SIMPLE_CHD_METADATA);
        let mut bytes = std::fs::read(&path).unwrap();
        let next = bytes.len() as u64;
        bytes[132..140].copy_from_slice(&next.to_be_bytes());
        let mut entry = vec![0; 16 + SIMPLE_CHD_METADATA.len()];
        entry[..4].copy_from_slice(tag);
        entry[5..8].copy_from_slice(&(SIMPLE_CHD_METADATA.len() as u32).to_be_bytes()[1..]);
        entry[16..].copy_from_slice(SIMPLE_CHD_METADATA.as_bytes());
        bytes.extend(entry);
        std::fs::write(&path, bytes).unwrap();
        assert!(verify_output_layout(&path, 16).is_err());
    }
}

#[test]
fn validated_text_is_resolved_without_reopening_the_sheet() {
    let original = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
    let (_dir, cue) = fixture(original, &[("data.bin", 2048 * 16)]);
    std::fs::write(&cue, "replacement is not a CUE").unwrap();
    let layout = resolve_cue_layout_text(&cue, original).unwrap();
    assert_eq!(layout.tracks[0].index_01.unwrap().frames, 0);
    assert_eq!(
        crate::optical_fingerprint::fingerprint_cue_layout(&layout)
            .unwrap()
            .structure
            .logical_sector_count,
        16
    );
    assert!(resolve_cue_layout(&cue).is_err());
}

#[test]
fn unusual_track_numbering_and_repeated_file_blocks_are_refused() {
    for body in [
        "TRACK 00 MODE1/2048\nINDEX 01 00:00:00\n",
        "TRACK 02 MODE1/2048\nINDEX 01 00:00:00\n",
        "TRACK 01 MODE1/2048\nINDEX 01 00:00:00\nTRACK 01 AUDIO\nINDEX 01 00:00:01\n",
        "TRACK 02 MODE1/2048\nINDEX 01 00:00:00\nTRACK 01 AUDIO\nINDEX 01 00:00:01\n",
        "TRACK 01 MODE1/2048\nINDEX 01 00:00:00\nFILE \"data.bin\" BINARY\n",
    ] {
        refused(
            &format!("FILE \"data.bin\" BINARY\n{body}"),
            &[("data.bin", 2048 * 16)],
        );
    }
}

#[test]
fn quarantine_cannot_break_nested_file_references() {
    let (dir, cue) = fixture(
        "FILE \"sub/data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        &[],
    );
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/data.bin"), vec![0; 2048]).unwrap();
    source_layout(&cue, ChdConversionSourceMode::KeepSource).unwrap();
    assert!(source_layout(&cue, ChdConversionSourceMode::QuarantineSource).is_err());
}

#[test]
fn a_cue_cannot_be_its_own_bin_even_if_sector_aligned() {
    let mut text = "FILE \"disc.cue\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n".to_owned();
    text.extend(std::iter::repeat_n(' ', 2048 - text.len()));
    refused(&text, &[]);
}

fn uncompressed_zero_chd(path: &Path) -> (usize, usize) {
    metadata_chd(path, b"CHT2", SIMPLE_CHD_METADATA);
    let mut bytes = std::fs::read(path).unwrap();
    let map_offset = bytes.len();
    let hunk_bytes = 2448 * 8;
    bytes[40..48].copy_from_slice(&(map_offset as u64).to_be_bytes());
    bytes.extend_from_slice(&1_u32.to_be_bytes());
    bytes.extend_from_slice(&2_u32.to_be_bytes());
    bytes.resize(hunk_bytes * 3, 0);
    std::fs::write(path, bytes).unwrap();
    (map_offset, hunk_bytes)
}

#[test]
fn complete_and_explicit_sparse_chd_hunks_are_supported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zero.chd");
    let (map, _) = uncompressed_zero_chd(&path);
    verify_output_layout(&path, 16).unwrap();
    verify_output_storage(&path, 16).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[map..map + 8].fill(0); // Explicit sparse hunks, not missing physical data.
    bytes.truncate(map + 8);
    std::fs::write(&path, bytes).unwrap();
    verify_output_storage(&path, 16).unwrap();
}

#[test]
fn truncated_hunks_cannot_be_certified_by_a_matching_zero_payload() {
    use crate::optical_fingerprint::{fingerprint_chd, fingerprint_cue_bin};
    let (dir, cue) = fixture(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        &[("data.bin", 2048 * 16)],
    );
    std::fs::write(dir.path().join("data.bin"), vec![0; 2048 * 16]).unwrap();
    let path = dir.path().join("truncated.chd");
    let (_, hunk) = uncompressed_zero_chd(&path);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..hunk * 2]).unwrap();
    // Existing logical decoding zero-fills the missing second hunk.
    assert_eq!(
        fingerprint_cue_bin(&cue).unwrap().canonical_sha256,
        fingerprint_chd(&path).unwrap().canonical_sha256
    );
    verify_output_layout(&path, 16).unwrap();
    assert!(
        verify_output_storage(&path, 16)
            .unwrap_err()
            .contains("truncated")
    );
}

#[test]
fn output_geometry_and_parent_dependencies_must_match_the_track() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("geometry.chd");
    uncompressed_zero_chd(&path);
    let original = std::fs::read(&path).unwrap();
    for (offset, replacement) in [
        (32, (2448_u64 * 20).to_be_bytes().to_vec()),
        (56, 19585_u32.to_be_bytes().to_vec()),
        (60, 2352_u32.to_be_bytes().to_vec()),
        (104, vec![1]),
    ] {
        let mut bytes = original.clone();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        std::fs::write(&path, bytes).unwrap();
        assert!(verify_output_storage(&path, 16).is_err());
    }
    std::fs::write(&path, original).unwrap();
    assert!(verify_output_storage(&path, 0).is_err());
    assert!(verify_output_storage(&path, 17).is_err());
}

#[test]
fn recorded_single_hunk_chd_that_reader_cannot_decode_is_refused() {
    // chdman 0.264, default compression, one cooked sector filled with 0x42.
    // Layout is known, but chd-rs 0.3.4 refuses this compressed hunk map.
    // Do not turn an unreadable payload into a preservation certificate.
    let bytes: &[u8] = &[
        0x4d, 0x43, 0x6f, 0x6d, 0x70, 0x72, 0x48, 0x44, 0x00, 0x00, 0x00, 0x7c, 0x00, 0x00, 0x00,
        0x05, 0x63, 0x64, 0x6c, 0x7a, 0x63, 0x64, 0x7a, 0x6c, 0x63, 0x64, 0x66, 0x6c, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x26, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7c, 0x00, 0x00, 0x4c, 0x80,
        0x00, 0x00, 0x09, 0x90, 0xa3, 0x80, 0xaf, 0x2a, 0x8a, 0x19, 0x2c, 0xea, 0xaf, 0xec, 0x5e,
        0x3b, 0xf0, 0xaa, 0xca, 0x20, 0x9c, 0xb8, 0x36, 0x43, 0x9b, 0x71, 0x16, 0x15, 0xc9, 0xf4,
        0x0e, 0xb2, 0x57, 0x1d, 0xa5, 0xfc, 0xfa, 0xa5, 0x63, 0x64, 0xb6, 0x1d, 0x2b, 0x08, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x43, 0x48, 0x54, 0x32, 0x01, 0x00, 0x00, 0x54, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x54, 0x52, 0x41, 0x43, 0x4b, 0x3a, 0x31, 0x20, 0x54, 0x59,
        0x50, 0x45, 0x3a, 0x4d, 0x4f, 0x44, 0x45, 0x31, 0x20, 0x53, 0x55, 0x42, 0x54, 0x59, 0x50,
        0x45, 0x3a, 0x4e, 0x4f, 0x4e, 0x45, 0x20, 0x46, 0x52, 0x41, 0x4d, 0x45, 0x53, 0x3a, 0x31,
        0x20, 0x50, 0x52, 0x45, 0x47, 0x41, 0x50, 0x3a, 0x30, 0x20, 0x50, 0x47, 0x54, 0x59, 0x50,
        0x45, 0x3a, 0x4d, 0x4f, 0x44, 0x45, 0x31, 0x20, 0x50, 0x47, 0x53, 0x55, 0x42, 0x3a, 0x4e,
        0x4f, 0x4e, 0x45, 0x20, 0x50, 0x4f, 0x53, 0x54, 0x47, 0x41, 0x50, 0x3a, 0x30, 0x00, 0x00,
        0x00, 0x25, 0xed, 0xc1, 0x01, 0x01, 0x00, 0x00, 0x00, 0x01, 0x20, 0xdb, 0xfc, 0x1f, 0x65,
        0x88, 0xaa, 0x05, 0x00, 0xde, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0xdc, 0x00, 0x63, 0x60, 0x18, 0x05, 0xa3, 0x60,
        0xe4, 0x02, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x4a, 0x7e,
        0x06, 0x00, 0x00, 0x00, 0x01, 0x11, 0x0b, 0x63, 0x1d, 0x00,
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recorded.chd");
    std::fs::write(&path, bytes).unwrap();
    verify_output_layout(&path, 1).unwrap();
    assert!(verify_output_storage(&path, 1).is_err());
    assert!(crate::optical_fingerprint::fingerprint_chd(&path).is_err());
}

#[test]
fn benign_annotations_do_not_change_layout_or_preview_admission() {
    use crate::conversion_planner::*;
    let simple = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
    let (dir, cue) = fixture(simple, &[("data.bin", 2048 * 16)]);
    let expected = source_layout(&cue, ChdConversionSourceMode::KeepSource).unwrap();
    for annotation in [
        "REM COMMENT dumped from original",
        "REM GENRE Game",
        "TITLE \"Disc ü\"",
        "rem\tcomment\tTitle",
        "title\t\"Disc\"\r",
    ] {
        for text in [
            format!("{annotation}\n{simple}"),
            format!("{simple}{annotation}\n"),
        ] {
            std::fs::write(&cue, text).unwrap();
            assert_eq!(
                source_layout(&cue, ChdConversionSourceMode::KeepSource).unwrap(),
                expected
            );
            let request = ConversionRequest {
                source: cue.clone(),
                destination: dir.path().join("out.chd"),
                platform: ConversionPlatform::Optical,
                source_format: ConversionFormat::Unknown,
                target_format: ConversionFormat::Chd,
                wit_mode: None,
                available_free_space: None,
                required_free_space: 0,
            };
            assert_eq!(
                plan_chd_conversion(&request, ChdMediaKind::Cd)
                    .unwrap()
                    .source_layout
                    .as_ref(),
                Some(&expected)
            );
        }
    }
    for bad in [
        "TITLE",
        "TITLE \"Disc\" FLAGS PRE",
        "REM SESSION 2",
        "CATALOG 1234567890123",
    ] {
        refused(&format!("{simple}{bad}\n"), &[("data.bin", 2048 * 16)]);
    }
}

#[cfg(unix)]
#[test]
fn quarantine_accepts_dot_prefix_but_refuses_component_aliases() {
    let (dir, cue) = fixture(
        "FILE \"./data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        &[("data.bin", 2048 * 16)],
    );
    source_layout(&cue, ChdConversionSourceMode::QuarantineSource).unwrap();
    std::os::unix::fs::symlink(dir.path().join("data.bin"), dir.path().join("alias.bin")).unwrap();
    std::fs::write(
        &cue,
        "FILE \"alias.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    source_layout(&cue, ChdConversionSourceMode::KeepSource).unwrap();
    assert!(source_layout(&cue, ChdConversionSourceMode::QuarantineSource).is_err());
}

const META_SIMPLE: &str = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
const META_KEEP: ChdConversionSourceMode = ChdConversionSourceMode::KeepSource;

#[test]
fn track_level_and_non_ascii_metadata_is_admitted_without_changing_the_layout() {
    let (_dir, cue) = fixture(META_SIMPLE, &[("data.bin", 2048 * 16)]);
    let expected = source_layout(&cue, META_KEEP).unwrap();
    for text in [
        // between TRACK and INDEX, indented as ripping tools write it
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\n  TITLE \"Track\"\n  REM COMMENT x\n  REM GENRE y\nINDEX 01 00:00:00\n",
        // disc level before FILE and track level together
        "TITLE \"Disc\"\nFILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\n  TITLE \"Track\"\nINDEX 01 00:00:00\n",
        // non-ASCII in a quoted TITLE and in REM values (including lowercase REM)
        "TITLE \"Disc é 日本\"\nREM COMMENT é 日本\nrem genre ü\nFILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        // bare REM COMMENT without a value
        "REM COMMENT\nFILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
    ] {
        std::fs::write(&cue, text).unwrap();
        assert_eq!(
            source_layout(&cue, META_KEEP).unwrap(),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn near_miss_metadata_syntax_and_control_characters_are_refused() {
    for line in [
        "REM",
        "REM DATE 1999",
        "REM DISCID 12345678",
        "REM LEAD-OUT 00:00:00",
        "REM COMMENTS x",
        "REM GENRES x",
        "REMCOMMENT x",
        "REM\u{a0}COMMENT x",
        "TITLE unquoted",
        "TITLE é",
        "TITLE\"x\"",
        "TITLE \"unterminated",
        "TITLE \"a\" \"b\"",
        "TITLES \"x\"",
        "REM COMMENT a\u{1}b",
        "TITLE \"a\u{1}b\"",
    ] {
        refused(
            &format!("{line}\n{META_SIMPLE}"),
            &[("data.bin", 2048 * 16)],
        );
    }
}

#[test]
fn benign_metadata_cannot_launder_unsafe_directives() {
    let meta = "REM COMMENT ok\nREM GENRE ok\nTITLE \"ok\"\n";
    for extra in [
        "PREGAP 00:00:02",
        "POSTGAP 00:00:02",
        "INDEX 00 00:00:00",
        "INDEX 02 00:00:08",
        "FLAGS DCP",
        "CATALOG 0123456789012",
        "PERFORMER \"x\"",
        "ISRC ABCDE1234567",
        "SONGWRITER \"x\"",
        "CDTEXTFILE \"x.cdt\"",
    ] {
        refused(
            &format!(
                "{meta}FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\n{extra}\nINDEX 01 00:00:00\n"
            ),
            &[("data.bin", 2048 * 16)],
        );
    }
    for body in [
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:01\n",
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
        "FILE \"data.bin\" BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n",
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nTRACK 02 MODE1/2048\nINDEX 01 00:00:08\n",
    ] {
        refused(&format!("{meta}{body}"), &[("data.bin", 2048 * 16)]);
    }
}

// ---- INDEX 00 / PREGAP preservation reasons surfaced to the conversion preview ----

fn refusal_message(text: &str, files: &[(&str, usize)]) -> String {
    refused(text, files);
    let (dir, cue) = fixture(text, files);
    let error = build_chd_conversion_plan(
        &cue,
        &dir.path().join("output.chd"),
        ChdConversionSourceMode::KeepSource,
        Some(&dir.path().join("converter-must-not-run")),
    )
    .unwrap_err();
    let ChdConversionError::InvalidSource(message) = error else {
        panic!("expected InvalidSource");
    };
    message
}

fn detail_of(text: &str, files: &[(&str, usize)]) -> String {
    let message = refusal_message(text, files);
    crate::repair::optical_conversion::layout_preservation_detail(&message)
        .unwrap_or_else(|| panic!("no preservation detail in: {message}"))
        .to_owned()
}

#[test]
fn no_pregap_data_only_disc_is_admitted_and_carries_no_warning() {
    let text = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
    let (_dir, cue) = fixture(text, &[("data.bin", 2048 * 16)]);
    source_layout(&cue, ChdConversionSourceMode::KeepSource).unwrap();
    assert_eq!(layout_preservation_detail(&cue), None);
}

#[test]
fn stored_index_00_gap_is_named_as_dropped_source_data_whatever_its_length() {
    // 1 second (75 frames): deliberately not the common two-second gap.
    let detail = detail_of(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:01:00\n",
        &[("data.bin", 2048 * 80)],
    );
    assert!(
        detail.contains("source-backed pregap (INDEX 00)"),
        "{detail}"
    );
    assert!(detail.contains("00:01:00"), "{detail}");
    assert!(detail.contains("would drop it"), "{detail}");
    // 7 frames: odd length, still reported exactly.
    let detail = detail_of(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:00:07\n",
        &[("data.bin", 2048 * 20)],
    );
    assert!(detail.contains("00:00:07"), "{detail}");
}

#[test]
fn synthetic_pregap_is_reported_as_not_stored_and_distinct_from_index_00() {
    let detail = detail_of(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGAP 00:02:00\nINDEX 01 00:00:00\n",
        &[("data.bin", 2048 * 16)],
    );
    assert!(detail.contains("synthetic PREGAP"), "{detail}");
    assert!(!detail.contains("source-backed"), "{detail}");
}

#[test]
fn index_00_plus_pregap_is_ambiguous_and_not_guessed() {
    let detail = detail_of(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGAP 00:02:00\nINDEX 00 00:00:00\nINDEX 01 00:00:02\n",
        &[("data.bin", 2048 * 16)],
    );
    assert!(detail.contains("not guessed"), "{detail}");
    assert!(detail.contains("ReviewRequired"), "{detail}");
}

#[test]
fn mixed_mode_disc_names_every_blocker_it_would_lose() {
    // Track 1 MODE1/2048 data, track 2 AUDIO with a stored INDEX 00 pregap, one BIN.
    let detail = detail_of(
        "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 00 00:00:16\nINDEX 01 00:00:18\n",
        &[("disc.bin", 2048 * 16 + 2352 * 100)],
    );
    // Mixed sector sizes in one file are refused by the timeline, never guessed.
    assert!(
        detail.contains("not guessed") || detail.contains("source-backed pregap"),
        "{detail}"
    );
}

#[test]
fn raw_mixed_mode_names_stored_pregap_audio_and_multiple_tracks() {
    let detail = detail_of(
        "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 00 00:00:50\nINDEX 01 00:00:52\n",
        &[("disc.bin", 2352 * 100)],
    );
    assert!(
        detail.contains("Track 2 has 00:00:02 of source-backed pregap"),
        "{detail}"
    );
    assert!(detail.contains("Track 2 is audio"), "{detail}");
    assert!(detail.contains("Only a single-track disc"), "{detail}");
}

#[test]
fn multiple_file_statements_are_named_and_pregap_stays_per_file() {
    let detail = detail_of(
        "FILE \"d.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nFILE \"a.bin\" BINARY\nTRACK 02 AUDIO\nPREGAP 00:02:00\nINDEX 01 00:00:00\n",
        &[("d.bin", 2352 * 20), ("a.bin", 2352 * 20)],
    );
    assert!(detail.contains("several FILEs"), "{detail}");
    assert!(
        detail.contains("Track 2 declares a synthetic PREGAP"),
        "{detail}"
    );
}

#[test]
fn malformed_ordering_and_index_00_without_index_01_are_refused_without_a_guess() {
    // INDEX 00 after INDEX 01.
    refused(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:04\nINDEX 00 00:00:08\n",
        &[("data.bin", 2048 * 16)],
    );
    // INDEX 00 equal to INDEX 01 (zero-length pregap).
    refused(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:04\nINDEX 01 00:00:04\n",
        &[("data.bin", 2048 * 16)],
    );
    // INDEX 00 with no INDEX 01 at all.
    refused(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\n",
        &[("data.bin", 2048 * 16)],
    );
    // Track numbering out of order.
    refused(
        "FILE \"data.bin\" BINARY\nTRACK 02 MODE1/2048\nINDEX 01 00:00:00\nTRACK 01 AUDIO\nINDEX 01 00:00:08\n",
        &[("data.bin", 2048 * 16)],
    );
}

#[test]
fn refusal_leaves_the_generic_gate_wording_and_adds_the_specific_reason() {
    let message = refusal_message(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:00:02\n",
        &[("data.bin", 2048 * 16)],
    );
    assert!(message.contains("outside the verified layout"), "{message}");
    assert!(message.contains("layout preservation:"), "{message}");
    assert!(message.ends_with(']'), "{message}");
}

#[test]
fn exact_fingerprint_refuses_every_fact_the_program_hash_does_not_cover() {
    use crate::optical_fingerprint::{fingerprint_cue_bin, fingerprint_cue_bin_exact};
    let plain = "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
    let (_dir, cue) = fixture(plain, &[("data.bin", 2048 * 16)]);
    assert_eq!(
        fingerprint_cue_bin(&cue).unwrap(),
        fingerprint_cue_bin_exact(&cue).unwrap()
    );
    for (name, text, bytes, needle) in [
        (
            "stored",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:00:02\n",
            2048 * 16,
            "INDEX 00",
        ),
        (
            "synthetic",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGAP 00:02:00\nINDEX 01 00:00:00\n",
            2048 * 16,
            "synthetic PREGAP",
        ),
        (
            "postgap",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nPOSTGAP 00:02:00\n",
            2048 * 16,
            "POSTGAP",
        ),
        (
            "offset",
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:02\n",
            2048 * 16,
            "not at the start",
        ),
    ] {
        let (_dir, cue) = fixture(text, &[("data.bin", bytes)]);
        let error = fingerprint_cue_bin_exact(&cue).unwrap_err().to_string();
        assert!(error.contains(needle), "{name}: {error}");
    }
    // The program-data identity is intentionally unchanged for stored pregap.
    let (_dir, cue) = fixture(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:00:02\n",
        &[("data.bin", 2048 * 16)],
    );
    assert!(fingerprint_cue_bin(&cue).is_ok());
}

#[test]
fn exact_fingerprint_refuses_data_plus_audio_instead_of_ignoring_the_audio() {
    use crate::optical_fingerprint::fingerprint_cue_bin_exact;
    let (_dir, cue) = fixture(
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nFILE \"a.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        &[("data.bin", 2352 * 16), ("a.bin", 2352 * 16)],
    );
    let error = fingerprint_cue_bin_exact(&cue).unwrap_err().to_string();
    assert!(error.contains("audio"), "{error}");
}

#[test]
fn non_ascii_directive_prefixes_are_refused_without_panicking() {
    // A multi-byte character straddling a directive-prefix boundary must be a
    // refusal, not a byte-slice panic, in both the gate and the detail path.
    for text in [
        "FILé \"data.bin\" BINARY\n",
        "FILE \"data.bin\" BINARY\nTRAé 01 MODE1/2048\n",
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGé 00:02:00\nINDEX 01 00:00:00\n",
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEé 01 00:00:00\n",
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\nPOSTGé 00:02:00\n",
    ] {
        refused(text, &[("data.bin", 2048 * 16)]);
    }
}
