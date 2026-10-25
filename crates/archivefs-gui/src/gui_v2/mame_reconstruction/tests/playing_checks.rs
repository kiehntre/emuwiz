use super::super::playing;
use super::*;
use std::os::unix::fs::PermissionsExt;

fn setup() -> (Lab, MameEnvironment, PathBuf) {
    let lab = Lab::new();
    let environment = version_environment(&lab);
    let xml = versioned_xml("0.250", CLONE)
        .replace(
            "<machine name=\"parent\"",
            "<machine runnable=\"yes\" name=\"parent\"",
        )
        .replace(
            "<machine name=\"clone\"",
            "<machine runnable=\"yes\" name=\"clone\"",
        );
    std::fs::write(lab.inputs("clone").dat, xml).unwrap();
    lab.zip("bios.zip", &[("sys.rom", BIOS)]);
    lab.zip("parent.zip", &[("p.rom", PARENT)]);
    lab.zip("clone.zip", &[("c.rom", CLONE)]);
    let executable = lab.dir.path().join("mame-test-not-executed");
    std::fs::write(&executable, b"#!/bin/sh\nexit 97\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    (lab, environment, executable)
}

#[test]
fn split_parent_and_clone_select_their_own_machine_not_a_rom_member() {
    let (lab, environment, executable) = setup();
    let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
    for machine in ["parent", "clone"] {
        let request = playing::request(&report, machine, executable.clone()).unwrap();
        let command = archivefs_core::launch::preflight_mame_launch(&request).unwrap();
        assert_eq!(command.set_name, machine);
        assert_eq!(command.arguments.last().unwrap(), machine);
        assert_eq!(request.rom_search_path, lab.root());
        assert!(request.set_resolutions[0].state == archivefs_core::dat::set::SetState::Complete);
    }
    assert!(playing::request(&report, "bios", executable.clone()).is_err());
    assert!(playing::request(&report, "c.rom", executable).is_err());
}

#[test]
fn a_merged_archive_keeps_the_selected_clone_identity() {
    let (lab, environment, executable) = setup();
    std::fs::remove_file(lab.root().join("clone.zip")).unwrap();
    lab.zip("parent.zip", &[("p.rom", PARENT), ("clone/c.rom", CLONE)]);
    let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
    let request = playing::request(&report, "clone", executable).unwrap();
    assert_eq!(request.selected_content, lab.root().join("parent.zip"));
    assert_eq!(request.set_resolutions[0].identity.game_name, "clone");
    assert_eq!(
        archivefs_core::launch::preflight_mame_launch(&request)
            .unwrap()
            .set_name,
        "clone"
    );
}

#[test]
fn missing_bios_device_and_chd_requirements_block_the_native_planner() {
    for kind in ["bios", "device", "chd", "chd-present"] {
        let (lab, environment, executable) = setup();
        if kind == "bios" {
            std::fs::remove_file(lab.root().join("bios.zip")).unwrap();
        } else {
            let path = lab.inputs("clone").dat;
            let xml = std::fs::read_to_string(&path).unwrap();
            let extra = if kind == "device" {
                "<device_ref name=\"missing-device\"/>"
            } else {
                "<disk name=\"disc\" sha1=\"1111111111111111111111111111111111111111\"/>"
            };
            std::fs::write(path, xml.replace("<machine runnable=\"yes\" name=\"clone\" cloneof=\"parent\" romof=\"parent\">", &format!("<machine runnable=\"yes\" name=\"clone\" cloneof=\"parent\" romof=\"parent\">{extra}"))).unwrap();
        }
        if kind == "chd-present" {
            let folder = lab.root().join("clone");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                folder.join("disc.chd"),
                b"MComprHD-only-a-header-not-verified-content",
            )
            .unwrap();
        }
        let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
        let machine = report.machines.iter().find(|m| m.name == "clone").unwrap();
        assert!(!machine.result.evidence.verified_complete(), "{kind}");
        assert!(
            playing::request(&report, "clone", executable).is_err(),
            "{kind}"
        );
    }
}

#[test]
fn source_or_catalogue_changes_require_new_library_review() {
    for change in ["catalogue", "content", "replacement", "missing", "symlink"] {
        let (lab, environment, executable) = setup();
        let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
        playing::revalidate(&report, &environment, "clone", executable.clone()).unwrap();
        let path = lab.root().join("clone.zip");
        match change {
            "catalogue" => {
                let dat = lab.inputs("clone").dat;
                let mut text = std::fs::read_to_string(&dat).unwrap();
                text.push_str("<!-- changed catalogue -->");
                std::fs::write(dat, text).unwrap();
            }
            "content" => {
                lab.zip("clone.zip", &[("c.rom", b"substitution")]);
            }
            "replacement" => {
                let bytes = std::fs::read(&path).unwrap();
                std::fs::rename(&path, lab.dir.path().join("old.zip")).unwrap();
                std::fs::write(&path, bytes).unwrap();
            }
            "missing" => {
                std::fs::remove_file(&path).unwrap();
            }
            "symlink" => {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(lab.root().join("parent.zip"), &path).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            playing::revalidate(&report, &environment, "clone", executable).is_err(),
            "{change}"
        );
    }
}

#[test]
fn explicit_rom_source_is_not_replaced_by_clone_ancestry() {
    let (lab, environment, executable) = setup();
    let path = lab.inputs("clone").dat;
    let xml = std::fs::read_to_string(&path).unwrap().replace(
        "name=\"clone\" cloneof=\"parent\" romof=\"parent\"",
        "name=\"clone\" cloneof=\"parent\" romof=\"absent-source\"",
    );
    std::fs::write(path, xml).unwrap();
    let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
    assert!(playing::request(&report, "clone", executable).is_err());
}

#[test]
fn default_runnable_is_supported_but_explicit_no_and_bad_flags_refuse() {
    for runnable in ["", "runnable=\"no\"", "runnable=\"unknown\""] {
        let (lab, environment, executable) = setup();
        let path = lab.inputs("clone").dat;
        let xml = std::fs::read_to_string(&path).unwrap().replace(
            "runnable=\"yes\" name=\"clone\"",
            &format!("{runnable} name=\"clone\""),
        );
        std::fs::write(path, xml).unwrap();
        let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
        assert_eq!(
            playing::request(&report, "clone", executable).is_ok(),
            runnable.is_empty()
        );
    }
}

#[test]
fn unavailable_emulator_refuses_without_executing_anything() {
    let (lab, environment, _) = setup();
    let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
    let reason =
        playing::request(&report, "clone", lab.dir.path().join("missing-mame")).unwrap_err();
    assert!(reason.contains("executable"), "{reason}");
}

#[test]
fn successful_reconstruction_refreshes_library_storage_without_a_playability_claim() {
    let (lab, environment, _) = setup();
    let mut panel = MameReconstructionPanel::with_executor(environment.clone(), Arc::new(execute));
    let inputs = lab.inputs("clone");
    panel.root = Some(inputs.root);
    panel.dat = Some(inputs.dat);
    panel.catalogue = Some(inputs.reference);
    panel.catalogue_trusted = true;
    panel.set = "clone".into();
    panel.refresh_playing_library();
    wait_library(&mut panel);
    let old = panel.playing.report.as_ref().unwrap().inventory.clone();
    let preview = version_preview(&lab, &environment);
    assert!(panel.start(MameRequest::Publish {
        preview: Box::new(preview)
    }));
    wait_for(&mut panel);
    wait_library(&mut panel);
    let report = panel.playing.report.as_ref().unwrap().clone();
    assert_ne!(
        report.inventory, old,
        "publication must trigger actual fresh inspection"
    );
    let parent = report
        .inventory
        .sets
        .iter()
        .find(|s| s.name == "parent")
        .unwrap();
    assert!(parent.members.iter().any(|m| m.name.ends_with("c.rom")));
    assert!(
        report
            .machines
            .iter()
            .find(|m| m.name == "clone")
            .unwrap()
            .result
            .evidence
            .verified_complete()
    );
    let text = render(&mut panel);
    assert!(text.contains("Verified ROM completeness does not prove that a machine boots"));
    assert_eq!(
        report
            .machines
            .iter()
            .find(|m| m.name == "clone")
            .unwrap()
            .result
            .evidence
            .runnability,
        archivefs_core::mame_collection_analyser::MameRunnability::NotAssessed
    );
    assert!(text.contains("MAME Playing Library"));
}

#[test]
fn fresh_validation_failure_is_reported_by_the_existing_launch_worker() {
    let (lab, environment, executable) = setup();
    let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
    let content = playing::request(&report, "clone", executable.clone())
        .unwrap()
        .selected_content;
    std::fs::remove_file(lab.root().join("bios.zip")).unwrap();
    let mut state = crate::launch_readiness_page::StandaloneLaunchState::default();
    state.start_mame_revalidated(content, move || {
        playing::revalidate(&report, &environment, "clone", executable)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.is_active() {
        assert!(Instant::now() < deadline);
        state.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    let error = state
        .failure_detail()
        .expect("stale evidence must fail before spawn");
    assert!(error.contains("changed"), "{error}");
}

fn wait_library(panel: &mut MameReconstructionPanel) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while panel.playing.busy() {
        panel.poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

const CHEAT_XML: &str = r#"<?xml version="1.0"?><mamecheat version="1"><cheat desc="Free Play"><script state="on"><action>0x2000 = 1</action></script></cheat><cheat desc="Other"><script state="on"><action>0x2001 = 1</action></script></cheat></mamecheat>"#;

/// A stand-in MAME that records its arguments and the cheat file it was given.
fn recording_mame(executable: &std::path::Path, out: &std::path::Path) {
    std::fs::write(
        executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{o}.argv'\nwhile [ $# -gt 1 ]; do [ \"$1\" = -cheatpath ] && cp \"$2\"/*.xml '{o}.seen'; shift; done\nexit 0\n",
            o = out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn run_to_end(state: &mut crate::launch_readiness_page::StandaloneLaunchState) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while state.is_active() {
        assert!(Instant::now() < deadline, "launch did not finish");
        state.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    state.poll();
}

#[test]
fn chosen_cheats_reach_mame_as_a_temporary_file_removed_after_exit() {
    use super::super::cheats::CheatChoice;
    let (lab, environment, executable) = setup();
    let out = lab.dir.path().join("rec");
    recording_mame(&executable, &out);
    let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
    let content = playing::request(&report, "clone", executable.clone())
        .unwrap()
        .selected_content;
    let source = lab.dir.path().join("mine").join("clone.xml");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, CHEAT_XML).unwrap();
    let mut choice = CheatChoice::load(&source, "clone").unwrap();
    assert_eq!(choice.chosen_count(), 0, "nothing is ticked by default");
    choice.entries[1].chosen = true;
    let root = lab.dir.path().join("cheat-root");
    let roots = archivefs_core::launch::cheat_runtime::CheatRuntimeRoots {
        approved_root: root.clone(),
    };
    let mut state = crate::launch_readiness_page::StandaloneLaunchState::default();
    state.start_mame_revalidated_with_cheats(
        content,
        choice.selection().unwrap(),
        roots,
        move || playing::revalidate(&report, &environment, "clone", executable),
    );
    run_to_end(&mut state);
    assert_eq!(state.failure_detail(), None);
    let argv = std::fs::read_to_string(format!("{}.argv", out.display())).unwrap();
    let args: Vec<&str> = argv.lines().collect();
    assert!(args.contains(&"-cheat"), "{args:?}");
    assert!(args.contains(&"-cheatpath"), "{args:?}");
    assert_eq!(args.last(), Some(&"clone"));
    let seen = std::fs::read_to_string(format!("{}.seen", out.display())).unwrap();
    assert!(seen.contains("Other"), "{seen}");
    assert!(
        !seen.contains("Free Play"),
        "unticked cheats must not be passed: {seen}"
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), CHEAT_XML);
    let leftovers = std::fs::read_dir(&root).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "temporary cheat workspace must be removed");
}

#[test]
fn a_cheat_file_that_no_longer_matches_stops_the_launch_before_mame_starts() {
    use super::super::cheats::CheatChoice;
    let (lab, environment, executable) = setup();
    let out = lab.dir.path().join("rec");
    recording_mame(&executable, &out);
    let report = playing::inspect(&lab.inputs("clone"), &environment).unwrap();
    let content = playing::request(&report, "clone", executable.clone())
        .unwrap()
        .selected_content;
    let source = lab.dir.path().join("mine").join("clone.xml");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, CHEAT_XML).unwrap();
    let mut choice = CheatChoice::load(&source, "clone").unwrap();
    choice.entries[0].chosen = true;
    let selection = choice.selection().unwrap();
    // The file changes after the user's choice: the core must refuse, not guess.
    std::fs::write(&source, CHEAT_XML.replace("Free Play", "Renamed")).unwrap();
    let roots = archivefs_core::launch::cheat_runtime::CheatRuntimeRoots {
        approved_root: lab.dir.path().join("cheat-root"),
    };
    let mut state = crate::launch_readiness_page::StandaloneLaunchState::default();
    state.start_mame_revalidated_with_cheats(content, selection, roots, move || {
        playing::revalidate(&report, &environment, "clone", executable)
    });
    run_to_end(&mut state);
    let error = state.failure_detail().expect("must be refused");
    assert!(error.contains("MAME was not started"), "{error}");
    assert!(!std::path::Path::new(&format!("{}.argv", out.display())).exists());
    assert!(!lab.dir.path().join("cheat-root").exists());
}
