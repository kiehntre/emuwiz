//! Single source of truth for "what kind of game content does this
//! extension represent", independent of how it is stored.
//!
//! This is deliberately a second registry alongside
//! [`crate::media_registry`], not a replacement for it. `media_registry`
//! answers "is this recognised as an [`crate::ArchiveKind`]" for the
//! existing archive-centric scanner; this one answers a broader, coarser
//! question for the [`super`] discovery pipeline - "what *category* of
//! content is this" (a cartridge ROM, a disc image, an Amiga image, a
//! computer floppy image) - without ever assigning a specific platform.
//! Platform assignment stays entirely [`crate::platform`]'s job, reused
//! unmodified via [`crate::ArchiveIdentity::from_path`].
//!
//! Like `media_registry`, a new extension is a one-line table addition;
//! [`super::tests::registry_extensions_are_all_individually_classified`]
//! keeps this table and [`ContentKind::label`] from drifting apart.

/// The coarse category of game content a file or container represents.
/// Never a platform - see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    /// A loose cartridge/ROM dump (Nintendo, Sega, and similar systems).
    RomCartridge,
    /// A disc-based game image (CD/DVD/GD-ROM), including CUE/BIN pairs.
    DiscImage,
    /// An Amiga disk or hard-drive image (ADF/HDF/HDFX/RDB) or an LhA
    /// archive of one.
    AmigaImage,
    /// A non-Amiga computer floppy/disk image already supported elsewhere
    /// in EmuWiz (Atari ST, Amstrad, and similar).
    ComputerDisk,
    /// A cassette/tape image. This deliberately says nothing about the
    /// platform: `.tap` and `.tzx` are shared by several 8-bit families and
    /// `.cdt` needs separate platform evidence too.
    TapeImage,
    /// A machine-state snapshot (`.z80`, `.sna`, `.szx`, ...): a frozen
    /// memory + register dump rather than a ROM, disc or tape. Category
    /// only - the Sinclair family it belongs to comes from parsing the
    /// bytes, not from this table.
    MachineSnapshot,
    /// A folder that is itself a WHDLoad installation (contains a
    /// `.slave` file), not a folder to recurse into.
    WhdloadInstall,
    /// A folder containing recognisable game content directly (an
    /// "extracted game folder" / emulator-ready folder), not a folder to
    /// recurse into.
    ExtractedGameFolder,
}

impl ContentKind {
    /// What a person sees for this content category. Never archive
    /// terminology - "what the game is", not "how it's stored".
    pub fn label(self) -> &'static str {
        match self {
            Self::RomCartridge => "ROM cartridge",
            Self::DiscImage => "Disc image",
            Self::AmigaImage => "Amiga image",
            Self::ComputerDisk => "Computer disk image",
            Self::TapeImage => "Cassette/tape image",
            Self::MachineSnapshot => "Machine snapshot",
            Self::WhdloadInstall => "WHDLoad install",
            Self::ExtractedGameFolder => "Game folder",
        }
    }
}

/// One extension EmuWiz recognises as belonging to a [`ContentKind`] on
/// its own (no folder/header corroboration needed to know the *category*;
/// corroboration is still needed downstream to know the *platform*).
#[derive(Debug, Clone, Copy)]
struct ContentFormat {
    extension: &'static str,
    kind: ContentKind,
}

/// Loose-file content formats. `cue`/`bin` are handled separately by
/// [`super::cue_bin`] - a `.cue` is the anchor for a disc candidate and a
/// lone `.bin` is paired or explicitly flagged, never guessed from this
/// table alone, so neither extension appears here.
const CONTENT_FORMATS: &[ContentFormat] = &[
    // --- Nintendo ---
    cf("nes", ContentKind::RomCartridge),
    cf("unf", ContentKind::RomCartridge),
    cf("smc", ContentKind::RomCartridge),
    cf("sfc", ContentKind::RomCartridge),
    cf("gb", ContentKind::RomCartridge),
    cf("gbc", ContentKind::RomCartridge),
    cf("gba", ContentKind::RomCartridge),
    cf("nds", ContentKind::RomCartridge),
    cf("z64", ContentKind::RomCartridge),
    cf("n64", ContentKind::RomCartridge),
    cf("v64", ContentKind::RomCartridge),
    // --- Sega ---
    cf("sms", ContentKind::RomCartridge),
    cf("gg", ContentKind::RomCartridge),
    // --- MSX ---
    cf("mx1", ContentKind::RomCartridge),
    cf("mx2", ContentKind::RomCartridge),
    cf("md", ContentKind::RomCartridge),
    cf("gen", ContentKind::RomCartridge),
    // WonderSwan cartridge images are distinct extensions. Shared `.bin`
    // remains intentionally absent and is handled only by cue/bin pairing.
    cf("ws", ContentKind::RomCartridge),
    cf("wsc", ContentKind::RomCartridge),
    // --- Disc-based ---
    cf("iso", ContentKind::DiscImage),
    cf("chd", ContentKind::DiscImage),
    cf("gdi", ContentKind::DiscImage),
    cf("cdi", ContentKind::DiscImage),
    // --- Amiga ---
    // `.hdf`/`.hdfx` are deliberately NOT registered here: they are a
    // real-world extension collision (Sharp X68000 hard-disk images also
    // use `.hdf` - confirmed against a real X68000 collection during
    // validation, which mislabelled `.hdf` archive members as Amiga
    // content). Files with these extensions are classified in
    // `super::discovery::discover_direct_file`, which requires either a
    // successful `amiga_disk::inspect_amiga_image` parse or independent
    // platform evidence before committing to a content kind - see that
    // function's docs.
    cf("adf", ContentKind::AmigaImage),
    cf("adz", ContentKind::AmigaImage),
    cf("rdb", ContentKind::AmigaImage),
    cf("lha", ContentKind::AmigaImage),
    // --- Computer formats already supported elsewhere in EmuWiz ---
    cf("dsk", ContentKind::ComputerDisk),
    cf("d64", ContentKind::ComputerDisk),
    cf("g64", ContentKind::ComputerDisk),
    cf("d71", ContentKind::ComputerDisk),
    cf("d81", ContentKind::ComputerDisk),
    cf("st", ContentKind::ComputerDisk),
    cf("msa", ContentKind::ComputerDisk),
    cf("ipf", ContentKind::ComputerDisk),
    cf("hdi", ContentKind::ComputerDisk),
    cf("ima", ContentKind::ComputerDisk),
    cf("img", ContentKind::ComputerDisk),
    cf("d88", ContentKind::ComputerDisk),
    // ZX Spectrum TR-DOS media. The *category* is a computer disk/archive;
    // the ZX Spectrum platform is confirmed only when
    // `crate::disk_format::inspect_disk_format` validates the TR-DOS system
    // descriptor (`.trd`) or the SINCLAIR archive layout (`.scl`) - see
    // `super::discovery::discover_trdos_media`.
    cf("trd", ContentKind::ComputerDisk),
    cf("scl", ContentKind::ComputerDisk),
    // These extensions identify a media family, not a system. Keep them out
    // of platform extension evidence unless another source can prove it.
    cf("cdt", ContentKind::TapeImage),
    cf("tap", ContentKind::TapeImage),
    cf("tzx", ContentKind::TapeImage),
    // --- Sinclair-family machine snapshots ---
    // The category is knowable from the extension; the machine family is not,
    // and is confirmed by `crate::zx_spectrum_snapshot` parsing the bytes in
    // `super::discovery::discover_direct_file`. `.szx` is included because it
    // has a self-identifying `ZXST` header; `.trd`/`.scl` (TR-DOS disks) are
    // deliberately still absent pending a filesystem-level parser.
    cf("z80", ContentKind::MachineSnapshot),
    cf("sna", ContentKind::MachineSnapshot),
    cf("szx", ContentKind::MachineSnapshot),
];

const fn cf(extension: &'static str, kind: ContentKind) -> ContentFormat {
    ContentFormat { extension, kind }
}

/// The [`ContentKind`] for a loose file's extension (lowercase, no leading
/// dot), if this registry recognises it on its own. `cue`/`bin` are never
/// resolved here - see [`super::cue_bin`].
pub fn content_kind_for_extension(extension: &str) -> Option<ContentKind> {
    CONTENT_FORMATS
        .iter()
        .find(|format| format.extension == extension)
        .map(|format| format.kind)
}

/// Every extension this registry recognises, for parity tests and for
/// "does this archive member look like game content" checks.
pub fn recognized_extensions() -> impl Iterator<Item = &'static str> {
    CONTENT_FORMATS.iter().map(|format| format.extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_extension_resolves_to_itself() {
        for format in CONTENT_FORMATS {
            assert_eq!(
                content_kind_for_extension(format.extension),
                Some(format.kind),
                "extension {} did not round-trip",
                format.extension
            );
        }
    }

    #[test]
    fn unknown_extension_resolves_to_nothing() {
        assert_eq!(content_kind_for_extension("xyz_not_a_real_format"), None);
    }

    #[test]
    fn cue_and_bin_are_never_registered_here() {
        assert_eq!(content_kind_for_extension("cue"), None);
        assert_eq!(content_kind_for_extension("bin"), None);
    }

    #[test]
    fn msx_cartridges_are_rom_content() {
        assert_eq!(
            content_kind_for_extension("mx1"),
            Some(ContentKind::RomCartridge)
        );
        assert_eq!(
            content_kind_for_extension("mx2"),
            Some(ContentKind::RomCartridge)
        );
        assert_eq!(content_kind_for_extension("rom"), None);
    }

    #[test]
    fn d88_is_computer_disk_content() {
        assert_eq!(
            content_kind_for_extension("d88"),
            Some(ContentKind::ComputerDisk)
        );
    }
}
