# mGBA Native Cheat Adapter

## mGBA format and source references

The adapter targets mGBA's native `.cheats` file, not a provider catalogue or
RetroArch's `.cht` writer. Current mGBA source defines a line limit of 512 and
an entry limit of 1000 in `src/core/cheats.c`; it reads `!` directives, uses
`#` lines as named cheat sets, preserves code lines, and writes the same shape
back out. `!disabled` applies to the next set. The native autosave suffix is
`.cheats` and the file is opened from mGBA's configured cheats directory.

References:

- [mGBA core cheat parser/writer](https://github.com/mgba-emu/mgba/blob/master/src/core/cheats.c)
- [mGBA directory model and `.cheats` autoload](https://github.com/mgba-emu/mgba/blob/master/src/core/core.c)
- [mGBA GBA cheat types](https://github.com/mgba-emu/mgba/blob/master/include/mgba/internal/gba/cheats.h)
- [mGBA GameShark implementation](https://github.com/mgba-emu/mgba/blob/master/src/gba/cheats/gameshark.c)
- [mGBA CodeBreaker implementation](https://github.com/mgba-emu/mgba/blob/master/src/gba/cheats/codebreaker.c)
- [mGBA project licence](https://github.com/mgba-emu/mgba/blob/master/LICENSE)

The source also accepts Libretro-style `cheats = N` and EZ Flash CHT input
through separate parsers. This task deliberately does not turn those external
formats into a second writer; the adapter preserves opaque/native lines and
leaves format-specific decoding to mGBA or a separately approved decoder.

## Support matrix

| Code type | Parse/retain | Preview | Normalize | Write/merge | Enable state | Apply |
|---|---:|---:|---:|---:|---:|---:|
| mGBA native set/directives | yes | yes | directives only | yes | `Enabled`/`Disabled` | yes with verified identity |
| VBA `address:value` | yes | yes | proven 8-bit write | yes | native set state | yes with verified identity |
| GameShark / GSAv1 | yes | yes | opaque in this adapter | yes | native set state | yes, mGBA interprets it |
| Pro Action Replay / ARv1-3 | yes | yes | opaque in this adapter | yes | native set state | yes, mGBA interprets it |
| CodeBreaker | yes | yes | opaque in this adapter | yes | native set state | yes, mGBA interprets it |
| Libretro `.cht` / EZ Flash CHT | not selected as native input | no | no | no | no | no |

The current branch has no approved GBA Action Replay/GameShark decoder to
reuse. Consequently, the adapter never fabricates address ranges for those
codes. This is intentional: mGBA's own implementation includes conditional,
pointer, hook, encrypted CodeBreaker, and ROM-patch semantics that are not
equivalent to a direct write.

## Parser and model

`MgbaCheatFile` contains ordered `MgbaCheatEntry` values. Each entry retains
its name, directives, format classification, enable state, and every original
code line. Limits are bounded at 1 MiB per file, 1000 entries, 128 code lines
per entry, and 512 bytes per line. Malformed or unknown lines become typed
issues but remain in the model; they are never silently discarded.

Rendering is deterministic and emits the native directive/name/code ordering.
Merging compares the format, name, and exact code lines, so a duplicate is
not added while unrelated entries remain unchanged.

## Identity and readiness

Apply requires either an exact ROM SHA-256 match or a verified EmuWiz game
identity. Provider-declared and title-only evidence remain useful display
metadata but produce `IdentityUnverified` for unattended apply. No ROM is
modified and no hash is sent outside the process.

## Apply and rollback

`build_mgba_cheat_apply_plan` reparses the deterministic output before it can
be applied. Apply checks the destination fingerprint again, rejects symlink
targets, writes a same-directory temporary file, flushes it, and atomically
renames it into place. The receipt retains the exact previous bytes. Rollback
restores those bytes, or removes a newly created file, only when the current
destination still has the adapter's output hash; external modification blocks
rollback.

The typed plan is intentionally local and mGBA-specific. Generic cheat routing,
provider retrieval, compatibility analysis, and emulator process control are
not changed. The existing GUI can expose these facts through the adapter seam;
this foundation does not add a database browser or automatic acquisition.

## GUI/loadability facts

The model exposes the native format, per-entry state, normalized-operation
presence, opaque-code warning, identity evidence, and the `.cheats` destination
concept. A future GUI card should say “mGBA preserves this code but EmuWiz
does not interpret it” for opaque GameShark/Action Replay/CodeBreaker lines,
and must refuse title-only Apply. It must not present opaque codes as verified
memory writes.

## Legal/source mode and limitations

Only local/user-supplied cheat files are handled. No databases are bundled,
downloaded, or scraped. mGBA is MPL-2.0 licensed; this feature copies no mGBA
source or commercial cheat content.

The adapter does not decode encrypted CodeBreaker, button/conditional codes,
ROM hooks, multi-write GameShark forms, or pointer operations. It preserves
them exactly and relies on mGBA for runtime semantics. Destination discovery
from a particular mGBA installation/profile remains an adapter integration
step because mGBA's configured cheats directory is profile-dependent.
