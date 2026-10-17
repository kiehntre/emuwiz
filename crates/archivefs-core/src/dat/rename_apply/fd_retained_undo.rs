//! Only the distinct signed RetainForeign action may call `settle`.
//! No foreign unlink/return, no pathname-source publication, no last-anchor unlink.
use super::*;

pub(crate) fn verify_candidate(
    root: &Path,
    tx: &RenameTransaction,
    index: usize,
) -> io::Result<()> {
    let e = &tx.entries[index];
    let v = evidence(tx, index)?.ok_or_else(|| io::Error::other("retention evidence missing"))?;
    if v.action != Action::Undo || !matches!(v.phase, Phase::ForeignHeld | Phase::RetentionPending)
    {
        return Err(io::Error::other(
            "only recorded foreign Undo holding is eligible for explicit retention",
        ));
    }
    let foreign = v
        .held_identity
        .as_ref()
        .ok_or_else(|| io::Error::other("foreign identity missing"))?;
    if foreign.kind != crate::dat::rename_apply::ObjectKind::RegularFile
        || identity_matches(foreign, &e.identity)
    {
        return Err(io::Error::other(
            "foreign holding identity is invalid or is the original",
        ));
    }
    let (held, salvage) = paths(root, tx, index)?;
    private_dir(&root.join(".emuwiz-r2-holding"), false)?;
    private_dir(held.parent().unwrap(), false)?;
    let _foreign = Publisher::open(root, &held, &e.source_path, foreign)?;
    let _original = Publisher::open(
        root,
        &anchor::path(root, tx, index)?,
        &e.source_path,
        &e.identity,
    )?;
    if !missing(&e.destination_path)
        || !missing(&salvage)
        || (!missing(&e.source_path) && !expected_at(&e.source_path, &e.identity))
    {
        return Err(io::Error::other(
            "retention refuses occupied, changed or unavailable original/destination/salvage slots",
        ));
    }
    Ok(())
}

pub(crate) fn verify_settled(
    root: &Path,
    tx: &RenameTransaction,
    index: usize,
    v: &Evidence,
) -> io::Result<()> {
    if v.action != Action::Undo || v.phase != Phase::RetainedForeign || v.retention.is_none() {
        return Err(io::Error::other(
            "retention receipt missing or contradictory",
        ));
    }
    let e = &tx.entries[index];
    let (held, salvage) = paths(root, tx, index)?;
    let foreign = v
        .held_identity
        .as_ref()
        .ok_or_else(|| io::Error::other("retained identity missing"))?;
    if identity_matches(foreign, &e.identity) {
        return Err(io::Error::other("retained object is not foreign"));
    }
    private_dir(&root.join(".emuwiz-r2-holding"), false)?;
    private_dir(held.parent().unwrap(), false)?;
    let _held = Publisher::open(root, &held, &e.source_path, foreign)?;
    let _anchor = Publisher::open(
        root,
        &anchor::path(root, tx, index)?,
        &e.source_path,
        &e.identity,
    )?;
    let original = Publisher::open(root, &e.source_path, &e.destination_path, &e.identity)?;
    original.dirs.verify_bound()?;
    if !missing(&e.destination_path) || !missing(&salvage) {
        return Err(io::Error::other(
            "retained settlement conflicts with current slots",
        ));
    }
    Ok(())
}

pub(crate) fn settle(
    root: &Path,
    tx: &mut RenameTransaction,
    index: usize,
    approval: &RetentionApproval,
    persist: &mut impl FnMut(&mut RenameTransaction) -> io::Result<()>,
    hook: &mut impl FnMut(Edge) -> io::Result<()>,
) -> io::Result<()> {
    verify_candidate(root, tx, index)?;
    let e = tx.entries[index].clone();
    let (held, _) = paths(root, tx, index)?;
    let previous = evidence(tx, index)?.unwrap();
    let foreign = previous.held_identity.clone().unwrap();
    // Keep both descriptors and their trusted hardlinks alive throughout.
    let foreign_pin = Publisher::open(root, &held, &e.source_path, &foreign)?;
    let original = Publisher::open(
        root,
        &anchor::path(root, tx, index)?,
        &e.source_path,
        &e.identity,
    )?;
    let mut receipt = value(
        Phase::RetentionPending,
        Some(foreign.clone()),
        "Separately signed operator retention: preserve foreign holding object and original continuity anchor; Undo incomplete",
    );
    receipt.retention = Some(approval.clone());
    checkpoint(tx, index, receipt.clone(), Action::Undo, persist)?;
    hook(Edge::RetentionIntentRecorded)?;
    if missing(&e.source_path) {
        hook(Edge::BeforeRetainedPublish)?;
        match original.publish(|| {})? {
            Publication::LinkedVerified => (),
            result => {
                return Err(io::Error::other(format!(
                    "retention publication incomplete: {}; evidence retained",
                    result.detail()
                )));
            }
        }
        hook(Edge::AfterRetainedPublish)?;
    } else if !expected_at(&e.source_path, &e.identity) {
        return Err(io::Error::other(
            "original path changed; retention remains incomplete",
        ));
    }
    original.file.sync_all()?;
    foreign_pin.file.sync_all()?;
    original.dirs.sync_and_verify(false)?;
    foreign_pin.dirs.sync_and_verify(false)?;
    receipt.phase = Phase::RetainedForeign;
    receipt.detail="Original restored and verified; foreign object and original continuity anchor permanently retained under separately signed operator decision".into();
    verify_settled(root, tx, index, &receipt)?;
    hook(Edge::BeforeRetentionReceipt)?;
    checkpoint(tx, index, receipt, Action::Undo, persist)?;
    hook(Edge::RetentionReceiptRecorded)?;
    Ok(())
}
