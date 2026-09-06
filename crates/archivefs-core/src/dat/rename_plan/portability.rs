//! Explicit filename portability checks for rename planning.
//!
//! The rename planner derives a basename; it does not currently know which
//! filesystem will receive a future apply.  This module keeps that concern
//! explicit and side-effect free so callers can validate a proposed component
//! against the intended destination profile without changing the approved
//! sanitisation policy.

use std::collections::BTreeSet;

/// Filesystem naming profiles supported by the rename portability contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortabilityTarget {
    /// A conventional Linux filesystem (ext4, XFS, btrfs, …).
    Linux,
    /// Windows/NTFS-compatible naming rules.
    WindowsNtfs,
    /// FAT/exFAT-style removable-media naming rules.
    FatExfat,
}

impl PortabilityTarget {
    fn windows_compatible(self) -> bool {
        matches!(self, Self::WindowsNtfs | Self::FatExfat)
    }
}

/// A concrete reason a proposed filename component is not portable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortabilityIssue {
    EmptyComponent,
    DotComponent,
    PathSeparator(char),
    ControlCharacter(char),
    ForbiddenCharacter(char),
    ReservedWindowsDeviceName,
    TrailingDot,
    TrailingSpace,
    ComponentTooLong { bytes: usize, max_bytes: usize },
}

impl std::fmt::Display for PortabilityIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyComponent => write!(f, "empty filename component"),
            Self::DotComponent => write!(f, "reserved '.' or '..' component"),
            Self::PathSeparator(ch) => write!(f, "path separator {ch:?}"),
            Self::ControlCharacter(ch) => write!(f, "control character U+{:04X}", *ch as u32),
            Self::ForbiddenCharacter(ch) => write!(f, "forbidden character {ch:?}"),
            Self::ReservedWindowsDeviceName => write!(f, "reserved Windows device name"),
            Self::TrailingDot => write!(f, "trailing dot"),
            Self::TrailingSpace => write!(f, "trailing space"),
            Self::ComponentTooLong { bytes, max_bytes } => {
                write!(f, "component is {bytes} bytes (maximum {max_bytes})")
            }
        }
    }
}

/// The result of validating one component against one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortabilityAssessment {
    pub target: PortabilityTarget,
    pub issues: Vec<PortabilityIssue>,
}

impl PortabilityAssessment {
    pub fn is_portable(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Validate a proposed basename component without rewriting it.
pub fn assess_component(name: &str, target: PortabilityTarget) -> PortabilityAssessment {
    let mut issues = BTreeSet::new();
    if name.is_empty() {
        issues.insert(PortabilityIssue::EmptyComponent);
    }
    if name == "." || name == ".." {
        issues.insert(PortabilityIssue::DotComponent);
    }
    for ch in name.chars() {
        if ch == '/' || ch == '\\' {
            issues.insert(PortabilityIssue::PathSeparator(ch));
        } else if ch.is_control() {
            issues.insert(PortabilityIssue::ControlCharacter(ch));
        } else if target.windows_compatible()
            && matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*')
        {
            issues.insert(PortabilityIssue::ForbiddenCharacter(ch));
        }
    }

    if target.windows_compatible() {
        if name.ends_with('.') {
            issues.insert(PortabilityIssue::TrailingDot);
        }
        if name.ends_with(' ') {
            issues.insert(PortabilityIssue::TrailingSpace);
        }
        if is_reserved_windows_device_name(name) {
            issues.insert(PortabilityIssue::ReservedWindowsDeviceName);
        }
    }

    // All three targets use a 255-byte maximum component in the contract.
    // This is a byte limit, not a character limit, so valid Unicode is
    // preserved and only genuinely oversized names are refused.
    if name.len() > MAX_COMPONENT_BYTES {
        issues.insert(PortabilityIssue::ComponentTooLong {
            bytes: name.len(),
            max_bytes: MAX_COMPONENT_BYTES,
        });
    }

    PortabilityAssessment {
        target,
        issues: issues.into_iter().collect(),
    }
}

/// Validate the same proposed component against several intended targets.
pub fn assess_component_for_targets(
    name: &str,
    targets: impl IntoIterator<Item = PortabilityTarget>,
) -> Vec<PortabilityAssessment> {
    targets
        .into_iter()
        .map(|target| assess_component(name, target))
        .collect()
}

/// Return the deterministic case-folding key used by case-insensitive
/// destination targets. Linux callers should not use this to reject names;
/// it exists to make portability-aware collision checks explicit.
pub fn case_insensitive_key(name: &str, target: PortabilityTarget) -> Option<String> {
    target
        .windows_compatible()
        .then(|| name.to_ascii_lowercase())
}

/// The contract's maximum filename-component size, in bytes.
pub const MAX_COMPONENT_BYTES: usize = 255;

fn is_reserved_windows_device_name(name: &str) -> bool {
    // Windows trims trailing dots/spaces before interpreting a device stem and
    // treats an extension-bearing form (CON.txt) as reserved too.
    let trimmed = name.trim_end_matches(|ch| ch == '.' || ch == ' ');
    let stem = trimmed.split('.').next().unwrap_or_default();
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper[3..].chars().all(|ch| ('1'..='9').contains(&ch)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_and_unicode_names_are_portable() {
        assert!(
            assess_component("Pokémon (日本).bin", PortabilityTarget::WindowsNtfs).is_portable()
        );
    }

    #[test]
    fn linux_allows_windows_punctuation_but_windows_does_not() {
        assert!(assess_component("Game:?.bin", PortabilityTarget::Linux).is_portable());
        assert!(!assess_component("Game:?.bin", PortabilityTarget::WindowsNtfs).is_portable());
    }

    #[test]
    fn separators_and_controls_are_always_rejected() {
        for name in ["a/b.bin", "a\\b.bin", "a\u{1}b.bin"] {
            assert!(!assess_component(name, PortabilityTarget::Linux).is_portable());
            assert!(!assess_component(name, PortabilityTarget::FatExfat).is_portable());
        }
    }

    #[test]
    fn windows_reserved_names_include_extensions_and_case() {
        for name in ["CON", "con.txt", "AUX ", "com1.bin", "LPT9.data"] {
            let result = assess_component(name, PortabilityTarget::WindowsNtfs);
            assert!(
                result
                    .issues
                    .contains(&PortabilityIssue::ReservedWindowsDeviceName),
                "{name}"
            );
        }
        assert!(assess_component("CONSOLE.bin", PortabilityTarget::WindowsNtfs).is_portable());
    }

    #[test]
    fn trailing_dot_and_space_are_windows_and_fat_issues() {
        for target in [PortabilityTarget::WindowsNtfs, PortabilityTarget::FatExfat] {
            assert!(!assess_component("game.", target).is_portable());
            assert!(!assess_component("game ", target).is_portable());
        }
        assert!(assess_component("game.", PortabilityTarget::Linux).is_portable());
    }

    #[test]
    fn empty_dot_and_oversized_components_are_reported() {
        assert!(!assess_component("", PortabilityTarget::Linux).is_portable());
        assert!(!assess_component(".", PortabilityTarget::Linux).is_portable());
        let long = "x".repeat(256);
        assert!(
            assess_component(&long, PortabilityTarget::Linux)
                .issues
                .iter()
                .any(|issue| matches!(issue, PortabilityIssue::ComponentTooLong { .. }))
        );
    }

    #[test]
    fn case_insensitive_collision_key_is_target_specific() {
        assert_eq!(
            case_insensitive_key("Game.BIN", PortabilityTarget::WindowsNtfs),
            Some("game.bin".to_string())
        );
        assert_eq!(
            case_insensitive_key("Game.BIN", PortabilityTarget::Linux),
            None
        );
    }

    #[test]
    fn assessment_is_deterministic_across_targets() {
        let first = assess_component_for_targets(
            "CON?.bin",
            [PortabilityTarget::Linux, PortabilityTarget::WindowsNtfs],
        );
        let second = assess_component_for_targets(
            "CON?.bin",
            [PortabilityTarget::Linux, PortabilityTarget::WindowsNtfs],
        );
        assert_eq!(first, second);
    }
}
