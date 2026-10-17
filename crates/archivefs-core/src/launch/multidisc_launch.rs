//! Launch-time handoff of a verified multi-disc set as an EmuWiz-managed M3U.
//!
//! One decision, [`plan_multidisc_launch`], is used by both Preview and
//! Launch, so they cannot disagree. A playlist is used only when ALL hold:
//! the selected file belongs to a media set, the selected emulator has
//! verified playlist input (DuckStation; or a RetroArch core whose own
//! installed `.info` lists `m3u`), and the existing reviewed composer in
//! [`super::media_handoff`] verifies the complete set for that emulator's
//! profile (state, explicit ordinals, duplicates, conflicts, missing members,
//! regular non-symlink files). Everything else keeps the ordinary single-disc
//! launch and says why.
//!
//! A playlist only lets the emulator offer its own disc-change controls.
//! EmuWiz does not change discs in a running emulator.
//!
//! Launch re-captures every disc and refuses if anything changed since
//! Preview, stages the playlist through the existing managed writer (never
//! replacing a non-managed file), and re-checks every disc and the exact
//! playlist bytes immediately before spawn.
use super::media_handoff::{
    MediaLaunchAdapter, duckstation_multimedia_profile, managed_playlist_body,
    managed_playlist_path, retroarch_multimedia_profile, stage_managed_playlist, verified_media,
};
use super::planning::LaunchTarget;
use super::process_spawn::{CapturedFileIdentity, capture_file_identity};
use crate::media_set::{MediaProfile, MediaSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The verified playlist a launch will use, with the disc identities Preview saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiDiscPlaylist {
    pub set_id: String,
    pub emulator: String,
    /// Verified order, start disc first.
    pub discs: Vec<PathBuf>,
    pub disc_identities: Vec<CapturedFileIdentity>,
    /// Deterministic managed path under the playlist root; nothing is written
    /// until [`stage_multidisc_playlist`].
    pub playlist_path: PathBuf,
    pub playlist_root: PathBuf,
    profile: MediaProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiDiscLaunch {
    /// The selected file is not part of a multi-member media set.
    NotMultiDisc,
    /// Launch hands the emulator the managed playlist.
    Playlist(MultiDiscPlaylist),
    /// Ordinary single-disc launch of the selected file, with the reason a
    /// playlist is not used. Shown in Preview and Game Details.
    SingleDisc { reason: String },
    /// A playlist should be used but cannot be prepared safely; Launch refuses
    /// rather than silently starting one disc.
    Refused { reason: String },
}

fn member_of<'a>(sets: &'a [MediaSet], selected: &Path) -> Option<&'a MediaSet> {
    sets.iter().find(|set| {
        set.members.len() > 1
            && set.members.iter().any(|member| {
                member.representations.iter().any(|r| {
                    r.record.source.archive_member.is_none() && r.record.source.path == selected
                })
            })
    })
}

/// Verified playlist capability of the selected launch target. `Err` is the
/// user-facing reason the ordinary single-disc launch is kept. `sandboxed` is
/// true for a Flatpak installation: its sandbox is granted only the selected
/// disc's folder, so it could read neither the managed playlist nor discs kept
/// elsewhere, and no wider grant is made here.
pub fn playlist_capability(
    target: &LaunchTarget,
    retroarch_core_extensions: Option<&[String]>,
    sandboxed: bool,
) -> Result<(MediaLaunchAdapter, MediaProfile, String), String> {
    let flatpak_retroarch = matches!(target, LaunchTarget::RetroArchCore { profile, .. }
        if profile.profile_kind == crate::emulator_environment::retroarch::ProfileKind::Flatpak);
    if sandboxed || flatpak_retroarch {
        return Err(
            "this emulator runs in a Flatpak sandbox that can only see the selected disc's folder, so no playlist is handed over"
                .into(),
        );
    }
    match target {
        LaunchTarget::Standalone { adapter_id, .. } if *adapter_id == "duckstation" => Ok((
            MediaLaunchAdapter::DuckStation,
            duckstation_multimedia_profile(),
            "DuckStation".into(),
        )),
        LaunchTarget::Standalone { adapter_id, .. } => Err(format!(
            "EmuWiz has no verified playlist launch for {adapter_id}"
        )),
        LaunchTarget::RetroArchCore { core_stem, .. } => {
            let lists_m3u = retroarch_core_extensions.is_some_and(|extensions| {
                extensions
                    .iter()
                    .any(|extension| extension.trim().eq_ignore_ascii_case("m3u"))
            });
            if lists_m3u {
                Ok((
                    MediaLaunchAdapter::RetroArch,
                    retroarch_multimedia_profile(core_stem.clone()),
                    format!("RetroArch ({core_stem})"),
                ))
            } else {
                Err(format!(
                    "the installed RetroArch core {core_stem} does not list m3u playlists in its own core information"
                ))
            }
        }
    }
}

/// The single decision shared by Preview and Launch. Read-only: captures file
/// identities and computes the managed playlist path, writes nothing.
pub fn plan_multidisc_launch(
    sets: &[MediaSet],
    selected: &Path,
    target: &LaunchTarget,
    retroarch_core_extensions: Option<&[String]>,
    sandboxed: bool,
    playlist_root: &Path,
) -> MultiDiscLaunch {
    let Some(set) = member_of(sets, selected) else {
        return MultiDiscLaunch::NotMultiDisc;
    };
    let (_adapter, profile, emulator) =
        match playlist_capability(target, retroarch_core_extensions, sandboxed) {
            Ok(capability) => capability,
            Err(reason) => {
                return MultiDiscLaunch::SingleDisc {
                    reason: format!("{reason}; only the selected disc is launched"),
                };
            }
        };
    let (_plan, discs, _family) = match verified_media(set, &profile) {
        Ok(verified) => verified,
        Err(error) => {
            return MultiDiscLaunch::SingleDisc {
                reason: format!(
                    "the disc set is not verified for a playlist ({}); only the selected disc is launched",
                    error.detail
                ),
            };
        }
    };
    if discs.len() < 2 {
        return MultiDiscLaunch::NotMultiDisc;
    }
    // Every disc, not just the selected one, must be readable by the target:
    // a RetroArch core must list each disc's own format, and every CUE's
    // referenced files must be present and safe.
    if let LaunchTarget::RetroArchCore { core_stem, .. } = target {
        let listed = |extension: &str| {
            retroarch_core_extensions.is_some_and(|all| {
                all.iter()
                    .any(|listed| listed.trim().eq_ignore_ascii_case(extension))
            })
        };
        if let Some(disc) = discs.iter().find(|disc| {
            disc.extension()
                .and_then(|extension| extension.to_str())
                .is_none_or(|extension| !listed(extension))
        }) {
            return MultiDiscLaunch::SingleDisc {
                reason: format!(
                    "the installed RetroArch core {core_stem} does not list the format of {}; only the selected disc is launched",
                    disc.display()
                ),
            };
        }
    }
    if let Err(reason) = cue_companions_ok(&discs) {
        return MultiDiscLaunch::SingleDisc {
            reason: format!("{reason}; only the selected disc is launched"),
        };
    }
    if !discs.iter().any(|disc| disc == selected) {
        return MultiDiscLaunch::SingleDisc {
            reason: "the selected file is not one of the verified discs for this emulator; only the selected disc is launched".into(),
        };
    }
    let mut disc_identities = Vec::with_capacity(discs.len());
    for disc in &discs {
        match capture_file_identity(disc) {
            Ok((metadata, identity)) if metadata.is_file() => disc_identities.push(identity),
            _ => {
                return MultiDiscLaunch::SingleDisc {
                    reason: format!(
                        "{} could not be read; only the selected disc is launched",
                        disc.display()
                    ),
                };
            }
        }
    }
    let playlist_path = match managed_playlist_path(playlist_root, &set.identity.key.value, &discs)
    {
        Ok(path) => path,
        Err(error) => {
            return MultiDiscLaunch::Refused {
                reason: format!("the managed playlist location is unsafe: {}", error.detail),
            };
        }
    };
    MultiDiscLaunch::Playlist(MultiDiscPlaylist {
        set_id: set.identity.key.value.clone(),
        emulator,
        discs,
        disc_identities,
        playlist_path,
        playlist_root: playlist_root.to_path_buf(),
        profile,
    })
}

fn cue_companions_ok(discs: &[PathBuf]) -> Result<(), String> {
    for disc in discs {
        let is_cue = disc
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"));
        if is_cue {
            crate::ingestion::cue_bin::resolve_cue_all_files(disc).map_err(|error| {
                format!("{} is incomplete or unsafe ({error:?})", disc.display())
            })?;
        }
    }
    Ok(())
}

fn find_set<'a>(
    sets: &'a [MediaSet],
    playlist: &MultiDiscPlaylist,
) -> Result<&'a MediaSet, String> {
    sets.iter()
        .find(|set| set.identity.key.value == playlist.set_id)
        .ok_or_else(|| "the disc set is no longer known; review Game Details again".to_string())
}

fn recheck_discs(playlist: &MultiDiscPlaylist) -> Result<(), String> {
    for (disc, expected) in playlist.discs.iter().zip(&playlist.disc_identities) {
        match capture_file_identity(disc) {
            Ok((metadata, current)) if metadata.is_file() && &current == expected => (),
            _ => {
                return Err(format!(
                    "{} changed or disappeared after Preview; review Game Details again",
                    disc.display()
                ));
            }
        }
    }
    Ok(())
}

/// Launch time: re-verifies the set with the same composer, refuses any disc
/// change since Preview, then stages the managed playlist. Returns its path.
pub fn stage_multidisc_playlist(
    sets: &[MediaSet],
    playlist: &MultiDiscPlaylist,
) -> Result<PathBuf, String> {
    let set = find_set(sets, playlist)?;
    recheck_discs(playlist)?;
    let (plan, discs, _family) = verified_media(set, &playlist.profile)
        .map_err(|error| format!("the disc set is no longer verified: {}", error.detail))?;
    cue_companions_ok(&discs)?;
    if discs != playlist.discs {
        return Err(
            "the verified disc order changed after Preview; review Game Details again".into(),
        );
    }
    let staged =
        stage_managed_playlist(&playlist.playlist_root, &playlist.set_id, &plan, &discs)
            .map_err(|error| format!("the playlist could not be prepared: {}", error.detail))?;
    if staged != playlist.playlist_path {
        return Err(
            "the staged playlist path differs from Preview; review Game Details again".into(),
        );
    }
    recheck_discs(playlist)?;
    Ok(staged)
}

/// Immediately before spawn: every disc is unchanged and the playlist is a
/// regular non-symlink file holding exactly the expected bytes.
pub fn recheck_multidisc_playlist(
    sets: &[MediaSet],
    playlist: &MultiDiscPlaylist,
) -> Result<(), String> {
    recheck_discs(playlist)?;
    let set = find_set(sets, playlist)?;
    let (plan, discs, _family) = verified_media(set, &playlist.profile)
        .map_err(|error| format!("the disc set is no longer verified: {}", error.detail))?;
    let metadata = std::fs::symlink_metadata(&playlist.playlist_path)
        .map_err(|error| format!("the staged playlist is missing: {error}"))?;
    if !metadata.is_file() {
        return Err("the staged playlist is not a regular file".into());
    }
    let actual = std::fs::read(&playlist.playlist_path)
        .map_err(|error| format!("the staged playlist cannot be read: {error}"))?;
    if actual != managed_playlist_body(&plan, &discs).into_bytes() {
        return Err("the staged playlist changed before launch".into());
    }
    Ok(())
}

/// Replaces the content argument with the playlist. Exactly one argument must
/// equal the verified content path; anything else refuses.
pub fn substitute_content_argument(
    arguments: &mut [OsString],
    content: &Path,
    playlist: &Path,
) -> Result<(), String> {
    let wanted = content.as_os_str();
    let positions: Vec<usize> = arguments
        .iter()
        .enumerate()
        .filter(|(_, argument)| argument.as_os_str() == wanted)
        .map(|(index, _)| index)
        .collect();
    let [index] = positions.as_slice() else {
        return Err(format!(
            "the launch command contains the selected disc {} times, not exactly once; refusing to substitute the playlist",
            positions.len()
        ));
    };
    arguments[*index] = playlist.as_os_str().to_os_string();
    Ok(())
}

#[cfg(test)]
#[path = "multidisc_launch_tests.rs"]
pub(crate) mod tests;
