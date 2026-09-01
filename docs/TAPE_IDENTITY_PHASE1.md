# Commodore tape identity — Phase 1

This phase adds bounded, read-only inspection for Commodore `TAP` and `T64`
media.

## What it proves

* A Commodore TAP is structurally valid when its `C64-TAPE-RAW` header,
  supported version/machine/video fields, non-empty declared payload, and
  exact file length agree. Its machine byte can distinguish Commodore 64 from
  VIC-20 when the header is valid.
* A T64 is structurally valid when its padded C64S header, supported version,
  bounded directory counts, entry records, address ranges, file offsets, and
  non-overlapping member ranges agree. Member names and address ranges are
  descriptive metadata only.

Neither format inspection identifies a game, release, or canonical title.
TAP pulse data is not decoded, and T64 member bytes are not extracted or
hashed. DAT/hash evidence remains the authority for exact release identity.

## Limits and safety

The observers reject files larger than 8 MiB. TAP reads only its 20-byte
header. T64 reads at most a 64-byte header plus 64 directory entries of 32
bytes each (2,112 bytes total). T64 counts, offsets, sizes, address ranges,
directory length, and overlap are checked before use with checked arithmetic;
no declared value controls an uncapped allocation or read.

Inspection uses the shared `safe_read` policy and never writes, extracts,
executes, mounts, or changes the input.

## Deliberate boundaries

The shared `.tap` extension remains weak evidence. Only a valid
`C64-TAPE-RAW` structure can provide Commodore TAP platform evidence; a file
that lacks it is not silently treated as Commodore media. ZX Spectrum TAP,
TZX, and Amstrad CDT remain deferred because their later parser design needs
separate bounded block/control-flow rules. WAV/audio captures remain outside
identity support: RIFF/WAVE structure alone cannot establish a tape or game
identity, and this phase does not decode pulses.
