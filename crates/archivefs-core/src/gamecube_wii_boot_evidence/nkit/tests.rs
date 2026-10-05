use super::*;
use std::fs;

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn fixture(wii: bool) -> Vec<u8> {
    // Header fixtures only, deliberately no real game data or valid FST.
    let mut bytes = vec![0; if wii { 0x50000 } else { 0x2440 }];
    bytes[..6].copy_from_slice(if wii { b"RTSE01" } else { b"GTSE01" });
    bytes[0x20..0x29].copy_from_slice(b"Synthetic");
    bytes[0x200..0x208].copy_from_slice(b"NKIT v01");
    put(&mut bytes, 0x208, 0x12345678);
    if wii {
        put(&mut bytes, 0x18, 0x5d1c9ea3);
        put(&mut bytes, 0x210, (WII_SIZE / 4) as u32);
        put(&mut bytes, 0x4e000, 2);
        bytes[0x60..0x62].copy_from_slice(&[1, 1]);
    } else {
        put(&mut bytes, 0x1c, 0xc2339f3d);
        put(&mut bytes, 0x210, GC_SIZE as u32);
        put(&mut bytes, 0x458, 1);
    }
    bytes
}

fn with_source<T>(bytes: &[u8], test: impl FnOnce(&Path) -> T) -> T {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("content.not-an-iso");
    fs::write(&path, bytes).unwrap();
    test(&path)
}

#[test]
fn gamecube_content_identification_and_exact_fields() {
    let mut bytes = fixture(false);
    bytes[6] = 1;
    bytes[7] = 2;
    bytes[0x214..0x218].copy_from_slice(b"TEST");
    let info = with_source(&bytes, |p| inspect(p).unwrap());
    assert_eq!(info.platform, NkitPlatform::GameCube);
    assert_eq!(&info.game_id, b"GTSE01");
    assert_eq!((info.disc_number, info.revision), (1, 2));
    assert_eq!(&info.title[..9], b"Synthetic");
    assert_eq!(info.region_word, 1);
    assert_eq!(info.claimed_original_size, GC_SIZE);
    assert_eq!(info.stored_size, bytes.len() as u64);
    assert_eq!(info.claimed_original_crc32, 0x12345678);
    assert_eq!(&info.forced_junk_id, b"TEST");
}

#[test]
fn wii_word_length_is_widened_before_multiplication() {
    let info = with_source(&fixture(true), |p| inspect(p).unwrap());
    assert_eq!(info.platform, NkitPlatform::Wii);
    assert_eq!(info.claimed_original_size, 8_511_160_320);
    assert_eq!(info.region_word, 2);
    assert_eq!(info.title[63], 0); // hash/encryption flags are not title bytes
}

#[test]
fn raw_unknown_text_and_region_survive() {
    let mut bytes = fixture(false);
    bytes[3] = 0xff;
    bytes[0x25] = 0xfe;
    put(&mut bytes, 0x458, u32::MAX);
    let info = with_source(&bytes, |p| inspect(p).unwrap());
    assert_eq!(info.game_id[3], 0xff);
    assert_eq!(info.title[5], 0xfe);
    assert_eq!(info.region_word, u32::MAX);
}

#[test]
fn malformed_signature_is_not_nkit_even_with_iso_magic() {
    let mut bytes = fixture(false);
    bytes[0x200] = b'X';
    assert_eq!(
        with_source(&bytes, inspect),
        Err(NkitInspectionError::NotNkit)
    );
}

#[test]
fn unknown_version_and_gcz_are_not_guessed() {
    let mut bytes = fixture(false);
    bytes[0x207] = b'2';
    assert!(matches!(
        with_source(&bytes, inspect),
        Err(NkitInspectionError::Unsupported(_))
    ));
    bytes[..4].copy_from_slice(&[1, 0xc0, 0x0b, 0xb1]);
    assert!(matches!(
        with_source(&bytes, inspect),
        Err(NkitInspectionError::Unsupported(_))
    ));
}

#[test]
fn missing_or_conflicting_platform_magic_refuses() {
    let mut bytes = fixture(false);
    put(&mut bytes, 0x1c, 0);
    assert!(matches!(
        with_source(&bytes, inspect),
        Err(NkitInspectionError::Malformed(_))
    ));
    put(&mut bytes, 0x1c, 0xc2339f3d);
    put(&mut bytes, 0x18, 0x5d1c9ea3);
    assert!(matches!(
        with_source(&bytes, inspect),
        Err(NkitInspectionError::Malformed(_))
    ));
}

#[test]
fn truncated_headers_and_platform_minimums_refuse() {
    for wii in [false, true] {
        let bytes = fixture(wii);
        for size in [0, 3, 0x207, 0x43f, bytes.len() - 1] {
            assert_eq!(
                with_source(&bytes[..size], inspect),
                Err(NkitInspectionError::Truncated)
            );
        }
    }
}

#[test]
fn impossible_and_out_of_policy_sizes_refuse() {
    for wii in [false, true] {
        let mut bytes = fixture(wii);
        put(&mut bytes, 0x210, 0);
        assert!(matches!(
            with_source(&bytes, inspect),
            Err(NkitInspectionError::Malformed(_))
        ));
        put(&mut bytes, 0x210, u32::MAX);
        assert!(matches!(
            with_source(&bytes, inspect),
            Err(NkitInspectionError::Unsupported(_))
        ));
    }
}

#[test]
fn mismatched_platform_recovery_metadata_refuses() {
    let mut bytes = fixture(false);
    put(&mut bytes, 0x218, 42);
    assert!(matches!(
        with_source(&bytes, inspect),
        Err(NkitInspectionError::Malformed(_))
    ));
    bytes = fixture(true);
    bytes[0x61] = 0;
    assert!(matches!(
        with_source(&bytes, inspect),
        Err(NkitInspectionError::Unsupported(_))
    ));
}

#[test]
fn no_declared_recovery_object_is_not_proof_of_reversibility() {
    for wii in [false, true] {
        let preview = with_source(&fixture(wii), |p| preview_iso_recovery(p, None).unwrap());
        assert_eq!(
            preview.representation,
            NkitRepresentationState::NkitV1HeaderRecognizedBodyUnverified
        );
        assert_eq!(preview.recoverability, NkitRecoverability::Unknown);
        assert_eq!(preview.update_recovery, UpdateRecoveryEvidence::NotDeclared);
        assert!(!preview.blockers.is_empty());
    }
}

#[test]
fn required_recovery_metadata_present_but_object_missing() {
    let mut bytes = fixture(true);
    put(&mut bytes, 0x218, 0x87654321);
    let preview = with_source(&bytes, |p| preview_iso_recovery(p, None).unwrap());
    assert_eq!(
        preview.recoverability,
        NkitRecoverability::DependenciesMissing
    );
    assert_eq!(preview.header.removed_update_crc32, Some(0x87654321));
    assert_eq!(
        preview.update_recovery,
        UpdateRecoveryEvidence::Missing {
            expected_crc32: 0x87654321
        }
    );
}

#[test]
fn supplied_object_crc_is_measured_not_inferred_from_filename() {
    let mut bytes = fixture(true);
    put(&mut bytes, 0x218, 0xcbf43926); // standard CRC32 vector
    with_source(b"123456789", |resource| {
        let candidate = UpdateRecoveryCandidate {
            path: resource,
            declared_recovery_span_crc32: 0xcbf43926,
        };
        let preview = with_source(&bytes, |p| {
            preview_iso_recovery(p, Some(candidate)).unwrap()
        });
        assert_eq!(preview.recoverability, NkitRecoverability::Unknown);
        match preview.update_recovery {
            UpdateRecoveryEvidence::Candidate {
                measured_crc32,
                measured_sha256,
                size,
                ..
            } => {
                assert_eq!(measured_crc32, 0xcbf43926);
                assert_eq!(
                    measured_sha256,
                    <[u8; 32]>::from(Sha256::digest(b"123456789"))
                );
                assert_eq!(size, 9);
            }
            _ => panic!("expected measured candidate"),
        }
        // Nine arbitrary bytes with matching CRC must never mean recoverable.
        assert!(
            preview
                .blockers
                .iter()
                .any(|b| b.contains("compatibility unchecked"))
        );
        put(&mut bytes, 0x218, 1);
        assert!(with_source(&bytes, |p| preview_iso_recovery(p, Some(candidate))).is_err());
        // Stored-file CRC and reconstructed-span CRC have different domains.
        // Neither agreement nor disagreement certifies the missing filler.
        let declared = UpdateRecoveryCandidate {
            declared_recovery_span_crc32: 1,
            ..candidate
        };
        let unchecked = with_source(&bytes, |p| preview_iso_recovery(p, Some(declared)).unwrap());
        assert_eq!(unchecked.recoverability, NkitRecoverability::Unknown);
        assert!(matches!(
            unchecked.update_recovery,
            UpdateRecoveryEvidence::Candidate {
                expected_crc32: 1,
                measured_crc32: 0xcbf43926,
                ..
            }
        ));
    });
}

#[test]
fn unexpected_recovery_object_refuses() {
    with_source(&fixture(false), |p| {
        assert!(
            preview_iso_recovery(
                p,
                Some(UpdateRecoveryCandidate {
                    path: p,
                    declared_recovery_span_crc32: 1
                })
            )
            .is_err()
        );
    });
}

#[test]
fn inspection_and_preview_leave_sources_and_directory_unchanged() {
    let mut bytes = fixture(true);
    let resource = vec![0x5a; 2 * 1024 * 1024 + 17];
    let mut crc = Crc32::new();
    crc.update(&resource);
    put(&mut bytes, 0x218, crc.finish());
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("image");
    let recovery = dir.path().join("recovery");
    fs::write(&image, &bytes).unwrap();
    fs::write(&recovery, &resource).unwrap();
    let before = (
        fs::metadata(&image).unwrap().modified().unwrap(),
        fs::metadata(&recovery).unwrap().modified().unwrap(),
    );
    assert_eq!(
        preview_iso_recovery(
            &image,
            Some(UpdateRecoveryCandidate {
                path: &recovery,
                declared_recovery_span_crc32: be32(&bytes, 0x218)
            })
        )
        .unwrap()
        .recoverability,
        NkitRecoverability::Unknown
    );
    assert_eq!(fs::read(&image).unwrap(), bytes);
    assert_eq!(fs::read(&recovery).unwrap(), resource);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    assert_eq!(
        before,
        (
            fs::metadata(&image).unwrap().modified().unwrap(),
            fs::metadata(&recovery).unwrap().modified().unwrap()
        )
    );
}

#[test]
fn full_disc_sparse_image_needs_only_fixed_header_reads() {
    with_source(&fixture(true), |p| {
        fs::OpenOptions::new()
            .write(true)
            .open(p)
            .unwrap()
            .set_len(WII_SIZE)
            .unwrap();
        let info = inspect(p).unwrap();
        assert_eq!(info.stored_size, WII_SIZE);
        assert_eq!(
            preview_iso_recovery(p, None).unwrap().recoverability,
            NkitRecoverability::Unknown
        );
    });
}

#[test]
fn nonexistent_relative_and_directory_paths_refuse() {
    assert!(inspect(Path::new("relative.nkit.iso")).is_err());
    let dir = tempfile::tempdir().unwrap();
    assert!(inspect(dir.path()).is_err());
    assert!(inspect(&dir.path().join("missing")).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_leaf_and_ancestor_refuse() {
    with_source(&fixture(false), |p| {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(p, &link).unwrap();
        assert!(inspect(&link).is_err());
        let ancestor = dir.path().join("ancestor");
        std::os::unix::fs::symlink(p.parent().unwrap(), &ancestor).unwrap();
        assert!(inspect(&ancestor.join(p.file_name().unwrap())).is_err());
    });
}
