//! SQL summary projection: no per-archive objects, writes, or filesystem probes.
use super::{Database, db_error};
use crate::attention::{
    ATTENTION_GROUP_LIMIT, AttentionCategory as C, AttentionDestination as D, AttentionItem,
    AttentionSeverity as S, AttentionSnapshot,
};

impl Database {
    /// A consistent read transaction over saved catalogue facts. The result is
    /// capped summary groups (not an unbounded list of files). Large groups are
    /// reviewed using the existing workflow's paginated detail views.
    pub fn attention_snapshot(&self) -> crate::Result<AttentionSnapshot> {
        let start = std::time::Instant::now();
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|e| db_error("start attention read snapshot", e))?;
        let mut snapshot = AttentionSnapshot::default();
        let queries = [
            // Never label unconfigured/offline sources using guessed error text.
            (
                "source",
                "SELECT CAST(id AS TEXT), '', COUNT(*), CAST(strftime('%s', last_scan_at) AS INTEGER), COALESCE(last_scan_error, 'The last source scan failed'), CAST(path AS TEXT) FROM source_folders WHERE removed_from_config_at IS NULL AND last_scan_status = 'failed' GROUP BY id LIMIT ?1",
                C::Sources,
                S::ActionNeeded,
                D::Sources,
                "Source folder needs attention",
            ),
            (
                "missing",
                "SELECT CAST(a.source_folder_id AS TEXT), COALESCE(p.platform, ''), COUNT(*), CAST(strftime('%s', MAX(a.last_verified_missing_at)) AS INTEGER), '', '' FROM archives a INDEXED BY archives_missing JOIN source_folders s ON s.id=a.source_folder_id LEFT JOIN platform_assignments p ON p.archive_id=a.id AND p.is_current=1 WHERE s.removed_from_config_at IS NULL AND a.last_verified_missing_at IS NOT NULL AND COALESCE(s.last_scan_status, '') != 'failed' GROUP BY a.source_folder_id, p.platform LIMIT ?1",
                C::Sources,
                S::ActionNeeded,
                D::Sources,
                "Catalogue files are missing from their source",
            ),
            (
                "identity",
                "SELECT 'unknown', '', COUNT(*), CAST(strftime('%s', MAX(a.last_seen_at)) AS INTEGER), '', '' FROM archives a JOIN source_folders s ON s.id=a.source_folder_id LEFT JOIN platform_assignments p ON p.archive_id=a.id AND p.is_current=1 WHERE s.removed_from_config_at IS NULL AND a.last_verified_missing_at IS NULL AND (p.platform IS NULL OR p.platform='' OR p.platform='Unknown') HAVING COUNT(*) > 0 LIMIT ?1",
                C::Identity,
                S::Warning,
                D::Discovery,
                "Files need platform identity review",
            ),
            (
                "dat",
                "SELECT d.verification_state || ':' || d.revision_marked_stale, COALESCE(p.platform, ''), COUNT(*), CAST(strftime('%s', MAX(d.audited_at)) AS INTEGER), '', '' FROM library_dat_identities d JOIN archives a ON a.id=d.archive_id JOIN source_folders s ON s.id=a.source_folder_id LEFT JOIN platform_assignments p ON p.archive_id=a.id AND p.is_current=1 WHERE a.last_verified_missing_at IS NULL AND s.removed_from_config_at IS NULL AND (d.verification_state IN ('no_match', 'conflicting', 'ambiguous_multiple_candidates', 'no_usable_evidence', 'filename_only_not_verified') OR d.revision_marked_stale=1) GROUP BY d.verification_state, d.revision_marked_stale, p.platform LIMIT ?1",
                C::Dat,
                S::ActionNeeded,
                D::DatReview,
                "DAT identity needs review",
            ),
            (
                "dat-set",
                "SELECT CASE WHEN d.stale=1 THEN 'stale' WHEN d.set_state_json='\"incomplete\"' THEN 'incomplete' ELSE 'review' END, COALESCE(d.platform, ''), COUNT(*), CAST(strftime('%s', MAX(d.audited_at)) AS INTEGER), '', '' FROM dat_set_audit_results d LEFT JOIN archives a ON a.id=d.archive_id LEFT JOIN source_folders s ON s.id=a.source_folder_id WHERE (d.archive_id IS NULL OR (a.last_verified_missing_at IS NULL AND s.removed_from_config_at IS NULL)) AND (d.stale=1 OR d.set_state_json!='\"complete\"' OR d.dependency_state_json NOT IN ('\"satisfied\"','\"not_applicable\"')) GROUP BY 1, d.platform LIMIT ?1",
                C::Dat,
                S::ActionNeeded,
                D::DatReview,
                "Saved DAT set audit needs review",
            ),
            // These are name candidates, NOT verified content duplicates.
            (
                "duplicate",
                "SELECT 'name_candidates', platform, COUNT(*), NULL, CAST(SUM(n) AS TEXT), '' FROM (SELECT COALESCE(p.platform, '') platform, a.normalized_name, COUNT(*) n FROM archives a JOIN source_folders s ON s.id=a.source_folder_id LEFT JOIN platform_assignments p ON p.archive_id=a.id AND p.is_current=1 WHERE a.last_verified_missing_at IS NULL AND s.removed_from_config_at IS NULL AND a.normalized_name != '' GROUP BY p.platform, a.normalized_name HAVING COUNT(*) > 1) GROUP BY platform LIMIT ?1",
                C::Duplicates,
                S::ActionNeeded,
                D::Duplicates,
                "Possible duplicate files are waiting for review",
            ),
        ];
        for (source, sql, category, severity, destination, title) in queries {
            let mut statement = transaction
                .prepare(sql)
                .map_err(|e| db_error("prepare attention summary", e))?;
            let rows = statement
                .query_map([ATTENTION_GROUP_LIMIT as i64 + 1], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?.max(0) as u64,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, String>(4)?,
                        match row.get_ref(5)? {
                            rusqlite::types::ValueRef::Text(bytes)
                            | rusqlite::types::ValueRef::Blob(bytes) => {
                                String::from_utf8_lossy(bytes).into_owned()
                            }
                            _ => String::new(),
                        },
                    ))
                })
                .map_err(|e| db_error("query attention summary", e))?;
            snapshot.query_count += 1;
            for row in rows {
                let (key, platform, count, observed, detail, path) =
                    row.map_err(|e| db_error("read attention summary", e))?;
                let mut item = AttentionItem::new(
                    format!("catalogue:{source}:{key}:{platform}"),
                    category,
                    severity,
                    title.into(),
                    destination,
                );
                item.platform = (!platform.is_empty()).then_some(platform);
                item.affected_count = count;
                item.last_observed = observed;
                item.affected = (!path.is_empty()).then_some(path);
                item.source_records = vec![format!("{source}:{key}")];
                item.provenance =
                    "Saved catalogue evidence; no live filesystem verification on this page".into();
                item.summary = match source {
                    "source" => detail,
                    "duplicate" => format!(
                        "{detail} files in {count} same-name groups. Matching names alone do not prove identical content."
                    ),
                    "dat-set" => format!(
                        "{count} saved set audits: {}. These are observed set/dependency verdicts, not a collection completeness percentage.",
                        match key.as_str() {
                            "stale" => "authority revision is stale",
                            "incomplete" => "required set members were not verified complete",
                            _ => "metadata or dependency review is required",
                        }
                    ),
                    "dat" => {
                        let explanation = if key.ends_with(":1") {
                            "The saved DAT authority revision is stale; re-audit before relying on it."
                        } else if key.starts_with("no_match:") {
                            "No DAT entry matched the recorded evidence."
                        } else if key.starts_with("conflicting:") {
                            "The recorded DAT identity evidence conflicts."
                        } else if key.starts_with("ambiguous_multiple_candidates:") {
                            "Several DAT identities match; a choice needs review."
                        } else {
                            "There is not enough verified evidence to establish DAT authority."
                        };
                        format!("{count} saved identity records. {explanation}")
                    }
                    _ => format!("{count} files in the last recorded catalogue state."),
                };
                if category == C::Dat {
                    snapshot.insert_dat_summary(item);
                } else {
                    snapshot.insert(item);
                }
            }
        }
        // Scan-owned counters are current for this completed scan, unlike an
        // old fingerprint row that can survive removal of an unsupported file.
        let mut statement = transaction.prepare("SELECT id, skipped_unsupported_extension, skipped_ambiguous_platform, CAST(strftime('%s', finished_at) AS INTEGER) FROM scan_runs WHERE status = 'completed' ORDER BY id DESC LIMIT 1")
            .map_err(|e| db_error("prepare attention discovery summary", e))?;
        let mut rows = statement
            .query([])
            .map_err(|e| db_error("query attention discovery summary", e))?;
        snapshot.query_count += 1;
        if let Some(row) = rows
            .next()
            .map_err(|e| db_error("read attention discovery summary", e))?
        {
            let id: i64 = row
                .get(0)
                .map_err(|e| db_error("read attention scan id", e))?;
            for (column, category, label) in [
                (1, C::Unsupported, "files have unsupported formats"),
                (2, C::Identity, "files have ambiguous platform identity"),
            ] {
                let count = row
                    .get::<_, i64>(column)
                    .map_err(|e| db_error("read attention discovery count", e))?
                    .max(0) as u64;
                if count == 0 {
                    continue;
                }
                let mut item = AttentionItem::new(
                    format!("discovery:{column}"),
                    category,
                    S::Warning,
                    format!("{count} {label}"),
                    D::Discovery,
                );
                item.affected_count = count;
                item.last_observed = row
                    .get(3)
                    .map_err(|e| db_error("read attention scan time", e))?;
                item.source_records.push(format!("scan_runs:{id}"));
                item.summary = "Recorded by the latest completed scan. Review its discovery details for paths and reasons; other source roots may not have been scanned in that run.".into();
                item.provenance = "Completed scan counters (not inferred from error text)".into();
                snapshot.insert(item);
            }
        } else {
            snapshot
                .coverage_notes
                .push("Discovery has no completed scan yet.".into());
        }
        drop(rows);
        drop(statement);
        // Report input cardinality, not a misleading count of returned groups.
        snapshot.source_rows = transaction.query_row("SELECT (SELECT COUNT(*) FROM archives) + (SELECT COUNT(*) FROM library_dat_identities) + (SELECT COUNT(*) FROM dat_set_audit_results) + (SELECT COUNT(*) FROM source_folders)", [], |row| row.get::<_, i64>(0))
            .map_err(|e| db_error("count attention source rows", e))?.max(0) as u64;
        snapshot.query_count += 1;
        transaction
            .commit()
            .map_err(|e| db_error("finish attention read snapshot", e))?;
        snapshot.query_millis = start.elapsed().as_millis();
        snapshot.coverage_notes.push("DAT coverage here is saved identity/audit evidence, not collection completeness. Expected/missing sets remain in DAT coverage review.".into());
        Ok(snapshot)
    }
}
