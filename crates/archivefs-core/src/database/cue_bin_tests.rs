//! Loose PlayStation CUE/BIN discs end to end, on synthetic files and a
//! disposable database: scan -> DAT audit -> saved identity -> freshness ->
//! disc order -> playlist hand-over. No real disc, DAT or library is touched.
use super::*;
use crate::dat::library_identity_summary::{DatProvenanceFreshness, LibraryItemHashes};
use crate::dat::limits::DatLimits;
use crate::dat::sources::DatSourceKind;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use crate::safe_read::TrustedRoots;
use sha1::{Digest, Sha1};
use std::sync::atomic::AtomicBool;

const SOURCE: &str = "redump-psx";
const SECTOR: usize = 2352;

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// One synthetic two-track disc: its sheet and track bytes.
struct Disc {
    entry: String,
    cue: String,
    tracks: Vec<(String, Vec<u8>)>,
}

fn disc(entry: &str, seed: u8) -> Disc {
    let tracks = vec![
        (format!("{entry} (Track 1).bin"), vec![seed; SECTOR * 20]),
        (
            format!("{entry} (Track 2).bin"),
            vec![seed.wrapping_add(100); SECTOR * 30],
        ),
    ];
    let cue = format!(
        "FILE \"{}\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\nFILE \"{}\" BINARY\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:00:10\n",
        tracks[0].0, tracks[1].0
    );
    Disc {
        entry: entry.into(),
        cue,
        tracks,
    }
}

fn dat_game(disc: &Disc) -> String {
    let mut roms = format!(
        r#"<rom name="{}.cue" size="{}" sha1="{}"/>"#,
        disc.entry,
        disc.cue.len(),
        sha1_hex(disc.cue.as_bytes())
    );
    for (name, bytes) in &disc.tracks {
        roms.push_str(&format!(
            r#"<rom name="{name}" size="{}" sha1="{}"/>"#,
            bytes.len(),
            sha1_hex(bytes)
        ));
    }
    format!(r#"<game name="{}">{roms}</game>"#, disc.entry)
}

struct Library {
    root: PathBuf,
    games: PathBuf,
    database: Database,
}

fn config(root: &Path) -> crate::Config {
    crate::Config {
        source_folders: vec![root.join("games")],
        mount_root: root.join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    }
}

/// Writes `discs` into `games/<folder>` (each disc's sheet named
/// `<local>.cue`) and a catalogue declaring `catalogue`.
fn library(folder: &str, discs: &[(&str, &Disc)], catalogue: &[&Disc]) -> Library {
    let root = tempfile::tempdir().unwrap().keep();
    let games = root.join("games").join(folder);
    fs::create_dir_all(&games).unwrap();
    for (local, disc) in discs {
        fs::write(games.join(format!("{local}.cue")), &disc.cue).unwrap();
        for (name, bytes) in &disc.tracks {
            fs::write(games.join(name), bytes).unwrap();
        }
    }
    let body: String = catalogue.iter().map(|disc| dat_game(disc)).collect();
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

impl Library {
    fn audit(&mut self) {
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
    }

    fn rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = self
            .database
            .load_archives()
            .unwrap()
            .into_iter()
            .map(|a| a.relative_path.to_string_lossy().into_owned())
            .collect();
        rows.sort();
        rows
    }

    fn cue(&self, local: &str) -> PathBuf {
        self.games.join(format!("{local}.cue"))
    }

    fn summary(
        &self,
        local: &str,
    ) -> crate::dat::library_identity_summary::LibraryDatIdentitySummary {
        let archive = self
            .database
            .load_archives()
            .unwrap()
            .into_iter()
            .find(|a| a.absolute_path == self.cue(local))
            .expect("the sheet is a library entry");
        self.database
            .library_dat_identity_summary_for_item(
                archive.id,
                SOURCE,
                Some(&LibraryItemHashes {
                    size_bytes: archive.size_bytes,
                    ..Default::default()
                }),
                None,
                true,
            )
            .unwrap()
            .expect("an identity was saved for the sheet")
    }

    fn verified_now(&self, local: &str) -> bool {
        let summary = self.summary(local);
        summary.is_verified() && summary.provenance_freshness == DatProvenanceFreshness::Current
    }
}

#[test]
fn a_playstation_cue_bin_disc_is_one_library_entry_and_other_platforms_are_unchanged() {
    let game = disc("Orbit (USA)", 1);
    let lib = library("psx", &[("Orbit (USA)", &game)], &[&game]);
    assert_eq!(lib.rows(), ["psx/Orbit (USA).cue"]);
    let entry = &lib.database.load_archives().unwrap()[0];
    assert_eq!(entry.platform.as_deref(), Some("PSX"));

    let saturn = library("saturn", &[("Orbit (USA)", &game)], &[&game]);
    assert!(saturn.rows().is_empty(), "{:?}", saturn.rows());

    // An incomplete PlayStation sheet is not an entry, and its tracks never
    // are. (A sheet that becomes incomplete after it was catalogued keeps its
    // row like any other existing file; its identity goes stale and launch
    // refuses it - see the tests below.)
    let mut incomplete = disc("Orbit (USA)", 1);
    incomplete.tracks[1].0 = "not written.bin".into();
    incomplete.cue = incomplete
        .cue
        .replace("Orbit (USA) (Track 2).bin", "absent.bin");
    let fresh_root = tempfile::tempdir().unwrap().keep();
    let fresh_games = fresh_root.join("games").join("psx");
    fs::create_dir_all(&fresh_games).unwrap();
    fs::write(fresh_games.join("Orbit (USA).cue"), &incomplete.cue).unwrap();
    fs::write(
        fresh_games.join(&incomplete.tracks[0].0),
        &incomplete.tracks[0].1,
    )
    .unwrap();
    let mut fresh = Database::open_or_create(fresh_root.join("library.sqlite3")).unwrap();
    crate::scan_and_persist(&mut fresh, &config(&fresh_root), "test").unwrap();
    assert!(fresh.load_archives().unwrap().is_empty());
    assert!(matches!(
        crate::ingestion::ps1_cue_disc::inspect_ps1_cue_disc(&fresh_games.join("Orbit (USA).cue")),
        Err(crate::ingestion::ps1_cue_disc::Ps1CueRefusal::MissingCompanion(_))
    ));
}

#[test]
fn a_whole_disc_matching_one_entry_is_verified_and_records_every_file() {
    let game = disc("Orbit (USA)", 1);
    let mut lib = library("psx", &[("my orbit", &game)], &[&game]);
    lib.audit();
    let summary = lib.summary("my orbit");
    assert!(summary.is_verified(), "{:?}", summary.verification_state);
    assert_eq!(
        summary.canonical.canonical_dat_name.as_deref(),
        Some("Orbit (USA)")
    );
    assert_eq!(
        summary.provenance_freshness,
        DatProvenanceFreshness::Current
    );
    let archive_id = lib.database.load_archives().unwrap()[0].id;
    let saved = lib
        .database
        .library_dat_identity_for_item(archive_id, SOURCE)
        .unwrap()
        .unwrap();
    assert_eq!(saved.disc_members.len(), 3, "sheet plus two tracks");
    assert_eq!(
        lib.database.library_dat_states().unwrap().get(&archive_id),
        Some(&LibraryDatState::Verified)
    );
}

#[test]
fn a_matching_sheet_with_a_wrong_track_or_other_release_is_not_verified() {
    let game = disc("Orbit (USA)", 1);
    let mut wrong_track = disc("Orbit (USA)", 1);
    wrong_track.tracks[1].1 = vec![9u8; SECTOR * 30];
    let mut lib = library("psx", &[("orbit", &wrong_track)], &[&game]);
    lib.audit();
    assert!(!lib.summary("orbit").is_verified());

    // The disc's bytes belong to another region's entry, which the catalogue
    // does not declare: nothing is verified.
    let europe = disc("Orbit (Europe)", 2);
    let mut lib = library("psx", &[("orbit", &europe)], &[&game]);
    lib.audit();
    assert!(!lib.summary("orbit").is_verified());
}

fn set_mtime(path: &Path, time: std::time::SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[test]
fn any_changed_replaced_or_missing_track_makes_the_saved_verification_stale() {
    let game = disc("Orbit (USA)", 1);
    let track = |lib: &Library, n: usize| lib.games.join(&game.tracks[n].0);

    // Unchanged: still verified, no re-audit needed.
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    assert!(lib.verified_now("orbit"));

    // The sheet unchanged, one track rewritten in place with equal size.
    let mut changed = game.tracks[1].1.clone();
    changed[0] ^= 0xff;
    let original_mtime = fs::metadata(track(&lib, 1)).unwrap().modified().unwrap();
    fs::write(track(&lib, 1), &changed).unwrap();
    set_mtime(&track(&lib, 1), original_mtime);
    assert!(!lib.verified_now("orbit"));
    let archive_id = lib.database.load_archives().unwrap()[0].id;
    assert_eq!(
        lib.database.library_dat_states().unwrap().get(&archive_id),
        None
    );
    assert!(lib.database.library_dat_exact_matches().unwrap().is_empty());

    // Replaced by a different file of equal size and the same timestamp.
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    let mtime = fs::metadata(track(&lib, 0)).unwrap().modified().unwrap();
    let replacement = lib.games.join("replacement.tmp");
    fs::write(&replacement, vec![7u8; SECTOR * 20]).unwrap();
    set_mtime(&replacement, mtime);
    fs::rename(&replacement, track(&lib, 0)).unwrap();
    assert!(!lib.verified_now("orbit"));

    // A track missing.
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    fs::remove_file(track(&lib, 1)).unwrap();
    assert!(!lib.verified_now("orbit"));
}

#[test]
fn verified_cue_bin_discs_reach_a_prepared_playlist_and_stale_tracks_stop_it() {
    use crate::launch::multidisc_launch::{
        MultiDiscLaunch, hand_over_playlist, plan_multidisc_launch,
    };
    use crate::launch::planning::LaunchTarget;
    use std::ffi::OsString;

    let one = disc("Orbit (USA) (Disc 1)", 1);
    let two = disc("Orbit (USA) (Disc 2)", 2);
    let mut lib = library("psx", &[("b", &one), ("a", &two)], &[&one, &two]);
    lib.audit();
    let order = lib.database.refresh_dat_disc_order(1).unwrap();
    assert_eq!((order.discs, order.written), (2, 1), "{order:?}");
    let sets = lib
        .database
        .load_media_topology_evidence(&lib.database.load_archives().unwrap())
        .unwrap();
    assert_eq!(sets[0].state, crate::media_set::MediaSetState::CompleteSet);

    let target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: "p".into(),
        profile_path: None,
    };
    let lists = lib.root.join("launch-playlists");
    let plan = || plan_multidisc_launch(&sets, &lib.cue("a"), &target, None, false, &lists);
    let MultiDiscLaunch::Playlist(playlist) = plan() else {
        panic!("{:?}", plan())
    };
    assert_eq!(playlist.discs, [lib.cue("b"), lib.cue("a")]);
    assert_eq!(playlist.companion_identities.len(), 4);
    let mut arguments = vec![
        OsString::from("-batch"),
        OsString::from("--"),
        lib.cue("a").into(),
    ];
    let staged = hand_over_playlist(&mut arguments, &lib.cue("a"), &sets, &playlist).unwrap();
    let listed: Vec<PathBuf> = fs::read_to_string(&staged)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(PathBuf::from)
        .collect();
    assert_eq!(listed, [lib.cue("b"), lib.cue("a")]);
    fs::remove_file(&staged).unwrap();

    // A track changed after Preview: Launch refuses and writes nothing.
    let MultiDiscLaunch::Playlist(playlist) = plan() else {
        panic!()
    };
    let track = lib.games.join(&one.tracks[1].0);
    let mut bytes = one.tracks[1].1.clone();
    bytes[0] ^= 1;
    fs::write(&track, &bytes).unwrap();
    let mut arguments = vec![
        OsString::from("-batch"),
        OsString::from("--"),
        lib.cue("a").into(),
    ];
    let error = hand_over_playlist(&mut arguments, &lib.cue("a"), &sets, &playlist).unwrap_err();
    assert!(
        error.contains("changed or disappeared after Preview"),
        "{error}"
    );
    assert!(!playlist.playlist_path.exists());

    // The stale disc no longer counts towards a saved order.
    let order = lib.database.refresh_dat_disc_order(2).unwrap();
    assert_eq!(order.discs, 1, "{order:?}");
    assert!(!lib.verified_now("b"));
}

/// What a remount looks like to EmuWiz: the same bytes and modification time
/// under a new inode. `bytes` replaces the content when given.
fn simulate_remount(path: &Path, bytes: Option<&[u8]>) {
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    let staged = path.with_extension("remount");
    match bytes {
        Some(bytes) => fs::write(&staged, bytes).unwrap(),
        None => {
            fs::copy(path, &staged).unwrap();
        }
    }
    set_mtime(&staged, modified);
    fs::rename(&staged, path).unwrap();
}

fn catalogue(lib: &Library) -> crate::CueDiscCatalogue {
    crate::CueDiscCatalogue {
        source_id: SOURCE.into(),
        display_name: "Redump PSX".into(),
        dat_path: lib.root.join("redump.dat"),
        kind: DatSourceKind::File,
    }
}

fn saved(lib: &Library) -> crate::dat::library_identity_summary::PersistedLibraryDatIdentity {
    let id = lib.database.load_archives().unwrap()[0].id;
    lib.database
        .library_dat_identity_for_item(id, SOURCE)
        .unwrap()
        .unwrap()
}

fn recheck(lib: &mut Library) -> crate::CueDiscRevalidation {
    let id = lib.database.load_archives().unwrap()[0].id;
    let catalogue = catalogue(lib);
    lib.database
        .revalidate_cue_disc(
            id,
            &catalogue,
            &TrustedRoots::none(),
            &AtomicBool::new(false),
        )
        .unwrap()
}

#[test]
fn a_remounted_disc_needs_a_recheck_and_identical_content_is_verified_again() {
    use crate::dat::library_identity_summary::DiscFileState;
    let game = disc("Orbit (USA)", 1);
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    assert!(lib.verified_now("orbit"));
    // Every file of the disc gets a new inode with identical bytes and times.
    simulate_remount(&lib.cue("orbit"), None);
    for (name, _) in &game.tracks {
        simulate_remount(&lib.games.join(name), None);
    }
    assert!(!lib.verified_now("orbit"));
    let state = saved(&lib).disc_files_state();
    assert!(
        matches!(state, DiscFileState::IdentityUncertain(_)),
        "{state:?}"
    );
    assert!(state.reason().unwrap().contains("remounted"));
    // Uncertain content keeps the disc out of disc order and playlists.
    assert_eq!(lib.database.refresh_dat_disc_order(1).unwrap().discs, 0);

    assert_eq!(
        recheck(&mut lib),
        crate::CueDiscRevalidation::Verified {
            entry: "Orbit (USA)".into()
        }
    );
    assert!(lib.verified_now("orbit"));
}

#[test]
fn a_remount_hiding_different_content_is_not_verified_and_keeps_the_saved_evidence() {
    use crate::dat::library_identity_summary::DiscFileState;
    let game = disc("Orbit (USA)", 1);
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    let before = saved(&lib);
    // Same size and times, new inode - but different bytes.
    let track = lib.games.join(&game.tracks[1].0);
    simulate_remount(&track, Some(&vec![9u8; game.tracks[1].1.len()]));
    assert!(matches!(
        saved(&lib).disc_files_state(),
        DiscFileState::IdentityUncertain(_)
    ));
    assert!(matches!(
        recheck(&mut lib),
        crate::CueDiscRevalidation::NotVerified { .. }
    ));
    assert_eq!(saved(&lib), before, "nothing is overwritten");
    assert!(!lib.verified_now("orbit"));
}

#[test]
fn changed_or_missing_tracks_and_an_impossible_recheck_are_refused() {
    use crate::dat::library_identity_summary::DiscFileState;
    let game = disc("Orbit (USA)", 1);

    // Changed in place, sheet untouched: changed, and a re-check refuses.
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    let track = lib.games.join(&game.tracks[1].0);
    let mut bytes = game.tracks[1].1.clone();
    bytes[0] ^= 1;
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&track, &bytes).unwrap();
    assert!(matches!(
        saved(&lib).disc_files_state(),
        DiscFileState::Changed(_)
    ));
    assert!(matches!(
        recheck(&mut lib),
        crate::CueDiscRevalidation::NotVerified { .. }
    ));

    // Missing: the disc is no longer a complete release, so nothing runs.
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    fs::remove_file(lib.games.join(&game.tracks[0].0)).unwrap();
    assert!(matches!(
        saved(&lib).disc_files_state(),
        DiscFileState::Missing(_)
    ));
    assert!(matches!(
        recheck(&mut lib),
        crate::CueDiscRevalidation::Refused { .. }
    ));

    // The catalogue cannot be read, or is now another version: refused, and
    // the saved evidence is left as it was.
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    simulate_remount(&lib.cue("orbit"), None);
    let before = saved(&lib);
    fs::rename(lib.root.join("redump.dat"), lib.root.join("moved.dat")).unwrap();
    assert!(matches!(
        recheck(&mut lib),
        crate::CueDiscRevalidation::Refused { .. }
    ));
    let newer = fs::read_to_string(lib.root.join("moved.dat"))
        .unwrap()
        .replace(
            "<version>2026-10-01</version>",
            "<version>2026-11-01</version>",
        );
    fs::write(lib.root.join("redump.dat"), newer).unwrap();
    assert!(matches!(
        recheck(&mut lib),
        crate::CueDiscRevalidation::Refused { .. }
    ));
    assert_eq!(saved(&lib), before);
}

#[test]
fn an_archive_whose_representative_member_is_a_cue_keeps_its_own_freshness() {
    use crate::dat::library_identity_summary::{ArchiveMemberProvenance, DiscFileState};
    let game = disc("Orbit (USA)", 1);
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    let mut identity = saved(&lib);
    identity.disc_members.clear();
    // A loose sheet match without recorded disc files is never current...
    assert_eq!(identity.disc_files_state(), DiscFileState::NotRecorded);
    // ...but a zip/7z/rar identity represented by its `.cue` member is not a
    // loose sheet, and keeps the archive's own freshness rules.
    identity.archive_member = Some(ArchiveMemberProvenance {
        member_name: "Orbit (USA).cue".into(),
        member_index: 0,
        archive_format: "zip".into(),
        algorithm: "SHA-1".into(),
    });
    assert_eq!(identity.disc_files_state(), DiscFileState::Unchanged);
    assert!(identity.disc_members_unchanged());
}

#[test]
fn managed_audit_persists_the_catalogue_inventory_without_local_validation() {
    let first = disc("Orbit (USA) (Disc 1)", 1);
    let second = disc("Orbit (USA) (Disc 2)", 2);
    let mut lib = library(
        "psx",
        &[("second", &second), ("first", &first)],
        &[&first, &second],
    );
    let outcome = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: "redump-games:psx".into(),
            source_display_name: "Managed Redump PSX".into(),
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
        .persist_library_dat_identities_from_audit(&outcome)
        .unwrap();
    assert_eq!(
        lib.database
            .expected_dat_entry_count("redump-games:psx")
            .unwrap(),
        2
    );
    let order = lib.database.refresh_dat_disc_order(1).unwrap();
    assert_eq!(order.discs, 2, "{order:?}");
    assert_eq!(order.written, 1, "{order:?}");
}

#[test]
fn catalogue_inventory_and_identities_rollback_together_on_a_write_failure() {
    let game = disc("Orbit (USA)", 1);
    let mut lib = library("psx", &[("orbit", &game)], &[&game]);
    lib.audit();
    let original = lib.summary("orbit").source.catalogue_sha256;
    let path = lib.root.join("redump.dat");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("</datafile>", "<!-- replacement --></datafile>");
    fs::write(&path, text).unwrap();
    let outcome = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: SOURCE.into(),
            source_display_name: "Redump".into(),
            dat_path: path,
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
    assert_ne!(outcome.catalogue_sha256, original);
    lib.database.connection.execute_batch("CREATE TRIGGER deny_identity BEFORE UPDATE ON library_dat_identities BEGIN SELECT RAISE(ABORT, 'synthetic persistence failure'); END;").unwrap();
    assert!(
        lib.database
            .persist_library_dat_identities_from_audit(&outcome)
            .is_err()
    );
    assert_eq!(
        lib.database.expected_dat_inventory_sha256(SOURCE).unwrap(),
        original
    );
    assert_eq!(lib.summary("orbit").source.catalogue_sha256, original);
}

#[test]
fn an_audit_with_no_library_associations_still_commits_its_exact_inventory() {
    let game = disc("Orbit (USA)", 1);
    let mut lib = library("psx", &[], &[&game]);
    // Put the disc on disk after the library scan: the audit has real input,
    // but no item can be associated with a saved archive row.
    fs::write(lib.cue("orbit"), &game.cue).unwrap();
    for (name, bytes) in &game.tracks {
        fs::write(lib.games.join(name), bytes).unwrap();
    }
    lib.audit();
    assert_eq!(lib.database.expected_dat_entry_count(SOURCE).unwrap(), 1);
    assert!(
        lib.database
            .expected_dat_inventory_sha256(SOURCE)
            .unwrap()
            .is_some()
    );
    assert!(lib.database.load_archives().unwrap().is_empty());
}
