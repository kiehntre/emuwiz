//! Pure argv planning for already-verified WHDLoad packages.
//!
//! The planner deliberately does not parse a slave, discover a package, infer
//! a machine, or manufacture memory/Kickstart settings. Those facts arrive
//! from the existing identity/profile/readiness stack.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::identity_source::whdload::ParsedWHDLoadSlave;
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{AmigaEmulatorKind, AmigaKickstartState};

pub const AMIGA_PLATFORM_ID: &str = "Amiga";

/// EmuWiz currently launches plain emulator binaries, not FS-UAE Launcher.
/// No version string or accepted CLI key proves a WHDLoad bootstrap consumer.
pub const WHDLOAD_TRAINER_RUNTIME_UNSUPPORTED: &str = "WHDLoad trainer launch unsupported: plain FS-UAE does not consume the Launcher's x_whdload_args bootstrap channel; Amiberry CUSTOM launch has not been verified. Saved options are not passed to the emulator.";

pub fn whdload_trainer_runtime_support(emulator: AmigaEmulatorKind) -> &'static str {
    match emulator {
        AmigaEmulatorKind::FsUae => {
            "FS-UAE: CUSTOM trainer launch unsupported in EmuWiz's plain-binary route (FS-UAE Launcher is not integrated)."
        }
        AmigaEmulatorKind::Amiberry => {
            "Amiberry: CUSTOM trainer launch unsupported; no verified option channel is integrated."
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhdloadPackageFormat {
    Lha,
    /// An installed (extracted) WHDLoad game folder holding the selected host
    /// slave directly. FS-UAE only: it is passed as a folder hard drive on the
    /// same `--x-whdload-args` channel the LHA contract uses. This is the only
    /// format for which saved trainer choices can be resolved, because only a host
    /// slave can be re-hashed against the declarations the options came from.
    InstalledDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWHDLoadTarget {
    pub package_path: PathBuf,
    pub format: WhdloadPackageFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedWHDLoadSlave {
    /// The exact selected artifact path for loose-install provenance.
    pub artifact_path: PathBuf,
    /// The exact slave basename passed to FS-UAE's WHDLoad bridge.
    pub name: String,
    pub parsed: ParsedWHDLoadSlave,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WHDLoadProfileInput {
    pub emulator: AmigaEmulatorKind,
    pub profile_id: String,
    pub executable: Option<PathBuf>,
    pub configuration: Option<PathBuf>,
    pub candidate_count: usize,
    pub eligible: bool,
    pub kickstart: AmigaKickstartState,
    pub verified_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WHDLoadLaunchInput {
    pub identity: CanonicalIdentityStatus,
    pub target: Option<VerifiedWHDLoadTarget>,
    pub slave: Option<SelectedWHDLoadSlave>,
    pub profile: WHDLoadProfileInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WHDLoadRequirements {
    pub base_memory_bytes: u32,
    /// Raw ws_ExpMem bits, including the signed optional-size encoding.
    /// Historical field name retained; no allocation policy is inferred.
    pub expanded_memory_flag: Option<u32>,
    pub kickstart_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaWHDLoadCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub preview: String,
    pub profile_id: String,
    pub profile_configuration: PathBuf,
    pub target: PathBuf,
    pub target_format: WhdloadPackageFormat,
    pub selected_slave: String,
    pub requirements: WHDLoadRequirements,
    /// Saved trainer options this command carries. The canonical preflight
    /// re-proves them (slave and option-file hashes, exact argv) whenever
    /// present, so no launch path can spawn them unchecked.
    pub trainer: Option<WhdloadLaunchTrainer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaWHDLoadCommandPlan {
    pub command: Option<AmigaWHDLoadCommand>,
    pub blockers: Vec<LaunchBlocker>,
    pub readiness: LaunchReadiness,
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

fn common_blockers(input: &WHDLoadLaunchInput, emulator: AmigaEmulatorKind) -> Vec<LaunchBlocker> {
    let mut blockers = Vec::new();
    match &input.identity {
        CanonicalIdentityStatus::Resolved(identity)
            if identity.platform_id == AMIGA_PLATFORM_ID => {}
        CanonicalIdentityStatus::Resolved(identity) => blockers.push(blocker(
            LaunchBlockerKind::AmiberryPlatformMismatch,
            format!("resolved platform is {}, not Amiga", identity.platform_id),
        )),
        CanonicalIdentityStatus::Unknown => blockers.push(blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "verified Amiga identity is unavailable",
        )),
        CanonicalIdentityStatus::Conflicting => blockers.push(blocker(
            LaunchBlockerKind::IdentityConflict,
            "verified Amiga identity is conflicting",
        )),
    }
    if input.profile.emulator != emulator {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadProfileMissing,
            "selected profile belongs to a different Amiga emulator",
        ));
    }
    if input.profile.candidate_count != 1 {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadProfileAmbiguous,
            "exactly one eligible emulator profile is required",
        ));
    }
    if !input.profile.eligible {
        blockers.push(blocker(
            LaunchBlockerKind::ProfileIneligible,
            "selected emulator profile is not eligible",
        ));
    }
    if input.profile.verified_identity.trim().is_empty() {
        blockers.push(blocker(
            LaunchBlockerKind::IdentityUnresolved,
            "profile has no verified WHDLoad identity binding",
        ));
    }
    if input.profile.executable.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadExecutableMissing,
            "emulator executable is missing",
        ));
    }
    // A configuration is passed positionally, so it must be an absolute
    // path that can never be read as an option.
    if input
        .profile
        .configuration
        .as_ref()
        .is_none_or(|configuration| !configuration.is_absolute())
    {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadProfileMissing,
            "emulator configuration is missing or not an absolute path",
        ));
    }
    if matches!(
        input.profile.kickstart,
        AmigaKickstartState::Missing
            | AmigaKickstartState::Unreadable
            | AmigaKickstartState::NotConfigured
    ) {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadKickstartUnavailable,
            "required Kickstart readiness gate is not satisfied",
        ));
    }
    let Some(target) = input.target.as_ref() else {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadTargetMissing,
            "no verified WHDLoad package target was supplied",
        ));
        return blockers;
    };
    if !target.package_path.is_absolute() || target.package_path.as_os_str().is_empty() {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadTargetMissing,
            "verified WHDLoad package path must be an absolute path",
        ));
    }
    match (target.format, emulator) {
        (WhdloadPackageFormat::Lha, _)
        | (WhdloadPackageFormat::InstalledDirectory, AmigaEmulatorKind::FsUae) => {}
        (WhdloadPackageFormat::InstalledDirectory, AmigaEmulatorKind::Amiberry) => {
            blockers.push(blocker(
                LaunchBlockerKind::WhdloadTargetUnsupported,
                "Amiberry has no proven launch contract for an installed WHDLoad folder",
            ))
        }
    }
    let Some(slave) = input.slave.as_ref() else {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadSlaveMissing,
            "a selected WHDLoad slave is required",
        ));
        return blockers;
    };
    if slave.name.trim().is_empty()
        || slave.name.contains('/')
        || slave.name.contains('\\')
        || !slave.artifact_path.is_absolute()
    {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadSlaveNotBound,
            "selected slave must be an explicit basename bound to an absolute artifact path",
        ));
    }
    if target.format == WhdloadPackageFormat::InstalledDirectory
        && (slave.artifact_path.parent() != Some(target.package_path.as_path())
            || slave.artifact_path.file_name() != Some(std::ffi::OsStr::new(&slave.name)))
    {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadSlaveNotBound,
            "the selected slave must be a file directly inside the installed WHDLoad folder",
        ));
    }
    blockers
}

fn requirements(slave: &SelectedWHDLoadSlave) -> WHDLoadRequirements {
    WHDLoadRequirements {
        base_memory_bytes: slave.parsed.base_mem_size,
        expanded_memory_flag: slave.parsed.exp_mem,
        kickstart_name: slave.parsed.kick_name.clone(),
    }
}

fn quote_preview(value: &OsString) -> String {
    let value = value.to_string_lossy();
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-._/:=".contains(c))
    {
        value.into_owned()
    } else {
        format!("{:?}", value)
    }
}

fn preview(executable: &Path, args: &[OsString]) -> String {
    std::iter::once(executable.as_os_str().to_os_string())
        .chain(args.iter().cloned())
        .map(|arg| quote_preview(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn build_amiberry_whdload_command_plan(input: &WHDLoadLaunchInput) -> AmigaWHDLoadCommandPlan {
    let blockers = common_blockers(input, AmigaEmulatorKind::Amiberry);
    if !blockers.is_empty() {
        return AmigaWHDLoadCommandPlan {
            command: None,
            blockers,
            readiness: LaunchReadiness::Blocked,
        };
    }
    let target = input.target.as_ref().expect("target checked");
    let slave = input.slave.as_ref().expect("slave checked");
    let executable = input
        .profile
        .executable
        .as_ref()
        .expect("executable checked")
        .clone();
    let configuration = input
        .profile
        .configuration
        .as_ref()
        .expect("configuration checked")
        .clone();
    let arguments = vec![
        OsString::from("--config"),
        configuration.clone().into_os_string(),
        OsString::from("--autoload"),
        target.package_path.clone().into_os_string(),
    ];
    let command = AmigaWHDLoadCommand {
        executable: executable.clone(),
        arguments: arguments.clone(),
        working_directory: target.package_path.parent().map(Path::to_path_buf),
        preview: preview(&executable, &arguments),
        profile_id: input.profile.profile_id.clone(),
        profile_configuration: configuration.clone(),
        target: target.package_path.clone(),
        target_format: target.format,
        selected_slave: slave.name.clone(),
        requirements: requirements(slave),
        trainer: None,
    };
    AmigaWHDLoadCommandPlan {
        command: Some(command),
        blockers: Vec::new(),
        readiness: LaunchReadiness::Ready,
    }
}

/// Saved WHDLoad trainer options for one game, as `CUSTOMn=value` arguments.
/// Normally produced by
/// [`resolve_saved_whdload_launch_trainer`](crate::launch::resolve_saved_whdload_launch_trainer).
///
/// The selection is bound to the verified game identity, to the exact host
/// slave it was declared by and to the exact option-file bytes it was read
/// from, so a selection for another game, a replaced slave or an option file
/// edited since planning is refused. This is a saved-choice projection, not
/// evidence of a runtime consumer. Planning and preflight currently refuse
/// nonempty projections. Nothing here edits a game or configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadLaunchTrainer {
    pub verified_game_identity: String,
    pub slave_name: String,
    /// The host slave file the options were declared by.
    pub slave_path: PathBuf,
    /// SHA-256 of that slave (re-checked at preflight).
    pub slave_sha256: String,
    /// EmuWiz's per-game option file the values were read from.
    pub option_file: PathBuf,
    /// SHA-256 of the option-file bytes read (re-checked at preflight).
    pub option_file_sha256: String,
    pub arguments: Vec<String>,
}

const MAX_TRAINER_ARGUMENTS: usize = 32;

fn safe_trainer_argument(argument: &str) -> bool {
    let Some((slot, value)) = argument.split_once('=') else {
        return false;
    };
    matches!(
        slot,
        "CUSTOM1" | "CUSTOM2" | "CUSTOM3" | "CUSTOM4" | "CUSTOM5"
    ) && !value.is_empty()
        && value.len() <= 10
        && value.bytes().all(|b| b.is_ascii_digit())
}

impl WhdloadLaunchTrainer {
    pub fn is_empty(&self) -> bool {
        self.arguments.is_empty()
    }

    fn arguments_are_safe(&self) -> bool {
        let mut slots = std::collections::BTreeSet::new();
        self.arguments.len() <= MAX_TRAINER_ARGUMENTS
            && self.arguments.iter().all(|argument| {
                safe_trainer_argument(argument)
                    && slots.insert(argument.split('=').next().unwrap_or_default().to_string())
            })
    }
}

/// The exact value of FS-UAE's `--x-whdload-args`: the slave name, then the
/// selected options in order. With no trainer it is the slave name alone.
pub fn fsuae_whdload_args_value(
    slave_name: &str,
    trainer: Option<&WhdloadLaunchTrainer>,
) -> String {
    let mut value = slave_name.to_string();
    for argument in trainer.map(|t| t.arguments.as_slice()).unwrap_or_default() {
        value.push(' ');
        value.push_str(argument);
    }
    value
}

pub fn build_fsuae_whdload_command_plan(input: &WHDLoadLaunchInput) -> AmigaWHDLoadCommandPlan {
    build_fsuae_whdload_command_plan_with_trainer(input, None)
}

/// Validates exact-game trainer bindings, then refuses nonempty options:
/// plain FS-UAE has no verified consumer for the Launcher bootstrap channel.
/// `None` or an empty selection produces exactly the ordinary command.
pub fn build_fsuae_whdload_command_plan_with_trainer(
    input: &WHDLoadLaunchInput,
    trainer: Option<&WhdloadLaunchTrainer>,
) -> AmigaWHDLoadCommandPlan {
    let trainer = trainer.filter(|trainer| !trainer.is_empty());
    let mut blockers = common_blockers(input, AmigaEmulatorKind::FsUae);
    if let Some(trainer) = trainer {
        if let (CanonicalIdentityStatus::Resolved(resolved), Some(slave)) =
            (&input.identity, input.slave.as_ref())
        {
            if trainer.verified_game_identity != resolved.game_key
                || trainer.verified_game_identity != input.profile.verified_identity
            {
                blockers.push(blocker(
                    LaunchBlockerKind::IdentityConflict,
                    "the selected trainer options were made for a different game",
                ));
            }
            if trainer.slave_name != slave.name
                || trainer.slave_path != slave.artifact_path
                || trainer.slave_sha256.trim().is_empty()
                || trainer.option_file_sha256.trim().is_empty()
            {
                blockers.push(blocker(
                    LaunchBlockerKind::WhdloadTrainerOptionsRefused,
                    "the saved trainer options are not bound to this slave",
                ));
            }
            // The options share one space-separated WHDLoad argument string
            // with the slave name, so the name must not be able to split or
            // smuggle extra arguments.
            if slave
                .name
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '=' | '"' | '\''))
            {
                blockers.push(blocker(
                    LaunchBlockerKind::WhdloadTrainerOptionsRefused,
                    "the slave name cannot carry trainer options safely (spaces, quotes or '=')",
                ));
            }
        }
        // An LHA package runs the slave inside the archive, which is not the
        // host file the options were checked against.
        if input
            .target
            .as_ref()
            .is_some_and(|target| target.format != WhdloadPackageFormat::InstalledDirectory)
        {
            blockers.push(blocker(
                LaunchBlockerKind::WhdloadTrainerOptionsRefused,
                "trainer options apply only to an installed WHDLoad folder, not a packaged game",
            ));
        }
        if !trainer.arguments_are_safe() {
            blockers.push(blocker(
                LaunchBlockerKind::WhdloadTrainerOptionsRefused,
                "the saved trainer options are not a safe CUSTOMn=number list",
            ));
        }
    }
    if trainer.is_some() {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadTrainerOptionsRefused,
            WHDLOAD_TRAINER_RUNTIME_UNSUPPORTED,
        ));
    }
    if !blockers.is_empty() {
        return AmigaWHDLoadCommandPlan {
            command: None,
            blockers,
            readiness: LaunchReadiness::Blocked,
        };
    }
    let target = input.target.as_ref().expect("target checked");
    let slave = input.slave.as_ref().expect("slave checked");
    let executable = input
        .profile
        .executable
        .as_ref()
        .expect("executable checked")
        .clone();
    let configuration = input
        .profile
        .configuration
        .as_ref()
        .expect("configuration checked");
    // The selected configuration is FS-UAE's first positional argument:
    // FS-UAE ignores `--config=` and would start with its default setup.
    let mut arguments = vec![
        crate::launch::fsuae_command::fsuae_config_argument(configuration),
        crate::launch::fsuae_command::fsuae_option("--hard-drive-0=", &target.package_path),
    ];
    // FS-UAE mounts a folder hard drive writable by default; the original
    // installed game must never be modified by the emulated Amiga.
    if target.format == WhdloadPackageFormat::InstalledDirectory {
        arguments.push(OsString::from("--hard-drive-0-read-only=1"));
    }
    arguments.push(OsString::from(format!(
        "--x-whdload-args={}",
        fsuae_whdload_args_value(&slave.name, trainer)
    )));
    let command = AmigaWHDLoadCommand {
        executable: executable.clone(),
        arguments: arguments.clone(),
        working_directory: None,
        preview: preview(&executable, &arguments),
        profile_id: input.profile.profile_id.clone(),
        profile_configuration: configuration.clone(),
        target: target.package_path.clone(),
        target_format: target.format,
        selected_slave: slave.name.clone(),
        requirements: requirements(slave),
        trainer: trainer.cloned(),
    };
    AmigaWHDLoadCommandPlan {
        command: Some(command),
        blockers: Vec::new(),
        readiness: LaunchReadiness::Ready,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};

    pub(crate) fn input(emulator: AmigaEmulatorKind) -> WHDLoadLaunchInput {
        WHDLoadLaunchInput {
            identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "Amiga".into(),
                game_key: "game".into(),
            }),
            target: Some(VerifiedWHDLoadTarget {
                package_path: "/games/My Game.lha".into(),
                format: WhdloadPackageFormat::Lha,
            }),
            slave: Some(SelectedWHDLoadSlave {
                artifact_path: "/games/My Game.slave".into(),
                name: "My Game.Slave".into(),
                parsed: ParsedWHDLoadSlave {
                    runtime_version: 18,
                    struct_size: 64,
                    flags: 0,
                    base_mem_size: 512 * 1024,
                    exec_install: 0,
                    game_loader: 0,
                    current_dir: None,
                    dont_cache: None,
                    key_debug: None,
                    key_exit: None,
                    exp_mem: None,
                    name: None,
                    copyright: None,
                    info: None,
                    kick_name: None,
                    kick_size: None,
                    kick_crc: None,
                    config: None,
                    extension_bytes: vec![],
                },
            }),
            profile: WHDLoadProfileInput {
                emulator,
                profile_id: "profile".into(),
                executable: Some("/bin/true".into()),
                configuration: Some("/games/profile.uae".into()),
                candidate_count: 1,
                eligible: true,
                kickstart: AmigaKickstartState::PresentUnverified,
                verified_identity: "game".into(),
            },
        }
    }
    #[test]
    fn amiberry_uses_documented_autoload_argv() {
        let plan = build_amiberry_whdload_command_plan(&input(AmigaEmulatorKind::Amiberry));
        assert_eq!(plan.readiness, LaunchReadiness::Ready);
        assert_eq!(
            plan.command.unwrap().arguments,
            vec![
                OsString::from("--config"),
                OsString::from("/games/profile.uae"),
                OsString::from("--autoload"),
                OsString::from("/games/My Game.lha")
            ]
        );
    }
    #[test]
    fn fsuae_uses_profile_hard_drive_and_slave_argv() {
        let plan = build_fsuae_whdload_command_plan(&input(AmigaEmulatorKind::FsUae));
        assert_eq!(
            plan.command.unwrap().arguments,
            vec![
                // Positional: FS-UAE ignores an unknown `--config=` option.
                OsString::from("/games/profile.uae"),
                OsString::from("--hard-drive-0=/games/My Game.lha"),
                OsString::from("--x-whdload-args=My Game.Slave")
            ]
        );
    }
    #[test]
    fn a_relative_configuration_is_never_passed_positionally() {
        let mut value = input(AmigaEmulatorKind::FsUae);
        value.profile.configuration = Some("-profile.uae".into());
        let plan = build_fsuae_whdload_command_plan(&value);
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::WhdloadProfileMissing)
        );
    }

    #[test]
    fn fsuae_whdload_selects_the_configuration_positionally_for_every_target() {
        for value in [
            input(AmigaEmulatorKind::FsUae),
            installed(AmigaEmulatorKind::FsUae),
        ] {
            let selected = value.profile.configuration.clone().unwrap();
            let arguments = build_fsuae_whdload_command_plan(&value)
                .command
                .expect("valid FS-UAE WHDLoad plan")
                .arguments;
            assert_eq!(arguments[0], selected.into_os_string());
            // FS-UAE silently ignores `--config=`; never emit it again.
            assert!(
                !arguments
                    .iter()
                    .any(|a| a.to_string_lossy().starts_with("--config")),
                "{arguments:?}"
            );
        }
    }
    fn trainer(arguments: &[&str]) -> WhdloadLaunchTrainer {
        WhdloadLaunchTrainer {
            verified_game_identity: "game".into(),
            slave_name: "MyGame.Slave".into(),
            slave_path: "/games/My Game/MyGame.Slave".into(),
            slave_sha256: "ab".repeat(32),
            option_file: "/data/whdload-trainer-options/x.options".into(),
            option_file_sha256: "cd".repeat(32),
            arguments: arguments.iter().map(|a| a.to_string()).collect(),
        }
    }
    /// The same game installed as a folder: the only target trainers apply to.
    fn installed(emulator: AmigaEmulatorKind) -> WHDLoadLaunchInput {
        let mut value = input(emulator);
        value.target = Some(VerifiedWHDLoadTarget {
            package_path: "/games/My Game".into(),
            format: WhdloadPackageFormat::InstalledDirectory,
        });
        let slave = value.slave.as_mut().unwrap();
        slave.artifact_path = "/games/My Game/MyGame.Slave".into();
        slave.name = "MyGame.Slave".into();
        value
    }
    fn fsuae_args(value: &WHDLoadLaunchInput, t: Option<&WhdloadLaunchTrainer>) -> Vec<OsString> {
        build_fsuae_whdload_command_plan_with_trainer(value, t)
            .command
            .expect("plan should be ready")
            .arguments
    }
    #[test]
    fn selected_trainers_are_refused_without_a_verified_runtime_channel() {
        let value = installed(AmigaEmulatorKind::FsUae);
        let plan = build_fsuae_whdload_command_plan_with_trainer(
            &value,
            Some(&trainer(&["CUSTOM1=1", "CUSTOM2=5"])),
        );
        assert!(plan.command.is_none());
        assert_eq!(plan.readiness, LaunchReadiness::Blocked);
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.detail == WHDLOAD_TRAINER_RUNTIME_UNSUPPORTED)
        );
    }
    #[test]
    fn no_or_empty_trainer_leaves_the_launch_unchanged() {
        let value = installed(AmigaEmulatorKind::FsUae);
        let baseline = build_fsuae_whdload_command_plan(&value);
        assert_eq!(
            fsuae_args(&value, None),
            baseline.command.clone().unwrap().arguments
        );
        assert_eq!(
            build_fsuae_whdload_command_plan_with_trainer(&value, Some(&trainer(&[]))),
            baseline
        );
    }
    #[test]
    fn a_selection_for_another_game_or_slave_is_refused() {
        let value = installed(AmigaEmulatorKind::FsUae);
        let mut other_game = trainer(&["CUSTOM1=1"]);
        other_game.verified_game_identity = "other".into();
        let plan = build_fsuae_whdload_command_plan_with_trainer(&value, Some(&other_game));
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::IdentityConflict)
        );
        let mut other_slave = trainer(&["CUSTOM1=1"]);
        other_slave.slave_name = "Other.Slave".into();
        let plan = build_fsuae_whdload_command_plan_with_trainer(&value, Some(&other_slave));
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::WhdloadTrainerOptionsRefused)
        );
    }
    #[test]
    fn unsafe_or_duplicate_options_are_refused() {
        let value = installed(AmigaEmulatorKind::FsUae);
        for bad in [
            vec!["CUSTOM1=1 CUSTOM2=2"],
            vec!["CUSTOM1=$(x)"],
            vec!["custom1=1"],
            vec!["CUSTOM1="],
            vec!["CUSTOM1=1", "CUSTOM1=2"],
            vec!["NOEQUALS"],
            vec!["CUSTOM6=1"],
            vec!["SPLASHDELAY=1"],
        ] {
            let plan = build_fsuae_whdload_command_plan_with_trainer(&value, Some(&trainer(&bad)));
            assert!(plan.command.is_none(), "{bad:?} must be refused");
        }
    }
    #[test]
    fn a_slave_name_that_could_split_the_whdload_arguments_refuses_trainers() {
        for name in [
            "My Game.Slave",
            "Game.Slave CUSTOM2=99",
            "a=b.Slave",
            "q\"t.Slave",
        ] {
            let mut value = installed(AmigaEmulatorKind::FsUae);
            let slave = value.slave.as_mut().unwrap();
            slave.name = name.into();
            slave.artifact_path = PathBuf::from("/games/My Game").join(name);
            let mut selected = trainer(&["CUSTOM1=1"]);
            selected.slave_name = name.into();
            selected.slave_path = slave.artifact_path.clone();
            let plan = build_fsuae_whdload_command_plan_with_trainer(&value, Some(&selected));
            assert!(plan.command.is_none(), "{name}");
            // Without trainer options the existing launch is unchanged.
            assert!(build_fsuae_whdload_command_plan(&value).command.is_some());
        }
    }
    #[test]
    fn trainer_options_never_change_the_amiberry_command() {
        let value = input(AmigaEmulatorKind::Amiberry);
        assert_eq!(
            build_amiberry_whdload_command_plan(&value)
                .command
                .unwrap()
                .arguments
                .len(),
            4
        );
    }
    #[test]
    fn missing_slave_is_blocked() {
        let mut value = input(AmigaEmulatorKind::Amiberry);
        value.slave = None;
        assert!(
            build_amiberry_whdload_command_plan(&value)
                .blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::WhdloadSlaveMissing)
        );
    }
    #[test]
    fn missing_kickstart_is_blocked() {
        let mut value = input(AmigaEmulatorKind::Amiberry);
        value.profile.kickstart = AmigaKickstartState::Missing;
        assert!(
            build_amiberry_whdload_command_plan(&value)
                .blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::WhdloadKickstartUnavailable)
        );
    }
    #[test]
    fn ambiguous_profile_is_blocked() {
        let mut value = input(AmigaEmulatorKind::Amiberry);
        value.profile.candidate_count = 2;
        assert_eq!(
            build_amiberry_whdload_command_plan(&value).readiness,
            LaunchReadiness::Blocked
        );
    }
    #[test]
    fn unknown_memory_is_not_fabricated() {
        let mut value = input(AmigaEmulatorKind::Amiberry);
        value.slave.as_mut().unwrap().parsed.base_mem_size = 0;
        value.slave.as_mut().unwrap().parsed.exp_mem = None;
        assert_eq!(
            build_amiberry_whdload_command_plan(&value)
                .command
                .unwrap()
                .requirements
                .expanded_memory_flag,
            None
        );
    }
    #[test]
    fn optional_expansion_memory_preserves_raw_bits_without_changing_arguments() {
        let mut value = input(AmigaEmulatorKind::Amiberry);
        let baseline = build_amiberry_whdload_command_plan(&value).command.unwrap();
        value.slave.as_mut().unwrap().parsed.exp_mem = Some(0xfff8_0000);
        let planned = build_amiberry_whdload_command_plan(&value).command.unwrap();
        assert_eq!(planned.requirements.expanded_memory_flag, Some(0xfff8_0000));
        assert_eq!(planned.arguments, baseline.arguments);
    }
    #[test]
    fn wrong_platform_is_blocked() {
        let mut value = input(AmigaEmulatorKind::Amiberry);
        value.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "DOS".into(),
            game_key: "game".into(),
        });
        assert_eq!(
            build_amiberry_whdload_command_plan(&value).readiness,
            LaunchReadiness::Blocked
        );
    }
}
