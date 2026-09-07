# Safe Launch Sandbox + Scratch-Media Primitive (V1 Design)

**Status: design only. No production code lands in this pass — see "Implementation decision"
below for why, and what would unblock implementation.**

## Threat model

**Core principle (verbatim from the task, and now the standing rule this primitive exists to
enforce): user source media is authoritative and must remain unmodified.**

Four independent, already-committed native-adapter audits in this repository reached the same
conclusion from four unrelated emulator families, each with its own upstream CLI, none of which
documents a proven read-only media switch:

- **NP2kai / PC-98** (`docs/research/PC98_NP2KAI_NATIVE_ADAPTER_AUDIT.md`): "does not document a
  standalone, command-line write-protect mode for D88, HDI, or NHD... a future adapter must use a
  verified scratch-copy lifecycle." D88/HDI both classified `PROFILE_REQUIRED` with the same note:
  "future adapter must scratch-copy or prove write protection." BIOS/config/CMOS/NVRAM state also
  called out as needing isolation in the same session root.
- **BBC Micro / b-em** (`docs/research/BBC_MICRO_NATIVE_ADAPTER_AUDIT.md`): "b-em's own README
  documents no explicit read-only/write-protect CLI flag... a future adapter must use a mandatory
  scratch-copy strategy," with a private, adapter-owned scratch location and discard-after-exit as
  the recommended mitigation.
- **Amstrad CPC / Caprice32** (`docs/research/AMSTRAD_CPC_NATIVE_ADAPTER_AUDIT.md`): "does not
  establish a command-line read-only switch"; the audit's own required V1 shape lists "scratch-copy
  protection for writable media" and "source identity and scratch binding" as prerequisites, not
  optional extras.
- **X68000 / PX68k** (`docs/research/X68000_NATIVE_ADAPTER_AUDIT.md`): "No safe read-only guarantee
  was established for guest writes... launch a scratch copy for media that is not demonstrably
  read-only."
- **Atari800**: referenced in this task's own brief as background — CAS has documented read-only
  behavior, but ATR/XFD/ATX were deliberately deferred because safe write protection was not
  proven. (No separate `docs/research/ATARI800_*` audit exists yet in this tree at the time of this
  design; the point stands regardless, since it is the same shape of problem.)

Five audits, five emulator families, one unresolved dependency each time. This is not an
emulator-specific edge case — it is a missing EmuWiz preservation primitive, and every one of
those audits explicitly deferred implementation pending it (PC-98's audit says this outright:
"NP2kai should follow completion of a reusable scratch-media/config-isolation primitive... It
should not wait" for anything else).

**The failure mode being defended against:** an emulator process, given a path directly into the
user's authoritative media/config, silently rewrites a sector, updates a CMOS/NVRAM image, or
touches a "last used" list — corrupting or drifting media that may be the only surviving copy of
preservation-relevant content, with no warning and no way back.

**What this primitive guarantees when followed:** the path any adapter hands to an emulator's argv
is never the source file. It is a private, uniquely-named, bounded-lifetime copy that the emulator
is free to mutate, and that EmuWiz discards on exit. The source is opened read-only (never
requiring write permission), never hardlinked as writable scratch, never chmod'd, never
symlinked-back, and never merged with what the emulator wrote. If any step of preparing that copy
fails, **the launch fails closed** — there is no fallback path that hands the emulator the real
file.

## Existing behavior (Task A — inventory)

`rg` across `crates/archivefs-core/src/launch/*.rs` for `scratch|tempdir|readonly|sandbox` found
**zero production scratch-media logic**. Every `tempdir()`/`TempDir::new()` hit in that search is a
**unit-test fixture** (`vice_execution.rs`, `sameboy_execution.rs`, `mame_execution.rs`,
`desmume_profile.rs`, `amiberry_cd_discovery.rs`, `melonds_execution.rs`, `mesen_execution.rs`,
`snes9x_execution.rs`, `amiga_whdload_execution.rs`, `amiberry_execution.rs`), never something an
adapter constructs at real launch time. No `docs/` file documents a scratch-media contract prior to
the four adapter audits cited above, which all recommend one without one existing yet.

Representative adapters, from reading their `_execution.rs`/`_command.rs` files and their own doc
comments:

| Adapter | Launches original media directly? | Proven read-only switch? | Temp copy today? | Config isolated? | Can modify source? | Can modify config/CMOS/state? |
|---|---|---|---|---|---|---|
| PCSX2 | Yes (typed path straight from the verified plan) | Not modeled | No | No | Plausible (memory cards, BIOS settings) | Yes |
| Dolphin | Yes | Not modeled | No | No | Plausible (save states, SD card images) | Yes |
| DuckStation | Yes | Not modeled | No | No | Plausible (memory cards) | Yes |
| RPCS3 | Yes | Not modeled | No | No | Plausible (save data, firmware cache) | Yes |
| Amiberry | Yes | Not modeled | No (production); only test fixtures | No | Plausible (ADF write-back is a known Amiga emulator behavior) | Yes |
| FS-UAE | Yes | Not modeled | No | No | Plausible | Yes |
| VICE | Yes | Not modeled | No | No | Plausible (D64 write-back is a known VICE behavior) | Yes |
| Hatari | Yes | Not modeled | No | No | Plausible | Yes |
| openMSX | Yes | Not modeled | No | No | Plausible | Yes |
| Fuse | Yes | Not modeled | No | No | Plausible (TAP/DSK write-back) | Yes |
| XRoar | Yes | Not modeled (CAS is documented read-only for at least one of its future siblings, per the Atari800 note above, but XRoar itself was not re-verified here) | No | No | Plausible | Yes |
| Tsugaru | Yes | Not modeled | No | No | Plausible | Yes |

**Reusable infrastructure that *does* already exist and this primitive should build on, not
duplicate:**

- **`launch::process_spawn::CapturedFileIdentity`** (`crates/archivefs-core/src/launch/process_spawn.rs:48`):
  a cheap, already-proven `(device, inode, size, modified)` stat-based identity, captured via
  `capture_file_identity(path)`. This is the exact "existing content/freshness binding" Task E asks
  to reuse rather than re-inventing an expensive whole-file hash.
- **The "fresh preflight" pattern**, documented in `launch/mod.rs`'s own module doc comment: mature
  adapters "live-revalidate a user-authorized launch request from scratch (fresh identity
  re-inspection, fresh environment discovery, a freshly rebuilt plan/command)" immediately before
  spawning. This primitive's `SCRATCH_MEDIA_REQUIRED` path plugs into exactly that seam — verify,
  then copy, then re-verify, then spawn — rather than introducing a second, parallel
  freshness-checking idiom.
- **`launch::process_spawn::{PreparedProcessCommand, WatchedProcess, spawn_watched_process}`**: the
  existing typed argv + watched-process contract every mature adapter already uses. This primitive
  produces scratch *paths*; it never needs to touch or replace `spawn_watched_process` itself — an
  adapter simply builds its `PreparedProcessCommand` from scratch paths instead of source paths.
- **`diagnostics::environment::{StorageResource, assess_storage}`** (`crates/archivefs-core/src/diagnostics/environment.rs`):
  the existing, already-correct `statvfs(3)`-based free-space assessment (`available_bytes`,
  critical/error floors, percentage bands). Task L's disk-space guard should call this, not a new
  `fs2`-style crate or a second `statvfs` wrapper.
- **`platform_evidence_fusion::cue_m3u_parsing`**: existing, tested CUE/M3U member-list parsing.
  Task F's "explicit launch-plan member lists" should consume this kind of already-resolved member
  list, not re-derive set relationships itself.

No existing primitive was duplicated by this design.

## Risk-class model (Task B)

```rust
/// How an adapter's target emulator is known to behave toward the media,
/// configuration, and state EmuWiz would otherwise hand it directly.
/// Declared once per adapter (or per adapter+media-kind combination where
/// that distinction matters, e.g. an emulator that is read-only for tapes
/// but not disks) - never inferred at launch time, and never silently
/// downgraded.
pub enum LaunchMediaSafety {
    /// The target emulator has a *proven*, documented, exercised read-only
    /// mode for this exact media kind (e.g. BeebEm's default
    /// write-protect-on-load, or b2's in-memory disc mode). The adapter
    /// still launches the original path directly - nothing changes for
    /// today's mature adapters.
    DirectReadOnly,

    /// No proven read-only mode exists. EmuWiz prepares a verified scratch
    /// copy of every media member and passes only scratch paths to the
    /// emulator. Configuration/CMOS/NVRAM/state are not isolated under
    /// this variant - only used where an adapter's own config is already
    /// known safe or irrelevant (e.g. a stateless, single-shot emulator
    /// invocation with no persistent config file at all).
    ScratchCopy,

    /// No proven read-only media mode *and* the emulator is known or
    /// suspected to write configuration, CMOS/NVRAM, "last used", or other
    /// state outside the media files themselves. EmuWiz prepares scratch
    /// media *and* an isolated configuration/state area (Task G/H). This
    /// is the expected declaration for every adapter named in the "why
    /// this exists" audits (NP2kai, b-em, Caprice32, PX68k).
    ScratchCopyWithIsolatedConfig,

    /// Neither a proven read-only mode nor a working scratch/isolation
    /// story exists yet for this media kind (e.g. a HDD image too large to
    /// copy affordably - see Task M). The adapter must refuse to launch
    /// rather than guess. This is a real, first-class outcome, not a
    /// placeholder - see Task Q, "launch fails closed."
    UnsafeUnsupported,
}
```

This matches the task's own four named classes (`DIRECT_READ_ONLY_SAFE`,
`SCRATCH_MEDIA_REQUIRED`, `SCRATCH_MEDIA_AND_CONFIG_REQUIRED`, `UNSAFE_UNSUPPORTED`), renamed to
follow this codebase's existing `CamelCase` enum convention (e.g. `LaunchCompatibility`,
`CandidateState` in `emulator_setup_page.rs`, `EligibilityBlocker` in `emulator_environment::es_de`)
rather than introducing `SCREAMING_CASE` variants nowhere else in the crate uses.

Adapters declare exactly one variant (see Task R below); they never each implement copying
differently, and the sandbox executor is the single place copy/verify/cleanup logic lives.

## Scratch workspace design (Task C)

### Location

`$XDG_RUNTIME_DIR/emuwiz/launch/<transaction-id>/`, falling back to
`$TMPDIR/emuwiz/launch/<transaction-id>/` (or `/tmp/emuwiz/launch/<transaction-id>/` if `$TMPDIR`
is unset) only when `$XDG_RUNTIME_DIR` is unavailable — never hardcoding `/tmp` as the primary
choice. `$XDG_RUNTIME_DIR` is the right first choice: per the XDG base-directory spec it is
user-owned (mode `0700`), tmpfs-backed on every mainstream Linux desktop, and is already cleared on
logout, which gives this primitive a second, OS-level cleanup guarantee on top of its own (Task K).
The one caveat worth naming honestly: `$XDG_RUNTIME_DIR` is frequently tmpfs (RAM-backed), so a
multi-gigabyte HDD-image scratch copy (Task M) may need the `$TMPDIR`/disk-backed fallback
specifically for large media even when `$XDG_RUNTIME_DIR` exists — the workspace root should be
selectable per-transaction based on the size estimate from Task L, not fixed for all transactions.

### Transaction ID

A per-launch unique id, e.g. `{unix-timestamp}-{random-hex}` (16 hex chars from a CSPRNG is ample -
this is a collision-avoidance identifier, not a security token). Bounded length, `[a-z0-9-]` only,
never derived from user-controlled input (game title, file name) — this closes the path-traversal
and "duplicate name" concerns in Tasks C/T17 at the root: the workspace directory name never
contains anything the user or a DAT/scan result influenced.

### Directory shape

```
$XDG_RUNTIME_DIR/emuwiz/launch/<transaction-id>/
    media/          # scratch copies of PrimaryMedia/SecondaryMedia members (Task F/P)
    config/         # isolated XDG_CONFIG_HOME target, only if declared (Task G)
    data/           # isolated XDG_DATA_HOME target, only if declared
    cache/          # isolated XDG_CACHE_HOME target, only if declared
    state/          # explicit-state-directory target for non-XDG emulators (Task G)
    .emuwiz-owned   # zero-byte marker file - see Task W
```

Created with `0700` permissions on the transaction root (owner-only), inheriting to subdirectories
by default umask under that root. Member filenames inside `media/` are a deterministic,
1:1 mapping from each source member's `Role` + original extension (Task P) — e.g.
`primary.d88`, `secondary.d88` — never the literal source filename, which both avoids leaking a
source path into a place another local user could plausibly read and gives every adapter a
predictable scratch filename to reference in its argv construction. No path traversal is possible
because scratch filenames are synthesized by this primitive, never taken from source path
components.

## Copy semantics (Task D)

**Plain physical copy (`std::fs::copy`, or an explicit read+write loop if finer control over
buffering/O_DIRECT is later justified) is the V1 baseline and the correctness floor.** Concretely:

- **No hardlink.** A hardlink to source is the same inode — an emulator writing into it *is*
  writing the source. Never used, full stop, regardless of same-filesystem convenience.
- **No reflink by default.** A reflink (`ioctl(FICLONE)` on btrfs/XFS/bcachefs) is copy-on-write at
  the *filesystem* level, which is only actually safe if the filesystem's CoW guarantee is real for
  that exact volume and mount — something this primitive cannot verify generically across every
  target machine EmuWiz runs on. If a later audit proves reflink-safe behavior on a specific,
  detected filesystem, it may be used **as a performance opt-in**, but correctness must never
  depend on it — the fallback is always the plain copy, and V1 does not implement the reflink path
  at all (Task M notes it as a named future optimization, not a V1 requirement).
- **No symlink** back to the original — defeats the entire purpose; the emulator would still be
  writing through to source.
- **No chmod/chown of the original.** The scratch copy gets whatever permissions a freshly-created
  file gets (mode `0600`, owner-writable so the emulator can actually use it) — the source's mode
  and ownership are never touched, in either direction.
- **Metadata preserved:** only what an emulator plausibly needs to function correctly — file
  *content*, exactly. Not mtime, not extended attributes, not ACLs. A scratch copy's mtime is
  naturally "now" (copy time), which is fine and arguably correct (it is a new, transient file).

## Source identity proof (Task E)

**Before copying:** capture `CapturedFileIdentity` (`device`, `inode`, `size`, `modified`) for
every source member via the existing `launch::process_spawn::capture_file_identity`. This is the
"sufficiently strong proven mechanism" the task allows substituting for a full hash, precisely
because it is already the mechanism every mature adapter's "fresh preflight" step relies on for the
same purpose (detecting a file that changed between plan time and launch time).

**After copying:** re-`stat` the *source* (not the copy) and compare against the captured identity.
If `device`/`inode`/`size`/`modified` do not all match, the copy is untrusted regardless of whether
the copy syscall itself reported success — the source could have been replaced mid-copy. This
directly satisfies Task T5 ("file identity drift before copy blocks").

**Copy verification:** because `std::fs::copy` is a deterministic, whole-file operation and the
source identity check above already proves the source didn't change out from under it, a **second
whole-file hash of the scratch copy is not required for V1** for ordinary media — re-checking size
(`scratch.len() == source_size_captured`) is sufficient given a deterministic copy path and a
stable source. For **small/critical members** (see Task M's `SMALL_MEDIA_SAFE` class — floppy/tape
images, typically well under 10 MB), computing and comparing a content hash (this codebase already
has hashing infrastructure via its existing DAT/identity subsystems, so no new hashing dependency
is required) costs effectively nothing and removes any doubt; V1 should apply it there. For large
HDD-class images (Task M), a full hash adds real, possibly minutes-long overhead disproportionate
to the risk once the size+identity checks already passed, so V1 documents the exact, honest
guarantee it gives instead of pretending a cheap check is a hash: **byte-for-byte content
correctness for large media is guaranteed by copy determinism plus source-identity stability, not
independently re-verified via hash.**

**Exact guarantee this proves, stated plainly for the doc's audience:** "EmuWiz confirmed the
source file did not change while it was being copied, and the scratch copy has the exact size the
source had at that moment." That is what ships in V1; whole-file re-hashing of large media is left
as a documented, not-yet-justified future strengthening.

## Multi-file / set media (Task F)

The sandbox executor never walks a directory or guesses companions. It receives an explicit,
already-resolved **launch-plan member list** — the same kind of resolved list
`platform_evidence_fusion::cue_m3u_parsing` already produces for CUE/M3U sets — and copies exactly
those files, preserving only their **relative flatness** (Task C's `media/` directory holds every
member as a sibling; no nested source directory structure is reproduced, since nothing in this
primitive needs it — CUE/BIN, M3U members, and two-floppy sets are all flat member lists in
practice). If a future media kind genuinely needs relative *subpaths* preserved between members,
that is an explicit, typed extension to the member list (a `relative_path: PathBuf` field), not a
general recursive copy.

```
source:
  /library/PC-98/Game/disk1.d88
  /library/PC-98/Game/disk2.d88

launch plan members:
  [ { role: PrimaryMedia,   source: "/library/PC-98/Game/disk1.d88" },
    { role: SecondaryMedia, source: "/library/PC-98/Game/disk2.d88" } ]

scratch:
  <workspace>/media/primary.d88
  <workspace>/media/secondary.d88
```

The planner/executor boundary receives **scratch paths only** — an adapter's command-building code
never sees or needs the source paths once the workspace is prepared (though the original plan
remains reachable for diagnostics — see Task O).

## Config/CMOS/NVRAM isolation (Task G)

Modeled as an explicit, adapter-declared **isolation strategy**, because — as the task correctly
warns — not every emulator obeys XDG:

```rust
pub enum ConfigIsolation {
    /// No isolation needed for this adapter (paired only with
    /// `LaunchMediaSafety::ScratchCopy`, never with the
    /// `...WithIsolatedConfig` variant).
    None,

    /// Point HOME/XDG_CONFIG_HOME/XDG_DATA_HOME/XDG_CACHE_HOME at the
    /// workspace's config/data/cache subdirectories for the spawned
    /// process's environment. Only correct for emulators verified to
    /// actually honor XDG env vars - never assumed by default.
    XdgEnvironment,

    /// Pass an explicit, emulator-specific config-file/state-directory
    /// flag pointing into the workspace (adapter supplies the exact flag
    /// shape, e.g. NP2kai's per-port configuration directory argument).
    ExplicitConfigPath { flag_template: &'static str },

    /// Both: some emulators need HOME redirected *and* an explicit flag
    /// for a secondary state file (e.g. CMOS image path) that isn't
    /// itself under the redirected HOME.
    Combined {
        environment: bool,
        explicit_flag_template: &'static str,
    },
}
```

Under `XdgEnvironment`/`Combined`, the spawned process's environment gets exactly:

```
HOME=<workspace>
XDG_CONFIG_HOME=<workspace>/config
XDG_DATA_HOME=<workspace>/data
XDG_CACHE_HOME=<workspace>/cache
```

layered onto (not replacing wholesale) the environment `spawn_watched_process` already builds —
this is an additive, per-launch override, never a change to the user's real shell/session
environment, and never written to any file outside the workspace.

**The user's real config is never opened for writing, never symlinked in, and never referenced by
path** unless explicitly seeded (Task H) — the isolated area starts empty (or seeded) and is
entirely disposable.

## Profile seeding (Task H)

```rust
pub enum ProfileSeed {
    /// Start from nothing - the emulator's own defaults apply.
    Empty,
    /// Copy a known-good, EmuWiz-reviewed profile/config file (never the
    /// user's live config) into the scratch config area before launch.
    /// `source` here is a read-only, versioned asset EmuWiz ships or has
    /// separately verified - never a path into the user's real profile
    /// directory.
    KnownProfile { source: PathBuf, scratch_relative: PathBuf },
}
```

Provenance is tracked the same way media provenance is: the seed's `CapturedFileIdentity` at copy
time is retained on the transaction record (Task O), so a diagnostic can always answer "what exact
profile went into this launch" without needing the scratch copy to still exist. The emulator is
free to mutate the *scratch* profile after seeding; the seed source is never reopened for writing.

## Output / save-data policy (Task I, J)

**V1 default: discard the entire scratch workspace, including any modified scratch media, on
normal exit.** The source was never touched, so "discarding the modification" is really just
"declining to propagate it anywhere" — there is nothing to roll back. No automatic merge of
emulator writes back into source, ever, in V1; a future explicit, transactional "export what the
emulator wrote" user action is out of scope here and would need its own design and explicit user
confirmation per file, per the task's own instruction.

**Task J's save-data distinction is the one genuine nuance this V1 must not blur.** "Source media"
(the ROM/disk/tape image itself) and "user save data" (memory card images, battery-backed SRAM,
save states) are different things with different lifecycles — a memory card is often the *only*
copy of a player's progress and is expected to persist across launches, unlike a scratch disk copy
which is deliberately transient. This primitive does not generically know, for an arbitrary
adapter, which files under an emulator's config/state area are "source-adjacent state" (safe to
discard) versus "user save data" (must persist). Rather than guess:

- **Scope `ScratchCopy`/`ScratchCopyWithIsolatedConfig` sandboxing, in V1, only to adapters that
  explicitly declare which of their state paths are persistent-save versus disposable-scratch** —
  a third field alongside the `LaunchMediaSafety`/`ConfigIsolation` declaration (Task R), e.g.
  `persistent_state: Vec<PersistentStatePath>`, each mapped to a real, stable, outside-the-workspace
  location EmuWiz already manages (or a documented "not yet handled, do not adopt sandboxing for
  this adapter's saves yet" placeholder).
- An adapter that has not made this declaration should not opt into scratch-media sandboxing for
  paths it cannot yet classify — better to leave such an adapter exactly as it behaves today (this
  matches Task R: "Default behavior for existing proven adapters should remain unchanged") than to
  silently vaporize a save file. This is deliberately conservative and is the reason Task X's
  complexity classification below treats "declare persistent-save paths honestly per adapter" as
  real, adapter-specific work this generic primitive can define the *shape* of but cannot resolve
  once for every future consumer.

## Cleanup policy (Task K)

- **Successful launch, clean exit:** delete the transaction workspace immediately (`media/`,
  `config/`, `data/`, `cache/`, `state/`, the marker file, the root).
- **Spawn failure** (the process never started): delete immediately — nothing useful can be
  diagnosed from a workspace whose emulator never ran.
- **Launch that started but exited non-zero, or was killed:** retain the workspace for a **bounded
  diagnostic window** (a fixed, short duration — e.g. on the order of the existing
  `PROCESS_STDERR_CAPTURE_LIMIT`'s bounded-diagnostics philosophy in `process_spawn.rs`, sized in
  minutes not hours) and surface its path in the failure report, rather than either deleting
  evidence a developer/user might need or leaving it forever. A background sweep (see below) is
  what actually enforces the bound, not a timer inside the launch call itself.
- **Process crash / EmuWiz itself crashes mid-launch:** best-effort — nothing can guarantee cleanup
  when the cleaning process is the one that died. This is why a **startup stale-workspace audit**
  is required, not optional: on EmuWiz start (or Doctor scan), scan
  `$XDG_RUNTIME_DIR/emuwiz/launch/` for directories carrying the `.emuwiz-owned` marker whose age
  exceeds the retention window, and remove only those. **Never delete anything under that root
  without the marker present** — an unmarked entry is not provably this subsystem's, so it is left
  alone and, at most, reported.

This is a direct, deliberate response to the task's own note: "Recent LBC audit found roughly 572
GB consumed under `/tmp`, largely EmuWiz Cargo targets... This primitive MUST NOT become another
source of abandoned disk usage." The ownership marker plus "never touch unmarked content" plus a
bounded retention window is the whole defense; it does not rely on a human remembering to clean up,
and it does not risk deleting another tool's `/tmp` content the way an unscoped sweep would.

## Disk-space guard (Task L)

Before copying, sum the sizes of every planned scratch member (captured as part of
`CapturedFileIdentity.size` during Task E's pre-copy identity capture) plus a fixed margin (a flat
percentage, e.g. 10%, with a sane minimum floor so a tiny multi-KB tape image still gets a workable
margin) and compare against `diagnostics::environment::assess_storage`'s `available_bytes` for the
chosen workspace filesystem — **reusing that existing, already-correct `statvfs` assessment**
rather than a new one. If projected need exceeds available space, **fail before copying anything**:

> "EmuWiz needs 3.2 GB of temporary space to launch this safely."

No partial copy is ever started once the guard has run — the check happens for the *whole* member
set up front, not per-file as copying proceeds, so a failure never leaves a half-filled workspace
(and if a later, unexpected `ENOSPC` still occurs mid-copy despite the guard — e.g. a race with
another process filling the disk — the executor treats that exactly like any other
`ScratchCopyFailed` and cleans up what it started, per Task Q).

## Large-media limitations (Task M)

| Class | Examples | V1 handling |
|---|---|---|
| `SMALL_MEDIA_SAFE` | Floppy/tape images (SSD/DSD/D88/DSK/CDT/ATR/CAS), typically well under 10 MB | Plain copy is cheap (sub-second on any real disk); apply the optional content-hash re-verification from Task E |
| `LARGE_MEDIA_EXPENSIVE` | CD/DVD-class ISO/CUE-BIN, tens of MB to a few GB | Plain copy is honest but not free — seconds to low minutes; the space guard and a visible "Preparing protected launch copy…" state (string only, no GUI work here — Task Y) are both required, not optional |
| `UNSUPPORTED_UNTIL_BETTER_OVERLAY` | NP2kai HDI/HDD images, PX68k large disk images — potentially tens of GB | V1 does **not** claim to make these safe via plain copy. An adapter targeting this class should declare `LaunchMediaSafety::UnsafeUnsupported` for it until a proven, filesystem-verified reflink/CoW path or an emulator-native overlay (qcow2-style, or the emulator's own snapshot/overlay feature if one exists and is documented) is separately audited and proven. This is Task M's explicit instruction ("V1 should prioritize correctness... do not hide large-copy cost") taken at face value: correctness-first means refusing rather than pretending a multi-tens-of-GB physical copy is a reasonable default UX. |

This directly matches the PC-98 audit's own HDI classification (`PROFILE_REQUIRED`, "HDD is
potentially writable; scratch-copy and profile contract needed") — this design gives that future
work an honest, named bucket instead of quietly assuming a full-image copy is always fine.

## Read-only source opening (Task N)

The copy step opens the source with `OpenOptions::new().read(true).write(false)` (or the
equivalent taken by whatever copy call is used) — write permission on the source is never
requested and never required. A source living on a read-only mount, a read-only bind-mount, or a
file the current user only has read access to must work exactly as well as one on writable media;
verifying this (a synthetic read-only-permission source fixture) is part of Task T/U's test matrix
(items 3 and, implicitly, the general "no source mutation" tests).

## Launch execution integration (Task O)

```
PLAN ORIGINAL MEDIA            (existing: adapter's typed launch-plan construction, unchanged)
  → FRESH PREFLIGHT            (existing "live-revalidate from scratch" pattern, unchanged)
  → PREPARE SAFE WORKSPACE     (new: this primitive - Tasks C/D/E/F/G/H/L/M)
  → MAP ORIGINAL PATHS TO SCRATCH PATHS   (new: typed mapping - Task P)
  → SPAWN WATCHED PROCESS      (existing: process_spawn::spawn_watched_process, unchanged - built
                                 from a PreparedProcessCommand whose argv now references scratch
                                 paths instead of source paths)
  → WATCH                      (existing: WatchedProcess::poll, unchanged)
  → CLEANUP                    (new: this primitive - Task K)
```

The **original plan/provenance is never overwritten or discarded** by workspace preparation — the
typed path mapping (Task P) is an additional, parallel structure carried alongside the original
plan, not a destructive rewrite of it. A diagnostic (or a failed-launch report) can always show
both "what EmuWiz was asked to launch" (source paths, `CapturedFileIdentity`) and "what was
actually handed to the emulator" (scratch paths) side by side. This is a hard requirement, not a
nicety: losing the original source identity mid-pipeline would make Task Q's "explicit failure with
useful diagnostics" impossible to honor.

No changes to `spawn_watched_process`, `PreparedProcessCommand`, or `WatchedProcess` are proposed —
they already do exactly what this integration needs (typed argv in, watched process out). This
primitive is a **new stage inserted before command construction**, not a modification to execution
itself.

## Typed path mapping (Task P)

```rust
pub enum MediaRole {
    PrimaryMedia,
    SecondaryMedia,
    Config,
    State,
}

pub struct ScratchPathMapping {
    pub role: MediaRole,
    pub original_path: PathBuf,
    pub scratch_path: PathBuf,
    /// Captured before copy, re-checked after - see Task E.
    pub original_identity: CapturedFileIdentity,
}
```

A `Vec<ScratchPathMapping>` (one entry per media/config/state member) replaces any temptation to
reach for a loose `HashMap<String, String>` — every field is typed, `Role` is a closed enum an
adapter can exhaustively match on (so adding a new role is a compile error at every call site until
handled, matching this codebase's existing preference for exhaustive matches seen throughout
`launch/`), and `original_identity` keeps the freshness proof attached to exactly the path it was
captured for rather than living in a separate, easily-desynced structure.

## Failure model (Task Q)

```rust
pub enum SafeLaunchSandboxError {
    ScratchSpaceUnavailable { workspace_root: PathBuf, source: std::io::Error },
    InsufficientTemporarySpace { required_bytes: u64, available_bytes: u64 },
    ScratchCopyFailed { member: PathBuf, source: std::io::Error },
    ScratchVerificationFailed { member: PathBuf, reason: VerificationFailureReason },
    ConfigIsolationFailed { path: PathBuf, source: std::io::Error },
    UnsafeMediaPolicy { reason: &'static str },
    CleanupFailed { workspace_root: PathBuf, source: std::io::Error },
}

pub enum VerificationFailureReason {
    SourceIdentityDrifted,
    ScratchSizeMismatch { expected: u64, actual: u64 },
    ScratchContentHashMismatch,
}
```

This follows the existing per-adapter pattern of a small, closed, adapter-facing error enum (e.g.
`DolphinLaunchSpawnError`, `PpssppLaunchSpawnError` already in `launch/`) rather than a stringly
`anyhow`-style error, so callers can exhaustively handle every failure mode.

**No silent fallback to launching the original writable media exists anywhere in this design.**
Every error variant above is terminal for that launch attempt — `CleanupFailed` included: a
workspace that failed to clean up is reported (and left for the stale-workspace audit, Task K), not
silently ignored, but it never causes EmuWiz to retry the launch against source media. This is the
task's own explicit, all-caps requirement ("If scratch setup fails: LAUNCH FAILS CLOSED") and this
design treats it as non-negotiable.

## Adapter declaration API (Task R)

The smallest possible opt-in surface — three values an adapter's module associates with itself
(as a `const` or a small struct returned from a function, mirroring how `LAUNCH_COMPATIBILITY`
entries already declare static per-platform facts in `launch/mod.rs`):

```rust
pub struct LaunchMediaSafetyDeclaration {
    pub safety: LaunchMediaSafety,
    pub config_isolation: ConfigIsolation,
    pub persistent_state: &'static [PersistentStatePathTemplate],
}
```

**Every existing adapter's behavior is unchanged by this primitive's mere existence.** An adapter
that does not add a `LaunchMediaSafetyDeclaration` continues launching exactly as it does today —
this is additive infrastructure, not a mandatory migration. `LaunchMediaSafety::DirectReadOnly` is
the honest declaration for an adapter that has not been audited for write risk yet (equivalent to
"unchanged"); adapters should only move to `ScratchCopy`/`ScratchCopyWithIsolatedConfig` once their
own audit (like the four already done) has established the need and the exact config/state shape,
per adapter, per Task J's save-data caution above.

## First future consumers (Task S — documentation only; none implemented here)

- **NP2kai:** `ScratchCopyWithIsolatedConfig` for D88/HDI; per-port BIOS/config directory becomes
  the `ExplicitConfigPath`/`Combined` target; HDD-class HDI likely lands in
  `UNSUPPORTED_UNTIL_BETTER_OVERLAY` (Task M) until a safe large-media path exists, while D88
  floppies are squarely `SMALL_MEDIA_SAFE`.
- **b-em (BBC Micro):** `ScratchCopyWithIsolatedConfig` for SSD/DSD/UEF wherever write risk applies
  (i.e. essentially always, given no documented write-protect flag); small media throughout, so the
  optional content-hash re-verification from Task E is cheap to apply everywhere for this adapter.
- **Caprice32 (Amstrad CPC):** `ScratchCopy` (or `...WithIsolatedConfig` if Caprice32's own config
  file turns out to be mutated — not yet independently confirmed) for DSK/CDT.
- **Atari800:** this primitive is exactly what would let ATR/XFD/ATX move out of `DEFER` — CAS
  stays `DirectReadOnly` (already proven), while ATR/XFD/ATX become candidates for `ScratchCopy`
  once this primitive lands and is proven, without needing a second bespoke scratch mechanism
  invented inside the Atari800 adapter itself.
- **PX68k (X68000):** `ScratchCopyWithIsolatedConfig` for XDF/DIM, same shape as NP2kai given the
  similarly-sized HDD-class image concern for some X68000 media.

None of these five adapters are implemented, modified, or scaffolded as part of this task, per the
explicit instruction.

## Test strategy (Task T/U/V — designed, not implemented in this pass)

The task's 22-item matrix maps directly onto this design's pieces; each numbered item below is
covered by a specific mechanism already described above rather than needing new machinery:

1. Source byte-identical after simulated scratch mutation → Task E's pre/post identity capture,
   proven against a real filesystem fixture (Task U).
2. Scratch file independently writable → scratch copies are created mode `0600`, owner-writable,
   distinct inode from source by construction (never hardlinked — Task D).
3. Read-only source works → Task N; a fixture opened `0444`/on a read-only bind mount must still
   copy successfully.
4/16/17. Symlink source refusal, path traversal refused, duplicate names handled → the transaction
   ID and scratch filenames are synthesized (never derived from user/source-controlled strings —
   Task C), which structurally prevents traversal and collision; symlink-source handling should be
   an explicit, tested policy choice (refuse, or resolve-then-copy-the-target — this design leans
   toward refusing a source that is itself a symlink to something outside the expected library
   root, consistent with the no-follow-symlink policy already established in
   `emulator_environment::HostReadOnlyFilesystem`, but this is flagged here as a decision for the
   implementing pass, not fully closed by this document).
5. Identity drift before copy blocks → Task E, `VerificationFailureReason::SourceIdentityDrifted`.
6. Multi-file set relative layout preserved → Task F's explicit member-list mapping.
7. No hardlink to source → Task D, structurally (the copy call never uses `hard_link`).
8/9. Config copied to isolated location, environment isolation correct → Task G/H, verified by
   asserting the spawned environment map contains exactly the expected `HOME`/`XDG_*` overrides and
   nothing from the real user environment leaks through unexpectedly.
10. Scratch failure never falls back to original → Task Q, exhaustively (every error variant is
    terminal).
11. Insufficient space fails before partial copy → Task L, whole-set-first check.
12/13/14. Successful/failed/crash cleanup → Task K's three explicit policies plus the
    stale-workspace audit for the crash case.
15. Unrelated user save data not destroyed → Task J's persistent-state declaration; a test proving
    a declared persistent-save path survives workspace cleanup while scratch media does not.
18. Concurrent launches receive separate workspaces → transaction-ID uniqueness (Task C);
    straightforward to prove by launching two synthetic transactions and asserting disjoint roots.
19. Watched-process integration preserved → Task O; a synthetic adapter using scratch paths through
    the existing `spawn_watched_process` should behave identically to today's direct-path adapters
    from that function's point of view (it never needs to know paths are scratch paths).
20. Source mtime/size/(hash where applicable) unchanged → same mechanism as item 1.
21. No network involved → true by construction (this primitive is pure local filesystem work; no
    test needed beyond code review, but worth a `#[test]` asserting no networking crate is even a
    dependency of the new module).
22. No media write-back to source → same mechanism as item 1/20, stated as its own explicit
    assertion in the test suite for clarity even though mechanically identical.

### Real-filesystem QA (Task U) — approach, not run in this pass

A synthetic fixture (a small generated file standing in for a floppy image, written under this
session's scratchpad or a `tempfile::tempdir()`-managed directory, never a real ROM) copied into a
real scratch workspace, with a simulated "emulator mutation" (just `std::fs::write` into the
scratch path) run afterward, then asserting via `CapturedFileIdentity` re-capture that the
*source's* inode, size, and mtime are exactly what they were before — this is a cheap, fast,
fully-synthetic test with no real preservation media involved, matching what this same session's
other adapter-audit test-matrix designs already do.

### Performance (Task V) — estimates, not benchmarked in this pass

Plain `std::fs::copy` throughput is bound by the underlying storage, not by anything this primitive
adds — the overhead specific to this design is the identity capture/verification (two `stat(2)`
calls per member, negligible) and, for `SMALL_MEDIA_SAFE` members, one content hash pass. Honest,
order-of-magnitude expectations rather than a fabricated benchmark table: a 10 MB floppy-like copy
is sub-second on any real disk or tmpfs; a 100 MB optical-ish copy is roughly a second on typical
SSD-class storage; a 1 GB HDD-like copy is several seconds and is exactly the size class where the
Task L space guard and a visible "Preparing protected launch copy…" status string stop being
optional. No synthetic multi-GB fixture was created to benchmark this in the current disk-pressure
environment (see Task W) — this is a documented, order-of-magnitude estimate, not a measured
number, and should be re-stated honestly as an estimate wherever it is cited until an actual
implementation is benchmarked.

## Interaction with the current `/tmp` disk pressure (Task W)

At the time of this audit, `/` (which is where `/tmp` and every worktree's Cargo `target/` dir also
live on this host) was observed at **100% capacity with as little as ~2.6–15 GB free**, fluctuating
as concurrent sessions built and cleaned up their own target directories. This is exactly the
condition Task W warns this primitive must not add to. Three design choices directly address it,
restated together here because they matter most under exactly this condition:

1. **Ownership marker + "never delete unknown content"** (Task K) — this primitive's cleanup logic
   can never become a second source of unbounded growth *or* a risk of deleting another tool's
   `/tmp` files, because it only ever acts on directories it marked itself.
2. **The space guard runs before any byte is copied** (Task L) — under real disk pressure like the
   condition observed during this audit, a launch that would exhaust free space fails immediately
   with a clear message instead of contributing to the exact kind of silent, gradual fill the LBC
   audit found.
3. **`$XDG_RUNTIME_DIR` as the default root** (Task C) — tmpfs-backed on most desktops, which means
   ordinary small/moderate scratch media (the common case) does not touch persistent disk at all;
   only the large-media fallback (Task M) needs `$TMPDIR`/disk space, and that is exactly the case
   the space guard is strictest about.

## Implementation decision (Task X)

**Classification: MODERATE in isolation, but real ownership collision exists right now.**

The primitive itself — a new, self-contained module (workspace creation, copy, verify, cleanup,
typed errors) — is moderate, well-scoped work with a clean, additive integration point. However,
making it *reachable* from the rest of the crate requires at minimum:

- a new `pub mod` declaration in `crates/archivefs-core/src/lib.rs`, and
- (for the `LaunchMediaSafety`/`ConfigIsolation` declaration types, per Task R, to be genuinely
  usable by a future adapter) likely a home in or alongside `crates/archivefs-core/src/launch/mod.rs`.

At the time of this audit's preflight, **both files are already modified, uncommitted, by other
concurrent lanes** (`git status --short` showed `M crates/archivefs-core/src/lib.rs` and
`M crates/archivefs-core/src/launch/mod.rs`, alongside `M crates/archivefs-core/src/patch_manager/mod.rs`
— the same three central registration points this session has repeatedly found other active lanes
mid-edit in). Adding hunks to any of them right now risks exactly the kind of collision this task
explicitly says to avoid ("Do not collide with active adapter/GUI work"), and mirrors this same
session's own established, successful pattern earlier in this campaign (the BBC Micro adapter audit
went docs-only for the identical reason, against the identical files).

**Decision: docs-only for this V1**, per the task's own explicit escape hatch ("If LARGE or
ownership collision exists: write decision-ready design only"). This document is written to be
directly implementable once `lib.rs`/`launch/mod.rs` settle — every type above is specified
precisely enough (field names, enum shapes, error variants) that implementation should not require
re-deriving the design, only writing the code this document already describes.

## Files changed

- `docs/research/SAFE_LAUNCH_SANDBOX_V1.md` (this file) — new.

No other file was created, modified, or staged.
