//! Native FS-UAE launch of one local Amiga ADF without a DAT match.
//!
//! This is a media route, not an identity route. It proves only that the
//! selected file is one structurally valid Amiga floppy image whose exact
//! bytes (SHA-256) were seen, that nothing known says it belongs to a
//! multi-disk set, and that the FS-UAE profile can run an ordinary Amiga
//! floppy. It never says which game the disk is: no catalogue identity is
//! read, written or rewritten, and the verified-game route in `fsuae_adf` is
//! untouched.
//!
//! The command planner needs a non-empty identity token. For this route that
//! token is `local-adf-sha256:<hex>`: derived from the bytes, namespaced so
//! it can never equal a catalogue game key, and used only inside the
//! preflight. It is not shown as, or stored as, a game identity.
//!
//! Multi-disk safety: ordering is never inferred from file names to
//! *allow* a launch. The existing media-set engine runs over the ADF and the
//! ADF files beside it; any ordinal, expected count above one, or second member
//! refuses. (The engine reports every lone disk as an incomplete set with
//! "unknown count" conflicts; those say nothing against a single disk.)
//! The local route proceeds only when the catalogue has no evidence either
//! way (`AmigaAdfIdentityRefusal::permits_local_media`); any other state blocks.
//! The multi-disk check is best-effort absence-of-evidence, not proof that the
//! game has one disk. (`is_evidence_against_standalone`).

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dat::configured_catalogues::ConfiguredCatalogue;
use crate::launch::fsuae_adf::{
    FsUaeAdfLaunchError, FsUaeAdfLaunchRequest, FsUaeAdfRoute, fsuae_adf_config,
    fsuae_adf_profile_refusal,
};
use crate::launch::fsuae_execution::{FsUaeLaunchRequest, preflight_fsuae_launch};
use crate::launch::input_projection::VerifiedIdentityFact;
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use crate::launch::process_spawn::PreparedProcessCommand;
use crate::media_set::{index_media, media_record, resolve_index};
use crate::patch_manager::{AmigaProfile, AmigaProfileDiscoveryRoots, discover_amiga_profiles};
use crate::{Database, canonical_platform_for_alias};

/// Standard Amiga DD and HD floppy image sizes.
const ADF_SIZES: [u64; 2] = [901_120, 1_802_240];
/// Most sibling files looked at when checking for a multi-disk set.
const MAX_SIBLINGS: usize = 4096;
/// Media-like extensions whose presence beside the ADF is refuse-only evidence.
const SIBLING_EXTENSIONS: [&str; 5] = ["adf", "adz", "dms", "ipf", "zip"];

/// Prefix of the internal, bytes-derived planner token. Never a game identity;
/// anything that persists or displays a game key must reject it.
pub const LOCAL_ADF_KEY_PREFIX: &str = "local-adf-sha256:";

/// One local ADF: its exact bytes' SHA-256 and the file facts seen now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAdfMedia {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

fn refuse(reason: impl Into<String>) -> FsUaeAdfLaunchError {
    FsUaeAdfLaunchError::LocalMedia(reason.into())
}

fn local_key(sha256: &str) -> String {
    format!("{LOCAL_ADF_KEY_PREFIX}{sha256}")
}

const MULTI_DISK_REASON: &str = "This ADF may be one disk of a multi-disk set (disk numbers, or other disk images beside it that EmuWiz cannot rule out). Multi-disk games need an explicit disk set and are not started from one disk.";

/// Best-effort absence-of-evidence check, used only to refuse. Returns a
/// reason if the existing media-set engine, run over this ADF and every
/// Amiga-media-like file beside it (symlinks included), puts it in a set with
/// an ordinal, an expected count above one or another member. It cannot prove
/// a game has one disk. An unreadable, oversized or unplaceable case refuses.
fn multi_disk_refusal(path: &Path) -> Option<String> {
    let mut paths = vec![path.to_path_buf()];
    if let Some(parent) = path.parent() {
        let Ok(entries) = fs::read_dir(parent) else {
            return Some(MULTI_DISK_REASON.into());
        };
        let mut seen = 0usize;
        for entry in entries {
            let Ok(entry) = entry else {
                return Some(MULTI_DISK_REASON.into());
            };
            seen += 1;
            if seen > MAX_SIBLINGS {
                return Some(MULTI_DISK_REASON.into());
            }
            let sibling = entry.path();
            let like_media = sibling
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| SIBLING_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)));
            if like_media && sibling != path {
                paths.push(sibling);
            }
        }
    }
    let records = paths
        .iter()
        .map(|p| media_record(p, Some("Amiga")))
        .collect();
    let report = resolve_index(index_media(records));
    let mut placed = false;
    for set in &report.sets {
        let ours = set.members.iter().any(|member| {
            member
                .representations
                .iter()
                .any(|r| r.record.source.path == path)
        });
        if !ours {
            continue;
        }
        placed = true;
        let ordinal = set.members.iter().any(|m| m.ordinal.is_some());
        let counted = set
            .expected_count
            .as_ref()
            .is_some_and(|(c, _)| c.count > 1);
        if set.members.len() > 1 || ordinal || counted {
            return Some(MULTI_DISK_REASON.into());
        }
    }
    // The engine must account for the input; otherwise nothing was checked.
    (!placed).then(|| MULTI_DISK_REASON.into())
}

/// Establishes the media facts or the plain reason it cannot be a launchable
/// floppy image. The file is opened once (no-follow, non-blocking), checked
/// by descriptor, inspected and hashed from that same descriptor, and the
/// descriptor's identity must be unchanged afterwards. Never writes.
pub fn local_adf_media(path: &Path) -> Result<LocalAdfMedia, FsUaeAdfLaunchError> {
    let is_adf = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("adf"));
    if !is_adf {
        return Err(FsUaeAdfLaunchError::NotAdf);
    }
    if !path.is_absolute() {
        return Err(refuse("The ADF path must be absolute."));
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| {
            refuse(format!(
                "The ADF could not be opened as a regular file: {e}"
            ))
        })?;
    let before = file
        .metadata()
        .map_err(|e| refuse(format!("The ADF is unreadable: {e}")))?;
    if !before.is_file() {
        return Err(refuse("The ADF is not a regular file."));
    }
    let size = before.len();
    if !ADF_SIZES.contains(&size) {
        return Err(refuse(format!(
            "The file is {size} bytes, which is not a standard Amiga floppy image size."
        )));
    }
    // Structure (boot block) from the same descriptor.
    let probe = file
        .try_clone()
        .map_err(|e| refuse(format!("The ADF is unreadable: {e}")))?;
    crate::amiga_disk::inspect_amiga_floppy_from_opened(path, probe).map_err(|e| {
        refuse(format!(
            "The file is not a recognisable Amiga floppy image: {e:?}"
        ))
    })?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| refuse(format!("The ADF is unreadable: {e}")))?;
    let mut bytes = Vec::with_capacity(size as usize);
    (&mut file)
        .take(size + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| refuse(format!("The ADF is unreadable: {e}")))?;
    let after = file
        .metadata()
        .map_err(|e| refuse(format!("The ADF is unreadable: {e}")))?;
    if bytes.len() as u64 != size
        || after.len() != size
        || after.ino() != before.ino()
        || after.dev() != before.dev()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(refuse("The ADF changed while it was being read."));
    }
    let sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if let Some(reason) = multi_disk_refusal(path) {
        return Err(refuse(reason));
    }
    Ok(LocalAdfMedia {
        path: path.to_path_buf(),
        sha256,
        size,
    })
}

/// Builds the click-time request for one discovered FS-UAE profile, binding
/// the exact media, executable and configuration objects seen now. Profile
/// refusals are the same ones the verified-game route applies.
pub fn build_fsuae_local_adf_request(
    media: &LocalAdfMedia,
    library_platform: &str,
    profile: &AmigaProfile,
    roots: &AmigaProfileDiscoveryRoots,
    database_path: PathBuf,
    dat_sources_config: PathBuf,
) -> Result<FsUaeAdfLaunchRequest, FsUaeAdfLaunchError> {
    // The platform the library records for this file, supplied by the
    // caller, must be Amiga; nothing else makes a loose ADF Amiga here.
    if canonical_platform_for_alias(library_platform) != Some("Amiga") {
        return Err(refuse("The library does not record this file as Amiga."));
    }
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
            selected_content_path: media.path.clone(),
            expected_platform_id: "Amiga".into(),
            expected_game_key: local_key(&media.sha256),
            profile_id: profile.profile_id.clone(),
            content_identity: capture(&media.path)?,
            executable_identity: capture(&binding.executable)?,
            expected_executable: binding.executable,
            config_identity: capture(&config)?,
            config_path: config,
            caps_backend_available: false,
        },
        roots: roots.clone(),
        database_path,
        dat_sources_config,
        route: FsUaeAdfRoute::LocalMedia {
            sha256: media.sha256.clone(),
        },
    })
}

/// Click-time revalidation for the local-media route. Re-derives the media
/// facts from the file, refuses if the catalogue now says the disk is part
/// of a set, then hands off to the shared FS-UAE preflight.
pub(crate) fn preflight_local_adf(
    request: &FsUaeAdfLaunchRequest,
    sha256: &str,
    database: &Database,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
) -> Result<PreparedProcessCommand, FsUaeAdfLaunchError> {
    let path = &request.fsuae.selected_content_path;
    if canonical_platform_for_alias(&request.fsuae.expected_platform_id) != Some("Amiga") {
        return Err(FsUaeAdfLaunchError::NotAdf);
    }
    let media = local_adf_media(path)?;
    if media.sha256 != sha256 || request.fsuae.expected_game_key != local_key(sha256) {
        return Err(refuse(
            "The ADF's contents changed since it was checked; check it again.",
        ));
    }
    // No loaded catalogue means no evidence either way: refuse, never assume.
    if catalogues.is_empty() {
        return Err(refuse(
            "No DAT catalogue is loaded, so EmuWiz cannot rule out catalogue evidence against this disk.",
        ));
    }
    // Only "no DAT evidence at all" permits the local route. A catalogue
    // that matched the disk, disagrees, is stale or could not be loaded
    // (which surfaces as stale) blocks it, as does a verified identity: that
    // disk belongs to the catalogue route.
    match database
        .amiga_adf_launch_identity(path, catalogues)
        .map_err(|error| FsUaeAdfLaunchError::Database(error.to_string()))?
    {
        Err(refusal) if refusal.permits_local_media() => {}
        Err(refusal) => return Err(FsUaeAdfLaunchError::Identity(refusal)),
        Ok(_) => {
            return Err(refuse(
                "The catalogue now identifies this disk; check it again.",
            ));
        }
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
    let key = local_key(sha256);
    let status = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Amiga".into(),
        game_key: key.clone(),
    });
    preflight_fsuae_launch(
        &request.fsuae,
        &request.roots,
        &status,
        &[VerifiedIdentityFact::AmigaIdentity(key)],
    )
    .map_err(FsUaeAdfLaunchError::Preflight)
}
