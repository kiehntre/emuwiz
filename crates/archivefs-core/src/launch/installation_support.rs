//! One answer to "can EmuWiz launch this installation?", shared by Doctor
//! and the launcher.
//!
//! The answer is not a separate guess: it is produced by running the same
//! profile discovery and launch-binding resolution that the launch layer
//! itself uses (`resolve_*_native_launch_binding`). An installation that a
//! binding resolves to is launchable; one that is detected but that no
//! binding resolves to is reported as detected-but-not-launchable with the
//! first blocker, never as "Ready" and never as "not installed".

use std::path::PathBuf;

use crate::emulator_lifecycle::ExactBinding;
use crate::launch::installation::{InstallationKind, LaunchInstallation};
use crate::launch::installation_known::{
    DOLPHIN, DUCKSTATION, KnownEmulator, KnownInstallRoots, MELONDS, PCSX2, PPSSPP, RPCS3,
    discover_appimages,
};
use crate::patch_manager::{
    DolphinLocalDiscoveryRoots, DuckStationProfileDiscoveryRoots, MelonDsProfileDiscoveryRoots,
    Pcsx2ProfileDiscoveryRoots, PpssppProfileDiscoveryRoots, Rpcs3ProfileDiscoveryRoots,
    discover_dolphin_local_profiles, discover_duckstation_profiles, discover_melonds_profiles,
    discover_pcsx2_profiles, discover_ppsspp_profiles, discover_rpcs3_profiles,
    resolve_dolphin_native_launch_binding, resolve_duckstation_native_launch_binding,
    resolve_melonds_native_launch_binding, resolve_pcsx2_native_launch_binding,
    resolve_ppsspp_native_launch_binding, resolve_rpcs3_native_launch_binding,
};

/// Whether an installation can be launched by EmuWiz.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchSupport {
    Launchable {
        kind: InstallationKind,
    },
    /// Detected, but no launch binding resolves to it.
    NotLaunchableYet {
        reason: String,
    },
    /// This emulator's launch adapter does not report installation support.
    NotAssessed,
}

impl LaunchSupport {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Launchable { kind } => format!("Ready to launch ({})", kind_label(*kind)),
            Self::NotLaunchableYet { reason } => {
                format!("Installed, but this installation type is not launchable yet: {reason}")
            }
            Self::NotAssessed => "Launch support not assessed for this emulator".into(),
        }
    }
}

#[must_use]
pub fn kind_label(kind: InstallationKind) -> &'static str {
    match kind {
        InstallationKind::Native => "native",
        InstallationKind::AppImage => "AppImage",
        InstallationKind::Flatpak => "Flatpak",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnownAppImage {
    pub emulator_id: &'static str,
    pub path: PathBuf,
    pub portable_marker: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResolvedBinding {
    emulator_id: &'static str,
    executable: PathBuf,
    installation: LaunchInstallation,
}

/// Facts gathered once from the real profile/binding resolution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LaunchAssessment {
    pub appimages: Vec<KnownAppImage>,
    resolved: Vec<ResolvedBinding>,
    blockers: Vec<(&'static str, String)>,
    assessed: Vec<&'static str>,
}

impl LaunchAssessment {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assessed.is_empty()
    }

    /// Support for one detected installation of `emulator_id`.
    #[must_use]
    pub fn support_for(&self, emulator_id: &str, binding: &ExactBinding) -> LaunchSupport {
        if !self
            .assessed
            .iter()
            .any(|id| id.eq_ignore_ascii_case(emulator_id))
        {
            return LaunchSupport::NotAssessed;
        }
        let matches = |resolved: &&ResolvedBinding| {
            resolved.emulator_id.eq_ignore_ascii_case(emulator_id)
                && match binding {
                    ExactBinding::FlatpakApp { app_id } => matches!(
                        &resolved.installation,
                        LaunchInstallation::Flatpak { app_id: id } if id == app_id
                    ),
                    ExactBinding::NativeExecutable { path }
                    | ExactBinding::PortableExecutable { path }
                    | ExactBinding::UnknownExternal { path } => {
                        !matches!(resolved.installation, LaunchInstallation::Flatpak { .. })
                            && &resolved.executable == path
                    }
                    ExactBinding::ManagedInstall {
                        executable_path, ..
                    } => &resolved.executable == executable_path,
                }
        };
        if let Some(found) = self.resolved.iter().find(matches) {
            return LaunchSupport::Launchable {
                kind: found.installation.kind(),
            };
        }
        let first = self
            .blockers
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(emulator_id))
            .map(|(_, detail)| detail.as_str())
            .unwrap_or(
                "no usable settings folder was found for it yet; start the emulator once so it creates one",
            );
        LaunchSupport::NotLaunchableYet {
            reason: first.to_string(),
        }
    }

    fn add_resolved(
        &mut self,
        emulator_id: &'static str,
        executable: PathBuf,
        installation: LaunchInstallation,
    ) {
        self.resolved.push(ResolvedBinding {
            emulator_id,
            executable,
            installation,
        });
    }

    fn add_blocker(&mut self, emulator_id: &'static str, detail: String) {
        if self.blockers.len() < 32 {
            self.blockers.push((emulator_id, detail));
        }
    }
}

/// Every root the assessment needs. Built from the environment in
/// production and from a temp directory in tests.
pub struct AssessmentRoots {
    pub known: KnownInstallRoots,
    pub pcsx2: Pcsx2ProfileDiscoveryRoots,
    pub duckstation: DuckStationProfileDiscoveryRoots,
    pub ppsspp: PpssppProfileDiscoveryRoots,
    pub melonds: MelonDsProfileDiscoveryRoots,
    pub dolphin: DolphinLocalDiscoveryRoots,
    pub rpcs3: Rpcs3ProfileDiscoveryRoots,
}

impl AssessmentRoots {
    #[must_use]
    pub fn from_environment() -> Option<Self> {
        Some(Self {
            known: KnownInstallRoots::from_environment()?,
            pcsx2: Pcsx2ProfileDiscoveryRoots::from_environment().ok()?,
            duckstation: DuckStationProfileDiscoveryRoots::from_environment().ok()?,
            ppsspp: PpssppProfileDiscoveryRoots::from_environment().ok()?,
            melonds: MelonDsProfileDiscoveryRoots::from_environment().ok()?,
            dolphin: DolphinLocalDiscoveryRoots::from_environment().ok()?,
            rpcs3: Rpcs3ProfileDiscoveryRoots::from_environment().ok()?,
        })
    }
}

/// Runs the real discovery and binding resolution for the emulators whose
/// launch adapters understand Native, AppImage and Flatpak installations.
/// Filesystem reads only; nothing is executed or written.
#[must_use]
pub fn assess_from_environment() -> LaunchAssessment {
    AssessmentRoots::from_environment()
        .map(|roots| assess(&roots))
        .unwrap_or_default()
}

#[must_use]
pub fn assess(roots: &AssessmentRoots) -> LaunchAssessment {
    let mut out = LaunchAssessment::default();
    for def in [&PCSX2, &DUCKSTATION, &PPSSPP, &MELONDS, &DOLPHIN, &RPCS3] {
        out.appimages.extend(appimages_of(def, &roots.known));
    }
    if let Ok(discovery) = discover_pcsx2_profiles(&roots.pcsx2) {
        out.assessed.push(PCSX2.id);
        for profile in discovery.profiles.iter().filter(|p| p.eligible) {
            match resolve_pcsx2_native_launch_binding(profile, &roots.pcsx2) {
                Ok(binding) => out.add_resolved(PCSX2.id, binding.executable, binding.installation),
                Err(e) => {
                    out.add_blocker(PCSX2.id, format!("{}: {}", e.detail, profile.provenance))
                }
            }
        }
    }
    out.assessed.push(DUCKSTATION.id);
    for profile in discover_duckstation_profiles(&roots.duckstation)
        .profiles
        .iter()
        .filter(|p| p.eligible)
    {
        match resolve_duckstation_native_launch_binding(profile, &roots.duckstation) {
            Ok(binding) => {
                out.add_resolved(DUCKSTATION.id, binding.executable, binding.installation)
            }
            Err(e) => out.add_blocker(
                DUCKSTATION.id,
                format!("{}: {}", e.detail, profile.provenance),
            ),
        }
    }
    out.assessed.push(PPSSPP.id);
    for profile in discover_ppsspp_profiles(&roots.ppsspp)
        .profiles
        .iter()
        .filter(|p| p.eligible)
    {
        match resolve_ppsspp_native_launch_binding(profile) {
            Ok(binding) => out.add_resolved(PPSSPP.id, binding.executable, binding.installation),
            Err(e) => out.add_blocker(PPSSPP.id, format!("{}: {}", e.detail, profile.provenance)),
        }
    }
    out.assessed.push(MELONDS.id);
    for profile in discover_melonds_profiles(&roots.melonds)
        .profiles
        .iter()
        .filter(|p| p.eligible)
    {
        match resolve_melonds_native_launch_binding(profile) {
            Ok(binding) => out.add_resolved(MELONDS.id, binding.executable, binding.installation),
            Err(e) => out.add_blocker(MELONDS.id, e.detail),
        }
    }
    out.assessed.push(DOLPHIN.id);
    let dolphin = discover_dolphin_local_profiles(&roots.dolphin);
    for profile in dolphin.profiles {
        if !profile.eligible {
            out.add_blocker(
                DOLPHIN.id,
                profile
                    .blocker
                    .unwrap_or_else(|| "profile is not launchable".into()),
            );
            continue;
        }
        match resolve_dolphin_native_launch_binding(&profile, &roots.dolphin) {
            Ok(binding) => out.add_resolved(DOLPHIN.id, binding.executable, binding.installation),
            Err(error) => out.add_blocker(DOLPHIN.id, error.detail),
        }
    }
    out.assessed.push(RPCS3.id);
    let rpcs3 = discover_rpcs3_profiles(&roots.rpcs3);
    for profile in rpcs3.profiles {
        if !profile.eligible {
            out.add_blocker(
                RPCS3.id,
                profile
                    .blockers
                    .first()
                    .map(|blocker| blocker.detail.clone())
                    .unwrap_or_else(|| "profile is not launchable".into()),
            );
            continue;
        }
        match resolve_rpcs3_native_launch_binding(&profile) {
            Ok(binding) => out.add_resolved(RPCS3.id, binding.executable, binding.installation),
            Err(error) => out.add_blocker(RPCS3.id, error.detail),
        }
    }
    out
}

fn appimages_of(def: &'static KnownEmulator, known: &KnownInstallRoots) -> Vec<KnownAppImage> {
    discover_appimages(def, known)
        .into_iter()
        .map(|found| KnownAppImage {
            emulator_id: def.id,
            path: found.path,
            portable_marker: found.portable_marker,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn binding(path: &str) -> ExactBinding {
        ExactBinding::PortableExecutable {
            path: Path::new(path).to_path_buf(),
        }
    }

    #[test]
    fn an_emulator_that_was_not_assessed_is_never_reported_ready() {
        let assessment = LaunchAssessment::default();
        assert_eq!(
            assessment.support_for("PCSX2", &binding("/a/PCSX2.AppImage")),
            LaunchSupport::NotAssessed
        );
        assert!(assessment.is_empty());
    }

    #[test]
    fn a_resolved_binding_makes_exactly_its_installation_launchable() {
        let mut a = LaunchAssessment::default();
        a.assessed.push("PCSX2");
        a.add_resolved(
            "PCSX2",
            "/a/PCSX2.AppImage".into(),
            LaunchInstallation::AppImage {
                extract_and_run: false,
            },
        );
        a.add_resolved(
            "PCSX2",
            "/usr/bin/flatpak".into(),
            LaunchInstallation::flatpak("net.pcsx2.PCSX2").unwrap(),
        );
        assert_eq!(
            a.support_for("PCSX2", &binding("/a/PCSX2.AppImage")),
            LaunchSupport::Launchable {
                kind: InstallationKind::AppImage
            }
        );
        assert_eq!(
            a.support_for(
                "PCSX2",
                &ExactBinding::FlatpakApp {
                    app_id: "net.pcsx2.PCSX2".into()
                }
            ),
            LaunchSupport::Launchable {
                kind: InstallationKind::Flatpak
            }
        );
        // The flatpak program path alone does not make some other app launchable.
        assert!(matches!(
            a.support_for(
                "PCSX2",
                &ExactBinding::FlatpakApp {
                    app_id: "org.other.App".into()
                }
            ),
            LaunchSupport::NotLaunchableYet { .. }
        ));
    }

    #[test]
    fn a_detected_but_unbound_installation_is_not_launchable_yet_with_a_reason() {
        let mut a = LaunchAssessment::default();
        a.assessed.push("DuckStation");
        a.add_blocker("DuckStation", "no portable marker".into());
        match a.support_for("DuckStation", &binding("/a/DuckStation.AppImage")) {
            LaunchSupport::NotLaunchableYet { reason } => {
                assert!(reason.contains("portable marker"))
            }
            other => panic!("{other:?}"),
        }
        assert!(
            a.support_for("DuckStation", &binding("/a/DuckStation.AppImage"))
                .label()
                .starts_with("Installed, but this installation type is not launchable yet")
        );
    }

    mod real_machine;
}
