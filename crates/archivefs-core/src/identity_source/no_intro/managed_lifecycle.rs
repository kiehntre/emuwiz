//! Offline lifecycle, coverage, freshness, rollback, and retention planning
//! for imported No-Intro packs.
//!
//! This is deliberately a registry of evidence, not an updater.  Pack bytes
//! remain content addressed and are never removed or rewritten here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::import::{ImportedNoIntroSource, NoIntroVariant};
use super::pack_import::{NoIntroPackImportError, RejectedNoIntroPackMember};

const LIFECYCLE_FILE: &str = "lifecycle.json";
const LIFECYCLE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoIntroPackStatus {
    Current,
    Superseded,
    Invalid,
    Partial,
    RetainedForRollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NoIntroStaleness {
    Current,
    Superseded,
    PossiblyStale,
    UnknownFreshness,
    Invalid,
    MissingCurrent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NoIntroRetention {
    KeepCurrent,
    KeepRollback,
    SupersededRetained,
    SafePruneCandidate,
    UnknownKeep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoIntroPackCoverage {
    pub canonical_platform: Option<String>,
    pub family: String,
    pub dat_member_identity: String,
    pub variant: NoIntroVariant,
    pub source_member_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoIntroPackSnapshot {
    pub pack_sha256: String,
    pub snapshot_sha256: String,
    pub imported_at_unix_seconds: u64,
    pub import_order: u64,
    pub status: NoIntroPackStatus,
    pub members: Vec<NoIntroPackMemberEvidence>,
    pub coverage: Vec<NoIntroPackCoverage>,
    pub predecessor: Option<String>,
    pub successor: Option<String>,
    pub rejected: Vec<RejectedNoIntroPackMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoIntroPackMemberEvidence {
    pub source_member_name: String,
    pub system_name: String,
    pub variant: NoIntroVariant,
    pub artifact_sha256: String,
    pub upstream_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LifecycleStore {
    schema_version: u32,
    next_import_order: u64,
    snapshots: Vec<NoIntroPackSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoIntroPackResolution {
    pub current: BTreeMap<String, String>,
    pub previous: BTreeMap<String, String>,
    pub superseded: BTreeSet<String>,
    pub unresolved: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoIntroStalenessReport {
    pub pack_sha256: String,
    pub state: NoIntroStaleness,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoIntroRollbackPlan {
    pub from_pack_sha256: String,
    pub to_pack_sha256: String,
    pub snapshot_sha256: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoIntroRetentionDecision {
    pub pack_sha256: String,
    pub classification: NoIntroRetention,
    pub reason: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn platform_for_system(system: &str) -> Option<String> {
    crate::canonical_platform_for_alias(system)
        .map(str::to_string)
        .or_else(|| {
            system
                .rsplit_once(" - ")
                .and_then(|(_, suffix)| crate::canonical_platform_for_alias(suffix))
                .map(str::to_string)
        })
}

fn coverage_for(source: &ImportedNoIntroSource) -> NoIntroPackCoverage {
    NoIntroPackCoverage {
        canonical_platform: platform_for_system(&source.system_name),
        family: source.system_name.clone(),
        dat_member_identity: source.artifact_sha256.clone(),
        variant: source.variant,
        source_member_name: source.artifact_name.clone(),
    }
}

fn load_store(root: &Path) -> Result<LifecycleStore, NoIntroPackImportError> {
    let path = root.join(LIFECYCLE_FILE);
    match fs::read_to_string(&path) {
        Ok(body) => {
            serde_json::from_str(&body).map_err(|e| NoIntroPackImportError::State(e.to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LifecycleStore {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            next_import_order: 0,
            snapshots: Vec::new(),
        }),
        Err(e) => Err(NoIntroPackImportError::State(format!(
            "cannot read {}: {e}",
            path.display()
        ))),
    }
}

fn save_store(root: &Path, store: &LifecycleStore) -> Result<(), NoIntroPackImportError> {
    let body = serde_json::to_string_pretty(store)
        .map_err(|e| NoIntroPackImportError::State(e.to_string()))?;
    crate::atomic_write_text(&root.join(LIFECYCLE_FILE), &format!("{body}\n"))
        .map_err(|e| NoIntroPackImportError::State(e.to_string()))
}

/// Registers a validated import. Exact pack identity is idempotent.
pub fn register_no_intro_pack_at(
    root: &Path,
    pack_sha256: &str,
    snapshot_sha256: &str,
    sources: &[ImportedNoIntroSource],
    rejected: &[RejectedNoIntroPackMember],
) -> Result<NoIntroPackSnapshot, NoIntroPackImportError> {
    let mut store = load_store(root)?;
    if store.schema_version != LIFECYCLE_SCHEMA_VERSION {
        return Err(NoIntroPackImportError::State(
            "unsupported No-Intro lifecycle schema".into(),
        ));
    }
    if let Some(existing) = store
        .snapshots
        .iter()
        .find(|s| s.pack_sha256 == pack_sha256)
    {
        return Ok(existing.clone());
    }
    let order = store.next_import_order.saturating_add(1);
    store.next_import_order = order;
    let members: Vec<_> = sources
        .iter()
        .map(|s| NoIntroPackMemberEvidence {
            source_member_name: s.artifact_name.clone(),
            system_name: s.system_name.clone(),
            variant: s.variant,
            artifact_sha256: s.artifact_sha256.clone(),
            upstream_version: s.upstream_version.clone(),
        })
        .collect();
    let mut coverage: Vec<_> = sources.iter().map(coverage_for).collect();
    coverage.sort_by(|a, b| {
        a.canonical_platform
            .cmp(&b.canonical_platform)
            .then_with(|| a.family.cmp(&b.family))
            .then_with(|| format!("{:?}", a.variant).cmp(&format!("{:?}", b.variant)))
            .then_with(|| a.dat_member_identity.cmp(&b.dat_member_identity))
    });
    let partial = !rejected.is_empty() || coverage.iter().any(|c| c.canonical_platform.is_none());
    let snapshot = NoIntroPackSnapshot {
        pack_sha256: pack_sha256.into(),
        snapshot_sha256: snapshot_sha256.into(),
        imported_at_unix_seconds: now(),
        import_order: order,
        status: if partial {
            NoIntroPackStatus::Partial
        } else {
            NoIntroPackStatus::Current
        },
        members,
        coverage,
        predecessor: None,
        successor: None,
        rejected: rejected.to_vec(),
    };
    store.snapshots.push(snapshot.clone());
    save_store(root, &store)?;
    Ok(snapshot)
}

pub fn load_no_intro_pack_snapshots_at(
    root: &Path,
) -> Result<Vec<NoIntroPackSnapshot>, NoIntroPackImportError> {
    Ok(load_store(root)?.snapshots)
}

fn version_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    if a == b {
        Some(std::cmp::Ordering::Equal)
    } else if a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()) {
        Some(a.cmp(b))
    } else {
        None
    }
}

fn compare(
    a: &NoIntroPackSnapshot,
    b: &NoIntroPackSnapshot,
    family: &str,
    variant: NoIntroVariant,
) -> Option<std::cmp::Ordering> {
    let av = a
        .members
        .iter()
        .find(|m| m.system_name == family && m.variant == variant)?
        .upstream_version
        .as_deref();
    let bv = b
        .members
        .iter()
        .find(|m| m.system_name == family && m.variant == variant)?
        .upstream_version
        .as_deref();
    match (av, bv) {
        (Some(x), Some(y)) => version_cmp(x, y),
        _ => Some(a.import_order.cmp(&b.import_order)),
    }
}

/// Pure deterministic per-coverage resolution. Authoritative numeric DAT
/// versions win; otherwise explicit local import order is the final signal.
pub fn resolve_no_intro_current(snapshots: &[NoIntroPackSnapshot]) -> NoIntroPackResolution {
    let mut candidates: BTreeMap<String, Vec<(&NoIntroPackSnapshot, &NoIntroPackCoverage)>> =
        BTreeMap::new();
    for s in snapshots
        .iter()
        .filter(|s| !matches!(s.status, NoIntroPackStatus::Invalid))
    {
        for c in &s.coverage {
            let key = format!(
                "{}|{}|{:?}",
                c.canonical_platform.as_deref().unwrap_or("unknown"),
                c.family,
                c.variant
            );
            candidates.entry(key).or_default().push((s, c));
        }
    }
    let mut out = NoIntroPackResolution {
        current: BTreeMap::new(),
        previous: BTreeMap::new(),
        superseded: BTreeSet::new(),
        unresolved: BTreeSet::new(),
    };
    for (key, mut list) in candidates {
        list.sort_by(|(a, _), (b, _)| a.pack_sha256.cmp(&b.pack_sha256));
        let mut winner = list[0];
        let mut conflict = false;
        for candidate in list.iter().skip(1) {
            match compare(
                winner.0,
                candidate.0,
                &candidate.1.family,
                candidate.1.variant,
            ) {
                Some(std::cmp::Ordering::Less) => winner = *candidate,
                Some(std::cmp::Ordering::Equal) => {
                    if winner.0.pack_sha256 != candidate.0.pack_sha256 {
                        conflict = true;
                    }
                }
                Some(std::cmp::Ordering::Greater) => {}
                None => conflict = true,
            }
        }
        if conflict {
            out.unresolved.insert(key);
            continue;
        }
        out.current
            .insert(key.clone(), winner.0.pack_sha256.clone());
        for (s, _) in list {
            if s.pack_sha256 != winner.0.pack_sha256 {
                out.superseded.insert(s.pack_sha256.clone());
            }
        }
    }
    let mut grouped: BTreeMap<String, Vec<(&String, &String)>> = BTreeMap::new();
    for (k, v) in &out.current {
        grouped
            .entry(k.split('|').next().unwrap_or(k).into())
            .or_default()
            .push((k, v));
    }
    for (_, mut entries) in grouped {
        entries.sort_by(|a, b| a.0.cmp(b.0));
        if entries.len() > 1 {
            out.previous
                .insert(entries[0].0.clone(), entries[0].1.clone());
        }
    }
    out
}

pub fn no_intro_staleness(
    snapshots: &[NoIntroPackSnapshot],
    pack_sha256: &str,
) -> NoIntroStalenessReport {
    let Some(pack) = snapshots.iter().find(|s| s.pack_sha256 == pack_sha256) else {
        return NoIntroStalenessReport {
            pack_sha256: pack_sha256.into(),
            state: NoIntroStaleness::MissingCurrent,
            reason: "no managed snapshot is registered".into(),
        };
    };
    if pack.status == NoIntroPackStatus::Invalid {
        return NoIntroStalenessReport {
            pack_sha256: pack_sha256.into(),
            state: NoIntroStaleness::Invalid,
            reason: "validation failed".into(),
        };
    }
    let resolution = resolve_no_intro_current(snapshots);
    if resolution
        .unresolved
        .iter()
        .any(|k| pack.coverage.iter().any(|c| k.contains(&c.family)))
    {
        return NoIntroStalenessReport {
            pack_sha256: pack_sha256.into(),
            state: NoIntroStaleness::UnknownFreshness,
            reason: "competing snapshots cannot be ordered safely".into(),
        };
    }
    if resolution.superseded.contains(pack_sha256) {
        return NoIntroStalenessReport {
            pack_sha256: pack_sha256.into(),
            state: NoIntroStaleness::Superseded,
            reason: "a newer valid snapshot covers the same family/platform".into(),
        };
    }
    if pack.members.iter().all(|m| m.upstream_version.is_some()) {
        NoIntroStalenessReport {
            pack_sha256: pack_sha256.into(),
            state: NoIntroStaleness::Current,
            reason: "current for its covered families".into(),
        }
    } else {
        NoIntroStalenessReport {
            pack_sha256: pack_sha256.into(),
            state: NoIntroStaleness::UnknownFreshness,
            reason: "DAT version/date metadata is absent".into(),
        }
    }
}

pub fn plan_no_intro_rollback(
    snapshots: &[NoIntroPackSnapshot],
    current_pack_sha256: &str,
) -> Option<NoIntroRollbackPlan> {
    let current = snapshots
        .iter()
        .find(|s| s.pack_sha256 == current_pack_sha256)?;
    let resolution = resolve_no_intro_current(snapshots);
    let prior = snapshots
        .iter()
        .filter(|s| {
            s.pack_sha256 != current_pack_sha256
                && s.status != NoIntroPackStatus::Invalid
                && s.import_order < current.import_order
        })
        .max_by_key(|s| s.import_order)?;
    if !resolution
        .current
        .values()
        .any(|v| v == current_pack_sha256)
    {
        return None;
    }
    Some(NoIntroRollbackPlan {
        from_pack_sha256: current_pack_sha256.into(),
        to_pack_sha256: prior.pack_sha256.clone(),
        snapshot_sha256: prior.snapshot_sha256.clone(),
        reason: "restore the previous valid retained snapshot; payloads remain untouched".into(),
    })
}

pub fn classify_no_intro_retention(
    snapshots: &[NoIntroPackSnapshot],
) -> Vec<NoIntroRetentionDecision> {
    let resolution = resolve_no_intro_current(snapshots);
    let rollback = snapshots
        .iter()
        .filter(|s| s.status != NoIntroPackStatus::Invalid)
        .max_by_key(|s| s.import_order)
        .and_then(|s| plan_no_intro_rollback(snapshots, &s.pack_sha256))
        .map(|p| p.to_pack_sha256);
    snapshots
        .iter()
        .map(|s| {
            let class = if resolution.current.values().any(|v| v == &s.pack_sha256) {
                NoIntroRetention::KeepCurrent
            } else if rollback.as_deref() == Some(&s.pack_sha256) {
                NoIntroRetention::KeepRollback
            } else if resolution.superseded.contains(&s.pack_sha256)
                && s.snapshot_sha256.len() == 64
            {
                NoIntroRetention::SafePruneCandidate
            } else if resolution
                .unresolved
                .iter()
                .any(|k| s.coverage.iter().any(|c| k.contains(&c.family)))
            {
                NoIntroRetention::UnknownKeep
            } else {
                NoIntroRetention::SupersededRetained
            };
            NoIntroRetentionDecision {
                pack_sha256: s.pack_sha256.clone(),
                classification: class,
                reason: "read-only classification; no pruning is performed".into(),
            }
        })
        .collect()
}

pub fn lifecycle_path(root: &Path) -> PathBuf {
    root.join(LIFECYCLE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        pack: &str,
        order: u64,
        family: &str,
        version: Option<&str>,
    ) -> NoIntroPackSnapshot {
        let member = NoIntroPackMemberEvidence {
            source_member_name: format!("{pack}.dat"),
            system_name: family.into(),
            variant: NoIntroVariant::Headerless,
            artifact_sha256: format!("{pack:0>64}"),
            upstream_version: version.map(str::to_string),
        };
        let coverage = NoIntroPackCoverage {
            canonical_platform: platform_for_system(family),
            family: family.into(),
            dat_member_identity: member.artifact_sha256.clone(),
            variant: member.variant,
            source_member_name: member.source_member_name.clone(),
        };
        NoIntroPackSnapshot {
            pack_sha256: format!("{pack:0>64}"),
            snapshot_sha256: format!("{pack:0>64}"),
            imported_at_unix_seconds: order,
            import_order: order,
            status: NoIntroPackStatus::Current,
            members: vec![member],
            coverage: vec![coverage],
            predecessor: None,
            successor: None,
            rejected: Vec::new(),
        }
    }

    #[test]
    fn authoritative_resolution_is_insertion_order_independent() {
        let old = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let new = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        let forward = resolve_no_intro_current(&[old.clone(), new.clone()]);
        let reverse = resolve_no_intro_current(&[new, old]);
        assert_eq!(forward.current, reverse.current);
        assert!(forward.current.values().any(|value| value.ends_with('b')));
    }

    #[test]
    fn unknown_freshness_is_not_stale_and_coverage_is_explicit() {
        let unknown = snapshot("a", 1, "Nintendo - Unreleased Handheld", None);
        let report = no_intro_staleness(std::slice::from_ref(&unknown), &unknown.pack_sha256);
        assert_eq!(report.state, NoIntroStaleness::UnknownFreshness);
        assert_eq!(unknown.coverage[0].canonical_platform, None);
    }

    #[test]
    fn newer_snapshot_supersedes_only_the_overlapping_family() {
        let old_gb = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let old_snes = snapshot(
            "c",
            1,
            "Nintendo - Super Nintendo Entertainment System",
            Some("20240101"),
        );
        let new_gb = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        let resolution = resolve_no_intro_current(&[old_gb, old_snes.clone(), new_gb]);
        assert!(
            resolution
                .superseded
                .iter()
                .any(|value| value.ends_with('a'))
        );
        assert!(
            !resolution
                .superseded
                .iter()
                .any(|value| value.ends_with('c'))
        );
        assert!(
            resolution
                .current
                .values()
                .any(|value| value.ends_with('b'))
        );
        assert!(
            resolution
                .current
                .values()
                .any(|value| value.ends_with('c'))
        );
    }

    #[test]
    fn rollback_plan_selects_previous_valid_snapshot_without_mutation() {
        let old = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let current = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        let before = vec![old.clone(), current.clone()];
        let plan = plan_no_intro_rollback(&before, &current.pack_sha256).expect("predecessor");
        assert_eq!(plan.to_pack_sha256, old.pack_sha256);
        assert_eq!(plan.snapshot_sha256, old.snapshot_sha256);
        assert_eq!(before, vec![old, current]);
    }

    #[test]
    fn rollback_is_unavailable_without_a_valid_predecessor() {
        let current = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        assert!(
            plan_no_intro_rollback(std::slice::from_ref(&current), &current.pack_sha256).is_none()
        );
        let invalid = NoIntroPackSnapshot {
            status: NoIntroPackStatus::Invalid,
            ..snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"))
        };
        assert!(
            plan_no_intro_rollback(&[invalid, current.clone()], &current.pack_sha256).is_none()
        );
    }

    #[test]
    fn retention_protects_current_and_immediate_rollback_predecessor() {
        let old = snapshot("a", 1, "Nintendo - Game Boy", Some("20230101"));
        let predecessor = snapshot("b", 2, "Nintendo - Game Boy", Some("20240101"));
        let current = snapshot("c", 3, "Nintendo - Game Boy", Some("20250101"));
        let decisions =
            classify_no_intro_retention(&[old.clone(), predecessor.clone(), current.clone()]);
        let class = |pack: &NoIntroPackSnapshot| {
            decisions
                .iter()
                .find(|d| d.pack_sha256 == pack.pack_sha256)
                .unwrap()
                .classification
        };
        assert_eq!(class(&current), NoIntroRetention::KeepCurrent);
        assert_eq!(class(&predecessor), NoIntroRetention::KeepRollback);
        assert_eq!(class(&old), NoIntroRetention::SafePruneCandidate);
    }

    #[test]
    fn retention_never_deletes_or_prunes_payload_evidence() {
        let old = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let current = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        let before = vec![old.clone(), current.clone()];
        let _ = classify_no_intro_retention(&before);
        assert_eq!(before, vec![old, current]);
    }

    #[test]
    fn incomparable_versions_remain_unresolved() {
        let left = snapshot("a", 1, "Nintendo - Game Boy", Some("v2024-beta"));
        let right = snapshot("b", 2, "Nintendo - Game Boy", Some("nightly"));
        let resolution = resolve_no_intro_current(&[left, right]);
        assert!(resolution.current.is_empty());
        assert_eq!(resolution.unresolved.len(), 1);
    }

    #[test]
    fn conflicting_variants_do_not_create_two_current_choices() {
        let mut headerless = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let mut headered = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        headered.coverage[0].variant = NoIntroVariant::Headered;
        headered.members[0].variant = NoIntroVariant::Headered;
        headerless.coverage[0].variant = NoIntroVariant::Headerless;
        let resolution = resolve_no_intro_current(&[headerless, headered]);
        assert_eq!(resolution.current.len(), 2);
    }

    #[test]
    fn partial_coverage_keeps_unrelated_platform_current() {
        let old_gb = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let old_snes = snapshot(
            "c",
            1,
            "Nintendo - Super Nintendo Entertainment System",
            Some("20240101"),
        );
        let new_gb = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        let resolution = resolve_no_intro_current(&[old_gb, old_snes, new_gb]);
        assert!(
            resolution
                .current
                .values()
                .any(|value| value.ends_with('b'))
        );
        assert!(
            resolution
                .current
                .values()
                .any(|value| value.ends_with('c'))
        );
    }

    #[test]
    fn duplicate_registry_evidence_has_one_current_and_is_deterministic() {
        let first = snapshot("a", 1, "Nintendo - Game Boy", Some("20250101"));
        let second = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        let forward = resolve_no_intro_current(&[first.clone(), second.clone()]);
        let reverse = resolve_no_intro_current(&[second, first]);
        assert!(forward.current.is_empty());
        assert!(forward.unresolved.iter().next().is_some());
        assert_eq!(forward, reverse);
    }
}
