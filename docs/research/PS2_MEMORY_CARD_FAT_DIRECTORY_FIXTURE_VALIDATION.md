# PS2 Memory-Card FAT and Directory Fixture Validation

Status: research and read-only fixture validation only. No production parser,
save enumeration, extraction, or card mutation was added.

## Scope and method

This validation used existing PCSX2 virtual memory-card files on the host. Each
file was opened read-only, converted in memory from 528-byte physical pages to
512-byte data pages, and inspected with bounded Python one-liners. No file was
mounted, opened for writing, timestamped, repaired, or copied. SHA-256 was
recorded before and after the reads.

The repository's current PS2 implementation remains structural-only:
[`memory_card_inventory.rs`](../../crates/archivefs-core/src/memory_card_inventory.rs)
recognises geometry and deliberately returns no filesystem entries. The prior
format audit correctly identified FAT indirection and directory layout as
unresolved; this document supplies the fixture evidence needed to decide the
next slice.

## Fixture inventory and immutability

| Fixture | Size | SHA-256 | Result |
|---|---:|---|---|
| `~/.config/PCSX2/memcards/Mcd001.ps2` | 8,650,752 | `9b1d2efea852b33b717b1449c98163f6355d1390bf25d7c4e7b8df3c3d900e25` | formatted, populated |
| `~/.config/PCSX2/memcards/Mcd002.ps2` | 8,650,752 | `09e4e1ad9725e8a0752833c694dcbe0f17a51846c4bc06a3f3aad79f91a5dd21` | formatted, empty |
| `~/.config/retroarch/system/pcsx2/memcards/Mcd001.ps2` | 8,650,752 | `47ebe237a3987f843fc19b0f801ce1edc1690768ef6b18e4b03a12ca6b298358` | unformatted/all `ff` |
| `~/.config/retroarch/system/pcsx2/memcards/Mcd002.ps2` | 8,650,752 | `47ebe237a3987f843fc19b0f801ce1edc1690768ef6b18e4b03a12ca6b298358` | same unformatted image |
| `~/.var/app/net.pcsx2.PCSX2/config/PCSX2/memcards/Mcd001.ps2` | 8,650,752 | `f64412aa717006c20bd6f29e4f377bd56271c0b3ffabeede4c03ca63c634056e` | formatted, populated |
| `~/.var/app/net.pcsx2.PCSX2/config/PCSX2/memcards/Mcd002.ps2` | 8,650,752 | `4a4c5fdade35929ced348b67fd3b4add2910d27aef4f5c61a79634c43e87e32f` | formatted, empty |

The after-read SHA-256 values were identical to these before-read values for
all six paths. The two RetroArch files are byte-identical to each other and
contain no valid Sony superblock; they are useful negative fixtures, not
filesystem-layout fixtures. The four formatted images all report the same
standard geometry, while their directory contents differ.

## Source inventory

The local sources inspected were:

* `docs/research/PS2_MEMORY_CARD_FORMAT_AUDIT.md`, which records the existing
  structural-only boundary and the former unresolved FAT/name questions.
* `docs/research/APOLLO_SAVE_TOOL_AUDIT.md`, especially its PS2 `McFsEntry`
  summary and the attribution to the low-level `mcio`/PS2-MCA-derived code.
* The current EmuWiz structural parser linked above.

Strong external implementation references were also inspected:

* [PCSX2 `MemoryCardFolder.h` superblock and entry definitions](https://sources.debian.org/src/pcsx2/1.6.0%2Bdfsg-1/pcsx2/gui/MemoryCardFolder.h/), including the packed superblock offsets, date structure, mode flags, 512-byte entry, 32-byte name, and relative cluster semantics.
* [Current PCSX2 `MemoryCardFolder.cpp`](https://github.com/PCSX2/pcsx2/blob/master/pcsx2/SIO/Memcard/MemoryCardFolder.cpp), including its two-level FAT setup and chain traversal.
* The small, read-only [`ps2-memcard` parser source](https://docs.rs/ps2-memcard/latest/src/ps2_memcard/lib.rs.html), used as an independent executable-format cross-check. Its source explicitly documents 512-byte data pages, 528-byte dumps, 512-byte directory entries, two-level allocation-table lookup, and fail-closed chain checks.
* [`ps2dev/mymc`](https://github.com/ps2dev/mymc), for the original public utility context and the fact that PS2 saves are directories containing multiple files.

The current PCSX2 source is the most useful semantic reference for the
directory and allocation structures. The Debian source is an older packaged
snapshot, so it is used for stable structure definitions rather than as proof
of a particular current PCSX2 implementation version.

## Page, cluster, and card geometry

All four formatted fixtures agree with the current structural model:

| Field | Offset | Observed value | Assessment |
|---|---:|---:|---|
| Magic | `0x00` | `Sony PS2 Memory Card Format ` | PROVEN |
| Version | `0x1c` | `1.2.0.0` | PROVEN on these cards |
| Data page length | `0x28` | `512` | PROVEN |
| Pages per cluster | `0x2a` | `2` | PROVEN |
| Pages per erase block | `0x2c` | `16` | PROVEN |
| Clusters per card | `0x30` | `8192` | PROVEN |
| Allocation offset | `0x34` | `41` | PROVEN |
| Allocation end (exclusive) | `0x38` | `8135` | PROVEN/consistent with chains |
| Root directory cluster | `0x3c` | `0` relative to allocation area | PROVEN by root bytes |
| Backup block 1 / 2 | `0x40` / `0x44` | `1023` / `1022` | PROVEN as stored; operational meaning not tested |
| IFC list first entry | `0x50` | `8` | PROVEN |
| Card type / flags | `0x150` / `0x151` | `0x02` / `0x2b` | PROVEN on these PCSX2 cards; flag semantics not fully decoded |

The 8,650,752-byte representation is exactly `16,384 * 528`. After removing
the 16-byte spare area from each physical page, the logical image is 8,388,608
bytes (`16,384 * 512`). The cards therefore contain 16,384 logical pages,
8,192 logical clusters, and 1,024 bytes per cluster. This agrees with the
independent parser's description that a standard 8 MiB image is 8 MiB of data
but 8,650,752 bytes when page spare areas are retained.

For data-bearing clusters, the byte offset is:

```text
logical_data_offset = (alloc_offset + relative_cluster) * 1024
```

The superblock, IFC, FAT, and backup areas are before the allocatable area.
Directory/file cluster fields observed in entries are relative to
`alloc_offset`, while FAT/IFC cluster references address logical clusters from
the start of the card. Treating the entry cluster as an absolute cluster is a
real and immediately visible parsing error: root save cluster `7` is at logical
cluster `48`, not cluster `7`.

## Superblock field validation

The first populated PCSX2 card contains the following raw little-endian
values at the implementation's existing offsets:

```text
0x28  00 02       512
0x2a  02 00       2
0x2c  00 10       16
0x30  00 20 00 00 8192
0x34  29 00 00 00 41
0x38  c7 1f 00 00 8135
0x3c  00 00 00 00 0
0x40  ff 03 00 00 1023
0x44  fe 03 00 00 1022
0x50  08 00 00 00 8
```

The same values occur on each formatted fixture. The all-`ff` negative fixtures
do not have a valid magic or usable geometry and must be rejected before any
FAT or directory read. The two backup-block values and unresolved marker bytes
remain metadata to preserve, not permission to treat an unformatted image as a
valid card.

## IFC and FAT chain

The allocation table is two-level indirect. For the standard cards,

```text
cluster_bytes       = page_len * pages_per_cluster = 512 * 2 = 1024
entries_per_cluster = cluster_bytes / 4 = 256
indirect_index      = relative_cluster / 256
indirect_slot       = indirect_index / 256
ifc_cluster         = IFC[indirect_slot]
fat_cluster         = u32(ifc_cluster, 4 * (indirect_index % 256))
fat_raw             = u32(fat_cluster, 4 * (relative_cluster % 256))
```

For these 8 MiB cards, only `IFC[0]` is needed. It is cluster `8`; the first
32 words in cluster 8 are `9, 10, 11, ..., 40`, so FAT clusters 9 through 40
hold entries for relative clusters 0 through 8191. This is the exact
double-indirection formula shown by the current PCSX2 implementation and the
independent parser: the IFC selects a FAT cluster group, then the FAT cluster
contains the individual entry.

FAT values are little-endian 32-bit words. The high bit marks allocation
(`0x80000000`). The low 31 bits hold the next relative cluster; `0x7fffffff`
means end-of-chain. `0xffffffff` is also observed as end-of-chain in ordinary
file chains (high bit set and low bits `0x7fffffff`), while a free/reserved
entry may be `0x7fffffff` without the allocated bit. Production code must keep
these states distinct when classifying free, allocated, reserved, and chain-end
entries.

### Worked fixture examples

The following examples use `~/.config/PCSX2/memcards/Mcd001.ps2`; offsets are in
the spare-stripped logical image.

| Relative cluster | IFC index/cluster | FAT cluster | FAT byte offset | Raw value | Next |
|---:|---:|---:|---:|---:|---:|
| `0` (root) | `0 / 8` | `9` | `9216` | `0x80000001` | `1` |
| `7` (save directory) | `0 / 8` | `9` | `9244` | `0x80000008` | `8` |
| `8` | `0 / 8` | `9` | `9248` | `0x80000037` | `55` |
| `55` | `0 / 8` | `9` | `9436` | `0xffffffff` | EOC |
| `445` (God of War directory) | `1 / 8` | `10` | `10996` | `0x800001bf` | `447` |
| `447` | `1 / 8` | `10` | `11004` | `0x80000211` | `529` |
| `529` | `2 / 8` | `11` | `11332` | `0x800002b2` | `690` |
| `690` | `2 / 8` | `11` | `11976` | `0x80000353` | `851` |
| `851` | `3 / 8` | `12` | `12620` | `0xffffffff` | EOC |

Thus the root directory chain is relative clusters `[0, 1, 124, 446]` and
the `BASCUS-97399GodOfWar` directory chain is `[445, 447, 529, 690, 851]`.
The non-contiguous transitions prove that a future implementation must follow
the FAT rather than assume contiguous directory storage.

The formula also explains the former “double-indirect” disagreement: the
second division is not a byte offset and not a data-cluster offset. It selects
the IFC-list slot from the number of FAT clusters already crossed. The
individual FAT cluster is then selected by `indirect_index % entries_per_cluster`.

## Root directory location and bounded walk

`rootdir_cluster = 0` is relative to `alloc_offset = 41`, so its data starts at
logical cluster 41 and byte offset `41 * 1024 = 41984`. The first root entry is
`.` and its `length` is the number of entries in the directory, not a byte
length. On the populated PCSX2 card it is `7`; the seven entries are:

```text
0  .                         mode 0x8427  length 7   cluster 0
1  ..                        mode 0xa426  length 0   cluster 0
2  BEDATA-SYSTEM             mode 0xa027  length 4   cluster 2
3  BASLUS-20827MANHUNT       mode 0x8427  length 6   cluster 7
4  BASLUS-20230              mode 0x8427  length 6   cluster 125
5  BASLUS-20502              mode 0x8427  length 6   cluster 335
6  BASCUS-97399GodOfWar      mode 0x8427  length 10  cluster 445
```

The directory's entry count is carried by its own directory entry in its
parent. The `.` entry inside a child directory has length zero on these real
cards; the parent entry supplies the child count. This is why using only the
child `.` length would incorrectly conclude that these directories are empty.
The independent parser's API makes the same operational choice: it reads the
parent directory entry's `length`, then reads exactly that many 512-byte
records.

The first child walk followed four real save directories. Examples:

* `BASLUS-20827MANHUNT` (`start=7`, six entries): `.`, `..`, `icon.sys`,
  `PS2VIEW.ICO`, a same-named zero-length file, and `MANHUNT0.SAV` (69,292
  bytes).
* `BASLUS-20230` (`start=125`, six entries): `.`, `..`, `icon.sys`,
  `PAYNE.ICO`, `invsave.inv` (143,640 bytes), and a same-named 100-byte file.
* `BASCUS-97399GodOfWar` (`start=445`, ten entries): `.`, `..`, its same-named
  68-byte file, five `data*.bin` files of 81,920 bytes each, `static.ico`, and
  `icon.sys`.

The Flatpak PCSX2 card gives an independent populated layout: root entries
include `BASLUS-21134SYS`, `BASLUS-20814MaxPay2`, and `BASLUS-20066SYSTEM`;
`BASLUS-20814MaxPay2` has a 17-entry directory and a fragmented chain
`[26, 27, 31, 107, 256, 259, 411, 712, 1013]`. The same formula and record
layout work without special casing the card or save name.

## Directory-entry layout

Each record is exactly 512 bytes. PCSX2's packed definition and the real bytes
agree on the fields relevant to safe enumeration:

| Offset | Size | Field | Interpretation |
|---:|---:|---|---|
| `0x00` | 4 | mode | low 16 bits carry type/permission flags; upper bits are retained |
| `0x04` | 4 | length | file byte length; directory entry count when the entry names a directory |
| `0x08` | 8 | created | unused byte, second, minute, hour, day, month, 16-bit year |
| `0x10` | 4 | cluster | start cluster, relative to `alloc_offset` for data/directory entries |
| `0x14` | 4 | dir entry | parent-directory entry number, meaningful for `.` in child directories |
| `0x18` | 8 | modified | same date-time shape as created |
| `0x20` | 4 | attr | attribute field; preserve, do not overinterpret |
| `0x24` | 28 | padding/reserved | preserve raw bytes |
| `0x40` | 32 | name | NUL-terminated name field on these fixtures |
| `0x60` | 416 | unused/reserved | preserve raw bytes; not an extended on-disk name field |

The record count per cluster is two (`1024 / 512`). An unused slot appears as
all zeroes in the sampled populated directories; an erased/free slot appears
as all `0xff`. PCSX2's source treats `mode == 0xffffffff` as invalid and the
independent parser only accepts entries with the `EXISTS` bit. A future parser
must not treat an arbitrary nonzero raw record as a valid name.

Observed mode examples:

* `0x8427`: used directory with read/write/execute and the directory bit.
* `0x8497`: used regular file with read/write/execute and the file bit.
* `0xa426`: used `..` entry with directory and an additional observed flag.
* `0xa027`: used system directory (`BEDATA-SYSTEM`) with directory bits and
  additional flags.

The proven minimum type test is the file bit `0x0010`, directory bit `0x0020`,
and used bit `0x8000` in the low 16 bits. Other bits must remain opaque until a
separate requirement needs them.

## Why the documented name lengths disagree

The apparent 32-byte versus 448-byte discrepancy is an abstraction mismatch,
not evidence of a variable-length on-disk record. The on-disk record is 512
bytes. Within it, the name occupies exactly 32 bytes at offset `0x40`; the
remaining 416 bytes are reserved/unused. PCSX2 exposes this as `name[0x20]`
followed by `unused[0x1a0]` in its packed `MemoryCardFileEntryData`, while an
older/tool-oriented in-memory representation may allocate a larger buffer for
path manipulation or preserve the rest of the record.

The fixture bytes make the distinction concrete. At logical byte offset
`43008 + 64 = 43072` (root cluster 42, entry 0), the bytes are:

```text
42 45 44 41 54 41 2d 53 59 53 54 45 4d 00 ...
BEDATA-SYSTEM\0
```

At offset `43520 + 64 = 43584` the 32-byte field contains
`BASLUS-20827MANHUNT\0` followed by zeroes; the rest of that 512-byte record
is not part of the name. The `McFsEntry` summary in the local Apollo audit and
the PCSX2 packed definition therefore agree on the on-disk field.

Names in all observed records are ASCII-compatible and NUL-terminated. That
does not prove that every legal PS2 filename is ASCII: future enumeration
should preserve raw name bytes and classify non-ASCII/invalid UTF-8 explicitly,
rather than use lossy conversion as identity.

## Cluster-chain validation

Directory and file start clusters are relative. Several chains were followed
using the FAT and checked for duplicate clusters, out-of-range values, and
premature termination.

For `BASCUS-97399GodOfWar`, the ten-entry directory occupies five clusters,
matching the two-records-per-cluster geometry. Its chain is
`445 → 447 → 529 → 690 → 851 → EOC`. The directory's 68-byte same-named file
starts at relative cluster 448 and the five 81,920-byte data files start at
449, 530, 691, 771, and 852. An 81,920-byte file needs exactly 80 clusters at
1,024 bytes, so this gives a direct size-to-chain bound to verify in a future
reader.

For `BASLUS-20814MaxPay2`, the 17-entry directory occupies nine clusters,
matching its chain length. Its six `savegame*.sav` files are each 153,600
bytes, requiring 150 clusters exactly. No sampled chain looped or exceeded the
exclusive allocation end.

Zero-length files use `cluster = 0xffffffff` in the real cards; this is a file
metadata convention and must not be followed as a FAT cluster. Directories
have a start cluster even where the child `.` record has length zero.

## Special entries, timestamps, and encoding

Every walked directory begins with `.` and `..`. They are ordinary 512-byte
records with directory mode bits and must be excluded from a user-save file
list. The `.` record in a child carries a parent entry index in the `dir_entry`
field (`2` for `BEDATA-SYSTEM` in the sample); `..` has a zero/unused parent
field. This is not a general POSIX inode model. No sampled record demonstrated
an independent deleted marker beyond unused/all-`ff` state, so deleted versus
free should remain conservative/unknown until additional source or fixtures
prove a distinction.

The timestamp bytes are not BCD in these fixtures. For example, a record has
the eight bytes `00 26 2c 0c 18 06 ea 07` at `0x08`, which decode as unused=0,
second=38, minute=44, hour=12, day=24, month=6, year=0x07ea=2026. PCSX2's
source definition uses native binary fields and converts through GMT+9, not a
BCD year or a Unix timestamp. The exact timezone presentation policy for a
future EmuWiz UI remains a separate decision; raw bytes should be retained.
Invalid zero dates should be reported as unknown, not converted to a real
calendar date.

All observed names are ASCII. The local Apollo audit identifies PS2 `icon.sys`
titles as Shift-JIS, but that is icon metadata, not proof that directory names
use Shift-JIS. A safe enumeration design should keep raw name bytes, accept
the proven NUL-terminated 32-byte field, and use an explicit non-lossy decoding
policy for bytes outside the observed ASCII subset.

## Corruption and fail-closed rules

A future reader should reject or mark the card/entry as structurally unsafe for
enumeration when any of the following occurs:

* magic, page length, pages-per-cluster, pages-per-block, cluster count, or
  raw/file length is inconsistent;
* a required IFC slot is absent, zero, erased, or outside the card;
* the calculated FAT cluster or FAT-entry offset is outside the logical image;
* a chain points below the allocated area, at/above `alloc_end`, to a free
  entry, or through a reserved marker;
* a chain loops, exceeds `alloc_end - alloc_offset` hops, or does not provide
  enough clusters for the declared file/record bytes;
* a directory count is greater than the available allocation bound or causes
  multiplication overflow;
* an entry has no NUL within the 32-byte name field, an unsafe raw name, or a
  contradictory file/directory mode;
* a non-empty file uses the empty-file marker `0xffffffff` as its start;
* physical data is truncated or a spare-bearing image has an incomplete page.

`NEEDS`/unresolved marker bytes and bad-block metadata must be retained as
evidence. They are not permission to guess missing FAT or directory semantics.
The format is not DOS FAT and must not be passed to a generic FAT library.

## Recommended production bounds

The geometry itself supplies hard limits for a standard card:

* maximum relative data-cluster hops: `alloc_end - alloc_offset` (`8,094` on
  these cards), with a visited-set check;
* maximum directory records: `alloc_end - alloc_offset`, additionally bounded
  by the directory's declared count and checked arithmetic;
* maximum directory depth: bounded by the same cluster/entry budget and a
  visited-directory set; a conservative implementation may use a lower
  product limit while preserving the format maximum;
* maximum name bytes: 32, including the terminator area;
* maximum file clusters: `ceil(file_size / cluster_size)`, never above the
  available data-cluster count;
* maximum total enumerated entries: the available data-cluster/2 record budget,
  with an implementation-level lower operational cap if needed.

These are format-derived limits, not “scan until EOF” behavior. A future parser
should also impose global byte/allocation accounting so multiple malformed
directories cannot cause repeated work.

## Multi-card comparison

The two populated PCSX2 cards and the two Flatpak PCSX2 cards demonstrate:

* identical standard geometry and the same IFC/FAT formula;
* empty cards whose root contains only `.` and `..` and terminates in one
  cluster;
* populated cards with different root counts, save names, file sizes, and
  fragmented chains;
* system directories and ordinary save directories in the same root;
* zero-length files using the `0xffffffff` start-cluster marker;
* names fitting the same 32-byte field;
* a negative all-`ff` image that must be classified as unformatted rather than
  parsed.

No fixture contradicted the relative-cluster interpretation, the two-level FAT
formula, 512-byte record size, 32-byte name field, or binary timestamp shape.
These are PCSX2 virtual images rather than raw physical-card captures, so this
does not prove every hardware-specific bad-block/ECC behavior.

## Production-readiness decision

* **A. FAT addressing proven enough for implementation? YES**, for the observed
  standard 8 MiB PCSX2/PS2 format, provided relative versus absolute cluster
  namespaces are kept explicit and all bounds/loop checks are fail-closed.
* **B. Directory-entry layout proven enough? YES**, for read-only metadata
  enumeration: 512-byte records, two per cluster, fields and 32-byte name
  location are cross-checked against source and multiple cards.
* **C. Name encoding proven enough? PARTIAL.** ASCII names and NUL termination
  are proven in fixtures; arbitrary non-ASCII legal-name handling is not.
* **D. Safe root-directory enumeration now implementable? YES**, as a bounded
  read-only operation after preserving the current structural preflight.
* **E. Safe nested save-directory enumeration now implementable? YES**, with
  parent-entry counts, FAT chains, directory/file type checks, and the same
  bounds. The child `.` length must not be used as the directory count.
* **F. Safe file extraction now implementable? NO for this phase.** Chain
  validation is evidenced, but extraction needs a separately reviewed
  read-only API, raw-name policy, ECC/error behavior, and source immutability
  tests. Enumeration should precede it.

## Smallest next implementation: PS2 Phase 2C

The smallest safe production slice is **read-only PS2 save inventory only**:

1. retain the existing geometry parser as the first gate;
2. strip spare bytes in memory for the proven representation;
3. parse IFC/FAT with the formula above and a visited-set/bounds budget;
4. enumerate root directories, classifying system directories separately from
   save directories;
5. enumerate one bounded level of child records, reporting raw name bytes,
   decoded name when safe, type, size, timestamps, start cluster, and chain
   health;
6. preserve corruption warnings and evidence provenance;
7. add no extraction, modification, deletion, undelete, repair, or restore
   behavior.

This phase should use synthetic corruption fixtures in addition to the real
cards, but must not write or mutate the real cards. Save Vault restore/apply,
launch integration, source roles, database schema, and GUI remain outside this
next slice.

## Final conclusion

Real PCSX2 bytes, PCSX2 source definitions, and an independent parser agree on
the disputed minimum: the allocation table is two-level indirect; directory
and file cluster numbers are relative to the allocation offset; records are
512 bytes with a 32-byte name at offset `0x40`; directory counts come from the
parent's directory entry; and chains can be validated with the standard
allocated/high-bit and end-of-chain markers. The evidence is sufficient to
begin a narrowly bounded read-only inventory implementation, but not to claim
safe extraction or arbitrary filename-encoding coverage.

