use super::*;
use archivefs_core::launch::{
    CandidatePreference, FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchContentRef,
    LaunchWarning,
};
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    dir: tempfile::TempDir,
    paths: Atari800Paths,
    game: PathBuf,
    plan: LaunchPlan,
}

fn atr_bytes() -> Vec<u8> {
    let mut bytes = vec![0x22; 92160 + 16];
    bytes[..16].fill(0);
    bytes[..2].copy_from_slice(&[0x96, 0x02]);
    bytes[2..4].copy_from_slice(&5760u16.to_le_bytes());
    bytes[4..6].copy_from_slice(&128u16.to_le_bytes());
    bytes
}

fn write_program(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn candidate(game: &Path, profile_id: &str) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: ADAPTER_KEY,
            profile_id: profile_id.into(),
            profile_path: None,
        },
        content: LaunchContentRef {
            kind: None,
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(game.to_owned()),
            requires_mount: false,
            provenance: "test".into(),
        },
        firmware: FirmwareReadiness::PresentUnverified,
        blockers: vec![],
        warnings: vec![LaunchWarning::new(
            LaunchWarningKind::FirmwarePresentUnverified,
            "required firmware is present but its contents were not verified",
        )],
        readiness: LaunchReadiness::ReadyWithWarnings,
        preference: CandidatePreference::SoleEligible,
    }
}

impl Fixture {
    /// A complete setup around a stand-in emulator that exits with `code`.
    fn new(code: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let program = root.join("atari800");
        write_program(&program, &format!("exit {code}"));
        let os = root.join("xl.rom");
        std::fs::write(&os, vec![0x31; 16384]).unwrap();
        let game = root.join("Homebrew Disk.atr");
        std::fs::write(&game, atr_bytes()).unwrap();
        let paths = Atari800Paths {
            setup_file: root.join("config/atari800_setup.json"),
            seed_directory: root.join("data/atari800"),
        };
        for edit in [
            Edit::Executable(Some(program)),
            Edit::Machine(Atari800Machine::Atari800Xl),
            Edit::Video(Atari800Video::Pal),
            Edit::BindRom(Atari800RomKind::OsXlXe, os),
            Edit::AcceptUnverified(true),
            Edit::AcknowledgeDisposable(true),
        ] {
            apply_edit(&paths, edit).unwrap();
        }
        let (_, resolution) = resolve_from(&paths).unwrap();
        assert!(resolution.eligible(), "{:?}", resolution.blockers);
        // This thread's Atari800 environment is the scratch setup above.
        TEST_PATHS.with(|slot| *slot.borrow_mut() = Some(paths.clone()));
        let report =
            crate::selected_evidence_page::gather_selected_evidence_fast(&game, Some(PLATFORM_ID))
                .unwrap();
        let CanonicalIdentityStatus::Resolved(identity) =
            archivefs_core::launch::canonical_identity_from_game_report(
                &report.game_identity_report,
            )
            .0
        else {
            panic!("disposable Atari 8-bit fixture has unresolved identity")
        };
        assert_eq!(identity.platform_id, PLATFORM_ID);
        let plan = LaunchPlan {
            platform_id: Some(identity.platform_id),
            game_key: Some(identity.game_key),
            candidates: vec![candidate(&game, &resolution.profile_id().unwrap())],
            summary: Default::default(),
            media_topology: None,
        };
        Self {
            dir,
            paths,
            game,
            plan,
        }
    }
    fn root(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }
    fn run(&self, request: Request) -> Result<Box<LaunchedAtari800>, String> {
        let base = self.root().join("run");
        std::fs::create_dir_all(&base).unwrap();
        request.execute_with(&self.paths, None, || SandboxManager::open(&base))
    }
}

fn wait(launched: &mut LaunchedAtari800) -> Option<i32> {
    for _ in 0..2000 {
        if let Some(report) = launched.process.process.poll() {
            return report.status.as_ref().ok().and_then(|status| status.code());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("stand-in emulator did not exit")
}

fn blue_input(plan: &LaunchPlan) -> LaunchReadinessInput {
    LaunchReadinessInput::Plan {
        plan: plan.clone(),
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
        shadps4: None,
    }
}

fn blue_request(plan: &LaunchPlan) -> Request {
    let GamerPlayAction::Launch(request) = gamer_play_action(&blue_input(plan)) else {
        panic!("Blue Play must offer the Atari800 launch")
    };
    let TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Atari800(request)) = *request
    else {
        panic!("not an Atari800 request")
    };
    request
}

#[test]
fn a_ready_atr_reaches_blue_play_with_every_caveat() {
    let fixture = Fixture::new(0);
    let GamerPlayAction::Launch(request) = gamer_play_action(&blue_input(&fixture.plan)) else {
        panic!("Blue Play must offer the Atari800 launch")
    };
    assert_eq!(request.adapter_name(), "Atari800");
    assert_eq!(
        request.unverified_status_label(),
        "Ready, temporary session, firmware unverified"
    );
    let warnings = request.unverified_warnings();
    assert!(warnings.iter().any(|line| line.contains("not a DAT check")));
    assert!(warnings.iter().any(|line| line.contains("cannot verify")));
    assert!(warnings.iter().any(|line| line.contains("discarded")));
}

#[test]
fn blue_play_refuses_other_formats_archives_and_unresolved_or_foreign_identity() {
    let fixture = Fixture::new(0);
    let plan = &fixture.plan;
    let mut archive = plan.clone();
    archive.candidates[0].content.container = Some(LaunchContainerKind::Archive);
    let mut mounted = plan.clone();
    mounted.candidates[0].content.requires_mount = true;
    let mut xfd = plan.clone();
    xfd.candidates[0].content.resolved_path = Some(fixture.root().join("Homebrew Disk.xfd"));
    let mut relative = plan.clone();
    relative.candidates[0].content.resolved_path = Some("Homebrew Disk.atr".into());
    let mut keyless = plan.clone();
    keyless.game_key = None;
    let mut atari5200 = plan.clone();
    atari5200.platform_id = Some("Atari5200".into());
    let mut other_warning = plan.clone();
    other_warning.candidates[0]
        .warnings
        .push(LaunchWarning::new(
            LaunchWarningKind::RetroArchSaveDirectoryUnusable,
            "unrelated",
        ));
    for refused in [
        archive,
        mounted,
        xfd,
        relative,
        keyless,
        atari5200,
        other_warning,
    ] {
        assert!(
            matches!(
                gamer_play_action(&blue_input(&refused)),
                GamerPlayAction::BlockedTyped(_)
            ),
            "{refused:?}"
        );
    }
}

#[test]
fn an_incomplete_setup_blocks_play_and_names_atari800_and_the_reason() {
    let fixture = Fixture::new(0);
    let mut blocked = fixture.plan.clone();
    blocked.candidates[0].readiness = LaunchReadiness::Blocked;
    // The planner never prefers a blocked candidate.
    blocked.candidates[0].preference = CandidatePreference::Undetermined;
    blocked.candidates[0].firmware = FirmwareReadiness::Missing;
    blocked.candidates[0].warnings.clear();
    blocked.candidates[0].blockers = vec![LaunchBlocker::new(
        LaunchBlockerKind::RequiredFirmwareMissing,
        "required firmware/BIOS is missing",
    )];
    let GamerPlayAction::BlockedTyped(blocker) = gamer_play_action(&blue_input(&blocked)) else {
        panic!("a blocked setup must never launch")
    };
    assert_eq!(blocker.kind, GamerBlockerKind::EmulatorSetupIncomplete);
    assert_eq!(blocker.heading(), "Atari800 needs setup");
    assert!(!blocker.detail.is_empty());
    // The Gold panel shows the setup's own first blocker, not a generic line.
    let offer = Request::offer(&blocked, &blocked.candidates[0], || {
        Some("Select your Atari XL/XE OS ROM".into())
    });
    assert_eq!(
        offer.err().as_deref(),
        Some("Select your Atari XL/XE OS ROM")
    );
}

#[test]
fn a_blue_request_runs_reports_exit_relaunches_and_never_writes_the_game() {
    for code in [0, 7] {
        let fixture = Fixture::new(code);
        let request = blue_request(&fixture.plan);
        let before = std::fs::read(&fixture.game).unwrap();
        for _ in 0..2 {
            let mut launched = fixture.run(request.clone()).unwrap();
            assert_eq!(wait(&mut launched), Some(code));
            assert_eq!(exit_failure(&launched).is_some(), code != 0);
            if code != 0 {
                let detail = exit_failure(&launched).unwrap();
                assert!(detail.contains("Atari800 failed"), "{detail}");
            }
        }
        assert_eq!(std::fs::read(&fixture.game).unwrap(), before);
        assert_eq!(
            std::fs::read(fixture.root().join("xl.rom")).unwrap(),
            vec![0x31; 16384]
        );
    }
}

#[test]
fn a_game_changed_after_play_was_offered_is_refused_at_click_time() {
    let fixture = Fixture::new(0);
    let request = blue_request(&fixture.plan);
    let mut bytes = atr_bytes();
    bytes[9000] ^= 0xff;
    std::fs::write(&fixture.game, bytes).unwrap();
    let error = fixture.run(request).err().unwrap();
    assert!(error.starts_with("Atari800 was not started"), "{error}");
    assert!(
        !fixture.root().join("run/emuwiz/launch").exists() || {
            std::fs::read_dir(fixture.root().join("run/emuwiz/launch"))
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_type().unwrap().is_dir())
        }
    );
}

#[test]
fn a_removed_emulator_or_withdrawn_consent_is_refused_at_click_time() {
    let fixture = Fixture::new(0);
    let request = blue_request(&fixture.plan);
    apply_edit(&fixture.paths, Edit::AcknowledgeDisposable(false)).unwrap();
    let error = fixture.run(request.clone()).err().unwrap();
    assert!(error.contains("Temporary Atari800 session"), "{error}");
    apply_edit(&fixture.paths, Edit::AcknowledgeDisposable(true)).unwrap();
    std::fs::remove_file(fixture.root().join("atari800")).unwrap();
    let error = fixture.run(request).err().unwrap();
    assert!(error.contains("not installed"), "{error}");
}

#[test]
fn an_unavailable_workspace_is_a_readable_refusal() {
    let fixture = Fixture::new(0);
    let request = blue_request(&fixture.plan);
    let missing = fixture.root().join("no-such-base");
    let error = request
        .execute_with(&fixture.paths, None, || SandboxManager::open(&missing))
        .err()
        .unwrap();
    assert!(
        error.contains("temporary launch workspace is unavailable"),
        "{error}"
    );
}

#[test]
fn setup_edits_validate_roms_and_reset_consent_for_a_new_file() {
    let fixture = Fixture::new(0);
    let root = fixture.root();
    let before = load_setup(&fixture.paths.setup_file).unwrap();
    // Wrong size for the role: refused, nothing saved.
    let small = root.join("small.rom");
    std::fs::write(&small, vec![0u8; 8192]).unwrap();
    let error = apply_edit(
        &fixture.paths,
        Edit::BindRom(Atari800RomKind::OsXlXe, small.clone()),
    )
    .unwrap_err();
    assert!(error.contains("needs exactly 16384"), "{error}");
    assert_eq!(load_setup(&fixture.paths.setup_file).unwrap(), before);
    // The same 8 KiB file is a valid BASIC binding, by explicit role only.
    apply_edit(&fixture.paths, Edit::BindRom(Atari800RomKind::Basic, small)).unwrap();
    let setup = load_setup(&fixture.paths.setup_file).unwrap();
    assert!(setup.basic.is_some() && !setup.basic_enabled);
    assert!(
        !setup.accept_present_unverified_roms,
        "new ROM, consent starts over"
    );
    // Re-selecting the identical OS ROM keeps an already given consent.
    apply_edit(&fixture.paths, Edit::AcceptUnverified(true)).unwrap();
    apply_edit(
        &fixture.paths,
        Edit::BindRom(Atari800RomKind::OsXlXe, root.join("xl.rom")),
    )
    .unwrap();
    assert!(
        load_setup(&fixture.paths.setup_file)
            .unwrap()
            .accept_present_unverified_roms
    );
    // A relative or symlinked program is never stored.
    std::os::unix::fs::symlink(root.join("atari800"), root.join("link")).unwrap();
    assert!(apply_edit(&fixture.paths, Edit::Executable(Some(root.join("link")))).is_err());
    assert!(apply_edit(&fixture.paths, Edit::Executable(Some("atari800".into()))).is_err());
    // Removing a selection never deletes the ROM file.
    apply_edit(&fixture.paths, Edit::ClearRom(Atari800RomKind::OsXlXe)).unwrap();
    assert!(root.join("xl.rom").exists());
    assert!(!resolve_from(&fixture.paths).unwrap().1.eligible());
}

#[test]
fn cleanup_outcomes_are_reported_in_words() {
    assert_eq!(cleanup_line(&CleanupOutcome::Running), None);
    assert!(
        cleanup_line(&CleanupOutcome::Removed)
            .unwrap()
            .contains("removed")
    );
    let kept = cleanup_line(&CleanupOutcome::Retained {
        path: "/run/x".into(),
        expires_unix: 0,
    })
    .unwrap();
    assert!(kept.contains("/run/x") && kept.contains("kept"), "{kept}");
    let refused = cleanup_line(&CleanupOutcome::Refused {
        path: "/run/y".into(),
        reason: "busy".into(),
    })
    .unwrap();
    assert!(
        refused.contains("could not be removed: busy") && refused.contains("were not changed"),
        "{refused}"
    );
}

#[test]
fn the_setup_panel_draws_without_starting_or_changing_anything() {
    let mut state = StandaloneLaunchState::default();
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_offer(ui, &mut state, Err("Choose the Atari machine".into()));
        });
    });
    assert!(!state.is_active());
    assert!(state.atari800.message.is_none());
}

fn retroarch_candidate(game: &Path, preference: CandidatePreference) -> LaunchCandidate {
    use archivefs_core::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
    LaunchCandidate {
        target: LaunchTarget::RetroArchCore {
            profile: ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
            },
            platform_id: PLATFORM_ID,
            core_stem: "atari800".into(),
        },
        content: LaunchContentRef {
            kind: None,
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(game.to_owned()),
            requires_mount: false,
            provenance: "test".into(),
        },
        firmware: FirmwareReadiness::NotRequired,
        blockers: vec![],
        warnings: vec![],
        readiness: LaunchReadiness::Ready,
        preference,
    }
}

fn is_atari800_launch(action: &GamerPlayAction) -> bool {
    matches!(
        action,
        GamerPlayAction::Launch(request)
            if matches!(**request, TypedLaunchRequest::Standalone(StandaloneLaunchRequest::Atari800(_)))
    )
}

/// With RetroArch as the automatic route, a ready native Atari800 beside it
/// must never take over Play; it launches only once it is the chosen route.
#[test]
fn a_second_ready_emulator_never_takes_play_from_the_automatic_route() {
    let fixture = Fixture::new(0);
    let mut both = fixture.plan.clone();
    both.candidates[0].preference = CandidatePreference::Undetermined;
    both.candidates.insert(
        0,
        retroarch_candidate(&fixture.game, CandidatePreference::SoleEligible),
    );
    assert!(!is_atari800_launch(&gamer_play_action(&blue_input(&both))));
    // The chooser can still offer it: choosing it yields a real request.
    assert!(atari800_launchable(&both, &both.candidates[1]));

    // Chosen explicitly (as the shared launch-choice facility marks it).
    let mut chosen = both.clone();
    chosen.candidates[0].preference = CandidatePreference::Undetermined;
    chosen.candidates[1].preference = CandidatePreference::Remembered;
    assert!(is_atari800_launch(&gamer_play_action(&blue_input(&chosen))));

    // A chosen route does not skip click-time checks: stale evidence refuses.
    let request = blue_request(&chosen);
    let mut bytes = atr_bytes();
    bytes[5000] ^= 0xff;
    std::fs::write(&fixture.game, bytes).unwrap();
    assert!(fixture.run(request).is_err());
}

#[test]
fn a_saved_choice_that_is_unavailable_holds_atari800_with_the_reason() {
    let fixture = Fixture::new(0);
    let mut held = fixture.plan.clone();
    held.candidates[0].preference = CandidatePreference::Undetermined;
    held.candidates[0].warnings.push(LaunchWarning::new(
        LaunchWarningKind::PreferredLaunchChoiceUnavailable,
        "The emulator in your default for this game is no longer available.",
    ));
    let offer = Request::offer(&held, &held.candidates[0], || None);
    assert_eq!(
        offer.err().as_deref(),
        Some("The emulator in your default for this game is no longer available.")
    );
    assert!(!is_atari800_launch(&gamer_play_action(&blue_input(&held))));
}

#[test]
fn tests_never_see_the_developers_own_setup() {
    TEST_PATHS.with(|slot| *slot.borrow_mut() = None);
    let Snapshot::Current(Ok((setup, resolution))) = current() else {
        panic!("tests resolve synchronously")
    };
    assert_eq!(setup, Atari800Setup::default());
    assert!(resolution.discovered.is_empty());
    assert!(standalone_profile_input().is_none());
}

/// A format Atari800 is not given is not reported as a setup problem.
#[test]
fn an_unsupported_format_is_not_called_a_setup_problem() {
    let fixture = Fixture::new(0);
    let mut xex = fixture.plan.clone();
    xex.candidates[0].content.resolved_path = Some(fixture.root().join("Game.xex"));
    let GamerPlayAction::BlockedTyped(blocker) = gamer_play_action(&blue_input(&xex)) else {
        panic!("only .atr launches")
    };
    assert_eq!(blocker.kind, GamerBlockerKind::NoSafeEmulator);
    assert!(blocker.detail.contains(".atr"), "{}", blocker.detail);
}

/// While the setup is being checked, or its drive is not answering, the
/// candidate is blocked with a truthful reason and nothing can launch.
#[test]
fn an_unchecked_setup_is_blocked_with_a_truthful_reason_and_cannot_launch() {
    let fixture = Fixture::new(0);
    let unknown = unknown_profile_input();
    assert!(!unknown.eligible);
    let mut plan = fixture.plan.clone();
    plan.candidates[0].target = LaunchTarget::Standalone {
        adapter_id: ADAPTER_KEY,
        profile_id: unknown.profile_id.clone(),
        profile_path: None,
    };
    plan.candidates[0].readiness = LaunchReadiness::Blocked;
    plan.candidates[0].warnings.clear();
    plan.candidates[0].blockers = vec![LaunchBlocker::new(
        LaunchBlockerKind::ProfileIneligible,
        "the discovered profile is not eligible",
    )];
    for (reason, checking) in [(CHECKING, true), (UNAVAILABLE, false)] {
        let offer = Request::offer(&plan, &plan.candidates[0], || Some(reason.into()));
        let shown = offer.err().unwrap();
        assert_eq!(shown, reason);
        assert_eq!(is_checking(&shown), checking);
    }
    // Even a request forged for that placeholder profile is refused at click.
    let mut forged = fixture.plan.clone();
    forged.candidates[0].target = plan.candidates[0].target.clone();
    let request = Request::offer(&forged, &forged.candidates[0], || None).unwrap();
    assert!(fixture.run(request).is_err());
}

/// A ROM swapped after it was bound cannot launch, whatever an earlier
/// background check said: the click re-reads and re-hashes it.
#[test]
fn firmware_changed_after_a_ready_check_cannot_launch() {
    let fixture = Fixture::new(0);
    let request = blue_request(&fixture.plan);
    std::fs::write(fixture.root().join("xl.rom"), vec![0x99; 16384]).unwrap();
    let error = fixture.run(request.clone()).err().unwrap();
    assert!(error.contains("no longer the file you selected"), "{error}");
    // The ROM disappears altogether (drive unplugged).
    std::fs::remove_file(fixture.root().join("xl.rom")).unwrap();
    let error = fixture.run(request).err().unwrap();
    assert!(error.contains("missing"), "{error}");
}

/// Offering Play never touches the game file (it runs on every redraw); a
/// file that is gone, a folder or a symlink is refused by the launch worker.
#[test]
fn the_game_file_is_only_examined_at_click_time_and_bad_files_are_refused_there() {
    let fixture = Fixture::new(0);
    let root = fixture.root();
    std::fs::create_dir(root.join("folder.atr")).unwrap();
    std::os::unix::fs::symlink(&fixture.game, root.join("link.atr")).unwrap();
    for name in ["absent.atr", "folder.atr", "link.atr"] {
        let mut plan = fixture.plan.clone();
        plan.candidates[0].content.resolved_path = Some(root.join(name));
        let request = blue_request(&plan);
        let error = fixture.run(request).err().unwrap();
        assert!(
            error.starts_with("Atari800 was not started"),
            "{name}: {error}"
        );
    }
    assert_eq!(std::fs::read(&fixture.game).unwrap(), atr_bytes());
}

/// Two other routes are tied (say, the same core in two RetroArch installs)
/// and wait for a choice. A ready native Atari800 beside them must wait too:
/// Play never starts it just because it is the only request that could build.
#[test]
fn tied_other_routes_never_let_native_atari800_win_play_by_default() {
    let fixture = Fixture::new(0);
    let tie = || {
        let mut candidate = retroarch_candidate(&fixture.game, CandidatePreference::Undetermined);
        candidate.warnings.push(LaunchWarning::new(
            LaunchWarningKind::MultipleEligibleProfiles,
            "more than one eligible profile exists for this platform and none is remembered",
        ));
        candidate.readiness = LaunchReadiness::ReadyWithWarnings;
        candidate
    };
    let mut plan = fixture.plan.clone();
    plan.candidates[0].preference = CandidatePreference::Undetermined;
    plan.candidates.insert(0, tie());
    plan.candidates.insert(0, tie());
    let action = gamer_play_action(&blue_input(&plan));
    assert!(!is_atari800_launch(&action), "native Atari800 took Play");
    assert!(matches!(action, GamerPlayAction::BlockedTyped(_)));
    // Choosing it is still possible, and then it launches.
    assert!(atari800_launchable(&plan, &plan.candidates[2]));
    plan.candidates[2].preference = CandidatePreference::Remembered;
    assert!(is_atari800_launch(&gamer_play_action(&blue_input(&plan))));
}
