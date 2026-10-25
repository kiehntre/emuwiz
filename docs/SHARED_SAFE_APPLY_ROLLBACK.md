# Shared safe apply, journal, and rollback

## CURRENT BEHAVIOR

The shared transaction engine applies selected verified cheat, patch, provider,
or supported local-mod records. It is not a general mod installer. An adapter
must supply an approved materialized source and exact safe destination.

Before writing, EmuWiz binds confirmation to the plan and rechecks identity,
source bytes, destination state, roots, symlink safety, and replacement
permission. It writes through a temporary file, verifies the result, creates a
verified backup before replacement, and records the operation in a durable
journal. The journal preserves paths, digests, states, and failures needed for
history and rollback.

Rollback begins with a fresh read-only preview. It removes only an unchanged
new file or restores an owned verified backup; changed destinations, missing
backups, unsafe paths, and repeated rollback fail closed. Partial success and
journal failure remain explicit outcomes.

The engine is used by current RetroArch catalogue materialization, PCSX2
PNACH installation, Dolphin Gecko/GameSettings installation, and selected
GameCube/Wii provider and texture/mod flows. Discovery, browsing, validation,
inventory, and preview remain read-only. No path launches a script, binary,
emulator, or downloaded installer.

## HISTORICAL DESIGN CONTEXT

Earlier adapter notes called all PCSX2 or Dolphin work read-only because only
inventory existed at that point. The inspection modules retain those names for
compatibility, but current selected apply flows are described in
[ADAPTER_SUPPORT_MATRIX.md](ADAPTER_SUPPORT_MATRIX.md).
