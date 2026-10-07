use super::*;
use crate::gui_v2::documents::{
    GameDocumentAssociation, GameDocumentFormat, GameDocumentKind, GameDocumentSource,
};
use archivefs_core::manual_document::ManualCapabilityGap;
use std::{
    fs,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

pub(in crate::gui_v2) fn cbz(path: &Path) {
    cbz_with_size(path, 8, 12);
}

fn cbz_with_size(path: &Path, width: u32, height: u32) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, color) in [("page10.png", 10), ("page2.png", 2), ("page1.png", 1)] {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([color, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes.get_ref()).unwrap();
    }
    writer.finish().unwrap();
}

fn fixture(
    resume: Option<DocumentReadingState>,
) -> (tempfile::TempDir, egui::Context, ManualViewer) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    cbz(&path);
    let context = egui::Context::default();
    let mut viewer = ManualViewer::default();
    let resume = resume.map(|mut saved| {
        saved.document_id = Some(
            ManualDocument::open(&path, &ManualLimits::default())
                .unwrap()
                .id()
                .clone(),
        );
        saved
    });
    viewer.open(&context, path, "Test manual".into(), resume);
    wait(&mut viewer, &context);
    (root, context, viewer)
}

fn wait(viewer: &mut ManualViewer, context: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        viewer.poll(context);
        let session = viewer.session.as_ref().unwrap();
        if session.texture.is_some() || session.error.is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "reader timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn document(readiness: ManualReadiness, viewer: DocumentOpenCapability) -> GameDocument {
    GameDocument {
        document_id: Some(ManualDocumentId {
            path: "manual.cbz".into(),
            len: 1,
            modified_nanos: None,
            device_inode: None,
        }),
        path: "manual.cbz".into(),
        format: GameDocumentFormat::Cbz,
        kind: GameDocumentKind::Manual,
        title: "Manual".into(),
        platform: None,
        game_id: Some(1),
        source: GameDocumentSource::NearbyGameDirectory,
        association: GameDocumentAssociation::Explicit,
        association_reason: "fixture".into(),
        page_count: Some(3),
        file_size: 1,
        viewer,
        readiness: Some(readiness),
        refusal_reason: None,
    }
}

fn enabled_buttons(readiness: ManualReadiness, external: DocumentOpenCapability) -> (bool, bool) {
    let context = egui::Context::default();
    let mut enabled = (false, false);
    let _ = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let (internal, external) = open_buttons(ui, &document(readiness, external));
            enabled = (internal.enabled(), external.enabled());
        });
    });
    enabled
}

#[test]
fn cbz_read_action_is_independent_of_external_handler() {
    assert_eq!(
        enabled_buttons(ManualReadiness::Viewable, DocumentOpenCapability::NoHandler),
        (true, false)
    );
    assert_eq!(
        enabled_buttons(ManualReadiness::Viewable, DocumentOpenCapability::Supported),
        (true, true)
    );
}

#[test]
fn pdf_renderer_capability_and_external_open_are_independent() {
    let readiness = ManualReadiness::InspectOnly {
        missing: ManualCapabilityGap::PdfRenderer,
    };
    assert_eq!(
        enabled_buttons(readiness, DocumentOpenCapability::Supported),
        (pdf_render::available(), true)
    );
    assert!(capability_text(Some(readiness)).contains("externally"));
}

#[test]
fn cbr_gap_does_not_enable_internal_reading() {
    let readiness = ManualReadiness::Unsupported {
        missing: ManualCapabilityGap::RarReader,
    };
    assert_eq!(
        enabled_buttons(readiness, DocumentOpenCapability::UnsupportedFormat),
        (false, false)
    );
    // The two capabilities stay independent if the external policy gains a handler later.
    assert_eq!(
        enabled_buttons(readiness, DocumentOpenCapability::Supported),
        (false, true)
    );
}

#[test]
fn open_initializes_core_state_count_and_natural_page_order() {
    let (_root, _ctx, viewer) = fixture(None);
    let session = viewer.session.as_ref().unwrap();
    let document = session.document.as_ref().unwrap();
    let mut canonical = ManualViewerState::closed();
    canonical.open(document.id().clone(), 3);
    assert_eq!(session.state, canonical);
    assert_eq!(
        document
            .inspection()
            .pages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["page1.png", "page2.png", "page10.png"]
    );
    assert_eq!(session.texture.as_ref().unwrap().size(), [8, 12]);
}

#[test]
fn resume_restores_existing_one_based_page_and_supported_zoom() {
    let (_root, _ctx, viewer) = fixture(Some(DocumentReadingState {
        last_page: Some(2),
        zoom_percent: Some(150),
        ..Default::default()
    }));
    assert_eq!(viewer.session.as_ref().unwrap().state.current_page(), 1);
    assert_eq!(
        viewer.session.as_ref().unwrap().state.zoom(),
        ManualZoom::Percent(150)
    );
    assert_eq!(viewer.reading().unwrap().1.last_page, Some(2));
}

#[test]
fn zero_resume_is_ignored_and_same_document_out_of_range_resume_is_clamped() {
    for page in [0, 99, usize::MAX] {
        let (_root, _ctx, viewer) = fixture(Some(DocumentReadingState {
            last_page: Some(page),
            zoom_percent: Some(133),
            ..Default::default()
        }));
        assert_eq!(
            viewer.session.as_ref().unwrap().state.page_number(),
            if page == 0 { 1 } else { 3 }
        );
        assert_eq!(
            viewer.session.as_ref().unwrap().state.zoom(),
            ManualZoom::FitPage
        );
    }
}

#[test]
fn all_canonical_zoom_steps_can_be_restored_including_100_percent() {
    let (_root, _ctx, mut viewer) = fixture(None);
    let state = &mut viewer.session.as_mut().unwrap().state;
    for percent in [25, 50, 75, 100, 125, 150, 200, 300, 400] {
        restore_zoom(state, Some(percent));
        assert_eq!(state.zoom(), ManualZoom::Percent(percent));
    }
}

#[test]
fn first_last_previous_next_use_canonical_bounds() {
    let (_root, ctx, mut viewer) = fixture(None);
    let mut expected = viewer.session.as_ref().unwrap().state.clone();
    for action in [
        Action::PreviousPage,
        Action::LastPage,
        Action::NextPage,
        Action::FirstPage,
        Action::NextPage,
    ] {
        expected.apply(action);
        viewer.action(action);
        wait(&mut viewer, &ctx);
        assert_eq!(viewer.session.as_ref().unwrap().state, expected);
    }
}

#[test]
fn zoom_and_fit_use_core_without_another_decode_or_texture_upload() {
    let (_root, _ctx, mut viewer) = fixture(None);
    let session = viewer.session.as_ref().unwrap();
    let generation = session.generation;
    let texture = session.texture.as_ref().unwrap().id();
    let mut expected = session.state.clone();
    for action in [
        Action::ZoomIn,
        Action::ZoomOut,
        Action::FitWidth,
        Action::FitPage,
    ] {
        expected.apply(action);
        viewer.action(action);
        let session = viewer.session.as_ref().unwrap();
        assert_eq!(session.state, expected);
        assert_eq!(session.generation, generation);
        assert_eq!(session.texture.as_ref().unwrap().id(), texture);
    }
}

#[test]
fn fitted_and_percentage_sizes_preserve_aspect_ratio() {
    let image = egui::vec2(100.0, 200.0);
    let available = egui::vec2(400.0, 300.0);
    assert_eq!(
        display_size(image, available, ManualZoom::FitPage),
        egui::vec2(150.0, 300.0)
    );
    assert_eq!(
        display_size(image, available, ManualZoom::FitWidth),
        egui::vec2(400.0, 800.0)
    );
    assert_eq!(
        display_size(image, available, ManualZoom::Percent(150)),
        egui::vec2(150.0, 300.0)
    );
}

#[test]
fn stale_page_result_cannot_replace_newer_requested_page() {
    let (_root, ctx, mut viewer) = fixture(None);
    let session = viewer.session.as_ref().unwrap();
    let document = session.document.as_ref().unwrap().clone();
    let old = session.generation;
    viewer.action(Action::NextPage);
    viewer.accept(
        &ctx,
        old,
        Reply::Page(
            document.id().clone(),
            0,
            Ok(document.decode_page(0).unwrap()),
        ),
    );
    assert!(viewer.session.as_ref().unwrap().texture.is_none());
    // Matching generation alone is insufficient: page and identity are checked too.
    let generation = viewer.session.as_ref().unwrap().generation;
    viewer.accept(
        &ctx,
        generation,
        Reply::Page(
            document.id().clone(),
            0,
            Ok(document.decode_page(0).unwrap()),
        ),
    );
    assert!(viewer.session.as_ref().unwrap().texture.is_none());
    wait(&mut viewer, &ctx);
    assert_eq!(viewer.session.as_ref().unwrap().state.current_page(), 1);
}

#[test]
fn switching_documents_reuses_worker_and_invalidates_old_results() {
    let (root, ctx, mut viewer) = fixture(None);
    let document = viewer
        .session
        .as_ref()
        .unwrap()
        .document
        .as_ref()
        .unwrap()
        .clone();
    let generation = viewer.session.as_ref().unwrap().generation;
    let worker = Arc::as_ptr(&viewer.worker.as_ref().unwrap().0);
    let next = root.path().join("next.cbz");
    cbz(&next);
    viewer.open(&ctx, next.clone(), "Next".into(), None);
    viewer.accept(&ctx, generation, Reply::Open(Ok(document.clone())));
    viewer.accept(
        &ctx,
        generation,
        Reply::Page(
            document.id().clone(),
            0,
            Ok(document.decode_page(0).unwrap()),
        ),
    );
    assert!(viewer.session.as_ref().unwrap().document.is_none());
    assert!(viewer.session.as_ref().unwrap().texture.is_none());
    assert_eq!(worker, Arc::as_ptr(&viewer.worker.as_ref().unwrap().0));
    wait(&mut viewer, &ctx);
    assert_eq!(
        viewer
            .session
            .as_ref()
            .unwrap()
            .state
            .document()
            .unwrap()
            .path,
        next
    );
}

#[test]
fn close_invalidates_pending_results_and_drops_texture() {
    let (_root, ctx, mut viewer) = fixture(None);
    let session = viewer.session.as_ref().unwrap();
    let document = session.document.as_ref().unwrap().clone();
    let generation = session.generation;
    viewer.action(Action::Close);
    viewer.accept(&ctx, generation, Reply::Open(Ok(document)));
    assert!(viewer.session.is_none());
    assert!(
        viewer
            .worker
            .as_ref()
            .unwrap()
            .0
            .0
            .lock()
            .unwrap()
            .request
            .is_none()
    );
}

#[test]
fn worker_mailbox_keeps_only_latest_pending_request() {
    let worker = Worker(Arc::new((Mutex::new(Mailbox::default()), Condvar::new())));
    for index in 0..1000 {
        worker.request(Some(Work::Open(
            format!("{index}.cbz").into(),
            ManualLimits::default(),
            None,
        )));
    }
    let mailbox = worker.0.0.lock().unwrap();
    assert_eq!(mailbox.generation, 1000);
    assert!(
        matches!(&mailbox.request, Some(Work::Open(path, _, _)) if path == Path::new("999.cbz"))
    );
}

#[test]
fn changed_source_surfaces_error_and_requires_explicit_reopen() {
    let (root, ctx, mut viewer) = fixture(None);
    fs::write(root.path().join("manual.cbz"), b"changed").unwrap();
    viewer.action(Action::NextPage);
    wait(&mut viewer, &ctx);
    let session = viewer.session.as_ref().unwrap();
    assert!(session.error.as_ref().unwrap().contains("changed"));
    assert!(session.texture.is_none());
    viewer.action(Action::NextPage);
    wait(&mut viewer, &ctx);
    assert!(
        viewer
            .session
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("changed")
    );
    assert!(viewer.session.as_ref().unwrap().texture.is_none());
    viewer.action(Action::Close);
    assert!(viewer.session.is_none());
}

#[test]
fn reading_never_rewrites_or_extracts_beside_the_source() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    cbz(&path);
    let before = fs::read(&path).unwrap();
    let ctx = egui::Context::default();
    let mut viewer = ManualViewer::default();
    viewer.open(&ctx, path.clone(), "Manual".into(), None);
    wait(&mut viewer, &ctx);
    for action in [Action::NextPage, Action::LastPage, Action::FirstPage] {
        viewer.action(action);
        wait(&mut viewer, &ctx);
    }
    viewer.action(Action::Close);
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn renderer_dimension_limit_is_enforced_by_the_canonical_decoder() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    cbz_with_size(&path, 1025, 12);
    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            max_texture_side: Some(1024),
            ..Default::default()
        },
        |_| {},
    );
    let mut viewer = ManualViewer::default();
    viewer.open(&ctx, path, "Manual".into(), None);
    wait(&mut viewer, &ctx);
    assert!(viewer.session.as_ref().unwrap().error.is_some());
    assert!(viewer.session.as_ref().unwrap().texture.is_none());
}

#[test]
fn malformed_container_returns_an_open_error() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    fs::write(&path, b"PK\x03\x04truncated").unwrap();
    let ctx = egui::Context::default();
    let mut viewer = ManualViewer::default();
    viewer.open(&ctx, path, "Manual".into(), None);
    wait(&mut viewer, &ctx);
    assert!(viewer.session.as_ref().unwrap().error.is_some());
    assert!(!viewer.session.as_ref().unwrap().state.is_open());
}

#[test]
fn content_signature_is_rechecked_even_when_extension_says_cbz() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("fake.cbz");
    fs::write(&path, b"Rar!\x1a\x07\x00").unwrap();
    let ctx = egui::Context::default();
    let mut viewer = ManualViewer::default();
    viewer.open(&ctx, path, "Fake CBZ".into(), None);
    wait(&mut viewer, &ctx);
    let session = viewer.session.as_ref().unwrap();
    assert!(!session.state.is_open());
    assert!(session.error.as_ref().unwrap().contains("RAR reader"));
}

#[test]
fn bad_image_is_a_visible_decode_failure_not_a_panic() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("bad.cbz");
    let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    writer
        .start_file("page.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"not an image").unwrap();
    writer.finish().unwrap();
    let ctx = egui::Context::default();
    let mut viewer = ManualViewer::default();
    viewer.open(&ctx, path, "Broken".into(), None);
    wait(&mut viewer, &ctx);
    assert!(viewer.session.as_ref().unwrap().error.is_some());
    assert!(viewer.session.as_ref().unwrap().texture.is_none());
}

#[test]
fn keyboard_maps_native_keys_and_consumes_escape_first() {
    let ctx = egui::Context::default();
    for (key, expected) in [
        (egui::Key::ArrowLeft, Action::PreviousPage),
        (egui::Key::PageUp, Action::PreviousPage),
        (egui::Key::ArrowRight, Action::NextPage),
        (egui::Key::PageDown, Action::NextPage),
        (egui::Key::Home, Action::FirstPage),
        (egui::Key::End, Action::LastPage),
        (egui::Key::Plus, Action::ZoomIn),
        (egui::Key::Equals, Action::ZoomIn),
        (egui::Key::Minus, Action::ZoomOut),
        (egui::Key::Escape, Action::Close),
    ] {
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: if key == egui::Key::Plus {
                        egui::Modifiers::SHIFT
                    } else {
                        egui::Modifiers::NONE
                    },
                }],
                ..Default::default()
            },
            |ctx| {
                assert_eq!(keyboard_action(ctx), Some(expected));
                assert_eq!(keyboard_action(ctx), None);
            },
        );
    }
}

#[test]
fn reading_state_round_trip_preserves_fingerprint_and_fit_and_restarts_changed_document() {
    let (root, ctx, mut viewer) = fixture(None);
    viewer.action(Action::LastPage);
    wait(&mut viewer, &ctx);
    viewer.action(Action::FitWidth);
    let saved = viewer.reading().unwrap().1;
    let serialized = serde_json::to_string(&saved).unwrap();
    let saved: DocumentReadingState = serde_json::from_str(&serialized).unwrap();
    assert_eq!(saved.fit_mode, Some(DocumentFitMode::Width));
    let path = root.path().join("manual.cbz");
    viewer.open(&ctx, path.clone(), "Manual".into(), Some(saved.clone()));
    wait(&mut viewer, &ctx);
    assert_eq!(viewer.session.as_ref().unwrap().state.page_number(), 3);
    assert_eq!(
        viewer.session.as_ref().unwrap().state.zoom(),
        ManualZoom::FitWidth
    );
    cbz_with_size(&path, 11, 12);
    viewer.open(&ctx, path, "Replacement".into(), Some(saved));
    wait(&mut viewer, &ctx);
    assert_eq!(viewer.session.as_ref().unwrap().state.page_number(), 1);
    assert_eq!(
        viewer.session.as_ref().unwrap().state.zoom(),
        ManualZoom::FitPage
    );
}

#[test]
fn legacy_path_only_resume_is_not_applied_to_an_unfingerprinted_document() {
    let (root, ctx, mut viewer) = fixture(None);
    let saved: DocumentReadingState =
        serde_json::from_str(r#"{"last_page":3,"zoom_percent":200}"#).unwrap();
    assert!(saved.document_id.is_none());
    viewer.open(
        &ctx,
        root.path().join("manual.cbz"),
        "Manual".into(),
        Some(saved),
    );
    wait(&mut viewer, &ctx);
    assert_eq!(viewer.session.as_ref().unwrap().state.page_number(), 1);
    assert_eq!(
        viewer.session.as_ref().unwrap().state.zoom(),
        ManualZoom::FitPage
    );
}

#[test]
fn weak_filename_and_unmatched_documents_never_offer_open_actions() {
    let ctx = egui::Context::default();
    for association in [
        GameDocumentAssociation::WeakFilename,
        GameDocumentAssociation::Unmatched,
    ] {
        let mut doc = document(ManualReadiness::Viewable, DocumentOpenCapability::Supported);
        doc.association = association;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (internal, external) = open_buttons(ui, &doc);
                assert!(!internal.enabled());
                assert!(!external.enabled());
            });
        });
    }
}

#[test]
fn discovery_fingerprint_refuses_a_source_replaced_before_open() {
    let (root, ctx, mut viewer) = fixture(None);
    let path = root.path().join("manual.cbz");
    let expected = viewer.session.as_ref().unwrap().state.document().cloned();
    cbz_with_size(&path, 11, 12);
    viewer.open_checked(&ctx, path, "Manual".into(), None, expected);
    wait(&mut viewer, &ctx);
    assert!(!viewer.session.as_ref().unwrap().state.is_open());
    assert!(
        viewer
            .session
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("changed")
    );
}

#[test]
fn requested_page_only_decode_allows_skipping_a_corrupt_page() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for (name, good) in [("1.png", true), ("2.png", false), ("3.png", true)] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        if good {
            let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            zip.write_all(bytes.get_ref()).unwrap();
        } else {
            zip.write_all(b"not an image").unwrap();
        }
    }
    zip.finish().unwrap();
    let ctx = egui::Context::default();
    let mut viewer = ManualViewer::default();
    viewer.open(&ctx, path, "Manual".into(), None);
    wait(&mut viewer, &ctx);
    assert!(viewer.session.as_ref().unwrap().texture.is_some());
    viewer.action(Action::NextPage);
    wait(&mut viewer, &ctx);
    assert!(viewer.session.as_ref().unwrap().error.is_some());
    viewer.action(Action::NextPage);
    wait(&mut viewer, &ctx);
    assert!(viewer.session.as_ref().unwrap().texture.is_some());
    assert!(viewer.session.as_ref().unwrap().error.is_none());
}

#[test]
fn explicit_and_exact_associations_open_the_selected_document_and_weak_does_not() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("selected.cbz");
    let other = root.path().join("other.cbz");
    cbz(&path);
    cbz(&other);
    let ctx = egui::Context::default();
    let parsed = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    let mut doc = document(ManualReadiness::Viewable, DocumentOpenCapability::NoHandler);
    doc.path = path.clone();
    doc.document_id = Some(parsed.id().clone());
    let evidence = archivefs_core::launch::planning::ResolvedIdentity {
        platform_id: "snes".into(),
        game_key: "selected".into(),
    };
    for association in [
        GameDocumentAssociation::Explicit,
        GameDocumentAssociation::ExactGameIdentity(
            super::super::documents::ExactIdentityEvidence::from(&evidence),
        ),
    ] {
        doc.association = association;
        let mut viewer = ManualViewer::default();
        viewer.open_associated(&ctx, &doc, None);
        wait(&mut viewer, &ctx);
        assert_eq!(
            viewer.session.as_ref().unwrap().state.document(),
            Some(parsed.id())
        );
        assert_eq!(viewer.session.as_ref().unwrap().path, path);
    }
    doc.association = GameDocumentAssociation::WeakFilename;
    let mut viewer = ManualViewer::default();
    viewer.open_associated(&ctx, &doc, None);
    assert!(viewer.session.is_none());
    assert!(viewer.worker.is_none());
}

#[test]
fn external_open_plan_checks_absolute_path_fingerprint_and_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.cbz");
    cbz(&path);
    let doc = ManualDocument::open(&path, &ManualLimits::default()).unwrap();
    assert!(super::super::documents::validate_external_document(&path, doc.id()).is_ok());
    assert!(
        super::super::documents::validate_external_document(Path::new("manual.cbz"), doc.id())
            .is_err()
    );
    cbz_with_size(&path, 11, 12);
    assert!(super::super::documents::validate_external_document(&path, doc.id()).is_err());
    #[cfg(unix)]
    {
        let replacement = root.path().join("replacement.cbz");
        cbz(&replacement);
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&replacement, &path).unwrap();
        assert!(super::super::documents::validate_external_document(&path, doc.id()).is_err());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn local_mapped_romm_manual_uses_the_same_internal_reader() {
    use archivefs_core::identity_source::{
        model::MediaReference,
        romm::media_mapping::{RommMediaMapping, validate_romm_media_mapping},
    };
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("manual.pdf");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 30] /Resources << >> >>",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    fs::write(&path, bytes).unwrap();
    let mapping = validate_romm_media_mapping(&RommMediaMapping {
        provider_prefix: "/assets/manuals".into(),
        local_root: root.path().into(),
    })
    .unwrap();
    let manual = MediaReference {
        hosted_reference: Some("/assets/manuals/manual.pdf".into()),
        public_reference: None,
    };
    let document = super::super::documents::project_romm_manual_document(
        super::super::documents::RommDocumentRequest {
            game_id: 4,
            platform: "SNES",
            mapping: Some(&mapping),
            manual: &manual,
            verified_identity: None,
        },
    )
    .unwrap();
    let ctx = egui::Context::default();
    let mut viewer = ManualViewer::default();
    viewer.open_associated(&ctx, &document, None);
    wait(&mut viewer, &ctx);
    assert_eq!(viewer.session.as_ref().unwrap().path, path);
    assert_eq!(viewer.session.as_ref().unwrap().state.page_count(), 1);
    assert_eq!(
        viewer
            .session
            .as_ref()
            .unwrap()
            .document
            .as_ref()
            .unwrap()
            .inspection()
            .kind,
        ManualDocumentKind::Pdf
    );
}
