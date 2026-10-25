//! Native FS-UAE launch of one verified, single-disk Amiga ADF.
//!
//! This is a narrow gate in front of the existing FS-UAE preflight. At click
//! time it re-derives the ADF's DAT identity from the catalogue database,
//! requires it to equal the identity the request was planned with, refuses a
//! configuration that names a CD32/CDTV machine or has no present Kickstart
//! file, and only then hands the fresh identity to [`preflight_fsuae_launch`],
//! which re-checks the media, profile, configuration and executable before
//! spawning. Only `.adf` media reaches it; ADZ, DMS, IPF, HDF, multi-disk and
//! CD32/CDTV stay out. Nothing is written, converted or extracted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::amiga_cd_evidence::AmigaCdMachine;
use crate::amiga_game_readiness::AmigaProfileFacts;
use crate::dat::amiga_adf_identity::{AmigaAdfIdentityRefusal, AmigaAdfLaunchIdentity};
use crate::dat::configured_catalogues::ConfiguredCatalogue;
use crate::launch::fsuae_execution::{
    FsUaeLaunchPreflightError, FsUaeLaunchRequest, preflight_fsuae_launch, spawn_fsuae,
};
use crate::launch::input_projection::VerifiedIdentityFact;
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use crate::launch::process_spawn::{PreparedProcessCommand, WatchedProcess};
use crate::patch_manager::{
    AmigaEmulatorKind, AmigaKickstartState, AmigaProfile, AmigaProfileDiscoveryRoots,
    discover_amiga_profiles,
};
use crate::{Database, canonical_platform_for_alias};

/// Everything a click needs; `fsuae` was built from the same identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUaeAdfLaunchRequest {
    pub fsuae: FsUaeLaunchRequest,
    pub roots: AmigaProfileDiscoveryRoots,
    pub database_path: PathBuf,
    pub dat_sources_config: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsUaeAdfLaunchError {
    NotAdf,
    Database(String),
    Identity(AmigaAdfIdentityRefusal),
    /// The ADF now verifies as a different game than the one planned.
    IdentityChanged,
    ProfileNotFound,
    /// The profile has no `fs-uae.conf` emulator configuration to start with.
    ConfigurationMissing,
    WrongMachine(String),
    KickstartNotReady(AmigaKickstartState),
    Preflight(FsUaeLaunchPreflightError),
    Spawn(String),
}

impl FsUaeAdfLaunchError {
    pub fn reason(&self) -> String {
        match self {
            Self::NotAdf => "Only ADF floppy images can be started this way.".into(),
            Self::Database(detail) => format!("The library catalogue could not be read: {detail}"),
            Self::Identity(refusal) => refusal.reason(),
            Self::IdentityChanged => {
                "The ADF no longer verifies as the game that was checked; check it again.".into()
            }
            Self::ProfileNotFound => "The FS-UAE setup that was checked is no longer there.".into(),
            Self::ConfigurationMissing => {
                "FS-UAE has no fs-uae.conf to start with; set FS-UAE up first.".into()
            }
            Self::WrongMachine(model) => format!(
                "FS-UAE is set up as {model}; an Amiga floppy game needs an ordinary Amiga model."
            ),
            Self::KickstartNotReady(state) => format!(
                "FS-UAE has no usable Kickstart ROM ({state:?}). EmuWiz never supplies one."
            ),
            Self::Preflight(error) => format!(
                "FS-UAE launch was refused: {:?}: {}",
                error.kind, error.detail
            ),
            Self::Spawn(detail) => format!("FS-UAE could not be started: {detail}"),
        }
    }
}

/// The launch identity a verified ADF resolves to.
pub fn adf_canonical_identity(
    identity: &AmigaAdfLaunchIdentity,
) -> (CanonicalIdentityStatus, Vec<VerifiedIdentityFact>) {
    (
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Amiga".into(),
            game_key: identity.game_key.clone(),
        }),
        vec![VerifiedIdentityFact::AmigaIdentity(
            identity.game_key.clone(),
        )],
    )
}

/// The configuration an ADF launch uses: the profile's own `fs-uae.conf`.
/// The launcher's `Launcher.ini` is not an emulator configuration, and a
/// per-game file under `Configurations/` belongs to another game.
pub fn fsuae_adf_config(profile: &AmigaProfile) -> Option<&Path> {
    profile
        .global_config_path
        .as_deref()
        .filter(|path| path.file_name().is_some_and(|name| name == "fs-uae.conf"))
}

/// Why this FS-UAE profile cannot run an ordinary Amiga floppy game, if it
/// cannot. A configuration that names no model is FS-UAE's default A500.
pub fn fsuae_adf_profile_refusal(
    profile: &AmigaProfile,
    facts: &AmigaProfileFacts,
) -> Option<FsUaeAdfLaunchError> {
    if profile.emulator != AmigaEmulatorKind::FsUae || !profile.eligible {
        return Some(FsUaeAdfLaunchError::ProfileNotFound);
    }
    if fsuae_adf_config(profile).is_none() {
        return Some(FsUaeAdfLaunchError::ConfigurationMissing);
    }
    if let Some(machine) = facts.machine()
        && machine != AmigaCdMachine::OrdinaryAmiga
    {
        return Some(FsUaeAdfLaunchError::WrongMachine(
            facts.machine_model.clone().unwrap_or_default(),
        ));
    }
    if facts.kickstart != AmigaKickstartState::PresentUnverified {
        return Some(FsUaeAdfLaunchError::KickstartNotReady(facts.kickstart));
    }
    None
}

/// Fresh validation against an open catalogue database. Returns the exact
/// command the existing FS-UAE preflight rebuilt.
pub fn preflight_fsuae_adf_launch(
    request: &FsUaeAdfLaunchRequest,
    database: &Database,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
) -> Result<PreparedProcessCommand, FsUaeAdfLaunchError> {
    let path = &request.fsuae.selected_content_path;
    let is_adf = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("adf"));
    if !is_adf || canonical_platform_for_alias(&request.fsuae.expected_platform_id) != Some("Amiga")
    {
        return Err(FsUaeAdfLaunchError::NotAdf);
    }
    let identity = database
        .amiga_adf_launch_identity(path, catalogues)
        .map_err(|error| FsUaeAdfLaunchError::Database(error.to_string()))?
        .map_err(FsUaeAdfLaunchError::Identity)?;
    if identity.game_key != request.fsuae.expected_game_key {
        return Err(FsUaeAdfLaunchError::IdentityChanged);
    }
    let discovery = discover_amiga_profiles(&request.roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|profile| profile.profile_id == request.fsuae.profile_id)
        .ok_or(FsUaeAdfLaunchError::ProfileNotFound)?;
    let facts = crate::amiga_game_readiness::profile_facts(profile);
    if let Some(refusal) = fsuae_adf_profile_refusal(profile, &facts) {
        return Err(refusal);
    }
    if fsuae_adf_config(profile) != Some(request.fsuae.config_path.as_path()) {
        return Err(FsUaeAdfLaunchError::ConfigurationMissing);
    }
    let (status, facts) = adf_canonical_identity(&identity);
    preflight_fsuae_launch(&request.fsuae, &request.roots, &status, &facts)
        .map_err(FsUaeAdfLaunchError::Preflight)
}

/// Opens the catalogue read-only, revalidates, and spawns FS-UAE.
pub fn preflight_and_launch_fsuae_adf(
    request: &FsUaeAdfLaunchRequest,
) -> Result<WatchedProcess, FsUaeAdfLaunchError> {
    let database = Database::open_read_only(&request.database_path)
        .map_err(|error| FsUaeAdfLaunchError::Database(error.to_string()))?;
    let catalogues =
        crate::dat::configured_catalogues::load_configured_catalogues(&request.dat_sources_config);
    let command = preflight_fsuae_adf_launch(request, &database, &catalogues)?;
    spawn_fsuae(&command).map_err(|error| FsUaeAdfLaunchError::Spawn(error.to_string()))
}

/// The database and DAT-source configuration this process uses.
pub fn default_adf_identity_paths() -> Option<(PathBuf, PathBuf)> {
    let database = crate::default_database_path().ok()?;
    let config = crate::dat::sources::default_dat_sources_config_path().ok()?;
    Some((database, config))
}

/// Reads the current launch identity of the ADF at `path` from this
/// process's catalogue database and configured DATs, or the plain reason it
/// has none.
pub fn current_adf_launch_identity(path: &Path) -> Result<AmigaAdfLaunchIdentity, String> {
    let (database_path, config) = default_adf_identity_paths()
        .ok_or_else(|| "The library catalogue location is unknown.".to_string())?;
    let database = Database::open_read_only(database_path)
        .map_err(|error| format!("The library catalogue could not be read: {error}"))?;
    let catalogues = crate::dat::configured_catalogues::load_configured_catalogues(&config);
    database
        .amiga_adf_launch_identity(path, &catalogues)
        .map_err(|error| format!("The library catalogue could not be read: {error}"))?
        .map_err(|refusal| refusal.reason())
}

/// Builds the click-time request for one discovered FS-UAE profile, binding
/// the exact media, executable and configuration objects seen now.
pub fn build_fsuae_adf_request(
    identity: &AmigaAdfLaunchIdentity,
    content: &Path,
    profile: &AmigaProfile,
    roots: &AmigaProfileDiscoveryRoots,
    database_path: PathBuf,
    dat_sources_config: PathBuf,
) -> Result<FsUaeAdfLaunchRequest, FsUaeAdfLaunchError> {
    let facts = crate::amiga_game_readiness::profile_facts(profile);
    if let Some(refusal) = fsuae_adf_profile_refusal(profile, &facts) {
        return Err(refusal);
    }
    let config = fsuae_adf_config(profile)
        .ok_or(FsUaeAdfLaunchError::ConfigurationMissing)?
        .to_path_buf();
    let binding = crate::launch::resolve_fsuae_native_launch_binding(profile)
        .map_err(|_| FsUaeAdfLaunchError::ProfileNotFound)?;
    let capture = |path: &Path| {
        crate::launch::process_spawn::capture_file_identity(path)
            .map(|(_, identity)| identity)
            .map_err(|error| FsUaeAdfLaunchError::Spawn(error.to_string()))
    };
    Ok(FsUaeAdfLaunchRequest {
        fsuae: FsUaeLaunchRequest {
            selected_content_path: content.to_path_buf(),
            expected_platform_id: "Amiga".into(),
            expected_game_key: identity.game_key.clone(),
            profile_id: profile.profile_id.clone(),
            content_identity: capture(content)?,
            executable_identity: capture(&binding.executable)?,
            expected_executable: binding.executable,
            config_identity: capture(&config)?,
            config_path: config,
            caps_backend_available: false,
        },
        roots: roots.clone(),
        database_path,
        dat_sources_config,
    })
}
