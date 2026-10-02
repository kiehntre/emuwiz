# Wii U WUD/WUX preservation foundation

Starting main: `3b166b523907514120639e51106118b414e3e0f6`.
Final validation base: `d34cb3393b58a31114e45319f53cccf5479f4f20` (independent
main promotions during this task; the backend candidate is rebased onto them).
Backend only; this document supersedes the earlier preview-only Wii U design.

WUD → WUX creation extension starts at main
`fc8bda7be687e626d708a4633f73840bcbf15c32`. It extends the same native
planner/executor and journaled publication path; there is no second converter.

## Architecture inventory and reuse

Current main already exports `wiiu_disc` and `wiiu_conversion`. This change
extends those modules, retaining the existing request, inspection and preview
vocabulary. `game_identity`/platform registration already distinguish Wii U;
GameCube/Wii boot evidence uses `nod` and is a separate format lane. The
existing general conversion planner, optical fingerprint/layout contract and
CUE/BIN → CHD executor provide the safety pattern, not a Wii U decoder.

The native executor reuses `safe_read::open_bounded_read`, fixed-buffer
`dat::rename_apply::identity::capture_identity`, existing SHA-256 dependency and standard formatting,
`tempfile`, Repair proposals/plans/transactions, atomic `rename_noreplace`,
journaling, re-verification and rollback/recovery. No new dependencies,
conversion framework, migrations, authority changes or process adapter.
The existing external-tool inventory types remain compatible; their probe
entry point returns an empty inventory without executing anything. Native
conversion needs no tool. Cancellation uses the existing `AtomicBool` pattern;
a callback reports bytes and completed logical blocks.

## WUD evidence

Format facts were checked against WUDD commit
[`04872a3e83293cab668c64d67a4f396ae3b1c2f6`](https://github.com/wiiu-env/wudd/tree/04872a3e83293cab668c64d67a4f396ae3b1c2f6).
The manufacturer region is 64 KiB, followed by a 32 KiB disc-ID region and a
32 KiB contents region. Disc-ID magic is big-endian `0xCC549EB9` at `0x10000`;
major/minor bytes are at `0x10005`/`0x10006`, and a bounded printable footprint
starts at `0x10020`. Sources:
[disc-ID definition](https://github.com/wiiu-env/wudd/blob/04872a3e83293cab668c64d67a4f396ae3b1c2f6/source/WUD/header/WiiUDiscId.h),
[field reader](https://github.com/wiiu-env/wudd/blob/04872a3e83293cab668c64d67a4f396ae3b1c2f6/source/WUD/header/WiiUDiscId.cpp),
[header assembly](https://github.com/wiiu-env/wudd/blob/04872a3e83293cab668c64d67a4f396ae3b1c2f6/source/WUD/header/WiiUDiscHeader.cpp).

Inspection reads only the 128 KiB header and file metadata. Magic, not suffix,
proves likely WUD structure. It projects observed size/32 KiB geometry,
printable manufacturer WUP identifier, versions, footprint, and header SHA-256.
It does not infer a decrypted title ID, game name, or region from identifier
letters. Non-text fields remain unavailable.

A plaintext contents header at `0x18000` has magic `0xCCA6E67B`, block size at
`+4`, partition count at `+0x1C` and 128-byte table entries at `0x18800`.
The count is limited by the 30 KiB table region (240 entries). Each entry has
31 name bytes, a volume count at `+31`, and up to eight big-endian volume LBAs
at `+32`; checked block-size multiplication and extent checks validate them.
Only table facts are projected; volumes/filesystems are not opened. Without
plaintext magic, the region is **encrypted or opaque**, not proven encrypted.
[Contents header](https://github.com/wiiu-env/wudd/blob/04872a3e83293cab668c64d67a4f396ae3b1c2f6/source/WUD/content/WiiUDiscContentsHeader.cpp),
[partition entry reader](https://github.com/wiiu-env/wudd/blob/04872a3e83293cab668c64d67a4f396ae3b1c2f6/source/WUD/content/partitions/WiiUPartition.cpp).

WUD has no container-level declared byte length. WUDD's retail dump extent is
`0x5D3A00000` bytes. `retail_size_matches` and a non-retail-size advisory keep
short, whole-sector synthetic images distinct from full retail dumps. Header
truncation, extents below 128 KiB, partial optical sectors and extents above
64 GiB fail closed. An aligned shortened opaque dump cannot be authenticated
as complete without external identity evidence; structural validity does not
claim historical dump completeness. The “absurd declared size” test belongs
to WUX; raw WUD tests instead check an absurd observed sparse-file extent.
[WUDD extent](https://github.com/wiiu-env/wudd/blob/04872a3e83293cab668c64d67a4f396ae3b1c2f6/source/WUDDumperState.h).

## WUX structure and limits

[Cemu's independent reader](https://github.com/cemu-project/Cemu/blob/main/src/Cafe/Filesystem/WUD/wud.cpp)
and [the original tool](https://github.com/cemu-project/WudCompress) establish
sector-index reconstruction. The 32-byte padded header uses little endian:
`WUX0` at 0, second magic `0x1099D02E` at 4, block size at 8, logical WUD size
at 16, flags at 24. Table length is `ceil(logical_size / block_size)` 32-bit
indices starting at byte 32. Payload starts at that table's end rounded up to
a block boundary. Index `i` names `payload_start + i * block_size`.

Repeated indices are valid deduplication, including stored zero-filled
sectors. **There is no sparse/zero sentinel.** Index 0 names stored sector 0;
`UINT32_MAX` cannot be interpreted as a hole. Fixed-size sector slots cannot
partially overlap: exact aliases are valid repeats. Reference count and repeat
count are projected. Every entry is checked before reading payload. WUD header
facts are projected through the same logical mapper used by conversion.

Limits: block size `0x100..0x10000000` (upper endpoint excluded), flags 0,
whole 32 KiB logical WUD sectors, 128 KiB–64 GiB logical size, 16 MiB maximum
table (4,194,304 entries). A retail 32 KiB-block dump needs about 3 MiB of table.
The reference bitmap is at most 4 MiB, header 128 KiB, table read buffer 8 KiB,
and decode scratch 64 KiB. These limits are checked **before allocation**;
working memory is at most roughly 21 MiB, independent of payload size.
Unknown flags, truncated tables/payload, invalid references, extra payload
slots beyond logical block count, partial physical slots and overflow refuse.
A final logical WUX block may consume only a prefix of its full physical slot.
No complete image is read into a Vec or `read_to_end`.
[WUDD's interoperable writer](https://github.com/wiiu-env/wudd/blob/04872a3e83293cab668c64d67a4f396ae3b1c2f6/source/fs/WUXFileWriter.cpp)
corroborates header padding, flags, alignment and stored-sector deduplication.

## Canonical WUD → WUX writer

Writing rules were checked before implementation against the original
[WudCompress v1 format notes and writer](https://github.com/cemu-project/WudCompress/blob/b0ab5f6b6a46a1972dbbd802b6d04d46de003aab/WudCompress/main.cpp),
its [header definition](https://github.com/cemu-project/WudCompress/blob/b0ab5f6b6a46a1972dbbd802b6d04d46de003aab/WudCompress/wud.h),
the pinned WUDD writer above, and
[Cemu's independent reader](https://github.com/cemu-project/Cemu/blob/e20bfd00ecfc4376e39048942c15a55463f065d0/src/Cafe/Filesystem/WUD/wud.cpp).
Only format facts were used; no upstream implementation was copied into EmuWiz.

The original format comment lists member types without ABI padding. The actual
header definition and interoperable readers/writer establish this 32-byte
layout; the table does **not** start after a packed 24-byte header.

| Offset | Width | Canonical bytes / meaning |
| --- | --- | --- |
| `0x00` | 4 | ASCII `WUX0` |
| `0x04` | 4 | little-endian `u32` `0x1099D02E` |
| `0x08` | 4 | little-endian `u32` block size `0x8000` |
| `0x0C` | 4 | zero ABI padding |
| `0x10` | 8 | little-endian `u64` exact source WUD byte size |
| `0x18` | 4 | little-endian `u32` flags 0 |
| `0x1C` | 4 | zero trailing header padding |
| `0x20` | `4 * logical_blocks` | little-endian `u32` physical block indices in logical order |
| table end | to next `0x8000` boundary | zero padding |
| aligned payload start | `stored_blocks * 0x8000` | consecutive complete physical blocks |

The source must meet the existing complete-header and whole-32-KiB-sector WUD
policy (128 KiB–64 GiB). WUX readers support a final partial logical block for
some other block sizes; this writer accepts only `0x8000`, so partial WUD
sectors refuse. No block-size tuning option is exposed. Raw WUD has no declared
extent, so an aligned shortened opaque dump still cannot be authenticated as
a historically complete disc.

WUX permits different valid physical encodings. EmuWiz chooses one reproducible
policy: walk logical blocks sequentially; store every non-zero block in encounter
order, store the first all-zero block at its ordinary next physical index, and
reference that index for later exactly zero blocks. There is no implicit hole,
special index, omitted first zero block, generic compression or special treatment
of `0xFF`/other repeated bytes. Non-zero duplicates remain separately stored.
Upstream writers deduplicate more broadly; neither their map order nor their
hash shortcut is part of this policy. It need not produce their same bytes.
The original comment calls the sector array unique, but its stated array size
uses logical sector count; repeated-index writer behaviour and reader addressing
prove the actual payload size is determined by physically stored slots. These
editorial ambiguities do not require private flags or change lookup semantics.

`WiiUWuxCreationLayout` proves every size/offset with checked arithmetic before
allocation. Worst-case WUX size is aligned payload start plus source size. At
64 GiB the lookup has 2,097,152 entries (8 MiB), below the existing 16 MiB reader
limit. Encoding keeps that bounded `u32` table, one 32 KiB input buffer, a fixed
zero buffer and fixed hashing/I/O state. No payload or per-block hash map is
retained. The creation table is dropped before re-inspection/reconstruction
allocates a reader table. There is no image-sized allocation.

All header/table/alignment padding is explicitly zero. No timestamp, transaction
ID, path or machine data enters WUX bytes. Identical source bytes produce
byte-identical WUX; random staging and journal IDs are outside the container.

## Preview, apply and verification

`plan_wiiu_conversion` performs zero writes. It exposes source/destination,
formats, physical/expected logical bytes, staged space, no-clobber status,
header evidence, block count and verification availability. WUD → WUX additionally
projects fixed block size, exact logical size, lookup size/alignment and maximum
output bytes. It does not scan all payload blocks to guess the final sharing
ratio. Creation reserves worst-case output space; decoding reserves exact WUD
space. One output extent
is required on the destination filesystem; atomic rename needs no second copy.
Free space excludes the source, which already exists. Metadata/journal space
is not estimated; any ENOSPC during writes fails safely. Optional caller
`HashAvailable` means **physical source SHA-256**, never original-disc identity.
Missing external hash does not block native structural verification.

The plan privately binds its source/destination and public inspection to file
size, precise modification time, Unix device/inode/ctime where available and
SHA-256 of bounded header/table evidence. Apply revalidates all of it before
creating a stage. Changed evidence refuses as stale; destination changes and
current free space are rechecked. Full source SHA-256 is then captured before
and after decode, using the existing descriptor-stability checks. Preview does
not hash a tens-of-gigabytes file. On non-Unix, preview has no inode/ctime proof;
full apply hashes still detect content changes during execution.

Apply opens the source read-only and reconstructs canonical logical order in
64 KiB chunks, resolving/validating every physical range and rejecting EOF.
All zero bytes are written explicitly. It syncs the staged output, verifies
exact declared size, inspects it as WUD, compares complete logical header
identity (including opaque bytes), and compares an independent output SHA-256
with the hash accumulated over reconstruction. WUX contains no payload checksum:
this proves faithful reconstruction, not authenticity against a historical
original or DAT. Source bytes are never rewritten, renamed or deleted.

For WUD → WUX, the same executor captures a stable full source SHA-256, streams
the source read-only into a private stage, and compares its streaming source
digest with that captured proof. It backfills the little-endian table, syncs,
re-inspects WUX, and compares exact logical size and complete WUD header evidence.
Stored/repeated block counts must match the encoder's receipt. It then reconstructs
**every logical byte** through the existing `read_wux_at` in 64 KiB buffers directly
into SHA-256. Equality with the complete original WUD digest is mandatory;
structural parsing alone cannot authorize publication. No second physical WUD
is materialised. Staged metadata must remain unchanged across reconstruction
and the stable full physical WUX hash capture. Source identity/hash/evidence are
rechecked before publication. The journal records physical WUX hash separately
from reconstructed/source WUD hash, exact logical/physical sizes, the writer
policy and stored/zero/reused-zero block counts. No verification-skip option exists.

Verified output enters a single existing journaled Repair `MovePath`.
Linux `renameat2(RENAME_NOREPLACE)` publishes atomically; existing destinations
and platforms without the verified primitive refuse. Success requires the
transaction to confirm one applied output and matching destination identity.
The journal's extension map records policy, source/container hash, reconstructed
WUD hash, output hash, header hash, exact size and source retention.

Encode/decode/verification failures and cancellation remove the temporary directory.
Once publication is handed to Repair, its stage directory is retained for
journal rollback/recovery. Unconfirmed publication attempts the existing
identity-checked rollback; an I/O failure that also prevents rollback is
reported with its recovery directory and never as success. A process
crash before handoff can leave a hidden `.emuwiz-wiiu-*` stage for manual cleanup;
it cannot expose a partial final destination. Cancellation is checked each
32 KiB encode block, 64 KiB reconstruction chunk, bounded table batch and phase
boundary; existing full-hash/Repair phases do not
provide chunk-level cancellation. No unattended conversion/resume is added.

## Unsupported cases and NKit boundary

WUD → WUX now uses the canonical native representation above. Arbitrary writer
block sizes, non-zero deduplication tuning, non-zero flags and partial optical
sectors are unsupported. The reader retains its wider validated block-size
support; writing does not infer rules from permissive parsing alone.
Split media remains bounded diagnostic inventory (64 part indices/4,096 directory
entries), never silently assembled or marked structurally complete. WUA,
extraction, key access and filesystem/title authentication remain unsupported.

NKit is a separate future reconstruction/recovery project. Legacy GameCube/Wii
NKit may rewrite filesystem offsets, omit update partitions, regenerate junk,
and rebuild encryption/hashes; safe restoration needs the correct recovery
artifacts, original identity and provenance for every reconstruction decision.
That differs from WUX's self-contained stored-sector mapping.
[Format/recovery documentation](https://wiki.gbatemp.net/wiki/NKit/NKitFormat).
Newer NKit tooling reading WUX does not make legacy NKit semantics equivalent.
No NKit converter, embedded keys, key downloader or emulator dependency exists.

## Future GUI projection and validation

Existing inspection and plan objects retain their API shape with additional
header/source evidence, repeat counts, retail-size flag, work estimate and
verification availability. The request shape is unchanged. Creation adds typed
geometry/maximum-size and receipt block-count projections; progress counts
logical bytes processed rather than physical WUX bytes written. Future GUI work can show opaque metadata and
non-retail warnings, request explicit apply, connect progress/cancellation and
surface the existing Repair transaction receipt/recovery path. No GUI edits
or automatic real-image conversion are part of this foundation.

Synthetic tests cover valid and malformed WUD/WUX, content/suffix mismatch,
plaintext/opaque table bounds, aliases/zero sectors, arithmetic/allocation limits,
byte-exact output, stale evidence, source preservation, collision refusal,
cancellation, premature EOF, staged corruption and journal provenance. A 64 MiB
logical fixture verifies 1,024 bounded decode chunks and every trailing zero;
a sparse retail-size raw fixture validates bounded inspection of large extents.

The creation extension adds 21 synthetic tests (49 total Wii U foundation
tests). They cover an independently specified tiny canonical byte vector,
zero/mixed/non-zero/near-zero blocks, cross-block reads, exact size and full
SHA-256 round trips through the existing decoder, two-run determinism, partial
sector refusal, overflow/allocation/alignment limits, stale source bindings,
alias/symlink/special-file refusals, destination races, cancellation and
premature EOF, malformed staged output and payload corruption that still parses,
source immutability, journal evidence and existing rollback. The streaming test
normally uses a 128 MiB sparse synthetic WUD; `EMUWIZ_WUD_WUX_PERF_BYTES`
selects a larger **test fixture**, not a production conversion option.
