//! Existing native archive readers over explicit paths only (no directory walk).
use crate::dat::archive::{
    ArchiveMemberSource, ArchiveRunBudget, limits::ArchiveLimits, sevenz::SevenZArchiveSource,
    zip::ZipArchiveSource,
};
use crate::identity_source::hashing::FileFingerprint;
use crate::safe_read::TrustedRoots;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
pub(super) type MemberMap = BTreeMap<(String, u64, String), Vec<String>>;
pub(super) fn read_members(
    path: &Path,
    platform: &str,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    budget: &mut ArchiveRunBudget,
) -> Result<Vec<(u64, String, String)>, String> {
    let generation = FileFingerprint::observe(path).ok_or("Archive reference unavailable")?;
    let is_lha = path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("lha") || s.eq_ignore_ascii_case("lzh"));
    let whd_package = if is_lha && platform == "Amiga" {
        whdload_package(path, cancel)?
    } else {
        false
    };
    let mut source: Box<dyn ArchiveMemberSource> = match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "zip" => Box::new(
            ZipArchiveSource::open(path, trusted, ArchiveLimits::default(), cancel)
                .map_err(|e| format!("ZIP: {e:?}"))?,
        ),
        "7z" => Box::new(
            SevenZArchiveSource::open(path, trusted, ArchiveLimits::default(), cancel)
                .map_err(|e| format!("7z: {e:?}"))?,
        ),
        "lha" | "lzh" => Box::new(
            crate::dat::archive::lha::LhaProvider::discover(std::time::Duration::from_secs(5))
                .map_err(|e| format!("LHA reader unavailable: {e}"))?
                .open(
                    path,
                    trusted,
                    ArchiveLimits::default(),
                    std::time::Duration::from_secs(5),
                )
                .map_err(|e| format!("LHA: {e}"))?,
        ),
        _ => {
            return Err(
                "This archive/container retains its existing specialist import limitation.".into(),
            );
        }
    };
    let pass = source.verify_all(cancel, budget);
    if !pass.is_complete() {
        return Err(format!(
            "Archive inspection incomplete: {:?}",
            pass.completion
        ));
    }
    let mut rows = Vec::new();
    for m in pass.members {
        let extension = Path::new(&m.member_name_display)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if super::discovery::is_known_non_game_extension(&extension) {
            continue;
        }
        if m.is_nested_archive || !m.is_hash_complete() {
            return Err("Nested or unsupported archive content cannot be selected.".into());
        }
        let platforms = crate::platform::platform_candidates_for_extension(&extension);
        if (platforms.is_empty() && !whd_package)
            || (!platforms.is_empty() && !platforms.contains(&platform))
        {
            return Err(format!(
                "Archive member {} is unsupported or conflicts with {platform}.",
                m.member_name_display
            ));
        }
        if let Some(h) = m.hashes {
            rows.push((m.logical_size, h.sha1, h.sha256));
        }
    }
    if rows.is_empty() {
        return Err("Archive has no supported game-content members.".into());
    }
    if FileFingerprint::observe(path).as_ref() != Some(&generation) {
        return Err("Archive changed during inspection.".into());
    }
    Ok(rows)
}
pub(super) fn references(
    known: &[crate::PersistedArchive],
    platforms: &std::collections::BTreeSet<String>,
    root: &Path,
    cancel: &AtomicBool,
    budget: &mut ArchiveRunBudget,
    input_bytes: &mut u64,
) -> Result<(MemberMap, Vec<(PathBuf, FileFingerprint)>), String> {
    let trusted = TrustedRoots::from_paths([root]);
    let mut map = MemberMap::new();
    let mut generations = Vec::new();
    for k in known.iter().filter(|k| {
        k.platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias)
            .is_some_and(|p| platforms.contains(p))
            && matches!(
                k.archive_kind.as_str(),
                "zip" | "sevenzip" | "7z" | "rar" | "lha"
            )
    }) {
        let g = FileFingerprint::observe(&k.absolute_path)
            .ok_or("Indexed archive reference is unavailable")?;
        *input_bytes = input_bytes
            .checked_add(g.size_bytes)
            .ok_or("Archive reference budget overflow")?;
        if *input_bytes > super::preview::MAX_COMPARISON_INPUT_BYTES {
            return Err(
                "Explicit archive reference read budget exceeded; no subset sealed.".into(),
            );
        }
        let platform = k
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias)
            .ok_or("Unknown archive reference platform")?;
        for (size, sha1, sha256) in
            read_members(&k.absolute_path, platform, &trusted, cancel, budget)?
        {
            map.entry((platform.to_string(), size, sha1))
                .or_default()
                .push(sha256);
        }
        generations.push((k.absolute_path.clone(), g));
    }
    Ok((map, generations))
}

/// Reuse the existing bounded slave inspector; multiple variants never become a
/// guessed single game. A whole package remains one container record, including
/// its data files. This does not assign a game identity or verify playability.
pub(super) fn whdload_package(path: &Path, cancel: &AtomicBool) -> Result<bool, String> {
    let result = crate::amiga_whdload_archive::discover_whdload_slaves_in_archive(path, cancel)
        .map_err(|e| format!("WHDLoad inspection unavailable: {e}"))?;
    if !result.diagnostics.is_empty() || !result.warnings.is_empty() {
        return Err(format!(
            "WHDLoad slave inspection incomplete: {:?}; {:?}",
            result.diagnostics, result.warnings
        ));
    }
    if result.candidates.len() > 1 {
        return Err("Multiple WHDLoad slaves remain ambiguous; none selected.".into());
    }
    Ok(result.candidates.len() == 1)
}
