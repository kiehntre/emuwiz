//! Synthetic two-, three- and four-disc sets on disposable temp files.
use super::*;
use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use crate::media_set::{
    ConflictKind, EvidenceKind, ExpectedCount, IdentityKey, MediaAvailability, MediaEvidence,
    MediaFamily, MediaOrdinal, MediaRecord, MediaRole, MediaSetConflict, MediaSetState,
    MediaSource, OrdinalUnit, SideLayout, index_media, resolve_index,
};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

fn root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let root = std::env::temp_dir().join(format!(
        "emuwiz-multidisc-launch-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}
fn set(root: &Path, count: u16) -> MediaSet {
    let paths: Vec<PathBuf> = (1..=count)
        .map(|number| {
            let path = root.join(format!("Game (Disc {number}).cue"));
            fs::write(&path, format!("disc {number}")).unwrap();
            path
        })
        .collect();
    verified_set_with(&paths)
}
/// A verified PS1 set whose discs are exactly `paths`, in order (files must exist).
pub(crate) fn verified_set_with(paths: &[PathBuf]) -> MediaSet {
    let count = paths.len() as u16;
    let mut records = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let number = index as u16 + 1;
        let path = path.clone();
        let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, "verified manifest");
        evidence.release = Some(IdentityKey::new("release", "game"));
        evidence.medium = Some(IdentityKey::new("medium", number.to_string()));
        evidence.ordinal = Some(MediaOrdinal {
            number,
            unit: OrdinalUnit::Medium,
        });
        evidence.expected_count = Some(ExpectedCount {
            count,
            unit: OrdinalUnit::Medium,
        });
        evidence.role = Some(MediaRole::GameMedia);
        evidence.side_layout = Some(SideLayout::WholeMedium);
        records.push(MediaRecord {
            source: MediaSource {
                path,
                archive_member: None,
            },
            platform: Some("PSX".into()),
            family: Some(MediaFamily::Optical),
            format: "cue".into(),
            availability: MediaAvailability::Observed,
            evidence: vec![evidence],
            warnings: Vec::new(),
        });
    }
    let set = resolve_index(index_media(records))
        .sets
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(set.state, MediaSetState::CompleteSet, "{:?}", set.conflicts);
    set
}
fn disc(root: &Path, n: u16) -> PathBuf {
    root.join(format!("Game (Disc {n}).cue"))
}
fn duckstation() -> LaunchTarget {
    LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "duck".into(),
        profile_path: None,
    }
}
fn retroarch(kind: ProfileKind) -> LaunchTarget {
    LaunchTarget::RetroArchCore {
        profile: ProfileRef {
            profile_kind: kind,
            scope: ProfileScope::User,
        },
        core_stem: "mednafen_psx_libretro".into(),
        platform_id: "psx",
    }
}
fn playlist(decision: MultiDiscLaunch) -> MultiDiscPlaylist {
    match decision {
        MultiDiscLaunch::Playlist(p) => p,
        other => panic!("expected a playlist, got {other:?}"),
    }
}
fn single(decision: MultiDiscLaunch) -> String {
    match decision {
        MultiDiscLaunch::SingleDisc { reason } => reason,
        other => panic!("expected single-disc, got {other:?}"),
    }
}

#[test]
fn verified_two_three_and_four_disc_sets_stage_ordered_playlists() {
    for count in [2u16, 3, 4] {
        let r = root("ordered");
        let sets = vec![set(&r, count)];
        let cache = r.join("playlists");
        let p = playlist(plan_multidisc_launch(
            &sets,
            &disc(&r, 1),
            &duckstation(),
            None,
            false,
            &cache,
        ));
        assert_eq!(
            p.discs,
            (1..=count).map(|n| disc(&r, n)).collect::<Vec<_>>()
        );
        assert!(!p.playlist_path.exists(), "Preview must not write anything");
        let staged = stage_multidisc_playlist(&sets, &p).unwrap();
        assert_eq!(
            staged, p.playlist_path,
            "Preview and Launch name the same playlist"
        );
        let body = fs::read_to_string(&staged).unwrap();
        let listed: Vec<_> = body.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(
            listed,
            p.discs
                .iter()
                .map(|d| d.to_str().unwrap())
                .collect::<Vec<_>>(),
            "{count} discs in verified order"
        );
        recheck_multidisc_playlist(&sets, &p).unwrap();
        // Staging twice is idempotent: same managed file, same bytes.
        assert_eq!(stage_multidisc_playlist(&sets, &p).unwrap(), staged);
        assert_eq!(fs::read_to_string(&staged).unwrap(), body);
    }
}

#[test]
fn selecting_a_later_disc_still_starts_from_the_verified_first_disc() {
    let r = root("later");
    let sets = vec![set(&r, 3)];
    let p = playlist(plan_multidisc_launch(
        &sets,
        &disc(&r, 2),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    assert_eq!(p.discs[0], disc(&r, 1));
}

#[test]
fn preview_and_launch_decisions_are_identical() {
    let r = root("consistent");
    let sets = vec![set(&r, 2)];
    let a = plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    );
    let b = plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    );
    assert_eq!(a, b);
}

#[test]
fn missing_intermediate_disc_keeps_single_disc_launch_with_reason() {
    let r = root("missing");
    let mut s = set(&r, 3);
    s.members.remove(1);
    s.state = MediaSetState::IncompleteSet;
    let reason = single(plan_multidisc_launch(
        &[s],
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    assert!(reason.contains("not verified"), "{reason}");
    assert!(reason.contains("only the selected disc"), "{reason}");
}

#[test]
fn duplicate_ordinals_and_conflicting_identities_keep_single_disc() {
    let r = root("duplicate");
    let mut s = set(&r, 2);
    s.members[1].ordinal = s.members[0].ordinal.clone();
    s.members[1].representations[0].ordinal = s.members[0].ordinal.clone();
    single(plan_multidisc_launch(
        &[s],
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    let r = root("conflict");
    let mut s = set(&r, 2);
    s.conflicts.push(MediaSetConflict {
        kind: ConflictKind::IdentityConflict,
        detail: "discs claim different regions".into(),
        blocking: true,
    });
    let reason = single(plan_multidisc_launch(
        &[s],
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    assert!(reason.contains("regions"), "{reason}");
}

#[test]
fn changed_or_removed_disc_between_preview_and_launch_is_refused() {
    let r = root("changed");
    let sets = vec![set(&r, 3)];
    let p = playlist(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    fs::write(disc(&r, 2), b"replaced bytes of a different length").unwrap();
    let error = stage_multidisc_playlist(&sets, &p).unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert!(!p.playlist_path.exists());
    let r = root("removed");
    let sets = vec![set(&r, 2)];
    let p = playlist(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    fs::remove_file(disc(&r, 2)).unwrap();
    assert!(stage_multidisc_playlist(&sets, &p).is_err());
}

#[test]
fn only_verified_playlist_emulators_receive_a_playlist() {
    let r = root("capability");
    let sets = vec![set(&r, 2)];
    let c = r.join("c");
    let pcsx2 = LaunchTarget::Standalone {
        adapter_id: "pcsx2",
        profile_id: "p".into(),
        profile_path: None,
    };
    assert!(
        single(plan_multidisc_launch(
            &sets,
            &disc(&r, 1),
            &pcsx2,
            None,
            false,
            &c
        ))
        .contains("no verified playlist")
    );
    // RetroArch depends on the installed core's own .info, never on the name.
    let no_info = single(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &retroarch(ProfileKind::Native),
        None,
        false,
        &c,
    ));
    assert!(no_info.contains("does not list m3u"), "{no_info}");
    let without: Vec<String> = ["cue", "chd", "pbp"].map(String::from).to_vec();
    single(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &retroarch(ProfileKind::Native),
        Some(&without),
        false,
        &c,
    ));
    let with: Vec<String> = ["cue", "M3U", "chd"].map(String::from).to_vec();
    playlist(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &retroarch(ProfileKind::AppImage),
        Some(&with),
        false,
        &c,
    ));
    // Sandboxed installations only see the selected disc's folder.
    assert!(
        single(plan_multidisc_launch(
            &sets,
            &disc(&r, 1),
            &duckstation(),
            None,
            true,
            &c
        ))
        .contains("Flatpak")
    );
    single(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &retroarch(ProfileKind::Flatpak),
        Some(&with),
        false,
        &c,
    ));
}

#[test]
fn single_disc_games_are_not_multidisc() {
    let r = root("single");
    let lone = r.join("Lone.cue");
    fs::write(&lone, b"x").unwrap();
    let sets = vec![set(&r, 2)];
    assert_eq!(
        plan_multidisc_launch(&sets, &lone, &duckstation(), None, false, &r.join("c")),
        MultiDiscLaunch::NotMultiDisc
    );
    assert_eq!(
        plan_multidisc_launch(&[], &disc(&r, 1), &duckstation(), None, false, &r.join("c")),
        MultiDiscLaunch::NotMultiDisc
    );
}

#[test]
fn failed_or_unsafe_staging_never_overwrites_and_refuses() {
    // A user's own file at the managed path is never replaced.
    let r = root("occupied");
    let sets = vec![set(&r, 2)];
    let p = playlist(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    fs::create_dir_all(p.playlist_path.parent().unwrap()).unwrap();
    fs::write(&p.playlist_path, b"my own playlist").unwrap();
    assert!(
        stage_multidisc_playlist(&sets, &p)
            .unwrap_err()
            .contains("could not be prepared")
    );
    assert_eq!(fs::read(&p.playlist_path).unwrap(), b"my own playlist");
    // A symlinked playlist root is refused before anything is planned to run.
    let r = root("symlink-root");
    let sets = vec![set(&r, 2)];
    let real = r.join("real");
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, r.join("link")).unwrap();
    match plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("link").join("c"),
    ) {
        MultiDiscLaunch::Refused { reason } => assert!(reason.contains("unsafe"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // Relative roots are refused too.
    assert!(matches!(
        plan_multidisc_launch(
            &sets,
            &disc(&r, 1),
            &duckstation(),
            None,
            false,
            Path::new("relative")
        ),
        MultiDiscLaunch::Refused { .. }
    ));
}

#[test]
fn tampered_playlist_is_refused_before_spawn() {
    let r = root("tamper");
    let sets = vec![set(&r, 2)];
    let p = playlist(plan_multidisc_launch(
        &sets,
        &disc(&r, 1),
        &duckstation(),
        None,
        false,
        &r.join("c"),
    ));
    let staged = stage_multidisc_playlist(&sets, &p).unwrap();
    let mut body = fs::read_to_string(&staged).unwrap();
    body.push_str("/somewhere/else.cue\n");
    fs::write(&staged, body).unwrap();
    assert!(
        recheck_multidisc_playlist(&sets, &p)
            .unwrap_err()
            .contains("changed")
    );
}

#[test]
fn content_argument_is_substituted_exactly_once_or_refused() {
    let content = Path::new("/games/Game (Disc 1).cue");
    let list = Path::new("/data/playlists/x.m3u");
    let mut args: Vec<OsString> = ["-batch", "--", "/games/Game (Disc 1).cue"]
        .map(OsString::from)
        .to_vec();
    substitute_content_argument(&mut args, content, list).unwrap();
    assert_eq!(args.last().unwrap(), list.as_os_str());
    let mut none: Vec<OsString> = ["-batch"].map(OsString::from).to_vec();
    assert!(substitute_content_argument(&mut none, content, list).is_err());
    let mut twice: Vec<OsString> = [content, content]
        .map(|p| p.as_os_str().to_os_string())
        .to_vec();
    assert!(substitute_content_argument(&mut twice, content, list).is_err());
}
