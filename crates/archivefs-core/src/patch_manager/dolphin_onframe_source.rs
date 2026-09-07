//! Read-only discovery and verified binding for Dolphin `[OnFrame]` patches.

use std::fs;
use std::path::{Path, PathBuf};

use super::cheat_ir::{
    CheatDocument, CheatIssue, CheatOperation, CheatPlatform, CheatSourceFormat,
};

#[derive(Debug, Clone)]
pub struct DolphinOnFrameCandidate {
    pub title: String,
    pub source_path: PathBuf,
    pub platform: CheatPlatform,
    pub document: CheatDocument,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DolphinOnFrameBinding {
    pub platform: CheatPlatform,
    pub game_id: String,
    pub profile: String,
    pub gamesettings_path: PathBuf,
    pub source_document: CheatDocument,
    pub provenance: PathBuf,
    pub can_install: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DolphinOnFrameSourceError {
    Io(String),
    MissingIdentity,
    ConflictingIdentity,
    UnsupportedPlatform,
}

impl std::fmt::Display for DolphinOnFrameSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Io(s) => s,
            Self::MissingIdentity => "verified Dolphin game identity is required",
            Self::ConflictingIdentity => "conflicting Dolphin game identities",
            Self::UnsupportedPlatform => "only GameCube/Wii OnFrame sources are supported",
        })
    }
}
impl std::error::Error for DolphinOnFrameSourceError {}

pub fn discover_dolphin_onframe_candidates(
    path: &Path,
    platform: CheatPlatform,
) -> Result<Vec<DolphinOnFrameCandidate>, DolphinOnFrameSourceError> {
    if !matches!(platform, CheatPlatform::GameCube | CheatPlatform::Wii) {
        return Err(DolphinOnFrameSourceError::UnsupportedPlatform);
    }
    let text =
        fs::read_to_string(path).map_err(|e| DolphinOnFrameSourceError::Io(e.to_string()))?;
    let mut in_section = false;
    let mut title: Option<String> = None;
    let mut ops = Vec::new();
    let mut issues = Vec::new();
    let mut out = Vec::new();
    let flush = |title: &mut Option<String>,
                 ops: &mut Vec<CheatOperation>,
                 issues: &mut Vec<CheatIssue>,
                 out: &mut Vec<DolphinOnFrameCandidate>| {
        if let Some(name) = title.take() {
            if !ops.is_empty() || !issues.is_empty() {
                let doc = CheatDocument {
                    title: name.clone(),
                    platform: platform.clone(),
                    source_format: CheatSourceFormat::DolphinOnFrame,
                    operations: std::mem::take(ops),
                    issues: std::mem::take(issues),
                    provenance: vec![path.display().to_string()],
                };
                out.push(DolphinOnFrameCandidate {
                    title: name,
                    source_path: path.to_path_buf(),
                    platform: platform.clone(),
                    document: doc,
                    warnings: Vec::new(),
                });
            }
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if t.eq_ignore_ascii_case("[onframe]") {
            in_section = true;
            continue;
        }
        if t.starts_with('[') {
            if in_section {
                flush(&mut title, &mut ops, &mut issues, &mut out);
            }
            in_section = false;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(name) = t.strip_prefix('$') {
            flush(&mut title, &mut ops, &mut issues, &mut out);
            title = Some(name.trim().to_string());
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let mut parts = t.split(':');
        let (Some(a), Some(width), Some(v)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let parsed = u64::from_str_radix(a.trim().trim_start_matches("0x"), 16).ok();
        let value = u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok();
        let op = match (parsed, value, width.trim().to_ascii_lowercase().as_str()) {
            (Some(address), Some(value), "byte") if value <= 0xff => {
                Some(CheatOperation::OnFrameWrite8 {
                    address,
                    value: value as u8,
                })
            }
            (Some(address), Some(value), "word") if value <= 0xffff => {
                Some(CheatOperation::OnFrameWrite16 {
                    address,
                    value: value as u16,
                })
            }
            (Some(address), Some(value), "dword") => {
                Some(CheatOperation::OnFrameWrite32 { address, value })
            }
            _ => None,
        };
        if let Some(op) = op {
            ops.push(op);
        } else {
            issues.push(CheatIssue::UnsupportedOperation(format!(
                "unsupported OnFrame line: {t}"
            )));
        }
    }
    if in_section {
        flush(&mut title, &mut ops, &mut issues, &mut out);
    }
    Ok(out)
}

pub fn bind_dolphin_onframe_candidate(
    candidate: &DolphinOnFrameCandidate,
    game_id: Option<&str>,
    profile: &Path,
    conflicting_ids: bool,
) -> Result<DolphinOnFrameBinding, DolphinOnFrameSourceError> {
    if conflicting_ids {
        return Err(DolphinOnFrameSourceError::ConflictingIdentity);
    }
    let game_id = game_id
        .filter(|v| !v.trim().is_empty())
        .ok_or(DolphinOnFrameSourceError::MissingIdentity)?
        .to_string();
    let gamesettings_path = profile.join("GameSettings").join(format!("{game_id}.ini"));
    let mut reasons = Vec::new();
    let can_install =
        candidate.document.issues.is_empty() && !candidate.document.operations.is_empty();
    if !can_install {
        reasons.push("OnFrame document contains unsupported or no operations".into());
    }
    Ok(DolphinOnFrameBinding {
        platform: candidate.platform.clone(),
        game_id,
        profile: profile.display().to_string(),
        gamesettings_path,
        source_document: candidate.document.clone(),
        provenance: candidate.source_path.clone(),
        can_install,
        reasons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_only_onframe_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GMSE01.ini");
        fs::write(
            &path,
            "[ActionReplay]\n$AR\n0x1\n[OnFrame]\n$60 FPS\n0x80001234:dword:0x1\n",
        )
        .unwrap();
        let c = discover_dolphin_onframe_candidates(&path, CheatPlatform::GameCube).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].title, "60 FPS");
    }
    #[test]
    fn binding_requires_verified_identity() {
        let candidate = DolphinOnFrameCandidate {
            title: "x".into(),
            source_path: PathBuf::from("x.ini"),
            platform: CheatPlatform::Wii,
            document: CheatDocument {
                title: "x".into(),
                platform: CheatPlatform::Wii,
                source_format: CheatSourceFormat::DolphinOnFrame,
                operations: vec![CheatOperation::OnFrameWrite8 {
                    address: 1,
                    value: 2,
                }],
                issues: Vec::new(),
                provenance: vec!["x".into()],
            },
            warnings: Vec::new(),
        };
        assert!(matches!(
            bind_dolphin_onframe_candidate(&candidate, None, Path::new("/p"), false),
            Err(DolphinOnFrameSourceError::MissingIdentity)
        ));
    }
}
