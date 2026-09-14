# PS2 Memory Card File Extraction Validation

Status: research-only validation. No production extraction API, exporter, card
writer, container writer, or Save Vault behavior was added by this document.

## Executive decision

The byte-level rule for reconstructing one regular PS2 memory-card file is now
well enough evidenced to implement a narrowly scoped Phase 2E exporter:

1. accept a validated regular-file directory entry;
2. validate its complete FAT chain without modifying the card;
3. read each referenced allocation-relative cluster in FAT order;
4. remove page spare/ECC bytes by reading only each page's data area; and
5. emit exactly the directory-declared logical file length.

That conclusion is not permission to export arbitrary entries or to write a
PSU/MAX/CBS/SPS/XPS container. A safe first exporter should be one selected
regular file, or at most a plainly named raw save-directory tree after the
destination and filename rules below are implemented. The current inventory
code proves the structure and chain health, but it does not yet reconstruct
file payload bytes.

The recommended readiness boundary is:

| Capability | Decision |
|---|---|
| Regular-file payload reconstruction | YES, format evidence is sufficient |
| Exact final-cluster truncation | YES |
| Fragmented-chain reconstruction | YES, algorithmically and by validated FAT semantics |
| Spare/ECC exclusion | YES |
| Safe filename policy | YES, as a design; not implemented |
| Single-file raw export | READY TO IMPLEMENT as Phase 2E |
| PSU/MAX/CBS/SPS/XPS writer | NOT READY |
| Whole-save export | NOT READY as a first implementation |

## Scope, repository state, and implementation boundary

The audit started at repository SHA `b2e17fb638d466112327065e10a89076415bab11`
(`feat(save-vault): inventory PS2 memory card saves`). The worktree also had
unrelated concurrent modifications in archive workflow, core module exports,
GUI, storage-health code, and an untracked Arcade Manager research document.
Those files are outside this research change and were preserved.

The relevant implementation is:

- `crates/archivefs-core/src/memory_card_inventory.rs`
- structural predecessor commit `d3ac15cf05ca2f40c78d216c1a110c539c93300a`
- inventory commit `b2e17fb638d466112327065e10a89076415bab11`

The current module reads a regular, non-symlink card into immutable memory.
For PS2 it validates the superblock geometry, supports raw 512-byte data-page
and 528-byte data-plus-spare representations, resolves IFC/FAT chains, walks
root and one save-directory level, records raw directory metadata, and reports
typed chain and metadata warnings. It does not expose file payload bytes,
does not copy bytes to a destination, and has no write handle or card mutation
path.

The current `Ps2SaveFile` contains the directory entry, declared size, and
`Ps2ClusterChainHealth`. That is enough to plan a future read, but not enough
to claim that an export has occurred: payload reconstruction and output hashing
remain separate operations.

## Reference sources

The primary and implementation-oriented references inspected were:

1. EmuWiz's fixture validation:
   [`PS2_MEMORY_CARD_FAT_DIRECTORY_FIXTURE_VALIDATION.md`](PS2_MEMORY_CARD_FAT_DIRECTORY_FIXTURE_VALIDATION.md).
   It records the six local fixture hashes, the validated geometry, the
   relative-cluster interpretation, the two-level IFC/FAT formula, directory
   layout, timestamps, and corruption bounds.
2. PCSX2's current
   [`MemoryCardFolder.cpp`](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/SIO/Memcard/MemoryCardFolder.cpp)
   and the stable packed definitions used by the fixture research. The
   current source derives `clusterSize` from `page_len * pages_per_cluster`,
   allocates file clusters with ceiling division, follows the allocation FAT,
   and treats `0xFFFFFFFF` as the zero-length-file marker.
3. Ross Ridge's public-domain
   [`mymc` repository](https://github.com/ps2dev/mymc), especially
   [`ps2mc.py`](https://raw.githubusercontent.com/ps2dev/mymc/master/ps2mc.py),
   [`ps2mc_dir.py`](https://raw.githubusercontent.com/ps2dev/mymc/master/ps2mc_dir.py),
   and [`ps2save.py`](https://raw.githubusercontent.com/ps2dev/mymc/master/ps2save.py).
   The checked source identifies itself as `ps2mc.py 1.11 22/01/15`, uses
   public-domain notices, reads logical pages separately from spare bytes,
   follows FAT chains, and exports EMS/PSU and MAX formats.
4. The small Rust
   [`ps2-memcard` reference parser](https://docs.rs/ps2-memcard/latest/ps2_memcard/),
   used by the existing fixture research as an independent structural
   cross-check. It describes the card as a Sony-specific filesystem with
   two-level allocation tables, 512-byte directory entries, and a spare area
   attached to each page—not as a DOS FAT volume.
5. Public PSU format documentation:
   [PS2 Developer Wiki PSU](https://www.psdevwiki.com/ps2/index.php?section=1&title=PSU)
   and the [EMS PSU format description](https://www.ps2savetools.com/documents/ps2-save-game-format-for-ems-adapter-psu/).
6. The `mymc` README, which documents that PS2 saves are directories containing
   multiple files and that mymc exports `.psu` and `.max`, while SharkPort/
   X-Port and Code Breaker formats are import-only in that tool:
   [`README.txt`](https://github.com/ps2dev/mymc/blob/master/README.txt).

The evidence hierarchy is deliberate. Executable reference code and the local
fixture observations are stronger than a generic format summary. Where a
legacy tool accepts malformed data or has permissive behavior, EmuWiz's
fail-closed safety policy remains stricter.

## Exact payload-addressing model

The proven reconstruction path is:

```text
regular-file directory entry
  -> start_cluster (allocation-relative)
  -> FAT/IFC chain of relative clusters
  -> (alloc_offset + relative_cluster) logical cluster
  -> pages_per_cluster logical pages
  -> page data bytes only
  -> concatenate clusters in FAT order
  -> truncate to declared file length
```

For a validated geometry:

```text
cluster_bytes = page_data_bytes * pages_per_cluster
absolute_cluster = alloc_offset + relative_cluster
logical_page = absolute_cluster * pages_per_cluster + page_index
physical_page_offset = logical_page * page_stride_bytes
payload_page = card[physical_page_offset .. physical_page_offset + page_data_bytes]
```

The current fixture values are:

```text
page_data_bytes   = 512
pages_per_cluster = 2
cluster_bytes     = 1024
page_stride       = 528 for spare-bearing PCSX2 dumps
alloc_offset      = 41
alloc_end         = 8135 (exclusive)
```

Thus a file cluster at relative cluster `r` begins at logical data offset
`(41 + r) * 1024` in a spare-stripped view. In a 528-byte physical image,
the two pages occupy two 528-byte records, but the exported cluster is only
the first 512 bytes of each record. The 16-byte spare area never enters the
logical file stream.

This is exactly the behavior in mymc: `read_page()` reads `page_size` bytes,
then reads the spare area separately; `read_cluster()` concatenates data pages;
`read_allocatable_cluster()` adds the allocation offset before reading the
cluster. The existing EmuWiz `ps2_relative_cluster()` follows the same
allocation-relative rule for inventory reads.

## Spare and ECC bytes

The local PCSX2 fixtures are 8,650,752 bytes, or `16,384 * 528`. Removing the
16-byte spare from each physical page yields 8,388,608 logical data bytes. The
fixture research and mymc agree that the physical representation is 512 data
bytes followed by a 16-byte spare/ECC area.

The spare area is card-integrity metadata, not file content. Export must never:

- copy a raw 528-byte page into the output;
- concatenate physical page records directly;
- include ECC or spare bytes at cluster boundaries; or
- recompute or rewrite ECC while reading.

For raw 512-byte representations, `page_stride == page_data_bytes`, so the
same logical reader degenerates to a contiguous read. Geometry, not a fixed
8 MiB constant, determines the representation. The format permits the
reader to support both known representations while retaining the original
card as an untouched shared container.

The mymc source also attempts ECC validation and falls back to a no-spare
interpretation when ECC data is absent. EmuWiz's current research boundary is
more conservative: no ECC repair or rewrite is proposed, and exact ECC
verification should remain separate until its algorithm and error policy are
independently reviewed.

## File-size and cluster-capacity semantics

The directory entry's `length` is the logical file byte length. It is not the
number of clusters and it is not the physical allocation size. The physical
capacity of a chain is:

```text
chain_capacity = chain_cluster_count * cluster_bytes
required_clusters = ceil(declared_length / cluster_bytes)
```

PCSX2 uses ceiling division when calculating the clusters required for a file.
mymc's `ps2mc_file.read()` limits reads to the file object's declared length,
while its cluster reader supplies complete 1024-byte clusters underneath.
That establishes the required final behavior: if a file declares 1,731 bytes
and has two 1,024-byte clusters, export exactly 1,731 bytes, never 2,048.

The final cluster's unused tail is padding from the memory-card allocation
view, not part of the file. It must not be emitted, hashed as file content, or
used to infer application data.

### Empty files

The validated convention is `length == 0` and `start_cluster == 0xFFFFFFFF`.
There is no FAT chain to read. A future exporter should produce a zero-byte
file only for that exact empty-file case after validating that the entry is a
regular file. A zero-length file with a non-marker start cluster should be
reported as unusual and require strict-mode review; a non-empty file with the
marker must be refused.

The mymc implementation explicitly treats `0xFFFFFFFF` as the zero-length
file case. This is stronger evidence than a filename or extension heuristic.

## Fragmented files

FAT order, not physical order, defines payload order. A valid chain such as:

```text
relative cluster 100 -> 274 -> 91 -> EOC
```

must produce:

```text
data(cluster 100) || data(cluster 274) || data(cluster 91)
```

then be truncated to the directory-declared length. It must not read clusters
100–102 contiguously and must not sort the cluster numbers.

The local fixtures already prove non-contiguous directory chains, including
the `445 -> 447 -> 529 -> 690 -> 851 -> EOC` chain for the God of War save
directory and a fragmented Max Payne directory chain. The PCSX2 and mymc
implementations both follow FAT links rather than assuming contiguous
allocation. That proves the extraction algorithm for fragmented chains,
although a future EmuWiz implementation still needs a byte-for-byte export
test against an independent tool for a real file.

## Corruption and strict extraction rules

Inventory health is evidence for a future exporter, not permission to emit
best-effort bytes. The following conditions must refuse a regular-file export:

| Condition | Future extraction result |
|---|---|
| Start cluster outside `[0, alloc_end - alloc_offset)` | Refuse: `ClusterOutOfRange` |
| FAT link outside allocation range | Refuse: `ClusterOutOfRange` |
| FAT value without allocated bit where a link is required | Refuse: `InvalidFatReference` |
| FAT loop | Refuse: `FatLoop`; no partial output considered successful |
| Chain ends before `ceil(length / cluster_bytes)` | Refuse: `FileSizeExceedsChain` / short chain |
| Card ends before a referenced page or cluster | Refuse: `TruncatedCard` |
| Directory entry is a directory, special, unused, or invalid entry | Refuse: `InvalidDirectoryEntry` |
| Non-empty file has `0xFFFFFFFF` start marker | Refuse: `InvalidFatReference` |
| Name cannot be safely represented for destination | Refuse or require explicit safe-name review |

No clamping, modulo arithmetic, wraparound, or “read until EOF” fallback is
safe. A malformed card must not cause an exporter to read a different cluster
than the one named by the validated chain.

### Chain longer than the declared file

The reference reader returns the logical file length and therefore does not
emit extra clusters. A chain with more clusters than strictly required can be
handled in a future exporter as:

- output exactly the declared length;
- retain an explicit excess-chain warning; and
- default to review/refusal in strict mode until the format policy is settled.

This is not equivalent to a short chain. A complete chain plus excess capacity
does not expose extra bytes as file content, but it may indicate stale or
manually altered allocation metadata. EmuWiz should not silently normalize it.

### Directories and special entries

Only entries whose validated mode contains the used and regular-file bits may
be passed to a single-file exporter. `.` and `..` are directory metadata, not
files. Unused/all-zero/all-`ff` records, contradictory file/directory modes,
system directories such as `BEDATA-SYSTEM`, and deleted/forensic records are
not ordinary export targets.

A future whole-save exporter may recursively represent a save directory, but
that is a different operation with a larger collision and partial-corruption
policy. The PSU implementation in mymc itself rejects a save containing a
subdirectory during its flat EMS export, which is an interoperability
constraint worth preserving rather than hiding.

## Filename and destination safety

The card's 32-byte name field is evidence, not a trusted host path. The raw
bytes must remain in the inventory/export manifest. A display string may use
replacement characters for undecodable bytes, but it must not become an
unquestioned filesystem path.

Future export should apply all of these checks:

- reject `/`, `\\`, `.` and `..` path components;
- reject absolute paths, drive prefixes, UNC-like forms, and NUL bytes;
- reject control characters and names that cannot be represented safely by the
  host filesystem;
- reject names that normalize to an empty or reserved host name;
- detect duplicate names before creating any output;
- use deterministic, documented collision suffixes only if the user explicitly
  accepts a non-byte-identical filename mapping;
- never use a card name to escape the selected destination root;
- perform no overwrite by default;
- re-check the destination using the existing safe-path primitives immediately
  before creation; and
- defend against destination symlinks and directory replacement.

For a first single-file exporter, the safest default is to require an explicit
destination file path selected by the caller and refuse if it exists. For a
save-directory export, the safer default is a two-phase plan: validate every
child name and every chain first, then create outputs only after the complete
plan is accepted.

## Directory and whole-save export

The simplest preservation-first future model is a normal directory tree:

```text
destination/<sanitized-save-name>/<sanitized-child-name>
```

This preserves ordinary payload bytes but does not, by itself, preserve PS2
mode flags, timestamps, directory-entry offsets, allocation clusters, or raw
name bytes. Those should be captured in a sidecar manifest, for example:

- original raw directory name and raw child name bytes;
- display/export name mapping;
- mode and attribute fields;
- created/modified raw timestamp bytes and interpreted timezone policy;
- declared length;
- FAT chain and source logical offsets;
- output SHA-256 and output length;
- card hash and card geometry;
- warnings and provenance.

The sidecar is evidence/provenance, not a promise that a host directory can
recreate a PS2 memory-card filesystem. It must never be used to silently write
back to a card.

For partial corruption, the safest first whole-save policy is strict atomic
planning: if any selected child is corrupt, refuse the whole-save export and
show healthy entries as inspectable but not silently omit them. A future
explicit best-effort mode may export only individually proven files, but it
must list every omitted file and never present the result as a complete save.

## Output integrity and verification

A future single-file export should report:

- source card SHA-256 captured before reading;
- source save directory and raw entry offset;
- declared logical size;
- validated FAT chain;
- output byte length;
- output SHA-256;
- warnings, including excess capacity or metadata anomalies.

The exporter should stream or bounded-buffer data in FAT order while counting
bytes, then require the final count to equal the declared size. It should not
write a successful result before the chain and destination plan are validated.
If an output is written incrementally and a later read fails, the partial file
must be removed or moved to a clearly failed-result quarantine; it must never
be reported as a successful export.

No expected file hash was invented in this audit. The local fixture research
records card hashes, not published per-save payload hashes. A trusted future
test should derive expected hashes from one independently executed extractor,
record the exact fixture/card hash and selected directory entry, and then use
byte-for-byte comparison.

## Independent-tool comparison

The strongest available independent extraction reference is mymc. Its source
shows the exact logical read path: `read_page()` reads only `page_size`,
`read_cluster()` concatenates those logical pages, `read_allocatable_cluster()`
adds the allocation offset, and `ps2mc_file.read()` reads only the requested
logical file length. Its source is public domain and is suitable as a
validation oracle, subject to independent test fixtures.

The `mymc` executable was not available in the current environment, and no
independent export was run against the private local saves. Therefore this
audit does not claim a real-file byte-for-byte hash match. That is the main
verification gap before production export is trusted.

The available local fixture set is the six-file set recorded by the prior
research: four formatted PCSX2 cards and two unformatted RetroArch negative
fixtures. The four formatted cards currently present were read-only hashed
before/after prior inspection and retained their hashes:

| Fixture | SHA-256 |
|---|---|
| PCSX2 `Mcd001.ps2` | `9b1d2efea852b33b717b1449c98163f6355d1390bf25d7c4e7b8df3c3d900e25` |
| PCSX2 `Mcd002.ps2` | `09e4e1ad9725e8a0752833c694dcbe0f17a51846c4bc06a3f3aad79f91a5dd21` |
| Flatpak PCSX2 `Mcd001.ps2` | `f64412aa717006c20bd6f29e4f377bd56271c0b3ffabeede4c03ca63c634056e` |
| Flatpak PCSX2 `Mcd002.ps2` | `4a4c5fdade35929ced348b67fd3b4add2910d27aef4f5c61a79634c43e87e32f` |

No payload contents or personal save titles are reproduced here. Before a
future extractor is accepted, one non-sensitive synthetic or user-authorized
fixture should be exported by both implementations and compared by length and
SHA-256, followed by a fragmented real-file case if available.

## PSU, MAX, CBS, SPS, and XPS assessment

### PSU / EMS

PSU is the most approachable interoperable container. The public description
and mymc implementation agree that it is an uncompressed archive of 512-byte
directory entries followed by file bytes padded to 1,024-byte boundaries. The
first entry describes the save directory, followed by `.` and `..`, then the
child files. It retains directory/file timestamps and modes better than a
plain host directory. The format is nevertheless an external transformation,
has no strong magic in the historical implementation, and has interoperability
edge cases around timestamps, same-named directory/file records, and
subdirectories.

Recommendation: do not make PSU the first EmuWiz writer. First prove raw
single-file export; then add a separately tested PSU writer only after fixture
round trips against mymc/uLaunchELF-compatible samples.

### MAX Drive

MAX is a container with a `Ps2PowerSave` header, directory name, icon-system
name, counts/lengths, CRC, and compressed payload. The mymc implementation uses
LZARI compression and reconstructs metadata from the save. This adds compression
and compatibility surface while preserving less direct raw filesystem detail.

Recommendation: later, if a user need justifies it; not first and not as a
preservation primitive.

### CBS / Code Breaker

CBS is supported by Apollo and common PS2 save tooling, but the available
primary open implementation evidence inspected here is not sufficient to make
EmuWiz's own CBS writer safe. Compression, metadata conventions, and version
interoperability need a separate format audit and known-good corpus.

Recommendation: unsupported until independently specified and tested.

### SPS / XPS / SharkPort

These are legacy proprietary/export ecosystems. The mymc README explicitly
documents import support but not export support for SharkPort/X-Port and Code
Breaker. That is a strong practical signal that writing these formats is not a
small generic wrapper around raw file bytes.

Recommendation: do not implement in Phase 2E or 2F.

### Container decision

For the first implementation, choose raw directory/file export, not a
standard PS2 save container. Raw export is the smallest transformation,
easiest to hash, and clearest about what was preserved. It is not directly
importable into every PS2 tool; that limitation should be stated plainly.
PSU can be a later explicitly labeled interoperability export, never the only
backup representation.

## Metadata preservation

Normal raw-file export can preserve exactly:

- every logical payload byte;
- child filenames after an explicit safe-name mapping;
- `icon.sys` and icon payload files as ordinary regular files;
- declared logical sizes;
- a manifest of raw names, modes, attributes, timestamps, offsets, and chains.

It cannot preserve PS2 directory-entry semantics merely by setting host file
timestamps and permissions. Host metadata is not a reversible representation
of PS2 mode flags or the memory-card allocation layout. The original card
hash and the manifest are therefore essential provenance.

`icon.sys` should remain opaque in Phase 2E. A later informational parser may
display its text, but no icon decompression or title-based identity should be
part of payload export.

## Security and bounds

The future exporter should enforce format-derived and operational limits before
opening an output:

| Resource | Recommended bound |
|---|---|
| FAT hops per chain | `alloc_end - alloc_offset`, with visited-set detection |
| Single-file declared size | no greater than `available_clusters * cluster_bytes`, and no greater than `u32::MAX` from the entry field |
| Single-file chain clusters | no greater than available allocation clusters |
| Directory records | available allocation clusters × records per cluster, additionally capped by the existing `PS2_MAX_INVENTORY_ENTRIES` |
| Save files | existing bounded inventory count; reject plans exceeding it |
| Total raw export bytes | no greater than the selected files' declared sizes and the card's available logical allocation capacity |
| Directory depth | existing `PS2_MAX_DIRECTORY_DEPTH`; do not recurse silently beyond it |
| Name bytes | exactly the validated 32-byte on-card field, with a safe host-name limit applied after decoding |
| Output path | destination-root-relative, canonicalized and symlink-checked |

The exporter must also defend against integer overflow in ceiling division,
cluster offsets, physical-page offsets, output counters, and path lengths. It
must not allocate a buffer based only on an untrusted declared size; streaming
or bounded chunks are safer.

## Recommended future result model

The eventual API should separate planning from execution and keep the source
immutable:

```text
Ps2FileExportPlan
  card_hash
  card_geometry
  save_directory_raw_name
  source_entry_raw_offset
  raw_name
  safe_destination_name
  declared_size
  validated_chain
  source_logical_offsets
  warnings

Ps2FileExportResult
  output_path
  output_size
  output_sha256
  source_card_hash
  verification_state

Ps2ExportError
  invalid_entry
  corrupt_chain
  unsafe_name
  destination_collision
  destination_escape
  output_failure
```

The plan must be immutable evidence captured before writes. Execution should
open the card read-only, revalidate the source file identity/hash if practical,
write only to a newly created destination, and return a result only after exact
length and hash verification.

## Synthetic test plan for Phase 2E

Before trusting production export, add fixture tests for:

1. one-cluster file with exact-length output;
2. multi-cluster file with a short final cluster;
3. fragmented chain such as `100 -> 274 -> 91 -> EOC`;
4. spare-bearing 528-byte image proving output excludes every 16-byte spare;
5. raw 512-byte image proving the same logical bytes are exported;
6. zero-length marker file;
7. zero-length file with an unexpected non-marker cluster;
8. short chain, loop, out-of-range, free/reserved, and truncated-card cases;
9. excess chain with declared-size truncation and explicit warning;
10. directory, `.`, `..`, unused, system, deleted, and contradictory entries;
11. invalid UTF/raw names, separators, traversal, control bytes, duplicates,
    and deterministic collision mapping;
12. destination collision and symlink escape;
13. source-card before/after byte equality and output SHA-256;
14. independent-tool byte-for-byte comparison for at least one real or
    synthetic non-sensitive save.

No test should require writing the source card. A test that needs to construct
a card should construct a synthetic byte buffer or a temporary fixture outside
the user's real memory-card paths.

## Production-readiness answers

**A. Regular-file payload reconstruction proven? YES.** The addressing chain,
logical page handling, FAT order, and declared-length semantics are supported
by PCSX2/mymc source and local fixture evidence.

**B. Final-cluster truncation proven? YES.** PCSX2's cluster allocation uses
ceiling division and mymc reads only the declared logical length.

**C. Fragmented-chain extraction proven? YES, format-wise.** The FAT chain
must be followed in order and fragmented directory chains are present in real
fixtures. A real-file independent hash comparison remains required.

**D. Spare/ECC exclusion proven? YES.** The reference reader explicitly
separates `page_size` from the spare bytes; local fixtures confirm 528-byte
physical pages and 512-byte logical pages.

**E. Safe filename export semantics sufficiently defined? YES, as a design.**
Raw-name retention, strict path rejection, collision detection, no-overwrite,
and destination-root checks are clear. They are not implemented yet.

**F. Raw directory export ready to implement? YES, narrowly.** Phase 2E should
start with one selected regular file and exact output verification.

**G. PSU/MAX/etc. container export ready? NO.** PSU is the best later target,
but container writing still needs independent round-trip fixtures. MAX/CBS/SPS/
XPS need more format-specific evidence.

**H. Whole-save export ready? NO.** It needs an atomic directory plan, complete
child validation, nested-directory policy, filename mapping, collision handling,
and an explicit strict-versus-best-effort decision.

## Smallest next implementation

### PS2 Phase 2E — read-only single-file export

In scope:

- select one inventory-proven regular file;
- require complete, loop-free, in-range chain health;
- read logical data pages only;
- concatenate clusters in FAT order;
- truncate exactly to declared length;
- use a destination-root-safe, no-overwrite path;
- preserve the card bytes and metadata;
- report source-card hash, output length, and output SHA-256.

Out of scope:

- card writes, repairs, undelete, or allocation changes;
- directory export;
- PSU/MAX/CBS/SPS/XPS writers;
- icon decompression or title interpretation;
- automatic filename guessing or overwrite;
- best-effort omission of corrupt files.

### PS2 Phase 2F — whole-save export

Only after 2E has independent byte-for-byte validation should EmuWiz consider
strict whole-save directory export, followed separately by a PSU writer if
interoperability is needed. A best-effort mode should be opt-in and should
produce an explicit incomplete-result manifest.

## Sources

- EmuWiz, [`PS2_MEMORY_CARD_FAT_DIRECTORY_FIXTURE_VALIDATION.md`](PS2_MEMORY_CARD_FAT_DIRECTORY_FIXTURE_VALIDATION.md).
- PCSX2, [`MemoryCardFolder.cpp`](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/SIO/Memcard/MemoryCardFolder.cpp), current `master`, inspected 2026-09-14.
- Ross Ridge, [`ps2dev/mymc`](https://github.com/ps2dev/mymc), public-domain project; `ps2mc.py` source revision `1.11 22/01/15`, `ps2mc_dir.py` revision `1.4 12/10/04`, and `ps2save.py` current `master`.
- [`ps2-memcard` Rust crate documentation](https://docs.rs/ps2-memcard/latest/ps2_memcard/), inspected 2026-09-14.
- [`PSU — PS2 Developer Wiki`](https://www.psdevwiki.com/ps2/index.php?section=1&title=PSU), retrieved 2026-09-14.
- [`PS2 save game format for EMS adapter (.psu)`](https://www.ps2savetools.com/documents/ps2-save-game-format-for-ems-adapter-psu/), published 2013-01-19, retrieved 2026-09-14.
- EmuWiz PS2 structural/inventory implementation commits `d3ac15cf05ca2f40c78d216c1a110c539c93300a` and `b2e17fb638d466112327065e10a89076415bab11`.
