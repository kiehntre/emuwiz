//! Deterministic fault injection for signed Undo of an interrupted or failed
//! Apply. Every case runs the real executor, bridge, admission, signed grant
//! and recovery dispatcher in separate processes that die with `_exit` at a
//! named boundary; a later signed Undo must restore every original exactly,
//! or refuse with all evidence retained. Process death is the logical crash
//! model used here; no power-loss durability claim is made.
use super::*;
use crate::dat::rename_apply::identity::fd_publication_probe::contract::{self, Action, Phase};
use crate::dat::rename_apply::identity::{content_identity_matches, identity_matches};
use crate::platform_evidence_fusion::plan_transaction::{RecoveryAssessment, assess_recovery};

const APPLY_EDGES: [&str; 11] = [
    "Intent",
    "BeforeCapture",
    "AfterCapture",
    "CapturedRecorded",
    "BeforePublish",
    "AfterPublish",
    "CleanupRecorded",
    "BeforeUnlink",
    "AfterUnlink",
    "BeforeComplete",
    "CompletionRecorded",
];

fn engine(n: usize) -> (Fixture, Rc<Bridge>) {
    let (f, b) = fixture_engine_mode(n, true, true, true);
    b.engine_enabled.set(true);
    (f, b)
}
fn signed(f: &Fixture, action: RequestedAction) -> Value {
    let grant = f.issue(action);
    let mut req = f.request(
        if action == RequestedAction::Apply {
            "apply"
        } else {
            "undo"
        },
        Some(&grant),
    );
    req["fd_engine"] = json!(true);
    req
}
/// A fresh enrolled batch whose Apply dies at `edge` while working on `entry`.
fn interrupted_apply(n: usize, entry: usize, edge: &str) -> Fixture {
    let (f, b) = engine(n);
    drop(b);
    let mut req = signed(&f, RequestedAction::Apply);
    req["fd_exit"] = json!(edge);
    req["fd_exit_entry"] = json!(entry);
    let (out, _) = f.child(req);
    assert_eq!(
        out.status.code(),
        Some(82),
        "{edge}@{entry}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    f
}
fn inspect(f: &Fixture) -> Value {
    let observed = f.run(f.request("inspect", None));
    assert_eq!(observed["ok"], true, "{observed}");
    observed
}
fn current(f: &Fixture) -> RenameTransaction {
    serde_json::from_value(inspect(f)["tx"].clone()).unwrap()
}
fn holding_clear(f: &Fixture) {
    let tx = f.base().transaction;
    for i in 0..tx.entries.len() {
        let (held, salvage) = contract::paths(&f.root, &tx, i).unwrap();
        assert!(!held.exists(), "holding slot {i} still occupied");
        assert!(!salvage.exists(), "salvage slot {i} still occupied");
        assert!(
            !contract::anchor::path(&f.root, &tx, i).unwrap().exists(),
            "anchor {i} not retired"
        );
    }
}
fn anchors_intact(f: &Fixture) {
    let tx = f.base().transaction;
    for (i, e) in tx.entries.iter().enumerate() {
        let anchor = contract::anchor::path(&f.root, &tx, i).unwrap();
        assert_eq!(capture_identity(&anchor).unwrap(), e.identity, "anchor {i}");
        assert_eq!(
            fs::read(anchor).unwrap(),
            format!("original bytes {i}").as_bytes()
        );
    }
}
/// Every original is still reachable through a verified name (library,
/// trusted holding or anchor) or a salvaged copy, and never under both
/// library names at once.
fn original_preserved(f: &Fixture) {
    let tx = f.base().transaction;
    for (i, e) in tx.entries.iter().enumerate() {
        let (held, salvage) = contract::paths(&f.root, &tx, i).unwrap();
        let anchor = contract::anchor::path(&f.root, &tx, i).unwrap();
        let same = |p: &Path| capture_identity(p).is_ok_and(|v| identity_matches(&e.identity, &v));
        let salvaged =
            capture_identity(&salvage).is_ok_and(|v| content_identity_matches(&e.identity, &v));
        assert!(
            same(&e.source_path)
                || same(&e.destination_path)
                || same(&held)
                || same(&anchor)
                || salvaged,
            "entry {i}: original object is no longer reachable"
        );
        assert!(
            !(same(&e.source_path) && same(&e.destination_path)),
            "entry {i}: original published under both library names"
        );
    }
}
/// Signed Undo restores everything, is idempotent under a second fresh grant,
/// and only then releases ownership.
fn assert_recovered(f: &Fixture, label: &str) -> Value {
    let mut first = signed(f, RequestedAction::Undo);
    first["release"] = json!(false);
    let result = f.run(first);
    assert_eq!(result["ok"], true, "{label}: {result}");
    assert_eq!(result["success"], true, "{label}: {result}");
    assert_eq!(result["tx"]["state"], "rolled_back", "{label}");
    f.restored();
    holding_clear(f);
    assert!(
        f.claims() > 0,
        "{label}: ownership released before verified release"
    );
    let again = f.run(signed(f, RequestedAction::Undo));
    assert_eq!(again["ok"], true, "{label}: {again}");
    assert_eq!(again["success"], true, "{label}: {again}");
    assert_eq!(
        again["tx"], result["tx"],
        "{label}: repeated recovery changed the record"
    );
    f.restored();
    holding_clear(f);
    assert_eq!(f.claims(), 0, "{label}");
    let read = inspect(f);
    assert_eq!(read["tx"], result["tx"], "{label}");
    assert_eq!(read["recovery"], 0, "{label}");
    assert_eq!(read["undoable"], 0, "{label}");
    assert_eq!(read["assessment"], "AlreadyRolledBack", "{label}");
    result
}

/// What a signed Undo must do with an entry whose Apply died at a boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Everything is restored and released.
    Recovers,
    /// The accepted Undo-direction ambiguity: parked with Undo evidence.
    UndoReview,
    /// A step happened but its record was lost: parked with Apply evidence
    /// instead of being completed from the anchored inode.
    ApplyReview(Phase),
}
fn apply_boundary_outcome(edge: &str) -> Expect {
    match edge {
        "AfterPublish" => Expect::ApplyReview(Phase::DestinationOccupied),
        "AfterUnlink" | "BeforeComplete" => Expect::ApplyReview(Phase::Incomplete),
        _ => Expect::Recovers,
    }
}
/// The entry is parked for review with Apply-direction evidence; entries
/// after it were already settled, entries before it are untouched, nothing
/// was published or unlinked, and further attempts refuse without mutation.
fn assert_apply_review(f: &Fixture, entry: usize, phase: Phase, label: &str) {
    let base = f.base().transaction;
    let before: Vec<Option<ObjectIdentity>> = base
        .entries
        .iter()
        .map(|e| capture_identity(&e.destination_path).ok())
        .collect();
    let result = f.run(signed(f, RequestedAction::Undo));
    assert_eq!(result["ok"], true, "{label}: {result}");
    assert_eq!(result["success"], false, "{label}: {result}");
    assert_eq!(result["tx"]["state"], "rolling_back", "{label}");
    let tx: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
    for (i, e) in tx.entries.iter().enumerate() {
        let v = contract::evidence(&tx, i).unwrap();
        match i.cmp(&entry) {
            std::cmp::Ordering::Less => {
                assert_eq!(e.state, EntryState::Applied, "{label} entry {i}");
                assert_eq!(v.unwrap().phase, Phase::Complete, "{label} entry {i}");
            }
            std::cmp::Ordering::Greater => {
                assert_eq!(e.state, EntryState::Skipped, "{label} entry {i}");
                assert_eq!(v.unwrap().phase, Phase::Unmutated, "{label} entry {i}");
            }
            std::cmp::Ordering::Equal => {
                assert_eq!(e.state, EntryState::ApplyFailed, "{label}");
                let v = v.unwrap();
                assert_eq!((v.action, v.phase), (Action::Apply, phase), "{label}");
                assert!(
                    v.detail.contains("operator review required"),
                    "{label}: {}",
                    v.detail
                );
                // The library destination is exactly as the crash left it.
                assert_eq!(
                    capture_identity(&e.destination_path).ok(),
                    before[i],
                    "{label}: recovery touched the destination name"
                );
                assert!(!e.source_path.exists(), "{label}: source re-published");
            }
        }
    }
    original_preserved(f);
    anchors_intact_for(f, |i| i == entry);
    assert!(f.claims() > 0, "{label}");
    let read = inspect(f);
    assert_eq!(read["tx"], result["tx"], "{label}");
    assert_ne!(read["assessment"], "SafeToResume", "{label}");
    refused_unchanged(
        f,
        signed(f, RequestedAction::Undo),
        "different interrupted FD action requires operator review",
    );
}
/// Anchors of the selected entries are still the enrolled originals.
fn anchors_intact_for(f: &Fixture, selected: impl Fn(usize) -> bool) {
    let tx = f.base().transaction;
    for (i, e) in tx.entries.iter().enumerate().filter(|(i, _)| selected(*i)) {
        let anchor = contract::anchor::path(&f.root, &tx, i).unwrap();
        assert_eq!(capture_identity(&anchor).unwrap(), e.identity, "anchor {i}");
    }
}

#[test]
fn interrupted_multifile_apply_is_recovered_by_signed_undo_at_every_boundary() {
    for entry in 0..3 {
        for edge in APPLY_EDGES {
            let label = format!("{edge}@{entry}");
            let f = interrupted_apply(3, entry, edge);
            let observed = inspect(&f);
            // Only the last entry's recorded completion leaves a fully applied
            // batch behind; every other boundary is an unfinished Apply.
            let batch_complete = entry == 2 && edge == "CompletionRecorded";
            assert_eq!(
                observed["tx"]["state"] == "applied",
                batch_complete,
                "{label}"
            );
            assert_ne!(observed["assessment"], "SafeToResume", "{label}");
            let tx: RenameTransaction = serde_json::from_value(observed["tx"].clone()).unwrap();
            for (i, e) in tx.entries.iter().enumerate() {
                let expected = match i.cmp(&entry) {
                    std::cmp::Ordering::Less => EntryState::Applied,
                    std::cmp::Ordering::Greater => EntryState::Planned,
                    std::cmp::Ordering::Equal if edge == "CompletionRecorded" => {
                        EntryState::Applied
                    }
                    std::cmp::Ordering::Equal => EntryState::Applying,
                };
                assert_eq!(e.state, expected, "{label} entry {i}");
            }
            anchors_intact(&f);
            original_preserved(&f);
            // The transaction is spent for Apply; Undo needs a genuine grant.
            refused_unchanged(
                &f,
                signed(&f, RequestedAction::Apply),
                "spent transaction cannot resume Apply",
            );
            let mut unsigned = f.request("undo", None);
            unsigned["fd_engine"] = json!(true);
            refused_unchanged(&f, unsigned, "genuine signed");
            match apply_boundary_outcome(edge) {
                Expect::ApplyReview(phase) => {
                    assert_apply_review(&f, entry, phase, &label);
                    continue;
                }
                Expect::UndoReview => unreachable!(),
                Expect::Recovers => (),
            }
            let result = assert_recovered(&f, &label);
            let tx: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
            for (i, e) in tx.entries.iter().enumerate() {
                let v = contract::evidence(&tx, i).unwrap().unwrap();
                assert_eq!(v.action, Action::Undo, "{label} entry {i}");
                if i > entry || (i == entry && matches!(edge, "Intent" | "BeforeCapture")) {
                    assert_eq!(e.state, EntryState::Skipped, "{label} entry {i}");
                    assert_eq!(v.phase, Phase::Unmutated, "{label} entry {i}");
                } else {
                    assert_eq!(e.state, EntryState::RolledBack, "{label} entry {i}");
                    assert_eq!(v.phase, Phase::Complete, "{label} entry {i}");
                }
            }
        }
    }
}

#[test]
fn executor_checkpoint_boundaries_of_an_interrupted_apply_are_recovered() {
    // Immediately before and after the durable Applying checkpoint of entry 1.
    for after in [false, true] {
        let (f, b) = engine(3);
        drop(b);
        let mut req = signed(&f, RequestedAction::Apply);
        req["fault"] = json!([EntryState::Applying, 1, after]);
        let (out, _) = f.child(req);
        assert_eq!(out.status.code(), Some(73), "after={after}");
        let tx = current(&f);
        assert_eq!(tx.entries[0].state, EntryState::Applied);
        assert_eq!(
            tx.entries[1].state,
            if after {
                EntryState::Applying
            } else {
                EntryState::Planned
            }
        );
        assert_eq!(tx.entries[2].state, EntryState::Planned);
        assert_recovered(&f, &format!("applying checkpoint after={after}"));
    }
    // Every numbered checkpoint from the batch preflight record to the first
    // entry's completion, before and after publication.
    // Checkpoints: 1 batch preflight, 2 Applying, 3 CapturePending, 4 Captured,
    // 5 CleanupPending, 6 Complete (entry 0). Dying BEFORE 5 or 6 is published
    // leaves a link or an unlink without its record: parked for review.
    for number in 1..=6 {
        for after in [false, true] {
            let (f, b) = engine(2);
            drop(b);
            let mut req = signed(&f, RequestedAction::Apply);
            req["checkpoint"] = json!([number, after]);
            let (out, _) = f.child(req);
            assert_eq!(out.status.code(), Some(76), "checkpoint {number} {after}");
            original_preserved(&f);
            let label = format!("checkpoint {number} after={after}");
            match (number, after) {
                (5, false) => assert_apply_review(&f, 0, Phase::DestinationOccupied, &label),
                (6, false) => assert_apply_review(&f, 0, Phase::Incomplete, &label),
                _ => {
                    assert_recovered(&f, &label);
                }
            }
        }
    }
    // A crash before the very first checkpoint leaves the transaction unspent:
    // a fresh signed Apply is still permitted and completes.
    let (f, b) = engine(2);
    drop(b);
    let mut req = signed(&f, RequestedAction::Apply);
    req["checkpoint"] = json!([1, false]);
    assert_eq!(f.child(req).0.status.code(), Some(76));
    let applied = f.run(signed(&f, RequestedAction::Apply));
    assert_eq!(applied["success"], true, "{applied}");
    assert_recovered(&f, "unspent re-apply");
    // Interrupted publication of an ApplyComplete record: a fully written but
    // uncommitted record is repaired under the fresh grant after filesystem
    // agreement; a torn record keeps refusing.
    for event in ["write", "sync", "commit", "ack"] {
        let (f, b) = engine(2);
        drop(b);
        let mut req = signed(&f, RequestedAction::Apply);
        req["publication"] = json!(event);
        assert_eq!(f.child(req).0.status.code(), Some(77), "{event}");
        original_preserved(&f);
        assert_recovered(&f, &format!("publication {event}"));
    }
    let (f, b) = engine(2);
    drop(b);
    let mut req = signed(&f, RequestedAction::Apply);
    req["io"] = json!("short");
    assert_eq!(f.run(req)["ok"], false);
    original_preserved(&f);
    refused_unchanged(
        &f,
        signed(&f, RequestedAction::Undo),
        "exact runtime values unavailable",
    );
}

#[test]
fn interrupted_recovery_is_completed_or_refused_truthfully_on_retry() {
    // (Apply edge for entry 1, recovery exit edge, exit entry, matches to
    // skip, what a retry must do). Entry 0 is Applied, entry 1 interrupted.
    use Expect::*;
    let cases: &[(&str, &str, u64, u64, Expect)] = &[
        ("Intent", "UnmutatedRecorded", 1, 0, Recovers),
        ("Intent", "BeforeAnchorUnlink", 1, 0, Recovers),
        ("Intent", "AfterAnchorUnlink", 1, 0, Recovers),
        ("AfterCapture", "AdoptedRecorded", 1, 0, Recovers),
        ("AfterCapture", "CapturedRecorded", 1, 0, Recovers),
        ("AfterCapture", "BeforePublish", 1, 0, Recovers),
        ("AfterCapture", "AfterPublish", 1, 0, UndoReview),
        ("AfterCapture", "CleanupRecorded", 1, 0, Recovers),
        ("AfterCapture", "BeforeAnchorUnlink", 1, 0, Recovers),
        ("AfterCapture", "AfterAnchorUnlink", 1, 0, Recovers),
        ("AfterCapture", "BeforeUnlink", 1, 0, Recovers),
        ("AfterCapture", "AfterUnlink", 1, 0, UndoReview),
        ("AfterCapture", "BeforeComplete", 1, 0, UndoReview),
        ("AfterCapture", "CompletionRecorded", 1, 0, Recovers),
        // Recorded publication: the recorded cleanup is finished, then undone.
        ("CleanupRecorded", "CleanupRecorded", 1, 0, Recovers),
        ("CleanupRecorded", "BeforeUnlink", 1, 0, Recovers),
        // Dying after that unlink but before its completion record leaves the
        // "unlink without record" state: parked, never completed from the anchor.
        (
            "CleanupRecorded",
            "AfterUnlink",
            1,
            0,
            ApplyReview(Phase::Incomplete),
        ),
        (
            "CleanupRecorded",
            "BeforeComplete",
            1,
            0,
            ApplyReview(Phase::Incomplete),
        ),
        ("CleanupRecorded", "CompletionRecorded", 1, 0, Recovers),
        ("CleanupRecorded", "CapturedRecorded", 1, 0, Recovers),
        ("CleanupRecorded", "AfterUnlink", 1, 1, UndoReview),
        ("AfterCapture", "CapturedRecorded", 0, 0, Recovers),
        ("AfterCapture", "AfterPublish", 0, 0, UndoReview),
    ];
    for (apply_edge, exit_edge, entry, skip, expect) in cases {
        let label =
            format!("apply@{apply_edge} then recovery exit {exit_edge}@{entry} skip {skip}");
        let f = interrupted_apply(2, 1, apply_edge);
        let mut req = signed(&f, RequestedAction::Undo);
        req["fd_exit"] = json!(exit_edge);
        req["fd_exit_entry"] = json!(entry);
        req["fd_exit_skip"] = json!(skip);
        let (out, _) = f.child(req);
        assert_eq!(
            out.status.code(),
            Some(82),
            "{label}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(f.claims() > 0, "{label}");
        original_preserved(&f);
        let observed = inspect(&f);
        assert_ne!(observed["tx"]["state"], "rolled_back", "{label}");
        assert_ne!(observed["assessment"], "SafeToResume", "{label}");
        match expect {
            Recovers => {
                assert_recovered(&f, &label);
                continue;
            }
            ApplyReview(phase) => {
                assert_apply_review(&f, *entry as usize, *phase, &label);
                continue;
            }
            UndoReview => (),
        }
        // The accepted Contract 1-I Undo ambiguities (a link or unlink whose
        // record was lost) stay incomplete and parked for review.
        let result = f.run(signed(&f, RequestedAction::Undo));
        assert_eq!(result["ok"], true, "{label}: {result}");
        assert_eq!(result["success"], false, "{label}: {result}");
        assert_ne!(result["tx"]["state"], "rolled_back", "{label}");
        let tx: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
        let v = contract::evidence(&tx, *entry as usize).unwrap().unwrap();
        assert_eq!(v.action, Action::Undo, "{label}");
        assert!(
            matches!(
                v.phase,
                Phase::DestinationOccupied | Phase::PossiblyUnrecoverable
            ),
            "{label}: {:?}",
            v.phase
        );
        original_preserved(&f);
        assert!(f.claims() > 0, "{label}");
        refused_unchanged(
            &f,
            signed(&f, RequestedAction::Undo),
            "operator review required",
        );
    }
}

#[test]
fn substituted_or_missing_recovery_material_refuses_without_publishing() {
    let retained = |f: &Fixture, name: &str| f.home.join(name);
    // (a) The trusted holding entry was replaced after an unrecorded capture.
    {
        let f = interrupted_apply(2, 1, "AfterCapture");
        let tx = f.base().transaction;
        let held = contract::paths(&f.root, &tx, 1).unwrap().0;
        fs::rename(&held, retained(&f, "original-1-retained")).unwrap();
        fs::write(&held, b"foreign held").unwrap();
        let result = f.run(signed(&f, RequestedAction::Undo));
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(result["success"], false, "{result}");
        let after: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
        assert_eq!(after.entries[1].state, EntryState::ApplyFailed);
        assert_eq!(
            after.entries[0].state,
            EntryState::Applied,
            "review stops before earlier entries"
        );
        let v = contract::evidence(&after, 1).unwrap().unwrap();
        assert_eq!((v.action, v.phase), (Action::Apply, Phase::ForeignHeld));
        assert_eq!(after.state, TransactionState::RollingBack);
        assert!(!tx.entries[1].source_path.exists());
        assert!(!tx.entries[1].destination_path.exists());
        assert_eq!(fs::read(&held).unwrap(), b"foreign held");
        assert_eq!(
            capture_identity(&retained(&f, "original-1-retained")).unwrap(),
            tx.entries[1].identity
        );
        assert_eq!(inspect(&f)["tx"], result["tx"]);
        refused_unchanged(
            &f,
            signed(&f, RequestedAction::Undo),
            "different interrupted FD action requires operator review",
        );
        assert!(f.claims() > 0);
    }
    // (b) The source was replaced before any capture: nothing is touched.
    {
        let f = interrupted_apply(2, 1, "Intent");
        let tx = f.base().transaction;
        let source = &tx.entries[1].source_path;
        fs::rename(source, retained(&f, "original-1-moved")).unwrap();
        fs::write(source, b"foreign source").unwrap();
        refused_unchanged(
            &f,
            signed(&f, RequestedAction::Undo),
            "source is a different object",
        );
        assert_eq!(fs::read(source).unwrap(), b"foreign source");
        anchors_intact(&f);
        assert!(!tx.entries[1].destination_path.exists());
    }
    // (c) A foreign object appeared at the destination after a recorded capture.
    {
        let f = interrupted_apply(2, 1, "CapturedRecorded");
        let tx = f.base().transaction;
        fs::write(&tx.entries[1].destination_path, b"foreign destination").unwrap();
        let result = f.run(signed(&f, RequestedAction::Undo));
        assert_eq!(result["success"], false, "{result}");
        let after: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
        let v = contract::evidence(&after, 1).unwrap().unwrap();
        assert_eq!(
            (v.action, v.phase),
            (Action::Apply, Phase::DestinationOccupied)
        );
        assert_eq!(after.entries[1].state, EntryState::ApplyFailed);
        let held = contract::paths(&f.root, &tx, 1).unwrap().0;
        assert_eq!(capture_identity(&held).unwrap(), tx.entries[1].identity);
        assert_eq!(
            fs::read(&tx.entries[1].destination_path).unwrap(),
            b"foreign destination"
        );
        assert!(!tx.entries[1].source_path.exists());
        refused_unchanged(
            &f,
            signed(&f, RequestedAction::Undo),
            "different interrupted FD action requires operator review",
        );
    }
    // (d) The enrollment anchor is gone: continuity cannot be proven.
    {
        let f = interrupted_apply(2, 1, "AfterCapture");
        let tx = f.base().transaction;
        let anchor = contract::anchor::path(&f.root, &tx, 1).unwrap();
        fs::rename(&anchor, retained(&f, "anchor-1-retained")).unwrap();
        refused_unchanged(
            &f,
            signed(&f, RequestedAction::Undo),
            "enrollment anchor missing",
        );
        let held = contract::paths(&f.root, &tx, 1).unwrap().0;
        assert_eq!(capture_identity(&held).unwrap(), tx.entries[1].identity);
    }
    // (e) The trusted holding entry vanished after its capture was recorded.
    {
        let f = interrupted_apply(2, 1, "CapturedRecorded");
        let tx = f.base().transaction;
        let held = contract::paths(&f.root, &tx, 1).unwrap().0;
        fs::rename(&held, retained(&f, "held-1-retained")).unwrap();
        refused_unchanged(
            &f,
            signed(&f, RequestedAction::Undo),
            "holding entry missing",
        );
        anchors_intact(&f);
    }
    // (f) The published destination was deleted after publication was recorded.
    {
        let f = interrupted_apply(2, 1, "CleanupRecorded");
        let tx = f.base().transaction;
        fs::remove_file(&tx.entries[1].destination_path).unwrap();
        let result = f.run(signed(&f, RequestedAction::Undo));
        assert_eq!(result["success"], false, "{result}");
        let after: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
        let v = contract::evidence(&after, 1).unwrap().unwrap();
        assert_eq!((v.action, v.phase), (Action::Apply, Phase::Incomplete));
        let held = contract::paths(&f.root, &tx, 1).unwrap().0;
        assert_eq!(capture_identity(&held).unwrap(), tx.entries[1].identity);
        refused_unchanged(
            &f,
            signed(&f, RequestedAction::Undo),
            "different interrupted FD action requires operator review",
        );
    }
    // (g) The published destination was swapped for a foreign file after the
    // holding link was already removed: only the anchor retains the original.
    {
        let f = interrupted_apply(2, 1, "AfterUnlink");
        let tx = f.base().transaction;
        let destination = &tx.entries[1].destination_path;
        fs::rename(destination, retained(&f, "published-1-moved")).unwrap();
        fs::write(destination, b"foreign destination").unwrap();
        let result = f.run(signed(&f, RequestedAction::Undo));
        assert_eq!(result["success"], false, "{result}");
        let after: RenameTransaction = serde_json::from_value(result["tx"].clone()).unwrap();
        let v = contract::evidence(&after, 1).unwrap().unwrap();
        assert_eq!((v.action, v.phase), (Action::Apply, Phase::Incomplete));
        anchors_intact(&f);
        assert_eq!(fs::read(destination).unwrap(), b"foreign destination");
        assert!(!tx.entries[1].source_path.exists());
        assert!(f.claims() > 0);
    }
}

#[test]
fn legacy_paths_cannot_claim_an_interrupted_r2_apply() {
    let f = interrupted_apply(2, 1, "AfterCapture");
    let image = f.image();
    let mut tx = current(&f);
    let err = rollback::rollback_transaction_confined(
        &mut tx,
        &f.root,
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.root]),
    )
    .unwrap_err();
    assert!(err.contains("admitted operation controller"), "{err}");
    let mut again = current(&f);
    let err =
        crate::dat::rename_apply::reconcile::reconcile_recovery(&mut again, &f.root).unwrap_err();
    assert!(
        err.contains("legacy two-name reconciliation refused"),
        "{err}"
    );
    assert!(
        journal::write_journal(&f.root, &current(&f))
            .unwrap_err()
            .to_string()
            .contains("enrolled checkpoint authority")
    );
    assert!(journal::remove_journal(&f.root, "enrolled").is_err());
    assert_eq!(f.image(), image);
    original_preserved(&f);
    assert!(f.claims() > 0);
    assert_recovered(&f, "after legacy refusals");
}

#[test]
fn competing_recovery_process_is_refused_while_admission_is_held() {
    let f = interrupted_apply(2, 1, "AfterCapture");
    let mut req = signed(&f, RequestedAction::Undo);
    req["fd_action"] = json!("compete-recovery");
    req["fd_action_edge"] = json!("AdoptedRecorded");
    let result = f.run(req);
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["success"], true, "{result}");
    let competitor: Value =
        serde_json::from_slice(&fs::read(f.home.join("fd-external-response.json")).unwrap())
            .unwrap();
    assert_eq!(competitor["busy"], true, "{competitor}");
    f.restored();
    holding_clear(&f);
    assert_eq!(f.claims(), 0);
}

#[test]
fn legacy_assessment_never_offers_resume_for_fd_owned_transactions() {
    let (f, b) = engine(1);
    drop(b);
    let mut parked = f.base().transaction;
    parked.state = TransactionState::ApplyFailed;
    parked.entries[0].state = EntryState::ApplyFailed;
    parked.entries[0].unknown.insert(
        contract::KEY.into(),
        serde_json::to_value(contract::Evidence {
            action: Action::Apply,
            phase: Phase::ForeignHeld,
            held_identity: None,
            salvage_identity: None,
            detail: "Held for review / Apply incomplete".into(),
        })
        .unwrap(),
    );
    assert_eq!(
        assess_recovery(&parked, &[]),
        RecoveryAssessment::ManualRecoveryRequired
    );
    let mut planned = f.base().transaction;
    planned.state = TransactionState::ApplyFailed;
    assert_eq!(
        assess_recovery(&planned, &[]),
        RecoveryAssessment::ManualRecoveryRequired
    );
    let mut undone = f.base().transaction;
    undone.state = TransactionState::RolledBack;
    undone.entries[0].state = EntryState::Skipped;
    assert_eq!(
        assess_recovery(&undone, &[]),
        RecoveryAssessment::AlreadyRolledBack
    );
    // Unchanged legacy behaviour for an unenrolled transaction.
    let mut legacy = f.base().transaction;
    legacy.state = TransactionState::ApplyFailed;
    for e in &mut legacy.entries {
        e.unknown.clear();
    }
    assert_eq!(
        assess_recovery(&legacy, &[]),
        RecoveryAssessment::SafeToResume
    );
}
