//! Disposable synthetic filesystem images and trainer sources only.
use super::*;
use serde_json::{Value, json};
use std::fs;

fn adf_bytes() -> Vec<u8> {
    let mut bytes = vec![0u8; 128 * 512];
    bytes[..4].copy_from_slice(b"DOS\x01");
    bytes[8..12].copy_from_slice(&64u32.to_be_bytes());
    let root = &mut bytes[64 * 512..65 * 512];
    for (offset, value) in [(0, 2u32), (12, 72), (508, 1)] {
        root[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    root[0x1b0] = 7;
    root[0x1b1..0x1b8].copy_from_slice(b"Fixture");
    let sum = root
        .chunks_exact(4)
        .map(|s| u32::from_be_bytes(s.try_into().unwrap()))
        .fold(0u32, u32::wrapping_add);
    root[20..24].copy_from_slice(&sum.wrapping_neg().to_be_bytes());
    bytes
}
struct Fixture {
    root: tempfile::TempDir,
    game: PathBuf,
    source: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game.adf");
        let source = root.path().join("trainers.json");
        fs::write(&game, adf_bytes()).unwrap();
        let f = Self { root, game, source };
        f.write_entries(vec![f.entry(9)]);
        f
    }
    fn entry(&self, value: u32) -> Value {
        let media = inspect_adf_media(&self.game, None).unwrap();
        json!({"platform":"Amiga", "title":format!("Lives {value}"), "description":"Synthetic, not a real game trainer", "mechanism":"memory_write",
            "target":{"media":[{"sha256":media.sha256,"disk":1}],"scope":"disk","disk":1},
            "writes":[{"width":"word","address":"0x1A2B4","value":value,"original":3,"timing":"every_frame"}],
            "source":{"name":"Synthetic author","reference":"https://example.invalid/synthetic"}})
    }
    fn write_entries(&self, entries: Vec<Value>) {
        fs::write(
            &self.source,
            json!({"schema":AMIGA_TRAINER_SCHEMA,"entries":entries}).to_string(),
        )
        .unwrap();
    }
    fn load(&self) -> Loaded {
        load(&self.game, "Synthetic game", &self.source).unwrap()
    }
    fn preview(
        &self,
        loaded: &Loaded,
        selected: &[usize],
        emulator: PreviewEmulator,
    ) -> Result<String, String> {
        preview(
            loaded,
            "Synthetic game",
            &self.source,
            &BTreeMap::new(),
            &selected.iter().copied().collect(),
            emulator,
        )
    }
}

#[test]
fn exact_image_preview_has_provenance_and_preserves_disk_identity_and_launch_settings() {
    let f = Fixture::new();
    let config = f.root.path().join("existing.fs-uae");
    let settings =
        b"[fs-uae]\namiga_model=A1200\nfloppy_drive_0=original.adf\njoystick_port_1=custom\n";
    fs::write(&config, settings).unwrap();
    let original = fs::read(&f.game).unwrap();
    let source = fs::read(&f.source).unwrap();
    let loaded = f.load();
    let evidence = loaded.media.identity_facts();
    let text = f.preview(&loaded, &[0], PreviewEmulator::FsUae).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["mode"], "preview_only");
    assert_eq!(report["execution_enabled"], false);
    assert_eq!(report["plan"]["media_sha256"], loaded.media.sha256);
    assert_eq!(report["source_sha256"], loaded.import.source_sha256);
    assert_eq!(report["plan"]["selected"], json!([0]));
    assert_eq!(report["trainers"].as_array().unwrap().len(), 1);
    assert_eq!(fs::read(&f.game).unwrap(), original);
    assert_eq!(fs::read(&f.source).unwrap(), source);
    assert_eq!(fs::read(config).unwrap(), settings);
    assert_eq!(loaded.media.identity_facts(), evidence);
}

#[test]
fn every_emulator_remains_explicitly_preview_only_with_no_launch_artifact() {
    let f = Fixture::new();
    let loaded = f.load();
    for emulator in [
        PreviewEmulator::FsUae,
        PreviewEmulator::WinUae,
        PreviewEmulator::Amiberry,
        PreviewEmulator::Other,
    ] {
        let report: Value =
            serde_json::from_str(&f.preview(&loaded, &[0], emulator).unwrap()).unwrap();
        assert_eq!(report["execution_enabled"], false);
        assert_eq!(report["emulator"], emulator.label());
        assert!(
            report["runtime_blocker"]
                .as_str()
                .unwrap()
                .starts_with("Preview only.")
        );
        assert!(report.get("command").is_none());
        assert!(report.get("launch_arguments").is_none());
    }
}

#[test]
fn selection_is_explicit_and_invalid_indexes_or_wrong_image_never_preview() {
    let f = Fixture::new();
    let loaded = f.load();
    assert!(f.preview(&loaded, &[], PreviewEmulator::FsUae).is_err());
    assert!(f.preview(&loaded, &[99], PreviewEmulator::FsUae).is_err());
    assert!(AdfTrainerState::default().selected.is_empty());
    let mut wrong = f.entry(9);
    wrong["target"]["media"][0]["sha256"] = json!("0".repeat(64));
    f.write_entries(vec![wrong]);
    let loaded = f.load();
    assert!(loaded.plan.selected.is_empty());
    assert!(f.preview(&loaded, &[0], PreviewEmulator::FsUae).is_err());
}

#[test]
fn canonical_conflicts_require_review_and_never_select_both_accidentally() {
    let f = Fixture::new();
    // Canonical conflicts are alternate values for the same named effect.
    // Distinct titles are separate canonical effects, even at one address.
    let first = f.entry(9);
    let mut second = f.entry(10);
    second["title"] = first["title"].clone();
    f.write_entries(vec![first, second]);
    let mut loaded = f.load();
    assert!(loaded.plan.selected.is_empty());
    assert!(f.preview(&loaded, &[0], PreviewEmulator::FsUae).is_err());
    let group = loaded.plan.conflict_groups[0].group_index;
    for (choice, selected) in [(CheatReviewChoice::KeepA, 0), (CheatReviewChoice::KeepB, 1)] {
        let choices = BTreeMap::from([(group, choice)]);
        replan(&mut loaded, &choices);
        assert_eq!(loaded.plan.selected, vec![selected]);
        assert!(
            preview(
                &loaded,
                "Synthetic game",
                &f.source,
                &choices,
                &BTreeSet::from([selected]),
                PreviewEmulator::FsUae
            )
            .is_ok()
        );
        assert!(
            preview(
                &loaded,
                "Synthetic game",
                &f.source,
                &choices,
                &BTreeSet::from([0, 1]),
                PreviewEmulator::FsUae
            )
            .is_err()
        );
    }
}

#[test]
fn malformed_records_are_visible_and_commands_or_overflow_do_not_import() {
    let f = Fixture::new();
    let mut overflow = f.entry(9);
    overflow["writes"][0]["value"] = json!(65536);
    let mut command = f.entry(9);
    command["command"] = json!("do-not-run");
    f.write_entries(vec![overflow, command, f.entry(9)]);
    let loaded = f.load();
    assert_eq!(loaded.import.rejected, 2);
    assert_eq!(loaded.import.diagnostics.len(), 2);
    assert_eq!(loaded.plan.selected, vec![2]);
    assert!(f.preview(&loaded, &[0], PreviewEmulator::FsUae).is_err());
    assert!(f.preview(&loaded, &[2], PreviewEmulator::FsUae).is_ok());
    fs::write(&f.source, b"not JSON").unwrap();
    assert!(load(&f.game, "Synthetic game", &f.source).is_err());
}

#[test]
fn changed_source_or_disk_invalidates_the_review() {
    let f = Fixture::new();
    let loaded = f.load();
    f.write_entries(vec![f.entry(10)]);
    assert!(f.preview(&loaded, &[0], PreviewEmulator::FsUae).is_err());
    let loaded = f.load();
    let mut bytes = fs::read(&f.game).unwrap();
    bytes[10000] = 42;
    fs::write(&f.game, bytes).unwrap();
    assert!(f.preview(&loaded, &[0], PreviewEmulator::FsUae).is_err());
}

#[test]
fn whdload_and_opaque_media_are_distinct_and_fake_adfs_are_rejected() {
    let f = Fixture::new();
    for extension in ["slave", "lha", "ipf", "dms", "hdf"] {
        let path = f.root.path().join(format!("game.{extension}"));
        fs::write(&path, adf_bytes()).unwrap();
        assert!(!is_candidate(&path));
        assert!(load(&path, "Synthetic game", &f.source).is_err());
    }
    fs::write(&f.game, b"Acorn image, not an AmigaDOS floppy").unwrap();
    assert!(load(&f.game, "Synthetic game", &f.source).is_err());
}

#[test]
fn alternate_media_and_whole_title_claims_are_not_promoted_by_the_gui() {
    let f = Fixture::new();
    let alternate = load(&f.game, "Synthetic [t +2]", &f.source).unwrap();
    assert!(alternate.plan.selected.is_empty());
    let mut whole = f.entry(9);
    whole["target"]["scope"] = json!("whole_title");
    whole["target"].as_object_mut().unwrap().remove("disk");
    f.write_entries(vec![whole]);
    let loaded = f.load();
    assert!(loaded.plan.selected.is_empty());
}

#[test]
fn asynchronous_load_starts_unselected_and_never_enables_anything() {
    let f = Fixture::new();
    let ctx = egui::Context::default();
    let mut state = AdfTrainerState {
        source: f.source.display().to_string(),
        ..Default::default()
    };
    state.start_load(f.game.clone(), "Synthetic game".into(), ctx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.worker.is_some() && std::time::Instant::now() < deadline {
        state.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(state.worker.is_none());
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(state.loaded.is_some());
    assert!(state.selected.is_empty());
    assert!(state.preview.is_none());
}

fn texts(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn add(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                out.push_str(t.galley.job.text.as_str());
                out.push('\n');
            }
            egui::epaint::Shape::Vec(v) => {
                for s in v {
                    add(s, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    for s in shapes {
        add(&s.shape, &mut out);
    }
    out
}

#[test]
fn real_panel_shows_provenance_runtime_blocker_and_disabled_enable() {
    let f = Fixture::new();
    let mut state = AdfTrainerState {
        source: f.source.display().to_string(),
        loaded: Some(f.load()),
        ..Default::default()
    };
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1600.0, 2000.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .show(ctx, |ui| show(ui, &f.game, "Synthetic game", &mut state));
    });
    let text = texts(&output.shapes);
    for expected in [
        "Amiga ADF trainers",
        "Preview only",
        "WHDLoad CUSTOM switches are separate",
        "SHA-256",
        "Synthetic author",
        "Enable in emulator",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(state.selected.is_empty());
    assert!(state.preview.is_none());
}

#[test]
fn bounded_import_reports_dropped_entries_instead_of_silently_hiding_them() {
    let f = Fixture::new();
    let mut entries = vec![json!({}); MAX_TRAINERS_PER_FILE + 1];
    entries[0] = f.entry(9);
    f.write_entries(entries);
    let loaded = f.load();
    assert_eq!(loaded.import.dropped_over_limit, 1);
    let report: Value =
        serde_json::from_str(&f.preview(&loaded, &[0], PreviewEmulator::FsUae).unwrap()).unwrap();
    assert_eq!(report["import_summary"]["dropped_over_limit"], 1);
    assert_eq!(report["import_summary"]["accepted"], 1);
}
