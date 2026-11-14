//! The diagnostic evidence model.
//!
//! An [`Evidence`] item is one observation: what was observed ([`Fact`]),
//! how far it can be trusted ([`Trust`]), whether it is still current
//! ([`EvidenceState`], [`SourceFingerprint`]) and where it came from. An
//! [`EvidenceSet`] is the complete input to the diagnostic engine; it also
//! reports which areas were *not* observed, so that missing evidence is as
//! visible as present evidence.
//!
//! Collection lives with whoever owns the source (the launch executor owns
//! the exit report, the readiness planner owns blockers). This module only
//! defines the vocabulary and the few read-only filesystem observations
//! that have no other owner.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::text;
use crate::launch::process_spawn::{
    CapturedFileIdentity, capture_file_identity, open_regular_file_nonblocking,
};

/// Version of the serialised evidence and report shape.
pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Upper bound on evidence items in one set. A collector that would exceed
/// it stops adding items and the set records that it was truncated; every
/// report made from it then carries a note saying so.
pub const MAX_EVIDENCE_ITEMS: usize = 2_000;

/// Identifies one evidence item inside one [`EvidenceSet`]. Assigned by the
/// set in insertion order, so the same collection always yields the same
/// ids.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EvidenceId(String);

impl EvidenceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EvidenceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What an evidence item is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceArea {
    GameIdentity,
    MediaIntegrity,
    EmulatorExecutable,
    Firmware,
    Graphics,
    EmulatorConfiguration,
    LaunchEnvironment,
    LaunchReadiness,
    ProcessExit,
    LogEvents,
    Filesystem,
    Modifications,
    KnownLimitations,
}

impl EvidenceArea {
    /// Every area, in the order a coverage report lists them.
    pub const ALL: [Self; 13] = [
        Self::GameIdentity,
        Self::MediaIntegrity,
        Self::EmulatorExecutable,
        Self::Firmware,
        Self::Graphics,
        Self::EmulatorConfiguration,
        Self::LaunchEnvironment,
        Self::LaunchReadiness,
        Self::ProcessExit,
        Self::LogEvents,
        Self::Filesystem,
        Self::Modifications,
        Self::KnownLimitations,
    ];

    /// Stable short name, used in evidence ids and in JSON.
    pub fn tag(self) -> &'static str {
        match self {
            Self::GameIdentity => "game_identity",
            Self::MediaIntegrity => "media_integrity",
            Self::EmulatorExecutable => "emulator_executable",
            Self::Firmware => "firmware",
            Self::Graphics => "graphics",
            Self::EmulatorConfiguration => "emulator_configuration",
            Self::LaunchEnvironment => "launch_environment",
            Self::LaunchReadiness => "launch_readiness",
            Self::ProcessExit => "process_exit",
            Self::LogEvents => "log_events",
            Self::Filesystem => "filesystem",
            Self::Modifications => "modifications",
            Self::KnownLimitations => "known_limitations",
        }
    }

    /// Plain-language name for people.
    pub fn label(self) -> &'static str {
        match self {
            Self::GameIdentity => "Game identity",
            Self::MediaIntegrity => "Game file integrity",
            Self::EmulatorExecutable => "Emulator program and version",
            Self::Firmware => "Firmware and BIOS",
            Self::Graphics => "Graphics card, driver and graphics API",
            Self::EmulatorConfiguration => "Emulator settings",
            Self::LaunchEnvironment => "Launch environment",
            Self::LaunchReadiness => "Checks made before launch",
            Self::ProcessExit => "How the emulator process ended",
            Self::LogEvents => "Emulator log",
            Self::Filesystem => "Files and permissions",
            Self::Modifications => "Patches, cheats and mods",
            Self::KnownLimitations => "Known compatibility limits",
        }
    }
}

/// How far an observation can be relied on. Ordered from most to least
/// reliable; the engine uses it to cap the confidence of a conclusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// EmuWiz measured it directly from the operating system or from a file
    /// it read itself: an exit status, a file's presence, a setting's value.
    Measured,
    /// A host tool or the emulator reported it through a structured,
    /// well-defined channel that content cannot write to.
    Reported,
    /// Free text that an emulator, a game or a third party can influence:
    /// log lines, stderr. Matched against fixed signatures only.
    UntrustedText,
    /// A person or an AI layer said so. Never counted as support.
    Asserted,
}

impl Trust {
    /// Whether evidence at this trust level can support a conclusion above
    /// [`Possible`](super::rules::Confidence::Possible) on its own account.
    pub fn is_firm(self) -> bool {
        matches!(self, Self::Measured | Self::Reported)
    }
}

/// Whether the observation was made and is still current.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EvidenceState {
    /// Observed, and not known to have changed since.
    Observed,
    /// Collection was attempted and could not produce an observation.
    Unavailable { reason: String },
    /// Nothing tried to observe this.
    NotCollected { reason: String },
    /// Observed earlier, but the source has changed since.
    Stale { reason: String },
}

impl EvidenceState {
    pub fn is_observed(&self) -> bool {
        matches!(self, Self::Observed)
    }
}

/// Seconds and nanoseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    pub seconds: u64,
    pub nanoseconds: u32,
}

impl Timestamp {
    fn from_system_time(time: Option<SystemTime>) -> Option<Self> {
        let elapsed = time?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            seconds: elapsed.as_secs(),
            nanoseconds: elapsed.subsec_nanos(),
        })
    }
}

/// Largest file [`SourceFingerprint::capture_with_digest`] hashes. Settings
/// files are small; anything larger is fingerprinted by identity only.
pub const FINGERPRINT_DIGEST_LIMIT: u64 = 4 * 1024 * 1024;

/// The state of a source file when evidence was read from it, so that a
/// later change can be detected instead of silently trusted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    /// The file's path, sanitised for display and export.
    pub path: String,
    /// The exact path the file was read from, used to look at it again. It
    /// is never serialised: a report restored from JSON cannot be compared
    /// with the file, and says so rather than opening whatever its text
    /// names.
    #[serde(skip)]
    real_path: std::path::PathBuf,
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified: Option<Timestamp>,
    pub changed: Option<Timestamp>,
    /// Lower-case hex SHA-256 of the content, when it was hashed.
    pub sha256: Option<String>,
}

/// Result of comparing a fingerprint with the file as it is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FingerprintCheck {
    Unchanged,
    Changed {
        detail: String,
    },
    /// The file can no longer be inspected (removed, replaced by a link,
    /// unreadable). Treated as changed.
    Unavailable {
        detail: String,
    },
}

impl SourceFingerprint {
    fn from_identity(path: &Path, identity: CapturedFileIdentity) -> Self {
        Self {
            path: text::sanitise(&path.display().to_string(), text::PATH_LIMIT),
            real_path: path.to_path_buf(),
            device: identity.device,
            inode: identity.inode,
            size: identity.size,
            modified: Timestamp::from_system_time(identity.modified),
            changed: Timestamp::from_system_time(identity.changed),
            sha256: None,
        }
    }

    /// Identity of a regular file, without reading it. A symbolic link or
    /// anything that is not a regular file is refused: evidence is only
    /// taken from the file actually named.
    pub fn capture(path: &Path) -> std::io::Result<Self> {
        let (metadata, identity) = capture_file_identity(path)?;
        if !metadata.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(Self::from_identity(path, identity))
    }

    /// Read a regular file once, without following a final symbolic link,
    /// and return its content together with a fingerprint that includes the
    /// SHA-256 of exactly those bytes. Refuses files over `limit` bytes
    /// rather than fingerprinting a prefix.
    pub fn read_with_digest(path: &Path, limit: u64) -> std::io::Result<(Vec<u8>, Self)> {
        let limit = limit.min(FINGERPRINT_DIGEST_LIMIT);
        let file = open_regular_file_nonblocking(path)?;
        let metadata = file.metadata()?;
        if metadata.len() > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "file is larger than the evidence read limit",
            ));
        }
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "file grew past the evidence read limit while it was read",
            ));
        }
        let mut fingerprint = Self::from_identity(path, CapturedFileIdentity::capture(&metadata));
        // The identity above is of the opened file; the size recorded is the
        // number of bytes actually hashed, so a file that changed while it
        // was being read does not produce a self-consistent fingerprint.
        fingerprint.size = bytes.len() as u64;
        fingerprint.sha256 = Some(hex(&Sha256::digest(&bytes)));
        Ok((bytes, fingerprint))
    }

    /// Compare with the file as it is now. When the fingerprint carries a
    /// digest the content is hashed again; otherwise identity, size and
    /// timestamps decide.
    pub fn check(&self) -> FingerprintCheck {
        if self.real_path.as_os_str().is_empty() {
            return FingerprintCheck::Unavailable {
                detail: format!(
                    "{}: this report was restored and can no longer be compared with the file",
                    self.path
                ),
            };
        }
        let path = self.real_path.as_path();
        let current = if self.sha256.is_some() {
            Self::read_with_digest(path, FINGERPRINT_DIGEST_LIMIT).map(|(_, current)| current)
        } else {
            Self::capture(path)
        };
        let current = match current {
            Ok(current) => current,
            Err(error) => {
                return FingerprintCheck::Unavailable {
                    detail: format!("{}: {}", self.path, error.kind()),
                };
            }
        };
        let changed = if self.sha256.is_some() {
            (current.sha256 != self.sha256).then_some("content")
        } else if (current.device, current.inode) != (self.device, self.inode) {
            Some("file identity")
        } else if current.size != self.size {
            Some("size")
        } else if current.modified != self.modified || current.changed != self.changed {
            Some("timestamps")
        } else {
            None
        };
        match changed {
            None => FingerprintCheck::Unchanged,
            Some(what) => FingerprintCheck::Changed {
                detail: format!("{}: {what} changed since it was read", self.path),
            },
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaIntegrityStatus {
    /// Content matched a trusted hash record.
    Verified,
    /// Present and structurally readable; no trusted hash to compare with.
    Unverified,
    /// A check found the content damaged, incomplete or of the wrong kind.
    Failed,
}

/// Mirrors [`crate::launch::FirmwareReadiness`] so it can be serialised in a
/// report; [`From`] keeps the two in step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirmwareStatus {
    Verified,
    PresentUnverified,
    Missing,
    Unknown,
    NotRequired,
}

impl From<crate::launch::FirmwareReadiness> for FirmwareStatus {
    fn from(readiness: crate::launch::FirmwareReadiness) -> Self {
        use crate::launch::FirmwareReadiness as R;
        match readiness {
            R::Verified => Self::Verified,
            R::PresentUnverified => Self::PresentUnverified,
            R::Missing => Self::Missing,
            R::Unknown => Self::Unknown,
            R::NotRequired => Self::NotRequired,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicsApi {
    Vulkan,
    OpenGl,
}

impl GraphicsApi {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vulkan => "Vulkan",
            Self::OpenGl => "OpenGL",
        }
    }
}

/// Where a graphics probe ran. A probe of the desktop session says nothing
/// certain about a sandboxed launch, so the two are never merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeScope {
    /// The same environment the emulator was (or will be) started in.
    LaunchEnvironment,
    /// The host in general, outside the launch environment.
    Host,
}

/// How a process ended, or that it did not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ExitOutcome {
    /// The program could not be started at all.
    SpawnFailed {
        reason: SpawnFailure,
        os_error: Option<i32>,
    },
    Exited {
        code: i32,
    },
    /// Ended by a signal delivered to the watched process itself.
    Signaled {
        signal: i32,
        core_dumped: bool,
    },
    /// Still running when observation stopped. Not a failure, and not
    /// evidence that a game loaded.
    StillRunning,
    /// The exit status could not be read.
    StatusUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnFailure {
    ProgramNotFound,
    PermissionDenied,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessExit {
    #[serde(flatten)]
    pub outcome: ExitOutcome,
    /// Time from start to exit (or to the end of observation).
    pub duration_ms: Option<u64>,
    /// Set when the watched process is a launcher (for example
    /// `flatpak run` or a wrapper script) rather than the emulator: the
    /// status is then the launcher's account of what happened.
    pub launcher: Option<String>,
}

impl ProcessExit {
    /// Whether this is, on its face, a failed run. `StillRunning`, a zero
    /// exit code and an ordinary stop request are not failures; none of
    /// them says a game loaded.
    pub fn is_failure(&self) -> bool {
        match self.outcome {
            ExitOutcome::SpawnFailed { .. } => true,
            ExitOutcome::Signaled { signal, .. } => !matches!(
                super::process::signal_info(signal).map(|info| info.kind),
                Some(super::process::SignalKind::StopRequest)
            ),
            // A launcher reports a program that was asked to stop as exit
            // status 128 + the signal number. That is the same stop request,
            // one step removed.
            ExitOutcome::Exited { code } => {
                code != 0
                    && !super::process::possibly_relayed_signal(code).is_some_and(|signal| {
                        matches!(
                            super::process::signal_info(signal).map(|info| info.kind),
                            Some(super::process::SignalKind::StopRequest)
                        )
                    })
            }
            ExitOutcome::StillRunning | ExitOutcome::StatusUnavailable => false,
        }
    }
}

/// What a matched log signature means, independent of the emulator that
/// wrote it. Rules reason over these, never over raw text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogMeaning {
    /// The Vulkan loader could not create an instance or context.
    VulkanUnavailable,
    /// The emulator could not open its video driver and stopped.
    VideoDriverInitFailed,
    /// A video driver finished initialising; `value` is its name.
    VideoDriverReady,
    /// A Vulkan device was selected; `value` is its name.
    VulkanDeviceSelected,
    /// An OpenGL context reported its renderer; `value` is the report.
    OpenGlRendererReported,
    /// Loading of an emulator core (plug-in) began; `value` is its path.
    CoreLoadStarted,
    /// The requested core was ignored and the emulator carried on without it.
    CoreRequestIgnored,
    /// The core file could not be opened as a library.
    CoreOpenFailed,
    /// The core described its video and audio output; `value` is the audio
    /// sample rate in Hz.
    CoreReportedOutput,
    /// Loading of a game file began; `value` is its path.
    ContentLoadStarted,
    /// The emulator could not read the game file.
    ContentReadFailed,
    /// The core read the game file and refused it.
    ContentRejected,
    /// The emulator's own account of how long content ran; `value` is whole
    /// seconds.
    ContentRanFor,
    /// The audio driver failed to start; the emulator continued silently.
    AudioDriverInitFailed,
    /// Writing save data failed.
    SaveWriteFailed,
    /// The dynamic loader could not find a shared library; `value` is its
    /// name.
    SharedLibraryMissing,
    /// A sandbox or application runtime refused to start the program.
    SandboxRefused,
    /// A fatal error with no usable detail about its cause.
    FatalErrorUnspecific,
}

impl LogMeaning {
    /// Generic messages say that something failed without saying what. They
    /// are recorded, shown, and never accepted as the basis of a cause.
    pub fn is_specific(self) -> bool {
        !matches!(self, Self::FatalErrorUnspecific)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    /// The emulator's own log file.
    LogFile,
    /// Standard error captured from the launched process.
    Stderr,
}

/// One log line that matched a fixed signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEvent {
    pub emulator: String,
    /// Identifier and version of the signature that matched.
    pub signature: String,
    pub signature_version: u32,
    pub meaning: LogMeaning,
    pub stream: LogStream,
    /// One-based line number in the examined text.
    pub line: u32,
    /// The matched line, sanitised and shortened. For display only.
    pub excerpt: String,
    /// Sanitised value the signature extracts, where it defines one.
    pub value: Option<String>,
}

/// How much log text was examined, so that "no error in the log" is never
/// confused with "the log was not read" or "the log was cut short".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogCoverage {
    pub emulator: String,
    pub stream: LogStream,
    pub lines_examined: u32,
    pub bytes_examined: u64,
    /// Text beyond the examination limits was not read.
    pub truncated: bool,
    /// Over-long lines that were skipped rather than matched on a prefix.
    pub lines_ignored: u32,
    /// The emulator was logging at its informational level. When false,
    /// only a few fatal messages are printed, so the absence of an event
    /// says nothing about whether it happened.
    pub verbose: bool,
}

/// The launch planner's own grouping of a blocker, carried over from
/// [`crate::ready_to_play::ReadinessReasonFamily`] by an exhaustive match so
/// that a new family there cannot be silently misfiled here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerFamily {
    Identity,
    Media,
    Firmware,
    Emulator,
    Configuration,
    Dependency,
    Modification,
    Controller,
    Unsupported,
    Other,
}

impl From<crate::ready_to_play::ReadinessReasonFamily> for BlockerFamily {
    fn from(family: crate::ready_to_play::ReadinessReasonFamily) -> Self {
        use crate::ready_to_play::ReadinessReasonFamily as F;
        match family {
            F::Identity => Self::Identity,
            F::Content | F::MediaTopology | F::Arcade | F::DatCompatibility => Self::Media,
            F::Firmware => Self::Firmware,
            F::Emulator => Self::Emulator,
            F::Configuration => Self::Configuration,
            F::Dependency => Self::Dependency,
            F::ModOrPatch => Self::Modification,
            F::Controller => Self::Controller,
            F::Unsupported => Self::Unsupported,
            F::LaunchPlan | F::UnknownEvidence => Self::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathRole {
    GameFile,
    EmulatorCore,
    EmulatorExecutable,
    ConfigurationFile,
    SaveDirectory,
    FirmwareFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPresence {
    RegularFile,
    Directory,
    SymbolicLink,
    Missing,
    /// Something else: a device, a socket, a pipe.
    Other,
    /// The path could not be examined (for example a folder on the way to
    /// it denies access), so what is there is not known.
    Unknown,
}

impl PathPresence {
    /// Completes the sentence "the path ...".
    pub fn describe(self) -> &'static str {
        match self {
            Self::RegularFile => "is a file",
            Self::Directory => "is a folder",
            Self::SymbolicLink => "is a symbolic link, which was not followed",
            Self::Missing => "does not exist",
            Self::Other => "is not a file or a folder",
            Self::Unknown => "could not be examined",
        }
    }
}

/// A read-only look at one path: what is there, and whether this user may
/// read or write it. Nothing is opened, created or changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathAccess {
    pub role: PathRole,
    pub path: String,
    pub presence: PathPresence,
    pub readable: Option<bool>,
    pub writable: Option<bool>,
}

/// Which settings file a value was read from. Later layers override
/// earlier ones; [`ConfigLayer::precedence`] gives the order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigLayer {
    /// The emulator's main settings file. Applies to every game.
    Main,
    /// A file layered over the main settings for one launch only.
    Session,
    /// An override the emulator applies to every game using one core.
    Core,
    /// An override the emulator applies to every game in one folder.
    ContentDirectory,
    /// An override the emulator applies to one game.
    Game,
}

impl ConfigLayer {
    /// Higher wins. The order is RetroArch's: appended files load with the
    /// main settings, then core, folder and game overrides apply in turn.
    pub fn precedence(self) -> u8 {
        match self {
            Self::Main => 0,
            Self::Session => 1,
            Self::Core => 2,
            Self::ContentDirectory => 3,
            Self::Game => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "main settings (all games)",
            Self::Session => "this launch only",
            Self::Core => "override for this core",
            Self::ContentDirectory => "override for this folder",
            Self::Game => "override for this game",
        }
    }
}

/// One observed fact. Each variant is deliberately narrow: a rule can only
/// use what a collector actually established.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fact {
    /// No value. Used with [`EvidenceState::Unavailable`] and
    /// [`EvidenceState::NotCollected`].
    Absent,
    GameIdentity {
        /// True only when a trusted authority confirmed the identity. A
        /// filename or folder hint is never `verified`.
        verified: bool,
        authority: String,
        platform: Option<String>,
        title: Option<String>,
    },
    MediaIntegrity {
        status: MediaIntegrityStatus,
        detail: String,
    },
    EmulatorExecutable {
        emulator: String,
        path: String,
        installation: Option<String>,
        version: Option<String>,
        version_source: Option<String>,
    },
    Firmware {
        requirement: String,
        status: FirmwareStatus,
    },
    GraphicsApi {
        api: GraphicsApi,
        scope: ProbeScope,
        usable: bool,
        devices: Vec<String>,
        detail: String,
    },
    /// One setting as written in one settings file. `value` is `None` when
    /// the key is not set in that file; the emulator then uses a value from
    /// a lower layer or its built-in default, which this does not report.
    ConfigValue {
        emulator: String,
        layer: ConfigLayer,
        key: String,
        value: Option<String>,
        line: Option<u32>,
    },
    /// No settings file exists for this layer, so it overrides nothing.
    ConfigLayerAbsent {
        emulator: String,
        layer: ConfigLayer,
    },
    /// Features of a settings file that limit how far its values can be
    /// taken as the settings the emulator will actually use.
    ConfigStructure {
        emulator: String,
        layer: ConfigLayer,
        /// `#include` directives. Included files are not read.
        includes: u32,
        malformed_lines: u32,
        /// Keys set more than once in this file.
        repeated_keys: Vec<String>,
    },
    EnvironmentVariable {
        name: String,
        value: Option<String>,
    },
    ProcessExit(ProcessExit),
    LogEvent(LogEvent),
    LogCoverage(LogCoverage),
    /// A blocker the launch planner raised before any process was started.
    /// `blocker` is the planner's own name for it and `family` its own
    /// classification ([`crate::ready_to_play::ReadinessReasonFamily`]).
    ReadinessBlocker {
        blocker: String,
        family: BlockerFamily,
        detail: String,
    },
    /// Whether an emulator's settings bind a controller to a player.
    ControllerBinding {
        emulator: String,
        player: u8,
        bound: bool,
    },
    PathAccess(PathAccess),
    /// Two active modifications that EmuWiz manages both change the same
    /// target. Established by the patch manager, not inferred here.
    ModificationConflict {
        target: String,
        first: String,
        second: String,
    },
    /// An entry of the built-in compatibility table that applies to this
    /// emulator. `violated` is true when the observed version is outside
    /// the supported range.
    KnownLimitation {
        id: String,
        summary: String,
        violated: bool,
    },
}

/// One observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: EvidenceId,
    pub area: EvidenceArea,
    #[serde(flatten)]
    pub state: EvidenceState,
    pub trust: Trust,
    /// Where the observation came from, in plain words.
    pub source: String,
    pub fingerprint: Option<SourceFingerprint>,
    pub fact: Fact,
}

impl Evidence {
    pub fn is_observed(&self) -> bool {
        self.state.is_observed()
    }

    /// One plain-language line describing the observation, safe to display.
    pub fn summary(&self) -> String {
        let described = match &self.state {
            EvidenceState::Unavailable { reason } => {
                format!("{}: unavailable ({reason})", self.source)
            }
            EvidenceState::NotCollected { reason } => format!("not collected ({reason})"),
            EvidenceState::Stale { reason } => {
                format!("{} [out of date: {reason}]", self.fact.describe())
            }
            EvidenceState::Observed => self.fact.describe(),
        };
        text::sanitise(&described, text::SUMMARY_LIMIT)
    }
}

impl Trust {
    pub fn label(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Reported => "reported",
            Self::UntrustedText => "emulator output",
            Self::Asserted => "asserted, not observed",
        }
    }
}

impl PathRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::GameFile => "game file",
            Self::EmulatorCore => "emulator core",
            Self::EmulatorExecutable => "emulator program",
            Self::ConfigurationFile => "settings file",
            Self::SaveDirectory => "save folder",
            Self::FirmwareFile => "firmware file",
        }
    }
}

impl ProcessExit {
    /// How the process ended, in plain words.
    pub fn describe(&self) -> String {
        let mut text = match &self.outcome {
            ExitOutcome::SpawnFailed { reason, os_error } => {
                let why = match reason {
                    SpawnFailure::ProgramNotFound => "the program was not found".to_string(),
                    SpawnFailure::PermissionDenied => "permission was denied".to_string(),
                    SpawnFailure::Other => match os_error {
                        Some(code) => format!("operating system error {code}"),
                        None => "the operating system refused".to_string(),
                    },
                };
                format!("could not be started: {why}")
            }
            ExitOutcome::Exited { code } => format!("exited with status {code}"),
            ExitOutcome::Signaled {
                signal,
                core_dumped,
            } => {
                let name = super::process::signal_info(*signal)
                    .map_or_else(|| format!("signal {signal}"), |info| info.name.to_string());
                let dumped = if *core_dumped { ", core dumped" } else { "" };
                format!("ended by {name}{dumped}")
            }
            ExitOutcome::StillRunning => "still running when observation stopped".to_string(),
            ExitOutcome::StatusUnavailable => "exit status could not be read".to_string(),
        };
        if let Some(ms) = self.duration_ms {
            text.push_str(&format!(
                " after {}.{:01} s",
                ms / 1_000,
                (ms % 1_000) / 100
            ));
        }
        if let Some(launcher) = &self.launcher {
            text.push_str(&format!(" (as reported by {launcher})"));
        }
        text
    }
}

impl Fact {
    /// A plain-language description. Callers that display it should go
    /// through [`Evidence::summary`], which also bounds and sanitises it.
    pub fn describe(&self) -> String {
        let yes_no = |flag: Option<bool>, yes: &str, no: &str| match flag {
            Some(true) => format!(", {yes}"),
            Some(false) => format!(", {no}"),
            None => String::new(),
        };
        match self {
            Self::Absent => "nothing observed".to_string(),
            Self::GameIdentity {
                verified,
                authority,
                platform,
                title,
            } => {
                let what = match (title, platform) {
                    (Some(title), Some(platform)) => format!("{title} ({platform})"),
                    (Some(title), None) => title.clone(),
                    (None, Some(platform)) => format!("a {platform} game"),
                    (None, None) => "the game".to_string(),
                };
                if *verified {
                    format!("{what}: identity verified by {authority}")
                } else {
                    format!("{what}: identity not verified ({authority})")
                }
            }
            Self::MediaIntegrity { status, detail } => {
                let state = match status {
                    MediaIntegrityStatus::Verified => "verified",
                    MediaIntegrityStatus::Unverified => "not verified",
                    MediaIntegrityStatus::Failed => "check failed",
                };
                format!("game file integrity {state}: {detail}")
            }
            Self::EmulatorExecutable {
                emulator,
                path,
                installation,
                version,
                ..
            } => {
                let version = version.as_ref().map_or_else(
                    || ", version unknown".to_string(),
                    |v| format!(", version {v}"),
                );
                let installation = installation
                    .as_ref()
                    .map_or_else(String::new, |form| format!(", installed as {form}"));
                format!("{emulator} at {path}{version}{installation}")
            }
            Self::Firmware {
                requirement,
                status,
            } => {
                let state = match status {
                    FirmwareStatus::Verified => "verified",
                    FirmwareStatus::PresentUnverified => "present, not verified",
                    FirmwareStatus::Missing => "missing",
                    FirmwareStatus::Unknown => "state unknown",
                    FirmwareStatus::NotRequired => "not required",
                };
                format!("{requirement}: {state}")
            }
            Self::GraphicsApi {
                api,
                scope,
                usable,
                devices,
                detail,
            } => {
                let place = match scope {
                    ProbeScope::LaunchEnvironment => "where the emulator runs",
                    ProbeScope::Host => "on this computer in general",
                };
                let state = if *usable {
                    "can be used"
                } else {
                    "cannot be used"
                };
                let devices = if devices.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", devices.join(", "))
                };
                let detail = if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                };
                format!("{} {state} {place}{devices}{detail}", api.label())
            }
            Self::ConfigValue {
                layer,
                key,
                value,
                line,
                ..
            } => match value {
                Some(value) => {
                    let line = line.map_or_else(String::new, |line| format!(", line {line}"));
                    format!("{key} = \"{value}\" in {}{line}", layer.label())
                }
                None => format!("{key} is not set in {}", layer.label()),
            },
            Self::ConfigStructure {
                layer,
                includes,
                malformed_lines,
                repeated_keys,
                ..
            } => {
                let repeated = if repeated_keys.is_empty() {
                    "no repeated keys".to_string()
                } else {
                    format!("set more than once: {}", repeated_keys.join(", "))
                };
                format!(
                    "{} file: {includes} included file(s) not read, {malformed_lines} \
                     unreadable line(s), {repeated}",
                    layer.label()
                )
            }
            Self::ConfigLayerAbsent { layer, .. } => {
                format!("no settings file exists for {}", layer.label())
            }
            Self::EnvironmentVariable { name, value } => match value {
                Some(value) => format!("{name}={value}"),
                None => format!("{name} is not set"),
            },
            Self::ProcessExit(exit) => format!("the emulator process {}", exit.describe()),
            Self::LogEvent(event) => format!("line {}: {}", event.line, event.excerpt),
            Self::LogCoverage(coverage) => {
                let stream = match coverage.stream {
                    LogStream::LogFile => "log file",
                    LogStream::Stderr => "standard error",
                };
                let detail = if coverage.verbose {
                    ", detailed logging"
                } else {
                    ", errors only"
                };
                let cut = if coverage.truncated {
                    ", cut short"
                } else {
                    ""
                };
                format!(
                    "{} line(s) of {stream} examined for {} messages{detail}{cut}",
                    coverage.lines_examined, coverage.emulator
                )
            }
            Self::ReadinessBlocker { detail, .. } => detail.clone(),
            Self::ControllerBinding {
                emulator,
                player,
                bound,
            } => {
                let state = if *bound {
                    "a controller is assigned"
                } else {
                    "no controller assigned"
                };
                format!("{emulator} player {player}: {state}")
            }
            Self::PathAccess(access) => {
                let state = match access.presence {
                    PathPresence::Missing
                    | PathPresence::SymbolicLink
                    | PathPresence::Other
                    | PathPresence::Unknown => access.presence.describe().to_string(),
                    PathPresence::RegularFile => format!(
                        "is a file{}{}",
                        yes_no(access.readable, "readable", "not readable"),
                        yes_no(access.writable, "writable", "not writable")
                    ),
                    PathPresence::Directory => format!(
                        "is a folder{}{}",
                        yes_no(access.readable, "readable", "not readable"),
                        yes_no(access.writable, "writable", "not writable")
                    ),
                };
                format!("{} {} {state}", access.role.label(), access.path)
            }
            Self::ModificationConflict {
                target,
                first,
                second,
            } => format!("{first} and {second} both change {target}"),
            Self::KnownLimitation {
                summary, violated, ..
            } => {
                let applies = if *violated {
                    "applies here"
                } else {
                    "satisfied"
                };
                format!("{summary} ({applies})")
            }
        }
    }
}

/// What the evidence is about. Descriptive only; never used to locate or
/// open anything.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub emulator: Option<String>,
    pub game: Option<String>,
}

/// Coverage of one area in an [`EvidenceSet`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AreaCoverage {
    pub area: EvidenceArea,
    /// Items observed by something, and still current.
    pub observed: usize,
    pub stale: usize,
    /// Statements that were only asserted. They are not observations and do
    /// not count as coverage.
    #[serde(default)]
    pub asserted: usize,
    /// Reasons collection failed or was not attempted. An area with no
    /// items at all reports a single "not collected" reason.
    pub missing: Vec<String>,
}

impl AreaCoverage {
    pub fn has_current_evidence(&self) -> bool {
        self.observed > 0
    }
}

/// Everything known about one launch problem.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSet {
    pub subject: Subject,
    items: Vec<Evidence>,
    /// True when a collector had more to add than [`MAX_EVIDENCE_ITEMS`].
    truncated: bool,
}

impl EvidenceSet {
    pub fn new(subject: Subject) -> Self {
        Self {
            subject,
            ..Self::default()
        }
    }

    pub fn items(&self) -> &[Evidence] {
        &self.items
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn get(&self, id: &EvidenceId) -> Option<&Evidence> {
        self.items.iter().find(|item| &item.id == id)
    }

    /// Add one item and return its id, or `None` when the set is full.
    pub fn push(
        &mut self,
        area: EvidenceArea,
        state: EvidenceState,
        trust: Trust,
        source: impl Into<String>,
        fingerprint: Option<SourceFingerprint>,
        fact: Fact,
    ) -> Option<EvidenceId> {
        if self.items.len() >= MAX_EVIDENCE_ITEMS {
            self.truncated = true;
            return None;
        }
        // Items are never removed, so the count of an area's items is also
        // the last number used in it, including after a set is restored
        // from JSON.
        let number = self.items.iter().filter(|item| item.area == area).count() + 1;
        let id = EvidenceId(format!("{}-{number}", area.tag()));
        self.items.push(Evidence {
            id: id.clone(),
            area,
            state,
            trust,
            source: text::sanitise(&source.into(), text::SOURCE_LIMIT),
            fingerprint,
            fact,
        });
        Some(id)
    }

    /// Add an observed fact.
    pub fn observe(
        &mut self,
        area: EvidenceArea,
        trust: Trust,
        source: impl Into<String>,
        fact: Fact,
    ) -> Option<EvidenceId> {
        self.push(area, EvidenceState::Observed, trust, source, None, fact)
    }

    /// Record that an area could not be observed.
    pub fn unavailable(
        &mut self,
        area: EvidenceArea,
        source: impl Into<String>,
        reason: impl Into<String>,
    ) -> Option<EvidenceId> {
        let reason = text::sanitise(&reason.into(), text::SOURCE_LIMIT);
        self.push(
            area,
            EvidenceState::Unavailable { reason },
            Trust::Measured,
            source,
            None,
            Fact::Absent,
        )
    }

    /// Record that nothing tried to observe an area.
    pub fn not_collected(
        &mut self,
        area: EvidenceArea,
        reason: impl Into<String>,
    ) -> Option<EvidenceId> {
        let reason = text::sanitise(&reason.into(), text::SOURCE_LIMIT);
        self.push(
            area,
            EvidenceState::NotCollected { reason },
            Trust::Measured,
            "not collected",
            None,
            Fact::Absent,
        )
    }

    /// Observed, current items in one area.
    pub fn observed(&self, area: EvidenceArea) -> impl Iterator<Item = &Evidence> {
        self.items
            .iter()
            .filter(move |item| item.area == area && item.is_observed())
    }

    /// Re-check every fingerprinted source and mark items whose source has
    /// changed as [`EvidenceState::Stale`]. Returns the ids that became
    /// stale. Run immediately before diagnosing and again before acting on
    /// a diagnosis: evidence read from a file that has since changed must
    /// not carry a conclusion.
    pub fn refresh_freshness(&mut self) -> Vec<EvidenceId> {
        let mut stale = Vec::new();
        // One check per distinct source file, however many items share it.
        let mut checked: BTreeMap<String, FingerprintCheck> = BTreeMap::new();
        for item in &mut self.items {
            if !item.state.is_observed() {
                continue;
            }
            let Some(fingerprint) = &item.fingerprint else {
                continue;
            };
            let key = format!("{}\0{:?}", fingerprint.path, fingerprint.sha256);
            let check = checked
                .entry(key)
                .or_insert_with(|| fingerprint.check())
                .clone();
            let reason = match check {
                FingerprintCheck::Unchanged => continue,
                FingerprintCheck::Changed { detail } | FingerprintCheck::Unavailable { detail } => {
                    detail
                }
            };
            item.state = EvidenceState::Stale {
                reason: text::sanitise(&reason, text::SOURCE_LIMIT),
            };
            stale.push(item.id.clone());
        }
        stale
    }

    /// Coverage of every area, including those with no evidence at all.
    pub fn coverage(&self) -> Vec<AreaCoverage> {
        EvidenceArea::ALL
            .iter()
            .map(|&area| {
                let mut coverage = AreaCoverage {
                    area,
                    observed: 0,
                    stale: 0,
                    asserted: 0,
                    missing: Vec::new(),
                };
                let mut any = false;
                for item in self.items.iter().filter(|item| item.area == area) {
                    any = true;
                    match &item.state {
                        EvidenceState::Observed if item.trust == Trust::Asserted => {
                            coverage.asserted += 1;
                        }
                        EvidenceState::Observed => coverage.observed += 1,
                        EvidenceState::Stale { .. } => coverage.stale += 1,
                        EvidenceState::Unavailable { reason } => {
                            coverage.missing.push(format!("unavailable: {reason}"));
                        }
                        EvidenceState::NotCollected { reason } => {
                            coverage.missing.push(format!("not collected: {reason}"));
                        }
                    }
                }
                if !any {
                    coverage.missing.push("not collected".into());
                } else if coverage.observed == 0 && coverage.asserted > 0 {
                    coverage.missing.push("only asserted, not observed".into());
                }
                coverage
            })
            .collect()
    }
}

/// Look at one path without opening it: what is there (never following a
/// final symbolic link) and whether this user may read or write it,
/// according to `access(2)`.
pub fn observe_path_access(role: PathRole, path: &Path) -> PathAccess {
    use std::os::unix::ffi::OsStrExt;
    let presence = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => PathPresence::SymbolicLink,
        Ok(metadata) if metadata.file_type().is_file() => PathPresence::RegularFile,
        Ok(metadata) if metadata.file_type().is_dir() => PathPresence::Directory,
        Ok(_) => PathPresence::Other,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PathPresence::Missing,
        Err(_) => PathPresence::Unknown,
    };
    let may = |mode: libc::c_int| -> Option<bool> {
        if !matches!(
            presence,
            PathPresence::RegularFile | PathPresence::Directory
        ) {
            return None;
        }
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c_path` is a valid NUL-terminated string for the duration
        // of the call, and access(2) does not retain the pointer.
        let result = unsafe { libc::access(c_path.as_ptr(), mode) };
        Some(result == 0)
    };
    PathAccess {
        role,
        path: text::sanitise(&path.display().to_string(), text::PATH_LIMIT),
        presence,
        readable: may(libc::R_OK),
        writable: may(libc::W_OK),
    }
}

#[cfg(test)]
#[path = "evidence/tests.rs"]
mod tests;
