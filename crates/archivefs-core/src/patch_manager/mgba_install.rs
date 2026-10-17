//! Bounded Blue mGBA bridge: native file inspection + existing shared transactions.
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
pub struct MgbaInstallRequest {
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
pub struct MgbaInstallPreview {
    request: MgbaInstallRequest,
    profile: MgbaProfile,
    evidence: Vec<Evidence>,
    directories: Vec<(PathBuf, CapturedFileIdentity)>,
    materialized: SharedMaterializedOutput,
    plan: SharedTransactionPlan,
    pub file: MgbaCheatFile,
    pub destination: PathBuf,
    pub autoload: Option<bool>,
}
impl MgbaInstallPreview {
    pub fn plan(&self) -> &SharedTransactionPlan {
        &self.plan
    }
    pub fn request(&self) -> &MgbaInstallRequest {
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
pub fn select_mgba_cheat_profile(
    roots: &MgbaProfileDiscoveryRoots,
    remembered: Option<&str>,
) -> Result<MgbaProfile, String> {
    let discovery = discover_mgba_profiles(roots);
    let candidates: Vec<_> = discovery
        .profiles
        .iter()
        .map(|p| EmulatorProfileCandidate {
            profile_id: p.profile_id.clone(),
            root: p.configuration_path.clone(),
            eligible: p.installation_type == MgbaInstallationType::Native
                && resolve_mgba_native_launch_binding(p).is_ok(),
            is_portable: false,
            evidence_priority: 0,
        })
        .collect();
    let EmulatorProfileSelection::Auto { profile_id, .. } =
        select_emulator_profile(&candidates, remembered, None)
    else {
        return Err("No unambiguous native mGBA profile. Check Emulator setup; unavailable remembered profiles are not replaced automatically.".into());
    };
    discovery
        .profiles
        .into_iter()
        .find(|p| p.profile_id == profile_id)
        .ok_or_else(|| "Resolved profile disappeared".into())
}
/// Native mGBA global/Qt options only. Relative, conflicting, absent, or
/// other-port paths are refused rather than guessing mGBA's fallback directory.
fn config_options(bytes: &[u8]) -> Result<(PathBuf, Option<bool>), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "mGBA configuration is not UTF-8")?;
    let mut section = String::new();
    let mut path: Option<PathBuf> = None;
    let mut autoload = None;
    let mut global_path = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].into();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "cheatsPath" || key == "cheatAutoload" {
            if !matches!(section.as_str(), "" | "ports.qt") {
                return Err("Port-specific cheat configuration is unsupported".into());
            }
            if key == "cheatsPath" {
                global_path |= section.is_empty();
                let candidate = PathBuf::from(value);
                if !candidate.is_absolute() || value.is_empty() {
                    return Err(
                        "Configure an explicit absolute mGBA cheatsPath first; no path is guessed"
                            .into(),
                    );
                }
                if path.as_ref().is_some_and(|p| p != &candidate) {
                    return Err("Conflicting cheatsPath settings".into());
                }
                path = Some(candidate);
            } else {
                let candidate = match value {
                    "0" => false,
                    "1" => true,
                    _ => return Err("Unknown cheatAutoload value".into()),
                };
                if autoload.is_some_and(|a| a != candidate) {
                    return Err("Conflicting cheatAutoload settings".into());
                }
                autoload = Some(candidate);
            }
        }
    }
    if !global_path {
        return Err("A global cheatsPath is required; Qt-only/other-port destinations cannot be inferred for this executable".into());
    }
    Ok((
        path.ok_or(
            "No explicit cheatsPath is configured; automatic/ROM-adjacent paths are unsupported",
        )?,
        autoload,
    ))
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
/// Reads the existing native parser. Blue installs only its proven, non-ROM
/// VBA 8-bit RAM-write subset; opaque/encrypted/directive variants remain visible.
pub fn inspect_mgba_local_cheats(source: &Path) -> Result<(MgbaCheatFile, String), String> {
    let (bytes, evidence) = read(source, MGBA_CHEAT_MAX_BYTES as u64)?;
    Ok((
        parse_mgba_cheat_file(&bytes).map_err(|e| e.to_string())?,
        evidence.hash,
    ))
}
pub fn mgba_entry_installable(entry: &MgbaCheatEntry) -> bool {
    !entry.name.is_empty()
        && !entry.codes.is_empty()
        && entry.directives.is_empty()
        && entry.codes.iter().all(|c| {
            matches!(
                c.normalized,
                Some(CheatOperation::Write8 {
                    address: 0x02000000..=0x0203ffff | 0x03000000..=0x03007fff,
                    ..
                })
            ) && c.raw.len() == 11
                && c.raw.as_bytes()[8] == b':'
        })
}
pub fn preview_mgba_install(
    request: &MgbaInstallRequest,
    roots: &MgbaProfileDiscoveryRoots,
    remembered: Option<&str>,
) -> Result<MgbaInstallPreview, String> {
    if !cfg!(target_os = "linux") {
        return Err("Blue mGBA installation currently supports native Linux profiles only".into());
    }
    let profile = select_mgba_cheat_profile(roots, remembered)?;
    if profile.profile_id != request.profile_id {
        return Err("mGBA selection changed; make a fresh Preview".into());
    }
    let executable = resolve_mgba_native_launch_binding(&profile)
        .map_err(|e| e.detail)?
        .executable;
    let config = profile.configuration_path.join("config.ini");
    if profile.config_path.as_ref() != Some(&config) {
        return Err(
            "An existing native config.ini is required; backup configs are not active profiles"
                .into(),
        );
    }
    let (config_bytes, config_evidence) = read(&config, MGBA_MAX_CONFIG_BYTES)?;
    let (cheats_root, autoload) = config_options(&config_bytes)?;
    check_dir(&profile.configuration_path)?;
    check_dir(&cheats_root)?;
    let (_, executable_evidence) = read(&executable, 64 * 1024 * 1024)?;
    if request
        .rom
        .extension()
        .is_none_or(|s| !s.eq_ignore_ascii_case("gba"))
    {
        return Err("Only direct Game Boy Advance .gba files are supported".into());
    }
    let (_, rom_evidence) = read(&request.rom, 32 * 1024 * 1024)?;
    if request.expected_rom_sha256.len() != 64
        || !request
            .expected_rom_sha256
            .eq_ignore_ascii_case(&rom_evidence.hash)
    {
        return Err("Cheat-source ROM SHA-256 is missing or differs from the selected ROM".into());
    }
    let (source_bytes, source_evidence) = read(&request.source, MGBA_CHEAT_MAX_BYTES as u64)?;
    if source_evidence.hash != request.expected_source_sha256 {
        return Err("Local cheat source changed since selection; choose it again".into());
    }
    let original = parse_mgba_cheat_file(&source_bytes).map_err(|e| e.to_string())?;
    if original
        .issues
        .iter()
        .any(|issue| !matches!(issue, MgbaCheatIssue::UnsupportedCode(_)))
    {
        return Err(
            "Malformed or truncated native cheat source; no installation is supported".into(),
        );
    }
    let mut file = MgbaCheatFile {
        entries: vec![],
        issues: vec![],
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut addresses = std::collections::BTreeSet::new();
    for &(i, enabled) in &request.selected {
        let entry = original.entries.get(i).filter(|e| mgba_entry_installable(e)).ok_or("Selected entry is unsupported; only directive-free VBA 8-bit GBA RAM writes are installable")?;
        if !seen.insert(i) {
            return Err("Duplicate selected entry".into());
        }
        for code in &entry.codes {
            if let Some(CheatOperation::Write8 { address, .. }) = code.normalized {
                if !addresses.insert(address) {
                    return Err(
                        "Selected cheats write the same RAM address; resolve the conflict first"
                            .into(),
                    );
                }
            }
        }
        let mut entry = entry.clone();
        entry.state = if enabled {
            MgbaCheatState::Enabled
        } else {
            MgbaCheatState::Disabled
        };
        file.entries.push(entry);
    }
    if file.entries.is_empty() {
        return Err("Select at least one supported cheat explicitly".into());
    }
    let filename = request
        .rom
        .with_extension("cheats")
        .file_name()
        .ok_or("ROM name missing")?
        .to_os_string();
    let destination = cheats_root.join(&filename);
    absent(&destination)?;
    let legacy = build_mgba_cheat_apply_plan(
        &destination,
        None,
        &file,
        MgbaCheatIdentity {
            game_title: "Exact locally bound GBA ROM".into(),
            rom_path: request.rom.clone(),
            evidence: MgbaCheatIdentityEvidence::ExactRomSha256 {
                expected: request.expected_rom_sha256.clone(),
                actual: rom_evidence.hash.clone(),
            },
        },
    )
    .map_err(|e| format!("mGBA native rendering refused: {e:?}"))?;
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Mgba,
        selected_archive: request.rom.clone(),
        platform: Some("Game Boy Advance".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::MgbaRomSha256,
            state: PreviewIdentityState::Verified,
            value: Some(rom_evidence.hash.clone()),
            archive_path: request.rom.clone(),
            revision: None,
        },
        destination_root: cheats_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Mgba,
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
        "mgba-native-local-cheats",
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
        output_digest: legacy.output_sha256,
        bytes: legacy.output,
        guard_paths: evidence[..3]
            .iter()
            .map(|e| (e.path.clone(), e.hash.clone()))
            .collect(),
    };
    Ok(MgbaInstallPreview {
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
pub fn mgba_install_attention(history: &Path, rom: &Path) -> Result<Vec<String>, String> {
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
        if journal.context.adapter == PreviewAdapter::Mgba
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
pub fn apply_mgba_install(
    preview: &MgbaInstallPreview,
    roots: &MgbaProfileDiscoveryRoots,
    remembered: Option<&str>,
    options: &SharedApplyOptions,
) -> Result<SharedApplyResult, String> {
    if !mgba_install_attention(&options.history_root, &preview.request.rom)?.is_empty() {
        return Err(
            "Earlier pending mGBA operation requires review; new installation refused".into(),
        );
    }
    let fresh = preview_mgba_install(&preview.request, roots, remembered)?;
    if fresh.profile != preview.profile
        || fresh.plan != preview.plan
        || fresh.directories != preview.directories
    {
        return Err("mGBA profile/preview changed; Preview again".into());
    }
    for old in &preview.evidence {
        let (_, now) = read(&old.path, 64 * 1024 * 1024)?;
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
