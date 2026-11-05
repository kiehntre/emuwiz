//! Launch-only identity for loose Nintendo 3DS `.3dsx` homebrew, and the
//! fresh-recheck launch path that feeds Azahar's existing preflight.
//!
//! This is deliberately *not* catalogue or DAT verification. A `.3dsx` has no
//! title ID and no catalogue of record, so nothing here says "this is game X".
//! What it establishes is narrower and bound to the exact bytes:
//!
//! - the path is a regular, non-symlink file with a `.3dsx` extension;
//! - the bytes carry a structurally valid 3DSX header (magic, header and
//!   relocation-header sizes, format version, and segment/SMDH extents that
//!   fit inside the file);
//! - the SHA-256 and filesystem identity of those bytes.
//!
//! The resulting [`CanonicalIdentityStatus`] uses a namespaced game key
//! (`3dsx-sha256:<hex>`) so it can never be confused with, or collide with, a
//! catalogue-verified Nintendo 3DS key. It exists only to satisfy the launch
//! planner's contract; callers must never persist it as a library identity or
//! show it as "Verified". The SMDH title is display-only.
//!
//! Anything else (retail `.3ds`/`.cci`/`.cia`, files whose extension is
//! `.3dsx` but whose bytes are not a 3DSX, oversized files) is refused.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::launch::azahar_command::{AZAHAR_SUPPORTED_PLATFORM_ID, AzaharLaunchRequest};
use crate::launch::azahar_execution::{
    AzaharPreflightError, LaunchedAzaharProcess, preflight_azahar_launch, spawn_azahar,
};
use crate::launch::integration::azahar_profile_eligible;
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    ResolvedIdentity,
};
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::patch_manager::{
    AzaharContentForm, AzaharDiscoveryRoots, AzaharEvidenceState, AzaharTitleIdentity,
    discover_azahar_executable, discover_azahar_profile, inspect_azahar_3dsx,
};

/// Namespace of the launch-only game key. Catalogue keys never use it.
pub const AZAHAR_HOMEBREW_KEY_PREFIX: &str = "3dsx-sha256:";
/// Homebrew is small; anything larger is refused rather than hashed.
pub const AZAHAR_HOMEBREW_MAX_BYTES: u64 = 128 * 1024 * 1024;

const BASE_HEADER: u16 = 0x20;
const EXTENDED_HEADER: u16 = 0x2C;
const RELOC_HEADER: u16 = 0x0C;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreeDsxRefusalKind {
    NotThreeDsxExtension,
    Unreadable,
    NotRegularFile,
    TooLarge,
    InvalidHeader,
    ChangedWhileReading,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreeDsxRefusal {
    pub kind: ThreeDsxRefusalKind,
    pub detail: String,
}

fn refuse(kind: ThreeDsxRefusalKind, detail: impl Into<String>) -> ThreeDsxRefusal {
    ThreeDsxRefusal {
        kind,
        detail: detail.into(),
    }
}

/// Evidence from one inspection of one `.3dsx` file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreeDsxEvidence {
    pub path: PathBuf,
    pub file_identity: CapturedFileIdentity,
    pub sha256: String,
    pub size: u64,
    /// Display-only embedded SMDH title, never part of the identity.
    pub display_title: Option<String>,
}

impl ThreeDsxEvidence {
    /// Launch-only identity. Namespaced so it is never a catalogue key.
    pub fn identity_status(&self) -> CanonicalIdentityStatus {
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: AZAHAR_SUPPORTED_PLATFORM_ID.into(),
            game_key: format!("{AZAHAR_HOMEBREW_KEY_PREFIX}{}", self.sha256),
        })
    }

    pub fn content_ref(&self) -> LaunchContentRef {
        LaunchContentRef {
            kind: Some(LaunchContentKind::Executable),
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(self.path.clone()),
            requires_mount: false,
            provenance:
                "structurally valid 3DSX homebrew file (launch target only; not a catalogue match)"
                    .into(),
        }
    }
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}
fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Bounded structural validation of a 3DSX header against the file size.
pub fn validate_three_dsx_header(head: &[u8], file_size: u64) -> Result<(), ThreeDsxRefusal> {
    let invalid = |detail: &str| refuse(ThreeDsxRefusalKind::InvalidHeader, detail);
    if head.len() < BASE_HEADER as usize || &head[0..4] != b"3DSX" {
        return Err(invalid("the file does not start with the 3DSX magic"));
    }
    let header_size = le16(head, 4);
    if header_size != BASE_HEADER && header_size != EXTENDED_HEADER {
        return Err(invalid("unsupported 3DSX header size"));
    }
    if head.len() < header_size as usize {
        return Err(invalid("the 3DSX header is truncated"));
    }
    if le16(head, 6) != RELOC_HEADER {
        return Err(invalid("unsupported 3DSX relocation header size"));
    }
    if le32(head, 8) != 0 {
        return Err(invalid("unsupported 3DSX format version"));
    }
    let (code, rodata, data, bss) = (
        u64::from(le32(head, 16)),
        u64::from(le32(head, 20)),
        u64::from(le32(head, 24)),
        u64::from(le32(head, 28)),
    );
    if bss > data {
        return Err(invalid("3DSX bss is larger than its data segment"));
    }
    let relocs = 3 * u64::from(RELOC_HEADER);
    let image = code + rodata + (data - bss);
    if u64::from(header_size) + relocs + image > file_size {
        return Err(invalid("3DSX segments extend past the end of the file"));
    }
    if header_size == EXTENDED_HEADER {
        let (smdh_offset, smdh_size) = (u64::from(le32(head, 32)), u64::from(le32(head, 36)));
        if smdh_size != 0 && smdh_offset + smdh_size > file_size {
            return Err(invalid("3DSX SMDH extends past the end of the file"));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Reads and hashes `path` once and returns evidence bound to those bytes.
pub fn verify_three_dsx(path: &Path) -> Result<ThreeDsxEvidence, ThreeDsxRefusal> {
    if AzaharContentForm::from_path(path) != AzaharContentForm::ThreeDsx {
        return Err(refuse(
            ThreeDsxRefusalKind::NotThreeDsxExtension,
            "Azahar launch supports only loose .3dsx homebrew",
        ));
    }
    let unreadable = |e: std::io::Error| refuse(ThreeDsxRefusalKind::Unreadable, e.to_string());
    let link = fs::symlink_metadata(path).map_err(unreadable)?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err(refuse(
            ThreeDsxRefusalKind::NotRegularFile,
            "the .3dsx is not a regular file",
        ));
    }
    let before = CapturedFileIdentity::capture(&link);
    if before.size > AZAHAR_HOMEBREW_MAX_BYTES {
        return Err(refuse(
            ThreeDsxRefusalKind::TooLarge,
            "the .3dsx is larger than the homebrew inspection limit",
        ));
    }
    // O_NOFOLLOW | O_NONBLOCK: a path swapped to a symlink or FIFO after the
    // stat above is refused or opened without blocking, then caught by the
    // handle-vs-stat identity comparison below.
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(unreadable)?
    };
    #[cfg(not(unix))]
    let mut file = fs::File::open(path).map_err(unreadable)?;
    // The handle we hash must be the file we statted.
    if CapturedFileIdentity::capture(&file.metadata().map_err(unreadable)?) != before {
        return Err(refuse(
            ThreeDsxRefusalKind::ChangedWhileReading,
            "the .3dsx changed while it was being opened",
        ));
    }
    let mut hasher = Sha256::new();
    let mut head = Vec::with_capacity(EXTENDED_HEADER as usize);
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(unreadable)?;
        if read == 0 {
            break;
        }
        if head.len() < EXTENDED_HEADER as usize {
            let need = EXTENDED_HEADER as usize - head.len();
            head.extend_from_slice(&buffer[..read.min(need)]);
        }
        total += read as u64;
        if total > AZAHAR_HOMEBREW_MAX_BYTES {
            return Err(refuse(
                ThreeDsxRefusalKind::TooLarge,
                "the .3dsx is larger than the homebrew inspection limit",
            ));
        }
        hasher.update(&buffer[..read]);
    }
    let after = CapturedFileIdentity::capture(&fs::symlink_metadata(path).map_err(unreadable)?);
    if after != before || total != before.size {
        return Err(refuse(
            ThreeDsxRefusalKind::ChangedWhileReading,
            "the .3dsx changed while it was being read",
        ));
    }
    validate_three_dsx_header(&head, total)?;
    let display_title = inspect_azahar_3dsx(path)
        .ok()
        .flatten()
        .and_then(|AzaharTitleIdentity { title, .. }| title);
    Ok(ThreeDsxEvidence {
        path: path.to_path_buf(),
        file_identity: before,
        sha256: hex(&hasher.finalize()),
        size: total,
        display_title,
    })
}

/// One reviewed homebrew launch. Everything is rechecked at execute time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzaharHomebrewLaunchRequest {
    pub evidence: ThreeDsxEvidence,
    pub expected_executable: PathBuf,
    pub executable_identity: CapturedFileIdentity,
}

#[derive(Debug)]
pub enum AzaharHomebrewLaunchError {
    Content(ThreeDsxRefusal),
    ContentChanged(String),
    Executable(String),
    Preflight(AzaharPreflightError),
    Spawn(std::io::Error),
}

/// Re-verifies the bytes, rediscovers and re-identifies Azahar, then runs the
/// existing Azahar preflight and spawn. Nothing from the review is trusted.
pub fn preflight_and_launch_azahar_homebrew(
    request: &AzaharHomebrewLaunchRequest,
    roots: &AzaharDiscoveryRoots,
) -> Result<LaunchedAzaharProcess, AzaharHomebrewLaunchError> {
    let fresh =
        verify_three_dsx(&request.evidence.path).map_err(AzaharHomebrewLaunchError::Content)?;
    if fresh.sha256 != request.evidence.sha256
        || fresh.file_identity != request.evidence.file_identity
    {
        return Err(AzaharHomebrewLaunchError::ContentChanged(
            "the .3dsx changed since it was checked".into(),
        ));
    }
    let executable = discover_azahar_executable(roots).ok_or_else(|| {
        AzaharHomebrewLaunchError::Executable("no usable Azahar executable was found".into())
    })?;
    if executable != request.expected_executable {
        return Err(AzaharHomebrewLaunchError::Executable(
            "the Azahar executable is not the one that was checked".into(),
        ));
    }
    let profile = discover_azahar_profile(roots, executable.clone());
    if !azahar_profile_eligible(&profile) {
        return Err(AzaharHomebrewLaunchError::Executable(
            "the Azahar configuration is unreadable or oversized".into(),
        ));
    }
    let profile_identity = profile
        .config
        .as_ref()
        .filter(|_| profile.config_state == AzaharEvidenceState::Present)
        .and_then(|config| fs::symlink_metadata(config).ok())
        .map(|metadata| CapturedFileIdentity::capture(&metadata));
    let launch = AzaharLaunchRequest {
        executable,
        platform: AZAHAR_SUPPORTED_PLATFORM_ID.into(),
        content: fresh.path.clone(),
        content_form: AzaharContentForm::ThreeDsx,
        profile: profile.config,
        profile_state: profile.config_state,
        profile_identity,
        title_identity: inspect_azahar_3dsx(&fresh.path).ok().flatten(),
        keys_state: AzaharEvidenceState::Unknown,
        system_data_state: AzaharEvidenceState::Unknown,
        captured_identity: fresh.file_identity,
        executable_identity: request.executable_identity,
    };
    let command = preflight_azahar_launch(&launch, &fresh.identity_status())
        .map_err(AzaharHomebrewLaunchError::Preflight)?;
    spawn_azahar(command).map_err(AzaharHomebrewLaunchError::Spawn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A minimal structurally valid extended-header 3DSX with an SMDH block.
    pub(crate) fn sample_3dsx() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"3DSX");
        bytes.extend_from_slice(&EXTENDED_HEADER.to_le_bytes());
        bytes.extend_from_slice(&RELOC_HEADER.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // format version
        bytes.extend_from_slice(&0u32.to_le_bytes()); // flags
        bytes.extend_from_slice(&16u32.to_le_bytes()); // code
        bytes.extend_from_slice(&8u32.to_le_bytes()); // rodata
        bytes.extend_from_slice(&8u32.to_le_bytes()); // data
        bytes.extend_from_slice(&4u32.to_le_bytes()); // bss
        let smdh_offset = 0x2C + 3 * 0x0C + 16 + 8 + 4;
        bytes.extend_from_slice(&(smdh_offset as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // smdh size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // fs offset
        bytes.resize(0x2C + 36 + 28, 0);
        bytes
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn fake_azahar(dir: &Path, exit_code: i32) -> PathBuf {
        let path = dir.join("azahar");
        fs::write(&path, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn roots(executable: &Path, config_root: &Path) -> AzaharDiscoveryRoots {
        AzaharDiscoveryRoots {
            explicit_executable: Some(executable.to_path_buf()),
            path: None,
            config_root: Some(config_root.to_path_buf()),
        }
    }

    fn request(evidence: ThreeDsxEvidence, executable: &Path) -> AzaharHomebrewLaunchRequest {
        AzaharHomebrewLaunchRequest {
            evidence,
            expected_executable: executable.to_path_buf(),
            executable_identity: CapturedFileIdentity::capture(
                &fs::symlink_metadata(executable).unwrap(),
            ),
        }
    }

    #[test]
    fn valid_header_resolves_a_namespaced_launch_only_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "My Homebrew.3dsx", &sample_3dsx());
        let evidence = verify_three_dsx(&path).unwrap();
        let CanonicalIdentityStatus::Resolved(identity) = evidence.identity_status() else {
            panic!("expected resolved");
        };
        assert_eq!(identity.platform_id, "Nintendo 3DS");
        assert!(identity.game_key.starts_with(AZAHAR_HOMEBREW_KEY_PREFIX));
        assert_eq!(
            identity.game_key.len(),
            AZAHAR_HOMEBREW_KEY_PREFIX.len() + 64
        );
    }

    #[test]
    fn extension_alone_is_never_identity() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            (
                "fake.3dsx",
                b"not a 3dsx at all, just text padding.....".to_vec(),
            ),
            ("empty.3dsx", Vec::new()),
        ] {
            let path = write(dir.path(), name, &bytes);
            assert_eq!(
                verify_three_dsx(&path).unwrap_err().kind,
                ThreeDsxRefusalKind::InvalidHeader,
                "{name}"
            );
        }
    }

    #[test]
    fn a_valid_3dsx_under_another_extension_or_retail_form_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["real.bin", "retail.3ds", "retail.cia", "retail.cci"] {
            let path = write(dir.path(), name, &sample_3dsx());
            assert_eq!(
                verify_three_dsx(&path).unwrap_err().kind,
                ThreeDsxRefusalKind::NotThreeDsxExtension,
                "{name}"
            );
        }
    }

    #[test]
    fn corrupt_headers_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mutate = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut bytes = sample_3dsx();
            f(&mut bytes);
            bytes
        };
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("badmagic", mutate(&|b| b[0] = b'X')),
            ("headersize", mutate(&|b| b[4] = 0x30)),
            ("relocsize", mutate(&|b| b[6] = 0x10)),
            ("version", mutate(&|b| b[8] = 1)),
            ("hugecode", mutate(&|b| b[19] = 0x7f)),
            ("bssbig", mutate(&|b| b[28] = 99)),
            ("smdh", mutate(&|b| b[36] = 0xff)),
            ("short", sample_3dsx()[..20].to_vec()),
        ];
        for (name, bytes) in cases {
            let path = write(dir.path(), &format!("{name}.3dsx"), &bytes);
            assert_eq!(
                verify_three_dsx(&path).unwrap_err().kind,
                ThreeDsxRefusalKind::InvalidHeader,
                "{name}"
            );
        }
    }

    #[test]
    fn a_fifo_at_the_path_is_refused_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.3dsx");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(
            verify_three_dsx(&fifo).unwrap_err().kind,
            ThreeDsxRefusalKind::NotRegularFile
        );
    }

    #[test]
    fn symlinks_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = write(dir.path(), "real.3dsx", &sample_3dsx());
        let link = dir.path().join("link.3dsx");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            verify_three_dsx(&link).unwrap_err().kind,
            ThreeDsxRefusalKind::NotRegularFile
        );
    }

    #[test]
    fn launch_runs_the_existing_preflight_and_spawns() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "game.3dsx", &sample_3dsx());
        let exe = fake_azahar(dir.path(), 0);
        let evidence = verify_three_dsx(&path).unwrap();
        let mut process = preflight_and_launch_azahar_homebrew(
            &request(evidence, &exe),
            &roots(&exe, dir.path()),
        )
        .unwrap();
        for _ in 0..100 {
            if process.poll().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("fixture process never exited");
    }

    #[test]
    fn changed_bytes_after_review_refuse_before_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "game.3dsx", &sample_3dsx());
        let exe = fake_azahar(dir.path(), 0);
        let evidence = verify_three_dsx(&path).unwrap();
        let mut changed = sample_3dsx();
        changed[0x2C + 36 + 5] = 7; // same size, different bytes
        fs::write(&path, changed).unwrap();
        assert!(matches!(
            preflight_and_launch_azahar_homebrew(
                &request(evidence, &exe),
                &roots(&exe, dir.path())
            ),
            Err(AzaharHomebrewLaunchError::ContentChanged(_))
        ));
    }

    #[test]
    fn replaced_with_non_3dsx_after_review_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "game.3dsx", &sample_3dsx());
        let exe = fake_azahar(dir.path(), 0);
        let evidence = verify_three_dsx(&path).unwrap();
        fs::write(&path, b"replaced").unwrap();
        assert!(matches!(
            preflight_and_launch_azahar_homebrew(
                &request(evidence, &exe),
                &roots(&exe, dir.path())
            ),
            Err(AzaharHomebrewLaunchError::Content(_))
        ));
    }

    #[test]
    fn executable_swap_or_move_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "game.3dsx", &sample_3dsx());
        let exe = fake_azahar(dir.path(), 0);
        let evidence = verify_three_dsx(&path).unwrap();
        let req = request(evidence, &exe);
        // Different executable discovered than the one that was checked.
        let other_dir = tempfile::tempdir().unwrap();
        let other = fake_azahar(other_dir.path(), 0);
        assert!(matches!(
            preflight_and_launch_azahar_homebrew(&req, &roots(&other, dir.path())),
            Err(AzaharHomebrewLaunchError::Executable(_))
        ));
        // Same path, replaced file (new inode): the existing drift check refuses.
        fs::remove_file(&exe).unwrap();
        fake_azahar(dir.path(), 1);
        assert!(matches!(
            preflight_and_launch_azahar_homebrew(&req, &roots(&exe, dir.path())),
            Err(AzaharHomebrewLaunchError::Preflight(_))
        ));
        // Gone entirely.
        fs::remove_file(&exe).unwrap();
        assert!(matches!(
            preflight_and_launch_azahar_homebrew(&req, &roots(&exe, dir.path())),
            Err(AzaharHomebrewLaunchError::Executable(_))
        ));
    }

    #[test]
    fn unreadable_config_blocks_launch() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "game.3dsx", &sample_3dsx());
        let exe = fake_azahar(dir.path(), 0);
        let big = fs::File::create(dir.path().join("qt-config.ini")).unwrap();
        big.set_len(crate::patch_manager::AZAHAR_MAX_CONFIG_BYTES + 1)
            .unwrap();
        let evidence = verify_three_dsx(&path).unwrap();
        assert!(matches!(
            preflight_and_launch_azahar_homebrew(
                &request(evidence, &exe),
                &roots(&exe, dir.path())
            ),
            Err(AzaharHomebrewLaunchError::Executable(_))
        ));
    }
}
