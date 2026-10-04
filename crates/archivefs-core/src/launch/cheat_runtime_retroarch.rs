//! RetroArch adapter for the per-launch cheat runtime (Phase 1 proof target).
//!
//! This module adds no launcher. It converts the existing planner's output
//! ([`CheatLaunchPlan`]) into the generic [`CheatRuntimePlan`], and composes the
//! canonical pieces in order:
//!
//! ```text
//! preflight_retroarch_launch        (canonical: fresh identity/content/core checks)
//! -> plan_cheat_launch              (existing planner)
//! -> execute_cheat_runtime          (validate, materialise, hand over, receipt)
//!      -> spawn_retroarch           (canonical spawn, with the extra arguments)
//! ```
//!
//! # The mechanism, and what it is not evidence of
//!
//! The planner already chose the mechanism: a generated, disposable base config
//! passed as `--config <scratch>/retroarch/profile/retroarch.cfg`, with
//! `config_save_on_exit = "false"`, save and state directories pinned to the
//! user's real ones, and a launch-owned cheat database containing only the
//! selected entries. The user's real `retroarch.cfg` is never written.
//!
//! `--config` replaces the base configuration rather than layering on it, so
//! the scratch file is *seeded* with the user's real settings (read-only,
//! bounded, minus the keys this launch owns) so controls, video and paths
//! still apply. If the real config cannot be read the launch proceeds with
//! RetroArch defaults and the receipt says so.
//!
//! The only empirical evidence for this approach is
//! `docs/research/RETROARCH_APPENDCONFIG_PERSISTENCE_TEST.md` (one core, no
//! content, RetroArch 1.22.2 Flatpak). It is not a universal guarantee, and no
//! test here shows a cheat taking effect in a game. A successful launch means
//! "RetroArch was launched with the prepared cheat configuration", nothing more.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::cheat_launch_plan::{
    CheatCandidate, CheatLaunchBlockReason, CheatLaunchPlan, CheatLaunchPlanStatus,
    CheatLaunchRequest, CheatLaunchSelection, CheatLaunchTarget, RETROARCH_MANDATORY_KEYS,
    RetroArchLaunchFacts, RetroArchOverrideFinding, RetroArchProfileIsolation, plan_cheat_launch,
};
use super::cheat_runtime::{
    CheatRuntimePlan, CheatRuntimePlanParts, CheatRuntimeReceipt, CheatRuntimeRoots,
    CheatRuntimeSession, ExitSummary, LiveLaunchBinding, ReceiptCheat, RuntimeFile,
    RuntimeFileKind, RuntimeProcess, RuntimeRefusal, SourceBinding, execute_cheat_runtime,
    workspace_name,
};
use super::execution::{
    LaunchPreflightError, LaunchSpawnError, LaunchedRetroArchProcess, RetroArchLaunchRequest,
    preflight_retroarch_launch, spawn_retroarch,
};
use super::resource_grants::{LaunchProjectionMethod, LaunchResourceRole};
use super::retroarch_command::RetroArchCommand;
use super::retroarch_launch_visibility::{
    FlatpakHost, RetroArchLaunchResources, RetroArchRootError, ensure_retroarch_resources_visible,
    select_retroarch_approved_root,
};
use super::retroarch_resource_projection::approved_retroarch_data_launch_root;
use crate::emulator_environment::retroarch::{
    DiscoveryEnvironment, DiscoveryError, PathPurpose, discover_retroarch_environment,
};
use crate::emulator_environment::{EncodedPath, ReadOnlyHostFilesystem};

/// The adapter id the planner and the runtime use for RetroArch.
pub const RETROARCH_ADAPTER_ID: &str = "retroarch";

/// Largest real `retroarch.cfg` that will be seeded.
const MAX_SEED_CONFIG_BYTES: u64 = 256 * 1024;

impl RuntimeProcess for LaunchedRetroArchProcess {
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

// ---------------------------------------------------------------------
// Seeding the scratch config from the real one (read-only)
// ---------------------------------------------------------------------

/// What happened when the real settings were read for seeding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeedOutcome {
    Seeded {
        lines: usize,
    },
    /// No real config exists yet; RetroArch's own defaults apply.
    NoRealConfig,
    /// A real config exists but was not used; settings are not inherited.
    Unusable {
        why: String,
    },
}

/// Reads the real config for seeding. The file is only read: a symlink, a
/// non-regular file, an oversized file or invalid UTF-8 is refused rather than
/// followed or truncated.
#[must_use]
pub fn read_seed_config(real_config: &Path) -> (Option<String>, SeedOutcome) {
    let metadata = match fs::symlink_metadata(real_config) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (None, SeedOutcome::NoRealConfig);
        }
        Err(e) => {
            return (
                None,
                SeedOutcome::Unusable {
                    why: format!("cannot inspect real config: {e}"),
                },
            );
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return (
            None,
            SeedOutcome::Unusable {
                why: "real config is not a regular file".into(),
            },
        );
    }
    if metadata.len() > MAX_SEED_CONFIG_BYTES {
        return (
            None,
            SeedOutcome::Unusable {
                why: "real config is larger than the seeding limit".into(),
            },
        );
    }
    let mut bytes = Vec::new();
    let read = fs::File::open(real_config)
        .and_then(|f| f.take(MAX_SEED_CONFIG_BYTES + 1).read_to_end(&mut bytes));
    match (read, String::from_utf8(bytes)) {
        (Ok(_), Ok(text)) => (Some(text), SeedOutcome::Seeded { lines: 0 }),
        _ => (
            None,
            SeedOutcome::Unusable {
                why: "real config could not be read as text".into(),
            },
        ),
    }
}

fn config_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return None;
    }
    trimmed.split_once('=').map(|(key, _)| key.trim())
}

/// Builds the final scratch config: the user's real settings first (without
/// the keys this launch owns, and without `#include`/`#reference` lines that
/// could pull in a file that redefines them), then the planner's generated
/// settings last. Deterministic.
#[must_use]
pub fn compose_base_config(seed: Option<&str>, generated: &str) -> (String, usize) {
    let owned: BTreeSet<&str> = generated
        .lines()
        .filter_map(config_key)
        .chain(RETROARCH_MANDATORY_KEYS.iter().copied())
        .collect();
    let mut out = String::new();
    let mut kept = 0usize;
    if let Some(seed) = seed {
        for line in seed.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("#include")
                || trimmed.starts_with("#reference")
                || trimmed.starts_with('#')
            {
                continue;
            }
            match config_key(line) {
                Some(key) if !owned.contains(key) => {
                    out.push_str(line.trim_end());
                    out.push('\n');
                    kept += 1;
                }
                _ => {}
            }
        }
    }
    out.push_str("# --- generated by EmuWiz for this launch only ---\n");
    out.push_str(generated);
    (out, kept)
}

// ---------------------------------------------------------------------
// Planner output -> generic runtime plan
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetroArchRuntimeError {
    /// The planner blocked the launch; nothing is created.
    PlanBlocked(Vec<String>),
    PlanIncomplete(String),
    Refused(RuntimeRefusal),
}

fn block_text(reason: &CheatLaunchBlockReason) -> String {
    format!("{reason:?}")
}

/// Converts a `Ready` planner result into the generic runtime plan.
/// `NoCheatsSelected` yields `Ok(None)`: the launch is exactly the canonical
/// one and no runtime material exists.
pub fn retroarch_runtime_plan(
    plan: &CheatLaunchPlan,
    launch_root: &Path,
    seed: Option<&str>,
) -> Result<Option<(CheatRuntimePlan, usize)>, RetroArchRuntimeError> {
    match plan.status {
        CheatLaunchPlanStatus::NoCheatsSelected => return Ok(None),
        CheatLaunchPlanStatus::Blocked => {
            let mut why: Vec<String> = plan.plan_blocks.iter().map(block_text).collect();
            why.extend(
                plan.blocked
                    .iter()
                    .map(|b| format!("{}: {}", b.logical_id, block_text(&b.reason))),
            );
            return Err(RetroArchRuntimeError::PlanBlocked(why));
        }
        CheatLaunchPlanStatus::Ready => {}
    }
    let incomplete = |text: &str| RetroArchRuntimeError::PlanIncomplete(text.into());
    let settings = plan
        .retroarch
        .as_ref()
        .ok_or_else(|| incomplete("ready plan without RetroArch settings"))?;
    let derivative = plan
        .derivative
        .as_ref()
        .ok_or_else(|| incomplete("ready plan without a derivative"))?;

    // Every generated directory the grants name, plus the parents of every
    // generated path, plus the profile subdirectories the config points at.
    let mut directories: BTreeSet<PathBuf> = BTreeSet::new();
    let add_with_parents = |path: &Path, directories: &mut BTreeSet<PathBuf>| {
        for ancestor in path.ancestors() {
            if ancestor == launch_root || !ancestor.starts_with(launch_root) {
                break;
            }
            directories.insert(ancestor.to_path_buf());
        }
    };
    for grant in &plan.grants.grants {
        let Some(path) = grant.presented_path.as_deref() else {
            continue;
        };
        match (grant.projection, grant.role) {
            (
                LaunchProjectionMethod::GeneratedDirectory,
                LaunchResourceRole::TemporaryRuntime
                | LaunchResourceRole::CheatMaterial
                | LaunchResourceRole::Cache,
            ) => add_with_parents(path, &mut directories),
            (LaunchProjectionMethod::GeneratedFile, _) => {
                if let Some(parent) = path.parent() {
                    add_with_parents(parent, &mut directories);
                }
            }
            _ => {}
        }
    }
    let profile_dir = settings
        .base_config_path
        .parent()
        .ok_or_else(|| incomplete("base config has no parent"))?
        .to_path_buf();
    for name in ["config", "playlists", "cache", "logs", "system"] {
        add_with_parents(&profile_dir.join(name), &mut directories);
    }
    if let Some(parent) = derivative.destination.parent() {
        add_with_parents(parent, &mut directories);
    }
    // A scratch system directory is only created when the plan owns it.
    // When the user's own system directory was passed through, it is theirs.
    let user_system = settings
        .base_config_contents
        .lines()
        .find_map(|l| l.strip_prefix("system_directory = \""))
        .and_then(|v| v.strip_suffix('"'))
        .map(PathBuf::from);
    if let Some(system) = user_system
        && !system.starts_with(launch_root)
    {
        directories.remove(&profile_dir.join("system"));
    }

    let (config_text, seeded_lines) = compose_base_config(seed, &settings.base_config_contents);
    let files = vec![
        RuntimeFile::new(
            settings.base_config_path.clone(),
            config_text.into_bytes(),
            RuntimeFileKind::Config,
        ),
        RuntimeFile::new(
            derivative.destination.clone(),
            derivative.bytes.clone(),
            RuntimeFileKind::CheatMaterial,
        ),
    ];
    if files[1].sha256 != derivative.sha256 {
        return Err(incomplete(
            "derivative bytes do not match their recorded hash",
        ));
    }

    // Source cheat files are evidence: each must be fingerprinted so a change
    // between planning and launch is detected.
    let mut sources: Vec<SourceBinding> = Vec::new();
    for planned in &plan.selected {
        if let Some(path) = &planned.source.source_path {
            let Some(hash) = &planned.source.source_sha256 else {
                return Err(incomplete(
                    "a selected cheat's source file has no recorded fingerprint",
                ));
            };
            let binding = SourceBinding {
                path: path.clone(),
                expected_sha256: hash.clone(),
            };
            if !sources.contains(&binding) {
                sources.push(binding);
            }
        }
    }
    sources.sort_by(|a, b| a.path.cmp(&b.path));

    let cheats = plan
        .selected
        .iter()
        .map(|c| ReceiptCheat {
            logical_id: c.logical_id.clone(),
            variant_id: c.variant_id.clone(),
            title: c.title.clone(),
            provider: c.source.provider.clone(),
            source_id: c.source.source_id.clone(),
        })
        .collect();

    let runtime = CheatRuntimePlan::new(CheatRuntimePlanParts {
        launch_id: plan.launch_id.clone(),
        adapter_id: plan.adapter_id.clone(),
        game_identity: plan.game_identity.clone(),
        root: launch_root.to_path_buf(),
        directories: directories.into_iter().collect(),
        files,
        extra_arguments: settings
            .extra_arguments
            .iter()
            .map(OsString::from)
            .collect(),
        sources,
        expectations: plan.expectations.clone(),
        cheats,
    })
    .map_err(RetroArchRuntimeError::Refused)?;
    Ok(Some((runtime, seeded_lines)))
}

// ---------------------------------------------------------------------
// Facts from the discovered environment
// ---------------------------------------------------------------------

/// The user's selection for one launch, plus the exact-core evidence the
/// planner needs. The core's `library_name` cannot be read from `.info`, so
/// the caller supplies it from the evidence it already holds.
#[derive(Clone, Debug)]
pub struct RetroArchCheatSelection {
    pub candidates: Vec<CheatCandidate>,
    pub selections: Vec<CheatLaunchSelection>,
    pub core_library_name: String,
    pub effective_overrides: Vec<RetroArchOverrideFinding>,
}

fn real_path(value: &Option<EncodedPath>) -> Option<PathBuf> {
    value
        .as_ref()
        .filter(|p| !p.lossy)
        .map(|p| PathBuf::from(&p.display))
}

/// Builds the planner's facts from the freshly discovered environment for the
/// exact profile the canonical preflight selected.
pub fn retroarch_facts(
    command: &RetroArchCommand,
    filesystem: &dyn ReadOnlyHostFilesystem,
    environment: &DiscoveryEnvironment,
    selection: &RetroArchCheatSelection,
    launch_root: PathBuf,
) -> Result<RetroArchLaunchFacts, String> {
    let report = discover_retroarch_environment(filesystem, environment)
        .map_err(|e: DiscoveryError| format!("environment discovery failed: {e:?}"))?;
    let profile = report
        .profiles
        .iter()
        .find(|p| {
            p.profile_kind == command.selection.profile.profile_kind
                && p.scope == command.selection.profile.scope
        })
        .ok_or("the selected RetroArch profile is no longer discoverable")?;
    let config = real_path(&Some(profile.config_file.path.clone()))
        .ok_or("the real RetroArch config path is not valid UTF-8")?;
    let find = |purpose: PathPurpose| {
        profile
            .paths
            .iter()
            .find(|p| p.purpose == purpose)
            .and_then(|p| real_path(&p.resolved_path))
    };
    let saves = find(PathPurpose::Saves)
        .ok_or("RetroArch's save directory is not resolved; saves cannot be protected")?;
    let states = find(PathPurpose::SaveStates)
        .ok_or("RetroArch's save-state directory is not resolved; states cannot be protected")?;
    Ok(RetroArchLaunchFacts {
        launch_root: Some(launch_root),
        real_config_path: Some(config),
        real_save_directory: Some(saves),
        real_state_directory: Some(states),
        content_path: command.selection.content_path.clone(),
        core_library_name: selection.core_library_name.clone(),
        system_directory: find(PathPurpose::System),
        // The generated config redirects every auxiliary directory (see the
        // planner), which is what the planner calls a disposable profile.
        profile_isolation: RetroArchProfileIsolation::DisposableProfile,
        effective_overrides: selection.effective_overrides.clone(),
    })
}

// ---------------------------------------------------------------------
// The launch entry point
// ---------------------------------------------------------------------

#[derive(Debug)]
pub enum RetroArchCheatLaunchError {
    /// The canonical preflight refused the launch itself.
    Preflight(LaunchPreflightError),
    /// The launch is fine but the cheats cannot be applied; nothing was created
    /// and nothing was started.
    Cheats(RetroArchRuntimeError),
    /// The environment facts the planner needs could not be established.
    Facts(String),
    /// A launch without cheats could not be started.
    Spawn(LaunchSpawnError),
    InvalidLaunchId,
    /// The sandbox RetroArch runs in cannot be shown to reach a resource the
    /// cheat launch needs (workspace, content, core, saves or states). Nothing
    /// was created and nothing was started.
    Sandbox(RetroArchRootError),
}

/// How a launch went. Either way the canonical launcher started the process.
pub enum RetroArchCheatLaunch {
    /// No cheat was selected: exactly the canonical launch, no runtime material.
    Plain(LaunchedRetroArchProcess),
    /// A cheat runtime exists for this launch. Its receipt says how far it got;
    /// materialisation can have been refused or failed, in which case no process
    /// was started.
    WithCheats {
        session: Box<CheatRuntimeSession<LaunchedRetroArchProcess>>,
        /// Number of the user's real settings copied into the scratch config.
        seeded_settings: usize,
    },
}

/// THE product entry point: canonical RetroArch launch, optionally with
/// explicitly selected cheats.
///
/// With no selection this is exactly [`super::execution::preflight_and_launch_retroarch`].
/// With a selection it runs the canonical preflight, picks a workspace root the
/// real spawned RetroArch can see (a Flatpak RetroArch has a private `/tmp`, so
/// it gets the EmuWiz data-directory root), proves that RetroArch can reach
/// every resource it will open, plans, materialises an EmuWiz-owned workspace
/// and starts RetroArch through the canonical spawn with the prepared
/// `--config`. The user's real configuration, saves and source media are only
/// fingerprinted, never written, and nothing is ever relocated.
pub fn preflight_and_launch_retroarch_with_cheats(
    request: &RetroArchLaunchRequest,
    filesystem: &dyn ReadOnlyHostFilesystem,
    environment: &DiscoveryEnvironment,
    cheats: &RetroArchCheatSelection,
    launch_id: &str,
) -> Result<RetroArchCheatLaunch, RetroArchCheatLaunchError> {
    let command = preflight_retroarch_launch(request, filesystem, environment)
        .map_err(RetroArchCheatLaunchError::Preflight)?;
    if cheats.selections.is_empty() {
        let process = spawn_retroarch(command).map_err(RetroArchCheatLaunchError::Spawn)?;
        return Ok(RetroArchCheatLaunch::Plain(process));
    }
    let approved_root = select_retroarch_approved_root(
        &command.executable,
        approved_retroarch_data_launch_root().as_deref(),
    )
    .map_err(RetroArchCheatLaunchError::Sandbox)?;
    launch_prepared_command(
        request,
        command,
        filesystem,
        environment,
        cheats,
        &CheatRuntimeRoots { approved_root },
        launch_id,
        FlatpakHost::from_env().as_ref(),
    )
}

/// The composition after preflight and root selection, separate so it can be
/// tested with a command built around a fake emulator executable and a known
/// Flatpak layout.
#[allow(clippy::too_many_arguments)]
pub fn launch_prepared_command(
    request: &RetroArchLaunchRequest,
    command: RetroArchCommand,
    filesystem: &dyn ReadOnlyHostFilesystem,
    environment: &DiscoveryEnvironment,
    cheats: &RetroArchCheatSelection,
    roots: &CheatRuntimeRoots,
    launch_id: &str,
    host: Option<&FlatpakHost>,
) -> Result<RetroArchCheatLaunch, RetroArchCheatLaunchError> {
    if cheats.selections.is_empty() {
        let process = spawn_retroarch(command).map_err(RetroArchCheatLaunchError::Spawn)?;
        return Ok(RetroArchCheatLaunch::Plain(process));
    }
    let launch_root = roots.approved_root.join(workspace_name(launch_id));
    let facts = retroarch_facts(
        &command,
        filesystem,
        environment,
        cheats,
        launch_root.clone(),
    )
    .map_err(RetroArchCheatLaunchError::Facts)?;
    // The process EmuWiz starts - not EmuWiz - opens these. Prove it can.
    let core = command
        .arguments
        .iter()
        .position(|argument| argument == "-L")
        .and_then(|at| command.arguments.get(at + 1))
        .map(PathBuf::from);
    let (Some(saves), Some(states)) = (
        facts.real_save_directory.as_deref(),
        facts.real_state_directory.as_deref(),
    ) else {
        return Err(RetroArchCheatLaunchError::Facts(
            "RetroArch's save or save-state directory is not resolved".into(),
        ));
    };
    ensure_retroarch_resources_visible(
        &command.executable,
        &RetroArchLaunchResources {
            workspace: &launch_root,
            content: &command.selection.content_path,
            core: core.as_deref(),
            saves,
            states,
        },
        host,
    )
    .map_err(RetroArchCheatLaunchError::Sandbox)?;
    let seed_path = facts.real_config_path.clone();
    let planner_request = CheatLaunchRequest {
        launch_id: launch_id.to_string(),
        target: CheatLaunchTarget {
            adapter_id: RETROARCH_ADAPTER_ID.into(),
            game_identity: request.expected_game_key.clone(),
            // The canonical preflight only returns a command for a freshly
            // resolved identity matching the request.
            identity_verified: true,
        },
        candidates: cheats.candidates.clone(),
        selections: cheats.selections.clone(),
        retroarch: Some(facts),
    };
    let plan = plan_cheat_launch(&planner_request);

    let (seed_text, seed_outcome) = match &seed_path {
        Some(path) => read_seed_config(path),
        None => (None, SeedOutcome::NoRealConfig),
    };
    let runtime = retroarch_runtime_plan(&plan, &launch_root, seed_text.as_deref())
        .map_err(RetroArchCheatLaunchError::Cheats)?;
    let Some((runtime, seeded)) = runtime else {
        // The planner found nothing to apply: the launch is the canonical one.
        let process = spawn_retroarch(command).map_err(RetroArchCheatLaunchError::Spawn)?;
        return Ok(RetroArchCheatLaunch::Plain(process));
    };

    let binding = LiveLaunchBinding {
        game_identity: request.expected_game_key.clone(),
        adapter_id: RETROARCH_ADAPTER_ID.into(),
    };
    let mut session = execute_cheat_runtime(
        &runtime,
        &binding,
        roots,
        command,
        |mut command: RetroArchCommand, extra: &[OsString]| {
            command.arguments.extend(extra.iter().cloned());
            command
        },
        |command| spawn_retroarch(command).map_err(|e| format!("{e:?}")),
    );
    if let SeedOutcome::Unusable { why } = seed_outcome {
        session.note(format!(
            "your RetroArch settings were not inherited for this launch ({why})"
        ));
    }
    Ok(RetroArchCheatLaunch::WithCheats {
        session: Box::new(session),
        seeded_settings: seeded,
    })
}

/// Convenience for callers that only want the receipt.
#[must_use]
pub fn receipt_of(launch: &RetroArchCheatLaunch) -> Option<&CheatRuntimeReceipt> {
    match launch {
        RetroArchCheatLaunch::Plain(_) => None,
        RetroArchCheatLaunch::WithCheats { session, .. } => Some(session.receipt()),
    }
}

#[cfg(test)]
mod tests;
