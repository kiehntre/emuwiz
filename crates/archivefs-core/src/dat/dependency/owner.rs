//! Where a ROM member's content is declared, by catalogue facts only.
//!
//! Clone ancestry (`cloneof`) and ROM sourcing (`romof` plus `<rom merge>`)
//! are different relationships. A member with no `merge=` belongs to the set
//! that declares it, whatever that set's parent is: a clone's own ROM is never
//! "the parent's", and a parent's ROMs are never a clone's requirements. A
//! member with `merge=` borrows a named declaration from its set's ROM source
//! and may borrow again from there (a clone borrowing from its parent, which
//! borrows from a BIOS). This module follows that chain to the declaration
//! that owns the content and refuses, with a typed outcome, whenever the
//! catalogue does not prove a single owner.
//!
//! The provider of a borrow is the same one [`super::resolve`] uses:
//! `romof`, or `cloneof` only when the set declares no `romof` at all.

use crate::dat::model::{ChecksumAlgorithm, DatRomEntry};

use super::DependencyOutcome;
use super::graph::{ChainFault, ChainGuard, DeclaredName, DependencyGraph, MemberRef, SetRef};
use super::resolve::{borrow_provider, checksums_conflict};

/// The declaration that owns one member's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RomOwner<'a> {
    /// No `merge=`: the declaring set owns it.
    Own,
    /// Borrowed: `declaration` in catalogue set `owner` holds the content.
    Borrowed {
        owner: usize,
        declaration: &'a DatRomEntry,
    },
}

/// Resolves the owner of `rom`, declared by catalogue set `index`.
///
/// Refuses as `Contradictory` when a `merge=` has no provider, names its own
/// set, names a member the provider does not declare, or disagrees with the
/// provider's checksum or size; `Ambiguous` when a set or member name is
/// declared more than once; `Cycle`/`Unsupported` for a looping or over-deep
/// chain; and `EvidenceUnavailable` when the provider set is absent.
pub(crate) fn rom_owner<'a>(
    graph: &DependencyGraph<'a>,
    index: usize,
    rom: &'a DatRomEntry,
) -> Result<RomOwner<'a>, DependencyOutcome> {
    rom_owner_with(graph, index, rom, |_| Ok(()))
}

/// One step of a borrow chain, offered to a caller's stricter checks.
pub(crate) enum OwnerHop<'a> {
    /// The provider set was resolved; its declaration is not yet read.
    Provider(usize),
    /// `declaration` in set `provider` passed every shared check.
    Declaration {
        provider: usize,
        declaration: &'a DatRomEntry,
    },
}

/// [`rom_owner`] with a hook that may refuse any step. The shared rules
/// always apply first; a caller can only add refusals, never relax one.
pub(crate) fn rom_owner_with<'a>(
    graph: &DependencyGraph<'a>,
    index: usize,
    rom: &'a DatRomEntry,
    mut hop: impl FnMut(OwnerHop<'a>) -> Result<(), DependencyOutcome>,
) -> Result<RomOwner<'a>, DependencyOutcome> {
    let mut wanted = match super::graph::declared_name(&rom.merge) {
        DeclaredName::Absent => return Ok(RomOwner::Own),
        DeclaredName::Malformed => return Err(DependencyOutcome::Contradictory),
        DeclaredName::Named(name) => name,
    };
    let mut current = index;
    let mut borrower = rom;
    // Every declaration must agree with every borrow before it in the
    // chain, not only with its neighbour.
    let mut borrowers = vec![rom];
    let mut guard = ChainGuard::starting_at(index);
    loop {
        let declaring = graph.game(current);
        let Some(provider) = borrow_provider(declaring) else {
            return Err(DependencyOutcome::Contradictory);
        };
        let provider = match super::graph::declared_name(provider) {
            DeclaredName::Named(name) => name,
            DeclaredName::Absent | DeclaredName::Malformed => {
                return Err(DependencyOutcome::Contradictory);
            }
        };
        if provider == declaring.name.trim() {
            return Err(DependencyOutcome::Contradictory);
        }
        let provider = match graph.resolve_set(provider) {
            SetRef::Unique(provider) => provider,
            SetRef::Duplicate => return Err(DependencyOutcome::Ambiguous),
            SetRef::Absent => return Err(DependencyOutcome::EvidenceUnavailable),
        };
        guard.visit(provider).map_err(|fault| match fault {
            ChainFault::Cycle => DependencyOutcome::Cycle,
            ChainFault::DepthExceeded => DependencyOutcome::Unsupported,
        })?;
        if graph.game(provider).unsupported_structure {
            return Err(DependencyOutcome::Unsupported);
        }
        hop(OwnerHop::Provider(provider))?;
        let declaration = match graph.rom_declared_as(provider, wanted) {
            MemberRef::Unique((_, declaration)) => declaration,
            MemberRef::Duplicate => return Err(DependencyOutcome::Ambiguous),
            MemberRef::Absent => return Err(DependencyOutcome::Contradictory),
        };
        if borrowers.iter().any(|earlier| {
            checksums_conflict(earlier, declaration)
                || matches!(
                    (earlier.size_bytes, declaration.size_bytes),
                    (Some(a), Some(b)) if a != b
                )
        }) {
            return Err(DependencyOutcome::Contradictory);
        }
        if malformed_sha1(borrower) || malformed_sha1(declaration) {
            return Err(DependencyOutcome::Contradictory);
        }
        // A NO_DUMP borrow of a NO_DUMP declaration has no content to compare
        // (MAME publishes no checksums for either); it routes by name alone.
        if !(is_nodump(borrower) && is_nodump(declaration))
            && !content_proven_same(borrower, declaration)
        {
            return Err(DependencyOutcome::EvidenceUnavailable);
        }
        hop(OwnerHop::Declaration {
            provider,
            declaration,
        })?;
        match super::graph::declared_name(&declaration.merge) {
            DeclaredName::Absent => {
                return Ok(RomOwner::Borrowed {
                    owner: provider,
                    declaration,
                });
            }
            DeclaredName::Malformed => return Err(DependencyOutcome::Contradictory),
            DeclaredName::Named(next) => {
                wanted = next;
                current = provider;
                borrower = declaration;
                borrowers.push(declaration);
            }
        }
    }
}

/// The borrow and the declaration it names agree on content the catalogue
/// can actually compare: at least one shared checksum algorithm, and SHA-1
/// whenever either side declares it. A CRC-only agreement never stands in
/// for a SHA-1 one side published.
fn content_proven_same(borrower: &DatRomEntry, declaration: &DatRomEntry) -> bool {
    let shared = borrower
        .checksums()
        .iter()
        .any(|checksum| declaration.checksums().contains(checksum));
    shared && has_sha1(borrower) == has_sha1(declaration)
}

fn has_sha1(rom: &DatRomEntry) -> bool {
    rom.checksums()
        .iter()
        .any(|checksum| checksum.algorithm == ChecksumAlgorithm::Sha1)
}

/// A SHA-1 field that is present but does not parse: never silently treated
/// as absent, which would let a CRC alone stand in for it.
fn malformed_sha1(rom: &DatRomEntry) -> bool {
    rom.sha1
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && !has_sha1(rom)
}

fn is_nodump(rom: &DatRomEntry) -> bool {
    rom.status
        .as_deref()
        .is_some_and(|status| status.trim().eq_ignore_ascii_case("nodump"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::DatGameEntry;

    const SHA_P: &str = "1111111111111111111111111111111111111111";
    const SHA_B: &str = "2222222222222222222222222222222222222222";
    const SHA_X: &str = "3333333333333333333333333333333333333333";

    fn rom(name: &str, sha1: Option<&str>, crc: Option<&str>, merge: Option<&str>) -> DatRomEntry {
        DatRomEntry {
            name: name.into(),
            size_bytes: Some(4),
            sha1: sha1.map(str::to_owned),
            crc32: crc.map(str::to_owned),
            merge: merge.map(str::to_owned),
            ..Default::default()
        }
    }

    fn set(
        name: &str,
        clone_of: Option<&str>,
        rom_of: Option<&str>,
        roms: Vec<DatRomEntry>,
    ) -> DatGameEntry {
        DatGameEntry {
            name: name.into(),
            clone_of: clone_of.map(str::to_owned),
            rom_of: rom_of.map(str::to_owned),
            roms,
            ..Default::default()
        }
    }

    /// Owner of the first ROM of the last set, as (set name, member name).
    fn owner_of(games: &[DatGameEntry]) -> Result<Option<(String, String)>, DependencyOutcome> {
        let graph = DependencyGraph::build(games);
        let index = games.len() - 1;
        rom_owner(&graph, index, &games[index].roms[0]).map(|owner| match owner {
            RomOwner::Own => None,
            RomOwner::Borrowed { owner, declaration } => {
                Some((graph.game(owner).name.clone(), declaration.name.clone()))
            }
        })
    }

    #[test]
    fn explicit_romof_wins_over_cloneof() {
        // The clone's ancestry is "parent" but it borrows from "source".
        let games = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("p.rom", Some(SHA_X), None, None)],
            ),
            set(
                "source",
                None,
                None,
                vec![rom("p.rom", Some(SHA_P), None, None)],
            ),
            set(
                "clone",
                Some("parent"),
                Some("source"),
                vec![rom("p.rom", Some(SHA_P), None, Some("p.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&games),
            Ok(Some(("source".into(), "p.rom".into())))
        );
    }

    #[test]
    fn present_but_unresolved_romof_never_falls_back_to_cloneof() {
        let parent = set(
            "parent",
            None,
            None,
            vec![rom("p.rom", Some(SHA_P), None, None)],
        );
        let borrow = vec![rom("p.rom", Some(SHA_P), None, Some("p.rom"))];
        let absent = vec![
            parent.clone(),
            set("clone", Some("parent"), Some("gone"), borrow.clone()),
        ];
        assert_eq!(
            owner_of(&absent),
            Err(DependencyOutcome::EvidenceUnavailable)
        );
        let empty = vec![parent, set("clone", Some("parent"), Some("  "), borrow)];
        assert_eq!(owner_of(&empty), Err(DependencyOutcome::Contradictory));
    }

    #[test]
    fn absent_romof_falls_back_to_cloneof_only_for_a_verified_merge() {
        let games = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("p.rom", Some(SHA_P), None, None)],
            ),
            set(
                "clone",
                Some("parent"),
                None,
                vec![rom("p.rom", Some(SHA_P), None, Some("p.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&games),
            Ok(Some(("parent".into(), "p.rom".into())))
        );
    }

    #[test]
    fn wrong_parent_content_is_contradictory() {
        let games = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("p.rom", Some(SHA_X), None, None)],
            ),
            set(
                "clone",
                Some("parent"),
                None,
                vec![rom("p.rom", Some(SHA_P), None, Some("p.rom"))],
            ),
        ];
        assert_eq!(owner_of(&games), Err(DependencyOutcome::Contradictory));
    }

    #[test]
    fn crc_only_or_hashless_agreement_is_not_verification() {
        // The provider publishes SHA-1; the borrow publishes only a CRC.
        let crc_only = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("p.rom", Some(SHA_P), Some("12345678"), None)],
            ),
            set(
                "clone",
                Some("parent"),
                Some("parent"),
                vec![rom("p.rom", None, Some("12345678"), Some("p.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&crc_only),
            Err(DependencyOutcome::EvidenceUnavailable)
        );
        let hashless = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("p.rom", Some(SHA_P), None, None)],
            ),
            set(
                "clone",
                Some("parent"),
                Some("parent"),
                vec![rom("p.rom", None, None, Some("p.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&hashless),
            Err(DependencyOutcome::EvidenceUnavailable)
        );
    }

    #[test]
    fn ambiguous_parent_or_member_is_refused() {
        let parent = set(
            "parent",
            None,
            None,
            vec![rom("p.rom", Some(SHA_P), None, None)],
        );
        let clone = set(
            "clone",
            Some("parent"),
            None,
            vec![rom("p.rom", Some(SHA_P), None, Some("p.rom"))],
        );
        let duplicated_set = vec![parent.clone(), parent.clone(), clone.clone()];
        assert_eq!(owner_of(&duplicated_set), Err(DependencyOutcome::Ambiguous));
        let mut twice = parent;
        twice.roms.push(rom("p.rom", Some(SHA_P), None, None));
        assert_eq!(owner_of(&[twice, clone]), Err(DependencyOutcome::Ambiguous));
    }

    #[test]
    fn clone_only_ancestry_never_moves_a_member() {
        // The parent declares a same-named file; without `merge=` the clone
        // owns its own declaration.
        let games = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("c.rom", Some(SHA_P), None, None)],
            ),
            set(
                "clone",
                Some("parent"),
                Some("parent"),
                vec![rom("c.rom", Some(SHA_P), None, None)],
            ),
        ];
        assert_eq!(owner_of(&games), Ok(None));
    }

    #[test]
    fn a_borrow_chain_ends_at_the_bios_that_owns_it() {
        let mut bios = set(
            "bios",
            None,
            None,
            vec![rom("sys.rom", Some(SHA_B), None, None)],
        );
        bios.is_bios = Some("yes".into());
        let games = vec![
            bios,
            set(
                "parent",
                None,
                Some("bios"),
                vec![rom("sys.rom", Some(SHA_B), None, Some("sys.rom"))],
            ),
            set(
                "clone",
                Some("parent"),
                Some("parent"),
                vec![rom("sys.rom", Some(SHA_B), None, Some("sys.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&games),
            Ok(Some(("bios".into(), "sys.rom".into())))
        );
    }

    #[test]
    fn cycles_self_references_and_providerless_merges_are_refused() {
        let cycle = vec![
            set(
                "a",
                None,
                Some("b"),
                vec![rom("x.rom", Some(SHA_P), None, Some("x.rom"))],
            ),
            set(
                "b",
                None,
                Some("a"),
                vec![rom("x.rom", Some(SHA_P), None, Some("x.rom"))],
            ),
        ];
        assert_eq!(owner_of(&cycle), Err(DependencyOutcome::Cycle));
        let own = vec![set(
            "a",
            None,
            Some("a"),
            vec![rom("x.rom", Some(SHA_P), None, Some("x.rom"))],
        )];
        assert_eq!(owner_of(&own), Err(DependencyOutcome::Contradictory));
        let nowhere = vec![set(
            "a",
            None,
            None,
            vec![rom("x.rom", Some(SHA_P), None, Some("x.rom"))],
        )];
        assert_eq!(owner_of(&nowhere), Err(DependencyOutcome::Contradictory));
        let missing_member = vec![
            set("p", None, None, vec![rom("y.rom", Some(SHA_P), None, None)]),
            set(
                "c",
                Some("p"),
                Some("p"),
                vec![rom("x.rom", Some(SHA_P), None, Some("x.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&missing_member),
            Err(DependencyOutcome::Contradictory)
        );
    }

    #[test]
    fn a_nodump_borrow_routes_by_name_and_never_becomes_unverified_content() {
        let nodump = |merge: Option<&str>| DatRomEntry {
            status: Some("nodump".into()),
            ..rom("gap.rom", None, None, merge)
        };
        let games = vec![
            set("parent", None, None, vec![nodump(None)]),
            set(
                "clone",
                Some("parent"),
                Some("parent"),
                vec![nodump(Some("gap.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&games),
            Ok(Some(("parent".into(), "gap.rom".into())))
        );
        // A NO_DUMP borrow naming a dumped declaration is still refused.
        let dumped = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("gap.rom", Some(SHA_P), None, None)],
            ),
            set(
                "clone",
                Some("parent"),
                Some("parent"),
                vec![nodump(Some("gap.rom"))],
            ),
        ];
        assert_eq!(
            owner_of(&dumped),
            Err(DependencyOutcome::EvidenceUnavailable)
        );
    }

    #[test]
    fn a_malformed_sha1_never_lets_a_crc_stand_in() {
        let games = vec![
            set(
                "parent",
                None,
                None,
                vec![rom("p.rom", Some(SHA_P), Some("12345678"), None)],
            ),
            set(
                "clone",
                Some("parent"),
                Some("parent"),
                vec![rom(
                    "p.rom",
                    Some("not-a-sha1"),
                    Some("12345678"),
                    Some("p.rom"),
                )],
            ),
        ];
        assert_eq!(owner_of(&games), Err(DependencyOutcome::Contradictory));
    }

    #[test]
    fn a_declaration_must_agree_with_every_earlier_borrow() {
        // child and middle agree on SHA-1; middle has no CRC; source's CRC
        // contradicts child's two hops back.
        let games = vec![
            set(
                "source",
                None,
                None,
                vec![rom("s.rom", Some(SHA_P), Some("bbbbbbbb"), None)],
            ),
            set(
                "middle",
                None,
                Some("source"),
                vec![rom("m.rom", Some(SHA_P), None, Some("s.rom"))],
            ),
            set(
                "child",
                None,
                Some("middle"),
                vec![rom("c.rom", Some(SHA_P), Some("aaaaaaaa"), Some("m.rom"))],
            ),
        ];
        assert_eq!(owner_of(&games), Err(DependencyOutcome::Contradictory));
    }
}
