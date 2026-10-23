//! Launch-planner inputs for one verified single-disk Amiga ADF and FS-UAE.
//!
//! Identity comes only from core's DAT check
//! (`Database::amiga_adf_launch_identity`); a filename, folder or extension
//! never creates it. Each discovered FS-UAE profile becomes a planner profile
//! plus, when it can run the game, a typed request that core preflights again
//! at click time. The plan-time identity is cached briefly because checking
//! it hashes every configured DAT; it is never launch authority.
use archivefs_core::dat::amiga_adf_identity::AmigaAdfLaunchIdentity;
use archivefs_core::launch::{
    CanonicalIdentityStatus, FirmwareReadiness, FsUaeAdfLaunchError, FsUaeAdfLaunchRequest,
    StandaloneProfileInput, adf_canonical_identity, build_fsuae_adf_request,
    current_adf_launch_identity, default_adf_identity_paths, fsuae_adf_profile_refusal,
};
use archivefs_core::patch_manager::{
    AmigaEmulatorKind, AmigaProfileDiscoveryRoots, discover_amiga_profiles,
};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How long a plan-time identity is reused for the same unchanged files.
const IDENTITY_REUSE: Duration = Duration::from_secs(10);

pub(crate) struct AdfLaunchInputs {
    pub(crate) identity: CanonicalIdentityStatus,
    pub(crate) profiles: Vec<StandaloneProfileInput>,
    pub(crate) requests: Vec<(String, String, FsUaeAdfLaunchRequest)>,
}

type Stamp = Vec<Option<(u64, SystemTime)>>;

thread_local! {
    static IDENTITY: RefCell<Option<(PathBuf, Stamp, Instant, Option<AmigaAdfLaunchIdentity>)>> =
        const { RefCell::new(None) };
}

fn stamp(paths: &[&Path]) -> Stamp {
    paths
        .iter()
        .map(|path| {
            std::fs::symlink_metadata(path)
                .ok()
                .and_then(|m| Some((m.len(), m.modified().ok()?)))
        })
        .collect()
}

fn is_amiga_adf(path: &Path, platform: Option<&str>) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("adf"))
        // Core re-checks the stored platform; here only a known other
        // platform skips the catalogue read.
        && platform.is_none_or(|p| archivefs_core::canonical_platform_for_alias(p) == Some("Amiga"))
}

fn cached_identity(path: &Path) -> Option<AmigaAdfLaunchIdentity> {
    let (database, config) = default_adf_identity_paths()?;
    let now = stamp(&[path, &database, &config]);
    IDENTITY.with(|cache| {
        if let Some((cached, at_stamp, at, identity)) = cache.borrow().as_ref()
            && cached == path
            && *at_stamp == now
            && at.elapsed() < IDENTITY_REUSE
        {
            return identity.clone();
        }
        let identity = current_adf_launch_identity(path).ok();
        *cache.borrow_mut() = Some((path.to_path_buf(), now, Instant::now(), identity.clone()));
        identity
    })
}

/// `None` unless `focused` is an Amiga ADF whose DAT identity is verified and
/// proves a single-disk game.
pub(crate) fn launch_inputs(focused: &Path, platform: Option<&str>) -> Option<AdfLaunchInputs> {
    if !is_amiga_adf(focused, platform) {
        return None;
    }
    let identity = cached_identity(focused)?;
    let roots = AmigaProfileDiscoveryRoots::from_environment()?;
    let (database, config) = default_adf_identity_paths()?;
    Some(inputs_for(&identity, focused, &roots, database, config))
}

pub(crate) fn inputs_for(
    identity: &AmigaAdfLaunchIdentity,
    content: &Path,
    roots: &AmigaProfileDiscoveryRoots,
    database: PathBuf,
    config: PathBuf,
) -> AdfLaunchInputs {
    let (status, _) = adf_canonical_identity(identity);
    let mut profiles = Vec::new();
    let mut requests = Vec::new();
    for profile in discover_amiga_profiles(roots).profiles {
        if profile.emulator != AmigaEmulatorKind::FsUae {
            continue;
        }
        let profile_facts = archivefs_core::amiga_game_readiness::profile_facts(&profile);
        let (eligible, firmware) = match fsuae_adf_profile_refusal(&profile, &profile_facts) {
            // A missing Kickstart is a firmware step, not a broken emulator.
            Some(FsUaeAdfLaunchError::KickstartNotReady(_)) => (true, FirmwareReadiness::Missing),
            Some(_) => (false, FirmwareReadiness::NotRequired),
            None => match build_fsuae_adf_request(
                identity,
                content,
                &profile,
                roots,
                database.clone(),
                config.clone(),
            ) {
                Ok(request) => {
                    requests.push(("fsuae".to_string(), profile.profile_id.clone(), request));
                    (true, FirmwareReadiness::NotRequired)
                }
                Err(_) => (false, FirmwareReadiness::NotRequired),
            },
        };
        profiles.push(StandaloneProfileInput {
            adapter_id: "fsuae",
            profile_id: profile.profile_id.clone(),
            profile_path: profile.global_config_path.clone(),
            eligible,
            firmware,
        });
    }
    AdfLaunchInputs {
        identity: status,
        profiles,
        requests,
    }
}

#[cfg(test)]
#[path = "amiga_adf_tests.rs"]
mod tests;
