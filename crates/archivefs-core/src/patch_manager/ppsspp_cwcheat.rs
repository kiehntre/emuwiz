//! Native PPSSPP CWCheat parsing, deterministic merge, and transaction preview.
//!
//! This module is deliberately PPSSPP-specific. It does not alter generic
//! cheat matching or routing and never interprets unknown CWCheat operations
//! as ordinary memory writes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tempfile::{TempDir, tempdir};

use super::cheat_ir::{CheatIssue, CheatOperation};
use super::ppsspp_local::PpssppProfile;
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport, SharedPreviewRequest,
    build_shared_preview,
};
use super::shared_transaction::{SharedTransactionPlan, build_shared_transaction_plan};

pub const PPSSPP_CWCHEAT_MAX_BYTES: u64 = 512 * 1024;
pub const PPSSPP_CWCHEAT_MAX_LINES: usize = 16_384;
pub const PPSSPP_CWCHEAT_MAX_ENTRIES: usize = 1_024;
pub const PPSSPP_CWCHEAT_MAX_CODE_LINES: usize = 256;
pub const PPSSPP_CWCHEAT_MAX_LINE_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PpssppCheatLine {
    pub raw: String,
    pub supported_operation: Option<CheatOperation>,
    pub issues: Vec<CheatIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PpssppCheatEntry {
    pub title: String,
    pub enabled: bool,
    pub lines: Vec<PpssppCheatLine>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PpssppCheatGame {
    pub game_id: String,
    pub title: Option<String>,
    pub cheats: Vec<PpssppCheatEntry>,
    pub retained_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PpssppCheatFile {
    pub game_id: String,
    pub game: PpssppCheatGame,
    pub source_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PpssppCheatParseIssue {
    Empty,
    TooLarge,
    TooManyLines,
    LineTooLong { line: usize },
    MissingGameId,
    MultipleGameIds,
    MalformedGameId { line: usize },
    CheatBeforeTitle { line: usize },
    TooManyCheats,
    TooManyCodeLines { line: usize },
    UnsupportedCode { line: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppCheatState {
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppGlobalCheatState {
    Enabled,
    Disabled,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PpssppReloadRequirement {
    ReloadCheatsOrGame,
}

#[derive(Debug)]
pub struct PpssppCwCheatStaged {
    pub staging_root: PathBuf,
    pub path: PathBuf,
    pub digest: String,
    pub contents: String,
    _guard: Arc<TempDir>,
}

#[derive(Debug)]
pub struct PpssppCwCheatPreview {
    pub report: SharedPreviewReport,
    pub transaction: SharedTransactionPlan,
    pub staged: PpssppCwCheatStaged,
    pub reload_requirement: PpssppReloadRequirement,
}

pub fn parse_ppsspp_cwcheat(bytes: &[u8]) -> Result<PpssppCheatFile, PpssppCheatParseIssue> {
    if bytes.is_empty() {
        return Err(PpssppCheatParseIssue::Empty);
    }
    if bytes.len() as u64 > PPSSPP_CWCHEAT_MAX_BYTES {
        return Err(PpssppCheatParseIssue::TooLarge);
    }
    let text = String::from_utf8_lossy(bytes);
    let mut game_id = None;
    let mut title = None;
    let mut entries = Vec::new();
    let mut current: Option<PpssppCheatEntry> = None;
    let mut retained = Vec::new();
    let mut lines = 0;
    for (index, raw) in text.lines().enumerate() {
        lines += 1;
        if lines > PPSSPP_CWCHEAT_MAX_LINES {
            return Err(PpssppCheatParseIssue::TooManyLines);
        }
        if raw.len() > PPSSPP_CWCHEAT_MAX_LINE_BYTES {
            return Err(PpssppCheatParseIssue::LineTooLong { line: index + 1 });
        }
        let line = raw.trim();
        if let Some(value) = line.strip_prefix("_S") {
            let id = normalize_game_id(value.trim())
                .ok_or(PpssppCheatParseIssue::MalformedGameId { line: index + 1 })?;
            if game_id.as_deref().is_some_and(|old| old != id) {
                return Err(PpssppCheatParseIssue::MultipleGameIds);
            }
            game_id = Some(id);
        } else if let Some(value) = line.strip_prefix("_G") {
            title = Some(value.trim().to_string());
        } else if let Some(value) = line
            .strip_prefix("_C0")
            .or_else(|| line.strip_prefix("_C1"))
        {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            if entries.len() >= PPSSPP_CWCHEAT_MAX_ENTRIES {
                return Err(PpssppCheatParseIssue::TooManyCheats);
            }
            current = Some(PpssppCheatEntry {
                title: value.trim().to_string(),
                enabled: line.starts_with("_C1"),
                lines: Vec::new(),
            });
        } else if let Some(value) = line.strip_prefix("_L") {
            let entry = current
                .as_mut()
                .ok_or(PpssppCheatParseIssue::CheatBeforeTitle { line: index + 1 })?;
            if entry.lines.len() >= PPSSPP_CWCHEAT_MAX_CODE_LINES {
                return Err(PpssppCheatParseIssue::TooManyCodeLines { line: index + 1 });
            }
            entry.lines.push(parse_code_line(value.trim(), index + 1));
        } else if !line.is_empty() {
            retained.push(raw.to_string());
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    let game_id = game_id.ok_or(PpssppCheatParseIssue::MissingGameId)?;
    Ok(PpssppCheatFile {
        game_id: game_id.clone(),
        game: PpssppCheatGame {
            game_id,
            title,
            cheats: entries,
            retained_lines: retained,
        },
        source_sha256: None,
    })
}

pub fn parse_ppsspp_cwcheat_file(path: &Path) -> Result<PpssppCheatFile, PpssppCheatParseIssue> {
    let metadata = fs::metadata(path).map_err(|_| PpssppCheatParseIssue::Empty)?;
    if metadata.len() > PPSSPP_CWCHEAT_MAX_BYTES {
        return Err(PpssppCheatParseIssue::TooLarge);
    }
    let bytes = fs::read(path).map_err(|_| PpssppCheatParseIssue::Empty)?;
    let mut parsed = parse_ppsspp_cwcheat(&bytes)?;
    parsed.source_sha256 = Some(sha256(&bytes));
    Ok(parsed)
}

pub fn set_ppsspp_cheat_state(
    file: &mut PpssppCheatFile,
    title: &str,
    state: PpssppCheatState,
) -> bool {
    if let Some(entry) = file
        .game
        .cheats
        .iter_mut()
        .find(|entry| entry.title == title)
    {
        entry.enabled = state == PpssppCheatState::Enabled;
        true
    } else {
        false
    }
}

pub fn remove_ppsspp_cheat(file: &mut PpssppCheatFile, title: &str) -> bool {
    let before = file.game.cheats.len();
    file.game.cheats.retain(|entry| entry.title != title);
    before != file.game.cheats.len()
}

pub fn merge_ppsspp_cheat(file: &mut PpssppCheatFile, incoming: PpssppCheatEntry) -> bool {
    if file
        .game
        .cheats
        .iter()
        .any(|entry| entry.title == incoming.title && entry.lines == incoming.lines)
    {
        return false;
    }
    file.game.cheats.push(incoming);
    file.game.cheats.sort_by(|a, b| {
        a.title
            .cmp(&b.title)
            .then(a.lines.len().cmp(&b.lines.len()))
    });
    true
}

pub fn render_ppsspp_cwcheat(file: &PpssppCheatFile) -> String {
    let mut output = String::new();
    output.push_str(&format!("_S {}\n", file.game_id));
    if let Some(title) = &file.game.title {
        output.push_str(&format!("_G {}\n", title));
    }
    for entry in &file.game.cheats {
        output.push_str(&format!(
            "_C{} {}\n",
            if entry.enabled { 1 } else { 0 },
            entry.title
        ));
        for line in &entry.lines {
            output.push_str(&format!("_L {}\n", line.raw.trim()));
        }
    }
    for line in &file.game.retained_lines {
        output.push_str(line);
        output.push('\n');
    }
    output
}

pub fn build_ppsspp_cwcheat_preview(
    profile: &PpssppProfile,
    verified_game_id: &str,
    selected_archive: &Path,
    file: &PpssppCheatFile,
) -> Result<PpssppCwCheatPreview, String> {
    let expected =
        normalize_game_id(verified_game_id).ok_or("verified PSP game ID is malformed")?;
    if file.game_id != expected {
        return Err("CWCheat game ID does not match the verified PSP game ID".into());
    }
    if !profile.eligible {
        return Err("PPSSPP profile is not eligible".into());
    }
    let guard = Arc::new(tempdir().map_err(|error| error.to_string())?);
    let contents = render_ppsspp_cwcheat(file);
    let path = guard.path().join(format!("{expected}.ini"));
    fs::write(&path, contents.as_bytes()).map_err(|error| error.to_string())?;
    let staged = PpssppCwCheatStaged {
        staging_root: guard.path().to_path_buf(),
        path: path.clone(),
        digest: sha256(contents.as_bytes()),
        contents,
        _guard: guard,
    };
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::Ppsspp,
        selected_archive: selected_archive.to_path_buf(),
        platform: Some("PSP".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::PpssppDiscId,
            state: PreviewIdentityState::Verified,
            value: Some(expected.clone()),
            archive_path: selected_archive.to_path_buf(),
            revision: None,
        },
        destination_root: profile.cheats_path.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Ppsspp,
            source_path: path,
            expected_source_digest: Some(staged.digest.clone()),
            destination_relative_paths: vec![PathBuf::from(format!("{expected}.ini"))],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| error.to_string())?;
    let transaction = build_shared_transaction_plan(
        &report,
        &profile.profile_id,
        "ppsspp-cwcheat",
        &staged.staging_root,
    )
    .map_err(|error| format!("{:?}: {}", error.kind, error.detail))?;
    Ok(PpssppCwCheatPreview {
        report,
        transaction,
        staged,
        reload_requirement: PpssppReloadRequirement::ReloadCheatsOrGame,
    })
}

pub fn ppsspp_global_cheat_state(enabled: Option<bool>) -> PpssppGlobalCheatState {
    match enabled {
        Some(true) => PpssppGlobalCheatState::Enabled,
        Some(false) => PpssppGlobalCheatState::Disabled,
        None => PpssppGlobalCheatState::Unknown,
    }
}

fn parse_code_line(raw: &str, line: usize) -> PpssppCheatLine {
    let words: Vec<_> = raw.split_whitespace().collect();
    if words.len() == 2
        && words[0].strip_prefix("0x").is_some()
        && words[1].strip_prefix("0x").is_some()
    {
        if let (Ok(code), Ok(value)) = (
            u32::from_str_radix(&words[0][2..], 16),
            u32::from_str_radix(&words[1][2..], 16),
        ) {
            if code >> 28 == 2 {
                return PpssppCheatLine {
                    raw: raw.into(),
                    supported_operation: Some(CheatOperation::Write32 {
                        address: (code & 0x0fff_ffff) as u64,
                        value,
                    }),
                    issues: Vec::new(),
                };
            }
        }
    }
    PpssppCheatLine {
        raw: raw.into(),
        supported_operation: None,
        issues: vec![CheatIssue::UnsupportedOperation(format!(
            "unsupported CWCheat line {line}"
        ))],
    }
}

pub fn normalize_game_id(value: &str) -> Option<String> {
    let compact = value.trim().to_ascii_uppercase().replace('-', "");
    let chars: Vec<_> = compact.chars().collect();
    if chars.len() != 9
        || !chars[..4].iter().all(|c| c.is_ascii_alphanumeric())
        || !chars[4..].iter().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some(compact)
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "_S ULUS-10000\n_G Example\n_C0 Infinite Health\n_L 0x2000100C 0x00000000\n_C1 Unsupported\n_L 0xD0000000 0x00000001\n# keep this note\n";

    #[test]
    fn parses_identity_state_and_supported_code() {
        let file = parse_ppsspp_cwcheat(SAMPLE.as_bytes()).unwrap();
        assert_eq!(file.game_id, "ULUS10000");
        assert!(!file.game.cheats[0].enabled);
        assert!(file.game.cheats[0].lines[0].supported_operation.is_some());
        assert!(file.game.cheats[1].lines[0].supported_operation.is_none());
    }

    #[test]
    fn state_changes_and_remove_preserve_other_entries() {
        let mut file = parse_ppsspp_cwcheat(SAMPLE.as_bytes()).unwrap();
        assert!(set_ppsspp_cheat_state(
            &mut file,
            "Infinite Health",
            PpssppCheatState::Enabled
        ));
        assert!(remove_ppsspp_cheat(&mut file, "Unsupported"));
        assert_eq!(file.game.cheats.len(), 1);
        assert!(render_ppsspp_cwcheat(&file).contains("_C1 Infinite Health"));
    }

    #[test]
    fn malformed_and_ambiguous_ids_fail_closed() {
        assert!(matches!(
            parse_ppsspp_cwcheat(b"_S BAD\n_G x\n"),
            Err(PpssppCheatParseIssue::MalformedGameId { .. })
        ));
        assert!(matches!(
            parse_ppsspp_cwcheat(b"_S ULUS10000\n_S ULES20000\n"),
            Err(PpssppCheatParseIssue::MultipleGameIds)
        ));
        assert!(matches!(
            parse_ppsspp_cwcheat(b"_G title\n_C1 code\n"),
            Err(PpssppCheatParseIssue::MissingGameId)
        ));
    }

    #[test]
    fn global_state_is_explicit() {
        assert_eq!(
            ppsspp_global_cheat_state(Some(false)),
            PpssppGlobalCheatState::Disabled
        );
        assert_eq!(
            ppsspp_global_cheat_state(None),
            PpssppGlobalCheatState::Unknown
        );
    }
}
