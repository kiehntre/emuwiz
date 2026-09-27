# Nintendo 64 GameShark / Action Replay Cheat Family

## Scope and source boundary

This is an independently written, local-only interoperability decoder. It
does not bundle or retrieve GameShark databases and does not copy emulator or
commercial implementation code. The implementation uses public N64 cheat
format descriptions and observable emulator behavior as research basis.

The decoder is deliberately conservative: only the direct 8-bit (`80`) and
16-bit (`81`) write families are normalized into EmuWiz's neutral IR.
Conditional records, master/enabler records, repeat/serial records, pointer or
offset records, and unknown records retain their native representation and
cannot be mistaken for unconditional writes.

## Decode and identity model

`N64CheatDecodeResult` preserves the original line, opcode family, normalized
operation, issues, region, revision evidence, required master codes, and
provenance. Exact ROM hash and verified game identity are apply-capable
identity evidence. Header-only and region-only evidence remain weaker; a
title-only match is explicitly `NotReady`.

N64 ROM headers are recognized in `.z64`, `.n64`, and `.v64` byte orders. The
header is canonicalized before extracting game code, region, and revision, so
byte order is not treated as a distinct game revision.

## Master/enabler and compatibility behavior

The known `F0`–`F3` control family is retained as an explicit
`MasterOrEnabler` operation and is reported as a master requirement. It is not
auto-selected when multiple incompatible enablers are present. Direct-write
operations can be projected to the existing compatibility analyser; native
conditional and master semantics remain visible as raw/unsupported evidence
rather than being flattened into false memory writes.

## Emulator projection

RetroArch Mupen64Plus-Next and standalone Mupen64Plus are recognized as
preview targets. This change does not claim a safe native writer or destination
path for either target, so projections are explicitly preview-only and require
restart/reload if a future reviewed writer is added.

## GUI

The core preview model supplies the GUI with the format name, original code,
understood/unsupported operation counts, revision evidence, master-code state,
and warnings such as a revision mismatch. No database acquisition or Apply
button is added by this phase.

## References

- Public Nintendo 64 GameShark code-format descriptions for the `80`/`81`
  direct-write families, conditional families, and enabler/control families.
- Mupen64Plus and libretro Mupen64Plus-Next public cheat behavior used only as
  behavioral interoperability reference; no source was copied.

## Limitations

- Dynamic pointer, repeat, serial, boot-time, and encrypted variants remain
  native raw/unsupported.
- Conditional bodies are preserved as ordered native records but are not
  emitted as unconditional neutral writes.
- No native writer or automatic emulator installation is implemented.
- Exact ROM hashing remains an explicit caller decision and is not performed
  during ordinary header inspection.
