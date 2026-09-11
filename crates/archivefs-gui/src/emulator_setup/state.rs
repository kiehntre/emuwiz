use crate::*;

pub(crate) enum RetroArchProfilesState {
    NotScanned,
    Scanning {
        receiver: Receiver<Result<RetroArchCheatSetupDiscovery, String>>,
    },
    Ready(RetroArchCheatSetupDiscovery),
    Error(String),
}

pub(crate) enum Pcsx2ProfilesState {
    NotScanned,
    Scanning {
        receiver: Receiver<Result<Pcsx2ProfileDiscovery, String>>,
    },
    Ready(Pcsx2ProfileDiscovery),
    Error(String),
}

pub(crate) enum DolphinProfilesState {
    NotScanned,
    Scanning {
        receiver: Receiver<Result<DolphinProfileDiscovery, String>>,
    },
    Ready(DolphinProfileDiscovery),
    Error(String),
}

/// Discovery for the modern [`archivefs_core::patch_manager::DolphinLocalProfile`]
/// model that [`archivefs_core::patch_manager::resolve_dolphin_native_launch_binding`]
/// consumes - distinct from [`DolphinProfilesState`]'s older
/// [`DolphinProfile`]/[`discover_dolphin_profiles`] pipeline (used by
/// Cheats & Mods), which cannot produce a launch binding. Launch Readiness
/// is the only reader of this state; scanning it never touches the Cheats &
/// Mods workflow.
pub(crate) struct DolphinLocalProfilesReady {
    pub(crate) discovery: archivefs_core::patch_manager::DolphinLocalProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::DolphinLocalDiscoveryRoots,
}

// One instance in `ArchiveFsApp`, only ever read by Launch Readiness. The
// larger `Ready` payload is the discovery result it must keep; the size gap
// vs the small variants is a non-issue for a single long-lived field.
#[allow(clippy::large_enum_variant)]
pub(crate) enum DolphinLocalProfilesState {
    NotScanned,
    Scanning {
        receiver: Receiver<Result<DolphinLocalProfilesReady, String>>,
    },
    Ready(DolphinLocalProfilesReady),
    // The detail is retained for parity with the other readiness states and
    // for a future error banner; Launch Readiness currently only checks
    // whether this is `Error(_)`, not what it says.
    Error(#[allow(dead_code)] String),
}

/// PCSX2 profile discovery, plus the roots it ran against, for Launch
/// Readiness's use only - see `pcsx2_launch_profiles`'s own doc comment for
/// why this is a separate scan from [`Pcsx2ProfilesState`] (used by Cheats
/// & Mods), which never retains its roots.
pub(crate) struct Pcsx2LaunchProfilesReady {
    pub(crate) discovery: archivefs_core::patch_manager::Pcsx2ProfileDiscovery,
    pub(crate) roots: archivefs_core::patch_manager::Pcsx2ProfileDiscoveryRoots,
}

// See `DolphinLocalProfilesState` - same single-long-lived-field rationale.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Pcsx2LaunchProfilesState {
    NotScanned,
    Scanning {
        receiver: Receiver<Result<Pcsx2LaunchProfilesReady, String>>,
    },
    Ready(Pcsx2LaunchProfilesReady),
    // See `DolphinLocalProfilesState::Error`.
    Error(#[allow(dead_code)] String),
}

pub(crate) struct FlycastProfilesReady {
    pub(crate) discovery: FlycastProfileDiscovery,
}

pub(crate) enum FlycastProfilesState {
    NotScanned,
    Scanning {
        receiver: Receiver<Result<FlycastProfilesReady, String>>,
    },
    Ready(FlycastProfilesReady),
    // See `DolphinLocalProfilesState::Error`.
    Error(#[allow(dead_code)] String),
}

/// PS2 firmware/BIOS evidence resolved from the user's registered DAT
/// sources - see [`pcsx2_firmware_evidence_from_registry`]. `Ready(vec![])`
/// is a genuine, honest outcome (nothing registered qualifies as Redump PS2
/// BIOS evidence yet), distinct from `Error`, which is reserved for an
/// actual registry read/parse failure - see
/// [`load_pcsx2_firmware_evidence_from_registry`].
pub(crate) enum Pcsx2FirmwareEvidenceState {
    NotLoaded,
    Loading {
        receiver: Receiver<
            Result<Vec<archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord>, String>,
        >,
    },
    Ready(Vec<archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord>),
    // Detail retained for a future error surface; readers currently only
    // test for `Error(_)`. See `DolphinLocalProfilesState::Error`.
    Error(#[allow(dead_code)] String),
}

/// Unlike Dolphin/PCSX2, Xenia Canary discovery only ever validates
/// caller-supplied explicit directories (no environment/HOME lookup, no
/// failure mode) - so it is synchronous and infallible; there is no
/// `Scanning` or `Error` state to represent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum XeniaProfilesState {
    NotScanned,
    Ready(XeniaProfileDiscovery),
}

/// Display label for a discovered profile's installation type.
pub(crate) fn profile_kind_label(kind: &ProfileKind) -> &'static str {
    match kind {
        ProfileKind::Native => "Native",
        ProfileKind::AppImage => "AppImage",
        ProfileKind::Flatpak => "Flatpak",
    }
}

/// Display label for a discovered profile's scope.
pub(crate) fn profile_scope_label(scope: &ProfileScope) -> &'static str {
    match scope {
        ProfileScope::User => "User",
        ProfileScope::System => "System",
    }
}

pub(crate) fn profile_presentation_tone(eligible: bool) -> widgets::StatusTone {
    if eligible {
        widgets::StatusTone::Success
    } else {
        widgets::StatusTone::Pending
    }
}

