//! Added only after independent acceptance of ec370d04's syscall implementation.
//! Process interruption characterization, NOT automatic recovery/rename completion.
use super::*;
use crate::dat::rename_apply::{
    RenameTransaction, TransactionState, journal_path, read_journal, write_journal,
};
use serde_json::{Value, json};

fn receipt(root: &Path) -> PathBuf {
    journal_path(&root.join("journals"), "fd-probe").unwrap()
}

fn save(root: &Path, tx: &RenameTransaction) {
    write_journal(&root.join("journals"), tx).unwrap();
    File::open(receipt(root)).unwrap().sync_all().unwrap();
    File::open(root.join("journals"))
        .unwrap()
        .sync_all()
        .unwrap();
}

fn initialize(f: &Fixture) {
    fs::create_dir(f.root.join("journals")).unwrap();
    for path in [
        f.source(),
        f.root.join("e.bin"),
        f.root.join("holding"),
        f.root.join("destination"),
        f.root.join("journals"),
        f.root.clone(),
    ] {
        File::open(path).unwrap().sync_all().unwrap();
    }
    // Reuse the existing transaction representation and unknown-field roundtrip.
    // No new authoritative log, terminal state, approval or resume grant.
    let tx: RenameTransaction = serde_json::from_value(json!({
        "transaction_id":"fd-probe", "plan_generation":1, "created_at_unix":1,
        "source_scan_root":f.root, "state":"rolling_back",
        "entries":[{"source_path":f.target(),"destination_path":f.source(),
            "original_basename":"s.bin","proposed_basename":"object",
            "identity":f.a,"state":"rolling_back"}],
        "fd_publication_probe":{"phase":"intent", "holding":f.root.join("holding/retired")}
    }))
    .unwrap();
    save(&f.root, &tx);
}

fn record(root: &Path, phase: &str) {
    let mut tx = read_journal(&receipt(root)).unwrap();
    tx.unknown.get_mut("fd_publication_probe").unwrap()["phase"] = json!(phase);
    assert_eq!(tx.state, TransactionState::RollingBack);
    save(root, &tx);
}

fn stop_at(requested: &str, boundary: &str) {
    if requested == boundary {
        // SAFETY: test subprocess only; deliberately skip destructors/flushing.
        // This models process loss, NOT a hardware power cut.
        unsafe { libc::_exit(73) }
    }
}

#[test]
fn mutation_worker() {
    let Some(root) = std::env::var_os("EMUWIZ_FDP_PROCESS_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let stop = std::env::var("EMUWIZ_FDP_STOP").unwrap();
    let tx = read_journal(&receipt(&root)).unwrap();
    stop_at(&stop, "intent");
    let e = &tx.entries[0];
    let p = Publisher::open(&root, &e.destination_path, &e.source_path, &e.identity).unwrap();
    link_descriptor(&p.file, &p.destination_dir, &p.destination_name).unwrap();
    stop_at(&stop, "link_before_sync");
    p.file.sync_all().unwrap();
    p.dirs.sync_and_verify(false).unwrap();
    stop_at(&stop, "link_synced_before_record");
    assert!(identity_matches(
        &e.identity,
        &capture_identity(&p.dirs.destination_entry()).unwrap()
    ));
    record(&root, "linked");
    stop_at(&stop, "linked_recorded");
    let held = root.join("holding/retired");
    let dirs = RenameDirs::open_in(
        &e.destination_path,
        &held,
        &TrustedRoots::from_paths([&root]),
    )
    .unwrap();
    dirs.rename_noreplace(false).unwrap();
    stop_at(&stop, "capture_before_record");
    dirs.sync_and_verify(false).unwrap();
    record(&root, "captured");
    stop_at(&stop, "captured_recorded");
    panic!("unknown interruption boundary");
}

fn observe(root: &Path) -> io::Result<Value> {
    let tx = read_journal(&receipt(root)).map_err(io::Error::other)?;
    if tx.state != TransactionState::RollingBack || tx.entries.len() != 1 {
        return Err(io::Error::other("unexpected transaction state"));
    }
    let expected = &tx.entries[0].identity;
    let phase = tx
        .unknown
        .get("fd_publication_probe")
        .and_then(|v| v["phase"].as_str())
        .ok_or_else(|| io::Error::other("missing phase evidence"))?;
    if !["intent", "linked", "captured"].contains(&phase)
        || tx.entries[0].source_path != root.join("destination/s.bin")
        || tx.entries[0].destination_path != root.join("holding/object")
        || tx.unknown["fd_publication_probe"]["holding"] != json!(root.join("holding/retired"))
    {
        return Err(io::Error::other("inconsistent receipt evidence"));
    }
    let mut names = serde_json::Map::new();
    for name in ["holding/object", "destination/s.bin", "holding/retired"] {
        let observation = match capture_identity(&root.join(name)) {
            Ok(id) => {
                json!({"binding": if identity_matches(expected, &id) {"expected"} else {"foreign"}, "identity":id})
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => json!({"binding":"absent"}),
            Err(_) => json!({"binding":"unavailable"}),
        };
        names.insert(name.into(), observation);
    }
    let b = |name: &str| names[name]["binding"].as_str().unwrap();
    let classification = match (
        b("holding/object"),
        b("destination/s.bin"),
        b("holding/retired"),
    ) {
        ("expected", "absent", "absent") if phase == "intent" => {
            "source_present_publication_not_observed"
        }
        ("expected", "expected", "absent") => "destination_matches_source_alias_retained",
        ("absent", "expected", "expected") => "destination_matches_retirement_anchor_retained",
        ("absent", "expected", "foreign") => "foreign_holding_requires_manual_review",
        _ => "inconsistent_or_unavailable",
    };
    // Neither names nor phase establish complete history or restart authority.
    // The lost descriptor is never reconstructed into automatic mutation rights.
    Ok(
        json!({"pid":std::process::id(),"phase":phase,"classification":classification,
        "names":names,"requires_review":true,"complete_undo":false,"resume_authorized":false}),
    )
}

#[test]
fn inspection_worker() {
    let Some(root) = std::env::var_os("EMUWIZ_FDP_PROCESS_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let result = match observe(&root) {
        Ok(v) => v,
        Err(e) => {
            json!({"refused":true,"error":e.to_string(),"requires_review":true,"complete_undo":false,"resume_authorized":false})
        }
    };
    // Report is harness output, not authoritative recovery state.
    fs::write(
        root.join("inspection-report.json"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}

fn interrupt(f: &Fixture, boundary: &str) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{TEST_PREFIX}recovery::mutation_worker"),
        ])
        .env("EMUWIZ_FDP_PROCESS_ROOT", &f.root)
        .env(
            crate::test_environment::ADOPT_ENVIRONMENT,
            crate::test_environment::isolated_root().expect("private test root"),
        )
        .env("EMUWIZ_FDP_STOP", boundary)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
}

fn inspect_fresh(f: &Fixture) -> Value {
    let original = fs::read(receipt(&f.root)).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{TEST_PREFIX}recovery::inspection_worker"),
        ])
        .env("EMUWIZ_FDP_PROCESS_ROOT", &f.root)
        .env(
            crate::test_environment::ADOPT_ENVIRONMENT,
            crate::test_environment::isolated_root().expect("private test root"),
        )
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read(receipt(&f.root)).unwrap(), original);
    let v: Value =
        serde_json::from_slice(&fs::read(f.root.join("inspection-report.json")).unwrap()).unwrap();
    assert_eq!(v["complete_undo"], false);
    assert_eq!(v["resume_authorized"], false);
    assert_eq!(v["requires_review"], true);
    if let Some(pid) = v["pid"].as_u64() {
        assert_ne!(pid, u64::from(std::process::id()));
    }
    println!(
        "FRESH INSPECTION {} {}",
        f.root.display(),
        v["classification"]
    );
    v
}

#[test]
fn six_interruption_boundaries_preserve_nonterminal_evidence_and_aliases() {
    for boundary in [
        "intent",
        "link_before_sync",
        "link_synced_before_record",
        "linked_recorded",
        "capture_before_record",
        "captured_recorded",
    ] {
        let f = Fixture::new(boundary);
        initialize(&f);
        interrupt(&f, boundary);
        let v = inspect_fresh(&f);
        let expected = if boundary == "intent" {
            "source_present_publication_not_observed"
        } else if boundary.starts_with("capture") {
            "destination_matches_retirement_anchor_retained"
        } else {
            "destination_matches_source_alias_retained"
        };
        assert_eq!(v["classification"], expected);
        assert_eq!(
            read_journal(&receipt(&f.root)).unwrap().state,
            TransactionState::RollingBack
        );
        if boundary != "intent" {
            f.assert_a(&f.target());
        }
        f.assert_a(&if boundary.starts_with("capture") {
            f.root.join("holding/retired")
        } else {
            f.source()
        });
    }
}

#[test]
fn corrupt_receipt_refuses_without_reconstructing_success_from_names() {
    let f = Fixture::new("corrupt-receipt-");
    initialize(&f);
    interrupt(&f, "linked_recorded");
    fs::write(receipt(&f.root), b"{truncated").unwrap();
    assert_eq!(inspect_fresh(&f)["refused"], true);
    f.assert_a(&f.target());
    f.assert_a(&f.source());
}

#[test]
fn lost_names_do_not_resurrect_a_lost_descriptor_after_restart() {
    let f = Fixture::new("lost-name-");
    initialize(&f);
    interrupt(&f, "intent");
    fs::remove_file(f.source()).unwrap(); // external fixture deletion
    assert_eq!(
        inspect_fresh(&f)["classification"],
        "inconsistent_or_unavailable"
    );
    assert!(!f.target().exists());
}

#[test]
fn substituted_holding_after_restart_requires_manual_review_and_no_unlink() {
    let f = Fixture::new("restart-holding-");
    initialize(&f);
    interrupt(&f, "captured_recorded");
    let held = f.root.join("holding/retired");
    fs::rename(&held, f.root.join("retained-a")).unwrap();
    fs::rename(f.root.join("e.bin"), &held).unwrap();
    assert_eq!(
        inspect_fresh(&f)["classification"],
        "foreign_holding_requires_manual_review"
    );
    f.assert_a(&f.target());
    f.assert_a(&f.root.join("retained-a"));
    assert!(identity_matches(&f.b, &capture_identity(&held).unwrap()));
}

#[test]
fn external_destination_replacement_after_crash_is_inconsistent() {
    let f = Fixture::new("restart-destination-");
    initialize(&f);
    interrupt(&f, "linked_recorded");
    fs::rename(f.target(), f.root.join("retained-a")).unwrap();
    fs::rename(f.root.join("e.bin"), f.target()).unwrap();
    assert_eq!(
        inspect_fresh(&f)["classification"],
        "inconsistent_or_unavailable"
    );
    f.assert_a(&f.source());
    f.assert_a(&f.root.join("retained-a"));
    assert!(identity_matches(
        &f.b,
        &capture_identity(&f.target()).unwrap()
    ));
}
