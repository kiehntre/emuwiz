//! Thin local adapters. Retains parser evidence, uses the shared importer walk
//! and bounded reader, and feeds the same typed plan seam as future batches.
use super::super::cheat_ir::{CheatIssue, CheatOperation};
use super::super::user_cheat_import as import;
use super::*;
use std::fs;

pub(super) fn validate(l: &CheatPackLimits) -> Result<(), UserCheatImportError> {
    import::validate_limits(&l.source)?;
    let d = CheatPackLimits::default();
    if l.source.max_file_bytes > d.source.max_file_bytes
        || l.source.max_total_bytes > d.source.max_total_bytes
        || l.source.max_files_visited > d.source.max_files_visited
        || l.source.max_depth > d.source.max_depth
        || l.source.max_cheats_per_file > d.source.max_cheats_per_file
        || l.source.max_warnings > d.source.max_warnings
        || l.max_observations == 0
        || l.max_observations > d.max_observations
        || l.max_line_bytes == 0
        || l.max_line_bytes > d.max_line_bytes
        || l.max_lines_per_file == 0
        || l.max_lines_per_file > d.max_lines_per_file
        || l.max_code_lines == 0
        || l.max_code_lines > d.max_code_lines
        || l.max_catalogue_games == 0
        || l.max_catalogue_games > d.max_catalogue_games
        || l.max_matches_per_file == 0
        || l.max_matches_per_file > d.max_matches_per_file
    {
        return Err(UserCheatImportError::InvalidLimits(
            "pack limits must be positive and within hard ceilings".into(),
        ));
    }
    Ok(())
}
fn failure(code: impl Into<String>, detail: impl Into<String>) -> CheatPackDiagnostic {
    CheatPackDiagnostic::ParseFailure {
        code: code.into(),
        detail: detail.into(),
    }
}
fn bounded<T>(mut rows: Vec<T>, limit: usize) -> (Vec<T>, bool) {
    let truncated = rows.len() > limit;
    rows.truncate(limit);
    (rows, truncated)
}
pub(super) fn platform(a: &CheatPackAssociation) -> CheatPlatform {
    match a
        .platform
        .as_deref()
        .and_then(crate::canonical_platform_for_alias)
    {
        Some("PlayStation 2") => CheatPlatform::Ps2,
        Some("GameCube") => CheatPlatform::GameCube,
        Some("Wii") => CheatPlatform::Wii,
        Some("Nintendo DS") => CheatPlatform::NintendoDs,
        Some("Nintendo 64") => CheatPlatform::Nintendo64,
        _ => CheatPlatform::Other(a.platform.clone().unwrap_or_default()),
    }
}
fn observation(
    file: &CheatPackFile,
    file_index: usize,
    title: String,
    raw: String,
    format: CheatSourceFormat,
    operations: Vec<CheatOperation>,
) -> CheatPackObservation {
    let provenance = file
        .provenance
        .clone()
        .unwrap_or_else(|| UserCheatProvenance {
            origin: import::UserCheatSourceOrigin::UserSupplied,
            original_path: file.path.clone(),
            original_filename: String::new(),
            source_sha256: String::new(),
        });
    let issues = if operations
        .iter()
        .any(|o| matches!(o, CheatOperation::UnsupportedRaw { .. }))
    {
        vec![CheatIssue::RawPreserved]
    } else {
        Vec::new()
    };
    CheatPackObservation {
        file_index,
        source_index: None,
        source_index_conflict: false,
        provenance: provenance.clone(),
        source_group: None,
        mirror_of: None,
        association: file.association.clone(),
        document: CheatDocument {
            source_evidence: Vec::new(),
            title,
            platform: platform(&file.association),
            source_format: format,
            operations,
            issues,
            provenance: vec![provenance.source_sha256],
        },
        raw_code: raw,
        code_truncated: false,
        full_code_digest: None,
        execution_fields: BTreeMap::new(),
        engine: None,
        source_enabled_by_default: false,
        applicability: CheatApplicabilityState::NeedsReview,
        assessment: None,
        native_cht: None,
        diagnostics: Vec::new(),
        diagnostics_truncated: false,
        logical_key: String::new(),
        action: CheatPackAction::WouldRequireReview,
    }
}
fn adapt(
    file: &mut CheatPackFile,
    bytes: &[u8],
    file_index: usize,
    limits: &CheatPackLimits,
) -> Vec<CheatPackObservation> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            file.state = CheatPackFileState::Malformed;
            file.diagnostics.push(failure(
                "unsupported_encoding",
                format!("invalid UTF-8 at {}", error.valid_up_to()),
            ));
            return Vec::new();
        }
    };
    if text.lines().count() > limits.max_lines_per_file
        || text.lines().any(|line| line.len() > limits.max_line_bytes)
    {
        file.state = CheatPackFileState::LimitRejected;
        file.diagnostics.push(CheatPackDiagnostic::Limit {
            detail: "line length/count exceeds pack bounds".into(),
        });
        return Vec::new();
    }
    let mut result = Vec::new();
    match file.format {
        Some(UserCheatFormat::RetroarchCht) => {
            let doc = match super::super::cht_document::parse_cht_bytes(bytes) {
                Ok(doc) => doc,
                Err(error) => {
                    file.state = CheatPackFileState::Malformed;
                    file.diagnostics
                        .push(failure(format!("{:?}", error.kind), error.detail));
                    return result;
                }
            };
            let projected = doc.reconciliation_entries(
                "unassigned",
                false,
                platform(&file.association),
                "local",
                &file.path.display().to_string(),
            );
            file.source_metadata = doc.global_fields.into_iter().collect();
            file.source_comments = doc.preserved_comments;
            file.diagnostics.extend(
                doc.warnings
                    .into_iter()
                    .map(CheatPackDiagnostic::ChtDocument),
            );
            for e in doc
                .entries
                .into_iter()
                .take(limits.source.max_cheats_per_file + 1)
            {
                let raw = e.code.clone().unwrap_or_default();
                let op = CheatOperation::UnsupportedRaw {
                    source_format: CheatSourceFormat::RetroArch,
                    raw: raw.clone(),
                    reason: "opaque native RetroArch code; engine semantics not inferred".into(),
                };
                // Missing descriptions are not invented as source evidence.
                let mut o = observation(
                    file,
                    file_index,
                    e.description.clone().unwrap_or_default(),
                    raw,
                    CheatSourceFormat::RetroArch,
                    vec![op],
                );
                // The shared hardened parser already classifies a repeated field with
                // a different value; an identical repeat is a benign duplicate.
                o.source_index_conflict = e.warnings.iter().any(|w| {
                    w.kind == super::super::cht_document::ChtEntryWarningKind::ConflictingDuplicate
                });
                if let Some(entry) = projected
                    .iter()
                    .find(|entry| entry.source_index == Some(e.index))
                {
                    o.document = entry.document.clone();
                    o.execution_fields = entry.applicability.metadata.clone();
                }
                o.native_cht = Some(e.clone());
                o.source_index = Some(e.index);
                o.source_enabled_by_default = e.enabled_by_default;
                o.engine = e
                    .extra_fields
                    .iter()
                    .find(|(key, _)| key == "handler")
                    .map(|(_, value)| value.clone());
                // An entry blocked *only* by a conflicting repeat parses fine but is
                // ambiguous: review-only, not malformed. Anything else blocking is.
                let only_conflict = e.code.as_deref().is_some_and(|code| !code.is_empty())
                    && e.blocking_warnings().all(|w| {
                        w.kind == super::super::cht_document::ChtEntryWarningKind::ConflictingDuplicate
                    });
                o.applicability = if !e.is_selectable() && !only_conflict {
                    CheatApplicabilityState::Malformed
                } else if e.warnings.is_empty() {
                    CheatApplicabilityState::Ready
                } else {
                    CheatApplicabilityState::NeedsReview
                };
                o.diagnostics = e
                    .warnings
                    .into_iter()
                    .map(CheatPackDiagnostic::ChtEntry)
                    .collect();
                result.push(o);
            }
        }
        Some(UserCheatFormat::Pcsx2Pnach) => {
            let mut title = file.association.title.clone().unwrap_or_default();
            let mut raw = Vec::new();
            let mut operations = Vec::new();
            let mut malformed = Vec::new();
            for (line_no, line) in text.lines().enumerate() {
                if let Some(t) = line.trim().strip_prefix("gametitle=") {
                    if title.is_empty() {
                        title = t.trim().into();
                    }
                }
                if line.trim().starts_with("patch=") {
                    match super::super::pcsx2_pnach::PnachPatchLine::parse(line.trim()) {
                        Ok(_) => {
                            raw.push(line.to_string());
                            operations.push(cheat_ir::pnach_line_to_ir(line.trim()));
                        }
                        Err(e) => malformed.push(failure(
                            "invalid_pnach_line",
                            format!("line {}: {}", line_no + 1, e),
                        )),
                    }
                }
            }
            if !raw.is_empty() {
                let mut o = observation(
                    file,
                    file_index,
                    title,
                    raw.join("\n"),
                    CheatSourceFormat::Pnach,
                    operations,
                );
                // Execution mode and CPU remain in raw code if IR cannot prove
                // them. No line from a different section is silently enabled.
                o.applicability = if malformed.is_empty() {
                    CheatApplicabilityState::Ready
                } else {
                    CheatApplicabilityState::NeedsReview
                };
                let execution: Vec<_> = raw
                    .iter()
                    .filter_map(|line| line.trim().strip_prefix("patch="))
                    .map(|line| {
                        line.split(',')
                            .take(2)
                            .map(str::trim)
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .collect();
                o.execution_fields
                    .insert("execution_modes_cpus".into(), execution.join("\n"));
                o.diagnostics = malformed;
                result.push(o);
            } else {
                file.state = CheatPackFileState::Malformed;
                file.diagnostics.extend(malformed);
                file.diagnostics
                    .push(failure("missing_code", "no valid PNACH patches"));
            }
        }
        Some(UserCheatFormat::DolphinGameSettingsIni) => {
            let doc = super::super::gecko_document::parse_dolphin_ini(text);
            file.diagnostics.extend(
                doc.warnings
                    .into_iter()
                    .map(|w| failure(format!("{:?}", w.kind), w.detail)),
            );
            for (format, codes) in [
                (CheatSourceFormat::Gecko, doc.gecko_codes),
                (
                    CheatSourceFormat::DolphinActionReplay,
                    doc.action_replay_codes,
                ),
            ] {
                for e in codes {
                    let operations = e
                        .lines
                        .iter()
                        .map(|line| cheat_ir::dolphin_line_to_ir(line, format.clone()))
                        .collect();
                    let mut o = observation(
                        file,
                        file_index,
                        e.name.clone(),
                        e.lines.join("\n"),
                        format.clone(),
                        operations,
                    );
                    o.document.provenance.extend(e.notes.iter().cloned());
                    o.source_index = e.source_line;
                    o.source_enabled_by_default = e.enabled_by_default;
                    o.applicability = if e.is_selectable() {
                        CheatApplicabilityState::Ready
                    } else {
                        CheatApplicabilityState::Malformed
                    };
                    o.diagnostics = e
                        .warnings
                        .into_iter()
                        .map(|w| failure(format!("{:?}", w.kind), w.detail))
                        .collect();
                    result.push(o);
                }
            }
            if result.is_empty() {
                file.state = CheatPackFileState::Unsupported;
                file.diagnostics.push(failure(
                    "not_dolphin_cheats",
                    "INI declares no supported cheat section",
                ));
            }
        }
        Some(UserCheatFormat::XeniaPatchToml) => {
            let doc = super::super::xenia_patch_document::parse_xenia_patch_toml(text);
            file.diagnostics.extend(
                doc.warnings
                    .iter()
                    .map(|w| failure(format!("{:?}", w.kind), w.detail.clone())),
            );
            if doc.is_fatally_malformed() {
                file.state = CheatPackFileState::Malformed;
                return result;
            }
            for (i, e) in doc.patches.iter().enumerate() {
                let raw = format!("{:?}", e.writes);
                let format = CheatSourceFormat::Other("xenia_patch_toml".into());
                let mut o = observation(
                    file,
                    file_index,
                    e.name.clone(),
                    raw.clone(),
                    format.clone(),
                    vec![CheatOperation::UnsupportedRaw {
                        source_format: format,
                        raw,
                        reason: "native Xenia write types retained, not converted".into(),
                    }],
                );
                o.source_index = u32::try_from(i).ok();
                o.source_enabled_by_default = e.enabled_by_default;
                o.execution_fields
                    .insert("hashes".into(), doc.hashes.join("\n"));
                o.execution_fields
                    .insert("media_ids".into(), doc.media_ids.join("\n"));
                o.document.provenance.push(format!("author: {}", e.author));
                o.applicability = if e.is_selectable() {
                    // Current main catalogue facts cannot prove Xenia module hash binding.
                    CheatApplicabilityState::NeedsReview
                } else {
                    CheatApplicabilityState::Malformed
                };
                o.diagnostics = e
                    .warnings
                    .iter()
                    .map(|w| failure(format!("{:?}", w.kind), w.detail.clone()))
                    .collect();
                result.push(o);
            }
            if result.is_empty() {
                file.state = CheatPackFileState::Malformed;
                file.diagnostics.push(failure(
                    "missing_code",
                    "Xenia document contains no patches",
                ));
            }
        }
        None => {
            file.state = CheatPackFileState::Unsupported;
            file.diagnostics.push(failure(
                "unsupported_format",
                "source format not supported by local cheat import",
            ));
        }
    }
    result
}
fn inferred_association(
    path: &Path,
    format: Option<UserCheatFormat>,
    bytes: &[u8],
) -> CheatPackAssociation {
    let filename = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned);
    let mut a = CheatPackAssociation {
        filename,
        platform: import::infer_platform_hint(path),
        ..Default::default()
    };
    match format {
        Some(UserCheatFormat::Pcsx2Pnach) => {
            a.platform = Some("PlayStation 2".into());
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let (serial, crc) = super::super::pcsx2::parse_patch_identity(stem);
            for (kind, value) in [
                (IdentityKind::Ps2Serial, serial),
                (IdentityKind::Pcsx2ExecutableCrc, crc),
            ] {
                if let Some(value) = value {
                    a.identities
                        .push(CheatPackIdentityRequirement { kind, value });
                }
            }
            if let Ok(text) = std::str::from_utf8(bytes) {
                a.title = text.lines().find_map(|line| {
                    line.trim()
                        .strip_prefix("gametitle=")
                        .map(|t| t.trim().into())
                });
            }
        }
        Some(UserCheatFormat::DolphinGameSettingsIni) => {
            let (id, revision, _region) = super::super::dolphin_local::parse_game_identity(
                path.file_stem().unwrap_or_default(),
            );
            if let Some(value) = id {
                a.identities.push(CheatPackIdentityRequirement {
                    kind: IdentityKind::DolphinGameId,
                    value,
                });
            }
            a.revision = revision.map(|v| v.to_string());
        }
        Some(UserCheatFormat::XeniaPatchToml) => {
            if let Ok(text) = std::str::from_utf8(bytes) {
                let doc = super::super::xenia_patch_document::parse_xenia_patch_toml(text);
                a.title = (!doc.title_name.is_empty()).then_some(doc.title_name);
                if !doc.title_id.is_empty() {
                    a.identities.push(CheatPackIdentityRequirement {
                        kind: IdentityKind::XexTitleId,
                        value: doc.title_id,
                    });
                }
                a.platform = Some("Xbox 360".into());
            }
        }
        _ => {}
    }
    a
}

pub(super) fn preview(
    root: &Path,
    catalogue: &[CheatPackCatalogueGame],
    associations: &BTreeMap<PathBuf, CheatPackAssociation>,
    existing: &BTreeSet<String>,
    limits: &CheatPackLimits,
) -> Result<CheatPackPreview, UserCheatImportError> {
    validate(limits)?;
    import::validate_path_length(root)?;
    if catalogue.len() > limits.max_catalogue_games {
        return Err(UserCheatImportError::InvalidLimits(
            "catalogue game bound exceeded".into(),
        ));
    }
    if root
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(UserCheatImportError::InvalidLimits(
            "source root must not contain parent traversal".into(),
        ));
    }
    // No unproven fallback silently follows Windows reparse points or parent
    // races. The pure planning seam is portable; local source reads need an
    // equivalent descriptor-bound reader before other platforms are enabled.
    if !cfg!(target_os = "linux") {
        return Err(UserCheatImportError::Io{path:root.into(),message:"confined local pack reading is currently supported on Linux; use inspected evidence for pure planning".into()});
    }
    let valid_association = |a: &CheatPackAssociation| {
        a.identities.len() <= 32
            && [&a.title, &a.filename, &a.platform, &a.region, &a.revision]
                .into_iter()
                .flatten()
                .all(|s| s.len() <= 4096)
            && a.identities.iter().all(|r| r.value.len() <= 4096)
    };
    if associations.len() > limits.source.max_files_visited
        || associations.iter().any(|(p, a)| {
            p.is_absolute()
                || p.as_os_str().len() > 4096
                || p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                || !valid_association(a)
        })
        || catalogue.iter().any(|g| {
            g.facts.len() > 64
                || g.game.title.len() > 4096
                || g.game.game_id.len() > 4096
                || g.facts
                    .iter()
                    .any(|f| f.value.as_ref().is_some_and(|v| v.len() > 4096))
        })
    {
        return Err(UserCheatImportError::InvalidLimits(
            "catalogue/association evidence exceeds bounds or contains invalid paths".into(),
        ));
    }
    let metadata = fs::symlink_metadata(root).map_err(|e| UserCheatImportError::Io {
        path: root.into(),
        message: e.to_string(),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(UserCheatImportError::SourceIsSymlink(root.into()));
    }
    let mut report = import::UserCheatImportReport::new(root);
    let mut paths = Vec::new();
    if metadata.is_dir() {
        import::collect_files(root, 0, &limits.source, &mut report, &mut paths)?;
    } else if metadata.is_file() {
        paths.push(root.to_path_buf());
    } else {
        return Err(UserCheatImportError::SourceIsNotRegularFile(root.into()));
    }
    paths.sort();
    let index = CatalogueIndex::new(catalogue, limits.max_matches_per_file);
    let mut p = CheatPackPreview {
        root: root.into(),
        limits: limits.clone(),
        files: Vec::new(),
        observations: Vec::new(),
        games: Vec::new(),
        logical_cheats: Vec::new(),
        diagnostics: report
            .diagnostics
            .into_iter()
            .map(CheatPackDiagnostic::Source)
            .collect(),
        totals: Default::default(),
        bytes_read: 0,
        complete: !report.truncated,
    };
    let mut matches_retained = 0;
    for path in paths {
        let relative = if metadata.is_dir() {
            path.strip_prefix(root).unwrap_or(&path).to_path_buf()
        } else {
            path.file_name().map(PathBuf::from).unwrap_or_default()
        };
        let format = import::format_for_path(&path);
        let mut f = CheatPackFile {
            path: relative.clone(),
            format,
            state: CheatPackFileState::Accepted,
            provenance: None,
            association: Default::default(),
            game_key: String::new(),
            matches: Vec::new(),
            match_strength: CheatPackMatchStrength::Unmatched,
            matches_truncated: false,
            observation_indices: Vec::new(),
            diagnostics: Vec::new(),
            diagnostics_truncated: false,
            source_metadata: BTreeMap::new(),
            source_comments: Vec::new(),
        };
        let meta = fs::symlink_metadata(&path);
        match meta {
            Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                f.state = CheatPackFileState::Unreadable;
                f.diagnostics.push(failure(
                    "source_changed",
                    "source is no longer a regular file",
                ));
            }
            Ok(meta) if import::is_executable_or_script(&path, &meta) => {
                f.state = CheatPackFileState::Unsupported;
                f.diagnostics.push(failure(
                    "executable_refused",
                    "scripts/executables are not parsed",
                ));
            }
            Ok(meta) if meta.len() > limits.source.max_file_bytes => {
                f.state = CheatPackFileState::LimitRejected;
                f.diagnostics.push(CheatPackDiagnostic::Limit {
                    detail: "file size limit exceeded".into(),
                });
            }
            Ok(meta) => {
                if p.bytes_read.saturating_add(meta.len()) > limits.source.max_total_bytes {
                    f.state = CheatPackFileState::LimitRejected;
                    f.diagnostics.push(CheatPackDiagnostic::Limit {
                        detail: "total input bytes limit reached".into(),
                    });
                    p.complete = false;
                } else {
                    match import::read_bounded_with_counter(
                        &path,
                        meta.len(),
                        limits
                            .source
                            .max_file_bytes
                            .min(limits.source.max_total_bytes.saturating_sub(p.bytes_read)),
                        &mut p.bytes_read,
                    ) {
                        Ok(bytes) => {
                            f.provenance = Some(UserCheatProvenance {
                                origin: import::UserCheatSourceOrigin::UserSupplied,
                                original_path: path.clone(),
                                original_filename: path
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                source_sha256: Sha256::digest(&bytes)
                                    .iter()
                                    .map(|b| format!("{b:02x}"))
                                    .collect(),
                            });
                            f.association = associations
                                .get(&relative)
                                .cloned()
                                .unwrap_or_else(|| inferred_association(&relative, format, &bytes));
                            (f.game_key, f.match_strength, f.matches, f.matches_truncated) = index
                                .associate(
                                    &f.association,
                                    limits.max_observations.saturating_sub(matches_retained),
                                );
                            matches_retained += f.matches.len();
                            if f.matches_truncated {
                                p.complete = false;
                                f.diagnostics.push(CheatPackDiagnostic::Limit{detail:"catalogue candidate retention bound reached; association remains ambiguous".into()});
                            }

                            let mut rows = adapt(&mut f, &bytes, p.files.len(), limits);
                            if rows.len() > limits.source.max_cheats_per_file
                                || p.observations.len() + rows.len() > limits.max_observations
                            {
                                p.complete = false;
                                f.diagnostics.push(CheatPackDiagnostic::Limit {
                                    detail: "observation retention bound reached".into(),
                                });
                                rows.truncate(limits.source.max_cheats_per_file.min(
                                    limits.max_observations.saturating_sub(p.observations.len()),
                                ));
                            }
                            for mut o in rows {
                                if o.raw_code.len()
                                    > super::super::cht_document::MAX_CHT_FIELD_BYTES
                                    || o.raw_code.lines().count() > limits.max_code_lines
                                {
                                    o.applicability = CheatApplicabilityState::Malformed;
                                    let full_bytes = o.raw_code.len();
                                    o.full_code_digest =
                                        Some(digest("full-source-code", &o.raw_code));
                                    let mut end = super::super::cht_document::MAX_CHT_FIELD_BYTES
                                        .min(full_bytes);
                                    while !o.raw_code.is_char_boundary(end) {
                                        end -= 1;
                                    }
                                    o.raw_code.truncate(end);
                                    o.document.operations.clear();
                                    o.code_truncated = true;
                                    p.complete = false;
                                    o.diagnostics.push(CheatPackDiagnostic::Limit {
                                        detail: format!("code length/line count bound exceeded ({full_bytes} original bytes); only a code sample retained; operations withheld"),
                                    });
                                }
                                let game = f
                                    .matches
                                    .first()
                                    .and_then(|m| {
                                        catalogue.iter().find(|g| g.game.game_id == m.game_id)
                                    })
                                    .map(selected_game)
                                    .unwrap_or_default();
                                let report = assess_observation(&o, game, None);
                                o.applicability = report.state;
                                o.assessment = Some(report);
                                (o.diagnostics, o.diagnostics_truncated) =
                                    bounded(o.diagnostics, limits.source.max_warnings);
                                p.complete &= !o.diagnostics_truncated;
                                f.observation_indices.push(p.observations.len());
                                p.observations.push(o);
                            }
                        }
                        Err(error) => {
                            f.state = CheatPackFileState::Unreadable;
                            f.diagnostics.push(failure("read_error", error.to_string()));
                        }
                    }
                }
            }
            Err(error) => {
                f.state = CheatPackFileState::Unreadable;
                f.diagnostics
                    .push(failure("metadata_error", error.to_string()));
            }
        }
        if f.state == CheatPackFileState::LimitRejected {
            p.complete = false;
        }
        (f.diagnostics, f.diagnostics_truncated) =
            bounded(f.diagnostics, limits.source.max_warnings);
        p.complete &= !f.diagnostics_truncated;
        p.files.push(f);
    }
    bound_diagnostics(&mut p);
    plan(&mut p, existing);
    Ok(p)
}
