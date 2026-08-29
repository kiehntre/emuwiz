//! Read-only "Build Playing Library" planner: elect one representative
//! release per authoritatively-grouped game family and propose non-
//! destructive linked-library symlinks.
//!
//! See [`model`] for the identity rule and election contract, and
//! [`evidence`] for what counts as trusted release metadata.
//!
//! # What this pass is
//!
//! Core/planner only. There is deliberately no GUI surface, no sidebar
//! entry, and no filesystem mutation anywhere in this module:
//! [`build_playing_library_plan`] performs zero I/O - it works purely on
//! caller-supplied parsed-DAT facts.
//!
//! # Apply seam (report, intentionally unwired)
//!
//! Elected operations map onto the existing linked-library apply path:
//! [`crate::dat::rename_apply`]'s durable-journal engine with
//! [`crate::dat::rom_organisation::OrganisationMode::BuildLinkedLibrary`]
//! produces exactly the needed no-clobber, crash-reconcilable,
//! rollback-safe symlink transactions for an approved subset of proposals.
//! Wiring that conversion (including platform resolution for organised
//! destinations) is a deliberate follow-up so this planning model lands
//! independently mergeable without touching `rename_apply`,
//! `rom_organisation`, or any GUI page.

pub mod apply_adapter;
pub mod evidence;
pub mod matching;
pub mod model;
pub mod retrodeck_projection;
pub mod romm_library_plan;
pub mod romm_projection;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::dat::dependency::graph::{DependencyGraph, SetRef};
use crate::dat::model::ParsedDat;

pub use apply_adapter::build_playing_library_transaction;
pub use matching::match_loose_files_against_dat;
pub use model::{
    CandidateEvidenceSummary, DestinationConflict, ElectedGame, ElectionExplanation,
    ExcludedCandidate, LinkedLibraryOperation, PlayingLibraryCandidate, PlayingLibraryPlan,
    PlayingLibraryPolicy, RejectedCandidate, RejectedLauncher, ReleaseClass, RevisionNumber,
    UnresolvedGroup,
};
pub use retrodeck_projection::{
    RetroDeckProjectedGame, RetroDeckProjectionPlan, RetroDeckVisibility,
    build_retrodeck_projection, build_retrodeck_projection_transaction,
};
pub use romm_library_plan::{
    RommLibraryBlockReason, RommLibraryBlockedPlatform, RommLibraryOperationKind, RommLibraryPlan,
    RommLibraryPlanEntry, RommLibraryPlatformInput, build_romm_library_apply_transactions,
    build_romm_library_plan,
};
pub use romm_projection::{
    RommLibraryProjectionPlan, RommProjectedGame, RommVisibility, build_romm_projection,
    build_romm_projection_transaction, build_romm_projection_with_visibility,
};

/// How far a clone chain may be walked while resolving one family root.
///
/// Mirrors `crate::dat::dependency::MAX_DEPENDENCY_DEPTH`: far above any
/// legitimate catalogue depth, small enough that a corrupt cycle-bound
/// declaration turns into a defined stop instead of unbounded work.
const MAX_FAMILY_DEPTH: usize = 64;

/// One verified archive-to-catalogue match the planner may trust.
///
/// Producing this match is the caller's job (hash evidence against an
/// indexed catalogue), exactly like every other audit flow; this planner
/// never re-verifies and never guesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatArchiveMatch {
    /// The untouched source file a playing-library link would point at -
    /// the launcher file (a CUE/GDI/M3U) for a multi-file release, or the
    /// sole file for an ordinary single-file release.
    pub archive_path: PathBuf,
    /// Index into [`ParsedDat::games`] this archive was verified against.
    /// For a multi-file release this is the entry [`Self::archive_path`]'s
    /// own primary track/disc verified against; [`Self::companion_paths`]
    /// need not each have their own distinct DAT entry (a CUE's `.bin`
    /// tracks each verify against their own `<rom>`, but the `.cue`/`.gdi`/
    /// `.m3u` file itself is never individually hashed by any real
    /// provider).
    pub dat_entry_index: usize,
    /// Every other file this release requires alongside
    /// [`Self::archive_path`] - already verified/safety-checked by the
    /// caller (see [`matching::detect_multi_file_matches`]). Empty for an
    /// ordinary single-file match, which behaves exactly as before this
    /// field existed.
    pub companion_paths: Vec<PathBuf>,
}

/// Everything one read-only planning run needs.
#[derive(Debug, Clone)]
pub struct PlayingLibraryRequest<'a> {
    /// The already-parsed catalogue whose facts back every decision.
    pub dat: &'a ParsedDat,
    pub matches: Vec<DatArchiveMatch>,
    /// Where proposed links will live under the eventual linked-library
    /// apply. Never created, read, or written during planning.
    pub destination_root: PathBuf,
    pub policy: PlayingLibraryPolicy,
}

/// Builds the read-only playing-library plan. Never touches the filesystem.
pub fn build_playing_library_plan(
    request: &PlayingLibraryRequest<'_>,
) -> Result<PlayingLibraryPlan, String> {
    // Candidate indexes must be real entries in the supplied catalogue: an
    // out-of-range match is a caller bug, not something to silently skip.
    for matched in &request.matches {
        if matched.dat_entry_index >= request.dat.games.len() {
            return Err(format!(
                "match names DAT entry {} but the catalogue has only {} games",
                matched.dat_entry_index,
                request.dat.games.len()
            ));
        }
    }

    let graph = DependencyGraph::build(&request.dat.games);
    // Family root -> that family's matched archives, keyed by the resolved
    // root's declaration position so plan output is deterministic.
    let mut families: BTreeMap<usize, Vec<DatArchiveMatch>> = BTreeMap::new();
    let mut ordered: Vec<&DatArchiveMatch> = request.matches.iter().collect();
    ordered.sort_by_key(|matched| matched.dat_entry_index);
    for matched in ordered {
        let root = resolve_family_root(&graph, matched.dat_entry_index);
        families.entry(root).or_default().push(matched.clone());
    }

    let mut plan = PlayingLibraryPlan {
        destination_root: request.destination_root.clone(),
        policy: request.policy.clone(),
        archives_examined: request.matches.len(),
        families_examined: families.len(),
        elected_games: Vec::new(),
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: 0,
        conflicts: Vec::new(),
        operations: Vec::new(),
        rejected_launchers: Vec::new(),
    };

    for (root_index, members) in &families {
        if members.len() == 1 {
            plan.singleton_families += 1;
        }
        elect_family(request, *root_index, members, &mut plan);
    }
    mark_destination_conflicts(&mut plan);
    // A conflicted destination is reported, never proposed again: nothing in
    // this plan may overwrite it at apply time. Requirement 10: collision
    // handling covers the *whole* release atomically - if any one of a
    // release's own operations (launcher or a companion) collides, every
    // operation for that same election is excluded together, never just
    // the one colliding file (which would otherwise propose linking a CUE
    // without its BIN, or a BIN without its CUE).
    let conflicted_destinations: BTreeSet<&PathBuf> = plan
        .conflicts
        .iter()
        .flat_map(|conflict| &conflict.destinations)
        .collect();
    plan.operations = plan
        .elected_games
        .iter()
        .filter(|elected| {
            !elected
                .all_operations()
                .any(|operation| conflicted_destinations.contains(&operation.destination_path))
        })
        .flat_map(|elected| elected.all_operations().cloned())
        .collect();
    Ok(plan)
}

/// Walks declared parent/clone chains up to the family root.
///
/// The chain is the authoritative grouping evidence. A reference that does
/// not resolve uniquely (absent target, duplicated name/`id`, contradictory
/// name-vs-`id`) or a revisited node (cycle) stops the walk where it stands;
/// that entry becomes the root of its own subfamily. Nothing is ever merged
/// across a broken or ambiguous link - false negatives stay acceptable here,
/// exactly as everywhere else in this module. There is deliberately no
/// filename-similarity fallback anywhere in this file.
fn resolve_family_root(graph: &DependencyGraph<'_>, start: usize) -> usize {
    let mut visited = BTreeSet::new();
    let mut current = start;
    loop {
        if !visited.insert(current) || visited.len() > MAX_FAMILY_DEPTH {
            return current;
        }
        let Some(reference) = graph
            .game(current)
            .clone_of
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return current;
        };
        match graph.resolve_set(reference) {
            SetRef::Unique(parent) => current = parent,
            _ => return current,
        }
    }
}

/// One family's full election: class-exclusion filter, then the fixed tier
/// sequence region -> language -> revision -> declared parent, narrating
/// every elimination. No alphabetical fallback; no score.
fn elect_family(
    request: &PlayingLibraryRequest<'_>,
    root_index: usize,
    members: &[DatArchiveMatch],
    plan: &mut PlayingLibraryPlan,
) {
    let evidence_of =
        |dat_index: usize| evidence::dat_release_evidence(&request.dat.games[dat_index].name);
    let name_of = |dat_index: usize| -> String { request.dat.games[dat_index].name.clone() };

    // Tier 0: explicit class exclusion. A candidate is excluded only when
    // its canonical DAT name carries a matching strict token; unknown
    // release status is never excluded by anything.
    let mut survivors: Vec<usize> = Vec::new();
    for (position, member) in members.iter().enumerate() {
        let classes_found = evidence_of(member.dat_entry_index).release_classes.clone();
        let excluded: Vec<ReleaseClass> = classes_found
            .iter()
            .copied()
            .filter(|class| request.policy.excluded_release_classes.contains(class))
            .collect();
        if excluded.is_empty() {
            survivors.push(position);
        } else {
            plan.exclusions.push(ExcludedCandidate {
                dat_entry_name: name_of(member.dat_entry_index),
                source_path: member.archive_path.clone(),
                excluded_classes: excluded
                    .iter()
                    .map(|class| class.label().to_string())
                    .collect(),
            });
        }
    }
    if survivors.is_empty() {
        return;
    }

    let mut reasons: Vec<Vec<String>> = vec![Vec::new(); members.len()];
    let mut steps: Vec<String> = Vec::new();

    // The policy-empty case disables its tier entirely: no preference was
    // expressed, so that field must not decide or explain anything.
    run_rank_tier(
        "preferred region",
        !request.policy.preferred_regions.is_empty(),
        &mut survivors,
        &mut reasons,
        &mut steps,
        |position| {
            rank_in_preference(
                &evidence_of(members[position].dat_entry_index).regions,
                &request.policy.preferred_regions,
            )
        },
        |position| {
            preference_label(
                &evidence_of(members[position].dat_entry_index).regions,
                &request.policy.preferred_regions,
            )
        },
        "lower preferred-region rank",
        members,
        &name_of,
    );
    run_rank_tier(
        "preferred language",
        !request.policy.preferred_languages.is_empty(),
        &mut survivors,
        &mut reasons,
        &mut steps,
        |position| {
            rank_in_preference(
                &evidence_of(members[position].dat_entry_index).languages,
                &request.policy.preferred_languages,
            )
        },
        |position| {
            preference_label(
                &evidence_of(members[position].dat_entry_index).languages,
                &request.policy.preferred_languages,
            )
        },
        "lower preferred-language rank",
        members,
        &name_of,
    );

    if request.policy.prefer_newest_revision && survivors.len() > 1 {
        run_revision_tier(
            &mut survivors,
            &mut reasons,
            &mut steps,
            members,
            &name_of,
            evidence_of,
        );
    }

    if request.policy.prefer_parent && survivors.len() > 1 {
        keep_declared_parents(
            root_index,
            &mut survivors,
            &mut reasons,
            &mut steps,
            members,
            &name_of,
        );
    }

    if survivors.len() == 1 {
        if steps.is_empty() {
            steps.push("the only election-eligible release in its family".to_string());
        }
        record_election(
            plan,
            request,
            root_index,
            members,
            survivors[0],
            steps,
            &reasons,
        );
    } else {
        finish_unresolved(request, root_index, members, &survivors, plan);
    }
}

/// A rank tier: every candidate is ranked against the ordered preference
/// list; the best rank survives, the rest are narrated away. No preference
/// expressed (empty list) or a single survivor disables the tier entirely.
fn run_rank_tier(
    tier_label: &'static str,
    enabled: bool,
    survivors: &mut Vec<usize>,
    reasons: &mut [Vec<String>],
    steps: &mut Vec<String>,
    rank_of: impl Fn(usize) -> usize,
    label_of: impl Fn(usize) -> String,
    loser_reason: &'static str,
    members: &[DatArchiveMatch],
    name_of: &dyn Fn(usize) -> String,
) {
    if !enabled || survivors.len() <= 1 {
        return;
    }
    let mut ranked: Vec<(usize, usize)> = survivors
        .iter()
        .copied()
        .map(|position| (rank_of(position), position))
        .collect();
    ranked.sort();
    let best_rank = ranked[0].0;
    let eliminated: Vec<usize> = ranked
        .iter()
        .filter(|(rank, _)| *rank != best_rank)
        .map(|(_, position)| *position)
        .collect();
    if eliminated.is_empty() {
        return;
    }
    let winner = ranked[0].1;
    survivors.retain(|position| rank_of(*position) == best_rank);
    for position in &eliminated {
        reasons[*position].push(loser_reason.to_string());
    }
    steps.push(format!(
        "{} {} ranked above {}",
        tier_label,
        label_of(winner),
        position_names(&eliminated, members, name_of),
    ));
}

/// The newest strictly-verified revision wins when the policy enables it.
/// Entries without any revision token lose only when some other entry has
/// one - absence alone never decides, and equal revisions all tie on.
fn run_revision_tier(
    survivors: &mut Vec<usize>,
    reasons: &mut [Vec<String>],
    steps: &mut Vec<String>,
    members: &[DatArchiveMatch],
    name_of: &dyn Fn(usize) -> String,
    evidence_of: impl Fn(usize) -> evidence::DatReleaseEvidence + Copy,
) {
    let newest = survivors
        .iter()
        .filter_map(|position| evidence_of(members[*position].dat_entry_index).revision)
        .max();
    let Some(newest_revision) = newest else {
        return; // nobody has verified revision evidence; undecided here
    };
    let keeps = |position: usize| {
        evidence_of(members[position].dat_entry_index).revision == Some(newest_revision)
    };
    let eliminated: Vec<usize> = survivors
        .iter()
        .copied()
        .filter(|position| !keeps(*position))
        .collect();
    if eliminated.is_empty() {
        return;
    }
    survivors.retain(|position| keeps(*position));
    for position in &eliminated {
        reasons[*position].push(format!(
            "older or absent verified revision (newest verified is {})",
            format_revision(&newest_revision)
        ));
    }
    steps.push(format!(
        "verified revision {} ranked above older or unverified revisions ({})",
        format_revision(&newest_revision),
        position_names(&eliminated, members, name_of),
    ));
}

/// Keeps entries that are themselves the declared family parent. Tiers that
/// cannot distinguish anything (no parent among survivors, or all of them
/// are parents) are skipped, never guessed.
fn keep_declared_parents(
    root_index: usize,
    survivors: &mut Vec<usize>,
    reasons: &mut [Vec<String>],
    steps: &mut Vec<String>,
    members: &[DatArchiveMatch],
    name_of: &dyn Fn(usize) -> String,
) {
    let keeps = |position: usize| members[position].dat_entry_index == root_index;
    if !survivors.iter().copied().any(keeps) {
        return;
    }
    let eliminated: Vec<usize> = survivors
        .iter()
        .copied()
        .filter(|position| !keeps(*position))
        .collect();
    if eliminated.is_empty() {
        return;
    }
    survivors.retain(|position| keeps(*position));
    for position in &eliminated {
        reasons[*position].push("a clone of the declared parent".to_string());
    }
    steps.push(format!(
        "the declared parent ranked above its clones ({})",
        position_names(&eliminated, members, name_of),
    ));
}

fn position_names(
    positions: &[usize],
    members: &[DatArchiveMatch],
    name_of: &dyn Fn(usize) -> String,
) -> String {
    positions
        .iter()
        .map(|position| name_of(members[*position].dat_entry_index))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Records an elected game and its proposed linked-library operation(s).
/// The launcher operation points at the original archive/launcher path
/// verbatim; each companion (if any) points at its own untouched source
/// file - planning never rewrites, reorders, or renames source material.
fn record_election(
    plan: &mut PlayingLibraryPlan,
    request: &PlayingLibraryRequest<'_>,
    root_index: usize,
    members: &[DatArchiveMatch],
    winner_position: usize,
    mut steps: Vec<String>,
    reasons: &[Vec<String>],
) {
    let evidence_summary_of = |position: usize| -> CandidateEvidenceSummary {
        let member = &members[position];
        let entry = &request.dat.games[member.dat_entry_index];
        let release_evidence = evidence::dat_release_evidence(&entry.name);
        CandidateEvidenceSummary {
            regions: release_evidence.regions,
            languages: release_evidence.languages,
            revision: release_evidence.revision.as_ref().map(format_revision),
            is_declared_parent: member.dat_entry_index == root_index,
            is_declared_clone: entry.clone_of.is_some(),
            companion_file_count: member.companion_paths.len(),
        }
    };
    let rejected = (0..members.len())
        .filter(|position| *position != winner_position && !reasons[*position].is_empty())
        .map(|position| RejectedCandidate {
            dat_entry_name: request.dat.games[members[position].dat_entry_index]
                .name
                .clone(),
            source_path: members[position].archive_path.clone(),
            reasons: reasons[position].clone(),
            evidence: evidence_summary_of(position),
        })
        .collect();
    let winner_evidence = evidence_summary_of(winner_position);
    let member = &members[winner_position];
    let launcher_operation = LinkedLibraryOperation {
        source_path: member.archive_path.clone(),
        destination_path: proposed_destination(&plan.destination_root, &member.archive_path),
    };
    let companion_operations: Vec<LinkedLibraryOperation> = member
        .companion_paths
        .iter()
        .map(|companion| LinkedLibraryOperation {
            source_path: companion.clone(),
            destination_path: proposed_destination(&plan.destination_root, companion),
        })
        .collect();
    if !companion_operations.is_empty() {
        steps.push(format!(
            "{} companion file(s) required to play this release are included alongside it",
            companion_operations.len()
        ));
    }
    plan.operations.push(launcher_operation.clone());
    plan.operations.extend(companion_operations.iter().cloned());
    plan.elected_games.push(ElectedGame {
        dat_entry_name: request.dat.games[member.dat_entry_index].name.clone(),
        family_root_name: request.dat.games[root_index].name.clone(),
        explanation: ElectionExplanation {
            steps,
            rejected,
            winner_evidence,
        },
        launcher_operation,
        companion_operations,
    });
}

/// Deterministic unresolve record: names only, fixed reason text.
fn finish_unresolved(
    request: &PlayingLibraryRequest<'_>,
    root_index: usize,
    members: &[DatArchiveMatch],
    survivors: &[usize],
    plan: &mut PlayingLibraryPlan,
) {
    plan.unresolved_groups.push(UnresolvedGroup {
        family_root_name: request.dat.games[root_index].name.clone(),
        tied_candidates: survivors
            .iter()
            .map(|position| {
                request.dat.games[members[*position].dat_entry_index]
                    .name
                    .clone()
            })
            .collect(),
        reason: "candidates remain indistinguishable under this policy after every trusted \
                 field was compared; no alphabetical or arbitrary fallback exists"
            .to_string(),
    });
}

/// Ranks a candidate's parsed tokens against the ordered preference list:
/// the earliest preferred value with any case-insensitive match wins; no
/// match anywhere ranks last. Never called "bad" - only "ranked below".
fn rank_in_preference(values: &[String], preferences: &[String]) -> usize {
    for (index, preference) in preferences.iter().enumerate() {
        if values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(preference))
        {
            return index;
        }
    }
    preferences.len()
}

/// Human label of a candidate's best matched preferred value, used only in
/// narration lines.
fn preference_label(values: &[String], preferences: &[String]) -> String {
    for preference in preferences {
        if values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(preference))
        {
            return format!("\"{}\"", preference);
        }
    }
    "\"(no match)\"".to_string()
}

/// Renders a parsed revision as its canonical DAT token text.
pub fn format_revision(revision: &RevisionNumber) -> String {
    let mut text = revision.major.to_string();
    if revision.minor > 0 {
        text.push('.');
        text.push_str(&revision.minor.to_string());
    }
    if revision.letter != '\0' {
        text.push(revision.letter);
    }
    text
}

/// The destination one proposed link would occupy: the destination root plus
/// the source archive's own file name. Planning never renames anything; the
/// display-name question belongs to a later milestone.
fn proposed_destination(destination_root: &Path, source_path: &Path) -> PathBuf {
    match source_path.file_name() {
        Some(file_name) => destination_root.join(file_name),
        None => destination_root.join(source_path),
    }
}

/// Detects destinations two different elections would fight over and marks
/// them as conflicts - planning never resolves a name clash by overwriting.
/// Case-insensitive clashes (casefolded equality) count too, matching the
/// destination discipline of the existing organisation planner.
///
/// Every operation an election proposes is checked - launcher and every
/// companion alike - so a collision on a companion file (e.g. two
/// different CUE-based releases each proposing a `track01.bin` at the
/// same destination) is caught exactly like a launcher collision. See
/// requirement 10 in the module's own design notes: [`build_playing_library_plan`]
/// then excludes a conflicted election's operations *as a whole*, never
/// just the one colliding file, so a release is never proposed half
/// linked.
fn mark_destination_conflicts(plan: &mut PlayingLibraryPlan) {
    let mut seen: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for (index, elected) in plan.elected_games.iter().enumerate() {
        for operation in elected.all_operations() {
            let Some(basename) = operation.destination_path.file_name() else {
                continue;
            };
            seen.entry(basename.to_string_lossy().to_ascii_lowercase())
                .or_default()
                .insert(index);
        }
    }
    let clashing: Vec<(String, BTreeSet<usize>)> = seen
        .into_iter()
        .filter(|(_, indexes)| indexes.len() > 1)
        .collect();
    for (basename, indexes) in clashing {
        let indexes: Vec<usize> = indexes.into_iter().collect();
        let conflict = DestinationConflict {
            destination_basename: basename.clone(),
            contenders: indexes
                .iter()
                .map(|index| plan.elected_games[*index].dat_entry_name.clone())
                .collect(),
            destinations: indexes
                .iter()
                .filter_map(|index| {
                    plan.elected_games[*index]
                        .all_operations()
                        .find(|operation| {
                            operation.destination_path.file_name().is_some_and(|name| {
                                name.to_string_lossy().to_ascii_lowercase() == basename
                            })
                        })
                        .map(|operation| operation.destination_path.clone())
                })
                .collect(),
        };
        plan.conflicts.push(conflict);
    }
}

#[cfg(test)]
mod tests;
