use super::*;
use crate::dat::rename_apply::{identity::capture_identity, journal};
use ring::{rand::SystemRandom, signature::Ed25519KeyPair};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

struct Fixture {
    home: PathBuf,
    root: PathBuf,
    original: Vec<u8>,
    config: admission::Configuration,
    issuer: approval::Issuer,
    ticket: Ticket,
}
fn transaction(root: &Path, n: usize) -> RenameTransaction {
    let entries = (0..n)
        .map(|i| {
            let source = root.join(format!("source-{i}-é.bin"));
            let destination = root.join(format!("destination-{i}-é.bin"));
            fs::write(&source, format!("original bytes {i}")).unwrap();
            TransactionEntry {
                identity: capture_identity(&source).unwrap(),
                original_basename: source.file_name().unwrap().to_str().unwrap().into(),
                proposed_basename: destination.file_name().unwrap().to_str().unwrap().into(),
                source_path: source,
                destination_path: destination,
                operation: TransactionOperation::RenameMove,
                preflight_passed: false,
                preflight_failures: vec![],
                state: EntryState::Planned,
                failure_reason: None,
                applied_at_unix: None,
                rolled_back_at_unix: None,
                unknown: BTreeMap::new(),
            }
        })
        .collect();
    RenameTransaction {
        transaction_id: "enrolled".into(),
        plan_generation: 1,
        classifier_version: Some(crate::dat::classification::CLASSIFIER_VERSION.into()),
        created_at_unix: 1,
        source_scan_root: root.to_str().unwrap().into(),
        state: TransactionState::Planned,
        entries,
        created_directories: vec![],
        recovery_resolution: None,
        recovery_resolved_at_unix: None,
        unknown: BTreeMap::from([("opaque".into(), json!("z".repeat(300_000)))]),
    }
}
fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn fixture(n: usize) -> (Fixture, Rc<Bridge>) {
    fixture_enrolled(n, false)
}
fn fixture_enrolled(n: usize, fd: bool) -> (Fixture, Rc<Bridge>) {
    fixture_mode(n, fd, fd)
}
fn fixture_mode(n: usize, fd: bool, anchor: bool) -> (Fixture, Rc<Bridge>) {
    fixture_engine_mode(n, fd, anchor, false)
}
fn fixture_engine_mode(n: usize, fd: bool, anchor: bool, engine: bool) -> (Fixture, Rc<Bridge>) {
    let home = tempfile::Builder::new()
        .prefix("emuwiz-r2-enrolled-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
        .keep();
    let root = home.join("files");
    private_dir(&root);
    let auth = home.join("admission");
    private_dir(&auth);
    let signer = home.join("issuer");
    private_dir(&signer);
    let config = admission::Configuration::create(&auth).unwrap();
    let mut tx = transaction(&root, n);
    if fd {
        for entry in &mut tx.entries {
            entry.unknown.insert("r2_fd_enrolled".into(), json!(1));
            if engine {
                entry
                    .unknown
                    .insert("r2_fd_shared_engine_v1".into(), json!(true));
            }
            if anchor {
                entry.unknown.insert("r2_fd_anchor_v1".into(), json!(true));
            }
        }
    }
    let original = serde_json::to_vec(&tx).unwrap();
    let base = Authority::decode_legacy(&original, 1024 * 1024).unwrap();
    let b = Bridge::create(&root, base, &config).unwrap();
    let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let issuer = approval::Issuer::new(
        &signer,
        original.clone(),
        b.store.ticket.clone(),
        key.as_ref(),
    )
    .unwrap();
    let f = Fixture {
        home,
        root,
        original,
        config,
        issuer,
        ticket: b.store.ticket.clone(),
    };
    (f, b)
}
impl Fixture {
    fn base(&self) -> Authority {
        Authority::decode_legacy(&self.original, 1024 * 1024).unwrap()
    }
    fn issue(&self, action: RequestedAction) -> approval::Approval {
        let base = self.base();
        let store = Store::reopen(&self.ticket, &base, CAP).unwrap();
        let context = self.issuer.context(&store, &base).unwrap();
        drop(store);
        let b =
            Bridge::reopen_trusted(&self.issuer.verifier, &context, base, &self.config).unwrap();
        self.issuer
            .issue(
                &context,
                action,
                &serde_json::to_vec(&b.observations().unwrap()).unwrap(),
            )
            .unwrap()
    }
    fn request(&self, mode: &str, grant: Option<&approval::Approval>) -> Value {
        json!({"mode":mode,"original":self.original,"authority":self.config,"verifier":self.issuer.verifier,"approval":grant})
    }
    fn child(&self, request: Value) -> (Output, Option<Value>) {
        let input = self.home.join("request.json");
        let output = self.home.join("response.json");
        if output.exists() {
            fs::rename(
                &output,
                self.home.join(format!(
                    "response-{}.json",
                    crate::dat::rename_apply::journal::new_transaction_id(0)
                )),
            )
            .unwrap();
        }
        fs::write(&input, serde_json::to_vec(&request).unwrap()).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "dat::rename_apply::compact_recovery::storage::integration::tests::worker",
                "--nocapture",
            ])
            .env("EMUWIZ_R2_REQUEST", &input)
            .env("EMUWIZ_R2_RESPONSE", &output);
        if let Some(edge) = request["exit"].as_str() {
            command.env("EMUWIZ_R2_EXIT", edge);
        }
        if let Some(edge) = request["fd_exit"].as_str() {
            command.env("EMUWIZ_CONTRACT_EXIT", edge);
        }
        if request["fd_action"].is_string() {
            command.env("EMUWIZ_CONTRACT_ACTION_REQUEST", &input);
        }
        let out = command.output().unwrap();
        let result = fs::read(&output)
            .ok()
            .map(|b| serde_json::from_slice(&b).unwrap());
        (out, result)
    }
    fn run(&self, request: Value) -> Value {
        let (out, result) = self.child(request);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        result.unwrap()
    }
    fn restored(&self) {
        for (i, e) in self.base().transaction.entries.iter().enumerate() {
            assert_eq!(
                fs::read(&e.source_path).unwrap(),
                format!("original bytes {i}").as_bytes()
            );
            assert_eq!(capture_identity(&e.source_path).unwrap(), e.identity);
            assert!(!e.destination_path.exists());
        }
    }
    fn image(&self) -> Vec<u8> {
        fs::read(self.root.join(NAMESPACE).join(LEAF)).unwrap()
    }
    fn claims(&self) -> i64 {
        let guard = self.config.acquire().unwrap();
        drop(guard);
        let db = rusqlite::Connection::open(self.home.join("admission/owners.sqlite")).unwrap();
        db.query_row("SELECT count(*) FROM claims", [], |r| r.get(0))
            .unwrap()
    }
}
fn decode<T: serde::de::DeserializeOwned>(v: &Value) -> io::Result<T> {
    serde_json::from_value(v.clone()).map_err(fail)
}
#[path = "current_writer_tests.rs"]
mod current_writers;

fn worker_run(r: Value) -> io::Result<Value> {
    let original: Vec<u8> = decode(&r["original"])?;
    let base = Authority::decode_legacy(&original, 1024 * 1024).map_err(protocol)?;
    let root = PathBuf::from(&base.transaction.source_scan_root);
    let config: admission::Configuration = decode(&r["authority"])?;
    let mode = r["mode"].as_str().unwrap();
    if mode == "current-legacy-writer" {
        return current_writers::legacy_writer(&base);
    }
    if mode == "held-writer" {
        return production_boundaries::held_writer(&base, &r);
    }
    if mode == "outside-writer" {
        let entry = &base.transaction.entries[0];
        fs::rename(
            &entry.destination_path,
            root.join("outside-writer-saved-original.bin"),
        )?;
        fs::write(&entry.destination_path, b"outside writer replacement")?;
        return Ok(json!({"ok":true}));
    }
    if mode == "fd-external-action" {
        let entry = &base.transaction.entries[0];
        match r["fd_action"].as_str() {
            Some("delete-destination") => fs::remove_file(&entry.source_path)?,
            Some("foreign-source") => {
                fs::rename(&entry.source_path, root.join("external-original.bin"))?;
                fs::write(&entry.source_path, b"foreign B")?;
            }
            Some("foreign-old") => {
                fs::rename(&entry.destination_path, root.join("external-original.bin"))?;
                fs::write(&entry.destination_path, b"foreign B")?;
            }
            _ => return Err(fail("unknown external fixture action")),
        }
        return Ok(json!({"ok":true}));
    }
    if mode == "admit" {
        let b = Bridge::create(&root, base, &config)?;
        return Ok(json!({"ok":true,"tx":b.load()?.transaction}));
    }
    if mode == "inspect" {
        let tx =
            journal::read_journal(&journal::journal_path(&root, base.transaction_id()).unwrap())
                .map_err(fail)?;
        let (list, problems) = journal::list_journals(&root);
        if !problems.is_empty() {
            return Err(fail(format!("discovery: {problems:?}")));
        }
        let (recovery, _) = journal::find_recovery_transactions(&root);
        let (undoable, _) = journal::find_rollbackable_transactions(&root);
        let summary = TransactionSummary::from_transaction(&tx);
        return Ok(
            json!({"ok":true,"tx":tx,"discovered":list.len(),"recovery":recovery.len(),"undoable":undoable.len(),"summary":format!("{summary:?}"),"assessment":format!("{:?}",crate::platform_evidence_fusion::plan_transaction::assess_recovery(&tx,&[]))}),
        );
    }
    let trusted: approval::Verifier = decode(&r["verifier"])?;
    let grant: approval::Approval = if r["approval"].is_null() {
        return Err(fail("genuine signed restart approval required"));
    } else {
        decode(&r["approval"])?
    };
    if r["public_controller"] == true {
        let action = if mode == "apply" {
            RequestedAction::Apply
        } else {
            RequestedAction::Undo
        };
        let controller = controller::Controller::reopen(
            base.original_bytes(),
            &config,
            &trusted,
            &grant,
            action,
            controller::RuntimeSwitch::enabled_for_evaluation(),
        )?;
        let (tx, success) = if mode == "apply" {
            let tx = controller.transaction()?;
            let approved = tx
                .entries
                .iter()
                .map(|e| e.source_path.to_string_lossy().into_owned())
                .collect();
            let out = controller.apply(approved, tx.plan_generation, &AtomicBool::new(false))?;
            let success = out.transaction.state == TransactionState::Applied;
            (out.transaction, success)
        } else {
            let out = controller.undo(&AtomicBool::new(false))?;
            let success = matches!(out.result, RollbackResult::FullyRolledBack);
            (out.transaction, success)
        };
        if tx != controller.transaction()? {
            return Err(fail("public outcome differs from persisted recovery"));
        }
        return Ok(json!({"ok":true,"success":success,"tx":tx}));
    }
    let b = Bridge::reopen_trusted(&trusted, &grant.context, base, &config)?;
    b.engine_enabled.set(r["fd_engine"] == true);
    if !r["receipt"].is_null() {
        b.verify(&decode(&r["receipt"])?)?;
    }
    let action = if mode == "apply" {
        RequestedAction::Apply
    } else {
        RequestedAction::Undo
    };
    b.accept_approval(&grant, action)?;
    if mode == "consume" {
        return Ok(json!({"ok":true,"consumed":true}));
    }
    if let Some(fault) = r["fault"].as_array() {
        *b.fault.borrow_mut() = Some((
            decode(&fault[0])?,
            fault[1].as_u64().unwrap() as usize,
            fault[2].as_bool().unwrap(),
        ));
    }
    if let Some(point) = r["checkpoint"].as_array() {
        *b.checkpoint_fault.borrow_mut() = Some((
            point[0].as_u64().unwrap() as usize,
            point[1].as_bool().unwrap(),
        ));
    }
    if let Some(event) = r["publication"].as_str() {
        let event = match event {
            "write" => Event::RecordWrite,
            "sync" => Event::RecordSync,
            "commit" => Event::RecordCommit,
            _ => Event::RecordCommitSync,
        };
        *b.publication_fault.borrow_mut() =
            Some((Fact::ApplyComplete, PublicationFault::Exit(event)));
    }
    if let Some(fault) = r["io"].as_str() {
        *b.publication_fault.borrow_mut() = Some((
            Fact::ApplyComplete,
            if fault == "short" {
                PublicationFault::Short(13)
            } else {
                PublicationFault::Fail(Event::RecordWrite)
            },
        ));
    }
    if r["sync_fail"] == true {
        crate::dat::rename_apply::dirsync::set_test_hook(Some(Box::new(|event| match event {
            crate::dat::rename_apply::dirsync::DirSyncEvent::Before(_) => {
                Err(io::Error::from_raw_os_error(libc::EIO))
            }
            _ => Ok(()),
        })));
    }
    let cancel = std::sync::Arc::new(AtomicBool::new(r["cancel"] == true));
    if let Some(n) = r["cancel_after"].as_u64() {
        let flag = cancel.clone();
        rollback::AFTER_REVERSAL.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |done| {
                if done == n as usize {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }))
        });
    }
    let receipt = b.load()?;
    let (tx, success, result) = if mode == "apply" {
        let out = b.apply(&receipt)?;
        let success = out.transaction.state == TransactionState::Applied;
        (out.transaction, success, "apply".to_string())
    } else {
        let out = b.undo(&receipt, &cancel)?;
        let success = matches!(out.result, RollbackResult::FullyRolledBack);
        (out.transaction, success, format!("{:?}", out.result))
    };
    let persisted = b.load()?.transaction;
    if tx != persisted {
        return Err(fail(
            "core outcome and persisted compact transaction differ",
        ));
    }
    let runtime = format!("{:?}", b.runtime());
    if mode == "undo" && success && r["release"] != false {
        b.release()?;
    }
    Ok(json!({"ok":true,"success":success,"tx":tx,"result":result,"runtime":runtime}))
}
#[test]
fn worker() {
    let Some(path) = std::env::var_os("EMUWIZ_R2_REQUEST") else {
        return;
    };
    let r = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let result = worker_run(r).unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
    fs::write(
        std::env::var_os("EMUWIZ_R2_RESPONSE").unwrap(),
        serde_json::to_vec(&result).unwrap(),
    )
    .unwrap();
}
#[path = "production_boundary_tests.rs"]
mod production_boundaries;

#[test]
fn active_round_trip_exact_current_readers_and_release() {
    let (f, b) = fixture(3);
    let applied = b.apply(&b.load().unwrap()).unwrap().transaction;
    assert_eq!(applied, b.load().unwrap().transaction);
    assert!(b.store.scan(&b.base).unwrap().count > 3);
    for (i, e) in applied.entries.iter().enumerate() {
        assert_eq!(
            fs::read(&e.destination_path).unwrap(),
            format!("original bytes {i}").as_bytes()
        );
        assert_eq!(capture_identity(&e.destination_path).unwrap(), e.identity);
        assert!(e.applied_at_unix.is_some());
    }
    drop(b);
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], serde_json::to_value(&applied).unwrap());
    assert_eq!(read["discovered"], 1);
    assert_eq!(read["undoable"], 1);
    let grant = f.issue(RequestedAction::Undo);
    let done = f.run(f.request("undo", Some(&grant)));
    assert_eq!(done["success"], true, "{done}");
    assert_eq!(done["tx"]["state"], "rolled_back");
    f.restored();
    assert_eq!(f.claims(), 0);
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], done["tx"]);
    assert_eq!(read["recovery"], 0);
    assert_eq!(read["undoable"], 0);
}
fn finish(f: &Fixture) {
    let grant = f.issue(RequestedAction::Undo);
    let done = f.run(f.request("undo", Some(&grant)));
    assert_eq!(done["success"], true, "{done}");
    assert_eq!(done["tx"]["state"], "rolled_back");
    f.restored();
    assert_eq!(f.claims(), 0);
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], done["tx"]);
    assert_eq!(read["recovery"], 0);
}
#[test]
fn interrupted_multifile_apply_and_undo_fresh_processes() {
    for (state, after) in [
        (EntryState::Applying, true),
        (EntryState::Applied, false),
        (EntryState::Applied, true),
        (EntryState::RollingBack, true),
        (EntryState::RolledBack, false),
        (EntryState::RolledBack, true),
    ] {
        let (f, b) = fixture(3);
        let undo = matches!(state, EntryState::RollingBack | EntryState::RolledBack);
        if undo {
            b.apply(&b.load().unwrap()).unwrap();
        }
        drop(b);
        let grant = f.issue(if undo {
            RequestedAction::Undo
        } else {
            RequestedAction::Apply
        });
        let mut request = f.request(if undo { "undo" } else { "apply" }, Some(&grant));
        request["fault"] = json!([state, 1, after]);
        let (out, _) = f.child(request);
        assert_eq!(
            out.status.code(),
            Some(73),
            "{state:?} {after} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(f.claims() > 0);
        let read = f.run(f.request("inspect", None));
        assert_eq!(read["ok"], true, "{read}");
        assert_ne!(read["assessment"], "SafeToResume");
        assert_ne!(read["tx"]["state"], "planned");
        finish(&f);
    }
}
#[test]
fn checkpoint_publication_and_acknowledgement_interruptions() {
    for number in [1, 2, 3, 4, 6, 9] {
        for after in [false, true] {
            let (f, b) = fixture(3);
            drop(b);
            let grant = f.issue(RequestedAction::Apply);
            let mut request = f.request("apply", Some(&grant));
            request["checkpoint"] = json!([number, after]);
            let (out, _) = f.child(request);
            assert_eq!(out.status.code(), Some(76), "checkpoint {number} {after}");
            assert!(f.claims() > 0);
            finish(&f);
        }
    }
    for event in ["write", "sync", "commit", "ack"] {
        let (f, b) = fixture(3);
        drop(b);
        let grant = f.issue(RequestedAction::Apply);
        let mut request = f.request("apply", Some(&grant));
        request["publication"] = json!(event);
        let (out, _) = f.child(request);
        assert_eq!(out.status.code(), Some(77));
        assert!(f.claims() > 0);
        let read = f.run(f.request("inspect", None));
        if matches!(event, "write" | "sync") {
            assert_eq!(read["ok"], false);
            assert!(
                read["error"]
                    .as_str()
                    .unwrap()
                    .contains("incomplete compact")
            );
        }
        finish(&f);
    }
}
fn refused_unchanged(f: &Fixture, request: Value, reason: &str) {
    let before = f.image();
    let claims = f.claims();
    let outcome = f.run(request);
    assert_eq!(outcome["ok"], false, "{outcome}");
    assert!(
        outcome["error"].as_str().unwrap().contains(reason),
        "wanted {reason}: {outcome}"
    );
    assert_eq!(f.image(), before);
    assert_eq!(f.claims(), claims);
}
#[test]
fn signed_refusals_reach_the_intended_checks() {
    let (f, b) = fixture(2);
    let stale = b.load().unwrap();
    b.apply(&stale).unwrap();
    drop(b);
    refused_unchanged(&f, f.request("undo", None), "genuine signed");
    let grant = f.issue(RequestedAction::Undo);
    let mut forged = f.request("undo", Some(&grant));
    let sig = forged["approval"]["context"]["signed"]["signature"]
        .as_array_mut()
        .unwrap();
    sig[0] = json!(sig[0].as_u64().unwrap() ^ 1);
    refused_unchanged(&f, forged, "signature invalid");
    let mut wrong = f.request("undo", Some(&grant));
    wrong["approval"]["context"]["verifier"]["public_key"] = json!(vec![0; 32]);
    refused_unchanged(&f, wrong, "cannot select its own issuer");
    let mut old = f.request("undo", Some(&grant));
    old["receipt"] = serde_json::to_value(stale).unwrap();
    refused_unchanged(&f, old, "stale complete receipt");
    let substituted = f.issue(RequestedAction::Apply);
    refused_unchanged(
        &f,
        f.request("undo", Some(&substituted)),
        "differs from requested evidence/action/identity",
    );
    // Consume without mutation: the image still matches. A replay therefore
    // reaches the durable tombstone check instead of an unrelated stale check.
    let consumed = f.run(f.request("consume", Some(&grant)));
    assert_eq!(consumed["ok"], true, "{consumed}");
    refused_unchanged(&f, f.request("undo", Some(&grant)), "already consumed");
    finish(&f);
}
#[test]
fn stale_signed_boundary_and_receipt_substitution_preserve_state() {
    let (f, b) = fixture(2);
    drop(b);
    let old = f.issue(RequestedAction::Undo);
    let apply = f.issue(RequestedAction::Apply);
    let result = f.run(f.request("apply", Some(&apply)));
    assert_eq!(result["success"], true, "{result}");
    refused_unchanged(
        &f,
        f.request("undo", Some(&old)),
        "evidence differs from independently issued boundary",
    );
    finish(&f);
}
#[test]
fn consumption_and_release_interruptions_keep_defined_ownership() {
    for point in [
        "before-consumption",
        "after-consumption",
        "before-release",
        "after-release",
    ] {
        let (f, b) = fixture(2);
        b.apply(&b.load().unwrap()).unwrap();
        drop(b);
        let grant = f.issue(RequestedAction::Undo);
        let mut req = f.request("undo", Some(&grant));
        req["exit"] = json!(point);
        let (out, _) = f.child(req);
        assert_eq!(out.status.code(), Some(79));
        if point == "after-release" {
            f.restored();
            assert_eq!(f.claims(), 0);
            continue;
        }
        assert!(f.claims() > 0);
        if point == "after-consumption" {
            refused_unchanged(&f, f.request("undo", Some(&grant)), "already consumed");
        }
        finish(&f);
    }
}
#[test]
fn cancellation_and_nested_rollback_failure_never_release_or_report_success() {
    let (f, b) = fixture(3);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let mut request = f.request("undo", Some(&grant));
    request["cancel"] = json!(true);
    let result = f.run(request);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["success"], false);
    assert_eq!(result["tx"]["state"], "rollback_failed");
    assert!(f.claims() > 0);
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], result["tx"]);
    assert!(read["summary"].as_str().unwrap().contains("RollbackFailed"));
    finish(&f);
}
#[test]
fn injected_directory_sync_failure_keeps_exact_reason_and_inflight_state() {
    let (f, b) = fixture(2);
    drop(b);
    let grant = f.issue(RequestedAction::Apply);
    let mut request = f.request("apply", Some(&grant));
    request["sync_fail"] = json!(true);
    let result = f.run(request);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["success"], false);
    assert_eq!(result["tx"]["state"], "apply_failed");
    assert_eq!(result["tx"]["entries"][0]["state"], "applying");
    assert!(
        result["tx"]["entries"][0]["failure_reason"]
            .as_str()
            .unwrap()
            .contains("sync")
    );
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], result["tx"]);
    assert_ne!(read["assessment"], "SafeToResume");
    assert!(f.claims() > 0);
    finish(&f);
}
#[test]
fn short_writes_and_sync_errors_retain_uncertainty_without_disk_filling() {
    for kind in ["short", "sync"] {
        let (f, b) = fixture(2);
        drop(b);
        let grant = f.issue(RequestedAction::Apply);
        let mut req = f.request("apply", Some(&grant));
        req["io"] = json!(kind);
        let result = f.run(req);
        assert_eq!(result["ok"], false, "{result}");
        assert!(f.claims() > 0);
        let inspect = f.run(f.request("inspect", None));
        assert_eq!(inspect["ok"], false);
        assert!(inspect["error"].as_str().unwrap().contains("incomplete"));
        if kind == "sync" {
            finish(&f);
        } else {
            let undo = f.issue(RequestedAction::Undo);
            refused_unchanged(
                &f,
                f.request("undo", Some(&undo)),
                "exact runtime values unavailable",
            );
        }
    }
}
#[test]
fn legacy_large_metadata_with_active_dispatcher() {
    let (f, b) = fixture(1);
    let _installed = b.installed().unwrap();
    for n in [75, 100] {
        let root = f.home.join(format!("legacy-{n}"));
        private_dir(&root);
        let mut tx = transaction(&root, n);
        tx.transaction_id = format!("legacy-{n}");
        let original = tx.clone();
        let outcome = executor::apply_transaction(&mut executor::ApplyExecution {
            approved_paths: tx
                .entries
                .iter()
                .map(|e| e.source_path.to_string_lossy().into())
                .collect(),
            current_generation: 1,
            trusted: TrustedRoots::from_paths([&root]),
            journal_dir: root.clone(),
            hard_conflict_mode: executor::HardConflictMode::AbortAll,
            cancel: &AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameDirectory,
            allow_symlink_source: false,
            transaction: &mut tx,
        })
        .unwrap();
        assert_eq!(outcome.transaction.state, TransactionState::Applied);
        let path = journal::journal_path(&root, &tx.transaction_id).unwrap();
        assert!(fs::metadata(&path).unwrap().len() > 300_000);
        let mut current = journal::read_journal(&path).unwrap();
        assert_eq!(current, outcome.transaction);
        let undone = rollback::rollback_transaction_confined(
            &mut current,
            &root,
            &AtomicBool::new(false),
            &TrustedRoots::from_paths([&root]),
        )
        .unwrap();
        assert_eq!(undone.result, RollbackResult::FullyRolledBack);
        assert_eq!(journal::read_journal(&path).unwrap(), undone.transaction);
        assert_eq!(undone.transaction.unknown, original.unknown);
        assert!(!root.join(NAMESPACE).exists());
        for (i, e) in original.entries.iter().enumerate() {
            assert_eq!(
                fs::read(&e.source_path).unwrap(),
                format!("original bytes {i}").as_bytes()
            );
            assert_eq!(capture_identity(&e.source_path).unwrap(), e.identity);
            assert!(!e.destination_path.exists());
        }
    }
    assert_eq!(b.store.scan(&b.base).unwrap().count, 0);
    f.restored();
}
#[test]
fn shared_admission_refuses_different_ids_aliases_and_both_name_slots() {
    shared_admission_case(false);
}
fn shared_admission_case(fd: bool) {
    shared_admission_engine_case(fd, false);
}
fn shared_admission_engine_case(fd: bool, engine: bool) {
    let (f, b) = fixture_engine_mode(1, fd, fd, engine);
    b.engine_enabled.set(engine);
    let source = b.base.transaction.entries[0].source_path.clone();
    let mut other = b.base.transaction.clone();
    other.transaction_id = "different-id".into();
    let request = |tx: &RenameTransaction| {
        let mut req = f.request("admit", None);
        req["original"] = json!(serde_json::to_vec(tx).unwrap());
        req
    };
    let busy = f.run(request(&other));
    assert!(
        busy["error"].as_str().unwrap().contains("operation busy"),
        "{busy}"
    );
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    // The two name tests use a distinct inode, so the object claim cannot
    // accidentally stand in for the source/destination slot checks.
    let extra = f.root.join("other.bin");
    fs::write(&extra, b"independent object").unwrap();
    for destination in [&source, &f.base().transaction.entries[0].destination_path] {
        let mut tx = other.clone();
        let e = &mut tx.entries[0];
        e.source_path = extra.clone();
        e.identity = capture_identity(&extra).unwrap();
        e.destination_path = destination.clone();
        let result = f.run(request(&tx));
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("object or name slot owned"),
            "{result}"
        );
        assert_eq!(fs::read(&extra).unwrap(), b"independent object");
    }
    let outside = f.home.join("alias-root");
    private_dir(&outside);
    let alias = outside.join("alias.bin");
    fs::hard_link(&f.base().transaction.entries[0].destination_path, &alias).unwrap();
    let mut tx = other.clone();
    tx.source_scan_root = outside.to_str().unwrap().into();
    tx.entries[0].source_path = alias.clone();
    tx.entries[0].destination_path = outside.join("next.bin");
    let result = f.run(request(&tx));
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("object or name slot owned"),
        "{result}"
    );
    assert!(alias.exists());
    assert!(!outside.join("next.bin").exists());
    if engine {
        let grant = f.issue(RequestedAction::Undo);
        let mut req = f.request("undo", Some(&grant));
        req["fd_engine"] = json!(true);
        assert_eq!(f.run(req)["success"], true);
        f.restored();
        assert_eq!(f.claims(), 0);
    } else {
        finish(&f);
    }
}
thread_local! {static COMPETE:RefCell<Option<Box<dyn FnOnce()>>>=RefCell::new(None);}
fn competing_process(_: &Path) {
    COMPETE.with(|c| c.borrow_mut().take().unwrap()());
}
#[test]
fn admission_is_held_across_actual_revalidation_mutation_and_checkpoint() {
    let (f, b) = fixture(1);
    let mut tx = b.base.transaction.clone();
    tx.transaction_id = "competitor-at-mutation".into();
    let mut request = f.request("admit", None);
    request["original"] = json!(serde_json::to_vec(&tx).unwrap());
    let input = f.home.join("compete.json");
    let output = f.home.join("compete-result.json");
    fs::write(&input, serde_json::to_vec(&request).unwrap()).unwrap();
    COMPETE.with(|c| {
        *c.borrow_mut() = Some(Box::new(move || {
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
            assert!(
                result["error"].as_str().unwrap().contains("operation busy"),
                "{result}"
            );
        }))
    });
    executor::set_test_before_rename_hook(Some(competing_process));
    let result = b.apply(&b.load().unwrap()).unwrap();
    assert_eq!(result.transaction.state, TransactionState::Applied);
    assert_eq!(b.load().unwrap().transaction, result.transaction);
    drop(b);
    finish(&f);
}
#[test]
fn unsupported_operations_refuse_before_admission_or_mutation() {
    let (f, b) = fixture(1);
    drop(b);
    let before = f.image();
    let claims = f.claims();
    for kind in [
        "directories",
        "exact",
        "symlink",
        "cross-directory",
        "too-many",
    ] {
        let mut tx = f.base().transaction;
        match kind {
            "directories" => tx.created_directories.push(f.root.join("created")),
            "exact" => {
                tx.unknown.insert(
                    "emuwiz_exact_resume_envelope".into(),
                    json!({"format_version":1}),
                );
            }
            "symlink" => tx.entries[0].identity.kind = ObjectKind::Symlink,
            "cross-directory" => tx.entries[0].destination_path = f.home.join("outside.bin"),
            _ => tx.entries = vec![tx.entries[0].clone(); 9],
        }
        let base =
            Authority::decode_legacy(&serde_json::to_vec(&tx).unwrap(), 1024 * 1024).unwrap();
        let error = Bridge::create(&f.root, base, &f.config)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("supports new") || error.contains("outside private"),
            "{kind}: {error}"
        );
        assert_eq!(f.image(), before);
        assert_eq!(f.claims(), claims);
        f.restored();
    }
    // No installed bridge may make a legacy writer overwrite a compact marker.
    assert!(
        journal::write_journal(&f.root, &f.base().transaction)
            .unwrap_err()
            .to_string()
            .contains("enrolled checkpoint authority")
    );
    assert_eq!(f.image(), before);
}
#[test]
fn corrupt_truncated_and_contradictory_evidence_is_retained_and_refused() {
    for kind in ["truncated", "digest", "contradiction", "descriptor"] {
        let (f, b) = fixture(2);
        b.apply(&b.load().unwrap()).unwrap();
        let base_end = b.store.layout.base_end;
        drop(b);
        let grant = f.issue(RequestedAction::Undo);
        let path = f.root.join(NAMESPACE).join(LEAF);
        let mut image = f.image();
        match kind {
            "truncated" => {
                OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_len(base_end + 7)
                    .unwrap();
            }
            "digest" => {
                image[base_end as usize + 30] ^= 1;
                fs::write(&path, &image).unwrap();
            }
            "descriptor" => {
                fs::write(
                    journal::journal_path(&f.root, "enrolled").unwrap(),
                    b"{ broken marker",
                )
                .unwrap();
            }
            _ => {
                let start = base_end as usize;
                let length = u64::from_le_bytes(
                    decoded(&image[start..start + 16])
                        .unwrap()
                        .try_into()
                        .unwrap(),
                ) as usize;
                let bytes = decoded(&image[start + 16..start + 16 + length * 2]).unwrap();
                let mut record: Value = serde_json::from_slice(&bytes[..bytes.len() - 32]).unwrap();
                record["runtime"]["state"] = json!("applied");
                let mut bytes = serde_json::to_vec(&record).unwrap();
                bytes.extend(Sha256::digest(&bytes).as_slice());
                let mut payload = encoded(&(bytes.len() as u64).to_le_bytes());
                payload.extend(encoded(&bytes));
                payload.push(COMMITTED);
                image[start..start + SLOT as usize].fill(0);
                image[start..start + payload.len()].copy_from_slice(&payload);
                fs::write(&path, &image).unwrap();
            }
        }
        let retained = f.image();
        let read = f.run(f.request("inspect", None));
        assert_eq!(read["ok"], false, "{kind}: {read}");
        if kind == "contradiction" {
            assert!(
                read["error"]
                    .as_str()
                    .unwrap()
                    .contains("overall state contradicts"),
                "{read}"
            );
        }
        // Signature boundary is independently stale for modified container;
        // descriptor damage alone is still detected by the public reader.
        if kind != "descriptor" {
            let undo = f.run(f.request("undo", Some(&grant)));
            assert_eq!(undo["ok"], false, "{undo}");
        }
        assert_eq!(f.image(), retained);
        assert!(f.claims() > 0);
        for e in &f.base().transaction.entries {
            assert!(!e.source_path.exists());
            assert_eq!(capture_identity(&e.destination_path).unwrap(), e.identity);
        }
    }
}
#[test]
fn nonparticipating_writer_wrong_object_is_explicitly_not_prevented() {
    let (f, b) = fixture(1);
    b.apply(&b.load().unwrap()).unwrap();
    let e = b.base.transaction.entries[0].clone();
    let saved = f.root.join("outside-writer-saved-original.bin");
    let input = f.home.join("unaware-request.json");
    let output = f.home.join("unaware-result.json");
    fs::write(
        &input,
        serde_json::to_vec(&f.request("outside-writer", None)).unwrap(),
    )
    .unwrap();
    rollback::BEFORE_REVERSE_RENAME.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
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
            assert_eq!(result["ok"], true);
        }))
    });
    let result = b.undo(&b.load().unwrap(), &AtomicBool::new(false)).unwrap();
    assert!(matches!(
        result.result,
        RollbackResult::RollbackFailed { .. }
    ));
    assert_eq!(result.transaction.state, TransactionState::RollbackFailed);
    assert_eq!(
        fs::read(&e.source_path).unwrap(),
        b"outside writer replacement"
    );
    assert_eq!(capture_identity(&saved).unwrap(), e.identity);
    assert_eq!(b.load().unwrap().transaction, result.transaction);
    assert!(b.release().is_err());
    drop(b);
    assert!(f.claims() > 0);
    println!(
        "NONPARTICIPATING WRITER: NOT PREVENTED; wrong object moved, failure detected, ownership retained"
    );
}

#[test]
fn partial_undo_and_its_history_are_not_success() {
    let (f, b) = fixture(3);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let mut request = f.request("undo", Some(&grant));
    request["cancel_after"] = json!(1);
    let result = f.run(request);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["success"], false);
    assert!(
        result["result"]
            .as_str()
            .unwrap()
            .starts_with("PartiallyRolledBack")
    );
    assert_eq!(result["tx"]["state"], "rollback_failed");
    assert!(
        result["runtime"]
            .as_str()
            .unwrap()
            .contains("RollbackFailed")
    );
    assert!(f.claims() > 0);
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], result["tx"]);
    assert!(read["summary"].as_str().unwrap().contains("RollbackFailed"));
    finish(&f);
}
#[test]
fn forged_grant_record_is_rejected_by_signature_check_itself() {
    let (f, b) = fixture(1);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let path = f
        .issuer
        .verifier
        .directory
        .join(format!("grant-{}", grant.nonce));
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let sig = record["signature"].as_array_mut().unwrap();
    sig[0] = json!(sig[0].as_u64().unwrap() ^ 1);
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    refused_unchanged(&f, f.request("undo", Some(&grant)), "signature invalid");
    finish(&f);
}

#[test]
fn legacy_opaque_marker_text_and_foreign_ids_are_not_intercepted() {
    let (f, b) = fixture(1);
    let _installed = b.installed().unwrap();
    let mut tx = b.base.transaction.clone();
    tx.transaction_id = "legacy-in-same-directory".into();
    tx.unknown.insert(
        "opaque-marker-text".into(),
        json!("emuwiz-enrolled-compact-v3"),
    );
    journal::write_journal(&f.root, &tx).unwrap();
    assert_eq!(
        journal::read_journal(&journal::journal_path(&f.root, &tx.transaction_id).unwrap())
            .unwrap(),
        tx
    );
    assert!(journal::remove_journal(&f.root, "enrolled").is_err());
    let before = f.image();
    let mut impossible = b.base.transaction.clone();
    impossible
        .created_directories
        .push(f.root.join("invented-directory"));
    assert!(
        b.checkpoint(&f.root, &mut impossible)
            .unwrap_err()
            .to_string()
            .contains("immutable operation authority changed")
    );
    assert_eq!(f.image(), before);
}

#[test]
fn missing_descriptor_still_surfaces_a_recovery_history_problem() {
    let (f, b) = fixture(1);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let marker = journal::journal_path(&f.root, "enrolled").unwrap();
    fs::rename(&marker, f.home.join("retained-descriptor.json")).unwrap();
    let (transactions, problems) = journal::list_journals(&f.root);
    assert!(transactions.is_empty());
    assert!(
        !problems.is_empty(),
        "missing descriptor must not hide compact evidence"
    );
    let (_, recovery_problems) = journal::find_recovery_transactions(&f.root);
    assert_eq!(problems, recovery_problems);
    assert!(journal::write_journal(&f.root, &f.base().transaction).is_err());
    assert!(f.claims() > 0);
}

#[test]
fn terminal_history_refuses_a_demonstrable_filesystem_contradiction() {
    let (f, b) = fixture(1);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    finish(&f);
    let source = f.base().transaction.entries[0].source_path.clone();
    fs::write(&source, b"later external modification").unwrap();
    let result = f.run(f.request("inspect", None));
    assert_eq!(result["ok"], false);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("terminal compact rollback contradicts source identity")
    );
}

#[test]
fn format_probe_does_not_open_legacy_nonregular_journals_for_blocking_reads() {
    use std::os::unix::ffi::OsStrExt;
    let (f, b) = fixture(1);
    let root = f.home.join("legacy-fifo");
    private_dir(&root);
    let mut tx = b.base.transaction.clone();
    tx.transaction_id = "fifo".into();
    let path = journal::journal_path(&root, "fifo").unwrap();
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    // Format detection must not wait for a FIFO producer. The unchanged legacy
    // publisher then refuses the nonregular object and preserves it.
    assert!(!super::super::is_enrolled(&root, "fifo"));
    let error = journal::write_journal(&root, &tx).unwrap_err().to_string();
    assert!(error.contains("not a regular file"), "{error}");
    use std::os::unix::fs::FileTypeExt;
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_fifo());
}

#[test]
fn attention_uses_current_compact_state_with_explicit_bounded_coverage() {
    use crate::attention::{AttentionSeverity, AttentionState};
    use crate::operation::{AttentionReceiptPaths, attention_receipt_snapshot};
    let (f, b) = fixture(3);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let path = journal::journal_path(&f.root, "enrolled").unwrap();
    let descriptor = fs::read(&path).unwrap();
    // Reproduces the pre-fix background reader's failure to see any current tx.
    assert!(serde_json::from_slice::<RenameTransaction>(&descriptor).is_err());
    let mut tiny_budget = 1;
    let error = super::super::read_bounded(&path, &descriptor, &mut tiny_budget)
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(error.contains("byte budget exceeded"), "{error}");
    let mut tiny_budget = 1;
    let destination = &f.base().transaction.entries[0].destination_path;
    assert!(
        crate::dat::rename_apply::identity::capture_identity_bounded(destination, &mut tiny_budget)
            .unwrap_err()
            .to_string()
            .contains("byte budget exceeded")
    );
    let paths = AttentionReceiptPaths {
        rename: Some(f.root.clone()),
        library_views: Some(f.home.join("unused-views")),
        shared_apply: Some(f.home.join("unused-shared")),
        database: Some(f.home.join("unused-db/database.sqlite")),
    };
    let applied = attention_receipt_snapshot(&paths);
    assert!(!applied.limited, "{:?}", applied.coverage_notes);
    assert_eq!(applied.items().count(), 1);
    assert_eq!(
        applied.items().next().unwrap().state,
        AttentionState::Resolved
    );
    let grant = f.issue(RequestedAction::Undo);
    let mut request = f.request("undo", Some(&grant));
    request["cancel_after"] = json!(1);
    assert_eq!(f.run(request)["success"], false);
    let partial = attention_receipt_snapshot(&paths);
    assert!(!partial.limited, "{:?}", partial.coverage_notes);
    assert_eq!(partial.items().count(), 1);
    let item = partial.items().next().unwrap();
    assert_eq!(item.state, AttentionState::Unresolved);
    assert_eq!(item.severity, AttentionSeverity::Blocking);
    finish(&f);
    let undone = attention_receipt_snapshot(&paths);
    assert!(!undone.limited, "{:?}", undone.coverage_notes);
    assert_eq!(
        undone.items().next().unwrap().state,
        AttentionState::Resolved
    );
    fs::rename(&path, f.home.join("retained-descriptor.json")).unwrap();
    let missing = attention_receipt_snapshot(&paths);
    assert!(missing.limited);
    assert!(
        missing
            .coverage_notes
            .iter()
            .any(|s| s.contains("Missing compact receipt"))
    );
    assert_eq!(missing.items().count(), 0);
}

#[path = "fd_contract_signed_tests.rs"]
mod fd_contract_signed;

#[path = "shared_engine_tests.rs"]
mod shared_engine_tests;
