//! Dedicated GUI state model for Dolphin OnFrame discovery and installation.
//!
//! This module is intentionally view/state only.  It never parses INI files or
//! writes emulator configuration; those operations remain in archivefs-core.

use std::path::PathBuf;

use archivefs_core::patch_manager::{
    DolphinOnFrameBinding, DolphinOnFrameCandidate, DolphinOnFrameInstallStatus,
    bind_dolphin_onframe_candidate,
};

#[derive(Debug, Clone)]
pub enum OnFrameInstallState {
    Idle,
    Discovering {
        source: PathBuf,
    },
    CandidateSelected {
        candidate: DolphinOnFrameCandidate,
    },
    Bound {
        binding: DolphinOnFrameBinding,
    },
    PreviewReady {
        binding: DolphinOnFrameBinding,
        status: DolphinOnFrameInstallStatus,
    },
    AwaitingConfirmation {
        binding: DolphinOnFrameBinding,
        status: DolphinOnFrameInstallStatus,
    },
    Applying {
        binding: DolphinOnFrameBinding,
    },
    Applied {
        binding: DolphinOnFrameBinding,
        transaction_id: Option<String>,
        rollback_available: bool,
    },
    RolledBack {
        binding: DolphinOnFrameBinding,
    },
    AlreadyInstalled {
        binding: DolphinOnFrameBinding,
    },
    Conflict {
        binding: DolphinOnFrameBinding,
        reason: String,
    },
    RecoveryRequired {
        binding: DolphinOnFrameBinding,
        detail: String,
    },
    Failed {
        message: String,
    },
}

impl Default for OnFrameInstallState {
    fn default() -> Self {
        Self::Idle
    }
}

impl OnFrameInstallState {
    pub fn begin_discovery(&mut self, source: PathBuf) {
        *self = Self::Discovering { source };
    }
    pub fn select_candidate(&mut self, candidate: DolphinOnFrameCandidate) {
        *self = Self::CandidateSelected { candidate };
    }

    pub fn bind(
        &mut self,
        game_id: Option<&str>,
        profile: Option<&std::path::Path>,
        conflicting: bool,
    ) {
        let Self::CandidateSelected { candidate } = self else {
            *self = Self::Failed {
                message: "OnFrame candidate is not selected".into(),
            };
            return;
        };
        let Some(profile) = profile else {
            *self = Self::Failed {
                message: "Select a Dolphin profile before binding this patch".into(),
            };
            return;
        };
        match bind_dolphin_onframe_candidate(candidate, game_id, profile, conflicting) {
            Ok(binding) => *self = Self::Bound { binding },
            Err(error) => {
                *self = Self::Failed {
                    message: error.to_string(),
                }
            }
        }
    }

    pub fn preview(&mut self, status: DolphinOnFrameInstallStatus) {
        let Self::Bound { binding } = self else {
            return;
        };
        *self = Self::PreviewReady {
            binding: binding.clone(),
            status,
        };
    }
    pub fn confirm(&mut self) {
        let Self::PreviewReady { binding, status } = self else {
            return;
        };
        if !binding.can_install {
            return;
        }
        *self = Self::AwaitingConfirmation {
            binding: binding.clone(),
            status: *status,
        };
    }
    pub fn approve(&mut self) {
        let Self::AwaitingConfirmation { binding, .. } = self else {
            return;
        };
        *self = Self::Applying {
            binding: binding.clone(),
        };
    }
    pub fn complete(
        &mut self,
        status: DolphinOnFrameInstallStatus,
        transaction_id: Option<String>,
    ) {
        let Self::Applying { binding } = self else {
            return;
        };
        *self = match status {
            DolphinOnFrameInstallStatus::AlreadyInstalled
            | DolphinOnFrameInstallStatus::EnabledExisting => Self::AlreadyInstalled {
                binding: binding.clone(),
            },
            DolphinOnFrameInstallStatus::Conflict => Self::Conflict {
                binding: binding.clone(),
                reason: "Dolphin already has a patch with this name, but the code is different."
                    .into(),
            },
            DolphinOnFrameInstallStatus::Ready => Self::Applied {
                binding: binding.clone(),
                transaction_id,
                rollback_available: true,
            },
            DolphinOnFrameInstallStatus::Refused => Self::Failed {
                message: "Dolphin refused this OnFrame install safely".into(),
            },
        };
    }
    pub fn recovery_required(&mut self, detail: impl Into<String>) {
        if let Self::Applying { binding } = self {
            *self = Self::RecoveryRequired {
                binding: binding.clone(),
                detail: detail.into(),
            };
        }
    }
    pub fn cancel_confirmation(&mut self) {
        if let Self::AwaitingConfirmation { binding, status } = self {
            *self = Self::PreviewReady {
                binding: binding.clone(),
                status: *status,
            };
        }
    }
    pub fn can_install(&self) -> bool {
        matches!(self, Self::PreviewReady { binding, .. } if binding.can_install)
    }
    pub fn destination(&self) -> Option<&PathBuf> {
        match self {
            Self::Bound { binding }
            | Self::PreviewReady { binding, .. }
            | Self::AwaitingConfirmation { binding, .. } => Some(&binding.gamesettings_path),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::patch_manager::{
        CheatDocument, CheatOperation, CheatPlatform, CheatSourceFormat,
    };
    fn candidate() -> DolphinOnFrameCandidate {
        DolphinOnFrameCandidate {
            title: "60 FPS".into(),
            source_path: PathBuf::from("x.ini"),
            platform: CheatPlatform::GameCube,
            document: CheatDocument {
                title: "60 FPS".into(),
                platform: CheatPlatform::GameCube,
                source_format: CheatSourceFormat::DolphinOnFrame,
                operations: vec![CheatOperation::OnFrameWrite32 {
                    address: 1,
                    value: 2,
                }],
                issues: vec![],
                provenance: vec!["x.ini".into()],
            },
            warnings: vec![],
        }
    }
    #[test]
    fn confirmation_is_required_before_applying() {
        let mut s = OnFrameInstallState::default();
        s.begin_discovery(PathBuf::from("x.ini"));
        s.select_candidate(candidate());
        s.bind(
            Some("GMSE01"),
            Some(std::path::Path::new("/dolphin")),
            false,
        );
        s.preview(DolphinOnFrameInstallStatus::Ready);
        assert!(s.can_install());
        s.confirm();
        assert!(matches!(
            s,
            OnFrameInstallState::AwaitingConfirmation { .. }
        ));
        s.approve();
        assert!(matches!(s, OnFrameInstallState::Applying { .. }));
    }
}
