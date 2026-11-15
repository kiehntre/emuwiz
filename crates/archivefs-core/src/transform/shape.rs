//! The only step sequences the engine executes.
//!
//! A plan's steps, point of no return and retained path are a pure function
//! of its kind, strategy, transaction id, the two library roots and the two
//! game paths. The planners build plans with these functions, and the
//! engine re-derives them and refuses any plan that differs (at apply, at
//! recovery and before every step). So an edited or forged plan, including
//! one read back from a journal, cannot rename or unlink anything the
//! planner would not have: every path a step touches lies inside one of the
//! two authorised roots, in the game's own folder or the engine's private
//! folder beside it.

use std::path::{Component, Path, PathBuf};

use super::model::{Expect, Plan, PlanKind, Step, Strategy};

pub(crate) fn plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// (steps, point of no return, retained original) of a relocation.
pub(crate) fn forward(
    strategy: Strategy,
    source: &Path,
    destination: &Path,
    sroot: &Path,
    droot: &Path,
    txn_id: &str,
) -> Result<(Vec<Step>, usize, Option<PathBuf>), String> {
    let source_parent = source.parent().ok_or("the source has no folder")?;
    let destination_dir = destination
        .parent()
        .ok_or("the destination has no folder")?;
    Ok(match strategy {
        Strategy::AtomicRename => (
            vec![
                Step::CheckPreconditions,
                Step::Rename {
                    from: source.into(),
                    to: destination.into(),
                    roots: vec![sroot.into(), droot.into()],
                    expect: Expect::Original,
                },
                Step::CommitCatalogue { reverse: false },
                Step::FinalVerify,
            ],
            2,
            None,
        ),
        Strategy::CopyVerifyRetire => {
            let staging = destination_dir.join(format!(".emuwiz-transform-{txn_id}"));
            let staged = staging.join("payload");
            let retired_dir = source_parent.join(format!(".emuwiz-retired-{txn_id}"));
            let retired = retired_dir.join("payload");
            (
                vec![
                    Step::CheckPreconditions,
                    Step::CreatePrivateDir {
                        dir: staging.clone(),
                        root: droot.into(),
                    },
                    Step::CreatePrivateDir {
                        dir: retired_dir,
                        root: sroot.into(),
                    },
                    Step::CopyToStaging {
                        staged: staged.clone(),
                    },
                    Step::VerifyStaged {
                        staged: staged.clone(),
                    },
                    Step::Rename {
                        from: staged,
                        to: destination.into(),
                        roots: vec![droot.into()],
                        expect: Expect::Staged,
                    },
                    Step::CommitCatalogue { reverse: false },
                    Step::Rename {
                        from: source.into(),
                        to: retired.clone(),
                        roots: vec![sroot.into()],
                        expect: Expect::Original,
                    },
                    Step::RemovePrivateDir {
                        dir: staging,
                        root: droot.into(),
                    },
                    Step::FinalVerify,
                ],
                7,
                Some(retired),
            )
        }
    })
}

/// (steps, point of no return) of the Undo of a relocation; `retired` is
/// the relocation's retained original.
pub(crate) fn undo(
    strategy: Strategy,
    source: &Path,
    destination: &Path,
    retired: Option<&Path>,
    sroot: &Path,
    droot: &Path,
    undo_id: &str,
) -> Result<(Vec<Step>, usize), String> {
    Ok(match strategy {
        Strategy::AtomicRename => (
            vec![
                Step::CheckPreconditions,
                Step::Rename {
                    from: destination.into(),
                    to: source.into(),
                    roots: vec![sroot.into(), droot.into()],
                    expect: Expect::Published,
                },
                Step::CommitCatalogue { reverse: true },
                Step::FinalVerify,
            ],
            2,
        ),
        Strategy::CopyVerifyRetire => {
            let retired = retired.ok_or("the relocation recorded no retained original")?;
            let retired_dir = retired
                .parent()
                .ok_or("invalid retained path")?
                .to_path_buf();
            let dest_parent = destination.parent().ok_or("invalid destination")?;
            let private = dest_parent.join(format!(".emuwiz-transform-{undo_id}"));
            let duplicate = private.join("payload");
            (
                vec![
                    Step::CheckPreconditions,
                    Step::Rename {
                        from: retired.into(),
                        to: source.into(),
                        roots: vec![sroot.into()],
                        expect: Expect::Original,
                    },
                    Step::CommitCatalogue { reverse: true },
                    Step::CreatePrivateDir {
                        dir: private.clone(),
                        root: droot.into(),
                    },
                    Step::Rename {
                        from: destination.into(),
                        to: duplicate.clone(),
                        roots: vec![droot.into()],
                        expect: Expect::Published,
                    },
                    Step::DiscardDuplicate {
                        path: duplicate,
                        keeper: source.into(),
                        root: droot.into(),
                    },
                    Step::RemovePrivateDir {
                        dir: private,
                        root: droot.into(),
                    },
                    Step::RemovePrivateDir {
                        dir: retired_dir,
                        root: sroot.into(),
                    },
                    Step::FinalVerify,
                ],
                5,
            )
        }
    })
}

/// Refuses a plan whose shape is not exactly what its own identity fields
/// imply, or whose paths are not plain absolute paths of a game directly
/// inside a folder of its library root.
pub(crate) fn validate(plan: &Plan) -> Result<(), String> {
    let refuse = |why: &str| {
        Err(format!(
            "the plan is not one the planner produces ({why}); nothing was changed"
        ))
    };
    let ids_ok = super::journal::valid_txn_id(&plan.txn_id)
        && match &plan.kind {
            PlanKind::Undo { of_txn, .. } => {
                super::journal::valid_txn_id(of_txn) && of_txn != &plan.txn_id
            }
            PlanKind::Relocate => true,
        };
    if !ids_ok {
        return refuse("a transaction id is malformed");
    }
    let (sroot, droot) = (&plan.source_root.path, &plan.destination_root.path);
    for (path, root) in [(&plan.source_path, sroot), (&plan.destination_path, droot)] {
        if !plain_absolute(path) || !plain_absolute(root) {
            return refuse("a path is not a plain absolute path");
        }
        let Ok(rel) = path.strip_prefix(root) else {
            return refuse("a game path lies outside its library root");
        };
        if rel.components().count() < 1 || path.file_name().is_none() {
            return refuse("a game path names no file in its root");
        }
    }
    if plan.source_path == plan.destination_path {
        return refuse("the source and destination are the same");
    }
    let (sp, dp) = (&plan.source_path, &plan.destination_path);
    let (steps, pnr, retired) = match &plan.kind {
        PlanKind::Relocate => forward(plan.strategy, sp, dp, sroot, droot, &plan.txn_id)?,
        PlanKind::Undo { of_txn, .. } => {
            // The retained original is exactly where that relocation put it.
            let retired = forward(plan.strategy, sp, dp, sroot, droot, of_txn)?.2;
            let (s, p) = undo(
                plan.strategy,
                sp,
                dp,
                retired.as_deref(),
                sroot,
                droot,
                &plan.txn_id,
            )?;
            (s, p, retired)
        }
    };
    if plan.steps != steps {
        return refuse("its steps differ");
    }
    if plan.point_of_no_return != pnr {
        return refuse("its point of no return differs");
    }
    if plan.retired_path != retired {
        return refuse("its retained-original path differs");
    }
    Ok(())
}
