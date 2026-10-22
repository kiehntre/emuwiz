# ZX Spectrum support: status and limits

This page records what EmuWiz really does with ZX Spectrum media. It is updated
slice by slice. It is not a roadmap promise: a format listed here as recognised
is not thereby launchable or verified.

## Machine model

`zx_spectrum_snapshot::SpectrumMachine` is the one machine vocabulary: 16K, 48K,
128K, +2, +2A, +3, Pentagon, Scorpion, **Spectrum Next** (added in slice A1), and
`OtherDocumented` for values a format documents but EmuWiz does not map.
`MachineEvidence` keeps how the machine is known separate from the title:
`Encoded` (a documented byte states it), `ImpliedByFormat`, `EncodedButUndocumented`,
`Absent`. A file extension never sets the machine.

## Media formats

The status words are the ones `zx_spectrum_media::SpectrumMediaStatus` reports:
*validated* = every structural field checked and the walk ended at end-of-file;
*partial* = recognised, with named integrity data not verified or damaged.
"Catalogued" means the scanner creates a Library row. Launch is a separate
decision (see below).

| Format | Inspector | Best status | Catalogued as | Machine evidence | Notes |
|---|---|---|---|---|---|
| TAP | `tape_identity` | validated (block XOR checksums) | game image | none | `.tap` is shared with C64; platform needs other evidence |
| TZX | `tape_identity` | validated (block walk) | game image | none | no jump/loop following |
| PZX | `zx_spectrum_media` (A1) | validated (DATA sizes checked) | game image | none | pulse timings not decoded |
| CSW, WAV | `tape_audio` (WAV only) | - | not catalogued as Spectrum | none | CSW not inspected; shared formats |
| Z80 v1/v2/v3, SNA, SZX | `zx_spectrum_snapshot` | validated | game image | encoded or implied | |
| TRD, SCL | `disk_format` | validated | game image | none (TR-DOS) | |
| DSK/EDSK (+3) | `disk_format` | validated (+3DOS spec) | game image | none | a bare CPCEMU DSK stays ambiguous |
| UDI | `zx_spectrum_media` (A1) | **partial**: track walk checked, trailing CRC32 not verified | game image | none | only MFM/FM/mixed track records; others are an unsupported variant |
| MDR | `zx_spectrum_media` (A1) | validated when all three Interface 1 checksums agree on every formatted sector; partial when record/data checksums fail | game image | none (needs Interface 1) | right size but no valid header checksum = **ambiguous**, never catalogued; mixed header checksums or repeated sector numbers = malformed |
| NEX | `zx_spectrum_media` (A1) | validated (header, bank table, length) | game image | **encoded: Spectrum Next** | bank contents not examined |
| RZX | `zx_spectrum_media` (A2) | partial (block chain walked; input frames not decoded); *protected* when signed | **preservation-only recording** | from an uncompressed embedded snapshot only | see below |
| MGT, IMG (+D/DISCiPLE) | - | - | not catalogued as Spectrum | - | shared with SAM Coupé; no signature. Gap |
| Interface 2 ROM, Timex DCK | - | - | - | - | `.rom` is shared. Gap |
| SNX | - | - | - | - | Next SNA variant. Gap |

Every inspector reads a bounded amount fixed from file metadata before reading
(`.nex`: only the 512-byte header), refuses a file that changes during the
read, and never writes. Refusals keep their distinctions: *not recognised*,
*truncated*, *corrupt*, *ambiguous*, *unsupported variant* and *too large* (a
reader limit, not damage). Each reaches the scan's discovery details with its own
explanation.

The new formats are dispatched only as loose files. ZIP members that are
Spectrum media go through the existing archive path, which does not run these
inspectors yet.

## RZX gameplay recordings (slice A2)

An RZX file records a play session: a starting snapshot plus input. It is not
a game.

- A structurally recognised RZX becomes a Library row of kind
  `preservation_recording` (`ArchiveKind::PreservationRecording`, content kind
  `gameplay_recording`), with its path, size, source folder, platform (ZX
  Spectrum, header provenance) and scan discovery explanation (format, status,
  machine evidence). It survives a database reopen and rescan unchanged. It is
  never a mount input, and `launch::build_launch_plan` always returns a single
  blocked candidate for it, whatever identity it carries.
- **Import-time fingerprint.** Inside each folder's write transaction (so it
  commits or rolls back with its row) the scan stores the full-file SHA-256 of
  every recording in `archives.content_hash`, the reserved column
  `docs/DATABASE_DESIGN.md` describes. It streams through
  `repair::exact_duplicate::hash_full_file_sha256` (bounded chunks, safe open,
  changed-while-reading refusal). It is stored only when the file is still the
  object the scan inspected: same size and mtime as the row, same device, inode
  and nanosecond mtime as the scan's observation, and an unchanged stamp
  (ctime included) after hashing. Otherwise the column stays `NULL`, the reason
  goes to `ScanPersistSummary::preservation_fingerprint_warnings`, and the next
  scan retries. It does not depend on DAT Verify. A recording that goes missing
  and comes back with a different size or mtime has its old hash cleared and
  re-recorded; one that comes back unchanged keeps it.
- **Freshness is separate.** `Database::verify_preservation_fingerprint` re-hashes
  and reports `Fresh`, `Changed` or `Unavailable` (missing/unsafe/changed while
  reading) without writing. A same-size edit with a restored mtime is invisible
  to the scan and keeps the import-time value; freshness reports it as `Changed`.
  A size/mtime change clears the value on upsert, and the scan records the new
  bytes' fingerprint.
- **Never a game DAT match.** DAT identity persistence skips preservation
  recordings (`preservation_only_skipped`), whatever their hash matched.
- **Accounting.** The "Preservation-only recordings" row (discovery stats
  `recordings`) counts only recordings kept as Library entries, never as games.
  Refused, malformed and unsupported RZX files are counted with the other refusals. The
  discovery details have a *Preservation-only* filter. The scanner keeps an
  unsupported RZX/NEX/PZX variant (`UnsupportedVariant`) apart from a corrupt
  file (`InvalidContent`), and files a size-only MDR lookalike under
  `AmbiguousPlatform`. The Organise page states how many recordings it leaves
  out. The "Add games" preview explains them as preservation-only.
- **Never published.** Library Views plan a recording as `SkipPreservationOnly`
  (counted under Skip, with a reason). The DAT rename and Playing Library
  source collectors leave `.rzx` files out. The Organise page states how many
  recordings these tools leave out.
- **Gaps:** no Blue control runs the freshness check yet (core API only).
  `launch::build_es_de_entry_plan` has no preservation guard of its own. It has
  no production caller today.
  Downgrade is unsupported once a library holds a recording; see
  [Downgrade policy](#downgrade-policy).

## Launch

`launch::fuse_command` plans native Fuse for direct `.tap`, `.tzx`, `.sna`, `.z80`
and `.szx` only, with no machine selection, tape-loading or disk-mounting
options yet (slice C). PZX, UDI, MDR, NEX, TRD, SCL and DSK have no confirmed
launch path. A process starting is never treated as gameplay success.

## POKEs and trainers

`classic_poke_family` and `patch_manager::fuse_local` hold the existing classic
POKE framework. Slice D has not audited Fuse `.pok` handling yet.

## Remaining slices

- **B** library identity: TOSEC Spectrum DAT matching for the A1 formats,
  multi-side/multi-part grouping, release variants/cracks/trainers.
- **C** launch: Fuse machine selection (`--machine`), PZX/TRD/SCL/DSK/MDR
  mounting, ROM/BIOS readiness, and an emulator for NEX (none confirmed).
- **D** Fuse `.pok` audit, trainer launch handoff and cleanup.
- **E** the Blue Spectrum workflow (machine, format, loading help) and acceptance.

## Downgrade policy

**Going back to an older EmuWiz after a scan has kept an RZX recording is
unsupported.** The library itself is never damaged by trying. Reproduced on
2026-10-22 with a disposable private-HOME library holding one PZX game and
one RZX recording (scratch evidence, not committed):

- **v0.9.0** (the published release, schema 16): every library command
  refuses the schema-26 database at its existing gate ("schema version 26 is
  newer than ... this build of EmuWiz understands"). Rows unchanged.
- **Schema-26 builds from before this change** (unreleased; for example the
  v0.10.0-prep candidate): `library-status` and `library-list` work and list
  the recording as an ordinary row. `library-scan` aborts before writing
  anything ("failed to read scan fingerprints"). Rows, the recording's
  fingerprint and the source files are unchanged. The same build scans a
  library without a recording normally. Going forward again, the current build
  rescans the library cleanly.
- Such an older build does not know the recording is preservation-only, so
  it could offer it for play.

Making older schema-26 builds refuse at the schema gate would need a
schema-27 marker migration. That also needs a fresh review of
`database/discovery_import.rs`'s `REVIEWED_SCHEMA` Undo guard, so it is left to
a separate change.

Suggested release-note wording:

> Libraries that contain ZX Spectrum RZX gameplay recordings can't be opened for
> scanning by earlier EmuWiz versions. If you roll back to a previous build
> after scanning RZX files, scanning will stop with a database error; your
> library and files are not changed. Upgrade again to continue. EmuWiz 0.9.0
> refuses the newer library outright, as it does after any upgrade.
