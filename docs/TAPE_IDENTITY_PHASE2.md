# Tape identity Phase 2

Phase 2 adds bounded, read-only structural inspection for ZX Spectrum TAP,
ZX Spectrum TZX, and Amstrad CPC CDT.

## ZX Spectrum TAP

ZX TAP has no identifying header. EmuWiz therefore accepts it only when a
linear stream of little-endian block lengths stays inside the input and every
block's standard XOR checksum validates. Standard 19-byte header blocks may
contribute a bounded filename, type, length, and parameter record. Those
fields are descriptive provenance only; they never become a game or release
identity. A DAT/hash match is still required for exact identity.

The shared `.tap` rule is explicit: a valid `C64-TAPE-RAW` header is
Commodore TAP, a valid headerless ZX block stream is a ZX TAP candidate, and
unvalidated bytes are neither. If both parsers ever validate the same bytes,
the result is refused as ambiguous rather than guessed.

## TZX and CDT

TZX is validated as a signed, versioned, linearly framed block container.
Standard, turbo, pure-tone, pulse-sequence, pure-data, direct-recording,
metadata, hardware, group, and safely length-prefixed blocks are bounded and
recorded. CDT reuses this parser because CDT is TZX-compatible; `.cdt` alone
does not prove Amstrad CPC. A caller-supplied CPC context may add conservative
compatibility evidence, just as a ZX context may add Spectrum compatibility
evidence. The shared container remains distinct from exact identity.

Jump, loop, call, return, and select blocks are records only. They are never
followed or executed. CSW/RLE and generalized-data payloads are framed only;
they are never expanded. Timing and waveform decoding are out of scope.
Unknown block IDs fail closed when a safe length cannot be established.

## Limits and evidence

Inspection accepts at most 8 MiB, 4,096 blocks, and 65,535 bytes per ZX TAP
block. TZX metadata retains at most 256 records and 64 KiB total, with text
bounded to 4 KiB per record; group depth is capped at 64. All length and
count arithmetic is checked before slicing or recording data.

The parser emits content evidence for tape media and format structure. It
does not assign a canonical platform, infer a title from embedded text, or
produce a verified game identity. WAV remains deferred: RIFF/WAVE structure
alone cannot safely prove tape platform or game identity.
