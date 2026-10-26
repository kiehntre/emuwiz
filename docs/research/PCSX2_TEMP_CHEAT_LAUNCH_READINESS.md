# PCSX2 temporary-cheat launch readiness (V1, read-only planner)

Module: `crates/archivefs-core/src/patch_manager/pcsx2_temp_cheat_readiness.rs`
(`plan_pcsx2_temporary_cheat_launch`). Pure projection over existing evidence;
it writes no PNACH, no `PCSX2.ini`, launches nothing and touches no memory card.

The [2026-10-26 disposable probe](PCSX2_TEMP_CHEAT_PROBE_20261026.md)
observed selected PNACH loading and enablement. The pure planner remains
unchanged; the separately reviewed `launch::cheat_runtime_pcsx2` adapter now
adds exact-deployment capability checks, immutable approval, a scoped runtime
configuration probe and the canonical staging/recovery executor. Blue exposes
that explicit action beside ordinary launch. See the
[activation report](PCSX2_TEMP_CHEAT_ACTIVATION_20261026.md) for supported scope
and observed results.

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
| Launch binding or its blocker | `resolve_pcsx2_native_launch_binding`, **re-resolved for this planning pass, never cached**; the planner cross-checks it against the profile (installation family, directory mode, discovered executable) and refuses mismatches with `LaunchBindingMismatch` |
| Cheat material or its refusal | `discover_local_pcsx2_pnach_file` |
| Folder routing | `Pcsx2FolderRouting::from_ini_text` over `PCSX2.ini` text supplied by the caller |

## Verdicts

* `Blocked` - at least one `Pcsx2TempCheatBlockerKind`, each with a plain
  reason and an action: `IdentityNotVerified`, `SerialMissing`,
  `IdentityConflict`, `CheatMaterialMissing`, `CheatMaterialIncompatible`,
  `PortableCannotBeRedirected`, `UnsupportedInstallation`,
  `LaunchBindingUnavailable`, `LaunchBindingMismatch`, `ConfigurationUnknown`,
  `CheatsFolderUnavailable`, `FolderRoutingUnknown`, `FolderRoutingUnsafe`.
* `RequiresRuntimeProof` - every static precondition holds; isolation is not
  yet *observed*. EmuWiz must not enable cheats for a launch on this basis.

There is deliberately **no** supported verdict in V1. The planner accepts no
runtime proof: a caller-supplied flag set is not evidence, and it is not itself a trusted
producer. `runtime_proof_needed` lists what a future, separately
reviewed probe must establish (Flatpak adds `SandboxSawStagedRoot` and
`SandboxSawRealUserFolders`; an absolute host path is not assumed visible in
the sandbox).

Identity: a non-empty verified serial **and** a verified executable CRC are
both required; the cheat file must match both.

Folder routing: relative or `..` entries in `[Folders]` (the user's AppImage
`Bios` entry is one) are `FolderRoutingUnsafe`, because a per-launch data root
would resolve them somewhere else. An absolute `Cheats` entry is refused (it would bypass the staged cheats
folder) until a validated workspace-pinning contract exists. A duplicate
`[Folders]` section, a duplicate key or a differently-cased key makes routing
`Unknown`: EmuWiz does not assume how PCSX2 interprets them. Unspecified user-data folders (memory cards,
save states, BIOS, ...) are not blockers but become mandatory
`PinUserFolderToRealDefault` staging requirements so a redirected launch can
never silently use an empty memory-card folder.

Routing also covers `Patches`, `Covers`, `GameSettings`, `UserResources`,
`DebuggerLayouts` and `DebuggerSettings`. Game settings use the existing
copy-to-workspace requirement, never a writable production pin; debugger
defaults are under `inis`. The installed v2.8.2 appends `PCSX2` to the
`-datapath` argument, so its resulting data root needs direct observation.

## Original executor requirements

These motivated the separate adapter; they are not a supported verdict in
this pure planner. The adapter privately copies BIOS/console state as well
as game settings, because PCSX2 may create `.mec`/`.nvm` beside the BIOS.

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
