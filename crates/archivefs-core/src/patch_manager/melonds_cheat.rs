//! Native melonDS Action Replay MCH adapter.
//!
//! The format is the text format implemented by melonDS's ARCodeFile:
//! ROOT, CAT, CODE, DESC, and pairs of eight-hex-digit words. This module
//! handles local user-owned MCH files only; it never acquires usrcheat.dat.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    CheatOperation, DsActionReplayClassification, PreviewAdapter, PreviewIdentity,
    PreviewIdentityKind, PreviewIdentityState, PreviewMatchStrength, PreviewSourceItem,
    SharedApplyOptions, SharedApplyResult, SharedMaterializedOutput, SharedPreviewRequest,
    SharedTransactionPlan, build_shared_preview, build_shared_transaction_plan,
    ds_action_replay_line_to_ir, execute_shared_materialized_apply,
};

pub const MELONDS_CHEAT_MAX_BYTES: usize = 512 * 1024;
pub const MELONDS_CHEAT_MAX_LINES: usize = 16_384;
pub const MELONDS_CHEAT_MAX_CODES: usize = 2_048;
pub const MELONDS_CHEAT_MAX_WORDS_PER_CODE: usize = 4_096;
pub const MELONDS_CHEAT_MAX_STRING_BYTES: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MelonDsCheatFormat {
    MchText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MelonDsCheatState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MelonDsCheatCode {
    pub words: Vec<(u32, u32)>,
    pub normalized: Vec<CheatOperation>,
    pub opaque_word_pairs: usize,
    pub comments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MelonDsCheatEntry {
    pub name: String,
    pub description: Option<String>,
    pub state: MelonDsCheatState,
    pub code: MelonDsCheatCode,
    pub comments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MelonDsCheatCategory {
    pub name: String,
    pub description: Option<String>,
    pub only_one_code_enabled: bool,
    pub entries: Vec<MelonDsCheatEntry>,
    pub comments: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MelonDsCheatItem {
    Category(MelonDsCheatCategory),
    RootCode(MelonDsCheatEntry),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MelonDsCheatParseIssue {
    ByteLimitReached,
    LineLimitReached,
    StringLimitReached,
    TooManyCodes,
    TooManyWords,
    MalformedDirective(String),
    DirectiveOutsideScope(String),
    DescriptionOutsideScope,
    DataOutsideCode,
    EmptyName,
    DuplicateName(String),
    MissingCode(String),
    InvalidWordPair(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MelonDsCheatFile {
    pub format: MelonDsCheatFormat,
    pub items: Vec<MelonDsCheatItem>,
    pub comments: Vec<String>,
    pub issues: Vec<MelonDsCheatParseIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MelonDsRomIdentity {
    pub rom_path: PathBuf,
    pub rom_sha256: Option<String>,
    pub game_code: Option<String>,
    pub verified: bool,
    pub title_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MelonDsLoadabilityFacts {
    pub cheat_path: PathBuf,
    pub target_game_identifier: String,
    pub native_format: MelonDsCheatFormat,
    pub global_cheats_key: String,
    pub global_cheat_setting_modified: bool,
    pub reload_required: bool,
    pub understood_operations_are_partial: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MelonDsUsrCheatStatus {
    UserImportOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MelonDsCheatApplyRequest {
    pub staging_path: PathBuf,
    pub selected_rom: PathBuf,
    pub destination_root: PathBuf,
    pub identity: MelonDsRomIdentity,
    pub existing_bytes: Option<Vec<u8>>,
    pub incoming: MelonDsCheatEntry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MelonDsCheatApplyPlan {
    pub shared: SharedTransactionPlan,
    pub output_bytes: Vec<u8>,
    pub destination: PathBuf,
    pub identity: MelonDsRomIdentity,
    pub changed_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CurrentCode {
    Category(usize, usize),
    Root(usize),
}

fn bounded_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MELONDS_CHEAT_MAX_STRING_BYTES
}

fn safe_sha256(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn safe_game_code(value: &str) -> bool {
    value.len() == 4 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

fn destination_path(rom_path: &Path, destination_root: Option<&Path>) -> Option<PathBuf> {
    if !rom_path.is_absolute() || rom_path.file_stem().is_none() {
        return None;
    }
    let root = destination_root
        .map(Path::to_path_buf)
        .or_else(|| rom_path.parent().map(Path::to_path_buf))?;
    if !root.is_absolute() {
        return None;
    }
    let stem = rom_path.file_stem()?.to_string_lossy();
    if !bounded_name(&stem) || stem == "." || stem == ".." {
        return None;
    }
    Some(root.join(format!("{stem}.mch")))
}

fn empty_file() -> MelonDsCheatFile {
    MelonDsCheatFile {
        format: MelonDsCheatFormat::MchText,
        items: Vec::new(),
        comments: Vec::new(),
        issues: Vec::new(),
    }
}

fn finish_code(file: &mut MelonDsCheatFile, current: Option<CurrentCode>) {
    let Some(current) = current else { return };
    let (name, words) = match current {
        CurrentCode::Category(category, code) => match &file.items[category] {
            MelonDsCheatItem::Category(value) => (
                value.entries[code].name.clone(),
                value.entries[code].code.words.len(),
            ),
            MelonDsCheatItem::RootCode(_) => return,
        },
        CurrentCode::Root(code) => match &file.items[code] {
            MelonDsCheatItem::RootCode(value) => (value.name.clone(), value.code.words.len()),
            MelonDsCheatItem::Category(_) => return,
        },
    };
    if words == 0 {
        file.issues.push(MelonDsCheatParseIssue::MissingCode(name));
    }
}

fn current_entry_mut(
    file: &mut MelonDsCheatFile,
    current: CurrentCode,
) -> Option<&mut MelonDsCheatEntry> {
    match current {
        CurrentCode::Category(category, code) => match file.items.get_mut(category)? {
            MelonDsCheatItem::Category(value) => value.entries.get_mut(code),
            MelonDsCheatItem::RootCode(_) => None,
        },
        CurrentCode::Root(code) => match file.items.get_mut(code)? {
            MelonDsCheatItem::RootCode(value) => Some(value),
            MelonDsCheatItem::Category(_) => None,
        },
    }
}

fn current_category_mut(
    file: &mut MelonDsCheatFile,
    category: usize,
) -> Option<&mut MelonDsCheatCategory> {
    match file.items.get_mut(category)? {
        MelonDsCheatItem::Category(value) => Some(value),
        MelonDsCheatItem::RootCode(_) => None,
    }
}

pub fn parse_melonds_cheat_file(input: &str) -> MelonDsCheatFile {
    let mut file = empty_file();
    if input.len() > MELONDS_CHEAT_MAX_BYTES {
        file.issues.push(MelonDsCheatParseIssue::ByteLimitReached);
        return file;
    }
    let mut current_category = None;
    let mut current_code = None;
    let mut pending_comments = Vec::new();
    let mut names = BTreeSet::new();
    for (line_number, raw) in input.lines().enumerate() {
        if line_number >= MELONDS_CHEAT_MAX_LINES {
            file.issues.push(MelonDsCheatParseIssue::LineLimitReached);
            break;
        }
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if trimmed.starts_with('#') {
                pending_comments.push(line.to_string());
            }
            continue;
        }
        if trimmed.len() > MELONDS_CHEAT_MAX_STRING_BYTES + 32 {
            file.issues.push(MelonDsCheatParseIssue::StringLimitReached);
            continue;
        }
        if trimmed.eq_ignore_ascii_case("ROOT") {
            finish_code(&mut file, current_code.take());
            current_category = None;
            continue;
        }
        if trimmed
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("CAT"))
        {
            finish_code(&mut file, current_code.take());
            let rest = trimmed[3..].trim_start();
            let (only_one, name) = if let Some(rest) = rest.strip_prefix('0') {
                (false, rest.trim_start())
            } else if let Some(rest) = rest.strip_prefix('1') {
                (true, rest.trim_start())
            } else {
                (false, rest)
            };
            if !bounded_name(name) {
                file.issues.push(MelonDsCheatParseIssue::EmptyName);
                continue;
            }
            if !names.insert(name.to_string()) {
                file.issues
                    .push(MelonDsCheatParseIssue::DuplicateName(name.to_string()));
            }
            let index = file.items.len();
            file.items
                .push(MelonDsCheatItem::Category(MelonDsCheatCategory {
                    name: name.to_string(),
                    description: None,
                    only_one_code_enabled: only_one,
                    entries: Vec::new(),
                    comments: std::mem::take(&mut pending_comments),
                }));
            current_category = Some(index);
            continue;
        }
        if trimmed
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("CODE"))
        {
            finish_code(&mut file, current_code.take());
            let rest = trimmed[4..].trim_start();
            let (enabled, name) = if let Some(rest) = rest.strip_prefix('0') {
                (false, rest.trim_start())
            } else if let Some(rest) = rest.strip_prefix('1') {
                (true, rest.trim_start())
            } else {
                file.issues
                    .push(MelonDsCheatParseIssue::MalformedDirective(line.into()));
                continue;
            };
            if !bounded_name(name) {
                file.issues.push(MelonDsCheatParseIssue::EmptyName);
                continue;
            }
            if !names.insert(name.to_string()) {
                file.issues
                    .push(MelonDsCheatParseIssue::DuplicateName(name.to_string()));
            }
            let entry = MelonDsCheatEntry {
                name: name.to_string(),
                description: None,
                state: if enabled {
                    MelonDsCheatState::Enabled
                } else {
                    MelonDsCheatState::Disabled
                },
                code: MelonDsCheatCode {
                    words: Vec::new(),
                    normalized: Vec::new(),
                    opaque_word_pairs: 0,
                    comments: Vec::new(),
                },
                comments: std::mem::take(&mut pending_comments),
            };
            if let Some(category) = current_category {
                let Some(value) = current_category_mut(&mut file, category) else {
                    file.issues
                        .push(MelonDsCheatParseIssue::DirectiveOutsideScope(line.into()));
                    continue;
                };
                if value.entries.len() >= MELONDS_CHEAT_MAX_CODES {
                    file.issues.push(MelonDsCheatParseIssue::TooManyCodes);
                    continue;
                }
                let code = value.entries.len();
                value.entries.push(entry);
                current_code = Some(CurrentCode::Category(category, code));
            } else {
                if file.items.len() >= MELONDS_CHEAT_MAX_CODES {
                    file.issues.push(MelonDsCheatParseIssue::TooManyCodes);
                    continue;
                }
                let code = file.items.len();
                file.items.push(MelonDsCheatItem::RootCode(entry));
                current_code = Some(CurrentCode::Root(code));
            }
            continue;
        }
        if trimmed
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("DESC"))
        {
            let description = trimmed[4..].trim_start();
            if !bounded_name(description) {
                file.issues.push(MelonDsCheatParseIssue::StringLimitReached);
                continue;
            }
            if let Some(current) = current_code {
                if let Some(entry) = current_entry_mut(&mut file, current) {
                    entry.description = Some(description.to_string());
                }
            } else if let Some(category) = current_category {
                if let Some(value) = current_category_mut(&mut file, category) {
                    value.description = Some(description.to_string());
                }
            } else {
                file.issues
                    .push(MelonDsCheatParseIssue::DescriptionOutsideScope);
            }
            continue;
        }
        let words = trimmed.split_whitespace().collect::<Vec<_>>();
        if words.len() != 2
            || words[0].len() != 8
            || words[1].len() != 8
            || !words
                .iter()
                .all(|word| word.chars().all(|c| c.is_ascii_hexdigit()))
        {
            file.issues
                .push(MelonDsCheatParseIssue::InvalidWordPair(line.into()));
            continue;
        }
        let Some(current) = current_code else {
            file.issues.push(MelonDsCheatParseIssue::DataOutsideCode);
            continue;
        };
        let first = u32::from_str_radix(words[0], 16).unwrap_or_default();
        let second = u32::from_str_radix(words[1], 16).unwrap_or_default();
        let Some(entry) = current_entry_mut(&mut file, current) else {
            file.issues.push(MelonDsCheatParseIssue::DataOutsideCode);
            continue;
        };
        if entry.code.words.len() >= MELONDS_CHEAT_MAX_WORDS_PER_CODE {
            file.issues.push(MelonDsCheatParseIssue::TooManyWords);
            continue;
        }
        entry.code.words.push((first, second));
        match ds_action_replay_line_to_ir(trimmed) {
            DsActionReplayClassification::DirectWrite(operation) => {
                entry.code.normalized.push(operation)
            }
            DsActionReplayClassification::Unsupported(_) => entry.code.opaque_word_pairs += 1,
        }
    }
    finish_code(&mut file, current_code);
    file.comments = pending_comments;
    file
}

fn render_entry(out: &mut String, entry: &MelonDsCheatEntry) {
    for comment in &entry.comments {
        out.push_str(comment);
        out.push('\n');
    }
    let enabled = matches!(entry.state, MelonDsCheatState::Enabled);
    out.push_str(&format!("CODE {} {}\n", u8::from(enabled), entry.name));
    if let Some(description) = &entry.description {
        out.push_str("DESC ");
        out.push_str(description);
        out.push('\n');
    }
    for (first, second) in &entry.code.words {
        out.push_str(&format!("{first:08X} {second:08X}\n"));
    }
    out.push('\n');
}

pub fn render_melonds_cheat_file(file: &MelonDsCheatFile) -> Vec<u8> {
    let mut out = String::new();
    for comment in &file.comments {
        out.push_str(comment);
        out.push('\n');
    }
    let mut in_root = false;
    for item in &file.items {
        match item {
            MelonDsCheatItem::Category(category) => {
                for comment in &category.comments {
                    out.push_str(comment);
                    out.push('\n');
                }
                out.push_str(&format!(
                    "CAT {} {}\n",
                    u8::from(category.only_one_code_enabled),
                    category.name
                ));
                if let Some(description) = &category.description {
                    out.push_str("DESC ");
                    out.push_str(description);
                    out.push('\n');
                }
                out.push('\n');
                for entry in &category.entries {
                    render_entry(&mut out, entry);
                }
                in_root = false;
            }
            MelonDsCheatItem::RootCode(entry) => {
                if !in_root {
                    out.push_str("ROOT\n\n");
                    in_root = true;
                }
                render_entry(&mut out, entry);
            }
        }
    }
    out.into_bytes()
}

fn matching_codes(file: &MelonDsCheatFile, name: &str) -> Vec<(Option<usize>, usize)> {
    let mut matches = Vec::new();
    for (item_index, item) in file.items.iter().enumerate() {
        match item {
            MelonDsCheatItem::Category(category) => {
                for (code_index, entry) in category.entries.iter().enumerate() {
                    if entry.name == name {
                        matches.push((Some(item_index), code_index));
                    }
                }
            }
            MelonDsCheatItem::RootCode(entry) if entry.name == name => {
                matches.push((None, item_index));
            }
            MelonDsCheatItem::RootCode(_) => {}
        }
    }
    matches
}

pub fn set_melonds_cheat_state(
    file: &mut MelonDsCheatFile,
    name: &str,
    state: MelonDsCheatState,
) -> Result<bool, String> {
    let matches = matching_codes(file, name);
    if matches.is_empty() {
        return Err("melonDS cheat was not found".into());
    }
    if matches.len() > 1 {
        return Err("melonDS cheat name is ambiguous".into());
    }
    let (category, index) = matches[0];
    if let Some(category_index) = category {
        let MelonDsCheatItem::Category(value) = &mut file.items[category_index] else {
            return Err("melonDS category structure changed".into());
        };
        let changed = value.entries[index].state != state;
        value.entries[index].state = state;
        if matches!(state, MelonDsCheatState::Enabled) && value.only_one_code_enabled {
            for (other, entry) in value.entries.iter_mut().enumerate() {
                if other != index {
                    entry.state = MelonDsCheatState::Disabled;
                }
            }
        }
        Ok(changed)
    } else {
        let MelonDsCheatItem::RootCode(entry) = &mut file.items[index] else {
            return Err("melonDS root code structure changed".into());
        };
        let changed = entry.state != state;
        entry.state = state;
        Ok(changed)
    }
}

pub fn remove_melonds_cheat(file: &mut MelonDsCheatFile, name: &str) -> Result<bool, String> {
    let matches = matching_codes(file, name);
    if matches.is_empty() {
        return Err("melonDS cheat was not found".into());
    }
    if matches.len() > 1 {
        return Err("melonDS cheat name is ambiguous".into());
    }
    let (category, index) = matches[0];
    if let Some(category_index) = category {
        let MelonDsCheatItem::Category(value) = &mut file.items[category_index] else {
            return Err("melonDS category structure changed".into());
        };
        value.entries.remove(index);
    } else {
        file.items.remove(index);
    }
    Ok(true)
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn identity_value(identity: &MelonDsRomIdentity) -> Option<String> {
    if !identity.verified {
        return None;
    }
    if let Some(hash) = identity
        .rom_sha256
        .as_deref()
        .filter(|hash| safe_sha256(hash))
    {
        return Some(format!("sha256:{hash}"));
    }
    identity
        .game_code
        .as_deref()
        .filter(|code| safe_game_code(code))
        .map(|code| format!("game_code:{code}"))
}

pub fn melonds_loadability_facts(identity: &MelonDsRomIdentity) -> Option<MelonDsLoadabilityFacts> {
    let identifier = identity_value(identity)?;
    Some(MelonDsLoadabilityFacts {
        cheat_path: destination_path(&identity.rom_path, None)?,
        target_game_identifier: identifier,
        native_format: MelonDsCheatFormat::MchText,
        global_cheats_key: "EnableCheats".into(),
        global_cheat_setting_modified: false,
        reload_required: true,
        understood_operations_are_partial: true,
    })
}

pub fn build_melonds_cheat_apply_plan(
    request: MelonDsCheatApplyRequest,
) -> Result<MelonDsCheatApplyPlan, String> {
    let identifier = identity_value(&request.identity)
        .ok_or_else(|| "verified ROM hash or game code is required".to_string())?;
    if !request.destination_root.is_absolute()
        || !request.staging_path.is_absolute()
        || request
            .staging_path
            .strip_prefix(&request.destination_root)
            .is_err()
    {
        return Err("melonDS staging path escaped the approved root".into());
    }
    let destination = destination_path(&request.selected_rom, Some(&request.destination_root))
        .ok_or_else(|| "unsafe melonDS destination".to_string())?;
    if destination
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("melonDS destination traversal refused".into());
    }
    let mut file = request
        .existing_bytes
        .as_deref()
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map_err(|_| "melonDS MCH is not UTF-8 text".to_string())
                .map(parse_melonds_cheat_file)
        })
        .transpose()?
        .unwrap_or_else(empty_file);
    let changed_name = request.incoming.name.clone();
    let matches = matching_codes(&file, &changed_name);
    if matches.len() > 1 {
        return Err("existing melonDS cheat name is ambiguous".into());
    }
    if let Some((category, index)) = matches.first().copied() {
        if let Some(category_index) = category {
            let MelonDsCheatItem::Category(value) = &mut file.items[category_index] else {
                return Err("melonDS category structure changed".into());
            };
            value.entries[index] = request.incoming;
        } else {
            let MelonDsCheatItem::RootCode(entry) = &mut file.items[index] else {
                return Err("melonDS root code structure changed".into());
            };
            *entry = request.incoming;
        }
    } else {
        file.items
            .push(MelonDsCheatItem::RootCode(request.incoming));
    }
    let output_bytes = render_melonds_cheat_file(&file);
    let reparsed = parse_melonds_cheat_file(std::str::from_utf8(&output_bytes).unwrap_or_default());
    if reparsed
        .issues
        .iter()
        .any(|issue| !matches!(issue, MelonDsCheatParseIssue::DuplicateName(_)))
    {
        return Err("generated melonDS MCH did not reparse cleanly".into());
    }
    let relative = destination
        .strip_prefix(&request.destination_root)
        .map_err(|_| "melonDS destination escaped approved root".to_string())?
        .to_path_buf();
    let preview = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::MelonDs,
        selected_archive: request.selected_rom.clone(),
        platform: Some("nds".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::MelonDsRomIdentity,
            state: PreviewIdentityState::Verified,
            value: Some(identifier),
            archive_path: request.selected_rom.clone(),
            revision: None,
        },
        destination_root: request.destination_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::MelonDs,
            source_path: request.staging_path,
            expected_source_digest: Some(sha256(&output_bytes)),
            destination_relative_paths: vec![relative],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| format!("melonDS preview refused: {error:?}"))?;
    let shared = build_shared_transaction_plan(
        &preview,
        "melonds-native-cheats",
        "melonds_mch",
        &request.destination_root,
    )
    .map_err(|error| format!("melonDS transaction plan refused: {error:?}"))?;
    Ok(MelonDsCheatApplyPlan {
        shared,
        output_bytes,
        destination,
        identity: request.identity,
        changed_name,
    })
}

pub fn apply_melonds_cheat_plan(
    plan: &MelonDsCheatApplyPlan,
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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# local comment\nCAT 1 Gameplay\nDESC mutually exclusive\nCODE 1 Infinite Lives\n02000000 00000063\nCODE 0 Max Money\n22000010 000000FF\n\nROOT\nCODE 1 Root Code\n94000130 FCFD0200\nD2000000 00000000\n";

    #[test]
    fn parses_native_categories_root_codes_and_state() {
        let file = parse_melonds_cheat_file(SAMPLE);
        assert!(file.issues.is_empty(), "issues: {:?}", file.issues);
        assert_eq!(file.items.len(), 2);
        let MelonDsCheatItem::Category(category) = &file.items[0] else {
            panic!()
        };
        assert!(category.only_one_code_enabled);
        assert_eq!(category.entries[0].state, MelonDsCheatState::Enabled);
        assert_eq!(category.entries[0].code.normalized.len(), 1);
        assert_eq!(category.entries[1].state, MelonDsCheatState::Disabled);
        let MelonDsCheatItem::RootCode(root) = &file.items[1] else {
            panic!()
        };
        assert_eq!(root.code.opaque_word_pairs, 2);
    }

    #[test]
    fn unknown_words_are_retained_and_writer_is_deterministic() {
        let file = parse_melonds_cheat_file(SAMPLE);
        let first = render_melonds_cheat_file(&file);
        assert_eq!(first, render_melonds_cheat_file(&file));
        assert!(String::from_utf8_lossy(&first).contains("94000130 FCFD0200"));
    }

    #[test]
    fn enable_disable_remove_and_only_one_rule_are_native() {
        let mut file = parse_melonds_cheat_file(SAMPLE);
        set_melonds_cheat_state(&mut file, "Max Money", MelonDsCheatState::Enabled).unwrap();
        let MelonDsCheatItem::Category(category) = &file.items[0] else {
            panic!()
        };
        assert_eq!(category.entries[0].state, MelonDsCheatState::Disabled);
        assert_eq!(category.entries[1].state, MelonDsCheatState::Enabled);
        set_melonds_cheat_state(&mut file, "Max Money", MelonDsCheatState::Disabled).unwrap();
        assert_eq!(remove_melonds_cheat(&mut file, "Max Money"), Ok(true));
        assert!(
            matches!(&file.items[0], MelonDsCheatItem::Category(value) if value.entries.len() == 1)
        );
    }

    #[test]
    fn malformed_and_identity_cases_fail_closed() {
        let file = parse_melonds_cheat_file("CODE 1 Broken\nnot words\nDESC outside\n");
        assert!(!file.issues.is_empty());
        let identity = MelonDsRomIdentity {
            rom_path: PathBuf::from("/games/title.nds"),
            rom_sha256: None,
            game_code: None,
            verified: false,
            title_hint: Some("Title only".into()),
        };
        assert!(melonds_loadability_facts(&identity).is_none());
        let verified = MelonDsRomIdentity {
            rom_sha256: Some("a".repeat(64)),
            verified: true,
            ..identity
        };
        assert!(melonds_loadability_facts(&verified).is_some());
    }

    #[test]
    fn usrcheat_is_user_import_only_and_no_rom_is_changed() {
        assert_eq!(
            MelonDsUsrCheatStatus::UserImportOnly,
            MelonDsUsrCheatStatus::UserImportOnly
        );
        let input = SAMPLE.as_bytes().to_vec();
        let file = parse_melonds_cheat_file(SAMPLE);
        assert_eq!(input, SAMPLE.as_bytes());
        assert_eq!(file.format, MelonDsCheatFormat::MchText);
    }
}
