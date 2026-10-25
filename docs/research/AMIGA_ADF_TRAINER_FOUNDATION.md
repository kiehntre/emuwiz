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
- `Preparable`: reserved; **unreachable today** (no verified trainer loading/execution binding exists in the current emulator backends).
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
No emulator execution projection, no scratch-copy launch, no ADZ/DMS/IPF/HDF/CD32 trainer support. The GUI preview route below now consumes this foundation.


## Recovered GUI route (2026-10-16)

Games → select a loose ADF → Mods & Cheats → Cheats opens the focused
`cheats_mods/adf_trainer.rs` panel before generic cheat installation or WHDLoad
CUSTOM controls. The extension selects the inspector only; a non-Amiga `.adf`
is refused by the existing structural validation.

1. Choose a local `emuwiz-amiga-adf-trainer/1` JSON source and click **Read disk
   and trainers**. Reads run on a worker. Nothing is selected automatically.
2. Review the exact disk and source SHA-256, declared author/reference, rejected
   entries, import limits, target compatibility, widths, addresses and timing.
   Source byte verification does not authenticate an author's claims or prove
   a trainer works in a game. No online trainer catalogue is bundled/fetched.
3. Resolve two-way conflicts with the existing canonical KeepA/KeepB/Skip
   choices, then explicitly check compatible options. Larger conflict groups
   remain skipped or unresolved. Whole-title/revision trainers needing a
   verified media set stay blocked; this panel binds one loose image.
4. **Preview selected options** rereads both inputs and refuses a stale review.
   The copyable report is explicitly `preview_only`, `execution_enabled=false`.
   It is a report, not an emulator cheat file. Source-path edits, emulator
   choice changes or selection/conflict changes invalidate the displayed preview.
   External file edits are detected when generating a new preview; a copied
   report remains a read-only snapshot, never execution authority.

There is no installer or enabled trainer state. **Enable in emulator** is
visibly disabled. The preview emulator selector does not change the selected
launch target, configuration files, original media or stored game identity.

## Exact runtime blocker and evidence

The existing FS-UAE launcher accepts media/profile arguments; it has no ADF
trainer-file consumer or guarded live-emulator connection. It also consumes
`VerifiedIdentityFact::AmigaIdentity`; the local exact-byte ADF evidence here
must not be promoted into that separate launch contract merely to force a
trainer launch. WinUAE debugger commands and Amiberry similarly have no
established execution binding in EmuWiz.

Official FS-UAE **v3.2.35** source narrows the old research claim: a build with
`WITH_LUA` does register byte/word memory APIs
([src/luascript.cpp](https://github.com/FrodeSolheim/fs-uae/blob/v3.2.35/src/luascript.cpp)),
and invokes a per-frame input callback
([src/fs-uae/main.c](https://github.com/FrodeSolheim/fs-uae/blob/v3.2.35/src/fs-uae/main.c)).
The frontend Lua-file loading path found is a **theme** script
([libfsemu/src/emu/theme.c](https://github.com/FrodeSolheim/fs-uae/blob/v3.2.35/libfsemu/src/emu/theme.c)),
not an established trainer import interface. These APIs alone do not establish
that EmuWiz can load a safe trainer through its existing backend. The installed
FS-UAE 3.1.66 executable used for this task has neither Lua dynamic linkage nor
the registered Lua API/callback strings. No emulator was launched and no ROM or
game content was acquired for this investigation.

Synthetic GUI tests cover exact-image selection, explicit choices, conflicts,
invalid source records, unsupported emulators/media, stale inputs, source
provenance, import bounds and byte-preserved disk/profile/identity evidence.
A native `show_cheats` dispatch test proves the panel is reachable even when
canonical title identity is unresolved, without promoting that identity or
changing the chosen launch target.
