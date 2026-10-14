# PCSX2 temporary-cheat launch readiness (V1, read-only planner)

Module: `crates/archivefs-core/src/patch_manager/pcsx2_temp_cheat_readiness.rs`
(`plan_pcsx2_temporary_cheat_launch`). Pure projection over existing evidence;
it writes no PNACH, no `PCSX2.ini`, launches nothing and touches no memory card.

## Correction to the feasibility report

`PCSX2_CHEAT_RUNTIME_FEASIBILITY.md` (2026-10-04) states that EmuWiz launches
native PCSX2 only. Current main's `launch/pcsx2_execution.rs` launches Native,
AppImage (portable or not) and Flatpak installs; only `NativeAlternate` is
refused. **Launch support is not cheat support**: a launchable install says
nothing about which `Cheats` folder PCSX2 reads, and a portable marker still
outranks `-datapath`. The planner therefore never derives "cheats supported"
from "can launch".

## Inputs (all already produced by existing read-only code)

| Input | Source |
|---|---|
| Game identity (verified serial + executable CRC) | `Pcsx2GameIdentity` |
| Profile (type, eligibility, cheats folder state) | `Pcsx2Profile` |
| Launch binding or its blocker | `resolve_pcsx2_native_launch_binding` |
| Cheat material or its refusal | `discover_local_pcsx2_pnach_file` |
| Folder routing | `Pcsx2FolderRouting::from_ini_text` over `PCSX2.ini` text supplied by the caller |
| Runtime proof | `Pcsx2RuntimeProof` (default: nothing proven) |

## Verdicts

* `Blocked` - at least one `Pcsx2TempCheatBlockerKind`, each with a plain
  reason and an action: `IdentityNotVerified`, `SerialMissing`,
  `IdentityConflict`, `CheatMaterialMissing`, `CheatMaterialIncompatible`,
  `PortableCannotBeRedirected`, `UnsupportedInstallation`,
  `LaunchBindingUnavailable`, `ConfigurationUnknown`,
  `CheatsFolderUnavailable`, `FolderRoutingUnknown`, `FolderRoutingUnsafe`.
* `RequiresRuntimeProof` - every static precondition holds; isolation is not
  yet *observed*. EmuWiz must not enable cheats for a launch on this basis.
* `SupportedIsolated` - only when all required `Pcsx2RuntimeProofItem`s were
  supplied (Flatpak additionally needs `SandboxSawStagedRoot`). Nothing in this
  change can produce proof, so the shipped answer is never better than
  `RequiresRuntimeProof`.

Folder routing: relative or `..` entries in `[Folders]` (the user's AppImage
`Bios` entry is one) are `FolderRoutingUnsafe`, because a per-launch data root
would resolve them somewhere else. Unspecified user-data folders (memory cards,
save states, BIOS, ...) are not blockers but become mandatory
`PinUserFolderToRealDefault` staging requirements so a redirected launch can
never silently use an empty memory-card folder.

## Remaining requirements for real cheat-on-launch execution

1. A boot probe on a legal homebrew ELF with a working display, using the
   installed PCSX2 version, proving each `Pcsx2RuntimeProofItem`: `-datapath`
   honoured, `Found N cheats in <staged file>` in `-logfile`, real memory
   cards/save states/BIOS used and unchanged (before/after hashes), real
   `PCSX2.ini` and data root unchanged, patch/widescreen behaviour preserved.
2. A staging executor (separate stage, separate review) that creates a private
   per-launch workspace, seeds `PCSX2.ini` from the user's, pins unspecified
   user-data folders to absolute real defaults, points only `Cheats` into the
   workspace, writes `<serial>_<CRC>.pnach`, `[EmuCore] EnableCheats` and
   `[Cheats] Enable = <names>`, copies per-game settings, and discards the
   workspace afterwards.
3. Flatpak: a per-launch, workspace-only filesystem grant
   (`launch::installation::VisibilityPlan`) and proof the sandbox saw it.
4. Portable installs remain unsupported until PCSX2 offers a per-launch cheat
   folder or portable mode that yields to an explicit `-datapath`; the only
   alternative is editing the real cheats folder, which this feature must not do.
5. CRC provenance: the executable CRC must be computed the way PCSX2 does; the
   planner trusts only `Pcsx2GameIdentity::verified_crc`.
