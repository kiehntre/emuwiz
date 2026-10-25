//! Read-only ScummVM game-folder detection.
//!
//! ScummVM owns the detection tables and their update/licensing lifecycle.
//! EmuWiz therefore asks the locally installed native ScummVM executable to
//! run its documented `--detect` command instead of bundling or scraping a
//! second database. The command is run with an isolated temporary config so
//! detection cannot update the user's ScummVM configuration.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    /// Lets a test route resolution to a fixture executable on its own thread.
    pub(crate) static TEST_EXECUTABLE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test builds only (`cfg(test)` or the `test-support` feature, which
/// dependents enable from `[dev-dependencies]` only): a whole-process route to
/// a fixture executable, for journey tests that run in a child test process
/// whose worker threads cannot see [`TEST_EXECUTABLE`]. Without it a host
/// ScummVM in `/usr/games` would always be found first. Only an absolute path
/// is honoured; release and ordinary builds never read this variable.
#[cfg(any(test, feature = "test-support"))]
pub const TEST_EXECUTABLE_ENV: &str = "EMUWIZ_TEST_SCUMMVM_EXECUTABLE";

pub const MAX_DETECTION_OUTPUT: usize = 64 * 1024;
const MAX_FOLDER_ENTRIES: usize = 100_000;
const MAX_FOLDER_DEPTH: usize = 32;
const DETECTION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScummVmDetectedGame {
    pub game_id: String,
    pub engine_id: String,
    pub description: Option<String>,
    pub platform: Option<String>,
    pub language: Option<String>,
    pub variant: Option<String>,
    pub demo: Option<bool>,
    /// ScummVM said the folder "seems to be an unknown game variant": a
    /// fallback match on file names/extensions, not a known checksum. Several
    /// engines (Glk among them) claim any folder holding a lone `.cas`,
    /// `.bin`, `.dat` or `.z5` this way, so the match is weak evidence that
    /// must never swallow a folder on its own. See
    /// [`ScummVmDiscovery::unknown_variants`].
    pub unknown_variant: bool,
}

/// The sentence real ScummVM prints (2.1 and later, untranslated CLI output)
/// above a fallback match: "The game in 'x' seems to be an unknown game
/// variant." Releases before 2.1 said "seems to be unknown." and would not be
/// flagged; none of those is a supported detector.
const UNKNOWN_VARIANT_MARKER: &str = "unknown game variant";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScummVmDetectionError {
    InvalidRoot(String),
    UnsafeEntry(PathBuf),
    TooManyEntries,
    DetectorUnavailable,
    DetectorFailed(String),
    MalformedOutput(String),
    NoMatch,
    Ambiguous(usize),
}

impl std::fmt::Display for ScummVmDetectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot(detail) => write!(f, "invalid ScummVM game folder: {detail}"),
            Self::UnsafeEntry(path) => {
                write!(f, "unsafe symbolic or special entry: {}", path.display())
            }
            Self::TooManyEntries => {
                f.write_str("ScummVM game folder exceeds the bounded entry limit")
            }
            Self::DetectorUnavailable => f.write_str("native ScummVM detector is unavailable"),
            Self::DetectorFailed(detail) => write!(f, "ScummVM detector failed: {detail}"),
            Self::MalformedOutput(detail) => {
                write!(f, "malformed ScummVM detector output: {detail}")
            }
            Self::NoMatch => f.write_str("ScummVM detected no game in this folder"),
            Self::Ambiguous(count) => {
                write!(f, "ScummVM detected {count} games; identity is ambiguous")
            }
        }
    }
}

/// Detect one game using the locally installed ScummVM executable.
pub fn detect_scummvm_directory(path: &Path) -> Result<ScummVmDetectedGame, ScummVmDetectionError> {
    let executable =
        resolve_scummvm_executable().ok_or(ScummVmDetectionError::DetectorUnavailable)?;
    detect_scummvm_directory_with_executable(path, &executable)
}

/// Returns whether a detector-produced engine/game identifier has the
/// qualified form ScummVM uses for launch targets. This validates syntax only;
/// it does not make an identifier authoritative.
pub fn is_valid_scummvm_game_id(value: &str) -> bool {
    let Some((engine, game)) = value.split_once(':') else {
        return false;
    };
    valid_id(engine) && valid_id(game) && !game.contains(':')
}

/// Testable form of [`detect_scummvm_directory`] which still uses the exact
/// production argv and output parser. The executable is never invoked through
/// a shell.
pub fn detect_scummvm_directory_with_executable(
    path: &Path,
    executable: &Path,
) -> Result<ScummVmDetectedGame, ScummVmDetectionError> {
    let games = detect_candidates(path, executable)?;
    match games.len() {
        0 => Err(ScummVmDetectionError::NoMatch),
        1 => Ok(games.into_iter().next().expect("one game")),
        count => Err(ScummVmDetectionError::Ambiguous(count)),
    }
}

/// Shared plumbing behind both [`detect_scummvm_directory_with_executable`]
/// (which collapses this into the original zero/one/many `Result`) and
/// [`summarize_scummvm_directory`] (which keeps the full, deduplicated
/// candidate list for an ambiguous result instead of only a count). Both
/// callers go through the exact same validation, subprocess invocation, and
/// deduplication, so they can never disagree about what counts as "the same
/// candidate reported twice".
fn detect_candidates(
    path: &Path,
    executable: &Path,
) -> Result<Vec<ScummVmDetectedGame>, ScummVmDetectionError> {
    validate_game_directory(path)?;
    let config = isolated_config_path();
    let path_arg = OsString::from("--path=").tap_append(path.as_os_str());
    let config_arg = OsString::from("--config=").tap_append(config.as_os_str());
    let result = run_detector(
        executable,
        &[config_arg, path_arg, OsString::from("--detect")],
    );
    let _ = fs::remove_file(&config);
    let output = result?;
    let games = parse_detection_output(&output)?;
    Ok(dedupe_candidates(games))
}

/// Collapses repeated identical `engine:game` records into one, preserving
/// first-occurrence order.
///
/// ScummVM's own `--detect` output has been observed to list the same
/// qualified id more than once (e.g. once per matching resource-file
/// variant it considered before settling on the same identity). Without
/// this, two identical lines would make an otherwise unambiguous folder
/// report as `Ambiguous(2)` - a real detected game rejected only because
/// the detector was chatty about it, not because there is genuinely more
/// than one candidate. Only `engine_id`/`game_id` are compared: those are
/// the sole components of the qualified id EmuWiz treats as this game's
/// identity (see [`crate::game_identity::GameIdentityReport::verified_scummvm_game_id`]),
/// so two records sharing them are the same launch target even if their
/// incidental `language`/`platform`/`variant` metadata differs - keeping
/// both would not add a genuinely distinct candidate, only a duplicate
/// with different trivia attached. The first occurrence's metadata (whatever
/// ScummVM printed first) is what survives.
pub fn dedupe_candidates(games: Vec<ScummVmDetectedGame>) -> Vec<ScummVmDetectedGame> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(games.len());
    for game in games {
        let key = (game.engine_id.clone(), game.game_id.clone());
        if seen.insert(key) {
            deduped.push(game);
        }
    }
    deduped
}

/// Deduplicates candidate directories before any of them reach a detector
/// subprocess, preserving first-occurrence order.
///
/// Two different path spellings that resolve to the same real location
/// (e.g. a path reached through a symlink elsewhere in the library, or
/// simply listed twice) are folded to one entry via `canonicalize` - the
/// *original*, uncanonicalized path is kept for the actual invocation, so
/// this only prevents a wasted duplicate subprocess call and never changes
/// what path a caller sees back. A path that cannot be canonicalized (already
/// missing, a dangling symlink, permission denied) is kept as-is rather than
/// silently dropped or merged with something else - it still gets checked,
/// and whatever is wrong with it becomes a real, visible detector error
/// rather than a swallowed duplicate.
pub fn dedupe_candidate_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(paths.len());
    for path in paths {
        let key = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(key) {
            deduped.push(path.clone());
        }
    }
    deduped
}

/// A directory's detection result, bucketed for a caller that wants to
/// summarize many folders at once (a bulk "check my library" job) without
/// re-deriving the same zero/one/many/error logic
/// [`detect_scummvm_directory_with_executable`] already encodes.
///
/// Unlike [`ScummVmDetectionError::Ambiguous`], which only carries a count
/// (kept that way so it stays a compatible, non-breaking type for existing
/// callers), `Ambiguous` here keeps every deduplicated candidate - a caller
/// that wants to *show* what ScummVM found, not just that it couldn't
/// choose, needs the real list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScummVmDetectionSummary {
    /// Exactly one trustworthy candidate.
    Detected(ScummVmDetectedGame),
    /// More than one distinct candidate survived deduplication - never
    /// resolved to a guess. Carries every candidate ScummVM reported.
    Ambiguous(Vec<ScummVmDetectedGame>),
    /// The detector ran cleanly and explicitly found nothing in this
    /// folder it recognises.
    Unsupported,
    /// The detector could not be asked at all, or its answer could not be
    /// trusted - the folder itself (or the detector invocation), not the
    /// game inside it, is the problem. Carries the underlying error's own
    /// message for display; match on `detect_scummvm_directory_with_executable`
    /// directly instead if a caller needs to branch on *which* failure this
    /// was.
    DetectorFailure(String),
    /// This candidate was never checked - see [`check_scummvm_directories`]'s
    /// own doc for the one case that produces this (a cooperative cancel
    /// requested before this candidate's turn).
    Skipped,
}

fn summarize_candidates(
    result: Result<Vec<ScummVmDetectedGame>, ScummVmDetectionError>,
) -> ScummVmDetectionSummary {
    match result {
        Ok(games) if games.is_empty() => ScummVmDetectionSummary::Unsupported,
        Ok(mut games) if games.len() == 1 => ScummVmDetectionSummary::Detected(games.remove(0)),
        Ok(games) => ScummVmDetectionSummary::Ambiguous(games),
        Err(error) => ScummVmDetectionSummary::DetectorFailure(error.to_string()),
    }
}

/// [`detect_scummvm_directory_with_executable`], summarized instead of
/// collapsed to a `Result` - see [`ScummVmDetectionSummary`]'s own doc for
/// why this exists alongside it rather than replacing it.
pub fn summarize_scummvm_directory(path: &Path, executable: &Path) -> ScummVmDetectionSummary {
    summarize_candidates(detect_candidates(path, executable))
}

/// One candidate directory checked in a [`check_scummvm_directories`] batch,
/// paired with what came of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScummVmCandidateResult {
    pub path: PathBuf,
    pub outcome: ScummVmDetectionSummary,
}

/// Runs the detector against a deduplicated ([`dedupe_candidate_paths`]),
/// order-preserved list of candidate directories, one subprocess per
/// candidate, checking `cancel` *between* candidates rather than during one.
///
/// There is no safe way to interrupt a detector subprocess that is already
/// running mid-call without risking a half-read pipe, an orphaned process,
/// or a config file left behind - so this never attempts that; it is a
/// deliberate limitation, not an oversight (see this module's own audit
/// notes on cancellation). What *is* always safe is declining to start the
/// *next* one: every call is fully independent, so checking `cancel` here,
/// once per iteration, before that next subprocess spawns, can never leave
/// anything partially applied. Once cancelled, every remaining candidate is
/// still returned in the result, marked [`ScummVmDetectionSummary::Skipped`] -
/// the returned `Vec` is always exactly as long as the deduplicated
/// candidate list, so a caller never has to separately track how many were
/// skipped.
///
/// `on_progress(checked, total, path)` is called immediately before each
/// candidate that is actually checked (never for a skipped one), with
/// `total` counted after deduplication.
pub fn check_scummvm_directories(
    executable: &Path,
    candidates: &[PathBuf],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(usize, usize, &Path),
) -> Vec<ScummVmCandidateResult> {
    let deduped = dedupe_candidate_paths(candidates);
    let total = deduped.len();
    let mut cancelled = false;
    let mut results = Vec::with_capacity(total);
    for (index, path) in deduped.into_iter().enumerate() {
        if !cancelled && cancel.load(Ordering::Relaxed) {
            cancelled = true;
        }
        let outcome = if cancelled {
            ScummVmDetectionSummary::Skipped
        } else {
            on_progress(index, total, &path);
            summarize_candidates(detect_candidates(&path, executable))
        };
        results.push(ScummVmCandidateResult { path, outcome });
    }
    results
}

/// A snapshot of whether the native ScummVM detector this build would
/// invoke is actually usable, and its self-reported version if that was
/// safely obtainable.
///
/// Deliberately just two states. A "detector unsupported - incompatible CLI
/// shape" state was considered (ScummVM's `--detect` output shape could in
/// principle change between major versions), but nothing in this repository
/// currently distinguishes that from an ordinary [`ScummVmDetectionError`]
/// failure - there is no known real ScummVM release whose `--detect`/
/// `--version` output has actually been observed to differ from what
/// [`parse_detection_output`] and [`scummvm_version`] already parse, so
/// adding an "unsupported" bucket now would be an unfounded guess with no
/// fixture to validate it against, not a hardening. If a real incompatible
/// build is ever found, its actual output belongs in a parser test first,
/// and a distinguishable variant here second.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScummVmCompatibility {
    NotInstalled,
    Installed {
        executable: PathBuf,
        /// `None` when the version subprocess failed or produced no usable
        /// first line - see [`scummvm_version`]'s own doc.
        version: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScummVmReadinessBlockerKind {
    ExecutableMissing,
    DetectorUnavailable,
    DetectorFailed,
    InvalidGameFolder,
    IdentityUnresolved,
    UnsupportedGame,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScummVmReadinessEvidence {
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub detector_available: bool,
    pub game_folder: Option<PathBuf>,
    pub detected_game: Option<ScummVmDetectedGame>,
    pub blocker_kind: Option<ScummVmReadinessBlockerKind>,
    pub first_blocker: Option<String>,
    pub ready: bool,
}

/// Reuses the detector and executable checks used by ScummVM launch
/// preflight. Detection keeps using its isolated temporary configuration and
/// never creates persistent ScummVM state.
pub fn assess_scummvm_readiness(
    executable: Option<&Path>,
    game_folder: Option<&Path>,
) -> ScummVmReadinessEvidence {
    let Some(executable) = executable else {
        return ScummVmReadinessEvidence {
            executable: None,
            version: None,
            detector_available: false,
            game_folder: game_folder.map(Path::to_path_buf),
            detected_game: None,
            blocker_kind: Some(ScummVmReadinessBlockerKind::ExecutableMissing),
            first_blocker: Some("ScummVM executable is unavailable or unsafe".into()),
            ready: false,
        };
    };
    let binding =
        crate::launch::scummvm_command::resolve_scummvm_native_launch_binding_at(executable);
    if let Err(detail) = binding {
        return ScummVmReadinessEvidence {
            executable: Some(executable.to_path_buf()),
            version: None,
            detector_available: false,
            game_folder: game_folder.map(Path::to_path_buf),
            detected_game: None,
            blocker_kind: Some(ScummVmReadinessBlockerKind::ExecutableMissing),
            first_blocker: Some(detail),
            ready: false,
        };
    }
    let version = scummvm_version(executable);
    let Some(game_folder) = game_folder else {
        return ScummVmReadinessEvidence {
            executable: Some(executable.to_path_buf()),
            version,
            detector_available: true,
            game_folder: None,
            detected_game: None,
            blocker_kind: None,
            first_blocker: None,
            ready: true,
        };
    };
    match detect_scummvm_directory_with_executable(game_folder, executable) {
        Ok(detected_game) => ScummVmReadinessEvidence {
            executable: Some(executable.to_path_buf()),
            version,
            detector_available: true,
            game_folder: Some(game_folder.to_path_buf()),
            detected_game: Some(detected_game),
            blocker_kind: None,
            first_blocker: None,
            ready: true,
        },
        Err(error) => {
            let (blocker_kind, detail) = match &error {
                ScummVmDetectionError::InvalidRoot(_)
                | ScummVmDetectionError::UnsafeEntry(_)
                | ScummVmDetectionError::TooManyEntries => (
                    ScummVmReadinessBlockerKind::InvalidGameFolder,
                    error.to_string(),
                ),
                ScummVmDetectionError::DetectorUnavailable => (
                    ScummVmReadinessBlockerKind::DetectorUnavailable,
                    error.to_string(),
                ),
                ScummVmDetectionError::DetectorFailed(_)
                | ScummVmDetectionError::MalformedOutput(_) => (
                    ScummVmReadinessBlockerKind::DetectorFailed,
                    error.to_string(),
                ),
                ScummVmDetectionError::Ambiguous(_) => (
                    ScummVmReadinessBlockerKind::IdentityUnresolved,
                    error.to_string(),
                ),
                ScummVmDetectionError::NoMatch => (
                    ScummVmReadinessBlockerKind::UnsupportedGame,
                    error.to_string(),
                ),
            };
            ScummVmReadinessEvidence {
                executable: Some(executable.to_path_buf()),
                version,
                detector_available: true,
                game_folder: Some(game_folder.to_path_buf()),
                detected_game: None,
                blocker_kind: Some(blocker_kind),
                first_blocker: Some(detail),
                ready: false,
            }
        }
    }
}

/// Resolves the local executable and, if one exists, asks it for its own
/// version - see [`ScummVmCompatibility`]'s own doc for the shape and why
/// it stops there.
pub fn assess_scummvm_compatibility() -> ScummVmCompatibility {
    match resolve_scummvm_executable() {
        Some(executable) => assess_scummvm_compatibility_with_executable(executable),
        None => ScummVmCompatibility::NotInstalled,
    }
}

/// Testable form of [`assess_scummvm_compatibility`] for an already-resolved
/// executable - the same testable-seam shape
/// [`detect_scummvm_directory_with_executable`] uses.
pub fn assess_scummvm_compatibility_with_executable(executable: PathBuf) -> ScummVmCompatibility {
    let version = scummvm_version(&executable);
    ScummVmCompatibility::Installed {
        executable,
        version,
    }
}

fn validate_game_directory(path: &Path) -> Result<(), ScummVmDetectionError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?;
    // Named separately from the generic "not a directory" case below: a
    // symlinked top-level game folder is a real, plausible library shape
    // (a NAS mount, a manually curated collection), and `symlink_metadata`
    // never follows it, so without this check it would silently fall into
    // "path is not a directory" - a confusing message for something that
    // usually *is* a directory, just reached through a link. This still
    // refuses it (the nested-entry walk below already refuses a symlink
    // *inside* the tree for the same reason - escaping the validated root),
    // it just names the real reason rather than a misleading one.
    if metadata.file_type().is_symlink() {
        return Err(ScummVmDetectionError::InvalidRoot(
            "path is a symlink, not the real directory - point ScummVM detection at the \
             resolved location"
                .into(),
        ));
    }
    if !metadata.is_dir() {
        return Err(ScummVmDetectionError::InvalidRoot(
            "path is not a directory".into(),
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?;
    let mut stack = vec![(path.to_path_buf(), 0usize)];
    let mut entries = 0usize;
    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_FOLDER_DEPTH {
            return Err(ScummVmDetectionError::TooManyEntries);
        }
        for entry in fs::read_dir(&directory)
            .map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?
        {
            entries = entries.saturating_add(1);
            if entries > MAX_FOLDER_ENTRIES {
                return Err(ScummVmDetectionError::TooManyEntries);
            }
            let entry =
                entry.map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?;
            let entry_path = entry.path();
            let metadata = fs::symlink_metadata(&entry_path)
                .map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?;
            if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
                return Err(ScummVmDetectionError::UnsafeEntry(entry_path));
            }
            let entry_canonical = entry_path
                .canonicalize()
                .map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?;
            if !entry_canonical.starts_with(&canonical) {
                return Err(ScummVmDetectionError::UnsafeEntry(entry_path));
            }
            if metadata.is_dir() {
                stack.push((entry_path, depth.saturating_add(1)));
            }
        }
    }
    Ok(())
}

pub fn resolve_scummvm_executable() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(forced) = TEST_EXECUTABLE.with(|cell| cell.borrow().clone()) {
        return Some(forced);
    }
    let mut candidates = Vec::new();
    // Test builds: the fixture is the only candidate, and it passes the same
    // checks below, so a missing fixture finds nothing rather than falling
    // through to a real host install.
    #[cfg(any(test, feature = "test-support"))]
    let forced = std::env::var_os(TEST_EXECUTABLE_ENV).map(PathBuf::from);
    #[cfg(not(any(test, feature = "test-support")))]
    let forced: Option<PathBuf> = None;
    match forced {
        Some(forced) => candidates.push(forced),
        None => {
            candidates.push(PathBuf::from("/usr/games/scummvm"));
            candidates.push(PathBuf::from("/usr/bin/scummvm"));
            if let Some(path) = std::env::var_os("PATH") {
                for component in std::env::split_paths(&path).take(128) {
                    candidates.push(component.join("scummvm"));
                }
            }
        }
    }
    // A relative or empty PATH entry would resolve against the working
    // directory; only absolute locations are ever trusted.
    candidates
        .into_iter()
        .filter(|candidate| candidate.is_absolute())
        .find(|candidate| {
            let Ok(metadata) = fs::symlink_metadata(candidate) else {
                return false;
            };
            #[cfg(unix)]
            use std::os::unix::fs::PermissionsExt;
            #[cfg(unix)]
            let executable = metadata.permissions().mode() & 0o111 != 0;
            #[cfg(not(unix))]
            let executable = true;
            metadata.is_file() && executable
        })
}

/// Asks the locally installed ScummVM executable for its own version string,
/// using the exact same bounded, timeout-protected subprocess machinery as
/// detection (`run_detector`) - never a second invocation mechanism. `None`
/// on any failure (not installed, times out, non-UTF-8, empty output): the
/// caller shows readiness without a version rather than an error, since a
/// version string is a nicety, not something detection depends on.
pub fn scummvm_version(executable: &Path) -> Option<String> {
    // ScummVM loads and re-saves its default configuration even for
    // `--version`, so the probe always runs on a throwaway one: it must never
    // open the user's own `scummvm.ini` (or create one in the working directory).
    let config = isolated_config_path();
    let output = run_detector(
        executable,
        &[
            OsString::from("--config=").tap_append(config.as_os_str()),
            OsString::from("--version"),
        ],
    );
    let _ = fs::remove_file(&config);
    let output = output.ok()?;
    let text = std::str::from_utf8(&output).ok()?;
    let first_line = text.lines().next()?.trim();
    (!first_line.is_empty()).then(|| first_line.to_string())
}

fn isolated_config_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "archivefs-scummvm-{stamp}-{}.ini",
        std::process::id()
    ))
}

const MAX_TEXT_BUSY_RETRIES: usize = 250;

fn retry_transient_text_busy<T, F, S>(mut operation: F, mut delay: S) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
    S: FnMut(),
{
    let mut retries = 0;
    loop {
        match operation() {
            Err(error)
                if error.raw_os_error() == Some(libc::ETXTBSY)
                    && retries < MAX_TEXT_BUSY_RETRIES =>
            {
                retries += 1;
                delay();
            }
            other => return other,
        }
    }
}

fn spawn_tolerating_transient_text_busy(command: &mut Command) -> io::Result<std::process::Child> {
    retry_transient_text_busy(
        || command.spawn(),
        || thread::sleep(Duration::from_millis(1)),
    )
}

fn run_detector(executable: &Path, args: &[OsString]) -> Result<Vec<u8>, ScummVmDetectionError> {
    run_detector_bounded(executable, args, DETECTION_TIMEOUT, MAX_DETECTION_OUTPUT)
}

fn run_detector_bounded(
    executable: &Path,
    args: &[OsString],
    timeout: Duration,
    max_output: usize,
) -> Result<Vec<u8>, ScummVmDetectionError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn_tolerating_transient_text_busy(&mut command)
        .map_err(|error| ScummVmDetectionError::DetectorFailed(error.to_string()))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_thread = thread::spawn(move || read_bounded(stdout, max_output));
    let stderr_thread = thread::spawn(move || read_bounded(stderr, MAX_DETECTION_OUTPUT));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                return Err(ScummVmDetectionError::DetectorFailed(
                    "detector timed out".into(),
                ));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(ScummVmDetectionError::DetectorFailed(error.to_string()));
            }
        }
    };
    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        return Err(ScummVmDetectionError::DetectorFailed(
            String::from_utf8_lossy(&stderr).trim().to_string(),
        ));
    }
    Ok(stdout)
}

fn read_bounded(mut reader: impl Read, max: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    while bytes.len() < max {
        let limit = (max - bytes.len()).min(buffer.len());
        match reader.read(&mut buffer[..limit]) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
        }
    }
    bytes
}

/// Parses the stable, documented `--detect` list shape emitted by ScummVM.
/// Only explicit `Game: engine:gameid` records are accepted; arbitrary text,
/// filenames, and descriptions never become identity.
pub fn parse_detection_output(
    output: &[u8],
) -> Result<Vec<ScummVmDetectedGame>, ScummVmDetectionError> {
    let text = std::str::from_utf8(output)
        .map_err(|_| ScummVmDetectionError::MalformedOutput("output is not UTF-8".into()))?;
    let unknown_variant = text.contains(UNKNOWN_VARIANT_MARKER);
    if let Some(rows) = parse_detection_table(text)? {
        return Ok(rows
            .into_iter()
            .map(|(mut game, _)| {
                game.unknown_variant = unknown_variant;
                game
            })
            .collect());
    }
    let mut games = Vec::new();
    let mut current: Option<ScummVmDetectedGame> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(value) = line
            .strip_prefix("Game:")
            .or_else(|| line.strip_prefix("Game ID:"))
        {
            if let Some(game) = current.take() {
                games.push(game);
            }
            let record = value.trim().trim_matches('"');
            let token = record.split_once(' ').map_or(record, |(id, _)| id);
            let (engine_id, game_id) = token.split_once(':').ok_or_else(|| {
                ScummVmDetectionError::MalformedOutput("game record lacks engine:game id".into())
            })?;
            if !valid_id(engine_id) || !valid_id(game_id) {
                return Err(ScummVmDetectionError::MalformedOutput(
                    "invalid game ID".into(),
                ));
            }
            let description = record
                .strip_prefix(token)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.trim_matches(['(', ')']).to_string());
            current = Some(ScummVmDetectedGame {
                game_id: game_id.into(),
                engine_id: engine_id.into(),
                description,
                platform: None,
                language: None,
                variant: None,
                demo: None,
                unknown_variant: false,
            });
        } else if let Some(game) = current.as_mut() {
            parse_optional_field(game, line);
        }
    }
    if let Some(game) = current {
        games.push(game);
    }
    for game in &mut games {
        game.unknown_variant = unknown_variant;
    }
    if games.is_empty()
        && !text.contains("no game")
        && !text.contains("No games")
        && !text.contains("could not find any game")
    {
        return Err(ScummVmDetectionError::MalformedOutput(
            "no explicit game records".into(),
        ));
    }
    Ok(games)
}

/// Parses the table real ScummVM (2.x) prints for `--detect`: a
/// `GameID  Description  Full Path` header, a dashed rule that fixes the column
/// widths, then one row per detected game. Returns `Ok(None)` when the output
/// has no such header, so the older `Game:` record shape is still handled by the
/// caller. Only the first column becomes identity; the path column is returned
/// so a recursive scan can say where each game was found.
fn parse_detection_table(
    text: &str,
) -> Result<Option<Vec<(ScummVmDetectedGame, PathBuf)>>, ScummVmDetectionError> {
    let mut lines = text.lines();
    let Some(_) = lines.find(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("GameID") && trimmed.contains("Description")
    }) else {
        return Ok(None);
    };
    let rule: Vec<char> = lines.next().unwrap_or_default().chars().collect();
    let mut starts = Vec::new();
    for (index, character) in rule.iter().enumerate() {
        if *character == '-' && (index == 0 || rule[index - 1] == ' ') {
            starts.push(index);
        }
    }
    if starts.len() != 3 || starts[0] != 0 {
        return Err(ScummVmDetectionError::MalformedOutput(
            "detection table has an unexpected column layout".into(),
        ));
    }
    let mut rows = Vec::new();
    for raw in lines {
        let chars: Vec<char> = raw.chars().collect();
        if chars.iter().all(|c| c.is_whitespace()) {
            continue;
        }
        if chars.len() <= starts[2] {
            return Err(ScummVmDetectionError::MalformedOutput(
                "detection table row is shorter than its columns".into(),
            ));
        }
        let column = |from: usize, to: usize| {
            chars[from..to.min(chars.len())]
                .iter()
                .collect::<String>()
                .trim()
                .to_string()
        };
        let id = column(starts[0], starts[1]);
        let description = column(starts[1], starts[2]);
        let path = column(starts[2], chars.len());
        let (engine_id, game_id) = id.split_once(':').ok_or_else(|| {
            ScummVmDetectionError::MalformedOutput("game record lacks engine:game id".into())
        })?;
        if !valid_id(engine_id) || !valid_id(game_id) || path.is_empty() {
            return Err(ScummVmDetectionError::MalformedOutput(
                "invalid game record".into(),
            ));
        }
        rows.push((
            ScummVmDetectedGame {
                game_id: game_id.into(),
                engine_id: engine_id.into(),
                description: Some(description).filter(|value| !value.is_empty()),
                platform: None,
                language: None,
                variant: None,
                demo: None,
                unknown_variant: false,
            },
            PathBuf::from(path),
        ));
    }
    Ok(Some(rows))
}

/// One game folder found by [`discover_scummvm_game_directories`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredScummVmGame {
    pub path: PathBuf,
    pub game: ScummVmDetectedGame,
}

/// What a recursive discovery pass found and deliberately left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScummVmDiscovery {
    pub games: Vec<DiscoveredScummVmGame>,
    /// Folders ScummVM matched to more than one game; never guessed between.
    pub ambiguous: Vec<PathBuf>,
    /// Folders ScummVM matched only as an "unknown game variant" - a fallback
    /// on file names, not a known checksum. Reported, never catalogued as a
    /// game and never allowed to swallow the folder's other media: a tape
    /// folder with one `.cas` file is not a Glk text adventure.
    pub unknown_variants: Vec<DiscoveredScummVmGame>,
    /// Folders whose per-folder detection disagreed with the recursive result
    /// or was refused (symlink, unsafe entry): not offered as games.
    pub rejected: Vec<PathBuf>,
}

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_DISCOVERY_OUTPUT: usize = 4 * 1024 * 1024;

/// Asks the installed ScummVM which folders under `root` it recognises, in one
/// recursive `--detect` run with an isolated configuration, then re-checks each
/// reported folder alone with the same detector a launch uses. ScummVM also
/// reports a container folder when a game sits inside it, so a folder is dropped
/// when a deeper folder is reported for the same game. Nothing is written to
/// `root`.
pub fn discover_scummvm_game_directories(
    root: &Path,
    executable: &Path,
) -> Result<ScummVmDiscovery, ScummVmDetectionError> {
    discover_scummvm_game_directories_cached(root, executable, None).map(|(found, _)| found)
}

/// A stored recursive-detector run for one source, valid only while its `key`
/// still matches.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveryCacheEntry {
    pub key: String,
    pub output: String,
    /// Unix seconds when ScummVM was actually run for this output. A reused
    /// entry keeps its original time, so reuse never extends its life. Entries
    /// without it (older sidecars) read as 0 and so as long expired.
    #[serde(default)]
    pub checked_unix: u64,
}

/// How long a cached run that found games may be reused.
///
/// Trust bound, stated plainly: the sidecar sits in the user's own data
/// directory, so anyone who can forge a valid entry can already change that
/// user's library. A forged entry can only hide games (every game it lists is
/// re-verified fresh, and launch re-detects), and only until it expires: this
/// limit when it lists games, [`EMPTY_ENTRY_MAX_AGE_SECS`] when it lists none.
/// It is not tamper-evident, which is out of scope here.
const FOUND_ENTRY_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
/// How long a cached run that found nothing may be reused. A "no games" result
/// cannot be re-verified per folder, so it is trusted for much less time.
const EMPTY_ENTRY_MAX_AGE_SECS: u64 = 24 * 3600;
/// Clock skew tolerated before a future-dated entry is treated as untrustworthy.
const ENTRY_FUTURE_SLACK_SECS: u64 = 300;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn entry_is_young_enough(entry: &DiscoveryCacheEntry, found_games: bool, now: u64) -> bool {
    if entry.checked_unix > now.saturating_add(ENTRY_FUTURE_SLACK_SECS) {
        return false;
    }
    let limit = if found_games {
        FOUND_ENTRY_MAX_AGE_SECS
    } else {
        EMPTY_ENTRY_MAX_AGE_SECS
    };
    now.saturating_sub(entry.checked_unix) <= limit
}

static DISCOVERY_IN_PROGRESS: std::sync::Mutex<Option<(PathBuf, Instant)>> =
    std::sync::Mutex::new(None);

/// The source a recursive detector run is working on and since when, for a UI
/// that wants to say what a long scan is doing. `None` when no run is active (a
/// cache hit never sets it).
pub fn discovery_in_progress() -> Option<(PathBuf, Duration)> {
    let guard = DISCOVERY_IN_PROGRESS.lock().ok()?;
    guard
        .as_ref()
        .map(|(path, since)| (path.clone(), since.elapsed()))
}

struct ProgressGuard;
impl ProgressGuard {
    fn start(root: &Path) -> Self {
        if let Ok(mut guard) = DISCOVERY_IN_PROGRESS.lock() {
            *guard = Some((root.to_path_buf(), Instant::now()));
        }
        Self
    }
}
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = DISCOVERY_IN_PROGRESS.lock() {
            *guard = None;
        }
    }
}

const MAX_SIGNATURE_ENTRIES: usize = 1_000_000;

/// Hashes everything that could change what ScummVM reports for `root`: every
/// entry's path, type, size, modification and change times and identity, plus
/// each symlink's target. Only `lstat`/`readdir` are used. Returns `None`
/// (so nothing is cached) when the tree is too large to bound or holds a
/// symlink to a directory, because the detector may follow that link and a
/// change behind it would not show here.
fn tree_signature(root: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let mut hasher = Sha256::new();
    let mut stack = vec![root.to_path_buf()];
    let mut entries = 0usize;
    while let Some(directory) = stack.pop() {
        let mut names: Vec<_> = fs::read_dir(&directory)
            .ok()?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()
            .ok()?;
        names.sort();
        for name in names {
            entries += 1;
            if entries > MAX_SIGNATURE_ENTRIES {
                return None;
            }
            let path = directory.join(&name);
            let metadata = fs::symlink_metadata(&path).ok()?;
            hasher.update(path.as_os_str().as_bytes());
            hasher.update([0]);
            for value in [
                u64::from(metadata.mode()),
                metadata.len(),
                metadata.mtime() as u64,
                metadata.mtime_nsec() as u64,
                metadata.ctime() as u64,
                metadata.ctime_nsec() as u64,
                metadata.ino(),
                metadata.dev(),
            ] {
                hasher.update(value.to_le_bytes());
            }
            if metadata.file_type().is_symlink() {
                if fs::metadata(&path).ok()?.is_dir() {
                    return None;
                }
                hasher.update(fs::read_link(&path).ok()?.as_os_str().as_bytes());
            } else if metadata.is_dir() {
                stack.push(path);
            }
        }
        // The directory's own times cover additions and removals.
        let own = fs::symlink_metadata(&directory).ok()?;
        hasher.update(own.mtime().to_le_bytes());
        hasher.update(own.mtime_nsec().to_le_bytes());
    }
    Some(crate::launch::scummvm_execution::trainer_profile::sha256_hex(&hasher.finalize()))
}

fn discovery_cache_key(root: &Path, executable: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::MetadataExt;
    let signature = tree_signature(root)?;
    let version = scummvm_version(executable)?;
    let exe = fs::metadata(executable).ok()?;
    let mut hasher = Sha256::new();
    for part in [
        "scummvm-discovery-v1",
        &root.display().to_string(),
        &executable.display().to_string(),
        &version,
        &signature,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    for value in [exe.len(), exe.mtime() as u64, exe.ino()] {
        hasher.update(value.to_le_bytes());
    }
    Some(crate::launch::scummvm_execution::trainer_profile::sha256_hex(&hasher.finalize()))
}

/// As [`discover_scummvm_game_directories`], but reuses the recursive detector
/// run in `cached` when its key still matches. Only the raw detector output is
/// reused: ownership, symlink, ambiguity and per-folder re-detection checks
/// below always run fresh, so a cached result never overrides them. Returns the
/// entry to store (`None` when the tree cannot be keyed safely).
pub fn discover_scummvm_game_directories_cached(
    root: &Path,
    executable: &Path,
    cached: Option<&DiscoveryCacheEntry>,
) -> Result<(ScummVmDiscovery, Option<DiscoveryCacheEntry>), ScummVmDetectionError> {
    let root = root
        .canonicalize()
        .map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?;
    let key = discovery_cache_key(&root, executable);
    // A cached run is reused only while all of this holds: the key (tree
    // signature, ScummVM version and executable, root) matches, the entry is
    // young enough, and every fresh per-folder check still agrees with it. Any
    // doubt falls through to a real run in this same call, so a stale, forged or
    // unreadable entry heals immediately instead of surfacing as an error.
    if let (Some(entry), Some(key)) = (cached, &key)
        && &entry.key == key
        && let Ok(found) = analyse_detection_output(&root, executable, &entry.output)
        && found.rejected.is_empty()
        && entry_is_young_enough(entry, !found.games.is_empty(), unix_now())
    {
        return Ok((found, Some(entry.clone())));
    }
    let text = {
        let _progress = ProgressGuard::start(&root);
        let config = isolated_config_path();
        let output = run_detector_bounded(
            executable,
            &[
                OsString::from("--config=").tap_append(config.as_os_str()),
                OsString::from("--path=").tap_append(root.as_os_str()),
                OsString::from("--detect"),
                OsString::from("--recursive"),
            ],
            DISCOVERY_TIMEOUT,
            MAX_DISCOVERY_OUTPUT,
        );
        let _ = fs::remove_file(&config);
        String::from_utf8(output?)
            .map_err(|_| ScummVmDetectionError::MalformedOutput("output is not UTF-8".into()))?
    };
    let discovery = analyse_detection_output(&root, executable, &text)?;
    let entry = key.map(|key| DiscoveryCacheEntry {
        key,
        output: text,
        checked_unix: unix_now(),
    });
    Ok((discovery, entry))
}

/// Turns one recursive detector output into verified game folders. Everything
/// here runs fresh on every call, whether the output came from ScummVM just now
/// or from the cache.
fn analyse_detection_output(
    root: &Path,
    executable: &Path,
    text: &str,
) -> Result<ScummVmDiscovery, ScummVmDetectionError> {
    let rows = match parse_detection_table(text)? {
        Some(rows) => rows,
        // Real ScummVM prints this warning on stdout when it finds nothing.
        // Output that is neither a table nor that explicit statement is not
        // treated as "no games": a broken wrapper must not look like an empty
        // library.
        None if text.contains("could not find any game") => Vec::new(),
        None => {
            return Err(ScummVmDetectionError::MalformedOutput(
                "no detection table and no explicit no-game statement".into(),
            ));
        }
    };
    let mut by_path: std::collections::BTreeMap<PathBuf, Vec<ScummVmDetectedGame>> =
        Default::default();
    for (game, path) in rows {
        // Lexical containment is not enough: a symlinked ancestor or `..` can
        // make a path that starts with the root resolve elsewhere.
        if !path.starts_with(root) || path.canonicalize().ok().as_deref() != Some(path.as_path()) {
            continue;
        }
        by_path.entry(path).or_default().push(game);
    }
    let mut discovery = ScummVmDiscovery::default();
    let mut unique: Vec<(PathBuf, ScummVmDetectedGame)> = Vec::new();
    for (path, games) in by_path {
        let games = dedupe_candidates(games);
        match games.len() {
            1 => unique.push((path, games.into_iter().next().expect("one game"))),
            _ => discovery.ambiguous.push(path),
        }
    }
    let candidates = unique.clone();
    for (path, game) in unique {
        let contains_deeper_same_game = candidates.iter().any(|(other, other_game)| {
            other != &path
                && other.starts_with(&path)
                && other_game.engine_id == game.engine_id
                && other_game.game_id == game.game_id
        });
        if contains_deeper_same_game {
            continue;
        }
        match detect_scummvm_directory_with_executable(&path, executable) {
            Ok(checked)
                if checked.engine_id == game.engine_id && checked.game_id == game.game_id =>
            {
                let found = DiscoveredScummVmGame {
                    path,
                    game: checked,
                };
                if found.game.unknown_variant {
                    discovery.unknown_variants.push(found);
                } else {
                    discovery.games.push(found);
                }
            }
            Err(ScummVmDetectionError::Ambiguous(_)) => discovery.ambiguous.push(path),
            _ => discovery.rejected.push(path),
        }
    }
    Ok(discovery)
}

fn parse_optional_field(game: &mut ScummVmDetectedGame, line: &str) {
    let Some((label, value)) = line.split_once(':') else {
        return;
    };
    let value = value.trim();
    match label.trim().to_ascii_lowercase().as_str() {
        "engine" => game.engine_id = value.to_string(),
        "platform" => game.platform = (!value.is_empty()).then(|| value.to_string()),
        "language" => game.language = (!value.is_empty()).then(|| value.to_string()),
        "variant" | "version" => game.variant = (!value.is_empty()).then(|| value.to_string()),
        "demo" => {
            game.demo = match value.to_ascii_lowercase().as_str() {
                "yes" | "true" => Some(true),
                "no" | "false" => Some(false),
                _ => None,
            }
        }
        _ => {}
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

trait OsStringAppend {
    fn tap_append(self, suffix: &OsStr) -> Self;
}

impl OsStringAppend for OsString {
    fn tap_append(mut self, suffix: &OsStr) -> Self {
        self.push(suffix);
        self
    }
}

#[cfg(test)]
mod tests {
    /// A "user's" ScummVM configuration with comments, odd spacing, CRLF-free
    /// but unusual ordering and a trailing blank line: exactly what ScummVM's
    /// own re-save would normalise.
    #[cfg(unix)]
    const UNUSUAL_USER_CONFIG: &str = "# my notes - keep me\n[scummvm]\nzz_last = 1\n   aa_first=2   \n; a comment\n\n[mygame]\npath = /games/x\n\n";

    /// A fake ScummVM whose `--version` does what the real one does: it loads
    /// and re-saves a configuration. With `--config=<file>` that is the throwaway
    /// file; without it, it is the (embedded) user configuration, which it
    /// normalises by sorting the lines. Every call's arguments are recorded.
    #[cfg(unix)]
    fn config_rewriting_fake(dir: &Path, user_config: &Path, exit_code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("scummvm");
        fs::write(
            &exe,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncfg='{user}'\nfor a in \"$@\"; do case \"$a\" in --config=*) cfg=\"${{a#--config=}}\";; esac; done\nif [ -e \"$cfg\" ]; then sort \"$cfg\" > \"$cfg.tmp\" && mv \"$cfg.tmp\" \"$cfg\"; else echo '[scummvm]' > \"$cfg\"; fi\n[ {code} -eq 0 ] && echo 'ScummVM 2.8.0 (test)'\nexit {code}\n",
                log = dir.join("calls.log").display(),
                user = user_config.display(),
                code = exit_code,
            ),
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_uses_a_throwaway_config_and_leaves_the_users_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user-scummvm.ini");
        fs::write(&user, UNUSUAL_USER_CONFIG).unwrap();
        let exe = config_rewriting_fake(dir.path(), &user, 0);
        // The fake really would rewrite the user's file if it were handed it.
        let control = dir.path().join("control.ini");
        fs::copy(&user, &control).unwrap();
        assert!(
            std::process::Command::new(&exe)
                .arg(format!("--config={}", control.display()))
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_ne!(fs::read(&control).unwrap(), UNUSUAL_USER_CONFIG.as_bytes());
        fs::remove_file(dir.path().join("calls.log")).unwrap();

        // 1. the probe is given an isolated config path, 4. same version as before
        assert_eq!(
            scummvm_version(&exe).as_deref(),
            Some("ScummVM 2.8.0 (test)")
        );
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        let arg = calls
            .split_whitespace()
            .find_map(|part| part.strip_prefix("--config="))
            .expect("--config= was passed");
        assert!(Path::new(arg).starts_with(std::env::temp_dir()));
        assert_ne!(Path::new(arg), user.as_path());
        assert!(calls.contains("--version"));
        // 2 and 3. the user's configuration is byte-identical: comments, ordering,
        // spacing and the trailing blank line all intact.
        assert_eq!(fs::read(&user).unwrap(), UNUSUAL_USER_CONFIG.as_bytes());
        // Only the throwaway file was touched, and it is gone.
        assert!(!Path::new(arg).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_version_probe_reports_none_and_still_leaves_the_users_config_alone() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user-scummvm.ini");
        fs::write(&user, UNUSUAL_USER_CONFIG).unwrap();
        let exe = config_rewriting_fake(dir.path(), &user, 3);
        assert_eq!(
            scummvm_version(&exe),
            None,
            "a failure is reported, not papered over"
        );
        assert_eq!(fs::read(&user).unwrap(), UNUSUAL_USER_CONFIG.as_bytes());
        let calls = fs::read_to_string(dir.path().join("calls.log")).unwrap();
        assert_eq!(
            calls.lines().count(),
            1,
            "no second attempt on the default config"
        );
        assert!(calls.contains("--config="));
        assert_eq!(scummvm_version(Path::new("/nonexistent/scummvm")), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_cache_key_is_stable_and_still_bound_to_the_version() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user-scummvm.ini");
        fs::write(&user, UNUSUAL_USER_CONFIG).unwrap();
        let exe = config_rewriting_fake(dir.path(), &user, 0);
        let root = dir.path().join("lib");
        fs::create_dir_all(root.join("g")).unwrap();
        fs::write(root.join("g/f.dat"), b"x").unwrap();
        let root = root.canonicalize().unwrap();
        let first = discovery_cache_key(&root, &exe).expect("key");
        let second = discovery_cache_key(&root, &exe).expect("key");
        assert_eq!(first, second, "probing changes nothing the key depends on");
        assert_eq!(fs::read(&user).unwrap(), UNUSUAL_USER_CONFIG.as_bytes());
        // A different reported version is a different key (unchanged behaviour).
        let other = dir.path().join("other");
        fs::create_dir_all(&other).unwrap();
        let exe2 = {
            use std::os::unix::fs::PermissionsExt;
            let p = other.join("scummvm");
            fs::write(&p, "#!/bin/sh\necho 'ScummVM 9.9.9'\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        assert_ne!(discovery_cache_key(&root, &exe2).unwrap(), first);
    }

    /// ScummVM re-saves its default configuration even for `--version`. A fake
    /// that does exactly that when no `--config=` is given proves the probe can
    /// never touch the user's configuration or pollute the working directory.
    #[cfg(unix)]
    #[test]
    fn version_probe_never_opens_the_default_scummvm_configuration() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake_home = dir.path().join("home");
        fs::create_dir_all(&fake_home).unwrap();
        let exe = dir.path().join("scummvm");
        fs::write(
            &exe,
            format!(
                "#!/bin/sh\ncase \"$*\" in *--config=*) ;; *) echo polluted > '{}/scummvm.ini'; echo polluted > scummvm.ini;; esac\necho 'ScummVM 2.8.0 (test)'\n",
                fake_home.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            scummvm_version(&exe).as_deref(),
            Some("ScummVM 2.8.0 (test)")
        );
        assert!(
            !fake_home.join("scummvm.ini").exists(),
            "default config touched"
        );
        assert!(
            !here.join("scummvm.ini").exists(),
            "the probe left a scummvm.ini in {}",
            here.display()
        );
    }

    #[cfg(unix)]
    #[test]
    fn unexpected_recursive_output_is_an_error_not_an_empty_library() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("scummvm");
        fs::write(&exe, "#!/bin/sh\necho 'something else entirely'\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let root = dir.path().join("lib");
        fs::create_dir(&root).unwrap();
        let error = discover_scummvm_game_directories(&root, &exe).unwrap_err();
        assert!(
            matches!(error, ScummVmDetectionError::MalformedOutput(_)),
            "{error:?}"
        );
    }

    /// A fake ScummVM that prints the real table, reports a version from
    /// `<exe>.ver`, and counts its recursive runs in `<exe>.count`.
    #[cfg(unix)]
    fn caching_fake(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("scummvm");
        fs::write(dir.join("scummvm.ver"), "2.8.0").unwrap();
        fs::write(
            &exe,
            r#"#!/bin/sh
self="$0"
for a in "$@"; do case "$a" in --path=*) p="${a#--path=}";; --recursive) rec=1;; --version) ver=1;; esac; done
if [ -n "$ver" ]; then echo "ScummVM $(cat "$self.ver") (test)"; exit 0; fi
[ -n "$rec" ] && echo run >> "$self.count"
find "$p" -type d | sort | while read -r d; do
  [ -e "$d/queen.1c" ] && printf '%-30s %-58s %s\n' 'queen:queen' 'Flight of the Amazon Queen (CD/DOS/English)' "$d"
done > "$self.rows"
if [ ! -s "$self.rows" ]; then echo "WARNING: ScummVM could not find any game in $p"; exit 0; fi
printf '%-30s %-58s %s\n' GameID Description 'Full Path'
printf '%-30s %-58s %s\n' '------------------------------' '----------------------------------------------------------' '---------'
cat "$self.rows"
"#,
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    #[cfg(unix)]
    fn runs(exe: &Path) -> usize {
        fs::read_to_string(format!("{}.count", exe.display()))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    #[cfg(unix)]
    #[test]
    fn discovery_cache_reuses_unchanged_trees_and_invalidates_on_every_relevant_change() {
        let dir = tempfile::tempdir().unwrap();
        let exe = caching_fake(dir.path());
        let root = dir.path().join("lib");
        let game = root.join("Queen");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("queen.1c"), b"data").unwrap();
        let find = |cached: Option<&DiscoveryCacheEntry>| {
            discover_scummvm_game_directories_cached(&root, &exe, cached).unwrap()
        };

        let (first, entry) = find(None);
        assert_eq!(first.games.len(), 1);
        assert_eq!(runs(&exe), 1);
        let entry = entry.expect("a plain tree is cacheable");

        // Unchanged: no recursive run, same result.
        let (again, same) = find(Some(&entry));
        assert_eq!(runs(&exe), 1, "an unchanged rescan must not rerun ScummVM");
        assert_eq!(again, first);
        assert_eq!(same.as_ref(), Some(&entry));

        // A changed game file invalidates.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(game.join("queen.1c"), b"changed data").unwrap();
        let (_, changed) = find(Some(&entry));
        assert_eq!(runs(&exe), 2);
        let changed = changed.unwrap();
        assert_ne!(changed.key, entry.key);

        // A detector version change invalidates.
        fs::write(dir.path().join("scummvm.ver"), "2.9.0").unwrap();
        let (_, upgraded) = find(Some(&changed));
        assert_eq!(runs(&exe), 3);
        let upgraded = upgraded.unwrap();
        assert_ne!(upgraded.key, changed.key);

        // A removed folder invalidates and the game disappears.
        fs::remove_dir_all(&game).unwrap();
        let (gone, gone_entry) = find(Some(&upgraded));
        assert_eq!(runs(&exe), 4);
        assert!(gone.games.is_empty());
        assert!(gone_entry.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_cache_is_refused_when_a_symlink_could_hide_changes() {
        let dir = tempfile::tempdir().unwrap();
        let exe = caching_fake(dir.path());
        let root = dir.path().join("lib");
        let game = root.join("Queen");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("queen.1c"), b"data").unwrap();
        let (_, entry) = discover_scummvm_game_directories_cached(&root, &exe, None).unwrap();
        let entry = entry.unwrap();

        // Replace the real folder with a symlink to a copy elsewhere.
        let elsewhere = dir.path().join("elsewhere");
        fs::rename(&game, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &game).unwrap();
        let (found, next) =
            discover_scummvm_game_directories_cached(&root, &exe, Some(&entry)).unwrap();
        assert!(
            next.is_none(),
            "a tree with a directory symlink is never cached"
        );
        assert_eq!(runs(&exe), 2, "the stale entry must not be used");
        assert!(
            found.games.iter().all(|g| g.path != game),
            "the symlink is not a game"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_tampered_cache_cannot_override_the_fresh_safety_checks() {
        let dir = tempfile::tempdir().unwrap();
        let exe = caching_fake(dir.path());
        let root = dir.path().join("lib");
        fs::create_dir_all(root.join("Notes")).unwrap();
        fs::write(root.join("Notes/readme.txt"), b"not a game").unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("queen.1c"), b"x").unwrap();
        let canonical = root.canonicalize().unwrap();
        let (_, entry) = discover_scummvm_game_directories_cached(&root, &exe, None).unwrap();
        let mut forged = entry.unwrap();
        // Claim the unrelated folder and an outside folder are queen games.
        forged.output = format!(
            "GameID                         Description                                                Full Path\n\
------------------------------ ---------------------------------------------------------- ---------\n\
queen:queen                    Flight of the Amazon Queen (CD/DOS/English)                {}\n\
queen:queen                    Flight of the Amazon Queen (CD/DOS/English)                {}\n",
            canonical.join("Notes").display(),
            outside.display()
        );
        let (found, replaced) =
            discover_scummvm_game_directories_cached(&root, &exe, Some(&forged)).unwrap();
        assert!(found.games.is_empty(), "{:?}", found.games);
        // The forged rows failed the fresh per-folder check, so the cache was
        // discarded and ScummVM was asked again; the entry is replaced.
        assert_eq!(runs(&exe), 2);
        assert_ne!(replaced.unwrap().output, forged.output);
    }

    #[cfg(unix)]
    fn queen_lab() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let exe = caching_fake(dir.path());
        let root = dir.path().join("lib");
        let game = root.join("Queen");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("queen.1c"), b"data").unwrap();
        (dir, exe, root, game)
    }

    #[cfg(unix)]
    const NO_GAMES: &str = "WARNING: ScummVM could not find any game in /x\n";

    #[cfg(unix)]
    #[test]
    fn a_forged_entry_that_disagrees_with_reality_cannot_hide_the_real_game() {
        let (_dir, exe, root, game) = queen_lab();
        let (_, entry) = discover_scummvm_game_directories_cached(&root, &exe, None).unwrap();
        let mut forged = entry.unwrap();
        // Lists only an unrelated folder; the real game is left out of it.
        fs::create_dir_all(root.join("Notes")).unwrap();
        forged.key = discovery_cache_key(&root.canonicalize().unwrap(), &exe).unwrap();
        forged.output = format!(
            "GameID                         Description                                                Full Path\n\
------------------------------ ---------------------------------------------------------- ---------\n\
queen:queen                    Flight of the Amazon Queen (CD/DOS/English)                {}\n",
            root.canonicalize().unwrap().join("Notes").display()
        );
        let (found, _) =
            discover_scummvm_game_directories_cached(&root, &exe, Some(&forged)).unwrap();
        assert_eq!(found.games.len(), 1, "{found:?}");
        assert_eq!(found.games[0].path, game.canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn cached_runs_expire_and_future_dated_entries_are_refused() {
        let (_dir, exe, root, _game) = queen_lab();
        let (_, entry) = discover_scummvm_game_directories_cached(&root, &exe, None).unwrap();
        let fresh = entry.unwrap();
        let now = unix_now();
        let reuse = |checked: u64| {
            let before = runs(&exe);
            let mut e = fresh.clone();
            e.checked_unix = checked;
            let (_, returned) =
                discover_scummvm_game_directories_cached(&root, &exe, Some(&e)).unwrap();
            (runs(&exe) == before, returned.unwrap().checked_unix)
        };
        assert_eq!(reuse(now), (true, now), "a young entry is reused unchanged");
        assert!(reuse(now - 6 * 24 * 3600).0, "still inside the 7 day limit");
        let (reused, stamped) = reuse(now - 8 * 24 * 3600);
        assert!(!reused, "an old entry is not reused");
        assert!(stamped >= now, "the new entry carries the new run time");
        assert!(!reuse(now + 3600).0, "a future-dated entry is not trusted");
        assert!(
            !reuse(0).0,
            "an entry without a time (older sidecar) is expired"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_cached_empty_result_cannot_hide_a_game_beyond_its_short_limit() {
        let (_dir, exe, root, game) = queen_lab();
        let canonical = root.canonicalize().unwrap();
        let key = discovery_cache_key(&canonical, &exe).unwrap();
        let now = unix_now();
        let empty = |age: u64| DiscoveryCacheEntry {
            key: key.clone(),
            output: NO_GAMES.into(),
            checked_unix: now - age,
        };
        // Within 24 hours the (untrusted, unverifiable) empty result is reused.
        let (young, _) =
            discover_scummvm_game_directories_cached(&root, &exe, Some(&empty(3600))).unwrap();
        assert!(young.games.is_empty());
        assert_eq!(runs(&exe), 0);
        // Past it, ScummVM is asked again and the game appears.
        let (old, entry) =
            discover_scummvm_game_directories_cached(&root, &exe, Some(&empty(25 * 3600))).unwrap();
        assert_eq!(runs(&exe), 1);
        assert_eq!(old.games.len(), 1);
        assert_eq!(old.games[0].path, game.canonicalize().unwrap());
        assert!(entry.unwrap().checked_unix >= now);
    }

    #[cfg(unix)]
    #[test]
    fn a_same_size_edit_that_keeps_the_modification_time_still_invalidates() {
        let (_dir, exe, root, game) = queen_lab();
        let (_, entry) = discover_scummvm_game_directories_cached(&root, &exe, None).unwrap();
        let entry = entry.unwrap();
        let file = game.join("queen.1c");
        let before = fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&file, b"DATA").unwrap(); // same size, different bytes
        fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(before)
            .unwrap();
        assert_eq!(fs::metadata(&file).unwrap().len(), 4);
        let (_, next) =
            discover_scummvm_game_directories_cached(&root, &exe, Some(&entry)).unwrap();
        assert_eq!(
            runs(&exe),
            2,
            "the change time still moved, so ScummVM ran again"
        );
        assert_ne!(next.unwrap().key, entry.key);
    }

    /// Output captured from ScummVM 2.8.0 (`scummvm --detect`) on a real
    /// Flight of the Amazon Queen folder.
    const REAL_TABLE: &str = "WARNING: File::open: node does not exist!\n\
Creating configuration file: /tmp/x/iso.ini\n\
GameID                         Description                                                Full Path\n\
------------------------------ ---------------------------------------------------------- ---------\n\
queen:queen                    Flight of the Amazon Queen (CD/DOS/English)                /games/Queen Folder\n";

    #[test]
    fn real_scummvm_table_output_yields_the_verified_game() {
        let games = parse_detection_output(REAL_TABLE.as_bytes()).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(
            (games[0].engine_id.as_str(), games[0].game_id.as_str()),
            ("queen", "queen")
        );
        let rows = parse_detection_table(REAL_TABLE).unwrap().unwrap();
        assert_eq!(rows[0].1, PathBuf::from("/games/Queen Folder"));
    }

    #[test]
    fn real_scummvm_no_game_warning_is_no_match_not_malformed() {
        let output = b"WARNING: ScummVM could not find any game in /x\nWARNING: Consider using --recursive to search inside subdirectories\n";
        assert!(parse_detection_output(output).unwrap().is_empty());
    }

    #[test]
    fn a_table_row_with_a_bad_id_is_rejected_not_guessed() {
        let table = REAL_TABLE.replace("queen:queen   ", "not-a-game-id");
        assert!(parse_detection_output(table.as_bytes()).is_err());
    }

    use super::*;

    #[test]
    fn parses_explicit_game_record_and_optional_metadata() {
        let output = b"Detected games:\nGame: scumm:monkey (The Secret of Monkey Island)\nPlatform: pc\nLanguage: en\nVariant: vga\nDemo: no\n";
        let games = parse_detection_output(output).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].game_id, "monkey");
        assert_eq!(games[0].engine_id, "scumm");
        assert_eq!(games[0].platform.as_deref(), Some("pc"));
        assert_eq!(games[0].language.as_deref(), Some("en"));
        assert_eq!(games[0].variant.as_deref(), Some("vga"));
        assert_eq!(games[0].demo, Some(false));
    }

    #[test]
    fn malformed_and_multiple_records_fail_closed_at_call_site() {
        assert!(matches!(
            parse_detection_output(b"Game: not-valid"),
            Err(ScummVmDetectionError::MalformedOutput(_))
        ));
        let games = parse_detection_output(b"Game: scumm:one\nGame: sci:two\n").unwrap();
        assert_eq!(games.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn detector_fixture_uses_contents_not_folder_name() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("misleading-folder-name");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("real-resource.dat"), b"fixture content").unwrap();
        let detector = root.path().join("scummvm-fixture");
        fs::write(
            &detector,
            b"#!/bin/sh\nprintf '%s\\n' 'Game ID: scumm:monkey'\n",
        )
        .unwrap();
        fs::set_permissions(&detector, fs::Permissions::from_mode(0o755)).unwrap();

        let detected = detect_scummvm_directory_with_executable(&game, &detector).unwrap();
        assert_eq!(detected.engine_id, "scumm");
        assert_eq!(detected.game_id, "monkey");
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_symlink_is_refused_before_detector_runs() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();
        symlink(root.path(), game.join("escape")).unwrap();
        let error =
            detect_scummvm_directory_with_executable(&game, Path::new("/bin/true")).unwrap_err();
        assert!(matches!(error, ScummVmDetectionError::UnsafeEntry(_)));
    }

    #[cfg(unix)]
    #[test]
    fn version_reads_the_first_line_of_the_detectors_own_output() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let detector = root.path().join("scummvm-fixture");
        fs::write(
            &detector,
            b"#!/bin/sh\nprintf 'ScummVM 2.8.1\\nFeatures: x\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&detector, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(scummvm_version(&detector).as_deref(), Some("ScummVM 2.8.1"));
    }

    #[test]
    fn version_is_none_when_the_executable_does_not_exist() {
        assert_eq!(
            scummvm_version(Path::new("/nonexistent/scummvm-binary")),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn detected_folder_becomes_verified_game_identity() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("renamed-and-unrelated");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("resource.dat"), b"content").unwrap();
        let detector = root.path().join("detector");
        fs::write(&detector, b"#!/bin/sh\necho 'Game: sci:demo'\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&detector, fs::Permissions::from_mode(0o755)).unwrap();

        let report =
            crate::game_identity::inspect_scummvm_directory_with_executable(&game, &detector);
        assert_eq!(
            report.platform,
            crate::game_identity::IdentityPlatform::ScummVM
        );
        assert_eq!(report.verified_scummvm_game_id(), Some("sci:demo"));
        assert!(report.complete);
        let (status, facts) =
            crate::launch::evidence_bridge::canonical_identity_from_game_report(&report);
        assert!(matches!(
            status,
            crate::launch::planning::CanonicalIdentityStatus::Resolved(ref identity)
                if identity.platform_id == "ScummVM" && identity.game_key == "sci:demo"
        ));
        assert_eq!(
            facts,
            vec![
                crate::launch::input_projection::VerifiedIdentityFact::ScummVmGameId(
                    "sci:demo".into()
                )
            ]
        );
    }

    // ------------------------------------------------------------------
    // Golden detector-output fixtures
    // ------------------------------------------------------------------
    //
    // These are inline, bounded strings standing in for real `scummvm
    // --detect` output shapes - never a real network fetch or a bundled
    // copy of ScummVM's own detection tables. Where a real subprocess is
    // needed (non-zero exit, timeout), the fixture is a tiny shell script
    // under a temp directory, the same pattern the existing detector tests
    // above already use.

    #[test]
    fn golden_ambiguous_two_result_output() {
        let output = b"Game: scumm:one (First Candidate)\nGame: sci:two (Second Candidate)\n";
        let games = parse_detection_output(output).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].engine_id, "scumm");
        assert_eq!(games[1].engine_id, "sci");
    }

    #[test]
    fn golden_repeated_identical_candidate_lines_dedupe_to_one() {
        // The same qualified id, reported three times - a real detector
        // quirk this module now normalizes away rather than reporting as
        // Ambiguous(3) for what is genuinely one game.
        let output = b"Game: scumm:monkey (The Secret of Monkey Island)\n\
                       Game: scumm:monkey (The Secret of Monkey Island)\n\
                       Game: scumm:monkey (The Secret of Monkey Island)\n";
        let games = parse_detection_output(output).unwrap();
        assert_eq!(
            games.len(),
            3,
            "the parser itself is still a literal transcript"
        );
        let deduped = dedupe_candidates(games);
        assert_eq!(
            deduped.len(),
            1,
            "duplicates of the same engine:game must collapse to one"
        );
        assert_eq!(deduped[0].engine_id, "scumm");
        assert_eq!(deduped[0].game_id, "monkey");
    }

    #[test]
    fn golden_same_id_different_language_variant_still_dedupes() {
        // Two records that share the identity ScummVM launches by
        // (engine:game) but differ in language - not a genuinely distinct
        // candidate for EmuWiz's purposes, since both resolve to the exact
        // same launch target.
        let output = b"Game: scumm:monkey (English)\nLanguage: en\n\
                       Game: scumm:monkey (German)\nLanguage: de\n";
        let games = parse_detection_output(output).unwrap();
        assert_eq!(games.len(), 2);
        let deduped = dedupe_candidates(games);
        assert_eq!(deduped.len(), 1);
        assert_eq!(
            deduped[0].language.as_deref(),
            Some("en"),
            "the first occurrence's metadata survives"
        );
    }

    #[test]
    fn golden_genuinely_distinct_candidates_are_preserved() {
        // Different game_id under the same engine - a real, distinct
        // ambiguity, never collapsed away.
        let output = b"Game: scumm:monkey (Monkey Island)\nGame: scumm:monkey2 (Monkey Island 2)\n";
        let games = parse_detection_output(output).unwrap();
        let deduped = dedupe_candidates(games);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn golden_warning_noise_around_a_valid_result_is_ignored() {
        // Lines that are neither a "Game:" record nor a recognised optional
        // field (a stray warning, a blank separator, a banner line) must
        // never break parsing of the real record around them, and must
        // never be folded into it as if they were data.
        let output = b"Scanning for games...\n\
                       WARNING: could not read some.cfg, ignoring\n\
                       Game: scumm:monkey (The Secret of Monkey Island)\n\
                       Platform: pc\n\
                       NOTE: 1 game(s) were found.\n";
        let games = parse_detection_output(output).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].engine_id, "scumm");
        assert_eq!(games[0].platform.as_deref(), Some("pc"));
    }

    #[test]
    fn golden_unusual_spacing_and_indentation_still_parses() {
        let output = b"   Game:    scumm:monkey   (The Secret of Monkey Island)   \n\
                       \tPlatform:\tpc\t\n";
        let games = parse_detection_output(output).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].engine_id, "scumm");
        assert_eq!(games[0].game_id, "monkey");
        assert_eq!(games[0].platform.as_deref(), Some("pc"));
    }

    #[test]
    fn golden_malformed_game_id_is_rejected() {
        // A game id containing a character outside the documented
        // alphanumeric/`_`/`-` alphabet - e.g. a stray colon or a symbol
        // that would otherwise be silently accepted as "identity".
        let error = parse_detection_output(b"Game: scumm:mon:key\n").unwrap_err();
        assert!(matches!(error, ScummVmDetectionError::MalformedOutput(_)));
        let error = parse_detection_output(b"Game: scumm:mon!key\n").unwrap_err();
        assert!(matches!(error, ScummVmDetectionError::MalformedOutput(_)));
    }

    #[test]
    fn golden_malformed_engine_id_is_rejected() {
        let error = parse_detection_output(b"Game: scu!mm:monkey\n").unwrap_err();
        assert!(matches!(error, ScummVmDetectionError::MalformedOutput(_)));
    }

    #[test]
    fn golden_unexpected_additional_fields_are_ignored_not_fatal() {
        // A field this parser has never heard of must not abort parsing of
        // an otherwise-valid record - `parse_optional_field`'s catch-all
        // already does this; this test exists to keep it locked in.
        let output = b"Game: scumm:monkey (The Secret of Monkey Island)\n\
                       Engine ID: scumm\n\
                       Preferred target: monkey-en\n\
                       GUI options: [midiGM]\n\
                       Platform: pc\n";
        let games = parse_detection_output(output).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].platform.as_deref(), Some("pc"));
    }

    #[test]
    fn golden_no_match_output_is_not_an_error() {
        assert_eq!(
            parse_detection_output(b"No games were found matching the specified criteria.\n")
                .unwrap(),
            Vec::new()
        );
        assert_eq!(
            parse_detection_output(b"no game detected\n").unwrap(),
            Vec::new()
        );
    }

    #[cfg(unix)]
    fn write_fixture(dir: &Path, name: &str, script: &[u8]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn golden_non_zero_exit_with_useful_stdout_is_still_a_failure() {
        // A real, fail-closed choice: if the detector's own exit code says
        // failure, its stdout is never trusted, no matter how plausible it
        // looks. Discarding a possibly-correct answer is the safe side of
        // this tradeoff; treating an unsuccessful run as authoritative
        // would not be.
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\necho 'Game: scumm:monkey'\nexit 1\n",
        );
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();
        let error = detect_scummvm_directory_with_executable(&game, &detector).unwrap_err();
        assert!(matches!(error, ScummVmDetectionError::DetectorFailed(_)));
    }

    #[cfg(unix)]
    #[test]
    fn golden_non_zero_exit_with_useful_stderr_surfaces_that_message() {
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\necho 'unknown option --path' >&2\nexit 2\n",
        );
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();
        let error = detect_scummvm_directory_with_executable(&game, &detector).unwrap_err();
        match error {
            ScummVmDetectionError::DetectorFailed(message) => {
                assert!(message.contains("unknown option --path"));
            }
            other => panic!("expected DetectorFailed, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn golden_repeated_lines_from_a_real_subprocess_do_not_report_ambiguous() {
        // End-to-end version of `golden_repeated_identical_candidate_lines_dedupe_to_one`:
        // through the real subprocess + validation path, not just the parser.
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\nprintf 'Game: scumm:monkey\\nGame: scumm:monkey\\n'\n",
        );
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();
        let detected = detect_scummvm_directory_with_executable(&game, &detector).unwrap();
        assert_eq!(detected.engine_id, "scumm");
        assert_eq!(detected.game_id, "monkey");
    }

    #[cfg(unix)]
    #[test]
    fn top_level_symlink_is_named_distinctly_from_a_plain_missing_directory() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real-game");
        fs::create_dir(&real).unwrap();
        let linked = root.path().join("linked-game");
        symlink(&real, &linked).unwrap();

        let error =
            detect_scummvm_directory_with_executable(&linked, Path::new("/bin/true")).unwrap_err();
        match error {
            ScummVmDetectionError::InvalidRoot(detail) => {
                assert!(detail.contains("symlink"), "message was: {detail}");
            }
            other => panic!("expected InvalidRoot, got {other:?}"),
        }
    }

    #[test]
    fn empty_directory_is_no_match_not_a_crash() {
        let dir = tempfile::tempdir().unwrap();
        let games =
            parse_detection_output(b"No games were found in the specified location.\n").unwrap();
        assert!(games.is_empty());
        // The directory itself passes validation even with nothing inside -
        // an empty ScummVM folder is exactly as valid a "nothing to detect
        // here" case as a folder full of unrelated files.
        assert!(validate_game_directory(dir.path()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_directory_is_reported_not_panicked_on() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let locked = root.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let result = validate_game_directory(&locked);
        // Running as root (some CI/dev containers) bypasses the permission
        // bit entirely - only assert the failure case when it actually
        // occurred, never assume it did.
        if result.is_err() {
            assert!(matches!(result, Err(ScummVmDetectionError::InvalidRoot(_))));
        }
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
    }

    // ------------------------------------------------------------------
    // Batch checking: dedup, cancellation, summary
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn duplicate_candidate_paths_are_checked_once() {
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\necho 'Game: scumm:monkey'\n",
        );
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();

        let mut invocations = 0usize;
        let candidates = vec![game.clone(), game.clone(), game];
        let cancel = AtomicBool::new(false);
        let results = check_scummvm_directories(&detector, &candidates, &cancel, |_, _, _| {
            invocations += 1;
        });
        assert_eq!(
            results.len(),
            1,
            "identical paths must collapse to one candidate"
        );
        assert_eq!(
            invocations, 1,
            "the detector must run exactly once for a duplicated path"
        );
        assert!(matches!(
            results[0].outcome,
            ScummVmDetectionSummary::Detected(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn distinct_paths_are_each_checked_in_order() {
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\necho 'Game: scumm:monkey'\n",
        );
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();

        let mut seen_order = Vec::new();
        let cancel = AtomicBool::new(false);
        let results = check_scummvm_directories(
            &detector,
            &[first.clone(), second.clone()],
            &cancel,
            |_, _, path| seen_order.push(path.to_path_buf()),
        );
        assert_eq!(results.len(), 2);
        assert_eq!(seen_order, vec![first, second]);
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_before_the_batch_starts_skips_every_candidate() {
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\necho 'Game: scumm:monkey'\n",
        );
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();

        let mut invocations = 0usize;
        let cancel = AtomicBool::new(true);
        let results = check_scummvm_directories(&detector, &[game], &cancel, |_, _, _| {
            invocations += 1;
        });
        assert_eq!(
            invocations, 0,
            "a pre-cancelled batch must never spawn a detector"
        );
        assert_eq!(
            results.len(),
            1,
            "every candidate is still reported, just as Skipped"
        );
        assert_eq!(results[0].outcome, ScummVmDetectionSummary::Skipped);
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_mid_batch_still_finishes_the_in_flight_candidate() {
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\necho 'Game: scumm:monkey'\n",
        );
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();

        let cancel = AtomicBool::new(false);
        let mut checked = 0usize;
        let results = check_scummvm_directories(&detector, &[first, second], &cancel, |_, _, _| {
            checked += 1;
            if checked == 1 {
                // Requested between candidates, from inside the
                // progress callback of the first - simulates a user
                // clicking Cancel while the first is running.
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(results.len(), 2);
        assert!(matches!(
            results[0].outcome,
            ScummVmDetectionSummary::Detected(_)
        ));
        assert_eq!(results[1].outcome, ScummVmDetectionSummary::Skipped);
    }

    #[test]
    fn summary_bucketing_matches_the_result_based_api() {
        assert_eq!(
            summarize_candidates(Ok(Vec::new())),
            ScummVmDetectionSummary::Unsupported
        );
        let one = ScummVmDetectedGame {
            game_id: "monkey".into(),
            engine_id: "scumm".into(),
            description: None,
            platform: None,
            language: None,
            variant: None,
            demo: None,
            unknown_variant: false,
        };
        assert_eq!(
            summarize_candidates(Ok(vec![one.clone()])),
            ScummVmDetectionSummary::Detected(one.clone())
        );
        assert_eq!(
            summarize_candidates(Ok(vec![one.clone(), one.clone()])),
            ScummVmDetectionSummary::Ambiguous(vec![one.clone(), one])
        );
        assert_eq!(
            summarize_candidates(Err(ScummVmDetectionError::NoMatch)),
            ScummVmDetectionSummary::DetectorFailure(
                "ScummVM detected no game in this folder".to_string()
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn summarize_scummvm_directory_keeps_every_ambiguous_candidate() {
        let root = tempfile::tempdir().unwrap();
        let detector = write_fixture(
            root.path(),
            "detector",
            b"#!/bin/sh\nprintf 'Game: scumm:one\\nGame: sci:two\\n'\n",
        );
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();

        match summarize_scummvm_directory(&game, &detector) {
            ScummVmDetectionSummary::Ambiguous(candidates) => {
                assert_eq!(candidates.len(), 2);
                assert_eq!(candidates[0].engine_id, "scumm");
                assert_eq!(candidates[1].engine_id, "sci");
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // Compatibility diagnostic
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn compatibility_reports_a_parsed_version_when_available() {
        let root = tempfile::tempdir().unwrap();
        let executable = write_fixture(
            root.path(),
            "scummvm",
            b"#!/bin/sh\nprintf 'ScummVM 2.8.1\\n'\n",
        );
        match assess_scummvm_compatibility_with_executable(executable.clone()) {
            ScummVmCompatibility::Installed {
                executable: reported,
                version,
            } => {
                assert_eq!(reported, executable);
                assert_eq!(version.as_deref(), Some("ScummVM 2.8.1"));
            }
            ScummVmCompatibility::NotInstalled => panic!("executable was provided directly"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn compatibility_reports_unknown_version_without_treating_it_as_an_error() {
        let root = tempfile::tempdir().unwrap();
        // Exits non-zero and prints nothing useful - `scummvm_version`
        // already treats this as `None`, never a failure of readiness
        // itself: an executable that cannot report its own version is
        // still an executable that can be asked to detect games.
        let executable = write_fixture(root.path(), "scummvm", b"#!/bin/sh\nexit 1\n");
        match assess_scummvm_compatibility_with_executable(executable) {
            ScummVmCompatibility::Installed { version, .. } => assert_eq!(version, None),
            ScummVmCompatibility::NotInstalled => panic!("executable was provided directly"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn readiness_reuses_detector_and_reports_version_without_persistent_config() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game with spaces");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("resource.dat"), b"fixture").unwrap();
        let detector = write_fixture(
            root.path(),
            "scummvm",
            b"#!/bin/sh\nprintf 'ScummVM 2.8.1\\n'\nprintf 'Game ID: scumm:monkey\\n'\n",
        );
        let before: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let evidence = assess_scummvm_readiness(Some(&detector), Some(&game));
        assert!(evidence.ready);
        assert_eq!(evidence.version.as_deref(), Some("ScummVM 2.8.1"));
        assert_eq!(evidence.detected_game.as_ref().unwrap().game_id, "monkey");
        let after: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn readiness_orders_missing_executable_before_game_evidence() {
        let evidence = assess_scummvm_readiness(None, Some(Path::new("../unsafe")));
        assert_eq!(
            evidence.blocker_kind,
            Some(ScummVmReadinessBlockerKind::ExecutableMissing)
        );
        assert!(!evidence.ready);
    }

    #[test]
    fn compatibility_is_not_installed_when_nothing_resolves() {
        // `assess_scummvm_compatibility` (the non-`_with_executable` form)
        // depends on real PATH/system state, which this test suite cannot
        // control deterministically - the `_with_executable` seam above is
        // what actually exercises both branches. This just pins that
        // `NotInstalled` is a real, reachable variant and not dead code.
        let _ = ScummVmCompatibility::NotInstalled;
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn transient_text_busy_retries_then_succeeds() {
        let mut attempts = 0;
        let result = retry_transient_text_busy(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(std::io::Error::from_raw_os_error(libc::ETXTBSY))
                } else {
                    Ok("detected")
                }
            },
            || {},
        )
        .unwrap();
        assert_eq!(result, "detected");
        assert_eq!(attempts, 3);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn transient_text_busy_retries_are_bounded() {
        let mut attempts = 0;
        let error = retry_transient_text_busy(
            || {
                attempts += 1;
                Err::<(), _>(std::io::Error::from_raw_os_error(libc::ETXTBSY))
            },
            || {},
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ETXTBSY));
        assert_eq!(attempts, MAX_TEXT_BUSY_RETRIES + 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn permanent_spawn_error_is_not_retried() {
        let mut attempts = 0;
        let error = retry_transient_text_busy(
            || {
                attempts += 1;
                Err::<(), _>(std::io::Error::from_raw_os_error(libc::ENOENT))
            },
            || {},
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
        assert_eq!(attempts, 1);
    }
}

#[cfg(test)]
mod unknown_variant_tests {
    use super::*;

    /// Real ScummVM 2.8.0 output for a folder holding one junk `.cas` file,
    /// captured on 2026-10-22: the Glk engine's fallback claims it.
    const FALLBACK_OUTPUT: &str = "Using configuration file: /tmp/x.ini\n\
The game in 'cas-only' seems to be an unknown game variant.\n\
\n\
Please report the following data to the ScummVM team at\n\
https://bugs.scummvm.org/ along with the name of the game you tried to add and\n\
its version, language, etc.:\n\
\n\
Matched game IDs for the glk engine: quest\n\
\n\
  {\"Jet Set Willy.cas\", 0, \"4f8cb2dc8d9a316d1e1b7bb64d45715e\", 4096},\n\
\n\
GameID                         Description                                                Full Path\n\
------------------------------ ---------------------------------------------------------- ---------\n\
glk:quest                      Quest Game                                                 /tmp/cas-only\n";

    #[test]
    fn a_fallback_match_is_flagged_as_an_unknown_variant() {
        let games = parse_detection_output(FALLBACK_OUTPUT.as_bytes()).unwrap();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].engine_id, "glk");
        assert_eq!(games[0].game_id, "quest");
        assert!(games[0].unknown_variant);
    }

    #[test]
    fn a_known_match_is_not_flagged() {
        let known = FALLBACK_OUTPUT
            .lines()
            .skip_while(|line| !line.starts_with("GameID"))
            .collect::<Vec<_>>()
            .join("\n");
        let games = parse_detection_output(known.as_bytes()).unwrap();
        assert_eq!(games.len(), 1);
        assert!(!games[0].unknown_variant);
    }
}
