//! Guidance and launch-note wording on synthetic, disposable disc sets.
use super::*;
use archivefs_core::launch::MultiDiscLaunch;
use archivefs_core::media_set::{
    EvidenceKind, ExpectedCount, IdentityKey, MediaAvailability, MediaEvidence, MediaFamily,
    MediaOrdinal, MediaRecord, MediaRole, MediaSource, OrdinalUnit, SideLayout, index_media,
    resolve_index,
};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let root = std::env::temp_dir().join(format!(
        "emuwiz-gui-multidisc-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

/// A verified PS1 set of `count` real files, in order.
pub(crate) fn verified_set(root: &Path, count: u16) -> (MediaSet, Vec<PathBuf>) {
    let mut records = Vec::new();
    let mut paths = Vec::new();
    for number in 1..=count {
        let path = root.join(format!("Game (Disc {number}).cue"));
        let bin = path.with_extension("bin");
        fs::write(&bin, vec![number as u8; 2352]).unwrap();
        fs::write(
            &path,
            format!(
                "FILE \"{}\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
                bin.file_name().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();
        paths.push(path.clone());
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
    assert_eq!(set.state, MediaSetState::CompleteSet);
    (set, paths)
}

#[test]
fn ordered_sets_show_this_entry_start_and_next_discs() {
    for count in [2u16, 3, 4] {
        let root = temp_root("ordered");
        let (set, paths) = verified_set(&root, count);
        let g = guidance(&set, &paths[1], None);
        assert!(g.order_verified, "{g:?}");
        assert_eq!(g.headline, format!("{count}-disc game · order verified"));
        assert_eq!(g.discs.len(), count as usize);
        assert!(g.discs[0].start && !g.discs[0].this_entry);
        assert!(g.discs[1].this_entry && !g.discs[1].start);
        assert_eq!(
            g.next_discs,
            (3..=count).map(|n| format!("Disc {n}")).collect::<Vec<_>>()
        );
        assert!(g.problems.is_empty(), "{:?}", g.problems);
        assert_eq!(g.launch, None, "no decision, no launch claim");
    }
}

#[test]
fn missing_intermediate_disc_is_shown_and_order_is_not_claimed() {
    let root = temp_root("missing");
    let (mut set, paths) = verified_set(&root, 3);
    set.members.remove(1);
    set.state = MediaSetState::IncompleteSet;
    let g = guidance(&set, &paths[0], None);
    assert!(!g.order_verified);
    assert!(g.headline.contains("order not verified"));
    assert!(
        g.discs.iter().all(|d| !d.start),
        "no start claim without verified order"
    );
    assert!(g.next_discs.is_empty());
    assert!(!g.problems.is_empty());
}

#[test]
fn duplicate_ordinals_are_reported_not_ordered() {
    let root = temp_root("duplicate");
    let (mut set, paths) = verified_set(&root, 2);
    set.members[1].ordinal = set.members[0].ordinal.clone();
    set.members[1].representations[0].ordinal = set.members[0].ordinal.clone();
    let g = guidance(&set, &paths[0], None);
    assert!(!g.order_verified, "{g:?}");
    assert!(!g.problems.is_empty());
}

#[test]
fn a_disc_removed_after_scanning_is_reported_unavailable() {
    let root = temp_root("removed");
    let (set, paths) = verified_set(&root, 2);
    fs::remove_file(&paths[1]).unwrap();
    let g = guidance(&set, &paths[0], None);
    assert!(
        g.problems.iter().any(|p| p.contains("not available")),
        "{:?}",
        g.problems
    );
}

#[test]
fn launch_notes_never_claim_emuwiz_changes_discs() {
    assert_eq!(launch_note(&MultiDiscLaunch::NotMultiDisc), None);
    let single = launch_note(&MultiDiscLaunch::SingleDisc { reason: "x".into() }).unwrap();
    assert!(single.starts_with("Only this disc is launched"), "{single}");
    let refused = launch_note(&MultiDiscLaunch::Refused { reason: "y".into() }).unwrap();
    assert!(refused.contains("not available"), "{refused}");
    let root = temp_root("note");
    let (set, paths) = verified_set(&root, 3);
    let target = archivefs_core::launch::LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "p".into(),
        profile_path: None,
    };
    let decision = archivefs_core::launch::plan_multidisc_launch(
        &[set],
        &paths[0],
        &target,
        None,
        false,
        &root.join("lists"),
    );
    let note = launch_note(&decision).unwrap();
    assert!(note.contains("playlist of all 3 discs"), "{note}");
    assert!(note.contains("own disc controls"), "{note}");
    assert!(note.contains("does not change discs"), "{note}");
}

#[test]
fn context_keeps_only_sets_containing_the_focused_game() {
    let a = temp_root("ctx-a");
    let b = temp_root("ctx-b");
    let (set_a, paths_a) = verified_set(&a, 2);
    let (set_b, _) = verified_set(&b, 2);
    let all = vec![set_a.clone(), set_b];
    let archives = catalogue_archives(&paths_a);
    let context = MultiDiscLaunchContext::for_content(&all, &paths_a[1], &archives).unwrap();
    assert_eq!(context.sets.as_slice(), &[set_a]);
    assert_eq!(context.catalogue.len(), 2);
    assert!(MultiDiscLaunchContext::for_content(&all, &a.join("other.cue"), &archives).is_none());
}

/// Catalogue rows matching the files exactly as they are now.
pub(crate) fn catalogue_archives(paths: &[PathBuf]) -> Vec<archivefs_core::PersistedArchive> {
    use std::os::unix::fs::MetadataExt;
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let metadata = fs::metadata(path).unwrap();
            archivefs_core::PersistedArchive {
                id: index as i64 + 1,
                source_folder_id: 1,
                relative_path: PathBuf::from(path.file_name().unwrap()),
                absolute_path: path.clone(),
                archive_kind: "direct".into(),
                display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
                normalized_name: path.file_stem().unwrap().to_string_lossy().into_owned(),
                size_bytes: Some(metadata.len()),
                modified_time_unix_seconds: Some(metadata.mtime()),
                platform: Some("PSX".into()),
                platform_source: None,
                last_known_health: "ok".into(),
                last_seen_at: "1".into(),
                last_verified_missing_at: None,
                identity_report: None,
            }
        })
        .collect()
}

/// A context for `set` with a catalogue matching the files as they are now.
pub(crate) fn context_for(
    set: MediaSet,
    paths: &[PathBuf],
    lists: PathBuf,
) -> MultiDiscLaunchContext {
    let archives = catalogue_archives(paths);
    let mut context =
        MultiDiscLaunchContext::for_content(std::slice::from_ref(&set), &paths[0], &archives)
            .unwrap();
    context.playlist_root = lists;
    context
}

pub(crate) fn duckstation_candidate(content: &Path) -> archivefs_core::launch::LaunchCandidate {
    archivefs_core::launch::LaunchCandidate {
        target: archivefs_core::launch::LaunchTarget::Standalone {
            adapter_id: "duckstation",
            profile_id: "p".into(),
            profile_path: None,
        },
        content: archivefs_core::launch::LaunchContentRef {
            kind: None,
            container: Some(archivefs_core::launch::LaunchContainerKind::PlainFile),
            resolved_path: Some(content.to_path_buf()),
            requires_mount: false,
            provenance: "test".into(),
        },
        firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: archivefs_core::launch::LaunchReadiness::Ready,
        preference: archivefs_core::launch::CandidatePreference::SoleEligible,
    }
}

#[test]
fn a_disc_changed_since_the_library_scan_never_goes_into_a_playlist() {
    let root = temp_root("scan");
    let (set, paths) = verified_set(&root, 3);
    let context = context_for(set, &paths, root.join("lists"));
    let candidate = duckstation_candidate(&paths[0]);
    assert!(matches!(
        context.decide(&candidate, None, false),
        MultiDiscLaunch::Playlist(_)
    ));
    fs::write(
        &paths[2],
        "FILE \"other.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
    )
    .unwrap();
    fs::write(paths[2].with_file_name("other.bin"), vec![9u8; 2352]).unwrap();
    match context.decide(&candidate, None, false) {
        MultiDiscLaunch::SingleDisc { reason } => {
            assert!(
                reason.contains("differs from the last library scan"),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn catalogue_derived_sets_are_filename_evidence_and_never_get_a_playlist() {
    // Real libraries today: no saved topology, sets derived from the catalogue.
    let root = temp_root("catalogue");
    let (_, paths) = verified_set(&root, 2);
    let archives = catalogue_archives(&paths);
    let sets = sets_from(&[], &archives, &Default::default());
    let context = MultiDiscLaunchContext::for_content(&sets, &paths[0], &archives)
        .expect("the catalogue groups the two discs");
    match context.decide(&duckstation_candidate(&paths[0]), None, false) {
        MultiDiscLaunch::SingleDisc { reason } => {
            assert!(reason.contains("rests only on file names"), "{reason}");
            let note = launch_note(&MultiDiscLaunch::SingleDisc { reason }).unwrap();
            assert_eq!(note.matches("Only this disc").count(), 1, "{note}");
            assert!(!note.contains("only the selected disc"), "{note}");
        }
        other => panic!("filename evidence must never become a playlist: {other:?}"),
    }
    let g = guidance(context.sets.first().unwrap(), &paths[0], None);
    assert!(!g.order_verified);
    assert_eq!(g.discs.len(), 2);
}

#[test]
fn other_emulators_get_an_explanation_not_silence() {
    let root = temp_root("dolphin");
    let (set, paths) = verified_set(&root, 2);
    let context = context_for(set, &paths, root.join("lists"));
    let mut candidate = duckstation_candidate(&paths[0]);
    candidate.target = archivefs_core::launch::LaunchTarget::Standalone {
        adapter_id: "pcsx2",
        profile_id: "p".into(),
        profile_path: None,
    };
    let decision = crate::launch_readiness_page::multidisc_decision_with(
        &candidate,
        None,
        None,
        Some(&context),
    );
    let note = launch_note(&decision).unwrap();
    assert!(
        note.contains("no verified playlist launch for pcsx2"),
        "{note}"
    );
}

fn catalogue_set(
    root: &Path,
    family: MediaFamily,
    names: &[&str],
    total: Option<u16>,
) -> (MediaSet, Vec<PathBuf>) {
    use archivefs_core::media_set::dat_disc_order::{
        DatDiscCandidate, dat_disc_record, dat_disc_token,
    };
    let extension = match family {
        MediaFamily::Floppy => "adf",
        MediaFamily::Tape => "tzx",
        MediaFamily::Optical => "iso",
    };
    let mut paths = Vec::new();
    let records = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let path = root.join(format!("{index}.{extension}"));
            fs::write(&path, name).unwrap();
            paths.push(path.clone());
            let candidate = DatDiscCandidate {
                path,
                platform: "Amiga".into(),
                format: extension.into(),
                source_id: "tosec".into(),
                source_revision: "1".into(),
                catalogue_sha256: None,
                canonical_name: (*name).into(),
                matched_algorithm: "SHA-1".into(),
                matched_value: None,
            };
            let token = dat_disc_token(name).unwrap();
            dat_disc_record(&candidate, family, &token, total.or(token.total))
        })
        .collect();
    (resolve_index(index_media(records)).sets.remove(0), paths)
}

#[test]
fn catalogue_ordered_floppies_say_disk_and_an_unknown_total_is_explained() {
    let root = temp_root("floppy");
    let (set, paths) = catalogue_set(
        &root,
        MediaFamily::Floppy,
        &[
            "Orbit (1989)(Acme)(Disk 2 of 2)",
            "Orbit (1989)(Acme)(Disk 1 of 2)",
        ],
        None,
    );
    let g = guidance(&set, &paths[1], None);
    assert!(g.order_verified, "{g:?}");
    assert_eq!(g.headline, "2-disk game · order verified");
    assert_eq!(g.discs[0].label, "Disk 1");
    assert_eq!(g.discs[0].path.as_deref(), Some(paths[1].as_path()));

    let root = temp_root("unknown-total");
    let (set, paths) = catalogue_set(
        &root,
        MediaFamily::Optical,
        &["Orbit (USA) (Disc 1)", "Orbit (USA) (Disc 2)"],
        None,
    );
    let g = guidance(&set, &paths[0], None);
    assert!(!g.order_verified);
    assert!(
        g.problems.iter().any(|p| p.contains("not how many discs")),
        "{:?}",
        g.problems
    );
}
