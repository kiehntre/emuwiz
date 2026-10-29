//! Verified POK -> Fuse-libretro native-code bridge. No file is written here.
//! Deliberately limited to uncompressed v1 48K Z80 snapshots: tapes are not
//! loaded when RetroArch applies startup cheats. No generic memory handler.
use super::pinning::{Observation, observe};
use super::*;
use crate::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityKind, IdentityProvenance, IdentityStatus,
};
use crate::launch::cheat_launch_plan::{
    CheatCandidate, CheatLaunchFormat, CheatLaunchSelection, CheatVariant,
};
use crate::launch::cheat_runtime::SourceBinding;
use crate::patch_manager::{
    CheatApplicabilityInput, CheatApplySupport, CheatDocument, CheatGameAssociation,
    CheatIdentityRequirement, CheatParseEvidence, CheatPlatform, CheatRoute, CheatRouteBasis,
    CheatRouteTarget, CheatSelectedGame, CheatSourceFormat, CheatSourceReference,
    assess_cheat_applicability, parse_cht_text, validate_destination_root,
};
use crate::safe_read::TrustedRoots;
use serde::{Deserialize, Serialize};

/// Empirically exercised installed native core, not a name-based assertion.
/// An upgrade must be tested again before its digest can be added.
pub const VERIFIED_FUSE_CORE_SHA256: &str =
    "5d07fc75358a76be48ecf62e6c90a81efa285aa72aee164921be27b2552c4c15";
/// Installed native RetroArch 1.21.0 used by the real backend probe.
pub const VERIFIED_RETROARCH_SHA256: &str =
    "07ecc7728586b3297c7b9101738a03d6f40fd6b6d87911ba051318eb7fdbd4e8";
pub const LIMITATION: &str = "POKE launch supports only the tested native RetroArch/Fuse binary pair and an uncompressed v1 48K .z80 snapshot. Tapes, compressed/128K snapshots, other cores and Flatpak/AppImage are refused. Only bank 8 RAM writes with no overlapping addresses are supported.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PokeLaunchPreview {
    game: ClassicTrainerGame,
    path: PathBuf,
    trainer: PathBuf,
    observations: Vec<(PathBuf, Observation)>,
    cheats: Vec<PokeCheat>,
}
impl PokeLaunchPreview {
    pub fn titles(&self) -> Vec<String> {
        self.cheats.iter().map(|c| c.title.clone()).collect()
    }
    pub fn game_path(&self) -> &Path {
        &self.path
    }
    pub fn identity_summary(&self) -> String {
        self.cheats
            .first()
            .and_then(|c| c.target_identity.clone())
            .unwrap_or_default()
    }
    pub fn entry_summary(&self, index: usize) -> Option<String> {
        self.cheats.get(index).map(|cheat| {
            cheat
                .operations
                .iter()
                .map(|op| {
                    format!(
                        "bank {:?}, address {}, value {}, restore {:?}",
                        op.bank,
                        op.address.0,
                        op.value.value,
                        op.restore_value.map(|v| v.value)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
    }
}

/// A saved choice is bound to the exact snapshot, trainer and pin observed at
/// preview time. It is never an instruction to start or activate an emulator.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPokeProfile {
    version: u8,
    game_path: PathBuf,
    game_sha256: String,
    trainer_sha256: String,
    pin_sha256: String,
    identity: Vec<String>,
    selected: Vec<usize>,
}

fn profile_for(preview: &PokeLaunchPreview, selected: &[usize]) -> SavedPokeProfile {
    SavedPokeProfile {
        version: 1,
        game_path: preview.path.clone(),
        game_sha256: preview.observations[0].1.sha256.clone(),
        trainer_sha256: preview.observations[1].1.sha256.clone(),
        pin_sha256: preview.observations[2].1.sha256.clone(),
        identity: preview.game.identities.clone(),
        selected: selected.to_vec(),
    }
}

pub fn save_profile(
    path: &Path,
    preview: &PokeLaunchPreview,
    selected: &[usize],
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    // The same composition gate used at launch rejects unsupported banks,
    // overlapping addresses, duplicate selections and unrepresentable titles.
    compose(preview, selected)?;
    let parent = path.parent().ok_or("Profile has no parent directory.")?;
    let root = parent.parent().ok_or("Profile root is missing.")?;
    validate_destination_root(root).map_err(|e| format!("Unsafe profile root: {e}"))?;
    match std::fs::create_dir(parent) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.to_string()),
    }
    validate_destination_root(parent).map_err(|e| format!("Unsafe profile folder: {e}"))?;
    let parent_metadata = std::fs::metadata(parent).map_err(|e| e.to_string())?;
    let body =
        serde_json::to_string_pretty(&profile_for(preview, selected)).map_err(|e| e.to_string())?;
    if body.len() > 4096 {
        return Err("Saved POKE profile exceeds 4 KiB.".into());
    }
    crate::atomic_text::write_in_existing_parent(
        path,
        &body,
        (parent_metadata.dev(), parent_metadata.ino()),
    )
    .map_err(|e| e.to_string())
}

pub fn load_profile(path: &Path, preview: &PokeLaunchPreview) -> Result<Vec<usize>, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return Err("Saved POKE profile is not a bounded regular file.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("Saved POKE profile is too large.".into());
    }
    let saved: SavedPokeProfile = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Saved POKE profile is invalid: {e}"))?;
    if saved.version != 1
        || saved.game_path != preview.path
        || saved.game_sha256 != preview.observations[0].1.sha256
        || saved.trainer_sha256 != preview.observations[1].1.sha256
        || saved.pin_sha256 != preview.observations[2].1.sha256
        || saved.identity != preview.game.identities
    {
        return Err("Saved POKE profile belongs to a different game revision, trainer or verified identity. Review and save a new choice.".into());
    }
    compose(preview, &saved.selected)?;
    Ok(saved.selected)
}

/// Reuses the pin parser and retained, bounded source reads from pin creation.
/// `game` must come from a freshly loaded, source-aware DAT projection.
pub fn preview(
    game: &ClassicTrainerGame,
    path: &Path,
    roots: &TrustedRoots,
) -> Result<PokeLaunchPreview, String> {
    if !game.identity_verified
        || poke_platform_for_label(&game.platform) != Some(PokePlatform::ZxSpectrum)
    {
        return Err("Current DAT-verified ZX Spectrum identity is required.".into());
    }
    let parent = path.parent().ok_or("Game has no parent directory.")?;
    validate_destination_root(parent).map_err(|e| format!("Unsafe trainer source folder: {e}"))?;
    if !roots.contains_canonical(parent) {
        return Err("Game folder is no longer inside configured trusted source roots.".into());
    }
    let (bytes, game_file) = observe(path, roots, pinning::GAME_MAX)?;
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("z80"))
        || bytes.len() != 30 + 49152
        || bytes[6..8] == [0, 0]
        || bytes[12] & 32 != 0
    {
        return Err(LIMITATION.into());
    }
    let sidecars: Vec<_> = ["pok", "POK"]
        .iter()
        .map(|e| path.with_extension(e))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect();
    let [trainer] = sidecars.as_slice() else {
        return Err("Exactly one same-name .pok/.POK trainer is required; missing or ambiguous sidecars are refused.".into());
    };
    let (source, trainer_file) = observe(trainer, roots, TRAINER_FILE_MAX_BYTES)?;
    let pin_path = trainer_target_path(path);
    let (pin, pin_file) = observe(&pin_path, roots, TRAINER_TARGET_MAX_BYTES)?;
    let (expected_trainer, identity) =
        parse_pin(std::str::from_utf8(&pin).map_err(|_| "Trainer pin is not UTF-8.")?)?;
    if expected_trainer != trainer_file.sha256
        || match_target(game, &identity, true) != ClassicTrainerMatch::VerifiedIdentity
    {
        return Err("Trainer pin or DAT-verified game identity no longer matches. Refresh and select again.".into());
    }
    let (kind, hash) = identity.split_once(':').ok_or("Invalid pinned identity.")?;
    let actual: String = match kind {
        "sha256" => game_file.sha256.clone(),
        "sha1" => sha1::Sha1::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        "md5" => md5::Md5::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        _ => return Err("Pinned identity is not an exact supported hash.".into()),
    };
    if actual != hash {
        return Err(
            "Game bytes changed since DAT verification. Rescan before launching trainers.".into(),
        );
    }
    let parsed = parse_zx_pok(std::str::from_utf8(&source).map_err(|_| "Trainer is not UTF-8.")?)
        .map_err(|e| format!("Trainer cannot be parsed: {e:?}"))?;
    let cheats = parsed
        .iter()
        .map(|p| {
            let mut cheat = normalize_zx_pok_family(p);
            cheat.target_verified = true;
            cheat.target_identity = Some(identity.clone());
            cheat
        })
        .collect();
    Ok(PokeLaunchPreview {
        game: game.clone(),
        path: path.into(),
        trainer: trainer.clone(),
        observations: vec![
            (path.into(), game_file),
            (trainer.clone(), trainer_file),
            (pin_path, pin_file),
        ],
        cheats,
    })
}

#[derive(Debug)]
pub struct PreparedPokes {
    pub candidates: Vec<CheatCandidate>,
    pub selections: Vec<CheatLaunchSelection>,
    pub sources: Vec<SourceBinding>,
}

/// Re-read everything, compare the exact entries the user saw, and bind all
/// source hashes for the existing runtime's checks immediately before handoff.
pub fn prepare(
    old: &PokeLaunchPreview,
    current: &ClassicTrainerGame,
    roots: &TrustedRoots,
    core: &Path,
    frontend: &Path,
    selected: &[usize],
) -> Result<PreparedPokes, String> {
    if selected.is_empty() {
        return Err("No POKE entries selected; use ordinary Play.".into());
    }
    let fresh = preview(current, &old.path, roots)?;
    if &fresh != old {
        return Err(
            "Trainer entries, pin, game or DAT identity changed. Refresh and select again.".into(),
        );
    }
    let mut prepared = compose(&fresh, selected)?;
    prepared.sources.push(verified_backend_file(
        core,
        VERIFIED_FUSE_CORE_SHA256,
        "Fuse core",
        8 * 1024 * 1024,
    )?);
    prepared.sources.push(verified_backend_file(
        frontend,
        VERIFIED_RETROARCH_SHA256,
        "native RetroArch frontend",
        32 * 1024 * 1024,
    )?);
    Ok(prepared)
}

fn verified_backend_file(
    path: &Path,
    expected: &str,
    label: &str,
    maximum: u64,
) -> Result<SourceBinding, String> {
    let roots = TrustedRoots::from_paths([path.parent().ok_or("Backend file has no parent.")?]);
    let (_, file) = observe(path, &roots, maximum)?;
    if file.sha256 != expected {
        return Err(format!(
            "This {label} build has not been verified for POK activation. Ordinary Play is still available."
        ));
    }
    Ok(SourceBinding {
        path: path.into(),
        expected_sha256: file.sha256,
    })
}

fn compose(fresh: &PokeLaunchPreview, selected: &[usize]) -> Result<PreparedPokes, String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut addresses = std::collections::BTreeSet::new();
    let mut candidates = Vec::new();
    let mut selections = Vec::new();
    for &index in selected {
        if !seen.insert(index) {
            return Err("Duplicate trainer selection is ambiguous.".into());
        }
        let cheat = fresh
            .cheats
            .get(index)
            .ok_or("Selected trainer entry no longer exists.")?;
        let projection = project_poke_runtime(cheat, PokeRuntimeEmulator::Fuse);
        if let Some(reason) = projection.refusal {
            return Err(format!(
                "{} cannot be projected exactly: {reason:?}",
                cheat.title
            ));
        }
        for op in &cheat.operations {
            if op.bank != crate::classic_poke_family::PokeBank::Number(8)
                || op.address.0 < 16384
                || !addresses.insert(op.address.0)
            {
                return Err("Only non-overlapping bank 8 RAM writes are verified for this launch; banked, ROM and overlapping POKEs are refused.".into());
            }
        }
        // Fuse's retro_cheat_set expects M/Z records separated by literal
        // backslash+n, with no N title or Y terminator. The native core handler
        // (0) is essential: RetroArch's generic memory handler is not equivalent.
        let code = projection.commands_or_files[0]
            .lines()
            .filter(|l| l.starts_with("M ") || l.starts_with("Z "))
            .collect::<Vec<_>>()
            .join("\\n");
        if cheat.title.contains(['"', '\\']) {
            return Err(
                "Trainer title cannot be represented faithfully in RetroArch configuration.".into(),
            );
        }
        let text = format!(
            "cheats = 1\ncheat0_desc = \"{}\"\ncheat0_code = \"{code}\"\ncheat0_enable = false\ncheat0_handler = 0\n",
            cheat.title
        );
        let entry = parse_cht_text(&text)
            .map_err(|e| format!("Fuse cheat container refused: {e:?}"))?
            .entries
            .remove(0);
        if !entry.is_selectable() {
            return Err("Selected trainer cannot be represented faithfully in the bounded Fuse cheat container.".into());
        }
        let digest = &fresh.observations[0].1.sha256;
        let applicability = assess_cheat_applicability(&CheatApplicabilityInput {
            game: CheatSelectedGame::from_evidence(&[IdentityEvidence {
                kind: IdentityKind::LooseRomSha256,
                status: IdentityStatus::Verified,
                value: Some(digest.clone()),
                confidence: IdentityConfidence::StructuredMetadata,
                provenance: IdentityProvenance {
                    archive_path: fresh.path.clone(),
                    member_path: None,
                    member_index: None,
                    method: "Current DAT-verified snapshot bytes and trainer pin revalidated"
                        .into(),
                },
                diagnostic: String::new(),
            }]),
            association: CheatGameAssociation {
                identities: vec![CheatIdentityRequirement {
                    kind: IdentityKind::LooseRomSha256,
                    value: digest.clone(),
                }],
                ..Default::default()
            },
            document: CheatDocument {
                source_evidence: vec![],
                title: cheat.title.clone(),
                platform: CheatPlatform::Other("ZX Spectrum".into()),
                source_format: CheatSourceFormat::RetroArch,
                operations: vec![],
                issues: vec![],
                provenance: vec!["Verified Fuse-libretro POK native code".into()],
            },
            parsing: CheatParseEvidence::from_cht_entry(&entry),
            native_cht: Some(entry.clone()),
            route: Some(CheatRoute {
                platform_id: "ZX Spectrum".into(),
                target: CheatRouteTarget::retroarch(Some("fuse")),
                basis: CheatRouteBasis::ExplicitSelection,
                apply_support: CheatApplySupport::Supported,
                native_format: "Fuse-libretro M/Z native code in RetroArch .cht",
                alternatives: vec![],
            }),
            reconciliation: None,
        });
        let id = format!("pok:{index}");
        candidates.push(CheatCandidate {
            logical_id: id.clone(),
            title: cheat.title.clone(),
            unresolved_conflict: false,
            variants: vec![CheatVariant {
                variant_id: "pinned-pok".into(),
                source: CheatSourceReference {
                    provider: "verified-pok".into(),
                    source_id: index.to_string(),
                    source_path: Some(fresh.trainer.clone()),
                    source_sha256: Some(fresh.observations[1].1.sha256.clone()),
                    entry_index: Some(index as u32),
                },
                format: CheatLaunchFormat::RetroArchCht,
                applicability,
                entry: Some(entry),
            }],
        });
        selections.push(CheatLaunchSelection {
            logical_id: id,
            variant_id: Some("pinned-pok".into()),
            review_acknowledged: false,
        });
    }
    let sources: Vec<_> = fresh
        .observations
        .iter()
        .map(|(path, file)| SourceBinding {
            path: path.clone(),
            expected_sha256: file.sha256.clone(),
        })
        .collect();
    Ok(PreparedPokes {
        candidates,
        selections,
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::cheat_launch_plan::{
        CheatLaunchPlanStatus, CheatLaunchRequest, CheatLaunchTarget, plan_cheat_launch,
    };
    use crate::patch_manager::CheatApplicabilityMatch;
    use crate::patch_manager::{CheatDerivativeInput, render_retroarch_selected_derivative};

    #[test]
    fn saved_choice_is_bound_to_exact_game_and_trainer_and_never_activates() {
        let (dir, game, path, roots) = fixture();
        let selected_preview = preview(&game, &path, &roots).unwrap();
        let saved = dir.path().join("profiles/choice.json");
        save_profile(&saved, &selected_preview, &[0, 1]).unwrap();
        assert_eq!(load_profile(&saved, &selected_preview).unwrap(), vec![0, 1]);
        assert!(
            !std::fs::read_to_string(&saved)
                .unwrap()
                .contains("activate")
        );
        assert!(save_profile(&saved, &selected_preview, &[0, 0]).is_err());
        assert!(save_profile(&saved, &selected_preview, &[0, 99]).is_err());
        let mut other = game.clone();
        other.identities.push(format!("sha1:{}", "a".repeat(40)));
        let changed = preview(&other, &path, &roots).unwrap();
        assert!(load_profile(&saved, &changed).is_err());
        std::fs::write(path.with_extension("pok"), "N Other\nZ 8 40000 1 0\nY\n").unwrap();
        assert!(preview(&game, &path, &roots).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_choice_refuses_linked_profile_directory() {
        let (dir, game, path, roots) = fixture();
        let selected_preview = preview(&game, &path, &roots).unwrap();
        let actual = dir.path().join("actual");
        std::fs::create_dir(&actual).unwrap();
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&actual, &linked).unwrap();
        assert!(save_profile(&linked.join("choice.json"), &selected_preview, &[0]).is_err());
        assert!(!actual.join("choice.json").exists());
    }

    #[test]
    fn oversized_saved_choice_is_never_published() {
        let (dir, game, path, roots) = fixture();
        let mut selected_preview = preview(&game, &path, &roots).unwrap();
        selected_preview.path = PathBuf::from("x".repeat(4096));
        let saved = dir.path().join("profiles/choice.json");
        assert!(save_profile(&saved, &selected_preview, &[0]).is_err());
        assert!(!saved.exists());
    }

    fn fixture() -> (tempfile::TempDir, ClassicTrainerGame, PathBuf, TrustedRoots) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.z80");
        let mut bytes = vec![0; 30 + 49152];
        bytes[7] = 128;
        std::fs::write(&path, &bytes).unwrap();
        let text = "N Lives\nM 8 40000 99 42\nZ 8 40001 88 0\nN Energy\nZ 8 40002 77 0\nY\n";
        std::fs::write(path.with_extension("pok"), text).unwrap();
        let identity = format!("sha256:{}", hex_sha256(&bytes));
        std::fs::write(
            trainer_target_path(&path),
            format!(
                "trainer-sha256 {}\ngame-sha256 {}\n",
                hex_sha256(text.as_bytes()),
                hex_sha256(&bytes)
            ),
        )
        .unwrap();
        let game = ClassicTrainerGame {
            platform: "ZX Spectrum".into(),
            identities: vec![identity],
            identity_verified: true,
        };
        let roots = TrustedRoots::from_paths([dir.path()]);
        (dir, game, path, roots)
    }

    #[test]
    fn selected_entries_use_fuse_native_handler_and_literal_record_separator() {
        let (_dir, game, path, roots) = fixture();
        let before = std::fs::read(&path).unwrap();
        let preview = preview(&game, &path, &roots).unwrap();
        assert_eq!(preview.titles(), ["Lives", "Energy"]);
        let prepared = compose(&preview, &[0]).unwrap();
        assert_eq!(prepared.candidates.len(), 1);
        assert_eq!(prepared.sources.len(), 3); // snapshot, .pok, pin
        let candidate = &prepared.candidates[0];
        let variant = &candidate.variants[0];
        assert_eq!(
            variant.applicability.identity_match,
            CheatApplicabilityMatch::ExactHash
        );
        let entry = variant.entry.as_ref().unwrap();
        assert!(entry.is_selectable());
        assert_eq!(
            entry.code.as_deref(),
            Some("M 8 40000 99 42\\nZ 8 40001 88 0")
        );
        assert!(entry.extra_fields.contains(&("handler".into(), "0".into())));
        assert!(!entry.enabled_by_default);
        let derivative = render_retroarch_selected_derivative(
            "Fuse",
            "game",
            &[CheatDerivativeInput {
                logical_id: &candidate.logical_id,
                variant_id: &variant.variant_id,
                source: &variant.source,
                entry,
            }],
        )
        .unwrap();
        let output = String::from_utf8(derivative.bytes).unwrap();
        assert!(output.contains("cheat0_handler = \"0\""));
        assert!(output.contains("cheat0_enable = true"));
        assert!(output.contains("M 8 40000 99 42\\nZ 8 40001 88 0"));
        assert!(!output.contains("Energy"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let plan = plan_cheat_launch(&CheatLaunchRequest {
            launch_id: "test".into(),
            target: CheatLaunchTarget {
                adapter_id: "retroarch".into(),
                game_identity: "game".into(),
                identity_verified: true,
            },
            candidates: prepared.candidates,
            selections: vec![],
            retroarch: None,
        });
        assert_eq!(plan.status, CheatLaunchPlanStatus::NoCheatsSelected);
    }

    #[test]
    fn launch_rechecks_game_trainer_pin_current_dat_and_selected_indices() {
        for mutation in 0..5 {
            let (_dir, game, path, roots) = fixture();
            let old = preview(&game, &path, &roots).unwrap();
            let mut current = game.clone();
            match mutation {
                0 => {
                    let mut b = std::fs::read(&path).unwrap();
                    b[40000] = 9;
                    std::fs::write(&path, b).unwrap();
                }
                1 => {
                    std::fs::write(
                        path.with_extension("pok"),
                        "N Different\nZ 8 40000 1 0\nY\n",
                    )
                    .unwrap();
                }
                2 => {
                    std::fs::write(trainer_target_path(&path), "trainer-sha256=wrong\n").unwrap();
                }
                3 => {
                    current.identity_verified = false;
                }
                _ => {
                    current.identities = vec![format!("sha256:{}", "a".repeat(64))];
                }
            }
            assert!(
                prepare(
                    &old,
                    &current,
                    &roots,
                    Path::new("/missing/core"),
                    Path::new("/missing/frontend"),
                    &[0]
                )
                .is_err()
            );
        }
        let (_dir, game, path, roots) = fixture();
        let old = preview(&game, &path, &roots).unwrap();
        assert!(
            prepare(
                &old,
                &game,
                &roots,
                Path::new("/missing/core"),
                Path::new("/missing/frontend"),
                &[]
            )
            .unwrap_err()
            .contains("No POKE")
        );
        assert!(compose(&old, &[9]).is_err());
        assert!(compose(&old, &[0, 0]).is_err());
    }

    #[test]
    fn unknown_core_and_unsupported_or_ambiguous_sources_are_blocked() {
        let (dir, game, path, roots) = fixture();
        let old = preview(&game, &path, &roots).unwrap();
        let core = dir.path().join("fuse_libretro.so");
        std::fs::write(&core, b"not a proven core").unwrap();
        assert!(
            prepare(
                &old,
                &game,
                &roots,
                &core,
                Path::new("/missing/frontend"),
                &[0]
            )
            .unwrap_err()
            .contains("not been verified")
        );
        assert!(
            verified_backend_file(
                &core,
                VERIFIED_RETROARCH_SHA256,
                "native RetroArch frontend",
                32 * 1024 * 1024
            )
            .unwrap_err()
            .contains("native RetroArch frontend")
        );
        std::fs::write(path.with_extension("POK"), b"other").unwrap();
        assert!(
            preview(&game, &path, &roots)
                .unwrap_err()
                .contains("ambiguous")
        );
        std::fs::remove_file(path.with_extension("POK")).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[12] = 32;
        std::fs::write(&path, bytes).unwrap();
        assert!(
            preview(&game, &path, &roots)
                .unwrap_err()
                .contains("uncompressed")
        );
    }

    #[test]
    fn banked_rom_and_overlapping_writes_cannot_be_silently_applied() {
        let (_dir, game, path, roots) = fixture();
        let old = preview(&game, &path, &roots).unwrap();
        for mode in 0..3 {
            let mut edited = old.clone();
            match mode {
                0 => {
                    edited.cheats[0].operations[0].bank =
                        crate::classic_poke_family::PokeBank::Number(5)
                }
                1 => {
                    edited.cheats[0].operations[0].address =
                        crate::classic_poke_family::PokeAddress(100)
                }
                _ => {
                    edited.cheats[1].operations[0].address = edited.cheats[0].operations[0].address
                }
            }
            assert!(compose(&edited, &[0, 1]).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn replaced_and_linked_sources_are_refused_after_selection() {
        let (dir, game, path, roots) = fixture();
        let old = preview(&game, &path, &roots).unwrap();
        let original = dir.path().join("original.z80");
        std::fs::rename(&path, &original).unwrap();
        std::os::unix::fs::symlink(&original, &path).unwrap();
        assert!(
            prepare(
                &old,
                &game,
                &roots,
                Path::new("/missing/core"),
                Path::new("/missing/frontend"),
                &[0]
            )
            .is_err()
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&original, &path).unwrap();
        assert!(preview(&game, &path, &roots).is_err());
    }
}

/// Explicit local evidence gate: never part of normal CI or normal launches.
/// Exercises the real installed frontend and approved core using disposable
/// synthetic content and private HOME/XDG roots; it requires no game ROM.
#[cfg(test)]
mod installed_backend_probe {
    use super::*;
    use crate::patch_manager::{CheatDerivativeInput, render_retroarch_selected_derivative};
    use std::process::{Command, Stdio};

    #[test]
    #[ignore = "opt-in installed RetroArch/Fuse probe using private synthetic fixtures"]
    fn installed_fuse_acknowledges_gold_generated_m_and_z_records() {
        let frontend = std::env::var_os("EMUWIZ_FUSE_PROBE_RETROARCH")
            .expect("set the native RetroArch executable");
        let core = PathBuf::from(
            std::env::var_os("EMUWIZ_FUSE_PROBE_CORE").expect("set the installed core path"),
        );
        let temp = tempfile::Builder::new()
            .prefix("emuwiz-job11-fuse-probe-")
            .tempdir()
            .unwrap();
        let root = temp.path();
        let path = root.join("fixture.z80");
        let mut bytes = vec![0; 30 + 49152];
        bytes[7] = 128;
        bytes[8] = 254;
        bytes[9] = 255;
        bytes[30 + 16384] = 0x76;
        let trainer = "N Probe M and Z\nM 8 40000 99 42\nZ 8 40001 88 0\nY\n";
        std::fs::write(&path, &bytes).unwrap();
        std::fs::write(path.with_extension("pok"), trainer).unwrap();
        let pin = format!(
            "trainer-sha256 {}\ngame-sha256 {}\n",
            hex_sha256(trainer.as_bytes()),
            hex_sha256(&bytes)
        );
        std::fs::write(trainer_target_path(&path), &pin).unwrap();
        let game = ClassicTrainerGame {
            platform: "ZX Spectrum".into(),
            identities: vec![format!("sha256:{}", hex_sha256(&bytes))],
            identity_verified: true,
        };
        let trusted = TrustedRoots::from_paths([root]);
        let seen = preview(&game, &path, &trusted).unwrap();
        let frontend = std::fs::canonicalize(frontend).unwrap();
        let prepared = prepare(&seen, &game, &trusted, &core, &frontend, &[0]).unwrap();
        let candidate = &prepared.candidates[0];
        let variant = &candidate.variants[0];
        let derivative = render_retroarch_selected_derivative(
            "Fuse",
            "fixture",
            &[CheatDerivativeInput {
                logical_id: &candidate.logical_id,
                variant_id: &variant.variant_id,
                source: &variant.source,
                entry: variant.entry.as_ref().unwrap(),
            }],
        )
        .unwrap();
        let cheats = root.join("cheats");
        std::fs::create_dir_all(cheats.join("Fuse")).unwrap();
        std::fs::write(cheats.join(&derivative.relative_name), &derivative.bytes).unwrap();
        for name in [
            "home", "config", "data", "cache", "runtime", "tmp", "saves", "states",
        ] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        let config = root.join("retroarch.cfg");
        std::fs::write(&config, format!("video_driver = \"null\"\naudio_enable = false\ninput_driver = \"null\"\nconfig_save_on_exit = false\napply_cheats_after_load = true\nsavestate_auto_load = false\nlibretro_log_level = 0\ncheat_database_path = \"{}\"\nsavefile_directory = \"{}\"\nsavestate_directory = \"{}\"\n", cheats.display(), root.join("saves").display(), root.join("states").display())).unwrap();
        let log = root.join("probe.log");
        let out = std::fs::File::create(&log).unwrap();
        let mut child = Command::new(&frontend)
            .args(["--verbose", "--max-frames", "2", "--config"])
            .arg(&config)
            .arg("-L")
            .arg(&core)
            .arg(&path)
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("TMPDIR", root.join("tmp"))
            .env("TMP", root.join("tmp"))
            .env("TEMP", root.join("tmp"))
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out)
            .spawn()
            .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(25);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= until {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!(
                    "probe timed out: {}",
                    std::fs::read_to_string(&log).unwrap()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(status.success(), "{text}");
        let expected = "Enabled cheat #0: M 8 40000 99 42\\nZ 8 40001 88 0";
        assert_eq!(
            text.matches(expected).count(),
            2,
            "Both POK operations must be acknowledged by Fuse, not just the frontend loading the file: {text}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::read_to_string(path.with_extension("pok")).unwrap(),
            trainer
        );
        assert_eq!(
            std::fs::read_to_string(trainer_target_path(&path)).unwrap(),
            pin
        );
        eprintln!(
            "Verified core digest: {VERIFIED_FUSE_CORE_SHA256}; Fuse acknowledged both Gold-generated M/Z writes. This is synthetic API evidence, not gameplay evidence."
        );
    }
}
