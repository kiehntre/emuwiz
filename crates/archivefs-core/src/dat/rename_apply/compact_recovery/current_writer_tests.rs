//! Current-version participation boundary. These are counterexamples, not
//! admission passes. Both subprocess mutations use the real legacy executor.
use super::*;

pub(super) fn legacy_writer(base: &Authority) -> io::Result<Value> {
    let root = Path::new(&base.transaction.source_scan_root);
    let destination = &base.transaction.entries[0].destination_path;
    let saved = root.join("current-writer-saved-original.bin");
    let replacement = root.join("current-writer-replacement.bin");
    fs::write(&replacement, b"current writer replacement")?;
    for (index, source, target) in [(0, destination, &saved), (1, &replacement, destination)] {
        let mut tx = base.transaction.clone();
        tx.transaction_id = format!("current-legacy-{index}");
        tx.entries.truncate(1);
        let entry = &mut tx.entries[0];
        // This fixture deliberately represents an UNENROLLED legacy caller,
        // not an attempted reuse of the other transaction's FD authority.
        for key in [
            "r2_fd_enrolled",
            "r2_fd_anchor_v1",
            "r2_fd_shared_engine_v1",
            "r2_fd_runtime_v1",
        ] {
            entry.unknown.remove(key);
        }
        entry.source_path = source.clone();
        entry.destination_path = target.clone();
        entry.identity = capture_identity(source)?;
        entry.original_basename = source.file_name().unwrap().to_str().unwrap().into();
        entry.proposed_basename = target.file_name().unwrap().to_str().unwrap().into();
        let outcome = executor::apply_transaction(&mut executor::ApplyExecution {
            approved_paths: [source.to_string_lossy().into_owned()].into(),
            current_generation: tx.plan_generation,
            transaction: &mut tx,
            trusted: TrustedRoots::from_paths([root]),
            journal_dir: root.join("current-legacy-journals"),
            hard_conflict_mode: executor::HardConflictMode::AbortAll,
            cancel: &AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameDirectory,
            allow_symlink_source: false,
        })
        .map_err(fail)?;
        if outcome.transaction.state != TransactionState::Applied {
            return Err(fail(format!(
                "legacy writer did not apply: {:?}",
                outcome.transaction.state
            )));
        }
    }
    Ok(json!({"ok": true, "pid": std::process::id()}))
}

#[test]
fn current_legacy_process_bypasses_enrolled_ownership_during_undo() {
    let (f, bridge) = fixture(1);
    bridge.apply(&bridge.load().unwrap()).unwrap();
    let entry = bridge.base.transaction.entries[0].clone();
    let saved = f.root.join("current-writer-saved-original.bin");
    let input = f.home.join("current-writer-request.json");
    let output = f.home.join("current-writer-result.json");
    fs::write(
        &input,
        serde_json::to_vec(&f.request("current-legacy-writer", None)).unwrap(),
    )
    .unwrap();
    rollback::BEFORE_REVERSE_RENAME.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "dat::rename_apply::compact_recovery::storage::integration::tests::worker",
                ])
                .env("EMUWIZ_R2_REQUEST", input)
                .env("EMUWIZ_R2_RESPONSE", &output)
                .status()
                .unwrap();
            assert!(status.success());
            let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            assert_eq!(result["ok"], true, "{result}");
            assert_ne!(
                result["pid"].as_u64().unwrap(),
                u64::from(std::process::id())
            );
        }));
    });
    let outcome = bridge
        .undo(&bridge.load().unwrap(), &AtomicBool::new(false))
        .unwrap();
    assert!(matches!(
        outcome.result,
        RollbackResult::RollbackFailed { .. }
    ));
    assert_eq!(outcome.transaction.state, TransactionState::RollbackFailed);
    assert_eq!(bridge.load().unwrap().transaction, outcome.transaction);
    assert_eq!(
        fs::read(&entry.source_path).unwrap(),
        b"current writer replacement"
    );
    assert_eq!(capture_identity(&saved).unwrap(), entry.identity);
    assert!(bridge.release().is_err());
    drop(bridge);
    assert!(f.claims() > 0);
    println!(
        "CURRENT UNENROLLED EMUWIZ: NOT PREVENTED; real legacy Apply bypassed admission, wrong-object Undo detected after mutation; ownership retained"
    );
}
