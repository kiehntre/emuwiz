# Mednafen Native Cheat Adapter

## Scope and source boundary

This adapter is local/user-import only. It does not download cheat databases,
upload hashes, modify ROM/disc media, execute expressions, or change Mednafen's
global configuration. It targets the native per-system `.cht` files documented
by current Mednafen documentation and uses EmuWiz's existing preview,
transaction, journal, and rollback primitives.

Primary references:

- [Mednafen 1.32.1 cheat documentation](https://mednafen.github.io/documentation/cheats.html)
- [Mednafen 1.32.1 configuration documentation](https://mednafen.github.io/documentation/settings.html)
- [Mednafen source: default cheat path and game-load integration](https://github.com/zeromus/mednafen/blob/master/mednafen.cpp)
- [Source package copy of `Documentation/cheats.txt`](https://sources.debian.org/src/mednafen/1.32.1%2Bdfsg-3/Documentation/cheats.txt)

Mednafen is GPL-2.0-or-later. No third-party cheat collection is bundled.

## Mednafen format

Mednafen stores cheats in a text file per system under the configured cheat
directory (`filesys.path_cheat`, whose default is `cheats`). A current native
file uses MD5-keyed sections:

```text
[32-character-md5] Display title
R A 1 L 0 001f006d 09 Infinite lives
```

The documented operation families are:

- `S`: substitute a value on memory read;
- `C`: compare before substituting, retaining a compare condition;
- `R`: replace the value before vertical blank;
- `A` / `I`: active or inactive state;
- width, endian, reserved field, address, value, optional compare value, and
  description fields.

The parser bounds total bytes, lines, line length, and entries. Unknown widths
and native/conditional operations remain represented as native operations; they
are never fabricated as unconditional memory writes. Names, comments, section
identity, and enabled/disabled state are retained where the native format
provides them. Rendering is deterministic.

Mednafen loads the cheat file during game load. Applying a changed file therefore
requires a restart or reload; the adapter does not pretend that changing the
file changes a running game immediately. The global `cheats` setting and
interactive cheat console are not silently modified.

## Supported systems

The adapter deliberately covers the requested priority systems for which the
current Mednafen module naming and generic cheat file model are usable:

| EmuWiz family | Mednafen system file/module | Status |
| --- | --- | --- |
| PC Engine / TurboGrafx-16 / SuperGrafx | `pce` (with `pce_fast` accepted as a native system identifier) | Native adapter |
| PC Engine CD | `pce` | Native adapter; disc identity is supplied by the caller |
| PC-FX | `pcfx` | Native adapter |
| Virtual Boy | `vb` | Native adapter |
| Atari Lynx | `lynx` | Native adapter |
| WonderSwan / WonderSwan Color | `wswan` | Native adapter |
| NES / Famicom | `nes` | Native adapter |
| SNES / Super Famicom | `snes` and `snes_faust` | Native adapter |
| Game Boy / Game Boy Color | `gb` | Native adapter |
| Game Gear | `gg` | Native adapter |
| Mega Drive / Genesis | `md` | Native adapter |
| PlayStation 1 | `psx` | Native adapter |
| Master System | `sms` | Native adapter |

The native parser is not a claim that every current Mednafen module has an
EmuWiz platform mapping. Modules outside this matrix remain unsupported by the
adapter until their identity and launch mapping are verified. This avoids
turning an old, similarly named Mednafen module into an accidental support
claim.

## Identity and memory model

An MD5 section is an exact native target identity, not merely a display label.
EmuWiz requires a verified local identity and a valid 32-hex-character MD5
before it builds an apply plan. A title alone is never sufficient. The caller
must also select the exact Mednafen system, so a section for one system cannot
be installed into another system's cheat file.

The adapter does not transmit or publish the MD5. It is used only in local
preflight and provenance.

Each normalized operation records:

- Mednafen system memory as the memory space;
- native address and width;
- endian marker;
- operation family (`S`, `R`, or `C`);
- original native text.

`S` and `R` direct writes are normalized only when their width is 8, 16, or 32
bits and the value fits. `C` operations retain their compare condition as a
native conditional operation and are passed to compatibility analysis as
unknown rather than being flattened. Wider or otherwise unsupported operations
are opaque. Compatibility identity includes the Mednafen system and memory
space, so equal numeric addresses in NES and PC Engine files do not conflict.

## Parser, merge, and writer

Parsing is bounded and deterministic. Malformed input, missing section headers,
invalid MD5s, invalid state tokens, unsupported operation kinds, and oversized
input fail safely. The adapter never evaluates arbitrary expressions.

An apply plan reads the existing destination only after rejecting a symlink.
The selected exact-MD5/name entry is replaced, or appended if new; unrelated
game sections and comments remain. The generated bytes are reparsed before the
shared preview is accepted. The native `.cht` file is therefore merged rather
than wholesale replaced.

## Apply, rollback, and loadability

Preflight requires:

- selected emulator/profile to be Mednafen at the caller's routing layer;
- exact supported system;
- verified game identity and exact MD5;
- safe absolute profile/staging roots and a non-symlink destination;
- unchanged source preview and destination preconditions;
- successful reparse of generated output.

The shared transaction publishes `cheats/<system>.cht` atomically, records the
prior bytes, and supports exact rollback. If the destination was modified
outside the journal after apply, destructive rollback is refused by the shared
transaction layer. A newly created file is removed only while its transaction
fingerprint still matches.

Loadability facts expose the system, expected cheat path, MD5, persistent native
state, and the restart requirement. Persistent enabled/disabled state is the
native `A`/`I` field; there is no invented runtime-only persistence model.

## GUI and compatibility boundary

The existing shared cheat preview now labels Mednafen as a native cheat adapter
and explains that conditional/opaque operations remain native. The core adapter
exports a loadability model and compatibility report, while generic selected-
emulator routing remains outside this focused task. This keeps Mednafen support
available to the existing Cheats workflow without redesigning routing.

The GUI should show “Mednafen native cheat”, the verified identity, system, and
that a restart/reload is required. A title-only or stale identity is a warning
for display but remains blocked for apply.

## Explicit refusals and limitations

- No title-only unattended apply.
- No cross-system compatibility inference.
- No unconditional interpretation of compare/conditional codes.
- No arbitrary expression execution.
- No automatic modification of the global `cheats` setting.
- No claim of support for Mednafen modules not in the verified matrix.
- The adapter does not add a database browser or acquire codes.
- A running Mednafen process must be reloaded/restarted through the existing
  launch workflow; the adapter does not signal or kill it.

