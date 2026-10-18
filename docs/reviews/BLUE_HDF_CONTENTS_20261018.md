# Blue HDF contents: focused connection

Initial comparison: fresh main `3a2437f04380c3c4c3598e1c3629a9ef33f2e668`.
Integration baseline after ownership handoff: fresh main
`978c1a8f29eaab35dfbf633b1766ac77df095996`.

Existing core `amiga_disk` already detects RDB and flat AmigaDOS images and
traverses OFS/FFS through `affs-read` and a partition-range read budget. Existing
Blue Selected Evidence calls the floppy inspector for ADF/ADZ. Neither that
panel nor Game Details exposes HDF filesystem contents. No parser or importer
is replaced by this connection.

The feature-owned contents adapter reuses that reader, metadata validator,
source identity and range device. It keeps one safely opened read-only image
descriptor, checks its identity before and after requests, and refuses changed
or replaced sources. RDB/PART checksums, overlapping partitions, contradictory
filesystem signatures, directory cycles, conflicting names and invalid entry
types are refused before a contents page is published. The existing reader
performs filesystem metadata parsing. Links and special entries remain visible
but are never followed. File sizes are explicitly header observations: file
payloads are not read or verified.

Game Details offers an explicit Inspect / refresh action. Partition selection
and directory navigation run in one bounded worker. Opening a different game
invalidates outstanding results; media changes require explicit reinspection.
Filesystem refusal preserves the detected partition choices. The panel uses
the existing Blue card component and stores only temporary view state.

Each page contains at most 128 entries. It visits only the selected directory,
replaying at most 4096 entries to paginate, with at most 8192 partition-sector
reads per request. Depth is limited to 32 and observed directory nodes to
10,000. The directory cutoff is displayed as a limitation, not as a complete
listing. PFS/SFS/MuFS, unknown filesystems and non-512-byte filesystem blocks
remain unsupported for browsing. A stalled underlying read has no wall-clock
deadline. Source identity checks are point-in-time metadata checks, not an
atomic filesystem snapshot or a content hash of the full image. Same-size writes
with indistinguishable timestamps can evade freshness checks; the panel therefore
requires explicit refresh and never asserts full-image integrity.

The UI explicitly separates structural inspection from game identification,
DAT verification and launch readiness. No catalogue confidence, saved evidence,
launch configuration or image is modified. There are no mount, repair, extract,
import or emulator actions.

The Amiga/WHDLoad owner explicitly released the four existing-file bridges: one declaration in
`amiga_disk/filesystem.rs`, one re-export in `amiga_disk/mod.rs`, one declaration
in `gui_v2/mod.rs`, and one Game Details call in `gui_v2/pages.rs`. Before that
handoff, tests used a disposable source overlay of those exact bridges, with the
original shared files unchanged. The preserved patch was applied unchanged after
release. All test media are generated disposable disks.

Focused validation: `cargo-iso --low-debug --incremental test -p archivefs-core
--lib --locked hdf_contents` and the corresponding `archivefs-gui` command.
Final runtime results and independent exact-commit review are required before
promotion.
