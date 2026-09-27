//! Read-only compatibility analysis for a selected stack of cheats.
//!
//! This layer deliberately does not decode or install cheats. Existing
//! format-specific parsers produce the IR; this module only compares the
//! semantics that are actually known and reports uncertainty explicitly.

use serde::{Deserialize, Serialize};

use super::cheat_ir::{CheatDocument, CheatOperation, CheatPlatform};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatStackReadiness {
    Compatible,
    CompatibleWithWarnings,
    OrderSensitive,
    Conflicting,
    WrongRevision,
    Ambiguous,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatConflictSeverity {
    Informational,
    Warning,
    Blocking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatConflictKind {
    SameAddressSameValue,
    SameAddressDifferentValue,
    OverlappingRange,
    ConditionalOverlap,
    MasterCodeMismatch,
    RevisionMismatch,
    PlatformMismatch,
    ExecutionOrderSensitive,
    UnsupportedOpcodeInteraction,
    DuplicateCheat,
    PotentialPointerAlias,
    AlwaysOnVsToggleConflict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatMemoryRange {
    pub address: u64,
    pub width_bytes: u8,
    pub value: Option<u32>,
    pub condition: CheatCondition,
    pub continuous: bool,
    pub pointer_based: bool,
}

impl CheatMemoryRange {
    fn end(&self) -> u64 {
        self.address
            .saturating_add(u64::from(self.width_bytes.saturating_sub(1)))
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.address <= other.end() && other.address <= self.end()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatCondition {
    Always,
    MemoryEquals {
        address: u64,
        width_bytes: u8,
        value: u32,
    },
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatCompatibilityOperation {
    Write(CheatMemoryRange),
    Increment(CheatMemoryRange),
    Decrement(CheatMemoryRange),
    PointerWrite(CheatMemoryRange),
    Unknown { raw: String, reason: String },
}

impl CheatCompatibilityOperation {
    fn range(&self) -> Option<&CheatMemoryRange> {
        match self {
            Self::Write(range)
            | Self::Increment(range)
            | Self::Decrement(range)
            | Self::PointerWrite(range) => Some(range),
            Self::Unknown { .. } => None,
        }
    }

    fn is_pointer(&self) -> bool {
        matches!(self, Self::PointerWrite(range) if range.pointer_based)
    }

    fn is_mutating_arithmetic(&self) -> bool {
        matches!(self, Self::Increment(_) | Self::Decrement(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatRevisionEvidence {
    ExactHash {
        hash: String,
    },
    VerifiedIdentity {
        identity: String,
        revision: Option<String>,
    },
    ProviderDeclared {
        revision: String,
    },
    TitleOnly {
        title: String,
    },
    Unknown,
}

impl CheatRevisionEvidence {
    fn key(&self) -> Option<String> {
        match self {
            Self::ExactHash { hash } => Some(format!("hash:{hash}")),
            Self::VerifiedIdentity { identity, revision } => Some(format!(
                "identity:{identity}:{}",
                revision.as_deref().unwrap_or("")
            )),
            Self::ProviderDeclared { revision } => Some(format!("provider:{revision}")),
            Self::TitleOnly { title } => Some(format!("title:{title}")),
            Self::Unknown => None,
        }
    }

    fn strength(&self) -> u8 {
        match self {
            Self::ExactHash { .. } => 4,
            Self::VerifiedIdentity { .. } => 3,
            Self::ProviderDeclared { .. } => 2,
            Self::TitleOnly { .. } => 1,
            Self::Unknown => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatMasterCodeRequirement {
    None,
    Required { code: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatCompatibilityEntry {
    pub id: String,
    pub title: String,
    pub platform: CheatPlatform,
    pub provider: String,
    pub source: String,
    pub operations: Vec<CheatCompatibilityOperation>,
    pub revision: CheatRevisionEvidence,
    pub master_code: CheatMasterCodeRequirement,
    pub original_code_id: Option<String>,
}

impl CheatCompatibilityEntry {
    /// Converts the existing format-neutral IR without inventing semantics.
    pub fn from_document(
        id: impl Into<String>,
        document: &CheatDocument,
        provider: impl Into<String>,
        source: impl Into<String>,
        revision: CheatRevisionEvidence,
        master_code: CheatMasterCodeRequirement,
    ) -> Self {
        Self {
            id: id.into(),
            title: document.title.clone(),
            platform: document.platform.clone(),
            provider: provider.into(),
            source: source.into(),
            operations: document
                .operations
                .iter()
                .map(CheatCompatibilityOperation::from_ir)
                .collect(),
            revision,
            master_code,
            original_code_id: None,
        }
    }
}

impl CheatCompatibilityOperation {
    pub fn from_ir(operation: &CheatOperation) -> Self {
        let write = |address, width_bytes, value, continuous| {
            Self::Write(CheatMemoryRange {
                address,
                width_bytes,
                value: Some(value),
                condition: CheatCondition::Always,
                continuous,
                pointer_based: false,
            })
        };
        match operation {
            CheatOperation::Write8 { address, value } => {
                write(*address, 1, u32::from(*value), false)
            }
            CheatOperation::Write16 { address, value } => {
                write(*address, 2, u32::from(*value), false)
            }
            CheatOperation::Write32 { address, value } => write(*address, 4, *value, false),
            CheatOperation::ConditionalWrite8 {
                address,
                value,
                compare,
            } => Self::Write(CheatMemoryRange {
                address: *address,
                width_bytes: 1,
                value: Some(u32::from(*value)),
                condition: CheatCondition::MemoryEquals {
                    address: *address,
                    width_bytes: 1,
                    value: u32::from(*compare),
                },
                continuous: false,
                pointer_based: false,
            }),
            CheatOperation::OnFrameWrite8 { address, value } => {
                write(*address, 1, u32::from(*value), true)
            }
            CheatOperation::OnFrameWrite16 { address, value } => {
                write(*address, 2, u32::from(*value), true)
            }
            CheatOperation::OnFrameWrite32 { address, value } => write(*address, 4, *value, true),
            CheatOperation::UnsupportedRaw { raw, reason, .. } => Self::Unknown {
                raw: raw.clone(),
                reason: reason.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatConflict {
    pub kind: CheatConflictKind,
    pub severity: CheatConflictSeverity,
    pub entry_a: String,
    pub entry_b: Option<String>,
    pub provider_a: String,
    pub provider_b: Option<String>,
    pub evidence: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatCompatibilityReport {
    pub selected_cheats: usize,
    pub operation_count: usize,
    pub readiness: CheatStackReadiness,
    pub conflicts: Vec<CheatConflict>,
    pub revision_evidence: Vec<CheatRevisionEvidence>,
    pub master_code_requirements: Vec<CheatMasterCodeRequirement>,
}

impl CheatCompatibilityReport {
    pub fn blocking_conflicts(&self) -> impl Iterator<Item = &CheatConflict> {
        self.conflicts
            .iter()
            .filter(|conflict| conflict.severity == CheatConflictSeverity::Blocking)
    }

    pub fn can_apply(&self) -> bool {
        !matches!(
            self.readiness,
            CheatStackReadiness::Conflicting
                | CheatStackReadiness::WrongRevision
                | CheatStackReadiness::Unsupported
        ) && self.blocking_conflicts().next().is_none()
    }
}

pub fn analyze_cheat_stack(entries: &[CheatCompatibilityEntry]) -> CheatCompatibilityReport {
    let mut conflicts = Vec::new();
    let operation_count = entries.iter().map(|entry| entry.operations.len()).sum();
    for (index, entry) in entries.iter().enumerate() {
        for other in entries.iter().skip(index + 1) {
            if normalized_entry_key(entry) == normalized_entry_key(other) {
                conflicts.push(conflict(
                    CheatConflictKind::DuplicateCheat,
                    CheatConflictSeverity::Informational,
                    entry,
                    Some(other),
                    "The same normalized cheat was selected more than once.",
                ));
            }
            if entry.platform != other.platform {
                conflicts.push(conflict(
                    CheatConflictKind::PlatformMismatch,
                    CheatConflictSeverity::Blocking,
                    entry,
                    Some(other),
                    "These cheats target different platforms.",
                ));
            }
            compare_revisions(entry, other, &mut conflicts);
            compare_master_codes(entry, other, &mut conflicts);
            for left in &entry.operations {
                for right in &other.operations {
                    compare_operations(entry, other, left, right, &mut conflicts);
                }
            }
        }
    }
    let mut readiness = CheatStackReadiness::Compatible;
    if conflicts
        .iter()
        .any(|item| item.severity == CheatConflictSeverity::Blocking)
    {
        readiness = if conflicts
            .iter()
            .any(|item| item.kind == CheatConflictKind::RevisionMismatch)
        {
            CheatStackReadiness::WrongRevision
        } else {
            CheatStackReadiness::Conflicting
        };
    } else if conflicts
        .iter()
        .any(|item| item.kind == CheatConflictKind::UnsupportedOpcodeInteraction)
    {
        readiness = CheatStackReadiness::Unsupported;
    } else if conflicts
        .iter()
        .any(|item| item.kind == CheatConflictKind::ExecutionOrderSensitive)
    {
        readiness = CheatStackReadiness::OrderSensitive;
    } else if conflicts.iter().any(|item| {
        item.kind == CheatConflictKind::RevisionMismatch
            && item.severity == CheatConflictSeverity::Warning
    }) {
        readiness = CheatStackReadiness::Ambiguous;
    } else if !conflicts.is_empty() {
        readiness = CheatStackReadiness::CompatibleWithWarnings;
    }
    CheatCompatibilityReport {
        selected_cheats: entries.len(),
        operation_count,
        readiness,
        conflicts,
        revision_evidence: entries.iter().map(|entry| entry.revision.clone()).collect(),
        master_code_requirements: entries
            .iter()
            .map(|entry| entry.master_code.clone())
            .collect(),
    }
}

fn normalized_entry_key(entry: &CheatCompatibilityEntry) -> String {
    format!(
        "{:?}|{:?}|{:?}",
        entry.platform, entry.operations, entry.master_code
    )
}

fn compare_revisions(
    a: &CheatCompatibilityEntry,
    b: &CheatCompatibilityEntry,
    conflicts: &mut Vec<CheatConflict>,
) {
    let (Some(left), Some(right)) = (a.revision.key(), b.revision.key()) else {
        return;
    };
    if left == right {
        return;
    }
    let stronger = a.revision.strength().max(b.revision.strength());
    let weaker = a.revision.strength().min(b.revision.strength());
    if stronger >= 3 && weaker >= 2 {
        conflicts.push(conflict(
            CheatConflictKind::RevisionMismatch,
            CheatConflictSeverity::Blocking,
            a,
            Some(b),
            "The selected cheats name different strong game revisions.",
        ));
    } else if stronger == 1 && weaker == 1 {
        conflicts.push(conflict(
            CheatConflictKind::RevisionMismatch,
            CheatConflictSeverity::Warning,
            a,
            Some(b),
            "Title-only revision evidence is ambiguous; exact identity is required.",
        ));
    }
}

fn compare_master_codes(
    a: &CheatCompatibilityEntry,
    b: &CheatCompatibilityEntry,
    conflicts: &mut Vec<CheatConflict>,
) {
    let (
        CheatMasterCodeRequirement::Required { code: left },
        CheatMasterCodeRequirement::Required { code: right },
    ) = (&a.master_code, &b.master_code)
    else {
        return;
    };
    if left != right {
        conflicts.push(conflict(
            CheatConflictKind::MasterCodeMismatch,
            CheatConflictSeverity::Blocking,
            a,
            Some(b),
            "The cheats require different master or enabler codes; EmuWiz will not choose one.",
        ));
    }
}

fn compare_operations(
    a: &CheatCompatibilityEntry,
    b: &CheatCompatibilityEntry,
    left: &CheatCompatibilityOperation,
    right: &CheatCompatibilityOperation,
    conflicts: &mut Vec<CheatConflict>,
) {
    if matches!(left, CheatCompatibilityOperation::Unknown { .. })
        || matches!(right, CheatCompatibilityOperation::Unknown { .. })
    {
        conflicts.push(conflict(
            CheatConflictKind::UnsupportedOpcodeInteraction,
            CheatConflictSeverity::Warning,
            a,
            Some(b),
            "At least one operation is unknown, so compatibility cannot be proven.",
        ));
        return;
    }
    if left.is_pointer() || right.is_pointer() {
        conflicts.push(conflict(
            CheatConflictKind::PotentialPointerAlias,
            CheatConflictSeverity::Warning,
            a,
            Some(b),
            "A dynamic pointer may resolve into an overlapping address range.",
        ));
    }
    let (Some(first), Some(second)) = (left.range(), right.range()) else {
        return;
    };
    if !first.overlaps(second) {
        return;
    }
    if !matches!(&first.condition, CheatCondition::Always)
        || !matches!(&second.condition, CheatCondition::Always)
    {
        conflicts.push(conflict(
            CheatConflictKind::ConditionalOverlap,
            CheatConflictSeverity::Warning,
            a,
            Some(b),
            "Conditional operations may affect the same memory when their conditions overlap.",
        ));
    }
    if first.address == second.address
        && first.width_bytes == second.width_bytes
        && first.value == second.value
    {
        conflicts.push(conflict(
            CheatConflictKind::SameAddressSameValue,
            CheatConflictSeverity::Informational,
            a,
            Some(b),
            "Both cheats write the same value to the same address.",
        ));
    } else if first.address == second.address && first.value != second.value {
        conflicts.push(conflict(
            CheatConflictKind::SameAddressDifferentValue,
            CheatConflictSeverity::Blocking,
            a,
            Some(b),
            "Both cheats write different values to the same address.",
        ));
    } else {
        conflicts.push(conflict(
            CheatConflictKind::OverlappingRange,
            CheatConflictSeverity::Blocking,
            a,
            Some(b),
            "Multi-byte writes overlap partially.",
        ));
    }
    if left.is_mutating_arithmetic()
        || right.is_mutating_arithmetic()
        || first.continuous != second.continuous
    {
        conflicts.push(conflict(
            CheatConflictKind::ExecutionOrderSensitive,
            CheatConflictSeverity::Warning,
            a,
            Some(b),
            "The final value can depend on execution order or continuous-write timing.",
        ));
    }
    if first.continuous != second.continuous {
        conflicts.push(conflict(
            CheatConflictKind::AlwaysOnVsToggleConflict,
            CheatConflictSeverity::Warning,
            a,
            Some(b),
            "A continuous write may overwrite a toggle or one-shot operation.",
        ));
    }
}

fn conflict(
    kind: CheatConflictKind,
    severity: CheatConflictSeverity,
    a: &CheatCompatibilityEntry,
    b: Option<&CheatCompatibilityEntry>,
    reason: &str,
) -> CheatConflict {
    CheatConflict {
        kind,
        severity,
        entry_a: a.id.clone(),
        entry_b: b.map(|entry| entry.id.clone()),
        provider_a: a.provider.clone(),
        provider_b: b.map(|entry| entry.provider.clone()),
        evidence: a
            .operations
            .iter()
            .filter_map(|operation| {
                operation
                    .range()
                    .map(|range| format!("0x{:08X}/{}", range.address, range.width_bytes))
            })
            .collect(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, operation: CheatCompatibilityOperation) -> CheatCompatibilityEntry {
        CheatCompatibilityEntry {
            id: id.into(),
            title: id.into(),
            platform: CheatPlatform::GameCube,
            provider: "test-provider".into(),
            source: format!("{id}.cht"),
            operations: vec![operation],
            revision: CheatRevisionEvidence::ExactHash {
                hash: "same".into(),
            },
            master_code: CheatMasterCodeRequirement::None,
            original_code_id: Some(id.into()),
        }
    }
    fn write(address: u64, width: u8, value: u32) -> CheatCompatibilityOperation {
        CheatCompatibilityOperation::Write(CheatMemoryRange {
            address,
            width_bytes: width,
            value: Some(value),
            condition: CheatCondition::Always,
            continuous: false,
            pointer_based: false,
        })
    }

    #[test]
    fn detects_same_and_different_values() {
        assert_eq!(
            analyze_cheat_stack(&[entry("a", write(1, 1, 7)), entry("b", write(1, 1, 7))])
                .readiness,
            CheatStackReadiness::CompatibleWithWarnings
        );
        let report = analyze_cheat_stack(&[entry("a", write(1, 1, 7)), entry("b", write(1, 1, 8))]);
        assert_eq!(report.readiness, CheatStackReadiness::Conflicting);
        assert!(!report.can_apply());
    }

    #[test]
    fn detects_partial_overlap_master_platform_and_duplicate() {
        let mut other = entry("b", write(2, 4, 1));
        other.master_code = CheatMasterCodeRequirement::Required { code: "B".into() };
        other.platform = CheatPlatform::Ps2;
        let duplicate = entry("a-copy", write(1, 2, 7));
        let mut master_a = entry("master-a", write(9, 1, 1));
        master_a.master_code = CheatMasterCodeRequirement::Required { code: "A".into() };
        let report = analyze_cheat_stack(&[entry("a", write(1, 2, 7)), duplicate, master_a, other]);
        assert!(
            report
                .conflicts
                .iter()
                .any(|item| item.kind == CheatConflictKind::DuplicateCheat)
        );
        assert!(
            report
                .conflicts
                .iter()
                .any(|item| item.kind == CheatConflictKind::MasterCodeMismatch)
        );
        assert!(
            report
                .conflicts
                .iter()
                .any(|item| item.kind == CheatConflictKind::PlatformMismatch)
        );
        assert!(
            report
                .conflicts
                .iter()
                .any(|item| item.kind == CheatConflictKind::OverlappingRange)
        );
    }

    #[test]
    fn unknown_and_pointer_operations_are_warnings_not_certainty() {
        let pointer = CheatCompatibilityOperation::PointerWrite(CheatMemoryRange {
            address: 1,
            width_bytes: 4,
            value: Some(1),
            condition: CheatCondition::Always,
            continuous: true,
            pointer_based: true,
        });
        let unknown = CheatCompatibilityOperation::Unknown {
            raw: "raw".into(),
            reason: "test".into(),
        };
        let report = analyze_cheat_stack(&[entry("pointer", pointer), entry("unknown", unknown)]);
        assert_eq!(report.readiness, CheatStackReadiness::Unsupported);
        assert!(
            report
                .conflicts
                .iter()
                .any(|item| item.kind == CheatConflictKind::UnsupportedOpcodeInteraction)
        );
    }

    #[test]
    fn stronger_revision_evidence_blocks_mismatch_and_title_only_is_ambiguous() {
        let mut exact = entry("exact", write(1, 1, 1));
        exact.revision = CheatRevisionEvidence::ExactHash { hash: "one".into() };
        let mut provider = entry("provider", write(2, 1, 1));
        provider.revision = CheatRevisionEvidence::ProviderDeclared {
            revision: "two".into(),
        };
        assert_eq!(
            analyze_cheat_stack(&[exact, provider]).readiness,
            CheatStackReadiness::WrongRevision
        );
        let mut left = entry("left", write(1, 1, 1));
        left.revision = CheatRevisionEvidence::TitleOnly {
            title: "Game".into(),
        };
        let mut right = entry("right", write(2, 1, 1));
        right.revision = CheatRevisionEvidence::TitleOnly {
            title: "Other revision".into(),
        };
        assert_eq!(
            analyze_cheat_stack(&[left, right]).readiness,
            CheatStackReadiness::Ambiguous
        );
    }
}
