# ROM representation diagnostics

EmuWiz keeps exact DAT identity authoritative, but a physical hash mismatch
does not always mean that the game content is wrong. A copier header, a known
byte order, or a proven interleaving can change the physical hash while
leaving the represented game bytes equivalent.

The backend diagnostic in
`crates/archivefs-core/src/rom_representation_diagnostics.rs` compares source
bytes with expected hash evidence and only reports equivalence after a known,
in-memory transform produces the expected hash. It retains the physical hash,
normalized hash, expected hash, operation, confidence, size evidence, and
reason. It never writes or rewrites a source.

## Supported representations

- SNES: a 512-byte copier-header candidate, accepted only when the stripped
  in-memory bytes hash to the expected identity.
- NES: a recognized 16-byte iNES header, using the existing header-aware
  parser; arbitrary prefixes are never stripped.
- N64: z64, v64, and n64 magic detection with the existing canonical Z64
  normalization.
- Genesis/Mega Drive: the existing conservative SMD shape detector and
  reversible de-interleaver. Shape alone is insufficient; the normalized hash
  must match.
- Generic input: exact physical hash matching. Unsupported or ambiguous
  formats remain `UnknownMismatch`.

`ContainerDifference` and `HeaderlessEquivalent` are represented in the type
model for future evidence-backed adapters but are not guessed by this initial
coverage.

## DAT and patch boundaries

Exact DAT identity is not weakened. A representation diagnostic is explanatory
evidence only; it does not promote a file into the verified identity store.
A future patch preview could use the diagnostic to explain “expected content
found, but file is byte-swapped” instead of showing only “hash mismatch”. Any
future apply path would remain a separate, explicit, reviewed boundary.

## Unsupported and ambiguous cases

Unknown keys, malformed N64 lengths, unsupported containers, arbitrary
prefixes, and normalized bytes that still do not match the expected identity
are not promoted. Depending on the available structural proof they are
reported as `ModifiedContent` or `UnknownMismatch`; no filename or extension
can upgrade either result.

Hasher-js and RomPatcher.js informed the research distinction between content
and representation. They are research references only and are not runtime
dependencies.
