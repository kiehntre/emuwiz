//! Built-in table of known compatibility limits.
//!
//! This is the only source of compatibility facts the rules accept. It is
//! compiled in and covered by the rule-set digest, so a report says exactly
//! which table produced it. A claim about compatibility that arrives from
//! anywhere else (a file, a web page, an AI suggestion) is not evidence.
//!
//! An entry is added only when the limit is already enforced or documented
//! elsewhere in this codebase, and it says where.

use super::evidence::{EvidenceArea, EvidenceId, EvidenceSet, Fact, Trust};

/// A minimum supported version of one emulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinimumVersion {
    pub id: &'static str,
    pub version: u32,
    pub emulator: &'static str,
    /// `(major, minor)`. Versions that do not parse are *not* treated as
    /// too old: the limit is then unknown, not violated.
    pub minimum: (u64, u64),
    pub summary: &'static str,
    /// Where in this codebase the limit comes from.
    pub basis: &'static str,
}

pub const MINIMUM_VERSIONS: &[MinimumVersion] = &[MinimumVersion {
    id: "openrct2.minimum_version",
    version: 1,
    emulator: "openrct2",
    minimum: (0, 4),
    summary: "OpenRCT2 older than 0.4 is not supported: the data-path option EmuWiz \
              launches it with is established in the 0.4 series.",
    basis: "launch/openrct2.rs MIN_SUPPORTED",
}];

/// `(major, minor)` from the start of a version string such as `0.4.11` or
/// `v0.4-beta`. `None` when the first two components are not numbers.
pub fn major_minor(version: &str) -> Option<(u64, u64)> {
    let version = version.trim().trim_start_matches(['v', 'V']);
    let mut parts = version.split(['.', '-', '+', ' ']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Whether an observed version violates an entry: `Some(true)` too old,
/// `Some(false)` supported, `None` not determinable.
pub fn violated(entry: &MinimumVersion, version: Option<&str>) -> Option<bool> {
    major_minor(version?).map(|observed| observed < entry.minimum)
}

/// Record every table entry for `emulator` against an observed version.
/// An entry whose outcome cannot be determined is recorded as unavailable,
/// never as satisfied.
pub fn record(set: &mut EvidenceSet, emulator: &str, version: Option<&str>) -> Vec<EvidenceId> {
    let mut ids = Vec::new();
    for entry in MINIMUM_VERSIONS
        .iter()
        .filter(|entry| entry.emulator == emulator)
    {
        let source = format!("built-in compatibility table ({})", entry.basis);
        let id = match violated(entry, version) {
            Some(violated) => set.observe(
                EvidenceArea::KnownLimitations,
                Trust::Measured,
                source,
                Fact::KnownLimitation {
                    id: entry.id.to_string(),
                    summary: entry.summary.to_string(),
                    violated,
                },
            ),
            None => set.unavailable(
                EvidenceArea::KnownLimitations,
                source,
                format!("{}: the installed version is not known", entry.id),
            ),
        };
        ids.extend(id);
    }
    ids
}

/// Whether `id` and `summary` are exactly an entry of the table. The rules
/// accept a compatibility limit only when this holds.
pub fn is_table_entry(id: &str, summary: &str) -> bool {
    MINIMUM_VERSIONS
        .iter()
        .any(|entry| entry.id == id && entry.summary == summary)
}

pub(super) fn digest_text() -> String {
    MINIMUM_VERSIONS
        .iter()
        .map(|entry| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{:?}",
                entry.id, entry.version, entry.emulator, entry.minimum
            )
        })
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

#[cfg(test)]
#[path = "limitations/tests.rs"]
mod tests;
