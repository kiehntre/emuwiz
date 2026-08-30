//! Single source of truth for which file extensions EmuWiz recognises as
//! library media, and how each is persisted as an [`ArchiveKind`].
//!
//! Before this module existed, "which extensions does EmuWiz support" was
//! answered independently in at least two places - [`crate::archive_kind`]
//! (directory scanning) and the filesystem watcher's own extension list -
//! and the two had already drifted apart (the watcher never learned about
//! `.gcz`/`.rvz`/`.wbfs`/`.ciso`). This module is now the only place that
//! answer is written down; scanning, rescanning, and watching all consult
//! it, so a new format is added in one place and every caller sees it.
//!
//! # What this is not
//!
//! This registry never assigns a platform. It answers "does EmuWiz
//! recognise this extension as media, and if so how is it persisted" -
//! nothing about *which game system* a file belongs to. Extension-based
//! platform evidence (strong/weak extensions per platform, e.g. `.d64`
//! being weak evidence for both Commodore 64 and Commodore 128) stays
//! entirely in [`crate::platform::PLATFORMS`]. A file can be recognised
//! here and still end up with no platform, or an ambiguous one - that is
//! the platform registry's decision to make, not this one's.
//!
//! It also never decides grouping (multi-disc `.cue`/`.bin` pairs,
//! `.m3u` playlists) - every entry here is a single independent file.
//!
//! # Scope: this is v1
//!
//! This is deliberately the v1 centralized media-extension/[`ArchiveKind`]
//! compatibility registry, and nothing more. It intentionally does not yet
//! model a richer `MediaKind`, a `StandalonePolicy`, or a `CatalogPolicy` -
//! those concepts (multi-file disc images, grouping/merge rules, per-format
//! cataloguing behaviour beyond "recognised or not") are planned for a
//! later descriptor/playlist/grouping slice, not this one. In particular,
//! CUE/BIN pairing, M3U playlists, and GDI multi-track sets are not
//! implemented here - every [`MediaFormat`] entry is still exactly one
//! independent file mapped to one [`ArchiveKind`], and that stays true
//! until that later slice deliberately changes it.

use crate::ArchiveKind;

/// One file extension EmuWiz recognises as library media on its own,
/// without needing corroboration from folder, source-root, or header
/// evidence, and how it is persisted for backward compatibility.
#[derive(Debug, Clone, Copy)]
pub struct MediaFormat {
    /// Lowercase, no leading dot.
    pub extension: &'static str,
    /// The persisted [`ArchiveKind`] this extension maps to. Multiple
    /// extensions may share a kind (every direct-image format persists as
    /// `ArchiveKind::DirectGameImage`) - `ArchiveKind` is a small,
    /// backward-compatible projection of a much larger set of recognised
    /// formats, never a one-to-one encoding of them.
    pub kind: ArchiveKind,
}

/// The whole media registry. Adding a new self-evidencing format is a
/// one-line addition here; nothing else needs to change for it to be
/// discovered by scanning, rescanning, and watching alike.
pub const MEDIA_FORMATS: &[MediaFormat] = &[
    MediaFormat {
        extension: "zip",
        kind: ArchiveKind::Zip,
    },
    MediaFormat {
        extension: "7z",
        kind: ArchiveKind::SevenZip,
    },
    MediaFormat {
        extension: "rar",
        kind: ArchiveKind::Rar,
    },
    // `.smd` (Super Magic Drive dump) is Mega Drive specific and needs no
    // corroboration, unlike `.md`/`.bin`/`.gen` - see
    // `crate::archive_kind_in_root` for the extensions that still require
    // folder/source/header corroboration before they resolve at all.
    MediaFormat {
        extension: "smd",
        kind: ArchiveKind::MegaDriveRom,
    },
    MediaFormat {
        extension: "iso",
        kind: ArchiveKind::DirectGameImage,
    },
    // PS3 digital packages are recognised as media, but platform identity is
    // granted only by the bounded PKG header observer.
    MediaFormat {
        extension: "pkg",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "pbp",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "gcm",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "gcz",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "rvz",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "wbfs",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "ciso",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "xiso",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "xbe",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "xex",
        kind: ArchiveKind::DirectGameImage,
    },
    // Loose Commodore floppy-disk images. Neither is a container format
    // EmuWiz can mount or unwrap - catalogued directly, like every other
    // `DirectGameImage` entry. Which Commodore platform a given file
    // belongs to is entirely the platform registry's decision (`.d64`/
    // `.g64` are shared, weak evidence there) - this registry only
    // recognises the file as media at all.
    MediaFormat {
        extension: "d64",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "g64",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "d71",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "d81",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "crt",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "ssd",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "dsd",
        kind: ArchiveKind::DirectGameImage,
    },
    // Computer disks and cassette images are recognised media families, but
    // their extensions are deliberately not platform authority. A `.dsk`,
    // `.img`, `.tap`, or `.cdt` remains platform-unresolved until evidence
    // elsewhere proves it.
    MediaFormat {
        extension: "dsk",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "adf",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "ima",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "img",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "hdi",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "nhd",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "cdt",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "tap",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "tzx",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "d88",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "xdf",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "dim",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Atari ---
    MediaFormat {
        extension: "a26",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "a52",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "a78",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "atr",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "atx",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "xfd",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "lnx",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "lyx",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "j64",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "jag",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "stx",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "st",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "msa",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "ipf",
        kind: ArchiveKind::DirectGameImage,
    },
    // Apple II disk images. The platform registry supplies the Apple II
    // identity; this registry only records the media projection.
    MediaFormat {
        extension: "do",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "po",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "woz",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "2mg",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "nib",
        kind: ArchiveKind::DirectGameImage,
    },
    // Macintosh disk images and StuffIt archives are media registrations;
    // `.sit` retains its Archive content classification in ingestion.
    MediaFormat {
        extension: "hfv",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "dc42",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "sit",
        kind: ArchiveKind::DirectGameImage,
    },
    // CHD (MAME Compressed Hunks of Data). Shared by many disc-based
    // platforms (Neo Geo CD, Sega CD, arcade sets, redump CD/DVD sets...);
    // resolving *which* platform a `.chd` belongs to is, again, the
    // platform registry's job, driven by folder/source evidence - `.chd`
    // is deliberately never strong extension evidence for any platform.
    MediaFormat {
        extension: "chd",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Loose cartridge ROMs ---
    //
    // Every entry below is a self-evidencing cartridge-ROM extension whose
    // platform (e.g. Nintendo 64, Game Boy Advance) needs no corroboration
    // from folder/source/header evidence to decide *what kind of file* it
    // is. Which *specific game system* it belongs to remains the platform
    // registry's decision via strong/weak extension evidence, exactly as
    // for `.iso`/`.d64`/`.chd` above. These extensions are already
    // recognised by the ingestion content registry
    // (`ContentKind::RomCartridge`) and by the per-platform strong-extension
    // tables in `crate::platform::PLATFORMS`; adding them here lets the
    // archive scanner persist them as `DirectGameImage` rows so the
    // existing Library Organisation pipeline can obtain an `archive_id` and
    // resolve a platform identity.
    //
    // Extensions that need corroboration (`.md`, `.bin`, `.gen`) are
    // deliberately excluded — they stay in `CORROBORATION_CANDIDATE_EXTENSIONS`.
    // Likewise, `.hdf` and `.smd` are already handled elsewhere and are not
    // duplicated here.
    // --- Nintendo 64 ---
    MediaFormat {
        extension: "z64",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "n64",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "v64",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Game Boy Advance ---
    MediaFormat {
        extension: "gba",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Game Boy / Game Boy Color ---
    MediaFormat {
        extension: "gb",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "gbc",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Super Nintendo / Super Famicom ---
    MediaFormat {
        extension: "sfc",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "smc",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Nintendo Entertainment System ---
    MediaFormat {
        extension: "nes",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "unf",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "fds",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Nintendo DS ---
    MediaFormat {
        extension: "nds",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Sega Master System ---
    MediaFormat {
        extension: "sms",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- Sega Game Gear ---
    MediaFormat {
        extension: "gg",
        kind: ArchiveKind::DirectGameImage,
    },
    // Bandai WonderSwan cartridge images. `.ws` and `.wsc` are distinct
    // platform-specific extensions; shared `.bin` remains a corroboration
    // candidate and is deliberately not registered here.
    MediaFormat {
        extension: "ws",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "wsc",
        kind: ArchiveKind::DirectGameImage,
    },
    // --- MSX cartridge images ---
    // `.mx1` and `.mx2` are generation-specific cartridge formats. The
    // platform registry decides whether they are MSX or MSX2; `.rom`/`.bin`
    // remain shared and corroboration-only.
    MediaFormat {
        extension: "mx1",
        kind: ArchiveKind::DirectGameImage,
    },
    MediaFormat {
        extension: "mx2",
        kind: ArchiveKind::DirectGameImage,
    },
];

/// Extensions that never resolve to an [`ArchiveKind`] on their own - they
/// need folder, source-root, or cartridge-header corroboration first (see
/// `crate::archive_kind_in_root`) - but that a filesystem watcher should
/// still treat as worth a rescan, since a rescan is what re-evaluates that
/// corroboration. Recognising a corroboration candidate here is never a
/// claim that the file *is* library media, only that it might become media
/// once evidence elsewhere confirms it.
const CORROBORATION_CANDIDATE_EXTENSIONS: &[&str] = &["md", "bin", "gen"];

/// The persisted [`ArchiveKind`] for `extension` (lowercase, no dot), if
/// this registry recognises it as media on its own.
pub fn kind_for_extension(extension: &str) -> Option<ArchiveKind> {
    MEDIA_FORMATS
        .iter()
        .find(|format| format.extension == extension)
        .map(|format| format.kind)
}

/// Whether `extension` (lowercase, no dot) is recognised as media on its
/// own, without corroboration.
pub fn is_recognized_extension(extension: &str) -> bool {
    kind_for_extension(extension).is_some()
}

/// Whether `extension` (lowercase, no dot) is a Mega Drive corroboration
/// candidate - see [`CORROBORATION_CANDIDATE_EXTENSIONS`]. This is the one
/// authoritative list; `crate::archive_kind_in_root` must consult it rather
/// than hardcoding its own copy of `"md" | "bin" | "gen"`.
pub fn is_corroboration_candidate(extension: &str) -> bool {
    CORROBORATION_CANDIDATE_EXTENSIONS.contains(&extension)
}

/// Whether a filesystem-watcher event on a file with `extension` (lowercase,
/// no dot) is worth a rescan: either the extension is recognised outright,
/// or it is a corroboration candidate whose eventual kind depends on
/// evidence a rescan re-evaluates. This is the single source of truth the
/// watcher consults - it must never maintain its own extension list.
pub fn is_watch_relevant_extension(extension: &str) -> bool {
    is_recognized_extension(extension) || is_corroboration_candidate(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_extension_is_lowercase_without_a_dot() {
        for format in MEDIA_FORMATS {
            assert!(!format.extension.starts_with('.'), "{}", format.extension);
            assert_eq!(format.extension, format.extension.to_ascii_lowercase());
        }
    }

    #[test]
    fn no_extension_is_registered_twice() {
        let mut seen = std::collections::HashSet::new();
        for format in MEDIA_FORMATS {
            assert!(
                seen.insert(format.extension),
                "`{}` is registered more than once",
                format.extension
            );
        }
    }

    #[test]
    fn watch_relevant_extensions_include_every_registered_extension() {
        for format in MEDIA_FORMATS {
            assert!(is_watch_relevant_extension(format.extension));
        }
    }

    #[test]
    fn watch_relevant_extensions_include_corroboration_candidates() {
        for extension in CORROBORATION_CANDIDATE_EXTENSIONS {
            assert!(is_watch_relevant_extension(extension));
        }
    }

    #[test]
    fn an_unrecognised_extension_is_neither_a_kind_nor_watch_relevant() {
        assert_eq!(kind_for_extension("nfo"), None);
        assert!(!is_watch_relevant_extension("nfo"));
    }

    // --- Loose cartridge-ROM recognition (added for Library Organisation gap) ---

    #[test]
    fn z64_is_recognized_as_direct_game_image() {
        assert_eq!(
            kind_for_extension("z64"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn gba_is_recognized_as_direct_game_image() {
        assert_eq!(
            kind_for_extension("gba"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn commodore_disk_extensions_are_recognized_as_direct_game_images() {
        for extension in ["d64", "g64", "d71", "d81"] {
            assert_eq!(
                kind_for_extension(extension),
                Some(ArchiveKind::DirectGameImage),
                ".{extension} must be a direct game image"
            );
        }
    }

    #[test]
    fn snes_extensions_are_recognized() {
        assert_eq!(
            kind_for_extension("sfc"),
            Some(ArchiveKind::DirectGameImage)
        );
        assert_eq!(
            kind_for_extension("smc"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn gb_and_gbc_are_recognized() {
        assert_eq!(kind_for_extension("gb"), Some(ArchiveKind::DirectGameImage));
        assert_eq!(
            kind_for_extension("gbc"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn nes_extensions_are_recognized() {
        assert_eq!(
            kind_for_extension("nes"),
            Some(ArchiveKind::DirectGameImage)
        );
        assert_eq!(
            kind_for_extension("unf"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn fds_is_recognized_as_a_direct_game_image() {
        assert_eq!(
            kind_for_extension("fds"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn nds_is_recognized_as_direct_game_image() {
        assert_eq!(
            kind_for_extension("nds"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn sega_cartridge_extensions_are_recognized() {
        assert_eq!(
            kind_for_extension("sms"),
            Some(ArchiveKind::DirectGameImage)
        );
        assert_eq!(kind_for_extension("gg"), Some(ArchiveKind::DirectGameImage));
    }

    #[test]
    fn wonderswan_cartridge_extensions_are_direct_game_images() {
        assert_eq!(kind_for_extension("ws"), Some(ArchiveKind::DirectGameImage));
        assert_eq!(
            kind_for_extension("wsc"),
            Some(ArchiveKind::DirectGameImage)
        );
        assert_eq!(kind_for_extension("bin"), None);
    }

    #[test]
    fn xbox_executables_and_xiso_are_direct_media() {
        for extension in ["xbe", "xex", "xiso"] {
            assert_eq!(
                kind_for_extension(extension),
                Some(ArchiveKind::DirectGameImage),
                ".{extension} must be a direct game image"
            );
        }
        assert_eq!(kind_for_extension("god"), None);
    }

    #[test]
    fn msx_cartridge_extensions_are_recognized() {
        assert_eq!(
            kind_for_extension("mx1"),
            Some(ArchiveKind::DirectGameImage)
        );
        assert_eq!(
            kind_for_extension("mx2"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn pc98_hard_disk_extensions_are_direct_game_images() {
        for extension in ["hdi", "nhd"] {
            assert_eq!(
                kind_for_extension(extension),
                Some(ArchiveKind::DirectGameImage),
                ".{extension} must be a direct game image"
            );
        }
    }

    #[test]
    fn atari_media_extensions_are_direct_game_images() {
        for extension in [
            "a26", "a52", "a78", "atr", "atx", "xex", "xfd", "lnx", "lyx", "j64", "jag", "stx",
            "st", "msa", "ipf",
        ] {
            assert_eq!(
                kind_for_extension(extension),
                Some(ArchiveKind::DirectGameImage),
                ".{extension} must be a direct Atari media registration"
            );
        }
        assert_eq!(kind_for_extension("cas"), None);
    }

    #[test]
    fn apple_media_extensions_are_recognized_as_direct_game_images() {
        for extension in ["do", "po", "woz", "2mg", "nib", "hfv", "dc42", "sit"] {
            assert_eq!(
                kind_for_extension(extension),
                Some(ArchiveKind::DirectGameImage),
                ".{extension} must be a direct media registration"
            );
        }
    }

    #[test]
    fn n64_variants_are_all_recognized() {
        assert_eq!(
            kind_for_extension("n64"),
            Some(ArchiveKind::DirectGameImage)
        );
        assert_eq!(
            kind_for_extension("v64"),
            Some(ArchiveKind::DirectGameImage)
        );
    }

    #[test]
    fn still_unrecognized_extensions_fail_closed() {
        // Extensions that are not in MEDIA_FORMATS and not corroboration
        // candidates must still return None.
        assert_eq!(kind_for_extension("exe"), None);
        assert_eq!(kind_for_extension("pdf"), None);
        assert_eq!(kind_for_extension("txt"), None);
    }

    #[test]
    fn corroboration_candidates_are_still_not_self_evidencing() {
        // `.md`, `.bin`, `.gen` need folder/source/header corroboration and
        // must never resolve from `kind_for_extension` alone.
        assert_eq!(kind_for_extension("md"), None);
        assert_eq!(kind_for_extension("bin"), None);
        assert_eq!(kind_for_extension("gen"), None);
        assert_eq!(kind_for_extension("rom"), None);
        // They are still watch-relevant though.
        assert!(is_watch_relevant_extension("md"));
        assert!(is_watch_relevant_extension("bin"));
        assert!(is_watch_relevant_extension("gen"));
    }

    #[test]
    fn d88_is_a_direct_game_image() {
        assert_eq!(
            kind_for_extension("d88"),
            Some(ArchiveKind::DirectGameImage)
        );
    }
}
