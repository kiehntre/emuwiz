//! Read-only adapters over existing firmware inspectors. No discovery, I/O,
//! profile selection, emulator execution or Launch decision is performed here.
//! Recovered from dd2e4a18, with explicit binding and contradiction checks.
use crate::dat::firmware_evidence::{FirmwareIdentityRecord, FirmwareSystem};
use crate::game_firmware_readiness::*;
use crate::launch::{FirmwareReadiness, LaunchCandidate, LaunchTarget};
use crate::patch_manager::*;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterFirmwareContext {
    pub game_id: Option<String>,
    pub platform: String,
    pub release_identity: Option<String>,
    pub game_region: Option<String>,
    pub game_revision: Option<String>,
    pub freshness: FirmwareFreshnessSnapshot,
}
fn item(
    path: Option<PathBuf>,
    status: FirmwareEvidenceStatus,
    strength: FirmwareEvidenceStrength,
    provenance: String,
) -> FirmwareEvidenceSummary {
    FirmwareEvidenceSummary {
        option_id: None,
        filename: path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(str::to_owned),
        path,
        sha256: None,
        strength,
        status,
        region: None,
        version: None,
        provenance,
    }
}
fn note(text: impl Into<String>) -> FirmwareEvidenceSummary {
    item(
        None,
        FirmwareEvidenceStatus::Unknown,
        FirmwareEvidenceStrength::FilenameOrPresenceOnly,
        text.into(),
    )
}
fn requirement(id: &str, label: &str) -> FirmwareRequirementDescriptor {
    FirmwareRequirementDescriptor {
        id: id.into(),
        label: label.into(),
        required: true,
        options: vec![],
        emulator_managed: false,
        region_requirement_proven: false,
        version_requirement_proven: false,
    }
}
fn bound(
    candidate: &LaunchCandidate,
    adapter: &str,
    profile_id: &str,
    path: &std::path::Path,
) -> bool {
    matches!(&candidate.target,LaunchTarget::Standalone{adapter_id,profile_id:id,profile_path:Some(p)} if *adapter_id==adapter && id==profile_id && p==path)
}
fn project(
    ctx: &AdapterFirmwareContext,
    candidate: &LaunchCandidate,
    req: FirmwareRequirementDescriptor,
    mut evidence: Vec<FirmwareEvidenceSummary>,
    observed: FirmwareReadiness,
    binding: bool,
    platform: &str,
) -> GameFirmwareReadiness {
    // The temporary projection input describes the inspected evidence, not a
    // mutation to the authoritative Launch candidate. In particular a cached
    // candidate Verified cannot override a fresh missing/conflicting result.
    let mut snapshot = candidate.clone();
    snapshot.firmware = observed;
    let conflict =
        candidate.firmware != FirmwareReadiness::Unknown && candidate.firmware != observed;
    let platform_matches = crate::canonical_platform_for_alias(&ctx.platform) == Some(platform);
    if !binding || !platform_matches {
        evidence = vec![note(
            "Unknown: inspected emulator/profile/platform does not match the selected candidate. Evidence was not attributed to this game.",
        )];
    }
    if conflict {
        evidence.push(note("Unknown: the existing Launch firmware snapshot conflicts with this inspection. Recheck readiness; Launch decisions have not been changed."));
    }
    let mut result = GameFirmwareReadiness::project(GameFirmwareReadinessInput {
        game_id: ctx.game_id.clone(),
        platform: ctx.platform.clone(),
        release_identity: ctx.release_identity.clone(),
        game_region: ctx.game_region.clone(),
        game_revision: ctx.game_revision.clone(),
        candidate: &snapshot,
        requirement: req,
        evidence,
        selected_option_id: None,
        freshness: FirmwareFreshness::current(ctx.freshness.clone()),
    });
    if !binding || !platform_matches || conflict {
        result.state = GameFirmwareState::Unknown;
    }
    result
}
fn known_hash(
    path: PathBuf,
    record: &FirmwareIdentityRecord,
    size: u64,
    crc: &str,
    md5: &str,
    sha1: &str,
) -> FirmwareEvidenceSummary {
    item(
        Some(path),
        FirmwareEvidenceStatus::Accepted,
        FirmwareEvidenceStrength::ExactKnownHash,
        format!(
            "Exact local DAT match: {:?}; {}; DAT version {}; size {size}; CRC32 {crc}; MD5 {md5}; SHA-1 {sha1}. No emulator was executed.",
            record.provider,
            record.name,
            record.dat_version.as_deref().unwrap_or("unknown")
        ),
    )
}

pub fn pcsx2(
    ctx: &AdapterFirmwareContext,
    candidate: &LaunchCandidate,
    profile: &Pcsx2Profile,
    result: &Pcsx2BiosVerificationOutcome,
) -> GameFirmwareReadiness {
    let (state,ev)=match result {
        Pcsx2BiosVerificationOutcome::Verified(v) if v.record.system==FirmwareSystem::PlayStation2 =>(FirmwareReadiness::Verified,vec![known_hash(v.path.clone(),&v.record,v.size_bytes,&v.crc32,&v.md5,&v.sha1)]),
        Pcsx2BiosVerificationOutcome::Unknown{path}=>(FirmwareReadiness::PresentUnverified,vec![item(Some(path.clone()),FirmwareEvidenceStatus::Unknown,FirmwareEvidenceStrength::FilenameOrPresenceOnly,"Selected BIOS was read, but no loaded authoritative record matched. This does not prove corruption or a hash mismatch.".into())]),
        Pcsx2BiosVerificationOutcome::Missing=>(FirmwareReadiness::Missing,vec![note("No selected PS2 BIOS was found by the existing PCSX2 resolver.")]),
        other=>(FirmwareReadiness::Unknown,vec![note(format!("PCSX2 BIOS inspection is unresolved: {other:?}"))]),
    };
    project(
        ctx,
        candidate,
        requirement("pcsx2-bios", "PlayStation 2 BIOS"),
        ev,
        state,
        bound(
            candidate,
            "pcsx2",
            &profile.profile_id,
            &profile.configuration_path,
        ),
        "PS2",
    )
}
pub fn duckstation(
    ctx: &AdapterFirmwareContext,
    candidate: &LaunchCandidate,
    profile: &DuckStationProfile,
    result: &DuckStationBiosVerificationOutcome,
) -> GameFirmwareReadiness {
    let (state,mut ev)=match result {
        DuckStationBiosVerificationOutcome::Verified(v) if v.record.system==FirmwareSystem::PlayStation => {
            let mut e=known_hash(v.path.clone(),&v.record,v.size_bytes,&v.crc32,&v.md5,&v.sha1);
            e.provenance.push_str(&format!(" Configured region slot: {}. A slot is not proof of the dump's region compatibility.",v.required_region.unwrap_or("not established")));
            (FirmwareReadiness::Verified,vec![e])
        },
        DuckStationBiosVerificationOutcome::Unknown{path}=>(FirmwareReadiness::PresentUnverified,vec![item(Some(path.clone()),FirmwareEvidenceStatus::Unknown,FirmwareEvidenceStrength::FilenameOrPresenceOnly,"Selected PS1 BIOS has no matching loaded authoritative record; compatibility and corruption are not inferred.".into())]),
        DuckStationBiosVerificationOutcome::Missing=>(FirmwareReadiness::Missing,vec![note("No BIOS for the resolved DuckStation slot was found.")]),
        other=>(FirmwareReadiness::Unknown,vec![note(format!("DuckStation BIOS selection is unresolved (no arbitrary global/per-game choice): {other:?}"))]),
    };
    ev.push(note("No game-specific BIOS region/version restriction is invented from a title, serial prefix or BIOS filename."));
    project(
        ctx,
        candidate,
        requirement("duckstation-bios", "PlayStation BIOS"),
        ev,
        state,
        bound(
            candidate,
            "duckstation",
            &profile.profile_id,
            &profile.configuration_path,
        ),
        "PSX",
    )
}
pub fn flycast(
    ctx: &AdapterFirmwareContext,
    candidate: &LaunchCandidate,
    profile: &FlycastProfile,
    health: &FlycastSystemHealth,
) -> GameFirmwareReadiness {
    let verified = health.dreamcast_bios == FlycastSystemFileState::Verified
        && health.dreamcast_flash == FlycastSystemFileState::Verified;
    let missing =
        [health.dreamcast_bios, health.dreamcast_flash].contains(&FlycastSystemFileState::Missing);
    let unverified = [health.dreamcast_bios, health.dreamcast_flash]
        .iter()
        .all(|s| {
            matches!(
                s,
                FlycastSystemFileState::Verified | FlycastSystemFileState::PresentUnverified
            )
        });
    let state = if verified {
        FirmwareReadiness::Verified
    } else if missing {
        FirmwareReadiness::Missing
    } else if unverified {
        FirmwareReadiness::PresentUnverified
    } else {
        FirmwareReadiness::Unknown
    };
    let ev=[("Dreamcast BIOS",health.dreamcast_bios),("Dreamcast flash",health.dreamcast_flash)].into_iter().map(|(label,s)|item(None,if verified{FirmwareEvidenceStatus::Accepted}else{FirmwareEvidenceStatus::Unknown},if s==FlycastSystemFileState::Verified{FirmwareEvidenceStrength::ExactKnownHash}else{FirmwareEvidenceStrength::FilenameOrPresenceOnly},format!("Existing Flycast inspector: {label}: {s:?}. Both verified files are required; this is not NAOMI/arcade firmware evidence."))).collect();
    project(
        ctx,
        candidate,
        requirement("flycast-dreamcast-system", "Dreamcast BIOS and flash"),
        ev,
        state,
        bound(
            candidate,
            "flycast",
            &profile.profile_id,
            &profile.configuration_path,
        ),
        "Dreamcast",
    )
}
pub fn hatari(
    ctx: &AdapterFirmwareContext,
    candidate: &LaunchCandidate,
    profile: &HatariProfile,
    health: &HatariHealth,
) -> GameFirmwareReadiness {
    let state = crate::launch::hatari_firmware_readiness(health.tos.health);
    let mut ev = item(
        health.tos.path.clone(),
        if health.tos.health == HatariTosHealth::Verified {
            FirmwareEvidenceStatus::Accepted
        } else {
            FirmwareEvidenceStatus::Unknown
        },
        if health.tos.health == HatariTosHealth::Verified {
            FirmwareEvidenceStrength::ExactKnownHash
        } else {
            FirmwareEvidenceStrength::FilenameOrPresenceOnly
        },
        format!(
            "Existing Hatari TOS inspector: {:?}. ROM region/version are observations, not proven requirements of this game. Machine/TOS game compatibility remains unknown.",
            health.tos.health
        ),
    );
    ev.sha256 = health.tos.sha256.clone();
    ev.region = health.tos.region.clone();
    ev.version = health.tos.version.clone();
    project(
        ctx,
        candidate,
        requirement("hatari-tos", "Atari ST TOS ROM"),
        vec![ev],
        state,
        bound(
            candidate,
            "hatari",
            &profile.profile_id,
            &profile.config_path,
        ),
        "AtariST",
    )
}
/// The caller supplies the exact target from the existing resolver; no policy
/// is inferred from an emulator's name, a firmware filename or a global pool.
pub fn pc_engine_cd(
    ctx: &AdapterFirmwareContext,
    candidate: &LaunchCandidate,
    inspected_target: &LaunchTarget,
    policy: PceCdEmulatorFirmwarePolicy,
    status: PceCdFirmwareReadiness,
) -> GameFirmwareReadiness {
    let mut req = requirement("pce-cd-system-card", "PC Engine CD System Card");
    let mut observed = crate::launch::pcengine_cd_firmware_readiness(status);
    let consistent = match policy {
        PceCdEmulatorFirmwarePolicy::Unknown => false,
        PceCdEmulatorFirmwarePolicy::EmbedsSystemCard => {
            status == PceCdFirmwareReadiness::EmulatorProvidesFirmware
        }
        PceCdEmulatorFirmwarePolicy::RequiresExternalSystemCard => {
            status != PceCdFirmwareReadiness::EmulatorProvidesFirmware
        }
    };
    if policy == PceCdEmulatorFirmwarePolicy::EmbedsSystemCard && consistent {
        req.required = false;
    }
    if !consistent
        || matches!(
            status,
            PceCdFirmwareReadiness::VerifiedButRequirementUnknown { .. }
        )
    {
        observed = FirmwareReadiness::Unknown;
    }
    let ev = vec![item(
        None,
        if observed == FirmwareReadiness::Verified {
            FirmwareEvidenceStatus::Accepted
        } else {
            FirmwareEvidenceStatus::Unknown
        },
        if observed == FirmwareReadiness::Verified {
            FirmwareEvidenceStrength::ExactKnownHash
        } else {
            FirmwareEvidenceStrength::FilenameOrPresenceOnly
        },
        format!(
            "Existing PC Engine CD resolver: {}. Emulator policy: {policy:?}. No per-title requirement is inferred.",
            status.summary()
        ),
    )];
    let binding = consistent
        && &candidate.target == inspected_target
        && matches!(
            candidate.content.kind,
            Some(crate::launch::LaunchContentKind::OpticalDisc)
        );
    project(ctx, candidate, req, ev, observed, binding, "PC Engine CD")
}
#[cfg(test)]
mod tests;
