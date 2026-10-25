//! A user-approved DOS launch target, never a verified game release.
use std::path::{Path, PathBuf};

use super::dosbox_autoexec::{DosBoxGameReview, DosBoxStartApproval};
use super::planning::{CanonicalIdentityStatus, ResolvedIdentity};

/// Session-only authority for exactly the folder and start program shown to
/// the user. The embedded approval records folder/file identities and hashes
/// of the config and executable. No catalogue or release identity is asserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserApprovedDosBoxTarget {
    folder: PathBuf,
    approval: DosBoxStartApproval,
}

impl UserApprovedDosBoxTarget {
    /// Read-only check used before the UI offers approval. No authority is minted.
    pub fn validate_review_paths(review: &DosBoxGameReview) -> Result<(), String> {
        let start = review
            .start
            .as_ref()
            .ok_or("No single start executable was reviewed.")?;
        for (label, path) in [
            ("game folder", &review.game_directory),
            ("start executable", &start.program_file),
        ] {
            if std::fs::canonicalize(path).ok().as_deref() != Some(path.as_path()) {
                return Err(format!(
                    "The {label} path {} is not canonical or passes through a symbolic link. Select the real folder and executable, then check again.",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    /// Call only after the user explicitly approves these displayed paths.
    /// The selected start must be the config's one reviewed start program;
    /// arbitrary commands, linked paths and substitutions cannot be selected.
    pub fn approve(
        review: &DosBoxGameReview,
        selected_folder: &Path,
        selected_start: &Path,
    ) -> Option<Self> {
        Self::validate_review_paths(review).ok()?;
        let approval = review.approve()?;
        if selected_folder != review.game_directory
            || selected_start != approval.start_file
            || std::fs::canonicalize(selected_folder).ok()?.as_path() != selected_folder
            || std::fs::canonicalize(selected_start).ok()?.as_path() != selected_start
        {
            return None;
        }
        Some(Self {
            folder: selected_folder.into(),
            approval,
        })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }
    pub fn approval(&self) -> &DosBoxStartApproval {
        &self.approval
    }

    /// Revalidate against the freshly read review immediately before launch.
    pub(crate) fn matches(&self, review: &DosBoxGameReview) -> bool {
        self.folder == review.game_directory
            && std::fs::canonicalize(&self.folder).ok().as_deref() == Some(self.folder.as_path())
            && std::fs::canonicalize(&self.approval.start_file)
                .ok()
                .as_deref()
                == Some(self.approval.start_file.as_path())
            && review.is_approvable()
            && self.approval.matches(review).is_ok()
    }

    /// Adapter input only. This is never displayed as verified release identity.
    pub(crate) fn planner_identity(&self) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "DOS".into(),
            game_key: format!(
                "user-approved-dos:{}:{}",
                self.folder.display(),
                self.approval.start_file_sha256
            ),
        })
    }
}

/// Keeps existing canonical launch callers distinct from explicit local trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DosBoxLaunchIdentity {
    Canonical(CanonicalIdentityStatus),
    UserApproved(UserApprovedDosBoxTarget),
}

impl From<CanonicalIdentityStatus> for DosBoxLaunchIdentity {
    fn from(identity: CanonicalIdentityStatus) -> Self {
        Self::Canonical(identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::review_dosbox_game_directory;

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("GAME.EXE"), b"MZ").unwrap();
        std::fs::write(
            root.path().join("dosbox.conf"),
            "[dosbox]\nmemsize=16\n[autoexec]\nmount c .\nc:\ngame.exe\nexit\n",
        )
        .unwrap();
        root
    }
    #[test]
    fn dosbox_target_requires_the_exact_displayed_paths_and_clean_review() {
        let root = fixture();
        let review = review_dosbox_game_directory(root.path()).unwrap();
        let start = root.path().join("GAME.EXE");
        let target = UserApprovedDosBoxTarget::approve(&review, root.path(), &start).unwrap();
        assert_eq!(target.folder(), root.path());
        assert!(target.matches(&review));
        assert!(
            UserApprovedDosBoxTarget::approve(&review, root.path(), &root.path().join("other.exe"))
                .is_none()
        );
        assert!(
            UserApprovedDosBoxTarget::approve(&review, &root.path().join("other"), &start)
                .is_none()
        );
        std::fs::write(
            root.path().join("dosbox.conf"),
            "[dosbox]\n[autoexec]\nmount c /\nc:\ngame.exe\n",
        )
        .unwrap();
        let unsafe_review = review_dosbox_game_directory(root.path()).unwrap();
        assert!(UserApprovedDosBoxTarget::approve(&unsafe_review, root.path(), &start).is_none());
        assert!(!target.matches(&unsafe_review));
    }
    #[test]
    fn dosbox_target_refuses_a_link_in_the_folder_or_executable_path() {
        let root = fixture();
        let aliases = tempfile::tempdir().unwrap();
        let alias = aliases.path().join("link");
        std::os::unix::fs::symlink(root.path(), &alias).unwrap();
        assert!(review_dosbox_game_directory(&alias).is_err());
        let review = review_dosbox_game_directory(root.path()).unwrap();
        assert!(
            UserApprovedDosBoxTarget::approve(&review, &alias, &alias.join("GAME.EXE")).is_none()
        );
        std::fs::rename(root.path().join("GAME.EXE"), root.path().join("REAL.EXE")).unwrap();
        std::os::unix::fs::symlink("REAL.EXE", root.path().join("GAME.EXE")).unwrap();
        let review = review_dosbox_game_directory(root.path()).unwrap();
        assert!(
            UserApprovedDosBoxTarget::approve(&review, root.path(), &root.path().join("GAME.EXE"))
                .is_none()
        );
    }
}
