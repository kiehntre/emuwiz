//! Why a catalogue checks none (or only some) of a system's games, and which
//! catalogue would cover the rest. Advice only: nothing here installs,
//! downloads or verifies anything, and a recommendation never claims that a
//! file will match. Wording follows measured coverage (Check Games
//! investigation 2026-11-26, Stage D): formats where no byte-level evidence
//! exists say so instead of naming a catalogue.

use std::collections::BTreeMap;
use std::path::Path;

use super::library_verification::{NOT_IN_CATALOGUE_FORMATS, VerificationPreview};

/// The games of one file format that this catalogue cannot check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatGap {
    /// Lower-case extension without the dot ("" for none).
    pub extension: String,
    pub games: usize,
    /// One plain sentence: what would check them, or why nothing can yet.
    pub advice: String,
}

/// The selected games this catalogue leaves unchecked only because of their
/// format, grouped by extension, largest group first.
pub fn format_gaps(preview: &VerificationPreview) -> Vec<FormatGap> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for exclusion in &preview.exclusions {
        if exclusion.reason == NOT_IN_CATALOGUE_FORMATS {
            let extension = Path::new(&exclusion.path)
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            *counts.entry(extension).or_default() += 1;
        }
    }
    let mut gaps: Vec<FormatGap> = counts
        .into_iter()
        .map(|(extension, games)| FormatGap {
            advice: advice_for(&preview.platform, &extension),
            extension,
            games,
        })
        .collect();
    gaps.sort_by(|a, b| b.games.cmp(&a.games).then(a.extension.cmp(&b.extension)));
    gaps
}

/// Plain explanation of a preview in which nothing can be checked because
/// the catalogue lists none of the files' formats. Any other reason (a
/// missing or unreadable file, an ambiguous path) gets no such advice: the
/// exclusion rows say why, and "nothing is wrong" would be untrue.
pub fn nothing_eligible_explanation(preview: &VerificationPreview) -> Option<String> {
    if preview.selected_games == 0 || !preview.lists_none_of_these_formats() {
        return None;
    }
    let listed = if preview.formats.is_empty() {
        "no file formats EmuWiz can compare".to_owned()
    } else {
        preview
            .formats
            .iter()
            .map(|f| format!(".{f}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Some(format!(
        "This catalogue lists {listed}. None of the {} selected {} games is in one of those formats, so it cannot check any of them. Nothing is wrong with the files; another catalogue is needed.",
        preview.selected_games, preview.platform
    ))
}

/// What would check `extension` files on `platform`.
pub fn advice_for(platform: &str, extension: &str) -> String {
    let id = crate::canonical_platform_for_alias(platform).unwrap_or(platform);
    let known: Option<&str> = match (id, extension) {
        ("Amstrad CPC", "dsk") => Some(
            "A TOSEC \"Amstrad CPC - Games - [DSK]\" catalogue lists disk images in this format.",
        ),
        ("Amiga", "lha") => Some(
            "The libretro WHDLoad catalogue lists WHDLoad .lha installs. Older versions of an install may not be listed.",
        ),
        ("Amiga", "adf") => {
            Some("A TOSEC \"Commodore Amiga - Games - [ADF]\" catalogue lists floppy images.")
        }
        ("Amiga", "hdf") => Some(
            "No public catalogue lists Amiga hard-disk images (.hdf), so these cannot be verified by catalogue.",
        ),
        ("NES", "fds") => Some(
            "A No-Intro Famicom Disk System catalogue lists .fds files, but library copies are often stored differently from its entries. EmuWiz cannot yet verify .fds files reliably.",
        ),
        ("Apple II", "dsk" | "do" | "po" | "woz") => {
            Some("A TOSEC Apple II disk catalogue lists images in this format.")
        }
        ("Apple II", "2mg") => Some("A TOSEC \"Apple IIGS - [2MG]\" catalogue lists .2mg images."),
        ("MSX2", "dsk") => Some("A TOSEC \"MSX2 - Games - [DSK]\" catalogue lists disk images."),
        ("AtariST", "stx") => {
            Some("A TOSEC \"Atari ST - Games - [STX]\" catalogue lists Pasti (.stx) images.")
        }
        ("AtariST", "st" | "msa") => {
            Some("A TOSEC Atari ST catalogue for this format lists floppy images.")
        }
        _ => None,
    };
    if let Some(known) = known {
        return known.to_owned();
    }
    match extension {
        "chd" | "rvz" | "cso" | "wbfs" | "gcz" => format!(
            "Catalogues list the uncompressed disc, not .{extension} files. EmuWiz cannot yet verify .{extension} against a catalogue; adding a disc catalogue alone will not change that."
        ),
        "cia" => "A .cia package wraps the game. EmuWiz cannot yet check the game inside it against a catalogue.".to_owned(),
        "n64" => "Catalogues list the standard (.z64) byte order. EmuWiz cannot yet check byte-swapped .n64 files.".to_owned(),
        "" => "Files without an extension cannot be matched to a catalogue format.".to_owned(),
        _ => format!(
            "The catalogue checked does not list .{extension} files for {platform}. Add a trusted catalogue for this format in DAT Management."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommends_measured_catalogues_and_never_claims_unproven_coverage() {
        assert!(advice_for("Amstrad CPC", "dsk").contains("[DSK]"));
        assert!(advice_for("Amiga", "lha").contains("WHDLoad"));
        assert!(advice_for("Amiga", "hdf").contains("cannot be verified"));
        assert!(advice_for("NES", "fds").contains("cannot yet verify"));
        assert!(advice_for("Atari ST", "stx").contains("[STX]"));
        assert!(advice_for("PlayStation", "chd").contains("alone will not change that"));
        assert!(advice_for("Game Boy Color", "cia").contains("cannot yet"));
        assert!(advice_for("Some System", "xyz").contains("DAT Management"));
    }
}
