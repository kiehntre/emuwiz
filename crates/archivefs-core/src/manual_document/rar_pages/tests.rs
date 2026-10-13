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
    let mut limits = ManualLimits::default();
    limits.max_members = 3;
    assert!(matches!(
        ManualDocument::open(&path, &limits).unwrap_err(),
        ManualViewerError::TooManyMembers { .. }
    ));
    let mut limits = ManualLimits::default();
    limits.max_member_bytes = 16;
    assert!(matches!(
        ManualDocument::open(&path, &limits).unwrap_err(),
        ManualViewerError::MemberTooLarge { .. }
    ));
    let mut limits = ManualLimits::default();
    limits.max_total_uncompressed_bytes = 64;
    assert!(matches!(
        ManualDocument::open(&path, &limits).unwrap_err(),
        ManualViewerError::ArchiveTooLarge { .. }
    ));
    let mut limits = ManualLimits::default();
    limits.max_pages = 2;
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
    let mut limits = ManualLimits::default();
    limits.max_image_dimension = 32;
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
