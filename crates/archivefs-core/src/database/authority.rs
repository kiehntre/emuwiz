//! Bounded catalogue reads: no ROM/DAT file access and no schema changes.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rusqlite::Connection;

use super::{Database, db_error};
use crate::Result;
use crate::dat::authority::*;

fn count(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(column)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

#[derive(Default)]
struct Evidence {
    checked: u64,
    verified: u64,
    extra: u64,
    outside_expected: u64,
    ambiguous: u64,
    unknown: u64,
    represented: HashSet<String>,
    pending: HashSet<String>,
    complete_sets: HashSet<String>,
    incomplete_sets: HashSet<String>,
}

impl Database {
    /// Consistent read transaction. Work is linear in persisted inventories
    /// and audited rows, not platforms times library size; no per-file query.
    pub fn dat_authority_dashboard(
        &self,
        sources: &[DatAuthoritySource],
    ) -> Result<DatAuthorityDashboard> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|e| db_error("could not read DAT dashboard snapshot", e))?;
        let result =
            project(&tx, sources).map_err(|e| db_error("could not read DAT authority", e))?;
        tx.commit()
            .map_err(|e| db_error("could not finish DAT dashboard read", e))?;
        Ok(result)
    }

    /// Compare two already imported inventories. Canonical-name changes are
    /// renames ONLY when both publish the same unique entry ID. No hash or
    /// BIOS facts are invented from names; the current inventory lacks them.
    pub fn compare_dat_authorities(
        &self,
        old: &str,
        new: &str,
        same_catalogue: bool,
    ) -> Result<DatRefreshImpact> {
        let read = |id: &str| -> rusqlite::Result<BTreeMap<String, Option<String>>> {
            let mut stmt = self.connection.prepare(
                "SELECT canonical_identity, json_extract(CAST(metadata_json AS TEXT), '$.dat_game_id')
                 FROM dat_expected_entries WHERE dat_source_id = ?1")?;
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        };
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|e| db_error("could not compare DATs", e))?;
        // Check metadata, not merely an empty query result (unknown != empty).
        for id in [old, new] {
            if self.expected_dat_inventory_meta(id)?.is_none() {
                return Err(crate::ArchiveFsError::Database(format!(
                    "No imported inventory for {id}"
                )));
            }
        }
        let a = read(old).map_err(|e| db_error("could not read old DAT inventory", e))?;
        let b = read(new).map_err(|e| db_error("could not read new DAT inventory", e))?;
        let removed: Vec<_> = a.iter().filter(|(n, _)| !b.contains_key(*n)).collect();
        let added: Vec<_> = b.iter().filter(|(n, _)| !a.contains_key(*n)).collect();
        let unique_ids = |entries: &BTreeMap<String, Option<String>>| {
            let mut counts = HashMap::new();
            for id in entries.values().flatten() {
                *counts.entry(id.clone()).or_insert(0u64) += 1;
            }
            counts
        };
        let ac = unique_ids(&a);
        let bc = unique_ids(&b);
        let added_ids: HashSet<_> = added.iter().filter_map(|(_, id)| id.as_deref()).collect();
        let renamed = if same_catalogue {
            removed
                .iter()
                .filter_map(|(_, id)| id.as_deref())
                .filter(|id| {
                    ac.get(*id) == Some(&1) && bc.get(*id) == Some(&1) && added_ids.contains(id)
                })
                .count() as u64
        } else {
            0
        };
        let result = DatRefreshImpact {
            old_source: old.into(), new_source: new.into(),
            added: added.len() as u64 - renamed, removed: removed.len() as u64 - renamed, renamed: same_catalogue.then_some(renamed),
            hash_changed: None, bios_requirements_changed: None,
            explanation: "Added entries expand the expected collection; removed entries leave this authority. Renames require a shared unique DAT entry ID. Hash/BIOS changes are unavailable: imported inventory retains names and IDs, not complete member hashes or BIOS declarations. Different source families/variants are not interchangeable. This comparison does not predict file operations or re-audit matches.".into(),
        };
        tx.commit()
            .map_err(|e| db_error("could not finish DAT comparison", e))?;
        Ok(result)
    }
}

fn project(
    db: &Connection,
    configured: &[DatAuthoritySource],
) -> rusqlite::Result<DatAuthorityDashboard> {
    let mut result = DatAuthorityDashboard::default();
    let mut sources: BTreeMap<String, DatAuthoritySource> = configured
        .iter()
        .map(|s| (s.id.clone(), s.clone()))
        .collect();
    let mut expected: HashMap<String, HashSet<String>> = HashMap::new();
    let mut multi_member: HashMap<String, HashSet<String>> = HashMap::new();
    let mut stmt =
        db.prepare("SELECT dat_source_id, canonical_identity, json_extract(CAST(metadata_json AS TEXT), '$.rom_count') FROM dat_expected_entries")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
        ))
    })? {
        let (id, name, members) = row?;
        // Unknown/zero member shape also cannot be proven by one flat-file
        // match. A set audit, not a guess, supplies the missing proof.
        if members != Some(1) {
            multi_member
                .entry(id.clone())
                .or_default()
                .insert(name.clone());
        }
        expected.entry(id.clone()).or_default().insert(name);
        sources
            .entry(id.clone())
            .or_insert_with(|| DatAuthoritySource {
                id: id.clone(),
                name: id,
                provenance: "Persisted inventory; no configured platform assignment".into(),
                ..Default::default()
            });
    }
    let mut meta = HashMap::new();
    let mut stmt = db.prepare("SELECT dat_source_id, source_revision, ecosystem, entry_count, duplicate_names_skipped, validated_at FROM dat_expected_inventory_meta")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            (
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?
                    .map(|s| serde_json::from_str::<crate::dat::model::DatEcosystem>(&s))
                    .transpose()
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                count(r, 3)?,
                count(r, 4)?,
                r.get::<_, String>(5)?,
            ),
        ))
    })? {
        let (id, value) = row?;
        sources
            .entry(id.clone())
            .or_insert_with(|| DatAuthoritySource {
                id: id.clone(),
                name: id.clone(),
                provenance: "Persisted inventory only".into(),
                ..Default::default()
            });
        meta.insert(id, value);
    }
    let mut platforms = BTreeMap::new();
    let mut stmt = db.prepare("SELECT pa.platform, COUNT(*) FROM platform_assignments pa JOIN archives a ON a.id = pa.archive_id WHERE pa.is_current = 1 AND a.last_verified_missing_at IS NULL GROUP BY pa.platform")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, count(r, 1)?)))? {
        let (p, n) = row?;
        platforms.insert(p, n);
    }
    let mut evidence: HashMap<(String, String), Evidence> = HashMap::new();
    let mut variants: HashMap<String, BTreeSet<String>> = HashMap::new();
    // Extract only the needed persisted facts rather than materialising full
    // audit reports. Size freshness is the existing catalogue fallback;
    // this never claims a fresh on-disk cryptographic verification.
    let mut stmt = db.prepare(
        "SELECT pa.platform, l.dat_source_id, l.verification_state,
        l.revision_marked_stale, l.source_revision, l.completeness,
        json_extract(CAST(l.facts_json AS TEXT), '$.canonical.canonical_dat_name'),
        json_extract(CAST(l.facts_json AS TEXT), '$.ambiguous_candidates'),
        json_extract(CAST(l.facts_json AS TEXT), '$.source.variant'),
        json_extract(CAST(l.facts_json AS TEXT), '$.audited_hashes.size_bytes'), a.size_bytes
        FROM library_dat_identities l JOIN archives a ON a.id = l.archive_id
        JOIN platform_assignments pa ON pa.archive_id = a.id AND pa.is_current = 1
        WHERE a.last_verified_missing_at IS NULL",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let platform: String = r.get(0)?;
        let id: String = r.get(1)?;
        let state: String = r.get(2)?;
        let revision: Option<String> = r.get(4)?;
        let complete: String = r.get(5)?;
        let name: Option<String> = r.get(6)?;
        let candidates: Option<String> = r.get(7)?;
        if let Some(v) = r.get::<_, Option<String>>(8)? {
            variants.entry(id.clone()).or_default().insert(v);
        }
        let before_size: Option<i64> = r.get(9)?;
        let size: Option<i64> = r.get(10)?;
        let current_revision = sources
            .get(&id)
            .and_then(|s| s.revision.as_ref())
            .or_else(|| meta.get(&id).and_then(|m| m.0.as_ref()));
        let fresh = r.get::<_, i64>(3)? == 0
            && before_size.is_some()
            && before_size == size
            && revision.as_ref() == current_revision;
        let outside_expected = name
            .as_ref()
            .is_some_and(|n| expected.get(&id).is_some_and(|i| !i.contains(n)));
        let e = evidence.entry((platform, id)).or_default();
        e.checked += 1;
        if fresh
            && state == "verified_single_match"
            && let Some(name) = name.as_ref()
        {
            e.verified += 1;
            if outside_expected {
                e.outside_expected += 1;
            }
            e.represented.insert(name.clone());
        } else if fresh && state == "no_match" && complete == "exhaustive" {
            e.extra += 1;
        } else {
            let ambiguous = matches!(
                state.as_str(),
                "ambiguous_multiple_candidates" | "conflicting"
            );
            if ambiguous {
                e.ambiguous += 1;
            }
            let names: Vec<String> = candidates
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?
                .unwrap_or_default();
            if !fresh || (name.is_none() && names.is_empty()) {
                e.unknown += 1;
            }
            e.pending.extend(names);
            e.pending.extend(name);
        }
    }
    // Dependency-aware set verdicts override flat file membership for arcade
    // sources. A verified ROM is not proof of a complete multi-member set.
    let arcade_sources: HashSet<_> = meta
        .iter()
        .filter(|(_, m)| {
            matches!(
                m.1,
                Some(
                    crate::dat::model::DatEcosystem::MAMEArcade
                        | crate::dat::model::DatEcosystem::MAMESoftwareList
                        | crate::dat::model::DatEcosystem::FBNeo
                )
            )
        })
        .map(|(id, _)| id.clone())
        .collect();
    for ((_, id), e) in &mut evidence {
        if arcade_sources.contains(id) {
            e.pending.extend(e.represented.drain());
        } else if let Some(names) = multi_member.get(id) {
            for name in names {
                if e.represented.remove(name) {
                    e.pending.insert(name.clone());
                }
            }
        }
    }
    let mut bios: HashMap<(String, String), (u64, u64)> = HashMap::new();
    let mut stmt = db.prepare("SELECT s.platform, s.source_id, s.game_name, s.set_state_json, s.dependency_state_json, s.stale, s.exhaustive,
        (SELECT COUNT(DISTINCT d.target_json) FROM dat_set_audit_dependencies d WHERE d.result_id = s.id AND d.dependency_kind = 'bios' AND d.dependency_outcome = 'missing'), s.dat_revision,
        json_extract(CAST(l.facts_json AS TEXT), '$.audited_hashes.size_bytes'), a.size_bytes, l.revision_marked_stale,
        json_extract(CAST(l.facts_json AS TEXT), '$.canonical.canonical_dat_name')
        FROM dat_set_audit_results s JOIN archives a ON a.id = s.archive_id
        LEFT JOIN library_dat_identities l ON l.archive_id = a.id AND l.dat_source_id = s.source_id
        JOIN platform_assignments pa ON pa.archive_id = a.id AND pa.is_current = 1 AND pa.platform = s.platform
        WHERE a.last_verified_missing_at IS NULL")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let platform: Option<String> = r.get(0)?;
        let Some(platform) = platform else {
            continue;
        };
        let id: String = r.get(1)?;
        let name: String = r.get(2)?;
        let revision: Option<String> = r.get(8)?;
        let current_revision = sources
            .get(&id)
            .and_then(|s| s.revision.as_ref())
            .or_else(|| meta.get(&id).and_then(|m| m.0.as_ref()));
        let size: Option<i64> = r.get(9)?;
        let current_size: Option<i64> = r.get(10)?;
        let current_name: Option<String> = r.get(12)?;
        if r.get::<_, i64>(5)? != 0 || r.get::<_, i64>(6)? != 1 {
            continue;
        }
        if revision.as_ref() != current_revision
            || size.is_none()
            || size != current_size
            || r.get::<_, Option<i64>>(11)? != Some(0)
            || current_name.as_deref() != Some(name.as_str())
        {
            continue;
        }
        let state: String = r.get(3)?;
        let dep: String = r.get(4)?;
        let key = (platform, id.clone());
        let b = bios.entry(key.clone()).or_default();
        b.0 += 1;
        b.1 += count(r, 7)?;
        let complete = state == "\"complete\""
            && matches!(dep.as_str(), "\"satisfied\"" | "\"not_applicable\"");
        let e = evidence.entry(key).or_default();
        if complete {
            e.complete_sets.insert(name.clone());
        } else {
            e.incomplete_sets.insert(name.clone());
        }
        if arcade_sources.contains(&id)
            || multi_member
                .get(&id)
                .is_some_and(|names| names.contains(&name))
        {
            if complete {
                e.pending.remove(&name);
                e.represented.insert(name);
            } else {
                e.pending.insert(name);
            }
        }
    }
    // A recorded negative set verdict is stronger than flat membership,
    // including single-ROM entries with unsupported structure/dependencies.
    // Another complete copy of the same identity can still satisfy the set;
    // result ordering must never decide completeness.
    for e in evidence.values_mut() {
        for name in e.incomplete_sets.difference(&e.complete_sets) {
            e.represented.remove(name);
            e.pending.insert(name.clone());
        }
    }
    for source in sources.into_values() {
        let m = meta.get(&source.id);
        let inventory = expected.get(&source.id);
        let drift = source
            .revision
            .as_ref()
            .is_some_and(|r| Some(r) != m.and_then(|m| m.0.as_ref()));
        let usable = source.enabled
            && source.platform.is_some()
            && source.validation_problem.is_none()
            && !drift
            && m.is_some_and(|m| {
                m.2 > 0 && m.3 == 0 && inventory.is_some_and(|i| i.len() as u64 == m.2)
            });
        let variant = variants
            .get(&source.id)
            .map(|v| v.iter().cloned().collect::<Vec<_>>().join(", "));
        let mut preparation = Vec::new();
        if let Some(problem) = &source.validation_problem {
            preparation.push(problem.clone());
        }
        if source.platform.is_none() {
            preparation.push("DAT is not linked to a platform; assign it in DAT Sources.".into());
        }
        if !source.enabled {
            preparation.push("Source is disabled or no longer configured.".into());
        }
        if m.is_none() {
            preparation
                .push("No expected inventory retained; validate this DAT in DAT Sources.".into());
        }
        if drift {
            preparation.push("Configured revision differs from the imported inventory; validate and review the new authority.".into());
        }
        if m.is_some_and(|m| m.3 > 0) {
            preparation.push("Duplicate DAT names make the inventory ambiguous.".into());
        }
        if variant.as_deref().is_none_or(|v| v.contains("unknown")) {
            preparation.push("DAT variant is not recorded or is unknown; inspect the source before comparing collections.".into());
        }
        preparation.push("This DAT may be imported, but publisher freshness cannot be verified from local records.".into());
        if usable {
            preparation.push("Ready for an audit against the imported inventory; this is not proof of publisher freshness.".into());
        }
        if let Some(platform) = &source.platform {
            let local = *platforms.entry(platform.clone()).or_default();
            let e = evidence
                .remove(&(platform.clone(), source.id.clone()))
                .unwrap_or_default();
            let mut counts = CompletenessCounts {
                local,
                ambiguous: e.ambiguous,
                extra: e.extra + e.outside_expected,
                verified_local: e.verified,
                unidentified_local: local.saturating_sub(e.verified + e.extra + e.ambiguous),
                bios_missing: bios
                    .get(&(platform.clone(), source.id.clone()))
                    .map(|b| b.1),
                ..Default::default()
            };
            let mut explanations = Vec::new();
            if usable {
                let inventory = inventory.expect("usable inventory");
                counts.expected = Some(inventory.len() as u64);
                counts.matched = Some(inventory.intersection(&e.represented).count() as u64);
                counts.pending_entries = Some(
                    inventory
                        .iter()
                        .filter(|n| !e.represented.contains(*n) && e.pending.contains(*n))
                        .count() as u64,
                );
                if e.checked >= local && e.unknown == 0 {
                    counts.missing = Some(
                        inventory
                            .iter()
                            .filter(|n| !e.represented.contains(*n) && !e.pending.contains(*n))
                            .count() as u64,
                    );
                } else {
                    explanations.push("Some local files lack current audit evidence; missing entries cannot yet be distinguished from unverified local content.".into());
                }
            } else {
                explanations.extend(preparation.clone());
            }
            if let Some(n) = counts.missing.filter(|n| *n > 0) {
                explanations.push(format!(
                    "{n} DAT entries have no representation in the catalogued library."
                ));
            }
            if counts.ambiguous > 0 {
                explanations.push(format!("{} local files have ambiguous DAT matches; their possible entries are not counted as missing.", counts.ambiguous));
            }
            if let Some(n) = counts.pending_entries.filter(|n| *n > 0) {
                explanations.push(format!("{n} expected entries have pending or ambiguous representation, not verified matches."));
            }
            if let Some(n) = counts.bios_missing.filter(|n| *n > 0) {
                explanations.push(format!("{n} missing BIOS dependency requirements are recorded by set audits (not a complete platform BIOS inventory)."));
            }
            let state = if !source.enabled {
                CompletenessState::NoAuthority
            } else if !usable {
                CompletenessState::PartialAuthority
            } else if counts.ambiguous > 0 {
                CompletenessState::Ambiguous
            } else if counts.matched == counts.expected && counts.bios_missing.unwrap_or(0) == 0 {
                CompletenessState::Complete
            } else if counts.missing.is_none() {
                CompletenessState::Unverified
            } else {
                CompletenessState::Incomplete
            };
            explanations.push("Counts use recorded catalogue evidence, not a live filesystem check. Completion is scoped to this imported DAT, not every game ever released or emulator readiness.".into());
            result.collections.push(CollectionCompleteness {
                platform: platform.clone(),
                source_id: Some(source.id.clone()),
                state,
                counts,
                explanations,
            });
        }
        result.authorities.push(DatAuthorityStatus {
            ecosystem: m.and_then(|m| m.1).or(source.ecosystem),
            variant,
            inventory_revision: m.and_then(|m| m.0.clone()),
            validated_at: m.map(|m| m.4.clone()),
            freshness: if drift {
                AuthorityFreshness::Stale
            } else {
                AuthorityFreshness::Unknown
            },
            authority_confidence: if usable {
                "Validated imported inventory; publisher currency unknown"
            } else {
                "Insufficient authority evidence"
            }
            .into(),
            inventory_usable: usable,
            preparation,
            source,
        });
    }
    for (platform, local) in platforms {
        if !result.collections.iter().any(|c| c.platform == platform) {
            result.collections.push(CollectionCompleteness { platform, source_id: None, state: CompletenessState::NoAuthority,
                counts: CompletenessCounts { local, unidentified_local: local, ..Default::default() },
                explanations: vec!["No authoritative DAT is assigned to this platform. Add or link a source in DAT Sources; no completeness denominator is available.".into()] });
        }
    }
    result
        .collections
        .sort_by(|a, b| (&a.platform, &a.source_id).cmp(&(&b.platform, &b.source_id)));
    Ok(result)
}

#[cfg(test)]
#[path = "authority_tests.rs"]
mod tests;
