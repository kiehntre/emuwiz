//! Native signature records: partial MD5s never enter the ordinary DAT index.
#![allow(clippy::items_after_test_module)]
use super::*;
use crate::safe_read::{TrustedRoots, open_bounded_read};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdTitleEntry {
    pub engine: String,
    pub game_id: String,
    pub canonical_id: String,
    pub title: String,
    pub platform: String,
    pub language: String,
    pub variant: String,
    pub source_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureFile {
    pub filename: String,
    pub size: Option<u64>,
    pub hash_key: String,
    pub signature: String,
    /// Preserve provider-specific flags/fields even when unsupported.
    pub fields: BTreeMap<String, String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionRecord {
    pub engine: String,
    pub game_id: String,
    pub title: String,
    pub platform: String,
    pub language: String,
    pub variant: String,
    pub files: Vec<SignatureFile>,
    pub fields: BTreeMap<String, String>,
}

/// Generated from the pinned official dump. This is a convenience index, not
/// an identity decision and not a copy of RomM's bundled fixture.
pub fn id_title_index(version: &str, records: &[DetectionRecord]) -> Vec<IdTitleEntry> {
    let mut entries = records
        .iter()
        .map(|record| IdTitleEntry {
            engine: record.engine.to_ascii_lowercase(),
            game_id: record.game_id.to_ascii_lowercase(),
            canonical_id: format!(
                "{}:{}",
                record.engine.to_ascii_lowercase(),
                record.game_id.to_ascii_lowercase()
            ),
            title: record.title.clone(),
            platform: record.platform.clone(),
            language: record.language.clone(),
            variant: record.variant.clone(),
            source_version: version.to_string(),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.canonical_id
            .cmp(&right.canonical_id)
            .then(left.variant.cmp(&right.variant))
    });
    entries.dedup_by(|left, right| {
        left.canonical_id == right.canonical_id
            && left.variant == right.variant
            && left.title == right.title
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn index_canonicalizes_ids_but_keeps_variant_evidence() {
        let entries = id_title_index(
            "2.8.0",
            &[DetectionRecord {
                engine: "SCUMM".into(),
                game_id: "Monkey2".into(),
                title: "Monkey Island 2".into(),
                platform: "dos".into(),
                language: "en".into(),
                variant: "CD".into(),
                files: Vec::new(),
                fields: BTreeMap::new(),
            }],
        );
        assert_eq!(entries[0].canonical_id, "scumm:monkey2");
        assert_eq!(entries[0].source_version, "2.8.0");
        assert_eq!(entries[0].variant, "CD");
    }

    #[test]
    fn dump_keeps_partial_signature_scope() {
        let dump = b"scummvm (\nversion \"2.8.0\"\n)\ngame (\nsourcefile \"scumm\"\nname \"monkey2\"\ntitle \"Monkey Island 2\"\nplatform \"dos\"\nlanguage \"en\"\nextra \"CD\"\nrom ( name \"MONKEY.000\" size \"4\" md5-2 \"c81d4e2ebf8b504c1f3d3b4b1e7c5c0a\" )\n)\n";
        let (_, records) = parse_dump(dump).unwrap();
        assert_eq!(records[0].files[0].hash_key, "md5-2");
        assert_eq!(records[0].files[0].size, Some(4));
    }

    #[test]
    fn runtime_id_without_exported_record_remains_a_coverage_gap() {
        let ids = BTreeSet::from(["scumm:tentacle".to_string()]);
        let records = vec![DetectionRecord {
            engine: "scumm".into(),
            game_id: "monkey2".into(),
            title: "Monkey Island 2".into(),
            platform: "dos".into(),
            language: "en".into(),
            variant: "CD".into(),
            files: Vec::new(),
            fields: BTreeMap::new(),
        }];
        assert_eq!(
            classify_detection(&ids, &records, 0, None),
            DetectionClass::OfficialDetectionCoverageGap
        );
    }
}

fn possible_base_game_id(target: Option<&str>) -> Option<String> {
    let target = target?.trim();
    let (engine, game) = target.split_once(':')?;
    let base = game.split('-').next().filter(|value| !value.is_empty())?;
    Some(format!(
        "{}:{}",
        engine.to_ascii_lowercase(),
        base.to_ascii_lowercase()
    ))
}

fn tokens(line: &str) -> ProviderResult<Vec<String>> {
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut present = false;
    for ch in line.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
            present = true;
            continue;
        }
        if !quoted && (ch.is_whitespace() || ch == '(' || ch == ')') {
            if present {
                result.push(std::mem::take(&mut token));
                present = false;
            }
        } else {
            token.push(ch);
            present = true;
        }
    }
    if quoted || escaped {
        return Err("Unterminated ScummVM quoted field".into());
    }
    if present {
        result.push(token);
    }
    Ok(result)
}

pub fn parse_dump(bytes: &[u8]) -> ProviderResult<(String, Vec<DetectionRecord>)> {
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err("ScummVM dump exceeds limit".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut version = None;
    let mut header = false;
    let mut game = false;
    let mut fields = BTreeMap::new();
    let mut files = Vec::new();
    let mut records = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.len() > 65536 {
            return Err("ScummVM dump line too long".into());
        }
        if line == "scummvm (" {
            if header || game {
                return Err("Nested dump block".into());
            }
            header = true;
            continue;
        }
        if line == "game (" {
            if header || game {
                return Err("Nested game block".into());
            }
            game = true;
            continue;
        }
        if line == ")" {
            if header {
                header = false;
                continue;
            }
            if !game {
                return Err("Unexpected dump block end".into());
            }
            let field = |key: &str| fields.get(key).cloned().unwrap_or_default();
            let record = DetectionRecord {
                engine: field("sourcefile"),
                game_id: field("name"),
                title: field("title"),
                platform: field("platform"),
                language: field("language"),
                variant: field("extra"),
                files: std::mem::take(&mut files),
                fields: std::mem::take(&mut fields),
            };
            if record.engine.is_empty() || record.game_id.is_empty() {
                return Err("Detection entry lacks engine/game ID".into());
            }
            records.push(record);
            game = false;
            if records.len() > 100_000 {
                return Err("Too many detection records".into());
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if !game && !header {
            continue;
        } // tool's configuration notice, never identity
        let pairs = tokens(line)?;
        if game && line.starts_with("rom (") {
            if !line.ends_with(')') {
                return Err("Unsupported multiline detection signature".into());
            }
            if pairs.len() % 2 != 1 {
                return Err("Malformed signature fields".into());
            }
            let mut raw = BTreeMap::new();
            for pair in pairs[1..].chunks_exact(2) {
                if raw.insert(pair[0].clone(), pair[1].clone()).is_some() {
                    return Err("Duplicate signature field".into());
                }
            }
            let filename = raw
                .get("name")
                .cloned()
                .ok_or("Missing signature filename")?;
            let (hash_key, signature) = raw
                .iter()
                .find(|(k, _)| k.starts_with("md5-"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .unwrap_or_default();
            let size = match raw.get("size").map(String::as_str) {
                None | Some("-1") => None,
                Some(v) => Some(v.parse().map_err(|_| "Invalid signature size")?),
            };
            files.push(SignatureFile {
                filename,
                size,
                hash_key,
                signature,
                fields: raw,
            });
            if files.len() > 128 {
                return Err("Too many signature files".into());
            }
        } else {
            if pairs.len() != 2 {
                return Err("Unsupported detection field shape".into());
            }
            if header && pairs[0] == "version" {
                version = Some(pairs[1].clone());
            }
            if game && fields.insert(pairs[0].clone(), pairs[1].clone()).is_some() {
                return Err("Duplicate detection field".into());
            }
        }
    }
    if game || header || records.is_empty() {
        return Err("Incomplete or empty detection dump".into());
    }
    Ok((version.ok_or("Missing ScummVM dump version")?, records))
}

pub fn capture(executable: &Path) -> ProviderResult<ProviderSnapshot> {
    let executable = executable.canonicalize().map_err(|e| e.to_string())?;
    let before = tool::fingerprint(&executable)?;
    let (bytes, stderr) = tool::run(
        &executable,
        &["--dump-all-detection-entries".into()],
        true,
        MAX_SNAPSHOT_BYTES,
    )?;
    if tool::fingerprint(&executable)? != before {
        return Err("ScummVM changed during capture".into());
    }
    let (version, records) = parse_dump(&bytes)?;
    // Config-path chatter is not snapshot content. Normalized records carry the
    // content hash; the raw source hash excludes the temporary config notice.
    let start = bytes
        .windows(9)
        .position(|w| w == b"scummvm (")
        .ok_or("Missing header")?;
    let mut warnings = vec!["Exported detection tables do not represent every engine's native detector. Unsupported forks/tail/archive signatures require native detection; never treated as plain MD5.".into()];
    if !stderr.trim().is_empty() {
        warnings.push(stderr);
    }
    Ok(ProviderSnapshot {
        provider: IdentityProvider::ScummVm,
        version,
        source_identifier: "official-local:scummvm/--dump-all-detection-entries".into(),
        executable,
        executable_sha256: before,
        source_sha256: sha256(&bytes[start..]),
        parser_version: PARSER_VERSION,
        records: ProviderRecords::NativeDetection(records),
        warnings,
    })
}

/// Only plain, bounded head-MD5 signatures with an explicit file size. Forks,
/// tail signatures, unknown size and ambiguous case-insensitive paths fail closed.
fn signature_matches(
    signature: &SignatureFile,
    root: &Path,
    names: &BTreeMap<String, Vec<std::path::PathBuf>>,
) -> ProviderResult<bool> {
    if Path::new(&signature.filename)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || signature.filename.contains('/')
        || signature.filename.contains('\\')
    {
        return Ok(false);
    }
    let Some(length) = signature
        .hash_key
        .strip_prefix("md5-")
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return Ok(false);
    };
    if length == 0
        || length > 2 * 1024 * 1024
        || signature.signature.len() != 32
        || !signature.signature.bytes().all(|b| b.is_ascii_hexdigit())
        || signature.fields.contains_key("size-rd")
    {
        return Ok(false);
    }
    let Some(size) = signature.size else {
        return Ok(false);
    };
    let Some(paths) = names.get(&signature.filename.to_ascii_lowercase()) else {
        return Ok(false);
    };
    if paths.len() != 1 {
        return Err("Case-insensitive signature filename is ambiguous".into());
    }
    let safe = open_bounded_read(&paths[0], &TrustedRoots::from_paths([root]))
        .map_err(|e| format!("Signature read refused: {e:?}"))?;
    if safe.len() != size {
        return Ok(false);
    }
    let mut file = safe.into_file();
    let before = file.metadata().map_err(|e| e.to_string())?;
    let mut bytes = vec![0u8; length.min(size) as usize];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("Signature file changed during read".into());
    }
    let digest: String = Md5::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(digest.eq_ignore_ascii_case(&signature.signature))
}

pub fn matching_records(
    records: &[DetectionRecord],
    root: &Path,
) -> ProviderResult<Vec<DetectionRecord>> {
    if !root.is_absolute() || !root.is_dir() {
        return Err("Choose an absolute existing game directory".into());
    }
    let mut names: BTreeMap<String, Vec<std::path::PathBuf>> = BTreeMap::new();
    for (i, entry) in fs::read_dir(root).map_err(|e| e.to_string())?.enumerate() {
        if i >= 2048 {
            return Err("Game folder exceeds 2048 immediate entries".into());
        }
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err("Native POC refuses symbolic entries".into());
        }
        if ty.is_file() {
            names
                .entry(entry.file_name().to_string_lossy().to_ascii_lowercase())
                .or_default()
                .push(entry.path());
        }
    }
    let mut matches = Vec::new();
    let mut attempted = 0;
    for record in records {
        // Mac forks and archive pseudo-paths are deliberately not reimplemented.
        if record.files.is_empty()
            || record.platform == "macintosh"
            || record.platform == "mac"
            || !record
                .files
                .iter()
                .all(|s| names.contains_key(&s.filename.to_ascii_lowercase()))
        {
            continue;
        }
        attempted += 1;
        if attempted > 512 {
            return Err("Too many candidate signature rows for a bounded POC verification".into());
        }
        let mut complete = true;
        for sig in &record.files {
            if !signature_matches(sig, root, &names)? {
                complete = false;
                break;
            }
        }
        if complete && !matches.contains(record) {
            matches.push(record.clone());
        }
    }
    Ok(matches)
}

/// Native confirmation is mandatory, including explicit rejection of fallback
/// unknown-variant output even when ScummVM prints a plausible game ID afterward.
pub fn native_ids(output: &str) -> BTreeSet<String> {
    if output.to_ascii_lowercase().contains("unknown game variant")
        || output.to_ascii_lowercase().contains("unknown variant")
    {
        return BTreeSet::new();
    }
    output
        .lines()
        .filter_map(|line| {
            let first = line.split_whitespace().next()?;
            crate::scummvm_detection::is_valid_scummvm_game_id(first).then(|| first.to_string())
        })
        .collect()
}

fn classify_detection(
    ids: &BTreeSet<String>,
    records: &[DetectionRecord],
    matched_count: usize,
    discovery_status: Option<MatchStatus>,
) -> DetectionClass {
    if matched_count > 0 {
        DetectionClass::OfficialExact
    } else if !ids.is_empty()
        && records
            .iter()
            .any(|record| ids.contains(&format!("{}:{}", record.engine, record.game_id)))
    {
        DetectionClass::OfficialFallback
    } else if !ids.is_empty() {
        DetectionClass::OfficialDetectionCoverageGap
    } else if discovery_status == Some(MatchStatus::Probable) {
        DetectionClass::EmuwizDerivedProbable
    } else {
        DetectionClass::Unknown
    }
}

pub fn verify(snapshot: &ProviderSnapshot, root: &Path) -> ProviderResult<ProviderIdentityResult> {
    let ProviderRecords::NativeDetection(records) = &snapshot.records else {
        return Err("ScummVM requires native records".into());
    };
    if tool::fingerprint(&snapshot.executable)? != snapshot.executable_sha256 {
        return Err("Needs re-check: ScummVM executable differs from the reviewed snapshot; check and preview an update".into());
    }
    let matches = matching_records(records, root)?;
    let configured_target_id = discovery::inspect_scummvm_target(root)?;
    let (output, stderr) = tool::run(
        &snapshot.executable,
        &[
            format!("--path={}", root.display()).into(),
            "--detect".into(),
        ],
        true,
        256 * 1024,
    )?;
    let native = format!("{}\n{stderr}", String::from_utf8_lossy(&output));
    let ids = native_ids(&native);
    let matches: Vec<_> = matches
        .into_iter()
        .filter(|r| ids.contains(&format!("{}:{}", r.engine, r.game_id)))
        .collect();
    let discovery = if matches.is_empty() {
        Some(discovery::inspect(root)?)
    } else {
        None
    };
    let status = match matches.len() {
        1 => MatchStatus::Exact,
        n if n > 1 => MatchStatus::Ambiguous,
        _ => discovery
            .as_ref()
            .map_or(MatchStatus::NoMatch, |d| d.status),
    };
    let detection_class = classify_detection(
        &ids,
        records,
        matches.len(),
        discovery.as_ref().map(|value| value.status),
    );
    let origin = if matches.is_empty() {
        MatchOrigin::EmuwizDiscovery
    } else {
        MatchOrigin::OfficialScummVm
    };
    let candidates = matches
        .iter()
        .map(|r| {
            format!(
                "{}:{} — {} / {} / {} / {}",
                r.engine, r.game_id, r.title, r.platform, r.language, r.variant
            )
        })
        .collect();
    let official_game_id = ids.iter().next().cloned();
    Ok(ProviderIdentityResult {
        provider: IdentityProvider::ScummVm,
        snapshot_sha256: snapshot.sha256()?,
        path: root.into(),
        status,
        detection_class,
        origin,
        detector_method: Some(
            "ScummVM runtime --detect plus pinned exported detection records".into(),
        ),
        official_game_id,
        configured_target_id: configured_target_id.clone(),
        configured_target_evidence: possible_base_game_id(configured_target_id.as_deref())
            .map(|_| "target-name heuristic".into()),
        possible_base_game_id: possible_base_game_id(configured_target_id.as_deref()),
        cleaned_local_title: root
            .file_name()
            .and_then(|name| name.to_str())
            .map(discovery::cleaned_local_title),
        candidates,
        details: vec![
            if matches.is_empty() {
                "Official exact match not established; discovery below is non-authoritative. Unsupported native signature forms are not a claim the game is unsupported.".into()
            } else {
                "Official ScummVM match: exported signatures and the pinned native detector agree."
                    .into()
            },
            serde_json::to_string(&matches).map_err(|e| e.to_string())?,
            native,
        ],
        discovery,
    })
}
