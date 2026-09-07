//! Shared-process execution for the pure WHDLoad command plans.

use std::fs;

use crate::launch::amiga_whdload_command::{AmigaWHDLoadCommand, AmigaWHDLoadCommandPlan};
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
}
