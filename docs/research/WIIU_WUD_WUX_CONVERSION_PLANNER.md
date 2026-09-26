# Wii U WUD/WUX conversion planner

This feature adds a read-only, verification-first plan for WUD → WUX and
WUX → WUD. It does not run a converter, assemble split parts, create output,
rewrite source files, decrypt content, use keys, or handle WUA/NKit/extracted
titles.

## Source validation

Planning reuses the bounded `WiiUDiscInspection` result. WUD plans require a
regular source and a complete split sequence with no missing or duplicate
parts. WUX plans require valid magic, sector size, checked logical size,
bounded index table, and in-container block references. WUA is explicitly
unsupported. Split parts remain a logical source set; EmuWiz does not silently
concatenate them.

## Tool discovery

`JWUDTool` and `WudCompress` are probed locally only. No tool is downloaded or
installed. A matching executable name is not trusted as a capability: the
bounded version probe records the path, version evidence, and unknown
capability state. A tool becomes usable in a plan only when an explicit,
reviewed capability record proves the requested direction.

## Identity and verification

The planner never hashes multi-gigabyte media during ordinary inspection. The
request carries one of `HashAvailable`, `HashMissing`, or `HashStale`. Missing
identity produces `VerificationRequired`; stale identity refuses planning.

WUD → WUX requires reconstructing the logical WUD stream from the resulting
WUX and comparing it with the pre-conversion WUD identity. WUX → WUD requires
hashing the reconstructed WUD and comparing it with the pre-conversion logical
identity. Without an available identity, preservation equivalence is not
claimed.

## Space model

WUX output has an unknown/range estimate; no compression ratio is invented.
WUD output has an exact logical size when the WUX parser proves it. Temporary
and atomic-publication estimates include the source and destination where an
exact destination size exists. Destination free space is reported or refused
only when it can be evaluated honestly.

## GUI-v2

Selected Wii U media shows the applicable direction, structural status, local
tool evidence, source size, output estimate, verification requirement, identity
state, refusals, and the no-key/no-mutation boundary. There is no Convert or
Apply button.
