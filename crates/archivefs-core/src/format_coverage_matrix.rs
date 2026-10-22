//! Generated PLATFORM × FORMAT coverage matrix.
//!
//! The platform registry ([`crate::platform::PLATFORMS`]) promises evidence:
//! "`.pce` is specific to PC Engine". The Library scanner persists only what
//! [`crate::media_registry`] (or one of the scanner's bounded structural
//! gates) recognises, and the discovery pipeline categorises only what
//! [`crate::ingestion::content_registry`] knows. Those three contracts are
//! deliberately separate, which is exactly how a promise can go unkept: a
//! platform can advertise a strong extension that no scanner ever stores.
//!
//! This module projects every registered platform and every extension it
//! claims into one row set, computed from the live registries, so the
//! disposition of each PLATFORM × EXTENSION pair is machine-readable and can
//! never drift from the code. Nothing here owns an extension list of its own:
//! the only hand-maintained data is [`DEFERRED_STRONG_EXTENSIONS`], which
//! names the strong claims the Library scanner *knowingly* does not persist
//! and why. The test suite fails when a strong claim is neither persisted nor
//! explicitly deferred, when a deferred entry becomes persisted (stale), and
//! when the checked-in document no longer matches the generated matrix.
//!
//! A row is a *disposition*, never proof of end-to-end support: a registered
//! media row means the scanner stores the file; whether the bytes are valid
//! media, whether a DAT names them and whether an emulator can run them are
//! separate facts recorded in their own columns, each with its own authority.

use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::ArchiveKind;
use crate::coverage_inventory::{COVERAGE, ValidationStatus};
use crate::ingestion::content_registry::{ContentKind, content_kind_for_extension};
use crate::inspector::{InspectorEntryClassification, classify_entry};
use crate::launch::platform_map::{MappingConfidence, launch_compatibility_for_platform};
use crate::media_registry::{self, kind_for_extension};
use crate::platform::detect::{DetectionConfidence, DetectionRequest, detect_platform_report};
use crate::platform::{
    PLATFORMS, Platform, is_shared_extension, platforms_with_strong_extension,
    platforms_with_weak_extension,
};

#[cfg(test)]
mod tests;

/// Path to the checked-in rendering of [`static_matrix`], relative to the
/// repository root. `tests::checked_in_document_matches_the_registry` keeps it
/// current.
pub const DOCUMENT_PATH: &str = "docs/PLATFORM_FORMAT_COVERAGE_MATRIX.md";

/// How strongly a platform claims an extension in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExtensionClaim {
    /// [`Platform::strong_extensions`]: specific enough to carry a detection
    /// alone (still never a verified game identity).
    Strong,
    /// [`Platform::weak_extensions`]: shared; only ever narrows a decision.
    Weak,
}

impl ExtensionClaim {
    pub fn label(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Weak => "weak",
        }
    }
}

/// How a loose file with this extension reaches a Library row through the
/// ordinary Add Source → Scan path (`crate::archive_kind_in_root`).
///
/// This table mirrors the scanner's dispatch order and is *verified against
/// the scanner* by `tests::library_route_table_matches_the_scanner`, which
/// feeds every extension through `Archive::from_path_in_root` and asserts the
/// observed behaviour. It is documentation that cannot go stale, not a second
/// implementation the scanner consults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LibraryRoute {
    /// A [`crate::media_registry::MEDIA_FORMATS`] row: the file is stored on
    /// its extension alone, as a `DirectGameImage` (or archive kind).
    RegisteredMedia,
    /// Stored only after a bounded structural inspector accepts the bytes
    /// (`.hdf`/`.hdfx` RDB, Sinclair snapshots and TR-DOS, a PlayStation
    /// `.cue`, a Wii U `.rpx` inside an extracted title). Junk bytes are
    /// refused and reported, never stored.
    StructuralGate,
    /// Stored only with folder, source-root or cartridge-header
    /// corroboration (`.md`/`.bin`/`.gen` Mega Drive candidates).
    Corroborated,
    /// The scanner never stores a loose file with this extension. For a
    /// strong claim this is a defect unless [`DEFERRED_STRONG_EXTENSIONS`]
    /// explains it.
    NotPersisted,
}

impl LibraryRoute {
    pub fn label(self) -> &'static str {
        match self {
            Self::RegisteredMedia => "stored (extension)",
            Self::StructuralGate => "stored (structural gate)",
            Self::Corroborated => "stored (needs corroboration)",
            Self::NotPersisted => "NOT STORED",
        }
    }
}

/// The scanner's special cases in the order `crate::archive_kind_in_root`
/// applies them, then the media registry, then Mega Drive corroboration.
pub fn library_route_for_extension(extension: &str) -> LibraryRoute {
    let extension = extension.to_ascii_lowercase();
    if extension == "rpx"
        || matches!(extension.as_str(), "hdf" | "hdfx")
        || media_registry::requires_spectrum_structure(&extension)
        || extension == "cue"
    {
        return LibraryRoute::StructuralGate;
    }
    if kind_for_extension(&extension).is_some() {
        return LibraryRoute::RegisteredMedia;
    }
    if media_registry::is_corroboration_candidate(&extension) {
        return LibraryRoute::Corroborated;
    }
    LibraryRoute::NotPersisted
}

/// A strong extension claim the Library scanner knowingly does not persist.
/// Each entry is a reviewed decision with a reason a person can read; the
/// tests keep this list exact (no missing entries, no stale ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DeferredStrongExtension {
    pub extension: &'static str,
    pub platform_id: &'static str,
    pub reason: &'static str,
}

/// See [`DeferredStrongExtension`]. Deliberately short: every entry here is a
/// game the plain scan cannot store, so each needs a real reason.
pub const DEFERRED_STRONG_EXTENSIONS: &[DeferredStrongExtension] = &[
    DeferredStrongExtension {
        extension: "gdi",
        platform_id: "Dreamcast",
        reason: "a GD-ROM is a sheet plus track files; the explicit Discovery import validates every \
                 track as a companion (`ingestion::gdi`), the plain scan has no structural gate yet",
    },
    DeferredStrongExtension {
        extension: "ecm",
        platform_id: "PSX",
        reason: "ECM is a generic error-correction-stripped wrapper around any CD image, not a \
                 PlayStation format; it needs a wrapper inspector before any platform claim",
    },
    DeferredStrongExtension {
        extension: "wad",
        platform_id: "Wii",
        reason: "collides with DOOM-engine IWAD/PWAD files; needs the Wii WAD header check before \
                 a loose file is stored as a Wii title",
    },
    DeferredStrongExtension {
        extension: "n-gage",
        platform_id: "NGage",
        reason: "non-standard extension with no documented container; no inspector exists",
    },
    DeferredStrongExtension {
        extension: "d81ns",
        platform_id: "Commodore 64",
        reason: "unknown variant of .d81 with no documented layout; registry claim under review",
    },
];

/// Whether `extension` is a reviewed deferred strong claim.
pub fn deferred_strong_extension(extension: &str) -> Option<&'static DeferredStrongExtension> {
    DEFERRED_STRONG_EXTENSIONS
        .iter()
        .find(|entry| entry.extension == extension)
}

/// What the platform detector concludes from the extension alone - a loose
/// file in a folder that names no platform, with no signature read. This is
/// the "recognised from filename hints only" column: it says what platform a
/// file would be *offered* as, never what it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtensionOnlyDetection {
    pub platform: Option<&'static str>,
    pub confidence: DetectionConfidence,
    pub requires_confirmation: bool,
}

/// The platform the Library scan actually stores for `game.<extension>` in a
/// folder naming no platform (`crate::detect_platform_with_details`, which
/// applies the registry's conflict policy on top of the detector's verdict).
/// `None` means the row is stored with no platform and waits for a person.
pub fn stored_platform_from_extension_alone(extension: &str) -> Option<&'static str> {
    let root = Path::new("/emuwiz-synthetic-neutral-root");
    let path = root.join(format!("game.{extension}"));
    let detected = crate::detect_platform_with_details(&path, root)
        .and_then(|detection| crate::platform::platform_by_id(&detection.platform))
        .map(|platform| platform.id);
    // `Archive::from_path_in_root` fills in Mega Drive for a `MegaDriveRom`
    // row that nothing stronger identified (a `.smd` dump is Mega Drive
    // specific); the integration test `platform_format_coverage` proves this
    // mirror against the real scanner.
    detected.or_else(|| {
        (kind_for_extension(&extension.to_ascii_lowercase()) == Some(ArchiveKind::MegaDriveRom))
            .then_some("MegaDrive")
    })
}

/// One PLATFORM × EXTENSION disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatrixRow {
    pub platform_id: &'static str,
    pub display_name: &'static str,
    pub extension: &'static str,
    pub claim: ExtensionClaim,
    /// `true` when the extension is in [`crate::platform::SHARED_EXTENSIONS`].
    pub shared: bool,
    /// Other platforms that also claim this extension as strong evidence.
    pub other_strong_claimants: Vec<&'static str>,
    /// Other platforms that claim this extension as weak evidence.
    pub other_weak_claimants: Vec<&'static str>,
    #[serde(serialize_with = "debug_option")]
    pub media_kind: Option<ArchiveKind>,
    #[serde(serialize_with = "label_option")]
    pub content_kind: Option<ContentKind>,
    /// Whether the archive Inspector treats a member with this extension as
    /// likely game content.
    pub inspector_likely_content: bool,
    pub library_route: LibraryRoute,
    pub deferred_reason: Option<&'static str>,
    pub extension_only: ExtensionOnlyDetection,
    /// What the scan stores as the platform for a loose file in a neutral
    /// folder; see [`stored_platform_from_extension_alone`].
    pub stored_platform: Option<&'static str>,
    /// Reviewed standalone launch mapping for the platform, if any.
    #[serde(serialize_with = "debug_option")]
    pub launch: Option<MappingConfidence>,
    /// Engineering evidence coverage for the platform
    /// ([`crate::coverage_inventory`]); `None` when the platform has no
    /// dedicated content-evidence work at all.
    #[serde(serialize_with = "debug_option")]
    pub evidence_coverage: Option<ValidationStatus>,
}

/// An extension the media or content registries recognise that no platform
/// claims. Not a defect by itself (`.rdb`, `.ima`, `.ciso` are real formats
/// reached through other evidence), but it must be visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnclaimedExtension {
    pub extension: &'static str,
    #[serde(serialize_with = "debug_option")]
    pub media_kind: Option<ArchiveKind>,
    #[serde(serialize_with = "label_option")]
    pub content_kind: Option<ContentKind>,
}

fn debug_option<T: std::fmt::Debug, S: serde::Serializer>(
    value: &Option<T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_some(&format!("{value:?}")),
        None => serializer.serialize_none(),
    }
}

fn label_option<S: serde::Serializer>(
    value: &Option<ContentKind>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_some(value.label()),
        None => serializer.serialize_none(),
    }
}

/// The whole generated matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageMatrix {
    pub platform_count: usize,
    pub rows: Vec<MatrixRow>,
    pub unclaimed: Vec<UnclaimedExtension>,
    pub deferred: Vec<DeferredStrongExtension>,
}

fn other_ids(platforms: Vec<&'static Platform>, except: &str) -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = platforms
        .into_iter()
        .map(|platform| platform.id)
        .filter(|id| *id != except)
        .collect();
    ids.sort_unstable();
    ids
}

/// The detector's verdict for `game.<extension>` in a neutral folder.
pub fn extension_only_detection(extension: &str) -> ExtensionOnlyDetection {
    let root = Path::new("/emuwiz-synthetic-neutral-root");
    let path = root.join(format!("game.{extension}"));
    let report = detect_platform_report(&DetectionRequest::new(&path, root));
    ExtensionOnlyDetection {
        platform: report.platform,
        confidence: report.confidence,
        requires_confirmation: report.requires_confirmation,
    }
}

fn row(platform: &'static Platform, extension: &'static str, claim: ExtensionClaim) -> MatrixRow {
    let route = library_route_for_extension(extension);
    MatrixRow {
        platform_id: platform.id,
        display_name: platform.display_name,
        extension,
        claim,
        shared: is_shared_extension(extension),
        other_strong_claimants: other_ids(platforms_with_strong_extension(extension), platform.id),
        other_weak_claimants: other_ids(platforms_with_weak_extension(extension), platform.id),
        media_kind: kind_for_extension(extension),
        content_kind: content_kind_for_extension(extension),
        inspector_likely_content: classify_entry(&format!("game.{extension}"), false)
            == InspectorEntryClassification::LikelyContent,
        library_route: route,
        deferred_reason: (claim == ExtensionClaim::Strong && route == LibraryRoute::NotPersisted)
            .then(|| deferred_strong_extension(extension).map(|entry| entry.reason))
            .flatten(),
        extension_only: extension_only_detection(extension),
        stored_platform: stored_platform_from_extension_alone(extension),
        launch: launch_compatibility_for_platform(platform.id).map(|entry| entry.confidence),
        evidence_coverage: COVERAGE
            .iter()
            .find(|entry| entry.canonical_id == platform.id)
            .map(|entry| entry.real_validation),
    }
}

/// Every PLATFORM × EXTENSION pair the registry declares, in registry order,
/// strong claims before weak ones within a platform.
pub fn static_matrix() -> CoverageMatrix {
    let mut rows = Vec::new();
    for platform in PLATFORMS {
        for extension in platform.strong_extensions {
            rows.push(row(platform, extension, ExtensionClaim::Strong));
        }
        for extension in platform.weak_extensions {
            rows.push(row(platform, extension, ExtensionClaim::Weak));
        }
    }
    let claimed = |extension: &str| {
        !platforms_with_strong_extension(extension).is_empty()
            || !platforms_with_weak_extension(extension).is_empty()
    };
    let mut unclaimed: Vec<UnclaimedExtension> = media_registry::MEDIA_FORMATS
        .iter()
        .map(|format| format.extension)
        .chain(crate::ingestion::content_registry::recognized_extensions())
        .filter(|extension| !claimed(extension))
        .map(|extension| UnclaimedExtension {
            extension,
            media_kind: kind_for_extension(extension),
            content_kind: content_kind_for_extension(extension),
        })
        .collect();
    unclaimed.sort_by(|left, right| left.extension.cmp(right.extension));
    unclaimed.dedup_by(|left, right| left.extension == right.extension);
    CoverageMatrix {
        platform_count: PLATFORMS.len(),
        rows,
        unclaimed,
        deferred: DEFERRED_STRONG_EXTENSIONS.to_vec(),
    }
}

/// Strong claims the scanner does not persist, whether or not they are
/// explicitly deferred. Each is a `(platform id, extension)` pair.
pub fn unpersisted_strong_claims() -> Vec<(&'static str, &'static str)> {
    static_matrix()
        .rows
        .iter()
        .filter(|row| {
            row.claim == ExtensionClaim::Strong && row.library_route == LibraryRoute::NotPersisted
        })
        .map(|row| (row.platform_id, row.extension))
        .collect()
}

/// Platforms whose every claimed extension is `NotPersisted` - a loose file
/// for them can never reach the Library through the plain scan, whatever its
/// bytes say.
pub fn platforms_with_no_library_route() -> Vec<&'static str> {
    let matrix = static_matrix();
    PLATFORMS
        .iter()
        .filter(|platform| {
            let claims: Vec<&MatrixRow> = matrix
                .rows
                .iter()
                .filter(|row| row.platform_id == platform.id)
                .collect();
            !claims.is_empty()
                && claims
                    .iter()
                    .all(|row| row.library_route == LibraryRoute::NotPersisted)
        })
        .map(|platform| platform.id)
        .collect()
}

/// Platforms that claim no extension at all (directory-layout or specialist
/// platforms such as ScummVM, Dragon/CoCo or Arcade sets reach the Library
/// through other evidence).
pub fn platforms_without_extension_claims() -> Vec<&'static str> {
    PLATFORMS
        .iter()
        .filter(|platform| {
            platform.strong_extensions.is_empty() && platform.weak_extensions.is_empty()
        })
        .map(|platform| platform.id)
        .collect()
}

fn confidence_label(confidence: DetectionConfidence) -> &'static str {
    match confidence {
        DetectionConfidence::Unknown => "unknown",
        DetectionConfidence::Ambiguous => "ambiguous",
        DetectionConfidence::Probable => "probable",
        DetectionConfidence::Confirmed => "confirmed",
    }
}

fn mapping_label(confidence: Option<MappingConfidence>) -> &'static str {
    match confidence {
        Some(MappingConfidence::Exact) => "standalone (exact)",
        Some(MappingConfidence::StronglyKnown) => "standalone (strongly known)",
        Some(MappingConfidence::Ambiguous) => "standalone (ambiguous)",
        Some(MappingConfidence::Unsupported) | None => "none reviewed",
    }
}

fn coverage_label(status: Option<ValidationStatus>) -> &'static str {
    match status {
        Some(ValidationStatus::RealValidated) => "real-validated",
        Some(ValidationStatus::SyntheticValidated) => "synthetic-validated",
        Some(ValidationStatus::Partial) => "partial",
        Some(ValidationStatus::Deferred) => "deferred",
        Some(ValidationStatus::NoDeepEvidence) | None => "none",
    }
}

fn list_or_dash(ids: &[&str]) -> String {
    if ids.is_empty() {
        "-".to_string()
    } else {
        ids.join(", ")
    }
}

/// Renders the matrix as the Markdown document checked in at
/// [`DOCUMENT_PATH`]. Deterministic: the same registry always renders the
/// same bytes.
pub fn render_markdown(matrix: &CoverageMatrix) -> String {
    let mut out = String::new();
    let unpersisted: Vec<&MatrixRow> = matrix
        .rows
        .iter()
        .filter(|row| {
            row.claim == ExtensionClaim::Strong && row.library_route == LibraryRoute::NotPersisted
        })
        .collect();
    let _ = writeln!(out, "# Platform × format coverage matrix");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "GENERATED from the live registries by `archivefs_core::format_coverage_matrix` - do not \
         edit by hand. Regenerate with `EMUWIZ_WRITE_COVERAGE_MATRIX=1 scripts/cargo-iso test -p \
         archivefs-core --lib format_coverage_matrix` or `emuwiz-cli platform-coverage`. The test \
         `checked_in_document_matches_the_registry` fails when this file is stale."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## What a row means");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- **Claim**: how the platform registry rates the extension (`strong` carries a detection \
         alone; `weak` only narrows). Neither is a verified game identity."
    );
    let _ = writeln!(
        out,
        "- **Library route**: how a loose file reaches a Library row through Add Source → Scan. \
         `NOT STORED` means the plain scan never persists it; for a strong claim that is a defect \
         unless the Deferred section explains it."
    );
    let _ = writeln!(
        out,
        "- **Ingestion**: the category the discovery pipeline assigns from the extension; `-` \
         means discovery reports the extension as unsupported."
    );
    let _ = writeln!(
        out,
        "- **Extension alone**: the platform detector's verdict for the file in a folder that \
         names no platform, with no bytes read. Every such verdict requires confirmation."
    );
    let _ = writeln!(
        out,
        "- **Stored as**: the platform the scan actually records for that file (the detector's \
         verdict filtered by the registry's conflict policy); `-` means stored without a \
         platform, waiting for a person."
    );
    let _ = writeln!(
        out,
        "- **Launch** and **Evidence** describe the *platform*, not the file: the reviewed \
         standalone launch mapping and the engineering evidence coverage."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Summary");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Measure | Count |");
    let _ = writeln!(out, "|---|---:|");
    let _ = writeln!(out, "| Registered platforms | {} |", matrix.platform_count);
    let _ = writeln!(out, "| PLATFORM × EXTENSION rows | {} |", matrix.rows.len());
    let _ = writeln!(
        out,
        "| Strong claims | {} |",
        matrix
            .rows
            .iter()
            .filter(|row| row.claim == ExtensionClaim::Strong)
            .count()
    );
    let _ = writeln!(
        out,
        "| Strong claims the plain scan does not store | {} |",
        unpersisted.len()
    );
    let _ = writeln!(
        out,
        "| Platforms with no Library route at all | {} |",
        platforms_with_no_library_route().len()
    );
    let _ = writeln!(
        out,
        "| Platforms claiming no extension | {} |",
        platforms_without_extension_claims().len()
    );
    let _ = writeln!(
        out,
        "| Registered extensions no platform claims | {} |",
        matrix.unclaimed.len()
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Deferred strong claims");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Strong claims the Library scanner knowingly does not persist. Each needs a reason; the \
         test suite keeps this list exact."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "| Extension | Platform | Reason |");
    let _ = writeln!(out, "|---|---|---|");
    for entry in &matrix.deferred {
        let _ = writeln!(
            out,
            "| `.{}` | {} | {} |",
            entry.extension, entry.platform_id, entry.reason
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Platforms with no Library route");
    let _ = writeln!(out);
    let none = platforms_with_no_library_route();
    if none.is_empty() {
        let _ = writeln!(out, "None.");
    } else {
        for id in none {
            let _ = writeln!(out, "- {id}");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Platforms claiming no extension");
    let _ = writeln!(out);
    for id in platforms_without_extension_claims() {
        let _ = writeln!(out, "- {id}");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Matrix");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| Platform | Ext | Claim | Library route | Ingestion | Inspector | Extension alone | \
         Stored as | Other strong | Other weak | Launch | Evidence |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|");
    for row in &matrix.rows {
        let route = match row.deferred_reason {
            Some(_) => format!("{} (deferred)", row.library_route.label()),
            None => row.library_route.label().to_string(),
        };
        let alone = match row.extension_only.platform {
            Some(platform) => format!(
                "{platform} ({})",
                confidence_label(row.extension_only.confidence)
            ),
            None => confidence_label(row.extension_only.confidence).to_string(),
        };
        let _ = writeln!(
            out,
            "| {} | `.{}` | {}{} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            row.display_name,
            row.extension,
            row.claim.label(),
            if row.shared { " (shared)" } else { "" },
            route,
            row.content_kind.map_or("-", ContentKind::label),
            if row.inspector_likely_content {
                "content"
            } else {
                "-"
            },
            alone,
            row.stored_platform.unwrap_or("-"),
            list_or_dash(&row.other_strong_claimants),
            list_or_dash(&row.other_weak_claimants),
            mapping_label(row.launch),
            coverage_label(row.evidence_coverage),
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Registered extensions no platform claims");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Ext | Media kind | Ingestion |");
    let _ = writeln!(out, "|---|---|---|");
    for entry in &matrix.unclaimed {
        let _ = writeln!(
            out,
            "| `.{}` | {} | {} |",
            entry.extension,
            entry
                .media_kind
                .map_or("-".to_string(), |kind| format!("{kind:?}")),
            entry.content_kind.map_or("-", ContentKind::label),
        );
    }
    out
}

/// The matrix as JSON, for tooling.
pub fn render_json(matrix: &CoverageMatrix) -> String {
    serde_json::to_string_pretty(matrix).expect("matrix serialises")
}
