//! Safe, explicit opening of a RomM-provided manual.
//!
//! This module supports only a manually mapped local file. Remote manual URLs
//! remain metadata until a dedicated, approved URL-opening design exists.

use std::path::{Path, PathBuf};

use super::media_mapping::ValidatedRommMediaMapping;
use super::media_mapping::resolve_romm_media_reference;
use crate::identity_source::model::MediaReference;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RommManualRefusal {
    NoHostedReference,
    NoLocalMapping,
    Unsafe(String),
    Unavailable,
    OpenFailed(String),
}

impl std::fmt::Display for RommManualRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHostedReference => write!(f, "RomM did not provide a hosted manual reference."),
            Self::NoLocalMapping => write!(f, "No local RomM media mapping is configured."),
            Self::Unsafe(detail) => write!(f, "The RomM manual reference was refused: {detail}"),
            Self::Unavailable => write!(
                f,
                "The mapped RomM manual is not available on this machine."
            ),
            Self::OpenFailed(detail) => write!(f, "The manual could not be opened: {detail}"),
        }
    }
}

impl std::error::Error for RommManualRefusal {}

/// Resolves a manual only through the validated local RomM mapping.
pub fn resolve_local_romm_manual(
    mapping: Option<&ValidatedRommMediaMapping>,
    manual: &MediaReference,
) -> Result<PathBuf, RommManualRefusal> {
    let reference = manual
        .hosted_reference
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(RommManualRefusal::NoHostedReference)?;
    let mapping = mapping.ok_or(RommManualRefusal::NoLocalMapping)?;
    let reference = reference
        .split(['?', '#'])
        .next()
        .unwrap_or(reference)
        .trim();
    if reference.is_empty() {
        return Err(RommManualRefusal::NoHostedReference);
    }
    resolve_romm_media_reference(Some(mapping), reference)
        .map_err(|error| RommManualRefusal::Unsafe(error.to_string()))?
        .ok_or(RommManualRefusal::Unavailable)
}

pub trait ManualOpener {
    fn open(&self, path: &Path) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DesktopManualOpener;

impl ManualOpener for DesktopManualOpener {
    fn open(&self, path: &Path) -> Result<(), String> {
        let program = if cfg!(target_os = "windows") {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let status = std::process::Command::new(program)
            .arg(path)
            .status()
            .map_err(|error| format!("{program} could not be started: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(match status.code() {
                Some(code) => format!("{program} exited with status {code}"),
                None => format!("{program} was terminated by a signal"),
            })
        }
    }
}

pub fn open_local_romm_manual(
    mapping: Option<&ValidatedRommMediaMapping>,
    manual: &MediaReference,
    opener: &dyn ManualOpener,
) -> Result<PathBuf, RommManualRefusal> {
    let path = resolve_local_romm_manual(mapping, manual)?;
    opener.open(&path).map_err(RommManualRefusal::OpenFailed)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity_source::romm::media_mapping::{
        RommMediaMapping, validate_romm_media_mapping,
    };
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    const PREFIX: &str = "/assets/romm/resources";

    fn mapping(root: &Path) -> ValidatedRommMediaMapping {
        validate_romm_media_mapping(&RommMediaMapping {
            provider_prefix: PREFIX.to_string(),
            local_root: root.to_path_buf(),
        })
        .unwrap()
    }

    fn manual(reference: &str) -> MediaReference {
        MediaReference {
            hosted_reference: Some(reference.to_string()),
            public_reference: None,
        }
    }

    #[derive(Default)]
    struct FakeOpener {
        opened: Arc<Mutex<Vec<PathBuf>>>,
        failure: Option<String>,
    }

    impl ManualOpener for FakeOpener {
        fn open(&self, path: &Path) -> Result<(), String> {
            self.opened.lock().unwrap().push(path.to_path_buf());
            self.failure.clone().map_or(Ok(()), Err)
        }
    }

    #[test]
    fn valid_local_pdf_is_opened_at_the_canonical_path() {
        let root = TempDir::new().unwrap();
        let manual_path = root.path().join("manual.pdf");
        fs::write(&manual_path, b"%PDF-test").unwrap();
        let opener = FakeOpener::default();
        let opened = open_local_romm_manual(
            Some(&mapping(root.path())),
            &manual("/assets/romm/resources/manual.pdf?ts=1"),
            &opener,
        )
        .unwrap();
        assert_eq!(opened, fs::canonicalize(manual_path).unwrap());
        assert_eq!(opener.opened.lock().unwrap().as_slice(), &[opened]);
    }

    #[test]
    fn unsafe_or_unavailable_manuals_never_reach_the_opener() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("manual.pdf"), b"%PDF-test").unwrap();
        symlink(
            outside.path().join("manual.pdf"),
            root.path().join("escape.pdf"),
        )
        .unwrap();
        let opener = FakeOpener::default();
        for reference in [
            "/assets/romm/resources/../manual.pdf",
            "/assets/other/manual.pdf",
            "/assets/romm/resources/missing.pdf",
            "/assets/romm/resources/manual.txt",
            "/assets/romm/resources/escape.pdf",
        ] {
            let result =
                open_local_romm_manual(Some(&mapping(root.path())), &manual(reference), &opener);
            assert!(result.is_err(), "{reference}");
        }
        assert!(opener.opened.lock().unwrap().is_empty());
    }

    #[test]
    fn no_mapping_and_public_only_reference_are_refused_without_opening() {
        let opener = FakeOpener::default();
        let no_mapping =
            open_local_romm_manual(None, &manual("/assets/romm/resources/manual.pdf"), &opener);
        assert!(matches!(no_mapping, Err(RommManualRefusal::NoLocalMapping)));
        let public_only = MediaReference {
            hosted_reference: None,
            public_reference: Some("https://example.invalid/manual.pdf".to_string()),
        };
        let result = open_local_romm_manual(None, &public_only, &opener);
        assert!(matches!(result, Err(RommManualRefusal::NoHostedReference)));
        assert!(opener.opened.lock().unwrap().is_empty());
    }

    #[test]
    fn opener_failure_is_returned_cleanly() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("manual.pdf"), b"%PDF-test").unwrap();
        let opener = FakeOpener {
            opened: Arc::new(Mutex::new(Vec::new())),
            failure: Some("fake opener failed".to_string()),
        };
        let result = open_local_romm_manual(
            Some(&mapping(root.path())),
            &manual("/assets/romm/resources/manual.pdf"),
            &opener,
        );
        assert!(matches!(result, Err(RommManualRefusal::OpenFailed(_))));
    }
}
