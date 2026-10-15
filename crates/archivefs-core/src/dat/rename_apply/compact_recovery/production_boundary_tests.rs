//! Production boundary characterizations in the accepted Rust path. A passing
//! NOT PREVENTED assertion is evidence of a deployment blocker, not protection.
use super::*;
use std::io::{Read, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

pub(super) fn held_writer(base: &Authority, request: &Value) -> io::Result<Value> {
    let file = OpenOptions::new()
        .write(true)
        .open(&base.transaction.entries[0].source_path)?;
    let id = identity(&file)?;
    fs::write(request["ready"].as_str().unwrap(), b"opened")?;
    let mut signal = [0];
    std::io::stdin().read_exact(&mut signal)?;
    file.write_all_at(b"tampered bytes 0", 0)?;
    file.sync_all()?;
    Ok(json!({"ok":true,"identity":id}))
}

#[test]
fn retained_writable_descriptor_is_explicitly_not_prevented() {
    let (f, b) = fixture(1);
    let receipt = b.load().unwrap();
    let entry = receipt.transaction.entries[0].clone();
    let input = f.home.join("held-writer-request.json");
    let output = f.home.join("held-writer-response.json");
    let ready = f.home.join("held-writer-ready");
    let mut request = f.request("held-writer", None);
    request["ready"] = json!(ready);
    fs::write(&input, serde_json::to_vec(&request).unwrap()).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "dat::rename_apply::compact_recovery::storage::integration::tests::worker",
        ])
        .env("EMUWIZ_R2_REQUEST", input)
        .env("EMUWIZ_R2_RESPONSE", &output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !ready.exists() {
        let _ = child.kill();
        let result = child.wait_with_output().unwrap();
        panic!("held writer failed to open: {result:?}");
    }
    // The other process holds the original writable inode across actual Apply.
    // Do not unwrap before closing its input and reaping it on this error path.
    let applied = b.apply(&receipt);
    let signal = child.stdin.take().unwrap().write_all(b"x");
    let result = child.wait_with_output().unwrap();
    signal.unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        applied.unwrap().transaction.state,
        TransactionState::Applied
    );
    let response: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(response["ok"], true);
    assert_eq!(
        response["identity"],
        json!([entry.identity.dev, entry.identity.ino])
    );
    assert!(!entry.source_path.exists());
    assert_eq!(
        fs::read(&entry.destination_path).unwrap(),
        b"tampered bytes 0"
    );
    assert_eq!(
        identity(&File::open(&entry.destination_path).unwrap()).unwrap(),
        (entry.identity.dev, entry.identity.ino)
    );
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let undo = f.run(f.request("undo", Some(&grant)));
    assert_eq!(undo["ok"], true, "{undo}");
    assert_eq!(undo["success"], false, "{undo}");
    assert_eq!(undo["tx"]["state"], "rollback_failed");
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], undo["tx"]);
    assert!(read["summary"].as_str().unwrap().contains("RollbackFailed"));
    assert!(
        !read["assessment"]
            .as_str()
            .unwrap()
            .contains("SafeToResume")
    );
    assert!(!entry.source_path.exists());
    assert_eq!(
        fs::read(&entry.destination_path).unwrap(),
        b"tampered bytes 0"
    );
    assert_eq!(f.claims(), 3);
    println!(
        "RETAINED WRITABLE DESCRIPTOR: NOT PREVENTED; original inode changed after Apply, failed Undo retained ownership; fixture {}",
        f.home.display()
    );
}

#[test]
fn container_rewind_refuses_against_independently_retained_signed_boundary() {
    let (f, b) = fixture(2);
    let initial = f.image();
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let current = f.image();
    let grant = f.issue(RequestedAction::Undo);
    let container = f.root.join(NAMESPACE).join(LEAF);
    // Preserve the inode: a replacement-inode refusal must not mask the check.
    fs::write(&container, &initial).unwrap();
    refused_unchanged(
        &f,
        f.request("undo", Some(&grant)),
        "evidence differs from independently issued boundary",
    );
    for entry in &f.base().transaction.entries {
        assert!(!entry.source_path.exists());
        assert_eq!(
            capture_identity(&entry.destination_path).unwrap(),
            entry.identity
        );
    }
    assert_eq!(f.claims(), 6);
    fs::write(f.home.join("rewound-container-evidence"), initial).unwrap();
    fs::write(container, current).unwrap();
    finish(&f);
}

#[test]
fn corrupt_ownership_database_refuses_without_rewriting_evidence() {
    let (f, b) = fixture(2);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let path = f.home.join("admission/owners.sqlite");
    let original = fs::read(&path).unwrap();
    let mut damaged = original.clone();
    damaged[0] ^= 0xff;
    fs::write(&path, &damaged).unwrap();
    let container = f.image();
    let result = f.run(f.request("undo", Some(&grant)));
    assert_eq!(result["ok"], false, "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("file is not a database"),
        "{result}"
    );
    assert_eq!(fs::read(&path).unwrap(), damaged);
    assert_eq!(f.image(), container);
    assert!(
        !f.home
            .join("issuer")
            .join(format!("used-{}", grant.nonce))
            .exists()
    );
    for entry in &f.base().transaction.entries {
        assert!(!entry.source_path.exists());
        assert_eq!(
            capture_identity(&entry.destination_path).unwrap(),
            entry.identity
        );
    }
    fs::write(f.home.join("corrupt-owners-evidence.sqlite"), damaged).unwrap();
    fs::write(path, original).unwrap();
    assert_eq!(f.claims(), 6);
    finish(&f);
}

#[test]
fn restored_authority_snapshot_reenables_spent_grant_not_prevented() {
    let (f, b) = fixture(2);
    b.apply(&b.load().unwrap()).unwrap();
    drop(b);
    let grant = f.issue(RequestedAction::Undo);
    let image = f.image();
    let database = fs::read(f.home.join("admission/owners.sqlite")).unwrap();
    let grant_path = f.home.join("issuer").join(format!("grant-{}", grant.nonce));
    let grant_bytes = fs::read(&grant_path).unwrap();
    let consumed = f.run(f.request("consume", Some(&grant)));
    assert_eq!(consumed["consumed"], true, "{consumed}");
    refused_unchanged(
        &f,
        f.request("undo", Some(&grant)),
        "grant already consumed",
    );
    // No operation ran after consumption. The used record is the only changed
    // authority content. Moving it out restores the complete earlier authority
    // snapshot while preserving directory/file identities and retaining evidence.
    let used = f.home.join("issuer").join(format!("used-{}", grant.nonce));
    fs::rename(used, f.home.join("spent-record-before-rollback")).unwrap();
    assert_eq!(f.image(), image);
    assert_eq!(
        fs::read(f.home.join("admission/owners.sqlite")).unwrap(),
        database
    );
    assert_eq!(fs::read(grant_path).unwrap(), grant_bytes);
    let replay = f.run(f.request("undo", Some(&grant)));
    assert_eq!(replay["success"], true, "{replay}");
    f.restored();
    assert_eq!(f.claims(), 0);
    println!(
        "AUTHORITY SNAPSHOT ROLLBACK: NOT PREVENTED; a spent genuine grant became usable after restoration; fixture {}",
        f.home.display()
    );
}

#[test]
fn bounded_payload_and_retry_exhaustion_preserve_current_state_and_ownership() {
    let (f, b) = fixture(1);
    let mut transaction = b.load().unwrap().transaction;
    transaction.entries[0].preflight_failures = vec!["x".repeat(SLOT as usize)];
    let image = f.image();
    let error = b.checkpoint(&f.root, &mut transaction).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("checkpoint payload exceeds reservation")
    );
    assert_eq!(f.image(), image);
    let applied = b.apply(&b.load().unwrap()).unwrap();
    assert_eq!(applied.transaction.state, TransactionState::Applied);
    transaction = applied.transaction;
    let used = b.store.scan(&b.base).unwrap().count;
    for index in used..b.store.layout.slots {
        transaction.entries[0].preflight_failures = vec![format!("bounded retry {index}")];
        b.checkpoint(&f.root, &mut transaction).unwrap();
    }
    assert_eq!(b.store.scan(&b.base).unwrap().count, b.store.layout.slots);
    let current = b.load().unwrap().transaction;
    let full = f.image();
    transaction.entries[0].preflight_failures = vec!["one retry too many".into()];
    let error = b.checkpoint(&f.root, &mut transaction).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("checkpoint reservation exhausted")
    );
    assert_eq!(f.image(), full);
    assert_eq!(full.len(), image.len());
    assert_eq!(b.load().unwrap().transaction, current);
    // Reach actual Undo checkpoint publication; a generic refusal of a spent
    // Apply would not prove that capacity stopped the operation before mutation.
    let error = b
        .undo(&b.load().unwrap(), &AtomicBool::new(false))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("checkpoint reservation exhausted"),
        "{error}"
    );
    assert_eq!(b.load().unwrap().transaction, current);
    assert_eq!(f.image(), full);
    drop(b);
    for entry in &f.base().transaction.entries {
        assert!(!entry.source_path.exists());
        assert_eq!(
            capture_identity(&entry.destination_path).unwrap(),
            entry.identity
        );
    }
    assert_eq!(f.claims(), 3);
    let read = f.run(f.request("inspect", None));
    assert_eq!(read["tx"], serde_json::to_value(current).unwrap());
}

#[test]
fn maximum_enrolled_capacity_round_trip() {
    let (f, b) = fixture(8);
    let result = b.apply(&b.load().unwrap()).unwrap();
    assert_eq!(result.transaction.state, TransactionState::Applied);
    assert!(b.store.layout.length <= CAP);
    drop(b);
    finish(&f);
}
