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
/// Machine ceiling for the audit's parse of a full catalogue (today's MAME lists ~47k).
const MAX_LISTXML_MACHINES: usize = 500_000;

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
    // The full catalogue is hundreds of megabytes: stream it into a file in an
    // EmuWiz-owned scratch directory (removed on every exit path) rather than
    // collecting it as one in-memory response.
    let spool = tempfile::tempdir().map_err(|e| e.to_string())?;
    let spool_path = spool.path().join("listxml.xml");
    let (streamed_sha256, written, stderr) = {
        let mut out =
            std::io::BufWriter::new(std::fs::File::create(&spool_path).map_err(|e| e.to_string())?);
        let result = tool::run_to_writer(&executable, &args, MAX_LISTXML_BYTES, &mut out)
            .map_err(explain_capture_error)?;
        out.flush().map_err(|e| e.to_string())?;
        result
    };
    if tool::fingerprint(&executable)? != before {
        return Err("MAME changed during capture".into());
    }
    let bytes = read_bounded(&spool_path, written)?;
    drop(spool);
    if sha256(&bytes) != streamed_sha256 {
        return Err("MAME's output changed while it was being stored".into());
    }
    let scan = scan_listxml(&bytes)?;
    let source_identifier = "official-local:mame/-listxml".to_string();
    let mut warnings = Vec::new();
    warnings.push(
        "Full machine catalogue from the installed MAME; software lists are not included".into(),
    );
    if !stderr.trim().is_empty() {
        warnings.push(stderr);
    }
    Ok(ProviderSnapshot {
        provider: IdentityProvider::Mame,
        version: scan.build.clone(),
        source_identifier: source_identifier.clone(),
        executable,
        executable_sha256: before,
        source_sha256: streamed_sha256,
        parser_version: PARSER_VERSION,
        records: ProviderRecords::DatLike {
            catalogue: scan.catalogue(&source_identifier),
            machine_count: scan.machines,
            listxml: String::from_utf8(bytes).map_err(|_| "MAME listxml is not UTF-8")?,
        },
        warnings,
    })
}

/// Reads a whole file once into a single buffer, never past the listxml
/// ceiling (the file is also checked against the length it was written with).
fn read_bounded(path: &Path, expected: u64) -> ProviderResult<Vec<u8>> {
    use std::io::Read;
    if expected > MAX_LISTXML_BYTES {
        return Err(too_large_message());
    }
    let mut bytes = Vec::with_capacity(expected as usize);
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_LISTXML_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_LISTXML_BYTES {
        return Err(too_large_message());
    }
    Ok(bytes)
}

fn too_large_message() -> String {
    format!(
        "That listxml is larger than the {} MiB import limit",
        MAX_LISTXML_BYTES / (1024 * 1024)
    )
}

/// What a constant-memory pass over a listxml learns: the declared build, the
/// machine count, and proof that the document is complete and well formed. No
/// machine is retained; per-machine parsing happens only in the bounded DAT
/// audit, when a collection is actually verified.
struct ListxmlScan {
    build: String,
    machines: usize,
}

impl ListxmlScan {
    /// Header-only catalogue (no machines): enough to name the source and its
    /// build without holding every machine in memory.
    fn catalogue(&self, source: &str) -> crate::dat::model::ParsedDat {
        use crate::dat::model::{DatEcosystem, DatFormat, DatPackingPolicy, DatSource, ParsedDat};
        ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: source.into(),
                name: Some("MAME -listxml".into()),
                description: None,
                version: Some(self.build.clone()),
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: self.machines,
                rom_count: 0,
                parse_warnings: Vec::new(),
                packing_policy: DatPackingPolicy::Standard,
            },
            games: Vec::new(),
        }
    }
}

fn scan_listxml(bytes: &[u8]) -> ProviderResult<ListxmlScan> {
    use quick_xml::{
        Reader,
        events::{BytesStart, Event},
    };
    let mut reader = Reader::from_reader(bytes);
    let mut buf = Vec::new();
    let (mut depth, mut machines) = (0_usize, 0_usize);
    let mut build = None;
    let mut root_closed = false;
    let mut element = |e: &BytesStart, depth: usize, root_closed: bool| -> ProviderResult<()> {
        if depth == 0 {
            if root_closed || !e.name().as_ref().eq_ignore_ascii_case(b"mame") {
                return Err("That file is not a MAME listxml (no <mame> root)".into());
            }
            for attribute in e.attributes().flatten() {
                if attribute.key.as_ref() == b"build" {
                    build = attribute
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .ok()
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty());
                }
            }
        } else if depth == 1
            && e.name().as_ref() == b"machine"
            && e.attributes().flatten().any(|a| a.key.as_ref() == b"name")
        {
            machines += 1;
        }
        Ok(())
    };
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("That is not a complete MAME listxml: {e}"))?
        {
            Event::Eof => break,
            Event::Start(e) => {
                element(&e, depth, root_closed)?;
                depth += 1;
            }
            Event::Empty(e) => {
                element(&e, depth, root_closed)?;
                if depth == 0 {
                    root_closed = true;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    root_closed = true;
                }
            }
            _ => {}
        }
        buf.clear();
    }
    if depth != 0 || !root_closed {
        return Err("That listxml looks incomplete (the document is not closed)".into());
    }
    Ok(ListxmlScan {
        build: build.ok_or("MAME listxml does not report a build version")?,
        machines,
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
            MAX_LISTXML_BYTES / (1024 * 1024)
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
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Choose a regular MAME listxml file".into());
    }
    if metadata.len() > MAX_LISTXML_BYTES {
        return Err(too_large_message());
    }
    let bytes = read_bounded(path, metadata.len())?;
    // Scanned from the bytes just read, so what is checked is exactly what is stored.
    let scan = scan_listxml(&bytes)?;
    let source_hash = sha256(&bytes);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let source_identifier = format!("{LOCAL_IMPORT_PREFIX}{name}");
    Ok(ProviderSnapshot {
        provider: IdentityProvider::Mame,
        version: scan.build.clone(),
        source_identifier: source_identifier.clone(),
        executable: PathBuf::from(format!("{LOCAL_IMPORT_PREFIX}mame-listxml")),
        executable_sha256: source_hash.clone(),
        source_sha256: source_hash,
        parser_version: PARSER_VERSION,
        records: ProviderRecords::DatLike {
            catalogue: scan.catalogue(&source_identifier),
            machine_count: scan.machines,
            listxml: String::from_utf8(bytes).map_err(|_| "MAME listxml is not UTF-8")?,
        },
        warnings: vec!["Imported locally; EmuWiz did not download or vouch for the source.".into()],
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
            // The audit's own parser keeps the catalogue it reads, so its file
            // ceiling is the real memory bound: raised to the listxml ceiling here.
            limits: DatLimits::builder()
                .max_file_size(MAX_LISTXML_BYTES)
                .max_entries(MAX_LISTXML_MACHINES)
                .build(),
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
