//! The optional RomM title update that follows a Rename, and its Undo.
//!
//! Off unless the person turns on "Rename also updates RomM's title". The
//! RomM request runs on a worker with an Activity job, after the local rename
//! has already finished: a RomM failure is reported as a title-update failure
//! and never touches the renamed file. The rules (which entry, when it is left
//! alone, what Undo restores) live in
//! `archivefs_core::identity_source::romm::title_sync`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use archivefs_core::identity_source::model::IdentityProvider;
use archivefs_core::identity_source::romm::title_sync::{
    TitleSyncOutcome, record_folder, restore_title_after_undo, update_title_after_rename,
};
use archivefs_core::identity_source::settings::{SettingsLocation, default_identity_root};

use super::App;

/// What to do in RomM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RommTitleAction {
    /// History entry `transaction_id` renamed `old` to `new`.
    AfterRename {
        transaction_id: String,
        old: PathBuf,
        new: PathBuf,
    },
    /// History entry `transaction_id` was undone.
    AfterUndo { transaction_id: String },
}

/// A running RomM request.
pub(super) struct RommTitleJob {
    job: u64,
    undo: bool,
    receiver: std::sync::mpsc::Receiver<Result<Option<TitleSyncOutcome>, String>>,
}

/// The setting's label and explanation, shown on the RomM card.
pub(super) const SETTING_LABEL: &str = "Rename also updates RomM's title";
pub(super) const SETTING_EXPLANATION: &str = "Off: EmuWiz only reads from RomM. On: after you rename a game, EmuWiz changes that one game's title in RomM, only if RomM never identified it and nobody edited its title, and History Undo puts it back unless the title was edited since. Nothing else in RomM is changed. It uses its own RomM client token with the roms.write permission, which RomM also allows for other edits to games: make a token only for this, keep its file private, and keep a separate read-only token for everything else. Browsing and import keep using the read-only token, and work without this one.";

/// The label of the title-update token field.
pub(super) const TOKEN_LABEL: &str = "Title-update token file (roms.write, used only for this)";

fn settings_location() -> Result<SettingsLocation, String> {
    let root = default_identity_root()?;
    Ok(SettingsLocation::new(&root, IdentityProvider::Romm))
}

/// Whether the person turned title updates on. `false` when RomM is not set
/// up or the settings cannot be read: the read-only default.
pub(super) fn title_updates_enabled() -> bool {
    settings_location()
        .and_then(|location| location.load().map_err(|error| error.detail()))
        .is_ok_and(|settings| settings.source.enabled && settings.rename_updates_title)
}

/// Saves the setting (only that field).
pub(super) fn set_title_updates(on: bool) -> Result<(), String> {
    let location = settings_location()?;
    let mut settings = location.load().map_err(|error| error.detail())?;
    settings.rename_updates_title = on;
    location
        .save(&settings)
        .map(|_| ())
        .map_err(|error| error.detail())
}

/// The configured title-update token file, for the card.
pub(super) fn title_token_path() -> String {
    settings_location()
        .and_then(|location| location.load().map_err(|error| error.detail()))
        .ok()
        .and_then(|settings| settings.title_token_path)
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

/// Saves the title-update token file (only that field). Empty clears it. The
/// file is checked the same way as the read-only token's, and must not be it.
pub(super) fn set_title_token_path(path: &str) -> Result<(), String> {
    let location = settings_location()?;
    let mut settings = location.load().map_err(|error| error.detail())?;
    let trimmed = path.trim();
    settings.title_token_path = (!trimmed.is_empty()).then(|| PathBuf::from(trimmed));
    if settings.title_token_path.is_some() {
        archivefs_core::identity_source::settings::title_token_file(&settings)?;
    }
    location
        .save(&settings)
        .map(|_| ())
        .map_err(|error| error.detail())
}

/// Runs `action` against the configured RomM. `Ok(None)`: nothing to do.
fn run(action: RommTitleAction) -> Result<Option<TitleSyncOutcome>, String> {
    use archivefs_core::identity_source::cache::{IdentityCacheLocation, load_cache};
    use archivefs_core::identity_source::romm::client::UreqTransport;
    use archivefs_core::identity_source::romm::config::ValidatedRommSource;
    use archivefs_core::identity_source::settings::title_token_file;

    let location = settings_location()?;
    let settings = location.load().map_err(|error| error.detail())?;
    let folder = record_folder(location.directory());
    if !settings.source.enabled || !settings.rename_updates_title {
        return Ok(match action {
            RommTitleAction::AfterRename { .. } => None,
            // Turned off since the rename: stay read-only, and say so.
            RommTitleAction::AfterUndo { transaction_id } => archivefs_core::identity_source::romm::title_sync::load_record(&folder, &transaction_id)?
                .map(|_| TitleSyncOutcome::LeftAlone(format!("\"{SETTING_LABEL}\" is off, so EmuWiz did not write to RomM. Change the title back in RomM if you want to."))),
        });
    }
    // Only the separate title-update token, never the read-only one.
    let token = title_token_file(&settings)?;
    let trusted: Vec<PathBuf> = archivefs_core::Config::load_default()
        .map(|config| config.source_folders)
        .map_err(|error| format!("EmuWiz's settings could not be read: {error}"))?;
    let source = ValidatedRommSource::validate(
        &settings.source,
        &token,
        &trusted,
        &archivefs_core::identity_source::net_policy::SystemResolver,
    )
    .map_err(|refusal| refusal.detail())?;
    let transport = UreqTransport::new();
    let cancel = AtomicBool::new(false);
    match action {
        RommTitleAction::AfterRename {
            transaction_id,
            old,
            new,
        } => {
            let root = default_identity_root()?;
            let cache = load_cache(
                &IdentityCacheLocation::new(&root, IdentityProvider::Romm),
                None,
            )
            .map_err(|refusal| {
                format!("EmuWiz's RomM import could not be read ({refusal:?}). Import RomM first.")
            })?;
            // The names Rename History proves this game had before (a game
            // renamed before title updates existed still shows such a title).
            let earlier = archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                .map_err(|error| format!("Rename History is unavailable ({error})"))
                .and_then(|journal| {
                    archivefs_core::dat::rom_organisation::game_rename::earlier_paths_from_history(
                        &journal, &old,
                    )
                });
            update_title_after_rename(
                &source,
                &transport,
                &cache,
                &folder,
                &transaction_id,
                &old,
                &new,
                earlier.as_deref().map_err(String::as_str),
                &cancel,
            )
            .map(Some)
        }
        RommTitleAction::AfterUndo { transaction_id } => {
            restore_title_after_undo(&source, &transport, &folder, &transaction_id, &cancel)
        }
    }
}

impl App {
    /// Starts `action` on a worker. One at a time; later ones wait in turn.
    pub(super) fn start_romm_title(&mut self, action: RommTitleAction) {
        if self.game_rename.romm.is_some() {
            self.game_rename.romm_queued.push_back(action);
            return;
        }
        // Tests never reach the person's own RomM.
        if cfg!(test) {
            return;
        }
        let undo = matches!(action, RommTitleAction::AfterUndo { .. });
        let title = match action {
            RommTitleAction::AfterRename { .. } => "Updating the game's title in RomM",
            RommTitleAction::AfterUndo { .. } => "Putting the game's RomM title back",
        };
        let job = self
            .activity
            .queue(title, super::Route::Section(super::Section::Games), false);
        self.activity.start(job);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(run(action));
        });
        self.game_rename.romm = Some(RommTitleJob {
            job,
            undo,
            receiver,
        });
    }

    /// Collects a finished RomM request into Activity and the rename card.
    pub(super) fn poll_romm_title(&mut self) {
        let Some(running) = &self.game_rename.romm else {
            return;
        };
        let result = match running.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the RomM request stopped unexpectedly".into())
            }
        };
        let RommTitleJob { job, undo, .. } = self.game_rename.romm.take().expect("checked above");
        let (line, error) = finished_line(result, undo);
        match line {
            Some(line) => {
                self.activity.finish(job, line.clone(), error);
                self.game_rename.romm_line = Some(line);
            }
            None => self.activity.finish(
                job,
                "RomM title updates are off; RomM was not contacted.".into(),
                None,
            ),
        }
        if let Some(next) = self.game_rename.romm_queued.pop_front() {
            self.start_romm_title(next);
        }
    }
}

/// The card line and Activity error for a finished request. `None`: nothing
/// was done (title updates off, or nothing to restore).
pub(super) fn finished_line(
    result: Result<Option<TitleSyncOutcome>, String>,
    undo: bool,
) -> (Option<String>, Option<String>) {
    match result {
        Ok(Some(outcome)) => (Some(outcome.line()), None),
        Ok(None) => (None, None),
        // The local rename or Undo stands; only the RomM part failed.
        Err(error) => (
            Some(format!(
                "The {} itself is done, but RomM's title was not changed: {error}",
                if undo { "Undo" } else { "rename" }
            )),
            Some(error),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_romm_failure_never_reads_as_a_failed_rename_or_undo() {
        let (line, error) =
            finished_line(Err("RomM could not be reached (timeout).".into()), false);
        let line = line.unwrap();
        assert!(line.starts_with("The rename itself is done"), "{line}");
        assert!(line.contains("RomM's title was not changed"), "{line}");
        assert_eq!(
            error.as_deref(),
            Some("RomM could not be reached (timeout).")
        );
        let (line, _) = finished_line(Err("refused".into()), true);
        assert!(line.unwrap().starts_with("The Undo itself is done"));
        // Nothing to do: no line at all.
        assert_eq!(finished_line(Ok(None), false), (None, None));
        let (line, error) = finished_line(
            Ok(Some(TitleSyncOutcome::Changed {
                from: "Zelda".into(),
                to: "Legend of Zelda".into(),
            })),
            false,
        );
        assert_eq!(
            line.unwrap(),
            "RomM: its title changed from \"Zelda\" to \"Legend of Zelda\"."
        );
        assert!(error.is_none());
    }

    #[test]
    fn the_setting_explains_the_write_permission_and_the_default() {
        assert!(SETTING_EXPLANATION.starts_with("Off: EmuWiz only reads from RomM."));
        for needed in [
            "roms.write",
            "never identified",
            "History Undo",
            "Nothing else in RomM is changed",
            "keep its file private",
            "its own RomM client token",
            "separate read-only token",
        ] {
            assert!(SETTING_EXPLANATION.contains(needed), "{needed}");
        }
    }
}
