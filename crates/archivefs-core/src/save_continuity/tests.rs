//! Tests for save continuity. Every save here is synthetic and lives in a
//! temporary folder; no test looks at a real emulator profile.

mod device;
mod discover;
mod observer;
mod session;
mod status;
mod store;

use super::model::InUse;
use super::store::{
    CaptureFile, CaptureReason, CaptureRequest, ContainedSave, InUseObserver, Provenance,
    SnapshotStore,
};
use std::fs;
use std::path::{Path, PathBuf};

/// An observer that always gives the same answer.
pub(super) struct Fixed(pub InUse);

impl InUseObserver for Fixed {
    fn observe(&self, _: &str, _: &Path) -> InUse {
        self.0
    }
}

pub(super) const CLOSED: Fixed = Fixed(InUse::NotDetected);

pub(super) fn provenance(device: &str, emulator: &str, root: &Path) -> Provenance {
    Provenance {
        device_id: device.to_string(),
        emulator: emulator.to_string(),
        profile: "native".to_string(),
        emulator_version: None,
        platform: None,
        source_root: root.to_path_buf(),
    }
}

pub(super) fn request(device: &str, files: &[&Path], time: i64) -> CaptureRequest {
    let root = files[0].parent().unwrap();
    CaptureRequest {
        provenance: provenance(device, "PCSX2", root),
        files: files
            .iter()
            .map(|path| CaptureFile {
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                source: path.to_path_buf(),
            })
            .collect(),
        contained_saves: Vec::new(),
        reason: CaptureReason::Manual,
        session_id: None,
        allow_unproven_closure: false,
        captured_unix: Some(time),
        structure: super::status::StructureCheck::NotChecked {
            why: "synthetic test file".to_string(),
        },
        preceding_session: super::status::PrecedingSession::unknown("no session in this test"),
    }
}

pub(super) fn save(key: &str, fingerprint: &str) -> ContainedSave {
    ContainedSave {
        key: key.to_string(),
        name: key.rsplit(':').next().unwrap().to_string(),
        product_code: None,
        title: None,
        fingerprint: fingerprint.to_string(),
    }
}

pub(super) struct Bench {
    pub dir: tempfile::TempDir,
    pub store: SnapshotStore,
}

impl Bench {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::open(&dir.path().join("store")).unwrap();
        fs::create_dir(dir.path().join("saves")).unwrap();
        Self { dir, store }
    }

    pub fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.path().join("saves").join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}

/// Every regular file under `root`, with its bytes: "did anything change?".
pub(super) fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                out.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}
