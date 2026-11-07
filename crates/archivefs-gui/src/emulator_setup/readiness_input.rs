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
    /// True only for a catalogued `arcade_set_directory` at exactly this path.
    pub(crate) is_arcade_set_directory: &'a dyn Fn(&Path) -> bool,
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
        // An extracted Arcade set folder is routed by the installed MAME's own
        // per-set data (driver status, ROM verify incl. extra read-only
        // rompaths), not by the old DAT join. No category is a gate.
        let mame_executable = self
            .readiness
            .emulator_setup_overrides
            .executable(crate::emulator_setup_overrides::OverridableEmulator::Mame);
        // Only a catalogued Arcade set folder is ever handed to MAME here; any
        // other game (ScummVM, DOS, PS3 ...) never reaches the probe and its
        // identity is left exactly as the other routes resolved it.
        let arcade_target =
            arcade_probe_target(focused, self.is_arcade_set_directory, mame_executable);
        // Selection moved on: stop foreground probes for any other folder.
        crate::arcade_mame_probe::cancel_foreground_except(arcade_target.map(|(path, _)| path));
        let mut arcade_probe = None;
        if let Some((path, executable)) = arcade_target {
            match crate::arcade_mame_probe::lookup(executable, path) {
                Some(crate::arcade_mame_probe::Lookup::Checking) => {
                    // Neither Ready nor Blocked: shown as "checking" and
                    // re-polled when the background probe wakes the UI.
                    return LaunchReadinessInput::EvidenceNotLoaded;
                }
                Some(crate::arcade_mame_probe::Lookup::Done(probe)) => arcade_probe = Some(probe),
                None => {}
            }
        }
        if let Some(probe) = &arcade_probe {
            identity_status = archivefs_core::launch::CanonicalIdentityStatus::Resolved(
                archivefs_core::launch::ResolvedIdentity {
                    platform_id: "Arcade".to_string(),
                    game_key: probe.set.clone(),
                },
            );
            mame_set_resolutions = vec![probe.resolution.clone()];
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
        let azahar_context = self.readiness.azahar_homebrew.context(focused);
        // Never impose Nintendo 3DS on a file that other evidence places on
        // another platform.
        let azahar_inputs = azahar_context
            .as_ref()
            .filter(|_| {
                selected_game_identity_report.is_none_or(|r| {
                    matches!(
                        r.platform,
                        archivefs_core::game_identity::IdentityPlatform::ThreeDS
                            | archivefs_core::game_identity::IdentityPlatform::Other
                    )
                }) && crate::launch_readiness_page::azahar::platform_allows(
                    focused_record.and_then(|r| r.mount_plan.archive.identity.platform.as_deref()),
                )
            })
            .and_then(|c| c.inputs(&identity_status));
        if let Some((identity, _)) = &azahar_inputs {
            identity_status = identity.clone();
        }
        let vita3k_context = self.readiness.vita3k_library.context(focused);
        // Never impose PlayStation Vita on a file other evidence places elsewhere.
        let vita3k_inputs = vita3k_context
            .as_ref()
            .filter(|_| {
                selected_game_identity_report.is_none_or(|r| {
                    matches!(
                        r.platform,
                        archivefs_core::game_identity::IdentityPlatform::Other
                    )
                }) && crate::launch_readiness_page::vita3k::platform_allows(
                    focused_record.and_then(|r| r.mount_plan.archive.identity.platform.as_deref()),
                )
            })
            .and_then(|c| c.inputs(&identity_status));
        if let Some((identity, _)) = &vita3k_inputs {
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
        // Owner-approved controlled path (2026-11-07): a recognised, safely
        // inspected ZX tape image may launch with a visible "identity
        // unverified" warning instead of requiring DAT or header identity.
        let mut identity_unverified = false;
        if matches!(
            identity_status,
            archivefs_core::launch::CanonicalIdentityStatus::Unknown
        ) && !evidence_freshness.is_incomplete()
            && let Some(path) = focused
            && let Some(hint) = focused_record
                .and_then(|record| record.mount_plan.archive.identity.platform.as_deref())
            && let Some(resolved) = crate::emulator_setup::unverified_media::resolve(path, hint)
        {
            identity_status = archivefs_core::launch::CanonicalIdentityStatus::Resolved(resolved);
            identity_unverified = true;
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
                    LaunchReadinessInput::IdentityUnknown(identity_unknown_reason(
                        focused,
                        selected_game_identity_report,
                        focused_record.map(|r| r.mount_plan.archive.identity.platform.as_deref()),
                    ))
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
            apply_mame_set_content(&mut content, focused);
        }

        // A ZIP holding exactly one verified cartridge ROM is launchable
        // through RetroArch by extracting that member at Play time.
        if content.requires_mount
            && let Some(zipped) = selected_game_identity_report
                .filter(|report| focused.is_some_and(|path| report.archive_path == path))
                .and_then(archivefs_core::launch::launch_content_ref_for_zipped_cartridge)
        {
            content = zipped;
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
        if let Some((_, selected_content)) = azahar_inputs {
            content = selected_content;
        }
        if let Some((_, selected_content)) = vita3k_inputs.as_ref() {
            content = selected_content.clone();
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
        // A Dolphin profile is a launch option only when a launch binding can
        // be proven for it now; otherwise it is blocked with that reason and
        // never ties with an option that can actually start.
        let dolphin_launch_profiles = dolphin_context.as_ref().map(|context| {
            archivefs_core::launch::dolphin_binding_readiness::dolphin_launch_profiles(
                &context.discovery,
                &context.roots,
            )
        });
        let dolphin_standalone_profiles: Vec<archivefs_core::launch::StandaloneProfileInput> =
            dolphin_launch_profiles
                .as_ref()
                .map(|profiles| profiles.inputs.clone())
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
        let snes9x_roots =
            archivefs_core::patch_manager::Snes9xProfileDiscoveryRoots::from_environment();
        standalone_profiles.extend(
            archivefs_core::patch_manager::discover_snes9x_profiles(&snes9x_roots)
                .profiles
                .iter()
                .map(|profile| archivefs_core::launch::StandaloneProfileInput {
                    adapter_id: "snes9x",
                    profile_id: profile.profile_id.clone(),
                    profile_path: None,
                    eligible: archivefs_core::patch_manager::resolve_snes9x_native_launch_binding(
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
        // The installation comes from the worker's own scan of Vita3K (the
        // very entry Play will recheck), only when the identity was supplied.
        if vita3k_inputs.is_some()
            && let Some(profile) = vita3k_context.as_ref().and_then(|c| c.profile_input())
        {
            standalone_profiles.push(profile);
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
        // Only RetroArch can launch a zipped cartridge; a standalone emulator
        // would be handed the ZIP itself, which its preflight refuses.
        if content.provenance == archivefs_core::launch::ZIPPED_CARTRIDGE_PROVENANCE {
            for candidate in &mut plan.candidates {
                if matches!(
                    candidate.target,
                    archivefs_core::launch::LaunchTarget::Standalone { .. }
                ) {
                    candidate.readiness = archivefs_core::launch::LaunchReadiness::Blocked;
                    candidate
                        .blockers
                        .push(archivefs_core::launch::LaunchBlocker::new(
                            archivefs_core::launch::LaunchBlockerKind::ContentNotResolved,
                            "a zipped cartridge launches through RetroArch only",
                        ));
                }
            }
        }
        launch_readiness_page::saturn_core::apply(&mut plan, retroarch_environment);
        if let Some(profiles) = &dolphin_launch_profiles {
            archivefs_core::launch::dolphin_binding_readiness::apply(&mut plan, profiles);
        }
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
        if identity_unverified {
            crate::emulator_setup::unverified_media::apply_warning(&mut plan);
        }
        if let Some(probe) = &arcade_probe {
            crate::arcade_mame_probe::apply_warnings(&mut plan, probe);
        }
        refine_arcade_set_blocker(
            &mut plan,
            self.readiness
                .emulator_setup_overrides
                .executable(crate::emulator_setup_overrides::OverridableEmulator::Mame)
                .is_some(),
        );
        if let Some(snapshot) = &mesen_context {
            snapshot.apply(&mut plan);
        }
        #[cfg(target_os = "linux")]
        if let Some(snapshot) = &shadps4_context {
            snapshot.apply(&mut plan, &content);
        }
        // Last: an explicit emulator choice only resolves what every check
        // above left usable, and never unblocks anything.
        archivefs_core::launch::launch_choice::apply(
            &mut plan,
            self.readiness.launch_choices.inputs(),
        );
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
            azahar: azahar_context,
            vita3k: vita3k_context,
            vice: vice_context,
            amiga_whdload: amiga_whdload_context,
            #[cfg(target_os = "linux")]
            shadps4: shadps4_context,
        }
    }
}

/// Archive container extensions EmuWiz may meet but has no identity route for.
fn is_archive_container(path: &std::path::Path) -> bool {
    path.extension().and_then(|x| x.to_str()).is_some_and(|x| {
        matches!(
            x.to_ascii_lowercase().as_str(),
            "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst"
        )
    })
}

/// Platforms (canonical ids) whose identity is resolved by a route other than
/// the shared identity inspection, before the "unknown" outcome is reached
/// (MAME arcade join, DeSmuME, DOSBox, Vita3K, tape/disc adapters and the ZX
/// `.z80` snapshot adapter). They must never be reported as having no
/// identity check, even though they have no `IdentityPlatform`. Platforms
/// that do have an `IdentityPlatform` never reach this check.
fn has_other_identity_route(id: &str, extension: &str) -> bool {
    match id {
        "ZX Spectrum" => extension == "z80",
        "NeoGeo"
        | "NeoGeo64"
        | "Nintendo DS"
        | "DOS"
        | "PC"
        | "PlayStation Vita"
        | "Dragon / Tandy CoCo"
        | "FM Towns"
        | "AmigaCD32"
        | "Commodore CDTV" => true,
        _ => false,
    }
}

/// True for platforms whose shared identity check reads only loose ROM files
/// (cartridge families), derived from `supported_loose_rom_format` itself so a
/// newly supported platform or extension is picked up automatically.
fn is_loose_rom_platform(platform: archivefs_core::game_identity::IdentityPlatform) -> bool {
    // Generous list of ROM/cartridge extensions; the test
    // `loose_rom_probe_covers_every_accepted_extension` brute-forces all
    // 1-3 character extensions against `supported_loose_rom_format`, so a
    // platform or extension added there without a probe entry fails loudly.
    const PROBE: &[&str] = &[
        "md", "gen", "smd", "bin", "sfc", "smc", "nes", "fds", "unf", "gb", "gbc", "gba", "vb",
        "vboy", "min", "sv", "z64", "v64", "n64", "a26", "a52", "a78", "atr", "atx", "xex", "xfd",
        "lnx", "lyx", "j64", "jag", "ngp", "ngc", "pce", "sms", "gg", "32x", "col", "cv", "sg",
        "rom",
    ];
    PROBE.iter().any(|ext| {
        archivefs_core::game_identity::supported_loose_rom_format(
            std::path::Path::new(&format!("x.{ext}")),
            platform,
        )
        .is_some()
    })
}

/// Why identity is unknown, from existing evidence only.
///
/// `recorded_platform` is the live library record's platform hint, if the
/// record exists at all (`None` outer = no live record, so "no system" cannot
/// be claimed). "System not chosen" means no platform hint is recorded and
/// none was identified; it is a reading of that evidence, not a trust grant.
/// A ZIP is never called unsupported (it is native content on many systems);
/// a ZIP with no platform is a missing system because choosing one makes it
/// inspectable, otherwise it stays "not confirmed". Only other containers
/// (7z, rar, ...) have no identity route.
pub(crate) fn identity_unknown_reason(
    focused: Option<&std::path::Path>,
    report: Option<&archivefs_core::game_identity::GameIdentityReport>,
    recorded_platform: Option<Option<&str>>,
) -> crate::launch_readiness_page::IdentityUnknownReason {
    use crate::launch_readiness_page::IdentityUnknownReason as Reason;
    use archivefs_core::game_identity::{IdentityImageFormat, IdentityPlatform};
    let hint = recorded_platform.flatten();
    let platform = match report.map(|r| r.platform) {
        Some(p) if p != IdentityPlatform::Other => p,
        _ => IdentityPlatform::from_catalogue(hint),
    };
    let platform_known = platform != IdentityPlatform::Other
        || hint.is_some_and(|p| archivefs_core::canonical_platform_for_alias(p).is_some());
    let system_missing = if recorded_platform.is_some() && !platform_known {
        Reason::SystemNotChosen
    } else {
        Reason::NotConfirmed
    };
    // Blocker-specific reasons come first and are derived from facts only:
    // the platform, the file extension and the container kind. When no
    // identity route exists, extracting an archive or opening Verify identity
    // cannot help, so the truthful reason must win.
    let extension = focused
        .and_then(|p| p.extension())
        .and_then(|x| x.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let archive = focused.is_some_and(is_archive_container);
    if platform == IdentityPlatform::Amiga {
        match extension.as_str() {
            "hdf" => return Reason::AmigaHardDisk,
            "ipf" => return Reason::AmigaIpf,
            _ => {}
        }
    }
    // A known system with no IdentityPlatform (so no shared identity check and
    // no launch platform id) and no other identity route.
    if platform == IdentityPlatform::Other
        && let Some(system) = hint.and_then(archivefs_core::canonical_platform_for_alias)
        && !has_other_identity_route(system, &extension)
    {
        return Reason::NoIdentityCheck(system);
    }
    // The system has an identity check but it reads only loose ROM files and
    // this loose file is not one it reads. Archives keep the archive advice.
    if platform != IdentityPlatform::Other
        && !archive
        && is_loose_rom_platform(platform)
        && report.is_some_and(|r| r.format == IdentityImageFormat::Unsupported)
        && let Some(path) = focused
        && archivefs_core::game_identity::supported_loose_rom_format(path, platform).is_none()
    {
        return Reason::UnsupportedFormat(platform.label());
    }
    if let (Some(path), Some(report)) = (focused, report)
        && report.format == IdentityImageFormat::Unsupported
        && is_archive_container(path)
    {
        // A ZIP is native content on many systems (arcade sets must stay
        // zipped), so it is never "unsupported"; it falls through to the
        // system / not-confirmed reasons below.
        let is_zip = path
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("zip"));
        if !is_zip {
            return Reason::UnsupportedArchive;
        }
    }
    if !platform_known {
        return system_missing;
    }
    Reason::NotConfirmed
}

/// The only case in which MAME is asked about a folder: a catalogued Arcade
/// set directory AND a configured MAME. Any other game yields `None`, so the
/// probe never runs and identity/set resolutions are never overwritten.
fn arcade_probe_target<'a>(
    focused: Option<&'a Path>,
    is_arcade_set_directory: &dyn Fn(&Path) -> bool,
    executable: Option<&'a Path>,
) -> Option<(&'a Path, &'a Path)> {
    let path = focused.filter(|path| is_arcade_set_directory(path))?;
    Some((path, executable?))
}

/// Says precisely what is missing for the MAME route when RetroArch refused an
/// extracted arcade set and no MAME candidate exists. Wording only: nothing is
/// unblocked and no candidate is added.
fn refine_arcade_set_blocker(plan: &mut archivefs_core::launch::LaunchPlan, mame_configured: bool) {
    use archivefs_core::launch::{LaunchBlockerKind, LaunchReadiness, LaunchTarget};
    let mame = plan.candidates.iter().find(|candidate| {
        matches!(
            candidate.target,
            LaunchTarget::Standalone {
                adapter_id: "mame",
                ..
            }
        )
    });
    let advice = match mame {
        Some(mame) if mame.readiness != LaunchReadiness::Blocked => return,
        Some(mame) => format!(
            "MAME is set up but cannot start this set yet: {}.",
            mame.blockers
                .first()
                .map_or("see its readiness details", |blocker| blocker.detail.as_str())
        ),
        None if !mame_configured => {
            "Choose the MAME program in Emulator Setup to play this copy.".to_string()
        }
        None => "MAME is set up, but this set has no current verified MAME record yet; run the MAME check for this folder first.".to_string(),
    };
    for candidate in &mut plan.candidates {
        if !matches!(candidate.target, LaunchTarget::RetroArchCore { .. }) {
            continue;
        }
        for blocker in &mut candidate.blockers {
            if blocker.kind == LaunchBlockerKind::RetroArchArcadeSetNeedsArchive {
                blocker.detail = format!("{} {advice}", blocker.detail);
            }
        }
    }
}

/// Points `content` at the MAME logical set the persisted DAT join resolved.
/// The join says which set this is, not what shape it has: an extracted set
/// directory keeps the `ExtractedArcadeSet` shape, so RetroArch cores (which
/// cannot load a directory) are refused with the true reason instead of being
/// shown as Ready.
fn apply_mame_set_content(
    content: &mut archivefs_core::launch::LaunchContentRef,
    focused: Option<&Path>,
) {
    content.resolved_path = focused.map(Path::to_path_buf);
    if focused.is_some_and(|path| std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())) {
        content.kind = Some(archivefs_core::launch::LaunchContentKind::ExtractedArcadeSet);
    }
    content.container = Some(archivefs_core::launch::LaunchContainerKind::PlainFile);
    content.requires_mount = false;
    content.provenance = "verified MAME logical-set directory from the persisted DAT join".into();
}

#[cfg(test)]
mod identity_reason_tests {
    use super::identity_unknown_reason;
    use crate::launch_readiness_page::IdentityUnknownReason as R;
    use archivefs_core::game_identity::{
        GameIdentityReport, IdentityImageFormat, IdentityPlatform,
    };
    use std::path::Path;

    fn report(platform: IdentityPlatform, format: IdentityImageFormat) -> GameIdentityReport {
        GameIdentityReport {
            archive_path: "x".into(),
            platform,
            format,
            evidence: Vec::new(),
            warnings: Vec::new(),
            bytes_read: 0,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        }
    }

    fn unsupported(platform: IdentityPlatform) -> GameIdentityReport {
        report(platform, IdentityImageFormat::Unsupported)
    }

    #[test]
    fn non_zip_archive_is_unsupported() {
        let r = unsupported(IdentityPlatform::Other);
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.7z")), Some(&r), Some(None)),
            R::UnsupportedArchive
        );
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.rar")), Some(&r), Some(Some("GBA"))),
            R::UnsupportedArchive
        );
    }

    #[test]
    fn failed_or_ambiguous_cartridge_zip_is_never_called_unsupported() {
        // Platform known via the record hint, report still at Unsupported.
        let r = unsupported(IdentityPlatform::Other);
        assert_eq!(
            identity_unknown_reason(
                Some(Path::new("/g/a.zip")),
                Some(&r),
                Some(Some("Game Boy Advance"))
            ),
            R::NotConfirmed
        );
        // Platform known via the report.
        let r = unsupported(IdentityPlatform::GameBoyAdvance);
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.zip")), Some(&r), Some(None)),
            R::NotConfirmed
        );
    }

    #[test]
    fn zip_without_platform_is_choose_system() {
        let r = unsupported(IdentityPlatform::Other);
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.zip")), Some(&r), Some(None)),
            R::SystemNotChosen
        );
    }

    #[test]
    fn zip_on_a_platform_without_an_identity_variant_is_never_unsupported() {
        // NeoGeo has its own MAME-join route, so it is never called
        // "no identity check"; Sharp X68000 has no route at all.
        let r = unsupported(IdentityPlatform::Other);
        let reason_for = |hint| {
            identity_unknown_reason(Some(Path::new("/g/kof98.zip")), Some(&r), Some(Some(hint)))
        };
        assert_eq!(reason_for("NeoGeo"), R::NotConfirmed);
        assert_eq!(
            reason_for("Sharp X68000"),
            R::NoIdentityCheck("Sharp X68000")
        );
    }

    #[test]
    fn no_live_record_never_claims_system_not_chosen() {
        let r = unsupported(IdentityPlatform::Other);
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.zip")), Some(&r), None),
            R::NotConfirmed
        );
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.bin")), None, None),
            R::NotConfirmed
        );
    }

    #[test]
    fn no_platform_anywhere_means_choose_system() {
        let r = unsupported(IdentityPlatform::Other);
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.bin")), Some(&r), Some(None)),
            R::SystemNotChosen
        );
        assert_eq!(
            identity_unknown_reason(
                Some(Path::new("/g/a.bin")),
                None,
                Some(Some("not a platform"))
            ),
            R::SystemNotChosen
        );
    }

    #[test]
    fn known_platform_without_confirmation_is_not_confirmed() {
        let r = report(IdentityPlatform::Other, IdentityImageFormat::Deferred);
        assert_eq!(
            identity_unknown_reason(
                Some(Path::new("/g/a.gba")),
                Some(&r),
                Some(Some("Game Boy Advance"))
            ),
            R::NotConfirmed
        );
        let known = report(IdentityPlatform::PlayStation, IdentityImageFormat::Deferred);
        assert_eq!(
            identity_unknown_reason(Some(Path::new("/g/a.bin")), Some(&known), Some(None)),
            R::NotConfirmed
        );
    }

    fn reason(path: &str, platform: IdentityPlatform, hint: Option<&str>) -> R {
        let r = unsupported(platform);
        identity_unknown_reason(Some(Path::new(path)), Some(&r), Some(hint))
    }

    #[test]
    fn systems_without_an_identity_check_get_the_truthful_reason_first() {
        let other = IdentityPlatform::Other;
        for (path, hint, name) in [
            ("/g/game.zip", "Sharp X68000", "Sharp X68000"),
            ("/g/game.7z", "Sharp X68000", "Sharp X68000"),
            ("/g/game.7z", "NEC PC-8801", "NEC PC-8801"),
            ("/g/game.tap", "ZX Spectrum", "ZX Spectrum"),
            ("/g/game.zip", "MSX", "MSX"),
            ("/g/game.rom", "MSX2", "MSX2"),
            ("/g/game.zip", "NEC PC-9801", "NEC PC-9801"),
            ("/g/game.ssd", "BBC Micro", "BBC Micro"),
            ("/g/game.adf", "Acorn Archimedes", "Acorn Archimedes"),
        ] {
            assert_eq!(
                reason(path, other, Some(hint)),
                R::NoIdentityCheck(name),
                "{path}"
            );
        }
    }

    /// Systems that are meant to stay unsupported for now. If one gains an
    /// `IdentityPlatform` this fails loudly: prune it here and add the new
    /// platform's own format tests (the reason logic itself is derived).
    #[test]
    fn systems_meant_to_stay_unsupported_have_no_identity_platform() {
        for id in [
            "Sharp X68000",
            "NEC PC-8801",
            "PC-98",
            "NEC PC-9801",
            "MSX",
            "MSX2",
            "BBC Micro",
            "Acorn Archimedes",
            "Acorn Electron",
            "ZX Spectrum",
        ] {
            assert_eq!(
                IdentityPlatform::from_catalogue(Some(id)),
                IdentityPlatform::Other,
                "{id} now has an IdentityPlatform: update this list and add format tests"
            );
        }
    }

    /// Merge-order independent: for any system, `NoIdentityCheck` holds exactly
    /// when the facts say so (no `IdentityPlatform`, no launch platform id, no
    /// other identity route), and a system with an `IdentityPlatform` never
    /// gets it - whichever cartridge-family changes have landed.
    #[test]
    fn no_identity_check_is_exactly_what_the_platform_facts_say() {
        use archivefs_core::launch::evidence_bridge::launch_platform_id;
        for id in [
            "Sharp X68000",
            "NEC PC-8801",
            "NEC PC-9801",
            "PC-98",
            "MSX",
            "MSX2",
            "BBC Micro",
            "Acorn Archimedes",
            "Acorn Electron",
            "ZX Spectrum",
            "PC Engine",
            "TurboGrafx-16",
            "PC Engine CD",
            "MasterSystem",
            "GameGear",
            "Sega 32X",
            "ColecoVision",
            "Atari2600",
            "Atari5200",
            "Atari7800",
            "Atari Lynx",
            "AtariST",
            "MegaDrive",
            "SNES",
            "NES",
            "GBA",
            "Amiga",
            "NeoGeo",
            "Nintendo DS",
            "DOS",
            "WiiU",
            "Arcade",
            "Amstrad CPC",
            "Oric",
        ] {
            let identity = IdentityPlatform::from_catalogue(Some(id));
            let has_route = archivefs_core::canonical_platform_for_alias(id)
                .is_some_and(|c| super::has_other_identity_route(c, "xyz"));
            let expected = identity == IdentityPlatform::Other
                && launch_platform_id(identity).is_none()
                && archivefs_core::canonical_platform_for_alias(id).is_some()
                && !has_route;
            let got = reason("/g/game.xyz", IdentityPlatform::Other, Some(id));
            assert_eq!(
                matches!(got, R::NoIdentityCheck(_)),
                expected,
                "{id}: {got:?}"
            );
            if identity != IdentityPlatform::Other {
                assert!(!matches!(got, R::NoIdentityCheck(_)), "{id}");
            }
        }
    }

    #[test]
    fn archives_of_a_platform_with_an_identity_check_keep_the_extract_advice() {
        assert_eq!(
            reason("/g/game.7z", IdentityPlatform::Atari2600, None),
            R::UnsupportedArchive
        );
        assert_eq!(
            reason("/g/game.7z", IdentityPlatform::GameBoyAdvance, None),
            R::UnsupportedArchive
        );
        assert_eq!(
            reason("/g/game.7z", IdentityPlatform::Other, Some("NEC PC-8801")),
            R::NoIdentityCheck("NEC PC-8801")
        );
        assert_eq!(
            reason("/g/game.zip", IdentityPlatform::Atari2600, None),
            R::NotConfirmed
        );
    }

    #[test]
    fn zx_snapshot_route_other_routes_and_supported_platforms_are_unaffected() {
        let other = IdentityPlatform::Other;
        assert_eq!(
            reason("/g/a.z80", other, Some("ZX Spectrum")),
            R::NotConfirmed
        );
        for hint in ["NeoGeo", "Nintendo DS", "DOS", "WiiU"] {
            assert_eq!(
                reason("/g/a.zip", other, Some(hint)),
                R::NotConfirmed,
                "{hint}"
            );
        }
        assert_eq!(
            reason("/g/a.zip", IdentityPlatform::GameBoyAdvance, None),
            R::NotConfirmed
        );
        assert_eq!(reason("/g/a.7z", other, Some("GBA")), R::UnsupportedArchive);
        assert_eq!(reason("/g/a.zip", other, None), R::SystemNotChosen);
    }

    #[test]
    fn amiga_hdf_and_ipf_route_to_the_honest_amiga_reasons() {
        let amiga = IdentityPlatform::Amiga;
        assert_eq!(reason("/g/a.hdf", amiga, None), R::AmigaHardDisk);
        assert_eq!(reason("/g/a.IPF", amiga, None), R::AmigaIpf);
        assert_eq!(
            reason("/g/a.hdf", IdentityPlatform::Other, Some("Amiga")),
            R::AmigaHardDisk
        );
        assert_eq!(reason("/g/a.adf", amiga, None), R::NotConfirmed);
    }

    #[test]
    fn loose_files_a_cartridge_identity_check_does_not_read_are_an_unsupported_format() {
        for (path, platform, name) in [
            ("/g/a.bin", IdentityPlatform::Atari2600, "Atari 2600"),
            ("/g/a.bin", IdentityPlatform::Atari5200, "Atari 5200"),
            ("/g/a.rom", IdentityPlatform::Atari7800, "Atari 7800"),
            ("/g/a.bin", IdentityPlatform::AtariLynx, "Atari Lynx"),
        ] {
            assert_eq!(
                reason(path, platform, None),
                R::UnsupportedFormat(name),
                "{path}"
            );
        }
        assert_eq!(
            reason("/g/a.a26", IdentityPlatform::Atari2600, None),
            R::NotConfirmed
        );
    }

    #[test]
    fn vita_and_other_routed_systems_are_never_called_unsupported() {
        for hint in ["PlayStation Vita", "NeoGeo", "Nintendo DS", "DOS"] {
            assert_eq!(
                reason("/g/title.vpk", IdentityPlatform::Other, Some(hint)),
                R::NotConfirmed,
                "{hint}"
            );
        }
    }

    #[test]
    fn loose_rom_probe_covers_every_accepted_extension() {
        use archivefs_core::game_identity::supported_loose_rom_format;
        let mut exts: Vec<String> = ["vboy"].iter().map(|e| e.to_string()).collect();
        let alnum: Vec<char> = ('a'..='z').chain('0'..='9').collect();
        for &a in &alnum {
            exts.push(a.to_string());
            for &b in &alnum {
                exts.push(format!("{a}{b}"));
                for &c in &alnum {
                    exts.push(format!("{a}{b}{c}"));
                }
            }
        }
        for alias in [
            "MegaDrive",
            "SNES",
            "NES",
            "Game Boy",
            "Game Boy Color",
            "GBA",
            "Virtual Boy",
            "Pokemon Mini",
            "Watara Supervision",
            "N64",
            "Atari2600",
            "Atari5200",
            "Atari7800",
            "Atari 8-bit",
            "Atari Lynx",
            "Atari Jaguar",
            "Neo Geo Pocket",
            "Neo Geo Pocket Color",
            "MasterSystem",
            "GameGear",
            "Sega 32X",
            "ColecoVision",
            "TurboGrafx-16",
            "PC Engine",
        ] {
            let platform = IdentityPlatform::from_catalogue(Some(alias));
            if platform == IdentityPlatform::Other {
                continue;
            }
            let accepts = exts.iter().any(|e| {
                supported_loose_rom_format(Path::new(&format!("x.{e}")), platform).is_some()
            });
            assert_eq!(super::is_loose_rom_platform(platform), accepts, "{alias}");
        }
    }
}

#[cfg(test)]
mod arcade_set_shape_tests {
    use super::*;
    use archivefs_core::launch::{LaunchContentKind, LaunchContentRef};

    fn unresolved() -> LaunchContentRef {
        LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: None,
            requires_mount: false,
            provenance: "placeholder".into(),
        }
    }

    fn arcade_plan(mame_blocker: Option<&str>) -> archivefs_core::launch::LaunchPlan {
        use archivefs_core::emulator_environment::retroarch::{
            ProfileKind, ProfileRef, ProfileScope,
        };
        use archivefs_core::launch::{
            CandidatePreference, FirmwareReadiness, LaunchBlocker, LaunchBlockerKind,
            LaunchCandidate, LaunchPlan, LaunchReadiness, LaunchTarget,
        };
        let content = LaunchContentRef {
            kind: Some(LaunchContentKind::ExtractedArcadeSet),
            container: None,
            resolved_path: None,
            requires_mount: false,
            provenance: String::new(),
        };
        let mut candidates = vec![LaunchCandidate {
            target: LaunchTarget::RetroArchCore {
                profile: ProfileRef {
                    profile_kind: ProfileKind::Native,
                    scope: ProfileScope::User,
                },
                core_stem: "fbneo".into(),
                platform_id: "Arcade",
            },
            content: content.clone(),
            firmware: FirmwareReadiness::NotRequired,
            blockers: vec![LaunchBlocker::new(
                LaunchBlockerKind::RetroArchArcadeSetNeedsArchive,
                "base.",
            )],
            warnings: Vec::new(),
            readiness: LaunchReadiness::Blocked,
            preference: CandidatePreference::Undetermined,
        }];
        if let Some(detail) = mame_blocker {
            candidates.push(LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: "mame",
                    profile_id: "mame".into(),
                    profile_path: None,
                },
                content,
                firmware: FirmwareReadiness::NotRequired,
                blockers: vec![LaunchBlocker::new(
                    LaunchBlockerKind::MameSetVerdictUnavailable,
                    detail,
                )],
                warnings: Vec::new(),
                readiness: LaunchReadiness::Blocked,
                preference: CandidatePreference::Undetermined,
            });
        }
        LaunchPlan {
            platform_id: Some("Arcade".into()),
            game_key: Some("19xx".into()),
            summary: Default::default(),
            candidates,
            media_topology: None,
        }
    }

    #[test]
    fn only_a_catalogued_arcade_set_directory_is_ever_probed() {
        let exe = Path::new("/usr/bin/mame");
        let arcade = Path::new("/lib/arcade/pacman");
        let is_arcade = |p: &Path| p == arcade;
        assert_eq!(
            arcade_probe_target(Some(arcade), &is_arcade, Some(exe)),
            Some((arcade, exe))
        );
        // ScummVM / DOS / PS3 / any other folder: no probe at all.
        for other in ["/lib/scummvm/monkey", "/lib/dos/doom", "/lib/ps3/game"] {
            assert_eq!(
                arcade_probe_target(Some(Path::new(other)), &is_arcade, Some(exe)),
                None
            );
        }
        assert_eq!(arcade_probe_target(None, &is_arcade, Some(exe)), None);
        // No MAME configured: nothing to ask.
        assert_eq!(arcade_probe_target(Some(arcade), &is_arcade, None), None);
    }

    #[test]
    fn arcade_blocker_names_exactly_what_is_missing_for_mame() {
        let detail = |plan: &archivefs_core::launch::LaunchPlan| {
            plan.candidates[0].blockers[0].detail.clone()
        };
        let mut plan = arcade_plan(None);
        refine_arcade_set_blocker(&mut plan, false);
        assert!(detail(&plan).contains("Choose the MAME program in Emulator Setup"));
        let mut plan = arcade_plan(None);
        refine_arcade_set_blocker(&mut plan, true);
        assert!(detail(&plan).contains("no current verified MAME record"));
        assert!(!detail(&plan).contains("Emulator Setup"));
        let mut plan = arcade_plan(Some("needs a fresh audit"));
        refine_arcade_set_blocker(&mut plan, true);
        assert!(detail(&plan).contains("cannot start this set yet: needs a fresh audit"));
    }

    #[test]
    fn joined_set_directory_keeps_the_extracted_arcade_set_shape() {
        let dir = std::env::temp_dir().join(format!("emuwiz-arcade-shape-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("19xx")).unwrap();
        std::fs::write(dir.join("19xx.zip"), b"zip").unwrap();

        let mut directory = unresolved();
        apply_mame_set_content(&mut directory, Some(&dir.join("19xx")));
        assert_eq!(directory.kind, Some(LaunchContentKind::ExtractedArcadeSet));
        assert_eq!(directory.resolved_path, Some(dir.join("19xx")));

        let mut archive = unresolved();
        apply_mame_set_content(&mut archive, Some(&dir.join("19xx.zip")));
        assert_eq!(archive.kind, None);

        let directory_kind = archivefs_core::ArchiveKind::ArcadeSetDirectory;
        assert!(crate::emulator_setup::catalogued_content_shape_ok(
            directory_kind,
            &dir.join("19xx")
        ));
        assert!(!crate::emulator_setup::catalogued_content_shape_ok(
            archivefs_core::ArchiveKind::DirectGameImage,
            &dir.join("19xx")
        ));
        assert!(crate::emulator_setup::catalogued_content_shape_ok(
            archivefs_core::ArchiveKind::DirectGameImage,
            &dir.join("19xx.zip")
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
