use super::*;
use archivefs_core::daedalus::collect::{LaunchArtefacts, Provenance, collect};
use archivefs_core::daedalus::evidence::ExitOutcome;
use archivefs_core::daedalus::history::{HISTORY_DIRECTORY, LaunchIdentity, SaveOutcome};
use archivefs_core::daedalus::rules::diagnose;
use std::time::{Duration, UNIX_EPOCH};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn store(dir: &tempfile::TempDir) -> HistoryStore {
    HistoryStore::at(dir.path().join(HISTORY_DIRECTORY))
}

fn record(store: &HistoryStore, title: &str, exit: i32, output: &str) -> String {
    let report = diagnose(&collect(&LaunchArtefacts {
        emulator: "retroarch".into(),
        exit: Some(ExitOutcome::Exited { code: exit }),
        stderr: Some(output.as_bytes().to_vec()),
        provenance: Provenance::ObservedByEmuWiz,
        ..LaunchArtefacts::default()
    }));
    let launch = LaunchIdentity {
        library_id: Some(7),
        title: Some(title),
        platform: Some("gb"),
        emulator: Some("retroarch"),
        core: Some("gambatte_libretro"),
        emulator_version: Some("1.21.0"),
    };
    match store.record_launch(&launch, &report, now()) {
        SaveOutcome::Saved { id, .. } => id,
        other => panic!("not saved: {other:?}"),
    }
}

fn files(store: &HistoryStore) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(store.directory())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

const FAILED: &str = "[INFO] [Core]: starting\n[ERROR] Failed to create Vulkan instance.\n";
const SAVE_FAILED: &str = "[INFO] [Core]: starting\n[ERROR] [SRAM]: Failed to save SRAM.\n";

#[test]
fn arguments_are_parsed_strictly() {
    assert_eq!(parse(&args(&[])), Ok(Request::List { json: false }));
    assert_eq!(parse(&args(&["list"])), Ok(Request::List { json: false }));
    assert_eq!(
        parse(&args(&["--json", "list"])),
        Ok(Request::List { json: true })
    );
    assert_eq!(
        parse(&args(&["show", "x", "--json"])),
        Ok(Request::Show {
            id: "x".into(),
            json: true
        })
    );
    assert_eq!(parse(&args(&["status"])), Ok(Request::Status));
    assert_eq!(
        parse(&args(&["delete", "x"])),
        Ok(Request::Delete { id: "x".into() })
    );
    assert_eq!(parse(&args(&["clear"])), Ok(Request::Clear));
    assert_eq!(parse(&args(&["on"])), Ok(Request::Switch { on: true }));
    assert_eq!(parse(&args(&["off"])), Ok(Request::Switch { on: false }));
    for help in ["--help", "-h", "help"] {
        assert_eq!(parse(&args(&[help])), Ok(Request::Help));
    }
    for bad in [
        &["show"][..],
        &["delete"],
        &["delete", "a", "b"],
        &["clear", "everything"],
        &["clear", "--json"],
        &["off", "--json"],
        &["list", "--json", "--json"],
        &["--help", "clear"],
        &["clear", "--help"],
        &["purge"],
    ] {
        assert!(parse(&args(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn listing_and_showing_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    // Nothing stored yet: nothing is created by looking.
    let empty = execute(Request::List { json: false }, &store, now()).unwrap();
    assert!(empty.contains("No launch is on record."), "{empty}");
    assert!(empty.contains("Recording is on."), "{empty}");
    assert!(!store.directory().exists());

    let failed = record(&store, "Tetris", 1, FAILED);
    let warned = record(&store, "Zelda", 0, SAVE_FAILED);
    // A leftover that a clean-up would remove stays while only looking.
    std::fs::write(
        store.directory().join("20270115T080000Z-00000bad.json"),
        "{",
    )
    .unwrap();
    let before = files(&store);

    let text = execute(Request::List { json: false }, &store, now()).unwrap();
    assert!(text.contains(&failed) && text.contains(&warned), "{text}");
    assert!(
        text.contains("Game 7 in the library on retroarch 1.21.0, core gambatte_libretro"),
        "{text}"
    );
    // A title is never kept: the game is named by its library id.
    assert!(
        !text.contains("Tetris") && !text.contains("Zelda"),
        "{text}"
    );
    assert!(
        text.contains("launch: the emulator process failed"),
        "{text}"
    );
    assert!(
        text.contains("Your progress may not have been saved"),
        "{text}"
    );
    assert!(
        text.contains("1 file(s) in the history folder are not readable records"),
        "{text}"
    );
    assert!(text.contains("no emulator output is kept"), "{text}");

    let json: serde_json::Value =
        serde_json::from_str(&execute(Request::List { json: true }, &store, now()).unwrap())
            .unwrap();
    assert_eq!(json["recording"], true);
    assert_eq!(json["records"].as_array().unwrap().len(), 2);
    assert_eq!(json["unreadable"], 1);

    let shown = execute(
        Request::Show {
            id: warned.clone(),
            json: false,
        },
        &store,
        now(),
    )
    .unwrap();
    assert!(shown.contains("Game: Game 7 in the library"), "{shown}");
    assert!(!shown.contains("Zelda"), "{shown}");
    assert!(
        shown.contains("Launch: no failure of the launch was observed"),
        "{shown}"
    );
    assert!(shown.contains("What to check:"), "{shown}");
    let status = execute(Request::Status, &store, now()).unwrap();
    assert!(
        status.contains("Recording is on.") && status.contains("Kept: 2 record(s)."),
        "{status}"
    );
    assert!(
        status.contains("the newest 20 records, none older than 14 days"),
        "{status}"
    );

    assert_eq!(files(&store), before);
    // Nothing the emulator printed is in anything shown.
    for output in [&text, &shown, &json.to_string()] {
        assert!(!output.contains("Failed to save SRAM"), "{output}");
        assert!(!output.contains("Vulkan instance"), "{output}");
    }
}

#[test]
fn records_can_be_removed_and_recording_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let first = record(&store, "Tetris", 1, FAILED);
    let second = record(&store, "Zelda", 0, SAVE_FAILED);

    assert_eq!(
        execute(Request::Delete { id: first.clone() }, &store, now()).unwrap(),
        "Removed."
    );
    assert_eq!(
        execute(Request::Delete { id: first.clone() }, &store, now()),
        Err("no record has that id".to_string())
    );
    assert_eq!(
        execute(
            Request::Delete {
                id: "../../etc/passwd".into()
            },
            &store,
            now()
        ),
        Err("that is not a record id".to_string())
    );
    assert_eq!(
        execute(
            Request::Show {
                id: first,
                json: false
            },
            &store,
            now()
        ),
        Err("no record has that id".to_string())
    );
    assert_eq!(files(&store), [format!("{second}.json")]);

    let off = execute(Request::Switch { on: false }, &store, now()).unwrap();
    assert!(off.starts_with("Recording is off."), "{off}");
    assert!(!store.is_enabled());
    // What was stored is kept until it is cleared.
    let listed = execute(Request::List { json: false }, &store, now()).unwrap();
    assert!(
        listed.contains("Recording is off. Nothing new is kept."),
        "{listed}"
    );
    assert!(listed.contains(&second), "{listed}");
    assert_eq!(
        execute(Request::Clear, &store, now()).unwrap(),
        "Removed 1 record(s)."
    );
    assert!(!store.is_enabled());
    assert_eq!(
        execute(Request::Switch { on: true }, &store, now()).unwrap(),
        "Recording is on."
    );
    assert!(store.is_enabled());
}
