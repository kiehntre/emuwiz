//! Read-only writability assessment for discovered emulator profiles.
//!
//! Doctor already knows which profiles exist: `discover_dolphin_profiles`,
//! `discover_pcsx2_profiles`, `discover_xenia_profiles`,
//! `discover_ppsspp_profiles` and `discover_duckstation_profiles` do that,
//! and each already reports its own blockers. This module answers the one
//! question none of them answers: **could EmuWiz write an install into that
//! profile?**
//!
//! It answers it from metadata only. Nothing here creates a directory, a
//! `GameSettings` file, a `.pnach`, a `.patch.toml`, a config file or a probe
//! file, and nothing changes a permission bit. The strongest verdict available
//! is therefore [`WritabilityAssessment::AppearsWritable`] - see that type for
//! why "writable: yes" is never claimed.
//!
//! ## Scope
//!
//! Only the adapters EmuWiz already supports, through their existing
//! discovery abstractions. No emulator gains support here. Flatpak profiles
//! are assessed exactly like native ones, which is honest but incomplete: a
//! portal can still refuse a write that the bits allow, so the narrowed
//! deferred entry in `DEFERRED_CHECKS` says so.
//!
//! PPSSPP and DuckStation are assessed the same read-only way as every other
//! adapter here, but - like Xenia - never become
//! [`managed_scan_targets`]: neither has an in-file EmuWiz ownership marker
//! (a managed block/section) to scan for, since neither has install/cheat-
//! write support yet.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::environment::{
    MountEntry, MountMode, PathPermissions, WritabilityAssessment, assess_permissions,
    assess_writability, mount_entry_for_path,
};
use super::managed::{ManagedFormat, ManagedScanTarget};
use super::{
    DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding, Measurement, NotCheckedCheck,
};
use crate::emulator_environment::EncodedPath;
use crate::launch::readiness::{FirmwareReadiness, rpcs3_firmware_readiness};
use crate::patch_manager::{
    DolphinProfileDiscovery, DolphinProfileDiscoveryRoots, DuckStationProfileDiscovery,
    DuckStationProfileDiscoveryRoots, EmulatorProfileSelection, Pcsx2ProfileDiscovery,
    Pcsx2ProfileDiscoveryRoots, PpssppLaunchBlocker, PpssppLaunchBlockerKind,
    PpssppProfileDiscovery, PpssppProfileDiscoveryRoots, Rpcs3GameRequest, Rpcs3LaunchBlocker,
    Rpcs3LaunchBlockerKind, Rpcs3ProfileDiscovery, Rpcs3ProfileDiscoveryRoots, XemuGameRequest,
    XemuLaunchBlocker, XemuLaunchBlockerKind, XemuProfileDiscovery, XemuProfileDiscoveryRoots,
    XemuSystemFileState, XeniaLaunchBlocker, XeniaLaunchBlockerKind, XeniaProfileDiscovery,
    XeniaProfileDiscoveryRoots, discover_dolphin_profiles, discover_duckstation_profiles,
    discover_pcsx2_profiles, discover_ppsspp_profiles, discover_rpcs3_profiles,
    discover_xemu_profiles, discover_xenia_profiles, inspect_rpcs3_game, inspect_xemu_game,
    resolve_ppsspp_native_launch_binding, resolve_rpcs3_native_launch_binding,
    resolve_xemu_native_launch_binding, resolve_xenia_launch_binding, select_dolphin_profile,
};

/// At most this many profiles are reported individually; beyond it Doctor
/// summarises. A machine with dozens of Flatpak and portable installs must not
/// bury every other result.
pub const MAX_INDIVIDUAL_PROFILE_FINDINGS: usize = 12;

/// Which emulator a profile belongs to. Only adapters EmuWiz already
/// supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmulatorKind {
    Dolphin,
    Pcsx2,
    Xenia,
    Ppsspp,
    DuckStation,
    Xemu,
    Rpcs3,
}

impl EmulatorKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dolphin => "Dolphin",
            Self::Pcsx2 => "PCSX2",
            Self::Xenia => "Xenia Canary",
            Self::Ppsspp => "PPSSPP",
            Self::DuckStation => "DuckStation",
            Self::Xemu => "xemu",
            Self::Rpcs3 => "RPCS3",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Dolphin => "dolphin",
            Self::Pcsx2 => "pcsx2",
            Self::Xenia => "xenia",
            Self::Ppsspp => "ppsspp",
            Self::DuckStation => "duckstation",
            Self::Xemu => "xemu",
            Self::Rpcs3 => "rpcs3",
        }
    }

    /// The ambiguity finding is per emulator. A shared id would let two
    /// emulators' results merge into one, because neither names a single
    /// affected path - which is exactly what a live scan showed happening.
    fn ambiguous_profile_finding_id(self) -> &'static str {
        match self {
            Self::Dolphin => "emulator_profile.ambiguous_preferred_dolphin_profile",
            Self::Pcsx2 => "emulator_profile.ambiguous_preferred_pcsx2_profile",
            Self::Xenia => "emulator_profile.ambiguous_preferred_xenia_profile",
            Self::Ppsspp => "emulator_profile.ambiguous_preferred_ppsspp_profile",
            Self::DuckStation => "emulator_profile.ambiguous_preferred_duckstation_profile",
            Self::Xemu => "emulator_profile.ambiguous_preferred_xemu_profile",
            Self::Rpcs3 => "emulator_profile.ambiguous_preferred_rpcs3_profile",
        }
    }
}

/// One profile's assessment. Everything the GUI and the CLI need, with the
/// evidence that produced each conclusion.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileAssessment {
    pub emulator: EmulatorKind,
    pub profile_id: String,
    /// The adapter's own installation-type label - native, Flatpak, AppImage,
    /// portable - taken verbatim rather than reclassified here.
    pub profile_kind: String,
    pub scope: String,
    /// The adapter's own provenance string: how this profile was discovered.
    pub discovery_confidence: String,
    /// Whether the adapter itself considers this profile usable.
    pub eligible: bool,
    /// The adapter's own blockers, if any.
    pub blockers: Vec<String>,
    pub root_path: EncodedPath,
    /// The one directory EmuWiz would write into for this adapter.
    pub destination_path: EncodedPath,
    pub destination_exists: bool,
    pub destination_is_directory: bool,
    pub destination_is_symlink: bool,
    pub mount_mode: MountMode,
    pub permissions: Option<PathPermissions>,
    pub writability: WritabilityAssessment,
    /// Set when EmuWiz already knows this is the selected/remembered
    /// profile for this adapter.
    pub preferred: Option<bool>,
}

impl ProfileAssessment {
    fn finding_id(&self) -> &'static str {
        match self.writability {
            WritabilityAssessment::MissingDestination => "emulator_profile.missing_destination",
            WritabilityAssessment::ReadOnlyFilesystem => "emulator_profile.read_only_filesystem",
            WritabilityAssessment::PermissionDenied => "emulator_profile.permission_denied",
            WritabilityAssessment::UnsafeDestination => "emulator_profile.unsafe_destination",
            WritabilityAssessment::NotProven => "emulator_profile.writability_not_proven",
            WritabilityAssessment::AppearsWritable => "emulator_profile.appears_writable",
        }
    }

    fn severity(&self) -> Option<DoctorSeverity> {
        match self.writability {
            // Nothing to report: this is the ordinary healthy case.
            WritabilityAssessment::AppearsWritable => None,
            // EmuWiz creates the destination during install, so its absence
            // is informational, not a fault.
            WritabilityAssessment::MissingDestination => Some(DoctorSeverity::Info),
            // An install into this profile would fail.
            WritabilityAssessment::ReadOnlyFilesystem
            | WritabilityAssessment::PermissionDenied
            | WritabilityAssessment::UnsafeDestination => Some(DoctorSeverity::Warning),
            // Honest uncertainty, not a problem.
            WritabilityAssessment::NotProven => Some(DoctorSeverity::Info),
        }
    }

    /// PPSSPP and DuckStation presently contribute profile inspection only:
    /// EmuWiz knows their cheat directories, but has no install or managed
    /// block support for either adapter. Keep their discovered, healthy
    /// profiles visible in Doctor instead of treating them as a silent pass.
    fn is_inspection_only_profile(&self) -> bool {
        matches!(
            self.emulator,
            EmulatorKind::Ppsspp | EmulatorKind::DuckStation
        )
    }

    fn inspection_finding_id(&self) -> &'static str {
        match self.emulator {
            EmulatorKind::Ppsspp => "emulator_profile.ppsspp_inspected",
            EmulatorKind::DuckStation => "emulator_profile.duckstation_inspected",
            EmulatorKind::Dolphin
            | EmulatorKind::Pcsx2
            | EmulatorKind::Xenia
            | EmulatorKind::Xemu
            | EmulatorKind::Rpcs3 => {
                unreachable!("only inspection-only profiles have inspection finding ids")
            }
        }
    }

    fn destination_label(&self) -> &'static str {
        if self.is_inspection_only_profile() {
            "Cheat destination"
        } else {
            "Destination"
        }
    }

    fn evidence(&self) -> Vec<String> {
        let mut evidence = vec![
            format!("Emulator: {}", self.emulator.label()),
            format!("Profile: {}", self.profile_id),
            format!("Profile type: {}", self.profile_kind),
            format!("Scope: {}", self.scope),
            format!("How it was found: {}", self.discovery_confidence),
            format!("Configuration path: {}", self.root_path.display),
            format!(
                "{}: {}",
                self.destination_label(),
                self.destination_path.display
            ),
            format!("Destination exists: {}", self.destination_exists),
            format!("Mount state: {}", self.mount_mode.label()),
            format!("Assessment: {}", self.writability.label()),
        ];
        if let Some(preferred) = self.preferred {
            evidence.push(format!("Currently selected profile: {preferred}"));
        }
        evidence.push(format!("Adapter considers it usable: {}", self.eligible));
        match &self.permissions {
            Some(permissions) => {
                if let Some(mode) = permissions.mode {
                    evidence.push(format!("Permissions: {:o}", mode & 0o7777));
                }
                if let Some(owned) = permissions.owned_by_current_user {
                    evidence.push(format!("Owned by the current user: {owned}"));
                }
                if let Some(may_write) = permissions.current_user_may_write {
                    evidence.push(format!(
                        "Permission bits allow this user to write: {may_write}"
                    ));
                }
            }
            None => evidence.push(
                "Permissions: not readable, so they were not used in this assessment".to_string(),
            ),
        }
        for blocker in &self.blockers {
            evidence.push(format!("Adapter blocker: {blocker}"));
        }
        evidence
    }
}

fn profile_finding(profile: &ProfileAssessment, severity: DoctorSeverity) -> Finding {
    let inspection_only = profile.is_inspection_only_profile();
    let (id, title, explanation) = if inspection_only {
        (
            profile.inspection_finding_id(),
            format!(
                "{} profile inspected: {}",
                profile.emulator.label(),
                profile.writability.label()
            ),
            format!(
                "{} profile {} uses {} as its cheat destination. {}",
                profile.emulator.label(),
                profile.profile_id,
                profile.destination_path.display,
                profile.writability.label(),
            ),
        )
    } else {
        (
            profile.finding_id(),
            format!(
                "{} profile: {}",
                profile.emulator.label(),
                profile.writability.label()
            ),
            format!(
                "{} ({}) would install into {}. {}",
                profile.emulator.label(),
                profile.profile_kind,
                profile.destination_path.display,
                profile.writability.label()
            ),
        )
    };
    let (why_it_matters, next_step) = if inspection_only {
        (
            "EmuWiz currently inspects this profile and its cheat destination only; it does not install PPSSPP or DuckStation cheats or manage a block in either emulator's files.",
            "No action is available in EmuWiz for this profile yet. The assessment is read-only and reports the destination metadata without writing a probe file.",
        )
    } else {
        match profile.writability {
            WritabilityAssessment::ReadOnlyFilesystem => (
                "Installing a cheat or patch into this profile would fail, because the filesystem itself is read-only.",
                "Remount that filesystem read-write, or pick a different profile before installing.",
            ),
            WritabilityAssessment::PermissionDenied => (
                "Installing a cheat or patch into this profile would fail with a permission error.",
                "Give the current user write access to that directory, or pick a different profile.",
            ),
            WritabilityAssessment::MissingDestination => (
                "EmuWiz creates this directory during an install, so this is only worth knowing about in advance.",
                "Nothing to do now. Launch the emulator once, or let EmuWiz create it during an install.",
            ),
            WritabilityAssessment::UnsafeDestination => (
                "EmuWiz refuses to write through a symlink or into a non-directory, so this profile cannot be used as-is.",
                "Replace the symlink with a real directory, or point EmuWiz at a different profile.",
            ),
            WritabilityAssessment::NotProven | WritabilityAssessment::AppearsWritable => (
                "EmuWiz cannot confirm from metadata alone whether a write here would succeed, and will not write a test file to find out.",
                "No action needed. An install will report the real outcome.",
            ),
        }
    };

    Finding::new(
        id,
        DoctorCategory::EmulatorProfiles,
        DoctorSubsystem::EmulatorProfiles,
        severity,
        title,
        explanation,
    )
    .with_affected(profile.destination_path.clone())
    .with_evidence(profile.evidence())
    .with_measurements([
        ("emulator", Measurement::text(profile.emulator.label())),
        ("profile", Measurement::text(&profile.profile_id)),
        ("profile_kind", Measurement::text(&profile.profile_kind)),
        (
            "configuration_path",
            Measurement::text(&profile.root_path.display),
        ),
        (
            "cheat_destination",
            Measurement::text(&profile.destination_path.display),
        ),
        (
            "discovery_confidence",
            Measurement::text(&profile.discovery_confidence),
        ),
        (
            "writability_assessment",
            Measurement::text(profile.writability.label()),
        ),
        (
            "filesystem_read_only",
            Measurement::Flag(profile.writability == WritabilityAssessment::ReadOnlyFilesystem),
        ),
        (
            "destination_exists",
            Measurement::Flag(profile.destination_exists),
        ),
        ("eligible", Measurement::Flag(profile.eligible)),
    ])
    .with_guidance(why_it_matters, next_step)
}

/// Everything Doctor could work out about emulator profiles.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileAssessmentReport {
    pub profiles: Vec<ProfileAssessment>,
    /// Adapters whose discovery failed or was not run, and why. Never
    /// presented as "no profiles".
    pub unavailable: Vec<(EmulatorKind, String)>,
    /// `true` when an adapter reported its own discovery as incomplete.
    pub discovery_incomplete: bool,
}

/// The already-completed discoveries Doctor assesses. Borrowed, and each
/// optional, so one adapter failing never stops the others.
#[derive(Debug, Default)]
pub struct ProfileDiscoveries<'a> {
    pub dolphin: Option<&'a DolphinProfileDiscovery>,
    pub dolphin_error: Option<String>,
    pub pcsx2: Option<&'a Pcsx2ProfileDiscovery>,
    pub pcsx2_error: Option<String>,
    pub xenia: Option<&'a XeniaProfileDiscovery>,
    pub xenia_error: Option<String>,
    pub ppsspp: Option<&'a PpssppProfileDiscovery>,
    pub ppsspp_error: Option<String>,
    pub duckstation: Option<&'a DuckStationProfileDiscovery>,
    pub duckstation_error: Option<String>,
    pub xemu: Option<&'a XemuProfileDiscovery>,
    pub xemu_error: Option<String>,
    pub rpcs3: Option<&'a Rpcs3ProfileDiscovery>,
    pub rpcs3_error: Option<String>,
    /// The profile ids EmuWiz currently prefers, when known.
    pub preferred_dolphin: Option<&'a str>,
    pub preferred_pcsx2: Option<&'a str>,
    pub preferred_xenia: Option<&'a str>,
    pub preferred_ppsspp: Option<&'a str>,
    pub preferred_duckstation: Option<&'a str>,
    pub preferred_xemu: Option<&'a str>,
    pub preferred_rpcs3: Option<&'a str>,
}

/// Assesses every discovered profile's write destination.
///
/// Read-only: `symlink_metadata` for existence and permissions, one already
/// read mount table for mount mode. Nothing is created and no permission is
/// changed.
pub fn assess_emulator_profiles(
    discoveries: &ProfileDiscoveries<'_>,
    mount_table: Option<&[MountEntry]>,
) -> ProfileAssessmentReport {
    let mut profiles = Vec::new();
    let mut unavailable = Vec::new();
    let mut discovery_incomplete = false;

    if let Some(discovery) = discoveries.dolphin {
        discovery_incomplete |= !discovery.complete;
        for profile in &discovery.profiles {
            profiles.push(assess_one(
                EmulatorKind::Dolphin,
                profile.profile_id.clone(),
                format!("{:?}", profile.installation_type),
                format!("{:?}", profile.scope),
                profile.provenance.clone(),
                profile.eligible,
                profile
                    .blockers
                    .iter()
                    .map(|blocker| format!("{:?}: {}", blocker.kind, blocker.detail))
                    .collect(),
                &profile.configuration_path,
                // The one directory EmuWiz writes into for Dolphin.
                &profile.game_settings_path,
                discoveries.preferred_dolphin,
                mount_table,
            ));
        }
    } else if let Some(error) = &discoveries.dolphin_error {
        unavailable.push((EmulatorKind::Dolphin, error.clone()));
    }

    if let Some(discovery) = discoveries.pcsx2 {
        discovery_incomplete |= !discovery.complete;
        for profile in &discovery.profiles {
            // PCSX2 can expose several patch directories; assess each, since
            // an install can target any of them.
            for directory in &profile.patch_directories {
                profiles.push(assess_one(
                    EmulatorKind::Pcsx2,
                    profile.profile_id.clone(),
                    format!("{:?}", profile.installation_type),
                    format!("{:?}", profile.scope),
                    profile.provenance.to_string(),
                    profile.eligible,
                    profile
                        .blockers
                        .iter()
                        .map(|blocker| format!("{:?}: {}", blocker.kind, blocker.detail))
                        .collect(),
                    &profile.configuration_path,
                    &directory.path,
                    discoveries.preferred_pcsx2,
                    mount_table,
                ));
            }
            if profile.patch_directories.is_empty() {
                profiles.push(assess_one(
                    EmulatorKind::Pcsx2,
                    profile.profile_id.clone(),
                    format!("{:?}", profile.installation_type),
                    format!("{:?}", profile.scope),
                    profile.provenance.to_string(),
                    profile.eligible,
                    profile
                        .blockers
                        .iter()
                        .map(|blocker| format!("{:?}: {}", blocker.kind, blocker.detail))
                        .collect(),
                    &profile.configuration_path,
                    // No patch directory was discovered; report the profile
                    // root so the finding still names something real.
                    &profile.configuration_path,
                    discoveries.preferred_pcsx2,
                    mount_table,
                ));
            }
        }
    } else if let Some(error) = &discoveries.pcsx2_error {
        unavailable.push((EmulatorKind::Pcsx2, error.clone()));
    }

    if let Some(discovery) = discoveries.xenia {
        discovery_incomplete |= !discovery.complete;
        for profile in &discovery.profiles {
            profiles.push(assess_one(
                EmulatorKind::Xenia,
                profile.profile_id.clone(),
                format!("{:?}", profile.installation_type),
                format!("{:?}", profile.scope),
                profile.provenance.to_string(),
                profile.eligible,
                profile
                    .blockers
                    .iter()
                    .map(|blocker| format!("{:?}: {}", blocker.kind, blocker.detail))
                    .collect(),
                &profile.configuration_path,
                &profile.patches_path,
                discoveries.preferred_xenia,
                mount_table,
            ));
        }
    } else if let Some(error) = &discoveries.xenia_error {
        unavailable.push((EmulatorKind::Xenia, error.clone()));
    }

    if let Some(discovery) = discoveries.ppsspp {
        discovery_incomplete |= !discovery.complete;
        for profile in &discovery.profiles {
            profiles.push(assess_one(
                EmulatorKind::Ppsspp,
                profile.profile_id.clone(),
                format!("{:?}", profile.installation_type),
                format!("{:?}", profile.scope),
                profile.provenance.to_string(),
                profile.eligible,
                profile
                    .blockers
                    .iter()
                    .map(|blocker| format!("{:?}: {}", blocker.kind, blocker.detail))
                    .collect(),
                &profile.configuration_path,
                // The directory a per-game PPSSPP cheat file would be
                // written into once cheat-write support exists.
                &profile.cheats_path,
                discoveries.preferred_ppsspp,
                mount_table,
            ));
        }
    } else if let Some(error) = &discoveries.ppsspp_error {
        unavailable.push((EmulatorKind::Ppsspp, error.clone()));
    }

    if let Some(discovery) = discoveries.duckstation {
        discovery_incomplete |= !discovery.complete;
        for profile in &discovery.profiles {
            profiles.push(assess_one(
                EmulatorKind::DuckStation,
                profile.profile_id.clone(),
                format!("{:?}", profile.installation_type),
                // DuckStation has no separate profile-scope concept - every
                // discovered profile is either found or explicitly supplied.
                "N/A".to_string(),
                "discovered configuration directory".to_string(),
                profile.eligible,
                profile.blocker.iter().cloned().collect(),
                &profile.configuration_path,
                // The directory a per-game DuckStation cheat file would be
                // written into once cheat-write support exists.
                &profile.cheats_path,
                discoveries.preferred_duckstation,
                mount_table,
            ));
        }
    } else if let Some(error) = &discoveries.duckstation_error {
        unavailable.push((EmulatorKind::DuckStation, error.clone()));
    }

    if let Some(discovery) = discoveries.xemu {
        discovery_incomplete |= !discovery.complete;
        for profile in &discovery.profiles {
            profiles.push(assess_one(
                EmulatorKind::Xemu,
                profile.profile_id.clone(),
                format!("{:?}", profile.installation_type),
                format!("{:?}", profile.scope),
                profile.provenance.to_string(),
                profile.eligible,
                profile
                    .blockers
                    .iter()
                    .map(|blocker| format!("{:?}: {}", blocker.kind, blocker.detail))
                    .collect(),
                &profile.configuration_path,
                // xemu has no separate install/cheat destination yet; report
                // the profile root itself so the finding still names
                // something real.
                &profile.configuration_path,
                discoveries.preferred_xemu,
                mount_table,
            ));
        }
    } else if let Some(error) = &discoveries.xemu_error {
        unavailable.push((EmulatorKind::Xemu, error.clone()));
    }

    if let Some(discovery) = discoveries.rpcs3 {
        discovery_incomplete |= !discovery.complete;
        for profile in &discovery.profiles {
            profiles.push(assess_one(
                EmulatorKind::Rpcs3,
                profile.profile_id.clone(),
                format!("{:?}", profile.installation_type),
                format!("{:?}", profile.scope),
                profile.provenance.to_string(),
                profile.eligible,
                profile
                    .blockers
                    .iter()
                    .map(|blocker| format!("{:?}: {}", blocker.kind, blocker.detail))
                    .collect(),
                &profile.configuration_path,
                // RPCS3 has no separate install/cheat destination yet;
                // report the profile root itself so the finding still names
                // something real.
                &profile.configuration_path,
                discoveries.preferred_rpcs3,
                mount_table,
            ));
        }
    } else if let Some(error) = &discoveries.rpcs3_error {
        unavailable.push((EmulatorKind::Rpcs3, error.clone()));
    }

    profiles.sort_by(|left, right| {
        (
            left.emulator,
            &left.profile_id,
            &left.destination_path.display,
        )
            .cmp(&(
                right.emulator,
                &right.profile_id,
                &right.destination_path.display,
            ))
    });
    ProfileAssessmentReport {
        profiles,
        unavailable,
        discovery_incomplete,
    }
}

#[allow(clippy::too_many_arguments)]
fn assess_one(
    emulator: EmulatorKind,
    profile_id: String,
    profile_kind: String,
    scope: String,
    discovery_confidence: String,
    eligible: bool,
    blockers: Vec<String>,
    root_path: &Path,
    destination_path: &PathBuf,
    preferred_profile_id: Option<&str>,
    mount_table: Option<&[MountEntry]>,
) -> ProfileAssessment {
    // `symlink_metadata`, never `metadata`: a symlinked destination must be
    // reported as unsafe rather than silently followed.
    let metadata = fs::symlink_metadata(destination_path).ok();
    let destination_exists = metadata.is_some();
    let destination_is_symlink = metadata
        .as_ref()
        .is_some_and(|value| value.file_type().is_symlink());
    let destination_is_directory = metadata.as_ref().is_some_and(fs::Metadata::is_dir);
    let mount_mode = match mount_table {
        Some(table) => mount_entry_for_path(table, destination_path)
            .map_or(MountMode::Unknown, MountEntry::mode),
        None => MountMode::Unknown,
    };
    let permissions = destination_exists
        .then(|| assess_permissions(destination_path))
        .flatten();
    let writability = assess_writability(
        destination_exists,
        destination_is_directory,
        destination_is_symlink,
        mount_mode,
        permissions,
    );
    ProfileAssessment {
        emulator,
        preferred: preferred_profile_id.map(|preferred| preferred == profile_id),
        profile_id,
        profile_kind,
        scope,
        discovery_confidence,
        eligible,
        blockers,
        root_path: EncodedPath::from_path(root_path),
        destination_path: EncodedPath::from_path(destination_path),
        destination_exists,
        destination_is_directory,
        destination_is_symlink,
        mount_mode,
        permissions,
        writability,
    }
}

/// Profile findings: one per profile that has something to report, plus a
/// summary when there are too many, an inspection result for every discovered
/// PPSSPP/DuckStation profile, and an ambiguity finding when several eligible
/// profiles compete and none is selected.
///
/// A profile that appears writable produces no finding at all except for
/// PPSSPP and DuckStation. Those adapters are inspection-only, so their
/// discovered profile metadata belongs in Doctor even when healthy.
pub fn findings_from_emulator_profiles(report: &ProfileAssessmentReport) -> Vec<Finding> {
    let mut findings = Vec::new();
    let notable: Vec<&ProfileAssessment> = report
        .profiles
        .iter()
        .filter(|profile| profile.severity().is_some() && !profile.is_inspection_only_profile())
        .collect();

    if notable.len() <= MAX_INDIVIDUAL_PROFILE_FINDINGS {
        for profile in &notable {
            let severity = profile.severity().expect("filtered above");
            findings.push(profile_finding(profile, severity));
        }
    } else {
        let mut evidence: Vec<String> = notable
            .iter()
            .take(MAX_INDIVIDUAL_PROFILE_FINDINGS)
            .map(|profile| {
                format!(
                    "{} ({}): {} - {}",
                    profile.emulator.label(),
                    profile.profile_kind,
                    profile.destination_path.display,
                    profile.writability.label()
                )
            })
            .collect();
        evidence.push(format!(
            "... and {} more",
            notable.len() - MAX_INDIVIDUAL_PROFILE_FINDINGS
        ));
        evidence.push(format!(
            "Individual profiles are not listed separately above {MAX_INDIVIDUAL_PROFILE_FINDINGS}, so this one result covers them all."
        ));
        findings.push(
            Finding::new(
                "emulator_profile.multiple_need_attention",
                DoctorCategory::EmulatorProfiles,
                DoctorSubsystem::EmulatorProfiles,
                notable
                    .iter()
                    .filter_map(|profile| profile.severity())
                    .min_by_key(|severity| severity.rank())
                    .unwrap_or(DoctorSeverity::Info),
                "Several emulator profiles need attention",
                format!(
                    "{} discovered emulator profiles have a destination EmuWiz could not confirm it can write to.",
                    notable.len()
                ),
            )
            .with_evidence(evidence)
            .with_measurements([
                ("profiles_needing_attention", Measurement::Integer(notable.len() as u64)),
                (
                    "individual_findings_truncated",
                    Measurement::Flag(true),
                ),
                (
                    "individual_finding_limit",
                    Measurement::Integer(MAX_INDIVIDUAL_PROFILE_FINDINGS as u64),
                ),
            ]),
        );
    }

    for profile in report
        .profiles
        .iter()
        .filter(|profile| profile.is_inspection_only_profile())
    {
        findings.push(profile_finding(
            profile,
            profile.severity().unwrap_or(DoctorSeverity::Info),
        ));
    }

    // Several eligible, apparently writable profiles with none selected is
    // genuinely ambiguous: an install would have to ask.
    for emulator in [
        EmulatorKind::Dolphin,
        EmulatorKind::Pcsx2,
        EmulatorKind::Xenia,
        EmulatorKind::Ppsspp,
        EmulatorKind::DuckStation,
        EmulatorKind::Xemu,
        EmulatorKind::Rpcs3,
    ] {
        let candidates: Vec<&ProfileAssessment> = report
            .profiles
            .iter()
            .filter(|profile| {
                profile.emulator == emulator
                    && profile.eligible
                    && profile.writability == WritabilityAssessment::AppearsWritable
            })
            .collect();
        let none_selected = candidates
            .iter()
            .all(|profile| profile.preferred != Some(true));
        if candidates.len() > 1 && none_selected {
            findings.push(
                Finding::new(
                    emulator.ambiguous_profile_finding_id(),
                    DoctorCategory::EmulatorProfiles,
                    DoctorSubsystem::EmulatorProfiles,
                    DoctorSeverity::Info,
                    format!("More than one {} profile could be used", emulator.label()),
                    format!(
                        "{} usable {} profiles were found and none is selected, so EmuWiz will ask before installing.",
                        candidates.len(),
                        emulator.label()
                    ),
                )
                .with_evidence(
                    candidates
                        .iter()
                        .map(|profile| {
                            format!(
                                "{} ({}): {}",
                                profile.profile_id,
                                profile.profile_kind,
                                profile.destination_path.display
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .with_guidance(
                    "This is not a fault. EmuWiz never picks an emulator profile for you when there is real ambiguity.",
                    "Choose a profile in the Cheats and Mods workflow to have it remembered.",
                ),
            );
        }
    }
    findings
}

/// Adapters whose profiles could not be assessed, and the sandbox caveat that
/// applies even when they could.
pub fn not_checked_from_emulator_profiles(
    report: &ProfileAssessmentReport,
) -> Vec<NotCheckedCheck> {
    let mut items: Vec<NotCheckedCheck> = report
        .unavailable
        .iter()
        .map(|(emulator, reason)| NotCheckedCheck {
            name: format!("{} profiles", emulator.label()),
            reason: reason.clone(),
            next_step:
                "Rescan profiles from the Cheats and Mods page, or set an explicit directory."
                    .to_string(),
        })
        .collect();
    if report.discovery_incomplete {
        items.push(NotCheckedCheck {
            name: "Complete emulator profile discovery".to_string(),
            reason: "One adapter reported its own discovery as incomplete, so some profiles may be missing from this result.".to_string(),
            next_step: "Rescan profiles from the Cheats and Mods page.".to_string(),
        });
    }
    // Stated on every run: this is the honest limit of a metadata-only check.
    if report.profiles.iter().any(|profile| {
        profile
            .profile_kind
            .to_ascii_lowercase()
            .contains("flatpak")
    }) {
        items.push(NotCheckedCheck {
            name: "Flatpak sandbox write permission".to_string(),
            reason: "A Flatpak profile's real writability depends on portal and sandbox permissions, which cannot be read from file metadata. EmuWiz reports what the permissions and mount state say, and no more.".to_string(),
            next_step: "No action needed. An install will report the real outcome.".to_string(),
        });
    }
    items
}

/// A stable, machine-readable form of one profile for `--json`, so a script
/// gets values rather than prose.
pub fn profile_json_summary(profile: &ProfileAssessment) -> serde_json::Value {
    serde_json::json!({
        "emulator": profile.emulator,
        "profile_id": profile.profile_id,
        "profile_kind": profile.profile_kind,
        "scope": profile.scope,
        "discovery_confidence": profile.discovery_confidence,
        "eligible": profile.eligible,
        "root_path": profile.root_path,
        "destination_path": profile.destination_path,
        "destination_exists": profile.destination_exists,
        "filesystem_read_only": profile.mount_mode == MountMode::ReadOnly,
        "mount_mode": profile.mount_mode,
        "writability_assessment": profile.writability,
        "preferred": profile.preferred,
        "finding_id": profile.finding_id(),
    })
}

impl EmulatorKind {
    /// Namespace fragment for ids, kept next to the labels so the two cannot
    /// drift.
    pub fn id_fragment(self) -> &'static str {
        self.slug()
    }
}

// --- Discovery gatherer ---------------------------------------------------

/// Owned profile discoveries, so a caller can gather once and then borrow for
/// the pure runner.
///
/// Each adapter is a separate `Result`: one emulator's discovery failing must
/// never hide another's, and "not discoverable" is reported rather than being
/// silently treated as "no profiles".
#[derive(Debug)]
pub struct DiscoveredProfiles {
    pub dolphin: Result<DolphinProfileDiscovery, String>,
    /// The unique active/credible Dolphin profile selected by the same rules
    /// as Cheats & Mods. `None` preserves genuine ambiguity.
    pub preferred_dolphin: Option<String>,
    pub pcsx2: Result<Pcsx2ProfileDiscovery, String>,
    /// Xenia has no documented native configuration path, so it is only ever
    /// discovered from roots the user has already pointed EmuWiz at. With
    /// none supplied there is nothing to assess - not a failure.
    pub xenia: Option<XeniaProfileDiscovery>,
    pub ppsspp: Result<PpssppProfileDiscovery, String>,
    pub duckstation: Result<DuckStationProfileDiscovery, String>,
    pub xemu: Result<XemuProfileDiscovery, String>,
    pub rpcs3: Result<Rpcs3ProfileDiscovery, String>,
}

impl DiscoveredProfiles {
    /// Discovers Dolphin, PCSX2, PPSSPP, DuckStation, xemu and RPCS3
    /// profiles from their documented paths, plus Xenia from the supplied
    /// explicit roots only.
    ///
    /// Read-only: each adapter's discovery inspects metadata of documented
    /// paths and never creates a directory or a profile.
    pub fn from_environment(explicit_xenia_roots: Vec<PathBuf>) -> Self {
        let dolphin = DolphinProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("Dolphin profiles could not be discovered: {error}"))
            .and_then(|roots| {
                discover_dolphin_profiles(&roots)
                    .map_err(|error| format!("Dolphin profiles could not be discovered: {error}"))
            });
        let pcsx2 = Pcsx2ProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("PCSX2 profiles could not be discovered: {error}"))
            .and_then(|roots| {
                discover_pcsx2_profiles(&roots)
                    .map_err(|error| format!("PCSX2 profiles could not be discovered: {error}"))
            });
        let xenia = if explicit_xenia_roots.is_empty() {
            None
        } else {
            Some(discover_xenia_profiles(&XeniaProfileDiscoveryRoots {
                explicit_configuration_roots: explicit_xenia_roots,
            }))
        };
        let ppsspp = PpssppProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("PPSSPP profiles could not be discovered: {error}"))
            .map(|roots| discover_ppsspp_profiles(&roots));
        let duckstation = DuckStationProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("DuckStation profiles could not be discovered: {error}"))
            .map(|roots| discover_duckstation_profiles(&roots));
        let xemu = XemuProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("xemu profiles could not be discovered: {error}"))
            .map(|roots| discover_xemu_profiles(&roots));
        let rpcs3 = Rpcs3ProfileDiscoveryRoots::from_environment()
            .map_err(|error| format!("RPCS3 profiles could not be discovered: {error}"))
            .map(|roots| discover_rpcs3_profiles(&roots));
        let preferred_dolphin = dolphin.as_ref().ok().and_then(|discovery| {
            match select_dolphin_profile(discovery, None) {
                EmulatorProfileSelection::Auto { profile_id, .. } => Some(profile_id),
                EmulatorProfileSelection::NeedsChoice { .. }
                | EmulatorProfileSelection::SetupNeeded => None,
            }
        });
        Self {
            dolphin,
            preferred_dolphin,
            pcsx2,
            xenia,
            ppsspp,
            duckstation,
            xemu,
            rpcs3,
        }
    }

    pub fn borrowed(&self) -> ProfileDiscoveries<'_> {
        ProfileDiscoveries {
            dolphin: self.dolphin.as_ref().ok(),
            dolphin_error: self.dolphin.as_ref().err().cloned(),
            pcsx2: self.pcsx2.as_ref().ok(),
            pcsx2_error: self.pcsx2.as_ref().err().cloned(),
            xenia: self.xenia.as_ref(),
            xenia_error: None,
            ppsspp: self.ppsspp.as_ref().ok(),
            ppsspp_error: self.ppsspp.as_ref().err().cloned(),
            duckstation: self.duckstation.as_ref().ok(),
            duckstation_error: self.duckstation.as_ref().err().cloned(),
            xemu: self.xemu.as_ref().ok(),
            xemu_error: self.xemu.as_ref().err().cloned(),
            rpcs3: self.rpcs3.as_ref().ok(),
            rpcs3_error: self.rpcs3.as_ref().err().cloned(),
            preferred_dolphin: self.preferred_dolphin.as_deref(),
            preferred_pcsx2: None,
            preferred_xenia: None,
            preferred_ppsspp: None,
            preferred_duckstation: None,
            preferred_xemu: None,
            preferred_rpcs3: None,
        }
    }
}

/// Every directory a discovered profile would be written into, for the
/// free-space and read-only checks. Deduplicated, deterministic order.
pub fn profile_destination_directories(report: &ProfileAssessmentReport) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = report
        .profiles
        .iter()
        .map(|profile| {
            let path = PathBuf::from(&profile.destination_path.display);
            // A destination that is a file (a .pnach, a GameSettings .ini)
            // shares its parent's filesystem, which is what capacity and
            // mount mode are actually about.
            if profile.destination_is_directory {
                path
            } else {
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            }
        })
        .collect();
    directories.sort();
    directories.dedup();
    directories
}

/// The managed-file scan targets implied by discovered profiles.
///
/// Only formats with an in-file EmuWiz marker are worth returning: PCSX2
/// managed blocks and Dolphin's `[ArchiveFS_Managed_GameHacking]` section.
/// Other formats carry no ownership proof, so walking them would read user
/// files for no diagnostic gain.
pub fn managed_scan_targets(report: &ProfileAssessmentReport) -> Vec<ManagedScanTarget> {
    let mut targets: Vec<ManagedScanTarget> = report
        .profiles
        .iter()
        .filter(|profile| {
            matches!(
                profile.emulator,
                EmulatorKind::Pcsx2 | EmulatorKind::Dolphin
            ) && profile.destination_exists
        })
        .map(|profile| {
            let path = PathBuf::from(&profile.destination_path.display);
            ManagedScanTarget {
                format: match profile.emulator {
                    EmulatorKind::Dolphin => ManagedFormat::DolphinGameSettings,
                    EmulatorKind::Pcsx2 => ManagedFormat::Pcsx2Pnach,
                    EmulatorKind::Xenia
                    | EmulatorKind::Ppsspp
                    | EmulatorKind::DuckStation
                    | EmulatorKind::Xemu
                    | EmulatorKind::Rpcs3 => {
                        unreachable!("filtered above")
                    }
                },
                destination_root: if profile.destination_is_directory {
                    path
                } else {
                    path.parent().map(Path::to_path_buf).unwrap_or(path)
                },
            }
        })
        .collect();
    targets.sort_by(|left, right| left.destination_root.cmp(&right.destination_root));
    targets.dedup();
    targets
}

// ---------------------------------------------------------------------------
// xemu / Xenia launch readiness
// ---------------------------------------------------------------------------
//
// A distinct question from the writability assessment above: not "could
// EmuWiz write into this profile" but "could this profile actually launch a
// game right now". Kept as its own small model rather than folded into
// `ProfileAssessment` because the two adapters' real failure reasons do not
// reduce to one shared shape - xemu has four independent system files with
// no Xenia equivalent, and Xenia's one real-world confusion (a Windows
// `xenia_canary.exe` sitting beside a real profile, unusable natively) has
// no xemu equivalent. Genericizing them into one struct would either lose
// xemu's four-way firmware detail or invent a fake firmware requirement for
// Xenia - see the module's own task notes.
//
// The `assess_*` functions below read profile/system-file metadata (via
// `resolve_xemu_native_launch_binding`/`resolve_xenia_launch_binding`/
// `inspect_xemu_game`, all already read-only) and belong in this gatherer
// module for exactly the same reason `assess_one` does. The `findings_from_*`
// functions are pure transforms over that already-gathered data, safe to
// call from the pure `runner`.

/// One eligible xemu profile's launch readiness: its native executable
/// binding, and all four required system files. Independent of any
/// specific game - xemu's system files are read from its own `xemu.toml`,
/// never from per-game state, so [`XemuGameRequest::default`] is always
/// enough here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct XemuReadinessAssessment {
    pub profile_id: String,
    pub root_path: EncodedPath,
    /// `Some` only when [`resolve_xemu_native_launch_binding`] succeeded.
    pub executable: Option<EncodedPath>,
    /// The plain-language reason no executable binding exists, when there
    /// is one.
    pub binding_problem: Option<String>,
    pub mcpx: XemuSystemFileState,
    pub flash_bios: XemuSystemFileState,
    pub eeprom: XemuSystemFileState,
    pub hdd: XemuSystemFileState,
}

/// Assesses every eligible discovered xemu profile's launch readiness.
///
/// Read-only: [`resolve_xemu_native_launch_binding`] inspects only
/// filesystem metadata, and [`inspect_xemu_game`] reads only `xemu.toml`
/// and the configured system-file paths' metadata. Nothing is created,
/// mounted, or executed.
pub fn assess_xemu_readiness(
    discovery: Option<&XemuProfileDiscovery>,
) -> Vec<XemuReadinessAssessment> {
    let Some(discovery) = discovery else {
        return Vec::new();
    };
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| {
            let binding = resolve_xemu_native_launch_binding(profile);
            let health = inspect_xemu_game(profile, &XemuGameRequest::default()).health;
            XemuReadinessAssessment {
                profile_id: profile.profile_id.clone(),
                root_path: EncodedPath::from_path(&profile.configuration_path),
                executable: binding
                    .as_ref()
                    .ok()
                    .map(|binding| EncodedPath::from_path(&binding.executable)),
                binding_problem: binding.as_ref().err().map(xemu_binding_problem),
                mcpx: health.mcpx,
                flash_bios: health.flash_bios,
                eeprom: health.eeprom,
                hdd: health.hdd,
            }
        })
        .collect()
}

/// A user-facing sentence for one xemu binding failure - never a raw enum
/// dump.
fn xemu_binding_problem(blocker: &XemuLaunchBlocker) -> String {
    match blocker.kind {
        XemuLaunchBlockerKind::ProfileIneligible => {
            "the profile is not eligible for a native launch".to_string()
        }
        XemuLaunchBlockerKind::UnsupportedInstallationType => {
            "the discovered xemu installation is not a supported native Linux install".to_string()
        }
        XemuLaunchBlockerKind::ExecutableMissing => {
            "no native xemu executable was found in the profile".to_string()
        }
        XemuLaunchBlockerKind::ExecutableUnsafe => {
            "the xemu executable is a symlink or not a regular file".to_string()
        }
        XemuLaunchBlockerKind::ExecutableNotExecutable => {
            "the xemu executable does not have the execute permission set".to_string()
        }
        XemuLaunchBlockerKind::AmbiguousExecutable => {
            "more than one native xemu executable was found and none is preferred".to_string()
        }
    }
}

/// A short, user-facing label for one xemu system file's state - never a
/// raw enum dump.
fn xemu_system_file_label(state: XemuSystemFileState) -> &'static str {
    match state {
        XemuSystemFileState::Present => "present",
        XemuSystemFileState::Missing => "missing",
        XemuSystemFileState::Unreadable => "present but unreadable",
        XemuSystemFileState::NotConfigured => "not configured",
        XemuSystemFileState::Unknown => "could not be determined",
    }
}

/// One Finding per eligible xemu profile, always produced (like the
/// PPSSPP/DuckStation inspection findings) so a healthy result is stated
/// explicitly rather than left as silence a user could mistake for "not
/// checked".
pub fn findings_from_xemu_readiness(assessments: &[XemuReadinessAssessment]) -> Vec<Finding> {
    assessments
        .iter()
        .map(|assessment| {
            let firmware: Vec<(&str, XemuSystemFileState)> = vec![
                ("MCPX boot ROM", assessment.mcpx),
                ("flash BIOS", assessment.flash_bios),
                ("EEPROM", assessment.eeprom),
                ("HDD image", assessment.hdd),
            ];
            let missing: Vec<&str> = firmware
                .iter()
                .filter(|(_, state)| *state != XemuSystemFileState::Present)
                .map(|(label, _)| *label)
                .collect();
            let ready = assessment.executable.is_some() && missing.is_empty();
            let severity = if ready {
                DoctorSeverity::Info
            } else {
                DoctorSeverity::Warning
            };
            let title = if ready {
                "xemu is ready to launch".to_string()
            } else {
                "xemu is not ready to launch".to_string()
            };
            let mut problems = Vec::new();
            if let Some(binding_problem) = &assessment.binding_problem {
                problems.push(format!("Executable: {binding_problem}"));
            }
            for (label, state) in &firmware {
                if *state != XemuSystemFileState::Present {
                    problems.push(format!("{label} is {}", xemu_system_file_label(*state)));
                }
            }
            let explanation = if ready {
                "A native xemu executable was found and all four required system files (MCPX \
                 boot ROM, flash BIOS, EEPROM, HDD image) are present."
                    .to_string()
            } else {
                format!("xemu cannot launch a game yet: {}.", problems.join("; "))
            };
            let mut evidence = vec![
                format!("Profile: {}", assessment.profile_id),
                format!("Configuration path: {}", assessment.root_path.display),
            ];
            match &assessment.executable {
                Some(executable) => evidence.push(format!("Executable: {}", executable.display)),
                None => evidence.push(format!(
                    "Executable: not available ({})",
                    assessment.binding_problem.as_deref().unwrap_or("unknown")
                )),
            }
            for (label, state) in &firmware {
                evidence.push(format!("{label}: {}", xemu_system_file_label(*state)));
            }
            let (why_it_matters, next_step) = if ready {
                (
                    "xemu needs a safe native executable and all four Xbox system files (MCPX \
                     boot ROM, flash BIOS, EEPROM, HDD image) to boot a game.",
                    "No action needed.",
                )
            } else {
                (
                    "xemu cannot launch a game until every required system file is present and a \
                     safe native executable is found.",
                    "Place the missing file(s) where xemu expects them, or fix the executable \
                     issue, then re-run Doctor.",
                )
            };
            Finding::new(
                "emulator_readiness.xemu",
                DoctorCategory::EmulatorProfiles,
                DoctorSubsystem::EmulatorReadiness,
                severity,
                title,
                explanation,
            )
            .with_affected(assessment.root_path.clone())
            .with_evidence(evidence)
            .with_measurements([
                ("profile", Measurement::text(&assessment.profile_id)),
                (
                    "executable_found",
                    Measurement::Flag(assessment.executable.is_some()),
                ),
                (
                    "mcpx",
                    Measurement::text(xemu_system_file_label(assessment.mcpx)),
                ),
                (
                    "flash_bios",
                    Measurement::text(xemu_system_file_label(assessment.flash_bios)),
                ),
                (
                    "eeprom",
                    Measurement::text(xemu_system_file_label(assessment.eeprom)),
                ),
                (
                    "hdd",
                    Measurement::text(xemu_system_file_label(assessment.hdd)),
                ),
                ("ready", Measurement::Flag(ready)),
            ])
            .with_guidance(why_it_matters, next_step)
        })
        .collect()
}

/// One eligible Xenia profile's launch readiness: its native Linux
/// executable binding only. Xenia has no firmware/BIOS concept in this
/// build, so none is invented here - see [`findings_from_xenia_readiness`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct XeniaReadinessAssessment {
    pub profile_id: String,
    pub root_path: EncodedPath,
    /// `Some` only when [`resolve_xenia_launch_binding`] succeeded.
    pub executable: Option<EncodedPath>,
    pub binding_problem: Option<String>,
    /// `true` when a Windows `xenia_canary.exe` exists in the profile
    /// directory - checked purely for wording: it never counts toward, or
    /// substitutes for, a valid native binding.
    pub windows_exe_present: bool,
}

/// Assesses every eligible discovered Xenia profile's launch readiness.
///
/// Read-only: [`resolve_xenia_launch_binding`] inspects only filesystem
/// metadata, and the `.exe` check is a single `Path::exists` read solely to
/// word the "Windows-only install" case precisely - it never influences
/// which binding is chosen.
pub fn assess_xenia_readiness(
    discovery: Option<&XeniaProfileDiscovery>,
) -> Vec<XeniaReadinessAssessment> {
    let Some(discovery) = discovery else {
        return Vec::new();
    };
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| {
            let binding = resolve_xenia_launch_binding(profile);
            XeniaReadinessAssessment {
                profile_id: profile.profile_id.clone(),
                root_path: EncodedPath::from_path(&profile.configuration_path),
                executable: binding
                    .as_ref()
                    .ok()
                    .map(|binding| EncodedPath::from_path(&binding.executable)),
                binding_problem: binding.as_ref().err().map(xenia_binding_problem),
                windows_exe_present: profile.configuration_path.join("xenia_canary.exe").exists(),
            }
        })
        .collect()
}

/// A user-facing sentence for one Xenia binding failure - never a raw enum
/// dump.
fn xenia_binding_problem(blocker: &XeniaLaunchBlocker) -> String {
    match blocker.kind {
        XeniaLaunchBlockerKind::ProfileRootMismatch => {
            "the profile configuration no longer matches what was discovered".to_string()
        }
        XeniaLaunchBlockerKind::ExecutableMissing => {
            "no native Linux Xenia executable (xenia_canary or xenia) was found".to_string()
        }
        XeniaLaunchBlockerKind::ExecutableUnsafe => {
            "the candidate executable is a symlink or not a regular file".to_string()
        }
        XeniaLaunchBlockerKind::ExecutableNotExecutable => {
            "the candidate executable does not have the execute permission set".to_string()
        }
        XeniaLaunchBlockerKind::AmbiguousExecutable => {
            "more than one native Xenia executable was found and none is preferred".to_string()
        }
    }
}

/// One Finding per eligible Xenia profile, always produced for the same
/// reason as [`findings_from_xemu_readiness`].
pub fn findings_from_xenia_readiness(assessments: &[XeniaReadinessAssessment]) -> Vec<Finding> {
    assessments
        .iter()
        .map(|assessment| {
            let ready = assessment.executable.is_some();
            let severity = if ready {
                DoctorSeverity::Info
            } else {
                DoctorSeverity::Warning
            };
            let title = if ready {
                "Xenia is ready to launch".to_string()
            } else {
                "Xenia is not ready to launch".to_string()
            };
            let explanation = match (&assessment.executable, assessment.windows_exe_present) {
                (Some(executable), _) => format!(
                    "A native Linux Xenia executable was found: {}.",
                    executable.display
                ),
                (None, true) => "A Windows xenia_canary.exe was found, but it cannot be launched \
                                  natively on Linux. It is never treated as a valid native \
                                  executable. Install or place a native Linux Xenia executable \
                                  (xenia_canary or xenia, without a .exe extension) in the same \
                                  folder."
                    .to_string(),
                (None, false) => format!(
                    "Xenia cannot launch a game yet: {}.",
                    assessment
                        .binding_problem
                        .as_deref()
                        .unwrap_or("no native executable is available")
                ),
            };
            let mut evidence = vec![
                format!("Profile: {}", assessment.profile_id),
                format!("Configuration path: {}", assessment.root_path.display),
                format!(
                    "Windows xenia_canary.exe present: {}",
                    assessment.windows_exe_present
                ),
            ];
            match &assessment.executable {
                Some(executable) => evidence.push(format!("Executable: {}", executable.display)),
                None => evidence.push(format!(
                    "Executable: not available ({})",
                    assessment.binding_problem.as_deref().unwrap_or("unknown")
                )),
            }
            let (why_it_matters, next_step) = if ready {
                (
                    "Xenia needs a safe native Linux executable to boot a game. Windows/Wine \
                     execution is not supported.",
                    "No action needed.",
                )
            } else if assessment.windows_exe_present {
                (
                    "Only a Windows build was found. EmuWiz never assumes or configures Wine/Proton \
                     to run it.",
                    "Download or build a native Linux Xenia (Canary) release and place its \
                     executable in the same folder as xenia-canary.config.toml.",
                )
            } else {
                (
                    "Xenia cannot launch a game until a safe native Linux executable is found.",
                    "Fix the executable issue, or place a native Linux Xenia executable in the \
                     profile folder, then re-run Doctor.",
                )
            };
            Finding::new(
                "emulator_readiness.xenia",
                DoctorCategory::EmulatorProfiles,
                DoctorSubsystem::EmulatorReadiness,
                severity,
                title,
                explanation,
            )
            .with_affected(assessment.root_path.clone())
            .with_evidence(evidence)
            .with_measurements([
                ("profile", Measurement::text(&assessment.profile_id)),
                ("executable_found", Measurement::Flag(ready)),
                (
                    "windows_exe_present",
                    Measurement::Flag(assessment.windows_exe_present),
                ),
                ("ready", Measurement::Flag(ready)),
            ])
            .with_guidance(why_it_matters, next_step)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// PPSSPP / RPCS3 launch readiness
// ---------------------------------------------------------------------------
//
// The same distinct question as the xemu/Xenia section above: not "could
// EmuWiz write into this profile" but "could this profile actually launch a
// game right now". PPSSPP has no firmware/BIOS concept at all in this build
// (`ppsspp_firmware_readiness` is a constant `FirmwareReadiness::NotRequired`
// - see `crate::launch::readiness`'s own doc comment), so none is invented
// here. RPCS3 does have one, but it is deliberately never fully verified -
// `Rpcs3FirmwareStatus` has no `Verified` variant, only `Present`/`Missing`/
// `Unknown` (see `crate::launch::rpcs3_execution`'s own module doc comment
// for why its preflight therefore accepts `ReadyWithWarnings`, not only
// strict `Ready`) - so Doctor's wording below mirrors that exact policy
// rather than inventing a stricter or looser one of its own.

/// One eligible PPSSPP profile's launch readiness: its native executable
/// binding only. PPSSPP needs no firmware/BIOS to boot a game, so none is
/// modeled here - see this section's own doc comment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PpssppReadinessAssessment {
    pub profile_id: String,
    pub root_path: EncodedPath,
    /// `Some` only when [`resolve_ppsspp_native_launch_binding`] succeeded.
    pub executable: Option<EncodedPath>,
    pub binding_problem: Option<String>,
}

/// Assesses every eligible discovered PPSSPP profile's launch readiness.
///
/// Read-only: [`resolve_ppsspp_native_launch_binding`] inspects only
/// filesystem metadata. Nothing is created, mounted, or executed.
pub fn assess_ppsspp_readiness(
    discovery: Option<&PpssppProfileDiscovery>,
) -> Vec<PpssppReadinessAssessment> {
    let Some(discovery) = discovery else {
        return Vec::new();
    };
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| {
            let binding = resolve_ppsspp_native_launch_binding(profile);
            PpssppReadinessAssessment {
                profile_id: profile.profile_id.clone(),
                root_path: EncodedPath::from_path(&profile.configuration_path),
                executable: binding
                    .as_ref()
                    .ok()
                    .map(|binding| EncodedPath::from_path(&binding.executable)),
                binding_problem: binding.as_ref().err().map(ppsspp_binding_problem),
            }
        })
        .collect()
}

/// A user-facing sentence for one PPSSPP binding failure - never a raw enum
/// dump.
fn ppsspp_binding_problem(blocker: &PpssppLaunchBlocker) -> String {
    match blocker.kind {
        PpssppLaunchBlockerKind::ProfileIneligible => {
            "the profile is not eligible for a native launch".to_string()
        }
        PpssppLaunchBlockerKind::UnsupportedInstallationType => {
            "the discovered PPSSPP installation is not a supported native Linux install".to_string()
        }
        PpssppLaunchBlockerKind::ExecutableMissing => {
            "no native PPSSPP executable was found in the profile".to_string()
        }
        PpssppLaunchBlockerKind::ExecutableUnsafe => {
            "the PPSSPP executable is a symlink or not a regular file".to_string()
        }
        PpssppLaunchBlockerKind::ExecutableNotExecutable => {
            "the PPSSPP executable does not have the execute permission set".to_string()
        }
        PpssppLaunchBlockerKind::AmbiguousExecutable => {
            "more than one native PPSSPP executable was found and none is preferred".to_string()
        }
    }
}

/// One Finding per eligible PPSSPP profile, always produced (like the
/// xemu/Xenia readiness findings) so a healthy result is stated explicitly
/// rather than left as silence a user could mistake for "not checked".
pub fn findings_from_ppsspp_readiness(assessments: &[PpssppReadinessAssessment]) -> Vec<Finding> {
    assessments
        .iter()
        .map(|assessment| {
            let ready = assessment.executable.is_some();
            let severity = if ready {
                DoctorSeverity::Info
            } else {
                DoctorSeverity::Warning
            };
            let title = if ready {
                "PPSSPP is ready to launch".to_string()
            } else {
                "PPSSPP is not ready to launch".to_string()
            };
            let explanation = match &assessment.executable {
                Some(executable) => format!(
                    "A native PPSSPP executable was found: {}. PPSSPP needs no separate \
                     firmware or BIOS to boot a game.",
                    executable.display
                ),
                None => format!(
                    "PPSSPP cannot launch a game yet: {}.",
                    assessment
                        .binding_problem
                        .as_deref()
                        .unwrap_or("no native executable is available")
                ),
            };
            let mut evidence = vec![
                format!("Profile: {}", assessment.profile_id),
                format!("Configuration path: {}", assessment.root_path.display),
            ];
            match &assessment.executable {
                Some(executable) => evidence.push(format!("Executable: {}", executable.display)),
                None => evidence.push(format!(
                    "Executable: not available ({})",
                    assessment.binding_problem.as_deref().unwrap_or("unknown")
                )),
            }
            let (why_it_matters, next_step) = if ready {
                (
                    "PPSSPP needs a safe native executable to boot a game.",
                    "No action needed.",
                )
            } else {
                (
                    "PPSSPP cannot launch a game until a safe native executable is found.",
                    "Fix the executable issue, then re-run Doctor.",
                )
            };
            Finding::new(
                "emulator_readiness.ppsspp",
                DoctorCategory::EmulatorProfiles,
                DoctorSubsystem::EmulatorReadiness,
                severity,
                title,
                explanation,
            )
            .with_affected(assessment.root_path.clone())
            .with_evidence(evidence)
            .with_measurements([
                ("profile", Measurement::text(&assessment.profile_id)),
                ("executable_found", Measurement::Flag(ready)),
                ("ready", Measurement::Flag(ready)),
            ])
            .with_guidance(why_it_matters, next_step)
        })
        .collect()
}

/// One eligible RPCS3 profile's launch readiness: its native executable
/// binding, and its one firmware status. Independent of any specific game -
/// RPCS3's firmware presence is read from `dev_flash`, never from per-game
/// state, so [`Rpcs3GameRequest::default`] is always enough here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rpcs3ReadinessAssessment {
    pub profile_id: String,
    pub root_path: EncodedPath,
    /// `Some` only when [`resolve_rpcs3_native_launch_binding`] succeeded.
    pub executable: Option<EncodedPath>,
    pub binding_problem: Option<String>,
    #[serde(serialize_with = "serialize_firmware_readiness")]
    pub firmware: FirmwareReadiness,
}

/// [`FirmwareReadiness`] does not derive `Serialize` (it lives in
/// `crate::launch::readiness`, which this diagnostics-only requirement is
/// not the place to change) - serialized here as the same user-facing label
/// [`rpcs3_firmware_label`] shows, which is more useful for a `--json`
/// consumer than the raw variant name would be anyway.
fn serialize_firmware_readiness<S: serde::Serializer>(
    value: &FirmwareReadiness,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(rpcs3_firmware_label(*value))
}

/// Assesses every eligible discovered RPCS3 profile's launch readiness.
///
/// Read-only: [`resolve_rpcs3_native_launch_binding`] inspects only
/// filesystem metadata, and [`inspect_rpcs3_game`] reads only `dev_flash`
/// metadata. Nothing is created, mounted, or executed.
pub fn assess_rpcs3_readiness(
    discovery: Option<&Rpcs3ProfileDiscovery>,
) -> Vec<Rpcs3ReadinessAssessment> {
    let Some(discovery) = discovery else {
        return Vec::new();
    };
    discovery
        .profiles
        .iter()
        .filter(|profile| profile.eligible)
        .map(|profile| {
            let binding = resolve_rpcs3_native_launch_binding(profile);
            let health = inspect_rpcs3_game(profile, &Rpcs3GameRequest::default()).health;
            Rpcs3ReadinessAssessment {
                profile_id: profile.profile_id.clone(),
                root_path: EncodedPath::from_path(&profile.configuration_path),
                executable: binding
                    .as_ref()
                    .ok()
                    .map(|binding| EncodedPath::from_path(&binding.executable)),
                binding_problem: binding.as_ref().err().map(rpcs3_binding_problem),
                firmware: rpcs3_firmware_readiness(&health.firmware),
            }
        })
        .collect()
}

/// A user-facing sentence for one RPCS3 binding failure - never a raw enum
/// dump.
fn rpcs3_binding_problem(blocker: &Rpcs3LaunchBlocker) -> String {
    match blocker.kind {
        Rpcs3LaunchBlockerKind::ProfileIneligible => {
            "the profile is not eligible for a native launch".to_string()
        }
        Rpcs3LaunchBlockerKind::UnsupportedInstallation => {
            "the discovered RPCS3 installation is not a supported native Linux install".to_string()
        }
        Rpcs3LaunchBlockerKind::ExecutableMissing => {
            "no native RPCS3 executable was found in the profile".to_string()
        }
        Rpcs3LaunchBlockerKind::ExecutableUnsafe => {
            "the RPCS3 executable is a symlink or not a regular file".to_string()
        }
        Rpcs3LaunchBlockerKind::ExecutableNotExecutable => {
            "the RPCS3 executable does not have the execute permission set".to_string()
        }
        Rpcs3LaunchBlockerKind::AmbiguousExecutable => {
            "more than one native RPCS3 executable was found and none is preferred".to_string()
        }
    }
}

/// A short, user-facing label for RPCS3's one firmware status - never a raw
/// enum dump. Mirrors `crate::launch::rpcs3_execution`'s own accepted
/// policy exactly: `PresentUnverified` is a real, launchable state under
/// current execution policy (RPCS3 never hash-verifies its firmware), and
/// must never read as "blocked".
fn rpcs3_firmware_label(firmware: FirmwareReadiness) -> &'static str {
    match firmware {
        FirmwareReadiness::Verified => "verified",
        FirmwareReadiness::PresentUnverified => "present (not hash-verified)",
        FirmwareReadiness::Missing => "missing",
        FirmwareReadiness::Unknown => "could not be determined",
        FirmwareReadiness::NotRequired => "not required",
    }
}

/// One Finding per eligible RPCS3 profile, always produced for the same
/// reason as [`findings_from_xemu_readiness`].
///
/// Readiness mirrors `crate::launch::rpcs3_execution::preflight_rpcs3_launch`'s
/// own accepted policy exactly: a safe native executable plus firmware that
/// is `Verified` or `PresentUnverified` is reported as ready (with an
/// explicit "not hash-verified" caveat in the latter case, never presented
/// as a clean pass) - `Missing`/`Unknown` firmware is reported as not ready,
/// exactly like `build_rpcs3_command_plan`'s own `FirmwareReadiness::Unknown`
/// gate (an *unknown* firmware state is never launchable, but Doctor's
/// wording never claims to have proven it missing either).
pub fn findings_from_rpcs3_readiness(assessments: &[Rpcs3ReadinessAssessment]) -> Vec<Finding> {
    assessments
        .iter()
        .map(|assessment| {
            let firmware_launchable = matches!(
                assessment.firmware,
                FirmwareReadiness::Verified | FirmwareReadiness::PresentUnverified
            );
            let ready = assessment.executable.is_some() && firmware_launchable;
            let firmware_unverified = assessment.firmware == FirmwareReadiness::PresentUnverified;
            let severity = if ready {
                DoctorSeverity::Info
            } else {
                DoctorSeverity::Warning
            };
            let title = if ready {
                "RPCS3 is ready to launch".to_string()
            } else {
                "RPCS3 is not ready to launch".to_string()
            };
            let mut problems = Vec::new();
            if let Some(binding_problem) = &assessment.binding_problem {
                problems.push(format!("Executable: {binding_problem}"));
            }
            if !firmware_launchable {
                problems.push(format!(
                    "Firmware is {}",
                    rpcs3_firmware_label(assessment.firmware)
                ));
            }
            let explanation = if ready {
                if firmware_unverified {
                    "A native RPCS3 executable was found and firmware is present. RPCS3 never \
                     hash-verifies firmware contents, so this is reported ready under the same \
                     policy the launcher itself uses, not as a fully proven pass."
                        .to_string()
                } else {
                    "A native RPCS3 executable was found and firmware is present.".to_string()
                }
            } else {
                format!("RPCS3 cannot launch a game yet: {}.", problems.join("; "))
            };
            let mut evidence = vec![
                format!("Profile: {}", assessment.profile_id),
                format!("Configuration path: {}", assessment.root_path.display),
            ];
            match &assessment.executable {
                Some(executable) => evidence.push(format!("Executable: {}", executable.display)),
                None => evidence.push(format!(
                    "Executable: not available ({})",
                    assessment.binding_problem.as_deref().unwrap_or("unknown")
                )),
            }
            evidence.push(format!(
                "Firmware: {}",
                rpcs3_firmware_label(assessment.firmware)
            ));
            let (why_it_matters, next_step) = if ready {
                (
                    "RPCS3 needs a safe native executable and PS3 firmware installed under \
                     dev_flash to boot a game.",
                    "No action needed.",
                )
            } else if assessment.firmware == FirmwareReadiness::Missing {
                (
                    "RPCS3 cannot launch a game until PS3 firmware is installed.",
                    "Install PS3 firmware in RPCS3 itself, then re-run Doctor.",
                )
            } else if assessment.firmware == FirmwareReadiness::Unknown {
                (
                    "RPCS3's firmware state could not be determined from the profile's \
                     dev_flash directory, so a launch is not attempted.",
                    "Open RPCS3 and confirm firmware is installed, then re-run Doctor.",
                )
            } else {
                (
                    "RPCS3 cannot launch a game until a safe native executable is found.",
                    "Fix the executable issue, then re-run Doctor.",
                )
            };
            Finding::new(
                "emulator_readiness.rpcs3",
                DoctorCategory::EmulatorProfiles,
                DoctorSubsystem::EmulatorReadiness,
                severity,
                title,
                explanation,
            )
            .with_affected(assessment.root_path.clone())
            .with_evidence(evidence)
            .with_measurements([
                ("profile", Measurement::text(&assessment.profile_id)),
                (
                    "executable_found",
                    Measurement::Flag(assessment.executable.is_some()),
                ),
                (
                    "firmware",
                    Measurement::text(rpcs3_firmware_label(assessment.firmware)),
                ),
                ("ready", Measurement::Flag(ready)),
            ])
            .with_guidance(why_it_matters, next_step)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::tests::{TempTree, snapshot_tree};
    use std::os::unix::fs::symlink;

    /// One assessment of a real path, through the same code path the runner
    /// uses. Nothing is created here except the fixture itself.
    fn assess(destination: &Path, mount_table: Option<&[MountEntry]>) -> ProfileAssessment {
        assess_one(
            EmulatorKind::Pcsx2,
            "pcsx2-native".to_string(),
            "Native".to_string(),
            "User".to_string(),
            "documented native path".to_string(),
            true,
            Vec::new(),
            destination.parent().unwrap_or(destination),
            &destination.to_path_buf(),
            None,
            mount_table,
        )
    }

    fn report(profiles: Vec<ProfileAssessment>) -> ProfileAssessmentReport {
        ProfileAssessmentReport {
            profiles,
            unavailable: Vec::new(),
            discovery_incomplete: false,
        }
    }

    /// A read-write mount covering every path a fixture can use, so a test
    /// exercises the permissions branch rather than the unknown-mount branch.
    fn rw_table() -> Vec<MountEntry> {
        vec![MountEntry {
            mount_point: PathBuf::from("/"),
            read_only_mount: false,
            read_only_superblock: false,
            filesystem_type: Some("ext4".to_string()),
        }]
    }

    fn read_only_table(mount_point: &Path) -> Vec<MountEntry> {
        vec![MountEntry {
            mount_point: mount_point.to_path_buf(),
            read_only_mount: true,
            read_only_superblock: false,
            filesystem_type: Some("ext4".to_string()),
        }]
    }

    /// Test 35
    #[test]
    fn an_existing_writable_profile_directory_appears_writable() {
        let tree = TempTree::new("profiles-writable");
        let destination = tree.path().join("patches");
        fs::create_dir_all(&destination).expect("fixture");
        let assessed = assess(&destination, Some(&rw_table()));
        assert!(assessed.destination_exists);
        assert!(assessed.destination_is_directory);
        assert_eq!(assessed.writability, WritabilityAssessment::AppearsWritable);
    }

    /// Test 36
    #[test]
    fn a_writable_profile_is_information_not_a_problem() {
        let tree = TempTree::new("profiles-healthy");
        let destination = tree.path().join("patches");
        fs::create_dir_all(&destination).expect("fixture");
        let assessed = assess(&destination, Some(&rw_table()));
        assert_eq!(
            assessed.severity(),
            None,
            "a profile EmuWiz can write to must produce no finding at all"
        );
        assert!(findings_from_emulator_profiles(&report(vec![assessed])).is_empty());
    }

    /// Test 37
    #[test]
    fn a_missing_profile_directory_is_reported_but_never_created() {
        let tree = TempTree::new("profiles-missing");
        let destination = tree.path().join("patches");
        let before = snapshot_tree(tree.path());
        let assessed = assess(&destination, Some(&rw_table()));
        assert_eq!(
            assessed.writability,
            WritabilityAssessment::MissingDestination
        );
        let findings = findings_from_emulator_profiles(&report(vec![assessed]));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "emulator_profile.missing_destination");
        assert_eq!(findings[0].severity, DoctorSeverity::Info);
        assert_eq!(
            snapshot_tree(tree.path()),
            before,
            "Doctor must never create a profile directory to make a check pass"
        );
    }

    /// Test 38
    #[test]
    fn a_read_only_filesystem_beats_permissions_that_look_writable() {
        let tree = TempTree::new("profiles-read-only");
        let destination = tree.path().join("patches");
        fs::create_dir_all(&destination).expect("fixture");
        let table = read_only_table(tree.path());
        let assessed = assess(&destination, Some(&table));
        assert_eq!(
            assessed.writability,
            WritabilityAssessment::ReadOnlyFilesystem
        );
        let findings = findings_from_emulator_profiles(&report(vec![assessed]));
        assert_eq!(findings[0].id, "emulator_profile.read_only_filesystem");
        assert!(
            findings[0]
                .next_step
                .as_deref()
                .expect("guidance")
                .contains("Remount"),
            "the advice must address the mount, not the permissions"
        );
    }

    /// Test 39
    #[test]
    fn a_symlinked_profile_destination_is_unsafe_and_is_not_followed() {
        let tree = TempTree::new("profiles-symlink");
        let real = tree.path().join("real-patches");
        fs::create_dir_all(&real).expect("fixture");
        let link = tree.path().join("patches");
        symlink(&real, &link).expect("fixture");
        let assessed = assess(&link, Some(&rw_table()));
        assert!(assessed.destination_is_symlink);
        assert_eq!(
            assessed.writability,
            WritabilityAssessment::UnsafeDestination
        );
        let findings = findings_from_emulator_profiles(&report(vec![assessed]));
        assert_eq!(findings[0].id, "emulator_profile.unsafe_destination");
        assert_eq!(findings[0].severity, DoctorSeverity::Warning);
    }

    /// Test 40
    #[test]
    fn a_file_where_a_profile_directory_belongs_is_unsafe() {
        let tree = TempTree::new("profiles-file");
        let destination = tree.path().join("patches");
        fs::write(&destination, b"not a directory").expect("fixture");
        let assessed = assess(&destination, Some(&rw_table()));
        assert!(!assessed.destination_is_directory);
        assert_eq!(
            assessed.writability,
            WritabilityAssessment::UnsafeDestination
        );
    }

    /// Test 41
    #[test]
    fn an_unknown_mount_state_leaves_writability_unproven_rather_than_claimed() {
        let tree = TempTree::new("profiles-unknown-mount");
        let destination = tree.path().join("patches");
        fs::create_dir_all(&destination).expect("fixture");
        let assessed = assess(&destination, None);
        assert_eq!(assessed.mount_mode, MountMode::Unknown);
        assert_eq!(assessed.writability, WritabilityAssessment::NotProven);
        let findings = findings_from_emulator_profiles(&report(vec![assessed]));
        assert_eq!(findings[0].id, "emulator_profile.writability_not_proven");
        assert!(
            findings[0]
                .why_it_matters
                .as_deref()
                .expect("guidance")
                .contains("will not write a test file"),
            "the reason for the uncertainty must be stated plainly"
        );
    }

    /// Test 42
    #[test]
    fn a_profile_finding_carries_machine_readable_values() {
        let tree = TempTree::new("profiles-measurements");
        let destination = tree.path().join("patches");
        let assessed = assess(&destination, Some(&rw_table()));
        let findings = findings_from_emulator_profiles(&report(vec![assessed]));
        let measurements = &findings[0].measurements;
        assert_eq!(
            measurements.get("profile_kind"),
            Some(&Measurement::Text("Native".to_string()))
        );
        assert_eq!(
            measurements.get("discovery_confidence"),
            Some(&Measurement::Text("documented native path".to_string()))
        );
        assert_eq!(
            measurements.get("writability_assessment"),
            Some(&Measurement::Text(
                WritabilityAssessment::MissingDestination
                    .label()
                    .to_string()
            ))
        );
        assert_eq!(
            measurements.get("filesystem_read_only"),
            Some(&Measurement::Flag(false))
        );
    }

    /// Test 43
    #[test]
    fn many_profiles_needing_attention_collapse_into_one_result() {
        let tree = TempTree::new("profiles-flood");
        let profiles: Vec<ProfileAssessment> = (0..MAX_INDIVIDUAL_PROFILE_FINDINGS + 5)
            .map(|index| {
                assess(
                    &tree.path().join(format!("patches-{index}")),
                    Some(&rw_table()),
                )
            })
            .collect();
        let findings = findings_from_emulator_profiles(&report(profiles));
        assert_eq!(
            findings.len(),
            1,
            "a dashboard must never be flooded with one result per profile"
        );
        assert_eq!(findings[0].id, "emulator_profile.multiple_need_attention");
        assert_eq!(
            findings[0].measurements.get("profiles_needing_attention"),
            Some(&Measurement::Integer(
                (MAX_INDIVIDUAL_PROFILE_FINDINGS + 5) as u64
            ))
        );
        assert!(
            findings[0]
                .evidence
                .iter()
                .any(|item| item.contains("and 5 more")),
            "the count that was left out must still be visible"
        );
    }

    /// Test 44
    #[test]
    fn exactly_the_limit_is_still_listed_individually() {
        let tree = TempTree::new("profiles-at-limit");
        let profiles: Vec<ProfileAssessment> = (0..MAX_INDIVIDUAL_PROFILE_FINDINGS)
            .map(|index| {
                assess(
                    &tree.path().join(format!("patches-{index}")),
                    Some(&rw_table()),
                )
            })
            .collect();
        assert_eq!(
            findings_from_emulator_profiles(&report(profiles)).len(),
            MAX_INDIVIDUAL_PROFILE_FINDINGS
        );
    }

    /// Test 45
    #[test]
    fn several_usable_profiles_with_none_selected_is_reported_as_ambiguous() {
        let tree = TempTree::new("profiles-ambiguous");
        let mut profiles = Vec::new();
        for index in 0..2 {
            let destination = tree.path().join(format!("patches-{index}"));
            fs::create_dir_all(&destination).expect("fixture");
            profiles.push(assess(&destination, Some(&rw_table())));
        }
        let findings = findings_from_emulator_profiles(&report(profiles));
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].id, "emulator_profile.ambiguous_preferred_pcsx2_profile",
            "the id names the emulator, so two emulators' results cannot merge"
        );
        assert_eq!(findings[0].severity, DoctorSeverity::Info);
    }

    /// Test 46
    #[test]
    fn a_selected_profile_removes_the_ambiguity() {
        let tree = TempTree::new("profiles-selected");
        let mut profiles = Vec::new();
        for index in 0..2 {
            let destination = tree.path().join(format!("patches-{index}"));
            fs::create_dir_all(&destination).expect("fixture");
            profiles.push(assess(&destination, Some(&rw_table())));
        }
        profiles[0].preferred = Some(true);
        assert!(
            findings_from_emulator_profiles(&report(profiles)).is_empty(),
            "once a profile is remembered there is nothing to ask about"
        );
    }

    /// Test 47
    #[test]
    fn a_failed_discovery_is_reported_as_unavailable_not_as_no_profiles() {
        let discoveries = ProfileDiscoveries {
            dolphin: None,
            dolphin_error: Some("HOME is not set".to_string()),
            pcsx2: None,
            pcsx2_error: None,
            xenia: None,
            xenia_error: None,
            ppsspp: None,
            ppsspp_error: None,
            duckstation: None,
            duckstation_error: None,
            xemu: None,
            xemu_error: None,
            rpcs3: None,
            rpcs3_error: None,
            preferred_dolphin: None,
            preferred_pcsx2: None,
            preferred_xenia: None,
            preferred_ppsspp: None,
            preferred_duckstation: None,
            preferred_xemu: None,
            preferred_rpcs3: None,
        };
        let assessed = assess_emulator_profiles(&discoveries, None);
        assert!(assessed.profiles.is_empty());
        assert_eq!(assessed.unavailable.len(), 1);
        assert_eq!(assessed.unavailable[0].0, EmulatorKind::Dolphin);
        let not_checked = not_checked_from_emulator_profiles(&assessed);
        assert!(
            not_checked
                .iter()
                .any(|item| item.name.contains("Dolphin") && item.reason.contains("HOME")),
            "a failed discovery must be stated, never presented as a clean result"
        );
    }

    #[test]
    fn doctor_sees_ppsspp() {
        use crate::patch_manager::{PpssppInstallationType, PpssppProfile, PpssppProfileScope};
        let tree = TempTree::new("profiles-doctor-sees-ppsspp");
        let root = tree.path().join("ppsspp");
        let ppsspp_profile = PpssppProfile {
            profile_id: "ppsspp-native".to_string(),
            installation_type: PpssppInstallationType::Native,
            scope: PpssppProfileScope::User,
            configuration_path: root.clone(),
            provenance: "documented native path",
            eligible: true,
            blockers: Vec::new(),
            executable_candidates: Vec::new(),
            memstick_path: root.join("PSP"),
            system_path: root.join("PSP/SYSTEM"),
            global_config_path: root.join("PSP/SYSTEM/ppsspp.ini"),
            cheats_path: root.join("PSP/Cheats"),
            textures_path: root.join("PSP/Textures"),
            savedata_path: root.join("PSP/SAVEDATA"),
            game_path: root.join("PSP/GAME"),
            state_path: root.join("PSP/PSP/STATE"),
        };
        let discovery = PpssppProfileDiscovery {
            profiles: vec![ppsspp_profile],
            warnings: Vec::new(),
            complete: true,
        };
        let discoveries = ProfileDiscoveries {
            dolphin: None,
            dolphin_error: None,
            pcsx2: None,
            pcsx2_error: None,
            xenia: None,
            xenia_error: None,
            ppsspp: Some(&discovery),
            ppsspp_error: None,
            duckstation: None,
            duckstation_error: None,
            xemu: None,
            xemu_error: None,
            rpcs3: None,
            rpcs3_error: None,
            preferred_dolphin: None,
            preferred_pcsx2: None,
            preferred_xenia: None,
            preferred_ppsspp: None,
            preferred_duckstation: None,
            preferred_xemu: None,
            preferred_rpcs3: None,
        };
        let assessed = assess_emulator_profiles(&discoveries, Some(&rw_table()));
        assert_eq!(assessed.profiles.len(), 1);
        let profile = &assessed.profiles[0];
        assert_eq!(profile.emulator, EmulatorKind::Ppsspp);
        assert_eq!(profile.emulator.label(), "PPSSPP");
        assert_eq!(profile.profile_id, "ppsspp-native");
        assert_eq!(
            profile.destination_path.display,
            root.join("PSP/Cheats").display().to_string()
        );
    }

    #[test]
    fn doctor_sees_duckstation() {
        use crate::patch_manager::{DuckStationInstallationType, DuckStationProfile};
        let tree = TempTree::new("profiles-doctor-sees-duckstation");
        let root = tree.path().join("duckstation");
        let duckstation_profile = DuckStationProfile {
            profile_id: "duckstation-native".to_string(),
            installation_type: DuckStationInstallationType::Native,
            configuration_path: root.clone(),
            provenance: "XDG_CONFIG_HOME DuckStation directory",
            eligible: true,
            blocker: None,
            executable_candidates: Vec::new(),
            global_config_path: root.join("settings.ini"),
            game_settings_path: root.join("gamesettings.ini"),
            cheats_path: root.join("cheats"),
            patches_path: root.join("patches"),
            textures_path: root.join("textures"),
            bios_path: root.join("bios"),
            memory_cards_path: root.join("memcards"),
            save_states_path: root.join("savestates"),
        };
        let discovery = DuckStationProfileDiscovery {
            profiles: vec![duckstation_profile],
            complete: true,
        };
        let discoveries = ProfileDiscoveries {
            dolphin: None,
            dolphin_error: None,
            pcsx2: None,
            pcsx2_error: None,
            xenia: None,
            xenia_error: None,
            ppsspp: None,
            ppsspp_error: None,
            duckstation: Some(&discovery),
            duckstation_error: None,
            xemu: None,
            xemu_error: None,
            rpcs3: None,
            rpcs3_error: None,
            preferred_dolphin: None,
            preferred_pcsx2: None,
            preferred_xenia: None,
            preferred_ppsspp: None,
            preferred_duckstation: None,
            preferred_xemu: None,
            preferred_rpcs3: None,
        };
        let assessed = assess_emulator_profiles(&discoveries, Some(&rw_table()));
        assert_eq!(assessed.profiles.len(), 1);
        let profile = &assessed.profiles[0];
        assert_eq!(profile.emulator, EmulatorKind::DuckStation);
        assert_eq!(profile.emulator.label(), "DuckStation");
        assert_eq!(profile.profile_id, "duckstation-native");
        assert_eq!(
            profile.destination_path.display,
            root.join("cheats").display().to_string()
        );
    }

    #[test]
    fn doctor_projects_writable_ppsspp_and_duckstation_profiles_for_inspection() {
        let tree = TempTree::new("profiles-inspection-projection");
        let ppsspp_root = tree.path().join("ppsspp");
        let ppsspp_cheats = ppsspp_root.join("PSP/Cheats");
        let duckstation_root = tree.path().join("duckstation");
        let duckstation_cheats = duckstation_root.join("cheats");
        fs::create_dir_all(&ppsspp_cheats).expect("fixture");
        fs::create_dir_all(&duckstation_cheats).expect("fixture");

        let ppsspp = assess_one(
            EmulatorKind::Ppsspp,
            "ppsspp-native".to_string(),
            "Native".to_string(),
            "User".to_string(),
            "documented native path".to_string(),
            true,
            Vec::new(),
            &ppsspp_root,
            &ppsspp_cheats,
            None,
            Some(&rw_table()),
        );
        let duckstation = assess_one(
            EmulatorKind::DuckStation,
            "duckstation-native".to_string(),
            "Native".to_string(),
            "N/A".to_string(),
            "discovered configuration directory".to_string(),
            true,
            Vec::new(),
            &duckstation_root,
            &duckstation_cheats,
            None,
            Some(&rw_table()),
        );

        let findings = findings_from_emulator_profiles(&report(vec![ppsspp, duckstation]));
        assert_eq!(findings.len(), 2);
        for (id, emulator, profile, configuration_path, cheat_destination) in [
            (
                "emulator_profile.ppsspp_inspected",
                "PPSSPP",
                "ppsspp-native",
                ppsspp_root.display().to_string(),
                ppsspp_cheats.display().to_string(),
            ),
            (
                "emulator_profile.duckstation_inspected",
                "DuckStation",
                "duckstation-native",
                duckstation_root.display().to_string(),
                duckstation_cheats.display().to_string(),
            ),
        ] {
            let finding = findings
                .iter()
                .find(|finding| finding.id == id)
                .expect("finding");
            assert_eq!(finding.severity, DoctorSeverity::Info);
            assert!(finding.evidence.contains(&format!("Emulator: {emulator}")));
            assert!(finding.evidence.contains(&format!("Profile: {profile}")));
            assert!(
                finding
                    .evidence
                    .contains(&format!("Configuration path: {configuration_path}"))
            );
            assert!(
                finding
                    .evidence
                    .contains(&format!("Cheat destination: {cheat_destination}"))
            );
            assert_eq!(
                finding.measurements.get("eligible"),
                Some(&Measurement::Flag(true))
            );
        }
    }

    #[test]
    fn ppsspp_and_duckstation_never_become_managed_scan_targets() {
        use crate::patch_manager::{
            DuckStationInstallationType, PpssppInstallationType, PpssppProfileScope,
        };
        let tree = TempTree::new("profiles-no-managed-block");
        let ppsspp_destination = tree.path().join("ppsspp-cheats");
        let duckstation_destination = tree.path().join("duckstation-cheats");
        fs::create_dir_all(&ppsspp_destination).expect("fixture");
        fs::create_dir_all(&duckstation_destination).expect("fixture");
        let ppsspp_assessed = assess_one(
            EmulatorKind::Ppsspp,
            "ppsspp-native".to_string(),
            format!("{:?}", PpssppInstallationType::Native),
            format!("{:?}", PpssppProfileScope::User),
            "documented native path".to_string(),
            true,
            Vec::new(),
            ppsspp_destination.parent().unwrap(),
            &ppsspp_destination,
            None,
            Some(&rw_table()),
        );
        let duckstation_assessed = assess_one(
            EmulatorKind::DuckStation,
            "duckstation-native".to_string(),
            format!("{:?}", DuckStationInstallationType::Native),
            "N/A".to_string(),
            "discovered configuration directory".to_string(),
            true,
            Vec::new(),
            duckstation_destination.parent().unwrap(),
            &duckstation_destination,
            None,
            Some(&rw_table()),
        );
        let targets = managed_scan_targets(&report(vec![ppsspp_assessed, duckstation_assessed]));
        assert!(
            targets.is_empty(),
            "neither adapter has managed-block support yet - see the module doc comment"
        );
    }

    /// Test 48
    #[test]
    fn a_flatpak_profile_always_states_the_sandbox_caveat() {
        let tree = TempTree::new("profiles-flatpak");
        let destination = tree.path().join("patches");
        fs::create_dir_all(&destination).expect("fixture");
        let mut assessed = assess(&destination, Some(&rw_table()));
        assessed.profile_kind = "Flatpak".to_string();
        let not_checked = not_checked_from_emulator_profiles(&report(vec![assessed]));
        assert!(
            not_checked
                .iter()
                .any(|item| item.name == "Flatpak sandbox write permission"),
            "metadata cannot see a portal, and that limit must be admitted"
        );
    }

    /// Test 49
    #[test]
    fn incomplete_discovery_is_declared_rather_than_hidden() {
        let mut assessed = report(Vec::new());
        assessed.discovery_incomplete = true;
        assert!(
            not_checked_from_emulator_profiles(&assessed)
                .iter()
                .any(|item| item.name.contains("Complete emulator profile discovery"))
        );
    }

    /// Test 50
    #[test]
    fn the_json_summary_exposes_the_values_a_script_needs() {
        let tree = TempTree::new("profiles-json");
        let destination = tree.path().join("patches");
        fs::create_dir_all(&destination).expect("fixture");
        let summary = profile_json_summary(&assess(&destination, Some(&rw_table())));
        for key in [
            "profile_kind",
            "discovery_confidence",
            "writability_assessment",
            "filesystem_read_only",
            "destination_exists",
            "finding_id",
        ] {
            assert!(summary.get(key).is_some(), "`{key}` is missing from --json");
        }
        assert_eq!(summary["filesystem_read_only"], serde_json::json!(false));
    }

    /// Test 51
    #[test]
    fn a_destination_file_contributes_its_parent_directory_to_the_storage_check() {
        let tree = TempTree::new("profiles-destinations");
        let file = tree.path().join("GameSettings/GALE01.ini");
        fs::create_dir_all(file.parent().expect("parent")).expect("fixture");
        fs::write(&file, b"[Gecko]\n").expect("fixture");
        let assessed = assess(&file, Some(&rw_table()));
        let directories = profile_destination_directories(&report(vec![assessed]));
        assert_eq!(directories, vec![tree.path().join("GameSettings")]);
    }

    /// Test 52
    #[test]
    fn pcsx2_and_dolphin_profiles_become_managed_scan_targets() {
        let tree = TempTree::new("profiles-targets");
        let pcsx2 = tree.path().join("patches");
        fs::create_dir_all(&pcsx2).expect("fixture");
        let mut dolphin = assess(&pcsx2, Some(&rw_table()));
        dolphin.emulator = EmulatorKind::Dolphin;
        let targets =
            managed_scan_targets(&report(vec![assess(&pcsx2, Some(&rw_table())), dolphin]));
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].destination_root, pcsx2);
        assert_eq!(targets[1].destination_root, pcsx2);
        assert_eq!(
            targets
                .iter()
                .map(|target| target.format)
                .collect::<Vec<_>>(),
            vec![
                ManagedFormat::Pcsx2Pnach,
                ManagedFormat::DolphinGameSettings
            ]
        );
    }

    /// Read-only proof 4: assessing every profile in a real tree changes
    /// nothing - no profile, no GameSettings file, no cheat, no test file.
    #[test]
    fn assessing_profiles_leaves_the_tree_byte_for_byte_unchanged() {
        let tree = TempTree::new("profiles-read-only-proof");
        let existing = tree.path().join("patches");
        fs::create_dir_all(&existing).expect("fixture");
        fs::write(existing.join("SLUS-20946.pnach"), b"// user's own file\n").expect("fixture");
        let missing = tree.path().join("absent-patches");
        let before = snapshot_tree(tree.path());

        let assessed = report(vec![
            assess(&existing, Some(&rw_table())),
            assess(&missing, Some(&rw_table())),
        ]);
        let _ = findings_from_emulator_profiles(&assessed);
        let _ = not_checked_from_emulator_profiles(&assessed);
        let _ = profile_destination_directories(&assessed);
        let _ = managed_scan_targets(&assessed);

        assert_eq!(
            snapshot_tree(tree.path()),
            before,
            "a profile assessment must not create a profile, a settings file or a probe"
        );
    }

    /// Read-only proof 5: this module contains no write, permission-changing
    /// or process-spawning call.
    #[test]
    fn this_module_contains_no_mutating_call() {
        let whole = include_str!("profiles.rs");
        let source = whole
            .split_once("#[cfg(test)]")
            .expect("this file ends with its own test module")
            .0;
        for forbidden in [
            "fs::write",
            "fs::create_dir",
            "fs::remove_",
            "fs::set_permissions",
            "File::create",
            "OpenOptions",
            "libc::access",
            "libc::chmod",
            "Command",
            "ureq",
            "install_cheat",
            "apply_shared",
        ] {
            assert!(
                !source.contains(forbidden),
                "`{forbidden}` must never appear in a read-only diagnostic module"
            );
        }
    }

    /// Read-only proof 6: no finding from this family offers a repair, so no
    /// Delete, Clean, Repair or Fix button can appear for one.
    #[test]
    fn no_profile_finding_offers_a_repair() {
        let tree = TempTree::new("profiles-no-repair");
        let table = read_only_table(tree.path());
        let destination = tree.path().join("patches");
        fs::create_dir_all(&destination).expect("fixture");
        let assessed = report(vec![
            assess(&destination, Some(&table)),
            assess(&tree.path().join("absent"), Some(&rw_table())),
        ]);
        for finding in findings_from_emulator_profiles(&assessed) {
            assert!(
                finding.repair.is_none(),
                "{} must not offer a repair in a diagnostic-only milestone",
                finding.id
            );
            assert!(
                finding.recovery.is_none(),
                "{} must not advertise a repair elsewhere either",
                finding.id
            );
        }
    }

    /// Test 97
    #[test]
    fn two_emulators_with_ambiguous_profiles_stay_two_separate_results() {
        let tree = TempTree::new("profiles-two-emulators");
        let mut profiles = Vec::new();
        for emulator in [EmulatorKind::Dolphin, EmulatorKind::Pcsx2] {
            for index in 0..2 {
                let destination = tree
                    .path()
                    .join(format!("{}-{index}", emulator.label().to_lowercase()));
                fs::create_dir_all(&destination).expect("fixture");
                let mut assessed = assess(&destination, Some(&rw_table()));
                assessed.emulator = emulator;
                profiles.push(assessed);
            }
        }
        let ids: Vec<String> = findings_from_emulator_profiles(&report(profiles))
            .into_iter()
            .map(|finding| finding.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "emulator_profile.ambiguous_preferred_dolphin_profile".to_string(),
                "emulator_profile.ambiguous_preferred_pcsx2_profile".to_string(),
            ],
            "a live scan merged these into one result when they shared an id"
        );
    }

    /// Doctor discovers and assesses PPSSPP and DuckStation profiles through
    /// their existing adapter discovery, with explicit call-supplied roots.
    #[test]
    fn doctor_discovers_ppsspp_and_duckstation_profiles() {
        let tree = TempTree::new("profiles-ppsspp-duckstation");
        let ppsspp_root = tree.path().join("ppsspp");
        fs::create_dir_all(ppsspp_root.join("PSP/SYSTEM")).expect("fixture");
        fs::write(ppsspp_root.join("PSP/SYSTEM/ppsspp.ini"), "").expect("fixture");
        let duck_root = tree.path().join("duckstation");
        fs::create_dir_all(&duck_root).expect("fixture");
        fs::write(duck_root.join("settings.ini"), "").expect("fixture");

        let roots = roots_for_ppsspp_and_duckstation(
            tree.path(),
            std::slice::from_ref(&ppsspp_root),
            std::slice::from_ref(&duck_root),
        );
        let ppsspp = discover_ppsspp_profiles(&roots.ppsspp);
        let duckstation = discover_duckstation_profiles(&roots.duckstation);

        let discoveries = ProfileDiscoveries {
            ppsspp: Some(&ppsspp),
            duckstation: Some(&duckstation),
            ..ProfileDiscoveries::default()
        };
        let assessed = assess_emulator_profiles(&discoveries, Some(&rw_table()));

        let kinds: Vec<EmulatorKind> = assessed
            .profiles
            .iter()
            .map(|profile| profile.emulator)
            .collect();
        assert!(kinds.contains(&EmulatorKind::Ppsspp));
        assert!(kinds.contains(&EmulatorKind::DuckStation));

        // Both profiles are eligible and assessed against their cheat-write
        // destination directory.
        let ppsspp_assessed = assessed
            .profiles
            .iter()
            .find(|profile| profile.emulator == EmulatorKind::Ppsspp)
            .expect("PPSSPP profile assessed");
        assert!(ppsspp_assessed.eligible);
        assert_eq!(
            ppsspp_assessed.destination_path.display,
            ppsspp_root.join("PSP/CHEATS").display().to_string()
        );
        let duck_assessed = assessed
            .profiles
            .iter()
            .find(|profile| profile.emulator == EmulatorKind::DuckStation)
            .expect("DuckStation profile assessed");
        assert!(duck_assessed.eligible);
        assert_eq!(
            duck_assessed.destination_path.display,
            duck_root.join("cheats").display().to_string()
        );
    }

    /// A small bag of already-built discovery-root structs for Doctor's
    /// PPSSPP and DuckStation discovery, so the test drives both without
    /// touching environment variables.
    struct PpssppDuckstationRoots {
        ppsspp: PpssppProfileDiscoveryRoots,
        duckstation: DuckStationProfileDiscoveryRoots,
    }

    /// Builds explicit-roots discovery for PPSSPP and DuckStation from a
    /// caller-supplied base directory and separate explicit configuration
    /// roots for each adapter. It never touches `HOME`/`XDG`.
    fn roots_for_ppsspp_and_duckstation(
        base: &Path,
        ppsspp_explicit: &[PathBuf],
        duckstation_explicit: &[PathBuf],
    ) -> PpssppDuckstationRoots {
        let home = base.join("home");
        let xdg_config_home = base.join("config");
        let xdg_data_home = base.join("data");
        PpssppDuckstationRoots {
            ppsspp: PpssppProfileDiscoveryRoots {
                home: home.clone(),
                xdg_config_home: xdg_config_home.clone(),
                xdg_data_home: xdg_data_home.clone(),
                explicit_configuration_roots: ppsspp_explicit.to_vec(),
                portable_configuration_roots: Vec::new(),
                explicit_executables: Vec::new(),
                known_version_outputs: std::collections::BTreeMap::new(),
                appimage_directory: None,
            },
            duckstation: DuckStationProfileDiscoveryRoots {
                home,
                xdg_config_home,
                xdg_data_home,
                xdg_config_home_explicit: false,
                explicit_configuration_roots: duckstation_explicit.to_vec(),
                portable_configuration_roots: Vec::new(),
                explicit_executables: Vec::new(),
                known_version_outputs: std::collections::BTreeMap::new(),
                appimage_directory: None,
            },
        }
    }

    // -----------------------------------------------------------------
    // xemu / Xenia launch readiness
    // -----------------------------------------------------------------
    //
    // xemu's own `resolve_xemu_native_launch_binding` only ever authorizes
    // an executable candidate whose installation type is `Native`, and
    // `discover_xemu_profiles`'s own executable discovery only ever
    // classifies a candidate `Native` when it is found by searching the
    // current process's real `PATH` - exactly the same PATH/global-env
    // testing limitation already established for xemu/PPSSPP/DuckStation's
    // own execution-layer tests (see `xemu_execution::tests`'s own module
    // doc comment). Mutating this test binary's real `PATH` to fabricate a
    // match would race every other concurrently running test that also
    // reads it, so - following that same precedent - the *finding-shaping*
    // logic below (severity, wording, measurements) is tested directly
    // against hand-built [`XemuReadinessAssessment`] values, never through
    // a real `Native` binding. The one gatherer-wiring test
    // (`xemu_readiness_is_only_assessed_for_eligible_profiles`) proves
    // `assess_xemu_readiness` itself reads real profile/health metadata and
    // filters ineligible profiles correctly, without depending on a
    // genuine `Native` executable ever being reachable in a test binary.
    //
    // Xenia has no such limitation: `resolve_xenia_launch_binding` never
    // searches `$PATH` at all, so its readiness tests below use real,
    // fully genuine fixtures throughout (see `xenia_execution::tests`'s own
    // module doc comment for the same observation at the execution layer).

    use crate::patch_manager::{
        Rpcs3FirmwareStatus, XemuGameIdMapping, XemuHealth, XemuProfileDiscoveryRoots,
        discover_xemu_profiles, discover_xenia_profiles,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn write_executable(path: &Path, contents: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn healthy_xemu_health() -> XemuHealth {
        XemuHealth {
            detected: true,
            config_readable: true,
            mcpx: XemuSystemFileState::Present,
            flash_bios: XemuSystemFileState::Present,
            eeprom: XemuSystemFileState::Present,
            hdd: XemuSystemFileState::Present,
            game_profile_mapping: XemuGameIdMapping::Unavailable,
            warnings: Vec::new(),
        }
    }

    fn xemu_assessment(
        executable: Option<&str>,
        binding_problem: Option<&str>,
        health: XemuHealth,
    ) -> XemuReadinessAssessment {
        XemuReadinessAssessment {
            profile_id: "xemu-native".to_string(),
            root_path: EncodedPath::from_path(Path::new("/home/user/.config/xemu")),
            executable: executable.map(|path| EncodedPath::from_path(Path::new(path))),
            binding_problem: binding_problem.map(str::to_string),
            mcpx: health.mcpx,
            flash_bios: health.flash_bios,
            eeprom: health.eeprom,
            hdd: health.hdd,
        }
    }

    fn xemu_finding(assessment: XemuReadinessAssessment) -> Finding {
        let mut findings = findings_from_xemu_readiness(&[assessment]);
        assert_eq!(findings.len(), 1);
        findings.remove(0)
    }

    #[test]
    fn xemu_ready_profile_is_reported_healthy() {
        let finding = xemu_finding(xemu_assessment(
            Some("/home/user/.config/xemu/xemu"),
            None,
            healthy_xemu_health(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Info);
        assert_eq!(finding.id, "emulator_readiness.xemu");
        assert_eq!(finding.title, "xemu is ready to launch");
        assert_eq!(
            finding.measurements.get("ready"),
            Some(&Measurement::Flag(true))
        );
        assert_eq!(
            finding.measurements.get("mcpx"),
            Some(&Measurement::text("present"))
        );
    }

    #[test]
    fn xemu_missing_mcpx_is_reported_by_name() {
        let mut health = healthy_xemu_health();
        health.mcpx = XemuSystemFileState::Missing;
        let finding = xemu_finding(xemu_assessment(
            Some("/home/user/.config/xemu/xemu"),
            None,
            health,
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(finding.explanation.contains("MCPX boot ROM is missing"));
        assert_eq!(
            finding.measurements.get("mcpx"),
            Some(&Measurement::text("missing"))
        );
        assert_eq!(
            finding.measurements.get("flash_bios"),
            Some(&Measurement::text("present"))
        );
    }

    #[test]
    fn xemu_missing_flash_bios_is_reported_by_name() {
        let mut health = healthy_xemu_health();
        health.flash_bios = XemuSystemFileState::Missing;
        let finding = xemu_finding(xemu_assessment(
            Some("/home/user/.config/xemu/xemu"),
            None,
            health,
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(finding.explanation.contains("flash BIOS is missing"));
        assert_eq!(
            finding.measurements.get("flash_bios"),
            Some(&Measurement::text("missing"))
        );
    }

    #[test]
    fn xemu_missing_eeprom_is_reported_by_name() {
        let mut health = healthy_xemu_health();
        health.eeprom = XemuSystemFileState::Missing;
        let finding = xemu_finding(xemu_assessment(
            Some("/home/user/.config/xemu/xemu"),
            None,
            health,
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(finding.explanation.contains("EEPROM is missing"));
        assert_eq!(
            finding.measurements.get("eeprom"),
            Some(&Measurement::text("missing"))
        );
    }

    #[test]
    fn xemu_missing_hdd_is_reported_by_name() {
        let mut health = healthy_xemu_health();
        health.hdd = XemuSystemFileState::Missing;
        let finding = xemu_finding(xemu_assessment(
            Some("/home/user/.config/xemu/xemu"),
            None,
            health,
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(finding.explanation.contains("HDD image is missing"));
        assert_eq!(
            finding.measurements.get("hdd"),
            Some(&Measurement::text("missing"))
        );
    }

    #[test]
    fn xemu_unsafe_executable_is_reported() {
        let finding = xemu_finding(xemu_assessment(
            None,
            Some("the xemu executable is a symlink or not a regular file"),
            healthy_xemu_health(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("symlink or not a regular file")
        );
        assert_eq!(
            finding.measurements.get("executable_found"),
            Some(&Measurement::Flag(false))
        );
    }

    #[test]
    fn xemu_ambiguous_executables_are_reported() {
        let finding = xemu_finding(xemu_assessment(
            None,
            Some("more than one native xemu executable was found and none is preferred"),
            healthy_xemu_health(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("more than one native xemu executable")
        );
    }

    /// Proves the real, I/O-touching gatherer: an ineligible profile
    /// contributes no assessment at all (its own writability finding
    /// already covers it), and an eligible one is genuinely inspected via
    /// [`inspect_xemu_game`]'s real, read-only firmware detection - not
    /// through any hand-built value.
    #[test]
    fn xemu_readiness_is_only_assessed_for_eligible_profiles() {
        let tree = TempTree::new("xemu-gatherer");
        // A native profile root with real system files and a real
        // xemu.toml - genuinely eligible and genuinely inspected, even
        // though no `Native`-classified executable can be placed here
        // without touching `$PATH` (see this section's own doc comment).
        let root = tree.path().join("config/xemu/xemu");
        fs::create_dir_all(root.join("system")).unwrap();
        fs::write(root.join("system/mcpx.bin"), b"mcpx").unwrap();
        fs::write(
            root.join("xemu.toml"),
            "[sys.files]\nbootrom_path = 'system/mcpx.bin'\n",
        )
        .unwrap();
        let discovery = discover_xemu_profiles(&XemuProfileDiscoveryRoots {
            home: tree.path().join("home"),
            xdg_config_home: tree.path().join("config"),
            xdg_data_home: tree.path().join("data-unused"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: std::collections::BTreeMap::new(),
            appimage_directory: None,
        });
        assert_eq!(discovery.profiles.len(), 1, "{discovery:?}");
        assert!(discovery.profiles[0].eligible);
        let assessments = assess_xemu_readiness(Some(&discovery));
        assert_eq!(assessments.len(), 1);
        assert_eq!(assessments[0].mcpx, XemuSystemFileState::Present);
        assert_eq!(
            assessments[0].flash_bios,
            XemuSystemFileState::NotConfigured
        );
        // No `Native` executable is reachable here (see the doc comment
        // above), so the binding is genuinely absent - proving the
        // gatherer reports that honestly rather than fabricating success.
        assert!(assessments[0].executable.is_none());
        assert!(assessments[0].binding_problem.is_some());

        assert!(assess_xemu_readiness(None).is_empty());
    }

    /// A real, eligible Xenia profile: `xenia-canary.config.toml` marker plus
    /// a native `xenia_canary` executable directly beneath `root`.
    fn xenia_ready_profile(root: &Path) -> XeniaProfileDiscovery {
        fs::write(root.join("xenia-canary.config.toml"), b"").unwrap();
        write_executable(&root.join("xenia_canary"), b"#!/bin/sh\nexit 0\n");
        discover_xenia_profiles(&crate::patch_manager::XeniaProfileDiscoveryRoots {
            explicit_configuration_roots: vec![root.to_path_buf()],
        })
    }

    #[test]
    fn xenia_ready_with_native_linux_binary_is_reported_healthy() {
        let tree = TempTree::new("xenia-ready");
        let discovery = xenia_ready_profile(tree.path());
        let assessments = assess_xenia_readiness(Some(&discovery));
        let findings = findings_from_xenia_readiness(&assessments);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, DoctorSeverity::Info);
        assert_eq!(findings[0].id, "emulator_readiness.xenia");
        assert_eq!(
            findings[0].measurements.get("ready"),
            Some(&Measurement::Flag(true))
        );
        assert_eq!(
            findings[0].measurements.get("windows_exe_present"),
            Some(&Measurement::Flag(false))
        );
    }

    #[test]
    fn xenia_windows_exe_only_is_reported_as_not_launchable_natively() {
        let tree = TempTree::new("xenia-exe-only");
        fs::write(tree.path().join("xenia-canary.config.toml"), b"").unwrap();
        fs::write(tree.path().join("xenia_canary.exe"), b"MZ fake pe").unwrap();
        let discovery =
            discover_xenia_profiles(&crate::patch_manager::XeniaProfileDiscoveryRoots {
                explicit_configuration_roots: vec![tree.path().to_path_buf()],
            });
        let assessments = assess_xenia_readiness(Some(&discovery));
        let findings = findings_from_xenia_readiness(&assessments);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, DoctorSeverity::Warning);
        assert_eq!(
            findings[0].measurements.get("windows_exe_present"),
            Some(&Measurement::Flag(true))
        );
        assert_eq!(
            findings[0].measurements.get("executable_found"),
            Some(&Measurement::Flag(false))
        );
        assert!(
            findings[0]
                .explanation
                .contains("cannot be launched natively on Linux")
        );
        // Never claims the .exe is a valid native executable anywhere.
        assert!(!findings[0].explanation.contains("A native Linux Xenia"));
    }

    #[test]
    fn xenia_missing_native_binding_is_reported() {
        let tree = TempTree::new("xenia-missing-binding");
        fs::write(tree.path().join("xenia-canary.config.toml"), b"").unwrap();
        let discovery =
            discover_xenia_profiles(&crate::patch_manager::XeniaProfileDiscoveryRoots {
                explicit_configuration_roots: vec![tree.path().to_path_buf()],
            });
        let assessments = assess_xenia_readiness(Some(&discovery));
        let findings = findings_from_xenia_readiness(&assessments);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, DoctorSeverity::Warning);
        assert_eq!(
            findings[0].measurements.get("windows_exe_present"),
            Some(&Measurement::Flag(false))
        );
        assert!(
            findings[0]
                .explanation
                .contains("no native Linux Xenia executable")
        );
    }

    #[test]
    fn xenia_non_executable_binary_is_reported() {
        let tree = TempTree::new("xenia-not-executable");
        fs::write(tree.path().join("xenia-canary.config.toml"), b"").unwrap();
        fs::write(tree.path().join("xenia_canary"), b"#!/bin/sh\nexit 0\n").unwrap();
        let discovery =
            discover_xenia_profiles(&crate::patch_manager::XeniaProfileDiscoveryRoots {
                explicit_configuration_roots: vec![tree.path().to_path_buf()],
            });
        let assessments = assess_xenia_readiness(Some(&discovery));
        let findings = findings_from_xenia_readiness(&assessments);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, DoctorSeverity::Warning);
        assert!(
            findings[0]
                .explanation
                .contains("does not have the execute permission")
        );
    }

    #[test]
    fn xenia_ambiguous_native_candidates_are_reported() {
        let tree = TempTree::new("xenia-ambiguous");
        fs::write(tree.path().join("xenia-canary.config.toml"), b"").unwrap();
        write_executable(&tree.path().join("xenia_canary"), b"#!/bin/sh\nexit 0\n");
        write_executable(&tree.path().join("xenia"), b"#!/bin/sh\nexit 0\n");
        let discovery =
            discover_xenia_profiles(&crate::patch_manager::XeniaProfileDiscoveryRoots {
                explicit_configuration_roots: vec![tree.path().to_path_buf()],
            });
        let assessments = assess_xenia_readiness(Some(&discovery));
        let findings = findings_from_xenia_readiness(&assessments);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, DoctorSeverity::Warning);
        assert!(
            findings[0]
                .explanation
                .contains("more than one native Xenia executable")
        );
    }

    #[test]
    fn unrelated_emulator_rows_are_unchanged_by_xemu_and_xenia_readiness() {
        // Adding xemu/Xenia readiness must never alter another adapter's own
        // writability finding shape or wording.
        let tree = TempTree::new("unrelated-unchanged");
        let ppsspp_root = tree.path().join("ppsspp");
        fs::create_dir_all(ppsspp_root.join("PSP/SYSTEM")).unwrap();
        fs::write(ppsspp_root.join("PSP/SYSTEM/ppsspp.ini"), b"[General]\n").unwrap();
        let roots = roots_for_ppsspp_and_duckstation(tree.path(), &[], &[]);
        let discovery = discover_ppsspp_profiles(&PpssppProfileDiscoveryRoots {
            explicit_configuration_roots: vec![ppsspp_root.clone()],
            ..roots.ppsspp
        });
        let discoveries = ProfileDiscoveries {
            ppsspp: Some(&discovery),
            ..ProfileDiscoveries::default()
        };
        let assessed = assess_emulator_profiles(&discoveries, None);
        let findings = findings_from_emulator_profiles(&assessed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "emulator_profile.ppsspp_inspected");
        assert_eq!(findings[0].category, DoctorCategory::EmulatorProfiles);
        assert_eq!(findings[0].subsystem, DoctorSubsystem::EmulatorProfiles);
    }

    // -----------------------------------------------------------------
    // PPSSPP / RPCS3 launch readiness
    // -----------------------------------------------------------------
    //
    // Both `resolve_ppsspp_native_launch_binding` and
    // `resolve_rpcs3_native_launch_binding` share the exact same PATH/
    // global-env testing limitation already documented above for xemu:
    // only a `$PATH`-discovered executable is ever classified `Native`,
    // and `roots.explicit_executables` is deliberately classified
    // `Explicit` instead. So, following the same precedent, finding-shape
    // tests use hand-built assessments, and one real gatherer-wiring test
    // per emulator proves the I/O-touching `assess_*` function itself
    // reads real profile/firmware metadata and filters ineligible profiles
    // correctly.

    fn ppsspp_assessment(
        executable: Option<&str>,
        binding_problem: Option<&str>,
    ) -> PpssppReadinessAssessment {
        PpssppReadinessAssessment {
            profile_id: "ppsspp-native".to_string(),
            root_path: EncodedPath::from_path(Path::new("/home/user/.config/ppsspp")),
            executable: executable.map(|path| EncodedPath::from_path(Path::new(path))),
            binding_problem: binding_problem.map(str::to_string),
        }
    }

    fn ppsspp_finding(assessment: PpssppReadinessAssessment) -> Finding {
        let mut findings = findings_from_ppsspp_readiness(&[assessment]);
        assert_eq!(findings.len(), 1);
        findings.remove(0)
    }

    #[test]
    fn ppsspp_ready_native_executable_is_reported_healthy() {
        let finding = ppsspp_finding(ppsspp_assessment(
            Some("/home/user/.config/ppsspp/ppsspp"),
            None,
        ));
        assert_eq!(finding.severity, DoctorSeverity::Info);
        assert_eq!(finding.id, "emulator_readiness.ppsspp");
        assert_eq!(finding.title, "PPSSPP is ready to launch");
        assert_eq!(
            finding.measurements.get("ready"),
            Some(&Measurement::Flag(true))
        );
        // No firmware/BIOS requirement is ever invented for PPSSPP: no
        // firmware measurement exists, and the explanation never claims one
        // is needed (it may only ever say the opposite - that none is
        // required).
        assert!(!finding.measurements.contains_key("firmware"));
        assert!(
            !finding
                .explanation
                .to_lowercase()
                .contains("requires firmware")
        );
        assert!(
            !finding
                .explanation
                .to_lowercase()
                .contains("needs firmware")
        );
    }

    #[test]
    fn ppsspp_missing_executable_is_reported() {
        let finding = ppsspp_finding(ppsspp_assessment(
            None,
            Some("no native PPSSPP executable was found in the profile"),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert_eq!(finding.title, "PPSSPP is not ready to launch");
        assert!(
            finding
                .explanation
                .contains("no native PPSSPP executable was found")
        );
        assert_eq!(
            finding.measurements.get("executable_found"),
            Some(&Measurement::Flag(false))
        );
    }

    #[test]
    fn ppsspp_unsafe_executable_is_reported() {
        let finding = ppsspp_finding(ppsspp_assessment(
            None,
            Some("the PPSSPP executable is a symlink or not a regular file"),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("symlink or not a regular file")
        );
    }

    #[test]
    fn ppsspp_non_executable_binary_is_reported() {
        let finding = ppsspp_finding(ppsspp_assessment(
            None,
            Some("the PPSSPP executable does not have the execute permission set"),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("does not have the execute permission")
        );
    }

    #[test]
    fn ppsspp_ambiguous_executable_is_reported() {
        let finding = ppsspp_finding(ppsspp_assessment(
            None,
            Some("more than one native PPSSPP executable was found and none is preferred"),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("more than one native PPSSPP executable")
        );
    }

    /// Proves the real, I/O-touching gatherer: an ineligible profile
    /// contributes no assessment at all, and an eligible one is genuinely
    /// discovered - even though no `Native`-classified executable can be
    /// placed here without touching `$PATH` (see this section's own doc
    /// comment), which also proves "missing profile" and "unusable
    /// profile" are represented honestly (no assessment at all) rather
    /// than as a fabricated success.
    #[test]
    fn ppsspp_readiness_is_only_assessed_for_eligible_profiles() {
        let tree = TempTree::new("ppsspp-gatherer");
        let root = tree.path().join("config/ppsspp");
        fs::create_dir_all(root.join("PSP/SYSTEM")).unwrap();
        fs::write(root.join("PSP/SYSTEM/ppsspp.ini"), b"[General]\n").unwrap();
        let discovery = discover_ppsspp_profiles(&PpssppProfileDiscoveryRoots {
            home: tree.path().join("home"),
            xdg_config_home: tree.path().join("config"),
            xdg_data_home: tree.path().join("data-unused"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: std::collections::BTreeMap::new(),
            appimage_directory: None,
        });
        assert_eq!(discovery.profiles.len(), 1, "{discovery:?}");
        assert!(discovery.profiles[0].eligible);
        let assessments = assess_ppsspp_readiness(Some(&discovery));
        assert_eq!(assessments.len(), 1);
        // No `Native` executable is reachable here, so the binding is
        // genuinely absent - the gatherer reports that honestly.
        assert!(assessments[0].executable.is_none());
        assert!(assessments[0].binding_problem.is_some());

        // No discovery at all (e.g. a missing/unreadable profile root
        // upstream) is represented as no assessments, never fabricated.
        assert!(assess_ppsspp_readiness(None).is_empty());

        // An ineligible profile (no PSP/SYSTEM/ppsspp.ini evidence)
        // contributes nothing here either - it is already covered by the
        // generic writability finding's own "adapter blocker" evidence.
        let ineligible_root = tree.path().join("config-ineligible/ppsspp");
        fs::create_dir_all(&ineligible_root).unwrap();
        let ineligible_discovery = discover_ppsspp_profiles(&PpssppProfileDiscoveryRoots {
            home: tree.path().join("home"),
            xdg_config_home: tree.path().join("config-ineligible"),
            xdg_data_home: tree.path().join("data-unused-2"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: std::collections::BTreeMap::new(),
            appimage_directory: None,
        });
        assert_eq!(ineligible_discovery.profiles.len(), 1);
        assert!(!ineligible_discovery.profiles[0].eligible);
        assert!(assess_ppsspp_readiness(Some(&ineligible_discovery)).is_empty());
    }

    fn healthy_rpcs3_firmware() -> Rpcs3FirmwareStatus {
        Rpcs3FirmwareStatus::Present(None)
    }

    fn rpcs3_assessment(
        executable: Option<&str>,
        binding_problem: Option<&str>,
        firmware: Rpcs3FirmwareStatus,
    ) -> Rpcs3ReadinessAssessment {
        Rpcs3ReadinessAssessment {
            profile_id: "rpcs3-native".to_string(),
            root_path: EncodedPath::from_path(Path::new("/home/user/.config/rpcs3")),
            executable: executable.map(|path| EncodedPath::from_path(Path::new(path))),
            binding_problem: binding_problem.map(str::to_string),
            firmware: rpcs3_firmware_readiness(&firmware),
        }
    }

    fn rpcs3_finding(assessment: Rpcs3ReadinessAssessment) -> Finding {
        let mut findings = findings_from_rpcs3_readiness(&[assessment]);
        assert_eq!(findings.len(), 1);
        findings.remove(0)
    }

    #[test]
    fn rpcs3_fully_ready_state_is_reported_healthy() {
        let finding = rpcs3_finding(rpcs3_assessment(
            Some("/home/user/.config/rpcs3/rpcs3"),
            None,
            healthy_rpcs3_firmware(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Info);
        assert_eq!(finding.id, "emulator_readiness.rpcs3");
        assert_eq!(finding.title, "RPCS3 is ready to launch");
        assert_eq!(
            finding.measurements.get("ready"),
            Some(&Measurement::Flag(true))
        );
        assert_eq!(
            finding.measurements.get("firmware"),
            Some(&Measurement::text("present (not hash-verified)"))
        );
    }

    /// The deliberate current RPCS3 semantics this task must preserve:
    /// `PresentUnverified` firmware is a real, launchable state under the
    /// execution layer's own accepted policy
    /// (`preflight_rpcs3_launch` accepts `ReadyWithWarnings`), so Doctor
    /// must report this as ready - with an honest caveat in the wording,
    /// never as fully blocked.
    #[test]
    fn rpcs3_present_unverified_firmware_is_ready_with_a_caveat_not_blocked() {
        let finding = rpcs3_finding(rpcs3_assessment(
            Some("/home/user/.config/rpcs3/rpcs3"),
            None,
            Rpcs3FirmwareStatus::Present(Some("1.0.0".to_string())),
        ));
        assert_eq!(
            finding.severity,
            DoctorSeverity::Info,
            "PresentUnverified firmware must never read as blocked"
        );
        assert_eq!(finding.title, "RPCS3 is ready to launch");
        assert_eq!(
            finding.measurements.get("ready"),
            Some(&Measurement::Flag(true))
        );
        assert!(finding.explanation.contains("never hash-verifies"));
    }

    #[test]
    fn rpcs3_missing_firmware_is_reported_as_not_ready() {
        let finding = rpcs3_finding(rpcs3_assessment(
            Some("/home/user/.config/rpcs3/rpcs3"),
            None,
            Rpcs3FirmwareStatus::Missing,
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert_eq!(finding.title, "RPCS3 is not ready to launch");
        assert_eq!(
            finding.measurements.get("ready"),
            Some(&Measurement::Flag(false))
        );
        assert_eq!(
            finding.measurements.get("firmware"),
            Some(&Measurement::text("missing"))
        );
        assert!(finding.explanation.contains("Firmware is missing"));
    }

    /// Firmware whose state could not be determined must never be
    /// confidently reported as "missing" - that would invent certainty the
    /// read-only check does not have - but it must also never be reported
    /// as ready, matching `build_rpcs3_command_plan`'s own
    /// `FirmwareReadiness::Unknown` gate.
    #[test]
    fn rpcs3_unknown_firmware_uses_correct_current_policy() {
        let finding = rpcs3_finding(rpcs3_assessment(
            Some("/home/user/.config/rpcs3/rpcs3"),
            None,
            Rpcs3FirmwareStatus::Unknown,
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert_eq!(finding.title, "RPCS3 is not ready to launch");
        assert_eq!(
            finding.measurements.get("ready"),
            Some(&Measurement::Flag(false))
        );
        assert_eq!(
            finding.measurements.get("firmware"),
            Some(&Measurement::text("could not be determined"))
        );
        assert!(!finding.explanation.to_lowercase().contains("missing"));
        assert!(finding.explanation.contains("could not be determined"));
    }

    #[test]
    fn rpcs3_missing_executable_is_reported() {
        let finding = rpcs3_finding(rpcs3_assessment(
            None,
            Some("no native RPCS3 executable was found in the profile"),
            healthy_rpcs3_firmware(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("no native RPCS3 executable was found")
        );
        assert_eq!(
            finding.measurements.get("executable_found"),
            Some(&Measurement::Flag(false))
        );
    }

    #[test]
    fn rpcs3_unsafe_executable_is_reported() {
        let finding = rpcs3_finding(rpcs3_assessment(
            None,
            Some("the RPCS3 executable is a symlink or not a regular file"),
            healthy_rpcs3_firmware(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("symlink or not a regular file")
        );
    }

    #[test]
    fn rpcs3_non_executable_binary_is_reported() {
        let finding = rpcs3_finding(rpcs3_assessment(
            None,
            Some("the RPCS3 executable does not have the execute permission set"),
            healthy_rpcs3_firmware(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("does not have the execute permission")
        );
    }

    #[test]
    fn rpcs3_ambiguous_executable_is_reported() {
        let finding = rpcs3_finding(rpcs3_assessment(
            None,
            Some("more than one native RPCS3 executable was found and none is preferred"),
            healthy_rpcs3_firmware(),
        ));
        assert_eq!(finding.severity, DoctorSeverity::Warning);
        assert!(
            finding
                .explanation
                .contains("more than one native RPCS3 executable")
        );
    }

    /// Proves the real, I/O-touching gatherer: an eligible profile is
    /// genuinely discovered and its real `dev_flash` firmware metadata is
    /// genuinely read, even though no `Native`-classified executable can be
    /// placed here without touching `$PATH`. Also proves "missing/ambiguous
    /// profile" is represented as no assessment at all, never fabricated.
    #[test]
    fn rpcs3_readiness_is_only_assessed_for_eligible_profiles() {
        let tree = TempTree::new("rpcs3-gatherer");
        let root = tree.path().join("config/rpcs3");
        fs::create_dir_all(root.join("dev_flash/vsh/module")).unwrap();
        write_executable(&root.join("dev_flash/vsh/module/vsh.self"), b"self");
        fs::write(root.join("config.yml"), b"---\n").unwrap();
        let discovery = discover_rpcs3_profiles(&Rpcs3ProfileDiscoveryRoots {
            home: tree.path().join("home"),
            xdg_config_home: tree.path().join("config"),
            xdg_data_home: tree.path().join("data-unused"),
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: std::collections::BTreeMap::new(),
            appimage_directory: None,
        });
        assert_eq!(discovery.profiles.len(), 1, "{discovery:?}");
        assert!(discovery.profiles[0].eligible);
        let assessments = assess_rpcs3_readiness(Some(&discovery));
        assert_eq!(assessments.len(), 1);
        assert_eq!(
            assessments[0].firmware,
            FirmwareReadiness::PresentUnverified
        );
        // No `Native` executable is reachable here, so the binding is
        // genuinely absent - the gatherer reports that honestly.
        assert!(assessments[0].executable.is_none());
        assert!(assessments[0].binding_problem.is_some());

        assert!(assess_rpcs3_readiness(None).is_empty());
    }

    #[test]
    fn unrelated_emulator_rows_are_unchanged_by_ppsspp_and_rpcs3_readiness() {
        // Adding PPSSPP/RPCS3 readiness must never alter another adapter's
        // own writability finding shape or wording.
        let tree = TempTree::new("unrelated-unchanged-ppsspp-rpcs3");
        let duckstation_root = tree.path().join("duckstation");
        fs::create_dir_all(&duckstation_root).unwrap();
        fs::write(duckstation_root.join("settings.ini"), b"[Main]\n").unwrap();
        let roots = roots_for_ppsspp_and_duckstation(tree.path(), &[], &[]);
        let discovery = discover_duckstation_profiles(&DuckStationProfileDiscoveryRoots {
            explicit_configuration_roots: vec![duckstation_root.clone()],
            ..roots.duckstation
        });
        let discoveries = ProfileDiscoveries {
            duckstation: Some(&discovery),
            ..ProfileDiscoveries::default()
        };
        let assessed = assess_emulator_profiles(&discoveries, None);
        let findings = findings_from_emulator_profiles(&assessed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "emulator_profile.duckstation_inspected");
        assert_eq!(findings[0].category, DoctorCategory::EmulatorProfiles);
        assert_eq!(findings[0].subsystem, DoctorSubsystem::EmulatorProfiles);
    }
}
