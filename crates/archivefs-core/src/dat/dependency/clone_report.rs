//! A per-entry report of every game's declared `cloneof` relationship.
//!
//! This is deliberately a *different*, smaller question than the rest of
//! this module answers. Stage 2c ([`crate::dat::set`]) asks whether a
//! set's own physical storage is present; Stage 2d ([`super::resolve`])
//! asks whether a set's dependency *requirements* are satisfied by real
//! catalogue members. Neither reports, on its own, "does this catalogue's
//! parent/clone naming graph itself make sense" - independent of whether
//! any file has ever been matched against it. [`report_clone_relationships`]
//! answers exactly that, for display and for the Playing Library's own
//! family grouping to point users at when something looks wrong.
//!
//! It is built entirely from [`super::graph::DependencyGraph`] - the
//! crate's one auditable name/id-to-entry resolver - and
//! [`super::graph::ChainGuard`], the same cycle/depth guard every other
//! chain walk in this crate uses. No second identity-resolution or
//! cycle-detection algorithm exists here.
//!
//! # Retool-style clone-list ingestion: deferred, not guessed
//!
//! The task this module was written for also asked for "Retool/clone-list
//! ingestion" - ingesting a reviewed, external 1G1R selection/clone list
//! (as published by the Retool project) as a second source of parent/clone
//! evidence alongside a DAT's own `cloneof`/`romof`/`merge` fields.
//!
//! That work is deliberately **not** implemented. Retool publishes its
//! selection lists as version-controlled plain-text files without a single
//! stable, machine-readable schema this crate could parse without
//! guessing at its shape from examples - exactly the kind of "invented
//! schema" the task explicitly forbade, and this crate has no runtime
//! network access with which to consult Retool's own documentation for an
//! authoritative one. Implementing an importer against a guessed format
//! would silently misclassify real catalogues the moment that guess was
//! wrong, which is a worse outcome than not having the feature at all.
//!
//! What *is* implemented is the complete, honest half of Feature 2: every
//! parent/clone relationship a DAT already declares is preserved and
//! reported exactly as this module describes, including every way such a
//! declaration can be broken (missing target, ambiguous target, malformed
//! declaration, or a cycle). A future change that can cite an authoritative
//! Retool export schema can add a second evidence source alongside this
//! one without altering anything here.

use super::graph::{ChainFault, ChainGuard, DeclaredName, DependencyGraph, SetRef, declared_name};
use crate::dat::model::DatGameEntry;

/// What a game's declared `clone_of` resolves to, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneRelationshipStatus {
    /// No `clone_of` was declared at all. This entry is its own family
    /// root as far as this report is concerned.
    NoRelationshipDeclared,
    /// `clone_of` was declared but empty/whitespace-only: a relationship
    /// was stated and then named nothing.
    MalformedDeclaration,
    /// `clone_of` names an entry that does not exist anywhere in this
    /// catalogue.
    MissingParent { declared_reference: String },
    /// `clone_of` resolves to more than one entry: a duplicated name, a
    /// duplicated `id`, or a name match and an `id` match that disagree.
    ConflictingReference { declared_reference: String },
    /// Walking the chain from this entry toward its ultimate root
    /// revisited an entry already on the path. Reported, never silently
    /// broken or merged into either side of the cycle.
    Cycle { declared_reference: String },
    /// `clone_of` resolves to exactly one entry, and every further hop up
    /// to the ultimate family root (an entry with no `clone_of` of its
    /// own, or whose own reference stops being resolvable) is itself
    /// resolvable and cycle-free.
    Resolved {
        parent_index: usize,
        parent_name: String,
        root_index: usize,
        root_name: String,
    },
}

/// One game's clone-relationship report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneRelationshipReport {
    pub game_index: usize,
    pub game_name: String,
    pub status: CloneRelationshipStatus,
}

/// Reports every game's declared `clone_of` relationship, preserving
/// exactly what the catalogue declares - nothing here infers a
/// relationship from a name, a filename, or any external list.
pub fn report_clone_relationships(games: &[DatGameEntry]) -> Vec<CloneRelationshipReport> {
    let graph = DependencyGraph::build(games);
    games
        .iter()
        .enumerate()
        .map(|(index, game)| {
            let status = match declared_name(&game.clone_of) {
                DeclaredName::Absent => CloneRelationshipStatus::NoRelationshipDeclared,
                DeclaredName::Malformed => CloneRelationshipStatus::MalformedDeclaration,
                DeclaredName::Named(reference) => match graph.resolve_set(reference) {
                    SetRef::Absent => CloneRelationshipStatus::MissingParent {
                        declared_reference: reference.to_string(),
                    },
                    SetRef::Duplicate => CloneRelationshipStatus::ConflictingReference {
                        declared_reference: reference.to_string(),
                    },
                    SetRef::Unique(parent_index) => {
                        match walk_to_root(&graph, index, parent_index) {
                            Ok(root_index) => CloneRelationshipStatus::Resolved {
                                parent_index,
                                parent_name: games[parent_index].name.clone(),
                                root_index,
                                root_name: games[root_index].name.clone(),
                            },
                            Err(ChainFault::Cycle | ChainFault::DepthExceeded) => {
                                CloneRelationshipStatus::Cycle {
                                    declared_reference: reference.to_string(),
                                }
                            }
                        }
                    }
                },
            };
            CloneRelationshipReport {
                game_index: index,
                game_name: game.name.clone(),
                status,
            }
        })
        .collect()
}

/// Walks the chain from `start` (already visited) through `first_hop` up
/// to its ultimate root, cycle- and depth-guarded by the same
/// [`ChainGuard`] every other chain walk in this crate uses. Stops (with
/// `Ok`) the moment an ancestor's own reference is absent, malformed, or
/// itself unresolvable - `current` at that point is still a perfectly
/// good, resolvable ancestor of `start`, just not one this report can walk
/// any further past.
fn walk_to_root(
    graph: &DependencyGraph<'_>,
    start: usize,
    first_hop: usize,
) -> Result<usize, ChainFault> {
    let mut guard = ChainGuard::starting_at(start);
    guard.visit(first_hop)?;
    let mut current = first_hop;
    loop {
        let next_reference = match declared_name(&graph.game(current).clone_of) {
            DeclaredName::Named(reference) => reference,
            DeclaredName::Absent | DeclaredName::Malformed => return Ok(current),
        };
        match graph.resolve_set(next_reference) {
            SetRef::Unique(next) => {
                guard.visit(next)?;
                current = next;
            }
            SetRef::Absent | SetRef::Duplicate => return Ok(current),
        }
    }
}

#[cfg(test)]
mod tests;
