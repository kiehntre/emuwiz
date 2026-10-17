use std::ffi::OsString;

use super::*;
use crate::launch::amiga_whdload_command::tests::input;
use crate::launch::amiga_whdload_command::{SelectedWHDLoadSlave, VerifiedWHDLoadTarget};
use crate::launch::amiga_whdload_execution::preflight_amiga_whdload_launch;
use crate::launch::planning::ResolvedIdentity;
use crate::patch_manager::whdload_trainer_discovery_fixtures::{host, inspection, synthetic_slave};
use crate::patch_manager::{
    AmigaEmulatorKind, TrainerOptionSelection, TrainerOptionValue, WhdloadTrainerApplyOptions,
    WhdloadTrainerRequest, apply_whdload_trainer_preview, build_whdload_trainer_preview,
};

/// Three bit options sharing CUSTOM1, a list on CUSTOM2 and an option syntax
/// EmuWiz does not interpret on CUSTOM4.
const CONFIG: &str = "BW;C1:X:Infinite lives:0;C1:X:Infinite energy:1;C1:X:In-game keys:3;\
                      C2:L:Start level:1,2,3;C4:Q:odd:0;";
const IDENTITY: &str = "amiga:synthetic";
const PROFILE: &[u8] = b"[fs-uae]\namiga_model=A1200\njoystick_port_1=custom-user-setting\n";

struct Lab {
    _root: tempfile::TempDir,
    game: PathBuf,
    slave: PathBuf,
    slave_bytes: Vec<u8>,
    options: PathBuf,
    executable: PathBuf,
    configuration: PathBuf,
    staging: PathBuf,
    history: PathBuf,
    backup: PathBuf,
}

fn lab() -> Lab {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("games/My Game");
    fs::create_dir_all(&game).unwrap();
    let slave = game.join("MyGame.slave");
    let slave_bytes = synthetic_slave(CONFIG);
    fs::write(&slave, &slave_bytes).unwrap();
    fs::write(game.join("data.bin"), b"game data").unwrap();
    let data = root.path().join("data/whdload-trainer-options");
    fs::create_dir_all(&data).unwrap();
    let executable = root.path().join("fs-uae");
    let configuration = root.path().join("profile.fs-uae");
    fs::write(&executable, b"").unwrap();
    fs::write(&configuration, PROFILE).unwrap();
    Lab {
        game,
        slave,
        slave_bytes,
        // Stand-in for `whdload_trainer_option_file`'s answer.
        options: data.join("0123abcd.options"),
        executable,
        configuration,
        staging: root.path().join("staging"),
        history: root.path().join("history"),
        backup: root.path().join("backup"),
        _root: root,
    }
}

impl Lab {
    fn input(&self, emulator: AmigaEmulatorKind) -> WHDLoadLaunchInput {
        let mut value = input(emulator);
        value.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Amiga".into(),
            game_key: IDENTITY.into(),
        });
        value.target = Some(VerifiedWHDLoadTarget {
            package_path: self.game.clone(),
            format: WhdloadPackageFormat::InstalledDirectory,
        });
        let parsed = value.slave.take().unwrap().parsed;
        value.slave = Some(SelectedWHDLoadSlave {
            artifact_path: self.slave.clone(),
            name: "MyGame.slave".into(),
            parsed,
        });
        value.profile.verified_identity = IDENTITY.into();
        value.profile.executable = Some(self.executable.clone());
        value.profile.configuration = Some(self.configuration.clone());
        value
    }

    fn inspection(&self) -> AmigaGameInspection {
        inspection(vec![host(&self.slave, &self.slave_bytes)], true)
    }

    fn plan(&self) -> (AmigaWHDLoadCommandPlan, WhdloadLaunchTrainerResolution) {
        let path = self.options.clone();
        plan_fsuae_whdload_launch_with_saved_trainers(
            &self.input(AmigaEmulatorKind::FsUae),
            &self.inspection(),
            move |_| Ok(path),
        )
    }

    fn projected_args(&self) -> Vec<OsString> {
        let (plan, resolution) = self.plan();
        if let Some(trainer) = resolution.trainer() {
            assert!(plan.command.is_none());
            assert_eq!(plan.readiness, LaunchReadiness::Blocked);
            assert!(preflight_amiga_whdload_launch(&plan).is_err());
            // Pure display projection of saved choices, never executable argv.
            let mut args = self.baseline_args();
            *args.last_mut().unwrap() = OsString::from(format!(
                "--x-whdload-args={}",
                super::super::amiga_whdload_command::fsuae_whdload_args_value(
                    "MyGame.slave",
                    Some(trainer)
                )
            ));
            args
        } else {
            preflight_amiga_whdload_launch(&plan).unwrap().arguments
        }
    }

    fn candidate_preview(&self) -> (AmigaWHDLoadCommandPlan, WhdloadLaunchTrainerResolution) {
        let (_, resolution) = self.plan();
        let mut plan = build_fsuae_whdload_command_plan(&self.input(AmigaEmulatorKind::FsUae));
        plan.command.as_mut().unwrap().arguments = self.projected_args();
        plan.command.as_mut().unwrap().trainer = resolution.trainer().cloned();
        (plan, resolution)
    }

    fn baseline_args(&self) -> Vec<OsString> {
        build_fsuae_whdload_command_plan(&self.input(AmigaEmulatorKind::FsUae))
            .command
            .unwrap()
            .arguments
    }

    fn expected(&self, whdload_args: &str) -> Vec<OsString> {
        vec![
            OsString::from(format!("--config={}", self.configuration.display())),
            OsString::from(format!("--hard-drive-0={}", self.game.display())),
            OsString::from("--hard-drive-0-read-only=1"),
            OsString::from(format!("--x-whdload-args={whdload_args}")),
        ]
    }

    /// Saves selections through the editor's own Preview -> Apply path, so
    /// launch is tested against exactly what the editor writes.
    fn save(&self, selections: &[(&str, bool)]) {
        let current = fs::read(&self.options).ok();
        let discovery =
            discover_whdload_trainer_options(&self.inspection(), current.as_deref().unwrap_or(&[]));
        let identity = discovery.identity.clone().expect("ready discovery");
        let selections = selections
            .iter()
            .map(|(label, on)| {
                let option = discovery
                    .options
                    .iter()
                    .find(|option| option.description == *label)
                    .unwrap_or_else(|| panic!("no option {label}"));
                TrainerOptionSelection {
                    custom_slot: option.selection_key.clone(),
                    value: Some(TrainerOptionValue::Boolean(*on)),
                }
            })
            .collect();
        let preview = build_whdload_trainer_preview(&WhdloadTrainerRequest {
            identity,
            configuration_path: self.options.clone(),
            staging_root: self.staging.clone(),
            declarations: discovery.declarations.clone(),
            selections,
            expected_configuration_sha256: current.as_deref().map(sha256_hex),
        })
        .unwrap();
        apply_whdload_trainer_preview(
            &preview,
            &WhdloadTrainerApplyOptions {
                general_approved: true,
                replacement_approved: true,
                operation_id: format!("launch-test-{}", self.history_count()),
                timestamp_unix_seconds: 1,
                history_root: self.history.clone(),
                backup_root: self.backup.clone(),
            },
        )
        .unwrap();
    }

    fn history_count(&self) -> usize {
        fs::read_dir(&self.history).map_or(0, |entries| entries.count())
    }

    fn refusal(&self) -> String {
        let (plan, resolution) = self.plan();
        assert!(plan.command.is_none(), "{resolution:?}");
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::WhdloadTrainerOptionsRefused)
        );
        match resolution {
            WhdloadLaunchTrainerResolution::Refused { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn assert_untouched(&self) {
        assert_eq!(fs::read(&self.configuration).unwrap(), PROFILE);
        assert_eq!(fs::read(&self.slave).unwrap(), self.slave_bytes);
        assert_eq!(fs::read(self.game.join("data.bin")).unwrap(), b"game data");
        assert_eq!(fs::read_dir(&self.game).unwrap().count(), 2);
    }
}

#[test]
fn no_saved_options_launches_exactly_as_without_trainers() {
    let lab = lab();
    let (_, resolution) = lab.plan();
    assert_eq!(resolution, WhdloadLaunchTrainerResolution::NoSavedOptions);
    assert_eq!(lab.projected_args(), lab.baseline_args());
    assert_eq!(lab.projected_args(), lab.expected("MyGame.slave"));
    // An empty file, and a file where every option is off, are the same.
    fs::write(&lab.options, b"").unwrap();
    assert_eq!(lab.projected_args(), lab.baseline_args());
    fs::write(&lab.options, b"CUSTOM1=0\n; nothing on\n").unwrap();
    assert_eq!(lab.plan().1, WhdloadLaunchTrainerResolution::NoSavedOptions);
    assert_eq!(lab.projected_args(), lab.baseline_args());
    lab.assert_untouched();
}

#[test]
fn saved_trainer_projection_is_not_an_executable_launch() {
    let lab = lab();
    lab.save(&[("Infinite lives", true)]);
    assert_eq!(lab.projected_args(), lab.expected("MyGame.slave CUSTOM1=1"));
    lab.assert_untouched();
}

#[test]
fn several_saved_bits_sharing_custom1_project_as_one_value() {
    let lab = lab();
    lab.save(&[
        ("Infinite lives", true),
        ("Infinite energy", true),
        ("In-game keys", true),
    ]);
    assert_eq!(fs::read_to_string(&lab.options).unwrap(), "CUSTOM1=11");
    assert_eq!(
        lab.projected_args(),
        lab.expected("MyGame.slave CUSTOM1=11")
    );
    let (_, resolution) = lab.plan();
    assert_eq!(resolution.trainer().unwrap().arguments, vec!["CUSTOM1=11"]);
    lab.assert_untouched();
}

#[test]
fn enabling_and_disabling_independent_options_changes_only_their_bits() {
    let lab = lab();
    lab.save(&[("Infinite lives", true), ("In-game keys", true)]);
    assert_eq!(lab.projected_args(), lab.expected("MyGame.slave CUSTOM1=9"));
    lab.save(&[("Infinite energy", true)]);
    assert_eq!(
        lab.projected_args(),
        lab.expected("MyGame.slave CUSTOM1=11")
    );
    lab.save(&[("Infinite lives", false)]);
    assert_eq!(
        lab.projected_args(),
        lab.expected("MyGame.slave CUSTOM1=10")
    );
    lab.save(&[("Infinite energy", false), ("In-game keys", false)]);
    assert_eq!(lab.projected_args(), lab.baseline_args());
}

#[test]
fn several_saved_slots_project_in_slot_order() {
    let lab = lab();
    fs::write(&lab.options, b"PRELOAD\nCUSTOM2=2\nC1=3 ; lives+energy\n").unwrap();
    assert_eq!(
        lab.projected_args(),
        lab.expected("MyGame.slave CUSTOM1=3 CUSTOM2=2")
    );
}

#[test]
fn malformed_conflicting_or_undeclared_saved_options_are_refused() {
    let lab = lab();
    for (bytes, needle) in [
        (&b"CUSTOM1=abc\n"[..], "malformed"),
        (b"CUSTOM1=1 CUSTOM2=2\n", "malformed"),
        (b"CUSTOM1=$(reboot)\n", "malformed"),
        (b"CUSTOM1=\n", "malformed"),
        (b"CUSTOM1\n", "malformed"),
        (b"CUSTOM1=99999999999\n", "malformed"),
        (b"CUSTOM1=1\nCUSTOM1=2\n", "more than once"),
        (b"CUSTOM1=1\nC1=1\n", "more than once"),
        (b"CUSTOM1=4\n", "bits no trainer option declares"),
        (b"CUSTOM2=7\n", "not one this game's slave allows"),
        (b"CUSTOM3=1\n", "not declared"),
        (b"CUSTOM4=1\n", "cannot be interpreted"),
        (b"\xff\xfe\n", "UTF-8"),
    ] {
        fs::write(&lab.options, bytes).unwrap();
        let reason = lab.refusal();
        assert!(reason.contains(needle), "{bytes:?}: {reason}");
    }
    lab.assert_untouched();
}

#[test]
fn an_unsafe_option_file_is_refused() {
    let lab = lab();
    let real = lab.options.with_extension("real");
    fs::write(&real, b"CUSTOM1=1").unwrap();
    std::os::unix::fs::symlink(&real, &lab.options).unwrap();
    assert!(lab.refusal().contains("not a regular file"));
    fs::remove_file(&lab.options).unwrap();
    fs::create_dir(&lab.options).unwrap();
    assert!(lab.refusal().contains("not a regular file"));
    fs::remove_dir(&lab.options).unwrap();
    fs::write(
        &lab.options,
        vec![b' '; WHDLOAD_TRAINER_MAX_CONFIG_BYTES + 1],
    )
    .unwrap();
    assert!(lab.refusal().contains("too large"));
}

#[test]
fn a_storage_error_refuses_rather_than_launching_without_the_options() {
    let lab = lab();
    let (plan, resolution) = plan_fsuae_whdload_launch_with_saved_trainers(
        &lab.input(AmigaEmulatorKind::FsUae),
        &lab.inspection(),
        |_| Err("EmuWiz's data folder could not be resolved".into()),
    );
    assert!(plan.command.is_none());
    assert!(matches!(
        resolution,
        WhdloadLaunchTrainerResolution::Refused { .. }
    ));
}

#[test]
fn a_slave_or_option_file_changed_after_planning_is_refused_before_spawn() {
    let lab = lab();
    lab.save(&[("Infinite lives", true)]);
    let (plan, resolution) = lab.candidate_preview();
    // A crafted preview's command carries the trainer: the canonical preflight that
    // every launch path uses re-proves it, so no caller can skip the check.
    assert_eq!(
        plan.command.as_ref().unwrap().trainer.as_ref(),
        resolution.trainer()
    );
    assert!(
        preflight_amiga_whdload_launch(&plan)
            .unwrap_err()
            .detail
            .contains("trainer launch unsupported")
    );

    fs::write(&lab.options, b"CUSTOM1=3").unwrap();
    let error = preflight_amiga_whdload_launch(&plan).unwrap_err();
    assert!(error.detail.contains("option file changed"), "{error:?}");
    fs::remove_file(&lab.options).unwrap();
    assert!(preflight_amiga_whdload_launch(&plan).is_err());
    fs::write(&lab.options, b"CUSTOM1=1").unwrap();
    assert!(
        preflight_amiga_whdload_launch(&plan)
            .unwrap_err()
            .detail
            .contains("trainer launch unsupported")
    );

    fs::write(&lab.slave, synthetic_slave("C1:X:Other:0;")).unwrap();
    let error = preflight_amiga_whdload_launch(&plan).unwrap_err();
    assert!(error.detail.contains("slave changed"), "{error:?}");
}

#[test]
fn a_slave_changed_since_inspection_is_refused_at_planning() {
    let lab = lab();
    lab.save(&[("Infinite lives", true)]);
    let inspection = lab.inspection();
    fs::write(&lab.slave, synthetic_slave(CONFIG).repeat(2)).unwrap();
    let path = lab.options.clone();
    let (plan, resolution) = plan_fsuae_whdload_launch_with_saved_trainers(
        &lab.input(AmigaEmulatorKind::FsUae),
        &inspection,
        move |_| Ok(path),
    );
    assert!(plan.command.is_none());
    assert!(matches!(
        resolution,
        WhdloadLaunchTrainerResolution::Refused { .. }
    ));
}

#[test]
fn a_command_whose_argv_no_longer_carries_the_options_is_refused() {
    let lab = lab();
    lab.save(&[("Infinite lives", true)]);
    let (mut plan, _) = lab.candidate_preview();
    let command = plan.command.as_mut().unwrap();
    let last = command.arguments.last_mut().unwrap();
    *last = OsString::from("--x-whdload-args=MyGame.slave CUSTOM1=3");
    let error = preflight_amiga_whdload_launch(&plan).unwrap_err();
    assert!(error.detail.contains("does not carry"), "{error:?}");
}

#[test]
fn tooltypes_and_non_slot_keys_are_ignored_as_the_editor_ignores_them() {
    let lab = lab();
    fs::write(
        &lab.options,
        b"PRELOAD\nCUSTOMSAVE=x\nC64=1\nCUSTOM9=1\nSPLASHDELAY=50\nCUSTOM1=1\n",
    )
    .unwrap();
    assert_eq!(lab.projected_args(), lab.expected("MyGame.slave CUSTOM1=1"));
}

#[test]
fn list_values_are_checked_by_declared_number_not_list_position() {
    // `easy,,hard`: the empty label is skipped but "hard" keeps value 2,
    // which is what the editor writes for it.
    let mut lab = lab();
    lab.slave_bytes = synthetic_slave("C2:L:Difficulty:easy,,hard;");
    fs::write(&lab.slave, &lab.slave_bytes).unwrap();
    fs::write(&lab.options, b"CUSTOM2=2").unwrap();
    assert_eq!(lab.projected_args(), lab.expected("MyGame.slave CUSTOM2=2"));
    fs::write(&lab.options, b"CUSTOM2=1").unwrap();
    assert!(lab.refusal().contains("not one this game's slave allows"));
}

#[test]
fn the_installed_folder_is_mounted_read_only() {
    let lab = lab();
    let baseline = lab.baseline_args();
    assert!(baseline.contains(&OsString::from("--hard-drive-0-read-only=1")));
}

#[test]
fn a_fifo_option_file_is_refused_without_blocking() {
    let lab = lab();
    let path = std::ffi::CString::new(lab.options.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(lab.refusal().contains("not a regular file"));
}

#[test]
fn missing_unverified_or_ambiguous_slaves_launch_unchanged_without_options() {
    let lab = lab();
    fs::write(&lab.options, b"CUSTOM1=1").unwrap();
    let path = lab.options.clone();
    let run = |inspection: AmigaGameInspection| {
        let path = path.clone();
        plan_fsuae_whdload_launch_with_saved_trainers(
            &lab.input(AmigaEmulatorKind::FsUae),
            &inspection,
            move |_| Ok(path),
        )
    };
    let other = lab.game.join("Other.slave");
    fs::write(&other, &lab.slave_bytes).unwrap();
    for inspection in [
        inspection(Vec::new(), true),
        inspection(vec![host(&lab.slave, &lab.slave_bytes)], false),
        inspection(
            vec![
                host(&lab.slave, &lab.slave_bytes),
                host(&other, &lab.slave_bytes),
            ],
            true,
        ),
    ] {
        let (plan, resolution) = run(inspection);
        assert!(
            matches!(
                resolution,
                WhdloadLaunchTrainerResolution::NotApplicable { .. }
            ),
            "{resolution:?}"
        );
        // The trainer layer adds nothing; the launch is the baseline plan.
        assert_eq!(
            plan,
            build_fsuae_whdload_command_plan(&lab.input(AmigaEmulatorKind::FsUae))
        );
    }
    // A slave file deleted after inspection is unreadable: no options.
    fs::remove_file(&other).unwrap();
    let inspection = lab.inspection();
    fs::remove_file(&lab.slave).unwrap();
    let (_, resolution) = run(inspection);
    assert!(matches!(
        resolution,
        WhdloadLaunchTrainerResolution::NotApplicable { .. }
    ));
}

#[test]
fn a_hdf_slave_never_receives_options() {
    let lab = lab();
    fs::write(&lab.options, b"CUSTOM1=1").unwrap();
    let mut candidate = host(&lab.slave, &lab.slave_bytes);
    candidate.from_hdf = true;
    let path = lab.options.clone();
    let (plan, resolution) = plan_fsuae_whdload_launch_with_saved_trainers(
        &lab.input(AmigaEmulatorKind::FsUae),
        &inspection(vec![candidate], true),
        move |_| Ok(path),
    );
    assert!(matches!(
        resolution,
        WhdloadLaunchTrainerResolution::NotApplicable { .. }
    ));
    assert_eq!(
        plan,
        build_fsuae_whdload_command_plan(&lab.input(AmigaEmulatorKind::FsUae))
    );
}

#[test]
fn an_lha_package_launches_unchanged_and_never_takes_options() {
    let lab = lab();
    fs::write(&lab.options, b"CUSTOM1=1").unwrap();
    let package = lab.game.parent().unwrap().join("My Game.lha");
    fs::write(&package, b"lha").unwrap();
    let mut value = lab.input(AmigaEmulatorKind::FsUae);
    value.target = Some(VerifiedWHDLoadTarget {
        package_path: package.clone(),
        format: WhdloadPackageFormat::Lha,
    });
    value.slave.as_mut().unwrap().artifact_path = package.clone();
    let mut called = false;
    let (plan, resolution) =
        plan_fsuae_whdload_launch_with_saved_trainers(&value, &lab.inspection(), |_| {
            called = true;
            Ok(PathBuf::new())
        });
    assert!(!called, "an LHA launch never looks for saved options");
    assert!(matches!(
        resolution,
        WhdloadLaunchTrainerResolution::NotApplicable { .. }
    ));
    assert_eq!(plan, build_fsuae_whdload_command_plan(&value));
    assert_eq!(
        plan.command.unwrap().arguments,
        vec![
            OsString::from(format!("--config={}", lab.configuration.display())),
            OsString::from(format!("--hard-drive-0={}", package.display())),
            OsString::from("--x-whdload-args=MyGame.slave"),
        ]
    );
    // Forcing a trainer onto an LHA plan is refused by the planner itself.
    lab.save(&[("Infinite lives", true)]);
    let (_, resolution) = lab.plan();
    let plan = build_fsuae_whdload_command_plan_with_trainer(&value, resolution.trainer());
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::WhdloadTrainerOptionsRefused)
    );
}

#[test]
fn amiberry_never_receives_options_and_cannot_launch_an_installed_folder() {
    let lab = lab();
    lab.save(&[("Infinite lives", true)]);
    let path = lab.options.clone();
    let resolution = resolve_saved_whdload_launch_trainer(
        &lab.input(AmigaEmulatorKind::Amiberry),
        &lab.inspection(),
        move |_| Ok(path),
    );
    match resolution {
        WhdloadLaunchTrainerResolution::NotApplicable { reason } => {
            assert!(reason.contains("Amiberry"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
    let plan = crate::launch::amiga_whdload_command::build_amiberry_whdload_command_plan(
        &lab.input(AmigaEmulatorKind::Amiberry),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::WhdloadTargetUnsupported)
    );
}

#[test]
fn options_for_another_game_or_slave_are_refused() {
    let lab = lab();
    lab.save(&[("Infinite lives", true)]);
    // The launch input names a different verified game than the inspection.
    let mut value = lab.input(AmigaEmulatorKind::FsUae);
    value.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Amiga".into(),
        game_key: "amiga:other".into(),
    });
    value.profile.verified_identity = "amiga:other".into();
    let path = lab.options.clone();
    let resolution =
        resolve_saved_whdload_launch_trainer(&value, &lab.inspection(), move |_| Ok(path));
    assert!(matches!(
        resolution,
        WhdloadLaunchTrainerResolution::Refused { .. }
    ));
    // The launch selects another slave in the folder.
    let other = lab.game.join("Other.slave");
    fs::write(&other, &lab.slave_bytes).unwrap();
    let mut value = lab.input(AmigaEmulatorKind::FsUae);
    let slave = value.slave.as_mut().unwrap();
    slave.artifact_path = other;
    slave.name = "Other.slave".into();
    let path = lab.options.clone();
    let resolution =
        resolve_saved_whdload_launch_trainer(&value, &lab.inspection(), move |_| Ok(path));
    assert!(matches!(
        resolution,
        WhdloadLaunchTrainerResolution::Refused { .. }
    ));
    // A trainer bound to one slave is refused by the planner on another.
    let (_, resolution) = lab.plan();
    let plan = build_fsuae_whdload_command_plan_with_trainer(&value, resolution.trainer());
    assert!(plan.command.is_none());
}

#[test]
fn the_installed_folder_must_hold_the_selected_slave_directly() {
    let lab = lab();
    let mut value = lab.input(AmigaEmulatorKind::FsUae);
    value.slave.as_mut().unwrap().artifact_path = lab.game.join("sub/MyGame.slave");
    let plan = build_fsuae_whdload_command_plan(&value);
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::WhdloadSlaveNotBound)
    );
    // A symlinked folder is refused at preflight.
    let link = lab.game.parent().unwrap().join("Linked");
    std::os::unix::fs::symlink(&lab.game, &link).unwrap();
    let mut value = lab.input(AmigaEmulatorKind::FsUae);
    value.target.as_mut().unwrap().package_path = link.clone();
    value.slave.as_mut().unwrap().artifact_path = link.join("MyGame.slave");
    let plan = build_fsuae_whdload_command_plan(&value);
    assert!(crate::launch::amiga_whdload_execution::preflight_amiga_whdload_launch(&plan).is_err());
}

#[test]
fn emulator_names_and_unknown_versions_never_enable_custom_launch() {
    let lab = lab();
    lab.save(&[("Infinite lives", true), ("Infinite energy", true)]);
    for name in ["fs-uae-3.1.66", "fs-uae-9.99", "fs-uae-launcher"] {
        let mut input = lab.input(AmigaEmulatorKind::FsUae);
        input.profile.executable = Some(lab.game.join(name));
        let (plan, resolution) =
            plan_fsuae_whdload_launch_with_saved_trainers(&input, &lab.inspection(), |_| {
                Ok(lab.options.clone())
            });
        assert!(matches!(
            resolution,
            WhdloadLaunchTrainerResolution::SavedOptions(_)
        ));
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.detail.contains("trainer launch unsupported"))
        );
    }
    lab.assert_untouched();
}
