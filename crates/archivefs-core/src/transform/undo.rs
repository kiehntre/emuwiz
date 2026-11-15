//! Undo of a Complete relocation, as its own journalled transaction.
//!
//! Undo is offered only while every precondition still holds (I8): the
//! relocation is Complete, no other Undo of it has completed or is in flight,
//! the relocated file still has exactly the identity the relocation
//! published (a later user edit blocks Undo, I6), nothing occupies the
//! original location, the retained original still matches its recorded
//! identity, and the catalogue still names the relocated location.
//!
//! A cross-filesystem Undo restores the retained original by an atomic
//! rename on its own filesystem; it never copies back. The relocated copy is
//! then moved into a private directory and removed only while the restored
//! original is verified intact (it is a byte-identical duplicate).

use super::engine::{self, Context, EngineError, TxnState};
use super::fsops;
use super::journal::{self, JournalWriter};
use super::model::*;
use crate::dat::rename_apply::identity::{capture_identity, identity_matches};
use crate::dat::rename_apply::model::ObjectIdentity;

/// Refuses unless `of_txn` is Complete (with the recorded outcome checksum
/// when given) and no Undo other than `own` has completed or is pending.
pub(crate) fn forward_still_undoable(
    ctx: &Context<'_>,
    of_txn: &str,
    of_complete_sum: &str,
    own: &str,
) -> Result<(), String> {
    let loaded = engine::load(ctx, of_txn)?;
    let (state, _) = engine::status(ctx, of_txn);
    if state != TxnState::Complete || !matches!(loaded.plan.kind, PlanKind::Relocate) {
        return Err("only a Complete relocation can be undone".into());
    }
    let sum = complete_sum(&loaded).ok_or("the relocation has no recorded outcome")?;
    if !of_complete_sum.is_empty() && sum != of_complete_sum {
        return Err("the relocation's journal changed since Undo was planned".into());
    }
    for (id, state, plan) in engine::list(ctx) {
        if id == own || id == of_txn {
            continue;
        }
        let Some(plan) = plan else { continue };
        if let PlanKind::Undo { of_txn: target, .. } = &plan.kind
            && target == of_txn
        {
            match state {
                TxnState::Compensated { .. } => {}
                TxnState::Complete => return Err("this relocation was already undone".into()),
                _ => {
                    return Err(
                        "another Undo of this relocation is in progress or needs recovery".into(),
                    );
                }
            }
        }
    }
    Ok(())
}

fn complete_sum(loaded: &engine::Loaded) -> Option<String> {
    loaded.journal.records.iter().find_map(|r| match &r.event {
        Event::Outcome {
            outcome: Outcome::Complete,
        } => Some(r.sum.clone()),
        _ => None,
    })
}

fn published_identity(loaded: &engine::Loaded) -> Option<ObjectIdentity> {
    let plan = &loaded.plan;
    match plan.strategy {
        Strategy::AtomicRename => Some(plan.source_identity.clone()),
        Strategy::CopyVerifyRetire => loaded.journal.records.iter().find_map(|r| match &r.event {
            Event::Done {
                step,
                direction: Direction::Forward,
                evidence: StepEvidence::Identity { identity },
            } if matches!(plan.steps.get(*step), Some(Step::CopyToStaging { .. })) => {
                Some(identity.clone())
            }
            _ => None,
        }),
    }
}

/// Binds an Undo plan to the relocation it reverses, before every step it
/// executes: the relocation's recorded plan digest and outcome, and every
/// field the Undo planner copies from that relocation, must match exactly.
/// So an Undo can only ever reverse what its relocation did. In particular
/// the copy it discards is the one that relocation published, never some
/// other file that merely has the same content (I9).
pub(crate) fn undo_matches_relocation(ctx: &Context<'_>, undo: &Plan) -> Result<(), String> {
    let PlanKind::Undo {
        of_txn,
        of_plan_digest,
        of_complete_sum,
    } = &undo.kind
    else {
        return Ok(());
    };
    let refuse = |what: &str| {
        Err(format!(
            "the Undo plan does not match the relocation it reverses ({what}); nothing was changed"
        ))
    };
    let loaded = engine::load(ctx, of_txn)?;
    let fwd = &loaded.plan;
    if !matches!(fwd.kind, PlanKind::Relocate) {
        return refuse("it is not a relocation");
    }
    if *of_plan_digest != journal::hex(&loaded.journal.plan_digest) {
        return refuse("the relocation's plan digest");
    }
    match complete_sum(&loaded) {
        Some(sum) if !of_complete_sum.is_empty() && sum == *of_complete_sum => {}
        _ => return refuse("the relocation's recorded outcome"),
    }
    let checks: [(&str, bool); 12] = [
        ("catalogue", undo.catalogue == fwd.catalogue),
        (
            "catalogue binding",
            undo.catalogue_binding == fwd.catalogue_binding,
        ),
        ("source root", undo.source_root == fwd.source_root),
        (
            "destination root",
            undo.destination_root == fwd.destination_root,
        ),
        ("original location", undo.source_path == fwd.source_path),
        (
            "relocated location",
            undo.destination_path == fwd.destination_path,
        ),
        (
            "original identity",
            undo.source_identity == fwd.source_identity,
        ),
        ("strategy", undo.strategy == fwd.strategy),
        ("retention", undo.retention == fwd.retention),
        ("retained original", undo.retired_path == fwd.retired_path),
        (
            "filesystem policy",
            undo.filesystem_policy == fwd.filesystem_policy,
        ),
        (
            "published identity",
            undo.published_identity.is_some()
                && undo.published_identity == published_identity(&loaded),
        ),
    ];
    match checks.iter().find(|(_, ok)| !ok) {
        Some((what, _)) => refuse(what),
        None => Ok(()),
    }
}

/// Plans an Undo, or explains why Undo is not available now. Read-only.
pub fn plan_undo(ctx: &Context<'_>, txn_id: &str) -> Result<Plan, String> {
    let loaded = engine::load(ctx, txn_id)?;
    let fwd = &loaded.plan;
    let undo_id = journal::new_txn_id().map_err(|e| e.to_string())?;
    forward_still_undoable(ctx, txn_id, "", &undo_id)?;
    let sum = complete_sum(&loaded).ok_or("the relocation has no recorded outcome")?;
    let published =
        published_identity(&loaded).ok_or("the relocation recorded no published identity")?;
    let current = capture_identity(&fwd.destination_path)
        .map_err(|e| format!("the relocated file cannot be read: {e}"))?;
    if !identity_matches(&published, &current) {
        return Err(
            "the relocated file changed since it was relocated; Undo would discard that change"
                .into(),
        );
    }
    if !fsops::not_found(&fwd.source_path).map_err(|e| e.to_string())? {
        return Err(
            "something now exists at the original location; Undo will not overwrite it".into(),
        );
    }
    if let Some(retired) = &fwd.retired_path {
        let r = capture_identity(retired)
            .map_err(|e| format!("the retained original cannot be read: {e}"))?;
        if !identity_matches(&fwd.source_identity, &r) {
            return Err("the retained original no longer matches its recorded identity".into());
        }
    }
    let (steps, pnr) = super::shape::undo(
        fwd.strategy,
        &fwd.source_path,
        &fwd.destination_path,
        fwd.retired_path.as_deref(),
        &fwd.source_root.path,
        &fwd.destination_root.path,
        &undo_id,
    )?;
    Ok(Plan {
        format: PLAN_FORMAT.into(),
        version: PLAN_VERSION,
        engine: ENGINE.into(),
        txn_id: undo_id,
        kind: PlanKind::Undo {
            of_txn: txn_id.into(),
            of_plan_digest: journal::hex(&loaded.journal.plan_digest),
            of_complete_sum: sum,
        },
        created_at_unix: super::plan::now_unix(),
        catalogue: fwd.catalogue.clone(),
        catalogue_binding: fwd.catalogue_binding.clone(),
        source_root: fwd.source_root.clone(),
        destination_root: fwd.destination_root.clone(),
        source_path: fwd.source_path.clone(),
        destination_path: fwd.destination_path.clone(),
        source_identity: fwd.source_identity.clone(),
        strategy: fwd.strategy,
        steps,
        point_of_no_return: pnr,
        retention: fwd.retention,
        dependency_digest: fwd.dependency_digest.clone(),
        effects: vec![
            "restores the original to its library location and the catalogue to name it".into(),
        ],
        filesystems: {
            let mut v = Vec::new();
            for root in [&fwd.source_root.path, &fwd.destination_root.path] {
                v.push(super::fscompat::check_root(root, &[], fwd.filesystem_policy)?.0);
            }
            let catalogue_dir = fwd
                .catalogue
                .parent()
                .ok_or("the catalogue has no folder")?;
            let (facts, _) = super::fscompat::check_path(catalogue_dir, fwd.filesystem_policy)
                .map_err(|e| format!("the catalogue's drive: {e}"))?;
            v.push(facts);
            v
        },
        filesystem_policy: fwd.filesystem_policy,
        published_identity: Some(published),
        retired_path: fwd.retired_path.clone(),
    })
}

/// Why Undo is not offered for `txn_id` now, or `None` when it is (I8).
pub fn undo_blocker(ctx: &Context<'_>, txn_id: &str) -> Option<String> {
    plan_undo(ctx, txn_id).err()
}

/// After an Undo completes, records it in the relocation's own journal for
/// audit. Status never depends on this record (it scans Undo journals), so
/// a crash before it is harmless.
pub fn mark_undone(ctx: &Context<'_>, undo_plan: &Plan) -> Result<(), String> {
    let PlanKind::Undo { of_txn, .. } = &undo_plan.kind else {
        return Ok(());
    };
    let (state, _) = engine::status(ctx, &undo_plan.txn_id);
    if state != TxnState::Complete {
        return Ok(());
    }
    let dir = ctx.journal_root.join(of_txn);
    let _lock = super::lock::lock_transaction(&dir).map_err(|e| e.to_string())?;
    let loaded = journal::load(&ctx.journal_root, of_txn).map_err(|e| format!("{e:?}"))?;
    if loaded
        .events()
        .any(|e| matches!(e, Event::UndoneBy { undo_txn } if *undo_txn == undo_plan.txn_id))
    {
        return Ok(());
    }
    let mut writer = JournalWriter::reopen(&loaded, None).map_err(|e| e.to_string())?;
    writer
        .append(Event::UndoneBy {
            undo_txn: undo_plan.txn_id.clone(),
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Applies an Undo plan and records it on the relocation when it completes.
pub fn apply_undo(
    ctx: &Context<'_>,
    plan: &Plan,
    approved_digest: &str,
) -> Result<TxnState, EngineError> {
    let state = engine::apply(ctx, plan, approved_digest)?;
    if state == TxnState::Complete {
        let _ = mark_undone(ctx, plan);
    }
    Ok(state)
}

/// The relocations that a completed Undo has reversed.
pub fn undone_by(ctx: &Context<'_>, txn_id: &str) -> Option<String> {
    engine::list(ctx)
        .into_iter()
        .find_map(|(id, state, plan)| match plan?.kind {
            PlanKind::Undo { of_txn, .. } if of_txn == txn_id && state == TxnState::Complete => {
                Some(id)
            }
            _ => None,
        })
}
