# Azahar / Citra-family native 3DS cheats

## Format and sources

Azahar's current [Gateway cheat class](https://github.com/azahar-emu/azahar/blob/master/src/core/cheats/gateway_cheat.h)
documents the native parser and opcode enum. The file is a title-ID-named
text file containing `[Cheat name]`, optional `*citra_enabled`, and Gateway
code lines. The [libretro Citra documentation](https://github.com/libretro/docs/blob/master/docs/library/citra.md)
confirms the persisted enable marker and `saves/Citra/cheats/<TITLE_ID>.txt`
layout. [Gateway documentation](https://wiki.gbatemp.net/wiki/Gateway_3DS)
confirms the title-ID naming and virtual-address model.

## Implemented model

`ThreeDsCheatFile` is bounded to 256 KiB, 512 entries, and 4096 lines. It
retains names, comments, original code text, deterministic normalized words,
enabled state, and typed parse issues.

Direct Gateway writes are normalized:

- `0XXXXXXX YYYYYYYY`: 32-bit write
- `1XXXXXXX 0000YYYY`: 16-bit write
- `2XXXXXXX 000000YY`: 8-bit write

The native source enumerates conditionals, offset registers, loops, terminators,
jokers, patches, and other controls. These remain opaque with their original
words and are never falsely represented as simple writes.

## Identity and version safety

The parser requires a valid 16-digit `0004...` title ID supplied by the caller;
the title ID is the authoritative destination identity. Existing NCSD/NCCH/CIA
evidence supplies the verified title identity and title kind. Cheat names may
contain a display version such as `v1.2`, which is retained as metadata but is
not treated as proof by itself. `assess_three_ds_version` distinguishes exact,
compatible, unknown, and mismatch states. A future Apply must block a mismatch
and require explicit review for unknown version evidence.

## Merge, state, and mutation boundary

Rendering is deterministic, preserves unrelated entries/comments, and writes
the native `*citra_enabled` marker only when the entry is enabled. Merge avoids
duplicate name-and-code entries while retaining existing state. This change is
inspection/merge infrastructure only: it does not modify global Azahar config,
ROM/content files, or claim a transaction Apply path until a destination/profile
binding is proven. Runtime enablement is native per-entry state; reload behavior
is emulator-controlled and must remain visible to a future adapter.

## GUI and legal boundary

GUI-v2 now exposes a local 3DS Gateway text preview with title ID, entry count,
understood operations, enabled state, and opaque-code warnings. No download or
database button is added. Sources are local/user imports only; no commercial
cheat database is bundled or scraped.

Azahar itself documents decrypted CCI/CIA requirements in its
[dumping-games guidance](https://github-wiki-see.page/m/azahar-emu/azahar/wiki/Dumping-Games).
This adapter does not decrypt, rewrite, or mutate those contents.
