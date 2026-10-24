//! Read-only bridge from a selected MAME machine to its original controls.
//! The explicitly selected catalogue is resolved and parsed on a worker; no
//! ROM files or emulator/controller settings are inspected or changed.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::{collections::BTreeMap, sync::Arc};

use archivefs_core::{
    dat::{
        catalogue_selection::{CatalogueProvenance, CatalogueRef, resolve_catalogue},
        mame_input_metadata::MameInputMetadata,
        model::{DatEcosystem, DatGameEntry},
    },
    game_identity::{GameIdentityReport, IdentityKind, IdentityPlatform, IdentityStatus},
    mame_collection_analyser::inspection::SourceStamp,
    mame_input_requirements::{
        ArcadeInputRequirement, ArcadeInputRequirementFamily, ArcadeInputRequirements,
    },
};
use eframe::egui;

use super::mame_collection_health::HealthPage;
use crate::dat_catalogue_picker::{
    CatalogueInventorySnapshot, DatCatalogueWorkflow as PickerWorkflow,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestKey {
    game_id: i64,
    machine: String,
    catalogue: String,
}

#[derive(Clone, Debug)]
struct CatalogueCache {
    catalogue_token: String,
    catalogue_name: String,
    provenance: String,
    version: Option<String>,
    machine_inputs: BTreeMap<String, Vec<Option<MameInputMetadata>>>,
    source_stamp: SourceStamp,
}

#[derive(Clone, Debug)]
struct Evidence {
    catalogue_name: String,
    provenance: String,
    version: Option<String>,
    requirements: Option<ArcadeInputRequirements>,
    source_stamp: SourceStamp,
}

enum Message {
    Loaded(u64, RequestKey, Result<Arc<CatalogueCache>, String>),
}

#[derive(Default)]
pub(super) struct State {
    generation: u64,
    key: Option<RequestKey>,
    result: Option<Evidence>,
    cache: Option<Arc<CatalogueCache>>,
    receiver: Option<Receiver<Message>>,
    error: Option<String>,
}

impl State {
    /// Returns true when the user asks to choose a catalogue in MAME Collection Health.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        game_id: i64,
        identity: Option<&GameIdentityReport>,
        health: &HealthPage,
    ) -> bool {
        let Some(machine) = verified_machine(identity) else {
            self.invalidate();
            egui::CollapsingHeader::new("Arcade controls")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(
                        "A single verified MAME machine identity is not available for this game.",
                    );
                    ui.label(
                        "Original controls are unknown until machine identity is established.",
                    );
                    show_runtime_unknown(ui);
                });
            return false;
        };
        let (Some(reference), Some(snapshot)) = (health.selected.clone(), health.picker.snapshot())
        else {
            self.invalidate();
            return show_setup(
                ui,
                "Choose a current MAME arcade catalogue to view original controls.",
            );
        };
        if !health
            .picker
            .is_usable(PickerWorkflow::MameHealth, &reference)
        {
            self.invalidate();
            return show_setup(
                ui,
                "The selected MAME catalogue is unavailable or no longer current. Choose it again.",
            );
        }
        let catalogue_name = health
            .picker
            .summaries()
            .iter()
            .find(|summary| summary.reference == reference)
            .map(|summary| summary.display_name.clone())
            .unwrap_or_else(|| "Selected MAME catalogue".into());
        let Some(catalogue_path) = health
            .picker
            .summaries()
            .iter()
            .find(|summary| summary.reference == reference)
            .and_then(|summary| summary.technical_path.clone())
        else {
            self.invalidate();
            return show_setup(
                ui,
                "The selected catalogue has no readable source path. Choose another catalogue.",
            );
        };
        let key = RequestKey {
            game_id,
            machine: machine.clone(),
            catalogue: reference.token(),
        };
        if self.key.as_ref() != Some(&key) {
            let cache_is_current = self.cache.as_ref().is_some_and(|cache| {
                cache.catalogue_token == key.catalogue
                    && cache.source_stamp.path == catalogue_path
                    && cache.source_stamp.still_matches()
            });
            self.key = Some(key.clone());
            self.result = None;
            self.error = None;
            if cache_is_current {
                self.project_cached(&key);
            } else {
                self.cache = None;
                self.start(key, reference, snapshot, catalogue_name, catalogue_path);
            }
        }
        self.poll();
        if self
            .result
            .as_ref()
            .is_some_and(|evidence| !evidence.source_stamp.still_matches())
        {
            self.result = None;
            self.error =
                Some("The selected catalogue changed after it was read. Choose it again.".into());
        }
        let mut open_setup = false;
        ui.collapsing("Arcade controls", |ui| {
            if self.receiver.is_some() {
                ui.horizontal(|ui| { ui.spinner(); ui.label("Reading selected arcade catalogue…"); });
            } else if let Some(error) = &self.error {
                ui.label(error);
                if ui.button("Open MAME catalogue setup").clicked() {
                    open_setup = true;
                }
            } else if let Some(evidence) = &self.result {
                ui.label(format!("Machine: {}", self.key.as_ref().map_or("unknown", |key| key.machine.as_str())));
                if let Some(version) = &evidence.version {
                    ui.label(format!("Catalogue version: {version}"));
                }
                ui.label(format!("Catalogue: {} · {}", evidence.catalogue_name, evidence.provenance));
                match &evidence.requirements {
                    Some(requirements) => {
                        if let Some(players) = &requirements.supported_players {
                            ui.label(format!("MAME lists support for up to {players} players."));
                        }
                        if requirements.requirements.is_empty() {
                            ui.label("No machine control entries are documented; input details are incomplete.");
                        } else {
                            for requirement in &requirements.requirements {
                                ui.label(control_label(requirement));
                            }
                            ui.weak("These are the original machine controls.");
                        }
                    }
                    _ => {
                        ui.label("This catalogue entry has no input metadata. Original controls are unknown.");
                    }
                }
            } else {
                ui.label("Original input evidence is not available.");
            }
            show_runtime_unknown(ui);
        });
        if self.receiver.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if open_setup {
            self.invalidate();
        }
        open_setup
    }

    fn start(
        &mut self,
        key: RequestKey,
        reference: CatalogueRef,
        snapshot: CatalogueInventorySnapshot,
        catalogue_name: String,
        catalogue_path: std::path::PathBuf,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.key = Some(key.clone());
        self.result = None;
        self.error = None;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let usable = snapshot;
            let result = (|| {
                let source_stamp = SourceStamp::observe(&catalogue_path).map_err(|_| {
                    "The selected catalogue could not be freshness-checked.".to_string()
                })?;
                let inputs = usable.inputs();
                let resolved = resolve_catalogue(&reference, inputs).map_err(|error| {
                    format!("The selected catalogue could not be validated: {error}")
                })?;
                if resolved.ecosystem != DatEcosystem::MAMEArcade {
                    return Err("The selected source is not a MAME arcade catalogue.".into());
                }
                if resolved.backing_path != catalogue_path {
                    return Err("The selected catalogue source changed. Choose it again.".into());
                }
                let machine_inputs = index_machine_inputs(&resolved.parsed.games);
                if !source_stamp.still_matches() {
                    return Err(
                        "The catalogue changed while it was being read. Choose it again.".into(),
                    );
                }
                Ok(Arc::new(CatalogueCache {
                    catalogue_token: key.catalogue.clone(),
                    catalogue_name,
                    provenance: provenance_label(&resolved.provenance),
                    version: resolved.parsed.source.version,
                    machine_inputs,
                    source_stamp,
                }))
            })();
            let _ = sender.send(Message::Loaded(generation, key, result));
        });
    }

    fn poll(&mut self) {
        let Some(receiver) = self.receiver.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Message::Loaded(generation, key, result)) => {
                self.receiver = None;
                if generation != self.generation || self.key.as_ref() != Some(&key) {
                    return;
                }
                match result {
                    Ok(cache) => {
                        self.cache = Some(cache);
                        self.project_cached(&key);
                    }
                    Err(error) => {
                        self.result = None;
                        self.error = Some(error);
                    }
                }
            }
            Err(TryRecvError::Disconnected) => {
                self.receiver = None;
                self.error = Some("Catalogue reading stopped before producing a result.".into());
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.key = None;
        self.result = None;
        self.receiver = None;
        self.error = None;
    }

    fn project_cached(&mut self, key: &RequestKey) {
        let Some(cache) = self.cache.as_ref() else {
            self.error = Some("Catalogue input index is not available.".into());
            return;
        };
        let Some(entries) = cache.machine_inputs.get(&key.machine) else {
            self.error =
                Some("The selected catalogue does not contain this verified machine.".into());
            return;
        };
        if entries.len() != 1 {
            self.error = Some(
                "The selected catalogue contains an ambiguous duplicate machine entry.".into(),
            );
            return;
        }
        self.result = Some(Evidence {
            catalogue_name: cache.catalogue_name.clone(),
            provenance: cache.provenance.clone(),
            version: cache.version.clone(),
            requirements: entries[0].as_ref().map(ArcadeInputRequirements::from_mame),
            source_stamp: cache.source_stamp.clone(),
        });
        self.error = None;
    }
}

fn index_machine_inputs(
    games: &[DatGameEntry],
) -> BTreeMap<String, Vec<Option<MameInputMetadata>>> {
    let mut index = BTreeMap::new();
    for game in games {
        index
            .entry(game.name.clone())
            .or_insert_with(Vec::new)
            .push(game.mame_input.clone());
    }
    index
}

fn verified_machine(identity: Option<&GameIdentityReport>) -> Option<String> {
    let identity = identity.filter(|identity| identity.platform == IdentityPlatform::Arcade)?;
    let names: Vec<_> = identity
        .evidence
        .iter()
        .filter(|evidence| {
            evidence.kind == IdentityKind::MameMachineName
                && evidence.status == IdentityStatus::Verified
        })
        .filter_map(|evidence| evidence.value.as_deref())
        .collect();
    (names.len() == 1)
        .then(|| names[0].to_string())
        .filter(|name| !name.trim().is_empty())
}

fn show_setup(ui: &mut egui::Ui, message: &str) -> bool {
    let mut open = false;
    egui::CollapsingHeader::new("Arcade controls")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(message);
            show_runtime_unknown(ui);
            if ui.button("Choose MAME catalogue").clicked() {
                open = true;
            }
        });
    open
}

fn show_runtime_unknown(ui: &mut egui::Ui) {
    ui.weak("Connected controller mapping and compatibility: Unknown.");
}

fn control_label(requirement: &ArcadeInputRequirement) -> String {
    let mut label = match requirement.family {
        ArcadeInputRequirementFamily::DigitalDirections => "Directional controls".to_string(),
        ArcadeInputRequirementFamily::Digital2Way => "2-way joystick / directions".to_string(),
        ArcadeInputRequirementFamily::Digital4Way => "4-way joystick".to_string(),
        ArcadeInputRequirementFamily::Digital8Way => "8-way joystick".to_string(),
        ArcadeInputRequirementFamily::DirectionalOther => "Other directional control".to_string(),
        ArcadeInputRequirementFamily::Buttons => requirement
            .buttons
            .as_ref()
            .map_or("Buttons".into(), |n| format!("{n} buttons")),
        ArcadeInputRequirementFamily::DualStick => "Dual sticks".into(),
        ArcadeInputRequirementFamily::AnalogAxis => requirement
            .raw_control_type
            .as_deref()
            .map_or("Analog axis".into(), |raw| {
                format!("Analog control ({raw})")
            }),
        ArcadeInputRequirementFamily::Pedal => "Pedals".into(),
        ArcadeInputRequirementFamily::RelativePointer => "Relative pointer".into(),
        ArcadeInputRequirementFamily::AbsolutePointer => "Absolute pointer".into(),
        ArcadeInputRequirementFamily::LightGun => "Light gun".into(),
        ArcadeInputRequirementFamily::Keyboard => "Keyboard".into(),
        ArcadeInputRequirementFamily::Mouse => "Mouse".into(),
        ArcadeInputRequirementFamily::Trackball => "Trackball".into(),
        ArcadeInputRequirementFamily::DialSpinner => "Dial / spinner".into(),
        ArcadeInputRequirementFamily::PositionalControl => "Positional control".into(),
        ArcadeInputRequirementFamily::SpecialPanel => "Special control panel".into(),
        ArcadeInputRequirementFamily::Unknown => requirement
            .raw_control_type
            .as_ref()
            .map_or("Control type unknown".into(), |raw| {
                format!("MAME control: {raw} (classification unavailable)")
            }),
    };
    if let Some(buttons) = requirement
        .buttons
        .as_deref()
        .filter(|_| requirement.family != ArcadeInputRequirementFamily::Buttons)
    {
        label.push_str(&format!(" · {buttons} buttons"));
    }
    if let Some(player) = &requirement.player {
        label.push_str(&format!(" · Player {player}"));
    }
    label
}

fn provenance_label(provenance: &CatalogueProvenance) -> String {
    match provenance {
        CatalogueProvenance::UserRegistered => "registered local source".into(),
        CatalogueProvenance::NoIntroPackProjection => "No-Intro projection".into(),
        CatalogueProvenance::TosecReleasePackProjection { pack_id } => {
            format!("TOSEC release pack {pack_id}")
        }
        CatalogueProvenance::EmuWizManaged { provider } => {
            format!("EmuWiz managed {provider:?} source")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::game_identity::{
        IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityProvenance,
    };
    use std::path::PathBuf;

    fn identity(names: &[(&str, IdentityStatus)]) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: PathBuf::from("/tmp/game.zip"),
            platform: IdentityPlatform::Arcade,
            format: IdentityImageFormat::Unsupported,
            evidence: names
                .iter()
                .map(|(name, status)| IdentityEvidence {
                    kind: IdentityKind::MameMachineName,
                    status: *status,
                    value: Some((*name).into()),
                    confidence: IdentityConfidence::StructuredMetadata,
                    provenance: IdentityProvenance {
                        archive_path: PathBuf::from("/tmp/game.zip"),
                        member_path: None,
                        member_index: None,
                        method: "fixture".into(),
                    },
                    diagnostic: String::new(),
                })
                .collect(),
            warnings: vec![],
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    #[test]
    fn accepts_only_one_verified_machine_name() {
        assert_eq!(
            verified_machine(Some(&identity(&[("pacman", IdentityStatus::Verified)]))).as_deref(),
            Some("pacman")
        );
        assert!(verified_machine(None).is_none());
        assert!(
            verified_machine(Some(&identity(&[("pacman", IdentityStatus::Candidate)]))).is_none()
        );
        assert!(
            verified_machine(Some(&identity(&[
                ("pacman", IdentityStatus::Verified),
                ("galaga", IdentityStatus::Verified)
            ])))
            .is_none()
        );
        assert!(
            verified_machine(Some(&identity(&[
                ("pacman", IdentityStatus::Verified),
                ("pacman", IdentityStatus::Verified),
            ])))
            .is_none()
        );
    }

    #[test]
    fn unknown_controls_and_runtime_compatibility_are_not_promoted() {
        assert_eq!(
            control_label(&ArcadeInputRequirement {
                family: ArcadeInputRequirementFamily::Unknown,
                raw_control_type: Some("future".into()),
                ..Default::default()
            }),
            "MAME control: future (classification unavailable)"
        );
        assert_eq!(
            control_label(&ArcadeInputRequirement {
                family: ArcadeInputRequirementFamily::Trackball,
                ..Default::default()
            }),
            "Trackball"
        );
        for (family, label) in [
            (ArcadeInputRequirementFamily::Digital8Way, "8-way joystick"),
            (ArcadeInputRequirementFamily::LightGun, "Light gun"),
            (ArcadeInputRequirementFamily::DialSpinner, "Dial / spinner"),
            (ArcadeInputRequirementFamily::Pedal, "Pedals"),
            (ArcadeInputRequirementFamily::AnalogAxis, "Analog axis"),
        ] {
            assert_eq!(
                control_label(&ArcadeInputRequirement {
                    family,
                    ..Default::default()
                }),
                label
            );
        }
    }

    #[test]
    fn absent_and_duplicate_catalogue_entries_fail_closed() {
        assert!(!index_machine_inputs(&[]).contains_key("pacman"));
        let duplicate = vec![
            DatGameEntry {
                name: "pacman".into(),
                ..Default::default()
            },
            DatGameEntry {
                name: "pacman".into(),
                ..Default::default()
            },
        ];
        assert_eq!(index_machine_inputs(&duplicate)["pacman"].len(), 2);
        let no_input = [DatGameEntry {
            name: "pacman".into(),
            ..Default::default()
        }];
        assert!(index_machine_inputs(&no_input)["pacman"][0].is_none());
    }

    #[test]
    fn exact_machine_projection_keeps_documented_players_joystick_and_buttons() {
        let dir = tempfile::tempdir().unwrap();
        let catalogue_path = dir.path().join("list.xml");
        std::fs::write(&catalogue_path, "unchanged fixture").unwrap();
        let input = MameInputMetadata {
            players: Some("2".into()),
            controls: vec![archivefs_core::dat::MameControlMetadata {
                control_type: Some("joy".into()),
                player: Some("1".into()),
                buttons: Some("3".into()),
                ways: Some("8".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let games = [DatGameEntry {
            name: "pacman".into(),
            mame_input: Some(input),
            ..Default::default()
        }];
        let cache = CatalogueCache {
            catalogue_token: "local:mame".into(),
            catalogue_name: "MAME 0.280".into(),
            provenance: "registered local source".into(),
            version: Some("0.280".into()),
            machine_inputs: index_machine_inputs(&games),
            source_stamp: SourceStamp::observe(&catalogue_path).unwrap(),
        };
        let mut state = State {
            key: Some(RequestKey {
                game_id: 12,
                machine: "pacman".into(),
                catalogue: "local:mame".into(),
            }),
            cache: Some(Arc::new(cache)),
            ..Default::default()
        };
        let key = state.key.clone().unwrap();
        state.project_cached(&key);
        let result = state.result.as_ref().unwrap();
        assert_eq!(result.catalogue_name, "MAME 0.280");
        assert_eq!(result.version.as_deref(), Some("0.280"));
        assert_eq!(result.provenance, "registered local source");
        let requirements = result.requirements.as_ref().unwrap();
        assert_eq!(requirements.supported_players.as_deref(), Some("2"));
        assert_eq!(requirements.requirements.len(), 2);
        assert_eq!(
            requirements.requirements[0].family,
            ArcadeInputRequirementFamily::Digital8Way
        );
        assert_eq!(
            control_label(&requirements.requirements[1]),
            "3 buttons · Player 1"
        );
    }

    #[test]
    fn source_stamp_detects_catalogue_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arcade.xml");
        std::fs::write(&path, "old catalogue").unwrap();
        let stamp = SourceStamp::observe(&path).unwrap();
        std::fs::write(&path, "new catalogue with a different size").unwrap();
        assert!(!stamp.still_matches());
    }

    #[test]
    fn missing_catalogue_prompts_for_existing_setup_and_fits_1024_by_600() {
        let context = egui::Context::default();
        let identity = identity(&[("pacman", IdentityStatus::Verified)]);
        let health = HealthPage::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 600.0));
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let mut state = State::default();
                    assert!(!state.show(ui, 7, Some(&identity), &health));
                });
            },
        );
        assert!(
            output.shapes.iter().any(|shape| shape_contains(
                &shape.shape,
                "Choose a current MAME arcade catalogue"
            ))
        );
        assert_eq!(screen.size(), egui::vec2(1024.0, 600.0));
    }

    fn shape_contains(shape: &egui::Shape, text: &str) -> bool {
        match shape {
            egui::Shape::Text(text_shape) => text_shape.galley.text().contains(text),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| shape_contains(shape, text)),
            _ => false,
        }
    }

    #[test]
    fn stale_selected_game_cannot_accept_a_late_catalogue_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalogue.xml");
        std::fs::write(&path, "fixture").unwrap();
        let mut state = State::default();
        let key = RequestKey {
            game_id: 1,
            machine: "pacman".into(),
            catalogue: "local:x".into(),
        };
        state.key = Some(key.clone());
        state.generation = 4;
        let (sender, receiver) = mpsc::channel();
        state.receiver = Some(receiver);
        state.generation = 5;
        assert!(
            sender
                .send(Message::Loaded(
                    4,
                    key,
                    Ok(Arc::new(CatalogueCache {
                        catalogue_token: "local:x".into(),
                        catalogue_name: "old".into(),
                        provenance: "old".into(),
                        version: None,
                        machine_inputs: BTreeMap::new(),
                        source_stamp: SourceStamp::observe(&path).unwrap(),
                    }))
                ))
                .is_ok()
        );
        state.poll();
        assert!(state.result.is_none());
    }
}
