//! Which system a DAT catalogue describes, read from the catalogue's own
//! header name.
//!
//! This answers "is this file an ordinary game list for one system, and
//! which one?" so a person does not have to assign every catalogue by hand.
//! It is deliberately narrow:
//!
//! - Only the header `<name>` (and the parser's ecosystem) are used. A file
//!   name is never evidence here: a renamed or misleadingly named file is
//!   recognised from what it says inside, or not at all.
//! - Systems are matched exactly against the reviewed platform alias registry
//!   ([`crate::canonical_platform_for_alias`]), the reviewed TOSEC system table
//!   and the short reviewed table below. Nothing is trimmed word by word or
//!   matched by similarity.
//! - Anything that is not an ordinary game list (downloadable content, source
//!   code, scans, development material, encrypted variants, an unfamiliar
//!   label) is reported as such and is never treated as the ordinary list.
//! - Recognition is a suggestion. Nothing here writes settings, reads game
//!   files, or makes any game Verified: verification still needs an exact
//!   content match against the catalogue.

use crate::dat::model::DatEcosystem;
use crate::dat::tosec_release_pack::{TosecFriendlyCategory, classify_tosec_catalogue_name};
use crate::dat::tosec_system_projection::{PlatformForTosecSystem, platform_for_tosec_system};
use crate::platform::{display_name_for, equivalent_platform_ids};

/// What a catalogue's header says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueRecognition {
    /// The ordinary game list for one system. The only kind that may be
    /// offered as a safe match.
    Games { platform: &'static str },
    /// The right system, but not its ordinary game list (for example
    /// downloadable content or e-Reader cards). Only used if a person
    /// chooses it.
    Extras {
        platform: &'static str,
        what: String,
    },
    /// The header names more than one different system.
    Ambiguous { platforms: Vec<&'static str> },
    /// One catalogue for many systems (MAME, FinalBurn Neo, software lists,
    /// PC-based arcade boards).
    SeveralSystems,
    /// Not a list of games at all (source code, scans, soundtracks).
    NotGames { what: &'static str },
    /// The header names a system EmuWiz does not know.
    NotRecognised { name: String },
    /// The header has no name.
    NoName,
}

impl CatalogueRecognition {
    /// The system of an ordinary game list; `None` for everything else.
    pub fn safe_platform(&self) -> Option<&'static str> {
        match self {
            Self::Games { platform } => Some(platform),
            _ => None,
        }
    }

    /// The system the catalogue belongs to, ordinary list or extras.
    pub fn platform(&self) -> Option<&'static str> {
        match self {
            Self::Games { platform } | Self::Extras { platform, .. } => Some(platform),
            _ => None,
        }
    }

    /// Whether this recognition belongs to `platform` (equivalent ids, such
    /// as `PC Engine` and `TurboGrafx-16`, count as the same system).
    pub fn is_for(&self, platform: &str) -> bool {
        self.platform()
            .is_some_and(|own| same_platform(own, platform))
    }

    /// One plain-English sentence for a person.
    pub fn describe(&self) -> String {
        match self {
            Self::Games { platform } => format!("{} games", display_name_for(platform)),
            Self::Extras { platform, what } => {
                format!("{} extras: {what}", display_name_for(platform))
            }
            Self::Ambiguous { platforms } => format!(
                "Could be {}",
                platforms
                    .iter()
                    .map(|p| display_name_for(p))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
            Self::SeveralSystems => "Covers several systems".to_string(),
            Self::NotGames { what } => format!("Not a game list ({what})"),
            Self::NotRecognised { name } => {
                format!("EmuWiz does not support this system yet ({name})")
            }
            Self::NoName => "The catalogue does not say which system it is for".to_string(),
        }
    }
}

/// Whether two canonical ids name the same system.
pub fn same_platform(left: &str, right: &str) -> bool {
    left == right || equivalent_platform_ids(left).contains(&right)
}

/// Names that are not game lists. Matched exactly, at the start of the name.
const NOT_GAMES: &[(&str, &str)] = &[
    ("Source Code - ", "source code"),
    ("Unofficial - Video Game OSTs", "soundtracks"),
    ("Unofficial - Video Game Magazine Scans", "magazine scans"),
    ("Unofficial - Video Game Manual Scans", "manual scans"),
    ("Unofficial - Video Game Scans", "scans"),
    ("Unofficial - Video Game Documents", "documents"),
    ("Unofficial - Super Mario Maker Courses", "level files"),
    ("Nintendo - Wallpapers", "wallpapers"),
    ("Nintendo - amiibo", "figure data"),
    ("Non-Redump - Audio CD", "audio discs"),
    ("Non-Redump - BD-Video", "video discs"),
    ("Non-Redump - DVD-Video", "video discs"),
    ("CD-ROM", "general discs"),
    ("DVD-ROM", "general discs"),
    ("MP3 CD", "audio discs"),
];

/// Catalogues for many systems, by exact name.
const SEVERAL_SYSTEMS: &[&str] = &[
    "Arcade - PC-based",
    "Project EGG",
    "Nintendo - Misc",
    "Unofficial - Obscure Gamers",
];

/// Reviewed No-Intro/Redump system names the alias registry does not spell.
/// Exact matches only. An entry with several platforms is a genuine
/// ambiguity (for example, a PC catalogue can hold DOS or Windows games).
const REVIEWED_SYSTEM_NAMES: &[(&str, &[&str])] = &[
    ("Apple - II", &["Apple II"]),
    ("Apple - IIe", &["Apple II"]),
    ("Apple - II Plus", &["Apple II"]),
    ("Apple - IIGS", &["Apple II"]),
    ("Atari - 8-bit Family", &["Atari 8-bit"]),
    ("Nintendo - New Nintendo 3DS", &["Nintendo 3DS"]),
    ("Sinclair - ZX Spectrum +3", &["ZX Spectrum"]),
    ("NEC - PC-88 series", &["NEC PC-8801"]),
    ("IBM - PC and Compatibles", &["PC", "DOS"]),
    ("IBM - PC Compatible", &["PC", "DOS"]),
];

/// Add-on systems the platform registry deliberately folds into a parent
/// platform. Their catalogues belong to the parent but are never its
/// ordinary game list.
const ADD_ON_SYSTEMS: &[(&str, &str, &str)] = &[
    (
        "Nintendo - Family Computer Disk System",
        "NES",
        "Famicom Disk System disks",
    ),
    ("Nintendo - Nintendo DSi", "Nintendo DS", "DSi software"),
    (
        "NEC - PC Engine SuperGrafx",
        "PC Engine",
        "SuperGrafx games",
    ),
];

/// Parenthesised labels that only say how the same games were dumped or
/// stored. A catalogue whose labels are all in this list is still the
/// ordinary game list. Every other label makes it extras.
const FORMAT_LABELS: &[&str] = &[
    "Headered",
    "Headerless",
    "BigEndian",
    "ByteSwapped",
    "A78",
    "BIN",
    "ROM",
    "JAG",
    "J64",
    "ABS",
    "COF",
    "LNX",
    "LYX",
    "BLL",
    "Flux",
    "KryoFlux",
    "Kryoflux",
    "Bitstream",
    "Bitstreams",
    "FluxDumps",
    "A2R",
    "WOZ",
    "DC42",
    "HDM",
    "Greaseweazle",
    "IPF",
    "SCP",
    "Tapes",
    "PP",
    "QD",
    "FDS",
    "HardDisk",
    "Waveform",
    "Discs",
];

/// Plain-English names for labels that mark extras.
const EXTRA_LABELS: &[(&[&str], &str)] = &[
    (
        &["DLC", "Updates and DLC", "Split DLC"],
        "downloadable content",
    ),
    (&["Updates", "Title Updates"], "updates"),
    (&["Themes"], "themes"),
    (&["Avatars"], "avatars"),
    (&["Video", "UMD Video", "BD-Video Extras"], "video"),
    (&["UMD Music"], "music"),
    (&["Memory Card", "Visual Memory Unit"], "memory card data"),
    (
        &["Development Kit Hard Drives", "Dev", "Dev ROMs", "Lotcheck"],
        "development material",
    ),
    (&["e-Reader"], "e-Reader cards"),
    (&["Multiboot"], "multiboot programs"),
    (&["Play-Yan"], "Play-Yan media"),
    (&["Download Play"], "Download Play programs"),
    (
        &[
            "Digital",
            "CDN",
            "PSN",
            "Pre-Install",
            "SpotPass",
            "Content",
        ],
        "digital downloads",
    ),
    (&["Encrypted"], "encrypted copies"),
    (&["Decrypted"], "decrypted copies"),
    (&["Private"], "private dumps"),
    (&["Deprecated"], "older dumps"),
    (&["Misc", "Uncategorized"], "mixed content"),
];

/// Trailing ` - ` segments of a No-Intro/Redump name that mark extras.
const EXTRA_SEGMENTS: &[(&str, &str)] = &[("BIOS Images", "BIOS files"), ("BIOS", "BIOS files")];

/// Recognises a catalogue from its header name and parsed ecosystem.
pub fn recognise_catalogue(
    header_name: Option<&str>,
    ecosystem: Option<DatEcosystem>,
) -> CatalogueRecognition {
    if matches!(
        ecosystem,
        Some(DatEcosystem::MAMEArcade | DatEcosystem::MAMESoftwareList | DatEcosystem::FBNeo)
    ) {
        return CatalogueRecognition::SeveralSystems;
    }
    let Some(name) = header_name.map(str::trim).filter(|name| !name.is_empty()) else {
        return CatalogueRecognition::NoName;
    };
    if let Some((_, what)) = NOT_GAMES
        .iter()
        .find(|(prefix, _)| starts_with_name(name, prefix))
    {
        return CatalogueRecognition::NotGames { what };
    }
    if SEVERAL_SYSTEMS
        .iter()
        .any(|exact| starts_with_name(name, exact))
    {
        return CatalogueRecognition::SeveralSystems;
    }
    if let Some(tosec) = recognise_tosec(name) {
        return tosec;
    }
    recognise_no_intro_style(name)
}

/// `name` is `prefix`, or `prefix` followed by a label or another segment.
/// A prefix ending in a separator matches anything after it.
fn starts_with_name(name: &str, prefix: &str) -> bool {
    if prefix.ends_with(' ') {
        return name.starts_with(prefix);
    }
    name == prefix
        || name
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(" (") || rest.starts_with(" - "))
}

/// `System - Category[ - [Media]...]`, the TOSEC convention. Only used when a
/// TOSEC category word is present, so No-Intro names never take this path.
fn recognise_tosec(name: &str) -> Option<CatalogueRecognition> {
    let classification = classify_tosec_catalogue_name(name);
    if classification.category == TosecFriendlyCategory::EverythingElse || !classification.confident
    {
        return None;
    }
    let platform = match tosec_system_platform(&classification.system) {
        Ok(platform) => platform,
        Err(other) => return Some(other),
    };
    let what: Option<String> = match classification.category {
        TosecFriendlyCategory::Games => {
            // Anything after "Games" other than media tags in brackets
            // ("Games - [ADF]") is a different collection.
            let segments: Vec<&str> = name.split(" - ").map(str::trim).collect();
            let after: &[&str] = segments
                .iter()
                .rposition(|segment| matches!(*segment, "Games" | "Game"))
                .map_or(&[], |index| &segments[index + 1..]);
            let extra = after
                .iter()
                .find(|segment| !(segment.starts_with('[') && segment.ends_with(']')));
            match (classification.qualifier.as_deref(), extra) {
                (Some(_), _) => Some("compilations".to_string()),
                (None, Some(extra)) => Some(format!("\u{201c}{extra}\u{201d}")),
                (None, None) => None,
            }
        }
        TosecFriendlyCategory::ManualsCoversPrintedMedia => {
            return Some(CatalogueRecognition::NotGames {
                what: "manuals and printed media",
            });
        }
        TosecFriendlyCategory::MusicAudio => {
            return Some(CatalogueRecognition::NotGames { what: "music" });
        }
        TosecFriendlyCategory::DemosScene => Some("demos".to_string()),
        TosecFriendlyCategory::EducationalProductivity => {
            Some("applications and educational software".to_string())
        }
        TosecFriendlyCategory::FirmwareSystemSoftware => {
            Some("firmware and system software".to_string())
        }
        TosecFriendlyCategory::PreservationVerificationData => {
            Some("preservation data".to_string())
        }
        TosecFriendlyCategory::EverythingElse => return None,
    };
    Some(match what {
        None => CatalogueRecognition::Games { platform },
        Some(what) => CatalogueRecognition::Extras { platform, what },
    })
}

/// The platform of a TOSEC system name such as `Sega Mark III & Master System`.
fn tosec_system_platform(system: &str) -> Result<&'static str, CatalogueRecognition> {
    if let PlatformForTosecSystem::Platform(platform) = platform_for_tosec_system(system) {
        return Ok(platform);
    }
    if let Some((_, platform)) = REVIEWED_TOSEC_SYSTEMS
        .iter()
        .find(|(exact, _)| *exact == system)
    {
        return Ok(platform);
    }
    let mut candidates = Vec::new();
    if collect(&mut candidates, system) {
        return decide(candidates, system);
    }
    let mut unknown = false;
    for part in split_names(system) {
        let known = collect(&mut candidates, part)
            | strip_maker(part).is_some_and(|rest| collect(&mut candidates, rest));
        unknown |= !known;
    }
    match decide(candidates, system) {
        // A part naming no system could narrow what the catalogue holds, so
        // it is never dropped to leave an ordinary list.
        Ok(platform) if unknown => Err(CatalogueRecognition::Extras {
            platform,
            what: format!("\u{201c}{system}\u{201d}"),
        }),
        other => other,
    }
}

/// TOSEC names lead with the maker ("Nintendo Game Boy"). Only these exact
/// maker words are removed, once, and only to retry an exact alias match.
const TOSEC_MAKERS: &[&str] = &[
    "Nintendo ",
    "Sega ",
    "Atari ",
    "Bandai ",
    "SNK ",
    "Sinclair ",
    "Commodore ",
    "Acorn ",
    "Amstrad ",
    "NEC ",
    "Sony ",
    "Coleco ",
    "Mattel ",
    "Philips ",
];

fn strip_maker(part: &str) -> Option<&str> {
    TOSEC_MAKERS
        .iter()
        .find_map(|maker| part.strip_prefix(maker))
        .filter(|rest| !rest.is_empty())
}

/// `[Non-Redump - |Unofficial - ]Maker - System[ - Other name][ (label)...]`.
fn recognise_no_intro_style(name: &str) -> CatalogueRecognition {
    let mut extras: Vec<String> = Vec::new();
    let mut rest = name;
    if let Some(stripped) = rest.strip_prefix("Non-Redump - ") {
        rest = stripped;
    }
    if let Some(stripped) = rest.strip_prefix("Unofficial - ") {
        rest = stripped;
        extras.push("unofficial dumps".to_string());
    }
    let (system, labels) = split_labels(rest);
    for label in &labels {
        if FORMAT_LABELS.contains(label) {
            continue;
        }
        let plain = EXTRA_LABELS
            .iter()
            .find(|(names, _)| names.contains(label))
            .map(|(_, plain)| (*plain).to_string())
            .unwrap_or_else(|| format!("\u{201c}{label}\u{201d}"));
        if !extras.contains(&plain) {
            extras.push(plain);
        }
    }
    let mut segments: Vec<&str> = system.split(" - ").map(str::trim).collect();
    if let Some(last) = segments.last()
        && let Some((_, plain)) = EXTRA_SEGMENTS.iter().find(|(segment, _)| segment == last)
        && segments.len() > 1
    {
        segments.pop();
        extras.push((*plain).to_string());
    }
    let system = segments.join(" - ");
    if let Some((_, platform, what)) = ADD_ON_SYSTEMS.iter().find(|(name, ..)| *name == system) {
        extras.insert(0, (*what).to_string());
        return CatalogueRecognition::Extras {
            platform,
            what: extras.join(", "),
        };
    }
    let platform = match no_intro_system_platform(&system) {
        Ok((platform, unknown)) => {
            // A part of the name that names no system is never ignored: the
            // catalogue may be a subset (for example "- Digital"), so it is
            // treated as extras rather than as the ordinary game list.
            for segment in unknown {
                let plain = format!("\u{201c}{segment}\u{201d}");
                if !extras.contains(&plain) {
                    extras.push(plain);
                }
            }
            platform
        }
        Err(other) => return other,
    };
    if extras.is_empty() {
        CatalogueRecognition::Games { platform }
    } else {
        CatalogueRecognition::Extras {
            platform,
            what: extras.join(", "),
        }
    }
}

/// Splits trailing `(label)` groups off a name, keeping their order.
fn split_labels(name: &str) -> (&str, Vec<&str>) {
    let mut labels = Vec::new();
    let mut rest = name.trim_end();
    while rest.ends_with(')') {
        let Some(open) = rest.rfind(" (") else { break };
        labels.push(&rest[open + 2..rest.len() - 1]);
        rest = rest[..open].trim_end();
    }
    labels.reverse();
    (rest, labels)
}

fn no_intro_system_platform(
    system: &str,
) -> Result<(&'static str, Vec<String>), CatalogueRecognition> {
    if let Some((_, platforms)) = REVIEWED_SYSTEM_NAMES
        .iter()
        .find(|(exact, _)| *exact == system)
    {
        return decide(platforms.to_vec(), system).map(|platform| (platform, Vec::new()));
    }
    let mut candidates = Vec::new();
    let mut segments = system.split(" - ");
    let maker = segments.next().unwrap_or_default();
    let names: Vec<&str> = segments.collect();
    // The whole name spelled as one alias needs nothing else checked.
    let mut whole = collect(&mut candidates, system);
    let mut unknown = Vec::new();
    if !names.is_empty() {
        // A leading word that is not a reviewed maker (for example "Hacks"
        // or "Homebrew") may describe a subset, so it is never ignored.
        let maker_known = is_known_maker(maker);
        if !maker_known && !whole {
            unknown.push(maker.to_string());
        }
        // Everything after the maker spelled as one alias (for example
        // "Sega - Master System - Mark III").
        let rest = collect(&mut candidates, &names.join(" - "));
        whole |= rest && maker_known;
    }
    for segment in names {
        for part in split_names(segment) {
            let known = collect(&mut candidates, part)
                | collect(&mut candidates, &format!("{maker} {part}"))
                | is_known_maker(part);
            if !known && !whole {
                unknown.push(part.to_string());
            }
        }
    }
    decide(candidates, system).map(|platform| (platform, unknown))
}

/// TOSEC system names whose second part the alias registry does not spell,
/// reviewed by hand: each names one machine.
const REVIEWED_TOSEC_SYSTEMS: &[(&str, &str)] = &[
    ("Nintendo Famicom & Entertainment System", "NES"),
    ("Atari 2600 & VCS", "Atari2600"),
];

/// Reviewed makers that lead a No-Intro / Redump style name. Any other
/// leading word is treated as a qualifier, not skipped.
const NO_INTRO_MAKERS: &[&str] = &[
    "Acorn",
    "Amstrad",
    "APF",
    "Apple",
    "Atari",
    "Bally",
    "Bandai",
    "Benesse",
    "Bit Corporation",
    "Casio",
    "Coleco",
    "Commodore",
    "Emerson",
    "Entex",
    "Epoch",
    "Fairchild",
    "Fujitsu",
    "Funtech",
    "GamePark",
    "GCE",
    "Hartung",
    "IBM",
    "Interton",
    "iQue",
    "Konami",
    "LeapFrog",
    "Magnavox",
    "Mattel",
    "Microsoft",
    "Mobile",
    "NEC",
    "Nichibutsu",
    "Nintendo",
    "Nokia",
    "Ouya",
    "Panasonic",
    "Philips",
    "Pioneer",
    "RCA",
    "Sanyo",
    "Sega",
    "Seta",
    "Sharp",
    "Sinclair",
    "SNK",
    "Sony",
    "Tandy",
    "Tangerine",
    "TeleNova",
    "Texas Instruments",
    "Thomson",
    "Tiger",
    "Tomy",
    "Toshiba",
    "VM Labs",
    "VTech",
    "Watara",
    "Welback",
    "Yamaha",
    "ZAPiT Games",
    "Zeebo",
];

fn is_known_maker(word: &str) -> bool {
    NO_INTRO_MAKERS.contains(&word.trim())
}

/// Splits `A + B`, `A & B` and `A / B` into separate names.
fn split_names(text: &str) -> Vec<&str> {
    let mut parts = vec![text];
    for separator in [" + ", " & ", " / "] {
        parts = parts
            .into_iter()
            .flat_map(|part| part.split(separator))
            .collect();
    }
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

/// Adds the system `text` names exactly, if any; true when it names one.
fn collect(candidates: &mut Vec<&'static str>, text: &str) -> bool {
    let Some(platform) = crate::canonical_platform_for_alias(text.trim()) else {
        return false;
    };
    if !candidates.contains(&platform) {
        candidates.push(platform);
    }
    true
}

/// One system (after folding equivalents), several, or none.
fn decide(candidates: Vec<&'static str>, name: &str) -> Result<&'static str, CatalogueRecognition> {
    let mut distinct: Vec<&'static str> = Vec::new();
    for candidate in candidates {
        if !distinct.iter().any(|seen| same_platform(seen, candidate)) {
            distinct.push(candidate);
        }
    }
    match distinct.as_slice() {
        [] => Err(CatalogueRecognition::NotRecognised {
            name: name.to_string(),
        }),
        [one] => Ok(one),
        _ => {
            distinct.sort_unstable();
            Err(CatalogueRecognition::Ambiguous {
                platforms: distinct,
            })
        }
    }
}

#[cfg(test)]
#[path = "catalogue_recognition/tests.rs"]
mod tests;
