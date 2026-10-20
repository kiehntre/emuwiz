//! Leftover launch-time cheat files (MAME and melonDS).
//!
//! If EmuWiz stops while MAME or melonDS is still running, the temporary
//! copy of the chosen cheats stays behind. This surface finds such copies and,
//! on explicit approval, removes the ones that are provably abandoned.
//!
//! * Looking is read-only and only happens when the user asks.
//! * Ownership, process and path decisions are the core's
//!   (`scan_cheat_runtime_workspaces` / `cleanup_stale_workspace`); this module
//!   adds no second rule set. Removal re-runs the core's checks itself, and
//!   here the entry must also still be exactly what the user reviewed.
//! * Only EmuWiz's own temporary copies are ever removed: never a ROM, cheat
//!   file, save or emulator setting.
use archivefs_core::launch::cheat_runtime::{
    CheatRuntimeRoots, CleanupError, DEFAULT_STALE_GRACE_SECS, WorkspaceDisposition,
    WorkspaceMarker, WorkspaceScanEntry, cleanup_stale_workspace, scan_cheat_runtime_workspaces,
};
use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Provably abandoned; may be removed after approval.
    Removable,
    /// EmuWiz or the emulator is still running.
    Running,
    /// Too new to judge.
    TooRecent,
    /// The emulator may still be running; never removed.
    MayBeRunning,
    /// Another emulator's workspace; not handled here.
    OtherEmulator,
    /// Not provably EmuWiz's; never touched.
    NotOurs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) path: PathBuf,
    pub(crate) emulator: String,
    pub(crate) game: String,
    pub(crate) created_unix: u64,
    pub(crate) verdict: Verdict,
    pub(crate) reason: String,
    /// What the scan saw; removal is refused if it no longer matches.
    marker: Option<WorkspaceMarker>,
}

#[derive(Default)]
pub(crate) struct State {
    rows: Vec<Row>,
    scanned: bool,
    review: Option<Row>,
    message: Option<String>,
    /// Test seam: scan these roots instead of the real ones.
    roots: Option<Vec<CheatRuntimeRoots>>,
}

fn emulator_label(adapter: &str) -> Option<&'static str> {
    match adapter {
        "mame" => Some("MAME"),
        "melonds" => Some("melonDS"),
        _ => None,
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn row_of(entry: &WorkspaceScanEntry) -> Row {
    let adapter = entry
        .marker
        .as_ref()
        .map(|m| m.adapter_id.clone())
        .unwrap_or_default();
    let label = emulator_label(&adapter);
    let (verdict, reason) = match entry.disposition {
        WorkspaceDisposition::OwnedStale if label.is_some() => (
            Verdict::Removable,
            "Abandoned: EmuWiz and the emulator are both gone.".to_string(),
        ),
        WorkspaceDisposition::OwnedStale => (
            Verdict::OtherEmulator,
            "Belongs to another emulator; not handled here.".to_string(),
        ),
        WorkspaceDisposition::OwnedActive => (
            Verdict::Running,
            format!("In use: {}. Close it first.", entry.detail),
        ),
        WorkspaceDisposition::OwnedYoung => (
            Verdict::TooRecent,
            "Created a moment ago; check again shortly.".to_string(),
        ),
        WorkspaceDisposition::OwnedAmbiguous => (
            Verdict::MayBeRunning,
            format!(
                "{}Not removed. Close {} if it is open; this can be removed after the computer restarts.",
                "The emulator may still be running. ",
                label.unwrap_or("the emulator")
            ),
        ),
        WorkspaceDisposition::Foreign => {
            (Verdict::NotOurs, format!("Not removed: {}", entry.detail))
        }
    };
    Row {
        path: entry.path.clone(),
        emulator: label.map_or(adapter.clone(), str::to_string),
        game: entry
            .marker
            .as_ref()
            .map(|m| m.game_identity.chars().take(12).collect())
            .unwrap_or_default(),
        created_unix: entry.marker.as_ref().map_or(0, |m| m.created_unix),
        verdict,
        reason,
        marker: entry.marker.clone(),
    }
}

fn describe(error: &CleanupError) -> String {
    match error {
        CleanupError::RootMissing => "It was already gone. Nothing was removed.".into(),
        CleanupError::OutsideApprovedRoot | CleanupError::NotAWorkspaceName => {
            "That folder is not one of EmuWiz's temporary cheat folders. Nothing was removed."
                .into()
        }
        CleanupError::RootIsSymlink => {
            "That location is a link, not a real folder. Nothing was removed.".into()
        }
        CleanupError::NotADirectory => "That is not a folder. Nothing was removed.".into(),
        CleanupError::MarkerMissing | CleanupError::MarkerInvalid | CleanupError::WrongLaunch => {
            "It no longer looks like an EmuWiz cheat folder. Nothing was removed.".into()
        }
        CleanupError::NotOwnedByCurrentUser => {
            "It belongs to another user. Nothing was removed.".into()
        }
        CleanupError::UnsafePermissions => {
            "Anyone on this computer can write to it, so it is not safe to remove automatically. Nothing was removed.".into()
        }
        CleanupError::Failed(why) => format!("Not removed: {why}"),
    }
}

impl State {
    fn roots(&self) -> Vec<CheatRuntimeRoots> {
        self.roots
            .clone()
            .unwrap_or_else(CheatRuntimeRoots::all_known)
    }

    fn scan_all(&self, now: u64) -> Vec<Row> {
        self.roots()
            .iter()
            .flat_map(|roots| scan_cheat_runtime_workspaces(roots, now, DEFAULT_STALE_GRACE_SECS))
            .map(|entry| row_of(&entry))
            .collect()
    }

    /// Read-only: lists what is there and says what each entry is.
    pub(crate) fn scan(&mut self, now: u64) {
        self.rows = self.scan_all(now);
        self.scanned = true;
        self.review = None;
        self.message = Some(if self.rows.is_empty() {
            "No leftover cheat files were found.".into()
        } else {
            format!(
                "Found {} temporary cheat folder(s). Nothing was changed.",
                self.rows.len()
            )
        });
    }

    /// Opens the confirmation for one removable entry. Anything else is refused.
    pub(crate) fn start_review(&mut self, path: &Path) {
        match self.rows.iter().find(|r| r.path == path) {
            Some(row) if row.verdict == Verdict::Removable => self.review = Some(row.clone()),
            Some(row) => {
                self.message = Some(format!("Not removable: {}", row.reason));
            }
            None => self.message = Some("That folder is not in the list. Check again.".into()),
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.review = None;
    }

    /// Removes the reviewed folder if, right now, it is still exactly what
    /// the user approved and the core still judges it abandoned.
    pub(crate) fn confirm(&mut self, now: u64) {
        let Some(review) = self.review.take() else {
            return;
        };
        let fresh = self
            .scan_all(now)
            .into_iter()
            .find(|row| row.path == review.path);
        let message = match fresh {
            None => "It was already gone. Nothing was removed.".to_string(),
            Some(row) if row.verdict != Verdict::Removable || row.marker != review.marker => {
                "It changed since you looked at it, so nothing was removed. Check again.".into()
            }
            Some(_) => {
                let roots = self
                    .roots()
                    .into_iter()
                    .find(|roots| review.path.parent() == Some(roots.approved_root.as_path()));
                match roots {
                    None => "That folder is not under an EmuWiz temporary location. Nothing was removed.".into(),
                    Some(roots) => {
                        match cleanup_stale_workspace(&roots, &review.path, now, DEFAULT_STALE_GRACE_SECS) {
                            Ok(()) => format!(
                                "Removed the temporary {} cheat folder. Your ROMs, cheat files, saves and emulator settings were not touched.",
                                review.emulator
                            ),
                            Err(error) => describe(&error),
                        }
                    }
                }
            }
        };
        self.rows = self.scan_all(now);
        self.scanned = true;
        self.message = Some(message);
    }
}

fn age_text(now: u64, created: u64) -> String {
    let secs = now.saturating_sub(created);
    match secs {
        0..=119 => "just now".into(),
        120..=7199 => format!("{} min ago", secs / 60),
        7200..=172_799 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// The panel. Looking happens only when the button is pressed.
pub(crate) fn show(ui: &mut egui::Ui, state: &mut State) {
    egui::CollapsingHeader::new("Leftover launch-time cheat files (MAME, melonDS)")
        .default_open(false)
        .show(ui, |ui| {
            ui.label("If EmuWiz stops while MAME or melonDS is running, the temporary copy of the cheats you chose stays behind. Looking is read-only; nothing is removed unless you approve it.");
            if ui.button("Check for leftover cheat files").clicked() {
                state.scan(unix_now());
            }
            let now = unix_now();
            let mut review_path = None;
            for row in &state.rows {
                ui.separator();
                ui.label(format!(
                    "{} · game {} · {}",
                    row.emulator,
                    if row.game.is_empty() { "unknown" } else { &row.game },
                    if row.created_unix == 0 { "unknown time".into() } else { age_text(now, row.created_unix) },
                ));
                ui.small(row.path.display().to_string());
                ui.label(row.reason.as_str());
                if row.verdict == Verdict::Removable
                    && state.review.is_none()
                    && ui.button("Remove this temporary folder…").clicked()
                {
                    review_path = Some(row.path.clone());
                }
            }
            if let Some(path) = review_path {
                state.start_review(&path);
            }
            let mut confirm = false;
            let mut cancel = false;
            if let Some(review) = &state.review {
                ui.separator();
                ui.label(format!(
                    "Remove {}? This deletes only EmuWiz's temporary {} cheat copy. Your ROMs, cheat files, saves and emulator settings are not touched.",
                    review.path.display(),
                    review.emulator
                ));
                confirm = ui.button("Yes, remove it").clicked();
                cancel = ui.button("Cancel").clicked();
            }
            if confirm {
                state.confirm(unix_now());
            } else if cancel {
                state.cancel();
            }
            if let Some(message) = &state.message {
                ui.label(message.as_str());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::launch::cheat_runtime::{
        MARKER_SCHEMA_VERSION, MarkerState, WORKSPACE_MARKER_NAME,
    };
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::process::{Child, Command, Stdio};

    struct Fx {
        dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("cheat-root");
            fs::create_dir_all(&root).unwrap();
            Self { dir, root }
        }

        fn state(&self) -> State {
            State {
                roots: Some(vec![CheatRuntimeRoots {
                    approved_root: self.root.clone(),
                }]),
                ..State::default()
            }
        }

        /// An EmuWiz-looking workspace with its contents.
        fn workspace(
            &self,
            id: &str,
            adapter: &str,
            state: MarkerState,
            owner_pid: u32,
            emulator_pid: Option<u32>,
        ) -> PathBuf {
            let path = self.root.join(format!("cheats-{id}"));
            fs::create_dir_all(path.join("cheats")).unwrap();
            fs::write(path.join("cheats").join("Game.mch"), b"CODE 0 X\n").unwrap();
            let marker = WorkspaceMarker {
                schema_version: MARKER_SCHEMA_VERSION,
                created_by: "emuwiz".into(),
                launch_id: id.into(),
                game_identity: "abcdef0123456789".into(),
                adapter_id: adapter.into(),
                plan_digest: format!("digest-{id}"),
                cheat_ids: vec![],
                created_unix: unix_now().saturating_sub(7200),
                owner_pid,
                owner_start_ticks: None,
                state,
                emulator_pid,
            };
            fs::write(
                path.join(WORKSPACE_MARKER_NAME),
                serde_json::to_vec(&marker).unwrap(),
            )
            .unwrap();
            path
        }
    }

    fn dead_pid() -> u32 {
        let mut child = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    /// A fake emulator that stays alive until killed.
    fn live_emulator() -> Child {
        Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn verdict(state: &State, path: &Path) -> Verdict {
        state.rows.iter().find(|r| r.path == path).unwrap().verdict
    }

    #[test]
    fn looking_is_read_only_and_classifies_every_kind_of_leftover() {
        let fx = Fx::new();
        let dead = dead_pid();
        let mut emulator = live_emulator();
        let abandoned = fx.workspace(
            "a1",
            "melonds",
            MarkerState::EmulatorStarted,
            dead,
            Some(dead),
        );
        let running = fx.workspace(
            "r1",
            "mame",
            MarkerState::EmulatorStarted,
            dead,
            Some(emulator.id()),
        );
        let owner_alive = fx.workspace(
            "o1",
            "mame",
            MarkerState::EmulatorStarted,
            std::process::id(),
            Some(dead),
        );
        let ambiguous = fx.workspace("m1", "melonds", MarkerState::LaunchAttempted, dead, None);
        let other = fx.workspace(
            "x1",
            "retroarch",
            MarkerState::EmulatorStarted,
            dead,
            Some(dead),
        );
        let foreign = fx.root.join("cheats-foreign");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("data.txt"), b"not ours").unwrap();
        let before: Vec<_> = [
            &abandoned,
            &running,
            &owner_alive,
            &ambiguous,
            &other,
            &foreign,
        ]
        .iter()
        .map(|p| p.exists())
        .collect();

        let mut state = fx.state();
        state.scan(unix_now());
        assert_eq!(verdict(&state, &abandoned), Verdict::Removable);
        assert_eq!(verdict(&state, &running), Verdict::Running);
        assert_eq!(verdict(&state, &owner_alive), Verdict::Running);
        assert_eq!(verdict(&state, &ambiguous), Verdict::MayBeRunning);
        assert_eq!(verdict(&state, &other), Verdict::OtherEmulator);
        assert_eq!(verdict(&state, &foreign), Verdict::NotOurs);
        let after: Vec<_> = [
            &abandoned,
            &running,
            &owner_alive,
            &ambiguous,
            &other,
            &foreign,
        ]
        .iter()
        .map(|p| p.exists())
        .collect();
        assert_eq!(before, after, "scanning must not remove anything");
        assert!(foreign.join("data.txt").exists());
        let reason = &state
            .rows
            .iter()
            .find(|r| r.path == ambiguous)
            .unwrap()
            .reason;
        assert!(reason.contains("may still be running"), "{reason}");
        assert!(reason.contains("restarts"), "{reason}");
        emulator.kill().unwrap();
        emulator.wait().unwrap();
    }

    #[test]
    fn nothing_is_removed_without_review_and_approval() {
        let fx = Fx::new();
        let dead = dead_pid();
        let abandoned = fx.workspace("a1", "mame", MarkerState::EmulatorStarted, dead, Some(dead));
        let mut state = fx.state();
        state.scan(unix_now());
        // Confirming without a review does nothing.
        state.confirm(unix_now());
        assert!(abandoned.exists());
        // Cancelling a review removes nothing.
        state.start_review(&abandoned);
        state.cancel();
        state.confirm(unix_now());
        assert!(abandoned.exists());
        // Approved removal removes exactly that workspace.
        state.start_review(&abandoned);
        state.confirm(unix_now());
        assert!(!abandoned.exists());
        assert!(state.message.as_ref().unwrap().contains("Removed"));
        assert!(fx.root.exists());
    }

    #[test]
    fn running_ambiguous_young_foreign_and_other_emulator_entries_cannot_be_reviewed() {
        let fx = Fx::new();
        let dead = dead_pid();
        let mut emulator = live_emulator();
        let paths = [
            fx.workspace(
                "r1",
                "mame",
                MarkerState::EmulatorStarted,
                dead,
                Some(emulator.id()),
            ),
            fx.workspace("m1", "melonds", MarkerState::LaunchAttempted, dead, None),
            fx.workspace(
                "x1",
                "retroarch",
                MarkerState::EmulatorStarted,
                dead,
                Some(dead),
            ),
        ];
        let mut state = fx.state();
        state.scan(unix_now());
        for path in &paths {
            state.start_review(path);
            assert!(state.review.is_none(), "{path:?} must not be reviewable");
            assert!(state.message.as_ref().unwrap().contains("Not removable"));
            state.confirm(unix_now());
            assert!(path.exists());
        }
        emulator.kill().unwrap();
        emulator.wait().unwrap();
    }

    #[test]
    fn an_entry_that_changed_after_review_is_not_removed() {
        let fx = Fx::new();
        let dead = dead_pid();
        let path = fx.workspace("a1", "mame", MarkerState::EmulatorStarted, dead, Some(dead));
        let mut state = fx.state();
        state.scan(unix_now());
        state.start_review(&path);
        // Between review and approval a new MAME is started in it (a live emulator).
        let mut emulator = live_emulator();
        let mut marker: WorkspaceMarker =
            serde_json::from_slice(&fs::read(path.join(WORKSPACE_MARKER_NAME)).unwrap()).unwrap();
        marker.emulator_pid = Some(emulator.id());
        fs::write(
            path.join(WORKSPACE_MARKER_NAME),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        state.confirm(unix_now());
        assert!(path.exists(), "a workspace now in use must be kept");
        assert!(
            state
                .message
                .as_ref()
                .unwrap()
                .contains("changed since you looked")
        );
        emulator.kill().unwrap();
        emulator.wait().unwrap();
    }

    #[test]
    fn a_workspace_replaced_by_a_link_after_review_is_refused_and_its_target_kept() {
        let fx = Fx::new();
        let dead = dead_pid();
        let path = fx.workspace(
            "a1",
            "melonds",
            MarkerState::EmulatorStarted,
            dead,
            Some(dead),
        );
        let mut state = fx.state();
        state.scan(unix_now());
        state.start_review(&path);
        let target = fx.dir.path().join("user-docs");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.txt"), b"mine").unwrap();
        fs::remove_dir_all(&path).unwrap();
        symlink(&target, &path).unwrap();
        state.confirm(unix_now());
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"mine");
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_linked_cheat_root_is_never_listed_or_cleaned() {
        let fx = Fx::new();
        let dead = dead_pid();
        let real = fx.dir.path().join("real-place");
        fs::create_dir_all(&real).unwrap();
        let victim = real.join("cheats-v1");
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep.txt"), b"mine").unwrap();
        let planted = fx.workspace("v1", "mame", MarkerState::EmulatorStarted, dead, Some(dead));
        fs::copy(
            planted.join(WORKSPACE_MARKER_NAME),
            victim.join(WORKSPACE_MARKER_NAME),
        )
        .unwrap();
        fs::remove_dir_all(&fx.root).unwrap();
        symlink(&real, &fx.root).unwrap();
        let mut state = fx.state();
        state.scan(unix_now());
        assert!(state.rows.is_empty());
        assert_eq!(fs::read(victim.join("keep.txt")).unwrap(), b"mine");
    }

    #[test]
    fn a_partly_removed_workspace_is_finished_and_repeating_the_removal_is_harmless() {
        let fx = Fx::new();
        let dead = dead_pid();
        let path = fx.workspace(
            "p1",
            "melonds",
            MarkerState::CleanupStarted,
            dead,
            Some(dead),
        );
        fs::remove_dir_all(path.join("cheats")).unwrap();
        let mut state = fx.state();
        state.scan(unix_now());
        state.start_review(&path);
        let approved = state.review.clone().unwrap();
        state.confirm(unix_now());
        assert!(!path.exists());
        // Approving the same removal again finds nothing and says so.
        state.review = Some(approved);
        state.confirm(unix_now());
        assert!(state.rows.is_empty());
        assert!(state.message.as_ref().unwrap().contains("already gone"));
    }

    #[test]
    fn an_unlikely_pid_reuse_for_the_owner_is_not_trusted_as_alive() {
        let fx = Fx::new();
        let path = fx.workspace(
            "u1",
            "mame",
            MarkerState::EmulatorStarted,
            std::process::id(),
            Some(dead_pid()),
        );
        // Record start ticks that cannot belong to this process: a later process
        // reusing the id is not the owner.
        let mut marker: WorkspaceMarker =
            serde_json::from_slice(&fs::read(path.join(WORKSPACE_MARKER_NAME)).unwrap()).unwrap();
        marker.owner_start_ticks = Some(1);
        fs::write(
            path.join(WORKSPACE_MARKER_NAME),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        let mut state = fx.state();
        state.scan(unix_now());
        assert_eq!(verdict(&state, &path), Verdict::Removable);
    }

    #[test]
    fn no_scan_happens_until_the_user_asks() {
        let fx = Fx::new();
        let dead = dead_pid();
        let path = fx.workspace("a1", "mame", MarkerState::EmulatorStarted, dead, Some(dead));
        let state = fx.state();
        assert!(!state.scanned && state.rows.is_empty());
        assert!(path.exists());
    }
}
