//! Typed, bounded description of a non-WHDLoad Amiga trainer.
//!
//! Only what is actually evidenced is representable: a set of memory writes, the
//! exact image(s) they were authored for, and where the claim came from. There is
//! no free-form command string anywhere, so nothing in this model can be
//! interpreted as something to execute.

use serde::Serialize;

use crate::media_set::IdentityKey;

pub const AMIGA_TRAINER_SCHEMA: &str = "emuwiz-amiga-adf-trainer/1";

/// Source file size accepted by the importer.
pub const MAX_TRAINER_SOURCE_BYTES: usize = 256 * 1024;
/// Entries read from one source file; further entries are counted and dropped.
pub const MAX_TRAINERS_PER_FILE: usize = 1024;
/// Trainers that may be planned for one image.
pub const MAX_TRAINERS_PER_GAME: usize = 256;
pub const MAX_WRITES_PER_TRAINER: usize = 64;
pub const MAX_MEDIA_REFS_PER_TRAINER: usize = 16;
pub const MAX_TITLE_BYTES: usize = 128;
pub const MAX_DESCRIPTION_BYTES: usize = 512;
pub const MAX_SOURCE_FIELD_BYTES: usize = 256;
/// A numeric field's text, before parsing (`0x` + 8 hex digits, or decimal).
pub const MAX_NUMBER_TEXT_BYTES: usize = 18;
/// Import diagnostics and conflict groups kept for display.
pub const MAX_REPORTED_ITEMS: usize = 256;

/// What a trainer claims to do. Only [`Self::MemoryWrite`] is representable as a
/// plan; every other mechanism is recognised so it can be shown and classified,
/// and is never applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AmigaTrainerMechanism {
    MemoryWrite,
    /// A trainer menu that runs from a modified disk image.
    BootMenu,
    /// A pre-trained disk image: alternate media, not an applied cheat.
    TrainedDisk,
    /// Action Replay-style cartridge codes (no code model and no ROM here).
    ActionReplayCode,
    /// Editing an emulator's save state (emulator-specific, not represented).
    SaveStateEdit,
    /// Patching disk sectors: needs writable media.
    DiskPatch,
}

impl AmigaTrainerMechanism {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MemoryWrite => "memory write",
            Self::BootMenu => "boot-time trainer menu",
            Self::TrainedDisk => "pre-trained disk image",
            Self::ActionReplayCode => "Action Replay-style code",
            Self::SaveStateEdit => "save-state edit",
            Self::DiskPatch => "disk patch",
        }
    }

    /// Why a mechanism cannot be planned. `None` for memory writes.
    #[must_use]
    pub const fn unsupported_reason(self) -> Option<&'static str> {
        match self {
            Self::MemoryWrite => None,
            Self::BootMenu => Some(
                "a trainer menu is part of a modified disk image; that disk is alternate media, not an EmuWiz-applied cheat",
            ),
            Self::TrainedDisk => Some(
                "a pre-trained or cracked disk is alternate media; EmuWiz does not generate, download or merge it",
            ),
            Self::ActionReplayCode => Some(
                "Action Replay codes need an emulated cartridge (a copyrighted ROM) and EmuWiz has no code model for them",
            ),
            Self::SaveStateEdit => {
                Some("save-state editing is emulator-specific and no state format is represented")
            }
            Self::DiskPatch => Some(
                "patching sectors needs writable media: it would require scratch-copy launch integration, which is not implemented",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AmigaMemoryWidth {
    Byte,
    Word,
    Long,
}

impl AmigaMemoryWidth {
    #[must_use]
    pub const fn bytes(self) -> u32 {
        match self {
            Self::Byte => 1,
            Self::Word => 2,
            Self::Long => 4,
        }
    }

    #[must_use]
    pub const fn max_value(self) -> u32 {
        match self {
            Self::Byte => 0xFF,
            Self::Word => 0xFFFF,
            Self::Long => u32::MAX,
        }
    }
}

/// When a write is meant to happen, if the source said. Unknown is a valid answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AmigaTrainerTiming {
    #[default]
    Unknown,
    Immediate,
    AfterBoot,
    EveryFrame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AmigaMemoryWrite {
    pub width: AmigaMemoryWidth,
    /// A 68k address. Word and long writes are even-aligned (an odd address is an
    /// address error on the 68000), checked at import.
    pub address: u32,
    pub value: u32,
    /// The value expected before the write, as a guard, when the source gave one.
    pub original: Option<u32>,
    pub timing: AmigaTrainerTiming,
}

/// How far a trainer's claim reaches across the disks of a title.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum AmigaTrainerScope {
    /// Every disk of one release; needs the complete, verified disk set.
    WholeTitle,
    /// One specific disk (1-based). Never carried over to another disk.
    Disk(u16),
    /// One specific release revision; the revision must be evidenced.
    Revision,
}

/// One exact image the trainer was authored against.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AmigaMediaRef {
    /// 64 lower-case hex characters: whole-image SHA-256.
    pub sha256: String,
    pub disk: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AmigaTrainerTarget {
    pub media: Vec<AmigaMediaRef>,
    pub scope: AmigaTrainerScope,
    /// The release (set) this applies to, in the media-set identity vocabulary.
    #[serde(skip)]
    pub release: Option<IdentityKey>,
    pub region: Option<String>,
    pub revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AmigaTrainerSource {
    pub name: String,
    pub reference: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AmigaTrainer {
    /// Position in the source file, for provenance and stable ordering.
    pub index: usize,
    /// What the source says this is for; checked against the verified platform.
    pub platform: String,
    pub title: String,
    pub description: Option<String>,
    pub mechanism: AmigaTrainerMechanism,
    pub target: AmigaTrainerTarget,
    pub writes: Vec<AmigaMemoryWrite>,
    pub source: AmigaTrainerSource,
}

/// What can be done with a trainer right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum AmigaTrainerStatus {
    /// EmuWiz can generate a deterministic artifact an emulator consumes, from
    /// evidenced syntax. **No emulator qualifies today**, so nothing reaches
    /// this state; it exists so a future, evidenced projection has a place.
    Preparable,
    /// Data can be shown, but identity or applicability is not established.
    PreviewOnly,
    /// A complete, verified, deterministic plan exists, but only a running
    /// emulator can act on it.
    RequiresEmulatorRuntime,
    /// Cannot be used for this mechanism, media or target.
    Unsupported,
}

impl AmigaTrainerStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Preparable => "Can be prepared",
            Self::PreviewOnly => "Preview only",
            Self::RequiresEmulatorRuntime => "Needs a running emulator",
            Self::Unsupported => "Not supported",
        }
    }
}

/// Other Amiga media: readiness only, never trainer preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AmigaMediaFamily {
    AdfFloppy,
    Adz,
    Dms,
    Ipf,
    Hdf,
    WhdloadInstall,
    Cd32Cdtv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MediaFamilyReadiness {
    pub status: AmigaTrainerStatus,
    pub note: &'static str,
}

#[must_use]
pub const fn trainer_readiness_for(family: AmigaMediaFamily) -> MediaFamilyReadiness {
    let (status, note) = match family {
        AmigaMediaFamily::AdfFloppy => (
            AmigaTrainerStatus::RequiresEmulatorRuntime,
            "implemented as a plan: exact-image memory-write trainers; applying one needs a running emulator",
        ),
        AmigaMediaFamily::Adz => (
            AmigaTrainerStatus::Unsupported,
            "gzip-compressed ADF: identity would be of the decompressed bytes, and decompression is not implemented here",
        ),
        AmigaMediaFamily::Dms => (
            AmigaTrainerStatus::Unsupported,
            "DMS needs a decompressor that is not a dependency",
        ),
        AmigaMediaFamily::Ipf => (
            AmigaTrainerStatus::Unsupported,
            "IPF is opaque without the proprietary CAPS/SPS library; hash-only pass-through",
        ),
        AmigaMediaFamily::Hdf => (
            AmigaTrainerStatus::Unsupported,
            "installed hard-disk images are writable installs; any in-image change would need scratch-copy launch integration",
        ),
        AmigaMediaFamily::WhdloadInstall => (
            AmigaTrainerStatus::Unsupported,
            "WHDLoad trainers are launch options (see patch_manager::whdload_trainer), not memory writes; they do not apply to ADF and ADF trainers do not apply to WHDLoad",
        ),
        AmigaMediaFamily::Cd32Cdtv => (
            AmigaTrainerStatus::Unsupported,
            "optical Amiga media: not floppy, and no trainer evidence exists",
        ),
    };
    MediaFamilyReadiness { status, note }
}

/// How well an emulator's debugger mechanism is evidenced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum RuntimeEvidence {
    /// Described in a third-party guide (URL). Not first-party documentation.
    ThirdPartyGuide(&'static str),
    /// No evidence was found for this emulator.
    NotEvidenced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AmigaRuntimeOption {
    pub emulator: &'static str,
    pub how: &'static str,
    /// Whether a non-interactive, scriptable form is evidenced. None is.
    pub scripted_form_evidenced: bool,
    pub evidence: RuntimeEvidence,
}

const GUIDE: &str = "https://tetracorp.github.io/guide/reverse-engineering-amiga.html";

/// What is known about acting on a memory-write plan in a running emulator.
/// The evidenced mechanism is an *interactive* debugger command on a frozen
/// emulation. Width semantics are not specified by the evidence, and no
/// scripted or file-based form is evidenced, so no projection is generated and
/// no process is spawned.
#[must_use]
pub const fn amiga_runtime_options() -> [AmigaRuntimeOption; 3] {
    [
        AmigaRuntimeOption {
            emulator: "FS-UAE",
            how: "built-in debugger (F12 then D), interactive `W <address> <value>`",
            scripted_form_evidenced: false,
            evidence: RuntimeEvidence::ThirdPartyGuide(GUIDE),
        },
        AmigaRuntimeOption {
            emulator: "WinUAE",
            how: "built-in debugger (Shift+F12), interactive `W <address> <value>`",
            scripted_form_evidenced: false,
            evidence: RuntimeEvidence::ThirdPartyGuide(GUIDE),
        },
        AmigaRuntimeOption {
            emulator: "Amiberry",
            how: "UAE-derived, but a debugger write mechanism was not independently evidenced",
            scripted_form_evidenced: false,
            evidence: RuntimeEvidence::NotEvidenced,
        },
    ]
}
