//! Actual shared-shell/worker tests. All catalogue and ROM bytes are synthetic.
use super::*;
use crate::gui_v2::mame_collection_health::{HEALTH_GOOD_FIXTURE, HealthFixture};
use crate::gui_v2::organisation::OrganisationView;

fn ready_page(context: &egui::Context, f: &HealthFixture, populated: bool) -> App {
    let mut app = fixture(context);
    app.router.current = Route::Section(Section::Build);
    app.organisation.view = OrganisationView::MameHealth;
    let page = &mut app.organisation.mame_health;
    page.root = f.request.root.to_string_lossy().into();
    page.selected = Some(f.request.reference.clone());
    page.version_output = "MAME 0.280".into();
    page.picker.ensure_loaded_with(f.request.snapshot.clone());
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while page.picker.loading && std::time::Instant::now() < end {
        page.picker.poll();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!page.picker.loading);
    if populated {
        page.report = Some(f.report().into());
        page.selected_set = Some("g".into());
        page.poll();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while page.checking_sources() && std::time::Instant::now() < end {
            page.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(!page.checking_sources());
    }
    app
}
fn visible(output: &egui::FullOutput, wanted: &str) -> Option<egui::Rect> {
    fn find(shape: &egui::Shape, clip: egui::Rect, wanted: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == wanted => {
                let r = egui::Rect::from_min_size(t.pos, t.galley.size());
                clip.contains_rect(r).then_some(r)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, clip, wanted)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|s| find(&s.shape, s.clip_rect, wanted))
}
fn reach(context: &egui::Context, app: &mut App, size: [f32; 2], wanted: &str) -> egui::Rect {
    for _ in 0..25 {
        let out = frame(context, app, size);
        if let Some(r) = visible(&out, wanted) {
            return r;
        }
        frame_with(
            context,
            app,
            size,
            vec![
                egui::Event::PointerMoved(egui::pos2(size[0] - 80.0, size[1] - 100.0)),
                egui::Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -80.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        for _ in 0..8 {
            frame(context, app, size);
        }
    }
    panic!("{wanted} is not reachable in {size:?}");
}
fn click(
    context: &egui::Context,
    app: &mut App,
    size: [f32; 2],
    rect: egui::Rect,
) -> egui::FullOutput {
    let mut output = None;
    for pressed in [true, false] {
        output = Some(frame_with(
            context,
            app,
            size,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        ));
    }
    output.unwrap()
}

#[test]
fn catalogue_choice_is_explicit_and_reaches_the_real_worker() {
    let f = HealthFixture::new(HEALTH_GOOD_FIXTURE);
    f.rom("g", b"hello world");
    let context = egui::Context::default();
    let mut app = ready_page(&context, &f, false);
    app.organisation.mame_health.selected = None;
    let size = [1024.0, 2000.0];
    let output = frame(&context, &mut app, size);
    assert!(app.organisation.mame_health.selected.is_none());
    assert!(
        text(&output)
            .iter()
            .any(|s| s.contains("Arcade catalogue type is not recorded"))
    );
    let row = reach(&context, &mut app, size, "Synthetic MAME arcade");
    click(&context, &mut app, size, row);
    assert_eq!(
        app.organisation.mame_health.selected,
        Some(f.request.reference.clone())
    );
    let inspect = reach(&context, &mut app, size, "Inspect collection");
    click(&context, &mut app, size, inspect);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while app.organisation.mame_health.job.is_some() && std::time::Instant::now() < end {
        frame(&context, &mut app, size);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        app.organisation
            .mame_health
            .report
            .as_ref()
            .unwrap()
            .analysis
            .passing_sets,
        1
    );
}

fn copied(output: &egui::FullOutput) -> Option<&str> {
    output
        .platform_output
        .commands
        .iter()
        .find_map(|c| match c {
            egui::OutputCommand::CopyText(text) => Some(text.as_str()),
            _ => None,
        })
}

#[test]
fn filters_search_and_copied_receipts_use_actual_evidence() {
    let xml = format!(
        "{}{}<machine name=\"u\"><description>Unowned Game</description><rom name=\"main.bin\" size=\"11\"/></machine>",
        HEALTH_GOOD_FIXTURE,
        HEALTH_GOOD_FIXTURE
            .replace("name=\"g\"", "name=\"b\"")
            .replace("Test Game", "Broken Game")
    );
    let f = HealthFixture::new(&xml);
    f.rom("g", b"hello world");
    f.rom("b", b"wrong bytes");
    let context = egui::Context::default();
    let mut app = ready_page(&context, &f, true);
    app.beginner_hints_enabled = true;
    app.organisation.mame_health.selected_set = None;
    let size = [1280.0, 4000.0];
    let all = text(&frame(&context, &mut app, size));
    for wanted in ["Good 1", "Bad 1", "Unknown 1"] {
        assert!(all.iter().any(|s| s == wanted));
    }
    let bad = reach(&context, &mut app, size, "Bad");
    click(&context, &mut app, size, bad);
    let rows = text(&frame(&context, &mut app, size));
    assert!(rows.iter().any(|s| s == "b"));
    assert!(!rows.iter().any(|s| s == "g" || s == "u"));
    let all = reach(&context, &mut app, size, "All");
    click(&context, &mut app, size, all);
    let search = reach(&context, &mut app, size, "Find set or description");
    click(&context, &mut app, size, search);
    frame_with(
        &context,
        &mut app,
        size,
        vec![egui::Event::Text("Test Game".into())],
    );
    assert_eq!(app.organisation.mame_health.query, "Test Game");
    let rows = text(&frame(&context, &mut app, size));
    assert!(rows.iter().any(|s| s == "g"));
    assert!(!rows.iter().any(|s| s == "b" || s == "u"));
    let json_button = reach(&context, &mut app, size, "Copy JSON report");
    let output = click(&context, &mut app, size, json_button);
    let receipt: serde_json::Value = serde_json::from_str(copied(&output).unwrap()).unwrap();
    assert_eq!(receipt["analysis"]["passing_sets"], 1);
    assert_eq!(receipt["analysis"]["failing_sets"], 1);
    assert_eq!(receipt["analysis"]["catalogue_version"], "0.280");
    assert_eq!(
        receipt["inspection"]["inventory"]["inspected_completely"],
        true
    );
    assert_eq!(receipt["catalogue"]["source_id"], "synthetic");
    assert_eq!(receipt["catalogue"]["sha256"].as_str().unwrap().len(), 64);
    let summary = reach(&context, &mut app, size, "Copy report summary");
    let output = click(&context, &mut app, size, summary);
    assert!(
        copied(&output)
            .unwrap()
            .contains("Inspection complete: true")
    );
    assert!(copied(&output).unwrap().contains("Catalogue SHA-256:"));
    let why = reach(&context, &mut app, size, "Why?");
    click(&context, &mut app, size, why);
    let copy = reach(&context, &mut app, size, "Copy this set's evidence");
    let mut output = click(&context, &mut app, size, copy);
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while copied(&output).is_none() && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(2));
        output = frame(&context, &mut app, size);
    }
    let receipt: serde_json::Value = serde_json::from_str(copied(&output).unwrap()).unwrap();
    assert_eq!(receipt["set"]["name"], "g");
    assert_eq!(receipt["catalogue"]["source_id"], "synthetic");
    assert_eq!(receipt["inspection_complete"], true);
    assert_eq!(
        receipt["declared_context"]["rom_declarations"][0]["size_bytes"],
        11
    );
    assert!(
        receipt["set"]["evidence"]["requirements"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["content"] == "verified")
    );
    assert!(app.canonical_organisation_job.is_none());
    assert!(app.playing_library_job.is_none());
    assert_eq!(
        fs::read(f.request.root.join("b/main.bin")).unwrap(),
        b"wrong bytes"
    );
}

#[test]
fn unrecognised_collection_sets_have_truthful_unknown_explanations() {
    let f = HealthFixture::new(HEALTH_GOOD_FIXTURE);
    f.rom("g", b"hello world");
    f.rom("unlisted", b"unrelated bytes");
    let context = egui::Context::default();
    let mut app = ready_page(&context, &f, true);
    app.organisation.mame_health.selected_set = None;
    let size = [1280.0, 4000.0];
    let unknown = reach(&context, &mut app, size, "Unknown");
    click(&context, &mut app, size, unknown);
    let row = text(&frame(&context, &mut app, size));
    assert!(
        row.iter().any(|s| s == "unlisted"),
        "unrecognised set is counted Unknown but has no row"
    );
    assert!(!row.iter().any(|s| s == "g"));
    let why = reach(&context, &mut app, size, "Why?");
    click(&context, &mut app, size, why);
    let strings = text(&frame(&context, &mut app, size));
    assert!(strings.iter().any(|s| s == "Why? unlisted"));
    assert!(
        strings
            .iter()
            .any(|s| s.contains("No matching machine declaration"))
    );
    assert!(
        strings
            .iter()
            .any(|s| s.contains("cannot establish verified content"))
    );
    assert!(app.canonical_organisation_job.is_none());
}

#[test]
fn both_shells_cancel_control_is_reachable_at_160_percent() {
    use crate::gui_v2::mame_collection_health::{HealthJob, HealthMessage};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    for simple in [false, true] {
        for zoom in [1.0, 1.6] {
            let f = HealthFixture::new(HEALTH_GOOD_FIXTURE);
            f.rom("g", b"hello world");
            let context = egui::Context::default();
            let mut app = ready_page(&context, &f, false);
            app.simple.enabled = simple;
            app.simple.ui_scale = zoom;
            let size = [1024.0 / zoom, 600.0 / zoom];
            let (sender, receiver) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            app.organisation.mame_health.job = Some(HealthJob {
                request: f.request.clone(),
                cancel: cancel.clone(),
                receiver,
            });
            frame(&context, &mut app, size);
            context.set_zoom_factor(zoom);
            let button = reach(&context, &mut app, size, "Cancel inspection");
            click(&context, &mut app, size, button);
            assert!(cancel.load(Ordering::Relaxed));
            // Exercise the real cancelled worker and the same completion path,
            // after deterministically testing the button before work starts.
            let request = f.request.clone();
            std::thread::spawn(move || {
                let result = crate::gui_v2::mame_collection_health::run_worker_for_test(
                    request,
                    &cancel,
                    &mut |_| {},
                );
                sender.send(HealthMessage::Finished(1, result)).unwrap();
            });
            let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while app.organisation.mame_health.job.is_some() && std::time::Instant::now() < end {
                frame(&context, &mut app, size);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(app.organisation.mame_health.report.is_none());
            assert!(
                app.organisation
                    .mame_health
                    .message
                    .as_ref()
                    .unwrap()
                    .contains("cancel")
            );
            assert!(app.canonical_organisation_job.is_none());
            assert!(!app.organisation.mame_reconstruction.publish_pending);
            assert_eq!(
                fs::read(f.request.root.join("g/main.bin")).unwrap(),
                b"hello world"
            );
            eprintln!(
                "MAMEHEALTH_CANCEL shell={} scale={zoom} viewport=1024x600: button/cancelled worker PASS",
                if simple { "simple" } else { "classic" }
            );
        }
    }
}

#[test]
fn both_shells_at_small_and_160_percent_allow_inspect_and_back() {
    for simple in [false, true] {
        for zoom in [1.0, 1.6] {
            let f = HealthFixture::new(HEALTH_GOOD_FIXTURE);
            f.rom("g", b"hello world");
            let context = egui::Context::default();
            let mut app = ready_page(&context, &f, false);
            app.simple.enabled = simple;
            app.simple.ui_scale = zoom;
            let size = [1024.0 / zoom, 600.0 / zoom];
            frame(&context, &mut app, size);
            context.set_zoom_factor(zoom);
            frame(&context, &mut app, size);
            let back = reach(&context, &mut app, size, "← Organisation");
            assert!(back.width() > 0.0);
            let inspect = reach(&context, &mut app, size, "Inspect collection");
            click(&context, &mut app, size, inspect);
            assert!(
                app.organisation.mame_health.job.is_some()
                    || app.organisation.mame_health.report.is_some(),
                "Inspect did not start: {:?}",
                app.organisation.mame_health.message
            );
            assert!(app.canonical_organisation_job.is_none());
            assert!(app.playing_library_job.is_none());
            let end = std::time::Instant::now() + std::time::Duration::from_secs(4);
            while app.organisation.mame_health.job.is_some() && std::time::Instant::now() < end {
                frame(&context, &mut app, size);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert_eq!(
                app.organisation
                    .mame_health
                    .report
                    .as_ref()
                    .unwrap()
                    .analysis
                    .passing_sets,
                1
            );
            eprintln!(
                "MAMEHEALTH_REACH shell={} scale={zoom} viewport=1024x600: back/inspect actual worker PASS",
                if simple { "simple" } else { "classic" }
            );
            assert_eq!(
                fs::read(f.request.root.join("g/main.bin")).unwrap(),
                b"hello world"
            );
            // Return to the top through ordinary page scrolling and activate
            // Back. No transaction worker or mutation plan is involved.
            for _ in 0..20 {
                frame_with(
                    &context,
                    &mut app,
                    size,
                    vec![
                        egui::Event::PointerMoved(egui::pos2(size[0] - 80.0, size[1] - 100.0)),
                        egui::Event::MouseWheel {
                            phase: egui::TouchPhase::Move,
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, 100.0),
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            let back = reach(&context, &mut app, size, "← Organisation");
            click(&context, &mut app, size, back);
            assert_eq!(app.organisation.view, OrganisationView::Landing);
        }
    }
}

#[test]
fn populated_health_has_real_explanations_and_no_destructive_controls() {
    let f = HealthFixture::new(HEALTH_GOOD_FIXTURE);
    f.rom("g", b"hello world");
    let context = egui::Context::default();
    let mut app = ready_page(&context, &f, true);
    let strings = text(&frame(&context, &mut app, [1280.0, 4000.0]));
    for wanted in [
        "Good 1",
        "Bad 0",
        "Unknown 0",
        "Why? g",
        "Copy JSON report",
        "Copy this set's evidence",
    ] {
        assert!(strings.iter().any(|s| s == wanted), "missing {wanted}");
    }
    assert!(strings.iter().any(|s| s.contains("strong hashes")));
    assert!(
        strings
            .iter()
            .any(|s| s.contains("does not prove game runnability"))
    );
    for forbidden in [
        "Apply",
        "Publish",
        "Repair",
        "Rebuild",
        "Replace",
        "Undo",
        "Publish rebuilt ZIP",
        "Review internal repair",
        "Confirm",
    ] {
        assert!(
            !strings.iter().any(|s| s == forbidden),
            "destructive control {forbidden}"
        );
    }
    assert!(app.canonical_organisation_job.is_none());
    assert!(app.playing_library_job.is_none());
    assert!(!app.organisation.mame_reconstruction.publish_pending);
    assert!(app.organisation.mame_reconstruction.undo_pending.is_none());
}

#[test]
fn beginner_explanations_expand_without_changing_assessment_or_sources() {
    let f = HealthFixture::new(HEALTH_GOOD_FIXTURE);
    f.rom("g", b"hello world");
    let context = egui::Context::default();
    let mut app = ready_page(&context, &f, true);
    let report = app.organisation.mame_health.report.clone().unwrap();
    let catalogue = fs::read(&f.request.catalogue_path).unwrap();
    let size = [1280.0, 4000.0];
    let closed = text(&frame(&context, &mut app, size));
    assert!(closed.iter().any(|s| s == "What do these results mean?"));
    assert!(!closed.iter().any(|s| s.starts_with("Good:")));

    let header = reach(&context, &mut app, size, "What do these results mean?");
    click(&context, &mut app, size, header);
    // Allow the collapsing header's animation to finish in the real shell.
    for _ in 0..8 {
        frame(&context, &mut app, size);
    }
    let expanded = text(&frame(&context, &mut app, size));
    for explanation in [
        "Good: required content was verified",
        "Bad: required content is missing or disagrees",
        "Unknown: the evidence cannot establish completeness",
        "Missing ROMs: required files were not found",
        "Incorrect hashes: a checksum or size disagrees",
        "BAD_DUMP: the reference marks this as a known imperfect dump",
        "NO_DUMP: no verified good dump is known",
        "Unverified means its content and any parent chain have not been verified",
        "Complete storage does not prove game runnability",
    ] {
        assert!(
            expanded.iter().any(|s| s.contains(explanation)),
            "missing explanation: {explanation}"
        );
    }
    assert!(expanded.iter().any(|s| s == "Good 1"));
    assert!(std::sync::Arc::ptr_eq(
        &report,
        app.organisation.mame_health.report.as_ref().unwrap()
    ));
    assert!(app.organisation.mame_health.job.is_none());
    assert!(app.canonical_organisation_job.is_none());
    assert!(app.playing_library_job.is_none());
    assert!(!app.organisation.mame_reconstruction.publish_pending);
    assert!(app.organisation.mame_reconstruction.undo_pending.is_none());
    assert_eq!(fs::read(&f.request.catalogue_path).unwrap(), catalogue);
    assert_eq!(
        fs::read(f.request.root.join("g/main.bin")).unwrap(),
        b"hello world"
    );

    let header = reach(&context, &mut app, size, "What do these results mean?");
    click(&context, &mut app, size, header);
    for _ in 0..8 {
        frame(&context, &mut app, size);
    }
    assert!(
        !text(&frame(&context, &mut app, size))
            .iter()
            .any(|s| s.starts_with("Good:"))
    );
}

#[test]
fn both_shells_keep_expanded_help_and_primary_controls_reachable_at_160_percent() {
    fn scroll_to_top(context: &egui::Context, app: &mut App, size: [f32; 2]) {
        for _ in 0..20 {
            frame_with(
                context,
                app,
                size,
                vec![
                    egui::Event::PointerMoved(egui::pos2(size[0] - 80.0, size[1] - 100.0)),
                    egui::Event::MouseWheel {
                        phase: egui::TouchPhase::Move,
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, 100.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }
    for simple in [false, true] {
        for zoom in [1.0, 1.6] {
            for populated in [false, true] {
                let f = HealthFixture::new(HEALTH_GOOD_FIXTURE);
                f.rom("g", b"hello world");
                let context = egui::Context::default();
                let mut app = ready_page(&context, &f, populated);
                app.simple.enabled = simple;
                app.simple.ui_scale = zoom;
                let size = [1024.0 / zoom, 600.0 / zoom];
                frame(&context, &mut app, size);
                context.set_zoom_factor(zoom);
                let header = reach(&context, &mut app, size, "What do these results mean?");
                click(&context, &mut app, size, header);
                reach(
                    &context,
                    &mut app,
                    size,
                    "CHDs: Missing means the required disk image was not found. Unverified means its content and any parent chain have not been verified; a .chd filename or its presence is insufficient.",
                );
                reach(
                    &context,
                    &mut app,
                    size,
                    "Choose a set's Why? for the evidence behind its result. Complete storage does not prove game runnability; this page does not test whether games run.",
                );
                // The surrounding page scrolls; expanding help must not hide
                // the existing primary controls or push them outside its width.
                scroll_to_top(&context, &mut app, size);
                reach(&context, &mut app, size, "Inspect collection");
                // Reaching Inspect can itself scroll Back above the viewport.
                scroll_to_top(&context, &mut app, size);
                let back = reach(&context, &mut app, size, "← Organisation");
                click(&context, &mut app, size, back);
                assert_eq!(app.organisation.view, OrganisationView::Landing);
                assert!(app.canonical_organisation_job.is_none());
                assert!(app.playing_library_job.is_none());
            }
        }
    }
}

#[test]
fn incomplete_and_preservation_only_reports_never_show_good() {
    let xml=HEALTH_GOOD_FIXTURE.replace("</machine>","<rom name=\"undumped.bin\" size=\"1\" status=\"nodump\"/><rom name=\"bad.bin\" size=\"11\" sha1=\"2aae6c35c94fcfb415dbe95f408b9ce91ee846ed\" status=\"baddump\"/></machine>");
    let f = HealthFixture::new(&xml);
    f.rom("g", b"hello world");
    let context = egui::Context::default();
    let mut app = ready_page(&context, &f, true);
    let strings = text(&frame(&context, &mut app, [1280.0, 4000.0]));
    assert!(
        strings
            .iter()
            .any(|s| s.contains("NO_DUMP: no verified good dump"))
    );
    assert!(
        strings
            .iter()
            .any(|s| s.contains("BAD_DUMP: the recorded dump"))
    );
    assert!(!strings.iter().any(|s| s == "Good 1"));
    fs::write(f.request.root.join("unknown.7z"), b"unsupported").unwrap();
    app.organisation.mame_health.report = Some(f.report().into());
    let strings = text(&frame(&context, &mut app, [1280.0, 4000.0]));
    assert!(
        strings
            .iter()
            .any(|s| s == "Incomplete inspection — no verified-complete sets")
    );
}

#[test]
fn organisation_entry_opens_health_without_a_mutation_plan() {
    let context = egui::Context::default();
    let mut app = fixture(&context);
    app.router.current = Route::Section(Section::Build);
    let size = [1024.0, 600.0];
    let button = reach(&context, &mut app, size, "Check MAME collection health");
    click(&context, &mut app, size, button);
    assert_eq!(app.organisation.view, OrganisationView::MameHealth);
    assert!(app.organisation.mame_reconstruction.preview.is_none());
    assert!(app.organisation.mame_health.report.is_none());
}
