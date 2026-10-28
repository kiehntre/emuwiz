//! Advisory comparison of existing inventory observations. No destination
//! textbox creates identity, and no compatibility result enables a writer.
use super::{App, library::Game, save_snapshots::snapshot_target};
use archivefs_core::{
    game_identity::{IdentityKind, IdentityStatus},
    persistent_state_inventory::{PersistentStateRecord, PersistentStateType, StateEmulator},
    save_migration_planner::{
        SaveMigrationFormat as Format, SaveMigrationIdentityConfidence as Confidence,
        SaveMigrationRepresentation as Representation, SaveMigrationSource, SaveMigrationTarget,
    },
    save_migration_readiness::{
        SaveMigrationReadiness, SaveMigrationRequest, SaveSourceStamp, SaveTargetEnvironment,
        plan_save_migration_for_target,
    },
    save_snapshots::SaveArtifactType,
};
use eframe::egui;
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub(super) struct Panel {
    key: Option<(i64, u64)>,
    game_observation: Option<(
        std::path::PathBuf,
        Option<archivefs_core::game_identity::GameIdentityReport>,
        bool,
        String,
    )>,
    game_hashes: Option<archivefs_core::dat::library_identity_summary::LibraryItemHashes>,
    game_dat_exact: Option<Option<&'static str>>,
    source: Option<usize>,
    target: Option<usize>,
    report: Option<SaveMigrationReadiness>,
    error: Option<String>,
    job: Option<Receiver<SaveMigrationReadiness>>,
}

fn identity(record: &PersistentStateRecord) -> Option<String> {
    let values: std::collections::BTreeSet<_> = record
        .game_identity
        .iter()
        .filter(|e| e.kind != IdentityKind::Platform && e.status == IdentityStatus::Verified)
        .filter_map(|e| e.value.as_ref().map(|v| format!("{:?}:{v}", e.kind)))
        .collect();
    (values.len() == 1)
        .then(|| values.into_iter().next())
        .flatten()
}

fn observed(record: &PersistentStateRecord, game: &Game) -> SaveMigrationSource {
    let linked = snapshot_target(record, game).ok().filter(|_| {
        record.sram_origin.is_none()
            && !game.inspection_incomplete
            && record.game_identity.iter().any(|identity| {
                identity.kind != IdentityKind::Platform
                    && identity.status == IdentityStatus::Verified
                    && identity.value.is_some()
                    && game.archive.identity_report.as_ref().is_some_and(|report| {
                        report.evidence.iter().any(|e| {
                            e.kind == identity.kind
                                && e.status == IdentityStatus::Verified
                                && e.value == identity.value
                        })
                    })
            })
    });
    let associated = record.sram_origin.as_ref().filter(|o| {
        o.state == archivefs_core::sram_provenance::OriginState::Associated
            && o.context.cartridge_identity.kind == IdentityKind::LooseRomSha256
            && o.context.cartridge_identity.status == IdentityStatus::Verified
            && o.context.cartridge_identity.value.as_ref() == Some(&o.context.cartridge.sha256)
    });
    let sram = associated.is_some();
    let (save_type, format, representation) = match record.state_type {
        PersistentStateType::NativeSave if sram => (
            SaveArtifactType::Sram,
            Format::RawSram,
            Representation::RawFile {
                size_bytes: record.size_bytes,
            },
        ),
        PersistentStateType::NativeSave if record.emulator == StateEmulator::Ppsspp => (
            SaveArtifactType::PspSavedata,
            Format::PspSavedata,
            Representation::Directory { file_count: None },
        ),
        PersistentStateType::MemoryCard => (
            SaveArtifactType::MemoryCard,
            if record.emulator == StateEmulator::DuckStation {
                Format::Ps1MemoryCard
            } else {
                Format::Unknown("Unproved card format".into())
            },
            Representation::MemoryCardImage {
                size_bytes: record.size_bytes,
                container: record.emulator.as_str().into(),
            },
        ),
        PersistentStateType::SaveState => (
            SaveArtifactType::SaveState,
            Format::Unknown("Save state uses separate compatibility evidence".into()),
            Representation::OpaqueContainer {
                size_bytes: record.size_bytes,
                kind: "Save state".into(),
            },
        ),
        _ => (
            SaveArtifactType::Unknown,
            Format::Unknown("Inventory does not prove a supported save format".into()),
            Representation::OpaqueContainer {
                size_bytes: record.size_bytes,
                kind: "Unproved representation".into(),
            },
        ),
    };
    let game_identity = associated
        .map(|o| format!("LooseRomSha256:{}", o.context.cartridge.sha256))
        .or_else(|| linked.as_ref().map(|target| target.game_identity.clone()))
        .or_else(|| identity(record));
    SaveMigrationSource {
        platform: linked.as_ref().map(|t| t.platform.clone()).or_else(|| {
            (associated.is_some() && super::saves_states::record_belongs_to_game(record, game))
                .then(|| game.platform.clone())
        }),
        game_identity: game_identity.clone(),
        region: None,
        emulator: Some(record.emulator.as_str().into()),
        core: record
            .sram_origin
            .as_ref()
            .map(|o| o.context.core.path.display().to_string()),
        save_type,
        format,
        representation,
        identity_confidence: if game_identity.is_some() {
            Confidence::Exact
        } else {
            Confidence::Unknown
        },
        filename: None,
        container_context: record
            .container_path
            .as_ref()
            .map(|p| p.display().to_string()),
        evidence_stale: !record.warnings.is_empty(),
    }
}

fn compare(
    source: &PersistentStateRecord,
    target: &PersistentStateRecord,
    game: &Game,
) -> SaveMigrationReadiness {
    compare_with_refresh(
        source,
        target,
        game,
        archivefs_core::sram_provenance::inventory_with_sram_origins,
    )
}

fn compare_with_refresh(
    source: &PersistentStateRecord,
    target: &PersistentStateRecord,
    game: &Game,
    refresh: impl FnOnce(
        &[archivefs_core::persistent_state_inventory::PersistentStateRoot],
    ) -> archivefs_core::persistent_state_inventory::PersistentStateInventory,
) -> SaveMigrationReadiness {
    let mut source = source.clone();
    let mut target = target.clone();
    // The existing provenance provider revalidates seals, cartridge/core/config
    // scope and exact artifact bytes. A cached Associated label is insufficient.
    let roots: Vec<_> = [&source, &target]
        .into_iter()
        .filter(|r| r.sram_origin.is_some())
        .map(|r| {
            archivefs_core::persistent_state_inventory::PersistentStateRoot::configured(
                r.emulator,
                r.path.clone(),
                Some(r.state_type),
            )
        })
        .collect();
    if !roots.is_empty() {
        let refreshed = refresh(&roots);
        for record in [&mut source, &mut target] {
            if record.sram_origin.is_some() {
                let current = refreshed
                    .records
                    .iter()
                    .find(|r| r.path == record.path && r.emulator == record.emulator);
                let still_bound = current.is_some_and(|r| {
                    r.sram_origin == record.sram_origin && r.sha256 == record.sha256
                });
                if !still_bound {
                    record.sram_origin = None;
                    record
                        .warnings
                        .push("Save origin changed; refresh inventory".into());
                }
            }
        }
    }
    let (source, target) = (&source, &target);
    let source_stamp = SaveSourceStamp::capture(&source.path);
    let target_stamp = SaveSourceStamp::capture(&target.path);
    let mut source_model = observed(source, game);
    let target_model = observed(target, game);
    // Bind observed bytes to the inventory digest; same path/size/mtime is
    // insufficient. Missing digests, changed files and directories stay unknown.
    if source_model.platform.is_none() {
        source_model.game_identity = None;
    }
    source_model.evidence_stale |=
        !source_stamp.matches_inventory(&source.path, source.size_bytes, source.sha256.as_deref());
    let target_current =
        target_stamp.matches_inventory(&target.path, target.size_bytes, target.sha256.as_deref());
    let target_identity = if target_current && !target_model.evidence_stale {
        target_model.game_identity
    } else {
        None
    };
    let installation = target.selected_installation.as_ref();
    let mut result = plan_save_migration_for_target(SaveMigrationRequest {
        source: source_model,
        target: SaveMigrationTarget {
            platform: target_model.platform,
            game_identity: target_identity,
            region: target_model.region,
            emulator: target_model.emulator,
            core: target_model.core,
            save_type: target_model.save_type,
            format: target_model.format,
            representation: target_model.representation,
            container_context: target_model.container_context,
            proven_conversion_paths: Vec::new(),
        },
        target_environment: SaveTargetEnvironment {
            emulator: Some(target.emulator.as_str().into()),
            emulator_version: target
                .emulator_version
                .clone()
                .or_else(|| installation.and_then(|i| i.version.clone())),
            core: target
                .sram_origin
                .as_ref()
                .map(|o| o.context.core.path.display().to_string()),
            profile: target
                .sram_origin
                .as_ref()
                .filter(|o| o.state == archivefs_core::sram_provenance::OriginState::Associated)
                .map(|o| o.context.scope.profile.clone())
                .or_else(|| installation.map(|i| i.installation_id.clone()))
                .or_else(|| target.slot_profile_account.clone()),
            // Proven native SRAM is raw persistent data, not a version-bound state.
            version_matters: false,
        },
        source_path: Some(source.path.clone()),
        source_stamp: Some(source_stamp),
        // No restore preflight has been performed by this read-only journey.
        restore_refusals: None,
    });
    if source.path == target.path {
        result.outcome =
            archivefs_core::save_migration_readiness::SaveMigrationOutcome::NeedsEvidence;
        result
            .reasons
            .push(archivefs_core::save_migration_readiness::SaveMigrationReason::SameSaveLocation);
        result.restore.restore_planning_may_proceed = false;
    }
    // Source revalidation may take time. Recheck target bytes after it too;
    // compatibility remains an observation, never a target-write approval.
    if !target_current
        || target_model.evidence_stale
        || target_stamp.freshness()
            != archivefs_core::save_migration_readiness::SaveSourceFreshness::Current
    {
        if result.outcome
            != archivefs_core::save_migration_readiness::SaveMigrationOutcome::Unsupported
        {
            result.outcome =
                archivefs_core::save_migration_readiness::SaveMigrationOutcome::NeedsEvidence;
        }
        result.reasons.push(archivefs_core::save_migration_readiness::SaveMigrationReason::TargetEvidenceMissingOrChanged);
        result.restore.restore_planning_may_proceed = false;
    }
    result
}

pub(super) fn show(app: &mut App, ui: &mut egui::Ui, game: &Game) {
    let key = (game.archive.id, app.saves_states.generation);
    let panel = &mut app.saves_states.compatibility;
    let changed_game =
        panel
            .game_observation
            .as_ref()
            .is_none_or(|(path, identity, incomplete, platform)| {
                path != &game.archive.absolute_path
                    || identity != &game.archive.identity_report
                    || *incomplete != game.inspection_incomplete
                    || platform != &game.platform
            });
    if panel.key != Some(key)
        || changed_game
        || panel.game_hashes != game.dat_exact_hashes
        || panel.game_dat_exact != game.dat_exact
    {
        *panel = Panel {
            key: Some(key),
            game_hashes: game.dat_exact_hashes.clone(),
            game_dat_exact: game.dat_exact,
            game_observation: Some((
                game.archive.absolute_path.clone(),
                game.archive.identity_report.clone(),
                game.inspection_incomplete,
                game.platform.clone(),
            )),
            ..Default::default()
        };
    }
    if let Some(rx) = &panel.job {
        match rx.try_recv() {
            Ok(report) => {
                panel.report = Some(report);
                panel.job = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                panel.job = None;
                panel.error = Some(
                    "Compatibility checking ended without a result. Refresh inventory and retry."
                        .into(),
                );
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    ui.collapsing("Check observed save compatibility", |ui| {
        ui.label("Compare two observed save locations. Direct SRAM compatibility currently requires independently witnessed saves in the supported Native/User RetroArch scope. This build does not establish direct SRAM compatibility across different profiles or emulators. This advisory preview cannot move or restore saves. Missing format, identity, version or complete-set evidence stays unknown. Refresh inventory and recheck after external changes.");
        let Some(inventory) = &app.saves_states.inventory else { ui.label("Inspect save locations first."); return; };
        let mut changed = false;
        ui.add_enabled_ui(panel.job.is_none() && !app.saves_states.loading, |ui| {
            for (label, selection) in [("Source save", &mut panel.source), ("Target profile/save representation", &mut panel.target)] {
                egui::ComboBox::from_id_salt(label).selected_text(selection.and_then(|i| inventory.records.get(i)).map(|r| r.path.display().to_string()).unwrap_or_else(|| "Select an observed save location".into())).show_ui(ui, |ui| {
                    for (index, record) in inventory.records.iter().enumerate() {
                        changed |= ui.selectable_value(selection, Some(index), format!("{}: {}", record.emulator.as_str(), record.path.display())).changed();
                    }
                });
            }
            if changed { panel.report = None; panel.error = None; }
            let selected = panel.source.zip(panel.target).and_then(|(s,t)| Some((inventory.records.get(s)?, inventory.records.get(t)?)));
            if selected.is_some_and(|(source, target)| source.path == target.path) {
                ui.label("Choose two different observed save locations.");
            }
            let records = selected.filter(|(source, target)| source.path != target.path);
            if ui.add_enabled(records.is_some(), egui::Button::new("Check observed compatibility (read only)")).clicked()
                && let Some((source, target)) = records {
                let (source, target, game) = (source.clone(), target.clone(), game.clone());
                let (tx,rx) = mpsc::sync_channel(1);
                panel.report = None; panel.error = None; panel.job = Some(rx);
                std::thread::spawn(move || { let _ = tx.send(compare(&source, &target, &game)); });
            }
        });
        if panel.job.is_some() { ui.spinner(); ui.label("Checking observed save bytes…"); }
        if let Some(error) = &panel.error { ui.label(error); }
        if let Some(report) = &panel.report { draw_report(ui, report); }
    });
    if panel.job.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn draw_report(ui: &mut egui::Ui, report: &SaveMigrationReadiness) {
    use archivefs_core::save_migration_readiness::{
        SaveMigrationOutcome as Outcome, SaveMigrationReason as Reason,
        SaveRestoreWriterCapability as Writer,
    };
    ui.strong(match report.outcome {
        Outcome::DirectlyCompatible => "Observed save formats are directly compatible",
        Outcome::ConversionRequired => "A conversion or container-aware workflow is required",
        Outcome::KnownIncompatible => "Observed game or platform evidence is incompatible",
        Outcome::NeedsEvidence => "More evidence is needed",
        Outcome::Unsupported => "This save representation is unsupported here",
    });
    let environment = &report.target_environment;
    for (label, value) in [
        ("Target emulator", environment.emulator.as_deref()),
        ("Target profile", environment.profile.as_deref()),
        ("Target core", environment.core.as_deref()),
        ("Target version", environment.emulator_version.as_deref()),
    ] {
        ui.label(format!("{label}: {}", value.unwrap_or("Unknown")));
    }
    ui.label(match report.container {
        archivefs_core::save_migration_readiness::SaveContainerSemantics::RawSingleFile => {
            "Source: one persistent-save file."
        }
        archivefs_core::save_migration_readiness::SaveContainerSemantics::SharedContainer => {
            "Source: a shared card or container; individual saves cannot be copied as raw files."
        }
        archivefs_core::save_migration_readiness::SaveContainerSemantics::DirectorySet {
            ..
        } => "Source: a directory save set; completeness must be proved separately.",
        archivefs_core::save_migration_readiness::SaveContainerSemantics::Other => {
            "Source representation is unknown."
        }
    });
    if let Some(plan) = &report.planner_plan {
        ui.label(&plan.explanation);
    }
    for reason in &report.reasons {
        ui.label(match reason {
            Reason::WrongGame => "The saves identify different games.",
            Reason::PlatformMismatch => "The platforms differ.",
            Reason::GameIdentityUnknown => "A verified game identity is missing.",
            Reason::FilenameOnlyIdentity => "A filename does not prove game identity.",
            Reason::PlatformEvidenceMissing => "Platform evidence is missing.",
            Reason::PspFileSetIncomplete => "The complete PSP save set has not been proved.",
            Reason::SourceEvidenceStale => "Current source evidence is unavailable.",
            Reason::SourceChangedSinceCapture => "The source changed or could not be rechecked.",
            Reason::TargetEvidenceMissingOrChanged => {
                "Target evidence is missing, changed or needs review. Refresh and recheck."
            }
            Reason::SameSaveLocation => "Choose two different observed save locations.",
            Reason::TargetVersionEvidenceMissing => "Required target version evidence is missing.",
            Reason::NoProvenConversionRule => "No proven conversion rule is available.",
            Reason::RepresentationDiffers => "The save representations differ.",
            Reason::ContainerAwareHandlingRequired => {
                "A shared card needs a container-aware workflow."
            }
            Reason::SaveStateIsNotPersistentSave => {
                "Save states use a separate compatibility model."
            }
            Reason::UnsupportedFormat => "Inventory evidence does not prove a supported format.",
            Reason::UnsupportedRepresentation => {
                "This representation is not supported by the compatibility planner."
            }
        });
    }
    ui.label(match report.restore.writer {
        Writer::SingleFileTransactional(_) => "A generic single-file restore writer exists. Its own preflight and approvals are still required.",
        Writer::DirectoryRestoreWorkflow => "A separate directory-restore workflow exists. Its own identity, space and closure checks are still required.",
        Writer::PreviewOnly => "No generic restore writer is bound for this card or container.",
        Writer::None => "No supported restore writer is recorded for this representation.",
    });
    ui.label("Restore safety: not assessed. No backup, emulator-closure or destination approval is established. No migration writer or converter is enabled here.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityPlatform, IdentityProvenance,
    };
    use archivefs_core::persistent_state_inventory::{
        PersistentStateRoot, inventory_persistent_state,
    };
    use archivefs_core::save_migration_readiness::{SaveMigrationOutcome, SaveMigrationReason};

    fn game() -> Game {
        Game::from_archive(super::super::tests::archive(501, "Synthetic", Some("GB")))
    }
    fn record(path: &std::path::Path) -> PersistentStateRecord {
        std::fs::write(path, [1u8; 16]).unwrap();
        inventory_persistent_state(&[PersistentStateRoot::configured(
            StateEmulator::RetroArch,
            path,
            Some(PersistentStateType::NativeSave),
        )])
        .records
        .remove(0)
    }

    fn witnessed_record(
        path: &std::path::Path,
        game: &Game,
        session: &str,
        cartridge_digest: &str,
    ) -> PersistentStateRecord {
        use archivefs_core::sram_provenance::{
            Context, DirectoryIdentity, Object, OriginState, Scope, SramOrigin, Stamp,
        };
        let mut record = record(path);
        let stamp = Stamp {
            device: 0,
            inode: 0,
            size: record.size_bytes,
            modified: 0,
            modified_ns: 0,
            changed: 0,
            changed_ns: 0,
            links: 1,
            mode: 0o100600,
            uid: 0,
        };
        let artifact = Object {
            path: path.to_path_buf(),
            stamp: stamp.clone(),
            sha256: record.sha256.clone().unwrap(),
        };
        let cartridge = Object {
            path: game.archive.absolute_path.clone(),
            stamp,
            sha256: cartridge_digest.into(),
        };
        let cartridge_identity = IdentityEvidence {
            kind: IdentityKind::LooseRomSha256,
            status: IdentityStatus::Verified,
            value: Some(cartridge.sha256.clone()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: cartridge.path.clone(),
                member_path: None,
                member_index: None,
                method: "isolated witness fixture".into(),
            },
            diagnostic: "isolated witness fixture".into(),
        };
        record.sram_origin = Some(SramOrigin {
            state: OriginState::Associated,
            session: session.into(),
            artifact: artifact.clone(),
            explanation: "isolated refreshed witness".into(),
            context: Context {
                cartridge,
                cartridge_identity,
                executable: artifact.clone(),
                core: artifact.clone(),
                scope: Scope {
                    configuration: artifact.clone(),
                    selected_executable: artifact.path.clone(),
                    selected_core: artifact.path.clone(),
                    saves: path.parent().unwrap().into(),
                    saves_identity: DirectoryIdentity {
                        device: 0,
                        inode: 0,
                    },
                    profile: "Native/User".into(),
                },
            },
        });
        record
    }

    #[test]
    fn refreshed_native_scope_sram_reports_compatibility_and_refuses_stale_or_wrong_game() {
        let dir = tempfile::tempdir().unwrap();
        let game = game();
        let source = witnessed_record(
            &dir.path().join("source.srm"),
            &game,
            "source",
            &"a".repeat(64),
        );
        let mut target = witnessed_record(
            &dir.path().join("target.srm"),
            &game,
            "target",
            &"a".repeat(64),
        );
        target.sram_origin.as_mut().unwrap().context =
            source.sram_origin.as_ref().unwrap().context.clone();
        // Model the inventory response after Core has verified its sealed
        // receipts. Core's native observer/store test covers that boundary;
        // this exercises Blue's comparison and refusal path with real files.
        let refreshed = |source: &PersistentStateRecord, target: &PersistentStateRecord| {
            let mut inventory = inventory_persistent_state(&[]);
            inventory.records = vec![source.clone(), target.clone()];
            inventory
        };
        let report = compare_with_refresh(&source, &target, &game, |_| refreshed(&source, &target));
        assert_eq!(report.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert_eq!(
            report.target_environment.profile.as_deref(),
            Some("Native/User")
        );
        assert_eq!(
            report.target_environment.core,
            Some(source.path.display().to_string())
        );
        assert_eq!(report.source_path.as_deref(), Some(source.path.as_path()));
        assert!(report.read_only);
        assert!(!report.automatic_migration_available);
        assert!(!report.restore.existing_restore_apply_ready);
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
        let context = egui::Context::default();
        let output = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| draw_report(ui, &report));
        });
        let mut text = String::new();
        for shape in output.shapes {
            collect(&shape.shape, &mut text);
        }
        assert!(
            text.contains("Observed save formats are directly compatible"),
            "{text}"
        );
        assert!(text.contains("Target profile: Native/User"), "{text}");
        assert!(
            text.contains("No migration writer or converter is enabled here"),
            "{text}"
        );

        let same_location =
            compare_with_refresh(&source, &source, &game, |_| refreshed(&source, &source));
        assert_eq!(same_location.outcome, SaveMigrationOutcome::NeedsEvidence);
        assert!(
            same_location
                .reasons
                .contains(&SaveMigrationReason::SameSaveLocation)
        );
        assert!(!same_location.restore.restore_planning_may_proceed);

        let wrong = witnessed_record(
            &dir.path().join("wrong.srm"),
            &game,
            "wrong",
            &"b".repeat(64),
        );
        let report = compare_with_refresh(&source, &wrong, &game, |_| refreshed(&source, &wrong));
        assert_eq!(report.outcome, SaveMigrationOutcome::KnownIncompatible);
        assert!(report.reasons.contains(&SaveMigrationReason::WrongGame));

        let mut rejected_refresh = source.clone();
        rejected_refresh.sram_origin = None;
        let report = compare_with_refresh(&source, &target, &game, |_| {
            refreshed(&rejected_refresh, &target)
        });
        assert_ne!(report.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert!(
            report
                .reasons
                .contains(&SaveMigrationReason::UnsupportedFormat)
        );

        std::fs::write(&source.path, [3u8; 16]).unwrap();
        let report = compare_with_refresh(&source, &target, &game, |_| refreshed(&source, &target));
        assert_ne!(report.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert!(
            report
                .reasons
                .contains(&SaveMigrationReason::SourceEvidenceStale)
        );
        std::fs::write(&source.path, [1u8; 16]).unwrap();

        std::fs::write(&target.path, [2u8; 16]).unwrap();
        let report = compare_with_refresh(&source, &target, &game, |_| refreshed(&source, &target));
        assert_ne!(report.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert!(
            report
                .reasons
                .contains(&SaveMigrationReason::TargetEvidenceMissingOrChanged)
        );
        assert_eq!(std::fs::read(&source.path).unwrap(), [1u8; 16]);
        assert_eq!(std::fs::read(&target.path).unwrap(), [2u8; 16]);
        assert_eq!(std::fs::read(&wrong.path).unwrap(), [1u8; 16]);
    }

    #[test]
    fn preview_never_guesses_identity_from_save_filename_and_preserves_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let source = record(&dir.path().join("Same Game.srm"));
        let target = record(&dir.path().join("Same Game.sav"));
        let report = compare(&source, &target, &game());
        assert_ne!(report.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert!(!report.automatic_migration_available);
        assert!(!report.restore.existing_restore_apply_ready);
        assert_eq!(std::fs::read(source.path).unwrap(), [1u8; 16]);
        assert_eq!(std::fs::read(target.path).unwrap(), [1u8; 16]);
    }

    #[test]
    fn target_warnings_and_changed_bytes_refuse_with_explicit_reason() {
        let dir = tempfile::tempdir().unwrap();
        let source = record(&dir.path().join("source.srm"));
        let mut target = record(&dir.path().join("target.srm"));
        target.warnings.push("conflicting target evidence".into());
        let report = compare(&source, &target, &game());
        assert_ne!(report.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert!(
            report
                .reasons
                .contains(&SaveMigrationReason::TargetEvidenceMissingOrChanged)
        );
        target.warnings.clear();
        std::fs::write(&target.path, [2u8; 16]).unwrap();
        let report = compare(&source, &target, &game());
        assert_ne!(report.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert!(
            report
                .reasons
                .contains(&SaveMigrationReason::TargetEvidenceMissingOrChanged)
        );
    }

    #[test]
    fn platform_only_matches_are_never_game_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = record(&dir.path().join("source.srm"));
        let evidence = IdentityEvidence {
            kind: IdentityKind::Platform,
            status: IdentityStatus::Verified,
            value: Some("GB".into()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: "fixture".into(),
                member_path: None,
                member_index: None,
                method: "synthetic".into(),
            },
            diagnostic: "fixture".into(),
        };
        let mut game = game();
        game.archive.identity_report = Some(GameIdentityReport {
            archive_path: game.archive.absolute_path.clone(),
            platform: IdentityPlatform::GameBoy,
            format: IdentityImageFormat::LooseCartridgeRom,
            evidence: vec![evidence.clone()],
            warnings: vec![],
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        });
        source.game_identity.push(evidence);
        let model = observed(&source, &game);
        assert!(model.game_identity.is_none());
        assert!(model.platform.is_none());
    }

    #[test]
    fn refreshed_inventory_or_game_evidence_discards_pending_results() {
        let ctx = egui::Context::default();
        let mut app = super::super::tests::fixture(&ctx);
        let mut game = game();
        let render = |app: &mut App, game: &Game| {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| show(app, ui, game));
            });
        };
        render(&mut app, &game);
        app.saves_states.compatibility.source = Some(9);
        let (_tx, rx) = mpsc::sync_channel(1);
        app.saves_states.compatibility.job = Some(rx);
        app.saves_states.generation += 1;
        render(&mut app, &game);
        assert!(app.saves_states.compatibility.job.is_none());
        assert!(app.saves_states.compatibility.source.is_none());
        app.saves_states.compatibility.target = Some(7);
        game.inspection_incomplete = true;
        render(&mut app, &game);
        assert!(app.saves_states.compatibility.target.is_none());
        app.saves_states.compatibility.source = Some(9);
        game.dat_exact_hashes = Some(Default::default());
        render(&mut app, &game);
        assert!(app.saves_states.compatibility.source.is_none());
    }
    #[test]
    fn associated_sram_projection_keeps_game_identity_and_refuses_forged_refresh() {
        use archivefs_core::sram_provenance::{
            Context, DirectoryIdentity, Object, OriginState, Scope, SramOrigin, Stamp,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut source = record(&dir.path().join("source.srm"));
        let game = game();
        let stamp = Stamp {
            device: 0,
            inode: 0,
            size: 16,
            modified: 0,
            modified_ns: 0,
            changed: 0,
            changed_ns: 0,
            links: 1,
            mode: 0o100600,
            uid: 0,
        };
        let artifact = Object {
            path: source.path.clone(),
            stamp: stamp.clone(),
            sha256: source.sha256.clone().unwrap(),
        };
        let cartridge = Object {
            path: game.archive.absolute_path.clone(),
            stamp: stamp.clone(),
            sha256: "a".repeat(64),
        };
        let gold = IdentityEvidence {
            kind: IdentityKind::LooseRomSha256,
            status: IdentityStatus::Verified,
            value: Some(cartridge.sha256.clone()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: cartridge.path.clone(),
                member_path: None,
                member_index: None,
                method: "synthetic observation".into(),
            },
            diagnostic: "test fixture, not a sealed receipt".into(),
        };
        source.sram_origin = Some(SramOrigin {
            state: OriginState::Associated,
            session: "synthetic observation".into(),
            artifact: artifact.clone(),
            explanation: "trusted-observation projection fixture only".into(),
            context: Context {
                cartridge,
                cartridge_identity: gold,
                executable: artifact.clone(),
                core: artifact.clone(),
                scope: Scope {
                    configuration: artifact.clone(),
                    selected_executable: artifact.path.clone(),
                    selected_core: artifact.path.clone(),
                    saves: dir.path().into(),
                    saves_identity: DirectoryIdentity {
                        device: 0,
                        inode: 0,
                    },
                    profile: "fixture".into(),
                },
            },
        });
        // Pure adapter projection of a trusted observation; no claim this
        // fabricated fixture is cryptographic association evidence.
        let model = observed(&source, &game);
        assert_eq!(model.platform, Some(game.platform.clone()));
        assert_eq!(
            model.game_identity,
            Some(format!("LooseRomSha256:{}", "a".repeat(64)))
        );
        assert_eq!(model.format, Format::RawSram);
        let mut other_game = game.clone();
        other_game.archive.absolute_path = dir.path().join("another-game.gb");
        assert!(observed(&source, &other_game).platform.is_none());
        // Production compare reopens the existing provider. A fabricated
        // Associated flag cannot pass its receipt refresh.
        let result = compare(&source, &source, &game);
        assert_ne!(result.outcome, SaveMigrationOutcome::DirectlyCompatible);
        assert_eq!(std::fs::read(&source.path).unwrap(), [1u8; 16]);
    }
}
