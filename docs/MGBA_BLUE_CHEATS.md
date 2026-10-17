# Standalone mGBA cheats in Blue

In Cheats & Mods, select a verified loose Game Boy Advance game, explicitly
choose mGBA in the existing emulator chooser, then choose a local `.cheats`
file. Supply the exact ROM SHA-256 declared by the cheat source. A title,
filename or region is not accepted as that binding. This local declaration
is not publisher authentication or proof that a code is correct in-game.

The panel reuses existing native mGBA discovery, executable binding and the
shared AUTO/remembered profile selector. An unavailable remembered profile
refuses instead of falling back. mGBA does not have the newer shared FORCED
installation policy or native Blue launch execution integration; this panel
does not add either, persist a preference, or switch Launch's emulator.
Use Launch Readiness separately. No installed emulator is executed by inspection.

Select entries explicitly; selection and enablement start off. Preview shows
the actual selected sets, code lines and enable states, executable/profile,
exact new destination, autoload setting and the no-replacement restriction.
Confirm that review and that mGBA is closed before Install. Success means
verified file installation, never verified in-game effectiveness. No config,
ROM, BIOS, save or emulator-selection file is written.

## Bounded supported installation

- Native Linux mGBA with one safe executable and an existing active `config.ini`.
- Existing explicit absolute `cheatsPath` from global configuration, with
  any Qt override agreeing. Qt-only or other-port targets refuse. Destination directory must already exist without symlinks.
- Direct `.gba` source with verified loose-ROM SHA-256 evidence matching the
  cheat-source declaration. Source, executable, config and directory evidence
  are revalidated at Apply; the shared writer also checks materialized guards.
- Directive-free native sets of VBA `address:byte` writes into GBA EWRAM/IWRAM.
  Duplicate selected entries or writes to the same address refuse. Malformed or
  truncated native input refuses. The existing parser/renderer is reused.
- Creates `<ROM basename>.cheats` only if absent. Existing cheat files are never
  merged or replaced. Source-native files and their original enable flags are
  unchanged; the reviewed output contains only the explicitly selected states.

Opaque GameShark/Action Replay/CodeBreaker, ROM hooks, other native directives,
Libretro/EZ Flash input, GB/GBC, portable/Flatpak/custom-root profiles, backup
configs, ambiguous executables, guessed/default/relative paths and uncertain
identity remain unavailable for installation. Inspection can display unsupported
entries. Configure the explicit cheat directory in mGBA itself when needed;
EmuWiz does not edit its configuration.

## History and Undo

New installs use the existing shared transaction journal, source/output checks,
no-overwrite publication, readback and durable recovery evidence. No second
journal or in-memory-only rollback mechanism is introduced. History/Undo is
available in the panel after restart via Refresh and in existing shared History.
Retained pending records are displayed for operator review, and refuse new
installation for that ROM. Corrupt or unreadable pending evidence also refuses;
no interrupted Apply resume is added.
Undo is separately previewed and confirmed; changed files, missing/inconsistent
records and unsupported recovery states refuse. Partial/failed operations remain
visible and are never presented as complete. Keep journals and backups.

This reuses the shared transaction contract; it is not R2 or a descriptor-bound
filesystem upgrade. Close mGBA to avoid its autosave interfering. Preflight
checks do not exclude arbitrary external/hostile concurrent pathname changes.
No runtime-effectiveness, power-loss or new Undo guarantee is asserted.

## Source evidence

The existing native parser remains the format implementation. Read-only wiring
was checked against mGBA 0.10.5 upstream:

- [Native parsing and set enable state](https://github.com/mgba-emu/mgba/blob/0.10.5/src/core/cheats.c)
- [GBA VBA parsing and RAM versus ROM writes](https://github.com/mgba-emu/mgba/blob/0.10.5/src/gba/cheats.c)
- [Configuration lookup and cheatsPath](https://github.com/mgba-emu/mgba/blob/0.10.5/src/core/config.c)
- [Filename suffix and directory mapping](https://github.com/mgba-emu/mgba/blob/0.10.5/src/core/directories.c)
- [Autoload/autosave](https://github.com/mgba-emu/mgba/blob/0.10.5/src/core/core.c)

Unknown installed versions are not represented as tested runtime compatibility.
