//! Official arcade listxml evidence through the existing parser/audit pipeline.
use super::*;
use crate::dat::{
    audit::AuditVerdict,
    limits::DatLimits,
    sources::{
        DatSourceKind,
        audit_run::{DatAuditRequest, run_dat_audit},
    },
};
use crate::safe_read::TrustedRoots;
use std::{io::Write, path::Path, sync::atomic::AtomicBool};

/// Deliberately bounded POC catalogue; the exact command is retained as source
/// identity. No software lists, download or claim of full-MAME coverage.
pub fn capture(executable: &Path) -> ProviderResult<ProviderSnapshot> {
    let executable = executable.canonicalize().map_err(|e| e.to_string())?;
    let before = tool::fingerprint(&executable)?;
    let (bytes, stderr) = tool::run(
        &executable,
        &[
            "-listxml".into(),
            "puckman".into(),
            "pacman".into(),
            "galaga".into(),
        ],
        false,
        MAX_SNAPSHOT_BYTES,
    )?;
    if tool::fingerprint(&executable)? != before {
        return Err("MAME changed during capture".into());
    }
    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    let imported = crate::identity_source::mame_listxml::import_mame_listxml(file.path())
        .map_err(|e| e.to_string())?;
    let mut catalogue = imported.dat;
    catalogue.source.file_path = "official-local:mame/-listxml/puckman,pacman,galaga".into();
    let version = catalogue
        .source
        .version
        .clone()
        .ok_or("MAME did not report a build version")?;
    let mut warnings = catalogue.source.parse_warnings.clone();
    warnings.push("POC scope: puckman, pacman, galaga and emitted device records; not the full arcade catalogue".into());
    if !stderr.trim().is_empty() {
        warnings.push(stderr);
    }
    Ok(ProviderSnapshot {
        provider: IdentityProvider::Mame,
        version,
        source_identifier: catalogue.source.file_path.clone(),
        executable,
        executable_sha256: before,
        source_sha256: sha256(&bytes),
        parser_version: PARSER_VERSION,
        records: ProviderRecords::DatLike {
            catalogue,
            listxml: String::from_utf8(bytes).map_err(|e| e.to_string())?,
        },
        warnings,
    })
}

pub fn verify(snapshot: &ProviderSnapshot, path: &Path) -> ProviderResult<ProviderIdentityResult> {
    let ProviderRecords::DatLike { listxml, .. } = &snapshot.records else {
        return Err("MAME requires listxml evidence".into());
    };
    if !path.is_file() {
        return Err(
            "Choose one bounded ROM/archive file for this POC, not a whole collection".into(),
        );
    }
    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    file.write_all(listxml.as_bytes())
        .map_err(|e| e.to_string())?;
    let trusted = TrustedRoots::from_paths([path]);
    let outcome = run_dat_audit(
        &DatAuditRequest {
            source_id: snapshot.sha256()?,
            source_display_name: "MAME official".into(),
            dat_path: file.path().to_path_buf(),
            dat_kind: DatSourceKind::File,
            scan_root: path.into(),
            limits: DatLimits::default(),
            policy: None,
            platform: Some("arcade".into()),
        },
        &trusted,
        &AtomicBool::new(false),
        &|_| {},
    )
    .map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    let mut exact = false;
    let mut probable = false;
    let mut ambiguous = false;
    let verdicts = outcome.report.entries.iter().map(|e| &e.verdict).chain(
        outcome
            .archives
            .iter()
            .flat_map(|a| a.members.iter().filter_map(|m| m.verdict.as_ref())),
    );
    for verdict in verdicts {
        match verdict {
            AuditVerdict::Exact { game_name, .. } => {
                exact = true;
                candidates.push(game_name.clone());
            }
            AuditVerdict::ExactMultipleCandidates { game_names, .. } => {
                ambiguous = true;
                candidates.extend(game_names.clone());
            }
            AuditVerdict::Probable { game_name, .. } => {
                probable = true;
                candidates.push(game_name.clone());
            }
            AuditVerdict::ProbableMultipleCandidates { game_names, .. } => {
                ambiguous = true;
                candidates.extend(game_names.clone());
            }
            AuditVerdict::Ambiguous { .. } => ambiguous = true,
            _ => {}
        }
    }
    candidates.sort();
    candidates.dedup();
    let status = if outcome.truncated || !outcome.unhashed.is_empty() {
        MatchStatus::NeedsRecheck
    } else if ambiguous || candidates.len() > 1 {
        MatchStatus::Ambiguous
    } else if exact {
        MatchStatus::Exact
    } else if probable {
        MatchStatus::Probable
    } else {
        MatchStatus::NoMatch
    };
    Ok(ProviderIdentityResult {
        provider: IdentityProvider::Mame,
        snapshot_sha256: snapshot.sha256()?,
        path: path.into(),
        status,
        detection_class: if matches!(status, MatchStatus::Exact) {
            DetectionClass::OfficialExact
        } else {
            DetectionClass::Unknown
        },
        origin: MatchOrigin::OfficialMame,
        detector_method: Some("MAME -listxml plus bounded official DAT audit".into()),
        official_game_id: None,
        configured_target_id: None,
        configured_target_evidence: None,
        possible_base_game_id: None,
        cleaned_local_title: None,
        candidates,
        details: vec!["Status describes ROM-byte identity, not completeness or permission to launch. Shared parent/clone ROMs remain ambiguous.".into(), serde_json::to_string(&outcome.sets).map_err(|e| e.to_string())?, serde_json::to_string(&outcome.report).map_err(|e| e.to_string())?, format!("{} archive(s); {} bytes hashed; {} unreadable items", outcome.archives.len(), outcome.bytes_hashed + outcome.archive_bytes_hashed, outcome.unhashed.len())],
        discovery: None,
    })
}
