//! Explicit native optical review through existing DAT parsing/hash APIs.
//! No platform hint, extension, profile or library label confers authority.
use archivefs_core::dat::{
    identity::{DatPlatformConfidence, DatPlatformIdentity, identify_dat_source},
    index::DatIndex,
    limits::DatLimits,
    parsers::parse_dat_file,
};
use archivefs_core::launch::{CanonicalIdentityStatus, ResolvedIdentity};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(super) fn verify(path: &Path) -> Result<CanonicalIdentityStatus, String> {
    use archivefs_core::dat::sources::{
        DatSourceKind, DatSourceRegistry, discover_dat_files, load_dat_sources_config_default,
    };
    let config = load_dat_sources_config_default()
        .map_err(|e| format!("DAT registry could not be read: {e}"))?;
    let (registry, problems) = DatSourceRegistry::from_config(&config);
    if !problems.is_empty() {
        return Err(format!(
            "Registered DAT sources need attention: {}",
            problems.join("; ")
        ));
    }
    let mut paths = Vec::new();
    for entry in registry.sorted_enabled() {
        match entry.kind {
            DatSourceKind::File => paths.push(entry.path.clone()),
            DatSourceKind::Folder => {
                let scan = discover_dat_files(&entry.path)
                    .map_err(|e| format!("DAT folder unavailable: {e:?}"))?;
                if scan.truncated || !scan.skipped.is_empty() {
                    return Err("A registered DAT folder could not be completely consulted. Resolve its source diagnostics, then check again.".into());
                }
                paths.extend(scan.files);
            }
        }
    }
    verify_from_paths(path, &paths)
}

pub(super) fn verify_from_paths(
    path: &Path,
    dat_paths: &[PathBuf],
) -> Result<CanonicalIdentityStatus, String> {
    use archivefs_core::identity_source::hashing::hash_file;
    use archivefs_core::safe_read::TrustedRoots;
    if !path.is_absolute() || std::fs::canonicalize(path).ok().as_deref() != Some(path) {
        return Err("Select a real absolute non-symlink disc path, then check again.".into());
    }
    if !path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("iso"))
    {
        return Err("Native optical catalogue review currently supports direct ISO only; descriptor/track identity is not inferred.".into());
    }
    let hashes =
        hash_file(path, &TrustedRoots::none(), None).map_err(|e| e.detail().to_string())?;
    let mut platforms = BTreeSet::new();
    for dat_path in dat_paths {
        // Validate source path as well as content; a broken enabled source is
        // not silently ignored when assessing cross-catalogue ambiguity.
        archivefs_core::launch::process_spawn::open_regular_file_nonblocking(dat_path)
            .map_err(|e| format!("DAT {} unavailable: {e}", dat_path.display()))?;
        let before = archivefs_core::identity_source::hashing::FileFingerprint::observe(dat_path)
            .ok_or("A registered DAT is unavailable.")?;
        let dat = parse_dat_file(dat_path, DatLimits::default())
            .map_err(|e| format!("DAT {} could not be parsed: {e}", dat_path.display()))?
            .dat;
        if archivefs_core::identity_source::hashing::FileFingerprint::observe(dat_path).as_ref()
            != Some(&before)
        {
            return Err("A registered DAT changed during review. Check again.".into());
        }
        let index = DatIndex::build(&dat);
        let matches: Vec<_> = index
            .lookup_sha1(&hashes.sha1)
            .iter()
            .filter(|m| m.size_bytes == Some(hashes.bytes_hashed))
            .collect();
        if matches.is_empty() {
            continue;
        }
        if matches.iter().any(|m| {
            m.status
                .as_deref()
                .is_some_and(|s| !s.eq_ignore_ascii_case("good"))
        }) {
            return Err("The matched DAT member has a dump status requiring review.".into());
        }
        match identify_dat_source(&dat){
            DatPlatformIdentity::Resolved{platform,confidence:DatPlatformConfidence::Strong,..}=>{platforms.insert(platform);},
            _=>return Err("The matching DAT has no unambiguous strong platform identity. Review the catalogue source.".into()),
        }
    }
    if platforms.len() != 1 {
        return Err("No unique whole-file SHA-1 and size match with strong DAT platform identity was found. Register a matching local optical-disc DAT, resolve source conflicts, then check again. File presence and library labels cannot authorize launch.".into());
    }
    let platform = platforms.into_iter().next().unwrap();
    if !matches!(
        platform.as_str(),
        "FM Towns" | "AmigaCD32" | "Commodore CDTV"
    ) {
        return Err(format!(
            "The matching catalogue identifies {platform}, outside the supported native CD targets."
        ));
    }
    // Check structural disagreement without trusting a caller's platform hint.
    let report = crate::selected_evidence_page::gather_selected_evidence_fast(path, None)?;
    use archivefs_core::platform_evidence_fusion::FusionOutcome;
    let content = &report.identity_result.content;
    if matches!(
        content.outcome,
        FusionOutcome::Conflict | FusionOutcome::Ambiguous
    ) || content.resolved_platform.is_some_and(|p| p != platform)
    {
        return Err("Content and catalogue platform evidence is conflicting or ambiguous. Resolve identity diagnostics before launch.".into());
    }
    let structural =
        archivefs_core::launch::canonical_identity_from_game_report(&report.game_identity_report).0;
    if !matches!(structural, CanonicalIdentityStatus::Unknown) {
        return Err("Structural identity already resolves another supported launch route. Use its existing launch review rather than overriding it with a CD profile.".into());
    }
    if archivefs_core::identity_source::hashing::FileFingerprint::observe(path).as_ref()
        != Some(&hashes.fingerprint)
    {
        return Err("Disc changed during catalogue review. Check again.".into());
    }
    Ok(CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: platform,
        game_key: hashes.sha1,
    }))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    pub(crate) fn dat(path: &Path, iso: &Path, name: &str) {
        let hashes = archivefs_core::identity_source::hashing::hash_file(
            iso,
            &archivefs_core::safe_read::TrustedRoots::none(),
            None,
        )
        .unwrap();
        std::fs::write(path,format!("<datafile><header><name>{name}</name><description>{name}</description></header><game name=\"synthetic\"><rom name=\"fixture.iso\" size=\"{}\" sha1=\"{}\"/></game></datafile>",hashes.bytes_hashed,hashes.sha1)).unwrap();
    }
    #[test]
    fn actual_dat_parser_and_hash_match_authorize_distinct_optical_platforms() {
        for (name, expected) in [
            ("Fujitsu - FM Towns", "FM Towns"),
            ("Commodore - Amiga CD32", "AmigaCD32"),
            ("Commodore - CDTV", "Commodore CDTV"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let iso = dir.path().join("unrelated filename.iso");
            std::fs::write(&iso, b"synthetic direct disc bytes").unwrap();
            let source = dir.path().join("local.dat");
            dat(&source, &iso, name);
            let identity = verify_from_paths(&iso, std::slice::from_ref(&source)).unwrap();
            assert!(
                matches!(identity,CanonicalIdentityStatus::Resolved(ref i) if i.platform_id==expected)
            );
            std::fs::write(&iso, b"replaced bytes").unwrap();
            assert!(verify_from_paths(&iso, &[source]).is_err());
        }
    }
    #[test]
    fn conflicting_catalogues_missing_sources_and_no_hash_match_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let iso = dir.path().join("FM Towns.iso");
        std::fs::write(&iso, b"synthetic").unwrap();
        assert!(verify_from_paths(&iso, &[]).is_err());
        let a = dir.path().join("a.dat");
        let b = dir.path().join("b.dat");
        dat(&a, &iso, "Fujitsu - FM Towns");
        dat(&b, &iso, "Commodore - Amiga CD32");
        assert!(verify_from_paths(&iso, &[a.clone(), b]).is_err());
        assert!(verify_from_paths(&iso, &[a, dir.path().join("missing.dat")]).is_err());
    }
}
