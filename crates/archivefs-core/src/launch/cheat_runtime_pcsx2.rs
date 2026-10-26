//! Explicit PCSX2 PNACH activation through the canonical per-launch runtime.
//!
//! The reviewed readiness projection stays read-only and cannot authorize a
//! launch. This adapter adds the executor contract: a tested deployment,
//! normalized/pinned folders, immutable approval, fresh ordinary preflight,
//! a disposable configuration probe, and sandbox-aware lifetime tracking.
//! Production INIs and cheat directories are never write destinations.

mod config;
mod flatpak;
#[cfg(test)]
mod tests;

use super::cheat_launch_plan::{
    FingerprintKind, LaunchStateClass, LaunchStateExpectation, StateExpectation, ViolationSeverity,
};
use super::cheat_runtime::{
    CheatRuntimePlan, CheatRuntimePlanParts, CheatRuntimeRoots, CheatRuntimeSession, ExitSummary,
    HandoffOutcome, LiveLaunchBinding, ReceiptCheat, RuntimeFile, RuntimeFileKind, RuntimeProcess,
    SourceBinding, execute_cheat_runtime, workspace_name,
};
use super::installation::{LaunchInstallation, VisibilityPlan};
use super::pcsx2_execution::{Pcsx2LaunchRequest, preflight_pcsx2_launch};
use super::process_spawn::{
    CapturedFileIdentity, PreparedProcessCommand, open_regular_file_nonblocking,
};
use crate::dat::firmware_evidence::FirmwareIdentityRecord;
use crate::game_identity::inspect_catalogued_game_identity;
use crate::patch_manager::{
    PCSX2_MAX_CONFIG_BYTES, Pcsx2FolderRouting, Pcsx2GameIdentity, Pcsx2InstallationType,
    Pcsx2NativeLaunchBinding, Pcsx2Profile, Pcsx2ProfileDiscoveryRoots, Pcsx2TempCheatInput,
    Pcsx2TempCheatVerdict, discover_local_pcsx2_pnach_file, discover_pcsx2_profiles,
    merge_managed_pnach_cheats, parse_pnach_document, plan_pcsx2_temporary_cheat_launch,
    resolve_pcsx2_native_launch_binding, validate_destination_root,
};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const SELECTED_GROUP: &str = "EmuWiz Selected";
pub const APP_ID: &str = "net.pcsx2.PCSX2";
// The exact deployment exercised by the retained disposable runtime probe.
// An update is a new capability, never silently accepted from a version string.
pub const TESTED_DEPLOYMENT: &str =
    "f8162e3d6b0ce75f54d0e10605d368b3678fda969bb1e806277500156c85fa10";
const ADAPTER: &str = "pcsx2";
pub(super) const INSTANCE_FILE: &str = "pcsx2-flatpak.instance";

pub(super) fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pcsx2CheatObservation {
    pub game_launch_handed_off: bool,
    pub file_loaded: bool,
    pub selection_enabled: bool,
    pub detail: Option<String>,
}

impl Pcsx2CheatObservation {
    pub fn label(&self) -> &'static str {
        if self.detail.is_some() && !self.game_launch_handed_off {
            "Game launch refused. No game activation was attempted; temporary-profile cleanup is shown below."
        } else if !self.game_launch_handed_off {
            "Checking the scoped PCSX2 configuration. The game has not started; cheat activation is NOT CONFIRMED."
        } else if self.file_loaded && self.selection_enabled {
            "PCSX2 reported loading the staged PNACH and enabling the selected patch group. In-game effect NOT CONFIRMED."
        } else if self.file_loaded {
            "PCSX2 reported loading the staged PNACH; enabling the selected group is NOT CONFIRMED. In-game effect NOT CONFIRMED."
        } else {
            "Temporary material was handed to PCSX2. PNACH loading and activation are NOT CONFIRMED."
        }
    }
}

/// Immutable, single-game approval evidence. Only the read-only preview
/// producer can construct this; no persisted caller-supplied proof flags.
#[derive(Clone, Debug)]
pub struct Pcsx2CheatPreview {
    request: Pcsx2LaunchRequest,
    source: PathBuf,
    source_hash: String,
    launch_id: String,
    digest: String,
    content: CapturedFileIdentity,
    executable: CapturedFileIdentity,
    identity: String,
    deployment: String,
    directories: Vec<(PathBuf, u64, u64)>,
    profile_path: PathBuf,
    patch_count: usize,
    preserved_folders: Vec<(String, PathBuf)>,
}

impl Pcsx2CheatPreview {
    pub fn profile_path(&self) -> &Path {
        &self.profile_path
    }
    pub fn patch_count(&self) -> usize {
        self.patch_count
    }
    pub fn preserved_folders(&self) -> &[(String, PathBuf)] {
        &self.preserved_folders
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn deployment(&self) -> &str {
        &self.deployment
    }
    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }
    pub fn matches(&self, request: &Pcsx2LaunchRequest) -> bool {
        &self.request == request
    }
}

#[derive(Clone)]
struct Prepared {
    preview: Pcsx2CheatPreview,
    plan: CheatRuntimePlan,
    command: PreparedProcessCommand,
    profile: Pcsx2Profile,
    binding: Pcsx2NativeLaunchBinding,
    visibility: VisibilityPlan,
    expected_folders: Vec<(String, PathBuf)>,
    cheat_path: PathBuf,
    roots: Pcsx2ProfileDiscoveryRoots,
    firmware: Vec<FirmwareIdentityRecord>,
}

fn expected(
    path: PathBuf,
    class: LaunchStateClass,
    fingerprint: FingerprintKind,
    expectation: StateExpectation,
) -> LaunchStateExpectation {
    LaunchStateExpectation {
        path,
        class,
        fingerprint,
        expectation,
        severity: ViolationSeverity::Corruption,
    }
}

fn capture_content(path: &Path) -> Result<CapturedFileIdentity, String> {
    let file = open_regular_file_nonblocking(path).map_err(|e| e.to_string())?;
    Ok(CapturedFileIdentity::capture(
        &file.metadata().map_err(|e| e.to_string())?,
    ))
}

/// Reads identity, selected material, profile and installed deployment; writes
/// nothing. The caller displays this preview before offering a launch approval.
pub fn preview_pcsx2_cheats(
    request: &Pcsx2LaunchRequest,
    roots: &Pcsx2ProfileDiscoveryRoots,
    firmware: &[FirmwareIdentityRecord],
    source: &Path,
    launch_id: &str,
    runtime: &CheatRuntimeRoots,
) -> Result<Pcsx2CheatPreview, String> {
    prepare(request, roots, firmware, source, launch_id, runtime).map(|p| p.preview)
}

fn prepare(
    request: &Pcsx2LaunchRequest,
    roots: &Pcsx2ProfileDiscoveryRoots,
    firmware: &[FirmwareIdentityRecord],
    source: &Path,
    launch_id: &str,
    runtime: &CheatRuntimeRoots,
) -> Result<Prepared, String> {
    let content = capture_content(&request.selected_content_path)?;
    let executable = capture_content(&request.expected_executable)?;
    let ordinary = preflight_pcsx2_launch(request, roots, firmware)
        .map_err(|e| format!("Ordinary PCSX2 preflight refused: {e:?}"))?;
    let discovery =
        discover_pcsx2_profiles(roots).map_err(|e| format!("PCSX2 discovery failed: {e:?}"))?;
    let profile = discovery
        .profiles
        .into_iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or("The selected PCSX2 profile disappeared")?;
    let binding = resolve_pcsx2_native_launch_binding(&profile, roots).map_err(|e| e.detail)?;
    if binding.executable != request.expected_executable
        || binding.installation != request.expected_installation
        || binding.user_directory_mode != request.expected_user_directory_mode
    {
        return Err("PCSX2 installation binding changed; preview again".into());
    }
    let deployment = flatpak::capability(&binding, profile.installation_type)?;
    let report = inspect_catalogued_game_identity(&request.selected_content_path, Some("PS2"));
    let identity = Pcsx2GameIdentity::from_report("Selected PS2 game", &report);
    let crc = identity
        .verified_crc()
        .ok_or("The game has no verified PCSX2 executable CRC")?;
    let serial = identity
        .serial
        .as_deref()
        .filter(|s| *s == request.expected_ps2_serial)
        .ok_or("The game's exact PS2 serial changed")?;
    let identity_key = format!("{serial}:{crc}");
    let material = discover_local_pcsx2_pnach_file(source, &identity).map_err(|e| e.to_string())?;
    if material
        .detected_serial
        .as_deref()
        .is_some_and(|s| s != serial)
    {
        return Err("The PNACH belongs to another PS2 serial".into());
    }
    let workspace = runtime.approved_root.join(workspace_name(launch_id));
    let mut sources = vec![SourceBinding {
        path: source.into(),
        expected_sha256: material.source_sha256.clone(),
    }];
    let config_path = config::global_path(&profile)?;
    let ini = config::source_ini(&config_path, &mut sources)?;
    let mut overlay = config::overlay(ini, &profile, &workspace, &request.selected_content_path)?;
    let readiness = plan_pcsx2_temporary_cheat_launch(&Pcsx2TempCheatInput {
        identity: &identity,
        profile: &profile,
        binding: Ok(&binding),
        cheat_material: Some(Ok(&material)),
        folder_routing: &overlay.routing,
    });
    if readiness.verdict == Pcsx2TempCheatVerdict::Blocked {
        return Err(readiness
            .blockers
            .iter()
            .map(|b| format!("{} {}", b.reason, b.action))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let data = workspace.join("PCSX2");
    let staged_settings = data.join("gamesettings");
    let mut files = Vec::new();
    config::stage_bios(&mut overlay, firmware, &data, &mut sources, &mut files)?;
    files.push(RuntimeFile::new(
        data.join("inis/PCSX2.ini"),
        overlay.ini.text().into_bytes(),
        RuntimeFileKind::Config,
    ));
    for filename in [format!("{serial}_{crc}.ini"), format!("{crc}.ini")] {
        let path = overlay.games.join(&filename);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                let mut game_ini = config::source_ini(&path, &mut sources)?;
                if game_ini.sections_present("Folders") {
                    return Err(
                        "Per-game folder overrides are not supported for temporary activation"
                            .into(),
                    );
                }
                if game_ini.get("Filenames", "BIOS").is_some() {
                    return Err("Per-game BIOS overrides require a separately verified firmware binding; temporary activation is refused".into());
                }
                game_ini.validate_hardcore(Some(&overlay.ini))?;
                game_ini.enable_selection()?;
                files.push(RuntimeFile::new(
                    staged_settings.join(filename),
                    game_ini.text().into_bytes(),
                    RuntimeFileKind::Config,
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    // Always create the exact serial+CRC per-game layer so a global Enable
    // list cannot accidentally yield to a user's per-game cheat switch.
    let exact_game = staged_settings.join(format!("{serial}_{crc}.ini"));
    if !files.iter().any(|f| f.path == exact_game) {
        // Copy the CRC-only fallback rather than masking it with an empty INI.
        let fallback = staged_settings.join(format!("{crc}.ini"));
        let bytes = files
            .iter()
            .find(|f| f.path == fallback)
            .map(|f| f.bytes.clone());
        let mut game_ini = config::Ini::parse(
            std::str::from_utf8(bytes.as_deref().unwrap_or_default())
                .map_err(|_| "Invalid per-game settings")?,
        )?;
        game_ini.enable_selection()?;
        files.push(RuntimeFile::new(
            exact_game,
            game_ini.text().into_bytes(),
            RuntimeFileKind::Config,
        ));
    }
    let mut selected_cheat = material.cheat.clone();
    selected_cheat.description = Some("Temporary explicitly approved PNACH selection".into());
    let rendered = merge_managed_pnach_cheats(
        &parse_pnach_document(b"").map_err(|e| e.to_string())?,
        std::slice::from_ref(&selected_cheat),
    )
    .map_err(|e| e.to_string())?;
    let cheat_path = data.join("cheats").join(format!("{serial}_{crc}.pnach"));
    let mut pnach = format!("[{SELECTED_GROUP}]\n").into_bytes();
    pnach.extend(rendered);
    files.push(RuntimeFile::new(
        cheat_path.clone(),
        pnach,
        RuntimeFileKind::CheatMaterial,
    ));
    files.push(RuntimeFile::new(
        workspace.join(INSTANCE_FILE),
        Vec::new(),
        RuntimeFileKind::Config,
    ));
    let mut expectations = sources
        .iter()
        .map(|s| {
            expected(
                s.path.clone(),
                LaunchStateClass::ProtectedConfig,
                if files.iter().any(|f| {
                    f.kind == RuntimeFileKind::VerifiedResource && f.sha256 == s.expected_sha256
                }) {
                    FingerprintKind::FileIdentity
                } else {
                    FingerprintKind::Sha256
                },
                StateExpectation::MustRemainUnchanged,
            )
        })
        .collect::<Vec<_>>();
    expectations.push(expected(
        request.selected_content_path.clone(),
        LaunchStateClass::ReadOnlySource,
        FingerprintKind::FileIdentity,
        StateExpectation::MustRemainUnchanged,
    ));
    expectations.push(expected(
        workspace.clone(),
        LaunchStateClass::EphemeralRuntime,
        FingerprintKind::NotPresent,
        StateExpectation::MustNotExistAfter,
    ));
    let plan = CheatRuntimePlan::new(CheatRuntimePlanParts {
        launch_id: launch_id.into(),
        adapter_id: ADAPTER.into(),
        game_identity: identity_key.clone(),
        root: workspace.clone(),
        directories: [
            "home",
            "config",
            "data",
            "cache",
            "PCSX2",
            "PCSX2/inis",
            "PCSX2/bios",
            "PCSX2/cheats",
            "PCSX2/gamesettings",
            "PCSX2/logs",
            "PCSX2/cache",
            "PCSX2/debuggerlayouts",
            "PCSX2/debuggersettings",
        ]
        .iter()
        .map(|p| workspace.join(p))
        .collect(),
        files,
        extra_arguments: vec![],
        sources,
        expectations,
        cheats: vec![ReceiptCheat {
            logical_id: material.cheat.id.clone(),
            variant_id: material.source_sha256.clone(),
            title: material.cheat.name.clone(),
            provider: "local-pnach".into(),
            source_id: material.cheat.id.clone(),
        }],
    })
    .map_err(|e| e.to_string())?;
    if capture_content(&request.selected_content_path)? != content {
        return Err("Game content changed during preview".into());
    }
    let directories = directory_bindings(&profile.configuration_path, overlay.folders.values())?;
    let preview = Pcsx2CheatPreview {
        request: request.clone(),
        source: source.into(),
        source_hash: material.source_sha256,
        launch_id: launch_id.into(),
        digest: plan.digest().into(),
        content,
        executable,
        identity: identity_key,
        deployment,
        directories,
        profile_path: profile.configuration_path.clone(),
        patch_count: material.cheat.patch_lines.len(),
        preserved_folders: overlay
            .folders
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    };
    let expected_folders = config::actual_folder_paths(&overlay.ini)?;
    for grant in overlay
        .visibility
        .grants()
        .iter()
        .filter(|g| g.access == super::installation::Access::ReadWrite && g.path != workspace)
    {
        if source.starts_with(&grant.path)
            || request.selected_content_path.starts_with(&grant.path)
            || config_path.starts_with(&grant.path)
            || overlay.visibility.grants().iter().any(|ro| {
                ro.access == super::installation::Access::ReadOnly
                    && ro.path.starts_with(&grant.path)
            })
        {
            return Err("A writable user-state folder overlaps protected game, cheat or configuration resources".into());
        }
    }
    Ok(Prepared {
        preview,
        plan,
        command: ordinary.command_spec(),
        profile,
        binding,
        visibility: overlay.visibility,
        expected_folders,
        cheat_path,
        roots: roots.clone(),
        firmware: firmware.to_vec(),
    })
}

fn directory_bindings<'a>(
    profile: &Path,
    folders: impl Iterator<Item = &'a PathBuf>,
) -> Result<Vec<(PathBuf, u64, u64)>, String> {
    use std::os::unix::fs::MetadataExt;
    let mut paths = folders.cloned().collect::<Vec<_>>();
    paths.push(profile.into());
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {
                Some(Ok((path, m.dev(), m.ino())))
            }
            Ok(_) => Some(Err(
                "A PCSX2 resource folder is not a regular directory".into()
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => Some(Err(e.to_string())),
        })
        .collect()
}

pub struct Pcsx2CheatLaunch {
    pub session: CheatRuntimeSession<flatpak::Pcsx2RuntimeProcess>,
    observation: Arc<Mutex<Pcsx2CheatObservation>>,
}

impl Pcsx2CheatLaunch {
    pub fn observation(&self) -> Pcsx2CheatObservation {
        self.observation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn poll(&mut self) {
        self.session.poll();
    }
}

/// Explicit approval of exactly one immutable preview. There is no fallback
/// to an ordinary launch when approval is stale or staging fails.
pub fn launch_pcsx2_cheats(
    approved: Pcsx2CheatPreview,
    roots: &Pcsx2ProfileDiscoveryRoots,
    firmware: &[FirmwareIdentityRecord],
    runtime: &CheatRuntimeRoots,
) -> Result<Pcsx2CheatLaunch, String> {
    if capture_content(&approved.request.expected_executable)? != approved.executable {
        return Err("The PCSX2 launcher changed since preview; preview again".into());
    }
    let prepared = prepare(
        &approved.request,
        roots,
        firmware,
        &approved.source,
        &approved.launch_id,
        runtime,
    )?;
    if approved.digest != prepared.preview.digest
        || approved.identity != prepared.preview.identity
        || approved.deployment != prepared.preview.deployment
        || approved.content != prepared.preview.content
        || approved.executable != prepared.preview.executable
        || approved.directories != prepared.preview.directories
    {
        return Err("Game, cheat material or profile changed since approval. Preview again; nothing was installed.".into());
    }
    let observation = Arc::new(Mutex::new(Pcsx2CheatObservation::default()));
    let shared = observation.clone();
    let binding = LiveLaunchBinding {
        game_identity: approved.identity,
        adapter_id: ADAPTER.into(),
    };
    let session = execute_cheat_runtime(
        &prepared.plan,
        &binding,
        runtime,
        (),
        |_, _| (),
        |_| flatpak::probe_and_spawn(&prepared, shared),
    );
    if !matches!(session.receipt().handoff, HandoffOutcome::PassedToEmulator) {
        return Err(format!(
            "Temporary PCSX2 launch refused: {}. Cleanup: {:?}",
            session.receipt().errors.join("; "),
            session.receipt().cleanup
        ));
    }
    Ok(Pcsx2CheatLaunch {
        session,
        observation,
    })
}

/// Recovery uses the same canonical ownership/age/PID gates, plus this
/// adapter's exact sandbox ID. Unknown lifetime evidence always keeps files.
pub(crate) fn workspace_may_be_active(workspace: &Path) -> bool {
    flatpak::workspace_may_be_active(workspace)
}
