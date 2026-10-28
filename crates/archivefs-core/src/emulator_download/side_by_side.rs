//! Read-only assessment of declared release metadata. Never an install permit.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataRefusal {
    UnsupportedEmulator,
    SourceIdentityMismatch,
    InvalidAsset,
    MissingPublishedDigest,
    InvalidPublishedDigest,
    DigestMismatch,
}
impl MetadataRefusal {
    pub fn explanation(self) -> &'static str {
        match self {
            Self::UnsupportedEmulator => {
                "No reviewed AppImage source is configured for this emulator."
            }
            Self::SourceIdentityMismatch => {
                "Release source identity differs from the configured project."
            }
            Self::InvalidAsset => "The asset name or URL fails the existing download safety rules.",
            Self::MissingPublishedDigest => {
                "A published SHA-256 and its expected digest are required."
            }
            Self::InvalidPublishedDigest => "The declared SHA-256 is malformed.",
            Self::DigestMismatch => {
                "The expected digest differs from the declared published SHA-256."
            }
        }
    }
}

/// Metadata consistency alone never authenticates a publisher or executable.
/// This function does no network, filesystem, download, or process work.
pub fn assess_metadata(plan: &EmulatorDownloadPlan) -> Result<(), MetadataRefusal> {
    let spec = emulator_download_spec(&plan.emulator_id)
        .filter(|s| s.distribution == EmulatorDistribution::GithubAppImage)
        .ok_or(MetadataRefusal::UnsupportedEmulator)?;
    if plan.distribution != spec.distribution
        || plan.project_url != spec.project_url
        || plan.official_project != spec.official_project
        || plan.release_tag.trim().is_empty()
    {
        return Err(MetadataRefusal::SourceIdentityMismatch);
    }
    let asset = ReleaseAsset {
        name: plan.asset_name.clone(),
        download_url: plan.asset_url.clone(),
        digest: plan.upstream_digest.clone(),
    };
    select_x86_64_asset(spec, &[asset]).map_err(|_| MetadataRefusal::InvalidAsset)?;
    validate_endpoint_url(&plan.asset_url, spec, EndpointKind::Asset, true)
        .map_err(|_| MetadataRefusal::InvalidAsset)?;
    let (Some(upstream), Some(expected)) = (&plan.upstream_digest, &plan.expected_sha256) else {
        return Err(MetadataRefusal::MissingPublishedDigest);
    };
    let upstream =
        normalize_sha256_digest(upstream).map_err(|_| MetadataRefusal::InvalidPublishedDigest)?;
    let expected =
        normalize_sha256_digest(expected).map_err(|_| MetadataRefusal::InvalidPublishedDigest)?;
    if upstream != expected {
        return Err(MetadataRefusal::DigestMismatch);
    }
    Ok(())
}

pub const DISABLED_REASON: &str = "Side-by-side installation is disabled. Publisher provenance, downloaded artifact integrity, safe publication and executable selection still require independent review. No previous version is installed or switched by this assessment.";

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> EmulatorDownloadPlan {
        let spec = emulator_download_spec("pcsx2").unwrap();
        EmulatorDownloadPlan {
            emulator_id: spec.id.into(),
            display_name: spec.display_name.into(),
            distribution: spec.distribution,
            official_project: spec.official_project.into(),
            project_url: spec.project_url.into(),
            release_tag: "v1.2.3".into(),
            release_name: None,
            asset_name: "pcsx2-x86_64.AppImage".into(),
            asset_url:
                "https://github.com/PCSX2/pcsx2/releases/download/v1.2.3/pcsx2-x86_64.AppImage"
                    .into(),
            upstream_digest: Some(format!("sha256:{}", "A".repeat(64))),
            expected_sha256: Some("a".repeat(64)),
            destination_path: "/nonexistent/never-written".into(),
            selection_policy: ReleaseSelectionPolicy::default(),
            replaces_existing_managed: false,
        }
    }
    #[test]
    fn consistent_declared_metadata_is_not_install_authority() {
        assert_eq!(assess_metadata(&plan()), Ok(()));
        assert!(DISABLED_REASON.contains("disabled"));
    }
    #[test]
    fn missing_malformed_and_conflicting_digests_are_refused() {
        let mut p = plan();
        p.upstream_digest = None;
        assert_eq!(
            assess_metadata(&p),
            Err(MetadataRefusal::MissingPublishedDigest)
        );
        p.upstream_digest = Some("sha256:garbage".into());
        assert_eq!(
            assess_metadata(&p),
            Err(MetadataRefusal::InvalidPublishedDigest)
        );
        p.upstream_digest = Some("b".repeat(64));
        assert_eq!(assess_metadata(&p), Err(MetadataRefusal::DigestMismatch));
    }
    #[test]
    fn public_plan_cannot_substitute_a_project_or_distribution() {
        for mutate in [
            |p: &mut EmulatorDownloadPlan| p.project_url = "https://example.invalid".into(),
            |p: &mut EmulatorDownloadPlan| p.official_project = "another publisher".into(),
            |p: &mut EmulatorDownloadPlan| p.distribution = EmulatorDistribution::Manual,
        ] {
            let mut p = plan();
            mutate(&mut p);
            assert_eq!(
                assess_metadata(&p),
                Err(MetadataRefusal::SourceIdentityMismatch)
            );
        }
        let mut p = plan();
        p.emulator_id = "unknown".into();
        assert_eq!(
            assess_metadata(&p),
            Err(MetadataRefusal::UnsupportedEmulator)
        );
    }
    #[test]
    fn inherited_asset_rules_reject_credentials_fragments_and_wrong_architecture() {
        for url in [
            "https://user@github.com/PCSX2/pcsx2/releases/download/v1/a.AppImage",
            "https://github.com/PCSX2/pcsx2/releases/download/v1/a.AppImage#fragment",
            "https://github.com/another/project/releases/download/v1/a.AppImage",
            "http://github.com/PCSX2/pcsx2/releases/download/v1/a.AppImage",
        ] {
            let mut p = plan();
            p.asset_url = url.into();
            assert_eq!(assess_metadata(&p), Err(MetadataRefusal::InvalidAsset));
        }
        let mut p = plan();
        p.asset_name = "pcsx2-aarch64.AppImage".into();
        assert_eq!(assess_metadata(&p), Err(MetadataRefusal::InvalidAsset));
    }
}
