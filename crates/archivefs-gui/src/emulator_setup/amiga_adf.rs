//! Launch-planner inputs for one single-disk Amiga ADF and FS-UAE.
//!
//! Game identity comes only from core's DAT check
//! (`Database::amiga_adf_launch_identity`); a filename, folder or extension
//! never creates it. Without a DAT match (and without catalogue evidence
//! against), core's local-media route can still allow one plausible,
//! non-multi-disk floppy image; that proves media, not which game it is. Each discovered FS-UAE profile becomes a planner profile
//! plus, when it can run the game, a typed request that core preflights again
//! at click time. The plan-time identity is cached briefly because checking
//! it hashes every configured DAT; it is never launch authority.
use archivefs_core::dat::amiga_adf_identity::{AmigaAdfIdentityRefusal, AmigaAdfLaunchIdentity};
use archivefs_core::launch::{
    CanonicalIdentityStatus, FirmwareReadiness, FsUaeAdfLaunchError, FsUaeAdfLaunchRequest,
    LocalAdfMedia, StandaloneProfileInput, adf_canonical_identity, build_fsuae_adf_request,
    build_fsuae_local_adf_request, current_adf_launch_decision, default_adf_identity_paths,
    fsuae_adf_profile_refusal, local_adf_media,
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
/// The ADF, its file stamps, when it was checked, and what was found.
type CachedIdentity = (PathBuf, Stamp, Instant, Option<Decision>);
/// `None`: the catalogue could not be read, so nothing is known against it.
type Decision = Result<AmigaAdfLaunchIdentity, AmigaAdfIdentityRefusal>;
type CachedMedia = (PathBuf, Stamp, Instant, Option<LocalAdfMedia>);

thread_local! {
    static IDENTITY: RefCell<Option<CachedIdentity>> = const { RefCell::new(None) };
    static MEDIA: RefCell<Option<CachedMedia>> = const { RefCell::new(None) };
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

fn cached_decision(path: &Path) -> Option<Decision> {
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
        let identity = current_adf_launch_decision(path).ok();
        *cache.borrow_mut() = Some((path.to_path_buf(), now, Instant::now(), identity.clone()));
        identity
    })
}

fn cached_media(path: &Path) -> Option<LocalAdfMedia> {
    let now = stamp(&[path]);
    MEDIA.with(|cache| {
        if let Some((cached, at_stamp, at, media)) = cache.borrow().as_ref()
            && cached == path
            && *at_stamp == now
            && at.elapsed() < IDENTITY_REUSE
        {
            return media.clone();
        }
        let media = local_adf_media(path).ok();
        *cache.borrow_mut() = Some((path.to_path_buf(), now, Instant::now(), media.clone()));
        media
    })
}

/// `None` unless `focused` is an Amiga ADF that either has a verified
/// single-disk DAT identity, or - with no catalogue evidence against it - is
/// one plausible, non-multi-disk floppy image (local media route; no game
/// identity is claimed). The catalogue is always asked first.
pub(crate) fn launch_inputs(focused: &Path, platform: Option<&str>) -> Option<AdfLaunchInputs> {
    if !is_amiga_adf(focused, platform) {
        return None;
    }
    let roots = AmigaProfileDiscoveryRoots::from_environment()?;
    let (database, config) = default_adf_identity_paths()?;
    match cached_decision(focused)? {
        Ok(identity) => Some(inputs_for(&identity, focused, &roots, database, config)),
        Err(refusal) if !refusal.permits_local_media() => None,
        Err(_) => {
            // The local route needs a library platform that says Amiga.
            let platform = platform?;
            let media = cached_media(focused)?;
            Some(local_inputs_for(&media, platform, &roots, database, config))
        }
    }
}

/// The local-media route: the same profile checks as the verified route, a
/// request marked as local media, and a planner identity that is only the
/// file's own bytes - never a catalogue game.
pub(crate) fn local_inputs_for(
    media: &LocalAdfMedia,
    platform: &str,
    roots: &AmigaProfileDiscoveryRoots,
    database: PathBuf,
    config: PathBuf,
) -> AdfLaunchInputs {
    let key = format!(
        "{}{}",
        archivefs_core::launch::LOCAL_ADF_KEY_PREFIX,
        media.sha256
    );
    collect(
        CanonicalIdentityStatus::Resolved(archivefs_core::launch::ResolvedIdentity {
            platform_id: "Amiga".into(),
            game_key: key,
        }),
        roots,
        |profile| {
            build_fsuae_local_adf_request(
                media,
                platform,
                profile,
                roots,
                database.clone(),
                config.clone(),
            )
        },
    )
}

pub(crate) fn inputs_for(
    identity: &AmigaAdfLaunchIdentity,
    content: &Path,
    roots: &AmigaProfileDiscoveryRoots,
    database: PathBuf,
    config: PathBuf,
) -> AdfLaunchInputs {
    let (status, _) = adf_canonical_identity(identity);
    collect(status, roots, |profile| {
        build_fsuae_adf_request(
            identity,
            content,
            profile,
            roots,
            database.clone(),
            config.clone(),
        )
    })
}

fn collect(
    status: CanonicalIdentityStatus,
    roots: &AmigaProfileDiscoveryRoots,
    build: impl Fn(
        &archivefs_core::patch_manager::AmigaProfile,
    ) -> Result<FsUaeAdfLaunchRequest, FsUaeAdfLaunchError>,
) -> AdfLaunchInputs {
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
            None => match build(&profile) {
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
