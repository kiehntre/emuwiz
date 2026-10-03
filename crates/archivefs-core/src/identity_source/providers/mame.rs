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
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// Marker carried by [`ProviderSnapshot::source_identifier`] for a locally
/// imported listxml (see [`ProviderSnapshot::is_local_import`]).
pub use super::LOCAL_IMPORT_PREFIX;

/// The exact, complete capture command (after the supervisor's `-noreadconfig`).
/// There is deliberately no machine list: this is the full machine catalogue.
pub const FULL_LISTXML_ARGS: &[&str] = &["-listxml"];

/// Largest number of candidate names a directory result lists; the full count
/// is always reported separately.
const MAX_LISTED_CANDIDATES: usize = 1000;

/// Capture the complete machine catalogue from an installed MAME build.
///
/// MAME's `-listxml` is read-only and reports its own build in the XML root.
/// There is no version-specific download endpoint: an installed build is the
/// only automatic acquisition route that can be proven for a historical
/// collection. The process is supervised (timeout, CPU and address-space
/// limits, bounded stdout, isolated scratch directory) and never goes through a
/// shell. The executable is fingerprinted before and after the run.
pub fn capture(executable: &Path) -> ProviderResult<ProviderSnapshot> {
    let executable = executable.canonicalize().map_err(|e| e.to_string())?;
    let before = tool::fingerprint(&executable)?;
    let args: Vec<std::ffi::OsString> = FULL_LISTXML_ARGS.iter().map(Into::into).collect();
    let (bytes, stderr) =
        tool::run(&executable, &args, false, MAX_SNAPSHOT_BYTES).map_err(explain_capture_error)?;
    if tool::fingerprint(&executable)? != before {
        return Err("MAME changed during capture".into());
    }
    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    let imported = crate::identity_source::mame_listxml::import_mame_listxml(file.path())
        .map_err(|e| e.to_string())?;
    let mut catalogue = imported.dat;
    catalogue.source.file_path = "official-local:mame/-listxml".into();
    let version = catalogue
        .source
        .version
        .clone()
        .ok_or("MAME did not report a build version")?;
    let mut warnings = catalogue.source.parse_warnings.clone();
    warnings.push(
        "Full machine catalogue from the installed MAME; software lists are not included".into(),
    );
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

/// Like [`capture`], but the installed MAME must report the build the caller
/// asked for. A different build is refused, never substituted: a historical
/// collection verified against today's catalogue gives wrong answers.
pub fn capture_matching(
    executable: &Path,
    expected_version: &str,
) -> ProviderResult<ProviderSnapshot> {
    let snapshot = capture(executable)?;
    require_matching_version(expected_version, &snapshot)?;
    Ok(snapshot)
}

fn explain_capture_error(error: String) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("limit") || lower.contains("exceed") || lower.contains("too large") {
        format!(
            "MAME's full machine list is larger than EmuWiz's snapshot limit ({} MiB). \
             Import a listxml for an older build instead. ({error})",
            MAX_SNAPSHOT_BYTES / (1024 * 1024)
        )
    } else {
        error
    }
}

/// Build a provider snapshot from a user-selected local MAME listxml.
///
/// The XML's own `<mame build>` is the version; the file name is display
/// provenance only. The result is explicitly *imported*: EmuWiz did not run MAME,
/// did not download anything, and does not vouch for the source, so it never
/// carries official trust or the official origin label. It uses the same
/// validation, staging, activation, history and rollback path as a live capture
/// (through [`super::ManagedProviderStore::new_imported`]).
pub fn snapshot_from_mame_listxml(path: &Path) -> ProviderResult<ProviderSnapshot> {
    use std::io::Read;
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Choose a regular MAME listxml file".into());
    }
    if metadata.len() > MAX_SNAPSHOT_BYTES {
        return Err(format!(
            "That listxml is larger than the {} MiB import limit",
            MAX_SNAPSHOT_BYTES / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err("That listxml grew past the import limit while it was read".into());
    }
    // A cut-off download would otherwise import as a quietly incomplete catalogue.
    let tail = &bytes[bytes.len().saturating_sub(64)..];
    if !String::from_utf8_lossy(tail)
        .trim_end()
        .ends_with("</mame>")
    {
        return Err("That listxml looks incomplete (it does not end with </mame>)".into());
    }
    let imported = crate::identity_source::mame_listxml::import_mame_listxml(path)
        .map_err(|error| error.to_string())?;
    let source_hash = sha256(&bytes);
    if imported.artifact_sha256 != source_hash {
        return Err("The listxml changed while it was being imported".into());
    }
    let version = imported
        .upstream_version
        .clone()
        .ok_or("MAME listxml does not report a build version")?;
    let mut warnings = imported.dat.source.parse_warnings.clone();
    warnings.push("Imported locally; EmuWiz did not download or vouch for the source.".into());
    let mut catalogue = imported.dat;
    catalogue.source.file_path = format!("{LOCAL_IMPORT_PREFIX}{}", imported.artifact_name);
    Ok(ProviderSnapshot {
        provider: IdentityProvider::Mame,
        version,
        source_identifier: format!("{LOCAL_IMPORT_PREFIX}{}", imported.artifact_name),
        executable: PathBuf::from(format!("{LOCAL_IMPORT_PREFIX}mame-listxml")),
        executable_sha256: source_hash.clone(),
        source_sha256: source_hash,
        parser_version: PARSER_VERSION,
        records: ProviderRecords::DatLike {
            catalogue,
            listxml: String::from_utf8(bytes).map_err(|_| "MAME listxml is not UTF-8")?,
        },
        warnings,
    })
}

/// Normalises a MAME build string so `0.264`, `0.264 (mame0264)` and `mame0264`
/// compare equal. `None` when no build number can be read.
pub fn normalize_mame_version(value: &str) -> Option<String> {
    let token = value.split_whitespace().next()?.to_ascii_lowercase();
    if let Some(digits) = token.strip_prefix("mame")
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
    {
        let number = digits.trim_start_matches('0');
        return Some(format!(
            "0.{}",
            if number.is_empty() { "0" } else { number }
        ));
    }
    let valid = !token.is_empty()
        && token.chars().all(|c| c.is_ascii_digit() || c == '.')
        && token.chars().next().is_some_and(|c| c.is_ascii_digit());
    valid.then_some(token)
}

/// Refuses when the evidence names a different MAME build than the one asked
/// for. Evidence is never silently swapped for another version.
pub fn require_matching_version(expected: &str, snapshot: &ProviderSnapshot) -> ProviderResult<()> {
    let (Some(wanted), Some(have)) = (
        normalize_mame_version(expected),
        normalize_mame_version(&snapshot.version),
    ) else {
        return Err(format!(
            "Cannot compare MAME versions ({expected} vs {}); nothing was substituted",
            snapshot.version
        ));
    };
    if wanted != have {
        return Err(format!(
            "Your collection is set to MAME {expected}, but this evidence is for MAME {}. \
             Choose a matching build or import matching data; EmuWiz will not silently \
             substitute a different version.",
            snapshot.version
        ));
    }
    Ok(())
}

pub fn verify(snapshot: &ProviderSnapshot, path: &Path) -> ProviderResult<ProviderIdentityResult> {
    verify_expecting(snapshot, path, None)
}

/// Verifies a ROM/archive file or a collection folder against the snapshot,
/// read-only. When `expected_version` is given the snapshot must be for that
/// MAME build or the request is refused.
///
/// A folder is audited through the bounded, read-only DAT audit (walk depth,
/// entry and file ceilings; truncation is reported, never hidden). Its result
/// describes *recognition*, not launchability: see the details lines for the
/// per-category counts and set completeness.
pub fn verify_expecting(
    snapshot: &ProviderSnapshot,
    path: &Path,
    expected_version: Option<&str>,
) -> ProviderResult<ProviderIdentityResult> {
    let ProviderRecords::DatLike { listxml, .. } = &snapshot.records else {
        return Err("MAME requires listxml evidence".into());
    };
    if let Some(expected) = expected_version {
        require_matching_version(expected, snapshot)?;
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("Choose the folder or file itself, not a link to it".into());
    }
    let collection = metadata.is_dir();
    if !collection && !metadata.is_file() {
        return Err("Choose an arcade folder or one ROM/archive file to verify".into());
    }
    let imported = snapshot.is_local_import();
    let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    file.write_all(listxml.as_bytes())
        .map_err(|e| e.to_string())?;
    let trusted = TrustedRoots::from_paths([path]);
    let outcome = run_dat_audit(
        &DatAuditRequest {
            source_id: snapshot.sha256()?,
            source_display_name: if imported {
                "Imported MAME verification data".into()
            } else {
                "MAME official".into()
            },
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
    let summary = &outcome.report.summary;
    let unrecognised = summary.not_in_dat + summary.no_evidence + summary.filename_only;
    let status = if outcome.truncated || !outcome.unhashed.is_empty() {
        MatchStatus::NeedsRecheck
    } else if collection {
        // A folder holds many games: several distinct names are normal and are
        // not ambiguity. Only a genuinely ambiguous verdict is.
        if ambiguous {
            MatchStatus::Ambiguous
        } else if candidates.is_empty() {
            MatchStatus::NoMatch
        } else if probable || unrecognised > 0 {
            MatchStatus::Probable
        } else {
            MatchStatus::Exact
        }
    } else if ambiguous || candidates.len() > 1 {
        MatchStatus::Ambiguous
    } else if exact {
        MatchStatus::Exact
    } else if probable {
        MatchStatus::Probable
    } else {
        MatchStatus::NoMatch
    };
    let total_candidates = candidates.len();
    candidates.truncate(MAX_LISTED_CANDIDATES);
    let mut details = vec![
        "Status describes ROM-byte identity, not completeness or permission to launch. Shared parent/clone ROMs remain ambiguous.".into(),
        serde_json::to_string(&outcome.sets).map_err(|e| e.to_string())?,
        serde_json::to_string(&outcome.report).map_err(|e| e.to_string())?,
        format!("{} archive(s); {} bytes hashed; {} unreadable items", outcome.archives.len(), outcome.bytes_hashed + outcome.archive_bytes_hashed, outcome.unhashed.len()),
    ];
    if imported {
        details.push("Evidence imported locally; EmuWiz did not download or vouch for it.".into());
    }
    if collection {
        details.push(collection_summary(&outcome, total_candidates));
    }
    Ok(ProviderIdentityResult {
        provider: IdentityProvider::Mame,
        snapshot_sha256: snapshot.sha256()?,
        path: path.into(),
        status,
        detection_class: if matches!(status, MatchStatus::Exact) && !imported {
            DetectionClass::OfficialExact
        } else {
            DetectionClass::Unknown
        },
        origin: if imported {
            MatchOrigin::ImportedMame
        } else {
            MatchOrigin::OfficialMame
        },
        detector_method: Some(if imported {
            "Locally imported MAME listxml (not vouched for) plus bounded DAT audit".into()
        } else {
            "MAME -listxml plus bounded official DAT audit".into()
        }),
        official_game_id: None,
        configured_target_id: None,
        configured_target_evidence: None,
        possible_base_game_id: None,
        cleaned_local_title: None,
        candidates,
        details,
        discovery: None,
    })
}

/// One plain line distinguishing complete, missing, bad and unrecognised items.
fn collection_summary(
    outcome: &crate::dat::sources::audit_run::DatAuditOutcome,
    recognised_games: usize,
) -> String {
    use crate::dat::set::SetState;
    let (mut complete, mut incomplete, mut review, mut bad_meta) = (0, 0, 0, 0);
    let (mut missing_members, mut bad_members) = (0usize, 0usize);
    for set in &outcome.sets {
        match set.state {
            SetState::Complete => complete += 1,
            SetState::Incomplete => incomplete += 1,
            SetState::BadMetadata(_) => bad_meta += 1,
            SetState::NeedsReview(_) => review += 1,
        }
        missing_members += set
            .members_required
            .len()
            .saturating_sub(set.members_verified.len());
        bad_members += set.members_bad.len();
    }
    let summary = &outcome.report.summary;
    format!(
        "Collection: {recognised_games} game(s) recognised; {} item(s) checked: {} exact, {} probable, {} not in the catalogue, {} with no usable evidence. Sets: {complete} complete, {incomplete} incomplete ({missing_members} member(s) missing), {bad_meta} with bad metadata ({bad_members} bad member(s)), {review} need review.",
        summary.total,
        summary.exact + summary.exact_multiple,
        summary.probable + summary.probable_multiple,
        summary.not_in_dat,
        summary.no_evidence + summary.filename_only,
    )
}

#[cfg(test)]
mod tests;
