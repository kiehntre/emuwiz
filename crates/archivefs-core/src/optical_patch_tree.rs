//! Content verification/copying for the two optical adapters. Publication,
//! freshness binding and recovery belong exclusively to patch_output_recovery::tree.
use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::model::ObjectKind;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

pub(crate) fn refuse(message: impl std::fmt::Display) -> io::Error {
    io::Error::other(message.to_string())
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Content {
    pub size: u64,
    pub sha256: String,
}
impl Content {
    pub fn bytes(bytes: &[u8]) -> Self {
        Self {
            size: bytes.len() as u64,
            sha256: digest(bytes),
        }
    }
}
/// None represents a directory, including empty directories. No receipts here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Contents(pub BTreeMap<PathBuf, Option<Content>>);

pub(crate) fn file_content(path: &Path, limit: u64) -> io::Result<Content> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.len() > limit {
        return Err(refuse("regular file exceeds policy or is unsafe"));
    }
    let identity = capture_identity(path)?;
    if identity.kind != ObjectKind::RegularFile || identity.size_bytes > limit {
        return Err(refuse("file changed or exceeds policy"));
    }
    let hash = identity
        .freshness
        .ok_or_else(|| refuse("missing content identity"))?
        .sha256;
    Ok(Content {
        size: identity.size_bytes,
        sha256: hash.iter().map(|b| format!("{b:02x}")).collect(),
    })
}
impl Contents {
    pub fn read(root: &Path, limit: u64) -> io::Result<Self> {
        if !root.is_absolute()
            || root
                .components()
                .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
        {
            return Err(refuse("absolute confined source required"));
        }
        let mut ancestor = PathBuf::new();
        for part in root.components() {
            ancestor.push(part);
            if fs::symlink_metadata(&ancestor)?.file_type().is_symlink() {
                return Err(refuse("symlink source ancestor"));
            }
        }
        if !fs::symlink_metadata(root)?.is_dir() {
            return Err(refuse("source must be a directory"));
        }
        fn visit(
            root: &Path,
            rel: &Path,
            out: &mut Contents,
            total: &mut u64,
            limit: u64,
            depth: usize,
        ) -> io::Result<()> {
            if depth > 64 || out.0.len() >= 16_384 {
                return Err(refuse("source membership/depth limit"));
            }
            let path = root.join(rel);
            let m = fs::symlink_metadata(&path)?;
            if m.is_dir() {
                out.0.insert(rel.to_owned(), None);
                for entry in fs::read_dir(path)? {
                    visit(
                        root,
                        &rel.join(entry?.file_name()),
                        out,
                        total,
                        limit,
                        depth + 1,
                    )?;
                }
            } else {
                let remaining = limit
                    .checked_sub(*total)
                    .ok_or_else(|| refuse("source logical size limit"))?;
                let content = file_content(&path, remaining)?;
                *total = total
                    .checked_add(content.size)
                    .ok_or_else(|| refuse("source logical size overflow"))?;
                out.0.insert(rel.to_owned(), Some(content));
            }
            Ok(())
        }
        let mut result = Self(BTreeMap::new());
        visit(root, Path::new(""), &mut result, &mut 0, limit, 0)?;
        Ok(result)
    }
    pub fn size(&self) -> io::Result<u64> {
        self.0.values().flatten().try_fold(0u64, |n, f| {
            n.checked_add(f.size)
                .ok_or_else(|| refuse("tree size overflow"))
        })
    }
    pub fn fingerprint(&self) -> io::Result<String> {
        Ok(digest(&serde_json::to_vec(self).map_err(refuse)?))
    }
    pub fn file(&self, rel: &Path) -> io::Result<&Content> {
        self.0
            .get(rel)
            .and_then(Option::as_ref)
            .ok_or_else(|| refuse(format!("missing reviewed file: {}", rel.display())))
    }
    pub fn copy(&self, source: &Path, staging: &Path) -> io::Result<()> {
        for (rel, content) in &self.0 {
            if rel.as_os_str().is_empty() {
                continue;
            }
            let destination = staging.join(rel);
            match content {
                None => fs::create_dir(&destination)?,
                Some(content) => {
                    let mut input = File::open(source.join(rel))?.take(content.size + 1);
                    let mut output = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(destination)?;
                    if io::copy(&mut input, &mut output)? != content.size {
                        return Err(refuse("source size changed while copying"));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn verify(&self, root: &Path, limit: u64) -> io::Result<()> {
        if &Self::read(root, limit)? != self {
            return Err(refuse(
                "output membership/content differs from reviewed plan",
            ));
        }
        Ok(())
    }
}

/// Backend-independent publication/recovery/immutability contract, run by both
/// adapters against their own fixtures.
#[cfg(test)]
pub(crate) mod contract {
    use crate::patch_output_recovery::tree::{self, PreparedTreePatch, TreePatchState};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    pub(crate) type Fresh<'a> =
        &'a dyn Fn() -> (tempfile::TempDir, Vec<PathBuf>, PathBuf, PreparedTreePatch);

    /// Names, kinds and bytes of everything under `path` (a file is itself).
    pub(crate) fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(base: &Path, path: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            let meta = fs::symlink_metadata(path).unwrap();
            let key = path.strip_prefix(base).unwrap().display().to_string();
            if meta.is_dir() {
                out.insert(format!("{key}/"), Vec::new());
                for entry in fs::read_dir(path).unwrap() {
                    walk(base, &entry.unwrap().path(), out);
                }
            } else if meta.file_type().is_symlink() {
                out.insert(key, b"<symlink>".to_vec());
            } else {
                out.insert(key, fs::read(path).unwrap());
            }
        }
        let mut out = BTreeMap::new();
        if fs::symlink_metadata(path).unwrap().is_dir() {
            walk(path, path, &mut out);
        } else {
            out.insert(String::new(), fs::read(path).unwrap());
        }
        out
    }

    fn staging_of(journal: &Path) -> PathBuf {
        journal.with_file_name(journal.file_stem().unwrap())
    }

    fn inputs_unchanged(inputs: &[PathBuf], before: &[BTreeMap<String, Vec<u8>>], what: &str) {
        assert!(
            before.iter().all(|snap| !snap.is_empty()),
            "empty snapshot proves nothing"
        );
        let after: Vec<_> = inputs.iter().map(|path| snapshot(path)).collect();
        assert_eq!(after, before, "source/patch inputs changed: {what}");
    }

    pub(crate) fn published_changes_never_gain_undo_authority(fresh: Fresh<'_>) {
        use std::os::unix::fs::symlink;
        for mutation in [
            "same-size edit, mtime restored",
            "size change",
            "added file",
            "removed file",
            "added empty directory",
            "added symlink",
            "member replaced by symlink",
            "hardlink to member",
        ] {
            let (_temp, inputs, destination, prepared) = fresh();
            let before: Vec<_> = inputs.iter().map(|path| snapshot(path)).collect();
            tree::publish(&prepared.journal_path).unwrap();
            let published = snapshot(&destination);
            let member = destination.join(
                published
                    .keys()
                    .find(|key| !key.ends_with('/') && !key.is_empty())
                    .unwrap(),
            );
            match mutation {
                "same-size edit, mtime restored" => {
                    let modified = fs::metadata(&member).unwrap().modified().unwrap();
                    let mut bytes = fs::read(&member).unwrap();
                    bytes[0] ^= 1;
                    fs::write(&member, bytes).unwrap();
                    fs::File::options()
                        .write(true)
                        .open(&member)
                        .unwrap()
                        .set_modified(modified)
                        .unwrap();
                }
                "size change" => fs::write(&member, b"short").unwrap(),
                "added file" => fs::write(destination.join("extra"), b"x").unwrap(),
                "removed file" => fs::remove_file(&member).unwrap(),
                "added empty directory" => fs::create_dir(destination.join("emptydir")).unwrap(),
                "added symlink" => symlink("/etc/passwd", destination.join("link")).unwrap(),
                "member replaced by symlink" => {
                    fs::remove_file(&member).unwrap();
                    symlink("/etc/passwd", &member).unwrap();
                }
                _ => fs::hard_link(&member, destination.join("hardlink")).unwrap(),
            }
            let changed = snapshot(&destination);
            assert!(tree::inspect(&prepared.journal_path).is_err(), "{mutation}");
            assert!(tree::undo(&prepared.journal_path).is_err(), "{mutation}");
            assert!(tree::publish(&prepared.journal_path).is_err(), "{mutation}");
            // Refusal must leave the changed tree exactly where it was.
            assert_eq!(snapshot(&destination), changed, "{mutation}");
            assert!(!staging_of(&prepared.journal_path).exists(), "{mutation}");
            inputs_unchanged(&inputs, &before, mutation);
        }
    }

    pub(crate) fn lifecycle_after_interruption_and_stale_plans_keep_inputs_intact(
        fresh: Fresh<'_>,
        stale: &dyn Fn(&[PathBuf]),
    ) {
        // prepare -> staged; crash before rename; publish; undo; republish.
        let (_temp, inputs, destination, prepared) = fresh();
        let before: Vec<_> = inputs.iter().map(|path| snapshot(path)).collect();
        assert_eq!(
            tree::inspect(&prepared.journal_path).unwrap(),
            TreePatchState::Staged
        );
        tree::publish(&prepared.journal_path).unwrap();
        assert_eq!(
            tree::inspect(&prepared.journal_path).unwrap(),
            TreePatchState::Published
        );
        tree::undo(&prepared.journal_path).unwrap();
        assert!(!destination.exists());
        assert!(tree::undo(&prepared.journal_path).is_err());
        tree::publish(&prepared.journal_path).unwrap();
        inputs_unchanged(&inputs, &before, "publish/undo/republish");

        // crash immediately after the rename: only the rename happened.
        let (_temp, inputs, destination, prepared) = fresh();
        let before: Vec<_> = inputs.iter().map(|path| snapshot(path)).collect();
        fs::rename(staging_of(&prepared.journal_path), &destination).unwrap();
        assert_eq!(
            tree::inspect(&prepared.journal_path).unwrap(),
            TreePatchState::Published
        );
        assert!(tree::publish(&prepared.journal_path).is_err());
        tree::undo(&prepared.journal_path).unwrap();
        assert_eq!(
            tree::inspect(&prepared.journal_path).unwrap(),
            TreePatchState::Staged
        );
        inputs_unchanged(&inputs, &before, "post-rename recovery");

        // destination appears after review: refused, nothing overwritten.
        let (_temp, inputs, destination, prepared) = fresh();
        let before: Vec<_> = inputs.iter().map(|path| snapshot(path)).collect();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("owner"), b"mine").unwrap();
        assert!(tree::publish(&prepared.journal_path).is_err());
        assert_eq!(fs::read(destination.join("owner")).unwrap(), b"mine");
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
        assert_eq!(tree::inspect(&prepared.journal_path).is_err(), true);
        inputs_unchanged(&inputs, &before, "destination collision");

        // stale plan: the caller changes a reviewed input; publication refuses
        // and the staged tree is retained untouched.
        let (_temp, inputs, destination, prepared) = fresh();
        stale(&inputs);
        assert!(tree::publish(&prepared.journal_path).is_err());
        assert!(!destination.exists());
        assert!(staging_of(&prepared.journal_path).is_dir());
    }
}

#[cfg(test)]
mod tests {
    use crate::patch_output_recovery::tree::{self, PreparedTreePatch, TreePatchState};
    #[test]
    fn both_adapters_return_the_same_durable_tree_contract() {
        let (_dc_temp, dc, dc_destination) = crate::dreamcast_dcp_apply::tests::fixture();
        let (_saturn_temp, saturn, saturn_destination) =
            crate::saturn_patch_apply::tests::fixture();
        let prepared: [PreparedTreePatch; 2] = [dc.prepare().unwrap(), saturn.prepare().unwrap()];
        for (receipt, destination) in prepared.iter().zip([dc_destination, saturn_destination]) {
            assert_eq!(
                tree::inspect(&receipt.journal_path).unwrap(),
                TreePatchState::Staged
            );
            tree::publish(&receipt.journal_path).unwrap();
            assert!(destination.is_dir());
            assert_eq!(
                tree::inspect(&receipt.journal_path).unwrap(),
                TreePatchState::Published
            );
            tree::undo(&receipt.journal_path).unwrap();
            assert!(!destination.exists());
            assert_eq!(
                tree::inspect(&receipt.journal_path).unwrap(),
                TreePatchState::Staged
            );
        }
    }
}
