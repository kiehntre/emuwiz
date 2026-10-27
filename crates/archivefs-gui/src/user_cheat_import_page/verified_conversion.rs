//! Only expose conversion text after the existing target-parser verifier passes.
use archivefs_core::patch_manager::{
    CheatConversionPreview, CheatDocument, CheatTargetFormat, convert_cheat_document,
};
use archivefs_core::patch_manager::{CheatConversionVerification, verify_conversion};
pub(super) fn verified_preview(
    document: &CheatDocument,
    target: CheatTargetFormat,
) -> CheatConversionPreview {
    let mut preview = convert_cheat_document(document, target);
    let verification = verify_conversion(document, &preview);
    if !matches!(verification, CheatConversionVerification::Verified { .. }) {
        preview.can_apply = false;
        preview.output_preview = None;
        preview.warnings.push(
            archivefs_core::patch_manager::CheatIssue::UnsupportedOperation(format!(
                "Target-parser round-trip verification refused: {verification:?}"
            )),
        );
    }
    preview
}
#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::patch_manager::parse_ds_action_replay_document;
    #[test]
    fn verified_round_trip_exposes_only_complete_output() {
        let d = parse_ds_action_replay_document("Example", "02345678 DEADBEEF", vec![]);
        let p = verified_preview(&d, CheatTargetFormat::ActionReplayDs);
        assert!(p.can_apply);
        assert!(p.output_preview.is_some());
        let malformed = parse_ds_action_replay_document(
            "Example",
            "02345678 DEADBEEF\nD2000000 00000000",
            vec![],
        );
        let p = verified_preview(&malformed, CheatTargetFormat::ActionReplayDs);
        assert!(!p.can_apply);
        assert!(p.output_preview.is_none());
    }
    #[test]
    fn absent_target_parser_never_exposes_copy_text() {
        let d = parse_ds_action_replay_document("Example", "02345678 DEADBEEF", vec![]);
        let p = verified_preview(&d, CheatTargetFormat::RetroArch);
        assert!(!p.can_apply);
        assert!(p.output_preview.is_none());
    }
    #[test]
    fn input_change_recomputes_output_instead_of_reusing_verification() {
        let first = parse_ds_action_replay_document("Example", "02345678 DEADBEEF", vec![]);
        let second = parse_ds_action_replay_document("Example", "02345678 12345678", vec![]);
        let a = verified_preview(&first, CheatTargetFormat::ActionReplayDs);
        let b = verified_preview(&second, CheatTargetFormat::ActionReplayDs);
        assert_ne!(a.output_preview, b.output_preview);
    }
    #[test]
    fn native_pnach_and_dolphin_documents_pass_the_copy_gate() {
        use archivefs_core::patch_manager::CheatPlatform;
        for (platform, source, text, target) in [
            (
                CheatPlatform::GameCube,
                0,
                "042318AC 3B8003E7",
                CheatTargetFormat::DolphinActionReplay,
            ),
            (
                CheatPlatform::Ps2,
                3,
                "patch=1,EE,20123456,word,12345678",
                CheatTargetFormat::Pnach,
            ),
        ] {
            let document =
                super::super::build_conversion_document("Example", platform, source, text).unwrap();
            let preview = verified_preview(&document, target);
            assert!(preview.can_apply, "{:?}", preview.warnings);
            assert!(preview.output_preview.is_some());
        }
    }
}
