//! Authority producer for a bounded native Amiberry CD profile.
//! Reads existing files only; delegates argv, refusal and spawn to the existing
//! CD32/CDTV planner/executor. Unsupported config semantics fail closed.
use crate::amiga_cd_evidence::*;
use crate::launch::amiberry_cd_command::{AmiberryCdFileBinding, AmiberryCdLaunchRequest};
use crate::launch::amiberry_cd_execution::{
    LaunchedAmiberryCdProcess, preflight_amiberry_cd_launch, preflight_and_launch_amiberry_cd,
};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::process_spawn::{CapturedFileIdentity, open_regular_file_nonblocking};
use crate::patch_manager::AmigaMachineProfile;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedAmiberryCdProfile {
    pub request: AmiberryCdLaunchRequest,
    pub machine: AmigaMachineProfile,
}
fn binding(path: &Path) -> Result<AmiberryCdFileBinding, String> {
    if !path.is_absolute() || std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err(format!(
            "{} is missing, non-canonical or passes through a symbolic link.",
            path.display()
        ));
    }
    let f = open_regular_file_nonblocking(path)
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if m.len() == 0 {
        return Err(format!("{} is empty.", path.display()));
    }
    Ok(AmiberryCdFileBinding {
        path: path.into(),
        identity: CapturedFileIdentity::capture(&m),
    })
}
fn executable(path: &Path) -> Result<AmiberryCdFileBinding, String> {
    let b = binding(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err("The configured Amiberry binary is not executable.".into());
        }
    }
    Ok(b)
}
fn settings(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let f = open_regular_file_nonblocking(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    f.take(262145)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 262144 {
        return Err("Amiberry profile exceeds the bounded review size.".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "Amiberry profile must be UTF-8 text.")?;
    let mut result = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or("Malformed or sectioned Amiberry profile; no launch was authorized.")?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        // A deliberately narrow native UAE subset. Includes, quickstart,
        // savestates, Lua, extra drives and host filesystem mounts cannot
        // introduce dependencies or override the selected machine unnoticed.
        if !matches!(
            key.as_str(),
            "chipset_compatible"
                | "cpu_model"
                | "chipset"
                | "chipmem_size"
                | "fastmem_size"
                | "z3mem_size"
                | "cdimage0"
                | "kickstart_rom_file"
                | "kickstart_ext_rom_file"
                | "use_gui"
                | "config_description"
                | "config_version"
                | "sound_output"
                | "sound_channels"
                | "sound_frequency"
                | "gfx_width"
                | "gfx_height"
                | "gfx_width_windowed"
                | "gfx_height_windowed"
                | "gfx_fullscreen_amiga"
                | "joyport0"
                | "joyport1"
        ) {
            return Err(format!(
                "Profile setting '{key}' is outside Blue's reviewed CD subset. Use an existing supported CD profile; EmuWiz will not rewrite it."
            ));
        }
        if result.insert(key.clone(), value.into()).is_some() {
            return Err(format!("Duplicate profile setting '{key}' is ambiguous."));
        }
    }
    Ok(result)
}
fn exact_path(settings: &BTreeMap<String, String>, key: &str) -> Result<PathBuf, String> {
    let raw = settings.get(key).ok_or_else(|| {
        format!("Configure {key} explicitly in your Amiberry profile, then check again.")
    })?;
    // Absolute literal paths avoid native ROM/CD multipath, quoting, comma
    // drive suffix, variable and profile-relative lookup ambiguity.
    if raw.contains(['"', '\\', ',', '$', '%', '\0']) || !Path::new(raw).is_absolute() {
        return Err(format!(
            "{key} needs an absolute literal path without expansion or drive suffix in this supported slice."
        ));
    }
    Ok(PathBuf::from(raw))
}

/// Review the actual native UAE options. `amiga_model` belongs to other
/// configuration dialects and is not accepted as native machine authority.
/// Native option evidence: https://github.com/BlitterStudio/amiberry/blob/master/src/cfgfile.cpp
pub fn review_amiberry_cd_profile(
    executable_path: &Path,
    profile: &Path,
    selected: &Path,
    identity: &CanonicalIdentityStatus,
) -> Result<ReviewedAmiberryCdProfile, String> {
    let (platform,machine,model)=match identity {
        CanonicalIdentityStatus::Resolved(i) if i.platform_id==AMIGA_CD32_PLATFORM_ID => (i.platform_id.clone(),AmigaCdMachine::Cd32,"CD32"),
        CanonicalIdentityStatus::Resolved(i) if i.platform_id==AMIGA_CDTV_PLATFORM_ID => (i.platform_id.clone(),AmigaCdMachine::Cdtv,"CDTV"),
        _=>return Err("A verified CD32 or CDTV identity is required. Generic Amiga and filename extensions cannot authorize CD launch.".into()),
    };
    let exe = executable(executable_path)?;
    let config = binding(profile)?;
    let selected_binding = binding(selected)?;
    let options = settings(profile)?;
    if options.get("chipset_compatible").map(String::as_str) != Some(model) {
        return Err(format!(
            "The native profile must explicitly bind chipset_compatible={model}; CD32 and CDTV are distinct machines."
        ));
    }
    if exact_path(&options, "cdimage0")? != selected {
        return Err("The profile mounts a different CD image. Select that disc or choose a matching profile; EmuWiz does not change media slots.".into());
    }
    let main = binding(&exact_path(&options, "kickstart_rom_file")?)?;
    let extended = binding(&exact_path(&options, "kickstart_ext_rom_file")?)?;
    let format = match selected
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("iso") => AmigaCdMediaFormat::Iso,
        Some("cue") => AmigaCdMediaFormat::CueBin,
        _ => return Err(
            "Only configured ISO or CUE/BIN media is supported. CHD and other formats are refused."
                .into(),
        ),
    };
    let mut dependencies = Vec::new();
    if format == AmigaCdMediaFormat::CueBin {
        // Shared strict generic CUE resolver enumerates every FILE and refuses
        // escapes/missing members; no platform inference from the descriptor.
        for path in crate::ingestion::cue_bin::resolve_cue_all_files(selected)
            .map_err(|e| format!("CUE dependencies: {e}"))?
        {
            dependencies.push(binding(&path)?);
        }
    }
    let readiness = assess_amiga_cd_readiness(
        identity,
        &AmigaCdIdentityEvidence::default(),
        machine,
        AmigaCdFirmwareEvidence {
            main_kickstart: AmigaCdFirmwareState::PresentUnverified,
            extended_rom: AmigaCdFirmwareState::PresentUnverified,
        },
        AmigaCdMediaEvidence {
            format,
            complete: true,
            identified_platform: Some(machine),
        },
    );
    let machine_profile = AmigaMachineProfile {
        machine_model: Some(model.into()),
        ..Default::default()
    };
    let request = AmiberryCdLaunchRequest {
        executable: exe.path,
        profile: config.path,
        canonical_platform: platform,
        machine,
        machine_model: model.into(),
        selected_content: selected_binding,
        media_format: format,
        media_dependencies: dependencies,
        firmware_main: main,
        firmware_extended: extended,
        readiness,
        identity: identity.clone(),
        profile_identity: config.identity,
        executable_identity: exe.identity,
        profile_media_configured: true,
    };
    // Refuse a change during review as well as a change between review/spawn.
    preflight_amiberry_cd_launch(&request, &request.readiness, &machine_profile, true)
        .map_err(|e| e.detail)?;
    Ok(ReviewedAmiberryCdProfile {
        request,
        machine: machine_profile,
    })
}
impl ReviewedAmiberryCdProfile {
    /// The caller supplies freshly resolved identity, never a cached plan.
    pub fn launch(
        &self,
        current_identity: &CanonicalIdentityStatus,
    ) -> Result<LaunchedAmiberryCdProcess, String> {
        if current_identity != &self.request.identity {
            return Err("CD identity changed after review. Check again.".into());
        }
        let current = review_amiberry_cd_profile(
            &self.request.executable,
            &self.request.profile,
            &self.request.selected_content.path,
            current_identity,
        )?;
        if current != *self {
            return Err("Amiberry profile, media, dependencies, executable or firmware changed after review. Check again.".into());
        }
        preflight_and_launch_amiberry_cd(
            &self.request,
            &current.request.readiness,
            &current.machine,
            current.request.profile_media_configured,
        )
        .map_err(|e| format!("Amiberry was not started: {e:?}"))
    }
}
#[cfg(test)]
#[path = "amiberry_cd_profile_tests.rs"]
mod tests;
