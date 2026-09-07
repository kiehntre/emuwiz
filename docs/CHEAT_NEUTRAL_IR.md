# Neutral cheat IR (V2)

The core patch manager exposes a deliberately small, read-only semantic
intermediate representation for future cheat conversion work.  It is an
analysis and preview seam; existing native Dolphin, PCSX2 and RetroArch
installers remain the only apply paths.

## Model and safety

`CheatDocument` retains the console, source format, title, provenance, typed
operations and issues.  V1 operations are only `Write8`, `Write16`, and
`Write32`.  Anything else is retained as `UnsupportedRaw` with its source text
and a reason.  This prevents conditions, activators, pointers, master codes,
encrypted codes, and unknown widths from being silently converted to writes.

## Current mappings

* Dolphin Action Replay and Gecko lines with the proven direct-write prefixes
  (`02` 16-bit and `04` 32-bit) map to the IR. Other lines remain opaque.
* PNACH `byte`, `short`, and `word` patch lines map to the matching width.
  `double` and `extended` remain unsupported because the IR does not model
  their semantics.
* RetroArch `.cht` entries, Action Replay DS records, GameShark, and
  CodeBreaker do not have a proven direct-write grammar in the current core;
  they remain native/browse-only or opaque until an authoritative parser is
  available. No encrypted representation is guessed.

The V2 `encode_operation` helper emits pure in-memory direct-write text for
Dolphin Action Replay, Gecko, and PNACH. `assess_document_conversion` returns
per-operation status, provenance, output preview text, and exact/lossy/
unsupported counts. A missing GameShark/CodeBreaker encoder is reported
explicitly, and `can_apply` is false whenever an operation, issue, platform,
or encoder is unresolved. No output is fabricated and no emulator file is
written.

Nintendo DS Action Replay, RetroArch expressions, GameShark, and CodeBreaker
remain opaque in this revision. The current repository does not provide an
authoritative grammar for their direct-write subsets; conditions, activators,
pointer/loop codes, master/init codes, and encrypted variants are therefore
preserved as unsupported rather than guessed. No melonDS, DeSmuME, or
RetroArch writer is enabled by the IR.

V4 exposes `convert_cheat_document` and `supported_targets_for` as the reusable
converter-service seam. They return GUI-ready capability and per-operation
previews without installing or overwriting emulator files. RetroArch remains
deferred because the existing `.cht` parser retains code expressions as opaque
strings rather than an authoritative width/address/value tuple.

The target audit found that DuckStation, melonDS, DeSmuME, mGBA, and SameBoy
currently provide inventory/read-only configuration in this repository, not a
proven direct-write native writer. No additional target encoder is invented in
V4; one should be added only after an authoritative native grammar exists.

The intended architecture is:

```text
source parser -> neutral IR -> capability validation -> target encoder
             -> preview -> explicit confirmation
```

This is N parsers plus N encoders, rather than an unsafe N×N collection of
brand-specific translators. AR, Gecko, GameShark, and CodeBreaker are not
assumed to be universal formats merely because they display hexadecimal code
lines.
