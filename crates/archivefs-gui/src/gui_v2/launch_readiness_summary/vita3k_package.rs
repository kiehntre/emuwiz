//! The Vita3K library-package answer, on Blue's main readiness card.
//!
//! Display-only: it reads the already finished package check (`Context`) and
//! never launches, scans or plans. The Play button itself still comes from
//! the shared launch input; this only makes sure every state that withholds
//! Play (or changes what Play does) is stated on the card itself.

use super::{GameReadinessSummary, ReadinessAction, ReadinessPresentationState as State};
use crate::launch_readiness_page::vita3k::Context;
use archivefs_core::launch::Vita3kLibraryResolution as Resolution;

pub(crate) fn note_vita3k_package(summary: &mut GameReadinessSummary, context: &Context) {
    let withhold = |summary: &mut GameReadinessSummary,
                    status: State,
                    headline: &str,
                    action: Option<ReadinessAction>| {
        summary.status = status;
        summary.headline = headline.into();
        summary.explanation = context.status_text();
        summary.primary_action = action;
        summary.findings.clear();
    };
    if context.extracted_folder {
        if summary.primary_action != Some(ReadinessAction::Play) {
            withhold(
                summary,
                State::Blocked,
                "Extracted Vita folders can't be played from the library",
                None,
            );
        }
        return;
    }
    match &context.result {
        Err(_) => withhold(
            summary,
            State::NeedsIdentityReview,
            "This package's Vita title can't be confirmed",
            Some(ReadinessAction::RecheckReadiness),
        ),
        Ok((_, Resolution::NotInstalled)) => withhold(
            summary,
            State::Blocked,
            "Not installed in Vita3K",
            Some(ReadinessAction::RecheckReadiness),
        ),
        Ok((_, Resolution::Ambiguous)) => withhold(
            summary,
            State::Blocked,
            "Installed in more than one Vita3K installation",
            Some(ReadinessAction::RecheckReadiness),
        ),
        Ok((_, Resolution::Blocked(_))) => withhold(
            summary,
            State::Blocked,
            "Installed in Vita3K, but it can't start",
            Some(ReadinessAction::RecheckReadiness),
        ),
        Ok((_, Resolution::NoInstallation(_))) => withhold(
            summary,
            State::NeedsEmulator,
            "Vita3K isn't set up",
            Some(ReadinessAction::SetUpEmulator),
        ),
        Ok((_, Resolution::Installed { param_differs, .. })) => {
            // Other blockers (firmware, profile) keep their own answer.
            if summary.primary_action != Some(ReadinessAction::Play) {
                return;
            }
            summary.explanation = format!(
                "Play starts the title already installed in Vita3K, not this .vpk package. {}",
                if *param_differs {
                    "The installed copy's metadata differs from this package (for example an update)."
                } else {
                    "Its title ID matches this package."
                }
            );
            if *param_differs {
                if summary.status == State::Ready {
                    summary.status = State::ReadyWithWarnings;
                }
                summary
                    .warnings
                    .push("The installed Vita3K version may differ from this package.".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_v2::launch_readiness_summary::not_started_summary;
    use archivefs_core::launch::process_spawn::CapturedFileIdentity;
    use archivefs_core::launch::{VpkEvidence, VpkRefusal, VpkRefusalKind};
    use std::path::PathBuf;

    fn evidence() -> VpkEvidence {
        VpkEvidence {
            path: PathBuf::from("/g/Game.vpk"),
            file_identity: CapturedFileIdentity {
                device: 1,
                inode: 2,
                size: 3,
                modified: None,
            },
            title_id: "PCSE00001".into(),
            param_sha256: "00".repeat(32),
            display_title: None,
        }
    }

    fn context(resolution: Resolution) -> Context {
        Context {
            selected: PathBuf::from("/g/Game.vpk"),
            result: Ok((evidence(), resolution)),
            extracted_folder: false,
        }
    }

    fn card(context: &Context) -> GameReadinessSummary {
        let mut summary = not_started_summary();
        note_vita3k_package(&mut summary, context);
        summary
    }

    #[test]
    fn every_withheld_state_is_named_on_the_card_with_no_play() {
        let cases = [
            (
                Resolution::NotInstalled,
                "Not installed in Vita3K",
                "not installed",
            ),
            (
                Resolution::Ambiguous,
                "Installed in more than one Vita3K installation",
                "more than one",
            ),
            (
                Resolution::Blocked("licence missing".into()),
                "Installed in Vita3K, but it can't start",
                "licence missing",
            ),
            (
                Resolution::NoInstallation("no Vita3K".into()),
                "Vita3K isn't set up",
                "no Vita3K",
            ),
        ];
        for (resolution, headline, text) in cases {
            let summary = card(&context(resolution));
            assert_eq!(summary.headline, headline);
            assert!(
                summary.explanation.contains(text),
                "{}",
                summary.explanation
            );
            assert_ne!(summary.primary_action, Some(ReadinessAction::Play));
        }
    }

    #[test]
    fn an_unconfirmable_package_and_an_extracted_folder_get_no_play() {
        let bad = Context {
            selected: PathBuf::from("/g/Game.vpk"),
            result: Err(VpkRefusal {
                kind: VpkRefusalKind::MissingParam,
                detail: "no param.sfo".into(),
            }),
            extracted_folder: false,
        };
        let summary = card(&bad);
        assert_eq!(summary.status, State::NeedsIdentityReview);
        assert!(summary.explanation.contains("no param.sfo"));
        let folder = Context {
            extracted_folder: true,
            ..bad
        };
        let summary = card(&folder);
        assert!(summary.headline.contains("Extracted Vita folders"));
        assert!(
            summary
                .explanation
                .contains("not launched from the library")
        );
        assert_eq!(summary.primary_action, None);
    }

    #[test]
    fn a_ready_card_says_play_starts_the_installed_title_and_flags_a_version_difference() {
        use archivefs_core::launch::Vita3kInstalledEntry;
        let identity = CapturedFileIdentity {
            device: 1,
            inode: 2,
            size: 3,
            modified: None,
        };
        let entry = Vita3kInstalledEntry {
            profile_id: "p".into(),
            executable: "/x".into(),
            executable_identity: identity,
            config_identity: None,
            title_id: "PCSE00001".into(),
            title_root: "/t".into(),
            title_identity: identity,
            param_identity: identity,
            param_sha256: "11".repeat(32),
            display_title: None,
            category: None,
            license: archivefs_core::patch_manager::Vita3kLicenseState::NotRequired,
            firmware: archivefs_core::patch_manager::Vita3kFirmwareState::Unknown,
            blocker: None,
        };
        for differs in [false, true] {
            let mut summary = not_started_summary();
            summary.status = State::Ready;
            summary.primary_action = Some(ReadinessAction::Play);
            let context = context(Resolution::Installed {
                entry: Box::new(entry.clone()),
                param_differs: differs,
            });
            note_vita3k_package(&mut summary, &context);
            assert_eq!(summary.primary_action, Some(ReadinessAction::Play));
            assert!(summary.explanation.contains("not this .vpk package"));
            assert_eq!(summary.warnings.is_empty(), !differs);
            assert_eq!(summary.status == State::ReadyWithWarnings, differs);
        }
    }
}
