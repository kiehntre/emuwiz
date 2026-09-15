# PS2 Whole-Save Container Formats

## 1. Executive Summary

The first whole-save export should be **PSU (EMS/uLaunchELF)**. It is a raw
container, has no container compression or encryption, preserves the PS2
directory-entry shape (including names, timestamps, permissions/mode bits and
attributes), and has an independent reader/writer path in the public-domain
`mymc` project. PSU is also the format used by uLaunchELF and is supported for
export by PCSX2's documented memory-card tooling.

The recommendation is deliberately narrower than “PSU is universally best”.
It means that EmuWiz can implement a bounded, deterministic PSU writer without
first implementing LZARI, RC4, or a proprietary wrapper. MAX is a reasonable
second target, but its LZARI stream is an additional implementation and
interoperability risk. CBS, SPS, and XPS remain import/conversion research
targets until independent writer fixtures and byte-level specifications are
available.

The byte-level PSU contract below is based on the public `mymc` implementation
and the PSU/PS2 filesystem references. Fields described by those references are
distinguished from fields that remain uncertain. No local card is modified by
this research.

## 2. What Constitutes a PS2 Save

A PS2 savedata item is a top-level directory shown as one icon by the PS2
browser, not one flat file. The directory contains the game-owned regular files
and normally includes `icon.sys` plus one or more icon assets. The memory-card
filesystem represents each file or directory with a 512-byte entry and stores
file data through FAT cluster chains. PS2 timestamps use a six-field time-of-day
encoding and Japan Standard Time (+09:00) in the documented filesystem model.

For the first writer, one save means:

* exactly one validated top-level save directory;
* all validated regular files below it that the inventory can safely
  reconstruct;
* original raw names, lengths, data, timestamps, modes and attributes where
  available;
* `icon.sys` and icon files are ordinary members and must be retained, not
  regenerated;
* no unresolved chain, unsafe name, unsupported nested directory, duplicate
  name, or contradictory entry is silently omitted.

The current inventory records a save directory and its direct children/files.
It reports nested directories as bounded warnings rather than recursively
inventing a complete tree. Therefore the first writer should reject a save with
unresolved nested content rather than produce a partial save.

## 3. PSU Format

### Proven byte layout

PSU has no magic number. Detection is structural: the first three 512-byte
records must decode as a directory, `.` directory, and `..` directory, with the
root directory count including those two entries. This is the detection rule
used by `mymc`; an extension is not authoritative.

All integer fields in the known entry layout are little-endian. A directory
entry is 512 bytes:

| Offset | Size | Field |
|---:|---:|---|
| 0x00 | 2 | mode/flags (`u16`) |
| 0x02 | 2 | reserved/unknown |
| 0x04 | 4 | logical length (`u32`) |
| 0x08 | 8 | created time: reserved, second, minute, hour, day, month, year (`<xBBBBBH`) |
| 0x10 | 4 | FAT cluster in a card; zero in PSU export entries |
| 0x14 | 4 | parent entry; zero in PSU export entries |
| 0x18 | 8 | modified time, same encoding |
| 0x20 | 4 | attributes |
| 0x24 | 0x1C | reserved/padding |
| 0x40 | 0x1C0 | NUL-terminated raw filename field |

The `mymc` format code uses the equivalent packed layout
`<HHL8sLL8sL28x448s`. The mode flags include read/write/execute, protected,
file/directory, hidden and existence bits; unknown bits must be preserved only
when their meaning is known and safe. Names are normally limited to 32 bytes by
the filesystem, are case-sensitive, and cannot contain `/`, `?`, `*`, ASCII
control characters, or an embedded NUL as meaningful content.

The PSU stream is:

1. root directory entry;
2. `.` entry;
3. `..` entry;
4. one 512-byte entry and file payload for each regular member;
5. each payload padded with zero bytes to the next 1024-byte boundary.

The root count is `2 + number_of_files`. The two dot entries are directories,
have zero length, use the root timestamp, and use the root directory name
relationship. The public writer rejects subdirectories; this is a useful
interoperability boundary for EmuWiz until a nested-directory PSU fixture is
independently validated. PSU has no documented container checksum or
compression field. Integrity must therefore come from staged output, final
hash/size verification, and an independent parser.

### Metadata and determinism

PSU is particularly suitable because it carries timestamps and permissions,
unlike a plain folder copy. The writer must preserve the source raw values when
valid. It must not use current time, truncate names, or normalize mode bits as
an unrecorded fallback. The entry order should be a documented canonical order
(recommended: source directory order, with a stable bytewise-name tie-breaker)
and repeated exports from identical evidence must produce identical bytes.

## 4. MAX Format

MAX files identify themselves with the 12-byte ASCII string `Ps2PowerSave`.
The 0x5c-byte little-endian header used by `mymc` is:

* magic[12];
* CRC32[4], calculated over the header with the CRC field zero plus the stored
  body;
* directory name[32];
* icon.sys display name[32];
* compressed length[4];
* file count[4];
* uncompressed length[4].

The body is LZARI-compressed. Its uncompressed records contain a 32-bit file
length, a 32-byte name, the file bytes, and padding to the next 16-byte record
boundary. The public reader notes a historical size-field variation, which is
another reason not to make MAX the first writer. MAX generally flattens file
metadata to generated directory/file defaults in the `mymc` path; it does not
provide the same straightforward metadata fidelity as PSU. A writer requires a
correct, deterministic LZARI encoder and compatibility fixtures.

## 5. CBS Format

CBS is identified by `CFU\0`. The public `mymc` parser describes a variable
header whose length is stored near the start, followed by a body length and
directory metadata. The body is RC4-transformed using a fixed initial state and
then zlib-decompressed. The decompressed body is a sequence of 64-byte file
headers followed by file bytes. The file header carries names, size, mode and
timestamps, but the parser observes that some directory mode/time fields are
not consistently valid and supplies fallbacks.

CBS therefore has compression, obfuscation, variable header behavior, and
known metadata quirks. A writer would need to define exact compatibility with
CodeBreaker readers and independently verify the checksum/trailer behavior.
It is not a safe first target for a minimal independent exporter.

## 6. SPS Format

SPS is the SharkPort save wrapper. The public parser identifies a 17-byte
prefix containing `SharkPortSave` preceded by `0d 00 00 00`, then reads a
little-endian save type and length-prefixed directory name, date string, and
comment. A file-length field precedes a variable-length directory descriptor.
The descriptor contains a 64-byte name, file count/length, mode, timestamps and
extension bytes; file descriptors follow with a similar variable header and
raw file data.

The parser ignores a four-byte trailing checksum, and the format notes include
byte-swapped mode values. This is evidence of a tool/device wrapper rather than
a small stable interchange contract. SPS must remain deferred until a writer
fixture is accepted by an independent SharkPort-compatible reader.

## 7. XPS Format

XPS is commonly treated as SharkPort/X-Port data, but it must not be silently
merged with SPS. The PS2 Save Tools XPS analysis is explicitly observational
and warns that parts may be incorrect. Its documented layout starts with
version-like `0x0000000d`, `SharkPortSave`, then length-prefixed filename/date
fields, a zero field, a body length, variable file descriptors, and a final
checksum. The descriptor is approximately 250 bytes in common files and
contains both ASCII and Shift-JIS title fields, dates, attributes, and sector
fields.

The overlap with SPS is real, but version markers, wrapper fields, descriptor
interpretation, and checksum behavior require separate fixtures. XPS has no
adequate basis for a deterministic EmuWiz writer in this slice.

## 8. Current EmuWiz Inventory Mapping

| Inventory fact | Container mapping | Status |
|---|---|---|
| `Ps2SaveDirectory.entry.raw_name` | save directory name | available |
| `Ps2SaveDirectory.entry.raw_mode` | directory mode/flags | available |
| `entry.attributes` | attributes field | available |
| `entry.created` / `modified` including raw bytes | timestamp fields | available when valid/raw preserved |
| `Ps2SaveFile.entry.raw_name` | file name field | available |
| `declared_size_bytes` | logical file length | available |
| validated `chain_health.clusters` | read source data in order | available |
| reconstructed data pages | PSU file payload | available through existing read-only logic |
| `icon.sys` and icon assets | ordinary PSU members | available if inventory exposes them as regular files |
| child entry kind | regular-file eligibility | available |
| card geometry/spare handling | source reconstruction only | already enforced by inventory/export |

The PSU writer should consume immutable evidence, not reread arbitrary paths or
walk the card independently. Source card bytes remain read-only.

## 9. Missing Inventory Facts

Before implementation, either expose or validate these facts at the writer
boundary:

* complete recursive path information for every member, or an explicit
  rejection of nested directories;
* exact raw 512-byte entry representation if unknown/reserved bytes must be
  preserved rather than canonicalized;
* a clear distinction between a valid timestamp and a timestamp whose raw bytes
  were retained only for diagnostics;
* duplicate/case-sensitive name checks within the exported root;
* a save-level proof that all direct children were observed and their FAT
  chains are complete;
* a bounded, validated representation of `icon.sys` and other icon files;
* an explicit policy for source entries whose mode contains unknown bits.

The present inventory deliberately reports nested-directory depth warnings and
does not provide a complete recursive tree. That is adequate for a first
single-level PSU writer only if such saves are rejected or separately proven.

## 10. Deterministic Writer Requirements

The future writer should have a pure planning phase and a create-only apply
phase:

1. Plan from one validated save directory and its immutable inventory evidence.
2. Reject incomplete chains, unsafe names, unsupported nested directories,
   duplicate names, invalid metadata, count/size overflow, and existing output.
3. Encode every entry with little-endian fields and a fixed zeroed reserved-byte
   policy.
4. Emit root, `.`, `..`, then regular files in the documented canonical order.
5. Write each payload exactly at its logical length, then zero-pad to 1024
   bytes; never include card spare/ECC bytes.
6. Stage beside the destination under an owned temporary name, flush and verify
   size/SHA-256, then publish atomically.
7. Recheck source-card hash and evidence before publication; remove only the
   owned staging file on failure.

No timestamp should be invented for deterministic output. If a required field
is unavailable, fail the plan rather than silently replacing it with “now”.
PSU has no container checksum, so the result record must carry the output hash
and provenance even though those bytes are not part of PSU.

## 11. Independent Validation Options

* **mymc / myMCpp:** parse the generated PSU and import it into a temporary
  PCSX2 memory-card image. `mymc` is public domain; its parser/writer is useful
  as an independent behavioral oracle, not a reason to copy implementation.
* **uLaunchELF ecosystem:** PSU is the native practical transfer format for
  moving a save to a real PS2 memory card; validation should use a disposable
  card/image or a documented emulator workflow, never a personal card as a
  test target.
* **PCSX2 documentation:** PCSX2 documents exporting saves to PSU or MAX and
  using MyMC for memory-card images.
* **PS2 Save Builder / PS2 Save Utility / psv-save-converter:** useful
  conversion/readback checks for synthetic saves, subject to their individual
  licenses and platform availability. Converter success is secondary to an
  independent PSU parser and exact byte/hash tests.

No proprietary save payload is required. A synthetic save containing
`icon.sys`, an icon asset, and small regular files is enough for round-trip
validation.

## 12. Security Requirements

The writer must:

* accept only a validated read-only inventory and complete file chains;
* bound file count, name length, total logical bytes, padding, and output size;
* reject NUL, control, separator, traversal-like, duplicate, and unsupported
  names;
* reject malformed or ambiguous directories and unsupported nested content;
* preserve the source card and never open it for writing;
* use a create-new staged destination and refuse an existing final path;
* validate destination parent and symlink behavior;
* verify final byte count and SHA-256 before atomic publication;
* clean staging output on every failure without deleting unknown files;
* report partial cleanup/publication failure as failure, never success.

## 13. Format Comparison

| Format | Complexity | Compression / crypto | Checksums | Metadata fidelity | Independent validation | Documentation | Risk | Priority |
|---|---|---|---|---|---|---|---|---|
| PSU | Low | none | none documented | high for entry metadata | mymc, uLaunchELF, PCSX2 | good enough with public code | low | **1** |
| MAX | Medium/high | LZARI | CRC32 header/body | moderate; many fields synthesized | mymc, converters | fair | encoder/interoperability risk | 2 |
| CBS | High | zlib + fixed-state RC4 | format details variable | moderate/quirky | mymc, converters | reverse-engineered | crypto/wrapper risk | deferred |
| SPS | Medium | raw wrapper | trailing checksum behavior uncertain | moderate | converters/tools | partial | descriptor/checksum ambiguity | deferred |
| XPS | Medium | raw wrapper | trailing checksum behavior uncertain | moderate/high fields, uncertain semantics | converters/tools | explicitly observational | revision ambiguity | deferred |

The table ranks writer suitability, not popularity. Existing readers can still
be supported independently of writer priority.

## 14. Recommended First Implementation

Implement **deterministic create-only PSU export for one validated, single-level
save directory**. This matches the existing inventory's strongest guarantees,
requires no new compression or cryptographic dependency, preserves source data
and metadata, and can be checked by `mymc` plus a byte-for-byte synthetic
fixture.

The writer must reject nested directories until recursive inventory facts and a
reader-validated PSU nested-directory contract are established. It must also
reject a save with missing/inconsistent `icon.sys` only if the selected product
contract requires icon metadata; PSU itself can carry ordinary files, so this
should be an explicit product policy rather than an invented format rule.

## 15. Deferred Work

MAX requires a bounded deterministic LZARI encoder and compatibility corpus.
CBS requires independently verified RC4/zlib framing, checksum behavior and
metadata policy. SPS and XPS require separate versioned fixtures, including
their descriptor and checksum differences. Recursive PSU directories, card
write-back, multi-save bundles, PSV signing, and GUI whole-save export are also
out of this research slice.

### Next implementation slice

Add a core-only `Ps2WholeSaveExportPlan` and `Ps2PsuWriter` beside the existing
PS2 inventory/export module (likely `crates/archivefs-core/src/ps2_save_export.rs`,
with a minimal `lib.rs` export only if the module boundary requires it). The
plan should contain the source-card hash, save-entry identity, ordered file
evidence, destination, output-size bound, and provenance. The writer should
encode the 512-byte entries and 1024-byte payload padding into an owned staged
file, verify size/SHA-256, and atomically publish create-only.

Tests should generate a tiny synthetic card/save with `icon.sys`, a fragmented
regular-file chain, raw timestamps/modes, unsafe-name and incomplete-chain
refusals, unchanged source hash, deterministic repeated output, existing
destination refusal, staging cleanup, and nested-directory rejection. Validate
the resulting PSU by an independent `mymc` parser/import against a temporary
card image, plus exact expected bytes for the synthetic fixture. No GUI is
needed in that slice.

### Sources and implementation/licence notes

* [Ross Ridge's public-domain `mymc` README](https://github.com/ps2dev/mymc/blob/master/README.txt)
  documents PSU/MAX export and SPS/CBS import-only support.
* [`mymc` PSU/MAX/CBS/SPS parser and writers](https://github.com/ps2dev/mymc/blob/master/ps2save.py)
  and its [public-domain directory-entry definitions](https://github.com/ps2dev/mymc/blob/master/ps2mc_dir.py)
  provide the byte layouts used above. Studying a public-domain implementation
  is permissible under that source's stated terms; EmuWiz should still write
  an independent implementation.
* [PS2 Developer Wiki PSU overview](https://www.psdevwiki.com/ps2/index.php?section=1&title=PSU)
  describes PSU's purpose and metadata-preserving behavior.
* [PS2 Save Tools XPS notes](https://www.ps2savetools.com/documents/xps-format/)
  explicitly characterize the XPS layout as reverse-engineered and potentially
  incomplete.
* [PCSX2 memory-card documentation](https://pcsx2.net/docs/configuration/memcards/)
  documents MyMC and PSU/MAX export in the PCSX2 workflow.
* [PCSX2's PS2 memory-card filesystem reference](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/Reference/PS2-MemoryCardFileSystem.htm)
  documents the underlying 512-byte entries, timestamps, names, modes and FAT
  model, while warning that it is an independent reverse-engineering document,
  not an official Sony specification.
* [PS2SaveUtility](https://github.com/root670/PS2SaveUtility) is a useful
  independent conversion/readback reference. Its repository reports support
  boundaries and has its own license; neither its code nor third-party save
  content is included in this research.

Format behavior, source licences, database/catalogue rights, and copyrighted
save-content rights are separate questions. This document relies on public
technical references only; it does not grant permission to redistribute game
save payloads or proprietary tool assets.
