//! Read-only save-state guidance recovered from 6cc05638, with conservative evidence rules.
//! Inventory paths describe where a state was found, not which build created it.
use crate::game_identity::{IdentityEvidence, IdentityKind, IdentityStatus};
use crate::persistent_state_inventory::{
    PersistentStateRecord, PersistentStateType, StateEmulator,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStateCompatibility {
    Supported,
    Incompatible,
    Unknown,
}

/// Capture metadata must come from the state or trustworthy capture provenance,
/// never from the installation currently owning its directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStateCapture {
    pub emulator: StateEmulator,
    pub version: Option<String>,
    pub core: Option<String>,
    pub core_version: Option<String>,
    pub profile: Option<String>,
    pub game: Vec<IdentityEvidence>,
    pub state_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStateCompatibilityTarget {
    pub emulator: StateEmulator,
    pub version: Option<String>,
    pub core: Option<String>,
    pub core_version: Option<String>,
    pub profile: Option<String>,
    pub game: Vec<IdentityEvidence>,
}

/// An adapter must explicitly validate format support and its version contract.
/// No production inventory adapter currently supplies Supported evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveStateFormatEvidence {
    Unknown,
    Unsupported(String),
    /// Verified format/version contract for this exact capture and target.
    /// Matching metadata alone is insufficient. The source explains the contract.
    Supported {
        capture: SaveStateCapture,
        target: SaveStateCompatibilityTarget,
        source: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStateAssessment {
    pub compatibility: SaveStateCompatibility,
    pub reasons: Vec<String>,
}

/// Only game-identifying fields count; platform, region, format, revision,
/// title text and filename candidates cannot establish game ownership.
pub fn game_identity_kind(kind: IdentityKind) -> bool {
    matches!(
        kind,
        IdentityKind::Ps1Serial
            | IdentityKind::Ps2Serial
            | IdentityKind::PspDiscId
            | IdentityKind::Ps3TitleId
            | IdentityKind::Ps4TitleId
            | IdentityKind::Ps4ContentId
            | IdentityKind::SaturnProductNumber
            | IdentityKind::DreamcastProductCode
            | IdentityKind::SegaCdProductCode
            | IdentityKind::Pcsx2ExecutableCrc
            | IdentityKind::DolphinGameId
            | IdentityKind::LooseRomSha256
            | IdentityKind::LooseRomCanonicalSha256
            | IdentityKind::MameMachineName
            | IdentityKind::XbeTitleId
            | IdentityKind::XexTitleId
            | IdentityKind::XexMediaId
            | IdentityKind::AmigaWHDLoad
            | IdentityKind::ScummVmGameId
            | IdentityKind::ThreeDoDiscId
    )
}

fn verified(e: &IdentityEvidence) -> bool {
    e.status == IdentityStatus::Verified
        && !matches!(
            e.confidence,
            crate::game_identity::IdentityConfidence::FilenameOnly
                | crate::game_identity::IdentityConfidence::Unavailable
        )
        && e.value.as_ref().is_some_and(|s| !s.trim().is_empty())
}

pub fn ambiguous_identity(items: &[IdentityEvidence]) -> bool {
    items.iter().any(|a| {
        matches!(
            a.status,
            IdentityStatus::Ambiguous | IdentityStatus::Invalid
        ) || (verified(a)
            && items
                .iter()
                .any(|b| verified(b) && a.kind == b.kind && a.value != b.value))
    })
}

/// Requires one title/content identity and rejects any shared conflicting fact.
pub fn same_game(a: &[IdentityEvidence], b: &[IdentityEvidence]) -> bool {
    !ambiguous_identity(a)
        && !ambiguous_identity(b)
        && a.iter().any(|x| {
            verified(x)
                && game_identity_kind(x.kind)
                && b.iter()
                    .any(|y| verified(y) && x.kind == y.kind && x.value == y.value)
        })
        && !a.iter().any(|x| {
            verified(x)
                && b.iter()
                    .any(|y| verified(y) && x.kind == y.kind && x.value != y.value)
        })
}

pub fn assess_save_state_compatibility(
    record: &PersistentStateRecord,
    capture: Option<&SaveStateCapture>,
    target: Option<&SaveStateCompatibilityTarget>,
    format: &SaveStateFormatEvidence,
    current_sha256: Option<&str>,
) -> SaveStateAssessment {
    let mut reasons = Vec::new();
    let mut incompatible = false;
    if record.state_type != PersistentStateType::SaveState {
        reasons.push("Ordinary game saves and memory cards use separate compatibility rules; this assessment applies only to emulator snapshots.".into());
        return SaveStateAssessment {
            compatibility: SaveStateCompatibility::Unknown,
            reasons,
        };
    }
    if !record.warnings.is_empty() {
        reasons.push("Inventory warnings need review; refresh the save locations.".into());
    }
    if current_sha256.is_none() {
        reasons.push("State file is missing, unreadable or not freshly checked. Refresh save locations before relying on this inventory.".into());
    }
    if record.sha256.as_deref() != current_sha256 || record.sha256.is_none() {
        reasons.push("State freshness is unproven or the file changed since inspection.".into());
    }
    if ambiguous_identity(&record.game_identity) {
        reasons.push("Inventory game identity is conflicting or ambiguous.".into());
    }
    let Some(capture) = capture else {
        reasons.push("Capture provenance is unavailable: the folder's emulator, version and profile identify its current location, not the build or core that created this state.".into());
        if target.is_none() {
            reasons.push("Select a game and review its current Launch readiness to establish the selected emulator/profile.".into());
        }
        return SaveStateAssessment {
            compatibility: SaveStateCompatibility::Unknown,
            reasons,
        };
    };
    let hash_ok = current_sha256.is_some_and(|hash| {
        hash.len() == 64
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            && hash == capture.state_sha256
    });
    if !hash_ok {
        reasons.push("Capture provenance is stale or unbound to the current state bytes; emulator/core claims cannot be trusted.".into());
        return SaveStateAssessment {
            compatibility: SaveStateCompatibility::Unknown,
            reasons,
        };
    }
    if ambiguous_identity(&capture.game) {
        reasons.push("Capture game identity is conflicting or ambiguous.".into());
    }
    let Some(target) = target else {
        reasons.push(
            "No unambiguous current emulator selection is available. Review Launch readiness."
                .into(),
        );
        return SaveStateAssessment {
            compatibility: SaveStateCompatibility::Unknown,
            reasons,
        };
    };
    if ambiguous_identity(&target.game) {
        reasons.push("Selected game identity is conflicting or ambiguous.".into());
    }
    if !same_game(&capture.game, &target.game) {
        reasons.push("State ownership is unproven, conflicting or belongs to different game/media evidence; filenames do not establish identity.".into());
    }
    if !record.game_identity.is_empty() && !same_game(&record.game_identity, &capture.game) {
        reasons.push("Inventory identity does not corroborate the capture provenance.".into());
    }
    if capture.emulator == StateEmulator::Unknown || target.emulator == StateEmulator::Unknown {
        reasons.push("Emulator identity is unknown.".into());
    } else if capture.emulator != target.emulator {
        reasons.push("Capture emulator differs from the currently selected emulator; states are not interchangeable between emulators.".into());
        incompatible = true;
    }
    if capture.emulator == StateEmulator::RetroArch && target.emulator == StateEmulator::RetroArch {
        match (capture.core.as_deref(), target.core.as_deref()) {
            (Some(a), Some(b)) if !a.trim().is_empty() && !b.trim().is_empty() && a != b => {
                reasons.push("Capture RetroArch core differs from the selected core.".into());
                incompatible = true;
            }
            (Some(a), Some(b)) if !a.trim().is_empty() && a == b => {}
            _ => reasons.push("RetroArch core identity is missing.".into()),
        }
        compare_version(
            &mut reasons,
            "Core",
            &capture.core_version,
            &target.core_version,
        );
    }
    compare_version(&mut reasons, "Emulator", &capture.version, &target.version);
    match (&capture.profile, &target.profile) {
        (Some(a), Some(b)) if !a.trim().is_empty() && a == b => {}
        (Some(_), Some(_)) => reasons.push(
            "Selected profile changed; configuration-dependent compatibility needs review.".into(),
        ),
        _ => reasons.push("Capture or selected profile provenance is missing.".into()),
    }
    match format {
        SaveStateFormatEvidence::Unsupported(detail) => {
            incompatible = true; reasons.push(format!("State format is unsupported: {detail}"));
        }
        SaveStateFormatEvidence::Supported { capture: checked_capture, target: checked_target, source }
            if checked_capture == capture && checked_target == target && !source.trim().is_empty() => {},
        _ => reasons.push("No verified state-format/version compatibility contract exists for this capture and selected target. Matching extensions or versions are insufficient.".into()),
    }
    let compatibility = if incompatible {
        SaveStateCompatibility::Incompatible
    } else if reasons.is_empty() {
        SaveStateCompatibility::Supported
    } else {
        SaveStateCompatibility::Unknown
    };
    SaveStateAssessment {
        compatibility,
        reasons,
    }
}

fn compare_version(reasons: &mut Vec<String>, label: &str, a: &Option<String>, b: &Option<String>) {
    match (a, b) {
        (Some(a), Some(b)) if !a.trim().is_empty() && a == b => {}
        (Some(_), Some(_)) => reasons.push(format!(
            "{label} versions differ; no cross-version format contract has been verified."
        )),
        _ => reasons.push(format!("{label} version evidence is missing.")),
    }
}

#[cfg(test)]
mod tests;
