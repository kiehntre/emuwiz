//! Thin operator-process conveniences. These files are locators, never approval authority.
use archivefs_core::dat::rename_apply::{
    compact_recovery::deployment as d, identity::capture_identity,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{DirBuilderExt, MetadataExt},
    },
    path::Path,
};
fn fail(s: impl ToString) -> io::Error {
    io::Error::other(s.to_string())
}
fn private(path: &Path, uid: u32) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != uid || m.mode() & 0o077 != 0 || fs::canonicalize(path)? != path {
        return Err(fail(
            "Safety workspace must be canonical, private and owned by the collection operator",
        ));
    }
    Ok(())
}
pub(super) fn info(root: &Path, workspace: &Path) -> io::Result<Value> {
    let uid = fs::metadata(root)?.uid();
    private(workspace, uid)?;
    let mut numbers = Vec::new();
    for e in fs::read_dir(workspace)? {
        let e = e?;
        let n = e.file_name();
        let n = n.to_string_lossy();
        if let Some(s) = n.strip_prefix("state-") {
            numbers.push(s.parse::<u32>().map_err(|_| {
                fail("Unknown transaction directory; preserve workspace for review")
            })?);
        }
        if numbers.len() > 256 {
            return Err(fail(
                "Safety workspace history limit reached; retain all evidence",
            ));
        }
    }
    numbers.sort_unstable();
    if numbers.is_empty() || numbers.iter().enumerate().any(|(i, n)| i as u32 != *n) {
        return Err(fail(
            "Missing or ambiguous transaction directories; no state is selected automatically",
        ));
    }
    let index = *numbers.last().unwrap();
    let state = workspace.join(format!("state-{index}"));
    let configured = d::configured_root(&state).map_err(|e| fail(format!("Cannot open transaction {}: {e}. Preserve all state. If this is interrupted root reuse, operator recovery must review preview-reuse with predecessor {} and the SAME next state {}; then explicitly confirm reuse. Blue does not automatically adopt or delete prepared state.", state.display(),workspace.join(format!("state-{}",index.saturating_sub(1))).display(),state.display())))?;
    if configured != root {
        return Err(fail("Safety state belongs to a different collection"));
    }
    Ok(
        json!({"state":state,"next_state":workspace.join(format!("state-{}",index+1)),"signer":workspace.join("operator"),"root":root,"status":d::status(&state)}),
    )
}
pub(super) fn init(root: &Path, workspace: &Path) -> io::Result<Value> {
    d::preview_root(root)?;
    let parent = workspace
        .parent()
        .ok_or_else(|| fail("Missing safety workspace parent"))?;
    let uid = fs::metadata(root)?.uid();
    if !parent.exists() {
        let grand = parent
            .parent()
            .ok_or_else(|| fail("Missing data directory"))?;
        if fs::canonicalize(grand)? != grand || fs::metadata(grand)?.uid() != uid {
            return Err(fail("Unsafe data-directory parent"));
        }
        fs::DirBuilder::new().mode(0o700).create(parent)?;
        fs::File::open(grand)?.sync_all()?;
    }
    private(parent, uid)?;
    if workspace.starts_with(root) || root.starts_with(workspace) {
        return Err(fail("Safety workspace and collection must be separate"));
    }
    fs::DirBuilder::new().mode(0o700).create(workspace)?;
    fs::File::open(parent)?.sync_all()?;
    private(workspace, uid)?;
    let key = d::issuer_init(&workspace.join("operator"))?;
    d::provision(
        root,
        &workspace.join("state-0"),
        &key,
        "Blue advanced safety operator",
        "Contract1-I",
    )?;
    info(root, workspace)
}
fn open_at(parent: &fs::File, name: &std::ffi::OsStr, flags: i32) -> io::Result<fs::File> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(name.as_bytes()).map_err(fail)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}
fn directory(path: &Path) -> io::Result<fs::File> {
    if !path.is_absolute() || fs::canonicalize(path)? != path {
        return Err(fail("Canonical directory required"));
    }
    let mut dir = fs::File::open("/")?;
    for part in path.components() {
        if let std::path::Component::Normal(name) = part {
            dir = open_at(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        }
    }
    Ok(dir)
}
pub(super) fn retrieve(
    state: &Path,
    index: usize,
    confirmation: &str,
    dest: &Path,
) -> io::Result<Value> {
    let st = d::status(state);
    require_verified_inventory(&serde_json::to_value(&st)?)?;
    let retained = st
        .entries
        .iter()
        .find(|e| e.index == index)
        .and_then(|e| e.retained_foreign.as_ref())
        .ok_or_else(|| fail("No current verified retained-file receipt for this entry"))?;
    let seal = hex(Sha256::digest(serde_json::to_vec(&serde_json::to_value(
        retained,
    )?)?));
    if seal != confirmation {
        return Err(fail("Retained-file evidence changed; review it again"));
    }
    let root = d::configured_root(state)?;
    copy_retained(&root, state, retained, dest)
}
fn copy_retained(
    root: &Path,
    state: &Path,
    retained: &d::RetainedFile,
    dest: &Path,
) -> io::Result<Value> {
    let parent = dest
        .parent()
        .ok_or_else(|| fail("Choose a new retrieval filename"))?;
    if !dest.is_absolute() || dest.starts_with(root) || dest.starts_with(state.parent().unwrap()) {
        return Err(fail(
            "Retrieve to a new filename outside collection and safety workspace",
        ));
    }
    let before = capture_identity(&retained.path)?;
    if before != retained.identity {
        return Err(fail("Retained file identity changed"));
    }
    let input_parent = directory(retained.path.parent().unwrap())?;
    let mut input = open_at(
        &input_parent,
        retained.path.file_name().unwrap(),
        libc::O_RDONLY,
    )?;
    let im = input.metadata()?;
    if (im.dev(), im.ino()) != (before.dev, before.ino) {
        return Err(fail("Retained path replaced while opening"));
    }
    let pinned_parent = directory(parent)?;
    let pm = pinned_parent.metadata()?;
    if pm.uid() != fs::metadata(root)?.uid() {
        return Err(fail("Retrieval directory must belong to this operator"));
    }
    let mut output = open_at(
        &pinned_parent,
        dest.file_name()
            .ok_or_else(|| fail("New filename required"))?,
        libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
    )?;
    let om = output.metadata()?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut remaining = before.size_bytes;
    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let n = input.read(&mut buffer[..limit])?;
        if n == 0 {
            return Err(fail("Retained file shortened; preserve partial retrieval"));
        }
        output.write_all(&buffer[..n])?;
        digest.update(&buffer[..n]);
        remaining -= n as u64;
    }
    let hash = hex(digest.finalize());
    output.sync_all()?;
    pinned_parent.sync_all()?;
    let current = directory(parent)?.metadata()?;
    let dest_meta = fs::symlink_metadata(dest)?;
    if capture_identity(&retained.path)? != before
        || hash != retained.sha256
        || (current.dev(), current.ino()) != (pm.dev(), pm.ino())
        || !dest_meta.is_file()
        || (dest_meta.dev(), dest_meta.ino()) != (om.dev(), om.ino())
    {
        return Err(fail(
            "Retrieval verification failed; preserve original and partial copy",
        ));
    }
    output.seek(SeekFrom::Start(0))?;
    let mut verify = Sha256::new();
    io::copy(&mut output, &mut DigestWriter(&mut verify))?;
    if hex(verify.finalize()) != retained.sha256 {
        return Err(fail(
            "Retrieved copy verification failed; preserve it for review",
        ));
    }
    Ok(
        json!({"copied":true,"destination":dest,"sha256":hash,"original_preserved":true,"new_inode":true}),
    )
}
struct DigestWriter<'a>(&'a mut Sha256);
impl Write for DigestWriter<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.update(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn require_verified_inventory(status: &Value) -> io::Result<()> {
    match status["state"].as_str() {
        Some(
            "applied"
            | "apply_incomplete"
            | "undo_incomplete"
            | "settled"
            | "settled_awaiting_release",
        ) => Ok(()),
        _ => Err(fail(format!(
            "Retained inventory is not verified: {}",
            status["reasons"]
        ))),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refused_inventory_never_authorises_retrieval() {
        for state in [
            "review_required",
            "busy",
            "not_enrolled",
            "awaiting_apply",
            "unknown",
        ] {
            assert!(require_verified_inventory(&json!({"state":state})).is_err());
        }
        assert!(require_verified_inventory(&json!({"state":"settled"})).is_ok());
    }
    #[test]
    fn retained_copy_is_verified_new_inode_and_never_clobbers() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("root");
        let state = t.path().join("private/state-0");
        fs::create_dir(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        let source = root.join("held");
        fs::write(&source, b"foreign retained data").unwrap();
        let identity = capture_identity(&source).unwrap();
        let receipt = d::RetainedFile {
            path: source.clone(),
            identity: identity.clone(),
            original_anchor: root.join("original"),
            signed_grant_nonce: "unit-receipt".into(),
            sha256: hex(Sha256::digest(b"foreign retained data")),
        };
        let dest = t.path().join("retrieved");
        assert_eq!(
            copy_retained(&root, &state, &receipt, &dest).unwrap()["copied"],
            true
        );
        assert_eq!(fs::read(&source).unwrap(), fs::read(&dest).unwrap());
        assert_ne!(fs::metadata(&dest).unwrap().ino(), identity.ino);
        assert!(copy_retained(&root, &state, &receipt, &dest).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"foreign retained data");
        fs::write(&source, b"changed").unwrap();
        let other = t.path().join("other");
        assert!(copy_retained(&root, &state, &receipt, &other).is_err());
        assert!(!other.exists());
    }
    #[test]
    fn retained_copy_refuses_symlink_parent_and_collection_destination() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("root");
        fs::create_dir(&root).unwrap();
        let source = root.join("held");
        fs::write(&source, b"foreign").unwrap();
        let receipt = d::RetainedFile {
            path: source.clone(),
            identity: capture_identity(&source).unwrap(),
            original_anchor: root.join("anchor"),
            signed_grant_nonce: "unit".into(),
            sha256: hex(Sha256::digest(b"foreign")),
        };
        let state = t.path().join("private/state-0");
        let alias = t.path().join("alias");
        std::os::unix::fs::symlink(t.path(), &alias).unwrap();
        assert!(copy_retained(&root, &state, &receipt, &alias.join("copy")).is_err());
        assert!(copy_retained(&root, &state, &receipt, &root.join("copy")).is_err());
        assert!(!root.join("copy").exists());
    }
    #[test]
    fn corrupt_or_missing_authority_creates_no_copy() {
        let t = tempfile::tempdir().unwrap();
        let dest = t.path().join("copy");
        assert!(retrieve(&t.path().join("missing"), 0, "invalid", &dest).is_err());
        assert!(!dest.exists());
    }
}
