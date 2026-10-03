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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhdloadPackageFormat {
    Lha,
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
    pub expanded_memory_flag: Option<u8>,
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
    pub selected_slave: String,
    pub requirements: WHDLoadRequirements,
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
    if input.profile.configuration.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadProfileMissing,
            "emulator configuration is missing",
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
    if target.format != WhdloadPackageFormat::Lha {
        blockers.push(blocker(
            LaunchBlockerKind::WhdloadTargetUnsupported,
            "only the proven WHDLoad LHA package contract is supported",
        ));
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
        selected_slave: slave.name.clone(),
        requirements: requirements(slave),
    };
    AmigaWHDLoadCommandPlan {
        command: Some(command),
        blockers: Vec::new(),
        readiness: LaunchReadiness::Ready,
    }
}

/// Trainer/custom options the person selected for one WHDLoad game, already
/// rendered by `patch_manager::render_whdload_arguments` (`CUSTOMn=value`).
///
/// The selection is bound to the verified game identity and to the exact slave
/// it was made against, so a selection for another game or a replaced slave is
/// refused instead of being passed to FS-UAE. Nothing here edits the package,
/// the slave or any configuration; the options travel only on the launch
/// command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadLaunchTrainer {
    pub verified_game_identity: String,
    pub slave_name: String,
    /// SHA-256 of the slave the options were declared by (re-checked at launch).
    pub slave_sha256: String,
    pub arguments: Vec<String>,
}

const MAX_TRAINER_ARGUMENTS: usize = 32;

fn safe_trainer_argument(argument: &str) -> bool {
    let Some((slot, value)) = argument.split_once('=') else {
        return false;
    };
    !slot.is_empty()
        && slot.len() <= 16
        && slot
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !value.is_empty()
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

/// Same plan as [`build_fsuae_whdload_command_plan`], plus the selected trainer
/// options on the existing `--x-whdload-args` channel. `None` or an empty
/// selection produces exactly the baseline command.
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
            if trainer.slave_name != slave.name || trainer.slave_sha256.trim().is_empty() {
                blockers.push(blocker(
                    LaunchBlockerKind::WhdloadSlaveNotBound,
                    "the selected trainer options are not bound to this slave",
                ));
            }
        }
        if !trainer.arguments_are_safe() {
            blockers.push(blocker(
                LaunchBlockerKind::WhdloadSlaveNotBound,
                "the selected trainer options are not a safe CUSTOMn=number list",
            ));
        }
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
    let arguments = vec![
        OsString::from(format!("--config={}", configuration.display())),
        OsString::from(format!("--hard-drive-0={}", target.package_path.display())),
        OsString::from(format!(
            "--x-whdload-args={}",
            fsuae_whdload_args_value(&slave.name, trainer)
        )),
    ];
    let command = AmigaWHDLoadCommand {
        executable: executable.clone(),
        arguments: arguments.clone(),
        working_directory: None,
        preview: preview(&executable, &arguments),
        profile_id: input.profile.profile_id.clone(),
        profile_configuration: configuration.clone(),
        target: target.package_path.clone(),
        selected_slave: slave.name.clone(),
        requirements: requirements(slave),
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
                OsString::from("--config=/games/profile.uae"),
                OsString::from("--hard-drive-0=/games/My Game.lha"),
                OsString::from("--x-whdload-args=My Game.Slave")
            ]
        );
    }
    fn trainer(arguments: &[&str]) -> WhdloadLaunchTrainer {
        WhdloadLaunchTrainer {
            verified_game_identity: "game".into(),
            slave_name: "My Game.Slave".into(),
            slave_sha256: "ab".repeat(32),
            arguments: arguments.iter().map(|a| a.to_string()).collect(),
        }
    }
    fn fsuae_args(value: &WHDLoadLaunchInput, t: Option<&WhdloadLaunchTrainer>) -> Vec<OsString> {
        build_fsuae_whdload_command_plan_with_trainer(value, t)
            .command
            .expect("plan should be ready")
            .arguments
    }
    #[test]
    fn selected_trainer_options_reach_the_fsuae_argv_in_order() {
        let value = input(AmigaEmulatorKind::FsUae);
        assert_eq!(
            fsuae_args(&value, Some(&trainer(&["CUSTOM1=1", "CUSTOM2=5"]))),
            vec![
                OsString::from("--config=/games/profile.uae"),
                OsString::from("--hard-drive-0=/games/My Game.lha"),
                OsString::from("--x-whdload-args=My Game.Slave CUSTOM1=1 CUSTOM2=5")
            ]
        );
    }
    #[test]
    fn no_or_empty_trainer_leaves_the_launch_unchanged() {
        let value = input(AmigaEmulatorKind::FsUae);
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
        let value = input(AmigaEmulatorKind::FsUae);
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
                .any(|b| b.kind == LaunchBlockerKind::WhdloadSlaveNotBound)
        );
    }
    #[test]
    fn unsafe_or_duplicate_options_are_refused() {
        let value = input(AmigaEmulatorKind::FsUae);
        for bad in [
            vec!["CUSTOM1=1 CUSTOM2=2"],
            vec!["CUSTOM1=$(x)"],
            vec!["custom1=1"],
            vec!["CUSTOM1="],
            vec!["CUSTOM1=1", "CUSTOM1=2"],
            vec!["NOEQUALS"],
        ] {
            let plan = build_fsuae_whdload_command_plan_with_trainer(&value, Some(&trainer(&bad)));
            assert!(plan.command.is_none(), "{bad:?} must be refused");
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
