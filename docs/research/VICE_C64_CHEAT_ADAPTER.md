# VICE C64 native cheat / POKE adapter

## VICE mechanism

VICE's [monitor manual](https://vice-emu.sourceforge.io/vice_12.html)
documents the `>` memory-write command and the C64 bank/address-space model.
The [settings manual](https://vice-emu.sourceforge.io/vice_6.html) documents
`-moncommands <Name>`, which executes a command file in the monitor after
startup. EmuWiz projects commands as an EmuWiz-managed session artifact and
does not edit VICE resources or the user's global configuration.

The projection emits:

```text
radix H
> C000 FF
x
```

The command file is inspectable before launch. VICE documents that monitor
commands run before the kernel reset sequence, so a command can be overwritten
by program startup. This is surfaced as a launch-timing warning; the adapter
does not claim persistent in-game trainer behavior. VICE's binary monitor also
has a documented memory-set protocol, but implementing a live transport is
outside this adapter.

## Supported operations and memory model

Only existing neutral `CheatOperation::Write8` operations are projected.
16/32-bit writes, arbitrary BASIC, scripts, executables, and unproven compare
guards remain preview-only.

Targets are classified conservatively:

- normal RAM: previewable for launch review;
- colour RAM: distinct C64 target;
- `$D000-$DFFF` I/O: warning and no automatic application;
- `$A000-$BFFF` / `$E000-$FFFF` ROM-mapped: bank ambiguity warning;
- `$8000-$9FFF` cartridge/banked: bank ambiguity warning;
- outside 16-bit C64 address space: rejected.

## Identity and persistence

Identity evidence accepts exact media hash, verified game identity, exact
program identity, title-only, or unknown. Title-only and unknown evidence are
preview-only. The projection is runtime-only and never writes D64/T64/TAP/PRG/
CRT media or VICE global configuration. No persistent enablement or rollback
claim is made because no destination file is written.

## GUI and legal boundary

GUI-v2 exposes a C64/VICE POKE preview with operation count, memory target,
runtime-only status, and warnings. Sources remain local/manual imports only;
no trainer executable, web scraping, or database bundling is introduced.

The repository currently has no neutral C64 POKE parser on this branch. This
adapter therefore consumes existing `CheatOperation` values and intentionally
does not duplicate the classic POKE-family parser/model.
