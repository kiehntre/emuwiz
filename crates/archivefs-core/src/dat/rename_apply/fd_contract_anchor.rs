//! Trusted hardlink continuity from enrolled admission through Undo. No inode
//! generation number is treated as an immutable authority across restart.
use super::*;
pub(crate) const ENROLLED: &str = "r2_fd_anchor_v1";
pub(crate) fn path(root: &Path, tx: &RenameTransaction, index: usize) -> io::Result<PathBuf> {
    Ok(paths(root, tx, index)?.0.with_extension("anchor"))
}
pub(crate) struct Setup(Vec<Publisher>);
impl Setup {
    /// Retain actual verified FDs before publishing immutable authority. The
    /// caller holds admission. A crash before setup completion is a refusal,
    /// never permission to recreate an absent anchor from a library pathname.
    pub(crate) fn prepare(root: &Path, tx: &RenameTransaction) -> io::Result<Self> {
        let mut pins = Vec::new();
        for (index, entry) in tx.entries.iter().enumerate() {
            if !enrolled(entry) || entry.unknown.get(ENROLLED) != Some(&serde_json::json!(true)) {
                continue;
            }
            let anchor = path(root, tx, index)?;
            private_dir(&root.join(".emuwiz-r2-holding"), true)?;
            private_dir(anchor.parent().unwrap(), true)?;
            if !missing(&anchor) {
                return Err(io::Error::other("enrollment anchor already occupied"));
            }
            pins.push(Publisher::open(
                root,
                &entry.source_path,
                &anchor,
                &entry.identity,
            )?);
        }
        Ok(Self(pins))
    }
    /// Called only after the existing authoritative base and descriptor exist.
    pub(crate) fn publish(self) -> io::Result<()> {
        for pin in self.0 {
            match pin.publish(|| {})? {
                Publication::LinkedVerified => (),
                other => {
                    return Err(io::Error::other(format!(
                        "anchor setup incomplete: {other:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn verify_before_apply(root: &Path, tx: &RenameTransaction) -> io::Result<()> {
    for (index, entry) in tx.entries.iter().enumerate() {
        if enrolled(entry) && entry.unknown.get(ENROLLED) == Some(&serde_json::json!(true)) {
            let anchor = path(root, tx, index)?;
            Publisher::open(root, &anchor, &entry.source_path, &entry.identity)?;
        }
    }
    Ok(())
}
