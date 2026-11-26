//! Same-filesystem staging and failed-conversion evidence for PSP CSO
//! conversion (ALCHEMY Q1-A, owner-approved design 2026-11-24).
//!
//! Layout, on the filesystem that holds the output:
//!
//! ```text
//! <root>/.emuwiz-diagnostics/psp-cso/        (0700, owned by the user)
//!     staging/<id>/intent.json, output.cso    in-progress work
//!     failed/<id>/intent.json, output.cso, diagnostic.json
//!     manifest.jsonl                          one line per retained failure
//! ```
//!
//! The base directory is chosen by [`DiagnosticsPlacement`] in the owner's
//! preference order: a configured directory, EmuWiz's data directory, then
//! `<mount point>/.emuwiz-diagnostics`, always on the output's filesystem.
//! (The layout above shows the third form.) Staging there means a
//! successful CSO is published with a same-filesystem no-clobber hard link,
//! and a failed one is kept with a same-filesystem rename: nothing is copied
//! and nothing unpublished ever appears in a game folder.
//!
//! Nothing here deletes retained evidence. A stage left by an interrupted
//! run stays in `staging/` with its `intent.json` and is reported by
//! [`diagnostics_usage`] as interrupted. The only removals are the stage of a
//! *successful* conversion (its data lives on at the output path) and the
//! transient restored-ISO copies, whose hashes are recorded.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{DestinationProblem, PspShrinkError};

pub const DIAGNOSTICS_DIR_NAME: &str = ".emuwiz-diagnostics";
pub const PSP_CSO_DIAGNOSTICS: &str = "psp-cso";
const STAGING: &str = "staging";
const FAILED: &str = "failed";
const MANIFEST: &str = "manifest.jsonl";
const REPORT: &str = "diagnostic.json";
/// Space kept free beyond the computed need (filesystem metadata, reports).
pub const FREE_SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// A validated `<root>/.emuwiz-diagnostics/psp-cso` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsRoot {
    dir: PathBuf,
    device: u64,
    source: DiagnosticsSource,
    skipped: Vec<String>,
}

fn unavailable(message: String) -> PspShrinkError {
    PspShrinkError::DiagnosticsUnavailable(message)
}

fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// A directory EmuWiz may use for evidence: a real directory (not a
/// symlink), owned by the current user, and not writable by group or others.
fn check_private_dir(path: &Path) -> Result<fs::Metadata, PspShrinkError> {
    let meta = fs::symlink_metadata(path)
        .map_err(|error| unavailable(format!("{}: {error}", path.display())))?;
    if meta.file_type().is_symlink() {
        return Err(unavailable(format!(
            "{} is a symbolic link",
            path.display()
        )));
    }
    if !meta.is_dir() {
        return Err(unavailable(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    if meta.uid() != current_uid() {
        return Err(unavailable(format!(
            "{} is owned by another user",
            path.display()
        )));
    }
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(unavailable(format!(
            "{} is writable by other users",
            path.display()
        )));
    }
    Ok(meta)
}

/// Creates `path` (mode 0700) if it is missing, then checks it.
fn ensure_private_dir(path: &Path) -> Result<fs::Metadata, PspShrinkError> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(unavailable(format!(
                "could not create {}: {error}",
                path.display()
            )));
        }
    }
    check_private_dir(path)
}

/// Test hooks: errnos the probe's steps report instead of calling the real
/// operation (CI has no FAT or exFAT mount, and sync/unlink failures cannot
/// be provoked on demand).
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ProbeHooks {
    pub sync: Option<i32>,
    pub link: Option<i32>,
    pub remove: Option<i32>,
    /// Fails the `sync_all` of `diagnostic.json` (not a probe operation;
    /// kept here so one test seam covers every injected sync failure).
    pub report_sync: Option<i32>,
}

#[cfg(test)]
thread_local! {
    pub(super) static PROBE_HOOKS: std::cell::Cell<ProbeHooks> =
        const {
            std::cell::Cell::new(ProbeHooks {
                sync: None,
                link: None,
                remove: None,
                report_sync: None,
            })
        };
}

#[cfg(test)]
fn hooked(pick: fn(ProbeHooks) -> Option<i32>) -> Option<io::Error> {
    pick(PROBE_HOOKS.with(std::cell::Cell::get)).map(io::Error::from_raw_os_error)
}

fn sync_probe(file: &File) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = hooked(|hooks| hooks.sync) {
        return Err(error);
    }
    file.sync_all()
}

fn link_for_probe(original: &Path, link: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = hooked(|hooks| hooks.link) {
        return Err(error);
    }
    fs::hard_link(original, link)
}

fn remove_probe(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    if let Some(error) = hooked(|hooks| hooks.remove) {
        return Err(error);
    }
    fs::remove_file(path)
}

fn classify_probe_error(error: &io::Error, linking: bool) -> DestinationProblem {
    match error.raw_os_error() {
        // FAT/exFAT answer link() with EPERM; others with EOPNOTSUPP, and a
        // FUSE filesystem without a link operation with ENOSYS. A failure to
        // create or sync our own probe file is never "no hard links".
        Some(libc::EPERM | libc::EOPNOTSUPP | libc::ENOSYS | libc::EXDEV) if linking => {
            DestinationProblem::HardLinksUnsupported
        }
        Some(libc::EPERM | libc::EACCES) => DestinationProblem::PermissionDenied,
        Some(libc::ENOSPC | libc::EDQUOT) => DestinationProblem::StorageFull,
        Some(libc::EROFS) => DestinationProblem::ReadOnly,
        _ => DestinationProblem::Other,
    }
}

/// Whether a probe failure belongs to one diagnostics location (its own
/// permissions or an error there) rather than to the whole filesystem (no
/// hard links, read-only, full). Only location-specific failures let the
/// next eligible location be tried.
fn is_location_specific(problem: DestinationProblem) -> bool {
    match problem {
        DestinationProblem::PermissionDenied | DestinationProblem::Other => true,
        DestinationProblem::HardLinksUnsupported
        | DestinationProblem::ReadOnly
        | DestinationProblem::StorageFull => false,
    }
}

/// [`probe_reporting_leftovers`] without the leftover flag, for tests.
#[cfg(test)]
pub(super) fn probe_hard_links(staging: &Path) -> Result<(), PspShrinkError> {
    probe_reporting_leftovers(staging).map_err(ProbeFailure::into_error)
}

/// Why the hard-link probe failed, and whether it left probe files behind.
struct ProbeFailure {
    problem: DestinationProblem,
    detail: String,
    left_files: bool,
}

impl ProbeFailure {
    fn into_error(self) -> PspShrinkError {
        PspShrinkError::DestinationUnsuitable {
            problem: self.problem,
            detail: self.detail,
        }
    }
}

/// Proves the filesystem can make the no-clobber hard link that publication
/// uses, before any expensive work. Creates a disposable probe file
/// (`create_new`, so it is always this call's own) and one link to it inside
/// the private `staging` folder, then removes exactly the names this call
/// created, on every path. A removal that fails is reported, never ignored:
/// `left_files` is `true` when a probe file could not be removed, and such a
/// location is never silently left behind for another.
fn probe_reporting_leftovers(staging: &Path) -> Result<(), ProbeFailure> {
    let id = unique_id();
    let probe = staging.join(format!(".link-probe-{id}"));
    let linked = staging.join(format!(".link-probe-{id}.link"));
    let place = staging.display();
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
        .map_err(|error| ProbeFailure {
            problem: classify_probe_error(&error, false),
            detail: format!("creating a probe file in {place}: {error}"),
            left_files: false,
        })?;
    // From here on `probe` is ours and is removed whatever happens.
    let mut created = vec![probe.clone()];
    let outcome = match sync_probe(&file) {
        Err(error) => Err((
            classify_probe_error(&error, false),
            format!("syncing the probe file in {place}: {error}"),
        )),
        Ok(()) => match link_for_probe(&probe, &linked) {
            Ok(()) => {
                created.push(linked.clone());
                Ok(())
            }
            Err(error) => Err((
                classify_probe_error(&error, true),
                format!("making a hard link in {place}: {error}"),
            )),
        },
    };
    drop(file);
    let leftovers: Vec<String> = created
        .iter()
        .filter_map(|path| {
            remove_probe(path)
                .err()
                .map(|error| format!("{}: {error}", path.display()))
        })
        .collect();
    let cleanup = || {
        format!(
            "the probe file(s) could not be removed: {}",
            leftovers.join("; ")
        )
    };
    let left = !leftovers.is_empty();
    match outcome {
        Ok(()) if !left => Ok(()),
        Ok(()) => Err(ProbeFailure {
            problem: DestinationProblem::Other,
            detail: format!("the hard-link check passed, but {}", cleanup()),
            left_files: true,
        }),
        Err((problem, detail)) if !left => Err(ProbeFailure {
            problem,
            detail,
            left_files: false,
        }),
        Err((problem, detail)) => Err(ProbeFailure {
            problem,
            detail: format!("{detail}; {}", cleanup()),
            left_files: true,
        }),
    }
}

/// The highest ancestor of `dir` on the same device: its mount point.
fn mount_point(dir: &Path) -> Result<PathBuf, PspShrinkError> {
    let device = fs::metadata(dir)?.dev();
    let mut top = dir.to_path_buf();
    while let Some(parent) = top.parent() {
        match fs::metadata(parent) {
            Ok(meta) if meta.dev() == device => top = parent.to_path_buf(),
            _ => break,
        }
    }
    Ok(top)
}

/// Where conversion evidence may go, in preference order (owner decision
/// 2026-11-24). Every candidate must be on the output's filesystem, a real
/// directory owned by the user and not writable by others, with no symbolic
/// link, and neither inside nor containing a configured game source folder.
/// A candidate that fails is skipped with its reason; if none passes, the
/// conversion is refused. Nothing ever needs root, sudo or changed mount
/// permissions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticsPlacement {
    /// Every configured game source folder, enabled and disabled.
    pub source_folders: Vec<PathBuf>,
    /// 1. Diagnostics directories the user configured (one per disk is
    ///    enough). They must already exist; EmuWiz never creates them.
    pub configured_dirs: Vec<PathBuf>,
    /// 2. EmuWiz's user data directory; `diagnostics/` is created inside it.
    pub data_dir: Option<PathBuf>,
    /// 3. `<mount point>/.emuwiz-diagnostics`, used only where the user can
    ///    already create it safely.
    pub allow_mount_root: bool,
}

/// Which preference produced the diagnostics directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticsSource {
    Configured,
    DataDirectory,
    MountRoot,
}

/// How a user configures a location when none was usable. Kept here so the
/// core message and the GUI say the same thing.
pub const CONFIGURE_HINT: &str = "Create a folder you own on the same disk as the output (outside your game folders) that other users and your group cannot write to (for example, run chmod 700 on it), and add its full path as a line in cso-diagnostics-dirs.txt in the EmuWiz configuration folder";

impl DiagnosticsRoot {
    /// Establishes the diagnostics directory for an output in `output_dir`,
    /// trying the [`DiagnosticsPlacement`] preferences in order.
    pub fn resolve(
        output_dir: &Path,
        placement: &DiagnosticsPlacement,
    ) -> Result<Self, PspShrinkError> {
        let output_dir = fs::canonicalize(output_dir).map_err(|error| {
            unavailable(format!(
                "output folder {} cannot be resolved: {error}",
                output_dir.display()
            ))
        })?;
        let mount = mount_point(&output_dir)?;
        Self::resolve_with_mount_point(&output_dir, placement, &mount)
    }

    /// [`Self::resolve`] with the mount point given (tests simulate one).
    pub(super) fn resolve_with_mount_point(
        output_dir: &Path,
        placement: &DiagnosticsPlacement,
        mount: &Path,
    ) -> Result<Self, PspShrinkError> {
        let device = fs::metadata(output_dir)?.dev();
        let mut candidates: Vec<(DiagnosticsSource, PathBuf, bool)> = Vec::new();
        for dir in &placement.configured_dirs {
            // Configured: must exist already (`false` = never create base).
            candidates.push((DiagnosticsSource::Configured, dir.clone(), false));
        }
        if let Some(data) = &placement.data_dir {
            candidates.push((
                DiagnosticsSource::DataDirectory,
                data.join("diagnostics"),
                true,
            ));
        }
        if placement.allow_mount_root {
            candidates.push((
                DiagnosticsSource::MountRoot,
                mount.join(DIAGNOSTICS_DIR_NAME),
                true,
            ));
        }
        let mut skipped = Vec::new();
        // The last probe failure that belonged to one location only; if no
        // later location works, it is the refusal (with the other reasons).
        let mut location_failure: Option<(DestinationProblem, String)> = None;
        for (source, base, create_base) in candidates {
            match Self::try_candidate(&base, create_base, device, &placement.source_folders) {
                Ok(dir) => match probe_reporting_leftovers(&dir.join(STAGING)) {
                    Ok(()) => {
                        return Ok(Self {
                            dir,
                            device,
                            source,
                            skipped,
                        });
                    }
                    // Every candidate is on the output's own device (checked
                    // in try_candidate), so a filesystem-wide problem cannot
                    // be avoided by another location: refuse at once. So does
                    // a probe that left files behind, so the leftover is
                    // reported now instead of only in a later intent.json.
                    Err(ProbeFailure {
                        problem,
                        detail,
                        left_files: false,
                    }) if is_location_specific(problem) => {
                        skipped.push(format!("{}: {detail}", base.display()));
                        location_failure = Some((problem, detail));
                    }
                    Err(failure) => return Err(failure.into_error()),
                },
                Err(reason) => skipped.push(format!("{}: {reason}", base.display())),
            }
        }
        if let Some((problem, _)) = location_failure {
            return Err(PspShrinkError::DestinationUnsuitable {
                problem,
                detail: format!(
                    "no eligible diagnostics location on the output's disk could be used ({}). {CONFIGURE_HINT}",
                    skipped.join("; ")
                ),
            });
        }
        Err(unavailable(format!(
            "no safe diagnostics folder on the output's disk ({}). {CONFIGURE_HINT}",
            if skipped.is_empty() {
                "no location was configured".to_string()
            } else {
                skipped.join("; ")
            }
        )))
    }

    /// Checks one candidate and returns its `psp-cso` directory. Every check
    /// that can be made before creating anything is made first.
    fn try_candidate(
        base: &Path,
        create_base: bool,
        device: u64,
        source_folders: &[PathBuf],
    ) -> Result<PathBuf, String> {
        if !base.is_absolute() {
            return Err("not an absolute path".into());
        }
        // The nearest existing directory decides the filesystem and the
        // symlink check before anything is created.
        let parent = base.parent().ok_or("has no parent folder")?;
        let resolved_parent =
            fs::canonicalize(parent).map_err(|error| format!("parent folder: {error}"))?;
        if resolved_parent != parent {
            return Err("its path contains a symbolic link".into());
        }
        if fs::metadata(parent).map_err(|e| e.to_string())?.dev() != device {
            return Err("on another filesystem than the output".into());
        }
        for source in source_folders {
            let source = fs::canonicalize(source).unwrap_or_else(|_| source.clone());
            if base.starts_with(&source) || source.starts_with(base) {
                return Err(format!(
                    "inside or containing the game source folder {}",
                    source.display()
                ));
            }
        }
        if !create_base && fs::symlink_metadata(base).is_err() {
            return Err("does not exist (configured folders are never created)".into());
        }
        let dir = base.join(PSP_CSO_DIAGNOSTICS);
        for path in [
            base.to_path_buf(),
            dir.clone(),
            dir.join(STAGING),
            dir.join(FAILED),
        ] {
            let meta = ensure_private_dir(&path).map_err(|error| match error {
                PspShrinkError::DiagnosticsUnavailable(message) => message,
                other => other.to_string(),
            })?;
            if meta.dev() != device {
                return Err(format!("{} crosses a mount boundary", path.display()));
            }
        }
        Ok(dir)
    }

    /// Which preference was used, and why earlier ones were skipped.
    pub fn source(&self) -> DiagnosticsSource {
        self.source
    }

    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Bytes available on the diagnostics (= output) filesystem.
    pub fn available_bytes(&self) -> Result<u64, PspShrinkError> {
        crate::recovery::prepared_media::available_bytes(&self.dir).map_err(unavailable)
    }

    /// Creates `staging/<id>` and writes `intent.json` into it first, so an
    /// interrupted run leaves an identifiable stage.
    pub(super) fn create_stage(&self, intent: &serde_json::Value) -> Result<Stage, PspShrinkError> {
        check_private_dir(&self.dir.join(STAGING))?;
        let id = unique_id();
        let path = self.dir.join(STAGING).join(&id);
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        write_new(&path.join("intent.json"), &to_json_bytes(intent))?;
        Ok(Stage { id, path })
    }

    /// Writes `diagnostic.json`, moves a failed stage to `failed/<id>` (same
    /// filesystem, no copy) and appends the manifest line. Never deletes
    /// evidence: if the move fails the stage stays where it is. A report that
    /// could not be written is reported in the error, never claimed as kept.
    pub(super) fn retain_failure(
        &self,
        stage: Stage,
        report: &serde_json::Value,
        reason: &str,
        cso_retained: bool,
    ) -> PspShrinkError {
        let report_error = write_report(&stage.path.join(REPORT), &to_json_bytes(report))
            .err()
            .map(|error| error.to_string());
        let target = self.dir.join(FAILED).join(&stage.id);
        let evidence = if !target.exists() && fs::rename(&stage.path, &target).is_ok() {
            target
        } else {
            stage.path.clone()
        };
        let bytes = dir_bytes(&evidence);
        let line = serde_json::json!({
            "id": stage.id,
            // Lossy text: `json!` must never fail on a non-UTF-8 path.
            "evidence": evidence.to_string_lossy(),
            "bytes": bytes,
            "reason": reason,
            "failed_cso_kept": cso_retained,
            "report_written": report_error.is_none(),
            "recorded_unix": unix_now(),
        });
        let manifest = self.dir.join(MANIFEST);
        let manifest_error = append_line(&manifest, &line)
            .err()
            .map(|error| error.to_string());
        PspShrinkError::ConversionFailed {
            reason: reason.to_string(),
            evidence,
            cso_retained,
            report_error,
            manifest,
            manifest_error,
        }
    }

    pub fn device(&self) -> u64 {
        self.device
    }
}

/// One in-progress conversion's private directory.
#[derive(Debug)]
pub(super) struct Stage {
    pub(super) id: String,
    pub(super) path: PathBuf,
}

impl Stage {
    /// After a successful publication only: removes this stage's own files.
    /// The CSO's data stays reachable at the output path (hard link). A
    /// failure to tidy up is left in place and shows as a leftover stage.
    pub(super) fn remove_after_publication(self) {
        let _ = fs::remove_file(self.path.join("output.cso"));
        let _ = fs::remove_file(self.path.join("intent.json"));
        let _ = fs::remove_dir(&self.path);
    }
}

/// Storage held by retained evidence, for the failure message and later
/// cleanup controls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticsUsage {
    pub failed_entries: u64,
    pub failed_bytes: u64,
    /// Stages left by an interrupted or crashed run.
    pub interrupted_entries: u64,
    pub interrupted_bytes: u64,
    /// Manifest lines whose evidence no longer exists.
    pub missing_entries: u64,
}

/// Reads the directory tree only; never modifies anything.
pub fn diagnostics_usage(root: &DiagnosticsRoot) -> DiagnosticsUsage {
    let mut usage = DiagnosticsUsage::default();
    for (sub, entries, bytes) in [
        (FAILED, &mut usage.failed_entries, &mut usage.failed_bytes),
        (
            STAGING,
            &mut usage.interrupted_entries,
            &mut usage.interrupted_bytes,
        ),
    ] {
        if let Ok(read) = fs::read_dir(root.dir.join(sub)) {
            for entry in read.flatten() {
                *entries += 1;
                *bytes += dir_bytes(&entry.path());
            }
        }
    }
    if let Ok(text) = fs::read_to_string(root.dir.join(MANIFEST)) {
        for line in text.lines() {
            let evidence = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|value| value.get("evidence")?.as_str().map(PathBuf::from));
            if evidence.is_some_and(|path| !path.exists()) {
                usage.missing_entries += 1;
            }
        }
    }
    usage
}

fn dir_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|read| {
            read.flatten()
                .filter_map(|entry| fs::symlink_metadata(entry.path()).ok())
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0)
}

fn unique_id() -> String {
    let mut random = [0u8; 8];
    let _ = File::open("/dev/urandom").and_then(|mut f| io::Read::read_exact(&mut f, &mut random));
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{suffix}", unix_now(), std::process::id())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn to_json_bytes(value: &serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// [`write_new`] for `diagnostic.json`, with a test seam on its sync.
fn write_report(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    #[cfg(test)]
    if let Some(error) = hooked(|hooks| hooks.report_sync) {
        return Err(error);
    }
    file.sync_all()
}

fn append_line(path: &Path, value: &serde_json::Value) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_file()) {
        return Err(io::Error::other("manifest is not a regular file"));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_all()
}
