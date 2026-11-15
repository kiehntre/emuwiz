//! The one rule for an archive member name that is safe to hand to an
//! external extractor and to use as a destination-relative path.
//!
//! Every archive backend (7-Zip for `.7z`/`.rar`/`.lha`, the ZIP reader) used
//! to carry its own version of this check, and they disagreed: one accepted
//! `folder/../game.rom` because it "stays inside", another accepted
//! backslashes as separators, a third refused `*` and `?`. Night Watch S01-03
//! found that 7-Zip treats a member argument as a wildcard pattern unless
//! told otherwise, and that comparing duplicates on the raw string misses
//! `a/../b.bin` against `b.bin`. The rule here is deliberately the strictest
//! of the three, and the backends share it.
//!
//! A name is accepted only when **all** of these hold:
//!
//! - it is non-empty and at most [`MAX_MEMBER_NAME_BYTES`] bytes;
//! - it contains no control character (so no embedded newline can
//!   masquerade as a listing line, Night Watch S01-01), no NUL, and no
//!   backslash (a separator on one platform and an ordinary character on
//!   another: ambiguous, so refused rather than normalised);
//! - it contains no `*` or `?`, the characters 7-Zip's name matching treats
//!   as wildcards, and no leading `-` (which 7-Zip would read as a switch
//!   without `--`; the backends pass `--`, this is defence in depth);
//! - it is relative: no leading `/`, no `X:` drive prefix in any component;
//! - every `/`-separated component is a plain name: not empty (no `//`, no
//!   trailing `/`), not `.` and not `..`. Nothing is normalised away; a name
//!   that needs normalising is refused.
//!
//! The rule is intentionally more conservative than any single format
//! requires. An archive refused here is reported as unsafe, never silently
//! re-interpreted.

use std::path::{Component, Path, PathBuf};

/// Longest member name accepted, in bytes.
pub const MAX_MEMBER_NAME_BYTES: usize = 4096;

/// Why a member name was refused. Each is a distinct, explainable reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberNameRefusal {
    Empty,
    TooLong,
    ControlCharacter,
    Backslash,
    WildcardCharacter,
    LeadingDash,
    Absolute,
    DrivePrefix,
    EmptyComponent,
    DotComponent,
    ParentComponent,
}

impl MemberNameRefusal {
    pub fn detail(self) -> &'static str {
        match self {
            Self::Empty => "the member name is empty",
            Self::TooLong => "the member name is longer than the accepted limit",
            Self::ControlCharacter => "the member name contains a control character",
            Self::Backslash => "the member name contains a backslash",
            Self::WildcardCharacter => "the member name contains a wildcard character (* or ?)",
            Self::LeadingDash => "the member name starts with a dash",
            Self::Absolute => "the member name is absolute",
            Self::DrivePrefix => "the member name contains a drive prefix",
            Self::EmptyComponent => "the member name contains an empty path component",
            Self::DotComponent => "the member name contains a `.` component",
            Self::ParentComponent => "the member name contains a `..` component",
        }
    }
}

/// Checks `name` against the rule documented on this module.
pub fn check_member_name(name: &str) -> Result<(), MemberNameRefusal> {
    if name.is_empty() {
        return Err(MemberNameRefusal::Empty);
    }
    if name.len() > MAX_MEMBER_NAME_BYTES {
        return Err(MemberNameRefusal::TooLong);
    }
    if name.chars().any(char::is_control) {
        return Err(MemberNameRefusal::ControlCharacter);
    }
    if name.contains('\\') {
        return Err(MemberNameRefusal::Backslash);
    }
    if name.contains(['*', '?']) {
        return Err(MemberNameRefusal::WildcardCharacter);
    }
    if name.starts_with('-') {
        return Err(MemberNameRefusal::LeadingDash);
    }
    if name.starts_with('/') {
        return Err(MemberNameRefusal::Absolute);
    }
    for component in name.split('/') {
        match component {
            "" => return Err(MemberNameRefusal::EmptyComponent),
            "." => return Err(MemberNameRefusal::DotComponent),
            ".." => return Err(MemberNameRefusal::ParentComponent),
            _ => {}
        }
        if matches!(component.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic()) {
            return Err(MemberNameRefusal::DrivePrefix);
        }
    }
    Ok(())
}

/// Whether `name` passes [`check_member_name`].
pub fn is_safe_member_name(name: &str) -> bool {
    check_member_name(name).is_ok()
}

/// The accepted name as a destination-relative path. Because the rule
/// admits only plain components, the returned path has exactly one
/// `Component::Normal` per `/`-separated part and nothing else - which a
/// debug assertion re-checks, since that is the property extraction relies
/// on.
pub fn safe_member_relative_path(name: &str) -> Result<PathBuf, MemberNameRefusal> {
    check_member_name(name)?;
    let path = PathBuf::from(name);
    debug_assert!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_)))
    );
    Ok(path)
}

/// Whether `name` would collide with `other` on a case-insensitive file
/// system. Both are compared after ASCII lower-casing only, so two names
/// that differ in non-ASCII case are reported as distinct and the extracted
/// tree is verified afterwards anyway.
pub fn case_fold_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Whether `path` is a destination-relative path made only of normal
/// components, as [`safe_member_relative_path`] produces.
pub fn is_plain_relative_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        && path.components().next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_nested_names_are_accepted() {
        for name in [
            "game.bin",
            "Game/Game.Slave",
            "a/b/c.d",
            "with space/x-y_z.bin",
            "ünïcödé/τ.bin",
        ] {
            assert_eq!(check_member_name(name), Ok(()), "{name}");
            let path = safe_member_relative_path(name).unwrap();
            assert!(is_plain_relative_path(&path));
        }
    }

    #[test]
    fn every_refusal_reason_fires_on_its_own_example() {
        let cases = [
            ("", MemberNameRefusal::Empty),
            ("a\nb", MemberNameRefusal::ControlCharacter),
            ("a\rb", MemberNameRefusal::ControlCharacter),
            ("a\0b", MemberNameRefusal::ControlCharacter),
            ("a\x1bb", MemberNameRefusal::ControlCharacter),
            ("a\tb", MemberNameRefusal::ControlCharacter),
            ("folder\\game.rom", MemberNameRefusal::Backslash),
            ("x?.bin", MemberNameRefusal::WildcardCharacter),
            ("*.bin", MemberNameRefusal::WildcardCharacter),
            ("-flag.bin", MemberNameRefusal::LeadingDash),
            ("/etc/passwd", MemberNameRefusal::Absolute),
            ("C:/x", MemberNameRefusal::DrivePrefix),
            ("z:x", MemberNameRefusal::DrivePrefix),
            ("a/C:/x", MemberNameRefusal::DrivePrefix),
            ("a//b", MemberNameRefusal::EmptyComponent),
            ("a/", MemberNameRefusal::EmptyComponent),
            ("./a", MemberNameRefusal::DotComponent),
            ("a/./b", MemberNameRefusal::DotComponent),
            ("../a", MemberNameRefusal::ParentComponent),
            ("a/../b.bin", MemberNameRefusal::ParentComponent),
            ("folder/../game.rom", MemberNameRefusal::ParentComponent),
        ];
        for (name, expected) in cases {
            assert_eq!(check_member_name(name), Err(expected), "{name:?}");
        }
        let long = "a".repeat(MAX_MEMBER_NAME_BYTES + 1);
        assert_eq!(check_member_name(&long), Err(MemberNameRefusal::TooLong));
        assert!(is_safe_member_name(&"a".repeat(MAX_MEMBER_NAME_BYTES)));
    }

    #[test]
    fn names_differing_only_in_ascii_case_share_a_fold_key() {
        assert_eq!(case_fold_key("Game/A.BIN"), case_fold_key("game/a.bin"));
        assert_ne!(case_fold_key("a.bin"), case_fold_key("b.bin"));
    }
}
