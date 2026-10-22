//! `emuwiz-cli repair rollback` exit-code hardening.
//!
//! `FullyRolledBack` must exit 0; `PartiallyRolledBack` and `RollbackFailed`
//! must exit non-zero even though the normal (text or JSON) result is still
//! printed first. These spawn the real CLI binary (never the in-process
//! `repair::run`) so the actual process exit code - the contract a caller or
//! script depends on - is exercised, not just the `Result` `run()` returns
//! internally.

archivefs_core::install_test_environment!();

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SHA1_TEST: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
const SHA1_ABC: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
const SHA1_XYZ: &str = "66b27417d37e024c46526c2f6d358a754fc552f3";

/// The spawned CLIs share this test process's isolated HOME (from
/// `install_test_environment!`). The CLI is a production binary, so it would
/// otherwise read and write the user's real config, database and audit cache;
/// and it must see the same Undo seal store as the in-process conversions
/// whose journals it undoes, or it rightly refuses them as unsealed.
fn throwaway_home() -> &'static Path {
    static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        archivefs_core::test_environment::isolated_root()
            .expect("the isolated test environment")
            .join("home")
    })
}

fn run_cli(args: &[&str]) -> Output {
    let home = throwaway_home();
    Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"))
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env_remove("EMUWIZ_DATA_HOME")
        .output()
        .expect("the CLI must run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temp dir")
}

/// One DAT game/rom, one wrongly-named loose ROM: a single-entry batch.
fn single_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let dat = dir.join("single.dat");
    std::fs::write(
        &dat,
        format!(
            r#"<?xml version="1.0"?>
<datafile>
    <header><name>Single</name></header>
    <game name="Super Game (World)">
        <rom name="super.bin" size="4" sha1="{SHA1_TEST}"/>
    </game>
</datafile>"#
        ),
    )
    .unwrap();
    let roms = dir.join("roms");
    std::fs::create_dir(&roms).unwrap();
    std::fs::write(roms.join("wrongname.bin"), b"test").unwrap();
    (dat, roms)
}

/// Three DAT games, three wrongly-named loose ROMs: a three-entry batch, so
/// a rollback can reverse some entries before hitting a tampered one.
fn three_entry_fixture(dir: &Path) -> (PathBuf, PathBuf) {
    let dat = dir.join("three.dat");
    std::fs::write(
        &dat,
        format!(
            r#"<datafile><header><name>Three</name></header>
<game name="Alpha"><rom name="alpha.bin" size="4" sha1="{SHA1_TEST}"/></game>
<game name="Beta"><rom name="beta.bin" size="3" sha1="{SHA1_ABC}"/></game>
<game name="Gamma"><rom name="gamma.bin" size="3" sha1="{SHA1_XYZ}"/></game>
</datafile>"#
        ),
    )
    .unwrap();
    let roms = dir.join("roms");
    std::fs::create_dir(&roms).unwrap();
    std::fs::write(roms.join("a.bin"), b"test").unwrap();
    std::fs::write(roms.join("b.bin"), b"abc").unwrap();
    std::fs::write(roms.join("c.bin"), b"xyz").unwrap();
    (dat, roms)
}

fn plan_generation(plan_path: &Path) -> u64 {
    let text = std::fs::read_to_string(plan_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value["generation"].as_u64().expect("a generation number")
}

/// Scans and applies the whole plan through the real CLI, returning the
/// journal directory and the resulting transaction's id. Asserts exactly one
/// journal was written (a whole-plan apply is a single `RenameTransaction`).
fn scan_and_apply(dir: &Path, dat: &Path, roms: &Path) -> (PathBuf, String) {
    let plan_path = dir.join("plan.json");
    let journal_dir = dir.join("journal");

    let scan = run_cli(&[
        "repair",
        "scan",
        "--root",
        roms.to_str().unwrap(),
        "--dat",
        dat.to_str().unwrap(),
        "--plan-out",
        plan_path.to_str().unwrap(),
    ]);
    assert!(scan.status.success(), "{}", stdout(&scan));

    let generation = plan_generation(&plan_path).to_string();
    let apply = run_cli(&[
        "repair",
        "apply",
        "--plan",
        plan_path.to_str().unwrap(),
        "--root",
        roms.to_str().unwrap(),
        "--dat",
        dat.to_str().unwrap(),
        "--generation",
        &generation,
        "--journal-dir",
        journal_dir.to_str().unwrap(),
    ]);
    assert!(apply.status.success(), "{}", stdout(&apply));

    let journal_files: Vec<PathBuf> = std::fs::read_dir(&journal_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(journal_files.len(), 1, "{journal_files:?}");
    let transaction_id = journal_files[0]
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    (journal_dir, transaction_id)
}

// 1. FullyRolledBack => success (exit code 0).
#[test]
fn fully_rolled_back_exits_successfully() {
    let dir = temp();
    let (dat, roms) = single_fixture(dir.path());
    let (journal_dir, transaction_id) = scan_and_apply(dir.path(), &dat, &roms);

    let rollback = run_cli(&[
        "repair",
        "rollback",
        "--transaction",
        &transaction_id,
        "--journal-dir",
        journal_dir.to_str().unwrap(),
    ]);
    let text = stdout(&rollback);
    assert!(
        rollback.status.success(),
        "a full rollback must exit 0:\n{text}"
    );
    assert!(text.contains("FullyRolledBack"), "{text}");
    assert!(roms.join("wrongname.bin").exists());
}

// 2. PartiallyRolledBack => non-zero/error exit, with the normal text result
// still printed.
#[test]
fn partially_rolled_back_exits_non_zero() {
    let dir = temp();
    let (dat, roms) = three_entry_fixture(dir.path());
    let (journal_dir, transaction_id) = scan_and_apply(dir.path(), &dat, &roms);

    // Tamper the FIRST journaled entry's destination. Rollback runs in
    // reverse order, so the first entry is the *last* one reversed - this
    // guarantees the other two entries roll back successfully before the
    // tampered one is reached and stops the pass.
    let journal_path = journal_dir.join(format!("{transaction_id}.json"));
    let journal_value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&journal_path).unwrap()).unwrap();
    let first_destination = journal_value["entries"][0]["destination_path"]
        .as_str()
        .expect("the first entry's destination path")
        .to_string();
    std::fs::write(&first_destination, b"tampered-content-of-a-different-size").unwrap();

    let rollback = run_cli(&[
        "repair",
        "rollback",
        "--transaction",
        &transaction_id,
        "--journal-dir",
        journal_dir.to_str().unwrap(),
    ]);
    let text = stdout(&rollback);
    assert!(
        !rollback.status.success(),
        "a partial rollback must exit non-zero:\n{text}"
    );
    assert!(text.contains("PartiallyRolledBack"), "{text}");
    assert!(text.contains("Entries restored: 2"), "{text}");
    assert!(text.contains("Entries not restored: 1"), "{text}");
}

// 3. RollbackFailed => non-zero/error exit, with the normal text result
// still printed.
#[test]
fn rollback_failed_exits_non_zero() {
    let dir = temp();
    let (dat, roms) = single_fixture(dir.path());
    let (journal_dir, transaction_id) = scan_and_apply(dir.path(), &dat, &roms);

    std::fs::write(
        roms.join("super.bin"),
        b"tampered-content-of-a-different-size",
    )
    .unwrap();

    let rollback = run_cli(&[
        "repair",
        "rollback",
        "--transaction",
        &transaction_id,
        "--journal-dir",
        journal_dir.to_str().unwrap(),
    ]);
    let text = stdout(&rollback);
    assert!(
        !rollback.status.success(),
        "a failed rollback must exit non-zero:\n{text}"
    );
    assert!(text.contains("RollbackFailed"), "{text}");
    assert!(!roms.join("wrongname.bin").exists(), "nothing was restored");
}

// 4. JSON output is still emitted before the non-zero exit on failure.
#[test]
fn json_output_is_emitted_before_the_non_zero_return_on_failure() {
    let dir = temp();
    let (dat, roms) = single_fixture(dir.path());
    let (journal_dir, transaction_id) = scan_and_apply(dir.path(), &dat, &roms);

    std::fs::write(
        roms.join("super.bin"),
        b"tampered-content-of-a-different-size",
    )
    .unwrap();

    let rollback = run_cli(&[
        "repair",
        "rollback",
        "--transaction",
        &transaction_id,
        "--journal-dir",
        journal_dir.to_str().unwrap(),
        "--json",
    ]);
    let text = stdout(&rollback);
    assert!(
        !rollback.status.success(),
        "the process must still exit non-zero even though JSON was printed:\n{text}"
    );
    let value: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("valid JSON must still be on stdout ({error}):\n{text}"));
    assert_eq!(value["result"], "RollbackFailed");
    assert_eq!(value["transaction_id"], transaction_id);
}

// --- CHD conversion Undo through the CLI -------------------------------------
//
// A CHD conversion writes two ordinary journals: one publishes the verified
// CHD, one quarantines the original CUE/BIN. Rolling the output journal back
// by itself would withdraw the CHD while the originals are still in
// quarantine and call that a full rollback. The CLI must go through the
// optical core instead: originals restored and proven first, CHD kept
// otherwise. Each test spawns the real binary, so every Undo here is a
// restart that has nothing but the journals on disk.

use archivefs_core::dat::rename_apply::{TransactionState, list_journals};
use archivefs_core::repair::{
    ChdConversionSourceMode, build_chd_conversion_plan, execute_chd_conversion,
};
use archivefs_core::safe_read::TrustedRoots;

const CHDMAN: &str = "/usr/bin/chdman";
const CUE_TEXT: &str = "FILE \"Track.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n";
const FOREIGN: &[u8] = b"foreign object at the original path";

fn bin_bytes() -> Vec<u8> {
    (0..16u8)
        .flat_map(|sector| std::iter::repeat_n(sector.wrapping_mul(5).wrapping_add(1), 2048))
        .collect()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A converted synthetic disc in a temp directory, with its journals.
struct Converted {
    dir: tempfile::TempDir,
    output_id: String,
    chd: Vec<u8>,
}

impl Converted {
    fn new(mode: ChdConversionSourceMode) -> Option<Self> {
        if !Path::new(CHDMAN).is_file() {
            eprintln!("chdman not installed; skipping");
            return None;
        }
        let dir = temp();
        std::fs::write(dir.path().join("Track.bin"), bin_bytes()).unwrap();
        std::fs::write(dir.path().join("Disc.cue"), CUE_TEXT).unwrap();
        let plan = build_chd_conversion_plan(
            &dir.path().join("Disc.cue"),
            &dir.path().join("Disc.chd"),
            mode,
            Some(Path::new(CHDMAN)),
        )
        .unwrap();
        let (result, transaction) = execute_chd_conversion(
            &plan,
            TrustedRoots::from_paths([dir.path()]),
            &dir.path().join("journal"),
            dir.path(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            result.source_quarantined,
            mode == ChdConversionSourceMode::QuarantineSource
        );
        // Only the journals survive into the spawned CLI process.
        drop(transaction);
        let chd = std::fs::read(dir.path().join("Disc.chd")).unwrap();
        Some(Self {
            dir,
            output_id: result.transaction_id,
            chd,
        })
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn journal(&self) -> PathBuf {
        self.root().join("journal")
    }
    fn undo(&self, json: bool) -> Output {
        let journal = self.journal();
        let mut args = vec![
            "repair",
            "rollback",
            "--transaction",
            self.output_id.as_str(),
            "--journal-dir",
            journal.to_str().unwrap(),
        ];
        if json {
            args.push("--json");
        }
        run_cli(&args)
    }
    fn quarantined(&self, name: &str) -> Option<PathBuf> {
        let base = self.root().join("optical-conversion");
        std::fs::read_dir(base)
            .ok()?
            .flatten()
            .map(|bucket| bucket.path().join(name))
            .find(|path| path.exists())
    }
    fn originals_in_place(&self) -> bool {
        std::fs::read(self.root().join("Track.bin")).ok() == Some(bin_bytes())
            && std::fs::read_to_string(self.root().join("Disc.cue"))
                .ok()
                .as_deref()
                == Some(CUE_TEXT)
    }
    fn originals_quarantined_intact(&self) -> bool {
        self.quarantined("Track.bin")
            .is_some_and(|path| std::fs::read(path).ok() == Some(bin_bytes()))
            && self
                .quarantined("Disc.cue")
                .is_some_and(|path| std::fs::read_to_string(path).ok().as_deref() == Some(CUE_TEXT))
    }
    fn chd_published_intact(&self) -> bool {
        std::fs::read(self.root().join("Disc.chd")).ok().as_ref() == Some(&self.chd)
    }
    /// `(output journal state, source journal state)`.
    fn states(&self) -> (TransactionState, Option<TransactionState>) {
        let (journals, problems) = list_journals(&self.journal());
        assert!(problems.is_empty(), "{problems:?}");
        let output = journals
            .iter()
            .find(|journal| journal.transaction_id == self.output_id)
            .expect("the output journal")
            .state;
        let source = journals
            .iter()
            .find(|journal| journal.transaction_id != self.output_id)
            .map(|journal| journal.state);
        (output, source)
    }
    fn journal_bytes(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(self.journal())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        files.sort();
        files
    }
}

/// CLI Undo with quarantined CUE/BIN, after a restart: the originals come
/// back byte-identical first, and only then is the CHD removed.
#[test]
fn chd_undo_restores_quarantined_originals_before_removing_the_chd() {
    let Some(disc) = Converted::new(ChdConversionSourceMode::QuarantineSource) else {
        return;
    };
    assert!(disc.originals_quarantined_intact() && !disc.originals_in_place());
    assert_eq!(
        disc.states(),
        (TransactionState::Applied, Some(TransactionState::Applied))
    );

    let output = disc.undo(false);
    let text = stdout(&output);
    assert!(output.status.success(), "{text}\n{}", stderr(&output));
    assert!(text.contains("Result: FullyRolledBack"), "{text}");
    assert!(text.contains("Conversion undone"), "{text}");

    assert!(
        disc.originals_in_place(),
        "originals restored byte-identically"
    );
    assert!(!disc.root().join("Disc.chd").exists(), "CHD withdrawn");
    assert!(disc.quarantined("Track.bin").is_none() && disc.quarantined("Disc.cue").is_none());
    assert_eq!(
        disc.states(),
        (
            TransactionState::RolledBack,
            Some(TransactionState::RolledBack)
        )
    );

    // A second request has nothing left to undo and says so; it must not
    // report another successful rollback or touch the restored originals.
    let again = disc.undo(false);
    assert!(!again.status.success());
    assert!(
        stderr(&again).contains("not rollbackable"),
        "{}",
        stderr(&again)
    );
    assert!(disc.originals_in_place());
}

/// The same, for a conversion that kept its originals: valid Undo after a
/// restart still works, and reports itself as a conversion in JSON.
#[test]
fn chd_undo_after_restart_with_kept_originals_reports_json() {
    let Some(disc) = Converted::new(ChdConversionSourceMode::KeepSource) else {
        return;
    };
    assert!(disc.originals_in_place());
    let output = disc.undo(true);
    assert!(output.status.success(), "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["chd_conversion"], true);
    let mut expected: Vec<String> = ORDINARY_ROLLBACK_JSON_KEYS
        .iter()
        .map(|key| key.to_string())
        .collect();
    expected.push("chd_conversion".into());
    expected.sort();
    assert_eq!(json_keys(&json), expected, "ordinary keys plus the marker");
    assert_eq!(json["result"], "FullyRolledBack");
    assert!(disc.originals_in_place());
    assert!(!disc.root().join("Disc.chd").exists());
    assert_eq!(disc.states(), (TransactionState::RolledBack, None));
}

/// CLI Undo with blocked restoration: a foreign file now sits at the original
/// BIN path. The Undo is reported as partial with a failing exit status, the
/// verified CHD is kept byte-identical, and nothing foreign is overwritten.
#[test]
fn chd_undo_with_blocked_restoration_keeps_the_chd_and_exits_non_zero() {
    let Some(disc) = Converted::new(ChdConversionSourceMode::QuarantineSource) else {
        return;
    };
    std::fs::write(disc.root().join("Track.bin"), FOREIGN).unwrap();

    for attempt in 0..2 {
        let output = disc.undo(false);
        let text = stdout(&output);
        assert!(!output.status.success(), "attempt {attempt}: {text}");
        assert!(
            !text.contains("Result: FullyRolledBack"),
            "attempt {attempt}: {text}"
        );
        assert!(
            !text.contains("Conversion undone"),
            "attempt {attempt}: {text}"
        );
        assert!(
            text.contains("Result: PartiallyRolledBack") || text.contains("Result: RollbackFailed"),
            "attempt {attempt}: {text}"
        );
        assert!(disc.chd_published_intact(), "attempt {attempt}: CHD kept");
        assert_eq!(
            std::fs::read(disc.root().join("Track.bin")).unwrap(),
            FOREIGN,
            "attempt {attempt}: the foreign file is never overwritten"
        );
        assert_eq!(
            std::fs::read(disc.quarantined("Track.bin").unwrap()).unwrap(),
            bin_bytes(),
            "attempt {attempt}: the original BIN is still safe in quarantine"
        );
        assert_eq!(
            disc.states(),
            (
                TransactionState::Applied,
                Some(TransactionState::RollbackFailed)
            ),
            "attempt {attempt}"
        );
    }
    let first = stdout(&disc.undo(false));
    assert!(first.contains("Files may already have moved"), "{first}");
    assert!(!first.contains("CHD was kept"), "{first}");
}

/// CLI Undo using a legacy unpaired output journal: without the recorded
/// originals there is nothing to prove, so the CLI refuses, changes nothing
/// (files and journals byte-identical), and says what to do instead.
#[test]
fn chd_undo_of_a_legacy_unpaired_output_journal_is_refused_untouched() {
    for mode in [
        ChdConversionSourceMode::QuarantineSource,
        ChdConversionSourceMode::KeepSource,
    ] {
        let Some(disc) = Converted::new(mode) else {
            return;
        };
        // Rewrite the output journal as an older build would have left it.
        let path = disc.journal().join(format!("{}.json", disc.output_id));
        let mut journal: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(
            journal
                .as_object_mut()
                .unwrap()
                .remove("chd_conversion_recovery")
                .is_some(),
            "the current journal carries the recovery link"
        );
        std::fs::write(&path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
        let journals_before = disc.journal_bytes();

        let output = disc.undo(false);
        let error = stderr(&output);
        assert!(!output.status.success(), "{mode:?}: {}", stdout(&output));
        assert!(!stdout(&output).contains("FullyRolledBack"), "{mode:?}");
        assert!(
            error.contains("cannot be undone automatically"),
            "{mode:?}: {error}"
        );
        assert!(error.contains("before rollback"), "{mode:?}: {error}");
        assert!(!error.contains("Nothing was changed"), "{mode:?}: {error}");
        assert!(
            !error.contains("CHD is still in place"),
            "{mode:?}: {error}"
        );
        assert!(error.contains("repair history"), "{mode:?}: {error}");

        assert!(disc.chd_published_intact(), "{mode:?}: CHD kept");
        match mode {
            ChdConversionSourceMode::QuarantineSource => {
                assert!(disc.originals_quarantined_intact(), "{mode:?}");
            }
            ChdConversionSourceMode::KeepSource => assert!(disc.originals_in_place(), "{mode:?}"),
        }
        assert_eq!(
            disc.journal_bytes(),
            journals_before,
            "{mode:?}: journals untouched"
        );
    }
}

/// Normal non-CHD CLI Undo is unchanged: the ordinary rename journal still
/// goes through the shared rollback engine, with the same text and JSON, and
/// is not labelled as a conversion.
#[test]
fn non_chd_rollback_is_unchanged_by_the_chd_route() {
    let dir = temp();
    let (dat, roms) = single_fixture(dir.path());
    let (journal_dir, transaction_id) = scan_and_apply(dir.path(), &dat, &roms);
    assert_eq!(std::fs::read(roms.join("super.bin")).unwrap(), b"test");

    let output = run_cli(&[
        "repair",
        "rollback",
        "--transaction",
        &transaction_id,
        "--journal-dir",
        journal_dir.to_str().unwrap(),
    ]);
    let text = stdout(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("Result: FullyRolledBack"), "{text}");
    assert!(text.contains("Entries restored: 1"), "{text}");
    assert!(
        !text.contains("Conversion") && !text.contains("CHD"),
        "{text}"
    );
    assert_eq!(std::fs::read(roms.join("wrongname.bin")).unwrap(), b"test");
    assert!(!roms.join("super.bin").exists());
    let (journals, _) = list_journals(&journal_dir);
    assert_eq!(journals[0].state, TransactionState::RolledBack);

    // JSON on a second, identical batch. The shape is exactly the one this
    // command always had: no conversion marker, not even a `false` placeholder.
    let dir = temp();
    let (dat, roms) = single_fixture(dir.path());
    let (journal_dir, transaction_id) = scan_and_apply(dir.path(), &dat, &roms);
    let output = run_cli(&[
        "repair",
        "rollback",
        "--transaction",
        &transaction_id,
        "--journal-dir",
        journal_dir.to_str().unwrap(),
        "--json",
    ]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json_keys(&json), ORDINARY_ROLLBACK_JSON_KEYS);
    assert!(json.get("chd_conversion").is_none());
    assert_eq!(json["result"], "FullyRolledBack");
    assert_eq!(json["restored"].as_array().unwrap().len(), 0);
    assert_eq!(json["failed"].as_array().unwrap().len(), 0);
    assert_eq!(json["transaction"]["state"], "rolled_back");
}

/// The top-level keys of `repair rollback --json` as emitted before CHD Undo
/// was routed through the optical core (captured from the unmodified CLI).
const ORDINARY_ROLLBACK_JSON_KEYS: [&str; 6] = [
    "failed",
    "restored",
    "result",
    "reverify",
    "transaction",
    "transaction_id",
];

fn json_keys(value: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

/// Failed ordinary rollbacks keep exactly the same JSON shape, result
/// semantics and non-zero exit as before, for each way a rollback can be
/// refused. The `failed` entries stay `{path, reason}` objects.
#[test]
fn non_chd_failed_rollback_json_keeps_its_exact_shape_and_exit_code() {
    type Tamper = fn(&Path);
    let cases: [(&str, Tamper); 3] = [
        ("destination replaced", |roms| {
            std::fs::write(roms.join("super.bin"), b"changed").unwrap();
        }),
        ("original path occupied", |roms| {
            std::fs::write(roms.join("wrongname.bin"), b"foreign").unwrap();
        }),
        ("destination removed", |roms| {
            std::fs::remove_file(roms.join("super.bin")).unwrap();
        }),
    ];
    for (name, tamper) in cases {
        let dir = temp();
        let (dat, roms) = single_fixture(dir.path());
        let (journal_dir, transaction_id) = scan_and_apply(dir.path(), &dat, &roms);
        tamper(&roms);

        let output = run_cli(&[
            "repair",
            "rollback",
            "--transaction",
            &transaction_id,
            "--journal-dir",
            journal_dir.to_str().unwrap(),
            "--json",
        ]);
        assert!(!output.status.success(), "{name}: must exit non-zero");
        let json: serde_json::Value = serde_json::from_str(&stdout(&output))
            .unwrap_or_else(|error| panic!("{name}: {error}: {}", stdout(&output)));
        assert_eq!(json_keys(&json), ORDINARY_ROLLBACK_JSON_KEYS, "{name}");
        assert!(json.get("chd_conversion").is_none(), "{name}");
        assert_eq!(json["result"], "RollbackFailed", "{name}");
        let failed = json["failed"].as_array().unwrap();
        assert_eq!(failed.len(), 1, "{name}: {json}");
        let mut entry_keys: Vec<_> = failed[0].as_object().unwrap().keys().cloned().collect();
        entry_keys.sort();
        assert_eq!(entry_keys, ["path", "reason"], "{name}");
        assert_eq!(json["transaction_id"], transaction_id.as_str(), "{name}");
    }
}

/// The originals are restored but the CHD can no longer be removed (it was
/// replaced): the foreign file survives, the exit is non-zero, and reporting
/// distinguishes the restored originals from the unverified published object.
#[test]
fn chd_undo_that_restores_originals_but_cannot_remove_the_chd_says_so() {
    let Some(disc) = Converted::new(ChdConversionSourceMode::QuarantineSource) else {
        return;
    };
    let chd_path = disc.root().join("Disc.chd");
    std::fs::write(&chd_path, FOREIGN).unwrap();

    for attempt in 0..2 {
        let output = disc.undo(false);
        let text = stdout(&output);
        assert!(!output.status.success(), "attempt {attempt}: {text}");
        assert!(
            text.contains("Result: RollbackFailed"),
            "attempt {attempt}: {text}"
        );
        assert!(
            text.contains("Files may already have moved"),
            "attempt {attempt}: {text}"
        );
        assert!(
            !text.contains("not undone; nothing was deleted"),
            "attempt {attempt}: {text}"
        );
        assert!(!text.contains("verified CHD was kept"), "{text}");
        assert!(text.contains("identity_mismatch"), "{text}");
        for name in ["Disc.cue", "Track.bin", "Disc.chd", "output.chd"] {
            assert!(text.contains(name), "{name}: {text}");
        }
        assert!(disc.originals_in_place(), "attempt {attempt}");
        assert_eq!(
            std::fs::read(&chd_path).unwrap(),
            FOREIGN,
            "attempt {attempt}"
        );
        assert_eq!(
            disc.states(),
            (
                TransactionState::RollbackFailed,
                Some(TransactionState::RolledBack)
            ),
            "attempt {attempt}"
        );
    }
}

#[cfg(target_os = "linux")]
#[path = "repair_rollback_exit_code/chd_journal_faults.rs"]
mod chd_journal_faults;
