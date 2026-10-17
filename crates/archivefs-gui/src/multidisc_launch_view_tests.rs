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
        fs::write(&path, format!("disc {number}")).unwrap();
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
    let context = MultiDiscLaunchContext::for_content(&all, &paths_a[1]).unwrap();
    assert_eq!(context.sets.as_slice(), &[set_a]);
    assert!(MultiDiscLaunchContext::for_content(&all, &a.join("other.cue")).is_none());
}
