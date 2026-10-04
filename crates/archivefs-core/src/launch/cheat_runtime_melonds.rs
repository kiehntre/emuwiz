//! Standalone melonDS adapter for the canonical per-launch cheat runtime.
//!
//! melonDS (1.1) has no command-line cheat or config option. It reads cheats
//! from `<CheatFilePath or the ROM's folder>/<rom name>.mch` and only when the
//! `EnableCheats` config key is true, both read from `melonDS.toml` in
//! `$XDG_CONFIG_HOME/melonDS`. This adapter therefore stages, inside the
//! canonical runtime workspace only:
//!
//! * `cheats/<rom stem>.mch` with just the chosen cheats, and
//! * `config/melonDS/melonDS.toml`, a copy of the user's config with
//!   `[Instance0] EnableCheats = true` and `CheatFilePath = <workspace>/cheats`,
//!
//! and starts melonDS with `XDG_CONFIG_HOME=<workspace>/config` on the child
//! only. The user's real config, cheat files, saves (which melonDS keeps next
//! to the ROM) and ROM are never written. Without a cheat selection the
//! launch is the unchanged plain launch.
//!
//! melonDS logs nothing when it loads cheats and has no UI-free way to report
//! which are active, so a launch only ever reaches "material staged and the
//! emulator pointed at it".

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::cheat_launch_plan::{
    FingerprintKind, LaunchStateClass, LaunchStateExpectation, StateExpectation, ViolationSeverity,
};
use super::cheat_runtime::{
    CheatRuntimePlan, CheatRuntimePlanParts, CheatRuntimeRoots, CheatRuntimeSession,
    HandoffOutcome, LiveLaunchBinding, ReceiptCheat, RuntimeFile, RuntimeFileKind,
    execute_cheat_runtime, workspace_name,
};
use super::melonds_execution::{
    MelonDsLaunchPreflightError, MelonDsLaunchRequest, preflight_melonds_launch, spawn_melonds,
};
use super::planning::CanonicalIdentityStatus;
use super::process_spawn::{
    PreparedProcessCommand, WatchedProcess, spawn_watched_process_with_environment,
};
use crate::patch_manager::{
    MelonDsCheatEntry, MelonDsCheatFile, MelonDsCheatItem, MelonDsCheatState,
    MelonDsProfileDiscoveryRoots, MelonDsRomIdentity, render_melonds_cheat_file,
};

pub const MELONDS_ADAPTER_ID: &str = "melonds";
const MAX_SEED_CONFIG_BYTES: u64 = 256 * 1024;

/// What a melonDS cheat launch can honestly say. melonDS gives no evidence of
/// loading or enabling, so there is no stronger state than this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MelonDsCheatActivation {
    MaterialStagedEmulatorPointedAtItEffectNotProven,
}

impl MelonDsCheatActivation {
    #[must_use]
    pub fn label(self) -> &'static str {
        "melonDS was started with cheats switched on and this launch's cheat file. \
         melonDS does not report which cheats it loaded, and EmuWiz has not seen \
         one work in a game."
    }
}

/// The chosen cheats for one ROM. `identity` must be verified; `file` is the
/// parsed native `.mch` they come from.
#[derive(Debug, Clone)]
pub struct MelonDsCheatSelection {
    pub identity: MelonDsRomIdentity,
    pub file: MelonDsCheatFile,
    pub selected: Vec<String>,
    /// The user's real `melonDS.toml`, read (never written) to seed the
    /// launch profile so input and firmware settings carry over.
    pub real_config: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MelonDsCheatError {
    /// Identity is not verified or does not match the ROM actually launched.
    IdentityNotVerified(String),
    IdentityMismatch(String),
    /// Cheat file has parse issues or an unusable selection.
    Material(String),
    NoCheatSelected,
    UnknownCheat(String),
    InvalidLaunchId,
    Runtime(String),
}

#[derive(Debug)]
pub enum MelonDsCheatLaunchError {
    Preflight(MelonDsLaunchPreflightError),
    Cheats(MelonDsCheatError),
    Spawn(std::io::Error),
}

pub enum MelonDsCheatLaunch {
    Plain(WatchedProcess),
    WithCheats {
        session: Box<CheatRuntimeSession<WatchedProcess>>,
        activation: MelonDsCheatActivation,
        /// Non-failure things to tell the user (for example config not inherited).
        notes: Vec<String>,
    },
}

/// The command plus the child-only environment the runtime must apply.
struct EnvCommand {
    command: PreparedProcessCommand,
    environment: Vec<(OsString, OsString)>,
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Checks the identity against the ROM that will really be launched.
fn verify_identity(identity: &MelonDsRomIdentity, rom: &Path) -> Result<(), MelonDsCheatError> {
    if !identity.verified {
        return Err(MelonDsCheatError::IdentityNotVerified(
            "the game identity is not verified".into(),
        ));
    }
    if identity.rom_path != rom {
        return Err(MelonDsCheatError::IdentityMismatch(
            "the identity is for a different ROM path".into(),
        ));
    }
    if identity.rom_sha256.is_none() && identity.game_code.is_none() {
        return Err(MelonDsCheatError::IdentityNotVerified(
            "neither a ROM hash nor a game code is known".into(),
        ));
    }
    let mismatch = |what: &str| MelonDsCheatError::IdentityMismatch(what.into());
    if let Some(code) = &identity.game_code {
        let mut header = [0u8; 0x10];
        std::fs::File::open(rom)
            .and_then(|mut f| f.read_exact(&mut header))
            .map_err(|e| mismatch(&format!("cannot read the ROM header: {e}")))?;
        if code.as_bytes() != &header[0x0C..0x10] {
            return Err(mismatch("the ROM's game code is not the cheat's game"));
        }
    }
    if let Some(expected) = &identity.rom_sha256 {
        let actual =
            sha256_file(rom).map_err(|e| mismatch(&format!("cannot hash the ROM: {e}")))?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(mismatch("the ROM's hash is not the cheat's game"));
        }
    }
    Ok(())
}

/// Keeps only the chosen entries, all switched on. Refuses a malformed file,
/// unknown names, and two chosen codes in an only-one-enabled category.
fn select_entries(
    file: &MelonDsCheatFile,
    selected: &[String],
) -> Result<MelonDsCheatFile, MelonDsCheatError> {
    if selected.is_empty() {
        return Err(MelonDsCheatError::NoCheatSelected);
    }
    if let Some(issue) = file.issues.first() {
        return Err(MelonDsCheatError::Material(format!(
            "the cheat file has problems: {issue:?}"
        )));
    }
    let mut out = file.clone();
    let mut keep = |entry: &mut MelonDsCheatEntry| -> bool {
        let chosen = selected.contains(&entry.name);
        if chosen {
            entry.state = MelonDsCheatState::Enabled;
        }
        chosen && !entry.code.words.is_empty()
    };
    let mut result: Result<(), MelonDsCheatError> = Ok(());
    out.items.retain_mut(|item| match item {
        MelonDsCheatItem::RootCode(entry) => keep(entry),
        MelonDsCheatItem::Category(category) => {
            category.entries.retain_mut(&mut keep);
            if category.only_one_code_enabled && category.entries.len() > 1 {
                result = Err(MelonDsCheatError::Material(format!(
                    "category \"{}\" allows only one enabled cheat",
                    category.name
                )));
            }
            !category.entries.is_empty()
        }
    });
    result?;
    for name in selected {
        let present = out.items.iter().any(|item| match item {
            MelonDsCheatItem::RootCode(e) => &e.name == name,
            MelonDsCheatItem::Category(c) => c.entries.iter().any(|e| &e.name == name),
        });
        if !present {
            return Err(MelonDsCheatError::UnknownCheat(name.clone()));
        }
    }
    Ok(out)
}

/// The user's config with the two cheat keys forced; or a minimal config.
/// Returns the text and a note when the real settings could not be inherited.
fn launch_config(real: Option<&Path>, cheat_dir: &Path) -> (String, Option<String>) {
    let (mut table, note) = match real {
        None => (toml::Table::new(), None),
        Some(path) => match read_toml(path) {
            Ok(table) => (table, None),
            Err(why) => (
                toml::Table::new(),
                Some(format!(
                    "your melonDS settings were not inherited for this launch ({why})"
                )),
            ),
        },
    };
    let instance = table
        .entry("Instance0")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !instance.is_table() {
        *instance = toml::Value::Table(toml::Table::new());
    }
    if let Some(instance) = instance.as_table_mut() {
        instance.insert("EnableCheats".into(), toml::Value::Boolean(true));
        instance.insert(
            "CheatFilePath".into(),
            toml::Value::String(cheat_dir.to_string_lossy().into_owned()),
        );
    }
    (table.to_string(), note)
}

fn read_toml(path: &Path) -> Result<toml::Table, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_SEED_CONFIG_BYTES {
        return Err("not a regular file within the size limit".into());
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_string(&mut text))
        .map_err(|e| e.to_string())?;
    text.parse::<toml::Table>().map_err(|e| e.to_string())
}

/// Builds the runtime plan. Reads the ROM and real config; writes nothing.
/// Returns the plan, the child environment, and any notes.
pub fn melonds_runtime_plan(
    rom: &Path,
    cheat: &MelonDsCheatSelection,
    launch_id: &str,
    approved_root: &Path,
) -> Result<(CheatRuntimePlan, Vec<(OsString, OsString)>, Vec<String>), MelonDsCheatError> {
    verify_identity(&cheat.identity, rom)?;
    let stem = rom
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.rsplit_once('.').map(|(stem, _)| stem))
        .filter(|s| !s.is_empty() && !s.contains(['/', '\\', '\0']))
        .ok_or_else(|| MelonDsCheatError::IdentityMismatch("the ROM has no usable name".into()))?;
    let chosen = select_entries(&cheat.file, &cheat.selected)?;

    let root = approved_root.join(workspace_name(launch_id));
    let cheat_dir = root.join("cheats");
    let config_home = root.join("config");
    let (config_text, note) = launch_config(cheat.real_config.as_deref(), &cheat_dir);
    let mch = RuntimeFile::new(
        cheat_dir.join(format!("{stem}.mch")),
        render_melonds_cheat_file(&chosen),
        RuntimeFileKind::CheatMaterial,
    );
    let config = RuntimeFile::new(
        config_home.join("melonDS").join("melonDS.toml"),
        config_text.into_bytes(),
        RuntimeFileKind::Config,
    );
    let expect =
        |path: PathBuf, class, fingerprint, expectation, severity| LaunchStateExpectation {
            path,
            class,
            expectation,
            fingerprint,
            severity,
        };
    let mut expectations = vec![
        expect(
            rom.to_path_buf(),
            LaunchStateClass::ReadOnlySource,
            FingerprintKind::FileIdentity,
            StateExpectation::MustRemainUnchanged,
            ViolationSeverity::Corruption,
        ),
        expect(
            root.clone(),
            LaunchStateClass::EphemeralRuntime,
            FingerprintKind::NotPresent,
            StateExpectation::MustNotExistAfter,
            ViolationSeverity::Warning,
        ),
    ];
    if let Some(real) = cheat.real_config.as_ref().filter(|p| p.exists()) {
        expectations.push(expect(
            real.clone(),
            LaunchStateClass::ProtectedConfig,
            FingerprintKind::Sha256,
            StateExpectation::MustRemainUnchanged,
            ViolationSeverity::LaunchAffecting,
        ));
    }
    let mut cheats = Vec::new();
    for item in &chosen.items {
        let entries: Vec<&MelonDsCheatEntry> = match item {
            MelonDsCheatItem::RootCode(e) => vec![e],
            MelonDsCheatItem::Category(c) => c.entries.iter().collect(),
        };
        cheats.extend(entries.into_iter().map(|e| ReceiptCheat {
            logical_id: e.name.clone(),
            variant_id: identity_key(&cheat.identity),
            title: e.name.clone(),
            provider: "melonds-mch".into(),
            source_id: format!("{stem}.mch"),
        }));
    }
    let plan = CheatRuntimePlan::new(CheatRuntimePlanParts {
        launch_id: launch_id.to_string(),
        adapter_id: MELONDS_ADAPTER_ID.into(),
        game_identity: identity_key(&cheat.identity),
        root,
        directories: vec![cheat_dir, config_home.clone(), config_home.join("melonDS")],
        files: vec![mch, config],
        extra_arguments: Vec::new(),
        sources: Vec::new(),
        expectations,
        cheats,
    })
    .map_err(|r| MelonDsCheatError::Runtime(format!("{r}")))?;
    let environment = vec![(
        OsString::from("XDG_CONFIG_HOME"),
        config_home.into_os_string(),
    )];
    Ok((plan, environment, note.into_iter().collect()))
}

/// The canonical melonDS preflight, then the canonical runtime around the same
/// spawn. With `cheat: None` this is exactly `preflight_melonds_launch` +
/// `spawn_melonds`: no environment, no profile, no config writes.
pub fn preflight_and_launch_melonds_with_cheats(
    request: &MelonDsLaunchRequest,
    roots: &MelonDsProfileDiscoveryRoots,
    identity: &CanonicalIdentityStatus,
    game_key: Option<&str>,
    cheat: Option<&MelonDsCheatSelection>,
    launch_id: &str,
    runtime_roots: &CheatRuntimeRoots,
) -> Result<MelonDsCheatLaunch, MelonDsCheatLaunchError> {
    let command = preflight_melonds_launch(request, roots, identity, game_key)
        .map_err(MelonDsCheatLaunchError::Preflight)?;
    let Some(cheat) = cheat else {
        return spawn_melonds(&command)
            .map(MelonDsCheatLaunch::Plain)
            .map_err(MelonDsCheatLaunchError::Spawn);
    };
    let (plan, environment, notes) = melonds_runtime_plan(
        &request.selected_content_path,
        cheat,
        launch_id,
        &runtime_roots.approved_root,
    )
    .map_err(MelonDsCheatLaunchError::Cheats)?;
    let binding = LiveLaunchBinding {
        game_identity: identity_key(&cheat.identity),
        adapter_id: MELONDS_ADAPTER_ID.into(),
    };
    let session = execute_cheat_runtime(
        &plan,
        &binding,
        runtime_roots,
        EnvCommand {
            command,
            environment,
        },
        |command, _extra: &[OsString]| command,
        |c: EnvCommand| {
            spawn_watched_process_with_environment(&c.command, &c.environment)
                .map_err(|e| e.to_string())
        },
    );
    if !matches!(session.receipt().handoff, HandoffOutcome::PassedToEmulator) {
        return Err(MelonDsCheatLaunchError::Cheats(MelonDsCheatError::Runtime(
            session.receipt().errors.join("; "),
        )));
    }
    Ok(MelonDsCheatLaunch::WithCheats {
        session: Box::new(session),
        activation: MelonDsCheatActivation::MaterialStagedEmulatorPointedAtItEffectNotProven,
        notes,
    })
}

fn identity_key(identity: &MelonDsRomIdentity) -> String {
    identity
        .rom_sha256
        .clone()
        .or_else(|| identity.game_code.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
