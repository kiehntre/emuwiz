# PPSSPP CWCheat native adapter

## Scope

This adapter is PPSSPP-specific and leaves generic cheat routing, identity
resolution, and conflict analysis unchanged. It parses and merges local
CWCheat files, creates a deterministic preview, and hands publication to the
existing shared transaction/history/rollback machinery. It performs no
network access and does not download `cheat.db`.

## Format and sources

PPSSPP uses `PSP/CHEATS/<game-id>.ini` under the discovered memstick. The
native source identifies the game with `_S`, displays `_G`, groups cheats with
`_C0`/`_C1`, and stores code rows with `_L`. `_C0` is disabled and `_C1` is
enabled. The parser accepts the documented game-ID forms with or without the
hyphen and normalizes them to the safe filename form.

References: [PPSSPP Config.cpp](https://github.com/hrydgard/ppsspp/blob/master/Core/Config.cpp),
[PPSSPP CWCheat screen](https://github.com/hrydgard/ppsspp/blob/master/UI/CwCheatScreen.cpp),
[PPSSPP CWCheat support notes](https://forums.ppsspp.org/showthread.php?pid=28486), and
[PPSSPP cheat directory guidance](https://forums.ppsspp.org/showthread.php?pid=50293).

The PPSSPP source confirms `EnableCheats` and the cheat refresh path. After an
external file publication EmuWiz reports `ReloadCheatsOrGame`; it does not
silently change the global setting.

## Parser and normalization

Parsing is bounded by file size, line count, line length, cheat count, and code
lines per cheat. Unknown/comment lines are retained. A supported `_L` row is
normalized only for the verified CWCheat 32-bit write form (`0x2... value`);
other code types remain raw with a typed unsupported issue. No arbitrary code
or script is executed.

Multiple `_S` IDs, malformed IDs, missing identity, and cheat entries before a
title fail closed. Identity must be the exact verified PSP game ID; title-only
matching is not sufficient for apply.

## Merge and per-code state

The writer emits deterministic `_S`, `_G`, `_C0`/`_C1`, and `_L` lines. Existing
unrelated cheats and retained comments are preserved semantically. Duplicate
title/code entries are not added. One named cheat can be enabled, disabled, or
removed without deleting other entries.

## Apply and rollback

The adapter stages the generated `.ini` in a private temporary directory, then
builds a `PreviewAdapter::Ppsspp` shared preview and transaction plan. The
preview proves the exact game ID, profile cheat root, source digest, and
destination state. Shared atomic publication, backup, stale-source and
destination detection, history, and exact rollback remain authoritative.

If an existing destination was externally changed after preview, shared apply
refuses it. If a transaction created the file and it remains unchanged, shared
rollback can remove it; replacements restore the exact backed-up bytes.

## Global state

`EnableCheats` is surfaced as Enabled, Disabled, or Unknown from the existing
PPSSPP config inspection. A file may be installed while global cheats are
disabled, but the caller must show: “Cheat file installed, but PPSSPP cheats
are disabled.” No global config mutation is added in this pass.

## Limits

This pass does not interpret every CWCheat opcode, write `cheat.db`, change
PPSSPP global settings, launch PPSSPP, or promise that a code is compatible
with every revision. Unsupported rows remain visible and preserved rather
than being silently discarded.
