use super::*;

fn project_id(provider: &str, provider_id: &str) -> HomebrewProjectId {
    HomebrewProjectId {
        provider: HomebrewProviderId::new(provider).unwrap(),
        provider_id: provider_id.into(),
    }
}

fn checksum() -> ArtifactChecksum {
    ArtifactChecksum {
        algorithm: ArtifactChecksumAlgorithm::Sha256,
        value: "ab".repeat(32),
    }
}

fn artifact(filename: &str) -> HomebrewArtifact {
    HomebrewArtifact {
        provider_asset_id: filename.into(),
        filename: filename.into(),
        size_bytes: Some(1024),
        acquisition: ArtifactAcquisition {
            source_page_url: "https://example.org/project".into(),
            link_url: Some(format!("https://example.org/assets/{filename}")),
            mode: ArtifactAcquisitionMode::BrowserRequired,
        },
        provider_digest: None,
        platform_claims: Vec::new(),
        rights: RightsEvidence::default(),
    }
}

fn project(provider: &str) -> HomebrewProject {
    HomebrewProject {
        id: project_id(provider, "123"),
        title: "Same project title".into(),
        summary: None,
        source_page_url: "https://example.org/project".into(),
        links: Vec::new(),
        rights: RightsEvidence::default(),
    }
}

#[test]
fn project_ids_are_stable_exact_provider_scoped_pairs() {
    let id = project_id("github", "Owner/Repo");
    let encoded = serde_json::to_string(&id).unwrap();
    assert_eq!(
        serde_json::from_str::<HomebrewProjectId>(&encoded).unwrap(),
        id
    );
    assert_ne!(id, project_id("itch", "Owner/Repo"));
    assert_ne!(id, project_id("github", "owner/repo"));
    assert!(HomebrewProviderId::new("").is_err());
    let ids = std::collections::BTreeSet::from([id.clone(), id, project_id("itch", "Owner/Repo")]);
    assert_eq!(ids.len(), 2);
}

#[test]
fn artifacts_can_exist_without_any_release_metadata() {
    let listing = HomebrewArtifactListing {
        project: project_id("itch", "123"),
        release: None,
        artifacts: vec![artifact("demo.nes")],
    };
    let encoded = serde_json::to_string(&listing).unwrap();
    let restored: HomebrewArtifactListing = serde_json::from_str(&encoded).unwrap();
    assert_eq!(restored, listing);
    assert!(restored.release.is_none());
    assert_eq!(HomebrewRelease::default().provider_release_id, None);
}

#[test]
fn one_release_can_contain_artifacts_for_multiple_platforms() {
    let mut nes = artifact("demo.nes");
    nes.platform_claims.push(PlatformClaim {
        platform: "NES".into(),
        evidence: PlatformClaimEvidence::Extension("nes".into()),
    });
    let mut snes = artifact("demo.sfc");
    snes.platform_claims.push(PlatformClaim {
        platform: "SNES".into(),
        evidence: PlatformClaimEvidence::ReleaseMetadata("SNES build".into()),
    });
    let listing = HomebrewArtifactListing {
        project: project_id("github", "123"),
        release: Some(HomebrewRelease {
            provider_release_id: Some("release-42".into()),
            version: Some("1.0".into()),
            tag: Some("v1.0".into()),
            published_at_unix_secs: Some(100),
            updated_at_unix_secs: Some(200),
            prerelease: true,
        }),
        artifacts: vec![nes, snes],
    };
    assert_eq!(listing.artifacts[0].platform_claims[0].platform, "NES");
    assert_eq!(listing.artifacts[1].platform_claims[0].platform, "SNES");
    assert_eq!(
        serde_json::from_str::<HomebrewArtifactListing>(&serde_json::to_string(&listing).unwrap())
            .unwrap(),
        listing
    );
}

#[test]
fn non_game_artifacts_need_no_platform_claim() {
    let manual = artifact("manual.pdf");
    assert!(manual.platform_claims.is_empty());
    let restored: HomebrewArtifact =
        serde_json::from_str(&serde_json::to_string(&manual).unwrap()).unwrap();
    assert!(restored.platform_claims.is_empty());
}

#[test]
fn all_rights_default_to_unknown_including_missing_json_fields() {
    let rights = RightsEvidence::default();
    assert_eq!(rights.software_licence, RightsFact::Unknown);
    assert_eq!(rights.asset_licence, RightsFact::Unknown);
    assert_eq!(rights.redistribution, RightsFact::Unknown);
    assert_eq!(rights.commercial_status, RightsFact::Unknown);
    assert_eq!(rights.source_availability, RightsFact::Unknown);
    assert_eq!(
        serde_json::from_str::<RightsEvidence>("{}").unwrap(),
        rights
    );
    let mut metadata = serde_json::to_value(project("github")).unwrap();
    metadata.as_object_mut().unwrap().remove("rights");
    assert_eq!(
        serde_json::from_value::<HomebrewProject>(metadata)
            .unwrap()
            .rights,
        rights
    );
}

#[test]
fn provider_digest_keeps_provider_computed_provenance() {
    let mut payload = artifact("demo.nes");
    assert!(payload.provider_digest_evidence().is_none());
    payload.provider_digest = Some(checksum());
    let evidence = payload.provider_digest_evidence().unwrap();
    assert_eq!(evidence.checksum, checksum());
    assert_eq!(evidence.provenance, DigestProvenance::ProviderComputed);
}

#[test]
fn local_digest_provenance_is_distinct_even_when_bytes_match_provider_digest() {
    let local = ArtifactDigest {
        checksum: checksum(),
        provenance: DigestProvenance::LocalComputed,
    };
    let provider = ArtifactDigest {
        checksum: checksum(),
        provenance: DigestProvenance::ProviderComputed,
    };
    assert_eq!(local.checksum, provider.checksum);
    assert_ne!(local, provider);
    assert_eq!(
        serde_json::from_str::<ArtifactDigest>(&serde_json::to_string(&local).unwrap()).unwrap(),
        local
    );
}

#[test]
fn cross_provider_linkage_requires_an_explicit_evidence_record() {
    let mut github = project("github");
    let itch = project("itch");
    // Even identical title and URL do not link the provider-scoped records.
    assert_eq!(github.title, itch.title);
    assert_eq!(github.source_page_url, itch.source_page_url);
    assert_ne!(github.id, itch.id);
    assert!(github.links.is_empty());
    github.links.push(HomebrewProjectLink {
        project: itch.id.clone(),
        evidence: "Author explicitly links this itch project from the repository".into(),
    });
    assert_eq!(github.links[0].project, itch.id);
    assert_ne!(github.id, itch.id);
}

#[test]
fn platform_evidence_is_preserved_without_automatic_identity_authority() {
    let evidence = [
        PlatformClaimEvidence::Extension("nes".into()),
        PlatformClaimEvidence::Filename("demo-nes.bin".into()),
        PlatformClaimEvidence::ReleaseMetadata("NES build".into()),
        PlatformClaimEvidence::StructuralInspection("NES header observed locally".into()),
    ];
    for basis in evidence {
        let claim = PlatformClaim {
            platform: "NES".into(),
            evidence: basis,
        };
        assert_eq!(claim.registered_platform().unwrap().id, "NES");
        let metadata = serde_json::to_value(&claim).unwrap();
        assert!(metadata.get("verified").is_none());
        assert_eq!(
            serde_json::from_value::<PlatformClaim>(metadata).unwrap(),
            claim
        );
    }
    let unknown = PlatformClaim {
        platform: "unknown-provider-platform".into(),
        evidence: PlatformClaimEvidence::ReleaseMetadata("provider assertion".into()),
    };
    assert!(unknown.registered_platform().is_none());
}

#[test]
fn download_code_licence_and_free_price_never_infer_other_rights() {
    let mut payload = artifact("free-game.nes");
    payload.acquisition.mode = ArtifactAcquisitionMode::DirectPermitted;
    payload.acquisition.source_page_url = "https://github.com/example/free-game".into();
    payload.rights.software_licence = RightsFact::Known {
        value: "MIT".into(),
        evidence: "Provider SPDX licence for repository code".into(),
    };
    assert_eq!(payload.rights.asset_licence, RightsFact::Unknown);
    assert_eq!(payload.rights.redistribution, RightsFact::Unknown);
    assert_eq!(payload.rights.commercial_status, RightsFact::Unknown);
    assert_eq!(payload.rights.source_availability, RightsFact::Unknown);
}

#[test]
fn discovery_and_artifact_release_traits_are_independent() {
    struct Discovery(HomebrewProviderId);
    impl ProjectDiscoveryProvider for Discovery {
        type Error = std::convert::Infallible;
        fn provider(&self) -> &HomebrewProviderId {
            &self.0
        }
        fn discover_projects(&self, _: &str) -> Result<Vec<HomebrewProject>, Self::Error> {
            Ok(vec![project(self.0.as_str())])
        }
    }
    struct Releases(HomebrewProviderId);
    impl ArtifactReleaseProvider for Releases {
        type Error = &'static str;
        fn provider(&self) -> &HomebrewProviderId {
            &self.0
        }
        fn artifact_releases(
            &self,
            project: &HomebrewProjectId,
        ) -> Result<Vec<HomebrewArtifactListing>, Self::Error> {
            if project.provider != self.0 {
                return Err("provider mismatch");
            }
            Ok(vec![HomebrewArtifactListing {
                project: project.clone(),
                release: None,
                artifacts: vec![artifact("manual.pdf")],
            }])
        }
    }
    let discovery = Discovery(HomebrewProviderId::new("itch").unwrap());
    assert_eq!(
        discovery.discover_projects("demo").unwrap()[0].id.provider,
        *discovery.provider()
    );
    let releases = Releases(HomebrewProviderId::new("github").unwrap());
    assert!(
        releases
            .artifact_releases(&project_id("itch", "123"))
            .is_err()
    );
    assert_eq!(
        releases
            .artifact_releases(&project_id("github", "123"))
            .unwrap()
            .len(),
        1
    );
}
