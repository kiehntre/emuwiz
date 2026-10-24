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
//! separate question the hash cannot answer: TOSEC lists every disk of a
//! multi-disk game as its own one-file entry, such as
//! `Game (1990)(Disk 1 of 3)`, so one file per entry proves nothing.
//! Standalone completeness ([`StandaloneEvidence`]) is accepted only on one
//! of two catalogue grounds, and the matched entry must always list exactly
//! one ROM, an `.adf`, with balanced brackets and no disk number outside them:
//!
//! 1. **Explicit:** the title says `(Disk 1 of 1)` and carries no other disk,
//!    disc, side, tape, volume, part, floppy or `N of M` group, and the same
//!    revision lists no numbered sibling of the text before that group.
//! 2. **TOSEC single-medium convention:** the TOSEC Naming Convention adds a
//!    media flag such as `(Disk 1 of 2)` whenever a release spans more than
//!    one medium, so an unmarked entry denotes a single medium. EmuWiz relies
//!    on that only when the matched catalogue is a TOSEC Commodore Amiga
//!    *Games* `[ADF]` catalogue whose complete, digest-bound entry list was
//!    saved (never a partial import), the entry has no set group at all, and
//!    no other entry of that list sharing its title root (the text before the
//!    first group, without a trailing version) carries any set marker. An
//!    alternate release of the same title with numbered disks therefore keeps
//!    every release of it blocked. The catalogue file must also carry
//!    TOSEC's own `(TOSEC-v<version>` name, a derived or filtered DAT that
//!    kept the header but dropped siblings being the main residual risk.
//!
//! On either ground, a disk TOSEC marks as not a game on its own is refused:
//! bad dumps (`[b …]`), hard-disk installs (`[HD …]`), disks that need
//! something else (`[req. …]`), non-interactive demos and add-ons (update,
//! installer, editor, scenario, levels, tracks, season, expansion or
//! construction disks, `[addon]`, `[datadisk]`).
//!
//! Anything else is `CompletenessUnknown` or `DiskNumbered`, and Play stays
//! blocked. Filenames never contribute.

/// Which catalogue convention, if any, the matched entry was published under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueConvention {
    None,
    /// A complete TOSEC Commodore Amiga Games `[ADF]` catalogue.
    TosecAmigaAdfGames,
}

/// What established that the verified disk is a complete game on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandaloneEvidence {
    /// The entry title says `(Disk 1 of 1)`.
    ExplicitDisk1Of1,
    /// An unmarked entry in a complete TOSEC Amiga Games `[ADF]` catalogue
    /// with no multi-disk release of the same title.
    TosecSingleMedium,
}

impl StandaloneEvidence {
    pub fn describe(self) -> &'static str {
        match self {
            Self::ExplicitDisk1Of1 => "the DAT entry is marked Disk 1 of 1",
            Self::TosecSingleMedium => {
                "the TOSEC catalogue lists this release as a single disk, with no multi-disk release of the same title"
            }
        }
    }
}

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
    /// Kept apart from the hash: why the disk counts as a whole game.
    pub standalone: StandaloneEvidence,
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
    /// The catalogue marks this disk as something other than a game that
    /// runs on its own (an add-on, bad dump, install or demo).
    NotStandaloneGame(String),
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
                "The DAT verifies this disk but does not establish that it is the whole game, so EmuWiz cannot rule out other disks.".into()
            }
            Self::DiskNumbered(group) => format!(
                "The DAT names this ADF as part of a set ({group}); multi-disk games cannot be started yet."
            ),
            Self::NotStandaloneGame(what) => format!(
                "The DAT marks this disk as {what}, not a game that runs on its own."
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

/// The convention a matched catalogue was published under. Only one TOSEC
/// catalogue (never a merged folder of several) whose header names a
/// Commodore Amiga *Games* `[ADF]` set qualifies: applications, save disks,
/// collections and other formats do not.
pub fn catalogue_convention(
    ecosystems: [Option<crate::dat::model::DatEcosystem>; 2],
    catalogue_names: &[String],
    dat_path: &str,
    revision: &str,
) -> CatalogueConvention {
    let tosec = ecosystems
        .iter()
        .all(|ecosystem| *ecosystem == Some(crate::dat::model::DatEcosystem::Tosec));
    let adf_games = match catalogue_names {
        [name] => {
            let lower = normalise(name);
            lower.starts_with("commodore amiga - games -") && lower.contains("[adf]")
        }
        _ => false,
    };
    // TOSEC publishes `<name> (TOSEC-v<version>_CM).dat`; a file that does
    // not carry its own revision in that form is not taken as the release.
    let file_name = std::path::Path::new(dat_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let published =
        !revision.trim().is_empty() && file_name.contains(&format!("(TOSEC-v{}", revision.trim()));
    if tosec && adf_games && published {
        CatalogueConvention::TosecAmigaAdfGames
    } else {
        CatalogueConvention::None
    }
}

/// The catalogue title root: text before the first group, without a
/// trailing version word such as `v1.2`, normalised.
fn title_root(name: &str) -> String {
    let head = name.split(['(', '[']).next().unwrap_or(name);
    let mut words: Vec<&str> = head.split_whitespace().collect();
    while words.last().is_some_and(|word| {
        let lower = word.to_ascii_lowercase();
        let numbered = |prefix: &str| {
            lower
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        };
        lower.len() > 1 && (numbered("v") || numbered("r") || lower.starts_with("rev"))
    }) {
        words.pop();
    }
    normalise(&words.join(" "))
}

/// Title words that name an add-on or data disk rather than a game.
const ADDON_WORDS: [&str; 14] = [
    "disk",
    "update",
    "installer",
    "editor",
    "edit",
    "addon",
    "addons",
    "scenario",
    "scenarios",
    "levels",
    "tracks",
    "season",
    "expansion",
    "construction",
];

/// Whether `lower` is a TOSEC flag `<letter>[digits]`, alone or followed
/// by a non-letter, e.g. `b`, `b2 dump`, `m baddump`.
fn tosec_flag(lower: &str, letter: char) -> bool {
    lower.strip_prefix(letter).is_some_and(|rest| {
        !rest
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .starts_with(|c: char| c.is_ascii_alphabetic())
    })
}

/// What TOSEC marks this disk as when it is not a game on its own.
fn not_standalone(name: &str) -> Option<String> {
    for group in name
        .split(['(', '['])
        .skip(1)
        .filter_map(|part| part.split([')', ']']).next())
    {
        let lower = group.trim().to_ascii_lowercase();
        let bad_modification = tosec_flag(&lower, 'm')
            && ["baddump", "bad dump", "corrupt", "missing"]
                .iter()
                .any(|word| lower.contains(word));
        if tosec_flag(&lower, 'b') || bad_modification {
            return Some(format!("a bad dump ({})", group.trim()));
        }
        if lower.starts_with("req") {
            return Some(format!("needing something else ({})", group.trim()));
        }
        let install = lower.split(',').map(str::trim).any(|token| {
            token == "ppc"
                || token
                    .strip_prefix("hd")
                    .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric()))
        });
        if install {
            return Some(format!("a hard-disk install ({})", group.trim()));
        }
        if lower == "demo-rolling" || lower == "demo-slideshow" {
            return Some(format!("a non-interactive demo ({})", group.trim()));
        }
        if ["addon", "add-on", "datadisk", "construction kit"]
            .iter()
            .any(|word| lower.contains(word))
        {
            return Some(format!("an add-on ({})", group.trim()));
        }
    }
    let head = name
        .split(['(', '['])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if head.contains("add-on") {
        return Some("an add-on".into());
    }
    head.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|word| ADDON_WORDS.contains(word))
        .map(|word| format!("an add-on or data disk ({word})"))
}

fn has_set_marker(name: &str) -> bool {
    !set_groups(name).is_empty() || numbered_outside(name).is_some()
}

/// Applies the catalogue-only single-disk rules to the matched entry.
pub fn prove_single_disk<'a>(
    entry_name: &str,
    rom_count: usize,
    rom_name: Option<&str>,
    catalogue_names: impl IntoIterator<Item = &'a str>,
    convention: CatalogueConvention,
) -> Result<StandaloneEvidence, AmigaAdfIdentityRefusal> {
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
    if let Some(what) = not_standalone(entry_name) {
        return Err(AmigaAdfIdentityRefusal::NotStandaloneGame(what));
    }
    if groups.is_empty() {
        if convention != CatalogueConvention::TosecAmigaAdfGames {
            // No indicator and no convention: completeness unknown.
            return Err(AmigaAdfIdentityRefusal::CompletenessUnknown);
        }
        let root = title_root(entry_name);
        if root.is_empty() {
            return Err(AmigaAdfIdentityRefusal::CompletenessUnknown);
        }
        for name in catalogue_names {
            if name != entry_name && title_root(name) == root && has_set_marker(name) {
                return Err(AmigaAdfIdentityRefusal::NumberedSiblings(name.to_string()));
            }
        }
        return Ok(StandaloneEvidence::TosecSingleMedium);
    }
    if groups.len() != 1 {
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
    Ok(StandaloneEvidence::ExplicitDisk1Of1)
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
            CatalogueConvention::None,
        )
        .map(|_| ())
    }

    fn tosec(name: &str, catalogue: &[&str]) -> Result<StandaloneEvidence, R> {
        prove_single_disk(
            name,
            1,
            Some(&format!("{name}.adf")),
            catalogue.iter().copied(),
            CatalogueConvention::TosecAmigaAdfGames,
        )
    }

    /// Entry names copied verbatim from the real TOSEC "Commodore Amiga -
    /// Games - [ADF] (TOSEC-v2025-01-30_CM)" catalogue.
    const REAL_SINGLE: &str = "1000cc Turbo (1990)(Impressions)[cr CSL]";
    const REAL_SINGLE_VARIANT: &str =
        "1000cc Turbo (1990)(Impressions)[cr CSL][m doscopy][a bootblock]";
    const REAL_SET: [&str; 2] = [
        "A-10 Tank Killer v1.0 (1990)(Dynamix)(Disk 1 of 2)",
        "A-10 Tank Killer v1.0 (1990)(Dynamix)(Disk 2 of 2)",
    ];
    /// Real: an unmarked crack of a release TOSEC also lists as two disks.
    const REAL_CRACK: &str = "'Nam 1965-1975 v1.0 (1991-03-18)(Domark)(M4)[cr QTX][h BTL]";
    const REAL_CRACK_SET_DISK: &str = "'Nam 1965-1975 v1.0 (1991-03-18)(Domark)(M4)(Disk 2 of 2)";
    const REAL_VOLUME: &str =
        "1000 Miglia - 1927-1933 Volume 1 (1992)(Simulmondo)(en-it)[cr CSL][b crack]";

    fn real_catalogue() -> Vec<&'static str> {
        vec![
            REAL_SINGLE,
            REAL_SINGLE_VARIANT,
            REAL_SET[0],
            REAL_SET[1],
            REAL_CRACK,
            REAL_CRACK_SET_DISK,
            REAL_VOLUME,
        ]
    }

    #[test]
    fn an_unmarked_entry_in_a_complete_tosec_amiga_adf_catalogue_is_single_medium() {
        let catalogue = real_catalogue();
        for name in [REAL_SINGLE, REAL_SINGLE_VARIANT] {
            assert_eq!(
                tosec(name, &catalogue),
                Ok(StandaloneEvidence::TosecSingleMedium),
                "{name}"
            );
            // Without the TOSEC convention the same entry stays unknown.
            assert_eq!(prove(name, &catalogue), Err(R::CompletenessUnknown));
        }
    }

    #[test]
    fn the_tosec_convention_never_admits_a_marked_disk_or_a_title_with_a_multi_disk_release() {
        let catalogue = real_catalogue();
        for name in REAL_SET {
            assert!(
                matches!(tosec(name, &catalogue), Err(R::DiskNumbered(_))),
                "{name}"
            );
        }
        // Real TOSEC lists 'Nam both as a two-disk set and as an unmarked
        // crack: the unmarked entry is refused, not taken as single-disk.
        assert_eq!(
            tosec(REAL_CRACK, &catalogue),
            Err(R::NumberedSiblings(REAL_CRACK_SET_DISK.into()))
        );
        // A different version of a numbered title is the same title root.
        let other_version = "A-10 Tank Killer v1.2 (1991)(Dynamix)[cr Skid Row]";
        assert!(matches!(
            tosec(other_version, &[other_version, REAL_SET[0]]),
            Err(R::NumberedSiblings(_))
        ));
        assert!(matches!(
            tosec(REAL_VOLUME, &catalogue),
            Err(R::DiskNumbered(_))
        ));
        assert_eq!(tosec("Game (1990", &[]), Err(R::CompletenessUnknown));
    }

    /// Unmarked entries copied verbatim from the real TOSEC Amiga Games and
    /// Public Domain [ADF] catalogues that are not games on their own.
    #[test]
    fn add_ons_bad_dumps_installs_and_rolling_demos_are_not_standalone_games() {
        for name in [
            "Beneath a Steel Sky - Update Disk (1994)(Virgin)(FW)[h Divine]",
            "Speris Legacy, The - Installer Disk (1996)(Team 17)",
            "Harpoon Scenario Editor v1.0 (1992)(PSS)[construction kit]",
            "Stunt Car Racer - The New Tracks (2004-06-17)(MicroStyle)(alpha)[p CARE]",
            "Ultimate Soccer Manager Update v1.179 (1995-10-13)(Impressions)(AGA)[English league]",
            "SkidMarks - Racer Disk Issue 1 (1995-08)(Acid)[h SR][addons]",
            "DFX Puzzle - Extra Levels (1997)(Cvijanovic, Goran - F1 Software)(AGA)(PD)[datadisk]",
            "Lotus Esprit Turbo Challenge (1990)(Gremlin)[cr Genesis - Angels][b corrupt file]",
            "Aliex (1990)(Trojan)[cr LGD][t +4 LGD][req. Phazer light gun]",
            "Arcade Pool (1994)(Team 17)(OCS-AGA)[h HD Access][HD]",
            "Armour-Geddon II - Codename Hellfire (1994)(Psygnosis)[HD, lha archive]",
            "A320 Airbus (demo-rolling) (1991)(Thalion)",
            "Addams Family, The (1992)(Ocean)(M3)[cr FSN][b2 dump]",
            "1943 - The Battle of Midway (1989)(GO!)[cr PNA][m baddump]",
            "Nightlong - Union City Conspiracy v1.0 (2000-10-16)(PXL)[h Faith][PPC, HD][update]",
        ] {
            assert!(
                matches!(tosec(name, &[name]), Err(R::NotStandaloneGame(_))),
                "{name}: {:?}",
                tosec(name, &[name])
            );
        }
        // A real single-disk game with a hack flag stays eligible.
        let game = "Stable Masters II v3.4 (1994)(ESP)[h PDX]";
        assert_eq!(
            tosec(game, &[game]),
            Ok(StandaloneEvidence::TosecSingleMedium)
        );
    }

    /// Real PD entries: unmarked "Dread rNNNN" builds beside a two-disk build.
    #[test]
    fn revision_suffixes_share_the_title_root_of_a_numbered_release() {
        let set = "Dread r1C23 (2021-12-24)(Kluczek, Krzysztof)(Disk 1 of 2)";
        let build = "Dread r1905 (2021-09-05)(Kluczek, Krzysztof)";
        assert_eq!(
            tosec(build, &[build, set]),
            Err(R::NumberedSiblings(set.into()))
        );
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
            tosec("Speedball (1988)(Image Works)(Disk 1 of 1)", &[]),
            Ok(StandaloneEvidence::ExplicitDisk1Of1)
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
            prove_single_disk(name, 2, Some("Game.adf"), [], CatalogueConvention::None),
            Err(R::NotSingleRom(2))
        );
        assert_eq!(
            prove_single_disk(name, 1, Some("Game.dms"), [], CatalogueConvention::None),
            Err(R::NotSingleRom(1))
        );
        assert_eq!(
            prove_single_disk(name, 1, None, [], CatalogueConvention::None),
            Err(R::NotSingleRom(1))
        );
    }

    #[test]
    fn only_a_single_tosec_amiga_games_adf_catalogue_carries_the_convention() {
        use crate::dat::model::DatEcosystem::{NoIntro, Tosec};
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let games = names(&["Commodore Amiga - Games - [ADF]"]);
        let pd = names(&["Commodore Amiga - Games - Public Domain - [ADF]"]);
        // Real published file names of those two catalogues.
        let games_file = "/dats/Commodore Amiga - Games - [ADF] (TOSEC-v2025-01-30_CM).dat";
        let pd_file =
            "/dats/Commodore Amiga - Games - Public Domain - [ADF] (TOSEC-v2025-01-28_CM).dat";
        assert_eq!(
            catalogue_convention([Some(Tosec), Some(Tosec)], &games, games_file, "2025-01-30"),
            CatalogueConvention::TosecAmigaAdfGames
        );
        assert_eq!(
            catalogue_convention([Some(Tosec), Some(Tosec)], &pd, pd_file, "2025-01-28"),
            CatalogueConvention::TosecAmigaAdfGames
        );
        // A renamed, derived or other-revision file is not the release.
        for (file, revision) in [
            ("/dats/fixdat_amiga.dat", "2025-01-30"),
            (games_file, "2025-02-01"),
            (games_file, ""),
        ] {
            assert_eq!(
                catalogue_convention([Some(Tosec), Some(Tosec)], &games, file, revision),
                CatalogueConvention::None,
                "{file} {revision}"
            );
        }
        for (ecosystems, list) in [
            ([Some(NoIntro), Some(Tosec)], games.clone()),
            ([Some(Tosec), None], games.clone()),
            (
                [Some(Tosec), Some(Tosec)],
                names(&["Commodore Amiga - Games - Save Disks"]),
            ),
            (
                [Some(Tosec), Some(Tosec)],
                names(&["Commodore Amiga - Applications - [ADF]"]),
            ),
            (
                [Some(Tosec), Some(Tosec)],
                names(&["Commodore Amiga - Games - SPS"]),
            ),
            (
                [Some(Tosec), Some(Tosec)],
                [games.clone(), pd.clone()].concat(),
            ),
        ] {
            assert_eq!(
                catalogue_convention(ecosystems, &list, games_file, "2025-01-30"),
                CatalogueConvention::None,
                "{list:?}"
            );
        }
    }

    #[test]
    fn game_key_is_the_normalised_hash() {
        assert_eq!(adf_game_key("SHA-1", " ABCDEF "), "adf-sha1:abcdef");
    }
}
