# Wii U WUD/WUX preservation foundation

Starting main: `3b166b523907514120639e51106118b414e3e0f6`.
Final validation base: `d34cb3393b58a31114e45319f53cccf5479f4f20` (independent
main promotions during this task; the backend candidate is rebased onto them).
Backend only; this document supersedes the earlier preview-only Wii U design.

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

## Preview, apply and verification

`plan_wiiu_conversion` performs zero writes. It exposes source/destination,
formats, physical/expected logical bytes, staged space, no-clobber status,
header evidence, block count and verification availability. One output extent
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

Verified output enters a single existing journaled Repair `MovePath`.
Linux `renameat2(RENAME_NOREPLACE)` publishes atomically; existing destinations
and platforms without the verified primitive refuse. Success requires the
transaction to confirm one applied output and matching destination identity.
The journal's extension map records policy, source/container hash, reconstructed
WUD hash, output hash, header hash, exact size and source retention.

Decode/verification failures and cancellation remove the temporary directory.
Once publication is handed to Repair, its stage directory is retained for
journal rollback/recovery. Unconfirmed publication attempts the existing
identity-checked rollback; an I/O failure that also prevents rollback is
reported with its recovery directory and never as success. A process
crash before handoff can leave a hidden `.emuwiz-wiiu-*` stage for manual cleanup;
it cannot expose a partial final destination. Cancellation is checked each
64 KiB chunk and at phase boundaries; existing full-hash/Repair phases do not
provide chunk-level cancellation. No unattended conversion/resume is added.

## Unsupported cases and NKit boundary

WUD → WUX is **DEFERRED**. Upstream writers document deduplication, but this
foundation does not establish a tested deterministic encoder policy and
independent writer interoperability corpus. The planner refuses that direction
even if supplied an external-tool capability; no pseudo-WUX writer is added.
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
verification availability. Future GUI work can show opaque metadata and
non-retail warnings, request explicit apply, connect progress/cancellation and
surface the existing Repair transaction receipt/recovery path. No GUI edits
or automatic real-image conversion are part of this foundation.

Synthetic tests cover valid and malformed WUD/WUX, content/suffix mismatch,
plaintext/opaque table bounds, aliases/zero sectors, arithmetic/allocation limits,
byte-exact output, stale evidence, source preservation, collision refusal,
cancellation, premature EOF, staged corruption and journal provenance. A 64 MiB
logical fixture verifies 1,024 bounded decode chunks and every trailing zero;
a sparse retail-size raw fixture validates bounded inspection of large extents.
