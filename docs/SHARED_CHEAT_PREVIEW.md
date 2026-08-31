# Shared Cheats & Mods preview

## CURRENT BEHAVIOR

The shared preview is a bounded, evidence-aware plan. It joins a selected
verified game identity to a discovered emulator profile, provider record,
materialized local source, exact destination, and current destination state.
It is not a generic patch interpreter and never executes cheat directives.

Read-only stages include provider browsing, source validation, emulator/profile
discovery, local inventory, and preview. They create no user files and do not
modify emulator files. A preview can report candidate, blocked, conflict,
preview-only, already-installed, or apply-eligible entries.

An apply-eligible entry requires an exact verified identity, an adapter-approved
materialized source, a safe destination, fresh source/destination checks, and
explicit confirmation. Supported PCSX2, Dolphin, RetroArch, and GameCube/Wii
flows use the shared transaction engine; coverage is adapter-specific.

Local mod-package inspection and planning are supported. Unsupported patch or
mod formats fail closed. Downloads and external installers are not part of
local mod-package Stage 1.

See [SHARED_SAFE_APPLY_ROLLBACK.md](SHARED_SAFE_APPLY_ROLLBACK.md),
[CHEATS_MODS_SAFETY.md](CHEATS_MODS_SAFETY.md), and
[ADAPTER_SUPPORT_MATRIX.md](ADAPTER_SUPPORT_MATRIX.md).
