//! The play sessions EmuWiz itself recorded, and the save snapshots it
//! holds. Looking only.
//!
//! Everything shown comes from two places written by EmuWiz: the session
//! journal (one record per launch from EmuWiz, see `session_tracking`) and
//! the snapshot store. Nothing is inferred from an emulator's own history
//! files, from file times or from what is installed. A session that was
//! never recorded is not shown, and a recorded session says only what was
//! observed: it does not claim the game loaded or was played.
//!
//! The page reads and never writes: it does not settle open sessions, take
//! snapshots, restore anything or create the folder it reads.

use super::guidance::GuidanceCategory;
use crate::ui::components::{self as widgets, ActionStyle, StatusTone};
use archivefs_core::save_continuity::session::{
    SaveChange, SaveTracking, SessionEnd, SessionRecord, SessionState, read_recorded_sessions,
};
use archivefs_core::save_continuity::status::Advisory;
use archivefs_core::save_continuity::store::{CaptureReason, SnapshotStore};
use archivefs_core::save_continuity::workflow::{Timeline, TimelineEntry, timelines};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// How many sessions are shown, newest first.
const SESSIONS_SHOWN: usize = 50;

pub(super) struct Loaded {
    pub sessions: Vec<SessionRecord>,
    pub saves: Vec<Timeline>,
}

#[derive(Default)]
pub(super) struct State {
    /// The save-continuity folder. `None` keeps the page inert (tests).
    root: Option<PathBuf>,
    loading: Option<Receiver<Loaded>>,
    loaded: Option<Loaded>,
    error: Option<String>,
}

impl State {
    pub(super) fn live() -> Self {
        Self {
            root: archivefs_core::save_continuity::default_root().ok(),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn with_root(root: &Path) -> Self {
        Self {
            root: Some(root.to_path_buf()),
            ..Self::default()
        }
    }

    fn reload(&mut self, context: &egui::Context) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.loading = Some(receiver);
        self.error = None;
        let context = context.clone();
        let spawned = std::thread::Builder::new()
            .name("emuwiz-save-continuity-page".to_string())
            .spawn(move || {
                let _ = sender.send(load(&root));
                context.request_repaint();
            });
        if spawned.is_err() {
            self.loading = None;
            self.error = Some("EmuWiz could not start reading the recorded sessions.".to_string());
        }
    }

    fn poll(&mut self, context: &egui::Context) {
        if let Some(receiver) = &self.loading {
            match receiver.try_recv() {
                Ok(loaded) => {
                    self.loaded = Some(loaded);
                    self.loading = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.error =
                        Some("Reading the recorded sessions stopped unexpectedly.".to_string());
                }
            }
        } else if self.loaded.is_none() && self.error.is_none() {
            self.reload(context);
        }
    }
}

/// Reads what is recorded under `root`. Creates and changes nothing; a
/// folder that does not exist yet is simply nothing recorded.
pub(super) fn load(root: &Path) -> Loaded {
    Loaded {
        sessions: read_recorded_sessions(root, SESSIONS_SHOWN),
        saves: SnapshotStore::open_existing(root)
            .map(|store| timelines(&store))
            .unwrap_or_default(),
    }
}

fn when(unix: i64) -> String {
    super::save_snapshots::format_unix_seconds(unix.max(0) as u64)
}

pub(super) fn length(seconds: i64) -> String {
    match seconds.max(0) {
        1 => "1 second".to_string(),
        s if s < 60 => format!("{s} seconds"),
        s if s < 120 => "1 minute".to_string(),
        s if s < 3600 => format!("{} minutes", s / 60),
        s => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

/// One fact on one line.
fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(format!("{label}: {value}"));
}

/// How long the emulator process ran, only where both ends were seen. For a
/// session whose end EmuWiz did not see, the recorded end time is when it
/// was found gone, which says nothing about how long it ran.
pub(super) fn ran_for(record: &SessionRecord) -> Option<String> {
    match (&record.state, record.started_unix, record.ended_unix) {
        (SessionState::Ended, Some(started), Some(ended)) => Some(length(ended - started)),
        _ => None,
    }
}

/// The state of a session, in the words shown, and how it is coloured.
/// Only an end that was actually observed to be abnormal is a warning;
/// everything not known is neutral.
pub(super) fn session_status(record: &SessionRecord) -> (String, StatusTone) {
    match (&record.state, &record.end) {
        (SessionState::Requested, _) => (
            "Launch requested; no emulator process recorded yet".to_string(),
            StatusTone::Pending,
        ),
        (SessionState::Started, _) => ("Recorded as still running".to_string(), StatusTone::Active),
        (SessionState::NeverStarted, _) => {
            ("The emulator did not start".to_string(), StatusTone::Info)
        }
        (SessionState::EndNotSeen, _) => (
            "Ended; EmuWiz was not watching, so how is not known".to_string(),
            StatusTone::Pending,
        ),
        (SessionState::Ended, Some(end)) => {
            let tone = match end {
                SessionEnd::Clean => StatusTone::Success,
                end if end.abnormal() => StatusTone::Warning,
                _ => StatusTone::Info,
            };
            (end.label(), tone)
        }
        (SessionState::Ended, None) => ("Ended; how is not known".to_string(), StatusTone::Info),
    }
}

/// What happened to the saves in a session, as recorded.
pub(super) fn save_summary(record: &SessionRecord) -> String {
    if matches!(
        record.state,
        SessionState::Requested | SessionState::Started | SessionState::NeverStarted
    ) {
        return String::new();
    }
    if let Some(SaveTracking::Unavailable { reason }) = &record.save_tracking {
        return format!("Save changes were not tracked for this session: {reason}.");
    }
    if record.saves_after.is_none() {
        return "The saves were not compared after this session.".to_string();
    }
    if record.save_changes.is_empty() {
        return "No save changed in this session.".to_string();
    }
    let changes: Vec<String> = record
        .save_changes
        .iter()
        .map(|change| match change {
            SaveChange::Created { label } => format!("new: {label}"),
            SaveChange::Modified { label } => format!("changed: {label}"),
            SaveChange::Missing { label } => format!("no longer there: {label}"),
            SaveChange::ModifiedBeforeNotKnown { label } => {
                format!("changed (what it held before the session is not known): {label}")
            }
            SaveChange::NotKnown { label } => format!("not known whether it changed: {label}"),
            SaveChange::CreatedBeforeNotKnown { label } => {
                format!("written (not known whether it existed before the session): {label}")
            }
        })
        .collect();
    format!("Saves in this session: {}.", changes.join("; "))
}

fn session_card(ui: &mut egui::Ui, record: &SessionRecord) {
    widgets::full_width_card(ui, |ui| {
        let title = if record.game.title.trim().is_empty() {
            "Game not named in the record"
        } else {
            record.game.title.as_str()
        };
        ui.label(crate::ui::theme::card_title(title));
        let (status, tone) = session_status(record);
        widgets::status_badge(ui, status, tone);
        let emulator = if record.launch.emulator.is_empty() {
            "not identified".to_string()
        } else if record.launch.profile.is_empty() {
            record.launch.emulator.clone()
        } else {
            format!("{} ({})", record.launch.emulator, record.launch.profile)
        };
        row(ui, "Emulator", &emulator);
        row(ui, "Launch requested", &when(record.requested_unix));
        if let Some(time) = ran_for(record) {
            row(ui, "Emulator process ran for", &time);
        }
        // The number as observed, whatever was or was not concluded from it.
        // It belongs to the process EmuWiz started, which may be a launcher.
        if let Some(status) = &record.observed_status {
            use archivefs_core::save_continuity::session::ObservedStatus;
            let text = match status {
                ObservedStatus::ExitCode { code } => format!("exit status {code}"),
                ObservedStatus::Signal { signal, .. } => format!("ended by signal {signal}"),
            };
            row(ui, "Process EmuWiz started", &text);
        }
        let saves = save_summary(record);
        if !saves.is_empty() {
            ui.label(saves);
        }
        if !record.snapshots.is_empty() {
            ui.label(format!(
                "Snapshots taken for this session: {}.",
                record.snapshots.len()
            ));
        }
        for note in &record.notes {
            ui.weak(note);
        }
    });
}

fn reason(reason: &CaptureReason) -> &'static str {
    match reason {
        CaptureReason::Manual => "taken by hand",
        CaptureReason::SessionStart => "before a session",
        CaptureReason::SessionEnd => "after a session",
        CaptureReason::BeforeContinuation => "before a continuation copy",
        CaptureReason::Imported => "received from another device",
    }
}

/// What to say about the session before a snapshot.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SessionNotice {
    /// An abnormal end that was actually observed.
    Warning(String),
    /// Something that is not known: said quietly, never as a warning.
    Quiet(String),
    None,
}

/// Decided from the preceding session alone. How the copy was taken and
/// whether its structure was checked are separate answers with their own
/// badge and lines; they never turn into a warning about the session.
pub(super) fn session_notice(entry: &TimelineEntry) -> SessionNotice {
    let Some(session) = &entry.preceding_session else {
        return SessionNotice::None;
    };
    if let Some(warning) = session.warning() {
        return SessionNotice::Warning(warning);
    }
    match session.uncertainty() {
        Some(text) if session.advisory() == Advisory::Caution => SessionNotice::Quiet(text),
        _ => SessionNotice::None,
    }
}

fn snapshot_card(ui: &mut egui::Ui, entry: &TimelineEntry) {
    widgets::full_width_card(ui, |ui| {
        ui.label(crate::ui::theme::card_title(&format!(
            "{} · {}",
            when(entry.captured_unix),
            reason(&entry.reason)
        )));
        if entry.verified_safe_point {
            widgets::status_badge(ui, "Copied at a verified safe point", StatusTone::Success);
        } else {
            widgets::status_badge(
                ui,
                "Best-effort copy: not verified as safe",
                StatusTone::Pending,
            );
        }
        match session_notice(entry) {
            SessionNotice::Warning(text) => widgets::banner(
                ui,
                "About the session before this snapshot",
                &text,
                StatusTone::Warning,
            ),
            SessionNotice::Quiet(text) => {
                ui.weak(text);
            }
            SessionNotice::None => {}
        }
        // Capture safety, structure and game-level validity are three
        // separate answers; they are shown as three lines, never as one.
        for line in &entry.status_lines {
            ui.label(line);
        }
        for change in &entry.changes {
            ui.weak(change);
        }
    });
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll(ui.ctx());
    super::save_snapshots::tip(
        ui,
        "save-continuity-page",
        GuidanceCategory::Explain,
        "These are the play sessions EmuWiz recorded when a game was launched from EmuWiz, and the save snapshots EmuWiz holds. Nothing here is guessed from an emulator's own history. A recorded session shows that EmuWiz started the emulator for that game; it does not show that the game loaded. This page only looks: nothing is restored or changed from here.",
    );
    ui.horizontal(|ui| {
        let busy = state.loading.is_some();
        if widgets::action_button(
            ui,
            "Refresh",
            ActionStyle::Secondary,
            !busy && state.root.is_some(),
        )
        .clicked()
        {
            state.reload(ui.ctx());
        }
        if busy {
            ui.weak("Reading what was recorded…");
        }
    });
    if let Some(error) = &state.error {
        widgets::banner(ui, "Could not read the records", error, StatusTone::Blocked);
    }
    let Some(loaded) = &state.loaded else {
        if state.root.is_none() {
            widgets::empty_state(
                ui,
                "Nothing to show",
                "The save-continuity folder is not available in this profile.",
                None,
            );
        }
        return;
    };

    widgets::section_header(
        ui,
        "Recorded sessions",
        Some("Newest first. Only launches made from EmuWiz are recorded."),
    );
    if loaded.sessions.is_empty() {
        widgets::empty_state(
            ui,
            "No sessions recorded yet",
            "A session is recorded each time a game is launched from EmuWiz.",
            None,
        );
    }
    for record in &loaded.sessions {
        session_card(ui, record);
    }

    widgets::section_header(
        ui,
        "Save snapshots",
        Some(
            "Per save, newest first. Each snapshot keeps three separate answers: how it was copied, whether its structure was checked, and whether the game's own data was checked.",
        ),
    );
    if loaded.saves.is_empty() {
        widgets::empty_state(
            ui,
            "No snapshots yet",
            "EmuWiz has not taken a snapshot of any save in this profile. Snapshots are never taken or restored automatically.",
            None,
        );
    }
    for save in &loaded.saves {
        ui.label(crate::ui::theme::section_title(&format!(
            "{} · {}",
            save.emulator,
            save.source.display()
        )));
        for entry in save.entries.iter().rev() {
            snapshot_card(ui, entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::save_continuity::session::{
        ExitObservation, SaveMark, SessionGame, SessionJournal, SessionLaunch,
    };

    struct Nothing;
    impl archivefs_core::save_continuity::session::ProcessTable for Nothing {
        fn start_ticks(&self, _pid: u32) -> Option<u64> {
            None
        }
    }

    fn mark(content: &str) -> SaveMark {
        SaveMark {
            save_id: "save-1".into(),
            label: "Slot 1".into(),
            content_sha256: content.into(),
            last_changed_unix: None,
            before_launch: None,
            folder: None,
            path: None,
        }
    }

    fn tree(root: &Path) -> Vec<(PathBuf, u64)> {
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    stack.push(path.clone());
                }
                found.push((path, meta.len()));
            }
        }
        found.sort();
        found
    }

    #[test]
    fn a_profile_with_nothing_recorded_shows_nothing_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("save-continuity");
        let loaded = load(&root);
        assert!(loaded.sessions.is_empty() && loaded.saves.is_empty());
        assert!(!root.exists(), "looking must not create the folder");
    }

    #[test]
    fn only_recorded_sessions_are_shown_and_reading_them_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("save-continuity");
        let journal = SessionJournal::open(&root, "dev").unwrap();
        let game = |title: &str| SessionGame {
            title: title.into(),
            ..Default::default()
        };
        let launch = |emulator: &str, direct: bool| SessionLaunch {
            emulator: emulator.into(),
            profile: "native".into(),
            through_launcher: !direct,
            ..Default::default()
        };
        // One that never started, one still open, one that ended with a
        // status that cannot be interpreted, one that ended normally.
        let never = journal
            .request(game("Never"), launch("PCSX2", true), vec![mark("v1")])
            .unwrap();
        journal
            .mark_never_started(&never.session_id, "the BIOS is missing")
            .unwrap();
        let open = journal
            .request(game("Open"), launch("DuckStation", true), vec![mark("v1")])
            .unwrap();
        journal
            .mark_started(&open.session_id, std::process::id(), &Nothing)
            .unwrap();
        let odd = journal
            .request(game("Odd"), launch("PCSX2", true), vec![mark("v1")])
            .unwrap();
        journal.mark_started(&odd.session_id, 70, &Nothing).unwrap();
        journal
            .mark_ended(&odd.session_id, ExitObservation::Code(139), None, &Nothing)
            .unwrap();
        let clean = journal
            .request(game("Clean"), launch("DuckStation", true), vec![mark("v1")])
            .unwrap();
        journal
            .mark_started(&clean.session_id, 71, &Nothing)
            .unwrap();
        journal
            .mark_ended(
                &clean.session_id,
                ExitObservation::Code(0),
                Some(&[mark("v2")]),
                &Nothing,
            )
            .unwrap();

        let before = tree(&root);
        let loaded = load(&root);
        assert_eq!(tree(&root), before, "the page only reads");
        let titles: Vec<&str> = loaded
            .sessions
            .iter()
            .map(|session| session.game.title.as_str())
            .collect();
        assert_eq!(
            titles,
            ["Clean", "Odd", "Open", "Never"],
            "newest first, nothing else"
        );

        let by = |title: &str| {
            loaded
                .sessions
                .iter()
                .find(|session| session.game.title == title)
                .unwrap()
        };
        // A normal close that was observed.
        let (text, tone) = session_status(by("Clean"));
        assert_eq!(
            (text.as_str(), tone),
            ("Closed normally", StatusTone::Success)
        );
        assert_eq!(
            save_summary(by("Clean")),
            "Saves in this session: changed: Slot 1."
        );
        // 139 that cannot be interpreted: neutral, never a warning or "crash".
        let (text, tone) = session_status(by("Odd"));
        assert_eq!(tone, StatusTone::Info, "{text}");
        assert!(!text.to_lowercase().contains("crash"), "{text}");
        assert_eq!(
            save_summary(by("Odd")),
            "The saves were not compared after this session."
        );
        // Still open: shown as recorded, not settled by the page.
        assert_eq!(session_status(by("Open")).1, StatusTone::Active);
        assert_eq!(by("Open").state, SessionState::Started);
        assert_eq!(save_summary(by("Open")), "");
        assert_eq!(session_status(by("Never")).0, "The emulator did not start");
        // A duration only where both ends were seen.
        assert!(ran_for(by("Clean")).is_some());
        assert_eq!(ran_for(by("Open")), None);
        assert_eq!(ran_for(by("Never")), None);
        let mut unseen = by("Clean").clone();
        unseen.state = SessionState::EndNotSeen;
        assert_eq!(
            ran_for(&unseen),
            None,
            "found gone later is not how long it ran"
        );
        assert_eq!(
            [
                length(1),
                length(59),
                length(60),
                length(119),
                length(120),
                length(3700)
            ],
            [
                "1 second",
                "59 seconds",
                "1 minute",
                "1 minute",
                "2 minutes",
                "1 h 1 min"
            ]
        );
    }

    #[test]
    fn only_the_preceding_session_decides_the_notice_on_a_snapshot() {
        use archivefs_core::save_continuity::status::PrecedingSession;
        use archivefs_core::save_continuity::store::CaptureBoundary;
        // A best-effort copy: its own caution is in `warning`, which is
        // about how the copy was taken, not about any session.
        let entry = |session: Option<PrecedingSession>| TimelineEntry {
            snapshot_id: "s".into(),
            captured_unix: 1,
            device_id: "d".into(),
            emulator: "PCSX2".into(),
            profile: "flatpak".into(),
            source: PathBuf::from("/saves/card.ps2"),
            boundary: CaptureBoundary::StableReadOnly,
            verified_safe_point: false,
            reason: CaptureReason::Manual,
            session_id: None,
            total_bytes: 1,
            saves: Vec::new(),
            changes: Vec::new(),
            structure: None,
            game_integrity: None,
            preceding_session: session,
            advisory: Advisory::Warning,
            warning: Some("Emulator state not confirmed: may have been taken mid-write".into()),
            uncertainty: None,
            status_lines: Vec::new(),
        };
        // Nothing known about the session: said quietly, never a warning.
        let unknown = session_notice(&entry(Some(PrecedingSession::unknown(
            "no session was recorded",
        ))));
        assert!(matches!(unknown, SessionNotice::Quiet(_)), "{unknown:?}");
        // A normal close, or no session information at all: nothing.
        let clean = PrecedingSession::CleanExit {
            session_id: "x".into(),
        };
        assert_eq!(session_notice(&entry(Some(clean))), SessionNotice::None);
        assert_eq!(session_notice(&entry(None)), SessionNotice::None);
        // An abnormal end that was observed: the warning, in its own words.
        let fatal = PrecedingSession::FatalSignal {
            session_id: "x".into(),
            signal: 11,
        };
        match session_notice(&entry(Some(fatal))) {
            SessionNotice::Warning(text) => {
                assert!(text.starts_with("Abnormal termination detected"), "{text}");
                assert!(!text.contains("mid-write"), "{text}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_page_draws_with_and_without_records() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("save-continuity");
        let draw = |state: &mut State| {
            let context = egui::Context::default();
            crate::ui::theme::apply(&context);
            let _ = context.run(Default::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| show(ui, state));
            });
        };
        // Inert (no folder known): draws, starts nothing.
        let mut inert = State::default();
        draw(&mut inert);
        assert!(inert.loading.is_none() && inert.loaded.is_none());
        // Nothing recorded.
        let mut empty = State::with_root(&root);
        empty.loaded = Some(load(&root));
        draw(&mut empty);
        // With a record.
        let journal = SessionJournal::open(&root, "dev").unwrap();
        journal
            .request(
                SessionGame {
                    title: "Drawn".into(),
                    ..Default::default()
                },
                SessionLaunch::default(),
                vec![],
            )
            .unwrap();
        let mut some = State::with_root(&root);
        some.loaded = Some(load(&root));
        assert_eq!(some.loaded.as_ref().unwrap().sessions.len(), 1);
        draw(&mut some);
    }
}
