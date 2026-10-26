//! Pure projection of save-state evidence. No filesystem I/O in rendering.
use archivefs_core::{
    game_identity::IdentityEvidence,
    launch::{LaunchBlocker, LaunchTarget},
    persistent_state_inventory::{
        PersistentStateInventory, PersistentStateRecord, PersistentStateType, StateEmulator,
    },
    save_state_compatibility::{
        SaveStateCompatibilityTarget, SaveStateFormatEvidence, assess_save_state_compatibility,
        same_game,
    },
    save_state_resume_readiness::plan_save_state_resume,
};
use eframe::egui;

type Context = (SaveStateCompatibilityTarget, Vec<LaunchBlocker>);

pub(super) fn selected_context(
    selection: &LaunchTarget,
    blockers: &[LaunchBlocker],
    duckstation: Option<&crate::launch_readiness_page::DuckStationLaunchContext>,
    game: Vec<IdentityEvidence>,
) -> Option<Context> {
    let mut target = target(selection, game);
    if let LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id,
        ..
    } = selection
    {
        let context = duckstation?;
        let crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority::Authorised {
            executable,
            profile_root,
            ..
        } = &context.authority
        else {
            return None;
        };
        let [profile] = context.discovery.profiles.as_slice() else {
            return None;
        };
        if &profile.profile_id != profile_id || &profile.configuration_path != profile_root {
            return None;
        }
        target.version = profile
            .executable_candidates
            .iter()
            .find(|candidate| &candidate.path == executable)
            .and_then(|candidate| candidate.version.clone());
    }
    Some((target, blockers.to_vec()))
}

pub(super) fn target(
    selection: &LaunchTarget,
    game: Vec<IdentityEvidence>,
) -> SaveStateCompatibilityTarget {
    let (emulator, profile, core) = match selection {
        LaunchTarget::Standalone {
            adapter_id,
            profile_id,
            ..
        } => {
            let emulator = match *adapter_id {
                "duckstation" => StateEmulator::DuckStation,
                "pcsx2" => StateEmulator::Pcsx2,
                "dolphin" => StateEmulator::Dolphin,
                "ppsspp" => StateEmulator::Ppsspp,
                "rpcs3" => StateEmulator::Rpcs3,
                "flycast" => StateEmulator::Flycast,
                "mame" => StateEmulator::Mame,
                "hatari" => StateEmulator::Hatari,
                "fs-uae" => StateEmulator::FsUae,
                "xemu" => StateEmulator::Xemu,
                "xenia" => StateEmulator::Xenia,
                "cemu" => StateEmulator::Cemu,
                "vita3k" => StateEmulator::Vita3k,
                _ => StateEmulator::Unknown,
            };
            (emulator, profile_id.clone(), None)
        }
        LaunchTarget::RetroArchCore {
            profile, core_stem, ..
        } => (
            StateEmulator::RetroArch,
            format!("{:?}/{:?}", profile.profile_kind, profile.scope),
            Some(core_stem.clone()),
        ),
    };
    // LaunchTarget carries no verified version facts. Never use a version from
    // the state inventory as the version of the current Launch selection.
    SaveStateCompatibilityTarget {
        emulator,
        profile: Some(profile),
        core,
        version: None,
        core_version: None,
        game,
    }
}

pub(super) fn show(ui: &mut egui::Ui, record: &PersistentStateRecord, context: Option<&Context>) {
    if record.state_type != PersistentStateType::SaveState {
        return;
    }
    let assessment = assess_save_state_compatibility(
        record,
        None,
        context.map(|c| &c.0),
        &SaveStateFormatEvidence::Unknown,
        None,
    );
    let readiness =
        plan_save_state_resume(assessment, context.map(|c| c.1.clone()).unwrap_or_default());
    ui.strong(format!(
        "Save-state compatibility: {:?}",
        readiness.assessment.compatibility
    ));
    if let Some((target, _)) = context {
        ui.label(format!(
            "Current Launch selection: {} · profile {}{} · version {}",
            target.emulator.as_str(),
            target.profile.as_deref().unwrap_or("unknown"),
            target
                .core
                .as_deref()
                .map(|core| format!(" · core {core}"))
                .unwrap_or_default(),
            target.version.as_deref().unwrap_or("Unknown")
        ));
    }
    for reason in &readiness.assessment.reasons {
        ui.label(reason);
    }
    for blocker in &readiness.launch_blockers {
        ui.label(format!("Launch remains blocked: {}", blocker.detail));
    }
    ui.weak("Save-state format compatibility is separate from ordinary game-save compatibility. Automatic state loading is unavailable; restoration has not been verified.");
}

pub(super) fn show_game(
    ui: &mut egui::Ui,
    game: &super::library::Game,
    inventory: Option<&PersistentStateInventory>,
    context: Option<&Context>,
) {
    ui.collapsing("Save-state compatibility guidance", |ui| {
        let records: Vec<_> = inventory.into_iter().flat_map(|i| &i.records)
            .filter(|record| record.state_type == PersistentStateType::SaveState)
            .filter(|record| game.archive.identity_report.as_ref().is_some_and(|report| same_game(&record.game_identity, &report.evidence))).collect();
        if records.is_empty() {
            ui.label("Unknown: no inspected save state has an unambiguous game identity matching this game. Open Saves & States, inspect save locations and review unassigned states.");
        }
        for record in records { ui.push_id(&record.path, |ui| { ui.label(record.path.display().to_string()); show(ui, record, context); }); }
        ui.weak("This guidance never loads a state or changes Launch readiness.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selected(profile: &str, adapter: &'static str) -> LaunchTarget {
        LaunchTarget::Standalone {
            adapter_id: adapter,
            profile_id: profile.into(),
            profile_path: None,
        }
    }
    #[test]
    fn launch_selection_projection_never_reuses_inventory_versions() {
        let a = target(&selected("portable", "duckstation"), vec![]);
        let b = target(&selected("native", "pcsx2"), vec![]);
        assert_eq!(a.emulator, StateEmulator::DuckStation);
        assert_eq!(b.emulator, StateEmulator::Pcsx2);
        assert_ne!(a.profile, b.profile);
        assert_eq!(a.version, None);
        assert_eq!(b.version, None);
        assert_eq!(
            target(&selected("profile", "unrecognized"), vec![]).emulator,
            StateEmulator::Unknown
        );
    }
    #[test]
    fn refused_or_missing_shared_resolver_never_becomes_a_selected_target() {
        use crate::emulator_setup::profile_resolution::DuckStationLaunchAuthority;
        use archivefs_core::patch_manager::{
            DuckStationProfileDiscovery, DuckStationProfileDiscoveryRoots,
        };
        let selection = selected("profile", "duckstation");
        assert!(selected_context(&selection, &[], None, vec![]).is_none());
        let context = crate::launch_readiness_page::DuckStationLaunchContext {
            multidisc: None,
            discovery: DuckStationProfileDiscovery {
                profiles: vec![],
                complete: true,
            },
            roots: DuckStationProfileDiscoveryRoots {
                home: "/synthetic".into(),
                xdg_config_home: "/synthetic/config".into(),
                xdg_data_home: "/synthetic/data".into(),
                xdg_config_home_explicit: true,
                explicit_configuration_roots: vec![],
                portable_configuration_roots: vec![],
                explicit_executables: vec![],
                known_version_outputs: Default::default(),
                appimage_directory: None,
                path_override: Some(vec![]),
            },
            firmware_evidence: vec![],
            verified_ps1_serial: None,
            authority: DuckStationLaunchAuthority::Refused {
                reason: "synthetic conflicting selections".into(),
            },
        };
        assert!(selected_context(&selection, &[], Some(&context), vec![]).is_none());
    }

    fn collect(shape: &egui::epaint::Shape, text: &mut String) {
        match shape {
            egui::epaint::Shape::Text(shape) => {
                text.push_str(shape.galley.text());
                text.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, text);
                }
            }
            _ => {}
        }
    }
    #[test]
    fn blue_state_guidance_renders_unknown_and_no_restore_claim() {
        use archivefs_core::persistent_state_inventory::{PortabilityClass, StatePathOrigin};
        let record = PersistentStateRecord {
            sram_origin: None,
            emulator: StateEmulator::DuckStation,
            selected_installation: None,
            state_type: PersistentStateType::SaveState,
            game_identity: vec![],
            path: "/synthetic/absent.state".into(),
            container_path: None,
            slot_profile_account: None,
            emulator_version: Some("observed installation version".into()),
            firmware_context: None,
            portability_class: PortabilityClass::VersionBound,
            source_path_origin: StatePathOrigin::Configured,
            provenance: "synthetic".into(),
            sha256: None,
            size_bytes: 0,
            warnings: vec![],
        };
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &record, None));
        });
        let mut text = String::new();
        for shape in output.shapes {
            collect(&shape.shape, &mut text);
        }
        assert!(text.contains("Save-state compatibility: Unknown"), "{text}");
        assert!(text.contains("Capture provenance is unavailable"), "{text}");
        assert!(text.contains("restoration has not been verified"), "{text}");
        assert!(text.contains("ordinary game-save compatibility"), "{text}");
        assert!(!text.contains("Supported"));
    }
}
