# Amiga ADF / non-WHDLoad trainer foundation

Module: `archivefs-core/src/amiga_adf_trainer/` (registered with one line in `lib.rs`).

## What exists on main (reused, not duplicated)
- `amiga_disk::inspect_amiga_floppy`: content-validated AmigaDOS floppy. Platform evidence only; no game identity.
- Verified Amiga game identity exists **only** for WHDLoad (`IdentityKind::AmigaWHDLoad`). A loose ADF has none.
- `media_set`: canonical multi-disk model (ordinal, expected count, state, release variant). Its medium keys are DAT names, not file hashes.
- `launch/fsuae_command.rs`: ADF is passed via `--floppy-drive-0`. WHDLoad trainers (`patch_manager::whdload_trainer`) are launch options, not memory writes.
- Canonical cheat model: `CheatReconciliationEntry`, `assess_cheat_applicability`, `reconcile_cheats_for_game`, `resolve_reviewed_cheat_plan`. No Amiga engine duplicates these.

## Identity anchor
A loose ADF is identified by the SHA-256 of the whole image **plus** structural validation, expressed as canonical `IdentityEvidence` (`LooseRomSha256`/`ExactBytes` + `Platform`). Filename, title and volume label never authorise. The supplied evidence is checked against the bytes actually measured.

## Mechanisms researched
| Mechanism | Evidence | Classification |
|---|---|---|
| Memory write in a running emulator (UAE debugger `W <addr> <value>`, FS-UAE F12-D, WinUAE Shift+F12) | Third-party guide; interactive only; width semantics unspecified; no scripted form | `RequiresEmulatorRuntime` (no projection generated) |
| Boot-time trainer menu | Part of a modified disk | Unsupported: alternate media |
| Pre-trained / cracked disk (TOSEC `[t]` `[cr]` `[h]` `[m]`) | Different disk, not an applied cheat | Unsupported; alternate media, never generated/downloaded/merged |
| Action Replay codes | Need a cartridge ROM; no code model | Unsupported |
| Save-state edit | Emulator-specific format | Unsupported |
| Disk sector patch | Needs writable media | Unsupported: "requires scratch-copy launch integration", not implemented |

## Classifications
- `Preparable`: reserved; **unreachable today** (no emulator syntax is evidenced well enough).
- `PreviewOnly`: shown, but identity/targeting/choice is missing (unverified or empty identity, no set, incomplete set, unconfirmed disk, unresolved conflict).
- `RequiresEmulatorRuntime`: complete verified plan; only a running emulator could act on it.
- `Unsupported`: wrong platform/disk/release/revision/region, image not targeted, alternate media, unsupported mechanism.

## Multi-disk
Scope is `WholeTitle` (needs complete verified set), `Disk(n)` (the set's ordinal must equal n; never carried to another disk) or `Revision`. Trainers list exact hashes per disk; a swapped or missing disk, or a different revision's hashes, is refused.

## Conflicts and provenance
Trainers become canonical reconciliation entries with `local_with_sha256` provenance. Duplicates and conflicts come from `reconcile_cheats_for_game`; a conflict selects nothing until a canonical `CheatReviewChoice` is supplied.

## Bounds
256 KiB source, 1024 entries, 64 writes, 16 media refs, 4 MiB image; checked `0x`-hex/decimal parsing, alignment and width range; malformed entries are rejected individually. Unknown fields (including any command string) are rejected.

## Gaps
No emulator projection, no scratch-copy launch, no ADZ/DMS/IPF/HDF/CD32 support, no GUI.
