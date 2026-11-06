use super::*;
use crate::gui_v2::launch_readiness_summary::{GameReadinessSummary, ReadinessFreshness, project};
use archivefs_core::launch::{
    CandidatePreference, FirmwareReadiness, LaunchContentRef, LaunchReadiness,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn candidate(
    adapter: &'static str,
    path: &Path,
    container: LaunchContainerKind,
) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: adapter,
            profile_id: "p".into(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: None,
            container: Some(container),
            resolved_path: Some(path.to_path_buf()),
            requires_mount: false,
            provenance: "test".into(),
        },
        firmware: FirmwareReadiness::NotRequired,
        blockers: vec![],
        warnings: vec![],
        readiness: LaunchReadiness::Ready,
        preference: CandidatePreference::SoleEligible,
    }
}

fn input(key: &str, candidates: Vec<LaunchCandidate>) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan: LaunchPlan {
            platform_id: Some("NES".into()),
            game_key: Some(key.into()),
            candidates,
            summary: Default::default(),
            media_topology: None,
        },
        retroarch: None,
        retroarch_scanned: true,
        standalone_scans_complete: true,
        dolphin: None,
        pcsx2: None,
        duckstation: None,
        ppsspp: None,
        rpcs3: None,
        xemu: None,
        xenia: None,
        melonds: None,
        cemu: None,
        flycast: None,
        xroar: None,
        azahar: None,
        vita3k: None,
        vice: None,
        amiga_whdload: None,
        #[cfg(target_os = "linux")]
        shadps4: None,
    }
}

fn ready_summary(input: &LaunchReadinessInput) -> GameReadinessSummary {
    project(input, ReadinessFreshness::Current)
}

fn wait_known(
    cache: &mut RouteCache,
    id: i64,
    path: &Path,
    input: &LaunchReadinessInput,
) -> RouteVerdict {
    let start = Instant::now();
    loop {
        if let Answer::Known(verdict) = cache.poll(id, path, input, None) {
            return verdict;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no answer");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_unwired_adapter_is_not_ready_even_though_the_plan_is() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.nes");
    std::fs::write(&rom, b"x").unwrap();
    for adapter in ["mesen", "snes9x", "hatari", "fuse"] {
        let input = input(
            "k",
            vec![candidate(adapter, &rom, LaunchContainerKind::PlainFile)],
        );
        let mut summary = ready_summary(&input);
        assert_eq!(
            summary.status,
            ReadinessPresentationState::Ready,
            "{adapter}"
        );
        apply(&mut summary, &assess(&input));
        assert_eq!(
            summary.status,
            ReadinessPresentationState::Blocked,
            "{adapter}"
        );
        assert!(
            summary.headline.starts_with("No Play route for"),
            "{adapter}"
        );
        assert_ne!(summary.primary_action, Some(ReadinessAction::Play));
    }
}

#[test]
fn unsupported_media_is_told_apart_from_a_missing_route() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.zip");
    std::fs::write(&rom, b"x").unwrap();
    let input = input(
        "k",
        vec![candidate("mesen", &rom, LaunchContainerKind::Archive)],
    );
    let mut summary = ready_summary(&input);
    apply(&mut summary, &assess(&input));
    assert!(summary.headline.contains("cannot start this kind of file"));
    assert!(summary.explanation.contains("inside an archive"));
}

#[test]
fn panel_routed_adapters_keep_the_plans_answer() {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("game.iso");
    std::fs::write(&iso, b"x").unwrap();
    let input = input(
        "k",
        vec![candidate("tsugaru", &iso, LaunchContainerKind::PlainFile)],
    );
    assert_eq!(assess(&input), RouteVerdict::Constructible);
}

#[test]
fn a_refusal_never_touches_a_summary_that_was_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.nes");
    std::fs::write(&rom, b"x").unwrap();
    let input = input(
        "k",
        vec![candidate("mesen", &rom, LaunchContainerKind::PlainFile)],
    );
    let mut summary = ready_summary(&input);
    summary.status = ReadinessPresentationState::NeedsFirmware;
    let before = summary.clone();
    apply(&mut summary, &assess(&input));
    assert_eq!(summary, before);
}

#[test]
fn pending_reads_as_checking_and_offers_no_play() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.nes");
    std::fs::write(&rom, b"x").unwrap();
    let input = input(
        "k",
        vec![candidate("mesen", &rom, LaunchContainerKind::PlainFile)],
    );
    let mut summary = ready_summary(&input);
    apply_pending(&mut summary);
    assert_eq!(summary.status, ReadinessPresentationState::Checking);
    assert_eq!(summary.primary_action, None);
}

static RUNS: AtomicUsize = AtomicUsize::new(0);
fn counting(input: &LaunchReadinessInput) -> RouteVerdict {
    RUNS.fetch_add(1, Ordering::SeqCst);
    slow_by_key(input)
}

/// Same answers as `counting` without touching the shared counter, so tests
/// running in parallel cannot disturb each other's counts.
fn slow_by_key(input: &LaunchReadinessInput) -> RouteVerdict {
    // Slow enough that repeated frames overlap the worker.
    std::thread::sleep(Duration::from_millis(30));
    let LaunchReadinessInput::Plan { plan, .. } = input else {
        unreachable!()
    };
    if plan.game_key.as_deref() == Some("a") {
        RouteVerdict::Constructible
    } else {
        assess(input)
    }
}

#[test]
fn repeated_frames_run_the_expensive_check_once_and_changes_rerun_it() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.nes");
    std::fs::write(&rom, b"x").unwrap();
    let a = input(
        "a",
        vec![candidate("mesen", &rom, LaunchContainerKind::PlainFile)],
    );
    let mut cache = RouteCache::with_assessor(counting);
    let base = RUNS.load(Ordering::SeqCst);
    // 200 repaints while the worker runs, then 200 after it finished.
    for _ in 0..200 {
        let _ = cache.poll(1, &rom, &a, None);
    }
    assert_eq!(
        wait_known(&mut cache, 1, &rom, &a),
        RouteVerdict::Constructible
    );
    for _ in 0..200 {
        assert!(matches!(cache.poll(1, &rom, &a, None), Answer::Known(_)));
    }
    assert_eq!(RUNS.load(Ordering::SeqCst) - base, 1);

    // A changed media file is a new key: Checking, then a fresh run.
    std::fs::write(&rom, b"changed contents").unwrap();
    assert_eq!(cache.poll(1, &rom, &a, None), Answer::Pending);
    wait_known(&mut cache, 1, &rom, &a);
    assert_eq!(RUNS.load(Ordering::SeqCst) - base, 2);

    // Another game, and a changed plan, each invalidate.
    assert_eq!(cache.poll(2, &rom, &a, None), Answer::Pending);
    wait_known(&mut cache, 2, &rom, &a);
    let b = input(
        "a",
        vec![candidate("mesen", &rom, LaunchContainerKind::Archive)],
    );
    assert_eq!(cache.poll(2, &rom, &b, None), Answer::Pending);
    wait_known(&mut cache, 2, &rom, &b);
    assert_eq!(RUNS.load(Ordering::SeqCst) - base, 4);
}

#[test]
fn an_outdated_worker_response_is_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.nes");
    std::fs::write(&rom, b"x").unwrap();
    let a = input(
        "a",
        vec![candidate("mesen", &rom, LaunchContainerKind::PlainFile)],
    );
    let b = input(
        "b",
        vec![candidate("mesen", &rom, LaunchContainerKind::PlainFile)],
    );
    let mut cache = RouteCache::with_assessor(slow_by_key);
    assert_eq!(cache.poll(1, &rom, &a, None), Answer::Pending);
    // Selection moves on before A's worker returns: B must never read as
    // A's (Constructible) answer.
    assert_eq!(cache.poll(1, &rom, &b, None), Answer::Pending);
    let verdict = wait_known(&mut cache, 1, &rom, &b);
    assert!(matches!(verdict, RouteVerdict::Refused(_)));
}
