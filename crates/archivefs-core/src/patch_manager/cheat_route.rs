//! Selected-emulator cheat routing.
//!
//! A cheat is only useful when the emulator that will actually run the game
//! reads it. This module decides which emulator a cheat install is aimed at,
//! using this precedence:
//!
//! 1. the emulator the user explicitly selected for this game;
//! 2. a configured default (a remembered emulator profile) that serves the
//!    verified platform;
//! 3. a platform fallback, and only when the fallback emulator can actually
//!    consume a cheat format EmuWiz writes.
//!
//! It never routes a game to RetroArch merely because RetroArch exists: a
//! platform with no reviewed libretro core (PS3, 3DS, Xbox 360, ...) has no
//! RetroArch route at all, and when a standalone emulator for the platform is
//! installed alongside RetroArch the decision is `Ambiguous` until the user
//! chooses. The decision is pure: it performs no I/O, never switches the
//! user's selection, and returns identical output for identical input.

use serde::Serialize;

use crate::launch::platform_map::{LaunchCompatibility, launch_compatibility_for_platform};

/// Stable adapter key for RetroArch in routing inputs. Standalone keys are
/// the same strings [`crate::launch::platform_map::LAUNCH_COMPATIBILITY`] and
/// the remembered-profile file already use.
pub const RETROARCH_ROUTE_ID: &str = "retroarch";

/// Platforms whose native cheat format belongs to a standalone emulator that
/// EmuWiz writes (PNACH, Dolphin GameSettings, Xenia patches). A libretro
/// route is never offered for them: EmuWiz's codes for these systems target
/// the standalone file layout, not a libretro core.
const STANDALONE_OWNED_PLATFORMS: &[&str] = &["PS2", "GameCube", "Wii", "Xbox360"];

/// One emulator a cheat could be aimed at.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheatRouteTarget {
    /// RetroArch plus, when known, the exact libretro core stem that runs
    /// this game. `core: None` is a real, reportable state: RetroArch alone
    /// is never treated as sufficient identity.
    RetroArch { core: Option<String> },
    /// A standalone emulator identified by its stable adapter key
    /// (`"duckstation"`, `"pcsx2"`, ...).
    Standalone { adapter_id: String },
}

impl CheatRouteTarget {
    pub fn standalone(adapter_id: &str) -> Self {
        Self::Standalone {
            adapter_id: adapter_id.to_ascii_lowercase(),
        }
    }

    pub fn retroarch(core: Option<&str>) -> Self {
        Self::RetroArch {
            core: core.map(str::to_owned),
        }
    }

    /// The stable adapter key (`"retroarch"` for every RetroArch target).
    pub fn adapter_id(&self) -> &str {
        match self {
            Self::RetroArch { .. } => RETROARCH_ROUTE_ID,
            Self::Standalone { adapter_id } => adapter_id,
        }
    }

    pub fn retroarch_core(&self) -> Option<&str> {
        match self {
            Self::RetroArch { core } => core.as_deref(),
            Self::Standalone { .. } => None,
        }
    }

    /// Plain-language emulator name for the GUI.
    pub fn display_name(&self) -> String {
        match self {
            Self::RetroArch { core: Some(core) } => format!("RetroArch ({core})"),
            Self::RetroArch { core: None } => "RetroArch (core not identified)".to_string(),
            Self::Standalone { adapter_id } => standalone_display_name(adapter_id).to_string(),
        }
    }

    /// Whether two targets are the same emulator, ignoring an unknown
    /// RetroArch core on either side.
    fn same_emulator(&self, other: &Self) -> bool {
        self.adapter_id() == other.adapter_id()
    }
}

pub fn standalone_display_name(adapter_id: &str) -> &str {
    match adapter_id {
        "pcsx2" => "PCSX2",
        "dolphin" => "Dolphin",
        "xenia" => "Xenia Canary",
        "duckstation" => "DuckStation",
        "ppsspp" => "PPSSPP",
        "rpcs3" => "RPCS3",
        "flycast" => "Flycast",
        "azahar" => "Azahar",
        "melonds" => "melonDS",
        "desmume" => "DeSmuME",
        "mgba" => "mGBA",
        "mame" => "MAME",
        other => other,
    }
}

/// What EmuWiz can do with cheats for the routed emulator today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatApplySupport {
    /// EmuWiz previews, installs, and can undo cheats for this emulator.
    Supported,
    /// EmuWiz can read this emulator's existing cheat files but cannot
    /// install cheats for it yet.
    InventoryOnly,
    /// EmuWiz has no cheat support for this emulator.
    Unsupported,
}

pub fn cheat_apply_support(target: &CheatRouteTarget) -> CheatApplySupport {
    match target {
        CheatRouteTarget::RetroArch { .. } => CheatApplySupport::Supported,
        CheatRouteTarget::Standalone { adapter_id } => match adapter_id.as_str() {
            "pcsx2" | "dolphin" | "xenia" | "duckstation" | "ppsspp" | "mgba" | "mame" => {
                CheatApplySupport::Supported
            }
            "flycast" | "rpcs3" => CheatApplySupport::InventoryOnly,
            _ => CheatApplySupport::Unsupported,
        },
    }
}

/// The native cheat file format the routed emulator reads.
pub fn native_cheat_format(target: &CheatRouteTarget) -> &'static str {
    match target {
        CheatRouteTarget::RetroArch { .. } => "RetroArch .cht",
        CheatRouteTarget::Standalone { adapter_id } => match adapter_id.as_str() {
            "pcsx2" => "PCSX2 .pnach",
            "dolphin" => "Dolphin GameSettings .ini (Gecko / Action Replay)",
            "xenia" => "Xenia .patch.toml",
            "duckstation" => "DuckStation .cht",
            "ppsspp" => "PPSSPP CWCheat .ini",
            "mgba" => "mGBA .cheats",
            "mame" => "MAME cheat XML",
            "flycast" => "Flycast .cht",
            "rpcs3" => "RPCS3 patch.yml",
            "azahar" => "Azahar/Citra cheats .txt",
            _ => "no cheat format reviewed",
        },
    }
}

/// Which precedence rule produced a routed decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatRouteBasis {
    ExplicitSelection,
    ConfiguredDefault,
    PlatformFallback,
}

/// A resolved route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatRoute {
    pub platform_id: String,
    pub target: CheatRouteTarget,
    pub basis: CheatRouteBasis,
    pub apply_support: CheatApplySupport,
    pub native_format: &'static str,
    /// Other emulators known to serve this platform. Offered for an explicit
    /// user choice only; never applied automatically.
    pub alternatives: Vec<CheatRouteTarget>,
}

impl CheatRoute {
    pub fn can_apply(&self) -> bool {
        self.apply_support == CheatApplySupport::Supported
    }
}

/// Why an explicit or configured emulator was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatRouteRefusal {
    /// The selected emulator does not run this platform in EmuWiz's
    /// reviewed compatibility table.
    SelectedEmulatorDoesNotServePlatform,
    /// RetroArch was selected for a platform whose cheats EmuWiz only writes
    /// for a standalone emulator, or that has no reviewed libretro core.
    RetroArchNotACheatRouteForPlatform,
}

impl CheatRouteRefusal {
    pub fn message(self) -> &'static str {
        match self {
            Self::SelectedEmulatorDoesNotServePlatform => {
                "The selected emulator does not run this game's platform."
            }
            Self::RetroArchNotACheatRouteForPlatform => {
                "RetroArch cannot load EmuWiz cheats for this platform."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum CheatRouteDecision {
    Routed(CheatRoute),
    /// The explicitly selected emulator cannot consume cheats for this game.
    /// Compatible emulators are listed for an explicit choice.
    Refused {
        platform_id: String,
        selected: CheatRouteTarget,
        refusal: CheatRouteRefusal,
        alternatives: Vec<CheatRouteTarget>,
    },
    /// More than one emulator could run this game and nothing selects one.
    Ambiguous {
        platform_id: String,
        candidates: Vec<CheatRouteTarget>,
    },
    /// No verified platform, or no reviewed emulator for it.
    NoRoute {
        platform_id: Option<String>,
    },
}

impl CheatRouteDecision {
    pub fn route(&self) -> Option<&CheatRoute> {
        match self {
            Self::Routed(route) => Some(route),
            _ => None,
        }
    }

    /// The routed target only when EmuWiz can actually install for it.
    pub fn applicable_target(&self) -> Option<&CheatRouteTarget> {
        self.route()
            .filter(|route| route.can_apply())
            .map(|route| &route.target)
    }

    /// Every emulator the user could explicitly choose instead.
    pub fn choices(&self) -> &[CheatRouteTarget] {
        match self {
            Self::Routed(route) => &route.alternatives,
            Self::Refused { alternatives, .. } => alternatives,
            Self::Ambiguous { candidates, .. } => candidates,
            Self::NoRoute { .. } => &[],
        }
    }

    /// One plain-language sentence describing the decision.
    pub fn headline(&self) -> String {
        match self {
            Self::Routed(route) => match route.apply_support {
                CheatApplySupport::Supported => format!(
                    "Cheats will be installed for {}.",
                    route.target.display_name()
                ),
                CheatApplySupport::InventoryOnly => format!(
                    "EmuWiz can read {}'s existing cheats but cannot install cheats for it yet.",
                    route.target.display_name()
                ),
                CheatApplySupport::Unsupported => format!(
                    "This cheat format is not supported by your selected emulator ({}).",
                    route.target.display_name()
                ),
            },
            Self::Refused {
                selected, refusal, ..
            } => format!("{} ({})", refusal.message(), selected.display_name()),
            Self::Ambiguous { .. } => {
                "More than one emulator can run this game. Choose which one should receive cheats."
                    .to_string()
            }
            Self::NoRoute { .. } => {
                "No emulator EmuWiz knows can load cheats for this game.".to_string()
            }
        }
    }
}

/// Routing inputs. Every field is caller-observed fact; nothing here is
/// inferred by this module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheatRouteRequest {
    /// Canonical or alias platform text for the selected game.
    pub platform: Option<String>,
    /// The emulator the user explicitly chose for this game.
    pub selected: Option<CheatRouteTarget>,
    /// Remembered/configured default emulators (any platform; filtered here).
    pub configured_defaults: Vec<CheatRouteTarget>,
    /// Standalone emulators observed installed on this machine.
    pub installed_standalone: Vec<String>,
    /// RetroArch cores observed installed that run this platform. Empty with
    /// `retroarch_installed == Some(true)` means RetroArch exists but no core
    /// for this platform was identified.
    pub retroarch_cores: Vec<String>,
    /// Whether RetroArch was observed installed. `None` means not scanned.
    pub retroarch_installed: Option<bool>,
}

/// Resolves the canonical compatibility-table platform ID for `platform`.
pub fn canonical_cheat_platform(platform: &str) -> Option<&'static str> {
    let trimmed = platform.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        return None;
    }
    if let Some(entry) = crate::launch::platform_map::LAUNCH_COMPATIBILITY
        .iter()
        .find(|entry| entry.platform_id.eq_ignore_ascii_case(trimmed))
    {
        return Some(entry.platform_id);
    }
    if let Some(platform) = crate::platform::platform_by_id(trimmed) {
        return Some(platform.id);
    }
    crate::platform::platform_for_alias(trimmed).map(|platform| platform.id)
}

fn retroarch_allowed(platform_id: &str, row: Option<&LaunchCompatibility>) -> bool {
    if STANDALONE_OWNED_PLATFORMS.contains(&platform_id) {
        return false;
    }
    match row {
        // Outside the reviewed table RetroArch candidate generation still
        // works (see `platform_map`), and `.cht` is RetroArch's own format.
        None => true,
        Some(row) => !row.retroarch_core_hints.is_empty() || row.standalone_adapters.is_empty(),
    }
}

fn standalone_serves(row: Option<&LaunchCompatibility>, adapter_id: &str) -> bool {
    row.is_some_and(|row| row.standalone_adapters.contains(&adapter_id))
}

fn serves_platform(
    platform_id: &str,
    row: Option<&LaunchCompatibility>,
    target: &CheatRouteTarget,
) -> Result<(), CheatRouteRefusal> {
    match target {
        CheatRouteTarget::RetroArch { .. } => {
            if retroarch_allowed(platform_id, row) {
                Ok(())
            } else {
                Err(CheatRouteRefusal::RetroArchNotACheatRouteForPlatform)
            }
        }
        CheatRouteTarget::Standalone { adapter_id } => {
            if standalone_serves(row, adapter_id) {
                Ok(())
            } else {
                Err(CheatRouteRefusal::SelectedEmulatorDoesNotServePlatform)
            }
        }
    }
}

/// Every emulator known to serve `platform_id`, in a deterministic order:
/// the platform's standalone adapters in table order, then RetroArch (one
/// entry per identified core, or a single core-unknown entry).
fn platform_candidates(
    platform_id: &str,
    row: Option<&LaunchCompatibility>,
    request: &CheatRouteRequest,
) -> Vec<CheatRouteTarget> {
    let mut candidates: Vec<CheatRouteTarget> = row
        .map(|row| {
            row.standalone_adapters
                .iter()
                .map(|adapter| CheatRouteTarget::standalone(adapter))
                .collect()
        })
        .unwrap_or_default();
    if retroarch_allowed(platform_id, row) && request.retroarch_installed != Some(false) {
        let mut cores = request.retroarch_cores.clone();
        cores.sort();
        cores.dedup();
        if cores.is_empty() {
            candidates.push(CheatRouteTarget::retroarch(None));
        } else {
            candidates.extend(
                cores
                    .iter()
                    .map(|core| CheatRouteTarget::retroarch(Some(core))),
            );
        }
    }
    candidates
}

fn build_route(
    platform_id: &str,
    target: CheatRouteTarget,
    basis: CheatRouteBasis,
    candidates: &[CheatRouteTarget],
) -> CheatRoute {
    let alternatives = candidates
        .iter()
        .filter(|candidate| *candidate != &target)
        .cloned()
        .collect();
    CheatRoute {
        platform_id: platform_id.to_string(),
        apply_support: cheat_apply_support(&target),
        native_format: native_cheat_format(&target),
        target,
        basis,
        alternatives,
    }
}

/// When RetroArch is selected without a core, adopt the core only if exactly
/// one installed core serves the platform. Otherwise the core stays unknown.
fn resolve_retroarch_core(
    target: CheatRouteTarget,
    request: &CheatRouteRequest,
) -> CheatRouteTarget {
    match target {
        CheatRouteTarget::RetroArch { core: None } => {
            let mut cores = request.retroarch_cores.clone();
            cores.sort();
            cores.dedup();
            if cores.len() == 1 {
                CheatRouteTarget::retroarch(Some(&cores[0]))
            } else {
                CheatRouteTarget::retroarch(None)
            }
        }
        other => other,
    }
}

pub fn route_cheat_install(request: &CheatRouteRequest) -> CheatRouteDecision {
    let Some(platform_id) = request
        .platform
        .as_deref()
        .and_then(canonical_cheat_platform)
    else {
        return CheatRouteDecision::NoRoute {
            platform_id: request
                .platform
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        };
    };
    let row = launch_compatibility_for_platform(platform_id);
    let candidates = platform_candidates(platform_id, row, request);

    // 1. Explicit selection always wins, or is refused - it is never
    //    silently replaced by another emulator.
    if let Some(selected) = &request.selected {
        return match serves_platform(platform_id, row, selected) {
            Ok(()) => CheatRouteDecision::Routed(build_route(
                platform_id,
                resolve_retroarch_core(selected.clone(), request),
                CheatRouteBasis::ExplicitSelection,
                &candidates,
            )),
            Err(refusal) => CheatRouteDecision::Refused {
                platform_id: platform_id.to_string(),
                selected: selected.clone(),
                refusal,
                alternatives: candidates,
            },
        };
    }

    // 2. A configured default that serves this platform. Several distinct
    //    defaults for one platform are a tie, not a pick.
    let mut defaults: Vec<CheatRouteTarget> = request
        .configured_defaults
        .iter()
        .filter(|target| serves_platform(platform_id, row, target).is_ok())
        .cloned()
        .collect();
    defaults.sort();
    defaults.dedup_by(|left, right| left.same_emulator(right));
    match defaults.len() {
        0 => {}
        1 => {
            let target = resolve_retroarch_core(defaults.remove(0), request);
            return CheatRouteDecision::Routed(build_route(
                platform_id,
                target,
                CheatRouteBasis::ConfiguredDefault,
                &candidates,
            ));
        }
        _ => {
            return CheatRouteDecision::Ambiguous {
                platform_id: platform_id.to_string(),
                candidates: defaults,
            };
        }
    }

    // 3. Platform fallback.
    if candidates.is_empty() {
        return CheatRouteDecision::NoRoute {
            platform_id: Some(platform_id.to_string()),
        };
    }
    // A standalone emulator EmuWiz writes cheats for owns the platform.
    if let Some(owner) = candidates.iter().find(|candidate| {
        matches!(candidate, CheatRouteTarget::Standalone { .. })
            && cheat_apply_support(candidate) == CheatApplySupport::Supported
    }) {
        return CheatRouteDecision::Routed(build_route(
            platform_id,
            owner.clone(),
            CheatRouteBasis::PlatformFallback,
            &candidates,
        ));
    }
    let installed_standalone: Vec<CheatRouteTarget> = candidates
        .iter()
        .filter(|candidate| match candidate {
            CheatRouteTarget::Standalone { adapter_id } => request
                .installed_standalone
                .iter()
                .any(|installed| installed.eq_ignore_ascii_case(adapter_id)),
            CheatRouteTarget::RetroArch { .. } => false,
        })
        .cloned()
        .collect();
    let retroarch: Vec<CheatRouteTarget> = candidates
        .iter()
        .filter(|candidate| matches!(candidate, CheatRouteTarget::RetroArch { .. }))
        .cloned()
        .collect();

    if !retroarch.is_empty() {
        if !installed_standalone.is_empty() {
            // A standalone emulator for this platform is installed too: the
            // user may well play there, so never assume RetroArch.
            let mut tie = installed_standalone;
            tie.extend(retroarch);
            return CheatRouteDecision::Ambiguous {
                platform_id: platform_id.to_string(),
                candidates: tie,
            };
        }
        let target = if retroarch.len() == 1 {
            retroarch[0].clone()
        } else {
            CheatRouteTarget::retroarch(None)
        };
        return CheatRouteDecision::Routed(build_route(
            platform_id,
            target,
            CheatRouteBasis::PlatformFallback,
            &candidates,
        ));
    }

    // No RetroArch route: the platform belongs to standalone emulators only.
    let pool = if installed_standalone.is_empty() {
        candidates.clone()
    } else {
        installed_standalone
    };
    if pool.len() == 1 {
        return CheatRouteDecision::Routed(build_route(
            platform_id,
            pool[0].clone(),
            CheatRouteBasis::PlatformFallback,
            &candidates,
        ));
    }
    CheatRouteDecision::Ambiguous {
        platform_id: platform_id.to_string(),
        candidates: pool,
    }
}

#[cfg(test)]
mod tests;
