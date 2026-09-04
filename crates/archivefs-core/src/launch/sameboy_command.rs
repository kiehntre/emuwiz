//! Read-only native SameBoy command planning for direct `.gb`/`.gbc` files.
//!
//! Self-contained, like [`crate::launch::fbneo_command`] and
//! [`crate::launch::cemu_command`]: not wired into
//! [`crate::launch::integration::DiscoveredStandaloneProfile`] or the shared
//! [`crate::launch::planning`] candidate matrix. mGBA (Game Boy/Color/
//! Advance) remains completely independent - this module never reads, calls,
//! or otherwise touches [`crate::launch::mgba_command`], and nothing here
//! changes mGBA's own discovery, command, or readiness behavior. The two are
//! meant to become separate, simultaneously-available candidates for the
//! same platform once a future shared-planner slice wires both in.
//!
//! Argv is exactly `sameboy <rom path>` - verified from `LIJI32/SameBoy`'s
//! `SDL/main.c: int main`, which refuses (usage + exit 1) any invocation
//! that is not exactly one bare, non-dash-prefixed path. No fullscreen,
//! shader, rewind, debugger, audio, model-forcing, or boot-ROM-override flag
//! is ever added, even though SameBoy supports several of them - the task's
//! own "minimum required arguments" rule and "never invent flags" rule both
//! apply here.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::gb_header_evidence::{GbColorSupport, GbHeaderFact};
use crate::launch::readiness::{
    LaunchBlocker, LaunchBlockerKind, LaunchWarning, LaunchWarningKind,
};
use crate::patch_manager::{SameBoyBootRomEvidence, SameBoyBootRomState, SameBoyConfigInspection};

pub const SAMEBOY_SUPPORTED_PLATFORM_IDS: &[&str] = &["Game Boy", "Game Boy Color"];

/// The Wii-U-adapter-style four-state verdict (see
/// [`crate::launch::cemu_command::CemuReadiness`] for the same reasoning):
/// the shared [`crate::launch::readiness::LaunchReadiness`] has no "needs
/// setup" state, and a missing executable/profile is something a person can
/// go fix, distinct from a hard content/identity failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameBoyReadiness {
    Ready,
    ReadyWithWarnings,
    NeedsSetup,
    Blocked,
}

fn is_setup_blocker(kind: LaunchBlockerKind) -> bool {
    matches!(kind, LaunchBlockerKind::SameBoyBindingUnavailable)
}

pub fn classify_sameboy_readiness(plan: &SameBoyCommandPlan) -> SameBoyReadiness {
    if plan.blockers.is_empty() {
        if plan.warnings.is_empty() {
            SameBoyReadiness::Ready
        } else {
            SameBoyReadiness::ReadyWithWarnings
        }
    } else if plan
        .blockers
        .iter()
        .all(|blocker| is_setup_blocker(blocker.kind))
    {
        SameBoyReadiness::NeedsSetup
    } else {
        SameBoyReadiness::Blocked
    }
}

/// The two content forms this build recognises. Never inferred from
/// extension alone at the identity layer - [`GbHeaderFact::color_support`]
/// is what actually decides whether the selected extension is trustworthy;
/// see [`build_sameboy_command_plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameBoyContentForm {
    Gb,
    Gbc,
}

pub fn form_for_path(path: &std::path::Path) -> Option<SameBoyContentForm> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "gb" => Some(SameBoyContentForm::Gb),
        "gbc" => Some(SameBoyContentForm::Gbc),
        _ => None,
    }
}

/// The narrow, immutable set of facts identifying exactly which SameBoy
/// launch is being requested and everything already gathered about it.
/// `rom_header` must be a *fresh* read of `selected_content` (never a cached
/// value) - see [`crate::launch::sameboy_execution`]'s preflight for why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyLaunchRequest {
    pub executable: PathBuf,
    pub profile_id: String,
    pub platform_id: String,
    pub selected_content: PathBuf,
    pub content_form: SameBoyContentForm,
    /// `None` when the header could not be parsed at all (too short /
    /// truncated) - always a hard blocker, never treated as "unknown but
    /// fine".
    pub rom_header: Option<GbHeaderFact>,
    pub boot_rom: SameBoyBootRomEvidence,
    pub config: SameBoyConfigInspection,
    /// Carried through for display only; never trusted by
    /// [`build_sameboy_command_plan`], which always recomputes its own
    /// blockers/warnings.
    pub readiness: SameBoyReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: SameBoyCommandSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameBoyCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub content_path: PathBuf,
    /// The cartridge header's own self-reported title - evidence, not a
    /// verified/trusted identity (see [`crate::gb_header_evidence`]'s own
    /// confidence model).
    pub rom_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SameBoyCommandPlan {
    pub command: Option<SameBoyCommand>,
    pub blockers: Vec<LaunchBlocker>,
    pub warnings: Vec<LaunchWarning>,
}

fn block(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}
fn warning(kind: LaunchWarningKind, detail: impl Into<String>) -> LaunchWarning {
    LaunchWarning {
        kind,
        detail: detail.into(),
    }
}

/// Builds the exact, minimal native SameBoy argv (`sameboy <rom path>` -
/// nothing else) - or explains with structured blockers why it cannot.
///
/// `request.rom_header` must already be a fresh parse of
/// `request.selected_content` via [`crate::gb_header_evidence::parse_gb_header`]
/// - this function never touches a filesystem itself.
pub fn build_sameboy_command_plan(request: &SameBoyLaunchRequest) -> SameBoyCommandPlan {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    if !SAMEBOY_SUPPORTED_PLATFORM_IDS.contains(&request.platform_id.as_str()) {
        blockers.push(block(
            LaunchBlockerKind::SameBoyPlatformMismatch,
            format!(
                "platform `{}` is not Game Boy or Game Boy Color",
                request.platform_id
            ),
        ));
    }

    let header = match &request.rom_header {
        Some(header) if header.logo_valid => Some(header),
        Some(_) => {
            blockers.push(block(
                LaunchBlockerKind::SameBoyRomHeaderInvalid,
                "the selected content's Nintendo logo bytes do not match - not a genuine \
                 Game Boy/Game Boy Color cartridge image",
            ));
            None
        }
        None => {
            blockers.push(block(
                LaunchBlockerKind::SameBoyRomHeaderInvalid,
                "the selected content is too short to contain a Game Boy cartridge header",
            ));
            None
        }
    };

    if let Some(header) = header {
        if !header.header_checksum_valid {
            warnings.push(warning(
                LaunchWarningKind::SameBoyRomHeaderChecksumInvalid,
                "the cartridge header checksum did not validate; the Nintendo logo still matched",
            ));
        }
        // The one genuine contradiction this evidence can prove: a cartridge
        // whose own header declares CGB-exclusivity physically cannot run on
        // original Game Boy hardware - see
        // `crate::gb_header_evidence::observe_gb_evidence`'s own doc comment
        // and `game_identity.rs`'s `.gb`-paired-with-CGB-only-content
        // precedent, reused here rather than re-derived.
        if request.platform_id == "Game Boy" && header.color_support == GbColorSupport::CgbOnly {
            blockers.push(block(
                LaunchBlockerKind::SameBoyIdentityConflict,
                "the cartridge header's own cgb_flag (0xC0) proves this title requires Game \
                 Boy Color hardware, but the selected platform is Game Boy",
            ));
        }
    }

    match request.boot_rom.state {
        SameBoyBootRomState::NotConfigured | SameBoyBootRomState::PresentUnverified => {}
        SameBoyBootRomState::Missing => warnings.push(warning(
            LaunchWarningKind::SameBoyCustomBootRomUnavailable,
            "a custom boot ROM directory is configured but no longer exists; SameBoy's own \
             built-in boot ROM will be used instead",
        )),
        SameBoyBootRomState::Unknown => warnings.push(warning(
            LaunchWarningKind::SameBoyCustomBootRomUnavailable,
            "a custom boot ROM directory is configured but does not contain a recognised boot \
             ROM file; SameBoy's own built-in boot ROM will be used instead",
        )),
    }

    if request.executable.as_os_str().is_empty() {
        blockers.push(block(
            LaunchBlockerKind::SameBoyBindingUnavailable,
            "no executable was provided",
        ));
    }

    if !blockers.is_empty() {
        return SameBoyCommandPlan {
            command: None,
            blockers,
            warnings,
        };
    }

    let header = header.expect("checked above");
    SameBoyCommandPlan {
        command: Some(SameBoyCommand {
            executable: request.executable.clone(),
            arguments: vec![request.selected_content.clone().into_os_string()],
            working_directory: None,
            selection: SameBoyCommandSelection {
                profile_id: request.profile_id.clone(),
                platform_id: request.platform_id.clone(),
                content_path: request.selected_content.clone(),
                rom_title: Some(header.title.clone()).filter(|title| !title.trim().is_empty()),
            },
        }),
        blockers: Vec::new(),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_header(color_support: GbColorSupport) -> GbHeaderFact {
        GbHeaderFact {
            logo_valid: true,
            title: "TEST GAME".to_string(),
            color_support,
            sgb_enhanced: false,
            cartridge_type: 0,
            rom_size_code: 0,
            ram_size_code: 0,
            destination_code: 0,
            old_licensee: 0,
            mask_rom_version: 0,
            header_checksum: 0,
            global_checksum: 0,
            header_checksum_valid: true,
        }
    }

    fn base_request(platform_id: &str, form: SameBoyContentForm) -> SameBoyLaunchRequest {
        SameBoyLaunchRequest {
            executable: PathBuf::from("/opt/sameboy"),
            profile_id: "sameboy:/x".into(),
            platform_id: platform_id.into(),
            selected_content: PathBuf::from("/games/Game With Spaces.gb"),
            content_form: form,
            rom_header: Some(valid_header(GbColorSupport::DmgOnly)),
            boot_rom: SameBoyBootRomEvidence {
                directory: None,
                state: SameBoyBootRomState::NotConfigured,
            },
            config: SameBoyConfigInspection {
                path: PathBuf::from("/x/prefs.bin"),
                exists: true,
                readable: true,
                oversized: false,
            },
            readiness: SameBoyReadiness::Ready,
        }
    }

    #[test]
    fn deterministic_minimal_argv_with_spaces() {
        let request = base_request("Game Boy", SameBoyContentForm::Gb);
        let plan = build_sameboy_command_plan(&request);
        assert_eq!(classify_sameboy_readiness(&plan), SameBoyReadiness::Ready);
        let command = plan.command.expect("command");
        assert_eq!(command.executable, PathBuf::from("/opt/sameboy"));
        assert_eq!(
            command.arguments,
            vec![OsString::from("/games/Game With Spaces.gb")]
        );
    }

    #[test]
    fn game_boy_color_accepted_with_cgb_enhanced_header() {
        let mut request = base_request("Game Boy Color", SameBoyContentForm::Gbc);
        request.rom_header = Some(valid_header(GbColorSupport::CgbEnhanced));
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_some());
    }

    #[test]
    fn gba_platform_is_rejected() {
        let request = base_request("Game Boy Advance", SameBoyContentForm::Gb);
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::SameBoyPlatformMismatch)
        );
    }

    #[test]
    fn malformed_rom_header_blocks() {
        let mut request = base_request("Game Boy", SameBoyContentForm::Gb);
        request.rom_header = None;
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::SameBoyRomHeaderInvalid)
        );
    }

    #[test]
    fn invalid_logo_blocks_even_though_header_parsed() {
        let mut request = base_request("Game Boy", SameBoyContentForm::Gb);
        let mut header = valid_header(GbColorSupport::DmgOnly);
        header.logo_valid = false;
        request.rom_header = Some(header);
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::SameBoyRomHeaderInvalid)
        );
    }

    #[test]
    fn cgb_only_header_selected_as_game_boy_is_a_contradiction() {
        let mut request = base_request("Game Boy", SameBoyContentForm::Gb);
        request.rom_header = Some(valid_header(GbColorSupport::CgbOnly));
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_none());
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.kind == LaunchBlockerKind::SameBoyIdentityConflict)
        );
    }

    #[test]
    fn cgb_only_header_selected_as_game_boy_color_is_not_a_contradiction() {
        let mut request = base_request("Game Boy Color", SameBoyContentForm::Gbc);
        request.rom_header = Some(valid_header(GbColorSupport::CgbOnly));
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_some());
    }

    #[test]
    fn missing_custom_boot_rom_is_a_warning_never_a_blocker() {
        let mut request = base_request("Game Boy", SameBoyContentForm::Gb);
        request.boot_rom = SameBoyBootRomEvidence {
            directory: Some(PathBuf::from("/custom/boot")),
            state: SameBoyBootRomState::Missing,
        };
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_some());
        assert_eq!(
            classify_sameboy_readiness(&plan),
            SameBoyReadiness::ReadyWithWarnings
        );
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.kind == LaunchWarningKind::SameBoyCustomBootRomUnavailable)
        );
    }

    #[test]
    fn invalid_header_checksum_is_a_warning_never_a_blocker() {
        let mut request = base_request("Game Boy", SameBoyContentForm::Gb);
        let mut header = valid_header(GbColorSupport::DmgOnly);
        header.header_checksum_valid = false;
        request.rom_header = Some(header);
        let plan = build_sameboy_command_plan(&request);
        assert!(plan.command.is_some());
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.kind == LaunchWarningKind::SameBoyRomHeaderChecksumInvalid)
        );
    }

    #[test]
    fn argv_never_goes_through_a_shell_string() {
        let request = base_request("Game Boy", SameBoyContentForm::Gb);
        let plan = build_sameboy_command_plan(&request);
        let command = plan.command.unwrap();
        assert_eq!(command.arguments.len(), 1);
    }
}
