//! Play for DOS games: DOSBox inside the bubblewrap sandbox.
//!
//! This is only the GUI over the core engine in
//! `archivefs_core::launch::{dosbox_autoexec, dosbox_execution, dosbox_sandbox}`.
//! It adds no launch logic of its own:
//!
//! 1. A background check finds the folder carrying the game's own
//!    `dosbox.conf`, the installed DOSBox, reviews `[autoexec]`, builds the
//!    command plan, asks the sandbox whether it would accept the grants, and
//!    probes that bubblewrap works on this machine. Every refusal is shown as
//!    a sentence; nothing is offered while any check fails.
//! 2. The user reads the folder that will be exposed (writable, to the game
//!    only), the start command and what else is shared, then explicitly
//!    approves them. The approval is minted by the core review itself and is
//!    kept only in memory for the review on screen.
//! 3. Launch goes through the shared standalone launch state to
//!    `preflight_and_launch_dosbox`, which re-reviews the folder, refuses if
//!    anything approved has moved, and only ever spawns inside the sandbox.
//!    There is no unsandboxed path, no Stop action and no "the game ran" claim.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use archivefs_core::launch::dosbox_autoexec::AutoexecEffect;
use archivefs_core::launch::dosbox_sandbox::sandboxed_command;
use archivefs_core::launch::planning::CanonicalIdentityStatus;
use archivefs_core::launch::{
    BWRAP_PATH, DosBoxBindingRefusal, DosBoxConfigStatus, DosBoxGameReview, DosBoxHostAccess,
    DosBoxLaunchExecutionError, DosBoxLaunchPreflightErrorKind, DosBoxLaunchRequest,
    DosBoxLaunchSpawnError, DosBoxNativeLaunchBinding, DosBoxSandboxError,
    build_dosbox_command_plan, discover_dosbox_config, locate_dosbox_game_directory, probe_sandbox,
    resolve_dosbox_native_launch_binding,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

use super::{StandaloneLaunchRequest, StandaloneLaunchState};
use crate::ui::{components as widgets, theme};

/// The canonical platform DOSBox serves.
pub(crate) const DOS_PLATFORM_ID: &str = archivefs_core::launch::DOSBOX_SUPPORTED_PLATFORM_ID;

/// The tracking key the shared standalone launch state uses for DOSBox.
pub(super) const ADAPTER_KEY: &str = "DOSBox";

/// Everything a clean check found. Only this, plus an approval minted from
/// `review`, can become a launch request.
#[derive(Debug, Clone)]
pub(crate) struct DosBoxReady {
    pub(crate) review: DosBoxGameReview,
    pub(crate) binding: DosBoxNativeLaunchBinding,
    pub(crate) access: DosBoxHostAccess,
}

/// The result of one background check of the selected game.
#[derive(Debug, Clone)]
pub(crate) enum DosBoxCheck {
    Ready(Box<DosBoxReady>),
    /// Plain sentences saying why DOSBox is not offered; never empty.
    Refused(Vec<String>),
}

fn refused(reason: impl Into<String>) -> DosBoxCheck {
    DosBoxCheck::Refused(vec![reason.into()])
}

/// The full read-only check. `binding` and `bwrap` are injected so tests
/// never depend on what is installed. Nothing is started except the sandbox
/// probe (`true` inside bubblewrap); no game file is changed.
pub(crate) fn check_dosbox_game(
    selected: &Path,
    identity: &CanonicalIdentityStatus,
    binding: Result<DosBoxNativeLaunchBinding, DosBoxBindingRefusal>,
    access: DosBoxHostAccess,
    bwrap: &Path,
) -> DosBoxCheck {
    match identity {
        CanonicalIdentityStatus::Unknown => {}
        CanonicalIdentityStatus::Resolved(resolved) if resolved.platform_id == DOS_PLATFORM_ID => {}
        _ => {
            return refused(
                "EmuWiz has not verified this game as a DOS game, so DOSBox is not offered.",
            );
        }
    }
    let Some(game_directory) = locate_dosbox_game_directory(selected, None) else {
        return refused(format!(
            "No folder with a usable dosbox.conf was found at {} or the folders just above it. \
             EmuWiz only starts DOS games whose own dosbox.conf has an [autoexec] section naming \
             the program to run; it never picks a program from the file names.",
            selected.display()
        ));
    };
    let mut reasons = Vec::new();
    if let Err(refusal) = &binding {
        reasons.push(format!(
            "DOSBox is not installed or cannot be used: {}. Install DOSBox Staging (or DOSBox) \
             from your distribution, then check again.",
            refusal.detail().trim_end_matches('.')
        ));
    }
    let review = match archivefs_core::launch::review_dosbox_game_directory(&game_directory) {
        Ok(review) => review,
        Err(problem) => {
            reasons.push(problem.explanation());
            return DosBoxCheck::Refused(reasons);
        }
    };
    reasons.extend(review.problems.iter().map(|problem| problem.explanation()));
    if review.is_approvable()
        && let Err(reason) =
            archivefs_core::launch::UserApprovedDosBoxTarget::validate_review_paths(&review)
    {
        reasons.push(reason);
    }

    let config = discover_dosbox_config(&game_directory, &TrustedRoots::none());
    if !matches!(&config, DosBoxConfigStatus::Verified { config_path, .. } if *config_path == review.config_path)
    {
        reasons.push("dosbox.conf changed while it was being checked. Check again.".into());
    }
    // Preview validation only. No launch identity is minted until approval.
    let preview_identity =
        CanonicalIdentityStatus::Resolved(archivefs_core::launch::ResolvedIdentity {
            platform_id: DOS_PLATFORM_ID.into(),
            game_key: "dosbox-review-only".into(),
        });
    let plan = build_dosbox_command_plan(&preview_identity, &game_directory, &config, &binding);
    // Binding refusals were already explained above in plainer words.
    reasons.extend(
        plan.blockers
            .iter()
            .filter(|blocker| {
                !matches!(
                    blocker.kind,
                    archivefs_core::launch::LaunchBlockerKind::DosBoxBindingUnavailable
                        | archivefs_core::launch::LaunchBlockerKind::DosBoxVariantUnsupported
                )
            })
            .map(|blocker| sentence(&blocker.detail)),
    );
    if let Some(command) = &plan.command
        && let Err(error) = sandboxed_command(command, &access, bwrap)
    {
        reasons.push(sandbox_sentence(&error));
    }
    if let Err(error) = probe_sandbox(bwrap) {
        reasons.push(sandbox_sentence(&error));
    }
    let Ok(binding) = binding else {
        return DosBoxCheck::Refused(reasons);
    };
    if !reasons.is_empty() || !review.is_approvable() {
        if reasons.is_empty() {
            reasons.push("The dosbox.conf review did not find one start program.".into());
        }
        return DosBoxCheck::Refused(reasons);
    }
    DosBoxCheck::Ready(Box::new(DosBoxReady {
        review,
        binding,
        access,
    }))
}

fn sentence(text: &str) -> String {
    let text = text.trim().trim_end_matches('.');
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn sandbox_sentence(error: &DosBoxSandboxError) -> String {
    match error {
        DosBoxSandboxError::Unavailable(detail) => format!(
            "DOSBox only runs inside the bubblewrap sandbox, and the sandbox is not available \
             here: {}.",
            detail.trim_end_matches('.')
        ),
        DosBoxSandboxError::InvalidGrant(detail) => format!(
            "The sandbox refused this game folder or DOSBox installation: {}.",
            detail.trim_end_matches('.')
        ),
    }
}

/// A launch refusal or failure in words, never the debug form of the error.
pub(super) fn dosbox_refusal_text(error: &DosBoxLaunchExecutionError) -> String {
    use DosBoxLaunchPreflightErrorKind as K;
    match error {
        DosBoxLaunchExecutionError::Preflight(preflight) => {
            let detail = preflight.detail.trim().trim_end_matches('.');
            let next = match preflight.kind {
                K::GameDirectoryNotAbsolute
                | K::GameDirectoryNotFound
                | K::GameDirectoryIsSymlink
                | K::GameDirectoryNotDirectory => {
                    "The game folder is no longer where it was checked. Restore it, then check again."
                }
                K::GameDirectoryChanged | K::StartCommandNotApproved => {
                    "Something you approved has changed. Check again and review the folder before approving it again."
                }
                K::ExecutableUnavailable => {
                    "The DOSBox installation changed or is unavailable. Check again."
                }
                K::ConfigNotVerified | K::ConfigPathDrift => {
                    "The game's dosbox.conf changed or moved. Check again."
                }
                K::AutoexecUnsafe => "The game's [autoexec] section no longer passes review.",
                K::CommandBlocked | K::CommandMissing => {
                    "The DOSBox command could not be built safely. Check again."
                }
            };
            format!("DOSBox was not started: {detail}. {next}")
        }
        DosBoxLaunchExecutionError::Spawn(DosBoxLaunchSpawnError::Sandbox(error)) => {
            format!("DOSBox was not started. {}", sandbox_sentence(error))
        }
        DosBoxLaunchExecutionError::Spawn(DosBoxLaunchSpawnError::Spawn(error)) => {
            format!("DOSBox could not be started inside the sandbox: {error}.")
        }
    }
}

/// What the user has approved, for exactly the check on screen.
pub(crate) fn launch_request(
    ready: &DosBoxReady,
    target: &archivefs_core::launch::UserApprovedDosBoxTarget,
) -> StandaloneLaunchRequest {
    StandaloneLaunchRequest::DosBox(Box::new(DosBoxLaunchRequest {
        game_directory: ready.review.game_directory.clone(),
        identity: archivefs_core::launch::DosBoxLaunchIdentity::UserApproved(target.clone()),
        expected_executable: ready.binding.executable.clone(),
        expected_variant: ready.binding.variant,
        expected_config_path: ready.review.config_path.clone(),
        approval: target.approval().clone(),
        host_access: ready.access.clone(),
    }))
}

/// What the sandbox shares besides the game folder, in words.
pub(crate) fn shared_with_dosbox(access: &DosBoxHostAccess) -> String {
    let names: Vec<&str> = access
        .environment
        .iter()
        .filter_map(|(name, _)| match name.as_str() {
            "WAYLAND_DISPLAY" => Some("the Wayland display"),
            "DISPLAY" => Some("the X11 display (other X11 programs can see its window and keys)"),
            _ => None,
        })
        .collect();
    if names.is_empty() {
        "No display was found in this session, so DOSBox may not be able to open a window.".into()
    } else {
        format!(
            "Shared read-only: {}. Not shared: sound, network, your home folder, other games.",
            names.join(" and ")
        )
    }
}

fn effect_label(effect: &AutoexecEffect) -> String {
    match effect {
        AutoexecEffect::Harmless => "no effect outside DOSBox".into(),
        AutoexecEffect::Mount { drive } => format!("mounts part of this folder as {drive}:"),
        AutoexecEffect::SwitchDrive(drive) => format!("switches to {drive}:"),
        AutoexecEffect::ChangeDirectory => "changes folder inside a mounted drive".into(),
        AutoexecEffect::Exit => "closes DOSBox".into(),
        AutoexecEffect::Start => "starts the selected program".into(),
    }
}

type CheckKey = (PathBuf, CanonicalIdentityStatus);

/// The DOSBox part of the shared standalone launch state.
#[derive(Default)]
pub(crate) struct Panel {
    /// The game the Launch page is showing, set by its caller each frame.
    focus: Option<PathBuf>,
    key: Option<CheckKey>,
    job: Option<Receiver<DosBoxCheck>>,
    result: Option<DosBoxCheck>,
    /// Minted from `result`'s review when the user approved it; cleared by
    /// every new check.
    approval: Option<archivefs_core::launch::UserApprovedDosBoxTarget>,
}

impl Panel {
    pub(crate) fn focus(&mut self, path: Option<&Path>) {
        let next = path.map(Path::to_path_buf);
        if self.focus != next {
            self.approval = None;
            self.key = None;
            self.result = None;
            self.job = None;
        }
        self.focus = next;
    }

    fn restart(&mut self, ctx: &egui::Context, key: CheckKey) {
        self.key = Some(key.clone());
        self.result = None;
        self.approval = None;
        let (sender, receiver) = mpsc::channel();
        self.job = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (path, identity) = key;
            let result = std::panic::catch_unwind(|| {
                check_dosbox_game(
                    &path,
                    &identity,
                    resolve_dosbox_native_launch_binding(),
                    DosBoxHostAccess::from_session_environment(),
                    Path::new(BWRAP_PATH),
                )
            })
            .unwrap_or_else(|_| refused("The DOSBox check stopped unexpectedly. Check again."));
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        match job.try_recv() {
            Ok(result) => {
                self.result = Some(result);
                self.job = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.result = Some(refused("The DOSBox check stopped unexpectedly."));
                self.job = None;
            }
        }
    }

    /// Records the user's approval of the review on screen. Returns false
    /// (and records nothing) when that review cannot be approved.
    pub(crate) fn approve(&mut self) -> bool {
        let Some(DosBoxCheck::Ready(ready)) = &self.result else {
            return false;
        };
        let Some(start) = &ready.review.start else {
            return false;
        };
        self.approval = archivefs_core::launch::UserApprovedDosBoxTarget::approve(
            &ready.review,
            &ready.review.game_directory,
            &start.program_file,
        );
        self.approval.is_some()
    }

    fn request(&self) -> Option<StandaloneLaunchRequest> {
        let (Some(DosBoxCheck::Ready(ready)), Some(approval), Some((path, _))) =
            (&self.result, &self.approval, &self.key)
        else {
            return None;
        };
        if self.focus.as_ref().is_some_and(|focused| focused != path) {
            return None;
        }
        Some(launch_request(ready, approval))
    }
}

/// Shared Blue/Gold review of a possible DOS launch target. A folder/config
/// suggests a review only; explicit approval creates local launch authority.
pub(super) fn show(
    ui: &mut egui::Ui,
    identity: &CanonicalIdentityStatus,
    state: &mut StandaloneLaunchState,
) {
    widgets::card(ui, |ui| {
        ui.strong("DOSBox (sandboxed)");
        let Some(path) = state.dosbox.focus.clone() else {
            ui.label("Open this game's Launch page to check DOSBox.");
            return;
        };
        let key = (path, identity.clone());
        let launching = state.is_active();
        if state.dosbox.key.as_ref() != Some(&key) {
            if launching {
                // Never show another game's check; wait for the running one.
                ui.label(
                    "Another game is running. Close it, then DOSBox is checked for this game.",
                );
                return;
            }
            state.dosbox.restart(ui.ctx(), key.clone());
        }
        state.dosbox.poll();
        let mut check_again = false;
        match &state.dosbox.result {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the game folder, DOSBox and the sandbox…");
                });
            }
            Some(DosBoxCheck::Refused(reasons)) => {
                widgets::banner(
                    ui,
                    "DOSBox will not start this game",
                    "Nothing was started and no file was changed.",
                    widgets::StatusTone::Blocked,
                );
                for reason in reasons {
                    ui.label(format!("• {reason}"));
                }
                check_again = ui.button("Check again").clicked();
            }
            Some(DosBoxCheck::Ready(ready)) => {
                let ready = ready.clone();
                check_again = show_ready(ui, &ready, state);
            }
        }
        if check_again && !state.is_active() {
            state.dosbox.restart(ui.ctx(), key);
        }
    });
}

/// Returns whether the user asked to check again.
fn show_ready(ui: &mut egui::Ui, ready: &DosBoxReady, state: &mut StandaloneLaunchState) -> bool {
    let review = &ready.review;
    let Some(start) = review.start.as_ref() else {
        return false;
    };
    ui.label("Local DOS launch target — not a DAT-verified game release.");
    ui.label(format!("Game folder: {}", review.game_directory.display()));
    ui.label(
        "DOSBox runs in a sandbox. The game can read and change every file in this folder \
         (that is how its saves work), and nothing else of yours.",
    );
    ui.label(format!(
        "{}: {}",
        ready.binding.variant.label(),
        ready.binding.executable.display()
    ));
    ui.label(shared_with_dosbox(&ready.access));
    ui.label(format!(
        "Start command (line {} of dosbox.conf): {}",
        start.line, start.command_text
    ));
    ui.label(format!("Starts: {}", start.program_file.display()));
    egui::CollapsingHeader::new(format!("All {} [autoexec] lines", review.lines.len()))
        .id_salt("dosbox-autoexec-lines")
        .show(ui, |ui| {
            for line in &review.lines {
                ui.label(format!(
                    "{:>3}  {}  — {}",
                    line.number,
                    line.text,
                    effect_label(&line.effect)
                ));
            }
        });
    ui.add_space(4.0);
    let mut check_again = false;
    match state.dosbox.request() {
        None => {
            ui.label(
                "Approving lets this game change files in its folder. Only approve a game you \
                 trust; the sandbox limits the damage, it is not a guarantee.",
            );
            ui.horizontal(|ui| {
                if ui.button("Allow this folder and start command").clicked() {
                    state.dosbox.approve();
                }
                check_again = ui.button("Check again").clicked();
            });
        }
        Some(request) => {
            ui.label(
                egui::RichText::new("User-approved for this session. EmuWiz checks everything again before DOSBox starts.")
                    .small()
                    .color(theme::muted(ui)),
            );
            super::show_standalone_launch_action(ui, state, request);
            if !state.is_active() {
                ui.horizontal(|ui| {
                    if ui.button("Withdraw approval").clicked() {
                        state.dosbox.approval = None;
                    }
                    check_again = ui.button("Check again").clicked();
                });
            }
        }
    }
    check_again
}

#[cfg(test)]
#[path = "dosbox_tests.rs"]
mod tests;

#[cfg(test)]
impl super::StandaloneLaunchState {
    pub(crate) fn dosbox_approval_for_test(
        &mut self,
        target: Option<archivefs_core::launch::UserApprovedDosBoxTarget>,
    ) -> bool {
        if let Some(target) = target {
            self.dosbox.approval = Some(target);
        }
        self.dosbox.approval.is_some()
    }
}
