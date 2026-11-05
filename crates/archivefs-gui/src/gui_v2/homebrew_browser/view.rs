//! Pure projection of homebrew catalogue/release metadata into display rows.
//! No I/O: everything shown is a recorded claim with its stated basis.

use archivefs_core::{
    homebrew_artifact::{
        ArtifactAcquisitionMode, ArtifactChecksumAlgorithm, HomebrewArtifact,
        HomebrewArtifactListing, PlatformClaim, PlatformClaimEvidence, RedistributionPermission,
        RightsEvidence, RightsFact, SourceAvailability,
    },
    homebrew_catalogue::LocalHomebrewRecord,
    homebrew_github::{GithubReleaseDetails, GithubRepositoryFacts},
};
use std::collections::BTreeSet;

pub(super) const LAUNCH_NOTE: &str = "Launch: not offered from discovery. Nothing listed here is downloaded, installed or run. After you obtain a file yourself and add it to your library, Blue's normal launch checks decide whether it can be played.";
const MAX_TEXT: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub key: String,
    pub title: String,
    pub summary: Option<String>,
    pub source: String,
    pub source_page_url: String,
    pub platforms: Vec<String>,
    pub releases: Vec<Release>,
    pub licence: Vec<String>,
    pub provenance: Vec<String>,
    pub compatibility: Vec<String>,
    /// The GitHub `owner/repository` this entry's source points at, when its
    /// own recorded source says so. Only used to offer an explicit lookup.
    pub github_repository: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Release {
    pub label: String,
    pub builds: Vec<String>,
}

/// Network-supplied text may carry control characters or be huge; neither is
/// ever shown raw.
pub(super) fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .take(MAX_TEXT)
        .collect();
    if text.chars().count() > MAX_TEXT {
        out.push('…');
    }
    out
}

pub(super) fn from_catalogue(record: &LocalHomebrewRecord, source: &str) -> Entry {
    let project = &record.project;
    let mut licence = rights_lines(&project.rights);
    let listings: Vec<&HomebrewArtifactListing> = record.listings.iter().collect();
    if licence.iter().all(|line| line.ends_with("unknown")) {
        // Fall back to per-build rights only when the project records none.
        for artifact in listings.iter().flat_map(|l| &l.artifacts) {
            let lines = rights_lines(&artifact.rights);
            if lines.iter().any(|line| !line.ends_with("unknown")) {
                licence = lines;
                break;
            }
        }
    }
    let github_repository = project_github_repository(record);
    Entry {
        key: format!("{}/{}", project.id.provider, project.id.provider_id),
        title: clean(&project.title),
        summary: project.summary.as_deref().map(clean),
        source: clean(source),
        source_page_url: clean(&project.source_page_url),
        platforms: platforms(&listings),
        releases: releases(&listings),
        licence,
        provenance: record
            .observations
            .iter()
            .map(|o| {
                format!(
                    "Observed {} at {}: {}",
                    clean(&o.observed_on),
                    clean(&o.source_url),
                    clean(&o.note)
                )
            })
            .collect(),
        compatibility: compatibility(&listings),
        github_repository,
    }
}

pub(super) fn from_github(
    repository: &GithubRepositoryFacts,
    listings: &[HomebrewArtifactListing],
    details: &[GithubReleaseDetails],
) -> Entry {
    let refs: Vec<&HomebrewArtifactListing> = listings.iter().collect();
    let mut licence = vec![match &repository.spdx_id {
        Some(spdx) => format!(
            "Repository software licence (SPDX): {}. This says nothing about game assets, music, or permission to redistribute.",
            clean(spdx)
        ),
        None => "Repository software licence: unknown".into(),
    }];
    licence.push("Asset licence: unknown".into());
    licence.push("Redistribution: unknown".into());
    let mut releases = releases(&refs);
    for (release, detail) in releases.iter_mut().zip(details) {
        if detail.draft {
            release.label.push_str(" (draft)");
        }
    }
    Entry {
        key: format!("github/{}", repository.repository_id),
        title: clean(&repository.full_name),
        summary: None,
        source: "GitHub Releases (looked up on request)".into(),
        source_page_url: clean(&repository.html_url),
        platforms: platforms(&refs),
        releases,
        licence,
        provenance: vec![format!(
            "Metadata fetched from api.github.com for repository id {}. Identity is the repository id; names and tags can change.",
            repository.repository_id
        )],
        compatibility: compatibility(&refs),
        github_repository: Some(clean(&repository.full_name)),
    }
}

fn project_github_repository(record: &LocalHomebrewRecord) -> Option<String> {
    let mut urls = vec![record.project.source_page_url.as_str()];
    if let RightsFact::Known {
        value: SourceAvailability::Available {
            source_url: Some(url),
        },
        ..
    } = &record.project.rights.source_availability
    {
        urls.push(url);
    }
    urls.into_iter().find_map(github_repository_from_url)
}

/// `https://github.com/owner/repo[/...]` only; anything else is not offered.
pub(super) fn github_repository_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let candidate = format!("{owner}/{repo}");
    archivefs_core::homebrew_github::GithubRepositoryRef::parse(&candidate)
        .ok()
        .map(|_| candidate)
}

fn rights_lines(rights: &RightsEvidence) -> Vec<String> {
    fn fact<T>(label: &str, fact: &RightsFact<T>, show: impl Fn(&T) -> String) -> String {
        match fact {
            RightsFact::Unknown => format!("{label}: unknown"),
            RightsFact::Known { value, evidence } => {
                format!("{label}: {} (basis: {})", show(value), clean(evidence))
            }
        }
    }
    vec![
        fact("Software licence", &rights.software_licence, |v| clean(v)),
        fact("Asset licence", &rights.asset_licence, |v| clean(v)),
        fact("Redistribution", &rights.redistribution, |v| match v {
            RedistributionPermission::Permitted => "permitted".into(),
            RedistributionPermission::Prohibited => "prohibited".into(),
        }),
    ]
}

fn artifacts<'a>(listings: &[&'a HomebrewArtifactListing]) -> Vec<&'a HomebrewArtifact> {
    listings.iter().flat_map(|l| &l.artifacts).collect()
}

fn platforms(listings: &[&HomebrewArtifactListing]) -> Vec<String> {
    let set: BTreeSet<String> = artifacts(listings)
        .iter()
        .flat_map(|a| a.platform_claims.iter().map(|c| clean(&c.platform)))
        .collect();
    set.into_iter().collect()
}

fn releases(listings: &[&HomebrewArtifactListing]) -> Vec<Release> {
    listings
        .iter()
        .map(|listing| {
            let label = match &listing.release {
                None => "Unversioned (no release recorded)".to_string(),
                Some(release) => {
                    let version = release
                        .version
                        .as_deref()
                        .or(release.tag.as_deref())
                        .map(clean)
                        .unwrap_or_else(|| "version not stated".into());
                    if release.prerelease {
                        format!("{version} (pre-release)")
                    } else {
                        version
                    }
                }
            };
            Release {
                label,
                builds: listing.artifacts.iter().map(build_line).collect(),
            }
        })
        .collect()
}

fn build_line(artifact: &HomebrewArtifact) -> String {
    let size = artifact
        .size_bytes
        .map_or("size unknown".to_string(), |b| format!("{b} bytes"));
    let digest = match &artifact.provider_digest {
        Some(hash) => {
            let algorithm = match hash.algorithm {
                ArtifactChecksumAlgorithm::Sha256 => "sha256",
                #[allow(unreachable_patterns)]
                _ => "digest",
            };
            format!(
                "{algorithm} {} (provider-reported integrity only)",
                clean(&hash.value)
            )
        }
        None => "no digest recorded".into(),
    };
    let how = match artifact.acquisition.mode {
        ArtifactAcquisitionMode::DirectPermitted => "direct link recorded (not followed by Blue)",
        ArtifactAcquisitionMode::BrowserRequired => "obtain through the source page in a browser",
        ArtifactAcquisitionMode::ExternalHost => "hosted externally",
        ArtifactAcquisitionMode::Unavailable => "no longer available",
        ArtifactAcquisitionMode::Unsupported => "acquisition not supported",
    };
    format!("{} · {size} · {digest} · {how}", clean(&artifact.filename))
}

fn compatibility(listings: &[&HomebrewArtifactListing]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut notes = Vec::new();
    for claim in artifacts(listings).iter().flat_map(|a| &a.platform_claims) {
        let line = claim_note(claim);
        if seen.insert(line.clone()) {
            notes.push(line);
        }
    }
    if notes.is_empty() {
        notes.push("No platform claim recorded; compatibility is unknown.".into());
    }
    notes
}

fn claim_note(claim: &PlatformClaim) -> String {
    let basis = match &claim.evidence {
        PlatformClaimEvidence::Extension(ext) => {
            format!("guessed from file extension {} (heuristic)", clean(ext))
        }
        PlatformClaimEvidence::Filename(name) => {
            format!("guessed from filename {} (heuristic)", clean(name))
        }
        PlatformClaimEvidence::ReleaseMetadata(note) => {
            format!("release metadata: {}", clean(note))
        }
        PlatformClaimEvidence::StructuralInspection(note) => {
            format!("structural inspection: {}", clean(note))
        }
    };
    let registry = if claim.registered_platform().is_some() {
        "Blue knows this platform name"
    } else {
        "not a platform name Blue recognises"
    };
    format!(
        "{}: {registry}. Basis: {basis}. A claim, not tested compatibility.",
        clean(&claim.platform)
    )
}

/// Case-insensitive title/summary filter plus an exact platform-claim filter.
pub(super) fn filter<'a>(
    entries: &'a [Entry],
    query: &str,
    platform: Option<&str>,
) -> Vec<&'a Entry> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|e| {
            (query.is_empty()
                || e.title.to_lowercase().contains(&query)
                || e.summary
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(&query)))
                && platform.is_none_or(|p| e.platforms.iter().any(|x| x == p))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::homebrew_artifact::ProjectDiscoveryProvider;
    use archivefs_core::homebrew_catalogue::LocalCatalogueProvider;

    fn starter() -> Vec<Entry> {
        let provider = LocalCatalogueProvider::bundled_starter().unwrap();
        provider
            .discover_projects("")
            .unwrap()
            .iter()
            .map(|p| {
                from_catalogue(
                    provider.record(&p.id).unwrap(),
                    "Built-in starter catalogue",
                )
            })
            .collect()
    }

    #[test]
    fn starter_entries_show_platform_licence_provenance_and_unknowns() {
        let entries = starter();
        let tobu = entries
            .iter()
            .find(|e| e.title == "Tobu Tobu Girl")
            .unwrap();
        assert!(tobu.platforms.contains(&"Game Boy".to_string()));
        assert!(
            tobu.licence
                .iter()
                .any(|l| l.starts_with("Software licence: MIT"))
        );
        assert!(tobu.licence.iter().any(|l| l == "Redistribution: unknown"));
        assert!(!tobu.provenance.is_empty());
        assert!(
            tobu.compatibility
                .iter()
                .any(|c| c.contains("not tested compatibility"))
        );
        assert_eq!(
            tobu.github_repository.as_deref(),
            Some("SimonLarsen/tobutobugirl")
        );
    }

    #[test]
    fn filters_by_text_and_exact_platform() {
        let entries = starter();
        assert_eq!(filter(&entries, "tobu", None).len(), 1);
        assert!(filter(&entries, "", Some("No Such Platform")).is_empty());
        assert!(!filter(&entries, "", Some("Game Boy")).is_empty());
    }

    #[test]
    fn only_https_github_repository_urls_are_offered_for_lookup() {
        assert_eq!(
            github_repository_from_url("https://github.com/a/b.git").as_deref(),
            Some("a/b")
        );
        assert_eq!(github_repository_from_url("http://github.com/a/b"), None);
        assert_eq!(
            github_repository_from_url("https://evil.example/github.com/a/b"),
            None
        );
        assert_eq!(github_repository_from_url("https://github.com/a"), None);
        assert_eq!(github_repository_from_url("https://github.com/../b"), None);
    }

    #[test]
    fn untrusted_text_has_controls_replaced_and_is_bounded() {
        let cleaned = clean(&format!("a\u{1b}[31m{}", "x".repeat(1000)));
        assert!(!cleaned.contains('\u{1b}'));
        assert!(cleaned.chars().count() <= MAX_TEXT + 1);
    }

    #[test]
    fn launch_note_never_promises_launch() {
        assert!(LAUNCH_NOTE.contains("not offered"));
    }
}
