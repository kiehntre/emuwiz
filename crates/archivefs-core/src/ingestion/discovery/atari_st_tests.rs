use super::super::*;
use std::fs;
#[test]
fn atari_st_discovery_rejects_named_garbage_and_explains_valid_structure() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Atari ST");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("Bad.st"), b"not a floppy").unwrap();
    fs::write(root.join("Bad.stx"), b"not Pasti").unwrap();
    let report = discover_source(&root).unwrap();
    assert_eq!(report.items.len(), 2);
    for item in &report.items {
        assert_ne!(
            item.validation_state,
            ValidationState::Accepted,
            "filename/folder alone cannot validate media: {item:?}"
        );
        assert!(matches!(
            item.skip_reason,
            Some(SkipReason::InvalidContent(_))
        ));
    }
}
fn st_bytes() -> Vec<u8> {
    let mut b = vec![0u8; 720 * 1024];
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = 2;
    b[14..16].copy_from_slice(&1u16.to_le_bytes());
    b[16] = 2;
    b[17..19].copy_from_slice(&112u16.to_le_bytes());
    b[19..21].copy_from_slice(&1440u16.to_le_bytes());
    b[22..24].copy_from_slice(&3u16.to_le_bytes());
    b[24..26].copy_from_slice(&9u16.to_le_bytes());
    b[26..28].copy_from_slice(&2u16.to_le_bytes());
    b
}
fn stx_bytes() -> Vec<u8> {
    let mut b = vec![0u8; 80];
    b[..4].copy_from_slice(b"RSY\0");
    b[4..6].copy_from_slice(&3u16.to_le_bytes());
    b[10] = 1;
    b[16..20].copy_from_slice(&64u32.to_le_bytes());
    b[24..26].copy_from_slice(&9u16.to_le_bytes());
    b[28..30].copy_from_slice(&6250u16.to_le_bytes());
    b
}
#[test]
fn atari_st_discovery_keeps_raw_platform_ambiguity_and_never_claims_a_game_match() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("Bare.st"), st_bytes()).unwrap();
    fs::write(tmp.path().join("Named (Disk 1 of 2).stx"), stx_bytes()).unwrap();
    let folder = tmp.path().join("Atari ST");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("Disk.st"), st_bytes()).unwrap();
    let report = discover_source(tmp.path()).unwrap();
    let bare = report
        .items
        .iter()
        .find(|i| i.path.ends_with("Bare.st"))
        .unwrap();
    assert_ne!(bare.validation_state, ValidationState::Accepted);
    assert!(matches!(
        bare.skip_reason,
        Some(SkipReason::AmbiguousPlatform)
    ));
    // Discovery intentionally groups nested content folders; inspect the Atari
    // ST source itself to exercise its direct per-disk admission contract.
    let atari_report = discover_source(&folder).unwrap();
    let items = [
        &atari_report.items[0],
        report
            .items
            .iter()
            .find(|i| i.path.ends_with("Named (Disk 1 of 2).stx"))
            .unwrap(),
    ];
    for item in items {
        assert_eq!(item.validation_state, ValidationState::Accepted, "{item:?}");
        assert_eq!(item.platform_hint.as_deref(), Some("AtariST"));
        assert!(item.explanation.contains("catalogue match"));
        assert!(item.explanation.contains("No sector payload decoding"));
    }
    assert_eq!(
        atari_report.items.len(),
        1,
        "disk labels do not create missing disks or merged sets"
    );
}
