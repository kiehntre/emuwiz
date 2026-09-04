//! Read-only native Cemu (Wii U) command planning.
//!
//! Only the extracted `code`/`content`/`meta` layout is ever turned into a
//! command here - see [`crate::patch_manager::cemu_local`]'s module doc
//! comment for why `.wud`/`.wux`/`.wua` are recognised but refused. This
//! module never mounts, extracts, decrypts, or otherwise touches content;
//! it only reads facts a caller already gathered (via
//! [`crate::patch_manager::cemu_local`]) and turns them into an argv, or
//! explains why it cannot.
//!
//! Self-contained, like [`crate::launch::fbneo_command`]: this adapter is
//! not wired into [`crate::launch::integration::DiscoveredStandaloneProfile`]
//! or the shared [`crate::launch::planning`] candidate matrix (that
//! integration seam already skips FBNeo and MAME for the same reason - see
//! this crate's own report for why wiring Cemu into it is left as future,
//! GUI-adjacent work rather than done here).

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::readiness::{
    LaunchBlocker, LaunchBlockerKind, LaunchWarning, LaunchWarningKind,
};
use crate::patch_manager::{
    CemuContentForm, CemuContentSupport, CemuExtractedLayout, CemuKeysEvidence, CemuKeysState,
    CemuLayoutError, CemuMlcEvidence, CemuMlcState, CemuTitleIdentity, CemuTitleKind,
    classify_title_kind,
};

pub const CEMU_SUPPORTED_PLATFORM_ID: &str = "WiiU";

/// The overall verdict this adapter reports for one request, independent of
/// the shared [`crate::launch::readiness::LaunchReadiness`] (which has no
/// "needs setup" state) - see the module's own report for why a fourth
/// state is worth keeping distinct from `Blocked` here: a missing MLC path
/// or missing `keys.txt` is something a person can go fix, where a content
/// drift or an unsupported format is not fixable from this screen at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CemuReadiness {
    Ready,
    ReadyWithWarnings,
    NeedsSetup,
    Blocked,
}

/// Blocker kinds that describe something a person can fix by configuring
/// Cemu itself (an MLC path, `keys.txt`) rather than a hard failure.
fn is_setup_blocker(kind: LaunchBlockerKind) -> bool {
    matches!(
        kind,
        LaunchBlockerKind::CemuMlcUnavailable
            | LaunchBlockerKind::CemuKeysRequired
            | LaunchBlockerKind::CemuBindingUnavailable
    )
}

/// Classifies a plan's own blockers/warnings into the four-state verdict
/// above. Pure - never re-inspects anything, never trusts a caller-supplied
/// readiness instead of this plan's own freshly computed blockers.
pub fn classify_cemu_readiness(plan: &CemuCommandPlan) -> CemuReadiness {
    if plan.blockers.is_empty() {
        if plan.warnings.is_empty() {
            CemuReadiness::Ready
        } else {
            CemuReadiness::ReadyWithWarnings
        }
    } else if plan
        .blockers
        .iter()
        .all(|blocker| is_setup_blocker(blocker.kind))
    {
        CemuReadiness::NeedsSetup
    } else {
        CemuReadiness::Blocked
    }
}

/// The narrow, immutable set of facts identifying exactly which Cemu launch
/// is being requested and everything already gathered about it. Every field
/// here is either a user-approved selection (`executable`, `profile_id`,
/// `selected_content`) or evidence a caller already collected via
/// [`crate::patch_manager::cemu_local`] - never a shell string, and never
/// re-derived from a filename or folder name by this module.
///
/// `readiness` is carried through for display purposes only: a candidate a
/// caller believed was ready. [`build_cemu_command_plan`] never trusts it -
/// it always recomputes its own blockers/warnings from the other fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuLaunchRequest {
    pub executable: PathBuf,
    pub profile_id: String,
    pub platform_id: String,
    pub selected_content: PathBuf,
    pub content_form: CemuContentForm,
    pub title_identity: Option<CemuTitleIdentity>,
    pub keys_evidence: CemuKeysEvidence,
    pub mlc_evidence: CemuMlcEvidence,
    pub readiness: CemuReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: CemuCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CemuCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub content_path: PathBuf,
    pub rpx_path: PathBuf,
    pub title_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CemuCommandPlan {
    pub command: Option<CemuCommand>,
    pub blockers: Vec<LaunchBlocker>,
    pub warnings: Vec<LaunchWarning>,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}
fn warning(kind: LaunchWarningKind, detail: impl Into<String>) -> LaunchWarning {
    LaunchWarning {
        kind,
        detail: detail.into(),
    }
}

/// Builds the exact, minimal native Cemu argv (`Cemu -g <rpx path>` -
/// nothing else: no graphics-pack, renderer, CPU-mode, fullscreen,
/// controller, shader, FPS, or hack flag is ever added here) - or explains
/// with structured blockers why it cannot.
///
/// `layout` must be the result of freshly calling
/// [`crate::patch_manager::cemu_local::inspect_extracted_layout`] on
/// `request.selected_content` - this function never walks a filesystem
/// itself.
pub fn build_cemu_command_plan(
    request: &CemuLaunchRequest,
    layout: &Result<CemuExtractedLayout, CemuLayoutError>,
) -> CemuCommandPlan {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    if request.platform_id != CEMU_SUPPORTED_PLATFORM_ID {
        blockers.push(block(
            LaunchBlockerKind::CemuContentFormatUnsupported,
            format!(
                "platform `{}` is not `{CEMU_SUPPORTED_PLATFORM_ID}`",
                request.platform_id
            ),
        ));
    }

    if !request.content_form.launchable_in_this_build() {
        blockers.push(block(
            LaunchBlockerKind::CemuContentFormatUnsupported,
            format!(
                "{:?} is not launchable in this build - only ExtractedTitle is",
                request.content_form
            ),
        ));
    }

    match request.mlc_evidence.state {
        CemuMlcState::Present => {}
        state => blockers.push(block(
            LaunchBlockerKind::CemuMlcUnavailable,
            format!("Cemu's configured MLC path is {state:?}, not Present"),
        )),
    }

    let keys_required = request.content_form.support() == CemuContentSupport::RequiresKeys;
    match request.keys_evidence.state {
        CemuKeysState::PresentUnverified => {
            warnings.push(warning(
                LaunchWarningKind::CemuKeysPresentUnverified,
                "keys.txt is present but its contents were never inspected",
            ));
        }
        CemuKeysState::NotConfigured | CemuKeysState::Unreadable if keys_required => {
            blockers.push(block(
                LaunchBlockerKind::CemuKeysRequired,
                "this content form requires keys.txt and none was found configured",
            ));
        }
        CemuKeysState::NotConfigured | CemuKeysState::Unreadable => {}
    }

    let layout = match layout {
        Ok(layout) => Some(layout),
        Err(error) => {
            blockers.push(block(
                LaunchBlockerKind::CemuLayoutInvalid,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    let title_id = request
        .title_identity
        .as_ref()
        .and_then(|identity| identity.title_id.clone());
    match &title_id {
        Some(id) if classify_title_kind(id) != CemuTitleKind::Base => {
            blockers.push(block(
                LaunchBlockerKind::CemuNotABaseTitle,
                format!(
                    "selected content's title ID classifies as {:?}, not Base",
                    classify_title_kind(id)
                ),
            ));
        }
        Some(_) => {}
        None => warnings.push(warning(
            LaunchWarningKind::CemuTitleIdentityUnavailable,
            "no meta.xml-derived title identity is available for this content",
        )),
    }

    if request.executable.as_os_str().is_empty() {
        blockers.push(block(
            LaunchBlockerKind::CemuBindingUnavailable,
            "no executable was provided",
        ));
    }

    if !blockers.is_empty() {
        return CemuCommandPlan {
            command: None,
            blockers,
            warnings,
        };
    }

    let layout = layout.expect("checked above");
    CemuCommandPlan {
        command: Some(CemuCommand {
            executable: request.executable.clone(),
            arguments: vec![
                OsString::from("-g"),
                layout.rpx_path.clone().into_os_string(),
            ],
            working_directory: None,
            selection: CemuCommandSelection {
                profile_id: request.profile_id.clone(),
                platform_id: request.platform_id.clone(),
                content_path: request.selected_content.clone(),
                rpx_path: layout.rpx_path.clone(),
                title_id,
            },
        }),
        blockers: Vec::new(),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::{CemuKeysEvidence, CemuKeysState, CemuMlcEvidence, CemuMlcState};

    fn base_request(content_form: CemuContentForm) -> CemuLaunchRequest {
        CemuLaunchRequest {
            executable: PathBuf::from("/opt/Cemu"),
            profile_id: "cemu:/x".into(),
            platform_id: CEMU_SUPPORTED_PLATFORM_ID.into(),
            selected_content: PathBuf::from("/games/Some Game"),
            content_form,
            title_identity: Some(CemuTitleIdentity {
                title_id: Some("00050000101010ED".into()),
                product_code: Some("WUP-P-ARAE".into()),
                company_code: Some("01".into()),
                title_version: Some("16".into()),
            }),
            keys_evidence: CemuKeysEvidence {
                path: None,
                state: CemuKeysState::NotConfigured,
            },
            mlc_evidence: CemuMlcEvidence {
                path: Some(PathBuf::from("/mlc")),
                state: CemuMlcState::Present,
            },
            readiness: CemuReadiness::Ready,
        }
    }

    fn ok_layout() -> Result<CemuExtractedLayout, CemuLayoutError> {
        Ok(CemuExtractedLayout {
            root: PathBuf::from("/games/Some Game"),
            code_dir: PathBuf::from("/games/Some Game/code"),
            content_dir: PathBuf::from("/games/Some Game/content"),
            meta_dir: PathBuf::from("/games/Some Game/meta"),
            rpx_path: PathBuf::from("/games/Some Game/code/game with spaces.rpx"),
            meta_xml_path: Some(PathBuf::from("/games/Some Game/meta/meta.xml")),
        })
    }

    #[test]
    fn extracted_title_produces_minimal_deterministic_argv_with_spaces() {
        let request = base_request(CemuContentForm::ExtractedTitle);
        let plan = build_cemu_command_plan(&request, &ok_layout());
        assert_eq!(classify_cemu_readiness(&plan), CemuReadiness::Ready);
        let command = plan.command.expect("command");
        assert_eq!(command.executable, PathBuf::from("/opt/Cemu"));
        assert_eq!(
            command.arguments,
            vec![
                OsString::from("-g"),
                OsString::from("/games/Some Game/code/game with spaces.rpx"),
            ]
        );
    }

    #[test]
    fn wud_and_wux_and_wua_are_unsupported_in_this_build() {
        for form in [
            CemuContentForm::Wud,
            CemuContentForm::Wux,
            CemuContentForm::Wua,
        ] {
            let request = base_request(form);
            let plan = build_cemu_command_plan(&request, &ok_layout());
            assert!(plan.command.is_none());
            assert!(
                plan.blockers
                    .iter()
                    .any(|b| b.kind == LaunchBlockerKind::CemuContentFormatUnsupported)
            );
        }
    }

    #[test]
    fn missing_mlc_blocks_and_is_a_setup_blocker() {
        let mut request = base_request(CemuContentForm::ExtractedTitle);
        request.mlc_evidence = CemuMlcEvidence {
            path: None,
            state: CemuMlcState::NotConfigured,
        };
        let plan = build_cemu_command_plan(&request, &ok_layout());
        assert!(plan.command.is_none());
        assert_eq!(classify_cemu_readiness(&plan), CemuReadiness::NeedsSetup);
    }

    #[test]
    fn keys_required_case_blocks_when_missing() {
        let mut request = base_request(CemuContentForm::Wud);
        request.keys_evidence = CemuKeysEvidence {
            path: None,
            state: CemuKeysState::NotConfigured,
        };
        let plan = build_cemu_command_plan(&request, &ok_layout());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::CemuKeysRequired)
        );
    }

    #[test]
    fn keys_not_required_for_extracted_title_is_never_blocked_by_keys() {
        let request = base_request(CemuContentForm::ExtractedTitle);
        let plan = build_cemu_command_plan(&request, &ok_layout());
        assert!(
            !plan
                .blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::CemuKeysRequired)
        );
    }

    #[test]
    fn present_unverified_keys_is_a_warning_never_a_secret_in_the_warning_text() {
        let mut request = base_request(CemuContentForm::ExtractedTitle);
        request.keys_evidence = CemuKeysEvidence {
            path: Some(PathBuf::from("/x/keys.txt")),
            state: CemuKeysState::PresentUnverified,
        };
        let plan = build_cemu_command_plan(&request, &ok_layout());
        assert!(plan.command.is_some());
        assert_eq!(
            classify_cemu_readiness(&plan),
            CemuReadiness::ReadyWithWarnings
        );
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.kind == LaunchWarningKind::CemuKeysPresentUnverified
                    && !w.detail.contains("txt\n")
                    && !w.detail.to_ascii_lowercase().contains("key="))
        );
    }

    #[test]
    fn missing_title_identity_is_a_warning_not_a_blocker() {
        let mut request = base_request(CemuContentForm::ExtractedTitle);
        request.title_identity = None;
        let plan = build_cemu_command_plan(&request, &ok_layout());
        assert!(plan.command.is_some());
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.kind == LaunchWarningKind::CemuTitleIdentityUnavailable)
        );
    }

    #[test]
    fn update_or_dlc_title_id_is_blocked_not_launched_as_a_base_game() {
        let mut request = base_request(CemuContentForm::ExtractedTitle);
        request.title_identity = Some(CemuTitleIdentity {
            title_id: Some("0005000C101010ED".into()),
            ..Default::default()
        });
        let plan = build_cemu_command_plan(&request, &ok_layout());
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::CemuNotABaseTitle)
        );
    }

    #[test]
    fn invalid_layout_blocks_with_no_command() {
        let request = base_request(CemuContentForm::ExtractedTitle);
        let layout = Err(CemuLayoutError {
            kind: crate::patch_manager::CemuLayoutErrorKind::MissingCodeDirectory,
            detail: "no code/ directory".into(),
        });
        let plan = build_cemu_command_plan(&request, &layout);
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::CemuLayoutInvalid)
        );
    }

    #[test]
    fn argv_never_goes_through_a_shell_string() {
        // Structural proof: `arguments` is `Vec<OsString>`, passed to
        // `std::process::Command::args` (see cemu_execution), never joined
        // into one string. This test exists to name that invariant.
        let request = base_request(CemuContentForm::ExtractedTitle);
        let plan = build_cemu_command_plan(&request, &ok_layout());
        let command = plan.command.unwrap();
        assert_eq!(command.arguments.len(), 2);
    }
}
