# FD-bound publication experiment

This is a Linux core-unit-test experiment, not an enabled Undo implementation.
`identity.rs` includes the probe only under `cfg(all(test, target_os = "linux"))`.
Legacy Undo, compact recovery, approval handling and application callers are unchanged.

The probe opens a regular file with `openat(O_RDONLY | O_NOFOLLOW | O_CLOEXEC |
O_NONBLOCK)` against a directory bound by the existing `RenameDirs` helper.
It retains that descriptor, compares its inode/device to the approval, and
uses the existing full identity reader with before/after descriptor snapshot
checks. Publication is exactly:

```text
linkat(AT_FDCWD, /proc/self/fd/<retained-fd>, destination-dirfd,
       destination-basename, AT_SYMLINK_FOLLOW)
```

There is no pathname-source fallback, `AT_EMPTY_PATH`, unlink or automatic
garbage collection in the primitive. The destination directory stays pinned.
After a successful link, sync or verification failure is reported as a link
that needs review, never as an unchanged filesystem.

The intended guarantee is narrowly about the inode linked by this syscall.
Replacing the old source/holding basename cannot redirect it to another inode.
An existing writable descriptor can still change that inode's contents; a
writer can replace the destination afterward; a moved destination directory
can invalidate the approved pathname. These do not become complete successes.
This is not protection against process-memory/descriptor-table manipulation,
approval-store rollback, or hostile modifications of recovery records.

Ordinary owned regular files on local ext4 were tested with UID 1000, no
effective capabilities and `protected_hardlinks=1`. Publication needs procfs,
permission to link the file, a writable destination and compatible mounts.
An occupied destination returns EEXIST. Removing every prior link makes the
ordinary pinned file unpublishable (ENOENT); a retained hardlink can keep it
publishable. Cross-filesystem linking returns EXDEV. Maximum link-count and
other filesystem/storage failures remain error conditions, not fallback
permissions. No root, foreign-owner fixture or link-limit exhaustion was used.

Publication creates an additional alias. The retirement probe can capture the
old name and return an observed foreign regular file through its own verified
descriptor without overwrite. It deliberately retains all holding entries.
Even a verified expected entry cannot safely be unlinked later by name while
another writer can replace that name. Foreign returns may create duplicate
links; blocked returns and substituted holding slots require manual review.
No retirement outcome certifies a completed Undo.

The original failed pathname-publication experiment and its failing assertion
remain untouched in `experiment/r2-capture-verify-release-20261015` and the
2026-10-15 capture–verify–release audit ledger. The revised test retains the
essential assertion that EmuWiz must not publish B as A.

Evidence and exact commands are kept outside the source tree in
`/home/davedap/emuwiz-audit-ledger/2026-10-15-r2-fd-publication/`.
Independent review of `ec370d045f3ba0d40edf2a850aa174bc72c15e7d` returned
PASS WITH CONDITIONS for publication and accepted bounded interruption tests.
Only then were the fresh-process tests added. The syscall implementation was
not changed. No production integration is authorized here.

The interruption tests use the existing transaction journal with test-only
phase annotations and a nonterminal `RollingBack` state. Workers exit without
destructors at six boundaries: intent, link before sync, link after sync before
recording, recorded link, retirement capture before recording, and recorded
capture. A separate process reads the receipt and observes each relevant name.
Four additional cases cover a corrupt receipt, lost names, a substituted
holding entry and an externally replaced destination.

These checks distinguish current observations, not authenticated complete
history. Even when destination and anchor both match A, the inspector grants
no resume authority, never unlinks a name and never reports completed Undo.
The output JSON is a test report, not a second recovery authority. Missing FDs
are not reconstructed from a directory scan. Hardware power loss, full
authority-store rollback and automatic retirement remain unproven.

References: [Linux link(2)](https://man7.org/linux/man-pages/man2/link.2.html),
[proc_pid_fd(5)](https://man7.org/linux/man-pages/man5/proc_pid_fd.5.html).
