//! Read-only ScummVM game-folder detection.
//!
//! ScummVM owns the detection tables and their update/licensing lifecycle.
//! EmuWiz therefore asks the locally installed native ScummVM executable to
//! run its documented `--detect` command instead of bundling or scraping a
//! second database. The command is run with an isolated temporary config so
//! detection cannot update the user's ScummVM configuration.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    match games.len() {
        0 => Err(ScummVmDetectionError::NoMatch),
        1 => Ok(games.into_iter().next().expect("one game")),
        count => Err(ScummVmDetectionError::Ambiguous(count)),
    }
}

fn validate_game_directory(path: &Path) -> Result<(), ScummVmDetectionError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ScummVmDetectionError::InvalidRoot(error.to_string()))?;
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
}
