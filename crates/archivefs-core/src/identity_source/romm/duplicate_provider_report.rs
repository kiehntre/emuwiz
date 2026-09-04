//! Explaining *why* a local destination is claimed by more than one RomM
//! record - never resolving it.
//!
//! [`PathClaims::contested`](crate::identity_source::matching::PathClaims::contested)
//! already tells a caller *how many* records claim one path, and
//! [`ExternalVerification::Ambiguous`](crate::identity_source::model::ExternalVerification::Ambiguous)
//! already refuses to pick a winner when that happens - both fail closed, as
//! they should. Neither says *why* the collision exists. After the provider-
//! alias fingerprint fix (see `import::source_fingerprint`) landed and a real
//! refresh was run, exactly that gap showed up in practice: 267 real
//! destinations became contested, and the honest question a person asks next
//! is "is this a real duplicate, or did I misconfigure something?" - not
//! answerable from a bare count.
//!
//! This module answers it using only evidence the system already has:
//! [`PathMappings::translate`] to learn *which* configured mapping (and which
//! of its `provider_aliases`) produced each claimant's destination, exactly
//! the same call the real import path already makes. It never inspects a
//! title, a filename heuristic, or any text a provider could phrase however
//! it likes - only the provider path string against the mapping configuration
//! already trusted to route it.
//!
//! # What this is not
//!
//! - Not a second ambiguity engine: [`PathClaims`] still decides *whether*
//!   something is contested; this only classifies an already-contested group.
//! - Not automatic resolution: nothing here picks a record, edits a mapping,
//!   or writes anything. [`RommDuplicateProviderRecommendation`] is text for a
//!   person to act on, never an action taken on their behalf.
//! - Not RomM-catalogue deduplication: a `DuplicateProviderPlatformAlias`
//!   group is *expected and correct* once EmuWiz has declared two provider
//!   platform slugs equivalent - the underlying RomM-side duplication is a
//!   fact about the remote catalogue, not something this build can or should
//!   fix.

use std::path::PathBuf;

use crate::identity_source::matching::PathClaims;
use crate::identity_source::model::{ExternalIdentityRecord, ExternalVerification};
use crate::identity_source::path_map::{PathMappings, PathTranslation};

/// One local destination more than one RomM record claims, and everything
/// this build can prove about why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommDuplicateProviderGroup {
    /// The one local path every record in `records` translates to.
    pub destination: PathBuf,
    /// Whether that path exists on disk right now. A read-only `stat`; never
    /// written to, moved, or deleted by this module.
    pub destination_exists: bool,
    /// Every claimant, in the cache's own order.
    pub records: Vec<RommDuplicateProviderRecord>,
    /// Best-evidence explanation for the collision - see its own doc.
    pub reason: RommDuplicateProviderReason,
    /// What a person should do about it - see its own doc. Never an
    /// instruction to delete a local file, and never a mapping-configuration
    /// change recommendation unless `reason` is `MappingCollision`.
    pub recommendation: RommDuplicateProviderRecommendation,
}

impl RommDuplicateProviderGroup {
    /// Whether every claimant's `provider_path` ends in the same final path
    /// component (e.g. both end in `Animal Crossing (USA).zip`). Evidence
    /// only - `reason` is never derived from this, since a shared basename
    /// proves nothing about *why* two provider records collide (two
    /// genuinely different games can share a generic filename); it is
    /// exposed only because a person reading this report will want to know
    /// whether the claimants even look like "the same file" before deciding
    /// what to do.
    pub fn claimants_share_basename(&self) -> bool {
        let mut basenames = self
            .records
            .iter()
            .map(|record| std::path::Path::new(&record.provider_path).file_name());
        let Some(first) = basenames.next() else {
            return true;
        };
        basenames.all(|basename| basename == first)
    }
}

/// One RomM record's side of a contested destination - the fields a person
/// needs to tell the claimants apart, nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommDuplicateProviderRecord {
    pub provider_game_id: String,
    pub provider_path: String,
    pub provider_platform_id: Option<String>,
    pub provider_platform_name: Option<String>,
    /// EmuWiz's own resolved platform for this record, when one was
    /// established - distinct from `provider_platform_name`, which is
    /// whatever RomM itself calls it.
    pub platform_candidate: Option<String>,
    pub verification: ExternalVerification,
}

/// Why a destination is contested, ranked from "expected, given current
/// configuration" to "cannot be explained from current evidence". Computed
/// purely from [`PathMappings::translate`] against each claimant's
/// `provider_path` - never from title text, filename heuristics, or any
/// provider-supplied description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RommDuplicateProviderReason {
    /// Every claimant's `provider_path` matched a *different* prefix
    /// (canonical or alias) of the *same* configured [`PathMapping`], i.e.
    /// EmuWiz's own configuration already declares these provider platform
    /// slugs equivalent (`gcn`/`ngc`, `ps`/`psx`, ...). The collision exists
    /// because RomM's own catalogue lists the same physical file under both
    /// slugs - a fact about the remote server, not a local misconfiguration.
    DuplicateProviderPlatformAlias,
    /// Every claimant's `provider_path` matched the exact same prefix of the
    /// exact same mapping - the destination is contested by more than one
    /// distinct provider record inside what is, as far as EmuWiz's mappings
    /// are concerned, a single provider platform. This points at RomM's own
    /// catalogue having more than one entry for the same file, not at an
    /// alias relationship.
    DuplicateProviderRecord,
    /// Claimants' `provider_path`s matched *different* [`PathMapping`]
    /// entries - mappings EmuWiz has not declared equivalent - that
    /// nonetheless resolved to the same local destination. This is the one
    /// reason that implicates the local mapping configuration itself.
    MappingCollision,
    /// At least one claimant's `provider_path` no longer translates under
    /// the *current* mapping set at all (e.g. the record was imported under
    /// an older configuration, or a mapping was since removed). Today's
    /// configuration cannot explain a collision recorded under yesterday's,
    /// so this is reported honestly rather than guessed at.
    UnknownConflict,
}

/// A read-only instruction for a person, never an action taken by this
/// build. See [`RommDuplicateProviderReason`] for what determines which
/// variant applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RommDuplicateProviderRecommendation {
    /// Matches [`RommDuplicateProviderReason::DuplicateProviderPlatformAlias`].
    ReviewProviderPlatformDuplication,
    /// Matches [`RommDuplicateProviderReason::DuplicateProviderRecord`].
    ReviewProviderCatalogueDuplicate,
    /// Matches [`RommDuplicateProviderReason::MappingCollision`].
    ReviewMappingConfiguration,
    /// Matches [`RommDuplicateProviderReason::UnknownConflict`].
    ReviewManually,
}

impl RommDuplicateProviderRecommendation {
    fn for_reason(reason: RommDuplicateProviderReason) -> Self {
        match reason {
            RommDuplicateProviderReason::DuplicateProviderPlatformAlias => {
                Self::ReviewProviderPlatformDuplication
            }
            RommDuplicateProviderReason::DuplicateProviderRecord => {
                Self::ReviewProviderCatalogueDuplicate
            }
            RommDuplicateProviderReason::MappingCollision => Self::ReviewMappingConfiguration,
            RommDuplicateProviderReason::UnknownConflict => Self::ReviewManually,
        }
    }

    /// The message a person actually reads. Deliberately never suggests
    /// deleting a local file, and never suggests changing an EmuWiz mapping
    /// unless the evidence itself implicates one.
    pub fn message(self) -> &'static str {
        match self {
            Self::ReviewProviderPlatformDuplication => {
                "RomM lists this file under more than one platform slug that EmuWiz already \
                 treats as equivalent. This is expected given your current mapping and is not \
                 an EmuWiz configuration problem - it will stay unresolved unless you disable \
                 or merge the duplicate platform on the RomM server itself."
            }
            Self::ReviewProviderCatalogueDuplicate => {
                "RomM's own catalogue has more than one entry for this file under the same \
                 platform. Review that platform's entries on the RomM server for a duplicate \
                 or re-scanned record."
            }
            Self::ReviewMappingConfiguration => {
                "These provider platforms are not configured as equivalent in EmuWiz, but they \
                 resolve to the same local destination. Review your RomM path mapping \
                 configuration for an overlapping destination."
            }
            Self::ReviewManually => {
                "This collision could not be explained from the current mapping configuration - \
                 possibly because it was recorded under a configuration that has since changed. \
                 Review it manually."
            }
        }
    }
}

/// Every contested destination in one cache, explained. Deterministic:
/// grouped and sorted the same way for the same input every time, and
/// touches nothing but a read-only filesystem `stat` per destination (to
/// report [`RommDuplicateProviderGroup::destination_exists`]) - no write, no
/// network call, no hash.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RommDuplicateProviderSummary {
    /// Sorted by destination, so the same cache and mappings always produce
    /// the same report - never dependent on the cache's own record order.
    pub groups: Vec<RommDuplicateProviderGroup>,
}

impl RommDuplicateProviderSummary {
    /// How many distinct local destinations are contested. Matches
    /// [`PathClaims::contested`]'s own count exactly - this report explains
    /// the same set, it does not compute a second one.
    pub fn contested_destination_count(&self) -> usize {
        self.groups.len()
    }

    /// How many individual records participate in a contested destination -
    /// each record counted exactly once, in exactly one group, since a
    /// destination can only be contested once and a record has exactly one
    /// `archivefs_path`.
    pub fn ambiguous_record_count(&self) -> usize {
        self.groups.iter().map(|group| group.records.len()).sum()
    }

    /// How many groups fall under one [`RommDuplicateProviderReason`] -
    /// every group has exactly one reason, so these counts never overlap and
    /// always sum to [`Self::contested_destination_count`].
    pub fn count_by_reason(&self, reason: RommDuplicateProviderReason) -> usize {
        self.groups
            .iter()
            .filter(|group| group.reason == reason)
            .count()
    }
}

/// Groups every contested destination in `records` and explains each one
/// against `mappings` - the exact [`PathMappings`] currently configured, the
/// same set a real import would translate against.
///
/// Read-only end to end: `records` and `mappings` are only ever read: no
/// record is edited, no mapping is added or changed, no cache or config file
/// is written, and the only I/O performed is one `Path::exists` check per
/// contested destination.
pub fn explain_duplicate_providers(
    records: &[ExternalIdentityRecord],
    mappings: &PathMappings,
) -> RommDuplicateProviderSummary {
    let claims = PathClaims::of(records);
    let mut contested: Vec<&PathBuf> = claims
        .contested()
        .into_iter()
        .map(|(path, _count)| path)
        .collect();
    // `PathClaims` is keyed by a `BTreeMap<PathBuf, _>` internally, so this
    // is already destination-sorted - re-sorting here is a deliberate,
    // low-cost guarantee against that detail ever changing underneath this
    // module rather than a correction of anything observed to be wrong.
    contested.sort();

    let groups = contested
        .into_iter()
        .map(|destination| {
            let claimants: Vec<&ExternalIdentityRecord> = records
                .iter()
                .filter(|record| record.archivefs_path.as_deref() == Some(destination.as_path()))
                .collect();
            build_group(destination.clone(), &claimants, mappings)
        })
        .collect();

    RommDuplicateProviderSummary { groups }
}

fn build_group(
    destination: PathBuf,
    claimants: &[&ExternalIdentityRecord],
    mappings: &PathMappings,
) -> RommDuplicateProviderGroup {
    let reason = classify(claimants, mappings);
    let destination_exists = destination.exists();
    RommDuplicateProviderGroup {
        destination_exists,
        records: claimants
            .iter()
            .map(|record| RommDuplicateProviderRecord {
                provider_game_id: record.provider_game_id.clone(),
                provider_path: record.provider_path.clone(),
                provider_platform_id: record.provider_platform_id.clone(),
                provider_platform_name: record.provider_platform_name.clone(),
                platform_candidate: record.platform_candidate.clone(),
                verification: record.verification,
            })
            .collect(),
        destination,
        recommendation: RommDuplicateProviderRecommendation::for_reason(reason),
        reason,
    }
}

/// Which configured mapping (by position in [`PathMappings::as_slice`])
/// produced a claimant's destination, and the exact prefix (canonical or
/// alias) that matched - or `None` when the claimant's own `provider_path`
/// no longer translates under this mapping set at all.
fn matched_mapping(
    record: &ExternalIdentityRecord,
    mappings: &PathMappings,
) -> Option<(usize, String)> {
    let PathTranslation::Translated { matched_prefix, .. } =
        mappings.translate(&record.provider_path)
    else {
        return None;
    };
    let index = mappings.as_slice().iter().position(|mapping| {
        mapping.provider_prefix == matched_prefix
            || mapping
                .provider_aliases
                .iter()
                .any(|alias| *alias == matched_prefix)
    })?;
    Some((index, matched_prefix))
}

fn classify(
    claimants: &[&ExternalIdentityRecord],
    mappings: &PathMappings,
) -> RommDuplicateProviderReason {
    let matches: Vec<Option<(usize, String)>> = claimants
        .iter()
        .map(|record| matched_mapping(record, mappings))
        .collect();

    // Any claimant whose own provider path does not translate under today's
    // mapping set at all cannot be explained by today's configuration - the
    // honest answer is "unknown", not a guess dressed up as one of the
    // other three.
    if matches.iter().any(Option::is_none) {
        return RommDuplicateProviderReason::UnknownConflict;
    }
    let matches: Vec<(usize, String)> = matches.into_iter().map(|m| m.expect("checked")).collect();

    let first_index = matches[0].0;
    let same_mapping = matches.iter().all(|(index, _)| *index == first_index);
    if !same_mapping {
        return RommDuplicateProviderReason::MappingCollision;
    }

    let first_prefix = &matches[0].1;
    let same_prefix = matches.iter().all(|(_, prefix)| prefix == first_prefix);
    if same_prefix {
        RommDuplicateProviderReason::DuplicateProviderRecord
    } else {
        RommDuplicateProviderReason::DuplicateProviderPlatformAlias
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::model::IdentityProvider;
    use crate::identity_source::path_map::{PathMapping, ProviderPathKind};

    /// Mirrors `stage1b_tests::record_for`'s shape (same field defaults),
    /// varied by the fields this module's classification actually reads.
    fn record(
        game_id: &str,
        provider_path: &str,
        platform_id: Option<&str>,
        platform_name: &str,
        destination: Option<PathBuf>,
    ) -> ExternalIdentityRecord {
        ExternalIdentityRecord {
            provider: IdentityProvider::Romm,
            server_id: "http://romm.example".to_string(),
            provider_platform_id: platform_id.map(str::to_string),
            provider_game_id: game_id.to_string(),
            provider_file_id: None,
            provider_path: provider_path.to_string(),
            archivefs_path: destination,
            title: Some(format!("Game {game_id}")),
            platform_candidate: Some(platform_name.to_string()),
            provider_platform_name: Some(platform_name.to_string()),
            regions: Vec::new(),
            revision: None,
            hashes: Vec::new(),
            file_size_bytes: Some(1024),
            metadata_provider_ids: Vec::new(),
            artwork: None,
            related_files: Vec::new(),
            sibling_game_ids: Vec::new(),
            imported_at_unix_seconds: 1_785_000_000,
            provider_updated_at: None,
            verification: ExternalVerification::Ambiguous,
            conflicts: Vec::new(),
            evidence: Vec::new(),
            synopsis: None,
            genres: Vec::new(),
            players: None,
            rating: None,
            release_year: None,
        }
    }

    fn mapping(prefix: &str, destination: &str, aliases: Vec<&str>) -> PathMapping {
        PathMapping {
            provider_prefix: prefix.to_string(),
            archivefs_prefix: PathBuf::from(destination),
            provider_aliases: aliases.into_iter().map(str::to_string).collect(),
        }
    }

    fn mappings(entries: Vec<PathMapping>) -> PathMappings {
        PathMappings::validate(&entries, &[], ProviderPathKind::ProviderRelative)
            .expect("test mappings must validate")
    }

    #[test]
    fn gcn_and_ngc_on_the_same_destination_is_a_duplicate_provider_platform_alias() {
        let mappings = mappings(vec![mapping(
            "roms/gcn",
            "/mnt/usbdrive/games/ngc",
            vec!["roms/ngc"],
        )]);
        let destination = PathBuf::from("/mnt/usbdrive/games/ngc/Animal Crossing (USA).zip");
        let records = vec![
            record(
                "44703",
                "roms/gcn/Animal Crossing (USA).zip",
                Some("20"),
                "gcn",
                Some(destination.clone()),
            ),
            record(
                "101059",
                "roms/ngc/Animal Crossing (USA).zip",
                Some("177"),
                "ngc",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert_eq!(summary.contested_destination_count(), 1);
        assert_eq!(summary.ambiguous_record_count(), 2);
        let group = &summary.groups[0];
        assert_eq!(group.destination, destination);
        assert_eq!(
            group.reason,
            RommDuplicateProviderReason::DuplicateProviderPlatformAlias
        );
        assert_eq!(
            group.recommendation,
            RommDuplicateProviderRecommendation::ReviewProviderPlatformDuplication
        );
        assert_eq!(group.records.len(), 2);
        // Both claimants present - neither dropped, neither picked as "the"
        // answer.
        let ids: Vec<&str> = group
            .records
            .iter()
            .map(|record| record.provider_game_id.as_str())
            .collect();
        assert_eq!(ids, vec!["44703", "101059"]);
    }

    #[test]
    fn ps_and_psx_on_the_same_destination_is_a_duplicate_provider_platform_alias() {
        let mappings = mappings(vec![mapping(
            "roms/ps",
            "/mnt/usbdrive/games/psx",
            vec!["roms/psx"],
        )]);
        let destination =
            PathBuf::from("/mnt/usbdrive/games/psx/007 - The World is Not Enough (NA).chd");
        let records = vec![
            record(
                "44705",
                "roms/ps/007 - The World is Not Enough (NA).chd",
                Some("7"),
                "ps",
                Some(destination.clone()),
            ),
            record(
                "9001",
                "roms/psx/007 - The World is Not Enough (NA).chd",
                Some("87"),
                "psx",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert_eq!(summary.contested_destination_count(), 1);
        assert_eq!(
            summary.groups[0].reason,
            RommDuplicateProviderReason::DuplicateProviderPlatformAlias
        );
    }

    #[test]
    fn two_records_under_the_same_slug_and_path_are_a_duplicate_provider_record() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let destination = PathBuf::from("/mnt/games/nes/Game.zip");
        let records = vec![
            record(
                "1",
                "roms/nes/Game.zip",
                Some("3"),
                "nes",
                Some(destination.clone()),
            ),
            record(
                "2",
                "roms/nes/Game.zip",
                Some("3"),
                "nes",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert_eq!(
            summary.groups[0].reason,
            RommDuplicateProviderReason::DuplicateProviderRecord
        );
        assert_eq!(
            summary.groups[0].recommendation,
            RommDuplicateProviderRecommendation::ReviewProviderCatalogueDuplicate
        );
    }

    #[test]
    fn two_unrelated_mappings_colliding_on_one_destination_is_a_mapping_collision() {
        // Two mappings EmuWiz has *not* declared equivalent (no
        // provider_aliases linking them) whose destinations nest one inside
        // the other - `PathMappings::validate` already refuses two mappings
        // with the *exact same* destination outright (see
        // `path_map::MappingRefusal::DuplicateDestination`), so a real
        // mapping-configuration collision can only arise this way: distinct,
        // individually-valid destinations that a specific relative path
        // still resolves into the same real file. A local configuration
        // problem, not a RomM one.
        let mappings = mappings(vec![
            mapping("roms/nes", "/mnt/games/shared", vec![]),
            mapping("roms/famicom", "/mnt/games/shared/nested", vec![]),
        ]);
        let destination = PathBuf::from("/mnt/games/shared/nested/Game.zip");
        let records = vec![
            record(
                "1",
                "roms/nes/nested/Game.zip",
                Some("3"),
                "nes",
                Some(destination.clone()),
            ),
            record(
                "2",
                "roms/famicom/Game.zip",
                Some("9"),
                "famicom",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert_eq!(
            summary.groups[0].reason,
            RommDuplicateProviderReason::MappingCollision
        );
        assert_eq!(
            summary.groups[0].recommendation,
            RommDuplicateProviderRecommendation::ReviewMappingConfiguration
        );
    }

    #[test]
    fn a_claimant_whose_path_no_longer_translates_is_an_unknown_conflict() {
        // The mapping that would have explained one claimant's path has
        // since been removed - today's configuration cannot prove why this
        // collision exists, so it must not guess one of the other reasons.
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let destination = PathBuf::from("/mnt/games/nes/Game.zip");
        let records = vec![
            record(
                "1",
                "roms/nes/Game.zip",
                Some("3"),
                "nes",
                Some(destination.clone()),
            ),
            record(
                "2",
                "roms/removed-platform/Game.zip",
                Some("99"),
                "removed-platform",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert_eq!(
            summary.groups[0].reason,
            RommDuplicateProviderReason::UnknownConflict
        );
        assert_eq!(
            summary.groups[0].recommendation,
            RommDuplicateProviderRecommendation::ReviewManually
        );
    }

    #[test]
    fn a_single_claimant_produces_no_group() {
        let mappings = mappings(vec![mapping("roms/nes", "/mnt/games/nes", vec![])]);
        let records = vec![record(
            "1",
            "roms/nes/Game.zip",
            Some("3"),
            "nes",
            Some(PathBuf::from("/mnt/games/nes/Game.zip")),
        )];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert!(summary.groups.is_empty());
        assert_eq!(summary.contested_destination_count(), 0);
        assert_eq!(summary.ambiguous_record_count(), 0);
    }

    #[test]
    fn a_missing_local_destination_is_reported_safely_not_panicked_on() {
        let mappings = mappings(vec![mapping(
            "roms/atarijaguar",
            "/mnt/usbdrive/games/jaguar",
            vec!["roms/jaguar"],
        )]);
        let destination = PathBuf::from("/mnt/usbdrive/games/jaguar/does-not-exist-anywhere.j64");
        let records = vec![
            record(
                "1",
                "roms/atarijaguar/does-not-exist-anywhere.j64",
                Some("2"),
                "atarijaguar",
                Some(destination.clone()),
            ),
            record(
                "2",
                "roms/jaguar/does-not-exist-anywhere.j64",
                Some("5"),
                "jaguar",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert!(!summary.groups[0].destination_exists);
        assert_eq!(summary.groups.len(), 1, "still reported, not dropped");
    }

    #[test]
    fn output_is_deterministic_across_repeated_calls() {
        let mappings = mappings(vec![
            mapping("roms/gcn", "/mnt/games/ngc", vec!["roms/ngc"]),
            mapping("roms/ps", "/mnt/games/psx", vec!["roms/psx"]),
        ]);
        let records = vec![
            record(
                "1",
                "roms/gcn/A.zip",
                Some("1"),
                "gcn",
                Some(PathBuf::from("/mnt/games/ngc/A.zip")),
            ),
            record(
                "2",
                "roms/ngc/A.zip",
                Some("2"),
                "ngc",
                Some(PathBuf::from("/mnt/games/ngc/A.zip")),
            ),
            record(
                "3",
                "roms/ps/B.chd",
                Some("3"),
                "ps",
                Some(PathBuf::from("/mnt/games/psx/B.chd")),
            ),
            record(
                "4",
                "roms/psx/B.chd",
                Some("4"),
                "psx",
                Some(PathBuf::from("/mnt/games/psx/B.chd")),
            ),
        ];

        let first = explain_duplicate_providers(&records, &mappings);
        let second = explain_duplicate_providers(&records, &mappings);

        assert_eq!(first, second);
        // Destination-sorted, not cache-order.
        assert_eq!(
            first
                .groups
                .iter()
                .map(|g| g.destination.clone())
                .collect::<Vec<_>>(),
            vec![
                PathBuf::from("/mnt/games/ngc/A.zip"),
                PathBuf::from("/mnt/games/psx/B.chd"),
            ]
        );
    }

    #[test]
    fn totals_never_double_count_a_record_across_groups() {
        let mappings = mappings(vec![
            mapping("roms/gcn", "/mnt/games/ngc", vec!["roms/ngc"]),
            mapping("roms/nes", "/mnt/games/nes", vec![]),
        ]);
        let records = vec![
            record(
                "1",
                "roms/gcn/A.zip",
                Some("1"),
                "gcn",
                Some(PathBuf::from("/mnt/games/ngc/A.zip")),
            ),
            record(
                "2",
                "roms/ngc/A.zip",
                Some("2"),
                "ngc",
                Some(PathBuf::from("/mnt/games/ngc/A.zip")),
            ),
            record(
                "3",
                "roms/nes/B.zip",
                Some("3"),
                "nes",
                Some(PathBuf::from("/mnt/games/nes/B.zip")),
            ),
            record(
                "4",
                "roms/nes/B.zip",
                Some("3"),
                "nes",
                Some(PathBuf::from("/mnt/games/nes/B.zip")),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert_eq!(summary.contested_destination_count(), 2);
        assert_eq!(summary.ambiguous_record_count(), 4);
        assert_eq!(
            summary.count_by_reason(RommDuplicateProviderReason::DuplicateProviderPlatformAlias),
            1
        );
        assert_eq!(
            summary.count_by_reason(RommDuplicateProviderReason::DuplicateProviderRecord),
            1
        );
        // The two reason counts must exhaust the total - no group counted
        // under more than one reason, and none left uncounted.
        assert_eq!(
            summary.count_by_reason(RommDuplicateProviderReason::DuplicateProviderPlatformAlias)
                + summary.count_by_reason(RommDuplicateProviderReason::DuplicateProviderRecord)
                + summary.count_by_reason(RommDuplicateProviderReason::MappingCollision)
                + summary.count_by_reason(RommDuplicateProviderReason::UnknownConflict),
            summary.contested_destination_count()
        );
    }

    #[test]
    fn nothing_here_ever_selects_a_winning_record() {
        // `RommDuplicateProviderGroup` exposes no "primary"/"chosen"/"winner"
        // field and no way to reduce `records` to one - this test exists as
        // a visible anchor: if a future edit adds such a field, a reviewer
        // sees this test's own assumption (every claimant survives, in
        // full) stop matching reality.
        let mappings = mappings(vec![mapping(
            "roms/gcn",
            "/mnt/games/ngc",
            vec!["roms/ngc"],
        )]);
        let destination = PathBuf::from("/mnt/games/ngc/A.zip");
        let records = vec![
            record(
                "1",
                "roms/gcn/A.zip",
                Some("1"),
                "gcn",
                Some(destination.clone()),
            ),
            record(
                "2",
                "roms/ngc/A.zip",
                Some("2"),
                "ngc",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);

        assert_eq!(summary.groups[0].records.len(), 2, "no record dropped");
    }

    #[test]
    fn claimants_share_basename_reflects_the_actual_provider_paths() {
        let mappings = mappings(vec![mapping(
            "roms/gcn",
            "/mnt/games/ngc",
            vec!["roms/ngc"],
        )]);
        let destination = PathBuf::from("/mnt/games/ngc/A.zip");
        let records = vec![
            record(
                "1",
                "roms/gcn/A.zip",
                Some("1"),
                "gcn",
                Some(destination.clone()),
            ),
            record("2", "roms/ngc/A.zip", Some("2"), "ngc", Some(destination)),
        ];
        let summary = explain_duplicate_providers(&records, &mappings);
        assert!(summary.groups[0].claimants_share_basename());
    }

    #[test]
    fn explain_duplicate_providers_takes_only_local_in_memory_types() {
        // Anchor test: the signature is `(&[ExternalIdentityRecord],
        // &PathMappings) -> RommDuplicateProviderSummary` - no transport,
        // no client, no file handle. A future edit that needs network or
        // filesystem access beyond a single `Path::exists` call would have
        // to change this signature, which is the point of pinning it here.
        fn assert_signature(
            _: fn(&[ExternalIdentityRecord], &PathMappings) -> RommDuplicateProviderSummary,
        ) {
        }
        assert_signature(explain_duplicate_providers);
    }

    #[test]
    fn calling_this_never_touches_the_real_destination_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let destination = dir.path().join("A.zip");
        std::fs::write(&destination, b"original bytes").unwrap();
        let before = std::fs::metadata(&destination).unwrap().modified().unwrap();
        let before_bytes = std::fs::read(&destination).unwrap();

        let mappings = mappings(vec![mapping(
            "roms/gcn",
            dir.path().to_str().unwrap(),
            vec!["roms/ngc"],
        )]);
        let records = vec![
            record(
                "1",
                "roms/gcn/A.zip",
                Some("1"),
                "gcn",
                Some(destination.clone()),
            ),
            record(
                "2",
                "roms/ngc/A.zip",
                Some("2"),
                "ngc",
                Some(destination.clone()),
            ),
        ];

        let summary = explain_duplicate_providers(&records, &mappings);
        assert!(summary.groups[0].destination_exists);

        let after = std::fs::metadata(&destination).unwrap().modified().unwrap();
        let after_bytes = std::fs::read(&destination).unwrap();
        assert_eq!(
            before, after,
            "the destination file's mtime must be untouched"
        );
        assert_eq!(
            before_bytes, after_bytes,
            "the destination file's contents must be untouched"
        );
    }

    #[test]
    fn calling_this_never_changes_the_mappings_it_was_given() {
        let mappings = mappings(vec![mapping(
            "roms/gcn",
            "/mnt/games/ngc",
            vec!["roms/ngc"],
        )]);
        let before = mappings.as_slice().to_vec();
        let records = vec![record(
            "1",
            "roms/gcn/A.zip",
            Some("1"),
            "gcn",
            Some(PathBuf::from("/mnt/games/ngc/A.zip")),
        )];

        let _ = explain_duplicate_providers(&records, &mappings);

        assert_eq!(mappings.as_slice(), before.as_slice());
    }
}
