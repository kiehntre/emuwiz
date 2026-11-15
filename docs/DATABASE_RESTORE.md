# Verified database restore

Database restore is an explicit recovery operation. Normal startup and
read-only catalogue loading never restore a database.

## What the user sees

History lists every retained pre-upgrade backup. `Review restore` prepares a
plan from the selected backup; the plan shows the backup's schema and, when it
is older than this build, that a staging copy will be upgraded first. Typing
`RESTORE DATABASE` enables `Restore database`. A completed restore shows
`Review rollback`, which puts the pre-restore database back from the emergency
backup after the same confirmation. A restore that was interrupted (crash,
kill, power loss) shows `Reconcile interrupted restore`, which records what is
actually on disk without changing any database file. The History page is a
separate EmuWiz process from Blue, so the protections below are all
cross-process.

Blue's own History page (the quick view inside Blue) shows the same restore
receipts under "Database restore & recovery": the state of every restore,
interrupted restores flagged as "reconciliation required" (or "manual review
required" when core cannot classify them), `Review rollback` / `Roll back
restore` for completed restores and `Reconcile interrupted restore` for
interrupted ones. It reads the same `OperationRegistry` projection (a
metadata-only preview; it never opens or hashes the database) and calls the
same core executors with the same confirmation phrase, on a worker thread so
the page stays responsive. Its actions are additionally refused while Blue's
own catalogue load, scan or database upgrade is running; core's own refusals
(locks, freshness, identity) apply unchanged. A rollback reloads Blue's
library; reconciliation changes no database file.

## Writer exclusion

A restore must never lose a commit that SQLite acknowledged to another
connection, in this process or another. The executor therefore takes, before
it creates or changes anything, and holds until it has finished or rolled
back:

* the database **upgrade lock** (`<db>.upgrade.lock`, `flock`): no other
  EmuWiz process can open, create, upgrade or restore the database meanwhile
  (opens wait up to ten seconds, then refuse);
* a **`BEGIN IMMEDIATE` transaction** on the live file, i.e. SQLite's
  RESERVED lock: no connection anywhere can begin or commit a write. Nothing
  is written through this transaction, so it creates no rollback journal.
  Readers are tolerated; they keep the inode they opened.

A writer that already holds a transaction makes the restore **refuse** within
about two seconds; the restore never waits a writer out. A writer that was
between transactions gets `SQLITE_BUSY` on its next write and, after the
exchange, `SQLITE_READONLY_DBMOVED`: loud failures, never silent loss. In the
GUI the restore is additionally blocked while this window's catalogue is
loading, scanning, or running a source, alias, platform, missing-removal or
library-view action. The plan's freshness (size, mtime, SHA-256) is re-checked
under the lock, so a plan reviewed before a commit is refused as stale.

## Sidecars

The catalogue runs in DELETE journal mode. Before SQLite may open the live
file the executor inspects its sidecars with `symlink_metadata`:

* `-wal` or `-shm` present: refuse (a WAL belongs to the pathname and would be
  carried onto the wrong database);
* `-journal` with a live header while SQLite's RESERVED/PENDING lock is held:
  refuse as **busy**, a writer is mid-transaction;
* `-journal` with a live header and no visible lock: refuse with **"database
  activity cannot be determined"**. A crashed writer and a writer EmuWiz
  cannot see (another locking VFS, a network filesystem, an unreadable lock
  table) look identical from outside, so no claim of a crash is made and the
  journal is left untouched for the copy-first recovery procedure. EmuWiz
  never rolls a hot journal back silently.

The lock is read from `/proc/locks`, never with `fcntl` on a descriptor of
the database: on POSIX, closing any descriptor a process holds on a file
releases every record lock that process has on it, so a probe that opened
and closed the live database would drop this process's own SQLite locks
(the exclusion's, or a running scan's). For the same reason this module
never opens a plain descriptor on a live inode at all: hashing reads through
the descriptor SQLite already holds for a connection
(`sqlite3_file_control(SQLITE_FCNTL_FILE_POINTER)` and the VFS `xRead`),
whose lifetime SQLite manages across every connection of this process (its
unix VFS defers a close while any connection holds a lock on the inode and
closes the deferred descriptors when the last lock is released). Nothing is
retained by EmuWiz: after each restore or rollback this process holds no
descriptor on the database directory, so a superseded original's blocks are
freed as soon as it is unlinked. Restore, rollback and their reviews refuse
to start while a SQLite connection of this process holds any lock on the
live file (that window is not idle), and refuse when `/proc/locks` cannot be
read, rather than guessing. The normal open path uses the same
classification, so a live writer is reported as "busy" there too.

Known window: between the journal inspection and `BEGIN IMMEDIATE` a writer
that already had the file open could crash and leave a hot journal, which
SQLite would roll back while the exclusion's transaction begins. No
acknowledged commit can be lost by that (a DELETE-mode commit is the
journal's deletion), and the plan's hash is re-checked under the lock, so the
restore then refuses as stale.

## Staged upgrade of older backups

Every backup EmuWiz retains is a pre-upgrade backup, so it is usually at an
older schema. The selected backup is only ever opened read-only. It is copied
with SQLite's online-backup API into a staging file beside the live database,
verified at its own schema, migrated there through the same migration chain
the protected upgrade uses, fsynced and verified again (`quick_check`, schema,
required tables). A failed migration removes the staging copy and fails the
restore with the original untouched and the backup unchanged. Disk space for
the emergency backup, the staging copy and its migration journals is checked
first.

## Order of operations and the durable record

Every receipt (`<db>.restore-<plan id>.json`) is written atomically: temporary
file, fsync, rename, directory fsync. The receipt names the emergency backup
and the staging file before either exists, and records the staging file's
SHA-256 and inode before the exchange, so an interruption at any point can be
classified afterwards.

1. receipt `Validating`;
2. exclusion (above), then plan validation under the lock, including
   `quick_check` of the live database;
3. emergency backup of the live database (online backup, fsync, verify),
   receipt `Emergency backup created`;
4. staging copy (and upgrade), receipt `Restore staged` with hash and inode;
5. final sidecar check, then the `renameat2(RENAME_EXCHANGE)` syscall of
   staging and live paths. This syscall is the only destructive boundary: if
   it fails nothing was exchanged; if it succeeds everything after it,
   including the directory fsync and receipt writes, is a post-exchange step.
   Receipt `Restore applied` names the superseded original's path;
6. directory fsync and verification of the restored file (inode and hash
   match the staged copy, `quick_check`, schema, tables); on any failure the
   superseded original, still at the staging path, is identity- and
   hash-checked and exchanged back (`Rolled back`); if the exchange back
   fails the receipt says the live file is the rejected replacement and names
   the original's path and the emergency backup (`Rollback failed`); if the
   exchange back succeeds but the original then fails verification the
   receipt says the original is back at the live path, names the rejected
   replacement at the staging path, and warns not to copy it over;
7. the superseded original is unlinked (its content is in the verified
   emergency backup), receipt `Completed`. A failure in this tidy-up does not
   fail a restore that happened: the receipt is `Completed` with the
   original's remaining path and a warning in the message.

Any failure before step 5 finalises the receipt as `Failed` with
`original_intact = true`. Receipt writes after the exchange are best effort:
a failed write is reported as a warning in the result, never as a failed
restore and never by stopping before verification.

## Recovery after restart

`interrupted_restore_receipts` lists receipts that never reached a terminal
state. Classification is by **file identity** (device and inode), which the
receipt records for the original, the staged replacement and a rollback
copy before the corresponding exchange, so it stays correct after the
database has been used again:

* the staged hash was never recorded, or the live pathname carries the
  original inode: the exchange never happened (or was undone), the original
  is in place; `modified_since` says whether it has been written since the
  receipt's hash; leftover emergency/staging files are listed, never deleted;
* the live pathname carries the replacement inode: the replacement is in
  place; `assess_interrupted_restore` verifies it (`quick_check`, schema,
  tables) and reports whether it has been used since;
* a `Rollback attempted` receipt is judged the same way against the restored
  database's identity and the rollback copy's identity;
* otherwise: manual review.

`preview_interrupted_restore` does the same from metadata only (no database
file is opened or hashed) and is what History shows, because History is
rebuilt every frame. `finalise_interrupted_restore` repeats the full
assessment under the exclusion and only rewrites the receipt (`Failed` with
the original intact, `Completed`, or `Rolled back`).

## Rollback

`review_database_rollback` checks, read-only, that the emergency backup exists
with its recorded hash and that the live database is still the restored
replacement. `rollback_database_restore` then takes the exclusion, records
the rollback staging path (and, once staged, its hash and inode) in the
receipt before the exchange, exchanges, fsyncs, verifies the rollback copy by
identity and `quick_check`, and records `Rolled back`; a failed verification
exchanges the restored database back (`Completed` again, with the reason), and
if that exchange fails the receipt says the live file is the unverified
rollback copy and where the restored database is preserved. An interrupted
rollback is classified and finalised like an interrupted restore.

## Limitations

* Readers in other processes keep their consistent view of the old inode
  until they reopen; the GUI reloads its own catalogue and says a restart may
  be needed elsewhere. Writers in other processes fail loudly rather than
  being paused.
* The restore runs on the History window's thread, so a very large database
  makes that window unresponsive for the duration (hashing is streamed, not
  loaded into memory).
* The emergency backup is recorded in the restore receipt, not under a backup
  ownership receipt, so it is offered through `Review rollback` rather than as
  a History backup entry.
* `/proc/locks` entries are matched by inode number only (device numbers are
  not stable across btrfs/overlay mounts); a lock on another filesystem's
  file with the same inode number can only cause a spurious refusal.
* A SQLite writer using a locking VFS without record locks (or on a network
  filesystem) is invisible to both the kernel lock table and the RESERVED
  lock; its journal is reported as "activity cannot be determined".
* `renameat2(RENAME_EXCHANGE)` is Linux-only; other platforms refuse before
  any change.
