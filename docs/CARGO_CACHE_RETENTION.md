# Cargo cache retention (phase 1: tracking, protection, dry-run planning)

`scripts/cargo-iso` keeps one isolated Cargo target per worktree. Nothing used to retire
those targets. `scripts/emuwiz-cargo-retention` is the lifecycle manager for them.
**Phase 1 never deletes anything and never runs automatically.**

## What `cargo-iso` now does

Every build runs under `emuwiz-cargo-retention run`, which

* takes a **shared lifecycle lock** (`flock`, outside the target) for the whole build;
  the kernel releases it if the build dies, and cargo's children inherit it, so an
  orphaned `rustc` still protects the cache;
* records canonical worktree, path hash, HEAD, owner, build start/end/last use, exit
  status, process identity (pid + start time + boot id), low-debug and filesystem in the
  registry (`~/.cache/emuwiz-cargo-retention`, override `EMUWIZ_RETENTION_DIR`);
* runs cargo with unchanged arguments and environment, forwards signals, returns its exit
  code (or dies by the same signal);
* never blocks a build when the registry is unusable or a record is malformed: it reports the
  tracking failure, releases the lock, leaves the malformed evidence untouched and runs cargo
  untracked (the target then has no valid lifecycle record and stays ineligible).
  `EMUWIZ_RETENTION=0` disables tracking the same way.

`--print-target`, the 8 GiB free-space guard and all target isolation are unchanged.

## Commands

```
scripts/emuwiz-cargo-retention status                 # totals, protected, free space, budgets
scripts/emuwiz-cargo-retention list [--json]          # each target: size, owner, activity, holds
scripts/emuwiz-cargo-retention plan [--json] [--output FILE] [--whatif-pending]
scripts/emuwiz-cargo-retention pin TARGET --reason active-development|independent-review|
                                              release-build|running-gui|preservation-evidence
scripts/emuwiz-cargo-retention release TARGET [--reason R]   # only pins you own
scripts/emuwiz-cargo-retention adopt TARGET --owner NAME     # record an owner for an untracked cache
```

`TARGET` is a target name, directory, or the worktree path it belongs to. The owner is
`$EMUWIZ_OWNER`, else `$USER`. Pins survive restarts and are never released by age.

Owner names are **advisory labels inside one Unix account**, not a security boundary: anyone
with the account can set `EMUWIZ_OWNER` or edit the registry. `--owner` may only restate your
own label; naming another owner is refused so a pin is not released by accident or by
supplying someone else's name. Pin state that is unreadable, corrupt or of the wrong shape is
`PIN_STATE_UNKNOWN`: the target is held, nothing is selected, `pin`/`release` refuse to touch
the file and it is never rewritten. A malformed target record is likewise preserved and the
target has no lifecycle record.
`status`, `list` and `plan` are read-only and create nothing.

## What makes a target retirable (all must hold, otherwise HOLD)

lifecycle record with a known owner; no pin; no build lock; no recorded live build; no
process with the target as cwd, executable, open file, mapped file or
`CARGO_TARGET_DIR`; no write or recorded build within 48 h; unchanged directory identity;
a real directory with an ordinary name; no mount point inside; and complete process
visibility (other users' unreadable user-space processes HOLD everything unless
`--accept-foreign-process-gap` is given, and the output says so).

Unrecorded targets (everything built before this change) are counted but never eligible.
`plan` lists inactive ones as *pending ownership*; `--whatif-pending` estimates what they
would free. `adopt` is a human attestation, not evidence of activity.

## What a plan may select

Only enumerated compiler output inside a profile directory: `.fingerprint`, `build`,
`deps`, `examples`, `incremental`, `.d`/`.rlib`/`.rmeta`/`.so`/`.a` outputs, executables
that have a matching `.d`, `.cargo-lock`, `.rustc_info.json`. Never `emuwiz-test-env`,
`tmp`, unknown files or symlinks. A subtree containing a symlink, a mount point, or a
database/journal/ROM/save-looking file is preserved whole. Recoverable bytes count each
inode once and only when **every** hard link is inside the selection.

## Budgets (advisory in phase 1)

220 GB soft, 240 GB hard (`EMUWIZ_SOFT_BUDGET_BYTES`, `EMUWIZ_HARD_BUDGET_BYTES`), on the
device holding the target root; other devices are reported separately. `status` names the
protected targets that stop further reduction. Active or protected caches are never
evicted.

## Manifest-authorised retirement (`scripts/emuwiz-cargo-retire`)

Dry run by default; deletes only with `--execute --approved-sha256 <digest> --receipt-dir DIR`.

```
scripts/emuwiz-cargo-retire --manifest RETIREMENT_BATCH.json --approved-sha256 <digest>          # dry run
scripts/emuwiz-cargo-retire ... --execute --receipt-dir <outside-any-cache> [--maintenance-window-attested]
```

Per cache, ALL of these must hold or the cache's paths are HELD with a reason:

1. manifest digest equals the approved digest (a modified manifest is refused whole);
2. canonical target path with no symlink component, manifest device/inode, real directory;
3. a lifecycle record whose ownership is *verified*: the source checkout exists, its HEAD
   equals the manifest's, and the target's path hash (or the recorded worktree) names that
   source. An adoption record alone never authorises deletion;
4. exclusive lifecycle lock obtained (non-blocking; a running build holds it shared, and a
   build that starts during retirement blocks on it), then a **fresh** census: no cwd, exe,
   open file, mapped file or `CARGO_TARGET_DIR` reference to the cache or its source, with
   complete process visibility;
5. no pin, no recorded live build, no write within 48 h or since the manifest was made;
6. cache never built through `cargo-iso` (no lock participation recorded) additionally
   needs `--maintenance-window-attested`.

Per path: inside the target, an enumerated compiler artifact (never `emuwiz-test-env`,
`tmp`, unknown files, Cargo's own `.cargo-lock` / `.cargo-build-lock` /
`.cargo-artifact-lock`), manifest device/inode/mode unchanged, no symlink, mount point or
data/journal/database/ROM/save-looking file in the subtree. Removal uses directory
descriptors opened `O_NOFOLLOW` (`openat`/`unlinkat` via `dir_fd`), re-checks identity
immediately before each entry and that the target path still names the open directory, and
stops the entry on anything unexpected. The receipt records per-path outcome and reason,
accounted freed bytes (inodes whose last link was removed) and free space before/after.

### Execution journal, partial results and the deletion-time guard

`--execute` first publishes a durable, fsynced journal `retire-<id>.journal.jsonl` in
`--receipt-dir` (which must be outside every cache; it is refused otherwise). It records the
execution identity and approved manifest digest, the plan, then per manifest entry an
`intent` **before** its first unlink and an `outcome` after it, and `end`. It holds
O(manifest entries) records, never one per deleted file. If it cannot be created or an
`intent` cannot be written (for example ENOSPC), nothing is removed; if an `outcome` cannot
be written the run stops after that entry and is reported `ABORTED`.

* An entry that stopped after removing some artifacts is **`PARTIAL`** (group `PARTIAL`),
  with the files, directories and bytes actually removed; it is never reported as refused
  or "nothing removed". Hardlinked blocks that stay referenced elsewhere are counted as
  retained, not freed.
* KeyboardInterrupt and SIGTERM/SIGHUP are journalled and exit 130/143 with a receipt of the
  progress so far. SIGKILL or a crash leaves a journal without `end`.
* `--inspect-journal FILE` (read-only) classifies each entry: `KNOWN_COMPLETED`,
  `KNOWN_PARTIAL`, `KNOWN_UNTOUCHED` (never started, or same identity and unchanged
  content count) or `UNCERTAIN_*`. A missing pathname with no recorded outcome is
  `UNCERTAIN_ABSENT_UNCONFIRMED`, never assumed complete.
* A later `--execute` refuses (exit 6) while an incomplete journal is present, until
  `--acknowledge-incomplete-journals`; incomplete journals are never modified or removed.
* Immediately before every unlink the executor re-classifies the entry: symlinks, other
  filesystems, journals/databases/saves/ROM-like names, `emuwiz-test-env`/`tmp`, and anything
  that was not in the snapshot taken under the exclusive lock are refused, and the file's
  inode is re-checked. Each directory's children are validated before any are removed.

Limits: inode numbers can be reused, so `KNOWN_UNTOUCHED` for a directory relies on identity
plus content count, and a partially emptied directory is classified uncertain rather than
proven partial. The final receipt is published atomically after the journal's `end`; if that
fails the journal is complete and the run exits 5.

### What advisory locks do NOT protect

Locks bind only processes that run through `cargo-iso`. A plain `cargo`/`rustc` started
with `CARGO_TARGET_DIR` pointing at a cache does not take the lock and cannot be excluded
by this design. The census narrows the window (and `--execute` does one fresh census while
holding the lock) but is a point-in-time observation. Required operational prerequisite
for any cache that was not built through `cargo-iso`: a **controlled maintenance window**
in which no agent runs Cargo against it (e.g. all agents idle, or the target's source
checkout is quiescent), attested with `--maintenance-window-attested`. The flag is an
attestation, not enforcement. Complete process visibility also needs sufficient privilege
(root, or a `/proc` mounted without `hidepid`): without it every cache is HELD.

## Phase 3: automatic lifecycle and maintenance (`scripts/emuwiz-cargo-maintain`)

### Lifecycle states
`list`/`status` derive one state per target: **ACTIVE** (a build or program uses it),
**PROTECTED** (pinned, or pin state unknown), **IDLE** (tracked, not released),
**RELEASED** (owner finished the task), **RETIRABLE** (released and every gate clear),
**UNTRACKED** (no valid lifecycle record: legacy, never automatic). A new build through
`cargo-iso` re-opens a released target.

### Finishing a task: the required order
Finishing a Cargo command is not finishing a task. While the worktree still exists:

```
scripts/cargo-iso --finish-task --task <name>      # idempotent release + "may I remove it?"
scripts/emuwiz-cargo-maintain worktree-check <wt>  # CLEAR only after verified retirement
```

Retirement verifies ownership from the source worktree and needs 48 h of inactivity, so the
worktree must stay until `worktree-check` reports CLEAR (artifacts retired with a verified
receipt/journal, or none was ever created: no record of a cargo-iso build and an absent or empty cache
directory). Missing artifacts alone prove nothing: they HOLD as `ARTIFACTS_MISSING_UNVERIFIED`. HOLD reasons: `NOT_RELEASED`, `WAITING_INACTIVITY`
(with the eligibility time), `AWAITING_RETIREMENT`, `ACTIVE`, `PINNED`, `PROTECTED_REMNANTS`,
`SOURCE_MISSING`, `NO_LIFECYCLE_RECORD`, `RECEIPT_UNVERIFIED`, `ARTIFACTS_MISSING_UNVERIFIED`. A release record alone never
clears the gate or authorises deletion; releasing does not stop builds, remove pins or claim
processes have exited. The gate never removes anything, and there is no hook into
`git worktree remove`: a cleanup workflow must call `worktree-check` itself. `AGENTS.md` and
`scripts/task-postcheck.sh` carry the reminder; nothing runs automatically.

### Policy, budgets, cycle
`emuwiz-cargo-maintain policy approve --owners a,b [--soft-gb 220 --hard-gb 240 --max-targets N
--max-gb N]` records a persistent, digest-protected, bounded policy (target root, owner labels,
per-cycle limits; idle floor 48 h). It is **disabled** until `policy enable`, which writes a separate
enable record bound to the policy digest plus a history event; the `enabled` flag inside the policy
file is ignored, and re-approving requires a fresh enable. A malformed policy, one missing fields
(`scope.kinds`, limits), one owned by an account other than the effective user or registry owner
(root is not a wildcard), or a group/world-writable file or state directory fails closed. A generated
manifest hash is never the authorisation: the manifest must name the live policy digest and only
groups the cycle itself planned. `max-gb` is a hard upper bound on a cycle's estimated reclaim: a
candidate that would cross it is skipped (a smaller, younger one may still be taken).
`run` (one cycle) acts only when the policy is approved and enabled, the process census is
complete (run as root: `sudo -n`; a partial census blocks the cycle), the total is over the soft
budget, and only for RETIRABLE targets with a wrapper-recorded build (`last_start`) in the policy
scope, oldest first, within the per-cycle bounds. It writes a manifest outside every cache and hands
it to the unchanged executor **without** `--maintenance-window-attested`, so legacy or
non-participating targets stay held with an explanation. Incomplete journals are never
auto-acknowledged. `status`, `plan`, `run --dry-run` and `history` report decisions, held reasons
(actionable), reclaimed space and the last cleanup.

### Admission and the minimum-free guard (ADVISORY ONLY)
`cargo-iso` calls `emuwiz-cargo-maintain admit` for a new or re-created (artifact-free) target.
The check is **advisory**: if allocated storage plus an allowance (3 GB default;
`EMUWIZ_COLD_TARGET_GB`) would exceed the hard budget it prints a warning, names the reclaimable
released output (or says protected/active output alone exceeds the budget) and records an
`admission_advisory` event. It **never refuses a build**; exit 5 is reserved and unused.
`EMUWIZ_ADMISSION=0` switches the advisory off and is recorded (`admission_disabled`).
`EMUWIZ_ADMISSION=enforce` is not available: it prints that it is unavailable and stays advisory.
Malformed, negative, zero or non-finite budget/allowance settings are reported ("ignored invalid")
and the defaults are used; an incomplete measurement draws no conclusion.
The separate 8 GiB physical free-space guard in `cargo-iso` is unchanged and still blocks (exit 4).

Not yet available (required before an enforcing mode may be exposed): one trusted allocation
measurement shared by status/run/admit (admit uses only `du` over the target root; status counts
worktree-local targets too), an atomic reservation so simultaneous cold builds cannot both pass on
the same cached total, durable recording of configuration changes, and tests for all of them.
Until then nothing can cap running builds; nothing is ever killed or evicted to meet a number.

### Scheduled operation (opt-in)
`emuwiz-cargo-maintain units` prints a systemd service/timer pair (root, for a complete census;
`ConditionPathExists` on the policy). Nothing is installed or enabled by default.

### Limits
Plain `cargo` that bypasses `cargo-iso` is not excluded by any lock: such targets are never
retired automatically. Caches whose worktree was removed first are held (`SOURCE_MISSING`).
Admission is advisory and cannot guarantee a physical cap. The policy, enable record and retirement
evidence are protected by ownership, permissions and unkeyed SHA-256 digests plus a cross-checked
history: this is structural verification, not authenticity. Anyone controlling the approving account
(or root) can forge a self-consistent set. Worktrees whose build produced no output are held rather
than guessed clear (`ARTIFACTS_MISSING_UNVERIFIED`) and need a human review.

## Required before hard enforcement or deletion (phase 2+)

1. A manifest-authorised executor that takes the **exclusive** lifecycle lock
   (non-blocking; failure = HOLD), re-runs the census, and re-checks each selected path's
   device/inode, then deletes with directory-file-descriptor operations
   (`openat`/`unlinkat`, no pathname re-resolution).
2. Registry records for every target (use `adopt` for legacy ones) and a task-end release.
3. An ENFORCING new-cold-target admission check (the current one is advisory only; see above).
4. Process visibility with sufficient privilege, or an explicit accepted exception.
5. Receipts: before/after allocated blocks, refusals, manifest digest.

## Known limitations

Process inspection is a point-in-time observation; the lock closes the interval only for
builds started through `cargo-iso`. Builds started by plain `cargo` are not locked.
Sizes are measured on demand and not cached. atime is reported but not used as activity
unless `--strict-atime`. The lock wait is 60 s (exit 75) so a future executor cannot
stall builds indefinitely.
