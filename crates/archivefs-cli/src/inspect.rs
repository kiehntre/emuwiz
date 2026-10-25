//! `inspect <path>`: one read-only view over analysis EmuWiz already has.
//!
//! This composes four existing, individually read-only reports for a single
//! supplied path and adds no identity model of its own:
//!
//! * platform detection ([`PlatformDetectionReport`]),
//! * catalogued game identity ([`GameIdentityReport`]),
//! * media-set topology (`media_set::inspect_paths` + `resolve_index`),
//! * the pure launch-topology projection (`project_media_set_for_launch`).
//!
//! Nothing here opens the database, the catalogue or the network, spawns a
//! process, discovers emulators, writes a file or follows an unreviewed
//! symlink. Filename-derived evidence is labelled as such and is never
//! reported as a verified fact. Emulator and firmware readiness need an
//! emulator selection that a path alone cannot supply, so they are reported
//! as not evaluated rather than guessed.

use std::fs;
use std::path::{Path, PathBuf};

use archivefs_core::game_identity::IdentityPlatform;
use archivefs_core::launch::project_media_set_for_launch;
use archivefs_core::media_set::{
    InspectionLimits, MediaSet, TopologyReport, index_media, inspect_paths, resolve_index,
};
use serde::Serialize;

use super::{
    Config, DetectionConfidence, DetectionRequest, GameIdentityReport, IdentityConfidence,
    IdentityKind, IdentityStatus, PlatformDetectionReport, TrustedRoots, detect_platform_report,
    format_platform_detection, inspect_catalogued_game_identity_in_roots,
};

const USAGE: &str = "usage: inspect <path> [--identity] [--evidence] [--readiness] [--media] [--provenance] [--platform NAME] [--root DIR] [--json]";

#[derive(Clone, Copy, Default)]
struct Views {
    identity: bool,
    evidence: bool,
    readiness: bool,
    media: bool,
    provenance: bool,
}

impl Views {
    fn all() -> Self {
        Self {
            identity: true,
            evidence: true,
            readiness: true,
            media: true,
            provenance: true,
        }
    }
    fn any(self) -> bool {
        self.identity || self.evidence || self.readiness || self.media || self.provenance
    }
}

struct Options {
    path: PathBuf,
    views: Views,
    json: bool,
    platform: Option<String>,
    root: Option<PathBuf>,
}

fn parse(args: Vec<String>) -> Result<Options, String> {
    let mut path = None;
    let mut views = Views::default();
    let (mut json, mut platform, mut root) = (false, None, None);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--identity" => views.identity = true,
            "--evidence" => views.evidence = true,
            "--readiness" => views.readiness = true,
            "--media" => views.media = true,
            "--provenance" => views.provenance = true,
            "--platform" => {
                platform = Some(args.next().ok_or("--platform requires a platform name")?)
            }
            "--root" => {
                root = Some(PathBuf::from(
                    args.next().ok_or("--root requires a directory")?,
                ))
            }
            "--" => {
                let rest: Vec<_> = args.by_ref().collect();
                if rest.len() != 1 || path.is_some() {
                    return Err(format!("exactly one path is required; {USAGE}"));
                }
                path = rest.into_iter().next().map(PathBuf::from);
            }
            other if other.starts_with('-') => {
                return Err(format!("unsupported option {other:?}; {USAGE}"));
            }
            other => {
                if path.replace(PathBuf::from(other)).is_some() {
                    return Err(format!(
                        "exactly one path is required (quote paths containing spaces); {USAGE}"
                    ));
                }
            }
        }
    }
    Ok(Options {
        path: path.ok_or_else(|| format!("inspect requires a path; {USAGE}"))?,
        views: if views.any() { views } else { Views::all() },
        json,
        platform,
        root,
    })
}

#[derive(Serialize)]
struct Target {
    path: String,
    path_is_utf8: bool,
    kind: &'static str,
    size_bytes: Option<u64>,
}

#[derive(Serialize)]
struct PlatformSection<'a> {
    #[serde(flatten)]
    report: &'a PlatformDetectionReport,
}

#[derive(Serialize)]
struct IdentityFact {
    kind: String,
    /// `identity`, `checksum`, `attribute` or `platform_context`.
    category: &'static str,
    value: Option<String>,
    status: String,
    /// `verified_fact`, `catalogue_context`, `candidate`,
    /// `filename_inference`, `ambiguous`, `invalid`, `incomplete` or
    /// `unsupported_or_unknown`. Only `verified_fact` (read from the bytes)
    /// is a verified identity.
    class: &'static str,
}

#[derive(Serialize)]
struct EvidenceItem {
    kind: String,
    status: String,
    confidence: String,
    diagnostic: String,
}

#[derive(Serialize)]
struct ProvenanceItem {
    kind: String,
    method: String,
    member_index: Option<usize>,
}

#[derive(Serialize)]
struct IdentitySection {
    /// `false` when identity inspection was deliberately skipped (see
    /// `needs_external_detector`); the facts are then empty, not "unknown".
    evaluated: bool,
    not_evaluated_reason: Option<&'static str>,
    platform_hint: Option<String>,
    platform_hint_source: &'static str,
    identity_platform: String,
    format: String,
    /// A content-derived identity fact (serial, title/product ID, ...) was
    /// verified. A checksum alone is not an identity.
    verified_identity: bool,
    /// A content checksum was computed and verified; matching it to a game
    /// needs DAT/catalogue evidence that `inspect` never consults.
    verified_checksum: bool,
    facts: Vec<IdentityFact>,
    warnings: Vec<String>,
    complete: bool,
    bytes_read: u64,
}

#[derive(Serialize)]
struct EvidenceSection {
    platform: Vec<serde_json::Value>,
    platform_candidates: Vec<serde_json::Value>,
    identity: Vec<EvidenceItem>,
}

#[derive(Serialize)]
struct ProvenanceSection {
    platform_deciding_source: Option<serde_json::Value>,
    platform_manually_assigned: bool,
    platform_evidence_sources: Vec<serde_json::Value>,
    identity: Vec<ProvenanceItem>,
    not_consulted: &'static str,
}

#[derive(Serialize)]
struct Readiness {
    media_set_id: String,
    topology_state: serde_json::Value,
    action_safety: serde_json::Value,
    explanation: String,
    missing_media: Vec<String>,
    conflicts: Vec<String>,
    start_media: Option<serde_json::Value>,
    media_sequence: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct ReadinessSection {
    media_sets: Vec<Readiness>,
    note: &'static str,
    emulator_and_firmware: &'static str,
}

#[derive(Serialize)]
struct MediaSection {
    records: usize,
    truncated_to_max_files: Option<usize>,
    topology: TopologyReport,
}

#[derive(Serialize)]
struct Blocker {
    /// `not_recognised`, `ambiguous`, `insufficient_evidence`, `unsupported`,
    /// `media_set_blocked`, `review_required` or `incomplete`.
    reason: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct InspectReport<'a> {
    read_only: bool,
    database: &'static str,
    target: Target,
    source_root: String,
    /// `recognised_verified`, `recognised_unverified`, `ambiguous` or
    /// `not_recognised`.
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<PlatformSection<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: Option<IdentitySection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<EvidenceSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<ProvenanceSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    readiness: Option<ReadinessSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media: Option<MediaSection>,
    blockers: Vec<Blocker>,
}

fn value<T: Serialize>(item: &T) -> serde_json::Value {
    serde_json::to_value(item).unwrap_or(serde_json::Value::Null)
}

fn name<T: Serialize>(item: &T) -> String {
    match value(item) {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    }
}

/// How far a piece of identity evidence may be trusted. Only facts read from
/// the bytes (exact or structured metadata) are `verified_fact`. Filename
/// evidence is `filename_inference` and catalogue/hint context is
/// `catalogue_context`, whatever their status says: a platform *hint* echoed
/// back as "Verified" is not evidence about the file.
fn class(status: IdentityStatus, confidence: IdentityConfidence) -> &'static str {
    match (status, confidence) {
        (_, IdentityConfidence::FilenameOnly) => "filename_inference",
        (_, IdentityConfidence::CatalogueContext) => "catalogue_context",
        (IdentityStatus::Verified, _) => "verified_fact",
        (IdentityStatus::Candidate, _) => "candidate",
        (IdentityStatus::Ambiguous, _) => "ambiguous",
        (IdentityStatus::Invalid, _) => "invalid",
        (IdentityStatus::ResourceLimitReached, _) => "incomplete",
        _ => "unsupported_or_unknown",
    }
}

/// What a piece of identity evidence is *about*. Exhaustive on purpose: a new
/// `IdentityKind` must be classified here before this view can compile, so a
/// checksum or attribute can never silently become a "game identity".
fn category(kind: IdentityKind) -> &'static str {
    use IdentityKind::*;
    match kind {
        Platform => "platform_context",
        AmigaWHDLoad | Ps1Serial | Ps2Serial | PspDiscId | Ps3TitleId | Ps4TitleId
        | Ps4ContentId | SaturnProductNumber | DreamcastProductCode | SegaCdProductCode
        | Pcsx2ExecutableCrc | DolphinGameId | MameMachineName | XbeTitleId | XexTitleId
        | XexMediaId | ScummVmGameId | ThreeDoDiscId | ThreeDsTitleId | ThreeDsProductCode => {
            "identity"
        }
        LooseRomSha256 | LooseRomCanonicalSha256 | PcfxDiscHash => "checksum",
        DolphinRevision
        | DolphinDiscNumber
        | DolphinRegion
        | LooseRomFormat
        | LooseRomTitle
        | N64Cic
        | N64CrcValidation
        | PceCdBootStructure
        | NeoGeoCdBootStructure
        | NesHeader
        | SnesHeader
        | TapeFormat
        | T64Directory
        | ThreeDsEncryption
        | ThreeDsClassification
        | ThreeDsPartition => "attribute",
    }
}

/// Refuse anything that is not a regular file or directory before any
/// analysis opens it: opening a FIFO or device can block or have effects.
fn target_facts(path: &Path) -> Result<Target, String> {
    let link = fs::symlink_metadata(path)
        .map_err(|error| format!("not found or not accessible: {}: {error}", path.display()))?;
    let resolved = fs::metadata(path)
        .map_err(|error| format!("not found or not accessible: {}: {error}", path.display()))?;
    let kind = |meta: &fs::Metadata| {
        if meta.is_file() {
            "file"
        } else if meta.is_dir() {
            "directory"
        } else {
            "special"
        }
    };
    if kind(&resolved) == "special" {
        return Err(format!(
            "unsafe/refused: {} is a special file (device, FIFO or socket) and is never opened",
            path.display()
        ));
    }
    Ok(Target {
        path: path.to_string_lossy().into_owned(),
        path_is_utf8: path.to_str().is_some(),
        kind: if link.file_type().is_symlink() {
            "symlink"
        } else {
            kind(&resolved)
        },
        size_bytes: resolved.is_file().then(|| resolved.len()),
    })
}

fn readiness(set: &MediaSet) -> Readiness {
    let projection = project_media_set_for_launch(set, None);
    Readiness {
        media_set_id: projection.media_set_id,
        topology_state: value(&projection.topology_state),
        action_safety: value(&projection.action_safety),
        explanation: projection.explanation,
        missing_media: projection
            .missing_media
            .into_iter()
            .map(|missing| missing.detail)
            .collect(),
        conflicts: projection.conflicts,
        start_media: projection.start_media.as_ref().map(value),
        media_sequence: projection.media_sequence.iter().map(value).collect(),
    }
}

/// The core identity inspector asks the locally installed ScummVM executable
/// to detect a game *directory* (a subprocess that also writes a temporary
/// config). `inspect` never spawns processes, so that one path is not taken:
/// identity (and the media record that reuses it) is skipped and said so.
fn needs_external_detector(hint: Option<&str>, target: &Target) -> bool {
    target.kind != "file" && IdentityPlatform::from_catalogue(hint) == IdentityPlatform::ScummVM
}

fn skipped_identity(hint: &Option<String>, source: &'static str) -> IdentitySection {
    IdentitySection {
        evaluated: false,
        not_evaluated_reason: Some(
            "ScummVM directory identity requires running the ScummVM detector executable, which inspect never does",
        ),
        platform_hint: hint.clone(),
        platform_hint_source: source,
        identity_platform: "not evaluated".into(),
        format: "not evaluated".into(),
        verified_identity: false,
        verified_checksum: false,
        facts: Vec::new(),
        warnings: Vec::new(),
        complete: false,
        bytes_read: 0,
    }
}

fn identity_section(
    report: &GameIdentityReport,
    hint: &Option<String>,
    source: &'static str,
) -> IdentitySection {
    let facts: Vec<IdentityFact> = report
        .evidence
        .iter()
        .map(|item| IdentityFact {
            kind: item.kind.to_string(),
            category: category(item.kind),
            value: item.value.clone(),
            status: item.status.to_string(),
            class: class(item.status, item.confidence),
        })
        .collect();
    IdentitySection {
        evaluated: true,
        not_evaluated_reason: None,
        platform_hint: hint.clone(),
        platform_hint_source: source,
        identity_platform: name(&report.platform),
        format: name(&report.format),
        // The platform entry only restates the supplied hint and a checksum
        // is not a name: a verified identity needs a content-derived fact
        // about the game itself.
        verified_identity: report.evidence.iter().any(|item| {
            category(item.kind) == "identity"
                && class(item.status, item.confidence) == "verified_fact"
        }),
        verified_checksum: report.evidence.iter().any(|item| {
            category(item.kind) == "checksum"
                && class(item.status, item.confidence) == "verified_fact"
        }),
        facts,
        warnings: report.warnings.clone(),
        complete: report.complete,
        bytes_read: report.bytes_read,
    }
}

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut options = parse(args)?;
    // Absolute without touching the filesystem or resolving symlinks, so the
    // default source root is meaningful for relative paths.
    options.path = std::path::absolute(&options.path)?;
    let target = target_facts(&options.path)?;
    let config = Config::load_default().ok();
    let trusted = config
        .as_ref()
        .map(TrustedRoots::from_config)
        .unwrap_or_else(|| {
            TrustedRoots::from_paths([options.path.parent().unwrap_or(Path::new("/"))])
        });
    // Same boundary a real scan would use: the configured source folder that
    // contains the path, else the path's own directory. Nothing is created.
    let root = options.root.clone().unwrap_or_else(|| {
        config
            .as_ref()
            .and_then(|config| {
                config
                    .source_folders
                    .iter()
                    .find(|folder| options.path.starts_with(folder))
                    .cloned()
            })
            .or_else(|| options.path.parent().map(Path::to_path_buf))
            .unwrap_or_default()
    });

    let platform = detect_platform_report(
        &DetectionRequest::new(&options.path, &root)
            .inspecting_content()
            .with_trusted_roots(trusted.clone()),
    );

    // The identity inspector takes a platform hint. A user's explicit choice
    // wins; otherwise only a *confirmed* detection is passed on, and the
    // report says which one was used.
    let (hint, hint_source) = match (&options.platform, platform.confidence, platform.platform) {
        (Some(user), _, _) => (Some(user.clone()), "user"),
        (None, DetectionConfidence::Confirmed, Some(detected)) => {
            (Some(detected.to_string()), "detected_confirmed")
        }
        _ => (None, "none"),
    };
    let skip_identity = needs_external_detector(hint.as_deref(), &target);
    let identity = (!skip_identity).then(|| {
        inspect_catalogued_game_identity_in_roots(&options.path, hint.as_deref(), &trusted)
    });
    let identity_view = match &identity {
        Some(report) => identity_section(report, &hint, hint_source),
        None => skipped_identity(&hint, hint_source),
    };
    let identity_evidence: &[_] = identity.as_ref().map_or(&[], |report| &report.evidence);

    let mut limits = InspectionLimits::default();
    if target.kind == "directory" {
        limits.max_files = limits.max_files.min(64);
    }
    let records = inspect_paths(
        std::slice::from_ref(&options.path),
        // The media record re-runs identity inspection with the same hint.
        hint.as_deref().filter(|_| !skip_identity),
        &trusted,
        &limits,
    );
    let record_count = records.len();
    let topology = resolve_index(index_media(records));
    let readiness_view = ReadinessSection {
        media_sets: topology.sets.iter().map(readiness).collect(),
        note: "topology readiness covers disc, floppy and tape media sets; a representation the topology engine does not model (for example a cartridge ROM) is reported UNSUPPORTED, which is not a statement about emulator support",
        emulator_and_firmware: "not evaluated: emulator and firmware readiness need an emulator selection that a path alone cannot supply, and inspect never discovers, probes or launches emulators",
    };

    let mut blockers = Vec::new();
    let outcome = match platform.confidence {
        DetectionConfidence::Confirmed | DetectionConfidence::Probable => {
            if identity_view.verified_identity {
                "recognised_verified"
            } else {
                blockers.push(Blocker {
                    reason: "insufficient_evidence",
                    detail: if identity_view.verified_checksum {
                        "a content checksum is verified but no game identity is: matching it to a game needs DAT/catalogue evidence, which inspect does not consult".into()
                    } else {
                        "no verified identity fact was found; platform detection alone is not a verified game identity".into()
                    },
                });
                "recognised_unverified"
            }
        }
        DetectionConfidence::Ambiguous => {
            blockers.push(Blocker {
                reason: "ambiguous",
                detail: platform
                    .ambiguity_reason
                    .clone()
                    .unwrap_or_else(|| "several platforms fit the evidence".into()),
            });
            "ambiguous"
        }
        _ => {
            blockers.push(Blocker {
                reason: "not_recognised",
                detail: "no usable platform evidence was found".into(),
            });
            "not_recognised"
        }
    };
    for fact in &identity_view.facts {
        let reason = match fact.class {
            "ambiguous" => Some("ambiguous"),
            "invalid" => Some("unsupported"),
            "incomplete" => Some("incomplete"),
            _ => None,
        };
        if let Some(reason) = reason {
            blockers.push(Blocker {
                reason,
                detail: format!("{}: {}", fact.kind, fact.status),
            });
        }
    }
    for set in &readiness_view.media_sets {
        let reason = match (set.action_safety.as_str(), set.topology_state.as_str()) {
            (Some("BLOCKED"), Some("UNSUPPORTED_SET")) => Some("unsupported"),
            (Some("BLOCKED"), _) => Some("media_set_blocked"),
            (Some("REVIEW_REQUIRED"), _) => Some("review_required"),
            _ => None,
        };
        if let Some(reason) = reason {
            blockers.push(Blocker {
                reason,
                detail: set.explanation.clone(),
            });
        }
    }

    let views = options.views;
    let report = InspectReport {
        read_only: true,
        database: "not opened",
        target,
        source_root: root.to_string_lossy().into_owned(),
        outcome,
        platform: (views.identity || views.evidence).then_some(PlatformSection { report: &platform }),
        identity: views.identity.then_some(identity_view),
        evidence: views.evidence.then(|| EvidenceSection {
            platform: platform.evidence.iter().map(value).collect(),
            platform_candidates: platform.candidates.iter().map(value).collect(),
            identity: identity_evidence
                .iter()
                .map(|item| EvidenceItem {
                    kind: item.kind.to_string(),
                    status: item.status.to_string(),
                    confidence: name(&item.confidence),
                    diagnostic: item.diagnostic.clone(),
                })
                .collect(),
        }),
        provenance: views.provenance.then(|| ProvenanceSection {
            platform_deciding_source: platform.deciding_source.as_ref().map(value),
            platform_manually_assigned: platform.manually_assigned,
            platform_evidence_sources: platform
                .evidence
                .iter()
                .map(|item| value(&item.source))
                .collect(),
            identity: identity_evidence
                .iter()
                .map(|item| ProvenanceItem {
                    kind: item.kind.to_string(),
                    method: item.provenance.method.clone(),
                    member_index: item.provenance.member_index,
                })
                .collect(),
            not_consulted: "user review decisions, catalogue/database records and provider data are not consulted: inspect never opens the database",
        }),
        readiness: views.readiness.then_some(readiness_view),
        media: views.media.then(|| MediaSection {
            records: record_count,
            truncated_to_max_files: (record_count >= limits.max_files).then_some(limits.max_files),
            topology,
        }),
        blockers,
    };

    if options.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_text(&report, &platform, &root));
    }
    Ok(())
}

fn render_text(
    report: &InspectReport<'_>,
    platform: &PlatformDetectionReport,
    root: &Path,
) -> String {
    let mut out = format!(
        "EmuWiz Inspect (read-only; database {})\n\nTarget: {} ({}{})\nOutcome: {}\n",
        report.database,
        report.target.path,
        report.target.kind,
        report
            .target
            .size_bytes
            .map(|size| format!(", {size} bytes"))
            .unwrap_or_default(),
        report.outcome
    );
    if report.platform.is_some() {
        out.push_str("\nPlatform detection\n");
        out.push_str(&format_platform_detection(
            Path::new(&report.target.path),
            root,
            platform,
        ));
    }
    if let Some(identity) = &report.identity {
        out.push_str(&format!(
            "\nIdentity (platform hint: {} from {}; verified identity: {}; verified checksum: {})\n",
            identity.platform_hint.as_deref().unwrap_or("none"),
            identity.platform_hint_source,
            if identity.verified_identity { "yes" } else { "no" },
            if identity.verified_checksum { "yes" } else { "no" }
        ));
        for fact in &identity.facts {
            out.push_str(&format!(
                "  [{}] {} {} = {} ({})\n",
                fact.class,
                fact.category,
                fact.kind,
                fact.value.as_deref().unwrap_or("-"),
                fact.status
            ));
        }
        if identity.facts.is_empty() {
            out.push_str("  no identity evidence\n");
        }
        for warning in &identity.warnings {
            out.push_str(&format!("  warning: {warning}\n"));
        }
    }
    if let Some(evidence) = &report.evidence {
        out.push_str("\nIdentity evidence\n");
        for item in &evidence.identity {
            out.push_str(&format!(
                "  {} [{} / {}] {}\n",
                item.kind, item.status, item.confidence, item.diagnostic
            ));
        }
    }
    if let Some(provenance) = &report.provenance {
        out.push_str("\nProvenance\n");
        for item in &provenance.identity {
            out.push_str(&format!(
                "  {} <- {}{}\n",
                item.kind,
                item.method,
                item.member_index
                    .map(|index| format!(" (archive member {index})"))
                    .unwrap_or_default()
            ));
        }
        out.push_str(&format!("  {}\n", provenance.not_consulted));
    }
    if let Some(readiness) = &report.readiness {
        out.push_str("\nLaunch readiness (media topology)\n");
        for (index, set) in readiness.media_sets.iter().enumerate() {
            out.push_str(&format!(
                "  set {}: {} - {}\n",
                index + 1,
                set.action_safety.as_str().unwrap_or("UNKNOWN"),
                set.explanation
            ));
        }
        if readiness.media_sets.is_empty() {
            out.push_str("  no media set was formed from this path\n");
        }
        out.push_str(&format!("  note: {}\n", readiness.note));
        out.push_str(&format!("  {}\n", readiness.emulator_and_firmware));
    }
    if let Some(media) = &report.media {
        out.push_str(&format!(
            "\nMedia: {} record(s), {} set(s){}\n",
            media.records,
            media.topology.sets.len(),
            media
                .truncated_to_max_files
                .map(|limit| format!(" (bounded at {limit} files)"))
                .unwrap_or_default()
        ));
    }
    if !report.blockers.is_empty() {
        out.push_str("\nWhy an operation would be blocked\n");
        for blocker in &report.blockers {
            out.push_str(&format!("  {}: {}\n", blocker.reason, blocker.detail));
        }
    }
    out
}

#[cfg(test)]
mod tests;
