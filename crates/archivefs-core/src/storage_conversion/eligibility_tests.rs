//! Storage Review must not call a CHD conversion ready unless EmuWiz's own
//! verified converter would admit it.
//!
//! Unsafe behaviour prevented (2026-11-03 optical audit, Priority 1): Storage
//! Review grouped multi-track and raw-sector CUE sheets, GDI descriptors and
//! CD-platform ISO images under "Can shrink safely", while the converter
//! (`repair::optical_conversion`) refuses every one of them. CD-platform ISOs
//! were also routed to chdman's DVD mode (`createdvd`) and labelled
//! byte-identical. Each case below runs the real analysis on small synthetic
//! files in a temporary directory; no tool is probed or executed.

use super::*;
use crate::storage_health::{StorageHealthInput, analyze_storage_health};
use std::fs;
use std::path::Path;

/// A synthetic inventory: `on_path` is what the PATH lookup found; `binding`
/// is the converter's own judgement of its reviewed chdman.
fn inventory(on_path: Option<&str>, binding: ChdmanBinding) -> ConversionToolInventory {
    ConversionToolInventory {
        tools: vec![ConversionToolRecord {
            name: "chdman".into(),
            path: on_path.map(PathBuf::from),
            version: on_path.map(|_| "chdman 0.264".into()),
            status: if on_path.is_some() {
                ToolCapabilityStatus::Found
            } else {
                ToolCapabilityStatus::Missing
            },
            capabilities: vec!["createcd".into(), "createdvd".into()],
            capability_source: "synthetic".into(),
        }],
        reviewed_chdman: binding,
    }
}

fn chdman_at(path: Option<&str>) -> ConversionToolInventory {
    let binding = match path {
        Some(path) => ChdmanBinding::Usable(PathBuf::from(path)),
        None => ChdmanBinding::Absent,
    };
    inventory(path, binding)
}

fn reviewed_chdman() -> ConversionToolInventory {
    chdman_at(Some("/usr/bin/chdman"))
}

fn admitted_cue(dir: &Path) -> PathBuf {
    cue_set(
        dir,
        "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        &[("disc.bin", 2048 * 4)],
    )
}

fn analyse(path: &Path, platform: &str) -> StorageHealthItem {
    let report = analyze_storage_health(&[StorageHealthInput {
        path: path.to_path_buf(),
        platform: Some(platform.into()),
        logical_size_bytes: None,
        format_hint: None,
        content_hash: None,
    }]);
    report.items.into_iter().next().expect("one analysed item")
}

/// A CUE sheet plus the files it names, each filled with `len` zero bytes.
fn cue_set(dir: &Path, cue: &str, files: &[(&str, usize)]) -> PathBuf {
    for (name, len) in files {
        fs::write(dir.join(name), vec![0_u8; *len]).unwrap();
    }
    let path = dir.join("disc.cue");
    fs::write(&path, cue).unwrap();
    path
}

fn assert_not_ready(item: &StorageHealthItem, inventory: &ConversionToolInventory, case: &str) {
    let capability = capability_for_item(item, inventory);
    assert_ne!(
        capability.eligibility,
        ConversionEligibility::ConversionReady,
        "{case}: must not be called ready: {capability:?}"
    );
    assert_ne!(
        item.opportunity.kind,
        StorageOpportunityKind::Compressible,
        "{case}: must not be offered as a compressible candidate"
    );
    assert_ne!(
        item.opportunity.round_trip,
        StorageRoundTripClass::ContentEquivalent,
        "{case}: no verified content-equivalent route exists"
    );
    assert!(
        !item.opportunity.estimate.explanation.trim().is_empty(),
        "{case}: the refusal must stay visible with a reason"
    );
}

#[test]
fn the_admitted_single_track_mode1_2048_cue_stays_conversion_ready() {
    let dir = tempfile::tempdir().unwrap();
    let item = analyse(&admitted_cue(dir.path()), "PlayStation");
    assert_eq!(item.opportunity.kind, StorageOpportunityKind::Compressible);
    assert_eq!(
        item.opportunity.round_trip,
        StorageRoundTripClass::ContentEquivalent
    );
    let capability = capability_for_item(&item, &reviewed_chdman());
    assert_eq!(
        capability.eligibility,
        ConversionEligibility::ConversionReady
    );
    assert_eq!(capability.mode.as_deref(), Some("createcd"));
    assert_eq!(capability.target_format, Some(StorageFormatClass::Chd));
    // Without the tool it is the same candidate, waiting for chdman.
    assert_eq!(
        capability_for_item(&item, &chdman_at(None)).eligibility,
        ConversionEligibility::ToolMissing
    );
}

#[test]
fn regression_multi_track_cue_with_audio_is_not_conversion_ready() {
    let dir = tempfile::tempdir().unwrap();
    let cue = cue_set(
        dir.path(),
        "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n\
         FILE \"audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        &[("data.bin", 2048 * 4), ("audio.bin", 2352 * 4)],
    );
    let item = analyse(&cue, "PlayStation");
    assert_not_ready(&item, &reviewed_chdman(), "multi-track CUE");
}

#[test]
fn regression_raw_sector_mode2_cue_is_not_conversion_ready() {
    // The usual PS1 dump shape. The converter admits only cooked MODE1/2048.
    let dir = tempfile::tempdir().unwrap();
    let cue = cue_set(
        dir.path(),
        "FILE \"disc.bin\" BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00\n",
        &[("disc.bin", 2352 * 4)],
    );
    let item = analyse(&cue, "PlayStation");
    assert_not_ready(&item, &reviewed_chdman(), "MODE2/2352 CUE");
}

#[test]
fn regression_cue_with_stored_pregap_is_refused_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let cue = cue_set(
        dir.path(),
        "FILE \"disc.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 00 00:00:00\nINDEX 01 00:00:02\n",
        &[("disc.bin", 2048 * 4)],
    );
    let item = analyse(&cue, "PlayStation");
    assert_not_ready(&item, &reviewed_chdman(), "INDEX 00 pregap CUE");
    assert!(
        item.opportunity
            .estimate
            .explanation
            .to_ascii_lowercase()
            .contains("pregap"),
        "the converter's own preservation reason is shown: {:?}",
        item.opportunity.estimate.explanation
    );
}

#[test]
fn regression_unsupported_cue_is_not_presented_as_waiting_for_a_tool() {
    // Installing chdman would not make this conversion possible.
    let dir = tempfile::tempdir().unwrap();
    let cue = cue_set(
        dir.path(),
        "FILE \"disc.bin\" BINARY\nTRACK 01 MODE2/2352\nINDEX 01 00:00:00\n",
        &[("disc.bin", 2352 * 4)],
    );
    let item = analyse(&cue, "PlayStation");
    let capability = capability_for_item(&item, &chdman_at(None));
    assert_ne!(capability.eligibility, ConversionEligibility::ToolMissing);
    assert_ne!(
        capability.eligibility,
        ConversionEligibility::ConversionReady
    );
}

#[test]
fn regression_gdi_descriptor_is_not_conversion_ready() {
    let dir = tempfile::tempdir().unwrap();
    for (name, len) in [
        ("track01.bin", 2352),
        ("track02.raw", 2352 * 2),
        ("track03.bin", 2352 * 2),
    ] {
        fs::write(dir.path().join(name), vec![0_u8; len]).unwrap();
    }
    let gdi = dir.path().join("disc.gdi");
    fs::write(
        &gdi,
        "3\n1 0 4 2352 track01.bin 0\n2 150 0 2352 track02.raw 0\n3 45000 4 2352 track03.bin 0\n",
    )
    .unwrap();
    assert!(
        crate::ingestion::gdi::resolve_gdi_all_tracks(&gdi).is_ok(),
        "fixture must be a valid GDI so only conversion support is under test"
    );
    let item = analyse(&gdi, "Sega Dreamcast");
    assert_not_ready(&item, &reviewed_chdman(), "GDI");
    let capability = capability_for_item(&item, &reviewed_chdman());
    assert_ne!(capability.mode.as_deref(), Some("createcd"));
}

#[test]
fn regression_cd_platform_iso_is_never_routed_to_dvd_mode() {
    let dir = tempfile::tempdir().unwrap();
    for platform in [
        "PlayStation",
        "Sega Saturn",
        "Sega Dreamcast",
        "Sega CD",
        "PC Engine CD",
        "Neo Geo CD",
        "3DO",
    ] {
        let iso = dir.path().join(format!("{platform}.iso"));
        fs::write(&iso, vec![0_u8; 2048 * 4]).unwrap();
        let item = analyse(&iso, platform);
        assert_not_ready(&item, &reviewed_chdman(), platform);
        let capability = capability_for_item(&item, &reviewed_chdman());
        assert_ne!(
            capability.mode.as_deref(),
            Some("createdvd"),
            "{platform}: a CD image must never be offered chdman's DVD mode"
        );
        assert_ne!(
            capability.round_trip,
            StorageRoundTripClass::ByteIdenticalExpected,
            "{platform}: no byte-identical CHD route is proven"
        );
    }
}

#[test]
fn regression_ps2_iso_without_media_classification_is_not_conversion_ready() {
    // PS2 shipped on both CD and DVD; nothing here proves which this is.
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("game.iso");
    fs::write(&iso, vec![0_u8; 2048 * 4]).unwrap();
    let item = analyse(&iso, "PlayStation 2");
    assert_not_ready(&item, &reviewed_chdman(), "PS2 ISO");
    let capability = capability_for_item(&item, &reviewed_chdman());
    assert_ne!(capability.mode.as_deref(), Some("createdvd"));
}

#[test]
fn readiness_follows_the_converters_binding_not_the_path_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let item = analyse(&admitted_cue(dir.path()), "PlayStation");

    // A different chdman earlier on PATH does not matter: the converter runs
    // its reviewed binding, and that one is usable.
    let earlier_on_path = inventory(
        Some("/usr/local/bin/chdman"),
        ChdmanBinding::Usable(PathBuf::from("/usr/bin/chdman")),
    );
    let capability = capability_for_item(&item, &earlier_on_path);
    assert_eq!(
        capability.eligibility,
        ConversionEligibility::ConversionReady
    );
    assert_eq!(
        capability.tool_path.as_deref(),
        Some(Path::new("/usr/bin/chdman"))
    );
    assert_eq!(
        capability.tool_version, None,
        "that version was of another copy"
    );
}

#[test]
fn regression_chdman_only_outside_the_reviewed_binding_is_a_missing_tool() {
    let dir = tempfile::tempdir().unwrap();
    let item = analyse(&admitted_cue(dir.path()), "PlayStation");
    let capability = capability_for_item(
        &item,
        &inventory(Some("/opt/tools/bin/chdman"), ChdmanBinding::Absent),
    );
    assert_eq!(capability.eligibility, ConversionEligibility::ToolMissing);
    let detail = capability
        .eligibility_detail
        .expect("say which chdman is needed");
    assert!(detail.contains("/usr/bin/chdman"), "{detail}");
    assert!(
        detail.contains("/opt/tools/bin/chdman is not used"),
        "{detail}"
    );
}

#[test]
fn regression_an_unusable_reviewed_chdman_is_not_conversion_ready() {
    // The PATH lookup accepts a symlink or a non-executable file; the
    // converter does not.
    let dir = tempfile::tempdir().unwrap();
    let item = analyse(&admitted_cue(dir.path()), "PlayStation");
    let capability = capability_for_item(
        &item,
        &inventory(
            Some("/usr/bin/chdman"),
            ChdmanBinding::Unusable("must be a regular non-symlink file".into()),
        ),
    );
    assert_eq!(
        capability.eligibility,
        ConversionEligibility::ReviewRequired
    );
    assert!(
        capability
            .eligibility_detail
            .is_some_and(|d| d.contains("cannot use /usr/bin/chdman"))
    );
}

#[cfg(unix)]
#[test]
fn the_binding_check_is_the_converters_executable_check() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("chdman");
    fs::write(&program, b"not run by this test").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        chdman_binding_at(&program),
        ChdmanBinding::Usable(program.clone())
    );

    let not_executable = dir.path().join("chdman-noexec");
    fs::write(&not_executable, b"").unwrap();
    fs::set_permissions(&not_executable, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        chdman_binding_at(&not_executable),
        ChdmanBinding::Unusable(_)
    ));

    let link = dir.path().join("chdman-link");
    std::os::unix::fs::symlink(&program, &link).unwrap();
    assert!(matches!(
        chdman_binding_at(&link),
        ChdmanBinding::Unusable(_)
    ));

    let dangling = dir.path().join("chdman-dangling");
    std::os::unix::fs::symlink(dir.path().join("gone"), &dangling).unwrap();
    assert_eq!(chdman_binding_at(&dangling), ChdmanBinding::Absent);
    assert_eq!(
        chdman_binding_at(&dir.path().join("missing")),
        ChdmanBinding::Absent
    );
}
