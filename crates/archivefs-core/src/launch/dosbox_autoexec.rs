//! Read-only review of a DOS game directory's DOSBox `[autoexec]` section, and
//! the explicit approval a launch must carry.
//!
//! DOSBox runs whatever the config's `[autoexec]` lists, inside the emulated
//! machine. Nothing here runs it. The review reads the lines, decides what each
//! one *would* do, and answers three questions before a launch may be offered:
//!
//! - Is every host path the config mounts inside the game directory?
//! - Is every line one this review understands (a fail-closed allowlist)?
//! - Is there exactly one start command, and does the program it names exist
//!   in the game directory as a regular `.exe`, `.com` or `.bat` file?
//!
//! The start command is never guessed from a file name. It is the one line the
//! config itself declares, shown to the user verbatim. An approval
//! ([`DosBoxStartApproval`]) can only be minted from a clean review, and it is
//! bound to the exact config bytes (SHA-256), the exact command line, the start
//! file's identity and the game directory's identity. A launch re-reviews and
//! refuses if any of that moved.
//!
//! # What this review does not cover
//!
//! - It never opens the start program or any batch file it names. Their
//!   contents are not reviewed, and a program found in the folder says nothing
//!   about whether the folder is a complete game.
//! - DOSBox also reads the user's own primary config before `-conf`. That
//!   file belongs to the user and is not reviewed here.
//! - Options outside `[autoexec]` are only checked for a short list of
//!   settings that reach host files or devices (capture folders, serial and
//!   parallel passthrough). Other options are not reviewed.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dosbox_config_evidence::MAX_DOSBOX_CONFIG_BYTES;
use crate::launch::dosbox_command::{DosBoxConfigStatus, discover_dosbox_config};
use crate::launch::process_spawn::{CapturedFileIdentity, open_regular_file_nonblocking};
use crate::safe_read::TrustedRoots;

/// Most `[autoexec]` lines the review reads. Longer sections are refused.
pub const MAX_REVIEWED_AUTOEXEC_LINES: usize = 128;

/// Most ancestor folders searched when looking for the folder that carries the
/// `dosbox.conf` for a selected file.
pub const MAX_GAME_DIRECTORY_ASCENT: usize = 4;

/// What one reviewed `[autoexec]` line would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoexecEffect {
    /// A comment, echo, `cls`, `set` and similar lines with no host effect.
    Harmless,
    /// `mount` / `imgmount` of a path confined to the game directory.
    Mount { drive: char },
    /// `c:` style drive switch.
    SwitchDrive(char),
    /// `cd` inside an already mounted drive.
    ChangeDirectory,
    /// `exit`.
    Exit,
    /// The line that starts the game.
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedLine {
    pub number: usize,
    pub text: String,
    pub effect: AutoexecEffect,
}

/// Why a game directory cannot be offered for launch. Every variant names the
/// line and says what to change, so the explanation never needs a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DosBoxReviewProblem {
    /// The folder, `dosbox.conf` or its bytes could not be read as reviewed.
    ConfigUnavailable(String),
    /// A mount reaches outside the game directory, or its path cannot be
    /// shown to stay inside it.
    UnsafeMount { line: usize, reason: String },
    /// A line this review does not model. It fails closed.
    UnsupportedLine { line: usize, command: String },
    /// The start program is not a regular `.exe`, `.com` or `.bat` file inside
    /// the mounted game directory.
    ProgramUnavailable { line: usize, reason: String },
    /// The config names no start command.
    NoStartCommand,
    /// The config names more than one start command.
    AmbiguousStart { lines: Vec<usize> },
    /// Too many `[autoexec]` lines to review.
    TooManyLines,
    /// A setting outside `[autoexec]` that reaches host files or devices
    /// (capture folders, serial or parallel passthrough).
    UnsafeOption { line: usize, key: String },
}

impl DosBoxReviewProblem {
    /// A plain-language sentence for the user.
    pub fn explanation(&self) -> String {
        match self {
            Self::ConfigUnavailable(detail) => {
                format!("The game's dosbox.conf could not be reviewed: {detail}.")
            }
            Self::UnsafeMount { line, reason } => format!(
                "Line {line} of [autoexec] mounts something unsafe: {reason}. DOSBox could then read or change files outside this game folder."
            ),
            Self::UnsupportedLine { line, command } => format!(
                "Line {line} of [autoexec] uses `{command}`, which EmuWiz does not review. It is refused rather than guessed."
            ),
            Self::ProgramUnavailable { line, reason } => format!(
                "Line {line} of [autoexec] names a start program that cannot be used: {reason}."
            ),
            Self::NoStartCommand => {
                "The [autoexec] section does not name a program to start, and EmuWiz will not pick one from the file names.".into()
            }
            Self::AmbiguousStart { lines } => format!(
                "The [autoexec] section starts more than one program (lines {}). EmuWiz will not choose between them.",
                lines
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::UnsafeOption { line, key } => format!(
                "Line {line} sets `{key}`, which would let DOSBox reach host files or devices outside this game folder. EmuWiz refuses it rather than guessing."
            ),
            Self::TooManyLines => format!(
                "The [autoexec] section has more than {MAX_REVIEWED_AUTOEXEC_LINES} lines, which is too long to review."
            ),
        }
    }
}

/// The single start command a clean review found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedStart {
    pub line: usize,
    /// The exact line as written in the config (trimmed).
    pub command_text: String,
    /// The program file it resolves to inside the game directory.
    pub program_file: PathBuf,
    pub program_identity: CapturedFileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxGameReview {
    pub game_directory: PathBuf,
    pub game_directory_identity: CapturedFileIdentity,
    pub config_path: PathBuf,
    /// Lower-case hex SHA-256 of the exact config bytes that were reviewed.
    pub config_sha256: String,
    pub lines: Vec<ReviewedLine>,
    /// Present only when exactly one start line resolved to a real file.
    pub start: Option<ReviewedStart>,
    pub problems: Vec<DosBoxReviewProblem>,
}

impl DosBoxGameReview {
    /// Whether the user may be asked to approve this review.
    pub fn is_approvable(&self) -> bool {
        self.problems.is_empty() && self.start.is_some()
    }

    /// Mints the approval for exactly this review, or `None` when the review
    /// has any problem. The caller must only do this after the user chose to
    /// approve what the review displayed.
    pub fn approve(&self) -> Option<DosBoxStartApproval> {
        if !self.is_approvable() {
            return None;
        }
        let start = self.start.as_ref()?;
        Some(DosBoxStartApproval {
            config_path: self.config_path.clone(),
            config_sha256: self.config_sha256.clone(),
            start_command: start.command_text.clone(),
            start_file: start.program_file.clone(),
            start_file_identity: start.program_identity,
            game_directory_identity: self.game_directory_identity,
        })
    }
}

/// What the user approved. Fields are public to read, but the only
/// constructor is [`DosBoxGameReview::approve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DosBoxStartApproval {
    pub config_path: PathBuf,
    pub config_sha256: String,
    pub start_command: String,
    pub start_file: PathBuf,
    pub start_file_identity: CapturedFileIdentity,
    pub game_directory_identity: CapturedFileIdentity,
}

/// How a fresh review differs from an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMismatch {
    GameDirectory,
    Config,
    StartCommand,
    StartFile,
}

impl ApprovalMismatch {
    pub fn describe(self) -> &'static str {
        match self {
            Self::GameDirectory => "the game folder was replaced since it was approved",
            Self::Config => "dosbox.conf changed since it was approved",
            Self::StartCommand => "the start command changed since it was approved",
            Self::StartFile => "the start program changed since it was approved",
        }
    }
}

impl DosBoxStartApproval {
    /// Compares the approval with a fresh review. `Ok` only when nothing the
    /// user approved has moved.
    pub fn matches(&self, review: &DosBoxGameReview) -> Result<(), ApprovalMismatch> {
        if review.game_directory_identity.device != self.game_directory_identity.device
            || review.game_directory_identity.inode != self.game_directory_identity.inode
        {
            return Err(ApprovalMismatch::GameDirectory);
        }
        if review.config_sha256 != self.config_sha256 || review.config_path != self.config_path {
            return Err(ApprovalMismatch::Config);
        }
        let Some(start) = &review.start else {
            return Err(ApprovalMismatch::StartCommand);
        };
        if start.command_text != self.start_command {
            return Err(ApprovalMismatch::StartCommand);
        }
        if start.program_file != self.start_file
            || start.program_identity != self.start_file_identity
        {
            return Err(ApprovalMismatch::StartFile);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Locating the game directory
// ---------------------------------------------------------------------------

/// The folder that carries a verified `dosbox.conf` for `selected`: the folder
/// itself, or the nearest ancestor (at most [`MAX_GAME_DIRECTORY_ASCENT`] up,
/// never above `stop_at`). Symlinked folders are never returned.
pub fn locate_dosbox_game_directory(selected: &Path, stop_at: Option<&Path>) -> Option<PathBuf> {
    let mut current = if std::fs::symlink_metadata(selected).ok()?.is_dir() {
        selected.to_path_buf()
    } else {
        selected.parent()?.to_path_buf()
    };
    for _ in 0..=MAX_GAME_DIRECTORY_ASCENT {
        let metadata = std::fs::symlink_metadata(&current).ok()?;
        if metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && discover_dosbox_config(&current, &TrustedRoots::none()).is_verified()
        {
            return Some(current);
        }
        if stop_at.is_some_and(|stop| stop == current) {
            return None;
        }
        current = current.parent()?.to_path_buf();
    }
    None
}

// ---------------------------------------------------------------------------
// Review
// ---------------------------------------------------------------------------

const HARMLESS_COMMANDS: &[&str] = &[
    "echo", "rem", "cls", "ver", "set", "path", "prompt", "pause", "dir", "type", "vol", "mem",
    "date", "time",
];

const WRAPPER_COMMANDS: &[&str] = &["call", "loadfix", "lh", "loadhigh"];

const MOUNT_FLAGS_WITH_VALUE: &[&str] = &["-t", "-freesize", "-label", "-size", "-fs"];
const MOUNT_FLAGS_PLAIN: &[&str] = &["-ro", "-nocachedir"];

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_config_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = open_regular_file_nonblocking(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_DOSBOX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_DOSBOX_CONFIG_BYTES {
        return Err("it is larger than a DOSBox config should be".into());
    }
    Ok(bytes)
}

/// The `[autoexec]` lines in file order as `(1-based line number, trimmed
/// text)`. Several `[autoexec]` sections are joined, as DOSBox does.
fn autoexec_lines(text: &str) -> Vec<(usize, String)> {
    let mut in_autoexec = false;
    let mut lines = Vec::new();
    for (index, raw) in text.split('\n').enumerate() {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') {
            let name = trimmed
                .split(']')
                .next()
                .unwrap_or("")
                .trim_start_matches('[')
                .trim()
                .to_ascii_lowercase();
            in_autoexec = name == "autoexec";
            continue;
        }
        if !in_autoexec
            || trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with(';')
        {
            continue;
        }
        lines.push((index + 1, trimmed.to_string()));
    }
    lines
}

/// Settings outside `[autoexec]` that reach the host. Fails closed.
fn option_problems(text: &str) -> Vec<DosBoxReviewProblem> {
    let mut problems = Vec::new();
    let mut in_autoexec = false;
    for (index, raw) in text.split('\n').enumerate() {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') {
            let name = trimmed
                .split(']')
                .next()
                .unwrap_or("")
                .trim_start_matches('[')
                .trim()
                .to_ascii_lowercase();
            in_autoexec = name == "autoexec";
            continue;
        }
        if in_autoexec || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        let first = value.split_whitespace().next().unwrap_or("");
        let unsafe_option = match key.as_str() {
            "captures" | "capture_dir" | "mapperfile" | "language" | "soundfont" => {
                !value.is_empty() && value != "capture" && value != "mapper.map"
            }
            k if k.starts_with("serial") && k[6..].chars().all(|c| c.is_ascii_digit()) => {
                !matches!(first, "" | "disabled" | "dummy")
            }
            k if k.starts_with("parallel") && k[8..].chars().all(|c| c.is_ascii_digit()) => {
                !matches!(first, "" | "disabled")
            }
            _ => false,
        };
        if unsafe_option {
            problems.push(DosBoxReviewProblem::UnsafeOption {
                line: index + 1,
                key,
            });
        }
    }
    problems
}

/// Splits on whitespace, keeping double-quoted runs together (quotes removed).
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut started = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(current);
    }
    tokens
}

struct ReviewState {
    game_directory: PathBuf,
    canonical_game_directory: PathBuf,
    mounts: HashMap<char, PathBuf>,
    drive: Option<char>,
    cwd: HashMap<char, Vec<String>>,
}

fn drive_letter(token: &str) -> Option<char> {
    let mut chars = token.chars();
    let letter = chars.next()?;
    (letter.is_ascii_alphabetic() && (chars.next().is_none())).then(|| letter.to_ascii_uppercase())
}

impl ReviewState {
    /// A host path that must stay inside the game directory.
    fn confine(&self, token: &str, want_dir: bool) -> Result<PathBuf, String> {
        if token.is_empty() || token.contains('\0') {
            return Err("the path is empty".into());
        }
        if token.starts_with('~') {
            return Err("it points into a home folder".into());
        }
        if token.contains('\\') {
            return Err("it uses a backslash, which a Linux host reads as part of a name".into());
        }
        let candidate = Path::new(token);
        if candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err("it climbs out with `..`".into());
        }
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.game_directory.join(candidate)
        };
        let resolved = std::fs::canonicalize(&joined)
            .map_err(|_| "it does not exist inside the game folder".to_string())?;
        if !resolved.starts_with(&self.canonical_game_directory) {
            return Err("it resolves outside the game folder".into());
        }
        let metadata =
            std::fs::metadata(&resolved).map_err(|_| "it cannot be inspected".to_string())?;
        if want_dir && !metadata.is_dir() {
            return Err("it is not a folder".into());
        }
        if !want_dir && !metadata.is_file() {
            return Err("it is not a file".into());
        }
        Ok(resolved)
    }

    fn current_root(&self) -> Option<(char, &PathBuf)> {
        let drive = self.drive?;
        self.mounts.get(&drive).map(|root| (drive, root))
    }

    /// Resolves a DOS program token to a real file under a mounted drive.
    fn resolve_program(&self, token: &str) -> Result<PathBuf, String> {
        let mut rest = token.trim();
        let mut drive = self.drive;
        let mut absolute = false;
        if let Some((head, tail)) = rest.split_once(':')
            && let Some(letter) = drive_letter(head)
        {
            drive = Some(letter);
            rest = tail;
        }
        if rest.starts_with('\\') || rest.starts_with('/') {
            absolute = true;
        }
        let drive = drive.ok_or_else(|| "no drive is selected".to_string())?;
        let root = self
            .mounts
            .get(&drive)
            .ok_or_else(|| format!("drive {drive}: is not mounted from the game folder"))?;
        let mut components: Vec<String> = if absolute {
            Vec::new()
        } else {
            self.cwd.get(&drive).cloned().unwrap_or_default()
        };
        let parts: Vec<&str> = rest
            .split(['\\', '/'])
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            return Err("the program name is empty".into());
        }
        for part in &parts {
            if *part == ".." || *part == "." || part.contains('\0') {
                return Err("the program path uses `.` or `..`".into());
            }
            components.push((*part).to_string());
        }
        let (file_name, directories) = components
            .split_last()
            .ok_or_else(|| "the program name is empty".to_string())?;
        let mut directory = root.clone();
        for name in directories {
            directory = find_entry(&directory, name, true)?;
        }
        let candidates: Vec<String> = match file_name.rsplit_once('.') {
            Some((_, extension))
                if ["exe", "com", "bat"].contains(&extension.to_ascii_lowercase().as_str()) =>
            {
                vec![file_name.clone()]
            }
            Some(_) => {
                return Err("only .exe, .com and .bat programs can be started".into());
            }
            None => ["com", "exe", "bat"]
                .iter()
                .map(|extension| format!("{file_name}.{extension}"))
                .collect(),
        };
        for candidate in candidates {
            if let Ok(found) = find_entry(&directory, &candidate, false) {
                return Ok(found);
            }
        }
        Err(format!("`{file_name}` is not in the mounted game folder"))
    }
}

/// Case-insensitive lookup of one name in `directory`. Symlinks are refused.
fn find_entry(directory: &Path, name: &str, want_dir: bool) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(directory).map_err(|error| error.to_string())?;
    for entry in entries.filter_map(Result::ok).take(8192) {
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
        {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if metadata.file_type().is_symlink() {
                return Err(format!("`{name}` is a symlink"));
            }
            if want_dir && !metadata.is_dir() {
                return Err(format!("`{name}` is not a folder"));
            }
            if !want_dir && !metadata.is_file() {
                return Err(format!("`{name}` is not a regular file"));
            }
            return Ok(path);
        }
    }
    Err(format!("`{name}` is not in the mounted game folder"))
}

fn review_line(
    number: usize,
    text: &str,
    state: &mut ReviewState,
    problems: &mut Vec<DosBoxReviewProblem>,
    starts: &mut Vec<(usize, String, Result<PathBuf, String>)>,
) -> AutoexecEffect {
    let tokens = tokenize(text.trim_start_matches('@'));
    let Some(first) = tokens.first() else {
        return AutoexecEffect::Harmless;
    };
    let lower = first.to_ascii_lowercase();
    let unsupported = |problems: &mut Vec<DosBoxReviewProblem>, command: &str| {
        problems.push(DosBoxReviewProblem::UnsupportedLine {
            line: number,
            command: command.chars().take(40).collect(),
        });
        AutoexecEffect::Harmless
    };

    if lower.starts_with("::") {
        return AutoexecEffect::Harmless;
    }
    if let Some(letter) = drive_letter(lower.trim_end_matches(':'))
        && lower.ends_with(':')
        && tokens.len() == 1
    {
        if !state.mounts.contains_key(&letter) {
            problems.push(DosBoxReviewProblem::ProgramUnavailable {
                line: number,
                reason: format!("drive {letter}: is used before it is mounted"),
            });
        }
        state.drive = Some(letter);
        return AutoexecEffect::SwitchDrive(letter);
    }
    if HARMLESS_COMMANDS.contains(&lower.as_str()) {
        return AutoexecEffect::Harmless;
    }
    if lower == "exit" {
        return AutoexecEffect::Exit;
    }
    if matches!(lower.as_str(), "cd" | "chdir" | "cd.." | "cd\\") {
        let Some((drive, _)) = state.current_root() else {
            problems.push(DosBoxReviewProblem::ProgramUnavailable {
                line: number,
                reason: "cd is used before a drive is mounted and selected".into(),
            });
            return AutoexecEffect::ChangeDirectory;
        };
        let entry = state.cwd.entry(drive).or_default();
        match lower.as_str() {
            "cd.." => {
                entry.pop();
            }
            "cd\\" => entry.clear(),
            _ => {
                let Some(target) = tokens.get(1) else {
                    return AutoexecEffect::ChangeDirectory;
                };
                for part in target.split(['\\', '/']).filter(|part| !part.is_empty()) {
                    match part {
                        "." => {}
                        ".." => {
                            entry.pop();
                        }
                        name => entry.push(name.to_string()),
                    }
                }
            }
        }
        return AutoexecEffect::ChangeDirectory;
    }
    if lower == "mount" || lower == "imgmount" {
        return review_mount(number, &lower, &tokens, state, problems);
    }
    if WRAPPER_COMMANDS.contains(&lower.as_str()) {
        let Some(program) = tokens.get(1) else {
            return unsupported(problems, &lower);
        };
        starts.push((number, text.to_string(), state.resolve_program(program)));
        return AutoexecEffect::Start;
    }
    // Any other DOSBox built-in is refused rather than guessed. A name that is
    // not a built-in is read as a program to start; if it names nothing real
    // in the mounted folder the review reports the missing program.
    const BUILT_INS: &[&str] = &[
        "config", "boot", "intro", "mixer", "keyb", "country", "ipx", "ipxnet", "serial", "subst",
        "append", "choice", "copy", "del", "erase", "ren", "rename", "md", "mkdir", "rd", "rmdir",
        "move", "attrib", "tree", "xcopy", "format", "fdisk", "if", "goto", "for", "shift",
        "title", "mode", "loadrom", "rescan", "imgmake", "overlay", "z:",
    ];
    if BUILT_INS.contains(&lower.as_str()) || lower.ends_with(':') || lower.starts_with(':') {
        return unsupported(problems, &lower);
    }
    starts.push((number, text.to_string(), state.resolve_program(first)));
    AutoexecEffect::Start
}

fn review_mount(
    number: usize,
    command: &str,
    tokens: &[String],
    state: &mut ReviewState,
    problems: &mut Vec<DosBoxReviewProblem>,
) -> AutoexecEffect {
    let mut bad = |reason: String| {
        problems.push(DosBoxReviewProblem::UnsafeMount {
            line: number,
            reason,
        });
        AutoexecEffect::Harmless
    };
    let unsupported = |problems: &mut Vec<DosBoxReviewProblem>| {
        problems.push(DosBoxReviewProblem::UnsupportedLine {
            line: number,
            command: command.to_string(),
        });
        AutoexecEffect::Harmless
    };
    if tokens.get(1).map(String::as_str) == Some("-u") {
        if let Some(letter) = tokens.get(2).and_then(|token| drive_letter(token)) {
            state.mounts.remove(&letter);
            if state.drive == Some(letter) {
                state.drive = None;
            }
        }
        return AutoexecEffect::Harmless;
    }
    let Some(letter) = tokens.get(1).and_then(|token| drive_letter(token)) else {
        return unsupported(problems);
    };
    let is_image = command == "imgmount";
    // Host paths run from tokens[2] up to the first flag.
    let rest = &tokens[2..];
    let path_count = rest.iter().take_while(|t| !t.starts_with('-')).count();
    if path_count == 0 || (!is_image && path_count != 1) {
        return unsupported(problems);
    }
    // Flags: only a small, understood set.
    let mut index = path_count;
    let mut is_cdrom_or_image = is_image;
    while index < rest.len() {
        let flag = rest[index].to_ascii_lowercase();
        if MOUNT_FLAGS_PLAIN.contains(&flag.as_str()) {
            index += 1;
        } else if MOUNT_FLAGS_WITH_VALUE.contains(&flag.as_str()) && index + 1 < rest.len() {
            if flag == "-t" {
                let kind = rest[index + 1].to_ascii_lowercase();
                if !["dir", "cdrom", "floppy", "hdd", "iso", "floppy"].contains(&kind.as_str()) {
                    return unsupported(problems);
                }
                is_cdrom_or_image |= kind == "cdrom";
            }
            index += 2;
        } else {
            // Includes -usecd, -ioctl, -aspi: host device access.
            return bad(format!("the flag `{}` is not reviewed", rest[index]));
        }
    }
    let mut first_host: Option<PathBuf> = None;
    for token in &rest[..path_count] {
        // A plain folder mount may be a folder; an image or CD mount a file
        // or folder (CD folders are allowed).
        let resolved = if is_image {
            state.confine(token, false)
        } else if is_cdrom_or_image {
            state.confine(token, true)
        } else {
            state.confine(token, true)
        };
        match resolved {
            Ok(path) => {
                first_host.get_or_insert(path);
            }
            Err(reason) => return bad(reason),
        }
    }
    if let Some(host) = first_host {
        // Only folder mounts let a start program be found. Image mounts are
        // allowed but a drive that holds an image has no host folder root.
        if !is_image {
            state.mounts.insert(letter, host);
            state.cwd.remove(&letter);
        } else {
            state.mounts.remove(&letter);
        }
    }
    AutoexecEffect::Mount { drive: letter }
}

/// Reviews the DOS game folder at `game_directory`. Read-only; nothing in the
/// folder is executed or changed.
pub fn review_dosbox_game_directory(
    game_directory: &Path,
) -> Result<DosBoxGameReview, DosBoxReviewProblem> {
    let unavailable = |detail: &str| DosBoxReviewProblem::ConfigUnavailable(detail.to_string());
    let metadata = std::fs::symlink_metadata(game_directory)
        .map_err(|error| unavailable(&format!("the game folder is unavailable: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(unavailable("the game folder is not a real folder"));
    }
    let game_directory_identity = CapturedFileIdentity::capture(&metadata);
    let canonical_game_directory = std::fs::canonicalize(game_directory)
        .map_err(|error| unavailable(&format!("the game folder cannot be resolved: {error}")))?;

    let config_path = match discover_dosbox_config(game_directory, &TrustedRoots::none()) {
        DosBoxConfigStatus::Verified { config_path, .. } => config_path,
        DosBoxConfigStatus::Missing => return Err(unavailable("there is no dosbox.conf")),
        DosBoxConfigStatus::Malformed(detail) => {
            return Err(unavailable(&format!("dosbox.conf is malformed: {detail}")));
        }
        DosBoxConfigStatus::ValidNoAutoexec => {
            return Err(unavailable("dosbox.conf has no [autoexec] section"));
        }
    };
    let bytes = read_config_bytes(&config_path)
        .map_err(|detail| unavailable(&format!("dosbox.conf cannot be read: {detail}")))?;
    let config_sha256 = hex_lower(&Sha256::digest(&bytes));
    let text = std::str::from_utf8(&bytes).map_err(|_| unavailable("dosbox.conf is not text"))?;

    let raw_lines = autoexec_lines(text);
    let mut problems = option_problems(text);
    if raw_lines.len() > MAX_REVIEWED_AUTOEXEC_LINES {
        problems.push(DosBoxReviewProblem::TooManyLines);
    }
    let mut state = ReviewState {
        game_directory: game_directory.to_path_buf(),
        canonical_game_directory,
        mounts: HashMap::new(),
        drive: None,
        cwd: HashMap::new(),
    };
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    for (number, text) in raw_lines.into_iter().take(MAX_REVIEWED_AUTOEXEC_LINES) {
        let effect = review_line(number, &text, &mut state, &mut problems, &mut starts);
        lines.push(ReviewedLine {
            number,
            text,
            effect,
        });
    }

    let mut start = None;
    match starts.len() {
        0 => problems.push(DosBoxReviewProblem::NoStartCommand),
        1 => {
            let (number, text, resolved) = starts.remove(0);
            match resolved {
                Ok(program_file) => match std::fs::symlink_metadata(&program_file) {
                    Ok(program_metadata) => {
                        start = Some(ReviewedStart {
                            line: number,
                            command_text: text,
                            program_file,
                            program_identity: CapturedFileIdentity::capture(&program_metadata),
                        });
                    }
                    Err(error) => problems.push(DosBoxReviewProblem::ProgramUnavailable {
                        line: number,
                        reason: error.to_string(),
                    }),
                },
                Err(reason) => problems.push(DosBoxReviewProblem::ProgramUnavailable {
                    line: number,
                    reason,
                }),
            }
        }
        _ => problems.push(DosBoxReviewProblem::AmbiguousStart {
            lines: starts.iter().map(|(number, _, _)| *number).collect(),
        }),
    }
    if !problems.is_empty() {
        start = None;
    }

    Ok(DosBoxGameReview {
        game_directory: game_directory.to_path_buf(),
        game_directory_identity,
        config_path,
        config_sha256,
        lines,
        start,
        problems,
    })
}

#[cfg(test)]
mod tests;
