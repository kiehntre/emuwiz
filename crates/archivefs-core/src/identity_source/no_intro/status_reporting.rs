//! Read-only status and reporting for the managed No-Intro lifecycle.
//!
//! This module is a view over the persisted lifecycle registry. It does not
//! read DAT/ZIP payloads, contact a network, repair state, or expose an apply
//! operation. All ordering is explicit so a report is stable across runs.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::import::NoIntroVariant;
use super::managed_lifecycle::{
    NoIntroPackCoverage, NoIntroPackSnapshot, NoIntroPackStatus, NoIntroRetention,
    classify_no_intro_retention, plan_no_intro_rollback, resolve_no_intro_current,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NoIntroFreshness {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NoIntroLifecycleHealth {
    Healthy,
    Stale,
    Unknown,
    Conflict,
    Invalid,
    NoCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedNoIntroCoverageStatus {
    pub canonical_platform: Option<String>,
    pub family: String,
    pub variant: NoIntroVariant,
    pub dat_member_sha256: String,
    pub source_member_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedNoIntroMemberStatus {
    pub source_member_name: String,
    pub system_name: String,
    pub variant: NoIntroVariant,
    pub dat_member_sha256: String,
    pub upstream_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedNoIntroPlatformStatus {
    /// Stable grouping key. Known platforms use their canonical id; unknown
    /// coverage uses `unknown:<family>` and remains visible in the report.
    pub platform_key: String,
    pub canonical_platform: Option<String>,
    pub current_snapshot_sha256: Option<String>,
    pub current_pack_sha256: Option<String>,
    pub dat_member_sha256: Vec<String>,
    pub imported_at_unix_seconds: Option<u64>,
    pub upstream_versions: Vec<String>,
    pub freshness: NoIntroFreshness,
    pub health: NoIntroLifecycleHealth,
    pub previous_snapshot_sha256: Option<String>,
    pub rollback_available: bool,
    pub rollback_target_snapshot_sha256: Option<String>,
    pub rollback_reason: String,
    pub coverage: Vec<ManagedNoIntroCoverageStatus>,
    pub lifecycle_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedNoIntroSnapshotStatus {
    pub snapshot_sha256: String,
    pub pack_sha256: String,
    pub current_for_platforms: Vec<String>,
    pub superseded_for_platforms: Vec<String>,
    pub historical_only: bool,
    pub protected_as_rollback_predecessor: bool,
    pub retention: NoIntroRetention,
    pub unresolved: bool,
    pub invalid: bool,
    pub imported_at_unix_seconds: u64,
    pub import_order: u64,
    pub members: Vec<ManagedNoIntroMemberStatus>,
    pub coverage: Vec<ManagedNoIntroCoverageStatus>,
    pub lifecycle_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ManagedNoIntroStatusSummary {
    pub managed_snapshots: usize,
    pub platforms_covered: usize,
    pub current: usize,
    pub stale: usize,
    pub freshness_unknown: usize,
    pub conflicts: usize,
    pub invalid: usize,
    pub no_current: usize,
    pub rollback_available: usize,
    pub historical_snapshots: usize,
    pub retention_candidates: usize,
    pub known_platforms: usize,
    pub unknown_platforms: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedNoIntroStatusReport {
    pub health: NoIntroLifecycleHealth,
    pub platforms: Vec<ManagedNoIntroPlatformStatus>,
    pub snapshots: Vec<ManagedNoIntroSnapshotStatus>,
    pub summary: ManagedNoIntroStatusSummary,
    pub lifecycle_warnings: Vec<String>,
}

fn platform_key(coverage: &NoIntroPackCoverage) -> String {
    coverage
        .canonical_platform
        .clone()
        .unwrap_or_else(|| format!("unknown:{}", coverage.family))
}

fn coverage_status(coverage: &NoIntroPackCoverage) -> ManagedNoIntroCoverageStatus {
    ManagedNoIntroCoverageStatus {
        canonical_platform: coverage.canonical_platform.clone(),
        family: coverage.family.clone(),
        variant: coverage.variant,
        dat_member_sha256: coverage.dat_member_identity.clone(),
        source_member_name: coverage.source_member_name.clone(),
    }
}

fn warning_sort(warnings: &mut Vec<String>) {
    warnings.sort();
    warnings.dedup();
}

fn registry_warnings(snapshots: &[NoIntroPackSnapshot]) -> BTreeMap<String, Vec<String>> {
    let mut warnings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut packs = BTreeMap::<&str, usize>::new();
    let mut snapshot_ids = BTreeMap::<&str, usize>::new();
    for snapshot in snapshots {
        if snapshot.status == NoIntroPackStatus::Invalid {
            warnings
                .entry(snapshot.pack_sha256.clone())
                .or_default()
                .push("snapshot is explicitly marked invalid".into());
        }
        if snapshot.pack_sha256.is_empty() || snapshot.snapshot_sha256.is_empty() {
            warnings
                .entry(snapshot.pack_sha256.clone())
                .or_default()
                .push("snapshot or pack identity is empty".into());
        }
        if *packs.entry(&snapshot.pack_sha256).or_default() > 0 {
            warnings
                .entry(snapshot.pack_sha256.clone())
                .or_default()
                .push("duplicate pack identity is registered".into());
        }
        *packs.entry(&snapshot.pack_sha256).or_default() += 1;
        if *snapshot_ids.entry(&snapshot.snapshot_sha256).or_default() > 0 {
            warnings
                .entry(snapshot.pack_sha256.clone())
                .or_default()
                .push("duplicate snapshot identity is registered".into());
        }
        *snapshot_ids.entry(&snapshot.snapshot_sha256).or_default() += 1;
        if snapshot.predecessor.as_deref() == Some(snapshot.pack_sha256.as_str())
            || snapshot.successor.as_deref() == Some(snapshot.pack_sha256.as_str())
        {
            warnings
                .entry(snapshot.pack_sha256.clone())
                .or_default()
                .push("snapshot self-supersession is invalid".into());
        }
        if let Some(predecessor) = &snapshot.predecessor
            && !snapshots.iter().any(|s| &s.pack_sha256 == predecessor)
        {
            warnings
                .entry(snapshot.pack_sha256.clone())
                .or_default()
                .push(format!("dangling predecessor reference: {predecessor}"));
        }
        if let Some(successor) = &snapshot.successor
            && !snapshots.iter().any(|s| &s.pack_sha256 == successor)
        {
            warnings
                .entry(snapshot.pack_sha256.clone())
                .or_default()
                .push(format!("dangling successor reference: {successor}"));
        }
        for coverage in &snapshot.coverage {
            if !snapshot.members.iter().any(|member| {
                member.source_member_name == coverage.source_member_name
                    && member.variant == coverage.variant
            }) {
                warnings
                    .entry(snapshot.pack_sha256.clone())
                    .or_default()
                    .push(format!(
                        "coverage references unknown member: {}",
                        coverage.source_member_name
                    ));
            } else if !snapshot.members.iter().any(|member| {
                member.source_member_name == coverage.source_member_name
                    && member.variant == coverage.variant
                    && member.artifact_sha256 == coverage.dat_member_identity
            }) {
                warnings
                    .entry(snapshot.pack_sha256.clone())
                    .or_default()
                    .push(format!(
                        "coverage identity does not match member: {}",
                        coverage.source_member_name
                    ));
            }
        }
    }
    for values in warnings.values_mut() {
        warning_sort(values);
    }
    warnings
}

fn invalid_snapshot(
    snapshot: &NoIntroPackSnapshot,
    warnings: &BTreeMap<String, Vec<String>>,
) -> bool {
    snapshot.status == NoIntroPackStatus::Invalid
        || warnings
            .get(&snapshot.pack_sha256)
            .is_some_and(|items| !items.is_empty())
}

/// Builds a deterministic, read-only view of the managed lifecycle registry.
pub fn report_no_intro_lifecycle(snapshots: &[NoIntroPackSnapshot]) -> ManagedNoIntroStatusReport {
    let warnings_by_pack = registry_warnings(snapshots);
    let resolution = resolve_no_intro_current(snapshots);
    let retention = classify_no_intro_retention(snapshots);
    let retention_by_pack: BTreeMap<_, _> = retention
        .iter()
        .map(|decision| (decision.pack_sha256.as_str(), decision))
        .collect();

    let mut grouped: BTreeMap<String, Vec<(&NoIntroPackSnapshot, &NoIntroPackCoverage)>> =
        BTreeMap::new();
    for snapshot in snapshots {
        for coverage in &snapshot.coverage {
            grouped
                .entry(platform_key(coverage))
                .or_default()
                .push((snapshot, coverage));
        }
    }

    let mut platforms = Vec::new();
    for (key, entries) in grouped {
        let canonical_platform = entries[0].1.canonical_platform.clone();
        let current_ids: BTreeSet<_> = entries
            .iter()
            .filter_map(|(_, coverage)| {
                let resolution_key = format!(
                    "{}|{}|{:?}",
                    coverage.canonical_platform.as_deref().unwrap_or("unknown"),
                    coverage.family,
                    coverage.variant
                );
                resolution.current.get(&resolution_key).cloned()
            })
            .collect();
        let unresolved = resolution.unresolved.iter().any(|unresolved_key| {
            entries.iter().any(|(_, coverage)| {
                unresolved_key.contains(&coverage.family)
                    && unresolved_key.contains(&format!("{:?}", coverage.variant))
            })
        });
        let current_id =
            (current_ids.len() == 1).then(|| current_ids.iter().next().unwrap().clone());
        let current = current_id
            .as_deref()
            .and_then(|id| snapshots.iter().find(|snapshot| snapshot.pack_sha256 == id));
        // Staleness is evaluated at the coverage/platform level here. A pack
        // can remain current for one platform while a newer partial pack
        // supersedes only another platform in the same payload.
        let current_state = current.map(|snapshot| {
            if snapshot.status == NoIntroPackStatus::Invalid {
                NoIntroFreshness::Stale
            } else if entries
                .iter()
                .filter(|(candidate, coverage)| {
                    candidate.pack_sha256 == snapshot.pack_sha256
                        && resolution.current.contains_key(&format!(
                            "{}|{}|{:?}",
                            coverage.canonical_platform.as_deref().unwrap_or("unknown"),
                            coverage.family,
                            coverage.variant
                        ))
                })
                .all(|(_, coverage)| {
                    snapshot.members.iter().any(|member| {
                        member.source_member_name == coverage.source_member_name
                            && member.variant == coverage.variant
                            && member.upstream_version.is_some()
                    })
                })
            {
                NoIntroFreshness::Fresh
            } else {
                NoIntroFreshness::Unknown
            }
        });
        let health = if snapshots
            .iter()
            .any(|snapshot| invalid_snapshot(snapshot, &warnings_by_pack))
            && current.is_none()
        {
            NoIntroLifecycleHealth::Invalid
        } else if unresolved || current_ids.len() > 1 {
            NoIntroLifecycleHealth::Conflict
        } else if current.is_none() {
            NoIntroLifecycleHealth::NoCurrent
        } else {
            match current_state.unwrap_or(NoIntroFreshness::Unknown) {
                NoIntroFreshness::Fresh => NoIntroLifecycleHealth::Healthy,
                NoIntroFreshness::Stale => NoIntroLifecycleHealth::Stale,
                NoIntroFreshness::Unknown => NoIntroLifecycleHealth::Unknown,
            }
        };
        let rollback =
            current.and_then(|snapshot| plan_no_intro_rollback(snapshots, &snapshot.pack_sha256));
        let mut coverage = entries
            .iter()
            .map(|(_, item)| coverage_status(item))
            .collect::<Vec<_>>();
        coverage.sort_by(|a, b| {
            a.family
                .cmp(&b.family)
                .then_with(|| format!("{:?}", a.variant).cmp(&format!("{:?}", b.variant)))
        });
        coverage.dedup();
        let mut member_hashes = coverage
            .iter()
            .map(|item| item.dat_member_sha256.clone())
            .collect::<Vec<_>>();
        member_hashes.sort();
        member_hashes.dedup();
        let mut versions = current
            .into_iter()
            .flat_map(|snapshot| {
                snapshot
                    .members
                    .iter()
                    .filter_map(|member| member.upstream_version.clone())
            })
            .collect::<Vec<_>>();
        versions.sort();
        versions.dedup();
        let mut platform_warnings = entries
            .iter()
            .flat_map(|(snapshot, coverage)| {
                warnings_by_pack
                    .get(&snapshot.pack_sha256)
                    .into_iter()
                    .flatten()
                    .map(move |warning| format!("{}: {warning}", coverage.family))
            })
            .collect::<Vec<_>>();
        if unresolved {
            platform_warnings.push("current snapshot cannot be resolved uniquely".into());
        }
        warning_sort(&mut platform_warnings);
        platforms.push(ManagedNoIntroPlatformStatus {
            platform_key: key,
            canonical_platform,
            current_snapshot_sha256: current.map(|snapshot| snapshot.snapshot_sha256.clone()),
            current_pack_sha256: current.map(|snapshot| snapshot.pack_sha256.clone()),
            dat_member_sha256: member_hashes,
            imported_at_unix_seconds: current.map(|snapshot| snapshot.imported_at_unix_seconds),
            upstream_versions: versions,
            freshness: current_state.unwrap_or(NoIntroFreshness::Unknown),
            health,
            previous_snapshot_sha256: rollback.as_ref().map(|plan| plan.snapshot_sha256.clone()),
            rollback_available: rollback.is_some(),
            rollback_target_snapshot_sha256: rollback.map(|plan| plan.snapshot_sha256),
            rollback_reason: current
                .and_then(|snapshot| plan_no_intro_rollback(snapshots, &snapshot.pack_sha256))
                .map(|plan| plan.reason)
                .unwrap_or_else(|| "no safe rollback predecessor is available".into()),
            coverage,
            lifecycle_warnings: platform_warnings,
        });
    }

    let current_for: BTreeMap<String, BTreeSet<String>> = platforms
        .iter()
        .filter_map(|platform| {
            platform
                .current_pack_sha256
                .as_ref()
                .map(|pack| (pack.clone(), platform.platform_key.clone()))
        })
        .fold(BTreeMap::new(), |mut map, (pack, platform)| {
            map.entry(pack).or_default().insert(platform);
            map
        });
    let superseded_for: BTreeMap<String, BTreeSet<String>> = resolution
        .superseded
        .iter()
        .map(|pack| {
            let platforms = platforms
                .iter()
                .filter(|platform| platform.current_pack_sha256.as_deref() != Some(pack))
                .filter(|platform| {
                    platform.coverage.iter().any(|coverage| {
                        snapshots.iter().any(|snapshot| {
                            &snapshot.pack_sha256 == pack
                                && snapshot
                                    .coverage
                                    .iter()
                                    .any(|item| item.family == coverage.family)
                        })
                    })
                })
                .map(|platform| platform.platform_key.clone())
                .collect();
            (pack.clone(), platforms)
        })
        .collect();

    let mut snapshot_statuses = snapshots
        .iter()
        .map(|snapshot| {
            let mut snapshot_warnings = warnings_by_pack
                .get(&snapshot.pack_sha256)
                .cloned()
                .unwrap_or_default();
            let current_platforms = current_for
                .get(&snapshot.pack_sha256)
                .cloned()
                .unwrap_or_default();
            let current_platforms_empty = current_platforms.is_empty();
            let superseded_platforms = superseded_for
                .get(&snapshot.pack_sha256)
                .cloned()
                .unwrap_or_default();
            let retention_decision = retention_by_pack.get(snapshot.pack_sha256.as_str());
            let retention_class = retention_decision
                .map_or(NoIntroRetention::UnknownKeep, |decision| {
                    decision.classification
                });
            let members = snapshot
                .members
                .iter()
                .map(|member| ManagedNoIntroMemberStatus {
                    source_member_name: member.source_member_name.clone(),
                    system_name: member.system_name.clone(),
                    variant: member.variant,
                    dat_member_sha256: member.artifact_sha256.clone(),
                    upstream_version: member.upstream_version.clone(),
                })
                .collect();
            let mut coverage = snapshot
                .coverage
                .iter()
                .map(coverage_status)
                .collect::<Vec<_>>();
            coverage.sort_by(|a, b| {
                a.family
                    .cmp(&b.family)
                    .then_with(|| format!("{:?}", a.variant).cmp(&format!("{:?}", b.variant)))
            });
            snapshot_warnings.extend(
                retention_decision
                    .into_iter()
                    .map(|decision| decision.reason.clone()),
            );
            warning_sort(&mut snapshot_warnings);
            ManagedNoIntroSnapshotStatus {
                snapshot_sha256: snapshot.snapshot_sha256.clone(),
                pack_sha256: snapshot.pack_sha256.clone(),
                current_for_platforms: current_platforms.into_iter().collect(),
                superseded_for_platforms: superseded_platforms.clone().into_iter().collect(),
                historical_only: current_platforms_empty && !superseded_platforms.is_empty(),
                protected_as_rollback_predecessor: retention_class
                    == NoIntroRetention::KeepRollback,
                retention: retention_class,
                unresolved: resolution.unresolved.iter().any(|key| {
                    snapshot
                        .coverage
                        .iter()
                        .any(|coverage| key.contains(&coverage.family))
                }),
                invalid: invalid_snapshot(snapshot, &warnings_by_pack),
                imported_at_unix_seconds: snapshot.imported_at_unix_seconds,
                import_order: snapshot.import_order,
                members,
                coverage,
                lifecycle_warnings: snapshot_warnings,
            }
        })
        .collect::<Vec<_>>();
    snapshot_statuses.sort_by(|a, b| {
        a.import_order
            .cmp(&b.import_order)
            .then_with(|| a.snapshot_sha256.cmp(&b.snapshot_sha256))
    });

    let mut summary = ManagedNoIntroStatusSummary {
        managed_snapshots: snapshots.len(),
        platforms_covered: platforms.len(),
        current: platforms
            .iter()
            .filter(|platform| platform.health == NoIntroLifecycleHealth::Healthy)
            .count(),
        stale: platforms
            .iter()
            .filter(|platform| platform.health == NoIntroLifecycleHealth::Stale)
            .count(),
        freshness_unknown: platforms
            .iter()
            .filter(|platform| platform.health == NoIntroLifecycleHealth::Unknown)
            .count(),
        conflicts: platforms
            .iter()
            .filter(|platform| platform.health == NoIntroLifecycleHealth::Conflict)
            .count(),
        invalid: platforms
            .iter()
            .filter(|platform| platform.health == NoIntroLifecycleHealth::Invalid)
            .count(),
        no_current: platforms
            .iter()
            .filter(|platform| platform.health == NoIntroLifecycleHealth::NoCurrent)
            .count(),
        rollback_available: platforms
            .iter()
            .filter(|platform| platform.rollback_available)
            .count(),
        historical_snapshots: snapshot_statuses
            .iter()
            .filter(|snapshot| snapshot.historical_only)
            .count(),
        retention_candidates: snapshot_statuses
            .iter()
            .filter(|snapshot| snapshot.retention == NoIntroRetention::SafePruneCandidate)
            .count(),
        known_platforms: platforms
            .iter()
            .filter(|platform| platform.canonical_platform.is_some())
            .count(),
        unknown_platforms: platforms
            .iter()
            .filter(|platform| platform.canonical_platform.is_none())
            .count(),
    };
    let mut report_warnings = warnings_by_pack
        .values()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    warning_sort(&mut report_warnings);
    let health = if !report_warnings.is_empty()
        || summary.invalid > 0
        || snapshots
            .iter()
            .any(|snapshot| invalid_snapshot(snapshot, &warnings_by_pack))
    {
        NoIntroLifecycleHealth::Invalid
    } else if summary.conflicts > 0 {
        NoIntroLifecycleHealth::Conflict
    } else if summary.no_current > 0 {
        NoIntroLifecycleHealth::NoCurrent
    } else if summary.stale > 0 {
        NoIntroLifecycleHealth::Stale
    } else if summary.freshness_unknown > 0 {
        NoIntroLifecycleHealth::Unknown
    } else {
        NoIntroLifecycleHealth::Healthy
    };
    summary.managed_snapshots = snapshot_statuses.len();
    ManagedNoIntroStatusReport {
        health,
        platforms,
        snapshots: snapshot_statuses,
        summary,
        lifecycle_warnings: report_warnings,
    }
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
        let artifact = format!("{pack:0>64}");
        let member_name = format!("{pack}.dat");
        NoIntroPackSnapshot {
            pack_sha256: artifact.clone(),
            snapshot_sha256: format!("{pack:1>64}"),
            imported_at_unix_seconds: order,
            import_order: order,
            status: NoIntroPackStatus::Current,
            members: vec![super::super::managed_lifecycle::NoIntroPackMemberEvidence {
                source_member_name: member_name.clone(),
                system_name: family.into(),
                variant: NoIntroVariant::Headerless,
                artifact_sha256: artifact.clone(),
                upstream_version: version.map(str::to_string),
            }],
            coverage: vec![NoIntroPackCoverage {
                canonical_platform: crate::canonical_platform_for_alias(
                    family.strip_prefix("Nintendo - ").unwrap_or(family),
                )
                .map(str::to_string),
                family: family.into(),
                dat_member_identity: artifact,
                variant: NoIntroVariant::Headerless,
                source_member_name: member_name,
            }],
            predecessor: None,
            successor: None,
            rejected: Vec::new(),
        }
    }

    #[test]
    fn single_current_is_fresh_and_has_no_rollback() {
        let report =
            report_no_intro_lifecycle(&[snapshot("a", 1, "Nintendo - Game Boy", Some("20250101"))]);
        assert_eq!(report.health, NoIntroLifecycleHealth::Healthy);
        assert_eq!(report.summary.current, 1);
        assert!(!report.platforms[0].rollback_available);
    }

    #[test]
    fn predecessor_and_retention_are_reported_without_mutation() {
        let old = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let current = snapshot("b", 2, "Nintendo - Game Boy", Some("20250101"));
        let before = vec![old.clone(), current.clone()];
        let report = report_no_intro_lifecycle(&before);
        assert!(report.platforms[0].rollback_available);
        assert_eq!(
            report.platforms[0].rollback_target_snapshot_sha256,
            Some(old.snapshot_sha256.clone())
        );
        assert!(
            report
                .snapshots
                .iter()
                .any(|snapshot| snapshot.protected_as_rollback_predecessor)
        );
        assert_eq!(before, vec![old, current]);
    }

    #[test]
    fn unknown_platform_and_freshness_remain_explicit() {
        let report =
            report_no_intro_lifecycle(&[snapshot("a", 1, "Nintendo - Unreleased Handheld", None)]);
        assert_eq!(report.summary.unknown_platforms, 1);
        assert_eq!(report.platforms[0].canonical_platform, None);
        assert_eq!(report.platforms[0].freshness, NoIntroFreshness::Unknown);
        assert_eq!(report.health, NoIntroLifecycleHealth::Unknown);
    }

    #[test]
    fn duplicate_equal_versions_are_a_conflict_not_repaired() {
        let report = report_no_intro_lifecycle(&[
            snapshot("a", 1, "Nintendo - Game Boy", Some("20250101")),
            snapshot("b", 2, "Nintendo - Game Boy", Some("20250101")),
        ]);
        assert_eq!(report.health, NoIntroLifecycleHealth::Conflict);
        assert_eq!(report.summary.conflicts, 1);
        assert!(report.platforms[0].health == NoIntroLifecycleHealth::Conflict);
    }

    #[test]
    fn dangling_and_mismatched_registry_evidence_is_invalid() {
        let mut bad = snapshot("a", 1, "Nintendo - Game Boy", Some("20250101"));
        bad.predecessor = Some("missing".into());
        bad.coverage[0].dat_member_identity = "wrong".into();
        let report = report_no_intro_lifecycle(&[bad]);
        assert_eq!(report.health, NoIntroLifecycleHealth::Invalid);
        assert!(report.snapshots[0].invalid);
        assert!(
            report
                .lifecycle_warnings
                .iter()
                .any(|warning| warning.contains("dangling predecessor"))
        );
    }

    #[test]
    fn report_is_insertion_order_independent_and_does_not_reparse_payloads() {
        let first = snapshot(
            "b",
            2,
            "Nintendo - Super Nintendo Entertainment System",
            Some("20250101"),
        );
        let second = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let forward = report_no_intro_lifecycle(&[first.clone(), second.clone()]);
        let reverse = report_no_intro_lifecycle(&[second, first]);
        assert_eq!(forward, reverse);
        assert!(
            forward
                .platforms
                .iter()
                .all(|platform| platform.lifecycle_warnings.is_empty())
        );
    }

    #[test]
    fn partial_pack_has_independent_platform_currents_and_no_zip_dependency() {
        let mut pack = snapshot("a", 1, "Nintendo - Game Boy", Some("20240101"));
        let mut old_snes = snapshot(
            "c",
            1,
            "Nintendo - Super Nintendo Entertainment System",
            Some("20240101"),
        );
        let snes = snapshot(
            "b",
            2,
            "Nintendo - Super Nintendo Entertainment System",
            Some("20250101"),
        );
        old_snes.pack_sha256 = pack.pack_sha256.clone();
        old_snes.snapshot_sha256 = pack.snapshot_sha256.clone();
        pack.members.push(old_snes.members[0].clone());
        pack.coverage.push(old_snes.coverage[0].clone());
        let report = report_no_intro_lifecycle(&[pack, snes]);
        assert_eq!(report.summary.platforms_covered, 2);
        assert_eq!(report.summary.current, 2);
    }
}
