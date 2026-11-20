//! Launch seam for games the recovery engine has prepared a verified copy of
//! (see `archivefs_core::recovery`).
//!
//! A game stored in a form no emulator starts (a CD image set inside a ZIP)
//! keeps its library entry; when a verified prepared copy exists for that
//! exact, unchanged file, readiness plans the launch from the copy instead.
//! Nothing here establishes identity or a system: the game still goes through
//! the unverified-media lane, with the same confirmation a folder-only system
//! always needs, and every click-time check runs on the prepared file.

use std::path::{Path, PathBuf};

/// The verified prepared copy readiness may plan a launch from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedLaunchCopy {
    pub(crate) launch_path: PathBuf,
    /// The one system this copy was prepared for. A try as any other system
    /// is never offered for it.
    pub(crate) platform_id: &'static str,
}

/// How long one answer is reused. The Launch page asks on every frame while
/// it shows the confirmation; the answer only changes when a copy is prepared
/// or removed, which the person does on another page.
const ANSWER_LIFETIME: std::time::Duration = std::time::Duration::from_millis(750);

static LAST: std::sync::Mutex<Option<Remembered>> = std::sync::Mutex::new(None);

/// Drops the remembered answer; called when a copy was just prepared or
/// removed so the next question is answered from disk.
pub(crate) fn forget() {
    if let Ok(mut last) = LAST.lock() {
        *last = None;
    }
}

type Remembered = (
    PathBuf,
    &'static str,
    std::time::Instant,
    Option<PreparedLaunchCopy>,
);

/// Two string checks rule out every game the engine has no preparation for.
/// Otherwise the answer comes from one small index record, the file's own
/// metadata and a handful of `stat` calls (the ZIP is never read here), and
/// is reused for a moment so drawing a page never repeats them per frame.
pub(crate) fn for_game(path: &Path, recorded: Option<&str>) -> Option<PreparedLaunchCopy> {
    let platform = recorded.and_then(archivefs_core::canonical_platform_for_alias)?;
    if !archivefs_core::recovery::preparation_may_apply(path, Some(platform)) {
        return None;
    }
    if let Ok(last) = LAST.lock()
        && let Some((last_path, last_platform, at, answer)) = last.as_ref()
        && last_path == path
        && *last_platform == platform
        && at.elapsed() < ANSWER_LIFETIME
    {
        return answer.clone();
    }
    let answer = look_up(path, platform);
    if let Ok(mut last) = LAST.lock() {
        *last = Some((
            path.to_path_buf(),
            platform,
            std::time::Instant::now(),
            answer.clone(),
        ));
    }
    answer
}

fn look_up(path: &Path, platform: &'static str) -> Option<PreparedLaunchCopy> {
    let store = archivefs_core::recovery::PreparedMediaStore::for_profile().ok()?;
    let summary = store.prepared_for_source(path)?;
    (summary.platform_id == platform).then_some(PreparedLaunchCopy {
        launch_path: summary.launch_path,
        platform_id: platform,
    })
}

/// Provenance text of a launch content reference built from a prepared copy.
pub(crate) const PREPARED_COPY_PROVENANCE: &str =
    "verified prepared copy of a disc stored in a ZIP (recovery engine); the ZIP is not opened";
