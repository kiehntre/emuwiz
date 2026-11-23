//! Advanced Protection: the saved checks of the files of ONE enrolled
//! transaction, confirmed again at its Apply (owner approval 2026-11-23).
//!
//! Enrollment links a recovery anchor to every original (`<n>.anchor` in the
//! transaction's own holding folder). Making a hard link changes the file's
//! change time and link count, and nothing else, so the saved check of the
//! file no longer looks current and Apply refused its own enrollment.
//!
//! The change time is NOT ignored. Just before enrollment, the exact state of
//! each file (device, inode, size, modification time, change time, link
//! count, owner, group, mode and every extended attribute, which carries
//! ACLs, capabilities and security labels) is recorded with the saved checks
//! it has then ([`capture`]). At Apply ([`reattest`]) a check of that file is
//! re-pointed at the file's present change time only when all of these hold:
//!
//! - the anchor this transaction made is the same inode as the original;
//! - the link count is exactly the recorded count plus that one anchor;
//! - device, inode, size and modification time are the recorded ones, and
//!   owner, group, mode and extended attributes are byte-for-byte unchanged;
//! - the whole file, read again now, has every hash the saved check verified
//!   (and the content proof the recovery engine sealed);
//! - the saved check is still the one recorded at enrollment;
//! - the file is observed unchanged before and after it is read.
//!
//! Anything else refuses Apply with the reason and leaves the check stale.
//! Other files, other transactions and every other freshness rule are
//! untouched.
use super::*;
use crate::identity_source::hashing::FileFingerprint;
use std::os::unix::ffi::OsStrExt;

/// The recorded state of one file just before enrollment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileState {
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub changed_seconds: i64,
    pub changed_nanoseconds: i64,
    pub links: u64,
    pub owner: u32,
    pub group: u32,
    pub mode: u32,
    /// Every extended attribute (name, value), sorted by name.
    pub attributes: Vec<(Vec<u8>, Vec<u8>)>,
}

impl FileState {
    pub fn observe(path: &Path) -> std::result::Result<Self, String> {
        let m = fs::symlink_metadata(path).map_err(|e| format!("cannot be read ({e})"))?;
        if !m.file_type().is_file() {
            return Err("is no longer a regular file".into());
        }
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
            size_bytes: m.len(),
            modified_seconds: m.mtime(),
            modified_nanoseconds: m.mtime_nsec(),
            changed_seconds: m.ctime(),
            changed_nanoseconds: m.ctime_nsec(),
            links: m.nlink(),
            owner: m.uid(),
            group: m.gid(),
            mode: m.mode(),
            attributes: attributes(path)?,
        })
    }

    /// What differs from `then`, other than the change time and link count
    /// (which the caller accounts for itself).
    fn difference(&self, then: &Self) -> Option<&'static str> {
        if (self.device, self.inode) != (then.device, then.inode) {
            Some("it is another file now")
        } else if self.size_bytes != then.size_bytes
            || (self.modified_seconds, self.modified_nanoseconds)
                != (then.modified_seconds, then.modified_nanoseconds)
        {
            Some("its size or modification time changed")
        } else if (self.owner, self.group) != (then.owner, then.group) {
            Some("its owner or group changed")
        } else if self.mode != then.mode {
            Some("its permissions changed")
        } else if self.attributes != then.attributes {
            Some("its extended attributes (ACL, capability or security label) changed")
        } else {
            None
        }
    }
}

/// Every extended attribute of `path` itself (never a link target).
fn attributes(path: &Path) -> std::result::Result<Vec<(Vec<u8>, Vec<u8>)>, String> {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "has an unusable name".to_string())?;
    let failed = || {
        format!(
            "its extended attributes cannot be read ({})",
            std::io::Error::last_os_error()
        )
    };
    // SAFETY: a null buffer of size 0 asks only for the size.
    let size = unsafe { libc::llistxattr(c.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        return Err(failed());
    }
    let mut names = vec![0u8; size as usize];
    // SAFETY: the buffer is exactly `names.len()` bytes.
    let size = unsafe { libc::llistxattr(c.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if size < 0 || size as usize != names.len() {
        return Err("its extended attributes changed while being read".into());
    }
    let mut out = Vec::new();
    for name in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let n = std::ffi::CString::new(name).map_err(|_| failed())?;
        // SAFETY: as above, size query then an exactly sized buffer.
        let len = unsafe { libc::lgetxattr(c.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0) };
        if len < 0 {
            return Err(failed());
        }
        let mut value = vec![0u8; len as usize];
        let got = unsafe {
            libc::lgetxattr(
                c.as_ptr(),
                n.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        if got < 0 || got as usize != value.len() {
            return Err("its extended attributes changed while being read".into());
        }
        out.push((name.to_vec(), value));
    }
    out.sort();
    Ok(out)
}

/// One file of the transaction, as it was just before enrollment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrolledFile {
    /// Its entry in the transaction (the anchor is `<index>.anchor`).
    pub index: usize,
    pub path: PathBuf,
    /// The sealed catalogue binding of the entry (database and row).
    binding: serde_json::Value,
    pub state: FileState,
    /// The saved checks current then, each with its exact recorded proof.
    checks: Vec<(i64, FileFingerprint)>,
}

/// The files of one transaction whose saved checks may be confirmed again
/// at its Apply.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrolledBaseline {
    pub version: u32,
    pub transaction_id: String,
    pub files: Vec<EnrolledFile>,
}

/// Records the files of `transaction` (the preview's, with its sealed
/// bindings) and their current saved checks, just before enrollment.
/// `transaction_id` is the recovery engine's own id for the same plan.
pub fn capture(
    transaction: &RenameTransaction,
    transaction_id: &str,
) -> std::result::Result<EnrolledBaseline, String> {
    let mut files = Vec::new();
    for (index, entry) in transaction.entries.iter().enumerate() {
        let Some(value) = entry.unknown.get(KEY) else {
            continue;
        };
        let binding: Binding = serde_json::from_value(value.clone())
            .map_err(|_| "the preview's library binding cannot be read".to_string())?;
        if binding.proofs.is_empty() {
            continue;
        }
        binding.verify_storage()?;
        let state = FileState::observe(&entry.source_path)
            .map_err(|why| format!("{}: {why}", entry.source_path.display()))?;
        let c = open_read_only_connection(&binding.database).map_err(|e| e.to_string())?;
        let mut checks = Vec::new();
        for carry in &binding.proofs {
            let Some(facts) = stored_facts(&c, carry.row, binding.archive)? else {
                continue;
            };
            let Some(proof) = carry.recorded(&facts, &entry.source_path) else {
                continue;
            };
            // Only a check that describes the file exactly as it is now.
            if proof.object.is_some_and(|o| {
                (o.changed_seconds, o.changed_nanoseconds)
                    == (state.changed_seconds, state.changed_nanoseconds)
            }) && FileFingerprint::observe(&entry.source_path).as_ref() == Some(&proof)
            {
                checks.push((carry.row, proof));
            }
        }
        if !checks.is_empty() {
            files.push(EnrolledFile {
                index,
                path: entry.source_path.clone(),
                binding: value.clone(),
                state,
                checks,
            });
        }
    }
    Ok(EnrolledBaseline {
        version: 1,
        transaction_id: transaction_id.into(),
        files,
    })
}

fn stored_facts(
    c: &Connection,
    row: i64,
    archive: i64,
) -> std::result::Result<Option<serde_json::Value>, String> {
    let stored: Option<Vec<u8>> = c
        .query_row(
            "SELECT facts_json FROM library_dat_identities WHERE id=?1 AND archive_id=?2",
            params![row, archive],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(stored.and_then(|s| serde_json::from_slice(&s).ok()))
}

/// Every hash the saved check verified agrees with the file read now, and
/// at least one of them is cryptographic.
fn hashes_agree(
    facts: &serde_json::Value,
    fresh: &crate::identity_source::hashing::LocalHashes,
    sha256: &str,
) -> bool {
    let audited = &facts["audited_hashes"];
    let mut strong = false;
    for (key, now) in [
        ("sha1", fresh.sha1.as_str()),
        ("md5", fresh.md5.as_str()),
        ("sha256", sha256),
        ("crc32", fresh.crc32.as_str()),
    ] {
        match audited.get(key) {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(v))
                if !now.is_empty() && v.eq_ignore_ascii_case(now) =>
            {
                strong |= key != "crc32";
            }
            Some(_) => return false,
        }
    }
    let size_ok = match audited.get("size_bytes") {
        None | Some(serde_json::Value::Null) => true,
        Some(v) => v.as_u64() == Some(fresh.fingerprint.size_bytes),
    };
    strong && size_ok
}

/// Confirms the files of `baseline` again for the Apply of `operator`
/// (the recovery engine's enrolled transaction, rooted at `root`) and
/// re-points their saved checks at their present change time. Returns how
/// many checks were re-pointed (0 when they were already current). Any
/// difference refuses with the file and the reason; nothing is written then.
pub fn reattest(
    baseline: &EnrolledBaseline,
    operator: &RenameTransaction,
    root: &Path,
) -> std::result::Result<usize, String> {
    if baseline.version != 1 || baseline.transaction_id != operator.transaction_id {
        return Err("the enrollment record belongs to another operation".into());
    }
    let mut confirmed = Vec::new();
    for file in &baseline.files {
        let name = file.path.display();
        let refuse = |why: &str| {
            format!(
                "Advanced Protection could not confirm that {name} is unchanged since it was \
                 prepared: {why}. Nothing was changed. Check the file, then check the game again \
                 and preview again."
            )
        };
        let Some(entry) = operator.entries.get(file.index) else {
            return Err(refuse("it is not part of this operation"));
        };
        if entry.source_path != file.path {
            return Err(refuse("it is not part of this operation"));
        }
        let anchor =
            crate::dat::rename_apply::identity::fd_publication_probe::contract::anchor::path(
                root, operator, file.index,
            )
            .map_err(|e| refuse(&format!("its recovery anchor cannot be located ({e})")))?;
        let before = FileState::observe(&file.path).map_err(|why| refuse(&why))?;
        if let Some(why) = before.difference(&file.state) {
            return Err(refuse(why));
        }
        let held =
            fs::symlink_metadata(&anchor).map_err(|_| refuse("its recovery anchor is missing"))?;
        if !held.file_type().is_file() || (held.dev(), held.ino()) != (before.device, before.inode)
        {
            return Err(refuse("its recovery anchor is not the same file"));
        }
        if before.links != file.state.links + 1 {
            return Err(refuse(
                "it has a hard link other than this operation's recovery anchor",
            ));
        }
        let observed = FileFingerprint::observe(&file.path).ok_or_else(|| refuse("it vanished"))?;
        let fresh = crate::identity_source::hashing::hash_file(
            &file.path,
            &crate::safe_read::TrustedRoots::none(),
            None,
        )
        .map_err(|e| refuse(&format!("it could not be read in full ({})", e.detail())))?;
        let identity = crate::dat::rename_apply::identity::capture_identity(&file.path)
            .map_err(|e| refuse(&format!("it could not be read in full ({e})")))?;
        if !crate::dat::rename_apply::identity::identity_matches(&entry.identity, &identity) {
            return Err(refuse(
                "its content differs from the content that was prepared",
            ));
        }
        let sha256 = identity
            .freshness
            .as_ref()
            .map(|f| {
                f.sha256
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            })
            .unwrap_or_default();
        let after = FileState::observe(&file.path).map_err(|why| refuse(&why))?;
        if after != before
            || fresh.fingerprint != observed
            || FileFingerprint::observe(&file.path).as_ref() != Some(&observed)
        {
            return Err(refuse("it changed while it was being read"));
        }
        let binding: Binding = serde_json::from_value(file.binding.clone())
            .map_err(|_| refuse("its library binding cannot be read"))?;
        confirmed.push((file, binding, observed, fresh, sha256));
    }
    let Some(first) = confirmed.first() else {
        return Ok(0);
    };
    let database = first.1.database.clone();
    if confirmed
        .iter()
        .any(|(_, binding, ..)| binding.database != database)
    {
        return Err("the files of this operation belong to different game libraries".into());
    }
    let c = open_durable_connection(&database)?;
    c.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("the game library is busy ({e}). Nothing was changed."))?;
    let result = (|| {
        let mut changed = 0;
        for (file, binding, observed, fresh, sha256) in &confirmed {
            let refuse = |why: &str| {
                format!(
                    "Advanced Protection could not confirm the saved check of {}: {why}. Nothing \
                     was changed. Check the game again and preview again.",
                    file.path.display()
                )
            };
            binding.verify_storage().map_err(|e| refuse(&e))?;
            for (row, proof) in &file.checks {
                let stored: Option<Vec<u8>> = c
                    .query_row(
                        "SELECT facts_json FROM library_dat_identities WHERE id=?1 AND archive_id=?2",
                        params![row, binding.archive],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| refuse(&e.to_string()))?;
                let Some(stored) = stored else {
                    return Err(refuse("it was removed"));
                };
                let mut facts: serde_json::Value =
                    serde_json::from_slice(&stored).map_err(|_| refuse("it cannot be read"))?;
                let recorded: Option<FileFingerprint> = facts
                    .get("audit_file")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                if recorded.as_ref() == Some(observed) {
                    continue;
                }
                if recorded.as_ref() != Some(proof) {
                    return Err(refuse("the game was checked again since it was prepared"));
                }
                if !hashes_agree(&facts, fresh, sha256) {
                    return Err(refuse("the file does not have the verified hashes"));
                }
                facts["audit_file"] =
                    serde_json::to_value(observed).map_err(|e| refuse(&e.to_string()))?;
                let updated = serde_json::to_vec(&facts).map_err(|e| refuse(&e.to_string()))?;
                let n = c
                    .execute(
                        "UPDATE library_dat_identities SET facts_json=?1 WHERE id=?2 AND facts_json=?3",
                        params![updated, row, stored],
                    )
                    .map_err(|e| refuse(&e.to_string()))?;
                if n != 1 {
                    return Err(refuse("it changed while it was being confirmed"));
                }
                changed += 1;
            }
            binding.verify_storage().map_err(|e| refuse(&e))?;
            if !observed.still_current() {
                return Err(refuse("the file changed while it was being confirmed"));
            }
        }
        Ok(changed)
    })();
    match result {
        Ok(changed) => {
            c.execute_batch("COMMIT")
                .map_err(|e| format!("the game library could not record the check ({e})"))?;
            Ok(changed)
        }
        Err(why) => {
            let _ = c.execute_batch("ROLLBACK");
            Err(why)
        }
    }
}
