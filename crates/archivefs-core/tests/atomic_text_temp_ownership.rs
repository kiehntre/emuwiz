//! Public-API preservation regressions. The child is a fresh process so the
//! original publisher's first PID/counter temporary name is deterministic.
#![cfg(unix)]

archivefs_core::install_test_environment!();

use archivefs_core::dat::rename_apply::{journal, model::RenameTransaction};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;

fn snapshot(path: &Path) -> Value {
    match fs::symlink_metadata(path) {
        Ok(meta) => json!({
            "exists": true,
            "device": meta.dev(), "inode": meta.ino(), "links": meta.nlink(),
            "mode": meta.mode(), "size": meta.len(),
            "mtime": [meta.mtime(), meta.mtime_nsec()],
            "ctime": [meta.ctime(), meta.ctime_nsec()],
            "symlink": meta.file_type().is_symlink(),
            "link_target": if meta.file_type().is_symlink() {
                Some(fs::read_link(path).unwrap().display().to_string())
            } else { None },
            "bytes": if meta.is_file() { Some(fs::read(path).unwrap()) } else { None },
        }),
        Err(error) => json!({"exists": false, "error": format!("{:?}", error.kind())}),
    }
}

#[test]
fn fixture_child() {
    let Some(case) = std::env::var_os("EMUWIZ_ATOMIC_CHILD_CASE") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("EMUWIZ_ATOMIC_FIXTURE").unwrap());
    let case = case.to_str().unwrap();
    let temporary = root.join(format!(
        ".archivefs-config-write-{}-0.tmp",
        std::process::id()
    ));
    let unrelated = root.join("unrelated.txt");
    let destination = root.join(if case == "journal" {
        "transaction.json"
    } else {
        "config.toml"
    });
    fs::write(&destination, b"original destination\n").unwrap();
    fs::write(&unrelated, b"unrelated sentinel\n").unwrap();
    match case {
        "regular" | "journal" => fs::write(&temporary, b"foreign temporary sentinel\n").unwrap(),
        "symlink" => symlink(&unrelated, &temporary).unwrap(),
        "hardlink" => fs::hard_link(&unrelated, &temporary).unwrap(),
        "failed_creation" => {
            assert_ne!(
                unsafe { libc::geteuid() },
                0,
                "requires an unprivileged test user"
            );
            fs::write(&temporary, b"read-only foreign temporary\n").unwrap();
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o444)).unwrap();
        }
        _ => panic!("unknown fixture"),
    }
    let before = json!({
        "temporary": snapshot(&temporary), "unrelated": snapshot(&unrelated),
        "destination": snapshot(&destination),
    });
    let result = if case == "journal" {
        let transaction = RenameTransaction {
            transaction_id: "transaction".into(),
            plan_generation: 1,
            classifier_version: None,
            created_at_unix: 1,
            source_scan_root: root.display().to_string(),
            state: Default::default(),
            entries: Vec::new(),
            created_directories: Vec::new(),
            recovery_resolution: None,
            recovery_resolved_at_unix: None,
            unknown: BTreeMap::from([("synthetic_marker".into(), json!("preserved"))]),
        };
        journal::write_journal(&root, &transaction)
    } else {
        archivefs_core::save_source_folder_configs_to(
            &destination,
            &[],
            &root.join("mounts"),
            "synthetic-ratarmount",
            None,
        )
    };
    let evidence = json!({
        "case": case, "uid": unsafe { libc::geteuid() },
        "before": before,
        "after": {
            "temporary": snapshot(&temporary), "unrelated": snapshot(&unrelated),
            "destination": snapshot(&destination),
        },
        "success": result.is_ok(), "outcome": format!("{result:?}"),
    });
    println!("ATOMIC_EVIDENCE {evidence}");
}

fn run_case(case: &str) {
    let fixture = tempfile::Builder::new()
        .prefix("emuwiz-atomic-collision-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fixture_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("EMUWIZ_ATOMIC_CHILD_CASE", case)
        .env("EMUWIZ_ATOMIC_FIXTURE", fixture.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let evidence_line = stdout
        .lines()
        .find_map(|line| {
            line.find("ATOMIC_EVIDENCE ")
                .map(|offset| &line[offset + "ATOMIC_EVIDENCE ".len()..])
        })
        .expect("child emitted no evidence");
    let evidence: Value = serde_json::from_str(evidence_line).unwrap();
    println!("ATOMIC_EVIDENCE {evidence}");
    assert_eq!(
        evidence["before"]["temporary"], evidence["after"]["temporary"],
        "pre-existing temporary object was changed or removed"
    );
    assert_eq!(
        evidence["before"]["unrelated"], evidence["after"]["unrelated"],
        "unrelated object was changed"
    );
    assert_eq!(
        evidence["success"], true,
        "a legacy temporary name must not prevent legitimate publication"
    );
    assert_eq!(evidence["after"]["destination"]["symlink"], false);
    if case == "journal" {
        let transaction = journal::read_journal(&fixture.path().join("transaction.json")).unwrap();
        assert_eq!(transaction.unknown["synthetic_marker"], "preserved");
    } else {
        assert!(archivefs_core::Config::load_from(fixture.path().join("config.toml")).is_ok());
    }
}

#[test]
fn existing_regular_temporary_is_preserved() {
    run_case("regular");
}
#[test]
fn existing_symlink_and_unrelated_target_are_preserved() {
    run_case("symlink");
}
#[test]
fn existing_hardlink_and_unrelated_inode_are_preserved() {
    run_case("hardlink");
}
#[test]
fn failed_legacy_stage_creation_does_not_delete_foreign_entry() {
    run_case("failed_creation");
}
#[test]
fn representative_journal_preserves_foreign_temporary() {
    run_case("journal");
}
