use super::*;
use crate::daedalus::history::HISTORY_DIRECTORY;
use crate::daedalus::notice::NoticeLevel;
use crate::daedalus::test_fixtures as fixtures;
use std::time::UNIX_EPOCH;

struct Scene {
    dir: tempfile::TempDir,
    settings: Vec<(PathBuf, ConfigLayer)>,
}

impl Scene {
    /// A home folder with a RetroArch settings file in it, and a data
    /// folder for the history.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("home/alexandra-q/.config/retroarch");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        let settings = config.join("retroarch.cfg");
        std::fs::write(&settings, "video_driver = \"vulkan\"\n").unwrap();
        Self {
            dir,
            settings: vec![(settings, ConfigLayer::Main)],
        }
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home/alexandra-q")
    }

    fn redactor(&self) -> Redactor {
        Redactor::new(self.home().to_str(), Some("alexandra-q"))
    }

    fn store(&self) -> HistoryStore {
        HistoryStore::at(self.dir.path().join("data").join(HISTORY_DIRECTORY))
    }

    fn launch<'a>(&'a self, code: i32, output: &'a [u8]) -> FinishedLaunch<'a> {
        FinishedLaunch {
            emulator: "retroarch",
            emulator_display: "RetroArch",
            exit: ExitOutcome::Exited { code },
            duration: Some(Duration::from_millis(1_400)),
            launcher: None,
            output: Some(output),
            settings: &self.settings,
            game_file: None,
            core: None,
            save_directory: None,
            environment: &[],
            identity: LaunchIdentity {
                library_id: Some(7),
                title: Some("Tetris"),
                platform: Some("gb"),
                emulator: Some("retroarch"),
                core: Some("gambatte_libretro"),
                emulator_version: Some("1.21.0"),
            },
        }
    }
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn shown(notice: &LaunchNotice) -> String {
    let warning = notice
        .save_warning
        .as_ref()
        .map_or_else(String::new, |warning| {
            format!(
                "{}\n{}\n{}",
                warning.headline, warning.explanation, warning.next_step
            )
        });
    format!(
        "{}\n{}\n{}\n{}\n{warning}",
        notice.headline,
        notice.lines.join("\n"),
        notice.next_step.clone().unwrap_or_default(),
        notice.save_coverage.clone().unwrap_or_default()
    )
}

#[test]
fn a_failed_launch_is_diagnosed_recorded_and_shown_without_private_details() {
    let scene = Scene::new();
    let output = format!(
        "[INFO] [Config]: Loading config: \"{}/.config/retroarch/retroarch.cfg\".\n{}",
        scene.home().display(),
        fixtures::RA_VULKAN_NO_DRIVER
    );
    let store = scene.store();
    let diagnosis = diagnose_finished_launch(
        &scene.launch(1, output.as_bytes()),
        Some(&store),
        &scene.redactor(),
        now(),
    );
    assert_eq!(diagnosis.notice.level, NoticeLevel::LaunchFailed);
    assert!(matches!(diagnosis.recorded, SaveOutcome::Saved { .. }));
    // EmuWiz watched this launch: the exit is measured, not supplied.
    assert!(!diagnosis.report.launch_was_supplied());

    // In memory the report keeps the real path, which freshness checks
    // need. What is shown and what is stored do not.
    let in_memory = serde_json::to_string(&diagnosis.report).unwrap();
    assert!(in_memory.contains("alexandra-q"));
    assert!(!shown(&diagnosis.notice).contains("alexandra-q"));
    let SaveOutcome::Saved { id, .. } = &diagnosis.recorded else {
        unreachable!()
    };
    let stored = std::fs::read_to_string(store.directory().join(format!("{id}.json"))).unwrap();
    assert!(!stored.contains("alexandra"), "{stored}");
    assert!(!stored.contains("Vulkan instance"), "{stored}");
    assert_eq!(store.latest_for_game(7, now()).unwrap().id, *id);
}

#[test]
fn a_launch_that_went_well_records_nothing_and_stays_quiet() {
    let scene = Scene::new();
    let store = scene.store();
    let diagnosis = diagnose_finished_launch(
        &scene.launch(0, fixtures::RA_SAVE_OK.as_bytes()),
        Some(&store),
        &scene.redactor(),
        now(),
    );
    assert_eq!(diagnosis.notice.level, NoticeLevel::Quiet);
    assert_eq!(
        diagnosis.recorded,
        SaveOutcome::NotSaved(NotSaved::NothingToRecord)
    );
    // Not even the history folder is made for an ordinary launch.
    assert!(!store.directory().exists());
    assert!(diagnosis.notice.save_coverage.is_some());
}

#[test]
fn a_failed_save_is_recorded_and_shown_although_the_launch_went_well() {
    let scene = Scene::new();
    let store = scene.store();
    let diagnosis = diagnose_finished_launch(
        &scene.launch(0, fixtures::RA_SAVE_DIR_READ_ONLY.as_bytes()),
        Some(&store),
        &scene.redactor(),
        now(),
    );
    assert_eq!(diagnosis.notice.level, NoticeLevel::SaveWriteFailed);
    assert!(diagnosis.saves.is_write_failure());
    let SaveOutcome::Saved { id, .. } = &diagnosis.recorded else {
        panic!("not recorded: {:?}", diagnosis.recorded)
    };
    let record = store.load(id, now()).unwrap();
    assert!(record.saves.assessment().is_write_failure());
    assert_eq!(
        record.launch,
        crate::daedalus::rules::LaunchOutcome::NoFailureObserved
    );
}

#[test]
fn a_history_that_is_missing_or_unusable_changes_nothing_else() {
    let scene = Scene::new();
    let output = fixtures::RA_VULKAN_NO_DRIVER.as_bytes();
    let with_store = diagnose_finished_launch(
        &scene.launch(1, output),
        Some(&scene.store()),
        &scene.redactor(),
        now(),
    );

    // No store at all.
    let without =
        diagnose_finished_launch(&scene.launch(1, output), None, &scene.redactor(), now());
    assert_eq!(without.notice, with_store.notice);
    assert!(matches!(
        without.recorded,
        SaveOutcome::NotSaved(NotSaved::StorageUnavailable { .. })
    ));

    // A store whose folder is a plain file: refused, and the file is intact.
    let blocked = scene.dir.path().join("data").join("blocked");
    std::fs::write(&blocked, "not a folder").unwrap();
    let unusable = diagnose_finished_launch(
        &scene.launch(1, output),
        Some(&HistoryStore::at(&blocked)),
        &scene.redactor(),
        now(),
    );
    assert_eq!(unusable.notice, with_store.notice);
    assert!(matches!(
        unusable.recorded,
        SaveOutcome::NotSaved(NotSaved::StorageUnavailable { .. })
    ));
    assert_eq!(std::fs::read_to_string(&blocked).unwrap(), "not a folder");

    // Recording switched off: diagnosed and shown, not kept.
    let off = scene.store();
    off.set_enabled(false).unwrap();
    let switched_off = diagnose_finished_launch(
        &scene.launch(1, output),
        Some(&off),
        &scene.redactor(),
        now(),
    );
    assert_eq!(switched_off.notice, with_store.notice);
    assert_eq!(
        switched_off.recorded,
        SaveOutcome::NotSaved(NotSaved::Disabled)
    );
}

#[test]
fn no_output_at_all_is_still_a_diagnosis_and_says_saves_were_not_checked() {
    let scene = Scene::new();
    let mut launch = scene.launch(0, b"");
    launch.output = None;
    let diagnosis = diagnose_finished_launch(&launch, None, &scene.redactor(), now());
    assert_eq!(diagnosis.notice.level, NoticeLevel::Quiet);
    assert_eq!(
        diagnosis.notice.save_coverage.as_deref(),
        Some("Save check: not available for this launch.")
    );
}
