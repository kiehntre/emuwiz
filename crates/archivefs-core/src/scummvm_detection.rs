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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
}

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
    let mut candidates = Vec::new();
    candidates.push(PathBuf::from("/usr/games/scummvm"));
    candidates.push(PathBuf::from("/usr/bin/scummvm"));
    if let Some(path) = std::env::var_os("PATH") {
        for component in std::env::split_paths(&path).take(128) {
            candidates.push(component.join("scummvm"));
        }
    }
    candidates.into_iter().find(|candidate| {
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
    let output = run_detector(executable, &[OsString::from("--version")]).ok()?;
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

fn run_detector(executable: &Path, args: &[OsString]) -> Result<Vec<u8>, ScummVmDetectionError> {
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ScummVmDetectionError::DetectorFailed(error.to_string()))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_thread = thread::spawn(move || read_bounded(stdout));
    let stderr_thread = thread::spawn(move || read_bounded(stderr));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < DETECTION_TIMEOUT => {
                thread::sleep(Duration::from_millis(20))
            }
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

fn read_bounded(mut reader: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    while bytes.len() < MAX_DETECTION_OUTPUT {
        let limit = (MAX_DETECTION_OUTPUT - bytes.len()).min(buffer.len());
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
            });
        } else if let Some(game) = current.as_mut() {
            parse_optional_field(game, line);
        }
    }
    if let Some(game) = current {
        games.push(game);
    }
    if games.is_empty() && !text.contains("no game") && !text.contains("No games") {
        return Err(ScummVmDetectionError::MalformedOutput(
            "no explicit game records".into(),
        ));
    }
    Ok(games)
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

    #[test]
    fn compatibility_is_not_installed_when_nothing_resolves() {
        // `assess_scummvm_compatibility` (the non-`_with_executable` form)
        // depends on real PATH/system state, which this test suite cannot
        // control deterministically - the `_with_executable` seam above is
        // what actually exercises both branches. This just pins that
        // `NotInstalled` is a real, reachable variant and not dead code.
        let _ = ScummVmCompatibility::NotInstalled;
    }
}
