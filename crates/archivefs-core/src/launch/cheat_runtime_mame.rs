//! MAME adapter for the canonical per-launch cheat runtime.
//!
//! Translates one validated selection of MAME XML cheats into a
//! [`CheatRuntimePlan`] (a staged `<shortname>.xml` under the runtime
//! workspace) and the argv additions `-cheat -cheatpath <workspace>/cheat`.
//! Workspace, lease, receipt, fingerprints and cleanup are the canonical
//! runtime's; nothing here owns a second launcher or session type.
//!
//! MAME XML cheats start switched off, so a launch can only say "the cheat
//! system is on and the material is available". Activation is never claimed.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::cheat_launch_plan::{
    FingerprintKind, LaunchStateClass, LaunchStateExpectation, StateExpectation, ViolationSeverity,
};
use super::cheat_runtime::{
    CheatRuntimePlan, CheatRuntimePlanParts, CheatRuntimeRoots, CheatRuntimeSession, ExitSummary,
    HandoffOutcome, LiveLaunchBinding, ReceiptCheat, RuntimeFile, RuntimeFileKind, RuntimeProcess,
    RuntimeRefusal, SourceBinding, execute_cheat_runtime, workspace_name,
};
use super::mame_command::MameCommand;
use super::mame_execution::{
    MameLaunchExecutionError, MameLaunchPreflightError, MameLaunchRequest,
    preflight_and_launch_mame, preflight_mame_launch, spawn_mame,
};
use super::process_spawn::WatchedProcess;
use crate::patch_manager::{
    MAME_CHEAT_MAX_FILE_BYTES, MameCheatProvenance, MameCheatReadiness, inspect_mame_cheat,
    parse_mame_cheat_xml, render_mame_cheat_xml,
};

pub const MAME_ADAPTER_ID: &str = "mame";
const CHEAT_DIR: &str = "cheat";

impl RuntimeProcess for WatchedProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn poll_exit(&mut self) -> Option<ExitSummary> {
        let report = self.poll()?;
        Some(match &report.status {
            Ok(status) => ExitSummary {
                success: status.success(),
                code: status.code(),
            },
            Err(_) => ExitSummary {
                success: false,
                code: None,
            },
        })
    }
}

/// What a MAME cheat launch can honestly say about the cheats. There is no
/// "activated" value on purpose: MAME starts XML cheats off and EmuWiz has no
/// in-emulator evidence either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MameCheatActivation {
    CheatSystemEnabledMaterialAvailableActivationNotProven,
}

impl MameCheatActivation {
    #[must_use]
    pub fn label(self) -> &'static str {
        "MAME was started with its cheat system on and this launch's cheat file. \
         The cheat starts switched off: turn it on in MAME's Cheat menu (Tab). \
         EmuWiz has not seen it work in a game."
    }
}

/// A staged `<shortname>.xml` and the descriptions to keep from it.
#[derive(Debug, Clone)]
pub struct MameCheatSelection {
    pub staged_file: PathBuf,
    pub selected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MameCheatError {
    /// The machine shortname is not a plain MAME name.
    UnresolvedMachine(String),
    /// The cheat file is not named for the machine being launched.
    WrongMachine {
        file: String,
        set_name: String,
    },
    /// Missing, unreadable, not a regular file, or invalid cheat material.
    Material(String),
    NoCheatSelected,
    UnknownCheat(String),
    NotReady(String),
    InvalidLaunchId,
    /// The canonical runtime refused or failed; nothing was started.
    Runtime(String),
}

#[derive(Debug)]
pub enum MameCheatLaunchError {
    Preflight(MameLaunchPreflightError),
    Cheats(MameCheatError),
    Spawn(std::io::Error),
}

pub enum MameCheatLaunch {
    Plain(WatchedProcess),
    WithCheats {
        session: Box<CheatRuntimeSession<WatchedProcess>>,
        activation: MameCheatActivation,
    },
}

fn plain_shortname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Builds the runtime plan. Reads the staged file; writes nothing.
pub fn mame_runtime_plan(
    command: &MameCommand,
    cheat: &MameCheatSelection,
    launch_id: &str,
    approved_root: &Path,
) -> Result<CheatRuntimePlan, MameCheatError> {
    let set_name = &command.set_name;
    if !plain_shortname(set_name)
        || command.arguments.last() != Some(&OsString::from(set_name.as_str()))
    {
        return Err(MameCheatError::UnresolvedMachine(set_name.clone()));
    }
    let expected = format!("{set_name}.xml");
    let name = cheat
        .staged_file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name != expected {
        return Err(MameCheatError::WrongMachine {
            file: name,
            set_name: set_name.clone(),
        });
    }
    if cheat.selected.is_empty() {
        return Err(MameCheatError::NoCheatSelected);
    }
    let material = |e: String| MameCheatError::Material(e);
    let metadata =
        std::fs::symlink_metadata(&cheat.staged_file).map_err(|e| material(e.to_string()))?;
    if !metadata.is_file() {
        return Err(material("the cheat file is not a regular file".into()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&cheat.staged_file)
        .and_then(|f| {
            f.take(MAME_CHEAT_MAX_FILE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|e| material(e.to_string()))?;
    let source_sha = RuntimeFile::new(
        cheat.staged_file.clone(),
        bytes.clone(),
        RuntimeFileKind::CheatMaterial,
    )
    .sha256;
    let mut file = parse_mame_cheat_xml(&bytes, set_name, MameCheatProvenance::LocalImport)
        .map_err(|e| material(e.to_string()))?;
    file.cheats
        .retain(|c| cheat.selected.contains(&c.description));
    for description in &cheat.selected {
        if !file.cheats.iter().any(|c| &c.description == description) {
            return Err(MameCheatError::UnknownCheat(description.clone()));
        }
    }
    let inspection = inspect_mame_cheat(&file, set_name);
    if !matches!(
        inspection.readiness,
        MameCheatReadiness::ReadyNative | MameCheatReadiness::ReadyWithOpaqueNativeOps
    ) {
        return Err(MameCheatError::NotReady(format!(
            "{:?}",
            inspection.readiness
        )));
    }

    let root = approved_root.join(workspace_name(launch_id));
    let cheat_dir = root.join(CHEAT_DIR);
    let derivative = RuntimeFile::new(
        cheat_dir.join(&expected),
        render_mame_cheat_xml(&file).into_bytes(),
        RuntimeFileKind::CheatMaterial,
    );
    let expect =
        |path: PathBuf, class, expectation, fingerprint, severity| LaunchStateExpectation {
            path,
            class,
            expectation,
            fingerprint,
            severity,
        };
    let expectations = vec![
        expect(
            cheat.staged_file.clone(),
            LaunchStateClass::ReadOnlySource,
            StateExpectation::MustRemainUnchanged,
            FingerprintKind::Sha256,
            ViolationSeverity::LaunchAffecting,
        ),
        expect(
            command.selected_content.clone(),
            LaunchStateClass::ReadOnlySource,
            StateExpectation::MustRemainUnchanged,
            FingerprintKind::FileIdentity,
            ViolationSeverity::Corruption,
        ),
        expect(
            root.clone(),
            LaunchStateClass::EphemeralRuntime,
            StateExpectation::MustNotExistAfter,
            FingerprintKind::NotPresent,
            ViolationSeverity::Warning,
        ),
    ];
    let cheats = file
        .cheats
        .iter()
        .map(|c| ReceiptCheat {
            logical_id: c.description.clone(),
            variant_id: set_name.clone(),
            title: c.description.clone(),
            provider: "mame-xml".into(),
            source_id: expected.clone(),
        })
        .collect();
    CheatRuntimePlan::new(CheatRuntimePlanParts {
        launch_id: launch_id.to_string(),
        adapter_id: MAME_ADAPTER_ID.into(),
        game_identity: set_name.clone(),
        root,
        directories: vec![cheat_dir.clone()],
        files: vec![derivative],
        extra_arguments: vec![
            OsString::from("-cheat"),
            OsString::from("-cheatpath"),
            cheat_dir.into_os_string(),
        ],
        sources: vec![SourceBinding {
            path: cheat.staged_file.clone(),
            expected_sha256: source_sha,
        }],
        expectations,
        cheats,
    })
    .map_err(|r: RuntimeRefusal| MameCheatError::Runtime(format!("{r}")))
}

/// The canonical MAME preflight, then the canonical runtime around the same
/// spawn. With `cheat: None` this is exactly [`preflight_and_launch_mame`].
/// Anything the runtime refuses or fails stops the launch.
pub fn preflight_and_launch_mame_with_cheats(
    request: &MameLaunchRequest,
    cheat: Option<&MameCheatSelection>,
    launch_id: &str,
    roots: &CheatRuntimeRoots,
) -> Result<MameCheatLaunch, MameCheatLaunchError> {
    let Some(cheat) = cheat else {
        return preflight_and_launch_mame(request)
            .map(MameCheatLaunch::Plain)
            .map_err(|e| match e {
                MameLaunchExecutionError::Preflight(p) => MameCheatLaunchError::Preflight(p),
                MameLaunchExecutionError::Spawn(s) => MameCheatLaunchError::Spawn(s),
            });
    };
    let command = preflight_mame_launch(request).map_err(MameCheatLaunchError::Preflight)?;
    let plan = mame_runtime_plan(&command, cheat, launch_id, &roots.approved_root)
        .map_err(MameCheatLaunchError::Cheats)?;
    let binding = LiveLaunchBinding {
        game_identity: command.set_name.clone(),
        adapter_id: MAME_ADAPTER_ID.into(),
    };
    let session = execute_cheat_runtime(
        &plan,
        &binding,
        roots,
        command,
        |mut command: MameCommand, extra: &[OsString]| {
            // MAME takes the machine shortname last.
            let at = command.arguments.len() - 1;
            command.arguments.splice(at..at, extra.iter().cloned());
            command
        },
        |command| spawn_mame(&command).map_err(|e| e.to_string()),
    );
    if !matches!(session.receipt().handoff, HandoffOutcome::PassedToEmulator) {
        return Err(MameCheatLaunchError::Cheats(MameCheatError::Runtime(
            format!("{:?}", session.receipt().handoff),
        )));
    }
    Ok(MameCheatLaunch::WithCheats {
        session: Box::new(session),
        activation: MameCheatActivation::CheatSystemEnabledMaterialAvailableActivationNotProven,
    })
}

#[cfg(test)]
mod tests;
