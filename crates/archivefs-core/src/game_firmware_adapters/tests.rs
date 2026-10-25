use super::*;
use crate::dat::model::DatEcosystem;
use crate::launch::{CandidatePreference, LaunchContentKind, LaunchContentRef, LaunchReadiness};
fn ctx(platform: &str) -> AdapterFirmwareContext {
    AdapterFirmwareContext {
        game_id: Some("synthetic-game".into()),
        platform: platform.into(),
        release_identity: Some("verified-local-identity".into()),
        freshness: FirmwareFreshnessSnapshot {
            profile_fingerprint: Some("profile".into()),
            firmware_path_fingerprint: Some("path".into()),
            firmware_file_fingerprint: Some("file".into()),
            game_identity_fingerprint: Some("game".into()),
            requirement_fingerprint: Some("policy".into()),
        },
        ..Default::default()
    }
}
fn candidate(adapter: &'static str, path: PathBuf, fw: FirmwareReadiness) -> LaunchCandidate {
    LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: adapter,
            profile_id: "selected".into(),
            profile_path: Some(path),
        },
        content: LaunchContentRef {
            kind: Some(LaunchContentKind::OpticalDisc),
            container: None,
            resolved_path: None,
            requires_mount: false,
            provenance: "synthetic".into(),
        },
        firmware: fw,
        blockers: vec![],
        warnings: vec![],
        readiness: LaunchReadiness::Blocked,
        preference: CandidatePreference::Remembered,
    }
}
fn p2() -> Pcsx2Profile {
    Pcsx2Profile {
        profile_id: "selected".into(),
        installation_type: Pcsx2InstallationType::Native,
        scope: Pcsx2ProfileScope::User,
        configuration_path: "/synthetic/p2".into(),
        provenance: "synthetic",
        eligible: true,
        blockers: vec![],
        patch_directories: vec![],
        configuration_identity: None,
        executable_candidates: vec![],
    }
}
fn ps1() -> DuckStationProfile {
    let p = PathBuf::from("/synthetic/ps1");
    DuckStationProfile {
        profile_id: "selected".into(),
        installation_type: DuckStationInstallationType::Native,
        configuration_path: p.clone(),
        provenance: "synthetic",
        eligible: true,
        blocker: None,
        executable_candidates: vec![],
        global_config_path: p.join("settings.ini"),
        game_settings_path: p.join("gamesettings"),
        cheats_path: p.join("cheats"),
        patches_path: p.join("patches"),
        textures_path: p.join("textures"),
        bios_path: p.join("bios"),
        memory_cards_path: p.join("memcards"),
        save_states_path: p.join("states"),
    }
}
fn dc() -> FlycastProfile {
    let p = PathBuf::from("/synthetic/dc");
    FlycastProfile {
        profile_id: "selected".into(),
        installation_type: FlycastInstallationType::Native,
        configuration_path: p.clone(),
        data_path: p.clone(),
        eligible: true,
        blocker: None,
        executable_candidates: vec![],
        config_path: p.join("emu.cfg"),
        system_path: p.join("data"),
        game_settings_path: p.join("games"),
        cheats_path: p.join("cheats"),
        textures_path: p.join("textures"),
        vmu_path: p.join("vmu"),
        save_states_path: p.join("states"),
    }
}
fn record(system: FirmwareSystem) -> FirmwareIdentityRecord {
    FirmwareIdentityRecord {
        system,
        provider: DatEcosystem::Redump,
        name: "Synthetic BIOS identity".into(),
        description: None,
        size_bytes: 7,
        crc32: "00000000".into(),
        md5: "0".repeat(32),
        sha1: "0".repeat(40),
        dat_version: Some("synthetic-v1".into()),
    }
}
fn verified_p2() -> Pcsx2BiosVerificationOutcome {
    let r = record(FirmwareSystem::PlayStation2);
    Pcsx2BiosVerificationOutcome::Verified(Pcsx2VerifiedBios {
        path: "/synthetic/p2/bios/owned.bin".into(),
        size_bytes: r.size_bytes,
        crc32: r.crc32.clone(),
        md5: r.md5.clone(),
        sha1: r.sha1.clone(),
        record: r,
    })
}
#[test]
fn exact_pcsx2_hash_retains_provenance_and_does_not_mutate_launch() {
    let p = p2();
    let c = candidate(
        "pcsx2",
        p.configuration_path.clone(),
        FirmwareReadiness::Verified,
    );
    let before = c.clone();
    let r = pcsx2(&ctx("PS2"), &c, &p, &verified_p2());
    assert_eq!(r.state, GameFirmwareState::Ready);
    assert_eq!(
        r.evidence[0].strength,
        FirmwareEvidenceStrength::ExactKnownHash
    );
    assert!(r.evidence[0].sha256.is_none());
    assert!(r.evidence[0].provenance.contains("synthetic-v1"));
    assert_eq!(before, c);
}
#[test]
fn cached_verified_never_overrides_missing_unsafe_or_ambiguous_pcsx2() {
    let p = p2();
    let c = candidate(
        "pcsx2",
        p.configuration_path.clone(),
        FirmwareReadiness::Verified,
    );
    for v in [
        Pcsx2BiosVerificationOutcome::Missing,
        Pcsx2BiosVerificationOutcome::Unsafe {
            path: "/link".into(),
            detail: "symlink".into(),
        },
        Pcsx2BiosVerificationOutcome::Ambiguous {
            detail: "two matches".into(),
        },
        Pcsx2BiosVerificationOutcome::Unreadable {
            detail: "read failed".into(),
        },
    ] {
        let r = pcsx2(&ctx("PS2"), &c, &p, &v);
        assert_eq!(r.state, GameFirmwareState::Unknown);
    }
}
#[test]
fn pcsx2_present_unknown_hash_is_not_mismatch() {
    let p = p2();
    let c = candidate(
        "pcsx2",
        p.configuration_path.clone(),
        FirmwareReadiness::PresentUnverified,
    );
    let r = pcsx2(
        &ctx("PS2"),
        &c,
        &p,
        &Pcsx2BiosVerificationOutcome::Unknown {
            path: "whatever.bin".into(),
        },
    );
    assert_eq!(r.state, GameFirmwareState::PresentUnverified);
    assert_ne!(r.state, GameFirmwareState::HashMismatch);
}
#[test]
fn profile_adapter_path_and_platform_must_all_match() {
    let p = p2();
    let good = candidate(
        "pcsx2",
        p.configuration_path.clone(),
        FirmwareReadiness::Verified,
    );
    let mut wrong = good.clone();
    wrong.target = LaunchTarget::Standalone {
        adapter_id: "duckstation",
        profile_id: p.profile_id.clone(),
        profile_path: Some(p.configuration_path.clone()),
    };
    let mut id = good.clone();
    if let LaunchTarget::Standalone { profile_id, .. } = &mut id.target {
        *profile_id = "other".into()
    };
    let mut path = good.clone();
    if let LaunchTarget::Standalone { profile_path, .. } = &mut path.target {
        *profile_path = Some("/different".into())
    };
    for c in [wrong, id, path] {
        let r = pcsx2(&ctx("PS2"), &c, &p, &verified_p2());
        assert_eq!(r.state, GameFirmwareState::Unknown);
        assert!(
            !r.evidence
                .iter()
                .any(|e| e.status == FirmwareEvidenceStatus::Accepted)
        );
    }
    assert_eq!(
        pcsx2(&ctx("PSX"), &good, &p, &verified_p2()).state,
        GameFirmwareState::Unknown
    );
}
#[test]
fn missing_freshness_prevents_verified_ready() {
    let p = p2();
    let c = candidate(
        "pcsx2",
        p.configuration_path.clone(),
        FirmwareReadiness::Verified,
    );
    let mut context = ctx("PS2");
    context.freshness.game_identity_fingerprint = None;
    assert_eq!(
        pcsx2(&context, &c, &p, &verified_p2()).state,
        GameFirmwareState::Unknown
    );
}
#[test]
fn duckstation_conflict_never_disappears_into_ready() {
    let p = ps1();
    let c = candidate(
        "duckstation",
        p.configuration_path.clone(),
        FirmwareReadiness::Verified,
    );
    let r = duckstation(
        &ctx("PSX"),
        &c,
        &p,
        &DuckStationBiosVerificationOutcome::Conflict {
            detail: "global A vs game B".into(),
        },
    );
    assert_eq!(r.state, GameFirmwareState::Unknown);
    assert!(
        r.evidence
            .iter()
            .any(|e| e.provenance.contains("global A vs game B"))
    );
}
#[test]
fn duckstation_hash_match_does_not_invent_region_or_emulator_verification() {
    let p = ps1();
    let c = candidate(
        "duckstation",
        p.configuration_path.clone(),
        FirmwareReadiness::Verified,
    );
    let rec = record(FirmwareSystem::PlayStation);
    let v = DuckStationBiosVerificationOutcome::Verified(DuckStationVerifiedBios {
        path: p.bios_path.join("arbitrary.bin"),
        size_bytes: rec.size_bytes,
        crc32: rec.crc32.clone(),
        md5: rec.md5.clone(),
        sha1: rec.sha1.clone(),
        record: rec,
        required_region: Some("PAL"),
    });
    let r = duckstation(&ctx("PSX"), &c, &p, &v);
    assert_eq!(r.state, GameFirmwareState::Ready);
    assert!(!r.requirement.region_requirement_proven);
    assert!(r.evidence[0].region.is_none());
    assert_eq!(
        r.evidence[0].strength,
        FirmwareEvidenceStrength::ExactKnownHash
    );
}
#[test]
fn flycast_requires_both_verified_files_and_dreamcast_binding() {
    let p = dc();
    let c = candidate(
        "flycast",
        p.configuration_path.clone(),
        FirmwareReadiness::Verified,
    );
    for flash in [
        FlycastSystemFileState::Missing,
        FlycastSystemFileState::Unknown,
        FlycastSystemFileState::PresentUnverified,
        FlycastSystemFileState::Verified,
    ] {
        let h = FlycastSystemHealth {
            dreamcast_bios: FlycastSystemFileState::Verified,
            dreamcast_flash: flash,
            arcade_system_roms: FlycastSystemFileState::Verified,
        };
        let r = flycast(&ctx("Dreamcast"), &c, &p, &h);
        assert_eq!(
            r.state == GameFirmwareState::Ready,
            flash == FlycastSystemFileState::Verified
        );
        assert_eq!(
            flycast(&ctx("Arcade"), &c, &p, &h).state,
            GameFirmwareState::Unknown
        );
    }
}
#[test]
fn pce_unknown_policy_or_title_never_inherits_verified() {
    let c = candidate(
        "mesen",
        "/synthetic/mesen".into(),
        FirmwareReadiness::Verified,
    );
    for policy in [
        PceCdEmulatorFirmwarePolicy::Unknown,
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
    ] {
        let r = pc_engine_cd(
            &ctx("PC Engine CD"),
            &c,
            &c.target,
            policy,
            PceCdFirmwareReadiness::VerifiedButRequirementUnknown {
                class: SystemCardClass::CdRom2V2,
            },
        );
        assert_eq!(r.state, GameFirmwareState::Unknown);
    }
}
#[test]
fn pce_cd_firmware_binds_to_cd_platform_and_never_to_hucards() {
    let c = candidate(
        "mesen",
        "/synthetic/profile".into(),
        FirmwareReadiness::Verified,
    );
    for platform in ["PC Engine CD", "TurboGrafx-CD"] {
        let r = pc_engine_cd(
            &ctx(platform),
            &c,
            &c.target,
            PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
            PceCdFirmwareReadiness::VerifiedSufficient {
                class: SystemCardClass::SuperSystemCardV3,
            },
        );
        assert_eq!(r.state, GameFirmwareState::Ready, "{platform}");
    }
    for platform in ["PC Engine", "TurboGrafx-16", "PC-FX"] {
        let r = pc_engine_cd(
            &ctx(platform),
            &c,
            &c.target,
            PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
            PceCdFirmwareReadiness::VerifiedSufficient {
                class: SystemCardClass::SuperSystemCardV3,
            },
        );
        assert_eq!(r.state, GameFirmwareState::Unknown, "{platform}");
    }
}
#[test]
fn pce_known_sufficient_and_embedded_policies_are_distinct() {
    let c = candidate(
        "mesen",
        "/synthetic/mesen".into(),
        FirmwareReadiness::Verified,
    );
    let r = pc_engine_cd(
        &ctx("PC Engine CD"),
        &c,
        &c.target,
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard,
        PceCdFirmwareReadiness::VerifiedSufficient {
            class: SystemCardClass::SuperSystemCardV3,
        },
    );
    assert_eq!(r.state, GameFirmwareState::Ready);
    let mut embedded = c.clone();
    embedded.firmware = FirmwareReadiness::NotRequired;
    assert_eq!(
        pc_engine_cd(
            &ctx("PC Engine CD"),
            &embedded,
            &embedded.target,
            PceCdEmulatorFirmwarePolicy::EmbedsSystemCard,
            PceCdFirmwareReadiness::EmulatorProvidesFirmware
        )
        .state,
        GameFirmwareState::NotRequired
    );
    let mut cart = embedded.clone();
    cart.content.kind = Some(LaunchContentKind::Cartridge);
    assert_eq!(
        pc_engine_cd(
            &ctx("PC Engine CD"),
            &cart,
            &cart.target,
            PceCdEmulatorFirmwarePolicy::EmbedsSystemCard,
            PceCdFirmwareReadiness::EmulatorProvidesFirmware
        )
        .state,
        GameFirmwareState::Unknown
    );
}
#[test]
fn hatari_observed_version_and_region_are_not_game_requirements() {
    let p = HatariProfile {
        profile_id: "selected".into(),
        installation_type: HatariInstallationType::Explicit,
        config_path: "/synthetic/hatari.cfg".into(),
        provenance: "synthetic",
        eligible: true,
        executable_candidates: vec![],
    };
    let c = candidate("hatari", p.config_path.clone(), FirmwareReadiness::Verified);
    let h = HatariHealth {
        detected: true,
        config_readable: true,
        tos: HatariTosRom {
            path: Some("/synthetic/tos.img".into()),
            state: HatariPathState::Present,
            health: HatariTosHealth::Verified,
            sha256: Some("0".repeat(64)),
            version: Some("2.06".into()),
            region: Some("UK".into()),
        },
        machine: Default::default(),
        warnings: vec![],
    };
    let r = hatari(&ctx("AtariST"), &c, &p, &h);
    assert_eq!(r.state, GameFirmwareState::Ready);
    assert!(!r.requirement.region_requirement_proven);
    assert!(!r.requirement.version_requirement_proven);
    assert_eq!(r.evidence[0].version.as_deref(), Some("2.06"));
}
