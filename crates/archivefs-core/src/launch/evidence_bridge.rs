//! The narrow, pure bridge from EmuWiz's existing authoritative identity/
//! content evidence to the data shapes [`crate::launch::planning`] already
//! requires.
//!
//! [`planning::build_launch_plan`] is deliberately generic over
//! [`planning::CanonicalIdentityStatus`] and [`planning::LaunchContentRef`]
//! rather than over any one identity/content evidence type this crate
//! produces - see that module's own doc comment. Something still has to
//! turn [`crate::game_identity::GameIdentityReport`] and
//! [`crate::ArchiveRecord`] (the structures the rest of EmuWiz already
//! trusts) into those two shapes, and that conversion belongs in core, not
//! guessed field-by-field in the GUI. This module is exactly that
//! conversion and nothing else.
//!
//! # What this module is not
//!
//! - It never resolves, fuses, or widens identity itself - every fact it
//!   emits already carried [`crate::game_identity::IdentityStatus::Verified`]
//!   on the source report. It only *routes* what the identity layer already
//!   decided into the launch planner's vocabulary.
//! - It never mounts an archive, reads a file, or inspects an inner member.
//!   [`launch_content_ref_from_archive_record`] takes an already-resolved
//!   inner member path as a plain `Option<&Path>` parameter when the caller
//!   has one; this module never derives one itself.
//! - It never starts a process, builds a RetroArch command line, or writes
//!   an ES-DE export - see [`crate::launch::retroarch_command`] and
//!   [`crate::launch::es_de_export`] for those, both entirely untouched by
//!   this module.
//!
//! # Why [`GameIdentityReport`] rather than [`crate::platform_evidence_fusion::identity_orchestrator::IdentityResult`]
//!
//! [`planning::ResolvedIdentity`] needs both a platform id and an opaque,
//! already-verified per-game key (a serial, a disc/title ID, a content
//! hash). [`GameIdentityReport`] is exactly that shape: a per-file report
//! whose [`IdentityEvidence`] entries already carry a
//! [`IdentityStatus::Verified`]/[`IdentityConfidence`] pair per candidate
//! key, gated by the same identity layer the rest of the codebase already
//! trusts. `IdentityResult` (`platform_evidence_fusion::identity_orchestrator`)
//! resolves a different, upstream question - *which platform* a bag of raw
//! structural evidence belongs to - and never produces an opaque per-game
//! key at all, so it cannot by itself populate [`ResolvedIdentity::game_key`].
//! Nothing here rules out a future caller using `IdentityResult` to help
//! decide *whether* to trust a `GameIdentityReport` in the first place; that
//! is a caller-side decision this bridge does not need to make to stay
//! narrow and honest about what it is given.

use std::path::{Path, PathBuf};

use crate::game_identity::{
    GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityKind, IdentityPlatform,
    IdentityStatus,
};
use crate::launch::input_projection::VerifiedIdentityFact;
use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentKind, LaunchContentRef,
    ResolvedIdentity,
};
use crate::{ArchiveKind, ArchiveRecord, MountState};

// ---------------------------------------------------------------------------
// Identity conversion
// ---------------------------------------------------------------------------

/// Which [`IdentityKind`] values are genuine, opaque per-game identity
/// facts - as opposed to display/format metadata that happens to also carry
/// [`IdentityStatus::Verified`] (see [`IdentityKind::LooseRomTitle`]'s own
/// doc comment: "not content identity"). Only kinds in this list can ever
/// contribute to [`ResolvedIdentity::game_key`] or a [`VerifiedIdentityFact`] -
/// this is the one place that distinction is made, so nothing downstream
/// has to re-derive it.
pub(crate) fn is_identity_conferring(kind: IdentityKind) -> bool {
    matches!(
        kind,
        IdentityKind::Ps1Serial
            | IdentityKind::Ps2Serial
            | IdentityKind::PspDiscId
            | IdentityKind::Ps3TitleId
            | IdentityKind::SaturnProductNumber
            | IdentityKind::DreamcastProductCode
            | IdentityKind::SegaCdProductCode
            | IdentityKind::Pcsx2ExecutableCrc
            | IdentityKind::DolphinGameId
            | IdentityKind::LooseRomSha256
            | IdentityKind::LooseRomCanonicalSha256
            | IdentityKind::XbeTitleId
            | IdentityKind::XexTitleId
            | IdentityKind::XexMediaId
            | IdentityKind::ScummVmGameId
            | IdentityKind::ThreeDoDiscId
            | IdentityKind::PcfxDiscHash
    )
}

/// Every genuinely verified, identity-conferring `(kind, value)` pair on
/// `report` - filtered to [`IdentityStatus::Verified`] status,
/// [`is_identity_conferring`] kinds, and never
/// [`IdentityConfidence::FilenameOnly`] confidence (defense in depth: no
/// current detector marks an identity-conferring kind `Verified` at
/// `FilenameOnly` confidence, but this bridge never trusts that invariant
/// silently - a filename can never promote itself into verified identity
/// here, regardless of what status a caller-supplied report claims).
fn verified_identity_values(report: &GameIdentityReport) -> Vec<(IdentityKind, String)> {
    report
        .evidence
        .iter()
        .filter(|evidence: &&IdentityEvidence| {
            evidence.status == IdentityStatus::Verified
                && evidence.confidence != IdentityConfidence::FilenameOnly
                && is_identity_conferring(evidence.kind)
        })
        .filter_map(|evidence| Some((evidence.kind, evidence.value.clone()?)))
        .collect()
}

/// Whether `resolved` contains two different verified values for the same
/// [`IdentityKind`] - the one shape of genuine, provable conflict this
/// bridge can see without fabricating a broader heuristic: the identity
/// layer itself asserted two incompatible `Verified` answers to the same
/// question.
fn has_conflicting_values(resolved: &[(IdentityKind, String)]) -> bool {
    resolved.iter().any(|(kind, value)| {
        resolved
            .iter()
            .any(|(other_kind, other_value)| other_kind == kind && other_value != value)
    })
}

fn find_value(resolved: &[(IdentityKind, String)], kind: IdentityKind) -> Option<&str> {
    resolved
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .map(|(_, value)| value.as_str())
}

/// Matches [`crate::platform::Platform::id`]'s existing values for exactly
/// the platforms [`GameIdentityReport`] can identify - never invented here.
fn launch_platform_id(platform: IdentityPlatform) -> Option<&'static str> {
    match platform {
        IdentityPlatform::PlayStation => Some("PSX"),
        IdentityPlatform::PlayStation2 => Some("PS2"),
        IdentityPlatform::Psp => Some("PSP"),
        IdentityPlatform::PlayStation3 => Some("PS3"),
        IdentityPlatform::Saturn => Some("Saturn"),
        IdentityPlatform::Dreamcast => Some("Dreamcast"),
        IdentityPlatform::SegaCd => Some("Sega CD"),
        IdentityPlatform::GameCube => Some("GameCube"),
        IdentityPlatform::Wii => Some("Wii"),
        IdentityPlatform::MegaDrive => Some("MegaDrive"),
        IdentityPlatform::Snes => Some("SNES"),
        IdentityPlatform::Nes => Some("NES"),
        IdentityPlatform::GameBoy => Some("Game Boy"),
        IdentityPlatform::GameBoyColor => Some("Game Boy Color"),
        IdentityPlatform::GameBoyAdvance => Some("Game Boy Advance"),
        IdentityPlatform::VirtualBoy => Some("Virtual Boy"),
        IdentityPlatform::N64 => Some("N64"),
        IdentityPlatform::Commodore64 | IdentityPlatform::Vic20 => None,
        IdentityPlatform::Xbox => Some("Xbox"),
        IdentityPlatform::Xbox360 => Some("Xbox360"),
        IdentityPlatform::ScummVM => Some("ScummVM"),
        IdentityPlatform::ThreeDo => Some("3DO"),
        IdentityPlatform::Pcfx => Some("PC-FX"),
        IdentityPlatform::PcEngineCd => Some("PC Engine CD"),
        IdentityPlatform::NeoGeoCd => Some("Neo Geo CD"),
        IdentityPlatform::Ngp => Some("Neo Geo Pocket"),
        IdentityPlatform::Ngpc => Some("Neo Geo Pocket Color"),
        IdentityPlatform::Atari2600 => Some("Atari2600"),
        IdentityPlatform::Atari5200 => Some("Atari5200"),
        IdentityPlatform::Atari7800 => Some("Atari7800"),
        IdentityPlatform::Atari8Bit => Some("Atari 8-bit"),
        IdentityPlatform::AtariLynx => Some("Atari Lynx"),
        IdentityPlatform::AtariJaguar => Some("Atari Jaguar"),
        IdentityPlatform::AtariST => Some("AtariST"),
        // `report.platform` was never determined at all - there is no
        // platform id to hand `ResolvedIdentity` without inventing one.
        // Modern Nintendo platforms are catalogue foundations only; no
        // launch identity kind exists for them yet.
        IdentityPlatform::WiiU
        | IdentityPlatform::ThreeDS
        | IdentityPlatform::Switch
        | IdentityPlatform::Other => None,
    }
}

/// Builds the `(platform_id, game_key, facts)` triple for one platform from
/// its already-verified `(kind, value)` pairs, or `None` when `platform`'s
/// own identity-conferring kind was never actually verified (e.g. a report
/// whose only verified evidence belongs to a different platform than
/// `report.platform` claims - should not happen given how
/// [`GameIdentityReport`] is built, but this bridge never assumes it and
/// fails closed to [`CanonicalIdentityStatus::Unknown`] instead of guessing).
fn resolved_identity_for_platform(
    platform: IdentityPlatform,
    resolved: &[(IdentityKind, String)],
) -> Option<(&'static str, String, Vec<VerifiedIdentityFact>)> {
    let platform_id = launch_platform_id(platform)?;
    match platform {
        IdentityPlatform::PlayStation => {
            let serial = find_value(resolved, IdentityKind::Ps1Serial)?;
            Some((
                platform_id,
                serial.to_string(),
                vec![VerifiedIdentityFact::Ps1Serial(serial.to_string())],
            ))
        }
        IdentityPlatform::PlayStation2 => {
            let serial = find_value(resolved, IdentityKind::Ps2Serial);
            let crc = find_value(resolved, IdentityKind::Pcsx2ExecutableCrc);
            let game_key = serial.or(crc)?.to_string();
            let mut facts = Vec::new();
            if let Some(serial) = serial {
                facts.push(VerifiedIdentityFact::Ps2Serial(serial.to_string()));
            }
            if let Some(crc) = crc {
                facts.push(VerifiedIdentityFact::Ps2ExecutableCrc(crc.to_string()));
            }
            Some((platform_id, game_key, facts))
        }
        IdentityPlatform::Psp => {
            let disc_id = find_value(resolved, IdentityKind::PspDiscId)?;
            Some((
                platform_id,
                disc_id.to_string(),
                vec![VerifiedIdentityFact::PspDiscId(disc_id.to_string())],
            ))
        }
        IdentityPlatform::PlayStation3 => {
            let title_id = find_value(resolved, IdentityKind::Ps3TitleId)?;
            Some((
                platform_id,
                title_id.to_string(),
                vec![VerifiedIdentityFact::Ps3TitleId(title_id.to_string())],
            ))
        }
        IdentityPlatform::Saturn => {
            let product_number = find_value(resolved, IdentityKind::SaturnProductNumber)?;
            Some((
                platform_id,
                product_number.to_string(),
                vec![VerifiedIdentityFact::SaturnProductCode(
                    product_number.to_string(),
                )],
            ))
        }
        IdentityPlatform::Dreamcast => {
            let product_code = find_value(resolved, IdentityKind::DreamcastProductCode)?;
            Some((
                platform_id,
                product_code.to_string(),
                vec![VerifiedIdentityFact::DreamcastProductCode(
                    product_code.to_string(),
                )],
            ))
        }
        IdentityPlatform::SegaCd => {
            let product_code = find_value(resolved, IdentityKind::SegaCdProductCode)?;
            Some((
                platform_id,
                product_code.to_string(),
                vec![VerifiedIdentityFact::SegaCdProductCode(
                    product_code.to_string(),
                )],
            ))
        }
        IdentityPlatform::GameCube => {
            let game_id = find_value(resolved, IdentityKind::DolphinGameId)?;
            Some((
                platform_id,
                game_id.to_string(),
                vec![VerifiedIdentityFact::GameCubeGameId(game_id.to_string())],
            ))
        }
        IdentityPlatform::Wii => {
            let game_id = find_value(resolved, IdentityKind::DolphinGameId)?;
            Some((
                platform_id,
                game_id.to_string(),
                vec![VerifiedIdentityFact::WiiGameId(game_id.to_string())],
            ))
        }
        IdentityPlatform::MegaDrive
        | IdentityPlatform::Snes
        | IdentityPlatform::Nes
        | IdentityPlatform::GameBoy
        | IdentityPlatform::GameBoyColor
        | IdentityPlatform::GameBoyAdvance
        | IdentityPlatform::VirtualBoy
        | IdentityPlatform::Ngp
        | IdentityPlatform::Ngpc => {
            // A verified full-file SHA-256 is a genuine opaque game key, but
            // `VerifiedIdentityFact` has no cartridge-hash variant - no
            // adapter-request projector reads a generic hash today, so
            // fabricating one here would be an unused, unverifiable variant.
            // The resolved identity itself is still real and reported.
            let sha256 = find_value(resolved, IdentityKind::LooseRomSha256)?;
            Some((platform_id, sha256.to_string(), Vec::new()))
        }
        IdentityPlatform::N64 => {
            // Prefer the byte-order-normalized canonical hash as the game
            // key when it exists: unlike the generic loose-ROM platforms
            // above, N64 dumps of the *same* game legitimately differ in
            // physical byte order (z64/v64/n64), and using the physical
            // hash here would wrongly treat those as different games. Falls
            // back to the physical hash when the header couldn't be
            // recognized/normalized (see `push_n64_canonical_evidence`) -
            // the resolved identity is still real, just not order-
            // independent in that case. No `VerifiedIdentityFact` variant
            // exists for either generic cartridge hash, so facts stay empty
            // exactly like the group above.
            let canonical = find_value(resolved, IdentityKind::LooseRomCanonicalSha256);
            let physical = find_value(resolved, IdentityKind::LooseRomSha256);
            let game_key = canonical.or(physical)?.to_string();
            Some((platform_id, game_key, Vec::new()))
        }
        IdentityPlatform::Xbox360 => {
            // `VerifiedIdentityFact::XboxTitleId` names the original Xbox,
            // not the 360 - a distinct platform this bridge must not
            // conflate. No 360-specific variant exists, so facts stay empty
            // even though the resolved identity itself is real.
            let title_id = find_value(resolved, IdentityKind::XexTitleId);
            let media_id = find_value(resolved, IdentityKind::XexMediaId);
            let game_key = title_id.or(media_id)?.to_string();
            Some((platform_id, game_key, Vec::new()))
        }
        IdentityPlatform::Xbox => {
            // The one platform this bridge conflates least: `IdentityKind::XbeTitleId`
            // is Xbox-only (see its own doc comment), and
            // `VerifiedIdentityFact::XboxTitleId` genuinely names this
            // platform - unlike Xbox 360 above, a real fact variant exists.
            let title_id = find_value(resolved, IdentityKind::XbeTitleId)?;
            Some((
                platform_id,
                title_id.to_string(),
                vec![VerifiedIdentityFact::XboxTitleId(title_id.to_string())],
            ))
        }
        IdentityPlatform::ScummVM => {
            let game_id = find_value(resolved, IdentityKind::ScummVmGameId)?;
            Some((
                platform_id,
                game_id.to_string(),
                vec![VerifiedIdentityFact::ScummVmGameId(game_id.to_string())],
            ))
        }
        IdentityPlatform::ThreeDo => {
            let disc_id = find_value(resolved, IdentityKind::ThreeDoDiscId)?;
            Some((
                platform_id,
                disc_id.to_string(),
                vec![VerifiedIdentityFact::ThreeDoDiscId(disc_id.to_string())],
            ))
        }
        IdentityPlatform::Pcfx => {
            let disc_hash = find_value(resolved, IdentityKind::PcfxDiscHash)?;
            Some((
                platform_id,
                disc_hash.to_string(),
                vec![VerifiedIdentityFact::PcfxDiscHash(disc_hash.to_string())],
            ))
        }
        IdentityPlatform::Atari2600
        | IdentityPlatform::Atari5200
        | IdentityPlatform::Atari7800
        | IdentityPlatform::Atari8Bit
        | IdentityPlatform::AtariLynx
        | IdentityPlatform::AtariJaguar => {
            let sha256 = find_value(resolved, IdentityKind::LooseRomSha256)?;
            Some((platform_id, sha256.to_string(), Vec::new()))
        }
        IdentityPlatform::AtariST => None,
        // PC Engine CD's IPL boot-record carries no serial/title, so there
        // is no resolvable game key here - exact identity is DAT/hash-driven.
        // The platform is still known (via `launch_platform_id`), so a
        // launcher can pick an emulator; identity simply stays `Unknown`.
        IdentityPlatform::PcEngineCd => None,
        // Neo Geo CD's IPL.TXT load manifest carries no serial/title, so
        // there is no resolvable game key here - exact identity is
        // DAT/hash-driven. The platform is still known (via
        // `launch_platform_id`), so a launcher can pick an emulator;
        // identity simply stays `Unknown`.
        IdentityPlatform::NeoGeoCd => None,
        // These catalogue platforms have no structural identity parser or
        // launch identity kind yet, so they remain deliberately unresolved.
        IdentityPlatform::WiiU
        | IdentityPlatform::ThreeDS
        | IdentityPlatform::Switch
        | IdentityPlatform::Commodore64
        | IdentityPlatform::Vic20 => None,
        IdentityPlatform::Other => None,
    }
}

/// Converts `report` - the existing authoritative per-file identity report -
/// into the [`CanonicalIdentityStatus`]/[`VerifiedIdentityFact`] shapes
/// [`crate::launch::planning::build_launch_plan`] and
/// [`crate::launch::input_projection`] already require.
///
/// - At least one genuinely verified, identity-conferring fact for
///   `report.platform`, with no conflicting value for the same fact kind ->
///   [`CanonicalIdentityStatus::Resolved`] plus every [`VerifiedIdentityFact`]
///   an adapter projector can read (may be empty for a platform with no
///   matching fact variant - see [`resolved_identity_for_platform`]'s own
///   notes).
/// - Two different verified values for the same identity-conferring kind ->
///   [`CanonicalIdentityStatus::Conflicting`] - the identity layer asserted
///   two incompatible answers, never silently resolved to one winner.
/// - No genuinely verified, identity-conferring evidence at all (missing,
///   ambiguous, candidate-only, or `report.platform` never determined) ->
///   [`CanonicalIdentityStatus::Unknown`].
///
/// Never promotes [`IdentityKind::LooseRomTitle`]/[`IdentityKind::LooseRomFormat`]
/// (filename/extension-derived display metadata, `Verified` status
/// notwithstanding - see their own construction site's doc comment) or any
/// [`IdentityConfidence::FilenameOnly`] evidence into identity.
pub fn canonical_identity_from_game_report(
    report: &GameIdentityReport,
) -> (CanonicalIdentityStatus, Vec<VerifiedIdentityFact>) {
    let resolved = verified_identity_values(report);
    if resolved.is_empty() {
        return (CanonicalIdentityStatus::Unknown, Vec::new());
    }
    if has_conflicting_values(&resolved) {
        return (CanonicalIdentityStatus::Conflicting, Vec::new());
    }
    match resolved_identity_for_platform(report.platform, &resolved) {
        Some((platform_id, game_key, facts)) => (
            CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                platform_id: platform_id.to_string(),
                game_key,
            }),
            facts,
        ),
        None => (CanonicalIdentityStatus::Unknown, Vec::new()),
    }
}

// ---------------------------------------------------------------------------
// Content conversion
// ---------------------------------------------------------------------------

/// Converts `record` - the existing authoritative archive/mount state - into
/// a [`LaunchContentRef`].
///
/// `resolved_member_path` is the exact, already-resolved runnable path
/// inside a mounted archive, when a caller genuinely has one (this bridge
/// never derives, guesses, or reads one itself - see the module doc
/// comment). It is only ever honored when `record.mount_state` is actually
/// [`MountState::Mounted`]; a caller-supplied path against a record that is
/// not actually mounted is treated as if none had been supplied, never
/// trusted at face value.
///
/// - `record`'s archive is loose/direct content
///   ([`ArchiveKind::is_mount_input`] is `false` - a [`ArchiveKind::DirectGameImage`]
///   or [`ArchiveKind::MegaDriveRom`]) -> `record`'s own path *is* the exact
///   runnable content path; `resolved_path` is `Some`, `requires_mount` is
///   `false`.
/// - `record`'s archive is a container (zip/7z/rar) -> `requires_mount` is
///   always `true`, and the *outer* archive path is never used as
///   `resolved_path` - only `resolved_member_path`, and only when the
///   record is actually [`MountState::Mounted`]. A mounted archive with no
///   resolved inner member, or a container that has not been mounted at
///   all, both leave `resolved_path` `None` - honestly unresolved, not
///   silently pointed at the container itself.
pub fn launch_content_ref_from_archive_record(
    record: &ArchiveRecord,
    resolved_member_path: Option<&Path>,
) -> LaunchContentRef {
    let archive = &record.mount_plan.archive;

    if !archive.kind.is_mount_input() {
        let kind = match archive.kind {
            ArchiveKind::MegaDriveRom => Some(LaunchContentKind::Cartridge),
            ArchiveKind::DirectGameImage
                if archive
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("vb")
                            || extension.eq_ignore_ascii_case("vboy")
                    }) =>
            {
                Some(LaunchContentKind::Cartridge)
            }
            // `DirectGameImage` covers more than one real platform/format;
            // this bridge does not know which without guessing.
            _ => None,
        };
        return LaunchContentRef {
            kind,
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(archive.path.clone()),
            requires_mount: false,
            provenance: "loose/direct content: the archive record's own path is the runnable \
                         file"
                .to_string(),
        };
    }

    let genuinely_mounted = record.mount_state == MountState::Mounted;
    let resolved_path: Option<PathBuf> = resolved_member_path
        .filter(|_| genuinely_mounted)
        .map(Path::to_path_buf);

    let provenance = match (genuinely_mounted, resolved_path.is_some()) {
        (true, true) => {
            "archive is mounted and a specific inner member has been resolved".to_string()
        }
        (true, false) => {
            "archive is mounted, but no specific inner member has been resolved yet".to_string()
        }
        (false, _) => "archive container has not been mounted".to_string(),
    };

    LaunchContentRef {
        kind: None,
        container: Some(LaunchContainerKind::Archive),
        resolved_path,
        requires_mount: true,
        provenance,
    }
}

#[cfg(test)]
mod tests;
