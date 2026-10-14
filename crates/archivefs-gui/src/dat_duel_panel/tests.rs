use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::*;

const S1: &str = "1111111111111111111111111111111111111111";
const S2: &str = "2222222222222222222222222222222222222222";
const S3: &str = "3333333333333333333333333333333333333333";

fn dat(dir: &tempfile::TempDir, name: &str, header: &str, games: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(
        &path,
        format!(
            "<?xml version=\"1.0\"?>\n<datafile><header><name>{header}</name><version>1</version></header>{games}</datafile>"
        ),
    )
    .unwrap();
    path
}

fn report(a: &Path, b: &Path) -> DuelReport {
    match run_duel(a, b, false) {
        WorkerResult::Report(r) => *r,
        WorkerResult::Refused(r) => panic!("refused: {r}"),
        WorkerResult::Failed(e) => panic!("failed: {e}"),
    }
}

fn wait(panel: &mut DatDuelPanel) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while panel.is_busy() {
        panel.poll();
        assert!(
            std::time::Instant::now() < deadline,
            "worker never finished"
        );
        std::thread::yield_now();
    }
}

fn shown(panel: &DatDuelPanel) -> &Shown {
    match &panel.outcome {
        Some(Outcome::Report(s)) => s,
        _ => panic!("expected a report"),
    }
}

fn text_of(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    output
        .shapes
        .iter()
        .for_each(|c| collect(&c.shape, &mut out));
    out
}

fn text_pos(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
    fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.text().contains(needle) => {
                Some(t.pos + egui::vec2(8.0, 6.0))
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, needle)),
            _ => None,
        }
    }
    output.shapes.iter().find_map(|c| find(&c.shape, needle))
}

fn frame(
    ctx: &egui::Context,
    panel: &mut DatDuelPanel,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 600.0),
        )),
        events,
        ..Default::default()
    };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| panel.show(ui));
        });
    })
}

fn click(ctx: &egui::Context, panel: &mut DatDuelPanel, at: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(ctx, panel, vec![egui::Event::PointerMoved(at)]);
    frame(ctx, panel, vec![button(true)]);
    frame(ctx, panel, vec![button(false)]);
}

fn compare_via_panel(a: &Path, b: &Path) -> DatDuelPanel {
    let mut panel = DatDuelPanel::default();
    panel.select(0, a.to_path_buf());
    panel.select(1, b.to_path_buf());
    let (a, b) = (a.to_path_buf(), b.to_path_buf());
    panel.start_with(move || run_duel(&a, &b, false));
    wait(&mut panel);
    panel
}

#[test]
fn strong_hash_match_with_different_filenames_is_verified() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(
            r#"<game name="Alpha"><rom name="old.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>"#
        ),
    );
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        &format!(
            r#"<game name="Alpha"><rom name="new.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>"#
        ),
    );
    let panel = compare_via_panel(&a, &b);
    let s = shown(&panel);
    let rows = panel.visible_rows(s);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].section, Section::Renamed);
    assert_eq!(rows[0].trust, Trust::Verified);
    assert!(rows[0].title.contains("old.bin") && rows[0].title.contains("new.bin"));
}

#[test]
fn crc_only_match_is_probable_never_verified() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        r#"<game name="Alpha"><rom name="x.bin" size="4" crc="aaaaaaaa"/></game>"#,
    );
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        r#"<game name="Alpha"><rom name="y.bin" size="4" crc="aaaaaaaa"/></game>"#,
    );
    let panel = compare_via_panel(&a, &b);
    let rows = panel.visible_rows(shown(&panel));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].trust, Trust::Probable);
    assert_ne!(rows[0].trust, Trust::Verified);
}

#[test]
fn checksum_contradictions_are_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(
            r#"<game name="Beta"><rom name="b.bin" size="4" crc="bbbbbbbb" sha1="{S2}"/></game>"#
        ),
    );
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        &format!(
            r#"<game name="Beta"><rom name="b.bin" size="4" crc="bbbbbbbb" sha1="{S3}"/></game>"#
        ),
    );
    let panel = compare_via_panel(&a, &b);
    let rows = panel.visible_rows(shown(&panel));
    assert!(
        rows.iter()
            .any(|r| r.section == Section::SameNameConflict && r.trust == Trust::Conflict),
        "{rows:?}"
    );
    assert!(rows.iter().all(|r| r.trust != Trust::Verified));
}

#[test]
fn multiple_candidates_are_ambiguous_and_not_verified() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(
            r#"<game name="One"><rom name="one.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>
           <game name="Two"><rom name="two.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>"#
        ),
    );
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        &format!(
            r#"<game name="Uno"><rom name="uno.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>
           <game name="Dos"><rom name="dos.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>"#
        ),
    );
    let panel = compare_via_panel(&a, &b);
    let rows = panel.visible_rows(shown(&panel));
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .any(|r| r.section == Section::Ambiguous && r.trust == Trust::Ambiguous),
        "{rows:?}"
    );
    assert!(
        rows.iter().all(|r| r.trust != Trust::Verified),
        "ambiguous must never read as verified"
    );
}

#[test]
fn one_sided_entries_are_split_into_confirmed_and_unverifiable() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(
            r#"<game name="OnlyA"><rom name="o.bin" size="4" crc="cccccccc" sha1="{S1}"/></game>
           <game name="NoSum"><rom name="n.bin" size="4"/></game>"#
        ),
    );
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        &format!(
            r#"<game name="Other"><rom name="p.bin" size="4" crc="dddddddd" sha1="{S2}"/></game>"#
        ),
    );
    let panel = compare_via_panel(&a, &b);
    let rows = panel.visible_rows(shown(&panel));
    assert!(
        rows.iter()
            .any(|r| r.section == Section::OnlyInA && r.trust == Trust::Unmatched)
    );
    assert!(
        rows.iter()
            .any(|r| r.section == Section::OnlyInA && r.trust == Trust::Unverifiable)
    );
    assert!(rows.iter().any(|r| r.section == Section::OnlyInB));
}

#[test]
fn incompatible_catalogues_are_refused_until_explicitly_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(&dir, "a.dat", "Nintendo - NES", "");
    let b = dat(&dir, "b.dat", "Sega - Mega Drive", "");
    let mut panel = DatDuelPanel::default();
    panel.select(0, a.clone());
    panel.select(1, b.clone());
    let (pa, pb) = (a.clone(), b.clone());
    // Unconfirmed: the real start path forwards the (false) confirmation.
    panel.start_with(move || run_duel(&pa, &pb, false));
    wait(&mut panel);
    assert!(matches!(panel.outcome, Some(Outcome::Refused(_))));
    assert!(!panel.confirm_incompatible);

    // The override only flows from the explicit acknowledgement.
    panel.set_confirm_incompatible(true);
    let ctx = egui::Context::default();
    panel.start(&ctx);
    wait(&mut panel);
    let s = shown(&panel);
    assert!(s.allowed_incompatible);
    assert!(!s.report.naming_comparable);

    // Changing a selection withdraws the acknowledgement.
    panel.select(0, dat(&dir, "c.dat", "Nintendo - NES", ""));
    assert!(!panel.confirm_incompatible);
    assert!(panel.outcome.is_none());
}

#[test]
fn missing_catalogue_name_is_called_out_in_the_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let no_name = dir.path().join("n.dat");
    std::fs::write(&no_name, "<datafile><game name=\"g\"><rom name=\"r\" size=\"1\" sha1=\"1111111111111111111111111111111111111111\"/></game></datafile>").unwrap();
    let b = dat(&dir, "b.dat", "Test System", "");
    let panel = compare_via_panel(&no_name, &b);
    let Some(Outcome::Refused(r)) = &panel.outcome else {
        panic!("expected refusal");
    };
    assert!(
        describe_refusal(r).contains("no catalogue name")
            || describe_refusal(r).contains("different")
    );
}

#[test]
fn source_replaced_during_comparison_discards_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(r#"<game name="A"><rom name="a.bin" size="4" sha1="{S1}"/></game>"#),
    );
    let b = dat(&dir, "b.dat", "Test System", "");
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let (pa, pb) = (a.clone(), b.clone());
    let mut panel = DatDuelPanel::default();
    panel.select(0, a.clone());
    panel.select(1, b.clone());
    panel.start_with(move || {
        let r = run_duel(&pa, &pb, false);
        gate_rx.recv().unwrap();
        r
    });
    // Replace A with different content (different length) mid-run.
    std::fs::write(
        &a,
        "<datafile><header><name>Test System</name></header></datafile>",
    )
    .unwrap();
    gate_tx.send(()).unwrap();
    wait(&mut panel);
    assert!(
        matches!(&panel.outcome, Some(Outcome::Stale(m)) if m.contains("DAT A")),
        "a replaced source must never show its old result"
    );
}

#[test]
fn selection_change_supersedes_and_a_second_comparison_waits() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(&dir, "a.dat", "Test System", "");
    let b = dat(&dir, "b.dat", "Test System", "");
    let c = dat(&dir, "c.dat", "Test System", "");
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let (pa, pb) = (a.clone(), b.clone());
    let mut panel = DatDuelPanel::default();
    panel.select(0, a);
    panel.select(1, b);
    panel.start_with(move || {
        gate_rx.recv().unwrap();
        run_duel(&pa, &pb, false)
    });
    assert!(panel.is_busy());
    panel.select(1, c.clone()); // supersede
    // Only one active comparison: a second start is ignored while the old
    // worker (which cannot be cancelled) is still running.
    let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = started.clone();
    panel.start_with(move || {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        WorkerResult::Failed("must not run".into())
    });
    gate_tx.send(()).unwrap();
    wait(&mut panel);
    assert!(!started.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        panel.outcome.is_none(),
        "a superseded result is never shown"
    );
}

#[test]
fn same_file_twice_is_rejected_before_any_work() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(&dir, "a.dat", "Test System", "");
    let mut panel = DatDuelPanel::default();
    panel.select(0, a.clone());
    panel.select(1, a);
    panel.start_with(|| WorkerResult::Failed("must not run".into()));
    assert!(!panel.is_busy());
    assert!(panel.validation.is_some());
}

#[test]
fn unreadable_dat_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let good = dat(&dir, "g.dat", "S", "");
    let bad = dir.path().join("bad.dat");
    std::fs::write(
        &bad,
        "<datafile><header><name>S</name></header><game name=\"x\"",
    )
    .unwrap();
    let panel = compare_via_panel(&good, &bad);
    assert!(matches!(&panel.outcome, Some(Outcome::Failed(m)) if m.contains("DAT B")));
}

#[test]
fn empty_catalogues_say_so_and_do_not_claim_equality() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(&dir, "a.dat", "Test System", "");
    let b = dat(&dir, "b.dat", "Test System", "");
    let mut panel = compare_via_panel(&a, &b);
    let ctx = egui::Context::default();
    let out = frame(&ctx, &mut panel, vec![]);
    let text = text_of(&out);
    assert!(text.contains("nothing to compare"), "{text}");
    assert!(!text.contains("No differences were found"));
}

#[test]
fn truncated_report_is_labelled_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(r#"<game name="A"><rom name="a.bin" size="4" sha1="{S1}"/></game>"#),
    );
    let b = dat(&dir, "b.dat", "Test System", "");
    let mut panel = compare_via_panel(&a, &b);
    if let Some(Outcome::Report(s)) = &mut panel.outcome {
        s.report.truncated_sections.push("only_in_a");
    }
    let ctx = egui::Context::default();
    let text = text_of(&frame(&ctx, &mut panel, vec![]));
    assert!(text.contains("Incomplete listing"), "{text}");
    assert!(!text.contains("No differences were found"));
}

#[test]
fn filters_and_paging_bound_the_rows_drawn() {
    let dir = tempfile::tempdir().unwrap();
    let mut games = String::new();
    for i in 0..60 {
        games.push_str(&format!(
            r#"<game name="Game {i:03}"><rom name="r{i:03}.bin" size="{s}" sha1="{h:040x}"/></game>"#,
            s = 100 + i,
            h = i as u128 + 1
        ));
    }
    let a = dat(&dir, "a.dat", "Test System", &games);
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        &format!(r#"<game name="Zed"><rom name="z.bin" size="1" sha1="{S3}"/></game>"#),
    );
    let mut panel = compare_via_panel(&a, &b);
    let total = panel.visible_rows(shown(&panel)).len();
    assert_eq!(total, 61, "60 only in A plus the one only in B");
    let ctx = egui::Context::default();
    let text = text_of(&frame(&ctx, &mut panel, vec![]));
    assert!(text.contains("Page 1 of 3"), "{text}");
    // Unverified absence must never look like a confirmed difference.
    assert!(!text.contains("[Verified"));
    assert!(text.contains("Game 000") && !text.contains("Game 059"));
    let drawn = text.matches("[No counterpart]").count();
    assert!((1..=PAGE_SIZE).contains(&drawn), "{drawn} rows drawn");
    // The rest of the page is reachable by scrolling the bounded list.
    let out = frame(&ctx, &mut panel, vec![]);
    let at = text_pos(&out, "Game 000").unwrap();
    let mut seen_late = false;
    for _ in 0..12 {
        let out = frame(
            &ctx,
            &mut panel,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -120.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: Default::default(),
                },
            ],
        );
        let t = text_of(&out);
        seen_late |= t.contains("Game 020") || t.contains("Game 024");
        assert!(
            !t.contains("Game 025"),
            "paging must stop at {PAGE_SIZE} rows"
        );
    }
    assert!(
        seen_late,
        "rows later on the page must be reachable by scrolling"
    );

    panel.set_filters(None, None, "game 05");
    assert_eq!(panel.visible_rows(shown(&panel)).len(), 10);
    panel.set_filters(Some(Section::OnlyInB), None, "");
    assert_eq!(panel.visible_rows(shown(&panel)).len(), 1);
    panel.set_filters(None, Some(Trust::Verified), "");
    assert!(panel.visible_rows(shown(&panel)).is_empty());
}

#[test]
fn row_expands_to_show_both_catalogues_hashes_sizes_and_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(
            r#"<game name="Alpha"><rom name="old.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>"#
        ),
    );
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        &format!(
            r#"<game name="Alpha"><rom name="new.bin" size="4" crc="aaaaaaaa" sha1="{S1}"/></game>"#
        ),
    );
    let mut panel = compare_via_panel(&a, &b);
    let ctx = egui::Context::default();
    let out = frame(&ctx, &mut panel, vec![]);
    assert!(!text_of(&out).contains(S1), "details start collapsed");
    let pos = text_pos(&out, "[Verified").expect("row header drawn at 1024x600");
    assert!(
        pos.y < 600.0,
        "first row must be reachable without scrolling the page: {pos:?}"
    );
    click(&ctx, &mut panel, pos);
    let text = text_of(&frame(&ctx, &mut panel, vec![]));
    assert!(text.contains(S1), "{text}");
    assert!(
        text.contains("A: Alpha / old.bin") && text.contains("B: Alpha / new.bin"),
        "{text}"
    );
    assert!(text.contains("4 bytes"));
    assert!(text.contains("CRC32 aaaaaaaa"));
}

#[test]
fn copied_report_is_bounded_and_marked() {
    let dir = tempfile::tempdir().unwrap();
    let mut games = String::new();
    for i in 0..1500 {
        games.push_str(&format!(
            r#"<game name="Game {i:05}"><rom name="a-long-rom-name-{i:05}.bin" size="{s}" crc="{c:08x}" sha1="{h:040x}"/></game>"#,
            s = 100 + i, c = i, h = i as u128 + 1
        ));
    }
    let a = dat(&dir, "a.dat", "Test System", &games);
    let b = dat(
        &dir,
        "b.dat",
        "Test System",
        &format!(r#"<game name="Zed"><rom name="z.bin" size="1" sha1="{S3}"/></game>"#),
    );
    let r = report(&a, &b);
    assert_eq!(r.only_in_a.len(), 1500);
    let (text, note) = bounded_json(&r).unwrap();
    assert!(text.len() <= COPY_BYTE_CAP, "{}", text.len());
    assert!(note.contains("bounded"), "{note}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["only_in_a"].as_array().unwrap().len() <= COPY_ITEMS_START);
    assert!(
        !v["copy_bounds"]["sections_cut"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Totals stay complete.
    assert_eq!(v["summary"]["only_in_a_no_counterpart"], 1500);
}

#[test]
fn comparison_never_alters_either_catalogue_or_creates_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = dat(
        &dir,
        "a.dat",
        "Test System",
        &format!(r#"<game name="A"><rom name="a.bin" size="4" sha1="{S1}"/></game>"#),
    );
    let b = dat(&dir, "b.dat", "Test System", "");
    let before = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let mut panel = compare_via_panel(&a, &b);
    let ctx = egui::Context::default();
    frame(&ctx, &mut panel, vec![]);
    assert_eq!(
        before,
        (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap())
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn the_panel_is_labelled_apart_from_refresh_impact() {
    let mut panel = DatDuelPanel::default();
    let ctx = egui::Context::default();
    let text = text_of(&frame(&ctx, &mut panel, vec![]));
    assert!(text.contains("Choose DAT A") && text.contains("Choose DAT B"));
    assert!(text.contains("Refresh impact"));
    assert!(text.contains("Read-only"));
}
