use crate::diagnostics::{DoctorCategory, DoctorSeverity, DoctorSubsystem, Finding};
use crate::emulator_environment::EncodedPath;
use crate::launch::{SNES9X_SUPPORTED_PLATFORM_IDS, STELLA_SUPPORTED_PLATFORM_ID};
use crate::patch_manager::{
    HatariIdentityState, HatariProfileDiscoveryRoots, HatariSelectedGameRequest,
    OpenMsxProfileDiscoveryRoots, Snes9xProfileDiscoveryRoots, StellaProfileDiscoveryRoots,
    ViceProfileDiscoveryRoots, Vita3kProfileDiscoveryRoots, assess_hatari_readiness,
    assess_openmsx_readiness, assess_vice_readiness, assess_vita3k_readiness,
    discover_hatari_profiles, discover_openmsx_profiles, discover_snes9x_profiles,
    discover_stella_profiles, discover_vice_profiles, discover_vita3k_profiles,
    inspect_hatari_game, resolve_snes9x_native_launch_binding,
    resolve_stella_native_launch_binding,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemainingEmulatorReadiness {
    pub adapter: String,
    pub executable: Option<EncodedPath>,
    pub version: Option<String>,
    pub profile: Option<EncodedPath>,
    pub supported_systems: Vec<String>,
    pub ready: bool,
    pub blockers: Vec<String>,
    pub evidence: Vec<String>,
    pub remediation: String,
}

struct RemainingEmulatorDetails {
    blockers: Vec<String>,
    evidence: Vec<String>,
    remediation: String,
}

fn details(
    blockers: Vec<String>,
    evidence: Vec<String>,
    remediation: impl Into<String>,
) -> RemainingEmulatorDetails {
    RemainingEmulatorDetails {
        blockers,
        evidence,
        remediation: remediation.into(),
    }
}

fn item(
    adapter: &str,
    executable: Option<EncodedPath>,
    version: Option<String>,
    profile: Option<EncodedPath>,
    systems: &[&str],
    ready: bool,
    details: RemainingEmulatorDetails,
) -> RemainingEmulatorReadiness {
    RemainingEmulatorReadiness {
        adapter: adapter.into(),
        executable,
        version,
        profile,
        supported_systems: systems.iter().map(|v| (*v).into()).collect(),
        ready,
        blockers: details.blockers,
        evidence: details.evidence,
        remediation: details.remediation,
    }
}

fn missing(adapter: &str, systems: &[&str]) -> RemainingEmulatorReadiness {
    item(
        adapter,
        None,
        None,
        None,
        systems,
        false,
        details(
            vec![format!("{adapter} executable/profile was not found")],
            Vec::new(),
            format!("Install {adapter} or select its executable in Emulator Setup."),
        ),
    )
}

pub fn discover_remaining_emulator_readiness() -> Vec<RemainingEmulatorReadiness> {
    let mut out = Vec::new();
    match HatariProfileDiscoveryRoots::from_environment() {
        Ok(roots) => {
            let d = discover_hatari_profiles(&roots);
            if d.profiles.is_empty() {
                out.push(missing(
                    "Hatari",
                    &["Atari ST", "Atari STE", "Atari TT", "Falcon"],
                ));
            }
            for p in d.profiles {
                let inspection = inspect_hatari_game(
                    &p,
                    &HatariSelectedGameRequest {
                        canonical_platform: None,
                        identity_state: HatariIdentityState::Unresolved,
                        verified_title: None,
                    },
                    &[],
                );
                let e = assess_hatari_readiness(&p, &inspection);
                let x = e.executable.as_ref();
                out.push(item("Hatari", x.map(|v| EncodedPath::from_path(&v.path)), x.and_then(|v| v.version.clone()), Some(EncodedPath::from_path(&e.config_path)), &["Atari ST", "Atari STE", "Atari TT", "Falcon"], e.ready, details(e.first_blocker.into_iter().collect(), vec![format!("Configuration: {}", if e.config_present { "present" } else { "missing" }), format!("Configuration readable: {}", e.config_readable), format!("Machine model: {:?}", e.machine.model), format!("TOS: {:?}", e.tos.health), format!("Media representations: {:?}", e.media_representations)], "Fix Hatari's first reported profile, machine, configuration, or TOS blocker, then run Doctor again.")));
            }
        }
        Err(_) => out.push(missing(
            "Hatari",
            &["Atari ST", "Atari STE", "Atari TT", "Falcon"],
        )),
    }
    match Vita3kProfileDiscoveryRoots::from_environment() {
        Ok(roots) => {
            let d = discover_vita3k_profiles(&roots);
            if d.profiles.is_empty() {
                out.push(missing("Vita3K", &["PlayStation Vita"]));
            }
            for p in d.profiles {
                let e = assess_vita3k_readiness(&p, None, None);
                let x = e.profile.executable.as_ref();
                out.push(item("Vita3K", x.map(|v| EncodedPath::from_path(&v.path)), e.profile.version.clone(), Some(EncodedPath::from_path(&p.configuration_path)), &["PlayStation Vita"], e.ready, details(e.first_blocker.into_iter().collect(), vec![format!("Firmware: {:?}", e.profile.firmware), "Installed title/license/content: not selected for profile inspection".into(), format!("Vita filesystem: {}", e.profile.vita_fs_path.display())], "Fix Vita3K's first reported profile or firmware blocker, then run Doctor again.")));
            }
        }
        Err(_) => out.push(missing("Vita3K", &["PlayStation Vita"])),
    }
    let d = discover_vice_profiles(&ViceProfileDiscoveryRoots::from_environment());
    if d.profiles.is_empty() {
        out.push(missing("VICE", &["Commodore 64"]));
    }
    for p in d.profiles {
        let e = assess_vice_readiness(&p);
        let x = e.executable.as_ref();
        out.push(item(
            "VICE",
            x.map(|v| EncodedPath::from_path(&v.path)),
            x.and_then(|v| v.version.clone()),
            None,
            &["Commodore 64"],
            e.ready,
            details(
                e.first_blocker.into_iter().collect(),
                vec![
                    "C64 support: present".into(),
                    "BIOS/firmware: no external requirement is modeled by VICE".into(),
                ],
                "Fix VICE's first reported executable/profile blocker, then run Doctor again.",
            ),
        ));
    }
    let d = discover_openmsx_profiles(&OpenMsxProfileDiscoveryRoots::from_environment());
    if d.profiles.is_empty() {
        out.push(missing("openMSX", &["MSX", "MSX2"]));
    }
    for p in d.profiles {
        let e = assess_openmsx_readiness(&p);
        let x = e.executable.as_ref();
        out.push(item(
            "openMSX",
            x.map(|v| EncodedPath::from_path(&v.path)),
            e.version.clone(),
            None,
            &["MSX", "MSX2"],
            e.ready,
            details(
                e.first_blocker.into_iter().collect(),
                vec![
                    "Version: unavailable by design".into(),
                    "Configuration inspection: unavailable by design".into(),
                    format!("C-BIOS bindings: {:?}", e.machine_bindings),
                ],
                "Fix openMSX's first reported executable/profile blocker, then run Doctor again.",
            ),
        ));
    }
    let d = discover_snes9x_profiles(&Snes9xProfileDiscoveryRoots::from_environment());
    if d.profiles.is_empty() {
        out.push(missing("Snes9x", SNES9X_SUPPORTED_PLATFORM_IDS));
    }
    for p in d.profiles {
        let b = resolve_snes9x_native_launch_binding(&p)
            .err()
            .map(|e| e.detail);
        out.push(item(
            "Snes9x",
            b.is_none().then(|| EncodedPath::from_path(&p.executable)),
            p.version.clone(),
            None,
            SNES9X_SUPPORTED_PLATFORM_IDS,
            b.is_none(),
            details(
                b.into_iter().collect(),
                vec![
                    "Profile semantics: executable-only".into(),
                    "Configuration/firmware: no requirement is modeled".into(),
                ],
                "Fix Snes9x's first reported executable/profile blocker, then run Doctor again.",
            ),
        ));
    }
    let d = discover_stella_profiles(&StellaProfileDiscoveryRoots::from_environment());
    if d.profiles.is_empty() {
        out.push(missing("Stella", &[STELLA_SUPPORTED_PLATFORM_ID]));
    }
    for p in d.profiles {
        let b = resolve_stella_native_launch_binding(&p)
            .err()
            .map(|e| e.detail);
        let x = p.executable_candidates.first();
        out.push(item(
            "Stella",
            b.is_none()
                .then(|| x.map(|v| EncodedPath::from_path(&v.path)))
                .flatten(),
            x.and_then(|v| v.version.clone()),
            None,
            &[STELLA_SUPPORTED_PLATFORM_ID],
            b.is_none(),
            details(
                b.into_iter().collect(),
                vec![
                    "Profile semantics: executable-only".into(),
                    "Configuration/firmware: no requirement is modeled".into(),
                ],
                "Fix Stella's first reported executable/profile blocker, then run Doctor again.",
            ),
        ));
    }
    out
}

pub fn findings_from_remaining_profiles() -> Vec<Finding> {
    discover_remaining_emulator_readiness().iter().map(|e| {
        let mut f = Finding::new(format!("emulator_profile.{}_readiness", e.adapter.to_ascii_lowercase().replace([' ', '-'], "_")), DoctorCategory::EmulatorProfiles, DoctorSubsystem::EmulatorReadiness, if e.ready { DoctorSeverity::Info } else { DoctorSeverity::Warning }, if e.ready { format!("{} ready", e.adapter) } else { format!("{} needs setup", e.adapter) }, e.blockers.first().cloned().unwrap_or_else(|| format!("{} has a usable discovered profile; selected content remains subject to launch preflight.", e.adapter))).with_evidence(e.evidence.clone()).with_evidence([format!("Supported systems: {}", e.supported_systems.join(", "))]).with_guidance("EmuWiz only reports inspected evidence and does not change emulator files.", e.remediation.clone());
        if let Some(v) = &e.executable { f = f.with_evidence([format!("Executable: {}", v.display)]); }
        if let Some(v) = &e.version { f = f.with_evidence([format!("Version: {v}")]); }
        if let Some(v) = &e.profile { f = f.with_evidence([format!("Profile/config: {}", v.display)]); }
        f
    }).collect()
}
