//! Read-only side-by-side review. A review is never an installation permit.
//! Reuses the release resolver's plan and the manifest layout; no publisher is called.
use super::*;
use crate::emulator_download::{
    MAX_DOWNLOAD_BYTES, MIN_APPIMAGE_BYTES, ReleaseAsset, select_x86_64_asset,
};
use std::io;

pub const PUBLICATION_BLOCKER: &str = "Publication is unavailable: the historical installer switches the current executable, stages outside the destination filesystem, and has no reviewed approval/journal/Undo contract for newly installed files. A reviewed transaction with destination-adjacent staging and immediate input revalidation is required.";
pub const COMPATIBILITY: &str = "Linux x86_64; AppImage type 2 runtime/FUSE support must be checked separately. No executable is run during review. Configuration and launch selections are preserved.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppImageInstallReview {
    pub plan: EmulatorDownloadPlan,
    pub proposed_destination: Option<PathBuf>,
    pub staging_parent: Option<PathBuf>,
    pub current_installations: Vec<String>,
    pub current_installations_inspected: bool,
    pub artifact: Option<PathBuf>,
    pub artifact_sha256: Option<String>,
    pub artifact_size: Option<u64>,
    pub available_bytes: Option<u64>,
    pub required_bytes: Option<u64>,
    pub refusals: Vec<String>,
    pub pending: Vec<String>,
}
impl AppImageInstallReview {
    /// Even fully verified review inputs cannot authorize the historical writer.
    pub fn publication_enabled(&self) -> bool {
        false
    }
}

/// Inspect existing local inputs only. No creation, chmod, download, execution,
/// selection change, publication or cleanup. Metadata is evidence of the curated
/// source only when obtained through the existing authenticated HTTPS resolver.
pub fn review_appimage_install(
    root: &Path,
    plan: &EmulatorDownloadPlan,
    artifact: Option<&Path>,
) -> AppImageInstallReview {
    let mut review = AppImageInstallReview {
        plan: plan.clone(),
        proposed_destination: None,
        staging_parent: None,
        current_installations: Vec::new(),
        current_installations_inspected: false,
        artifact: artifact.map(Path::to_path_buf),
        artifact_sha256: None,
        artifact_size: None,
        available_bytes: None,
        required_bytes: None,
        refusals: Vec::new(),
        pending: vec![PUBLICATION_BLOCKER.into()],
    };
    let Some(spec) = emulator_download_spec(&plan.emulator_id)
        .filter(|s| s.distribution == EmulatorDistribution::GithubAppImage)
    else {
        review
            .refusals
            .push("No curated AppImage source exists for this emulator.".into());
        return review;
    };
    let expected = match review_metadata(plan, spec) {
        Ok(digest) => digest,
        Err(error) => {
            review.refusals.push(error);
            return review;
        }
    };
    let installs = root.join("emulators").join(spec.id).join("installs");
    // A proposed, unreserved ID in the existing manifest layout. A future
    // transaction must reserve/revalidate its own ID before publication.
    let destination = installs
        .join(format!("install-{}", &expected[..24]))
        .join(spec.installed_binary);
    review.proposed_destination = Some(destination.clone());
    review.staging_parent = Some(installs.clone());
    match inspect_destination(root, &installs, &destination) {
        Ok((ancestor, available)) => {
            review.available_bytes = Some(available);
            if ancestor != installs {
                review.pending.push(format!("Destination directories do not exist yet; ownership and space must be rechecked after approved creation beneath {}.", ancestor.display()));
            }
            // Only list paths and pointer targets; do not hash or follow an
            // existing installation just to present what will be preserved.
            review.current_installations_inspected = true;
            let emulator_root = root.join("emulators").join(spec.id);
            let receipt = emulator_root.join("install.json");
            if fs::symlink_metadata(&receipt).is_ok() {
                match read_json_evidence(&receipt) {
                    Ok(value) => review.current_installations.push(format!(
                        "{} — legacy download receipt reports version {} and SHA-256 {} (record only; existing bytes not reverified or adopted)", receipt.display(),
                        value.get("version").and_then(|v|v.as_str()).unwrap_or("unknown"),
                        value.get("sha256").and_then(|v|v.as_str()).unwrap_or("missing")
                    )),
                    Err(error) => review.pending.push(format!("Existing download receipt could not be read safely: {error}")),
                }
            }
            for path in [
                emulator_root.join(spec.installed_binary),
                emulator_root.join("current"),
            ] {
                match fs::symlink_metadata(&path) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        review.current_installations.push(format!(
                            "{} → {} (existing pointer; preserved, target not opened)",
                            path.display(),
                            fs::read_link(&path)
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|_| "unreadable".into())
                        ));
                    }
                    Ok(_) => review.current_installations.push(format!(
                        "{} (existing installation; preserved, ownership not inferred)",
                        path.display()
                    )),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => review.refusals.push(format!(
                        "Cannot inspect existing installation {}: {e}",
                        path.display()
                    )),
                }
            }
            if installs.exists() {
                match fs::read_dir(&installs) {
                    Ok(entries) => {
                        for (index, entry) in entries.enumerate() {
                            if index >= 128 {
                                review.pending.push("More than 128 manifest installation entries; use the installation inventory for complete history.".into());
                                break;
                            }
                            match entry {
                                Ok(entry) => review
                                    .current_installations
                                    .push(manifest_summary(&entry, spec)),
                                Err(error) => review
                                    .refusals
                                    .push(format!("Cannot list installation history: {error}")),
                            }
                        }
                    }
                    Err(e) => review
                        .refusals
                        .push(format!("Cannot inspect installation history: {e}")),
                }
            }
        }
        Err(error) => review.refusals.push(error),
    }
    match artifact {
        None => review.pending.push("Choose an already downloaded local AppImage to verify its bytes against this release's published SHA-256. No artifact has been downloaded or verified.".into()),
        Some(path) => match verify_artifact(path, &expected) {
            Ok(size) => {
                review.artifact_sha256 = Some(expected);
                review.artifact_size = Some(size);
                let required = size.saturating_mul(2).saturating_add(1024 * 1024);
                review.required_bytes = Some(required);
                if review.available_bytes.is_some_and(|free| free < required) { review.refusals.push(format!("Insufficient destination space: require at least {required} bytes for staging and publication.")); }
            }
            Err(error) => review.refusals.push(error),
        }
    }
    review.pending.push("Compatibility is not established by a version string: Linux x86_64 and AppImage runtime/FUSE requirements still need confirmation before any future installation.".into());
    review
}

fn normalized_digest(value: &str) -> Result<String, String> {
    let value = value
        .strip_prefix("sha256:")
        .or_else(|| value.strip_prefix("sha256="))
        .unwrap_or(value)
        .trim();
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Published SHA-256 is malformed.".into());
    }
    Ok(value.to_ascii_lowercase())
}
fn review_metadata(
    plan: &EmulatorDownloadPlan,
    spec: &crate::emulator_download::EmulatorDownloadSpec,
) -> Result<String, String> {
    if plan.distribution != spec.distribution
        || plan.project_url != spec.project_url
        || plan.official_project != spec.official_project
        || plan.release_tag.trim().is_empty()
    {
        return Err("Source identity does not match the curated publisher catalogue.".into());
    }
    let asset = ReleaseAsset {
        name: plan.asset_name.clone(),
        download_url: plan.asset_url.clone(),
        digest: plan.upstream_digest.clone(),
    };
    select_x86_64_asset(spec, &[asset]).map_err(|e| format!("Release asset refused: {e}"))?;
    // Require exact release/asset binding in addition to the resolver's source allowlist.
    let prefix = format!(
        "{}/releases/download/{}/",
        spec.project_url, plan.release_tag
    );
    if !plan.asset_url.starts_with(&prefix)
        || plan.asset_url.rsplit('/').next() != Some(plan.asset_name.as_str())
    {
        return Err("Asset URL is not bound to the exact declared release and filename.".into());
    }
    let expected = normalized_digest(
        plan.expected_sha256
            .as_deref()
            .ok_or("Publisher did not provide a SHA-256; installation is refused.")?,
    )?;
    if normalized_digest(
        plan.upstream_digest
            .as_deref()
            .ok_or("Published digest evidence is absent.")?,
    )? != expected
    {
        return Err("Published and expected SHA-256 evidence conflict.".into());
    }
    Ok(expected)
}

fn manifest_summary(
    entry: &fs::DirEntry,
    spec: &crate::emulator_download::EmulatorDownloadSpec,
) -> String {
    let summary = match entry.file_type() {
        Ok(kind) if kind.is_dir() => match read_json_evidence(&entry.path().join("manifest.json"))
            .and_then(|value| {
                serde_json::from_value::<ManagedInstallManifest>(value).map_err(|e| e.to_string())
            }) {
            Ok(manifest)
                if manifest.emulator_id == spec.id
                    && manifest.manifest_schema_version
                        == MANAGED_INSTALL_MANIFEST_SCHEMA_VERSION
                    && manifest.install_root == entry.path()
                    && manifest.executable_path == entry.path().join(spec.installed_binary) =>
            {
                format!(
                    "manifest reports version {}, SHA-256 {}, ownership {:?}; existing bytes not reverified or adopted",
                    manifest.installed_version,
                    manifest.installed_executable_sha256,
                    manifest.ownership_state
                )
            }
            Ok(_) => "manifest identity/layout mismatch; preserved, not trusted".into(),
            Err(error) => format!("manifest unreadable/invalid: {error}; preserved, not trusted"),
        },
        _ => "entry is not a direct installation directory; not followed".into(),
    };
    format!("{} ({summary})", entry.path().display())
}

/// Bounded informational evidence only. No record confers ownership or authority.
#[cfg(unix)]
fn read_json_evidence(path: &Path) -> Result<serde_json::Value, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MANAGED_INSTALL_MAX_MANIFEST_BYTES {
        return Err("Evidence must be a regular file within the manifest size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MANAGED_INSTALL_MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MANAGED_INSTALL_MAX_MANIFEST_BYTES {
        return Err("Evidence exceeds manifest size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}
#[cfg(not(unix))]
fn read_json_evidence(_: &Path) -> Result<serde_json::Value, String> {
    Err("Safe local evidence inspection requires Unix".into())
}

#[cfg(unix)]
fn inspect_destination(
    root: &Path,
    parent: &Path,
    destination: &Path,
) -> Result<(PathBuf, u64), String> {
    use std::os::unix::fs::MetadataExt;
    if !root.is_absolute()
        || root
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("Destination root must be absolute without traversal.".into());
    }
    let mut path = PathBuf::new();
    let mut ancestor = PathBuf::new();
    let mut root_device = None;
    let uid = unsafe { libc::geteuid() };
    for component in parent.components() {
        path.push(component.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err(format!(
                        "Destination ancestor is a symlink or not a directory: {}",
                        path.display()
                    ));
                }
                // A private root below an attacker-writable ancestor is not
                // stable. Root-owned sticky shared roots (e.g. /tmp) protect
                // user-owned children; other shared/unowned ancestors refuse.
                if !path.starts_with(root)
                    && ((meta.uid() != uid && meta.uid() != 0)
                        || (meta.mode() & 0o022 != 0
                            && !(meta.uid() == 0 && meta.mode() & 0o1000 != 0)))
                {
                    return Err(format!(
                        "Destination ancestor ownership or shared write permissions are unsafe: {}",
                        path.display()
                    ));
                }
                // The chosen root and its descendants must be privately owned.
                if path.starts_with(root) {
                    if meta.uid() != uid || meta.mode() & 0o022 != 0 {
                        return Err(format!(
                            "Destination must be owned by this user and not writable by other users: {}",
                            path.display()
                        ));
                    }
                    if let Some(device) = root_device {
                        if meta.dev() != device {
                            return Err(format!(
                                "Destination crosses a filesystem boundary: {}",
                                path.display()
                            ));
                        }
                    } else {
                        root_device = Some(meta.dev());
                    }
                }
                ancestor = path.clone();
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(format!(
                    "Cannot inspect destination {}: {e}",
                    path.display()
                ));
            }
        }
    }
    if ancestor.as_os_str().is_empty() {
        return Err("No inspectable destination ancestor.".into());
    }
    // Any existing object, including a dangling symlink, is a collision.
    match fs::symlink_metadata(destination.parent().unwrap()) {
        Ok(_) => {
            return Err(format!(
                "Proposed installation ID already exists: {}. Choose a fresh reviewed ID; nothing will be overwritten.",
                destination.parent().unwrap().display()
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Cannot check destination collision: {e}")),
    }
    let cpath = std::ffi::CString::new(ancestor.as_os_str().as_encoded_bytes())
        .map_err(|_| "Invalid destination path")?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(cpath.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(format!(
            "Cannot determine available space: {}",
            io::Error::last_os_error()
        ));
    }
    let stats = unsafe { stats.assume_init() };
    if stats.f_flag & libc::ST_RDONLY != 0 {
        return Err("Destination filesystem is read-only.".into());
    }
    Ok((
        ancestor,
        (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64),
    ))
}
#[cfg(not(unix))]
fn inspect_destination(_: &Path, _: &Path, _: &Path) -> Result<(PathBuf, u64), String> {
    Err("Side-by-side AppImages require Linux/Unix filesystem semantics.".into())
}

#[cfg(unix)]
fn verify_artifact(path: &Path, expected: &str) -> Result<u64, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // Refuse known special objects before open; fd metadata below repeats this
    // check to handle replacement between inspection and open.
    let inspected = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !inspected.is_file() || inspected.file_type().is_symlink() {
        return Err(
            "Artifact must be a direct regular file, not a symlink or special device.".into(),
        );
    }
    // No execution, no following the final symlink, no blocking on FIFO/device.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("Cannot safely open artifact: {e}"))?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    if !before.is_file()
        || before.len() < MIN_APPIMAGE_BYTES as u64
        || before.len() > MAX_DOWNLOAD_BYTES as u64
    {
        return Err(
            "Local artifact must be a regular AppImage within the existing size limits.".into(),
        );
    }
    let mut header = [0u8; 64];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || &header[8..11] != b"AI\x02"
        || u16::from_le_bytes([header[18], header[19]]) != 62
    {
        return Err(
            "Local artifact is not a Linux x86_64 type 2 AppImage; compatibility is refused."
                .into(),
        );
    }
    let mut hash = Sha256::new();
    hash.update(header);
    let mut size = 64u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        size += count as u64;
        if size > MAX_DOWNLOAD_BYTES as u64 {
            return Err(
                "Artifact grew beyond the maximum download size during verification.".into(),
            );
        }
        hash.update(&buffer[..count]);
    }
    let after = file.metadata().map_err(|e| e.to_string())?;
    let now = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if identity(&before) != identity(&after)
        || identity(&after) != identity(&now)
        || now.file_type().is_symlink()
        || size != after.len()
    {
        return Err("Artifact changed during verification; select and review it again.".into());
    }
    let digest = hash
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if digest != expected {
        return Err(
            "Local artifact SHA-256 does not match the exact curated release evidence.".into(),
        );
    }
    Ok(size)
}
#[cfg(not(unix))]
fn verify_artifact(_: &Path, _: &str) -> Result<u64, String> {
    Err("Local artifact verification requires Unix no-follow file handling.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, EmulatorDownloadPlan, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let artifact = dir.path().join("download.AppImage");
        let mut bytes = vec![0u8; MIN_APPIMAGE_BYTES];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[8..11].copy_from_slice(b"AI\x02");
        bytes[18] = 62;
        fs::write(&artifact, &bytes).unwrap();
        let digest = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let spec = emulator_download_spec("pcsx2").unwrap();
        let plan = EmulatorDownloadPlan {
            emulator_id: spec.id.into(),
            display_name: spec.display_name.into(),
            distribution: spec.distribution,
            official_project: spec.official_project.into(),
            project_url: spec.project_url.into(),
            release_tag: "v1".into(),
            release_name: None,
            asset_name: "pcsx2-x86_64.AppImage".into(),
            asset_url: "https://github.com/PCSX2/pcsx2/releases/download/v1/pcsx2-x86_64.AppImage"
                .into(),
            expected_sha256: Some(digest.clone()),
            upstream_digest: Some(format!("sha256:{digest}")),
            destination_path: dir.path().join("untrusted-plan-destination"),
            selection_policy: Default::default(),
            replaces_existing_managed: false,
        };
        (dir, plan, artifact)
    }
    #[cfg(unix)]
    fn private_test_directories(fixture: &Path, path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut current = fixture.to_path_buf();
        for component in path.strip_prefix(fixture).unwrap().components() {
            current.push(component.as_os_str());
            match fs::create_dir(&current) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("{error}"),
            }
            fs::set_permissions(&current, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    #[test]
    fn local_artifact_review_is_useful_and_does_not_create_installation_or_approval() {
        let (dir, plan, artifact) = fixture();
        let root = dir.path().join("data");
        let review = review_appimage_install(&root, &plan, Some(&artifact));
        assert!(review.refusals.is_empty(), "{:?}", review.refusals);
        assert_eq!(review.artifact_sha256, plan.expected_sha256);
        assert_eq!(review.artifact_size, Some(MIN_APPIMAGE_BYTES as u64));
        assert!(review.available_bytes.is_some());
        assert!(review.required_bytes.is_some());
        assert!(
            review
                .staging_parent
                .as_ref()
                .unwrap()
                .ends_with("emulators/pcsx2/installs")
        );
        assert!(
            review
                .proposed_destination
                .as_ref()
                .unwrap()
                .starts_with(&root)
        );
        assert!(!review.publication_enabled());
        assert!(!root.exists());
        assert!(artifact.exists());
    }
    #[test]
    fn source_digest_and_release_substitutions_refuse_without_reading_artifact() {
        let (dir, plan, artifact) = fixture();
        for mutate in [
            |p: &mut EmulatorDownloadPlan| p.project_url = "https://evil.invalid".into(),
            |p: &mut EmulatorDownloadPlan| p.official_project = "another publisher".into(),
            |p: &mut EmulatorDownloadPlan| p.upstream_digest = None,
            |p: &mut EmulatorDownloadPlan| p.expected_sha256 = Some("0".repeat(64)),
            |p: &mut EmulatorDownloadPlan| p.release_tag = "v2".into(),
            |p: &mut EmulatorDownloadPlan| p.asset_name = "pcsx2-arm64.AppImage".into(),
            |p: &mut EmulatorDownloadPlan| p.emulator_id = "dolphin".into(),
        ] {
            let mut changed = plan.clone();
            mutate(&mut changed);
            let review = review_appimage_install(dir.path(), &changed, Some(&artifact));
            assert!(!review.refusals.is_empty());
            assert!(review.artifact_sha256.is_none());
            assert!(!review.publication_enabled());
        }
    }
    #[test]
    fn missing_artifact_remains_held_and_corrupt_artifact_is_refused() {
        let (dir, plan, artifact) = fixture();
        let review = review_appimage_install(dir.path(), &plan, None);
        assert!(review.artifact_sha256.is_none());
        assert!(review.pending.iter().any(|s| s.contains("No artifact")));
        let mut bytes = fs::read(&artifact).unwrap();
        bytes[100] = 1;
        fs::write(&artifact, bytes).unwrap();
        let review = review_appimage_install(dir.path(), &plan, Some(&artifact));
        assert!(review.refusals.iter().any(|s| s.contains("SHA-256")));
        assert!(review.artifact_sha256.is_none());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_and_fifo_artifacts_are_refused_without_execution_or_blocking() {
        let (dir, plan, artifact) = fixture();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&artifact, &link).unwrap();
        assert!(verify_artifact(&link, plan.expected_sha256.as_deref().unwrap()).is_err());
        let fifo = dir.path().join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(verify_artifact(&fifo, plan.expected_sha256.as_deref().unwrap()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn destination_symlink_shared_permissions_and_existing_install_id_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, plan, _) = fixture();
        let root = dir.path().join("data");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            review_appimage_install(&root, &plan, None)
                .refusals
                .iter()
                .any(|s| s.contains("ancestor ownership"))
        );
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("emulators")).unwrap();
        assert!(
            review_appimage_install(&root, &plan, None)
                .refusals
                .iter()
                .any(|s| s.contains("symlink"))
        );
        fs::remove_file(root.join("emulators")).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            review_appimage_install(&root, &plan, None)
                .refusals
                .iter()
                .any(|s| s.contains("owned"))
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let review = review_appimage_install(&root, &plan, None);
        let install = review
            .proposed_destination
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        private_test_directories(dir.path(), &install);
        let saved = install.join("preserved.cfg");
        fs::write(&saved, b"config").unwrap();
        let review = review_appimage_install(&root, &plan, None);
        assert!(review.refusals.iter().any(|s| s.contains("already exists")));
        assert_eq!(fs::read(saved).unwrap(), b"config");
    }
    #[cfg(unix)]
    #[test]
    fn existing_binary_and_current_pointer_are_displayed_and_preserved() {
        let (dir, plan, _) = fixture();
        let root = dir.path().join("data");
        let emulator = root.join("emulators/pcsx2");
        private_test_directories(dir.path(), &emulator);
        fs::write(emulator.join("pcsx2.AppImage"), b"previous").unwrap();
        std::os::unix::fs::symlink("installs/previous", emulator.join("current")).unwrap();
        let review = review_appimage_install(&root, &plan, None);
        assert_eq!(review.current_installations.len(), 2);
        assert!(review.refusals.is_empty());
        assert_eq!(
            fs::read(emulator.join("pcsx2.AppImage")).unwrap(),
            b"previous"
        );
        assert_eq!(
            fs::read_link(emulator.join("current")).unwrap(),
            Path::new("installs/previous")
        );
        assert!(!emulator.join("installs").exists());
    }
    #[test]
    fn elf_magic_alone_and_wrong_architecture_are_not_appimage_verification() {
        let (dir, plan, artifact) = fixture();
        let mut bytes = fs::read(&artifact).unwrap();
        for index in [8, 18] {
            let saved = bytes[index];
            bytes[index] = 0;
            fs::write(&artifact, &bytes).unwrap();
            assert!(
                review_appimage_install(dir.path(), &plan, Some(&artifact))
                    .refusals
                    .iter()
                    .any(|s| s.contains("type 2"))
            );
            bytes[index] = saved;
        }
    }
}
