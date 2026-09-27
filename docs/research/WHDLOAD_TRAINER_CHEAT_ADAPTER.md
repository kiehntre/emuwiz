# WHDLoad Trainer / Custom-Option Cheat Adapter

## Scope

This feature treats WHDLoad trainer controls as launch-time configuration,
not as GameShark-style memory writes. It reads declarations from the selected
installed slave, validates user selections against those declarations, and
updates only an explicit EmuWiz-owned per-game option/tooltype layer. Slave
files, LHA packages, ROMs, disks, and global emulator configuration are never
modified.

The existing read-only projection remains the source boundary:
`parse_whdload_custom_options` accepts documented `C1`–`C5` declarations and
retains the raw specification. The adapter does not infer semantics from
labels such as “Infinite Lives”.

## WHDLoad option model

`TrainerOption` contains:

- `custom_slot`: canonical `CUSTOM1`–`CUSTOM5` slot;
- description and source slave provenance;
- typed kind and allowed values;
- current/default value;
- exact slave SHA-256 and verified EmuWiz game identity;
- readiness and provenance text.

`TrainerOptionValue` deliberately remains separate from memory-cheat
operations: Boolean, Numeric, Enum, Bitfield, and Opaque values are not
converted into addresses or writes.

The exact installed slave path, its SHA-256, verified game identity, and
optional package version form the target identity. A title-only match or a
different slave revision cannot apply.

## Supported CUSTOM declaration types

The WHDLoad `ws_config` autodoc defines:

- `B`: Boolean, stored as 0/1;
- `L`: list/cycle option, represented as deterministic enum choices;
- `M`: documented multi-bit list, represented as a bitfield with a declared
  bit range;
- `X`: documented single-bit Boolean, with the declared bit preserved inside
  the surrounding CUSTOM value;
- N: retained only as EmuWiz's internal bounded decimal compatibility
  representation; it is not an official ws_config declaration type;
- unknown/legacy syntax: visible as Opaque/Unsupported and refused for apply.

For `X` and `M`, applying one selection preserves unrelated bits in the same
CUSTOM slot. Invalid bit ranges, empty choice lists, out-of-range values, and
opaque syntax are blocking.

The GUI-v2 Cheats & Mods route now exposes a dedicated WHDLoad Trainer /
Custom Options panel. It renders controls only after an exact installed-slave
and EmuWiz-owned per-game configuration binding has been supplied. Until then
it shows the archive context and a blocked safety gate; an archive name is
never treated as a slave identity. Bound options use checkbox, enum, bounded
bitfield, or numeric controls according to the projected type, while opaque
declarations remain visible but non-applyable.

Primary references:

- [WHDLoad Usage and Options](https://www.whdload.de/docs/en/opt.html):
  local options, `CUSTOM1`–`CUSTOM5`, and the distinction between numeric,
  string, and switch options.
- [WHDLoad Resload API autodoc](https://www.whdload.de/docs/autodoc.html):
  `ws_config` grammar for `B`, `L`, `M`, and `X`, including list values and
  bit ranges.

## Apply model

The preview accepts an explicit `configuration_path` for an EmuWiz-owned
per-game launch/tooltype layer. It reads that bounded UTF-8 text file,
preserves unrelated lines/options, replaces selected CUSTOM entries, and
removes an entry when the option is disabled. New values are rendered as
ordinary WHDLoad arguments such as:

```text
CUSTOM1=1
CUSTOM2=5
```

The preview stages the resulting option file, then builds a typed
`PreviewAdapter::AmigaWhdloadTrainer` shared transaction. Apply uses the
existing atomic write, destination precondition, backup, journal, and exact
rollback machinery. History and backup roots must not overlap the source or
destination scope.

The current Amiga launch projection has no generic per-game tooltype writer:
Amiberry uses `--autoload`, while the existing FS-UAE projection carries the
documented WHDLoad argument channel. Therefore this adapter does not invent a
new emulator command-line switch. The rendered arguments are exposed for the
existing launch integration, and the GUI clearly identifies the operation as
launch-option-only until a selected launch profile supplies the option layer.
Apply rechecks the exact slave SHA-256 and configuration fingerprint captured
by preview before entering the shared transaction journal, refusing stale
external changes.

## Conflicts and refusal rules

The adapter reports typed conflicts for:

- two selections assigning different values to one CUSTOM slot;
- duplicate selections;
- invalid Boolean, numeric, enum, or bitfield values;
- a slot absent from the selected slave declaration;
- opaque or unsupported declaration syntax;
- changed slave bytes or changed expected configuration bytes.

There is no automatic fallback to another slave, global `S:WHDLoad.prefs`,
another game with a similar name, or a trainer executable.

## GUI-v2

Amiga routes now identify the native format as **WHDLoad CUSTOM/tooltype
options**. The Cheats page presents the distinction:

> These options come from the installed WHDLoad slave. EmuWiz updates launch
> options only; it does not modify the game files.

It also labels the operation **Apply: updates per-game launch options only**.
The generic GUI workflow does not claim that a native WHDLoad apply editor is
available until the launch context exposes the exact installed slave and
EmuWiz-owned option file.

## Extension research

ScummVM has game/debug toggles and DOSBox-family front ends have per-game
configuration keys, but neither was found in the current EmuWiz launch model
with a comparable, documented, identity-bound trainer declaration plus an
existing safe config writer. They are not implemented here. Reusing the
WHDLoad model for them would risk treating emulator settings or debug
switches as game trainers.

## Legal/source mode

Only installed local slave metadata and user-owned local configuration are
used. No trainer executables, proprietary databases, scraping, downloads, or
external scripts are added. No network activity is required by the adapter.

## Limitations

- WHDLoad itself does not assign meanings to `CUSTOM1`–`CUSTOM5`; the slave
  author’s documented declaration is authoritative.
- The current launcher integration still needs a per-game option-layer binding
  to expose a complete interactive GUI editor for every Amiberry/FS-UAE
  profile.
- Global `S:WHDLoad.prefs`, Workbench icon tooltypes, and undocumented custom
  syntax remain outside the apply contract.
