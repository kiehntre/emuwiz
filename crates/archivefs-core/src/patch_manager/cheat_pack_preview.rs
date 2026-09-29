//! Local, read-only pack analysis. This is an inspection/plan over existing
//! parsers and reconciliation, not a second installer. Nothing here accepts a
//! destination, database handle, executor, persisted review or enable command.
//! Source observations and logical cheats have separate, reconcilable totals.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::cheat_ir::{
    self, CheatDocument, CheatPlatform, CheatReconciliationEntry, CheatReconciliationOutcome,
    CheatReconciliationResult, CheatSourceFormat,
};
use super::cht_document::{ChtDocumentWarning, ChtEntryWarning};
use super::user_cheat_import::{
    UserCheatDiagnostic, UserCheatFormat, UserCheatImportError, UserCheatImportLimits,
    UserCheatLibraryGame, UserCheatProvenance,
};
use crate::game_identity::{IdentityEvidence, IdentityKind, IdentityStatus};

mod source;
#[cfg(all(test, target_os = "linux"))]
mod tests;

/// Safety limits cannot exceed these defaults. Enumeration includes directories,
/// preventing huge empty-directory trees from bypassing the source-file budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackLimits {
    pub source: UserCheatImportLimits,
    pub max_observations: usize,
    pub max_line_bytes: usize,
    pub max_lines_per_file: usize,
    pub max_code_lines: usize,
    pub max_catalogue_games: usize,
    pub max_matches_per_file: usize,
}
impl Default for CheatPackLimits {
    fn default() -> Self {
        Self {
            source: UserCheatImportLimits {
                max_file_bytes: 512 * 1024,
                max_total_bytes: 128 * 1024 * 1024,
                max_files_visited: 10_000,
                max_depth: 16,
                max_cheats_per_file: 1024,
                max_warnings: 256,
            },
            max_observations: 65_536,
            max_line_bytes: 8192,
            max_lines_per_file: 16_384,
            max_code_lines: 1024,
            max_catalogue_games: 100_000,
            max_matches_per_file: 128,
        }
    }
}

/// Expectations from source data or an explicitly supplied local manifest.
/// A requirement is not proof of the ROM's identity. No manifest format is
/// invented: callers may adapt their existing evidence to this in-memory seam.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CheatPackAssociation {
    pub title: Option<String>,
    pub filename: Option<String>,
    pub platform: Option<String>,
    pub region: Option<String>,
    pub revision: Option<String>,
    pub identities: Vec<CheatPackIdentityRequirement>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackIdentityRequirement {
    pub kind: IdentityKind,
    pub value: String,
}
/// Verified facts use EmuWiz's existing IdentityEvidence, never optional string
/// fields in UserCheatLibraryGame as an implicit verification flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackCatalogueGame {
    pub game: UserCheatLibraryGame,
    pub facts: Vec<IdentityEvidence>,
    pub revision: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CheatPackMatchStrength {
    Unmatched,
    Possible,
    Strong,
    Exact,
    Ambiguous,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackGameMatch {
    pub game_id: String,
    pub identity_key: String,
    pub strength: CheatPackMatchStrength,
    pub evidence: Vec<CheatPackIdentityRequirement>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CheatPackApplicability {
    Ready,
    PossibleMatch,
    NeedsReview,
    WrongRegion,
    WrongRevision,
    UnsupportedFormat,
    UnsupportedTarget,
    Malformed,
    Unmatched,
    Ambiguous,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CheatPackAction {
    WouldAdd,
    WouldCorroborate,
    WouldRetainExisting,
    WouldRequireReview,
    WouldRejectMalformed,
    WouldRejectUnsupported,
    WouldRemainUnmatched,
    WouldRemainAmbiguous,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CheatPackRelationship {
    ExactDuplicate,
    EquivalentDuplicate,
    CorroboratingObservation,
    NameConflict,
    CodeConflict,
    SourceIndexConflict,
    RegionVariant,
    RevisionVariant,
    SyntaxVariant,
    AmbiguousPossibleDuplicate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CheatPackFileState {
    Accepted,
    Malformed,
    Unsupported,
    Unreadable,
    LimitRejected,
}
/// Narrow adapter for typed parser diagnostics. Original .cht kinds, line and
/// bounded raw source evidence remain intact; Batch 1 can feed this directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum CheatPackDiagnostic {
    ChtDocument(ChtDocumentWarning),
    ChtEntry(ChtEntryWarning),
    ParseFailure { code: String, detail: String },
    Source(UserCheatDiagnostic),
    Limit { detail: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackFile {
    pub path: PathBuf,
    pub format: Option<UserCheatFormat>,
    pub state: CheatPackFileState,
    pub provenance: Option<UserCheatProvenance>,
    pub association: CheatPackAssociation,
    pub game_key: String,
    pub matches: Vec<CheatPackGameMatch>,
    pub match_strength: CheatPackMatchStrength,
    pub matches_truncated: bool,
    pub observation_indices: Vec<usize>,
    pub diagnostics: Vec<CheatPackDiagnostic>,
    pub diagnostics_truncated: bool,
    pub source_metadata: BTreeMap<String, String>,
    pub source_comments: Vec<String>,
}
/// Observation provenance is separate from logical identity. Identical content
/// digests are known copies, not evidence of independent source authorship.
/// source_group/mirror_of may be supplied by Batch 3's richer provenance; an
/// arbitrary filesystem path is never treated as an independent source group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackObservation {
    pub file_index: usize,
    pub source_index: Option<u32>,
    pub source_index_conflict: bool,
    pub provenance: UserCheatProvenance,
    pub source_group: Option<String>,
    pub mirror_of: Option<String>,
    pub association: CheatPackAssociation,
    pub document: CheatDocument,
    pub raw_code: String,
    pub code_truncated: bool,
    pub full_code_digest: Option<String>,
    pub execution_fields: BTreeMap<String, String>,
    pub engine: Option<String>,
    pub source_enabled_by_default: bool,
    pub applicability: CheatPackApplicability,
    pub diagnostics: Vec<CheatPackDiagnostic>,
    pub diagnostics_truncated: bool,
    pub logical_key: String,
    pub action: CheatPackAction,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackLogicalCheat {
    pub key: String,
    pub game_key: String,
    pub observation_indices: Vec<usize>,
    pub relationships: BTreeSet<CheatPackRelationship>,
    pub distinct_source_contents: usize,
    pub independent_source_groups: usize,
    pub known_mirrors: usize,
    pub known_copies: usize,
    pub usable: bool,
    /// Existing main reconciliation evidence when identity is verified. Its
    /// groups are evidence only: a duplicate subgroup cannot hide a conflict.
    pub reconciliation: Option<CheatReconciliationResult>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackGame {
    pub key: String,
    pub file_indices: Vec<usize>,
    pub logical_cheat_indices: Vec<usize>,
    pub strength: CheatPackMatchStrength,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CheatPackTotals {
    pub files_discovered: usize,
    pub files_readable: usize,
    pub files_accepted: usize,
    pub files_malformed: usize,
    pub files_rejected: usize,
    pub games_represented: usize,
    pub exact_matches: usize,
    pub strong_matches: usize,
    pub possible_matches: usize,
    pub ambiguous_matches: usize,
    pub unmatched_games: usize,
    pub observations: usize,
    pub logical_cheats: usize,
    pub usable_cheats: usize,
    pub malformed_cheats: usize,
    /// Relationship totals count logical groups, not additional cheat rows.
    pub exact_duplicates: usize,
    pub equivalent_duplicates: usize,
    pub corroborating_sources: usize,
    pub conflicts: usize,
    pub region_mismatches: usize,
    pub revision_mismatches: usize,
    pub unsupported_formats: usize,
    pub unsupported_targets: usize,
    pub needs_review: usize,
    /// These disjoint action totals partition observations, not logical cheats.
    pub would_add: usize,
    pub would_corroborate: usize,
    pub would_retain_existing: usize,
    pub would_review: usize,
    pub would_reject: usize,
    pub would_remain_unmatched: usize,
    pub would_remain_ambiguous: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatPackPreview {
    pub root: PathBuf,
    pub limits: CheatPackLimits,
    pub files: Vec<CheatPackFile>,
    pub observations: Vec<CheatPackObservation>,
    pub games: Vec<CheatPackGame>,
    pub logical_cheats: Vec<CheatPackLogicalCheat>,
    pub diagnostics: Vec<CheatPackDiagnostic>,
    pub totals: CheatPackTotals,
    pub bytes_read: u64,
    /// False means an explicitly reported scan/retention bound was reached;
    /// totals describe only retained evidence, never an estimate of omitted rows.
    pub complete: bool,
}
impl CheatPackPreview {
    pub fn can_apply(&self) -> bool {
        false
    }
    pub fn actions_reconcile(&self) -> bool {
        let t = &self.totals;
        t.observations
            == t.would_add
                + t.would_corroborate
                + t.would_retain_existing
                + t.would_review
                + t.would_reject
                + t.would_remain_unmatched
                + t.would_remain_ambiguous
    }
}

/// Only local plain sources. associations are keyed by root-relative paths;
/// existing is an immutable snapshot of known logical keys, not a DB handle.
pub fn preview_cheat_pack(
    root: &Path,
    catalogue: &[CheatPackCatalogueGame],
    associations: &BTreeMap<PathBuf, CheatPackAssociation>,
    existing: &BTreeSet<String>,
    limits: &CheatPackLimits,
) -> Result<CheatPackPreview, UserCheatImportError> {
    source::preview(root, catalogue, associations, existing, limits)
}

/// Re-plan immutable, already inspected evidence (Batch 1-4 integration seam).
/// This accepts no filesystem/database/executor. Caller-provided Ready is an
/// evidence state, not install permission. Invalid references fail closed.
pub fn plan_cheat_pack_preview(
    mut preview: CheatPackPreview,
    existing: &BTreeSet<String>,
) -> Result<CheatPackPreview, UserCheatImportError> {
    source::validate(&preview.limits)?;
    if preview.files.len() > preview.limits.source.max_files_visited
        || preview.observations.len() > preview.limits.max_observations
        || preview.observations.iter().any(|o| {
            o.file_index >= preview.files.len()
                || o.raw_code.len() > super::cht_document::MAX_CHT_FIELD_BYTES
                || o.document.operations.len() > preview.limits.max_code_lines
        })
    {
        return Err(UserCheatImportError::InvalidLimits(
            "prepared pack exceeds bounds or has invalid source references".into(),
        ));
    }
    if preview
        .files
        .iter()
        .map(|f| &f.path)
        .collect::<BTreeSet<_>>()
        .len()
        != preview.files.len()
    {
        return Err(UserCheatImportError::InvalidLimits(
            "prepared pack has duplicate source paths".into(),
        ));
    }
    preview.logical_cheats.clear();
    preview.games.clear();
    // Normalize prepared enumeration without changing logical identity.
    let mut file_order: Vec<_> = (0..preview.files.len()).collect();
    file_order.sort_by(|&a, &b| preview.files[a].path.cmp(&preview.files[b].path));
    let mut remap = vec![0; file_order.len()];
    let files = file_order
        .into_iter()
        .enumerate()
        .map(|(new, old)| {
            remap[old] = new;
            preview.files[old].clone()
        })
        .collect();
    preview.files = files;
    for o in &mut preview.observations {
        o.file_index = remap[o.file_index];
    }
    preview.observations.sort_by(|a, b| {
        a.file_index
            .cmp(&b.file_index)
            .then_with(|| a.source_index.cmp(&b.source_index))
            .then_with(|| content_key(a).cmp(&content_key(b)))
            .then_with(|| a.document.title.cmp(&b.document.title))
            .then_with(|| a.mirror_of.cmp(&b.mirror_of))
            .then_with(|| a.source_group.cmp(&b.source_group))
            .then_with(|| a.provenance.source_sha256.cmp(&b.provenance.source_sha256))
            .then_with(|| a.provenance.original_path.cmp(&b.provenance.original_path))
            .then_with(|| a.applicability.cmp(&b.applicability))
    });
    for f in &mut preview.files {
        f.observation_indices.clear();
    }
    for (i, o) in preview.observations.iter().enumerate() {
        preview.files[o.file_index].observation_indices.push(i);
    }
    bound_diagnostics(&mut preview);
    plan(&mut preview, existing);
    Ok(preview)
}

fn digest<T: Serialize>(domain: &str, value: &T) -> String {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    // All inputs here are structs/string/enums/integer collections; serde_json
    // cannot reject them. Still return a deterministic key without panicking.
    if let Ok(bytes) = serde_json::to_vec(value) {
        hash.update(bytes);
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn requirement_key(kind: IdentityKind, value: &str) -> String {
    // Identifier spelling uses the existing PS2 normalizers where applicable.
    let value = match kind {
        IdentityKind::Ps2Serial => {
            super::pcsx2::normalize_serial(value).unwrap_or_else(|| value.into())
        }
        IdentityKind::Pcsx2ExecutableCrc => {
            super::pcsx2::normalize_crc(value).unwrap_or_else(|| value.into())
        }
        IdentityKind::LooseRomSha256 | IdentityKind::LooseRomCanonicalSha256 => {
            value.to_ascii_lowercase()
        }
        _ => value.trim().to_string(),
    };
    digest("cheat-pack-identity-v1", &(kind, value))
}
fn verified_keys(game: &CheatPackCatalogueGame) -> BTreeSet<String> {
    let mut by_kind = BTreeMap::<String, BTreeSet<String>>::new();
    for fact in game
        .facts
        .iter()
        .filter(|f| f.status == IdentityStatus::Verified && is_game_identity(f.kind))
    {
        if let Some(value) = fact.value.as_deref().filter(|v| !v.trim().is_empty()) {
            by_kind
                .entry(digest("identity-kind", &fact.kind))
                .or_default()
                .insert(requirement_key(fact.kind, value));
        }
    }
    if by_kind.values().any(|values| values.len() > 1) {
        return BTreeSet::new();
    }
    by_kind.into_values().flatten().collect()
}

fn catalogue_key(game: &CheatPackCatalogueGame) -> String {
    let release: BTreeSet<_> = game
        .facts
        .iter()
        .filter(|f| {
            f.status == IdentityStatus::Verified
                && matches!(
                    f.kind,
                    IdentityKind::DolphinRegion | IdentityKind::DolphinRevision
                )
        })
        .filter_map(|f| f.value.as_deref().map(|v| requirement_key(f.kind, v)))
        .collect();
    digest("cheat-pack-game-v1", &(verified_keys(game), release))
}

fn is_game_identity(kind: IdentityKind) -> bool {
    matches!(
        kind,
        IdentityKind::LooseRomSha256
            | IdentityKind::LooseRomCanonicalSha256
            | IdentityKind::Pcsx2ExecutableCrc
            | IdentityKind::Ps2Serial
            | IdentityKind::Ps1Serial
            | IdentityKind::PspDiscId
            | IdentityKind::DolphinGameId
            | IdentityKind::XexTitleId
    )
}
/// Indexed matching: no platform-only matches and no title/filename promotion.
struct CatalogueIndex<'a> {
    games: &'a [CheatPackCatalogueGame],
    order: Vec<usize>,
    match_limit: usize,
    by_key: BTreeMap<String, usize>,
    facts: BTreeMap<String, BTreeSet<usize>>,
    titles: BTreeMap<String, BTreeSet<usize>>,
}
impl<'a> CatalogueIndex<'a> {
    fn new(games: &'a [CheatPackCatalogueGame], match_limit: usize) -> Self {
        let sort_keys: Vec<_> = games
            .iter()
            .map(|g| (&g.game.game_id, catalogue_key(g)))
            .collect();
        let mut order: Vec<_> = (0..games.len()).collect();
        order.sort_by(|&a, &b| sort_keys[a].cmp(&sort_keys[b]));
        let mut result = Self {
            games,
            order,
            match_limit,
            by_key: BTreeMap::new(),
            facts: BTreeMap::new(),
            titles: BTreeMap::new(),
        };
        for (i, &original) in result.order.iter().enumerate() {
            result
                .by_key
                .entry(sort_keys[original].1.clone())
                .or_insert(original);
            let game = &games[original];
            for key in verified_keys(game) {
                result.facts.entry(key).or_default().insert(i);
            }
            let title = super::user_cheat_import::normalize_title(&game.game.title);
            if !title.is_empty() {
                result.titles.entry(title).or_default().insert(i);
            }
        }
        result
    }
    fn associate(
        &self,
        association: &CheatPackAssociation,
        remaining_matches: usize,
    ) -> (
        String,
        CheatPackMatchStrength,
        Vec<CheatPackGameMatch>,
        bool,
    ) {
        let match_limit = self.match_limit.min(remaining_matches);
        let mut candidates = BTreeSet::<usize>::new();
        let mut wanted = BTreeSet::new();
        for requirement in &association.identities {
            if is_game_identity(requirement.kind) && !requirement.value.trim().is_empty() {
                let key = requirement_key(requirement.kind, &requirement.value);
                if let Some(indices) = self.facts.get(&key) {
                    candidates.extend(indices.iter().take(match_limit + 1).copied());
                }
                wanted.insert(key);
            }
        }
        for text in [
            association.title.as_deref(),
            association.filename.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            let title = super::user_cheat_import::normalize_title(text);
            if !title.is_empty()
                && let Some(indices) = self.titles.get(&title)
            {
                candidates.extend(indices.iter().take(match_limit + 1).copied());
            }
        }
        let mut matches = Vec::new();
        let truncated = candidates.len() > match_limit;
        for i in candidates.into_iter().take(match_limit) {
            let game = &self.games[self.order[i]];
            if let (Some(a), Some(b)) = (
                association.platform.as_deref(),
                game.game.platform.as_deref(),
            ) && crate::canonical_platform_for_alias(a).unwrap_or(a)
                != crate::canonical_platform_for_alias(b).unwrap_or(b)
            {
                continue;
            }
            let facts = verified_keys(game);
            let matching: Vec<_> = association
                .identities
                .iter()
                .filter(|r| facts.contains(&requirement_key(r.kind, &r.value)))
                .cloned()
                .collect();
            // Conflicting requirements never fall back to title association.
            if !wanted.is_empty() && !wanted.is_subset(&facts) {
                continue;
            }
            let strength = if matching.iter().any(|r| {
                matches!(
                    r.kind,
                    IdentityKind::LooseRomSha256
                        | IdentityKind::LooseRomCanonicalSha256
                        | IdentityKind::Pcsx2ExecutableCrc
                )
            }) {
                CheatPackMatchStrength::Exact
            } else if !matching.is_empty() {
                CheatPackMatchStrength::Strong
            } else {
                CheatPackMatchStrength::Possible
            };
            matches.push(CheatPackGameMatch {
                game_id: game.game.game_id.clone(),
                identity_key: catalogue_key(game),
                strength,
                evidence: matching,
            });
        }
        matches.sort_by(|a, b| {
            b.strength
                .cmp(&a.strength)
                .then_with(|| a.game_id.cmp(&b.game_id))
                .then_with(|| digest("evidence", &a.evidence).cmp(&digest("evidence", &b.evidence)))
        });
        matches.dedup();
        let best = matches
            .first()
            .map_or(CheatPackMatchStrength::Unmatched, |m| m.strength);
        let top = matches.iter().filter(|m| m.strength == best).count();
        let strength = if top > 1 || truncated {
            CheatPackMatchStrength::Ambiguous
        } else {
            best
        };
        if strength == CheatPackMatchStrength::Ambiguous {
            for m in &mut matches {
                if m.strength == best {
                    m.strength = CheatPackMatchStrength::Ambiguous;
                }
            }
        }
        let key = if !truncated
            && !matches.is_empty()
            && top == 1
            && matches!(
                best,
                CheatPackMatchStrength::Exact | CheatPackMatchStrength::Strong
            ) {
            matches[0].identity_key.clone()
        } else {
            digest("cheat-pack-association-v1", &association_key(association))
        };
        (key, strength, matches, truncated)
    }
}
fn association_key(
    a: &CheatPackAssociation,
) -> (Option<String>, Option<String>, Option<String>, Vec<String>) {
    let identities = a
        .identities
        .iter()
        .map(|r| requirement_key(r.kind, &r.value))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    (
        a.title
            .as_deref()
            .map(super::user_cheat_import::normalize_title),
        a.filename
            .as_deref()
            .map(super::user_cheat_import::normalize_title),
        a.platform.clone(),
        identities,
    )
}
fn entry_for(
    observation: &CheatPackObservation,
    game_key: &str,
    verified: bool,
) -> CheatReconciliationEntry {
    CheatReconciliationEntry {
        game_identity: game_key.into(),
        identity_verified: verified,
        title: observation.document.title.clone(),
        source: observation.provenance.source_sha256.clone(),
        source_format: observation.document.source_format.clone(),
        document: observation.document.clone(),
        raw_code: Some(observation.raw_code.clone()),
        provenance: vec![
            observation.provenance.original_path.display().to_string(),
            observation.provenance.source_sha256.clone(),
        ],
    }
}
fn content_key(o: &CheatPackObservation) -> String {
    // Known semantics reuse main's identity. Opaque codes keep byte spelling,
    // format and all execution fields; no arbitrary whitespace/hex repair.
    let semantics = cheat_ir::semantic_fingerprint(&o.document);
    digest(
        "cheat-pack-code-v1",
        &(
            semantics.clone(),
            semantics.is_none().then_some((
                &o.document.source_format,
                &o.raw_code,
                &o.full_code_digest,
            )),
            &o.document.issues,
            &o.execution_fields,
            &o.engine,
            &o.association.region,
            &o.association.revision,
        ),
    )
}

fn plan(preview: &mut CheatPackPreview, existing: &BTreeSet<String>) {
    let n = preview.observations.len();
    let mut difference_budget = preview.limits.source.max_warnings.saturating_sub(
        preview.diagnostics.len()
            + preview
                .files
                .iter()
                .map(|f| f.diagnostics.len())
                .sum::<usize>()
            + preview
                .observations
                .iter()
                .map(|o| o.diagnostics.len())
                .sum::<usize>(),
    );
    let mut parent: Vec<_> = (0..n).collect();
    fn root(parent: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        let mut j = i;
        while parent[j] != j {
            let next = parent[j];
            parent[j] = r;
            j = next;
        }
        r
    }
    let mut buckets = BTreeMap::new();
    let mut keys = Vec::with_capacity(n);
    for (i, o) in preview.observations.iter().enumerate() {
        let game = &preview.files[o.file_index].game_key;
        let content = content_key(o);
        keys.push(digest(
            "cheat-pack-logical-v1",
            &(game, &o.document.platform, &content),
        ));
        let name = cheat_ir::reconciliation_title(&o.document.title);
        let mut comparisons = vec![digest("code-bucket", &(game, &content))];
        if !name.is_empty() {
            comparisons.push(digest("name-bucket", &(game, &name)));
        }
        comparisons.push(digest(
            "raw-bucket",
            &(game, &o.document.source_format, &o.raw_code),
        ));
        if let Some(index) = o.source_index {
            comparisons.push(digest(
                "index-bucket",
                &(game, o.provenance.source_sha256.clone(), index),
            ));
        }
        for bucket in comparisons {
            if let Some(&other) = buckets.get(&bucket) {
                let a = root(&mut parent, i);
                let b = root(&mut parent, other);
                parent[a] = b;
            } else {
                buckets.insert(bucket, i);
            }
        }
    }
    let mut groups = BTreeMap::<usize, Vec<usize>>::new();
    for i in 0..n {
        groups.entry(root(&mut parent, i)).or_default().push(i);
    }
    for indices in groups.into_values() {
        let first = &preview.observations[indices[0]];
        let all_ready = indices
            .iter()
            .all(|&i| preview.observations[i].applicability == CheatPackApplicability::Ready);
        let game_key = preview.files[first.file_index].game_key.clone();
        let mut relations = BTreeSet::new();
        let mut by_name = BTreeMap::<String, BTreeSet<String>>::new();
        let mut by_index = BTreeMap::<(String, u32), BTreeSet<String>>::new();
        let mut formats = BTreeSet::new();
        let mut regions = BTreeSet::new();
        let mut revisions = BTreeSet::new();
        let mut engines = BTreeSet::new();
        let mut contents = BTreeMap::<String, Vec<usize>>::new();
        let mut raw_semantics = BTreeMap::<String, BTreeSet<String>>::new();
        let mut provenance = BTreeSet::new();
        let mut copy_paths = BTreeMap::<String, BTreeSet<PathBuf>>::new();
        let mut independent = BTreeSet::new();
        let mut mirrors = BTreeSet::new();
        for &i in &indices {
            let o = &preview.observations[i];
            let content = content_key(o);
            if o.source_index_conflict {
                relations.insert(CheatPackRelationship::SourceIndexConflict);
            }
            by_name
                .entry(cheat_ir::reconciliation_title(&o.document.title))
                .or_default()
                .insert(content.clone());
            if let Some(index) = o.source_index {
                by_index
                    .entry((o.provenance.source_sha256.clone(), index))
                    .or_default()
                    .insert(digest("source-variant", &(&o.document.title, &content)));
            }
            formats.insert(digest("format", &o.document.source_format));
            if let Some(v) = &o.association.region {
                regions.insert(v.clone());
            }
            if let Some(v) = &o.association.revision {
                revisions.insert(v.clone());
            }
            if let Some(v) = &o.engine {
                engines.insert(v.clone());
            }
            provenance.insert(o.provenance.source_sha256.clone());
            copy_paths
                .entry(o.provenance.source_sha256.clone())
                .or_default()
                .insert(o.provenance.original_path.clone());
            if o.mirror_of.is_none()
                && let Some(v) = &o.source_group
            {
                independent.insert(v.clone());
            }
            if let Some(v) = &o.mirror_of {
                mirrors.insert(v.clone());
            }
            raw_semantics
                .entry(digest(
                    "strict-raw",
                    &(&o.document.source_format, &o.raw_code),
                ))
                .or_default()
                .insert(content.clone());
            contents.entry(content).or_default().push(i);
        }
        let ambiguous_information = indices
            .iter()
            .map(|&i| preview.observations[i].association.region.is_some())
            .collect::<BTreeSet<_>>()
            .len()
            > 1
            || indices
                .iter()
                .map(|&i| preview.observations[i].association.revision.is_some())
                .collect::<BTreeSet<_>>()
                .len()
                > 1
            || indices
                .iter()
                .map(|&i| preview.observations[i].engine.is_some())
                .collect::<BTreeSet<_>>()
                .len()
                > 1;
        if ambiguous_information {
            relations.insert(CheatPackRelationship::AmbiguousPossibleDuplicate);
        }
        if !ambiguous_information
            && raw_semantics.values().any(|v| v.len() > 1)
            && regions.len() < 2
            && revisions.len() < 2
            && engines.len() < 2
        {
            relations.insert(CheatPackRelationship::CodeConflict);
        }
        if regions.len() > 1 {
            relations.insert(CheatPackRelationship::RegionVariant);
        }
        if revisions.len() > 1 {
            relations.insert(CheatPackRelationship::RevisionVariant);
        }
        if engines.len() > 1 || formats.len() > 1 {
            relations.insert(CheatPackRelationship::SyntaxVariant);
        }
        if by_index.values().any(|v| v.len() > 1) {
            relations.insert(CheatPackRelationship::SourceIndexConflict);
        }
        let variant = relations.iter().any(|r| {
            matches!(
                r,
                CheatPackRelationship::RegionVariant
                    | CheatPackRelationship::RevisionVariant
                    | CheatPackRelationship::SyntaxVariant
            )
        });
        if !variant && !ambiguous_information && by_name.values().any(|v| v.len() > 1) {
            relations.insert(CheatPackRelationship::NameConflict);
        }
        for duplicate in contents.values().filter(|v| v.len() > 1) {
            let titles: BTreeSet<_> = duplicate
                .iter()
                .map(|&i| cheat_ir::reconciliation_title(&preview.observations[i].document.title))
                .collect();
            let raw: BTreeSet<_> = duplicate
                .iter()
                .map(|&i| &preview.observations[i].raw_code)
                .collect();
            relations.insert(if titles.len() == 1 && raw.len() == 1 {
                CheatPackRelationship::ExactDuplicate
            } else {
                CheatPackRelationship::EquivalentDuplicate
            });
            if duplicate
                .iter()
                .map(|&i| &preview.observations[i].provenance.source_sha256)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
            {
                relations.insert(CheatPackRelationship::CorroboratingObservation);
            }
        }
        let conflict = relations.iter().any(|r| {
            matches!(
                r,
                CheatPackRelationship::NameConflict
                    | CheatPackRelationship::CodeConflict
                    | CheatPackRelationship::SourceIndexConflict
                    | CheatPackRelationship::RegionVariant
                    | CheatPackRelationship::RevisionVariant
                    | CheatPackRelationship::SyntaxVariant
                    | CheatPackRelationship::AmbiguousPossibleDuplicate
            )
        });
        let verified = indices.iter().all(|&i| {
            matches!(
                preview.files[preview.observations[i].file_index].match_strength,
                CheatPackMatchStrength::Exact | CheatPackMatchStrength::Strong
            )
        });
        if !verified && indices.len() > 1 {
            relations.remove(&CheatPackRelationship::ExactDuplicate);
            relations.remove(&CheatPackRelationship::EquivalentDuplicate);
            relations.remove(&CheatPackRelationship::CorroboratingObservation);
            relations.insert(CheatPackRelationship::AmbiguousPossibleDuplicate);
        }
        let conflict = conflict || (!verified && indices.len() > 1);
        let mut reconciliation = if verified {
            match cheat_ir::reconcile_cheats_for_game(
                indices
                    .iter()
                    .map(|&i| entry_for(&preview.observations[i], &game_key, true))
                    .collect(),
            ) {
                CheatReconciliationOutcome::Ready(result) => Some(result),
                _ => None,
            }
        } else {
            None
        };
        if let Some(report) = &mut reconciliation {
            for group in &mut report.groups {
                if group.differences.len() > difference_budget {
                    group.differences.truncate(difference_budget);
                    preview.complete = false;
                }
                difference_budget = difference_budget.saturating_sub(group.differences.len());
            }
        }
        // Use stable content keys to order plan ownership; file order only breaks
        // ties among identical observations. No representative is selected for
        // activation, and every unsafe member remains individually classified.
        let mut added = BTreeSet::new();
        let mut retained_sources = BTreeSet::new();
        for &i in &indices {
            let o = &mut preview.observations[i];
            o.logical_key = keys[i].clone();
            o.action = match o.applicability {
                CheatPackApplicability::Malformed => CheatPackAction::WouldRejectMalformed,
                CheatPackApplicability::UnsupportedFormat
                | CheatPackApplicability::UnsupportedTarget => {
                    CheatPackAction::WouldRejectUnsupported
                }
                CheatPackApplicability::Unmatched => CheatPackAction::WouldRemainUnmatched,
                CheatPackApplicability::Ambiguous => CheatPackAction::WouldRemainAmbiguous,
                CheatPackApplicability::Ready if !conflict => {
                    if existing.contains(&o.logical_key) {
                        CheatPackAction::WouldRetainExisting
                    } else if added.insert(o.logical_key.clone()) {
                        retained_sources
                            .insert((o.logical_key.clone(), o.provenance.source_sha256.clone()));
                        CheatPackAction::WouldAdd
                    } else if retained_sources
                        .insert((o.logical_key.clone(), o.provenance.source_sha256.clone()))
                        && o.mirror_of.is_none()
                    {
                        CheatPackAction::WouldCorroborate
                    } else {
                        CheatPackAction::WouldRetainExisting
                    }
                }
                _ => CheatPackAction::WouldRequireReview,
            };
        }
        let group_keys: BTreeSet<_> = indices.iter().map(|&i| keys[i].clone()).collect();
        let key = if group_keys.len() == 1 {
            group_keys.first().cloned().unwrap_or_default()
        } else {
            digest("cheat-pack-conflict-v1", &group_keys)
        };
        preview.logical_cheats.push(CheatPackLogicalCheat {
            key,
            game_key,
            observation_indices: indices,
            relationships: relations,
            distinct_source_contents: provenance.len(),
            independent_source_groups: independent.len(),
            known_mirrors: mirrors.len(),
            known_copies: copy_paths
                .values()
                .map(|paths| paths.len().saturating_sub(1))
                .sum(),
            usable: !conflict && contents.len() == 1 && all_ready,
            reconciliation,
        });
    }
    preview.logical_cheats.sort_by(|a, b| a.key.cmp(&b.key));
    rebuild_totals(preview);
}

fn rebuild_totals(p: &mut CheatPackPreview) {
    let mut games = BTreeMap::<String, CheatPackGame>::new();
    let mut t = CheatPackTotals::default();
    t.files_discovered = p.files.len();
    for (i, f) in p.files.iter().enumerate() {
        t.files_readable += usize::from(f.provenance.is_some());
        match f.state {
            CheatPackFileState::Accepted => t.files_accepted += 1,
            CheatPackFileState::Malformed => t.files_malformed += 1,
            _ => t.files_rejected += 1,
        }
        if f.state == CheatPackFileState::Unsupported {
            t.unsupported_formats += 1;
        }
        if f.format.is_some()
            && f.provenance.is_some()
            && f.state != CheatPackFileState::Unsupported
        {
            games
                .entry(f.game_key.clone())
                .or_insert_with(|| CheatPackGame {
                    key: f.game_key.clone(),
                    file_indices: Vec::new(),
                    logical_cheat_indices: Vec::new(),
                    strength: f.match_strength,
                })
                .file_indices
                .push(i);
            if let Some(game) = games.get_mut(&f.game_key) {
                game.strength = game.strength.max(f.match_strength);
            }
        }
    }
    for (i, g) in p.logical_cheats.iter().enumerate() {
        if let Some(game) = games.get_mut(&g.game_key) {
            game.logical_cheat_indices.push(i);
        }
        t.usable_cheats += usize::from(g.usable);
        t.exact_duplicates += usize::from(
            g.relationships
                .contains(&CheatPackRelationship::ExactDuplicate),
        );
        t.equivalent_duplicates += usize::from(
            g.relationships
                .contains(&CheatPackRelationship::EquivalentDuplicate),
        );
        t.corroborating_sources += usize::from(
            g.relationships
                .contains(&CheatPackRelationship::CorroboratingObservation),
        );
        t.conflicts += usize::from(g.relationships.iter().any(|r| {
            matches!(
                r,
                CheatPackRelationship::NameConflict
                    | CheatPackRelationship::CodeConflict
                    | CheatPackRelationship::SourceIndexConflict
            )
        }));
    }
    p.games = games.into_values().collect();
    for g in &p.games {
        match g.strength {
            CheatPackMatchStrength::Exact => t.exact_matches += 1,
            CheatPackMatchStrength::Strong => t.strong_matches += 1,
            CheatPackMatchStrength::Possible => t.possible_matches += 1,
            CheatPackMatchStrength::Ambiguous => t.ambiguous_matches += 1,
            CheatPackMatchStrength::Unmatched => t.unmatched_games += 1,
        }
    }
    for o in &p.observations {
        match o.applicability {
            CheatPackApplicability::Malformed => t.malformed_cheats += 1,
            CheatPackApplicability::WrongRegion => t.region_mismatches += 1,
            CheatPackApplicability::WrongRevision => t.revision_mismatches += 1,
            CheatPackApplicability::UnsupportedTarget => t.unsupported_targets += 1,
            _ => {}
        }
        match o.action {
            CheatPackAction::WouldAdd => t.would_add += 1,
            CheatPackAction::WouldCorroborate => t.would_corroborate += 1,
            CheatPackAction::WouldRetainExisting => t.would_retain_existing += 1,
            CheatPackAction::WouldRequireReview => t.would_review += 1,
            CheatPackAction::WouldRejectMalformed | CheatPackAction::WouldRejectUnsupported => {
                t.would_reject += 1
            }
            CheatPackAction::WouldRemainUnmatched => t.would_remain_unmatched += 1,
            CheatPackAction::WouldRemainAmbiguous => t.would_remain_ambiguous += 1,
        }
    }
    t.needs_review = t.would_review + t.would_remain_ambiguous;
    t.games_represented = p.games.len();
    t.observations = p.observations.len();
    t.logical_cheats = p.logical_cheats.len();
    p.totals = t;
}

/// One pack-wide retention budget, not a fresh budget per source file.
fn bound_diagnostics(p: &mut CheatPackPreview) {
    let mut remaining = p.limits.source.max_warnings;
    if p.diagnostics.len() > remaining {
        p.diagnostics.truncate(remaining);
        p.complete = false;
    }
    remaining = remaining.saturating_sub(p.diagnostics.len());
    for f in &mut p.files {
        if f.diagnostics.len() > remaining {
            f.diagnostics.truncate(remaining);
            f.diagnostics_truncated = true;
            p.complete = false;
        }
        remaining = remaining.saturating_sub(f.diagnostics.len());
        for &i in &f.observation_indices {
            let o = &mut p.observations[i];
            if o.diagnostics.len() > remaining {
                o.diagnostics.truncate(remaining);
                o.diagnostics_truncated = true;
                p.complete = false;
            }
            remaining = remaining.saturating_sub(o.diagnostics.len());
        }
    }
}
