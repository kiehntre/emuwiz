//! Shared-process execution for the pure WHDLoad command plans.

use std::fs;

use crate::launch::amiga_whdload_command::{
    AmigaWHDLoadCommand, AmigaWHDLoadCommandPlan, WhdloadLaunchTrainer, fsuae_whdload_args_value,
};
use crate::launch::process_spawn::{self, PreparedProcessCommand, WatchedProcess};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaWHDLoadLaunchError {
    pub detail: String,
}

pub struct LaunchedAmigaWHDLoadProcess {
    pub pid: u32,
    pub command: PreparedProcessCommand,
    watched: WatchedProcess,
}

impl LaunchedAmigaWHDLoadProcess {
    pub fn poll(&mut self) -> Option<&process_spawn::ProcessExitReport> {
        self.watched.poll()
    }
    pub fn is_running(&self) -> bool {
        self.watched.is_running()
    }
}

fn prepared(command: &AmigaWHDLoadCommand) -> PreparedProcessCommand {
    PreparedProcessCommand {
        executable: command.executable.clone(),
        arguments: command.arguments.clone(),
        working_directory: command.working_directory.clone(),
    }
}

fn regular_file(path: &std::path::Path, label: &str) -> Result<(), AmigaWHDLoadLaunchError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| AmigaWHDLoadLaunchError {
        detail: format!("{label} unavailable: {e}"),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AmigaWHDLoadLaunchError {
            detail: format!("{label} is not a safe regular file"),
        });
    }
    Ok(())
}

pub fn preflight_amiga_whdload_launch(
    plan: &AmigaWHDLoadCommandPlan,
) -> Result<PreparedProcessCommand, AmigaWHDLoadLaunchError> {
    let command = plan
        .command
        .as_ref()
        .ok_or_else(|| AmigaWHDLoadLaunchError {
            detail: format!("WHDLoad launch blocked: {:?}", plan.blockers),
        })?;
    regular_file(&command.executable, "emulator executable")?;
    regular_file(&command.profile_configuration, "WHDLoad emulator profile")?;
    regular_file(&command.target, "verified WHDLoad package")?;
    Ok(prepared(command))
}

pub fn spawn_amiga_whdload(
    command: PreparedProcessCommand,
) -> Result<LaunchedAmigaWHDLoadProcess, AmigaWHDLoadLaunchError> {
    let watched =
        process_spawn::spawn_watched_process(&command).map_err(|e| AmigaWHDLoadLaunchError {
            detail: format!("could not spawn WHDLoad emulator: {e}"),
        })?;
    let pid = watched.pid;
    Ok(LaunchedAmigaWHDLoadProcess {
        pid,
        command,
        watched,
    })
}

pub fn preflight_and_launch_amiga_whdload(
    plan: &AmigaWHDLoadCommandPlan,
) -> Result<LaunchedAmigaWHDLoadProcess, AmigaWHDLoadLaunchError> {
    spawn_amiga_whdload(preflight_amiga_whdload_launch(plan)?)
}

/// Largest slave file hashed when re-proving a trainer selection.
const MAX_SLAVE_BYTES: u64 = 16 * 1024 * 1024;

/// Preflight for a launch that carries trainer options. Runs the canonical
/// preflight first, then proves the selection is still exactly what was
/// planned: the selected slave file is a safe regular file whose SHA-256 still
/// equals the one the options were declared against, and the command carries
/// precisely the expected `--x-whdload-args` value. A stale or tampered
/// selection is refused before anything is spawned.
pub fn preflight_amiga_whdload_launch_with_trainer(
    plan: &AmigaWHDLoadCommandPlan,
    trainer: Option<&WhdloadLaunchTrainer>,
    slave_artifact: &std::path::Path,
) -> Result<PreparedProcessCommand, AmigaWHDLoadLaunchError> {
    let prepared = preflight_amiga_whdload_launch(plan)?;
    let Some(trainer) = trainer.filter(|trainer| !trainer.is_empty()) else {
        return Ok(prepared);
    };
    let command = plan
        .command
        .as_ref()
        .expect("preflight checked the command");
    regular_file(slave_artifact, "WHDLoad slave")?;
    let size = fs::metadata(slave_artifact)
        .map_err(|e| AmigaWHDLoadLaunchError {
            detail: format!("WHDLoad slave unavailable: {e}"),
        })?
        .len();
    if size > MAX_SLAVE_BYTES {
        return Err(AmigaWHDLoadLaunchError {
            detail: "WHDLoad slave is larger than the bounded trainer check allows".into(),
        });
    }
    let bytes = fs::read(slave_artifact).map_err(|e| AmigaWHDLoadLaunchError {
        detail: format!("WHDLoad slave unreadable: {e}"),
    })?;
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&bytes);
    let actual = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if !actual.eq_ignore_ascii_case(trainer.slave_sha256.trim()) {
        return Err(AmigaWHDLoadLaunchError {
            detail: "the slave changed since the trainer options were selected; select them again"
                .into(),
        });
    }
    let expected = std::ffi::OsString::from(format!(
        "--x-whdload-args={}",
        fsuae_whdload_args_value(&command.selected_slave, Some(trainer))
    ));
    if !prepared.arguments.contains(&expected) {
        return Err(AmigaWHDLoadLaunchError {
            detail: "the launch command does not carry the selected trainer options".into(),
        });
    }
    Ok(prepared)
}

pub fn preflight_and_launch_amiga_whdload_with_trainer(
    plan: &AmigaWHDLoadCommandPlan,
    trainer: Option<&WhdloadLaunchTrainer>,
    slave_artifact: &std::path::Path,
) -> Result<LaunchedAmigaWHDLoadProcess, AmigaWHDLoadLaunchError> {
    spawn_amiga_whdload(preflight_amiga_whdload_launch_with_trainer(
        plan,
        trainer,
        slave_artifact,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::amiga_whdload_command::tests::input;
    use crate::patch_manager::AmigaEmulatorKind;
    #[test]
    fn executor_receives_exact_planner_argv_without_shell() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("My Game.lha");
        let configuration = temp.path().join("profile.uae");
        fs::write(&package, b"verified package fixture").unwrap();
        fs::write(&configuration, b"profile fixture").unwrap();
        let mut value = input(AmigaEmulatorKind::Amiberry);
        value.target.as_mut().unwrap().package_path = package;
        value.profile.configuration = Some(configuration);
        let plan =
            crate::launch::amiga_whdload_command::build_amiberry_whdload_command_plan(&value);
        let command = plan.command.as_ref().unwrap();
        assert_eq!(
            preflight_amiga_whdload_launch(&plan).unwrap().arguments,
            command.arguments
        );
    }
    #[test]
    fn trainer_preflight_accepts_a_current_selection_and_refuses_a_stale_slave() {
        use crate::launch::amiga_whdload_command::{
            WhdloadLaunchTrainer, build_fsuae_whdload_command_plan_with_trainer,
        };
        use sha2::Digest;
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("My Game.lha");
        let configuration = temp.path().join("profile.uae");
        let slave = temp.path().join("My Game.slave");
        fs::write(&package, b"package").unwrap();
        fs::write(&configuration, b"profile").unwrap();
        fs::write(&slave, b"slave bytes").unwrap();
        let sha = sha2::Sha256::digest(b"slave bytes")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut value = input(AmigaEmulatorKind::FsUae);
        value.target.as_mut().unwrap().package_path = package.clone();
        value.profile.configuration = Some(configuration);
        let trainer = WhdloadLaunchTrainer {
            verified_game_identity: "game".into(),
            slave_name: "My Game.Slave".into(),
            slave_sha256: sha,
            arguments: vec!["CUSTOM1=1".into()],
        };
        let plan = build_fsuae_whdload_command_plan_with_trainer(&value, Some(&trainer));
        let prepared =
            preflight_amiga_whdload_launch_with_trainer(&plan, Some(&trainer), &slave).unwrap();
        assert!(
            prepared
                .arguments
                .iter()
                .any(|a| a == "--x-whdload-args=My Game.Slave CUSTOM1=1")
        );
        // The source package and slave are never touched.
        assert_eq!(fs::read(&package).unwrap(), b"package");
        assert_eq!(fs::read(&slave).unwrap(), b"slave bytes");
        // A replaced slave makes the selection stale.
        fs::write(&slave, b"replaced").unwrap();
        let stale =
            preflight_amiga_whdload_launch_with_trainer(&plan, Some(&trainer), &slave).unwrap_err();
        assert!(stale.detail.contains("changed since"));
        // No trainer: canonical preflight only, no slave check.
        assert!(preflight_amiga_whdload_launch_with_trainer(&plan, None, &slave).is_ok());
    }
    #[test]
    fn a_command_without_the_selected_options_is_refused_by_preflight() {
        use crate::launch::amiga_whdload_command::{
            WhdloadLaunchTrainer, build_fsuae_whdload_command_plan,
        };
        use sha2::Digest;
        let temp = tempfile::tempdir().unwrap();
        let (package, configuration, slave) = (
            temp.path().join("g.lha"),
            temp.path().join("p.uae"),
            temp.path().join("g.slave"),
        );
        for f in [&package, &configuration, &slave] {
            fs::write(f, b"x").unwrap();
        }
        let mut value = input(AmigaEmulatorKind::FsUae);
        value.target.as_mut().unwrap().package_path = package;
        value.profile.configuration = Some(configuration);
        let trainer = WhdloadLaunchTrainer {
            verified_game_identity: "game".into(),
            slave_name: "My Game.Slave".into(),
            slave_sha256: sha2::Sha256::digest(b"x")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            arguments: vec!["CUSTOM1=1".into()],
        };
        let plain_plan = build_fsuae_whdload_command_plan(&value);
        let error =
            preflight_amiga_whdload_launch_with_trainer(&plain_plan, Some(&trainer), &slave)
                .unwrap_err();
        assert!(error.detail.contains("does not carry"));
    }
}
