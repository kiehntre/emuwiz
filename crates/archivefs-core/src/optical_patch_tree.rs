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
