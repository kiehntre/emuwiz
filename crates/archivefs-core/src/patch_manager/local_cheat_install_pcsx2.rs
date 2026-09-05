//! Bridges a user-selected local PCSX2 `.pnach` file into the existing,
//! unmodified PCSX2 install-plan pipeline (`pcsx2_install_plan`,
//! `pcsx2_pnach`, `shared_transaction`).
//!
//! This module adds no new write path. It only:
//! - validates the user-picked source file with the same source-path
//!   safety rules used across every local-file install in this codebase
//!   (reject symlink/directory/non-regular/oversized/wrong-extension
//!   before any parse),
//! - parses the file's own `patch=` lines with the existing, unmodified
//!   [`PnachPatchLine::parse`] validator (never a new cheat-code format),
//! - binds the file to the currently selected game via the exact PCSX2
//!   filename convention [`super::pcsx2::parse_patch_identity`] already
//!   reads back from `<SERIAL>_<CRC>.pnach` / `<CRC>.pnach` names, checked
//!   against the selected game's own verified [`Pcsx2GameIdentity`],
//! - and produces exactly the [`ManagedPnachCheat`] one entry that
//!   [`super::pcsx2_install_plan::stage_pcsx2_pnach`] and
//!   [`super::pcsx2_install_plan::build_pcsx2_install_preview`] (both
//!   unmodified) already know how to stage, preview, apply, and roll back.
//!
//! ## Scope
//!
//! PCSX2 `.pnach` only, matching the V1 discovery audit's identified gap.
//! Dolphin Gecko/Action Replay and Xenia `.patch.toml` local-file install
//! each need their own equivalent bridge and are out of scope here.
//!
//! ## Idempotent re-apply
//!
//! [`ManagedPnachCheat::id`] is derived from the source file's own SHA-256
//! digest, so re-selecting the exact same file always resolves to the same
//! managed block ID. [`check_local_pcsx2_install_state`] detects that case
//! *before* staging - [`merge_managed_pnach_cheats`] treats a duplicate
//! managed ID as a hard error (it has no concept of "identical, skip"), so
//! this bridge must never hand a duplicate ID to it. Re-selecting the exact
//! same file therefore reports [`LocalPcsx2InstallState::AlreadyInstalled`]
//! and takes no further action, rather than surfacing that internal error
//! as a destructive-looking conflict.

use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::pcsx2::parse_patch_identity;
use super::pcsx2_identity::{Pcsx2GameIdentity, pcsx2_cheats_directory};
use super::pcsx2_install_plan::{
    Pcsx2InstallPlanError, Pcsx2InstallPlanErrorKind, load_existing_pcsx2_pnach,
    pcsx2_pnach_filename,
};
use super::pcsx2_local::Pcsx2Profile;
use super::pcsx2_pnach::{
    MAX_MANAGED_PNACH_BYTES, ManagedPnachCheat, PnachPatchLine, parse_pnach_document,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalPcsx2FileError {
    NotFound {
        path: PathBuf,
        detail: String,
    },
    IsDirectory {
        path: PathBuf,
    },
    IsSymlink {
        path: PathBuf,
    },
    NotRegularFile {
        path: PathBuf,
    },
    UnsupportedExtension {
        path: PathBuf,
    },
    TooLarge {
        path: PathBuf,
    },
    Malformed {
        path: PathBuf,
        detail: String,
    },
    /// The selected game's own PCSX2 identity is not yet a verified CRC.
    IdentityUnresolved {
        detail: String,
    },
    /// The file name's `<SERIAL>_<CRC>` (or bare `<CRC>`) does not match
    /// the selected game's verified identity.
    IdentityConflict {
        detail: String,
    },
    /// Surfaced unchanged from the existing PCSX2 install-plan machinery
    /// (destination unsafe, profile unavailable, document unsafe, ...).
    Plan(Pcsx2InstallPlanError),
}

impl std::fmt::Display for LocalPcsx2FileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::IsDirectory { path } => {
                write!(
                    formatter,
                    "{} is a directory, not a cheat file",
                    path.display()
                )
            }
            Self::IsSymlink { path } => {
                write!(
                    formatter,
                    "{} is a symlink and is not followed",
                    path.display()
                )
            }
            Self::NotRegularFile { path } => {
                write!(formatter, "{} is not a regular file", path.display())
            }
            Self::UnsupportedExtension { path } => write!(
                formatter,
                "{} is not a supported PCSX2 .pnach cheat file",
                path.display()
            ),
            Self::TooLarge { path } => write!(
                formatter,
                "{} exceeds the managed-file byte limit",
                path.display()
            ),
            Self::Malformed { path, detail } => write!(formatter, "{}: {detail}", path.display()),
            Self::IdentityUnresolved { detail } => formatter.write_str(detail),
            Self::IdentityConflict { detail } => formatter.write_str(detail),
            Self::Plan(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for LocalPcsx2FileError {}

impl From<Pcsx2InstallPlanError> for LocalPcsx2FileError {
    fn from(error: Pcsx2InstallPlanError) -> Self {
        Self::Plan(error)
    }
}

/// The one managed cheat a local `.pnach` file resolves to, plus the
/// filename-derived identity it was checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPcsx2Discovery {
    pub source_path: PathBuf,
    pub source_sha256: String,
    pub detected_serial: Option<String>,
    pub detected_crc: String,
    pub cheat: ManagedPnachCheat,
}

/// Validates `source_path` (must exist, be a regular non-symlink file, have
/// a `.pnach` extension, and be within the managed-file byte limit), parses
/// its `patch=` lines with the existing validator, and checks its
/// `<SERIAL>_<CRC>.pnach` / `<CRC>.pnach` filename identity against the
/// selected game's verified PCSX2 identity.
pub fn discover_local_pcsx2_pnach_file(
    source_path: &Path,
    identity: &Pcsx2GameIdentity,
) -> Result<LocalPcsx2Discovery, LocalPcsx2FileError> {
    let metadata =
        std::fs::symlink_metadata(source_path).map_err(|error| LocalPcsx2FileError::NotFound {
            path: source_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    if metadata.file_type().is_symlink() {
        return Err(LocalPcsx2FileError::IsSymlink {
            path: source_path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        return Err(LocalPcsx2FileError::IsDirectory {
            path: source_path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(LocalPcsx2FileError::NotRegularFile {
            path: source_path.to_path_buf(),
        });
    }
    let has_pnach_extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pnach"));
    if !has_pnach_extension {
        return Err(LocalPcsx2FileError::UnsupportedExtension {
            path: source_path.to_path_buf(),
        });
    }
    if metadata.len() > MAX_MANAGED_PNACH_BYTES as u64 {
        return Err(LocalPcsx2FileError::TooLarge {
            path: source_path.to_path_buf(),
        });
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(source_path)
        .map_err(|error| LocalPcsx2FileError::NotFound {
            path: source_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    let mut bytes = Vec::new();
    file.take((MAX_MANAGED_PNACH_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| LocalPcsx2FileError::Malformed {
            path: source_path.to_path_buf(),
            detail: format!("could not be read: {error}"),
        })?;
    if bytes.len() > MAX_MANAGED_PNACH_BYTES {
        return Err(LocalPcsx2FileError::TooLarge {
            path: source_path.to_path_buf(),
        });
    }

    let (patch_lines, title_hint) =
        parse_local_pnach_bytes(&bytes).map_err(|detail| LocalPcsx2FileError::Malformed {
            path: source_path.to_path_buf(),
            detail,
        })?;

    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let (detected_serial, detected_crc) = parse_patch_identity(stem);
    let Some(detected_crc) = detected_crc else {
        return Err(LocalPcsx2FileError::IdentityConflict {
            detail: format!(
                "{stem}.pnach does not follow the <SERIAL>_<CRC>.pnach or <CRC>.pnach naming \
                 convention, so its target game cannot be verified"
            ),
        });
    };
    let verified_crc =
        identity
            .verified_crc()
            .ok_or_else(|| LocalPcsx2FileError::IdentityUnresolved {
                detail: "the selected game has no verified PCSX2 executable CRC yet".to_string(),
            })?;
    if !detected_crc.eq_ignore_ascii_case(verified_crc) {
        return Err(LocalPcsx2FileError::IdentityConflict {
            detail: format!(
                "{stem}.pnach targets CRC {detected_crc}, which does not match the selected \
                 game's verified CRC {verified_crc}"
            ),
        });
    }
    if let (Some(file_serial), Some(game_serial)) = (&detected_serial, &identity.serial)
        && !file_serial.eq_ignore_ascii_case(game_serial)
    {
        return Err(LocalPcsx2FileError::IdentityConflict {
            detail: format!(
                "{stem}.pnach targets serial {file_serial}, which does not match the selected \
                 game's verified serial {game_serial}"
            ),
        });
    }

    let source_sha256 = hex_sha256(&bytes);
    let id = format!("local-{}", &source_sha256[..16]);
    let name = title_hint.unwrap_or_else(|| {
        if stem.is_empty() {
            "Imported PCSX2 cheat file".to_string()
        } else {
            stem.to_string()
        }
    });
    let cheat = ManagedPnachCheat {
        id,
        name,
        description: Some(format!(
            "Installed from local file {}",
            source_path.display()
        )),
        patch_lines,
    };

    Ok(LocalPcsx2Discovery {
        source_path: source_path.to_path_buf(),
        source_sha256,
        detected_serial,
        detected_crc,
        cheat,
    })
}

/// Whether the discovered cheat's exact content is already installed at
/// the resolved destination for `profile`. Reads the existing destination
/// through the same [`load_existing_pcsx2_pnach`] safety checks every other
/// PCSX2 install uses; never itself writes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPcsx2InstallState {
    New,
    AlreadyInstalled,
}

pub fn check_local_pcsx2_install_state(
    profile: &Pcsx2Profile,
    discovery: &LocalPcsx2Discovery,
) -> Result<LocalPcsx2InstallState, LocalPcsx2FileError> {
    let cheats_directory =
        pcsx2_cheats_directory(profile).ok_or_else(|| Pcsx2InstallPlanError {
            kind: Pcsx2InstallPlanErrorKind::ProfileUnavailable,
            path: Some(profile.configuration_path.clone()),
            detail: "confirmed profile has no safe normal cheats directory".to_string(),
        })?;
    let file_name = pcsx2_pnach_filename(
        discovery.detected_serial.as_deref(),
        &discovery.detected_crc,
    )?;
    let destination_path = cheats_directory.join(&file_name);
    let original = load_existing_pcsx2_pnach(&destination_path)?;
    let document = parse_pnach_document(&original).map_err(|failure| Pcsx2InstallPlanError {
        kind: Pcsx2InstallPlanErrorKind::DocumentUnsafe,
        path: Some(destination_path.clone()),
        detail: failure.to_string(),
    })?;
    Ok(
        if document.managed_block_ids().contains(&discovery.cheat.id) {
            LocalPcsx2InstallState::AlreadyInstalled
        } else {
            LocalPcsx2InstallState::New
        },
    )
}

/// Parses only what an install needs: `gametitle=` (first occurrence, used
/// as a display name), `comment=`/`comment_*=` (ignored - free text), and
/// `patch=` lines (validated with the existing, unmodified parser). Unlike
/// the read-only import index, this is a write-path parser: any line that
/// is not blank, a `//`/`;` comment, `gametitle=`, `comment(_...)=`, or a
/// valid `patch=` line is treated as malformed and rejects the whole file,
/// and a file with zero valid patch lines is rejected the same way.
fn parse_local_pnach_bytes(bytes: &[u8]) -> Result<(Vec<PnachPatchLine>, Option<String>), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "PNACH is not valid UTF-8".to_string())?;
    let mut patch_lines = Vec::new();
    let mut title = None;
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with(';') {
            continue;
        }
        if let Some(value) = line.strip_prefix("gametitle=") {
            let value = value.trim();
            if !value.is_empty() && title.is_none() {
                title = Some(value.to_string());
            }
            continue;
        }
        if line.starts_with("comment=") || line.starts_with("comment_") {
            continue;
        }
        if line.starts_with("patch=") {
            let parsed = PnachPatchLine::parse(line)
                .map_err(|error| format!("line {}: {error}", index + 1))?;
            patch_lines.push(parsed);
            continue;
        }
        return Err(format!(
            "line {}: unsupported or malformed PNACH line",
            index + 1
        ));
    }
    if patch_lines.is_empty() {
        return Err("PNACH contains no valid patch codes".to_string());
    }
    Ok((patch_lines, title))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::{
        Pcsx2IdentityState, Pcsx2InstallationType, Pcsx2PatchCategory, Pcsx2PatchDirectory,
        Pcsx2PatchDirectoryState, Pcsx2ProfileScope,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "archivefs-pcsx2-local-install-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn profile(root: &Path) -> Pcsx2Profile {
        Pcsx2Profile {
            profile_id: "fixture".to_string(),
            installation_type: Pcsx2InstallationType::Portable,
            scope: Pcsx2ProfileScope::Portable,
            configuration_path: root.to_path_buf(),
            provenance: "test",
            eligible: true,
            blockers: Vec::new(),
            patch_directories: vec![Pcsx2PatchDirectory {
                path: root.join("cheats"),
                category: Pcsx2PatchCategory::Cheats,
                state: Pcsx2PatchDirectoryState::Missing,
                warning: None,
                identity: None,
            }],
            configuration_identity: None,
            executable_candidates: Vec::new(),
        }
    }

    fn verified_identity(
        serial: Option<&str>,
        crc: &str,
        archive_path: PathBuf,
    ) -> Pcsx2GameIdentity {
        Pcsx2GameIdentity {
            archive_path,
            title: "Test Game".to_string(),
            region: None,
            serial: serial.map(str::to_string),
            executable_crc: Some(crc.to_string()),
            state: Pcsx2IdentityState::Verified,
            evidence: Vec::new(),
            plain_failure_reason: None,
        }
    }

    fn unresolved_identity(archive_path: PathBuf) -> Pcsx2GameIdentity {
        Pcsx2GameIdentity {
            archive_path,
            title: "Test Game".to_string(),
            region: None,
            serial: None,
            executable_crc: None,
            state: Pcsx2IdentityState::MissingCrc,
            evidence: Vec::new(),
            plain_failure_reason: Some("no verified executable CRC".to_string()),
        }
    }

    const VALID_PNACH: &str =
        "gametitle=Test Game\ncomment=Infinite Health\npatch=1,EE,20123456,word,00000064\n";

    #[test]
    fn supported_local_pnach_matches_the_selected_verified_game() {
        let root = temp("supported");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("SLUS-20312_A1B2C3D4.pnach");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let identity = verified_identity(Some("SLUS-20312"), "A1B2C3D4", root.join("game.iso"));

        let discovery =
            discover_local_pcsx2_pnach_file(&file, &identity).expect("discovery succeeds");
        assert_eq!(discovery.detected_crc, "A1B2C3D4");
        assert_eq!(discovery.detected_serial.as_deref(), Some("SLUS-20312"));
        assert_eq!(discovery.cheat.patch_lines.len(), 1);
        assert_eq!(discovery.cheat.name, "Test Game");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unsupported_extension_is_rejected() {
        let root = temp("ext");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("cheat.cht");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));

        let error = discover_local_pcsx2_pnach_file(&file, &identity).unwrap_err();
        assert!(matches!(
            error,
            LocalPcsx2FileError::UnsupportedExtension { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_pnach_is_rejected() {
        let root = temp("malformed");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("A1B2C3D4.pnach");
        std::fs::write(&file, "this is not a pnach file at all\n").unwrap();
        let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));

        let error = discover_local_pcsx2_pnach_file(&file, &identity).unwrap_err();
        assert!(matches!(error, LocalPcsx2FileError::Malformed { .. }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wrong_game_crc_is_blocked() {
        let root = temp("wrong-crc");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("DEADBEEF.pnach");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));

        let error = discover_local_pcsx2_pnach_file(&file, &identity).unwrap_err();
        assert!(matches!(
            error,
            LocalPcsx2FileError::IdentityConflict { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wrong_game_serial_is_blocked_even_with_matching_crc() {
        let root = temp("wrong-serial");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("SLUS-99999_A1B2C3D4.pnach");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let identity = verified_identity(Some("SLUS-20312"), "A1B2C3D4", root.join("game.iso"));

        let error = discover_local_pcsx2_pnach_file(&file, &identity).unwrap_err();
        assert!(matches!(
            error,
            LocalPcsx2FileError::IdentityConflict { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unresolved_identity_is_refused_before_any_file_use_matters() {
        let root = temp("unresolved");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("A1B2C3D4.pnach");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let identity = unresolved_identity(root.join("game.iso"));

        let error = discover_local_pcsx2_pnach_file(&file, &identity).unwrap_err();
        assert!(matches!(
            error,
            LocalPcsx2FileError::IdentityUnresolved { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unresolvable_filename_identity_is_blocked() {
        let root = temp("no-name-identity");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("my cheats.pnach");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));

        let error = discover_local_pcsx2_pnach_file(&file, &identity).unwrap_err();
        assert!(matches!(
            error,
            LocalPcsx2FileError::IdentityConflict { .. }
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn symlinked_source_file_is_rejected() {
        let root = temp("symlink");
        std::fs::create_dir_all(&root).unwrap();
        let real = root.join("real.pnach");
        std::fs::write(&real, VALID_PNACH).unwrap();
        let link = root.join("A1B2C3D4.pnach");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));
            let error = discover_local_pcsx2_pnach_file(&link, &identity).unwrap_err();
            assert!(matches!(error, LocalPcsx2FileError::IsSymlink { .. }));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_source_is_rejected() {
        let root = temp("dir");
        std::fs::create_dir_all(&root).unwrap();
        let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));
        let error = discover_local_pcsx2_pnach_file(&root, &identity).unwrap_err();
        assert!(matches!(error, LocalPcsx2FileError::IsDirectory { .. }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn discovery_never_mutates_the_selected_file() {
        let root = temp("no-mutate");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("A1B2C3D4.pnach");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let before = std::fs::read(&file).unwrap();
        let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));

        let _ = discover_local_pcsx2_pnach_file(&file, &identity);

        let after = std::fs::read(&file).unwrap();
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn same_file_selected_twice_reports_already_installed_after_the_first_apply() {
        let root = temp("already-installed");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("A1B2C3D4.pnach");
        std::fs::write(&file, VALID_PNACH).unwrap();
        let identity = verified_identity(None, "A1B2C3D4", root.join("game.iso"));
        let pcsx2_profile = profile(&root);

        let discovery =
            discover_local_pcsx2_pnach_file(&file, &identity).expect("discovery succeeds");
        assert_eq!(
            check_local_pcsx2_install_state(&pcsx2_profile, &discovery).unwrap(),
            LocalPcsx2InstallState::New
        );

        let staged = super::super::pcsx2_install_plan::stage_pcsx2_pnach(
            &root.join("staging"),
            &pcsx2_profile,
            discovery.detected_serial.as_deref(),
            &discovery.detected_crc,
            std::slice::from_ref(&discovery.cheat),
        )
        .unwrap();
        std::fs::create_dir_all(staged.destination_path.parent().unwrap()).unwrap();
        std::fs::write(&staged.destination_path, &staged.contents).unwrap();

        let redisovered =
            discover_local_pcsx2_pnach_file(&file, &identity).expect("discovery succeeds again");
        assert_eq!(
            check_local_pcsx2_install_state(&pcsx2_profile, &redisovered).unwrap(),
            LocalPcsx2InstallState::AlreadyInstalled
        );

        let _ = std::fs::remove_dir_all(root);
    }
}
