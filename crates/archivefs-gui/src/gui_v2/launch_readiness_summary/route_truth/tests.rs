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
    let mut cache = RouteCache::with_assessor(counting).with_revalidate(Duration::ZERO);
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
    // The worker's stamp look notices it; the answer is then recomputed.
    std::fs::write(&rom, b"changed contents").unwrap();
    let start = Instant::now();
    while RUNS.load(Ordering::SeqCst) - base < 2 {
        let _ = cache.poll(1, &rom, &a, None);
        assert!(start.elapsed() < Duration::from_secs(10), "no recheck");
        std::thread::sleep(Duration::from_millis(5));
    }
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
    let mut cache =
        RouteCache::with_assessor(slow_by_key).with_revalidate(Duration::from_secs(3600));
    assert_eq!(cache.poll(1, &rom, &a, None), Answer::Pending);
    // Selection moves on before A's worker returns: B must never read as
    // A's (Constructible) answer.
    assert_eq!(cache.poll(1, &rom, &b, None), Answer::Pending);
    let verdict = wait_known(&mut cache, 1, &rom, &b);
    assert!(matches!(verdict, RouteVerdict::Refused(_)));
}

fn warned(adapter: &'static str, path: &Path) -> LaunchReadinessInput {
    let mut candidate = candidate(adapter, path, LaunchContainerKind::PlainFile);
    candidate.warnings = vec![archivefs_core::launch::LaunchWarning::new(
        archivefs_core::launch::LaunchWarningKind::FirmwarePresentUnverified,
        "firmware is present but not verified",
    )];
    input("k", vec![candidate])
}

#[test]
fn a_warning_the_builder_refuses_blocks_with_the_warning_named() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.gb");
    std::fs::write(&rom, b"x").unwrap();
    // SameBoy's request builder refuses any warned candidate.
    let input = warned("sameboy", &rom);
    let mut summary = ready_summary(&input);
    assert_eq!(
        summary.status,
        ReadinessPresentationState::ReadyWithWarnings
    );
    apply(&mut summary, &assess(&input));
    assert_eq!(summary.status, ReadinessPresentationState::Blocked);
    assert!(summary.headline.contains("held back by a warning"));
    assert!(
        summary
            .explanation
            .contains("firmware is present but not verified")
    );
    assert!(!summary.headline.contains("No Play route"));
    assert_ne!(summary.primary_action, Some(ReadinessAction::Play));
}

#[test]
fn a_constructible_route_keeps_its_warning_and_play() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.gb");
    std::fs::write(&rom, b"x").unwrap();
    let input = warned("sameboy", &rom);
    let mut summary = ready_summary(&input);
    let before = summary.clone();
    // Builders that accept ReadyWithWarnings (XRoar, Azahar, shadPS4) answer
    // Constructible: the warning stays on the card and Play stays offered.
    apply(&mut summary, &RouteVerdict::Constructible);
    assert_eq!(summary, before);
    assert_eq!(
        summary.status,
        ReadinessPresentationState::ReadyWithWarnings
    );
    assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
}

#[test]
fn adapters_that_accept_warnings_are_not_blamed_on_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.3dsx");
    std::fs::write(&rom, b"x").unwrap();
    for adapter in ["azahar", "vita3k", "xroar", "shadps4"] {
        let input = warned(adapter, &rom);
        let mut summary = ready_summary(&input);
        apply(&mut summary, &assess(&input));
        assert!(
            !summary.headline.contains("held back by a warning"),
            "{adapter}: {}",
            summary.headline
        );
    }
}

static CHECKS: AtomicUsize = AtomicUsize::new(0);
fn checking_then_constructible(_: &LaunchReadinessInput) -> RouteVerdict {
    if CHECKS.fetch_add(1, Ordering::SeqCst) == 0 {
        RouteVerdict::Refused(RouteRefusal {
            status: ReadinessPresentationState::Checking,
            headline: "c".into(),
            explanation: "c".into(),
            action: ReadinessAction::RecheckReadiness,
        })
    } else {
        RouteVerdict::Constructible
    }
}

#[test]
fn a_still_checking_answer_is_asked_again_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.nes");
    std::fs::write(&rom, b"x").unwrap();
    let a = input(
        "a",
        vec![candidate("mesen", &rom, LaunchContainerKind::PlainFile)],
    );
    let mut cache =
        RouteCache::with_assessor(checking_then_constructible).with_revalidate(Duration::ZERO);
    let start = Instant::now();
    loop {
        if cache.poll(1, &rom, &a, None) == Answer::Known(RouteVerdict::Constructible) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "stuck on Checking"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn panicking(_: &LaunchReadinessInput) -> RouteVerdict {
    panic!("simulated worker failure");
}

#[test]
fn a_panicking_worker_does_not_leave_the_game_checking_for_ever() {
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.nes");
    std::fs::write(&rom, b"x").unwrap();
    let a = input(
        "a",
        vec![candidate("mesen", &rom, LaunchContainerKind::PlainFile)],
    );
    let mut cache = RouteCache::with_assessor(panicking).with_revalidate(Duration::from_secs(3600));
    let RouteVerdict::Refused(refusal) = wait_known(&mut cache, 1, &rom, &a) else {
        panic!("a failed check must not read as Constructible");
    };
    assert_eq!(refusal.status, ReadinessPresentationState::Blocked);
}

#[test]
fn the_authoritys_own_reason_wins_over_a_warning_or_route_guess() {
    use crate::launch_readiness_page::{GamerBlocker, GamerBlockerKind};
    let dir = tempfile::tempdir().unwrap();
    let rom = dir.path().join("game.gb");
    std::fs::write(&rom, b"x").unwrap();
    let input = warned("sameboy", &rom);
    let LaunchReadinessInput::Plan { plan, .. } = &input else {
        unreachable!()
    };
    let refusal = refusal_for(
        plan,
        &plan.candidates[0],
        GamerBlocker {
            kind: GamerBlockerKind::NoSafeEmulator,
            emulator: None,
            detail: "Bubblewrap is missing".into(),
        },
    );
    assert!(refusal.explanation.contains("Bubblewrap is missing"));
    assert!(!refusal.headline.contains("warning"));
}
