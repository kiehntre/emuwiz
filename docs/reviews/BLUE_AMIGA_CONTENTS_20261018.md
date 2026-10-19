# Blue bounded filesystem contents: RDB, HDFX and ADF exposure

Initial fresh GitHub main: `6fb066491fc960aef0d2d15ef12c611e250ba7a4`.
Existing contents foundation: `4071a52dd8e31ffa97dc02061b8f198e2a556fd8`.
The preceding reconciliation is preserved locally at `ab6c3783` and in the
`2026-10-18-amiga-reconciliation` audit ledger; no historical ref was removed.

The missing join was GUI eligibility: Game Details already called the contents
panel, but that panel accepted only `.hdf` names. Its core reader already detects
RDB and flat AmigaDOS structures independently of a suffix. This change extends
the same panel to `.rdb`, `.hdfx` and `.adf`, with neutral contents labels and an
explicit Inspect / refresh action. Merely opening Details starts no inspection.
A suffix offers an inspector; it never establishes a supported format or platform.
HDFX here is a filename convention for an inspected raw/RDB image, not a new codec.

## Supported and refused combinations

| Actual structure | Filesystem | Contents result |
|---|---|---|
| Checksummed, non-overlapping RDB/PART container, including HDF/HDFX/RDB-named files | Validated OFS DOS0/2/4/6 or FFS DOS1/3/5/7, 512-byte filesystem blocks | Existing bounded partition selection and directory listing. |
| Flat image at offset zero, including HDF/HDFX and filesystem-backed raw DD/HD ADF | Same validated DOS0–7 OFS/FFS, 512-byte filesystem blocks | Existing bounded directory listing; no fabricated partition label. |
| Either valid layout under another offered suffix, including a mismatched `.adf` or `.rdb` name | Same validated supported filesystem | Actual detected layout is displayed; the suffix supplies no evidence. |
| Recognized RDB/flat container | PFS, SFS, MuFS or unknown filesystem | No contents page; explicit unsupported-filesystem refusal. Container partition choices remain visible where valid. |
| Recognized FFS with larger filesystem blocks | For example a generated 1024-byte-block root | Explicit UnsupportedBlockSize; no partial listing. |
| Custom/protected/plausible ADF without supported valid filesystem metadata; Acorn/non-Amiga bytes | No validated OFS/FFS | No listing. Plausible media detection is not filesystem support. |
| ADZ/gzip, DMS, IPF, HFE/track encodings, archive containers | Any | No browser eligibility/decoder added. Synthetic DMS/IPF/extended-ADF signatures refuse core inspection. |
| Overlap, cycle, corrupt metadata/checksum, contradictory partition/boot signatures, ambiguous names/entry types, unsafe source | Any | Existing refusal remains; no new fallback. |

Only filesystem blocks of 512 bytes are traversed. Existing RDB metadata block
size/geometry/range validation is unchanged; no new offset or block-size formula
was introduced. Generated direct tests exercise all eight DOS variants, both
layouts across all four offered suffixes, and standard DD/HD raw ADF geometry.

## Read contract and unchanged safeguards

The owner explicitly selected preservation of the existing RDB-first probe and
provided the following wording, shown visibly in Blue:

> Blue performs bounded container probing and filesystem-metadata traversal only. It does not follow file-data block chains, extract files, read file contents, or claim payload integrity. Initial container-probe sectors may physically overlap regions later classified as file data.

The original detector, descriptor-pinned safe opens, range device, checksums,
link/special-entry handling, source freshness and generation checks remain.
Budgets remain 128 entries/page, 4096 entries/directory, depth32, 10,000 observed
directory nodes and 8192 sector reads per filesystem request. Existing bounded
RDB probing is retained. Metadata freshness is a point-in-time check, not an
atomic snapshot/full-image hash; stalled host reads have no wall-clock deadline.
No file-data chains, payload validation, extraction, mounting, import, repair,
writing, identity updates, DAT updates or Launch actions were added.

Test-only thread-local hooks observe the existing container and positional read
sites. Generated file headers point to poisoned sectors inside and outside the
container prefix. Tests explicitly demonstrate the accepted prefix overlap and
that filesystem browsing does not follow those file-data pointers. Hooks are
`cfg(test)` only and do not change production descriptors, offsets or budgets.

The original HDF owner released the contents modules, fixtures and both Amiga
reader bridge files. No shared Blue `pages.rs`, `mod.rs` or library root edit was
needed. The old HDF worktree, dirty development trees and running owner GUI remain
untouched. Validation uses private HOME/XDG/TMP/app roots and generated media only.

Exact baseline failure, candidate file/diff hashes, focused build/test results,
pre-commit independent review, post-review replay and rerun evidence are recorded
in `/home/davedap/emuwiz-audit-ledger/2026-10-18-blue-amiga-contents/`.
