//! Source-verified melonDS 1.1 native MCH installation over shared transactions.
//! No ROM/config writes, implicit emulator selection, or runtime effectiveness claim.
use super::*;
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::safe_read::{TrustedRoots, open_bounded_read};
use sha2::{Digest, Sha256};
use std::result::Result;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MelonDsInstallRequest {
    pub rom: PathBuf,
    /// Exact hash supplied by the local cheat source, not inferred from its title.
    pub expected_rom_sha256: String,
    pub source: PathBuf,
    pub expected_source_sha256: String,
    pub selected: Vec<(usize, bool)>,
    pub profile_id: String,
}
#[derive(Clone, Debug)]
struct Evidence {
    path: PathBuf,
    hash: String,
    identity: CapturedFileIdentity,
}
#[derive(Clone, Debug)]
pub struct MelonDsInstallPreview {
    request: MelonDsInstallRequest,
    profile: MelonDsProfile,
    evidence: Vec<Evidence>,
    directories: Vec<(PathBuf, CapturedFileIdentity)>,
    materialized: SharedMaterializedOutput,
    plan: SharedTransactionPlan,
    pub file: MelonDsInstallSource,
    pub destination: PathBuf,
    pub autoload: Option<bool>,
}
impl MelonDsInstallPreview {
    pub fn plan(&self) -> &SharedTransactionPlan {
        &self.plan
    }
    pub fn request(&self) -> &MelonDsInstallRequest {
        &self.request
    }
    pub fn executable(&self) -> &Path {
        &self.evidence[2].path
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
        return Err(format!(
            "File exceeds bounded inspection limit: {}",
            path.display()
        ));
    }
    let mut f = source.into_file();
    let before = f.metadata().map_err(|e| e.to_string())?;
    let id = CapturedFileIdentity::capture(&before);
    let mut bytes = Vec::new();
    (&mut f)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit
        || CapturedFileIdentity::capture(&f.metadata().map_err(|e| e.to_string())?) != id
    {
        return Err("File changed during bounded inspection".into());
    }
    let digest = hash(&bytes);
    Ok((
        bytes,
        Evidence {
            path: path.into(),
            hash: digest,
            identity: id,
        },
    ))
}
fn absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "Existing destination is protected; no merge/replacement is supported: {}",
            path.display()
        )),
        Err(e) => Err(format!("Destination state unavailable: {e}")),
    }
}
/// Resolves only the native installation known by the existing discovery/binding.
/// Remembered profile policy is reused; a missing remembered profile never falls back.
pub fn select_melonds_cheat_profile(
    roots: &MelonDsProfileDiscoveryRoots,
    remembered: Option<&str>,
) -> Result<MelonDsProfile, String> {
    let discovery = discover_melonds_profiles(roots);
    let candidates: Vec<_> = discovery
        .profiles
        .iter()
        .map(|p| EmulatorProfileCandidate {
            profile_id: p.profile_id.clone(),
            root: p.configuration_path.clone(),
            eligible: p.installation_type == MelonDsInstallationType::Native
                && resolve_melonds_native_launch_binding(p).is_ok(),
            is_portable: false,
            evidence_priority: 0,
        })
        .collect();
    let EmulatorProfileSelection::Auto { profile_id, .. } =
        select_emulator_profile(&candidates, remembered, None)
    else {
        return Err("No unambiguous native melonDS profile. Check Emulator setup; unavailable remembered profiles are not replaced automatically.".into());
    };
    discovery
        .profiles
        .into_iter()
        .find(|p| p.profile_id == profile_id)
        .ok_or_else(|| "Resolved profile disappeared".into())
}
/// Only instance zero with an explicit, absolute cheat directory is enabled.
fn config_options(bytes: &[u8]) -> Result<(PathBuf, Option<bool>), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Configuration is not UTF-8")?;
    let config: toml::Value =
        toml::from_str(text).map_err(|e| format!("Invalid melonDS TOML: {e}"))?;
    if config
        .get("Emu")
        .and_then(|e| e.get("ConsoleType"))
        .is_some_and(|v| v.as_integer() != Some(0))
    {
        return Err("Only DS console mode is enabled; DSi configurations are not supported".into());
    }
    let instance = config
        .get("Instance0")
        .and_then(toml::Value::as_table)
        .ok_or("Instance0 configuration is required")?;
    if config
        .as_table()
        .unwrap()
        .keys()
        .any(|k| k.starts_with("Instance") && k != "Instance0")
    {
        return Err("Multi-instance profiles are not enabled".into());
    }
    let root = instance.get("CheatFilePath").and_then(toml::Value::as_str).filter(|s| !s.is_empty()).ok_or("Set an absolute CheatFilePath in melonDS before Preview; default and relative paths are not enabled")?;
    // getAssetPath strips trailing separators before appending the filename.
    // A root-only path becomes relative there; it is not a proven load target.
    let effective = root.trim_end_matches(['/', '\\']);
    if effective.is_empty() || root.contains('\\') || root.chars().any(char::is_control) {
        return Err("CheatFilePath has an ambiguous native load-path mapping".into());
    }
    let root = PathBuf::from(effective);
    check_dir(&root)?;
    let enabled = instance
        .get("EnableCheats")
        .map(|v| v.as_bool().ok_or("EnableCheats must be boolean"))
        .transpose()?;
    Ok((root, enabled))
}
fn check_dir(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Unsafe directory".into());
    }
    for ancestor in path.ancestors() {
        let m = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err("Directory must exist without symlinks".into());
        }
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct MelonDsInstallSource {
    pub entries: Vec<MelonDsCheatEntry>,
    pub issues: Vec<MelonDsCheatParseIssue>,
}
pub fn inspect_melonds_local_cheats(
    source: &Path,
) -> Result<(MelonDsInstallSource, String), String> {
    let (bytes, evidence) = read(source, MELONDS_CHEAT_MAX_BYTES as u64)?;
    Ok((parse_install_source(&bytes)?, evidence.hash))
}
fn parse_install_source(bytes: &[u8]) -> Result<MelonDsInstallSource, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "MCH must be UTF-8")?;
    // The legacy parser accepts a broader grammar. Installation only accepts
    // canonical 1.1 exports, preventing prefix/name/state reinterpretation.
    let mut scope = false;
    let mut code = false;
    for raw in text.lines() {
        // Upstream fgets uses a 1024-byte buffer even for comments. Ignoring
        // an overlong comment here could expose its continuation as native code.
        if raw.len() > 1022 || raw.chars().any(char::is_control) {
            return Err("MCH control characters or overlong line refused".into());
        }
        let line = raw.trim_matches(' ');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "ROOT" {
            scope = true;
            code = false;
        } else if let Some(name) = line
            .strip_prefix("CAT 0 ")
            .or_else(|| line.strip_prefix("CAT 1 "))
        {
            if name.is_empty() || name.len() > 127 {
                return Err("MCH category name exceeds upstream limits".into());
            }
            scope = true;
            code = false;
        } else if let Some(name) = line
            .strip_prefix("CODE 0 ")
            .or_else(|| line.strip_prefix("CODE 1 "))
        {
            if !scope || name.is_empty() || name.len() > 127 {
                return Err("MCH code requires ROOT/CAT and a name of at most 127 bytes".into());
            }
            code = true;
        } else if let Some(desc) = line.strip_prefix("DESC ") {
            if !scope || desc.is_empty() || desc.len() > 255 {
                return Err("MCH description is out of scope or too long".into());
            }
        } else if !code
            || !line.is_ascii()
            || line.len() != 17
            || line.as_bytes()[8] != b' '
            || !line[..8].bytes().all(|b| b.is_ascii_hexdigit())
            || !line[9..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(
                "Only canonical melonDS 1.1 MCH directives and hexadecimal word pairs are enabled"
                    .into(),
            );
        }
    }
    let file = parse_melonds_cheat_file(text);
    let mut entries = Vec::new();
    for item in file.items {
        match item {
            MelonDsCheatItem::RootCode(e) => entries.push(e),
            MelonDsCheatItem::Category(c) => entries.extend(c.entries),
        }
    }
    Ok(MelonDsInstallSource {
        entries,
        issues: file.issues,
    })
}
/// Only unambiguous, aligned main-RAM direct writes; no opcode translation.
pub fn melonds_entry_installable(entry: &MelonDsCheatEntry) -> bool {
    !entry.name.is_empty()
        && entry.name.len() <= 127
        && entry
            .description
            .as_ref()
            .is_none_or(|d| d.len() <= 255 && !d.contains(['\r', '\n', '\0']))
        && !entry.name.contains(['\r', '\n', '\0'])
        && !entry.code.words.is_empty()
        && entry.code.words.iter().all(|(a, b)| {
            let line = format!("{a:08X} {b:08X}");
            match ds_action_replay_line_to_ir(&line) {
                DsActionReplayClassification::DirectWrite(CheatOperation::Write8 {
                    address,
                    ..
                }) => (0x02000000..=0x023fffff).contains(&address),
                DsActionReplayClassification::DirectWrite(CheatOperation::Write16 {
                    address,
                    ..
                }) => address % 2 == 0 && (0x02000000..=0x023ffffe).contains(&address),
                DsActionReplayClassification::DirectWrite(CheatOperation::Write32 {
                    address,
                    ..
                }) => address % 4 == 0 && (0x02000000..=0x023ffffc).contains(&address),
                _ => false,
            }
        })
}
/// Same basename rule as 1.1 getAssetPath. Refuse fallback/ambiguous inputs.
fn cheat_filename(rom: &Path) -> Result<PathBuf, String> {
    let path = rom
        .to_str()
        .ok_or("Non-Unicode ROM paths have no proven Qt mapping")?;
    if path.contains(['|', '\\']) || path.chars().any(char::is_control) {
        return Err("Archive-member syntax and ambiguous ROM paths are not enabled".into());
    }
    let name = rom
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("ROM filename is unavailable")?;
    let (stem, extension) = name
        .rsplit_once('.')
        .ok_or("Only direct .nds files are enabled")?;
    if stem.is_empty() || !extension.eq_ignore_ascii_case("nds") {
        return Err("A nonempty DS ROM basename ending in .nds is required".into());
    }
    Ok(PathBuf::from(format!("{stem}.mch")))
}
pub fn preview_melonds_install(
    request: &MelonDsInstallRequest,
    roots: &MelonDsProfileDiscoveryRoots,
    remembered: Option<&str>,
) -> Result<MelonDsInstallPreview, String> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(
            "Blue melonDS installation currently supports native Linux profiles only".into(),
        );
    }
    let profile = select_melonds_cheat_profile(roots, remembered)?;
    if profile.profile_id != request.profile_id {
        return Err("melonDS selection changed; make a fresh Preview".into());
    }
    let executable = resolve_melonds_native_launch_binding(&profile)
        .map_err(|e| e.detail)?
        .executable;
    let config = profile.configuration_path.join("melonDS.toml");
    if profile.config_path.as_ref() != Some(&config) {
        return Err(
            "An existing native melonDS.toml is required; backup configs are not active profiles"
                .into(),
        );
    }
    let (config_bytes, config_evidence) = read(&config, MELONDS_MAX_CONFIG_BYTES)?;
    let (cheats_root, autoload) = config_options(&config_bytes)?;
    check_dir(&profile.configuration_path)?;
    check_dir(&cheats_root)?;
    let (_, executable_evidence) = read(&executable, 64 * 1024 * 1024)?;
    if executable_evidence.hash
        != "5f80351b63e2a8fd7b7cdf8b61c07761c6240d0c586601b72b788ad6147121ce"
    {
        #[cfg(not(test))]
        return Err(
            "Only the exact official melonDS 1.1 Ubuntu x86-64 native binary is enabled".into(),
        );
        #[cfg(test)]
        if executable_evidence.hash != hash(b"synthetic melonDS 1.1") {
            return Err("Unsupported melonDS executable".into());
        }
    }
    if executable.to_str().is_none() || profile.configuration_path.to_str().is_none() {
        return Err("Non-Unicode executable/profile paths have no proven Qt mapping".into());
    }
    absent(
        &executable
            .parent()
            .ok_or("Executable parent missing")?
            .join("portable"),
    )
    .map_err(|_| "Portable override is present or its absence cannot be established".to_string())?;
    if profile.configuration_path != roots.xdg_config_home.join("melonDS") {
        return Err("Only the ordinary XDG melonDS profile is enabled".into());
    }
    let filename = cheat_filename(&request.rom)?;
    let (_, rom_evidence) = read(&request.rom, 512 * 1024 * 1024)?;
    if request.expected_rom_sha256.len() != 64
        || !request
            .expected_rom_sha256
            .eq_ignore_ascii_case(&rom_evidence.hash)
    {
        return Err("Cheat-source ROM SHA-256 is missing or differs from the selected ROM".into());
    }
    let (source_bytes, source_evidence) = read(&request.source, MELONDS_CHEAT_MAX_BYTES as u64)?;
    if source_evidence.hash != request.expected_source_sha256 {
        return Err("Local cheat source changed since selection; choose it again".into());
    }
    let original = parse_install_source(&source_bytes)?;
    if !original.issues.is_empty() {
        return Err("Malformed/truncated/ambiguous native MCH source is protected".into());
    }
    if request.selected.len() != 1 {
        return Err(
            "Select exactly one supported entry; category/exclusive combinations are not installed"
                .into(),
        );
    }
    let (index, enabled) = request.selected[0];
    let mut entry = original
        .entries
        .get(index)
        .filter(|e| melonds_entry_installable(e))
        .ok_or("Only aligned 8/16/32-bit DS main-RAM direct writes are enabled")?
        .clone();
    entry.state = if enabled {
        MelonDsCheatState::Enabled
    } else {
        MelonDsCheatState::Disabled
    };
    let file = MelonDsInstallSource {
        entries: vec![entry.clone()],
        issues: vec![],
    };
    let destination = cheats_root.join(&filename);
    absent(&destination)?;
    let output_bytes = render_melonds_cheat_file(&MelonDsCheatFile {
        format: MelonDsCheatFormat::MchText,
        items: vec![MelonDsCheatItem::RootCode(entry)],
        comments: vec![],
        issues: vec![],
    });
    let rendered = parse_install_source(&output_bytes)?;
    if !rendered.issues.is_empty() || rendered.entries != file.entries {
        return Err("Generated MCH does not preserve the reviewed entry".into());
    }
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::MelonDs,
        selected_archive: request.rom.clone(),
        platform: Some("NDS".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::MelonDsRomIdentity,
            state: PreviewIdentityState::Verified,
            value: Some(rom_evidence.hash.clone()),
            archive_path: request.rom.clone(),
            revision: None,
        },
        destination_root: cheats_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::MelonDs,
            source_path: request.source.clone(),
            expected_source_digest: Some(source_evidence.hash.clone()),
            destination_relative_paths: vec![filename.into()],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|e| format!("Shared preview refused: {e:?}"))?;
    let plan = build_shared_transaction_plan(
        &report,
        &profile.profile_id,
        "melonds-native-local-cheats",
        request.source.parent().ok_or("No source parent")?,
    )
    .map_err(|e| format!("Shared plan refused: {e:?}"))?;
    let directories = [&profile.configuration_path, &cheats_root]
        .into_iter()
        .map(|p| {
            fs::symlink_metadata(p)
                .map(|m| (p.clone(), CapturedFileIdentity::capture(&m)))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let evidence = vec![
        rom_evidence,
        config_evidence,
        executable_evidence,
        source_evidence,
    ];
    let materialized = SharedMaterializedOutput {
        source_path: request.source.clone(),
        source_digest: hash(&source_bytes),
        output_digest: hash(&output_bytes),
        bytes: output_bytes,
        guard_paths: evidence[..3]
            .iter()
            .map(|e| (e.path.clone(), e.hash.clone()))
            .collect(),
    };
    Ok(MelonDsInstallPreview {
        request: request.clone(),
        profile,
        evidence,
        directories,
        materialized,
        plan,
        file,
        destination,
        autoload,
    })
}
/// Read-only attention reporting using the existing authoritative decoder.
/// A retained pending record is never silently treated as completed, including
/// terminal-labelled records whose removal/acknowledgement was interrupted.
pub fn melonds_install_attention(history: &Path, rom: &Path) -> Result<Vec<String>, String> {
    match fs::symlink_metadata(history) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("History is unavailable: {e}")),
        Ok(m) if !m.is_dir() || m.file_type().is_symlink() => {
            return Err("History root is unsafe".into());
        }
        _ => {}
    }
    check_dir(history)?;
    let mut attention = Vec::new();
    let mut count = 0;
    let mut total = 0u64;
    for entry in fs::read_dir(history).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(".pending.json"))
        {
            continue;
        }
        count += 1;
        total = total
            .checked_add(
                fs::symlink_metadata(&path)
                    .map_err(|e| e.to_string())?
                    .len(),
            )
            .ok_or("History size overflow")?;
        if count > 128 || total > 16 * 1024 * 1024 {
            return Err("Pending evidence exceeds bounded review; use History & Logs".into());
        }
        let journal = super::shared_transaction::read_pending_journal(&path).map_err(|e| {
            format!(
                "Uninterpretable pending evidence retained at {}: {e:?}",
                path.display()
            )
        })?;
        if journal.context.adapter == PreviewAdapter::MelonDs
            && journal
                .context
                .selected_archive
                .to_path_buf()
                .map_err(|e| format!("{e:?}"))?
                == rom
        {
            attention.push(format!("{} · recorded {:?} · {}. Retained pending evidence requires operator review; no automatic resume is supported here.",journal.operation_id,journal.transaction_state,path.display()));
        }
    }
    Ok(attention)
}
pub fn apply_melonds_install(
    preview: &MelonDsInstallPreview,
    roots: &MelonDsProfileDiscoveryRoots,
    remembered: Option<&str>,
    options: &SharedApplyOptions,
) -> Result<SharedApplyResult, String> {
    if !melonds_install_attention(&options.history_root, &preview.request.rom)?.is_empty() {
        return Err(
            "Earlier pending melonDS operation requires review; new installation refused".into(),
        );
    }
    let fresh = preview_melonds_install(&preview.request, roots, remembered)?;
    if fresh.profile != preview.profile
        || fresh.plan != preview.plan
        || fresh.directories != preview.directories
    {
        return Err("melonDS profile/preview changed; Preview again".into());
    }
    for old in &preview.evidence {
        let (_, now) = read(&old.path, 512 * 1024 * 1024)?;
        if now.hash != old.hash || now.identity != old.identity {
            return Err(format!("Reviewed file changed: {}", old.path.display()));
        }
    }
    absent(&preview.destination)?;
    let mut result =
        execute_shared_materialized_apply(&preview.plan, options, &preview.materialized);
    if result.journal.status == SharedApplyStatus::Success {
        let verified = matches!(result.journal.entries.as_slice(), [entry]
            if entry.outcome == SharedApplyOutcome::InstalledNew && entry.verification_succeeded
                && entry.final_destination_digest.as_deref() == Some(preview.materialized.output_digest.as_str()));
        if !verified || result.journal_path.is_none() || result.journal_failure.is_some() {
            // The shared API can describe verified bytes alongside a failed
            // terminal receipt. Do not turn that nested outcome into success.
            result.journal.status = if result.journal.entries.iter().any(|e| {
                matches!(
                    e.outcome,
                    SharedApplyOutcome::InstalledNew | SharedApplyOutcome::ReplacedExisting
                )
            }) {
                SharedApplyStatus::PartialFailure
            } else {
                SharedApplyStatus::Failed
            };
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
