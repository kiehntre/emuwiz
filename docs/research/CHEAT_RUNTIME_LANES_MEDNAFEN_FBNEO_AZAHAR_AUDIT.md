# Cheat runtime audit: Mednafen, FBNeo (standalone) and Azahar

Date: 2026-10-04. Main: `8b831eca`. Read-only audit; no code was changed. The three emulators were checked for the smallest missing seam between a staged cheat and an emulator launch.

Levels: L0 research, L1 parse, L2 normalise, L3 validate, L4 stage/install, L5 launch wiring, L6 emulator explicitly instructed to consume the cheat, L7 gameplay effect verified.

## Summary

| Emulator | Highest level on main | Missing seam | Smallest honest next step |
| --- | --- | --- | --- |
| Mednafen | L4 (API only, no caller) | There is **no standalone Mednafen launch adapter at all** (no command planner, preflight or spawn). The only Mednafen references in `launch/` are RetroArch libretro core hints. | Build the standalone Mednafen launch adapter first; cheat wiring cannot be "the smallest connection" to a command that does not exist. |
| FBNeo (standalone) | L4 (API only, no caller) | A launch exists (`fbneo <driver>`), but **FBNeo has no command-line cheat switch**; consumption needs a global support-path setting and the in-emulator Cheats dialog. Nothing per-launch can be added to the argv. | Report-only until the supported path/enable behaviour is verified on an installed FBNeo. |
| Azahar | L3 (parse, normalise, version/title validation); **no stage/apply** | Launch supports only loose `.3dsx` homebrew; cheats attach to retail title IDs. The two do not intersect. There is also no apply adapter. | Retail-title launch support and a cheat apply adapter are separate, larger pieces of work. |

## Mednafen

- **Cheat side:** `patch_manager/mednafen_cheat.rs` parses, validates and renders Mednafen's native cheat file (named by the game's MD5), merges, builds and applies a shared-transaction plan with rollback, and reports loadability facts, for a fixed system matrix (`mednafen_supported_systems`: gb, gg, lynx, md, nes, pce, pcfx, psx, sms, snes, vb, wswan). The adapter documents that a restart/reload is required and that the adapter never signals a running process.
- **Launch side:** `launch/` has no `mednafen_*` module. `platform_map.rs` and `integration.rs` only list libretro cores such as `mednafen_psx`, which belong to the RetroArch lane.
- **Callers:** none of the adapter's public functions are called outside its own module.
- **Host:** Mednafen is not installed here, so no real run was possible.
- **Unverified here:** whether a per-launch setting override (cheat enable, cheat directory) can stand in for a persistent configuration change. That would be the preferred design, but it must be confirmed against an installed Mednafen before any code relies on it.

## FBNeo (standalone)

- **Launch side:** `launch/fbneo_command.rs` builds the command from verified FBNeo-specific DAT evidence: exactly `[<driver shortname>]` with an explicit executable binding. `fbneo_execution.rs` has the canonical preflight and spawn.
- **Cheat side:** `patch_manager/fbneo_cheat.rs` stages `<shortname>.ini` (FBNeo's per-set cheat format) into the configured FBNeo cheat-support directory through the shared transaction; its readiness is `RuntimeEnableRequired`.
- **Why no wiring:** the standalone emulator reads cheats from its configured support path and enables entries through its Cheats dialog; there is no argument to add to the launch command. Whether an entry's `default` option starts enabled was not verified. Pointing FBNeo at a staged directory is a global configuration change, not a per-launch one.
- **Not this lane:** libretro FBNeo (RetroArch core) is a different consumption path owned by the RetroArch work.
- **Host:** standalone FBNeo is not installed here.
- **Honest status:** staged is not the same as seen, and seen is not the same as enabled. If manual in-emulator activation is required, say so rather than automate.

## Azahar (Citra family)

- **Launch side:** `launch/azahar_command.rs` is "Phase 1": loose `.3dsx` homebrew only. Any other content form is refused (`AzaharContentFormatUnsupported`: "only loose .3dsx homebrew is supported"). The command is just `[<content path>]`.
- **Cheat side:** `patch_manager/three_ds_cheat.rs` exposes `parse_three_ds_cheat_file`, `render_three_ds_cheat_file`, `merge_three_ds_cheat_file`, `assess_three_ds_version` and `three_ds_cheat_document`. That is L1–L3 only: there is **no** stage/apply function and no shared-transaction plan (an earlier audit note rated it L4; that was too generous).
- **Where Azahar looks:** a per-title text file named by title ID in the emulator's user directory (`cheats/`), with `*citra_enabled` marking an enabled entry. On this host both user trees contain an empty `cheats/` directory: the native one (`~/.local/share/azahar-emu/`) and the Flatpak one (`~/.var/app/org.azahar_emu.Azahar/data/azahar-emu/`). The `azahar` command on PATH is a wrapper around `flatpak run org.azahar_emu.Azahar`, so which user tree applies depends on how the emulator is installed.
- **Why no wiring:** cheats are for retail titles identified by title ID; the launch path cannot launch those. Closing that needs retail-content launch (including key and system-data handling), a cheat apply adapter, and install-type-aware user-directory resolution. No old Citra behaviour is assumed here.
- **Real emulator:** installed (Flatpak 2126.1.1) but not run: there is no suitable legal retail title to verify against, and a homebrew launch cannot demonstrate a cheat.
- **Distinctions to keep:** staged / emulator can see the file / cheat enabled / gameplay effect are four separate facts; none beyond "parses and validates" exists today.
