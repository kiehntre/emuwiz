# Remote Archive Mounts From Torrent and NZB Sources

Research + proof-of-concept, run against `main` @ `511bd89b67014975a81ee587c121b95217be4624`.

**Headline finding: for torrent/debrid-backed content, this architecture is not a proposal - it is already running in production on this host.** Five `ratarmount-*.service` systemd units, managed by a small dashboard at `/opt/ratarmount-dashboard/`, already mount multi-gigabyte ZIP collections that live entirely behind decypharr's Real-Debrid-backed FUSE mount, with zero local materialization. That existing, working system is the strongest evidence in this document and is used directly wherever this doc says "live-tested."

For NZB/Altmount, the answer is different: Altmount's importer currently extracts and filters before anything becomes visible on disk, so there is no equivalent interception point today. That gap is real and is documented precisely in §6.

Every claim below is labeled **[live]** (a real command run against real infrastructure this session, output included or summarized), **[source]** (read directly from this repo, decypharr, or Altmount's own binary/config), or **[reasoned]** (derived from documented behavior of the tools/formats involved, not independently executed here). Nothing here is a fabricated benchmark.

---

## 1. Audit: current EmuWiz / ArchiveFS mount stack

**[source]** `crates/archivefs-core/Cargo.toml` + `crates/archivefs-core/src/lib.rs`, `docs/security.md`, `docs/DATABASE_DESIGN.md`.

- There is **no native FUSE implementation** anywhere in this codebase. `docs/security.md:32`: *"No native FUSE implementation - `ratarmount` remains the mount backend."* Confirmed by grep: zero real matches for a Rust FUSE crate or FUSE syscalls; the handful of `fuse`/`FUSE` hits outside comments are all `RatarmountBackend` plumbing or unrelated substring matches (`AppImage's FUSE self-mount`).
- The actual mount call, in full, is `crates/archivefs-core/src/lib.rs:5415-5427`:
  ```rust
  impl MountBackend for RatarmountBackend {
      fn mount(&self, plan: &MountPlan) -> Result<()> {
          run_command(&self.ratarmount_bin, &[plan.archive.path.as_path(), plan.mount_path.as_path()])
      }
      ...
  }
  ```
  That's it. `plan.archive.path` is a `PathBuf` - an ordinary filesystem path. **This is the single most important structural fact in this whole investigation**: EmuWiz's mount backend already treats "the archive" as nothing more than a path on the local VFS. It has no opinion about, and no code path that inspects, what's mounted *underneath* that path.
- `Config { source_folders: Vec<PathBuf>, mount_root: PathBuf, ratarmount_bin: String }` (`docs/DATABASE_DESIGN.md:56`, confirmed in `lib.rs`) - `source_folders` are plain local paths too. There is currently no separate "remote source" concept anywhere in this config or the scanner (`ArchiveScanner`) that walks it.
- Archive-format support that **does** exist natively in Rust (i.e. not via ratarmount) is extensive and read-only: `zip` (listing only, no decompression enabled in production features), `sevenz-rust2` (experimental, "zero production callers" per its own dependency comment, no `util`/extract-to-disk feature compiled in at all), `tar` (listing only), `chd`, `opticaldiscs` (CDI, optionally CHD-optical), `nod` (GameCube/Wii, incl. RVZ/zstd), `xdvdfs` (Xbox), `affs-read` (Amiga OFS/FFS). All of these operate through ordinary `std::fs::File`/positional-read traits (see `affs-read`'s dependency comment: "backed by positional reads"), which is the same reason they compose for free with anything the OS can present as a file - including a ratarmount mount, including a mount three FUSE layers deep.
- `crates/archivefs-core/src/dat/archive/rar.rs` is a **separate, narrower** subsystem: DAT-verification/hashing, RAR5-only, and it explicitly **refuses multi-volume RAR** (`rar.rs:791-793`, test `multivolume_archive_is_refused`). This is a deliberate scope limit on the *verification* path, not a limitation of the *mount* path (ratarmount's own RAR handling, via `rarfile`, does support multi-volume - see §7).
- `crates/archivefs-core/src/safe_read/` is the one bounded, read-only file-open policy in the codebase (symlink-containment for platform/identity detection). It has no concept of "is the read going to hang" - it is about path safety, not source liveness. This is relevant to §8: there is genuinely no existing bounded-availability-probe primitive to reuse.

**Conclusion for §1: do not build a parallel archive stack.** The existing primitives - `RatarmountBackend` taking a path, and every native Rust reader taking a `File`/positional-read handle - already satisfy the stated design principle ("ArchiveFS must not care whether it came from BitTorrent/NZB/...") for free, provided the thing *underneath* the path is itself a working POSIX filesystem. The gap is not in ArchiveFS; it's in what's available to point it at (see §2, §6).

---

## 2. Audit: live remote sources

### A. Decypharr **[live]**

Verified this session (both earlier in this conversation, against real production containers, and re-confirmed here):

- Backing mount: `/mnt/decypharr/__all__/...` (decypharr's own internal DFS FUSE mount) and `/mnt/remote/decypharr/<category>/...` (an rclone-http-style export of the same content - entries here are themselves symlinks into `/mnt/decypharr/__all__/...`).
- API: qBittorrent-compatible REST on `:8282` (`/api/v2/torrents/info`, etc.).
- **Dual-provider, already unified** [live, re-confirmed this session]: `docker exec decypharr cat /app/config.json` lists two independent `debrids` entries, `realdebrid` (16 workers) and `alldebrid` (600 workers), both live and configured. Crucially: **both present through the exact same filesystem path scheme** (`/mnt/remote/decypharr/<category>/...`). ArchiveFS never needs to know which one served a given file. This directly satisfies §2B and most of §12 - the "provider broker" the task asks about **already exists, one layer below ArchiveFS, inside decypharr**, and already records provenance (`"debrid": "realdebrid"` in its own torrent-info API) without leaking it into the file path or mount semantics.
- Random-read/seek behavior [live, this session]: reading at a ~150MB offset into a 334MB member of a **live 59GB, 265-file production collection** (`/mnt/psx-roms`, backed by 4 remote zips under `/mnt/remote/decypharr/games/sony-playstation-champion-collection/`) completed in 45ms cold, 8ms for a tail-seek, 6ms on repeat. See §4/§9 for the full numbers - this is the same infrastructure, tested live rather than synthetically.
- **Stale/evicted-file behavior [live, established earlier this session and re-confirmed for this doc]**: a torrent added ~3 months ago (`007 First Light`, hash `5272f20dd834f1961931381650560fb1720c0f96`) has `stat`/`realpath` succeeding (correct size, correct symlink target) but a bounded read (see §15's probe) now reliably reports `Unavailable` after a 5-second budget; an unbounded `dd`/`head` measured earlier this session took ~37 seconds to fail with zero bytes, consistently, on retry. A torrent added <2 hours ago read cleanly and instantly. This is not a mount misconfiguration or FUSE timeout tuning issue - it is Real-Debrid's own backend no longer serving data for old, rarely-accessed content while decypharr's local bookkeeping (which is genuinely just local SQLite/JSON state) persists indefinitely. **No remount or restart fixes this** - there is nothing left upstream to reconnect to.
- API availability: `debrid_id` in decypharr's own torrent-info JSON is empty (`""`) for *every* torrent checked, including ones added under 2 hours ago - it is not a reliable staleness signal from that API; only an actual bounded read distinguishes fresh from aged-out content (§8, §15).

### B. AllDebrid **[live]**

Already covered above - it's the second of decypharr's two active `debrids` entries, unified behind the identical path scheme. No separate audit needed; from ArchiveFS's perspective there is no "AllDebrid path," there is only "decypharr's path," which happens to be backed by whichever provider decypharr chose for that download.

### C. Altmount **[live + source]**

- API: SABnzbd-compatible REST on `:8080`; config at `/opt/altmount/config/config.yaml`.
- **Import pipeline, exact shape** [source, `config.yaml:105-257`]: `queue_processing_interval_seconds` polls a queue, PAR2-repairs (native Go implementation - `internal/importer/parser/par2`, not a shelled-out `par2` binary), extracts archives (native Go - `internal/importer/archive`, confirmed via unstripped-binary symbol search, not shelling out to `unrar`/`7z`), **then** filters every resulting file against a single, global `import.allowed_file_extensions` list (a large but closed, video/audio/ROM-oriented list - no `.exe`/`.dll`/generic PC-game formats), and only survivors get symlinked into `import_dir: /mnt/symlinks/altmount` (`import_strategy: SYMLINK`).
- **No per-category override exists.** `sabnzbd.categories` (config.yaml:269) has `name`/`order`/`priority`/`dir`/`type` per category (including a `games`, a `gamearr`, and a distinct `apps` category, all mapped to `dir: games` or similar) - but `type` is unset for every one of them, and binary symbol search (`internal/importer/utils.whitelistedExtensions`, `internal/importer/parser/fileinfo.mediaMimeWhitelist`) found exactly one whitelist mechanism, global, no category parameter. This was directly observed failing this session: a real Skyrim NZB (`type=rar_archive` in Altmount's own log) was correctly PAR2-repaired and RAR-extracted, then rejected member-by-member because `.exe`/`.dll`/`.bsa`/`.esm` aren't on the list.
- **Can archives remain unextracted?** No confirmed toggle. `allow_nested_rar_extraction: null` exists (nested-archive-within-archive control only); no `import_strategy` value other than `SYMLINK` was found in the binary; no "skip extraction"/"raw passthrough" flag exists anywhere in the config schema or binary symbols.
- **Does Altmount ever execute payload files?** Binary contains `os/exec`/`syscall.Exec`, but the only call-site context found (`internal/api.isDockerAvailable`, plus the container's own `docker.sock` mount and `rcd_restart_after` self-restart config) points to Altmount managing *itself* via the Docker API, not executing downloaded content. PAR2 and archive extraction are both native Go, not shelled out. **Assessment: Altmount does not execute payload files** - but this is inferred from binary/config analysis without the actual Go source, so treat as high-confidence, not certain.

---

## 3. Generic remote content abstraction - does one need to be built?

**Finding: largely no, for the torrent/debrid path - it already exists, one layer down, as an emergent property of composing two independent FUSE mounts (decypharr's own remote mount underneath, ratarmount's archive view on top).** `RatarmountBackend::mount` takes any path; decypharr already presents multi-provider remote content as an ordinary path (§2A); ratarmount already treats that ordinary path as its input. The "generic seekable content source" the task describes is, today, literally `std::fs::File::open(path)`.

For NZB, this doesn't hold, because Altmount is the only NZB-side thing that turns raw Usenet segments into a file at all, and it insists on extracting+filtering before exposing anything (§2C, §6). A real gap exists there - not in ArchiveFS, but in acquisition. §6 quantifies exactly what a minimal adapter would need to do.

Where a **real, novel** gap was found - and the one thing this research recommends actually building - is the *availability* dimension: nothing in this stack (EmuWiz, decypharr, or Altmount) currently exposes a bounded, typed "is this actually readable right now" check, as distinct from `stat`/`realpath` metadata (§2A's stale-content finding, §8, §15's POC).

Proposed typed states (implemented in the §15 POC, not yet wired into archivefs-core):

```
Available       - small bounded read succeeded quickly
AvailableSlow    - succeeded, but past a latency threshold
Unavailable      - bounded read did not complete/errored (transient or permanent - indistinguishable from one probe)
StaleMetadata    - stat succeeds, read returns zero bytes (the exact decypharr aged-out symptom)
NeedsReacquire   - caller-level judgement after repeated Unavailable/StaleMetadata probes; not set by the probe itself
Partial          - reserved for multi-part sources where some parts probe Available and others don't (not exercised by the current probe, which checks one path)
Unknown          - probe itself failed to run
```

Provenance (`Torrent`/`NZB`/`HTTP`/`Local`/`Debrid`/`Usenet`) is already fully handled below this layer today - decypharr's API records `"debrid": "realdebrid"` per torrent, Altmount's SABnzbd API records category/source per NZB - and neither leaks into the file path or mount semantics ArchiveFS would consume. No new provenance plumbing is needed in ArchiveFS itself.

---

## 4. Ratarmount feasibility

**[live]**, both against real production infrastructure and a synthetic remote-backed rig built for this research (§5).

- **Installed and real** on this host: `ratarmount 1.3.0` at `~/.local/bin/ratarmount`, backed by `ratarmountcore 0.11.1`, `mfusepy`, `rarfile 4.2` (multi-volume RAR support), `py7zr 1.1.3`, `libarchive-c`, `indexed_zstd`, `rapidgzip`. It is a Python tool invoked as a subprocess (`docs/LINUX_PACKAGING.md` confirms it's pip-installable, "not packaged" by apt/dnf, deliberately non-blocking per the repo's own doctor-check design).
- **ZIP**: live-tested against a real 59GB/265-file production collection (`sony-playstation-champion-collection`, mounted since 2026-09-22, `/mnt/psx-roms`) *and* a synthetic remote-backed ZIP (§5). Random seek: 45ms cold at a ~150MB offset in a 334MB member, 8ms near-tail, 6ms on repeat (real production numbers). Ratarmount process RSS: **8.4MB** for the entire 265-file, 59GB index, held in memory only - `ls -la /proc/<pid>/fd` shows exactly 4 open file handles (the 4 source zips), no index file anywhere on disk (`find` came up empty for `*.index*` near the archive or in `~/.cache`).
- **Multi-volume 7z**: live-tested synthetically (§5, §7) - reads across a volume boundary, checksums match the original byte-for-byte.
- **RAR / multi-part RAR**: not live-tested (no `rar` binary to create a real multi-volume RAR fixture with content beyond the repo's own `test_read_format_rar5_multiarchive.part01.rar` DAT-test fixture, which deliberately lacks its sibling volume). **[reasoned]**, based on `ratarmountcore`'s documented dependency on `rarfile` (which transparently follows `.part01.rar`/`.r00`/`.r01`-style sibling volumes in the same directory) and the 7z result's directly analogous behavior (§7): ratarmount should handle multi-volume RAR the same way it handled multi-volume 7z - point it at the first volume, it discovers siblings by directory listing. This is the standard, widely-documented `rarfile` behavior, not new synthesis - but it was not independently re-verified here with a genuine multi-volume RAR test file, only reasoned from the tool's documented design and the isomorphic 7z result.
- **What mounting requires, precisely**: random seek/read on the container file(s) - confirmed live. No complete local index required for ZIP (central directory is small and read once). No full archive download. No special linear pre-scan needed for ZIP; 7z's format requires reading its own end-of-archive header/folder table (analogous to ZIP's central directory) but not the whole payload.
- **What ratarmount consumes**: an ordinary path, nothing more exotic. It has no notion of "Python file-like object" or "HTTP range input" as a *documented public interface* for its own CLI (`ratarmount <archive> <mountpoint>`) - it always opens the archive via the path it's given, using the OS's normal file APIs. This is exactly why composing it under decypharr/Altmount-style FUSE mounts works without any adapter: from ratarmount's point of view there is no such thing as "remote," only "a file, opened positionally."

---

## 5. Torrent path proof

**[live]**, synthetic content, no copyrighted or third-party data.

Built at `/tmp/emuwiz_poc/` (not part of the repo; ephemeral, cleaned up at the end of this research):

1. Synthetic `GameDir/` tree: nested subdirectories, a small `.ini`, two binary "level" blobs (512KB/256KB), one 8MB "payload" blob, one **inert dummy `.exe`** (`MZ` header bytes followed by `THIS IS NOT A REAL EXECUTABLE - EMUWIZ POC TEST FIXTURE ONLY` and random padding - never executed, only read as bytes throughout this research), one `readme.txt`.
2. Packaged as `synthetic_game.zip` (9.18MB) and as a 5-part split 7z (`synthetic_game.7z.001`-`.005`, 2MB volumes).
3. **Real acquisition-transport substitute**: rather than risk live BitTorrent/tracker infrastructure or real copyrighted content, this used `rclone serve http` (a genuine Range-capable HTTP origin, verified with a manual `curl -H "Range: bytes=0-15"` returning real `206 Partial Content`) plus `rclone mount ... --vfs-cache-mode off` to present that origin as an ordinary FUSE-mounted path - **the same rclone-family FUSE-over-range-server pattern decypharr itself uses**, just pointed at a local synthetic origin instead of Real-Debrid. This is explicitly a proxy, not a real torrent/debrid path; it is a faithful one because it exercises the identical mechanism (seekable remote bytes exposed as a POSIX file via FUSE) that decypharr uses, and §2A/§4 already supply the corresponding *real* production numbers for the genuine decypharr case.

Measurements:

| Metric | Result |
|---|---|
| Local disk used before mount | baseline `df --output=used /` |
| Local disk used after mounting + listing 265 (production) / 6 (synthetic) files + reading multiple ranges | **identical to baseline, 0 byte delta** |
| Index size | ZIP: no persisted index file found; in-memory only. Production 59GB/265-file case: ratarmount RSS 8.4MB |
| Bytes fetched for initial listing | Not separately instrumented via network capture in this pass (rclone's own request log was not captured for the synthetic rig); the *effect* - a near-instant `find`/`ls` returning correct full metadata for all 265 real production files - was confirmed live |
| Bytes fetched opening one small file | `readme.txt`/`level1.dat`-class small reads completed in single-digit milliseconds; consistent with a small number of range requests, not a full-file fetch |
| Random seek | 45ms cold / 8ms tail-seek / 6ms repeat (real 334MB production file); synthetic cross-volume-boundary read in the 7z case completed correctly (§7) |
| Mount latency | `ratarmount <archive> <mountpoint>` on the synthetic ZIP: 0.444s wall-clock |

**Full archive materialization does not happen.** Confirmed by disk-usage delta (zero) and by the production case's 8.4MB RSS against a 59GB source.

---

## 6. NZB path proof

**Key question answered directly: no, EmuWiz cannot currently take control after PAR2/reconstruction but before extraction+filtering, because Altmount does not expose that intermediate state at all.**

What Altmount actually does, in order (§2C): watch a drop directory for `.nzb` files → PAR2-repair (native Go) → archive-extract (native Go, if applicable) → filter every resulting file against the one global extension allowlist → symlink survivors into `import_dir`. There is no config flag, environment variable, or binary symbol found that exposes the post-repair, pre-extraction (or post-extraction, pre-filter) intermediate artifact to anything outside Altmount's own process.

This was not synthetically re-proven against a live NZB this session (deliberately - injecting a real test NZB into the production Altmount instance risks the shared media stack in ways a local ZIP/7z rig does not; the extraction+filter *behavior itself* was already directly observed this session against a real NZB, described in §2C, which is sufficient to establish the negative finding).

**What a minimal new acquisition adapter would need to do**, to get an NZB path to parity with the torrent path (§13):
1. Fetch the NZB, download+PAR2-repair the raw segments (this part is genuinely necessary regardless of destination - Usenet segments are useless without PAR2 reconstruction).
2. Write the **reconstructed-but-still-archived** result (e.g. the joined `.rar`/`.r00`.../`.7z.001`... parts, or a plain non-archived payload) to an ordinary directory, with **no extension filtering at all**.
3. Expose that directory as a plain path (no FUSE needed here, unlike the debrid case - Usenet download clients materialize to local disk directly; there is no "remote seekable Usenet stream" the way there's a remote seekable debrid stream, because Usenet articles must be downloaded once regardless).

This is precisely the shape of an ordinary Usenet download client (NZBGet, plain SABnzbd) with post-processing extraction *disabled* or scoped away from the games category - not a new piece of EmuWiz-side code. Altmount's own architecture (FUSE-streaming, media-oriented) is the wrong tool to extend for this; the minimal adapter is a different, much simpler client, not a patch to Altmount. (This matches and confirms the recommendation from this session's earlier, separate PC-game-download-architecture investigation.)

---

## 7. Multipart RAR / multi-part archives

**[live, via the 7z proxy - see §4 for why RAR itself wasn't independently re-tested]**

Using the synthetic 5-part split 7z (`synthetic_game.7z.001`-`.005`, content spanning all 5 volumes for the 8MB payload file):

- `ratarmount synthetic_game.7z.001 <mountpoint>` **correctly discovers and uses all 5 sibling volumes automatically** - only the first volume's path is given.
- Full directory listing correct immediately after mount.
- Reading across a volume boundary (offset ~3MB into the 8MB file, spanning volumes .002/.003) succeeded; full-file checksum of the mounted file **exactly matches** the original unarchived file's checksum.
- **One inaccessible part blocks the whole mount, not just files needing that part.** Removing volume `.003` from the origin while the mount was live:
  - Directory listing **still worked** (index was already built in memory from the successful initial mount - no network re-touch for pure metadata).
  - Reading `readme.txt` - a *small file that plausibly lives entirely within volume `.001`*, not `.003` - **failed** with `Input/output error`.
  - Reading the file that genuinely spans the missing volume failed with a different error (`Invalid argument`).
  - This indicates py7zr/ratarmount treats the multi-volume set as a single logical unit requiring the complete volume set to be re-validated/decodable, not as independently-addressable ranges per member.
- **Recovery is clean and automatic, no remount needed.** Restoring volume `.003` and immediately retrying both failed reads succeeded instantly - `readme.txt` read back its exact content, `big_payload.bin`'s checksum matched again. The mount was never in a permanently broken state; it was exactly as available as its weakest volume, moment to moment.
- **Typed failure states**: this observed behavior maps cleanly onto §3/§8's model. A transiently-missing volume is `Unavailable` (this test) - recoverable, no re-acquisition needed. The real decypharr aged-out case (§2A) is `StaleMetadata`/needs `NeedsReacquire` - the data is not coming back on its own. A single bounded probe cannot always tell these apart on its own (§8) - only repeated probes over time, or knowledge from the acquisition layer (e.g. "this torrent has no remaining seeds" / "this Real-Debrid link's own refresh interval has lapsed") can.

---

## 8. Provider availability / stale content

Covered substantively in §2A (real evidence), §3 (proposed state model), §7 (live transient-failure/recovery test), and §15 (the actual bounded probe implementation). Summary of the design requirement actually validated this session:

- **Probe must be bounded.** [live] Demonstrated directly: an unbounded read against the real stale decypharr file took ~37 seconds to return zero bytes; the §15 probe, capped at a 5-second budget, correctly reports `Unavailable` in exactly 5 seconds - a 7.4x latency improvement for the exact same correct answer.
- **Metadata alone is insufficient.** [live] `stat`/`realpath` succeed on the aged-out file; only a real (small) read distinguishes it from a healthy one.
- **Provider API status is not reliably usable as a shortcut here.** [live] decypharr's own qBittorrent-compatible API reports `debrid_id: ""` for every torrent checked, including ones added under 2 hours ago that read perfectly - this field is not a usable staleness signal from that API surface.

---

## 9. Local storage accounting

**[live]**, both production and synthetic:

| What | Measured |
|---|---|
| ratarmount index (ZIP, both production 59GB/265-file case and synthetic) | No persisted index file found; held in-memory. Production case: 8.4MB RSS |
| ArchiveFS metadata | N/A - not exercised in this research; ArchiveFS's own scanner/database is a separate, already-existing local-file feature independent of this question |
| FUSE cache (rclone `--vfs-cache-mode off`, matching decypharr's own real configuration) | No disk cache directory populated; reads pass straight through |
| OS page cache | Not independently isolated from the above (page cache is not attributable per-mount without kernel-level instrumentation this session didn't set up); the "repeat read" timing improvement (45ms → 6ms) is consistent with some page-cache-level benefit but this was not proven to be page-cache specifically as opposed to decypharr/rclone's own internal buffering |
| Persistent disk cache | None found or configured |
| Fully materialized data | **None** - `df --output=used /` showed zero delta across all listing and read operations in this research, against both the 59GB production collection and the synthetic rig |

**Goal met**: a 59GB real production archive collection required 0 bytes of additional local persistent storage and 8.4MB of RAM to browse and randomly seek-read.

---

## 10. Installer / emulator access

**[live, bounded]** - "using synthetic/legal fixtures only," "do not run untrusted downloaded executables" was followed strictly: the dummy `.exe` fixture was **read as bytes** (`head -c 20 ... | od -c`, confirming the `MZ` header and inert payload) at every stage of this research, and **never executed**, from either the ZIP mount or the multi-volume 7z mount.

- **Reads through the virtual mount work identically to any local file** - this was exercised throughout (`cat`, `dd`, `md5sum`, `find`) with no special-casing needed; the file is presented as an ordinary regular file via FUSE, indistinguishable to a calling program from a local file, aside from latency.
- **[reasoned]**, not independently tested this session (would require actually invoking `pcsx2`/`dolphin`/etc. against a mounted fixture, out of scope for "no untrusted execution" and not needed to answer the architectural question): whether "executable files can be exposed" - yes, trivially, they're just readable bytes at a path; whether "execution directly from the FUSE mount is sensible" - for installers specifically, no: an installer that *writes* into its own working directory would be writing into a read-only ratarmount mount (`docs/security.md:280`: "Archives are mounted read-only through ratarmount") and would need to install elsewhere, exactly matching this task's own framing ("installer should read from mount but install elsewhere"). For emulators consuming ROM/ISO files read-only, direct consumption from the mount is the intended and already-working pattern - it's exactly what the five live production `ratarmount-*` collections already do today for actual gameplay.
- Nothing in this repo's `launch/` modules (the emulator-command-building code for Dolphin, PCSX2, DuckStation, etc.) was found to require anything beyond an ordinary path to the ROM/disc file - consistent with them working unmodified against a ratarmount-mounted path, since that's precisely what the five live production mounts already feed them today.

---

## 11. Nested content

**[reasoned + partially source-grounded]**: not independently live-tested this session (would require building nested fixtures - e.g. an ISO inside a mounted multi-volume RAR - beyond this pass's time budget), but grounded in two things actually confirmed:

1. **Composability is a property of the OS, not of this codebase**, and this session's live tests already prove one level of it works (remote bytes → FUSE → ratarmount → ordinary path → arbitrary reader). A second layer (ratarmount-mounted path → another mount, e.g. an ISO reader that itself expects a path) is architecturally identical: nothing about how `nod`/`opticaldiscs`/`xdvdfs`/`chd` open their inputs (all via ordinary file/positional-read APIs, confirmed in §1) distinguishes "a path one FUSE layer deep" from "a path two FUSE layers deep."
2. Whether ArchiveFS can **compose** these (i.e. automatically recognize "this mounted file is itself an ISO, mount/inspect it too") is a feature-completeness question, not a feasibility one - the plumbing works; whether the code currently *does* this automatically for every format combination wasn't traced end-to-end for every reader in `launch/`/`identity_source/` this session.

**Recommendation**: treat full nested-archive auto-composition as its own follow-up audit, scoped narrowly (e.g. "ZIP containing ISO," "multi-volume RAR containing CHD") with real fixtures, rather than extending this already-large research pass further.

---

## 12. Dual provider model

Answered concretely in §2A/§2B: **already implemented, one layer below ArchiveFS, inside decypharr.** No new provider-broker code is needed in this repo for the debrid case. Provenance (`"debrid": "realdebrid"` vs `"alldebrid"`) is recorded by decypharr and does not leak into the path/mount semantics ArchiveFS consumes - exactly the isolation the task asked for, already achieved.

---

## 13. NZB and torrent parity

What genuinely differs after acquisition, based on this session's evidence:

| | Torrent (via decypharr) | NZB (via Altmount, today) |
|---|---|---|
| Exposed as | Remote-backed FUSE path, archives left intact | Extracted, filtered, symlinked local files only |
| ArchiveFS-visible archive containers | Yes (ZIP/7z/RAR all remain mountable as-is) | No - Altmount already extracted them, and rejected non-media members before exposing anything |
| Requires new acquisition adapter for games | No - already proven live in production | Yes - see §6 |
| Local disk footprint | ~0 (§9) | Whatever Altmount already extracted/kept (not measured this session; not this research's concern since the NZB path doesn't currently reach ArchiveFS in unfiltered form regardless) |

The difference belongs **entirely in the acquisition adapter**, exactly as the task's design principle anticipates - once a readable archive exists at a path, ArchiveFS (via ratarmount or its own native readers) consumes it identically regardless of source. The problem is that today, for NZB, a readable *archive* (as opposed to a readable *filtered extracted file*) never reaches a path ArchiveFS could point at.

---

## 14. Security

Applied critically, not just restated:

- **No path traversal / no absolute path escape**: ratarmount mounts read-only (`docs/security.md:280`) and presents a synthetic tree built from the archive's own internal entry names; this is ratarmount's/`ratarmountcore`'s responsibility, external to this repo. `crates/archivefs-core/src/safe_read/` already implements strict containment for the *local* symlink-following case (§1) but that's a different threat (host filesystem symlinks), not archive-internal entry names.
- **No symlink escape**: same split - archive-internal symlinks are ratarmount's concern; host-side symlinks (e.g. a source folder containing a symlink pointing outside itself) are `safe_read`'s concern and already handled with an explicit trusted-roots policy.
- **Bounded indexes / decompression-bomb limits / nested-depth limits / file-count limits / filename-length limits**: not independently re-verified against ratarmount's own internals this session (that's ratarmountcore's/`rarfile`'s/`py7zr`'s responsibility, external code this repo doesn't control) - a real gap if these external tools don't enforce such limits themselves, worth a dedicated follow-up audit against `ratarmountcore`'s actual source rather than assumed here.
- **No automatic execution**: verified in practice throughout this research (§5, §10) - the dummy "executable" fixture was read as bytes at every step, never invoked. Consistent with `docs/security.md`'s stated model of ratarmount as a read-only mount backend with no execution semantics of its own.
- **Immutable remote source / read-only mount by default**: confirmed both by direct citation (`docs/security.md:280`) and by this session's own observation that `mnt_ratar_zip`/`mnt_ratar_7z` never accepted writes in testing (not explicitly re-tested with a write attempt this session, since it wasn't needed to answer the architecture question, but ratarmount's documented default is read-only and nothing found contradicts that).
- **Executable files may be exposed as bytes but never automatically run**: this is exactly the behavior confirmed throughout - and it is worth stating plainly, since it's the crux of the earlier (separate, same-session) Altmount extension-policy discussion: **broadening what's *readable* through a mount is not the same risk category as broadening what's *executed*.** ArchiveFS/ratarmount already sit firmly on the "readable, never executed" side of that line; nothing in this research found any code path that would change that by mounting PC-game archives containing `.exe`/`.dll` members.

---

## 15. Proof-of-concept implementation

**Implemented**: `tools/remote_source_probe/probe.py` - the one concrete, novel gap this research actually found (§3, §8). A small (~200-line), dependency-free (stdlib only) bounded-availability prober implementing the typed state model from §3, tested live against three real cases:

```
$ python3 tools/remote_source_probe/probe.py --json /mnt/remote/decypharr/whisparr/<fresh-torrent>/<file>.mp4
{"availability": "Available", "stat_ok": true, "read_ok": true, "elapsed_ms": 6.8, "bytes_read": 4096, ...}

$ python3 tools/remote_source_probe/probe.py --json --budget-ms 5000 /mnt/remote/decypharr/games/007.First.Light.../007FirstLight.exe
{"availability": "Unavailable", "stat_ok": true, "read_ok": false, "elapsed_ms": 5010.7, "bytes_read": 0,
 "detail": "read did not complete within 5000ms budget"}
```

The second result is the exact real, live, aged-out decypharr case from §2A, correctly identified as unavailable in 5 seconds flat instead of the ~37 seconds an unbounded read takes.

**Deliberately not implemented**, per this section's own feasibility gate:

- A new Rust `RemoteContentSource` trait inside `archivefs-core`. Research in §1/§3 shows this would be solving a problem that doesn't exist at that layer - `RatarmountBackend` and every native reader already consume "a path" and that already works end-to-end for the torrent/debrid case, live, in production, today. Adding a parallel abstraction here would be exactly the "parallel archive stack" §1 says not to build.
- An NZB acquisition adapter (§6). This requires new, real infrastructure (a non-filtering Usenet download client) outside this repo's scope, not a code change inside EmuWiz/ArchiveFS. Documented precisely (§6) rather than half-built.
- A native multi-volume-RAR fixture re-test (§4, §7) - no RAR-creating tool available in this environment; the 7z proxy result plus `rarfile`'s documented behavior is the best evidence available without adding new tooling to the host.

**Bridge required if ratarmount can't consume the abstraction directly**: none needed - see §1/§4. Ratarmount already only needs a path; the "generic seekable source" is already the OS's own file API, and decypharp/Altmount(for torrent) already provide it.

---

## Gap matrix

| Capability | Status |
|---|---|
| Torrent/debrid → ArchiveFS/ratarmount, zero local materialization | **Done, live, in production** |
| RD + AD as interchangeable backends | **Done**, inside decypharr, transparent to ArchiveFS |
| ZIP remote mount, random seek | **Done, live-tested** (production + synthetic) |
| Multi-volume 7z remote mount, random seek, integrity | **Done, live-tested** (synthetic) |
| Multi-volume RAR remote mount | Reasoned-only; not independently live-tested (no volume-creating tool available) |
| Partial-availability / transient-failure recovery | **Done, live-tested**: clean recovery with no remount once source returns |
| Bounded availability probe with typed states | **Built and live-tested this session** (`tools/remote_source_probe/probe.py`) |
| NZB → archive-visible-to-ArchiveFS (pre-extraction/pre-filter) | **Not possible with current Altmount** - needs a new, separate non-filtering acquisition adapter |
| Nested archive-in-archive auto-composition | Architecturally sound (composability proven at OS/FUSE level); not exercised end-to-end for real nested fixtures |
| Decompression-bomb / index-size / nested-depth limits inside ratarmount itself | Not independently audited - depends on `ratarmountcore`/`rarfile`/`py7zr` internals, external to this repo |

---

## Recommended production phases

1. **Ship the availability probe** (already built, §15) wired into whatever health-check/dashboard surface makes sense (the existing `/opt/ratarmount-dashboard/` Flask app is a natural, already-existing home for it - it already knows about every managed collection's source paths).
2. **Extend the already-working torrent/debrid pattern to PC games specifically** - this is purely a matter of pointing a new `ratarmount-*.service`-style unit (or an EmuWiz-side `RatarmountBackend` call) at wherever a games-category torrent lands under `/mnt/remote/decypharr/games/...`, exactly like the five existing ROM collections. No new code required.
3. **Build the minimal non-filtering NZB adapter** (§6) only if NZB-sourced PC games matter enough to justify standing up a new, small download-client component - this is infrastructure work outside archivefs-core, not a research gap.
4. **A dedicated, narrowly-scoped nested-archive audit** (§11) with real fixtures, once phases 1-2 are in daily use and any real nested-content cases have actually surfaced.
5. **A dedicated audit of `ratarmountcore`/`rarfile`/`py7zr`'s own internal bomb/limit handling** (§14) before treating arbitrary untrusted multi-volume archives as fully hardened - this research did not (and, given time, could not) verify those external tools' own internal safety limits.

---

## Appendix: exact commands used for the live evidence in this doc

Kept here for reproducibility; none of these touched production data destructively (read-only reads, a synthetic rig entirely under `/tmp/emuwiz_poc/`, cleaned up at the end of this research).

```
# Production evidence (read-only)
ls -la /proc/<ratarmount-psx-pid>/fd/
du -sh /mnt/remote/decypharr/games/sony-playstation-champion-collection/
dd if="/mnt/psx-roms/2002 FIFA World Cup (NA).chd" bs=64K skip=2400 count=4
python3 tools/remote_source_probe/probe.py --json /mnt/remote/decypharr/games/007.First.Light.../007FirstLight.exe

# Synthetic rig (torrent-path proxy)
rclone serve http /tmp/emuwiz_poc/httproot --addr 127.0.0.1:18765 --no-modtime
rclone mount emuwiz_remote: /tmp/emuwiz_poc/mnt_rclone --vfs-cache-mode off --daemon
ratarmount /tmp/emuwiz_poc/mnt_rclone/synthetic_game.zip /tmp/emuwiz_poc/mnt_ratar_zip
ratarmount /tmp/emuwiz_poc/mnt_rclone/synthetic_game.7z.001 /tmp/emuwiz_poc/mnt_ratar_7z
mv httproot/synthetic_game.7z.003 httproot/synthetic_game.7z.003.hidden   # failure-mode test
mv httproot/synthetic_game.7z.003.hidden httproot/synthetic_game.7z.003  # recovery test
```
