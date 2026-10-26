//! Verified classic C64 POKEs composed into the existing disposable runtime.
//! No launcher or trainer interpreter: parsing, projection, monitor encoding,
//! receipts and cleanup all remain their existing Gold implementations.
use super::pinning::{Observation, observe};
use super::*;
use crate::classic_poke_family::find_poke_conflicts;
use crate::launch::cheat_launch_plan::{
    FingerprintKind, LaunchStateClass, LaunchStateExpectation, StateExpectation, ViolationSeverity,
};
use crate::launch::cheat_runtime::{
    CheatRuntimePlan, CheatRuntimePlanParts, ReceiptCheat, RuntimeFile, RuntimeFileKind,
    SourceBinding, workspace_name,
};
use crate::launch::vice_command::ViceCheatLaunch;
use crate::launch::vice_execution::ViceMonitorScript;
use crate::patch_manager::{
    CheatOperation, ViceCheatIdentity, project_vice_c64_pokes, validate_destination_root,
};
use crate::safe_read::TrustedRoots;

pub const LIMITATION: &str = "C64 trainers are optional and default OFF. Only the verified native VICE/standard 8 KiB CRT lane supports one bundled simple-POKE sidecar, pinned to this exact DAT-verified game. Only unguarded, non-overlapping byte writes to $C000–$CFFF are supported. Writes run once at startup; they are not a freeze and the game may overwrite them. Prepared commands and process start do not confirm in-game activation.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    game: ClassicTrainerGame,
    path: PathBuf,
    cheats: Vec<PokeCheat>,
    observations: Vec<(PathBuf, Observation)>,
}
impl Preview {
    pub fn titles(&self) -> Vec<String> {
        self.cheats.iter().map(|c| c.title.clone()).collect()
    }
    pub fn game_path(&self) -> &Path {
        &self.path
    }
    pub fn compatibility(&self, index: usize) -> Result<(), String> {
        compatible(
            self.cheats
                .get(index)
                .ok_or("Trainer entry no longer exists.")?,
        )
    }
    pub fn operation_count(&self, index: usize) -> usize {
        self.cheats.get(index).map_or(0, |c| c.operations.len())
    }
    pub fn operation_preview(&self, index: usize) -> String {
        self.cheats.get(index).map_or_else(String::new, |cheat| {
            project_poke_runtime(cheat, PokeRuntimeEmulator::Vice).preview
        })
    }
    pub fn source_label(&self) -> &str {
        &self.cheats[0].source
    }
}

pub fn preview(
    game: &ClassicTrainerGame,
    path: &Path,
    roots: &TrustedRoots,
) -> Result<Preview, String> {
    if !game.identity_verified
        || poke_platform_for_label(&game.platform) != Some(PokePlatform::Commodore64)
    {
        return Err("Current exact C64 DAT identity is required.".into());
    }
    let parent = path.parent().ok_or("Game has no parent folder.")?;
    validate_destination_root(parent).map_err(|e| format!("Unsafe source folder: {e}"))?;
    if !roots.contains_canonical(parent) {
        return Err("Game is outside configured trusted roots.".into());
    }
    let (game_bytes, game_file) = observe(path, roots, 8272)?;
    crate::disk_format::crt::verify_standard_8k_rom(path, &game_bytes)?;
    let candidates: Vec<_> = ["pok", "POK"]
        .iter()
        .map(|e| path.with_extension(e))
        .filter(|p| p.symlink_metadata().is_ok())
        .collect();
    let [trainer] = candidates.as_slice() else {
        return Err("Exactly one same-name .pok/.POK sidecar is required; missing or ambiguous sources are refused.".into());
    };
    let (source, trainer_file) = observe(trainer, roots, TRAINER_FILE_MAX_BYTES)?;
    let target = trainer_target_path(path);
    let (pin, pin_file) = observe(&target, roots, TRAINER_TARGET_MAX_BYTES)?;
    let (expected_trainer, identity) =
        parse_pin(std::str::from_utf8(&pin).map_err(|_| "Pin is not UTF-8.")?)?;
    if expected_trainer != trainer_file.sha256
        || match_target(game, &identity, true) != ClassicTrainerMatch::VerifiedIdentity
    {
        return Err("Trainer source or pin no longer matches this DAT-verified game.".into());
    }
    let (kind, expected) = identity.split_once(':').ok_or("Invalid pinned hash.")?;
    let actual: String = match kind {
        "sha256" => game_file.sha256.clone(),
        "sha1" => sha1::Sha1::digest(&game_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        "md5" => md5::Md5::digest(&game_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        _ => return Err("Unsupported exact game hash.".into()),
    };
    if actual != expected {
        return Err("Game bytes differ from the current DAT identity.".into());
    }
    let mut cheats = parse_sidecar_trainers(
        game,
        std::str::from_utf8(&source).map_err(|_| "Trainer is not UTF-8.")?,
    )?;
    for cheat in &mut cheats {
        cheat.target_identity = Some(identity.clone());
        cheat.target_verified = true;
        cheat.source = trainer.file_name().map_or_else(
            || "local C64 sidecar".into(),
            |s| s.to_string_lossy().into_owned(),
        );
    }
    Ok(Preview {
        game: game.clone(),
        path: path.into(),
        cheats,
        observations: vec![
            (path.into(), game_file),
            (trainer.clone(), trainer_file),
            (target, pin_file),
        ],
    })
}

fn compatible(cheat: &PokeCheat) -> Result<(), String> {
    let projected = project_poke_runtime(cheat, PokeRuntimeEmulator::Vice);
    if let Some(reason) = projected.refusal {
        return Err(format!("Gold VICE projection refused: {reason:?}"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for op in &cheat.operations {
        if !(0xc000..=0xcfff).contains(&op.address.0) {
            return Err("Only startup RAM bytes at $C000–$CFFF are supported by this initial cartridge contract.".into());
        }
        if !seen.insert(op.address.0) {
            return Err("Overlapping writes are ambiguous and are refused.".into());
        }
    }
    if cheat.operations.is_empty() || !find_poke_conflicts(std::slice::from_ref(cheat)).is_empty() {
        return Err("No unambiguous operations are available.".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    game_key: String,
    bytes: Vec<u8>,
    sources: Vec<SourceBinding>,
    cheats: Vec<ReceiptCheat>,
}

pub fn prepare(
    old: &Preview,
    game: &ClassicTrainerGame,
    roots: &TrustedRoots,
    selected: &[usize],
    game_key: &str,
) -> Result<Prepared, String> {
    if selected != [0] {
        return Err("Explicit selection of the whole C64 trainer bundle is required; empty, duplicate or unknown selections are refused.".into());
    }
    let fresh = preview(game, &old.path, roots)?;
    if &fresh != old {
        return Err(
            "Game, trainer entries, pin or identity changed. Refresh and select again.".into(),
        );
    }
    let game_sha256 = &fresh.observations[0].1.sha256;
    if game_key != format!("sha256:{game_sha256}") {
        return Err("Trainer and VICE game identity disagree.".into());
    }
    let cheat = &fresh.cheats[0];
    compatible(cheat)?;
    let operations: Vec<_> = cheat
        .operations
        .iter()
        .map(|op| CheatOperation::Write8 {
            address: u64::from(op.address.0),
            value: op.value.value as u8,
        })
        .collect();
    let projection = project_vice_c64_pokes(
        &operations,
        &ViceCheatIdentity::VerifiedGame(game_key.into()),
    );
    let bytes = ViceMonitorScript::render(&projection, game_key).map_err(|e| e.detail)?;
    Ok(Prepared {
        game_key: game_key.into(),
        bytes,
        sources: fresh
            .observations
            .iter()
            .map(|(path, file)| SourceBinding {
                path: path.clone(),
                expected_sha256: file.sha256.clone(),
            })
            .collect(),
        cheats: vec![ReceiptCheat {
            logical_id: "c64-poke-bundle-0".into(),
            variant_id: "startup-c000-cfff".into(),
            title: cheat.title.clone(),
            provider: "local-classic-poke".into(),
            source_id: cheat.source.clone(),
        }],
    })
}

impl Prepared {
    /// Canonical runtime material; this performs no writes and starts no process.
    pub fn runtime_plan(
        &self,
        launch_id: &str,
        approved_root: &Path,
        executable: &Path,
        executable_sha256: &str,
    ) -> Result<(CheatRuntimePlan, ViceCheatLaunch), String> {
        let root = approved_root.join(workspace_name(launch_id));
        let script = RuntimeFile::new(
            root.join("commands.txt"),
            self.bytes.clone(),
            RuntimeFileKind::CheatMaterial,
        );
        let handoff = ViceCheatLaunch {
            game_key: self.game_key.clone(),
            script_path: script.path.clone(),
            script_sha256: script.sha256.clone(),
        };
        let mut sources = self.sources.clone();
        sources.push(SourceBinding {
            path: executable.into(),
            expected_sha256: executable_sha256.into(),
        });
        let mut expectations: Vec<_> = sources
            .iter()
            .map(|s| LaunchStateExpectation {
                path: s.path.clone(),
                class: LaunchStateClass::ReadOnlySource,
                expectation: StateExpectation::MustRemainUnchanged,
                fingerprint: FingerprintKind::Sha256,
                severity: ViolationSeverity::LaunchAffecting,
            })
            .collect();
        expectations.push(LaunchStateExpectation {
            path: root.clone(),
            class: LaunchStateClass::EphemeralRuntime,
            expectation: StateExpectation::MustNotExistAfter,
            fingerprint: FingerprintKind::NotPresent,
            severity: ViolationSeverity::Warning,
        });
        let plan = CheatRuntimePlan::new(CheatRuntimePlanParts {
            launch_id: launch_id.into(),
            adapter_id: "vice".into(),
            game_identity: self.game_key.clone(),
            root,
            directories: vec![],
            files: vec![script],
            extra_arguments: vec![
                "-moncommands".into(),
                handoff.script_path.clone().into_os_string(),
            ],
            sources,
            expectations,
            cheats: self.cheats.clone(),
        })
        .map_err(|e| e.to_string())?;
        Ok((plan, handoff))
    }
}

#[cfg(test)]
mod tests;
