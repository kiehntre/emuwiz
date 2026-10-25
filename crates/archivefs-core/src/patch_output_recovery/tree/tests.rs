use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    temp: tempfile::TempDir,
    plan: TreePatchPlan,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("original.bin"), b"source bytes").unwrap();
        let patch = temp.path().join("patch.ips");
        fs::write(&patch, b"reviewed package").unwrap();
        let plan = TreePatchPlan::review(&[source, patch], &temp.path().join("output with spaces"))
            .unwrap();
        Self { temp, plan }
    }
    fn prepare(&self) -> PreparedTreePatch {
        prepare(
            &self.plan,
            |root| {
                fs::create_dir(root.join("nested"))?;
                fs::write(root.join("descriptor.cue"), b"descriptor")?;
                fs::write(root.join("nested/track.bin"), b"patched bytes")
            },
            |root| {
                if fs::read(root.join("nested/track.bin"))? != b"patched bytes" {
                    return Err(refuse("bad output"));
                }
                Ok(())
            },
        )
        .unwrap()
    }
    fn assert_source(&self) {
        assert_eq!(
            fs::read(self.temp.path().join("source/original.bin")).unwrap(),
            b"source bytes"
        );
        assert_eq!(
            fs::read(self.temp.path().join("patch.ips")).unwrap(),
            b"reviewed package"
        );
    }
}

#[test]
fn complete_tree_publishes_and_undo_retains_bytes_for_resume() {
    let f = Fixture::new();
    let p = f.prepare();
    assert_eq!(inspect(&p.journal_path).unwrap(), TreePatchState::Staged);
    publish(&p.journal_path).unwrap();
    assert_eq!(inspect(&p.journal_path).unwrap(), TreePatchState::Published);
    assert_eq!(
        fs::read(f.plan.destination.join("nested/track.bin")).unwrap(),
        b"patched bytes"
    );
    assert!(publish(&p.journal_path).is_err());
    undo(&p.journal_path).unwrap();
    assert!(!f.plan.destination.exists());
    assert_eq!(inspect(&p.journal_path).unwrap(), TreePatchState::Staged);
    publish(&p.journal_path).unwrap();
    f.assert_source();
}

#[test]
fn failed_partial_production_or_failed_verification_never_publishes() {
    for fail_verify in [false, true] {
        let f = Fixture::new();
        let result = prepare(
            &f.plan,
            |root| {
                fs::write(root.join("only-first-component"), b"partial")?;
                if fail_verify {
                    Ok(())
                } else {
                    Err(refuse("second component failed"))
                }
            },
            |_| Err(refuse("component set incomplete")),
        );
        assert!(result.unwrap_err().to_string().contains("staging retained"));
        assert!(!f.plan.destination.exists());
        f.assert_source();
    }
}

#[test]
fn verification_must_be_read_only() {
    let f = Fixture::new();
    assert!(
        prepare(
            &f.plan,
            |p| fs::write(p.join("x"), b"before"),
            |p| fs::write(p.join("x"), b"after")
        )
        .is_err()
    );
    assert!(!f.plan.destination.exists());
}

#[test]
fn stale_source_patch_and_added_component_refuse_before_publication() {
    for which in 0..3 {
        let f = Fixture::new();
        let p = f.prepare();
        match which {
            0 => fs::write(f.temp.path().join("source/original.bin"), b"modified"),
            1 => fs::write(f.temp.path().join("patch.ips"), b"modified"),
            _ => fs::write(f.temp.path().join("source/extra.bin"), b"added"),
        }
        .unwrap();
        assert!(publish(&p.journal_path).is_err());
        assert!(!f.plan.destination.exists());
    }
}

#[test]
fn stale_preview_refuses_before_producer_runs() {
    let f = Fixture::new();
    fs::write(f.temp.path().join("patch.ips"), b"changed").unwrap();
    assert!(prepare(&f.plan, |_| panic!("must not produce"), |_| Ok(())).is_err());
}

#[test]
fn collision_after_sealing_never_clobbers_even_empty_directory_or_symlink() {
    for symlinked in [false, true] {
        let f = Fixture::new();
        let p = f.prepare();
        if symlinked {
            symlink(f.temp.path().join("missing"), &f.plan.destination).unwrap();
        } else {
            fs::create_dir(&f.plan.destination).unwrap();
        }
        assert!(publish(&p.journal_path).is_err());
        assert!(fs::symlink_metadata(&f.plan.destination).is_ok());
        f.assert_source();
    }
}

#[test]
fn crash_after_rename_is_recognized_without_completion_checkpoint() {
    let f = Fixture::new();
    let p = f.prepare();
    let (_, receipt) = load(&p.journal_path).unwrap();
    rename_noreplace(&receipt.staging, &receipt.plan.destination).unwrap();
    assert_eq!(inspect(&p.journal_path).unwrap(), TreePatchState::Published);
    undo(&p.journal_path).unwrap();
    f.assert_source();
}

#[test]
fn complete_membership_and_identity_required_for_recovery() {
    for change in 0..4 {
        let f = Fixture::new();
        let p = f.prepare();
        publish(&p.journal_path).unwrap();
        let target = f.plan.destination.join("nested/track.bin");
        match change {
            0 => fs::write(&target, b"changed").unwrap(),
            1 => fs::remove_file(&target).unwrap(),
            2 => fs::write(f.plan.destination.join("foreign"), b"foreign").unwrap(),
            _ => {
                fs::rename(&target, f.temp.path().join("old")).unwrap();
                fs::write(&target, b"patched bytes").unwrap();
            }
        }
        assert!(inspect(&p.journal_path).is_err());
        assert!(undo(&p.journal_path).is_err());
        assert!(f.plan.destination.exists());
    }
}

#[test]
fn links_escape_and_special_members_are_refused() {
    let f = Fixture::new();
    assert!(
        TreePatchPlan::review(
            &[f.temp.path().join("source")],
            &f.temp.path().join("source/out")
        )
        .is_err()
    );
    for hard in [false, true] {
        assert!(
            prepare(
                &f.plan,
                |root| {
                    let original = f.temp.path().join("source/original.bin");
                    if hard {
                        fs::hard_link(original, root.join("escape"))
                    } else {
                        symlink(original, root.join("escape"))
                    }
                },
                |_| Ok(())
            )
            .is_err()
        );
    }
    assert!(!f.plan.destination.exists());
    f.assert_source();
}

#[test]
fn substituted_parent_and_forged_journal_paths_refuse() {
    let f = Fixture::new();
    let p = f.prepare();
    let (_, mut receipt) = load(&p.journal_path).unwrap();
    receipt.plan.parent.1 += 1;
    fs::write(&p.journal_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(publish(&p.journal_path).is_err());
    receipt.plan.parent.1 -= 1;
    receipt.plan.destination = f.temp.path().join("../escape");
    fs::write(&p.journal_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(publish(&p.journal_path).is_err());
}

#[test]
fn partial_and_symlink_journals_fail_closed() {
    let f = Fixture::new();
    let p = f.prepare();
    let old = p.journal_path.with_extension("saved");
    fs::rename(&p.journal_path, &old).unwrap();
    symlink(&old, &p.journal_path).unwrap();
    assert!(publish(&p.journal_path).is_err());
    fs::remove_file(&p.journal_path).unwrap();
    fs::write(&p.journal_path, b"{").unwrap();
    assert!(publish(&p.journal_path).is_err());
    assert!(!f.plan.destination.exists());
}

#[test]
fn receipt_lock_serializes_publish_and_undo() {
    let f = Fixture::new();
    let p = f.prepare();
    let (lease, _) = load(&p.journal_path).unwrap();
    assert!(publish(&p.journal_path).is_err());
    drop(lease);
    publish(&p.journal_path).unwrap();
}

#[test]
fn existing_ips_preparation_composes_inside_tree_without_source_writes() {
    use crate::standalone_patch::{
        build_standalone_patch_apply_plan, inspect_standalone_patch,
        prepare_standalone_patch_output,
    };
    let f = Fixture::new();
    let patch = f.temp.path().join("patch.ips");
    fs::write(&patch, b"PATCH\0\0\0\0\x01XEOF").unwrap();
    let plan = TreePatchPlan::review(
        &[f.temp.path().join("source"), patch.clone()],
        &f.plan.destination,
    )
    .unwrap();
    let prepared = prepare(
        &plan,
        |root| {
            let staged_base = root.join("component with spaces.bin");
            fs::copy(f.temp.path().join("source/original.bin"), &staged_base)?;
            let inspection = inspect_standalone_patch(&patch).map_err(io::Error::other)?;
            let component = build_standalone_patch_apply_plan(
                &inspection,
                &staged_base,
                root.join("unused.bin"),
                root,
            )
            .map_err(io::Error::other)?;
            let bytes = prepare_standalone_patch_output(&component)
                .map_err(io::Error::other)?
                .bytes;
            fs::write(&staged_base, bytes)?;
            fs::write(root.join("unchanged.bin"), b"second component")
        },
        |root| {
            if fs::read(root.join("component with spaces.bin"))? != b"Xource bytes" {
                return Err(refuse("independent component proof failed"));
            }
            if fs::read(root.join("unchanged.bin"))? != b"second component" {
                return Err(refuse("component set incomplete"));
            }
            Ok(())
        },
    )
    .unwrap();
    publish(&prepared.journal_path).unwrap();
    assert_eq!(
        fs::read(f.temp.path().join("source/original.bin")).unwrap(),
        b"source bytes"
    );
    undo(&prepared.journal_path).unwrap();
}

#[test]
fn source_change_during_production_prevents_sealing() {
    let f = Fixture::new();
    let result = prepare(
        &f.plan,
        |root| {
            fs::write(root.join("component"), b"output")?;
            fs::write(f.temp.path().join("patch.ips"), b"concurrent change")
        },
        |_| Ok(()),
    );
    assert!(result.is_err());
    assert!(!f.plan.destination.exists());
}

#[test]
fn explicit_policy_accepts_sparse_input_above_default_without_buffering_it() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("optical-component.bin");
    let logical_bytes = DEFAULT_MAX_TOTAL_BYTES + 1;
    File::create(&source)
        .unwrap()
        .set_len(logical_bytes)
        .unwrap();
    let output = temp.path().join("output");
    let inputs = [source.clone()];
    assert!(
        TreePatchPlan::review(&inputs, &output)
            .unwrap_err()
            .to_string()
            .contains("byte bound")
    );
    let plan = TreePatchPlan::review_with_max_total_bytes(&inputs, &output, logical_bytes).unwrap();
    assert_eq!(plan.max_total_bytes, logical_bytes);
    let Entry::File(identity) = &plan.inputs[0].snapshot.0[Path::new("")] else {
        panic!("full file identity required");
    };
    assert_eq!(identity.size_bytes, logical_bytes);
    assert!(identity.freshness.is_some());
    assert!(!output.exists());
}

#[test]
fn small_policy_bounds_combined_inputs_and_staged_output_before_verification() {
    let f = Fixture::new();
    let inputs = [
        f.temp.path().join("source"),
        f.temp.path().join("patch.ips"),
    ];
    // Each input fits 20 bytes separately; their combined 28 bytes do not.
    assert!(TreePatchPlan::review_with_max_total_bytes(&inputs, &f.plan.destination, 20).is_err());
    let plan =
        TreePatchPlan::review_with_max_total_bytes(&inputs, &f.plan.destination, 32).unwrap();
    let error = prepare(
        &plan,
        |root| {
            fs::write(root.join("first"), [0; 17])?;
            fs::write(root.join("second"), [0; 16])
        },
        |_| panic!("oversized output must refuse before verification"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("byte bound"));
    assert!(!plan.destination.exists());
    assert!(
        !fs::read_dir(f.temp.path()).unwrap().any(|p| p
            .unwrap()
            .path()
            .extension()
            .is_some_and(|e| e == "json"))
    );
    f.assert_source();
}

#[test]
fn byte_policy_ceiling_is_enforced_at_review_and_receipt_load() {
    let f = Fixture::new();
    let inputs = [f.temp.path().join("source")];
    for limit in [0, HARD_MAX_TOTAL_BYTES + 1, u64::MAX] {
        assert!(
            TreePatchPlan::review_with_max_total_bytes(&inputs, &f.plan.destination, limit)
                .is_err()
        );
    }
    TreePatchPlan::review_with_max_total_bytes(&inputs, &f.plan.destination, HARD_MAX_TOTAL_BYTES)
        .unwrap();
    let p = f.prepare();
    let (_, mut receipt) = load(&p.journal_path).unwrap();
    receipt.plan.max_total_bytes = HARD_MAX_TOTAL_BYTES + 1;
    fs::write(&p.journal_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(inspect(&p.journal_path).is_err());
    assert!(publish(&p.journal_path).is_err());
    assert!(undo(&p.journal_path).is_err());
    assert!(!f.plan.destination.exists());
}

#[test]
fn byte_accounting_checks_overflow_and_exact_limits() {
    assert_eq!(add_bytes(31, 1, 32).unwrap(), 32);
    assert!(
        add_bytes(32, 1, 32)
            .unwrap_err()
            .to_string()
            .contains("byte bound")
    );
    assert!(
        add_bytes(u64::MAX, 1, HARD_MAX_TOTAL_BYTES)
            .unwrap_err()
            .to_string()
            .contains("overflow")
    );
}

#[test]
fn explicit_policy_survives_publication_undo_and_recovery() {
    let f = Fixture::new();
    let plan = TreePatchPlan::review_with_max_total_bytes(
        &[f.temp.path().join("source")],
        &f.plan.destination,
        40,
    )
    .unwrap();
    let p = prepare(
        &plan,
        |root| fs::write(root.join("component"), [0; 40]),
        |_| Ok(()),
    )
    .unwrap();
    let (_, mut receipt) = load(&p.journal_path).unwrap();
    assert_eq!(receipt.plan.max_total_bytes, 40);
    publish(&p.journal_path).unwrap();
    assert_eq!(inspect(&p.journal_path).unwrap(), TreePatchState::Published);
    undo(&p.journal_path).unwrap();
    assert_eq!(inspect(&p.journal_path).unwrap(), TreePatchState::Staged);
    // Recovery must use the persisted allowance, not the default.
    receipt.plan.max_total_bytes = 39;
    fs::write(&p.journal_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(inspect(&p.journal_path).is_err());
    assert!(publish(&p.journal_path).is_err());
    assert!(!plan.destination.exists());
    f.assert_source();
}

#[test]
fn receipts_without_a_byte_policy_keep_the_original_default() {
    let f = Fixture::new();
    let p = f.prepare();
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&p.journal_path).unwrap()).unwrap();
    receipt["plan"]
        .as_object_mut()
        .unwrap()
        .remove("max_total_bytes");
    fs::write(&p.journal_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert_eq!(
        load(&p.journal_path).unwrap().1.plan.max_total_bytes,
        DEFAULT_MAX_TOTAL_BYTES
    );
    publish(&p.journal_path).unwrap();
    undo(&p.journal_path).unwrap();
}
