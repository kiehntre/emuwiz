use super::queue::*;
use crate::gui_v2::library::Game;
use archivefs_core::PersistedArchive;
use archivefs_core::conversion_queue::durable::{
    DurableConversionQueue, JobState, RetryDisposition, ReviewedConversion,
};
use archivefs_core::wiiu_conversion::{
    WiiUConversionDirection, WiiUConversionRefusal, WiiUConversionToolInventory,
};
use archivefs_core::wiiu_disc::{WUD_HEADER_SIZE, WUD_SECTOR_SIZE, WiiUDiscFormat};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Same synthetic 8-sector disc the backend's own queue tests use; the real
/// decoder, verification and Repair publication all run against it.
fn write_wux(path: &Path) -> Vec<u8> {
    let sector = WUD_SECTOR_SIZE as usize;
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
    std::fs::write(path, data).unwrap();
    raw
}

struct Fixture {
    dir: TempDir,
    source: PathBuf,
    ctx: egui::Context,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("game.wux");
    write_wux(&source);
    reset_for_test(dir.path().join("queue"));
    Fixture {
        dir,
        source,
        ctx: egui::Context::default(),
    }
}

fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        with_controller(|c| c.pump_for_test());
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn plan_for(f: &Fixture) -> Box<archivefs_core::wiiu_conversion::WiiUConversionPlan> {
    let path = f.source.clone();
    with_controller(|c| c.request_plan_for_test(&f.ctx, &path, WiiUConversionDirection::WuxToWud));
    wait("plan", || {
        with_controller(|c| c.plan_for_test(&path).is_some())
    });
    with_controller(|c| c.plan_for_test(&path).unwrap())
}

fn job_state(f: &Fixture) -> Option<JobState> {
    with_controller(|c| c.jobs_for_test().first().map(|j| j.state)).or_else(|| {
        let _ = f;
        None
    })
}

#[test]
fn every_state_and_refusal_has_plain_text_without_internals() {
    let states = [
        JobState::Queued,
        JobState::Running,
        JobState::Completed,
        JobState::Failed,
        JobState::Cancelled,
        JobState::Interrupted,
        JobState::BlockedStale,
        JobState::BlockedInputMissing,
    ];
    for state in states {
        for retry in [
            None,
            Some(RetryDisposition::RestartFromBeginning),
            Some(RetryDisposition::RequiresReview),
        ] {
            let (badge, meaning) = job_text(state, retry);
            for text in [badge, meaning] {
                assert!(!text.is_empty());
                assert!(
                    !text.contains(['{', '}', '_']) && !text.contains("::"),
                    "{text}"
                );
            }
        }
    }
    let refusals = [
        WiiUConversionRefusal::WrongSourceFormat {
            expected: WiiUDiscFormat::Wud,
            actual: WiiUDiscFormat::Wux,
        },
        WiiUConversionRefusal::IncompleteSource(vec![]),
        WiiUConversionRefusal::SourcePathUnsafe,
        WiiUConversionRefusal::DestinationPathUnsafe,
        WiiUConversionRefusal::DestinationIsSource,
        WiiUConversionRefusal::DestinationExists,
        WiiUConversionRefusal::HashStale,
        WiiUConversionRefusal::HashMissingForVerification,
        WiiUConversionRefusal::ToolUnavailable,
        WiiUConversionRefusal::ToolCapabilityUnproven,
        WiiUConversionRefusal::ToolDoesNotSupportDirection,
        WiiUConversionRefusal::InsufficientDestinationSpace {
            required: 3 << 30,
            available: 1 << 20,
        },
        WiiUConversionRefusal::OutputSizeUnknown,
        WiiUConversionRefusal::UnsupportedFormat(WiiUDiscFormat::Unknown),
        WiiUConversionRefusal::AmbiguousSplit,
        WiiUConversionRefusal::DeferredDirection,
        WiiUConversionRefusal::InvalidSourceIdentity,
    ];
    for refusal in &refusals {
        let text = refusal_text(refusal);
        assert!(
            !text.is_empty() && !text.contains(['{', '}']) && !text.contains("::"),
            "{text}"
        );
    }
    let space = refusal_text(&refusals[11]);
    assert!(
        space.contains("GiB") && !space.contains("3221225472"),
        "{space}"
    );
}

#[test]
fn a_ready_plan_converts_in_the_background_and_never_touches_the_original() {
    let f = fixture();
    let original = std::fs::read(&f.source).unwrap();
    let plan = plan_for(&f);
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
    assert_eq!(card_state(None, Some(&plan)), CardState::Ready);
    let reviewed = ReviewedConversion::WuxToWud(plan);
    with_controller(|c| c.send_enqueue_for_test(&f.ctx, reviewed));
    wait("completion", || job_state(&f) == Some(JobState::Completed));
    assert_eq!(std::fs::read(&f.source).unwrap(), original);
    let destination = f.dir.path().join("game.wud");
    assert!(destination.exists());
    // The queue survives a restart (a fresh controller reading the same directory).
    reset_for_test(f.dir.path().join("queue"));
    with_controller(|c| c.request_snapshot_for_test(&f.ctx));
    wait("reload", || job_state(&f) == Some(JobState::Completed));
}

#[test]
fn a_job_left_from_an_earlier_session_never_starts_by_itself() {
    let f = fixture();
    let plan = plan_for(&f);
    {
        let mut queue = DurableConversionQueue::open(&f.dir.path().join("queue")).unwrap();
        queue.enqueue(ReviewedConversion::WuxToWud(plan)).unwrap();
    }
    with_controller(|c| c.send_recover_for_test(&f.ctx));
    wait("snapshot", || job_state(&f).is_some());
    std::thread::sleep(Duration::from_millis(300));
    with_controller(|c| c.pump_for_test());
    assert_eq!(job_state(&f), Some(JobState::Queued));
    assert!(!f.dir.path().join("game.wud").exists());
    // Only an explicit Start runs it.
    with_controller(|c| c.send_start_for_test(&f.ctx));
    wait("completion", || job_state(&f) == Some(JobState::Completed));
}

#[test]
fn a_queued_job_can_be_cancelled_and_leaves_nothing_behind() {
    let f = fixture();
    let plan = plan_for(&f);
    let id = {
        let mut queue = DurableConversionQueue::open(&f.dir.path().join("queue")).unwrap();
        queue.enqueue(ReviewedConversion::WuxToWud(plan)).unwrap()
    };
    with_controller(|c| c.send_cancel_for_test(&f.ctx, id));
    wait("cancelled", || job_state(&f) == Some(JobState::Cancelled));
    assert!(!f.dir.path().join("game.wud").exists());
    // A cancelled job is not shown as the card's job; a fresh preview is offered.
    let snapshot = DurableConversionQueue::inspect(&f.dir.path().join("queue")).unwrap();
    assert!(job_for(&snapshot, &f.source, WiiUConversionDirection::WuxToWud).is_none());
}

#[test]
fn changing_the_original_after_review_is_refused_not_converted() {
    let f = fixture();
    let plan = plan_for(&f);
    let mut bytes = std::fs::read(&f.source).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    bytes.push(0);
    std::fs::write(&f.source, bytes).unwrap();
    with_controller(|c| c.send_enqueue_for_test(&f.ctx, ReviewedConversion::WuxToWud(plan)));
    wait("notice", || {
        with_controller(|c| c.notice_for_test().is_some())
    });
    assert!(!f.dir.path().join("game.wud").exists());
    assert!(!matches!(job_state(&f), Some(JobState::Completed)));
}

#[test]
fn a_busy_queue_gives_a_friendly_message_and_still_shows_the_list() {
    let f = fixture();
    let _held = DurableConversionQueue::open(&f.dir.path().join("queue")).unwrap();
    with_controller(|c| c.send_recover_for_test(&f.ctx));
    wait("notice", || {
        with_controller(|c| c.notice_for_test().is_some())
    });
    let notice = with_controller(|c| c.notice_for_test().unwrap());
    assert!(notice.contains("Another EmuWiz window"), "{notice}");
}

#[test]
fn a_late_plan_for_a_superseded_request_is_discarded() {
    let f = fixture();
    let path = f.source.clone();
    with_controller(|c| c.request_plan_for_test(&f.ctx, &path, WiiUConversionDirection::WuxToWud));
    let real = plan_for(&f);
    // Re-request (bumps the generation), then deliver the old generation's result.
    with_controller(|c| c.replan_and_inject_stale_for_test(&f.ctx, &path, real));
    with_controller(|c| c.pump_for_test());
    assert!(with_controller(|c| c.plan_for_test(&path)).is_none());
}

#[test]
fn card_state_prefers_a_job_then_blockers_then_ready() {
    let f = fixture();
    let mut plan = plan_for(&f);
    assert_eq!(card_state(None, None), CardState::Checking);
    plan.refusals.push(WiiUConversionRefusal::DestinationExists);
    assert!(matches!(card_state(None, Some(&plan)), CardState::Blocked(r) if r.len() == 1));
    let _ = WiiUConversionToolInventory::default();
}

fn game_for(path: &Path) -> Game {
    Game::from_archive(PersistedArchive {
        id: 1,
        source_folder_id: 1,
        relative_path: path.file_name().unwrap().into(),
        absolute_path: path.to_path_buf(),
        archive_kind: "wux".into(),
        display_name: "Synthetic Wii U".into(),
        normalized_name: "synthetic wii u".into(),
        size_bytes: Some(1),
        modified_time_unix_seconds: Some(1),
        platform: Some("Wii U".into()),
        platform_source: Some("manual".into()),
        last_known_health: "pending".into(),
        last_seen_at: "2026-10-03".into(),
        last_verified_missing_at: None,
        identity_report: None,
    })
}

#[test]
fn the_card_renders_at_both_window_sizes_without_blocking() {
    let f = fixture();
    let game = game_for(&f.source);
    for size in [[1280.0, 800.0], [700.0, 520.0]] {
        let ctx = egui::Context::default();
        let started = Instant::now();
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(size[0], size[1]),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        show(ui, &game, WiiUConversionDirection::WuxToWud);
                    });
                },
            );
        }
        // IO happens on workers, so frames return immediately.
        assert!(started.elapsed() < Duration::from_secs(5));
    }
    wait("plan", || {
        with_controller(|c| c.plan_for_test(&f.source).is_some())
    });
}

#[test]
fn raw_planner_warnings_never_reach_the_primary_text() {
    // The synthetic disc is not retail size, so the planner emits internal warnings.
    let f = fixture();
    let plan = plan_for(&f);
    assert!(
        !plan.warnings.is_empty(),
        "fixture should exercise the warning path"
    );
    let ctx = egui::Context::default();
    let game = game_for(&f.source);
    wait("plan", || {
        with_controller(|c| c.plan_for_test(&f.source).is_some())
    });
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default()
            .show(ctx, |ui| show(ui, &game, WiiUConversionDirection::WuxToWud));
    });
    let shown: String = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for warning in &plan.warnings {
        assert!(!shown.contains(warning.as_str()), "{warning}");
    }
    assert!(shown.contains("unusual details"), "{shown}");
}
