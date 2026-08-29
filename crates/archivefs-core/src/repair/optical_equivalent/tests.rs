use std::sync::atomic::AtomicBool;

use tempfile::tempdir;

use super::*;
use crate::chd_logical_media::{MODE1_USER_DATA_OFFSET, RAW_SECTOR_BYTES};
use crate::safe_read::TrustedRoots;

fn sector(value: u8) -> Vec<u8> {
    let mut bytes = vec![0u8; RAW_SECTOR_BYTES];
    bytes[..12].copy_from_slice(&crate::raw_cd_sector::SYNC_PATTERN);
    bytes[15] = 1;
    bytes[MODE1_USER_DATA_OFFSET..MODE1_USER_DATA_OFFSET + 2048].fill(value);
    bytes
}

fn chd_for(sectors: &[Vec<u8>]) -> Vec<u8> {
    let unit = RAW_SECTOR_BYTES as u32;
    let hunk = unit * sectors.len() as u32;
    let payload = format!(
        "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:{} PREGAP:0 PGTYPE:NONE PGSUB:NONE POSTGAP:0",
        sectors.len()
    );
    let meta_offset = 124u64;
    let map_offset = meta_offset + 16 + payload.len() as u64;
    let data_offset = (map_offset + 4).div_ceil(hunk as u64) * hunk as u64;
    let mut chd = vec![0u8; data_offset as usize];
    chd[..8].copy_from_slice(b"MComprHD");
    chd[8..12].copy_from_slice(&124u32.to_be_bytes());
    chd[12..16].copy_from_slice(&5u32.to_be_bytes());
    chd[32..40].copy_from_slice(&(hunk as u64).to_be_bytes());
    chd[40..48].copy_from_slice(&map_offset.to_be_bytes());
    chd[48..56].copy_from_slice(&meta_offset.to_be_bytes());
    chd[56..60].copy_from_slice(&hunk.to_be_bytes());
    chd[60..64].copy_from_slice(&unit.to_be_bytes());
    let p = meta_offset as usize;
    chd[p..p + 4].copy_from_slice(b"CHT2");
    chd[p + 5..p + 8].copy_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
    chd[p + 16..p + 16 + payload.len()].copy_from_slice(payload.as_bytes());
    chd[map_offset as usize..map_offset as usize + 4]
        .copy_from_slice(&(data_offset / hunk as u64).to_be_bytes()[4..]);
    for raw in sectors {
        chd.extend_from_slice(raw);
    }
    chd
}

#[test]
fn matching_cue_bin_and_chd_form_one_group_with_atomic_cue_bin_members() {
    let dir = tempdir().unwrap();
    let cue = dir.path().join("Disc ü.cue");
    let bin = dir.path().join("Track ü.bin");
    let chd = dir.path().join("Disc.chd");
    let payload = [0x11u8; 2048 * 2];
    std::fs::write(&bin, payload).unwrap();
    std::fs::write(
        &cue,
        "FILE \"Track ü.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(&chd, chd_for(&[sector(0x11), sector(0x11)])).unwrap();
    let trusted = TrustedRoots::from_paths([dir.path()]);
    let report = scan_optical_equivalent_duplicates(
        &[cue.clone(), bin.clone(), chd.clone()],
        &trusted,
        None,
    );
    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.preferred, chd);
    assert_eq!(group.quarantine_candidates, vec![cue, bin]);
    assert_eq!(
        group.projected_savings,
        std::fs::metadata(group.cue_bin.files[0].path.clone())
            .unwrap()
            .len()
            + std::fs::metadata(group.cue_bin.files[1].path.clone())
                .unwrap()
                .len()
    );
}

#[test]
fn unsupported_and_malformed_optical_candidates_are_excluded() {
    let dir = tempdir().unwrap();
    let cue = dir.path().join("bad.cue");
    std::fs::write(
        &cue,
        "FILE \"missing.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    let chd = dir.path().join("bad.chd");
    std::fs::write(&chd, b"not a chd").unwrap();
    let report = scan_optical_equivalent_duplicates(
        &[cue.clone(), chd.clone()],
        &TrustedRoots::from_paths([dir.path()]),
        None,
    );
    assert!(report.groups.is_empty());
    assert_eq!(report.excluded.len(), 2);
}

#[test]
fn stale_or_missing_cue_bin_is_refused_before_any_move() {
    let dir = tempdir().unwrap();
    let cue = dir.path().join("disc.cue");
    let bin = dir.path().join("track.bin");
    let chd = dir.path().join("disc.chd");
    std::fs::write(&bin, [0x11u8; 2048]).unwrap();
    std::fs::write(
        &cue,
        "FILE \"track.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(&chd, chd_for(&[sector(0x11)])).unwrap();
    let trusted = TrustedRoots::from_paths([dir.path()]);
    let group = scan_optical_equivalent_duplicates(&[cue.clone(), chd.clone()], &trusted, None)
        .groups
        .remove(0);
    std::fs::remove_file(&bin).unwrap();
    let result = apply_optical_equivalent_group(
        &group,
        dir.path(),
        trusted,
        &dir.path().join("journal"),
        &AtomicBool::new(false),
    );
    assert!(matches!(
        result,
        Err(RepairExecutionError::StaleSource { .. })
    ));
    assert!(cue.exists());
}

#[test]
fn apply_quarantines_cue_and_bin_together_and_rolls_back() {
    let dir = tempdir().unwrap();
    let cue = dir.path().join("disc.cue");
    let bin = dir.path().join("track.bin");
    let chd = dir.path().join("disc.chd");
    std::fs::write(&bin, [0x11u8; 2048]).unwrap();
    std::fs::write(
        &cue,
        "FILE \"track.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
    )
    .unwrap();
    std::fs::write(&chd, chd_for(&[sector(0x11)])).unwrap();
    let journal = dir.path().join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    let trusted = TrustedRoots::from_paths([dir.path()]);
    let group = scan_optical_equivalent_duplicates(&[cue.clone(), chd.clone()], &trusted, None)
        .groups
        .into_iter()
        .next()
        .unwrap();
    let cancel = AtomicBool::new(false);
    let mut result =
        apply_optical_equivalent_group(&group, dir.path(), trusted, &journal, &cancel).unwrap();
    assert!(!cue.exists());
    assert!(!bin.exists());
    assert!(chd.exists());
    rollback_optical_equivalent_group(&mut result.transaction, &journal, &cancel).unwrap();
    assert!(cue.exists());
    assert!(bin.exists());
}
