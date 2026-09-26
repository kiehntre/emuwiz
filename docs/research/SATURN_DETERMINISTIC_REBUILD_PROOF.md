# Saturn deterministic data-track rebuild proof

Status: proof-only, synthetic fixtures, no production rebuild or apply path.

## Result

EmuWiz can compare a candidate rebuilt data track against a complete Saturn
manifest and identify changed versus preserved cooked sectors. It can also
prove that two independently materialized outputs are equivalent while
ignoring their temporary filesystem paths.

That comparator is now implemented as
`archivefs_core::saturn_rebuild_proof`. It is deliberately not a builder:
there is no production ISO writer, CUE writer, patch applier, output publisher,
or GUI Apply action.

The synthetic proof demonstrates a constrained result:

- identical inputs and deterministic layout rules produce byte-identical
  outputs;
- a same-size replacement leaves the descriptor, audio tracks, track
  topology, INDEX values, pregaps/postgaps, System ID, and data-track length
  unchanged, with the replacement sector isolated as the changed sector; and
- a replacement crossing a sector-allocation boundary changes the packed
  filesystem layout and data-track length, so the comparator refuses the
  preservation claim as a topology change.

This is not evidence that an arbitrary Saturn builder is safe. A future
builder must be pinned, independently audited, and verified against these
same invariants.

## Builder and library audit

| Candidate | Licence/status | Findings | Decision |
|---|---|---|---|
| GNU xorriso / libisoburn / libisofs | GPL-2.0-or-later for the xorriso distribution; libburnia components are separately documented | Creates ISO-9660 images and exposes timestamp/date controls and an API, but is not installed in the proof environment and does not by itself preserve Saturn CUE/audio/security topology | Research candidate only |
| cdrtools `mkisofs` | Open-source project with licensing considerations that must be checked for the exact release and distribution | Mature ISO builder, but no approved EmuWiz dependency or Saturn mixed-mode preservation contract was established | Not selected |
| Existing Rust dependencies | Read-only optical/CUE/ISO readers and CHD adapters | No ISO9660 writer or Saturn data-track rebuild primitive exists | Not a builder |
| SSP / Sega Saturn Patcher | External tool/package; byte-level semantics and reproducible rebuild contract remain unproven | Cannot be invoked or treated as a deterministic backend | Refused |

Sources consulted:

- GNU xorriso project and licence information: <https://www.gnu.org/software/xorriso/>
- Debian xorriso manual, including image creation and timestamp behavior:
  <https://manpages.debian.org/unstable/xorriso/xorriso.1.en.html>
- cdrtools `mkisofs` project documentation:
  <https://github.com/Distrotech/cdrtools/blob/master/README.mkisofs>
- reproducible-builds `SOURCE_DATE_EPOCH` specification:
  <https://reproducible-builds.org/specs/source-date-epoch/>
- EmuWiz Saturn patching audit:
  `docs/research/SATURN_PATCHING_SSP_RESEARCH.md`

## Fields that must be controlled or measured

A data-track rebuild can change all of the following even when one file is
the declared logical replacement:

- directory entry ordering and path-table ordering;
- file extents, allocation rounding, and inter-file padding;
- directory/file timestamps and volume timestamps;
- volume identifiers, volume-space size, path-table locations, and padding;
- logical sector size and raw sector mode/form;
- boot/System ID bytes and any security or mastering area;
- file ordering and total data-track length; and
- CUE file boundaries, INDEX 00/01, pregap, postgap, and track references if
  the descriptor is regenerated.

`SOURCE_DATE_EPOCH` can make timestamps reproducible for tools that honor it,
but it does not establish Saturn preservation. Exact file ordering, builder
version/options, volume metadata, boot bytes, and output topology still need
to be fixed and verified.

## Synthetic fixture

The proof tests create a legal ISO-9660-shaped, entirely synthetic data track:

- a fixed `SEGA SEGASATURN` System ID with synthetic product/title fields;
- a Primary Volume Descriptor, root directory records, path-independent fixed
  logical sectors, and two synthetic files;
- one replaceable file and one unchanged-content file;
- a mixed-mode CUE with one MODE1/2048 data track and two AUDIO tracks;
- explicit `INDEX 00`, `INDEX 01`, `PREGAP`, and an in-file audio INDEX 00;
- independent synthetic audio component files; and
- no copyrighted image, patch, BIOS, or external media.

The test-only layout packs directory file extents in deterministic name/order
sequence. It is an experiment fixture, not a Saturn mastering implementation.

## Measured proof cases

### Repeat build

Two builds with identical synthetic inputs and rules produced byte-identical
data/audio components and equivalent manifests. No nondeterministic field was
observed because the fixture fixes ordering, timestamps, metadata, padding,
and content bytes.

### Same-size replacement

Only the replacement data sector changed. The descriptor hash, audio hashes,
track order/modes, INDEX 00/01, pregaps/postgaps, System ID, and data-track
sector count remained equal. The proof comparator identifies the changed and
preserved sector sets directly.

### Smaller replacement

The replacement crossed from two allocated sectors to one. The directory
record size and the following file's extent moved; the data-track length also
changed. Audio stayed unchanged, but the typed proof reports a topology delta
and refuses a preservation-safe result.

### Larger replacement

The replacement crossed from two allocated sectors to three. The following
file's extent moved and the data-track length increased. Audio stayed
unchanged, but the proof again reports a topology delta and refuses a
preservation-safe result.

## Proof model

`SaturnRebuildProof` records:

- `deterministic` — set only after comparing repeated outputs;
- changed and preserved cooked data sectors;
- typed topology deltas;
- System ID deltas;
- audio invariant state;
- filesystem delta notes;
- warnings; and
- typed refusals for missing/ambiguous data tracks, unsupported modes, read
  failures, audio/System ID changes, descriptor changes, incomplete manifests,
  and topology movement.

The comparator does not infer file-level filesystem identity from hashes. A
future production builder must provide a bounded filesystem diff that binds
file names, extents, sizes, and contents to the sector proof.

## Production decision

No safe production rebuild path exists yet.

Missing primitives are:

1. a licensed, pinned ISO9660 builder with deterministic ordering, timestamps,
   extents, padding, and metadata controls;
2. Saturn-aware preservation of System ID/security/mastering bytes;
3. a mixed-mode output writer that retains exact audio components and CUE
   topology; and
4. a bounded filesystem/file-extent diff integrated with the existing
   manifest verifier and transaction system.

Until those primitives exist, Saturn rebuild remains proof/readiness-only and
must refuse Apply.
