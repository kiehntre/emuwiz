//! Candidate discovery only. A provider edge never establishes completeness;
//! the shared storage classifier and collection dependency resolver do that.
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use super::{MemberClass, classify_disk_member, declared_disks, declared_roms};
use crate::dat::{
    dependency::{
        graph::{DeclaredName, DependencyGraph, SetRef, declared_name},
        resolve::borrow_provider,
    },
    model::DatGameEntry,
};

pub(super) fn borrowed_candidates<'a>(
    games: &'a [DatGameEntry],
    anchors: &BTreeSet<&str>,
) -> Vec<&'a str> {
    let graph = DependencyGraph::build(games);
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for (index, game) in games.iter().enumerate() {
        let mut disks = declared_disks(index, game).peekable();
        if declared_roms(index, game).next().is_some()
            || disks.peek().is_none()
            || !disks.all(|(_, disk)| classify_disk_member(disk) == MemberClass::Borrowed)
        {
            continue;
        }
        if let Some(provider) = borrow_provider(game)
            && let DeclaredName::Named(name) = declared_name(provider)
            && let SetRef::Unique(provider) = graph.resolve_set(name)
        {
            children.entry(provider).or_default().push(index);
        }
    }

    // Start only from this run's positional evidence. IDs, name collisions
    // and romof/cloneof precedence follow exactly the dependency resolver.
    let mut reached = HashSet::new();
    let mut pending = VecDeque::new();
    for name in anchors {
        if let SetRef::Unique(index) = graph.resolve_set(name)
            && reached.insert(index)
        {
            pending.push_back(index);
        }
    }
    let mut candidates = Vec::new();
    while let Some(provider) = pending.pop_front() {
        for &index in children.get(&provider).into_iter().flatten() {
            if reached.insert(index) {
                candidates.push(games[index].name.as_str());
                pending.push_back(index);
            }
        }
    }
    candidates
}
