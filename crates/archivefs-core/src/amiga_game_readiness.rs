//! Read-only Amiga game readiness for one selected library entry.
//!
//! This joins evidence that already exists elsewhere: the stored platform,
//! the file's representation, discovered FS-UAE/Amiberry profiles, their
//! Kickstart inspection, the bounded WHDLoad package inspector and the
//! library's multi-disk topology. It explains what is ready and what is not.
//! It never builds a launch command, grants Play, opens a disk image, extracts
//! a package, or writes anything. Launch planning and preflight stay the only
//! authority for starting a game.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::amiga_cd_evidence::{
    AMIGA_CD32_PLATFORM_ID, AMIGA_CDTV_PLATFORM_ID, AMIGA_PLATFORM_ID, AmigaCdMachine,
};
use crate::amiga_whdload_archive::{WhdloadArchiveError, discover_whdload_slaves_in_archive};
use crate::patch_manager::{
    AmigaEmulatorKind, AmigaGameRequest, AmigaKickstartState, AmigaProfile,
    AmigaProfileDiscoveryRoots, discover_amiga_profiles, inspect_amiga_whdload_game,
};

/// WHDLoad `ws_Flags` bits (WHDLoad Slave header, documented flag values).
const WHDLF_REQ68020: u16 = 0x0010;
const WHDLF_REQAGA: u16 = 0x0020;

/// How the selected entry's file is stored. Each kind is distinct: an ADF is
/// not a DMS, an IPF or an HFE, and CD media is never a floppy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaMediaKind {
    /// Raw AmigaDOS-sized floppy image.
    Adf,
    /// Gzip-compressed ADF.
    Adz,
    /// DiskMasher archive. EmuWiz has no DMS decoder.
    Dms,
    /// SPS/CAPS preservation image. Needs the CAPS/SPS library.
    Ipf,
    /// HxC/Gotek floppy-emulator image. No Amiga launch route exists.
    Hfe,
    /// HDF, HDFX or RDB hard-disk image.
    HardDisk,
    /// LHA/LZH archive that may hold an installed WHDLoad game.
    WhdloadPackage,
    /// CUE/BIN, ISO or CHD disc image.
    CdImage,
    /// ZIP/7z/RAR container. EmuWiz never extracts it for Amiga play.
    Container,
    Unsupported,
}

impl AmigaMediaKind {
    pub fn from_path(path: &Path) -> Self {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "adf" => Self::Adf,
            "adz" => Self::Adz,
            "dms" => Self::Dms,
            "ipf" => Self::Ipf,
            "hfe" => Self::Hfe,
            "hdf" | "hdfx" | "rdb" => Self::HardDisk,
            "lha" | "lzh" => Self::WhdloadPackage,
            "cue" | "iso" | "chd" => Self::CdImage,
            "zip" | "7z" | "rar" => Self::Container,
            _ => Self::Unsupported,
        }
    }

    /// Short format name for presentation.
    pub fn label(self) -> &'static str {
        match self {
            Self::Adf => "ADF floppy image",
            Self::Adz => "ADZ (compressed ADF) floppy image",
            Self::Dms => "DMS disk archive",
            Self::Ipf => "IPF preservation image",
            Self::Hfe => "HFE floppy-emulator image",
            Self::HardDisk => "Hard-disk image (HDF/RDB)",
            Self::WhdloadPackage => "WHDLoad package (LHA)",
            Self::CdImage => "CD image",
            Self::Container => "Packed in an archive",
            Self::Unsupported => "Unrecognised file type",
        }
    }

    pub fn is_floppy(self) -> bool {
        matches!(
            self,
            Self::Adf | Self::Adz | Self::Dms | Self::Ipf | Self::Hfe
        )
    }
}

/// The Amiga machine family the stored platform names, or `None` for any
/// other platform. A file extension never decides this.
pub fn machine_for_platform(platform_id: &str) -> Option<AmigaCdMachine> {
    let canonical = crate::canonical_platform_for_alias(platform_id).unwrap_or(platform_id);
    match canonical {
        AMIGA_PLATFORM_ID => Some(AmigaCdMachine::OrdinaryAmiga),
        AMIGA_CD32_PLATFORM_ID => Some(AmigaCdMachine::Cd32),
        AMIGA_CDTV_PLATFORM_ID => Some(AmigaCdMachine::Cdtv),
        _ => None,
    }
}

pub fn machine_label(machine: AmigaCdMachine) -> &'static str {
    match machine {
        AmigaCdMachine::OrdinaryAmiga => "Amiga",
        AmigaCdMachine::Cd32 => "Amiga CD32",
        AmigaCdMachine::Cdtv => "Commodore CDTV",
    }
}

pub fn emulator_label(emulator: AmigaEmulatorKind) -> &'static str {
    match emulator {
        AmigaEmulatorKind::FsUae => "FS-UAE",
        AmigaEmulatorKind::Amiberry => "Amiberry",
    }
}

/// What one discovered emulator profile says, read from its own files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaProfileFacts {
    pub emulator: AmigaEmulatorKind,
    pub profile_id: String,
    pub eligible: bool,
    pub executable_found: bool,
    pub kickstart: AmigaKickstartState,
    pub machine_model: Option<String>,
    pub chipset: Option<String>,
    pub whdload_support_present: bool,
}

impl AmigaProfileFacts {
    /// The machine family the configuration names, if it names one.
    pub fn machine(&self) -> Option<AmigaCdMachine> {
        let model = self.machine_model.as_deref()?.trim().to_ascii_uppercase();
        if model.contains("CD32") {
            Some(AmigaCdMachine::Cd32)
        } else if model.contains("CDTV") {
            Some(AmigaCdMachine::Cdtv)
        } else if model.starts_with('A') && model[1..].starts_with(|c: char| c.is_ascii_digit()) {
            Some(AmigaCdMachine::OrdinaryAmiga)
        } else {
            None
        }
    }

    /// Whether the configuration provides the AGA chipset, when it says.
    pub fn aga(&self) -> Option<bool> {
        if let Some(chipset) = self.chipset.as_deref() {
            let chipset = chipset.to_ascii_lowercase();
            if chipset.contains("aga") {
                return Some(true);
            }
            if chipset.contains("ocs") || chipset.contains("ecs") {
                return Some(false);
            }
        }
        let model = self.machine_model.as_deref()?.trim().to_ascii_uppercase();
        if ["A1200", "A4000", "CD32"]
            .iter()
            .any(|aga| model.starts_with(aga))
        {
            Some(true)
        } else if ["A500", "A600", "A1000", "A2000", "A3000", "CDTV"]
            .iter()
            .any(|ocs| model.starts_with(ocs))
        {
            Some(false)
        } else {
            None
        }
    }
}

/// What the bounded WHDLoad package inspector found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhdloadPackageFacts {
    Slaves(Vec<WhdloadSlaveFacts>),
    BackendUnavailable,
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadSlaveFacts {
    pub member_path: String,
    pub requires_aga: bool,
    pub requires_68020: bool,
}

/// Everything read from disk for one assessment. Collected off the UI thread.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AmigaEnvironmentFacts {
    pub profiles: Vec<AmigaProfileFacts>,
    /// False when the environment has no HOME to discover profiles from.
    pub discovery_available: bool,
    pub whdload: Option<WhdloadPackageFacts>,
    /// For an ADF: the DAT entry its hash matched plus why it counts as a
    /// complete single-disk game, or the plain reason it is not launchable.
    /// `None` when not read.
    pub adf_identity: Option<Result<String, String>>,
}

/// The library's own multi-disk grouping for this entry, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmigaDiskSetFacts {
    pub count: usize,
    pub order_verified: bool,
    pub problems: usize,
}

/// Reads the discovered emulator profiles and, for an LHA/LZH package, the
/// package's Slave list. Read-only and bounded by the existing inspectors.
pub fn observe_amiga_environment(
    roots: Option<&AmigaProfileDiscoveryRoots>,
    content: &Path,
    cancel: &AtomicBool,
) -> AmigaEnvironmentFacts {
    let profiles = roots
        .map(|roots| {
            discover_amiga_profiles(roots)
                .profiles
                .iter()
                .map(profile_facts)
                .collect()
        })
        .unwrap_or_default();
    let whdload =
        (AmigaMediaKind::from_path(content) == AmigaMediaKind::WhdloadPackage).then(|| {
            match discover_whdload_slaves_in_archive(content, cancel) {
                Ok(discovery) => WhdloadPackageFacts::Slaves(
                    discovery
                        .candidates
                        .iter()
                        .map(|candidate| WhdloadSlaveFacts {
                            member_path: candidate.member_path.clone(),
                            requires_aga: candidate.artifact.parsed.flags & WHDLF_REQAGA != 0,
                            requires_68020: candidate.artifact.parsed.flags & WHDLF_REQ68020 != 0,
                        })
                        .collect(),
                ),
                Err(WhdloadArchiveError::BackendUnavailable) => {
                    WhdloadPackageFacts::BackendUnavailable
                }
                Err(error) => WhdloadPackageFacts::Unreadable(error.to_string()),
            }
        });
    AmigaEnvironmentFacts {
        profiles,
        discovery_available: roots.is_some(),
        whdload,
        adf_identity: None,
    }
}

/// One profile's facts, read from its own configuration and Kickstart path.
pub fn profile_facts(profile: &AmigaProfile) -> AmigaProfileFacts {
    // No identity is supplied: this inspection only reads the profile's own
    // configuration and Kickstart path, never maps the game.
    let inspection = inspect_amiga_whdload_game(profile, &AmigaGameRequest::default());
    AmigaProfileFacts {
        emulator: profile.emulator,
        profile_id: profile.profile_id.clone(),
        eligible: profile.eligible,
        executable_found: !profile.executable_candidates.is_empty(),
        kickstart: inspection.health.kickstart.state,
        machine_model: inspection.config.machine.machine_model.clone(),
        chipset: inspection.config.machine.chipset.clone(),
        whdload_support_present: inspection.health.whdload_support_present,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AmigaCheckState {
    Ready,
    Warning,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaCheckKind {
    Media,
    Emulator,
    Machine,
    Kickstart,
    Disks,
    Whdload,
    Trainers,
    Play,
}

/// Where an existing EmuWiz page can fix or explain a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmigaFixRoute {
    EmulatorSetup,
    Firmware,
    MultiDisk,
    Trainers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaCheck {
    pub kind: AmigaCheckKind,
    pub state: AmigaCheckState,
    pub detail: String,
    pub route: Option<AmigaFixRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmigaGameReadiness {
    pub machine: AmigaCdMachine,
    pub media: AmigaMediaKind,
    pub disk_count: Option<usize>,
    pub checks: Vec<AmigaCheck>,
    /// True only for a WHDLoad package on an ordinary Amiga with no blocked
    /// check: then the existing launch card may offer Play. Never Play itself.
    pub launch_route_exists: bool,
}

impl AmigaGameReadiness {
    pub fn check(&self, kind: AmigaCheckKind) -> Option<&AmigaCheck> {
        self.checks.iter().find(|check| check.kind == kind)
    }
}

fn check(
    kind: AmigaCheckKind,
    state: AmigaCheckState,
    detail: impl Into<String>,
    route: Option<AmigaFixRoute>,
) -> AmigaCheck {
    AmigaCheck {
        kind,
        state,
        detail: detail.into(),
        route,
    }
}

/// Pure assessment. `facts` is `None` while the environment is still being
/// read; emulator-dependent rows are then omitted rather than guessed.
pub fn assess_amiga_game(
    machine: AmigaCdMachine,
    content: &Path,
    scan_incomplete: bool,
    disks: Option<AmigaDiskSetFacts>,
    facts: Option<&AmigaEnvironmentFacts>,
) -> AmigaGameReadiness {
    use AmigaCheckKind as K;
    use AmigaCheckState::{Blocked, Ready, Warning};
    let media = AmigaMediaKind::from_path(content);
    let cd_machine = machine != AmigaCdMachine::OrdinaryAmiga;
    let mut checks = Vec::new();

    let (state, detail) = if scan_incomplete {
        (
            Blocked,
            "The last scan could not finish reading this file. It may be damaged or unreadable; scan it again before relying on it.".to_string(),
        )
    } else {
        media_check(machine, media)
    };
    checks.push(check(K::Media, state, detail, None));

    let slaves = match facts.and_then(|facts| facts.whdload.as_ref()) {
        Some(WhdloadPackageFacts::Slaves(slaves)) => slaves.as_slice(),
        _ => &[],
    };
    let needs_aga = slaves.iter().any(|slave| slave.requires_aga);
    if let Some(facts) = facts {
        let eligible: Vec<&AmigaProfileFacts> = facts
            .profiles
            .iter()
            .filter(|profile| profile.eligible)
            .collect();
        let runnable: Vec<&AmigaProfileFacts> = eligible
            .iter()
            .copied()
            .filter(|profile| profile.executable_found)
            .collect();
        checks.push(if !facts.discovery_available {
            check(
                K::Emulator,
                Blocked,
                "EmuWiz could not look for FS-UAE or Amiberry (no home folder is set).",
                Some(AmigaFixRoute::EmulatorSetup),
            )
        } else if facts.profiles.is_empty() {
            check(
                K::Emulator,
                Blocked,
                "No FS-UAE or Amiberry setup was found.",
                Some(AmigaFixRoute::EmulatorSetup),
            )
        } else if eligible.is_empty() {
            check(
                K::Emulator,
                Blocked,
                "FS-UAE or Amiberry settings were found, but none can be used safely.",
                Some(AmigaFixRoute::EmulatorSetup),
            )
        } else if runnable.is_empty() {
            check(
                K::Emulator,
                Blocked,
                format!(
                    "{} settings were found, but not the program itself.",
                    names(&eligible)
                ),
                Some(AmigaFixRoute::EmulatorSetup),
            )
        } else {
            check(
                K::Emulator,
                Ready,
                format!("{} found.", names(&runnable)),
                None,
            )
        });

        let suitable: Vec<&AmigaProfileFacts> = runnable
            .iter()
            .copied()
            .filter(|profile| profile.machine() == Some(machine))
            .filter(|profile| !needs_aga || profile.aga() == Some(true))
            .collect();
        let unnamed: Vec<&AmigaProfileFacts> = runnable
            .iter()
            .copied()
            .filter(|profile| {
                profile.machine().is_none()
                    || (profile.machine() == Some(machine) && needs_aga && profile.aga().is_none())
            })
            .collect();
        if !runnable.is_empty() {
            let want = if needs_aga {
                format!("{} with the AGA chipset", machine_label(machine))
            } else {
                machine_label(machine).to_string()
            };
            checks.push(if !suitable.is_empty() {
                check(
                    K::Machine,
                    Ready,
                    format!("{} is set up as {want}.", names(&suitable)),
                    None,
                )
            } else if !unnamed.is_empty() {
                check(
                    K::Machine,
                    Warning,
                    format!(
                        "{} does not say which Amiga model it emulates, so EmuWiz cannot confirm it suits {want}.",
                        names(&unnamed)
                    ),
                    Some(AmigaFixRoute::EmulatorSetup),
                )
            } else {
                let configured = runnable
                    .iter()
                    .map(|profile| {
                        format!(
                            "{} ({})",
                            emulator_label(profile.emulator),
                            profile.machine_model.as_deref().unwrap_or("model not named")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                check(
                    K::Machine,
                    Blocked,
                    format!("Wrong machine: this game needs {want}; set up is {configured}."),
                    Some(AmigaFixRoute::EmulatorSetup),
                )
            });

            let considered = if suitable.is_empty() {
                unnamed
            } else {
                suitable
            };
            checks.push(kickstart_check(machine, &considered));
        }
    }

    if let Some(disks) = disks.filter(|disks| disks.count > 1) {
        let (state, detail) = if disks.problems > 0 {
            (
                Blocked,
                format!(
                    "{} disks grouped; the set is incomplete or conflicting (see the disk list).",
                    disks.count
                ),
            )
        } else if disks.order_verified {
            (
                Ready,
                format!(
                    "{} disks, order verified. Change disks from the emulator's own menu (F12 in FS-UAE and Amiberry); EmuWiz does not swap disks for you.",
                    disks.count
                ),
            )
        } else {
            (
                Warning,
                format!(
                    "{} disks grouped by name only; the order is not verified.",
                    disks.count
                ),
            )
        };
        checks.push(check(
            K::Disks,
            state,
            detail,
            Some(AmigaFixRoute::MultiDisk),
        ));
    } else if media.is_floppy() {
        checks.push(check(
            K::Disks,
            Ready,
            "One disk entry. No other disks of this game were grouped with it.",
            None,
        ));
    }

    if media == AmigaMediaKind::WhdloadPackage {
        let (state, detail) = match facts.and_then(|facts| facts.whdload.as_ref()) {
            None => (Warning, "Reading the package…".to_string()),
            Some(WhdloadPackageFacts::BackendUnavailable) => (
                Warning,
                "The package could not be opened: no 7-Zip with LHA support was found.".into(),
            ),
            Some(WhdloadPackageFacts::Unreadable(detail)) => (
                Blocked,
                format!("The package could not be read (damaged, encrypted or not LHA): {detail}"),
            ),
            Some(WhdloadPackageFacts::Slaves(slaves)) => match slaves.as_slice() {
                [] => (
                    Blocked,
                    "No WHDLoad .Slave was found in the package.".into(),
                ),
                [slave] => {
                    let mut needs = Vec::new();
                    if slave.requires_aga {
                        needs.push("AGA chipset");
                    }
                    if slave.requires_68020 {
                        needs.push("68020 CPU");
                    }
                    let needs = if needs.is_empty() {
                        String::new()
                    } else {
                        format!(" It declares it needs: {}.", needs.join(", "))
                    };
                    (Ready, format!("Slave {} found.{needs}", slave.member_path))
                }
                many => (
                    Blocked,
                    format!(
                        "{} .Slave files were found; EmuWiz will not choose one for you.",
                        many.len()
                    ),
                ),
            },
        };
        checks.push(check(K::Whdload, state, detail, None));
    }

    match media {
        AmigaMediaKind::WhdloadPackage => checks.push(check(
            K::Trainers,
            Warning,
            "Trainer choices can be reviewed in Mods & Cheats where the Slave supports them. Saved choices are not passed to FS-UAE or Amiberry when the game starts.",
            Some(AmigaFixRoute::Trainers),
        )),
        AmigaMediaKind::Adf => checks.push(check(
            K::Trainers,
            Warning,
            "ADF trainer review in Mods & Cheats is a preview only; nothing is applied at launch.",
            Some(AmigaFixRoute::Trainers),
        )),
        _ => {}
    }

    let blocked = checks.iter().any(|check| check.state == Blocked);
    let adf_identity = facts.and_then(|facts| facts.adf_identity.as_ref());
    let verified_adf =
        media == AmigaMediaKind::Adf && !cd_machine && matches!(adf_identity, Some(Ok(_)));
    let launch_route_exists =
        (media == AmigaMediaKind::WhdloadPackage && !cd_machine) || verified_adf;
    let (state, detail) = match (media, adf_identity) {
        (AmigaMediaKind::Adf, Some(Err(reason))) if !cd_machine => (Blocked, reason.clone()),
        (AmigaMediaKind::Adf, None) if !cd_machine && facts.is_some() => (
            Blocked,
            "Not available: EmuWiz could not check whether a DAT verifies this ADF.".into(),
        ),
        (AmigaMediaKind::Adf, None) if !cd_machine => {
            (Warning, "Checking whether a DAT verifies this ADF…".into())
        }
        _ if !launch_route_exists => (
            // Launch authority for Amiga comes only from a verified WHDLoad
            // Slave or a DAT-proven single-disk ADF.
            Blocked,
            format!(
                "Not available yet: Blue cannot start {} because EmuWiz has no verified identity for them that launch could rely on. Only WHDLoad packages and DAT-verified single-disk ADFs can be started. EmuWiz will not start it with a guessed setup.",
                play_noun(machine, media)
            ),
        ),
        _ if blocked => (
            Blocked,
            "Play stays unavailable until the problems above are fixed.".into(),
        ),
        (_, Some(Ok(entry))) => (
            Ready,
            format!(
                "Verified by its DAT hash as {entry}. Play (FS-UAE) is offered in the launch card once its own checks pass."
            ),
        ),
        _ => (
            Ready,
            "Play is offered in the launch card once its own checks pass.".into(),
        ),
    };
    checks.push(check(K::Play, state, detail, None));

    AmigaGameReadiness {
        machine,
        media,
        disk_count: disks.map(|disks| disks.count),
        checks,
        launch_route_exists: launch_route_exists && !blocked,
    }
}

fn play_noun(machine: AmigaCdMachine, media: AmigaMediaKind) -> String {
    match (machine, media) {
        (AmigaCdMachine::Cd32 | AmigaCdMachine::Cdtv, _) => {
            format!("{} discs", machine_label(machine))
        }
        (_, AmigaMediaKind::Adf | AmigaMediaKind::Adz) => "Amiga floppy images directly".into(),
        (_, AmigaMediaKind::HardDisk) => "Amiga hard-disk images directly".into(),
        (_, AmigaMediaKind::Dms) => "DMS disk archives".into(),
        (_, AmigaMediaKind::Ipf) => "IPF images".into(),
        (_, AmigaMediaKind::Hfe) => "HFE images".into(),
        (_, AmigaMediaKind::CdImage) => "CD images".into(),
        (_, AmigaMediaKind::Container) => "packed archives".into(),
        (_, AmigaMediaKind::WhdloadPackage | AmigaMediaKind::Unsupported) => {
            "files like this".into()
        }
    }
}

fn media_check(machine: AmigaCdMachine, media: AmigaMediaKind) -> (AmigaCheckState, String) {
    use AmigaCheckState::{Blocked, Ready, Warning};
    let cd_machine = machine != AmigaCdMachine::OrdinaryAmiga;
    match media {
        AmigaMediaKind::CdImage if cd_machine => (
            Ready,
            format!("{} disc image. It is CD media, not a floppy.", machine_label(machine)),
        ),
        AmigaMediaKind::CdImage => (
            Blocked,
            "A CD image stored as an ordinary Amiga game. CD32 and CDTV discs are not floppies; check the platform.".into(),
        ),
        _ if cd_machine => (
            Blocked,
            format!(
                "{} games need CD media; a {} does not fit this machine.",
                machine_label(machine),
                media.label().to_ascii_lowercase()
            ),
        ),
        AmigaMediaKind::Adf => (Ready, "ADF floppy image.".into()),
        AmigaMediaKind::Adz => (Ready, "ADZ: an ADF floppy image compressed with gzip.".into()),
        AmigaMediaKind::Dms => (
            Warning,
            "DMS disk archive. EmuWiz cannot open DMS to check its contents.".into(),
        ),
        AmigaMediaKind::Ipf => (
            Warning,
            "IPF preservation image. Emulators need the CAPS/SPS library for IPF; EmuWiz cannot confirm it is installed.".into(),
        ),
        AmigaMediaKind::Hfe => (
            Blocked,
            "HFE is a Gotek/HxC floppy-emulator format. EmuWiz has no Amiga emulator route for it and will not convert it.".into(),
        ),
        AmigaMediaKind::HardDisk => (
            Warning,
            "Hard-disk image. It needs a hard-drive entry in the emulator's configuration; its contents are not proof of a complete game.".into(),
        ),
        AmigaMediaKind::WhdloadPackage => (
            Ready,
            "WHDLoad package: an installed game run from a virtual hard drive, not a set of floppy images.".into(),
        ),
        AmigaMediaKind::Container => (
            Blocked,
            "Packed inside a ZIP, 7z or RAR archive. EmuWiz does not extract Amiga media for play.".into(),
        ),
        AmigaMediaKind::Unsupported => (
            Blocked,
            "EmuWiz does not recognise this file as Amiga media it can explain.".into(),
        ),
    }
}

fn kickstart_check(machine: AmigaCdMachine, profiles: &[&AmigaProfileFacts]) -> AmigaCheck {
    use AmigaCheckState::{Blocked, Ready, Warning};
    let present: Vec<&AmigaProfileFacts> = profiles
        .iter()
        .copied()
        .filter(|profile| profile.kickstart == AmigaKickstartState::PresentUnverified)
        .collect();
    let cd_note = if machine == AmigaCdMachine::OrdinaryAmiga {
        ""
    } else {
        " The CD32/CDTV extended ROM is not checked here."
    };
    if !present.is_empty() {
        return check(
            AmigaCheckKind::Kickstart,
            if cd_note.is_empty() { Ready } else { Warning },
            format!(
                "Kickstart file set in {} (present, not hash-verified).{cd_note}",
                names(&present)
            ),
            (!cd_note.is_empty()).then_some(AmigaFixRoute::Firmware),
        );
    }
    if profiles.is_empty() {
        return check(
            AmigaCheckKind::Kickstart,
            Warning,
            "No suitable emulator setup to check Kickstart against.",
            Some(AmigaFixRoute::Firmware),
        );
    }
    let (state, detail) = if profiles
        .iter()
        .any(|profile| profile.kickstart == AmigaKickstartState::Unreadable)
    {
        (Blocked, "The configured Kickstart file cannot be read.")
    } else if profiles
        .iter()
        .any(|profile| profile.kickstart == AmigaKickstartState::Missing)
    {
        (Blocked, "The configured Kickstart file is missing.")
    } else if profiles
        .iter()
        .all(|profile| profile.kickstart == AmigaKickstartState::NotConfigured)
    {
        (
            Blocked,
            "No Kickstart ROM is configured. EmuWiz never supplies one.",
        )
    } else {
        (Warning, "Kickstart status could not be determined.")
    };
    check(
        AmigaCheckKind::Kickstart,
        state,
        format!("{detail}{cd_note}"),
        Some(AmigaFixRoute::Firmware),
    )
}

fn names(profiles: &[&AmigaProfileFacts]) -> String {
    let mut names: Vec<&str> = profiles
        .iter()
        .map(|profile| emulator_label(profile.emulator))
        .collect();
    names.sort_unstable();
    names.dedup();
    names.join(" and ")
}

#[cfg(test)]
#[path = "amiga_game_readiness_tests.rs"]
mod tests;
