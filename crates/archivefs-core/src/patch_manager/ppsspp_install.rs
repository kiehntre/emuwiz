//! Proven new-file-only CWCheat slice; see docs/ppsspp-cheat-install.md.
use super::*;
use crate::game_identity::{GameIdentityReport, IdentityKind, IdentityStatus};
use crate::safe_read::{TrustedRoots, open_bounded_read};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
type Checked<T> = std::result::Result<T, String>;
const APPIMAGE: &str = "540f317c1ed6661894167d2dce21b62295857e1a912274982da7048cf2256466";
pub const SOURCE_MODE: &str = "ppsspp-1.19.3-disabled-local:";

#[derive(Clone, Debug)]
pub struct Environment {
    pub roots: PpssppProfileDiscoveryRoots,
    pub executable: PathBuf,
    pub profile_id: String,
    pub proc_root: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub profile_root: PathBuf,
    pub cheats_root: PathBuf,
    pub guards: Vec<(PathBuf, String)>,
}
fn hash(path: &Path, maximum: u64) -> Checked<String> {
    let safe = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    if safe.len() > maximum {
        return Err("File exceeds supported inspection bound".into());
    }
    let before =
        crate::identity_source::hashing::FileFingerprint::observe(path).ok_or("Unsafe file")?;
    let mut file = safe.into_file();
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > maximum {
            return Err("File grew past inspection bound".into());
        }
        hasher.update(&chunk[..n]);
    }
    if crate::identity_source::hashing::FileFingerprint::observe(path).as_ref() != Some(&before)
        || total != before.size_bytes
    {
        return Err("File changed during inspection".into());
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn absent(path: &Path) -> Checked<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(format!(
            "Existing or uncertain path is preserved: {}",
            path.display()
        )),
    }
}
/// The first environment variable name that overrides how an AppImage or its
/// libraries load (portable-runtime and dynamic-linker overrides), if any.
/// Pure, so the refusal policy is testable without touching the real process
/// environment.
fn runtime_override_in(keys: impl IntoIterator<Item = std::ffi::OsString>) -> Option<String> {
    keys.into_iter().find_map(|key| {
        let key = key.to_string_lossy();
        (key.starts_with("URUNTIME_")
            || key.starts_with("SHARUN_")
            || matches!(
                key.as_ref(),
                "TARGET_APPIMAGE" | "APPIMAGE_EXTRACT_AND_RUN" | "LD_PRELOAD" | "LD_LIBRARY_PATH"
            ))
        .then(|| key.into_owned())
    })
}

/// Variable names of the real process environment. Under test this is a
/// per-test (thread-local) set, empty by default: the Cargo test harness itself
/// exports `LD_LIBRARY_PATH`, which must not decide the outcome of a fixture.
fn process_environment_keys() -> Vec<std::ffi::OsString> {
    #[cfg(test)]
    {
        tests::TEST_ENVIRONMENT_KEYS.with(|keys| keys.borrow().clone())
    }
    #[cfg(not(test))]
    {
        std::env::vars_os().map(|(key, _)| key).collect()
    }
}

impl Environment {
    pub fn resolve(&self) -> Checked<Binding> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err("Only Linux x86-64 is supported".into());
        }
        if let Some(key) = runtime_override_in(process_environment_keys()) {
            return Err(format!("Unsupported runtime override: {key}"));
        }
        for suffix in [".env", ".home", ".config", ".share"] {
            let mut path = self.executable.as_os_str().to_os_string();
            path.push(suffix);
            absent(Path::new(&path))?;
        }
        if let Some(problem) =
            crate::emulator_profile_resolver::fsfacts::executable_problem(&self.executable)
        {
            return Err(format!("Executable unavailable: {problem:?}"));
        }
        let executable_hash = hash(&self.executable, 128 * 1024 * 1024)?;
        let supported = executable_hash == APPIMAGE;
        #[cfg(test)]
        let supported = supported
            || executable_hash
                == Sha256::digest(b"synthetic pinned PPSSPP")
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
        if !supported {
            return Err(
                "Unsupported build: select the exact official PPSSPP 1.19.3 Linux x86-64 AppImage"
                    .into(),
            );
        }
        let mut roots = self.roots.clone();
        // Explicit executable choice, not the first program found on PATH.
        roots.explicit_executables = vec![self.executable.clone()];
        roots.path_override = Some(vec![]);
        let profile = discover_ppsspp_profiles(&roots)
            .profiles
            .into_iter()
            .find(|p| p.profile_id == self.profile_id)
            .ok_or("Selected PPSSPP profile disappeared")?;
        if !profile.eligible
            || profile.configuration_path != roots.xdg_config_home.join("ppsspp")
            || profile.installation_type != PpssppInstallationType::Native
        {
            return Err("Only the existing default XDG PPSSPP profile is supported; custom, data-directory and Flatpak layouts are refused".into());
        }
        let root =
            super::destination_safety::validate_destination_root(&profile.configuration_path)
                .map_err(|e| e.to_string())?;
        let cheats = super::destination_safety::validate_destination_root(&profile.cheats_path)
            .map_err(|e| e.to_string())?;
        ensure_stopped(&self.proc_root, &self.executable)?;
        Ok(Binding {
            profile_root: root.path().into(),
            cheats_root: cheats.path().into(),
            guards: vec![
                (self.executable.clone(), executable_hash),
                (
                    profile.global_config_path.clone(),
                    hash(&profile.global_config_path, PPSSPP_MAX_CONFIG_BYTES)?,
                ),
            ],
        })
    }
}
#[cfg(unix)]
fn ensure_stopped(proc_root: &Path, executable: &Path) -> Checked<()> {
    use std::os::unix::fs::MetadataExt;
    let own = fs::metadata(executable).map_err(|e| e.to_string())?;
    for (n, entry) in fs::read_dir(proc_root)
        .map_err(|e| e.to_string())?
        .enumerate()
    {
        if n >= 16384 {
            return Err(
                "Process inspection incomplete; close PPSSPP before changing cheats".into(),
            );
        }
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        let comm = match fs::read_to_string(entry.path().join("comm")) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("Process state is unavailable; installation refused".into()),
        };
        let same = fs::metadata(entry.path().join("exe"))
            .is_ok_and(|m| m.dev() == own.dev() && m.ino() == own.ino());
        if same || comm.to_ascii_lowercase().contains("ppsspp") {
            return Err("Close all PPSSPP instances before Preview, Install or Undo".into());
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn ensure_stopped(_: &Path, _: &Path) -> Checked<()> {
    Err("Linux process inspection required".into())
}
pub fn disc_id(report: &GameIdentityReport, game: &Path) -> Checked<String> {
    if report.archive_path != game || !report.complete || report.verified_psp_disc_id().is_none() {
        return Err("Complete identity for this exact selected game is required".into());
    }
    let facts: Vec<_> = report
        .evidence
        .iter()
        .filter(|f| f.kind == IdentityKind::PspDiscId)
        .collect();
    if facts.is_empty() || facts.iter().any(|f| f.status != IdentityStatus::Verified) {
        return Err("PSP Disc ID is unavailable or conflicting".into());
    }
    let ids: BTreeSet<_> = facts
        .iter()
        .map(|f| f.value.as_deref().and_then(normalize_game_id))
        .collect();
    if ids.len() != 1 || ids.contains(&None) {
        return Err("PSP Disc IDs are malformed or ambiguous".into());
    }
    let id = ids.into_iter().next().flatten().unwrap();
    if !id.as_bytes()[..4].iter().all(u8::is_ascii_uppercase) {
        return Err("Unsupported PSP Disc ID".into());
    }
    Ok(id)
}
fn fresh_game(game: &Path, expected: &str) -> Checked<String> {
    // Shared materialized guards intentionally retain their existing 512 MiB bound.
    let digest = hash(game, SHARED_MAX_MATERIALIZED_BYTES)?;
    let report = crate::game_identity::inspect_game_identity(game, Some("PSP"));
    if disc_id(&report, game)? != expected {
        return Err("Selected PSP identity changed".into());
    }
    Ok(digest)
}
pub fn disabled_file(bytes: &[u8], id: &str) -> Checked<PpssppCheatFile> {
    let text = std::str::from_utf8(bytes).map_err(|_| "CWCheat must be valid UTF-8")?;
    for line in text.lines().map(str::trim) {
        if line.len() >= 2047 || line.chars().any(|c| c.is_control() && c != '\t') {
            return Err("CWCheat line exceeds the supported PPSSPP parser contract".into());
        }
        if !line.is_empty()
            && !line.starts_with('#')
            && !line.starts_with("//")
            && !["_S ", "_G ", "_C0 ", "_C1 ", "_L "]
                .iter()
                .any(|p| line.starts_with(p))
        {
            return Err("Unsupported CWCheat directive; no conversion performed".into());
        }
    }
    let mut file = parse_ppsspp_cwcheat(bytes).map_err(|e| format!("{e:?}"))?;
    if file.game_id != id || file.game.cheats.is_empty() {
        return Err("Cheat source must contain entries for the exact verified PSP Disc ID".into());
    }
    let mut names = BTreeSet::new();
    for entry in &mut file.game.cheats {
        if entry.title.is_empty()
            || !names.insert(entry.title.clone())
            || entry.lines.is_empty()
            || entry
                .lines
                .iter()
                .any(|l| l.supported_operation.is_none() || !l.issues.is_empty())
        {
            return Err("Only unambiguous CWCheat Write32 entries are supported".into());
        }
        entry.enabled = false;
    }
    Ok(file)
}
#[derive(Debug)]
pub struct Preview {
    pub cheat_count: usize,
    pub native: PpssppCwCheatPreview,
    pub binding: Binding,
    pub game: PathBuf,
    pub id: String,
    pub source: PathBuf,
    guards: Vec<(PathBuf, String)>,
}
pub fn preview(env: &Environment, game: &Path, id: &str, source: &Path) -> Checked<Preview> {
    let binding = env.resolve()?;
    let game_hash = fresh_game(game, id)?;
    let source_hash = hash(source, PPSSPP_CWCHEAT_MAX_BYTES)?;
    let mut bytes = Vec::new();
    open_bounded_read(source, &TrustedRoots::none())
        .map_err(|e| e.detail())?
        .into_file()
        .take(PPSSPP_CWCHEAT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let file = disabled_file(&bytes, id)?;
    let profile = {
        let mut roots = env.roots.clone();
        roots.explicit_executables = vec![env.executable.clone()];
        roots.path_override = Some(vec![]);
        discover_ppsspp_profiles(&roots)
            .profiles
            .into_iter()
            .find(|p| p.profile_id == env.profile_id)
            .ok_or("Profile disappeared")?
    };
    absent(&binding.cheats_root.join(format!("{id}.ini")))?;
    let mut native = build_ppsspp_cwcheat_preview(&profile, id, game, &file)?;
    let provenance =
        serde_json::to_string(&(SharedTransactionPath::from_path(source), &source_hash))
            .map_err(|e| e.to_string())?;
    native.transaction = build_shared_transaction_plan(
        &native.report,
        &profile.profile_id,
        &format!("{SOURCE_MODE}{provenance}"),
        &native.staged.staging_root,
    )
    .map_err(|e| format!("{e:?}"))?;
    if native.transaction.entries.len() != 1
        || native
            .report
            .entries
            .iter()
            .any(|e| e.state != PreviewState::InstallNew)
    {
        return Err("Only a new cheat file can be installed; existing files are preserved".into());
    }
    let mut guards = binding.guards.clone();
    guards.push((game.into(), game_hash));
    guards.push((source.into(), source_hash));
    for (p, h) in &guards {
        if hash(p, SHARED_MAX_MATERIALIZED_BYTES)? != *h {
            return Err("Source or identity changed during Preview".into());
        }
    }
    if env.resolve()? != binding {
        return Err("PPSSPP profile changed during Preview".into());
    }
    Ok(Preview {
        cheat_count: file.game.cheats.len(),
        native,
        binding,
        game: game.into(),
        id: id.into(),
        source: source.into(),
        guards,
    })
}
pub fn apply(
    env: &Environment,
    p: &Preview,
    options: &SharedApplyOptions,
) -> Checked<SharedApplyResult> {
    if env.resolve()? != p.binding || fresh_game(&p.game, &p.id)? != p.guards[p.guards.len() - 2].1
    {
        return Err("Profile or selected game changed; Preview again".into());
    }
    let first = &p.native.transaction.entries[0];
    let output = SharedMaterializedOutput {
        source_path: first
            .source_path
            .to_path_buf()
            .map_err(|e| format!("{e:?}"))?,
        source_digest: first.source_digest.clone(),
        output_digest: p.native.staged.digest.clone(),
        bytes: p.native.staged.contents.as_bytes().to_vec(),
        guard_paths: p.guards.clone(),
    };
    Ok(execute_shared_materialized_apply(
        &p.native.transaction,
        options,
        &output,
    ))
}
pub fn history(
    env: &Environment,
    game: &Path,
    id: &str,
    root: &Path,
) -> Checked<Vec<(SharedTransactionPath, SharedApplyJournal)>> {
    let binding = env.resolve()?;
    let h = discover_shared_apply_history(root);
    if !h.complete || !h.warnings.is_empty() {
        return Err("History incomplete; inspect recovery evidence before continuing".into());
    }
    Ok(h.journals
        .into_iter()
        .filter(|(_, j)| {
            j.context.adapter == PreviewAdapter::Ppsspp
                && j.context.source_mode.starts_with(SOURCE_MODE)
                && j.context.selected_archive == SharedTransactionPath::from_path(game)
                && j.context.verified_game_identity == id
                && j.context.profile_id == env.profile_id
                && j.destination_root == SharedTransactionPath::from_path(&binding.cheats_root)
        })
        .collect())
}
pub fn undo_preview(
    env: &Environment,
    game: &Path,
    id: &str,
    journal: &Path,
    history_root: &Path,
    backups: &Path,
) -> Checked<(SharedRollbackPreview, Binding)> {
    if !history(env, game, id, history_root)?
        .iter()
        .any(|(p, _)| *p == SharedTransactionPath::from_path(journal))
    {
        return Err("Receipt does not belong to this game, profile and install workflow".into());
    }
    let binding = env.resolve()?;
    let preview = preview_shared_rollback(journal, &binding.cheats_root, backups);
    if !preview.available {
        return Err(format!("Undo refused: {:?}", preview.entries));
    }
    Ok((preview, binding))
}
pub fn undo(
    env: &Environment,
    preview: &SharedRollbackPreview,
    binding: &Binding,
    options: &SharedRollbackOptions,
) -> Checked<SharedRollbackResult> {
    if env.resolve()? != *binding {
        return Err("Profile changed since Undo review".into());
    }
    Ok(execute_shared_rollback(preview, options))
}
#[cfg(test)]
mod tests;
