# Standalone Flycast cheats: verified installation boundary

Research baseline: official Flycast v2.7, commit
`5aa091fde632fb332c8d8c34e280d62dc951954c` (2026-08-14).
The reviewed upstream master `6bb0fc3e18dac4b01694f488c7a71f5930e8a648`
(2026-10-17) has the same cheat loader. This is source-verified compatibility;
no in-game effectiveness or full-emulator runtime test is claimed.

## Enabled configurations

Linux x86-64, ordinary effective `$XDG_CONFIG_HOME/flycast/emu.cfg`, or
`$HOME/.config/flycast/emu.cfg` when XDG_CONFIG_HOME is unset, with exactly:

| Artifact | SHA-256 |
| --- | --- |
| Official `flycast-x86_64-2.7.AppImage` | `5b9f8a636a6acb8446bc78fe96650d8cb8233ac9f8ce69665dab3f39a1cbaae1` |
| Its enclosed `usr/bin/flycast` executable | `141ba9413b41e8578e313e07935ea256d26c21f516de358f07529bc80b2163f1` |

The AppImage digest is published by the [official release API](https://api.github.com/repos/flyinghead/flycast/releases/tags/v2.7).
The enclosed executable was extracted with `unsquashfs -o 944632` and independently
hashed twice; extraction did not execute the AppImage. Renaming an artifact does
not change its recognition. Current Launch discovery must already find it as its
single executable (for example a real executable named `flycast` on PATH).
No new executable override or launcher flags are introduced.

The shared AUTO/PREFERRED/FORCED resolver and selected Blue Launch profile must
agree on the exact pair. The shared selection must be explicitly saved before
Preview so its bytes can be guarded before writing. Flycast has no supported
`--version` flag; the application never runs it to guess a version. The new
inventory enum value only permits use of the shared resolver/persistence; the
update subsystem returns no Flycast metadata endpoint or update-support claim.

## Actual loading mechanism

[CheatManager::loadCheatFile and reset](https://github.com/flyinghead/flycast/blob/5aa091fde632fb332c8d8c34e280d62dc951954c/core/cheats.cpp#L381)
read a sectionless native `.cht` file and associate it through
`emu.cfg` `[cheats]`, where the exact game ID maps to an absolute filename.
The installer explicitly creates that mapping; the chosen `0-emuwiz-<product>.cht`
filename is not an implicit discovery rule.

[Dreamcast game ID creation](https://github.com/flyinghead/flycast/blob/5aa091fde632fb332c8d8c34e280d62dc951954c/core/emulator.cpp#L858)
uses raw IP metadata product bytes, terminating at NUL and trimming trailing
whitespace. It does not uppercase, insert hyphens, or append disc numbers.
The wrapper rereads boot metadata through existing GDI, CUE, CDI and CHD readers
(and the existing cooked ISO reader), compares the exact key with verified core
identity, and binds Preview to that metadata digest. CDI/CHD readers must be
compiled in. Unsupported/ambiguous readers and normalisation mismatches refuse
installation. Product IDs shared by discs/revisions share the mapping; source
applicability requires explicit user confirmation and is not authenticated by
matching a product declaration.

v2.6 added `Dreamcast.CheatPath` folder lookup (commit `ce07a7fb`, 2025-11-19).
It tries the full content basename **including its original extension**, followed
by `.cht` and then `.txt`: `Title.gdi.cht`, not `Title.cht` or a serial filename.
This workflow uses the explicit mapping instead. v2.5 has the older mapping
mechanism, but unpinned versions/builds are deliberately not enabled.

[Native Linux startup](https://github.com/flyinghead/flycast/blob/5aa091fde632fb332c8d8c34e280d62dc951954c/core/linux-dist/main.cpp#L43)
selects the XDG/HOME path. The official
[AppImage AppRun](https://github.com/flyinghead/flycast/blob/5aa091fde632fb332c8d8c34e280d62dc951954c/shell/linux/make-appimage.sh)
changes library variables and invokes the enclosed executable with those same
Linux profile rules. Arbitrary native executable-adjacent `emu.cfg` files do not
establish portable support.

## Supported native entries

Upstream native operation numbers are 0 disabled, 1 set, 2 increment,
3 decrement, 4 equal condition, 5 unequal condition, 6 greater condition,
7 less condition and 8 copy. Width fields encode `1 << memory_search_size`
bits; upstream also understands sub-byte masks and repeat fields. These are
native memory-operation records, not a general code-only GameShark decoder.
The explicit external loader and GUI Load action are separate from code-entry
conversion; this task adds no converter.


The existing parser and renderer are unchanged. A strict installation wrapper
accepts dense, explicit counts, files smaller than 1 MiB, and native set-value
operations of 8, 16 or 32 bits, wholly within 16 MiB Dreamcast RAM, with fitting
values. It accepts upstream-saved inert `big_endian=false`, empty `code`,
`handler=1`, zero destination/mask/increments and repeat count 1. It refuses
conflicting address ranges, unknown fields, truncated/sparse/duplicate metadata,
ambiguous whitespace/quoting, repeated/masked writes, conditions, increment,
decrement, copy operations and CodeBreaker/GameShark code-only imports.

Flycast supports more native operations than this deliberately narrow installation
lane. Unsupported existing files are preserved intact and refused, not silently
partially rewritten. Existing supported entries are merged with their native
fields and enabled states preserved; guarded Undo restores original bytes.

`cheatN_enable` controls loading state. Blue defaults imported entries to disabled.
[Flycast's Save action](https://github.com/flyinghead/flycast/blob/5aa091fde632fb332c8d8c34e280d62dc951954c/core/cheats.cpp#L1060)
forces saved entries disabled; GUI toggles are runtime changes. Hardcore
achievements can suppress loading, and online modes can suppress application.
Installed files therefore do not prove that a cheat worked in-game.

## Blue journey and recovery

1. Select a Dreamcast game with verified identity and an unambiguous standalone
   Flycast selection in Launch Readiness.
2. Open Blue Cheats & Mods → standalone Flycast; choose a local native `.cht`,
   declare its product code and select entries/enabled states.
3. Save AUTO, prefer, or force the current pair through the existing shared
   selection store. Launch selection is preserved; disagreement refuses install.
4. Preview. A worker rereads source/game/profile/binary/policy, validates native
   semantics, rejects conflicting mappings, and presents exact output paths,
   bytes, before/after SHA-256, configuration mapping and boot metadata.
5. Confirm that exact plan, replacements and product-wide disc/revision scope;
   Install. A worker revalidates and uses the existing shared atomic writer,
   backups, durable History and readback verification. The managed cheat filename
   sorts before `emu.cfg`; the configuration write additionally requires the
   installed cheat to match the approved digest, so a failed/stale cheat cannot
   activate a new mapping. The shared engine/journals
   and R2 are unchanged. A single transaction covers both approved files.
6. Refresh Flycast History, Preview Undo, separately confirm Undo. The selected
   installation/configuration is checked again; shared Undo checks current file
   and backup hashes and restores originals only when those guards hold.

All Flycast instances must be closed while changing the profile. The wrapper
refuses named Flycast processes and running copies of the pinned artifacts,
including the AppImage's enclosed executable, to avoid an emulator later saving
stale in-memory configuration. Unknown/inaccessible process evidence cannot be
used as proof of another emulator's identity.

Interrupted, partial and uncertain results retain their evidence and direct the
user to History/recovery; they are never presented as complete installs.
Unfinished Flycast operations block new installation across games sharing the profile.
A completed refusal proven to precede every write permits a fresh Preview;
uncertain or partially applied operations still require recovery. A successful guarded rollback
receipt releases the corresponding failed-operation block. Changed configuration,
cheats, sources, binary or selection requires another Preview. Ordinary browsing
starts no file inspection, network request, download, emulator, or Apply.

## Refused configurations

- Other releases, self-built/distro binaries, scripts/wrappers or unknown hashes:
  installed build behaviour has not been tied to reviewed source.
- AppImage `.home`/`.config` portable overrides: the runtime mechanism is real
  ([AppImage documentation](https://docs.appimage.org/user-guide/portable-mode.html)),
  but current Launch does not prove the altered profile pairing. They are refused.
- Flatpak `org.flycast.Flycast`: the official
  [manifest](https://github.com/flathub/org.flycast.Flycast/blob/master/org.flycast.Flycast.yml)
  uses sandbox XDG roots, commonly host
  `~/.var/app/org.flycast.Flycast/config/flycast`. Installed deployment identity,
  overrides, sandbox visibility and the actual Launch pair are not proven by
  that conventional host filename; writing there is refused.
- Arbitrary native portable/explicit profile roots, symlinks, malformed or duplicate
  configuration sections/keys, non-exact game identity, ambiguous executables,
  stale policy and existing mappings to another user file: uncertain or conflicting
  ownership/loading evidence. No user mapping is replaced to manufacture support.
- Existing unmapped destination files: associating one while merging would need
  two replacements. The existing shared writer cannot create a second backup
  in the same operation. Preview refuses before any write. A new cheat file
  plus configuration mapping, or a merge into an already-mapped file, needs
  only one replacement backup and is supported. The shared writer is unchanged.

No ROM, BIOS, save, unrelated emulator, transaction-journal implementation or R2
file is changed. No cheat download, automatic enabling, conversion, or background
update engine is added.
