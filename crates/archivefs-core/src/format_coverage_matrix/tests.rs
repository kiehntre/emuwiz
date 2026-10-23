//! The matrix is only worth having if it cannot lie. These tests feed every
//! claimed extension through the real scanner and discovery entry points on
//! synthetic files and compare the observed behaviour with what the matrix
//! says, so a registry edit that changes behaviour changes the document too.

use super::*;
use crate::Archive;
use crate::ingestion::{SkipReason, discover_source};
use crate::platform::{SHARED_EXTENSIONS, equivalent_platform_ids, platform_by_id};
use std::collections::BTreeSet;
use std::fs;

/// Bytes that validate as nothing: no signature, no consistent structure.
fn junk(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u8).wrapping_mul(0xA5) ^ 0x5A)
        .collect()
}

fn every_claimed_extension() -> BTreeSet<&'static str> {
    let matrix = static_matrix();
    matrix
        .rows
        .iter()
        .map(|row| row.extension)
        .chain(matrix.unclaimed.iter().map(|entry| entry.extension))
        .collect()
}

fn strong_extensions() -> BTreeSet<&'static str> {
    static_matrix()
        .rows
        .iter()
        .filter(|row| row.claim == ExtensionClaim::Strong)
        .map(|row| row.extension)
        .collect()
}

#[test]
fn every_platform_is_represented() {
    let matrix = static_matrix();
    let claimed: BTreeSet<&str> = matrix.rows.iter().map(|row| row.platform_id).collect();
    let unclaimed: BTreeSet<&str> = platforms_without_extension_claims().into_iter().collect();
    for platform in PLATFORMS {
        assert!(
            claimed.contains(platform.id) ^ unclaimed.contains(platform.id),
            "{} must appear exactly once: in the matrix or in the no-claim list",
            platform.id
        );
    }
    assert_eq!(matrix.platform_count, PLATFORMS.len());
}

#[test]
fn library_route_table_matches_the_scanner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("neutral");
    fs::create_dir_all(&root).unwrap();
    for extension in every_claimed_extension() {
        let path = root.join(format!("game.{extension}"));
        fs::write(&path, junk(4096)).unwrap();
        let observed = Archive::from_path_in_root(&path, &root).map(|archive| archive.kind);
        match library_route_for_extension(extension) {
            LibraryRoute::RegisteredMedia => assert_eq!(
                observed,
                kind_for_extension(extension),
                ".{extension}: a registered media extension is stored on its extension"
            ),
            LibraryRoute::NotPersisted => assert_eq!(
                observed, None,
                ".{extension}: the matrix says NOT STORED but the scanner stored it"
            ),
            LibraryRoute::Corroborated => assert_eq!(
                observed, None,
                ".{extension}: needs corroboration, so a neutral folder must not store it"
            ),
            LibraryRoute::StructuralGate => assert_eq!(
                observed, None,
                ".{extension}: junk bytes must be refused by the structural gate"
            ),
        }
    }

    // The corroboration route is real: the same junk `.md` in a Mega Drive
    // folder is stored as a Mega Drive ROM candidate.
    let md_root = dir.path().join("megadrive");
    fs::create_dir_all(&md_root).unwrap();
    let md = md_root.join("game.md");
    fs::write(&md, junk(4096)).unwrap();
    assert_eq!(
        Archive::from_path_in_root(&md, &md_root).map(|archive| archive.kind),
        Some(ArchiveKind::MegaDriveRom)
    );
}

#[test]
fn every_strong_claim_is_stored_or_explicitly_deferred() {
    let unexplained: Vec<(&str, &str)> = unpersisted_strong_claims()
        .into_iter()
        .filter(|(_, extension)| deferred_strong_extension(extension).is_none())
        .collect();
    assert!(
        unexplained.is_empty(),
        "strong extension claims the plain scan never stores, with no reviewed reason \
         (platform, extension): {unexplained:?}"
    );
}

#[test]
fn deferred_entries_are_current() {
    for entry in DEFERRED_STRONG_EXTENSIONS {
        let platform = platform_by_id(entry.platform_id).unwrap_or_else(|| {
            panic!(
                "deferred entry names unknown platform {}",
                entry.platform_id
            )
        });
        assert!(
            platform.has_strong_extension(entry.extension),
            "stale deferral: .{} is no longer a strong claim of {}",
            entry.extension,
            entry.platform_id
        );
        assert_eq!(
            library_route_for_extension(entry.extension),
            LibraryRoute::NotPersisted,
            "stale deferral: .{} is now stored; remove it from DEFERRED_STRONG_EXTENSIONS",
            entry.extension
        );
        assert!(!entry.reason.trim().is_empty());
    }
}

#[test]
fn discovery_never_calls_a_strong_claim_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("neutral");
    fs::create_dir_all(&root).unwrap();
    for extension in strong_extensions() {
        if deferred_strong_extension(extension).is_some() {
            continue;
        }
        fs::write(root.join(format!("game.{extension}")), junk(4096)).unwrap();
    }
    let report = discover_source(&root).unwrap();
    let unsupported: Vec<String> = report
        .items
        .iter()
        .filter(|item| item.skip_reason == Some(SkipReason::UnsupportedExtension))
        .map(|item| item.path.display().to_string())
        .collect();
    assert!(
        unsupported.is_empty(),
        "discovery reported a registry-strong extension as an unsupported file type: {unsupported:?}"
    );
}

#[test]
fn extension_case_never_changes_the_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("neutral");
    fs::create_dir_all(&root).unwrap();
    for (lower, variants) in [
        ("gba", ["GBA", "Gba"]),
        ("adf", ["ADF", "Adf"]),
        ("z64", ["Z64", "z64"]),
        ("iso", ["ISO", "Iso"]),
        ("md", ["MD", "Md"]),
    ] {
        let reference = root.join(format!("game.{lower}"));
        fs::write(&reference, junk(4096)).unwrap();
        let expected = Archive::from_path_in_root(&reference, &root).map(|archive| archive.kind);
        for variant in variants {
            let path = root.join(format!("other.{variant}"));
            fs::write(&path, junk(4096)).unwrap();
            assert_eq!(
                Archive::from_path_in_root(&path, &root).map(|archive| archive.kind),
                expected,
                ".{variant} must scan exactly like .{lower}"
            );
        }
    }
    let report = discover_source(&root).unwrap();
    let mut by_stem: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for item in &report.items {
        let extension = item
            .path
            .extension()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        by_stem.entry(extension).or_default().push(format!(
            "{:?}/{:?}",
            item.content,
            item.skip_reason.as_ref().map(SkipReason::label)
        ));
    }
    for (extension, outcomes) in by_stem {
        assert!(
            outcomes.windows(2).all(|pair| pair[0] == pair[1]),
            ".{extension}: discovery outcome depends on letter case: {outcomes:?}"
        );
    }
}

#[test]
fn a_shared_extension_alone_never_assigns_a_platform_without_confirmation() {
    for extension in SHARED_EXTENSIONS {
        let verdict = extension_only_detection(extension);
        assert!(
            verdict.requires_confirmation,
            ".{extension} alone must require confirmation, got {verdict:?}"
        );
        assert_ne!(
            verdict.confidence,
            DetectionConfidence::Confirmed,
            ".{extension} alone must never be confirmed"
        );
    }
}

#[test]
fn no_extension_only_verdict_is_ever_confirmed() {
    for row in static_matrix().rows {
        assert_ne!(
            row.extension_only.confidence,
            DetectionConfidence::Confirmed,
            "{} .{}: an extension is never decisive",
            row.platform_id,
            row.extension
        );
        assert!(row.extension_only.requires_confirmation);
    }
}

/// Strong claims shared between platforms that are *not* the same hardware
/// under two identifiers. The detector reports these as ambiguous rather than
/// guessing; the list is explicit so a new collision cannot slip in unnoticed.
const KNOWN_STRONG_COLLISIONS: &[(&str, &[&str])] = &[
    // D88 is a Japanese floppy container used by PC-88, PC-98 and others.
    ("d88", &["NEC PC-8801", "NEC PC-9801", "PC-98"]),
    // EBOOT.PBP wraps PSP titles and PS1 Classics alike; the container's
    // own header settles it, never the extension.
    ("pbp", &["PSP", "PSX"]),
    // Atari 8-bit binary-load files and Xbox 360 executables share `.xex`;
    // discovery routes on the first bytes (`discover_xex_content`).
    ("xex", &["Atari 8-bit", "Xbox360"]),
];

#[test]
fn strong_claims_shared_by_different_hardware_are_explicitly_known() {
    let mut collisions: Vec<(&str, Vec<&str>)> = Vec::new();
    for extension in strong_extensions() {
        let mut claimants: Vec<&str> = platforms_with_strong_extension(extension)
            .into_iter()
            .map(|platform| platform.id)
            .collect();
        claimants.sort_unstable();
        if claimants.len() < 2 {
            continue;
        }
        let all_equivalent = claimants.iter().all(|left| {
            claimants
                .iter()
                .all(|right| left == right || equivalent_platform_ids(left).contains(right))
        });
        if !all_equivalent {
            collisions.push((extension, claimants));
        }
    }
    let expected: Vec<(&str, Vec<&str>)> = KNOWN_STRONG_COLLISIONS
        .iter()
        .map(|(extension, ids)| (*extension, ids.to_vec()))
        .collect();
    assert_eq!(
        collisions, expected,
        "strong-extension collisions changed; review KNOWN_STRONG_COLLISIONS"
    );
    for (extension, _) in KNOWN_STRONG_COLLISIONS {
        let verdict = extension_only_detection(extension);
        assert_eq!(
            verdict.platform, None,
            ".{extension} is claimed by different hardware, so alone it must choose nobody"
        );
    }
}

#[test]
fn json_rendering_is_valid_and_complete() {
    let matrix = static_matrix();
    let json = render_json(&matrix);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value["rows"].as_array().map(Vec::len),
        Some(matrix.rows.len())
    );
    assert_eq!(value["platform_count"], PLATFORMS.len());
}

#[test]
fn checked_in_document_matches_the_registry() {
    let rendered = render_markdown(&static_matrix(), Some(&content_dispositions()));
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(DOCUMENT_PATH);
    if std::env::var_os("EMUWIZ_WRITE_COVERAGE_MATRIX").is_some() {
        fs::write(&path, &rendered).unwrap();
        return;
    }
    let current = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == rendered,
        "{DOCUMENT_PATH} is stale: regenerate with EMUWIZ_WRITE_COVERAGE_MATRIX=1 \
         scripts/cargo-iso test -p archivefs-core --lib format_coverage_matrix"
    );
}

/// Old-cache → new-scanner: a fingerprint that recorded "unsupported" for an
/// extension the registry has since learned is re-evaluated; one for an
/// extension that is still unsupported is reused. No scanner-version bump, so
/// the rest of a real library's fingerprints stay valid.
#[test]
fn stale_unsupported_fingerprints_are_re_evaluated_only_for_newly_stored_extensions() {
    use crate::{ArchiveScanner, Config, SCANNER_VERSION, ScanFingerprint};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("roms");
    fs::create_dir_all(&root).unwrap();
    let now_stored = root.join("Bonk.pce");
    let still_unsupported = root.join("Notes.xyz");
    let still_ambiguous = root.join("Lone.bin");
    for path in [&now_stored, &still_unsupported, &still_ambiguous] {
        fs::write(path, junk(4096)).unwrap();
    }
    let fingerprint = |path: &Path, verdict: &str| {
        let metadata = fs::metadata(path).unwrap();
        ScanFingerprint {
            relative_path: path.strip_prefix(&root).unwrap().to_path_buf(),
            size_bytes: metadata.len(),
            modified_time_ns: metadata
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as i128,
            archive_kind: ArchiveKind::DirectGameImage,
            display_name: String::new(),
            normalized_name: String::new(),
            platform: None,
            platform_provenance: None,
            scanner_version: SCANNER_VERSION.into(),
            parser_version: crate::ARCHIVE_PARSER_VERSION.into(),
            cache_version: crate::SCAN_CACHE_VERSION,
            ingestion: None,
            non_archive_kind: Some(verdict.to_string()),
        }
    };
    let cached = vec![
        (1, fingerprint(&now_stored, "unsupported")),
        (1, fingerprint(&still_unsupported, "unsupported")),
        (1, fingerprint(&still_ambiguous, "ambiguous")),
    ];
    let config = Config {
        source_folders: vec![root.clone()],
        mount_root: dir.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    let discovery = ArchiveScanner::new(&config)
        .scan_archives_with_cache(&cached)
        .unwrap();
    let found: Vec<&Path> = discovery
        .archives
        .iter()
        .map(|archive| archive.path.as_path())
        .collect();
    assert_eq!(
        found,
        vec![now_stored.as_path()],
        "the newly registered .pce is stored; the stale verdict was not trusted"
    );
    assert_eq!(
        discovery.archives[0].identity.display_name, "Bonk",
        "the row was built from the file, not rebuilt from the stale fingerprint"
    );
    assert_eq!(
        discovery.timings.reused, 2,
        "the still-unsupported .xyz and still-ambiguous .bin verdicts are reused"
    );
    assert!(
        discovery.non_archive_fingerprints.is_empty(),
        "nothing needed a fresh non-archive fingerprint: {:?}",
        discovery.non_archive_fingerprints
    );
}

/// Every valid sample whose bytes prove a single platform must be stored and
/// discovered with that platform. Shared tape containers stay ambiguous.
#[test]
fn content_samples_prove_their_platform_through_both_pipelines() {
    for row in content_dispositions() {
        if matches!(row.extension, "tap" | "tzx") {
            assert_eq!(row.stored_platform, None, "{row:?}");
            continue;
        }
        if row.structure.contains("dual-mode") {
            // The CGB-enhanced header flag is itself the proof of Game Boy
            // Color (header identity); the catalogue DAT lookup may not match.
            assert_eq!(row.stored_platform.as_deref(), Some("Game Boy Color"), "{row:?}");
            assert_eq!(row.stored_provenance.as_deref(), Some("header_identity"), "{row:?}");
            continue;
        }
        assert_eq!(
            row.stored_platform.as_deref(),
            Some(row.platform_id),
            "{row:?}"
        );
        assert_eq!(
            row.discovery,
            format!("accepted: {}", row.platform_id),
            "{row:?}"
        );
    }
}

/// A platform the registry detector settled on a shared signature is
/// re-evaluated on the next scan; folder-assigned rows are reused as before.
#[test]
fn detector_assigned_shared_signature_rows_are_re_evaluated_not_reused() {
    use crate::{ArchiveScanner, Config, PlatformProvenance, SCANNER_VERSION, ScanFingerprint};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("roms");
    fs::create_dir_all(&root).unwrap();
    let cartridge = root.join("Knuckles.md");
    fs::write(&cartridge, crate::synthetic_media::sega32x_bin(0x400)).unwrap();
    let fingerprint = |path: &Path, provenance: PlatformProvenance| {
        let metadata = fs::metadata(path).unwrap();
        ScanFingerprint {
            relative_path: path.strip_prefix(&root).unwrap().to_path_buf(),
            size_bytes: metadata.len(),
            modified_time_ns: metadata
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as i128,
            archive_kind: ArchiveKind::MegaDriveRom,
            display_name: "Knuckles".into(),
            normalized_name: "knuckles".into(),
            platform: Some("MegaDrive".into()),
            platform_provenance: Some(provenance),
            scanner_version: SCANNER_VERSION.into(),
            parser_version: crate::ARCHIVE_PARSER_VERSION.into(),
            cache_version: crate::SCAN_CACHE_VERSION,
            ingestion: None,
            non_archive_kind: None,
        }
    };
    let config = Config {
        source_folders: vec![root.clone()],
        mount_root: dir.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    // Settled by the detector on the shared `SEGA` bytes: looked at again,
    // and the console name now says 32X.
    let discovery = ArchiveScanner::new(&config)
        .scan_archives_with_cache(&[(
            1,
            fingerprint(&cartridge, PlatformProvenance::RegistryDetector),
        )])
        .unwrap();
    assert_eq!(discovery.timings.reused, 0);
    assert_eq!(
        discovery.archives[0].identity.platform.as_deref(),
        Some("Sega 32X"),
        "{:?}",
        discovery.archives[0].identity
    );
    // Settled by the folder name: reused as before.
    let discovery = ArchiveScanner::new(&config)
        .scan_archives_with_cache(&[(1, fingerprint(&cartridge, PlatformProvenance::FolderAlias))])
        .unwrap();
    assert_eq!(discovery.timings.reused, 1);
    assert_eq!(
        discovery.archives[0].identity.platform.as_deref(),
        Some("MegaDrive")
    );
}
