use super::*;
use std::os::unix::fs::PermissionsExt;

/// A stand-in for MAME (never the real program): answers `-listxml` with the
/// given machine body and `-verifyroms` with the given text.
fn fake(dir: &Path, name: &str, machine_inner: &str, mach_attrs: &str, verify: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n*-listxml*) cat <<'XML'\n<mame><machine name=\"gam\" {mach_attrs}><driver status=\"good\" emulation=\"good\"/>{machine_inner}</machine></mame>\nXML\n;;\n*-verifyroms*) cat <<'V'\n{verify}\nV\n;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn set_dir(root: &Path) -> PathBuf {
    let dir = root.join("arcade").join("gam");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Waits (bounded) for the background probe to finish.
fn settle(executable: &Path, dir: &Path, extras: &[PathBuf]) -> Arc<ArcadeSetProbe> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(Lookup::Done(probe)) = lookup_with(executable, dir, extras, &[]) {
            return probe;
        }
        assert!(Instant::now() < deadline, "probe never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn plan_with_mame() -> LaunchPlan {
    use archivefs_core::launch::{CandidatePreference, FirmwareReadiness, LaunchCandidate};
    LaunchPlan {
        platform_id: Some("Arcade".into()),
        game_key: Some("gam".into()),
        summary: Default::default(),
        candidates: vec![LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "mame",
                profile_id: "mame".into(),
                profile_path: None,
            },
            content: archivefs_core::launch::LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: None,
                requires_mount: false,
                provenance: String::new(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: Vec::new(),
            warnings: Vec::new(),
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::Undetermined,
        }],
        media_topology: None,
    }
}

#[test]
fn first_lookup_is_checking_off_thread_and_then_resolves_to_ready() {
    let root = tempfile::tempdir().unwrap();
    // A slow stand-in proves the first call returns immediately.
    let exe = root.path().join("slow");
    std::fs::write(
        &exe,
        "#!/bin/sh\nsleep 1\ncase \"$*\" in *-listxml*) echo '<mame><machine name=\"gam\"><driver status=\"good\" emulation=\"good\"/></machine></mame>';; *) echo 'romset gam is good';; esac\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dir = set_dir(root.path());
    let started = Instant::now();
    let first = lookup_with(&exe, &dir, &[], &[]).unwrap();
    assert!(matches!(first, Lookup::Checking));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "UI thread blocked"
    );
    // One in-flight probe per key: a second call is also just Checking.
    assert!(matches!(
        lookup_with(&exe, &dir, &[], &[]).unwrap(),
        Lookup::Checking
    ));
    let probe = settle(&exe, &dir, &[]);
    assert_eq!(probe.verdict.readiness, ArcadeReadiness::Ready);
    assert_eq!(probe.resolution.state, SetState::Complete);
}

#[test]
fn imperfect_set_gets_a_named_warning_on_the_mame_candidate() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(
        root.path(),
        "m2",
        "<feature type=\"sound\" status=\"imperfect\"/>",
        "",
        "romset gam is good",
    );
    let probe = settle(&exe, &set_dir(root.path()), &[]);
    let mut plan = plan_with_mame();
    apply_warnings(&mut plan, &probe);
    assert_eq!(
        plan.candidates[0].readiness,
        LaunchReadiness::ReadyWithWarnings
    );
    assert!(plan.candidates[0].warnings[0].detail.contains("sound"));
}

#[test]
fn blocked_set_yields_an_exact_blocker_and_an_incomplete_resolution() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(
        root.path(),
        "m3",
        "",
        "",
        "romset gam is bad\ngam         : x.bin (10 bytes) - NOT FOUND (neogeo)",
    );
    let probe = settle(&exe, &set_dir(root.path()), &[]);
    assert_eq!(probe.resolution.state, SetState::Incomplete);
    let blocker = blocker_for(&probe);
    assert_eq!(blocker.kind, LaunchBlockerKind::MameDependencyBlocked);
    assert!(blocker.detail.contains("neogeo"));
}

#[test]
fn fruit_and_video_with_the_same_mame_data_get_the_same_readiness() {
    let root = tempfile::tempdir().unwrap();
    let video = fake(
        root.path(),
        "v",
        "<control type=\"joy\"/>",
        "",
        "romset gam is good",
    );
    let fruit = fake(
        root.path(),
        "f",
        "<control type=\"gambling\"/>",
        "ismechanical=\"yes\"",
        "romset gam is good",
    );
    let dir = set_dir(root.path());
    let v = settle(&video, &dir, &[]);
    let f = settle(&fruit, &dir, &[]);
    assert_eq!(v.resolution.state, f.resolution.state);
    assert_ne!(f.verdict.readiness, ArcadeReadiness::Blocked);
    assert!(
        f.verdict
            .warnings
            .iter()
            .any(|w| w.contains("gambling/AWP"))
    );
}

#[test]
fn files_unsafe_names_and_non_set_paths_are_never_probed() {
    let root = tempfile::tempdir().unwrap();
    // A stand-in that records every call: it must never run.
    let log = root.path().join("calls");
    let exe = root.path().join("spy");
    std::fs::write(&exe, format!("#!/bin/sh\necho x >> {}\n", log.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let file = root.path().join("gam.zip");
    std::fs::write(&file, b"z").unwrap();
    assert!(lookup_with(&exe, &file, &[], &[]).is_none());
    let odd = root.path().join("arcade").join("a b");
    std::fs::create_dir_all(&odd).unwrap();
    assert!(lookup_with(&exe, &odd, &[], &[]).is_none());
    assert!(lookup_with(&exe, &root.path().join("missing"), &[], &[]).is_none());
    std::thread::sleep(Duration::from_millis(200));
    assert!(!log.exists(), "MAME must not run for non-set paths");
}

#[test]
fn timeouts_and_spawn_failures_are_not_cached_as_answers() {
    let root = tempfile::tempdir().unwrap();
    let dir = set_dir(root.path());
    let missing = root.path().join("no-such-mame");
    let probe = settle(&missing, &dir, &[]);
    assert_eq!(
        probe.verdict.block_reason,
        Some(ArcadeBlockReason::CheckFailed)
    );
    let state = manager().state.lock().unwrap();
    let id: Id = (missing.clone(), dir.clone(), Vec::new());
    assert!(
        !state.done.contains_key(&id),
        "failure was cached as an answer"
    );
    assert!(
        state.failed.contains_key(&id),
        "failure is tracked for back-off only"
    );
}

#[test]
fn cache_refreshes_when_rom_files_the_parent_or_an_extra_path_change() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(root.path(), "m", "", "", "romset gam is good");
    let dir = set_dir(root.path());
    let bios = root.path().join("bios");
    std::fs::create_dir(&bios).unwrap();
    let extras = [std::fs::canonicalize(&bios).unwrap()];
    let id: Id = (exe.clone(), dir.clone(), extras.to_vec());
    let first = settle(&exe, &dir, &extras);
    let revalidate = |key_changes: Box<dyn Fn()>| {
        key_changes();
        // Force re-validation now rather than waiting for the interval.
        manager()
            .state
            .lock()
            .unwrap()
            .done
            .get_mut(&id)
            .unwrap()
            .validated = Instant::now() - Duration::from_secs(10);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(Lookup::Done(p)) = lookup_with(&exe, &dir, &extras, &[])
                && !Arc::ptr_eq(&p, &first)
            {
                return p;
            }
            assert!(Instant::now() < deadline, "no refresh after change");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    // Unchanged: still the same cached answer.
    manager()
        .state
        .lock()
        .unwrap()
        .done
        .get_mut(&id)
        .unwrap()
        .validated = Instant::now() - Duration::from_secs(10);
    match lookup_with(&exe, &dir, &extras, &[]).unwrap() {
        Lookup::Done(p) => assert!(Arc::ptr_eq(&p, &first)),
        Lookup::Checking => panic!("unchanged folders must not re-probe"),
    }
    let dir2 = dir.clone();
    let after_rom = revalidate(Box::new(move || {
        std::thread::sleep(Duration::from_millis(40));
        std::fs::write(dir2.join("rom.bin"), b"x").unwrap();
    }));
    let bios2 = bios.clone();
    let first_after_rom = after_rom.clone();
    let _ = first_after_rom;
    // Parent folder change (a sibling set appears).
    let parent_new = dir.parent().unwrap().join("sibling");
    std::thread::sleep(Duration::from_millis(40));
    let id2 = id.clone();
    manager()
        .state
        .lock()
        .unwrap()
        .done
        .get_mut(&id2)
        .unwrap()
        .validated = Instant::now() - Duration::from_secs(10);
    std::fs::create_dir(&parent_new).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(Lookup::Done(p)) = lookup_with(&exe, &dir, &extras, &[])
            && !Arc::ptr_eq(&p, &after_rom)
        {
            break;
        }
        assert!(Instant::now() < deadline, "no refresh after parent change");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Extra rompath change.
    let current = settle(&exe, &dir, &extras);
    std::thread::sleep(Duration::from_millis(40));
    manager()
        .state
        .lock()
        .unwrap()
        .done
        .get_mut(&id)
        .unwrap()
        .validated = Instant::now() - Duration::from_secs(10);
    std::fs::write(bios2.join("neogeo.zip"), b"x").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(Lookup::Done(p)) = lookup_with(&exe, &dir, &extras, &[])
            && !Arc::ptr_eq(&p, &current)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no refresh after extra path change"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn explicit_recheck_forgets_the_cached_answer() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(root.path(), "m", "", "", "romset gam is good");
    let dir = set_dir(root.path());
    let first = settle(&exe, &dir, &[]);
    invalidate_path(&dir);
    assert!(matches!(
        lookup_with(&exe, &dir, &[], &[]).unwrap(),
        Lookup::Checking
    ));
    let second = settle(&exe, &dir, &[]);
    assert!(!Arc::ptr_eq(&first, &second));
}

#[test]
fn worker_threads_wait_for_the_same_cached_answer() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(root.path(), "m", "", "", "romset gam is good");
    let dir = set_dir(root.path());
    let cancel = AtomicBool::new(false);
    let result = with_blocking(&cancel, || lookup_with(&exe, &dir, &[], &[]));
    let Some(Lookup::Done(probe)) = result else {
        panic!("blocking lookup must return the answer");
    };
    assert_eq!(probe.verdict.readiness, ArcadeReadiness::Ready);
    // The UI-thread lookup now hits the same cache.
    assert!(matches!(
        lookup_with(&exe, &dir, &[], &[]).unwrap(),
        Lookup::Done(_)
    ));
}

#[test]
fn invalid_extra_rompath_lines_become_visible_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let ok = dir.path().join("bios");
    std::fs::create_dir(&ok).unwrap();
    let (good, diagnostics) =
        split_extra_rompaths(&[ok, PathBuf::from("relative"), dir.path().join("nope")]);
    assert_eq!(good.len(), 1);
    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .iter()
            .all(|d| d.starts_with("ignored extra ROM path"))
    );
    let probe = ArcadeSetProbe {
        set: "gam".into(),
        resolution: plan_resolution(),
        verdict: archivefs_core::arcade_mame_routing::evaluate_arcade_set(
            "gam",
            None,
            &archivefs_core::arcade_mame_routing::parse_verify_output("", "gam"),
            false,
        ),
        extra_rompaths: Vec::new(),
        extra_diagnostics: diagnostics,
    };
    assert!(
        blocker_for(&probe)
            .detail
            .contains("ignored extra ROM path")
    );
}

fn plan_resolution() -> SetResolution {
    SetResolution {
        identity: SetIdentity {
            source_id: "t".into(),
            game_name: "gam".into(),
        },
        archive_path: "/x".into(),
        state: SetState::Incomplete,
        members_required: Vec::new(),
        members_verified: Vec::new(),
        members_bad: Vec::new(),
        members_optional: Vec::new(),
        members_borrowed: Vec::new(),
        disks_required: Vec::new(),
        disks_identity_matched: Vec::new(),
        disks_parent_required: Vec::new(),
        dependencies: SetDependencyReport {
            state: DependencyState::NotApplicable,
            requirements: Vec::new(),
        },
    }
}

#[test]
fn peek_never_starts_a_probe() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("calls");
    let exe = root.path().join("spy");
    std::fs::write(&exe, format!("#!/bin/sh\necho x >> {}\n", log.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dir = set_dir(root.path());
    let (extras, diagnostics) = (Vec::new(), Vec::new());
    assert!(lookup_inner(&exe, &dir, &extras, &diagnostics, false).is_none());
    std::thread::sleep(Duration::from_millis(200));
    assert!(!log.exists(), "peek must not run MAME");
}
