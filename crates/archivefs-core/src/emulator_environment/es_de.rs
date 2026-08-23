//! Read-only ES-DE (EmulationStation Desktop Edition) environment
//! discovery - first slice.
//!
//! Discovers a Native `~/ES-DE` profile plus an `Explicit` profile from a
//! caller-supplied home directory, and reads
//! `custom_systems/es_systems.xml` (ES-DE's own documented location for
//! user-editable system definitions - always attempted) plus any
//! explicitly caller-supplied stand-ins for the bundled/default
//! `es_systems.xml` (never guessed - see "Bundled vs. custom systems"
//! below). Nothing here downloads, installs, executes, launches a game,
//! generates a gamelist, or modifies `es_systems.xml` or anything else.
//!
//! # Source verification
//!
//! Every default path in this module was checked against ES-DE's own
//! documentation on 2026-08-23 (network access was available in this
//! environment for this revision - an earlier revision of this module
//! guessed at several of these and got them wrong; see the corrections
//! below):
//!
//! - `USERGUIDE.md` (`gitlab.com/es-de/emulationstation-de`, `master`
//!   branch): "On Linux this means `/home/<username>/ES-DE`" for the
//!   application data/home directory; the directory tree lists
//!   `settings/` (`es_settings.xml`, `es_input.xml`), `custom_systems/`,
//!   `gamelists/`, `collections/`, `downloaded_media/`, `logs/`,
//!   `themes/` all directly under it; a custom `es_systems.xml` is
//!   documented to "complement the bundled configuration, meaning only
//!   systems that are to be customized should be included" (not the
//!   complete system list); "As of ES-DE 2.0.0 any gamelist.xml files
//!   stored in the game system directories... will not get loaded, they
//!   are instead required to be placed in the `~/ES-DE/gamelists/`
//!   directory tree"; ES-DE itself is not distributed as a Flatpak -
//!   RetroDECK (a separate, third-party project bundling ES-DE with
//!   emulators) is "shipped as a Flatpak", a distinct thing from ES-DE's
//!   own official packaging.
//! - `INSTALL.md` (same repository): ES-DE's own officially supported
//!   Linux packaging formats are AppImage, `.deb`, `.rpm`, and Haiku
//!   `.hpkg` - no official Flatpak. For an AppImage release "the default
//!   es_systems.xml is embedded within the AppImage" itself; reference
//!   copies (not a live install path) exist in the repository under
//!   `resources/systems/linux`/`resources/systems/linuxarm`.
//!
//! # Corrections from the first revision of this module
//!
//! - The home directory is **`~/ES-DE`** - a single root, not the
//!   `~/.config/ES-DE` (config) + `~/ES-DE` (data) split the first
//!   revision invented by analogy with RetroArch's XDG-based layout.
//!   ES-DE does not use an XDG-style config/data split at all.
//! - **`ProfileKind::Flatpak` has been removed.** ES-DE itself has no
//!   official standalone Flatpak; the `org.es_de.emulationstation-de`
//!   application id the first revision used was fabricated and did not
//!   correspond to anything real. RetroDECK (Flatpak-distributed, bundles
//!   ES-DE with its own emulators, "supports less systems and emulators"
//!   than a standalone install per `USERGUIDE.md`) is a genuinely
//!   different product, out of scope here - an unusual packaged install
//!   like RetroDECK's is still reachable through `ProfileKind::Explicit`.
//! - `es_settings.xml`'s location (`settings/es_settings.xml` under the
//!   ES-DE home) is now source-verified and its *existence* is now part
//!   of the model (`EsDeProfile::settings_file`, a probe only - its
//!   content is not read; see "Deliberately deferred" below).
//! - `ProfileKind::AppImage` still does not exist as a separate profile:
//!   an AppImage release uses the same `~/ES-DE` home as any other Linux
//!   package. Instead, `EsDeProfile::appimage_candidates` records
//!   *evidence* of an ES-DE AppImage file at a small, fixed, bounded,
//!   non-recursive set of locations - the same locations
//!   `USERGUIDE.md` documents ES-DE's own bundled configuration using to
//!   search for *emulator* AppImages, reused here by analogy for
//!   ES-DE's own AppImage since no location more specific to ES-DE
//!   itself is documented. This is provenance only: the AppImage is
//!   never extracted, mounted, or executed.
//!
//! # Bundled vs. custom systems
//!
//! `custom_systems/es_systems.xml` **complements** ES-DE's bundled
//! default system list; it is not the complete list of systems ES-DE
//! actually supports (source-verified, see above). The bundled file's
//! real location is packaging-dependent - embedded inside the AppImage
//! itself for an AppImage release, or somewhere distro-specific for a
//! `.deb`/`.rpm` install - and this adapter never guesses at it (that
//! would mean either extracting/mounting the AppImage, forbidden by this
//! milestone, or fabricating a distro-specific share path with no
//! general basis). A caller who independently knows their bundled file's
//! path (e.g. read from `resources/systems/linux` in a source checkout,
//! or extracted through some other authorized channel) may supply it via
//! [`DiscoveryEnvironment::explicit_bundled_systems_files`], read through
//! the exact same bounded parser and tagged [`SystemsFileRole::Bundled`]
//! in the result so a caller can always tell which role a given
//! `es_systems.xml`-shaped file played.
//!
//! [`EsDeProfile::systems_may_be_incomplete`] is `true` whenever no
//! `Bundled`-role file was successfully read - the loud, structural
//! answer to "does an empty/short `systems` list mean ES-DE has no
//! systems configured?" (No: it almost always just means the bundled
//! file was never supplied.)
//!
//! # Deliberately deferred
//!
//! `es_settings.xml`'s *content* (e.g. a configured `ROMDirectory`) is
//! still not read in this slice - only its existence is probed. Nothing
//! required by this milestone depends on its content, and parsing it
//! would be new, unreviewed scope.
//!
//! # Installation discovery extension (executable + eligibility)
//!
//! In addition to the config/systems discovery above, this module answers
//! "is ES-DE installed, and is that installation usable" for four kinds of
//! candidate: [`ProfileKind::Native`] (PATH lookup), [`ProfileKind::Explicit`]
//! (caller-supplied home directory and/or executable),
//! [`ProfileKind::AppImage`] (caller-supplied AppImage executable path -
//! never guessed at, never the bounded evidence search described above),
//! and [`ProfileKind::Portable`] (caller-supplied executable *and* home
//! directory together - never inferred from a filename alone). There is
//! still no `Flatpak` kind: confirmed again against `INSTALL.md`/
//! `USERGUIDE.md` (same repository, `master`, checked 2026-08-23) - ES-DE's
//! own officially supported Linux packaging remains AppImage/`.deb`/`.rpm`
//! only, and RetroDECK (the Flatpak-shipped, ES-DE-bundling product) stays
//! out of scope; it is still reachable through `ProfileKind::Explicit`.
//!
//! - Native executable name and install path: `INSTALL.md` documents
//!   `/usr/bin/es-de` as the installed binary and shows it invoked as
//!   `./es-de` from a build directory - the executable this module's
//!   bounded `$PATH` scan looks for is exactly `es-de`.
//! - The `--home` flag: `USERGUIDE.md` documents that "If the `--home`
//!   command line option was used to start ES-DE, the tilde `~` symbol
//!   will resolve to whatever directory was passed as an argument to this
//!   option" - so for any profile representing a `--home` override
//!   (`Explicit`, `Portable`, and an `AppImage` profile whose caller
//!   supplied its own `config_root`), `~`-relative `<path>` values in
//!   `es_systems.xml` are resolved against *that profile's own* home
//!   directory, not the real machine `$HOME`; a bare `AppImage` profile
//!   with no `config_root` override uses the same undecorated `~/ES-DE`
//!   as `Native`.
//! - Portable release: `USERGUIDE.md`/`Windows_Portable_README.txt`
//!   describe a ZIP that "can be unzipped anywhere" - there is no
//!   documented default location or filename convention for it, so
//!   [`DiscoveryEnvironment::explicit_portables`] is the only way a
//!   `Portable` profile is ever produced; this module never scans for one.
//!
//! Every caller-supplied executable path (`Explicit`, `AppImage`,
//! `Portable`) is probed with the same no-follow [`ReadOnlyHostFilesystem::probe`]
//! used everywhere else in this module - a symlink there is treated as
//! unsafe, never followed. Only the `Native` `$PATH` scan follows a final
//! symlink, mirroring `retroarch::discover_native_executables`'s identical,
//! narrowly-scoped exception (see the symlink policy note on
//! [`ReadOnlyHostFilesystem`] in the parent module).
//!
//! # Platform identity
//!
//! ES-DE system names, platform tags, and theme identifiers are
//! downstream frontend metadata only. Nothing in this module resolves,
//! matches, or otherwise turns them into canonical EmuWiz platform
//! identity - see [`EsDeSystemFinding`].

use std::env;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::Event;
use serde::Serialize;

use super::{
    BoundedListResult, BoundedReadResult, EncodedPath, ExecutableProbe, FsProbe,
    HostReadOnlyFilesystem, ReadOnlyHostFilesystem,
};

/// The installed native Linux binary name - source-verified, see the
/// module doc comment ("Installation discovery extension").
const NATIVE_EXECUTABLE_NAME: &str = "es-de";

/// Bound on how many `$PATH` entries a native executable lookup walks -
/// mirrors the defensive intent of this module's other bounded limits;
/// no real `$PATH` is anywhere near this long.
const MAX_PATH_ENTRIES: usize = 512;

pub const MAX_EXPLICIT_APPIMAGE_CANDIDATES: usize = 8;
pub const MAX_EXPLICIT_PORTABLE_CANDIDATES: usize = 8;

/// ES-DE's documented location for user-editable system definitions,
/// relative to the ES-DE home directory - source-verified, see the
/// module doc comment.
const CUSTOM_SYSTEMS_RELATIVE_PATH: &str = "custom_systems/es_systems.xml";

/// Where `es_settings.xml` is generated, relative to the ES-DE home
/// directory - source-verified, see the module doc comment.
const SETTINGS_FILE_RELATIVE_PATH: &str = "settings/es_settings.xml";

/// Bounded, non-recursive AppImage evidence search roots, relative to
/// `$HOME` - the same fixed list `USERGUIDE.md` documents ES-DE's own
/// bundled configuration using to search for *emulator* AppImages,
/// reused here by analogy for evidence of ES-DE's own AppImage. See the
/// module doc comment.
const APPIMAGE_SEARCH_ROOT_RELATIVE_PATHS: [&str; 4] = [
    "Applications",
    ".local/share/applications",
    ".local/bin",
    "bin",
];

pub const MAX_SYSTEMS_XML_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SYSTEMS_PER_FILE: usize = 1024;
pub const MAX_FIELD_TEXT_BYTES: usize = 8 * 1024;
pub const MAX_EXTENSIONS_PER_SYSTEM: usize = 256;
pub const MAX_PLATFORM_TAGS_PER_SYSTEM: usize = 32;
pub const MAX_APPIMAGE_SEARCH_ROOT_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// The one real ES-DE home directory (`~/ES-DE`), regardless of
    /// which officially-supported package format (AppImage, `.deb`,
    /// `.rpm`) put it there - they all share this same layout. See
    /// `appimage_candidates` for AppImage-specific provenance evidence.
    /// Its executable is located through a bounded `$PATH` scan for
    /// `es-de` - see "Installation discovery extension" in the module doc
    /// comment.
    Native,
    /// A caller-supplied home directory and/or executable - never
    /// auto-discovered. The way this adapter reaches a non-standard
    /// install (a custom `--home-path`, RetroDECK's own layout, ...).
    Explicit,
    /// A caller-supplied AppImage executable path (and, optionally, a
    /// distinct configuration root) - distinct from
    /// `EsDeProfile::appimage_candidates`, which is only unexecuted,
    /// unopened evidence. Never auto-discovered from a bare filename.
    AppImage,
    /// A caller-supplied executable *and* home directory pair
    /// representing ES-DE's portable (ZIP) release - never inferred from
    /// a filename or location alone, since ES-DE documents no fixed
    /// portable location.
    Portable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategory {
    Discovery,
    ConfigParse,
    SystemsInventory,
    Filesystem,
}

/// A structured, machine-readable finding - deliberately no free-text
/// `message` field, mirroring `emulator_environment::retroarch::Diagnostic`.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: DiagnosticSeverity,
    pub detail_kind: DiagnosticCategory,
    pub profile_kind: Option<ProfileKind>,
    pub path: Option<EncodedPath>,
}

impl Diagnostic {
    fn new(
        code: &'static str,
        severity: DiagnosticSeverity,
        detail_kind: DiagnosticCategory,
        profile_kind: Option<ProfileKind>,
        path: Option<&Path>,
    ) -> Self {
        Self {
            code,
            severity,
            detail_kind,
            profile_kind,
            path: path.map(EncodedPath::from_path),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DirectoryProbeFinding {
    pub path: EncodedPath,
    pub probe: FsProbe,
}

fn probe_directory(filesystem: &dyn ReadOnlyHostFilesystem, path: &Path) -> DirectoryProbeFinding {
    DirectoryProbeFinding {
        path: EncodedPath::from_path(path),
        probe: filesystem.probe(path),
    }
}

/// Evidence of an ES-DE AppImage file - see the module doc comment.
/// Never extracted, mounted, or executed.
#[derive(Debug, Clone, Serialize)]
pub struct AppImageCandidate {
    pub path: EncodedPath,
    pub probe: FsProbe,
}

/// Which role one `es_systems.xml`-shaped file played - see "Bundled vs.
/// custom systems" in the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemsFileRole {
    /// `custom_systems/es_systems.xml` under the ES-DE home - always
    /// attempted. Complements, never replaces, the bundled default
    /// system list.
    Custom,
    /// A caller-supplied path standing in for the bundled/default
    /// `es_systems.xml`, whose real location this adapter cannot safely
    /// determine on its own.
    Bundled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemsFileReadOutcome {
    Parsed {
        systems_found: u32,
        /// `true` only when a `<system>` element failed to parse (e.g. an
        /// unclosed tag) and the reader had to stop early - already-parsed
        /// systems before the failure are still returned, never discarded.
        truncated: bool,
    },
    NotFound,
    TooLarge {
        limit_bytes: u64,
    },
    InvalidUtf8,
    Unreadable,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemsFileFinding {
    pub role: SystemsFileRole,
    pub path: EncodedPath,
    pub probe: FsProbe,
    pub read: SystemsFileReadOutcome,
}

/// How one system's `<path>` value resolved. Mirrors the shape of
/// `retroarch::ResolutionState`, redefined locally rather than shared -
/// see the module doc comment on why no shared trait/vocabulary exists
/// yet between the two adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathResolutionState {
    /// Absolute, or `~`-prefixed and expanded against the profile's own
    /// home directory.
    Resolved,
    /// Contains an ES-DE `%VARIABLE%` placeholder (e.g. `%ROMPATH%`) this
    /// adapter does not substitute - see the module doc comment.
    ContainsUnexpandedVariable,
    /// A non-empty value that is neither absolute, `~`-relative, nor
    /// recognized as containing a placeholder - EmuWiz declines to guess
    /// a resolution base for it.
    Unresolved,
    /// The `<path>` element was absent or empty.
    NotConfigured,
}

/// How a profile's executable search concluded. See "Installation
/// discovery extension" in the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableSearchOutcome {
    /// A regular, executable file was found.
    Found,
    NotFound,
    /// Present but not safely usable: a symlink (never followed for a
    /// caller-supplied path), a directory, or a regular file missing the
    /// executable permission bit.
    Unsafe,
    /// No caller-supplied executable path exists to check (an `Explicit`
    /// profile the caller gave only a home directory for).
    NotSearched,
}

/// How a profile's executable path was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableProvenance {
    /// Found via a bounded `$PATH` scan for `es-de` - follows a final
    /// symlink, the same narrowly-scoped exception `retroarch` uses for
    /// its own native executable lookup.
    PathLookup,
    /// An exact path supplied directly by the caller - never guessed at,
    /// never searched for. A symlink here is never followed.
    CallerSuppliedPath,
    NotSearched,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutableFinding {
    pub path: Option<EncodedPath>,
    pub outcome: ExecutableSearchOutcome,
    pub provenance: ExecutableProvenance,
}

/// Why a profile is not [`EsDeProfile::eligible`]. See "Installation
/// discovery extension" in the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EligibilityBlocker {
    ExecutableMissing,
    ExecutableUnsafe,
    ConfigurationRootMissing,
    ConfigurationRootUnsafe,
    /// Two or more caller-supplied candidates resolved to the same
    /// configuration root (or the same home directory, for `Portable`)
    /// through different executables - this adapter never silently picks
    /// one, so every profile sharing the conflict is ineligible.
    ConflictingCandidates,
}

/// How a profile's home directory/executable pairing was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileProvenance {
    /// `~/ES-DE`, ES-DE's own documented default home directory, paired
    /// with a `$PATH`-discovered executable.
    DocumentedDefaultHome,
    /// A home directory and/or executable path supplied directly by the
    /// caller - never auto-discovered.
    CallerSupplied,
}

/// One `<system>` entry read from an `es_systems.xml`-shaped file.
///
/// Every field here is downstream ES-DE frontend metadata, preserved
/// verbatim. Nothing in this module - or any caller of it - may treat
/// `name`, `platform_tags`, or `theme` as canonical EmuWiz platform
/// identity: ES-DE's own system namespace (short ids like `snes`,
/// `mastersystem`) and platform tag vocabulary are declared by ES-DE, not
/// by EmuWiz, and the two are never assumed to agree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EsDeSystemFinding {
    pub name: Option<String>,
    pub fullname: Option<String>,
    pub rom_path_raw: Option<String>,
    pub rom_path_resolved: Option<EncodedPath>,
    pub rom_path_resolution: PathResolutionState,
    /// From `<extension>`, whitespace-split. Never deduplicated or
    /// reordered - see `split_whitespace_tokens`.
    pub extensions: Vec<String>,
    /// The raw `<command>` text, preserved exactly as written (including
    /// any `%ROM%`/`%EMULATOR_*%`/`%CORE_*%` placeholders). Never parsed
    /// into argv tokens, never executed, never used to select or launch
    /// anything in this milestone.
    pub command: Option<String>,
    /// From `<platform>`, comma-split. Downstream metadata only - see
    /// this struct's own doc comment.
    pub platform_tags: Vec<String>,
    pub theme: Option<String>,
}

/// One discovered gamelist/media location for a system already listed in
/// [`EsDeProfile::systems`]. Location only - the file's content is never
/// read or parsed by this milestone.
#[derive(Debug, Clone, Serialize)]
pub struct EsDeSystemDataLocations {
    pub system_name: String,
    pub gamelist_file: DirectoryProbeFinding,
    pub media_directory: DirectoryProbeFinding,
}

#[derive(Debug, Clone, Serialize)]
pub struct EsDeProfile {
    /// Stable identifier for this profile, unique within one report - e.g.
    /// `"native"`, `"explicit"`, or `"app_image:<executable path>"`. Never
    /// reused across a different candidate.
    pub profile_id: String,
    pub profile_kind: ProfileKind,
    pub provenance: ProfileProvenance,
    /// The `es-de` executable this profile would launch - never itself
    /// executed, launched, or version-probed. See "Installation discovery
    /// extension" in the module doc comment.
    pub executable: ExecutableFinding,
    /// Whether every fail-closed requirement in "Installation discovery
    /// extension" is satisfied: a `Found` executable and a `PresentDirectory`
    /// configuration root, with no conflicting candidate. `false` whenever
    /// `blockers` is non-empty.
    pub eligible: bool,
    pub blockers: Vec<EligibilityBlocker>,
    /// `~/ES-DE` (or the explicit equivalent) - the single ES-DE home
    /// directory, source-verified to hold `settings/`, `custom_systems/`,
    /// `gamelists/`, `downloaded_media/`, etc. directly (no separate
    /// config-vs-data split).
    pub home_directory: DirectoryProbeFinding,
    /// `<home_directory>/settings/es_settings.xml` - existence probe
    /// only, see "Deliberately deferred" in the module doc comment.
    pub settings_file: DirectoryProbeFinding,
    /// `<home_directory>/gamelists` - the parent directory gamelist
    /// locations in `system_data` are resolved under. Probed once here
    /// so a caller can tell "ES-DE has never created any gamelists at
    /// all" apart from "this one system has none yet".
    pub gamelists_directory: DirectoryProbeFinding,
    /// `<home_directory>/downloaded_media` - see `gamelists_directory`.
    pub media_root_directory: DirectoryProbeFinding,
    /// Evidence only - see [`AppImageCandidate`] and the module doc
    /// comment. Sorted by encoded path.
    pub appimage_candidates: Vec<AppImageCandidate>,
    /// Every `es_systems.xml`-shaped file this profile actually read:
    /// `custom_systems/es_systems.xml` (always attempted, role
    /// [`SystemsFileRole::Custom`]) followed by every path in
    /// [`DiscoveryEnvironment::explicit_bundled_systems_files`] (role
    /// [`SystemsFileRole::Bundled`]).
    pub systems_files: Vec<SystemsFileFinding>,
    /// Every system successfully parsed, concatenated across
    /// `systems_files` in the order the files were read. A duplicate
    /// `name` across two files is never merged or deduplicated - both
    /// entries are kept, exactly as ES-DE's own layered-override
    /// behavior would need a caller to reason about.
    pub systems: Vec<EsDeSystemFinding>,
    /// `true` whenever no [`SystemsFileRole::Bundled`] file was
    /// successfully parsed - see "Bundled vs. custom systems" in the
    /// module doc comment. A caller must never read a short/empty
    /// `systems` list as "ES-DE has no systems configured" while this is
    /// `true`.
    pub systems_may_be_incomplete: bool,
    /// One entry per `systems` entry with a non-empty `name`, in the
    /// same order.
    pub system_data: Vec<EsDeSystemDataLocations>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EsDeEnvironmentReport {
    pub format_version: u32,
    pub profiles: Vec<EsDeProfile>,
    pub diagnostics: Vec<Diagnostic>,
    /// `false` whenever some part of discovery could not be fully carried
    /// out - a candidate list hit its bound (see
    /// [`MAX_EXPLICIT_APPIMAGE_CANDIDATES`]/[`MAX_EXPLICIT_PORTABLE_CANDIDATES`]
    /// or the AppImage evidence search), or a required path could not be
    /// read due to a permission/IO error. `true` never implies every
    /// profile is eligible - only that discovery itself ran to
    /// completion.
    pub discovery_complete: bool,
}

/// Injected discovery inputs - mirrors
/// `retroarch::DiscoveryEnvironment`'s reasoning (production code uses
/// [`Self::from_process_environment`]; tests construct this directly so
/// discovery never depends on the developer's real `HOME` or a real ES-DE
/// install).
#[derive(Debug, Clone)]
pub struct DiscoveryEnvironment {
    pub home: Option<std::ffi::OsString>,
    /// `$PATH`, used only for the bounded native `es-de` executable
    /// lookup - see "Installation discovery extension" in the module doc
    /// comment. Production code uses the real process `$PATH`; tests
    /// inject their own so discovery never depends on the real machine's
    /// installed programs.
    pub path: Option<std::ffi::OsString>,
    /// Caller-supplied paths standing in for ES-DE's bundled/default
    /// `es_systems.xml`, for every profile discovered - never
    /// auto-discovered. See "Bundled vs. custom systems" in the module
    /// doc comment.
    pub explicit_bundled_systems_files: Vec<PathBuf>,
    /// Bounded, fixed AppImage evidence search roots - production code
    /// populates this with [`APPIMAGE_SEARCH_ROOT_RELATIVE_PATHS`] under
    /// `$HOME`; tests inject their own so discovery never depends on the
    /// real machine's home directory contents.
    pub appimage_search_roots: Vec<PathBuf>,
    /// An explicit caller-supplied home directory, if any - produces a
    /// [`ProfileKind::Explicit`] profile in addition to `Native`.
    pub explicit_root: Option<ExplicitRoot>,
    /// Caller-supplied AppImage executable paths - each produces one
    /// [`ProfileKind::AppImage`] profile. Bounded to
    /// [`MAX_EXPLICIT_APPIMAGE_CANDIDATES`]; exact duplicates are
    /// collapsed to a single profile. Never auto-discovered.
    pub explicit_appimages: Vec<ExplicitAppImage>,
    /// Caller-supplied portable-release executable/home-directory pairs -
    /// each produces one [`ProfileKind::Portable`] profile. Bounded to
    /// [`MAX_EXPLICIT_PORTABLE_CANDIDATES`]; exact duplicates are
    /// collapsed to a single profile. Never auto-discovered, never
    /// inferred from a filename.
    pub explicit_portables: Vec<ExplicitPortableRoot>,
}

#[derive(Debug, Clone)]
pub struct ExplicitRoot {
    pub home_directory: PathBuf,
    /// The `es-de` executable for this explicit install, if the caller
    /// supplied one. `None` leaves [`EsDeProfile::executable`] at
    /// [`ExecutableSearchOutcome::NotSearched`] (and therefore
    /// ineligible - see [`EligibilityBlocker::ExecutableMissing`]).
    pub executable_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ExplicitAppImage {
    pub executable_path: PathBuf,
    /// The configuration root for this AppImage. AppImage releases use
    /// the same undecorated `~/ES-DE` home as any other Linux package
    /// (source-verified, see the module doc comment) unless the caller
    /// knows this AppImage was actually run with its own `--home`
    /// override, so `None` defaults to `~/ES-DE` rather than being left
    /// unresolved.
    pub config_root: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ExplicitPortableRoot {
    pub executable_path: PathBuf,
    pub home_directory: PathBuf,
}

impl DiscoveryEnvironment {
    pub fn from_process_environment() -> Self {
        let home = env::var_os("HOME");
        let path = env::var_os("PATH");
        let appimage_search_roots = home
            .as_ref()
            .map(|home| {
                APPIMAGE_SEARCH_ROOT_RELATIVE_PATHS
                    .iter()
                    .map(|relative| PathBuf::from(home).join(relative))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            home,
            path,
            explicit_bundled_systems_files: Vec::new(),
            appimage_search_roots,
            explicit_root: None,
            explicit_appimages: Vec::new(),
            explicit_portables: Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscoveryError {
    NoHome,
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome => {
                write!(
                    formatter,
                    "HOME is not set; cannot determine any ES-DE discovery roots"
                )
            }
        }
    }
}

impl std::error::Error for DiscoveryError {}

pub fn discover_es_de_environment(
    filesystem: &dyn ReadOnlyHostFilesystem,
    environment: &DiscoveryEnvironment,
) -> Result<EsDeEnvironmentReport, DiscoveryError> {
    let home = environment
        .home
        .as_ref()
        .filter(|value| !value.is_empty())
        .ok_or(DiscoveryError::NoHome)?;
    let home_dir = PathBuf::from(home);

    let mut report_diagnostics = Vec::new();
    let mut profiles = Vec::new();

    let native_executable = environment
        .path
        .as_ref()
        .and_then(|path_value| discover_native_executable(filesystem, path_value));
    let native_executable_finding = ExecutableFinding {
        path: native_executable
            .as_ref()
            .map(|p| EncodedPath::from_path(p)),
        outcome: if native_executable.is_some() {
            ExecutableSearchOutcome::Found
        } else {
            ExecutableSearchOutcome::NotFound
        },
        provenance: ExecutableProvenance::PathLookup,
    };
    profiles.push(build_profile(
        filesystem,
        "native".to_string(),
        ProfileKind::Native,
        ProfileProvenance::DocumentedDefaultHome,
        native_executable_finding,
        &home_dir.join("ES-DE"),
        &home_dir,
        environment,
    ));

    if let Some(explicit) = &environment.explicit_root {
        let executable =
            explicit_executable_finding(filesystem, explicit.executable_path.as_deref());
        profiles.push(build_profile(
            filesystem,
            "explicit".to_string(),
            ProfileKind::Explicit,
            ProfileProvenance::CallerSupplied,
            executable,
            &explicit.home_directory,
            &explicit.home_directory,
            environment,
        ));
    }

    let (appimage_candidates, appimage_limited) = dedupe_and_bound(
        &environment.explicit_appimages,
        |candidate| candidate.executable_path.clone(),
        MAX_EXPLICIT_APPIMAGE_CANDIDATES,
    );
    if appimage_limited {
        report_diagnostics.push(Diagnostic::new(
            "explicit_appimage_candidate_limit_reached",
            DiagnosticSeverity::Warning,
            DiagnosticCategory::Discovery,
            Some(ProfileKind::AppImage),
            None,
        ));
    }
    // Two distinct AppImage executables that resolve to the same
    // configuration root can never be safely told apart - both stay
    // ineligible rather than one being silently preferred.
    let appimage_config_roots: Vec<PathBuf> = appimage_candidates
        .iter()
        .map(|candidate| {
            candidate
                .config_root
                .clone()
                .unwrap_or_else(|| home_dir.join("ES-DE"))
        })
        .collect();
    for (index, candidate) in appimage_candidates.iter().enumerate() {
        let conflicts = appimage_config_roots
            .iter()
            .enumerate()
            .any(|(other_index, root)| {
                other_index != index && root == &appimage_config_roots[index]
            });
        let config_root = appimage_config_roots[index].clone();
        let tilde_home = if candidate.config_root.is_some() {
            config_root.clone()
        } else {
            home_dir.clone()
        };
        let executable = explicit_executable_finding(filesystem, Some(&candidate.executable_path));
        let mut profile = build_profile(
            filesystem,
            format!(
                "app_image:{}",
                EncodedPath::from_path(&candidate.executable_path).display
            ),
            ProfileKind::AppImage,
            ProfileProvenance::CallerSupplied,
            executable,
            &config_root,
            &tilde_home,
            environment,
        );
        if conflicts
            && !profile
                .blockers
                .contains(&EligibilityBlocker::ConflictingCandidates)
        {
            profile
                .blockers
                .push(EligibilityBlocker::ConflictingCandidates);
            profile.eligible = false;
        }
        profiles.push(profile);
    }

    let (portable_candidates, portable_limited) = dedupe_and_bound(
        &environment.explicit_portables,
        |candidate| candidate.executable_path.clone(),
        MAX_EXPLICIT_PORTABLE_CANDIDATES,
    );
    if portable_limited {
        report_diagnostics.push(Diagnostic::new(
            "explicit_portable_candidate_limit_reached",
            DiagnosticSeverity::Warning,
            DiagnosticCategory::Discovery,
            Some(ProfileKind::Portable),
            None,
        ));
    }
    let portable_homes: Vec<PathBuf> = portable_candidates
        .iter()
        .map(|candidate| candidate.home_directory.clone())
        .collect();
    for (index, candidate) in portable_candidates.iter().enumerate() {
        let conflicts = portable_homes
            .iter()
            .enumerate()
            .any(|(other_index, home)| other_index != index && home == &portable_homes[index]);
        let executable = explicit_executable_finding(filesystem, Some(&candidate.executable_path));
        let mut profile = build_profile(
            filesystem,
            format!(
                "portable:{}",
                EncodedPath::from_path(&candidate.home_directory).display
            ),
            ProfileKind::Portable,
            ProfileProvenance::CallerSupplied,
            executable,
            &candidate.home_directory,
            &candidate.home_directory,
            environment,
        );
        if conflicts
            && !profile
                .blockers
                .contains(&EligibilityBlocker::ConflictingCandidates)
        {
            profile
                .blockers
                .push(EligibilityBlocker::ConflictingCandidates);
            profile.eligible = false;
        }
        profiles.push(profile);
    }

    let discovery_complete = compute_discovery_complete(&profiles, &report_diagnostics);

    Ok(EsDeEnvironmentReport {
        format_version: 3,
        profiles,
        diagnostics: report_diagnostics,
        discovery_complete,
    })
}

/// Collapses exact-duplicate candidates (by `key`) to a single entry,
/// preserving first-seen order, then truncates to `limit`. Returns
/// whether truncation actually discarded anything.
fn dedupe_and_bound<T: Clone>(
    candidates: &[T],
    key: impl Fn(&T) -> PathBuf,
    limit: usize,
) -> (Vec<T>, bool) {
    let mut seen = Vec::new();
    let mut deduped = Vec::new();
    for candidate in candidates {
        let candidate_key = key(candidate);
        if seen.contains(&candidate_key) {
            continue;
        }
        seen.push(candidate_key);
        deduped.push(candidate.clone());
    }
    let limited = deduped.len() > limit;
    deduped.truncate(limit);
    (deduped, limited)
}

fn compute_discovery_complete(profiles: &[EsDeProfile], report_diagnostics: &[Diagnostic]) -> bool {
    let limiting_codes = [
        "appimage_search_root_listing_too_large",
        "explicit_appimage_candidate_limit_reached",
        "explicit_portable_candidate_limit_reached",
    ];
    let has_limiting_diagnostic = report_diagnostics
        .iter()
        .chain(
            profiles
                .iter()
                .flat_map(|profile| profile.diagnostics.iter()),
        )
        .any(|diagnostic| limiting_codes.contains(&diagnostic.code));
    let has_unreadable_probe = profiles.iter().any(|profile| {
        matches!(
            profile.home_directory.probe,
            FsProbe::Inaccessible | FsProbe::IoError
        )
    });
    !has_limiting_diagnostic && !has_unreadable_probe
}

/// Probes a caller-supplied executable path with the module's standard
/// no-follow policy - never used for the `Native` `$PATH` lookup, which
/// deliberately follows a final symlink instead (see
/// [`discover_native_executable`]).
fn explicit_executable_finding(
    filesystem: &dyn ReadOnlyHostFilesystem,
    executable_path: Option<&Path>,
) -> ExecutableFinding {
    let Some(path) = executable_path else {
        return ExecutableFinding {
            path: None,
            outcome: ExecutableSearchOutcome::NotSearched,
            provenance: ExecutableProvenance::NotSearched,
        };
    };
    let outcome = match filesystem.probe(path) {
        FsProbe::PresentFile => match filesystem.probe_regular_file_executable_bit(path) {
            Some(true) => ExecutableSearchOutcome::Found,
            _ => ExecutableSearchOutcome::Unsafe,
        },
        FsProbe::Missing => ExecutableSearchOutcome::NotFound,
        _ => ExecutableSearchOutcome::Unsafe,
    };
    ExecutableFinding {
        path: Some(EncodedPath::from_path(path)),
        outcome,
        provenance: ExecutableProvenance::CallerSuppliedPath,
    }
}

/// Bounded `$PATH` scan for the native `es-de` binary - the one place in
/// this module that follows a final-component symlink, matching
/// `retroarch::discover_native_executables`'s identical exception.
fn discover_native_executable(
    filesystem: &dyn ReadOnlyHostFilesystem,
    path_value: &OsStr,
) -> Option<PathBuf> {
    for (index, directory_bytes) in path_value
        .as_bytes()
        .split(|&byte| byte == b':')
        .enumerate()
    {
        if index >= MAX_PATH_ENTRIES {
            break;
        }
        if directory_bytes.is_empty() {
            continue;
        }
        let candidate =
            PathBuf::from(OsStr::from_bytes(directory_bytes)).join(NATIVE_EXECUTABLE_NAME);
        if filesystem.probe_executable(&candidate) == ExecutableProbe::RegularExecutable {
            return Some(candidate);
        }
    }
    None
}

fn eligibility_for(
    executable: &ExecutableFinding,
    home_directory: &DirectoryProbeFinding,
) -> (bool, Vec<EligibilityBlocker>) {
    let mut blockers = Vec::new();
    match executable.outcome {
        ExecutableSearchOutcome::Found => {}
        ExecutableSearchOutcome::NotFound | ExecutableSearchOutcome::NotSearched => {
            blockers.push(EligibilityBlocker::ExecutableMissing);
        }
        ExecutableSearchOutcome::Unsafe => blockers.push(EligibilityBlocker::ExecutableUnsafe),
    }
    match home_directory.probe {
        FsProbe::PresentDirectory => {}
        FsProbe::Missing => blockers.push(EligibilityBlocker::ConfigurationRootMissing),
        _ => blockers.push(EligibilityBlocker::ConfigurationRootUnsafe),
    }
    (blockers.is_empty(), blockers)
}

#[allow(clippy::too_many_arguments)]
fn build_profile(
    filesystem: &dyn ReadOnlyHostFilesystem,
    profile_id: String,
    profile_kind: ProfileKind,
    provenance: ProfileProvenance,
    executable: ExecutableFinding,
    home_directory: &Path,
    tilde_home: &Path,
    environment: &DiscoveryEnvironment,
) -> EsDeProfile {
    let mut diagnostics = Vec::new();

    let home_directory_finding = probe_directory(filesystem, home_directory);
    let settings_file = probe_directory(
        filesystem,
        &home_directory.join(SETTINGS_FILE_RELATIVE_PATH),
    );
    let gamelists_directory = probe_directory(filesystem, &home_directory.join("gamelists"));
    let media_root_directory =
        probe_directory(filesystem, &home_directory.join("downloaded_media"));

    let appimage_candidates = discover_appimage_candidates(
        filesystem,
        &environment.appimage_search_roots,
        profile_kind,
        &mut diagnostics,
    );

    let mut systems_files = Vec::new();
    let mut systems = Vec::new();

    let (custom_finding, custom_systems) = read_systems_file(
        filesystem,
        &home_directory.join(CUSTOM_SYSTEMS_RELATIVE_PATH),
        SystemsFileRole::Custom,
        profile_kind,
        tilde_home,
        &mut diagnostics,
    );
    systems_files.push(custom_finding);
    systems.extend(custom_systems);

    let mut bundled_available = false;
    for path in &environment.explicit_bundled_systems_files {
        let (finding, parsed) = read_systems_file(
            filesystem,
            path,
            SystemsFileRole::Bundled,
            profile_kind,
            tilde_home,
            &mut diagnostics,
        );
        if matches!(finding.read, SystemsFileReadOutcome::Parsed { .. }) {
            bundled_available = true;
        }
        systems_files.push(finding);
        systems.extend(parsed);
    }

    let system_data = systems
        .iter()
        .filter_map(|system| {
            let name = system.name.as_ref()?;
            Some(EsDeSystemDataLocations {
                system_name: name.clone(),
                gamelist_file: probe_directory(
                    filesystem,
                    &home_directory
                        .join("gamelists")
                        .join(name)
                        .join("gamelist.xml"),
                ),
                media_directory: probe_directory(
                    filesystem,
                    &home_directory.join("downloaded_media").join(name),
                ),
            })
        })
        .collect();

    let (eligible, blockers) = eligibility_for(&executable, &home_directory_finding);

    EsDeProfile {
        profile_id,
        profile_kind,
        provenance,
        executable,
        eligible,
        blockers,
        home_directory: home_directory_finding,
        settings_file,
        gamelists_directory,
        media_root_directory,
        appimage_candidates,
        systems_files,
        systems,
        systems_may_be_incomplete: !bundled_available,
        system_data,
        diagnostics,
    }
}

/// Filename looks like an ES-DE AppImage - conservative substring/suffix
/// match only, never opened, mounted, or executed.
fn is_es_de_appimage_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".appimage") && (lower.contains("es-de") || lower.contains("es_de"))
}

fn discover_appimage_candidates(
    filesystem: &dyn ReadOnlyHostFilesystem,
    search_roots: &[PathBuf],
    profile_kind: ProfileKind,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<AppImageCandidate> {
    let mut candidates = Vec::new();
    for root in search_roots {
        match filesystem.list_dir_bounded(root, MAX_APPIMAGE_SEARCH_ROOT_ENTRIES) {
            BoundedListResult::Ok(entries) => {
                for entry in entries {
                    if entry.probe != FsProbe::PresentFile {
                        continue;
                    }
                    let name = entry.file_name.to_string_lossy();
                    if is_es_de_appimage_filename(&name) {
                        candidates.push(AppImageCandidate {
                            path: EncodedPath::from_path(&root.join(&entry.file_name)),
                            probe: entry.probe,
                        });
                    }
                }
            }
            BoundedListResult::TooLarge => {
                diagnostics.push(Diagnostic::new(
                    "appimage_search_root_listing_too_large",
                    DiagnosticSeverity::Warning,
                    DiagnosticCategory::Discovery,
                    Some(profile_kind),
                    Some(root),
                ));
            }
            _ => {}
        }
    }
    candidates.sort_by(|left, right| left.path.display.cmp(&right.path.display));
    candidates
}

fn read_systems_file(
    filesystem: &dyn ReadOnlyHostFilesystem,
    path: &Path,
    role: SystemsFileRole,
    profile_kind: ProfileKind,
    tilde_home: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> (SystemsFileFinding, Vec<EsDeSystemFinding>) {
    let probe = filesystem.probe(path);
    let (read, systems) = match probe {
        FsProbe::Missing => (SystemsFileReadOutcome::NotFound, Vec::new()),
        FsProbe::Symlink | FsProbe::WrongType | FsProbe::PresentDirectory => {
            (SystemsFileReadOutcome::Unreadable, Vec::new())
        }
        FsProbe::Inaccessible | FsProbe::IoError => {
            (SystemsFileReadOutcome::Unreadable, Vec::new())
        }
        FsProbe::PresentFile => match filesystem.read_bounded(path, MAX_SYSTEMS_XML_BYTES) {
            BoundedReadResult::Ok(bytes) => match std::str::from_utf8(&bytes) {
                Ok(text) => {
                    let outcome = parse_systems_xml(text, path, tilde_home, diagnostics);
                    (
                        SystemsFileReadOutcome::Parsed {
                            systems_found: outcome.len() as u32,
                            truncated: false,
                        },
                        outcome,
                    )
                }
                Err(_) => (SystemsFileReadOutcome::InvalidUtf8, Vec::new()),
            },
            BoundedReadResult::TooLarge => (
                SystemsFileReadOutcome::TooLarge {
                    limit_bytes: MAX_SYSTEMS_XML_BYTES as u64,
                },
                Vec::new(),
            ),
            _ => (SystemsFileReadOutcome::Unreadable, Vec::new()),
        },
    };

    if matches!(probe, FsProbe::Symlink) {
        diagnostics.push(Diagnostic::new(
            "systems_file_symlink_not_followed",
            DiagnosticSeverity::Warning,
            DiagnosticCategory::Filesystem,
            Some(profile_kind),
            Some(path),
        ));
    }
    if matches!(read, SystemsFileReadOutcome::TooLarge { .. }) {
        diagnostics.push(Diagnostic::new(
            "systems_file_too_large",
            DiagnosticSeverity::Warning,
            DiagnosticCategory::ConfigParse,
            Some(profile_kind),
            Some(path),
        ));
    }
    if matches!(read, SystemsFileReadOutcome::InvalidUtf8) {
        diagnostics.push(Diagnostic::new(
            "systems_file_invalid_utf8",
            DiagnosticSeverity::Warning,
            DiagnosticCategory::ConfigParse,
            Some(profile_kind),
            Some(path),
        ));
    }

    (
        SystemsFileFinding {
            role,
            path: EncodedPath::from_path(path),
            probe,
            read,
        },
        systems,
    )
}

/// Which `<system>` child element is currently being accumulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemField {
    Name,
    FullName,
    Path,
    Extension,
    Command,
    Platform,
    Theme,
}

fn system_field_for_tag(tag: &[u8]) -> Option<SystemField> {
    match tag {
        b"name" => Some(SystemField::Name),
        b"fullname" => Some(SystemField::FullName),
        b"path" => Some(SystemField::Path),
        b"extension" => Some(SystemField::Extension),
        b"command" => Some(SystemField::Command),
        b"platform" => Some(SystemField::Platform),
        b"theme" => Some(SystemField::Theme),
        _ => None,
    }
}

#[derive(Default)]
struct RawSystem {
    name: Option<String>,
    fullname: Option<String>,
    path: Option<String>,
    extension: Option<String>,
    command: Option<String>,
    platform: Option<String>,
    theme: Option<String>,
}

impl RawSystem {
    fn set(&mut self, field: SystemField, value: String) {
        let slot = match field {
            SystemField::Name => &mut self.name,
            SystemField::FullName => &mut self.fullname,
            SystemField::Path => &mut self.path,
            SystemField::Extension => &mut self.extension,
            SystemField::Command => &mut self.command,
            SystemField::Platform => &mut self.platform,
            SystemField::Theme => &mut self.theme,
        };
        // First occurrence wins, mirroring `retroarch::parse_config`'s
        // own "first value for a duplicate key" policy - never silently
        // overwritten by a later, possibly-unexpected duplicate element.
        slot.get_or_insert(value);
    }

    fn into_finding(self, tilde_home: &Path) -> EsDeSystemFinding {
        let (rom_path_resolved, rom_path_resolution) =
            resolve_rom_path(self.path.as_deref(), tilde_home);
        EsDeSystemFinding {
            name: self.name,
            fullname: self.fullname,
            rom_path_raw: self.path,
            rom_path_resolved,
            rom_path_resolution,
            extensions: split_whitespace_tokens(
                self.extension.as_deref(),
                MAX_EXTENSIONS_PER_SYSTEM,
            ),
            command: self.command,
            platform_tags: split_comma_tokens(
                self.platform.as_deref(),
                MAX_PLATFORM_TAGS_PER_SYSTEM,
            ),
            theme: self.theme,
        }
    }
}

fn resolve_rom_path(
    raw: Option<&str>,
    tilde_home: &Path,
) -> (Option<EncodedPath>, PathResolutionState) {
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return (None, PathResolutionState::NotConfigured);
    };
    if raw.contains('%') {
        return (None, PathResolutionState::ContainsUnexpandedVariable);
    }
    if let Some(path) = raw.strip_prefix("~/") {
        return (
            Some(EncodedPath::from_path(&tilde_home.join(path))),
            PathResolutionState::Resolved,
        );
    }
    if raw == "~" {
        return (
            Some(EncodedPath::from_path(tilde_home)),
            PathResolutionState::Resolved,
        );
    }
    if raw.starts_with('/') {
        return (
            Some(EncodedPath::from_path(Path::new(raw))),
            PathResolutionState::Resolved,
        );
    }
    (None, PathResolutionState::Unresolved)
}

fn split_whitespace_tokens(raw: Option<&str>, max_tokens: usize) -> Vec<String> {
    raw.map(|value| {
        value
            .split_ascii_whitespace()
            .take(max_tokens)
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

fn split_comma_tokens(raw: Option<&str>, max_tokens: usize) -> Vec<String> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .take(max_tokens)
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

/// Parses an `es_systems.xml`-shaped document into every `<system>` it
/// declares. Never panics: a read/decode error inside `quick_xml` (an
/// unclosed tag, invalid markup, ...) stops the walk and is reported as a
/// diagnostic, but every `<system>` fully parsed before that point is
/// still returned - a malformed file never discards otherwise-good data.
fn parse_systems_xml(
    text: &str,
    source_path: &Path,
    tilde_home: &Path,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<EsDeSystemFinding> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);

    let mut systems = Vec::new();
    let mut current: Option<RawSystem> = None;
    let mut current_field: Option<SystemField> = None;

    loop {
        if systems.len() >= MAX_SYSTEMS_PER_FILE {
            diagnostics.push(Diagnostic::new(
                "systems_file_system_limit_reached",
                DiagnosticSeverity::Warning,
                DiagnosticCategory::SystemsInventory,
                None,
                Some(source_path),
            ));
            break;
        }
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let tag = start.name();
                let tag_bytes = tag.as_ref();
                if tag_bytes == b"system" {
                    current = Some(RawSystem::default());
                    current_field = None;
                } else if current.is_some() {
                    current_field = system_field_for_tag(tag_bytes);
                }
            }
            Ok(Event::Text(text_event)) => {
                if let (Some(system), Some(field)) = (current.as_mut(), current_field) {
                    // Same decode-then-unescape split `dat::parsers::logiqx`
                    // uses for quick-xml 0.41 - an unescape failure (only a
                    // DTD entity could cause one, and no DTD is processed)
                    // keeps the raw decoded text rather than losing the
                    // field silently.
                    if let Ok(decoded) = text_event.decode() {
                        let text = unescape(&decoded)
                            .map(|value| value.into_owned())
                            .unwrap_or_else(|_| decoded.into_owned());
                        let bounded: String = text.chars().take(MAX_FIELD_TEXT_BYTES).collect();
                        if !bounded.is_empty() {
                            system.set(field, bounded);
                        }
                    }
                }
            }
            Ok(Event::End(end)) => {
                let tag_bytes = end.name();
                let tag_bytes = tag_bytes.as_ref();
                if tag_bytes == b"system" {
                    if let Some(system) = current.take() {
                        systems.push(system.into_finding(tilde_home));
                    }
                    current_field = None;
                } else if current_field.is_some() && system_field_for_tag(tag_bytes).is_some() {
                    current_field = None;
                }
            }
            Ok(Event::Eof) => {
                // quick-xml does not itself treat "the document ended
                // while a tag was still open" as a hard error - it just
                // stops producing events. A `<system>` (or one of its
                // child fields) left open at `Eof` is this adapter's own
                // signal that the file was truncated/malformed; the
                // incomplete system is dropped rather than fabricated
                // as if it had been fully read.
                if current.is_some() {
                    diagnostics.push(Diagnostic::new(
                        "systems_file_unclosed_element_at_eof",
                        DiagnosticSeverity::Warning,
                        DiagnosticCategory::ConfigParse,
                        None,
                        Some(source_path),
                    ));
                }
                break;
            }
            Ok(_) => {}
            Err(_) => {
                diagnostics.push(Diagnostic::new(
                    "systems_file_malformed_xml",
                    DiagnosticSeverity::Warning,
                    DiagnosticCategory::ConfigParse,
                    None,
                    Some(source_path),
                ));
                break;
            }
        }
    }

    systems
}

/// Production convenience wrapper around [`discover_es_de_environment`]
/// with [`DiscoveryEnvironment::from_process_environment`] - production
/// code should prefer calling this or constructing its own
/// `DiscoveryEnvironment` for explicit/portable roots.
pub fn discover_es_de_environment_default() -> Result<EsDeEnvironmentReport, DiscoveryError> {
    discover_es_de_environment(
        &HostReadOnlyFilesystem,
        &DiscoveryEnvironment::from_process_environment(),
    )
}

#[cfg(test)]
mod tests;
