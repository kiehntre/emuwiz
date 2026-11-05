//! Explicit, user-triggered per-game arcade compatibility check.
//!
//! This is the missing evidence producer for [`crate::arcade_recommendation`].
//! It gathers nothing on its own initiative: a caller names one machine, its
//! archive, and (optionally) a MAME listxml catalogue and an FBNeo DAT with the
//! installed-emulator facts it holds. The check then
//!
//! 1. parses each named catalogue through the bounded DAT parser,
//! 2. hashes the machine's archive plus the dependency archives its
//!    expectation names, looking **only beside the selected archive**, through
//!    the bounded ZIP source (no extraction, no nested archives, no CHDs),
//! 3. hands the observations to the existing MAME and FBNeo evaluators and the
//!    existing recommendation policy.
//!
//! No compatibility rule lives here. Anything that was not gathered is
//! reported as unknown/partial evidence, never as a mismatch, and a complete
//! ROM set is storage evidence, not proof the machine runs: the report keeps
//! `launch_readiness` out of its vocabulary on purpose.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::arcade_compatibility::{
    ArcadeEmulatorCompatibility, ArcadeEmulatorCompatibilitySummary, evaluate_arcade_compatibility,
};
use crate::arcade_fbneo_compatibility::{
    FbNeoExpectationIndex, FbNeoSetExpectation, InstalledFbNeoEvidence,
};
use crate::arcade_mame_compatibility::{
    InstalledMameEvidence, MameExpectationIndex, MameObservedRom, MameSetExpectation,
    ObservedArcadeEvidenceSource, ObservedArcadeSetEvidence, ObservedEvidenceCompleteness,
};
use crate::arcade_recommendation::{
    ArcadeEmulatorRecommendation, ArcadeRecommendationOptions, ArcadeUserPreference,
    recommend_arcade_emulator,
};
use crate::dat::archive::limits::ArchiveLimits;
use crate::dat::archive::zip::ZipArchiveSource;
use crate::dat::archive::{ArchiveMemberSource, ArchivePassCompletion, ArchiveRunBudget};
use crate::dat::limits::DatLimits;
use crate::dat::model::{DatEcosystem, ParsedDat};
use crate::dat::parsers::mame_listxml::MAME_LISTXML_PARSER_SCHEMA_VERSION;
use crate::dat::parsers::parse_dat_file_bound;
use crate::mame_collection_analyser::parse_version_evidence;
use crate::safe_read::TrustedRoots;

/// Logical bytes the whole check may decode across every archive it opens.
pub const CHECK_RUN_BUDGET_BYTES: u64 = 1 << 30;
/// Most dependency archives one check will open (machine + BIOS/parent/devices).
pub const CHECK_MAX_ARCHIVES: usize = 24;
/// Most mismatch lines carried into the report.
const MAX_DETAIL_LINES: usize = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MameCheckInput {
    /// A MAME `-listxml` catalogue (explicitly chosen by the user).
    pub catalogue_path: PathBuf,
    /// Captured `mame -version` output. Never guessed; empty means unknown.
    pub version_output: String,
    /// Executable the version output is claimed to come from (provenance
    /// only; not executed).
    pub executable_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FbNeoCheckInput {
    /// A standalone-FBNeo-specific DAT (explicitly chosen by the user).
    pub dat_path: PathBuf,
    /// The installed FBNeo core or executable the DAT is claimed to match.
    pub core_path: Option<PathBuf>,
    pub reported_version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArcadeCheckRequest {
    pub set_name: String,
    pub archive_path: PathBuf,
    pub mame: Option<MameCheckInput>,
    pub fbneo: Option<FbNeoCheckInput>,
    /// The user's existing emulator preference, if any. It can only break a
    /// tie between two proven emulators.
    pub user_preference: Option<ArcadeUserPreference>,
}

/// Size and mtime of one input; a re-stat that differs invalidates the report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FileStamp {
    pub path: PathBuf,
    pub len: u64,
    pub modified_ns: Option<i128>,
}

impl FileStamp {
    pub fn observe(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        let modified_ns = meta.modified().ok().and_then(|time| {
            time.duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_nanos() as i128)
        });
        Ok(Self {
            path: path.to_path_buf(),
            len: meta.len(),
            modified_ns,
        })
    }

    pub fn still_matches(&self) -> bool {
        Self::observe(&self.path).is_ok_and(|now| now == *self)
    }
}

/// Everything the result depends on. Freshness is a stat-only comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CheckProvenance {
    pub archives: Vec<FileStamp>,
    /// Closure sets whose archive was not found beside the selected one.
    pub missing_archives: Vec<String>,
    pub mame_catalogue: Option<(FileStamp, String)>,
    pub fbneo_dat: Option<(FileStamp, String)>,
    /// Files named as emulator provenance (stat only; may be absent).
    pub emulator_files: Vec<FileStamp>,
    pub mame_version_output: Option<String>,
    pub fbneo_version: Option<String>,
    pub checked_at_unix: u64,
}

impl CheckProvenance {
    /// True while every stamped input is unchanged. Never reads file content.
    pub fn is_current(&self) -> bool {
        self.archives.iter().all(FileStamp::still_matches)
            && self
                .mame_catalogue
                .iter()
                .chain(self.fbneo_dat.iter())
                .all(|(stamp, _)| stamp.still_matches())
            && self.emulator_files.iter().all(FileStamp::still_matches)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArcadeCheckReport {
    pub set_name: String,
    pub summary: ArcadeEmulatorCompatibilitySummary,
    pub recommendation: ArcadeEmulatorRecommendation,
    pub observed_completeness: ObservedEvidenceCompleteness,
    /// Plain-language limits of this check, e.g. unchecked CHDs.
    pub limits: Vec<String>,
    /// Bounded mismatch / evidence lines from the two evaluators.
    pub details: Vec<String>,
    pub provenance: CheckProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArcadeCheckError {
    InvalidSetName,
    NothingToCheck,
    Cancelled,
    Archive(String),
    Catalogue(String),
}

impl std::fmt::Display for ArcadeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSetName => f.write_str("The machine name is not a safe set name."),
            Self::NothingToCheck => {
                f.write_str("Choose a MAME catalogue or an FBNeo DAT to check against.")
            }
            Self::Cancelled => f.write_str("The check was cancelled."),
            Self::Archive(detail) | Self::Catalogue(detail) => f.write_str(detail),
        }
    }
}

/// A set name becomes a file name beside the archive, so it must not carry any
/// path structure.
pub fn is_safe_set_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn dat_limits() -> DatLimits {
    DatLimits {
        max_entries: 100_000,
        max_roms_per_entry: 16_384,
        ..DatLimits::default()
    }
}

fn load_catalogue(
    path: &Path,
    ecosystem: DatEcosystem,
    what: &str,
) -> Result<(ParsedDat, String, FileStamp), ArcadeCheckError> {
    let before = FileStamp::observe(path)
        .map_err(|_| ArcadeCheckError::Catalogue(format!("The {what} could not be read.")))?;
    let (outcome, digest) = parse_dat_file_bound(path, dat_limits())
        .map_err(|error| ArcadeCheckError::Catalogue(format!("The {what} was refused: {error}")))?;
    if !before.still_matches() {
        return Err(ArcadeCheckError::Catalogue(format!(
            "The {what} changed while it was being read."
        )));
    }
    if outcome.dat.source.ecosystem != ecosystem {
        return Err(ArcadeCheckError::Catalogue(format!(
            "The selected {what} is not a {ecosystem:?} catalogue."
        )));
    }
    Ok((outcome.dat, digest, before))
}

fn installed_mame(
    input: &MameCheckInput,
    dat: &ParsedDat,
    digest: &str,
) -> (InstalledMameEvidence, Vec<String>) {
    let mut notes = Vec::new();
    let text = input.version_output.trim();
    let product_marker = text
        .split_ascii_whitespace()
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("mame"));
    let installed = product_marker
        .then(|| parse_version_evidence(text))
        .flatten();
    let catalogue = dat
        .source
        .version
        .as_deref()
        .and_then(parse_version_evidence);
    let version_text = match (&installed, &catalogue) {
        (Some(a), Some(b)) if a == b => text.to_string(),
        (Some(_), Some(_)) => {
            notes.push("The captured MAME version differs from the catalogue version, so the catalogue is not evidence for that MAME.".into());
            String::new()
        }
        (None, _) => {
            notes.push(
                "No usable captured MAME version output; MAME evidence stays unknown.".into(),
            );
            String::new()
        }
        (_, None) => {
            notes.push(
                "The MAME catalogue declares no supported version; MAME evidence stays unknown."
                    .into(),
            );
            String::new()
        }
    };
    let mut evidence = InstalledMameEvidence::new(
        input.executable_path.clone().unwrap_or_default(),
        version_text,
        input.catalogue_path.clone(),
        std::iter::empty(),
        Some(digest.to_string()),
        None,
    );
    if !dat.games.is_empty()
        && dat
            .games
            .iter()
            .all(|g| g.mame_listxml_schema_version == Some(MAME_LISTXML_PARSER_SCHEMA_VERSION))
    {
        evidence.parser_schema_version = MAME_LISTXML_PARSER_SCHEMA_VERSION;
    }
    (evidence, notes)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Run one explicit check. Blocking; call from a worker thread and set
/// `cancel` to stop it between archives and members.
pub fn run_arcade_compat_check(
    request: &ArcadeCheckRequest,
    cancel: &AtomicBool,
) -> Result<ArcadeCheckReport, ArcadeCheckError> {
    if !is_safe_set_name(&request.set_name) {
        return Err(ArcadeCheckError::InvalidSetName);
    }
    if request.mame.is_none() && request.fbneo.is_none() {
        return Err(ArcadeCheckError::NothingToCheck);
    }
    let check_cancel = || {
        if cancel.load(Ordering::Relaxed) {
            Err(ArcadeCheckError::Cancelled)
        } else {
            Ok(())
        }
    };

    let mut limits = Vec::new();
    let mut emulator_files = Vec::new();
    let mut mame_loaded = None;
    let mut fbneo_loaded = None;
    if let Some(input) = &request.mame {
        mame_loaded = Some(load_catalogue(
            &input.catalogue_path,
            DatEcosystem::MAMEArcade,
            "MAME catalogue",
        )?);
        emulator_files.extend(
            input
                .executable_path
                .iter()
                .filter_map(|p| FileStamp::observe(p).ok()),
        );
    }
    if let Some(input) = &request.fbneo {
        fbneo_loaded = Some(load_catalogue(
            &input.dat_path,
            DatEcosystem::FBNeo,
            "FBNeo DAT",
        )?);
        emulator_files.extend(
            input
                .core_path
                .iter()
                .filter_map(|p| FileStamp::observe(p).ok()),
        );
    }
    check_cancel()?;

    let mame_index = mame_loaded
        .as_ref()
        .map(|(dat, _, _)| MameExpectationIndex::new(dat));
    let mame_expectation: Option<MameSetExpectation> = mame_index
        .as_ref()
        .and_then(|index| index.expectation_for(&request.set_name));
    let fbneo_expectation: Option<FbNeoSetExpectation> = fbneo_loaded
        .as_ref()
        .and_then(|(dat, _, _)| FbNeoExpectationIndex::new(dat).expectation_for(&request.set_name));

    // The sets whose archives must be observed: the machine itself plus every
    // dependency either expectation names.
    let mut closure = BTreeSet::from([request.set_name.clone()]);
    let mut has_disks = false;
    if let Some(e) = &mame_expectation {
        closure.extend(e.dependencies.closure_set_names.iter().cloned());
        has_disks |= !e.disks.is_empty();
    }
    if let Some(e) = &fbneo_expectation {
        closure.extend(e.dependencies.closure_set_names.iter().cloned());
        has_disks |= !e.disks.is_empty();
    }
    if closure.len() > CHECK_MAX_ARCHIVES {
        return Err(ArcadeCheckError::Archive(format!(
            "This machine depends on {} sets; the check opens at most {CHECK_MAX_ARCHIVES}.",
            closure.len()
        )));
    }
    // A dependency whose name is not a safe file name is never opened; it
    // counts as unobserved, so the evidence stays partial.
    let mut missing_archives = Vec::new();
    let mut all_complete = true;
    closure.retain(|name| {
        let safe = is_safe_set_name(name);
        if !safe {
            missing_archives.push(name.clone());
            all_complete = false;
        }
        safe
    });

    // Observe archives. The selected archive is used as given for the machine;
    // dependencies are `<set>.zip` beside it, never searched for elsewhere.
    let dir = request
        .archive_path
        .parent()
        .ok_or_else(|| ArcadeCheckError::Archive("The archive has no folder.".into()))?;
    let mut budget = ArchiveRunBudget::new(CHECK_RUN_BUDGET_BYTES);
    let mut stamps = Vec::new();
    let mut observed_roms: Vec<MameObservedRom> = Vec::new();
    let mut sources = Vec::new();
    let mut observed_names = Vec::new();
    for name in &closure {
        check_cancel()?;
        let path = if *name == request.set_name {
            request.archive_path.clone()
        } else {
            dir.join(format!("{name}.zip"))
        };
        let is_regular = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
        if !is_regular {
            if *name == request.set_name {
                return Err(ArcadeCheckError::Archive(
                    "The selected game's file is not a regular file.".into(),
                ));
            }
            missing_archives.push(name.clone());
            all_complete = false;
            continue;
        }
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            return Err(ArcadeCheckError::Archive(
                "Only ZIP arcade sets can be checked; this game's archive is not a ZIP.".into(),
            ));
        }
        let stamp = FileStamp::observe(&path).map_err(|_| {
            ArcadeCheckError::Archive(format!("{} could not be read.", path.display()))
        })?;
        let mut source = ZipArchiveSource::open(
            &path,
            &TrustedRoots::none(),
            ArchiveLimits::default(),
            cancel,
        )
        .map_err(|error| ArcadeCheckError::Archive(format!("{}: {error:?}", path.display())))?;
        let outcome = source.verify_all(cancel, &mut budget);
        check_cancel()?;
        if !stamp.still_matches() {
            return Err(ArcadeCheckError::Archive(format!(
                "{} changed while it was being checked.",
                path.display()
            )));
        }
        let pass = ObservedArcadeSetEvidence::from_archive_pass(
            name.clone(),
            Some(path.clone()),
            path.file_name().map(PathBuf::from),
            None,
            &outcome.members,
            &outcome.completion,
            [name.clone()],
        );
        if pass.completeness != ObservedEvidenceCompleteness::Complete
            || outcome.completion != ArchivePassCompletion::Complete
        {
            all_complete = false;
        }
        observed_roms.extend(pass.observed_roms);
        sources.extend(pass.sources);
        observed_names.push(name.clone());
        stamps.push(stamp);
    }
    if !missing_archives.is_empty() {
        limits.push(format!(
            "Not found beside the selected archive: {}. Missing support sets are unknown here, not a failure.",
            missing_archives.join(", ")
        ));
    }
    if has_disks {
        all_complete = false;
        limits.push("This machine uses CHD disk images, which this check does not read. Compatibility cannot be proven.".into());
    }
    limits.push("File checks cover ROM members only; a complete set is storage evidence, not proof the game runs.".into());

    let mut observed = ObservedArcadeSetEvidence::new(
        request.set_name.clone(),
        if all_complete {
            ObservedEvidenceCompleteness::Complete
        } else {
            ObservedEvidenceCompleteness::Partial
        },
    );
    observed.observed_set_names = observed_names;
    observed.observed_roms = observed_roms;
    observed.sources = sources
        .into_iter()
        .map(|s: ObservedArcadeEvidenceSource| s)
        .collect();

    // Emulator evidence. An emulator the caller did not describe is Unknown
    // (not "not installed"): the evaluators receive unusable evidence.
    let mut notes = Vec::new();
    let (mame_evidence, mame_exp) = match (&request.mame, &mame_loaded) {
        (Some(input), Some((dat, digest, _))) => {
            let (evidence, mut n) = installed_mame(input, dat, digest);
            notes.append(&mut n);
            (evidence, mame_expectation)
        }
        _ => (
            InstalledMameEvidence::new(
                PathBuf::new(),
                "",
                PathBuf::new(),
                std::iter::empty(),
                None,
                None,
            ),
            None,
        ),
    };
    let fbneo_evidence = match (&request.fbneo, &fbneo_loaded) {
        (Some(input), Some((dat, digest, _))) => {
            match input.core_path.as_ref().filter(|p| p.is_file()) {
                Some(core) => InstalledFbNeoEvidence::new(
                    core.clone(),
                    input.reported_version.clone(),
                    None,
                    input.dat_path.clone(),
                    dat.source.version.clone(),
                    Some(digest.clone()),
                    None,
                ),
                None => {
                    notes.push("No existing FBNeo core or executable was named; FBNeo evidence stays unknown.".into());
                    InstalledFbNeoEvidence::unknown()
                }
            }
        }
        _ => InstalledFbNeoEvidence::unknown(),
    };
    let summary = evaluate_arcade_compatibility(
        &observed,
        Some((&mame_evidence, mame_exp)),
        Some((&fbneo_evidence, fbneo_expectation)),
    );
    // Availability is deliberately left to the evidence: the caller never
    // claims an emulator is installed or missing.
    let recommendation = recommend_arcade_emulator(
        &summary,
        ArcadeRecommendationOptions {
            user_preference: request.user_preference,
            mame_available: None,
            fbneo_available: None,
        },
    );

    let mut details = notes;
    if let ArcadeEmulatorCompatibility::Evaluated(result) = &summary.mame {
        details.extend(
            result
                .mismatches
                .iter()
                .take(MAX_DETAIL_LINES)
                .map(|m| format!("MAME: {}", m.detail)),
        );
    }
    if let ArcadeEmulatorCompatibility::Evaluated(result) = &summary.fbneo {
        details.extend(
            result
                .mismatches
                .iter()
                .take(MAX_DETAIL_LINES)
                .map(|m| format!("FBNeo: {}", m.detail)),
        );
    }
    details.truncate(2 * MAX_DETAIL_LINES);

    Ok(ArcadeCheckReport {
        set_name: request.set_name.clone(),
        summary,
        recommendation,
        observed_completeness: observed.completeness,
        limits,
        details,
        provenance: CheckProvenance {
            archives: stamps,
            missing_archives,
            mame_catalogue: mame_loaded.map(|(_, d, s)| (s, d)),
            fbneo_dat: fbneo_loaded.map(|(_, d, s)| (s, d)),
            emulator_files,
            mame_version_output: request
                .mame
                .as_ref()
                .map(|m| m.version_output.trim().to_string()),
            fbneo_version: request
                .fbneo
                .as_ref()
                .and_then(|f| f.reported_version.clone()),
            checked_at_unix: now_unix(),
        },
    })
}
