//! Shared-process execution for the pure WHDLoad command plans.

use std::fs;
use std::io::Read;

use crate::launch::amiga_whdload_command::{
    AmigaWHDLoadCommand, AmigaWHDLoadCommandPlan, WhdloadLaunchTrainer, WhdloadPackageFormat,
    fsuae_whdload_args_value,
};
use crate::launch::process_spawn::{self, PreparedProcessCommand, WatchedProcess};
use crate::patch_manager::WHDLOAD_TRAINER_MAX_CONFIG_BYTES;

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

fn real_directory(path: &std::path::Path, label: &str) -> Result<(), AmigaWHDLoadLaunchError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| AmigaWHDLoadLaunchError {
        detail: format!("{label} unavailable: {e}"),
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AmigaWHDLoadLaunchError {
            detail: format!("{label} is not a real folder"),
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
    match command.target_format {
        WhdloadPackageFormat::Lha => regular_file(&command.target, "verified WHDLoad package")?,
        WhdloadPackageFormat::InstalledDirectory => {
            real_directory(&command.target, "installed WHDLoad folder")?;
            regular_file(
                &command.target.join(&command.selected_slave),
                "selected WHDLoad slave",
            )?;
        }
    }
    if let Some(trainer) = command.trainer.as_ref().filter(|t| !t.is_empty()) {
        verify_trainer(command, trainer)?;
        // Defence in depth for a caller-constructed command or stale candidate
        // plan. Valid bytes and argv do not establish an emulator consumer.
        return Err(AmigaWHDLoadLaunchError {
            detail: super::amiga_whdload_command::WHDLOAD_TRAINER_RUNTIME_UNSUPPORTED.into(),
        });
    }
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

/// SHA-256 of a regular file, opened without following a final symlink and
/// checked on the opened handle, read with a size bound.
fn bounded_sha256(
    path: &std::path::Path,
    label: &str,
    limit: u64,
) -> Result<String, AmigaWHDLoadLaunchError> {
    use std::os::unix::fs::OpenOptionsExt;
    let unavailable = |e: std::io::Error| AmigaWHDLoadLaunchError {
        detail: format!("{label} unavailable: {e}"),
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(unavailable)?;
    if !file.metadata().map_err(unavailable)?.is_file() {
        return Err(AmigaWHDLoadLaunchError {
            detail: format!("{label} is not a safe regular file"),
        });
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| AmigaWHDLoadLaunchError {
            detail: format!("{label} unreadable: {e}"),
        })?;
    if bytes.len() as u64 > limit {
        return Err(AmigaWHDLoadLaunchError {
            detail: format!("{label} is larger than the bounded trainer check allows"),
        });
    }
    use sha2::Digest;
    Ok(sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Proves the saved trainer options a command carries are still exactly what
/// was planned: the bound slave is the command's selected slave and still has
/// the SHA-256 the options were checked against, the option file still holds
/// the bytes that were read, and the command carries precisely the expected
/// `--x-whdload-args` value. A stale or tampered selection is refused before
/// anything is spawned.
fn verify_trainer(
    command: &AmigaWHDLoadCommand,
    trainer: &WhdloadLaunchTrainer,
) -> Result<(), AmigaWHDLoadLaunchError> {
    if command.target_format != WhdloadPackageFormat::InstalledDirectory
        || trainer.slave_path != command.target.join(&command.selected_slave)
    {
        return Err(AmigaWHDLoadLaunchError {
            detail: "the saved trainer options are not bound to the slave this command runs".into(),
        });
    }
    let slave = bounded_sha256(&trainer.slave_path, "WHDLoad slave", MAX_SLAVE_BYTES)?;
    if !slave.eq_ignore_ascii_case(trainer.slave_sha256.trim()) {
        return Err(AmigaWHDLoadLaunchError {
            detail: "the slave changed since the trainer options were read; plan the launch again"
                .into(),
        });
    }
    let option_file = bounded_sha256(
        &trainer.option_file,
        "trainer option file",
        WHDLOAD_TRAINER_MAX_CONFIG_BYTES as u64,
    )?;
    if !option_file.eq_ignore_ascii_case(trainer.option_file_sha256.trim()) {
        return Err(AmigaWHDLoadLaunchError {
            detail: "the trainer option file changed since it was read; plan the launch again"
                .into(),
        });
    }
    let expected = std::ffi::OsString::from(format!(
        "--x-whdload-args={}",
        fsuae_whdload_args_value(&command.selected_slave, Some(trainer))
    ));
    if !command.arguments.contains(&expected) {
        return Err(AmigaWHDLoadLaunchError {
            detail: "the launch command does not carry the saved trainer options".into(),
        });
    }
    Ok(())
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
        // A real regular file: the preflight refuses symlinks, and /bin/true is
        // one on distributions whose coreutils is a multicall binary.
        let emulator = temp.path().join("amiberry");
        fs::write(&emulator, b"emulator fixture").unwrap();
        value.profile.executable = Some(emulator);
        let plan =
            crate::launch::amiga_whdload_command::build_amiberry_whdload_command_plan(&value);
        let command = plan.command.as_ref().unwrap();
        assert_eq!(
            preflight_amiga_whdload_launch(&plan).unwrap().arguments,
            command.arguments
        );
    }
}
