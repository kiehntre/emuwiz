//! Source-verified Flycast v2.7 installation, over the existing parser/planner
//! and shared transactions. See docs/flycast-cheats-compatibility.md.
use super::*;
mod game;
use crate::emulator_inventory::InventoryEmulator;
use crate::emulator_profile_resolver::EmulatorProfileCandidate;
use crate::emulator_profile_resolver::*;
use crate::launch::installation::LaunchInstallation;
pub use game::{FlycastGameEvidence, inspect_flycast_game};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

type Checked<T> = std::result::Result<T, String>;
const APPIMAGE: &str = "5b9f8a636a6acb8446bc78fe96650d8cb8233ac9f8ce69665dab3f39a1cbaae1";
const NATIVE: &str = "141ba9413b41e8578e313e07935ea256d26c21f516de358f07529bc80b2163f1";
const SOURCE_MODE: &str = "flycast-v27-explicit-mapping";

/// Never execute an emulator to identify it. Unknown builds are refused, even
/// when a filename, injected version string or launcher claims version 2.7.
fn artifact(path: &Path) -> Checked<LaunchInstallation> {
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    return Err("Only source-verified Linux x86-64 installations are supported".into());
    if let Some(problem) = crate::emulator_profile_resolver::fsfacts::executable_problem(path) {
        return Err(format!("Unsafe Flycast executable: {problem:?}"));
    }
    let digest = digest(&read_file(path, 32 * 1024 * 1024)?);
    #[cfg(test)]
    if tests::mock_artifact(path, &digest) {
        return Ok(LaunchInstallation::Native);
    }
    match digest.as_str() {
        APPIMAGE => Ok(LaunchInstallation::AppImage { extract_and_run: false }),
        NATIVE => Ok(LaunchInstallation::Native),
        _ => Err("Unsupported or unverified Flycast build: requires the exact official v2.7 x86-64 AppImage or its enclosed native executable".into()),
    }
}

fn effective_root(
    roots: &FlycastProfileDiscoveryRoots,
    executable: &Path,
    installation: &LaunchInstallation,
) -> Checked<PathBuf> {
    // AppImage runtime .config takes precedence over inherited XDG_CONFIG_HOME.
    // .home only changes HOME; inherited XDG_CONFIG_HOME is still authoritative.
    // Discovery roots represent the actual environment, not inferred folders.
    if matches!(installation, LaunchInstallation::AppImage { .. }) {
        for suffix in [".config", ".home"] {
            let mut sibling = executable.as_os_str().to_os_string();
            sibling.push(suffix);
            if fs::symlink_metadata(PathBuf::from(sibling)).is_ok() {
                return Err("AppImage portable .home/.config overrides require a proven Launch profile binding; this layout is not enabled".into());
            }
        }
    }
    let root = roots.xdg_config_home.join("flycast");
    safe_path(&root)?;
    Ok(root)
}

/// A facts-only adapter for main's existing AUTO/PREFERRED/FORCED resolver.
/// Launch remains authoritative too: the two projections must agree.
pub struct FlycastInstallAdapter {
    pub roots: FlycastProfileDiscoveryRoots,
}
impl FlycastInstallAdapter {
    fn build(&self, identity: &CandidateIdentity) -> EmulatorProfileCandidate {
        let root = identity.profile_root.clone();
        let mut unusable = Vec::new();
        let mut evidence = Vec::new();
        let installation = identity
            .executable
            .as_deref()
            .and_then(|p| artifact(p).ok());
        let pairing = match (&installation, &identity.executable) {
            (Some(kind), Some(exe)) => {
                effective_root(&self.roots, exe, kind).is_ok_and(|p| p == root)
            }
            _ => false,
        };
        if !pairing || identity.emulator != InventoryEmulator::Flycast {
            unusable.push(UnusableReason::IncompatiblePairing);
        }
        if installation.is_none() {
            unusable.push(UnusableReason::ExecutableIdentityUnconfirmed);
        }
        let main_config = root.join("emu.cfg");
        if read_file(&main_config, FLYCAST_MAX_CONFIG_BYTES as usize).is_err() {
            unusable.push(UnusableReason::MainConfigUnreadable);
        }
        evidence.push(if pairing {
            SelectionEvidence::supports(
                EvidenceKind::PairingProven,
                "Official v2.7 binary; upstream Linux XDG config rule",
            )
        } else {
            SelectionEvidence::disqualifies(
                EvidenceKind::Unusable,
                "Version or effective Flycast profile is unproven",
            )
        });
        EmulatorProfileCandidate {
            identity: identity.clone(),
            installation: installation.unwrap_or_default(),
            layout: ProfileLayout::DefaultUser,
            main_config,
            folders: BTreeMap::new(),
            readiness: ProfileReadiness {
                profile_directory_present: root.is_dir(),
                main_config_present: root.join("emu.cfg").is_file(),
                setup: SetupState::NotApplicable,
            },
            evidence,
            unusable,
            warnings: vec![],
            details: None,
        }
    }
}
impl EmulatorProfileAdapter for FlycastInstallAdapter {
    fn emulator(&self) -> InventoryEmulator {
        InventoryEmulator::Flycast
    }
    fn discover(&self) -> Vec<EmulatorProfileCandidate> {
        let mut identities = BTreeSet::new();
        for profile in discover_flycast_profiles(&self.roots).profiles {
            // Keep exactly the executable association Launch would use, including
            // its refusal of ambiguous executables and stale profiles.
            if let Ok(binding) = resolve_flycast_native_launch_binding(&profile) {
                identities.insert(CandidateIdentity {
                    emulator: InventoryEmulator::Flycast,
                    executable: Some(binding.executable),
                    profile_root: profile.configuration_path,
                });
            }
        }
        identities.iter().map(|id| self.build(id)).collect()
    }
    fn assess(&self, identity: &CandidateIdentity) -> EmulatorProfileCandidate {
        self.build(identity)
    }
}

#[derive(Clone, Debug)]
pub struct FlycastInstallEnvironment {
    pub roots: FlycastProfileDiscoveryRoots,
    pub selection_path: PathBuf,
    pub launch_profile_id: String,
    pub launch_profile_root: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlycastInstallBinding {
    pub executable: PathBuf,
    pub profile_root: PathBuf,
    pub executable_sha256: String,
    pub config_sha256: String,
    pub selection: EmulatorSelectionMode,
    pub selection_summary: String,
    pub selection_sha256: Option<String>,
}
impl FlycastInstallEnvironment {
    pub fn resolve(&self) -> Checked<FlycastInstallBinding> {
        let selection = load_selection_from(&self.selection_path, InventoryEmulator::Flycast)
            .map_err(|e| e.to_string())?;
        let adapter = FlycastInstallAdapter {
            roots: self.roots.clone(),
        };
        let outcome = resolve_emulator_profile(&adapter, &selection);
        let resolution = outcome.resolved().ok_or_else(|| format!("Flycast AUTO/PREFERRED/FORCED resolution refused: {outcome:?}. Only the official v2.7 native/XDG pairing is enabled; portable and Flatpak bindings are not proven."))?;
        let selected = &resolution.profile;
        if selected.profile_root != self.launch_profile_root
            || self.launch_profile_id != format!("flycast:{}", selected.profile_root.display())
        {
            return Err("Flycast resolver and selected Blue Launch profile disagree; review Launch selection".into());
        }
        let executable = selected.executable.clone();
        // assess() must not manufacture a pinned pair which Launch cannot use.
        let profile = discover_flycast_profiles(&self.roots)
            .profiles
            .into_iter()
            .find(|p| p.profile_id == self.launch_profile_id)
            .ok_or("Selected Launch profile disappeared")?;
        let binding =
            resolve_flycast_native_launch_binding(&profile).map_err(|e| format!("{e:?}"))?;
        if binding.executable != executable {
            return Err("Selected Launch executable changed or is ambiguous".into());
        }
        ensure_stopped(&executable)?;
        Ok(FlycastInstallBinding {
            executable_sha256: digest(&read_file(&executable, 32 * 1024 * 1024)?),
            executable,
            profile_root: selected.profile_root.clone(),
            config_sha256: digest(&read_file(
                &selected.main_config,
                FLYCAST_MAX_CONFIG_BYTES as usize,
            )?),
            selection,
            selection_sha256: match fs::symlink_metadata(&self.selection_path) {
                Ok(_) => Some(digest(&read_file(&self.selection_path, 256 * 1024)?)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.to_string()),
            },
            selection_summary: format!(
                "{:?}: {:?}; warnings: {:?}",
                selected.mode, selected.reason, selected.warnings
            ),
        })
    }
}

/// Preview retains its private staging directory; no installed files are changed.
#[derive(Debug)]
pub struct FlycastInstallPreview {
    pub plan: SharedTransactionPlan,
    pub report: SharedPreviewReport,
    pub binding: FlycastInstallBinding,
    pub changes: Vec<(PathBuf, Vec<u8>)>,
    pub product: String,
    pub game_evidence: FlycastGameEvidence,
    pub source: PathBuf,
    source_sha256: String,
    _staging: tempfile::TempDir,
}

pub fn preview_flycast_install(
    env: &FlycastInstallEnvironment,
    game: &Path,
    product: &str,
    source: &Path,
    selected: &[(usize, bool)],
) -> Checked<FlycastInstallPreview> {
    let binding = env.resolve()?;
    let game_evidence = inspect_flycast_game(game)?;
    if game_evidence.product != product {
        return Err("Flycast exact game ID disagrees with verified catalogue identity".into());
    }
    if binding.selection_sha256.is_none() {
        return Err("Choose and save AUTO, PREFERRED or FORCED for Flycast before Preview, so the selection policy can be guarded at write time".into());
    }
    let original = read_file(source, FLYCAST_CHEAT_MAX_BYTES - 1)?;
    let parsed = validate_native_file(&original)?;
    if selected.is_empty() {
        return Err("Select at least one supported native entry".into());
    }
    let mut unique = BTreeSet::new();
    let mut entries = Vec::new();
    for &(index, enabled) in selected {
        if !unique.insert(index) {
            return Err("Duplicate selected cheat".into());
        }
        let mut entry = parsed
            .entries
            .get(index)
            .cloned()
            .ok_or("Selected cheat disappeared")?;
        entry.enabled = enabled;
        entry.fields.insert("enable".into(), enabled.to_string());
        entries.push(entry);
    }
    let destination = binding.profile_root.join(format!("0-emuwiz-{product}.cht"));
    // The legacy planner still owns merge/render and identity/path validation.
    let existing = match fs::symlink_metadata(&destination) {
        Ok(_) => Some(read_file(&destination, FLYCAST_CHEAT_MAX_BYTES - 1)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let mut all = if let Some(bytes) = &existing {
        validate_native_file(bytes)?.entries
    } else {
        vec![]
    };
    all.extend(entries.clone());
    reject_overlaps(&all)?;
    let config = binding.profile_root.join("emu.cfg");
    let old_config = read_file(&config, FLYCAST_MAX_CONFIG_BYTES as usize)?;
    let new_config = mapped_config(&old_config, product, &destination)?;
    if existing.is_some() && new_config != old_config {
        return Err("An existing unmapped destination is a user-file conflict. Preserve it intact; explicitly associate it in Flycast before merging. This install lane permits only one replacement backup per transaction".into());
    }
    let staging = tempfile::Builder::new()
        .prefix("emuwiz-flycast-preview-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    // Adapt the legacy planner's filename-only assumption in private scratch
    // space. The consumed destination is explicitly bound by emu.cfg below.
    let merge_root = staging.path().join("merge");
    fs::create_dir(&merge_root).map_err(|e| e.to_string())?;
    let merge_destination = merge_root.join(format!("{product}.cht"));
    if let Some(bytes) = &existing {
        fs::write(&merge_destination, bytes).map_err(|e| e.to_string())?;
    }
    let legacy = build_flycast_cheat_preview(&FlycastCheatRequest {
        profile_id: env.launch_profile_id.clone(),
        cheats_root: merge_root,
        staging_root: staging.path().into(),
        identity: DreamcastCheatIdentity {
            verified_product_code: product.into(),
            disc_number: 1,
            source_digest: Some(digest(&original)),
        },
        destination: merge_destination,
        expected_destination_sha256: existing.as_ref().map(|b| digest(b)),
        existing_or_imported: entries,
        shared_across_discs: true,
    })
    .map_err(|e| e.to_string())?;
    if !legacy.conflicts.is_empty() {
        return Err(format!("Existing cheat conflicts: {:?}", legacy.conflicts));
    }
    let rendered = read_file(
        &staging.path().join("flycast-cheats.cht"),
        FLYCAST_CHEAT_MAX_BYTES - 1,
    )?;
    validate_native_file(&rendered)?;
    let mut changes = vec![(destination, rendered)];
    if new_config != old_config {
        changes.push((config, new_config));
    }
    let mut sources = Vec::new();
    for (i, (destination, bytes)) in changes.iter().enumerate() {
        let staged = staging.path().join(format!("output-{i}"));
        fs::write(&staged, bytes).map_err(|e| e.to_string())?;
        sources.push(PreviewSourceItem {
            adapter: PreviewAdapter::Flycast,
            source_path: staged,
            expected_source_digest: Some(digest(bytes)),
            destination_relative_paths: vec![
                destination
                    .strip_prefix(&binding.profile_root)
                    .map_err(|e| e.to_string())?
                    .into(),
            ],
            match_strength: PreviewMatchStrength::VerifiedExact,
        });
    }
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Flycast,
        selected_archive: game.into(),
        platform: Some("Dreamcast".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::DreamcastProductCode,
            state: PreviewIdentityState::Verified,
            value: Some(product.into()),
            archive_path: game.into(),
            revision: None,
        },
        destination_root: binding.profile_root.clone(),
        source_items: sources,
    })
    .map_err(|e| format!("{e:?}"))?;
    let plan =
        build_shared_transaction_plan(&report, &env.launch_profile_id, SOURCE_MODE, staging.path())
            .map_err(|e| format!("{e:?}"))?;
    if plan.entries.len() != changes.len() {
        return Err(format!(
            "Installation preview contains blocked or conflicting files: {:?}",
            report.entries
        ));
    }
    if changes.len() > 1
        && plan
            .entries
            .last()
            .and_then(|e| e.destination_relative_path.to_path_buf().ok())
            .as_deref()
            != Some(Path::new("emu.cfg"))
    {
        return Err("Configuration dependency ordering could not be proven".into());
    }
    // Refuse any source/profile/config change while assembling the preview.
    if env.resolve()? != binding {
        return Err("Flycast profile changed during Preview".into());
    }
    Ok(FlycastInstallPreview {
        plan,
        report,
        binding,
        changes,
        product: product.into(),
        game_evidence,
        source: source.into(),
        source_sha256: digest(&original),
        _staging: staging,
    })
}

pub fn apply_flycast_install(
    env: &FlycastInstallEnvironment,
    preview: &FlycastInstallPreview,
    game: &Path,
    fresh_product: &str,
    options: &FlycastCheatApplyOptions,
) -> Checked<SharedApplyResult> {
    refuse_unfinished(&options.history_root, game)?;
    if inspect_flycast_game(game)? != preview.game_evidence
        || env.resolve()? != preview.binding
        || fresh_product != preview.product
        || preview
            .plan
            .context
            .selected_archive
            .to_path_buf()
            .map_err(|e| format!("{e:?}"))?
            != game
        || digest(&read_file(&preview.source, FLYCAST_CHEAT_MAX_BYTES - 1)?)
            != preview.source_sha256
    {
        return Err("Preview is stale: game, source, selected installation, policy or configuration changed; Preview again".into());
    }
    let context = SharedApplyContext {
        adapter: PreviewAdapter::Flycast,
        selected_archive: SharedTransactionPath::from_path(game),
        verified_game_identity: fresh_product.into(),
        profile_id: env.launch_profile_id.clone(),
        source_mode: SOURCE_MODE.into(),
    };
    // A new configuration mapping must only become visible after its exact
    // approved cheat bytes exist. The managed filename sorts before emu.cfg.
    // A mapped-file merge retains the same guards on its single write.
    let first = if preview.changes.len() > 1 {
        preview
            .plan
            .entries
            .iter()
            .find(|entry| {
                entry
                    .destination_relative_path
                    .to_path_buf()
                    .ok()
                    .as_deref()
                    == Some(Path::new("emu.cfg"))
            })
            .ok_or("Configuration entry missing")?
    } else {
        preview
            .plan
            .entries
            .first()
            .ok_or("Empty installation plan")?
    };
    let relative = first
        .destination_relative_path
        .to_path_buf()
        .map_err(|e| format!("{e:?}"))?;
    let destination = preview.binding.profile_root.join(relative);
    let bytes = preview
        .changes
        .iter()
        .find(|(p, _)| p == &destination)
        .ok_or("Preview output missing")?
        .1
        .clone();
    let mut guards = vec![
        (
            env.selection_path.clone(),
            preview
                .binding
                .selection_sha256
                .clone()
                .ok_or("Saved selection policy missing")?,
        ),
        (
            preview.binding.executable.clone(),
            preview.binding.executable_sha256.clone(),
        ),
        (preview.source.clone(), preview.source_sha256.clone()),
        (
            preview.binding.profile_root.join("emu.cfg"),
            preview.binding.config_sha256.clone(),
        ),
    ];
    if preview.changes.len() > 1 {
        let (cheat, bytes) = preview
            .changes
            .iter()
            .find(|(p, _)| p.file_name() != Some(std::ffi::OsStr::new("emu.cfg")))
            .ok_or("Cheat dependency missing")?;
        guards.push((cheat.clone(), digest(bytes)));
    }
    let output = SharedMaterializedOutput {
        source_path: first
            .source_path
            .to_path_buf()
            .map_err(|e| format!("{e:?}"))?,
        source_digest: first.source_digest.clone(),
        output_digest: digest(&bytes),
        bytes,
        guard_paths: guards,
    };
    Ok(execute_shared_materialized_apply(
        &preview.plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: preview.plan.plan_id.clone(),
                general_approved: options.general_approved,
                replacement_approved: options.replacement_approved,
            }),
            operation_id: options.operation_id.clone(),
            timestamp_unix_seconds: options.timestamp_unix_seconds,
            current_context: context,
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
        &output,
    ))
}

pub fn refuse_unfinished(history: &Path, _game: &Path) -> Checked<()> {
    if discover_pending_operations(history)
        .iter()
        .any(|p| p.journal.context.adapter == PreviewAdapter::Flycast)
    {
        return Err("An interrupted Flycast operation needs recovery review in History before another install".into());
    }
    let report = discover_shared_apply_history(history);
    if !report.complete || !report.warnings.is_empty() {
        return Err(
            "History is incomplete; inspect retained recovery evidence before installing".into(),
        );
    }
    if report.journals.iter().any(|(journal_path, j)| {
        j.context.adapter == PreviewAdapter::Flycast
            && !matches!(
                j.status,
                SharedApplyStatus::Success | SharedApplyStatus::DryRun
            )
            && !proven_before_write_refusal(j)
            && !recovered(history, journal_path, j)
    }) {
        return Err("An incomplete Flycast transaction requires History/recovery review before another installation".into());
    }
    Ok(())
}

// A completed, explicit pre-write refusal is different from an interrupted
// or partially applied operation. Permit a fresh Preview after these outcomes;
// its hashes/approvals are rebuilt. Uncertain writes still require recovery.
fn proven_before_write_refusal(journal: &SharedApplyJournal) -> bool {
    journal.status == SharedApplyStatus::Failed
        && journal.created_root_directories.is_empty()
        && journal.entries.iter().all(|entry| {
            entry.backup_path.is_none()
                && entry.temporary_path.is_none()
                && entry.final_destination_digest.is_none()
                && entry.created_directories.is_empty()
                && !entry.verification_succeeded
                && matches!(
                    entry.outcome,
                    SharedApplyOutcome::SkippedNotEligible
                        | SharedApplyOutcome::SkippedConflict
                        | SharedApplyOutcome::SkippedReplacementNotApproved
                        | SharedApplyOutcome::SourceChanged
                        | SharedApplyOutcome::DestinationChanged
                )
        })
}

fn recovered(
    history: &Path,
    journal_path: &SharedTransactionPath,
    journal: &SharedApplyJournal,
) -> bool {
    if journal.operation_id.is_empty()
        || !journal
            .operation_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return false;
    }
    let marker = history.join(format!("{}.rollback.json", journal.operation_id));
    let Ok(bytes) = read_file(&marker, 2 * 1024 * 1024) else {
        return false;
    };
    let Ok(receipt) = serde_json::from_slice::<SharedRollbackPreview>(&bytes) else {
        return false;
    };
    receipt.original_operation_id == journal.operation_id
        && receipt.journal_path == *journal_path
        && receipt.destination_root == journal.destination_root
        && receipt.entries.len() == journal.entries.len()
        && receipt.entries.iter().all(|e| {
            matches!(
                e.outcome,
                SharedRollbackOutcome::RemovedInstalledFile
                    | SharedRollbackOutcome::RestoredBackup
                    | SharedRollbackOutcome::NoChangeRequired
            )
        })
}

/// Deliberately narrower than Flycast's full native language: no conversion,
/// repeats, masked writes, conditions or copy operations. Existing unsupported
/// files are refused intact rather than truncated or silently reinterpreted.
pub fn validate_native_file(bytes: &[u8]) -> Checked<FlycastCheatFile> {
    if bytes.len() >= FLYCAST_CHEAT_MAX_BYTES {
        return Err("Flycast requires files smaller than 1 MiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut counts = 0;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') || t.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key.eq_ignore_ascii_case("cheats") {
                counts += 1;
                if key != "cheats" {
                    return Err("Native keys are case-sensitive".into());
                }
            }
            if key != "cheats" {
                let (prefix, suffix) = key.split_once('_').ok_or("Unsupported native key")?;
                let index = prefix
                    .strip_prefix("cheat")
                    .and_then(|v| v.parse::<usize>().ok())
                    .ok_or("Unsupported native entry key")?;
                if suffix.is_empty() || prefix != format!("cheat{index}") {
                    return Err(
                        "Noncanonical native cheat index would change meaning when rendered".into(),
                    );
                }
            }
            let value = value.strip_prefix(' ').unwrap_or(value);
            if value != value.trim() || value.starts_with('"') {
                return Err(
                    "Unsupported native quoting or value whitespace; no user file was changed"
                        .into(),
                );
            }
        }
    }
    if counts != 1 {
        return Err("Exactly one explicit native cheat count is required".into());
    }
    let file = parse_flycast_cheat_file(bytes).map_err(|e| e.to_string())?;
    if !file.warnings.is_empty()
        || file.entries.is_empty()
        || file.declared_count != Some(file.entries.len())
        || file.entries.iter().enumerate().any(|(i, e)| i != e.index)
    {
        return Err(
            "Unsupported native file: requires a complete dense count and no parser diagnostics"
                .into(),
        );
    }
    // The existing parser preserves unrecognised lines; do not allow them to
    // change section/key interpretation when Flycast reads the rendered file.
    if file.preserved_lines.iter().any(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with(';') && !t.starts_with('#')
    }) {
        return Err("Unsupported native sections or unrecognised top-level fields".into());
    }
    for entry in &file.entries {
        let DreamcastCheatCode::DirectWrite {
            address,
            width,
            value,
        } = entry.code
        else {
            return Err("Only native set-value entries are supported".into());
        };
        if address
            .checked_add(width)
            .is_none_or(|end| end > 16 * 1024 * 1024)
            || !matches!(width, 1 | 2 | 4)
            || (width < 4 && value >= 1u32 << (width * 8))
        {
            return Err("Cheat address, width or value exceeds Dreamcast RAM limits".into());
        }
        for (key, val) in &entry.fields {
            let valid = match key.as_str() {
                "desc" => !val.contains(['\r', '\n', '\0', '"']) && !val.starts_with(' '),
                "big_endian" => val == "false",
                "handler" => val == "1",
                "code" => val.is_empty(),
                "enable" => matches!(val.as_str(), "true" | "false"),
                "address" | "value" | "memory_search_size" | "cheat_type" => number(val).is_some(),
                "repeat_count" => number(val) == Some(1),
                "repeat_add_to_value"
                | "repeat_add_to_address"
                | "dest_address"
                | "address_bit_position" => number(val) == Some(0),
                _ => false,
            };
            if !valid {
                return Err(format!("Unsupported or malformed native field {key}"));
            }
        }
        if !entry.fields.contains_key("enable")
            || !entry.fields.contains_key("value")
            || !entry.fields.contains_key("desc")
        {
            return Err("Explicit native description, value and enabled state required".into());
        }
    }
    reject_overlaps(&file.entries)?;
    Ok(file)
}
fn number(value: &str) -> Option<u32> {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).ok()
    } else {
        value.parse().ok()
    }
}
fn reject_overlaps(entries: &[FlycastCheatEntry]) -> Checked<()> {
    let mut ranges = Vec::new();
    for e in entries {
        if let DreamcastCheatCode::DirectWrite { address, width, .. } = e.code {
            let end = address.checked_add(width).ok_or("Cheat address overflow")?;
            if ranges
                .iter()
                .any(|&(start, stop)| address < stop && start < end)
            {
                return Err(
                    "Conflicting or duplicate cheat addresses; existing user cheats were preserved"
                        .into(),
                );
            }
            ranges.push((address, end));
        }
    }
    Ok(())
}

fn mapped_config(bytes: &[u8], product: &str, destination: &Path) -> Checked<Vec<u8>> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    if text.contains(['\r', '\0'])
        || product.trim() != product
        || product.is_empty()
        || !product
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b' '))
    {
        return Err("Unsupported config encoding or non-exact Dreamcast product identity".into());
    }
    let path = destination
        .to_str()
        .ok_or("Non-UTF8 cheat destination is unsupported")?;
    if path.contains(['\n', '\r', '\0', '"', '\\']) {
        return Err("Destination cannot be represented safely in Flycast configuration".into());
    }
    let mut section = "";
    let mut sections = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut insert = None;
    let mut offset = 0;
    let mut found = false;
    for raw in text.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let t = line.trim_start();
        if t.is_empty() || t.starts_with(';') {
            offset += raw.len();
            continue;
        }
        if t.starts_with('[') {
            if section == "cheats" {
                insert = Some(offset);
            }
            if !t.ends_with(']') {
                return Err("Malformed Flycast config section".into());
            }
            section = &t[1..t.len() - 1];
            if section.trim() != section || !sections.insert(section) {
                return Err("Duplicate or uncertain Flycast config section".into());
            }
            keys.clear();
        } else {
            let (key, value) = line
                .split_once('=')
                .ok_or("Malformed Flycast configuration entry")?;
            let key = key.trim_matches([' ', '\t']);
            if key.is_empty() || !keys.insert(key) {
                return Err("Duplicate or empty Flycast configuration key".into());
            }
            // Match upstream's single readability space and optional quotes,
            // without inventing trimming/normalisation of the persisted path.
            let value = value.strip_prefix(' ').unwrap_or(value);
            let value = if value.starts_with('"') {
                value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .ok_or("Malformed quoted config value")?
            } else {
                value
            };
            if section == "cheats" && key == product {
                if value != path {
                    return Err("Existing game-to-cheat mapping targets another user file; leave it intact and resolve the conflict in Flycast".into());
                }
                found = true;
            }
        }
        offset += raw.len();
    }
    if found {
        return Ok(bytes.to_vec());
    }
    if section == "cheats" {
        insert = Some(text.len());
    }
    let mapping = format!("{product} = \"{path}\"\n");
    let mut out = text.to_string();
    if let Some(at) = insert {
        let prefix = if at > 0 && !text[..at].ends_with('\n') {
            "\n"
        } else {
            ""
        };
        out.insert_str(at, &format!("{prefix}{mapping}"));
    } else {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("\n[cheats]\n");
        out.push_str(&mapping);
    }
    if out.len() > FLYCAST_MAX_CONFIG_BYTES as usize {
        return Err("Flycast config size limit reached".into());
    }
    Ok(out.into_bytes())
}

fn safe_path(path: &Path) -> Checked<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("Absolute, non-traversing paths required".into());
    }
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("Symlink paths are unsupported for Flycast installation".into());
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
fn read_file(path: &Path, maximum: usize) -> Checked<Vec<u8>> {
    safe_path(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Regular file required".into());
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > maximum {
        return Err("File exceeds supported bounded read".into());
    }
    Ok(bytes)
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn ensure_stopped(executable: &Path) -> Checked<()> {
    ensure_stopped_at(executable, Path::new("/proc"))
}
fn ensure_stopped_at(executable: &Path, proc_root: &Path) -> Checked<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let own = fs::metadata(executable).map_err(|e| e.to_string())?;
        for (i, process) in fs::read_dir(proc_root)
            .map_err(|e| e.to_string())?
            .enumerate()
        {
            if i >= 65536 {
                return Err(
                    "Process inspection limit reached; close Flycast before changing its profile"
                        .into(),
                );
            }
            let process = process.map_err(|e| e.to_string())?;
            if !process
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
            {
                continue;
            }
            let exe = process.path().join("exe");
            let same =
                fs::metadata(&exe).is_ok_and(|m| m.dev() == own.dev() && m.ino() == own.ino());
            let named = fs::read_link(&exe).is_ok_and(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().contains("flycast"))
            });
            // A differently named copy of the pinned native ELF also owns the
            // same configuration. Read only its executable, never environment.
            let known = if !same && !named {
                if let Ok(f) = std::fs::File::open(&exe) {
                    if f.metadata()
                        .is_ok_and(|m| m.is_file() && matches!(m.len(), 20396536 | 32289088))
                    {
                        let mut bytes = Vec::new();
                        f.take(32 * 1024 * 1024 + 1)
                            .read_to_end(&mut bytes)
                            .map_err(|e| e.to_string())?;
                        matches!(digest(&bytes).as_str(), NATIVE | APPIMAGE)
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if same || named || known {
                return Err("Close all Flycast instances before Preview, Install or Undo; they can share the selected profile".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
