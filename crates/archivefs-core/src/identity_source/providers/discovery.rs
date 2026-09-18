//! Explicitly non-authoritative clues. Never constructs an official match.
#![allow(clippy::items_after_test_module)]
use super::{MatchStatus, ProviderResult};
use crate::platform_evidence_fusion::evidence_lineage::ClaimStrength;
use crate::safe_read::{TrustedRoots, open_bounded_read};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, io::Read, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryClue {
    pub engine: String,
    pub observation: String,
    pub strength: ClaimStrength,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResult {
    pub status: MatchStatus,
    pub probable_engines: Vec<String>,
    pub clues: Vec<DiscoveryClue>,
}

/// Human-facing fallback only. It never creates an official game identity.
pub fn cleaned_local_title(name: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    let cleaned = stem.split(" ( ").next().unwrap_or(stem);
    let cleaned = cleaned.split(" (").next().unwrap_or(cleaned);
    cleaned.replace('_', " ").trim().to_string()
}

pub fn inspect_scummvm_target(path: &Path) -> ProviderResult<Option<String>> {
    let mut target = None;
    for entry in fs::read_dir(path).map_err(|e| e.to_string())?.take(2049) {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err("Discovery refuses symbolic entries".into());
        }
        if ty.is_file() && entry.path().extension().and_then(|e| e.to_str()) == Some("scummvm") {
            let safe = open_bounded_read(&entry.path(), &TrustedRoots::from_paths([path]))
                .map_err(|e| format!(".scummvm read refused: {e:?}"))?;
            if safe.len() > 4096 {
                return Err(".scummvm launcher evidence exceeds 4 KiB".into());
            }
            let mut bytes = Vec::with_capacity(safe.len() as usize);
            safe.into_file()
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let value = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            let value = value.trim().to_string();
            if !value.is_empty() {
                target = Some(value);
                break;
            }
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_fallback_removes_extension_and_release_tags() {
        assert_eq!(
            cleaned_local_title("Day Of The Tentacle (CD Dos).7z"),
            "Day Of The Tentacle"
        );
        assert_eq!(cleaned_local_title("my_game.zip"), "my game");
    }
}

/// No title is inferred from the folder name. These recognizable filename
/// combinations justify investigation, not a claim that the engine can run it.
pub fn from_names(names: &[String]) -> DiscoveryResult {
    let names: BTreeSet<_> = names.iter().map(|s| s.to_ascii_lowercase()).collect();
    let mut clues = Vec::new();
    let mut add = |engine: &str, observation: &str| {
        clues.push(DiscoveryClue {
            engine: engine.into(),
            observation: observation.into(),
            strength: ClaimStrength::Weak,
        })
    };
    if names.contains("resource.map")
        && names.iter().any(|n| {
            n.starts_with("resource.") && n[9..].bytes().all(|c| c.is_ascii_digit()) && n.len() > 9
        })
    {
        add(
            "sci",
            "resource.map with numbered resource volumes; filenames only",
        );
    }
    if names.contains("ac2game.dat") {
        add(
            "ags",
            "ac2game.dat filename; not an official signature match",
        );
    }
    if names.contains("data.dcp") {
        add(
            "wintermute",
            "data.dcp filename; container contents not yet confirmed",
        );
    }
    for name in &names {
        if let Some(base) = name.strip_suffix(".000")
            && names.contains(&format!("{base}.001"))
            && base != "resource"
        {
            add(
                "scumm",
                "paired .000/.001 resources; filenames only, also possible in unrelated formats",
            );
            break;
        }
    }
    let engines: BTreeSet<_> = clues.iter().map(|c| c.engine.clone()).collect();
    let status = match engines.len() {
        0 => MatchStatus::NoMatch,
        1 => MatchStatus::Probable,
        _ => MatchStatus::Ambiguous,
    };
    DiscoveryResult {
        status,
        probable_engines: engines.into_iter().collect(),
        clues,
    }
}

pub fn inspect(path: &Path) -> ProviderResult<DiscoveryResult> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| e.to_string())?.take(2049) {
        let entry = entry.map_err(|e| e.to_string())?;
        if names.len() == 2048 {
            return Err(
                "Discovery exceeds 2048 immediate entries; no partial result promoted".into(),
            );
        }
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    Ok(from_names(&names))
}
