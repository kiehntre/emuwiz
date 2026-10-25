//! CBR reader tests. Fixtures are synthesised in memory with the `rars` writer
//! (dev-dependency only): no third-party archive, ROM or proprietary tool is
//! involved, and every page is a few-hundred-byte generated PNG.
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use rars::{ArchiveVersion, Builder};

use super::super::*;
use super::ManualCancellation;

fn png(width: u32, height: u32, shade: u8) -> Vec<u8> {
    let image =
        image::RgbaImage::from_pixel(width, height, image::Rgba([shade, 0, 255 - shade, 255]));
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png encode");
    out.into_inner()
}

fn archive(
    version: ArchiveVersion,
    members: &[(&str, Vec<u8>)],
    solid: bool,
    store: bool,
) -> Vec<u8> {
    let mut builder = Builder::new(version).solid(solid).store(store);
    for (name, data) in members {
        builder
            .add_bytes(name.as_bytes().to_vec(), data.clone(), None, None)
            .expect("add member");
    }
    builder.to_bytes().expect("build archive")
}

fn pages(n: u8) -> Vec<(String, Vec<u8>)> {
    (0..n)
        .map(|i| {
            (
                format!("page{:02}.png", n - i),
                png(8 + u32::from(i), 6, i * 20),
            )
        })
        .collect()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn borrowed(members: &[(String, Vec<u8>)]) -> Vec<(&str, Vec<u8>)> {
    members
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect()
}

const ALL: [ArchiveVersion; 4] = [
    ArchiveVersion::Rar15,
    ArchiveVersion::Rar29,
    ArchiveVersion::Rar50,
    ArchiveVersion::Rar70,
];

#[test]
fn multi_page_cbr_reads_in_natural_order_for_every_format_and_mode() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(12);
    for version in ALL {
        for (solid, store) in [(false, false), (true, false), (false, true)] {
            let label = format!("{version:?} solid={solid} store={store}");
            let bytes = archive(version, &borrowed(&members), solid, store);
            let path = write(dir.path(), "m.cbr", &bytes);
            let document = ManualDocument::open(&path, &ManualLimits::default())
                .unwrap_or_else(|e| panic!("{label}: {e:?}"));
            let inspection = document.inspection();
            assert_eq!(inspection.kind, ManualDocumentKind::Cbr, "{label}");
            assert_eq!(inspection.readiness, ManualReadiness::Viewable, "{label}");
            assert_eq!(inspection.page_count, Some(12), "{label}");
            let names: Vec<&str> = inspection.pages.iter().map(|p| p.name.as_str()).collect();
            let want: Vec<String> = (1..=12).map(|n| format!("page{n:02}.png")).collect();
            assert_eq!(
                names,
                want.iter().map(String::as_str).collect::<Vec<_>>(),
                "{label}"
            );
            // Read out of order: later pages of a solid archive need their
            // predecessors, and an earlier page after it must still be correct.
            for index in [11usize, 0, 5, 11] {
                let image = document
                    .decode_page(index)
                    .unwrap_or_else(|e| panic!("{label} page {index}: {e:?}"));
                let (_, original) = members
                    .iter()
                    .find(|(n, _)| *n == inspection.pages[index].name)
                    .unwrap();
                let expected = image::load_from_memory(original).unwrap().to_rgba8();
                assert_eq!(
                    (image.width, image.height),
                    expected.dimensions(),
                    "{label}"
                );
                assert_eq!(image.rgba, expected.into_raw(), "{label} page {index}");
            }
        }
    }
}

#[test]
fn identification_is_by_signature_not_extension() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(2);
    let rar = archive(ArchiveVersion::Rar50, &borrowed(&members), false, false);
    let path = write(dir.path(), "disguised.cbz", &rar);
    let inspection = inspect_manual(&path, &ManualLimits::default()).unwrap();
    assert_eq!(inspection.kind, ManualDocumentKind::Cbr);
    assert!(
        inspection
            .warnings
            .iter()
            .any(|w| matches!(w, ManualWarning::ExtensionMismatch { .. }))
    );
}

#[test]
fn directories_and_junk_members_are_ignored_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let mut builder = Builder::new(ArchiveVersion::Rar50);
    builder
        .add_directory(b"Scans".to_vec(), None, None)
        .unwrap();
    builder
        .add_bytes(b"Scans/p1.png".to_vec(), png(4, 4, 10), None, None)
        .unwrap();
    builder
        .add_bytes(b"Thumbs.db".to_vec(), b"junk".to_vec(), None, None)
        .unwrap();
    builder
        .add_bytes(b"notes.txt".to_vec(), b"hello".to_vec(), None, None)
        .unwrap();
    let path = write(dir.path(), "d.cbr", &builder.to_bytes().unwrap());
    let inspection = inspect_manual(&path, &ManualLimits::default()).unwrap();
    assert_eq!(inspection.page_count, Some(1));
    assert!(
        inspection
            .warnings
            .contains(&ManualWarning::IgnoredMembers(2))
    );
}

#[test]
fn navigation_zoom_and_resume_state_work_for_cbr_documents() {
    // The viewer state machine is format-independent; prove it is driven by a
    // real CBR page count and keeps its position when the same file is reopened.
    let dir = tempfile::tempdir().unwrap();
    let members = pages(5);
    let path = write(
        dir.path(),
        "n.cbr",
        &archive(ArchiveVersion::Rar29, &borrowed(&members), true, false),
    );
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    let count = document.inspection().pages.len();
    assert_eq!(count, 5);
    let mut state = ManualViewerState::closed();
    state.open(document.id().clone(), count);
    assert!(state.apply(ManualViewerAction::NextPage));
    assert!(state.apply(ManualViewerAction::NextPage));
    assert!(state.apply(ManualViewerAction::ZoomIn));
    assert_eq!(state.current_page(), 2);
    assert_eq!(state.zoom(), ManualZoom::Percent(125));
    assert!(document.decode_page(state.current_page()).is_ok());
    // Reopening the same, unchanged file resumes at the same page and zoom.
    let reopened = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    state.open(reopened.id().clone(), reopened.inspection().pages.len());
    assert_eq!(state.current_page(), 2);
    assert_eq!(state.zoom(), ManualZoom::Percent(125));
    assert!(state.apply(ManualViewerAction::PreviousPage));
    assert_eq!(state.page_number(), 2);
    assert!(state.apply(ManualViewerAction::LastPage));
    assert!(!state.can_go_next());
    assert!(reopened.decode_page(state.current_page()).is_ok());
}

#[test]
fn truncated_and_corrupted_archives_error_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(4);
    for version in ALL {
        let bytes = archive(version, &borrowed(&members), true, false);
        let step = (bytes.len() / 40).max(1);
        for cut in (0..bytes.len()).step_by(step) {
            let path = write(dir.path(), "t.cbr", &bytes[..cut]);
            if let Ok(document) = ManualDocument::open(&path, &ManualLimits::default()) {
                for page in 0..document.inspection().pages.len() {
                    let _ = document.decode_page(page); // Ok or Err; a panic fails.
                }
            }
        }
        for position in (0..bytes.len()).step_by(step) {
            let mut flipped = bytes.clone();
            flipped[position] ^= 0xA5;
            let path = write(dir.path(), "c.cbr", &flipped);
            if let Ok(document) = ManualDocument::open(&path, &ManualLimits::default()) {
                for page in 0..document.inspection().pages.len() {
                    let _ = document.decode_page(page);
                }
            }
        }
    }
}

#[test]
fn a_truncated_archive_never_yields_a_partial_page() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(3);
    let bytes = archive(ArchiveVersion::Rar50, &borrowed(&members), false, false);
    let path = write(dir.path(), "full.cbr", &bytes);
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    // Cut the file after inspection; the identity check must refuse the read.
    fs::write(&path, &bytes[..bytes.len() - 20]).unwrap();
    assert_eq!(
        document.read_page_bytes(0),
        Err(ManualViewerError::SourceChanged)
    );
}

#[test]
fn highly_compressible_members_are_refused_as_bombs() {
    let dir = tempfile::tempdir().unwrap();
    // 8 MiB of zeros packs to a few hundred bytes: far beyond 200:1.
    let bomb = vec![0u8; 8 * 1024 * 1024];
    for version in ALL {
        let bytes = archive(version, &[("bomb.png", bomb.clone())], false, false);
        let path = write(dir.path(), "b.cbr", &bytes);
        let error = ManualDocument::open(&path, &ManualLimits::default()).unwrap_err();
        assert!(
            matches!(error, ManualViewerError::SuspiciousCompression { .. }),
            "{version:?}: {error:?}"
        );
    }
}

#[test]
fn declared_size_and_member_count_limits_apply() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(6);
    let path = write(
        dir.path(),
        "l.cbr",
        &archive(ArchiveVersion::Rar50, &borrowed(&members), false, true),
    );
    let limits = ManualLimits {
        max_members: 3,
        ..ManualLimits::default()
    };
    assert!(matches!(
        ManualDocument::open(&path, &limits).unwrap_err(),
        ManualViewerError::TooManyMembers { .. }
    ));
    let limits = ManualLimits {
        max_member_bytes: 16,
        ..ManualLimits::default()
    };
    assert!(matches!(
        ManualDocument::open(&path, &limits).unwrap_err(),
        ManualViewerError::MemberTooLarge { .. }
    ));
    let limits = ManualLimits {
        max_total_uncompressed_bytes: 64,
        ..ManualLimits::default()
    };
    assert!(matches!(
        ManualDocument::open(&path, &limits).unwrap_err(),
        ManualViewerError::ArchiveTooLarge { .. }
    ));
    let limits = ManualLimits {
        max_pages: 2,
        ..ManualLimits::default()
    };
    assert!(matches!(
        ManualDocument::open(&path, &limits).unwrap_err(),
        ManualViewerError::TooManyPages { .. }
    ));
}

#[test]
fn oversized_page_dimensions_are_refused_before_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "big.cbr",
        &archive(
            ArchiveVersion::Rar50,
            &[("p.png", png(64, 64, 1))],
            false,
            false,
        ),
    );
    let limits = ManualLimits {
        max_image_dimension: 32,
        ..ManualLimits::default()
    };
    let document = ManualDocument::open(&path, &limits).unwrap();
    assert!(matches!(
        document.decode_page(0),
        Err(ManualViewerError::ImageTooLarge { .. })
    ));
}

#[test]
fn nested_archives_and_unsupported_images_are_refused_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let nested = archive(
        ArchiveVersion::Rar50,
        &[("a.png", png(4, 4, 1)), ("inner.zip", b"PK".to_vec())],
        false,
        false,
    );
    let path = write(dir.path(), "n.cbr", &nested);
    assert!(matches!(
        inspect_manual(&path, &ManualLimits::default()).unwrap_err(),
        ManualViewerError::NestedArchive { .. }
    ));
    let only_tiff = archive(
        ArchiveVersion::Rar50,
        &[("a.tiff", vec![1, 2, 3])],
        false,
        false,
    );
    let path = write(dir.path(), "t.cbr", &only_tiff);
    assert!(matches!(
        inspect_manual(&path, &ManualLimits::default()).unwrap_err(),
        ManualViewerError::UnsupportedImage { .. }
    ));
}

#[test]
fn symlinks_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    for version in [ArchiveVersion::Rar50, ArchiveVersion::Rar70] {
        let mut builder = Builder::new(version);
        builder
            .add_bytes(b"p.png".to_vec(), png(4, 4, 1), None, None)
            .unwrap();
        builder
            .add_unix_symlink(
                b"link.png".to_vec(),
                b"/etc/passwd".to_vec(),
                false,
                None,
                None,
            )
            .unwrap();
        let path = write(dir.path(), "s.cbr", &builder.to_bytes().unwrap());
        let error = inspect_manual(&path, &ManualLimits::default()).unwrap_err();
        assert!(
            matches!(
                error,
                ManualViewerError::UnsafeMember {
                    reason: UnsafeMemberReason::Symlink,
                    ..
                }
            ),
            "{version:?}: {error:?}"
        );
    }
}

#[test]
fn encrypted_archives_are_refused_without_prompting() {
    let dir = tempfile::tempdir().unwrap();
    for version in [ArchiveVersion::Rar29, ArchiveVersion::Rar50] {
        for headers in [false, true] {
            let mut builder = Builder::new(version)
                .password(Some(b"secret".to_vec()))
                .header_encryption(headers);
            builder
                .add_bytes(b"p.png".to_vec(), png(4, 4, 1), None, None)
                .unwrap();
            let Ok(bytes) = builder.to_bytes() else {
                // The fixture writer does not offer header encryption for RAR 2.9.
                assert!(headers, "{version:?} must build with data encryption");
                continue;
            };
            let path = write(dir.path(), "e.cbr", &bytes);
            let error = inspect_manual(&path, &ManualLimits::default()).unwrap_err();
            assert!(
                matches!(
                    error,
                    ManualViewerError::EncryptedMember { .. }
                        | ManualViewerError::UnsupportedArchive(ArchiveLimitation::Encrypted)
                ),
                "{version:?} headers={headers}: {error:?}"
            );
        }
    }
}

#[test]
fn multi_volume_archives_are_refused_with_a_clear_reason() {
    let dir = tempfile::tempdir().unwrap();
    let mut builder = Builder::new(ArchiveVersion::Rar50)
        .volume_size(Some(4096))
        .store(true);
    for i in 0..4u8 {
        // Incompressible filler: only the container is under test here.
        let filler: Vec<u8> = (0..6000u32)
            .map(|n| (n.wrapping_mul(2654435761) >> 13) as u8 ^ i)
            .collect();
        builder
            .add_bytes(format!("p{i}.png").into_bytes(), filler, None, None)
            .unwrap();
    }
    let volumes = builder.build_volumes(None).unwrap();
    assert!(volumes.len() > 1, "fixture must really be multi-volume");
    let path = write(dir.path(), "v.part1.cbr", &volumes[0]);
    let error = inspect_manual(&path, &ManualLimits::default()).unwrap_err();
    assert_eq!(
        error,
        ManualViewerError::UnsupportedArchive(ArchiveLimitation::MultiVolume)
    );
    assert!(error.user_message().contains("split"));
}

#[test]
fn cancellation_returns_no_page_and_does_not_poison_later_reads() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(4);
    let path = write(
        dir.path(),
        "c.cbr",
        &archive(ArchiveVersion::Rar50, &borrowed(&members), true, false),
    );
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    let token = ManualCancellation::new();
    token.cancel();
    assert_eq!(
        document.read_page_bytes_cancellable(1, Some(&token)),
        Err(ManualViewerError::Cancelled)
    );
    assert_eq!(
        document.decode_page_cancellable(1, Some(&token)),
        Err(ManualViewerError::Cancelled)
    );
    // A fresh token (or none) reads the same page normally.
    assert!(
        document
            .decode_page_cancellable(1, Some(&ManualCancellation::new()))
            .is_ok()
    );
    assert!(document.decode_page(1).is_ok());
}

#[test]
fn replacing_the_file_between_inspection_and_read_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let first = pages(3);
    let path = write(
        dir.path(),
        "r.cbr",
        &archive(ArchiveVersion::Rar50, &borrowed(&first), false, false),
    );
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    let other: Vec<(String, Vec<u8>)> = vec![("zzz.png".into(), png(30, 30, 99))];
    fs::write(
        &path,
        archive(ArchiveVersion::Rar50, &borrowed(&other), false, false),
    )
    .unwrap();
    assert_eq!(
        document.read_page_bytes(0),
        Err(ManualViewerError::SourceChanged)
    );
    assert_eq!(
        document.check_unchanged(),
        Err(ManualViewerError::SourceChanged)
    );
}

#[test]
fn reading_pages_never_modifies_the_source_or_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(5);
    let bytes = archive(ArchiveVersion::Rar29, &borrowed(&members), true, false);
    let path = write(dir.path(), "ro.cbr", &bytes);
    let before = fs::metadata(&path).unwrap();
    let listing = |d: &Path| {
        let mut names: Vec<_> = fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let names = listing(dir.path());
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    for page in 0..5 {
        document.decode_page(page).unwrap();
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
    assert_eq!(
        listing(dir.path()),
        names,
        "nothing may be written beside the document"
    );
}

#[test]
fn a_symlinked_cbr_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let members = pages(1);
    let real = write(
        dir.path(),
        "real.cbr",
        &archive(ArchiveVersion::Rar50, &borrowed(&members), false, false),
    );
    let link = dir.path().join("link.cbr");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(
        inspect_manual(&link, &ManualLimits::default()).unwrap_err(),
        ManualViewerError::NotARegularFile
    );
}

/// Writes fixtures for one-off cross-checking with an independent RAR decoder
/// (`unrar t`, `7z t`) during review. Not part of the normal run.
#[test]
#[ignore = "writes fixtures to $EMUWIZ_RAR_FIXTURE_DIR for external decoder cross-checks"]
fn dump_fixtures_for_external_validation() {
    let dir =
        PathBuf::from(std::env::var("EMUWIZ_RAR_FIXTURE_DIR").expect("set EMUWIZ_RAR_FIXTURE_DIR"));
    fs::create_dir_all(&dir).unwrap();
    let members = pages(6);
    for version in ALL {
        for (solid, store) in [(false, false), (true, false), (false, true)] {
            let name = format!(
                "{version:?}-solid{}-store{}.cbr",
                u8::from(solid),
                u8::from(store)
            );
            fs::write(
                dir.join(name),
                archive(version, &borrowed(&members), solid, store),
            )
            .unwrap();
        }
    }
}

#[test]
fn dos_reparse_point_members_are_treated_as_links() {
    let dir = tempfile::tempdir().unwrap();
    for version in [ArchiveVersion::Rar29, ArchiveVersion::Rar50] {
        let mut builder = Builder::new(version);
        builder
            .add_bytes(b"p.png".to_vec(), png(4, 4, 1), None, None)
            .unwrap();
        builder
            .add_bytes(b"l.png".to_vec(), png(4, 4, 2), None, None)
            .unwrap();
        builder.set_dos_attributes(b"l.png", 0x400).unwrap();
        let path = write(dir.path(), "r.cbr", &builder.to_bytes().unwrap());
        let error = inspect_manual(&path, &ManualLimits::default()).unwrap_err();
        assert!(
            matches!(
                error,
                ManualViewerError::UnsafeMember {
                    reason: UnsafeMemberReason::Symlink,
                    ..
                }
            ),
            "{version:?}: {error:?}"
        );
    }
}

#[test]
fn decoder_errors_map_to_accurate_user_facing_reasons() {
    use rars::Error as E;
    let cases: Vec<(E, ArchiveLimitation)> = vec![
        (
            E::FeatureDisabled {
                feature: "encryption",
            },
            ArchiveLimitation::Encrypted,
        ),
        (
            E::HeaderBytesLimitExceeded {
                limit: 1,
                required: 2,
            },
            ArchiveLimitation::Resources,
        ),
        (
            E::Rar50DictionaryLimitExceeded {
                limit: 1,
                required: 2,
            },
            ArchiveLimitation::Resources,
        ),
    ];
    for (error, want) in cases {
        assert_eq!(
            super::map_error(&error, None),
            ManualViewerError::UnsupportedArchive(want),
            "{error}"
        );
    }
    assert_eq!(
        super::map_error(&E::Cancelled, None),
        ManualViewerError::Cancelled
    );
}

// ---- header-forging helpers (fixtures only) --------------------------------

fn vint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let low = (value & 0x7F) as u8;
        value >>= 7;
        out.push(low | if value == 0 { 0 } else { 0x80 });
        if value == 0 {
            return out;
        }
    }
}

fn read_vint(bytes: &[u8]) -> (u64, usize) {
    let (mut value, mut shift) = (0u64, 0);
    for (i, byte) in bytes.iter().enumerate() {
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return (value, i + 1);
        }
        shift += 7;
    }
    panic!("unterminated vint");
}

/// Replace `from` with `to` (same encoded length) inside the first RAR5 file
/// header and recompute that header's CRC32, so the forged value is accepted.
fn forge_rar5_unpacked_size(archive: &mut [u8], from: u64, to: u64) {
    let (from_bytes, to_bytes) = (vint(from), vint(to));
    assert_eq!(
        from_bytes.len(),
        to_bytes.len(),
        "forged vint must keep its length"
    );
    let mut pos = 8; // after the 8-byte RAR5 signature
    for _ in 0..8 {
        let (size, len) = read_vint(&archive[pos + 4..]);
        let end = pos + 4 + len + size as usize;
        let header = &mut archive[pos + 4..end];
        if let Some(at) = header
            .windows(from_bytes.len())
            .position(|w| w == from_bytes.as_slice())
        {
            header[at..at + to_bytes.len()].copy_from_slice(&to_bytes);
            let crc = rars::crc32::crc32(&archive[pos + 4..end]);
            archive[pos..pos + 4].copy_from_slice(&crc.to_le_bytes());
            return;
        }
        // Skip this block's data area (flag 0x2 in the header flags) if any.
        let (_, tlen) = read_vint(&archive[pos + 4 + len..]);
        let (flags, flen) = read_vint(&archive[pos + 4 + len + tlen..]);
        let mut next = end;
        if flags & 0x2 != 0 {
            let at = pos + 4 + len + tlen + flen;
            let at = if flags & 0x1 != 0 {
                at + read_vint(&archive[at..]).1 // skip extra-area size
            } else {
                at
            };
            next += read_vint(&archive[at..]).0 as usize;
        }
        pos = next;
    }
    panic!("unpacked size not found in a RAR5 header");
}

/// RAR 1.5-4.x: offset of the first file header after the marker and main header.
fn rar4_file_header(archive: &[u8]) -> (usize, usize) {
    assert_eq!(&archive[..7], b"Rar!\x1a\x07\x00");
    let main_size = u16::from_le_bytes([archive[7 + 5], archive[7 + 6]]) as usize;
    let start = 7 + main_size;
    assert_eq!(archive[start + 2], 0x74, "expected a file header");
    let size = u16::from_le_bytes([archive[start + 5], archive[start + 6]]) as usize;
    (start, size)
}

fn refresh_rar4_crc(archive: &mut [u8], start: usize, size: usize) {
    let crc = rars::crc32::crc32(&archive[start + 2..start + size]) as u16;
    archive[start..start + 2].copy_from_slice(&crc.to_le_bytes());
}

fn low_entropy(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            ((state >> 24) & 0x07) as u8
        })
        .collect()
}

// ---- 1. a header that understates the real output --------------------------

#[test]
fn understated_declared_size_is_refused_before_the_page_is_produced() {
    let dir = tempfile::tempdir().unwrap();
    const ACTUAL: usize = 300_000;
    const DECLARED: u64 = 100_000;
    let data = low_entropy(ACTUAL, 7);
    for (label, version) in [
        ("rar5", ArchiveVersion::Rar50),
        ("rar4", ArchiveVersion::Rar29),
    ] {
        for solid in [false, true] {
            let mut bytes = archive(version, &[("p.png", data.clone())], solid, false);
            if version == ArchiveVersion::Rar50 {
                forge_rar5_unpacked_size(&mut bytes, ACTUAL as u64, DECLARED);
            } else {
                let (start, size) = rar4_file_header(&bytes);
                bytes[start + 11..start + 15].copy_from_slice(&(DECLARED as u32).to_le_bytes());
                refresh_rar4_crc(&mut bytes, start, size);
            }
            let path = write(dir.path(), "lie.cbr", &bytes);
            // The configured per-member ceiling is also below the real output.
            let limits = ManualLimits {
                max_member_bytes: 150_000,
                ..ManualLimits::default()
            };
            let document = ManualDocument::open(&path, &limits)
                .unwrap_or_else(|e| panic!("{label} solid={solid}: {e:?}"));
            assert_eq!(document.inspection().pages[0].uncompressed_bytes, DECLARED);
            super::PEAK_ACCEPTED.with(|peak| peak.set(0));
            let result = document.read_page_bytes(0);
            let peak = super::PEAK_ACCEPTED.with(std::cell::Cell::get);
            assert!(
                result.is_err(),
                "{label} solid={solid}: must not return a page"
            );
            assert!(
                peak <= DECLARED,
                "{label} solid={solid}: accepted {peak} bytes, past the declared {DECLARED}"
            );
            assert!(peak < ACTUAL as u64);
        }
    }
}

// ---- 2. cancellation while a decode is running ------------------------------

#[test]
fn cancellation_stops_a_decode_that_is_already_running() {
    use std::time::{Duration, Instant};
    const SLOW_PAGE_BYTES: usize = 3 * 1024 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let data = low_entropy(SLOW_PAGE_BYTES, 11);
    // Fastest compression level keeps fixture synthesis cheap; decode stays slow.
    let mut builder = Builder::new(ArchiveVersion::Rar50).compression_level(Some(1));
    builder
        .add_bytes(b"big.png".to_vec(), data, None, None)
        .unwrap();
    let path = write(dir.path(), "slow.cbr", &builder.to_bytes().unwrap());
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();

    let started = Instant::now();
    let full = document.read_page_bytes(0).expect("uncancelled read");
    let full_time = started.elapsed();
    assert_eq!(full.len(), SLOW_PAGE_BYTES);
    assert!(
        full_time >= Duration::from_millis(150),
        "fixture decodes too quickly ({full_time:?}) to prove mid-decode cancellation"
    );

    let token = ManualCancellation::new();
    let trigger = token.clone();
    let delay = full_time / 8;
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(delay);
        trigger.cancel();
    });
    let started = Instant::now();
    let result = document.read_page_bytes_cancellable(0, Some(&token));
    let elapsed = started.elapsed();
    canceller.join().unwrap();
    assert_eq!(result, Err(ManualViewerError::Cancelled));
    assert!(
        elapsed < full_time * 7 / 10,
        "cancelled read took {elapsed:?} vs {full_time:?} uncancelled"
    );
    // The document is not poisoned: a fresh token reads the full page again.
    assert_eq!(document.read_page_bytes(0).unwrap().len(), SLOW_PAGE_BYTES);
}

// ---- 3. RAR 1.5-4.x host numbering (3 = Unix, 5 = BeOS treated as Unix) -----

#[test]
fn rar4_unix_and_beos_hosts_are_both_checked_for_links_and_special_files() {
    let dir = tempfile::tempdir().unwrap();
    for (host, mode, expect) in [
        (3u8, 0o120_777u32, Some(UnsafeMemberReason::Symlink)),
        (5, 0o120_777, Some(UnsafeMemberReason::Symlink)),
        (3, 0o020_644, Some(UnsafeMemberReason::SpecialFile)),
        (5, 0o020_644, Some(UnsafeMemberReason::SpecialFile)),
        (5, 0o100_644, None),
    ] {
        let mut builder = Builder::new(ArchiveVersion::Rar29);
        builder
            .add_bytes(b"p.png".to_vec(), png(4, 4, 1), None, Some(mode))
            .unwrap();
        let mut bytes = builder.to_bytes().unwrap();
        let (start, size) = rar4_file_header(&bytes);
        assert_eq!(
            bytes[start + 15],
            3,
            "the writer emits Unix host 3 for a Unix mode"
        );
        bytes[start + 15] = host;
        refresh_rar4_crc(&mut bytes, start, size);
        let path = write(dir.path(), "h.cbr", &bytes);
        let result = inspect_manual(&path, &ManualLimits::default());
        match expect {
            Some(reason) => assert!(
                matches!(
                    result,
                    Err(ManualViewerError::UnsafeMember { reason: r, .. }) if r == reason
                ),
                "host {host} mode {mode:o}: {result:?}"
            ),
            None => {
                let inspection =
                    result.unwrap_or_else(|e| panic!("host {host} mode {mode:o}: {e:?}"));
                assert_eq!(inspection.page_count, Some(1));
            }
        }
    }
}
