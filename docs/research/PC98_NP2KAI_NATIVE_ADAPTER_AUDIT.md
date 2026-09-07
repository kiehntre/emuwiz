# PC-98 NP2kai native adapter audit V1

Date: 2026-09-07
Scope: research only. This document defines the gate for a future native
NP2kai adapter; it does not add an adapter, parse media, or authorise media
launch.

## Decision

**Preferred target: NP2kai's native Linux SDL/X ports, provisionally SOON,
not NEXT.** NP2kai is an actively maintained PC-9801-series emulator with
native Linux builds (`sdlnp21kai` and `xnp21kai`), a documented positional
media invocation, and a maintained upstream repository. It is a better target
than the original Neko Project II lineage, whose upstream is substantially
older and whose Linux CLI/config contract is less current. No other standalone
Linux target found in this audit had a stronger documented native contract.

It is **not safe to implement as a direct-image adapter yet**: the authoritative
documentation confirms runtime FDD/HDD swapping and configuration directories,
but does not document a standalone, command-line write-protect mode for D88,
HDI, or NHD. A future adapter must use a verified scratch-copy lifecycle for
all writable media, or refuse launch. It must also confirm its selected binary's
exact config-file argument against that binary's own `--help`/source before
constructing an argv that includes a config path.

## Sources and confidence

- NP2kai upstream README and build instructions: native Linux X/SDL builds,
  executable names, BIOS/config locations, positional media examples, and
  runtime FDD/HDD swapping: <https://github.com/AZO234/NP2kai>.
- Upstream change log says SDL2's config file is selectable on the command
  line, but the README does **not** publish a stable flag spelling. Therefore
  the exact config option is intentionally unresolved for V1.
- EmuWiz's existing bounded PC-98 evidence:
  `pc98_boot_evidence.rs`, `pc98_container_evidence.rs`, and validated
  `disk_format/{d88,hdi}.rs`.

This deliberately distinguishes documented facts from suggested integration
policy. Community command snippets and libretro behaviour are not used to
invent native flags.

## Candidate emulator comparison

| Candidate | Linux/native state | CLI/media evidence | Firmware/config | Write safety | Result |
| --- | --- | --- | --- | --- | --- |
| **NP2kai** | Current upstream supports native SDL/X Linux builds; executables include `sdlnp21kai` and `xnp21kai` | Positional media invocation documented; first floppy to FDD1, second to FDD2; HDD/CD selected by extension | BIOS and config live under per-port configuration directories; configurable file support exists but exact argument must be source-verified | No documented standalone read-only image flag found | **Choose, but gate on scratch copies/config isolation** |
| Original Neko Project II | Historical lineage; no stronger current Linux contract found | Insufficient current authoritative native CLI proof in this audit | Legacy configuration model | Not proven | Defer |
| Libretro NP2kai core | Linux-capable, but not a standalone adapter | Existing RetroArch route; disk-control semantics are core-specific | RetroArch BIOS/config model | Not a replacement for native write policy | Keep as an independent alternative |
| T98-Next/Anex86-style container tools | Container provenance, not a Linux PC-98 emulator target | N/A | N/A | N/A | Not emulator candidates |

## Native CLI contract

### What is documented

The upstream README documents an extension-routed positional invocation:

```text
<np2kai executable> <floppy-image> <hard-disk-image> <cd-image>
```

Its example mounts an `.fdi` in FDD1, an `.hdi` in HDD1, and an `.iso` in the
CD drive. A media list may place the first floppy in FDD1 and second in FDD2.
The exact native executable differs by installed frontend, so discovery must
consider only reviewed names such as `sdlnp21kai` and `xnp21kai`, plus an
explicit regular executable path; it must not assume a bare `np2kai` binary.

Future typed argv examples, conditional on a binary-version contract test:

```text
sdlnp21kai /scratch/game-1.d88
xnp21kai /scratch/game-1.d88 /scratch/game-2.d88
sdlnp21kai /scratch/system.d88 /scratch/game.hdi
```

These are argv vectors, never shell strings. They are only valid after the
binary's actual positional routing and config syntax are confirmed. V1 should
not attach a CD image: the present PC-98 identity/evidence path does not prove
CD semantics or establish a write-safe CD/profile contract.

### Explicitly not documented enough

- A stable native command-line flag to choose a config file, despite upstream
  saying SDL2 supports command-line config selection.
- A documented direct-image write-protect switch for floppy/HDD images.
- A stable CLI flag that selects PC-9801 vs PC-9821 machine generations.
- A documented fullscreen/windowed flag suitable for an invariant command
  plan.

Future implementation must reject unsupported binary variants rather than
guessing any of these flags.

## Machine/profile model

NP2kai describes itself as a **PC-9801 series** emulator. The minimum safe
initial model is one explicitly selected **PC-9801-compatible** profile, with
its CPU/memory/sound settings contained in the isolated reviewed profile.

PC-9821 must not be inferred from disk/container/title. It is a future separate
profile only when the selected NP2kai frontend exposes a stable, source-verified
machine setting and its specific BIOS assets can be checked. A profile choice
sets emulator configuration; it never strengthens EmuWiz's identity result.

No launch decision may use filename, folder, extension, disk name, or generic
FAT as evidence of machine generation.

## Existing EmuWiz evidence and media gate

| Media | What EmuWiz can currently prove | Launch status | Reason |
| --- | --- | --- | --- |
| D88 with strong PC-98 boot evidence | Valid D88 layout plus bytes-derived PC-98 boot/IPL evidence; bounded logical boot-sector location | **PROFILE_REQUIRED** | Floppy semantics are known, but future adapter must scratch-copy or prove write protection |
| D88 without strong PC-98 evidence | Shared Japanese container only | **REFUSE** | D88 is also PC-88, FM Towns, and X68000; container is not identity |
| HDI with strong PC-98 boot evidence | Valid HDI container plus NEC-specific boot evidence | **PROFILE_REQUIRED** | HDD is potentially writable; scratch-copy and profile contract needed |
| NHD with strong PC-98 boot evidence | Valid NHD container plus NEC-specific boot evidence | **NEEDS_MORE_EVIDENCE** | PC-98 evidence is sufficient, but this audit lacks upstream native NHD-attachment proof and write policy |
| Generic HDI/NHD/FAT | Container/geometry or generic FAT only | **REFUSE** | Does not prove PC-98 |
| FDI/HDM/XDF/other NP2kai formats | Emulator may accept formats | **DEFER** | Current EmuWiz identity/container safety is not equivalent for this adapter |
| ISO/CUE/CD | Upstream example establishes only general CD attachment | **DEFER** | No current PC-98 CD identity/firmware/read-only policy in this lane |

The strict eligibility predicate is:

```text
resolved strong PC-98 evidence
+ explicit compatible NP2kai profile
+ reviewed executable binding
+ supported media classification
+ ready firmware/config binding
+ verified scratch-copy or documented write protection
```

`NEC PC-9801` remains an existing equivalent identity spelling; the future
adapter should accept it only through the existing canonical-equivalence policy,
never by a second extension rule.

## Multi-disk

The upstream media-list documentation supports two initial floppies (FDD1 and
FDD2), while later swapping is a runtime UI action. No automatic swap protocol
is documented or appropriate.

Recommended implementation order:

1. **SINGLE_DISK** only, scratch copied.
2. Add **TWO_DRIVE** only after both selected media independently meet the
   PC-98 evidence and scratch-copy conditions, with explicit FDD1/FDD2 order.
3. **DEFER_MULTI_DISK** sets and runtime swaps. Show an explanatory blocker;
   never infer disk order from filenames.

## Firmware and configuration

Upstream lists `bios.rom`, `font.rom`/`font.bmp`, `itf.rom`, and `sound.rom`,
with extra 9821 assets described as optional/uncertain. The original-font path
is recommended; upstream instructs users to obtain BIOS files from their own
PC-98 hardware. EmuWiz must not download assets or promote names to hashes.

Future readiness values:

| Asset/profile observation | Readiness |
| --- | --- |
| Exact asset verified by a future authoritative verifier | VERIFIED |
| Required regular non-symlink asset is configured but unverified | PRESENT_UNVERIFIED |
| Explicit configured path absent/unsafe | MISSING |
| No selected profile, unknown frontend requirements, or unreadable config | UNKNOWN |
| Optional sound enhancement absent | NOT_REQUIRED for base boot; surface separately |

The native frontends use configuration directories such as
`~/.config/sdlnp21kai` and `~/.config/xnp21kai`, and the upstream material
shows that configuration is created/used. A future adapter must not point at or
silently rewrite that live state. It should use a generated isolated profile
only after source verification establishes the exact config argument for the
selected binary. If that cannot be proven, launch must refuse.

## Write safety

NP2kai exposes runtime FDD/HDD image swapping. This audit found no authoritative
standalone `--read-only`/write-protect CLI contract for D88, HDI, or NHD.
Accordingly direct preservation-image launch is prohibited.

Future safe path:

1. preflight validates source is a regular non-symlink file and captures its
   identity;
2. create a bounded, uniquely named scratch copy outside source roots;
3. capture the scratch identity and pass **only scratch paths** to NP2kai;
4. isolate config/CMOS/NVRAM/state writes in the same temporary session root;
5. watch the process and retain scratch/session results for inspection; never
   copy changes back automatically;
6. source identity must still match immediately before copying/spawn.

If an upstream supported frontend later proves a genuine read-only media mode,
it can replace scratch copying only after a focused source-immutability test.

## Future preflight and GUI contract

Pre-spawn checks must verify: exact executable remains a regular executable and
has unchanged captured identity; selected source remains a regular file and
unchanged; PC-98 evidence remains resolved; selected profile/machine remains
compatible; required firmware and generated config binding are ready; media is
one of the reviewed forms; and scratch-copy/session state is fresh. Any drift
fails closed.

The generic GUI projection should say:

```text
NP2kai
PC-98
Detected / Missing
Profile: PC-9801-compatible
Firmware: Present, not verified / Missing / Unknown
Media: PC-98 D88 (scratch launch required)
Launch: Ready / Needs setup
```

Identity is evidence about content; emulator availability and profile readiness
remain independent user-facing facts.

## Future test plan

1. native executable discovery and explicit binding;
2. explicit PC-9801 profile; PC-9821 only if its contract is proven;
3. strong PC-98 D88/HDI accepted into scratch planning;
4. NHD remains blocked pending NP2kai media proof;
5. generic D88/HDI/NHD and generic FAT refused;
6. wrong platform refused; existing RetroArch/MAME alternatives retained;
7. missing/incompatible firmware and profile/config drift block;
8. exact argv vectors for one floppy, two drives, and HDD only after direct
   upstream contract tests;
9. no auto disk swapping;
10. scratch copy is distinct, bounded, and source byte-identical before/after;
11. executable/content/config/scratch drift blocks;
12. watched process only, no shell;
13. no source-media, live-config, CMOS/NVRAM, or user state mutation.

## Ranking

Scores use 1 (poor/high risk) to 10 (ready/low risk), based on current
EmuWiz evidence rather than emulator popularity.

| Adapter | Identity readiness | CLI quality | Write safety | Firmware complexity | Implementation complexity | User value | Priority |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| NP2kai | 8 | 6 | 3 | 4 | 6 | 8 | **SOON** |
| Atari800 | 7 | 8 | 4 | 5 | 5 | 7 | SOON |
| BBC / b-em | 6 | 5 | 4 | 5 | 6 | 6 | LATER |
| Caprice32 CPC | 7 | 6 | 5 | 5 | 5 | 7 | SOON |
| X68000 / PX68k | 7 | 5 | 3 | 4 | 7 | 7 | LATER |

NP2kai should follow completion of a reusable scratch-media/config-isolation
primitive or a focused proof of NP2kai read-only operation. It should not wait
for new PC-98 identity parsing: the current strong boot evidence is already the
right eligibility anchor.

## Non-goals

- No PC-98 parser, DAT change, extension-only classification, title guessing,
  firmware download, emulator execution, media mutation, or conversion.
- No recommendation to bypass copy protection or repair images.
- No shared launch or GUI change in this audit.
