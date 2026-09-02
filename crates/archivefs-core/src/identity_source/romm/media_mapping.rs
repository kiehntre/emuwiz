//! Optional, explicit mapping from RomM's media namespace to a host path.
//!
//! RomM's `path_cover_*`, `path_screenshots`, and `path_manual` values are
//! HTTP-served provider references. They become local files only when a user
//! explicitly maps the provider prefix to an existing host-visible directory.
//! This module never guesses container mounts and never reads outside that
//! mapped directory.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::identity_source::path_map::{MappingRefusal, ProviderPathKind, normalise_prefix};

pub const MAX_MEDIA_PREFIX_BYTES: usize = 4096;

/// Persisted, user-entered mapping. The local root is canonicalized when it is
/// validated before saving; loading still revalidates it before use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RommMediaMapping {
    /// Provider namespace prefix, for example `/assets/romm/resources`.
    pub provider_prefix: String,
    /// Host-visible directory containing the corresponding media files.
    pub local_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRommMediaMapping {
    provider_prefix: String,
    kind: ProviderPathKind,
    local_root: PathBuf,
}

impl ValidatedRommMediaMapping {
    pub fn provider_prefix(&self) -> &str {
        &self.provider_prefix
    }

    pub fn local_root(&self) -> &Path {
        &self.local_root
    }

    /// Resolves one provider reference to a readable local media file.
    ///
    /// `Ok(None)` means the mapped file is absent, so callers can retain the
    /// existing remote/cache path.
    /// Malformed, unsafe, or unsupported references are errors and must not be
    /// reinterpreted as local paths.
    pub fn resolve(&self, reference: &str) -> Result<Option<PathBuf>, RommMediaMappingError> {
        if !is_real_directory_no_follow(&self.local_root) {
            return Err(RommMediaMappingError::RootUnavailable(
                self.local_root.clone(),
            ));
        }
        let normalized = normalise_prefix(reference, self.kind)
            .map_err(RommMediaMappingError::InvalidReference)?;
        let suffix = if self.provider_prefix == "/" {
            normalized.strip_prefix('/').unwrap_or_default()
        } else if normalized == self.provider_prefix {
            return Ok(None);
        } else if let Some(suffix) = normalized.strip_prefix(&self.provider_prefix) {
            suffix.strip_prefix('/').ok_or_else(|| {
                RommMediaMappingError::OutsideProviderPrefix(reference.to_string())
            })?
        } else {
            return Err(RommMediaMappingError::OutsideProviderPrefix(
                reference.to_string(),
            ));
        };
        if suffix.is_empty() {
            return Ok(None);
        }
        let candidate = self.local_root.join(suffix);
        let canonical = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RommMediaMappingError::ReadFailed(
                    candidate,
                    error.to_string(),
                ));
            }
        };
        if !canonical.starts_with(&self.local_root) {
            return Err(RommMediaMappingError::SymlinkEscape(canonical));
        }
        let metadata = fs::symlink_metadata(&canonical).map_err(|error| {
            RommMediaMappingError::ReadFailed(canonical.clone(), error.to_string())
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(RommMediaMappingError::NotAFile(canonical));
        }
        if !supported_media_type(&canonical) {
            return Err(RommMediaMappingError::UnsupportedMediaType(canonical));
        }
        OpenOptions::new()
            .read(true)
            .open(&canonical)
            .map_err(|error| {
                RommMediaMappingError::ReadFailed(canonical.clone(), error.to_string())
            })?;
        Ok(Some(canonical))
    }
}

/// Resolves a reference only when an explicit local mapping is configured.
/// Without a mapping, callers retain the existing remote/cache behaviour.
pub fn resolve_romm_media_reference(
    mapping: Option<&ValidatedRommMediaMapping>,
    reference: &str,
) -> Result<Option<PathBuf>, RommMediaMappingError> {
    mapping.map_or(Ok(None), |mapping| mapping.resolve(reference))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RommMediaMappingError {
    InvalidProviderPrefix(MappingRefusal),
    InvalidReference(MappingRefusal),
    RootNotAbsolute(PathBuf),
    RootUnavailable(PathBuf),
    RootIsSymlink(PathBuf),
    OutsideProviderPrefix(String),
    SymlinkEscape(PathBuf),
    NotAFile(PathBuf),
    UnsupportedMediaType(PathBuf),
    ReadFailed(PathBuf, String),
}

impl std::fmt::Display for RommMediaMappingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProviderPrefix(error) => write!(
                formatter,
                "invalid RomM media provider prefix: {}",
                error.detail()
            ),
            Self::InvalidReference(error) => write!(
                formatter,
                "invalid RomM media reference: {}",
                error.detail()
            ),
            Self::RootNotAbsolute(path) => write!(
                formatter,
                "local media root is not absolute: {}",
                path.display()
            ),
            Self::RootUnavailable(path) => write!(
                formatter,
                "local media root is missing or not a directory: {}",
                path.display()
            ),
            Self::RootIsSymlink(path) => write!(
                formatter,
                "local media root is a symlink: {}",
                path.display()
            ),
            Self::OutsideProviderPrefix(reference) => write!(
                formatter,
                "media reference is outside the configured provider prefix: {reference}"
            ),
            Self::SymlinkEscape(path) => write!(
                formatter,
                "local media path escapes the configured root: {}",
                path.display()
            ),
            Self::NotAFile(path) => write!(
                formatter,
                "local media path is not a regular file: {}",
                path.display()
            ),
            Self::UnsupportedMediaType(path) => write!(
                formatter,
                "unsupported local media type: {}",
                path.display()
            ),
            Self::ReadFailed(path, detail) => write!(
                formatter,
                "local media file could not be read ({}): {detail}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RommMediaMappingError {}

/// Validates and canonicalizes a user-configured media mapping.
pub fn validate_romm_media_mapping(
    mapping: &RommMediaMapping,
) -> Result<ValidatedRommMediaMapping, RommMediaMappingError> {
    if mapping.provider_prefix.len() > MAX_MEDIA_PREFIX_BYTES {
        return Err(RommMediaMappingError::InvalidProviderPrefix(
            MappingRefusal::TooLong {
                bytes: mapping.provider_prefix.len(),
                maximum: MAX_MEDIA_PREFIX_BYTES,
            },
        ));
    }
    let kind = ProviderPathKind::observed_in(&mapping.provider_prefix);
    let provider_prefix = normalise_prefix(&mapping.provider_prefix, kind)
        .map_err(RommMediaMappingError::InvalidProviderPrefix)?;
    if !mapping.local_root.is_absolute() {
        return Err(RommMediaMappingError::RootNotAbsolute(
            mapping.local_root.clone(),
        ));
    }
    let metadata = fs::symlink_metadata(&mapping.local_root)
        .map_err(|_| RommMediaMappingError::RootUnavailable(mapping.local_root.clone()))?;
    if metadata.file_type().is_symlink() {
        return Err(RommMediaMappingError::RootIsSymlink(
            mapping.local_root.clone(),
        ));
    }
    if !metadata.is_dir() {
        return Err(RommMediaMappingError::RootUnavailable(
            mapping.local_root.clone(),
        ));
    }
    let local_root = fs::canonicalize(&mapping.local_root)
        .map_err(|_| RommMediaMappingError::RootUnavailable(mapping.local_root.clone()))?;
    Ok(ValidatedRommMediaMapping {
        provider_prefix,
        kind,
        local_root,
    })
}

fn is_real_directory_no_follow(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn supported_media_type(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "avif" | "pdf"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn mapping(root: &Path) -> RommMediaMapping {
        RommMediaMapping {
            provider_prefix: "/assets/romm/resources".into(),
            local_root: root.to_path_buf(),
        }
    }

    #[test]
    fn valid_reference_resolves_under_canonical_root() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("cover.png"), b"image").unwrap();
        let validated = validate_romm_media_mapping(&mapping(root.path())).unwrap();
        assert_eq!(
            validated
                .resolve("/assets/romm/resources/cover.png")
                .unwrap(),
            Some(fs::canonicalize(root.path().join("cover.png")).unwrap())
        );
    }

    #[test]
    fn unrelated_provider_reference_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let validated = validate_romm_media_mapping(&mapping(root.path())).unwrap();
        assert!(matches!(
            validated.resolve("/assets/other/cover.png"),
            Err(RommMediaMappingError::OutsideProviderPrefix(_))
        ));
    }

    #[test]
    fn no_mapping_preserves_remote_behaviour() {
        assert_eq!(
            resolve_romm_media_reference(None, "/assets/romm/resources/cover.png").unwrap(),
            None
        );
    }

    #[test]
    fn traversal_and_absolute_injection_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let validated = validate_romm_media_mapping(&mapping(root.path())).unwrap();
        assert!(matches!(
            validated.resolve("/assets/romm/resources/../cover.png"),
            Err(RommMediaMappingError::InvalidReference(_))
        ));
        assert!(matches!(
            validated.resolve("/etc/passwd"),
            Err(RommMediaMappingError::OutsideProviderPrefix(_))
        ));
    }

    #[test]
    fn escaping_symlink_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("cover.png"), b"image").unwrap();
        symlink(
            outside.path().join("cover.png"),
            root.path().join("cover.png"),
        )
        .unwrap();
        let validated = validate_romm_media_mapping(&mapping(root.path())).unwrap();
        assert!(matches!(
            validated.resolve("/assets/romm/resources/cover.png"),
            Err(RommMediaMappingError::SymlinkEscape(_))
        ));
    }

    #[test]
    fn invalid_roots_are_rejected() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(matches!(
            validate_romm_media_mapping(&RommMediaMapping {
                provider_prefix: "/assets".into(),
                local_root: file.path().into()
            }),
            Err(RommMediaMappingError::RootUnavailable(_))
        ));
        assert!(matches!(
            validate_romm_media_mapping(&RommMediaMapping {
                provider_prefix: "/assets".into(),
                local_root: PathBuf::from("relative")
            }),
            Err(RommMediaMappingError::RootNotAbsolute(_))
        ));
    }

    #[test]
    fn symlinked_root_and_unsupported_file_are_rejected() {
        let real_root = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let symlinked_root = parent.path().join("resources");
        symlink(real_root.path(), &symlinked_root).unwrap();
        assert!(matches!(
            validate_romm_media_mapping(&RommMediaMapping {
                provider_prefix: "/assets".into(),
                local_root: symlinked_root,
            }),
            Err(RommMediaMappingError::RootIsSymlink(_))
        ));

        fs::write(real_root.path().join("notes.txt"), b"not media").unwrap();
        let validated = validate_romm_media_mapping(&mapping(real_root.path())).unwrap();
        assert!(matches!(
            validated.resolve("/assets/romm/resources/notes.txt"),
            Err(RommMediaMappingError::UnsupportedMediaType(_))
        ));
    }

    #[test]
    fn mapping_round_trips_through_persisted_json() {
        let original = mapping(Path::new("/srv/romm/resources"));
        let encoded = serde_json::to_vec(&original).unwrap();
        let decoded: RommMediaMapping = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, original);
    }
}
