use super::*;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn reviewed(root: &Path, name: &str) -> ReviewedConversion {
    use archivefs_core::wiiu_disc::{WUD_HEADER_SIZE, WUD_SECTOR_SIZE};
    let sector = WUD_SECTOR_SIZE as usize;
    let source = root.join(format!("{name}.wux"));
    let mut raw = vec![0; 8 * sector];
    raw[..10].copy_from_slice(b"WUP-P-TEST");
    raw[0x10000..0x10004].copy_from_slice(&0xcc54_9eb9_u32.to_be_bytes());
    raw[0x10005] = 1;
    raw[0x18000..WUD_HEADER_SIZE as usize].fill(0xa5);
    let mut data = vec![0; 5 * sector];
    data[..4].copy_from_slice(b"WUX0");
    data[4..8].copy_from_slice(&0x1099_d02e_u32.to_le_bytes());
    data[8..12].copy_from_slice(&WUD_SECTOR_SIZE.to_le_bytes());
    data[16..24].copy_from_slice(&(raw.len() as u64).to_le_bytes());
    for (i, block) in [0_u32, 1, 2, 3, 1, 1, 1, 1].into_iter().enumerate() {
        data[32 + i * 4..36 + i * 4].copy_from_slice(&block.to_le_bytes());
    }
    data[sector..].copy_from_slice(&raw[..4 * sector]);
    std::fs::write(&source, data).unwrap();
    let plan = plan_wiiu_conversion(&WiiUConversionRequest {
        source,
        destination: root.join(format!("{name}.wud")),
        direction: WiiUConversionDirection::WuxToWud,
        source_identity: WiiUConversionIdentity::HashMissing,
        available_free_space: None,
        tools: WiiUConversionToolInventory::default(),
    });
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    ReviewedConversion::WuxToWud(Box::new(plan))
}

fn setup() -> (TempDir, Controller, egui::Context) {
    let dir = tempfile::tempdir().unwrap();
    let mut q = DurableConversionQueue::open(&dir.path().join("queue")).unwrap();
    for name in ["first", "second"] {
        q.enqueue(reviewed(dir.path(), name)).unwrap();
    }
    let mut c = Controller::new(Some(dir.path().join("queue")));
    c.snapshot = Some(Ok(q.snapshot().clone()));
    c.snapshot_requested = true;
    c.recovered = true;
    (dir, c, egui::Context::default())
}

fn wait_idle(c: &mut Controller) {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        c.pump();
        if !c.inbox.lock().unwrap().alive {
            c.pump();
            return;
        }
        assert!(Instant::now() < until, "worker did not settle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn frame(
    c: &mut Controller,
    ctx: &egui::Context,
    history: bool,
    source: Option<&Path>,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if history {
                    overview::history(ui, c, source);
                } else {
                    overview::show(ui, c);
                }
            });
        },
    )
}
fn texts(output: &egui::FullOutput) -> String {
    output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t.galley.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn click(c: &mut Controller, ctx: &egui::Context, label: &str) {
    let mut output = frame(c, ctx, false, None, vec![]);
    for _ in 0..2 {
        output = frame(c, ctx, false, None, vec![]);
    }
    let pos = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == label => Some(t.pos + t.galley.size() / 2.0),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing button {label}: {}", texts(&output)));
    for pressed in [true, false] {
        frame(
            c,
            ctx,
            false,
            None,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}

#[test]
fn converter_shows_pending_formats_space_and_waits_for_start() {
    let (dir, mut c, ctx) = setup();
    let queue_before = std::fs::read(dir.path().join("queue/queue.json")).unwrap();
    let text = texts(&frame(&mut c, &ctx, false, None, vec![]));
    for expected in [
        "Pending conversions",
        "WUD",
        "WUX",
        "Original size:",
        "New file size:",
        "Staging space:",
        "Free at review:",
        "Start queued conversions",
        "Open conversion records in History",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(!text.contains("smaller file"));
    assert!(!text.contains("Undo available"));
    assert_eq!(
        queue_before,
        std::fs::read(dir.path().join("queue/queue.json")).unwrap()
    );
    assert!(!dir.path().join("first.wud").exists());
    assert!(!dir.path().join("second.wud").exists());
}

#[test]
fn converter_start_button_runs_two_synthetic_jobs_and_retains_sources() {
    let (dir, mut c, ctx) = setup();
    let originals: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|n| std::fs::read(dir.path().join(format!("{n}.wux"))).unwrap())
        .collect();
    click(&mut c, &ctx, "Start queued conversions");
    wait_idle(&mut c);
    let snapshot = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(snapshot.jobs.len(), 2);
    for (job, old) in snapshot.jobs.iter().zip(originals) {
        assert_eq!(job.state, JobState::Completed);
        assert!(job.result.is_some());
        assert_eq!(std::fs::read(job.reviewed.source()).unwrap(), old);
        assert!(job.reviewed.destination().exists());
        assert!(job.attempts[0].journal_dir.is_dir());
    }
}

#[test]
fn a_plan_admitted_after_start_does_not_join_that_run() {
    let (dir, mut c, ctx) = setup();
    let later = reviewed(dir.path(), "later");
    // Load both commands before the worker starts; admission is processed AFTER
    // Start, but BEFORE run_next. This deterministically exercises the cutoff.
    c.inbox.lock().unwrap().cmds.push_back(Cmd::Start);
    c.send(&ctx, Cmd::Enqueue(Box::new(later)));
    wait_idle(&mut c);
    let s = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(
        s.jobs
            .iter()
            .filter(|j| j.state == JobState::Completed)
            .count(),
        2
    );
    assert_eq!(s.jobs.last().unwrap().state, JobState::Queued);
    assert!(!dir.path().join("later.wud").exists());
}

#[test]
fn a_blocked_source_pauses_the_following_queued_job() {
    let (dir, mut c, ctx) = setup();
    std::fs::write(dir.path().join("first.wux"), b"changed").unwrap();
    c.send(&ctx, Cmd::Start);
    wait_idle(&mut c);
    let s = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(s.jobs[0].state, JobState::BlockedStale);
    assert_eq!(s.jobs[1].state, JobState::Queued);
    assert!(!dir.path().join("second.wud").exists());
}

#[test]
fn stale_cancel_cannot_cancel_the_next_or_external_job() {
    let (_dir, mut c, _ctx) = setup();
    {
        let mut i = c.inbox.lock().unwrap();
        i.alive = true;
        i.active_job = Some(2);
    }
    c.cancel_running(1);
    assert!(!c.cancel.load(Ordering::Acquire));
    c.cancel_running(2);
    assert!(c.cancel.load(Ordering::Acquire));
    c.cancel.store(false, Ordering::Release);
    c.inbox.lock().unwrap().active_job = None;
    c.cancel_running(2);
    assert!(!c.cancel.load(Ordering::Acquire));
}

#[test]
fn native_progress_is_visible_before_completion_without_a_success_claim() {
    let (_dir, mut c, ctx) = setup();
    c.tx.send(Msg::Progress {
        id: 1,
        progress: ProgressSnapshot {
            bytes_processed: Some(5),
            total_bytes: Some(10),
            phase: "Checking logical stream".into(),
            updated_at_unix: 1,
        },
    })
    .unwrap();
    c.pump();
    assert_eq!(
        c.snapshot.as_ref().unwrap().as_ref().unwrap().jobs[0].state,
        JobState::Running
    );
    let text = texts(&frame(&mut c, &ctx, false, None, vec![]));
    assert!(text.contains("Checking logical stream"), "{text}");
    assert!(!text.contains("Done"), "{text}");
}

fn interrupted_fixture(dir: &Path) {
    let root = dir.join("queue");
    let mut s = DurableConversionQueue::inspect(&root).unwrap();
    let stage = tempfile::Builder::new()
        .prefix(".emuwiz-conversion-queue-")
        .tempdir_in(dir)
        .unwrap()
        .keep();
    let journal = root.join("transactions/1/1");
    std::fs::create_dir_all(&journal).unwrap();
    // An incomplete publication journal makes the restart deliberately uncertain.
    std::fs::write(journal.join("interrupted-publication.json"), b"{").unwrap();
    s.jobs[0].state = JobState::Running;
    s.jobs[0].attempts.push(
        archivefs_core::conversion_queue::durable::ConversionAttempt {
            journal_dir: journal,
            staging_root: stage,
            started_at_unix: 1,
            state: JobState::Running,
        },
    );
    std::fs::write(root.join("queue.json"), serde_json::to_vec(&s).unwrap()).unwrap();
}

#[test]
fn history_inspection_does_not_recover_or_execute_an_interrupted_worker() {
    let (dir, mut c, ctx) = setup();
    interrupted_fixture(dir.path());
    c.snapshot = Some(Ok(DurableConversionQueue::inspect(
        &dir.path().join("queue"),
    )
    .unwrap()));
    let before = std::fs::read(dir.path().join("queue/queue.json")).unwrap();
    let text = texts(&frame(&mut c, &ctx, true, None, vec![]));
    assert!(text.contains("Last recorded state: running"), "{text}");
    assert!(!text.contains("Start queued conversions"));
    assert_eq!(
        before,
        std::fs::read(dir.path().join("queue/queue.json")).unwrap()
    );
    assert!(!c.inbox.lock().unwrap().alive);
}

#[test]
fn converter_recovers_interrupted_state_but_does_not_restart_it() {
    let (dir, mut c, ctx) = setup();
    interrupted_fixture(dir.path());
    c.snapshot = Some(Ok(DurableConversionQueue::inspect(
        &dir.path().join("queue"),
    )
    .unwrap()));
    c.recovered = false;
    frame(&mut c, &ctx, false, None, vec![]);
    wait_idle(&mut c);
    let s = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(s.jobs[0].state, JobState::Interrupted);
    assert_eq!(s.jobs[0].retry, Some(RetryDisposition::RequiresReview));
    assert_eq!(s.jobs[1].state, JobState::Queued);
    assert!(!dir.path().join("first.wud").exists());
    assert!(!dir.path().join("second.wud").exists());
}

#[test]
fn history_filters_by_exact_source_and_does_not_offer_execution() {
    let (dir, mut c, ctx) = setup();
    let source = dir.path().join("second.wux");
    let text = texts(&frame(&mut c, &ctx, true, Some(&source), vec![]));
    assert!(text.contains("second.wux"), "{text}");
    assert!(!text.contains("first.wux"), "{text}");
    assert!(!text.contains("Start queued conversions"));
    assert!(!text.contains("Queue retry"));
    assert!(!text.contains("Undo available"));
    assert!(text.contains("Undo is not offered"));
}

#[test]
fn cancelling_a_waiting_item_pauses_the_batch() {
    let (dir, mut c, ctx) = setup();
    c.inbox.lock().unwrap().cmds.push_back(Cmd::Start);
    c.send(&ctx, Cmd::Cancel(1));
    wait_idle(&mut c);
    let s = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(s.jobs[0].state, JobState::Cancelled);
    assert_eq!(s.jobs[1].state, JobState::Queued);
    assert!(!dir.path().join("second.wud").exists());
}

fn mark_first_failed(dir: &Path) {
    let root = dir.join("queue");
    let mut s = DurableConversionQueue::inspect(&root).unwrap();
    s.jobs[0].state = JobState::Failed;
    s.jobs[0].retry = Some(RetryDisposition::RestartFromBeginning);
    std::fs::write(root.join("queue.json"), serde_json::to_vec(&s).unwrap()).unwrap();
}

#[test]
fn retry_after_start_moves_the_same_id_outside_the_authorised_order() {
    let (dir, mut c, ctx) = setup();
    mark_first_failed(dir.path());
    c.inbox.lock().unwrap().cmds.push_back(Cmd::Start);
    c.send(&ctx, Cmd::Retry(1));
    wait_idle(&mut c);
    let s = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(s.jobs[0].state, JobState::Queued);
    assert_eq!(s.jobs[1].state, JobState::Completed);
    assert!(s.jobs[0].order > s.jobs[1].order);
    assert!(!dir.path().join("first.wud").exists());
}

#[test]
fn a_blocked_retry_pauses_the_previously_started_queue() {
    let (dir, mut c, ctx) = setup();
    mark_first_failed(dir.path());
    std::fs::write(dir.path().join("first.wux"), b"changed").unwrap();
    c.inbox.lock().unwrap().cmds.push_back(Cmd::Start);
    c.send(&ctx, Cmd::Retry(1));
    wait_idle(&mut c);
    let s = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(s.jobs[0].state, JobState::BlockedStale);
    assert_eq!(s.jobs[1].state, JobState::Queued);
    assert!(!dir.path().join("second.wud").exists());
}

#[test]
fn selected_source_preview_requires_confirmation_and_only_queues() {
    let (dir, mut c, ctx) = setup();
    let source = dir.path().join("third.wux");
    let reviewed = reviewed(dir.path(), "third");
    let ReviewedConversion::WuxToWud(plan) = reviewed else {
        unreachable!()
    };
    c.plans.insert(source.clone(), (1, Some(plan)));
    c.selected = Some((source.clone(), WiiUConversionDirection::WuxToWud));
    click(&mut c, &ctx, "Add to conversion queue…");
    assert_eq!(c.confirm.as_deref(), Some(source.as_path()));
    assert_eq!(
        DurableConversionQueue::inspect(&dir.path().join("queue"))
            .unwrap()
            .jobs
            .len(),
        2
    );
    click(&mut c, &ctx, "Add to queue");
    wait_idle(&mut c);
    let s = DurableConversionQueue::inspect(&dir.path().join("queue")).unwrap();
    assert_eq!(s.jobs.len(), 3);
    assert!(s.jobs.iter().all(|j| j.state == JobState::Queued));
    assert!(!dir.path().join("third.wud").exists());
}
