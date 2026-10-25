//! Conservative projection of neutral C64 direct writes into VICE monitor commands.
//! Runtime projection only; media and VICE global config are never modified.

use super::cheat_compatibility::CheatRevisionEvidence;
use super::cheat_ir::CheatOperation;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViceMemoryTarget {
    Ram,
    RomMapped,
    Io,
    ColourRam,
    CartridgeBanked,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViceCheatIssue {
    InvalidAddress,
    InvalidValue,
    DangerousIo,
    BankUnknown,
    IdentityWeak,
    IdentityMismatch,
    UnsupportedOperation,
    CompareUnsupported,
    LaunchTimingWarning,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViceCheatReadiness {
    ReadyForPreview,
    ReadyForLaunchReview,
    PreviewOnly,
    Blocked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViceCheatCommand {
    pub text: String,
    pub address: u16,
    pub value: u8,
    pub memory: ViceMemoryTarget,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViceCheatProjection {
    pub commands: Vec<ViceCheatCommand>,
    pub script: String,
    pub launch_argument: String,
    pub readiness: ViceCheatReadiness,
    pub issues: Vec<ViceCheatIssue>,
    pub provenance: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViceCheatIdentity {
    ExactMediaHash(String),
    VerifiedGame(String),
    ExactProgram(String),
    TitleOnly(String),
    Unknown,
}

pub fn classify_c64_memory(address: u64) -> (ViceMemoryTarget, Option<ViceCheatIssue>) {
    if address > 0xFFFF {
        return (
            ViceMemoryTarget::Unknown,
            Some(ViceCheatIssue::InvalidAddress),
        );
    }
    let a = address as u16;
    match a {
        0xD800..=0xDBFF => (ViceMemoryTarget::ColourRam, None),
        0xD000..=0xDFFF => (ViceMemoryTarget::Io, Some(ViceCheatIssue::DangerousIo)),
        0xA000..=0xBFFF | 0xE000..=0xFFFF => (
            ViceMemoryTarget::RomMapped,
            Some(ViceCheatIssue::BankUnknown),
        ),
        0x8000..=0x9FFF => (
            ViceMemoryTarget::CartridgeBanked,
            Some(ViceCheatIssue::BankUnknown),
        ),
        _ => (ViceMemoryTarget::Ram, None),
    }
}
fn write8(op: &CheatOperation) -> Option<(u16, u8)> {
    match op {
        CheatOperation::Write8 { address, value } if *address <= 0xFFFF => {
            Some((*address as u16, *value))
        }
        _ => None,
    }
}
pub fn project_vice_c64_pokes(
    operations: &[CheatOperation],
    identity: &ViceCheatIdentity,
) -> ViceCheatProjection {
    let mut commands = Vec::new();
    let mut issues = Vec::new();
    if matches!(
        identity,
        ViceCheatIdentity::TitleOnly(_) | ViceCheatIdentity::Unknown
    ) {
        issues.push(ViceCheatIssue::IdentityWeak);
    }
    for op in operations {
        let Some((address, value)) = write8(op) else {
            issues.push(ViceCheatIssue::UnsupportedOperation);
            continue;
        };
        let (memory, issue) = classify_c64_memory(address as u64);
        if let Some(issue) = issue {
            issues.push(issue);
        }
        commands.push(ViceCheatCommand {
            text: format!("> {address:04X} {value:02X}"),
            address,
            value,
            memory,
        });
    }
    let mut script = String::from("radix H\n");
    for command in &commands {
        script.push_str(&command.text);
        script.push('\n');
    }
    script.push_str("x\n");
    let readiness = if commands.is_empty()
        || issues.iter().any(|i| {
            matches!(
                i,
                ViceCheatIssue::InvalidAddress
                    | ViceCheatIssue::UnsupportedOperation
                    | ViceCheatIssue::IdentityWeak
                    | ViceCheatIssue::BankUnknown
            )
        }) {
        ViceCheatReadiness::PreviewOnly
    } else {
        ViceCheatReadiness::ReadyForLaunchReview
    };
    if !commands.is_empty() {
        issues.push(ViceCheatIssue::LaunchTimingWarning);
    }
    ViceCheatProjection {
        commands,
        script,
        launch_argument: "-moncommands <EmuWiz-managed-session-file>".into(),
        readiness,
        issues,
        provenance: vec!["VICE monitor command projection; runtime only".into()],
    }
}
pub fn vice_cheat_identity_strength(identity: &ViceCheatIdentity) -> CheatRevisionEvidence {
    match identity {
        ViceCheatIdentity::ExactMediaHash(hash) => {
            CheatRevisionEvidence::ExactHash { hash: hash.clone() }
        }
        ViceCheatIdentity::VerifiedGame(id) => CheatRevisionEvidence::VerifiedIdentity {
            identity: id.clone(),
            revision: None,
        },
        ViceCheatIdentity::ExactProgram(id) => CheatRevisionEvidence::ProviderDeclared {
            revision: id.clone(),
        },
        ViceCheatIdentity::TitleOnly(title) => CheatRevisionEvidence::TitleOnly {
            title: title.clone(),
        },
        ViceCheatIdentity::Unknown => CheatRevisionEvidence::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simple_ram_poke_projects_deterministically() {
        let op = CheatOperation::Write8 {
            address: 49152,
            value: 255,
        };
        let a = project_vice_c64_pokes(
            &[op.clone()],
            &ViceCheatIdentity::ExactProgram("game-v1".into()),
        );
        let b = project_vice_c64_pokes(&[op], &ViceCheatIdentity::ExactProgram("game-v1".into()));
        assert_eq!(a, b);
        assert_eq!(a.commands[0].memory, ViceMemoryTarget::Ram);
        assert!(a.script.contains("> C000 FF"));
    }
    #[test]
    fn io_rom_and_banked_ranges_warn() {
        assert!(matches!(
            classify_c64_memory(0xD400).1,
            Some(ViceCheatIssue::DangerousIo)
        ));
        assert!(matches!(
            classify_c64_memory(0xA000).0,
            ViceMemoryTarget::RomMapped
        ));
        assert!(matches!(
            classify_c64_memory(0x8000).1,
            Some(ViceCheatIssue::BankUnknown)
        ));
    }
    #[test]
    fn weak_identity_is_preview_only_and_non_byte_ops_are_not_faked() {
        let r = project_vice_c64_pokes(
            &[CheatOperation::Write16 {
                address: 1,
                value: 2,
            }],
            &ViceCheatIdentity::TitleOnly("game".into()),
        );
        assert_eq!(r.readiness, ViceCheatReadiness::PreviewOnly);
        assert!(r.commands.is_empty());
        assert!(r.issues.contains(&ViceCheatIssue::UnsupportedOperation));
    }
    #[test]
    fn out_of_range_is_rejected() {
        let r = project_vice_c64_pokes(
            &[CheatOperation::Write8 {
                address: 0x1_0000,
                value: 1,
            }],
            &ViceCheatIdentity::ExactMediaHash("abc".into()),
        );
        assert!(r.issues.contains(&ViceCheatIssue::UnsupportedOperation));
    }
}
