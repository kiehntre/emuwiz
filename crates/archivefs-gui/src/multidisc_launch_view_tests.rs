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
        assert_eq!(
            g.selected_launch_disc.as_deref(),
            Some("Disc 2 (selected entry)")
        );
        assert!(
            g.provenance
                .iter()
                .any(|source| source.contains("verified manifest"))
        );
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
    assert_eq!(g.headline, "3-disc game · order not verified");
    assert_eq!(g.missing_media, ["Disc 2 of 3"]);
}

#[test]
fn a_lone_disc_of_a_larger_set_still_names_what_is_missing() {
    let root = temp_root("lone");
    let (mut set, paths) = verified_set(&root, 3);
    set.members.truncate(1);
    set.state = MediaSetState::IncompleteSet;
    let context = MultiDiscLaunchContext::for_content(std::slice::from_ref(&set), &paths[0], &[])
        .expect("a lone disc of a three-disc set is still a multi-disc game");
    let g = guidance(&context.sets[0], &paths[0], None);
    assert_eq!(g.headline, "3-disc game · order not verified");
    assert_eq!(g.missing_media, ["Disc 2 of 3", "Disc 3 of 3"]);

    set.expected_count = None;
    assert!(
        MultiDiscLaunchContext::for_content(std::slice::from_ref(&set), &paths[0], &[]).is_none(),
        "a single file with no expected total stays a single-disc game"
    );
}

/// Guidance the library would show for files known only by name.
fn named_guidance(label: &str, names: &[&str]) -> Option<DiscGuidance> {
    let root = temp_root(label);
    let paths: Vec<PathBuf> = names.iter().map(|name| root.join(name)).collect();
    for path in &paths {
        fs::write(path, [0u8; 16]).unwrap();
    }
    let archives = catalogue_archives(&paths);
    let sets = sets_from(&[], &archives);
    let context = MultiDiscLaunchContext::for_content(&sets, &paths[0], &archives)?;
    Some(guidance(&context.sets[0], &paths[0], None))
}

#[test]
fn a_lone_disc_named_by_filename_lists_the_rest() {
    let g = named_guidance("lone-name", &["Game (Disc 1 of 3).iso"]).unwrap();
    assert_eq!(g.headline, "3-disc game · order not verified");
    assert_eq!(g.missing_media, ["Disc 2 of 3", "Disc 3 of 3"]);
}

#[test]
fn dates_and_bare_ranges_never_invent_a_multi_disc_game() {
    for name in [
        "SkidMarks - Racer Disk Issue 1 (1995-08)(Acid)[h SR][addons].adf",
        "Compilation (1990-1996).adf",
        "Collection (1-4).iso",
        "Game (Side 1 of 2).d64",
    ] {
        assert_eq!(named_guidance("not-multi", &[name]), None, "{name}");
    }
}

#[test]
fn parts_are_never_called_discs() {
    let g = named_guidance(
        "parts",
        &["Game (Part 1 of 3).iso", "Game (Part 3 of 3).iso"],
    )
    .unwrap();
    assert_eq!(g.headline, "3-part game · order not verified");
    assert_eq!(g.missing_media, ["Part 2 of 3"]);
}

#[test]
fn the_headline_counts_media_not_sides() {
    let g = named_guidance(
        "sides",
        &[
            "Game (Disk 1 of 3 Side A).adf",
            "Game (Disk 1 of 3 Side B).adf",
            "Game (Disk 2 of 3 Side A).adf",
            "Game (Disk 2 of 3 Side B).adf",
        ],
    )
    .unwrap();
    assert_eq!(g.headline, "3-disk game · order not verified");
    assert_eq!(g.missing_media, ["Disk 3 of 3"]);
}

#[test]
fn unnumbered_media_are_not_reported_as_missing_numbers() {
    let root = temp_root("unnumbered");
    let (mut set, paths) = verified_set(&root, 2);
    set.members[1].ordinal = None;
    set.members[1].representations[0].ordinal = None;
    set.state = MediaSetState::IncompleteSet;
    let g = guidance(&set, &paths[0], None);
    assert!(g.missing_media.is_empty(), "{g:?}");
    assert!(!g.conflicts.iter().any(|c| c.contains("missing")), "{g:?}");
    assert_eq!(g.headline, "2-disc game · order not verified");
}

#[test]
fn a_bonus_disc_does_not_hide_missing_game_discs() {
    let g = named_guidance(
        "bonus",
        &["Game (Disc 1 of 3).iso", "Game (Bonus Disc).iso"],
    )
    .unwrap();
    assert_eq!(g.headline, "3-disc game · order not verified");
    assert_eq!(g.missing_media, ["Disc 2 of 3", "Disc 3 of 3"]);
}

#[test]
fn duplicate_ordinals_are_reported_not_ordered() {
    let root = temp_root("duplicate");
    let (mut set, paths) = verified_set(&root, 2);
    set.members[1].ordinal = set.members[0].ordinal.clone();
    set.members[1].representations[0].ordinal = set.members[0].ordinal.clone();
    let g = guidance(&set, &paths[0], None);
    assert!(!g.order_verified, "{g:?}");
    assert!(
        g.conflicts
            .iter()
            .any(|conflict| conflict.contains("claims to be Disc 1")),
        "{g:?}"
    );
}

#[test]
fn filename_only_stale_order_evidence_is_never_presented_as_verified() {
    let root = temp_root("stale-order-evidence");
    let (mut set, paths) = verified_set(&root, 2);
    for member in &mut set.members {
        for representation in &mut member.representations {
            for evidence in &mut representation.record.evidence {
                evidence.provenance.kind = EvidenceKind::Filename;
                evidence.provenance.source = "stale catalogue claim".into();
            }
        }
    }
    set.identity.verified = false;
    set.confidence = archivefs_core::media_set::MediaSetConfidence::Likely;
    set.state = MediaSetState::UnverifiedSet;

    let g = guidance(&set, &paths[0], None);
    assert!(!g.order_verified, "{g:?}");
    assert!(g.headline.contains("order not verified"));
    assert!(
        g.provenance
            .iter()
            .any(|source| source.contains("Filename: stale catalogue claim"))
    );
    assert!(g.discs.iter().all(|disc| !disc.start));
}

#[test]
fn four_disc_details_panel_fits_a_1024_by_600_viewport() {
    let root = temp_root("compact-panel");
    let (set, paths) = verified_set(&root, 4);
    let guidance = guidance(&set, &paths[0], None);
    let context = eframe::egui::Context::default();
    let mut height = 0.0;
    let output = context.run(
        eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                eframe::egui::Pos2::ZERO,
                eframe::egui::vec2(1024.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                show_guidance(ui, &guidance);
                height = ui.cursor().top() - ui.min_rect().top();
            });
        },
    );
    assert!(height < 500.0, "multi-disc details used {height} points");
    fn text_shapes(shape: &eframe::egui::epaint::Shape, out: &mut String) {
        match shape {
            eframe::egui::epaint::Shape::Text(text) => out.push_str(text.galley.text()),
            eframe::egui::epaint::Shape::Vec(shapes) => {
                shapes.iter().for_each(|shape| text_shapes(shape, out))
            }
            _ => {}
        }
    }
    let mut rendered = String::new();
    output
        .shapes
        .iter()
        .for_each(|clipped| text_shapes(&clipped.shape, &mut rendered));
    assert!(rendered.contains("4-disc game · order verified"));
    assert!(rendered.contains("Order evidence: TrustedDat: verified manifest"));
}

#[test]
fn region_conflicts_stay_separate_and_are_visible() {
    use archivefs_core::media_set::ConflictKind;
    let root = temp_root("region-conflict");
    let (set, _) = verified_set(&root, 2);
    let records = set
        .members
        .iter()
        .flat_map(|member| &member.representations)
        .map(|representation| {
            let mut record = representation.record.clone();
            for evidence in &mut record.evidence {
                evidence.variant.region = Some(
                    if representation.ordinal.as_ref().unwrap().number == 1 {
                        "usa"
                    } else {
                        "europe"
                    }
                    .into(),
                );
            }
            record
        })
        .collect();
    let report = resolve_index(index_media(records));
    assert_eq!(report.sets.len(), 2, "different regions remain separate");
    assert!(report.sets.iter().all(|set| set.members.len() == 1));
    for set in &report.sets {
        assert!(
            set.conflicts
                .iter()
                .any(|conflict| conflict.kind == ConflictKind::VariantConflict)
        );
        let path = &set.members[0].representations[0].record.source.path;
        let g = guidance(set, path, None);
        assert!(!g.order_verified, "{g:?}");
        assert!(
            g.conflicts
                .iter()
                .any(|conflict| conflict.contains("incompatible")),
            "{g:?}"
        );
    }
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
        std::slice::from_ref(&set),
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
    let shown = guidance(&set, &paths[1], Some(&decision));
    assert_eq!(
        shown.selected_launch_disc.as_deref(),
        Some("Disc 1 (playlist start)")
    );
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
    let decision = context.decide(&candidate, None, false);
    let shown = guidance(context.sets.first().unwrap(), &paths[0], Some(&decision));
    assert!(
        shown
            .launch
            .as_deref()
            .is_some_and(|note| note.contains("last library scan"))
    );
}

#[test]
fn catalogue_derived_sets_are_filename_evidence_and_never_get_a_playlist() {
    // Real libraries today: no saved topology, sets derived from the catalogue.
    let root = temp_root("catalogue");
    let (_, paths) = verified_set(&root, 2);
    let archives = catalogue_archives(&paths);
    let sets = sets_from(&[], &archives);
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
        g.conflicts
            .iter()
            .any(|conflict| conflict.contains("not how many discs")),
        "{:?}",
        g.conflicts
    );
}

#[test]
fn a_cue_bin_release_is_never_shown_a_retroarch_playlist() {
    let root = temp_root("retroarch-cue");
    let (set, paths) = verified_set(&root, 2);
    let context = context_for(set, &paths, root.join("lists"));
    let mut candidate = duckstation_candidate(&paths[0]);
    candidate.target = archivefs_core::launch::LaunchTarget::RetroArchCore {
        profile: archivefs_core::emulator_environment::retroarch::ProfileRef {
            profile_kind: archivefs_core::emulator_environment::retroarch::ProfileKind::Native,
            scope: archivefs_core::emulator_environment::retroarch::ProfileScope::User,
        },
        core_stem: "swanstation".into(),
        platform_id: "PSX",
    };
    candidate.content.container = Some(archivefs_core::launch::LaunchContainerKind::CueBin);
    let extensions = vec!["cue".to_string(), "m3u".to_string()];
    match context.decide(&candidate, Some(&extensions), false) {
        MultiDiscLaunch::SingleDisc { reason } => {
            assert!(reason.contains("DuckStation only"), "{reason}")
        }
        other => panic!("{other:?}"),
    }
}
