//! Read-only, typed conversion plans around the neutral cheat IR.
//!
//! The existing IR assessor decides whether individual operations have a
//! proven target encoding.  This module adds the authority boundary needed by
//! callers: verified game identity, source/profile binding, destination
//! collision checks, and stale-plan rejection.  It never writes files or
//! enables cheats.

use super::cheat_ir::{
    CheatConversionPreview, CheatDocument, CheatOperation, CheatSourceFormat, CheatTargetFormat,
    DsActionReplayClassification, assess_document_conversion, dolphin_line_to_ir,
    dolphin_on_frame_line_to_ir, ds_action_replay_line_to_ir, pnach_line_to_ir,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatConversionMismatchReason {
    EntryCountChanged,
    AddressChanged,
    ValueChanged,
    WidthChanged,
    OperationTypeChanged,
    GroupingChanged,
    ConditionalLost,
    IdentityMetadataLost,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatConversionVerification {
    NotRun,
    Verified {
        emitted_digest: String,
        parsed_entry_count: usize,
        expected_entry_count: usize,
        summary: String,
    },
    ParseFailed {
        detail: String,
    },
    SemanticMismatch {
        reasons: Vec<CheatConversionMismatchReason>,
    },
    UnsupportedForRoundTrip {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheatConversionRequest {
    pub source_digest: String,
    pub game_identity: String,
    pub identity_verified: bool,
    pub emulator: String,
    pub profile: String,
    pub target: CheatTargetFormat,
    pub destination: String,
    pub destination_exists: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheatConversionPlan {
    pub request: CheatConversionRequest,
    pub preview: CheatConversionPreview,
    pub blockers: Vec<String>,
    pub verification: CheatConversionVerification,
}

impl CheatConversionPlan {
    #[must_use]
    pub fn build(document: &CheatDocument, request: CheatConversionRequest) -> Self {
        let preview = assess_document_conversion(document, request.target.clone());
        let mut blockers = Vec::new();
        if !request.identity_verified || request.game_identity.trim().is_empty() {
            blockers.push("verified game identity is required".into());
        }
        if request.destination.trim().is_empty() {
            blockers.push("a destination is required".into());
        }
        if request.destination_exists {
            blockers.push("destination already exists; overwrite is not implicit".into());
        }
        if !preview.can_apply {
            blockers.push("the requested conversion is not losslessly representable".into());
        }
        let verification = verify_conversion(document, &preview);
        if !matches!(&verification, CheatConversionVerification::Verified { .. }) {
            blockers.push(
                "conversion output did not pass target-parser round-trip verification".into(),
            );
        }
        Self {
            request,
            preview,
            blockers,
            verification,
        }
    }

    #[must_use]
    pub fn is_fresh(&self, request: &CheatConversionRequest) -> bool {
        &self.request == request
    }

    /// A verified result is reusable only while every authority input in the
    /// original request is unchanged.
    #[must_use]
    pub fn verification_is_fresh(&self, request: &CheatConversionRequest) -> bool {
        self.is_fresh(request)
            && matches!(
                &self.verification,
                CheatConversionVerification::Verified { .. }
            )
    }

    /// Returns deterministic bytes for a fully eligible plan.  This is an
    /// emit-to-memory operation only; installation remains a later, explicit
    /// workflow owned by the target emulator adapter.
    pub fn emit(&self) -> Result<String, String> {
        if !self.blockers.is_empty() {
            return Err(self.blockers.join("; "));
        }
        self.preview
            .output_preview
            .clone()
            .ok_or_else(|| "conversion produced no verified output".into())
    }
}

fn target_parser_format(target: &CheatTargetFormat) -> Option<CheatSourceFormat> {
    match target {
        CheatTargetFormat::DolphinActionReplay => Some(CheatSourceFormat::DolphinActionReplay),
        CheatTargetFormat::Gecko => Some(CheatSourceFormat::Gecko),
        CheatTargetFormat::Pnach => Some(CheatSourceFormat::Pnach),
        CheatTargetFormat::ActionReplayDs => Some(CheatSourceFormat::ActionReplayDs),
        CheatTargetFormat::DolphinOnFrame => Some(CheatSourceFormat::DolphinOnFrame),
        CheatTargetFormat::RetroArch
        | CheatTargetFormat::GameSharkPs2
        | CheatTargetFormat::CodeBreakerPs2 => None,
    }
}

fn digest_text(text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(text.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Reparse and semantically verify an already-emitted conversion preview.
/// This is intentionally read-only and in-memory; callers must gate any
/// future destination write on [`CheatConversionVerification::Verified`].
pub fn verify_conversion(
    document: &CheatDocument,
    preview: &CheatConversionPreview,
) -> CheatConversionVerification {
    if !preview.can_apply {
        return CheatConversionVerification::NotRun;
    }
    let Some(parser_format) = target_parser_format(&preview.target_format) else {
        return CheatConversionVerification::UnsupportedForRoundTrip {
            reason: "target parser is not available in the shared cheat IR".into(),
        };
    };
    let Some(output) = preview.output_preview.as_deref() else {
        return CheatConversionVerification::ParseFailed {
            detail: "conversion emitted no text".into(),
        };
    };
    let mut parsed = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let operation = match parser_format {
            CheatSourceFormat::DolphinActionReplay | CheatSourceFormat::Gecko => {
                dolphin_line_to_ir(line, parser_format.clone())
            }
            CheatSourceFormat::Pnach => pnach_line_to_ir(line),
            CheatSourceFormat::DolphinOnFrame => dolphin_on_frame_line_to_ir(line),
            CheatSourceFormat::ActionReplayDs => match ds_action_replay_line_to_ir(line) {
                DsActionReplayClassification::DirectWrite(operation) => operation,
                DsActionReplayClassification::Unsupported(unsupported) => {
                    return CheatConversionVerification::ParseFailed {
                        detail: unsupported.reason,
                    };
                }
            },
            _ => unreachable!(),
        };
        if matches!(operation, CheatOperation::UnsupportedRaw { .. }) {
            return CheatConversionVerification::ParseFailed {
                detail: format!("target parser rejected emitted line: {line}"),
            };
        }
        parsed.push(operation);
    }
    let reasons = semantic_mismatch_reasons(&document.operations, &parsed);
    if reasons.is_empty() {
        CheatConversionVerification::Verified {
            emitted_digest: digest_text(output),
            parsed_entry_count: parsed.len(),
            expected_entry_count: document.operations.len(),
            summary: "reparsed target semantics match the source conversion plan".into(),
        }
    } else {
        CheatConversionVerification::SemanticMismatch { reasons }
    }
}

fn semantic_mismatch_reasons(
    expected: &[CheatOperation],
    observed: &[CheatOperation],
) -> Vec<CheatConversionMismatchReason> {
    let mut reasons = Vec::new();
    if expected.len() != observed.len() {
        reasons.push(CheatConversionMismatchReason::EntryCountChanged);
    }
    for (left, right) in expected.iter().zip(observed) {
        if std::mem::discriminant(left) != std::mem::discriminant(right) {
            let direct = |operation: &CheatOperation| {
                matches!(
                    operation,
                    CheatOperation::Write8 { .. }
                        | CheatOperation::Write16 { .. }
                        | CheatOperation::Write32 { .. }
                )
            };
            if direct(left) && direct(right) {
                reasons.push(CheatConversionMismatchReason::WidthChanged);
            } else {
                reasons.push(CheatConversionMismatchReason::OperationTypeChanged);
            }
            continue;
        }
        match (left, right) {
            (
                CheatOperation::Write8 {
                    address: la,
                    value: lv,
                },
                CheatOperation::Write8 {
                    address: ra,
                    value: rv,
                },
            ) => {
                if la != ra {
                    reasons.push(CheatConversionMismatchReason::AddressChanged);
                }
                if lv != rv {
                    reasons.push(CheatConversionMismatchReason::ValueChanged);
                }
            }
            (
                CheatOperation::Write16 {
                    address: la,
                    value: lv,
                },
                CheatOperation::Write16 {
                    address: ra,
                    value: rv,
                },
            ) => {
                if la != ra {
                    reasons.push(CheatConversionMismatchReason::AddressChanged);
                }
                if lv != rv {
                    reasons.push(CheatConversionMismatchReason::ValueChanged);
                }
            }
            (
                CheatOperation::Write32 {
                    address: la,
                    value: lv,
                },
                CheatOperation::Write32 {
                    address: ra,
                    value: rv,
                },
            ) => {
                if la != ra {
                    reasons.push(CheatConversionMismatchReason::AddressChanged);
                }
                if lv != rv {
                    reasons.push(CheatConversionMismatchReason::ValueChanged);
                }
            }
            (
                CheatOperation::OnFrameWrite8 {
                    address: la,
                    value: lv,
                },
                CheatOperation::OnFrameWrite8 {
                    address: ra,
                    value: rv,
                },
            ) => {
                if la != ra {
                    reasons.push(CheatConversionMismatchReason::AddressChanged);
                }
                if lv != rv {
                    reasons.push(CheatConversionMismatchReason::ValueChanged);
                }
            }
            (
                CheatOperation::OnFrameWrite16 {
                    address: la,
                    value: lv,
                },
                CheatOperation::OnFrameWrite16 {
                    address: ra,
                    value: rv,
                },
            ) => {
                if la != ra {
                    reasons.push(CheatConversionMismatchReason::AddressChanged);
                }
                if lv != rv {
                    reasons.push(CheatConversionMismatchReason::ValueChanged);
                }
            }
            (
                CheatOperation::OnFrameWrite32 {
                    address: la,
                    value: lv,
                },
                CheatOperation::OnFrameWrite32 {
                    address: ra,
                    value: rv,
                },
            ) => {
                if la != ra {
                    reasons.push(CheatConversionMismatchReason::AddressChanged);
                }
                if lv != rv {
                    reasons.push(CheatConversionMismatchReason::ValueChanged);
                }
            }
            _ => reasons.push(CheatConversionMismatchReason::OperationTypeChanged),
        }
    }
    reasons.sort_by_key(|reason| format!("{reason:?}"));
    reasons.dedup();
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::cheat_ir::{CheatOperation, CheatPlatform, CheatSourceFormat};

    fn request(destination_exists: bool) -> CheatConversionRequest {
        CheatConversionRequest {
            source_digest: "source-1".into(),
            game_identity: "game-1".into(),
            identity_verified: true,
            emulator: "Dolphin".into(),
            profile: "default".into(),
            target: CheatTargetFormat::Gecko,
            destination: "out.ini".into(),
            destination_exists,
        }
    }

    fn document() -> CheatDocument {
        CheatDocument {
            title: "Example".into(),
            platform: CheatPlatform::GameCube,
            source_format: CheatSourceFormat::DolphinActionReplay,
            operations: vec![CheatOperation::Write8 {
                address: 1,
                value: 2,
            }],
            issues: vec![],
            provenance: vec!["fixture".into()],
        }
    }

    #[test]
    fn verified_lossless_plan_emits_without_writing() {
        let plan = CheatConversionPlan::build(&document(), request(false));
        assert!(plan.blockers.is_empty());
        assert_eq!(plan.emit().unwrap(), "00000001 00000002");
    }

    #[test]
    fn identity_and_destination_safety_block_emit() {
        let mut req = request(true);
        req.identity_verified = false;
        let plan = CheatConversionPlan::build(&document(), req);
        assert!(plan.emit().is_err());
    }

    #[test]
    fn changing_source_or_profile_makes_plan_stale() {
        let req = request(false);
        let plan = CheatConversionPlan::build(&document(), req.clone());
        assert!(plan.is_fresh(&req));
        let mut changed = req;
        changed.profile = "other".into();
        assert!(!plan.is_fresh(&changed));
    }

    #[test]
    fn dolphin_ar_and_gecko_round_trip_direct_writes() {
        for target in [
            CheatTargetFormat::DolphinActionReplay,
            CheatTargetFormat::Gecko,
        ] {
            let mut req = request(false);
            req.target = target;
            let plan = CheatConversionPlan::build(&document(), req);
            assert!(matches!(
                plan.verification,
                CheatConversionVerification::Verified { .. }
            ));
            assert!(plan.blockers.is_empty());
        }
    }

    #[test]
    fn pnach_round_trip_preserves_supported_widths() {
        let doc = CheatDocument {
            title: "PS2".into(),
            platform: CheatPlatform::Ps2,
            source_format: CheatSourceFormat::Pnach,
            operations: vec![
                CheatOperation::Write8 {
                    address: 0x2012_3456,
                    value: 0xab,
                },
                CheatOperation::Write16 {
                    address: 0x2012_3458,
                    value: 0xcdef,
                },
                CheatOperation::Write32 {
                    address: 0x2012_345c,
                    value: 0xdead_beef,
                },
            ],
            issues: vec![],
            provenance: vec!["fixture".into()],
        };
        let mut req = request(false);
        req.target = CheatTargetFormat::Pnach;
        let plan = CheatConversionPlan::build(&doc, req);
        assert!(matches!(
            plan.verification,
            CheatConversionVerification::Verified { .. }
        ));
        assert!(plan.blockers.is_empty());
    }

    #[test]
    fn on_frame_round_trip_preserves_execution_policy() {
        let mut req = request(false);
        req.target = CheatTargetFormat::DolphinOnFrame;
        let doc = CheatDocument {
            operations: vec![CheatOperation::OnFrameWrite8 {
                address: 1,
                value: 2,
            }],
            ..document()
        };
        let plan = CheatConversionPlan::build(&doc, req);
        assert!(matches!(
            plan.verification,
            CheatConversionVerification::Verified { .. }
        ));
        assert!(plan.blockers.is_empty());
    }

    #[test]
    fn verification_digest_is_deterministic_and_stale_requests_are_rejected() {
        let req = request(false);
        let first = CheatConversionPlan::build(&document(), req.clone());
        let second = CheatConversionPlan::build(&document(), req.clone());
        assert_eq!(first.verification, second.verification);
        assert!(first.verification_is_fresh(&req));
        let mut changed = req;
        changed.destination = "other.ini".into();
        assert!(!first.verification_is_fresh(&changed));
    }

    #[test]
    fn malformed_or_unsupported_operations_are_diagnostic_only() {
        let mut doc = document();
        doc.operations = vec![CheatOperation::UnsupportedRaw {
            source_format: CheatSourceFormat::DolphinActionReplay,
            raw: "not a code".into(),
            reason: "malformed direct-write line".into(),
        }];
        let plan = CheatConversionPlan::build(&doc, request(false));
        assert!(!plan.blockers.is_empty());
        assert!(plan.emit().is_err());
        assert!(plan.preview.output_preview.is_none());
    }

    #[test]
    fn missing_or_incompatible_platform_context_fails_closed() {
        let mut missing = document();
        missing.platform = CheatPlatform::Other("unknown".into());
        let missing_plan = CheatConversionPlan::build(&missing, request(false));
        assert!(missing_plan.emit().is_err());

        let mut incompatible_request = request(false);
        incompatible_request.target = CheatTargetFormat::Pnach;
        let incompatible_plan = CheatConversionPlan::build(&document(), incompatible_request);
        assert!(incompatible_plan.emit().is_err());
    }

    #[test]
    fn empty_usable_entry_set_cannot_emit() {
        let mut empty = document();
        empty.operations.clear();
        let plan = CheatConversionPlan::build(&empty, request(false));
        assert!(plan.emit().is_err());
        assert!(plan.preview.output_preview.is_none());
    }

    #[test]
    fn conversion_plan_is_memory_only() {
        let plan = CheatConversionPlan::build(&document(), request(false));
        let emitted = plan.emit().unwrap();
        assert_eq!(emitted, "00000001 00000002");
    }
}
