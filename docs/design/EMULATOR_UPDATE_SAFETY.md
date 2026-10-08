# Emulator update, Undo and recovery: safety model and limits

Scope: the single-executable updater (`archivefs-core::emulator_update`) and its
Emulator Manager page. Emulator updates remain **not enabled for general use**;
this document records what the implementation guarantees and what it cannot.

## Exclusion

* One installation directory has one lock: a kernel `flock` on the **pinned
  directory inode** (`safety::acquire`). There is no lock file, so there is no
  lock pathname to rename, replace or unlink. Every opener of the directory
  inode, by any path, conflicts with the holder. The kernel drops the lock when
  the holder's descriptor closes, including on process death, so a crash before
  the first journal exists leaves nothing to clean up.
* Apply, Undo and recovery all take this same lock; recovery therefore cannot act
  on a live updater's journal.
* Replacing or renaming the directory does not move the lock to the new
  directory. The holder's `check()` fails (the path no longer names the locked
  inode) and the new directory is a different inode with different entries.
* Measured on ext4 and tmpfs with two real processes: the same directory inode
  is `BLOCKED` through the original path, through a symlink alias and after the
  directory was renamed; a *new* directory created at the old path is a different
  inode and can be locked (which is why record/installation bindings, not the
  lock, are what stop it from reaching the old directory's evidence); the lock
  is free as soon as the holder closes it.
* Because the lock is per directory, updates of two executables in one directory
  are serialised. A lock-file scheme used by an older build is not honoured by
  this one; the two must not run at the same time (the feature is not released).
* Local ext-family, Btrfs and tmpfs only. Network, FUSE and case-folding storage
  is refused, **before** any directory is created.

## Hard-linked executables

An executable with more than one hard link cannot be excluded by a per-directory
lock (another name may live in another directory) and moving one name would leave
the others on the same bytes. Planning marks it invalid; Apply, Undo and recovery
refuse before mutating (`UnsafeTarget`). Creating a new hard link between the
check and the move is a residual race (below).

## Ownership evidence

Content equality never establishes ownership. A record carries the device/inode of
the original and staged executables, the installation root binding and a per-target
sequence. Undo and mutating recovery require all of them; a record written before
these receipts existed is **review-only**: not offered as Undo, refused by Undo
and by recovery, never silently upgraded (missing identity is not fabricated).
Terminal states (`Published`, `RolledBack`, `Failed`) are written only after the
disk agrees with every recorded identity (`terminal_consistent`); contradictory
evidence stays `NeedsReconciliation`.

## Partial operations

After the first executable has moved, any stop (lost lock, changed installation
directory, process visibility becoming unknown, a changed backup) is reported as a
partial operation: the durable record stays in its in-flight state (`Undoing` or
`BackupMoved`) with the cause attached, the message says an executable had
already moved, and recovery is required. The Emulator Manager then re-reads the
saved records, discards the Undo snapshot, the review and both typed
confirmations, and shows "RECOVERY REQUIRED". Recovery is idempotent.

## Process visibility (what "stopped" can mean)

`Stopped` requires that every process be inspected. On ordinary Linux an
unprivileged user cannot read `/proc/<pid>/exe` of other users' processes. On the
development host used for the review, 666 of 852 processes denied it, so the
probe returns `Unknown` for any executable and the planner reports "cannot verify
the emulator is stopped", distinct from "emulator is running". This is
deliberate: an unreadable process is never treated as not running.

Consequence, stated plainly: **on a typical multi-user host an unprivileged
EmuWiz can rarely prove the emulator is stopped, so updates will usually be
unavailable.** Narrowing this safely (for example ignoring other-user processes
when the file has no group/other execute bit) still cannot exclude root-owned
processes, which exist on every system; any such relaxation is a design decision
for the project owner, not something to add as another check.

## Residual races (unchanged by this work)

* A process can start the emulator after the last probe; the probe and the lock
  do not stop external launches.
* Filesystem entries can be changed by any same-user process between a check and
  the following rename; moves are no-clobber and content/identity are re-verified
  after the fact, but a check-then-act window exists.
* A new hard link, or a swap of the directory path, can race the pre-move checks;
  the post-move identity verification and the journal make it recoverable.
