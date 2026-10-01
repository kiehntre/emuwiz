//! `emuwiz-cli inspect`: observational by construction. Every test runs the
//! real binary against a throwaway HOME so a stray config, database or cache
//! write would be visible.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct World {
    _temp: tempfile::TempDir,
    home: PathBuf,
    library: PathBuf,
}

fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let library = temp.path().join("library");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&library).unwrap();
    World {
        _temp: temp,
        home,
        library,
    }
}

fn nes(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = b"NES\x1a\x01\x01".to_vec();
    bytes.extend(vec![0u8; 10 + 16384 + 8192]);
    fs::write(path, bytes).unwrap();
}

fn run(world: &World, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
        .arg("inspect")
        .args(args)
        .env("HOME", &world.home)
        .env("XDG_CONFIG_HOME", world.home.join(".config"))
        .env("XDG_DATA_HOME", world.home.join(".local/share"))
        .env("XDG_CACHE_HOME", world.home.join(".cache"))
        .env("XDG_STATE_HOME", world.home.join(".local/state"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A hang (for example opening a FIFO) must fail the test, not stall it.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        assert!(Instant::now() < deadline, "inspect did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn json(world: &World, args: &[&str]) -> serde_json::Value {
    let mut all = args.to_vec();
    all.push("--json");
    let output = run(world, &all);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Everything observable about a tree: names, kinds, sizes, modes, mtimes,
/// ctimes, inodes and content.
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, path: &Path, out: &mut BTreeMap<String, String>) {
        let meta = fs::symlink_metadata(path).unwrap();
        let key = path.strip_prefix(base).unwrap().display().to_string();
        let content = if meta.is_file() {
            format!(
                "{:?}",
                fs::read(path)
                    .unwrap()
                    .iter()
                    .fold(0u64, |a, b| a.wrapping_mul(131).wrapping_add(*b as u64))
            )
        } else {
            String::new()
        };
        out.insert(
            key,
            format!(
                "{} {} {:o} {}.{} {}.{} {} {content}",
                meta.file_type().is_dir(),
                meta.len(),
                meta.mode(),
                meta.mtime(),
                meta.mtime_nsec(),
                meta.ctime(),
                meta.ctime_nsec(),
                meta.ino()
            ),
        );
        if meta.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(base, &entry.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn recognised_file_in_a_spaced_non_ascii_path_separates_checksum_from_identity() {
    let w = world();
    let rom = w.library.join("rom dir/ünï côdé/Gäme (USA) [!].nes");
    nes(&rom);
    let report = json(&w, &[rom.to_str().unwrap()]);
    assert_eq!(report["read_only"], true);
    assert_eq!(report["database"], "not opened");
    assert_eq!(report["target"]["kind"], "file");
    assert_eq!(report["target"]["path_is_utf8"], true);
    assert_eq!(report["platform"]["platform"], "NES");
    assert_eq!(report["platform"]["confidence"], "confirmed");
    assert_eq!(report["platform"]["deciding_source"], "signature");
    let identity = &report["identity"];
    assert_eq!(identity["platform_hint_source"], "detected_confirmed");
    assert_eq!(identity["verified_checksum"], true);
    // A checksum is not a game identity; nothing here names the game.
    assert_eq!(identity["verified_identity"], false);
    assert_eq!(report["outcome"], "recognised_unverified");
    let facts = identity["facts"].as_array().unwrap();
    let sha = facts
        .iter()
        .find(|f| f["kind"] == "Local ROM SHA-256")
        .unwrap();
    assert_eq!(sha["class"], "verified_fact");
    assert_eq!(sha["category"], "checksum");
    // The echoed platform hint and the filename-derived title are labelled.
    for fact in facts {
        if fact["kind"] == "Platform" {
            assert_eq!(fact["class"], "catalogue_context");
            assert_ne!(fact["class"], "verified_fact");
        }
        if fact["kind"] == "Normalized ROM title" {
            assert_ne!(fact["class"], "verified_fact");
        }
    }
    let reasons: Vec<_> = report["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["reason"].as_str().unwrap())
        .collect();
    assert!(reasons.contains(&"insufficient_evidence"), "{reasons:?}");
}

#[test]
fn unknown_and_malformed_inputs_are_reported_not_guessed() {
    let w = world();
    let unknown = w.library.join("mystery.dat");
    fs::write(&unknown, b"just some text, no platform at all").unwrap();
    let report = json(&w, &[unknown.to_str().unwrap()]);
    assert_eq!(report["outcome"], "not_recognised");
    assert_eq!(report["identity"]["verified_identity"], false);
    assert_eq!(report["identity"]["verified_checksum"], false);
    assert!(
        report["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["reason"] == "not_recognised")
    );

    // Truncated NES header and an empty ROM must not panic or over-claim.
    for (name, bytes) in [("short.nes", &b"NES\x1a"[..]), ("empty.nes", &b""[..])] {
        let path = w.library.join(name);
        fs::write(&path, bytes).unwrap();
        let output = run(&w, &[path.to_str().unwrap(), "--json"]);
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_ne!(report["outcome"], "recognised_verified", "{name}");
        assert_eq!(report["identity"]["verified_identity"], false, "{name}");
    }
}

#[test]
fn shared_extension_is_ambiguous_with_candidates_and_no_selected_platform() {
    let w = world();
    let iso = w.library.join("unknown disc.iso");
    fs::write(&iso, vec![0u8; 4096]).unwrap();
    let report = json(&w, &[iso.to_str().unwrap()]);
    assert_eq!(report["outcome"], "ambiguous");
    assert_eq!(report["platform"]["confidence"], "ambiguous");
    assert!(report["platform"]["platform"].is_null());
    assert!(report["platform"]["candidates"].as_array().unwrap().len() > 1);
    assert!(
        report["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["reason"] == "ambiguous")
    );
    // Ambiguity must not be turned into a hint for the identity inspector.
    assert_eq!(report["identity"]["platform_hint_source"], "none");
    assert_eq!(report["identity"]["verified_identity"], false);
}

#[test]
fn unsupported_topology_and_unevaluated_emulator_readiness_are_explicit() {
    let w = world();
    let rom = w.library.join("game.nes");
    nes(&rom);
    let report = json(&w, &[rom.to_str().unwrap(), "--readiness"]);
    let readiness = &report["readiness"];
    assert!(
        readiness["emulator_and_firmware"]
            .as_str()
            .unwrap()
            .starts_with("not evaluated")
    );
    let set = &readiness["media_sets"][0];
    assert_eq!(set["action_safety"], "BLOCKED");
    assert_eq!(set["topology_state"], "UNSUPPORTED_SET");
    assert!(
        report["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["reason"] == "unsupported")
    );
    // Only the requested view (plus the always-present summary) is emitted.
    for absent in ["identity", "evidence", "provenance", "media", "platform"] {
        assert!(report.get(absent).is_none(), "{absent}");
    }
}

#[test]
fn json_has_a_stable_typed_shape_without_debug_blobs() {
    let w = world();
    let rom = w.library.join("shape.nes");
    nes(&rom);
    let report = json(&w, &[rom.to_str().unwrap()]);
    let keys: Vec<_> = report.as_object().unwrap().keys().cloned().collect();
    for key in [
        "read_only",
        "database",
        "target",
        "source_root",
        "outcome",
        "platform",
        "identity",
        "evidence",
        "provenance",
        "readiness",
        "media",
        "blockers",
    ] {
        assert!(keys.contains(&key.to_string()), "{key} in {keys:?}");
    }
    assert!(
        report["provenance"]["not_consulted"]
            .as_str()
            .unwrap()
            .contains("database")
    );
    assert!(report["evidence"]["identity"].is_array());
    assert!(report["media"]["topology"]["sets"].is_array());
    // No Rust Debug formatting leaked into string values.
    let text = serde_json::to_string(&report).unwrap();
    assert!(!text.contains("Some("), "{text}");
    assert!(!text.contains("PathBuf"), "{text}");
}

#[test]
fn inspection_changes_nothing_and_creates_no_config_database_or_cache() {
    let w = world();
    let rom = w.library.join("sub dir/untouched.nes");
    nes(&rom);
    let neighbour = w.library.join("sub dir/untouched.cue");
    fs::write(&neighbour, "FILE \"x.bin\" BINARY\n").unwrap();
    let before_library = snapshot(&w.library);
    let before_home = snapshot(&w.home);
    for extra in [
        &[][..],
        &["--identity"],
        &["--media", "--readiness"],
        &["--evidence", "--provenance"],
    ] {
        let mut args = vec![rom.to_str().unwrap()];
        args.extend_from_slice(extra);
        assert!(run(&w, &args).status.success());
        // Text mode too.
        let mut args = vec![w.library.join("sub dir").to_str().unwrap().to_string()];
        args.extend(extra.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert!(run(&w, &refs).status.success());
    }
    // Control: the snapshot really would notice a change.
    fs::write(w.library.join("control"), b"x").unwrap();
    assert_ne!(snapshot(&w.library), before_library);
    fs::remove_file(w.library.join("control")).unwrap();
    let after = snapshot(&w.library);
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        before_library.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        after["sub dir/untouched.nes"], before_library["sub dir/untouched.nes"],
        "library changed"
    );
    assert_eq!(
        after["sub dir/untouched.cue"], before_library["sub dir/untouched.cue"],
        "library changed"
    );
    assert_eq!(
        snapshot(&w.home),
        before_home,
        "HOME gained files (config/db/cache)"
    );
    assert!(fs::read_dir(&w.home).unwrap().next().is_none());
}

#[test]
fn special_files_and_missing_paths_are_refused_cleanly() {
    let w = world();
    let fifo = w.library.join("pipe.bin");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc_mkfifo(c.as_ptr(), 0o600) }, 0);
    let output = run(&w, &[fifo.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe/refused"));
    // A symlink to the FIFO is refused too, without opening it.
    let link = w.library.join("link.bin");
    std::os::unix::fs::symlink(&fifo, &link).unwrap();
    let output = run(&w, &[link.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsafe/refused"));

    let output = run(&w, &[w.library.join("nope.nes").to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not found or not accessible"));
}

#[test]
fn argument_errors_name_the_problem() {
    let w = world();
    for (args, expected) in [
        (&["--bogus", "x"][..], "unsupported option"),
        (&["a", "b"], "exactly one path"),
        (&[], "requires a path"),
    ] {
        let output = run(&w, args);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
    }
}

#[test]
fn a_directory_is_inspected_with_a_bounded_media_walk() {
    let w = world();
    for index in 0..100 {
        fs::write(w.library.join(format!("f{index}.dat")), b"x").unwrap();
    }
    let report = json(&w, &[w.library.to_str().unwrap(), "--media"]);
    assert_eq!(report["target"]["kind"], "directory");
    assert!(report["media"]["records"].as_u64().unwrap() <= 64);
}

fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
    // Deterministic xorshift: arbitrary-looking bytes without a dependency.
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

#[test]
fn arbitrary_bytes_never_become_a_verified_identity() {
    let w = world();
    let mut checksum_seen = false;
    // Structured-header samples with arbitrary payloads, so the checksum
    // branch of the invariant is actually exercised.
    for (index, seed) in [11u64, 12, 13].into_iter().enumerate() {
        let path = w.library.join(format!("headered-{index}.nes"));
        let mut bytes = b"NES\x1a\x01\x01".to_vec();
        bytes.extend(random_bytes(seed, 10 + 16_384 + 8_192));
        fs::write(&path, bytes).unwrap();
        let report = json(&w, &[path.to_str().unwrap()]);
        assert_eq!(report["identity"]["verified_identity"], false);
        checksum_seen |= report["identity"]["verified_checksum"] == true;
        assert_eq!(report["outcome"], "recognised_unverified");
    }
    for (index, extension) in [
        "bin", "dat", "nes", "sfc", "z64", "gba", "img", "rom", "iso",
    ]
    .iter()
    .enumerate()
    {
        for (size, seed) in [(0usize, 1u64), (1, 2), (77, 3), (4096, 4), (70_000, 5)] {
            let path = w.library.join(format!("random-{index}-{size}.{extension}"));
            fs::write(&path, random_bytes(seed * 31 + index as u64, size)).unwrap();
            let report = json(&w, &[path.to_str().unwrap()]);
            let identity = &report["identity"];
            // The invariant: bytes with no identity structure are at most a
            // checksum, never an identity, and never "recognised_verified".
            assert_eq!(identity["verified_identity"], false, "{path:?}");
            assert_ne!(report["outcome"], "recognised_verified", "{path:?}");
            for fact in identity["facts"].as_array().unwrap() {
                if fact["class"] == "verified_fact" {
                    assert_ne!(fact["category"], "identity", "{path:?} {fact}");
                    assert_ne!(fact["category"], "platform_context", "{path:?} {fact}");
                }
                if fact["category"] == "platform_context" {
                    assert_ne!(fact["class"], "verified_fact", "{path:?}");
                }
            }
            if identity["verified_checksum"] == true {
                checksum_seen = true;
                // The exact promise: a checksum alone is not an identity.
                assert_eq!(identity["verified_identity"], false);
                assert!(
                    report["blockers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|b| b["reason"] == "insufficient_evidence")
                        || report["outcome"] == "ambiguous"
                        || report["outcome"] == "not_recognised",
                    "{report}"
                );
            }
        }
    }
    assert!(
        checksum_seen,
        "no sample produced a checksum, so the invariant was not exercised"
    );
}

#[test]
fn a_checksum_alone_is_verified_checksum_not_verified_identity() {
    let w = world();
    let rom = w.library.join("only checksum.nes");
    // A valid iNES header with an arbitrary payload: the bytes yield a
    // checksum and a header fact, but nothing that names a game.
    let mut bytes = b"NES\x1a\x01\x01".to_vec();
    bytes.extend(random_bytes(99, 10 + 16_384 + 8_192));
    fs::write(&rom, bytes).unwrap();
    let identity = json(&w, &[rom.to_str().unwrap(), "--identity"])["identity"].clone();
    assert_eq!(identity["verified_checksum"], true);
    assert_eq!(identity["verified_identity"], false);
}

#[test]
fn scummvm_directories_skip_identity_instead_of_running_the_detector() {
    let w = world();
    let game = w.library.join("some game");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("RESOURCE.GEN"), b"x").unwrap();
    for hint in ["scummvm", "ScummVM", "scumm vm"] {
        let report = json(&w, &[game.to_str().unwrap(), "--platform", hint]);
        let identity = &report["identity"];
        assert_eq!(identity["evaluated"], false, "{hint}");
        assert!(
            identity["not_evaluated_reason"]
                .as_str()
                .unwrap()
                .contains("never does")
        );
        assert_eq!(identity["verified_identity"], false);
        assert!(identity["facts"].as_array().unwrap().is_empty());
        assert_eq!(report["identity"]["platform_hint_source"], "user");
    }
    // Other hints and plain inspection still evaluate identity.
    let report = json(&w, &[game.to_str().unwrap(), "--platform", "gamecube"]);
    assert_eq!(report["identity"]["evaluated"], true);
}

#[test]
fn plain_and_json_are_two_renderings_of_one_report() {
    let w = world();
    let rom = w.library.join("parity.nes");
    nes(&rom);
    let report = json(&w, &[rom.to_str().unwrap()]);
    let plain_output = run(&w, &[rom.to_str().unwrap()]);
    assert!(plain_output.status.success());
    let plain = String::from_utf8(plain_output.stdout).unwrap();
    assert!(plain.contains(&format!("Outcome: {}", report["outcome"].as_str().unwrap())));
    let identity = &report["identity"];
    let yes_no = |value: &serde_json::Value| if value == true { "yes" } else { "no" };
    assert!(plain.contains(&format!(
        "verified identity: {}; verified checksum: {}",
        yes_no(&identity["verified_identity"]),
        yes_no(&identity["verified_checksum"])
    )));
    // Every fact appears in both with the same class and category, and no
    // plain-text fact is stronger than its JSON class.
    for fact in identity["facts"].as_array().unwrap() {
        let line = format!(
            "[{}] {} {} = ",
            fact["class"].as_str().unwrap(),
            fact["category"].as_str().unwrap(),
            fact["kind"].as_str().unwrap()
        );
        assert!(plain.contains(&line), "{line}\n{plain}");
    }
    let plain_facts = plain
        .lines()
        .filter(|l| l.trim_start().starts_with('[') && l.contains(" = "))
        .count();
    assert!(plain_facts >= identity["facts"].as_array().unwrap().len());
    for blocker in report["blockers"].as_array().unwrap() {
        assert!(plain.contains(&format!("{}: ", blocker["reason"].as_str().unwrap())));
    }
    assert!(!plain.contains("Some(") && !plain.contains("PathBuf"));
}

#[test]
fn sockets_devices_links_to_them_and_unreadable_targets_are_refused_or_survived() {
    use std::os::unix::fs::PermissionsExt;
    let w = world();
    let socket = w.library.join("sock.bin");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let device_link = w.library.join("null-link.bin");
    std::os::unix::fs::symlink("/dev/null", &device_link).unwrap();
    let socket_link = w.library.join("sock-link.bin");
    std::os::unix::fs::symlink(&socket, &socket_link).unwrap();
    for path in [
        socket.as_path(),
        Path::new("/dev/null"),
        Path::new("/dev/zero"),
        device_link.as_path(),
        socket_link.as_path(),
    ] {
        let output = run(&w, &[path.to_str().unwrap()]);
        assert!(!output.status.success(), "{path:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unsafe/refused"),
            "{path:?}"
        );
    }
    // Inaccessible targets: a clean error or a report, never a panic.
    let locked = w.library.join("locked.nes");
    nes(&locked);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0)).unwrap();
    let locked_dir = w.library.join("locked dir");
    fs::create_dir(&locked_dir).unwrap();
    fs::write(locked_dir.join("inner.nes"), b"x").unwrap();
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0)).unwrap();
    for path in [
        locked.clone(),
        locked_dir.clone(),
        locked_dir.join("inner.nes"),
    ] {
        let output = run(&w, &[path.to_str().unwrap()]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("panicked"), "{path:?}: {stderr}");
        if !output.status.success() {
            assert!(
                stderr.contains("not found or not accessible"),
                "{path:?}: {stderr}"
            );
        }
    }
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn saves_inspect_resolves_ids_exactly_in_plain_and_json() {
    let w = world();
    // A deterministic DuckStation profile with one memory card.
    let cards = w.home.join(".local/share/duckstation/memcards");
    fs::create_dir_all(&cards).unwrap();
    fs::write(w.home.join(".local/share/duckstation/settings.ini"), b"").unwrap();
    fs::write(cards.join("shared_card_1.mcd"), vec![0u8; 131_072]).unwrap();
    let saves = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
            .arg("saves")
            .args(args)
            .env("HOME", &w.home)
            .env("XDG_CONFIG_HOME", w.home.join(".config"))
            .env("XDG_DATA_HOME", w.home.join(".local/share"))
            .output()
            .unwrap()
    };
    let listing: serde_json::Value =
        serde_json::from_slice(&saves(&["list", "--json"]).stdout).unwrap();
    let record = listing["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("shared_card_1.mcd"))
        })
        .expect("fixture record listed");
    let id = record["id"].as_str().unwrap();

    // Valid id, JSON: exactly that record, not an emulator-filtered list.
    let output = saves(&["inspect", id, "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inspected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inspected["records"].as_array().unwrap().len(), 1);
    assert_eq!(inspected["records"][0]["id"], id);
    // Valid id, plain.
    let output = saves(&["inspect", id]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("shared_card_1.mcd"));
    // The id is not an emulator filter: an explicit filter still applies.
    let output = saves(&["inspect", id, "--emulator", "pcsx2", "--json"]);
    assert!(
        !output.status.success(),
        "id must not match through a different emulator filter"
    );
    // Unknown ids fail in both modes rather than printing an empty success.
    for extra in [&[][..], &["--json"]] {
        let output = saves(&[&["inspect", "state-9999"][..], extra].concat());
        assert!(!output.status.success(), "{extra:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("was not found"));
    }
}

unsafe extern "C" {
    #[link_name = "mkfifo"]
    fn libc_mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
}

#[test]
fn saves_inspect_of_an_unknown_record_id_is_an_error_in_json_too() {
    // Regression: the record id used to be taken as an emulator filter, so
    // `--json` printed an empty report and exited 0.
    let w = world();
    for extra in [&["--json"][..], &[]] {
        let output = Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
            .args(["saves", "inspect", "no-such-record"])
            .args(extra)
            .env("HOME", &w.home)
            .env("XDG_CONFIG_HOME", w.home.join(".config"))
            .env("XDG_DATA_HOME", w.home.join(".local/share"))
            .output()
            .unwrap();
        assert!(!output.status.success(), "{extra:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("was not found"));
    }
}
