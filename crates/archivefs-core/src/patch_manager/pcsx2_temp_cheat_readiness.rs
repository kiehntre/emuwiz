//! Read-only answer to: "can these PS2 cheats be enabled for this one launch
//! without permanently altering my PCSX2 configuration?"
//!
//! This is a *planner projection*. It consumes evidence EmuWiz already has
//! (verified game identity, a discovered PCSX2 profile, the read-only launch
//! binding result, a discovered local `.pnach` file and the user's `PCSX2.ini`
//! text) and returns a verdict with actionable reasons. It never reads or
//! writes the filesystem itself, never writes a PNACH file or `PCSX2.ini`,
//! never launches PCSX2 and never touches memory cards.
//!
//! # Why a launch capability is not a cheat capability
//!
//! `launch::pcsx2_execution` can start native, AppImage and Flatpak PCSX2.
//! That says nothing about where cheats would be read from: PCSX2 loads
//! `<serial>_<CRC>.pnach` from the `Cheats` folder of whichever data root it
//! resolved, and a *portable* marker beside the executable outranks every
//! command-line way of choosing another root (see
//! `docs/research/PCSX2_CHEAT_RUNTIME_FEASIBILITY.md`). So the only candidate
//! isolation mechanism is an explicit `-datapath` pointing at a per-launch
//! workspace, which this module treats as **unproven until runtime evidence
//! says otherwise**: [`Pcsx2TempCheatVerdict::SupportedIsolated`] is only
//! reachable when every required [`Pcsx2RuntimeProofItem`] has been supplied.
//! With no proof the best answer is
//! [`Pcsx2TempCheatVerdict::RequiresRuntimeProof`].
//!
//! # Remaining requirements for actual cheat-on-launch execution
//!
//! Recorded in [`Pcsx2TempCheatReadiness::staging_requirements`] and
//! [`Pcsx2TempCheatReadiness::runtime_proof_needed`] and summarised in
//! `docs/research/PCSX2_TEMP_CHEAT_LAUNCH_READINESS.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use super::local_cheat_install_pcsx2::{LocalPcsx2Discovery, LocalPcsx2FileError};
use super::pcsx2_identity::{Pcsx2GameIdentity, Pcsx2IdentityState, pcsx2_cheats_directory};
use super::pcsx2_local::{
    PCSX2_MAX_CONFIG_BYTES, Pcsx2InstallationType, Pcsx2LaunchBlocker, Pcsx2LaunchBlockerKind,
    Pcsx2NativeLaunchBinding, Pcsx2Profile, Pcsx2UserDirectoryMode,
};
use crate::launch::installation::LaunchInstallation;

/// `[Folders]` keys whose contents must stay in the user's real data root
/// when PCSX2 is pointed at a different one. Memory cards and save states are
/// user data: a redirected launch must never silently switch to empty ones.
pub const PCSX2_USER_DATA_FOLDER_KEYS: &[&str] = &[
    "Bios",
    "Snapshots",
    "Savestates",
    "MemoryCards",
    "Logs",
    "Cache",
    "Textures",
    "InputProfiles",
    "Videos",
];

/// How one `[Folders]` entry of `PCSX2.ini` is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2FolderEntry {
    /// Key absent or empty: PCSX2 uses its default folder under the data root.
    Unspecified,
    /// An absolute path: meaning is the same under any data root.
    Absolute(PathBuf),
    /// A relative path: its meaning depends on the data root, so a redirected
    /// root would resolve it somewhere else.
    Relative(String),
    /// Contains `..`, a NUL or otherwise cannot be reasoned about.
    Unsafe(String),
}

/// What EmuWiz knows about where PCSX2 will look for folders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2FolderRouting {
    /// `PCSX2.ini` was not available, too large, unreadable or not parseable.
    Unknown { reason: String },
    Known {
        cheats: Pcsx2FolderEntry,
        /// Entries for [`PCSX2_USER_DATA_FOLDER_KEYS`].
        user_data: BTreeMap<String, Pcsx2FolderEntry>,
    },
}

impl Pcsx2FolderRouting {
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self::Unknown {
            reason: reason.into(),
        }
    }

    /// Classifies the `[Folders]` section of `PCSX2.ini` text. A missing
    /// section is valid PCSX2 configuration (every folder is its default).
    pub fn from_ini_text(text: &str) -> Self {
        if text.len() > PCSX2_MAX_CONFIG_BYTES as usize {
            return Self::unknown("PCSX2.ini exceeds the configuration size limit");
        }
        if text.contains('\0') {
            return Self::unknown("PCSX2.ini contains a NUL byte");
        }
        let mut in_folders = false;
        let mut values: BTreeMap<String, String> = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[') {
                let Some(name) = section.strip_suffix(']') else {
                    return Self::unknown("PCSX2.ini has a malformed section header");
                };
                in_folders = name.trim() == "Folders";
                continue;
            }
            if in_folders && let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        let classify = |key: &str| match values.get(key).map(String::as_str) {
            None | Some("") => Pcsx2FolderEntry::Unspecified,
            Some(value) => classify_folder_value(value),
        };
        Self::Known {
            cheats: classify("Cheats"),
            user_data: PCSX2_USER_DATA_FOLDER_KEYS
                .iter()
                .map(|key| ((*key).to_string(), classify(key)))
                .collect(),
        }
    }
}

fn classify_folder_value(value: &str) -> Pcsx2FolderEntry {
    let path = Path::new(value);
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Pcsx2FolderEntry::Unsafe(value.to_string());
    }
    if path.is_absolute() {
        Pcsx2FolderEntry::Absolute(path.to_path_buf())
    } else {
        Pcsx2FolderEntry::Relative(value.to_string())
    }
}

/// Evidence that must exist, from an observed launch, before EmuWiz may call
/// a temporary-cheat launch supported. Nothing in this module produces these;
/// they are inputs from a future, separately reviewed probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2RuntimeProofItem {
    /// The installed PCSX2 version honoured an explicit `-datapath` and no
    /// marker, environment variable or setting overrode it.
    DataPathHonoured,
    /// PCSX2's own log showed it consuming the staged `.pnach`
    /// (`Found N cheats in <file>`) and enabling the chosen cheats.
    StagedCheatsConsumed,
    /// Real memory cards, save states and BIOS were read and written in the
    /// user's real folders, with before/after hashes.
    RealUserDataUntouched,
    /// The user's real `PCSX2.ini` and data root were byte-identical after the
    /// launch.
    RealConfigurationUnchanged,
    /// Widescreen / no-interlacing / patch switches behaved as in a normal
    /// launch of the same game.
    PatchBehaviourPreserved,
    /// (Flatpak only) the sandbox could see the staged data root through an
    /// explicit, per-launch filesystem grant.
    SandboxSawStagedRoot,
}

/// Runtime evidence supplied by the caller. [`Default`] is "nothing proven".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pcsx2RuntimeProof {
    pub proven: BTreeSet<Pcsx2RuntimeProofItem>,
}

/// Why a temporary-cheat launch is not available (or not yet claimable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2TempCheatBlockerKind {
    /// The game's serial/CRC are not verified (missing, deferred, ambiguous,
    /// unsupported image).
    IdentityNotVerified,
    /// Verified CRC exists but the PS2 serial does not; the launch adapter
    /// requires a verified serial.
    SerialMissing,
    /// The cheat material targets a different serial or CRC than the game.
    IdentityConflict,
    /// No cheat file was supplied or it could not be found.
    CheatMaterialMissing,
    /// The cheat file exists but is not usable for this game (malformed,
    /// wrong format, unsafe, too large, install-plan refusal).
    CheatMaterialIncompatible,
    /// A portable marker (or portable profile) decides the data root and
    /// outranks any launch-scoped redirection.
    PortableCannotBeRedirected,
    /// Installation type EmuWiz does not model for launch (NativeAlternate,
    /// an already explicit data path).
    UnsupportedInstallation,
    /// The launch binding could not be proven (ambiguous, missing or unsafe
    /// executable).
    LaunchBindingUnavailable,
    /// Profile/default root no longer matches discovery, or the profile is
    /// ineligible: where PCSX2 would look is unknown.
    ConfigurationUnknown,
    /// The profile has no usable `cheats` folder to compare against.
    CheatsFolderUnavailable,
    /// `PCSX2.ini` was not available or could not be classified.
    FolderRoutingUnknown,
    /// Folder entries are relative or contain `..`: a redirected root would
    /// resolve them elsewhere.
    FolderRoutingUnsafe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pcsx2TempCheatBlocker {
    pub kind: Pcsx2TempCheatBlockerKind,
    /// Plain-language reason shown to the user.
    pub reason: String,
    /// What the user (or a later EmuWiz stage) can do about it.
    pub action: String,
}

/// Things a future execution stage must do that this planner only records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2StagingRequirement {
    /// Launch with an explicit `-datapath <per-launch workspace>`.
    ExplicitDataPathArgument,
    /// Stage `<serial>_<CRC>.pnach` in the workspace `Cheats` folder, with
    /// `[EmuCore] EnableCheats` and `[Cheats] Enable = <chosen names>` in the
    /// workspace configuration (cheats are not patches).
    StageCheatFileAndEnableList { file_name: String },
    /// Seed the workspace `PCSX2.ini` from the user's, rewriting nothing that
    /// changes patch or widescreen behaviour.
    SeedConfigurationFromUserIni,
    /// This user-data folder is unspecified (default under the real data
    /// root); pin it to the real absolute default so the redirected launch
    /// does not see an empty one.
    PinUserFolderToRealDefault { key: String },
    /// Copy this game's per-game settings into the workspace.
    CarryPerGameSettings,
    /// Grant the Flatpak sandbox access to exactly the workspace for this
    /// launch (see `launch::installation::VisibilityPlan`).
    FlatpakWorkspaceGrant { app_id: String },
    /// Delete the workspace after the launch; never reuse it.
    DiscardWorkspaceAfterLaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pcsx2TempCheatVerdict {
    /// Static preconditions hold and every required runtime proof item was
    /// supplied.
    SupportedIsolated,
    /// Static preconditions hold, but isolation has not been proven by
    /// observation. EmuWiz must not enable cheats for a launch on this basis.
    RequiresRuntimeProof,
    /// At least one blocker; see [`Pcsx2TempCheatReadiness::blockers`].
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pcsx2TempCheatReadiness {
    pub verdict: Pcsx2TempCheatVerdict,
    pub blockers: Vec<Pcsx2TempCheatBlocker>,
    /// Proof items still missing. Empty only for `SupportedIsolated`.
    pub runtime_proof_needed: Vec<Pcsx2RuntimeProofItem>,
    /// Empty when blocked.
    pub staging_requirements: Vec<Pcsx2StagingRequirement>,
}

impl Pcsx2TempCheatReadiness {
    pub fn is_supported(&self) -> bool {
        self.verdict == Pcsx2TempCheatVerdict::SupportedIsolated
    }
}

/// Everything the planner looks at. All references; nothing is read here.
pub struct Pcsx2TempCheatInput<'a> {
    pub identity: &'a Pcsx2GameIdentity,
    pub profile: &'a Pcsx2Profile,
    /// Result of the existing read-only
    /// [`super::resolve_pcsx2_native_launch_binding`] for `profile`.
    pub binding: Result<&'a Pcsx2NativeLaunchBinding, &'a Pcsx2LaunchBlocker>,
    /// Result of the existing
    /// [`super::discover_local_pcsx2_pnach_file`] for `identity`, or `None`
    /// when no cheat file was chosen.
    pub cheat_material: Option<Result<&'a LocalPcsx2Discovery, &'a LocalPcsx2FileError>>,
    pub folder_routing: &'a Pcsx2FolderRouting,
    pub runtime_proof: &'a Pcsx2RuntimeProof,
}

fn blocker(
    kind: Pcsx2TempCheatBlockerKind,
    reason: impl Into<String>,
    action: impl Into<String>,
) -> Pcsx2TempCheatBlocker {
    Pcsx2TempCheatBlocker {
        kind,
        reason: reason.into(),
        action: action.into(),
    }
}

/// Builds the readiness projection. Pure: same input, same answer.
pub fn plan_pcsx2_temporary_cheat_launch(
    input: &Pcsx2TempCheatInput<'_>,
) -> Pcsx2TempCheatReadiness {
    let mut blockers = Vec::new();
    check_identity(input.identity, &mut blockers);
    check_cheat_material(input, &mut blockers);
    let installation = check_installation(input, &mut blockers);
    check_routing(input, &mut blockers);

    if !blockers.is_empty() {
        blockers.sort_by_key(|item| item.kind);
        return Pcsx2TempCheatReadiness {
            verdict: Pcsx2TempCheatVerdict::Blocked,
            blockers,
            runtime_proof_needed: Vec::new(),
            staging_requirements: Vec::new(),
        };
    }

    let mut staging = vec![
        Pcsx2StagingRequirement::ExplicitDataPathArgument,
        Pcsx2StagingRequirement::SeedConfigurationFromUserIni,
        Pcsx2StagingRequirement::CarryPerGameSettings,
    ];
    if let Some(Ok(material)) = input.cheat_material {
        staging.push(Pcsx2StagingRequirement::StageCheatFileAndEnableList {
            file_name: staged_file_name(material),
        });
    }
    if let Pcsx2FolderRouting::Known { user_data, .. } = input.folder_routing {
        for (key, entry) in user_data {
            if *entry == Pcsx2FolderEntry::Unspecified {
                staging
                    .push(Pcsx2StagingRequirement::PinUserFolderToRealDefault { key: key.clone() });
            }
        }
    }
    let mut required = vec![
        Pcsx2RuntimeProofItem::DataPathHonoured,
        Pcsx2RuntimeProofItem::StagedCheatsConsumed,
        Pcsx2RuntimeProofItem::RealUserDataUntouched,
        Pcsx2RuntimeProofItem::RealConfigurationUnchanged,
        Pcsx2RuntimeProofItem::PatchBehaviourPreserved,
    ];
    if let LaunchInstallation::Flatpak { app_id } = &installation {
        staging.push(Pcsx2StagingRequirement::FlatpakWorkspaceGrant {
            app_id: app_id.clone(),
        });
        required.push(Pcsx2RuntimeProofItem::SandboxSawStagedRoot);
    }
    staging.push(Pcsx2StagingRequirement::DiscardWorkspaceAfterLaunch);

    let missing: Vec<_> = required
        .into_iter()
        .filter(|item| !input.runtime_proof.proven.contains(item))
        .collect();
    Pcsx2TempCheatReadiness {
        verdict: if missing.is_empty() {
            Pcsx2TempCheatVerdict::SupportedIsolated
        } else {
            Pcsx2TempCheatVerdict::RequiresRuntimeProof
        },
        blockers,
        runtime_proof_needed: missing,
        staging_requirements: staging,
    }
}

fn staged_file_name(material: &LocalPcsx2Discovery) -> String {
    match &material.detected_serial {
        Some(serial) => format!("{serial}_{}.pnach", material.detected_crc),
        None => format!("{}.pnach", material.detected_crc),
    }
}

fn check_identity(identity: &Pcsx2GameIdentity, blockers: &mut Vec<Pcsx2TempCheatBlocker>) {
    if identity.state != Pcsx2IdentityState::Verified {
        let reason = identity
            .plain_failure_reason
            .clone()
            .unwrap_or_else(|| "EmuWiz has not verified this game's PCSX2 identity.".to_string());
        blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::IdentityNotVerified,
            reason,
            "Select the game image so EmuWiz can verify its serial and executable CRC.",
        ));
        return;
    }
    if identity.serial.is_none() {
        blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::SerialMissing,
            "The executable CRC is verified but the PS2 serial is not; EmuWiz will not start \
             PCSX2 without a verified serial.",
            "Use a game image from which EmuWiz can read SYSTEM.CNF.",
        ));
    }
}

fn check_cheat_material(
    input: &Pcsx2TempCheatInput<'_>,
    blockers: &mut Vec<Pcsx2TempCheatBlocker>,
) {
    match input.cheat_material {
        None => blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::CheatMaterialMissing,
            "No PNACH cheat file has been chosen for this game.",
            "Choose a .pnach file named <SERIAL>_<CRC>.pnach or <CRC>.pnach.",
        )),
        Some(Err(LocalPcsx2FileError::NotFound { .. })) => blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::CheatMaterialMissing,
            "The chosen PNACH cheat file could not be found.",
            "Choose an existing .pnach file.",
        )),
        Some(Err(LocalPcsx2FileError::IdentityConflict { detail })) => blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::IdentityConflict,
            detail.clone(),
            "Choose cheats written for this exact game serial and CRC.",
        )),
        Some(Err(LocalPcsx2FileError::IdentityUnresolved { detail })) => blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::IdentityNotVerified,
            detail.clone(),
            "Verify the game's executable CRC first.",
        )),
        Some(Err(other)) => blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::CheatMaterialIncompatible,
            other.to_string(),
            "Choose a plain, valid .pnach file within the size limit.",
        )),
        Some(Ok(material)) => {
            if material.cheat.patch_lines.is_empty() {
                blockers.push(blocker(
                    Pcsx2TempCheatBlockerKind::CheatMaterialIncompatible,
                    "The PNACH file contains no patch lines.",
                    "Choose a cheat file that contains cheats.",
                ));
            }
            // Defensive re-check: the discovery result may have been made
            // against another identity object.
            if let Some(crc) = input.identity.verified_crc()
                && !material.detected_crc.eq_ignore_ascii_case(crc)
            {
                blockers.push(blocker(
                    Pcsx2TempCheatBlockerKind::IdentityConflict,
                    format!(
                        "The cheat file targets CRC {}, not this game's verified CRC {crc}.",
                        material.detected_crc
                    ),
                    "Choose cheats written for this exact game.",
                ));
            }
            if let (Some(file), Some(game)) = (&material.detected_serial, &input.identity.serial)
                && !file.eq_ignore_ascii_case(game)
            {
                blockers.push(blocker(
                    Pcsx2TempCheatBlockerKind::IdentityConflict,
                    format!("The cheat file targets serial {file}, not this game's serial {game}."),
                    "Choose cheats written for this exact game.",
                ));
            }
        }
    }
}

/// Returns the installation shape when the binding is usable, so staging
/// requirements can depend on it.
fn check_installation(
    input: &Pcsx2TempCheatInput<'_>,
    blockers: &mut Vec<Pcsx2TempCheatBlocker>,
) -> LaunchInstallation {
    let portable_reason = "This PCSX2 is portable: a portable marker beside the program makes its \
        own folder the data root and overrides any per-launch data path, so cheats could only be \
        enabled by changing its real cheats folder.";
    let portable_action = "Use a non-portable PCSX2 installation, or add the cheats to this \
        installation yourself.";
    if input.profile.installation_type == Pcsx2InstallationType::Portable {
        blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::PortableCannotBeRedirected,
            portable_reason,
            portable_action,
        ));
        return LaunchInstallation::Native;
    }
    if !input.profile.eligible {
        blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::ConfigurationUnknown,
            "The selected PCSX2 profile is not eligible, so EmuWiz does not know which \
             configuration a launch would use.",
            "Re-run PCSX2 discovery and choose an eligible profile.",
        ));
    }
    match input.binding {
        Ok(binding) => {
            match &binding.user_directory_mode {
                Pcsx2UserDirectoryMode::PortableBesideExecutable => blockers.push(blocker(
                    Pcsx2TempCheatBlockerKind::PortableCannotBeRedirected,
                    portable_reason,
                    portable_action,
                )),
                Pcsx2UserDirectoryMode::ExplicitDataPath(path) => blockers.push(blocker(
                    Pcsx2TempCheatBlockerKind::UnsupportedInstallation,
                    format!(
                        "This installation already launches with the explicit data path {}; \
                         replacing it per launch is not modelled.",
                        path.display()
                    ),
                    "Use a PCSX2 installation that resolves its own default data folder.",
                )),
                Pcsx2UserDirectoryMode::DefaultNative | Pcsx2UserDirectoryMode::FlatpakSandbox => {}
            }
            binding.installation.clone()
        }
        Err(launch_blocker) => {
            blockers.push(binding_blocker(launch_blocker));
            LaunchInstallation::Native
        }
    }
}

fn binding_blocker(launch_blocker: &Pcsx2LaunchBlocker) -> Pcsx2TempCheatBlocker {
    let detail = &launch_blocker.detail;
    match launch_blocker.kind {
        Pcsx2LaunchBlockerKind::PortableMarkerConflict => blocker(
            Pcsx2TempCheatBlockerKind::PortableCannotBeRedirected,
            format!(
                "A portable marker sits beside PCSX2 and would override a per-launch data path: {detail}"
            ),
            "Use a non-portable PCSX2 installation, or add the cheats to this installation yourself.",
        ),
        Pcsx2LaunchBlockerKind::UnsupportedInstallationType => blocker(
            Pcsx2TempCheatBlockerKind::UnsupportedInstallation,
            format!("EmuWiz does not launch this kind of PCSX2 installation: {detail}"),
            "Use a native, AppImage or Flatpak PCSX2 installation.",
        ),
        Pcsx2LaunchBlockerKind::AmbiguousExecutable
        | Pcsx2LaunchBlockerKind::ExecutableMissing
        | Pcsx2LaunchBlockerKind::ExecutableUnsafe
        | Pcsx2LaunchBlockerKind::ExecutableNotExecutable => blocker(
            Pcsx2TempCheatBlockerKind::LaunchBindingUnavailable,
            format!("EmuWiz cannot prove which PCSX2 program to start: {detail}"),
            "Resolve the PCSX2 executable problem shown in the launch readiness.",
        ),
        Pcsx2LaunchBlockerKind::ProfileRootMismatch
        | Pcsx2LaunchBlockerKind::DefaultResolutionMismatch => blocker(
            Pcsx2TempCheatBlockerKind::ConfigurationUnknown,
            format!("PCSX2's configuration location is not proven: {detail}"),
            "Re-run PCSX2 discovery; EmuWiz will not guess which folder PCSX2 would use.",
        ),
    }
}

fn check_routing(input: &Pcsx2TempCheatInput<'_>, blockers: &mut Vec<Pcsx2TempCheatBlocker>) {
    if pcsx2_cheats_directory(input.profile).is_none() {
        blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::CheatsFolderUnavailable,
            "EmuWiz found no usable cheats folder for this PCSX2 profile, so it cannot tell \
             where PCSX2 would read cheats from.",
            "Re-run PCSX2 discovery, or start PCSX2 once so it creates its folders.",
        ));
    }
    match input.folder_routing {
        Pcsx2FolderRouting::Unknown { reason } => blockers.push(blocker(
            Pcsx2TempCheatBlockerKind::FolderRoutingUnknown,
            format!("PCSX2's folder routing is unknown: {reason}"),
            "Make PCSX2.ini readable so EmuWiz can classify its folder entries.",
        )),
        Pcsx2FolderRouting::Known { cheats, user_data } => {
            let mut unsafe_keys: Vec<String> = Vec::new();
            let mut note = |key: &str, entry: &Pcsx2FolderEntry| match entry {
                Pcsx2FolderEntry::Relative(value) => {
                    unsafe_keys.push(format!("{key} (relative: {value})"));
                }
                Pcsx2FolderEntry::Unsafe(value) => {
                    unsafe_keys.push(format!("{key} (unsafe: {value})"));
                }
                Pcsx2FolderEntry::Unspecified | Pcsx2FolderEntry::Absolute(_) => {}
            };
            note("Cheats", cheats);
            for (key, entry) in user_data {
                note(key, entry);
            }
            if !unsafe_keys.is_empty() {
                blockers.push(blocker(
                    Pcsx2TempCheatBlockerKind::FolderRoutingUnsafe,
                    format!(
                        "These PCSX2.ini folders would resolve differently under a per-launch \
                         data root: {}.",
                        unsafe_keys.join(", ")
                    ),
                    "Change them to absolute paths in PCSX2.ini, then check again.",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests;
