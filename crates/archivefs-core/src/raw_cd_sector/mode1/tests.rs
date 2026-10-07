use super::*;
use crate::optical_patch_tree::digest;
use crate::raw_cd_sector::SYNC_PATTERN;
fn vector() -> [u8; 2352] {
    let mut b = [0; 2352];
    b[..12].copy_from_slice(&SYNC_PATTERN);
    b[12..16].copy_from_slice(&[0, 2, 0, 1]);
    for (i, v) in b[16..2064].iter_mut().enumerate() {
        *v = i as u8;
    }
    b
}
#[test]
fn edc_known_vectors() {
    // CRC catalogue CRC-32/CD-ROM-EDC check value; no initial/final inversion.
    assert_eq!(mode1_edc(b"123456789"), 0x6ec2edc4);
    assert_eq!(mode1_edc(&vector()[..2064]), 0xe6399727);
}
#[test]
fn ecc_independent_chd_table_vectors() {
    // Frozen independent oracle: chd 0.3.4 compression/ecc.rs ECC_P_OFF,
    // ECC_Q_OFF and its row-based ecc_compute_bytes. 00:02:00 Mode 1 header,
    // user byte[i] = i mod 256; EDC independently computed bit-by-bit.
    // This fixture uses no code from regenerate_mode1 to calculate expectations.
    let mut b = vector();
    regenerate_mode1(&mut b).unwrap();
    assert_eq!(
        digest(&b[2076..2248]),
        "908f3dfc1769d7fa87131a6da11f9f9d47f44a0ed5d7ce1ccd0e9023eec4d0d5"
    );
    assert_eq!(
        digest(&b[2248..2352]),
        "bcbd52ea2a4fca36c5925f6670b29ef5ecdd5c4682acf7131276c70dc1bdc01e"
    );
    assert_eq!(
        digest(&b),
        "f6a94f686e87d4dd2816703689017aa16078564724f033e97eb0faa23cb1adb3"
    );
}
#[test]
fn header_payload_preserved_and_corruption_refused() {
    let mut b = vector();
    let original = b;
    regenerate_mode1(&mut b).unwrap();
    assert_eq!(b[..2064], original[..2064]);
    verify_mode1(&b).unwrap();
    for position in [12, 16, 2064, 2068, 2076, 2248, 2351] {
        let mut bad = b;
        bad[position] ^= 1;
        assert!(verify_mode1(&bad).is_err(), "{position}");
    }
}
#[test]
fn other_sector_modes_refused() {
    let mut b = vector();
    b[15] = 2;
    assert!(regenerate_mode1(&mut b).is_err());
    b[15] = 1;
    b[0] = 1;
    assert!(verify_mode1(&b).is_err());
}
