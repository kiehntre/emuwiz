//! Real path, synthetic data: temp disc files -> `scan_and_persist` ->
//! `run_dat_audit` -> saved identities and catalogue inventory ->
//! `refresh_dat_disc_order` -> reload. Nothing touches a real library.
//! Discs are `.iso` because the scanner catalogues ISO images but not CUE
//! sheets; the producer only sees catalogued library items.
use super::*;
use crate::dat::limits::DatLimits;
use crate::dat::sources::DatSourceKind;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use crate::media_set::dat_disc_order::{
    DAT_DISC_ORDER_PRODUCER, DatDiscCandidate, dat_disc_record, dat_disc_token,
};
use crate::media_set::*;
use crate::safe_read::TrustedRoots;
use sha1::{Digest, Sha1};
use std::sync::atomic::AtomicBool;

const SOURCE: &str = "redump-psx";

struct Library {
    root: PathBuf,
    games: PathBuf,
    database: Database,
}

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `catalogue`: every `(entry name, disc bytes)` the DAT declares.
/// `library`: `(local file name, entry it really is)`.
fn library(catalogue: &[(&str, &str)], library: &[(&str, &str)]) -> Library {
    library_in("psx", catalogue, library)
}

fn library_in(folder: &str, catalogue: &[(&str, &str)], library: &[(&str, &str)]) -> Library {
    let root = tempfile::tempdir().unwrap().keep();
    let games = root.join("games").join(folder);
    fs::create_dir_all(&games).unwrap();
    for (file, entry) in library {
        fs::write(games.join(file), format!("disc bytes for {entry}")).unwrap();
    }
    let body: String = catalogue
        .iter()
        .map(|(entry, bytes)| {
            format!(
                r#"<game name="{entry}"><rom name="{entry}.bin" size="{}" sha1="{}"/></game>"#,
                bytes.len(),
                sha1_hex(bytes.as_bytes())
            )
        })
        .collect();
    fs::write(
        root.join("redump.dat"),
        format!(
            r#"<?xml version="1.0"?><datafile><header><name>Sony - PlayStation</name><version>2026-10-01</version><author>redump.org</author></header>{body}</datafile>"#
        ),
    )
    .unwrap();
    let mut database = Database::open_or_create(root.join("library.sqlite3")).unwrap();
    crate::scan_and_persist(&mut database, &config(&root), "test").unwrap();
    Library {
        root,
        games,
        database,
    }
}

fn config(root: &Path) -> crate::Config {
    crate::Config {
        source_folders: vec![root.join("games")],
        mount_root: root.join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    }
}

impl Library {
    fn rescan(&mut self) {
        crate::scan_and_persist(&mut self.database, &config(&self.root), "test").unwrap();
    }

    fn audit_and_refresh(&mut self) -> DatDiscOrderOutcome {
        let outcome = run_dat_audit_with_cache(
            &DatAuditRequest {
                source_id: SOURCE.into(),
                source_display_name: "Redump PSX".into(),
                dat_path: self.root.join("redump.dat"),
                dat_kind: DatSourceKind::File,
                scan_root: self.root.join("games"),
                limits: DatLimits::default(),
                policy: None,
                platform: None,
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
            AuditCacheConfig::Disabled,
        )
        .unwrap();
        self.database
            .persist_library_dat_identities_from_audit(&outcome)
            .unwrap();
        self.database.refresh_dat_disc_order(1).unwrap()
    }

    fn validate_catalogue(&mut self) {
        let parsed = crate::dat::parsers::parse_dat_file(
            &self.root.join("redump.dat"),
            DatLimits::default(),
        )
        .unwrap();
        let expected =
            crate::dat::expected_inventory::project_expected_dat_inventory(&parsed.dat.games);
        self.database
            .replace_expected_dat_inventory(
                SOURCE,
                parsed.dat.source.version.as_deref(),
                Some(parsed.dat.source.ecosystem),
                &expected.entries,
                expected.duplicate_names_skipped as u64,
            )
            .unwrap();
    }

    fn sets(&self) -> Vec<MediaSet> {
        self.database
            .load_media_topology_evidence(&self.database.load_archives().unwrap())
            .unwrap()
    }

    fn order(set: &MediaSet) -> Vec<String> {
        set.members
            .iter()
            .map(|m| {
                m.representations[0]
                    .record
                    .source
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

const FF: [(&str, &str); 3] = [
    (
        "Orbit Quest (USA) (Disc 1)",
        "disc bytes for Orbit Quest (USA) (Disc 1)",
    ),
    (
        "Orbit Quest (USA) (Disc 2)",
        "disc bytes for Orbit Quest (USA) (Disc 2)",
    ),
    (
        "Orbit Quest (USA) (Disc 3)",
        "disc bytes for Orbit Quest (USA) (Disc 3)",
    ),
];

#[test]
fn verified_discs_save_catalogue_order_despite_misleading_local_names() {
    let mut lib = library(
        &FF,
        &[
            ("orbit (Disc 3).iso", "Orbit Quest (USA) (Disc 1)"),
            ("orbit (Disc 1).iso", "Orbit Quest (USA) (Disc 2)"),
            ("aaa.iso", "Orbit Quest (USA) (Disc 3)"),
        ],
    );
    let outcome = lib.audit_and_refresh();
    assert_eq!(
        (
            outcome.discs,
            outcome.unknown_total,
            outcome.refused,
            outcome.written
        ),
        (3, 0, 0, 1),
        "{outcome:?}"
    );
    let sets = lib.sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(
        sets[0].state,
        MediaSetState::CompleteSet,
        "{:?}",
        sets[0].conflicts
    );
    assert_eq!(
        Library::order(&sets[0]),
        ["orbit (Disc 3).iso", "orbit (Disc 1).iso", "aaa.iso"]
    );
    let note = &sets[0].members[0].representations[0].record.evidence[0].notes;
    assert!(
        note.iter().any(|n| n.contains("matched exactly by SHA-1")),
        "{note:?}"
    );

    // Running it again changes nothing and reports no conflict.
    let again = lib.audit_and_refresh();
    assert_eq!((again.written, again.conflicts_kept), (1, 0));
    assert_eq!(Library::order(&lib.sets()[0]), Library::order(&sets[0]));

    // Saved order is bound to the catalogue: once a rescan records a changed
    // disc, the saved order no longer loads.
    fs::write(lib.games.join("aaa.iso"), "replaced").unwrap();
    lib.rescan();
    assert!(lib.sets().is_empty());
}

#[test]
fn missing_disc_is_saved_incomplete_and_a_catalogue_gap_leaves_the_total_unknown() {
    let mut lib = library(
        &FF,
        &[
            ("one.iso", "Orbit Quest (USA) (Disc 1)"),
            ("three.iso", "Orbit Quest (USA) (Disc 3)"),
        ],
    );
    lib.audit_and_refresh();
    let sets = lib.sets();
    assert_eq!(sets[0].state, MediaSetState::IncompleteSet);

    let mut gap = library(
        &[FF[0], FF[2]],
        &[
            ("one.iso", "Orbit Quest (USA) (Disc 1)"),
            ("three.iso", "Orbit Quest (USA) (Disc 3)"),
        ],
    );
    let outcome = gap.audit_and_refresh();
    assert_eq!(outcome.unknown_total, 2);
    let sets = gap.sets();
    assert_eq!(sets[0].state, MediaSetState::UnverifiedSet);
    assert!(
        sets[0]
            .conflicts
            .iter()
            .any(|c| c.kind == ConflictKind::UnknownCount)
    );
}

#[test]
fn unmatched_files_and_a_missing_catalogue_inventory_save_nothing() {
    let mut lib = library(
        &FF,
        &[
            ("Orbit Quest (USA) (Disc 1).iso", "not in the catalogue 1"),
            ("Orbit Quest (USA) (Disc 2).iso", "not in the catalogue 2"),
        ],
    );
    let outcome = lib.audit_and_refresh();
    assert_eq!((outcome.discs, outcome.written), (0, 0));

    // Verified discs whose catalogue inventory was never saved are refused.
    let mut lib = library(
        &FF,
        &[
            ("1.iso", "Orbit Quest (USA) (Disc 1)"),
            ("2.iso", "Orbit Quest (USA) (Disc 2)"),
            ("3.iso", "Orbit Quest (USA) (Disc 3)"),
        ],
    );
    lib.database
        .replace_expected_dat_inventory(SOURCE, Some("2026-09-01"), None, &[], 0)
        .unwrap();
    let audit = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: SOURCE.into(),
            source_display_name: "Redump PSX".into(),
            dat_path: lib.root.join("redump.dat"),
            dat_kind: DatSourceKind::File,
            scan_root: lib.root.join("games"),
            limits: DatLimits::default(),
            policy: None,
            platform: None,
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    lib.database
        .persist_library_dat_identities_from_audit(&audit)
        .unwrap();
    lib.database
        .replace_expected_dat_inventory(SOURCE, Some("2026-09-01"), None, &[], 0)
        .unwrap();
    let outcome = lib.database.refresh_dat_disc_order(1).unwrap();
    assert_eq!((outcome.discs, outcome.refused, outcome.written), (0, 3, 0));
    assert_eq!(
        outcome.validation_stale,
        BTreeSet::from([SOURCE.to_string()])
    );
    assert!(outcome.validation_missing.is_empty());
}

fn claim(path: &Path, entry: &str) -> MediaRecord {
    let candidate = DatDiscCandidate {
        path: path.to_path_buf(),
        platform: "psx".into(),
        format: "cue".into(),
        source_id: SOURCE.into(),
        source_revision: "2026-10-01".into(),
        catalogue_sha256: None,
        canonical_name: entry.into(),
        matched_algorithm: "SHA-1".into(),
        matched_value: None,
    };
    dat_disc_record(
        &candidate,
        MediaFamily::Optical,
        &dat_disc_token(entry).unwrap(),
        Some(2),
    )
}

#[test]
fn conflicting_new_order_never_replaces_current_saved_order() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.cue"), dir.path().join("b.cue"));
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let mut database = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
    let report = |records| TopologyReport {
        sets: resolve_index(index_media(records)).sets,
        stats: TopologyStats::default(),
    };
    let save = |database: &mut Database, records| {
        database
            .persist_media_topology_evidence_checked(
                &report(records),
                DAT_DISC_ORDER_PRODUCER,
                MEDIA_TOPOLOGY_PRODUCER_VERSION,
                1,
            )
            .unwrap()
    };
    let first = save(
        &mut database,
        vec![
            claim(&a, "Orbit (USA) (Disc 1)"),
            claim(&b, "Orbit (USA) (Disc 2)"),
        ],
    );
    assert_eq!(first.written, 1);
    let swapped = vec![
        claim(&a, "Orbit (USA) (Disc 2)"),
        claim(&b, "Orbit (USA) (Disc 1)"),
    ];
    let kept = save(&mut database, swapped.clone());
    assert_eq!((kept.written, kept.conflicts_kept), (0, 1));
    let saved_json: Vec<u8> = database
        .connection
        .query_row(
            "SELECT evidence_json FROM media_topology_evidence",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let saved: MediaSet = serde_json::from_slice(&saved_json).unwrap();
    assert_eq!(saved.members[0].representations[0].record.source.path, a);

    // Once the saved discs themselves changed, the saved claim is stale and
    // fresh evidence may replace it.
    fs::write(&a, "a changed").unwrap();
    let replaced = save(&mut database, swapped);
    assert_eq!((replaced.written, replaced.conflicts_kept), (1, 0));
}

#[test]
fn floppy_disks_and_tapes_take_their_order_from_the_catalogue_too() {
    let disks = [
        (
            "Orbit Quest (1989)(Acme)(Disk 1 of 2)",
            "disc bytes for Orbit Quest (1989)(Acme)(Disk 1 of 2)",
        ),
        (
            "Orbit Quest (1989)(Acme)(Disk 2 of 2)",
            "disc bytes for Orbit Quest (1989)(Acme)(Disk 2 of 2)",
        ),
    ];
    let mut amiga = library_in(
        "amiga",
        &disks,
        &[
            ("orbit-a.adf", "Orbit Quest (1989)(Acme)(Disk 2 of 2)"),
            ("orbit-b.adf", "Orbit Quest (1989)(Acme)(Disk 1 of 2)"),
        ],
    );
    let outcome = amiga.audit_and_refresh();
    assert_eq!((outcome.discs, outcome.written), (2, 1), "{outcome:?}");
    let sets = amiga.sets();
    assert_eq!(
        sets[0].state,
        MediaSetState::CompleteSet,
        "{:?}",
        sets[0].conflicts
    );
    assert_eq!(sets[0].family, Some(MediaFamily::Floppy));
    assert_eq!(Library::order(&sets[0]), ["orbit-b.adf", "orbit-a.adf"]);

    let tapes = [
        (
            "Orbit Quest (Europe) (Tape 1)",
            "disc bytes for Orbit Quest (Europe) (Tape 1)",
        ),
        (
            "Orbit Quest (Europe) (Tape 2)",
            "disc bytes for Orbit Quest (Europe) (Tape 2)",
        ),
    ];
    let mut spectrum = library_in(
        "zxspectrum",
        &tapes,
        &[
            ("side 1.tzx", "Orbit Quest (Europe) (Tape 2)"),
            ("side 2.tzx", "Orbit Quest (Europe) (Tape 1)"),
        ],
    );
    spectrum.audit_and_refresh();
    let sets = spectrum.sets();
    assert_eq!(
        sets[0].state,
        MediaSetState::CompleteSet,
        "{:?}",
        sets[0].conflicts
    );
    assert_eq!(sets[0].family, Some(MediaFamily::Tape));
    assert_eq!(Library::order(&sets[0]), ["side 2.tzx", "side 1.tzx"]);

    // A disc number on a floppy image is not evidence for it.
    let mut wrong = library_in(
        "amiga",
        &FF,
        &[
            ("1.adf", "Orbit Quest (USA) (Disc 1)"),
            ("2.adf", "Orbit Quest (USA) (Disc 2)"),
        ],
    );
    let outcome = wrong.audit_and_refresh();
    assert_eq!((outcome.discs, outcome.written), (0, 0));
}

/// DAT evidence -> saved order -> Preview plan -> each launcher's own
/// argument shape receives the staged playlist; a disc changed after Preview
/// refuses without writing one.
#[test]
fn dat_order_reaches_a_prepared_playlist_for_duckstation_and_retroarch() {
    use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
    use crate::launch::multidisc_launch::{
        MultiDiscLaunch, hand_over_playlist, plan_multidisc_launch,
    };
    use crate::launch::planning::LaunchTarget;
    use std::ffi::OsString;

    let catalogue = [
        FF[0],
        FF[1],
        FF[2],
        ("Solo Game (USA)", "disc bytes for Solo Game (USA)"),
        (
            "Gap Game (USA) (Disc 1)",
            "disc bytes for Gap Game (USA) (Disc 1)",
        ),
        (
            "Gap Game (USA) (Disc 3)",
            "disc bytes for Gap Game (USA) (Disc 3)",
        ),
    ];
    let mut lib = library(
        &catalogue,
        &[
            ("b.iso", "Orbit Quest (USA) (Disc 1)"),
            ("c.iso", "Orbit Quest (USA) (Disc 2)"),
            ("a.iso", "Orbit Quest (USA) (Disc 3)"),
            ("solo.iso", "Solo Game (USA)"),
            ("gap1.iso", "Gap Game (USA) (Disc 1)"),
            ("gap3.iso", "Gap Game (USA) (Disc 3)"),
        ],
    );
    lib.audit_and_refresh();
    let sets = lib.sets();
    let lists = lib.root.join("launch-playlists");
    let disc = |name: &str| lib.games.join(name);
    let duckstation = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "p".into(),
        profile_path: None,
    };
    let retroarch = |core: &str| LaunchTarget::RetroArchCore {
        profile: ProfileRef {
            profile_kind: ProfileKind::Native,
            scope: ProfileScope::User,
        },
        core_stem: core.into(),
        platform_id: "PSX",
    };
    let ext = |list: &[&str]| list.iter().map(|e| e.to_string()).collect::<Vec<_>>();
    let plan = |target: &LaunchTarget, selected: &str, extensions: Option<&[String]>| {
        plan_multidisc_launch(&sets, &disc(selected), target, extensions, false, &lists)
    };

    let expected = vec![disc("b.iso"), disc("c.iso"), disc("a.iso")];
    let with_iso = ext(&["cue", "iso", "chd", "m3u"]);
    for (target, extensions, argv) in [
        (
            duckstation.clone(),
            None,
            vec![OsString::from("-batch"), OsString::from("--")],
        ),
        (
            retroarch("swanstation"),
            Some(with_iso.as_slice()),
            vec![
                OsString::from("-L"),
                OsString::from("/cores/swanstation_libretro.so"),
            ],
        ),
    ] {
        let MultiDiscLaunch::Playlist(playlist) = plan(&target, "c.iso", extensions) else {
            panic!("{target:?}: {:?}", plan(&target, "c.iso", extensions));
        };
        assert_eq!(
            playlist.discs, expected,
            "catalogue order, whichever disc was chosen"
        );
        let mut arguments = argv.clone();
        arguments.push(disc("c.iso").into_os_string());
        let staged = hand_over_playlist(&mut arguments, &disc("c.iso"), &sets, &playlist).unwrap();
        assert_eq!(arguments.last().unwrap(), staged.as_os_str());
        assert_eq!(arguments[..argv.len()], argv[..]);
        let listed = fs::read_to_string(&staged)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        assert_eq!(listed, expected);
        fs::remove_file(&staged).unwrap();
    }

    // Unsupported pairings stay single-disc and say why.
    let no_m3u = ext(&["cue", "iso"]);
    let no_iso = ext(&["cue", "chd", "m3u"]);
    for (target, extensions, why) in [
        (
            retroarch("pcsx_rearmed"),
            Some(no_m3u.as_slice()),
            "does not list m3u",
        ),
        (
            retroarch("mednafen_psx"),
            Some(no_iso.as_slice()),
            "does not list the format",
        ),
        (
            LaunchTarget::Standalone {
                adapter_id: "pcsx2",
                profile_id: "p".into(),
                profile_path: None,
            },
            None,
            "",
        ),
    ] {
        match plan(&target, "b.iso", extensions) {
            MultiDiscLaunch::SingleDisc { reason } => assert!(reason.contains(why), "{reason}"),
            other => panic!("{target:?}: {other:?}"),
        }
    }
    // A single-disc game is untouched; an unknown total is explained.
    assert_eq!(
        plan(&duckstation, "solo.iso", None),
        MultiDiscLaunch::NotMultiDisc
    );
    match plan(&duckstation, "gap1.iso", None) {
        MultiDiscLaunch::SingleDisc { reason } => {
            assert!(reason.contains("not how many discs"), "{reason}")
        }
        other => panic!("{other:?}"),
    }

    // Changed after Preview: Launch refuses and writes no playlist.
    let MultiDiscLaunch::Playlist(playlist) = plan(&duckstation, "b.iso", None) else {
        panic!()
    };
    fs::write(disc("a.iso"), "swapped for something else").unwrap();
    let mut arguments = vec![
        OsString::from("-batch"),
        OsString::from("--"),
        disc("b.iso").into(),
    ];
    let error = hand_over_playlist(&mut arguments, &disc("b.iso"), &sets, &playlist).unwrap_err();
    assert!(
        error.contains("changed or disappeared after Preview"),
        "{error}"
    );
    assert!(!playlist.playlist_path.exists());
    assert_eq!(arguments[2], disc("b.iso").into_os_string());
}

#[test]
fn a_second_source_naming_another_entry_contests_the_disc() {
    let mut lib = library(
        &FF,
        &[
            ("1.iso", "Orbit Quest (USA) (Disc 1)"),
            ("2.iso", "Orbit Quest (USA) (Disc 2)"),
            ("3.iso", "Orbit Quest (USA) (Disc 3)"),
        ],
    );
    assert_eq!(lib.audit_and_refresh().written, 1);
    // Another catalogue verifies disc 1's exact bytes as a different entry.
    let bytes = "disc bytes for Orbit Quest (USA) (Disc 1)";
    fs::write(
        lib.root.join("other.dat"),
        format!(
            r#"<?xml version="1.0"?><datafile><header><name>Other</name><version>7</version></header><game name="Orbit Quest (Europe) (Disc 1)"><rom name="x.bin" size="{}" sha1="{}"/></game></datafile>"#,
            bytes.len(),
            sha1_hex(bytes.as_bytes())
        ),
    )
    .unwrap();
    let other = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: "other".into(),
            source_display_name: "Other".into(),
            dat_path: lib.root.join("other.dat"),
            dat_kind: DatSourceKind::File,
            scan_root: lib.root.join("games"),
            limits: DatLimits::default(),
            policy: None,
            platform: None,
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    lib.database
        .persist_library_dat_identities_from_audit(&other)
        .unwrap();
    let outcome = lib.database.refresh_dat_disc_order(2).unwrap();
    assert_eq!((outcome.discs, outcome.refused), (2, 1), "{outcome:?}");
    let sets = lib.sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].state, MediaSetState::IncompleteSet);
    assert_eq!(Library::order(&sets[0]), ["2.iso", "3.iso"]);
}

#[test]
fn audit_without_validation_reports_the_missing_step_then_validate_saves_the_order() {
    let mut lib = library(
        &FF,
        &[
            ("1.iso", "Orbit Quest (USA) (Disc 1)"),
            ("2.iso", "Orbit Quest (USA) (Disc 2)"),
            ("3.iso", "Orbit Quest (USA) (Disc 3)"),
        ],
    );
    let mut audit = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: SOURCE.into(),
            source_display_name: "Redump PSX".into(),
            dat_path: lib.root.join("redump.dat"),
            dat_kind: DatSourceKind::File,
            scan_root: lib.root.join("games"),
            limits: DatLimits::default(),
            policy: None,
            platform: None,
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    audit.catalogue_inventory = None;
    audit.catalogue_sha256 = None;
    lib.database
        .persist_library_dat_identities_from_audit(&audit)
        .unwrap();
    let before = lib.database.refresh_dat_disc_order(1).unwrap();
    assert_eq!((before.discs, before.written), (0, 0));
    assert_eq!(
        before.validation_missing,
        BTreeSet::from([SOURCE.to_string()])
    );
    assert!(lib.sets().is_empty());

    lib.validate_catalogue();
    let after = lib.database.refresh_dat_disc_order(2).unwrap();
    assert_eq!((after.discs, after.written), (3, 1), "{after:?}");
    assert!(after.validation_missing.is_empty() && after.validation_stale.is_empty());
    assert_eq!(lib.sets()[0].state, MediaSetState::CompleteSet);
}

#[test]
fn a_filename_only_result_from_another_source_never_contests_the_order() {
    let mut lib = library(
        &FF,
        &[
            ("1.iso", "Orbit Quest (USA) (Disc 1)"),
            ("2.iso", "Orbit Quest (USA) (Disc 2)"),
            ("3.iso", "Orbit Quest (USA) (Disc 3)"),
        ],
    );
    assert_eq!(lib.audit_and_refresh().written, 1);
    // Another catalogue's filename-only match names a different entry for
    // disc 1. It carries no hash, so it must neither help nor block.
    let disc_one = lib
        .database
        .find_archive_id_by_absolute_path(&lib.games.join("1.iso"))
        .unwrap()
        .unwrap();
    let mut filename_only = lib
        .database
        .library_dat_identity_for_item(disc_one, SOURCE)
        .unwrap()
        .unwrap();
    filename_only.verification_state =
        crate::dat::library_identity_summary::DatVerificationState::FilenameOnlyNotVerified;
    filename_only.source.source_id = "other".into();
    filename_only.canonical.canonical_dat_name = Some("Something Else (Europe) (Disc 2)".into());
    lib.database
        .persist_library_dat_identity(disc_one, &filename_only)
        .unwrap();
    let outcome = lib.database.refresh_dat_disc_order(2).unwrap();
    assert_eq!((outcome.discs, outcome.refused), (3, 0), "{outcome:?}");
    let sets = lib.sets();
    assert_eq!(sets[0].state, MediaSetState::CompleteSet);
    assert_eq!(Library::order(&sets[0]), ["1.iso", "2.iso", "3.iso"]);
}
