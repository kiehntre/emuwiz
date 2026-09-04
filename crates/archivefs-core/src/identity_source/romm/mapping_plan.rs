//! Read-only planning for reconciling RomM provider paths with EmuWiz folders.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::identity_source::cache::IdentityCache;
use crate::identity_source::model::ExternalIdentityRecord;
use crate::identity_source::path_map::{MAX_MAPPINGS, PathMapping, PathMappings, normalise_prefix};
use crate::identity_source::romm::normalise::canonical_platform_for_romm_slug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingProposalKind {
    ExactExisting,
    StaleSourceRootReplacement,
    SafeNewMapping,
    Ambiguous,
    NoLocalFolder,
    UnknownPlatform,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommMappingProposal {
    pub provider_prefix: String,
    pub provider_platform_slug: Option<String>,
    pub canonical_platform: Option<String>,
    pub candidate_local_folder: Option<PathBuf>,
    pub candidate_exists: bool,
    pub current_mapping: Option<PathMapping>,
    pub current_destination_exists: bool,
    pub current_destination_inside_source: bool,
    pub inferred_old_root: Option<PathBuf>,
    pub configured_new_source_root: Option<PathBuf>,
    pub proposed_destination: Option<PathBuf>,
    pub record_count: usize,
    pub kind: MappingProposalKind,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommMappingPlan {
    pub proposals: Vec<RommMappingProposal>,
    pub proposed_mappings: Vec<PathMapping>,
    pub current_translatable: usize,
    pub rescued_by_replacement: usize,
    pub rescued_by_new_mapping: usize,
    pub still_unmapped: usize,
    pub unknown_platforms: usize,
    pub ambiguous_or_conflicting: usize,
}

/// Builds a bounded, filesystem-observing but otherwise read-only mapping plan.
/// It uses platform identity and immediate folder aliases only; it never matches
/// titles, hashes files, or changes configuration.
pub fn plan_mapping_reconciliation(
    cache: &IdentityCache,
    current: &PathMappings,
    source_roots: &[PathBuf],
) -> RommMappingPlan {
    let mut groups: BTreeMap<String, (Option<String>, usize)> = BTreeMap::new();
    for record in &cache.records {
        let Some(prefix) = provider_platform_prefix(record) else {
            continue;
        };
        let entry = groups
            .entry(prefix)
            .or_insert_with(|| (record.provider_platform_name.clone(), 0));
        entry.1 += 1;
    }

    let mut proposals = Vec::new();
    let mut proposed_mappings = current.as_slice().to_vec();
    let mut plan = RommMappingPlan {
        proposals: Vec::new(),
        proposed_mappings: Vec::new(),
        current_translatable: 0,
        rescued_by_replacement: 0,
        rescued_by_new_mapping: 0,
        still_unmapped: 0,
        unknown_platforms: 0,
        ambiguous_or_conflicting: 0,
    };

    for (provider_prefix, (slug, record_count)) in groups {
        let canonical = slug
            .as_deref()
            .and_then(canonical_platform_for_romm_slug)
            .map(str::to_string);
        let current_mapping = current
            .as_slice()
            .iter()
            .find(|mapping| {
                normalise_prefix(&mapping.provider_prefix, current.kind()).ok()
                    == Some(provider_prefix.clone())
            })
            .cloned();
        let candidates = canonical
            .as_deref()
            .map(|platform| candidate_folders(platform, source_roots))
            .unwrap_or_default();
        let candidate = (candidates.len() == 1).then(|| candidates[0].clone());
        let current_destination_exists = current_mapping
            .as_ref()
            .is_some_and(|mapping| mapping.archivefs_prefix.exists());
        let current_destination_inside_source = current_mapping
            .as_ref()
            .is_some_and(|mapping| is_inside_any(&mapping.archivefs_prefix, source_roots));
        let configured_new_source_root = candidate
            .as_deref()
            .and_then(|path| containing_root(path, source_roots));
        let inferred_old_root = current_mapping
            .as_ref()
            .and_then(|mapping| mapping.archivefs_prefix.parent().map(Path::to_path_buf));
        let mut kind = match (
            &canonical,
            &slug,
            candidates.len(),
            &current_mapping,
            &candidate,
        ) {
            (None, Some(_), _, _, _) => MappingProposalKind::UnknownPlatform,
            (_, _, n, _, _) if n > 1 => MappingProposalKind::Ambiguous,
            (_, _, 0, _, _) => MappingProposalKind::NoLocalFolder,
            (Some(_), _, 1, Some(existing), Some(candidate))
                if existing.archivefs_prefix == *candidate =>
            {
                MappingProposalKind::ExactExisting
            }
            (Some(_), _, 1, Some(_), Some(candidate))
                if is_inside_any(candidate, source_roots)
                    && (!current_destination_exists || !current_destination_inside_source) =>
            {
                MappingProposalKind::StaleSourceRootReplacement
            }
            (Some(_), _, 1, Some(_), Some(_)) => MappingProposalKind::Conflict,
            (Some(_), _, 1, None, Some(candidate)) if is_inside_any(candidate, source_roots) => {
                MappingProposalKind::SafeNewMapping
            }
            _ => MappingProposalKind::Conflict,
        };
        if kind == MappingProposalKind::SafeNewMapping && proposed_mappings.len() >= MAX_MAPPINGS {
            kind = MappingProposalKind::Conflict;
        }
        let reason = proposal_reason(kind, &current_mapping, candidate.as_deref());
        if matches!(kind, MappingProposalKind::ExactExisting) {
            plan.current_translatable += record_count;
        }
        match kind {
            MappingProposalKind::StaleSourceRootReplacement => {
                plan.rescued_by_replacement += record_count;
                replace_mapping(&mut proposed_mappings, &provider_prefix, candidate.clone());
            }
            MappingProposalKind::SafeNewMapping => {
                plan.rescued_by_new_mapping += record_count;
                proposed_mappings.push(PathMapping {
                    provider_prefix: provider_prefix.clone(),
                    archivefs_prefix: candidate.clone().expect("safe candidate"),
                });
            }
            MappingProposalKind::UnknownPlatform => plan.unknown_platforms += record_count,
            MappingProposalKind::Ambiguous | MappingProposalKind::Conflict => {
                plan.ambiguous_or_conflicting += record_count
            }
            MappingProposalKind::NoLocalFolder => {}
            MappingProposalKind::ExactExisting => {}
        }
        if !matches!(kind, MappingProposalKind::ExactExisting) {
            plan.still_unmapped += match kind {
                MappingProposalKind::StaleSourceRootReplacement
                | MappingProposalKind::SafeNewMapping => 0,
                _ => record_count,
            };
        }
        proposals.push(RommMappingProposal {
            provider_prefix,
            provider_platform_slug: slug,
            canonical_platform: canonical,
            candidate_local_folder: candidate.clone(),
            candidate_exists: candidate.is_some(),
            current_mapping,
            current_destination_exists,
            current_destination_inside_source,
            inferred_old_root,
            configured_new_source_root,
            proposed_destination: candidate,
            record_count,
            kind,
            reason,
        });
    }
    plan.proposals = proposals;
    plan.proposed_mappings = proposed_mappings;
    plan
}

fn provider_platform_prefix(record: &ExternalIdentityRecord) -> Option<String> {
    let mut components = record.provider_path.split('/');
    let root = components.next()?;
    let platform = components.next()?;
    (root == "roms" && !platform.is_empty()).then(|| format!("{root}/{platform}"))
}

fn candidate_folders(platform: &str, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && crate::platform::detect::platform_for_folder_name(
                    &entry.file_name().to_string_lossy(),
                )
                .is_some_and(|found| found.id == platform)
            {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn is_inside_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn containing_root(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .cloned()
}

fn replace_mapping(mappings: &mut Vec<PathMapping>, prefix: &str, candidate: Option<PathBuf>) {
    if let Some(mapping) = mappings
        .iter_mut()
        .find(|mapping| mapping.provider_prefix == prefix)
        && let Some(candidate) = candidate
    {
        mapping.archivefs_prefix = candidate;
    }
}

fn proposal_reason(
    kind: MappingProposalKind,
    current: &Option<PathMapping>,
    candidate: Option<&Path>,
) -> String {
    match kind {
        MappingProposalKind::ExactExisting => "The configured mapping already points at the recognised local platform folder.".to_string(),
        MappingProposalKind::StaleSourceRootReplacement => format!("This mapping appears to point to an older library location. Replace {} with {}.", current.as_ref().map(|m| m.archivefs_prefix.display().to_string()).unwrap_or_default(), candidate.map(|p| p.display().to_string()).unwrap_or_default()),
        MappingProposalKind::SafeNewMapping => "The RomM platform and one existing local platform folder agree, and the folder is inside a configured source.".to_string(),
        MappingProposalKind::Ambiguous => "More than one local folder matches this canonical platform; no mapping was guessed.".to_string(),
        MappingProposalKind::NoLocalFolder => "RomM's platform is known, but no matching local platform folder exists under a configured source.".to_string(),
        MappingProposalKind::UnknownPlatform => "RomM reported a platform EmuWiz does not recognise.".to_string(),
        MappingProposalKind::Conflict => "The candidate conflicts with an existing mapping, so no change was proposed.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::cache::CACHE_FORMAT_VERSION;
    use crate::identity_source::model::IdentityProvider;
    use crate::identity_source::path_map::ProviderPathKind;
    use crate::identity_source::romm::normalise::{NormalisationReport, normalise_rom};
    use serde_json::json;

    fn fixture_cache(platform: &str, path: &str) -> IdentityCache {
        let mappings =
            PathMappings::validate(&[], &[], ProviderPathKind::ProviderRelative).unwrap();
        let mut report = NormalisationReport::default();
        let record = normalise_rom(
            &json!({
                "id": 1,
                "platform_slug": platform,
                "fs_path": format!("roms/{platform}"),
                "fs_name": path.rsplit('/').next().unwrap(),
                "name": "Game"
            }),
            "test",
            &mappings,
            1,
            &mut report,
        )
        .unwrap();
        IdentityCache {
            format_version: CACHE_FORMAT_VERSION,
            provider: IdentityProvider::Romm,
            server_id: "test".to_string(),
            server_version: None,
            source_fingerprint: "test".to_string(),
            imported_at_unix_seconds: 1,
            platforms: Vec::new(),
            records: vec![record],
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: Some(1),
        }
    }

    #[test]
    fn a_known_platform_folder_produces_a_safe_new_mapping() {
        let root = std::env::temp_dir().join("archivefs-mapping-plan-test");
        let folder = root.join("n64");
        std::fs::create_dir_all(&folder).unwrap();
        let cache = fixture_cache("n64", "roms/n64/Game.v64");
        let current = PathMappings::validate(&[], &[], ProviderPathKind::ProviderRelative).unwrap();
        let plan = plan_mapping_reconciliation(&cache, &current, std::slice::from_ref(&root));
        assert_eq!(plan.proposals[0].kind, MappingProposalKind::SafeNewMapping);
        assert_eq!(plan.rescued_by_new_mapping, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_stale_mapping_is_replaced_only_in_the_preview() {
        let root = std::env::temp_dir().join("archivefs-mapping-plan-stale-test");
        let folder = root.join("n64");
        std::fs::create_dir_all(&folder).unwrap();
        let cache = fixture_cache("n64", "roms/n64/Game.v64");
        let current = PathMappings::validate(
            &[PathMapping {
                provider_prefix: "roms/n64".to_string(),
                archivefs_prefix: PathBuf::from("/mnt/games/roms/n64"),
            }],
            &[],
            ProviderPathKind::ProviderRelative,
        )
        .unwrap();
        let plan = plan_mapping_reconciliation(&cache, &current, std::slice::from_ref(&root));
        assert_eq!(
            plan.proposals[0].kind,
            MappingProposalKind::StaleSourceRootReplacement
        );
        assert_eq!(plan.rescued_by_replacement, 1);
        assert_eq!(plan.proposed_mappings[0].archivefs_prefix, folder);
        assert_eq!(
            current.as_slice()[0].archivefs_prefix,
            PathBuf::from("/mnt/games/roms/n64")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_known_stale_platform_mappings_are_replacements() {
        let root = std::env::temp_dir().join("archivefs-mapping-plan-stale-platforms-test");
        for folder in ["n64", "gb", "zxs", "wii"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        let mut cache = IdentityCache {
            format_version: CACHE_FORMAT_VERSION,
            provider: IdentityProvider::Romm,
            server_id: "test".to_string(),
            source_fingerprint: "test".to_string(),
            imported_at_unix_seconds: 1,
            platforms: Vec::new(),
            records: Vec::new(),
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_version: None,
            server_reported_total: Some(4),
        };
        for (id, platform) in [(1, "n64"), (2, "gb"), (3, "zxs"), (4, "wii")] {
            cache.records.push(
                fixture_cache(platform, &format!("roms/{platform}/Game{id}.bin")).records[0]
                    .clone(),
            );
        }
        let current = PathMappings::validate(
            &[
                ("n64", "/mnt/games/roms/n64"),
                ("gb", "/mnt/games/roms/gb"),
                ("zxs", "/mnt/games/roms/zxs"),
                ("wii", "/mnt/games/roms/wii"),
            ]
            .into_iter()
            .map(|(provider, local)| PathMapping {
                provider_prefix: format!("roms/{provider}"),
                archivefs_prefix: PathBuf::from(local),
            })
            .collect::<Vec<_>>()
            .as_slice(),
            &[],
            ProviderPathKind::ProviderRelative,
        )
        .unwrap();
        let plan = plan_mapping_reconciliation(&cache, &current, std::slice::from_ref(&root));
        assert_eq!(
            plan.proposals
                .iter()
                .map(|proposal| proposal.kind)
                .collect::<Vec<_>>(),
            vec![
                MappingProposalKind::StaleSourceRootReplacement,
                MappingProposalKind::StaleSourceRootReplacement,
                MappingProposalKind::StaleSourceRootReplacement,
                MappingProposalKind::StaleSourceRootReplacement,
            ]
        );
        assert_eq!(plan.rescued_by_replacement, 4);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_known_platform_without_a_local_folder_is_not_proposed() {
        let root = std::env::temp_dir().join("archivefs-mapping-plan-missing-folder-test");
        std::fs::create_dir_all(&root).unwrap();
        let cache = fixture_cache("n64", "roms/n64/Game.v64");
        let current = PathMappings::validate(&[], &[], ProviderPathKind::ProviderRelative).unwrap();
        let plan = plan_mapping_reconciliation(&cache, &current, std::slice::from_ref(&root));
        assert_eq!(plan.proposals[0].kind, MappingProposalKind::NoLocalFolder);
        assert!(plan.proposed_mappings.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_existing_old_destination_outside_the_current_source_is_still_a_migration() {
        let root = std::env::temp_dir().join("archivefs-mapping-plan-existing-old-root-test");
        let old_folder = root.join("old-root/n64");
        let new_folder = root.join("library/n64");
        std::fs::create_dir_all(&old_folder).unwrap();
        std::fs::create_dir_all(&new_folder).unwrap();
        let cache = fixture_cache("n64", "roms/n64/Game.v64");
        let current = PathMappings::validate(
            &[PathMapping {
                provider_prefix: "roms/n64".to_string(),
                archivefs_prefix: old_folder.clone(),
            }],
            &[],
            ProviderPathKind::ProviderRelative,
        )
        .unwrap();
        let plan = plan_mapping_reconciliation(
            &cache,
            &current,
            std::slice::from_ref(&root.join("library")),
        );
        let proposal = &plan.proposals[0];
        assert_eq!(
            proposal.kind,
            MappingProposalKind::StaleSourceRootReplacement
        );
        assert!(proposal.current_destination_exists);
        assert!(!proposal.current_destination_inside_source);
        assert_eq!(proposal.inferred_old_root, Some(root.join("old-root")));
        assert_eq!(
            proposal.configured_new_source_root,
            Some(root.join("library"))
        );
        assert_eq!(proposal.proposed_destination, Some(new_folder));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unknown_and_missing_platforms_are_not_guessed() {
        let root = std::env::temp_dir().join("archivefs-mapping-plan-unknown-test");
        std::fs::create_dir_all(&root).unwrap();
        let unknown = fixture_cache("not-a-real-platform", "roms/not-a-real-platform/Game.bin");
        let current = PathMappings::validate(&[], &[], ProviderPathKind::ProviderRelative).unwrap();
        let plan = plan_mapping_reconciliation(&unknown, &current, std::slice::from_ref(&root));
        assert_eq!(plan.proposals[0].kind, MappingProposalKind::UnknownPlatform);
        assert_eq!(plan.unknown_platforms, 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
