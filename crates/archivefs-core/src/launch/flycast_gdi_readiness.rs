//! Selection-bound GDI launch freshness. Preview digests are NOT catalogue verification.
//! Native Linux contract: flyinghead/flycast 6bb0fc3e18dac4b01694f488c7a71f5930e8a648,
//! core/linux-dist/main.cpp (XDG roots), core/cfg/cl.cpp (positional GDI),
//! core/oslib/oslib.cpp (BiosPath before default data root).
use super::{flycast_execution::*, process_spawn::CapturedFileIdentity};
use crate::{
    ingestion::gdi,
    patch_manager::{
        FLYCAST_MAX_CONFIG_BYTES, FlycastInstallationType, FlycastProfileDiscoveryRoots,
        discover_flycast_profiles,
    },
};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    identity: CapturedFileIdentity,
    changed: (i64, i64),
}
impl Stamp {
    fn capture(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("An absolute path is required".into());
        }
        // Check original ancestors, not only a canonicalized track target.
        for ancestor in path.ancestors() {
            let m = std::fs::symlink_metadata(ancestor)
                .map_err(|e| format!("{}: {e}", ancestor.display()))?;
            if m.file_type().is_symlink() {
                return Err(format!("Symlink refused: {}", ancestor.display()));
            }
        }
        let m = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !m.is_file() {
            return Err(format!("Not a regular file: {}", path.display()));
        }
        #[cfg(unix)]
        let changed = {
            use std::os::unix::fs::MetadataExt;
            (m.ctime(), m.ctime_nsec())
        };
        #[cfg(not(unix))]
        let changed = (0, 0);
        Ok(Self {
            path: path.into(),
            identity: CapturedFileIdentity::capture(&m),
            changed,
        })
    }
}
// Reuse the existing bounded, nofollow, stable-open SHA-256 freshness proof.
// These comparisons bind bytes to Preview; they do not match a trusted DAT.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Proof {
    stamp: Stamp,
    content: crate::dat::rename_apply::model::ObjectIdentity,
}
impl Proof {
    #[cfg(target_os = "linux")]
    fn capture(path: &Path, budget: &mut u64) -> Result<Self, String> {
        let stamp = Stamp::capture(path)?;
        let content = crate::dat::rename_apply::identity::capture_identity_bounded(path, budget)
            .map_err(|e| e.to_string())?;
        if content.kind != crate::dat::rename_apply::model::ObjectKind::RegularFile
            || content.freshness.is_none()
            || content.size_bytes != stamp.identity.size
            || content.dev != stamp.identity.device
            || content.ino != stamp.identity.inode
            || content.freshness.as_ref().map(|f| f.modified) != stamp.identity.modified
            || Stamp::capture(path)? != stamp
        {
            return Err("File changed while binding Preview content".into());
        }
        Ok(Self { stamp, content })
    }
    #[cfg(not(target_os = "linux"))]
    fn capture(_path: &Path, _budget: &mut u64) -> Result<Self, String> {
        Err("Native Linux content proof is required".into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GdiStamp {
    descriptor: gdi::GdiDescriptor,
    files: Vec<Proof>,
}
impl GdiStamp {
    fn capture(path: &Path) -> Result<Self, String> {
        let mut descriptor_budget = 16384;
        let first = Proof::capture(path, &mut descriptor_budget)?;
        let mut budget =
            crate::dreamcast_extent_patch::MAX_SET_BYTES - first.stamp.identity.size - 1;
        if first.stamp.identity.size >= 16384 {
            return Err("Native Flycast requires a GDI descriptor smaller than 16384 bytes".into());
        }
        let text = String::from_utf8(read_bound(path, 16383, true)?).map_err(|e| e.to_string())?;
        let mut quoted = false;
        for c in text.chars() {
            if c == '"' {
                quoted = !quoted;
            }
            if !quoted && c.is_whitespace() && !c.is_ascii() {
                return Err("Native GDI field delimiters must use ASCII whitespace".into());
            }
        }
        let descriptor = gdi::parse_gdi_descriptor(path).map_err(|e| e.to_string())?;
        gdi::data_track_from_descriptor(&descriptor).map_err(|e| e.to_string())?;
        let tracks = &descriptor.tracks;
        if tracks.len() < 3
            || tracks
                .iter()
                .enumerate()
                .any(|(i, t)| t.number != i as u32 + 1)
            || tracks[0].track_type != gdi::GdiTrackType::Data
            || tracks[1].track_type != gdi::GdiTrackType::Audio
            || tracks[2].track_type != gdi::GdiTrackType::Data
            || tracks[2].start_lba != 45000
        {
            return Err("Native Flycast requires ordered GDI tracks beginning data/audio/data with track 3 at LBA 45000".into());
        }
        for (track, line) in tracks.iter().zip(
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .skip(1),
        ) {
            if track.source_filename.as_os_str().len() == 1 && !line.contains('"') {
                return Err(
                    "Native Flycast cannot parse an unquoted one-byte track filename".into(),
                );
            }
        }
        let mut files = vec![first.clone()];
        for track in &descriptor.tracks {
            let original = path
                .parent()
                .ok_or("No parent directory")?
                .join(&track.source_filename);
            files.push(Proof::capture(&original, &mut budget)?);
            if std::fs::canonicalize(&original).map_err(|e| e.to_string())? != track.source_path {
                return Err("GDI companion target changed".into());
            }
        }
        for proof in &files {
            if Stamp::capture(&proof.stamp.path)? != proof.stamp {
                return Err("GDI companion changed during inspection".into());
            }
        }
        Ok(Self { descriptor, files })
    }
}
#[derive(Clone, Debug)]
pub struct Preview {
    pub request: FlycastLaunchRequest,
    content: GdiStamp,
    profile_files: Vec<Proof>,
    observed: Instant,
}

fn native_files(
    request: &FlycastLaunchRequest,
    roots: &FlycastProfileDiscoveryRoots,
) -> Result<Vec<Proof>, String> {
    if !cfg!(target_os = "linux")
        || roots.appimage_directory.is_some()
        || std::env::var_os("APPIMAGE").is_some()
    {
        return Err(
            "Only native Linux XDG Flycast is supported; AppImage profiles are not bound".into(),
        );
    }
    // Discovery roots must describe the environment the existing spawner inherits.
    let actual = FlycastProfileDiscoveryRoots::from_environment().map_err(|e| e.to_string())?;
    if roots.home != actual.home
        || roots.xdg_config_home != actual.xdg_config_home
        || roots.xdg_data_home != actual.xdg_data_home
    {
        return Err("Flycast profile roots differ from the launch environment".into());
    }
    let discovery = discover_flycast_profiles(roots);
    let profile = discovery
        .profiles
        .iter()
        .find(|p| p.profile_id == request.profile_id)
        .ok_or("Selected Flycast profile is missing")?;
    if profile.installation_type != FlycastInstallationType::Native
        || profile.configuration_path != roots.xdg_config_home.join("flycast")
        || profile.system_path != roots.xdg_data_home.join("flycast")
    {
        return Err("Selected Flycast profile has no supported native XDG binding".into());
    }
    let header = read_bound(&request.expected_executable, 12, false)?;
    if header.len() < 12 {
        return Err("Flycast executable header is truncated".into());
    }
    if &header[..4] != b"\x7fELF" || &header[8..10] == b"AI" {
        return Err(
            "Only a native ELF Flycast executable is supported; wrappers and AppImages are refused"
                .into(),
        );
    }
    let paths = vec![
        profile.config_path.clone(),
        request.expected_executable.clone(),
        profile.system_path.join("dc_boot.bin"),
    ];
    let stamp = Stamp::capture(&profile.config_path)?;
    if stamp.identity.size > FLYCAST_MAX_CONFIG_BYTES {
        return Err("Flycast configuration exceeds inspection limit".into());
    }
    let text = String::from_utf8(read_bound(
        &profile.config_path,
        FLYCAST_MAX_CONFIG_BYTES,
        true,
    )?)
    .map_err(|e| e.to_string())?;
    check_config_contract(&text)?;
    if Stamp::capture(&profile.config_path)? != stamp {
        return Err("Flycast settings changed during inspection".into());
    }

    let mut budget = 256 * 1024 * 1024;
    let proofs: Vec<_> = paths
        .iter()
        .map(|p| Proof::capture(p, &mut budget))
        .collect::<Result<_, _>>()?;
    for proof in &proofs {
        if Stamp::capture(&proof.stamp.path)? != proof.stamp {
            return Err("Flycast profile changed during content inspection".into());
        }
    }
    Ok(proofs)
}
fn read_bound(path: &Path, limit: u64, complete: bool) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let before = Stamp::capture(path)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() || CapturedFileIdentity::capture(&opened) != before.identity {
        return Err("File changed while opening".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + u64::from(complete))
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if (complete && bytes.len() as u64 > limit) || Stamp::capture(path)? != before {
        return Err("File changed or exceeded inspection limit".into());
    }
    Ok(bytes)
}

fn check_config_contract(text: &str) -> Result<(), String> {
    if text.contains('\0') {
        return Err("NUL in Flycast configuration".into());
    }
    for (key, value) in text.split('\n').filter_map(|l| l.split_once('=')) {
        let key = key.trim().to_ascii_lowercase();
        if (key == "biospath" || key.ends_with(".biospath"))
            && !matches!(value, "" | " " | "\"\"" | " \"\"")
        {
            return Err("Configured Dreamcast.BiosPath requires an explicit runtime binding; launch refused".into());
        }
        if key == "usereios" || key.ends_with(".usereios") {
            if !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "false" | "no" | "0"
            ) {
                return Err(
                    "HLE BIOS selection is unsupported by the verified native BIOS launch path"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

impl Preview {
    /// Read-only, run in a worker. Calls the existing identity/profile/firmware planner.
    pub fn inspect(
        request: FlycastLaunchRequest,
        roots: &FlycastProfileDiscoveryRoots,
    ) -> Result<Self, String> {
        if !request
            .selected_content_path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("gdi"))
        {
            return Err("This action supports direct GDI only".into());
        }
        let profile_files = native_files(&request, roots)?;
        let content = GdiStamp::capture(&request.selected_content_path)?;
        preflight_flycast_launch(&request, roots)
            .map_err(|e| format!("{:?}: {}", e.kind, e.detail))?;
        let mut result = Self {
            request,
            content,
            profile_files,
            observed: Instant::now(),
        };
        result.verify_current(roots)?;
        result.observed = Instant::now();
        Ok(result)
    }
    pub fn expired(&self) -> bool {
        self.observed.elapsed() > std::time::Duration::from_secs(30)
    }
    pub fn recheck(&self, roots: &FlycastProfileDiscoveryRoots) -> Result<(), String> {
        if self.expired() {
            return Err("GDI preview expired; refresh Dreamcast readiness".into());
        }
        self.verify_current(roots)?;
        if self.expired() {
            return Err(
                "GDI content recheck exceeded the preview lifetime; refresh required".into(),
            );
        }
        Ok(())
    }
    fn verify_current(&self, roots: &FlycastProfileDiscoveryRoots) -> Result<(), String> {
        if GdiStamp::capture(&self.request.selected_content_path)? != self.content
            || native_files(&self.request, roots)? != self.profile_files
        {
            return Err(
                "GDI companions or Flycast profile changed; refresh Dreamcast readiness".into(),
            );
        }
        Ok(())
    }
    /// The existing executor owns process creation; no new argv or game-media writes.
    pub fn launch(
        &self,
        roots: &FlycastProfileDiscoveryRoots,
    ) -> Result<LaunchedFlycastProcess, String> {
        self.recheck(roots)?;
        let command = preflight_flycast_launch(&self.request, roots)
            .map_err(|e| format!("{:?}: {}", e.kind, e.detail))?;
        self.recheck(roots)?;
        spawn_flycast(command).map_err(|e| format!("{e:?}"))
    }
}
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("disc.gdi");
        std::fs::write(d.path().join("low.bin"), vec![0; 2048]).unwrap();
        std::fs::write(d.path().join("audio.raw"), vec![0; 2352]).unwrap();
        std::fs::write(d.path().join("game.bin"), vec![0; 2048]).unwrap();
        std::fs::write(
            &p,
            "3\n1 0 4 2048 low.bin 0\n2 600 0 2352 audio.raw 0\n3 45000 4 2048 game.bin 0\n",
        )
        .unwrap();
        (d, p)
    }
    #[test]
    fn every_track_participates_in_freshness() {
        for track in ["low.bin", "audio.raw", "game.bin"] {
            let (d, p) = fixture();
            let before = GdiStamp::capture(&p).unwrap();
            let path = d.path().join(track);
            let bytes = std::fs::read(&path).unwrap();
            let replacement = d.path().join("replacement");
            std::fs::write(&replacement, vec![1; bytes.len()]).unwrap();
            std::fs::rename(replacement, &path).unwrap();
            assert_ne!(before, GdiStamp::capture(&p).unwrap());
        }
    }
    #[test]
    fn missing_and_ambiguous_tracks_are_refused() {
        let (d, p) = fixture();
        std::fs::remove_file(d.path().join("audio.raw")).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
        let (_d, p) = fixture();
        let text = std::fs::read_to_string(&p)
            .unwrap()
            .replace("2 600 0 2352 audio.raw", "2 45001 4 2352 audio.raw");
        std::fs::write(&p, text).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn contained_symlink_is_refused() {
        let (d, p) = fixture();
        std::fs::rename(d.path().join("audio.raw"), d.path().join("real.raw")).unwrap();
        std::os::unix::fs::symlink("real.raw", d.path().join("audio.raw")).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn same_inode_payload_edit_with_restored_mtime_is_stale() {
        let (d, p) = fixture();
        let before = GdiStamp::capture(&p).unwrap();
        let path = d.path().join("audio.raw");
        let old = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, vec![9; 2352]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        assert_ne!(before, GdiStamp::capture(&p).unwrap());
    }
    #[test]
    fn actual_native_and_per_game_bios_overrides_are_refused() {
        for text in [
            "[config]\nDreamcast.BiosPath=/other",
            "[T-8109N]\nconfig.Dreamcast.BiosPath=/other",
            "[config]\nUseReios=yes",
            "[T-8109N]\nconfig.UseReios=1",
        ] {
            assert!(check_config_contract(text).is_err(), "{text}");
        }
        assert!(check_config_contract("[config]\nUseReios=no\n").is_ok());
    }
    #[test]
    fn configuration_reads_are_bounded() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("emu.cfg");
        std::fs::write(&p, vec![b'x'; 257]).unwrap();
        assert!(read_bound(&p, 256, true).is_err());
    }
    #[test]
    fn expired_preview_refuses_before_using_any_profile() {
        let (_d, p) = fixture();
        let content = GdiStamp::capture(&p).unwrap();
        let roots = FlycastProfileDiscoveryRoots::from_environment().unwrap();
        let preview = Preview {
            request: FlycastLaunchRequest {
                selected_content_path: p,
                expected_platform_id: "Dreamcast".into(),
                expected_game_key: "T-8109N".into(),
                expected_dreamcast_product_code: "T-8109N".into(),
                profile_id: "missing".into(),
                expected_executable: "/missing".into(),
            },
            content,
            profile_files: vec![],
            observed: Instant::now() - std::time::Duration::from_secs(31),
        };
        assert!(preview.recheck(&roots).unwrap_err().contains("expired"));
    }
    #[test]
    fn native_layout_requires_three_tracks() {
        let (_d, p) = fixture();
        std::fs::write(&p, "1\n1 45000 4 2048 game.bin 0\n").unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn native_layout_requires_declaration_order() {
        let (_d, p) = fixture();
        let text = std::fs::read_to_string(&p).unwrap();
        let mut rows: Vec<_> = text.lines().collect();
        rows.swap(1, 2);
        std::fs::write(&p, rows.join("\n")).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn native_layout_requires_exact_high_density_lba() {
        let (_d, p) = fixture();
        let text = std::fs::read_to_string(&p)
            .unwrap()
            .replace("45000", "45001");
        std::fs::write(&p, text).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn native_layout_requires_expected_track_prefix() {
        let (_d, p) = fixture();
        let text = std::fs::read_to_string(&p)
            .unwrap()
            .replace("2 600 0", "2 600 4");
        std::fs::write(&p, text).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn native_layout_refuses_native_descriptor_size_limit() {
        let (_d, p) = fixture();
        let text = std::fs::read_to_string(&p).unwrap() + &"\n".repeat(16384);
        std::fs::write(&p, text).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn native_layout_refuses_unquoted_one_byte_filename() {
        let (d, p) = fixture();
        std::fs::rename(d.path().join("audio.raw"), d.path().join("a")).unwrap();
        let text = std::fs::read_to_string(&p)
            .unwrap()
            .replace("audio.raw", "a");
        std::fs::write(&p, text).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn native_layout_refuses_unicode_delimiters() {
        let (_d, p) = fixture();
        let text = std::fs::read_to_string(&p)
            .unwrap()
            .replace("3 45000", "3\u{a0}45000");
        std::fs::write(&p, text).unwrap();
        assert!(GdiStamp::capture(&p).is_err());
    }
    #[test]
    fn quoted_short_and_unicode_names_remain_supported() {
        for name in ["a", "音 track.raw", "track\u{a0}name.raw"] {
            let (d, p) = fixture();
            std::fs::rename(d.path().join("audio.raw"), d.path().join(name)).unwrap();
            let text = std::fs::read_to_string(&p)
                .unwrap()
                .replace("audio.raw", &format!("\"{name}\""));
            std::fs::write(&p, text).unwrap();
            assert!(GdiStamp::capture(&p).is_ok());
        }
    }
    #[test]
    fn exactly_empty_default_bios_path_is_supported() {
        for value in ["", " ", "\"\"", " \"\""] {
            assert!(
                check_config_contract(&format!("[config]\nDreamcast.BiosPath={value}\n")).is_ok()
            );
        }
        for value in ["  ", "\t", "\r", "\" \""] {
            assert!(
                check_config_contract(&format!("[config]\nDreamcast.BiosPath={value}\n")).is_err()
            );
        }
    }
    #[test]
    fn freshness_hashing_refuses_exhausted_byte_budget() {
        let (d, _) = fixture();
        let mut budget = 2048;
        assert!(Proof::capture(&d.path().join("game.bin"), &mut budget).is_err());
    }
}
