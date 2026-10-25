# EW Foundation Brick 1A: shared atomic-text temporary ownership

Implementation candidate for independent exact-commit review. Parent baseline:
`31eecd87d03ce92c678b3a21c62db8f08276441e`. No promotion, release, general
filesystem safety certification, or wider journal redesign is implied.

## Isolation and source of truth

Remote main was checked with `git ls-remote origin refs/heads/main` before work
and before production edits. It matched the expected baseline. A fresh worktree,
`/home/davedap/emuwiz-brick1a-atomic-text`, was created on
`fix/brick1a-atomic-text-owned-staging` with a clean scope baseline.

Git worktree metadata was inspected for 93 registered worktrees; scoped dirty
files in nine older worktrees were left untouched. No active overlapping owner
was found in the available ownership metadata or visible process working
directories. Dirty worktree source was not inspected or treated as authoritative.
These observations cannot prove the absence of an invisible or idle owner.

All Cargo commands used `scripts/cargo-iso --low-debug`, `--offline --locked`,
private HOME/XDG/TMP/application roots, and one worktree-specific target under
`/tmp/emuwiz-brick1a-i6_4brrb/targets`. Fixtures ran as UID 1000 on ext4. No
workspace-wide build, real ROM/save/catalogue/installation mutation, push,
merge, release, or cleanup of another owner's files was performed.

## Reproductions before production changes

The public-API harness in
`crates/archivefs-core/tests/atomic_text_temp_ownership.rs` ran on unchanged
baseline production code before the first production edit. Each case runs in
a fresh process, so the original first temporary name is
`.archivefs-config-write-<pid>-0.tmp`. Four cases exercise
`save_source_folder_configs_to`; the fifth exercises the actual rename-journal
writer. The assertions demand preservation and are unchanged in meaning after
the correction: baseline exit 101, five failures, one inert child entry passed.

Full before/after bytes, device/inode, link count, mode, size, mtime/ctime,
symlink targets, destination state and actual Result are checked in as
[synthetic evidence](atomic_text_temp_ownership_evidence.json). Byte arrays are
exact; no real user data or credentials appear. Inodes are specific to these
disposable runs and must not be compared between runs as persistent identities.
All objects below have device 64512.

| Original case | Baseline observed result | Final candidate observed result |
| --- | --- | --- |
| Existing regular temporary | Foreign inode 7768227 appropriated as destination; 27-byte sentinel replaced by 150-byte TOML. Returned `Ok`. | Foreign inode 7768247, bytes and metadata unchanged; destination 7768244 replaced by distinct inode 7768252, 150-byte regular TOML. |
| Symlink to unrelated file | Unrelated inode 13286259 changed from 19 to 150 bytes; symlink inode 13286262 published as destination. Returned `Ok`. | Symlink inode 7768256 and unrelated bytes/identity unchanged; destination 7768248 replaced by regular inode 7768264. |
| Hardlink to unrelated file | Shared inode 13286264 changed from 19 to 150 bytes and became destination; link count remained two. Returned `Ok`. | Shared inode 7768245 and both links/bytes unchanged; destination 7768242 replaced by independent inode 7768249, link count one. |
| Failed legacy creation | Read-only foreign inode 7768230 (28 bytes, 0444) deleted after `PermissionDenied`; old destination inode 7768228 unchanged. | Foreign inode 7768257 remains 0444 with identical bytes/metadata; an independently created stage successfully replaces destination 7768251 with inode 7768263. |
| Journal publication | Foreign inode 13286267 appropriated as journal destination, 27 to 249 bytes. Returned `Ok`. | Foreign inode 7768261 unchanged; journal destination 7768259 replaced by inode 7768267; valid 249-byte JSON with unknown field preserved. |

Initial destination bytes in all cases are `original destination\n`; unrelated
bytes are `unrelated sentinel\n`. Foreign regular/journal temporary bytes are
`foreign temporary sentinel\n`; the read-only bytes are
`read-only foreign temporary\n`. Generated TOML/JSON contains that run's fixture
path, so cross-run payload comparisons must account for the different roots.
All five final API calls returned `Ok`; every foreign temporary and unrelated
snapshot remained identical. The original weakness and its five manifestations
are one deduplicated ownership defect, not five separate vulnerabilities.

Baseline reproduction log SHA-256:
`29ee01c228c97381ecab11a9e5ef5005a74792e4157d12b7a666bfe992d1d66b`.
Final reproduction log SHA-256:
`c9b89473fe09a79ac46711d7abfec3d6c584df83f0d70d6e0cb702a6deb31b49`.
Raw logs remain under the private root; their paths and hashes are in the
checked-in evidence. Fixture directories were disposable, not real libraries.

## Candidate ordering regression found before freeze

Final review found that mode application before file sync had moved the last
eligibility observation of the destination earlier than the baseline's
post-sync refusal. A new test substituted a destination symlink at the pre-sync
hook using real filesystem operations. The intermediate candidate returned
`Ok`, published regular inode 7768239 over that link, and failed the preservation
assertion (exit 101; one failed test). This was a candidate regression, never a
promoted-main finding. The unrelated bytes remained unchanged.

The corrected helper rechecks destination eligibility after sync, immediately
before rename (`atomic_text.rs:179`). The same final test returns a Config error,
retains the planted symlink and unrelated bytes, preserves the fixture's retained
old destination, and removes only its owned stage. It remains a scheduling-hook
reproduction, not a power-loss or uncontrolled external-process experiment.
The exact before/after outcomes, failing-module SHA-256 and log hashes are in
[the evidence](atomic_text_temp_ownership_evidence.json). The ordinary final
check/use race still exists; this restores the previous refusal location.

## Production contract and changed files

The production diff is limited to three approved paths:

* `crates/archivefs-core/src/lib.rs:2880`: retains the existing crate-private
  `atomic_write_text(&Path, &str) -> Result<()>` API and forwards to a private
  module. Directory synchronization remains the existing best-effort helper.
* `crates/archivefs-core/src/atomic_text.rs:22`: exclusively creates a same-parent
  `NamedTempFile` through the already locked `tempfile` 3.27.0 dependency.
  Its Unix backend uses `OpenOptions::create_new(true)`; collisions are never
  opened/truncated. Random names improve collision avoidance, but exclusive
  creation is the ownership boundary. Cargo manifests and lockfile are unchanged.
* `crates/archivefs-core/src/dat/rename_apply/journal.rs:1` and `:72`:
  documentation only; clarifies file sync, atomic replacement and best-effort
  directory sync without a universal power-loss durability claim.

`atomic_text.rs:67` compares the retained descriptor and the current temporary
entry against the created device/inode and requires a regular file with one
link. It checks before writing, before publication and before explicit cleanup.
`NamedTempFile` pathname destruction is disabled from construction. A failed
exclusive creation grants no cleanup authority. Metadata failure after creation
retains the entry and reports its path instead of guessing ownership.

Writes, flush, chmod and file sync all use the retained handle
(`atomic_text.rs:78`, `:145`, `:151`, `:168`). An existing destination's mode is
requested at creation, applied before content is written, and reapplied from
the current regular destination before sync. Chmod errors refuse publication.
New destinations request 0666 and let the kernel apply umask/default ACLs;
production code never reads or changes the process-wide umask. This preserves
the previous new-file creation policy rather than adopting tempfile's usual
0600 default. Mode copying is not ownership or full ACL/xattr copying.

Publication remains intentional `fs::rename` replacement of an eligible
destination, not no-clobber destination creation (`atomic_text.rs:185`). Public
APIs, serializers, newline conventions, journal formats and legitimate regular
replacement remain unchanged. Replacing a hardlinked destination affects its
selected name while preserving the old bytes under its other aliases; staging
hardlinks are refused. Temporary names keep the old prefix and `.tmp` suffix,
but replace PID/counter naming with randomized names.

## Failure, interruption and recovery semantics

Creation failure leaves any pre-existing candidate name alone. Pre-publication
write/chmod/sync/eligibility errors return failure and attempt cleanup only
after the owned descriptor/name checks. Observed substitution, added hardlinks,
or inability to establish ownership retain the named entry. Cleanup refusal or
failure is included alongside the primary cause and an inspection path
(`atomic_text.rs:93`). Composite errors retain the primary I/O kind, although
the composite source does not retain its raw OS error code.

Panic or process death closes the descriptor but retains a possible orphaned
stage; no pathname destructor removes a substituted object. The SIGKILL test
proves an old local destination survives death before publication, with a
complete stage retained. It does not simulate power loss. There is no new
automatic cleanup, transaction receipt, Undo algorithm, or staged-file replay.
Old journals still parse through unchanged readers, including unknown fields;
old temporary files are not claimed as owned or migrated. Uncertain retained
entries need inspection, not blind deletion based on the prefix.

Errors whose initial destination metadata is unreadable now refuse early.
Required chmod failure no longer silently publishes default permissions.
Permission-restricted, read-only, disappeared or unsupported storage can
therefore produce more failures; that is an intentional compatibility cost.
Destination eligibility errors and cleanup diagnostics may have different
wording. Parent directories newly created before a later failure may remain.

## Direct dependency impact

Targeted source tracing found 28 direct call sites across 19 files. This is the
shared publisher's caller inventory, not a repeated whole-codebase inventory.
Paths below are relative to `crates/archivefs-core/src/` at this candidate.

| Consumer | Source and call lines |
| --- | --- |
| Configuration/source folders and mount setting | `lib.rs:2500,2567` |
| Profile selection and remembered profiles | `emulator_profile_resolver/persist.rs:100,112`; `patch_manager/emulator_profile_memory.rs:97,115` |
| Rename journal and History/recovery sidecar | `dat/rename_apply/journal.rs:88`; `dat/rename_apply/history.rs:217` |
| DAT acquisition, registries, source configuration and state | `dat/acquisition.rs:303`; `dat/custom_sources.rs:305`; `dat/managed_sources.rs:405`; `dat/sources/config.rs:270`; `dat/updates.rs:1169`; `dat/tosec_release_pack/mod.rs:741` |
| Managed identity-source snapshots/import/lifecycle | `identity_source/managed_snapshot.rs:863,937`; `identity_source/no_intro/managed_lifecycle.rs:168`; `identity_source/no_intro/pack_import.rs:522,557` |
| Conversion queue snapshot | `conversion_queue/durable.rs:713` |
| Patch-output recovery journal | `patch_output_recovery.rs:293` |
| Library Views settings/manifest | `library_views.rs:576,732` |
| Cheat-source preferences | `patch_manager/cheat_source_registry/config.rs:132` |
| ES-DE gamelist, rollback text and recovery record | `launch/es_de_publish.rs:525,585,639,720` |

Independent publication engines are unchanged. In particular, emulator-update
receipts/publication use their existing safety engine rather than this helper.
The independent Library Views symlink engine, installer, databases, duplicate
Undo and PS2 preservation candidates are not consolidated or promoted here.

## Executed assurance coverage

Commands used the private runner at
`/tmp/emuwiz-brick1a-i6_4brrb/cargo_run.py`, which invokes the repository wrapper
and isolates HOME, XDG_CONFIG/DATA/CACHE/STATE/RUNTIME, TMPDIR,
EMUWIZ_CONFIG/DATA_HOME and EMUWIZ_TARGET_ROOT. Existing Cargo/Rustup tool caches
were retained; builds reused one target. No destructive real-machine test ran.

Final baseline for each command is the candidate's final production/test source.
No subsequent production or Rust-test edits were made after these runs.

| Command selection after `scripts/cargo-iso --low-debug test --offline --locked -p archivefs-core` | Passed | Failed | Ignored |
| --- | ---: | ---: | ---: |
| `--test atomic_text_temp_ownership -- --nocapture` | 6 | 0 | 0 |
| `--lib atomic_text::tests -- --nocapture` | 26 | 0 | 0 |
| `--lib save_source_folder_configs` | 2 | 0 | 0 |
| `--lib mount_root_update` | 4 | 0 | 0 |
| `--lib set_mount_root` | 4 | 0 | 0 |
| `--lib master_rom_root_round_trips` | 1 | 0 | 0 |
| `--lib emulator_profile_resolver::tests` | 28 | 0 | 1 |
| `--lib patch_manager::emulator_profile_memory::tests` | 14 | 0 | 0 |
| `--lib dat::rename_apply::` | 138 | 0 | 0 |
| `--lib dat::updates::tests` | 72 | 0 | 0 |
| `--lib conversion_queue::durable::tests` | 45 | 0 | 0 |
| `--lib patch_output_recovery::tests` | 5 | 0 | 0 |
| `--lib library_views::tests` | 105 | 0 | 0 |
| `--lib launch::es_de_publish::tests` | 29 | 0 | 0 |
| **Total final selections** | **479** | **0** | **1** |

Two passing entries are inactive child entry points; their parent tests run
child-process assertions. The ignored test is
`real_machine_duckstation_auto_resolution_report`, intentionally inspecting a
real machine. It was not enabled. Consumer commands used `-- --test-threads=2`.

New adversarial tests exercise post-sync destination-symlink refusal, forced allocator collisions with regular files,
symlinks, hardlinks and directories; regular/symlink/hardlink/directory stage
substitution; held-handle writes/chmod after substitution; added staging links;
cleanup substitution; panic; inaccessible/non-directory parents; actual rename
and cleanup permission failure; actual RLIMIT_FSIZE short write/EFBIG; actual
SIGKILL; actual two-process publication; new-file umasks 000/002/022/027/077;
restricted creation mode and legitimate destination hardlink replacement.
Chmod syscall errors and sync errors are injected test hooks, not evidence of
actual chmod failure on NTFS/exFAT/FUSE. Hooks are thread-local and test-only;
exclusive allocation uses the production tempfile backend even under collision
injection. Child-only resource limits, signals and umask changes do not alter
the parent application's environment.

Targeted rustfmt checks, the GUI-root boundary guard, scope guard and
`git diff --check` also passed. Existing warnings about `CompanionBasis`,
`ExtractEntry.hash` and `FilePlan.before` were not repaired. Baseline's five
expected reproduction failures and the focused ordering-regression failure above
are the only failing runs. Initial 23/25-test runs passed; the final 26-test suite
and all final representative selections passed.

## Assurance limits and remaining races

Passing tests establish these observations, not universal safety.

* Accidental collisions: exclusive creation prevents truncation/appropriation;
  both actual legacy collisions and forced allocator collisions were tested.
* Cooperating processes: staging names/inodes are independent, but publication
  remains last-writer-wins. This helper provides no cross-process serialization,
  journal generation comparison or application-level conflict resolution.
* Other users: no new parent ownership, ACL, sticky-directory or group-policy
  check exists. OS namespace permissions remain essential. Mode preservation
  does not copy destination owner/group, ACLs, labels or extended attributes.
* Hostile same-UID interference: descriptor ownership does not prevent another
  process from changing bytes/mode, adding aliases, or swapping names after a
  check. Checks detect observed substitutions; conditional rename/unlink is not
  provided. The final identity-check/use windows remain. There is no assurance
  that a racing attacker cannot cause foreign publication or cleanup.
* Parent resolution: symlinked ancestors remain followed; there is no pinned
  parent, dirfd-relative publication or unavailable-drive identity receipt.
  Directory substitution and mounts disappearing remain outside this slice.
* Durability: file sync happens before rename; directory sync is best effort.
  Power loss, post-rename death, hardware write-cache behavior, remote-server
  durability, stale network metadata and rename-error-after-publication are
  unverified. An error is not universal proof that nothing was published.
* Environments: executed coverage is Linux/ext4 only. NTFS, exFAT, FUSE, network
  filesystems, default ACLs, group-writable layouts and non-Unix targets were
  not tested. There is no new capability detector or platform port; the module
  uses Unix metadata/mode APIs, consistent with existing Unix core consumers.
* Consumer coverage: requested representative suites passed; not every path in
  all 19 caller files, GUI presentation, real installed frontend/emulator,
  mixed-version writer or downstream destructive workflow was tested. Existing
  ES-DE compatibility tests do not independently close its preservation ledger.

## K01/K02 audit-register evidence correction

For EW-FOUNDATION-P1, preserve IDs K01 and K02 and credit the owner's previously
supplied **independent exact-tree PASS WITH CONDITIONS** for
`31eecd87d03ce92c678b3a21c62db8f08276441e`. Those emulator-update corrections
are promoted on main and independently reviewed, not merely local candidate
claims. This Brick executes no duplicate emulator-update review.

The promoted source/commit and
[EMULATOR_UPDATE_SAFETY.md](EMULATOR_UPDATE_SAFETY.md) document the retained
conditions: updates are not enabled for general use; local ext-family/Btrfs/tmpfs
only; network/FUSE/case-folding storage refused; legacy records without ownership
receipts review-only; hardlinked executables refused; old lock-file builds must
not run concurrently; unknown process visibility blocks eligibility and commonly
makes updates unavailable on multi-user hosts. External launches and same-UID
check/use races remain. The owner's independent verdict is prior supplied
evidence; the current source trace here is not a replacement review.

This addendum does not change IDs, reevaluate K03–K09, or credit unpromoted
ES-DE, database, installer, Duplicate Undo or PS2 candidates as main fixes.

## Independent review and integration

Review the exact local commit reported with this document, whose parent must be
the full baseline SHA above. Use a separate clean, reviewer-owned checkout and
private HOME/XDG/TMP/application roots. Do not edit this frozen candidate or read
dirty implementation worktrees. Run Cargo only through `scripts/cargo-iso
--low-debug`, with one target per checkout and synthetic fixtures.

1. Compare the exact-tree diff to the three approved production files. Confirm
   the journal diff is comments only, APIs/serializers are unchanged, and
   `Cargo.toml`/`Cargo.lock` and other safety engines are untouched.
2. Inspect the locked tempfile Unix backend: exclusive creation, requested
   creation mode and disabled pathname destructor must remain true in the
   resolved dependency. Inspect descriptor-only write/chmod/sync and the
   identity/link checks before publication/cleanup; do not treat those checks
   as atomic conditional namespace operations.
3. Independently reproduce the baseline five cases using only the integration
   harness in an isolated baseline checkout; do not patch baseline production.
   Expect five preservation assertions to fail. Compare exact recorded outcomes,
   not fixture-specific inode numbers. Alternatively inspect retained baseline
   evidence, explicitly recording any reproduction not independently executed.
4. Run the final 26-test unit suite, six-entry public-API suite, and the matrix's
   consumer selections. Confirm injected errors versus real syscalls/process
   events, mode behavior, preserved foreign objects, retained uncertainty and
   old-journal/ES-DE text compatibility. The real-machine ignored test stays
   ignored. Review caller error handling without treating passing compatibility
   tests as a GUI truthfulness certification.
5. Return PASS, PASS WITH CONDITIONS or FAIL against the exact final SHA, with
   evidence and the limits above. No independent verdict has been issued for
   this candidate by its implementation owner. Security-sensitive promotion
   requires this independent exact-commit review under `AGENTS.md`.

Do not combine this candidate with unreviewed preservation candidates. Owner
approval and integration checks must precede any promotion. The bounded change
has no schema migration; rollback is a revert of this one commit, preserving
existing journals and retaining uncertain temporary artifacts. Reverting also
restores the reproduced weakness. Do not use rollback as authorization to
delete leftover stages. Baseline changes or overlapping active ownership require
stopping integration and establishing a new reviewable baseline.
