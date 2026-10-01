//! Synthetic fixtures only: every PDF, ZIP and image here is built in memory.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::ZlibEncoder;
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use super::order::natural_path_cmp_for_tests;
use super::*;

// ------------------------------------------------------------- helpers ----

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 120, 200, 255]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    zip_with(entries, CompressionMethod::Stored)
}

fn zip_with(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(method);
    for (name, data) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn cbz(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    write(dir, name, &zip_of(entries))
}

fn inspect(path: &Path) -> Result<ManualInspection, ManualViewerError> {
    inspect_manual(path, &ManualLimits::default())
}

fn page_names(inspection: &ManualInspection) -> Vec<&str> {
    inspection.pages.iter().map(|p| p.name.as_str()).collect()
}

fn tight(f: impl FnOnce(&mut ManualLimits)) -> ManualLimits {
    let mut limits = ManualLimits::default();
    f(&mut limits);
    limits
}

// ------------------------------------------------------- PDF builders ----

/// A classic-xref PDF. `catalog_extra` and `trailer_extra` are spliced in
/// verbatim; `prefix` bytes precede `%PDF-` (xref offsets are header relative).
struct PdfBuilder {
    pages: i64,
    catalog_extra: String,
    trailer_extra: String,
    info: Option<String>,
    prefix: Vec<u8>,
}

impl PdfBuilder {
    fn new(pages: i64) -> Self {
        Self {
            pages,
            catalog_extra: String::new(),
            trailer_extra: String::new(),
            info: None,
            prefix: Vec::new(),
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut objects: Vec<String> = vec![
            format!("<< /Type /Catalog /Pages 2 0 R {} >>", self.catalog_extra),
            format!("<< /Type /Pages /Kids [3 0 R] /Count {} >>", self.pages),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
        ];
        if let Some(info) = &self.info {
            objects.push(info.clone());
        }
        let mut body = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(body.len());
            body.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
        }
        let xref_at = body.len();
        body.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        body.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            body.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let info_ref = if self.info.is_some() {
            " /Info 4 0 R"
        } else {
            ""
        };
        body.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R{info_ref} {} >>\nstartxref\n{xref_at}\n%%EOF\n",
                objects.len() + 1,
                self.trailer_extra
            )
            .as_bytes(),
        );
        let mut out = self.prefix.clone();
        out.extend(body);
        out
    }
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// PDF 1.5 style: catalog and page tree live in an object stream and the
/// cross-reference is a Flate stream with a PNG-Up predictor.
fn object_stream_pdf(pages: i64, with_encrypt: bool) -> Vec<u8> {
    let catalog = "<< /Type /Catalog /Pages 2 0 R >>";
    let page_tree = format!("<< /Type /Pages /Kids [] /Count {pages} >>");
    let header = format!("1 0 2 {} ", catalog.len() + 1);
    let stm_data = format!("{header}{catalog} {page_tree}");
    let compressed = zlib(stm_data.as_bytes());

    let mut out = b"%PDF-1.5\n".to_vec();
    let stm_offset = out.len();
    out.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /ObjStm /N 2 /First {} /Filter /FlateDecode /Length {} >>\nstream\n",
            header.len(),
            compressed.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&compressed);
    out.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_offset = out.len();
    // W [1 2 1]: type, field2, field3. Objects 0..=6.
    let rows: [[u8; 4]; 7] = [
        [0, 0, 0, 0],
        [2, 0, 5, 0],
        [2, 0, 5, 1],
        [0, 0, 0, 0],
        [0, 0, 0, 0],
        [1, (stm_offset >> 8) as u8, stm_offset as u8, 0],
        [1, (xref_offset >> 8) as u8, xref_offset as u8, 0],
    ];
    // PNG "Up" predictor over the raw rows.
    let mut predicted = Vec::new();
    let mut previous = [0u8; 4];
    for row in rows {
        predicted.push(2u8);
        for i in 0..4 {
            predicted.push(row[i].wrapping_sub(previous[i]));
        }
        previous = row;
    }
    let packed = zlib(&predicted);
    let encrypt = if with_encrypt { " /Encrypt 9 0 R" } else { "" };
    out.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /W [1 2 1] /Root 1 0 R{encrypt} /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 >> /Length {} >>\nstream\n",
            packed.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&packed);
    out.extend_from_slice(
        format!("\nendstream\nendobj\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
    );
    out
}

// ------------------------------------------------------------ detection ----

#[test]
fn signatures_decide_the_kind_not_the_extension() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = PdfBuilder::new(4).build();
    let zip = zip_of(&[("1.png", &png(2, 2))]);

    // A PDF named .cbz is a PDF, a ZIP named .pdf is a CBZ, and both say so.
    let a = inspect(&write(dir.path(), "guide.cbz", &pdf)).unwrap();
    assert_eq!(a.kind, ManualDocumentKind::Pdf);
    assert_eq!(a.evidence.signature, ManualSignature::PdfHeader);
    assert!(a.warnings.contains(&ManualWarning::ExtensionMismatch {
        claimed: "cbz".into(),
        detected: ManualDocumentKind::Pdf,
    }));
    let b = inspect(&write(dir.path(), "manual.pdf", &zip)).unwrap();
    assert_eq!(b.kind, ManualDocumentKind::Cbz);
    assert!(
        b.warnings
            .iter()
            .any(|w| matches!(w, ManualWarning::ExtensionMismatch { .. }))
    );

    // A correctly named file produces no mismatch warning, and an odd
    // extension is not a mismatch at all.
    let c = inspect(&write(dir.path(), "ok.pdf", &pdf)).unwrap();
    assert!(
        !c.warnings
            .iter()
            .any(|w| matches!(w, ManualWarning::ExtensionMismatch { .. }))
    );
    let d = inspect(&write(dir.path(), "ok.bin", &pdf)).unwrap();
    assert!(
        !d.warnings
            .iter()
            .any(|w| matches!(w, ManualWarning::ExtensionMismatch { .. }))
    );
}

#[test]
fn unknown_content_is_refused_whatever_it_is_called() {
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        ("fake.pdf", b"just some text".as_slice()),
        ("fake.cbz", b"\x89PNG\r\n\x1a\n...."),
        ("empty.cbr", b""),
        ("seven.cbz", b"7z\xBC\xAF\x27\x1C\x00\x04"),
    ] {
        assert_eq!(
            inspect(&write(dir.path(), name, bytes)),
            Err(ManualViewerError::UnrecognisedFormat),
            "{name}"
        );
    }
}

#[test]
fn a_zip_that_merely_mentions_a_pdf_header_is_still_a_zip() {
    let dir = tempfile::tempdir().unwrap();
    let zip = zip_of(&[("%PDF-1.4", b"x"), ("1.png", &png(2, 2))]);
    assert_eq!(
        inspect(&write(dir.path(), "t.pdf", &zip)).unwrap().kind,
        ManualDocumentKind::Cbz
    );
}

#[test]
fn only_regular_files_are_opened() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(inspect(dir.path()), Err(ManualViewerError::NotARegularFile));
    assert!(matches!(
        inspect(&dir.path().join("missing.pdf")),
        Err(ManualViewerError::Io(_))
    ));
    #[cfg(unix)]
    {
        let real = write(dir.path(), "real.pdf", &PdfBuilder::new(1).build());
        let link = dir.path().join("link.pdf");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(inspect(&link), Err(ManualViewerError::NotARegularFile));
    }
}

#[test]
fn oversized_files_are_refused_before_being_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "big.pdf", &PdfBuilder::new(1).build());
    let limits = tight(|l| l.max_file_bytes = 100);
    assert!(matches!(
        inspect_manual(&path, &limits),
        Err(ManualViewerError::TooLarge { max: 100, .. })
    ));
}

#[test]
fn limits_can_only_be_tightened() {
    let loose = ManualLimits {
        max_members: usize::MAX,
        max_pages: usize::MAX,
        max_image_pixels: u64::MAX,
        ..ManualLimits::default()
    }
    .clamped_to_defaults();
    assert_eq!(loose, ManualLimits::default());
}

// ------------------------------------------------------------------ PDF ----

#[test]
fn a_classic_pdf_reports_pages_metadata_and_a_renderer_gap() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = PdfBuilder::new(12);
    b.info = Some(
        "<< /Title <FEFF004D0061006E00750061006C> /Author (A\\051 B) /Producer (Test) >>".into(),
    );
    let inspection = inspect(&write(dir.path(), "m.pdf", &b.build())).unwrap();
    assert_eq!(inspection.kind, ManualDocumentKind::Pdf);
    assert_eq!(inspection.page_count, Some(12));
    assert_eq!(inspection.metadata.title.as_deref(), Some("Manual"));
    assert_eq!(inspection.metadata.author.as_deref(), Some("A) B"));
    assert_eq!(inspection.display_title(), "Manual");
    assert!(inspection.pages.is_empty());
    assert_eq!(
        inspection.readiness,
        ManualReadiness::InspectOnly {
            missing: ManualCapabilityGap::PdfRenderer
        }
    );
    assert!(!inspection.readiness.can_view());
    assert!(
        inspection
            .warnings
            .contains(&ManualWarning::PageCountIsDeclared)
    );
}

#[test]
fn a_pdf_cannot_be_rendered_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "m.pdf", &PdfBuilder::new(3).build());
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    assert_eq!(
        document.decode_page(0).unwrap_err(),
        ManualViewerError::CapabilityUnavailable(ManualCapabilityGap::PdfRenderer)
    );
}

#[test]
fn object_streams_and_xref_streams_are_followed() {
    let dir = tempfile::tempdir().unwrap();
    let inspection = inspect(&write(
        dir.path(),
        "modern.pdf",
        &object_stream_pdf(37, false),
    ))
    .unwrap();
    assert_eq!(inspection.page_count, Some(37));
}

#[test]
fn encrypted_pdfs_report_that_state_and_are_not_opened() {
    let dir = tempfile::tempdir().unwrap();
    let mut classic = PdfBuilder::new(5);
    classic.trailer_extra = "/Encrypt 9 0 R".into();
    assert_eq!(
        inspect(&write(dir.path(), "a.pdf", &classic.build())),
        Err(ManualViewerError::Encrypted)
    );
    assert_eq!(
        inspect(&write(dir.path(), "b.pdf", &object_stream_pdf(5, true))),
        Err(ManualViewerError::Encrypted)
    );
    assert!(
        ManualViewerError::Encrypted
            .user_message()
            .contains("encrypted")
    );
    assert!(
        !ManualViewerError::Encrypted
            .to_string()
            .contains("password"),
        "must not claim a password is required"
    );
}

#[test]
fn incremental_updates_use_the_newest_definition() {
    let dir = tempfile::tempdir().unwrap();
    let base = PdfBuilder::new(1).build();
    let first_xref: usize = {
        let text = String::from_utf8_lossy(&base);
        text.rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let mut updated = base;
    let new_pages = updated.len();
    updated.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 9 >>\nendobj\n");
    let xref_at = updated.len();
    updated.extend_from_slice(
        format!(
            "xref\n2 1\n{new_pages:010} 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R /Prev {first_xref} >>\nstartxref\n{xref_at}\n%%EOF\n"
        )
        .as_bytes(),
    );
    let inspection = inspect(&write(dir.path(), "inc.pdf", &updated)).unwrap();
    assert_eq!(inspection.page_count, Some(9));
}

#[test]
fn junk_before_the_header_shifts_offsets_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = PdfBuilder::new(6);
    b.prefix = b"GARBAGE-BEFORE-HEADER\n".to_vec();
    assert_eq!(
        inspect(&write(dir.path(), "junk.pdf", &b.build()))
            .unwrap()
            .page_count,
        Some(6)
    );
}

#[test]
fn active_content_is_reported_never_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = PdfBuilder::new(2);
    b.catalog_extra = "/OpenAction << /S /JavaScript /JS (app.alert(1)) >> /AA << >> \
        /AcroForm << /Fields [] >> /Names << /JavaScript << >> /EmbeddedFiles << >> >>"
        .into();
    let inspection = inspect(&write(dir.path(), "active.pdf", &b.build())).unwrap();
    assert_eq!(
        inspection.active_content,
        vec![
            ManualActiveContent::OpenAction,
            ManualActiveContent::AdditionalActions,
            ManualActiveContent::JavaScript,
            ManualActiveContent::EmbeddedFiles,
            ManualActiveContent::InteractiveForm,
        ]
    );
    assert!(
        inspection
            .warnings
            .contains(&ManualWarning::ActiveContentIgnored)
    );
    assert_eq!(inspection.page_count, Some(2));

    let clean = inspect(&write(dir.path(), "clean.pdf", &PdfBuilder::new(2).build())).unwrap();
    assert!(clean.active_content.is_empty());
    assert!(
        !clean
            .warnings
            .contains(&ManualWarning::ActiveContentIgnored)
    );
}

#[test]
fn malformed_pdfs_are_refused_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let good = PdfBuilder::new(3).build();

    // Truncated: the tail with startxref is gone.
    let truncated = &good[..good.len() / 2];
    assert!(matches!(
        inspect(&write(dir.path(), "t.pdf", truncated)),
        Err(ManualViewerError::Malformed(_))
    ));
    // startxref points past the end of the file.
    let text = String::from_utf8(good.clone()).unwrap();
    let bad_offset = text.replace(
        &format!(
            "startxref\n{}",
            text.rsplit("startxref\n")
                .next()
                .unwrap()
                .lines()
                .next()
                .unwrap()
        ),
        "startxref\n99999999",
    );
    assert!(matches!(
        inspect(&write(dir.path(), "o.pdf", bad_offset.as_bytes())),
        Err(ManualViewerError::Malformed(_))
    ));
    // startxref points at garbage.
    let garbage = text.replacen("xref\n0 4", "yref\n0 4", 1);
    assert!(matches!(
        inspect(&write(dir.path(), "g.pdf", garbage.as_bytes())),
        Err(ManualViewerError::Malformed(_))
    ));
    // A catalog with no page tree, and a page tree with no count.
    let mut no_pages = PdfBuilder::new(1).build();
    let s = String::from_utf8(no_pages.clone())
        .unwrap()
        .replace("/Count 1", "/Cnt 1");
    no_pages = s.into_bytes();
    assert!(matches!(
        inspect(&write(dir.path(), "n.pdf", &no_pages)),
        Err(ManualViewerError::Malformed(_))
    ));
}

#[test]
fn a_cross_reference_loop_is_refused_not_followed_forever() {
    let dir = tempfile::tempdir().unwrap();
    // The newest trailer points /Prev at its own section. A fixed-width
    // placeholder keeps every offset valid when the real value is patched in.
    let mut builder = PdfBuilder::new(1);
    builder.trailer_extra = "/Prev 99999".into();
    let text = String::from_utf8(builder.build()).unwrap();
    let xref_at: usize = text
        .rsplit("startxref\n")
        .next()
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let looped = text.replace("/Prev 99999", &format!("/Prev {xref_at:05}"));
    assert_eq!(looped.len(), text.len());
    let result = inspect(&write(dir.path(), "loop.pdf", looped.as_bytes()));
    assert!(
        matches!(result, Err(ManualViewerError::Malformed(_))),
        "{result:?}"
    );
}

#[test]
fn a_pdf_padded_with_nuls_after_its_end_is_still_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = PdfBuilder::new(7).build();
    bytes.extend(std::iter::repeat_n(0u8, 3 * 1024 * 1024)); // far more than the tail window
    assert_eq!(
        inspect(&write(dir.path(), "padded.pdf", &bytes))
            .unwrap()
            .page_count,
        Some(7)
    );
    let mut spaces = PdfBuilder::new(7).build();
    spaces.extend(b"\r\n\r\n   \n".repeat(50));
    assert_eq!(
        inspect(&write(dir.path(), "ws.pdf", &spaces))
            .unwrap()
            .page_count,
        Some(7)
    );
}

#[test]
fn unbounded_padding_is_refused_not_scanned_forever() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = PdfBuilder::new(7).build();
    bytes.extend(std::iter::repeat_n(0u8, 200_000));
    let limits = tight(|l| l.pdf_max_tail_padding = 50_000);
    assert!(matches!(
        inspect_manual(&write(dir.path(), "p.pdf", &bytes), &limits),
        Err(ManualViewerError::Malformed(_))
    ));
}

#[test]
fn a_long_incremental_update_chain_is_followed_and_a_runaway_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = PdfBuilder::new(1).build();
    let mut prev: usize = {
        let t = String::from_utf8_lossy(&file);
        t.rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let chain = |file: &mut Vec<u8>, prev: &mut usize, updates: usize| {
        for n in 0..updates {
            let at = file.len();
            file.extend_from_slice(
                format!(
                    "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count {} >>\nendobj\n",
                    10 + n
                )
                .as_bytes(),
            );
            let xref_at = file.len();
            file.extend_from_slice(format!("xref\n2 1\n{at:010} 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
            *prev = xref_at;
        }
    };
    chain(&mut file, &mut prev, 120);
    // 120 updates is realistic (real manuals have ~100) and the newest wins.
    assert_eq!(
        inspect(&write(dir.path(), "long.pdf", &file))
            .unwrap()
            .page_count,
        Some(10 + 119)
    );
    chain(&mut file, &mut prev, 200);
    assert!(matches!(
        inspect(&write(dir.path(), "runaway.pdf", &file)),
        Err(ManualViewerError::Malformed(_))
    ));
}

#[test]
fn absurd_page_counts_and_nesting_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        inspect(&write(
            dir.path(),
            "many.pdf",
            &PdfBuilder::new(20_000).build()
        )),
        Err(ManualViewerError::TooManyPages { count: 20_000, .. })
    ));
    let mut deep = PdfBuilder::new(1);
    deep.catalog_extra = format!("/X {}{}", "[".repeat(500), "]".repeat(500));
    assert!(matches!(
        inspect(&write(dir.path(), "deep.pdf", &deep.build())),
        Err(ManualViewerError::Malformed(_))
    ));
    let mut negative = PdfBuilder::new(-4);
    negative.info = None;
    assert!(matches!(
        inspect(&write(dir.path(), "neg.pdf", &negative.build())),
        Err(ManualViewerError::Malformed(_))
    ));
}

#[test]
fn pdf_metadata_strings_are_cut_and_stripped() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = PdfBuilder::new(1);
    b.info = Some(format!("<< /Title ({}) >>", "x".repeat(2000)));
    let title = inspect(&write(dir.path(), "long.pdf", &b.build()))
        .unwrap()
        .metadata
        .title
        .unwrap();
    assert_eq!(
        title.chars().count(),
        ManualLimits::default().max_metadata_chars
    );
}

// ------------------------------------------------------------------ CBZ ----

#[test]
fn a_normal_cbz_lists_pages_in_natural_order_and_reads_them_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(4, 6);
    let path = cbz(
        dir.path(),
        "comic.cbz",
        &[
            ("page10.png", &p),
            ("page2.png", &p),
            ("page1.png", &p),
            ("ComicInfo.xml", b"<ComicInfo/>"),
            ("Thumbs.db", b"junk"),
        ],
    );
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    let inspection = document.inspection();
    assert_eq!(
        page_names(inspection),
        ["page1.png", "page2.png", "page10.png"]
    );
    assert_eq!(inspection.page_count, Some(3));
    assert!(inspection.readiness.can_view());
    assert!(
        inspection
            .warnings
            .contains(&ManualWarning::IgnoredMembers(2))
    );
    let page = document.decode_page(2).unwrap();
    assert_eq!((page.width, page.height), (4, 6));
    assert_eq!(page.rgba.len(), 4 * 6 * 4);
    assert_eq!(document.read_page_bytes(0).unwrap(), p);
    assert_eq!(
        document.read_page_bytes(3),
        Err(ManualViewerError::PageOutOfRange { index: 3, count: 3 })
    );
}

#[test]
fn pages_are_identical_across_runs_and_member_order() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    let a = cbz(
        dir.path(),
        "a.cbz",
        &[
            ("3.png", &p),
            ("1.png", &p),
            ("2.png", &p),
            ("cover.png", &p),
        ],
    );
    let b = cbz(
        dir.path(),
        "b.cbz",
        &[
            ("cover.png", &p),
            ("2.png", &p),
            ("3.png", &p),
            ("1.png", &p),
        ],
    );
    let (ia, ib) = (inspect(&a).unwrap(), inspect(&b).unwrap());
    assert_eq!(page_names(&ia), ["cover.png", "1.png", "2.png", "3.png"]);
    assert_eq!(page_names(&ia), page_names(&ib));
    assert_eq!(inspect(&a).unwrap().pages, ia.pages);
    assert_eq!(ia.pages[0].group, PageGroup::Cover);
}

#[test]
fn traversal_absolute_and_drive_paths_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    for (name, reason) in [
        ("../evil.png", UnsafeMemberReason::Traversal),
        ("a/../../evil.png", UnsafeMemberReason::Traversal),
        ("..\\evil.png", UnsafeMemberReason::Traversal),
        ("/etc/evil.png", UnsafeMemberReason::AbsolutePath),
        ("\\evil.png", UnsafeMemberReason::AbsolutePath),
        ("C:/evil.png", UnsafeMemberReason::DriveOrUnc),
        ("//host/share/evil.png", UnsafeMemberReason::DriveOrUnc),
    ] {
        let path = cbz(dir.path(), "bad.cbz", &[("1.png", &p), (name, &p)]);
        match inspect(&path) {
            Err(ManualViewerError::UnsafeMember { reason: got, .. }) => {
                assert_eq!(got, reason, "{name}")
            }
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn control_characters_and_long_names_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    let path = cbz(dir.path(), "c.cbz", &[("a\u{7}b.png", &p)]);
    assert!(matches!(
        inspect(&path),
        Err(ManualViewerError::UnsafeMember {
            reason: UnsafeMemberReason::ControlCharacter,
            ..
        })
    ));
    let long = format!("{}.png", "x".repeat(600));
    let path = cbz(dir.path(), "l.cbz", &[(long.as_str(), &p)]);
    assert!(matches!(
        inspect(&path),
        Err(ManualViewerError::UnsafeMember {
            reason: UnsafeMemberReason::NameTooLong,
            ..
        })
    ));
}

#[test]
fn symlink_members_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("1.png", options).unwrap();
    writer.write_all(&png(2, 2)).unwrap();
    writer.add_symlink("2.png", "/etc/passwd", options).unwrap();
    let path = write(
        dir.path(),
        "link.cbz",
        &writer.finish().unwrap().into_inner(),
    );
    assert!(matches!(
        inspect(&path),
        Err(ManualViewerError::UnsafeMember {
            reason: UnsafeMemberReason::Symlink,
            ..
        })
    ));
}

#[test]
fn member_count_member_size_and_total_size_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    let five = cbz(
        dir.path(),
        "five.cbz",
        &[
            ("1.png", &p),
            ("2.png", &p),
            ("3.png", &p),
            ("4.png", &p),
            ("5.png", &p),
        ],
    );
    assert!(matches!(
        inspect_manual(&five, &tight(|l| l.max_members = 3)),
        Err(ManualViewerError::TooManyMembers { max: 3, .. })
    ));
    let one = cbz(dir.path(), "one.cbz", &[("1.png", &[0u8; 300])]);
    assert!(matches!(
        inspect_manual(&one, &tight(|l| l.max_member_bytes = 100)),
        Err(ManualViewerError::MemberTooLarge {
            bytes: 300,
            max: 100,
            ..
        })
    ));
    let two = cbz(
        dir.path(),
        "two.cbz",
        &[("1.png", &[0u8; 100]), ("2.png", &[0u8; 100])],
    );
    assert!(matches!(
        inspect_manual(&two, &tight(|l| l.max_total_uncompressed_bytes = 150)),
        Err(ManualViewerError::ArchiveTooLarge { max: 150, .. })
    ));
    // Non-page members count toward the totals too, so a bomb cannot hide in one.
    let hidden = cbz(
        dir.path(),
        "hidden.cbz",
        &[("1.png", &p), ("notes.txt", &[0u8; 300])],
    );
    assert!(matches!(
        inspect_manual(&hidden, &tight(|l| l.max_member_bytes = 100)),
        Err(ManualViewerError::MemberTooLarge { .. })
    ));
}

#[test]
fn a_decompression_bomb_is_refused_before_anything_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let zeros = vec![0u8; 8 * 1024 * 1024];
    let bytes = zip_with(
        &[("1.png", &png(2, 2)), ("big.png", &zeros)],
        CompressionMethod::Deflated,
    );
    assert!(
        bytes.len() < 100_000,
        "fixture should be tiny on disk: {}",
        bytes.len()
    );
    let path = write(dir.path(), "bomb.cbz", &bytes);
    assert!(matches!(
        inspect(&path),
        Err(ManualViewerError::SuspiciousCompression { .. })
    ));
}

#[test]
fn malformed_zips_are_refused_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let garbage = [b"PK\x03\x04".as_slice(), &[0xAB; 200]].concat();
    assert!(matches!(
        inspect(&write(dir.path(), "g.cbz", &garbage)),
        Err(ManualViewerError::Malformed(_))
    ));
    let good = zip_of(&[("1.png", &png(2, 2))]);
    assert!(matches!(
        inspect(&write(dir.path(), "t.cbz", &good[..good.len() - 10])),
        Err(ManualViewerError::Malformed(_))
    ));
}

#[test]
fn duplicate_members_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    let bytes = zip_of(&[("aaa.png", &p), ("bbb.png", &p)]);
    let needle = b"bbb.png";
    let mut patched = bytes.clone();
    let mut start = 0;
    while let Some(at) = patched[start..]
        .windows(needle.len())
        .position(|w| w == needle)
    {
        patched[start + at..start + at + needle.len()].copy_from_slice(b"aaa.png");
        start += at + needle.len();
    }
    let result = inspect(&write(dir.path(), "dup.cbz", &patched));
    assert!(
        matches!(result, Err(ManualViewerError::DuplicateMember { .. })),
        "{result:?}"
    );
}

#[test]
fn nested_archives_and_encrypted_members_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    let path = cbz(
        dir.path(),
        "n.cbz",
        &[("1.png", &p), ("extras/inner.zip", b"PK\x05\x06")],
    );
    assert!(matches!(
        inspect(&path),
        Err(ManualViewerError::NestedArchive { .. })
    ));

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_aes_encryption(zip::AesMode::Aes256, "secret");
    writer.start_file("1.png", options).unwrap();
    writer.write_all(&p).unwrap();
    let path = write(dir.path(), "e.cbz", &writer.finish().unwrap().into_inner());
    assert!(matches!(
        inspect(&path),
        Err(ManualViewerError::EncryptedMember { .. })
    ));
}

#[test]
fn unsupported_images_and_empty_archives_say_what_is_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    // Only a GIF: recognised as an image but not decodable here.
    let only_gif = cbz(dir.path(), "g.cbz", &[("1.gif", b"GIF89a")]);
    assert!(matches!(
        inspect(&only_gif),
        Err(ManualViewerError::UnsupportedImage { .. })
    ));
    // No images at all.
    let none = cbz(dir.path(), "n.cbz", &[("readme.txt", b"hi")]);
    assert_eq!(inspect(&none), Err(ManualViewerError::NoSupportedPages));
    let empty = write(dir.path(), "e.cbz", &zip_of(&[]));
    assert_eq!(inspect(&empty), Err(ManualViewerError::NoSupportedPages));
    // A GIF alongside real pages is skipped with a warning, not fatal.
    let mixed = cbz(dir.path(), "m.cbz", &[("1.png", &p), ("2.gif", b"GIF89a")]);
    let inspection = inspect(&mixed).unwrap();
    assert_eq!(page_names(&inspection), ["1.png"]);
    assert!(
        inspection
            .warnings
            .contains(&ManualWarning::UnsupportedImageMembers(1))
    );
    // macOS resource forks carry image extensions but are not pages.
    let mac = cbz(
        dir.path(),
        "x.cbz",
        &[
            ("1.png", &p),
            ("__MACOSX/._1.png", b"junk"),
            (".hidden.png", b"junk"),
        ],
    );
    let inspection = inspect(&mac).unwrap();
    assert_eq!(page_names(&inspection), ["1.png"]);
    assert!(
        inspection
            .warnings
            .contains(&ManualWarning::IgnoredMembers(2))
    );
}

#[test]
fn huge_dimensions_are_refused_before_decoding() {
    let dir = tempfile::tempdir().unwrap();
    // 20_000 x 1 is a few bytes on disk but over the default dimension limit.
    let wide = png(20_000, 1);
    let path = cbz(dir.path(), "wide.cbz", &[("1.png", &wide)]);
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    assert_eq!(
        document.decode_page(0),
        Err(ManualViewerError::ImageTooLarge {
            width: 20_000,
            height: 1
        })
    );
    // The pixel budget is separate from the per-side limit.
    let path = cbz(dir.path(), "px.cbz", &[("1.png", &png(100, 100))]);
    let limits = tight(|l| l.max_image_pixels = 1000);
    let document = ManualDocument::open(&path, &limits).unwrap();
    assert!(matches!(
        document.decode_page(0),
        Err(ManualViewerError::ImageTooLarge { .. })
    ));
    // And so is the decoded-bytes budget.
    let limits = tight(|l| l.max_decoded_bytes = 1000);
    let document = ManualDocument::open(&path, &limits).unwrap();
    assert!(matches!(
        document.decode_page(0),
        Err(ManualViewerError::ImageTooLarge { .. })
    ));
}

#[test]
fn bad_images_fail_that_page_and_leave_the_document_usable() {
    let dir = tempfile::tempdir().unwrap();
    let good = png(8, 8);
    let truncated = good[..good.len() / 2].to_vec();
    let path = cbz(
        dir.path(),
        "bad.cbz",
        &[
            ("1.png", &good),
            ("2.png", &truncated),
            ("3.png", b"not an image at all"),
        ],
    );
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    assert!(document.decode_page(0).is_ok());
    assert!(matches!(
        document.decode_page(1),
        Err(ManualViewerError::MalformedImage(_))
    ));
    assert!(matches!(
        document.decode_page(2),
        Err(ManualViewerError::UnsupportedImage { .. })
    ));
    assert!(
        document.decode_page(0).is_ok(),
        "an earlier failure must not poison the document"
    );
}

#[test]
fn a_page_whose_extension_lies_about_its_content_is_not_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let gif_named_png: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    let path = cbz(dir.path(), "lie.cbz", &[("1.png", gif_named_png)]);
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    assert!(matches!(
        document.decode_page(0),
        Err(ManualViewerError::UnsupportedImage { .. })
    ));
}

#[test]
fn a_file_that_changes_after_opening_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    let path = cbz(dir.path(), "c.cbz", &[("1.png", &p)]);
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    assert!(document.read_page_bytes(0).is_ok());
    fs::write(&path, zip_of(&[("1.png", &p), ("2.png", &p)])).unwrap();
    assert_eq!(
        document.read_page_bytes(0),
        Err(ManualViewerError::SourceChanged)
    );
}

#[test]
fn inspection_never_modifies_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(3, 3);
    let cbz_path = cbz(dir.path(), "a.cbz", &[("1.png", &p)]);
    let pdf_path = write(dir.path(), "b.pdf", &PdfBuilder::new(2).build());
    let before: Vec<_> = [&cbz_path, &pdf_path]
        .iter()
        .map(|p| {
            (
                fs::read(p).unwrap(),
                fs::metadata(p).unwrap().modified().unwrap(),
            )
        })
        .collect();
    for path in [&cbz_path, &pdf_path] {
        let document = ManualDocument::open(path, &ManualLimits::default()).unwrap();
        let _ = document.decode_page(0);
    }
    let after: Vec<_> = [&cbz_path, &pdf_path]
        .iter()
        .map(|p| {
            (
                fs::read(p).unwrap(),
                fs::metadata(p).unwrap().modified().unwrap(),
            )
        })
        .collect();
    assert_eq!(before, after);
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "nothing may be written beside the document: {names:?}"
    );
}

// ------------------------------------------------------------------ CBR ----

#[test]
fn cbr_is_recognised_by_signature_and_reported_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    for (name, magic, sig) in [
        (
            "five.cbr",
            b"Rar!\x1A\x07\x01\x00junk".as_slice(),
            ManualSignature::Rar5,
        ),
        (
            "four.cbr",
            b"Rar!\x1A\x07\x00junk".as_slice(),
            ManualSignature::Rar4,
        ),
        // A RAR named .cbz is still a RAR, and says so.
        (
            "lies.cbz",
            b"Rar!\x1A\x07\x01\x00junk".as_slice(),
            ManualSignature::Rar5,
        ),
    ] {
        let inspection = inspect(&write(dir.path(), name, magic)).unwrap();
        assert_eq!(inspection.kind, ManualDocumentKind::Cbr, "{name}");
        assert_eq!(inspection.evidence.signature, sig);
        assert_eq!(
            inspection.readiness,
            ManualReadiness::Unsupported {
                missing: ManualCapabilityGap::RarReader
            }
        );
        assert_eq!(inspection.page_count, None);
        assert!(inspection.pages.is_empty());
    }
    let lies = inspect(&write(dir.path(), "l.cbz", b"Rar!\x1A\x07\x00x")).unwrap();
    assert!(
        lies.warnings
            .iter()
            .any(|w| matches!(w, ManualWarning::ExtensionMismatch { .. }))
    );
}

#[test]
fn cbr_pages_cannot_be_read_and_the_gap_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "c.cbr", b"Rar!\x1A\x07\x00data");
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    assert_eq!(
        document.read_page_bytes(0),
        Err(ManualViewerError::CapabilityUnavailable(
            ManualCapabilityGap::RarReader
        ))
    );
    assert!(ManualCapabilityGap::RarReader.detail().contains("RAR"));
    assert!(
        !ManualReadiness::Unsupported {
            missing: ManualCapabilityGap::RarReader
        }
        .can_view()
    );
}

// ------------------------------------------------------------- ordering ----

fn sorted(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    v.sort_by(|a, b| compare_page_names(a, b));
    v
}

#[test]
fn numbers_sort_by_value_not_text() {
    assert_eq!(
        sorted(&["10.png", "2.png", "1.png", "3.png"]),
        ["1.png", "2.png", "3.png", "10.png"]
    );
    assert_eq!(
        sorted(&["page10.jpg", "page2.jpg", "page02b.jpg", "page1.jpg"]),
        ["page1.jpg", "page2.jpg", "page02b.jpg", "page10.jpg"]
    );
    assert_eq!(
        sorted(&["003.png", "001.png", "010.png", "002.png"]),
        ["001.png", "002.png", "003.png", "010.png"]
    );
}

#[test]
fn leading_zeros_and_huge_numbers_are_ordered_without_overflow() {
    let big = "page99999999999999999999999.png";
    let small = "page00000000000000000001.png";
    assert_eq!(
        sorted(&[big, small, "page5.png"]),
        [small, "page5.png", big]
    );
    // Equal values with different padding still have a total, stable order.
    let a = sorted(&["p01.png", "p1.png"]);
    let b = sorted(&["p1.png", "p01.png"]);
    assert_eq!(a, b);
}

#[test]
fn cover_and_back_matter_have_fixed_places() {
    assert_eq!(
        sorted(&["002.png", "back.jpg", "001.png", "Cover.JPG"]),
        ["Cover.JPG", "001.png", "002.png", "back.jpg"]
    );
    assert_eq!(page_group("front_cover.png"), PageGroup::Cover);
    assert_eq!(page_group("Front-Cover.png"), PageGroup::Cover);
    assert_eq!(page_group("rear cover.png"), PageGroup::BackCover);
    // Only exact names count; "cover_art_2" is an ordinary page.
    assert_eq!(page_group("cover_art_2.png"), PageGroup::Body);
    assert_eq!(page_group("discover.png"), PageGroup::Body);
    // A cover inside a sub-folder is an ordinary page.
    assert_eq!(page_group("ch1/cover.jpg"), PageGroup::Body);
    assert_eq!(
        sorted(&["ch1/cover.jpg", "cover.jpg", "ch1/002.png"]),
        ["cover.jpg", "ch1/002.png", "ch1/cover.jpg"]
    );
}

#[test]
fn folders_sort_naturally_before_their_files() {
    assert_eq!(
        sorted(&[
            "ch10/1.png",
            "ch2/2.png",
            "ch2/10.png",
            "ch2/1.png",
            "ch1/1.png"
        ]),
        [
            "ch1/1.png",
            "ch2/1.png",
            "ch2/2.png",
            "ch2/10.png",
            "ch10/1.png"
        ]
    );
    assert_eq!(
        natural_path_cmp_for_tests("a\\1.png", "a/1.png"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn ordering_is_independent_of_input_permutation() {
    let names = [
        "b10.png",
        "b2.png",
        "A1.png",
        "a1.png",
        "cover.png",
        "back.png",
        "c/1.png",
        "c/01.png",
    ];
    let expected = sorted(&names);
    // Every rotation and the reverse produce the same order.
    for shift in 0..names.len() {
        let mut rotated: Vec<&str> = names.to_vec();
        rotated.rotate_left(shift);
        assert_eq!(sorted(&rotated), expected);
    }
    let mut reversed: Vec<&str> = names.to_vec();
    reversed.reverse();
    assert_eq!(sorted(&reversed), expected);
}

// ---------------------------------------------------------- viewer state ----

fn id(path: &str, len: u64) -> ManualDocumentId {
    ManualDocumentId {
        path: PathBuf::from(path),
        len,
        modified_nanos: Some(1),
        device_inode: Some((1, 1)),
    }
}

fn opened(pages: usize) -> ManualViewerState {
    let mut state = ManualViewerState::closed();
    state.open(id("/m/a.cbz", 10), pages);
    state
}

#[test]
fn page_movement_stops_at_both_ends() {
    let mut state = opened(3);
    assert!(!state.apply(ManualViewerAction::PreviousPage));
    assert_eq!((state.current_page(), state.page_number()), (0, 1));
    assert!(state.apply(ManualViewerAction::NextPage));
    assert!(state.apply(ManualViewerAction::NextPage));
    assert!(
        !state.apply(ManualViewerAction::NextPage),
        "must not run past the last page"
    );
    assert_eq!(state.page_number(), 3);
    assert!(!state.can_go_next() && state.can_go_previous());
    assert!(state.apply(ManualViewerAction::FirstPage));
    assert!(!state.apply(ManualViewerAction::FirstPage));
    assert!(state.apply(ManualViewerAction::LastPage));
    assert!(!state.apply(ManualViewerAction::LastPage));
    assert_eq!(state.current_page(), 2);
}

#[test]
fn a_single_page_and_an_empty_document_never_move() {
    let mut one = opened(1);
    for action in [
        ManualViewerAction::NextPage,
        ManualViewerAction::PreviousPage,
        ManualViewerAction::LastPage,
        ManualViewerAction::FirstPage,
    ] {
        assert!(!one.apply(action));
    }
    let mut empty = opened(0);
    assert_eq!(empty.page_number(), 0);
    for action in [
        ManualViewerAction::NextPage,
        ManualViewerAction::LastPage,
        ManualViewerAction::PreviousPage,
    ] {
        assert!(!empty.apply(action));
    }
    assert_eq!(empty.current_page(), 0);
}

#[test]
fn changing_document_resets_page_and_zoom_but_keeps_fullscreen() {
    let mut state = opened(10);
    state.apply(ManualViewerAction::LastPage);
    state.apply(ManualViewerAction::ZoomIn);
    state.apply(ManualViewerAction::ToggleFullscreen);
    state.open(id("/m/other.cbz", 99), 4);
    assert_eq!((state.current_page(), state.page_count()), (0, 4));
    assert_eq!(state.zoom(), ManualZoom::FitPage);
    assert!(state.is_fullscreen());
    assert_eq!(
        state.document().unwrap().path,
        PathBuf::from("/m/other.cbz")
    );
}

#[test]
fn a_changed_file_at_the_same_path_counts_as_a_different_document() {
    let mut state = opened(10);
    state.apply(ManualViewerAction::LastPage);
    state.open(id("/m/a.cbz", 11), 10); // same path, different size
    assert_eq!(state.current_page(), 0);
}

#[test]
fn reopening_the_same_document_keeps_position_and_clamps_it() {
    let mut state = opened(10);
    state.apply(ManualViewerAction::LastPage);
    state.open(id("/m/a.cbz", 10), 10);
    assert_eq!(state.current_page(), 9);
    state.open(id("/m/a.cbz", 10), 4); // fewer pages than before
    assert_eq!(state.current_page(), 3);
}

#[test]
fn zoom_steps_are_bounded_and_fit_modes_are_explicit() {
    let mut state = opened(5);
    assert_eq!(state.zoom(), ManualZoom::FitPage);
    assert!(state.apply(ManualViewerAction::ZoomIn));
    assert_eq!(state.zoom(), ManualZoom::Percent(125));
    for _ in 0..20 {
        state.apply(ManualViewerAction::ZoomIn);
    }
    assert_eq!(state.zoom(), ManualZoom::Percent(400));
    assert!(!state.apply(ManualViewerAction::ZoomIn));
    for _ in 0..20 {
        state.apply(ManualViewerAction::ZoomOut);
    }
    assert_eq!(state.zoom(), ManualZoom::Percent(25));
    assert!(!state.apply(ManualViewerAction::ZoomOut));
    assert!(state.apply(ManualViewerAction::FitWidth));
    assert_eq!(state.zoom(), ManualZoom::FitWidth);
    assert!(!state.apply(ManualViewerAction::FitWidth));
    assert!(state.apply(ManualViewerAction::ZoomOut));
    assert_eq!(
        state.zoom(),
        ManualZoom::Percent(75),
        "fit modes step from 100%"
    );
    assert!(state.apply(ManualViewerAction::FitPage));
    assert_eq!(state.zoom(), ManualZoom::FitPage);
}

#[test]
fn zoom_survives_page_turns() {
    let mut state = opened(5);
    state.apply(ManualViewerAction::FitWidth);
    state.apply(ManualViewerAction::NextPage);
    assert_eq!(state.zoom(), ManualZoom::FitWidth);
}

#[test]
fn fullscreen_toggles_and_close_resets_everything() {
    let mut state = opened(5);
    assert!(state.apply(ManualViewerAction::ToggleFullscreen));
    assert!(state.is_fullscreen());
    assert!(state.apply(ManualViewerAction::ToggleFullscreen));
    assert!(!state.is_fullscreen());
    state.apply(ManualViewerAction::ToggleFullscreen);
    state.apply(ManualViewerAction::NextPage);
    assert!(state.apply(ManualViewerAction::Close));
    assert_eq!(state, ManualViewerState::closed());
    assert!(!state.is_open());
    assert!(
        !state.apply(ManualViewerAction::Close),
        "closing twice changes nothing"
    );
}

#[test]
fn a_closed_viewer_ignores_every_action() {
    let mut state = ManualViewerState::closed();
    for action in [
        ManualViewerAction::NextPage,
        ManualViewerAction::PreviousPage,
        ManualViewerAction::FirstPage,
        ManualViewerAction::LastPage,
        ManualViewerAction::ZoomIn,
        ManualViewerAction::ZoomOut,
        ManualViewerAction::FitWidth,
        ManualViewerAction::FitPage,
        ManualViewerAction::ToggleFullscreen,
        ManualViewerAction::Close,
    ] {
        assert!(!state.apply(action), "{action:?}");
    }
    assert_eq!(state, ManualViewerState::closed());
}

#[test]
fn go_to_page_ignores_out_of_range_requests() {
    let mut state = opened(5);
    assert!(state.go_to_page(3));
    assert!(!state.go_to_page(3));
    assert!(!state.go_to_page(5));
    assert!(!state.go_to_page(usize::MAX));
    assert_eq!(state.current_page(), 3);
    assert!(!ManualViewerState::closed().go_to_page(0));
}

#[test]
fn the_state_is_driven_end_to_end_from_a_real_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let p = png(2, 2);
    let path = cbz(
        dir.path(),
        "c.cbz",
        &[("2.png", &p), ("1.png", &p), ("3.png", &p)],
    );
    let document = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    let mut state = ManualViewerState::closed();
    state.open(
        document.id().clone(),
        document.inspection().page_count.unwrap(),
    );
    state.apply(ManualViewerAction::LastPage);
    assert_eq!(state.page_number(), 3);
    assert_eq!(
        document.inspection().pages[state.current_page()].name,
        "3.png"
    );
    assert!(document.decode_page(state.current_page()).is_ok());
}

// Physical ZIP records, deliberately bypassing ZipWriter's duplicate-name guard.
fn physical_zip(entries: &[(String, u32, u16)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, logical, flags) in entries {
        let offset = out.len() as u32;
        let mut local = vec![0u8; 30];
        local[..4].copy_from_slice(b"PK\x03\x04");
        local[4..6].copy_from_slice(&20u16.to_le_bytes());
        local[6..8].copy_from_slice(&flags.to_le_bytes());
        local[18..22].copy_from_slice(&1u32.to_le_bytes());
        local[22..26].copy_from_slice(&logical.to_le_bytes());
        local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend(local);
        out.extend(name.as_bytes());
        out.push(0);
        let mut header = vec![0u8; 46];
        header[..4].copy_from_slice(b"PK\x01\x02");
        header[4..6].copy_from_slice(&20u16.to_le_bytes());
        header[6..8].copy_from_slice(&20u16.to_le_bytes());
        header[8..10].copy_from_slice(&flags.to_le_bytes());
        header[20..24].copy_from_slice(&1u32.to_le_bytes());
        header[24..28].copy_from_slice(&logical.to_le_bytes());
        header[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        header[42..46].copy_from_slice(&offset.to_le_bytes());
        central.extend(header);
        central.extend(name.as_bytes());
    }
    let offset = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    let mut end = vec![0u8; 22];
    end[..4].copy_from_slice(b"PK\x05\x06");
    end[8..10].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    end[10..12].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    end[12..16].copy_from_slice(&size.to_le_bytes());
    end[16..20].copy_from_slice(&offset.to_le_bytes());
    out.extend(end);
    out
}

#[test]
fn physical_member_ceiling_includes_duplicates_directories_and_trailing_junk() {
    let dir = tempfile::tempdir().unwrap();
    // Valid image pages at the exact ceiling remain inspectable.
    let names: Vec<_> = (0..10_000).map(|i| format!("{i}.png")).collect();
    let image = png(1, 1);
    let entries: Vec<_> = names
        .iter()
        .map(|n| (n.as_str(), image.as_slice()))
        .collect();
    let valid = zip_of(&entries);
    assert_eq!(
        inspect(&write(dir.path(), "valid.cbz", &valid))
            .unwrap()
            .pages
            .len(),
        10_000
    );
    for duplicate in [false, true] {
        for directories in [false, true] {
            let entries: Vec<_> = (0..10_001)
                .map(|i| {
                    (
                        if duplicate {
                            "1.png".into()
                        } else {
                            format!("{i}{}", if directories { "/" } else { ".png" })
                        },
                        1,
                        0,
                    )
                })
                .collect();
            let bytes = physical_zip(&entries);
            for trailing in [false, true] {
                let mut bytes = bytes.clone();
                if trailing {
                    bytes.extend(b"trailing bytes");
                }
                let result = inspect(&write(dir.path(), "count.cbz", &bytes));
                if trailing {
                    assert!(result.is_err());
                } else {
                    assert!(
                        matches!(
                            result,
                            Err(ManualViewerError::TooManyMembers { max: 10_000, .. })
                        ),
                        "{result:?}"
                    );
                }
            }
        }
    }
    // A lying EOCD count cannot hide physical records beyond its declared count.
    let mut bytes = physical_zip(&[("a.png".into(), 1, 0), ("b.png".into(), 1, 0)]);
    let end = bytes.len() - 22;
    bytes[end + 8..end + 12].copy_from_slice(&[1, 0, 1, 0]);
    assert!(inspect(&write(dir.path(), "hidden.cbz", &bytes)).is_err());
}

#[test]
fn physical_duplicate_names_are_checked_before_indexing_or_page_filtering() {
    let dir = tempfile::tempdir().unwrap();
    for (a, b) in [
        ("1.png", "1.png"),
        ("a/1.png", "a\\1.png"),
        ("a/./1.png", "a//1.png"),
        ("a/", "a/"),
        ("notes.txt", "./notes.txt"),
    ] {
        let bytes = physical_zip(&[(a.into(), 1, 0), (b.into(), 1, 0)]);
        assert!(
            matches!(
                inspect(&write(dir.path(), "dup.cbz", &bytes)),
                Err(ManualViewerError::DuplicateMember { .. })
            ),
            "{a} {b}"
        );
        let mut trailing = bytes;
        trailing.extend(b"junk");
        assert!(inspect(&write(dir.path(), "trailing.cbz", &trailing)).is_err());
    }
}

#[test]
fn directories_receive_all_generic_safety_checks_without_decode() {
    let dir = tempfile::tempdir().unwrap();
    for (size, flags, limits) in [
        (u32::MAX - 1, 0, ManualLimits::default()),
        (0, 1, ManualLimits::default()),
        (200, 0, tight(|l| l.max_total_uncompressed_bytes = 200)),
        (201, 0, ManualLimits::default()),
    ] {
        let bytes = physical_zip(&[("folder/".into(), size, flags), ("1.png".into(), 1, 0)]);
        assert!(inspect_manual(&write(dir.path(), "dir.cbz", &bytes), &limits).is_err());
    }
    let bytes = physical_zip(&[("folder/".into(), 100, 0), ("1.png".into(), 1, 0)]);
    assert_eq!(
        inspect(&write(dir.path(), "dir.cbz", &bytes))
            .unwrap()
            .pages
            .len(),
        1
    );
}

#[test]
fn ratio_is_absolute_exact_and_overflow_safe() {
    use super::zip_pages::within_expansion_ratio as allowed;
    assert!(allowed(199_999, 1_000, 200));
    assert!(allowed(200_000, 1_000, 200));
    assert!(!allowed(200_001, 1_000, 200));
    assert!(!allowed(200_838, 1_000, 200));
    assert!(!allowed(877, 1, 200));
    assert!(allowed(0, 0, 200));
    assert!(!allowed(1, 0, 200));
    assert!(allowed(u64::MAX, u64::MAX, 200));
    assert!(!allowed(u64::MAX, 1, 200));
    let dir = tempfile::tempdir().unwrap();
    for size in [100_000, 2_000_000] {
        let bytes = zip_with(&[("1.png", &vec![0; size])], CompressionMethod::Deflated);
        assert!(matches!(
            inspect(&write(dir.path(), "ratio.cbz", &bytes)),
            Err(ManualViewerError::SuspiciousCompression { .. })
        ));
    }
}

#[test]
fn pdf_prev_and_hybrid_offsets_distinguish_absence_from_damage() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        inspect(&write(
            dir.path(),
            "absent.pdf",
            &PdfBuilder::new(1).build()
        ))
        .is_ok()
    );
    for key in ["Prev", "XRefStm"] {
        for value in [
            "-1",
            "999999999",
            "1.5",
            "null",
            "/bad",
            "(123)",
            "18446744073709551616",
            "[1]",
            "",
        ] {
            let mut builder = PdfBuilder::new(1);
            builder.trailer_extra = format!("/{key} {value}");
            assert!(
                matches!(
                    inspect(&write(dir.path(), "bad.pdf", &builder.build())),
                    Err(ManualViewerError::Malformed(_))
                ),
                "{key} {value}"
            );
        }
    }
}

#[test]
fn pdf_live_xref_offsets_fail_closed_even_for_unreferenced_objects() {
    let dir = tempfile::tempdir().unwrap();
    // Entry 3 is not needed for the declared page count, but must still validate.
    let text = String::from_utf8(PdfBuilder::new(1).build()).unwrap();
    let at = text.find("xref\n").unwrap();
    let entries: Vec<_> = text[at..].lines().collect();
    let entry = entries[5];
    for offset in [
        "9999999999",
        "-000000001",
        "000000x123",
        "18446744073709551616",
        "0000000000",
    ] {
        let bad = text.replace(entry, &format!("{offset} 00000 n "));
        assert!(
            matches!(
                inspect(&write(dir.path(), "bad.pdf", bad.as_bytes())),
                Err(ManualViewerError::Malformed(_))
            ),
            "{offset}"
        );
    }
    let free = text.replace(entry, "0000000000 65535 f ");
    assert!(inspect(&write(dir.path(), "free.pdf", free.as_bytes())).is_ok());
}

#[test]
fn javascript_and_openaction_flags_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    for (extra, info, js, open) in [
        (
            "/OpenAction << /S /JavaScript /JS (never execute) >>",
            None,
            true,
            true,
        ),
        (
            "/OpenAction 4 0 R",
            Some("<< /S /JavaScript /JS (never execute) >>"),
            true,
            true,
        ),
        (
            "/OpenAction << /S /GoTo /D [3 0 R /Fit] >>",
            None,
            false,
            true,
        ),
        (
            "/Names << /JavaScript << /Names [] >> >>",
            None,
            true,
            false,
        ),
    ] {
        let mut builder = PdfBuilder::new(1);
        builder.catalog_extra = extra.into();
        builder.info = info.map(str::to_string);
        let result = inspect(&write(dir.path(), "actions.pdf", &builder.build())).unwrap();
        assert_eq!(
            result
                .active_content
                .contains(&ManualActiveContent::JavaScript),
            js
        );
        assert_eq!(
            result
                .active_content
                .contains(&ManualActiveContent::OpenAction),
            open
        );
    }
}

#[test]
fn independent_pdf_mutations_never_panic() {
    let mut builder = PdfBuilder::new(1);
    builder.trailer_extra = "/Prev 9999999".into();
    let original = builder.build();
    let dir = tempfile::tempdir().unwrap();
    let mut seed = 0x739A5678_u64;
    for _ in 0..4000 {
        let mut data = original.clone();
        for _ in 0..4 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let i = (seed as usize) % data.len();
            data[i] = (seed >> 32) as u8;
        }
        let path = write(dir.path(), "mutated.pdf", &data);
        assert!(std::panic::catch_unwind(|| inspect(&path)).is_ok());
    }
}

#[test]
fn xref_stream_live_offsets_do_not_turn_into_free_entries() {
    let dir = tempfile::tempdir().unwrap();
    for (kind, offset, valid) in [
        (0, u64::MAX, true),
        (1, u64::MAX, false),
        (1, 999999, false),
        (1, 0, false),
    ] {
        let mut body = b"%PDF-1.5\n".to_vec();
        let root = body.len() as u64;
        body.extend(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let pages = body.len() as u64;
        body.extend(b"2 0 obj\n<< /Type /Pages /Count 1 /Kids [] >>\nendobj\n");
        let xref = body.len();
        let mut data = Vec::new();
        for (kind, offset) in [(1, root), (1, pages), (kind, offset)] {
            data.push(kind);
            data.extend(offset.to_be_bytes());
            data.push(0);
        }
        body.extend(format!("4 0 obj\n<< /Type /XRef /Size 5 /Root 1 0 R /W [1 8 1] /Index [1 3] /Length {} >>\nstream\n", data.len()).bytes());
        body.extend(data);
        body.extend(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").bytes());
        assert_eq!(
            inspect(&write(dir.path(), "stream.pdf", &body)).is_ok(),
            valid,
            "kind={kind}, offset={offset}"
        );
    }
}

#[test]
fn lying_zip_size_is_refused_before_decode_or_at_bounded_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = zip_with(&[("1.png", &vec![0; 100_000])], CompressionMethod::Deflated);
    let at = bytes.windows(4).position(|x| x == b"PK\x01\x02").unwrap();
    bytes[at + 24..at + 28].copy_from_slice(&10u32.to_le_bytes());
    let path = write(dir.path(), "lie.cbz", &bytes);
    match ManualDocument::open(&path, &ManualLimits::default()) {
        Err(_) => {}
        Ok(doc) => assert!(doc.read_page_bytes(0).is_err()),
    }
}
