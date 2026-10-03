use super::*;
use crate::identity_source::managed_snapshot::ManagedSourceTrust;
use sha1::{Digest as _, Sha1};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn crc32(bytes: &[u8]) -> String {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    format!("{:08x}", !crc)
}

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A tiny but real listxml: two machines, each with one hashed ROM.
fn listxml(build: &str, roms: &[(&str, &str, &[u8])]) -> String {
    let machines: String = roms
        .iter()
        .map(|(machine, rom, data)| {
            format!(
                r#"<machine name="{machine}"><description>{machine}</description><rom name="{rom}" size="{}" crc="{}" sha1="{}"/></machine>"#,
                data.len(),
                crc32(data),
                sha1_hex(data)
            )
        })
        .collect();
    format!(r#"<?xml version="1.0"?><mame build="{build}">{machines}</mame>"#)
}

const ALPHA: &[u8] = b"alpha rom contents for the arcade fixture";
const BETA: &[u8] = b"beta rom contents, a different payload";

fn fixture_xml(build: &str) -> String {
    listxml(
        build,
        &[("alpha", "alpha.rom", ALPHA), ("beta", "beta.rom", BETA)],
    )
}

fn import(dir: &Path, build: &str) -> ProviderSnapshot {
    let path = dir.join("arcade-0174.xml");
    fs::write(&path, fixture_xml(build)).unwrap();
    snapshot_from_mame_listxml(&path).unwrap()
}

/// Writes an executable stand-in for MAME that records its arguments.
fn fake_mame(dir: &Path, build: &str) -> (PathBuf, PathBuf) {
    let xml = dir.join("fake.xml");
    fs::write(&xml, fixture_xml(build)).unwrap();
    let args = dir.join("args.txt");
    let script = dir.join("fake-mame");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat '{}'\n",
            args.display(),
            xml.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (script, args)
}

/// Executing a just-written file can race other test threads' forks (ETXTBSY).
fn capture_retrying(script: &Path) -> ProviderResult<ProviderSnapshot> {
    for attempt in 0..8 {
        match capture(script) {
            Err(error) if error.contains("Text file busy") && attempt < 7 => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            other => return other,
        }
    }
    unreachable!()
}

fn tree_state(root: &Path) -> Vec<(String, u64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).unwrap();
                out.push((path.display().to_string(), bytes.len() as u64, bytes));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn local_listxml_keeps_declared_build_and_says_it_was_imported_not_vouched_for() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = import(dir.path(), "0.174");
    assert_eq!(snapshot.version, "0.174");
    assert_eq!(snapshot.source_identifier, "local-import:arcade-0174.xml");
    assert!(snapshot.is_local_import());
    assert!(
        snapshot
            .warnings
            .iter()
            .any(|w| w.contains("did not download or vouch"))
    );
    snapshot.validate().unwrap();
    // The file name is display provenance only; the XML decides the version.
    let renamed = dir.path().join("totally-0.999.xml");
    fs::write(&renamed, fixture_xml("0.174")).unwrap();
    assert_eq!(
        snapshot_from_mame_listxml(&renamed).unwrap().version,
        "0.174"
    );
}

#[test]
fn imported_evidence_is_never_labelled_official() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = import(dir.path(), "0.174");
    let imported_store =
        ManagedProviderStore::new_imported(dir.path().join("imported"), IdentityProvider::Mame)
            .unwrap();
    let candidate = imported_store.stage_snapshot(&snapshot).unwrap();
    let result = imported_store.activate_snapshot(&candidate, None).unwrap();
    assert_eq!(result.active.trust, ManagedSourceTrust::UserProvided);
    assert_eq!(result.active.provider_id, "mame-local-import");
    assert_eq!(
        imported_store.active_snapshot().unwrap().unwrap().version,
        "0.174"
    );
    // The official store refuses it, and the imported store refuses a capture.
    let official = ManagedProviderStore::new(
        dir.path().join("official"),
        IdentityProvider::Mame,
        Path::new("/opt/mame/mame"),
    )
    .unwrap();
    assert!(official.stage_snapshot(&snapshot).is_err());
    let (script, _) = fake_mame(dir.path(), "0.264");
    let captured = capture_retrying(&script).unwrap();
    assert!(imported_store.stage_snapshot(&captured).is_err());
    // Verification results carry the imported origin, not the official one.
    let rom = dir.path().join("alpha.rom");
    fs::write(&rom, ALPHA).unwrap();
    let verified = verify(&snapshot, &rom).unwrap();
    assert_eq!(verified.origin, MatchOrigin::ImportedMame);
    assert_ne!(verified.detection_class, DetectionClass::OfficialExact);
    assert!(
        verified
            .details
            .iter()
            .any(|d| d.contains("did not download or vouch"))
    );
}

#[test]
fn full_capture_requests_the_whole_catalogue_not_the_three_game_poc() {
    let dir = tempfile::tempdir().unwrap();
    let (script, args) = fake_mame(dir.path(), "0.264");
    let snapshot = capture_retrying(&script).unwrap();
    let recorded = fs::read_to_string(args).unwrap();
    let words: Vec<_> = recorded.lines().collect();
    assert_eq!(
        words,
        ["-noreadconfig", "-listxml"],
        "no machine names may be passed"
    );
    assert_eq!(FULL_LISTXML_ARGS, ["-listxml"]);
    assert_eq!(snapshot.source_identifier, "official-local:mame/-listxml");
    assert_eq!(
        snapshot.record_count(),
        2,
        "every machine in the output is kept"
    );
    assert!(snapshot.warnings.iter().all(|w| !w.contains("POC")));
    assert!(!snapshot.is_local_import());
    assert_eq!(snapshot.version, "0.264");
}

#[test]
fn a_different_build_is_refused_never_substituted() {
    let dir = tempfile::tempdir().unwrap();
    let (script, _) = fake_mame(dir.path(), "0.264");
    let error = (0..8)
        .find_map(|_| match capture_matching(&script, "0.174") {
            Err(e) if e.contains("Text file busy") => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                None
            }
            other => Some(other),
        })
        .unwrap()
        .unwrap_err();
    assert!(error.contains("will not silently substitute"), "{error}");
    // verify honours the same rule.
    let snapshot = import(dir.path(), "0.264");
    let rom = dir.path().join("alpha.rom");
    fs::write(&rom, ALPHA).unwrap();
    assert!(verify_expecting(&snapshot, &rom, Some("0.174")).is_err());
    assert!(verify_expecting(&snapshot, &rom, Some("0.264")).is_ok());
    assert!(verify_expecting(&snapshot, &rom, None).is_ok());
    // Equivalent spellings of one build are the same build.
    assert_eq!(normalize_mame_version("0.264").as_deref(), Some("0.264"));
    assert_eq!(
        normalize_mame_version("0.264 (mame0264)").as_deref(),
        Some("0.264")
    );
    assert_eq!(normalize_mame_version("mame0264").as_deref(), Some("0.264"));
    assert_eq!(normalize_mame_version("mame0174").as_deref(), Some("0.174"));
    assert_eq!(normalize_mame_version("nonsense"), None);
    let unreadable = require_matching_version("garbage", &snapshot).unwrap_err();
    assert!(unreadable.contains("nothing was substituted"));
}

#[test]
fn a_folder_is_verified_and_reports_recognised_and_unrecognised_items() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = import(dir.path(), "0.174");
    let collection = dir.path().join("collection");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("alpha.rom"), ALPHA).unwrap();
    fs::write(collection.join("beta.rom"), BETA).unwrap();
    let all_known = verify(&snapshot, &collection).unwrap();
    assert_eq!(
        all_known.status,
        MatchStatus::Exact,
        "{:?}",
        all_known.details
    );
    assert_eq!(
        all_known.candidates,
        ["alpha", "beta"],
        "two games are not ambiguity"
    );
    assert!(
        all_known
            .details
            .iter()
            .any(|d| d.starts_with("Collection:") && d.contains("2 game(s) recognised"))
    );
    // An extra unknown file is reported as unrecognised, not silently ignored.
    fs::write(collection.join("mystery.bin"), b"not in any catalogue").unwrap();
    let with_extra = verify(&snapshot, &collection).unwrap();
    assert_eq!(with_extra.status, MatchStatus::Probable);
    assert!(
        with_extra
            .details
            .iter()
            .any(|d| d.contains("1 not in the catalogue")),
        "{:?}",
        with_extra.details
    );
    // A folder with nothing recognisable is NoMatch.
    let empty = dir.path().join("nothing");
    fs::create_dir_all(&empty).unwrap();
    fs::write(empty.join("x.bin"), b"zzz").unwrap();
    assert_eq!(
        verify(&snapshot, &empty).unwrap().status,
        MatchStatus::NoMatch
    );
}

#[test]
fn a_single_file_still_verifies_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = import(dir.path(), "0.174");
    let rom = dir.path().join("alpha.rom");
    fs::write(&rom, ALPHA).unwrap();
    let result = verify(&snapshot, &rom).unwrap();
    assert_eq!(result.status, MatchStatus::Exact);
    assert_eq!(result.candidates, ["alpha"]);
    assert!(!result.details.iter().any(|d| d.starts_with("Collection:")));
    assert!(verify(&snapshot, &dir.path().join("missing.rom")).is_err());
}

#[test]
fn verification_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = import(dir.path(), "0.174");
    let collection = dir.path().join("collection");
    fs::create_dir_all(collection.join("sub")).unwrap();
    fs::write(collection.join("alpha.rom"), ALPHA).unwrap();
    fs::write(collection.join("sub/beta.rom"), BETA).unwrap();
    fs::write(collection.join("junk.bin"), b"junk").unwrap();
    let before = tree_state(&collection);
    let before_names: Vec<_> = fs::read_dir(&collection)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    verify(&snapshot, &collection).unwrap();
    verify(&snapshot, &collection.join("alpha.rom")).unwrap();
    assert_eq!(tree_state(&collection), before);
    let mut after_names: Vec<_> = fs::read_dir(&collection)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let mut before_names = before_names;
    before_names.sort();
    after_names.sort();
    assert_eq!(
        after_names, before_names,
        "nothing may be created beside the files"
    );
}

#[test]
fn malformed_wrong_and_unversioned_listxml_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in [
        ("garbage.xml", "this is not xml at all".to_string()),
        ("truncated.xml", "<?xml version=\"1.0\"?><mame build=\"0.174\"><machine name=\"a\">".to_string()),
        (
            "logiqx.xml",
            r#"<?xml version="1.0"?><datafile><header><name>Other</name><version>1</version></header><game name="g"><rom name="r.bin" size="1" sha1="86f7e437fa060d3f29a2f2c2e1f7b8f7d3b2e0d4"/></game></datafile>"#.to_string(),
        ),
        (
            "nobuild.xml",
            r#"<?xml version="1.0"?><mame><machine name="a"><rom name="a.rom" size="1" crc="00000000"/></machine></mame>"#.to_string(),
        ),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, body).unwrap();
        assert!(snapshot_from_mame_listxml(&path).is_err(), "{name} must be refused");
    }
    assert!(snapshot_from_mame_listxml(&dir.path().join("absent.xml")).is_err());
}

#[test]
fn oversized_input_and_links_are_refused_before_reading() {
    let dir = tempfile::tempdir().unwrap();
    // A sparse file one byte over the limit: refused from its length alone.
    let big = dir.path().join("huge.xml");
    let file = fs::File::create(&big).unwrap();
    file.set_len(MAX_LISTXML_BYTES + 1).unwrap();
    let error = snapshot_from_mame_listxml(&big).unwrap_err();
    assert!(error.contains("import limit"), "{error}");
    // A directory or symlink is not a listxml file.
    assert!(snapshot_from_mame_listxml(dir.path()).is_err());
    let real = dir.path().join("real.xml");
    fs::write(&real, fixture_xml("0.174")).unwrap();
    let link = dir.path().join("link.xml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(snapshot_from_mame_listxml(&link).is_err());
    // Verification refuses a link to the collection, too.
    let snapshot = import(dir.path(), "0.174");
    let target = dir.path().join("target");
    fs::create_dir_all(&target).unwrap();
    let target_link = dir.path().join("target-link");
    std::os::unix::fs::symlink(&target, &target_link).unwrap();
    assert!(verify(&snapshot, &target_link).is_err());
}

#[test]
fn scummvm_cannot_be_imported_or_version_pinned_through_the_mame_paths() {
    assert!(import_provider_snapshot(IdentityProvider::ScummVm, Path::new("x")).is_err());
    assert!(
        ManagedProviderStore::new_imported(
            tempfile::tempdir().unwrap().path().to_path_buf(),
            IdentityProvider::ScummVm
        )
        .is_err()
    );
}

/// A stand-in MAME whose `-listxml` is larger than the old 256 MiB bound,
/// produced on the fly so nothing large is stored in the repository.
fn fake_huge_mame(dir: &Path, machines: u64) -> PathBuf {
    let script = dir.join("fake-huge-mame");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '<?xml version=\"1.0\"?>\\n<mame build=\"0.999 (fake)\">\\n'\n\
             yes '<machine name=\"m\"><description>padding padding padding</description></machine>' | head -n {machines}\n\
             printf '</mame>\\n'\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[test]
fn capture_larger_than_the_old_cap_is_spooled_to_disk_and_still_validated() {
    let dir = tempfile::tempdir().unwrap();
    // ~300 MB of listxml: above the former 256 MiB in-memory limit.
    let script = fake_huge_mame(dir.path(), 4_000_000);
    let snapshot = capture_retrying(&script).unwrap();
    let ProviderRecords::DatLike {
        listxml,
        machine_count,
        catalogue,
    } = &snapshot.records
    else {
        panic!("MAME evidence is listxml");
    };
    assert!(
        listxml.len() as u64 > MAX_SNAPSHOT_BYTES,
        "{}",
        listxml.len()
    );
    assert_eq!(*machine_count, 4_000_000);
    assert_eq!(snapshot.record_count(), 4_000_000);
    // No per-machine records are retained beside the XML itself.
    assert!(catalogue.games.is_empty());
    assert_eq!(snapshot.version, "0.999 (fake)");
    assert_eq!(snapshot.source_sha256, sha256(listxml.as_bytes()));
}

#[test]
fn capture_beyond_the_listxml_ceiling_is_refused_not_truncated() {
    let dir = tempfile::tempdir().unwrap();
    // A tool that never stops talking must be stopped at the ceiling.
    let script = dir.path().join("endless-mame");
    fs::write(&script, "#!/bin/sh\nexec yes '<machine name=\"m\"/>'\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let error = capture_retrying(&script).unwrap_err();
    assert!(
        error.to_ascii_lowercase().contains("limit") || error.contains("MiB"),
        "{error}"
    );
}

#[test]
fn capture_that_stops_midway_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("cut-mame");
    fs::write(
        &script,
        "#!/bin/sh\nprintf '<?xml version=\"1.0\"?><mame build=\"0.264\"><machine name=\"a\">'\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(capture_retrying(&script).is_err());
}

/// Real-world checks against an installed MAME; run explicitly:
/// `EMUWIZ_REAL_MAME=/usr/games/mame cargo test ... real_installed_mame -- --ignored --nocapture`
#[test]
#[ignore]
fn real_installed_mame_full_capture_stage_and_verify() {
    let mame = PathBuf::from(std::env::var("EMUWIZ_REAL_MAME").unwrap());
    let started = std::time::Instant::now();
    let snapshot = capture(&mame).unwrap();
    let ProviderRecords::DatLike {
        listxml,
        machine_count,
        ..
    } = &snapshot.records
    else {
        panic!()
    };
    eprintln!(
        "REAL version={} xml_bytes={} machines={} capture={:?} above_old_cap={}",
        snapshot.version,
        listxml.len(),
        machine_count,
        started.elapsed(),
        listxml.len() as u64 > MAX_SNAPSHOT_BYTES
    );
    let dir = tempfile::tempdir().unwrap();
    let store =
        ManagedProviderStore::new(dir.path().join("store"), IdentityProvider::Mame, &mame).unwrap();
    let t = std::time::Instant::now();
    let candidate = store.stage_snapshot(&snapshot).unwrap();
    store.activate_snapshot(&candidate, None).unwrap();
    eprintln!("REAL stage+activate={:?}", t.elapsed());
    let active = store.active_snapshot().unwrap().unwrap();
    assert_eq!(active.version, snapshot.version);
    // Verify a disposable folder: one real ROM-less file only; the collection is synthetic.
    let root = dir.path().join("arcade");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("unrelated.bin"), b"not a rom").unwrap();
    let t = std::time::Instant::now();
    let result = verify(&active, &root).unwrap();
    eprintln!("REAL verify={:?} status={:?}", t.elapsed(), result.status);
}

/// A disposable collection with every interesting state. Nothing outside the
/// temp directory is touched, and nothing inside it changes.
#[test]
fn directory_verify_over_a_realistic_collection_is_read_only_and_contained() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("alpha.rom"), ALPHA).unwrap(); // must never be reached via a link
    let root = dir.path().join("collection");
    fs::create_dir_all(root.join("nested/deeper")).unwrap();
    fs::write(root.join("alpha.rom"), ALPHA).unwrap(); // valid member
    fs::write(root.join("nested/beta.rom"), b"WRONG HASH CONTENT").unwrap(); // wrong hash
    fs::write(root.join("nested/deeper/readme.txt"), b"unrelated extra").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap(); // link out of the root
    let snapshot = import(dir.path(), "0.174");
    let before = tree_state(dir.path());
    let result = verify(&snapshot, &root).unwrap();
    assert_eq!(
        tree_state(dir.path()),
        before,
        "verification must not change anything"
    );
    let text = format!("{result:?}");
    assert!(text.contains("alpha"), "{text}");
    // Only a valid-alpha exact match; the wrong-hash and unrelated files never become exact.
    assert!(!matches!(result.status, MatchStatus::NoMatch), "{text}");
}
