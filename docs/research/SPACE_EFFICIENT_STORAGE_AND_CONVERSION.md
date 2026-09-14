# Space-Efficient Storage, Conversion, and Competitor Landscape — Research

> **Research snapshot** — This document records research and design reasoning. It is
> not current capability documentation; see the [README](../../README.md),
> [roadmap](../../ROADMAP.md), and [launch support](../LAUNCH_SUPPORT.md) for present
> guidance. Nothing in this document has been implemented.

Status: **research only.** No production code, Publisher Profile code, Save Vault code,
GUI, or conversion feature was changed by this document. No user file was converted,
compressed, deleted, or moved. Researched against this repository at
`4c71ffe7a9bc282bcb86e13367fa45f9a516479a` (`main`, "feat(publisher): add safe directory
and symlink transactions").

Companion documents (already in this tree, and not contradicted here):

- [`CHD_VERIFICATION_IMPLEMENTATION_RESEARCH.md`](CHD_VERIFICATION_IMPLEMENTATION_RESEARCH.md) —
  CHD format model, current `.chd` handling in this repo, and the read-only
  `chd-rs` P0 recommendation.
- [`ARCHIVE_AWARE_DAT_VERIFICATION_RESEARCH.md`](ARCHIVE_AWARE_DAT_VERIFICATION_RESEARCH.md) —
  the general verification pipeline (outer container → payload → raw verification →
  DAT match → provenance-rich result).
- [`PUBLISHER_PROFILES_PHASE1.md`](PUBLISHER_PROFILES_PHASE1.md) — the publisher
  planning/link-mode surfaces this document reasons about.

**Tagging key**

| Tag | Meaning |
|---|---|
| **DOCUMENTED FACT** | Stated by an external, cited primary source (man page, kernel/format source, official tool or project documentation). |
| **CONCLUSION FROM SOURCE** | Stated by a `file:line` in *this* repository. |
| **INFERENCE** | Reasoning drawn by this document; not directly asserted by a source. |
| **UNCERTAIN** | Explicitly flagged as unverified. Not guessed at, and not to be built on without a probe or test. |

---
## 1. Executive summary

EmuWiz's stated direction — minimise duplicate storage, preserve source libraries, be
explicit rather than automatic, be reversible where possible, verify conversions, fail
closed on uncertainty, and avoid casual full-file copies — is **defensible and better
evidenced than the direction taken by most comparable tools** (section 14). This research
finds that the direction should be kept, and that only a small number of additions are
justified.

Headline results:

1. **Reflink publishing: ADOPT — but only as an explicit, probed, non-default third
   mode, never as an automatic substitute for anything.** A reflink shares physical data
   (copy-on-write) and is *not* a copy, a hardlink, or a symlink
   (**DOCUMENTED FACT**: `ioctl_ficlonerange(2)`). It is the only zero-extra-space
   publishing mode available on some same-filesystem-but-not-hardlinkable layouts, and the
   only one that isolates later mutation of the published file from the source. It is
   *not* available on ext2/ext3/ext4, NFS (client-side), many SMB/NTFS mounts, or on ZFS
   before OpenZFS 2.2, and ZFS's implementation has a documented partial-clone bug
   (section 5). Therefore: **probe, never assume; no silent fallback** (section 16).
2. **The publishing hierarchy should be HARDLINK → REFLINK → SYMLINK → COPY only when
   explicitly and individually requested** (section 16), with a live capability probe per
   (source, destination) pair and a *typed refusal* when the selected mode is unavailable —
   the shape the code already uses for `PublisherExecutionError::HardlinkUnavailable`
   (**CONCLUSION FROM SOURCE**
   `crates/archivefs-core/src/publisher_profile/execution.rs:78-81,224-231`).
3. **Safest compression formats for preservation-grade work: CHD (optical; `cdlz`/`cdzl`
   for CDs, `lzma`/`zlib` for DVDs) and RVZ (GameCube/Wii; `zstd`).** Both are
   hunk/block-structured for random access, both are designed to be reconstructed back to
   their source media, and both have first-party tooling with integrity metadata
   (**DOCUMENTED FACT**: `chdman` documentation; Dolphin `docs/WiaAndRvz.md`). CHD's
   defaults are asymmetric by media type and are *not* interchangeable: `createcd` on a
   cooked `.iso` produces a 2448-byte-frame CD image, so the extracted result is BIN/CUE,
   not the original ISO (sections 6.2 and 7).
4. **CSO/ZSO are acceptable for PSP and (CSO only) PS2, but are a weaker preservation
   choice than CHD:** CSO v1 is a single-file wrapper with no multi-track or subchannel
   topology, its block size trades random-access performance against ratio, and at least
   one codec choice (`libdeflate`) is documented as *incompatible with some PSP custom
   firmware* (**DOCUMENTED FACT**: `maxcso` README). Emulator support is narrower and
   version-sensitive: PCSX2 accepts CSO v1 and ZSO v1 but rejects CSO v2
   (**DOCUMENTED FACT** `pcsx2/CDVD/CsoFileReader.cpp:41-51`), and PPSSPP's block device
   accepts `CISO` plus CHD (**DOCUMENTED FACT**
   `Core/FileSystems/BlockDevices.cpp:535`, `:36`).
5. **WBFS and any "scrubbed" output are the two conversions EmuWiz must treat as
   preservation-hostile.** Scrubbing is defined by Dolphin as zero-filling clusters it
   considers junk (**DOCUMENTED FACT** `DiscIO/ScrubbedBlob.cpp:45-69`), and
   `dolphin-tool` exposes it as a flag (`--scrub`). WIA/RVZ store Wii partition data
   *decrypted and without hashes* to make it compressible
   (**DOCUMENTED FACT** `docs/WiaAndRvz.md:5-7`), so reconstruction is a rebuild, not a
   byte copy.
6. **Byte-identical reversibility must be treated as a per-pair, per-source property —
   never as a property of a format pair in general** (section 9). EmuWiz may advertise
   only: `BYTE-IDENTICAL REVERSIBLE` (proven by hash on *this* file), `CONTENT-EQUIVALENT
   REVERSIBLE` (proven by reconstruction plus semantic comparison), `PLAYABLE BUT NOT
   ORIGINAL-RECONSTRUCTABLE`, or `LOSSY / UNSAFE FOR PRESERVATION`. Anything unproven is
   the last class by default.
7. **No original may ever be offered for removal on exit-code-0 alone.** Section 11
   defines the evidence gate. The strongest available evidence is a reconstructed-output
   hash that matches the *pre-conversion* source hash for the same logical content, plus
   format-level integrity verification, plus topology/metadata comparison, plus a journal
   of source identity (device/inode/size/mtime/hash) captured **before** conversion.
8. **Space savings cannot be predicted precisely before conversion.** EmuWiz should
   present a **range or a coarse category, never a fake exact number**, and label it as an
   estimate derived from media type / source format / block structure; the exact figure is
   only available after conversion (section 12).
9. **Competitor landscape:** the closest philosophical match is `oxyROMon` ("only supports
   original and lossless ROM formats… can export in various popular lossy formats, leaving
   the lossless ROM files untouched"), and the only inspected tool shipping the exact
   `hardlink`/`symlink`/`reflink` link-mode triple is `igir` (`--link-mode`). No inspected
   ROM manager offers EmuWiz's combination of DAT-scoped identity evidence, fail-closed
   planning, journaled link transactions, and frontend-profile projection (section 14).
10. **Do not build** the things listed in section 19 — they include deletion of originals,
    automatic whole-library format "optimisation", background conversion without an
    explicit queue, block-level deduplication of the user's filesystem, and GUI exactness
    the data cannot support.

---
## 2. Method, evidence base, and environment

Method:

1. Read the repository's current storage, link, identity, and DAT-verification
   surfaces, so every recommendation lands on an existing seam rather than inventing one
   (section 3).
2. Read primary external sources: Linux man pages, kernel/filesystem documentation,
   filesystem source (via Elixir cross-reference), format references in the upstream
   implementations of the format owners (`mamedev/mame`, `dolphin-emu/dolphin`,
   `unknownbrackets/maxcso`, `PCSX2/pcsx2`, `hrydgard/ppsspp`, `openzfs/zfs`,
   `stenzek/duckstation`), and official project documentation for comparable applications.
3. Where a search engine result was the only route to a real-world behaviour claim, it is
   used for that behaviour only, never as the sole basis for a technical claim
   (**DOCUMENTED FACT** vs **INFERENCE** separation throughout).
4. Verified locally where possible: the host's filesystem type and the presence/version of
   the tools a converter would call.

Environment actually observed on the research host (evidence for section 18):

- `stat -f -c '%T'` on both `/home/davedap` and `/tmp` reports `ext2/ext3`. This matters:
  **the research host cannot reflink at all**, so any reflink path in EmuWiz must be
  exercised on btrfs/XFS/bcachefs/ZFS 2.2+ and must *fail closed* on this host by design.
- `chdman` is present at `/usr/bin/chdman` and reports
  `chdman - MAME Compressed Hunks of Data (CHD) manager 0.264 (unknown)`.
  **INFERENCE**: 0.264 is exactly the version `oxyROMon` documents as the minimum for
  Dreamcast CHD handling, which is a useful floor for any GD-ROM work.
- `7z`, `zip`, `unzip`, `filefrag`, `xfs_io`, `btrfs`, `du` are present;
  `dolphin-tool`, `maxcso`, and `dattool` are **not** installed
  (**CONCLUSION FROM SOURCE**: an EmuWiz conversion feature cannot assume Dolphin or
  maxcso tooling exists — tool absence must be a first-class, typed state, as
  `command_available` already does for `ratarmount` at `crates/archivefs-core/src/lib.rs:7203`).

## 3. EmuWiz's current storage, link, and conversion surfaces (grounding)

This section exists so the recommendations are not generic. Everything below is
**CONCLUSION FROM SOURCE**.

| Surface | Current state | Where |
|---|---|---|
| Link modes | Exactly two: `PublisherLinkMode::Hardlink` and `PublisherLinkMode::Symlink`. No reflink, no copy, "no automatic fallback or copy mode" | `publisher_profile/execution.rs:5-7,27-31` |
| Same-filesystem gate | `same_filesystem()` compares `st_dev`; returns `false` off-Unix | `publisher_profile/execution.rs:1062-1081` |
| Hardlink refusal | Typed, fail-closed, with user-facing guidance to choose explicit SYMLINK mode | `publisher_profile/execution.rs:78-81,224-231,358-365` |
| Transaction operations | `TransactionOperation::CreateHardlink` / `CreateSymlink` only; journaled, with identity re-check, confinement check, case-fold collision checks | `publisher_profile/execution.rs:404-415`; `dat/rename_apply/model.rs:267` |
| Planner default | Symlink. The planner's own comment states Hardlink stays "declared-but-unselected" because the evidence to select it automatically is insufficient | `publisher_profile/planner.rs:141-154` |
| Declared-but-unused action kinds | `PublisherActionKind` already declares `Copy`, `DirectoryCreate`, `MetadataWrite`, `PlaylistCreate` — declared, not selected by any reviewed profile | `publisher_profile/model.rs:173-182` |
| Executor scope | Operations act only on destination-side paths; sources are never moved | `publisher_profile/execution.rs:104-112` |
| `.chd` identity | Explicitly `IdentityImageFormat::Deferred` — recognised, never opened | `game_identity.rs:513` |
| `.chd` archive kind | Absent from `ArchiveKind::DirectGameImage` (`.iso`, `.gcm`, `.gcz`, `.rvz`, `.wbfs`, `.ciso` are present) | `lib.rs:3296-3327` |
| `chdman` usage in repo | None anywhere | repo-wide search |
| Subprocess abstraction | One generic argv-array runner (`run_command_os_with_timeout`), 30 s timeout, 64 KiB output cap — far too small for a CHD convert/verify pass | `lib.rs:7225-7283` |
| Archive handling | Read-only *mounting* (ratarmount) and read-only inspection. **EmuWiz has no archive writer, no compressor, and no converter of any kind today.** | `README.md`; `ROADMAP.md` |
| Reversibility discipline already present | Preview-before-apply, verify, rollback-or-refuse, no-clobber, "a rollback can refuse to act when a destination, backup, or journal no longer matches the verified state" | `README.md` (Current limitations) |
| Prior CHD research | `chd-rs` (pure Rust, in-process, read-only header identity + bounded streaming integrity) recommended for P0; **any claim that a CHD's bytes equal a Redump BIN/CUE hash without reconstruction must stay unproven** | `docs/research/CHD_VERIFICATION_IMPLEMENTATION_RESEARCH.md:205,422` |

**INFERENCE (important framing):** EmuWiz is currently a *consumer* of storage formats and
a *linker* of files. Everything in sections 6–12 is therefore about creating a new,
write-capable surface — which is exactly why sections 17 and 19 are deliberately narrow
and phase-gated.

## 4. Reflink vs hardlink vs symlink vs copy — what each one actually guarantees

### 4.1 What a reflink guarantees

**DOCUMENTED FACT** (`ioctl_ficlonerange(2)`, Linux man-pages):

- It is an ioctl pair — `FICLONE` (whole file) and `FICLONERANGE` (a byte range) — used to
  "make some of the data in the `src_fd` file appear in the `dest_fd` file by sharing the
  underlying storage".
- "**Both files must reside within the same filesystem.**" Failing that: `EXDEV`.
- "If a file write should occur to a shared region, the filesystem must ensure that the
  changes remain private to the file being written. This behavior is commonly referred to
  as **copy on write**."
- "Clones are atomic with regards to concurrent writes, so no locks need to be taken to
  obtain a consistent cloned copy."
- Errors include `EOPNOTSUPP` ("the filesystem does not support reflinking"), `EINVAL`
  (unreflinkable ranges; block-size alignment; XFS and Btrfs "do not support overlapping
  reflink ranges in the same file"), `EBADF` (source not open for read / destination not
  open for write / "the filesystem which `src_fd` resides on does not support reflink"),
  `ETXTBSY` (swap file), `EPERM` (immutable destination).
- Introduced in Linux 4.5; previously Btrfs-private (`BTRFS_IOC_CLONE`).
- Because CoW may need to allocate, `fallocate(2)` "may unshare shared blocks" — i.e. space
  can be *reserved* by unsharing.

**Critical nuance:** the destination file is a **new inode with its own metadata** that
merely shares data extents. Nothing about the source inode (ownership, mode, xattrs,
link count, or hardlink identity) is transferred by the clone. `cp` then applies attributes
separately (**DOCUMENTED FACT**: `cp(1)`, `--preserve`), which is why a "reflink copy" is
byte-equal but not inode-equal.

### 4.2 `cp --reflink` semantics (the userspace contract EmuWiz should mirror)

**DOCUMENTED FACT** (`cp(1)`, coreutils 9.11):

- Default / `--reflink=auto`: "cp will try a lightweight copy, where the data blocks are
  copied only when modified, **falling back to a standard copy if this is not possible**."
- `--reflink=always`: "cp will **fail** if CoW is not supported."
- `--reflink=never`: "ensures a standard copy is performed."

**INFERENCE — this is the single most important design input for EmuWiz's no-silent-fallback
rule.** EmuWiz must always behave like `--reflink=always`, never like the `auto` default:
the whole point of the storage policy is that the user is told, in the plan and in the
journal, which mechanism produced each published entry. A silent fallback to a full copy
would convert a "0 bytes" plan into a multi-terabyte write with no consent.

### 4.3 Comparison table

| Property | COPY | HARDLINK | REFLINK (FICLONE) | SYMLINK |
|---|---|---|---|---|
| Mechanism | Byte-for-byte duplicate | Second directory entry for the *same inode* | New inode sharing the same data extents (CoW) | A path stored as data in a new inode |
| Same filesystem required | No | Yes (and, for Btrfs, same subvolume — **UNCERTAIN**, section 18) | Yes ("both files must reside within the same filesystem") | No |
| Extra space at creation | = full file size | 0 (metadata inode only, already counted) | 0 to kilobytes (new inode + extent/refcount records) | 0 (a few hundred bytes) |
| Extra space after modifying the *published* file | 0 (already private) | **The source is modified too** | Only the changed blocks become private | The source is modified too (it *is* the source) |
| Extra space after modifying the *source* | 0 | **The published copy changes too** | Only the changed blocks become private | The published copy changes too |
| Deletion semantics | Independent | File exists until *all* links are gone (`st_nlink` decremented) | Independent inode; deleting one does not affect the other's data | Deleting the link deletes only the link |
| Survives the source being moved | Yes | Yes | Yes | **No — dangling** |
| Survives the source being deleted | Yes | Yes (data kept alive by the published link) | Yes | **No — dangling** |
| Detectable as "not a real file" | n/a | `st_nlink > 1`, or same `(st_dev, st_ino)` as the source | `FIEMAP_EXTENT_SHARED` on extents; `du` reports full blocks (**INFERENCE**, section 5.4) | `lstat` file type is symlink |
| Frontend/emulator compatibility | Best | Best | Best (it is a real file, real size, real content) | Usually fine; fails if the consumer does not follow links or has sandbox path rules |
| Backups / rsync / sync tools | Simple | Duplicated per-link unless `-H` | Duplicated unless the tool understands shared extents | Followed, or not, depending on flags |
| Failure mode if unsupported | n/a | `EXDEV` / `EMLINK` | `EXDEV` / `EOPNOTSUPP` / `EBADF` | Almost never unsupported |

**CONCLUSION FROM SOURCE:** EmuWiz already implements exactly the two rows' rejections it
can prove today — non-regular sources are refused (`execution.rs:218-223`) and cross-`st_dev`
hardlinks are refused with a typed error (`execution.rs:224-231`). A reflink mode would add
a third row with its own probe and its own typed refusal, not modify these.

### 4.4 What a reflink does NOT give EmuWiz

- It does **not** make the source safe from the *published* copy — actually it does, and
  that is its one advantage over a hardlink. But it does **not** prevent a *third* tool
  (patching, translation, trimming) from rewriting the published file and consuming the
  full size on disk. Space accounting must therefore be "shared until first write"
  (**INFERENCE**).
- It does **not** work across filesystems, across datasets (see ZFS below), or on the
  majority of Linux desktop installs today (ext4/ext2/ext3).
- It does **not** transfer hardlink identity, so it cannot be used as a substitute when the
  user's expectation is "the same file" (e.g. multi-disc playlists or a frontend that
  checks `st_ino`).

## 5. Filesystem support matrix for reflink, and how to detect support safely

### 5.1 Matrix

| Filesystem | Reflink (`FICLONE`) | Requirements | Same-FS constraint | Notes / evidence |
|---|---|---|---|---|
| **Btrfs** | **Yes** | Kernel ≥ 4.5 (the ioctls were Btrfs-private before that); userspace `cp` ≥ 8.x for `--reflink` | Same filesystem. Reflinking across *subvolumes* of one filesystem is expected to work (same superblock) — **INFERENCE, verify** | Feature is listed first-party: kernel docs list "Reflink, deduplication" among Btrfs's main features (**DOCUMENTED FACT**: kernel BTRFS page) |
| **XFS** | **Yes**, when the refcount btree exists | `mkfs.xfs -m reflink=1` (the **default**; requires `-m crc=1`, also default) | Same filesystem | `mkfs.xfs(8)`: the refcount btree "enables the sharing of physical extents between the data forks of different files, which is commonly known as `reflink`… allows up to four billion arbitrary inode/logical block pairs to map to a physical block… write will be redirected to a new block… (copy on write)". **Filesystem DAX is incompatible with reflink.** Kernel implements `.remap_file_range = xfs_file_remap_range` (**DOCUMENTED FACT**: `fs/xfs/xfs_file.c`, Elixir/linux) |
| **ext4 / ext3 / ext2** | **No** | — | — | `fs/ext4/file.c` contains **no** `remap_file_range` file operation at all (**DOCUMENTED FACT**: Elixir/linux; 0 occurrences, verified during this research). There is no upstream ext4 reflink. This is the default root filesystem on most Linux distributions, including the research host |
| **ZFS (OpenZFS on Linux)** | **Yes, since 2.2.0**, via block cloning | OpenZFS ≥ 2.2.0 (block cloning, PR #13392) | **Same dataset is required for practical cloning**; cross-dataset cloning is explicitly not handled (**DOCUMENTED FACT**: PR #15050 "does not attempt to address the issues surround cross-dataset cloning in Linux") | PR #15050 wired `copy_file_range`/`FICLONE`/`FICLONERANGE` to block cloning; `FIDEDUPERANGE` returns `EOPNOTSUPP`/`ENOTTY`. Release notes: block cloning "is used to implement 'reflinks' or 'file-level copy-on-write'". **Known defect:** issue #15728 "BRT: Linux FICLONE truncates large files with dirty blocks" — `cp --reflink=always` **reported success while producing a truncated output file** |
| **bcachefs** | **Yes** | Kernel with bcachefs (in-tree since 6.7) | Same filesystem | "Reflink" is listed among bcachefs's features (**DOCUMENTED FACT**: bcachefs documentation) |
| **tmpfs / ramfs** | No | — | — | No persistent block sharing; not a library location anyway |
| **NFS (client)** | Generally **no** client-side reflink | Server-side copy is a different mechanism | n/a | `copy_file_range(2)` describes "server-side-copy (in the case of NFS)" as a *copy-acceleration* path, not as `FICLONE` |
| **SMB/CIFS (cifs.ko)** | **Not via `FICLONE`** | — | n/a | SMB has its own server-side copy surfaced through `copy_file_range`; `FICLONE` is expected to fail `EOPNOTSUPP`/`EBADF` (**INFERENCE** from the ioctl's documented error list) |
| **FAT / exFAT / NTFS-3g** | No | — | — | No CoW/sharing model at all (**INFERENCE**) |
| **overlayfs / ecryptfs / fuse mounts** | **Do not assume** | Depends on layers and the FUSE implementation | n/a | May fail with `EOPNOTSUPP`; must be probed |

### 5.2 Why `st_dev` equality is a *conservative*, not a correct, reflink test

**CONCLUSION FROM SOURCE:** `same_filesystem()` compares `st_dev`
(`execution.rs:1062-1081`). That is a correct and conservative gate for hardlinks. It is
**not** sufficient for reflink because:

- Btrfs assigns distinct device numbers per subvolume, so `st_dev` can differ between two
  paths on the **same superblock**; `FICLONE` is a superblock-level operation and may
  succeed where a hardlink is refused. (**INFERENCE — flagged UNCERTAIN, section 18.**)
- Conversely, equal `st_dev` never guarantees `FICLONE` success (ext4 has one `st_dev` and
  no reflink at all).

**Recommendation:** reflink capability must be *probed*, not derived, and the probe should be
the operation itself against real files in the real destination directory.

### 5.3 Recommended runtime capability probe (fail-closed)

**INFERENCE** (probe design), grounded in the documented semantics above:

1. Confirm the destination directory exists, is a directory, and is inside the publisher
   root (EmuWiz already does this).
2. Open the **source** read-only through the existing safe-open path (no follow, trusted
   root, identity re-check).
3. Create a uniquely-named temporary destination **in the destination directory** (never in
   `/tmp` — the answer is only meaningful for that directory and its mount).
4. Attempt the clone: `ioctl(dest_fd, FICLONE, src_fd)`. Do **not** use `copy_file_range` as
   a capability test: it is documented to fall back to a real copy, so success proves
   nothing about CoW.
5. Delete the temporary file unconditionally, then re-verify the source identity
   (device/inode/size/mtime) has not changed.
6. Cache the result **per (destination directory, filesystem instance)** for the lifetime of
   a plan — not per file — and record the probe outcome in the plan's evidence.
7. Any of: probe error, unexpected `errno`, cleanup failure, or source identity drift ⇒
   **refuse that plan item** (never downgrade silently).

`cp --reflink=always` already fails closed and is the canonical reference behaviour
(**DOCUMENTED FACT**). If EmuWiz ever shells out instead of using the ioctl directly, it must
use `always`, never `auto`.

### 5.4 Space accounting and fragmentation findings

- `du(1)`: `-l`/`--count-links` exists specifically to "count sizes many times if hard
  linked"; the existence of that flag implies the **default counts a hardlinked file once**
  (**DOCUMENTED FACT** for the flag text; **INFERENCE** for the default). Hardlink
  publishing therefore shows the expected zero growth under `du`.
- For reflinks there is no inode-sharing signal for `du` to key on: it sums `st_blocks` per
  directory entry. **INFERENCE:** reflink publishing can make `du` *over-report* usage while
  `df` shows the true free space. This is the well-known "du lies about reflinks" behaviour
  users encounter; the GUI must state it up front rather than let the user discover it.
- The kernel exposes the truth per extent: `FIEMAP_EXTENT_SHARED` = "Space shared with other
  files" (**DOCUMENTED FACT**: `/usr/include/linux/fiemap.h`). `filefrag -v` and
  `xfs_io -c "fiemap -v"` are the practical instruments; their human-readable "shared"
  column is implementation detail, so prefer the `FIEMAP` flag when certainty is needed
  (**INFERENCE**).
- **Fragmentation:** sharing extents does not itself fragment data, but CoW writes after
  sharing allocate new extents, and filesystem compression cannot retroactively compress
  already-shared extents (**INFERENCE**). XFS documents the refcount btree as the enabling
  structure and makes no compression claim; Btrfs documents online defragmentation and
  compression as separate features. **Conclusion:** a reflinked library that the user later
  edits (patches, translations, trims) can reclaim *less* than expected, so reflink must not
  be the default.
- **ZFS specifics:** block cloning is bounded by dataset, and the documented
  truncation-on-dirty-blocks defect means a "successful" `FICLONE` on ZFS 2.2.x can produce a
  short file. **EmuWiz must verify destination size and content hash after any clone on
  ZFS**, never trust the return value alone.

### 5.5 Recommendation — SHOULD EMUWIZ SUPPORT REFLINK PUBLISHING?

**Classification: ADOPT** — as an explicit third link mode, non-default, probe-gated, and
never silently substituted.

Reasoning:

- It is the only zero-space mode that still works when a hardlink is impossible for reasons
  other than "a different filesystem" (Btrfs subvolume boundaries; any layout where the
  destination must not share inode identity) — **INFERENCE / partially UNCERTAIN**.
- It is the only zero-space mode that keeps the *source* immune to edits made to the
  *published* file. EmuWiz's own "preserve source libraries" principle is better served by
  reflink than by hardlink in that specific scenario.
- The implementation cost is genuinely low: one ioctl on a temporary file, the same typed
  refusal pattern the code already uses
  (`PublisherExecutionError::HardlinkUnavailable`), and one more mechanism string in the
  journal.
- The risk is genuine and is why it must not be the default: unsupported on the most common
  Linux filesystem; a documented integrity defect on ZFS 2.2.x; `du`-invisible space that
  can grow later; and no inode identity, so it cannot satisfy a consumer that expects "the
  same file".

Ordering relative to the other mechanisms (full policy in section 16):
**HARDLINK first → REFLINK second (when hardlink is refused, or when mutation isolation is
explicitly chosen) → SYMLINK third (cross-filesystem; the only mode that requires the source
to stay put) → COPY only on explicit request.**

Normal `COPY` is explicitly **not** recommended "for completeness": it is already available
to the user outside EmuWiz, it defeats the entire storage objective, and
`PublisherActionKind::Copy` remaining declared-but-unselected
(`publisher_profile/model.rs:178`) is the correct resting place for it.

## 6. Compression / decompression research by platform and media type

### 6.1 The three questions that decide whether a format is usable

**INFERENCE** — every candidate below is judged on:

1. **Is it a container with topology, or just a byte stream?** A format that stores track
   count, track modes, pregaps, and subchannel layout can preserve a disc; a format that
   stores only a byte array cannot.
2. **Is it block-addressed for random access?** Optical emulation reads scattered small
   chunks; a single-stream codec (gzip, solid archives) turns a 4 KiB read into a full-stream
   decode and is unsuitable regardless of ratio.
3. **Does the format own integrity metadata, and is that the same as source verification?**
   CHD and WIA/RVZ store SHA-1s over their own internal data (**DOCUMENTED FACT**: `chd.h`
   header fields; `WiaAndRvz.md`), which enables *format-level* verification — but neither
   publishes the hash of the *original* file, so format integrity is **not** source
   verification.

### 6.2 CHD — the strongest general optical choice

**DOCUMENTED FACT** (`chdman` documentation / MAME docs source; MAME `src/lib/util/chd.h`;
`src/lib/util/cdrom.h`; `src/tools/chdman.cpp`; local `chdman 0.264`):

- Container: header (`MComprHD`, version, codec ids, `logicalbytes`, `hunkbytes`,
  `unitbytes`, `rawsha1`, combined `sha1`, `parentsha1`), hunk map, and metadata; all
  big-endian. Hunk size "must be no smaller than 16 bytes and no larger than 1048576 bytes
  (1 MiB)" and "must be a multiple of the sector size or unit size of the media".
- Larger hunk sizes "may give better compression ratios, but reduce performance for small
  random reads as an entire hunk needs to be read and decompressed at a time" — hunk size is
  an explicit ratio **vs** random-access trade-off.
- Commands: `info`, `verify`, `createraw`, `createhd`, `createcd`, `createdvd`, `createld`,
  `extractraw`, `extracthd`, `extractcd`, `extractdvd`, `extractld`, `copy`, `addmeta`,
  `delmeta`, `dumpmeta`, `listtemplates`.
- **`createcd`** defaults: hunk = 8 frames (19,584 bytes); codecs `cdlz,cdzl,cdfl`.
  **`createdvd`** defaults: hunk = 2 sectors (4,096 bytes); codecs `lzma,zlib,huff,flac`.
  **`createld`** defaults to `avhu` (LaserDisc A/V only). The defaults are media-type
  specific and must never be copied between them.
- CD-aware codecs are documented as separate handling of audio and subchannel data: `cdzl`,
  `cdzs`, `cdlz` (LZMA for audio, zlib for subchannel), `cdfl` (FLAC for audio). `cdfl`/`flac`
  give "good compression ratios for audio CD tracks".
- `zstd`/`cdzs` "give very good compression and decompression performance with better
  compression ratios than zlib deflate, **but older software may not support CHD files that
  use Zstandard compression**" — a compatibility caveat that must be surfaced, not defaulted.
- `--hunksize` is marked **required** in the `createcd`/`createdvd`/`createld` sections while
  the prose says a default applies when omitted; the documentation is inconsistent here
  (**UNCERTAIN**: pass an explicit hunk size in any implementation; do not rely on defaults).
- `addmeta`/`delmeta` **modify their input file**, and `verify --fix` also modifies its input.
  Any implementation must treat these as write operations with a conversion-grade safety
  envelope, or avoid them entirely.
- `extractcd` supports `--outputbin`/`-ob` and `--splitbin`/`-sb`. From `chdman.cpp`:
  "GDIs will always output as split bin" (`:2689-2695`); a GD-ROM cue/bin output is forced to
  split per track because "GD-ROM cue/bin is in Redump format which should always be split by
  tracks"; and **subcode data cannot be represented in bin/cue or gdi** — "Track %d has
  subcode data. bin/cue and gdi formats cannot contain subcode data and it will be omitted."
  That is a documented, *stdout-warned-only* data-loss path: a parser looking at exit codes
  would miss it.
- CD track padding: `chdman.cpp` computes
  `padded = (trackinfo.frames + TRACK_PADDING - 1) / TRACK_PADDING` and
  `extraframes = padded * TRACK_PADDING - trackinfo.frames`, with `TRACK_PADDING = 4`
  (`cdrom.h:28`). CD frames are
  `FRAME_SIZE = MAX_SECTOR_DATA + MAX_SUBCODE_DATA` (`cdrom.h:35`). **INFERENCE:** a CD CHD's
  logical stream is a padded 2448-byte-frame stream, not the source `.bin` bytes — exactly
  the warning already recorded in `CHD_VERIFICATION_IMPLEMENTATION_RESEARCH.md`, and the
  reason a CHD's own SHA-1 is not a Redump BIN/CUE hash.
- GD-ROM is handled explicitly: `chdman.cpp` parses the TOC, tests `CD_FLAG_GDROM`, calls
  `adjust_high_density_area()` (`:2185-2192`), has a `MODE_GDI` output path (`:75`, `:1533`),
  and carries a `GDROM_OLD_METADATA_TAG` for legacy files (`:2540`). Metadata-tag macros
  include `CDROM_TRACK_METADATA_TAG` (`:2534`) and `DVD_METADATA_TAG` (`:2308`).
- `verify`: "The input file must be a read-only CHD format file (**the integrity of writable
  CHD files cannot be verified**)". **INFERENCE:** `chdman verify` is a *format-integrity*
  check, not proof that the source was reproduced.

**Emulator/tooling implications:** CHD is consumed by MAME (native), DuckStation ("MAME CHD";
"CHD images with built-in subchannel information are also supported"), PCSX2
(`ChdFileReader.cpp` via `libchdr`), and PPSSPP (`BlockDevices.cpp` →
`CHDFileBlockDevice`, via `libchdr`) — all **DOCUMENTED FACT**. **INFERENCE:** CHD is the
only single format plausibly usable for PS1, PS2, PSP, Dreamcast, Saturn, Sega CD, PC Engine
CD, and 3DO in EmuWiz's current platform registry, which is why it is the correct first
converter.

### 6.3 GameCube / Wii — RVZ, GCZ, WIA, WBFS, ISO

**DOCUMENTED FACT** (Dolphin `docs/WiaAndRvz.md`, the official WIA/RVZ format description;
Dolphin `Source/Core/DolphinTool/ConvertCommand.cpp`; `DiscIO/WIABlob.h`;
`DiscIO/ScrubbedBlob.cpp`):

- **RVZ and WIA are the same container family, differentiated by magic** —
  `WIA_MAGIC = "WIA\x1"`, `RVZ_MAGIC = "RVZ\x1"` (`WIABlob.h:40-41`).
- The format description states its own purpose: WIA's unique features "compared to older
  formats like GCZ" are (a) bzip2/LZMA/LZMA2 support and (b) "**Wii partition data is
  stored decrypted and without hashes, making it compressible**" (`WiaAndRvz.md:3-7`).
- "Like essentially all compressed GC/Wii disc image formats, WIA divides the data into
  blocks… Each chunk is compressed separately, making random access of compressed data
  possible." Chunk size "must be a multiple of 2 MiB".
- Compression enum: `None=0, Purge=1, Bzip2=2, LZMA=3, LZMA2=4, Zstd=5`
  (`WIABlob.h:28-36`). `dolphin-tool` rejects `purge` for RVZ and `zstd` for WIA
  (`ConvertCommand.cpp`), i.e. **RVZ is zstd-capable and WIA is not** — the opposite of a
  "pick either" situation.
- **RVZ packing**: runs of the GameCube/Wii padding pattern are replaced by a 68-byte PRNG
  seed, and decoded with a Lagged Fibonacci generator (f = xor, j = 32, k = 521)
  (`WiaAndRvz.md:196-247`). **INFERENCE:** packing is only *lossless* where the original
  "junk" really is the console's PRNG output; where it is not, the encoder stores the bytes
  literally (the format has a "read `size` bytes and output them unchanged" branch), so
  correctness depends on the encoder, not on an assumption.
- **Scrubbing is a different, deliberate data-destroying operation.** `dolphin-tool convert`
  exposes `-s/--scrub` = "Scrub junk data as part of conversion", and `ScrubbedBlob` fills
  scrubbable clusters with zeros on read (`ScrubbedBlob.cpp:53-56`). Any scrubbed output is
  **LOSSY / UNSAFE FOR PRESERVATION** by definition (section 9).
- `dolphin-tool`: `-f/--format` ∈ {`iso`,`gcz`,`wia`,`rvz`} (default RVZ);
  `-b/--block_size` with "Suggested value for RVZ: 131072 (128 KiB)";
  `-c/--compression` ∈ {`none`,`zstd`,`bzip2`,`lzma`,`lzma2`}; `-l/--compression_level`
  ("Suggested value for zstd: 5"); and for GCZ the tool warns when the block size is not
  legacy-compatible (`IsGCZBlockSizeLegacyCompatible`) for Dolphin < 5.0-11893.
  **INFERENCE:** RVZ with `zstd` level ~5 and 128 KiB blocks is the well-supported default
  for a modern EmuWiz workflow; `gcz` exists mainly for legacy compatibility.
- **WBFS**: not a Dolphin format; it is `wit`/WWiimms ISO Tools territory and is treated by
  `oxyROMon` as a lossy *export* target. Its structure is a filesystem of disc partitions
  rather than a byte image, so a WBFS output is **not** byte-identical to the ISO it came
  from (**INFERENCE**; no primary WBFS specification was retrieved during this research —
  **UNCERTAIN**, section 18).

### 6.4 PSP — CSO, ZSO, ISO, CHD

**DOCUMENTED FACT** (`maxcso` `README_CSO.md`, `README_ZSO.md`, `README.md`;
`PCSX2/pcsx2/CDVD/CsoFileReader.cpp`; `hrydgard/ppsspp` `Core/FileSystems/BlockDevices.cpp`):

- **CSO v1** (magic `CISO`, little-endian): header (`header_size` — "does not always
  contain a reliable value", `uncompressed_size`, `block_size` — "usually 2048", `version`,
  `index_shift`, 2 unused bytes), then `ceil(uncompressed_size / block_size) + 1`
  `uint32` index entries. The low 31 bits ≪ `index_shift` give the block offset; the block
  length is the delta to the next entry; the high bit means "stored uncompressed"; blocks use
  **raw deflate, window 15** (i.e. `inflateInit2(-15)`); "index entries must be
  incrementing. Reordering or deduplication of blocks is not supported."
- **CSO v2** is marked **EXPERIMENTAL** and adds lz4-as-alternative-block-method semantics.
  PPSSPP's comment is blunt: "**CSOv2 isn't actually a thing. It was partially implemented
  in maxcso but it has never been in active use**" (`BlockDevices.cpp:531`), and PCSX2 rejects
  `ver > 1` outright (`CsoFileReader.cpp:47-51`).
- **ZSO** (magic `ZISO`): "general format is the same as the CSO v1 format", but blocks are
  **lz4** instead of deflate; the format is declared "not final, and is experimental".
- `maxcso`: "always uses compression level 9"; larger block sizes "will help compression";
  `--block=16384` is the README's own PS2 example. Multi-codec trial compresses slightly
  better ("usual results are between 0.5% to 1.0% smaller"). Critical compatibility warning:
  "**Libdeflate is also disabled by default, because its output is not compatible with some
  PSP CFW.**"
- PPSSPP's block device validates CSO block size (power of two, ≥ one sector, ≤ 16 MiB),
  index alignment, index monotonicity, and truncation, and accepts **`CISO` magic only** in
  that code path (`BlockDevices.cpp:535`). **Caveat for EmuWiz:** ZSO support in PPSSPP is
  **not** proven by the code inspected here — treat "PPSSPP reads ZSO" as **UNCERTAIN** and
  verify per emulator version before offering ZSO for PSP.
- **CHD for PSP** is supported by PPSSPP via `libchdr` (`BlockDevices.cpp:36`, `:256`).
  **INFERENCE:** this makes CHD a *better* PSP recommendation than CSO where the emulator
  version supports it, because CHD keeps hunk-level random access without the CSO block-size
  coupling.

### 6.5 PS1 / PS2 — CHD, BIN/CUE, ISO, CSO/ZSO, gzip

**DOCUMENTED FACT** (PCSX2 `CDVD/` sources; DuckStation `README.md`; `chdman` docs):

- **PS1:** DuckStation reads "CD, bin/cue images, MAME CHD, single-track ECM, MDS/MDF, CCD,
  and unencrypted PBP formats", and notes "CHD images with built-in subchannel information
  are also supported". `chdman createcd` is the correct CHD path for CD media (8-frame hunks,
  CD codecs); `createdvd` must **not** be used for a CD-based disc.
- **PS2:** `createdvd` is correct for DVD media (2-sector hunks, `lzma,zlib,huff,flac`);
  PCSX2 reads CHD via `libchdr`, reads CSO/ZSO via `CsoFileReader.cpp`, and also ships
  `GzippedFileReader.cpp`. **INFERENCE:** gzip-wrapped ISO is the worst random-access option
  of the three and should be recognised-but-discouraged, i.e. an "identity only, no
  conversion" state.
- **CSO for PS2:** PCSX2 accepts CSO v1 and ZSO v1 and rejects CSO v2
  (`CsoFileReader.cpp:41-51`), requires a power-of-two frame size ≥ 2048, and selects lz4
  purely from the magic byte (`hdr.magic[0] == 'Z'`, `:137-138`). **INFERENCE:** PS2 CSO is
  only safe at CSO v1 / ZSO v1 and only if the emulator version is known — another case where
  EmuWiz must gate on evidence rather than assume.
- **ECM:** DuckStation supports single-track ECM, which is losslessly reversible — but
  `chdman` cannot read it, so ECM is a *decompression* target only, never a compression
  target for EmuWiz (**INFERENCE**).

### 6.6 Other systems worth considering (deliberately not a catalogue)

- **NES/SNES/FDS/N64/Genesis cartridges:** several well-known "compressions" are really
  *header/byte-order normalisations* and are byte-identical reversible **only when the exact
  transform is recorded**: copier headers (EmuWiz already models copier-header handling),
  SMD↔BIN de-interleaving (`smd_normalization`, both directions), N64 byte order
  (`n64_byte_order`, both directions). These are **not** storage compression and must never
  be sold as space savings.
- **Trimmed NDS/GBA/PSP ISOs, `.nsz`/`.xcz`:** trimming removes trailing padding, is
  **LOSSY / UNSAFE FOR PRESERVATION**, and breaks DAT hashes. Excluded (section 19).
- **TorrentZip / zstd-zip / 7z (MAME-style):** a *canonical, reproducible* archive is a
  legitimate preservation structure — RomVault and clrmamepro both build TorrentZip for
  exactly this reason. **INFERENCE:** for cartridge sets that are already `.zip`-canonical,
  the correct posture is to verify and leave alone, not to re-compress.
- **LaserDisc CHD (`createld`/`avhu`)** and **hard-disk CHDs (`createhd`)**: real, but out of
  scope for the first phases (section 17); LaserDisc already has its own research in
  [`LASERDISC_SET_VERIFICATION_V7.md`](../LASERDISC_SET_VERIFICATION_V7.md).
- **Aaru/`.aaru` and other archival formats:** genuinely relevant to long-term preservation,
  but a different ecosystem with no emulator consumption path in EmuWiz's launch matrix, and
  a large scope expansion. **Do not build** (section 19).

## 7. Compression-format matrix

Legend: **Lossless** = no content discarded; **Reversible** = the original structure can be
reconstructed; **Random access** = per-block/hunk decode is possible; **Topology** = track
count/modes/pregap/subchannel are representable.

| Format | Platforms / media | Lossless | Reversible | Topology | Subchannel | Random access | Tooling | Ratio character | Dangerous conversions | Verify |
|---|---|---|---|---|---|---|---|---|---|---|
| **CHD `createcd`** | CD family: PS1, Saturn, Sega CD, PCE CD, 3DO, Dreamcast GD-ROM | Yes | Yes, **with caveats** (4-frame track padding; subcode lost in bin/cue + gdi output) | Yes (`CHT2`/track metadata) | Yes, inside the CHD | Yes (hunk; 8 frames default) | `chdman`, `chd-rs`, `libchdr` | Medium–high; `cdfl` best for audio tracks | `createcd` on a cooked `.iso`; extracting to bin/cue or gdi when the source had subcode | `chdman verify` (read-only CHDs only) |
| **CHD `createdvd`** | DVD media: PS2, PSP UMD | Yes | Yes (unit = 2048) | Minimal (data only) | n/a | Yes (hunk; 2 sectors default) | as above | Medium–high; `lzma`/`zstd` | Using `createcd` for DVD media and vice versa | as above |
| **ISO (raw)** | all disc media | Yes | n/a (baseline) | None (no track table) | None | Native | none needed | 1:1 (or sparse) | Treating a multi-track disc as a single ISO | Whole-file hash |
| **BIN/CUE, GDI, CCD/IMG/SUB** | CD/GD optical | Yes | n/a (baseline) | Yes for CUE/GDI/CCD | BIN/CUE no; CCD/SUB yes | Native | none | 1:1 | "Converting" between these without a topology-preserving reader | Per-track hash + cue/gdi text compare |
| **RVZ** | GameCube, Wii | Yes (unless scrubbed) | Yes (via Dolphin; `oxyROMon` documents RVZ as interchangeable with Dolphin's own in both directions) | Wii partition table + disc header preserved | n/a | Yes (chunk; 128 KiB suggested) | `dolphin-tool`, `nod` (Rust, used by oxyROMon) | High with zstd | `--scrub` (lossy); RVZ→ISO where original junk was not PRNG-consistent (**UNCERTAIN**) | Dolphin header SHA-1s; **plus** reconstructed-ISO hash |
| **WIA** | GameCube, Wii | Yes (unless purged) | Yes | as RVZ | n/a | Yes (2 MiB-multiple chunks) | `wit`, Dolphin | Lower than RVZ (no zstd) | `Purge` compression type | as RVZ |
| **GCZ** | GameCube, Wii (legacy) | Yes | Yes | as RVZ | n/a | Yes (block) | Dolphin | Medium | Legacy block-size incompatibility with Dolphin < 5.0-11893 | Dolphin |
| **WBFS** | Wii | Yes | **Rebuild, not byte copy** | Wii partition-level | n/a | Yes | `wit` | Medium | Treating it as a preservation format | `wit verify` (**UNCERTAIN**) |
| **CSO v1** | PSP ISO, PS2 ISO | Yes | Yes (byte-exact per-block deflate/inflate round trip) | **None** (single byte stream) | None | Yes, at block granularity (`index_shift`) | `maxcso`, PCSX2, PPSSPP | Medium; block-size dependent | `libdeflate`-compressed CSO on PSP CFW; CSO v2 | Per-block round trip + whole-file hash |
| **ZSO / CSO v2** | PSP, PS2 | Yes | Yes | None | None | Yes | `maxcso`, PCSX2 | Lower than CSO v1 (lz4) | **Experimental**; PPSSPP acceptance **UNCERTAIN** | as CSO |
| **gzip'd ISO** | PS2 (PCSX2 `GzippedFileReader`) | Yes | Yes | None | None | **Effectively none** | PCSX2 | Medium | Choosing it at all for a large library | Whole-file hash |
| **7z / zip (as archive)** | cartridge sets, MAME sets | Yes | Yes | n/a | n/a | Per-member | `7z`, `zip` | High (solid 7z) / canonical (TorrentZip) | Solid 7z changes extraction cost; zip is per-member | Member hashes; DAT match |

**INFERENCE from the matrix:** the only candidates satisfying "container with topology +
block random access + first-party integrity metadata + first-party tooling" are **CHD**
(optical) and **RVZ** (GameCube/Wii). Everything else is a byte-stream wrapper (CSO/ZSO/gzip),
a rebuild (WBFS/WIA-with-purge), or a legacy/experimental variant.

## 8. Emulator compatibility findings

All rows below are **DOCUMENTED FACT** from the emulator's own repository or documentation,
except where marked. Version and variant gating is real: PCSX2 rejects CSO v2, and PPSSPP
accepts `CISO` only in the block-device code path inspected.

| Emulator | CHD | CSO | ZSO | RVZ | GCZ/WIA | Notes |
|---|---|---|---|---|---|---|
| **MAME** | Yes (format owner) | No | No | No | No | Supplies `chdman`; also `createld`/`avhu` for LaserDisc |
| **DuckStation** (PS1) | Yes ("MAME CHD"; subchannel-aware CHD supported) | Not stated | Not stated | n/a | n/a | Also ECM, MDS/MDF, CCD, PBP |
| **PCSX2** (PS2) | Yes (`ChdFileReader.cpp`, `libchdr`) | Yes, **v1 only** | Yes, **v1 only** | n/a | n/a | Rejects `ver > 1`; also `GzippedFileReader` for `.gz` ISOs |
| **PPSSPP** (PSP) | Yes (`BlockDevices.cpp`, `libchdr`) | Yes (`CISO` magic; block-size/index validation) | **UNCERTAIN** (no `ZISO` path found in the inspected block device) | n/a | n/a | Rejects CSO versions above 1 |
| **Dolphin** (GC/Wii) | No | No | No | Yes (format owner) | Yes | `--scrub` is lossy; RVZ↔ISO via `dolphin-tool convert` |
| **Flycast / other targets in EmuWiz's launch matrix** | Not verified in this research | — | — | — | — | **UNCERTAIN**; verify per emulator before recommending a format for a platform |
| **RetroArch/libretro (aggregate)** | Depends on the *core*, not RetroArch | — | — | — | — | Official guidance: "content from disc-based systems (Compact Disc images, etc.) should not be zipped for RetroArch use" |

**INFERENCE — the design implication:** format support is a property of *(emulator,
emulator version, platform, media type)*, not of a file extension. EmuWiz already models this
kind of evidence with explicit states (`IdentityStatus::{Confirmed, Probable, Deferred, …}`),
so any "which format should I use?" recommendation must be a **platform-specific storage
policy artefact** (section 17, phase C6), not a global setting.

## 9. Round-trip safety: what "safe reversible conversion" must mean

### 9.1 The four classes EmuWiz should use

Every conversion must be labelled with exactly one of these, derived from evidence gathered
on *the actual files involved* — never from a table of format pairs alone:

| Class | Definition | Evidence required to claim it |
|---|---|---|
| **1. BYTE-IDENTICAL REVERSIBLE** | Reconstruction yields a file (or file set) byte-identical to the source | SHA-256 of each reconstructed file equals the SHA-256 recorded for the corresponding source file **before** conversion, plus size/name/order equality for multi-file sets |
| **2. CONTENT-EQUIVALENT REVERSIBLE** | Bytes differ, but every user-visible property of the media is provably preserved: same track table, track bytes, sector data, and carryable metadata | Per-track/per-sector hashes match; topology (track count, modes, pregaps, index points) compares equal; container text (`.cue`/`.gdi`) compares equal after normalisation; the difference is confined to padding, metadata ordering, or container framing |
| **3. PLAYABLE BUT NOT ORIGINAL-RECONSTRUCTABLE** | The emulator runs it, but the original artefact cannot be rebuilt from the output | The format is documented as rebuild/export (WBFS, purged WIA), or the reconstruction path is undocumented/lossy |
| **4. LOSSY / UNSAFE FOR PRESERVATION** | Something was deliberately discarded (scrub, trim, junk removal), or the round trip is unproven | Any scrub/trim flag was used; or the tool emitted a warning the exit code does not reflect (chdman's subcode warning); or the round trip was never tested for this source |

**Rule (fail closed):** anything not positively demonstrated as class 1 or 2 is class 3, and
anything involving a discarding operation is class 4. There is no "probably reversible".

### 9.2 Round-trip matrix for the pairs EmuWiz is likely to meet

| Pair | Expected class | Reasoning / evidence | How it must be proven |
|---|---|---|---|
| **BIN/CUE → CHD (`createcd`) → BIN/CUE** | **2 — content-equivalent**; class 1 only for well-formed split-per-track Redump sources | CD CHDs pad each track to `TRACK_PADDING = 4` frames and store a 2448-byte frame stream (`cdrom.h:28,35`; `chdman.cpp` padding maths). Extraction rebuilds track data and a matching `.cue`, but `.cue` text and split/naming may be regenerated, and **subcode in the source is dropped on extraction** | Compare each track hash to the source track hashes and the normalised `.cue`; **refuse class 1** where the source had subchannel data |
| **GDI → CHD (`createcd`) → GDI** | **2 — content-equivalent** | GD-ROM is handled explicitly (`CD_FLAG_GDROM`, `adjust_high_density_area`, `MODE_GDI`, GDI always split-bin; GDI audio tracks are byte-reversed — `chdman.cpp:2952-2954`), so reconstruction exists but rewrites `.gdi` text and per-track files | Track hashes + normalised `.gdi`; verify high-density area handling; require `chdman ≥ 0.264` (as `oxyROMon` documents) |
| **ISO → CHD (`createdvd`) → ISO** | **1 — byte-identical** (expected) | `createdvd` uses 2048-byte units and stores a raw unit stream; `extractdvd` writes a single `.iso`. No track table, no padding beyond hunk alignment | SHA-256 of the reconstructed ISO == source SHA-256. If it fails, report class 3 — never silently retry with another tool |
| **ISO → CHD (`createcd`) → anything** | **3, and semantically wrong** | `createcd` interprets a cooked `.iso` as CD media, producing a 2448-byte-frame logical stream; extraction yields BIN/CUE (or split bins), not the original ISO. **This is the single most dangerous CHD mistake EmuWiz can make** | Refuse for DVD-family media outright; require explicit media-type detection before any CHD write |
| **ISO → RVZ (`zstd`) → ISO** | **2 at best; class 1 only where the source junk is PRNG-consistent** | WIA/RVZ store Wii partition data decrypted and without hashes (`WiaAndRvz.md:5-7`); RVZ packing substitutes PRNG seeds for padding runs (`:196-231`). Dolphin reconstructs the ISO; byte-identity depends on the source | SHA-256 of the reconstructed ISO == source SHA-256, **plus** Dolphin's own integrity check. Scrub flag ⇒ class 4 immediately |
| **ISO → CSO → ISO** | **1 — byte-identical** (expected) | CSO v1 is a per-block deflate/`store` wrapper with no topology (`README_CSO.md`); a correct decoder reproduces the ISO exactly | SHA-256 equality, plus a per-block round trip during conversion |
| **ISO → ZSO → ISO** | **1 — byte-identical** (expected); see emulator gating | ZSO = CSO v1 layout with lz4 blocks (`README_ZSO.md`); the format is declared experimental | SHA-256 equality; emulator acceptance is a *separate* question (section 8) |
| **RVZ → ISO → RVZ** | **Not class 1 in general** | Two RVZ encodes of the same ISO need not be byte-identical (block size, level, and packing decisions are encoder choices) | Compare ISO hashes, not RVZ hashes |
| **WBFS → ISO** | **3 — playable, rebuild** | WBFS is a partition store, not a byte image (**UNCERTAIN**; no primary spec retrieved) | Out of scope until proven |

**No exact-reversibility claim is made in this document for any CHD CD/GD pair without a
per-file hash comparison.** The prior CHD research in this tree reaches the same conclusion
independently, and that conclusion stands.

## 10. Per-format verification evidence catalogue

What can actually be compared, and what each comparison does and does not prove
(**INFERENCE**, built on the cited format facts above):

| Evidence | Available for | Proves | Does NOT prove |
|---|---|---|---|
| **Source SHA-256 (whole file)** | any file | The exact bytes EmuWiz had in hand, at a point in time | Anything about the converted file |
| **Reconstructed output SHA-256 vs source SHA-256** | BIN/CUE, GDI track sets, ISO, CSO/ZSO, RVZ-reconstructed ISO, CHD-reconstructed DVD | Byte-identity of the round trip — the only evidence strong enough for class 1 | If the tool rebuilt a *different but equivalent* structure, hashes differ even though nothing was lost (⇒ class 2, not class 4) |
| **Per-track hashes** | BIN/CUE, GDI, CCD/IMG, CHD-with-CD-metadata (via track extraction) | Track-level equivalence where container framing changed | Pregap/index/subchannel semantics unless compared separately |
| **Sector/content hashes (over 2048/2448-byte logical sectors)** | ISO/DVD-family media | That the *logical* media is unchanged when framing differs | Anything about discarded/re-added framing bytes |
| **CHD-internal hashes (`rawsha1`, combined `sha1`)** | CHD | Format-level integrity of the CHD's own logical stream, and delta-parent lineage | That the CHD equals the original source media (**DOCUMENTED FACT**: the logical stream is padded/framed, per `chdman.cpp` + `cdrom.h`) |
| **WIA/RVZ header hashes** | WIA, RVZ | Integrity of header structs and partition/group entries | The original ISO hash |
| **CSO/ZSO per-block round trip** | CSO, ZSO | That every block decodes to the bytes it encoded | That the *file* as a whole is unchanged (index/padding choices vary) |
| **DAT matching (CRC32/MD5/SHA-1/SHA-256)** | whatever the DAT describes | Whether the artefact is the canonical published dump — when the DAT describes *that* artefact (MAME `<disk>` vs Redump per-track `<rom>`) | Whether the conversion preserved anything: a DAT match is an identity claim, not a conversion-integrity claim |
| **Topology comparison (track count/mode/pregap/index; `.cue`/`.gdi` text)** | optical containers | That the disc *structure* survived | The bytes inside each track |
| **Metadata comparison (CHD metadata tags, WIA/RVZ structs, container text)** | CHD, WIA/RVZ | That metadata EmuWiz cares about survived, or which tags were added/dropped (`chdman` writes its own track metadata) | User-visible correctness |
| **Filesystem identity of the source (dev/ino/size/mtime + hash)** | any file | That EmuWiz did not touch the source, and that nothing else did either | Anything about the output |

**INFERENCE:** a single SHA-256 comparison is the *strongest* evidence and the *only* basis
for class 1 — but also the evidence most likely to be unobtainable (CD padding,
re-encryption, container regeneration). Verification must therefore be a **composite with an
explicit verdict per axis**, not a single boolean.

## 11. Verification model, and the "original can safely be removed" gate

### 11.1 Recommended verification levels

| Level | Name | Content | Applies to |
|---|---|---|---|
| **V0** | Source identity captured | Source dev/inode/size/mtime + SHA-256 **before** conversion starts, persisted with the conversion record | always |
| **V1** | Format integrity | The output's own integrity metadata validates (`chdman verify` on a read-only CHD; Dolphin's check for RVZ; per-block decode for CSO/ZSO) | always — and *never* sufficient alone |
| **V2** | Reconstruction + byte comparison | Reconstruct the source representation to a **separate temporary location** and compare per-file SHA-256 with V0 | whenever the pair is expected class 1/2 and the tool can reconstruct |
| **V3** | Content comparison | Per-track / per-sector hashes plus normalised topology and metadata comparison, where byte comparison is not meaningful | CD/GD-ROM CHDs, RVZ, anything class 2 |
| **V4** | DAT / canonical identity | The converted artefact (or its reconstruction) matches a DAT entry where the DAT actually describes it | only where the DAT vocabulary supports it — MAME `<disk>` for CHDs, per-track `<rom>` for BIN/CUE, and **never** by inventing a mapping |

**INFERENCE:** V0+V1 are cheap and always possible; V2 is the only evidence that supports
class 1; V3 is the fallback that supports class 2. Levels should be recorded per file, not
per batch, because a single unreadable member must not be able to "pass" on the strength of
its siblings.

### 11.2 Evidence required before EmuWiz may offer "Original can safely be removed"

**EmuWiz must never delete an original.** This section defines only what evidence would have
to exist before a *future, separately-approved* task could even *display* that offer. All of
the following must hold; any missing item ⇒ the offer must not be shown:

1. **V0 recorded and re-checked**: source dev/inode/size/mtime and SHA-256 captured before
   conversion, and the source *still* matches that identity at offer time (not modified,
   replaced, or swapped for a different file with the same name).
2. **V1 passed** on a finalised output whose size is non-zero and whose inode/mtime are
   stable (no partial write, no in-flight tool).
3. **V2 passed** — reconstructed bytes equal the V0 hashes for every source file in the set —
   **or**, where V2 is unachievable for that pair, **V3 passed with a positive class-2
   designation** that the user has seen.
4. **Topology and metadata equality** for multi-file/multi-track sources (track count, modes,
   pregaps, index points; normalised container text equal).
5. **The conversion is recorded as class 1 or class 2 by the evidence**, not by the pair
   table.
6. **The output is verified readable by the intended consumer** — i.e. the target
   emulator/profile format support is itself evidence-backed (section 8).
7. **No lossy flag was used** anywhere in the pipeline (no `--scrub`, no purge, no trim), and
   no tool emitted a data-loss warning its exit code did not reflect.
8. **A journal entry exists** naming the exact tool, version, arguments, both hashes, and the
   verification levels passed — and the journal itself is intact.
9. **The offer is per-item, explicit, and reversible**: a round-trip-backup of the source must
   remain recoverable for a stated period, or the source must still be reconstructable from
   the output by an EmuWiz-verified path.

**Explicit anti-rule (must appear in the UI and in code comments):** *exit code 0 from
`chdman`, `dolphin-tool`, `maxcso`, or any future converter is not evidence of anything.*
Every tool inspected here can succeed while producing a semantically different artefact
(chdman's subcode-omission warning; ZFS's truncating `FICLONE`; a CSO v2 "success" that no
inspected emulator accepts).

## 12. Space-savings estimation

### 12.1 Can EmuWiz estimate savings before conversion? Partly — and it should say so

**INFERENCE** — available signals, in descending order of trustworthiness:

| Signal | Source | Trustworthiness | Notes |
|---|---|---|---|
| **Source format already compressed?** | File extension + (preferably) header magic | **High** | A `.chd`/`.rvz`/`.cso`/`.gcz` source has already had the entropy removed by another codec. Re-encoding is usually **neutral or negative** (a different codec may win by a few percent — `maxcso` reports 0.5–1.0% for multi-codec trials) |
| **Used vs raw media size** | Container header (`logicalbytes` for CHD; `iso_file_size` for WIA/RVZ; `uncompressed_size` for CSO), or file size for ISO/BIN | High for containers, **low** for raw files without a reader | Tells EmuWiz the *denominator*, not the outcome |
| **Sparse / zero-extent fraction** | `FIEMAP`, `lseek(SEEK_HOLE/SEEK_DATA)`, `st_blocks` vs size | High | **Critical warning:** a source that is already sparse may show a large `du` win after conversion while `df` shows almost none — or the conversion may **increase** real usage by materialising zeros. EmuWiz must compare `st_blocks`, not `st_size` |
| **Compression sample** | Encode a bounded slice of the *real* source with the *actual* codec and hunk/block size, then extrapolate | **Medium** | `chdman` supports `--inputstartbyte`/`--inputbytes` (and hunk equivalents), so a trial encode of a slice into a temporary CHD is possible without a full conversion. Extrapolation is unreliable when the disc has heterogeneous regions (a PS2 disc's video stream vs its executable data) |
| **Historical statistics from the user's own library** | EmuWiz's existing SQLite catalogue: observed input/output sizes per (platform, source format, target format, codec, level) | Medium–high **after enough samples**, useless with few | Best long-run estimator, and it is honest: it is "what happened to your other 40 PS2 discs", not a promise |
| **Tool dry-run / info modes** | `chdman info` (existing CHD only), `dolphin-tool` (no dry-run), `maxcso` (no dry-run) | **Low** | None of the inspected tools provides a pre-conversion size prediction |

### 12.2 What EmuWiz must NOT do

- **Do not compute `size / 2` style heuristics** or present a "compression ratio" constant per
  platform. Media contents vary by orders of magnitude (a mostly-audio CD vs a mostly-video
  DVD).
- **Do not present a single number** before conversion. The honest output is a range plus the
  evidence it came from.
- **Do not compare against `st_size`** when the source may be sparse.
- **Do not treat "already compressed" as "no benefit"** without checking the target codec: a
  `.cso` (deflate) → CHD (`cdlz`) conversion can genuinely win, and CHD → RVZ is meaningless
  (different media).
- **Do not promise savings at all for cross-filesystem operations that materialise data** (a
  reflink or hardlink publishing plan saves space; a COPY plan does not, and must be labelled
  as consuming space).

### 12.3 Recommended GUI presentation

**INFERENCE / recommendation:** three distinct surfaces, never conflated:

1. **Before conversion (plan/preview):** a **category plus range**, always labelled
   `estimate`, with the basis named and the confidence stated as the evidence's own
   confidence:
   - `Already compressed` / `No saving expected` / `May grow slightly` — for already-compressed
     sources.
   - `Likely 0–10 % smaller`, `Likely 20–50 % smaller`, `Likely 50–75 % smaller` — ranges, not
     points.
   - Basis label: `based on media type`, `based on a sampled trial`, or
     `based on your library's history (N samples)`.
   - Sparse warning where applicable, phrased in `st_blocks` terms.
2. **After conversion (result):** the **exact** before/after bytes, `st_blocks` before/after,
   ratio to one decimal place, and the time taken. This is the point where a number is
   honest, and it becomes the input to (3).
3. **Library history (aggregate):** observed ratios per platform/format pair, with sample
   counts, so the ranges in (1) improve over time without anyone inventing precision.

**Refusal case:** if EmuWiz cannot establish the media type or the source container, it must
show **no estimate at all** rather than a default — matching the product's existing
"fail closed on uncertainty" posture.

## 13. Competitor and comparable-application landscape

### 13.1 What was inspected, and how

Each entry is based on the project's own repository or official site, read during this
research. Where the public material does not state a capability, this document says so rather
than filling the gap. **Activity status as observed on 13–14 September 2026.**

| Application | Kind | Activity observed | Primary evidence read |
|---|---|---|---|
| **RomVault** (+ SAM, DatVault, RV CommandLine) | Windows ROM manager, DAT-driven | Active (v3.8.0, Aug 2026) | `romvault.com` front page / changelog |
| **clrmamepro** / **clrmame** | ROM manager + rebuilder | Active (`clrmame 0.7.3`, Aug 2026; legacy 4.050 still shipped) | `mamedev.emulab.it/clrmamepro` news |
| **RomCenter** | Windows ROM manager | **Dormant** (4.2 released Feb 2024; author's Dec 2025 "development paused" status) | `romcenter.com` news feed |
| **RomM** | Self-hosted web ROM manager and player | Active | `rommapp/romm` README |
| **RetroDECK** | Flatpak all-in-one retro platform (ES-DE fork, components, tools) | Active | `RetroDECK/RetroDECK` README |
| **igir** | Node CLI ROM collection manager | Active | `emmercm/igir` README + generated `--help` |
| **Retool** | DAT filter/pre-processor for 1G1R | **No longer maintained** (project's own notice, issue #337) | `unexpectedpanda/retool` readme |
| **oxyROMon** | Rust CLI ROM organiser | Active (v0.23.0; repo now `alucryd/oxyromon`, pushed Sep 2026) | `alucryd/oxyromon` README |
| **SabreTools** (+ DatTools, FileTypes) | .NET DAT management suite + rebuilder/verifier | Active | `SabreTools/SabreTools` README |
| **`dolphin-tool`**, **`chdman`**, **`maxcso`**, **`wit`** | Format tools, not managers | Active / mixed | their own READMEs and docs |

Also genuinely relevant, identified but not deeply inspected (stated as such): **Dolphin**,
**DuckStation/PCSX2/PPSSPP/MAME** (consumers of the formats above), **Romba** (a depot format
referenced by SabreTools' rebuilder; not inspected here — **UNCERTAIN**), **Steam ROM
Manager**, **Skyscraper**, and **Hasheous/Playmatch** (hash→metadata services listed by RomM).

### 13.2 Factual findings per application

**RomVault** — **DOCUMENTED FACT** (site/changelog):

- Built-in **CHD** handling: "CHD built in support Chdman.exe is no longer needed!, RV now
  supports reading all CHD Versions, and compression", with parallel scanning claimed "around
  a 3 to 4 times speed improvement in scanning over chdman.exe".
- Archive formats: **TorrentZip** (a canonical zip standard the author documents), zstd-zip
  ("RVZSTD: RomVault zstd format"), and 7z in LZMA-solid/LZMA-non-solid/zstd-solid/
  zstd-non-solid variants, with raw-copy between archives; `SAM` (Structured Archive Maker) as
  a separate tool for structure/format migration and repair.
- DAT management: **DatVault** as a DAT distribution/update service, plus an MIA ("missing in
  action") tracking system with Auto-MIA.
- **No symlink, hardlink, or reflink capability appears anywhere in its public material**
  (searched for exactly those terms). Link-based publishing is therefore an EmuWiz
  differentiator, not a parity item.

**clrmamepro / clrmame** — **DOCUMENTED FACT** (site news): long-lived MAME-oriented manager
with scanner, fixer, rebuilder; the new C++ rewrite added a **dir2dat** module, configurable
compression level/method, nonmerged/standalone merge modes, zlibNg-backed zip speed-ups, and
(in `clrmameUI 0.4`) **CHD version checking**. Legacy `clrmamepro 4.050` is still published.
No link-based publishing, no frontend library projection.

**RomCenter** — **DOCUMENTED FACT** (site news): Windows/.NET manager with datafile plugins;
4.2 released Feb 2024; the author's Dec 2025 status post describes development as paused.
**INFERENCE:** treat as a historical reference rather than a live competitor.

**RomM** — **DOCUMENTED FACT** (README): self-hosted **server** ("Scan, enrich, browse and
play your ROM collection from one beautiful & free self-hosted app"), 400+ platforms,
metadata from IGDB/ScreenScraper/LaunchBox/MobyGames, SteamGridDB artwork,
RetroAchievements, saves and states synced across devices with conflict resolution, an
on-the-fly ROM patcher, mods/hacks/manuals, per-user permissions and OIDC SSO, and
EmulatorJS in-browser play. **INFERENCE:** RomM is EmuWiz's *destination*, not its
competitor — EmuWiz already builds RomM-compatible projections
(`playing_library/romm_projection.rs`), and RomM is server-first where EmuWiz is local-first.

**RetroDECK** — **DOCUMENTED FACT** (README): a self-contained Flatpak retro platform for
SteamOS/Linux bundling emulators, engines, ports and tools, maintaining **its own fork of
ES-DE** (`RetroDECK/ES-DE`). **INFERENCE:** EmuWiz's ES-DE/RetroDECK projection target is a
real, moving consumer — and this reinforces that EmuWiz must never rewrite a frontend config
silently, because the frontend ships its own fork and its own opinions.

**igir** — **DOCUMENTED FACT** (README's generated `--help`):

- Commands: `copy`, `move`, `link`, `extract`, `zip`, `dir2dat`, `fixdat`, `report`, `test`.
- **`--link-mode [choices: "hardlink","symlink","reflink"] [default: "hardlink"]`**, plus
  `--symlink-relative`: the only inspected tool shipping the full link-mode triple.
- Writing: `--zip-compression-type [torrentzip|rvzstd]`, `--zip-dat-name`, and output-path
  tokens for many ecosystems, including `{romm}`, `{es}`, `{retrodeck}`, `{batocera}`.
- DAT: multi-DAT processing, `--dat-combine`, parent/clone filtering, `--merge-roms`
  (fullnonmerged/nonmerged/split/merged), 1G1R (`--single`, prefer-regex, region/language
  filters), `--exclude-disks` (CHD disks in DATs), header parsing/removal, ROM patching,
  fixdats and reports, per-DAT/reader/writer thread controls, caching.

**Retool** — **DOCUMENTED FACT** (readme): a Redump/No-Intro DAT *pre-processor* for superior
1G1R — "You add your DAT files to Retool, and it creates new DAT files with all your
preferences, leaving the originals intact" — with priority-based region/language filtering,
demo/application exclusions, regex filters, and local filenames. **Unmaintained.** It delegates
all file management to RomVault/clrmamepro/igir.

**oxyROMon** — **DOCUMENTED FACT** (README): Rust CLI, "cross-platform opinionated ROM
organizer". "Designed with archiving in mind, so it **only supports original and lossless ROM
formats**. It can, however, **export in various popular lossy formats, leaving the lossless ROM
files untouched**." Subcommands include `import-dats`, `download-dats`, `import-roms`,
`sort-roms` (regions and/or 1G1R with `PREFER_*` election settings), `convert-roms`,
`check-roms`, `export-roms`, `purge-roms`, and a `Trash` subdirectory convention (deleted ROMs
move to `Trash`; `purge-roms -t` physically deletes). Conversion pairs listed: CUE/BIN↔CHD,
ISO↔CHD, ISO↔CSO, ISO↔RVZ, ISO↔ZSO; "CHD will be extracted to their original split CUE/BIN
where applicable"; "**CHD for Dreamcast requires at least chdman 0.264**". Settings include
`CHD_CD_HUNK_SIZE`, `CHD_CD_COMPRESSION_ALGORITHMS`, `CHD_DVD_HUNK_SIZE`,
`CHD_DVD_COMPRESSION_ALGORITHMS`, `CHD_PARENTS` (delta CHDs), `RVZ_BLOCK_SIZE` (default
128 KiB), `RVZ_COMPRESSION_ALGORITHM` (default `zstd`), `RVZ_COMPRESSION_LEVEL` (default 5),
and `RVZ_SCRUB` (applies only to `export-roms`). External tools: `chdman`, `dolphin-tool`,
`maxcso`, `wit` — or the optional Rust `nod` feature to handle RVZ/WBFS natively ("RVZ files
are interchangeable with Dolphin's own in both directions"; caveat: "`RVZ_SCRUB` has no
equivalent in `nod` and is ignored").

**SabreTools** — **DOCUMENTED FACT** (README): DAT creation (`Dir2DAT`), DAT conversion
between ClrMamePro/Logiqx/RomCenter, DAT splitting/statistics, **Rebuild From DAT** (outputs
unarchived, TAR, TorrentZip, 7zip, TorrentGZ; can rebuild from a Romba depot; fixdat output),
**Verify From DAT** (exact and hash-only modes), hashing up to SHA-512, and explicit handling of
**"Aaruformat, Archives, and CHDs … external hashes"**, with CHDs treated "like files" during
rebuild/verify. Header/copier-header tooling (NES, SNES, FDS, Lynx, 7800, PC Engine, PSID/SPC)
now lives in a separate project.

### 13.3 Feature-position summary (not a checklist — a comparison of *postures*)

| Capability | RomVault | clrmame | igir | oxyROMon | SabreTools | RomM | **EmuWiz today** |
|---|---|---|---|---|---|---|---|
| DAT import/audit | Yes | Yes | Yes | Yes | Yes | Partial (scan/enrich) | **Yes** (multiple formats, managed snapshots) |
| 1G1R | Yes | Yes | Yes | Yes | No | No | **Yes** (deterministic, explainable election) |
| Archive read | Yes (7z/zip/zstd) | Yes | Yes | Yes (7z/zip/sz/zstd) | Yes | Via server scan | **Yes, read-only mounts (zip/7z/rar)** |
| Archive *write* | Yes (TZIP/7z/zstd) | Yes | Yes (TZIP/rvzstd) | Yes | Yes | No | **No** |
| Conversion/compression | CHD (built-in) | CHD version check | No | CHD/RVZ/CSO/ZSO/WBFS | No | No | **No** |
| CHD support | Yes | Partial | DAT-awareness only | Yes | Yes (`<disk>`) | No | **Deferred (recognised, never opened)** |
| RVZ support | No | No | No | Yes | No | No | **No** |
| Hardlink publishing | Not found | No | **Yes (default)** | No | No | No | **Yes (explicit mode)** |
| Reflink publishing | Not found | No | **Yes** | No | No | No | **No** |
| Symlink publishing | Not found | No | Yes | No | No | No | **Yes (planner default)** |
| Journaled transactions + rollback | Not documented | No | No | Trash dir, no journal | No | DB, no journal | **Yes (preview/verify/rollback-or-refuse)** |
| Frontend profile projection (RomM/ES-DE) | No | No | Path tokens only | No | No | *is* the frontend | **Yes (projections + no-clobber transactions)** |
| Local-first / offline | Yes | Yes | Yes | Yes | Yes | **No (server)** | **Yes** |
| GUI | Yes | Yes | No | No | No | Web | **Yes (desktop)** |

**INFERENCE:** EmuWiz's differentiators are not "more formats"; they are **evidence discipline
(identity states, DAT-scoped refusal), storage discipline (link modes with no silent
fallback), and transaction discipline (journal + rollback-or-refuse)**. The competitive gap is
entirely in **format handling**: CHD/RVZ/CSO conversion, which is what section 17 proposes
to close — narrowly and gated.

## 14. What each competitor teaches EmuWiz

For each important application: **what they do better**, **what EmuWiz already does better**,
**the idea worth adopting**, and **what EmuWiz must not copy**.

### 14.1 RomVault

- **Better than EmuWiz:** CHD handling is a first-class, in-process engine (all CHD versions,
  reading *and* compression) rather than a deferred extension — no external `chdman`
  dependency at all. Its archive matrix (TorrentZip, zstd-zip, four 7z variants, raw-copy
  between archives) is far ahead of EmuWiz's read-only mount story. DatVault solves DAT
  *supply* as a service, and Auto-MIA models "which dumps are known-missing", which EmuWiz
  does not.
- **EmuWiz does better:** no Windows-only dependency; deterministic, explainable 1G1R election
  with recorded evidence; explicit identity/refusal vocabulary; journaled transactions with
  rollback; local-first with no account or hosted service; Linux-native CLI + GUI.
- **Idea worth adopting:** *in-process container reading instead of shelling out*, for the
  read-only half — precisely the `chd-rs` P0 recommendation already in this tree
  (`CHD_VERIFICATION_IMPLEMENTATION_RESEARCH.md`). Also: **"the DAT needs a supply chain"** —
  EmuWiz imports DATs and keeps snapshots, but has no curated, self-updating DAT catalogue
  concept, which RomVault/DatVault shows is the feature users feel most.
- **Must not copy:** "repair by default". RomVault's model assumes the tool may rebuild and
  re-compress a library in place. EmuWiz's promise ("your collection stays yours", previews,
  no silent rewrite) is incompatible with a default-repair posture. Also do not invent a *new*
  proprietary archive format (its "RVZSTD" zip variant) — that fragments preservation.

### 14.2 clrmamepro / clrmame

- **Better than EmuWiz:** decades of corner-case experience with MAME sets (merge modes, device
  roms, samples, nonmerged/split/fullnonmerged), and a rebuilder that works at set level rather
  than file level. It now checks CHD versions, and ships `dir2dat` (DAT from a folder), which
  EmuWiz lacks.
- **EmuWiz does better:** cross-platform and not MAME-first; modern CLI/GUI; frontend
  publishing; safety/rollback; and no requirement to understand parent/clone semantics to avoid
  damage.
- **Idea worth adopting:** **`dir2dat`-style local DAT generation is a genuine gap** — it is how
  users make DATs for material no public DAT covers, and it is a *read-only* feature (hash a
  folder), so it fits EmuWiz's safety model perfectly. Adopt the concept, not the dialect
  trivia.
- **Must not copy:** the "scan → fix" idiom that mutates a library to match a DAT. EmuWiz audits
  and reports; it must not treat a DAT as an instruction to rewrite files.

### 14.3 RomCenter

- **Better than EmuWiz:** nothing current — development is paused (site status, Dec 2025) and
  its datafile-plugin model is Windows-bound.
- **EmuWiz does better:** everything it does today, plus it is maintained.
- **Idea worth adopting:** none.
- **Must not copy:** plugin-per-DAT-format architecture. EmuWiz's single well-tested parser
  surface with explicit `NeedsReview`/refusal states is better; a plugin market is how DAT tools
  accumulate silent mis-parses.

### 14.4 RomM

- **Better than EmuWiz:** it is the frontend and the server — metadata enrichment at scale,
  per-user permissions, save-sync with conflict resolution, in-browser play, and an API other
  tools integrate with. It defines the very directory contract EmuWiz targets.
- **EmuWiz does better:** local-first and offline, no daemon or database service to run, no
  user accounts; identity/DAT verification *before* publishing; link-based publishing so the
  frontend's library costs no extra storage; explicit previews of what would be written into
  RomM's layout.
- **Idea worth adopting:** **treat RomM as a consumer contract and keep the dependency
  one-way** — EmuWiz should be able to describe (and explicitly write) what RomM expects, but
  RomM's schema must never become a hard dependency of EmuWiz's scanner/verifier. Also worth
  adopting: RomM's explicit **conflict-resolution** vocabulary for saves/states is a good model
  for presenting conversion conflicts (never silently "winning").
- **Must not copy:** server-first assumptions — a required login, a required database service, a
  required network sync, or hosted telemetry. "Runs locally on Linux; your collection stays
  yours" is the differentiator.

### 14.5 RetroDECK

- **Better than EmuWiz:** a complete, curated, community-verified runtime with emulators, tools,
  and its own ES-DE fork. It can guarantee a working emulator stack.
- **EmuWiz does better:** it need not ship emulators to be useful; it works with what the user
  already has; it verifies identity and DAT membership; it does not impose a Flatpak sandbox on
  the library.
- **Idea worth adopting:** **publish a machine-readable compatibility matrix of what the
  downstream frontend/fork actually supports** (RetroDECK ships its own ES-DE), so EmuWiz can
  refuse a format the user's frontend cannot read — the *platform storage policy* artefact in
  section 17 (phase C6).
- **Must not copy:** bundling emulators, or claiming ownership of the emulator configuration.
  EmuWiz already (correctly) refuses to silently edit emulator/frontend configuration.

### 14.6 igir

- **Better than EmuWiz:** breadth and speed of *bulk* operations — mass extract/archive, linking
  with three modes, patching, header removal, MAME merge modelling, and a rich output-path
  templating language. It has the only `--link-mode reflink` among the inspected tools.
- **EmuWiz does better:** identity evidence and DAT-scoped verification before acting; a GUI;
  transactional safety (plan → preview → verify → rollback); no Node runtime; explicit refusal
  states rather than "best effort"; desktop-safe path handling.
- **Idea worth adopting:** **the three-mode link enum is the right shape** — hardlink default,
  symlink, reflink as an explicit choice — and `--symlink-relative` as an explicit *policy*
  rather than a hidden default. Adopt the shape (three explicit modes, no auto-fallback) and the
  insight that **relative symlinks are a policy decision** (source-move robustness), while keeping
  EmuWiz's stronger probe/refusal behaviour.
- **Must not copy:** the "do the whole library in one command with defaults" posture. igir's
  default hardlink mode and default zip compression type are exactly the implicit decisions
  EmuWiz exists to make explicit. EmuWiz must never default a *write* operation.

### 14.7 Retool

- **Better than EmuWiz:** 1G1R preferences as a *first-class, DAT-preprocessing* problem —
  region/language priority, exclusions, regex filters, local names, and community-maintained
  clone lists.
- **EmuWiz does better:** it is maintained; it performs 1G1R on *real* evidence (identity,
  availability) rather than rewriting a DAT in advance; and it explains its election.
- **Idea worth adopting:** **local/official title names and exclusion categories belong in the
  election inputs**, not in the naming layer. The election should be able to consume "prefer the
  Japanese local name", "discard Beta", "discard Virtual Console" as evidence-backed preferences
  — the same knobs oxyROMon exposes as `PREFER_*` / `DISCARD_*` / `LANGUAGES` / `REGIONS_*`.
- **Must not copy:** maintaining community clone lists as a core responsibility, and silently
  rewriting user DATs. Any DAT transformation must be an explicit, separate artefact — never a
  side effect.

### 14.8 oxyROMon

- **Better than EmuWiz:** the closest working reference implementation of EmuWiz's *conversion*
  ambition, and it gets the philosophy right — lossless-only originals, lossy **exports** that
  leave originals untouched, a `Trash` convention instead of immediate deletion, per-format
  configuration knobs (CHD hunk size/codecs, RVZ block size/algorithm/level), delta-CHD parent
  support, and a documented external-tool matrix with an optional native Rust path (`nod`) for
  RVZ/WBFS. It also documents the exact real-world gotcha EmuWiz needs (`chdman ≥ 0.264` for
  Dreamcast CHD).
- **EmuWiz does better:** DAT/identity evidence discipline; refusal states; previews and
  journaled rollback (oxyROMon relies on `Trash` + `check-roms`, with no transaction journal);
  GUI; frontend profile projection; and no requirement to adopt its opinionated directory layout.
- **Ideas worth adopting (in priority order):**
  1. **`convert-roms` vs `export-roms`**: conversion keeps the artefact lossless and verified;
     export produces lossy artefacts and never touches the original. This maps exactly onto
     EmuWiz's class 1/2 (convert) vs class 3/4 (export) vocabulary and gives users a safe place
     for "I want a small file for my handheld".
  2. **`Trash` + explicit purge**: never delete, always move to a recoverable location, and make
     physical deletion a separate explicit command.
  3. **Per-format configuration with documented defaults** (hunk size, codec list, block size,
     level) instead of one global "compression level".
  4. **Require the tool version as evidence** (the `chdman ≥ 0.264` rule) — record and enforce
     minimum tool versions per conversion, exactly as EmuWiz already does for `ratarmount`.
- **Must not copy:** the requirement that the user reorganise their library into oxyROMon's
  system/directory model before it will help, and its tolerance for conversion paths that depend
  on external binaries being present with no capability probe. EmuWiz must probe, name the exact
  tool version, and refuse when it is missing — not fail with a stack trace.

### 14.9 SabreTools

- **Better than EmuWiz:** DAT *engineering* — conversion between DAT dialects, splitting,
  merging, diffing, statistics, and a rebuilder with multiple output formats including
  TorrentZip/TorrentGZ plus Romba depot interop. It explicitly models CHD/Aaruformat external
  hashes and treats CHDs as verifiable units.
- **EmuWiz does better:** consumer-grade UX, transactional safety, frontend publishing, and a
  single coherent identity model instead of many CLI programs.
- **Idea worth adopting:** **DAT conversion/merging/splitting as an explicit, separate artefact**
  (an output *new* DAT, never a mutation of the user's DATs). This is the safe version of
  Retool's idea, and would let users build collection-specific views without leaving EmuWiz.
- **Must not copy:** "hash-only verification" as an acceptable outcome without a provenance
  label. EmuWiz verdicts must always carry *how* they were reached — already the provenance axis
  in `ARCHIVE_AWARE_DAT_VERIFICATION_RESEARCH.md`.

### 14.10 Format tools (`chdman`, `dolphin-tool`, `maxcso`, `wit`)

- **Better than EmuWiz:** they *are* the format owners and will always know their formats better.
  `chdman` in particular is both the reference encoder and the reference verifier.
- **EmuWiz does better:** nothing yet — it has no format handling at all.
- **Idea worth adopting:** **treat each tool as an oracle with a version, a capability probe, and
  a machine-readable result**, and cross-check the tool's own verification against EmuWiz's
  independent hash evidence (the "optional external oracle" pattern already proposed for CHD).
  Also adopt `--reflink=always`-style strictness: no tool may be invoked in a mode that silently
  degrades.
- **Must not copy:** invoking a tool for a *write* on a user's file without a preview, a
  destination other than the original, a recorded tool version, and post-write verification.

## 15. Gap analysis (ranked)

### 15.1 HIGH VALUE / LOW RISK

1. **Reflink as an explicit third link mode with a live probe** — small, contained, reuses the
   existing typed-refusal and journal machinery; closes both the "same filesystem but cannot
   hardlink" hole and the "the published file must not be able to corrupt the source" concern
   (sections 4–5).
2. **Capability detection as a first-class, typed state** (tool present/absent/too-old, filesystem
   reflink yes/no, cross-filesystem yes/no) surfaced in plans and the GUI *before* anything is
   written. Zero write risk, and immediately useful even with no converter present.
3. **Source-identity journaling (V0) for every file EmuWiz is about to operate near** —
   dev/inode/size/mtime + SHA-256, captured before the operation. This is the precondition for
   *every* later safety claim (section 11), and it is pure bookkeeping.
4. **Round-trip class labelling (classes 1–4) in the plan/preview**, with the evidence that
   produced it, even before a converter exists for a given pair — it turns "trust me" into "here
   is why".
5. **Relative-vs-absolute symlink as an explicit policy** (with the stated robustness difference
   when the source moves), because the current transaction convention hard-codes absolute targets
   (`execution.rs:409-414`) and users on portable drives genuinely need the choice.
6. **A plan-only (`--dry-run`) JSON surface for every storage operation**, so the storage policy
   can be scripted and diffed like the rest of EmuWiz's JSON API.

### 15.2 HIGH VALUE / NEEDS RESEARCH

1. **CHD read-only identity + integrity for `.chd` files** — already researched and recommended in
   this tree (`chd-rs`, in-process, bounded, no exec). Needs a fixture strategy and a DAT mapping
   decision (`<disk>` parsing) before implementation.
2. **Safe CHD *write* path (compression) with media-type detection** — high value, but requires
   reliable CD-vs-DVD media detection, explicit hunk/codec selection, tool-version enforcement, and
   a verification pipeline that can reconstruct and hash. Needs a prototype on legal synthetic
   fixtures on a filesystem other than this host's.
3. **Round-trip reconstruction for CD-family CHDs (V2/V3)** — the hardest correctness problem here
   (track padding, pregap, subchannel, `.cue`/`.gdi` regeneration). Must be its own phase with its
   own fixtures, per the prior CHD research's "separately phased, fail-closed" requirement.
4. **RVZ read/convert (GameCube/Wii)** — high value for a large share of many libraries, but depends
   on either `dolphin-tool` (absent on the research host) or the `nod` Rust crate, and on proving
   byte-identity of ISO reconstruction on the user's actual discs (section 9).
5. **Sparse-aware savings estimation** using `FIEMAP`/`SEEK_HOLE` and `st_blocks`, plus a
   sampled-trial estimator. Needs empirical work to avoid over-promising (section 12).
6. **Platform storage policy matrix** (which format for which platform, given which
   emulator/frontend) — needs per-emulator verification (Flycast et al. are unverified) and a
   maintenance story for a matrix that will age.

### 15.3 USEFUL LATER

1. **Conversion history / rollback UI** on top of the journal — meaningful only once conversions
   exist.
2. **Resumable/batched conversion queue** with per-item checkpoints — needed only once large
   batches are real.
3. **PSP CSO/ZSO** — lower ratio benefit than CHD, version-sensitive emulator support, and
   `maxcso` is not installed by default. Do it after CHD.
4. **Local DAT generation (`dir2dat`-style)** — useful and read-only, but a different feature area.
5. **DAT conversion/merge/split as explicit artefacts** (SabreTools-style) — same reasoning.
6. **Delta/parent CHD support** — powerful for multi-disc libraries, but a large complexity step
   that only makes sense after single-file CHD conversion is proven.
7. **Archive *writing* (TorrentZip/native zstd-zip)** for cartridge sets — a big feature with a
   small storage win over a verified canonical archive that already exists.

### 15.4 NOT WORTH BUILDING

1. **A global "optimise my library now" button** — no single policy is correct across platforms,
   filesystems, emulator versions, and user intent; the plan/preview model exists precisely
   because of this.
2. **A per-platform "compression ratio" constant used to promise savings** — fake precision
   (section 12).
3. **A proprietary EmuWiz archive/format** — fragments preservation and defeats frontend
   compatibility (already rejected as "what not to copy" from RomVault).
4. **Windows/macOS support for these paths** — the repository's mount and watcher design is
   deliberately Linux-only; adding storage formats does not change that, and reflink/CoW semantics
   differ entirely there.
5. **Automatic in-place re-compression of an existing `.chd`/`.rvz`** — "re-encode to save a few
   percent" is exactly the casual full-file rewrite the project exists to avoid.

### 15.5 CONFLICTS WITH EMUWIZ PHILOSOPHY

1. **Silent fallback from reflink/hardlink to copy** (the `cp --reflink=auto` behaviour) —
   contradicts explicit-over-automatic and would turn a 0-byte plan into a full copy.
2. **Deleting originals after conversion without the section-11 evidence gate** — contradicts
   preserve-sources and verify-everything.
3. **Background conversion without an explicit, visible queue** — contradicts
   explicit-not-automatic.
4. **Filesystem block-level deduplication** (`duperemove`/`bees`, or building a deduplicating
   store for the user's files) — a whole-library, hard-to-reverse mutation with its own data-loss
   history, and properly the filesystem's job, not a ROM manager's.
5. **Scrub/trim/"optimise junk data" offered as a conversion** — class 4 by definition, breaks DAT
   hashes, and is precisely the casual full-file rewrite the project rejects. Offering it as an
   *export-only, clearly-labelled lossy* action is defensible; offering it as a conversion is not.

## 16. Recommended EmuWiz storage policy (publishing hierarchy)

### 16.1 The hierarchy

**Recommended user-facing policy — exactly this order, evaluated per item, never globally:**

| Rank | Mode | When it is the right choice | Space cost | Source-move robustness | Frontend compatibility |
|---|---|---|---|---|---|
| **1** | **HARDLINK** | Proven same filesystem (`st_dev` equal — the existing gate) *and* the user wants the published entry to be the same file | 0 | Survives source move and deletion | Best (a real file, real inode content) |
| **2** | **REFLINK** | The user explicitly selects it, **or** hardlink was probed and refused while the same-superblock clone probe succeeds (Btrfs subvolume boundaries and similar) | ~0 at creation; can grow if the published file is later modified | Survives source move and deletion | Best (a real file with its own inode) |
| **3** | **SYMLINK** | Cross-filesystem, or the user wants the source to remain the single authoritative copy and accepts the coupling | ~0 | **Breaks if the source moves or is deleted** | Good, but fails for consumers that do not follow links or that apply sandbox path rules |
| **4** | **COPY** | Only when the user explicitly and individually requests a physical duplicate (e.g. a removable drive that must be self-contained) | Full file size | Fully independent | Best |

**Rules that make this policy EmuWiz-shaped rather than generic:**

1. **Every item records the mechanism that was actually used**, in the plan, the journal, and
   the JSON output. A published entry whose mechanism is unknown is a bug.
2. **No silent fallback, in either direction.** If hardlink is selected and impossible → typed
   refusal naming the reason and the alternative (`HardlinkUnavailable`, which already tells the
   user to choose explicit SYMLINK mode: `execution.rs:227-230`). If reflink is selected and
   `FICLONE` fails → typed refusal, not a copy.
3. **The probe precedes the plan, not the write.** Capability is established per (source,
   destination) pair before the user is shown a plan, and the plan states it.
4. **COPY is never a default and never an automatic degradation**, for the reason in section 4.2.
5. **The mechanism is reversible where the filesystem says so**: a hardlink can be removed
   without touching the source; a reflink can be removed without touching the source; a symlink
   can be removed trivially; a copy leaves an orphan that EmuWiz must therefore *track* (it is
   the only mode where deletion of the source is genuinely lossless-looking and still wrong to
   assume).

### 16.2 Same-filesystem vs cross-filesystem cases

- **Same filesystem, `st_dev` equal:** hardlink first. The current gate is right; keep it.
- **Same filesystem, `st_dev` different (Btrfs subvolumes, btrfs `mount` variations, overlay
  layers):** hardlink may be impossible even though it is one filesystem, and reflink may
  succeed. **UNCERTAIN** — this must be *probed*, and the probe result, not the filesystem name,
  decides. (Section 18, item 1.)
- **Cross-filesystem (different `st_dev` and different mount):** hardlink and reflink both fail
  (`EXDEV`). Only SYMLINK or COPY are available. EmuWiz must say so plainly, and default to
  SYMLINK while warning that the source must now stay put.
- **Cross-filesystem *and* the consumer cannot follow symlinks:** COPY is the only option, and
  the plan must show the exact bytes that will be written before the user agrees.

### 16.3 Source mutability and robustness

- **If the source is in a location EmuWiz does not manage** (external drive, network share, a
  directory the user reorganises): symlink is fragile. Hardlink and reflink both keep the data
  alive; hardlink is preferable because it also keeps them *identical*.
- **If the source may be patched or modified later:** reflink is the safer publishing choice
  because the published copy will not silently change under the frontend — but a hardlink also
  guarantees the frontend gets the *updated* content, which is what some users want. This is
  exactly the kind of choice EmuWiz should present, not decide.
- **If the source may be deleted by another tool:** symlink dangles. The plan must state it, and
  EmuWiz's existing dangling-symlink detection (`romm_browse.rs`, `romm_config`) is the right
  place to report it afterwards.

### 16.4 Space consumption presentation

For each plan, show three numbers, all in terms of **allocated blocks (`st_blocks`)**, not
`st_size`:

1. **Bytes written now** — 0 for hardlink/reflink/symlink, full size for copy.
2. **Free space before/after** (`statvfs`) — the only number that cannot mislead.
3. **A footnote for reflink** stating that `du` may report the published library as larger than
   it really is, and that a later edit to a published file will consume real space (section 5.4).

### 16.5 What to do with the existing planner default

**CONCLUSION FROM SOURCE:** the planner currently emits `Symlink` for every item and documents
why (`planner.rs:141-154`: no same-filesystem evidence exists at planning time to justify a
hardlink). That reasoning is sound, and this research does not overturn it.

**Recommendation:** keep SYMLINK as the *planning* default, and make the *link mode* an explicit
plan input (`PublisherLinkMode` extended with `Reflink`), so:

- the plan's default (symlink) stays honest and safe;
- the user can explicitly choose hardlink or reflink, and the executor then applies the probe
  and the typed refusal;
- the plan's evidence records which mode was requested *and* which was proven available.

This is the minimum change that adds a mode without weakening any existing guarantee, and it is
the shape `igir` validates with its `--link-mode` enum.

## 17. Recommended converter roadmap

Each phase has a **hard safety boundary** — the thing that must not be crossed in that phase,
regardless of how useful it would be. Phases are ordered by *evidence dependency*, not by
convenience: nothing writes until reading and verification are proven.

### PHASE C1 — read-only conversion capability detection (no writes at all)

**Goal:** EmuWiz can truthfully say what it *could* do, per file and per destination, without
touching anything.

- Detect tool presence + **version** (`chdman`, `dolphin-tool`, `maxcso`), mapping absence to a
  typed state (the existing `command_available` pattern at `lib.rs:7203`).
- Detect source container and media type from **headers**, not extensions: CHD header
  (`unitbytes`/`hunkbytes`/codecs), RVZ/WIA magic + header, CSO/ZSO magic + block size, ISO/BIN
  heuristics, `.cue`/`.gdi` topology.
- Detect filesystem capability for the target directory: `st_dev` comparison (already exists) and
  a **reflink probe** (section 5.3).
- Compute and display the **round-trip class** each candidate pair could achieve *if* the evidence
  supports it (default: class 3, "not established").
- Report the exact command line that *would* be run, without running it.

**Hard boundary:** read-only. No ioctl except the reflink probe's temporary file (which must be
deleted, and must never be inside the user's library); no conversion, no metadata write, no
`chdman addmeta`. No GUI "Convert" button before C2 exists.

### PHASE C2 — safe CHD compression (write, verified, never in place)

**Goal:** a user can compress a supported optical source to CHD, and EmuWiz can prove what
happened.

- Media-type-driven command selection (`createcd` vs `createdvd`), **with an explicit refusal if
  the media type is not established**.
- Explicit `--compression` and `--hunksize` — never rely on tool defaults (section 6.2 documents
  their per-command divergence and the docs' own inconsistency).
- **Write to a new file in a staging location on the same filesystem as the destination**, then
  verify, then move into place. Never overwrite, never in place.
- Verification: V0 before, V1 after (`chdman verify`), and **capture stdout/stderr** so the
  subcode-omission warning (section 6.2) becomes a recorded finding instead of a lost line.
  **NOTE:** the existing runner's 30 s timeout and 64 KiB output cap (`lib.rs:7225-7283`) are
  unusable here — a conversion-grade runner is a prerequisite, not an implementation detail.
- Record: source identity, tool name + version, complete argv, output size, output `st_blocks`,
  elapsed time, verification levels, assigned class.

**Hard boundary:** never modify or delete the source; never write into the source's directory
unless the user chose that destination; refuse rather than guess the media type; refuse if the
tool version is below the documented minimum for the media (e.g. `chdman ≥ 0.264` for GD-ROM);
no deletion, no source cleanup, no "reclaim space" action.

### PHASE C3 — CHD decompression and round-trip verification

**Goal:** prove the round trip before anyone is tempted to delete anything.

- `extractcd`/`extractdvd` to a **fresh temporary directory**, then compare per-track /
  per-sector / whole-file hashes against V0; compare normalised `.cue`/`.gdi`; compare topology.
- Assign the class (1 or 2) from the comparison and record, in machine-readable form, exactly
  which bytes differed and why for a class 2.
- Where class 2 is unavoidable (CD padding, regenerated `.cue`), present it explicitly — never as
  a failure, and never as an unqualified success either.

**Hard boundary:** reconstructed output goes to a temporary location and is deleted on the user's
instruction (or kept only on explicit request); EmuWiz still does not delete the original. The
"original can safely be removed" *offer* remains out of scope for this phase — it needs the
section-11 gate plus a separate, explicit decision.

### PHASE C4 — RVZ support (GameCube / Wii)

- Prefer a **native Rust path** (`nod`-class library) if it can be audited; otherwise
  `dolphin-tool` with an enforced minimum version. Either way: `zstd`, block size 128 KiB, level
  ~5 as documented defaults, with the reason shown to the user.
- **Scrub is not offered as a conversion.** If the concept appears at all it is an explicitly
  labelled lossy *export* with its own warning, never part of the convert path.
- Verification: Dolphin's integrity check *plus* reconstructed-ISO SHA-256 vs V0 (section 9.2).

**Hard boundary:** media detection (GameCube vs Wii vs anything else); refuse when the source is
already RVZ (no re-encode); refuse scrub inside a conversion; never touch the source.

### PHASE C5 — PSP CSO/ZSO

- Only if still demanded after C2–C4; only CSO v1 / ZSO v1 (the versions PCSX2 accepts); only with
  `maxcso` present and its version recorded; and only with the `libdeflate`-CFW compatibility
  caveat surfaced (or libdeflate disabled by default, matching `maxcso`'s own choice).
- Verification: whole-file SHA-256 round trip (ISO→CSO→ISO) plus a per-block check.

**Hard boundary:** no CSO v2 (no inspected emulator accepts it — PPSSPP's own source says so); no
ZSO for a platform/frontend whose acceptance is unproven; never offer CSO for a multi-track disc
(it cannot represent one).

### PHASE C6 — conversion queue, storage policy, GUI, history

- A **visible, pausable queue** with per-item state showing: source, target, class, estimated
  range, tool version, disk space required, and the verification level passed. No background
  conversion the user cannot see and stop.
- **Platform storage policy matrix** (sections 8 and 14.5): per platform, which format is
  recommended, the emulator/frontend evidence behind it, and the date it was verified.
- **History and rollback** on top of the journal: every conversion replayable, every verification
  re-runnable, every output removable without touching the source.
- **Savings reporting**: ranges before, exact after, aggregate over history (section 12.3).

**Hard boundary:** the queue never acts without a plan; the GUI never has a "convert everything"
default; rollback never touches the source; and no item may be marked "safe to delete original" by
a batch process — that decision is per item, per section 11, and is a separate feature with its own
review.

### 17.1 Phase ordering vs the requested example ordering

The requested ordering is **preserved unchanged** — the research supports it. Two deliberate
internal changes:

1. **C2 must include the stdout/stderr-capturing conversion runner and the staging-directory
   mechanics.** They are prerequisites, not polish; today's generic runner cannot support them.
2. **A "no-write identity + evidence" slice belongs inside C1**, because EmuWiz already has the
   identity and DAT machinery to consume it — the highest-value, zero-risk work is entirely in C1.

## 18. Open questions that must be verified empirically before implementation

These are the **UNCERTAIN** items collected above. Each must be answered on real
hardware/filesystems before any code depends on it.

1. **Btrfs subvolume semantics.** Do `st_dev` values differ per subvolume? Does `link(2)` return
   `EXDEV` across subvolumes while `FICLONE` succeeds? (Asserted here as **INFERENCE** from the
   ioctl's superblock-level check; not tested.) This determines whether REFLINK is genuinely needed
   alongside HARDLINK for same-filesystem cases.
2. **XFS reflink with `reflink=0`.** Confirm a reflink attempt on an XFS filesystem created with
   `-m reflink=0` fails (`EOPNOTSUPP`) and that `cp --reflink=always` fails there rather than
   copying.
3. **ext4 confirmation on supported kernels.** Confirm the clone ioctl fails
   `EOPNOTSUPP`/`ENOTTY` on ext4 across the supported kernel range, and that the section-5.3 probe
   reports this cleanly.
4. **ZFS 2.2.x status of issue #15728.** Is the truncating-`FICLONE` defect fixed in the versions
   EmuWiz's users would have? Until answered, ZFS reflinks must be size-**and**-hash verified, or
   refused.
5. **RVZ byte-identity.** For real GameCube and Wii discs, does `dolphin-tool convert` RVZ→ISO
   reproduce the original ISO's SHA-256? Which sources fail, and is the failure confined to junk
   regions (content-equivalent) or does it extend to real data?
6. **CHD byte-identity for DVD-family media.** Confirm `createdvd`/`extractdvd` round-trips
   byte-identically for PS2 and PSP sources, including the final partial hunk.
7. **CHD CD/GD round trip against a real Redump DAT.** Which track hashes survive, which `.cue`/
   `.gdi` fields are regenerated, and what the class-2 discrepancy looks like in practice. This is
   the single most important empirical test in the roadmap.
8. **PPSSPP ZSO support.** The inspected block device accepts `CISO` only. Confirm whether any
   current PPSSPP version reads `ZISO`, and if so on which code path.
9. **Flycast (and any other EmuWiz launch target) CHD/GDI handling** — unverified here, and required
   before EmuWiz recommends CHD for Dreamcast in a platform storage policy.
10. **WBFS primary specification.** No primary source was retrieved; the "rebuild, not byte copy"
    judgement is an inference and must be confirmed or dropped.
11. **Relative symlink viability per frontend.** Do ES-DE (and RetroDECK's fork), RomM's ingest, and
    the tested emulators all tolerate relative symlinks in the published tree? This decides whether
    the relative-symlink policy (section 15.1) can ship at all.
12. **`du`/`df` behaviour of reflinked trees on btrfs and XFS** as users' tooling reports it —
    needed to word the section 16.4 footnote accurately rather than approximately.

---

## 19. DO-NOT-BUILD list (explicit)

EmuWiz must **not** build, in this area:

1. **Any deletion of an original file** — not after a conversion, not after verification, not
   behind a flag, and not in this task's scope. Only a future, separately-reviewed feature could
   *offer* it, and only under the section-11 gate.
2. **Automatic conversion of a whole library**, or any "optimise" action that runs without a
   per-item plan the user has seen.
3. **Silent fallback of any link mode to any other mode**, including `cp --reflink=auto` semantics
   and copy-on-hardlink-failure.
4. **Scrub, trim, junk-removal, or "game shrinking" as a conversion option.** Lossy export at most,
   clearly labelled, and never applied to a verified original.
5. **Filesystem-level deduplication** of the user's data (`duperemove`, `bees`, custom
   block-hashing stores) — a whole-library mutation, and the filesystem's job.
6. **In-place rewriting of an existing `.chd`, `.rvz`, `.cso`, or archive** — including
   "re-compress to save a few percent".
7. **A proprietary EmuWiz container or archive format.**
8. **A global compression-level/ratio setting** spanning formats, platforms, and media — the correct
   knobs are per-format and per-media (section 12).
9. **GUI-exact savings numbers before a conversion has happened** (section 12.3).
10. **A conversion path that depends on an external binary's *default* behaviour** rather than
    passing explicit arguments and enforcing a minimum version.
11. **Any conversion feature that does not record tool name, tool version, full argv, source
    identity, and verification results.** If it cannot be recorded, it must not be offered.
12. **Windows/macOS ports of the storage/conversion paths** in the current architecture.

## 20. Sources inspected

### 20.1 Linux / filesystem / link semantics

- `cp(1)` — Linux man-pages (coreutils): `--reflink=auto|always|never`, `--preserve`, `--sparse`.
  <https://man7.org/linux/man-pages/man1/cp.1.html>
- `ioctl_ficlonerange(2)` / `ioctl_ficlone(2)` — Linux man-pages: `FICLONE`/`FICLONERANGE`
  semantics, same-filesystem requirement, copy-on-write, error list, Linux 4.5 history.
  <https://man7.org/linux/man-pages/man2/ioctl_ficlonerange.2.html>
- `copy_file_range(2)` — Linux man-pages: in-kernel/server-side copy, reflink "copy acceleration",
  cross-filesystem rules, `EOPNOTSUPP` since 5.19.
  <https://man7.org/linux/man-pages/man2/copy_file_range.2.html>
- `link(2)`, `symlink(2)`, `du(1)`, `statfs(2)` — Linux man-pages.
- `mkfs.xfs(8)` — Linux man-pages: `reflink=0|1`, refcount btree, CoW, default-on, `crc=1`
  requirement, DAX incompatibility. <https://man7.org/linux/man-pages/man8/mkfs.xfs.8.html>
- BTRFS — Linux kernel documentation: feature list including "Reflink, deduplication".
  <https://www.kernel.org/doc/html/latest/filesystems/btrfs.html>
- `fs/xfs/xfs_file.c` — Elixir cross-reference (Linux): `.remap_file_range = xfs_file_remap_range`.
  <https://elixir.bootlin.com/linux/latest/source/fs/xfs/xfs_file.c>
- `fs/ext4/file.c` — Elixir cross-reference (Linux): **no** `remap_file_range` (0 occurrences).
  <https://elixir.bootlin.com/linux/latest/source/fs/ext4/file.c>
- OpenZFS 2.2.0 release notes: block cloning (#13392), "used to implement 'reflinks' or file-level
  copy-on-write". <https://github.com/openzfs/zfs/releases/tag/zfs-2.2.0>
- OpenZFS PR #15050: "Linux: wire up `copy_file_range`, `FICLONE`, etc to block cloning";
  cross-dataset cloning explicitly out of scope; `FIDEDUPERANGE` returns `EOPNOTSUPP`/`ENOTTY`.
  <https://github.com/openzfs/zfs/pull/15050>
- OpenZFS issue #15728: "BRT: Linux FICLONE truncates large files with dirty blocks".
  <https://github.com/openzfs/zfs/issues/15728>
- OpenZFS PR #17308: removal of pre-4.5 compat shims for those ioctls.
  <https://github.com/openzfs/zfs/pull/17308>
- bcachefs documentation: "Reflink" among features. <https://bcachefs-docs.readthedocs.io/>
- Btrfs documentation: online defragmentation/compression.
  <https://btrfs.readthedocs.io/en/latest/btrfs-filesystem.html>
- `/usr/include/linux/fiemap.h` (local): `FIEMAP_EXTENT_SHARED` = "Space shared with other files".

### 20.2 Format and tool documentation

- `chdman` documentation (MAME docs source and rendered page): commands, options, defaults,
  codecs, hunk-size limits, `verify` semantics.
  <https://raw.githubusercontent.com/mamedev/mame/master/docs/source/tools/chdman.rst> ·
  <https://docs.mamedev.org/tools/chdman.html>
- MAME `src/lib/util/chd.h`: v1–v5 header layouts, `rawsha1`/`sha1`/`parentsha1`.
  <https://raw.githubusercontent.com/mamedev/mame/master/src/lib/util/chd.h>
- MAME `src/lib/util/cdrom.h`: `TRACK_PADDING = 4`, `FRAMES_PER_HUNK = 8`,
  `FRAME_SIZE = MAX_SECTOR_DATA + MAX_SUBCODE_DATA`.
- MAME `src/tools/chdman.cpp`: GD-ROM handling (`CD_FLAG_GDROM`, `adjust_high_density_area`,
  `MODE_GDI`), GDI split-bin rule, subcode-omission warning, track padding maths, metadata tags.
- Dolphin `docs/WiaAndRvz.md`: official WIA/RVZ format description (chunks, compression enum, RVZ
  packing + PRNG). <https://raw.githubusercontent.com/dolphin-emu/dolphin/master/docs/WiaAndRvz.md>
- Dolphin `Source/Core/DolphinTool/ConvertCommand.cpp`: `--format`/`--scrub`/`--block_size`/
  `--compression`/`--compression_level`, GCZ legacy block-size warning.
- Dolphin `Source/Core/DiscIO/WIABlob.h`: magics, `WIARVZCompressionType`, header structs.
- Dolphin `Source/Core/DiscIO/ScrubbedBlob.cpp`: scrubbing is zero-fill on read.
- `maxcso` README, `README_CSO.md`, `README_ZSO.md`: CSO v1/v2 and ZSO layout, codecs, level,
  block-size trade-offs, libdeflate/CFW incompatibility. <https://github.com/unknownbrackets/maxcso>
- PCSX2 `pcsx2/CDVD/ChdFileReader.cpp`, `CsoFileReader.cpp` (plus `GzippedFileReader.cpp` in the
  directory listing): CHD via `libchdr`; CSO v1/ZSO v1 only; `ver > 1` rejected.
  <https://github.com/PCSX2/pcsx2/tree/master/pcsx2/CDVD>
- PPSSPP `Core/FileSystems/BlockDevices.cpp`: `libchdr` CHD block device; `CISO` magic; CSO
  validation; "CSOv2 isn't actually a thing". <https://github.com/hrydgard/ppsspp>
- DuckStation README: supported disc formats including MAME CHD, ECM, CCD, MDS/MDF, PBP, and
  subchannel-aware CHD. <https://github.com/stenzek/duckstation>
- RetroArch/libretro docs (ROM handling guidance): "content from disc-based systems … should not be
  zipped for RetroArch use". <https://docs.libretro.com/guides/roms-playlists-thumbnails/>
- `wit`/Wiimms ISO Tools WIA documentation. <https://wit.wiimm.de/info/wia.html>

### 20.3 Comparable applications

- RomVault (site + changelog: CHD engine, TorrentZip/zstd zip/7z variants, SAM, DatVault,
  Auto-MIA). <https://www.romvault.com/>
- clrmamepro / clrmame (news: versions 0.2–0.7.3, dir2dat, zstd zip, CHD version checking; legacy
  4.050 still shipped). <https://mamedev.emulab.it/clrmamepro/>
- RomCenter (news: 4.2 Feb 2024; "development paused" Dec 2025). <https://www.romcenter.com/>
- RomM README (self-hosted server, metadata providers, saves sync, ROM patcher, permissions).
  <https://github.com/rommapp/romm>
- RetroDECK README (Flatpak platform; its own ES-DE fork). <https://github.com/RetroDECK/RetroDECK>
- igir README + generated `--help` (`--link-mode hardlink|symlink|reflink`, zip types, output-path
  tokens, merge/1G1R/filter options, `--exclude-disks`). <https://github.com/emmercm/igir>
- Retool readme (unmaintained notice; 1G1R DAT preprocessing).
  <https://github.com/unexpectedpanda/retool>
- oxyROMon README (lossless-only originals + lossy export, `convert-roms`/`export-roms`/
  `check-roms`/`purge-roms`, Trash convention, CHD/RVZ settings, `chdman ≥ 0.264` for Dreamcast,
  optional native `nod` path). <https://github.com/alucryd/oxyromon>
- SabreTools README (Dir2DAT, DAT conversion/splitting, rebuild/verify, TorrentZip/TorrentGZ,
  Romba depot interop, CHD/Aaruformat hashes). <https://github.com/SabreTools/SabreTools>

### 20.4 This repository (CONCLUSION FROM SOURCE)

- `crates/archivefs-core/src/publisher_profile/execution.rs` (link modes, same-filesystem gate,
  typed refusals, transaction operations, executor scope).
- `crates/archivefs-core/src/publisher_profile/{model,planner}.rs` (action kinds, safety states,
  planner default and its rationale).
- `crates/archivefs-core/src/dat/rename_apply/model.rs` (`CreateHardlink`).
- `crates/archivefs-core/src/lib.rs` (`ArchiveKind::DirectGameImage`, `command_available`,
  `run_command_os_with_timeout` and its 30 s / 64 KiB limits).
- `crates/archivefs-core/src/game_identity.rs` (`.chd` → `IdentityImageFormat::Deferred`).
- `crates/archivefs-core/src/{smd_normalization,n64_byte_order}.rs` (existing lossless
  normalisations, both directions).
- `docs/research/CHD_VERIFICATION_IMPLEMENTATION_RESEARCH.md`,
  `docs/research/ARCHIVE_AWARE_DAT_VERIFICATION_RESEARCH.md`,
  `docs/research/PUBLISHER_PROFILES_PHASE1.md`, `README.md`, `ROADMAP.md`.

### 20.5 Environment checks performed on the research host

- `stat -f -c '%T'` → `ext2/ext3` for `/home/davedap` and `/tmp` (no reflink available here).
- `chdman --help` → `chdman - MAME Compressed Hunks of Data (CHD) manager 0.264`.
- Tool presence: `7z`, `zip`, `unzip`, `filefrag`, `xfs_io`, `btrfs`, `du` present;
  `dolphin-tool`, `maxcso`, `dattool` absent.

**Known limitation of this research:** no conversion and no reflink was executed against real user
data (this task is research-only), so every "expected class" in section 9.2 remains an expectation
to be tested rather than a verified result. Section 18 lists exactly what must be tested.

---

## 21. Validation record for this research task

- `git status --short` and `git diff --check` were run before committing.
- The only file added by this task is
  `docs/research/SPACE_EFFICIENT_STORAGE_AND_CONVERSION.md`.
- No Rust production code, Publisher Profile code, Save Vault code, GUI, or converter code was
  modified. No user file was converted, compressed, decompressed, moved, hardlinked, reflinked, or
  deleted.
- Implementation targets identified by this research (sections 15.1/15.2 and the phase boundaries in
  section 17) are explicitly **not** implemented here and must be raised as separate tasks.

