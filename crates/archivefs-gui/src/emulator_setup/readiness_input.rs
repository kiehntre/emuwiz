//! The launch-readiness input builder, separated from live `ArchiveFsApp`
//! state. Every read the builder makes goes through [`ReadinessSources`], so
//! the same code serves the selected game (borrowing the app) and any later
//! caller that supplies its own immutable snapshots.

use crate::*;

use super::controller::disc_files_diagnostic;
use super::profile_resolution::ppsspp::PpssppResolutionInputs;
use super::profile_resolution::{
    DuckStationLaunchAuthority, DuckStationLaunchSelection, DuckStationResolutionInputs,
    select_for_launch,
};

/// What the builder reads, borrowed. The app builds one from itself; nothing
/// here is mutated and nothing is launched.
pub(crate) struct ReadinessSources<'a> {
    pub(crate) readiness: &'a EmulatorReadinessState,
    pub(crate) evidence: &'a selected_evidence_page::SelectedEvidenceState,
    pub(crate) focused: Option<&'a Path>,
    pub(crate) cheat_workflow: &'a Option<CheatWorkflowState>,
    pub(crate) database_snapshot: Option<&'a CachedLibrarySnapshot>,
    pub(crate) database_generation: u64,
    pub(crate) freshness_for:
        &'a dyn Fn(&Path, Option<&LoadedData>) -> archivefs_core::launch::EvidenceFreshness,
    pub(crate) member_path: &'a dyn Fn(&ArchiveRecord) -> Option<PathBuf>,
    pub(crate) is_scummvm_folder: &'a dyn Fn(&Path) -> bool,
    pub(crate) loose_ref: &'a dyn Fn(&Path) -> Option<archivefs_core::launch::LaunchContentRef>,
}

impl ReadinessSources<'_> {
    pub(crate) fn build(
        &self,
        live: Option<&LoadedData>,
    ) -> launch_readiness_page::LaunchReadinessInput {
        use launch_readiness_page::LaunchReadinessInput;

        if !matches!(
            self.evidence,
            selected_evidence_page::SelectedEvidenceState::Ready { .. }
        ) {
            return LaunchReadinessInput::EvidenceNotLoaded;
        }
        let retroarch_scanned = matches!(
            self.readiness.retroarch_profiles,
            RetroArchProfilesState::Ready(_)
        );
        let empty_retroarch =
            archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
                format_version: 2,
                profiles: Vec::new(),
                diagnostics: Vec::new(),
            };
        let retroarch_environment = match &self.readiness.retroarch_profiles {
            RetroArchProfilesState::Ready(discovery) => &discovery.environment,
            RetroArchProfilesState::NotScanned
            | RetroArchProfilesState::Scanning { .. }
            | RetroArchProfilesState::Error(_) => &empty_retroarch,
        };

        let focused = self.focused;
        let selected_game_identity_report = focused.and_then(|focused| match &self.evidence {
            selected_evidence_page::SelectedEvidenceState::Ready { report, .. }
                if report.path == focused =>
            {
                Some(&report.game_identity_report)
            }
            _ => None,
        });
        // Keep older in-process test/compatibility fixtures usable when they
        // have not yet been given the new selected-evidence report. A real
        // selected game always takes the evidence-worker report above, so
        // opening Cheats & Mods is no longer required for launch planning.
        let game_identity_report = selected_game_identity_report
            .filter(|report| {
                !matches!(
                    archivefs_core::launch::canonical_identity_from_game_report(report).0,
                    archivefs_core::launch::CanonicalIdentityStatus::Unknown
                )
            })
            .or_else(|| {
                focused
                    .and_then(|focused| {
                        self.cheat_workflow
                            .as_ref()
                            .filter(|workflow| workflow.archive_path == focused)
                    })
                    .and_then(ready_game_identity)
            });

        // A saved report from a scan whose inspection did not finish may not
        // authorise a launch; independent evidence below still can.
        let evidence_freshness = focused
            .map_or(archivefs_core::launch::EvidenceFreshness::Current, |path| {
                (self.freshness_for)(path, live)
            });
        let (identity_status, verified_facts) = match game_identity_report {
            Some(report) => archivefs_core::launch::canonical_identity_for_saved_report(
                report,
                evidence_freshness,
            ),
            None => (
                archivefs_core::launch::CanonicalIdentityStatus::Unknown,
                Vec::new(),
            ),
        };
        // PCSX2's `Pcsx2LaunchRequest` needs a genuinely verified PS2
        // serial specifically - never `plan.game_key` alone, which for PS2
        // may instead be a verified executable CRC (see
        // `evidence_bridge::resolved_identity_for_platform`'s `Ps2Serial`/
        // `Pcsx2ExecutableCrc` handling) when no serial was verified.
        let verified_ps2_serial = verified_facts.iter().find_map(|fact| match fact {
            archivefs_core::launch::VerifiedIdentityFact::Ps2Serial(serial) => Some(serial.clone()),
            _ => None,
        });

        // Arcade directory archives are logical MAME sets, not ordinary
        // files, so their identity comes from the persisted, SHA-bound MAME
        // join. This is the narrow projection bridge that keeps raw members
        // (for example ru_04b.img) out of launch identity.
        let focused_record = live.and_then(|data| {
            focused.and_then(|path| {
                data.records
                    .iter()
                    .find(|record| record.mount_plan.archive.path == path)
            })
        });
        let mut identity_status = identity_status;
        if matches!(
            identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
        ) && let Some(path) = focused
            && let selected_evidence_page::SelectedEvidenceState::Ready { report, .. } =
                &self.evidence
            && let Some(identity) = crate::launch_readiness_page::desmume::selected_identity(
                path,
                report,
                evidence_freshness,
            )
        {
            identity_status = identity;
        }
        let mut mame_set_resolutions = Vec::new();
        if let Some(focused) = focused
            && let Ok(database_path) = archivefs_core::default_database_path()
            && let Ok(database) = archivefs_core::Database::open_read_only(database_path)
            && let Ok(Some(evidence)) = database.mame_arcade_join_for_archive_path(
                archivefs_core::dat::mame_arcade_join::MAME_0174_SHA256,
                focused,
            )
            && let Some(resolution) =
                archivefs_core::dat::mame_arcade_join::launch_resolution_for_join(
                    &evidence, focused,
                )
        {
            identity_status = archivefs_core::launch::CanonicalIdentityStatus::Resolved(
                archivefs_core::launch::ResolvedIdentity {
                    platform_id: "Arcade".to_string(),
                    game_key: resolution.identity.game_name.clone(),
                },
            );
            mame_set_resolutions.push(resolution);
        }

        // A loose ADF has no header identity; its only launch identity is a
        // current DAT match that also proves a single-disk game.
        let amiga_adf = matches!(
            identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
        )
        .then(|| {
            focused.and_then(|path| {
                crate::emulator_setup::amiga_adf::launch_inputs(
                    path,
                    focused_record.and_then(|r| r.mount_plan.archive.identity.platform.as_deref()),
                )
            })
        })
        .flatten();
        if let Some(inputs) = &amiga_adf {
            identity_status = inputs.identity.clone();
        }

        // Read-only Atari ST setup/media projection has no typed launch request.
        // Preserve stronger/conflicting identities and keep this separate from
        // canonical game identity, verification and the executable Plan lane.
        if matches!(
            identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
        ) && let Some(path) = focused
            && (path
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "st" | "stx"))
                || selected_game_identity_report.is_some_and(|r| {
                    r.platform == archivefs_core::game_identity::IdentityPlatform::AtariST
                }))
        {
            return LaunchReadinessInput::AtariSt(
                crate::emulator_setup::hatari::Controller::snapshot(
                    &self.readiness.hatari,
                    crate::emulator_setup::hatari::Selection {
                        path: path.into(),
                        source_root: focused_record
                            .map(|r| r.mount_plan.archive.identity.source_root.clone())
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or_else(|| path.parent().unwrap_or(path).into()),
                        platform: focused_record
                            .and_then(|r| r.mount_plan.archive.identity.platform.clone()),
                    },
                ),
            );
        }

        let flycast_context = self.readiness.flycast_gdi.context(focused);
        let flycast_inputs = flycast_context
            .as_ref()
            .and_then(|c| c.inputs(&identity_status));
        if let Some((identity, _)) = &flycast_inputs {
            identity_status = identity.clone();
        }
        let vice_context = self.readiness.vice.context(focused);
        let vice_inputs = vice_context
            .as_ref()
            .and_then(|c| c.inputs(&identity_status));
        if let Some((identity, _)) = &vice_inputs {
            identity_status = identity.clone();
        }
        let xroar_context = self.readiness.xroar_cassette.context(focused);
        let xroar_inputs = xroar_context
            .as_ref()
            .and_then(|context| context.inputs(&identity_status));
        if let Some((identity, _)) = &xroar_inputs {
            identity_status = identity.clone();
        }
        let cemu_context = self.readiness.cemu.launch_context(focused);
        let cemu_inputs = cemu_context
            .as_ref()
            .filter(|_| {
                selected_game_identity_report.is_none_or(|r| {
                    matches!(
                        r.platform,
                        archivefs_core::game_identity::IdentityPlatform::WiiU
                            | archivefs_core::game_identity::IdentityPlatform::Other
                    )
                }) && focused_record.is_none_or(|r| {
                    r.mount_plan
                        .archive
                        .identity
                        .platform
                        .as_deref()
                        .is_none_or(|p| {
                            archivefs_core::canonical_platform_for_alias(p) == Some("WiiU")
                        })
                })
            })
            .and_then(|context| context.inputs(&identity_status));
        if let Some((identity, _)) = &cemu_inputs {
            identity_status = identity.clone();
        }

        // The snapshot adapter is shared with canonical preflight. It can
        // only mint authority from current DAT evidence and matching bytes.
        if matches!(
            identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
        ) && let Some(path) = focused
            && let Some(evidence) = crate::emulator_setup::zx_snapshot::readiness(path)
        {
            identity_status = evidence.identity();
        }
        match identity_status {
            archivefs_core::launch::CanonicalIdentityStatus::Unknown => {
                // The worker's own report already says which track of a CUE
                // disc could not be resolved; carry it instead of hiding it.
                if let Some(detail) = selected_game_identity_report
                    .and_then(|report| disc_files_diagnostic(report, focused))
                {
                    return LaunchReadinessInput::DiscFilesUnresolved(detail);
                }
                return if !evidence_freshness.is_incomplete()
                    && focused.is_some_and(|path| {
                        archivefs_core::launch::locate_dosbox_game_directory(path, None).is_some()
                    }) {
                    LaunchReadinessInput::DosBoxReview
                } else if evidence_freshness.is_incomplete() {
                    LaunchReadinessInput::InspectionIncomplete
                } else {
                    LaunchReadinessInput::IdentityUnknown
                };
            }
            archivefs_core::launch::CanonicalIdentityStatus::Conflicting => {
                return LaunchReadinessInput::IdentityConflicting;
            }
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(_) => {}
        }

        // Archive safety: only the transient, selection-bound preparation
        // state may provide an inner member. The bridge still refuses it
        // unless the record is genuinely mounted.
        let mut content = match focused_record {
            Some(record) => {
                let member = (self.member_path)(record);
                archivefs_core::launch::launch_content_ref_from_archive_record(
                    record,
                    member.as_deref(),
                )
            }
            // Live library data isn't loaded, so content cannot be honestly
            // classified either way - this reads exactly like an
            // unresolved bridge result, never a fabricated "found" or a
            // misleading reuse of the evidence-not-loaded message (evidence
            // genuinely was loaded; only the live library data was not).
            // A ScummVM game folder is catalogued by the scan's detector and
            // has no live archive record: its own path is the runnable
            // content, exactly as for any other loose, directly launched item.
            None if focused.is_some_and(|path| (self.is_scummvm_folder)(path)) => {
                archivefs_core::launch::LaunchContentRef {
                    kind: None,
                    container: Some(archivefs_core::launch::LaunchContainerKind::PlainFile),
                    resolved_path: focused.map(Path::to_path_buf),
                    requires_mount: false,
                    provenance: "extracted ScummVM game folder from the persisted catalogue"
                        .to_string(),
                }
            }
            // Blue does not load live library data. The page's persisted
            // row for this exact path is plain loose content, which core's
            // bridge would resolve to its own path; a vanished or replaced
            // file is still refused by launch preflight.
            None if let Some(content) = focused.and_then(|path| (self.loose_ref)(path)) => content,
            None => archivefs_core::launch::LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: None,
                requires_mount: false,
                provenance: "live library data is not loaded; content readiness cannot be \
                             checked"
                    .to_string(),
            },
        };
        if !mame_set_resolutions.is_empty() {
            content.resolved_path = focused.map(Path::to_path_buf);
            content.container = Some(archivefs_core::launch::LaunchContainerKind::PlainFile);
            content.requires_mount = false;
            content.provenance =
                "verified MAME logical-set directory from the persisted DAT join".into();
        }

        if let Some((_, selected_content)) = flycast_inputs {
            content = selected_content;
        }
        if let Some((_, selected_content)) = cemu_inputs {
            content = selected_content;
        }
        if let Some((_, selected_content)) = vice_inputs {
            content = selected_content;
        }
        if let Some((_, selected_content)) = xroar_inputs {
            content = selected_content;
        }

        // WHDLoad is an additive, content-bound launch seam.  The package
        // and slave are accepted only after the existing bounded LHA
        // inspector found exactly one valid slave; ADF/HDF and arbitrary
        // paths never enter this branch.
        let mut amiga_whdload_profiles = Vec::new();
        let mut amiga_whdload_requests = Vec::new();
        let mut verified_whdload_content = None;
        if let Some(record) = focused_record
            && matches!(
                identity_status,
                archivefs_core::launch::CanonicalIdentityStatus::Resolved(_)
            )
            && record
                .mount_plan
                .archive
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(extension.to_ascii_lowercase().as_str(), "lha" | "lzh")
                })
        {
            let cancel = AtomicBool::new(false);
            if let Ok(discovery) =
                archivefs_core::amiga_whdload_archive::discover_whdload_slaves_in_archive(
                    &record.mount_plan.archive.path,
                    &cancel,
                )
                && discovery.candidates.len() == 1
            {
                let candidate = &discovery.candidates[0];
                let mut artifact = candidate.artifact.clone();
                artifact.path = record.mount_plan.archive.path.clone();
                let selected_name = Path::new(&candidate.member_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned);
                if let Some(selected_name) = selected_name {
                    artifact.name = selected_name.clone();
                    let target = archivefs_core::launch::VerifiedWHDLoadTarget {
                        package_path: record.mount_plan.archive.path.clone(),
                        format: archivefs_core::launch::WhdloadPackageFormat::Lha,
                    };
                    let selected_slave = archivefs_core::launch::SelectedWHDLoadSlave {
                        sha256: artifact.hashes.sha256.clone(),
                        artifact_path: record.mount_plan.archive.path.clone(),
                        name: selected_name.clone(),
                        parsed: artifact.parsed.clone(),
                    };
                    verified_whdload_content =
                        Some(archivefs_core::launch::VerifiedWHDLoadContent {
                            target: target.clone(),
                            selected_slave: selected_slave.clone(),
                        });
                    if let Some(roots) =
                        archivefs_core::patch_manager::AmigaProfileDiscoveryRoots::from_environment(
                        )
                    {
                        let discovery =
                            archivefs_core::patch_manager::discover_amiga_profiles(&roots);
                        let identity = match &identity_status {
                            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) => {
                                identity
                            }
                            _ => unreachable!(),
                        };
                        for profile in &discovery.profiles {
                            let emulator = profile.emulator;
                            let candidate_count = discovery
                                .profiles
                                .iter()
                                .filter(|other| other.emulator == emulator)
                                .count();
                            let inspection =
                                archivefs_core::patch_manager::inspect_amiga_whdload_game(
                                    profile,
                                    &archivefs_core::patch_manager::AmigaGameRequest {
                                        verified_amiga_identity: Some(identity.game_key.clone()),
                                        bare_slaves: vec![artifact.clone()],
                                        ..Default::default()
                                    },
                                );
                            let executable = match profile.executable_candidates.as_slice() {
                                [executable] => Some(executable.path.clone()),
                                _ => None,
                            };
                            let mut request = archivefs_core::launch::WHDLoadLaunchInput {
                                #[cfg(target_os = "linux")]
                                boot: Err(
                                    "No authorized WHDLoad boot template has been inspected."
                                        .into(),
                                ),
                                identity: identity_status.clone(),
                                target: Some(target.clone()),
                                slave: Some(selected_slave.clone()),
                                profile: archivefs_core::launch::WHDLoadProfileInput {
                                    emulator,
                                    profile_id: profile.profile_id.clone(),
                                    executable,
                                    configuration: profile.global_config_path.clone(),
                                    candidate_count,
                                    eligible: profile.eligible,
                                    kickstart: inspection.health.kickstart.state,
                                    verified_identity: identity.game_key.clone(),
                                },
                            };
                            #[cfg(target_os = "linux")]
                            if emulator == archivefs_core::patch_manager::AmigaEmulatorKind::FsUae {
                                request.boot =
                                    archivefs_core::launch::fsuae_whdload_boot::inspect_boot(
                                        &request,
                                    );
                            }
                            let adapter_id = match emulator {
                                archivefs_core::patch_manager::AmigaEmulatorKind::Amiberry => {
                                    "amiberry"
                                }
                                archivefs_core::patch_manager::AmigaEmulatorKind::FsUae => "fsuae",
                            };
                            amiga_whdload_profiles.push(
                                archivefs_core::launch::StandaloneProfileInput {
                                    adapter_id,
                                    profile_id: profile.profile_id.clone(),
                                    profile_path: profile.global_config_path.clone(),
                                    eligible: profile.eligible,
                                    firmware:
                                        archivefs_core::launch::FirmwareReadiness::NotRequired,
                                },
                            );
                            amiga_whdload_requests.push((
                                adapter_id.to_string(),
                                profile.profile_id.clone(),
                                request,
                            ));
                        }
                    }
                }
            }
        }
        let (amiga_adf_profiles, amiga_adf_requests, amiga_adf_scanned) = match amiga_adf {
            Some(inputs) => (inputs.profiles, inputs.requests, true),
            None => (Vec::new(), Vec::new(), false),
        };
        let amiga_whdload_context = (!amiga_whdload_requests.is_empty()
            || !amiga_adf_requests.is_empty())
        .then_some(launch_readiness_page::AmigaWHDLoadLaunchContext {
            requests: amiga_whdload_requests,
            adf: amiga_adf_requests,
        });
        if let Some(binding) = verified_whdload_content {
            content.kind = Some(archivefs_core::launch::LaunchContentKind::Whdload(binding));
        }

        // Real, already-discovered Dolphin profiles only - never a
        // fabricated `StandaloneProfileInput`. `DolphinLocalProfilesState`
        // starting `NotScanned`/`Scanning`/`Error` simply contributes no
        // Dolphin candidate yet (the same honest, fail-closed shape as an
        // empty slice), rather than blocking the whole panel the way a
        // missing RetroArch scan does - Dolphin readiness is additive here.
        let dolphin_context = match &self.readiness.dolphin_local_profiles {
            DolphinLocalProfilesState::Ready(ready) => {
                Some(launch_readiness_page::DolphinLaunchContext {
                    discovery: ready.discovery.clone(),
                    roots: ready.roots.clone(),
                })
            }
            DolphinLocalProfilesState::NotScanned
            | DolphinLocalProfilesState::Scanning { .. }
            | DolphinLocalProfilesState::Error(_) => None,
        };
        let dolphin_standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            dolphin_context
                .as_ref()
                .map(|context| {
                    context
                        .discovery
                        .profiles
                        .iter()
                        .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                            adapter_id: "dolphin",
                            profile_id: profile.profile_id.clone(),
                            profile_path: Some(profile.configuration_root.clone()),
                            eligible: profile.eligible,
                            firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                        })
                        .collect()
                })
                .unwrap_or_default();

        // Real, already-discovered PCSX2 profiles and already-resolved PS2
        // firmware evidence only - never fabricated. Both
        // `Pcsx2LaunchProfilesState` and `Pcsx2FirmwareEvidenceState`
        // starting anything other than `Ready` simply contribute no PCSX2
        // candidate yet, the same additive-not-blocking shape as `dolphin`
        // above - a missing/incomplete DAT scan never widens readiness, it
        // only means no PCSX2 candidate is offered until it completes.
        let pcsx2_firmware_evidence: &[archivefs_core::dat::firmware_evidence::FirmwareIdentityRecord] =
            match &self.readiness.pcsx2_firmware_evidence {
                Pcsx2FirmwareEvidenceState::Ready(evidence) => evidence,
                Pcsx2FirmwareEvidenceState::NotLoaded
                | Pcsx2FirmwareEvidenceState::Loading { .. }
                | Pcsx2FirmwareEvidenceState::Error(_) => &[],
            };
        let pcsx2_context = match &self.readiness.pcsx2_launch_profiles {
            Pcsx2LaunchProfilesState::Ready(ready) => {
                Some(launch_readiness_page::Pcsx2LaunchContext {
                    discovery: ready.discovery.clone(),
                    roots: ready.roots.clone(),
                    firmware_evidence: pcsx2_firmware_evidence.to_vec(),
                    verified_ps2_serial: verified_ps2_serial.clone(),
                })
            }
            Pcsx2LaunchProfilesState::NotScanned
            | Pcsx2LaunchProfilesState::Scanning { .. }
            | Pcsx2LaunchProfilesState::Error(_) => None,
        };
        // A profile's BIOS readiness never depends on the currently
        // selected game - `resolve_pcsx2_bios`/`inspect_pcsx2_bios` only
        // ever read the profile's own global config and `bios/` directory
        // - so an all-`None` `Pcsx2GameRequest` here is a genuine,
        // honest inspection, not a placeholder.
        let pcsx2_standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            pcsx2_context
                .as_ref()
                .map(|context| {
                    context
                        .discovery
                        .profiles
                        .iter()
                        .map(|profile| {
                            let inspection =
                                archivefs_core::patch_manager::inspect_pcsx2_game_with_firmware_evidence(
                                    profile,
                                    &archivefs_core::patch_manager::Pcsx2GameRequest {
                                        verified_ps2_serial: None,
                                        verified_executable_crc: None,
                                        emulator_serial: None,
                                    },
                                    &context.firmware_evidence,
                                );
                            archivefs_core::launch::StandaloneProfileInput {
                                adapter_id: "pcsx2",
                                profile_id: profile.profile_id.clone(),
                                profile_path: Some(profile.configuration_path.clone()),
                                eligible: profile.eligible,
                                firmware: archivefs_core::launch::pcsx2_firmware_readiness(
                                    inspection.inspection.bios.verification,
                                ),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

        // Flycast is additive to the existing readiness view. Its adapter
        // already owns configuration, executable and firmware inspection;
        // this page only projects those read-only results into the shared
        // launch planner. A binding is resolved here solely so a missing,
        // unsafe or ambiguous executable is shown as blocked rather than
        // advertised as launch-ready. Core preflight repeats the same check.
        let flycast_standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            match &self.readiness.flycast_profiles {
                FlycastProfilesState::Ready(ready) => ready
                    .discovery
                    .profiles
                    .iter()
                    .map(|profile| {
                        let verified_product_code = verified_facts.iter().find_map(|fact| match fact {
                            archivefs_core::launch::VerifiedIdentityFact::DreamcastProductCode(code) => {
                                Some(code.clone())
                            }
                            _ => None,
                        });
                        let inspection = archivefs_core::patch_manager::inspect_flycast_game(
                            profile,
                            &archivefs_core::patch_manager::FlycastGameRequest {
                                canonical_platform: Some("Dreamcast".to_string()),
                                flycast_platform: Some(
                                    archivefs_core::patch_manager::FlycastPlatform::Dreamcast,
                                ),
                                verified_dreamcast_product_code: verified_product_code,
                                ..Default::default()
                            },
                        );
                        archivefs_core::launch::StandaloneProfileInput {
                            adapter_id: "flycast",
                            profile_id: profile.profile_id.clone(),
                            profile_path: Some(profile.configuration_path.clone()),
                            eligible: profile.eligible
                                && archivefs_core::patch_manager::resolve_flycast_native_launch_binding(
                                    profile,
                                )
                                .is_ok(),
                            firmware: archivefs_core::launch::flycast_firmware_readiness(
                                inspection.health.system.dreamcast_bios,
                            ),
                        }
                    })
                    .collect(),
                FlycastProfilesState::NotScanned
                | FlycastProfilesState::Scanning { .. }
                | FlycastProfilesState::Error(_) => Vec::new(),
            };

        let mut standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            dolphin_standalone_profiles
                .into_iter()
                .chain(pcsx2_standalone_profiles)
                .chain(flycast_standalone_profiles)
                .chain(amiga_whdload_profiles)
                .chain(amiga_adf_profiles)
                .collect();

        if let Some(context) = &vice_context {
            standalone_profiles.extend(context.profiles());
        }

        // Read-only openMSX discovery contributes only real executable
        // bindings. Core's launch planner selects the explicit C-BIOS machine
        // and refuses unsupported MSX media and identity.
        let openmsx_roots =
            archivefs_core::patch_manager::OpenMsxProfileDiscoveryRoots::from_environment();
        let openmsx_discovery =
            archivefs_core::patch_manager::discover_openmsx_profiles(&openmsx_roots);
        standalone_profiles.extend(openmsx_discovery.profiles.iter().map(|profile| {
            archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "openmsx",
                profile_id: profile.profile_id.clone(),
                profile_path: Some(profile.executable.path.clone()),
                eligible: profile.eligible,
                firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
            }
        }));

        // MAME has no profile directory; its lifecycle binding is the exact
        // executable selected through Emulator Setup, while its game
        // eligibility comes only from the persisted MAME join above.
        if let Some(executable) = self
            .readiness
            .emulator_setup_overrides
            .executable(crate::emulator_setup_overrides::OverridableEmulator::Mame)
            && !mame_set_resolutions.is_empty()
        {
            standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "mame",
                profile_id: format!("mame:{}", executable.display()),
                profile_path: Some(executable.to_path_buf()),
                eligible: true,
                firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
            });
        }

        // Fuse is a narrow, read-only ZX Spectrum adapter.  Discovery is
        // additive: no executable means no Fuse candidate, and the generic
        // planner still requires an independently resolved ZX Spectrum
        // identity plus a supported direct content path.
        let fuse_roots =
            archivefs_core::patch_manager::FuseProfileDiscoveryRoots::from_environment();
        let fuse_discovery = archivefs_core::patch_manager::discover_fuse_profiles(&fuse_roots);
        standalone_profiles.extend(fuse_discovery.profiles.iter().map(|profile| {
            archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "fuse",
                profile_id: profile.profile_id.clone(),
                profile_path: Some(profile.executable.clone()),
                eligible: profile.eligible,
                firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
            }
        }));

        // Tsugaru is an additive FM Towns candidate.  Its discovery remains
        // read-only and only becomes launchable when an explicit ROM
        // directory is configured; the shared planner still requires a
        // separately resolved FM Towns identity.
        let tsugaru_roots =
            archivefs_core::patch_manager::TsugaruProfileDiscoveryRoots::from_environment();
        let tsugaru_discovery =
            archivefs_core::patch_manager::discover_tsugaru_profiles(&tsugaru_roots);
        standalone_profiles.extend(tsugaru_discovery.profiles.iter().map(|profile| {
            archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "tsugaru",
                profile_id: profile.profile_id.clone(),
                profile_path: Some(profile.executable.path.clone()),
                eligible: profile.eligible,
                firmware: match profile.firmware {
                    archivefs_core::patch_manager::TsugaruFirmwareState::Verified => {
                        archivefs_core::launch::FirmwareReadiness::Verified
                    }
                    archivefs_core::patch_manager::TsugaruFirmwareState::PresentUnverified => {
                        archivefs_core::launch::FirmwareReadiness::PresentUnverified
                    }
                    archivefs_core::patch_manager::TsugaruFirmwareState::Missing => {
                        archivefs_core::launch::FirmwareReadiness::Missing
                    }
                    archivefs_core::patch_manager::TsugaruFirmwareState::Unknown => {
                        archivefs_core::launch::FirmwareReadiness::Unknown
                    }
                },
            }
        }));

        let xroar_roots =
            archivefs_core::patch_manager::XRoarProfileDiscoveryRoots::from_environment();
        let xroar_discovery = archivefs_core::patch_manager::discover_xroar_profiles(&xroar_roots);
        standalone_profiles.extend(
            xroar_discovery
                .profiles
                .iter()
                .filter(|profile| {
                    xroar_context
                        .as_ref()
                        .and_then(|context| context.result.as_ref().ok())
                        .is_some_and(|evidence| evidence.supports_machine(profile.machine))
                })
                .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "xroar",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.executable.path.clone()),
                    eligible: profile.eligible,
                    firmware: match profile.firmware {
                        archivefs_core::patch_manager::XRoarFirmwareState::Verified => {
                            archivefs_core::launch::FirmwareReadiness::Verified
                        }
                        archivefs_core::patch_manager::XRoarFirmwareState::PresentUnverified => {
                            archivefs_core::launch::FirmwareReadiness::PresentUnverified
                        }
                        archivefs_core::patch_manager::XRoarFirmwareState::Missing => {
                            archivefs_core::launch::FirmwareReadiness::Missing
                        }
                        archivefs_core::patch_manager::XRoarFirmwareState::Unknown => {
                            archivefs_core::launch::FirmwareReadiness::Unknown
                        }
                    },
                }),
        );

        // The remaining native adapters are additive inputs to the same
        // shared planner. Discovery and inspection are kept read-only and
        // bounded by their existing adapters; this block does not rebuild a
        // command, infer identity, or turn an executable's presence into a
        // Ready result. A later launch click still invokes that adapter's
        // own preflight and execution path.
        // Already-validated EmuWiz-managed AppImages (if any) - caller-
        // confirmed explicit executables fed into the *existing* per-adapter
        // discovery. The `install.json`-backed managed slice is swept once
        // here (bounded, read-only, no shell) and each adapter's executable
        // is projected from it by exact catalogue display name. Empty on any
        // machine without a managed install, so launch readiness there is
        // byte-for-byte unchanged. This adds no app-level state - it is
        // derived here, exactly like every other discovery call in this
        // function.
        let managed_appimage_installs = discover_managed_appimage_installations();
        let managed_ppsspp_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "PPSSPP");
        let managed_rpcs3_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "RPCS3");
        // The user's persisted RPCS3 executable choice (Emulator Setup). It
        // decides between several discovered AppImages; it never adds a
        // guessed candidate and is refused with an explanation when stale.
        let rpcs3_selected_executable: Option<PathBuf> = self
            .readiness
            .emulator_setup_overrides
            .executable(crate::emulator_setup_overrides::OverridableEmulator::Rpcs3)
            .map(Path::to_path_buf);
        let managed_duckstation_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "DuckStation");
        let managed_xemu_appimages: Vec<PathBuf> =
            managed_appimage_explicit_executables(&managed_appimage_installs, "xemu");
        let ppsspp_launch_roots =
            PpssppResolutionInputs::from_environment(&self.readiness.emulator_setup_overrides)
                .ok()
                .map(|inputs| {
                    let mut roots = inputs.roots;
                    roots
                        .explicit_executables
                        .extend(managed_ppsspp_appimages.iter().cloned());
                    roots
                });
        // Launch consumes the shared resolver's answer for DuckStation: only
        // the pair it authorises is offered, and a refusal blocks DuckStation
        // outright instead of falling back to a separate discovery.
        let duckstation_selection: Option<DuckStationLaunchSelection> =
            DuckStationResolutionInputs::from_environment(
                &self.readiness.emulator_setup_overrides,
                &managed_duckstation_appimages,
            )
            .ok()
            .map(|inputs| select_for_launch(&inputs));
        if let Some(selection) = &duckstation_selection {
            let discovery = &selection.discovery;
            let request = archivefs_core::patch_manager::DuckStationGameRequest {
                verified_ps1_serial: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::Ps1Serial(serial) => {
                        Some(serial.clone())
                    }
                    _ => None,
                }),
                ..Default::default()
            };
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                let inspection =
                    archivefs_core::patch_manager::inspect_duckstation_game_with_firmware_evidence(
                        profile,
                        &request,
                        pcsx2_firmware_evidence,
                    );
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "duckstation",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::duckstation_firmware_readiness(
                        inspection.inspection.health.bios,
                    ),
                }
            }));
            if matches!(
                selection.authority,
                DuckStationLaunchAuthority::Refused { .. }
            ) {
                // A visible, ineligible DuckStation entry: Launch is blocked
                // by the resolver's decision, not "no emulator found".
                standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "duckstation",
                    profile_id: "duckstation:shared-resolver-refused".to_string(),
                    profile_path: None,
                    eligible: false,
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                });
            }
        }
        if let Some(roots) = ppsspp_launch_roots.as_ref() {
            let discovery = archivefs_core::patch_manager::discover_ppsspp_profiles(roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "ppsspp",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        if let Ok(mut roots) =
            archivefs_core::patch_manager::Rpcs3ProfileDiscoveryRoots::from_environment()
        {
            roots.selected_executable = rpcs3_selected_executable.clone();
            let discovery = archivefs_core::patch_manager::discover_rpcs3_profiles(&roots);
            let request = archivefs_core::patch_manager::Rpcs3GameRequest {
                verified_ps3_title_id: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::Ps3TitleId(id) => {
                        Some(id.clone())
                    }
                    _ => None,
                }),
                ..Default::default()
            };
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                let inspection =
                    archivefs_core::patch_manager::inspect_rpcs3_game(profile, &request);
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "rpcs3",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::rpcs3_firmware_readiness(
                        &inspection.health.firmware,
                    ),
                }
            }));
        }
        if let Ok(roots) =
            archivefs_core::patch_manager::XemuProfileDiscoveryRoots::from_environment()
        {
            let discovery = archivefs_core::patch_manager::discover_xemu_profiles(&roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "xemu",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    // xemu's command planner performs its own MCPX/BIOS/
                    // EEPROM/HDD health validation during preflight.
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        if let Some(inspection) = self.readiness.cemu.inspection_for(focused) {
            standalone_profiles.extend(inspection.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "cemu",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.profile_path.clone()),
                    eligible: profile.command.is_some()
                        && cemu_context
                            .as_ref()
                            .is_some_and(|context| context.profile_is_bound(profile)),
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        if let Ok(roots) =
            archivefs_core::patch_manager::MelonDsProfileDiscoveryRoots::from_environment()
        {
            let discovery = archivefs_core::patch_manager::discover_melonds_profiles(&roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "melonds",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::melonds_firmware_readiness(&profile.firmware),
                }
            }));
        }
        if let Ok(roots) =
            archivefs_core::patch_manager::MgbaProfileDiscoveryRoots::from_environment()
        {
            let discovery = archivefs_core::patch_manager::discover_mgba_profiles(&roots);
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "mgba",
                    profile_id: profile.profile_id.clone(),
                    profile_path: profile.config_path.clone(),
                    eligible: archivefs_core::patch_manager::resolve_mgba_native_launch_binding(
                        profile,
                    )
                    .is_ok(),
                    // A BIOS is optional for ordinary GBA/GB/GBC launch;
                    // mGBA's own preflight retains any configured BIOS
                    // evidence without turning absence into a blocker.
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
        }
        let desmume_roots =
            archivefs_core::launch::DesmumeProfileDiscoveryRoots::from_environment();
        standalone_profiles.extend(
            archivefs_core::launch::discover_desmume_profiles(&desmume_roots)
                .profiles
                .iter()
                .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "desmume",
                    profile_id: profile.profile_id.clone(),
                    profile_path: profile
                        .executable
                        .as_ref()
                        .map(|executable| executable.path.clone()),
                    eligible: archivefs_core::launch::resolve_desmume_native_launch_binding(
                        profile,
                    )
                    .is_ok(),
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }),
        );
        let rmg_roots = archivefs_core::patch_manager::RmgProfileDiscoveryRoots::from_environment();
        standalone_profiles.extend(
            archivefs_core::patch_manager::discover_rmg_profiles(&rmg_roots)
                .profiles
                .iter()
                .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "rmg",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: archivefs_core::patch_manager::resolve_rmg_native_launch_binding(
                        profile,
                    )
                    .is_ok(),
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }),
        );
        let stella_roots =
            archivefs_core::patch_manager::StellaProfileDiscoveryRoots::from_environment();
        standalone_profiles.extend(
            archivefs_core::patch_manager::discover_stella_profiles(&stella_roots)
                .profiles
                .iter()
                .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "stella",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: archivefs_core::patch_manager::resolve_stella_native_launch_binding(
                        profile,
                    )
                    .is_ok(),
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }),
        );
        if let Ok(roots) =
            archivefs_core::patch_manager::SameBoyProfileDiscoveryRoots::from_environment()
        {
            standalone_profiles.extend(
                archivefs_core::patch_manager::discover_sameboy_profiles(&roots)
                    .profiles
                    .iter()
                    .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                        adapter_id: "sameboy",
                        profile_id: profile.profile_id.clone(),
                        profile_path: Some(profile.configuration_path.clone()),
                        eligible:
                            archivefs_core::patch_manager::resolve_sameboy_native_launch_binding(
                                profile,
                            )
                            .is_ok(),
                        firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                    }),
            );
        }
        if let Some(home) = std::env::var_os("HOME") {
            let roots = archivefs_core::patch_manager::AzaharDiscoveryRoots {
                explicit_executable: None,
                path: std::env::var_os("PATH"),
                config_root: Some(PathBuf::from(home).join(".config/azahar")),
            };
            if let Some(executable) =
                archivefs_core::patch_manager::discover_azahar_executable(&roots)
            {
                let profile =
                    archivefs_core::patch_manager::discover_azahar_profile(&roots, executable);
                standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "azahar",
                    profile_id: format!("azahar:{}", profile.executable.display()),
                    profile_path: profile.config.clone(),
                    eligible: archivefs_core::launch::azahar_profile_eligible(&profile),
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                });
            }
        }
        if let XeniaProfilesState::Ready(discovery) = &self.readiness.xenia_profiles {
            let (title_id, media_id) = game_identity_report
                .map(|report| {
                    (
                        report.verified_xex_title_id().map(str::to_owned),
                        report.verified_xex_media_id().map(str::to_owned),
                    )
                })
                .unwrap_or((None, None));
            standalone_profiles.extend(discovery.profiles.iter().map(|profile| {
                archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "xenia",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_path.clone()),
                    eligible: profile.eligible,
                    firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
                }
            }));
            let _ = (title_id, media_id);
        }

        // The focused game's verified media set(s), from the cached library
        // snapshot, for the shared multi-disc launch decision.
        let multidisc = focused.and_then(|path| {
            self.database_snapshot.and_then(|snapshot| {
                let sets =
                    crate::multidisc_launch_view::library_sets(snapshot, self.database_generation);
                crate::multidisc_launch_view::MultiDiscLaunchContext::for_content(
                    &sets,
                    path,
                    &snapshot.archives,
                )
                .map(|context| context.with_disc_members(&snapshot.disc_members))
            })
        });
        let duckstation_context = duckstation_selection.map(|selection| {
            launch_readiness_page::DuckStationLaunchContext {
                multidisc: multidisc.clone(),
                discovery: selection.discovery,
                roots: selection.roots,
                firmware_evidence: pcsx2_firmware_evidence.to_vec(),
                authority: selection.authority,
                verified_ps1_serial: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::Ps1Serial(serial) => {
                        Some(serial.clone())
                    }
                    _ => None,
                }),
            }
        });
        let ppsspp_context =
            ppsspp_launch_roots.map(|roots| launch_readiness_page::PpssppLaunchContext {
                discovery: archivefs_core::patch_manager::discover_ppsspp_profiles(&roots),
                roots,
                verified_psp_disc_id: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::PspDiscId(id) => Some(id.clone()),
                    _ => None,
                }),
                verified_standalone_pbp_sha256: verified_facts.iter().find_map(|fact| match fact {
                    archivefs_core::launch::VerifiedIdentityFact::PspStandalonePbpSha256(hash) => {
                        Some(hash.clone())
                    }
                    _ => None,
                }),
            });
        let rpcs3_context =
            archivefs_core::patch_manager::Rpcs3ProfileDiscoveryRoots::from_environment()
                .ok()
                .map(|mut roots| {
                    // Already-validated EmuWiz-managed RPCS3 AppImage (if
                    // any) as a caller-confirmed explicit executable. Empty
                    // -> no-op, behaviour unchanged.
                    roots
                        .explicit_executables
                        .extend(managed_rpcs3_appimages.iter().cloned());
                    roots.selected_executable = rpcs3_selected_executable.clone();
                    launch_readiness_page::Rpcs3LaunchContext {
                        discovery: archivefs_core::patch_manager::discover_rpcs3_profiles(&roots),
                        roots,
                        verified_ps3_title_id: verified_facts.iter().find_map(|fact| match fact {
                            archivefs_core::launch::VerifiedIdentityFact::Ps3TitleId(id) => {
                                Some(id.clone())
                            }
                            _ => None,
                        }),
                    }
                });
        let xemu_context =
            archivefs_core::patch_manager::XemuProfileDiscoveryRoots::from_environment()
                .ok()
                .map(|mut roots| {
                    // Already-validated EmuWiz-managed xemu AppImage (if any)
                    // as a caller-confirmed explicit executable. The Xbox
                    // BIOS/MCPX/EEPROM/HDD readiness stays independent - it
                    // is validated by the xemu command planner at preflight,
                    // not here. Empty -> no-op, behaviour unchanged.
                    roots
                        .explicit_executables
                        .extend(managed_xemu_appimages.iter().cloned());
                    launch_readiness_page::XemuLaunchContext {
                        discovery: archivefs_core::patch_manager::discover_xemu_profiles(&roots),
                        roots,
                        verified_xbox_title_id: verified_facts.iter().find_map(|fact| match fact {
                            archivefs_core::launch::VerifiedIdentityFact::XboxTitleId(id) => {
                                Some(id.clone())
                            }
                            _ => None,
                        }),
                    }
                });
        let melonds_context =
            archivefs_core::patch_manager::MelonDsProfileDiscoveryRoots::from_environment()
                .ok()
                .map(|roots| launch_readiness_page::MelonDsLaunchContext {
                    discovery: archivefs_core::patch_manager::discover_melonds_profiles(&roots),
                    roots,
                    verified_game_key: verified_facts.iter().find_map(|fact| match fact {
                        archivefs_core::launch::VerifiedIdentityFact::NintendoDsGameKey(key) => {
                            Some(key.clone())
                        }
                        _ => None,
                    }),
                });
        let xenia_context =
            if let XeniaProfilesState::Ready(discovery) = &self.readiness.xenia_profiles {
                let roots = archivefs_core::patch_manager::XeniaProfileDiscoveryRoots {
                    explicit_configuration_roots: discovery
                        .profiles
                        .iter()
                        .map(|profile| profile.configuration_path.clone())
                        .collect(),
                };
                game_identity_report.map(|report| launch_readiness_page::XeniaLaunchContext {
                    discovery: discovery.clone(),
                    roots,
                    verified_xex_title_id: report.verified_xex_title_id().map(str::to_owned),
                    verified_xex_media_id: report.verified_xex_media_id().map(str::to_owned),
                })
            } else {
                None
            };

        let standalone_scans_complete = match &identity_status {
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) => {
                match identity.platform_id.as_str() {
                    "GameCube" | "Wii" => {
                        matches!(
                            self.readiness.dolphin_local_profiles,
                            DolphinLocalProfilesState::Ready(_)
                        )
                    }
                    "PS2" => matches!(
                        self.readiness.pcsx2_launch_profiles,
                        Pcsx2LaunchProfilesState::Ready(_)
                    ),
                    "Xbox360" => {
                        matches!(self.readiness.xenia_profiles, XeniaProfilesState::Ready(_))
                    }
                    // These lanes are already discovered synchronously by
                    // this read-only input builder using their existing
                    // adapter roots.
                    "PSX" | "PSP" | "PS3" | "Xbox" => true,
                    "Amiga" => amiga_whdload_context.is_some() || amiga_adf_scanned,
                    _ => true,
                }
            }
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
            | archivefs_core::launch::CanonicalIdentityStatus::Conflicting => false,
        };

        let remembered: Vec<archivefs_core::launch::RememberedPreference> = self
            .readiness
            .remembered_emulator_profiles
            .iter()
            .map(|profile| archivefs_core::launch::RememberedPreference {
                adapter_id: profile.adapter.clone(),
                profile_id: profile.profile_id.clone(),
            })
            .collect();
        #[cfg(target_os = "linux")]
        let shadps4_context = if matches!(&identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) if identity.platform_id == "PS4")
        {
            focused.map(|path| {
                crate::emulator_setup::shadps4::Controller::snapshot(
                    &self.readiness.shadps4,
                    crate::emulator_setup::shadps4::Selection {
                        path: path.to_path_buf(),
                        identity: identity_status.clone(),
                    },
                )
            })
        } else {
            None
        };
        // ScummVM has no profile directory: the candidate is the one safe native
        // executable the detector already resolves, offered only for a resolved
        // ScummVM identity. Core re-detects and re-checks it at launch.
        if matches!(&identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity) if identity.platform_id == "ScummVM")
            // The same verified engine:game gate the shared integration uses.
            && matches!(
                archivefs_core::launch::project_scummvm_launch_input(&verified_facts),
                archivefs_core::launch::LaunchInputProjection::Authorized(_)
            )
            && let Some(executable) =
                archivefs_core::scummvm_detection::resolve_scummvm_executable()
        {
            standalone_profiles.push(archivefs_core::launch::StandaloneProfileInput {
                adapter_id: "scummvm",
                profile_id: format!("scummvm:{}", executable.display()),
                profile_path: Some(executable),
                eligible: true,
                firmware: archivefs_core::launch::FirmwareReadiness::NotRequired,
            });
        }
        let mesen_context = match (&identity_status, focused) {
            (archivefs_core::launch::CanonicalIdentityStatus::Resolved(identity), Some(path))
                if identity.platform_id == "NES" =>
            {
                let snapshot = crate::emulator_setup::mesen::Controller::snapshot(
                    &self.readiness.mesen,
                    crate::emulator_setup::mesen::Selection {
                        path: path.into(),
                        identity: identity_status.clone(),
                    },
                );
                standalone_profiles.extend(snapshot.inputs());
                Some(snapshot)
            }
            _ => None,
        };
        let mut plan = archivefs_core::launch::build_launch_plan(
            &identity_status,
            &content,
            &standalone_profiles,
            retroarch_environment,
            &remembered,
        );
        launch_readiness_page::saturn_core::apply(&mut plan, retroarch_environment);
        archivefs_core::launch::nkit_readiness::inspect_and_apply(&mut plan);
        archivefs_core::launch::cemu_readiness::apply_cemu_readiness(
            &mut plan,
            self.readiness.cemu.inspection_for(focused),
        );
        if let Some(context) = &flycast_context {
            context.apply(&mut plan);
        }
        if let Some(context) = &vice_context {
            context.apply(&mut plan);
        }
        if let Some(context) = &cemu_context {
            context.apply(&mut plan);
        }
        if let Some(context) = &amiga_whdload_context {
            launch_readiness_page::amiga_boot::apply(&mut plan, context);
        }
        launch_readiness_page::apply_mame_strict_preflight(&mut plan);
        if let Some(snapshot) = &mesen_context {
            snapshot.apply(&mut plan);
        }
        #[cfg(target_os = "linux")]
        if let Some(snapshot) = &shadps4_context {
            snapshot.apply(&mut plan, &content);
        }
        #[cfg(target_os = "linux")]
        let standalone_scans_complete = shadps4_context
            .as_ref()
            .map_or(standalone_scans_complete, |snapshot| {
                snapshot.inspection.is_some() || snapshot.error.is_some()
            });
        LaunchReadinessInput::Plan {
            plan,
            retroarch: match &self.readiness.retroarch_profiles {
                RetroArchProfilesState::Ready(discovery) => Some(
                    launch_readiness_page::retroarch_launch_context(&discovery.environment)
                        .with_multidisc(multidisc.clone()),
                ),
                RetroArchProfilesState::NotScanned
                | RetroArchProfilesState::Scanning { .. }
                | RetroArchProfilesState::Error(_) => None,
            },
            retroarch_scanned,
            standalone_scans_complete,
            dolphin: dolphin_context,
            pcsx2: pcsx2_context,
            duckstation: duckstation_context,
            ppsspp: ppsspp_context,
            rpcs3: rpcs3_context,
            xemu: xemu_context,
            xenia: xenia_context,
            melonds: melonds_context,
            cemu: cemu_context,
            flycast: flycast_context,
            xroar: xroar_context,
            vice: vice_context,
            amiga_whdload: amiga_whdload_context,
            #[cfg(target_os = "linux")]
            shadps4: shadps4_context,
        }
    }
}
