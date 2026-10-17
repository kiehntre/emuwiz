//! Allowlist-only insertion using the catalogue's existing write transaction,
//! upsert savepoint and platform precedence. No scanner or missing reconciliation.
use super::*;
use crate::identity_source::hashing::{FileFingerprint, hash_file};
use crate::ingestion::bounded_import::{BoundedImportPlan, BoundedImportResult};
use crate::safe_read::TrustedRoots;
use crate::{
    build_source_folder_views, canonical_platform_for_alias, load_source_folder_configs_from,
};
use std::sync::atomic::{AtomicBool, Ordering};

impl Database {
    pub(crate) fn bounded_source_bindings(
        &self,
    ) -> std::result::Result<Vec<(i64, Option<(String, i64)>)>, String> {
        self.list_source_folders()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|s| {
                self.source_binding(s.id)
                    .map(|b| (s.id, b))
                    .map_err(|e| e.to_string())
            })
            .collect()
    }
    pub(crate) fn validate_bounded_import(
        &self,
        plan: &BoundedImportPlan,
    ) -> std::result::Result<RegisteredSourceFolder, String> {
        if self.path != plan.database {
            return Err("Library path changed.".into());
        }
        if self.schema_version().map_err(|e| e.to_string())? != latest_known_version(MIGRATIONS) {
            return Err(
                "Library schema changed; no migration is permitted by bounded Apply.".into(),
            );
        }
        let sources = build_source_folder_views(
            &load_source_folder_configs_from(&plan.config).map_err(|e| e.to_string())?,
            &self.list_source_folders().map_err(|e| e.to_string())?,
        );
        if sources != plan.sources
            || self.list_platform_aliases().map_err(|e| e.to_string())? != plan.aliases
        {
            return Err(
                "Saved source roles, platform overrides or aliases changed. Prepare again.".into(),
            );
        }
        let source = sources
            .iter()
            .filter(|s| plan.root().starts_with(&s.path))
            .max_by_key(|s| s.path.components().count())
            .ok_or("Preview root is outside configured sources")?;
        if self.bounded_source_bindings()? != plan.source_bindings
            || !self
                .source_binding_current(source.id.ok_or("Unregistered source")?, &source.path)
                .map_err(|e| e.to_string())?
        {
            return Err("Accepted source binding changed or is unavailable. Use the existing source review/rebind workflow, then prepare again.".into());
        }
        if !source.enabled
            || !source.role.may_enter_game_scan()
            || sources
                .iter()
                .any(|s| s.path != plan.root() && s.path.starts_with(plan.root()))
        {
            return Err(
                "Source is disabled, protected, or crosses child-source boundaries.".into(),
            );
        }
        if fs::canonicalize(plan.root()).map_err(|e| e.to_string())? != plan.root() {
            return Err("Source root is no longer canonical.".into());
        }
        let current: Vec<_> = self
            .load_archives()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|a| a.absolute_path.starts_with(plan.root()))
            .collect();
        if current != plan.evidence.known {
            return Err("Library records changed after preview. Prepare again; existing identities are protected.".into());
        }
        Ok(RegisteredSourceFolder {
            id: source
                .id
                .ok_or("Source has no existing database registration")?,
            path: source.path.clone(),
            role: source.role,
            assigned_platform: source.assigned_platform.clone(),
            excluded_source_roots: Vec::new(),
        })
    }
    pub(crate) fn apply_bounded_import(
        plan: &BoundedImportPlan,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(usize, usize),
    ) -> std::result::Result<BoundedImportResult, String> {
        plan.current_filesystem()?;
        let read = Self::open_read_only(&plan.database).map_err(|e| e.to_string())?;
        let reviewed_source = read.validate_bounded_import(plan)?;
        drop(read);
        let mut result = BoundedImportResult {
            inserted: Vec::new(),
            skipped: plan.exclusions().to_vec(),
        };
        let mut valid = Vec::new();
        let trusted = TrustedRoots::from_paths([plan.root()]);
        for (n, c) in plan.evidence.candidates.iter().enumerate() {
            progress(n, plan.candidate_count());
            if cancel.load(Ordering::Relaxed) {
                return Err("Bounded Apply cancelled. No insertions committed.".into());
            }
            let path = &c.hashes.fingerprint.path;
            let refusal = if c.ambiguous {
                Some("Ambiguous catalogue identity; no identity selected.".to_string())
            } else if !c.relationships_complete {
                Some("Archive/catalogue comparison was incomplete; prepare a complete preview before import.".into())
            } else if !path.starts_with(plan.root()) {
                Some("Outside reviewed source scope.".into())
            } else if FileFingerprint::observe(path).as_ref() != Some(&c.hashes.fingerprint) {
                Some("File changed, missing, protected or unsafe since preview.".into())
            } else {
                None
            };
            if let Some(reason) = refusal {
                result.skipped.push((path.clone(), reason));
                continue;
            }
            match hash_file(path, &trusted, Some(cancel)) {
                Ok(h) if h == c.hashes => {
                    if let Some(archive) = Archive::from_path_in_root(path, &reviewed_source.path) {
                        if archive.kind.storage_name() == c.kind
                            && archive.kind != ArchiveKind::ArcadeSetDirectory
                            && archive
                                .identity
                                .platform
                                .as_deref()
                                .and_then(canonical_platform_for_alias)
                                == Some(c.platform.as_str())
                        {
                            if let Err(reason) =
                                crate::ingestion::bounded_import::validate_companions(
                                    c,
                                    plan.root(),
                                    &trusted,
                                    cancel,
                                )
                            {
                                result.skipped.push((path.clone(), reason));
                                continue;
                            }
                            valid.push((archive, h.fingerprint, c.platform.clone()));
                            continue;
                        }
                    }
                    result.skipped.push((
                        path.clone(),
                        "Unsupported, ambiguous or structurally refused media during revalidation."
                            .into(),
                    ));
                }
                Ok(_) => result.skipped.push((
                    path.clone(),
                    "Content identity changed since preview.".into(),
                )),
                Err(e) => result.skipped.push((
                    path.clone(),
                    format!("Unreadable, protected or unstable media: {e:?}"),
                )),
            }
        }
        plan.current_filesystem()?;
        let mut db = Self {
            connection: open_connection_with(&plan.database, false).map_err(|e| e.to_string())?,
            path: plan.database.clone(),
        };
        db.begin_catalogue_refresh().map_err(|e| e.to_string())?;
        let applied = (|| {
            let folder = db.validate_bounded_import(plan)?;
            let mut inserted_generations = Vec::new();
            for (archive, generation, expected_platform) in valid {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Bounded Apply cancelled; all insertions rolled back.".into());
                }
                if FileFingerprint::observe(&archive.path).as_ref() != Some(&generation) {
                    result.skipped.push((
                        archive.path.clone(),
                        "File changed or became unavailable before insertion.".into(),
                    ));
                    continue;
                }
                // Check globally by exact pathname too, avoiding duplicate rows
                // under overlapping or legacy source ownership.
                let exists: bool = db
                    .connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM archives WHERE absolute_path_cached=?1)",
                        [archive.path.as_os_str().as_bytes()],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if exists {
                    result.skipped.push((
                        archive.path.clone(),
                        "Existing library record is protected; never updated.".into(),
                    ));
                    continue;
                }
                db.begin_folder_refresh().map_err(|e| e.to_string())?;
                let outcome = db
                    .upsert_archive(folder.id, &folder.path, &archive)
                    .map_err(|e| e.to_string())?;
                if outcome.change != ArchiveChangeKind::New {
                    return Err("Existing record encountered; transaction rolled back.".into());
                }
                assign_platform_for_scanned_archive(&mut db, &folder, &archive, outcome.archive_id)
                    .map_err(|e| e.to_string())?;
                let platform: Option<String> = db
                    .connection
                    .query_row(
                        "SELECT platform FROM platform_assignments WHERE archive_id=?1",
                        [outcome.archive_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                    .flatten();
                if platform.as_deref().and_then(canonical_platform_for_alias)
                    != Some(expected_platform.as_str())
                {
                    db.rollback_folder_refresh().map_err(|e| e.to_string())?;
                    result.skipped.push((archive.path.clone(),"Saved platform precedence conflicts with the reviewed platform or remains ambiguous; no record inserted.".into()));
                    continue;
                }
                db.commit_folder_refresh().map_err(|e| e.to_string())?;
                result
                    .inserted
                    .push((archive.path.clone(), outcome.archive_id));
                inserted_generations.push((archive.path, generation));
            }
            progress(plan.candidate_count(), plan.candidate_count());
            if cancel.load(Ordering::Relaxed) {
                return Err("Bounded Apply cancelled; all insertions rolled back.".into());
            }
            plan.current_filesystem()?;
            let inserted_paths: std::collections::BTreeSet<_> =
                inserted_generations.iter().map(|(p, _)| p).collect();
            if plan
                .evidence
                .candidates
                .iter()
                .filter(|c| inserted_paths.contains(&c.hashes.fingerprint.path))
                .flat_map(|c| c.companions.iter())
                .any(|h| {
                    FileFingerprint::observe(&h.fingerprint.path).as_ref() != Some(&h.fingerprint)
                })
            {
                return Err("Companion changed before commit; all insertions rolled back.".into());
            }
            if !db
                .source_binding_current(folder.id, &folder.path)
                .map_err(|e| e.to_string())?
            {
                return Err(
                    "Accepted parent source changed before commit; all insertions rolled back."
                        .into(),
                );
            }
            if inserted_generations
                .iter()
                .any(|(p, g)| FileFingerprint::observe(p).as_ref() != Some(g))
            {
                return Err("File changed before commit; all insertions rolled back.".into());
            }
            db.commit_catalogue_refresh().map_err(|e| e.to_string())?;
            Ok(result)
        })();
        if applied.is_err() {
            db.rollback_catalogue_refresh();
        }
        applied
    }
}
