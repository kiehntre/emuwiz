//! Read-only bridge from a selected Amiga game's inspected WHDLoad slave to
//! the trainer option editor.
//!
//! The slave's own `ws_config` declarations are the only option source: no
//! catalogue, title or filename is consulted. Exactly one host slave file must
//! be present; several candidates are reported, never chosen between. The
//! slave is re-read once and its declarations and SHA-256 come from the same
//! bytes, so the hash handed to the preview describes what was parsed.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::identity_source::whdload::parse_whdload_slave;
use crate::open_retro_cheat_providers::{WhdloadCustomOption, parse_whdload_custom_options};

use super::amiga_whdload_local::{AmigaGameInspection, AmigaGameMapping};
use super::whdload_trainer::{
    TrainerOption, TrainerOptionKind, TrainerOptionReadiness, TrainerOptionSource,
    WhdloadTrainerIdentity, project_whdload_trainer_options,
};

const MAX_SLAVE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhdloadTrainerDiscoveryState {
    /// One host slave, a verified game identity and at least one option.
    Ready,
    /// The game identity is not verified; options are listed but the
    /// preview cannot be built.
    IdentityUnverified,
    NoSlave,
    /// More than one valid slave; EmuWiz does not pick one.
    AmbiguousSlaves,
    /// The only slave lives inside an HDF image. Its options are listed but
    /// the preview needs a host file it can re-hash before applying.
    SlaveInsideHdf,
    /// The slave changed since it was inspected.
    SlaveChanged,
    SlaveUnreadable,
    /// The slave has no `ws_config` (runtime < 16) or declares no CUSTOM1-5
    /// options.
    NoTrainerOptions,
}

impl WhdloadTrainerDiscoveryState {
    /// Plain wording for the editor's empty or blocked state.
    #[must_use]
    pub const fn plain_label(self) -> &'static str {
        match self {
            Self::Ready => "Trainer options found in this game's WHDLoad slave",
            Self::IdentityUnverified => "Confirm the exact game before changing trainer options",
            Self::NoSlave => "No WHDLoad slave was found for this game",
            Self::AmbiguousSlaves => "This game has more than one WHDLoad slave",
            Self::SlaveInsideHdf => "The WHDLoad slave is inside a hard-disk image",
            Self::SlaveChanged => "The WHDLoad slave changed since it was checked",
            Self::SlaveUnreadable => "The WHDLoad slave could not be read",
            Self::NoTrainerOptions => "This game's WHDLoad slave has no trainer options",
        }
    }

    /// Whether [`WhdloadTrainerDiscovery::identity`] can feed
    /// `build_whdload_trainer_preview`.
    #[must_use]
    pub const fn can_preview(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadTrainerDiscovery {
    pub state: WhdloadTrainerDiscoveryState,
    pub detail: String,
    pub slave_path: Option<PathBuf>,
    pub slave_sha256: Option<String>,
    pub slave_runtime_version: Option<u16>,
    /// Every C1-C5 declaration the slave makes, including opaque ones.
    pub declarations: Vec<WhdloadCustomOption>,
    /// Declarations projected against the supplied configuration bytes, with
    /// current values. Opaque options stay listed with `Unsupported`.
    pub options: Vec<TrainerOption>,
    /// Present only when `state` is `Ready`.
    pub identity: Option<WhdloadTrainerIdentity>,
    /// Display paths of every slave candidate considered.
    pub candidates: Vec<String>,
}

impl WhdloadTrainerDiscovery {
    fn blocked(state: WhdloadTrainerDiscoveryState, detail: impl Into<String>) -> Self {
        Self {
            state,
            detail: detail.into(),
            slave_path: None,
            slave_sha256: None,
            slave_runtime_version: None,
            declarations: Vec::new(),
            options: Vec::new(),
            identity: None,
            candidates: Vec::new(),
        }
    }

    /// Number of options a user could actually change.
    #[must_use]
    pub fn applyable_option_count(&self) -> usize {
        self.options
            .iter()
            .filter(|option| {
                option.kind != TrainerOptionKind::Opaque
                    && option.readiness == TrainerOptionReadiness::Ready
            })
            .count()
    }
}

/// Discover the trainer options of the selected game from its inspected
/// WHDLoad slave. `configuration` is the current content of the EmuWiz-owned
/// per-game option file (empty when it does not exist yet); it only supplies
/// current values. Nothing is written.
pub fn discover_whdload_trainer_options(
    inspection: &AmigaGameInspection,
    configuration: &[u8],
) -> WhdloadTrainerDiscovery {
    let candidates: Vec<String> = inspection
        .slave_candidates
        .iter()
        .map(|candidate| {
            candidate
                .provenance_path
                .clone()
                .unwrap_or_else(|| candidate.artifact.path.display().to_string())
        })
        .collect();
    let mut result = match inspection.slave_candidates.as_slice() {
        [] => WhdloadTrainerDiscovery::blocked(
            WhdloadTrainerDiscoveryState::NoSlave,
            "the game inspection found no valid WHDLoad slave",
        ),
        [candidate] if candidate.from_hdf => {
            let mut result = WhdloadTrainerDiscovery::blocked(
                WhdloadTrainerDiscoveryState::SlaveInsideHdf,
                "options are shown read-only; trainer changes need a slave file outside the image",
            );
            result.slave_sha256 = Some(candidate.artifact.hashes.sha256.clone());
            result.slave_runtime_version = Some(candidate.artifact.parsed.runtime_version);
            result.declarations = declarations_of(candidate.artifact.parsed.config.as_deref());
            result
        }
        [candidate] => {
            discover_from_host_slave(&candidate.artifact.path, &candidate.artifact.hashes.sha256)
        }
        _ => WhdloadTrainerDiscovery::blocked(
            WhdloadTrainerDiscoveryState::AmbiguousSlaves,
            "several valid WHDLoad slaves were found; none was chosen",
        ),
    };
    result.candidates = candidates;
    if result.declarations.is_empty()
        && matches!(
            result.state,
            WhdloadTrainerDiscoveryState::Ready | WhdloadTrainerDiscoveryState::SlaveInsideHdf
        )
    {
        result.state = WhdloadTrainerDiscoveryState::NoTrainerOptions;
        result.detail = if result
            .slave_runtime_version
            .is_some_and(|version| version < 16)
        {
            "the slave predates WHDLoad's ws_config option declarations".into()
        } else {
            "the slave declares no CUSTOM1-5 options".into()
        };
    }
    let verified_identity = (inspection.game_mapping == AmigaGameMapping::VerifiedIdentity)
        .then(|| inspection.verified_identity.clone())
        .flatten()
        .filter(|identity| !identity.trim().is_empty());
    if let (Some(path), Some(sha256)) = (&result.slave_path, &result.slave_sha256) {
        let source = TrainerOptionSource {
            slave_path: path.clone(),
            slave_sha256: sha256.clone(),
            verified_game_identity: verified_identity.clone().unwrap_or_default(),
            package_version: None,
        };
        match project_whdload_trainer_options(&result.declarations, &source, configuration) {
            Ok(options) => result.options = options,
            Err(error) => {
                result.state = WhdloadTrainerDiscoveryState::SlaveUnreadable;
                result.detail = format!("trainer options could not be projected: {error}");
            }
        }
    }
    if result.state == WhdloadTrainerDiscoveryState::Ready {
        match verified_identity {
            Some(identity) => {
                result.identity = Some(WhdloadTrainerIdentity {
                    slave_path: result.slave_path.clone().unwrap_or_default(),
                    expected_slave_sha256: result.slave_sha256.clone().unwrap_or_default(),
                    verified_game_identity: identity,
                    package_version: None,
                });
            }
            None => {
                result.state = WhdloadTrainerDiscoveryState::IdentityUnverified;
                result.detail = "the game identity is not verified".into();
            }
        }
    }
    result
}

fn declarations_of(config: Option<&str>) -> Vec<WhdloadCustomOption> {
    config.map(parse_whdload_custom_options).unwrap_or_default()
}

fn discover_from_host_slave(path: &Path, inspected_sha256: &str) -> WhdloadTrainerDiscovery {
    let bytes = match read_regular_bounded(path) {
        Ok(bytes) => bytes,
        Err(detail) => {
            return WhdloadTrainerDiscovery::blocked(
                WhdloadTrainerDiscoveryState::SlaveUnreadable,
                detail,
            );
        }
    };
    let sha256: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if !sha256.eq_ignore_ascii_case(inspected_sha256) {
        let mut result = WhdloadTrainerDiscovery::blocked(
            WhdloadTrainerDiscoveryState::SlaveChanged,
            "the slave's contents differ from the inspected slave; inspect the game again",
        );
        result.slave_path = Some(path.to_path_buf());
        return result;
    }
    let parsed = match parse_whdload_slave(&bytes) {
        Ok(parsed) => parsed,
        Err(error) => {
            return WhdloadTrainerDiscovery::blocked(
                WhdloadTrainerDiscoveryState::SlaveUnreadable,
                error.to_string(),
            );
        }
    };
    let mut result = WhdloadTrainerDiscovery::blocked(
        WhdloadTrainerDiscoveryState::Ready,
        "options come from the selected slave's own ws_config declarations",
    );
    result.slave_path = Some(path.to_path_buf());
    result.slave_sha256 = Some(sha256);
    result.slave_runtime_version = Some(parsed.runtime_version);
    result.declarations = declarations_of(parsed.config.as_deref());
    result
}

fn read_regular_bounded(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_absolute() {
        return Err("slave path is not absolute".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("slave is not a regular file".into());
    }
    if metadata.len() > MAX_SLAVE_BYTES {
        return Err("slave exceeds the size limit".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_SLAVE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_SLAVE_BYTES {
        return Err("slave exceeds the size limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::identity_source::whdload::{SlaveArtifact, SlaveHashes};
    use crate::patch_manager::amiga_whdload_local::{
        AmigaConfig, AmigaHealth, AmigaKickstart, AmigaKickstartState, AmigaSlaveCandidate,
    };
    use crate::patch_manager::whdload_trainer::{
        TrainerOptionReadiness, TrainerOptionSelection, TrainerOptionValue, WhdloadTrainerRequest,
        build_whdload_trainer_preview,
    };

    /// Minimal valid v17 slave: HUNK_HEADER, one code hunk holding the
    /// WHDLoad header with ws_config pointing at `config`.
    pub(crate) fn synthetic_slave(config: &str) -> Vec<u8> {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
        code.extend_from_slice(b"WHDLOADS");
        code.extend_from_slice(&17u16.to_be_bytes());
        code.extend_from_slice(&0u16.to_be_bytes()); // flags
        code.extend_from_slice(&0x80000u32.to_be_bytes()); // base mem
        code.extend_from_slice(&0u32.to_be_bytes()); // exec install
        code.extend_from_slice(&0u16.to_be_bytes()); // game loader (patched below)
        code.extend_from_slice(&0u16.to_be_bytes()); // current dir
        code.extend_from_slice(&0u16.to_be_bytes()); // dont cache
        code.extend_from_slice(&[0, 0]); // key debug, key exit
        code.extend_from_slice(&0u32.to_be_bytes()); // expansion memory
        code.extend_from_slice(&[0u8; 8]); // name, copy, info, kickname
        code.extend_from_slice(&0u32.to_be_bytes()); // kick size
        code.extend_from_slice(&0u16.to_be_bytes()); // kick crc
        let config_offset = 52u16;
        code.extend_from_slice(&config_offset.to_be_bytes()); // ws_config
        assert_eq!(code.len(), 52);
        code.extend_from_slice(config.as_bytes());
        code.push(0);
        if code.len() % 2 != 0 {
            code.push(0);
        }
        let entry = code.len() as u16;
        code[24..26].copy_from_slice(&entry.to_be_bytes());
        code.extend_from_slice(&[0x4e, 0x75]);
        while code.len() % 4 != 0 {
            code.push(0);
        }
        let mut out = Vec::new();
        for word in [
            0x3f3u32,
            0,
            1,
            0,
            0,
            (code.len() / 4) as u32,
            0x3e9,
            (code.len() / 4) as u32,
        ] {
            out.extend_from_slice(&word.to_be_bytes());
        }
        out.extend_from_slice(&code);
        out.extend_from_slice(&0x3f2u32.to_be_bytes());
        out
    }

    pub(crate) fn artifact(path: &Path, bytes: &[u8]) -> SlaveArtifact {
        SlaveArtifact {
            path: path.to_path_buf(),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            size_bytes: bytes.len() as u64,
            parsed: parse_whdload_slave(bytes).expect("synthetic slave parses"),
            hashes: SlaveHashes {
                sha1: String::new(),
                sha256: Sha256::digest(bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            },
        }
    }

    pub(crate) fn inspection(
        candidates: Vec<AmigaSlaveCandidate>,
        verified: bool,
    ) -> AmigaGameInspection {
        AmigaGameInspection {
            game_mapping: if verified {
                AmigaGameMapping::VerifiedIdentity
            } else {
                AmigaGameMapping::Unavailable
            },
            verified_identity: verified.then(|| "amiga:synthetic".into()),
            emulator_metadata: None,
            config: AmigaConfig {
                path: PathBuf::from("/synthetic/fs-uae.conf"),
                exists: false,
                readable: false,
                machine: Default::default(),
                warnings: Vec::new(),
            },
            per_game_config: None,
            slave_candidates: candidates,
            hdf_inspections: Vec::new(),
            save_state_candidates: Vec::new(),
            save_state_complete: true,
            health: AmigaHealth {
                detected: true,
                config_readable: true,
                kickstart: AmigaKickstart {
                    path: None,
                    state: AmigaKickstartState::Missing,
                    hash_verified: false,
                },
                whdload_support_present: true,
                controller_configured: false,
                game_mapping: AmigaGameMapping::Unavailable,
                warnings: Vec::new(),
            },
        }
    }

    pub(crate) fn host(path: &Path, bytes: &[u8]) -> AmigaSlaveCandidate {
        AmigaSlaveCandidate {
            provenance_path: None,
            artifact: artifact(path, bytes),
            from_hdf: false,
        }
    }

    const CONFIG: &str =
        "BW;C1:X:Infinite lives:0;C1:X:Infinite energy:1;C2:L:Start level:1,2,3;C4:Q:odd:0;";

    #[test]
    fn real_slave_declarations_reach_a_previewable_editor() {
        let root = tempfile::tempdir().unwrap();
        let slave = root.path().join("Game.slave");
        let bytes = synthetic_slave(CONFIG);
        fs::write(&slave, &bytes).unwrap();
        let found =
            discover_whdload_trainer_options(&inspection(vec![host(&slave, &bytes)], true), b"");
        assert_eq!(
            found.state,
            WhdloadTrainerDiscoveryState::Ready,
            "{}",
            found.detail
        );
        assert_eq!(found.slave_runtime_version, Some(17));
        assert_eq!(found.options.len(), 4);
        assert_eq!(found.applyable_option_count(), 3);
        assert_eq!(
            found.options[3].readiness,
            TrainerOptionReadiness::Unsupported
        );
        assert_eq!(found.options[0].selection_key, "CUSTOM1:X:0");
        assert_eq!(found.options[1].selection_key, "CUSTOM1:X:1");
        assert_eq!(found.options[2].selection_key, "CUSTOM2");

        // The discovered identity drives the existing preview end to end.
        let config = root.path().join("Game.options");
        let preview = build_whdload_trainer_preview(&WhdloadTrainerRequest {
            identity: found.identity.clone().unwrap(),
            configuration_path: config,
            staging_root: root.path().join("staging"),
            declarations: found.declarations.clone(),
            selections: vec![
                TrainerOptionSelection {
                    custom_slot: found.options[0].selection_key.clone(),
                    value: Some(TrainerOptionValue::Boolean(true)),
                },
                TrainerOptionSelection {
                    custom_slot: found.options[1].selection_key.clone(),
                    value: Some(TrainerOptionValue::Boolean(true)),
                },
            ],
            expected_configuration_sha256: None,
        })
        .unwrap();
        assert_eq!(preview.rendered_arguments, vec!["CUSTOM1=3"]);
        assert_eq!(fs::read(&slave).unwrap(), bytes, "slave is never modified");
    }

    #[test]
    fn unverified_ambiguous_missing_hdf_and_optionless_slaves_are_honest() {
        let root = tempfile::tempdir().unwrap();
        let slave = root.path().join("Game.slave");
        let bytes = synthetic_slave(CONFIG);
        fs::write(&slave, &bytes).unwrap();

        let unverified =
            discover_whdload_trainer_options(&inspection(vec![host(&slave, &bytes)], false), b"");
        assert_eq!(
            unverified.state,
            WhdloadTrainerDiscoveryState::IdentityUnverified
        );
        assert_eq!(unverified.options.len(), 4, "options stay visible");
        assert!(unverified.identity.is_none());
        assert!(!unverified.state.can_preview());

        let none = discover_whdload_trainer_options(&inspection(Vec::new(), true), b"");
        assert_eq!(none.state, WhdloadTrainerDiscoveryState::NoSlave);

        let other = root.path().join("Other.slave");
        fs::write(&other, &bytes).unwrap();
        let ambiguous = discover_whdload_trainer_options(
            &inspection(vec![host(&slave, &bytes), host(&other, &bytes)], true),
            b"",
        );
        assert_eq!(
            ambiguous.state,
            WhdloadTrainerDiscoveryState::AmbiguousSlaves
        );
        assert_eq!(ambiguous.candidates.len(), 2);
        assert!(ambiguous.options.is_empty() && ambiguous.identity.is_none());

        let mut in_hdf = host(Path::new("Game.slave"), &bytes);
        in_hdf.from_hdf = true;
        in_hdf.provenance_path = Some("/images/Game.hdf:DH0/Game.slave".into());
        let hdf = discover_whdload_trainer_options(&inspection(vec![in_hdf], true), b"");
        assert_eq!(hdf.state, WhdloadTrainerDiscoveryState::SlaveInsideHdf);
        assert_eq!(hdf.declarations.len(), 4);
        assert!(hdf.identity.is_none());

        let plain = root.path().join("Plain.slave");
        let plain_bytes = synthetic_slave("BW;");
        fs::write(&plain, &plain_bytes).unwrap();
        let optionless = discover_whdload_trainer_options(
            &inspection(vec![host(&plain, &plain_bytes)], true),
            b"",
        );
        assert_eq!(
            optionless.state,
            WhdloadTrainerDiscoveryState::NoTrainerOptions
        );
        assert!(optionless.identity.is_none());
    }

    #[test]
    fn changed_or_symlinked_slave_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let slave = root.path().join("Game.slave");
        let bytes = synthetic_slave(CONFIG);
        fs::write(&slave, &bytes).unwrap();
        let candidate = host(&slave, &bytes);
        fs::write(&slave, synthetic_slave("C1:B:Different;")).unwrap();
        let changed = discover_whdload_trainer_options(&inspection(vec![candidate], true), b"");
        assert_eq!(changed.state, WhdloadTrainerDiscoveryState::SlaveChanged);
        assert!(changed.options.is_empty() && changed.identity.is_none());

        let link = root.path().join("Link.slave");
        let real = root.path().join("Real.slave");
        fs::write(&real, &bytes).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let linked =
            discover_whdload_trainer_options(&inspection(vec![host(&link, &bytes)], true), b"");
        assert_eq!(linked.state, WhdloadTrainerDiscoveryState::SlaveUnreadable);
    }

    #[test]
    fn current_values_come_from_the_supplied_configuration() {
        let root = tempfile::tempdir().unwrap();
        let slave = root.path().join("Game.slave");
        let bytes = synthetic_slave(CONFIG);
        fs::write(&slave, &bytes).unwrap();
        let found = discover_whdload_trainer_options(
            &inspection(vec![host(&slave, &bytes)], true),
            b"CUSTOM1=2\nCUSTOM2=1\n",
        );
        assert_eq!(
            found.options[0].current_value,
            Some(TrainerOptionValue::Boolean(false))
        );
        assert_eq!(
            found.options[1].current_value,
            Some(TrainerOptionValue::Boolean(true))
        );
        assert_eq!(
            found.options[2].current_value,
            Some(TrainerOptionValue::Enum("2".into()))
        );
    }
}
