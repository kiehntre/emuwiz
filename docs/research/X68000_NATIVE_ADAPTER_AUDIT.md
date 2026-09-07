# X68000 Native Emulator Adapter Audit V1

**Scope:** research/design only. No production code, launch registration, identity
parser, media, or configuration files were changed.

## Executive conclusion

No dedicated native X68000 adapter exists in the current EmuWiz tree. The existing
launch system contains no X68000 standalone row; X68000 appears in ES-DE mappings
and can use the generic RetroArch path when a matching core is discovered. Existing
XDF/DIM/D88/HDI/NHD and Human68k evidence is sufficient to gate platform confidence,
but not to select a machine model or exact title.

The practical Linux candidate is **PX68k through RetroArch/libretro**, not a
standalone XM6 TypeG process. PX68k has documented Linux-capable distribution via
RetroArch, deterministic content command files, and explicit multi-disk `.m3u`
support. It is therefore the only realistic future adapter target, but it is a
**RetroArch-core integration**, not a new standalone emulator binary. A native
XM6 TypeG adapter is deferred because Linux operation generally requires Wine and
does not provide a stable, reviewed cross-machine CLI contract.

## Local authority and current implementation

The audit started at local HEAD `d97fa0257e23cceefd648a9bdf2b9a0f9007ab07` on
`feature/archivefs-unified-platform`. The worktree is dirty in unrelated lanes
(Atari800, cheats, Amiga, optical, ingestion, GUI tests); none were modified.
No `xm6`, `xm6typeg`, `px68k`, or `x68000` executable is on PATH.

Current implementation evidence:

* `crates/archivefs-core/src/disk_format/x68000.rs` and
  `crates/archivefs-core/src/x68000_human68k.rs` provide bounded XDF/DIM and
  Human68k IPL/BPB evidence.
* `crates/archivefs-core/src/launch/platform_map.rs` has no X68000 standalone
  compatibility row. `retroarch_platform_candidate` remains the existing generic
  alternative.
* `crates/archivefs-core/src/launch/integration.rs` has no X68000 native profile.
* `crates/archivefs-gui/src/launch_readiness_page.rs` has no dedicated X68000
  adapter surface.
* `docs/research/JAPANESE_DISK_IDENTITY_AUDIT.md` explicitly records launch as a
  gap and keeps exact title identity DAT/hash-led.

No branch or worktree found in the local inventory is an X68000 adapter lane. Git
history contains the identity work (`66c5695 feat(identity): add X68000 Human68k
disk evidence`) but no native emulator adapter commit.

## Candidate ecosystem comparison

| Candidate | Linux | CLI determinism | Media | Firmware/config | Process suitability | Decision |
|---|---|---|---|---|---|---|
| XM6 TypeG | Usually Wine/community builds; no validated native Linux package found | Weak for a portable adapter; Windows GUI/config assumptions | Floppy/HDD support is emulator-specific and GUI/config dependent | Requires ROM/config setup; exact paths vary | Wine process and config writes complicate watched execution | **DEFER** |
| PX68k libretro core | Yes through RetroArch packages/cores | Good at RetroArch content boundary; core options are profile/config data | `.dim` and `.xdf` are documented; `.m3u`/command files support multi-disk; HDF support is documented by project lineage | BIOS in RetroArch system directory (`keropi/iplrom.dat`, `cgrom.dat`); profile is explicit RetroArch configuration | Existing RetroArch watched-process path | **PREFERRED** |
| Other standalone native Linux | No maintained candidate with a reviewed deterministic CLI was found | Unknown | Unknown | Unknown | Cannot safely integrate | **NONE FOUND** |

The [Libretro PX68k documentation](https://docs.libretro.com/library/px68k/)
documents launching a command file and an `.m3u` for multi-disk games, e.g. a
content command containing two DIM paths. The project lineage also records HDF and
second-HDD support in its release history ([px68k-libretro history](https://github.com/KMFDManic/px68k-libretro)). These are useful ecosystem
references, not proof that every local build exposes identical options.

## Machine/profile scope

V1 should not model every X68000 revision. Require an explicit user-selected
**PX68k X68000 profile** with:

* core name/version and RetroArch system directory;
* one conservative default machine configuration supplied by the installed core;
* verified IPL and CG ROM readiness;
* optional SCSI/SASI or sound extensions only when the selected profile explicitly
  declares them.

Do not infer model, CPU, RAM, FPU, SCSI, MIDI, or sound board from filename,
directory, disk size, or title. If a title needs a special profile, refuse launch
until the user selects one. Existing Human68k evidence proves platform family, not
hardware revision.

## Media contract

| Format | Current EmuWiz evidence | Future V1 decision | Reason |
|---|---|---|---|
| XDF | Strong raw X68000 layout plus Human68k evidence when present | **SAFE FOR V1** | Best bounded floppy path; require strong evidence, never suffix alone |
| DIM | DIFC/container and mapped Human68k evidence | **SAFE FOR V1** | Structured X68000-oriented image; preserve mapped-sector checks |
| D88 | Shared track/sector parser; family ambiguous | **NEEDS STRONGER EVIDENCE** | Attach only after Human68k evidence says X68000; D88 alone is not enough |
| HDI | Geometry/container evidence; no broad partition walk | **DEFER** | HDD boot/partition semantics and write safety need a separate lane |
| NHD | Geometry/container evidence; no broad partition walk | **DEFER** | Same limitation as HDI |
| HDF | PX68k ecosystem support is documented, but current EmuWiz identity/readiness is not sufficient | **DEFER** | Requires explicit HDD evidence and profile policy |
| raw floppy / multi-disk | Shared infrastructure varies | **DEFER** | No safe, format-specific binding contract yet |

Single-disk XDF/DIM is the narrowest safe launch. Multi-disk should be represented
by an explicit `.m3u`/command-file binding only after all members are verified and no
member is silently dropped. Do not invent runtime disk swapping.

## Firmware and configuration readiness

PX68k deployments conventionally use a RetroArch system directory named `keropi`.
The documented/commonly required assets include `iplrom.dat` (IPL ROM) and
`cgrom.dat` (character generator); exact hashes and additional optional ROMs must
come from the installed core/profile, not this audit. Readiness states should be:

* **VERIFIED** — profile-specific hash/manifest confirms the required ROM;
* **PRESENT_UNVERIFIED** — file exists but no trusted hash;
* **MISSING** — required file absent;
* **UNKNOWN** — profile cannot determine requirements;
* **NOT_REQUIRED** — profile explicitly does not use the asset.

Never download firmware, accept arbitrary blobs as verified, or substitute PC-98,
FM Towns, or another X68000 ROM set. RetroArch configuration should be read-only
for readiness; the launch adapter must not rewrite the user's config.

## CLI / argv findings

No local PX68k standalone executable or authoritative standalone CLI was found.
The verified invocation boundary is RetroArch:

```text
retroarch -L /path/to/px68k_libretro.so /path/to/game.dim
retroarch -L /path/to/px68k_libretro.so /path/to/game.m3u
```

The first form is a documented RetroArch content launch; the second is the
documented multi-disk route. A command-file form is also documented by Libretro
for two DIM members. These examples are conceptual argv elements for a future
RetroArch integration and must be resolved through the existing typed RetroArch
planner, never assembled as a shell string. Core options, BIOS directory, video,
fullscreen, and machine details belong to the selected RetroArch profile and are
not guessed here.

## Write safety and watched execution

PX68k may write guest disk state and RetroArch may write configuration, saves, and
core state. A future adapter must therefore:

1. preflight executable/core, profile, firmware, every content member, and identity;
2. launch a scratch copy for media that is not demonstrably read-only;
3. keep RetroArch config/save/state directories in an explicit profile destination;
4. use the existing watched-process abstraction;
5. recheck executable/content/profile identities immediately before spawn.

No safe read-only guarantee was established for guest writes. Directly launching
user XDF/DIM/HDI/NHD in place should be refused unless the selected profile proves
read-only behavior or a scratch-copy transaction is used. Source media must never
be modified by default.

## Existing evidence fit

* Strong Human68k/XDF/DIM evidence can bind **platform = Sharp X68000**.
* D88/HDI/NHD geometry alone remains family-only or ambiguous.
* Exact software identity remains DAT/hash-led and is not required to prove the
  platform, but is valuable for title-specific launch profiles.
* HDD launch should remain deferred until partition/boot evidence and scratch-copy
  policy are implemented.
* The generic RetroArch alternative must remain available and must not be silently
  replaced by PX68k merely because a core is installed.

## Recommended future V1

**RetroArch PX68k, single verified XDF/DIM floppy, explicit profile, verified IPL
and CG ROM readiness, typed argv, fresh preflight, watched process, and scratch-copy
or proven read-only media policy.**

Explicit exclusions: XM6/Wine; native standalone binary; HDI/NHD/HDF; arbitrary D88;
machine auto-selection; disk swapping automation; BIOS downloads; config rewriting;
title inference; source-media writes; and a new X68000 identity parser.

## Future test plan

The adapter lane should add tests for executable/core discovery, explicit profile
selection, verified XDF and DIM, wrong-platform refusal, generic D88 refusal,
missing/incompatible firmware, exact RetroArch argv, two-disk `.m3u` completeness,
media/profile/executable drift, scratch-copy/write protection, watched execution,
no shell invocation, and preservation of the existing RetroArch alternative.

## Value comparison

Scores are relative (10 = best value / readiness / safety; complexity is inverse,
where 10 = hardest to implement).

| Adapter | Value | Identity readiness | CLI quality | Firmware complexity | Media safety | Implementation complexity |
|---|---:|---:|---:|---:|---:|---:|
| Atari800 | 7 | 6 | 8 | 6 | 7 | 5 |
| NP2kai | 8 | 8 | 4 | 5 | 5 | 7 |
| BBC/b-em | 6 | 6 | 7 | 7 | 7 | 5 |
| Amstrad CPC | 6 | 5 | 7 | 7 | 7 | 5 |
| X68000/PX68k | 7 | 8 | 7 (RetroArch boundary) | 5 | 5 | 6 |
| openMSX follow-up | 6 | 4 | 8 | 4 | 6 | 5 |

X68000 should follow NP2kai/BBC once shared launch ownership is clear, unless the
project deliberately prioritises its already-strong Human68k evidence. PX68k gives
better media identity than many rare-platform lanes, but its RetroArch dependency
reduces the value of a separate native adapter.

## Final recommendation

Do not add a native XM6 adapter. When the launch surface is available, implement a
small PX68k RetroArch binding that consumes existing verified X68000 evidence and
uses explicit profile/firmware and scratch-copy policy. Until then, classify native
X68000 standalone support as absent and the existing generic RetroArch route as the
only supported launch path.

