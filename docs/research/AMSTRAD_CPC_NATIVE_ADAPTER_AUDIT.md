# Amstrad CPC Native Emulator Adapter Audit V1

**Date:** 2026-09-07
**Repository:** EmuWiz (`feature/archivefs-unified-platform`)
**Scope:** read-only audit; no adapter implementation

## Executive conclusion

EmuWiz currently has no dedicated Amstrad CPC standalone adapter. CPC content can
be recognised by the identity and tape/audio layers and can use the generic
RetroArch route when an installed core advertises a compatible platform, but
`launch/platform_map.rs` has no CPC standalone row and no CPC-native command or
readiness module exists.

There is a suitable native target for a future lane: **Caprice32 (`cap32`)**. It
is the best balance of maintained upstream source, Linux support, explicit CPC
machine coverage, documented command-line loading, and broad media support. A
safe V1 should still be deliberately small: explicit CPC model profiles, verified
DSK/CDT (and later snapshots), typed argv, a read-only/scratch-copy policy, and
fresh preflight. It must not infer a model or platform from a suffix.

Caprice32 is a good *candidate*, not an installed dependency: no CPC emulator
binary was found on this host. Installation, ROM acquisition, and a real launch
smoke test remain prerequisites for an implementation lane.

## What is already in EmuWiz

The current platform registry calls the canonical platform **Amstrad CPC**. It has
folder aliases such as `amstrad`, `amstradcpc`, `cpc`, `cpc464`, and `cpc6128`,
but no filename aliases or strong extensions. Its weak extensions include `cdt`,
`dsk`, `sna`, `tap`, and `tzx`, reflecting the real format collisions.

The registry gives CPC the same corroborating `ZXTape!` signature as ZX Spectrum.
That signature proves a TZX/CDT container, not which machine produced it; folder,
DAT, and structural/timing evidence must resolve the platform. The explanation
in `platform/mod.rs` explicitly says that a `.dsk` is shared with BBC Micro, Atari
ST, Apple II, PC-98 and other systems.

The disk layer validates standard and extended CPCEMU DSK structure, walks bounded
track descriptors, and recognises a Spectrum +3/PCW disk specification when its
geometry and checksum agree. A bare valid CPCEMU container remains ambiguous. It
does not provide a CPC platform claim merely from the container magic.

The tape layer includes CDT/TZX structural evidence and standard/custom CPC WAV
analysis. V9/V10-style CPC recovery can expose leader, sync, CRC, block metadata,
stage boundaries and generic custom timing, but this is identity/evidence work,
not a native launch adapter. `.wav` is therefore evidence input, not a safe direct
Caprice32 launch input.

Snapshot inspection is currently ZX Spectrum-specific (`.sna`, `.z80`, `.szx`),
so a CPC adapter must not treat every SNA as CPC. CPR is a CPC Plus/GX4000
cartridge format in CPC emulators, but it is not currently a strong CPC identity
signal in the EmuWiz registry. No CPC-specific CPR readiness path is justified by
this audit.

## Local inventory

The required read-only checks returned no paths for:

```text
cap32 caprice32 arnold cpcec cpcemu
```

No matching CPC Flatpak or Debian package was present in the local inventory.
This means the audit cannot claim local executable detection, version probing, ROM
readiness, or a physical CPC launch. It also means a future adapter must treat
absence as `MISSING_EXECUTABLE`, not silently fall back to an invented command.

## Candidate comparison

| Candidate | Linux story | Machine/media coverage | CLI and readiness confidence | Maintenance/practicality | Audit result |
|---|---|---|---|---|---|
| **Caprice32** | Current upstream documents Linux builds and releases; GPLv2 source | CPC464/664/6128; partial Plus/GX4000; DSK/IPF/CT-RAW, VOC/CDT, CPR, SNA, ZIP | `cap32 [options] [FILE]...`; `--cfg_file`, `--override`, `--help`, `--version`; files are passed as full paths | Active upstream repository (README copyright through 2025), broad and familiar | **Preferred target** |
| **CPCEC** | SDL2 build supports multiple operating systems; source documents Unix configuration and past GNU/Linux builds | CPC464/664/6128, Plus, GX4000; CDT/CSW/WAV tape, DSK, CPR, SNA and more | Compact switches are documented (`-m0..3`, `-h`, `-W`, etc.); file arguments auto-load by type | Feature-rich, but current Linux packaging/executable discovery and write policy need a fresh build proof | Strong alternative, not selected for V1 |
| **Arnold** | Unix/Linux SDL and GTK ports exist | CPC464/664/6128, CPC+, KC Compact; DSK, CDT/TZX, CPR, SNA | Explicit options are documented (`-drivea`, `-driveb`, `-tape`, `-cart`, `-cpctype`, `-snapshot`) | Linux port is explicitly described upstream as work in progress; documentation is old | Research fallback only |
| **ByteBox** | Linux AppImage and source build are documented | CPC 6128 focus; disk loading and an `--autocmd` option | Very simple `--disk`/`-d` contract | Recent and interesting, but only one machine and disk-first scope; tape/snapshot/ROM readiness are not yet a complete V1 contract | Watch, not V1 |

### Why Caprice32 wins

Caprice32's current upstream README says it runs on Linux, macOS and Windows and
faithfully emulates CPC464, CPC664 and CPC6128. The same document lists DSK,
IPF, CT-RAW, VOC, CDT, CPR and SNA support, with partial Plus-range support. Its
manual describes a stable positional media contract: one or two disk files fill
drive A and B, while the full path must be supplied on the command line. The
manual also documents `--cfg_file`, `--override`, `--help`, and `--version`.

That is a better foundation for a typed EmuWiz adapter than an emulator whose
Linux port is explicitly unfinished, or a newer project that currently targets
only CPC6128 disks. Caprice32 still has configuration and ROM-path state, so
“binary found” cannot be treated as “ready”.

Sources: [Caprice32 README](https://github.com/ColinPitrat/caprice32/blob/master/README.md),
[Caprice32 manual](https://github.com/ColinPitrat/caprice32/blob/master/doc/man.html),
[CPCEC manual](https://github.com/AmatCoder/CPCEG/blob/master/cpcec-e.txt),
[Arnold README](https://github.com/rofl0r/arnold/blob/master/readme.txt),
[ByteBox README](https://github.com/nicolasbauw/amstrad_cpc/blob/master/README.md).

## Machine model and profiles

The adapter should expose explicit profiles, not guess from filenames:

| Profile | Caprice32 evidence | V1 recommendation |
|---|---|---|
| CPC 464 | Fully emulated; `system.model` can be selected through the documented config override mechanism | Include, if the user selects it or existing verified metadata requires it |
| CPC 664 | Fully emulated | Include as explicit selection; do not infer it from a disk |
| CPC 6128 | Fully emulated and the safest default *only when the user chooses a default* | Include; likely first smoke profile |
| CPC Plus / CPC 464+ / 6128+ | Partial Plus support in Caprice32 | Defer until separate identity and ROM tests exist |
| GX4000 | A Plus-range machine, not interchangeable with a CPC6128 | Keep separate in any future taxonomy; defer V1 |

Machine identity is a launch configuration, not a filename property. A DSK can
be used by more than one CPC model; the adapter should report `PROFILE_REQUIRED`
when no explicit model is available rather than silently choosing 6128.

## EmuWiz media evidence versus launch safety

| Media | Current EmuWiz evidence | Caprice32 capability | Future V1 decision |
|---|---|---|---|
| Standard/extended `.dsk` | CPCEMU structure is validated; bare container is shared and can remain ambiguous | Documented disk image input; two drives | **Safe only after platform evidence + explicit model** |
| `.cdt` | Shared TZX/CDT container; CPC context and CPC waveform evidence can corroborate | Documented tape image input | **Safe after CPC context; never extension-only** |
| `.tzx` | Shared with Spectrum; signature is corroborating only | Caprice32's manual lists CDT, not TZX as a distinct CPC contract | **Defer unless the container/context proves CPC and launch is tested** |
| `.csw` | CPC tape evidence is not a dedicated direct-launch contract | Not listed in Caprice32's README/manual media list | **Defer** |
| `.wav` | CPC WAV decoder supplies evidence and provenance | Not a documented direct slot input | **Evidence only; no conversion in adapter V1** |
| `.sna` | Current SNA parser is Spectrum-specific | Caprice32 lists SNA snapshots | **Defer until CPC snapshot identity is implemented** |
| `.cpr` | CPC Plus/GX4000 identity is not safely separated today | Listed as cartridge input | **Defer with Plus/GX4000** |
| `.ipf` / `.raw` | No CPC-specific EmuWiz identity contract | Caprice32 lists both | **Defer; preservation format support is not identity proof** |

The practical V1 intersection is therefore **verified CPC DSK and, after a
separate launch smoke, CPC CDT**, with an explicit machine profile. A valid DSK
in a folder named `cpc` can be a useful user hint, but a future launch planner
must retain the existing identity confidence and refuse unresolved collisions.

## Firmware and configuration readiness

Caprice32 looks for a configuration file in documented locations, including an
explicit `--cfg_file` path, XDG configuration, `$HOME/.cap32.cfg`, and `/etc`.
The configuration's `rom_path` points to system ROMs. The project does not make
ROM filenames or hashes a universal public contract, so an adapter should not
invent names such as “CPC6128.ROM” or download firmware.

Future readiness should distinguish:

* `VERIFIED`: selected profile's required ROM/config is present and validated by
  the adapter's safe probe;
* `PRESENT_UNVERIFIED`: a candidate file exists but identity/hash is not known;
* `MISSING`: the required system ROM/config cannot be found;
* `UNKNOWN`: the installation layout cannot be inspected safely;
* `NOT_REQUIRED`: only for a proven emulator mode that genuinely embeds the
  needed firmware (not established for Caprice32 here).

A profile-specific ROM hash catalogue would be a later, separately researched
task. Until then, `PRESENT_UNVERIFIED` must not be displayed as fully ready for a
preservation-grade launch.

## Exact CLI contract (verified upstream)

The Caprice32 manual establishes the following safe, typed pieces:

```text
cap32 [OPTION]... [FILE]...
cap32 --help
cap32 --version
cap32 --cfg_file=/path/to/cap32.cfg /path/to/game.dsk
cap32 --override system.model=... /path/to/game.dsk
cap32 /path/to/drive-a.dsk /path/to/drive-b.dsk
cap32 /path/to/game.cdt
```

The manual states that the first two disk files populate drive A and B and that
full paths should be supplied. It documents `--override` for configuration
options, but the exact numeric `system.model` values and their mapping should be
read from the installed profile/config contract before implementation; this
audit deliberately does not turn an unverified folklore value into a launch rule.

The command contract does **not** establish a safe Caprice32 flag for fullscreen,
autostart, drive swapping, or read-only media. These must remain configuration or
runtime UI concerns until an installed build proves them. The future adapter must
use `std::process::Command`/typed argv through the existing launch executor, never
a shell string.

Arnold and CPCEC expose more model switches in their manuals, but selecting one
because it has a convenient flag would be premature: neither has been validated
locally in this repository, and neither is the preferred V1 target.

## Write-back and preservation safety

Caprice32's manual says its configuration can be saved from the GUI and that
disks are mutable emulator media. It also describes normal disk images and a
configuration `dsk_path`; it does not establish a command-line read-only switch
in the reviewed contract. Therefore a future adapter must:

1. open source media through EmuWiz's existing read-only validation;
2. launch a scratch copy when the emulator may write a DSK;
3. keep the original path bound in the selected-game identity;
4. make the scratch-copy decision visible in readiness/preview; and
5. refuse launch if a safe copy cannot be made.

The same policy applies to CDT/other images if the emulator can record or save
them. Do not rely on a GUI default, file permissions, or an undocumented flag as
preservation protection. Snapshots and configuration should be written only to a
managed user/runtime location, never beside the preserved source by accident.

## Two drives and multi-disk sets

Caprice32 documents two disk drives and positional A/B loading. This is enough
for a conservative V1 preview: a verified two-disk set can produce an A/B argv
plan. Automatic disk swapping, side flipping, M3U interpretation, and emulator
keyboard automation are not established by the contract and should be deferred.

If a set has three or more disks, or ordering is not proven by existing set
identity, keep it as a multi-file set requiring explicit user selection. Never
choose an arbitrary first file merely to make the game appear launchable.

## Recommended future adapter design

The smallest trustworthy implementation would add one Caprice32 adapter behind
the existing launch/readiness architecture:

* executable discovery for `cap32`, with safe `--version`/`--help` probes;
* explicit CPC464, CPC664 and CPC6128 profiles;
* strict platform/content identity gate before candidate creation;
* DSK A/B planning and CPC CDT planning only after media intersection tests;
* configuration/ROM readiness with the five states above;
* typed argv and the shared process executor;
* scratch-copy/read-only protection for writable media;
* fresh preflight immediately before spawn, rechecking executable, profile,
  source identity and scratch binding;
* normal launch result/error projection, not a CPC-specific GUI page.

Explicit exclusions: no new DSK/CDT parser, no WAV conversion, no CPC Plus or
GX4000, no filename/model guessing, no automatic disk swapping, no firmware
download, no shell execution, no native write-back to preserved media, and no
claim that a generic DSK is CPC without independent evidence. RetroArch remains
the valid alternative for content that is recognised but outside this native
intersection.

## Future test plan (not implemented in this audit)

The adapter lane should use synthetic paths and disposable profiles, then a
physical launch smoke with a legally held test image:

1. executable found, missing, and version probe failure;
2. explicit CPC464/664/6128 profile selection;
3. verified DSK A and DSK A+B argv exactness;
4. verified CPC CDT launch after platform-context gating;
5. ambiguous DSK, Spectrum +3 DSK, and wrong-platform refusal;
6. unsupported TZX/CSW/WAV/SNA/CPR behavior;
7. missing, present-unverified, and mismatched ROM/config states;
8. source media remains byte-identical after a launch attempt;
9. scratch-copy write path and cleanup/recovery;
10. executable, content, and profile drift between preview and spawn;
11. no shell metacharacter interpretation;
12. two-disk ordering and refusal of unproven multi-disk order;
13. RetroArch candidate remains available when native readiness is absent;
14. CPC Plus/GX4000 are not collapsed into CPC6128.

## Ranking against deferred native adapters

Scores are relative audit judgements (1 low, 7 high), not benchmark results.

| Platform/target | Identity readiness | Native CLI quality | Media safety | Firmware complexity | Implementation complexity | User value | Recommendation |
|---|---:|---:|---:|---:|---:|---:|---|
| **Amstrad CPC / Caprice32** | 5 | 6 | 4 | 4 | 5 | 6 | **SOON** |
| Atari 8-bit / Atari800 | 5 | 6 | 5 | 4 | 5 | 6 | Next if active adapter lane completes |
| PC-98 / NP2kai | 4 | 4 | 4 | 3 | 6 | 5 | Later; identity and profile complexity remain |
| BBC Micro / b-em | 5 | 5 | 5 | 4 | 4 | 5 | Soon, comparable to CPC |
| X68000 / PX68k fallback | 5 | 2 | 4 | 3 | 6 | 4 | Later; fallback is currently more practical than native |
| MSX / openMSX | 6 | 6 | 5 | 4 | 3 | 6 | **Completed** |

CPC should be **SOON**, not “next at any cost”. Atari800 has an active adapter
lane and should not be displaced by a documentation audit. BBC/b-em is similarly
credible. NP2kai and X68000 need more conservative identity/profile work. openMSX
is already complete and is not a competing implementation task.

## Final audit status

* **Native CPC support:** genuinely absent in current EmuWiz; no dedicated
  adapter, readiness row, command planner, or local executable was found.
* **Preferred target:** Caprice32, conditional on building/probing it in a future
  lane. CPCEC is the strongest alternative; Arnold is not sufficiently current
  for first choice; ByteBox is promising but too narrow today.
* **Safe V1:** explicit CPC464/664/6128, verified DSK and tested CDT, typed argv,
  ROM/config readiness, scratch-copy protection, and shared executor.
* **Deferred:** TZX/CSW/WAV direct launch, CPC snapshots, CPR/Plus/GX4000, IPF/
  CT-RAW, automatic swapping, and any extension-only identity.
