# Dreamcast GDI topology foundation

`archivefs_core::ingestion::gdi::parse_gdi_descriptor` exposes a validated,
ordered view of a GDI descriptor and every safely resolved track source.
Existing bounded parsing, track-count/number/LBA checks, path containment,
regular-file checks, and the identity-track resolver remain in place. The
new model also exposes typed data/audio track classification, supported
sector widths, the original relative filename, canonical source path, source
file length, and the descriptor's numeric offset field.

`GdiDescriptor::logical_topology` projects ordered track number, starting LBA,
track type, sector width, offset token, and a frame count when it can be
derived from a zero-offset source whose length is an exact number of sectors.
The offset token is preserved as parsed; this foundation does not assume its
units. If it is nonzero, frame count is unknown. `compare_gdi_topologies`
reports structural differences and deliberately ignores source filenames and
paths.

These terms describe different evidence:

- **Descriptor equality** means the exact descriptor text/bytes and source
  references agree. Structural equality of `GdiDescriptor` is not byte
  equality because whitespace and unmodeled text are not retained.
- **Topology equality** means the modeled ordered track facts compare equal.
  It says nothing about the sectors stored in those tracks.
- **Logical-content equality** requires comparing the represented sector
  payloads with format-aware handling. This API does not perform that check.
- **Byte-exact equality** requires byte comparisons of the descriptor and all
  referenced track files. This API does not perform that check either.

CUE and CHD are not converted into this model in this change. CUE's timeline
contains INDEX 00/01, pregap/postgap, and per-file coordinate details; CHD
metadata has its own track/frame facts. A future shared comparison needs to
normalize those coordinates while retaining those details, then compare actual
track content separately before making logical-equivalence claims.

No writer, patcher, or conversion tool is invoked by this foundation.
