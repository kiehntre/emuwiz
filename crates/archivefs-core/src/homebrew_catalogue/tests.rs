use super::*;
use crate::homebrew_artifact::{
    ArtifactChecksumAlgorithm, DigestProvenance, PlatformClaimEvidence, RightsFact,
};
use serde_json::{Value, json};

fn id(provider: &str, project: &str) -> HomebrewProjectId {
    HomebrewProjectId {
        provider: HomebrewProviderId::new(provider).unwrap(),
        provider_id: project.into(),
    }
}

fn build(asset: &str, filename: &str, platform: Option<&str>) -> Value {
    json!({
        "provider_asset_id": asset, "filename": filename,
        "size_bytes": null, "provider_digest": null,
        "acquisition": {
            "source_page_url": "https://example.org/project",
            "link_url": "https://example.org/game.nes", "mode": "browser_required"
        },
        "platform_claims": platform.map(|p| vec![json!({"platform": p,
            "evidence": {"release_metadata": "https://example.org/project; developer claim, unverified"}})]).unwrap_or_default()
    })
}

fn record(provider: &str, project: &str, title: &str) -> Value {
    json!({
        "project": {"id": {"provider": provider, "provider_id": project},
            "title": title, "summary": "A synthetic puzzle platformer",
            "source_page_url": "https://example.org/project"},
        "listings": [{"project": {"provider": provider, "provider_id": project},
            "release": null, "artifacts": [build("standard", "game.nes", Some("NES"))]}],
        "observations": [{"source_url": "https://example.org/project",
            "observed_on": "2026-10-11", "note": "Synthetic metadata; no game bytes."}]
    })
}

fn catalogue(provider: &str, records: Vec<Value>) -> Value {
    json!({"schema_version": 1, "provider": provider, "records": records})
}

fn load(value: &Value) -> Result<LocalCatalogueProvider, LocalCatalogueError> {
    LocalCatalogueProvider::from_json(&serde_json::to_vec(value).unwrap())
}

#[test]
fn exact_partial_and_description_search() {
    let provider = load(&catalogue(
        "local",
        vec![record("local", "1", "Moon Garden")],
    ))
    .unwrap();
    for query in ["Moon Garden", "Garden", "puzzle", "", "  "] {
        assert_eq!(
            provider.discover_projects(query).unwrap()[0].id,
            id("local", "1")
        );
    }
    assert!(provider.discover_projects("unrelated").unwrap().is_empty());
}

#[test]
fn case_insensitive_unicode_search() {
    let provider = load(&catalogue(
        "local",
        vec![record("local", "1", "Äventyr Мир 猫 Straße ΟΣ Café")],
    ))
    .unwrap();
    for query in [
        "ÄVENTYR",
        "äventyr",
        "мир",
        "МИР",
        "猫",
        "STRASSE",
        "οσ",
        "ος",
        "CAFE\u{301}",
    ] {
        assert_eq!(
            provider.discover_projects(query).unwrap().len(),
            1,
            "{query}"
        );
    }
}

#[test]
fn filters_only_explicit_canonical_platform_claims() {
    let mut gb = record("local", "gb", "Tiny Quest");
    gb["listings"][0]["artifacts"] = json!([build("gb", "game.gb", Some("Game Boy"))]);
    let provider = load(&catalogue(
        "local",
        vec![record("local", "nes", "Tiny Quest"), gb],
    ))
    .unwrap();
    assert_eq!(
        provider.discover_for_platform("quest", "NES").unwrap()[0]
            .id
            .provider_id,
        "nes"
    );
    assert_eq!(
        provider.discover_for_platform("quest", "Game Boy").unwrap()[0]
            .id
            .provider_id,
        "gb"
    );
    assert!(matches!(
        provider.discover_for_platform("", "gb"),
        Err(LocalCatalogueError::UnsupportedPlatform(_))
    ));
}

#[test]
fn multiple_builds_releases_and_equal_filenames_remain_distinct() {
    let mut project = record("local", "1", "Two Builds");
    project["listings"][0]["artifacts"] = json!([
        build("pal", "game.nes", Some("NES")),
        build("ntsc", "game.nes", Some("NES"))
    ]);
    let mut newer = project["listings"][0].clone();
    newer["release"] = json!({"provider_release_id":"v2", "version":"2.0"});
    newer["artifacts"][0]["provider_asset_id"] = json!("v2-pal");
    newer["artifacts"][1]["provider_asset_id"] = json!("v2-ntsc");
    project["listings"].as_array_mut().unwrap().push(newer);
    let provider = load(&catalogue("local", vec![project])).unwrap();
    assert_eq!(provider.discover_projects("builds").unwrap().len(), 1);
    let releases = provider.artifact_releases(&id("local", "1")).unwrap();
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[0].artifacts.len(), 2);
    assert_ne!(
        releases[0].artifacts[0].provider_asset_id,
        releases[0].artifacts[1].provider_asset_id
    );
    assert_eq!(
        releases[1].release.as_ref().unwrap().version.as_deref(),
        Some("2.0")
    );
}

#[test]
fn same_titles_across_providers_never_merge_or_create_links() {
    let a = load(&catalogue(
        "local-a",
        vec![record("local-a", "1", "Moon Garden")],
    ))
    .unwrap();
    let b = load(&catalogue(
        "local-b",
        vec![record("local-b", "1", "Moon Garden")],
    ))
    .unwrap();
    let a_project = &a.discover_projects("").unwrap()[0];
    let b_project = &b.discover_projects("").unwrap()[0];
    assert_ne!(a_project.id, b_project.id);
    assert!(a_project.links.is_empty());
    assert!(matches!(
        a.artifact_releases(&b_project.id),
        Err(LocalCatalogueError::WrongProvider(_))
    ));
    assert!(matches!(
        a.record(&id("local-a", "missing")),
        Err(LocalCatalogueError::ProjectNotFound(_))
    ));
}

#[test]
fn exact_duplicates_collapse_but_conflicting_project_ids_fail() {
    let entry = record("local", "1", "Moon Garden");
    let provider = load(&catalogue("local", vec![entry.clone(), entry.clone()])).unwrap();
    assert_eq!(provider.discover_projects("").unwrap().len(), 1);
    for field in ["title", "summary", "source_page_url"] {
        let mut conflict = entry.clone();
        conflict["project"][field] = json!(if field == "source_page_url" {
            "https://example.org/other"
        } else {
            "Different"
        });
        let error = load(&catalogue("local", vec![entry.clone(), conflict]))
            .err()
            .unwrap();
        assert!(matches!(error, LocalCatalogueError::Conflict { .. }));
        assert!(error.to_string().contains("1"));
    }
}

#[test]
fn duplicate_build_and_release_ids_are_checked_without_merging() {
    let mut entry = record("local", "1", "Duplicate");
    let first = entry["listings"][0]["artifacts"][0].clone();
    entry["listings"][0]["artifacts"]
        .as_array_mut()
        .unwrap()
        .push(first.clone());
    let provider = load(&catalogue("local", vec![entry.clone()])).unwrap();
    assert_eq!(
        provider.record(&id("local", "1")).unwrap().listings[0]
            .artifacts
            .len(),
        1
    );
    entry["listings"][0]["artifacts"][1]["filename"] = json!("different.nes");
    assert!(matches!(
        load(&catalogue("local", vec![entry])).err().unwrap(),
        LocalCatalogueError::Conflict { .. }
    ));
    let mut entry = record("local", "1", "Duplicate");
    let first = entry["listings"][0].clone();
    entry["listings"].as_array_mut().unwrap().push(first);
    assert_eq!(
        load(&catalogue("local", vec![entry.clone()]))
            .unwrap()
            .record(&id("local", "1"))
            .unwrap()
            .listings
            .len(),
        1
    );
    entry["listings"][1]["artifacts"][0]["filename"] = json!("conflict.nes");
    assert!(matches!(
        load(&catalogue("local", vec![entry])).err().unwrap(),
        LocalCatalogueError::Conflict { .. }
    ));
}

#[test]
fn unknown_rights_stay_unknown_and_code_licence_does_not_license_builds() {
    let mut entry = record("local", "1", "Free Game");
    entry["project"]["rights"] = json!({"software_licence":{"known":{"value":"MIT", "evidence":"https://example.org/LICENSE; code only"}}});
    let provider = load(&catalogue("local", vec![entry])).unwrap();
    let stored = provider.record(&id("local", "1")).unwrap();
    assert!(matches!(
        stored.project.rights.software_licence,
        RightsFact::Known { .. }
    ));
    assert_eq!(stored.project.rights.asset_licence, RightsFact::Unknown);
    assert_eq!(stored.project.rights.redistribution, RightsFact::Unknown);
    assert_eq!(
        stored.listings[0].artifacts[0].rights.software_licence,
        RightsFact::Unknown
    );
    assert_eq!(
        stored.listings[0].artifacts[0].rights.redistribution,
        RightsFact::Unknown
    );
}

#[test]
fn incomplete_platform_evidence_never_comes_from_filename_or_project_title() {
    let mut entry = record("local", "1", "NES Game");
    entry["listings"][0]["artifacts"] = json!([
        build("unknown", "game.nes", None),
        build("alias", "game.nes", Some("unregistered-provider-name"))
    ]);
    let provider = load(&catalogue("local", vec![entry])).unwrap();
    assert_eq!(provider.discover_projects("").unwrap().len(), 1);
    assert!(
        provider
            .discover_for_platform("", "NES")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        provider.record(&id("local", "1")).unwrap().listings[0].artifacts[0].platform_claims[0]
            .registered_platform(),
        None
    );
}

#[test]
fn malformed_records_report_precise_fields() {
    let cases = [
        ("/project/title", json!("  "), "title"),
        ("/project/id/provider", json!("other"), "provider"),
        ("/project/id/provider_id", json!(""), "provider_id"),
        (
            "/project/source_page_url",
            json!("javascript:alert(1)"),
            "source_page_url",
        ),
        ("/listings/0/project/provider_id", json!("other"), "project"),
        (
            "/listings/0/artifacts/0/provider_asset_id",
            json!(""),
            "provider_asset_id",
        ),
        (
            "/listings/0/artifacts/0/platform_claims/0/evidence/release_metadata",
            json!(""),
            "evidence",
        ),
        (
            "/observations/0/observed_on",
            json!("2026-02-30"),
            "observed_on",
        ),
        ("/observations", json!([]), "observations"),
    ];
    for (pointer, value, expected) in cases {
        let mut entry = record("local", "1", "Bad entry");
        *entry.pointer_mut(pointer).unwrap() = value;
        let error = load(&catalogue("local", vec![entry]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("records[0]"), "{error}");
        assert!(error.contains(expected), "{pointer}: {error}");
    }
}

#[test]
fn unknown_fields_and_schema_are_refused_instead_of_silently_dropped() {
    let mut entry = record("local", "1", "Typo");
    entry["project"]["rights"] = json!({"asset_license":"MIT"});
    assert!(
        load(&catalogue("local", vec![entry]))
            .err()
            .unwrap()
            .to_string()
            .contains("asset_license")
    );
    let mut empty = catalogue("local", vec![]);
    empty["provider"] = json!("");
    assert!(load(&empty).is_err());
    empty = catalogue("local", vec![]);
    empty["schema_version"] = json!(2);
    assert!(matches!(
        load(&empty),
        Err(LocalCatalogueError::UnsupportedSchema(2))
    ));
    for bytes in [
        b"{broken".as_slice(),
        b"{\"schema_version\":1,\"provider\":\"local\",\"records\":[{}]}".as_slice(),
    ] {
        assert!(matches!(
            LocalCatalogueProvider::from_json(bytes),
            Err(LocalCatalogueError::Malformed(_))
        ));
    }
}

#[test]
fn duplicate_json_fields_missing_release_ids_and_blank_rights_evidence_fail() {
    let duplicate = br#"{"schema_version":1,"schema_version":1,"provider":"local","records":[]}"#;
    assert!(
        LocalCatalogueProvider::from_json(duplicate)
            .err()
            .unwrap()
            .to_string()
            .contains("duplicate field")
    );
    let mut entry = record("local", "1", "Versioned");
    entry["listings"][0]["release"] = json!({"version":"1.0"});
    assert!(
        load(&catalogue("local", vec![entry]))
            .err()
            .unwrap()
            .to_string()
            .contains("provider_release_id")
    );
    let mut entry = record("local", "1", "Rights");
    entry["project"]["rights"] =
        json!({"redistribution":{"known":{"value":"permitted","evidence":""}}});
    assert!(
        load(&catalogue("local", vec![entry]))
            .err()
            .unwrap()
            .to_string()
            .contains("redistribution.evidence")
    );
}

#[test]
fn equivalent_reordered_metadata_duplicates_collapse_and_links_keep_provenance() {
    let mut entry = record("local", "1", "Evidence");
    entry["project"]["links"] = json!([{"project":{"provider":"developer","provider_id":"stable-id"},"evidence":"https://example.org/project; author explicitly names the external record"}]);
    let other = json!({"source_url":"https://example.org/other","observed_on":"2026-10-10","note":"Second source"});
    entry["observations"].as_array_mut().unwrap().push(other);
    let mut reordered = entry.clone();
    reordered["observations"].as_array_mut().unwrap().reverse();
    let provider = load(&catalogue("local", vec![entry, reordered])).unwrap();
    assert_eq!(provider.discover_projects("").unwrap().len(), 1);
    let stored = provider.record(&id("local", "1")).unwrap();
    assert_eq!(stored.observations.len(), 2);
    assert_eq!(
        stored.project.links[0].project,
        id("developer", "stable-id")
    );
    assert!(
        stored.project.links[0]
            .evidence
            .contains("author explicitly")
    );
}

#[test]
fn checked_in_starter_corpus_preserves_scoped_builds_rights_and_confidence() {
    let provider = LocalCatalogueProvider::from_json(include_bytes!(
        "../../tests/fixtures/homebrew/starter-catalogue.json"
    ))
    .unwrap();
    assert_eq!(provider.discover_projects("").unwrap().len(), 3);
    assert_eq!(provider.discover_for_platform("", "NES").unwrap().len(), 2);
    assert_eq!(
        provider
            .discover_for_platform("tobu", "Game Boy")
            .unwrap()
            .len(),
        1
    );
    let tobu = provider
        .record(&id("emuwiz-starter", "tobu-tobu-girl"))
        .unwrap();
    assert!(
        matches!(&tobu.project.rights.software_licence,RightsFact::Known{value,..} if value=="MIT")
    );
    assert!(
        matches!(&tobu.project.rights.asset_licence,RightsFact::Known{value,..} if value=="CC-BY-4.0")
    );
    let nova = provider
        .record(&id("emuwiz-starter", "nova-the-squirrel"))
        .unwrap();
    assert_eq!(
        nova.listings[0]
            .release
            .as_ref()
            .unwrap()
            .version
            .as_deref(),
        Some("1.0.6a")
    );
    let tilt = provider
        .record(&id("emuwiz-starter", "super-tilt-bro"))
        .unwrap();
    assert_eq!(tilt.listings[0].artifacts.len(), 2);
    assert_eq!(tilt.project.rights.asset_licence, RightsFact::Unknown);
    for project in provider.discover_projects("").unwrap() {
        let stored = provider.record(&project.id).unwrap();
        assert_eq!(project.rights.redistribution, RightsFact::Unknown);
        assert!(
            stored
                .observations
                .iter()
                .all(|o| o.observed_on == "2026-10-11" && o.source_url.starts_with("https://"))
        );
        for artifact in stored.listings.iter().flat_map(|l| &l.artifacts) {
            assert!(artifact.provider_digest.is_none());
            assert!(artifact.acquisition.link_url.is_none());
            assert!(artifact.platform_claims.iter().all(|p| matches!(&p.evidence,PlatformClaimEvidence::ReleaseMetadata(note) if note.contains("unverified developer claim"))));
        }
    }
}

#[cfg(unix)]
#[test]
fn unreadable_catalogue_and_fifo_are_refused_without_fallback_or_waiting() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("unreadable.json");
    std::fs::write(&path, b"{}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
    // SAFETY: geteuid takes no arguments and only reads process identity.
    if unsafe { libc::geteuid() } != 0 {
        assert!(
            matches!(LocalCatalogueProvider::from_path(&path),Err(LocalCatalogueError::Io{source,..}) if source.kind()==std::io::ErrorKind::PermissionDenied)
        );
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let fifo = temp.path().join("catalogue.pipe");
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: NUL-terminated path is inside the exclusively owned TempDir.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        LocalCatalogueProvider::from_path(fifo),
        Err(LocalCatalogueError::NotRegularFile(_))
    ));
}

#[test]
fn provider_checksums_and_claim_provenance_survive_without_identity_promotion() {
    let mut entry = record("local", "1", "Digest");
    entry["listings"][0]["artifacts"][0]["provider_digest"] =
        json!({"algorithm":"sha256", "value":"ab".repeat(32)});
    let provider = load(&catalogue("local", vec![entry.clone()])).unwrap();
    let stored = provider.record(&id("local", "1")).unwrap();
    let artifact = &stored.listings[0].artifacts[0];
    assert_eq!(
        artifact.provider_digest_evidence().unwrap().provenance,
        DigestProvenance::ProviderComputed
    );
    assert_eq!(
        artifact.provider_digest.as_ref().unwrap().algorithm,
        ArtifactChecksumAlgorithm::Sha256
    );
    assert!(matches!(
        artifact.platform_claims[0].evidence,
        PlatformClaimEvidence::ReleaseMetadata(_)
    ));
    assert_eq!(stored.observations[0].observed_on, "2026-10-11");
    assert_eq!(
        stored.project.source_page_url,
        "https://example.org/project"
    );
    entry["listings"][0]["artifacts"][0]["provider_digest"]["value"] = json!("abc");
    assert!(
        load(&catalogue("local", vec![entry]))
            .err()
            .unwrap()
            .to_string()
            .contains("provider_digest")
    );
}

#[test]
fn deterministic_across_repeated_scans_input_order_and_duplicates() {
    let entries = vec![
        record("local", "z", "Zulu"),
        record("local", "b", "alpha"),
        record("local", "a", "Alpha"),
    ];
    let provider = load(&catalogue("local", entries.clone())).unwrap();
    let expected = provider.discover_projects("").unwrap();
    assert_eq!(
        expected
            .iter()
            .map(|p| p.id.provider_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "z"]
    );
    for _ in 0..5 {
        assert_eq!(provider.discover_projects("").unwrap(), expected);
    }
    let mut reversed = entries.clone();
    reversed.reverse();
    reversed.push(entries[0].clone());
    assert_eq!(
        load(&catalogue("local", reversed))
            .unwrap()
            .discover_projects("")
            .unwrap(),
        expected
    );
}

#[test]
fn missing_unavailable_and_oversized_local_catalogues_fail_clearly() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.json");
    let error = LocalCatalogueProvider::from_path(&missing).err().unwrap();
    assert!(matches!(error, LocalCatalogueError::Io { .. }));
    assert!(error.to_string().contains("missing.json"));
    assert!(matches!(
        LocalCatalogueProvider::from_path(temp.path()),
        Err(LocalCatalogueError::NotRegularFile(_))
    ));
    assert!(matches!(
        LocalCatalogueProvider::from_json(&vec![b' '; MAX_HOMEBREW_CATALOGUE_BYTES + 1]),
        Err(LocalCatalogueError::TooLarge)
    ));
    let oversized = temp.path().join("oversized.json");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len((MAX_HOMEBREW_CATALOGUE_BYTES + 1) as u64)
        .unwrap();
    assert!(matches!(
        LocalCatalogueProvider::from_path(oversized),
        Err(LocalCatalogueError::TooLarge)
    ));
}

#[test]
fn loading_is_explicit_and_discovery_uses_an_immutable_offline_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalogue.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&catalogue("local", vec![record("local", "1", "Offline")])).unwrap(),
    )
    .unwrap();
    let provider = LocalCatalogueProvider::from_path(&path).unwrap();
    std::fs::write(&path, b"{now broken}").unwrap();
    assert_eq!(provider.discover_projects("offline").unwrap().len(), 1);
    assert!(LocalCatalogueProvider::from_path(path).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn offline_discovery_opens_no_network_sockets() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "homebrew_catalogue::tests::offline_discovery_network_blocked_child",
            "--nocapture",
        ])
        .env("EMUWIZ_HOMEBREW_OFFLINE_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child was killed or failed: {:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[cfg(target_os = "linux")]
#[test]
fn offline_discovery_network_blocked_child() {
    if std::env::var_os("EMUWIZ_HOMEBREW_OFFLINE_CHILD").is_none() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("offline.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&catalogue("local", vec![record("local", "1", "Offline")])).unwrap(),
    )
    .unwrap();
    // Kill the isolated child on any socket/connect/send syscall. A swallowed
    // network error cannot make this test pass. No filter touches the parent.
    let mut instructions = vec![libc::sock_filter {
        code: 0x20,
        jt: 0,
        jf: 0,
        k: 0,
    }]; // load syscall nr
    for syscall in [
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_sendto,
        libc::SYS_sendmsg,
    ] {
        instructions.push(libc::sock_filter {
            code: 0x15,
            jt: 0,
            jf: 1,
            k: syscall as u32,
        });
        instructions.push(libc::sock_filter {
            code: 0x06,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_KILL_PROCESS,
        });
    }
    instructions.push(libc::sock_filter {
        code: 0x06,
        jt: 0,
        jf: 0,
        k: libc::SECCOMP_RET_ALLOW,
    });
    let program = libc::sock_fprog {
        len: instructions.len() as u16,
        filter: instructions.as_mut_ptr(),
    };
    // SAFETY: valid BPF buffer remains alive for both calls; filter applies to
    // this throwaway child only, and forbids networking rather than granting it.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
        assert_eq!(
            libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program),
            0
        );
    }
    let provider = LocalCatalogueProvider::from_path(&path).unwrap();
    assert_eq!(provider.discover_projects("offline").unwrap().len(), 1);
    assert_eq!(provider.discover_for_platform("", "NES").unwrap().len(), 1);
    assert_eq!(
        provider.artifact_releases(&id("local", "1")).unwrap().len(),
        1
    );
}
