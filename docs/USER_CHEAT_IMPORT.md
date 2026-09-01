# User cheat import — Phase 1

EmuWiz Phase 1 can index an existing personal cheat collection without
changing the collection or any emulator installation. The backend accepts one
plain file or recursively scans a directory, then returns bounded candidates,
parser diagnostics, provenance, duplicate groups, and library-match evidence.

## Supported formats

- RetroArch/libretro `.cht` files, parsed by the existing bounded CHT document
  parser. Descriptions, enabled-by-default state, and code presence are
  inspected; the source bytes are never rewritten.
- PCSX2 `.pnach` files, using the existing strict PNACH patch-line validator.
  `gametitle=` metadata is retained as a title hint, while the conventional
  `SERIAL_CRC.pnach` filename supplies optional serial and executable-CRC
  evidence. A file must contain at least one valid `patch=` line.

Other plain files are ignored by the format adapter. Executables, scripts,
archives, and native binaries are explicitly rejected or reported as
unsupported; nothing is executed and archives are not unpacked in this phase.

## Trust and matching

Every candidate has `UserSupplied` provenance. That describes origin, not
trust: a user file is unverified input and cannot authorize emulator writes.

Matching is evidence-based:

- `Exact` requires objective identity evidence such as serial, title ID, CRC,
  or content hash together with compatible platform evidence.
- `Strong` means normalized title and platform agree, but objective identity is
  absent.
- `Possible` records a weaker title/platform/identifier signal.
- `Ambiguous` is returned when multiple library games tie for the best match.
- `NoMatch` means no library evidence agrees; malformed or unsupported input
  is reported separately as `Unsupported` diagnostics.

Filename text alone is never exact identity evidence. Ambiguous candidates are
returned for review and are never attached automatically.

## Bounds and safety

The default scan limits are:

- 8 MiB per file;
- 256 MiB cumulative file bytes;
- 10,000 visited regular files;
- 32 directory levels;
- 16,384 retained cheat/code records per file;
- 256 retained report diagnostics.

Directory and file symlinks are not followed. Directory traversal is sorted for
deterministic results. A malformed file contributes a bounded diagnostic and
does not abort the rest of a directory scan. Exact duplicate files are grouped
by SHA-256; no duplicate is deleted or replaced.

## Read-only Phase 1 boundary

The report explicitly states that it is read-only, performed zero writes, and
has no apply capability. It does not install, enable, disable, normalize, move,
delete, or copy cheats; it does not modify emulator files or the library
database. A later phase can feed reviewed candidates into EmuWiz's existing
preview/approval/apply machinery, preserving this original-file provenance.
