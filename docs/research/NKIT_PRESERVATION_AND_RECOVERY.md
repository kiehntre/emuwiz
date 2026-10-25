# NKit preservation and recovery foundation

Research date: 2026-10-02. Starting EmuWiz main and origin/main:
`a0b1e6f661a9e4fc6d15e6d10036472df3a13047`, also confirmed against the live
remote. Branch: `feature/nkit-preservation-foundation`.

Main subsequently advanced through `b7527507` and `84792108` (WUD/WUX and
durable queue work). Neither touched this foundation's files. The candidate
was rebased onto `84792108c940c937098b7e567d5e46de466c841a` for final validation;
main itself was not modified by this task.

## Decision

Ship a bounded, read-only **legacy NKit v1 raw-header inspector** and an
explicitly blocked ISO recovery readiness preview. **Defer reconstruction,
executable conversion planning, publication and queue integration.** Neither
emulator compatibility, a recognizable header, nor CRC agreement proves an
original disc was reconstructed. No external program is launched.

The implementation is an independently written field reader under
`gamecube_wii_boot_evidence::nkit`; it copies no upstream implementation and
adds no dependency. It uses existing `safe_read`, `identity_source::hashing::Crc32`
and SHA-256. The current identity and optical planner integration points are
owned by other dirty worktrees, so that integration is deferred. This does not
change how the existing general identity API classifies `.iso` files.

## Sources and version boundary

The author’s [format notes][format] and [user guide][guide] are useful, but the
decisions below were checked against **NKitv1 commit
`31ac76828e6533ba3981eeda79ebfba2f16102af`**:

| Source | Evidence checked |
|---|---|
| [NStream][nstream] | Raw/GCZ routing, disc magic, platform-specific size units |
| [GameCube reader][gc-reader], [writer][gc-writer] | FST address restoration, gap reconstruction, original size/CRC |
| [Wii reader][wii-reader], [writer][wii-writer] | Removed updates, hash exceptions, re-encryption, missing-resource behavior |
| [Gaps][gaps], [JunkStream][junk], [WiiHashStore][hashstore] | Gap kinds, generated padding, preserved hash data |
| [Converter][converter], [Coordinator][coordinator], [VerifyWriter][verify] | Conversion versus recovery, verification result propagation |
| [RecoveryData][recovery], [FileItems][fileitems], [NDisc][ndisc] | Resource lookup, recovery file shapes, extraction |
| [DatData][dat], [GC recovery reader][gc-recover], [Wii recovery reader][wii-recover] | DAT matching and repair-specific behavior |
| [GCZ writer][gcz] | Block wrapper, separate physical CRC forcing |

The differently named [current NKit repository][nkit-current] includes newer
work. Legacy `NKIT v01` inside a GC/Wii disc is **not** the NKit 2 metadata
header handled by EmuWiz’s installed `nod` 1.4.4 in WBFS/CISO/WIA, and is not
NKDS. Do not route by the substring “NKit” or copy a newer header’s size rules.
`nod` provides general GC/Wii/container readers, not a proven legacy v1
reconstruction API. `.nkit.iso` is a transformed raw disc representation;
`.nkit.gcz` adds GCZ block compression around that representation. The outer
extension does not establish the inner platform or variant.

## Verified header layout and conservative interpretation

All four-byte integers in this legacy NKit header are big endian. Offsets are
in the **decoded raw NKit stream**, not the GCZ file. Reader source, rather
than the format wiki’s abbreviated length description, establishes units.

| Offset | Bytes | Meaning |
|---:|---:|---|
| `0x000` | 6 | Game ID including maker code; retained raw |
| `0x006` | 1 | Disc number |
| `0x007` | 1 | Revision |
| `0x018` | 4 | Wii magic `5D1C9EA3` |
| `0x01C` | 4 | GameCube magic `C2339F3D` |
| `0x020` | 64 | Title bytes, no assumed UTF-8 encoding |
| `0x060`, `0x061` | 1 each | Wii hash/encryption omission flags; v1 writer sets both to 1 |
| `0x200` | 4 | Exact `NKIT` marker |
| `0x204` | 4 | Exact ` v01` version |
| `0x208` | 4 | Claimed original/source CRC32 |
| `0x20C` | 4 | CRC-forcing patch, not an authenticity signature |
| `0x210` | 4 | GC original byte count; Wii original count **multiplied by four** |
| `0x214` | 4 | Forced junk-generation ID, normally zero; retained raw |
| `0x218` | 4 | Removed Wii partition/filler-span CRC; zero means no removal declared |
| `0x458` (GC) | 4 | BI2 region word, at `0x440 + 0x18` |
| `0x4E000` (Wii) | 4 | Disc region word |

Sources: [NStream][nstream], both [GC][gc-reader]/[Wii][wii-reader] readers,
[Wii writer][wii-writer], and region extraction in [NDisc][ndisc]. The installed
[`nod` DiscHeader][nod-header] provides the independent 64-byte title boundary.
NStream’s convenience title getter requests up to 96 bytes through the Wii
flag region; its recovery extractor saves 64. EmuWiz uses the conservative
64-byte title and never interprets Wii flags as title text. No edit or
reconstruction decision depends on text decoding.

The v1 writers overwrite these NKit words and readers zero them during
restoration. That contract must be proven for each supported input class;
arbitrary nonzero original reserved bytes are not automatically recoverable.
The GC reader accepts one exact version; NStream also contains a permissive
“missing platform magic means GC” fallback. EmuWiz rejects missing or conflicting
magic instead. Unknown versions and nonstandard Wii flags are unsupported.

EmuWiz’s inspection policy accepts claimed logical lengths between `0x2440`
and `0x57058000` for GC and between `0x50000` and `0x1FB4E0000` for Wii.
These are conservative supported bounds, not an assertion that every length
within them describes a valid disc. Below-minimum lengths are malformed;
larger geometry is unsupported. Wii multiplication widens to `u64` first.
Stored images must contain at least the platform’s fixed preamble. The
inspector does **not** require stored size to equal original size, nor assume
that compression always makes a representation smaller.

## GameCube and Wii reconstruction are different

GameCube conversion compacts the filesystem and records gap descriptions.
Restoration puts files back at their original positions and repairs FST/DOL
addresses. Supported junk can be regenerated from the game/junk ID, disc
number and position. Uniform scrub bytes and nonuniform preserved bytes have
different representations; “padding” is not permission to replace all gaps
with zeros. Unrecognized discarded data cannot be deduced from a CRC.
[GC reader][gc-reader], [GC writer][gc-writer], [gap model][gaps],
[junk generator][junk].

Wii has an outer partition table and independently structured partitions.
The v1 representation removes encryption and reproducible hash sectors,
retains exception data when regeneration would differ, compacts partition
filesystems, and restores encrypted output during decoding. An original
`0x8000` sector contains `0x400` hash bytes and `0x7C00` data bytes. Partition
data length and original length are distinct; partition-header `0x2BC` and
the compacted partition’s `0x210` participate in restoration. Update and
channel partitions are disc content, not disposable cache. No keys are read,
embedded or requested by this inspector. Future encryption support needs its
own explicit key/resource policy. [Wii reader][wii-reader],
[Wii writer][wii-writer], [hash storage][hashstore].

The writer can remove an update partition, save its recovery object, retain
the original partition table in filler metadata, and record the object’s CRC
at `0x218`. The reader can continue when the object is absent: it marks the
result recoverable, inserts zero filler and reports CRC results. That is
not successful original restoration. EmuWiz must fail closed for the requested
preservation claim, irrespective of upstream process success or playability.
[Wii reader][wii-reader], [writer][wii-writer], [Coordinator][coordinator].

## Direction classifications

These are **conditional format capabilities**, not promises made by the
header inspector. “Original” must name its witness: the precise source ISO
fed into Convert, or a separately identified known disc sought by Recover.

| Direction or evidence | Classification and boundary |
|---|---|
| GC ISO → NKit | `BYTE_PERFECT_REVERSIBLE` only for supported inputs with a measured, byte-identical inverse; otherwise `UNKNOWN`. A scrubbed input can be preserved exactly without becoming an original dump. |
| GC NKit → ISO | Potentially `BYTE_PERFECT_REVERSIBLE` to the encoded source; header-only evidence is `UNKNOWN`. Repair toward a known original may be `RECOVERABLE_WITH_EXTERNAL_DATABASE` and actual recovery objects. |
| Wii ISO → NKit | Potentially `BYTE_PERFECT_REVERSIBLE` as the **image plus every required recovery object**. If update removal is enabled, the single output alone is insufficient. Unsupported corruption or unpreserved bytes prevent that claim. |
| Wii NKit → ISO | Potentially `BYTE_PERFECT_REVERSIBLE` only with all dependencies, correct hash/encryption reconstruction and full witness comparison. Missing declared update data gives `DependenciesMissing`, not success. |
| NKit → RVZ | Rewrapping the transformed stream does not reconstruct original disc bytes. For preservation, first reconstruct/verify ISO, then use a lossless RVZ route and verify its decoded stream. Direct original-restoration claim is `UNSUPPORTED` here. |
| NKit → WBFS | GC is `UNSUPPORTED`; ordinary Wii WBFS may discard sectors and is `LOSSY` relative to a full original. A known reconstructible scrubbed result may be `STRUCTURALLY_RECOVERABLE` or require external resources; generic byte-perfect guarantees are refused. |
| NKit without external recovery data | Not universally lossy: an intact supported GC source conversion, or Wii without removed content, can be self-contained. The inspector reports `UNKNOWN`. A nonzero update marker explicitly identifies a missing dependency. |
| NKit with recovery data | Resource presence alone is still `UNKNOWN`; a mismatch refuses. `RECOVERABLE_WITH_EXTERNAL_DATABASE` means conditional potential, not that a DAT contains missing bytes. Only measured output equality earns the stronger state. |
| Modified/scrubbed source, unverifiable reconstruction | At most `STRUCTURALLY_RECOVERABLE` after actual structural validation; `UNKNOWN` beforehand. If required original bytes were irretrievably discarded, `LOSSY` relative to that target. |

The author distinguishes Convert (restore its source) from Recover (attempt
repair toward known disc data). [User guide][guide], [conversion routing][converter],
[GC recovery][gc-recover], [Wii recovery][wii-recover]. RVZ’s own preservation
mechanisms include hash exceptions and junk packing; those mechanisms cannot
retroactively supply missing input content. [Dolphin RVZ specification][rvz].

## Recovery resources, DATs and provenance

There is no single magic “Redump recovery database” that supplies all bytes.
The legacy implementation uses several distinct inputs:

| Resource | Verified representation and purpose |
|---|---|
| DAT | XML `rom` records with name, CRC, MD5 and SHA1. Describes candidate known outputs; supplies no partition or file contents. |
| GC apploader | Raw `appldr[...]...[CRC].bin` data, shared across some discs. |
| GC FST recovery | `fst[id8][apploaderCRC][fstCRC][postFstCRC].bin`; `0x50`-byte prefix contains DOL/FST offsets, maximum FST size, region and 64-byte title, followed by FST data. This is not just raw fst.bin. |
| Wii update recovery | Extracted partition/filler object named with a 40-character identifier, label and span CRC. Trailing reconstructible filler can be omitted from the file; the name/header CRC need not equal the stored file's CRC. |
| Wii channel recovery | Separate named extracted objects with identity/index/type/CRC components; not interchangeable with update objects. |
| Configuration | Explicit recovery paths, known CRC lists, junk ID substitutions and disc-specific junk patch rules. These affect the result and belong in provenance. |

Sources: [DAT loading][dat], [recovery lookup][recovery], [resource layout][fileitems],
[extraction][ndisc], [settings][settings]. The variable `WiiUPartsData` in v1
means Wii **update** partitions, not Wii U. The upstream update lookup uses
filename CRCs. `WriteRecoveryPartitionFiller` hashes the entire filler span
but can leave trailing generated blocks out of the saved file. This foundation
keeps the declared span CRC and measured file hashes in separate domains;
it does not assert that a candidate has valid structure or reproduces that
span. A future validator must recover the original span/layout and validate
the reconstructed bytes, not compare a trimmed file CRC to `0x218`.

The source licence is [MIT][license]; that is permission for the software,
not a demonstrated redistribution grant for Nintendo partition/apploader
content or a separate DAT collection. No applicable blanket redistribution
permission for recovery packs was established. EmuWiz therefore bundles and
downloads **none** of those resources. User-supplied objects can be inspected
read-only; future use must record explicit selection, file identity, full
digest, resource kind, matching evidence and the DAT’s origin/version/digest.
Distribution clearance remains a separate gate if bundling is ever proposed.
No recovery pack or copyrighted disc image was fetched for this task.

## Verification: CRC is necessary evidence, not proof

The raw writer deliberately changes `0x20C` until the physical NKit CRC equals
the source CRC. The GCZ writer additionally forces its physical CRC with a
word at `0x04`. These files can have the source’s CRC while containing entirely
different representations. Do not send that CRC to identity/DAT logic as
proof of original raw-image equality. [Wii writer][wii-writer], [GCZ writer][gcz].

The upstream pipeline distinguishes validation and verification CRCs and
optional output hashes. Its DAT lookup initially selects by CRC; the inspected
`GetRedumpEntry` also assigns the MD5 value into the returned SHA1 field, so
EmuWiz must not treat an upstream result label as independent strong evidence.
Compute the actual full output hashes and compare to the intended record.
[Coordinator][coordinator], [VerifyWriter][verify], [DatData][dat].

Future receipts must separate `SourceEquivalentVerified` (full reconstructed
bytes match the retained source witness) from `KnownTargetVerified` (full
size/hash and structural checks match an explicitly selected trusted disc
record). Record CRC, available DAT SHA1/MD5 and measured SHA-256 without
promoting a checksum recorded by an untrusted input into an independent
witness. Emulator boot success is neither state.

## Implemented inspector/readiness contract

`inspect(path)` reads 4 magic bytes, a fixed `0x440` prefix, then 4 region
bytes: **1,096 bytes maximum**, independent of image length. `safe_read` refuses
relative/non-normal paths, symlink leaves/ancestors and special files. Errors
distinguish `NotNkit`, `Truncated`, `Malformed`, `Unsupported` and unsafe/I/O
failures. GCZ is recognized only as a wrapper and refused; an ordinary GCZ is
not mislabeled NKit. Raw title/ID/region values remain evidence, not inferred
canonical identity.

`NkitHeaderObservation` reports platform, stored/claimed original size, raw
ID/title, disc/revision, region, source CRC claim, CRC patch, junk ID and update
requirement. Its SHA-256 covers **only the prefix**. It is explicitly not a
full source hash or stale-plan binding. The partition table, FST, gaps,
partition hashes, encrypted output, and remainder of the image are not
validated. Truncation outside inspected ranges cannot be diagnosed here.
Even a synthetic empty body with a plausible header remains `Unknown`, never
“valid complete disc” or “recoverable”.

`preview_iso_recovery(source, optional_update_candidate)` performs no writes.
No marker means `NotDeclared`, not “no dependencies needed”. A declared
resource absent from the request means `DependenciesMissing`. Candidate
metadata declares its recovery-span CRC; disagreement with the source's
requirement refuses before resource I/O. This declaration is an untrusted
lookup hint, not proof. An explicitly selected candidate with agreeing
metadata is streamed once through the existing CRC32 and SHA-256
implementations using 64 KiB of memory, capped at the Wii dual-layer byte
limit; empty, unsafe or unreadable resources refuse. Length/mtime changes
during that read refuse. File hashes remain separate from the expected
reconstructed-span CRC. Both matching and differing file CRCs leave `Unknown`
with a span/structure/compatibility blocker. Supplying update
data to a source without that requirement refuses. No directory scan, filename
trust, DAT parsing, downloads or binary recovery decoding takes place.

All successful previews still contain reconstruction, full-body/source-binding
and trusted-output-witness blockers. There is **no apply method**, destination
reservation, no-clobber promise, persistent executable plan, or queue job.
Callers may display the evidence; they cannot use it as authorization to
publish. A new preview re-reads its inputs; this foundation makes no claim
that an old preview can survive source replacement.

## Existing architecture and deferred conversion design

Read-only inspection of current main found:

- `game_identity.rs`: bounded ISO/GCM disc identity, RVZ uncompressed-header
  projection, CISO stored-header projection and single-disc WBFS mapping.
  This is title/structure evidence, not a legacy NKit decoder.
- `gamecube_wii_boot_evidence.rs`: existing `nod`-backed disc observations.
  The new nested module owns the narrow legacy evidence API without changing
  the old collector’s semantics or `lib.rs`.
- `logical_media.rs`: existing bounded random-access abstraction. A future
  decoded stream can adapt here; this task needs only `safe_read`.
- `repair/optical_conversion.rs`: reviewed CUE/BIN→CHD, source evidence,
  staging, verification and journaled Repair publication/rollback.
- `wiiu_conversion.rs`: an architectural reference for destination-adjacent
  staging, output reinspection/full hashing and Repair no-clobber publication.
  WUD/WUX format semantics are not reused for GC/Wii reconstruction.
- `dat/rename_apply/identity.rs`: shared full-content `ObjectIdentity` capture
  and revalidation, including identity, size, mtime and SHA-256.
- `conversion_queue/durable.rs`: typed reviewed converters, persisted original
  plans, restart/Interrupted handling, retry from byte zero, and Repair journals.
  At the starting SHA it supported WUX→WUD; the intervening main commits add
  WUD→WUX. No preview-only NKit job is added.

If a decoder later passes the gate, the executable plan must include source
and destination, exact variant/platform, target format, independently selected
output witness, claimed and verified sizes, explicit dependency inventory,
strong source/resource identities, no-clobber/alias checks, supported geometry,
space estimate, verification level, warnings and blockers. Preview remains
read-only. Ambiguous resources or missing required bytes refuse; never pick a
DAT/recovery object merely because its filename or CRC appears plausible.

Apply must revalidate source/resources/target ancestry before staging, decode
through a fixed-size buffer into destination-adjacent owned staging, retain
bounded metadata or spill it to scratch, and verify the entire output before
publication. Check exact length, full-stream hashes, GC filesystem structure
or Wii table/partition/encryption/hash structure, game identity and a complete
resource-consumption receipt. Verify no unaccounted generated/discarded bytes.
Revalidate all inputs again; publish through existing Repair no-clobber
machinery and retain source files. Destination collision, source/destination
alias, unsafe paths, stale identity/hash, overflow, verification failure or
unconsumed required resources must leave the destination unpublished.

Only then extend the durable queue with a typed reviewed plan: serialize the
original evidence, recover abandoned running work as Interrupted, preserve
journal receipts, and retry from byte zero. Never resume an unverified partial
NKit stream or turn an informational header preview into a Ready job.

## Reconstruction gate assessment

| Gate | Finding |
|---|---|
| Sufficient semantics | Header understood; full adversarial decoder, gap/hash exception bounds and variant corpus not established. **Fail for apply.** |
| Available usable resources | No real recovery material acquired or licensed for bundling; user-supplied candidate hashing is not resource validation. **Fail for general apply.** |
| Bounded reconstruction | Technically plausible, but upstream preserves FST/patch/hash data in variable memory and has an explicit excessive-hash guard. No EmuWiz streaming decoder with proven metadata ceilings exists. **Not demonstrated.** |
| Strong output verification | Header has CRC only; no original strong witness or complete synthetic reconstruction vectors validated. **Fail.** |
| Safe publication | Existing Repair infrastructure is reusable, but a successful container transaction cannot compensate for the preceding failures. |

Thus the exact next blocker is a separately reviewed, bounded v1 decoder
contract with complete synthetic restoration vectors, explicit dependency
validation and strong output witnesses. No converter is simulated or enabled.
GCZ inner inspection, partition/FST/body validation, Redump resource integration,
RVZ/WBFS conversion and GUI exposure remain deferred.

## Ownership and validation record

Initial ownership scan checked **426 registered worktrees**. Dirty overlapping
integration points were:

- `crates/archivefs-core/src/game_identity.rs` in `/home/davedap/archivefs`,
  `emuwiz-082-9f-tape-refactor`, `emuwiz-082-batch1-dryrun`,
  `emuwiz-082-batch1a-dryrun`, `emuwiz-082-batch1b-dryrun`,
  `emuwiz-082-batch3-launch`, `emuwiz-082-batch4-dat-dryrun`,
  `emuwiz-082-batch4-dat-v2`, `emuwiz-082-first-wave-reapply-check`,
  `emuwiz-082-integration`, and `emuwiz-internal-gold` (all under `/home/davedap`).
- `crates/archivefs-core/src/repair/optical_conversion.rs` in
  `/home/davedap/emuwiz-arcade-readiness`, `emuwiz-custom-dat-lifecycle`,
  `emuwiz-scummvm-dosbox-readiness`, `emuwiz-snes9x-stella-readiness`, and
  `emuwiz-vita3k-readiness` (all under `/home/davedap`).

The final scope scan checked 427 worktrees, including this new worktree,
and found no changed foundation paths elsewhere.
No required foundation file overlapped those edits. No ownership boundary was
crossed; integration is design-only. Untracked research documents on main
were left untouched. The allowed change scope is this document, the two new
`gamecube_wii_boot_evidence/nkit` source/test files, and a two-line module export
in the existing GC/Wii file. No GUI, DAT internals, MAME, Wii U, queue, migration,
CLI, root library, adapter, Cargo or standalone patch files change.

Synthetic fixtures contain only invented headers/data. The largest image
fixture is a sparse **8,511,160,320-byte** Wii-shaped file; inspection still
reads 1,096 bytes. The largest recovery candidate is **2 MiB + 17 bytes**, which
exercises multiple 64 KiB hashing chunks. These are header/resource tests,
not evidence of full-disc reconstruction. A filename search under
`/home/davedap` found **zero** `.nkit.iso`, `.nkit.gcz` or `.nkit` files; no real
image was opened or converted. This was not a mounted-library inventory.

Validation used isolated `CARGO_TARGET_DIR=/tmp/emuwiz-nkit-target` on the
rebased candidate:

| Check | Result |
|---|---|
| `cargo test --offline --locked -p archivefs-core --lib` | **10,785 passed, 0 failed, 3 ignored**, 142.50 seconds (test runtime, excluding compilation) |
| NKit tests within that suite | **17 passed**: GC/Wii identification, exact fields, raw unknown values, bad signature/magic/version/flags, truncation, size bounds, recovery marker/metadata mismatch, different checksum domains, missing resource, no false recoverability, immutable inputs, no-write preview, sparse large input and unsafe paths |
| Existing GC/Wii boot-evidence tests | **9 passed** |
| Game identity tests | **240 passed**, including GC/Wii, RVZ, GCZ deferral, WBFS and safe-read cases |
| Optical conversion tests | **18 passed** (unchanged implementation) |
| `cargo check --offline --locked --workspace` | **Passed**; four existing GUI warnings (`LaunchWarningKind`, `show_with_playing_library_plan`, `dat_health_label`, `Informational`) left unchanged |
| `cargo fmt --all -- --check` | **Passed** |
| `git diff --check` and scope guard | **Passed** |

The full core suite ran with existing localhost test fixtures permitted; no
network-dependent production behavior was added. Queue integration is absent;
the existing queue tests nevertheless ran as part of the full core suite.
At final validation, local main, origin/main and the live remote all remained
`84792108c940c937098b7e567d5e46de466c841a`, with tracked main clean. No main
mutation or push was performed by this task. The foundation is suitable for
promotion as a read-only backend/research change; NKit conversion remains
unsupported and unqueued.

[format]: https://wiki.gbatemp.net/w/index.php?title=NKit/NKitFormat&oldid=70754
[guide]: https://wiki.gbatemp.net/w/index.php?title=NKit/UserGuide&oldid=70755
[nkit-current]: https://github.com/Nanook/NKit
[nstream]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/FilesAndStreams/NStream.cs
[gc-reader]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Readers/NkitReaderGc.cs
[wii-reader]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Readers/NkitReaderWii.cs
[gc-writer]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Writers/NkitWriterGc.cs
[wii-writer]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Writers/NkitWriterWii.cs
[gaps]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Gaps.cs
[junk]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/FilesAndStreams/JunkStream.cs
[hashstore]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/WiiHashStore.cs
[converter]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Converter.cs
[coordinator]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Coordinator.cs
[verify]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Writers/VerifyWriter.cs
[gcz]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Writers/GczWriter.cs
[recovery]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Settings/RecoveryData.cs
[fileitems]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Settings/FileItems.cs
[ndisc]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/DiscImage/NDisc.cs
[dat]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Settings/DatData.cs
[settings]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Settings/Settings.cs
[gc-recover]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Readers/RecoverReaderGc.cs
[wii-recover]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/NKit/Conversion/Readers/RecoverReaderWii.cs
[license]: https://github.com/Nanook/NKitv1/blob/31ac76828e6533ba3981eeda79ebfba2f16102af/LICENSE
[nod-header]: https://docs.rs/nod/1.4.4/src/nod/disc/mod.rs.html
[rvz]: https://github.com/dolphin-emu/dolphin/blob/master/docs/WiaAndRvz.md
