//! Native DuckStation per-serial CHT adapter.
//!
//! DuckStation's current upstream contract is represented here: per-serial
//! cheat files contain named sections, metadata, and raw code bodies;
//! per-code enablement is a list in the serial game-settings layer. This
//! adapter never downloads the community database and never promotes a
//! title-only match to an apply target.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    CheatOperation, PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedApplyOptions, SharedApplyResult,
    SharedMaterializedOutput, SharedPreviewRequest, SharedTransactionPlan, build_shared_preview,
    build_shared_transaction_plan, execute_shared_materialized_apply,
};

pub const DUCKSTATION_CHEAT_MAX_BYTES: usize = 512 * 1024;
pub const DUCKSTATION_CHEAT_MAX_LINES: usize = 8_192;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuckStationCheatState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuckStationEnablement {
    Enable,
    Disable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationCheatCode {
    pub raw: String,
    pub normalized: Option<CheatOperation>,
    pub supported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationCheatEntry {
    pub name: String,
    pub metadata: BTreeMap<String, String>,
    pub comments: Vec<String>,
    pub codes: Vec<DuckStationCheatCode>,
    pub state: DuckStationCheatState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuckStationCheatParseIssue {
    EmptySectionName,
    DuplicateName(String),
    MetadataOutsideSection(String),
    MalformedMetadata(String),
    UnsupportedCode(String),
    MissingCode(String),
    LineLimitReached,
    ByteLimitReached,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationCheatFile {
    pub serial: String,
    pub game_hash: Option<String>,
    pub preamble_comments: Vec<String>,
    pub entries: Vec<DuckStationCheatEntry>,
    pub trailing_comments: Vec<String>,
    pub issues: Vec<DuckStationCheatParseIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationIdentity {
    pub serial: String,
    pub verified: bool,
    pub game_hash: Option<String>,
    pub title_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationLoadabilityFacts {
    pub cheat_path: PathBuf,
    pub game_settings_path: PathBuf,
    pub enablement_section: String,
    pub enablement_key: String,
    pub reload_required: bool,
    pub global_enablement_is_not_modified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuckStationCheatParseResult {
    pub file: DuckStationCheatFile,
    pub facts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuckStationCheatApplyPlan {
    pub shared: SharedTransactionPlan,
    pub output_bytes: Vec<u8>,
    pub destination: PathBuf,
    pub identity: DuckStationIdentity,
    pub changed_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuckStationCheatApplyRequest {
    pub profile_root: PathBuf,
    pub staging_path: PathBuf,
    pub selected_game: PathBuf,
    pub identity: DuckStationIdentity,
    pub existing_bytes: Option<Vec<u8>>,
    pub incoming: DuckStationCheatEntry,
}

fn safe_serial(serial: &str) -> bool {
    !serial.is_empty()
        && serial.len() <= 32
        && serial
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn safe_hash(hash: &str) -> bool {
    hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

fn destination_path(root: &Path, identity: &DuckStationIdentity) -> Option<PathBuf> {
    if !safe_serial(&identity.serial)
        || identity
            .game_hash
            .as_deref()
            .is_some_and(|value| !safe_hash(value))
    {
        return None;
    }
    let filename = match &identity.game_hash {
        Some(hash) => format!("{}_{}.cht", identity.serial, hash.to_ascii_uppercase()),
        None => format!("{}.cht", identity.serial),
    };
    Some(root.join("cheats").join(filename))
}

fn direct_code(raw: &str) -> Option<CheatOperation> {
    let fields: Vec<_> = raw.split_whitespace().collect();
    if fields.len() != 2
        || fields[0].len() != 8
        || fields[1].len() != 8
        || !fields
            .iter()
            .all(|field| field.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    let command = u8::from_str_radix(&fields[0][..2], 16).ok()?;
    let address = u64::from_str_radix(&fields[0][2..], 16).ok()? | 0x8000_0000;
    let value = u32::from_str_radix(fields[1], 16).ok()?;
    match command {
        0x30 => Some(CheatOperation::Write8 {
            address,
            value: (value & 0xff) as u8,
        }),
        0x80 => Some(CheatOperation::Write16 {
            address,
            value: (value & 0xffff) as u16,
        }),
        0xa0 => Some(CheatOperation::Write32 { address, value }),
        _ => None,
    }
}

/// Parses bounded DuckStation syntax while retaining comments, metadata, and
/// unsupported body lines rather than dropping them.
pub fn parse_duckstation_cheat_file(
    input: &str,
    serial: impl Into<String>,
    game_hash: Option<String>,
) -> DuckStationCheatParseResult {
    let mut file = DuckStationCheatFile {
        serial: serial.into(),
        game_hash,
        preamble_comments: Vec::new(),
        entries: Vec::new(),
        trailing_comments: Vec::new(),
        issues: Vec::new(),
    };
    if input.len() > DUCKSTATION_CHEAT_MAX_BYTES {
        file.issues
            .push(DuckStationCheatParseIssue::ByteLimitReached);
        return DuckStationCheatParseResult {
            file,
            facts: vec!["input exceeded the bounded DuckStation cheat size".into()],
        };
    }
    let mut current: Option<DuckStationCheatEntry> = None;
    let mut seen = BTreeSet::new();
    let mut line_count = 0;
    for raw_line in input.lines() {
        line_count += 1;
        if line_count > DUCKSTATION_CHEAT_MAX_LINES {
            file.issues
                .push(DuckStationCheatParseIssue::LineLimitReached);
            break;
        }
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.starts_with(';') || trimmed.starts_with('#') || trimmed.is_empty() {
            if let Some(entry) = current.as_mut() {
                entry.comments.push(line.to_string());
            } else {
                file.preamble_comments.push(line.to_string());
            }
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(entry) = current.take() {
                if entry.codes.is_empty() {
                    file.issues
                        .push(DuckStationCheatParseIssue::MissingCode(entry.name.clone()));
                }
                file.entries.push(entry);
            }
            let name = trimmed[1..trimmed.len() - 1].trim().to_string();
            if name.is_empty() {
                file.issues
                    .push(DuckStationCheatParseIssue::EmptySectionName);
                continue;
            }
            if !seen.insert(name.clone()) {
                file.issues
                    .push(DuckStationCheatParseIssue::DuplicateName(name.clone()));
            }
            current = Some(DuckStationCheatEntry {
                name,
                metadata: BTreeMap::new(),
                comments: Vec::new(),
                codes: Vec::new(),
                state: DuckStationCheatState::Unknown,
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            file.issues
                .push(DuckStationCheatParseIssue::MetadataOutsideSection(
                    trimmed.into(),
                ));
            continue;
        };
        if let Some((key, value)) = trimmed.split_once('=') {
            if key.trim().is_empty() {
                file.issues
                    .push(DuckStationCheatParseIssue::MalformedMetadata(
                        trimmed.into(),
                    ));
            } else {
                entry
                    .metadata
                    .insert(key.trim().to_string(), value.trim().to_string());
            }
        } else {
            let normalized = direct_code(trimmed);
            if normalized.is_none() {
                file.issues
                    .push(DuckStationCheatParseIssue::UnsupportedCode(trimmed.into()));
            }
            entry.codes.push(DuckStationCheatCode {
                raw: trimmed.into(),
                normalized: normalized.clone(),
                supported: normalized.is_some(),
            });
        }
    }
    if let Some(entry) = current.take() {
        if entry.codes.is_empty() {
            file.issues
                .push(DuckStationCheatParseIssue::MissingCode(entry.name.clone()));
        }
        file.entries.push(entry);
    }
    let facts = vec![
        "DuckStation per-serial .cht format".into(),
        "per-code enablement is stored in the game-settings Cheats/Enable list".into(),
        "reload required after file/settings changes".into(),
    ];
    DuckStationCheatParseResult { file, facts }
}

fn render_entry(entry: &DuckStationCheatEntry) -> String {
    let mut out = format!("[{}]\n", entry.name);
    for comment in &entry.comments {
        if comment.starts_with(';') || comment.starts_with('#') || comment.is_empty() {
            out.push_str(comment);
            out.push('\n');
        }
    }
    for (key, value) in &entry.metadata {
        out.push_str(key);
        out.push_str(" = ");
        out.push_str(value);
        out.push('\n');
    }
    for code in &entry.codes {
        out.push_str(&code.raw);
        out.push('\n');
    }
    out
}

#[must_use]
pub fn render_duckstation_cheat_file(file: &DuckStationCheatFile) -> Vec<u8> {
    let mut out = String::new();
    for line in &file.preamble_comments {
        out.push_str(line);
        out.push('\n');
    }
    for (index, entry) in file.entries.iter().enumerate() {
        if index > 0 || !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_entry(entry));
    }
    out.into_bytes()
}

/// Adds/replaces one named cheat without duplicating it. Existing unrelated
/// entries and their comments/unknown metadata remain in the output.
pub fn merge_duckstation_cheat(
    file: &mut DuckStationCheatFile,
    incoming: DuckStationCheatEntry,
) -> bool {
    if let Some(existing) = file
        .entries
        .iter_mut()
        .find(|entry| entry.name == incoming.name)
    {
        if *existing == incoming {
            return false;
        }
        *existing = incoming;
        return true;
    }
    file.entries.push(incoming);
    true
}

/// Removes one named cheat while leaving all other sections byte-representable
/// through the structured writer. The caller must still preview/apply the
/// resulting settings through the shared transaction layer.
pub fn remove_duckstation_cheat(file: &mut DuckStationCheatFile, name: &str) -> bool {
    let before = file.entries.len();
    file.entries.retain(|entry| entry.name != name);
    before != file.entries.len()
}

/// Updates DuckStation's per-game `[Cheats] Enable` list without touching the
/// global enablement setting. Unknown settings and comments are preserved.
pub fn update_duckstation_enablement(
    input: &str,
    cheat_name: &str,
    operation: DuckStationEnablement,
) -> Result<Vec<u8>, String> {
    if input.len() > DUCKSTATION_CHEAT_MAX_BYTES {
        return Err("DuckStation settings file exceeds the bounded size".into());
    }
    if cheat_name.trim().is_empty() || cheat_name.contains(['\r', '\n']) {
        return Err("cheat name is not a valid settings value".into());
    }
    let mut output = String::new();
    let mut in_cheats = false;
    let mut saw_enable = false;
    let mut saw_enable_cheats = false;
    let mut inserted = false;
    for raw_line in input.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_cheats && matches!(operation, DuckStationEnablement::Enable) && !inserted {
                output.push_str("Enable = ");
                output.push_str(cheat_name);
                output.push('\n');
                inserted = true;
            }
            in_cheats = trimmed[1..trimmed.len() - 1].trim() == "Cheats";
        }
        if in_cheats && trimmed.starts_with("EnableCheats") {
            saw_enable_cheats = true;
            if matches!(operation, DuckStationEnablement::Enable) {
                output.push_str("EnableCheats = true\n");
                continue;
            }
        }
        if in_cheats && trimmed.starts_with("Enable") && trimmed.contains('=') {
            let value = trimmed
                .split_once('=')
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            if value == cheat_name {
                saw_enable = true;
                if matches!(operation, DuckStationEnablement::Disable) {
                    continue;
                }
                inserted = true;
            }
        }
        output.push_str(line);
        output.push('\n');
    }
    if in_cheats && matches!(operation, DuckStationEnablement::Enable) && !inserted {
        output.push_str("Enable = ");
        output.push_str(cheat_name);
        output.push('\n');
    }
    if matches!(operation, DuckStationEnablement::Enable) && !saw_enable_cheats {
        let marker = "[Cheats]\n";
        if let Some(index) = output.find(marker) {
            let insert_at = index + marker.len();
            output.insert_str(insert_at, "EnableCheats = true\n");
        }
    }
    if matches!(operation, DuckStationEnablement::Disable) && !saw_enable {
        return Ok(input.as_bytes().to_vec());
    }
    Ok(output.into_bytes())
}

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Builds an apply plan from caller-owned staged output. The shared executor
/// performs destination checks, atomic publication, backup, history, and
/// rollback; this adapter supplies identity and merged bytes.
pub fn build_duckstation_cheat_apply_plan(
    request: DuckStationCheatApplyRequest,
) -> Result<DuckStationCheatApplyPlan, String> {
    if !request.identity.verified || !safe_serial(&request.identity.serial) {
        return Err("verified DuckStation serial identity is required".into());
    }
    if !request.profile_root.is_absolute()
        || !request.staging_path.is_absolute()
        || request
            .staging_path
            .strip_prefix(&request.profile_root)
            .is_err()
    {
        return Err("staging path must remain beneath the approved profile root".into());
    }
    let destination = destination_path(&request.profile_root, &request.identity)
        .ok_or_else(|| "unsafe DuckStation serial/hash destination".to_string())?;
    if destination
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("destination traversal refused".into());
    }
    let mut file = request
        .existing_bytes
        .as_deref()
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map_err(|_| "DuckStation cheat file is not UTF-8".to_string())
                .map(|text| {
                    parse_duckstation_cheat_file(
                        text,
                        &request.identity.serial,
                        request.identity.game_hash.clone(),
                    )
                    .file
                })
        })
        .transpose()?
        .unwrap_or_else(|| DuckStationCheatFile {
            serial: request.identity.serial.clone(),
            game_hash: request.identity.game_hash.clone(),
            preamble_comments: Vec::new(),
            entries: Vec::new(),
            trailing_comments: Vec::new(),
            issues: Vec::new(),
        });
    let changed_name = request.incoming.name.clone();
    if !merge_duckstation_cheat(&mut file, request.incoming) {
        return Err("cheat is already present with identical content".into());
    }
    let output_bytes = render_duckstation_cheat_file(&file);
    let relative = destination
        .strip_prefix(&request.profile_root)
        .map_err(|_| "destination escaped profile root".to_string())?
        .to_path_buf();
    let preview = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::DuckStation,
        selected_archive: request.selected_game.clone(),
        platform: Some("ps1".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::DuckStationSerial,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.serial.clone()),
            archive_path: request.selected_game,
            revision: None,
        },
        destination_root: request.profile_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::DuckStation,
            source_path: request.staging_path,
            expected_source_digest: Some(sha256(&output_bytes)),
            destination_relative_paths: vec![relative],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| format!("DuckStation preview refused: {error:?}"))?;
    let shared = build_shared_transaction_plan(
        &preview,
        "duckstation-native-cheats",
        "duckstation_cht",
        &request.profile_root,
    )
    .map_err(|error| format!("DuckStation transaction plan refused: {error:?}"))?;
    Ok(DuckStationCheatApplyPlan {
        shared,
        output_bytes,
        destination,
        identity: request.identity,
        changed_names: vec![changed_name],
    })
}

pub fn apply_duckstation_cheat_plan(
    plan: &DuckStationCheatApplyPlan,
    options: &SharedApplyOptions,
) -> SharedApplyResult {
    let source_path = plan
        .shared
        .entries
        .first()
        .and_then(|entry| entry.source_path.to_path_buf().ok())
        .unwrap_or_default();
    let digest = sha256(&plan.output_bytes);
    execute_shared_materialized_apply(
        &plan.shared,
        options,
        &SharedMaterializedOutput {
            source_path,
            source_digest: digest.clone(),
            output_digest: digest,
            bytes: plan.output_bytes.clone(),
            guard_paths: Vec::new(),
        },
    )
}

#[must_use]
pub fn duckstation_loadability_facts(
    profile_root: &Path,
    identity: &DuckStationIdentity,
) -> Option<DuckStationLoadabilityFacts> {
    if !identity.verified {
        return None;
    }
    Some(DuckStationLoadabilityFacts {
        cheat_path: destination_path(profile_root, identity)?,
        game_settings_path: profile_root
            .join("gamesettings")
            .join(format!("{}.ini", identity.serial)),
        enablement_section: "Cheats".into(),
        enablement_key: "Enable".into(),
        reload_required: true,
        global_enablement_is_not_modified: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, code: &str) -> DuckStationCheatEntry {
        let normalized = direct_code(code);
        DuckStationCheatEntry {
            name: name.into(),
            metadata: BTreeMap::from([
                ("Activation".into(), "EndFrame".into()),
                ("Type".into(), "Gameshark".into()),
            ]),
            comments: vec!["; preserved comment".into()],
            codes: vec![DuckStationCheatCode {
                raw: code.into(),
                normalized,
                supported: direct_code(code).is_some(),
            }],
            state: DuckStationCheatState::Unknown,
        }
    }

    #[test]
    fn parses_realistic_multiple_sections_and_direct_writes() {
        let text = "; header\n[Infinite Health]\nType = Gameshark\nActivation = EndFrame\n30123456 00000063\n\n[Unknown]\nType = Gameshark\nE0123456 00000001\n";
        let result = parse_duckstation_cheat_file(text, "SLUS-12345", None);
        assert_eq!(result.file.entries.len(), 2);
        assert!(result.file.entries[0].codes[0].supported);
        assert!(!result.file.entries[1].codes[0].supported);
    }

    #[test]
    fn malformed_and_duplicate_sections_are_retained_as_issues() {
        let result = parse_duckstation_cheat_file("[Same]\nfoo\n[Same]\n", "SLUS-1", None);
        assert!(result.file.issues.iter().any(|issue| matches!(
            issue,
            DuckStationCheatParseIssue::DuplicateName(_)
                | DuckStationCheatParseIssue::MissingCode(_)
                | DuckStationCheatParseIssue::UnsupportedCode(_)
        )));
        assert_eq!(result.file.entries.len(), 2);
    }

    #[test]
    fn writer_is_deterministic_and_preserves_unrelated_cheats() {
        let mut file = parse_duckstation_cheat_file(
            "[Other]\nAuthor = User\nA0000000 00000001\n",
            "SLUS-1",
            None,
        )
        .file;
        assert!(merge_duckstation_cheat(
            &mut file,
            entry("Infinite Health", "30123456 00000063")
        ));
        let first = render_duckstation_cheat_file(&file);
        assert_eq!(first, render_duckstation_cheat_file(&file));
        assert!(String::from_utf8_lossy(&first).contains("[Other]"));
        assert!(String::from_utf8_lossy(&first).contains("[Infinite Health]"));
    }

    #[test]
    fn identity_and_paths_fail_closed() {
        assert!(
            duckstation_loadability_facts(
                Path::new("/profile"),
                &DuckStationIdentity {
                    serial: "title-only".into(),
                    verified: false,
                    game_hash: None,
                    title_hint: Some("Game".into()),
                }
            )
            .is_none()
        );
        assert!(
            duckstation_loadability_facts(
                Path::new("/profile"),
                &DuckStationIdentity {
                    serial: "SLUS-12345".into(),
                    verified: true,
                    game_hash: None,
                    title_hint: None,
                }
            )
            .unwrap()
            .cheat_path
            .ends_with("cheats/SLUS-12345.cht")
        );
    }

    #[test]
    fn enablement_changes_one_code_and_preserves_other_settings() {
        let settings =
            "[Cheats]\nEnableCheats = false\nEnable = Other\n\n[Display]\nRenderer = Vulkan\n";
        let enabled = update_duckstation_enablement(
            settings,
            "Infinite Health",
            DuckStationEnablement::Enable,
        )
        .unwrap();
        let enabled = String::from_utf8(enabled).unwrap();
        assert!(enabled.contains("EnableCheats = true"));
        assert!(enabled.contains("Enable = Other"));
        assert!(enabled.contains("Enable = Infinite Health"));
        assert!(enabled.contains("Renderer = Vulkan"));

        let disabled = update_duckstation_enablement(
            &enabled,
            "Infinite Health",
            DuckStationEnablement::Disable,
        )
        .unwrap();
        let disabled = String::from_utf8(disabled).unwrap();
        assert!(!disabled.contains("Enable = Infinite Health"));
        assert!(disabled.contains("Enable = Other"));
    }

    #[test]
    fn removing_one_cheat_does_not_remove_unrelated_entries() {
        let mut file = parse_duckstation_cheat_file(
            "[Keep]\n30123456 00000001\n[Remove]\n30123457 00000002\n",
            "SLUS-1",
            None,
        )
        .file;
        assert!(remove_duckstation_cheat(&mut file, "Remove"));
        assert_eq!(
            file.entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["Keep"]
        );
    }
}
