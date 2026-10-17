//! Bounded, read-only input for the existing profile planner. User-supplied
//! recommendation evidence is advisory, never authenticated compatibility data.
use super::*;
use crate::game_profile_planner::{
    GameProfileEvidence, GameProfileSetting, plan_game_profile_recommendations,
};
use crate::patch_manager::{
    DolphinLocalDiscoveryRoots, DolphinLocalProfile, parse_dolphin_ini,
    resolve_dolphin_native_launch_binding,
};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 256 * 1024;
const MAX_SETTINGS: usize = 256;

#[derive(Debug)]
pub struct DolphinGuidanceInspection {
    pub plan: DolphinProfileLaunchPlan,
    pub document_path: PathBuf,
    pub document_sha256: String,
    pub local_path: PathBuf,
    pub local_sha256: Option<String>,
    pub notes: Vec<String>,
}

fn read_text(path: &Path) -> Result<String, String> {
    let source = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    if source.len() > MAX_BYTES {
        return Err("Guidance input exceeds 256 KiB".into());
    }
    let mut bytes = Vec::new();
    source
        .into_file()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Guidance input grew beyond 256 KiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "Guidance input is not UTF-8".into())
}

/// No discovery, process execution, configuration changes or launch authority.
/// Call on a worker thread. Identity and profile come from the existing planner.
pub fn inspect_dolphin_profile_guidance(
    identity: &CanonicalIdentityStatus,
    candidate: &LaunchCandidate,
    profile: &DolphinLocalProfile,
    roots: &DolphinLocalDiscoveryRoots,
    document_path: &Path,
) -> Result<DolphinGuidanceInspection, String> {
    let CanonicalIdentityStatus::Resolved(identity_facts) = identity else {
        return Err("A verified game identity is required".into());
    };
    let LaunchTarget::Standalone {
        adapter_id: "dolphin",
        profile_id,
        ..
    } = &candidate.target
    else {
        return Err("Select an existing Dolphin launch candidate".into());
    };
    if profile_id != &profile.profile_id
        || !matches!(identity_facts.platform_id.as_str(), "GameCube" | "Wii")
    {
        return Err("Dolphin platform/profile pairing changed; check readiness again".into());
    }
    let game_id = &identity_facts.game_key;
    if game_id.len() != 6 || !game_id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("An exact six-character verified Dolphin game ID is required".into());
    }
    let binding = resolve_dolphin_native_launch_binding(profile, roots);
    let version = match &binding {
        Ok(binding) => profile
            .executable_candidates
            .iter()
            .find(|e| e.path == binding.executable)
            .and_then(|e| e.version.clone()),
        Err(_) => None,
    };
    let text = read_text(document_path)?;
    let settings: Vec<GameProfileSetting> = serde_json::from_str(&text).map_err(|e| {
        format!("Expected a JSON array of existing GameProfileSetting records: {e}")
    })?;
    if settings.len() > MAX_SETTINGS {
        return Err("At most 256 recommendation records are supported".into());
    }
    let context = dolphin_game_profile_candidate(
        &identity_facts.platform_id,
        game_id,
        profile_id,
        version.clone(),
    );
    let plan = plan_game_profile_recommendations(context, settings);
    let local_path = profile.game_settings_path.join(format!("{game_id}.ini"));
    let mut local = Vec::new();
    let mut local_sha256 = None;
    let mut notes = vec![
        "User-supplied profile provenance is not authenticated. Recommendations are advisory, not a compatibility verdict.".into(),
        "Only the exact Game ID INI is compared. Global, inherited, revision-specific and running-emulator settings are not resolved.".into(),
        "Controller/accessory requirements are unverified; controller mappings and Wii system state are never imported.".into(),
        "Read-only snapshot. Recheck after changing profiles, documents, settings or emulator installations.".into(),
    ];
    match std::fs::symlink_metadata(&local_path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            notes.push("No exact Game ID local override file was present at inspection.".into())
        }
        Err(e) => notes.push(format!(
            "Local settings unavailable, not confirmed absent: {e}"
        )),
        Ok(_) => match read_text(&local_path) {
            Err(e) => notes.push(format!("Local settings unavailable: {e}")),
            Ok(text) => {
                local_sha256 = Some(
                    Sha256::digest(text.as_bytes())
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>(),
                );
                let ini = parse_dolphin_ini(&text);
                let sections = ini.section_names();
                let mut seen_sections = std::collections::BTreeSet::new();
                let duplicate_section = sections
                    .iter()
                    .any(|name| !seen_sections.insert(name.to_ascii_lowercase()));
                if !ini.warnings.is_empty() || duplicate_section {
                    notes.push("Local INI has warnings or duplicate sections; comparison is ambiguous and unavailable.".into());
                } else {
                    let mut seen_keys = std::collections::BTreeSet::new();
                    let mut duplicate_key = false;
                    for section in sections {
                        for line in ini.named_section_lines(&section) {
                            let line = line.trim();
                            if line.starts_with(['#', ';', '$', '*']) {
                                continue;
                            }
                            if let Some((key, value)) = line.split_once('=') {
                                duplicate_key |= !seen_keys.insert(
                                    format!("{}.{}", section, key.trim()).to_ascii_lowercase(),
                                );
                                local.push(GameProfileSetting {
                                    key: format!("{}.{}", section, key.trim()),
                                    proposed_value: value.trim().into(),
                                    evidence: GameProfileEvidence {
                                        provider: local_path.display().to_string(), source_version: None, source_date: None,
                                        platform: Some(identity_facts.platform_id.clone()), game_identity: Some(game_id.clone()),
                                        game_identity_confidence: GameProfileIdentityConfidence::Exact,
                                        emulator: "Dolphin".into(), emulator_version: version.clone(), profile: Some(profile_id.clone()),
                                        reason: "Observed exact-ID local override; not an effective-settings or compatibility claim".into(),
                                    },
                                });
                            }
                        }
                    }
                    if duplicate_key {
                        local.clear();
                        notes.push("Local INI has duplicate setting keys; comparison is ambiguous and unavailable.".into());
                    }
                }
            }
        },
    }
    let mut guidance = plan_dolphin_launch_with_game_profile(
        identity,
        candidate,
        &binding,
        plan,
        version.as_deref(),
        &local,
    );
    if notes.iter().any(|note| {
        note.starts_with("Local settings unavailable") || note.starts_with("Local INI has")
    }) {
        guidance
            .warnings
            .push("Local comparison is incomplete; review the unavailable evidence.".into());
        if guidance.status != DolphinProfileLaunchStatus::Blocked {
            guidance.status = DolphinProfileLaunchStatus::NeedsReview;
        }
    }
    Ok(DolphinGuidanceInspection {
        plan: guidance,
        document_path: document_path.into(),
        document_sha256: Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        local_path,
        local_sha256,
        notes,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::launch::planning::{
        CandidatePreference, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
        ResolvedIdentity,
    };
    use crate::launch::readiness::FirmwareReadiness;
    use crate::patch_manager::discover_dolphin_local_profiles;
    use std::collections::BTreeMap;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture {
        _temp: tempfile::TempDir,
        roots: DolphinLocalDiscoveryRoots,
        profile: DolphinLocalProfile,
        identity: CanonicalIdentityStatus,
        candidate: LaunchCandidate,
        doc: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let user = root.join("user");
            std::fs::create_dir_all(user.join("GameSettings")).unwrap();
            std::fs::create_dir_all(user.join("Config")).unwrap();
            std::fs::write(user.join("Config/Dolphin.ini"), "[Core]\n").unwrap();
            std::fs::write(user.join("Dolphin.ini"), "[Core]\n").unwrap();
            let exe = root.join("dolphin-emu");
            std::fs::write(&exe, "fixture, never executed").unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
            let roots = DolphinLocalDiscoveryRoots {
                home: root.join("home"),
                xdg_config_home: root.join("config"),
                xdg_data_home: root.join("data"),
                explicit_configuration_roots: vec![user],
                portable_configuration_roots: vec![],
                explicit_executables: vec![exe.clone()],
                known_version_outputs: BTreeMap::from([(exe, "Dolphin 5.0".into())]),
                appimage_directory: None,
                dolphin_emu_userpath_override: None,
                path_override: Some(vec![]),
            };
            let profile = discover_dolphin_local_profiles(&roots)
                .profiles
                .into_iter()
                .find(|p| p.eligible)
                .unwrap();
            let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: "GameCube".into(),
                game_key: "GALE01".into(),
            });
            let candidate = LaunchCandidate {
                target: LaunchTarget::Standalone {
                    adapter_id: "dolphin",
                    profile_id: profile.profile_id.clone(),
                    profile_path: Some(profile.configuration_root.clone()),
                },
                content: LaunchContentRef {
                    kind: Some(LaunchContentKind::OpticalDisc),
                    container: Some(LaunchContainerKind::PlainFile),
                    resolved_path: Some(root.join("game.iso")),
                    requires_mount: false,
                    provenance: "fixture".into(),
                },
                firmware: FirmwareReadiness::NotRequired,
                blockers: vec![],
                warnings: vec![],
                readiness: LaunchReadiness::Ready,
                preference: CandidatePreference::SoleEligible,
            };
            std::fs::write(root.join("game.iso"), b"unchanged fixture source").unwrap();
            let doc = root.join("profile.json");
            let records = vec![GameProfileSetting {
                key: "Core.MMU".into(),
                proposed_value: "True".into(),
                evidence: GameProfileEvidence {
                    provider: "synthetic profile".into(),
                    source_version: Some("1".into()),
                    source_date: None,
                    platform: Some("GameCube".into()),
                    game_identity: Some("GALE01".into()),
                    game_identity_confidence: GameProfileIdentityConfidence::Exact,
                    emulator: "Dolphin".into(),
                    emulator_version: Some("5.0".into()),
                    profile: Some(profile.profile_id.clone()),
                    reason: "fixture only".into(),
                },
            }];
            std::fs::write(&doc, serde_json::to_vec(&records).unwrap()).unwrap();
            Self {
                _temp: temp,
                roots,
                profile,
                identity,
                candidate,
                doc,
            }
        }
        fn inspect(&self) -> Result<DolphinGuidanceInspection, String> {
            inspect_dolphin_profile_guidance(
                &self.identity,
                &self.candidate,
                &self.profile,
                &self.roots,
                &self.doc,
            )
        }
        fn local(&self) -> PathBuf {
            self.profile.game_settings_path.join("GALE01.ini")
        }
    }
    #[test]
    fn dolphin_profile_inspection_compares_local_settings_without_any_writes_or_argv_changes() {
        let f = Fixture::new();
        std::fs::write(f.local(), "[Core]\nMMU = False\n").unwrap();
        let before_doc = std::fs::read(&f.doc).unwrap();
        let before_local = std::fs::read(f.local()).unwrap();
        let before_game =
            std::fs::read(f.candidate.content.resolved_path.as_ref().unwrap()).unwrap();
        let report = f.inspect().unwrap();
        assert_eq!(report.plan.local_conflicts.len(), 1);
        assert_eq!(report.plan.portable_recommendations.len(), 1);
        assert!(report.plan.read_only);
        let normal = build_dolphin_command_plan(
            &f.identity,
            &f.candidate,
            &resolve_dolphin_native_launch_binding(&f.profile, &f.roots),
        );
        assert_eq!(report.plan.command_plan, normal);
        assert_eq!(std::fs::read(&f.doc).unwrap(), before_doc);
        assert_eq!(std::fs::read(f.local()).unwrap(), before_local);
        assert_eq!(
            std::fs::read(f.candidate.content.resolved_path.as_ref().unwrap()).unwrap(),
            before_game
        );
    }
    #[test]
    fn dolphin_profile_inspection_missing_malformed_oversized_and_symlink_documents_refuse() {
        let f = Fixture::new();
        std::fs::remove_file(&f.doc).unwrap();
        assert!(f.inspect().is_err());
        std::fs::write(&f.doc, b"{invalid").unwrap();
        assert!(f.inspect().is_err());
        std::fs::write(&f.doc, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(f.inspect().is_err());
        std::fs::remove_file(&f.doc).unwrap();
        symlink(f.candidate.content.resolved_path.as_ref().unwrap(), &f.doc).unwrap();
        assert!(f.inspect().is_err());
    }
    #[test]
    fn dolphin_profile_inspection_local_missing_unreadable_and_symlink_stay_distinct() {
        let f = Fixture::new();
        let report = f.inspect().unwrap();
        assert!(report.notes.iter().any(|n| n.contains("No exact Game ID")));
        symlink(&f.doc, f.local()).unwrap();
        let report = f.inspect().unwrap();
        assert!(report.local_sha256.is_none());
        assert!(report.notes.iter().any(|n| n.contains("unavailable")));
        std::fs::remove_file(f.local()).unwrap();
        std::fs::create_dir(f.local()).unwrap();
        let report = f.inspect().unwrap();
        assert!(report.notes.iter().any(|n| n.contains("unavailable")));
        assert!(!report.notes.iter().any(|n| n.contains("No exact Game ID")));
    }
    #[test]
    fn dolphin_profile_inspection_unknown_version_and_missing_executable_never_claim_applicability()
    {
        let mut f = Fixture::new();
        for e in &mut f.profile.executable_candidates {
            e.version = None;
        }
        let report = f.inspect().unwrap();
        assert!(report.plan.portable_recommendations.is_empty());
        assert_eq!(report.plan.status, DolphinProfileLaunchStatus::NeedsReview);
        std::fs::remove_file(&f.profile.executable_candidates[0].path).unwrap();
        let report = f.inspect().unwrap();
        assert_eq!(report.plan.status, DolphinProfileLaunchStatus::Blocked);
        assert!(report.plan.command_plan.command.is_none());
    }
    #[test]
    fn dolphin_profile_inspection_recheck_does_not_reuse_changed_document_or_local_evidence() {
        let f = Fixture::new();
        let first = f.inspect().unwrap();
        std::fs::write(&f.doc, b"[]").unwrap();
        let second = f.inspect().unwrap();
        assert_ne!(first.document_sha256, second.document_sha256);
        assert!(second.plan.portable_recommendations.is_empty());
        std::fs::write(f.local(), "[Core]\nMMU = False\n").unwrap();
        let third = f.inspect().unwrap();
        assert!(third.local_sha256.is_some());
        assert_ne!(second.local_sha256, third.local_sha256);
    }
    #[test]
    fn dolphin_profile_inspection_refuses_wrong_profile_platform_and_path_like_identity() {
        let mut f = Fixture::new();
        f.profile.profile_id = "other".into();
        assert!(f.inspect().is_err());
        let mut f = Fixture::new();
        if let CanonicalIdentityStatus::Resolved(i) = &mut f.identity {
            i.platform_id = "Wii".into();
        }
        let report = f.inspect().unwrap();
        assert!(report.plan.portable_recommendations.is_empty());
        if let CanonicalIdentityStatus::Resolved(i) = &mut f.identity {
            i.game_key = "../bad".into();
        }
        assert!(f.inspect().is_err());
    }
    #[test]
    fn dolphin_profile_inspection_duplicate_sections_or_keys_require_review() {
        for text in [
            "[Core]\nMMU=True\n[Core]\nMMU=False\n",
            "[Core]\nMMU=True\n[core]\nMMU=False\n",
            "[Core]\nMMU=True\nMMU=False\n",
        ] {
            let f = Fixture::new();
            std::fs::write(f.local(), text).unwrap();
            let report = f.inspect().unwrap();
            assert_eq!(report.plan.status, DolphinProfileLaunchStatus::NeedsReview);
            assert!(report.notes.iter().any(|n| n.contains("ambiguous")));
            assert!(
                report.plan.local_conflicts.is_empty(),
                "must not guess an effective local value"
            );
            assert_eq!(std::fs::read_to_string(f.local()).unwrap(), text);
        }
    }
}
