//! Native Linux PS4 launch adapter. Plans are private-field, previewable values;
//! actual spawn requires fresh revalidation. No emulator configuration writes,
//! probes, downloads, package installation, shell, or compatibility claims.
use super::{
    planning::{CanonicalIdentityStatus, LaunchTarget, ResolvedIdentity},
    process_spawn::{self, LaunchCommandSpec, WatchedProcess},
    readiness::LaunchReadiness,
    shadps4_input::ShadPs4GameInput,
    shadps4_profile::{
        ADAPTER_ID, BoundFile, PLATFORM_ID, ShadPs4Profile, ShadPs4Refusal,
        ShadPs4RefusalKind as Kind, profile_is_fresh, real_directory, refuse,
    },
};
use std::{fs, path::PathBuf};

/// A requirement declared by the caller's known game/setup policy. The adapter
/// does not infer that every PS4 game requires every module. Presence is not
/// authenticity or compatibility verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShadPs4Sysmodule {
    AudioDec,
    CesCs,
    Font,
    FontFt,
    FreeTypeOt,
    JpegDec,
    JpegEnc,
    Json,
    Json2,
    LibcInternal,
    Ngs2,
    PngEnc,
    Rtc,
    SystemGesture,
    Ult,
}
impl ShadPs4Sysmodule {
    pub fn filename(self) -> &'static str {
        match self {
            Self::AudioDec => "libSceAudiodec.sprx",
            Self::CesCs => "libSceCesCs.sprx",
            Self::Font => "libSceFont.sprx",
            Self::FontFt => "libSceFontFt.sprx",
            Self::FreeTypeOt => "libSceFreeTypeOt.sprx",
            Self::JpegDec => "libSceJpegDec.sprx",
            Self::JpegEnc => "libSceJpegEnc.sprx",
            Self::Json => "libSceJson.sprx",
            Self::Json2 => "libSceJson2.sprx",
            Self::LibcInternal => "libSceLibcInternal.sprx",
            Self::Ngs2 => "libSceNgs2.sprx",
            Self::PngEnc => "libScePngEnc.sprx",
            Self::Rtc => "libSceRtc.sprx",
            Self::SystemGesture => "libSceSystemGesture.sprx",
            Self::Ult => "libSceUlt.sprx",
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShadPs4LaunchOptions {
    pub fullscreen: Option<bool>,
    pub required_sysmodules: Vec<ShadPs4Sysmodule>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadPs4PreflightState {
    /// Verified launch binding only. Not a claim of firmware authenticity,
    /// complete game integrity, host graphics capability, or game compatibility.
    VerifiedReady,
}
#[derive(Debug, Clone)]
pub struct ShadPs4LaunchPlan {
    profile: ShadPs4Profile,
    game: ShadPs4GameInput,
    options: ShadPs4LaunchOptions,
    target: LaunchTarget,
    identity: ResolvedIdentity,
    command: LaunchCommandSpec,
    modules: Vec<BoundFile>,
    overrides: Vec<(PathBuf, Option<BoundFile>)>,
    warnings: Vec<&'static str>,
}
impl ShadPs4LaunchPlan {
    pub fn target(&self) -> &LaunchTarget {
        &self.target
    }
    pub fn identity(&self) -> &ResolvedIdentity {
        &self.identity
    }
    pub fn game(&self) -> &ShadPs4GameInput {
        &self.game
    }
    pub fn profile(&self) -> &ShadPs4Profile {
        &self.profile
    }
    pub fn command_preview(&self) -> &LaunchCommandSpec {
        &self.command
    }
    pub fn warnings(&self) -> &[&'static str] {
        &self.warnings
    }
    pub fn readiness(&self) -> LaunchReadiness {
        LaunchReadiness::ReadyWithWarnings
    }
}
/// Bind an upstream-resolved identity to the SAME inspected PS4 source. No
/// filename-only policy exception is added. Content/title IDs remain separate.
pub fn plan_shadps4_launch(
    profile: &ShadPs4Profile,
    game: &ShadPs4GameInput,
    identity: &CanonicalIdentityStatus,
    mut options: ShadPs4LaunchOptions,
) -> Result<ShadPs4LaunchPlan, ShadPs4Refusal> {
    let CanonicalIdentityStatus::Resolved(identity) = identity else {
        return Err(refuse(
            Kind::IdentityUnverified,
            "launch needs resolved PS4 identity, not filename evidence",
        ));
    };
    if identity.platform_id != PLATFORM_ID || identity.game_key != game.title_id {
        return Err(refuse(
            Kind::IdentityUnverified,
            "resolved identity does not match this PS4 SFO title ID",
        ));
    }
    if !profile_is_fresh(profile) || !game.fresh() {
        return Err(refuse(
            Kind::ChangedAfterPreview,
            "profile or game changed after inspection",
        ));
    }
    if options.required_sysmodules.len() > 15 {
        return Err(refuse(
            Kind::RequiredSysmoduleMissing,
            "too many module requirements",
        ));
    }
    options.required_sysmodules.sort();
    options.required_sysmodules.dedup();
    let mut modules = Vec::new();
    for module in &options.required_sysmodules {
        let root = &profile.settings.sysmodules_directory;
        if !real_directory(root) {
            return Err(refuse(
                Kind::RequiredSysmoduleMissing,
                format!(
                    "required {} is unavailable in the configured sysmodule directory",
                    module.filename()
                ),
            ));
        }
        let bound = BoundFile::capture(
            &root.join(module.filename()),
            super::shadps4_profile::MAX_BINARY_BYTES,
        )
        .map_err(|e| {
            refuse(
                Kind::RequiredSysmoduleMissing,
                format!("{}: {}", module.filename(), e.detail),
            )
        })?;
        if bound.identity.size == 0 {
            return Err(refuse(
                Kind::RequiredSysmoduleMissing,
                "required module is empty",
            ));
        }
        modules.push(bound);
    }
    let mut overrides = Vec::new();
    let override_root = profile.user_directory.join("custom_configs");
    if !optional_directory_is_safe(&override_root) {
        return Err(refuse(
            Kind::UnsafePath,
            "game override directory is unsafe",
        ));
    }
    for suffix in ["json", "toml"] {
        let path = override_root.join(format!("{}.{}", game.title_id, suffix));
        let bound = match fs::symlink_metadata(&path) {
            Ok(_) => {
                if !real_directory(path.parent().unwrap()) {
                    return Err(refuse(
                        Kind::UnsafePath,
                        "game override directory is unsafe",
                    ));
                }
                Some(BoundFile::capture(&path, 1024 * 1024)?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(refuse(Kind::ConfigurationUnavailable, e.to_string())),
        };
        overrides.push((path, bound));
    }
    let command = command(profile, game, &options)?;
    let target = LaunchTarget::Standalone {
        adapter_id: ADAPTER_ID,
        profile_id: profile.profile_id.clone(),
        profile_path: Some(profile.config_path.clone()),
    };
    let warnings = vec![
        "Structural launch preconditions checked; host Vulkan/CPU support and individual-game compatibility are not verified.",
        "Firmware/sysmodule contents, completeness and game-specific requirements are not verified; supplied requirements only check bound file presence.",
        "EmuWiz inspection is read-only. shadPS4 may write its own user/profile state and apply configured patches, DLC or per-game settings during launch.",
        "Emulator recognition is not publisher authentication; version is unknown and no version probe was executed.",
    ];
    Ok(ShadPs4LaunchPlan {
        profile: profile.clone(),
        game: game.clone(),
        options,
        target,
        identity: identity.clone(),
        command,
        modules,
        overrides,
        warnings,
    })
}
fn optional_directory_is_safe(path: &std::path::Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => real_directory(path),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}
fn command(
    profile: &ShadPs4Profile,
    game: &ShadPs4GameInput,
    options: &ShadPs4LaunchOptions,
) -> Result<LaunchCommandSpec, ShadPs4Refusal> {
    let mut arguments = vec!["-g".into(), game.boot.path.as_os_str().to_owned()];
    if let Some(fullscreen) = options.fullscreen {
        arguments.extend([
            "--fullscreen".into(),
            if fullscreen { "true" } else { "false" }.into(),
        ]);
    }
    let arguments = profile
        .installation
        .wrap_arguments(&super::installation::VisibilityPlan::default(), arguments)
        .map_err(|e| refuse(Kind::ExecutableNotRunnable, e.to_string()))?;
    Ok(LaunchCommandSpec {
        executable: profile.executable.clone(),
        arguments,
        working_directory: Some(profile.working_directory.clone()),
    })
}
/// No prepared command is cached across authorization. Revalidate source,
/// config precedence, executable bytes and optional override appearance, then
/// rebuild the exact argv. Returns a typed verdict without launching.
pub fn preflight_shadps4_launch(
    plan: &ShadPs4LaunchPlan,
) -> Result<ShadPs4PreflightState, ShadPs4Refusal> {
    if !profile_is_fresh(&plan.profile)
        || !plan.game.fresh()
        || plan
            .modules
            .iter()
            .any(|file| !real_directory(file.path.parent().unwrap()) || !file.unchanged())
        || plan.overrides.iter().any(|(path, bound)| match bound {
            Some(file) => !real_directory(path.parent().unwrap()) || !file.unchanged(),
            None => {
                !optional_directory_is_safe(path.parent().unwrap())
                    || !fs::symlink_metadata(path)
                        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            }
        })
    {
        return Err(refuse(
            Kind::ChangedAfterPreview,
            "executable, PS4 source, configuration selection, override or required sysmodule changed since preview",
        ));
    }
    if command(&plan.profile, &plan.game, &plan.options)? != plan.command {
        return Err(refuse(
            Kind::ChangedAfterPreview,
            "rebuilt command differs from preview",
        ));
    }
    Ok(ShadPs4PreflightState::VerifiedReady)
}
/// Existing watched-process supervision: one direct native/AppImage process,
/// inherited environment plus profile-bound XDG_DATA_HOME, PID and bounded
/// stderr/exit result. This infrastructure has no stop/cancel API; none is
/// invented here, and no detached wrapper or shell is added.
pub fn launch_shadps4(plan: &ShadPs4LaunchPlan) -> Result<WatchedProcess, ShadPs4Refusal> {
    preflight_shadps4_launch(plan)?;
    let command = command(&plan.profile, &plan.game, &plan.options)?;
    process_spawn::spawn_watched_process_with_environment(&command, &plan.profile.environment())
        .map_err(|e| refuse(Kind::SpawnFailed, e.to_string()))
}
#[cfg(test)]
mod tests;
