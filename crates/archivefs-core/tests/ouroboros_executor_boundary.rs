//! Project Ouroboros executor boundary, seen from outside the crate.
//!
//! An integration test links `archivefs_core` exactly as `emuwiz-cli` and
//! Blue do: without `cfg(test)`, and with whatever features the build turns
//! on (including `--all-features`). From here the scratch authority does not
//! exist, so even a hand-forged plan that is structurally valid, correctly
//! digested, names real directories and selects the test scratch policy must
//! be refused by every mutating entry point, before anything is written.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use archivefs_core::catalogue_health::SourceRootBinding;
use archivefs_core::dat::rename_apply::identity::capture_identity;
use archivefs_core::transform::fscompat::{self, FsPolicy};
use archivefs_core::transform::journal::{self, JournalWriter};
use archivefs_core::transform::model::{
    ENGINE, Event, Expect, PLAN_FORMAT, PLAN_VERSION, RootBinding,
};
use archivefs_core::transform::{
    Context, EngineError, PRODUCTION_UNAVAILABLE, Plan, PlanKind, RetentionPolicy, Step, Strategy,
    apply, apply_undo, engine::plan_bytes, plan_digest, recover, recover_all,
};

struct Forged {
    _tmp: tempfile::TempDir,
    root_a: PathBuf,
    source: PathBuf,
    content: Vec<u8>,
    journal: PathBuf,
}

fn forge(policy: FsPolicy, kind: PlanKind) -> (Forged, Plan) {
    let tmp = tempfile::tempdir().unwrap();
    let root_a = tmp.path().join("lib-a");
    let root_b = tmp.path().join("lib-b");
    for d in [&root_a, &root_b] {
        std::fs::create_dir_all(d.join("nes")).unwrap();
    }
    let source = root_a.join("nes").join("Game (USA).nes");
    let content = b"forged-plan target".to_vec();
    std::fs::write(&source, &content).unwrap();
    let destination = root_b.join("nes").join("Game (USA).nes");
    let binding = |p: &Path| SourceRootBinding::inspect(p).expect("root binding");
    let facts = |p: &Path| fscompat::inspect(p).expect("mount facts");
    let plan = Plan {
        format: PLAN_FORMAT.into(),
        version: PLAN_VERSION,
        engine: ENGINE.into(),
        txn_id: journal::new_txn_id().unwrap(),
        kind,
        created_at_unix: 0,
        catalogue: tmp.path().join("catalogue.sqlite3"),
        catalogue_binding: serde_json::Value::Null,
        source_root: RootBinding {
            path: root_a.clone(),
            binding: binding(&root_a),
            source_folder_id: 1,
        },
        destination_root: RootBinding {
            path: root_b.clone(),
            binding: binding(&root_b),
            source_folder_id: 2,
        },
        source_path: source.clone(),
        destination_path: destination.clone(),
        source_identity: capture_identity(&source).unwrap(),
        strategy: Strategy::AtomicRename,
        steps: vec![
            Step::CheckPreconditions,
            Step::Rename {
                from: source.clone(),
                to: destination,
                roots: vec![root_a.clone(), root_b.clone()],
                expect: Expect::Original,
            },
            Step::CommitCatalogue { reverse: false },
            Step::FinalVerify,
        ],
        point_of_no_return: 1,
        retention: RetentionPolicy::RetainUntilReleased,
        dependency_digest: String::new(),
        effects: Vec::new(),
        filesystems: vec![facts(&root_a), facts(&root_b)],
        filesystem_policy: policy,
        published_identity: None,
        retired_path: None,
    };
    let journal = tmp.path().join("journals");
    (
        Forged {
            _tmp: tmp,
            root_a,
            source,
            content,
            journal,
        },
        plan,
    )
}

fn refused_as_unavailable<T: std::fmt::Debug>(r: Result<T, EngineError>) {
    match r {
        Err(EngineError::Refused(m)) => assert_eq!(m, PRODUCTION_UNAVAILABLE),
        other => panic!("expected the production refusal, got {other:?}"),
    }
}

fn untouched(f: &Forged) {
    assert_eq!(
        std::fs::read(&f.source).unwrap(),
        f.content,
        "the source is unchanged"
    );
    let entries: Vec<_> = std::fs::read_dir(f.root_a.join("nes"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(entries.len(), 1, "nothing was staged next to the source");
}

#[test]
fn a_forged_scratch_or_production_plan_is_refused_before_anything_is_written() {
    for policy in [FsPolicy::TestScratch, FsPolicy::Production] {
        let (f, plan) = forge(policy, PlanKind::Relocate);
        let digest = plan_digest(&plan);
        refused_as_unavailable(apply(&Context::new(&f.journal), &plan, &digest));
        untouched(&f);
        assert!(!f.journal.exists(), "no journal was created");
    }
}

#[test]
fn a_forged_undo_is_refused() {
    let kind = PlanKind::Undo {
        of_txn: "0".repeat(32),
        of_plan_digest: "0".repeat(64),
        of_complete_sum: "0".repeat(64),
    };
    let (f, plan) = forge(FsPolicy::TestScratch, kind);
    let digest = plan_digest(&plan);
    refused_as_unavailable(apply_undo(&Context::new(&f.journal), &plan, &digest));
    untouched(&f);
    assert!(!f.journal.exists(), "no journal was created");
}

#[test]
fn an_interrupted_journal_written_from_outside_is_never_recovered() {
    // The public journal writer lets anyone record a begun transaction.
    // Recovery still needs the authority, so it refuses and appends nothing.
    let (f, plan) = forge(FsPolicy::TestScratch, PlanKind::Relocate);
    journal::ensure_root(&f.journal).unwrap();
    let mut w = JournalWriter::create(&f.journal, &plan.txn_id, &plan_bytes(&plan), None).unwrap();
    w.append(Event::Begun {
        pid: std::process::id(),
        engine: ENGINE.into(),
    })
    .unwrap();
    drop(w);
    let before = journal::load(&f.journal, &plan.txn_id)
        .unwrap()
        .events()
        .count();
    let ctx = Context::new(&f.journal);
    refused_as_unavailable(recover(&ctx, &plan.txn_id));
    let all = recover_all(&ctx);
    assert_eq!(all.len(), 1);
    for (_, r) in all {
        refused_as_unavailable(r);
    }
    assert_eq!(
        journal::load(&f.journal, &plan.txn_id)
            .unwrap()
            .events()
            .count(),
        before,
        "nothing appended"
    );
    untouched(&f);
}
