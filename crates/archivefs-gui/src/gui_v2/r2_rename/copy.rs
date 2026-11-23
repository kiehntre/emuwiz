pub(super) const STANDARD: &str = "Standard Rename (recommended)";
pub(super) const ADVANCED: &str = "Advanced Protection";
pub(super) const COMPARISON: &str = "Standard Rename keeps the existing reviewed preview, freshness checks and Undo. It is the recommended everyday choice. Advanced Protection adds durable original-file anchors, transaction-bound one-shot approval, conservative interrupted recovery and verified ownership release. Its extra checks take longer.";
pub(super) const SCOPE: &str = "Advanced Protection eligibility: local ext4 with 4096-byte blocks; one to eight direct regular files in one folder; supported RenameMove only. Links, nested moves, other filesystems and larger batches are not supported.";
pub(super) const LIMITS: &str = "Advanced Protection does not prevent other programs modifying files and is not resistant to complete authority-store rollback. Keep other writers away during the operation; do not restore old recovery records over live state.";
pub(super) const MR_WIZ: &str = "Standard Rename is a good everyday choice. Advanced Protection is optional: it keeps extra identity and recovery evidence for a small rename batch, so it takes more time. I'll show the files and any limits before you decide. You can choose Standard Rename explicitly if this operation doesn't fit.";
pub(super) const BENCHMARKS: &str = "Measured examples, not guarantees: on the test host, warm-cache medians from two samples of a complete plan/review/Apply/Undo cycle were about 0.14 s with Standard Rename versus 0.88 s with Advanced Protection for one 1 MiB file; eight 1 MiB files took about 0.55 s versus 4.00 s. These are backend measurements, not GUI timing promises. File size, storage and other work change the time.";

/// Turns an operator refusal into what the user can do about it. The
/// operator's own words stay at the end for the record; refusals with no
/// known remedy are shown unchanged.
pub(super) fn explain_refusal(refusal: &str, root: Option<&std::path::Path>) -> String {
    use std::os::unix::fs::MetadataExt;
    let folder = root
        .map(|r| r.display().to_string())
        .unwrap_or_else(|| "the game folder".into());
    let quoted = root
        .map(|r| format!("'{}'", r.display().to_string().replace('\'', r"'\''")))
        .unwrap_or_else(|| "<game folder>".into());
    let mode = root
        .and_then(|r| std::fs::metadata(r).ok())
        .map(|m| format!(" (its permissions are {:o})", m.mode() & 0o777))
        .unwrap_or_default();
    let help = if refusal.contains("can be changed by other users") {
        format!(
            "Advanced Protection can't use {folder}: other users can change what is in it{mode}. \
             It needs a game folder that only you can change, so nobody else can add, swap or remove \
             a file during a protected rename. To allow it, remove write access for others once, \
             for example: chmod g-w,o-w {quoted} (others can still read the folder). \
             Or use Standard Rename, which does not need this."
        )
    } else if refusal.contains("game folder not owned by you") {
        format!(
            "Advanced Protection can't use {folder}: it belongs to another user. It only works on \
             game folders you own. Use Standard Rename, or ask the folder's owner to change its owner to you."
        )
    } else if refusal.contains("filesystem has not passed") {
        format!(
            "Advanced Protection can't use {folder}: it only supports game folders on a local ext4 \
             drive (4096-byte blocks). Use Standard Rename for this folder."
        )
    } else if refusal.contains("canonical game folder required")
        || refusal.contains("root must be canonical")
    {
        format!(
            "Advanced Protection can't use {folder}: it is reached through a link. Choose the real \
             folder it points to, or use Standard Rename."
        )
    } else if refusal.contains("Safety workspace must be canonical")
        || refusal.contains("Unsafe data-directory parent")
    {
        "Advanced Protection's safety folder in your EmuWiz data folder \
         (emuwiz-r2-blue) must be a real folder that only you can open (permissions 700) and that \
         you own. Fix its permissions, for example chmod 700 on it, then try again."
            .to_string()
    } else if refusal.contains("private owner-only fixture required")
        || refusal.contains("holding directory is not private and owned")
        || refusal.contains("storage binding or ownership changed")
    {
        // May come from any step, including after changes: never claims
        // that nothing changed.
        return format!(
            "A recovery folder that Advanced Protection made (inside {folder} or in its safety \
             folder) is no longer private to you: its owner or permissions changed. Do not delete \
             it; it may be needed to undo or recover. Set it back to owner-only (chmod 700) or \
             review it, and open the existing safety history to see whether any operation \
             still needs to finish, before using Advanced Protection again. (Details: {refusal})"
        );
    } else {
        return refusal.to_string();
    };
    // Every case above is a folder precondition, checked before anything
    // is enrolled or renamed.
    format!("{help} Nothing was changed. (Details: {refusal})")
}
