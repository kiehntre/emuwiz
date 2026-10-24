//! Launch identity for one loose Amiga ADF, proven by a trusted DAT.
//!
//! An ADF has no header that names its game, so its only launch identity is
//! the catalogue entry its own bytes matched exactly. This module holds the
//! pure rules: what that identity is, and when the matched entry proves a
//! complete single-disk release. The database side
//! (`Database::amiga_adf_launch_identity`) supplies only current, saved,
//! content-hash evidence; filenames never contribute.
//!
//! Two facts are kept apart. The exact whole-file hash verifies *which
//! disk* this is. Whether that disk is a *complete game on its own* is a
//! separate question the hash cannot answer: TOSEC, for example, lists every
//! disk of a multi-disk game as its own one-file entry, such as
//! `Game (1990)(Disk 1 of 3)`. So one file per entry proves nothing, and an
//! entry with no disk indicator is not proof either. Standalone completeness
//! is accepted only when the catalogue states it:
//! - the matched entry lists exactly one ROM, and that ROM is an `.adf`;
//! - the entry title carries an explicit `(Disk 1 of 1)`, and no other disk,
//!   disc, side, tape, volume, part or `N of M` group;
//! - the same catalogue revision lists no other entry that is this title
//!   followed by such a group (for example `Game (1990)(Disk 2 of 2)`).
//!
//! An entry with no indicator is `CompletenessUnknown`; any other indicator
//! is `DiskNumbered`. Both keep Play blocked. Filenames never contribute.

/// A verified, single-disk ADF launch identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaAdfLaunchIdentity {
    /// Opaque launch key: the matched content hash, e.g. `adf-sha1:<hex>`.
    pub game_key: String,
    pub source_id: String,
    pub source_revision: String,
    pub catalogue_sha256: String,
    /// The catalogue entry name the bytes matched.
    pub entry_name: String,
    pub matched_algorithm: String,
    pub matched_value: String,
}

/// Why an ADF has no launch identity. Every variant fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmigaAdfIdentityRefusal {
    NotInLibrary,
    NotAdf,
    NotAmiga,
    /// The file changed, moved or disappeared since it was scanned or audited.
    FileChanged,
    /// No current exact SHA-256/SHA-1/MD5 match from a configured catalogue.
    NoCurrentMatch,
    /// Current content evidence names more than one entry, or is ambiguous.
    Conflicting(Vec<String>),
    /// The catalogue that matched it is no longer configured, or its file is
    /// not the one that was audited.
    CatalogueStale,
    /// No complete validated entry list for that catalogue revision.
    InventoryMissing,
    /// The saved entry list belongs to other catalogue bytes or another version.
    InventoryStale,
    EntryNotInInventory,
    /// The matched entry lists this many ROMs, not exactly one ADF.
    NotSingleRom(usize),
    /// The matched entry's title numbers a disk, side, part or role.
    DiskNumbered(String),
    /// The catalogue does not state that this disk is a complete game.
    CompletenessUnknown,
    /// The catalogue also lists numbered disks of this title.
    NumberedSiblings(String),
}

impl AmigaAdfIdentityRefusal {
    /// Plain words for the person looking at the game.
    pub fn reason(&self) -> String {
        match self {
            Self::NotInLibrary => "This file is not in the library catalogue; scan it first.".into(),
            Self::NotAdf => "Only ADF floppy images can be started this way.".into(),
            Self::NotAmiga => "This game is not stored as an ordinary Amiga game.".into(),
            Self::FileChanged => {
                "The ADF changed since it was checked; scan and verify it again.".into()
            }
            Self::NoCurrentMatch => {
                "No configured DAT verified this ADF by an exact hash. Verify it against an Amiga DAT first.".into()
            }
            Self::Conflicting(names) => format!(
                "The DAT evidence for this ADF disagrees ({}); EmuWiz will not choose.",
                names.join(", ")
            ),
            Self::CatalogueStale => {
                "The DAT that verified this ADF is no longer configured or has changed; verify again.".into()
            }
            Self::InventoryMissing => {
                "The DAT that verified this ADF has no complete validated entry list; validate the DAT.".into()
            }
            Self::InventoryStale => {
                "The DAT's saved entry list is from another version; validate the DAT again.".into()
            }
            Self::EntryNotInInventory => {
                "The matched DAT entry is not in the DAT's saved entry list; validate the DAT again.".into()
            }
            Self::NotSingleRom(count) => format!(
                "The matched DAT entry lists {count} files, not one ADF, so EmuWiz cannot prove a single-disk game."
            ),
            Self::CompletenessUnknown => {
                "The DAT verifies this disk but does not say it is the whole game (it is not marked Disk 1 of 1), so EmuWiz cannot rule out other disks.".into()
            }
            Self::DiskNumbered(group) => format!(
                "The DAT names this ADF as part of a set ({group}); multi-disk games cannot be started yet."
            ),
            Self::NumberedSiblings(name) => format!(
                "The DAT also lists numbered disks of this title (for example {name}), so EmuWiz cannot prove it is a single-disk game."
            ),
        }
    }
}

const SET_WORDS: [&str; 14] = [
    "disk", "disc", "side", "tape", "volume", "vol", "part", "pt", "cd", "floppy", "diskette",
    "seite", "teil", "boot",
];

/// Lowercase, single spaces, and no spaces beside brackets, so
/// `Game (1990) (Pub)` and `Game (1990)(Pub)` compare equal.
fn normalise(text: &str) -> String {
    let joined = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let chars: Vec<char> = joined.chars().collect();
    let bracket = |c: Option<&char>| c.is_some_and(|c| matches!(c, '(' | ')' | '[' | ']'));
    chars
        .iter()
        .enumerate()
        .filter(|&(i, &c)| {
            !(c == ' ' && (bracket(chars.get(i + 1)) || (i > 0 && bracket(chars.get(i - 1)))))
        })
        .map(|(_, &c)| c)
        .collect()
}

/// Whether every `(` and `[` closes in order. A truncated title is unknown.
fn balanced(title: &str) -> bool {
    let mut open = Vec::new();
    for c in title.chars() {
        match c {
            '(' | '[' => open.push(c),
            ')' if open.pop() != Some('(') => return false,
            ']' if open.pop() != Some('[') => return false,
            _ => {}
        }
    }
    open.is_empty()
}

/// Text outside brackets that numbers a disk, e.g. `Game Disk 2`.
fn numbered_outside(title: &str) -> Option<String> {
    let mut depth = 0usize;
    let outside: String = title
        .chars()
        .map(|c| match c {
            '(' | '[' => {
                depth += 1;
                ' '
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                ' '
            }
            _ if depth > 0 => ' ',
            _ => c,
        })
        .collect();
    let lower = outside.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.windows(2).find_map(|w| {
        let numbered = w[1].chars().all(|c| c.is_ascii_digit())
            || (w[1].len() == 1 && matches!(w[1], "a" | "b" | "c" | "d"));
        (SET_WORDS.contains(&w[0]) && numbered).then(|| format!("{} {}", w[0], w[1]))
    })
}

/// `N of M` with or without a unit word, e.g. `(1 of 3)`.
fn numbered_of(lower: &str) -> bool {
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.windows(3).any(|w| {
        w[1] == "of"
            && w[0].chars().all(|c| c.is_ascii_digit())
            && w[2].chars().all(|c| c.is_ascii_digit())
    })
}

fn set_groups(title: &str) -> Vec<String> {
    let mut groups = Vec::new();
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let after = &rest[open + 1..];
        let Some(close) = after.find([')', ']']) else {
            break;
        };
        let group = after[..close].trim();
        let lower = group.to_ascii_lowercase();
        if numbered_of(&lower)
            || lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| SET_WORDS.contains(&word))
        {
            groups.push(group.to_string());
        }
        rest = &after[close + 1..];
    }
    groups
}

fn explicit_single(group: &str) -> bool {
    group
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .eq(["disk", "1", "of", "1"].map(String::from))
}

/// Applies the catalogue-only single-disk rules to the matched entry.
pub fn prove_single_disk<'a>(
    entry_name: &str,
    rom_count: usize,
    rom_name: Option<&str>,
    catalogue_names: impl IntoIterator<Item = &'a str>,
) -> Result<(), AmigaAdfIdentityRefusal> {
    let adf_rom = rom_name.is_some_and(|name| name.to_ascii_lowercase().ends_with(".adf"));
    if rom_count != 1 || !adf_rom {
        return Err(AmigaAdfIdentityRefusal::NotSingleRom(rom_count));
    }
    if !balanced(entry_name) {
        return Err(AmigaAdfIdentityRefusal::CompletenessUnknown);
    }
    if let Some(words) = numbered_outside(entry_name) {
        return Err(AmigaAdfIdentityRefusal::DiskNumbered(words));
    }
    let groups = set_groups(entry_name);
    if let Some(group) = groups.iter().find(|group| !explicit_single(group)) {
        return Err(AmigaAdfIdentityRefusal::DiskNumbered(group.clone()));
    }
    if groups.len() != 1 {
        // No indicator at all: a hash-verified disk, completeness unknown.
        return Err(AmigaAdfIdentityRefusal::CompletenessUnknown);
    }
    // The release name a numbered sibling would share is the title before
    // the explicit group; flags such as `[cr TRSI]` may follow either.
    let single = &groups[0];
    let Some(at) = [format!("({single})"), format!("[{single}]")]
        .iter()
        .find_map(|token| entry_name.find(token.as_str()))
    else {
        // A padded or unusual group cannot anchor the sibling search.
        return Err(AmigaAdfIdentityRefusal::CompletenessUnknown);
    };
    let base = normalise(&entry_name[..at]);
    for name in catalogue_names {
        if name == entry_name {
            continue;
        }
        let candidate = normalise(name);
        let Some(rest) = candidate.strip_prefix(base.as_str()) else {
            continue;
        };
        let rest = rest.trim_start();
        let numbered = !set_groups(rest).is_empty() || numbered_outside(rest).is_some();
        if rest.starts_with(['(', '[']) && numbered {
            return Err(AmigaAdfIdentityRefusal::NumberedSiblings(name.to_string()));
        }
    }
    Ok(())
}

/// The opaque launch key for a matched hash, e.g. `adf-sha1:<lowercase hex>`.
pub fn adf_game_key(algorithm: &str, value: &str) -> String {
    let algorithm: String = algorithm
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("adf-{algorithm}:{}", value.trim().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use AmigaAdfIdentityRefusal as R;

    fn prove(name: &str, siblings: &[&str]) -> Result<(), R> {
        prove_single_disk(
            name,
            1,
            Some(&format!("{name}.adf")),
            siblings.iter().copied(),
        )
    }

    #[test]
    fn only_an_explicit_disk_1_of_1_entry_proves_a_standalone_game() {
        assert_eq!(
            prove("Speedball (1988)(Image Works)(Disk 1 of 1)", &[]),
            Ok(())
        );
        // Unrelated titles that merely share a prefix do not count.
        assert_eq!(
            prove(
                "Speedball (1988)(Image Works)(Disk 1 of 1)",
                &["Speedball 2 (1990)(Image Works)(Disk 1 of 2)"]
            ),
            Ok(())
        );
    }

    #[test]
    fn a_one_file_entry_without_an_indicator_is_unknown_not_single_disk() {
        assert_eq!(
            prove("Speedball (1988)(Image Works)", &[]),
            Err(R::CompletenessUnknown)
        );
        assert_eq!(
            prove("Speedball (1988)(Image Works)[a]", &[]),
            Err(R::CompletenessUnknown)
        );
    }

    #[test]
    fn each_disk_of_a_tosec_multi_disk_set_is_refused_although_each_entry_has_one_file() {
        let set = [
            "Lotus Turbo (1990)(Gremlin)(Disk 1 of 3)",
            "Lotus Turbo (1990)(Gremlin)(Disk 2 of 3)",
            "Lotus Turbo (1990)(Gremlin)(Disk 3 of 3)",
        ];
        for name in set {
            assert_eq!(
                prove(name, &set),
                Err(R::DiskNumbered(
                    name.rsplit_once('(')
                        .unwrap()
                        .1
                        .trim_end_matches(')')
                        .into()
                )),
                "{name}"
            );
        }
    }

    #[test]
    fn unknown_ambiguous_or_role_disk_indicators_are_refused() {
        for name in [
            "Game (1990)(Disk 2)",
            "Game (1990)(Disk A)",
            "Game (1990)(Side B)",
            "Game (1990)(Save Disk)",
            "Game (1990)[Data Disk]",
            "Game (1990)(Disc 1)",
            "Game (1990)(Volume 2)",
            "Game (1990)(Part 1)",
            "Game (1990)(1 of 2)",
            "Game (1990)(Disk 1 of 1)(Disk 2 of 2)",
        ] {
            assert!(
                matches!(prove(name, &[]), Err(R::DiskNumbered(_))),
                "{name}"
            );
        }
    }

    #[test]
    fn a_disk_1_of_1_entry_with_numbered_catalogue_siblings_is_refused() {
        assert_eq!(
            prove(
                "Game (1990)(Pub)(Disk 1 of 1)",
                &["Game (1990)(Pub)(Disk 2 of 2)"]
            ),
            Err(R::NumberedSiblings("Game (1990)(Pub)(Disk 2 of 2)".into()))
        );
    }

    #[test]
    fn numbered_siblings_are_found_around_flags_case_and_group_order() {
        for (entry, sibling) in [
            (
                "Game (1990)(Pub)(Disk 1 of 1)[cr TRSI]",
                "Game (1990)(Pub)(Disk 2 of 2)[cr TRSI]",
            ),
            (
                "Game (1990)(Pub)(Disk 1 of 1)[cr TRSI]",
                "Game (1990)(Pub)(Disk 2 of 2)",
            ),
            (
                "Game (Disk 1 of 1)(1990)(Pub)",
                "Game (Disk 2 of 2)(1990)(Pub)",
            ),
            (
                "Game (1990)(Pub)(Disk 1 of 1)",
                "game  (1990)(pub)(disk 2 of 2)",
            ),
            (
                "Game (1990)(Pub)(Disk 1 of 1)",
                "Game (1990) (Pub) (Disk 2 of 2)",
            ),
        ] {
            assert_eq!(
                prove(entry, &[sibling]),
                Err(R::NumberedSiblings(sibling.into())),
                "{entry} / {sibling}"
            );
        }
    }

    #[test]
    fn truncated_titles_and_numbering_outside_brackets_are_refused() {
        assert_eq!(
            prove("Game (1990)(Disk 1 of 1)(Disk 2 of 3", &[]),
            Err(R::CompletenessUnknown)
        );
        assert!(matches!(
            prove("Game Disk 2 (1990)(Disk 1 of 1)", &[]),
            Err(R::DiskNumbered(_))
        ));
        assert!(matches!(
            prove("Game (1990)(Floppy 2)(Disk 1 of 1)", &[]),
            Err(R::DiskNumbered(_))
        ));
        assert_eq!(
            prove("Game (1990)( Disk 1 of 1 )", &[]),
            Err(R::CompletenessUnknown)
        );
        // A title word that is not numbering stays allowed.
        assert_eq!(prove("Side Arms (1988)(Capcom)(Disk 1 of 1)", &[]), Ok(()));
    }

    #[test]
    fn entries_with_several_or_non_adf_roms_are_refused() {
        let name = "Game (Disk 1 of 1)";
        assert_eq!(
            prove_single_disk(name, 2, Some("Game.adf"), []),
            Err(R::NotSingleRom(2))
        );
        assert_eq!(
            prove_single_disk(name, 1, Some("Game.dms"), []),
            Err(R::NotSingleRom(1))
        );
        assert_eq!(
            prove_single_disk(name, 1, None, []),
            Err(R::NotSingleRom(1))
        );
    }

    #[test]
    fn game_key_is_the_normalised_hash() {
        assert_eq!(adf_game_key("SHA-1", " ABCDEF "), "adf-sha1:abcdef");
    }
}
