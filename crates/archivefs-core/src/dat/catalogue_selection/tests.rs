//! Focused scenarios for the shared installed-catalogue selector.
//!
//! Every fixture is a temporary directory built in the test; no live user
//! DAT, registry, or managed store is read.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::*;
use crate::dat::sources::DatSourcesConfig;
use crate::dat::updates::{
    ManagedDatSnapshot, ManagedDatSourceDescriptor, ManagedDatState, ManagedDatUpdatePolicy,
    save_managed_dat_state,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn logiqx_dat(platform_name: &str, game: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><datafile><header><name>{platform_name}</name><description>{platform_name}</description></header><game name="{game}"><rom name="{game}.bin" size="4" crc="abcd1234"/></game></datafile>"#
    )
}

fn mame_softwarelist_dat(name: &str, game: &str) -> String {
    format!(
        r#"<softwarelist name="{name}"><software name="{game}"><description>Test</description><year>1997</year><publisher>Test</publisher><part name="cart" interface="cart"><dataarea name="rom" size="1"><rom name="{game}.bin" size="1" crc="00000000"/></dataarea></part></software></softwarelist>"#
    )
}

/// Writes a DAT file and returns its path.
fn write_dat(dir: &Path, file_name: &str, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(file_name);
    fs::write(&path, body).unwrap();
    path
}

#[derive(Default)]
struct LocalEntrySpec {
    id: &'static str,
    display_name: &'static str,
    path: PathBuf,
    platform: Option<&'static str>,
    enabled: bool,
    kind_folder: bool,
    health_valid: bool,
    added: Option<u64>,
}

fn registry(entries: &[LocalEntrySpec]) -> DatSourceRegistry {
    let mut toml = String::new();
    for spec in entries {
        toml.push_str("[[sources]]\n");
        toml.push_str(&format!("id = \"{}\"\n", spec.id));
        toml.push_str(&format!("display_name = \"{}\"\n", spec.display_name));
        toml.push_str(&format!(
            "path = \"{}\"\n",
            spec.path.to_string_lossy().replace('\\', "\\\\")
        ));
        toml.push_str(&format!(
            "kind = \"{}\"\n",
            if spec.kind_folder { "folder" } else { "file" }
        ));
        toml.push_str(&format!("enabled = {}\n", spec.enabled));
        if let Some(platform) = spec.platform {
            toml.push_str(&format!("platform = \"{platform}\"\n"));
        }
        if let Some(added) = spec.added {
            toml.push_str(&format!("added_unix_seconds = {added}\n"));
        }
        if spec.health_valid {
            toml.push_str("health_state = \"valid\"\n");
        }
        toml.push('\n');
    }
    let config: DatSourcesConfig = toml::from_str(&toml).unwrap();
    let (registry, problems) = DatSourceRegistry::from_config(&config);
    assert!(problems.is_empty(), "registry problems: {problems:?}");
    registry
}

/// Installs a MAME software-list managed source whose object bytes really
/// hash to the recorded snapshot digest. Returns its typed id and digest.
fn install_mame(managed_root: &Path, name: &str, body: &str) -> (ManagedDatSourceId, String) {
    let descriptor = ManagedDatSourceDescriptor::mame_software_list(name)
        .unwrap()
        .with_update_policy(ManagedDatUpdatePolicy::Manual);
    let sha = sha256_hex(body.as_bytes());
    let state =
        ManagedDatState::new(&descriptor, ManagedDatSnapshot::new(sha.clone()).unwrap()).unwrap();
    let object_path = managed_root
        .join(state.source_id.storage_relative_path())
        .join("objects")
        .join(&sha);
    fs::create_dir_all(object_path.parent().unwrap()).unwrap();
    fs::write(&object_path, body).unwrap();
    save_managed_dat_state(managed_root, &state).unwrap();
    (descriptor.source_id().clone(), sha)
}

fn mame_sources(names: &[&str]) -> ManagedDatSources {
    let mut sources = ManagedDatSources::new();
    for name in names {
        sources
            .add_mame_software_list(*name, ManagedDatUpdatePolicy::Manual)
            .unwrap();
    }
    sources
}

fn inputs<'a>(
    registry: &'a DatSourceRegistry,
    managed_sources: &'a ManagedDatSources,
    managed_root: &'a Path,
) -> CatalogueInventoryInputs<'a> {
    CatalogueInventoryInputs {
        local_registry: registry,
        managed_sources,
        managed_root,
        limits: DatLimits::default(),
    }
}

// ---------------------------------------------------------------------------
// 1. Enumeration spans local + managed stores
// ---------------------------------------------------------------------------

#[test]
fn enumeration_spans_local_and_managed_stores() {
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(
        temp.path(),
        "nes.dat",
        &logiqx_dat("Nintendo NES", "Game 1"),
    );
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES (local)",
        path: dat,
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    install_mame(
        &managed_root,
        "gamecom",
        &mame_softwarelist_dat("gamecom", "Foo"),
    );
    let sources = mame_sources(&["gamecom"]);

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));

    assert_eq!(rows.len(), 2, "one local + one managed row: {rows:#?}");
    assert!(rows.iter().any(|r| r.store == CatalogueStore::LocalRegistry
        && matches!(r.reference, CatalogueRef::Local { .. })));
    assert!(
        rows.iter()
            .any(|r| r.store == CatalogueStore::ManagedMameSoftwareList
                && matches!(r.reference, CatalogueRef::ManagedCurrent { .. }))
    );
}

// ---------------------------------------------------------------------------
// 2. Deterministic ordering
// ---------------------------------------------------------------------------

#[test]
fn enumeration_is_deterministically_ordered() {
    let temp = tempfile::tempdir().unwrap();
    let a = write_dat(temp.path(), "a.dat", &logiqx_dat("Sega Saturn", "A"));
    let b = write_dat(temp.path(), "b.dat", &logiqx_dat("Nintendo NES", "B"));
    let c = write_dat(temp.path(), "c.dat", &logiqx_dat("Nintendo NES", "C"));
    // Declared in a deliberately unsorted order.
    let reg = registry(&[
        LocalEntrySpec {
            id: "zeta",
            display_name: "Saturn",
            path: a,
            platform: Some("saturn"),
            enabled: true,
            health_valid: true,
            ..Default::default()
        },
        LocalEntrySpec {
            id: "alpha",
            display_name: "NES two",
            path: b.clone(),
            platform: Some("nes"),
            enabled: true,
            health_valid: true,
            ..Default::default()
        },
        LocalEntrySpec {
            id: "beta",
            display_name: "NES one",
            path: c,
            platform: Some("nes"),
            enabled: true,
            health_valid: true,
            ..Default::default()
        },
    ]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let first: Vec<_> = list_installed_catalogues(inputs(&reg, &sources, &managed_root))
        .into_iter()
        .map(|r| r.reference.token())
        .collect();
    let second: Vec<_> = list_installed_catalogues(inputs(&reg, &sources, &managed_root))
        .into_iter()
        .map(|r| r.reference.token())
        .collect();

    assert_eq!(first, second, "ordering must be stable across calls");
    // NES rows precede the Saturn row (platform is the primary key), and
    // the two NES rows are ordered by display name, not declaration order.
    assert_eq!(
        first,
        vec![
            "local:beta".to_string(),
            "local:alpha".to_string(),
            "local:zeta".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// 3. Same platform / different store + ecosystem stay distinct
// ---------------------------------------------------------------------------

#[test]
fn same_platform_different_store_and_ecosystem_stay_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES (Logiqx)",
        path: dat,
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    install_mame(&managed_root, "nes", &mame_softwarelist_dat("nes", "Foo"));
    let sources = mame_sources(&["nes"]);

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));

    assert_eq!(rows.len(), 2, "not collapsed by shared platform: {rows:#?}");
    let stores: Vec<_> = rows.iter().map(|r| r.store).collect();
    assert!(stores.contains(&CatalogueStore::LocalRegistry));
    assert!(stores.contains(&CatalogueStore::ManagedMameSoftwareList));
    // Distinct ecosystem evidence: the managed row is a confirmed MAME
    // software list; the local row has not been inspected.
    let managed = rows
        .iter()
        .find(|r| r.store == CatalogueStore::ManagedMameSoftwareList)
        .unwrap();
    assert_eq!(
        managed.ecosystem,
        EvidenceValue::Confirmed(DatEcosystem::MAMESoftwareList)
    );
    let local = rows
        .iter()
        .find(|r| r.store == CatalogueStore::LocalRegistry)
        .unwrap();
    assert_eq!(local.ecosystem, EvidenceValue::Unknown);
    assert_ne!(local.reference, managed.reference);
}

// ---------------------------------------------------------------------------
// 4. Same platform / two local variants stay distinct
// ---------------------------------------------------------------------------

#[test]
fn same_platform_two_local_variants_stay_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let headered = write_dat(temp.path(), "h.dat", &logiqx_dat("nes", "H"));
    let headerless = write_dat(temp.path(), "hl.dat", &logiqx_dat("nes", "HL"));
    let reg = registry(&[
        LocalEntrySpec {
            id: "nes-headered",
            display_name: "NES (Headered)",
            path: headered,
            platform: Some("nes"),
            enabled: true,
            health_valid: true,
            ..Default::default()
        },
        LocalEntrySpec {
            id: "nes-headerless",
            display_name: "NES (Headerless)",
            path: headerless,
            platform: Some("nes"),
            enabled: true,
            health_valid: true,
            ..Default::default()
        },
    ]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));

    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].reference, rows[1].reference);
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(&r.platform, EvidenceValue::Assigned(p) if p == "Nintendo Entertainment System"
                || p == "nes"))
            .count(),
        2,
        "both rows still describe the same platform without being merged"
    );
}

// ---------------------------------------------------------------------------
// 5. Missing backing file -> unavailable, not omitted
// ---------------------------------------------------------------------------

#[test]
fn missing_local_backing_file_is_unavailable_not_omitted() {
    let temp = tempfile::tempdir().unwrap();
    let present = write_dat(temp.path(), "ok.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[
        LocalEntrySpec {
            id: "present",
            display_name: "Present",
            path: present,
            platform: Some("nes"),
            enabled: true,
            health_valid: true,
            ..Default::default()
        },
        LocalEntrySpec {
            id: "gone",
            display_name: "Gone",
            path: temp.path().join("does-not-exist.dat"),
            platform: Some("snes"),
            enabled: true,
            health_valid: true,
            ..Default::default()
        },
    ]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));

    assert_eq!(rows.len(), 2, "the broken row is represented, not dropped");
    let gone = rows
        .iter()
        .find(|r| r.reference == CatalogueRef::local("gone"))
        .unwrap();
    assert!(matches!(
        gone.availability,
        CatalogueAvailability::Missing { .. }
    ));
    assert!(!gone.capabilities.single_catalogue_1g1r);
    // ...and the healthy row alongside it is unaffected.
    let present = rows
        .iter()
        .find(|r| r.reference == CatalogueRef::local("present"))
        .unwrap();
    assert!(present.availability.is_ready());
}

// ---------------------------------------------------------------------------
// 6. Resolve rejects an expected-hash mismatch
// ---------------------------------------------------------------------------

#[test]
fn resolve_rejects_expected_hash_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let managed_root = temp.path().join("managed");
    let (source_id, _real) = install_mame(
        &managed_root,
        "gamecom",
        &mame_softwarelist_dat("gamecom", "Foo"),
    );
    let reg = DatSourceRegistry::new();
    let sources = mame_sources(&["gamecom"]);

    let wrong = "0".repeat(64);
    let reference = CatalogueRef::managed_current(source_id, wrong.clone());
    let err = resolve_catalogue(&reference, inputs(&reg, &sources, &managed_root)).unwrap_err();

    match err {
        CatalogueResolveError::SnapshotHashMismatch { expected, .. } => {
            assert_eq!(expected, wrong);
        }
        other => panic!("expected SnapshotHashMismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 7. Resolve rejects changed snapshot bytes
// ---------------------------------------------------------------------------

#[test]
fn resolve_rejects_changed_snapshot_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let managed_root = temp.path().join("managed");
    let (source_id, sha) = install_mame(
        &managed_root,
        "gamecom",
        &mame_softwarelist_dat("gamecom", "Foo"),
    );
    let reg = DatSourceRegistry::new();
    let sources = mame_sources(&["gamecom"]);
    let reference = CatalogueRef::managed_current(source_id.clone(), sha.clone());

    // Sanity: it resolves before tampering.
    resolve_catalogue(&reference, inputs(&reg, &sources, &managed_root)).unwrap();

    // Overwrite the immutable object's bytes, keeping the digest-named path.
    let object_path = managed_root
        .join(source_id.storage_relative_path())
        .join("objects")
        .join(&sha);
    fs::write(
        &object_path,
        mame_softwarelist_dat("gamecom", "Tampered With"),
    )
    .unwrap();

    let err = resolve_catalogue(&reference, inputs(&reg, &sources, &managed_root)).unwrap_err();
    match err {
        CatalogueResolveError::SnapshotHashMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, sha);
            assert_ne!(actual, sha);
        }
        other => panic!("expected SnapshotHashMismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 8. Resolve rejects a corrupt catalogue
// ---------------------------------------------------------------------------

#[test]
fn resolve_rejects_corrupt_catalogue() {
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "broken.dat", "<datafile><this is not xml");
    let reg = registry(&[LocalEntrySpec {
        id: "broken",
        display_name: "Broken",
        path: dat,
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let err = resolve_catalogue(
        &CatalogueRef::local("broken"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap_err();
    assert!(
        matches!(err, CatalogueResolveError::CorruptCatalogue { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. Stale managed state -> unavailable, and resolve fails closed
// ---------------------------------------------------------------------------

#[test]
fn stale_managed_state_is_unavailable_and_resolve_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let managed_root = temp.path().join("managed");
    let (source_id, sha) = install_mame(
        &managed_root,
        "gamecom",
        &mame_softwarelist_dat("gamecom", "Foo"),
    );
    // Remove the object the state points at: state.json now references a
    // snapshot that is not on disk.
    let object_path = managed_root
        .join(source_id.storage_relative_path())
        .join("objects")
        .join(&sha);
    fs::remove_file(&object_path).unwrap();

    let reg = DatSourceRegistry::new();
    let sources = mame_sources(&["gamecom"]);

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));
    assert_eq!(rows.len(), 1, "the row is still listed");
    assert!(matches!(
        rows[0].availability,
        CatalogueAvailability::StaleManagedState { .. }
    ));
    assert!(!rows[0].capabilities.single_catalogue_1g1r);

    let err = resolve_catalogue(
        &CatalogueRef::managed_current(source_id, sha),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap_err();
    assert!(
        matches!(err, CatalogueResolveError::StaleManagedState { .. }),
        "got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 10. Ambiguous candidates -> typed ambiguity carrying summaries
// ---------------------------------------------------------------------------

#[test]
fn resolve_for_platform_returns_ambiguity_with_candidate_summaries() {
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES (Logiqx)",
        path: dat,
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    install_mame(&managed_root, "nes", &mame_softwarelist_dat("nes", "Foo"));
    let sources = mame_sources(&["nes"]);

    let err =
        resolve_catalogue_for_platform("nes", inputs(&reg, &sources, &managed_root)).unwrap_err();
    match err {
        CatalogueResolveError::MultipleCandidates {
            platform,
            candidates,
        } => {
            assert_eq!(platform, "nes");
            assert_eq!(candidates.len(), 2);
            // The caller is handed real summaries to choose between - not
            // an opaque count.
            assert!(
                candidates
                    .iter()
                    .any(|c| c.store == CatalogueStore::LocalRegistry)
            );
            assert!(
                candidates
                    .iter()
                    .any(|c| c.store == CatalogueStore::ManagedMameSoftwareList)
            );
        }
        other => panic!("expected MultipleCandidates, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 11. Explicit reference resolves deterministically
// ---------------------------------------------------------------------------

#[test]
fn explicit_reference_resolves_deterministically() {
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES",
        path: dat.clone(),
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let a = resolve_catalogue(
        &CatalogueRef::local("local-nes"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap();
    let b = resolve_catalogue(
        &CatalogueRef::local("local-nes"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap();

    assert_eq!(a.backing_path, dat);
    assert_eq!(a.source_id, b.source_id);
    assert_eq!(a.parsed.games.len(), b.parsed.games.len());
    assert_eq!(a.reference, CatalogueRef::local("local-nes"));
    assert_eq!(a.assigned_platform.as_deref(), Some("nes"));
}

// ---------------------------------------------------------------------------
// 12. A resolved reference feeds the Build Playing Library planner
// ---------------------------------------------------------------------------

#[test]
fn resolved_reference_feeds_playing_library_planner() {
    use crate::playing_library::{
        DatArchiveMatch, PlayingLibraryPolicy, build_playing_library_plan,
    };

    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES",
        path: dat,
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let resolved = resolve_catalogue(
        &CatalogueRef::local("local-nes"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap();

    let request = resolved.playing_library_request(
        vec![DatArchiveMatch {
            archive_path: PathBuf::from("/library/Game 1.bin"),
            dat_entry_index: 0,
            companion_paths: vec![],
        }],
        PathBuf::from("/playing"),
        PlayingLibraryPolicy::default(),
    );
    let plan = build_playing_library_plan(&request).unwrap();
    assert_eq!(plan.archives_examined, 1);
}

// ---------------------------------------------------------------------------
// 13. 1G1R election is byte-for-byte identical via resolver vs raw parse
// ---------------------------------------------------------------------------

#[test]
fn one_g_one_r_election_is_identical_via_resolver_and_via_raw_parse() {
    use crate::dat::parsers::parse_dat_file;
    use crate::playing_library::{
        DatArchiveMatch, PlayingLibraryPolicy, PlayingLibraryRequest, build_playing_library_plan,
    };

    let temp = tempfile::tempdir().unwrap();
    let body = logiqx_dat("nes", "Game 1");
    let dat = write_dat(temp.path(), "nes.dat", &body);
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES",
        path: dat.clone(),
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let matches = vec![DatArchiveMatch {
        archive_path: PathBuf::from("/library/Game 1.bin"),
        dat_entry_index: 0,
        companion_paths: vec![],
    }];

    // Path-based caller: parse the file directly.
    let raw = parse_dat_file(&dat, DatLimits::default()).unwrap().dat;
    let via_path = build_playing_library_plan(&PlayingLibraryRequest {
        dat: &raw,
        matches: matches.clone(),
        destination_root: PathBuf::from("/playing"),
        policy: PlayingLibraryPolicy::default(),
    })
    .unwrap();

    // Reference-based caller: go through the resolver.
    let resolved = resolve_catalogue(
        &CatalogueRef::local("local-nes"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap();
    let via_reference = build_playing_library_plan(&resolved.playing_library_request(
        matches,
        PathBuf::from("/playing"),
        PlayingLibraryPolicy::default(),
    ))
    .unwrap();

    assert_eq!(
        via_path, via_reference,
        "the catalogue-selection seam must not change election output"
    );
}

// ---------------------------------------------------------------------------
// 14. Verify backend consumes the same resolved reference
// ---------------------------------------------------------------------------

#[test]
fn verify_backend_consumes_the_same_resolved_reference() {
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES catalogue",
        path: dat.clone(),
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let resolved = resolve_catalogue(
        &CatalogueRef::local("local-nes"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap();

    let scan_root = temp.path().join("roms");
    let audit = resolved.to_dat_audit_request(scan_root.clone(), DatLimits::default(), None);
    assert_eq!(audit.dat_path, dat);
    assert_eq!(audit.dat_kind, DatSourceKind::File);
    assert_eq!(audit.scan_root, scan_root);
    assert_eq!(audit.source_id, resolved.source_id);
    assert_eq!(audit.platform.as_deref(), Some("nes"));

    let combined = resolved.to_combined_dat_audit_source();
    assert_eq!(combined.dat_path, dat);
    assert_eq!(combined.source_display_name, "NES catalogue");
}

// ---------------------------------------------------------------------------
// 15. Repair integration keeps the existing request shape and safety inputs
// ---------------------------------------------------------------------------

#[test]
fn repair_integration_preserves_existing_request_shape() {
    use crate::dat::sources::audit_cache::AuditCacheConfig;
    use crate::repair::library::RepairProfile;

    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES catalogue",
        path: dat.clone(),
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let resolved = resolve_catalogue(
        &CatalogueRef::local("local-nes"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap();

    let scan_root = temp.path().join("roms");
    let scan = resolved.to_library_scan_request(
        scan_root.clone(),
        DatLimits::default(),
        RepairProfile::CanonicalInPlace,
        AuditCacheConfig::Disabled,
    );
    assert_eq!(scan.dat_path, dat);
    assert_eq!(scan.dat_kind, DatSourceKind::File);
    assert_eq!(scan.scan_root, scan_root);
    assert_eq!(scan.profile, RepairProfile::CanonicalInPlace);
    assert_eq!(scan.source_id, resolved.source_id);
}

// ---------------------------------------------------------------------------
// 16. Logical identity, not a raw path
// ---------------------------------------------------------------------------

#[test]
fn catalogue_ref_is_logical_not_path_based() {
    let temp = tempfile::tempdir().unwrap();
    let managed_root = temp.path().join("managed");
    let (source_id, sha) = install_mame(
        &managed_root,
        "gamecom",
        &mame_softwarelist_dat("gamecom", "Foo"),
    );
    let reg = DatSourceRegistry::new();
    let sources = mame_sources(&["gamecom"]);

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));
    let row = &rows[0];

    // The reference encodes provider + source key + snapshot digest, and
    // carries no filesystem path.
    match &row.reference {
        CatalogueRef::ManagedCurrent {
            source_id: id,
            snapshot_sha256,
        } => {
            assert_eq!(id, &source_id);
            assert_eq!(snapshot_sha256, &sha);
        }
        other => panic!("expected a managed reference, got {other:?}"),
    }
    let token = row.reference.token();
    assert!(!token.contains('/'), "token must not embed a path: {token}");
    assert!(token.starts_with("managed:mame:gamecom@"));

    // The path is retained, but only as technical metadata.
    assert!(row.technical_path.is_some());
    assert!(
        row.technical_path
            .as_ref()
            .unwrap()
            .starts_with(&managed_root)
    );
}

// ---------------------------------------------------------------------------
// 17. Extra fail-closed guards: aggregate folder + disabled + dedup
// ---------------------------------------------------------------------------

#[test]
fn folder_source_is_aggregate_and_not_a_single_catalogue() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("dats");
    write_dat(&folder, "one.dat", &logiqx_dat("nes", "A"));
    let reg = registry(&[LocalEntrySpec {
        id: "folder-src",
        display_name: "A folder of DATs",
        path: folder,
        platform: Some("nes"),
        enabled: true,
        kind_folder: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));
    assert!(matches!(
        rows[0].availability,
        CatalogueAvailability::AggregateFolder { .. }
    ));
    assert!(!rows[0].capabilities.single_catalogue_1g1r);
    assert!(!rows[0].capabilities.repair);

    let err = resolve_catalogue(
        &CatalogueRef::local("folder-src"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            CatalogueResolveError::AggregateFolderNotSingleCatalogue { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn disabled_local_source_is_visible_but_not_resolvable() {
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "off",
        display_name: "Disabled",
        path: dat,
        platform: Some("nes"),
        enabled: false,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));
    assert_eq!(rows.len(), 1, "still shown for management");
    assert!(!rows[0].enabled);
    assert!(!rows[0].capabilities.single_catalogue_1g1r);

    let err = resolve_catalogue(
        &CatalogueRef::local("off"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap_err();
    assert!(
        matches!(err, CatalogueResolveError::Disabled { .. }),
        "got {err:?}"
    );
}

#[test]
fn unknown_reference_fails_closed() {
    let reg = DatSourceRegistry::new();
    let sources = ManagedDatSources::new();
    let managed_root = PathBuf::from("/nonexistent-managed-root");

    let err = resolve_catalogue(
        &CatalogueRef::local("nope"),
        inputs(&reg, &sources, &managed_root),
    )
    .unwrap_err();
    assert!(
        matches!(err, CatalogueResolveError::UnknownReference { .. }),
        "got {err:?}"
    );
}

#[test]
fn enumeration_deduplicates_by_reference() {
    // Two registry files cannot legally hold the same id, but the
    // enumerator must still guarantee no duplicate reference reaches a
    // caller even if a future store overlaps.
    let temp = tempfile::tempdir().unwrap();
    let dat = write_dat(temp.path(), "nes.dat", &logiqx_dat("nes", "Game 1"));
    let reg = registry(&[LocalEntrySpec {
        id: "local-nes",
        display_name: "NES",
        path: dat,
        platform: Some("nes"),
        enabled: true,
        health_valid: true,
        ..Default::default()
    }]);
    let managed_root = temp.path().join("managed");
    let sources = ManagedDatSources::new();

    let rows = list_installed_catalogues(inputs(&reg, &sources, &managed_root));
    let mut refs: Vec<_> = rows.iter().map(|r| r.reference.clone()).collect();
    let before = refs.len();
    refs.dedup();
    assert_eq!(before, refs.len(), "no duplicate references");
}
