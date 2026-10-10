//! Process-local faults against the real CLI, after its real reverse rename.
use super::*;
use archivefs_core::dat::rename_apply::{EntryState, read_journal};

fn injector() -> &'static Path {
    static INJECTOR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    INJECTOR
        .get_or_init(|| {
            let dir = temp();
            let output = Command::new("cc")
                .args(["-shared", "-fPIC", "-O2"])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/chd_rollback_journal_fault.c"),
                )
                .arg("-o")
                .arg(dir.path().join("fault.so"))
                .arg("-ldl")
                .output()
                .expect("the Linux fault regression requires a C compiler");
            assert!(output.status.success(), "{}", stderr(&output));
            dir
        })
        .path()
}

fn journal_write_fault(checkpoint: u8, json: bool) {
    let Some(disc) = Converted::new(ChdConversionSourceMode::QuarantineSource) else {
        return;
    };
    let journal_path = disc.journal().join(format!("{}.json", disc.output_id));
    let before = read_journal(&journal_path).unwrap();
    let staged = before.entries[0].source_path.clone();
    let home = throwaway_home();
    let mut command = Command::new(env!("CARGO_BIN_EXE_emuwiz-cli"));
    command
        .args([
            "repair",
            "rollback",
            "--transaction",
            &disc.output_id,
            "--journal-dir",
        ])
        .arg(disc.journal())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env_remove("EMUWIZ_DATA_HOME")
        .env("LD_PRELOAD", injector().join("fault.so"))
        .env("REVIEW_CHD_PUBLISHED", disc.root().join("Disc.chd"))
        .env("REVIEW_OUTPUT_JOURNAL", format!("{}.json", disc.output_id))
        .env("REVIEW_FAIL_AT", checkpoint.to_string());
    if json {
        command.arg("--json");
    }
    let output = command.output().unwrap();
    let text = stdout(&output);
    let error = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{text}\n{error}");
    assert!(
        error.contains("REVIEW_FAULT reverse rename succeeded"),
        "{error}"
    );
    assert!(error.contains("AFTER reverse rename"), "{error}");
    assert!(!disc.root().join("Disc.chd").exists());
    assert_eq!(std::fs::read(&staged).unwrap(), disc.chd);
    assert!(disc.originals_in_place());
    let persisted = read_journal(&journal_path).unwrap();
    assert_eq!(persisted.state, TransactionState::RollingBack);
    assert_eq!(
        persisted.entries[0].state,
        if checkpoint == 1 {
            EntryState::RollingBack
        } else {
            EntryState::RolledBack
        }
    );
    assert_eq!(disc.states().1, Some(TransactionState::RolledBack));
    assert!(!format!("{text}{error}").contains("verified CHD was kept"));
    assert!(!format!("{text}{error}").contains("Nothing was changed"));
    if json {
        let report: serde_json::Value =
            serde_json::from_str(&text).expect("CHD hard errors report JSON");
        assert_eq!(report["result"], "RollbackError");
        assert_eq!(report["chd_conversion"], true);
        assert_eq!(report["recovery"]["state"], "incomplete_or_uncertain");
        assert_eq!(report["recovery"]["published_chd"][0]["status"], "absent");
        assert_eq!(
            report["recovery"]["staged_chd"][0]["status"],
            "matches_recorded_identity"
        );
        assert_eq!(
            report["recovery"]["staged_chd"][0]["path"],
            staged.to_str().unwrap()
        );
        let originals = report["recovery"]["originals"].as_array().unwrap();
        assert_eq!(originals.len(), 2);
        assert!(
            originals
                .iter()
                .all(|original| original["status"] == "matches_recorded_identity")
        );
        let quarantine = report["recovery"]["quarantine"].as_array().unwrap();
        assert_eq!(quarantine.len(), 2);
        assert!(
            quarantine
                .iter()
                .all(|original| original["status"] == "absent")
        );
        assert!(
            report["recovery"]["guidance"]
                .as_str()
                .unwrap()
                .contains("repair history")
        );
    } else {
        assert!(error.contains("Files may already have moved"), "{error}");
        for path in [
            staged.as_path(),
            &disc.root().join("Disc.chd"),
            &disc.root().join("Disc.cue"),
            &disc.root().join("Track.bin"),
        ] {
            assert!(error.contains(path.to_str().unwrap()), "{error}");
        }
        assert!(error.contains("Quarantine CUE/BIN"), "{error}");
        assert!(error.contains("repair history"), "{error}");
    }
    // A fresh CLI process without injection can still reconcile and finish.
    let retry = disc.undo(json);
    assert!(
        retry.status.success(),
        "{}\n{}",
        stdout(&retry),
        stderr(&retry)
    );
    assert!(!staged.exists());
    assert!(disc.originals_in_place());
    assert_eq!(
        disc.states(),
        (
            TransactionState::RolledBack,
            Some(TransactionState::RolledBack)
        )
    );
}

#[test]
fn post_reverse_entry_journal_failure_text() {
    journal_write_fault(1, false);
}
#[test]
fn post_reverse_entry_journal_failure_json() {
    journal_write_fault(1, true);
}
#[test]
fn post_reverse_final_journal_failure_text() {
    journal_write_fault(2, false);
}
#[test]
fn post_reverse_final_journal_failure_json() {
    journal_write_fault(2, true);
}

#[test]
fn foreign_published_chd_json_reports_restored_originals_without_preservation_claim() {
    let Some(disc) = Converted::new(ChdConversionSourceMode::QuarantineSource) else {
        return;
    };
    std::fs::write(disc.root().join("Disc.chd"), FOREIGN).unwrap();
    for _ in 0..2 {
        let output = disc.undo(true);
        assert_eq!(output.status.code(), Some(1));
        let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(report["result"], "RollbackFailed");
        assert_eq!(
            report["recovery"]["published_chd"][0]["status"],
            "identity_mismatch"
        );
        assert_eq!(report["recovery"]["staged_chd"][0]["status"], "absent");
        assert!(
            report["recovery"]["originals"]
                .as_array()
                .unwrap()
                .iter()
                .all(|original| original["status"] == "matches_recorded_identity")
        );
        assert!(disc.originals_in_place());
        assert_eq!(
            std::fs::read(disc.root().join("Disc.chd")).unwrap(),
            FOREIGN
        );
    }
}

#[test]
fn legacy_refusal_does_not_claim_a_missing_chd_is_present() {
    let Some(disc) = Converted::new(ChdConversionSourceMode::QuarantineSource) else {
        return;
    };
    let path = disc.journal().join(format!("{}.json", disc.output_id));
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    journal
        .as_object_mut()
        .unwrap()
        .remove("chd_conversion_recovery");
    std::fs::write(&path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    std::fs::rename(disc.root().join("Disc.chd"), disc.root().join("Saved.chd")).unwrap();
    let before = disc.journal_bytes();
    for json in [false, true] {
        let output = disc.undo(json);
        let all = format!("{}{}", stdout(&output), stderr(&output));
        assert_eq!(output.status.code(), Some(1));
        assert!(!all.contains("CHD is still in place"), "{all}");
        assert!(!all.contains("Nothing was changed"), "{all}");
        assert!(all.contains("repair history"), "{all}");
        if json {
            let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
            assert_eq!(report["recovery"]["published_chd"][0]["status"], "absent");
            assert_eq!(report["recovery"]["locations_complete"], false);
        }
        assert!(disc.originals_quarantined_intact());
        assert_eq!(
            std::fs::read(disc.root().join("Saved.chd")).unwrap(),
            disc.chd
        );
        assert_eq!(disc.journal_bytes(), before);
    }
}

#[test]
fn unproven_originals_do_not_repeat_the_engines_retention_claim() {
    let Some(disc) = Converted::new(ChdConversionSourceMode::KeepSource) else {
        return;
    };
    std::fs::rename(disc.root().join("Disc.cue"), disc.root().join("Saved.cue")).unwrap();
    std::fs::write(disc.root().join("Disc.chd"), FOREIGN).unwrap();
    let before = disc.journal_bytes();
    for json in [false, true] {
        let output = disc.undo(json);
        let all = format!("{}{}", stdout(&output), stderr(&output));
        assert_eq!(output.status.code(), Some(1));
        assert!(all.contains("restoration is not proven"), "{all}");
        assert!(!all.contains("verified CHD retained"), "{all}");
        assert!(!all.contains("verified CHD was kept"), "{all}");
        assert!(all.contains("repair history"), "{all}");
        if json {
            let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
            assert_eq!(
                report["recovery"]["published_chd"][0]["status"],
                "identity_mismatch"
            );
            assert!(
                report["recovery"]["originals"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|original| original["status"] == "absent")
            );
        }
        assert_eq!(
            std::fs::read(disc.root().join("Saved.cue")).unwrap(),
            CUE_TEXT.as_bytes()
        );
        assert_eq!(
            std::fs::read(disc.root().join("Track.bin")).unwrap(),
            bin_bytes()
        );
        assert_eq!(
            std::fs::read(disc.root().join("Disc.chd")).unwrap(),
            FOREIGN
        );
        assert_eq!(disc.journal_bytes(), before);
    }
}

#[test]
fn a_misnamed_journal_cannot_redirect_chd_undo_to_another_transaction() {
    for json in [false, true] {
        let Some(disc) = Converted::new(ChdConversionSourceMode::QuarantineSource) else {
            return;
        };
        let recorded = disc.journal().join(format!("{}.json", disc.output_id));
        std::fs::copy(&recorded, disc.journal().join("different-request.json")).unwrap();
        let before = disc.journal_bytes();
        let journal_dir = disc.journal();
        let mut args = vec![
            "repair",
            "rollback",
            "--transaction",
            "different-request",
            "--journal-dir",
            journal_dir.to_str().unwrap(),
        ];
        if json {
            args.push("--json");
        }
        let output = run_cli(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("conversion journal identity mismatch"));
        if json {
            let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
            assert_eq!(report["transaction_id"], "different-request");
            assert_eq!(report["phase"], "before_rollback");
            assert_eq!(report["recovery"]["locations_complete"], false);
        }
        assert!(disc.chd_published_intact());
        assert!(disc.originals_quarantined_intact());
        assert!(!disc.originals_in_place());
        assert_eq!(disc.journal_bytes(), before);
    }
}
