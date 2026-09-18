//! Read-only analysis for a possible original-Xbox XISO reversible shrink.
//!
//! XDVDFS inspection proves that an image is structurally usable, but an
//! extracted/rebuilt XISO does not preserve the original raw sector layout or
//! padding.  Until a representation stores every omitted byte and its exact
//! placement, EmuWiz must not call that transformation reversible.  This
//! module therefore records source evidence and fails closed when a caller
//! asks for a byte-exact shrink proposal.

use sha2::{Digest, Sha256};
use std::{fmt, path::PathBuf};

use crate::xbox_boot_evidence::observe_xbox_disc;
use crate::xdvdfs_traversal::list_root;

/// The only state that may eventually be presented as a verified reversible
/// shrink.  It is deliberately absent from the current analyzer's successful
/// result: no byte-exact XISO representation is proven yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XisoReversibleState {
    ReversibleExact,
    PlayableButNotByteExact,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XisoLayout {
    RawOrStripped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XisoAnalysis {
    pub source_size: u64,
    pub source_sha256: String,
    pub layout: Option<XisoLayout>,
    pub state: XisoReversibleState,
    pub shrinkable_bytes: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XisoShrinkError {
    OutputExists(PathBuf),
    NoByteExactPath,
}

impl fmt::Display for XisoShrinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutputExists(path) => write!(f, "output already exists: {}", path.display()),
            Self::NoByteExactPath => write!(
                f,
                "no byte-exact XISO shrink representation has been proven"
            ),
        }
    }
}

impl std::error::Error for XisoShrinkError {}

/// Analyze an XISO-shaped byte stream without writing, deleting, or changing
/// it. XDVDFS traversal supplies the bounded structure check; hashing is one
/// linear pass over the caller-owned source bytes.
pub fn analyze_xiso_bytes(bytes: &[u8]) -> XisoAnalysis {
    let source_sha256 = hex_sha256(bytes);
    let source_size = bytes.len() as u64;
    let minimum_probe = 32 * 2048 + 20;
    if bytes.len() < minimum_probe {
        return XisoAnalysis {
            source_size,
            source_sha256,
            layout: None,
            state: XisoReversibleState::Unknown,
            shrinkable_bytes: None,
            reason: "the source is too short to establish an XDVDFS volume descriptor".into(),
        };
    }

    let observation = observe_xbox_disc(bytes);
    if !observation.xdvdfs_signature_present {
        return XisoAnalysis {
            source_size,
            source_sha256,
            layout: None,
            state: XisoReversibleState::Unsupported,
            shrinkable_bytes: None,
            reason: "no raw/stripped XDVDFS volume descriptor was found".into(),
        };
    }

    if list_root(bytes).is_err() {
        return XisoAnalysis {
            source_size,
            source_sha256,
            layout: Some(XisoLayout::RawOrStripped),
            state: XisoReversibleState::Unsupported,
            shrinkable_bytes: None,
            reason:
                "the XDVDFS signature is present but the bounded directory structure is malformed"
                    .into(),
        };
    }

    XisoAnalysis {
        source_size,
        source_sha256,
        layout: Some(XisoLayout::RawOrStripped),
        state: XisoReversibleState::PlayableButNotByteExact,
        shrinkable_bytes: None,
        reason: "the image is structurally readable, but rebuilding or removing padding cannot reconstruct the original raw bytes".into(),
    }
}

/// Keep the future output-collision contract explicit even though the current
/// research result refuses to create an output at all.
pub fn propose_reversible_shrink(
    output: impl Into<PathBuf>,
    output_exists: bool,
) -> Result<(), XisoShrinkError> {
    let output = output.into();
    if output_exists {
        return Err(XisoShrinkError::OutputExists(output));
    }
    Err(XisoShrinkError::NoByteExactPath)
}

/// Return the byte savings for two already-created representations. This is a
/// display helper only; it never authorizes a transformation.
pub fn savings_bytes(original_size: u64, derived_size: u64) -> u64 {
    original_size.saturating_sub(derived_size)
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_bytes_are_not_modified_and_hash_is_retained() {
        let source = b"not an XISO".to_vec();
        let before = source.clone();
        let result = analyze_xiso_bytes(&source);
        assert_eq!(source, before);
        assert_eq!(result.source_sha256, hex_sha256(&before));
    }

    #[test]
    fn unsupported_image_fails_closed() {
        let result = analyze_xiso_bytes(&vec![0u8; 33 * 2048]);
        assert_eq!(result.state, XisoReversibleState::Unsupported);
        assert!(result.shrinkable_bytes.is_none());
    }

    #[test]
    fn malformed_xdvdfs_is_rejected_without_unbounded_walk() {
        let mut source = vec![0u8; 33 * 2048];
        source[32 * 2048..32 * 2048 + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        let result = analyze_xiso_bytes(&source);
        assert_eq!(result.state, XisoReversibleState::Unsupported);
        assert!(result.reason.contains("malformed"));
    }

    #[test]
    fn readable_xdvdfs_stays_playable_but_not_byte_exact() {
        let source = crate::xdvdfs_traversal::test_support::synthetic_single_root_file_image(
            "default.xbe",
            b"XBEH synthetic",
        );
        let result = analyze_xiso_bytes(&source);
        assert_eq!(result.state, XisoReversibleState::PlayableButNotByteExact);
        assert_eq!(result.layout, Some(XisoLayout::RawOrStripped));
        assert!(result.shrinkable_bytes.is_none());
    }

    #[test]
    fn output_collision_is_refused_before_no_path_result() {
        let error = propose_reversible_shrink("existing.xiso", true).unwrap_err();
        assert!(matches!(error, XisoShrinkError::OutputExists(_)));
    }

    #[test]
    fn exact_restore_state_is_not_claimed_by_the_analyzer() {
        let result = analyze_xiso_bytes(b"not enough");
        assert_ne!(result.state, XisoReversibleState::ReversibleExact);
        assert!(matches!(
            propose_reversible_shrink("derived.xiso", false),
            Err(XisoShrinkError::NoByteExactPath)
        ));
    }

    #[test]
    fn playable_but_not_exact_is_distinct_from_unsupported_and_unknown() {
        assert_ne!(
            XisoReversibleState::PlayableButNotByteExact,
            XisoReversibleState::Unsupported
        );
        assert_ne!(
            XisoReversibleState::PlayableButNotByteExact,
            XisoReversibleState::Unknown
        );
    }

    #[test]
    fn savings_is_saturating_and_does_not_authorize_deletion() {
        assert_eq!(savings_bytes(100, 60), 40);
        assert_eq!(savings_bytes(60, 100), 0);
    }
}
