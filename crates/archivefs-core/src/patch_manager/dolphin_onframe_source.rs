//! Read-only discovery and verified binding for Dolphin `[OnFrame]` patches.

use std::fs;
use std::path::{Path, PathBuf};

use super::cheat_ir::{
    CheatDocument, CheatIssue, CheatOperation, CheatPlatform, CheatSourceFormat,
    dolphin_on_frame_line_to_ir,
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
    let mut original_lines = Vec::<String>::new();
    let mut ops = Vec::new();
    let mut issues = Vec::new();
    let mut out = Vec::new();
    let flush = |original_lines: &mut Vec<String>,
                 title: &mut Option<String>,
                 ops: &mut Vec<CheatOperation>,
                 issues: &mut Vec<CheatIssue>,
                 out: &mut Vec<DolphinOnFrameCandidate>| {
        if let Some(name) = title.take()
            && (!ops.is_empty() || !issues.is_empty())
        {
            let doc = CheatDocument {
                source_evidence: vec![super::cheat_provenance::CheatRecordProvenance {
                    record_index: u32::try_from(out.len()).ok(),
                    original_description: Some(name.clone()),
                    original_code: Some(std::mem::take(original_lines).join("\n")),
                    ..super::cheat_provenance::CheatRecordProvenance::local(
                        path,
                        text.as_bytes(),
                        "dolphin_onframe",
                    )
                }],
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
    };
    for line in text.lines() {
        let t = line.trim();
        if t.eq_ignore_ascii_case("[onframe]") {
            in_section = true;
            continue;
        }
        if t.starts_with('[') {
            if in_section {
                flush(
                    &mut original_lines,
                    &mut title,
                    &mut ops,
                    &mut issues,
                    &mut out,
                );
            }
            in_section = false;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(name) = t.strip_prefix('$') {
            flush(
                &mut original_lines,
                &mut title,
                &mut ops,
                &mut issues,
                &mut out,
            );
            title = Some(name.trim().to_string());
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        original_lines.push(line.to_string());
        // The project's one strict reading of an OnFrame line: exactly an
        // address, a width and a value. Anything else (a conditional line
        // with more fields, a line with fewer) is an issue that stops the
        // install, never a different write or a line quietly left out.
        match dolphin_on_frame_line_to_ir(t) {
            CheatOperation::UnsupportedRaw { reason, .. } => {
                issues.push(CheatIssue::UnsupportedOperation(format!(
                    "unsupported OnFrame line: {t} ({reason})"
                )));
            }
            operation => ops.push(operation),
        }
    }
    if in_section {
        flush(
            &mut original_lines,
            &mut title,
            &mut ops,
            &mut issues,
            &mut out,
        );
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
    /// A line this module cannot read exactly is not installed as something
    /// else: a conditional line with four fields is not cut down to an
    /// unconditional write, and a line with too few fields is not dropped
    /// while the rest of the patch installs as if it were complete.
    #[test]
    fn a_line_that_is_not_an_exact_write_stops_the_install() {
        let dir = tempfile::tempdir().unwrap();
        for (body, kept) in [
            ("$Conditional\n0x80001234:dword:0x1:0x0\n", 0),
            ("$Short\n0x80001234:dword\n0x80005678:dword:0x2\n", 1),
        ] {
            let path = dir.path().join("GMSE01.ini");
            fs::write(&path, format!("[OnFrame]\n{body}")).unwrap();
            let found =
                discover_dolphin_onframe_candidates(&path, CheatPlatform::GameCube).unwrap();
            assert_eq!(found.len(), 1, "{body}");
            let document = &found[0].document;
            assert_eq!(document.operations.len(), kept, "{body}");
            assert_eq!(document.issues.len(), 1, "{body}");
            let binding =
                bind_dolphin_onframe_candidate(&found[0], Some("GMSE01"), Path::new("/p"), false)
                    .unwrap();
            assert!(!binding.can_install, "{body}");
        }
    }

    /// The `=` spelling the strict reader accepts is read as the same write.
    #[test]
    fn an_exact_write_in_either_spelling_is_installable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("GMSE01.ini");
        fs::write(
            &path,
            "[OnFrame]\n$Both\n0x80001234:dword:0x1\n0x80001238=word:0x2\n",
        )
        .unwrap();
        let found = discover_dolphin_onframe_candidates(&path, CheatPlatform::GameCube).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].document.issues.is_empty());
        assert_eq!(
            found[0].document.operations,
            vec![
                CheatOperation::OnFrameWrite32 {
                    address: 0x8000_1234,
                    value: 1
                },
                CheatOperation::OnFrameWrite16 {
                    address: 0x8000_1238,
                    value: 2
                },
            ]
        );
    }

    #[test]
    fn binding_requires_verified_identity() {
        let candidate = DolphinOnFrameCandidate {
            title: "x".into(),
            source_path: PathBuf::from("x.ini"),
            platform: CheatPlatform::Wii,
            document: CheatDocument {
                source_evidence: Vec::new(),
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
