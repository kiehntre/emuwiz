# EW Foundation Brick 1B: atomic-text parent publication boundary

Candidate implementation evidence, pending independent exact-commit review.
Authoritative parent: `cb3205b7690221703de1fd600fa8996d195fe991` (promoted,
independently reviewed Brick 1A). This document adds the remaining parent
boundary investigation; it does not reopen that review or the emulator-update
review credited in `ATOMIC_TEXT_TEMP_OWNERSHIP.md`.

## Source and isolation

The original checkout HEAD was `eba83521309942d26c164f4bca9493fe4c017423`.
An actual `git fetch --no-tags origin`, fetched `origin/main`, and actual
`git ls-remote origin refs/heads/main` established the expected `cb3205b7` tree.
Remote main was checked again after baseline reproduction and before repair;
it remained exactly the expected full SHA. A final actual fetch and ls-remote
check before the local commit also matched; dedicated HEAD remained at that
parent until committing.

Worktree: `/home/davedap/emuwiz-brick1b-parent-boundary`, branch
`fix/brick1b-parent-boundary`, initially clean at that SHA. Available worktree
metadata and visible process ownership were checked before creation. No
overlapping Brick 1B owner was found; visibility does not establish that every
other worktree is idle. Dirty worktree source and uncommitted work were not used as evidence.

Allowed files: `crates/archivefs-core/src/atomic_text.rs`, its private
`atomic_text/parent_dir.rs`, `atomic_text/tests.rs`,
`atomic_text/tests/{parent_boundary,pinned_parent}.rs`, documentation on
`atomic_write_text` in `lib.rs`, and this document plus
`ATOMIC_TEXT_PARENT_BOUNDARY_EVIDENCE.json`. No caller production code,
dependencies, formats, GUI, other publication engines or real data changed.

Fixtures/builds used `/tmp/emuwiz-brick1b-q7karliu`, private HOME, XDG
CONFIG/DATA/CACHE/STATE/RUNTIME, TMPDIR, EMUWIZ CONFIG/DATA roots and target root.
The runner invokes `scripts/cargo-iso --low-debug`, retains cached Cargo/Rustup
tools, and reuses one worktree-specific target. Executed environment: Linux
6.8.0-142-generic, UID 1000, local ext4; no privileged mount or real-machine test.

## Confirmed reproduction and deduplication

**B1B-01: parent authority was not retained through publication.** This is one
root cause with multiple observable consequences, not five separate defects.
The baseline retained the staging file descriptor but resolved stage metadata,
destination metadata, rename, cleanup and directory sync through mutable paths
(`atomic_text.rs` at baseline: creation 41, name check 69, cleanup 90,
destination check 117, rename 185). Staging inode identity cannot prove which
directory contains that inode after it is moved.

Five deterministic fixtures ran before any production edit. The unchanged
baseline production files were checked with `git diff --exit-code` against the
exact parent. Existing Brick 1A thread-local hooks scheduled substitutions;
there were no probabilistic sleeps. The baseline run exited 101 with **five
expected regression-assertion failures**, rather than a compilation failure.

| Fixture | Actual baseline behavior | Required candidate behavior |
| --- | --- | --- |
| Parent moved aside after stage creation, replacement directory has foreign same-basename stage | `Err`; both destinations and foreign stage preserved, owned empty stage retained in original directory | `Err`; original owned stage cleaned, foreign stage untouched |
| Parent renamed, replacement installed, same staging inode moved into replacement | `Ok(())`; replacement destination overwritten with `new text`, original unchanged | `Err`; both destinations unchanged; externally relocated stage retained |
| Initially symlinked parent rebound after same staging inode relocation | `Ok(())`; second directory overwritten, original unchanged | `Err`; both destinations unchanged; relocated stage retained |
| Actual rename-journal writer with same relocation | `Ok(())`; Applied receipt written into replacement, original Planned receipt unchanged | `Err`; original receipt and unrelated replacement journal unchanged |
| Actual ES-DE recovery-record publication with same relocation | Outer `Ok(())`; replacement gamelist overwritten and recovery record removed; original gamelist unchanged | Outer `Err`; neither gamelist changed |

The plain replacement is an observed conservative refusal/retained-artifact
limitation, not evidence of foreign deletion. The four redirected successes
are reproduced failures. Unsafe pathname cleanup after the final observation
was a source-grounded race risk; it was not separately executed against the
baseline. Candidate-only `BeforeUnlink` tests validate the repaired boundary.

Machine-readable before/after bytes, mode, device, inode, link counts, parent
identities, symlink information and actual results are in the companion evidence
JSON. Raw full logs remain in the private task root; fixture directories are
disposable and are removed by their owning TempDir after snapshotting.

## Bounded contract and production implementation

1. Validate a raw final basename without normalizing trailing slash, `.` or
   `..` into a different destination. Relative and non-UTF-8 basenames remain
   supported. Create missing ancestors using the existing `create_dir_all`
   policy, then establish the final parent by opening a directory descriptor.
2. Linux uses `O_PATH | O_DIRECTORY | O_CLOEXEC`; it does not add a parent-read
   requirement. The original parent symlink policy remains: initial traversal
   follows symlinks. This is object binding, not a new symlink prohibition.
3. The descriptor precedes authoritative destination eligibility/mode reads and
   exclusive staging creation. `tempfile::Builder::make_in` supplies random
   names/retries only. Actual creation uses `openat(O_CREAT | O_EXCL |
   O_NOFOLLOW | O_CLOEXEC | O_RDWR)` against the retained descriptor. Requested
   existing mode or new-file 0666 remains subject to kernel umask/default ACLs;
   process-wide umask is never changed.
4. Held staging-file metadata and `fstatat(AT_SYMLINK_NOFOLLOW)` under that same
   parent must agree on regular type, single link and device/inode. Writes,
   required chmod and file synchronization still use the retained staging file.
5. `renameat` uses two single basenames and the same directory descriptor.
   `renameat2(RENAME_NOREPLACE)` is deliberately not used: it would break the
   existing intentional destination-replacement contract. Publication cannot
   re-resolve a substituted parent pathname even after the last identity probe.
6. Failure cleanup checks custody under the original descriptor and uses
   `unlinkat`. A foreign entry in a replacement directory is never the cleanup
   target. An owned stage moved outside the pinned directory is retained; there
   is no search-and-delete or authority over its new containing directory.
7. Parent pathname metadata is compared with the held directory's device/inode
   after establishment, after creation, before rename and after publication.
   These observations diagnose rebinding; they are not the operation authority.
   Alias/symlink changes still resolving to the same object remain allowed.
8. Best-effort directory sync reopens `.` through the directory descriptor and
   retains the existing ignored-sync-error policy. This changes the addressed
   object, not the durability contract.

Source references below are relative to `crates/archivefs-core/src/` unless
another crate is named. Implementation references: `atomic_text.rs:28` (owned creation), `:84` (custody),
`:105` (cleanup), `:164` (parent establishment), `:230` (publication), `:234`
(post-publication error); `atomic_text/parent_dir.rs:17` (basename), `:43`
(directory opening), `:63` (binding observation), `:83` (exclusive creation),
`:97` (metadata), `:114` (replacement), `:127` (cleanup), `:132` (sync).

These primitives' documented invariants come from the Linux man-pages project:
[open/openat](https://man7.org/linux/man-pages/man2/open.2.html) explains stable
directory references and O_PATH permissions;
[renameat](https://man7.org/linux/man-pages/man2/rename.2.html) binds relative
names to directory descriptors;
[unlinkat](https://man7.org/linux/man-pages/man2/unlink.2.html) does the same for
removal; [fstatat](https://man7.org/linux/man-pages/man2/stat.2.html) supplies
descriptor-relative metadata and final-symlink inspection. Basename validation
matters because absolute names would bypass the directory descriptor.

## Failure, receipt and outer-result semantics

Observed substitution before rename returns `Err` and attempts only original-
directory, custody-checked staging cleanup. Required permission errors continue
to refuse before publication. Cleanup uncertainty remains explicitly reported.
On the reproduced local ext4 refusals, both destination files remain unchanged.
That is not a universal guarantee for every filesystem rename error.

If substitution happens after the final probe, rename can complete in the
**original pinned directory only**. A detected post-rename binding loss returns
an error explicitly saying publication completed there and requires inspection
before recovery/retry. It must not run staging cleanup after successful rename:
that name could now belong to another operation. Tests install such a foreign
entry after publication and verify it survives. No attempt is made to undo an
already completed rename or to modify the replacement directory.

The actual journal writer tail-calls the helper (`dat/rename_apply/journal.rs:88`).
The executor propagates initial and subsequent journal errors through `?`
(`executor.rs:313,335,415`) and returns outer `Err(ApplyError::Journal)` on this
refusal; a dedicated test proves source bytes/inode and transaction entry state
remain unchanged on an initial-journal refusal. Other cancellation/entry-failure
paths can intentionally return `Ok(ApplyOutcome)` carrying failed inner state;
this brick does not change or certify those unrelated outcomes.

ES-DE uses `write_recovery_record(...)?` before gamelist mutation
(`launch/es_de_publish.rs:518`), maps gamelist-helper errors to an outer `Err`
(`:525`), and forwards recovery-writer errors (`:639`). The direct false success
reproduced above came from helper `Ok`, not conversion of helper `Err`. An
existing internal comment at `:527` says a failed gamelist write changed
nothing; it is not a valid inference for post-rename errors. Returned error
details now explicitly identify the helper's partial outcome; no GUI/caller
production edits are included here.

Conversion queue commits return helper errors and poison the owner before
updating the in-memory snapshot (`conversion_queue/durable.rs:713,721,724`). Patch
journal writes map/propagate errors (`patch_output_recovery.rs:293`). Library
Views manifest writes propagate through `?` before outer report success
(`library_views.rs:2087`). Its separately implemented best-effort history uses
a report warning; it is not a shared-helper success conversion. Rename GUI
workers send helper-derived errors as Failed
(`crates/archivefs-gui/src/dat_sources_page.rs:4571`),
handled as `apply_error` (`crates/archivefs-gui/src/dat_sources_page.rs:3638`). This is source inspection plus focused core
tests, not executed GUI certification.

## Dependency impact

The 28 direct call sites in 19 existing source files remain unchanged. The
complete grouped register is the Brick 1A document's consumer table, with
configuration, profiles, rename journals/recovery History, DAT state/registries,
managed identity snapshots, conversion queue, patch-output recovery, Library
Views config/manifests, cheat-source config and ES-DE text publication.
`lib.rs:2882` still exposes exactly `atomic_write_text(&Path, &str) -> Result<()>`.
Independent emulator-update, installer, database, Duplicate Undo, PS2 and
Library Views symlink engines are not consolidated or promoted here.

## Executed validation

The baseline reproduction command is:

```text
scripts/cargo-iso --low-debug test --offline --locked -p archivefs-core \
  --lib atomic_text::tests::parent_boundary -- --nocapture
```

All final selections used the unchanged final production/test source after the
first bounded implementation attempt. No candidate Cargo test failed.

| Selection after `scripts/cargo-iso --low-debug test --offline --locked -p archivefs-core` | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| `--lib atomic_text::tests -- --nocapture` | 42 | 0 | 0 |
| `--test atomic_text_temp_ownership -- --nocapture` | 6 | 0 | 0 |
| `--lib save_source_folder_configs -- --test-threads=2` | 2 | 0 | 0 |
| `--lib mount_root_update -- --test-threads=2` | 4 | 0 | 0 |
| `--lib set_mount_root -- --test-threads=2` | 4 | 0 | 0 |
| `--lib master_rom_root_round_trips -- --test-threads=2` | 1 | 0 | 0 |
| `--lib emulator_profile_resolver::tests -- --test-threads=2` | 28 | 0 | 1 |
| `--lib patch_manager::emulator_profile_memory::tests -- --test-threads=2` | 14 | 0 | 0 |
| `--lib dat::rename_apply:: -- --test-threads=2` | 138 | 0 | 0 |
| `--lib dat::updates::tests -- --test-threads=2` | 72 | 0 | 0 |
| `--lib conversion_queue::durable::tests -- --test-threads=2` | 45 | 0 | 0 |
| `--lib patch_output_recovery::tests -- --test-threads=2` | 5 | 0 | 0 |
| `--lib library_views::tests -- --test-threads=2` | 105 | 0 | 0 |
| `--lib launch::es_de_publish::tests -- --test-threads=2` | 29 | 0 | 0 |
| **Final total** | **495** | **0** | **1** |

The ignored test is `real_machine_duckstation_auto_resolution_report`; it was
not enabled. Two counted passing tests are inert child entry points; parent
tests actually exercise process death, umask, short write and concurrency in
private child processes. The 42 atomic unit tests include all 26 unmodified Brick
1A tests plus the five identical baseline reproductions and eleven new boundary
cases. The public ownership integration suite is unchanged and passes all six.

New syscall barriers cover staging creation after a parent probe, rename after
the last probe, symlink rebinding after the last probe, cleanup after its last
custody observation, disappearance, and post-publication name reuse. They also
check modes, write/search-only parent access, same-object aliases, non-UTF-8
basenames and rejection of suffixes that could otherwise normalize into writes.
The executor test independently exercises an outer journal error before any
synthetic source rename. No probabilistic parent-race test is used.

Baseline reproduction exited 101 with 0 passed/5 expected failures (357.62 s,
including the initial isolated build). Final atomic suite passed in 249.69 s,
including its rebuild; tests themselves took 0.19 s. Consumer suites reuse that
same target. A private regression-driver syntax error was corrected before it
launched any Cargo command; this was a harness preparation error, not a
candidate test failure. Existing unrelated core warnings were left unchanged.

Targeted rustfmt checks, scope postcheck, GUI-root boundary guard and
`git diff --check` passed. No full-workspace or GUI build/test ran. Commands,
results and log hashes are recorded in the evidence JSON. Existing Brick 1A
consumer coverage is reused as a suite selection, not presumed to certify
unexecuted paths or every caller's GUI integration.

## Limits and independent-review acceptance

Executed evidence is Linux/ext4 only. NTFS, exFAT, FUSE, network filesystems,
default ACL/group layouts, unavailable/remounted drives and privileged bind
mounts have no new validation/capability detector. Other Unix targets use a
descriptor with an unverified O_RDONLY fallback that requires parent read
permission; Windows support is not added. No cross-platform guarantee is made.

This does not freeze a pathname, serialize writers, protect initial ancestor
creation/traversal, or bind an earlier caller validation to the helper's later
open. It pins the object established **inside one helper call**. An unsampled
change/restore can evade diagnostics; a change after the last post-publication
probe can happen before return. Syscalls remain bound to the original descriptor
in both cases. Separate later helper calls establish their own boundaries;
whole multi-step workflows are not converted into directory transactions.

Hostile same-UID or permission-authorized interference can still alter a stage,
swap leaf names after a custody check, add aliases, change destination modes or
replace destination entries. There is no atomic conditional leaf rename/unlink,
cross-process lock, ACL ownership policy, destination identity custody or
last-writer-wins change. Device/inode observations do not certify mount identity
or server honesty. Crash durability, post-rename process death, stale remote
metadata, server-side rename-error-after-publication and hardware caches retain
the previously documented limits. `Err` is not universal proof of no mutation.

Independent review must inspect the exact final commit and its parent; rerun
the five identical baseline repros against the parent (test-only transplantation),
then against the candidate; exercise the final syscall barriers; verify every
at-operation name is a single component, every entry syscall uses the original
descriptor, FD ownership/closing and unsafe-wrapper validity, mode/umask behavior,
and the two distinct error phases. Preserve all Brick 1A custody tests and the
representative caller suites. Review must not treat implementation-owner test
results as an independent PASS. No production promotion is authorized.

There is no on-disk migration or dependency/lockfile change. The candidate is
one local commit; reject/revert that one commit before promotion if review
fails. Rollback would restore the documented parent-boundary limitation.
