# Neutral cheat IR (V2)

The core patch manager exposes a deliberately small, read-only semantic
intermediate representation for future cheat conversion work.  It is an
analysis and preview seam; existing native Dolphin, PCSX2 and RetroArch
installers remain the only apply paths.

## Model and safety

`CheatDocument` retains the console, source format, title, provenance, typed
operations and issues. V1 direct operations are `Write8`, `Write16`, and
`Write32`; V10 adds timing-explicit `OnFrameWrite8/16/32`. Anything else is
retained as `UnsupportedRaw` with its source text and a reason. This prevents
conditions, activators, pointers, master codes, encrypted codes, and unknown
widths from being silently converted to writes.

## Current mappings

* Dolphin Action Replay and Gecko lines with the proven direct-write prefixes
  (`02` 16-bit and `04` 32-bit) map to the IR. Other lines remain opaque.
* PNACH `byte`, `short`, and `word` patch lines map to the matching width.
  `double` and `extended` remain unsupported because the IR does not model
  their semantics.
* Nintendo DS Action Replay now has a strict classifier and pure text encoder
  canonical direct-only `0XXXXXXX YYYYYYYY`, `1XXXXXXX 0000YYYY`, and
  `2XXXXXXX 000000YY` records. It maps those records to `Write32`, `Write16`,
  and `Write8` respectively, and emits the exact inverse canonical forms when
  the document is Nintendo DS and contains only supported operations. Every
  other line remains `UnsupportedRaw` with a typed refusal reason.
* RetroArch `.cht` entries, GameShark, and CodeBreaker remain native/browse-only
  or opaque until an authoritative, version-specific parser and encoder exist.
  No encrypted representation is guessed.
* Dolphin `[OnFrame]` has a separate timing-preserving subset. Canonical
  `0xADDRESS:byte:0xVALUE`, `word`, and `dword` entries map to
  `OnFrameWrite8`, `OnFrameWrite16`, and `OnFrameWrite32`. These operations are
  applied on Dolphin's frame patch cycle; they are not collapsed into ordinary
  writes. Conditional comparands, malformed lines, unsupported types, and
  values that would be truncated remain `UnsupportedRaw`.

The `encode_operation` helper emits pure in-memory direct-write text for
Dolphin Action Replay, Gecko, PNACH, the canonical DS Action Replay subset,
and the timing-explicit Dolphin OnFrame format. `assess_document_conversion`
returns per-operation status, provenance,
output preview text, and exact/lossy/unsupported counts. A missing
GameShark/CodeBreaker or RetroArch encoder is reported explicitly, and
`can_apply` is false whenever an operation, issue, platform, or encoder is
unresolved. No output is fabricated and no emulator file is written.

DS Action Replay conditions, activators, pointers, loops/multi-writes,
copy/fill, offset/data-register operations, processor/master-init forms, and
unknown/encrypted variants remain preserved as unsupported rather than
guessed. The parser rejects malformed, noncanonical, and misaligned direct
writes. Conversion preview exposes complete text only; mixed documents never
expose a partial export. No emulator installation writer is enabled by the IR.

## DS target-writer audit (V7)

The V7 encoder is an in-memory format encoder, not an emulator installer.

| Target | Accepts DS AR directly | Per-game storage evidence | Safe EmuWiz writer | Decision |
|---|---:|---|---:|---|
| melonDS standalone | not established | no reviewed native cheat-file/identity contract in the current repo | No | defer pending a versioned native format and transactional identity seam |
| DeSmuME standalone | parser accepts AR semantics, but native persistence is not an EmuWiz contract | no reviewed per-game native install contract | No | defer; do not write config or `.dct`-style state |
| RetroArch DS cores | core-dependent `.cht` handler; not a generic DS AR target | game-specific `.cht` path is known by RetroArch, but handler/core semantics remain authoritative | No | defer; core-specific writer audit required |

The reviewed RetroArch melonDS documentation records RetroArch cheats as
supported but native cheats as unsupported, and current DS database `.cht`
entries contain core-dependent multi-line expressions. Therefore a common
`Write8`/`Write16`/`Write32` IR is not enough to claim a safe writer. No target
writer or emulator installation was added in V7. The first recommended writer
target is a named RetroArch DS core only after its handler grammar and
per-game file association are fixture-proven; standalone melonDS and DeSmuME
remain later candidates.

The converter service exposes `convert_cheat_document`,
`supported_targets_for`, and the pure `export_conversion_preview` seam. They
return GUI-ready capability and complete previews without installing or
overwriting emulator files. RetroArch remains deferred because the existing
`.cht` parser retains code expressions as opaque strings rather than an
authoritative width/address/value tuple.

The target audit found that melonDS, DeSmuME, and RetroArch DS cores do not
currently provide a proven, reusable native DS direct-write writer seam in
this repository. No native installation encoder is invented in V7; one should
be added only after an authoritative grammar, per-game identity, and
transactional storage path are reviewed.

The intended architecture is:

```text
source parser -> neutral IR -> capability validation -> target encoder
             -> preview -> explicit confirmation
```

This is N parsers plus N encoders, rather than an unsafe N×N collection of
brand-specific translators. AR, Gecko, GameShark, and CodeBreaker are not
assumed to be universal formats merely because they display hexadecimal code
lines.

## Dolphin OnFrame boundary (V10)

Dolphin parses OnFrame entries as `address:type:value[:comparand]`, with
`byte`, `word`, and `dword` direct writes. A fourth field is a conditional
comparand. The native patch engine applies the OnFrame list during its frame
patch cycle, so EmuWiz preserves this execution policy in distinct
`OnFrameWrite8/16/32` operations.

Complete GameCube/Wii OnFrame documents can be rendered back to canonical
OnFrame text. V10 does not convert ordinary Action Replay/Gecko writes to
OnFrame, or OnFrame writes to AR/Gecko: the existing neutral direct-write
address model does not prove exact address-space and lifetime equivalence for
that conversion. Existing AR/Gecko direct-write conversion is unchanged.
Float, conditional, branch, pointer, and other stateful patch forms remain
unsupported. Dolphin's native INI loader/writer already handles
`[OnFrame]`, `[OnFrame_Enabled]`, and `[OnFrame_Disabled]`; V10 only adds the
pure IR parser/preview/encoder and writes no emulator files.
