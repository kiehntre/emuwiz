//! Read-only netplay readiness planning.
//!
//! The planner compares supplied peer evidence only.  It never opens sockets,
//! probes the network, changes ports/firewalls/NAT, starts an emulator, or
//! mutates controller, firmware, save, or configuration state.

use crate::launch::readiness::FirmwareReadiness;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetplayProvider {
    RetroArch,
    Dolphin,
    Unknown(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetplayStrategy {
    Rollback,
    DelayLockstep,
    EmulatorNative,
    FrontendCoreNative,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetplayIdentityConfidence {
    Exact,
    Strong,
    FilenameOnly,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetplayGameIdentity {
    pub platform: String,
    pub title: Option<String>,
    pub revision: Option<String>,
    pub content_hash: Option<String>,
    pub confidence: NetplayIdentityConfidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetplayCoreIdentity {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetplayControllerAssignment {
    pub player: u8,
    pub controller: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetplayNetworkCapability {
    Direct,
    Relay,
    HostClient,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetplayDeterministicSettings {
    NotRequired,
    Matched(String),
    Mismatched(String),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetplayCapability {
    ProviderNative(NetplayProvider),
    CoreIdentity,
    ContentIdentity,
    FirmwareEvidence,
    ControllerSlots,
    NetworkEvidence(NetplayNetworkCapability),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetplayRequirement {
    StrongGameIdentity,
    SameProvider,
    SameCore,
    SameCoreVersion,
    SameEmulatorVersion,
    FirmwareParity,
    CompleteControllerAssignment,
    KnownNetworkCapability,
    DeterministicSettingsWhenRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetplayEvidence {
    pub provider: String,
    pub source: String,
    pub source_version: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetplayPeer {
    pub peer_id: String,
    pub provider: NetplayProvider,
    pub emulator_identity: String,
    pub emulator_version: Option<String>,
    pub core: Option<NetplayCoreIdentity>,
    pub game: NetplayGameIdentity,
    pub firmware: FirmwareReadiness,
    pub firmware_required: bool,
    pub deterministic_settings: NetplayDeterministicSettings,
    pub required_players: u8,
    pub controller_assignments: Vec<NetplayControllerAssignment>,
    pub network: NetplayNetworkCapability,
    pub capabilities: Vec<NetplayCapability>,
    pub requirements: Vec<NetplayRequirement>,
    pub evidence: Vec<NetplayEvidence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetplayCompatibility {
    Compatible,
    Incompatible,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetplayReadiness {
    Ready,
    IdentityMismatch,
    EmulatorMismatch,
    CoreMismatch,
    VersionMismatch,
    FirmwareMismatch,
    ControllerAssignmentIncomplete,
    NetworkUnknown,
    RequirementsUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetplayIssue {
    StrongIdentityRequired,
    GameIdentityMismatch,
    PlatformMismatch,
    ProviderMismatch,
    EmulatorIdentityMismatch,
    CoreMissing,
    CoreMismatch,
    CoreVersionMismatch,
    CoreVersionUnknown,
    EmulatorVersionMismatch,
    EmulatorVersionUnknown,
    FirmwareMismatch,
    FirmwareUnknown,
    DuplicatePlayerAssignment(u8),
    MissingPlayerAssignment(u8),
    DeterministicSettingsMismatch,
    DeterministicSettingsUnknown,
    NetworkCapabilityUnknown,
    UnsupportedProvider,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetplayReadinessPlan {
    pub provider: NetplayProvider,
    pub strategy: NetplayStrategy,
    pub compatibility: NetplayCompatibility,
    pub readiness: NetplayReadiness,
    pub peers: Vec<NetplayPeer>,
    pub evidence: Vec<NetplayEvidence>,
    pub issues: Vec<NetplayIssue>,
    pub read_only: bool,
}

fn strong_identity(peer: &NetplayPeer) -> bool {
    matches!(
        peer.game.confidence,
        NetplayIdentityConfidence::Exact | NetplayIdentityConfidence::Strong
    ) && peer.game.content_hash.is_some()
}

fn identity_issue(left: &NetplayPeer, right: &NetplayPeer) -> Option<NetplayIssue> {
    if !strong_identity(left) || !strong_identity(right) {
        return Some(NetplayIssue::StrongIdentityRequired);
    }
    if left.game.platform != right.game.platform
        || left.game.content_hash != right.game.content_hash
        || left.game.revision != right.game.revision
    {
        return Some(NetplayIssue::GameIdentityMismatch);
    }
    None
}

fn provider_strategy(provider: &NetplayProvider) -> NetplayStrategy {
    match provider {
        NetplayProvider::RetroArch => NetplayStrategy::FrontendCoreNative,
        NetplayProvider::Dolphin => NetplayStrategy::EmulatorNative,
        NetplayProvider::Unknown(_) => NetplayStrategy::Unknown,
    }
}

fn provider_issue(left: &NetplayPeer, right: &NetplayPeer) -> Option<NetplayIssue> {
    if left.provider != right.provider {
        return Some(NetplayIssue::ProviderMismatch);
    }
    match &left.provider {
        NetplayProvider::RetroArch => {
            if left.emulator_identity != "RetroArch" || right.emulator_identity != "RetroArch" {
                Some(NetplayIssue::EmulatorIdentityMismatch)
            } else {
                None
            }
        }
        NetplayProvider::Dolphin => {
            if left.emulator_identity != "Dolphin" || right.emulator_identity != "Dolphin" {
                Some(NetplayIssue::EmulatorIdentityMismatch)
            } else {
                None
            }
        }
        NetplayProvider::Unknown(_) => Some(NetplayIssue::UnsupportedProvider),
    }
}

fn version_issue(left: &NetplayPeer, right: &NetplayPeer) -> Option<NetplayIssue> {
    match &left.provider {
        NetplayProvider::RetroArch => {
            if left.emulator_version != right.emulator_version {
                if left.emulator_version.is_some() && right.emulator_version.is_some() {
                    Some(NetplayIssue::EmulatorVersionMismatch)
                } else {
                    Some(NetplayIssue::EmulatorVersionUnknown)
                }
            } else if left.emulator_version.is_none() {
                Some(NetplayIssue::EmulatorVersionUnknown)
            } else {
                None
            }
        }
        NetplayProvider::Dolphin => {
            if left.emulator_version != right.emulator_version {
                if left.emulator_version.is_some() && right.emulator_version.is_some() {
                    Some(NetplayIssue::EmulatorVersionMismatch)
                } else {
                    Some(NetplayIssue::EmulatorVersionUnknown)
                }
            } else if left.emulator_version.is_none() {
                Some(NetplayIssue::EmulatorVersionUnknown)
            } else {
                None
            }
        }
        NetplayProvider::Unknown(_) => Some(NetplayIssue::UnsupportedProvider),
    }
}

fn core_issue(left: &NetplayPeer, right: &NetplayPeer) -> Option<NetplayIssue> {
    if !matches!(left.provider, NetplayProvider::RetroArch) {
        return None;
    }
    match (&left.core, &right.core) {
        (Some(left), Some(right)) if left.name != right.name => Some(NetplayIssue::CoreMismatch),
        (Some(left), Some(right)) if left.version != right.version => {
            if left.version.is_some() && right.version.is_some() {
                Some(NetplayIssue::CoreVersionMismatch)
            } else {
                Some(NetplayIssue::CoreVersionUnknown)
            }
        }
        (Some(left), Some(_)) if left.version.is_none() => Some(NetplayIssue::CoreVersionUnknown),
        (Some(_), None) | (None, Some(_)) | (None, None) => Some(NetplayIssue::CoreMissing),
        _ => None,
    }
}

fn firmware_issue(left: &NetplayPeer, right: &NetplayPeer) -> Option<NetplayIssue> {
    if !left.firmware_required && !right.firmware_required {
        return None;
    }
    if left.firmware_required != right.firmware_required {
        return Some(NetplayIssue::FirmwareMismatch);
    }
    match (left.firmware, right.firmware) {
        (FirmwareReadiness::Verified, FirmwareReadiness::Verified)
        | (FirmwareReadiness::NotRequired, FirmwareReadiness::NotRequired) => None,
        (FirmwareReadiness::Missing, _) | (_, FirmwareReadiness::Missing) => {
            Some(NetplayIssue::FirmwareMismatch)
        }
        _ => Some(NetplayIssue::FirmwareUnknown),
    }
}

fn controller_issues(peers: &[NetplayPeer]) -> Vec<NetplayIssue> {
    let required = peers
        .iter()
        .map(|peer| peer.required_players)
        .max()
        .unwrap_or(0);
    let mut assignments = std::collections::BTreeSet::new();
    let mut issues = Vec::new();
    for peer in peers {
        for assignment in &peer.controller_assignments {
            if !assignments.insert(assignment.player) {
                issues.push(NetplayIssue::DuplicatePlayerAssignment(assignment.player));
            }
        }
    }
    for player in 1..=required {
        if !assignments.contains(&player) {
            issues.push(NetplayIssue::MissingPlayerAssignment(player));
        }
    }
    issues
}

/// Plan whether two supplied peers are compatible enough to attempt netplay.
/// This is a pure comparison of evidence and has no networking side effects.
pub fn plan_netplay_readiness(left: NetplayPeer, right: NetplayPeer) -> NetplayReadinessPlan {
    let provider = left.provider.clone();
    let strategy = provider_strategy(&provider);
    let mut issues = Vec::new();
    if let Some(issue) = identity_issue(&left, &right) {
        issues.push(issue);
    }
    if let Some(issue) = provider_issue(&left, &right) {
        issues.push(issue);
    }
    if let Some(issue) = core_issue(&left, &right) {
        issues.push(issue);
    }
    if let Some(issue) = version_issue(&left, &right) {
        issues.push(issue);
    }
    if let Some(issue) = firmware_issue(&left, &right) {
        issues.push(issue);
    }
    issues.extend(controller_issues(&[left.clone(), right.clone()]));
    if left.deterministic_settings != NetplayDeterministicSettings::NotRequired
        || right.deterministic_settings != NetplayDeterministicSettings::NotRequired
    {
        match (&left.deterministic_settings, &right.deterministic_settings) {
            (
                NetplayDeterministicSettings::Matched(left),
                NetplayDeterministicSettings::Matched(right),
            ) if left == right => {}
            (NetplayDeterministicSettings::Mismatched(_), _)
            | (_, NetplayDeterministicSettings::Mismatched(_)) => {
                issues.push(NetplayIssue::DeterministicSettingsMismatch)
            }
            _ => issues.push(NetplayIssue::DeterministicSettingsUnknown),
        }
    }
    if matches!(left.network, NetplayNetworkCapability::Unknown)
        || matches!(right.network, NetplayNetworkCapability::Unknown)
    {
        issues.push(NetplayIssue::NetworkCapabilityUnknown);
    }
    let readiness = if issues.is_empty() {
        NetplayReadiness::Ready
    } else if issues.iter().any(|issue| {
        matches!(
            issue,
            NetplayIssue::StrongIdentityRequired
                | NetplayIssue::GameIdentityMismatch
                | NetplayIssue::PlatformMismatch
        )
    }) {
        NetplayReadiness::IdentityMismatch
    } else if issues.iter().any(|issue| {
        matches!(
            issue,
            NetplayIssue::ProviderMismatch | NetplayIssue::EmulatorIdentityMismatch
        )
    }) {
        NetplayReadiness::EmulatorMismatch
    } else if issues.iter().any(|issue| {
        matches!(
            issue,
            NetplayIssue::CoreMissing | NetplayIssue::CoreMismatch
        )
    }) {
        NetplayReadiness::CoreMismatch
    } else if issues.iter().any(|issue| {
        matches!(
            issue,
            NetplayIssue::CoreVersionMismatch | NetplayIssue::EmulatorVersionMismatch
        )
    }) {
        NetplayReadiness::VersionMismatch
    } else if issues
        .iter()
        .any(|issue| matches!(issue, NetplayIssue::FirmwareMismatch))
    {
        NetplayReadiness::FirmwareMismatch
    } else if issues.iter().any(|issue| {
        matches!(
            issue,
            NetplayIssue::DuplicatePlayerAssignment(_) | NetplayIssue::MissingPlayerAssignment(_)
        )
    }) {
        NetplayReadiness::ControllerAssignmentIncomplete
    } else if issues
        .iter()
        .any(|issue| matches!(issue, NetplayIssue::NetworkCapabilityUnknown))
    {
        NetplayReadiness::NetworkUnknown
    } else {
        NetplayReadiness::RequirementsUnknown
    };
    let compatibility = if matches!(readiness, NetplayReadiness::Ready) {
        NetplayCompatibility::Compatible
    } else if issues.iter().any(|issue| {
        matches!(
            issue,
            NetplayIssue::StrongIdentityRequired
                | NetplayIssue::GameIdentityMismatch
                | NetplayIssue::ProviderMismatch
                | NetplayIssue::EmulatorIdentityMismatch
                | NetplayIssue::CoreMismatch
                | NetplayIssue::CoreMissing
                | NetplayIssue::CoreVersionMismatch
                | NetplayIssue::EmulatorVersionMismatch
                | NetplayIssue::FirmwareMismatch
                | NetplayIssue::DuplicatePlayerAssignment(_)
                | NetplayIssue::MissingPlayerAssignment(_)
                | NetplayIssue::DeterministicSettingsMismatch
                | NetplayIssue::UnsupportedProvider
        )
    }) {
        NetplayCompatibility::Incompatible
    } else {
        NetplayCompatibility::Unknown
    };
    let mut evidence = left.evidence.clone();
    evidence.extend(right.evidence.clone());
    NetplayReadinessPlan {
        provider,
        strategy,
        compatibility,
        readiness,
        peers: vec![left, right],
        evidence,
        issues,
        read_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(provider: NetplayProvider, version: Option<&str>) -> NetplayPeer {
        let retroarch = matches!(provider, NetplayProvider::RetroArch);
        NetplayPeer {
            peer_id: "peer".into(),
            provider,
            emulator_identity: if retroarch { "RetroArch" } else { "Dolphin" }.into(),
            emulator_version: version.map(str::to_string),
            core: retroarch.then(|| NetplayCoreIdentity {
                name: "snes9x".into(),
                version: Some("1.0".into()),
            }),
            game: NetplayGameIdentity {
                platform: if retroarch { "SNES" } else { "GameCube" }.into(),
                title: Some("Example Game".into()),
                revision: Some("rev-a".into()),
                content_hash: Some("hash-a".into()),
                confidence: NetplayIdentityConfidence::Exact,
            },
            firmware: FirmwareReadiness::NotRequired,
            firmware_required: false,
            deterministic_settings: NetplayDeterministicSettings::NotRequired,
            required_players: 2,
            controller_assignments: vec![NetplayControllerAssignment {
                player: 1,
                controller: "pad-a".into(),
            }],
            network: NetplayNetworkCapability::Direct,
            capabilities: Vec::new(),
            requirements: Vec::new(),
            evidence: vec![NetplayEvidence {
                provider: "synthetic".into(),
                source: "fixture".into(),
                source_version: Some("1".into()),
                reason: "synthetic evidence".into(),
            }],
        }
    }

    fn complete_pair() -> (NetplayPeer, NetplayPeer) {
        let mut left = peer(NetplayProvider::RetroArch, Some("1.20"));
        let mut right = peer(NetplayProvider::RetroArch, Some("1.20"));
        right.peer_id = "peer-2".into();
        right.controller_assignments = vec![NetplayControllerAssignment {
            player: 2,
            controller: "pad-b".into(),
        }];
        left.core = Some(NetplayCoreIdentity {
            name: "snes9x".into(),
            version: Some("1.0".into()),
        });
        right.core = left.core.clone();
        (left, right)
    }

    #[test]
    fn same_exact_game_core_and_version_is_compatible() {
        let (left, right) = complete_pair();
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(plan.readiness, NetplayReadiness::Ready);
        assert_eq!(plan.compatibility, NetplayCompatibility::Compatible);
        assert_eq!(plan.strategy, NetplayStrategy::FrontendCoreNative);
    }

    #[test]
    fn filename_only_identity_is_insufficient() {
        let (mut left, right) = complete_pair();
        left.game.confidence = NetplayIdentityConfidence::FilenameOnly;
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(plan.readiness, NetplayReadiness::IdentityMismatch);
        assert!(plan.issues.contains(&NetplayIssue::StrongIdentityRequired));
    }

    #[test]
    fn different_revision_or_hash_is_identity_mismatch() {
        let (left, mut right) = complete_pair();
        right.game.revision = Some("rev-b".into());
        right.game.content_hash = Some("hash-b".into());
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(plan.readiness, NetplayReadiness::IdentityMismatch);
        assert!(plan.issues.contains(&NetplayIssue::GameIdentityMismatch));
    }

    #[test]
    fn different_retroarch_core_is_mismatched() {
        let (left, mut right) = complete_pair();
        right.core.as_mut().unwrap().name = "genesis-plus-gx".into();
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(plan.readiness, NetplayReadiness::CoreMismatch);
    }

    #[test]
    fn incompatible_core_version_is_explicit() {
        let (left, mut right) = complete_pair();
        right.core.as_mut().unwrap().version = Some("2.0".into());
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(plan.readiness, NetplayReadiness::VersionMismatch);
        assert!(plan.issues.contains(&NetplayIssue::CoreVersionMismatch));
    }

    #[test]
    fn dolphin_version_mismatch_is_explicit() {
        let mut left = peer(NetplayProvider::Dolphin, Some("5.0"));
        let mut right = peer(NetplayProvider::Dolphin, Some("5.1"));
        left.controller_assignments[0].player = 1;
        right.controller_assignments = vec![NetplayControllerAssignment {
            player: 2,
            controller: "pad-b".into(),
        }];
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(plan.readiness, NetplayReadiness::VersionMismatch);
        assert_eq!(plan.strategy, NetplayStrategy::EmulatorNative);
    }

    #[test]
    fn missing_controller_assignment_is_blocked() {
        let (mut left, mut right) = complete_pair();
        left.controller_assignments.clear();
        right.controller_assignments.clear();
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(
            plan.readiness,
            NetplayReadiness::ControllerAssignmentIncomplete
        );
        assert!(
            plan.issues
                .contains(&NetplayIssue::MissingPlayerAssignment(1))
        );
    }

    #[test]
    fn complete_player_assignments_are_satisfied() {
        let (left, right) = complete_pair();
        let plan = plan_netplay_readiness(left, right);
        assert!(!plan.issues.iter().any(|issue| matches!(
            issue,
            NetplayIssue::DuplicatePlayerAssignment(_) | NetplayIssue::MissingPlayerAssignment(_)
        )));
    }

    #[test]
    fn unknown_network_evidence_never_reports_ready() {
        let (mut left, right) = complete_pair();
        left.network = NetplayNetworkCapability::Unknown;
        let plan = plan_netplay_readiness(left, right);
        assert_eq!(plan.readiness, NetplayReadiness::NetworkUnknown);
        assert_ne!(plan.readiness, NetplayReadiness::Ready);
    }

    #[test]
    fn planner_is_read_only_and_does_not_probe_network() {
        let (left, right) = complete_pair();
        let plan = plan_netplay_readiness(left, right);
        assert!(plan.read_only);
        assert_eq!(plan.peers.len(), 2);
    }
}
