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
`CARGO_TARGET_DIR`; explicit task completion released by its known owner, with no later
write or build; unchanged directory identity;
a real directory with an ordinary name; no mount point inside; and complete process
visibility (other users' unreadable user-space processes HOLD everything unless
`--accept-foreign-process-gap` is given, and the output says so).

Unrecorded targets (everything built before this change) are counted but never eligible.
`plan` lists inactive ones as *pending ownership*; `--whatif-pending` estimates what they
would free. `adopt` is a human attestation, not evidence of activity.

A target's source is inferred from `--repo`'s worktrees. Only per-worktree targets left
unmatched are then searched for: a git checkout whose physical path hashes to the target
name's 12-hex suffix (the `cargo-iso` naming rule), up to three levels below `$HOME`, each
worktree's parent, `--source-root` and `EMUWIZ_SOURCE_ROOTS`. The search skips hidden
directories, never follows symlinks and never enters another mount point below a root (a
root you name that is itself on a hung mount will still block). Inference only fills in the
reported worktree/HEAD and what `adopt` records; it never removes a hold. At retirement
`emuwiz-cargo-retire` re-verifies the manifest's source path hash and compares the source's
current HEAD with the manifest's `source_head`, not with the adopted HEAD.

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
5. no pin, no recorded live build; the known owner explicitly released the completed task,
   with no subsequent write/build and no write since the manifest was made;
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
* On restart, an incomplete journal reconciles automatically only when its own bytes prove it
  safe. Both rules need a complete `begin` naming this host and a valid pid that is no longer an
  executor, contiguous sequence numbers and no second `begin` or `end`:
  * `CLEAN_SELF_RECORDED_INTERRUPTION`: the executor's own `interrupted` is the final record,
    every intent is closed by an outcome for the same planned path, and every entry is `KNOWN_*`
    from recorded outcomes alone. Recorded counts are a lower bound, short by at most one file
    or directory per entry, so a fully removed entry can appear as `KNOWN_PARTIAL`.
  * `NO_UNLINK_STARTED`: no intent was ever completed (only plans, optionally a final
    `interrupted`, optionally a torn final line). Intents are fsynced before an entry's first
    unlink, so nothing was removed.
  The next run lists each such
  journal and its SHA-256 in its own `begin` (`reconciled_interrupted`) and in its receipt; it
  writes no resolution receipt and still re-validates its own approved manifest. The verdict
  is recomputed from the bytes every time, so any change to the journal is judged afresh.
  `--inspect-journal` shows it as `restart_reconciliation`.
* Any other incomplete journal (torn line, SIGKILL or crash, an intent with no outcome, an
  interrupted entry with nothing recorded removed, another host, an unsafe file) makes a later
  `--execute` refuse (exit 6) until it is reviewed with `--resolve-journal` (or the run is given
  `--acknowledge-incomplete-journals`). An empty journal or one whose `begin` is torn (for
  example ENOSPC at creation) carries no host or pid, so it is neither reconciled nor
  resolvable; clearing it needs one deliberate manual `--execute --acknowledge-incomplete-journals`,
  which maintain never passes. Resolution receipts written by the 2026-10-21 deployed resolver
  (`"schema": 1`, bound to this host, the journal's name and SHA-256, with an `accepted_uncertain`
  list) are honoured. Incomplete journals are never modified or removed.
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
by this design. The census narrows the window (`--execute` rechecks after snapshots
and immediately before each selected subtree while holding lifecycle and metadata locks;
new process arrivals also trigger a fresh census immediately before unlink/rmdir) but is a point-in-time observation. Required operational prerequisite
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

### Completion replaces the mandatory waiting period
There is no mandatory 48-hour delay. Immediate retirement requires a conclusively completed,
promoted or explicitly abandoned task: required commits and evidence are preserved, and no
agent owns or expects to resume using its target. Only then may the known owner attest this
with `--finish-task` / `finish` (record the task and evidence in `--task` / `--note`). A successful
Cargo command, old mtime, merged source ancestry or an adoption record alone is not completion.
Missing, mismatched or unverified completion ownership stays HOLD, even after months of inactivity.
The manual executor requires this release too; an approved manifest does not bypass it.

All process, executable, file-handle, pin, lock, ownership, source verification, path identity,
symlink/mount, approval and durable audit gates still apply immediately before deletion. A
new tracked build reopens the task; later output invalidates its release. No automatic branch,
source worktree, Git object, ROM, DAT, production database, recovery evidence or USB-backup deletion
is authorised. The running Blue GUI target must remain pinned and must never be released while
in use. Source worktree removal remains a separate explicit action after verified retirement.

`--min-idle-hours` is an optional *additional* delay, default zero; it never replaces these
checks. Existing approved policies retain their recorded delay and digest. To remove an old
48-hour value, explicitly reapprove with `--min-idle-hours 0` and enable the new policy; do not
edit or silently migrate approval records. Exact manual manifests also use the zero default.

### Finishing a task: the required order
Finishing a Cargo command is not finishing a task. While the worktree still exists:

```
scripts/cargo-iso --finish-task --task <name>      # idempotent release + "may I remove it?"
scripts/emuwiz-cargo-maintain worktree-check <wt>  # CLEAR only when retired and history is preserved
```

Retirement verifies ownership from the source worktree, so the
worktree must stay until `worktree-check` reports CLEAR (artifacts retired with a verified
receipt/journal, or none was ever created: no record of a cargo-iso build and an absent or empty cache
directory). Missing artifacts alone prove nothing: they HOLD as `ARTIFACTS_MISSING_UNVERIFIED`. HOLD reasons: `NOT_RELEASED`, `OWNER_UNVERIFIED`, `COMPLETION_UNVERIFIED`,
`WRITTEN_AFTER_RELEASE`, `AWAITING_RETIREMENT`, `ACTIVE`, `PINNED`, `PROTECTED_REMNANTS`,
`SOURCE_MISSING`, `NO_LIFECYCLE_RECORD`, `RECEIPT_UNVERIFIED`, `ARTIFACTS_MISSING_UNVERIFIED`. The gate also holds on the checkout's own
Git state, because `git worktree remove` deletes its reflog, ORIG_HEAD, per-worktree refs and ignored files:
`CHECKOUT_NOT_CLEAN`, `CHECKOUT_INDEX_UNVERIFIED`, `CHECKOUT_PATH_UNVERIFIED`, `GIT_WORKTREE_PROTECTED`,
`GIT_HISTORY_NOT_PRESERVED`, `GIT_STATE_UNVERIFIED`. A commit counts as preserved only when a ref names it, so
retain orphans under `refs/preservation/`. `--cargo-state-only` reports the Cargo half alone and never authorises
removal. A path with no `.git` always holds, including one already removed correctly: the gate cannot
tell that from a checkout that vanished while still registered, so check before removing, not after.
A release record alone never
clears the gate or authorises deletion; releasing does not stop builds, remove pins or claim
processes have exited. The release command takes the target's exclusive lifecycle lock and
refuses while a `cargo-iso` build holds its shared lock; retry completion after that build
exits. The gate never removes anything, and there is no hook into
`git worktree remove`: a cleanup workflow must call `worktree-check` itself. `AGENTS.md` and
`scripts/task-postcheck.sh` carry the reminder. A completing workflow can use
`task-postcheck.sh --baseline FILE --allow PATH --complete-task NAME` to release only
after its checks pass. Without that explicit completion attestation, postcheck never
releases a task. Build registration durably records a task label (explicit `EMUWIZ_TASK`,
or the canonical worktree identity); completion also records source HEAD and worktree.

### Explicit reconciliation of a genuinely absent legacy target

`worktree-check` continues to report `ARTIFACTS_MISSING_UNVERIFIED` when historical
retirement cannot be proved. That cache-history result is independent of whether
an obsolete Git checkout can now be disposed of. The separate owner-invoked path is:

```
scripts/emuwiz-cargo-maintain worktree-reconcile <wt> --json
scripts/emuwiz-cargo-maintain worktree-reconcile <wt> --remove --json
```

Both commands return `historical_cache_retirement: UNKNOWN`. They never manufacture
retirement receipts, edit old records or delete external cache content. Any existing
target object (directory, file or even a dangling symlink) refuses this path and
requires the normal Cargo lifecycle. Incomplete or ambiguous registry ownership also
refuses; every checked record is considered, rather than dropping corrupt records.

The checkout must be a registered secondary worktree with no Git lock/operation,
tracked change, untracked file or ignored artifact. Index entries marked
assume-unchanged or skip-worktree are refused because Git status can hide their
local edits. HEAD must be contained in
`origin/main`; for an explicitly archived unmerged branch, `--preserved-ref refs/...`
must name an existing local branch/tag/archive/preservation ref at exactly HEAD.
This flag preserves code; it is not evidence that a feature was integrated.
`git worktree remove` also deletes the checkout's own HEAD reflog, `ORIG_HEAD` and
per-worktree refs (`refs/worktree`, `refs/bisect`, `refs/rewritten`), so any per-worktree
ref refuses, and every commit named by either ID of any HEAD reflog entry or by
`ORIG_HEAD` must be on `origin/main` or reachable from a ref (for example a
`refs/preservation/...` ref) or the command refuses with `GIT_HISTORY_NOT_PRESERVED`.
Git runs without inherited `GIT_*` variables and with replace refs and grafts ignored,
so none of them can make unmerged history look merged; `sequencer` counts as an
unfinished operation and submodule changes are never ignored. `FETCH_HEAD` is not
checked: commits named only there were fetched from a remote and are not preserved by
this command.
Normal campaign disposition must still identify the checkout as obsolete before
invocation. Valuable unfinished features remain retained until a recovery decision.

The command checks target absence using `lstat`, exclusive target ownership, pins,
recorded live jobs and a complete host process census for checkout and target.
Checkout inode identities are matched against that census so hard-link, bind-mount
and namespace aliases cannot bypass the open-handle check. Incomplete or cross-device
checkout identity scans refuse removal.
R2/R3 branch, path or recorded task identities retain an unresolved production
deployment HOLD, explicitly distinguished from proof of a currently active owner.
No deployment override is provided. Root operation requires the existing trusted
installation and `--as-user`; Git subprocesses run entirely as that owner.

`--remove` holds the target's exclusive lifecycle lock and registry metadata lock,
records an intent, repeats all checks, then invokes `git worktree remove` without
`--force`. It records `CHECKOUT_REMOVED` only after Git succeeds. An eligibility
result or historical intent is never read as reusable authorization: every call
revalidates target recreation, ownership, Git state and process references. Current
same-account nonparticipating writers cannot be prevented by Cargo locks; normal
Git refusal and immediate rechecks remain necessary. Reconciliation does not prove
that an old cache deletion was safe and does not weaken the cache retirement engine.

Focused synthetic regressions: `python3 scripts/test_emuwiz_legacy_worktree.py`.
A later wrapper build removes the release, including after successful retirement.

### Policy, budgets, cycle
`emuwiz-cargo-maintain policy approve --owners a,b [--soft-gb 220 --hard-gb 240 --max-targets N
--max-gb N]` records a persistent, digest-protected, bounded policy (target root, owner labels,
per-cycle limits; default idle delay 0 h, free reserve 100 GiB). It is **disabled** until `policy enable`, which writes a separate
enable record bound to the policy digest plus a history event; the `enabled` flag inside the policy
file is ignored, and re-approving requires a fresh enable. A malformed policy, one missing fields
(`scope.kinds`, limits), one owned by an account other than the effective user or registry owner
(root is not a wildcard), or a group/world-writable file or state directory fails closed. A generated
manifest hash is never the authorisation: the manifest must name the live policy digest and only
groups the cycle itself planned. `max-gb` is a hard upper bound on a cycle's estimated reclaim: a
candidate that would cross it is skipped (a smaller, younger one may still be taken).
`run` (one cycle) acts only when the policy is approved and enabled, the process census is
complete (needs root, see *Privileged execution*; a partial census blocks the cycle), either
the total exceeds the soft budget or free space falls below `--reserve-free-gib` (default 100), and only for RETIRABLE targets with a wrapper-recorded build (`last_start`) in the policy
scope, oldest first, within the per-cycle bounds. It writes a manifest outside every cache and hands
it to the executor **without** `--maintenance-window-attested`, so legacy or
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

### Policy state, ordering and cancellation (R2)
- **State machine.** `enable.json` (single state record: ENABLED/DISABLED, generation, policy digest)
  plus the append-only `history.jsonl`. Enablement is accepted only if the *whole* history parses
  (every line a JSON object, newline-terminated), policy transitions have strictly increasing
  generations, the record's generation is the highest, the final transition is the matching
  `policy_enabled`, and every transition since the latest approval names this policy digest.
  A replayed older ENABLED record or event, a duplicate/out-of-order/malformed generation, a torn or
  garbage line, or a missing history all leave the policy disabled. `policy approve` over an
  unreconcilable history renames it to `history.jsonl.unreconciled-<time>-<pid>` (never deletes it)
  and continues the generation counter.
- **Provenance.** policy, state record and history are read with `O_NOFOLLOW`, must be regular files
  owned by the effective user, the registry owner or root, not group/world-writable, and unchanged
  between lstat, fstat and a post-read lstat. The maintenance directory must be owned/trusted and
  not group/world-writable; the registry directory must be trusted and not world-writable.
- **Ordering.** approve, enable and disable take `.policy-transition.lock`. A destructive cycle
  holds the same lock from re-reading authority until the executor returns, so the linearisation
  point is that re-read under the lock. `disable`/`approve` called during a cycle WAIT (and print
  that they are waiting); a transaction already in the executor is never interrupted (that would
  break its journal) and completes whole. Once disable has returned, no cycle can start or continue:
  a later cycle re-reads DISABLED under the lock. Authority (policy digest + generation) is checked
  once more immediately before the executor is entered; a replaced policy, or a disable written
  directly into the state files, makes that check REFUSE the cycle.
- **Limits.** There is no independent trusted anchor: someone who can rewrite the policy, the state
  record and the entire history consistently (or root) is not detected. Cancellation of a running
  executor is not offered.
- **Budget variables.** `EMUWIZ_SOFT_BUDGET_BYTES`/`EMUWIZ_HARD_BUDGET_BYTES` must be positive
  integers up to 10^15; anything else is reported once and the default is used, so a bad value never
  blocks a build or `--finish-task`. Automatic retirement uses only the validated policy limits.

### Privileged execution (scheduled and manual: same rules)
A complete process census needs root; nothing else does. Both privileged entry points,
`emuwiz-cargo-maintain` (the scheduled cycle) and `emuwiz-cargo-retire` (manual cleanup), follow
the same rules whenever they start as root:

- **Trusted code only.** Before the script imports anything but `os`/`stat`/`sys` (built in or
  frozen) they require `python3 -I -S` and check that the script, every sibling it loads, the
  interpreter and every module directory, each with all parent directories, are root-owned and not
  group/world-writable, with no symlink. A Git worktree, or any copy an unprivileged account could
  change, is refused ("untrusted location"). This is a tripwire for a misplaced or modified copy:
  it cannot vouch for the interpreter binary itself or for the few stdlib modules the interpreter
  loads before any script code (`encodings`), so the interpreter and its library must already be in
  root-owned directories. `verify-install` runs the same check on demand (also unprivileged, which
  is how it should be run first) and prints the SHA-256 of the installed files; `units` refuses to
  write units for an interpreter or installation that fails it.
- **Explicit owner.** `--as-user OWNER` is required (never root's `HOME`/`USER` guess). The process
  sets its effective uid/gid and groups to OWNER and keeps uid 0 only as real/saved uid: every file
  open, create, replace and delete happens with OWNER's own permissions, so a planted symlink can
  only lead where OWNER could already write, and every lock, record, journal, receipt, manifest and
  history line stays owned by OWNER (no chown step, ever). Root's effective uid returns only around
  the read-only `/proc` census. Child processes (`git`) start with every uid and gid set to OWNER and
  no capabilities. The registry (`$EMUWIZ_RETENTION_DIR`, default `$HOME/.cache/emuwiz-cargo-retention`)
  and, for the maintenance commands, the target root must be real directories owned by OWNER.
- **Limited commands.** As root, `emuwiz-cargo-maintain` accepts only `status`, `plan`, `run`,
  `history`, `worktree-check`, `worktree-reconcile`, `units` and `verify-install`; policy changes, `task-complete` and
  `admit` are owner actions. `EMUWIZ_RETENTION_NOW` is refused.
- **Locks.** Lifecycle and registry locks are opened with `O_NOFOLLOW` and must be regular files owned
  by the acting user; otherwise the target is HELD (`LIFECYCLE_LOCK_UNAVAILABLE` / `LOCK_STATE_UNKNOWN`).
  The receipt directory must be a real directory owned by the acting user, not group/world-writable.
- **Stale releases.** Output written after the release was recorded (for example a build the wrapper
  could not track) holds the target as `WRITTEN_AFTER_RELEASE` (also in `worktree-check`): a reopened
  worktree is never retired on an old release record. `cargo-iso --finish-task` then records a new
  release (keeping the old time as `previous_at`). `emuwiz-cargo-retention` and `cargo-iso` refuse to
  run as root at all.

**Host prerequisite.** The interpreter's directories must be root-owned. If `verify-install` reports
`/usr/bin is owned by uid ...`, the host must be repaired first (`sudo chown root:root /usr/bin`, and
review any non-root-owned entries in it); until then the privileged tools refuse to start, by design.

**Installation** (from a clean checkout at the reviewed commit `$SHA`; nothing here enables anything):

```
L=/usr/local/libexec/emuwiz-cargo-maintain
for f in emuwiz-cargo-maintain emuwiz-cargo-retention emuwiz-cargo-retire; do
  git show "$SHA:scripts/$f" | sha256sum                    # expected hashes
done
sudo install -d -o root -g root -m 0755 "$L"
sudo install -o root -g root -m 0644 scripts/emuwiz-cargo-maintain scripts/emuwiz-cargo-retention \
     scripts/emuwiz-cargo-retire "$L"/
/usr/bin/python3 -I -S "$L/emuwiz-cargo-maintain" verify-install      # unprivileged first; compare hashes
sudo /usr/bin/python3 -I -S "$L/emuwiz-cargo-maintain" --as-user "$USER" verify-install
U=$(mktemp -d); /usr/bin/python3 -I -S "$L/emuwiz-cargo-maintain" units --output-dir "$U"
sudo install -o root -g root -m 0644 "$U"/emuwiz-cargo-maintain.service "$U"/emuwiz-cargo-maintain.timer \
     /etc/systemd/system/
sudo cat /etc/systemd/system/emuwiz-cargo-maintain.service   # review the INSTALLED text (ExecStart, paths)
sudo systemctl daemon-reload
```

The hashes are compared after the copy is root-owned, so a worktree file swapped during the copy is
detected. Take the expected hashes from a reference the unprivileged account cannot alter (for example
the reviewed-commit hashes recorded in the review report), not only from that account's own `git`.
The unit files pass through the owner's temporary directory, so review the installed text as root. `units` writes units for the installed copy only (`ExecStart=/usr/bin/python3 -I -S
$L/emuwiz-cargo-maintain --as-user OWNER run`, `NoNewPrivileges`, `ProtectSystem=strict` with the
registry and target root writable, a capability bounding set of `CAP_SETUID CAP_SETGID CAP_SYS_PTRACE
CAP_DAC_READ_SEARCH`); `--python` names another root-owned interpreter. To update, first make sure
no cycle is running (`systemctl is-active emuwiz-cargo-maintain.service`), then repeat the
copy and `verify-install`.

**Activation is a separate owner decision** (after `policy approve` and `policy enable` as OWNER):
`sudo systemctl enable --now emuwiz-cargo-maintain.timer`. To stop: `sudo systemctl disable --now
emuwiz-cargo-maintain.timer` (and `emuwiz-cargo-maintain policy disable` as OWNER). Check:
`systemctl list-timers emuwiz-cargo-maintain.timer`.

**Manual cleanup** uses the same installed copy:

```
X=/usr/local/libexec/emuwiz-cargo-maintain/emuwiz-cargo-retire
sudo env HOME=/home/OWNER /usr/bin/python3 -I -S "$X" --as-user OWNER --manifest M --approved-sha256 D
sudo env HOME=/home/OWNER /usr/bin/python3 -I -S "$X" --as-user OWNER --manifest M --approved-sha256 D \
     --execute --receipt-dir R
```

`HOME` (or `EMUWIZ_RETENTION_DIR`) selects OWNER's registry; a registry not owned by OWNER is refused.
Manifest digest, dry run, holds, census, locks, journal and receipts are unchanged.

Tests: `scripts/test_emuwiz_cargo_privileged.py` runs all of this as real root (`sudo -n`) on
disposable fixtures and a disposable root-owned installation; `EMUWIZ_TEST_SYSTEMD=1` also runs the
generated service once as a uniquely named runtime unit (never the production unit or the timer).

### Limits
Plain `cargo` that bypasses `cargo-iso` is not excluded by any lock: such targets are never
retired automatically. Caches whose worktree was removed first are held (`SOURCE_MISSING`).
Admission is advisory and cannot guarantee a physical cap. The policy, enable record and retirement
evidence are protected by ownership, permissions and unkeyed SHA-256 digests plus a cross-checked
history: this is structural verification, not authenticity. Anyone controlling the approving account
(or root) can forge a self-consistent set. Worktrees whose build produced no output are held rather
than guessed clear (`ARTIFACTS_MISSING_UNVERIFIED`) and need a human review. For a
genuinely absent legacy target, the independent checkout reconciliation above can
permit Git checkout disposal while keeping that historical uncertainty explicit.

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


### Lifecycle audit and operational safeguards (2026-10-17)

`scripts/emuwiz-cargo-lifecycle-audit EXISTING/cargo-classifications.json` explains an
existing inventory without walking artifacts. It joins durable lifecycle records and
exact source-path hashes from existing worktrees. A verified source association changes
UNKNOWN to HOLD with missing participation/completion; it never manufactures CLEAR,
registers a task, or grants deletion authority. Saved inventory is evidence for planning;
only fresh executor checks authorise retirement.

The saved 171-row inventory contained 151 HOLD and 20 UNKNOWN:

| Actual saved reason | Targets |
| --- | ---: |
| Released, time-only HOLD | 42 |
| Missing completion/participation, including 19 missing targets | 104 |
| Live process/build or pin | 5 |
| Missing lifecycle records | 20 |

The 42 time-only targets held 121.84 GiB in that snapshot. One (Blue eight-regressions)
subsequently reopened, so its old completion cannot be reused. The detailed per-target
explanation is preserved in the task audit ledger, including all remaining unknowns.
Neither successful Cargo exit, GitHub promotion nor branch ancestry proves inactivity.

The root-owned installed helper and enabled approved policy are required for automation.
The generated timer runs five minutes after boot and five minutes after the previous
cycle finishes, with up to thirty seconds jitter. It selects only released participating
per-worktree targets, within the approved byte/target caps. Unknown/shared/external caches
remain held. An absent policy or missing timer means there is no automatic cleanup;
changing checkout code alone does not update the privileged installed copy.

The executor refuses unreadable same-user processes as well as foreign processes,
checks the process census after selection and before each artifact subtree, matches
fd/executable/cwd/maps kernel identities to catch path aliases, refuses missing mount
visibility and same-device bind mounts, serializes pin/reopen metadata with deletion,
and verifies filesystem owners and parent path identities. A light PID-list check before
each unlink/rmdir triggers a new full census when a process arrives. Removed last-link
inodes leave the live identity set so inode reuse cannot falsely protect unrelated files. DAT/BIOS/ROM/media-looking
files, backups, history and audit evidence remain preserved even inside compiler folders.
A sandbox census is insufficient: production runs must use the installed host helper.
The PID1 check is a tripwire, not a portable proof of initial namespace identity.

A point-in-time census cannot atomically exclude an arbitrary uncooperative process
opening a file after the last check. Lifecycle locks exclude cooperating Cargo builds;
all development builds must use cargo-iso. Incomplete execution journals continue to
refuse further automatic deletion pending inspection; they are never auto-acknowledged
or discarded. A completed retirement can be resumed by building in the same worktree:
it recreates artifacts and reopens ownership under the lifecycle lock.

### Rustc incremental output after a partial retirement

Incremental caches are classified as one output unit, rather than treating every
`.bin` as either disposable or user data. A shared validator checks the observed
rustc crate/session directory grammar, paired zero-length session locks, complete
committed-cache sets, and the narrow interrupted `working`/`dep-graph.part.bin`
form. It recognises RSIC cache headers, Rust metadata and ELF/LLVM object headers,
reading at most eight bytes per compiler file. Unknown children, formats, special
files, owners, symlinks and filesystem boundaries refuse the unit. This does not
recognise arbitrary content simply because its directory is named `incremental`.

The planner can locate an orphan incremental unit in `debug`/`release`, including
under a target triple, after `.fingerprint` and `deps` have been retired. Other
profile/output classification (`deps`, `.fingerprint`, `build`, examples and
executables with `.d` files) is unchanged. `tests`/`benches`, unknown files and
source/worktree/audit/evidence paths do not acquire a new classification. If
incremental contents are unrecognised, planning preserves the profile's other
outputs too, so it does not remove their markers and strand incremental output.

Classification is not permission to retire. The existing exact target-root,
source ownership/HEAD, explicit completion release, lifecycle lease, live process,
pin, filesystem identity, manifest, snapshot, durable journal and metadata-fence
checks still apply. Only the complete validated incremental unit receives the
narrow exception for its proven compiler cache files. The executor records file
identity, ownership, size, mtime and ctime and rechecks immediately before unlink;
new or changed files stop retirement. For compiler hardlinks, a retained descriptor
proves the exact inode and unchanged content metadata when our own unlink advances
ctime; other planned aliases receive that transition. Final-link block accounting
remains separate from retained links. No generic `.bin` exception is introduced.

The non-Cargo route `bash scripts/test-cargo-retention.sh` includes realistic
synthetic rustc layouts and interrupted sessions, lease/completion/owner/pin and
path refusals, partial prior retirement, plan/execute separation, file replacement,
late user files and shared-inode accounting. Its wrapper tests use fake Cargo in
private temporary HOME/registry/targets; they never run a compiler.

Deployment still requires exact-commit independent safety PASS and matching hashes
for all three root-owned installed scripts. Keep scheduling stopped while a
coordinator-scoped cleanup is being reviewed/executed. A versioned installation
can be verified without changing or enabling the timer. A dry-run exact manifest
through `emuwiz-cargo-retire` lists the chosen paths and estimated bytes and does
not authorise execution. Automatic policy byte caps remain in force; a larger
single unit needs its own explicit coordinator-approved manual manifest. Preserve
test environments, unknown remnants and source worktrees even after retirement.

### Activation binding (2026-10-23)

Retirement needs BOTH `policy.json` `enabled: true` AND an enablement record (`enable.json` plus the matching
`policy_enabled` history event) whose `code_sha256` is the fingerprint of the `emuwiz-cargo-maintain`,
`emuwiz-cargo-retention` and `emuwiz-cargo-retire` scripts now running. Installing, upgrading, rolling back or editing
any of them, and any record without a fingerprint, leaves retirement DISABLED until the owner runs `policy enable` again.
A non-dry-run `run` revokes a stale ENABLED record (DISABLED transition, policy flag false).

Residual gaps (accepted, documented):
- Revocation happens on a `run`. If code B is installed and rolled back to the exact enabled code A before any `run`
  fires, A's approval still stands. Always run `policy disable` BEFORE replacing or rolling back the scripts.
- The fingerprint covers the three scripts only (not the interpreter or stdlib), and code replaced inside an
  already-running process is not detected; both need write access to the install directory (root).

### Explicit batches: `plan|run --only <target-name>` (2026-10-23)

Repeatable. The batch is EXACTLY the named targets or the whole request is refused: names must be bare registry names
(no path, wildcard, duplicate or empty value) and each must be a current auto-eligible target with retirable artifacts,
and the batch must fit the standing per-cycle target and byte caps. Nothing is ever substituted, dropped or added.
Everything else is unchanged (policy must be enabled for `run`, journals, HOLD, pins, process census, free space).
`run --only` plans again from a fresh snapshot after the manifest is written and refuses unless the same targets and the
same artifact paths (device and inode) are still planned. Omitting `--only` keeps the automatic behaviour.

Not atomic: the executor still judges each cache on its own, so a hold that appears in the last moments skips that cache
while others complete. The cycle report and `history.jsonl` (`batch`: requested, retired, remaining, atomic=false,
reclaimed_bytes) state what was actually retired; a partial batch is never described as complete.

An explicit batch ignores the "need" amount beyond requiring it to be positive (it may reclaim more than the soft-budget
need; the per-cycle caps still bound it). `plan --only` exits 1 when the request is refused, and `run --only` exits 1 when
the batch did not fully complete (see `batch.remaining`).

A target whose executor outcome is PARTIAL with freed bytes is recorded as retired in the registry (existing behaviour) while a batch lists it under `remaining`; trust `batch.remaining` and the per-target outcomes.

## No manual deletion route, and source-less caches

Every refusal of `emuwiz-cargo-retire` (`SOURCE_MISSING`, `NOT_ADOPTED`, `PINNED`, `BUILD_LOCK_HELD`,
`INCOMPLETE_PROCESS_VISIBILITY`, ...) is final. Never replace the tool with a script, `rm -rf`, `find -delete` or a copy of an
earlier one-off script: the pin, lock, process, identity, preservation and journal checks do not exist in a copy. If a
hold looks wrong, fix the tool (tests, review, install approval) or report the hold.

`SOURCE_MISSING` stays the default for a cache without a source checkout. The one exception is a group whose manifest sets
`"orphan_route": "GIT_PRESERVED"`, `"source_exists": false`, no source paths, `source_head` and `orphan_repo`. The tool then
verifies freshly and holds on anything unverifiable: a lifecycle record with a cargo-iso build and a 40-hex HEAD equal to
`source_head`; the target name bound to the recorded worktree path by its hash; the worktree path absent and not
registered by `git worktree list` in `orphan_repo`; the HEAD a commit contained in that repository's
`refs/remotes/origin/main`. Holds: `SOURCE_MISSING`, `OWNER_UNVERIFIED`, `ORPHAN_SOURCE_PRESENT`, `ORPHAN_REPO_UNVERIFIED`,
`GIT_HISTORY_NOT_PRESERVED`. Every other gate (completion release, pin, lock, census, identity, idle time, journal) applies
unchanged. Unidentified caches (no recorded build HEAD, such as the 2026-10-15 adopted scratch/source directories) cannot use
the route and stay hard `SOURCE_MISSING`.
