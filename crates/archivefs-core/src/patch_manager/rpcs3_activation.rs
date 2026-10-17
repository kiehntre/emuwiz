//! Bounded Blue bridge for existing RPCS3 definitions. Only an absent
//! patch_config.yml may be created; no definition database is ever targeted.
use super::*;
use crate::game_identity::{
    IdentityKind, IdentityPlatform, IdentityStatus, inspect_catalogued_game_identity,
};
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::param_sfo::{MAX_SFO_BYTES, SfoValue, parse_param_sfo};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use sha2::{Digest, Sha256};
use std::result::Result;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const RPCS3_ACTIVATION_SOURCE_MODE: &str = "rpcs3-blue-local-activation";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rpcs3ActivationSelection {
    pub game: PathBuf,
    pub roots: Rpcs3ProfileDiscoveryRoots,
    pub profile_id: String,
    pub configuration_root: PathBuf,
    pub executable: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rpcs3ActivationEntry {
    pub source: PathBuf,
    pub source_sha256: String,
    pub hash: String,
    pub definition: Rpcs3PatchEntryDefinition,
    pub readiness: Rpcs3PatchReadiness,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rpcs3ActivationCatalog {
    pub title_id: String,
    pub app_version: String,
    pub entries: Vec<Rpcs3ActivationEntry>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rpcs3ActivationRequest {
    pub selection: Rpcs3ActivationSelection,
    pub source: PathBuf,
    pub expected_source_sha256: String,
    pub hash: String,
    pub description: String,
    pub expected_title_id: String,
    pub expected_app_version: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Evidence {
    path: PathBuf,
    digest: String,
    identity: CapturedFileIdentity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inspection {
    catalog: Rpcs3ActivationCatalog,
    evidence: Vec<Evidence>,
    absent: Vec<PathBuf>,
    directories: Vec<(PathBuf, CapturedFileIdentity)>,
}
#[derive(Clone, Debug)]
pub struct Rpcs3ActivationPreview {
    request: Rpcs3ActivationRequest,
    inspection: Inspection,
    plan: SharedTransactionPlan,
    materialized: SharedMaterializedOutput,
    pub config_bytes: Vec<u8>,
    pub destination: PathBuf,
}
impl Rpcs3ActivationPreview {
    pub fn plan(&self) -> &SharedTransactionPlan {
        &self.plan
    }
    pub fn request(&self) -> &Rpcs3ActivationRequest {
        &self.request
    }
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn read(path: &Path, limit: u64) -> Result<(Vec<u8>, Evidence), String> {
    let source = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    if source.len() > limit {
        return Err(format!("Bounded file limit exceeded: {}", path.display()));
    }
    let mut file = source.into_file();
    let before = CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?);
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit
        || before != CapturedFileIdentity::capture(&file.metadata().map_err(|e| e.to_string())?)
    {
        return Err("File changed during bounded inspection".into());
    }
    Ok((
        bytes.clone(),
        Evidence {
            path: path.into(),
            digest: hash(&bytes),
            identity: before,
        },
    ))
}
fn absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "Protected or unsupported existing path: {}",
            path.display()
        )),
        Err(e) => Err(format!(
            "Cannot establish absence of {}: {e}",
            path.display()
        )),
    }
}
fn directory(path: &Path) -> Result<CapturedFileIdentity, String> {
    if !path.is_absolute() {
        return Err("Absolute directories are required".into());
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        ) {
            return Err("Unresolved directory component".into());
        }
        current.push(component.as_os_str());
        let m = fs::symlink_metadata(&current).map_err(|e| e.to_string())?;
        if m.file_type().is_symlink() || !m.is_dir() {
            return Err(format!("Unsafe directory: {}", current.display()));
        }
    }
    Ok(CapturedFileIdentity::capture(
        &fs::symlink_metadata(path).map_err(|e| e.to_string())?,
    ))
}
fn unique_sfo(bytes: &[u8], key: &str) -> Result<String, String> {
    let sfo = parse_param_sfo(bytes).ok_or("Invalid PARAM.SFO")?;
    let values: Vec<_> = sfo.entries.iter().filter(|e| e.key == key).collect();
    match values.as_slice() {
        [entry] => match &entry.value {
            SfoValue::Text(v) => Ok(v.clone()),
            _ => Err(format!("Non-text {key}")),
        },
        _ => Err(format!("Missing or duplicate PARAM.SFO {key}")),
    }
}
fn inspect(selection: &Rpcs3ActivationSelection) -> Result<Inspection, String> {
    if !cfg!(target_os = "linux")
        || selection.roots.appimage_directory.is_some()
        || std::env::var_os("APPIMAGE").is_some()
    {
        return Err(
            "Only native Linux RPCS3 without inherited APPIMAGE routing is supported".into(),
        );
    }
    directory(&selection.game)?;
    if selection.configuration_root.starts_with(&selection.game)
        || selection.game.starts_with(&selection.configuration_root)
    {
        return Err("Game and emulator configuration roots must not overlap".into());
    }
    let discovery = discover_rpcs3_profiles(&selection.roots);
    if !discovery.complete {
        return Err("RPCS3 profile discovery is incomplete".into());
    }
    let profile = discovery
        .profiles
        .iter()
        .find(|p| p.profile_id == selection.profile_id)
        .ok_or("Launch profile is no longer available")?;
    if profile.installation_type != Rpcs3InstallationType::Native
        || profile.configuration_path != selection.configuration_root
        || profile.configuration_path != selection.roots.xdg_config_home.join("rpcs3")
    {
        return Err("Only the exact native XDG Launch profile is installation-supported".into());
    }
    let binding = resolve_rpcs3_native_launch_binding(profile).map_err(|e| e.detail)?;
    if binding.executable != selection.executable
        || !matches!(
            binding.installation,
            crate::launch::installation::LaunchInstallation::Native
        )
        || selection
            .executable
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("appimage"))
    {
        return Err("RPCS3 executable or installation changed; Preview again".into());
    }
    let mut evidence = Vec::new();
    let (executable, executable_evidence) = read(&selection.executable, 256 * 1024 * 1024)?;
    if !executable.starts_with(b"\x7fELF") {
        return Err("Only a native ELF executable has a supported profile contract; wrappers are unavailable".into());
    }
    evidence.push(executable_evidence);
    let (_, global_config) = read(
        &profile.global_config_path,
        super::rpcs3_local::RPCS3_MAX_CONFIG_BYTES,
    )?;
    evidence.push(global_config);
    let mut absent_paths = vec![
        profile.configuration_path.join("vfs.yml"),
        selection
            .executable
            .parent()
            .ok_or("Executable parent missing")?
            .join("portable"),
    ];
    for p in &absent_paths {
        absent(p)?;
    }
    let game_dir = if selection.game.file_name().is_some_and(|n| n == "PS3_GAME") {
        selection.game.clone()
    } else {
        selection.game.join("PS3_GAME")
    };
    let (sfo, sfo_evidence) = read(&game_dir.join("PARAM.SFO"), MAX_SFO_BYTES as u64)?;
    let title_id = unique_sfo(&sfo, "TITLE_ID")?;
    let app_version = unique_sfo(&sfo, "APP_VER")?;
    if !super::rpcs3_patch::valid_title_id(&title_id)
        || !super::rpcs3_patch::valid_app_version(&app_version)
    {
        return Err("A valid exact TITLE_ID and APP_VER are required".into());
    }
    evidence.push(sfo_evidence);
    let (self_bytes, self_evidence) = read(&game_dir.join("USRDIR/EBOOT.BIN"), 64 * 1024 * 1024)?;
    if !self_bytes.starts_with(b"SCE\0") {
        return Err("PS3 SELF evidence is unavailable".into());
    }
    evidence.push(self_evidence);
    let identity = inspect_catalogued_game_identity(&selection.game, Some("PS3"));
    let title_evidence: Vec<_> = identity
        .evidence
        .iter()
        .filter(|e| e.kind == IdentityKind::Ps3TitleId)
        .collect();
    if !identity.complete
        || identity.platform != IdentityPlatform::PlayStation3
        || title_evidence.is_empty()
        || title_evidence
            .iter()
            .any(|e| e.status != IdentityStatus::Verified || e.value.as_deref() != Some(&title_id))
    {
        return Err("Core PS3 identity does not verify this exact folder and TITLE_ID".into());
    }
    absent_paths.push(profile.dev_hdd0_path.join("game").join(&title_id));
    absent_paths.push(profile.configuration_path.join("patch_config.yml"));
    for p in &absent_paths {
        absent(p)?;
    }
    let paths = [
        profile.patches_path.join("patch.yml"),
        profile.patches_path.join("imported_patch.yml"),
        profile.patches_path.join(format!("{title_id}_patch.yml")),
    ];
    let mut entries = Vec::new();
    let mut identifiers = BTreeSet::new();
    for path in paths {
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                absent_paths.push(path);
                continue;
            }
            Err(e) => return Err(e.to_string()),
            Ok(_) => {}
        }
        let (bytes, observed) = read(&path, RPCS3_PATCH_MAX_BYTES as u64)?;
        let file = super::rpcs3_patch::inspect_rpcs3_patch_subset(&bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        for group in file.groups {
            for entry in group.entries {
                if !identifiers.insert((group.hash.clone(), entry.description.clone())) {
                    return Err("Conflicting/duplicate patch identifiers across installed databases; activation refused".into());
                }
                entries.push(Rpcs3ActivationEntry {
                    source: path.clone(),
                    source_sha256: observed.digest.clone(),
                    hash: group.hash.clone(),
                    readiness: rpcs3_patch_readiness(&entry, &title_id, &app_version),
                    definition: entry,
                });
            }
        }
        evidence.push(observed);
    }
    if entries.len() > RPCS3_PATCH_MAX_ENTRIES {
        return Err("Combined local patch entry limit exceeded".into());
    }
    let mut dirs = vec![
        profile.configuration_path.clone(),
        profile.patches_path.clone(),
        selection.game.clone(),
        game_dir.clone(),
        game_dir.join("USRDIR"),
        selection.executable.parent().unwrap().into(),
    ];
    // Existing parents of required-absent paths are also stable-preview facts.
    for p in &absent_paths {
        let mut parent = p.parent();
        while let Some(p) = parent {
            if fs::symlink_metadata(p).is_ok() {
                dirs.push(p.into());
                break;
            }
            parent = p.parent();
        }
    }
    dirs.sort();
    dirs.dedup();
    let directories = dirs
        .into_iter()
        .map(|p| directory(&p).map(|id| (p, id)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Inspection {
        catalog: Rpcs3ActivationCatalog {
            title_id,
            app_version,
            entries,
        },
        evidence,
        absent: absent_paths,
        directories,
    })
}
pub fn inspect_rpcs3_activation(
    selection: &Rpcs3ActivationSelection,
) -> Result<Rpcs3ActivationCatalog, String> {
    inspect(selection).map(|i| i.catalog)
}
/// The mode tag binds confirmation to the complete reviewed activation facts,
/// without changing the shared plan/journal schema.
fn approval_mode(inspection: &Inspection, config: &[u8]) -> Result<String, String> {
    let identity = |id: &CapturedFileIdentity| (id.device, id.inode, id.size, id.modified);
    let files: Vec<_> = inspection
        .evidence
        .iter()
        .map(|e| {
            (
                SharedTransactionPath::from_path(&e.path),
                &e.digest,
                identity(&e.identity),
            )
        })
        .collect();
    let absent: Vec<_> = inspection
        .absent
        .iter()
        .map(|p| SharedTransactionPath::from_path(p))
        .collect();
    let directories: Vec<_> = inspection
        .directories
        .iter()
        .map(|(p, id)| (SharedTransactionPath::from_path(p), identity(id)))
        .collect();
    let facts = serde_json::to_vec(&(
        RPCS3_ACTIVATION_SOURCE_MODE,
        &inspection.catalog.title_id,
        &inspection.catalog.app_version,
        config,
        files,
        absent,
        directories,
    ))
    .map_err(|e| format!("Cannot bind exact activation approval: {e}"))?;
    Ok(format!("{RPCS3_ACTIVATION_SOURCE_MODE}:{}", hash(&facts)))
}
pub fn is_rpcs3_activation_source_mode(mode: &str) -> bool {
    mode.strip_prefix(RPCS3_ACTIVATION_SOURCE_MODE)
        .and_then(|suffix| suffix.strip_prefix(':'))
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
pub fn preview_rpcs3_activation(
    request: &Rpcs3ActivationRequest,
) -> Result<Rpcs3ActivationPreview, String> {
    let inspection = inspect(&request.selection)?;
    if inspection.catalog.title_id != request.expected_title_id
        || inspection.catalog.app_version != request.expected_app_version
    {
        return Err("Game TITLE_ID/version changed; inspect again".into());
    }
    let entry = inspection
        .catalog
        .entries
        .iter()
        .find(|e| {
            e.source == request.source
                && e.hash == request.hash
                && e.definition.description == request.description
        })
        .ok_or("Selected installed patch is unavailable")?;
    if entry.source_sha256 != request.expected_source_sha256 {
        return Err("Patch definition changed; inspect again".into());
    }
    let config_bytes = super::rpcs3_patch::render_rpcs3_patch_activation_config(
        &entry.hash,
        &entry.definition,
        &inspection.catalog.title_id,
        &inspection.catalog.app_version,
        if request.enabled {
            Rpcs3PatchState::Enabled
        } else {
            Rpcs3PatchState::Disabled
        },
    )
    .map_err(|e| e.to_string())?;
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Rpcs3Patch,
        selected_archive: request.selection.game.clone(),
        platform: Some("PS3".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::Rpcs3TitleId,
            state: PreviewIdentityState::Verified,
            value: Some(inspection.catalog.title_id.clone()),
            archive_path: request.selection.game.clone(),
            revision: None,
        },
        destination_root: request.selection.configuration_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Rpcs3Patch,
            source_path: entry.source.clone(),
            expected_source_digest: Some(entry.source_sha256.clone()),
            destination_relative_paths: vec!["patch_config.yml".into()],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|e| e.to_string())?;
    let plan = build_shared_transaction_plan(
        &report,
        &request.selection.profile_id,
        &approval_mode(&inspection, &config_bytes)?,
        request.source.parent().ok_or("Source parent unavailable")?,
    )
    .map_err(|e| format!("{e:?}"))?;
    let materialized = SharedMaterializedOutput {
        source_path: entry.source.clone(),
        source_digest: entry.source_sha256.clone(),
        output_digest: hash(&config_bytes),
        bytes: config_bytes.clone(),
        guard_paths: inspection
            .evidence
            .iter()
            .map(|e| (e.path.clone(), e.digest.clone()))
            .collect(),
    };
    Ok(Rpcs3ActivationPreview {
        request: request.clone(),
        inspection,
        plan,
        materialized,
        config_bytes,
        destination: request
            .selection
            .configuration_root
            .join("patch_config.yml"),
    })
}
pub fn rpcs3_activation_attention(history: &Path) -> Result<(), String> {
    match fs::symlink_metadata(history) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
        Ok(_) => {
            directory(history)?;
        }
    }
    let mut count = 0usize;
    for entry in fs::read_dir(history).map_err(|e| e.to_string())? {
        count += 1;
        if count > 4096 {
            return Err("History scan exceeds safe bound".into());
        }
        let path = entry.map_err(|e| e.to_string())?.path();
        if !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(".pending.json"))
        {
            continue;
        }
        let journal = super::shared_transaction::read_pending_journal(&path)
            .map_err(|e| format!("Pending evidence is uncertain; inspect History: {e:?}"))?;
        if journal.context.adapter == PreviewAdapter::Rpcs3Patch {
            return Err("Retained RPCS3 pending operation requires History/recovery review before another Apply".into());
        }
    }
    Ok(())
}
pub fn apply_rpcs3_activation(
    preview: &Rpcs3ActivationPreview,
    current: &Rpcs3ActivationSelection,
    options: &SharedApplyOptions,
) -> Result<SharedApplyResult, String> {
    apply_with_probe(preview, current, options, || {
        observe_emulator_process(&CheatRouteTarget::standalone("rpcs3"))
    })
}
fn apply_with_probe(
    preview: &Rpcs3ActivationPreview,
    current: &Rpcs3ActivationSelection,
    options: &SharedApplyOptions,
    process: impl FnOnce() -> EmulatorProcessObservation,
) -> Result<SharedApplyResult, String> {
    if options.history_root.starts_with(&current.game)
        || options.backup_root.starts_with(&current.game)
    {
        return Err("History and backups may not write inside game content".into());
    }
    if current != &preview.request.selection {
        return Err("RPCS3 Launch installation/profile selection changed after Preview".into());
    }
    rpcs3_activation_attention(&options.history_root)?;
    let fresh = preview_rpcs3_activation(&preview.request)?;
    if fresh.inspection != preview.inspection
        || fresh.plan != preview.plan
        || fresh.config_bytes != preview.config_bytes
    {
        return Err("Game, profile, definitions or file freshness changed after Preview".into());
    }
    for p in &preview.inspection.absent {
        absent(p)?;
    }
    for (p, id) in &preview.inspection.directories {
        if directory(p)? != *id {
            return Err("Reviewed directory changed".into());
        }
    }
    if process() != EmulatorProcessObservation::NotRunning {
        return Err("RPCS3 must be closed; process state is running or unverified".into());
    }
    let mut result =
        execute_shared_materialized_apply(&preview.plan, options, &preview.materialized);
    if result.journal.status == SharedApplyStatus::Success {
        let verified = matches!(result.journal.entries.as_slice(), [entry] if entry.outcome == SharedApplyOutcome::InstalledNew && entry.verification_succeeded && entry.final_destination_digest.as_deref() == Some(preview.materialized.output_digest.as_str()));
        if !verified || result.journal_path.is_none() || result.journal_failure.is_some() {
            result.journal.status = SharedApplyStatus::PartialFailure;
        }
    }
    Ok(result)
}
#[cfg(all(test, target_os = "linux"))]
mod tests;
