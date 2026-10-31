use super::*;
use archivefs_core::dat::firmware_evidence::{
    FirmwareIdentityRecord, FirmwareSystem, hash_firmware_file,
};
use archivefs_core::dat::model::DatEcosystem;
use archivefs_core::launch::{
    CandidatePreference, FirmwareReadiness, LaunchContentRef, LaunchReadiness,
};
struct Fixture {
    _root: tempfile::TempDir,
    request: Request,
    config: PathBuf,
    bios: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let p = root.path().join("pcsx2");
        std::fs::create_dir_all(p.join("bios")).unwrap();
        let config = p.join("PCSX2.ini");
        std::fs::write(&config, b"[Filenames]\nBIOS = owned.bin\n").unwrap();
        let bios = p.join("bios/owned.bin");
        std::fs::write(
            &bios,
            b"synthetic owner-created firmware bytes, not a real BIOS",
        )
        .unwrap();
        let h = hash_firmware_file(&bios, 1024 * 1024, 8192).unwrap();
        let evidence = FirmwareIdentityRecord {
            system: FirmwareSystem::PlayStation2,
            provider: DatEcosystem::Redump,
            name: "Synthetic BIOS fixture".into(),
            description: None,
            size_bytes: h.size_bytes,
            crc32: h.crc32,
            md5: h.md5,
            sha1: h.sha1,
            dat_version: Some("fixture-v1".into()),
        };
        let profile = Pcsx2Profile {
            profile_id: "synthetic".into(),
            installation_type: Pcsx2InstallationType::Native,
            scope: Pcsx2ProfileScope::User,
            configuration_path: p.clone(),
            provenance: "synthetic",
            eligible: true,
            blockers: vec![],
            patch_directories: vec![],
            configuration_identity: None,
            executable_candidates: vec![],
        };
        let game = root.path().join("game.iso");
        std::fs::write(&game, b"synthetic game identity input").unwrap();
        let request = Request {
            context: AdapterFirmwareContext {
                game_id: Some("game-1".into()),
                platform: "PS2".into(),
                release_identity: Some("verified-synthetic".into()),
                ..Default::default()
            },
            candidate: LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: "pcsx2",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(p),
                },
                content: LaunchContentRef {
                    kind: None,
                    container: None,
                    resolved_path: Some(game.clone()),
                    requires_mount: false,
                    provenance: "synthetic".into(),
                },
                firmware: FirmwareReadiness::Verified,
                blockers: vec![],
                warnings: vec![],
                readiness: LaunchReadiness::Blocked,
                preference: CandidatePreference::Remembered,
            },
            game_fingerprint: FileFingerprint::observe(&game),
            game_path: game,
            source: Source::Pcsx2(profile, vec![evidence]),
        };
        Self {
            _root: root,
            request,
            config,
            bios,
        }
    }
}
#[test]
fn current_selected_profile_exact_hash_is_guidance_only_and_performs_no_writes() {
    let f = Fixture::new();
    let before = [&f.config, &f.bios, &f.request.game_path].map(|p| std::fs::read(p).unwrap());
    let candidate = f.request.candidate.clone();
    let r = inspection::inspect(f.request.clone()).unwrap();
    assert_eq!(r.state, GameFirmwareState::Ready);
    assert_eq!(r.profile_id.as_deref(), Some("synthetic"));
    assert_eq!(r.freshness.state, FirmwareFreshnessState::Current);
    assert_eq!(f.request.candidate, candidate);
    for (path, bytes) in [&f.config, &f.bios, &f.request.game_path]
        .iter()
        .zip(before)
    {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(
        std::fs::read_dir(f.bios.parent().unwrap()).unwrap().count(),
        1
    );
}
#[test]
fn changed_file_during_observation_refuses_current_result() {
    let f = Fixture::new();
    let path = f.bios.clone();
    inspection::after_sample_hook(move || {
        std::fs::write(path, b"changed synthetic bytes").unwrap()
    });
    let r = inspection::inspect(f.request).unwrap();
    assert_eq!(r.state, GameFirmwareState::Stale);
    assert_eq!(r.freshness.state, FirmwareFreshnessState::Stale);
}
#[test]
fn stale_or_missing_game_identity_never_makes_verified_firmware_ready() {
    let mut f = Fixture::new();
    std::fs::write(&f.request.game_path, b"changed game identity").unwrap();
    assert_eq!(
        inspection::inspect(f.request.clone()).unwrap().state,
        GameFirmwareState::Unknown
    );
    f.request.game_fingerprint = None;
    assert_eq!(
        inspection::inspect(f.request).unwrap().state,
        GameFirmwareState::Unknown
    );
}
#[test]
fn changed_selected_profile_discards_an_inflight_result() {
    let f = Fixture::new();
    let result = inspection::inspect(f.request).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(Observed {
        readiness: result,
        checked_at: Instant::now(),
    }))
    .unwrap();
    let mut s = State {
        key: "old-profile".into(),
        job: Some(Job {
            key: "old-profile".into(),
            receiver: rx,
        }),
        ..Default::default()
    };
    s.select("new-profile".into());
    s.poll();
    assert!(s.result.is_none());
    assert!(s.job.is_none());
}
#[test]
fn disconnected_worker_is_unknown_not_ready() {
    let (tx, rx) = mpsc::channel();
    drop(tx);
    let mut s = State {
        job: Some(Job {
            key: String::new(),
            receiver: rx,
        }),
        ..Default::default()
    };
    s.poll();
    assert!(s.result.is_none());
    assert!(
        s.error
            .as_deref()
            .unwrap()
            .contains("No readiness conclusion")
    );
}
#[test]
fn duplicate_profile_matches_are_never_chosen_arbitrarily() {
    assert_eq!(unique_profile(&[1, 1], |v| *v == 1), None);
    assert_eq!(unique_profile(&[1, 2], |v| *v == 1), Some(&1));
}
#[test]
fn global_inventory_does_not_authorise_a_selected_profile() {
    let pool = archivefs_core::bios_projection::BiosMasterInventory {
        root: "/synthetic/global".into(),
        entries: vec![],
        warnings: vec![],
    };
    assert!(inventory_note(Some(&pool)).contains("not proof"));
    assert!(required_label("AtariST").contains("unknown"));
    assert!(required_label("PC Engine CD").contains("global inventory"));
}
#[test]
fn pc_engine_cd_guidance_uses_the_cd_platform_not_hucards() {
    assert!(supports_platform("PC Engine CD"));
    assert!(!supports_platform("PC Engine"));
    assert!(required_label("PC Engine CD").contains("System Card"));
    assert!(required_label("PC Engine CD").contains("unknown"));
}
fn frame(
    s: &mut State,
    ctx: &egui::Context,
    r: &Request,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024.0, 600.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui, Some(r.clone()), "", None));
        },
    )
}
fn text_rect(out: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
    fn find(s: &egui::Shape, l: &str) -> Option<egui::Rect> {
        match s {
            egui::Shape::Text(t) if t.galley.text() == l => {
                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
            }
            egui::Shape::Vec(v) => v.iter().find_map(|s| find(s, l)),
            _ => None,
        }
    }
    out.shapes.iter().find_map(|s| find(&s.shape, label))
}
fn click(s: &mut State, ctx: &egui::Context, r: &Request, label: &str) {
    let _ = frame(s, ctx, r, vec![]);
    let out = frame(s, ctx, r, vec![]);
    let pos = text_rect(&out, label)
        .unwrap_or_else(|| panic!("Missing control {label}"))
        .center();
    for pressed in [true, false] {
        frame(
            s,
            ctx,
            r,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}
#[test]
fn blue_check_button_completes_read_only_journey_at_1024x600() {
    let f = Fixture::new();
    let ctx = egui::Context::default();
    let mut s = State::default();
    click(
        &mut s,
        &ctx,
        &f.request,
        "BIOS / firmware evidence for this game",
    );
    click(&mut s, &ctx, &f.request, "Check BIOS / firmware evidence");
    for _ in 0..400 {
        frame(&mut s, &ctx, &f.request, vec![]);
        if s.result.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        s.result.as_ref().map(|r| &r.state),
        Some(&GameFirmwareState::Ready)
    );
    let out = frame(&mut s, &ctx, &f.request, vec![]);
    assert!(text_rect(&out, "Firmware assessment: Ready").is_some());
    for label in ["Launch", "Apply", "Download BIOS", "Save settings"] {
        assert!(text_rect(&out, label).is_none());
    }
}

#[test]
fn delayed_ready_report_keeps_worker_observation_time_and_is_expired() {
    let f = Fixture::new();
    let readiness = inspection::inspect(f.request).unwrap();
    let (tx, rx) = mpsc::channel();
    tx.send(Ok(Observed {
        readiness,
        checked_at: Instant::now() - Duration::from_secs(60),
    }))
    .unwrap();
    let mut s = State {
        job: Some(Job {
            key: String::new(),
            receiver: rx,
        }),
        ..Default::default()
    };
    s.poll();
    assert_eq!(s.displayed_state(), Some(GameFirmwareState::Stale));
}
fn core_request(mut r: Request, platform: &'static str, name: &str, system: PathBuf) -> Request {
    use archivefs_core::emulator_environment::retroarch::{
        CoreInfoFinding, FirmwarePresence, FirmwareRequirement, ProfileKind, ProfileRef,
        ProfileScope,
    };
    r.context.platform = platform.into();
    r.candidate.target = LaunchTarget::RetroArchCore {
        profile: ProfileRef {
            profile_kind: ProfileKind::Native,
            scope: ProfileScope::User,
        },
        core_stem: if platform == "AtariST" {
            "hatari"
        } else {
            "selected-pce-core"
        }
        .into(),
        platform_id: platform,
    };
    r.source = Source::RetroArch(
        CoreInfoFinding::Found {
            display_name: None,
            display_version: None,
            system_name: Some(platform.into()),
            supported_extensions: vec![],
            core_name: None,
            manufacturer: None,
            categories: None,
            database: None,
            firmware: vec![FirmwareRequirement {
                index: 0,
                path: Some(name.into()),
                description: None,
                optional: false,
                presence: FirmwarePresence::Unknown,
            }],
        },
        Some(system),
    );
    r
}
#[test]
fn hatari_core_file_presence_is_not_standalone_tos_verification() {
    let f = Fixture::new();
    let root = f.bios.parent().unwrap().to_path_buf();
    let r = inspection::inspect(core_request(f.request, "AtariST", "owned.bin", root)).unwrap();
    assert_eq!(r.state, GameFirmwareState::Unknown);
    assert_eq!(r.freshness.state, FirmwareFreshnessState::Unknown);
    assert!(
        r.evidence
            .iter()
            .any(|e| e.provenance.contains("regular file was observed"))
    );
    assert!(
        !r.evidence
            .iter()
            .any(|e| e.status == FirmwareEvidenceStatus::Accepted)
    );
}
#[test]
fn pce_selected_core_declarations_do_not_invent_title_sufficiency() {
    let f = Fixture::new();
    let root = f.bios.parent().unwrap().to_path_buf();
    let r = inspection::inspect(core_request(
        f.request.clone(),
        "PC Engine CD",
        "syscard3.pce",
        root.clone(),
    ))
    .unwrap();
    assert_eq!(r.state, GameFirmwareState::Unknown);
    assert!(
        r.evidence
            .iter()
            .any(|e| e.provenance.contains("No file was observed"))
    );
    assert!(!r.requirement.version_requirement_proven);
    std::fs::write(root.join("syscard3.pce"), b"not a System Card ROM").unwrap();
    let r = inspection::inspect(core_request(
        f.request,
        "PC Engine CD",
        "syscard3.pce",
        root,
    ))
    .unwrap();
    assert_eq!(r.state, GameFirmwareState::Unknown);
    assert!(
        r.evidence
            .iter()
            .any(|e| e.provenance.contains("regular file was observed"))
    );
    assert!(
        r.evidence
            .iter()
            .all(|e| e.status != FirmwareEvidenceStatus::Accepted)
    );
}
#[test]
fn unresolved_core_paths_are_not_inspected_or_used_as_evidence() {
    let f = Fixture::new();
    let root = f.bios.parent().unwrap().to_path_buf();
    let r = inspection::inspect(core_request(f.request, "AtariST", "../owned.bin", root)).unwrap();
    assert_eq!(r.state, GameFirmwareState::Unknown);
    assert!(r.evidence.iter().all(|e| e.path.is_none()));
    assert!(
        r.evidence
            .iter()
            .any(|e| e.provenance.contains("no inspection was attempted"))
    );
}
