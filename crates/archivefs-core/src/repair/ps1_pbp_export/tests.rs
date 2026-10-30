use super::*;
use crate::dat::limits::DatLimits;
use crate::dat::sources::DatSourceKind;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use sha1::Sha1;
use std::process::Command;

const SOURCE: &str = "synthetic-redump-psx";

struct Fixture {
    root: tempfile::TempDir,
    db: Database,
    set: MediaSet,
    cues: Vec<PathBuf>,
    sources: Vec<DatSourceEntry>,
}

fn writer() -> PathBuf {
    let path = std::env::var_os("EMUWIZ_PSXPACKAGER_V163")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                "/tmp/emuwiz-psxpackager-v1.6.3/extracted/psxpackager-linux-x64/psxpackager",
            )
        });
    assert!(
        path.is_file(),
        "install the pinned writer and set EMUWIZ_PSXPACKAGER_V163"
    );
    path
}

fn fixture(count: usize) -> Fixture {
    assert!((2..=6).contains(&count));
    assert!(
        Command::new("genisoimage")
            .arg("--version")
            .output()
            .is_ok()
    );
    let root = tempfile::tempdir().unwrap();
    let games = root.path().join("games/psx");
    fs::create_dir_all(&games).unwrap();
    let mut dat_games = String::new();
    let mut cues = Vec::new();
    for ordinal in 1..=count {
        let name = format!("Synthetic Quest (USA) (Disc {ordinal} of {count})");
        let folder = root.path().join(format!("master{ordinal}"));
        fs::create_dir(&folder).unwrap();
        let serial = format!("SLUS_{:03}.{:02}", ordinal, ordinal);
        fs::write(
            folder.join("SYSTEM.CNF"),
            format!("BOOT=cdrom:\\{serial};1\r\n"),
        )
        .unwrap();
        let mut exe = vec![0u8; 2048];
        exe[..8].copy_from_slice(b"PS-X EXE");
        fs::write(folder.join(&serial), exe).unwrap();
        let iso = root.path().join(format!("disc{ordinal}.iso"));
        assert!(
            Command::new("genisoimage")
                .args(["-quiet", "-iso-level", "1", "-o"])
                .arg(&iso)
                .arg(&folder)
                .status()
                .unwrap()
                .success()
        );
        let cooked = fs::read(&iso).unwrap();
        assert_eq!(cooked.len() % 2048, 0);
        let mut raw = Vec::with_capacity(cooked.len() / 2048 * 2352);
        for sector in cooked.chunks_exact(2048) {
            let mut frame = [0u8; 2352];
            frame[..12].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0]);
            frame[15] = 2;
            frame[24..2072].copy_from_slice(sector);
            raw.extend_from_slice(&frame);
        }
        let bin_name = format!("disc{ordinal}.bin");
        let cue_name = format!("disc{ordinal}.cue");
        let cue_text =
            format!("FILE \"{bin_name}\" BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00\n");
        let cue = games.join(&cue_name);
        fs::write(&cue, &cue_text).unwrap();
        fs::write(games.join(&bin_name), &raw).unwrap();
        assert_eq!(
            inspect_game_identity(&cue, Some("PSX")).verified_ps1_serial(),
            Some(format!("SLUS-{:03}{:02}", ordinal, ordinal).as_str())
        );
        let cue_sha1: String = Sha1::digest(cue_text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let bin_sha1: String = Sha1::digest(&raw)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        dat_games.push_str(&format!(
            "<game name=\"{name}\"><rom name=\"{cue_name}\" size=\"{}\" sha1=\"{cue_sha1}\"/><rom name=\"{bin_name}\" size=\"{}\" sha1=\"{bin_sha1}\"/></game>",
            cue_text.len(), raw.len()));
        cues.push(cue);
    }
    let dat_path = root.path().join("synthetic.dat");
    fs::write(&dat_path, format!(
        "<?xml version=\"1.0\"?><datafile><header><name>Sony - PlayStation</name><version>phase1</version><author>synthetic</author></header>{dat_games}</datafile>"
    )).unwrap();
    let mut db = Database::open_or_create(root.path().join("library.sqlite3")).unwrap();
    let config = crate::Config {
        source_folders: vec![root.path().join("games")],
        mount_root: root.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    crate::scan_and_persist(&mut db, &config, "test").unwrap();
    let parsed = crate::dat::parsers::parse_dat_file(&dat_path, DatLimits::default()).unwrap();
    let expected =
        crate::dat::expected_inventory::project_expected_dat_inventory(&parsed.dat.games);
    db.replace_expected_dat_inventory(
        SOURCE,
        parsed.dat.source.version.as_deref(),
        Some(parsed.dat.source.ecosystem),
        &expected.entries,
        expected.duplicate_names_skipped as u64,
    )
    .unwrap();
    let audit = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: SOURCE.into(),
            source_display_name: "Synthetic PSX DAT".into(),
            dat_path,
            dat_kind: DatSourceKind::File,
            scan_root: root.path().join("games"),
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
    db.persist_library_dat_identities_from_audit(&audit)
        .unwrap();
    let outcome = db.refresh_dat_disc_order(1).unwrap();
    assert_eq!(outcome.discs, count, "{outcome:?}");
    assert_eq!(outcome.written, 1, "{outcome:?}");
    let sets = db
        .load_media_topology_evidence(&db.load_archives().unwrap())
        .unwrap();
    assert_eq!(sets.len(), 1);
    let sources = vec![DatSourceEntry::new(
        SOURCE.into(),
        "Synthetic PSX DAT".into(),
        root.path().join("synthetic.dat"),
        DatSourceKind::File,
    )];
    Fixture {
        root,
        db,
        set: sets.into_iter().next().unwrap(),
        cues,
        sources,
    }
}

fn destination(f: &Fixture) -> PathBuf {
    let out = f.root.path().join("out");
    fs::create_dir_all(&out).unwrap();
    out.join("Synthetic Quest.pbp")
}

#[test]
fn region_and_serial_are_fail_closed() {
    assert_eq!(region_of("Example (USA)"), Some("USA"));
    assert_eq!(serial_region("SLUS-12345"), Some("USA"));
    assert_eq!(serial_region("SLES-12345"), Some("Europe"));
    assert_eq!(region_of("Example"), None);
    assert_eq!(region_of("Example (USA) (Europe)"), None);
    assert_eq!(serial_region("UNKNOWN-12345"), None);
}

#[test]
#[ignore = "requires pinned external PSXPackager and genisoimage; run scripts/test-ps1-pbp-export.sh"]
fn two_and_five_disc_roundtrip_publish_with_receipt() {
    let writer = writer();
    for count in [2, 5] {
        let f = fixture(count);
        let destination = destination(&f);
        let cancel = AtomicBool::new(false);
        let plan =
            plan_ps1_pbp_export(&f.db, &f.sources, &f.set, &destination, &writer, &cancel).unwrap();
        assert_eq!(plan.discs.len(), count);
        let result = execute_ps1_pbp_export(
            &f.db,
            &f.sources,
            &plan,
            &f.root.path().join("journal"),
            &cancel,
        )
        .unwrap();
        assert!(destination.is_file());
        assert!(plan.receipt_path.is_file());
        assert_eq!(result.receipt.discs.len(), count);
        assert_eq!(result.receipt.label, DERIVED_LABEL);
        for cue in &f.cues {
            assert!(cue.is_file());
        }
    }
}

#[test]
#[ignore = "requires pinned external PSXPackager and genisoimage; run scripts/test-ps1-pbp-export.sh"]
fn source_replacement_and_destination_collision_refuse() {
    let writer = writer();
    let f = fixture(2);
    let target_one = destination(&f);
    let cancel = AtomicBool::new(false);
    let plan =
        plan_ps1_pbp_export(&f.db, &f.sources, &f.set, &target_one, &writer, &cancel).unwrap();
    fs::write(&plan.discs[0].bin, b"replaced").unwrap();
    assert!(
        execute_ps1_pbp_export(
            &f.db,
            &f.sources,
            &plan,
            &f.root.path().join("journal"),
            &cancel
        )
        .is_err()
    );
    assert!(!target_one.exists());
    let fresh = fixture(2);
    let target = destination(&fresh);
    fs::write(&target, b"foreign").unwrap();
    assert!(
        plan_ps1_pbp_export(
            &fresh.db,
            &fresh.sources,
            &fresh.set,
            &target,
            &writer,
            &cancel
        )
        .is_err()
    );
    assert_eq!(fs::read(target).unwrap(), b"foreign");
}

#[test]
#[ignore = "requires pinned external PSXPackager and genisoimage; run scripts/test-ps1-pbp-export.sh"]
fn six_discs_and_cancellation_refuse() {
    let writer = writer();
    let six = fixture(6);
    let target = destination(&six);
    let cancel = AtomicBool::new(false);
    assert!(
        plan_ps1_pbp_export(&six.db, &six.sources, &six.set, &target, &writer, &cancel).is_err()
    );
    let two = fixture(2);
    let target = destination(&two);
    let cancel = AtomicBool::new(true);
    assert!(
        plan_ps1_pbp_export(&two.db, &two.sources, &two.set, &target, &writer, &cancel).is_err()
    );
    assert!(!target.exists());
}

#[test]
#[ignore = "requires pinned external PSXPackager and genisoimage; run scripts/test-ps1-pbp-export.sh"]
fn wrong_order_missing_companion_and_unsupported_track_refuse() {
    let writer = writer();
    let cancel = AtomicBool::new(false);
    let mut wrong = fixture(2);
    let target = destination(&wrong);
    wrong.set.members[0].ordinal.as_mut().unwrap().number = 2;
    assert!(
        plan_ps1_pbp_export(
            &wrong.db,
            &wrong.sources,
            &wrong.set,
            &target,
            &writer,
            &cancel
        )
        .is_err()
    );

    let missing = fixture(2);
    let target = destination(&missing);
    fs::remove_file(missing.cues[0].with_extension("bin")).unwrap();
    assert!(
        plan_ps1_pbp_export(
            &missing.db,
            &missing.sources,
            &missing.set,
            &target,
            &writer,
            &cancel
        )
        .is_err()
    );

    let unsupported = fixture(2);
    let target = destination(&unsupported);
    let text = fs::read_to_string(&unsupported.cues[0]).unwrap();
    fs::write(
        &unsupported.cues[0],
        text.replace("MODE2/2352", "MODE1/2352"),
    )
    .unwrap();
    assert!(
        plan_ps1_pbp_export(
            &unsupported.db,
            &unsupported.sources,
            &unsupported.set,
            &target,
            &writer,
            &cancel
        )
        .is_err()
    );
}

#[test]
fn writer_failure_and_unpinned_writer_refuse() {
    let cancel = AtomicBool::new(false);
    let stage = tempfile::tempdir().unwrap();
    assert!(run_writer(Path::new("/bin/false"), &[], stage.path(), &cancel).is_err());
    assert!(writer_check(Path::new("/bin/false"), &cancel).is_err());
}

#[test]
#[ignore = "requires pinned external PSXPackager and genisoimage; run scripts/test-ps1-pbp-export.sh"]
fn removed_or_changed_dat_source_refuses() {
    let writer = writer();
    let f = fixture(2);
    let target = destination(&f);
    let cancel = AtomicBool::new(false);
    assert!(plan_ps1_pbp_export(&f.db, &[], &f.set, &target, &writer, &cancel).is_err());
    fs::write(f.root.path().join("synthetic.dat"), b"replaced catalogue").unwrap();
    assert!(plan_ps1_pbp_export(&f.db, &f.sources, &f.set, &target, &writer, &cancel).is_err());
}

fn interrupt_after_receipt_before_pbp(moving: &Path) {
    if moving.extension().is_some_and(|ext| ext == "pbp") {
        let out = moving.parent().unwrap().parent().unwrap().parent().unwrap();
        fs::rename(out.join("journal"), out.join("journal-saved")).unwrap();
        fs::write(out.join("journal"), b"interrupted journal namespace").unwrap();
    } else {
        crate::dat::rename_apply::executor::set_test_before_rename_hook(Some(
            interrupt_after_receipt_before_pbp,
        ));
    }
}

#[test]
#[ignore = "requires pinned external PSXPackager and genisoimage; run scripts/test-ps1-pbp-export.sh"]
fn interrupted_publication_recovers_without_touching_sources() {
    let writer = writer();
    let f = fixture(2);
    let target = destination(&f);
    let out = target.parent().unwrap();
    let journal = out.join("journal");
    let cancel = AtomicBool::new(false);
    let plan = plan_ps1_pbp_export(&f.db, &f.sources, &f.set, &target, &writer, &cancel).unwrap();
    let source_hashes = f
        .cues
        .iter()
        .map(|cue| sha256(&cue.with_extension("bin"), &cancel).unwrap())
        .collect::<Vec<_>>();
    crate::dat::rename_apply::executor::set_test_before_rename_hook(Some(
        interrupt_after_receipt_before_pbp,
    ));
    let failed = execute_ps1_pbp_export(&f.db, &f.sources, &plan, &journal, &cancel);
    crate::dat::rename_apply::executor::set_test_before_rename_hook(None);
    assert!(failed.is_err());
    // The move can complete before the sabotaged journal checkpoint. It is
    // unresolved output until the saved journal is recovered below.
    assert!(target.exists());
    assert!(plan.receipt_path.exists());
    fs::remove_file(&journal).unwrap();
    fs::rename(out.join("journal-saved"), &journal).unwrap();
    let saved = fs::read_dir(&journal)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("interrupted transaction journal");
    let mut tx = crate::dat::rename_apply::read_journal(&saved).unwrap();
    assert_eq!(tx.state, TransactionState::Applying);
    let recovered = rollback_repair_transaction(&mut tx, &journal, &cancel).unwrap();
    assert!(matches!(
        recovered,
        crate::dat::rename_apply::model::RollbackResult::FullyRolledBack
    ));
    assert!(!target.exists());
    assert!(!plan.receipt_path.exists());
    for (cue, before) in f.cues.iter().zip(source_hashes) {
        assert!(cue.exists());
        assert_eq!(sha256(&cue.with_extension("bin"), &cancel).unwrap(), before);
    }
}
